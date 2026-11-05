//! The download hub end to end: a fake qBittorrent, a fake SABnzbd and a fake app (its webhook) on one local
//! port; Lantern registers the app, sends grabs to the clients, follows them to the end, retries failures and
//! calls the app back with the files' paths in the app's own view.

use axum::body::Body;
use axum::extract::{Multipart, Query, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// What the fake clients and the fake app hold.
#[derive(Default)]
struct Fake {
    /// qBittorrent's torrents, as `torrents/info` lists them.
    torrents: Vec<Value>,
    /// qBittorrent's categories: name → folder.
    qbit_categories: HashMap<String, String>,
    /// SABnzbd's queue slots.
    queue: Vec<Value>,
    /// SABnzbd's history slots.
    history: Vec<Value>,
    /// SABnzbd's categories: name → folder.
    sab_categories: HashMap<String, String>,
    /// nzo ids SABnzbd was told to delete.
    sab_deleted: Vec<String>,
    /// How many NZBs SABnzbd got.
    nzbs: usize,
    /// qBittorrent's deletions: (hashes, deleteFiles).
    qbit_deleted: Vec<(String, String)>,
    /// The webhook calls: (event header, signature header, body as sent).
    hooks: Vec<(String, String, String)>,
}

type Shared = Arc<Mutex<Fake>>;

/// Whether the request carries qBittorrent's session cookie.
fn qbit_session(h: &HeaderMap) -> bool {
    h.get(header::COOKIE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("SID=s1"))
}

async fn qbit_login(Form(f): Form<HashMap<String, String>>) -> Response {
    if f.get("username").map(String::as_str) == Some("admin") && f.get("password").map(String::as_str) == Some("adminpw") {
        ([(header::SET_COOKIE, "SID=s1; path=/")], "Ok.").into_response()
    } else {
        "Fails.".into_response()
    }
}

async fn qbit_version(h: HeaderMap) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    "v5.0.0".into_response()
}

async fn qbit_create(State(s): State<Shared>, h: HeaderMap, Form(f): Form<HashMap<String, String>>) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut s = s.lock().unwrap();
    if s.qbit_categories.contains_key(&f["category"]) {
        return StatusCode::CONFLICT.into_response();
    }
    s.qbit_categories.insert(f["category"].clone(), f["savePath"].clone());
    "".into_response()
}

async fn qbit_edit(State(s): State<Shared>, h: HeaderMap, Form(f): Form<HashMap<String, String>>) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    s.lock().unwrap().qbit_categories.insert(f["category"].clone(), f["savePath"].clone());
    "".into_response()
}

async fn qbit_add(State(s): State<Shared>, h: HeaderMap, mut m: Multipart) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut f: HashMap<String, String> = HashMap::new();
    while let Ok(Some(field)) = m.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        f.insert(name, field.text().await.unwrap_or_default());
    }
    let mut s = s.lock().unwrap();
    let n = s.torrents.len() + 1;
    let name = "Show.S01E01";
    let save = f.get("savepath").cloned().unwrap_or_default();
    s.torrents.push(json!({
        "hash": format!("hash{n}"), "name": name, "state": "downloading", "progress": 0.3, "size": 100, "dlspeed": 50, "eta": 10,
        "tags": f.get("tags").cloned().unwrap_or_default(), "category": f.get("category").cloned().unwrap_or_default(),
        "save_path": save, "content_path": format!("{save}/{name}"), "urls": f.get("urls").cloned().unwrap_or_default(),
    }));
    "Ok.".into_response()
}

async fn qbit_info(State(s): State<Shared>, h: HeaderMap) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(Value::Array(s.lock().unwrap().torrents.clone())).into_response()
}

async fn qbit_files(h: HeaderMap) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(json!([{ "name": "Show.S01E01/Show.S01E01.mkv", "size": 90 }, { "name": "Show.S01E01/Show.S01E01.nfo", "size": 10 }]))
        .into_response()
}

