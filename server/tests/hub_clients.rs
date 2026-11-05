//! Every download client kind through the hub: a fake Transmission, Deluge, rTorrent and NZBGet on one local
//! port, each speaking its own protocol (session ids, Web UI sign-in, XML-RPC, JSON-RPC with Basic auth). For each,
//! an app registers, a grab is sent into the app's folder, followed to the end, and its files reported.

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// What the fake clients hold.
#[derive(Default)]
struct Fake {
    /// Transmission's torrents.
    tr: Vec<Value>,
    /// Deluge's torrents: hash → status.
    de: serde_json::Map<String, Value>,
    /// Whether Deluge's Web UI is connected to its daemon.
    de_connected: bool,
    /// Deluge's labels.
    de_labels: Vec<String>,
    /// Deluge's torrent labels: hash → label.
    de_label_of: HashMap<String, String>,
    /// rTorrent's torrents: (hash, name, directory, label, tag, complete).
    rt: Vec<(String, String, String, String, String, bool)>,
    /// NZBGet's queue.
    nz_queue: Vec<Value>,
    /// NZBGet's history.
    nz_history: Vec<Value>,
    /// What NZBGet's append got.
    nz_appended: Vec<Value>,
    /// Ids the clients were told to remove.
    removed: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

/// Whether `h` has Basic auth `admin:pw`.
fn basic_ok(h: &HeaderMap) -> bool {
    let want = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("admin:pw"));
    h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some(want.as_str())
}

