// Ported from autobrr v1.90.0 (internal/http/filter_test.go), GPL-2.0-or-later, with API shape checks against
// autobrr's Go structs (tests/fixtures/upstream/autobrr/api/structs.json, read from internal/domain).
//! autobrr's API under `/autobrr/api`: filters, actions and download clients answer with autobrr's JSON, status
//! codes and error words; a filter's autobrr actions run when a release passes it.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const KEY: &str = "test-key";

async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-abapi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let announce = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/announce").display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some(KEY.to_string()),
        "APP_ANNOUNCE_DIR" => Some(announce.clone()),
        "APP_DEFINITIONS_DIR" => Some("/nonexistent".to_string()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    (server::app(st.clone()), st)
}

/// Calls `path` with a raw `body`.
async fn raw(app: &Router, method: &str, path: &str, body: Option<&str>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = body.map(|v| v.to_string());
    raw(app, method, &format!("/autobrr/api{path}"), b.as_deref()).await
}

/// The JSON keys autobrr's Go struct `name` always sends, and those it may.
fn shape(name: &str) -> (Vec<String>, Vec<String>) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/api/structs.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let s = &v["structs"][name];
    let list = |k: &str| s[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
    (list("always"), list("omitempty"))
}

/// `v` has every key struct `name` always sends, and none it doesn't have (but `extra`).
fn assert_shape(name: &str, v: &Value, extra: &[&str]) {
    let (always, maybe) = shape(name);
    let o = v.as_object().unwrap_or_else(|| panic!("{name} is not an object: {v}"));
    for k in &always {
        assert!(o.contains_key(k), "{name} lacks {k}: {v}");
    }
    for k in o.keys() {
        assert!(always.contains(k) || maybe.contains(k) || extra.contains(&k.as_str()), "{name} has {k}, which autobrr's hasn't: {v}");
    }
}

fn assert_error(v: &Value, code: &str, message: &str) {
    assert_eq!(v["code"], code, "{v}");
    assert_eq!(v["message"], message, "{v}");
    assert_eq!(v["detail"], message, "Lantern's detail holds the same words: {v}");
}

