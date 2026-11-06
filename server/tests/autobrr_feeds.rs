// Ported from autobrr v1.90.0 (internal/feed/service_test.go, torznab_test.go, newznab_test.go, rss_test.go
// fetch cases; pkg/torznab and pkg/newznab search_test.go and torznab_test.go GetCaps; internal/http/feed.go,
// list.go and webhook.go behaviour), GPL-2.0-or-later.
//! autobrr's feeds and lists under `/autobrr/api`, against stand-in Torznab, Newznab, RSS, Radarr and list
//! servers on 127.0.0.1: the Torznab and Newznab clients' paging and caps, each feed format read as releases, the
//! feed routes (adding, changing, turning on and off, testing, forced runs, the seen-item cache, the last run),
//! the feed job following its indexer, and the lists filling their filters. Nothing here reaches the internet.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::Router;
use serde_json::{json, Value};
use server::releases::feed::refresh::Source;
use server::releases::feed::sources::{RssSource, ZnabSource};
use server::releases::feed::znab::{self, Kind};
use server::releases::feed::Feed;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;

const KEY: &str = "test-key";

/// One request a stand-in server saw.
#[derive(Debug, Clone, Default)]
struct Seen {
    /// The path.
    path: String,
    /// The query, decoded.
    query: HashMap<String, String>,
    /// The headers, lower-case names.
    headers: HashMap<String, String>,
}

impl Seen {
    fn q(&self, k: &str) -> &str {
        self.query.get(k).map(String::as_str).unwrap_or("")
    }
}

/// What a stand-in answers: status, content type, body.
type Reply = (u16, &'static str, String);

/// A stand-in server answering with `f`; its address and what it saw.
async fn serve(f: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s2, f) = (seen.clone(), Arc::new(f));
    let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let (seen, f) = (s2.clone(), f.clone());
        async move {
            let req = Seen {
                path: uri.path().to_string(),
                query: url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()).into_owned().collect(),
                headers: headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect(),
            };
            seen.lock().unwrap().push(req.clone());
            let slow = req.q("slow") == "1" || req.path.ends_with("/slow");
            if slow && req.q("t") != "caps" {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            let (status, ct, body) = f(&req);
            (StatusCode::from_u16(status).unwrap(), [(header::CONTENT_TYPE, ct)], body).into_response()
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

/// autobrr's feed fixture `name`.
fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/feed").join(name)).unwrap()
}

/// Lantern with autobrr's generic Torznab, Newznab and RSS definitions.
async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-abfeed-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let announce = dir.join("announce");
    std::fs::create_dir_all(&announce).unwrap();
    let defs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/definitions");
    for d in ["generic_torznab.yaml", "generic_newznab.yaml", "generic_rss.yaml"] {
        std::fs::copy(defs.join(d), announce.join(d)).unwrap();
    }
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
    (server::app(st.clone()), st)
}

/// Calls autobrr's `path` (under `/autobrr/api`): the status and the body, JSON or else as text.
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(format!("/autobrr/api{path}")).header("x-api-key", KEY);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()));
    (s, v)
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

const CAPS_100: &str = r#"<caps><limits max="100" default="50"/></caps>"#;
const EMPTY_RSS: &str = "<rss><channel><title>test</title></channel></rss>";

