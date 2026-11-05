//! Indexers end to end: a small private tracker (served here, from tests/fixtures) added through the API, signed
//! in to, searched through Torznab (one indexer and all of them), the Prowlarr API and the web app's search, and a
//! release's file fetched through Lantern's signed download link.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

const KEY: &str = "test-key";

#[path = "support/tracker.rs"]
mod tracker;
use tracker::{start_tracker, TORRENT};

async fn lantern(name: &str) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-ix-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let defs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definitions").display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some(KEY.to_string()),
        "APP_DEFINITIONS_DIR" => Some(defs.clone()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    (server::app(st.clone()), st, dir)
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let r = app.clone().oneshot(req).await.unwrap();
    let (s, h) = (r.status(), r.headers().clone());
    let b = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
    (s, String::from_utf8_lossy(&b).into_owned(), h)
}

fn api(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY).header(header::HOST, "lantern.test:9696");
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

fn get_as_app(path: &str) -> Request<Body> {
    Request::get(path).header(header::HOST, "lantern.test:9696").header(header::USER_AGENT, "Sonarr/4.0").body(Body::empty()).unwrap()
}

#[tokio::test]
async fn a_private_tracker_is_added_signed_in_to_searched_and_grabbed() {
    let site = start_tracker().await;
    let (app, _, _) = lantern("full").await;

    // the definitions are there to pick from
    let (s, body, _) = call(&app, api("GET", "/api/indexers/definitions?q=lantern", None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body.contains("lantern-test"), "{body}");

    // a wrong password says so
    let (s, body, _) = call(
        &app,
        api(
            "POST",
            "/api/indexers",
            Some(json!({ "definition": "lantern-test", "base_url": site, "settings": { "username": "me", "password": "nope" } })),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let id = serde_json::from_str::<Value>(&body).unwrap()["id"].as_i64().unwrap();
    let (_, body, _) = call(&app, api("POST", &format!("/api/indexers/{id}/test"), None)).await;
    assert!(body.contains("\"ok\":false") && body.contains("Wrong username or password"), "{body}");

    // the right one works, and the password is never sent back
    let (s, body, _) = call(&app, api("PUT", &format!("/api/indexers/{id}"), Some(json!({ "settings": { "password": "pw" } })))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(!body.contains("\"pw\"") && body.contains("••••"), "{body}");
    let (_, body, _) = call(&app, api("POST", &format!("/api/indexers/{id}/test"), None)).await;
    assert!(body.contains("\"ok\":true") && body.contains("4 results"), "{body}");

    // Torznab caps: the modes and categories of the definition
    let (s, caps, _) = call(&app, get_as_app(&format!("/{id}/api?t=caps&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::OK, "{caps}");
    assert!(caps.contains(r#"<tv-search available="yes" supportedParams="q,season,ep,imdbid" />"#), "{caps}");
    assert!(caps.contains(r#"<subcat id="5040" name="TV/HD" />"#) && caps.contains(r#"<category id="100002" name="TV HD" />"#), "{caps}");

    // a key is needed
    let (s, body, _) = call(&app, get_as_app(&format!("/{id}/api?t=search&q=x&apikey=wrong"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(body.contains(r#"code="100""#), "{body}");

    // a TV search for HD TV: one release, with its fields read
    let (s, rss, _) = call(&app, get_as_app(&format!("/{id}/api?t=tvsearch&q=Show%20Name&season=1&ep=2&cat=5040&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::OK, "{rss}");
    assert!(rss.contains("<title>Show.Name.S01E02.1080p.WEB-DL-GRP</title>"), "{rss}");
    assert!(!rss.contains("Movie.2025"), "{rss}");
    for want in [
        r#"name="seeders" value="12""#,
        r#"name="peers" value="15""#,
        r#"name="downloadvolumefactor" value="0""#,
        r#"name="size" value="1610612736""#,
        "<pubDate>Wed, 04 Nov 2026 10:30:00 +0000</pubDate>",
        "http://lantern.test:9696/",
        "<category>5040</category>",
    ] {
        assert!(rss.contains(want), "{want} not in {rss}");
    }

    // the release's link goes through Lantern and gives the torrent
    let link = rss.split("<link>").nth(2).and_then(|l| l.split("</link>").next()).unwrap().replace("&amp;", "&");
    let path = link.trim_start_matches("http://lantern.test:9696");
    let (s, file, h) = call(&app, get_as_app(path)).await;
    assert_eq!(s, StatusCode::OK, "{file}");
    assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/x-bittorrent");
    assert_eq!(file.as_bytes(), TORRENT);
    // a link Lantern didn't make is refused
    let forged = path.replace("sig=", "sig=00");
    let (s, _, _) = call(&app, get_as_app(&forged)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // all indexers at once, and nothing found is an empty feed
    let (s, rss, _) = call(&app, get_as_app(&format!("/all/api?t=movie&imdbid=tt0111161&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(rss.contains("Movie.2025.2160p.UHD-GRP") && rss.contains(r#"name="imdbid" value="tt0111161""#), "{rss}");
    let (_, rss, _) = call(&app, get_as_app(&format!("/{id}/api?t=search&q=nothing%20here&apikey={KEY}"))).await;
    assert!(rss.contains("<channel>") && !rss.contains("<item>"), "{rss}");

    // the Prowlarr API: status, indexers and search
    let (s, body, _) = call(&app, api("GET", "/api/v1/system/status", None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (_, body, _) = call(&app, api("GET", "/api/v1/indexer", None)).await;
    let list: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(list[0]["name"], "Lantern Test Tracker");
    assert_eq!(list[0]["protocol"], "torrent");
    let (_, body, _) =
        call(&app, api("GET", &format!("/api/v1/search?query=Show&type=search&indexerIds={id}&categories=5000"), None)).await;
    let results: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(results.as_array().map(Vec::len), Some(1), "{body}");
    assert_eq!(results[0]["indexerFlags"], json!(["freeleech"]));

    // stats and history
    let (_, body, _) = call(&app, api("GET", "/api/indexers/stats", None)).await;
    assert!(body.contains("\"grabs\":1"), "{body}");
}

#[tokio::test]
async fn a_site_that_cant_be_reached_is_put_on_hold_and_said_so() {
    let (app, st, _) = lantern("down").await;
    let (_, body, _) = call(&app, api("POST", "/api/indexers", Some(json!({ "definition": "lantern-test", "base_url": "http://127.0.0.1:9/", "settings": { "username": "me", "password": "pw" } })))).await;
    let id = serde_json::from_str::<Value>(&body).unwrap()["id"].as_i64().unwrap();
    for _ in 0..2 {
        let (_, rss, _) = call(&app, get_as_app(&format!("/{id}/api?t=search&q=x&apikey={KEY}"))).await;
        assert!(rss.contains("be reached"), "{rss}");
    }
    let ix = st.indexers.get(id).unwrap();
    assert_eq!(ix.failures, 2);
    assert!(ix.disabled_until > 0.0, "two failures in a row put it on hold");
    assert_eq!(st.indexers.failing().unwrap().len(), 1);
    // a Usenet indexer needs an address
    let (s, body, _) = call(&app, api("POST", "/api/indexers", Some(json!({ "kind": "newznab", "name": "Mine" })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let (s, body, _) = call(
        &app,
        api("POST", "/api/indexers", Some(json!({ "kind": "newznab", "definition": "nzbgeek", "settings": { "apikey": "k" } }))),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // a preset needs only the API key: its address, API path and categories are filled in
    let geek: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(geek["base_url"], "https://api.nzbgeek.info");
    assert_eq!(geek["settings"]["api_path"], "/api");
    assert_eq!(geek["settings"]["categories"], "1000,2000,3000,4000,5000,6000,7000,8000");
    let (s, body, _) = call(
        &app,
        api("POST", "/api/indexers", Some(json!({ "kind": "newznab", "definition": "tabularasa", "settings": { "apikey": "k" } }))),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let tr: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((tr["base_url"].as_str(), tr["settings"]["api_path"].as_str()), (Some("https://www.tabula-rasa.pw"), Some("/api/v1/api")));
    // apps see only the preset's categories
    let (s, caps, _) = call(&app, get_as_app(&format!("/{}/api?t=caps&apikey={KEY}", tr["id"]))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(caps.contains(r#"id="5040""#) && !caps.contains(r#"id="8010""#), "{caps}");
}

#[tokio::test]
async fn torrent_sites_and_usenet_indexers_are_one_list_to_add_from() {
    let (app, _, _) = lantern("catalog").await;
    let (s, body, _) = call(&app, api("GET", "/api/indexers/catalog", None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let torrent = v["counts"]["protocol"]["torrent"].as_u64().unwrap();
    let usenet = v["counts"]["protocol"]["usenet"].as_u64().unwrap();
    assert!(torrent >= 1 && usenet >= 20, "{}", v["counts"]);
    assert_eq!(v["counts"]["all"].as_u64(), Some(torrent + usenet));
    let sites = v["sites"].as_array().unwrap();
    assert!(sites.iter().any(|x| x["protocol"] == "torrent" && x["kind"] == "cardigann"));
    let slug = sites.iter().find(|x| x["id"] == "drunkenslug").unwrap();
    assert_eq!((slug["url"].as_str(), slug["api_path"].as_str()), (Some("https://drunkenslug.com"), Some("/api")));
    // narrowed by protocol, privacy and words
    let (_, body, _) = call(&app, api("GET", "/api/indexers/catalog?protocol=usenet&privacy=public", None)).await;
    let v: Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = v["sites"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["AnimeTosho (Usenet)"]);
    let (_, body, _) = call(&app, api("GET", "/api/indexers/catalog?q=ninja", None)).await;
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["sites"][0]["name"], "NinjaCentral");
}