async fn qbit_delete(State(s): State<Shared>, h: HeaderMap, Form(f): Form<HashMap<String, String>>) -> Response {
    if !qbit_session(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut s = s.lock().unwrap();
    s.torrents.retain(|t| t["hash"] != f["hashes"]);
    s.qbit_deleted.push((f["hashes"].clone(), f["deleteFiles"].clone()));
    "".into_response()
}

async fn sab_api(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>, m: Option<Multipart>) -> Response {
    if q.get("apikey").map(String::as_str) != Some("sabkey") {
        return Json(json!({ "status": false, "error": "API Key Incorrect" })).into_response();
    }
    let mut s = s.lock().unwrap_or_else(|e| e.into_inner());
    match q.get("mode").map(String::as_str).unwrap_or("") {
        "version" => Json(json!({ "version": "4.3.3" })).into_response(),
        "queue" if q.get("name").map(String::as_str) == Some("delete") => {
            s.sab_deleted.push(q["value"].clone());
            s.queue.retain(|x| x["nzo_id"] != q["value"]);
            Json(json!({ "status": true })).into_response()
        }
        "history" if q.get("name").map(String::as_str) == Some("delete") => {
            s.history.retain(|x| x["nzo_id"] != q["value"]);
            Json(json!({ "status": true })).into_response()
        }
        "queue" => Json(json!({ "queue": { "slots": s.queue } })).into_response(),
        "history" => Json(json!({ "history": { "slots": s.history } })).into_response(),
        "set_config" => {
            s.sab_categories.insert(q["keyword"].clone(), q["dir"].clone());
            Json(json!({ "status": true })).into_response()
        }
        "addfile" => {
            drop(m);
            s.nzbs += 1;
            let id = format!("SABnzbd_nzo_{}", s.nzbs);
            s.queue.push(json!({ "nzo_id": id, "filename": q["nzbname"], "status": "Downloading", "percentage": "40", "mb": "10", "timeleft": "0:00:30", "cat": q["cat"] }));
            Json(json!({ "status": true, "nzo_ids": [id] })).into_response()
        }
        _ => Json(json!({ "status": false, "error": "not handled" })).into_response(),
    }
}

async fn nzb() -> Response {
    (
        [(header::CONTENT_TYPE, "application/x-nzb")],
        "<?xml version=\"1.0\"?><nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"><file subject=\"x\"></file></nzb>",
    )
        .into_response()
}

async fn hook(State(s): State<Shared>, h: HeaderMap, body: String) -> StatusCode {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    s.lock().unwrap().hooks.push((get("x-lantern-event"), get("x-lantern-signature"), body));
    StatusCode::OK
}

/// The fake clients and app, listening on a local port: (their address, what they hold).
async fn fakes() -> (String, Shared) {
    let s: Shared = Default::default();
    let app = Router::new()
        .route("/api/v2/auth/login", post(qbit_login))
        .route("/api/v2/app/version", get(qbit_version))
        .route("/api/v2/torrents/createCategory", post(qbit_create))
        .route("/api/v2/torrents/editCategory", post(qbit_edit))
        .route("/api/v2/torrents/add", post(qbit_add))
        .route("/api/v2/torrents/info", get(qbit_info))
        .route("/api/v2/torrents/files", get(qbit_files))
        .route("/api/v2/torrents/delete", post(qbit_delete))
        .route("/sab/api", get(sab_api).post(sab_api))
        .route("/files/book.nzb", get(nzb))
        .route("/hook", post(hook))
        .with_state(s.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), s)
}

/// A fresh Lantern on a data folder of its own, with its downloads folder in it; `env` adds settings.
async fn server(name: &str, env: &[(&str, &str)]) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-hub-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let downloads = dir.join("downloads").to_string_lossy().to_string();
    let mut vars: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    vars.insert("APP_API_KEY".into(), "test-key".into());
    vars.insert("APP_DOWNLOADS_DIR".into(), downloads);
    let st = server::AppState::open(&dir.join("data"), web, move |n| vars.get(n).cloned()).await.unwrap();
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

