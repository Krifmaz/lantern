// Ported from autobrr v1.90.0 (internal/indexer/definition_test.go, deprecations_test.go, service_test.go;
// internal/http/irc.go and indexer.go behaviour; internal/irc/handler_test.go sign-in cases), GPL-2.0-or-later.
//! autobrr's IRC networks and indexers under `/autobrr/api`, against a stand-in IRC server on 127.0.0.1: NickServ
//! and SASL sign-in, invite commands, joining, multi-line announces reaching the filters, the channel history,
//! the live event stream, sending by hand, bans; the indexers' CRUD with redacted secrets and deprecations; and
//! every bundled autobrr definition read with its own test lines. Nothing here reaches the internet.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use base64::Engine;
use futures_util::StreamExt;
use serde_json::{json, Value};
use server::releases::definition::{is_secret_indexer_setting, Library, ParseLine};
use server::releases::irc::Timing;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tower::ServiceExt;

const KEY: &str = "test-key";

/// A made-up tracker in autobrr's v2 format: a multi-line announce, NickServ, an invite bot.
const HARBOUR: &str = r##"
version: 2
identifier: harbour
name: Harbour
urls: [https://harbour.example/]
privacy: private
protocol: torrent
supports: [irc]
settings:
  - { name: passkey, type: secret, required: true, label: Passkey }
irc:
  network: Harbour
  server: irc.harbour.example
  port: 6697
  tls: true
  auth: { mechanism: NICKSERV }
  settings:
    - { name: nick, type: text, required: true, label: Nick }
    - { name: auth.account, type: text, label: NickServ Account }
    - { name: auth.password, type: secret, label: NickServ Password }
    - { name: invite_command, type: secret, default: "Keeper invite IRCKEY", label: Invite command }
  channels:
    - name: "#New"
      announcers: [Bot]
      parse:
        type: multi
        forcesizeunit: MB
        lines:
          - pattern: 'Name: (.+)'
            vars: [torrentName]
          - pattern: 'Size: (\d+) Tags: (.*)'
            vars: [torrentSize, tags]
          - pattern: 'Link: (https?://.+/)t/(\d+)'
            vars: [baseUrl, torrentId]
        match:
          downloadurl: "/get/{{ .torrentId }}?k={{ .passkey }}"
          infourl: "/t/{{ .torrentId }}"
"##;

/// Lantern with `HARBOUR` as its only announce definition (in `<dir>/announce`), short IRC timings.
async fn lantern(name: &str) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-abirc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let announce = dir.join("announce");
    std::fs::create_dir_all(&announce).unwrap();
    std::fs::write(announce.join("harbour.yaml"), HARBOUR).unwrap();
    let a = announce.display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some(KEY.to_string()),
        "APP_ANNOUNCE_DIR" => Some(a.clone()),
        "APP_DEFINITIONS_DIR" => Some("/nonexistent".to_string()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    st.releases.irc.set_timing(Timing {
        after_connect: Duration::from_millis(50),
        connect_retry: Duration::from_millis(200),
        connect_jitter: Duration::from_millis(10),
        reconnect: Duration::from_millis(200),
        tick: Duration::from_millis(20),
        ..Timing::default()
    });
    (server::app(st.clone()), st, announce)
}

/// Calls autobrr's `path` (under `/autobrr/api`).
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(format!("/autobrr/api{path}")).header("x-api-key", KEY);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The JSON keys autobrr's Go struct `name` always sends, and those it may.
fn shape(name: &str) -> (Vec<String>, Vec<String>) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/api/structs.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let s = &v["structs"][name];
    let list = |k: &str| s[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
    (list("always"), list("omitempty"))
}

/// `v` has every key struct `name` always sends, and none it doesn't have.
fn assert_shape(name: &str, v: &Value) {
    let (always, maybe) = shape(name);
    let o = v.as_object().unwrap_or_else(|| panic!("{name} is not an object: {v}"));
    for k in &always {
        assert!(o.contains_key(k), "{name} lacks {k}: {v}");
    }
    for k in o.keys() {
        assert!(always.contains(k) || maybe.contains(k), "{name} has {k}, which autobrr's hasn't: {v}");
    }
}

fn assert_error(v: &Value, code: &str, message: &str) {
    assert_eq!(v["code"], code, "{v}");
    assert_eq!(v["message"], message, "{v}");
}

/// Waits up to 10 s for `f`.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A stand-in IRC server: every line Lantern says, and a way to say lines to it.
struct Ircd {
    port: u16,
    said: Arc<Mutex<Vec<String>>>,
    to_client: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
}

impl Ircd {
    /// Starts one on a free port; `reply` answers each line Lantern says.
    async fn start(reply: impl Fn(&str) -> Vec<String> + Send + Sync + 'static) -> Ircd {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let to_client: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>> = Arc::default();
        let (s2, t2, reply) = (said.clone(), to_client.clone(), Arc::new(reply));
        tokio::spawn(async move {
            while let Ok((sock, _)) = l.accept().await {
                let (r, mut w) = sock.into_split();
                let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                *t2.lock().unwrap() = Some(tx.clone());
                tokio::spawn(async move {
                    while let Some(l) = rx.recv().await {
                        if w.write_all(format!("{l}\r\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
                let (s3, reply) = (s2.clone(), reply.clone());
                tokio::spawn(async move {
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        s3.lock().unwrap().push(l.clone());
                        for out in reply(&l) {
                            let _ = tx.send(out);
                        }
                    }
                });
            }
        });
        Ircd { port, said, to_client }
    }

    /// Says `line` to Lantern.
    fn say(&self, line: &str) {
        self.to_client.lock().unwrap().as_ref().expect("Lantern is connected").send(line.to_string()).unwrap();
    }

    /// Whether Lantern said `line`.
    fn heard(&self, line: &str) -> bool {
        self.said.lock().unwrap().iter().any(|l| l == line)
    }

    /// Waits up to 10 s for Lantern to say `line`.
    async fn wait_for(&self, line: &str) {
        for _ in 0..500 {
            if self.heard(line) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("Lantern never said {line}: {:?}", self.said.lock().unwrap());
    }
}

/// A network on the stand-in server at `port`.
fn network(port: u16, auth: Value, invite: &str) -> Value {
    json!({
        "name": "Harbour", "enabled": true, "server": "irc.harbour.example", "port": 6697, "tls": false,
        "nick": "me", "auth": auth, "invite_command": invite, "use_bouncer": true,
        "bouncer_addr": format!("127.0.0.1:{port}"), "bot_mode": false,
        "channels": [{ "name": "#New", "enabled": true }, { "name": "#chat", "enabled": true }],
    })
}

/// The release log's newest row with `title`, waiting up to 10 s for it.
async fn logged(app: &Router, title: &str) -> Value {
    for _ in 0..500 {
        let r = Request::builder().uri("/api/releases/log").header("x-api-key", KEY).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        let v: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 22).await.unwrap()).unwrap();
        if let Some(row) = v["releases"].as_array().into_iter().flatten().find(|r| r["title"] == title) {
            return row.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{title} never reached the filters");
}

#[tokio::test]
async fn a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces() {
    let (app, _st, _) = lantern("nickserv").await;
    let ircd = Ircd::start(|l| match l {
        l if l.starts_with("USER ") => vec![
            ":irc.harbour 001 me :Welcome".into(),
            ":irc.harbour 005 me CASEMAPPING=rfc1459 :are supported".into(),
            ":irc.harbour 422 me :MOTD File is missing".into(),
        ],
        "PRIVMSG NickServ :IDENTIFY pw" => vec![":NickServ!s@services NOTICE me :You are now identified for me.".into()],
        "PRIVMSG Keeper :invite IRCKEY" => vec![":Keeper!k@h INVITE me #New".into()],
        l if l.starts_with("JOIN ") => {
            let c = l[5..].split(' ').next().unwrap().to_string();
            vec![format!(":me!u@h JOIN {c}"), format!(":irc.harbour 366 me {c} :End of /NAMES list.")]
        }
        l if l.starts_with("NICK me2") => vec![":me!u@h NICK me2".into()],
        _ => vec![],
    })
    .await;

    // the indexer first, as autobrr's web app does, then its network
    let (s, ix) = call(
        &app,
        "POST",
        "/indexer",
        Some(json!({ "name": "Harbour", "identifier": "harbour", "implementation": "irc", "enabled": true, "base_url": "https://harbour.example/", "settings": { "passkey": "abc" } })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{ix}");
    assert_shape("Indexer", &ix);
    assert_eq!(ix["settings"]["passkey"], "<redacted>");
    let (s, v) = call(
        &app,
        "POST",
        "/irc",
        Some(network(ircd.port, json!({ "mechanism": "NICKSERV", "account": "me", "password": "pw" }), "Keeper invite IRCKEY")),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");

    // NickServ first, then the invite command, then the join the invite allows (autobrr joins by the channel's
    // lowercased name)
    ircd.wait_for("JOIN #new").await;
    let said = ircd.said.lock().unwrap().clone();
    let at = |l: &str| said.iter().position(|x| x == l).unwrap_or_else(|| panic!("never said {l}: {said:?}"));
    assert!(at("PRIVMSG NickServ :IDENTIFY pw") < at("PRIVMSG Keeper :invite IRCKEY"), "{said:?}");
    assert!(at("PRIVMSG Keeper :invite IRCKEY") < at("JOIN #new"), "{said:?}");
    assert!(ircd.heard("JOIN #chat"), "the network's other channels are joined too: {said:?}");

    let (s, list) = call(&app, "GET", "/irc", None).await;
    assert_eq!(s, StatusCode::OK);
    let net = &list[0];
    assert_shape("IrcNetworkWithHealth", net);
    assert_eq!(net["auth"], json!({ "mechanism": "NICKSERV", "account": "me", "password": "<redacted>" }));
    assert_eq!(net["connected"], true);
    let id = net["id"].as_i64().unwrap();
    for _ in 0..200 {
        let (_, list) = call(&app, "GET", "/irc", None).await;
        if list[0]["channels"].as_array().unwrap().iter().all(|c| c["state"] == "Monitoring") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (_, list) = call(&app, "GET", "/irc", None).await;
    for c in list[0]["channels"].as_array().unwrap() {
        assert_shape("IrcChannelWithHealth", c);
        assert_eq!(c["state"], "Monitoring", "{c}");
        assert_eq!(c["monitoring"], true);
    }
    assert_eq!(list[0]["healthy"], true, "{list}");

    // the live stream sees what is said from now on
    let r = Request::builder().uri("/autobrr/api/irc/events?stream=irc").header("x-api-key", KEY).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(r).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/event-stream"));
    let mut events = resp.into_body().into_data_stream();

    // someone who isn't an announcer is ignored; the announce's three lines make one release
    ircd.say(":Eve!e@h PRIVMSG #New :Name: Fake.Release-EVE");
    ircd.say(":Bot!b@h PRIVMSG #New :Name: Movie.2024.2160p.BluRay-GRP");
    ircd.say(":Bot!b@h PRIVMSG #New :Size: 2048 Tags: hdr, action");
    ircd.say(":Bot!b@h PRIVMSG #New :\x02Link:\x02 https://harbour.example/t/9");
    let row = logged(&app, "Movie.2024.2160p.BluRay-GRP").await;
    assert_eq!(row["source_name"], "Harbour");
    assert_eq!(row["size"], 2_048_000_000u64);
    assert_eq!(row["info_url"], "https://harbour.example/t/9");
    assert_eq!(row["status"], "rejected", "no filters yet: {row}");

    let mut seen = String::new();
    while !seen.contains("Movie.2024.2160p.BluRay-GRP") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), events.next()).await.expect("an event").unwrap().unwrap();
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.contains("event: PRIVMSG"), "{seen}");
    // r3labs/sse's answers without a stream, or for one Lantern doesn't serve
    for (path, want) in
        [("/autobrr/api/irc/events", "Please specify a stream!\n"), ("/autobrr/api/events?stream=logs", "Stream not found!\n")]
    {
        let r = Request::builder().uri(path).header("x-api-key", KEY).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&body), want, "{path}");
    }

    // the history keeps every line said there, cleaned of formatting
    let (s, h) = call(&app, "GET", &format!("/irc/network/{id}/channel/New/history"), None).await;
    assert_eq!(s, StatusCode::OK, "{h}");
    let lines: Vec<&str> = h.as_array().unwrap().iter().map(|m| m["msg"].as_str().unwrap()).collect();
    assert_eq!(
        lines,
        [
            "Name: Fake.Release-EVE",
            "Name: Movie.2024.2160p.BluRay-GRP",
            "Size: 2048 Tags: hdr, action",
            "Link: https://harbour.example/t/9"
        ]
    );
    assert_shape("IrcMessage", &h[0]);
    assert_eq!(h[1]["nick"], "Bot");
    let (s, h) = call(&app, "GET", "/irc/network/999/channel/New/history", None).await;
    assert_eq!((s, h), (StatusCode::OK, json!([])), "no handler: an empty history");

    // an announce sent by hand, one line at a time
    for msg in ["Name: Manual.Release-X", "Size: 10 Tags: ", "Link: https://harbour.example/t/10"] {
        let (s, v) = call(&app, "POST", &format!("/irc/network/{id}/channel/New/announce/process"), Some(json!({ "msg": msg }))).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    }
    logged(&app, "Manual.Release-X").await;
    let (s, v) = call(&app, "POST", &format!("/irc/network/{id}/channel/other/announce/process"), Some(json!({ "msg": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("channel #other not found on network with id {id}"));
    let (s, v) = call(&app, "POST", &format!("/irc/network/{id}/channel/chat/announce/process"), Some(json!({ "msg": "x" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", &format!("channel #chat is not an indexer announce channel on network with id {id}"));
    let (s, v) = call(&app, "POST", "/irc/network/999/channel/New/announce/process", Some(json!({ "msg": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "network with id 999 not found");

    // a line said by hand
    let (s, _) = call(&app, "POST", &format!("/irc/network/{id}/cmd"), Some(json!({ "channel": "#chat", "msg": "hello there" }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    ircd.wait_for("PRIVMSG #chat :hello there").await;
    let (s, v) = call(&app, "POST", "/irc/network/999/cmd", Some(json!({ "channel": "#chat", "msg": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "network with id 999 not found");

    // one network with its channels; redacted secrets keep the saved ones; a nick change is made in place
    let (s, n) = call(&app, "GET", &format!("/irc/network/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("IrcNetwork", &n);
    assert_shape("IrcChannel", &n["channels"][0]);
    let mut changed = n.clone();
    changed["nick"] = json!("me2");
    let (s, v) = call(&app, "PUT", &format!("/irc/network/{id}"), Some(changed)).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    ircd.wait_for("NICK me2").await;
    let (_, list) = call(&app, "GET", "/irc", None).await;
    assert_eq!(list[0]["current_nick"], "me2");
    let (_, n2) = call(&app, "GET", &format!("/irc/network/{id}"), None).await;
    assert_eq!(n2["nick"], "me2");
    let quits = ircd.said.lock().unwrap().iter().filter(|l| l.starts_with("QUIT")).count();
    assert_eq!(quits, 0, "a nick change needs no reconnect");

    // the errors in autobrr's words
    let (s, v) = call(&app, "GET", "/irc/network/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "network with id 999 not found");
    let (s, v) = call(&app, "GET", "/irc/network/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "networkID parameter is invalid");
    let (s, v) = call(&app, "DELETE", "/irc/network/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "network with id 999 does not exist");
    let (s, v) =
        call(&app, "POST", "/irc", Some(json!({ "name": "x", "server": "s", "port": 1, "nick": "n", "auth": { "mechanism": "TYPO" } })))
            .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "invalid IRC authentication mechanism: \"TYPO\"");

    // a channel added by hand is saved (the running connection isn't told, as in autobrr)
    let (s, _) =
        call(&app, "POST", &format!("/irc/network/{id}/channel"), Some(json!({ "name": "#extra", "enabled": true, "password": "k" })))
            .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, n3) = call(&app, "GET", &format!("/irc/network/{id}"), None).await;
    assert!(n3["channels"].as_array().unwrap().iter().any(|c| c["name"] == "#extra" && c["password"] == "<redacted>"), "{n3}");
    let (s, v) = call(&app, "POST", "/irc/network/999/channel", Some(json!({ "name": "#x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "network with id 999 not found");

    // a restart reconnects, and the added channel is joined with its key
    let count = |p: &str| ircd.said.lock().unwrap().iter().filter(|l| l.starts_with(p)).count();
    let (s, v) = call(&app, "GET", &format!("/irc/network/{id}/restart"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    ircd.wait_for("JOIN #extra k").await;
    assert_eq!((count("QUIT"), count("USER ")), (1, 2), "{:?}", ircd.said.lock().unwrap());
    let (s, v) = call(&app, "GET", "/irc/network/999/restart", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "network with id 999 not found");

    // gone: the connection ends
    let (s, _) = call(&app, "DELETE", &format!("/irc/network/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    until("the quit", || count("QUIT") == 2).await;
    let (_, list) = call(&app, "GET", "/irc", None).await;
    assert_eq!(list, json!([]));
}

#[tokio::test]
async fn a_network_signs_in_with_sasl_and_joins_without_an_invite() {
    let (app, _st, _) = lantern("sasl").await;
    let token = base64::engine::general_purpose::STANDARD.encode("acc\0acc\0pw");
    let t2 = token.clone();
    let ircd = Ircd::start(move |l| match l {
        "CAP LS 302" => vec![":irc.harbour CAP * LS :multi-prefix sasl".into()],
        "CAP REQ sasl" => vec![":irc.harbour CAP * ACK :sasl".into()],
        "AUTHENTICATE PLAIN" => vec!["AUTHENTICATE +".into()],
        l if l == format!("AUTHENTICATE {t2}") => vec![
            ":irc.harbour 900 me me!u@h acc :You are now logged in as acc".into(),
            ":irc.harbour 903 me :SASL authentication successful".into(),
        ],
        "CAP END" => vec![":irc.harbour 001 me :Welcome".into(), ":irc.harbour 422 me :MOTD File is missing".into()],
        l if l.starts_with("JOIN ") => {
            let c = l[5..].split(' ').next().unwrap().to_string();
            vec![format!(":me!u@h JOIN {c}"), format!(":irc.harbour 366 me {c} :End of /NAMES list.")]
        }
        _ => vec![],
    })
    .await;
    let (s, _) = call(
        &app,
        "POST",
        "/indexer",
        Some(json!({ "name": "Harbour", "identifier": "harbour", "implementation": "irc", "enabled": true, "base_url": "https://harbour.example/", "settings": { "passkey": "abc" } })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) =
        call(&app, "POST", "/irc", Some(network(ircd.port, json!({ "mechanism": "SASL_PLAIN", "account": "acc", "password": "pw" }), "")))
            .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    ircd.wait_for("JOIN #new").await;
    assert!(ircd.heard(&format!("AUTHENTICATE {token}")));
    let said = ircd.said.lock().unwrap().clone();
    assert!(!said.iter().any(|l| l.contains("IDENTIFY")), "SASL signs in, so NickServ isn't asked: {said:?}");
    assert!(!said.iter().any(|l| l.contains("invite")), "no invite command: {said:?}");
    assert_eq!(said[0], "CAP LS 302", "{said:?}");
}

#[tokio::test]
async fn a_ban_stops_the_network_with_the_reason() {
    let (app, _st, _) = lantern("ban").await;
    let ircd = Ircd::start(|l| match l {
        l if l.starts_with("USER ") => vec![":irc.harbour 465 me :You are banned from this server".into()],
        _ => vec![],
    })
    .await;
    let (s, _) = call(&app, "POST", "/irc", Some(network(ircd.port, json!({ "mechanism": "NONE" }), ""))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let mut last = Value::Null;
    for _ in 0..500 {
        let (_, list) = call(&app, "GET", "/irc", None).await;
        last = list[0].clone();
        if last["connection_errors"].as_array().is_some_and(|e| !e.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(last["connection_errors"], json!(["banned from network: You are banned from this server"]), "{last}");
    assert_eq!(last["connected"], false);
    // stopped: no new connection is tried
    let users = || ircd.said.lock().unwrap().iter().filter(|l| l.starts_with("USER ")).count();
    let before = users();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(users(), before, "a ban isn't retried");
}

#[tokio::test]
async fn indexers_keep_their_secrets_and_follow_autobrrs_rules() {
    let (app, st, announce) = lantern("indexers").await;
    let (s, schema) = call(&app, "GET", "/indexer/schema", None).await;
    assert_eq!(s, StatusCode::OK);
    let harbour = schema.as_array().unwrap().iter().find(|d| d["identifier"] == "harbour").unwrap();
    assert_shape("IndexerDefinition", harbour);
    assert_shape("IndexerSetting", &harbour["settings"][0]);

    let body = json!({ "name": " Harbour ", "identifier": "harbour", "implementation": "irc", "enabled": true, "base_url": "https://harbour.example/", "settings": { "passkey": "abc" } });
    let (s, ix) = call(&app, "POST", "/indexer", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{ix}");
    assert_eq!(ix["name"], "Harbour", "names are trimmed");
    assert_eq!(ix["identifier_external"], "Harbour");
    let id = ix["id"].as_i64().unwrap();

    let (_, one) = call(&app, "GET", &format!("/indexer/{id}"), None).await;
    assert_eq!(one["settings"]["passkey"], "<redacted>");
    let (_, options) = call(&app, "GET", "/indexer/options", None).await;
    assert_eq!(options[0]["id"], id);
    let (_, mapped) = call(&app, "GET", "/indexer", None).await;
    assert_shape("IndexerDefinition", &mapped[0]);
    assert_eq!(mapped[0]["id"], id);
    assert_eq!(mapped[0]["settings"][0]["value"], "<redacted>", "{}", mapped[0]["settings"]);

    // a redacted secret keeps the saved one; leaving it out is refused; an IRC indexer needs its base URL
    let mut upd = one.clone();
    upd["name"] = json!("Harbour 2");
    let (s, v) = call(&app, "PUT", &format!("/indexer/{id}"), Some(upd.clone())).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["settings"]["passkey"], "<redacted>");
    assert_eq!(st.releases.mapped_definition(id).unwrap().settings_map["passkey"], "abc");
    let mut no_secret = upd.clone();
    no_secret["settings"] = json!({});
    let (s, v) = call(&app, "PUT", &format!("/indexer/{id}"), Some(no_secret)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "update omits saved secret setting 'passkey'");
    let mut no_base = upd.clone();
    no_base["base_url"] = json!("");
    let (s, v) = call(&app, "PUT", &format!("/indexer/{id}"), Some(no_base)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "indexer baseURL must not be empty");
    let mut missing = upd.clone();
    missing["id"] = json!(999);
    let (s, v) = call(&app, "PUT", "/indexer/999", Some(missing)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "indexer with id 999 not found");

    // on and off; the site API test; the errors
    let (s, _) = call(&app, "PATCH", &format!("/indexer/{id}/enabled"), Some(json!({ "enabled": false }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, one) = call(&app, "GET", &format!("/indexer/{id}"), None).await;
    assert_eq!(one["enabled"], false);
    let (s, v) = call(&app, "POST", &format!("/indexer/{id}/api/test"), Some(json!({}))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "indexer (harbour) does not support api");
    let (s, v) = call(&app, "GET", "/indexer/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "indexer with id 999 not found");
    let (s, v) = call(&app, "GET", "/indexer/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "indexerID parameter is invalid");

    // a second one, removed again
    let body = json!({ "name": "Spare", "identifier": "harbour", "implementation": "irc", "enabled": true, "base_url": "https://harbour.example/", "settings": { "passkey": "x" } });
    let (_, spare) = call(&app, "POST", "/indexer", Some(body)).await;
    let (s, _) = call(&app, "DELETE", &format!("/indexer/{}", spare["id"]), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&app, "DELETE", &format!("/indexer/{}", spare["id"]), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("indexer with id {} not found", spare["id"]));

    // a filter watching it
    let (s, f) = call(&app, "POST", "/filters", Some(json!({ "name": "watch", "indexers": [{ "id": id }] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    let fid = f["id"].as_i64().unwrap();

    // the definition is removed upstream: the indexer is archived and can't be changed (TestReconcileDeprecations,
    // TestServiceRejectsArchivedMutations)
    std::fs::remove_file(announce.join("harbour.yaml")).unwrap();
    std::fs::create_dir_all(announce.join("deprecated")).unwrap();
    std::fs::write(
        announce.join("deprecated/harbour.yaml"),
        "identifier: harbour\nname: Harbour\nreason: Tracker shut down\nissue_url: https://github.com/autobrr/autobrr/pull/1\ndeprecated_at: 2026-08-18T00:00:00Z\n",
    )
    .unwrap();
    st.releases.library.reload();
    st.releases.reconcile_deprecations().unwrap();
    let (_, one) = call(&app, "GET", &format!("/indexer/{id}"), None).await;
    assert_eq!(one["archived"], true, "{one}");
    assert!(one["archived_at"].is_string());
    let (s, deps) = call(&app, "GET", "/indexer/deprecations", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("IndexerDeprecation", &deps[0]);
    assert_eq!((deps[0]["identifier"].as_str(), deps[0]["filter_count"].as_i64()), (Some("harbour"), Some(1)));
    for (method, path, body) in [
        ("PUT", format!("/indexer/{id}"), Some(upd.clone())),
        ("DELETE", format!("/indexer/{id}"), None),
        ("PATCH", format!("/indexer/{id}/enabled"), Some(json!({ "enabled": true }))),
        ("POST", format!("/indexer/{id}/api/test"), Some(json!({}))),
    ] {
        let (s, v) = call(&app, method, &path, body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{method} {path}");
        assert_error(&v, "BAD_REQUEST_PARAMS", "indexer is archived");
    }
    // still used by a filter, so it stays until the filters let go of it
    let (s, v) = call(&app, "DELETE", &format!("/indexer/deprecations/{id}"), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "indexer is still used by filters");
    let (s, v) = call(&app, "POST", "/filters/indexers/prune-deprecated", Some(json!({ "identifiers": ["harbour"] }))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({ "removed": 1 })));
    let (_, f) = call(&app, "GET", &format!("/filters/{fid}"), None).await;
    assert_eq!(f["indexers"], json!([]), "{f}");
    let (s, _) = call(&app, "DELETE", &format!("/indexer/deprecations/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&app, "DELETE", &format!("/indexer/deprecations/{id}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("archived indexer with id {id} not found"));
}

/// autobrr's bundled definitions (UPSTREAM.lock's announce definitions, copied in the fixtures).
fn bundled() -> Library {
    Library::new(vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/definitions")])
}

// definition_test.go TestIndexerYamlExpectations
#[test]
fn every_bundled_definition_reads_its_own_test_lines() {
    let lib = bundled();
    assert_eq!(lib.errors(), Vec::<String>::new());
    let all = lib.all();
    assert!(all.len() > 100, "{}", all.len());
    let mut tested = 0;
    for d in all.iter().filter(|d| d.irc.is_some()) {
        for c in &d.irc.as_ref().unwrap().channels {
            for pl in c.parse.iter().flat_map(|p| p.lines.iter()) {
                for t in &pl.tests {
                    let mut out = std::collections::BTreeMap::new();
                    let ok = ParseLine::parse_line(pl, &mut out, &t.line, pl.ignore);
                    assert_eq!(ok, Ok(true), "{} {}: {}", d.name, c.name, t.line);
                    assert_eq!(out, t.expect, "{} {}: {}", d.name, c.name, t.line);
                    tested += 1;
                }
            }
        }
    }
    assert!(tested > 150, "{tested}");
}

// definition_test.go TestIndexerYamlSecretSettings
#[test]
fn secret_settings_are_secret_by_their_names_too() {
    for d in bundled().all() {
        for s in &d.settings {
            assert_eq!(s.kind.eq_ignore_ascii_case("secret"), is_secret_indexer_setting(&s.name), "{} {}", d.identifier, s.name);
        }
    }
}

// definition_test.go TestBundledDefinitionIRCAuth
#[test]
fn bundled_definitions_say_how_their_networks_sign_in() {
    let lib = bundled();
    for id in ["aither", "animebytes", "brks", "docspedia", "funfile", "superbits", "torrentleech", "zenith"] {
        let d = lib.get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(d.irc.as_ref().unwrap().auth.as_ref().map(|a| a.mechanism.as_str()), Some("NICKSERV"), "{id}");
    }
    assert!(lib.get("alpharatio").unwrap().irc.as_ref().unwrap().auth.is_none());
}

// deprecations_test.go TestDeprecatedIndexerDefinitions
#[test]
fn deprecated_indexers_have_no_live_definition() {
    let lib = bundled();
    let deps = lib.deprecations();
    assert!(!deps.is_empty());
    for d in deps {
        assert!(lib.get(&d.identifier).is_none(), "{} is deprecated but still has a definition", d.identifier);
        assert!(!d.reason.is_empty());
    }
}