async fn transmission(State(s): State<Shared>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if !basic_ok(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if h.get("x-transmission-session-id").and_then(|v| v.to_str().ok()) != Some("sess1") {
        return (StatusCode::CONFLICT, [("x-transmission-session-id", "sess1")]).into_response();
    }
    let mut s = s.lock().unwrap();
    let a = &b["arguments"];
    let args = match b["method"].as_str().unwrap_or("") {
        "session-get" => json!({ "version": "4.0.6" }),
        "torrent-add" => {
            assert!(a["filename"].as_str().unwrap().starts_with("magnet:"));
            s.tr.push(json!({ "hashString": "th1", "name": "Rel", "status": 4, "percentDone": 0.5, "totalSize": 7,
                "rateDownload": 3, "eta": 2, "downloadDir": a["download-dir"], "labels": a["labels"], "error": 0 }));
            json!({ "torrent-added": { "hashString": "th1", "name": "Rel", "id": 1 } })
        }
        "torrent-get" if a.get("ids").is_some() => {
            json!({ "torrents": [{ "downloadDir": s.tr[0]["downloadDir"], "files": [{ "name": "Rel/a.mkv", "length": 7 }] }] })
        }
        "torrent-get" => json!({ "torrents": s.tr }),
        "torrent-remove" => {
            s.removed.push(a["ids"][0].as_str().unwrap_or("").to_string());
            json!({})
        }
        _ => return Json(json!({ "result": "method name not recognized" })).into_response(),
    };
    Json(json!({ "result": "success", "arguments": args })).into_response()
}

async fn deluge(State(s): State<Shared>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    let method = b["method"].as_str().unwrap_or("").to_string();
    let p = &b["params"];
    let mut s = s.lock().unwrap();
    let ok = |r: Value| Json(json!({ "id": b["id"], "result": r, "error": null })).into_response();
    if method == "auth.login" {
        return if p[0] == "pw" {
            ([(header::SET_COOKIE, "_session_id=d1; path=/")], Json(json!({ "id": b["id"], "result": true, "error": null })))
                .into_response()
        } else {
            ok(json!(false))
        };
    }
    let signed = h.get(header::COOKIE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("_session_id=d1"));
    if !signed {
        return Json(json!({ "id": b["id"], "result": null, "error": { "message": "Not authenticated", "code": 1 } })).into_response();
    }
    match method.as_str() {
        "web.connected" => ok(json!(s.de_connected)),
        "web.get_hosts" => ok(json!([["host1", "127.0.0.1", 58846, "localclient"]])),
        "web.connect" => {
            s.de_connected = true;
            ok(json!([]))
        }
        "daemon.info" if s.de_connected => ok(json!("2.1.1")),
        "label.get_labels" => ok(json!(s.de_labels)),
        "label.add" => {
            s.de_labels.push(p[0].as_str().unwrap().to_string());
            ok(Value::Null)
        }
        "core.add_torrent_magnet" => {
            s.de.insert(
                "dh1".into(),
                json!({ "name": "Rel", "state": "Downloading", "progress": 50.0, "total_size": 7, "save_path": p[1]["download_location"] }),
            );
            ok(json!("dh1"))
        }
        "label.set_torrent" => {
            s.de_label_of.insert(p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into());
            ok(Value::Null)
        }
        "core.get_torrents_status" => ok(Value::Object(s.de.clone())),
        "core.get_torrent_status" => ok(json!({ "save_path": s.de["dh1"]["save_path"], "files": [{ "path": "Rel/a.mkv", "size": 7 }] })),
        "core.remove_torrent" => {
            s.removed.push(p[0].as_str().unwrap().into());
            ok(json!(true))
        }
        m => {
            Json(json!({ "id": b["id"], "result": null, "error": { "message": format!("Unknown method {m}"), "code": 2 } })).into_response()
        }
    }
}

/// An XML-RPC answer holding `inner` (a `<value>…</value>`).
fn xml_answer(inner: &str) -> Response {
    (
        [(header::CONTENT_TYPE, "text/xml")],
        format!("<?xml version=\"1.0\"?><methodResponse><params><param>{inner}</param></params></methodResponse>"),
    )
        .into_response()
}

/// The text of every `<string>` in `body`, in order.
fn strings(body: &str) -> Vec<String> {
    body.split("<string>")
        .skip(1)
        .map(|p| p.split("</string>").next().unwrap_or("").replace("&quot;", "\"").replace("&amp;", "&"))
        .collect()
}

async fn rtorrent(State(s): State<Shared>, body: String) -> Response {
    let method = body.split("<methodName>").nth(1).and_then(|p| p.split("</methodName>").next()).unwrap_or("").to_string();
    let args = strings(&body);
    let mut s = s.lock().unwrap();
    match method.as_str() {
        "system.client_version" => xml_answer("<value><string>0.9.8</string></value>"),
        "load.start" => {
            let get = |prefix: &str| args.iter().find_map(|a| a.strip_prefix(prefix)).unwrap_or("").trim_matches('"').to_string();
            assert!(args[1].starts_with("magnet:"), "{args:?}");
            s.rt.push(("RH1".into(), "Rel".into(), format!("{}/Rel", get("d.directory.set=")), get("d.custom1.set="), get("d.custom2.set="), false));
            xml_answer("<value><i8>0</i8></value>")
        }
        "d.multicall2" => {
            let rows: String = s
                .rt
                .iter()
                .map(|(h, n, d, _, tag, done)| {
                    let c = if *done { 7 } else { 3 };
                    format!(
                        "<value><array><data><value><string>{h}</string></value><value><string>{n}</string></value><value><i8>7</i8></value>\
                         <value><i8>{c}</i8></value><value><i8>1</i8></value><value><i8>{}</i8></value><value><i8>1</i8></value><value><i8>1</i8></value>\
                         <value><string>{d}</string></value><value><i8>0</i8></value><value><string>{tag}</string></value><value><string></string></value>\
                         <value><i8>0</i8></value><value><i8>1</i8></value></data></array></value>",
                        *done as i32
                    )
                })
                .collect();
            xml_answer(&format!("<value><array><data>{rows}</data></array></value>"))
        }
        "f.multicall" => xml_answer("<value><array><data><value><array><data><value><string>a.mkv</string></value><value><i8>7</i8></value></data></array></value></data></array></value>"),
        "d.erase" => {
            s.removed.push(args[0].clone());
            xml_answer("<value><i8>0</i8></value>")
        }
        m => ([(header::CONTENT_TYPE, "text/xml")], format!("<methodResponse><fault><value><struct><member><name>faultCode</name><value><int>-506</int></value></member><member><name>faultString</name><value><string>Method '{m}' not defined</string></value></member></struct></value></fault></methodResponse>")).into_response(),
    }
}

async fn nzbget(State(s): State<Shared>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if !basic_ok(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let p = &b["params"];
    let mut s = s.lock().unwrap();
    let r = match b["method"].as_str().unwrap_or("") {
        "version" => json!("24.3"),
        "append" => {
            s.nz_appended.push(p.clone());
            s.nz_queue.push(json!({ "NZBID": 42, "NZBName": "Rel", "Status": "DOWNLOADING", "FileSizeMB": 10, "RemainingSizeMB": 5, "DestDir": "/inter/Rel.#42" }));
            json!(42)
        }
        "listgroups" => json!(s.nz_queue),
        "history" => json!(s.nz_history),
        "editqueue" => {
            s.removed.push(format!("{}:{}", p[0].as_str().unwrap_or(""), p[2][0]));
            json!(true)
        }
        _ => return Json(json!({ "error": { "message": "Invalid procedure" } })).into_response(),
    };
    Json(json!({ "version": "1.1", "result": r })).into_response()
}

/// The fake clients, listening on a local port: (their address, what they hold).
async fn fakes() -> (String, Shared) {
    let s: Shared = Default::default();
    let app = Router::new()
        .route("/transmission/rpc", post(transmission))
        .route("/json", post(deluge))
        .route("/RPC2", post(rtorrent))
        .route("/jsonrpc", post(nzbget))
        .route("/files/rel.nzb", get(|| async { "<?xml version=\"1.0\"?><nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"></nzb>" }))
        .with_state(s.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), s)
}

/// A fresh Lantern with its downloads folder in its own test folder.
async fn server(name: &str) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-clients-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let downloads = dir.join("downloads").to_string_lossy().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some("test-key".into()),
        "APP_DOWNLOADS_DIR" => Some(downloads.clone()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    (server::app(st.clone()), st, dir)
}

/// One request with the API key; the status and the JSON body.
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("x-api-key", "test-key");
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Adds client `client`, checks its test says `version`, registers app `app`, and queues a grab of `url` for it;
/// the grab's id and the app's folder.
async fn start(app: &Router, st: &server::AppState, client: Value, version: &str, name: &str, url: &str) -> (i64, String) {
    let (s, c) = call(app, "POST", "/api/hub/clients", Some(client)).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    let (_, t) = call(app, "POST", &format!("/api/hub/clients/{}/test", c["id"]), None).await;
    assert_eq!(t, json!({ "ok": true, "message": format!("Connected to {version}") }));
    let (_, a) = call(app, "POST", "/api/hub/apps", Some(json!({ "name": name }))).await;
    let (s, g) = call(app, "POST", "/api/hub/grabs", Some(json!({ "app": name, "url": url }))).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    st.hub.tick().await;
    (g["id"].as_i64().unwrap(), a["folder"].as_str().unwrap().to_string())
}

/// The grab's state, progress and files.
fn seen(st: &server::AppState, id: i64) -> (String, f64, Vec<(String, i64)>) {
    let g = st.hub.get_grab(id).unwrap();
    (g.state, g.progress, g.files)
}

const MAGNET: &str = "magnet:?xt=urn:btih:abcdef&dn=Rel";

#[tokio::test]
async fn transmission_gets_the_session_id_the_folder_and_the_labels() {
    let (base, fake) = fakes().await;
    let (app, st, _) = server("transmission").await;
    let client = json!({ "kind": "transmission", "url": base, "username": "admin", "password": "pw" });
    let (id, folder) = start(&app, &st, client, "Transmission 4.0.6", "Radarr", MAGNET).await;
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.tr[0]["downloadDir"], json!(folder));
        assert_eq!(f.tr[0]["labels"], json!(["lantern", format!("lantern-{id}"), "radarr"]));
    }
    assert_eq!(seen(&st, id).0, "downloading");
    assert_eq!(seen(&st, id).1, 0.5);
    {
        let mut f = fake.lock().unwrap();
        f.tr[0]["status"] = json!(6);
        f.tr[0]["percentDone"] = json!(1.0);
    }
    st.hub.tick().await;
    assert_eq!(seen(&st, id), ("seeding".to_string(), 1.0, vec![(format!("{folder}/Rel/a.mkv"), 7)]));
    let (s, _) = call(&app, "DELETE", &format!("/api/hub/grabs/{id}?delete_files=true"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(fake.lock().unwrap().removed, ["th1"]);
}

#[tokio::test]
async fn deluge_signs_in_connects_its_daemon_and_labels_the_torrent() {
    let (base, fake) = fakes().await;
    let (app, st, _) = server("deluge").await;
    let client = json!({ "kind": "deluge", "url": base, "password": "pw" });
    let (id, folder) = start(&app, &st, client, "Deluge 2.1.1", "Lidarr", MAGNET).await;
    {
        let f = fake.lock().unwrap();
        assert!(f.de_connected);
        assert_eq!(f.de_labels, ["lidarr"]);
        assert_eq!(f.de_label_of.get("dh1").map(String::as_str), Some("lidarr"));
        assert_eq!(f.de["dh1"]["save_path"], json!(folder));
    }
    assert_eq!(seen(&st, id).0, "downloading");
    {
        let mut f = fake.lock().unwrap();
        let t = f.de.get_mut("dh1").unwrap();
        t["state"] = json!("Paused");
        t["progress"] = json!(100.0);
    }
    st.hub.tick().await;
    assert_eq!(seen(&st, id), ("completed".to_string(), 1.0, vec![(format!("{folder}/Rel/a.mkv"), 7)]));

    let (_, c) = call(&app, "GET", "/api/hub/clients", None).await;
    let kinds: Vec<&str> =
        c["kinds"].as_array().unwrap().iter().filter(|k| k["default"] == true).map(|k| k["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["qbittorrent", "sabnzbd"]);
}

#[tokio::test]
async fn rtorrent_takes_xml_rpc_and_is_found_again_by_its_tag() {
    let (base, fake) = fakes().await;
    let (app, st, _) = server("rtorrent").await;
    let client = json!({ "kind": "rtorrent", "url": base });
    let (id, folder) = start(&app, &st, client, "rTorrent 0.9.8", "Sonarr", MAGNET).await;
    {
        let f = fake.lock().unwrap();
        let (_, _, dir, label, tag, _) = &f.rt[0];
        assert_eq!((dir.as_str(), label.as_str(), tag.clone()), (format!("{folder}/Rel").as_str(), "sonarr", format!("lantern-{id}")));
    }
    let g = st.hub.get_grab(id).unwrap();
    assert_eq!((g.state.as_str(), g.client_ref.as_str()), ("downloading", "RH1"));
    fake.lock().unwrap().rt[0].5 = true;
    st.hub.tick().await;
    assert_eq!(seen(&st, id), ("seeding".to_string(), 1.0, vec![(format!("{folder}/Rel/a.mkv"), 7)]));
    assert_eq!(st.hub.get_grab(id).unwrap().path, format!("{folder}/Rel"));
}

#[tokio::test]
async fn nzbget_gets_the_category_and_its_folder_is_read_when_done() {
    let (base, fake) = fakes().await;
    let (app, st, _) = server("nzbget").await;
    let client = json!({ "kind": "nzbget", "url": base, "username": "admin", "password": "pw" });
    let (id, folder) = start(&app, &st, client, "NZBGet 24.3", "Readarr", &format!("{base}/files/rel.nzb")).await;
    {
        let f = fake.lock().unwrap();
        let p = &f.nz_appended[0];
        assert_eq!((p[0].as_str(), p[2].as_str()), (Some("rel.nzb"), Some("readarr")));
        assert_eq!(p[9][0]["Value"], format!("lantern-{id}"));
    }
    let g = st.hub.get_grab(id).unwrap();
    assert_eq!((g.state.as_str(), g.client_ref.as_str(), g.progress), ("downloading", "42", 0.5));
    let done = PathBuf::from(&folder).join("Rel");
    std::fs::create_dir_all(&done).unwrap();
    std::fs::write(done.join("book.epub"), b"1234567").unwrap();
    {
        let mut f = fake.lock().unwrap();
        f.nz_queue.clear();
        f.nz_history
            .push(json!({ "NZBID": 42, "Name": "Rel", "Status": "SUCCESS/ALL", "DestDir": done.to_string_lossy(), "FileSizeMB": 1 }));
    }
    st.hub.tick().await;
    assert_eq!(seen(&st, id), ("completed".to_string(), 1.0, vec![(done.join("book.epub").to_string_lossy().to_string(), 7)]));

    // a client of the wrong protocol is never picked, and a failed NZB is removed and retried
    let (_, g2) =
        call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Readarr", "url": format!("{base}/files/rel.nzb"), "title": "Two" })))
            .await;
    let id2 = g2["id"].as_i64().unwrap();
    st.hub.tick().await;
    {
        let mut f = fake.lock().unwrap();
        f.nz_queue.clear();
        f.nz_history.retain(|h| h["NZBID"] != 42);
        f.nz_history.push(json!({ "NZBID": 42, "Name": "Two", "Status": "FAILURE/PAR" }));
    }
    st.hub.tick().await;
    let g2 = st.hub.get_grab(id2).unwrap();
    assert_eq!((g2.state.as_str(), g2.attempts), ("queued", 1));
    assert_eq!(g2.error, "NZBGet couldn't finish the download (FAILURE/PAR)");
    assert!(fake.lock().unwrap().removed.contains(&"HistoryFinalDelete:42".to_string()));
}
