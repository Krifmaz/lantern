//! Parity with Prowlarr v2.7.0.5723's indexer proxies, indexer links and Newznab/Torznab capabilities:
//!
//! - `/api/v1/indexerproxy*` answers as `IndexerProxyController` does (fixtures in
//!   tests/fixtures/upstream/prowlarr/, built from its resource classes and validators);
//! - indexers that share a tag with an HTTP, SOCKS4 or SOCKS5 proxy go through it, others don't
//!   (`IndexerHttpClient.GetProxies`);
//! - a FlareSolverr proxy is asked only when a site shows a Cloudflare check, with Prowlarr's command, and its
//!   cookies and user agent are used from then on (`FlareSolverr.PreRequest`/`PostResponse`);
//! - a Newznab/Torznab API is asked for its own capabilities once (`NewznabCapabilitiesProvider`);
//! - an indexer's tags, app profile and download client are kept and answered in Prowlarr's shape.

use axum::body::Body;
use axum::extract::{Query as Q, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use server::indexers::release::Query;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

#[path = "support/challenge.rs"]
mod challenge;

const KEY: &str = "test-key";

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/prowlarr").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn fixture_json(name: &str) -> Value {
    serde_json::from_str(&fixture(name)).unwrap()
}

async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-ixproxy-{name}-{}", std::process::id()));
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
    (server::app(st.clone()), st)
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value, HeaderMap) {
    let mut r = Request::builder().method(method).uri(path).header("X-Api-Key", KEY);
    if body.is_some() {
        r = r.header(header::CONTENT_TYPE, "application/json");
    }
    let req = r.body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (s, h) = (resp.status(), resp.headers().clone());
    let b = axum::body::to_bytes(resp.into_body(), 1 << 24).await.unwrap();
    let v = serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into_owned()));
    (s, v, h)
}

/// The fields of a proxy of `kind` from the schema, with `values` set.
fn proxy_body(name: &str, kind: &str, values: &[(&str, Value)], tags: &[i64]) -> Value {
    let schema = fixture_json("indexerproxy_schema.json");
    let mut p = schema.as_array().unwrap().iter().find(|p| p["implementation"] == kind).unwrap().clone();
    for f in p["fields"].as_array_mut().unwrap() {
        if let Some((_, v)) = values.iter().find(|(k, _)| f["name"] == *k) {
            f["value"] = v.clone();
        }
    }
    p["name"] = json!(name);
    p["tags"] = json!(tags);
    p.as_object_mut().unwrap().remove("presets");
    p
}

async fn tag(app: &Router, label: &str) -> i64 {
    let (s, v, _) = call(app, "POST", "/api/v1/tag", Some(json!({ "label": label }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_i64().unwrap()
}

// ---- a Torznab site, a Cloudflare-checked one, FlareSolverr, SOCKS proxies ----

/// What a test site counts.
#[derive(Default)]
struct Hits {
    /// `t=caps` requests.
    caps: AtomicUsize,
    /// Searches.
    searches: AtomicUsize,
    /// Cloudflare check pages sent.
    checks: AtomicUsize,
    /// Pings (proxy tests).
    pings: AtomicUsize,
    /// When set, the site shows a Cloudflare check to requests without FlareSolverr's cookie and user agent.
    cloudflare: AtomicBool,
}

const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>
<item><title>Show.S01E01.1080p.WEB-DL-GRP</title><guid>https://site.test/details/1</guid><link>https://site.test/dl/1.torrent</link>
<pubDate>Wed, 04 Nov 2026 10:00:00 +0000</pubDate><size>1000</size><torznab:attr name="category" value="5040"/><torznab:attr name="seeders" value="5"/></item>
</channel></rss>"#;

async fn torznab(State(hits): State<Arc<Hits>>, headers: HeaderMap, Q(q): Q<HashMap<String, String>>, uri: axum::http::Uri) -> Response {
    if uri.path() == "/ping" {
        hits.pings.fetch_add(1, Ordering::SeqCst);
        return (StatusCode::OK, "pong").into_response();
    }
    if hits.cloudflare.load(Ordering::SeqCst) {
        let cookie = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("");
        let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
        if !cookie.contains("cf_clearance=solved") || ua != "FlareSolverr-UA" {
            hits.checks.fetch_add(1, Ordering::SeqCst);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::SERVER, "cloudflare"), (header::CONTENT_TYPE, "text/html")],
                "<html><head><title>Just a moment...</title></head><body>checking</body></html>",
            )
                .into_response();
        }
    }
    let xml = [(header::CONTENT_TYPE, "application/rss+xml")];
    if q.get("t").map(String::as_str) == Some("caps") {
        hits.caps.fetch_add(1, Ordering::SeqCst);
        return (xml, fixture("newznab_caps.xml")).into_response();
    }
    hits.searches.fetch_add(1, Ordering::SeqCst);
    (xml, RSS).into_response()
}

