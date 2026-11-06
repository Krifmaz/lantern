//! Prowlarr's download clients (`/api/v1/downloadclient`) and notifications (`/api/v1/notification`) as Lantern
//! runs them: the schema, checks and tests with Prowlarr's answers, the hub seeing the same clients, and each
//! client and notification service talking to a local stand-in (never the internet).

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use kit_server_core::{p, Db};
use serde_json::{json, Value};
use server::indexers::cardigann::Grabbed;
use server::indexers::release::{Protocol, Release};
use server::prowlarr::apps::downloads::{self, DownloadClients};
use server::prowlarr::apps::provider;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;

/// The API key the tests' Lantern has.
const KEY: &str = "test-key";

/// A fresh Lantern on a data folder of its own.
async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-pconnect-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let st = server::AppState::open(&dir.join("data"), web, |n| (n == "APP_API_KEY").then(|| KEY.to_string())).await.unwrap();
    (server::app(st.clone()), st)
}

/// One request: status and body (as JSON, `Null` when it is not JSON).
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

/// The new provider of `implementation` from `/api/v1/<resource>/schema`.
async fn template(app: &Router, resource: &str, implementation: &str) -> Value {
    let (s, list) = call(app, "GET", &format!("/api/v1/{resource}/schema"), None).await;
    assert_eq!(s, StatusCode::OK);
    list.as_array()
        .unwrap()
        .iter()
        .find(|t| t["implementation"] == implementation)
        .cloned()
        .unwrap_or_else(|| panic!("no {implementation}"))
}

/// Sets one field of a resource.
fn set(r: &mut Value, name: &str, value: Value) {
    let f = r["fields"].as_array_mut().unwrap().iter_mut().find(|f| f["name"] == name).unwrap_or_else(|| panic!("no field {name}"));
    f["value"] = value;
}

/// One field of a resource.
fn field<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap_or_else(|| panic!("no field {name}"))
}

/// What the stand-in was sent: method, path with query, headers, body.
type Log = Arc<Mutex<Vec<(String, String, HeaderMap, String)>>>;

/// One stand-in for every client and service: it notes each request and answers as the real one would.
async fn stand_in(State(log): State<Log>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let text = String::from_utf8_lossy(&body).to_string();
    log.lock().unwrap().push((
        method.to_string(),
        format!("{path}{}{query}", if query.is_empty() { "" } else { "?" }),
        headers.clone(),
        text.clone(),
    ));
    let xml = |inner: &str| {
        let body =
            format!("<?xml version=\"1.0\"?><methodResponse><params><param><value>{inner}</value></param></params></methodResponse>");
        ([(header::CONTENT_TYPE, "text/xml")], body).into_response()
    };
    match path.as_str() {
        "/rpc" if text.contains("aria2.getVersion") => {
            if !text.contains("token:secret") {
                let fault = "<?xml version=\"1.0\"?><methodResponse><fault><value><struct><member><name>faultCode</name><value><int>1</int></value></member><member><name>faultString</name><value><string>Unauthorized</string></value></member></struct></value></fault></methodResponse>";
                return ([(header::CONTENT_TYPE, "text/xml")], fault).into_response();
            }
            xml("<struct><member><name>version</name><value><string>1.36.0</string></value></member></struct>")
        }
        "/rpc" => xml("<string>2089b05ecca3d829</string>"),
        "/flood/api/auth/authenticate" => {
            let ok = serde_json::from_str::<Value>(&text).ok().is_some_and(|v| v["password"] == "pw");
            if ok {
                ([(header::SET_COOKIE, "jwt=abc; Path=/")], "{\"success\":true}").into_response()
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            }
        }
        "/flood/api/auth/verify" if headers.get(header::COOKIE).is_some() => "{}".into_response(),
        "/flood/api/auth/verify" => StatusCode::UNAUTHORIZED.into_response(),
        "/gui/token.html" => "<html><div id='token' style='display:none;'>TOK123</div></html>".into_response(),
        "/gui/" if query.contains("getsettings") => axum::Json(json!({ "build": 46000, "settings": [] })).into_response(),
        "/gui/" => axum::Json(json!({ "build": 46000 })).into_response(),
        "/hadouken/api" => {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            axum::Json(json!({ "id": v["id"], "result": { "versions": { "hadouken": "5.1" } } })).into_response()
        }
        p if p.starts_with("/botBAD") => {
            (StatusCode::BAD_REQUEST, axum::Json(json!({ "ok": false, "description": "Bad Request: chat not found" }))).into_response()
        }
        p if p.starts_with("/gotify/message") && !query.contains("token=app") => StatusCode::UNAUTHORIZED.into_response(),
        _ => axum::Json(json!({ "ok": true, "success": true })).into_response(),
    }
}

