//! The IRC connections: one per enabled IRC source. Each signs in (NICK/USER, then NickServ and the tracker's
//! invite command when set), joins the announce channels, reads what the announcers say and hands every
//! announce to [`Releases::handle`] at once, so a release is pushed seconds after it is announced. A dropped
//! connection comes back by itself, waiting a little longer each time (5 s up to 5 min).
//!
//! Nothing sent to the server is logged (it carries the NickServ password and the invite key).

use super::announce::{self, LineReader};
use super::{Releases, Source};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// How one IRC connection is doing, as the API shows it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Status {
    /// Signed in on the server now.
    pub connected: bool,
    /// The server it talks to (`irc.tracker.example:6697`).
    pub server: String,
    /// The nick it has.
    pub nick: String,
    /// The channels it is in.
    pub channels: Vec<String>,
    /// Announces read since it started.
    pub announces: u64,
    /// When the last announce came (0: none yet).
    pub last_announce: f64,
    /// The last problem, in words.
    pub last_error: String,
}

/// One running connection: its task and its state.
type Running = (tokio::task::AbortHandle, Arc<Mutex<Status>>);

/// The running connections, by source id.
#[derive(Clone, Default)]
pub struct Runner {
    running: Arc<Mutex<HashMap<i64, Running>>>,
}

impl Runner {
    /// Stops source `id`'s connection, if it has one.
    pub fn stop(&self, id: i64) {
        if let Some((h, _)) = self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            h.abort();
        }
    }

    /// Starts `s`'s connection afresh (stopping the one it had); a disabled source just stops.
    pub fn restart(&self, releases: Releases, s: Source) {
        self.stop(s.id);
        if !s.enabled {
            return;
        }
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();
        let id = s.id;
        let task = tokio::spawn(async move { run(releases, s, st).await });
        self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(id, (task.abort_handle(), status));
    }

    /// Source `id`'s state (all empty when it doesn't run).
    pub fn status(&self, id: i64) -> Status {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|(_, s)| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .unwrap_or_default()
    }
}

/// Where to connect and as whom: the definition's network with the source's own choices on top.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The host.
    pub server: String,
    /// The port.
    pub port: u16,
    /// Use TLS.
    pub tls: bool,
    /// The nick.
    pub nick: String,
    /// The server password (a bouncer's), if any.
    pub pass: String,
    /// NickServ account and password, if any.
    pub nickserv: Option<(String, String)>,
    /// The invite command: who gets it and what (`Bot`, `invite KEY`).
    pub invite: Option<(String, String)>,
    /// The channels and their keys.
    pub channels: Vec<(String, String)>,
    /// The nicks that announce (lowercase; empty: anyone in the channels).
    pub announcers: Vec<String>,
}

/// The plan for source `s` with its definition; the problem in words.
pub fn plan(releases: &Releases, s: &Source) -> Result<(Plan, Arc<announce::Definition>), String> {
    let def = releases.library.get(&s.definition).ok_or_else(|| format!("The announce definition {} is missing", s.definition))?;
    let irc = def.irc.as_ref().ok_or_else(|| format!("{} doesn't announce on IRC", def.name))?;
    let set = |k: &str| s.settings.get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let channels: Vec<(String, String)> = match set("channels") {
        Some(list) => list.split(',').map(|c| c.trim()).filter(|c| !c.is_empty()).map(split_key).collect(),
        None => irc.channels.iter().map(|c| split_key(c)).collect(),
    };
    let invite = set("invite_command").and_then(|c| {
        let c = c.trim_start_matches("/msg ").trim_start_matches("/MSG ").trim().to_string();
        let (to, what) = c.split_once(' ')?;
        Some((to.to_string(), what.trim().to_string()))
    });
    let nickserv = match (set("auth.account").or_else(|| set("nick")), set("auth.password")) {
        (Some(a), Some(p)) => Some((a, p)),
        _ => None,
    };
    Ok((
        Plan {
            server: set("server").unwrap_or_else(|| irc.server.clone()),
            port: set("port").and_then(|p| p.parse().ok()).unwrap_or(if irc.port == 0 { 6667 } else { irc.port }),
            tls: set("tls").map(|t| t == "true" || t == "1").unwrap_or(irc.tls),
            nick: set("nick").ok_or("Choose the nick Lantern uses on IRC")?,
            pass: set("pass").unwrap_or_default(),
            nickserv,
            invite,
            channels,
            announcers: irc.announcers.iter().map(|a| a.to_lowercase()).collect(),
        },
        def,
    ))
}

/// `#chan key` as the channel and its key.
fn split_key(c: &str) -> (String, String) {
    match c.trim().split_once(' ') {
        Some((n, k)) => (n.to_string(), k.trim().to_string()),
        None => (c.trim().to_string(), String::new()),
    }
}