/// Starts the Torznab site: its address and its counts.
async fn start_torznab() -> (String, Arc<Hits>) {
    let hits = Arc::new(Hits::default());
    let app = Router::new().fallback(any(torznab)).with_state(hits.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), hits)
}

/// A FlareSolverr stand-in: the commands it got, and whether it answers without cookies.
#[derive(Default)]
struct Flare {
    commands: Mutex<Vec<Value>>,
    no_cookies: AtomicBool,
}

async fn flare_v1(State(f): State<Arc<Flare>>, Json(cmd): Json<Value>) -> Response {
    f.commands.lock().unwrap().push(cmd.clone());
    if f.no_cookies.load(Ordering::SeqCst) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "status": "error", "message": "Error: Timeout", "version": "3.5.2" })))
            .into_response();
    }
    Json(json!({
        "status": "ok", "message": "Challenge solved!", "version": "3.5.2", "startTimestamp": 1, "endTimestamp": 2,
        "solution": {
            "url": cmd["url"], "status": 200, "response": "<html></html>", "userAgent": "FlareSolverr-UA",
            "cookies": [{ "name": "cf_clearance", "value": "solved", "domain": "127.0.0.1", "path": "/" }],
        },
    }))
    .into_response()
}

async fn start_flaresolverr() -> (String, Arc<Flare>) {
    let f = Arc::new(Flare::default());
    let app = Router::new().route("/v1", post(flare_v1)).with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}/"), f)
}

/// What a SOCKS proxy saw: the destinations asked for, and the sign-ins.
#[derive(Default)]
struct Socks {
    destinations: Mutex<Vec<String>>,
    users: Mutex<Vec<String>>,
}

