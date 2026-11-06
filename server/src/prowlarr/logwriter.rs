// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Instrumentation/DatabaseTarget.cs, ReconfigureLogging.cs,
// src/NzbDrone.Common/Instrumentation/NzbDroneLogger.cs, CleanseLogMessage.cs, CleansingFileTarget.cs,
// src/NzbDrone.Common/Extensions/IpAddressExtensions.cs), GPL-3.0.

//! The log writer: what Lantern logs goes, with its secrets taken out, to the log table that `/api/v1/log` pages
//! through and to the log files `/api/v1/log/file` lists, as Prowlarr's NLog targets do:
//!
//! - the table (`prowlarr_logs`): Info and above;
//! - `logs/lantern.txt`: Info and above, `logs/lantern.debug.txt` from Debug and `logs/lantern.trace.txt` from
//!   Trace, each only when the `LogLevel` setting reaches it. A file that grows past `LogSizeLimit` megabytes is
//!   moved to `lantern.0.txt` (the older ones to `.1`, `.2`, …) and only `LogRotate` old files are kept.
//!
//! Entries are written in batches every half second (Prowlarr's `SlowRunningAsyncTargetWrapper`), from a thread of
//! their own, so logging never waits on the database. What is logged before the writer is attached is kept (up to
//! a thousand entries) and written once it is.

use super::config;
use kit_server_core::{p, Db, SqliteDb};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;

/// A log level, from the most detailed (NLog's names, as Prowlarr writes them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Every step.
    Trace,
    /// What helps find a problem.
    Debug,
    /// What happened.
    Info,
    /// Something to look at.
    Warn,
    /// Something failed.
    Error,
    /// Lantern can't go on.
    Fatal,
}

impl Level {
    /// The level's name as Prowlarr writes it (`Info`).
    pub fn name(self) -> &'static str {
        match self {
            Level::Trace => "Trace",
            Level::Debug => "Debug",
            Level::Info => "Info",
            Level::Warn => "Warn",
            Level::Error => "Error",
            Level::Fatal => "Fatal",
        }
    }

    /// The level named `name` in any case (`LogLevel.FromString`); `None` for `off` or a name it doesn't know.
    pub fn parse(name: &str) -> Option<Level> {
        match name.trim().to_lowercase().as_str() {
            "trace" => Some(Level::Trace),
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            "fatal" => Some(Level::Fatal),
            _ => None,
        }
    }

    /// A tracing level as a log level.
    fn of(level: &tracing::Level) -> Level {
        match *level {
            tracing::Level::TRACE => Level::Trace,
            tracing::Level::DEBUG => Level::Debug,
            tracing::Level::INFO => Level::Info,
            tracing::Level::WARN => Level::Warn,
            _ => Level::Error,
        }
    }
}

/// One log entry (Prowlarr's `Log`).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// When, seconds since 1970.
    pub time: f64,
    /// How serious it is.
    pub level: Level,
    /// Which part of Lantern logged it (the module, without `server::`).
    pub logger: String,
    /// What happened.
    pub message: String,
    /// The error behind it, in full, when there is one.
    pub exception: Option<String>,
    /// The error's kind, when there is one.
    pub exception_type: Option<String>,
}

// ---- CleanseLogMessage ----

/// One of Prowlarr's cleansing rules: its pattern (groups named `secret…` are replaced by `(removed)`), and for a
/// rule whose secret is in a repeated group, the pattern of each repeat inside the match.
struct Rule {
    /// The pattern.
    re: fancy_regex::Regex,
    /// Each repeat's pattern, for a repeated group (.NET keeps every repeat's capture; Rust only the last).
    inner: Option<fancy_regex::Regex>,
}