/// Keeps source `s` connected until it is stopped.
async fn run(releases: Releases, s: Source, status: Arc<Mutex<Status>>) {
    let mut wait = 5u64;
    loop {
        let started = std::time::Instant::now();
        let problem = match plan(&releases, &s) {
            Ok((p, def)) => match session(&releases, &s, &p, def, &status).await {
                Ok(()) => "The IRC server closed the connection".to_string(),
                Err(e) => e,
            },
            Err(e) => e,
        };
        {
            let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
            st.connected = false;
            st.channels.clear();
            st.last_error = problem.clone();
        }
        releases.source_result(s.id, &format!("{problem}. Lantern tries again by itself"));
        tracing::warn!("IRC {}: {problem}", s.name);
        if started.elapsed() > Duration::from_secs(120) {
            wait = 5;
        }
        tokio::time::sleep(Duration::from_secs(wait)).await;
        wait = (wait * 2).min(300);
    }
}

/// Something IRC can be spoken over: a TCP or TLS stream.
trait Wire: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Wire for T {}

/// Connects to the plan's server, with TLS when it asks.
async fn connect(p: &Plan) -> Result<Box<dyn Wire>, String> {
    let tcp = tokio::time::timeout(Duration::from_secs(20), tokio::net::TcpStream::connect((p.server.as_str(), p.port)))
        .await
        .map_err(|_| format!("{}:{} didn't answer within 20 seconds", p.server, p.port))?
        .map_err(|e| format!("{}:{} can't be reached: {e}", p.server, p.port))?;
    let _ = tcp.set_nodelay(true);
    if !p.tls {
        return Ok(Box::new(tcp));
    }
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config =
        tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS can't be set up: {e}"))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(p.server.clone())
        .map_err(|_| format!("{} is not a server name TLS can check", p.server))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| {
        format!("The secure connection to {} failed: {e}. If the server has no TLS, turn TLS off for this source", p.server)
    })?;
    Ok(Box::new(tls))
}

/// One IRC message: its sender's nick, command and parameters (the last one may have spaces).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    /// The sender's nick (empty for the server's own lines without a prefix).
    pub nick: String,
    /// `PRIVMSG`, `PING`, `001` …
    pub command: String,
    /// The parameters.
    pub params: Vec<String>,
}

/// Reads one IRC line (tags dropped).
pub fn parse_line(line: &str) -> Message {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    if rest.starts_with('@') {
        rest = rest.split_once(' ').map(|(_, r)| r).unwrap_or("");
    }
    let mut nick = String::new();
    if let Some(r) = rest.strip_prefix(':') {
        let (prefix, r) = r.split_once(' ').unwrap_or((r, ""));
        nick = prefix.split('!').next().unwrap_or("").to_string();
        rest = r;
    }
    let (head, trailing) = match rest.split_once(" :") {
        Some((h, t)) => (h, Some(t)),
        None => (rest, None),
    };
    let mut words = head.split_whitespace();
    let command = words.next().unwrap_or("").to_uppercase();
    let mut params: Vec<String> = words.map(str::to_string).collect();
    if let Some(t) = trailing {
        params.push(t.to_string());
    }
    Message { nick, command, params }
}

