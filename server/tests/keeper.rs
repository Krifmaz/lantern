//! The account keeper through its HTTP API: rules and numbers give risks in words, the notice banner names them,
//! a linked account is signed in to through its indexer, and each warning goes once to ntfy and Discord.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[path = "support/tracker.rs"]
mod tracker;

/// The API key of the tests' server.
const KEY: &str = "test-key";

/// A fresh Lantern for test `name`, with the test definitions and `env` as more environment variables.
async fn lantern(name: &str, env: Vec<(&'static str, String)>) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-keeper-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let defs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definitions").display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some(KEY.to_string()),
        "APP_DEFINITIONS_DIR" => Some(defs.clone()),
        other => env.iter().find(|(k, _)| *k == other).map(|(_, v)| v.clone()),
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    (server::app(st.clone()), st)
}

/// One request through the API key; the status and the body as JSON.
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
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

/// A vault account named `name`; its id.
async fn add_account(app: &Router, name: &str) -> String {
    let (s, v) = call(
        app,
        "POST",
        "/api/vault/items",
        Some(json!({ "kind": "tracker", "name": name, "fields": { "username": "me", "password": "pw" } })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn rules_and_numbers_give_risks_in_words() {
    let (app, _) = lantern("risks", vec![]).await;
    let id = add_account(&app, "Site").await;
    let (_, v) = call(&app, "GET", "/api/keeper", None).await;
    assert_eq!(v["accounts"][0]["name"], "Site");
    assert_eq!(v["accounts"][0]["risks"], json!([]));
    assert_eq!(v["warn_days"], 7.0);
    let (s, v) = call(
        &app,
        "PUT",
        &format!("/api/keeper/{id}/rules"),
        Some(json!({ "min_ratio": 1.0, "max_hnr": 2, "cookie_expires": "2000-01-01" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let gib = 1024.0 * 1024.0 * 1024.0;
    let (s, v) = call(
        &app,
        "PUT",
        &format!("/api/keeper/{id}/stats"),
        Some(json!({ "uploaded": 80.0 * gib, "downloaded": 100.0 * gib, "hnr_count": 2, "bonus": 1500 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ratio"], 0.8);
    let keys: Vec<&str> = v["risks"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["cookie_expires", "ratio", "hnr"]);
    assert!(v["risks"][0]["text"].as_str().unwrap().starts_with("Site: the saved cookie ran out "), "{v}");
    // the banner names the most urgent one
    let (_, n) = call(&app, "POST", "/api/notices/accounts/check", None).await;
    assert_eq!(n["ok"], false, "{n}");
    assert!(n["text"].as_str().unwrap().starts_with("Site: the saved cookie ran out"), "{n}");
    // a second reading goes into the history
    call(&app, "PUT", &format!("/api/keeper/{id}/stats"), Some(json!({ "uploaded": 120.0 * gib, "hnr_count": 0 }))).await;
    let (_, h) = call(&app, "GET", &format!("/api/keeper/{id}/history"), None).await;
    assert_eq!(h["history"].as_array().map(Vec::len), Some(2));
    assert_eq!(h["history"][1]["downloaded"], 100.0 * gib, "a number not sent stays");
    let (_, v) = call(&app, "PUT", &format!("/api/keeper/{id}/rules"), Some(json!({ "cookie_expires": null }))).await;
    assert_eq!(v["risks"], json!([]));
    let (_, n) = call(&app, "POST", "/api/notices/accounts/check", None).await;
    assert_eq!(n["ok"], true, "{n}");
    // words for what can't be used
    let (s, v) = call(&app, "PUT", &format!("/api/keeper/{id}/rules"), Some(json!({ "login_expires": "next week" }))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("The login's end must be a date like 2026-12-31.")));
    let (s, v) = call(&app, "PUT", &format!("/api/keeper/{id}/stats"), Some(json!({ "ratio": -1 }))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("The ratio must be a number of 0 or more.")));
    let (s, _) = call(&app, "GET", "/api/keeper/nothing", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_linked_account_is_kept_signed_in_through_its_indexer() {
    let site = tracker::start_tracker().await;
    let (app, st) = lantern("keepalive", vec![]).await;
    let (_, ix) = call(
        &app,
        "POST",
        "/api/indexers",
        Some(json!({ "definition": "lantern-test", "base_url": site, "settings": { "username": "me", "password": "pw" } })),
    )
    .await;
    let ix = ix["id"].as_i64().unwrap();
    let id = add_account(&app, "Test Tracker").await;
    let (_, v) = call(&app, "PUT", &format!("/api/keeper/{id}/rules"), Some(json!({ "login_days": 30 }))).await;
    assert_eq!(v["risks"][0]["key"], "unknown_login", "{v}");
    let (s, v) = call(&app, "POST", &format!("/api/keeper/{id}/sign-in"), None).await;
    assert_eq!((s, v["detail"].as_str().map(|d| d.contains("isn't linked to an indexer"))), (StatusCode::BAD_REQUEST, Some(true)));
    call(&app, "PUT", &format!("/api/keeper/{id}/rules"), Some(json!({ "indexer_id": ix }))).await;
    // the round signs in on its own, once
    assert_eq!(server::keeper::keep_alive(&st).await, 1);
    let (_, v) = call(&app, "GET", &format!("/api/keeper/{id}"), None).await;
    assert!(v["signed_in_at"].is_number(), "{v}");
    assert_eq!(v["keep_alive_error"], "");
    assert_eq!(v["risks"], json!([]));
    assert_eq!(server::keeper::keep_alive(&st).await, 0);
    let (s, v) = call(&app, "POST", &format!("/api/keeper/{id}/sign-in"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // an indexer that isn't there can't be linked
    let (s, v) = call(&app, "PUT", &format!("/api/keeper/{id}/rules"), Some(json!({ "indexer_id": 999 }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
}

#[tokio::test]
async fn each_warning_goes_once_to_ntfy_and_discord() {
    let got: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let (g1, g2) = (got.clone(), got.clone());
    let hooks = Router::new()
        .route(
            "/topic",
            post(move |h: axum::http::HeaderMap, body: String| async move {
                let title = h.get("title").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                g1.lock().unwrap().push((format!("ntfy:{title}"), body));
                "ok"
            }),
        )
        .route(
            "/discord",
            post(move |body: String| async move {
                g2.lock().unwrap().push(("discord".into(), body));
                StatusCode::NO_CONTENT
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, hooks).await.unwrap() });
    let (app, st) = lantern(
        "alerts",
        vec![("APP_ALERT_NTFY_URL", format!("http://{addr}/topic")), ("APP_ALERT_DISCORD_WEBHOOK", format!("http://{addr}/discord"))],
    )
    .await;
    let (s, v) = call(&app, "POST", "/api/keeper/test-alerts", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["results"].as_array().unwrap().iter().filter(|r| r["ok"] == true).count(), 2, "{v}");
    got.lock().unwrap().clear();
    let id = add_account(&app, "Site").await;
    call(&app, "PUT", &format!("/api/keeper/{id}/rules"), Some(json!({ "min_ratio": 1.0 }))).await;
    call(&app, "PUT", &format!("/api/keeper/{id}/stats"), Some(json!({ "ratio": 0.5 }))).await;
    server::keeper::tick(&st).await;
    server::keeper::tick(&st).await;
    let sent = got.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "once per channel: {sent:?}");
    assert_eq!(sent.iter().filter(|(k, _)| k == "ntfy:Lantern: an account is at risk now").count(), 1, "{sent:?}");
    assert!(sent.iter().all(|(_, b)| b.contains("Site: the ratio 0.5 is under the site's minimum of 1.")), "{sent:?}");
    // cleared, then back: sent again
    call(&app, "PUT", &format!("/api/keeper/{id}/stats"), Some(json!({ "ratio": 2.0 }))).await;
    server::keeper::tick(&st).await;
    call(&app, "PUT", &format!("/api/keeper/{id}/stats"), Some(json!({ "ratio": 0.5 }))).await;
    server::keeper::tick(&st).await;
    assert_eq!(got.lock().unwrap().len(), 4);
    // with no channel, the test says where to set one
    let (app, _) = lantern("no-alerts", vec![]).await;
    let (s, v) = call(&app, "POST", "/api/keeper/test-alerts", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["detail"].as_str().unwrap().starts_with("No alert channel is set."));
}