// pkg/torznab and pkg/newznab search_test.go
#[tokio::test]
async fn znab_search_pages_with_the_caps_limit_and_keeps_host_params() {
    for kind in [Kind::Torznab, Kind::Newznab] {
        // TestClient_Search_Offset
        let (url, seen) = serve(|r| match r.q("t") {
            "caps" => (200, "application/xml", CAPS_100.into()),
            _ => (200, "application/xml", EMPTY_RSS.into()),
        })
        .await;
        for (offset, want) in [(0, ""), (100, "100")] {
            let mut c = znab::Client::plain(kind, &url, "");
            let resp = c.search("", &[], offset).await.unwrap();
            let last = seen.lock().unwrap().iter().rev().find(|s| s.q("t") == "search").cloned().unwrap();
            assert_eq!(last.q("offset"), want, "{kind:?}");
            assert_eq!(last.q("limit"), "100", "page size comes from the caps max");
            assert_eq!(resp.limit, 100);
        }

        // TestClient_Search_DefaultLimit
        let (url, _) = serve(|r| match r.q("t") {
            "caps" => (200, "application/xml", "<caps></caps>".into()),
            _ => (200, "application/xml", EMPTY_RSS.into()),
        })
        .await;
        let resp = znab::Client::plain(kind, &url, "").search("", &[], 0).await.unwrap();
        assert_eq!(resp.limit, 50, "caps without limits fall back to the default page size");

        // TestClient_Search_PagingResponse
        for (body, total) in [
            (
                r#"<rss><channel><title>test</title><newznab:response xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/" offset="0" total="7"/><item><title>a</title><guid>g1</guid></item></channel></rss>"#,
                7,
            ),
            (
                r#"<rss><channel><title>test</title><torznab:response xmlns:torznab="http://torznab.com/schemas/2015/feed" offset="0" total="7"/><item><title>a</title><guid>g1</guid></item></channel></rss>"#,
                7,
            ),
            ("<rss><channel><title>test</title><item><title>a</title><guid>g1</guid></item></channel></rss>", 0),
        ] {
            let body = body.to_string();
            let (url, _) = serve(move |r| match r.q("t") {
                "caps" => (200, "application/xml", CAPS_100.into()),
                _ => (200, "application/xml", body.clone()),
            })
            .await;
            assert_eq!(znab::Client::plain(kind, &url, "").search("", &[], 0).await.unwrap().total, total);
        }

        // TestClient_Search_KeepsHostQueryParams
        let (url, seen) = serve(|r| match r.q("t") {
            "caps" => (200, "application/xml", CAPS_100.into()),
            _ => (200, "application/xml", EMPTY_RSS.into()),
        })
        .await;
        znab::Client::plain(kind, &format!("{url}/api?foo=bar&apikey=hostkey"), "fieldkey").search("", &[], 25).await.unwrap();
        let all = seen.lock().unwrap().clone();
        let caps = all.iter().find(|s| s.q("t") == "caps").unwrap();
        let search = all.iter().find(|s| s.q("t") == "search").unwrap();
        assert_eq!((search.q("foo"), search.q("apikey"), search.q("offset")), ("bar", "fieldkey", "25"));
        assert_eq!((caps.q("foo"), caps.q("apikey")), ("bar", "fieldkey"));

        // TestClient_Search_KeepsValidHostParamsOnMalformedQuery
        let (url, seen) = serve(|r| match r.q("t") {
            "caps" => (200, "application/xml", CAPS_100.into()),
            _ => (200, "application/xml", EMPTY_RSS.into()),
        })
        .await;
        znab::Client::plain(kind, &format!("{url}/api?foo=bar&bad=%zz"), "").search("", &[], 0).await.unwrap();
        assert!(seen.lock().unwrap().iter().all(|s| s.q("foo") == "bar"));
    }
}

// pkg/torznab torznab_test.go TestClient_GetCaps
#[tokio::test]
async fn znab_caps_read_the_fixture_and_refuse_a_bad_key() {
    let (url, _) = serve(|r| {
        if r.q("apikey") != "mock-key" {
            return (401, "text/plain", String::new());
        }
        (200, "application/xml", fixture("caps_response.xml"))
    })
    .await;
    let caps = znab::Client::plain(Kind::Torznab, &format!("{url}/api/"), "mock-key").fetch_caps().await.unwrap();
    assert_eq!(caps.server[0], "1.1");
    assert_eq!(caps.server[4], "http://indexer.local/");
    assert_eq!((caps.limits_max, caps.limits_default), (100, 50));
    assert_eq!(caps.categories.len(), 2);
    let err = znab::Client::plain(Kind::Torznab, &url, "badkey").fetch_caps().await.unwrap_err();
    assert_eq!(err, "could not get caps for feed: unauthorized");
}

fn mock_feed(kind: &str, url: &str) -> Feed {
    let mut f = Feed { kind: kind.into(), url: url.into(), ..Feed::default() };
    f.indexer.name = "Mock Feed".into();
    f.indexer.identifier = "mock-feed".into();
    f
}