/// Grab `id` as stored.
fn grab(st: &server::AppState, id: i64) -> server::hub::grabs::Grab {
    st.hub.get_grab(id).unwrap()
}

#[tokio::test]
async fn a_torrent_goes_to_qbittorrent_and_the_app_hears_where_its_files_are() {
    let (base, fake) = fakes().await;
    let (app, st, dir) = server("torrent", &[]).await;
    let root = dir.join("downloads").to_string_lossy().to_string();

    // qBittorrent sees the downloads folder at /data/downloads
    let (s, c) = call(
        &app,
        "POST",
        "/api/hub/clients",
        Some(json!({ "kind": "qbittorrent", "url": base, "username": "admin", "password": "adminpw", "root": "/data/downloads/" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(
        (c["name"].as_str(), c["password"].as_str(), c["root"].as_str()),
        (Some("qBittorrent"), Some("••••"), Some("/data/downloads"))
    );
    let (_, t) = call(&app, "POST", &format!("/api/hub/clients/{}/test", c["id"]), None).await;
    assert_eq!(t, json!({ "ok": true, "message": "Connected to qBittorrent v5.0.0" }));
    let (_, wrong) = call(&app, "PUT", &format!("/api/hub/clients/{}", c["id"]), Some(json!({ "password": "nope" }))).await;
    assert_eq!(wrong["password"], "••••");
    let (_, t) = call(&app, "POST", &format!("/api/hub/clients/{}/test", c["id"]), None).await;
    assert_eq!(t["ok"], false);
    assert!(t["message"].as_str().unwrap().contains("refused the user name or password"), "{t}");
    call(&app, "PUT", &format!("/api/hub/clients/{}", c["id"]), Some(json!({ "password": "adminpw" }))).await;

    // the app registers: its category and folder are made
    let reg = json!({ "name": "Sonarr 4K", "media": "tv", "import_path": "/tv-downloads", "callback_url": format!("{base}/hook") });
    let (s, a) = call(&app, "POST", "/api/hub/apps", Some(reg.clone())).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!((a["category"].as_str(), a["import_path"].as_str()), (Some("sonarr-4k"), Some("/tv-downloads")));
    assert_eq!(a["folder"].as_str().unwrap(), format!("{root}/sonarr-4k"));
    assert!(dir.join("downloads/sonarr-4k").is_dir());
    let (_, again) = call(&app, "POST", "/api/hub/apps", Some(reg)).await;
    assert_eq!(again["id"], a["id"], "registering again updates the same app");

    let mut events = st.hub.events.subscribe();
    let (s, g) =
        call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Sonarr 4K", "url": "magnet:?xt=urn:btih:abc&dn=Show.S01E01" }))).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!((g["title"].as_str(), g["protocol"].as_str()), (Some("Show.S01E01"), Some("torrent")));
    let id = g["id"].as_i64().unwrap();

    st.hub.tick().await;
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.qbit_categories.get("sonarr-4k").map(String::as_str), Some("/data/downloads/sonarr-4k"));
        let t = &f.torrents[0];
        assert_eq!(t["category"], "sonarr-4k");
        assert_eq!(t["save_path"], "/data/downloads/sonarr-4k");
        assert_eq!(t["tags"], format!("lantern,lantern-{id}"));
        assert!(t["urls"].as_str().unwrap().starts_with("magnet:"));
    }
    // the same step already asked qBittorrent where it is
    let g = grab(&st, id);
    assert_eq!((g.state.as_str(), g.progress, g.client_ref.as_str()), ("downloading", 0.3, "hash1"));
    st.hub.tick().await;

    {
        let mut f = fake.lock().unwrap();
        f.torrents[0]["state"] = json!("stalledUP");
        f.torrents[0]["progress"] = json!(1.0);
    }
    st.hub.tick().await;
    let (_, g) = call(&app, "GET", &format!("/api/hub/grabs/{id}"), None).await;
    assert_eq!(g["state"], "seeding");
    assert_eq!(g["path"], "/tv-downloads/Show.S01E01");
    assert_eq!(
        g["files"],
        json!([{ "path": "/tv-downloads/Show.S01E01/Show.S01E01.mkv", "size": 90 }, { "path": "/tv-downloads/Show.S01E01/Show.S01E01.nfo", "size": 10 }])
    );
    assert_eq!(grab(&st, id).path, format!("{root}/sonarr-4k/Show.S01E01"), "Lantern keeps its own view");

    // the live stream had every step
    let mut seen = Vec::new();
    while let Ok(e) = events.try_recv() {
        seen.push(e.event);
    }
    assert_eq!(seen, ["grab.queued", "grab.sent", "grab.progress", "grab.completed"]);

    // and the webhook got the same, signed with the API key
    let mut done = None;
    for _ in 0..50 {
        done = fake.lock().unwrap().hooks.iter().find(|h| h.0 == "grab.completed").cloned();
        if done.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (_, sig, raw) = done.expect("the app was called back");
    assert_eq!(sig, server::hub::events::signature("test-key", raw.as_bytes()));
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body["grab"]["files"][0]["path"], "/tv-downloads/Show.S01E01/Show.S01E01.mkv");

    let (_, list) = call(&app, "GET", "/api/hub/apps", None).await;
    assert_eq!(list["apps"][0]["grabs"]["seeding"], 1, "{list}");
}

#[tokio::test]
async fn usenet_grabs_wait_for_a_client_retry_failures_and_fail_after_the_last_try() {
    let (base, fake) = fakes().await;
    let (app, st, dir) = server("usenet", &[("APP_GRAB_ATTEMPTS", "3")]).await;
    let (_, a) = call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Readarr", "media": "book" }))).await;
    assert_eq!(a["category"], "readarr");

    let (_, g) =
        call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": a["id"], "url": format!("{base}/files/book.nzb"), "title": "A Book" })))
            .await;
    let id = g["id"].as_i64().unwrap();
    assert_eq!(g["protocol"], "usenet");
    st.hub.tick().await;
    let g = grab(&st, id);
    assert_eq!((g.state.as_str(), g.attempts), ("queued", 1));
    assert_eq!(g.error, "There is no download client for Usenet yet. Add one under Download clients.");
    assert!(g.next_try > server::auth::now() + 20.0, "it waits before trying again");

    let (s, c) =
        call(&app, "POST", "/api/hub/clients", Some(json!({ "kind": "sabnzbd", "url": format!("{base}/sab"), "api_key": "sabkey" }))).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["protocol"], "usenet");
    let (_, t) = call(&app, "POST", &format!("/api/hub/clients/{}/test", c["id"]), None).await;
    assert_eq!(t, json!({ "ok": true, "message": "Connected to SABnzbd 4.3.3" }));
    st.db_now_due(id);
    st.hub.tick().await;
    let g = grab(&st, id);
    assert_eq!((g.state.as_str(), g.client_ref.as_str(), g.error.as_str(), g.progress), ("downloading", "SABnzbd_nzo_1", "", 0.4));
    let folder = dir.join("downloads/readarr");
    assert_eq!(fake.lock().unwrap().sab_categories.get("readarr").map(String::as_str), Some(folder.to_string_lossy().as_ref()));

    // SABnzbd finishes: the files are read from the folder it reports
    let book = folder.join("A Book");
    std::fs::create_dir_all(&book).unwrap();
    std::fs::write(book.join("book.epub"), b"12345").unwrap();
    {
        let mut f = fake.lock().unwrap();
        f.queue.clear();
        f.history.push(
            json!({ "nzo_id": "SABnzbd_nzo_1", "name": "A Book", "status": "Completed", "storage": book.to_string_lossy(), "bytes": 5 }),
        );
    }
    st.hub.tick().await;
    let g = grab(&st, id);
    assert_eq!(g.state, "completed");
    assert_eq!(g.files, vec![(book.join("book.epub").to_string_lossy().to_string(), 5)]);

    // a download that fails in SABnzbd is removed there and sent again, then fails for good
    let (_, g2) = call(
        &app,
        "POST",
        "/api/hub/grabs",
        Some(json!({ "app": "Readarr", "url": format!("{base}/files/book.nzb"), "title": "Another" })),
    )
    .await;
    let id2 = g2["id"].as_i64().unwrap();
    let mut events = st.hub.events.subscribe();
    for round in 0..3 {
        st.db_now_due(id2);
        st.hub.tick().await;
        let nzo = grab(&st, id2).client_ref;
        assert!(nzo.starts_with("SABnzbd_nzo_"), "round {round}: {:?}", grab(&st, id2));
        {
            let mut f = fake.lock().unwrap();
            f.queue.clear();
            f.history.push(json!({ "nzo_id": nzo, "name": "Another", "status": "Failed", "fail_message": "Out of retention" }));
        }
        st.hub.tick().await;
    }
    let g2 = grab(&st, id2);
    assert_eq!((g2.state.as_str(), g2.attempts), ("failed", 3));
    assert_eq!(g2.error, "SABnzbd couldn't finish the download: Out of retention");
    assert_eq!(fake.lock().unwrap().sab_deleted.len(), 3);
    let mut seen = Vec::new();
    while let Ok(e) = events.try_recv() {
        seen.push(e.event);
    }
    assert_eq!(seen.iter().filter(|e| *e == "grab.retrying").count(), 2, "{seen:?}");
    assert_eq!(seen.last().map(String::as_str), Some("grab.failed"));

    // tried again by hand, from the start
    let (s, r) = call(&app, "POST", &format!("/api/hub/grabs/{id2}/retry"), None).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["attempts"], 0);
    let (s, r) = call(&app, "POST", &format!("/api/hub/grabs/{id}/retry"), None).await;
    assert_eq!((s, r["detail"].as_str()), (StatusCode::CONFLICT, Some("Only a failed grab can be tried again")));
}

