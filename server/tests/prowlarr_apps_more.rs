//! Prowlarr's other applications as Lantern syncs them: a stand-in Lidarr, Readarr and Whisparr (the Arr API) and
//! a stand-in Mylar and LazyLibrarian (one address with a `cmd`), each of which must receive what Prowlarr
//! v2.7.0.5723's `Build<App>Indexer` would push (Prowlarr's `Lidarr.cs`, `Readarr.cs`, `Whisparr.cs`, `Mylar.cs`,
//! `LazyLibrarian.cs`). Nothing reaches the internet.

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

/// The API key the tests' Lantern has.
const KEY: &str = "test-key";
/// The API key of the stand-in apps.
const ARR_KEY: &str = "arr-key";
/// The Prowlarr server address the apps are told.
const LANTERN_URL: &str = "http://lantern:9696";

/// A fresh Lantern on a data folder of its own.
async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-papps2-{name}-{}", std::process::id()));
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

/// Prowlarr's Sonarr indexer schema, the base of the other apps' schemas.
fn sonarr_schema() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/prowlarr/apps_sonarr_indexer_schema.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// An app's schema: Sonarr's Newznab and Torznab with `rename` applied to the field names and `extra` fields added.
fn schema_like(rename: &[(&str, &str)], extra: &[&str], contract_prefix: &str) -> Value {
    let mut s = sonarr_schema();
    for entry in s.as_array_mut().unwrap() {
        let implementation = entry["implementation"].as_str().unwrap_or("").to_string();
        entry["configContract"] = json!(format!("{implementation}Settings"));
        let _ = contract_prefix;
        let fields = entry["fields"].as_array_mut().unwrap();
        fields.retain(|f| !f["name"].as_str().unwrap_or("").starts_with("anime"));
        for f in fields.iter_mut() {
            if let Some((_, to)) = rename.iter().find(|(from, _)| f["name"] == *from) {
                f["name"] = json!(to);
            }
        }
        if implementation == "Newznab" || implementation == "Torznab" {
            for e in extra {
                fields.push(json!({ "order": 20, "name": e, "label": e, "value": 0, "type": "number", "advanced": true }));
            }
        }
    }
    s
}

/// What a stand-in app was sent: method, path with query, body.
type Log = Arc<Mutex<Vec<(String, String, Value)>>>;

/// A stand-in Arr app: its indexer schema, indexers and test, with the API key checked.
#[derive(Clone)]
struct Arr {
    /// Its indexer schema.
    schema: Value,
    /// The version it reports.
    version: &'static str,
    /// Its indexers.
    indexers: Arc<Mutex<Vec<Value>>>,
    /// What it was sent.
    log: Log,
}

/// Every request the stand-in Arr app answers.
async fn arr_any(State(a): State<Arr>, method: Method, uri: Uri, headers: HeaderMap, path: Option<Path<String>>, body: Bytes) -> Response {
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    a.log.lock().unwrap().push((method.to_string(), uri.to_string(), v.clone()));
    let answer = |status: StatusCode, body: Value| {
        (status, [("x-application-version", a.version), ("content-type", "application/json")], body.to_string()).into_response()
    };
    if headers.get("x-api-key").and_then(|h| h.to_str().ok()) != Some(ARR_KEY) {
        return answer(StatusCode::UNAUTHORIZED, Value::Null);
    }
    let tail = path.map(|Path(p)| p).unwrap_or_default();
    let mut list = a.indexers.lock().unwrap();
    match (method.as_str(), tail.as_str()) {
        ("GET", "schema") => answer(StatusCode::OK, a.schema.clone()),
        ("POST", "test") => answer(StatusCode::OK, json!({})),
        ("GET", "") => answer(StatusCode::OK, Value::Array(list.clone())),
        ("POST", "") => {
            let mut v = v;
            v["id"] = json!(list.len() as i64 + 1);
            list.push(v.clone());
            answer(StatusCode::CREATED, v)
        }
        _ => answer(StatusCode::OK, json!({})),
    }
}

