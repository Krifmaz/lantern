//! Releases end to end: a made-up tracker's IRC server (served here) announces a release, Lantern reads it,
//! checks it against a filter and pushes it to a stand-in Sonarr within seconds; an RSS feed's new item does the
//! same; the dry-run check says why a release would be skipped; secrets never come back.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;

const KEY: &str = "test-key";

async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-rel-{name}-{}", std::process::id()));
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

/// A stand-in Sonarr: records every release pushed to it; turns down names with "Reject" in them.
async fn sonarr() -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
    let got: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let g = got.clone();
    let app = Router::new().route(
        "/api/v3/release/push",
        post(move |headers: axum::http::HeaderMap, Json(v): Json<Value>| {
            let g = g.clone();
            async move {
                let key = headers.get("x-api-key").and_then(|k| k.to_str().ok()).unwrap_or("").to_string();
                let reject = v["title"].as_str().unwrap_or("").contains("Reject");
                g.lock().unwrap().push((key, v));
                if reject {
                    Json(json!([{ "approved": false, "rejections": ["Unknown Series"] }]))
                } else {
                    Json(json!([{ "approved": true, "rejections": [] }]))
                }
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), got)
}

/// Waits up to 10 s for the release log to have a row with `status`.
async fn log_row(app: &Router, status: &str) -> Value {
    for _ in 0..100 {
        let (_, v) = call(app, "GET", &format!("/api/releases/log?status={status}"), None).await;
        if let Some(r) = v["releases"].as_array().and_then(|a| a.first()) {
            return r.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no {status} release in the log");
}

#[tokio::test]
async fn an_irc_announce_is_pushed_to_sonarr() {
    let (app, _) = lantern("irc").await;
    let (sonarr_url, got) = sonarr().await;

    // the tracker's IRC server: welcomes, waits for the JOIN, announces
    let irc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = irc.local_addr().unwrap().port();
    let said: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = said.clone();
    tokio::spawn(async move {
        let (sock, _) = irc.accept().await.unwrap();
        let (r, mut w) = sock.into_split();
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            s2.lock().unwrap().push(l.clone());
            if l.starts_with("USER ") {
                w.write_all(b":irc.lighthouse 001 lantern|bot :Welcome\r\n").await.unwrap();
            }
            if l.starts_with("JOIN #announce") {
                w.write_all(b":lantern|bot!u@h JOIN #announce\r\n").await.unwrap();
                w.write_all(b":someone!u@h PRIVMSG #announce :New Torrent: Fake.S01E01.1080p.WEB-DL.H.264-X [TV :: HD] 1 GB by u - https://lighthouse.example/torrent/1\r\n").await.unwrap();
                w.write_all(b":Keeper!bot@h PRIVMSG #announce :\x02New Torrent:\x02 Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP [TV :: HD] 1.5 GB by uploader FREELEECH - https://lighthouse.example/torrent/123\r\n").await.unwrap();
                w.write_all(b"PING :keepalive\r\n").await.unwrap();
            }
        }
    });

    let (s, f) = call(
        &app,
        "POST",
        "/api/releases/filters",
        Some(json!({
            "name": "TV in HD", "resolutions": ["1080p"], "freeleech": true, "max_size": "10 GB",
            "actions": [{ "kind": "arr", "app": "sonarr", "url": sonarr_url, "api_key": "sonarr-key" }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{f}");
    assert_eq!(f["actions"][0]["api_key"], "••••", "the API key never comes back");
    assert_eq!(f["skip_duplicates"], true, "race-friendly defaults");
    assert_eq!(f["delay"], 0);

    let (s, src) = call(
        &app,
        "POST",
        "/api/releases/sources",
        Some(json!({
            "kind": "irc", "definition": "lighthouse",
            "settings": { "nick": "lantern|bot", "passkey": "secret-passkey", "auth.password": "nickserv-pass", "server": "127.0.0.1", "port": port.to_string(), "tls": "false" }
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{src}");
    assert_eq!(src["name"], "Lighthouse");
    assert_eq!(src["settings"]["passkey"], "••••");
    assert_eq!(src["settings"]["auth.password"], "••••");
    assert_eq!(src["settings"]["nick"], "lantern|bot");

    let row = log_row(&app, "pushed").await;
    assert_eq!(row["title"], "Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP");
    assert_eq!(row["filter_name"], "TV in HD");
    assert_eq!(row["detail"], "Sent to Sonarr");
    assert_eq!(row["category"], "TV/HD");
    assert!(!row.to_string().contains("secret-passkey"), "the passkey is never logged");

    let pushed = got.lock().unwrap().clone();
    assert_eq!(pushed.len(), 1, "only the announcer's line counts");
    assert_eq!(pushed[0].0, "sonarr-key");
    assert_eq!(pushed[0].1["title"], "Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP");
    assert!(pushed[0].1["downloadUrl"].as_str().unwrap().starts_with("https://lighthouse.example/download/123/secret-passkey/"));
    assert_eq!(pushed[0].1["size"], 1_500_000_000u64);
    assert_eq!(pushed[0].1["indexer"], "Lighthouse");

    let said = said.lock().unwrap().clone();
    assert!(said.contains(&"NICK lantern|bot".to_string()), "{said:?}");
    assert!(said.contains(&"PRIVMSG NickServ :IDENTIFY lantern|bot nickserv-pass".to_string()), "{said:?}");
    assert!(said.contains(&"PONG :keepalive".to_string()), "{said:?}");

    let (_, list) = call(&app, "GET", "/api/releases/sources", None).await;
    let irc = &list["sources"][0]["irc"];
    assert_eq!(irc["connected"], true, "{list}");
    assert_eq!(irc["channels"], json!(["#announce"]));
    assert_eq!(irc["announces"], 1);

    // the same name again from an announce line: the filter already took it
    let id = src["id"].as_i64().unwrap();
    let line = "New Torrent: Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP [TV :: HD] 1.5 GB by uploader FREELEECH - https://lighthouse.example/torrent/124";
    let (_, v) = call(&app, "POST", "/api/releases/announce", Some(json!({ "source_id": id, "line": line }))).await;
    assert_eq!(v["status"], "rejected", "{v}");
    assert_eq!(v["reasons"][0]["why"][0], "it already took a release with this name");

    // Sonarr turning it down is a failure in words
    let line = "New Torrent: Reject.S01E03.1080p.WEB-DL.H.264-GROUP [TV :: HD] 1.5 GB by uploader FREELEECH - https://lighthouse.example/torrent/125";
    let (_, v) = call(&app, "POST", "/api/releases/announce", Some(json!({ "source_id": id, "line": line }))).await;
    assert_eq!(v["status"], "failed", "{v}");
    assert_eq!(v["detail"], "Sonarr doesn't want it: Unknown Series");

    let (s, v) = call(&app, "DELETE", &format!("/api/releases/sources/{id}"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn new_rss_items_go_through_the_filters_and_the_backlog_does_not() {
    let (app, _) = lantern("rss").await;
    let (sonarr_url, got) = sonarr().await;
    let items: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(vec!["Old.Movie.2020.1080p.BluRay.x264-OLD"]));
    let i2 = items.clone();
    let feed = Router::new().route(
        "/rss",
        get(move || {
            let i2 = i2.clone();
            async move {
                let body: String = i2
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|t| format!("<item><title>{t}</title><guid>{t}</guid><link>https://feed.example/dl/{t}.torrent</link><size>2000000000</size></item>"))
                    .collect();
                format!("<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Feed</title>{body}</channel></rss>")
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, feed).await.unwrap() });

    call(&app, "POST", "/api/releases/filters", Some(json!({ "name": "Movies", "years": "2020-2026", "actions": [{ "kind": "arr", "app": "sonarr", "url": sonarr_url, "api_key": "k" }] })))
        .await;
    let (s, src) = call(
        &app,
        "POST",
        "/api/releases/sources",
        Some(json!({ "kind": "rss", "url": format!("http://{addr}/rss?passkey=abc"), "interval_min": 1 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{src}");
    assert_eq!(src["url"], format!("http://{addr}/rss"), "the passkey in the address never comes back");
    assert_eq!(src["interval_min"], 2, "at least two minutes between looks");
    let id = src["id"].as_i64().unwrap();

    let (_, t) = call(&app, "POST", &format!("/api/releases/sources/{id}/test"), None).await;
    assert_eq!(t["ok"], true, "{t}");
    assert_eq!(t["message"], "The feed answered; nothing new since the last look");

    items.lock().unwrap().push("New.Movie.2025.2160p.WEB-DL.H.265-NEW");
    let (_, t) = call(&app, "POST", &format!("/api/releases/sources/{id}/test"), None).await;
    assert_eq!(t["message"], "The feed answered with 1 new releases", "{t}");
    let row = log_row(&app, "pushed").await;
    assert_eq!(row["title"], "New.Movie.2025.2160p.WEB-DL.H.265-NEW");
    let pushed = got.lock().unwrap().clone();
    assert_eq!(pushed.len(), 1);
    assert!(pushed[0].1["downloadUrl"].as_str().unwrap().ends_with("New.Movie.2025.2160p.WEB-DL.H.265-NEW.torrent"));

    // a masked or unchanged address keeps the saved one (with its passkey)
    let (s, v) =
        call(&app, "PUT", &format!("/api/releases/sources/{id}"), Some(json!({ "url": format!("http://{addr}/rss"), "name": "My feed" })))
            .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, t) = call(&app, "POST", &format!("/api/releases/sources/{id}/test"), None).await;
    assert_eq!(t["ok"], true, "{t}");
}

#[tokio::test]
async fn the_check_says_why_and_bad_input_is_explained() {
    let (app, _) = lantern("check").await;
    call(
        &app,
        "POST",
        "/api/releases/filters",
        Some(json!({ "name": "UHD", "resolutions": ["2160p"], "except_groups": "BAD", "min_size": "5 GB" })),
    )
    .await;
    let (s, v) =
        call(&app, "POST", "/api/releases/check", Some(json!({ "title": "Movie.2024.1080p.WEB-DL.x264-BAD", "size": "2 GB" }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["name"]["resolution"], "1080p");
    assert_eq!(v["filters"][0]["passes"], false);
    let why: Vec<String> = serde_json::from_value(v["filters"][0]["why"].clone()).unwrap();
    assert_eq!(why, vec!["its resolution (1080p) isn't one to grab", "the group BAD is one to skip", "it is 2.0 GB, smaller than 5 GB"]);

    let (s, v) = call(&app, "POST", "/api/releases/filters", Some(json!({ "name": "Bad", "max_size": "huge" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["detail"], "The largest size \"huge\" can't be read. Write it like 700 MB or 1.5 GB");
    let (s, v) = call(
        &app,
        "POST",
        "/api/releases/sources",
        Some(json!({ "kind": "irc", "definition": "lighthouse", "settings": { "nick": "me" } })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["detail"], "Fill in Passkey for Lighthouse");
    let (_, v) = call(&app, "GET", "/api/releases/definitions?q=light", None).await;
    assert_eq!(v["definitions"][0]["channels"], json!(["#announce"]));
    let (_, v) = call(&app, "GET", "/api/releases/definitions/lighthouse", None).await;
    assert_eq!(v["settings"][0]["secret"], true);

    // a filter with nowhere to send says so
    let (_, v) = call(
        &app,
        "POST",
        "/api/releases/announce",
        Some(json!({ "title": "Movie.2024.2160p.WEB-DL.x265-GOOD", "size": 6_000_000_000u64 })),
    )
    .await;
    assert_eq!(v["status"], "rejected");
    assert_eq!(v["reasons"][0]["why"][0], "it passes, but the filter has nowhere to send it");
}

#[tokio::test]
async fn a_client_action_becomes_a_grab_in_the_download_hub() {
    let (app, st) = lantern("hub").await;
    let (s, v) = call(
        &app,
        "POST",
        "/api/releases/filters",
        Some(json!({ "name": "To the hub", "resolutions": ["1080p"], "actions": [{ "kind": "client" }] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = call(
        &app,
        "POST",
        "/api/releases/announce",
        Some(json!({ "title": "Show.S01E01.1080p.WEB-DL.H.264-X", "download_url": "https://tracker.example/dl/1.torrent" })),
    )
    .await;
    assert_eq!(v["status"], "pushed", "{v}");
    assert!(v["detail"].as_str().unwrap().starts_with("Sent to the download hub for Releases"), "{v}");
    let hub_app = st.hub.app_named("Releases").expect("the Releases app is registered at the first push");
    let grabs = st.hub.grabs(Some(hub_app.id), None, 10).unwrap();
    assert_eq!(grabs.len(), 1);
    assert_eq!(grabs[0].title, "Show.S01E01.1080p.WEB-DL.H.264-X");

    // a hub app that doesn't exist is said in words
    call(&app, "PUT", "/api/releases/filters/1", Some(json!({ "actions": [{ "kind": "client", "client": "Nobody" }] }))).await;
    let (_, v) = call(
        &app,
        "POST",
        "/api/releases/announce",
        Some(json!({ "title": "Show.S01E02.1080p.WEB-DL.H.264-X", "download_url": "https://tracker.example/dl/2.torrent" })),
    )
    .await;
    assert_eq!(v["status"], "failed", "{v}");
    assert_eq!(v["detail"], "There is no app named Nobody. Register it first.");
}