/// Prowlarr's `CleansingRules`, in its order. Where .NET reads a lookbehind of more than one length, it is written
/// as one lookbehind per length, or as a plain prefix the replacement leaves alone; a group name used twice is
/// `secret` and `secret2`.
fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let r = |p: &str| Rule { re: fancy_regex::Regex::new(p).expect("a fixed pattern"), inner: None };
        let mut iptorrents = r(r"(?i)iptorrents\.com/[/a-z0-9?&;]*?(?:[?&;](u|tp)=(?<secret>[^&=;]+?))+(?= |;|&|$)");
        iptorrents.inner = Some(fancy_regex::Regex::new(r"(?i)[?&;](?:u|tp)=(?<secret>[^&=;]+?)(?= |;|&|[?]|$)").expect("a fixed pattern"));
        vec![
            // Url
            r(r#"(?i)(?<=[?&: ;])(apikey|api_key|(?:(?:access|api)[-_]?)?token|pass(?:key|wd)?|auth|authkey|rsskey|user|u?id|api|[a-z_]*apikey|account|pid|pwd)=(?<secret>[^&="]+?)(?=[ "&=]|$)"#),
            r(r"(?i)(?<=[?& ;])[^=]*?(_?(?<!use)(?<!get_)token|username|passwo?rd)=(?<secret>[^&=]+?)(?= |&|$|;)"),
            r(r"(?i)rss(24h)?\.torrentleech\.org/(?!rss)(?<secret>[0-9a-z]+)"),
            r(r"(?i)torrentleech\.org/rss/download/[0-9]+/(?<secret>[0-9a-z]+)"),
            iptorrents,
            r(r"/fetch/[a-z0-9]{32}/(?<secret>[a-z0-9]{32})"),
            r(r"(?i)getnzb.*?(?<=\?|&)(r)=(?<secret>[^&=]+?)(?= |&|$)"),
            r(r"(?i)\b(\w*)?(_?(?<!use)(?<!get_)token|username|passwo?rd)=(?<secret>[^&=]+?)(?= |&|$|;)"),
            r(r#"(?i)(?<=authkey = ")(?<secret>[^&=]+?)(?=")"#),
            r(r"(?i)(?:beyond-hd\.[a-z]+/api/torrents/)(?<secret>[^&=][a-z0-9]+)"),
            r(r"(?i)(?:beyond-hd\.[a-z]+/torrent/download/[\w\d-]+[.]\d+[.])(?<secret>[a-z0-9]+)"),
            r(r"(?i)(?:sharewood)\.[a-z]{2,3}/api/(?<secret>[a-z0-9]{16,})/"),
            // UNIT3D
            r(r"(?i)(?:[a-z0-9-]+\.[a-z]+/torrent/download/\d+\.)(?<secret>[^&=][a-z0-9]+)"),
            // Path
            r(r#"(?i)"C:\\Users\\(?<secret>[^"]+?)(\\|$)"#),
            r(r#"(?i)"/(home|Users)/(?<secret>[^/"]+?)(/|$)"#),
            // Trackers Announce Keys; Designed for Qbit Json; should work for all in theory
            r(r"(?i)announce(\.php)?(/|%2f|%3fpasskey%3d)(?<secret>[a-z0-9]{16,})|(?<secret2>[a-z0-9]{16,})(/|%2f)announce"),
            // NzbGet
            r(r#"(?i)"Name"\s*:\s*"[^"]*(username|password)"\s*,\s*"Value"\s*:\s*"(?<secret>[^"]+?)""#),
            // Sabnzbd
            r(r#"(?i)"[^"]*(username|password|api_?key|nzb_key)"\s*:\s*"(?<secret>[^"]+?)""#),
            r(r#"(?i)"email_(account|to|from|pwd)"\s*:\s*"(?<secret>[^"]+?)""#),
            // uTorrent
            r(r#"(?i)\["[a-z._]*(username|password)",\d,"(?<secret>[^"]+?)""#),
            r(r#"(?i)\["(boss_key|boss_key_salt|proxy\.proxy)",\d,"(?<secret>[^"]+?)""#),
            // Deluge
            r(r#"(?i)auth.login\("(?<secret>[^"]+?)""#),
            // BroadcastheNet (;torrent_pass|torrents_notify_ is for MTV)
            r(r#"(?i)"?method"?\s*:\s*"(getTorrents)",\s*"?params"?\s*:\s*\[\s*"(?<secret>[^"]+?)""#),
            r(r#"(?i)getTorrents\("(?<secret>[^"]+?)""#),
            r(r#"(?i)(?<=\?|&|;|=)(authkey|torrent_pass|torrents_notify)[_=](?<secret>[^&=]+?)(?="|&|$)"#),
            // Plex
            r(r"(?i)(?<=\?|&)(X-Plex-Client-Identifier|X-Plex-Token)=(?<secret>[^&=]+?)(?= |&|$)"),
            // Indexer Responses
            r(r"(?i)(?:avistaz|exoticaz|cinemaz|privatehd)\.[a-z]{2,3}/rss/download/(?<secret>[^&=]+?)/(?<secret2>[^&=]+?)\.torrent"),
            r(r#"(?i)(?:animebytes)\.[a-z]{2,3}/torrent/[0-9]+/download/(?<secret>[^&=]+?)["]"#),
            r(r#"(?i)"(info_hash|token|((pass|rss)[- _]?key))":"(?<secret>[^&=]+?)""#),
            // Applications
            r(r#"(?i)"name":"apikey","value":"(?<secret>[^&=]+?)""#),
            // Discord
            r(r"(?i)discord.com/api/webhooks/((?<secret>[\w-]+)/)?(?<secret2>[\w-]+)"),
        ]
    })
}

/// The ranges of `caps`' groups named `secret…` that took part, relative to `base`.
fn secret_ranges(re: &fancy_regex::Regex, caps: &fancy_regex::Captures, base: usize) -> Vec<(usize, usize)> {
    re.capture_names()
        .enumerate()
        .filter(|(_, n)| n.is_some_and(|n| n.starts_with("secret")))
        .filter_map(|(i, _)| caps.get(i).map(|m| (m.start() - base, m.end() - base)))
        .collect()
}

/// `text` with each range replaced by `(removed)`.
fn remove_ranges(text: &str, mut ranges: Vec<(usize, usize)>) -> String {
    ranges.sort();
    ranges.dedup();
    let mut out = text.to_string();
    for (start, end) in ranges.into_iter().rev() {
        out.replace_range(start..end, "(removed)");
    }
    out
}

/// One rule over `message` (`Regex.Replace` with Prowlarr's evaluator: each match with its secrets removed).
fn apply(rule: &Rule, message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut last = 0;
    for caps in rule.re.captures_iter(message).flatten() {
        let Some(m) = caps.get(0) else { continue };
        let value = &message[m.start()..m.end()];
        let ranges = match &rule.inner {
            Some(inner) => inner.captures_iter(value).flatten().flat_map(|c| secret_ranges(inner, &c, 0)).collect(),
            None => secret_ranges(&rule.re, &caps, m.start()),
        };
        out.push_str(&message[last..m.start()]);
        out.push_str(&remove_ranges(value, ranges));
        last = m.end();
    }
    out.push_str(&message[last..]);
    out
}

/// Prowlarr's `CleanseLogMessage.Cleanse`: API keys, passwords, passkeys, tokens, user folders and the like taken
/// out of `message`, and outside addresses shortened to their first and last number (`32.*.*.5`).
pub fn cleanse(message: &str) -> String {
    if message.trim().is_empty() {
        return message.to_string();
    }
    let mut out = message.to_string();
    for rule in rules() {
        out = apply(rule, &out);
    }
    cleanse_remote_ips(&out)
}

/// `CleanseRemoteIPRegex` and `CleanseRemoteIP`: a sign-in's or a request's address, unless it is a local one.
fn cleanse_remote_ips(message: &str) -> String {
    static RE: OnceLock<fancy_regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        fancy_regex::Regex::new(r"(?:Auth-\w+(?<!Failure)(?<!Unauthorized) ip|from) (\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})")
            .expect("a fixed pattern")
    });
    let mut out = String::with_capacity(message.len());
    let mut last = 0;
    for caps in re.captures_iter(message).flatten() {
        let (Some(m), Some(g)) = (caps.get(0), caps.get(1)) else { continue };
        out.push_str(&message[last..m.start()]);
        match g.as_str().parse::<std::net::Ipv4Addr>() {
            Ok(ip) if !is_local(ip) => {
                let o = ip.octets();
                out.push_str(&message[m.start()..g.start()]);
                out.push_str(&format!("{}.*.*.{}", o[0], o[3]));
                out.push_str(&message[g.end()..m.end()]);
            }
            _ => out.push_str(m.as_str()),
        }
        last = m.end();
    }
    out.push_str(&message[last..]);
    out
}

/// `IPAddressExtensions.IsLocalAddress` for an IPv4 address: loopback, link-local or a private range.
fn is_local(ip: std::net::Ipv4Addr) -> bool {
    let b = ip.octets();
    ip.is_loopback()
        || (b[0] == 169 && b[1] == 254)
        || b[0] == 10
        || (b[0] == 172 && (16..=31).contains(&b[1]))
        || (b[0] == 192 && b[1] == 168)
}

// ---- the targets ----

/// Prowlarr's log files: (name, level it starts at).
pub const FILES: [(&str, Level); 3] =
    [("lantern.txt", Level::Info), ("lantern.debug.txt", Level::Debug), ("lantern.trace.txt", Level::Trace)];

/// Where the entries go: the database's log table and the log folder.
#[derive(Clone)]
pub struct Writer {
    /// The database with `prowlarr_logs` and the logging settings.
    pub db: SqliteDb,
    /// The log folder (`<data>/logs`).
    pub folder: PathBuf,
}

impl Writer {
    /// Writes `entries` to the table and the files, as the settings say now (`ReconfigureLogging`).
    pub fn write(&self, entries: &[Entry]) {
        if entries.is_empty() {
            return;
        }
        let min = Level::parse(&config::log_level(&self.db)).unwrap_or(Level::Info);
        MAX_LEVEL.store(min as u8, Ordering::Relaxed);
        // DatabaseTarget: Info and above
        for e in entries.iter().filter(|e| e.level >= Level::Info) {
            let _ = self.db.execute(
                "INSERT INTO prowlarr_logs (message, time, logger, exception, exception_type, level) VALUES (?, ?, ?, ?, ?, ?)",
                p![cleanse(&e.message), e.time, e.logger, e.exception.as_deref().map(cleanse), e.exception_type, e.level.name()],
            );
        }
        let limit = config::log_size_limit(&self.db) as u64 * 1024 * 1024;
        let keep = config::log_rotate(&self.db).max(0) as usize;
        // appFileInfo, appFileDebug, appFileTrace: each from its level, when LogLevel reaches it
        for (name, from) in FILES {
            if min > from {
                continue;
            }
            let lines: String = entries.iter().filter(|e| e.level >= from).map(line).collect();
            if !lines.is_empty() {
                if let Err(e) = append(&self.folder, name, &lines, limit, keep) {
                    eprintln!("The log file {name} can't be written: {e}");
                }
            }
        }
    }
}

/// An entry as a line of Prowlarr's `FileLogLayout`: `yyyy-MM-dd HH:mm:ss.f|Level|logger|message`, then the
/// error on lines of its own.
pub fn line(e: &Entry) -> String {
    let time = chrono::DateTime::from_timestamp_millis((e.time * 1000.0) as i64)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S%.f").to_string())
        .unwrap_or_default();
    // .f: tenths of a second
    let time = match time.find('.') {
        Some(dot) => time[..(dot + 2).min(time.len())].to_string(),
        None => format!("{time}.0"),
    };
    let mut out = format!("{time}|{}|{}|{}", e.level.name(), e.logger, cleanse(&e.message));
    if let Some(x) = &e.exception {
        out.push_str(&format!("\n\n[v{}] {}\n\n", crate::VERSION, cleanse(x)));
    }
    out.push('\n');
    out
}

/// Adds `text` to the log file `name` in `folder`; a file at `limit` bytes or more is archived first (rolling:
/// `name.0.txt` is the newest old file), keeping `keep` old files.
pub fn append(folder: &Path, name: &str, text: &str, limit: u64, keep: usize) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(name);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size > 0 && size + text.len() as u64 > limit {
        archive(folder, name, keep)?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(text.as_bytes())
}

/// The name of old file number `n` of `name` (`lantern.txt` → `lantern.0.txt`).
pub fn archive_name(name: &str, n: usize) -> String {
    match name.strip_suffix(".txt") {
        Some(stem) => format!("{stem}.{n}.txt"),
        None => format!("{name}.{n}"),
    }
}

/// Moves `name` to old file 0, the older ones up by one; the ones past `keep` are deleted.
fn archive(folder: &Path, name: &str, keep: usize) -> std::io::Result<()> {
    if keep == 0 {
        return std::fs::remove_file(folder.join(name));
    }
    let _ = std::fs::remove_file(folder.join(archive_name(name, keep - 1)));
    for n in (0..keep - 1).rev() {
        let from = folder.join(archive_name(name, n));
        if from.exists() {
            std::fs::rename(&from, folder.join(archive_name(name, n + 1)))?;
        }
    }
    // a gap left by an older keep setting: files past it go
    let mut n = keep;
    while folder.join(archive_name(name, n)).exists() {
        let _ = std::fs::remove_file(folder.join(archive_name(name, n)));
        n += 1;
    }
    std::fs::rename(folder.join(name), folder.join(archive_name(name, 0)))
}

// ---- the tracing layer ----

/// The most detailed level the writer takes now (the `LogLevel` setting, Info and above for everything else).
static MAX_LEVEL: AtomicU8 = AtomicU8::new(Level::Debug as u8);

/// The channel to the writer's thread, and what was logged before it started.
static SINK: Mutex<Sink> = Mutex::new(Sink { tx: None, early: Vec::new() });

/// Where the layer hands its entries.
struct Sink {
    /// The writer's thread, once it runs.
    tx: Option<mpsc::Sender<Entry>>,
    /// Entries logged before it runs.
    early: Vec<Entry>,
}

/// How many entries logged before the writer starts are kept.
const EARLY_MAX: usize = 1000;

/// Starts the writer's thread on `writer` (once; later calls do nothing) and hands it what was logged so far.
pub fn attach(writer: Writer) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    MAX_LEVEL.store(Level::parse(&config::log_level(&writer.db)).unwrap_or(Level::Info) as u8, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<Entry>();
    let early = {
        let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
        sink.tx = Some(tx);
        std::mem::take(&mut sink.early)
    };
    std::thread::spawn(move || {
        writer.write(&early);
        while let Ok(first) = rx.recv() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            writer.write(&batch);
        }
    });
}

/// Hands an entry to the writer.
fn send(e: Entry) {
    let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
    match &sink.tx {
        Some(tx) => {
            let _ = tx.send(e);
        }
        None if sink.early.len() < EARLY_MAX => sink.early.push(e),
        None => {}
    }
}

/// The logger name of a tracing target: the module without `server::` (Prowlarr leaves out `NzbDrone.`).
pub fn logger_of(target: &str) -> String {
    target.strip_prefix("server::").unwrap_or(target).to_string()
}

/// Whether an event at `level` from `target` goes to the writer: Lantern's own from the `LogLevel` setting on,
/// other libraries' from Warn on.
pub fn wanted(target: &str, level: Level) -> bool {
    let own = target == "server" || target.starts_with("server::") || target.starts_with("lantern");
    let min = if own { level_from(MAX_LEVEL.load(Ordering::Relaxed)) } else { Level::Warn };
    level >= min
}

/// The level stored as a number.
fn level_from(n: u8) -> Level {
    [Level::Trace, Level::Debug, Level::Info, Level::Warn, Level::Error, Level::Fatal].get(n as usize).copied().unwrap_or(Level::Info)
}

/// Collects an event's message and its other fields (`key=value`, after the message).
#[derive(Default)]
struct Fields {
    /// The message.
    message: String,
    /// The other fields.
    rest: Vec<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.rest.push(format!("{}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.rest.push(format!("{}={value:?}", field.name()));
        }
    }
}

/// The tracing layer that hands Lantern's log entries to the writer; `main` adds it next to the console output.
pub struct Layer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Layer {
    fn enabled(&self, meta: &tracing::Metadata<'_>, _: Context<'_, S>) -> bool {
        wanted(meta.target(), Level::of(meta.level()))
    }

    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let meta = event.metadata();
        let level = Level::of(meta.level());
        if !wanted(meta.target(), level) {
            return;
        }
        let mut f = Fields::default();
        event.record(&mut f);
        let mut message = f.message;
        for r in f.rest {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&r);
        }
        send(Entry { time: crate::auth::now(), level, logger: logger_of(meta.target()), message, exception: None, exception_type: None });
    }
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Common.Test/InstrumentationTests/CleanseLogMessageFixture.cs,
    // src/NzbDrone.Core.Test/InstrumentationTests/DatabaseTargetFixture.cs), GPL-3.0.
    use super::*;

    const SHOULD_CLEAN: &[&str] = &[
        // Indexer Urls
        r"https://iptorrents.com/torrents/rss?u=mySecret;tp=mySecret;l5;download",
        r"http://rss.torrentleech.org/mySecret",
        r"https://rss24h.torrentleech.org/mySecret",
        r"http://rss.torrentleech.org/rss/download/12345/01233210/file.name-RLSGRP.torrent",
        r"https://www.torrentleech.org/rss/download/12345/01233210/file.name-RLSGRP.torrent",
        r"http://www.bitmetv.org/rss.php?uid=mySecret&passkey=mySecret",
        r"https://rss.omgwtfnzbs.org/rss-search.php?catid=19,20&user=sonarr&api=mySecret&eng=1",
        r"https://dognzb.cr/fetch/2b51db35e1912ffc138825a12b9933d2/2b51db35e1910123321025a12b9933d2",
        r"https://baconbits.org/feeds.php?feed=torrents_tv&user=12345&auth=2b51db35e1910123321025a12b9933d2&passkey=mySecret&authkey=2b51db35e1910123321025a12b9933d2",
        r"http://127.0.0.1:9117/dl/indexername?jackett_apikey=flwjiefewklfjacketmySecretsdfldskjfsdlk&path=we0re9f0sdfbase64sfdkfjsdlfjk&file=The+Torrent+File+Name.torrent",
        r"http://nzb.su/getnzb/2b51db35e1912ffc138825a12b9933d2.nzb&i=37292&r=2b51db35e1910123321025a12b9933d2",
        r"https://horrorcharnel.org/takeloginhorror.php: username=mySecret&password=mySecret&use_sslvalue==&perm_ssl=1&submitme=X&use_ssl=1&returnto=%2F&captchaSelection=1230456",
        r"https://torrentdb.net/login: _token=2b51db35e1912ffc138825a12b9933d2&username=mySecret&password=mySecret&remember=on",
        r#" var authkey = "2b51db35e1910123321025a12b9933d2";"#,
        r"https://hd-space.org/index.php?page=login: uid=mySecret&pwd=mySecret",
        r"https://beyond-hd.me/api/torrents/2b51db35e1912ffc138825a12b9933d2",
        r"Req: [POST] https://www3.yggtorrent.nz/user/login: id=mySecret&pass=mySecret&ci_csrf_token=2b51db35e1912ffc138825a12b9933d2",
        r"https://torrentseeds.org/api/torrents/filter?api_token=2b51db35e1912ffc138825a12b9933d2&name=&sortField=created_at&sortDirection=desc&perPage=100&page=1",
        r"https://beyond-hd.me/torrent/download/the-next-365-days-2022-2160p-nf-web-dl-dual-ddp-51-dovi-hdr-hevc-apex.225146.2b51db35e1912ffc138825a12b9933d2",
        r"https://anthelion.me/api.php?api_key=2b51db35e1910123321025a12b9933d2&o=json&t=movie&q=&tmdb=&imdb=&cat=&limit=100&offset=0",
        r"https://avistaz.to/api/v1/jackett/auth: username=mySecret&password=mySecret&pid=mySecret",
        r"https://www.sharewood.tv/api/2b51db35e1910123321025a12b9933d2/last-torrents",
        r"https://example.org/rss/torrents?rsskey=2b51db35e1910123321025a12b9933d2&search=",
        // avistaz response
        r#""download":"https://avistaz.to/rss/download/2b51db35e1910123321025a12b9933d2/tb51db35e1910123321025a12b9933d2.torrent","#,
        r#","info_hash":"2b51db35e1910123321025a12b9933d2","#,
        r#""token":"2b51db35e1910123321025a12b9933d2""#,
        // animebytes response
        r#""Link":"https://animebytes.tv/torrent/994064/download/tb51db35e1910123321025a12b9933d2","#,
        // danish bytes response
        r#","rsskey":"2b51db35e1910123321025a12b9933d2","#,
        r#","passkey":"2b51db35e1910123321025a12b9933d2","#,
        r#"{"rsskey":"2b51db35e1910123321025a12b9933d2"}"#,
        r#"{"passkey":"2b51db35e1910123321025a12b9933d2"}"#,
        // nzbgeek & usenet response
        r#"<guid isPermaLink="true">https://api.nzbgeek.info/api?t=details&amp;id=2b51db35e1910123321025a12b9933d2&amp;apikey=2b51db35e1910123321025a12b9933d2</guid>"#,
        // UNIT3D Response
        r#""download_link":"https://blutopia.xyz/torrent/download/114592.2b51db35e1910123321025a12b9933d2","#,
        r#""download_link":"https://desitorrents.tv/torrent/download/114592.2b51db35e1910123321025a12b9933d2","#,
        // NzbGet
        r#"{ "Name" : "ControlUsername", "Value" : "mySecret" }, { "Name" : "ControlPassword", "Value" : "mySecret" }, "#,
        r#"{ "Name" : "Server1.Username", "Value" : "mySecret" }, { "Name" : "Server1.Password", "Value" : "mySecret" }, "#,
        // MTV
        r#"<link rel="alternate" type="application/rss+xml" href="/feeds.php?feed=torrents_notify_2b51db35e1910123321025a12b9933d2&amp;user=(removed)&amp;auth=(removed)&amp;passkey=(removed)&amp;authkey=(removed) title="MoreThanTV - P.T.N." />"#,
        r#"href="/torrents.php?action=download&amp;id=(removed)&amp;authkey=(removed)&amp;torrent_pass=2b51db35e1910123321025a12b9933d2" title="Download Torrent""#,
        // Sabnzbd
        r"http://127.0.0.1:1234/api/call?vv=1&apikey=mySecret",
        r"http://127.0.0.1:1234/api/call?vv=1&ma_username=mySecret&ma_password=mySecret",
        r#""config":{"newzbin":{"username":"mySecret","password":"mySecret"}"#,
        r#""nzbxxx":{"username":"mySecret","apikey":"mySecret"}"#,
        r#""growl":{"growl_password":"mySecret","growl_server":""}"#,
        r#""nzbmatrix":{"username":"mySecret","apikey":"mySecret"}"#,
        r#""misc":{"username":"mySecret","api_key":"mySecret","password":"mySecret","nzb_key":"mySecret"}"#,
        r#""servers":[{"username":"mySecret","password":"mySecret"}]"#,
        r#""misc":{"email_account":"mySecret","email_to":[],"email_from":"","email_pwd":"mySecret"}"#,
        // uTorrent
        r"http://localhost:9091/gui/?token=wThmph5l0ZXfH-a6WOA4lqiLvyjCP0FpMrMeXmySecret_VXBO11HoKL751MAAAAA&list=1",
        r#",["boss_key",0,"mySecret",{"access":"Y"}],["boss_key_salt",0,"mySecret",{"access":"W"}]"#,
        r#",["webui.username",2,"mySecret",{"access":"Y"}],["webui.password",2,"mySecret",{"access":"Y"}]"#,
        r#",["webui.uconnect_username",2,"mySecret",{"access":"Y"}],["webui.uconnect_password",2,"mySecret",{"access":"Y"}]"#,
        r#",["proxy.proxy",2,"mySecret",{"access":"Y"}]"#,
        r#",["proxy.username",2,"mySecret",{"access":"Y"}],["proxy.password",2,"mySecret",{"access":"Y"}]"#,
        // Deluge
        r#",{"download_location": "C:\Users\\mySecret mySecret\\Downloads"}"#,
        r#",{"download_location": "/home/mySecret/Downloads"}"#,
        r#",{"download_location": "/Users/mySecret/Downloads"}"#,
        r#"auth.login("mySecret")"#,
        // Download Station
        r"webapi/entry.cgi?api=(removed)&version=2&method=login&account=01233210&passwd=mySecret&format=sid&session=DownloadStation",
        // Announce URLs (passkeys) Magnet & Tracker
        r#"magnet_uri":"magnet:?xt=urn:btih:9pr04sgkillroyimaveql2tyu8xyui&dn=&tr=https%3a%2f%2fxxx.yyy%2f9pr04sg601233210IMAveQL2tyu8xyui%2fannounce"}"#,
        r#"magnet_uri":"magnet:?xt=urn:btih:9pr04sgkillroyimaveql2tyu8xyui&dn=&tr=https%3a%2f%2fxxx.yyy%2ftracker.php%2f9pr04sg601233210IMAveQL2tyu8xyui%2fannounce"}"#,
        r#"magnet_uri":"magnet:?xt=urn:btih:9pr04sgkillroyimaveql2tyu8xyui&dn=&tr=https%3a%2f%2fxxx.yyy%2fannounce%2f9pr04sg601233210IMAveQL2tyu8xyui"}"#,
        r#"magnet_uri":"magnet:?xt=urn:btih:9pr04sgkillroyimaveql2tyu8xyui&dn=&tr=https%3a%2f%2fxxx.yyy%2fannounce.php%3fpasskey%3d9pr04sg601233210IMAveQL2tyu8xyui"}"#,
        r#"tracker":"https://xxx.yyy/9pr04sg601233210IMAveQL2tyu8xyui/announce"}"#,
        r#"tracker":"https://xxx.yyy/tracker.php/9pr04sg601233210IMAveQL2tyu8xyui/announce"}"#,
        r#"tracker":"https://xxx.yyy/announce/9pr04sg601233210IMAveQL2tyu8xyui"}"#,
        r#"tracker":"https://xxx.yyy/announce.php?passkey=9pr04sg601233210IMAveQL2tyu8xyui"}"#,
        r#"tracker":"http://xxx.yyy/announce.php?passkey=9pr04sg601233210IMAveQL2tyu8xyui","info":"http://xxx.yyy/info?a=b""#,
        // BroadcastheNet
        r#"method: "getTorrents", "params": [ "mySecret","#,
        r#"getTorrents("mySecret", [asdfasdf], 100, 0)"#,
        r#""DownloadURL":"https://broadcasthe.net/torrents.php?action=download&id=123&authkey=mySecret&torrent_pass=mySecret""#,
        // Webhooks - Notifiarr
        r"https://xxx.yyy/api/v1/notification/prowlarr/9pr04sg6-0123-3210-imav-eql2tyu8xyui",
        "https://notifiarr.com/notifier.php: api=1234530f-422f-4aac-b6b3-01233210aaaa&radarr_health_issue_message=Download",
        // RSS
        r#"<atom:link href = "https://api.nzb.su/api?t=search&amp;extended=1&amp;cat=3030&apikey=mySecret&amp;q=Diggers" rel="self" type="application/rss+xml" />"#,
        // Applications
        r#""name":"apiKey","value":"mySecret""#,
        // Internal
        r"[Info] MigrationController: *** Migrating Database=prowlarr-main;Host=postgres14;Username=mySecret;Password=mySecret;Port=5432;Enlist=False ***",
        "/readarr/signalr/messages/negotiate?access_token=1234530f422f4aacb6b301233210aaaa&negotiateVersion=1",
        r"[Info] MigrationController: *** Migrating Database=prowlarr-main;Host=postgres14;Username=mySecret;Password=mySecret;Port=5432;token=mySecret;Enlist=False&username=mySecret;mypassword=mySecret;mypass=shouldkeep1;test_token=mySecret;password=123%@%_@!#^#@;use_password=mySecret;get_token=shouldkeep2;usetoken=shouldkeep3;passwrd=mySecret;",
        // Discord
        r"https://discord.com/api/webhooks/mySecret",
        r"https://discord.com/api/webhooks/mySecret/01233210",
    ];

    #[test]
    fn should_clean_message() {
        for message in SHOULD_CLEAN {
            let cleansed = cleanse(message);
            for secret in ["mySecret", "123%@%_@!#^#@", "01233210"] {
                assert!(!cleansed.contains(secret), "{message}\n→ {cleansed}");
            }
        }
    }

    #[test]
    fn should_keep_message() {
        let message = r"[Info] MigrationController: *** Migrating Database=radarr-main;Host=postgres14;Username=mySecret;Password=mySecret;Port=5432;token=mySecret;Enlist=False&username=mySecret;mypassword=mySecret;mypass=shouldkeep1;test_token=mySecret;password=123%@%_@!#^#@;use_password=mySecret;get_token=shouldkeep2;usetoken=shouldkeep3;passwrd=mySecret;";
        let cleansed = cleanse(message);
        for secret in ["mySecret", "123%@%_@!#^#@", "01233210"] {
            assert!(!cleansed.contains(secret), "{cleansed}");
        }
        for kept in ["shouldkeep1", "shouldkeep2", "shouldkeep3"] {
            assert!(cleansed.contains(kept), "{kept}: {cleansed}");
        }
    }

    #[test]
    fn should_clean_ipaddress() {
        for message in [
            "Some message (from 32.2.3.5 user agent)",
            "Auth-Invalidated ip 32.2.3.5",
            "Auth-Success ip 32.2.3.5",
            "Auth-Logout ip 32.2.3.5",
        ] {
            let cleansed = cleanse(message);
            assert!(!cleansed.contains(".2.3."), "{cleansed}");
        }
        assert_eq!(cleanse("Auth-Success ip 32.2.3.5"), "Auth-Success ip 32.*.*.5");
    }

    #[test]
    fn should_not_clean_ipaddress() {
        for message in ["Some message (from 10.2.3.2 user agent)", "Auth-Unauthorized ip 32.2.3.5", "Auth-Failure ip 32.2.3.5"] {
            assert_eq!(cleanse(message), message);
        }
    }

    #[test]
    fn should_not_clean_usetoken() {
        let message = "&useToken=2b51db35e1910123321025a12b9933d2";
        assert_eq!(cleanse(message), message);
    }

    #[test]
    fn should_not_clean_url() {
        for message in [
            "https://www.torrentleech.org/torrents/browse/list/imdbID/tt8005374/categories/29,2,26,27,32,44,7,34,35",
            "https://torrentapi.org/pubapi_v2.php?get_token=get_token&app_id=Prowlarr",
        ] {
            assert_eq!(cleanse(message), message);
        }
    }

    fn writer(name: &str) -> (Writer, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lantern-logwriter-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = SqliteDb::open(&dir.join("app.db")).unwrap();
        super::super::migrate(&db).unwrap();
        (Writer { db, folder: dir.join("logs") }, dir)
    }

    fn entry(level: Level, message: &str) -> Entry {
        Entry {
            time: 1_793_966_400.25,
            level,
            logger: "prowlarr::commands".into(),
            message: message.into(),
            exception: None,
            exception_type: None,
        }
    }

    #[test]
    fn write_log_info_and_above_to_the_table() {
        // DatabaseTargetFixture.write_log / write_long_log / write_log_exception: what is written comes back
        let (w, _) = writer("table");
        let mut failed = entry(Level::Error, "");
        failed.exception = Some("Something failed: apikey=mySecret".into());
        failed.exception_type = Some("std::io::Error".into());
        w.write(&[
            entry(Level::Debug, "debug"),
            entry(Level::Info, "Test apikey=mySecret&x"),
            entry(Level::Warn, &"x".repeat(5000)),
            failed,
        ]);
        let rows = w.db.query("SELECT * FROM prowlarr_logs ORDER BY id", p![]).unwrap();
        use kit_server_core::RowExt;
        assert_eq!(rows.iter().map(|r| r.s("level")).collect::<Vec<_>>(), ["Info", "Warn", "Error"]);
        assert_eq!(rows[0].s("message"), "Test apikey=(removed)&x");
        assert_eq!(rows[0].s("logger"), "prowlarr::commands");
        assert_eq!(rows[0].f("time"), 1_793_966_400.25);
        assert_eq!(rows[1].s("message").len(), 5000);
        assert_eq!(
            (rows[2].s("exception"), rows[2].s("exception_type")),
            ("Something failed: apikey=(removed)".into(), "std::io::Error".into())
        );
    }

    #[test]
    fn files_follow_the_log_level() {
        let (w, _) = writer("files");
        // LogLevel debug (Prowlarr's default): lantern.txt and lantern.debug.txt
        w.write(&[entry(Level::Trace, "trace"), entry(Level::Debug, "debug"), entry(Level::Info, "info")]);
        let read = |n: &str| std::fs::read_to_string(w.folder.join(n)).unwrap_or_default();
        assert!(read("lantern.txt").ends_with("|Info|prowlarr::commands|info\n"), "{}", read("lantern.txt"));
        assert_eq!(read("lantern.debug.txt").lines().count(), 2);
        assert!(!w.folder.join("lantern.trace.txt").exists());
        // yyyy-MM-dd HH:mm:ss.f
        assert!(read("lantern.txt").contains(":00.2|Info|"), "{}", read("lantern.txt"));
        super::super::config_set(&w.db, "loglevel", "trace").unwrap();
        w.write(&[entry(Level::Trace, "trace")]);
        assert_eq!(read("lantern.trace.txt").lines().count(), 1);
        super::super::config_set(&w.db, "loglevel", "info").unwrap();
        w.write(&[entry(Level::Debug, "debug")]);
        assert_eq!(read("lantern.debug.txt").lines().count(), 2);
    }

    #[test]
    fn full_files_roll_over_and_old_ones_go() {
        let (w, _) = writer("roll");
        for i in 0..5 {
            append(&w.folder, "lantern.txt", &format!("{i}{}\n", "x".repeat(60)), 100, 2).unwrap();
        }
        let read = |n: &str| std::fs::read_to_string(w.folder.join(n)).unwrap_or_default();
        assert!(read("lantern.txt").starts_with('4'));
        assert!(read("lantern.0.txt").starts_with('3'));
        assert!(read("lantern.1.txt").starts_with('2'));
        assert!(!w.folder.join("lantern.2.txt").exists());
        assert_eq!(archive_name("lantern.debug.txt", 3), "lantern.debug.3.txt");
    }

    #[test]
    fn the_layer_takes_lanterns_own_entries_from_the_log_level() {
        MAX_LEVEL.store(Level::Debug as u8, Ordering::Relaxed);
        assert!(wanted("server::prowlarr::commands", Level::Debug));
        assert!(!wanted("server::prowlarr::commands", Level::Trace));
        assert!(!wanted("hyper::proto", Level::Info));
        assert!(wanted("hyper::proto", Level::Warn));
        assert_eq!(logger_of("server::prowlarr::backup"), "prowlarr::backup");
    }
}