/// Starts a SOCKS4/SOCKS5 proxy that sends every connection to `site` (whatever was asked for).
async fn start_socks(site: String) -> (String, Arc<Socks>) {
    let seen = Arc::new(Socks::default());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let target = site.trim_start_matches("http://").to_string();
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else { return };
            let (seen, target) = (s2.clone(), target.clone());
            tokio::spawn(async move {
                let mut v = [0u8; 1];
                c.read_exact(&mut v).await.ok()?;
                if v[0] == 5 {
                    let mut n = [0u8; 1];
                    c.read_exact(&mut n).await.ok()?;
                    let mut methods = vec![0u8; n[0] as usize];
                    c.read_exact(&mut methods).await.ok()?;
                    if methods.contains(&2) {
                        c.write_all(&[5, 2]).await.ok()?;
                        let mut h = [0u8; 2];
                        c.read_exact(&mut h).await.ok()?;
                        let mut user = vec![0u8; h[1] as usize];
                        c.read_exact(&mut user).await.ok()?;
                        let mut pl = [0u8; 1];
                        c.read_exact(&mut pl).await.ok()?;
                        let mut pass = vec![0u8; pl[0] as usize];
                        c.read_exact(&mut pass).await.ok()?;
                        seen.users.lock().unwrap().push(format!("{}:{}", String::from_utf8_lossy(&user), String::from_utf8_lossy(&pass)));
                        c.write_all(&[1, 0]).await.ok()?;
                    } else {
                        c.write_all(&[5, 0]).await.ok()?;
                    }
                    let mut req = [0u8; 4];
                    c.read_exact(&mut req).await.ok()?;
                    let dest = match req[3] {
                        1 => {
                            let mut a = [0u8; 4];
                            c.read_exact(&mut a).await.ok()?;
                            std::net::Ipv4Addr::from(a).to_string()
                        }
                        3 => {
                            let mut l = [0u8; 1];
                            c.read_exact(&mut l).await.ok()?;
                            let mut d = vec![0u8; l[0] as usize];
                            c.read_exact(&mut d).await.ok()?;
                            String::from_utf8_lossy(&d).into_owned()
                        }
                        _ => return None,
                    };
                    let mut port = [0u8; 2];
                    c.read_exact(&mut port).await.ok()?;
                    seen.destinations.lock().unwrap().push(format!("socks5 {dest}:{}", u16::from_be_bytes(port)));
                    c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok()?;
                } else if v[0] == 4 {
                    let mut head = [0u8; 7];
                    c.read_exact(&mut head).await.ok()?;
                    let port = u16::from_be_bytes([head[1], head[2]]);
                    let ip = std::net::Ipv4Addr::new(head[3], head[4], head[5], head[6]);
                    let mut user = Vec::new();
                    loop {
                        let mut b = [0u8; 1];
                        c.read_exact(&mut b).await.ok()?;
                        if b[0] == 0 {
                            break;
                        }
                        user.push(b[0]);
                    }
                    seen.users.lock().unwrap().push(String::from_utf8_lossy(&user).into_owned());
                    seen.destinations.lock().unwrap().push(format!("socks4 {ip}:{port}"));
                    c.write_all(&[0, 0x5a, 0, 0, 0, 0, 0, 0]).await.ok()?;
                } else {
                    return None;
                }
                let mut up = tokio::net::TcpStream::connect(&target).await.ok()?;
                tokio::io::copy_bidirectional(&mut c, &mut up).await.ok()?;
                Some(())
            });
        }
    });
    (format!("127.0.0.1:{}", addr.port()), seen)
}

fn search() -> Query {
    Query { keywords: "Show".into(), ..Query::default() }
}

// ---- the API ----

