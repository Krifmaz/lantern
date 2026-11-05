//! cross-seed's four torrent clients against stand-ins of qBittorrent, Transmission, Deluge and rTorrent that
//! answer as those programs do and write down every request. The requests must be the ones cross-seed v6.13.9
//! sends, in its order (`fixtures/upstream/cross-seed/clients.json`, read from cross-seed's client code).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use base64::Engine;
use serde_json::{json, Value};
use server::crossseed::clients::{instantiate, Client, ResumeTiming, SearcheeOptions};
use server::crossseed::config::RuntimeConfig;
use server::crossseed::constants::{Decision, InjectionResult, MatchMode};
use server::crossseed::searchee::Searchee;
use server::crossseed::torrent::{self, Metafile};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The release in the client.
const TITLE: &str = "Show.Name.S01.1080p.WEB-DL-GRP";
/// Its files.
const FILES: [(&str, i64); 2] = [("Show.Name.S01E01.mkv", 700), ("Show.Name.S01E02.mkv", 800)];

/// The torrent in the client (A), the match (B) and a partial match (C).
fn metas() -> (Metafile, Metafile, Metafile) {
    let a = Metafile::decode(&torrent::build(TITLE, &FILES, "a")).unwrap();
    let b = Metafile::decode(&torrent::build(TITLE, &FILES, "b")).unwrap();
    let c = Metafile::decode(&torrent::build(TITLE, &[FILES[0], FILES[1], ("Show.Name.S01.nfo", 50)], "c")).unwrap();
    (a, b, c)
}

/// What a stand-in keeps: the request log, the torrents it has (hash, name, state, left), and whether the next
/// request finds the session gone.
#[derive(Default)]
struct Stand {
    log: Vec<Value>,
    torrents: Vec<(String, String, String, i64)>,
    expire: bool,
}

type Shared = Arc<Mutex<Stand>>;

async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