#[tokio::test]
async fn grabs_take_lanterns_own_download_links_and_refuse_strange_ones() {
    let (app, st, _) = server("links", &[]).await;
    let (l, sig) = st.signer.sign(3, "https://tracker.test/dl/9?passkey=x");
    let from_feed = json!({ "url": format!("http://lantern:9696/3/download?link={l}&sig={sig}&file=X") });
    assert_eq!(st.hub.source_of(&from_feed).unwrap(), server::hub::grabs::Source::Indexer(3, "https://tracker.test/dl/9?passkey=x".into()));
    let token = server::hub::grabs::token(&st.signer, 3, "https://tracker.test/dl/9?passkey=x");
    assert_eq!(
        st.hub.source_of(&json!({ "token": token })).unwrap(),
        server::hub::grabs::Source::Indexer(3, "https://tracker.test/dl/9?passkey=x".into())
    );
    assert!(st.hub.source_of(&json!({ "token": "3.bad.sig" })).is_err());

    call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Lidarr", "media": "music" }))).await;
    let (s, v) = call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Lidarr", "url": "file:///etc/passwd" }))).await;
    assert_eq!(
        (s, v["detail"].as_str()),
        (StatusCode::BAD_REQUEST, Some("The address to download must start with http://, https:// or magnet:"))
    );
    let (s, v) = call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Nobody", "url": "magnet:?xt=1" }))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::NOT_FOUND, Some("There is no app named Nobody. Register it first.")));
    let (s, v) = call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "X", "media": "games" }))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("The media type must be tv, movie, music, book or other")));
    let (s, v) = call(&app, "POST", "/api/hub/clients", Some(json!({ "kind": "utorrent", "url": "http://x:1" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let r = app.clone().oneshot(Request::get("/api/hub/events").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = app.clone().oneshot(Request::get("/api/hub/events?apikey=test-key").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get(header::CONTENT_TYPE).unwrap(), "text/event-stream");
}

#[tokio::test]
async fn an_imported_torrent_is_cleaned_up_once_it_has_seeded_enough() {
    let (base, fake) = fakes().await;
    let (app, st, _) = server("cleanup", &[("APP_SEED_RATIO", "1.0"), ("APP_CLEANUP", "delete")]).await;
    server::hub::migrate(&st.db).unwrap();
    call(&app, "POST", "/api/hub/clients", Some(json!({ "kind": "qbittorrent", "url": base, "username": "admin", "password": "adminpw" })))
        .await;
    call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Radarr", "media": "movie" }))).await;
    let (_, g) = call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Radarr", "url": "magnet:?xt=urn:btih:abc&dn=Film" }))).await;
    let id = g["id"].as_i64().unwrap();
    let (s, v) = call(&app, "POST", &format!("/api/hub/grabs/{id}/imported"), None).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::CONFLICT, Some("Only a finished grab can be marked as imported")));
    st.hub.tick().await;
    {
        let mut f = fake.lock().unwrap();
        f.torrents[0]["state"] = json!("uploading");
        f.torrents[0]["progress"] = json!(1.0);
        f.torrents[0]["ratio"] = json!(0.5);
        f.torrents[0]["seeding_time"] = json!(60);
    }
    st.hub.tick().await;
    let (_, g) = call(&app, "GET", &format!("/api/hub/grabs/{id}"), None).await;
    assert_eq!((g["state"].as_str(), g["ratio"].as_f64(), g["seeded"].as_bool()), (Some("seeding"), Some(0.5), Some(false)));

    let (s, g) = call(&app, "POST", &format!("/api/hub/grabs/{id}/imported"), None).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert!(g["imported"].as_f64().unwrap() > 0.0);
    st.hub.tick().await;
    assert_eq!(grab(&st, id).state, "seeding", "imported but not seeded enough: it keeps seeding");
    assert!(fake.lock().unwrap().qbit_deleted.is_empty());

    let mut events = st.hub.events.subscribe();
    fake.lock().unwrap().torrents[0]["ratio"] = json!(1.2);
    st.hub.tick().await;
    assert_eq!(grab(&st, id).state, "removed");
    assert_eq!(fake.lock().unwrap().qbit_deleted, [("hash1".to_string(), "true".to_string())]);
    let mut removed = None;
    while let Ok(e) = events.try_recv() {
        if e.event == "grab.removed" {
            removed = Some(e.grab);
        }
    }
    assert_eq!(removed.unwrap()["reason"], "cleaned up: removed with its files");
}

#[tokio::test]
async fn nothing_is_cleaned_up_while_cleanup_is_off() {
    let (base, fake) = fakes().await;
    let (app, st, _) = server("cleanup-off", &[]).await;
    call(&app, "POST", "/api/hub/clients", Some(json!({ "kind": "qbittorrent", "url": base, "username": "admin", "password": "adminpw" })))
        .await;
    call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Radarr" }))).await;
    let (_, g) = call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Radarr", "url": "magnet:?xt=urn:btih:abc&dn=Film" }))).await;
    let id = g["id"].as_i64().unwrap();
    st.hub.tick().await;
    {
        let mut f = fake.lock().unwrap();
        f.torrents[0]["state"] = json!("pausedUP");
        f.torrents[0]["progress"] = json!(1.0);
    }
    st.hub.tick().await;
    let (_, g) = call(&app, "POST", &format!("/api/hub/grabs/{id}/imported"), None).await;
    assert_eq!(
        (g["state"].as_str(), g["seeded"].as_bool()),
        (Some("completed"), Some(true)),
        "no goal: stopped by its client counts as seeded"
    );
    st.hub.tick().await;
    assert_eq!(grab(&st, id).state, "completed");
    assert!(fake.lock().unwrap().qbit_deleted.is_empty());
}

/// Test helpers on the state.
trait Due {
    /// Makes grab `id`'s next try due now.
    fn db_now_due(&self, id: i64);
}

impl Due for server::AppState {
    fn db_now_due(&self, id: i64) {
        use kit_server_core::Db;
        self.db.execute("UPDATE hub_grabs SET next_try=0 WHERE id=?", vec![json!(id)]).unwrap();
    }
}