#[tokio::test]
async fn indexer_proxy_api_answers_like_prowlarr() {
    let (app, st) = lantern("api").await;
    let (site, hits) = start_torznab().await;
    st.indexers.set_proxy_ping_url(&format!("{site}/ping"));

    // the schema: the four kinds, by name, ready to fill in
    let (s, schema, _) = call(&app, "GET", "/api/v1/indexerproxy/schema", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(schema, fixture_json("indexerproxy_schema.json"));

    // without a tag a proxy is off: saved without checking or trying it
    let body = proxy_body(
        "Office proxy",
        "Http",
        &[("host", json!("proxy.lan")), ("port", json!(3128)), ("username", json!("me")), ("password", json!("secret"))],
        &[],
    );
    let (s, saved, h) = call(&app, "POST", "/api/v1/indexerproxy", Some(body.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{saved}");
    assert_eq!(h.get(header::LOCATION).unwrap(), "/api/v1/indexerproxy/1");
    assert_eq!(saved, fixture_json("indexerproxy.json"));
    assert_eq!(st.indexers.find_proxy(1).unwrap().unwrap().settings.password, "secret");
    let (s, one, _) = call(&app, "GET", "/api/v1/indexerproxy/1", None).await;
    assert_eq!((s, one), (StatusCode::OK, fixture_json("indexerproxy.json")));

    // the resource's checks: a name, one no other proxy has
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy", Some(body.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!((v[0]["propertyName"].as_str(), v[0]["errorMessage"].as_str()), (Some("Name"), Some("Should be unique")));
    let mut nameless = body.clone();
    nameless["name"] = json!("");
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy", Some(nameless)).await;
    assert_eq!((s, v[0]["errorMessage"].as_str()), (StatusCode::BAD_REQUEST, Some("'Name' must not be empty.")));

    // with a tag, its settings are checked, then it is tried
    let t = tag(&app, "proxied").await;
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy", Some(proxy_body("Empty", "Http", &[("host", json!(""))], &[t]))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, fixture_json("indexerproxy_invalid.json")));
    let (s, v, _) = call(
        &app,
        "POST",
        "/api/v1/indexerproxy",
        Some(proxy_body("Gone", "Socks5", &[("host", json!("127.0.0.1")), ("port", json!(9))], &[t])),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let msg = v[0]["errorMessage"].as_str().unwrap();
    assert!(
        msg.starts_with("Unable to connect to proxy: ") && msg.ends_with(". Check the log surrounding this error for details"),
        "{msg}"
    );
    assert_eq!(v[0]["propertyName"], "Host");
    let (s, v, _) =
        call(&app, "POST", "/api/v1/indexerproxy", Some(proxy_body("Flare", "FlareSolverr", &[("requestTimeout", json!(0))], &[t]))).await;
    assert_eq!(
        (s, v[0]["errorMessage"].as_str()),
        (StatusCode::BAD_REQUEST, Some("'Request Timeout' must be between 1 and 180. You entered 0."))
    );

    // a SOCKS5 proxy that works is tried through to the ping address, then saved
    let (socks, seen) = start_socks(site.clone()).await;
    let (sh, sp) = socks.split_once(':').unwrap();
    let good = proxy_body("Socks", "Socks5", &[("host", json!(sh)), ("port", json!(sp.parse::<i64>().unwrap()))], &[t]);
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy/test", Some(good.clone())).await;
    assert_eq!((s, v), (StatusCode::OK, json!("{}")));
    assert_eq!(hits.pings.load(Ordering::SeqCst), 1);
    let (s, saved, _) = call(&app, "POST", "/api/v1/indexerproxy", Some(good)).await;
    assert_eq!(s, StatusCode::CREATED, "{saved}");
    assert_eq!(hits.pings.load(Ordering::SeqCst), 2);
    assert!(seen.destinations.lock().unwrap().iter().all(|d| d.starts_with("socks5 127.0.0.1:")));
    let socks_id = saved["id"].as_i64().unwrap();

    // a change keeps the saved password when it comes back masked; forceSave skips the try
    let mut change = saved.clone();
    change["fields"][1]["value"] = json!(9);
    let (s, v, _) = call(&app, "PUT", &format!("/api/v1/indexerproxy/{socks_id}"), Some(change.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v, h) = call(&app, "PUT", &format!("/api/v1/indexerproxy/{socks_id}?forceSave=true"), Some(change)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(h.get(header::LOCATION).unwrap().to_str().unwrap(), format!("/api/v1/indexerproxy/{socks_id}"));
    assert_eq!(v["fields"][1]["value"], 9);
    let mut office = fixture_json("indexerproxy.json");
    office["name"] = json!("Office");
    let (s, v, _) = call(&app, "PUT", "/api/v1/indexerproxy/1", Some(office)).await;
    assert_eq!((s, v["name"].as_str()), (StatusCode::ACCEPTED, Some("Office")));
    assert_eq!(st.indexers.find_proxy(1).unwrap().unwrap().settings.password, "secret");
    let (s, v, _) = call(
        &app,
        "PUT",
        "/api/v1/indexerproxy/0",
        Some(json!({ "name": "x", "implementation": "Http", "configContract": "HttpSettings" })),
    )
    .await;
    assert_eq!((s, v[0]["errorMessage"].as_str()), (StatusCode::BAD_REQUEST, Some("'Id' must be greater than '0'.")));

    // the list, by name; testall tries the ones in use; a missing one is a 404
    let (_, list, _) = call(&app, "GET", "/api/v1/indexerproxy", None).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Office", "Socks"]);
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy/testall", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!((v[0]["id"].as_i64(), v[0]["isValid"].as_bool()), (Some(socks_id), Some(false)));
    let (s, v, _) = call(&app, "GET", "/api/v1/indexerproxy/99", None).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::NOT_FOUND, Some("IndexerProxyDefinition with ID 99 does not exist")));
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy/action/whatever", Some(fixture_json("indexerproxy.json"))).await;
    assert_eq!((s, v), (StatusCode::OK, Value::Null));
    let (s, v, _) = call(&app, "DELETE", &format!("/api/v1/indexerproxy/{socks_id}"), None).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy/testall", None).await;
    assert_eq!((s, v), (StatusCode::OK, json!([])));
}

// ---- proxies on indexer requests ----

#[tokio::test]
async fn indexers_go_through_the_proxies_that_share_their_tags() {
    let (app, st) = lantern("route").await;
    let (site, hits) = start_torznab().await;
    st.indexers.set_proxy_ping_url(&format!("{site}/ping"));
    let t = tag(&app, "proxied").await;

    // an HTTP proxy that wants a sign-in, for the indexers tagged `proxied`
    let (http_proxy, proxy_hits) = challenge::start_proxy(site.clone(), "me", "pa:ss").await;
    let port: i64 = http_proxy.rsplit(':').next().unwrap().parse().unwrap();
    let body = proxy_body(
        "Http",
        "Http",
        &[("host", json!("127.0.0.1")), ("port", json!(port)), ("username", json!("me")), ("password", json!("pa:ss"))],
        &[t],
    );
    let (s, v, _) = call(&app, "POST", "/api/v1/indexerproxy", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // a site only the proxy can reach: the tagged indexer gets there, the other one doesn't
    let tagged = st
        .indexers
        .add(&json!({ "kind": "torznab", "name": "Tagged", "base_url": "http://torznab.invalid", "tags": "proxied", "settings": { "apikey": "k" } }))
        .unwrap();
    let plain = st
        .indexers
        .add(&json!({ "kind": "torznab", "name": "Plain", "base_url": "http://torznab.invalid", "settings": { "apikey": "k" } }))
        .unwrap();
    let (found, failed) = st.indexers.search(&[tagged], &search(), "test").await.unwrap();
    assert_eq!((found.len(), failed.len()), (1, 0), "{failed:?}");
    assert!(proxy_hits.passed.load(Ordering::SeqCst) >= 2, "the caps and the search went through the proxy");
    assert_eq!(proxy_hits.refused.load(Ordering::SeqCst), 0, "with its sign-in");
    let before = proxy_hits.passed.load(Ordering::SeqCst);
    let (found, failed) = st.indexers.search(&[plain], &search(), "test").await.unwrap();
    assert!(found.is_empty() && failed.len() == 1, "{failed:?}");
    assert_eq!(proxy_hits.passed.load(Ordering::SeqCst), before, "an indexer without the tag doesn't use the proxy");

    // SOCKS5 lets the proxy look the name up; SOCKS4 sends the address
    let (socks, seen) = start_socks(site.clone()).await;
    let (_, sp) = socks.split_once(':').unwrap();
    let sp: i64 = sp.parse().unwrap();
    let t5 = tag(&app, "socks5").await;
    let body = proxy_body(
        "S5",
        "Socks5",
        &[("host", json!("127.0.0.1")), ("port", json!(sp)), ("username", json!("u5")), ("password", json!("p5"))],
        &[t5],
    );
    assert_eq!(call(&app, "POST", "/api/v1/indexerproxy", Some(body)).await.0, StatusCode::CREATED);
    let s5 = st.indexers.add(&json!({ "kind": "torznab", "name": "S5", "base_url": "http://socks5.invalid", "tags": "socks5" })).unwrap();
    let (found, failed) = st.indexers.search(&[s5], &search(), "test").await.unwrap();
    assert_eq!((found.len(), failed.len()), (1, 0), "{failed:?}");
    assert!(seen.destinations.lock().unwrap().contains(&"socks5 socks5.invalid:80".to_string()), "{:?}", seen.destinations);
    assert!(seen.users.lock().unwrap().contains(&"u5:p5".to_string()));
    let t4 = tag(&app, "socks4").await;
    let body = proxy_body("S4", "Socks4", &[("host", json!("127.0.0.1")), ("port", json!(sp))], &[t4]);
    assert_eq!(call(&app, "POST", "/api/v1/indexerproxy", Some(body)).await.0, StatusCode::CREATED);
    let s4 = st.indexers.add(&json!({ "kind": "torznab", "name": "S4", "base_url": "http://192.0.2.10:8080", "tags": "socks4" })).unwrap();
    let (found, failed) = st.indexers.search(&[s4], &search(), "test").await.unwrap();
    assert_eq!((found.len(), failed.len()), (1, 0), "{failed:?}");
    assert!(seen.destinations.lock().unwrap().contains(&"socks4 192.0.2.10:8080".to_string()), "{:?}", seen.destinations);
    assert!(hits.searches.load(Ordering::SeqCst) >= 3);
}

#[tokio::test]
async fn flaresolverr_is_asked_only_for_a_cloudflare_check() {
    let (app, st) = lantern("flare").await;
    let (site, hits) = start_torznab().await;
    hits.cloudflare.store(true, Ordering::SeqCst);
    let (flare, f) = start_flaresolverr().await;
    st.indexers.set_proxy_ping_url(&format!("{site}/ping"));
    let t = tag(&app, "cf").await;
    let (s, v, _) =
        call(&app, "POST", "/api/v1/indexerproxy", Some(proxy_body("FlareSolverr", "FlareSolverr", &[("host", json!(flare))], &[t]))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    // the proxy test asked it for the ping address with Prowlarr's command
    assert_eq!(
        f.commands.lock().unwrap()[0],
        json!({ "cmd": "request.get", "url": format!("{site}/ping"), "maxTimeout": 60000, "proxy": {} })
    );

    let id = st
        .indexers
        .add(&json!({ "kind": "torznab", "name": "Checked", "base_url": site, "tags": "cf", "settings": { "apikey": "k" } }))
        .unwrap();
    let (found, failed) = st.indexers.search(&[id], &search(), "test").await.unwrap();
    assert_eq!((found.len(), failed.len()), (1, 0), "{failed:?}");
    // one check, one FlareSolverr command for it; the search after it used the cookies and the user agent
    let cmds = f.commands.lock().unwrap().clone();
    assert_eq!(cmds.len(), 2, "{cmds:?}");
    assert_eq!(cmds[1], json!({ "cmd": "request.get", "url": format!("{site}/api?t=caps&apikey=k"), "maxTimeout": 60000, "proxy": {} }));
    assert_eq!(hits.checks.load(Ordering::SeqCst), 1);
    assert_eq!((hits.caps.load(Ordering::SeqCst), hits.searches.load(Ordering::SeqCst)), (1, 1));

    // no cookies from FlareSolverr: Prowlarr's words
    f.no_cookies.store(true, Ordering::SeqCst);
    let id2 = st
        .indexers
        .add(&json!({ "kind": "torznab", "name": "Checked2", "base_url": site.replace("127.0.0.1", "localhost"), "tags": "cf" }))
        .unwrap();
    let (_, failed) = st.indexers.search(&[id2], &search(), "test").await.unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].1.ends_with("Empty cookies returned by FlareSolverr."), "{}", failed[0].1);

    // a FlareSolverr at Lantern's own port is answered by Lantern's solver, not over the network (nothing listens
    // there in this test, so a network request could only fail to connect)
    let own = format!("http://localhost:{}/", server::solver_port());
    let (s, v, _) =
        call(&app, "POST", "/api/v1/indexerproxy/test", Some(proxy_body("Own", "FlareSolverr", &[("host", json!(own))], &[t]))).await;
    if s != StatusCode::OK {
        let msg = v[0]["errorMessage"].as_str().unwrap_or("");
        assert!(!msg.starts_with("Unable to connect to proxy"), "{msg}");
    }
}

// ---- capabilities, tags, app profile, download client, request delay ----

#[tokio::test]
async fn newznab_capabilities_come_from_the_api_once() {
    let (app, st) = lantern("caps").await;
    let (site, hits) = start_torznab().await;
    let id = st.indexers.add(&json!({ "kind": "torznab", "name": "Caps", "base_url": site, "settings": { "apikey": "k" } })).unwrap();
    let get = |p: String| Request::get(p).header("X-Api-Key", KEY).body(Body::empty()).unwrap();
    for _ in 0..2 {
        let r = app.clone().oneshot(get(format!("/{id}/api?t=caps"))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let b = String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
        assert!(b.contains(r#"<limits default="25" max="60" />"#) || b.contains(r#"max="60""#), "{b}");
        assert!(b.contains(r#"<subcat id="5050" name="TV/Other" />"#) && b.contains(r#"name="TV/Other""#), "{b}");
    }
    // `should_not_request_same_caps_twice`
    assert_eq!(hits.caps.load(Ordering::SeqCst), 1);
    // kept with the indexer: a new process reads it without asking
    assert!(st.indexers.get(id).unwrap().caps_xml.contains("<caps>"));
    let caps = st.indexers.capabilities(&st.indexers.get(id).unwrap());
    assert_eq!((caps.limits_default, caps.limits_max), (Some(25), Some(60)));
    assert!(caps.book_params.is_empty(), "no book-search in its answer");
    // a search by a category it doesn't have doesn't reach it
    let (found, _) = st.indexers.search(&[id], &Query { categories: vec![6000], ..search() }, "test").await.unwrap();
    assert!(found.is_empty());
    assert_eq!(hits.searches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tags_app_profile_and_download_client_are_kept_in_prowlarrs_shape() {
    let (app, st) = lantern("links").await;
    let movies = tag(&app, "movies").await;
    let tv = tag(&app, "tv").await;
    let (site, _) = start_torznab().await;
    let id = st.indexers.add(&json!({ "kind": "torznab", "name": "Linked", "base_url": site, "tags": [movies, tv] })).unwrap();
    let ix = st.indexers.get(id).unwrap();
    assert_eq!((ix.tags.as_str(), ix.app_profile_id, ix.download_client_id), ("movies,tv", 1, 0));
    let (_, v, _) = call(&app, "GET", &format!("/api/v1/indexer/{id}"), None).await;
    assert_eq!((v["tags"].clone(), v["appProfileId"].clone(), v["downloadClientId"].clone()), (json!([movies, tv]), json!(1), json!(0)));
    let e = st.indexers.update(id, &json!({ "downloadClientId": 99 })).unwrap_err();
    assert_eq!((e.status.as_u16(), e.detail.as_str()), (400, "Download Client does not exist"));
    let client = st.hub.add_client(&json!({ "kind": "qbittorrent", "url": "http://qbit.lan:8080" })).unwrap();
    st.indexers.update(id, &json!({ "tags": "tv", "appProfileId": 3, "downloadClientId": client })).unwrap();
    let (_, v, _) = call(&app, "GET", &format!("/api/v1/indexer/{id}"), None).await;
    assert_eq!((v["tags"].clone(), v["appProfileId"].clone(), v["downloadClientId"].clone()), (json!([tv]), json!(3), json!(client)));
    let e = st.indexers.update(id, &json!({ "appProfileId": 0 })).unwrap_err();
    assert_eq!((e.status.as_u16(), e.detail.as_str()), (400, "'App Profile Id' must be greater than '0'."));
    let e = st.indexers.add(&json!({ "kind": "torznab", "name": "Bad", "base_url": site, "app_profile_id": -1 })).unwrap_err();
    assert_eq!(e.detail, "'App Profile Id' must be greater than '0'.");
    // the seeding goals a download client is given (`SeedConfigProvider`)
    st.indexers.update(id, &json!({ "settings": { "torrentBaseSettings.seedRatio": "2", "torrentBaseSettings.seedTime": "60" } })).unwrap();
    let seed = st.indexers.seed_configuration(id).unwrap();
    assert_eq!((seed.ratio, seed.seed_time), (Some(2.0), Some(std::time::Duration::from_secs(3600))));
    assert!(st.indexers.seed_configuration(0).is_none());
}

#[tokio::test]
async fn requests_to_one_site_wait_for_the_request_delay() {
    let (_, st) = lantern("delay").await;
    let (site, hits) = start_torznab().await;
    let id =
        st.indexers.add(&json!({ "kind": "torznab", "name": "Slow", "base_url": site, "settings": { "request_delay": "1" } })).unwrap();
    let started = std::time::Instant::now();
    st.indexers.search(&[id], &search(), "test").await.unwrap();
    st.indexers.search(&[id], &search(), "test").await.unwrap();
    // caps, search, search: two waits of a second between three requests to the same host
    assert_eq!(hits.caps.load(Ordering::SeqCst) + hits.searches.load(Ordering::SeqCst), 3);
    assert!(started.elapsed() >= std::time::Duration::from_millis(1900), "{:?}", started.elapsed());
}