fn db(name: &str) -> kit_server_core::SqliteDb {
    let dir = std::env::temp_dir().join(format!("lantern-xs-clients-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = kit_server_core::SqliteDb::open(&dir.join("app.db")).unwrap();
    server::crossseed::migrate(&db).unwrap();
    db
}

/// The fixture's list for `client`, with the placeholders filled in.
fn expected(client: &str, a: &Metafile, b: &Metafile, c: &Metafile) -> Vec<Value> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/upstream/cross-seed/clients.json")).unwrap();
    let text = fixture[client].to_string();
    let text = text
        .replace("<HU>", &a.info_hash.to_uppercase())
        .replace("<H>", &a.info_hash)
        .replace("<B>", &b.info_hash)
        .replace("<C>", &c.info_hash)
        .replace("<F>", &format!("{TITLE}.cross-seed.torrent"));
    serde_json::from_str(&text).unwrap()
}

fn cfg(entry: String) -> RuntimeConfig {
    RuntimeConfig { torrent_clients: vec![entry], ..RuntimeConfig::default() }
}

/// The client's searchee for torrent A.
async fn searchee(client: &Client, db: &kit_server_core::SqliteDb, cfg: &RuntimeConfig, a: &Metafile) -> Searchee {
    let all = client.get_client_searchees(db, cfg, &SearcheeOptions::default()).await;
    assert_eq!(all.searchees.len(), 1);
    let s = all.searchees[0].clone();
    assert_eq!(s.info_hash.as_deref(), Some(a.info_hash.as_str()));
    assert_eq!((s.name.as_str(), s.save_path.as_deref(), s.length), (TITLE, Some("/downloads/tv"), 1500));
    assert_eq!(
        s.files.iter().map(|f| (f.path.as_str(), f.length)).collect::<Vec<_>>(),
        vec![(format!("{TITLE}/{}", FILES[0].0).as_str(), 700), (format!("{TITLE}/{}", FILES[1].0).as_str(), 800)]
    );
    assert_eq!(s.trackers.as_deref(), Some(&["tracker.example".to_string()][..]));
    assert_eq!(s.title, "Show.Name.S01.1080p.WEB-DL-GRP");
    // the second listing comes from the cache, the same
    let again = client.get_client_searchees(db, cfg, &SearcheeOptions::default()).await;
    assert_eq!(again.searchees[0].files, s.files);
    assert_eq!(again.searchees[0].info_hash, s.info_hash);
    s
}

// ---------------------------------------------------------------- qBittorrent

fn form_line(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

async fn qbit(State(st): State<Shared>, h: HeaderMap, uri: axum::http::Uri, body: Bytes) -> Response {
    let path = uri.path().to_string();
    let (a, _, _) = metas();
    let ct = h.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut s = st.lock().unwrap();
    let line = if ct.starts_with("multipart/form-data") {
        // the field names and values, in order (the torrent by its file name)
        let boundary = ct.split("boundary=").nth(1).unwrap_or("").to_string();
        let text = String::from_utf8_lossy(&body).into_owned();
        let mut fields = Vec::new();
        for part in text.split(&format!("--{boundary}")) {
            let Some((head, value)) = part.split_once("\r\n\r\n") else { continue };
            let name = head.split("name=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("");
            match head.split("filename=\"").nth(1).and_then(|r| r.split('"').next()) {
                Some(f) => fields.push(format!("{name}={f}")),
                None => fields.push(format!("{name}={}", value.trim_end_matches("\r\n"))),
            }
        }
        fields.join("|")
    } else {
        form_line(&body)
    };
    if path.ends_with("/auth/login") {
        s.log.push(json!(format!("POST {path} {line}")));
        return ([("set-cookie", "SID=abc; HttpOnly; path=/")], "Ok.").into_response();
    }
    if s.expire || h.get("cookie").and_then(|c| c.to_str().ok()) != Some("SID=abc") {
        s.expire = false;
        s.log.push(json!(format!("POST {path} 403")));
        return StatusCode::FORBIDDEN.into_response();
    }
    s.log.push(json!(format!("POST {path} {line}")));
    let info = |t: &(String, String, String, i64)| {
        json!({ "hash": t.0, "name": t.1, "state": t.2, "amount_left": t.3, "save_path": "/downloads/tv", "content_path": format!("/downloads/tv/{}", t.1),
                "category": "tv", "tags": "", "auto_tmm": false, "total_size": 1500, "tracker": "https://tracker.example/announce" })
    };
    match path.trim_start_matches("/qbit/api/v2") {
        "/app/version" => "v5.0.1".into_response(),
        "/torrents/createTags" | "/torrents/stop" | "/torrents/recheck" | "/torrents/start" => "".into_response(),
        "/torrents/info" => {
            let want = line.strip_prefix("hashes=").map(String::from);
            let list: Vec<Value> = s.torrents.iter().filter(|t| want.as_ref().is_none_or(|w| *w == t.0)).map(info).collect();
            axum::Json(Value::Array(list)).into_response()
        }
        "/torrents/files" => {
            axum::Json(json!(FILES.iter().map(|(f, n)| json!({ "name": format!("{TITLE}/{f}"), "size": n })).collect::<Vec<_>>()))
                .into_response()
        }
        "/torrents/trackers" => axum::Json(json!([{ "url": "https://tracker.example/a/announce", "tier": 0 }])).into_response(),
        "/torrents/add" => {
            // the file part's bytes: after its headers, up to the next boundary
            let find =
                |hay: &[u8], needle: &[u8], from: usize| hay[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from);
            let boundary = format!("\r\n--{}", ct.split("boundary=").nth(1).unwrap_or(""));
            let meta = find(&body, b"filename=\"", 0)
                .and_then(|f| find(&body, b"\r\n\r\n", f))
                .and_then(|s| find(&body, boundary.as_bytes(), s + 4).map(|e| &body[s + 4..e]))
                .and_then(torrent::parse)
                .map(|m| (m.info_hash, m.name));
            let _ = &a;
            let stopped = line.contains("stopped=true");
            if let Some((hash, name)) = meta {
                s.torrents.push((hash, name, if stopped { "stoppedDL".into() } else { "stalledUP".into() }, if stopped { 50 } else { 0 }));
            }
            "Ok.".into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::test]
async fn qbittorrent_as_cross_seed_talks_to_it() {
    let (a, b, c) = metas();
    let shared = Shared::default();
    shared.lock().unwrap().torrents.push((a.info_hash.clone(), TITLE.into(), "uploading".into(), 0));
    let url = serve(Router::new().fallback(post(qbit)).with_state(shared.clone())).await;
    let entry = format!("qbittorrent:{}/qbit", url.replace("http://", "http://admin:p%40ss@"));
    let cfg = cfg(entry);
    let client = instantiate(&cfg.torrent_clients).remove(0);
    let db = db("qbit");
    client.validate_config().await.unwrap();
    let s = searchee(&client, &db, &cfg, &a).await;

    // a MATCH next to the files, with the searchee's category and the cross-seed tag
    assert_eq!(client.inject(&b, &s, Decision::Match, true, None, &cfg).await, InjectionResult::Success);
    // a partial match from linked files: checked, stopped, under linkCategory
    let partial = RuntimeConfig { match_mode: MatchMode::Partial, ..cfg.clone() };
    assert_eq!(client.inject(&c, &s, Decision::MatchPartial, true, Some("/links/Site C"), &partial).await, InjectionResult::Success);
    // once checked, 50 bytes missing are within autoResumeMaxDownload: started
    let now = ResumeTiming { sleep: Duration::ZERO, err_sleep: Duration::ZERO, stop_after: Duration::from_secs(5) };
    client.resume_injection(&c, Decision::MatchPartial, true, &partial, now).await;
    // the session ran out: signed in again, then asked again
    shared.lock().unwrap().expire = true;
    assert_eq!(client.is_torrent_in_client(&b.info_hash).await, Ok(true));
    assert_eq!(client.is_torrent_in_client(&"f".repeat(40)).await, Ok(false));
    let log = shared.lock().unwrap().log.clone();
    let mut want = expected("qbittorrent", &a, &b, &c);
    want.push(json!(format!("POST /qbit/api/v2/torrents/info ")));
    assert_eq!(log, want);
    assert_eq!(client.is_torrent_complete(&b.info_hash).await, Some(true));
    assert_eq!(client.is_torrent_checking(&b.info_hash).await, Some(false));
    assert_eq!(client.get_download_dir(server::crossseed::clients::MetaRef::of_meta(&b), true, &cfg).await, Ok("/downloads/tv".into()));
}

// ---------------------------------------------------------------- Transmission

async fn transmission(State(st): State<Shared>, h: HeaderMap, body: Bytes) -> Response {
    let mut s = st.lock().unwrap();
    let mut v: Value = serde_json::from_slice(&body).unwrap();
    let session = h.get("x-transmission-session-id").and_then(|v| v.to_str().ok()) == Some("sid-1");
    assert_eq!(h.get("authorization").and_then(|v| v.to_str().ok()), Some("Basic dTpw"), "basic sign-in as u:p");
    if let Some(m) = v.pointer_mut("/arguments/metainfo") {
        let raw = base64::engine::general_purpose::STANDARD.decode(m.as_str().unwrap()).unwrap();
        let meta = torrent::parse(&raw).unwrap();
        s.torrents.push((meta.info_hash, meta.name, "0".into(), 0));
        *m = json!("<T>");
    }
    s.log.push(json!({ "session": session, "method": v["method"], "arguments": v["arguments"] }));
    if !session {
        return (StatusCode::CONFLICT, [("x-transmission-session-id", "sid-1")], "").into_response();
    }
    let ids: Option<Vec<String>> =
        v.pointer("/arguments/ids").and_then(Value::as_array).map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect());
    let torrents: Vec<Value> = s
        .torrents
        .iter()
        .filter(|t| ids.as_ref().is_none_or(|ids| ids.contains(&t.0)))
        .map(|t| {
            json!({ "hashString": t.0, "name": t.1, "status": 0, "totalSize": 1500, "labels": [], "downloadDir": "/downloads/tv", "percentDone": 1.0, "leftUntilDone": t.3,
                    "files": FILES.iter().map(|(f, n)| json!({ "name": format!("{TITLE}/{f}"), "length": n, "bytesCompleted": n })).collect::<Vec<_>>(),
                    "trackers": [{ "announce": "https://tracker.example/announce", "tier": 0 }] })
        })
        .collect();
    let args = match v["method"].as_str().unwrap() {
        "torrent-get" => json!({ "torrents": torrents }),
        "torrent-add" => json!({ "torrent-added": { "hashString": "x", "id": 2, "name": TITLE } }),
        _ => json!({}),
    };
    axum::Json(json!({ "result": "success", "arguments": args })).into_response()
}

#[tokio::test]
async fn transmission_as_cross_seed_talks_to_it() {
    let (a, b, c) = metas();
    let shared = Shared::default();
    shared.lock().unwrap().torrents.push((a.info_hash.clone(), TITLE.into(), "6".into(), 0));
    let url = serve(Router::new().fallback(post(transmission)).with_state(shared.clone())).await;
    let entry = format!("transmission:{}/transmission/rpc", url.replace("http://", "http://u:p@"));
    let cfg = cfg(entry);
    let client = instantiate(&cfg.torrent_clients).remove(0);
    let db = db("transmission");
    client.validate_config().await.unwrap();
    let s = searchee(&client, &db, &cfg, &a).await;
    assert_eq!(client.inject(&b, &s, Decision::Match, true, None, &cfg).await, InjectionResult::Success);
    let now = ResumeTiming { sleep: Duration::ZERO, err_sleep: Duration::ZERO, stop_after: Duration::from_secs(5) };
    client.resume_injection(&b, Decision::Match, true, &cfg, now).await;
    assert_eq!(shared.lock().unwrap().log, expected("transmission", &a, &b, &c));
}

// ---------------------------------------------------------------- Deluge

async fn deluge(State(st): State<Shared>, h: HeaderMap, body: Bytes) -> Response {
    let mut s = st.lock().unwrap();
    let mut v: Value = serde_json::from_slice(&body).unwrap();
    let cookie = h.get("cookie").and_then(|v| v.to_str().ok()) == Some("_session_id=abc");
    let method = v["method"].as_str().unwrap().to_string();
    if method == "core.add_torrent_file" {
        let raw = base64::engine::general_purpose::STANDARD.decode(v["params"][1].as_str().unwrap()).unwrap();
        let meta = torrent::parse(&raw).unwrap();
        s.torrents.push((meta.info_hash, meta.name, "Seeding".into(), 0));
        v["params"][1] = json!("<T>");
    }
    s.log.push(json!({ "method": method, "params": v["params"], "cookie": cookie }));
    let id = v["id"].clone();
    let torrents: serde_json::Map<String, Value> = s
        .torrents
        .iter()
        .filter(|t| v["params"][1]["hash"].as_str().is_none_or(|h| h == t.0))
        .map(|t| {
            (t.0.clone(), json!({ "name": t.1, "label": "tv", "save_path": "/downloads/tv", "total_size": 1500, "state": t.2, "progress": 100.0, "total_remaining": 0,
                    "files": FILES.iter().map(|(f, n)| json!({ "path": format!("{TITLE}/{f}"), "size": n })).collect::<Vec<_>>(),
                    "trackers": [{ "url": "https://tracker.example/announce", "tier": 0 }] }))
        })
        .collect();
    let result = match method.as_str() {
        "auth.login" => {
            return ([("set-cookie", "_session_id=abc; Path=/json")], axum::Json(json!({ "result": true, "error": null, "id": id })))
                .into_response()
        }
        "web.connected" => json!(true),
        "core.get_enabled_plugins" => json!(["Label"]),
        "web.update_ui" => json!({ "torrents": torrents }),
        "label.get_labels" => json!(["tv"]),
        _ => Value::Null,
    };
    axum::Json(json!({ "result": result, "error": null, "id": id })).into_response()
}

#[tokio::test]
async fn deluge_as_cross_seed_talks_to_it() {
    let (a, b, c) = metas();
    let shared = Shared::default();
    shared.lock().unwrap().torrents.push((a.info_hash.clone(), TITLE.into(), "Seeding".into(), 0));
    let url = serve(Router::new().fallback(post(deluge)).with_state(shared.clone())).await;
    let entry = format!("deluge:{}/json", url.replace("http://", "http://:p%40ss@"));
    let cfg = cfg(entry);
    let client = instantiate(&cfg.torrent_clients).remove(0);
    let db = db("deluge");
    client.validate_config().await.unwrap();
    let mut s = searchee(&client, &db, &cfg, &a).await;
    assert_eq!(s.category.as_deref(), Some("tv"), "Deluge's label is the category");
    s.trackers = None;
    assert_eq!(client.inject(&b, &s, Decision::Match, true, None, &cfg).await, InjectionResult::Success);
    assert_eq!(shared.lock().unwrap().log, expected("deluge", &a, &b, &c));
    // a password is needed, in cross-seed's words
    let none = instantiate(&[format!("deluge:{url}/json")]).remove(0);
    let e = none.validate_config().await.unwrap_err();
    assert!(e.ends_with("You need to define a password in the delugeRpcUrl. (e.g. http://:<PASSWORD>@localhost:8112)"), "{e}");
}

// ---------------------------------------------------------------- rTorrent

/// The texts of `<string>` and `<base64>` values in an XML-RPC call, in order (`<T>` for base64).
fn xml_strings(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    loop {
        let s = rest.find("<string>");
        let b = rest.find("<base64>");
        match (s, b) {
            (Some(i), b) if b.is_none_or(|b| i < b) => {
                let after = &rest[i + 8..];
                let end = after.find("</string>").unwrap();
                out.push(quick_xml::escape::unescape(&after[..end]).unwrap().into_owned());
                rest = &after[end..];
            }
            (_, Some(i)) => {
                let after = &rest[i + 8..];
                let end = after.find("</base64>").unwrap();
                out.push("<T>".into());
                rest = &after[end..];
            }
            _ => return out,
        }
    }
}

fn xml_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("<value><string>{s}</string></value>"),
        Value::Number(n) => format!("<value><i8>{n}</i8></value>"),
        Value::Array(a) => format!("<value><array><data>{}</data></array></value>", a.iter().map(xml_value).collect::<String>()),
        _ => "<value/>".into(),
    }
}

fn xml_answer(v: &Value) -> String {
    format!("<?xml version=\"1.0\"?><methodResponse><params><param>{}</param></params></methodResponse>", xml_value(v))
}

async fn rtorrent(State(st): State<Shared>, body: Bytes) -> Response {
    let xml = String::from_utf8_lossy(&body).into_owned();
    let method = xml.split("<methodName>").nth(1).and_then(|r| r.split("</methodName>").next()).unwrap_or("").to_string();
    let strings = xml_strings(&xml);
    let mut s = st.lock().unwrap();
    let answer = match method.as_str() {
        "download_list" => {
            s.log.push(json!("download_list"));
            json!(s.torrents.iter().map(|t| t.0.to_uppercase()).collect::<Vec<_>>())
        }
        "system.multicall" => {
            // methodName, then its params, for each call (`f.path=` and the like are params of a multicall)
            let mut calls: Vec<(String, Vec<String>)> = Vec::new();
            for x in strings {
                let method = x.starts_with("d.") || x.starts_with("f.multicall") || x.starts_with("t.multicall");
                let param_of_multicall = x.ends_with('=') && calls.last().is_some_and(|c| c.1.len() >= 2);
                if method && !param_of_multicall {
                    calls.push((x, Vec::new()));
                    continue;
                }
                calls.last_mut().unwrap().1.push(x);
            }
            let shown: Vec<String> = calls.iter().map(|(m, p)| format!("{m}({})", p.join(","))).collect();
            s.log.push(json!(format!("system.multicall {}", shown.join(" "))));
            let dir = format!("/downloads/tv/{TITLE}");
            json!(calls
                .iter()
                .map(|(m, _)| match m.as_str() {
                    "d.name" => json!([TITLE]),
                    "d.size_bytes" => json!(["1500"]),
                    "d.directory" => json!([dir]),
                    "d.is_multi_file" | "d.complete" => json!(["1"]),
                    "d.custom1" => json!([""]),
                    "d.left_bytes" | "d.hashing" | "d.is_active" => json!(["0"]),
                    "f.multicall" => json!([FILES.iter().map(|(f, n)| json!([f, n.to_string()])).collect::<Vec<_>>()]),
                    "t.multicall" => json!([[["https://tracker.example/announce", "0"]]]),
                    _ => json!([]),
                })
                .collect::<Vec<_>>())
        }
        m if m.starts_with("load.raw") => {
            let b64 = xml.split("<base64>").nth(1).and_then(|r| r.split("</base64>").next()).unwrap();
            let raw = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
            assert!(raw.windows(20).any(|w| w == b"17:libtorrent_resume"), "the resume data goes with the torrent");
            let meta = torrent::parse(&raw).unwrap();
            s.torrents.push((meta.info_hash, meta.name, String::new(), 0));
            let params: Vec<String> = strings
                .iter()
                .map(|p| if p.starts_with("d.custom.set=addtime,") { "d.custom.set=addtime,<time>".to_string() } else { p.clone() })
                .collect();
            s.log.push(json!(format!("{m} {}", params.join(","))));
            json!(0)
        }
        m => {
            s.log.push(json!(m));
            json!(0)
        }
    };
    ([("content-type", "text/xml")], xml_answer(&answer)).into_response()
}

#[tokio::test]
async fn rtorrent_as_cross_seed_talks_to_it() {
    let (a, b, c) = metas();
    let shared = Shared::default();
    shared.lock().unwrap().torrents.push((a.info_hash.clone(), TITLE.into(), String::new(), 0));
    let url = serve(Router::new().fallback(post(rtorrent)).with_state(shared.clone())).await;
    let cfg = cfg(format!("rtorrent:{url}/RPC2"));
    let client = instantiate(&cfg.torrent_clients).remove(0);
    let db = db("rtorrent");
    client.validate_config().await.unwrap();
    let s = searchee(&client, &db, &cfg, &a).await;
    assert_eq!(client.inject(&b, &s, Decision::Match, true, None, &cfg).await, InjectionResult::Success);
    assert_eq!(shared.lock().unwrap().log, expected("rtorrent", &a, &b, &c));
}

// ---------------------------------------------------------------- linking and the actions

/// cross-seed's `performAction` for a release found in a data folder: its files are linked into the link folder
/// (under the site's name, or straight in with `flatLinking`) and the match goes to the client from there, under
/// `linkCategory`; with `action: save` the `.torrent` is saved instead.
#[tokio::test]
async fn a_match_from_a_data_folder_is_linked_then_injected() {
    use server::crossseed::action::{perform_action_without_mutex, Ctx};
    use server::crossseed::constants::{Action, LinkType};
    let (a, b, _) = metas();
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let root = std::env::temp_dir().join(format!("lantern-xs-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data").join(TITLE);
    std::fs::create_dir_all(&data).unwrap();
    for (f, n) in FILES {
        std::fs::write(data.join(f), vec![b'x'; n as usize]).unwrap();
    }
    let links = root.join("links");
    std::fs::create_dir_all(&links).unwrap();
    let searchee = server::crossseed::searchee::create_searchee_from_path(&data).unwrap();

    // the client holds other torrents (cross-seed won't add to an empty one)
    let shared = Shared::default();
    shared.lock().unwrap().torrents.push((a.info_hash.clone(), TITLE.into(), "uploading".into(), 0));
    let url = serve(Router::new().fallback(post(qbit)).with_state(shared.clone())).await;
    let entry = format!("qbittorrent:{}/qbit", url.replace("http://", "http://admin:p%40ss@"));
    let base = RuntimeConfig { link_dirs: vec![links.to_string_lossy().into_owned()], link_type: LinkType::Hardlink, ..cfg(entry) };
    let ctx = |cfg: RuntimeConfig| Ctx { clients: instantiate(&cfg.torrent_clients), db: db("link"), output_dir: root.join("out"), cfg };

    // linked under the site's name, then added from there
    let c = ctx(base.clone());
    let r = perform_action_without_mutex(&c, &b, Decision::Match, &searchee, "Site B", None, true).await;
    assert_eq!(r.action_result, InjectionResult::Success, "{:?}", shared.lock().unwrap().log);
    assert!(r.linked_new_files);
    let site_dir = links.join("Site B");
    assert_eq!(r.destination_dir.as_deref(), Some(site_dir.to_string_lossy().as_ref()));
    #[cfg(unix)]
    for (f, _) in FILES {
        use std::os::unix::fs::MetadataExt;
        let linked = std::fs::metadata(site_dir.join(TITLE).join(f)).unwrap();
        assert_eq!(linked.ino(), std::fs::metadata(data.join(f)).unwrap().ino(), "a hard link to {f}");
    }
    let add = shared.lock().unwrap().log.iter().filter_map(Value::as_str).find(|l| l.contains("/torrents/add")).unwrap().to_string();
    assert!(add.contains(&format!("savepath={}|", site_dir.display())), "{add}");
    assert!(add.contains("category=cross-seed-link|"), "{add}");
    // linking again finds the files there: nothing new is linked
    shared.lock().unwrap().torrents.retain(|t| t.0 == a.info_hash);
    let r = perform_action_without_mutex(&c, &b, Decision::Match, &searchee, "Site B", None, true).await;
    assert_eq!((r.action_result, r.linked_new_files), (InjectionResult::Success, false));

    // flatLinking: straight into the link folder, as symbolic links
    shared.lock().unwrap().torrents.retain(|t| t.0 == a.info_hash);
    let flat = ctx(RuntimeConfig { flat_linking: true, link_type: LinkType::Symlink, ..base.clone() });
    std::fs::remove_dir_all(&site_dir).unwrap();
    let r = perform_action_without_mutex(&flat, &b, Decision::Match, &searchee, "Site B", None, true).await;
    assert_eq!(r.action_result, InjectionResult::Success);
    assert_eq!(r.destination_dir.as_deref(), Some(links.to_string_lossy().as_ref()));
    let link = links.join(TITLE).join(FILES[0].0);
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), data.join(FILES[0].0));
    assert!(!site_dir.exists());

    // action: save keeps the .torrent in the output folder and adds nothing
    let saved = ctx(RuntimeConfig { action: Action::Save, ..base });
    let before = shared.lock().unwrap().log.len();
    let r = perform_action_without_mutex(&saved, &b, Decision::Match, &searchee, "Site B", None, true).await;
    assert_eq!(r.action_result, InjectionResult::Saved);
    assert_eq!(shared.lock().unwrap().log.len(), before, "the client isn't asked");
    let names: Vec<String> =
        std::fs::read_dir(root.join("out")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec![format!("[pack][Site B]{TITLE}[{}].torrent", b.info_hash)]);
    let _ = std::fs::remove_dir_all(&root);
}
