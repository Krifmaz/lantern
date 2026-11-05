//! Soulseek through the hub: a fake slskd on a local port answers searches and downloads, and writes the files
//! where slskd would. A music app searches, grabs a folder by its token, and gets the files in its own folder.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// What the fake slskd holds.
#[derive(Default)]
struct Fake {
    /// The words of each search.
    searched: Vec<String>,
    /// The downloads asked for: (user, file name, size, state, bytes in).
    downloads: Vec<(String, String, i64, String, i64)>,
    /// Ids of transfers removed.
    removed: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

/// Whether the request has the right API key.
fn key_ok(h: &HeaderMap) -> bool {
    h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some("slsk-key")
}

async fn application(h: HeaderMap) -> Response {
    if !key_ok(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "version": { "full": "0.22.2" } })).into_response()
}

async fn start_search(State(s): State<Shared>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if !key_ok(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    s.lock().unwrap().searched.push(b["searchText"].as_str().unwrap_or("").to_string());
    Json(json!({ "id": "s1", "isComplete": false })).into_response()
}

async fn responses() -> Json<Value> {
    Json(json!([
        { "username": "ann", "hasFreeUploadSlot": true, "uploadSpeed": 1000, "queueLength": 0, "files": [
            { "filename": "@@music\\Artist\\Album\\01 - One.flac", "size": 4 },
            { "filename": "@@music\\Artist\\Album\\02 - Two.flac", "size": 4 },
        ]},
    ]))
}

async fn enqueue(State(s): State<Shared>, Path(user): Path<String>, Json(b): Json<Value>) -> StatusCode {
    let mut s = s.lock().unwrap();
    for f in b.as_array().unwrap() {
        s.downloads.push((user.clone(), f["filename"].as_str().unwrap().into(), f["size"].as_i64().unwrap(), "Queued, Remotely".into(), 0));
    }
    StatusCode::CREATED
}

async fn transfers(State(s): State<Shared>, Path(user): Path<String>) -> Json<Value> {
    let s = s.lock().unwrap();
    let files: Vec<Value> = s
        .downloads
        .iter()
        .enumerate()
        .filter(|(_, d)| d.0 == user)
        .map(|(i, d)| json!({ "id": format!("t{i}"), "filename": d.1, "size": d.2, "state": d.3, "bytesTransferred": d.4, "averageSpeed": 2.0 }))
        .collect();
    Json(json!({ "username": user, "directories": [{ "directory": "@@music\\Artist\\Album", "files": files }] }))
}

async fn remove(State(s): State<Shared>, Path((_, id)): Path<(String, String)>) -> StatusCode {
    s.lock().unwrap().removed.push(id);
    StatusCode::NO_CONTENT
}

/// The fake slskd, listening on a local port: (its address, what it holds).
async fn fake() -> (String, Shared) {
    let s: Shared = Default::default();
    let app = Router::new()
        .route("/api/v0/application", get(application))
        .route("/api/v0/searches", post(start_search))
        .route(
            "/api/v0/searches/{id}",
            get(|| async { Json(json!({ "id": "s1", "isComplete": true })) }).delete(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/api/v0/searches/{id}/responses", get(responses))
        .route("/api/v0/transfers/downloads/{user}", post(enqueue).get(transfers))
        .route("/api/v0/transfers/downloads/{user}/{id}", delete(remove))
        .with_state(s.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), s)
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

#[tokio::test]
async fn a_music_app_gets_a_soulseek_folder_into_its_own_folder() {
    let (base, fake) = fake().await;
    let dir = std::env::temp_dir().join(format!("lantern-soulseek-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let downloads = dir.join("downloads").to_string_lossy().to_string();
    let slsk_dir = dir.join("downloads").join("soulseek");
    let vars: HashMap<&str, String> = HashMap::from([
        ("APP_API_KEY", "test-key".to_string()),
        ("APP_DOWNLOADS_DIR", downloads.clone()),
        ("APP_SOULSEEK_URL", base.clone()),
        ("APP_SOULSEEK_API_KEY", "slsk-key".to_string()),
        ("APP_SOULSEEK_SEARCH_SECS", "3".to_string()),
    ]);
    let st = server::AppState::open(&dir.join("data"), web, move |n| vars.get(n).cloned()).await.unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    let app = server::app(st.clone());

    let (_, t) = call(&app, "POST", "/api/hub/soulseek/test", None).await;
    assert_eq!(t, json!({ "ok": true, "message": "Connected to slskd 0.22.2" }));

    let (_, a) = call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Lidarr", "media": "music" }))).await;
    let folder = a["folder"].as_str().unwrap().to_string();

    // a music app's search also searches Soulseek; a TV app's doesn't
    let (s, found) = call(&app, "GET", "/api/hub/search?app=Lidarr&q=Artist%20Album", None).await;
    assert_eq!(s, StatusCode::OK, "{found}");
    let r = found["results"].as_array().unwrap().iter().find(|r| r["protocol"] == "soulseek").cloned().expect("a Soulseek result");
    assert_eq!(
        (r["title"].as_str(), r["size"].as_i64(), r["files"].as_i64(), r["user"].as_str()),
        (Some("Artist - Album"), Some(8), Some(2), Some("ann"))
    );
    assert_eq!(fake.lock().unwrap().searched, ["Artist Album"]);
    call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Sonarr", "media": "tv" }))).await;
    let (_, tv) = call(&app, "GET", "/api/hub/search?app=Sonarr&q=Artist", None).await;
    assert!(tv["results"].as_array().unwrap().iter().all(|r| r["protocol"] != "soulseek"));

    // the grab asks slskd for both files
    let (s, g) = call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Lidarr", "token": r["token"] }))).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!((g["protocol"].as_str(), g["title"].as_str()), (Some("soulseek"), Some("Artist - Album")));
    let id = g["id"].as_i64().unwrap();
    st.hub.tick().await;
    assert_eq!(fake.lock().unwrap().downloads.len(), 2);
    assert_eq!(st.hub.get_grab(id).unwrap().state, "waiting");

    // one file in, one halfway
    {
        let mut f = fake.lock().unwrap();
        f.downloads[0].3 = "Completed, Succeeded".into();
        f.downloads[0].4 = 4;
        f.downloads[1].3 = "InProgress".into();
        f.downloads[1].4 = 2;
    }
    st.hub.tick().await;
    let g = st.hub.get_grab(id).unwrap();
    assert_eq!((g.state.as_str(), g.progress), ("downloading", 0.75));

    // both in: slskd's files are linked into the app's folder
    std::fs::create_dir_all(slsk_dir.join("Album")).unwrap();
    std::fs::write(slsk_dir.join("Album").join("01 - One.flac"), "one!").unwrap();
    std::fs::write(slsk_dir.join("Album").join("02 - Two.flac"), "two!").unwrap();
    {
        let mut f = fake.lock().unwrap();
        f.downloads[1].3 = "Completed, Succeeded".into();
        f.downloads[1].4 = 4;
    }
    st.hub.tick().await;
    let g = st.hub.get_grab(id).unwrap();
    assert_eq!(g.state, "completed");
    assert_eq!(g.path, format!("{folder}/Artist - Album"));
    assert_eq!(g.files, vec![(format!("{folder}/Artist - Album/01 - One.flac"), 4), (format!("{folder}/Artist - Album/02 - Two.flac"), 4)]);

    // removing it with its files takes it out of slskd and deletes both copies
    let (s, _) = call(&app, "DELETE", &format!("/api/hub/grabs/{id}?delete_files=true"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(fake.lock().unwrap().removed, ["t0", "t1"]);
    assert!(!std::path::Path::new(&g.path).exists());
    assert!(!slsk_dir.join("Album").join("01 - One.flac").exists());
}

#[tokio::test]
async fn a_file_turned_down_is_tried_again() {
    let (base, fake) = fake().await;
    let dir = std::env::temp_dir().join(format!("lantern-soulseek-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let downloads = dir.join("downloads").to_string_lossy().to_string();
    let vars: HashMap<&str, String> = HashMap::from([
        ("APP_API_KEY", "test-key".to_string()),
        ("APP_DOWNLOADS_DIR", downloads),
        ("APP_SOULSEEK_URL", base),
        ("APP_SOULSEEK_API_KEY", "slsk-key".to_string()),
    ]);
    let st = server::AppState::open(&dir.join("data"), web, move |n| vars.get(n).cloned()).await.unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    let app = server::app(st.clone());
    call(&app, "POST", "/api/hub/apps", Some(json!({ "name": "Lidarr", "media": "music" }))).await;
    let (_, found) = call(&app, "GET", "/api/hub/search?app=Lidarr&q=Album", None).await;
    let token = found["results"].as_array().unwrap().iter().find(|r| r["protocol"] == "soulseek").unwrap()["token"].clone();
    let (_, g) = call(&app, "POST", "/api/hub/grabs", Some(json!({ "app": "Lidarr", "token": token }))).await;
    let id = g["id"].as_i64().unwrap();
    st.hub.tick().await;
    fake.lock().unwrap().downloads[0].3 = "Completed, Rejected".into();
    st.hub.tick().await;
    let g = st.hub.get_grab(id).unwrap();
    assert_eq!((g.state.as_str(), g.attempts), ("queued", 1));
    assert_eq!(g.error, "ann turned down the download of 01 - One.flac.");
}