/// Starts a stand-in Arr app with its API at `api`; its address and itself.
async fn arr(api: &str, schema: Value, version: &'static str) -> (String, Arr) {
    let a = Arr { schema, version, indexers: Arc::default(), log: Arc::default() };
    let r = Router::new()
        .route(&format!("{api}/indexer"), get(arr_any).post(arr_any))
        .route(&format!("{api}/indexer/{{tail}}"), get(arr_any).post(arr_any).put(arr_any).delete(arr_any))
        .with_state(a.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    (format!("http://{addr}"), a)
}

/// Waits (up to 20 s) for a request in `log` that `pick` accepts; its path and body.
async fn sent(log: &Log, what: &str, pick: impl Fn(&str, &str, &Value) -> bool) -> (String, Value) {
    for _ in 0..200 {
        if let Some((_, u, b)) = log.lock().unwrap().iter().find(|(m, u, b)| pick(m, u, b)) {
            return (u.clone(), b.clone());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the app was never sent {what}: {:#?}", log.lock().unwrap());
}

/// The JSON for an application of `implementation` at `base_url`.
fn app_body(implementation: &str, base_url: &str) -> Value {
    json!({
        "name": implementation,
        "implementation": implementation,
        "configContract": format!("{implementation}Settings"),
        "syncLevel": "fullSync",
        "tags": [],
        "fields": [
            { "name": "prowlarrUrl", "value": LANTERN_URL },
            { "name": "baseUrl", "value": base_url },
            { "name": "apiKey", "value": ARR_KEY },
        ],
    })
}

/// Adds a Usenet indexer (1) and a torrent indexer (2, priority 10, seeding 1.5 and 60 minutes) with music (3010),
/// book (7020), comic (7030) and adult (6000) categories, and every search mode.
fn add_indexers(st: &server::AppState) {
    let usenet = json!({
        "kind": "newznab", "name": "Usenet Site", "base_url": "http://127.0.0.1:9/",
        "settings": { "apikey": "x", "categories": "3010,7020,7030,6000" },
    });
    assert_eq!(st.indexers.add(&usenet).unwrap(), 1);
    let torrent = json!({
        "kind": "torznab", "name": "Torrent Site", "base_url": "http://127.0.0.1:9/", "priority": 10,
        "settings": { "apikey": "x", "categories": "3010,7020,7030,6000", "torrentBaseSettings.seedRatio": "1.5", "torrentBaseSettings.seedTime": "60" },
    });
    assert_eq!(st.indexers.add(&torrent).unwrap(), 2);
}

/// A field's value in a pushed indexer.
fn field(ix: &Value, name: &str) -> Value {
    ix["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).map(|f| f["value"].clone()).unwrap_or(Value::Null)
}

/// The field names of a pushed indexer.
fn field_names(ix: &Value) -> Vec<String> {
    let mut v: Vec<String> = ix["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

/// Adds `implementation` (an Arr app at `api`) and checks the Usenet and torrent indexers it gets: Prowlarr's name,
/// address, key, the categories its sync categories share, the seeding settings and exactly the fields Prowlarr's
/// `Build<App>Indexer` copies from the schema. The torrent push.
async fn arr_push(name: &str, api: &str, version: &'static str, schema: Value, cats: Value, fields: &[&str]) -> Value {
    let (app, st) = lantern(name).await;
    let (url, stand_in) = arr(api, schema, version).await;
    add_indexers(&st);
    server::prowlarr::apps::sync::check_changes(&st).await;
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body(name, &url))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    // the test with Prowlarr's test indexer, at the app's API
    let (path, _) = sent(&stand_in.log, "the test", |m, u, _| m == "POST" && u.ends_with("/indexer/test")).await;
    assert_eq!(path, format!("{api}/indexer/test"));
    let add = format!("{api}/indexer");
    for (implementation, id) in [("Newznab", 1), ("Torznab", 2)] {
        let (_, ix) = sent(&stand_in.log, implementation, |m, u, b| m == "POST" && u == add && b["implementation"] == implementation).await;
        let site = if id == 1 { "Usenet Site" } else { "Torrent Site" };
        assert_eq!(ix["name"], format!("{site} (Prowlarr)"));
        assert_eq!(ix["configContract"], format!("{implementation}Settings"));
        assert_eq!((ix["enableRss"].clone(), ix["enableAutomaticSearch"].clone()), (json!(true), json!(true)));
        assert_eq!(field(&ix, "baseUrl"), format!("{LANTERN_URL}/{id}/"));
        assert_eq!((field(&ix, "apiPath"), field(&ix, "apiKey")), (json!("/api"), json!(KEY)));
        assert_eq!(field(&ix, "categories"), cats, "{name} {implementation}");
        let mut want: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
        if implementation == "Newznab" {
            want.retain(|f| !f.starts_with("seedCriteria") && f != "minimumSeeders" && f != "rejectBlocklistedTorrentHashesWhileGrabbing");
        }
        want.sort();
        assert_eq!(field_names(&ix), want, "{name} {implementation}");
        if implementation == "Torznab" {
            assert_eq!(ix["priority"], 10);
            assert_eq!((field(&ix, "seedCriteria.seedRatio"), field(&ix, "seedCriteria.seedTime")), (json!(1.5), json!(60)));
            return ix;
        }
    }
    unreachable!()
}

#[tokio::test]
async fn lidarr_gets_the_indexers_prowlarr_would_push() {
    // BuildLidarrIndexer: the music categories (a sub-category brings its parent, as Prowlarr's category tree does), discography seed time from the seed time, earlyReleaseLimit and
    // additionalParameters on the first sync
    let schema = schema_like(&[("seedCriteria.seasonPackSeedTime", "seedCriteria.discographySeedTime")], &["earlyReleaseLimit"], "");
    let fields = [
        "baseUrl",
        "apiPath",
        "apiKey",
        "categories",
        "additionalParameters",
        "earlyReleaseLimit",
        "minimumSeeders",
        "seedCriteria.seedRatio",
        "seedCriteria.seedTime",
        "seedCriteria.discographySeedTime",
        "rejectBlocklistedTorrentHashesWhileGrabbing",
    ];
    let ix = arr_push("Lidarr", "/api/v1", "2.5.3.4341", schema, json!([3000, 3010]), &fields).await;
    assert_eq!(field(&ix, "seedCriteria.discographySeedTime"), json!(60));
}

#[tokio::test]
async fn readarr_gets_the_indexers_prowlarr_would_push() {
    // BuildReadarrIndexer: the book categories (the 7000s, with their parent), the discography seed time
    let schema = schema_like(&[("seedCriteria.seasonPackSeedTime", "seedCriteria.discographySeedTime")], &[], "");
    let fields = [
        "baseUrl",
        "apiPath",
        "apiKey",
        "categories",
        "minimumSeeders",
        "seedCriteria.seedRatio",
        "seedCriteria.seedTime",
        "seedCriteria.discographySeedTime",
        "rejectBlocklistedTorrentHashesWhileGrabbing",
    ];
    let ix = arr_push("Readarr", "/api/v1", "0.4.0.2593", schema, json!([7000, 7020, 7030]), &fields).await;
    assert_eq!(field(&ix, "seedCriteria.discographySeedTime"), json!(60));
}

#[tokio::test]
async fn whisparr_gets_the_indexers_prowlarr_would_push() {
    // BuildWhisparrIndexer: the adult categories (a whole parent brings its sub-categories), at /api/v3, the season pack seed time
    let schema = schema_like(&[], &[], "");
    let fields = [
        "baseUrl",
        "apiPath",
        "apiKey",
        "categories",
        "minimumSeeders",
        "seedCriteria.seedRatio",
        "seedCriteria.seedTime",
        "seedCriteria.seasonPackSeedTime",
        "rejectBlocklistedTorrentHashesWhileGrabbing",
    ];
    let ix =
        arr_push("Whisparr", "/api/v3", "2.0.0.548", schema, json!([6000, 6010, 6020, 6030, 6040, 6045, 6050, 6070, 6080, 6090]), &fields)
            .await;
    assert_eq!(field(&ix, "seedCriteria.seasonPackSeedTime"), json!(60));
}

/// A stand-in Mylar or LazyLibrarian: `/api?cmd=…&apikey=…`, its providers kept as the app keeps them.
#[derive(Clone, Default)]
struct Nab {
    /// Each command's query.
    log: Arc<Mutex<Vec<HashMap<String, String>>>>,
    /// Its providers: (type, query of the add).
    providers: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

/// Every command the stand-in answers.
async fn nab_any(State(n): State<Nab>, Query(q): Query<HashMap<String, String>>) -> Response {
    n.log.lock().unwrap().push(q.clone());
    if q.get("apikey").map(String::as_str) != Some(ARR_KEY) {
        return axum::Json(json!({ "success": false, "error": { "code": 401, "message": "Wrong API key" } })).into_response();
    }
    let cmd = q.get("cmd").cloned().unwrap_or_default();
    match cmd.as_str() {
        "getVersion" => axum::Json(json!({ "success": true, "data": { "current_version": "1.0" } })).into_response(),
        "listProviders" | "listNabProviders" => {
            let list = n.providers.lock().unwrap().clone();
            let of = |kind: &str| -> Vec<Value> {
                list.iter()
                    .filter(|p| p.get("providertype").map(String::as_str) == Some(kind))
                    .map(|p| {
                        json!({
                            "name": p["name"], "host": p["host"], "apikey": p["prov_apikey"], "categories": p["categories"],
                            "enabled": p["enabled"] == "true", "altername": p.get("altername").cloned().unwrap_or_default(),
                        })
                    })
                    .collect()
            };
            axum::Json(json!({ "success": true, "data": { "torznabs": of("torznab"), "newznabs": of("newznab") } })).into_response()
        }
        "addProvider" => {
            n.providers.lock().unwrap().push(q);
            axum::Json(json!({ "success": true })).into_response()
        }
        _ => axum::Json(json!({ "success": true })).into_response(),
    }
}

/// Starts a stand-in Mylar or LazyLibrarian; its address and itself.
async fn nab() -> (String, Nab) {
    let n = Nab::default();
    let r = Router::new().route("/api", get(nab_any)).with_state(n.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    (format!("http://{addr}"), n)
}

/// Adds `implementation` (Mylar or LazyLibrarian) and waits for its two providers; each add's query, by type.
async fn nab_push(implementation: &str) -> HashMap<String, HashMap<String, String>> {
    let (app, st) = lantern(implementation).await;
    let (url, stand_in) = nab().await;
    add_indexers(&st);
    server::prowlarr::apps::sync::check_changes(&st).await;
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body(implementation, &url))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    for _ in 0..200 {
        let adds: Vec<HashMap<String, String>> = stand_in.providers.lock().unwrap().clone();
        if adds.len() == 2 {
            return adds.into_iter().map(|q| (q["providertype"].clone(), q)).collect();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{implementation} never got both providers: {:#?}", stand_in.log.lock().unwrap());
}

#[tokio::test]
async fn mylar_gets_the_providers_prowlarr_would_add() {
    // BuildMylarIndexer and MylarV3Proxy.AddIndexer: the comic category (7030), no priority or seeding
    let adds = nab_push("Mylar").await;
    for (kind, id) in [("newznab", 1), ("torznab", 2)] {
        let q = &adds[kind];
        assert_eq!(q["cmd"], "addProvider");
        assert_eq!(q["host"], format!("{LANTERN_URL}/{id}/api"));
        assert_eq!((q["prov_apikey"].as_str(), q["enabled"].as_str(), q["categories"].as_str()), (KEY, "true", "7030"));
        assert!(q["name"].ends_with(" (Prowlarr)"));
        assert!(!q.contains_key("dlpriority") && !q.contains_key("seed_ratio"));
    }
}

#[tokio::test]
async fn lazylibrarian_gets_the_providers_prowlarr_would_add() {
    // BuildLazyLibrarianIndexer and LazyLibrarianV1Proxy.AddIndexer: book and audiobook categories (music is not one), the priority as
    // dlpriority (51 minus it, plus one … Prowlarr's 50 - priority + 1) and the torrent's seeding
    let adds = nab_push("LazyLibrarian").await;
    let usenet = &adds["newznab"];
    assert_eq!(usenet["host"], format!("{LANTERN_URL}/1/api"));
    assert_eq!(usenet["categories"], "7000,7020,7030");
    assert!(!usenet.contains_key("seed_ratio"));
    let torrent = &adds["torznab"];
    assert_eq!(torrent["dlpriority"], (50 - 10 + 1).to_string());
    assert_eq!((torrent["seed_ratio"].as_str(), torrent["seed_duration"].as_str()), ("1.5", "60"));
}