// internal/feed torznab_test.go, newznab_test.go and rss_test.go: the fetch cases
#[tokio::test]
async fn each_format_reads_a_page_as_releases() {
    // TestTorznabSource_fetch
    let (url, seen) = serve(|r| match r.q("t") {
        "caps" => (200, "application/xml", fixture("caps_response.xml")),
        _ => (200, "application/xml", fixture("torznab_response.xml")),
    })
    .await;
    let mut src = ZnabSource::new(Arc::new(mock_feed("TORZNAB", &url)), Kind::Torznab);
    let res = src.fetch(50).await.unwrap();
    assert_eq!(seen.lock().unwrap().iter().find(|s| s.q("t") == "search").unwrap().q("offset"), "50", "offset sent to the indexer");
    assert!(!res.raw.is_empty());
    assert!(!res.entries.is_empty());
    for e in &res.entries {
        assert!(!e.key.is_empty());
        let r = (e.release)();
        assert_eq!(r.torrent_name, e.title);
        assert_eq!(r.implementation, "TORZNAB");
    }

    // TestNewznabSource_fetch
    let response = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
<title>Mock Indexer</title>
<item>
<title>Some.Release.Title.2022.09.22.720p.WEB.h264-GROUP</title>
<guid>https://indexer.local/details/abc</guid>
<pubDate>Mon, 05 Oct 2026 10:00:00 +0000</pubDate>
<enclosure url="https://indexer.local/getnzb/abc.nzb" length="1073741824" type="application/x-nzb"/>
</item>
<item>
<title>Item.Without.Guid</title>
</item>
</channel>
</rss>"#;
    let (url, seen) = serve(move |r| match r.q("t") {
        "caps" => (200, "application/xml", "<caps/>".into()),
        _ => (200, "application/xml", response.into()),
    })
    .await;
    let mut src = ZnabSource::new(Arc::new(mock_feed("NEWZNAB", &url)), Kind::Newznab);
    let res = src.fetch(50).await.unwrap();
    assert_eq!(seen.lock().unwrap().iter().find(|s| s.q("t") == "search").unwrap().q("offset"), "50");
    assert!(res.raw.contains("Mock Indexer"));
    assert_eq!(res.entries.len(), 1, "the item without a guid is skipped");
    assert_eq!(res.entries[0].key, "https://indexer.local/details/abc");
    assert_eq!(res.entries[0].pub_date.unwrap().to_rfc3339(), "2026-10-05T10:00:00+00:00");
    assert_eq!((res.entries[0].release)().download_url, "https://indexer.local/getnzb/abc.nzb");

    // TestRSSSource_fetch
    let response = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
<title>Mock Feed</title>
<item>
<title>Item.With.Guid</title>
<link>https://fake-feed.com/download/1</link>
<guid>guid-1</guid>
<pubDate>Mon, 05 Oct 2026 10:00:00 +0000</pubDate>
</item>
<item>
<title>Item.Without.Guid</title>
<link>https://fake-feed.com/download/2</link>
</item>
<item>
<title>Item.With.Only.Title</title>
</item>
</channel>
</rss>"#;
    let (url, seen) = serve(move |_| (200, "application/rss+xml", response.into())).await;
    let mut f = mock_feed("RSS", &url);
    f.cookie = "uid=1; pass=secret".into();
    f.user_agent = "autobrr-test".into();
    let mut src = RssSource::new(Arc::new(f));
    let res = src.fetch(0).await.unwrap();
    let got = seen.lock().unwrap()[0].clone();
    assert_eq!(got.headers.get("cookie").map(String::as_str), Some("uid=1; pass=secret"));
    assert_eq!(got.headers.get("user-agent").map(String::as_str), Some("autobrr-test"));
    assert!(res.raw.contains("Item.With.Guid"));
    let keys: Vec<&str> = res.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["guid-1", "https://fake-feed.com/download/2", "Item.With.Only.Title"], "guid, then link, then title");
    assert_eq!(res.entries[0].pub_date.unwrap().to_rfc3339(), "2026-10-05T10:00:00+00:00");
    assert!(res.entries[1].pub_date.is_none(), "missing pub date stays unknown so max age skips it");
    let r = (res.entries[0].release)();
    assert_eq!(
        (r.implementation.as_str(), r.download_url.as_str(), r.raw_cookie.as_str()),
        ("RSS", "https://fake-feed.com/download/1", "uid=1; pass=secret")
    );
}