/// One connection, from sign-in until it drops.
async fn session(
    releases: &Releases,
    s: &Source,
    p: &Plan,
    def: Arc<announce::Definition>,
    status: &Arc<Mutex<Status>>,
) -> Result<(), String> {
    let wire = connect(p).await?;
    let (r, mut w) = tokio::io::split(wire);
    let mut lines = BufReader::new(r).lines();
    let mut nick = p.nick.clone();
    {
        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
        st.server = format!("{}:{}", p.server, p.port);
        st.nick = nick.clone();
        st.last_error.clear();
    }
    let mut out = String::new();
    if !p.pass.is_empty() {
        out.push_str(&format!("PASS {}\r\n", p.pass));
    }
    out.push_str(&format!("NICK {nick}\r\nUSER {} 0 * :Lantern\r\n", p.nick.split('|').next().unwrap_or(&p.nick)));
    send(&mut w, &out).await?;
    let mut reader = LineReader::new(def.clone());
    let wanted: Vec<String> = p.channels.iter().map(|(c, _)| c.to_lowercase()).collect();
    let mut pinged = false;
    loop {
        let line = match tokio::time::timeout(Duration::from_secs(240), lines.next_line()).await {
            Err(_) if !pinged => {
                pinged = true;
                send(&mut w, "PING :lantern\r\n").await?;
                continue;
            }
            Err(_) => return Err("The IRC server stopped answering".into()),
            Ok(Err(e)) => return Err(format!("The IRC connection broke: {e}")),
            Ok(Ok(None)) => return Ok(()),
            Ok(Ok(Some(l))) => l,
        };
        pinged = false;
        let m = parse_line(&line);
        match m.command.as_str() {
            "PING" => send(&mut w, &format!("PONG :{}\r\n", m.params.last().cloned().unwrap_or_default())).await?,
            "001" => {
                status.lock().unwrap_or_else(|e| e.into_inner()).connected = true;
                releases.source_result(s.id, "");
                if let Some((account, pass)) = &p.nickserv {
                    send(&mut w, &format!("PRIVMSG NickServ :IDENTIFY {account} {pass}\r\n")).await?;
                }
                if let Some((to, what)) = &p.invite {
                    send(&mut w, &format!("PRIVMSG {to} :{what}\r\n")).await?;
                }
                if p.nickserv.is_some() || p.invite.is_some() {
                    // give NickServ and the invite bot a moment before joining
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                let mut join = String::new();
                for (c, k) in &p.channels {
                    join.push_str(&if k.is_empty() { format!("JOIN {c}\r\n") } else { format!("JOIN {c} {k}\r\n") });
                }
                send(&mut w, &join).await?;
            }
            "433" => {
                // the nick is taken: try another until NickServ can sort it out
                nick.push('_');
                status.lock().unwrap_or_else(|e| e.into_inner()).nick = nick.clone();
                send(&mut w, &format!("NICK {nick}\r\n")).await?;
            }
            "JOIN" if m.nick.eq_ignore_ascii_case(&nick) => {
                if let Some(c) = m.params.first() {
                    let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                    if !st.channels.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                        st.channels.push(c.clone());
                    }
                }
            }
            "INVITE" => {
                if let Some(c) = m.params.get(1) {
                    if wanted.contains(&c.to_lowercase()) {
                        send(&mut w, &format!("JOIN {c}\r\n")).await?;
                    }
                }
            }
            "KICK" if m.params.get(1).is_some_and(|n| n.eq_ignore_ascii_case(&nick)) => {
                if let Some(c) = m.params.first() {
                    status.lock().unwrap_or_else(|e| e.into_inner()).channels.retain(|x| !x.eq_ignore_ascii_case(c));
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    send(&mut w, &format!("JOIN {c}\r\n")).await?;
                }
            }
            "471" | "473" | "474" | "475" => {
                let why = m.params.last().cloned().unwrap_or_default();
                let c = m.params.get(1).cloned().unwrap_or_default();
                let problem = format!("Lantern can't join {c}: {why}. Check the invite command and the NickServ password");
                status.lock().unwrap_or_else(|e| e.into_inner()).last_error = problem.clone();
                releases.source_result(s.id, &problem);
            }
            "ERROR" => return Err(format!("The IRC server ended the connection: {}", m.params.last().cloned().unwrap_or_default())),
            "PRIVMSG" | "NOTICE" => {
                let (Some(target), Some(text)) = (m.params.first(), m.params.get(1)) else { continue };
                if !wanted.contains(&target.to_lowercase()) {
                    continue;
                }
                if !p.announcers.is_empty() && !p.announcers.contains(&m.nick.to_lowercase()) {
                    continue;
                }
                if let Some(vars) = reader.feed(text) {
                    match announce::found(&def, vars, &s.settings) {
                        Ok(mut f) => {
                            f.source_id = s.id;
                            f.source_name = s.name.clone();
                            {
                                let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                                st.announces += 1;
                                st.last_announce = crate::auth::now();
                            }
                            let r = releases.clone();
                            tokio::spawn(async move {
                                r.handle(f).await;
                            });
                        }
                        Err(e) => tracing::warn!("IRC {}: {e}", s.name),
                    }
                }
            }
            _ => {}
        }
    }
}

/// Writes `text` to the server.
async fn send<W: AsyncWrite + Unpin>(w: &mut W, text: &str) -> Result<(), String> {
    w.write_all(text.as_bytes()).await.map_err(|e| format!("The IRC connection broke: {e}"))?;
    w.flush().await.map_err(|e| format!("The IRC connection broke: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_irc_lines() {
        let m = parse_line(":Keeper!bot@tracker PRIVMSG #announce :New Torrent: a b c\r\n");
        assert_eq!(
            m,
            Message { nick: "Keeper".into(), command: "PRIVMSG".into(), params: vec!["#announce".into(), "New Torrent: a b c".into()] }
        );
        let m = parse_line("PING :irc.example");
        assert_eq!((m.command.as_str(), m.params[0].as_str()), ("PING", "irc.example"));
        let m = parse_line("@time=x :srv 001 me :Welcome");
        assert_eq!((m.nick.as_str(), m.command.as_str()), ("srv", "001"));
        assert_eq!(split_key("#chan key"), ("#chan".into(), "key".into()));
    }
}
