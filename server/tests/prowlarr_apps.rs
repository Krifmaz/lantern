//! Prowlarr's applications as Lantern runs them: the `/api/v1/applications` and `/api/v1/appprofile` API with
//! Prowlarr's JSON and checks, and the app sync, against a stand-in Sonarr and Radarr that must receive exactly the
//! indexer Prowlarr v2.7.0.5723 would push (`tests/fixtures/upstream/prowlarr/apps_*.json`, built from
//! `BuildSonarrIndexer` and `BuildRadarrIndexer`).

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use kit_server_core::{p, Db, RowExt};
use serde_json::{json, Value};
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
    let dir = std::env::temp_dir().join(format!("lantern-papps-{name}-{}", std::process::id()));
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

/// Prowlarr's answer or push for `name`.
fn upstream(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/upstream/prowlarr/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// What a stand-in app was sent: method, path with query, body.
type Log = Arc<Mutex<Vec<(String, String, Value)>>>;

/// A stand-in Sonarr or Radarr (`/api/v3`): its indexer schema, indexers and test, with the API key checked.
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

impl Arr {
    /// An answer with the app's version header.
    fn answer(&self, status: StatusCode, body: Value) -> Response {
        (status, [("x-application-version", self.version), ("content-type", "application/json")], body.to_string()).into_response()
    }

    /// Notes the request; whether it carries the app's API key.
    fn note(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body: &Bytes) -> bool {
        let v = serde_json::from_slice(body).unwrap_or(Value::Null);
        self.log.lock().unwrap().push((method.to_string(), uri.to_string(), v));
        headers.get("x-api-key").and_then(|h| h.to_str().ok()) == Some(ARR_KEY)
    }
}

/// Every request the stand-in app answers.
async fn arr_any(State(a): State<Arr>, method: Method, uri: Uri, headers: HeaderMap, path: Option<Path<String>>, body: Bytes) -> Response {
    if !a.note(&method, &uri, &headers, &body) {
        return a.answer(StatusCode::UNAUTHORIZED, Value::Null);
    }
    let tail = path.map(|Path(p)| p).unwrap_or_default();
    let mut list = a.indexers.lock().unwrap();
    match (method.as_str(), tail.as_str()) {
        ("GET", "schema") => a.answer(StatusCode::OK, a.schema.clone()),
        ("POST", "test") => a.answer(StatusCode::OK, json!({})),
        ("GET", "") => a.answer(StatusCode::OK, Value::Array(list.clone())),
        ("POST", "") => {
            let mut v: Value = serde_json::from_slice(&body).unwrap();
            let id = list.iter().filter_map(|i| i["id"].as_i64()).max().unwrap_or(0) + 1;
            v["id"] = json!(id);
            v["implementationName"] = v["implementation"].clone();
            list.push(v.clone());
            a.answer(StatusCode::CREATED, v)
        }
        (m, id) => {
            let id: i64 = id.parse().unwrap_or(-1);
            let Some(pos) = list.iter().position(|i| i["id"].as_i64() == Some(id)) else {
                return a.answer(StatusCode::NOT_FOUND, json!({"message": "NotFound"}));
            };
            match m {
                "GET" => a.answer(StatusCode::OK, list[pos].clone()),
                "PUT" => {
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    list[pos] = v.clone();
                    a.answer(StatusCode::ACCEPTED, v)
                }
                _ => {
                    list.remove(pos);
                    a.answer(StatusCode::OK, json!({}))
                }
            }
        }
    }
}

/// Starts a stand-in app on a free port; its address and itself.
async fn arr(schema: &str, version: &'static str) -> (String, Arr) {
    let a = Arr { schema: upstream(schema), version, indexers: Arc::default(), log: Arc::default() };
    let r = Router::new()
        .route("/api/v3/indexer", get(arr_any).post(arr_any))
        .route("/api/v3/indexer/{tail}", get(arr_any).post(arr_any).put(arr_any).delete(arr_any))
        .with_state(a.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    (format!("http://{addr}"), a)
}

/// Waits (up to 20 s) until the stand-in app was sent a request that `pick` accepts; that request's body.
async fn sent(a: &Arr, what: &str, pick: impl Fn(&str, &str, &Value) -> bool) -> Value {
    for _ in 0..200 {
        if let Some((_, _, b)) = a.log.lock().unwrap().iter().find(|(m, u, b)| pick(m, u, b)) {
            return b.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the app was never sent {what}: {:#?}", a.log.lock().unwrap());
}

/// The JSON for an application of `implementation` at `base_url`.
fn app_body(name: &str, implementation: &str, base_url: &str, sync_level: &str) -> Value {
    json!({
        "name": name,
        "implementation": implementation,
        "configContract": format!("{implementation}Settings"),
        "syncLevel": sync_level,
        "tags": [],
        "fields": [
            { "name": "prowlarrUrl", "value": LANTERN_URL },
            { "name": "baseUrl", "value": base_url },
            { "name": "apiKey", "value": ARR_KEY },
        ],
    })
}

/// Adds Lantern's two indexers: a Usenet one (1) and a torrent one (2) with seeding settings.
fn add_indexers(st: &server::AppState) {
    let usenet = json!({
        "kind": "newznab", "name": "Usenet Site", "base_url": "http://127.0.0.1:9/",
        "settings": { "apikey": "x", "categories": "5040,5070,2040" },
    });
    assert_eq!(st.indexers.add(&usenet).unwrap(), 1);
    let torrent = json!({
        "kind": "torznab", "name": "Torrent Site", "base_url": "http://127.0.0.1:9/", "priority": 10,
        "settings": { "apikey": "x", "categories": "5030,2040", "torrentBaseSettings.seedRatio": "1.5", "torrentBaseSettings.seedTime": "60" },
    });
    assert_eq!(st.indexers.add(&torrent).unwrap(), 2);
}

/// The number of indexers Lantern holds in app `app_id`.
fn maps(st: &server::AppState, app_id: i64) -> i64 {
    st.db.one("SELECT COUNT(*) AS n FROM prowlarr_app_indexer_maps WHERE app_id=?", p![app_id]).unwrap().unwrap().i("n")
}

#[tokio::test]
async fn sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step() {
    let (app, st) = lantern("sonarr").await;
    let (url, sonarr) = arr("apps_sonarr_indexer_schema", "4.0.11.2680").await;
    add_indexers(&st);
    server::prowlarr::apps::sync::check_changes(&st).await;

    // saving the app tests it with Prowlarr's test indexer, then gives it Lantern's indexers
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body("Sonarr", "Sonarr", &url, "fullSync"))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(v["syncLevel"], "fullSync");
    assert_eq!(v["enable"], true);
    let test = sent(&sonarr, "the test indexer", |m, u, _| m == "POST" && u.ends_with("/indexer/test")).await;
    assert_eq!(test, upstream("apps_sonarr_push_test"));
    let usenet =
        sent(&sonarr, "the Usenet indexer", |m, u, b| m == "POST" && u == "/api/v3/indexer" && b["implementation"] == "Newznab").await;
    assert_eq!(usenet, upstream("apps_sonarr_push_newznab"));
    let torrent =
        sent(&sonarr, "the torrent indexer", |m, u, b| m == "POST" && u == "/api/v3/indexer" && b["implementation"] == "Torznab").await;
    assert_eq!(torrent, upstream("apps_sonarr_push_torznab"));
    assert_eq!(maps(&st, id), 2);

    // the app's ids, by name (Prowlarr adds indexers in name order)
    let remote_id =
        |name: &str| sonarr.indexers.lock().unwrap().iter().find(|i| i["name"] == name).map(|i| i["id"].as_i64().unwrap()).unwrap();
    let (usenet_id, torrent_id) = (remote_id("Usenet Site (Prowlarr)"), remote_id("Torrent Site (Prowlarr)"));
    assert_eq!((torrent_id, usenet_id), (1, 2));

    // a changed indexer is changed in the app (Full Sync), keeping the app's own settings and fields
    {
        let mut list = sonarr.indexers.lock().unwrap();
        let mine = list.iter_mut().find(|i| i["id"] == usenet_id).unwrap();
        mine["seasonSearchMaximumSingleEpisodeAge"] = json!(14);
        mine["fields"].as_array_mut().unwrap().push(json!({ "name": "failDownloads", "value": [1], "type": "select", "advanced": true }));
    }
    st.indexers.update(1, &json!({ "name": "Usenet Renamed" })).unwrap();
    server::prowlarr::apps::sync::check_changes(&st).await;
    let put = sent(&sonarr, "the renamed indexer", |m, _, b| m == "PUT" && b["name"] == "Usenet Renamed (Prowlarr)").await;
    assert_eq!(put["id"], usenet_id);
    assert_eq!(put["seasonSearchMaximumSingleEpisodeAge"], 14);
    let kept = put["fields"].as_array().unwrap().iter().find(|f| f["name"] == "failDownloads").cloned();
    assert_eq!(kept.map(|f| f["value"].clone()), Some(json!([1])), "the app's other fields are kept");

    // a deleted indexer leaves the app
    st.indexers.remove(2).unwrap();
    server::prowlarr::apps::sync::check_changes(&st).await;
    let gone = format!("/api/v3/indexer/{torrent_id}");
    sent(&sonarr, "the removal", |m, u, _| m == "DELETE" && u == gone).await;
    assert_eq!(maps(&st, id), 1);

    // the sync command with forceSync sends what is already there again
    let before = sonarr.log.lock().unwrap().iter().filter(|(m, _, _)| m == "PUT").count();
    let (s, v) = call(&app, "POST", "/api/v1/command", Some(json!({ "name": "ApplicationIndexerSync", "forceSync": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((v["body"]["forceSync"].as_bool(), v["body"]["completionMessage"].as_str()), (Some(true), Some("Completed")));
    let cmd = v["id"].as_i64().unwrap();
    let mut done = Value::Null;
    for _ in 0..200 {
        let (_, v) = call(&app, "GET", &format!("/api/v1/command/{cmd}"), None).await;
        if v["status"] == "completed" {
            done = v;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(done["message"], "Completed", "{done}");
    assert_eq!(sonarr.log.lock().unwrap().iter().filter(|(m, _, _)| m == "PUT").count(), before + 1);

    // removing the app forgets what it held
    let (s, v) = call(&app, "DELETE", &format!("/api/v1/applications/{id}"), None).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    assert_eq!(maps(&st, id), 0);
}

#[tokio::test]
async fn radarr_gets_its_first_sync_fields_and_add_only_skips_changes() {
    let (app, st) = lantern("radarr").await;
    let (url, radarr) = arr("apps_radarr_indexer_schema", "5.14.0.9383").await;
    add_indexers(&st);
    server::prowlarr::apps::sync::check_changes(&st).await;
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body("Radarr", "Radarr", &url, "addOnly"))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    // Radarr's default sync categories are movies only: both indexers have 2040
    let torrent =
        sent(&radarr, "the torrent indexer", |m, u, b| m == "POST" && u == "/api/v3/indexer" && b["implementation"] == "Torznab").await;
    assert_eq!(torrent, upstream("apps_radarr_push_torznab"));
    sent(&radarr, "the Usenet indexer", |m, u, b| m == "POST" && u == "/api/v3/indexer" && b["implementation"] == "Newznab").await;
    assert_eq!(maps(&st, id), 2);
    // Add Only: a change stays in Lantern
    st.indexers.update(1, &json!({ "name": "Usenet Renamed" })).unwrap();
    server::prowlarr::apps::sync::check_changes(&st).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!radarr.log.lock().unwrap().iter().any(|(m, _, _)| m == "PUT"));
}

#[tokio::test]
async fn an_app_with_tags_gets_only_the_indexers_that_share_one() {
    let (app, st) = lantern("tags").await;
    let (url, sonarr) = arr("apps_sonarr_indexer_schema", "4.0.11.2680").await;
    add_indexers(&st);
    st.indexers.update(1, &json!({ "tags": "tv" })).unwrap();
    server::prowlarr::apps::sync::check_changes(&st).await;
    let (s, tag) = call(&app, "POST", "/api/v1/tag", Some(json!({ "label": "tv" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{tag}");
    let mut body = app_body("Sonarr", "Sonarr", &url, "fullSync");
    body["tags"] = json!([tag["id"]]);
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    let usenet = sent(&sonarr, "the tagged indexer", |m, u, _| m == "POST" && u == "/api/v3/indexer").await;
    assert_eq!(usenet["name"], "Usenet Site (Prowlarr)");
    // a full sync leaves the untagged indexer out
    let (s, _) = call(&app, "POST", "/api/v1/command", Some(json!({ "name": "ApplicationIndexerSync" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    for _ in 0..50 {
        if call(&app, "GET", "/api/v1/command", None).await.1.as_array().unwrap().iter().any(|c| c["status"] == "completed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let posts = sonarr.log.lock().unwrap().iter().filter(|(m, u, _)| m == "POST" && u == "/api/v3/indexer").count();
    assert_eq!((posts, maps(&st, id)), (1, 1));
}

#[tokio::test]
async fn an_old_sonarr_or_a_wrong_key_fails_the_test_with_prowlarrs_messages() {
    let (app, _st) = lantern("old").await;
    let (url, _) = arr("apps_sonarr_indexer_schema", "3.0.4.1000").await;
    let (s, v) = call(&app, "POST", "/api/v1/applications/test", Some(app_body("Sonarr", "Sonarr", &url, "fullSync"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v[0]["errorMessage"], "Sonarr version should be at least 3.0.5. Version reported is 3.0.4.1000");
    let mut body = app_body("Sonarr", "Sonarr", &url, "fullSync");
    body["fields"][2]["value"] = json!("wrong");
    let (s, v) = call(&app, "POST", "/api/v1/applications/test", Some(body)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!((v[0]["propertyName"].as_str(), v[0]["errorMessage"].as_str()), (Some("ApiKey"), Some("API Key is invalid")));
    // nothing listens here: the save is refused, unless it is turned off
    let closed = "http://127.0.0.1:9";
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body("Gone", "Sonarr", closed, "fullSync"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body("Gone", "Sonarr", closed, "disabled"))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["enable"], false);
    // test all: only apps that sync are tested
    let (s, v) = call(&app, "POST", "/api/v1/applications/testall", None).await;
    assert_eq!((s, v), (StatusCode::OK, json!([])));
    let (s, v) = call(&app, "PUT", "/api/v1/applications/1?forceSave=true", Some(app_body("Old", "Sonarr", &url, "fullSync"))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let (s, v) = call(&app, "POST", "/api/v1/applications/testall", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(
        v,
        json!([{ "id": 1, "isValid": false, "validationFailures": [
            { "propertyName": "", "errorMessage": "Sonarr version should be at least 3.0.5. Version reported is 3.0.4.1000",
              "attemptedValue": "3.0.4.1000", "severity": "error" }
        ] }])
    );
    // apps have no actions of their own: Prowlarr answers null
    let (s, v) = call(&app, "POST", "/api/v1/applications/action/anything", Some(app_body("Old", "Sonarr", &url, "fullSync"))).await;
    assert_eq!((s, v), (StatusCode::OK, Value::Null));
}

#[tokio::test]
async fn the_application_api_follows_prowlarrs_provider_controller() {
    let (app, st) = lantern("crud").await;

    // the schema: one template per app, by name, with Prowlarr's defaults and Full Sync
    let (s, v) = call(&app, "GET", "/api/v1/applications/schema", None).await;
    assert_eq!(s, StatusCode::OK);
    let names: Vec<&str> = v.as_array().unwrap().iter().map(|t| t["implementation"].as_str().unwrap()).collect();
    assert_eq!(names, ["LazyLibrarian", "Lidarr", "Mylar", "Radarr", "Readarr", "Sonarr", "Whisparr"]);
    let sonarr = v.as_array().unwrap().iter().find(|t| t["implementation"] == "Sonarr").unwrap().clone();
    assert_eq!(sonarr, upstream("apps_schema_sonarr"));

    // checks: a name, then the app's own settings
    let mut body = app_body("", "Sonarr", "http://sonarr:8989", "disabled");
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(body.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!((v[0]["propertyName"].as_str(), v[0]["errorMessage"].as_str()), (Some("Name"), Some("'Name' must not be empty.")));
    body["name"] = json!("Sonarr");
    body["fields"][2]["value"] = json!("");
    // the settings are only checked for an app that syncs
    body["syncLevel"] = json!("fullSync");
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(body.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!((v[0]["propertyName"].as_str(), v[0]["errorMessage"].as_str()), (Some("ApiKey"), Some("'Api Key' must not be empty.")));
    body["syncLevel"] = json!("disabled");
    body["fields"][2]["value"] = json!(ARR_KEY);

    // save, read, change, refuse a second one with the same name
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(body.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    let key = v["fields"].as_array().unwrap().iter().find(|f| f["name"] == "apiKey").unwrap().clone();
    assert_eq!(key["value"], "********", "the API key is masked");
    assert_eq!(v["infoLink"], "https://wiki.servarr.com/prowlarr/supported#sonarr");
    let (s, v) = call(&app, "POST", "/api/v1/applications", Some(body.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v[0]["errorMessage"], "Should be unique");
    let (s, list) = call(&app, "GET", "/api/v1/applications", None).await;
    assert_eq!((s, list.as_array().unwrap().len()), (StatusCode::OK, 1));
    let mut changed = list[0].clone();
    changed["name"] = json!("Sonarr 4K");
    let (s, v) = call(&app, "PUT", &format!("/api/v1/applications/{id}"), Some(changed)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["name"], "Sonarr 4K");
    let saved = provider_settings(&st, id);
    assert_eq!(saved["apiKey"], ARR_KEY, "a masked key keeps the saved one");
    let (s, v) = call(&app, "GET", "/api/v1/applications/99", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");

    // bulk: change the sync level and the tags, then remove
    let (s, v) = call(&app, "PUT", "/api/v1/applications/bulk", Some(json!({ "ids": [] }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["message"], "BadRequest: ids must be provided");
    let (s, v) = call(
        &app,
        "PUT",
        "/api/v1/applications/bulk",
        Some(json!({ "ids": [id], "syncLevel": "addOnly", "tags": [3], "applyTags": "add" })),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!((v[0]["syncLevel"].as_str(), v[0]["tags"].clone()), (Some("addOnly"), json!([3])));
    let (s, v) = call(&app, "DELETE", "/api/v1/applications/bulk", Some(json!({ "ids": [id] }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, list) = call(&app, "GET", "/api/v1/applications", None).await;
    assert_eq!(list, json!([]));
}

/// The saved settings of application `id`.
fn provider_settings(st: &server::AppState, id: i64) -> Value {
    let r = st.db.one("SELECT settings FROM prowlarr_applications WHERE id=?", p![id]).unwrap().unwrap();
    serde_json::from_str(&r.s("settings")).unwrap()
}

#[tokio::test]
async fn app_profiles_follow_prowlarr() {
    let (app, st) = lantern("profiles").await;
    let (s, v) = call(&app, "GET", "/api/v1/appprofile", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!([{ "id": 1, "name": "Standard", "enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true, "minimumSeeders": 1 }])
    );
    let (_, v) = call(&app, "GET", "/api/v1/appprofile/schema", None).await;
    assert_eq!(v["name"], "");
    // the last profile can't go
    let (s, v) = call(&app, "DELETE", "/api/v1/appprofile/1", None).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(v["message"], "Profile [1] is in use.");
    let (s, v) = call(&app, "POST", "/api/v1/appprofile", Some(json!({ "name": "" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v[0]["propertyName"], "Name");
    let (s, v) = call(
        &app,
        "POST",
        "/api/v1/appprofile",
        Some(json!({ "name": "Interactive", "enableInteractiveSearch": true, "minimumSeeders": 3 })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((v["id"].as_i64(), v["enableRss"].as_bool(), v["minimumSeeders"].as_i64()), (Some(2), Some(false), Some(3)));
    let (s, v) =
        call(&app, "PUT", "/api/v1/appprofile/2", Some(json!({ "name": "Interactive only", "enableInteractiveSearch": true }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["name"], "Interactive only");
    // an indexer using it keeps it
    st.indexers
        .add(&json!({ "kind": "newznab", "name": "Site", "base_url": "http://127.0.0.1:9/", "settings": { "appProfileId": "2" } }))
        .unwrap();
    let (s, v) = call(&app, "DELETE", "/api/v1/appprofile/2", None).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("Profile [2] is in use.")));
    st.indexers.remove(1).unwrap();
    let (s, v) = call(&app, "DELETE", "/api/v1/appprofile/2", None).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    let (s, _) = call(&app, "GET", "/api/v1/appprofile/2", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn apps_held_back_after_failures_show_in_health() {
    let (app, st) = lantern("health").await;
    for (name, url) in [("Sonarr", "http://sonarr:8989"), ("Radarr", "http://radarr:7878")] {
        let (s, v) = call(&app, "POST", "/api/v1/applications", Some(app_body(name, name, url, "disabled"))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        let (s, v) =
            call(&app, "PUT", &format!("/api/v1/applications/{}?forceSave=true", v["id"]), Some(app_body(name, name, url, "fullSync")))
                .await;
        assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    }
    let now = server::auth::now();
    let hold = |id: i64| {
        st.db
            .execute(
                "INSERT INTO prowlarr_application_status (provider_id, initial_failure, most_recent_failure, escalation_level, disabled_till) VALUES (?,?,?,?,?)",
                p![id, now - 600.0, now - 60.0, 3, now + 600.0],
            )
            .unwrap();
    };
    hold(1);
    let (_, v) = call(&app, "GET", "/api/v1/health", None).await;
    let f = v.as_array().unwrap().iter().find(|f| f["source"] == "ApplicationStatusCheck").cloned().unwrap();
    assert_eq!(
        f,
        json!({ "source": "ApplicationStatusCheck", "type": "warning", "message": "Applications unavailable due to failures: Sonarr",
            "wikiUrl": "https://wiki.servarr.com/prowlarr/system#applications-are-unavailable-due-to-failures" })
    );
    hold(2);
    let (_, v) = call(&app, "GET", "/api/v1/health", None).await;
    let f = v.as_array().unwrap().iter().find(|f| f["source"] == "ApplicationStatusCheck").cloned().unwrap();
    assert_eq!((f["type"].as_str(), f["message"].as_str()), (Some("error"), Some("All applications are unavailable due to failures")));
    // removing an app removes its status
    call(&app, "DELETE", "/api/v1/applications/1", None).await;
    assert!(st.db.one("SELECT * FROM prowlarr_application_status WHERE provider_id=1", p![]).unwrap().is_none());
}

#[tokio::test]
async fn apps_imported_from_prowlarr_are_taken_over_once() {
    let (app, st) = lantern("imported").await;
    st.db
        .execute(
            "INSERT INTO imported_apps (name, kind, base_url, api_key, source_url, sync_level, tags, categories, imported) VALUES (?,?,?,?,?,?,?,?,?)",
            p!["Sonarr", "Sonarr", "http://sonarr:8989", "abc", "http://prowlarr:9696", "add_remove", "tv", "[5000,5040]", 1.0],
        )
        .unwrap();
    st.db
        .execute(
            "INSERT INTO imported_apps (name, kind, base_url, imported) VALUES (?,?,?,?)",
            p!["Overseerr", "Overseerr", "http://overseerr:5055", 1.0],
        )
        .unwrap();
    server::prowlarr::apps::applications::take_imported(&st);
    server::prowlarr::apps::applications::take_imported(&st);
    let (_, list) = call(&app, "GET", "/api/v1/applications", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let a = &list[0];
    assert_eq!((a["name"].as_str(), a["syncLevel"].as_str()), (Some("Sonarr"), Some("addOnly")));
    let field = |n: &str| a["fields"].as_array().unwrap().iter().find(|f| f["name"] == n).unwrap()["value"].clone();
    assert_eq!(
        (field("baseUrl"), field("prowlarrUrl"), field("syncCategories")),
        (json!("http://sonarr:8989"), json!("http://prowlarr:9696"), json!([5000, 5040]))
    );
    assert_eq!(a["tags"].as_array().unwrap().len(), 1);
    assert_eq!(provider_settings(&st, a["id"].as_i64().unwrap())["apiKey"], "abc");
}