#[tokio::test]
async fn download_clients_redact_and_keep_secrets() {
    let (app, st) = lantern("dl").await;
    let body = json!({
        "name": "qb", "type": "QBITTORRENT", "enabled": true, "host": "http://qbit:8080", "port": 0, "tls": false,
        "tls_skip_verify": false, "username": "admin", "password": "secret",
        "settings": { "apikey": "", "rules": { "enabled": true, "max_active_downloads": 2 }, "auth": { "enabled": true, "type": "BASIC_AUTH", "username": "u", "password": "bp" } }
    });
    let (s, d) = call(&app, "POST", "/downloaders", Some(body.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{d}");
    assert_shape("Downloader", &d, &[]);
    assert_shape("DownloaderSettings", &d["settings"], &[]);
    assert_shape("DownloaderRules", &d["settings"]["rules"], &[]);
    assert_eq!(d["password"], "<redacted>");
    assert_eq!(d["settings"]["auth"]["password"], "<redacted>");
    let id = d["id"].as_i64().unwrap();

    let (s, list) = call(&app, "GET", "/downloaders", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, one) = call(&app, "GET", &format!("/downloaders/{id}"), None).await;
    assert_eq!(one["name"], "qb");

    let mut changed = one.clone();
    changed["name"] = json!("qb2");
    let (s, v) = call(&app, "PUT", "/downloaders", Some(changed)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let saved = st.releases.downloader(id).unwrap();
    assert_eq!((saved.name.as_str(), saved.password.as_str(), saved.settings.auth.password.as_str()), ("qb2", "secret", "bp"));

    let (s, v) = call(&app, "GET", "/downloaders/99", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "download client with id 99 not found");
    let (s, v) = call(&app, "GET", "/downloaders/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "clientID parameter is invalid");
    let (s, v) = call(&app, "POST", "/downloaders", Some(json!({ "name": "x", "type": "QBITTORRENT" }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "validation error: missing host");
    let (s, v) = call(&app, "POST", "/downloaders/test", Some(json!({ "name": "x", "type": "NOPE", "host": "h" }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "invalid download client type");
    let (s, v) = call(&app, "GET", &format!("/downloaders/{id}/arr/tags"), None).await;
    assert_eq!((s, v), (StatusCode::OK, json!([])), "a torrent client has no tags");

    let (s, _) = call(&app, "DELETE", &format!("/downloaders/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&app, "DELETE", &format!("/downloaders/{id}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn actions_store_toggle_and_delete() {
    let (app, _) = lantern("actions").await;
    let (s, a) = call(&app, "POST", "/actions", Some(json!({ "name": "test", "type": "TEST", "enabled": true, "filter_id": 0 }))).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert_shape("Action", &a, &[]);
    let id = a["id"].as_i64().unwrap();
    let (s, v) = call(&app, "PUT", &format!("/actions/{id}"), Some(json!({ "name": "renamed", "type": "TEST", "enabled": true }))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["name"], "renamed");
    let (s, _) = call(&app, "PATCH", &format!("/actions/{id}/toggleEnabled"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, "GET", "/actions", None).await;
    assert_eq!(list[0]["enabled"], false);
    let (s, _) = call(&app, "DELETE", &format!("/actions/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&app, "PATCH", &format!("/actions/{id}/toggleEnabled"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("action with id {id} not found"));
}

#[tokio::test]
async fn filters_keep_autobrrs_shape_and_rules() {
    let (app, st) = lantern("filters").await;
    let (s, f) = call(&app, "POST", "/filters", Some(json!({ "name": "TV", "enabled": true, "resolutions": ["1080p"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    assert_eq!(f["announce_types"], json!(["NEW"]), "a new filter takes NEW announces");
    assert_shape("Filter", &f, &[]);
    let id = f["id"].as_i64().unwrap();
    call(&app, "POST", "/filters", Some(json!({ "name": "Movies", "priority": 5 }))).await;

    let (s, v) = call(&app, "GET", "/filters?sort=bogus", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "sort parameter is invalid");
    let (_, v) = call(&app, "GET", "/filters?sort=name-asc", None).await;
    assert_eq!((v[0]["name"].as_str(), v[1]["name"].as_str()), (Some("Movies"), Some("TV")));
    let (_, v) = call(&app, "GET", "/filters", None).await;
    assert_eq!(v[0]["name"], "Movies", "highest priority first");

    // replace: the actions, external filters and notifications come along
    let mut full = f.clone();
    full["actions"] = json!([{ "name": "a1", "type": "TEST", "enabled": true }, { "name": "a2", "type": "TEST", "enabled": false }]);
    full["external"] = json!([{ "name": "ok", "index": 0, "type": "EXEC", "enabled": true, "exec_cmd": "true", "on_error": "REJECT" }]);
    let (s, v) = call(&app, "PUT", &format!("/filters/{id}"), Some(full.clone())).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, got) = call(&app, "GET", &format!("/filters/{id}"), None).await;
    assert_shape("Filter", &got, &[]);
    assert_eq!((got["actions_count"].as_i64(), got["actions_enabled_count"].as_i64()), (Some(2), Some(1)));
    assert_shape("Action", &got["actions"][0], &[]);
    assert_shape("FilterExternal", &got["external"][0], &[]);
    // actions left out of a save stay (autobrr's StoreFilterActions)
    let mut one = got.clone();
    one["actions"] = json!([got["actions"][0].clone()]);
    call(&app, "PUT", &format!("/filters/{id}"), Some(one)).await;
    let (_, got) = call(&app, "GET", &format!("/filters/{id}"), None).await;
    assert_eq!(got["actions"].as_array().unwrap().len(), 2);

    // a missing program, indexer or notification is refused in autobrr's words
    let mut bad = got.clone();
    bad["external"] = json!([{ "name": "x", "type": "EXEC", "enabled": true, "exec_cmd": "no-such-program-here" }]);
    let (s, v) = call(&app, "PUT", &format!("/filters/{id}"), Some(bad)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(v["message"].as_str().unwrap().starts_with("could not find external exec command: no-such-program-here"), "{v}");
    let mut bad = got.clone();
    bad["indexers"] = json!([{ "id": 999 }]);
    let (s, v) = call(&app, "PUT", &format!("/filters/{id}"), Some(bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "indexer with id 999 does not exist: indexer not found");

    let (s, _) = call(&app, "PATCH", &format!("/filters/{id}"), Some(json!({ "priority": 9 }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&app, "PUT", &format!("/filters/{id}/enabled"), Some(json!({ "enabled": false }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, got) = call(&app, "GET", &format!("/filters/{id}"), None).await;
    assert_eq!((got["priority"].as_i64(), got["enabled"].as_bool()), (Some(9), Some(false)));

    let (s, copy) = call(&app, "GET", &format!("/filters/{id}/duplicate"), None).await;
    assert_eq!(s, StatusCode::OK, "{copy}");
    assert_eq!((copy["name"].as_str(), copy["enabled"].as_bool()), (Some("TV Copy"), Some(false)));
    assert_eq!(copy["actions"].as_array().unwrap().len(), 2);
    assert_ne!(copy["actions"][0]["id"], got["actions"][0]["id"], "the copy has actions of its own");

    // TestFilterHandlerUpdateNotifications
    let (s, v) =
        call(&app, "PUT", &format!("/filters/{id}/notifications"), Some(json!([{ "notification_id": 3, "events": ["PUSH_APPROVED"] }])))
            .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "notification with id 3 does not exist: notification not found");
    let n = st.releases.add_notification("Discord", "DISCORD").unwrap();
    let body = json!([{ "notification_id": n, "events": ["PUSH_APPROVED"] }]).to_string();
    let (s, _) = raw(&app, "PUT", &format!("/autobrr/api/filters/{id}/notifications/"), Some(&body)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, v) = call(&app, "GET", &format!("/filters/{id}/notifications"), None).await;
    assert_shape("FilterNotification", &v[0], &[]);
    assert_eq!((v[0]["notification_id"].as_i64(), v[0]["events"].clone()), (Some(n), json!(["PUSH_APPROVED"])));

    let (_, v) = call(&app, "POST", "/filters/indexers/prune-deprecated", Some(json!({ "identifiers": ["gone"] }))).await;
    assert_eq!(v, json!({ "removed": 0 }));

    let (s, _) = call(&app, "DELETE", &format!("/filters/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&app, "GET", &format!("/filters/{id}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("filter with id {id} not found"));
    let (s, v) = call(&app, "GET", "/filters/x", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "filterID parameter is invalid");
}

/// TestFilterHandlerTestExternal.
#[tokio::test]
async fn external_filter_test_route() {
    let (app, _) = lantern("external").await;
    let body = json!({ "name": "hook", "type": "EXEC", "enabled": true, "exec_cmd": "true", "on_error": "REJECT" }).to_string();
    let (s, v) = raw(&app, "POST", "/autobrr/api/filters/external/test", Some(&body)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["success"], true);
    assert_shape("FilterExternalTestResult", &v, &[]);
    let (s, _) = raw(&app, "POST", "/autobrr/api/filters/external/test", Some("{not json")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "rejects malformed json");
    let (s, v) = raw(&app, "POST", "/autobrr/api/filters/external/test", Some("null")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "rejects null body");
    assert_eq!(v["message"], "external filter is required");
    let (s, v) = raw(&app, "POST", "/autobrr/api/filters/external/test", Some(r#"{"type":"NOPE"}"#)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "rejects unknown type");
    assert_eq!(v["code"], "BAD_REQUEST_PARAMS");
}

/// A stand-in Sonarr recording every push.
async fn sonarr() -> (String, Arc<Mutex<Vec<Value>>>) {
    let got: Arc<Mutex<Vec<Value>>> = Arc::default();
    let g = got.clone();
    let app = Router::new().route(
        "/api/v3/release/push",
        post(move |Json(v): Json<Value>| {
            let g = g.clone();
            async move {
                g.lock().unwrap().push(v);
                Json(json!([{ "approved": true, "rejected": false, "rejections": [] }]))
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), got)
}

#[tokio::test]
async fn a_passing_release_runs_the_filters_autobrr_actions() {
    let (app, st) = lantern("run").await;
    let (url, pushed) = sonarr().await;
    let (_, d) = call(
        &app,
        "POST",
        "/downloaders",
        Some(json!({ "name": "sonarr", "type": "SONARR", "enabled": true, "host": url, "settings": { "apikey": "k", "external_download_client": "qbit" } })),
    )
    .await;
    let (_, f) = call(&app, "POST", "/filters", Some(json!({ "name": "TV", "enabled": true, "resolutions": ["1080p"] }))).await;
    let mut full = f.clone();
    full["actions"] = json!([
        { "name": "to sonarr", "type": "SONARR", "enabled": true, "client_id": d["id"] },
        { "name": "to the hub", "type": "DOWNLOAD_HUB", "enabled": true },
        { "name": "off", "type": "EXEC", "enabled": false, "exec_cmd": "false" }
    ]);
    let (s, v) = call(&app, "PUT", &format!("/filters/{}", f["id"]), Some(full)).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    let (_, v) = raw(
        &app,
        "POST",
        "/api/releases/announce",
        Some(&json!({ "title": "Show.S01E01.1080p.WEB-DL.H.264-X", "download_url": "https://tracker.example/dl/1.torrent" }).to_string()),
    )
    .await;
    assert_eq!(v["status"], "pushed", "{v}");
    let p = pushed.lock().unwrap().clone();
    assert_eq!(p.len(), 1, "{v}");
    assert_eq!(p[0]["title"], "Show.S01E01.1080p.WEB-DL.H.264-X");
    assert_eq!(p[0]["downloadUrl"], "https://tracker.example/dl/1.torrent");
    assert_eq!(p[0]["downloadClient"], "qbit");
    assert_eq!(p[0]["protocol"], "torrent");
    let hub_app = st.hub.app_named("Releases").expect("the DOWNLOAD_HUB action went through the hub");
    assert_eq!(st.hub.grabs(Some(hub_app.id), None, 10).unwrap().len(), 1);

    // an external filter turning it down keeps it from the actions
    let (_, got) = call(&app, "GET", &format!("/filters/{}", f["id"]), None).await;
    let mut ext = got.clone();
    ext["external"] = json!([{ "name": "no", "index": 0, "type": "EXEC", "enabled": true, "exec_cmd": "false", "exec_expect_status": 0, "on_error": "REJECT" }]);
    call(&app, "PUT", &format!("/filters/{}", f["id"]), Some(ext)).await;
    let (_, v) = raw(
        &app,
        "POST",
        "/api/releases/announce",
        Some(&json!({ "title": "Show.S01E02.1080p.WEB-DL.H.264-X", "download_url": "https://tracker.example/dl/2.torrent" }).to_string()),
    )
    .await;
    assert_eq!(v["status"], "rejected", "{v}");
    assert!(v["reasons"][0]["why"][0].as_str().unwrap().starts_with("[external script exit code] not matching: got 1 want: 0"), "{v}");
    assert_eq!(pushed.lock().unwrap().len(), 1);
}