/// Adds a feed indexer (`torznab`, `newznab` or `rss`) and its feed, off, as autobrr's web app does; the feed.
async fn add_feed(app: &Router, implementation: &str, name: &str, url: &str) -> Value {
    let (s, ix) = call(app, "POST", "/indexer", Some(json!({ "name": name, "identifier": implementation, "implementation": implementation, "enabled": true, "settings": { "url": url, "api_key": "secret-key" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ix}");
    let body = json!({
        "name": name, "indexer_id": ix["id"], "type": implementation.to_uppercase(), "enabled": false, "url": url,
        "api_key": "secret-key", "interval": 30, "timeout": 60, "max_age": 0, "categories": [2000, 5040],
        "settings": { "download_type": "TORRENT", "cache_ttl_days": 0, "max_pages": 0, "process_order": "" }
    });
    let (s, f) = call(app, "POST", "/feeds", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    f
}

// internal/http/feed.go and internal/feed/service.go through the API
#[tokio::test]
async fn feeds_are_added_changed_run_and_removed() {
    let (app, st) = lantern("crud").await;
    let (url, seen) = serve(|r| match r.q("t") {
        "caps" => (200, "application/xml", fixture("caps_response.xml")),
        _ => (200, "application/xml", fixture("torznab_response.xml")),
    })
    .await;
    let f = add_feed(&app, "torznab", "Mock", &url).await;
    assert_shape("Feed", &f);
    assert_eq!(f["api_key"], "<redacted>");
    let id = f["id"].as_i64().unwrap();
    let ix = f["indexer_id"].as_i64().unwrap();

    // the list: inner joined on the indexer, no indexer_id, no job yet
    let (s, list) = call(&app, "GET", "/feeds", None).await;
    assert_eq!(s, StatusCode::OK);
    let one = &list[0];
    assert_shape("Feed", one);
    assert_shape("FeedSettingsJSON", &one["settings"]);
    assert!(one.get("indexer_id").is_none());
    assert_eq!(one["indexer"]["identifier"], "torznab-mock");
    assert_eq!(one["indexer"]["name"], "Mock");
    assert_eq!(one["next_run"], "0001-01-01T00:00:00Z");
    assert_eq!(one["last_run"], "0001-01-01T00:00:00Z");
    assert_eq!(one["categories"], json!([2000, 5040]));
    // autobrr's Store keeps no max age or cookie, and a null capability reads back as an empty one
    assert_eq!(one["capabilities"]["limits"], json!({ "max": 0, "default": 0 }));
    let (s, by_id) = call(&app, "GET", &format!("/feeds/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(by_id["name"], "Mock");

    // errors in autobrr's words
    let (s, v) = call(&app, "GET", "/feeds/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "feedID parameter is invalid");
    let (s, v) = call(&app, "GET", "/feeds/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find feed with id 999");
    let (s, v) = call(&app, "GET", &format!("/feeds/{id}/latest"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("no last run data found for feed with id {id}"));
    let (s, v) =
        call(&app, "POST", "/feeds", Some(json!({ "name": "x", "indexer_id": ix, "type": "TORZNAB", "user_agent": "a\nb" }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "user agent must not contain control characters");
    let (s, v) =
        call(&app, "POST", "/feeds", Some(json!({ "name": "x", "indexer_id": ix, "settings": { "process_order": "RANDOM" } }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "invalid process order: RANDOM");
    let (s, _) = call(&app, "POST", "/feeds", Some(json!({ "name": "x", "indexer_id": 999 }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "the indexer must be there");

    // turned on: a job; the list says when it runs next
    let (s, _) = call(&app, "PATCH", &format!("/feeds/{id}/enabled"), Some(json!({ "enabled": true }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(st.releases.scheduler.has(&format!("feed-{id}")));
    let (_, list) = call(&app, "GET", "/feeds", None).await;
    assert_ne!(list[0]["next_run"], "0001-01-01T00:00:00Z");

    // a filter watches the indexer: a forced run reads the feed and hands its releases to the filters
    let (s, flt) = call(&app, "POST", "/filters", Some(json!({ "name": "watch", "enabled": true, "indexers": [{ "id": ix }] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{flt}");
    let (s, v) = call(&app, "POST", &format!("/feeds/{id}/forcerun"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    let search = seen.lock().unwrap().iter().find(|s| s.q("t") == "search").cloned().unwrap();
    assert_eq!((search.q("cat"), search.q("apikey")), ("2000,5040", "secret-key"));
    let logged = st.releases.log(100, None).unwrap();
    assert_eq!(logged.len(), 2, "the fixture's two items (one guid twice) reached the filters: {logged:?}");
    let (s, latest) = call(&app, "GET", &format!("/feeds/{id}/latest"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(latest.as_str().unwrap().contains("<rss"), "{latest}");
    let (_, list) = call(&app, "GET", "/feeds", None).await;
    assert_ne!(list[0]["last_run"], "0001-01-01T00:00:00Z");
    // a second run finds them cached: nothing new
    call(&app, "POST", &format!("/feeds/{id}/forcerun"), None).await;
    assert_eq!(st.releases.log(100, None).unwrap().len(), 2);
    // forgetting the cache: the next run reads them again
    let (s, _) = call(&app, "DELETE", &format!("/feeds/{id}/cache"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    call(&app, "POST", &format!("/feeds/{id}/forcerun"), None).await;
    assert_eq!(st.releases.log(100, None).unwrap().len(), 4);
    let (s, v) = call(&app, "DELETE", "/feeds/999/cache", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find feed with id 999");

    // the caps of a saved feed are read and kept
    let (s, caps) = call(&app, "GET", &format!("/feeds/{id}/caps"), None).await;
    assert_eq!(s, StatusCode::OK, "{caps}");
    assert_shape("FeedCapabilities", &caps);
    assert_shape("FeedCapabilitiesSearching", &caps["searching"]);
    assert_shape("FeedCapsSearch", &caps["searching"]["tv-search"]);
    let (_, by_id) = call(&app, "GET", &format!("/feeds/{id}"), None).await;
    assert_eq!(by_id["capabilities"]["limits"]["max"], 100);
    assert_eq!(by_id["capabilities"]["categories"].as_array().unwrap().len(), 2);

    // changed with the redacted key: the saved key stays; max age and cookie are kept now
    let mut upd = by_id.clone();
    upd["name"] = json!("Mock 2");
    upd["max_age"] = json!(3600);
    upd["cookie"] = json!("c=1");
    let (s, v) = call(&app, "PUT", &format!("/feeds/{id}"), Some(upd)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["api_key"].as_str(), v["cookie"].as_str()), (Some("<redacted>"), Some("<redacted>")));
    let saved = st.releases.feed_by_id(id).unwrap();
    assert_eq!((saved.name.as_str(), saved.api_key.as_str(), saved.cookie.as_str(), saved.max_age), ("Mock 2", "secret-key", "c=1", 3600));
    let (s, v) = call(&app, "PUT", "/feeds/999", Some(json!({ "id": 999, "name": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find feed with id 999");

    // the indexer turned off stops the job; on again starts it
    let (s, _) = call(&app, "PATCH", &format!("/indexer/{ix}/enabled"), Some(json!({ "enabled": false }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(!st.releases.scheduler.has(&format!("feed-{id}")));
    call(&app, "PATCH", &format!("/indexer/{ix}/enabled"), Some(json!({ "enabled": true }))).await;
    assert!(st.releases.scheduler.has(&format!("feed-{id}")));
    // the feed turned off stops it too
    call(&app, "PATCH", &format!("/feeds/{id}/enabled"), Some(json!({ "enabled": false }))).await;
    assert!(!st.releases.scheduler.has(&format!("feed-{id}")));

    // removed
    let (s, _) = call(&app, "DELETE", &format!("/feeds/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = call(&app, "DELETE", &format!("/feeds/{id}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", &format!("could not find feed with id {id}"));

    // a feed goes with its indexer
    let f = add_feed(&app, "rss", "Gone", &url).await;
    let fid = f["id"].as_i64().unwrap();
    call(&app, "PATCH", &format!("/feeds/{fid}/enabled"), Some(json!({ "enabled": true }))).await;
    assert!(st.releases.scheduler.has(&format!("feed-{fid}")));
    let (s, _) = call(&app, "DELETE", &format!("/indexer/{}", f["indexer_id"]), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&app, "GET", &format!("/feeds/{fid}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(!st.releases.scheduler.has(&format!("feed-{fid}")));
}

// internal/feed service_test.go: TestServiceTestTimeout, TestServiceTestSendsRefreshRequest, TestServiceFetchCaps
#[tokio::test]
async fn feed_tests_and_caps_go_through_the_feeds_own_client() {
    let (app, _st) = lantern("test").await;
    for kind in ["torznab", "newznab"] {
        let (url, seen) = serve(|r| match r.q("t") {
            "caps" => (200, "application/xml", "<caps/>".into()),
            _ => (200, "application/xml", "<rss><channel></channel></rss>".into()),
        })
        .await;
        let f = add_feed(&app, kind, &format!("T {kind}"), &url).await;
        let (s, v) = call(&app, "POST", "/feeds/test", Some(f.clone())).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
        let search =
            seen.lock().unwrap().iter().find(|s| s.q("t") == "search").cloned().expect("test must send the search a refresh sends");
        assert_eq!((search.q("t"), search.q("cat")), ("search", "2000,5040"));
        assert_eq!(search.q("apikey"), "secret-key", "the redacted key is the saved one");

        // the feed's timeout ends a hanging test
        let (slow, _) = serve(|r| match r.q("t") {
            "caps" => (200, "application/xml", "<caps/>".into()),
            _ => (200, "application/xml", EMPTY_RSS.into()),
        })
        .await;
        let mut t = f.clone();
        t["url"] = json!(format!("{slow}/slow"));
        t["timeout"] = json!(1);
        let started = Instant::now();
        let (s, v) = call(&app, "POST", "/feeds/test", Some(t)).await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(v["message"].as_str().unwrap().contains("deadline exceeded"), "{v}");
        assert!(started.elapsed() < Duration::from_secs(4), "the feed timeout must expire first");
    }
    let (s, v) = call(&app, "POST", "/feeds/test", Some(json!({ "id": 999, "type": "TORZNAB" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find feed with id 999");

    // caps of a feed as sent
    let (url, _) = serve(|_| (200, "application/xml", fixture("caps_response.xml"))).await;
    for kind in ["TORZNAB", "NEWZNAB"] {
        let (s, caps) = call(&app, "POST", "/feeds/caps", Some(json!({ "type": kind, "url": url }))).await;
        assert_eq!(s, StatusCode::OK, "{caps}");
        assert_eq!(caps["limits"]["max"], 100);
        assert_eq!(caps["categories"].as_array().unwrap().len(), 2);
        assert!(caps["categories"][0]["subcategories"].is_array());
    }
    let (s, v) = call(&app, "POST", "/feeds/caps", Some(json!({ "type": "RSS", "url": url }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "feed type RSS has no capabilities");
    let (_, v) = call(&app, "POST", "/feeds/caps", Some(json!({ "type": "TORZNAB" }))).await;
    assert_error(&v, "INTERNAL_SERVER_ERROR", "feed URL is required");
    let (_, v) = call(&app, "POST", "/feeds/caps", Some(Value::Null)).await;
    assert_error(&v, "INTERNAL_SERVER_ERROR", "feed is required");
    let (_, v) = call(&app, "POST", "/feeds/caps", Some(json!({ "type": "JSON", "url": url }))).await;
    assert_error(&v, "INTERNAL_SERVER_ERROR", "unsupported feed type: JSON");
}

// internal/list and internal/http/list.go, webhook.go through the API
#[tokio::test]
async fn lists_fill_their_filters() {
    let (app, st) = lantern("lists").await;
    let (s, flt) = call(&app, "POST", "/filters", Some(json!({ "name": "TV", "enabled": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{flt}");
    let fid = flt["id"].as_i64().unwrap();
    let (url, seen) = serve(|r| match r.path.as_str() {
        "/list.txt" => (200, "text/plain; charset=utf-8", "The Office (US)\n\nMonsters, Inc.\n".into()),
        "/html" => (200, "text/html", "<html></html>".into()),
        "/api/v3/movie" => (
            200,
            "application/json",
            json!([
                { "title": "Dune", "originalTitle": "Dune", "monitored": true, "tags": [1] },
                { "title": "Skipped", "monitored": false, "tags": [] }
            ])
            .to_string(),
        ),
        "/api/v3/tag" => (200, "application/json", json!([{ "id": 1, "label": "want" }]).to_string()),
        _ => (404, "text/plain", String::new()),
    })
    .await;

    // validation in autobrr's words
    for (body, msg) in [
        (json!({ "type": "PLAINTEXT" }), "name is required"),
        (json!({ "name": "x" }), "type is required"),
        (json!({ "name": "x", "type": "IMDB" }), "invalid list type: IMDB"),
        (json!({ "name": "x", "type": "RADARR" }), "arr client id is required"),
        (json!({ "name": "x", "type": "PLAINTEXT" }), "list url is required"),
        (json!({ "name": "x", "type": "PLAINTEXT", "url": "http://x" }), "at least one filter is required"),
    ] {
        let (s, v) = call(&app, "POST", "/lists", Some(body)).await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
        assert_error(&v, "INTERNAL_SERVER_ERROR", msg);
    }

    // a plain text list, on: refreshed at once into the filter's shows
    let body = json!({
        "name": "Shows", "type": "PLAINTEXT", "enabled": true, "url": format!("{url}/list.txt"), "headers": ["X-Token=abc", "bad"],
        "filters": [{ "id": fid, "name": "TV" }], "match_release": false, "tags_included": [], "tags_excluded": []
    });
    let (s, l) = call(&app, "POST", "/lists", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{l}");
    assert_shape("List", &l);
    let lid = l["id"].as_i64().unwrap();
    let got = seen.lock().unwrap().iter().find(|s| s.path == "/list.txt").cloned().unwrap();
    assert_eq!(got.headers.get("x-token").map(String::as_str), Some("abc"));
    let (_, f) = call(&app, "GET", &format!("/filters/{fid}"), None).await;
    assert_eq!(f["shows"], "The?Office,The?Office*US,The?Office*US?,Monsters*Inc,Monsters*Inc?");
    let (_, all) = call(&app, "GET", "/filters", None).await;
    assert_eq!(all[0]["is_auto_updated"], true, "{all}");

    let (s, lists) = call(&app, "GET", "/lists", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("List", &lists[0]);
    assert_shape("ListFilter", &lists[0]["filters"][0]);
    assert_eq!(lists[0]["last_refresh_status"], "SUCCESS");
    assert_ne!(lists[0]["last_refresh_time"], "0001-01-01T00:00:00Z");
    assert_eq!(lists[0]["headers"], json!(["X-Token=abc", "bad"]));

    // a list that fails says why, and is marked so
    let mut bad = lists[0].clone();
    bad["url"] = json!(format!("{url}/html"));
    let (s, v) = call(&app, "PUT", &format!("/lists/{lid}"), Some(bad)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    let why = format!("could not make new request for URL: unexpected content type for URL: {url}/html expected text/plain got text/html");
    assert_error(&v, "INTERNAL_SERVER_ERROR", &why);
    let (_, lists) = call(&app, "GET", "/lists", None).await;
    assert_eq!((lists[0]["last_refresh_status"].as_str(), lists[0]["last_refresh_error"].as_str()), (Some("ERROR"), Some(why.as_str())));

    // a Radarr list fills match_releases when it matches releases, with the included tags only
    let (s, d) = call(
        &app,
        "POST",
        "/downloaders",
        Some(json!({ "name": "radarr", "type": "RADARR", "enabled": true, "host": url, "settings": { "apikey": "rk" } })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{d}");
    let body = json!({ "name": "Movies", "type": "RADARR", "enabled": false, "client_id": d["id"], "match_release": true, "tags_included": ["want"], "filters": [{ "id": fid }] });
    let (s, m) = call(&app, "POST", "/lists", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{m}");
    let mid = m["id"].as_i64().unwrap();
    let (s, v) = call(&app, "POST", &format!("/lists/{mid}/refresh"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    let movie = seen.lock().unwrap().iter().find(|s| s.path == "/api/v3/movie").cloned().unwrap();
    assert_eq!(movie.headers.get("x-api-key").map(String::as_str), Some("rk"));
    let (_, f) = call(&app, "GET", &format!("/filters/{fid}"), None).await;
    assert_eq!((f["shows"].as_str(), f["match_releases"].as_str()), (Some(""), Some("*Dune*")));

    // a Sonarr list on a Radarr client is refused before anything is read
    let mut wrong = m.clone();
    wrong["type"] = json!("SONARR");
    wrong["enabled"] = json!(true);
    let (s, v) = call(&app, "PUT", &format!("/lists/{mid}"), Some(wrong)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", &format!("download client {} (RADARR) has runtime type *radarr.Client", d["id"]));

    // the webhooks refresh in the background, or by kind
    let before = seen.lock().unwrap().len();
    let (s, _) = call(&app, "GET", "/webhook/lists/trigger", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    until("the background refresh", || seen.lock().unwrap().len() > before).await;
    // autobrr saved the Sonarr change before its refresh failed, so the *arr lists fail on it now
    let (s, v) = call(&app, "POST", "/webhook/lists/trigger/arr", None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{v}");
    assert!(v["message"].as_str().unwrap().starts_with("error while refreshing Movies: download client"), "{v}");
    let (s, v) = call(&app, "POST", "/webhook/lists/trigger/lists", None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "the failing plain text list: {v}");
    assert!(v["message"].as_str().unwrap().starts_with("error while refreshing Shows: "), "{v}");

    // refreshing every list waits and names each list that failed, one per line
    let (s, v) = call(&app, "POST", "/lists/refresh", None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{v}");
    let msg = v["message"].as_str().unwrap();
    assert!(msg.contains("error while refreshing Movies: ") && msg.contains("\nerror while refreshing Shows: "), "{v}");

    // the other webhook routes answer the same way
    let (s, _) = call(&app, "POST", "/webhook/lists/trigger", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&app, "GET", "/webhook/lists/trigger/arr", None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    let (s, _) = call(&app, "GET", "/webhook/lists/trigger/lists", None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    let (s, v) = call(&app, "POST", &format!("/webhook/lists/trigger/{lid}"), None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", &why);
    let (s, v) = call(&app, "POST", "/webhook/lists/trigger/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "listID parameter is invalid");

    // removed, and gone from the filter's auto-updated mark when every list is
    let (s, v) = call(&app, "DELETE", "/lists/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find list");
    let (s, v) = call(&app, "POST", "/lists/999/refresh", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find list");
    for id in [lid, mid] {
        let (s, _) = call(&app, "DELETE", &format!("/lists/{id}"), None).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }
    let (_, all) = call(&app, "GET", "/filters", None).await;
    assert_eq!(all[0]["is_auto_updated"], false);
    assert!(st.releases.lists(false).unwrap().is_empty());
}

// internal/list: every list type read from a stand-in app or list into the fields autobrr's fills
#[tokio::test]
async fn every_list_type_fills_its_fields() {
    let (app, _st) = lantern("types").await;
    let (url, seen) = serve(|r| {
        let j = |v: Value| (200, "application/json", v.to_string());
        match r.path.as_str() {
            "/api/v3/series" => j(json!([{ "title": "Lost", "monitored": true, "alternateTitles": [{ "title": "Lost (US)" }] }])),
            "/api/v3/movie" => j(json!([{ "title": "Dune", "monitored": true }])),
            "/api/v3/system/status" => j(json!({ "version": "" })),
            "/api/v1/album" => j(json!([{ "title": "Live!", "monitored": true, "artistId": 1 }])),
            "/api/v1/artist/1" => j(json!({ "artistName": "Pulp", "monitored": true })),
            "/api/v1/book" => j(json!([{ "title": "Dune Messiah", "monitored": true }])),
            "/api/leagues" => j(json!([{ "name": "Premier League", "monitored": true }])),
            "/mdblist" => j(json!([{ "title": "Heat", "release_year": 1995, "mediatype": "movie" }])),
            "/trakt" => j(json!([{ "movie": { "title": "Alien" } }])),
            "/metacritic" => j(json!({ "title": "x", "albums": [{ "artist": "Pulp", "title": "Live!" }] })),
            "/steam" => j(json!({ "1": { "name": "Hades II" } })),
            "/anilist" => j(json!([{ "english": "Frieren", "romaji": "Sousou no Frieren", "synonyms": [] }])),
            _ => (404, "text/plain", String::new()),
        }
    })
    .await;
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        ("SONARR", json!({ "include_alternate_titles": true }), "shows", "Lost,Lost*US,Lost*US?"),
        ("WHISPARR", json!({ "include_alternate_titles": true }), "shows", "Lost"),
        ("WHISPARR_V3", json!({}), "shows", "Dune"),
        ("LIDARR", json!({}), "albums", "Live,Live,Live?"),
        ("READARR", json!({}), "match_releases", "Dune?Messiah"),
        ("SPORTARR", json!({}), "shows", "Premier?League"),
        (
            "MDBLIST",
            json!({ "url": format!("{url}/mdblist"), "match_release": true, "include_year": true }),
            "match_releases",
            "*Heat?1995*",
        ),
        ("TRAKT", json!({ "url": format!("{url}/trakt"), "api_key": "tk" }), "shows", "Alien"),
        ("METACRITIC", json!({ "url": format!("{url}/metacritic") }), "match_releases", "*Pulp*Live*"),
        ("STEAM", json!({ "url": format!("{url}/steam") }), "match_releases", "Hades?II"),
        ("ANILIST", json!({ "url": format!("{url}/anilist") }), "shows", "Frieren,Sousou?no?Frieren"),
    ];
    for (kind, extra, field, want) in cases {
        let (_, flt) = call(&app, "POST", "/filters", Some(json!({ "name": kind, "enabled": true }))).await;
        let mut body = json!({ "name": kind, "type": kind, "enabled": true, "filters": [{ "id": flt["id"] }] });
        if !extra.get("url").is_some_and(Value::is_string) {
            let client = json!({ "name": kind, "type": kind, "enabled": true, "host": url, "settings": { "apikey": "k" } });
            let (s, d) = call(&app, "POST", "/downloaders", Some(client)).await;
            assert_eq!(s, StatusCode::CREATED, "{d}");
            body["client_id"] = d["id"].clone();
        }
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (s, v) = call(&app, "POST", "/lists", Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{kind}: {v}");
        let (_, f) = call(&app, "GET", &format!("/filters/{}", flt["id"]), None).await;
        assert_eq!(f[field], want, "{kind}: {f}");
        if kind == "LIDARR" {
            assert_eq!(f["artists"], "Pulp");
        }
        if kind == "METACRITIC" {
            assert_eq!((f["albums"].as_str(), f["artists"].as_str()), (Some(""), Some("")));
        }
    }
    let trakt = seen.lock().unwrap().iter().find(|s| s.path == "/trakt").cloned().unwrap();
    assert_eq!(trakt.headers.get("trakt-api-version").map(String::as_str), Some("2"));
    assert_eq!(trakt.headers.get("trakt-api-key").map(String::as_str), Some("tk"));
}

// internal/feed/service.go and internal/list/service.go: the jobs autobrr starts, and the daily cache cleanup
#[tokio::test]
async fn the_cleanup_and_list_jobs_are_scheduled_and_the_cleanup_drops_old_items() {
    use kit_server_core::{p, Db, RowExt};
    let (app, st) = lantern("jobs").await;
    st.releases.start_feeds();
    st.releases.start_lists();
    let now = chrono::Utc::now();
    let cleanup = st.releases.scheduler.next_run("feed-cache-cleanup").expect("the cleanup is scheduled");
    assert!(cleanup > now && cleanup <= now + chrono::Duration::days(1));
    let lists = st.releases.scheduler.next_run("lists-updater").expect("the list refresh is scheduled");
    assert!(lists > now && lists <= now + chrono::Duration::hours(6));

    let f = add_feed(&app, "torznab", "Cleanup", "http://127.0.0.1:9/").await;
    let fid = f["id"].as_i64().unwrap();
    let past = (now - chrono::Duration::days(1)).timestamp_millis();
    let later = (now + chrono::Duration::days(1)).timestamp_millis();
    for (feed, key, ttl) in [(fid, "old", past), (fid, "kept", later), (fid + 100, "orphan", later)] {
        st.db.execute("INSERT INTO autobrr_feed_cache (feed_id, key, value, ttl) VALUES (?,?,?,?)", p![feed, key, key, ttl]).unwrap();
    }
    st.releases.clean_feed_cache().unwrap();
    let rows = st.db.query("SELECT key FROM autobrr_feed_cache ORDER BY key", p![]).unwrap();
    assert_eq!(rows.iter().map(|r| r.s("key")).collect::<Vec<_>>(), ["kept"]);
}