/// Starts the stand-in; its address and what it was sent.
async fn start_stand_in() -> (String, Log) {
    let log: Log = Default::default();
    let app = Router::new().fallback(stand_in).with_state(log.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), log)
}

/// The requests the stand-in got to `path_start`, as (method, path, body).
fn sent(log: &Log, path_start: &str) -> Vec<(String, String, String)> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(_, p, _, _)| p.starts_with(path_start))
        .map(|(m, p, _, b)| (m.clone(), p.clone(), b.clone()))
        .collect()
}

/// The host and port of an address.
fn host_port(base: &str) -> (String, i64) {
    let u = url::Url::parse(base).unwrap();
    (u.host_str().unwrap().to_string(), i64::from(u.port().unwrap()))
}

/// A release to send.
fn release(protocol: Protocol) -> Release {
    Release {
        title: "Show.Name.S01E02.1080p.WEB-DL-GRP".into(),
        indexer: "Tracker".into(),
        protocol,
        categories: vec![5040],
        ..Default::default()
    }
}

#[tokio::test]
async fn download_clients_answer_and_check_as_prowlarr_does() {
    let (app, st) = lantern("dl-api").await;

    // the templates: every client, by name, with Prowlarr's defaults and choices
    let (_, list) = call(&app, "GET", "/api/v1/downloadclient/schema", None).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|t| t["implementationName"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 18);
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "{names:?}");
    let q = template(&app, "downloadclient", "QBittorrent").await;
    assert_eq!((q["enable"].clone(), q["priority"].clone(), q["protocol"].clone()), (json!(false), json!(1), json!("torrent")));
    assert_eq!(
        (q["supportsCategories"].clone(), q["categories"].clone(), q["configContract"].clone()),
        (json!(true), json!([]), json!("QBittorrentSettings"))
    );
    assert_eq!((field(&q, "host")["value"].clone(), field(&q, "port")["value"].clone()), (json!("localhost"), json!(8080)));
    assert_eq!(field(&q, "category")["value"], "prowlarr");
    let states: Vec<Value> = field(&q, "initialState")["selectOptions"].as_array().unwrap().iter().map(|o| o["name"].clone()).collect();
    assert_eq!(states, [json!("Started"), json!("Force Started"), json!("Stopped")]);
    assert_eq!(field(&q, "urlBase")["helpText"], "Adds a prefix to the qBittorrent url, such as http://[host]:[port]/[urlBase]/api");
    let r = template(&app, "downloadclient", "RTorrent").await;
    assert_eq!(r["message"]["type"], "warning");
    assert_eq!(field(&template(&app, "downloadclient", "Sabnzbd").await, "priority")["value"], -100);

    // Prowlarr's checks: the priority (1 to 50), the host and the category
    let mut bad = q.clone();
    bad["name"] = json!("qBit");
    bad["priority"] = json!(0);
    let (s, v) = call(&app, "POST", "/api/v1/downloadclient", Some(bad.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v[0]["errorMessage"], "'Priority' must be between 1 and 50. You entered 0.");
    bad["priority"] = json!(1);
    bad["enable"] = json!(true);
    set(&mut bad, "host", json!("http://qbit"));
    set(&mut bad, "category", json!("/tv"));
    let (s, v) = call(&app, "POST", "/api/v1/downloadclient", Some(bad.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let msgs: Vec<Value> = v.as_array().unwrap().iter().map(|f| f["errorMessage"].clone()).collect();
    assert_eq!(msgs, [json!("must be valid Host without http://"), json!(r"Can not contain '\', '//', or start/end with '/'")]);

    // a client that's off is saved without a test; the hub sees it with its address
    let mut off = q.clone();
    off["name"] = json!("qBit");
    set(&mut off, "host", json!("qbit"));
    set(&mut off, "password", json!("secret"));
    let (s, v) = call(&app, "POST", "/api/v1/downloadclient", Some(off)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(field(&v, "password")["value"], "********");
    let id = v["id"].as_i64().unwrap();
    let hub = st.hub.client(id).unwrap();
    assert_eq!(
        (hub.kind.as_str(), hub.url.as_str(), hub.password.as_str(), hub.enabled),
        ("qbittorrent", "http://qbit:8080", "secret", false)
    );

    // and a client the hub adds is a Prowlarr client, its address kept exactly
    let hid = st
        .hub
        .add_client(&json!({ "kind": "transmission", "name": "Trans", "url": "https://trans.example:9091/transmission/rpc", "root": "/data/dl", "priority": 3 }))
        .unwrap();
    let (_, v) = call(&app, "GET", &format!("/api/v1/downloadclient/{hid}"), None).await;
    assert_eq!((v["implementation"].clone(), v["priority"].clone(), v["enable"].clone()), (json!("Transmission"), json!(3), json!(true)));
    assert_eq!(
        (field(&v, "host")["value"].clone(), field(&v, "port")["value"].clone(), field(&v, "useSsl")["value"].clone()),
        (json!("trans.example"), json!(9091), json!(true))
    );
    assert_eq!(field(&v, "urlBase")["value"], "/transmission");
    let c = st.hub.client(hid).unwrap();
    assert_eq!((c.url.as_str(), c.root.as_str()), ("https://trans.example:9091/transmission/rpc", "/data/dl"));
    // a change through Prowlarr's API moves the hub's address with it
    let mut changed = v.clone();
    set(&mut changed, "host", json!("other"));
    let (s, _) = call(&app, "PUT", &format!("/api/v1/downloadclient/{hid}?forceSave=true"), Some(changed)).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(st.hub.client(hid).unwrap().url, "https://other:9091/transmission/rpc");
    assert_eq!(st.hub.client(hid).unwrap().root, "/data/dl");

    // bulk: off and priority 7 for both
    let (s, v) = call(&app, "PUT", "/api/v1/downloadclient/bulk", Some(json!({ "ids": [id, hid], "enable": false, "priority": 7 }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert!(v.as_array().unwrap().iter().all(|c| c["priority"] == 7 && c["enable"] == false));
    // the config page answers only its id
    let (_, v) = call(&app, "GET", "/api/v1/config/downloadclient", None).await;
    assert_eq!(v, json!({ "id": 1 }));
    // removing one through the hub removes it from Prowlarr's list
    st.hub.remove_client(hid).unwrap();
    let (s, _) = call(&app, "GET", &format!("/api/v1/downloadclient/{hid}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, v) = call(&app, "DELETE", "/api/v1/downloadclient/bulk", Some(json!({ "ids": [id] }))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    assert!(st.hub.clients().unwrap().is_empty());
}

#[tokio::test]
async fn old_hub_clients_move_with_their_ids() {
    let (app, st) = lantern("dl-move").await;
    st.db
        .execute(
            "INSERT INTO download_clients (id, kind, name, url, username, password, api_key, root, enabled, priority, added) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            p![42, "sabnzbd", "SAB", "http://sab:8080/sabnzbd", "", "", "sab-key", "/data", true, 5, 100.0],
        )
        .unwrap();
    downloads::migrate(&st.db).unwrap();
    downloads::migrate(&st.db).unwrap();
    let c = st.hub.client(42).unwrap();
    assert_eq!(
        (c.kind.as_str(), c.url.as_str(), c.api_key.as_str(), c.priority, c.added),
        ("sabnzbd", "http://sab:8080/sabnzbd", "sab-key", 5, 100.0)
    );
    let (_, v) = call(&app, "GET", "/api/v1/downloadclient", None).await;
    assert_eq!(v.as_array().unwrap().len(), 1, "moved once: {v}");
    assert_eq!((v[0]["implementation"].clone(), field(&v[0], "urlBase")["value"].clone()), (json!("Sabnzbd"), json!("/sabnzbd")));
}

#[tokio::test]
async fn each_download_client_is_tested_and_sent_to() {
    let (base, log) = start_stand_in().await;
    let (host, port) = host_port(&base);
    let (app, st) = lantern("dl-clients").await;

    // add a client through the API (its test runs), then send it a release
    async fn add(app: &Router, implementation: &str, fields: &[(&str, Value)]) -> (StatusCode, Value) {
        let mut t = template(app, "downloadclient", implementation).await;
        t["name"] = json!(implementation);
        t["enable"] = json!(true);
        for (k, v) in fields {
            set(&mut t, k, v.clone());
        }
        call(app, "POST", "/api/v1/downloadclient", Some(t)).await
    }
    let torrent = b"d4:infod4:name4:test12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee".to_vec();
    let send = |id: i64, g: Grabbed, p: Protocol| {
        let st = st.clone();
        async move {
            let def = provider::find::<DownloadClients>(&st.db, id).unwrap().unwrap();
            downloads::send(&st, &def, &release(p), g).await
        }
    };

    // Aria2: a wrong secret fails the test; the right one adds the torrent by XML-RPC
    let (s, v) = add(&app, "Aria2", &[("host", json!(host)), ("port", json!(port)), ("secretToken", json!("nope"))]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v[0]["errorMessage"].as_str().unwrap().contains("Unauthorized"), "{v}");
    let (s, v) =
        add(&app, "Aria2", &[("host", json!(host)), ("port", json!(port)), ("secretToken", json!("secret")), ("directory", json!("/dl"))])
            .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let gid = send(v["id"].as_i64().unwrap(), Grabbed::File(torrent.clone()), Protocol::Torrent).await.unwrap();
    assert_eq!(gid.download_id, "2089b05ecca3d829");
    let calls = sent(&log, "/rpc");
    let last = &calls.last().unwrap().2;
    assert!(
        last.contains("<methodName>aria2.addTorrent</methodName>") && last.contains("<name>dir</name><value><string>/dl</string>"),
        "{last}"
    );

    // Flood: signs in, then adds the file with its tags
    let (s, v) = add(
        &app,
        "Flood",
        &[("host", json!(host)), ("port", json!(port)), ("urlBase", json!("flood")), ("username", json!("u")), ("password", json!("pw"))],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    send(v["id"].as_i64().unwrap(), Grabbed::File(torrent.clone()), Protocol::Torrent).await.unwrap();
    let add_files = sent(&log, "/flood/api/torrents/add-files");
    let body: Value = serde_json::from_str(&add_files[0].2).unwrap();
    assert_eq!((body["tags"].clone(), body["start"].clone()), (json!(["prowlarr"]), json!(true)));

    // uTorrent: the token, then the file, then the label
    let (s, v) =
        add(&app, "UTorrent", &[("host", json!(host)), ("port", json!(port)), ("username", json!("u")), ("password", json!("p"))]).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(v["message"]["message"].as_str().unwrap().contains("cryptominers"));
    send(v["id"].as_i64().unwrap(), Grabbed::File(torrent.clone()), Protocol::Torrent).await.unwrap();
    let gui = sent(&log, "/gui/?");
    assert!(gui.iter().any(|(m, p, _)| m == "POST" && p.contains("token=TOK123&action=add-file")), "{gui:?}");
    assert!(gui.iter().any(|(_, p, _)| p.contains("action=setprops") && p.contains("v=prowlarr")), "{gui:?}");

    // Hadouken: JSON-RPC with the label
    let (s, v) = add(
        &app,
        "Hadouken",
        &[("host", json!(host)), ("port", json!(port)), ("urlBase", json!("hadouken")), ("username", json!("u")), ("password", json!("p"))],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    send(v["id"].as_i64().unwrap(), Grabbed::Magnet("magnet:?xt=urn:btih:abc".into()), Protocol::Torrent).await.unwrap();
    let rpc: Value = serde_json::from_str(&sent(&log, "/hadouken/api").last().unwrap().2).unwrap();
    assert_eq!(
        (rpc["method"].clone(), rpc["params"].clone()),
        (json!("webui.addTorrent"), json!(["url", "magnet:?xt=urn:btih:abc", { "label": "prowlarr" }]))
    );

    // the blackholes and Pneumatic write only inside the data folder
    let hole = st.data_dir.join("blackhole");
    std::fs::create_dir_all(&hole).unwrap();
    let (s, v) = add(&app, "TorrentBlackhole", &[("torrentFolder", json!("/tmp"))]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v[0]["errorMessage"].as_str().unwrap().starts_with("Lantern only writes inside its data folder"), "{v}");
    let (s, v) =
        add(&app, "TorrentBlackhole", &[("torrentFolder", json!(hole.display().to_string())), ("saveMagnetFiles", json!(true))]).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let tid = v["id"].as_i64().unwrap();
    send(tid, Grabbed::File(torrent.clone()), Protocol::Torrent).await.unwrap();
    send(tid, Grabbed::Magnet("magnet:?xt=urn:btih:abc".into()), Protocol::Torrent).await.unwrap();
    assert_eq!(std::fs::read(hole.join("Show.Name.S01E02.1080p.WEB-DL-GRP.torrent")).unwrap(), torrent);
    assert_eq!(std::fs::read_to_string(hole.join("Show.Name.S01E02.1080p.WEB-DL-GRP.magnet")).unwrap(), "magnet:?xt=urn:btih:abc");
    let (s, v) =
        add(&app, "Pneumatic", &[("nzbFolder", json!(hole.display().to_string())), ("strmFolder", json!(hole.display().to_string()))])
            .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    send(v["id"].as_i64().unwrap(), Grabbed::File(b"<nzb/>".to_vec()), Protocol::Usenet).await.unwrap();
    let strm = std::fs::read_to_string(hole.join("Show.Name.S01E02.1080p.WEB-DL-GRP.strm")).unwrap();
    assert!(strm.starts_with("plugin://plugin.program.pneumatic/?mode=strm&type=add_file&nzb="), "{strm}");

    // the ones Lantern doesn't reach yet say so
    let (s, v) = add(&app, "NzbVortex", &[("apiKey", json!("k"))]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v[0]["errorMessage"], "Lantern can't send downloads to NZBVortex yet. Use another download client for now.");

    // the indexer's client must be one Prowlarr has; testall lists each enabled client
    let (s, v) = call(&app, "POST", "/api/v1/downloadclient/testall", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v.as_array().unwrap().iter().all(|r| r["isValid"] == true));
}

/// A tiny SMTP server that takes one mail and keeps it.
async fn smtp_stand_in() -> (i64, Arc<Mutex<String>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = i64::from(l.local_addr().unwrap().port());
    let got: Arc<Mutex<String>> = Default::default();
    let keep = got.clone();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = l.accept().await else { return };
            let keep = keep.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut r = BufReader::new(r);
                w.write_all(b"220 stand-in ESMTP\r\n").await.unwrap();
                let mut line = String::new();
                let mut data = false;
                while r.read_line(&mut line).await.unwrap_or(0) > 0 {
                    let l = line.clone();
                    line.clear();
                    if data {
                        if l == ".\r\n" {
                            data = false;
                            w.write_all(b"250 OK\r\n").await.unwrap();
                        } else {
                            keep.lock().unwrap().push_str(&l);
                        }
                        continue;
                    }
                    let cmd = l.to_uppercase();
                    let answer: &[u8] = if cmd.starts_with("EHLO") || cmd.starts_with("HELO") {
                        b"250 stand-in\r\n"
                    } else if cmd.starts_with("DATA") {
                        data = true;
                        b"354 go on\r\n"
                    } else if cmd.starts_with("QUIT") {
                        w.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        b"250 OK\r\n"
                    };
                    w.write_all(answer).await.unwrap();
                }
            });
        }
    });
    (port, got)
}

#[tokio::test]
async fn notifications_answer_test_and_send_as_prowlarr_does() {
    let (base, log) = start_stand_in().await;
    server::prowlarr::apps::notifications::senders::route_fixed_hosts_to(&base);
    let (host, port) = host_port(&base);
    let (app, st) = lantern("notify").await;

    // the templates: every provider; Slack and Mailgun have no grab notification; no bulk routes
    let (_, list) = call(&app, "GET", "/api/v1/notification/schema", None).await;
    assert_eq!(list.as_array().unwrap().len(), 20);
    let slack = template(&app, "notification", "Slack").await;
    assert_eq!(
        (slack["supportsOnGrab"].clone(), slack["supportsOnHealthIssue"].clone(), slack["onGrab"].clone()),
        (json!(false), json!(true), json!(false))
    );
    let hook = template(&app, "notification", "Webhook").await;
    assert_eq!(field(&hook, "method")["value"], 1);
    assert_eq!(
        field(&hook, "method")["selectOptions"],
        json!([{ "value": 1, "name": "POST", "order": 1, "hint": "(1)" }, { "value": 2, "name": "PUT", "order": 2, "hint": "(2)" }])
    );
    let (s, _) = call(&app, "PUT", "/api/v1/notification/bulk", Some(json!({ "ids": [1] }))).await;
    assert_ne!(s, StatusCode::ACCEPTED);

    // test one provider: `{}` when it worked, Prowlarr's failure when it didn't
    async fn test(app: &Router, implementation: &str, fields: &[(&str, Value)]) -> (StatusCode, Value) {
        let mut t = template(app, "notification", implementation).await;
        t["name"] = json!(implementation);
        t["onHealthIssue"] = json!(true);
        for (k, v) in fields {
            set(&mut t, k, v.clone());
        }
        call(app, "POST", "/api/v1/notification/test", Some(t)).await
    }
    let ok = (StatusCode::OK, json!({}));
    let at = |path: &str| sent(&log, path).last().cloned().unwrap_or_else(|| panic!("nothing sent to {path}"));

    // services at an address of one's own
    assert_eq!(test(&app, "Webhook", &[("url", json!(format!("{base}/hook"))), ("method", json!(2))]).await, ok);
    let (m, _, b) = at("/hook");
    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!((m.as_str(), v["eventType"].clone(), v["instanceName"].clone()), ("PUT", json!("Test"), json!("Lantern")));
    assert_eq!(test(&app, "Discord", &[("webHookUrl", json!(format!("{base}/discord"))), ("username", json!("bot"))]).await, ok);
    let v: Value = serde_json::from_str(&at("/discord").2).unwrap();
    assert!(v["content"].as_str().unwrap().starts_with("Test message from Lantern posted at "), "{v}");
    assert_eq!(v["username"], "bot");
    assert_eq!(test(&app, "Slack", &[("webHookUrl", json!(format!("{base}/slack"))), ("icon", json!(":ghost:"))]).await, ok);
    assert_eq!(serde_json::from_str::<Value>(&at("/slack").2).unwrap()["icon_emoji"], ":ghost:");
    assert_eq!(test(&app, "Gotify", &[("server", json!(format!("{base}/gotify"))), ("appToken", json!("app"))]).await, ok);
    assert!(at("/gotify/message").2.contains("title=Test+Notification"));
    let (s, v) = test(&app, "Gotify", &[("server", json!(format!("{base}/gotify"))), ("appToken", json!("bad"))]).await;
    assert_eq!((s, v[0]["errorMessage"].clone()), (StatusCode::BAD_REQUEST, json!("Unauthorized - AuthToken is invalid")));
    assert_eq!(test(&app, "Apprise", &[("serverUrl", json!(format!("{base}/apprise"))), ("configurationKey", json!("key1"))]).await, ok);
    let v: Value = serde_json::from_str(&at("/apprise/notify/key1").2).unwrap();
    assert_eq!((v["title"].clone(), v["type"].clone()), (json!("Lantern - Test Notification"), json!("info")));
    assert_eq!(test(&app, "Ntfy", &[("serverUrl", json!(format!("{base}/ntfy"))), ("topics", json!(["alerts"]))]).await, ok);
    assert!(at("/ntfy/alerts").1.contains("title=Lantern"));
    assert_eq!(
        test(&app, "Signal", &[("host", json!(host)), ("port", json!(port)), ("senderNumber", json!("+1")), ("receiverId", json!("+2"))])
            .await,
        ok
    );
    assert_eq!(serde_json::from_str::<Value>(&at("/v2/send").2).unwrap()["recipients"], json!(["+2"]));

    // services at a fixed address, reached at the stand-in
    assert_eq!(test(&app, "Telegram", &[("botToken", json!("T1")), ("chatId", json!("42"))]).await, ok);
    assert!(at("/botT1/sendmessage").2.contains("chat_id=42"));
    let (s, v) = test(&app, "Telegram", &[("botToken", json!("BAD")), ("chatId", json!("42"))]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!((v[0]["propertyName"].clone(), v[0]["errorMessage"].clone()), (json!("ChatId"), json!("Bad Request: chat not found")));
    assert_eq!(test(&app, "Pushover", &[("apiKey", json!("a")), ("userKey", json!("u"))]).await, ok);
    assert!(at("/1/messages.json").2.contains("user=u"));
    assert_eq!(test(&app, "Prowl", &[("apiKey", json!("a"))]).await, ok);
    assert!(at("/publicapi/add").2.contains("application=Lantern"));
    assert_eq!(test(&app, "Simplepush", &[("key", json!("k"))]).await, ok);
    assert_eq!(test(&app, "PushBullet", &[("apiKey", json!("a")), ("deviceIds", json!(["dev1"]))]).await, ok);
    assert!(at("/v2/pushes").2.contains("device_iden=dev1"));
    assert_eq!(test(&app, "Pushcut", &[("apiKey", json!("a")), ("notificationName", json!("n1"))]).await, ok);
    assert_eq!(test(&app, "Join", &[("apiKey", json!("a")), ("deviceNames", json!("phone"))]).await, ok);
    assert!(at("/_ah/api/messaging/v1/sendPush").1.contains("deviceNames=phone"));
    assert_eq!(
        test(
            &app,
            "Mailgun",
            &[("apiKey", json!("a")), ("from", json!("a@b.c")), ("senderDomain", json!("mg.example")), ("recipients", json!(["x@y.z"]))]
        )
        .await,
        ok
    );
    assert!(at("/v3/mg.example/messages").2.contains("to=x%40y.z"));
    assert_eq!(test(&app, "SendGrid", &[("apiKey", json!("a")), ("from", json!("a@b.c")), ("recipients", json!(["x@y.z"]))]).await, ok);
    assert_eq!(serde_json::from_str::<Value>(&at("/v3/mail/send").2).unwrap()["personalizations"][0]["to"], json!([{ "email": "x@y.z" }]));
    assert_eq!(test(&app, "Notifiarr", &[("aPIKey", json!("a"))]).await, ok);
    assert_eq!(serde_json::from_str::<Value>(&at("/api/v1/notification/prowlarr").2).unwrap()["eventType"], "Test");

    // email, through a stand-in SMTP server
    let (smtp_port, mail) = smtp_stand_in().await;
    let fields = [
        ("server", json!("127.0.0.1")),
        ("port", json!(smtp_port)),
        ("useEncryption", json!(2)),
        ("from", json!("lantern@example.com")),
        ("to", json!(["me@example.com"])),
    ];
    assert_eq!(test(&app, "Email", &fields).await, ok);
    assert!(mail.lock().unwrap().contains("Subject: Lantern - Test Notification"), "{}", mail.lock().unwrap());

    // a script of one's own: run with no arguments, the event in its environment
    let dir = st.data_dir.join("scripts");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.txt");
    let script = dir.join("hook.sh");
    std::fs::write(&script, format!("#!/bin/sh\necho \"$Prowlarr_EventType $#\" > {}\n", out.display())).unwrap();
    let failing = dir.join("fail.sh");
    std::fs::write(&failing, "#!/bin/sh\nexit 3\n").unwrap();
    for f in [&script, &failing] {
        let mut perm = std::fs::metadata(f).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(f, perm).unwrap();
    }
    assert_eq!(test(&app, "CustomScript", &[("path", json!(script.display().to_string()))]).await, ok);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "Test 0\n");
    let (s, v) = test(&app, "CustomScript", &[("path", json!(failing.display().to_string()))]).await;
    assert_eq!((s, v[0]["errorMessage"].clone()), (StatusCode::BAD_REQUEST, json!("Script exited with code: 3")));
    let (s, v) = test(&app, "CustomScript", &[("path", json!("/usr/bin/env")), ("arguments", json!("-i"))]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let msgs: Vec<Value> = v.as_array().unwrap().iter().map(|f| f["errorMessage"].clone()).collect();
    assert_eq!(msgs, [json!("Must not be a descendant of '/usr/bin'"), json!("Arguments are no longer supported for custom scripts")]);

    // a saved webhook on for grabs hears of one, with Prowlarr's message
    let mut hook = template(&app, "notification", "Webhook").await;
    hook["name"] = json!("Hook");
    hook["onGrab"] = json!(true);
    set(&mut hook, "url", json!(format!("{base}/grabs")));
    let (s, v) = call(&app, "POST", "/api/v1/notification", Some(hook)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(
        (v["onGrab"].clone(), v["supportsOnGrab"].clone(), v["includeManualGrabs"].clone()),
        (json!(true), json!(true), json!(false))
    );
    use server::prowlarr::apps::notifications::{grabbed, GrabMessage};
    let r = release(Protocol::Torrent);
    let msg = |manual: bool| GrabMessage {
        release: r.clone(),
        source: "Sonarr".into(),
        host: "10.0.0.2".into(),
        successful: true,
        download_client_name: "qBit".into(),
        download_client_type: "qBittorrent".into(),
        download_id: "abc".into(),
        redirect: false,
        manual,
        message: GrabMessage::message_for(&r, manual, "Sonarr", "qBit"),
    };
    // saving it sent the test
    assert_eq!(sent(&log, "/grabs").len(), 1);
    grabbed(&st, &msg(true), &[]).await;
    assert_eq!(sent(&log, "/grabs").len(), 1, "manual grabs are left out unless included");
    grabbed(&st, &msg(false), &[]).await;
    let v: Value = serde_json::from_str(&at("/grabs").2).unwrap();
    assert_eq!((v["eventType"].clone(), v["trigger"].clone(), v["downloadClient"].clone()), (json!("Grab"), json!("Api"), json!("qBit")));
    assert_eq!(v["release"]["releaseTitle"], r.title);
    assert_eq!(v["release"]["categories"], json!(["TV/HD"]));
}
