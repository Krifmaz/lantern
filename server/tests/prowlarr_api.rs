//! Prowlarr's v1 API as Lantern answers it, endpoint by endpoint, against Prowlarr's own answers
//! (`tests/fixtures/upstream/prowlarr/`, built from Prowlarr v2.7.0.5723's resource classes): the same fields, the
//! same types, the same defaults, status codes and error texts.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use kit_server_core::{p, Db};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tower::ServiceExt;

/// The API key the tests' Lantern has.
const KEY: &str = "test-key";
/// The owner's password.
const PASSWORD: &str = "correct horse battery";

/// A fresh Lantern on a data folder of its own, with an owner `admin`.
async fn lantern(name: &str) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-pv1-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let st = server::AppState::open(&dir.join("data"), web, |n| (n == "APP_API_KEY").then(|| KEY.to_string())).await.unwrap();
    server::auth::ensure_owner(&st, None, Some(PASSWORD.into())).unwrap();
    (server::app(st.clone()), st, dir.join("data"))
}

/// One request: status, body (as JSON, `Null` when it is not JSON), headers.
async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value, HeaderMap) {
    let r = app.clone().oneshot(req).await.unwrap();
    let (s, h) = (r.status(), r.headers().clone());
    let b = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null), h)
}

/// A request with the API key in `X-Api-Key`, as Prowlarr's web app sends it.
fn api(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY).header(header::USER_AGENT, "curl/8.5.0");
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// Prowlarr's answer for `name`.
fn upstream(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/upstream/prowlarr/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// The shape of an answer: every field's name and type (an array by its first item), with the fields in `skip`
/// left out at any depth and the ones in `opaque` only typed.
fn shape(v: &Value, skip: &[&str], opaque: &[&str]) -> Value {
    match v {
        Value::Object(m) => {
            let fields: BTreeMap<String, Value> = m
                .iter()
                .filter(|(k, _)| !skip.contains(&k.as_str()))
                .map(|(k, x)| (k.clone(), if opaque.contains(&k.as_str()) { json!(type_of(x)) } else { shape(x, skip, opaque) }))
                .collect();
            json!(fields)
        }
        Value::Array(a) => match a.first() {
            Some(x) => json!([shape(x, skip, opaque)]),
            None => json!([]),
        },
        other => json!(type_of(other)),
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Lantern's answer has Prowlarr's fields and types (the kit's `detail` on errors and Prowlarr's .NET stack trace in
/// `description` aside).
fn assert_same_shape(lantern: &Value, prowlarr: &Value, opaque: &[&str]) {
    let skip = ["detail", "description"];
    assert_eq!(shape(lantern, &skip, opaque), shape(prowlarr, &skip, opaque), "\nLantern: {lantern:#}\nProwlarr: {prowlarr:#}");
}

/// The fields of `lantern` and `prowlarr` that should be equal (Prowlarr's defaults), all but `except`.
fn assert_same_values(lantern: &Value, prowlarr: &Value, except: &[&str]) {
    for (k, want) in prowlarr.as_object().unwrap() {
        if !except.contains(&k.as_str()) {
            assert_eq!(&lantern[k], want, "{k}");
        }
    }
}

#[tokio::test]
async fn ping_and_status() {
    let (app, _, _) = lantern("status").await;
    // /ping needs no key, GET and HEAD
    let (s, v, _) = call(&app, Request::get("/ping").body(Body::empty()).unwrap()).await;
    assert_eq!((s, v), (StatusCode::OK, upstream("ping")));
    let (s, _, _) = call(&app, Request::head("/ping").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);

    let (s, v, _) = call(&app, api("GET", "/api/v1/system/status", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_same_shape(&v, &upstream("system_status"), &[]);
    assert_eq!((v["appName"].as_str(), v["instanceName"].as_str()), (Some("Lantern"), Some("Lantern")));
    assert_eq!(
        (v["branch"].as_str(), v["authentication"].as_str(), v["databaseType"].as_str()),
        (Some("master"), Some("forms"), Some("sqLite"))
    );

    let r = app.clone().oneshot(api("GET", "/api/v1/system/routes", None)).await.unwrap();
    assert_eq!(r.headers()[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    let text = String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(text.starts_with("digraph DFA {") && text.contains("/api/v1/command"), "{text}");
}

#[tokio::test]
async fn the_key_or_a_session_is_needed_like_prowlarr() {
    let (app, _, _) = lantern("auth").await;
    let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();
    assert_eq!(call(&app, get("/api/v1/system/status")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, get("/api/v1/system/status?apikey=wrong")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, get(&format!("/api/v1/system/status?apikey={KEY}"))).await.0, StatusCode::OK);
    let bearer = Request::get("/api/v1/tag").header(header::AUTHORIZATION, format!("Bearer {KEY}")).body(Body::empty()).unwrap();
    assert_eq!(call(&app, bearer).await.0, StatusCode::OK);
    // the query parameter is read first, as Prowlarr reads it
    let both = Request::get("/api/v1/tag?apikey=wrong").header("x-api-key", KEY).body(Body::empty()).unwrap();
    assert_eq!(call(&app, both).await.0, StatusCode::UNAUTHORIZED);

    // a signed-in session works too
    let login = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "username": "admin", "password": PASSWORD }).to_string()))
        .unwrap();
    let (_, _, h) = call(&app, login).await;
    let cookie = h.get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let (s, v, _) = call(&app, Request::get("/api/v1/config/ui").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!((s, v["uiLanguage"].as_str()), (StatusCode::OK, Some("en")));
}

#[tokio::test]
async fn host_settings_have_prowlarrs_fields_defaults_and_checks() {
    let (app, st, _) = lantern("host").await;
    let (s, v, _) = call(&app, api("GET", "/api/v1/config/host", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let want = upstream("config_host");
    assert_same_shape(&v, &want, &[]);
    assert_same_values(&v, &want, &["apiKey", "password", "instanceName"]);
    assert_eq!((v["apiKey"].as_str(), v["instanceName"].as_str()), (Some(KEY), Some("Lantern")));
    let (_, by_id, _) = call(&app, api("GET", "/api/v1/config/host/1", None)).await;
    assert_eq!(by_id, v);

    // a bad value: Prowlarr's 400 with the failures
    let mut bad = v.clone();
    bad["port"] = json!(0);
    let (s, fails, _) = call(&app, api("PUT", "/api/v1/config/host/1", Some(bad))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_same_shape(&fails, &upstream("config_host_invalid"), &["formattedMessagePlaceholderValues"]);
    assert_eq!(fails[0]["errorMessage"], upstream("config_host_invalid")[0]["errorMessage"]);
    // an empty body
    let (s, e, _) = call(&app, api("PUT", "/api/v1/config/host/1", None)).await;
    assert_eq!((s, e["message"].as_str()), (StatusCode::BAD_REQUEST, Some("BadRequest: Request body can't be empty")));

    // a good change is saved, and the fields Lantern has settings for change them
    let mut good = v.clone();
    good["logSizeLimit"] = json!(5);
    good["branch"] = json!("Develop");
    good["trustedNetworks"] = json!(" 10.0.0.0/8 ,, 172.17.0.1 ");
    good["proxyEnabled"] = json!(true);
    good["proxyType"] = json!("socks5");
    good["proxyHostname"] = json!("proxy.lan");
    good["proxyPort"] = json!(1080);
    good["applicationUrl"] = json!("http://lantern.lan:9696");
    good["instanceName"] = json!("Lantern (home)");
    good["backupInterval"] = json!(2);
    good["apiKey"] = json!("not-changed-here");
    let (s, saved, h) = call(&app, api("PUT", "/api/v1/config/host/1", Some(good))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{saved}");
    assert_eq!(h[header::LOCATION], "/api/v1/config/host/1");
    assert_eq!((saved["logSizeLimit"].as_i64(), saved["branch"].as_str()), (Some(5), Some("develop")));
    assert_eq!(saved["trustedNetworks"], "10.0.0.0/8, 172.17.0.1");
    assert_eq!(st.settings.str("site_proxy"), "socks5://proxy.lan:1080");
    assert_eq!((saved["proxyEnabled"].as_bool(), saved["proxyType"].as_str()), (Some(true), Some("socks5")));
    assert_eq!(st.settings.str("public_url"), "http://lantern.lan:9696");
    assert_eq!(st.app_name(), "Lantern (home)");
    // Prowlarr never saves the API key from here
    assert_eq!(saved["apiKey"], KEY);
    // the backup task follows the backup interval
    let (_, tasks, _) = call(&app, api("GET", "/api/v1/system/task", None)).await;
    let backup = tasks.as_array().unwrap().iter().find(|t| t["taskName"] == "Backup").unwrap();
    assert_eq!(backup["interval"], 2 * 24 * 60);

    // the owner's password, with the confirmation
    let mut pw = saved.clone();
    pw["password"] = json!("a new password");
    pw["passwordConfirmation"] = json!("something else");
    let (s, fails, _) = call(&app, api("PUT", "/api/v1/config/host/1", Some(pw.clone()))).await;
    assert_eq!((s, fails[0]["errorMessage"].as_str()), (StatusCode::BAD_REQUEST, Some("Must match Password")));
    pw["passwordConfirmation"] = json!("a new password");
    assert_eq!(call(&app, api("PUT", "/api/v1/config/host/1", Some(pw))).await.0, StatusCode::ACCEPTED);
    let login = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "username": "admin", "password": "a new password" }).to_string()))
        .unwrap();
    assert_eq!(call(&app, login).await.0, StatusCode::OK);
}

#[tokio::test]
async fn ui_and_development_settings() {
    let (app, _, _) = lantern("ui").await;
    let (_, v, _) = call(&app, api("GET", "/api/v1/config/ui", None)).await;
    assert_same_shape(&v, &upstream("config_ui"), &[]);
    assert_same_values(&v, &upstream("config_ui"), &[]);
    let mut fr = v.clone();
    fr["uiLanguage"] = json!("fr");
    let (s, saved, _) = call(&app, api("PUT", "/api/v1/config/ui/1", Some(fr))).await;
    assert_eq!((s, saved["uiLanguage"].as_str()), (StatusCode::ACCEPTED, Some("fr")));
    let (_, strings, _) = call(&app, api("GET", "/api/v1/localization", None)).await;
    assert_eq!(strings["Strings"]["BackupNow"], "Sauvegarder maintenant");
    let mut bad = v.clone();
    bad["uiLanguage"] = json!("xx");
    let (s, fails, _) = call(&app, api("PUT", "/api/v1/config/ui/1", Some(bad))).await;
    assert_eq!((s, fails[0]["errorMessage"].as_str()), (StatusCode::BAD_REQUEST, Some("Invalid UI Language value")));

    let (_, v, _) = call(&app, api("GET", "/api/v1/config/development", None)).await;
    assert_same_shape(&v, &upstream("config_development"), &[]);
    assert_same_values(&v, &upstream("config_development"), &[]);
    let mut dev = v.clone();
    dev["logRotate"] = json!(10);
    dev["logSql"] = json!(true);
    let (s, saved, _) = call(&app, api("PUT", "/api/v1/config/development/1", Some(dev))).await;
    assert_eq!((s, saved["logRotate"].as_i64(), saved["logSql"].as_bool()), (StatusCode::ACCEPTED, Some(10), Some(true)));
    let (_, again, _) = call(&app, api("GET", "/api/v1/config/development/1", None)).await;
    assert_eq!(again, saved);
}

#[tokio::test]
async fn tags_and_their_details() {
    let (app, st, _) = lantern("tags").await;
    let (s, movies, h) = call(&app, api("POST", "/api/v1/tag", Some(json!({ "label": "movies" })))).await;
    assert_eq!(s, StatusCode::CREATED, "{movies}");
    assert_eq!(h[header::LOCATION], format!("/api/v1/tag/{}", movies["id"]));
    // the same label again is the same tag
    let (s, again, _) = call(&app, api("POST", "/api/v1/tag", Some(json!({ "label": "movies" })))).await;
    assert_eq!((s, &again), (StatusCode::CREATED, &movies));
    // another case of a saved label hits Prowlarr's unique label
    let (s, _, _) = call(&app, api("POST", "/api/v1/tag", Some(json!({ "label": "Movies" })))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, tv, _) = call(&app, api("POST", "/api/v1/tag", Some(json!({ "label": "TV" })))).await;
    assert_eq!((s, tv["label"].as_str()), (StatusCode::CREATED, Some("tv")));
    let (s, fails, _) = call(&app, api("POST", "/api/v1/tag", Some(json!({ "label": "tv shows" })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_same_shape(&fails, &upstream("tag_invalid"), &["formattedMessagePlaceholderValues"]);
    assert_eq!(fails[0]["errorMessage"], upstream("tag_invalid")[0]["errorMessage"]);

    let (_, list, _) = call(&app, api("GET", "/api/v1/tag", None)).await;
    assert_same_shape(&list, &upstream("tag"), &[]);
    assert_eq!(list.as_array().unwrap().iter().map(|t| t["label"].as_str().unwrap()).collect::<Vec<_>>(), ["movies", "tv"]);

    // an indexer that uses "movies" (and a tag only it has)
    st.db
        .execute("INSERT INTO indexers (kind, name, tags, added) VALUES ('torznab', 'Test', 'movies,4k', ?)", p![server::auth::now()])
        .unwrap();
    let (_, details, _) = call(&app, api("GET", "/api/v1/tag/detail", None)).await;
    assert_same_shape(&details, &upstream("tag_detail"), &[]);
    let four_k = details.as_array().unwrap().iter().find(|d| d["label"] == "4k").unwrap();
    assert_eq!(four_k["indexerIds"].as_array().unwrap().len(), 1);
    let (_, one, _) = call(&app, api("GET", &format!("/api/v1/tag/detail/{}", movies["id"]), None)).await;
    assert_eq!(one["indexerIds"].as_array().unwrap().len(), 1);

    // in use: Prowlarr's 409
    let (s, e, _) = call(&app, api("DELETE", &format!("/api/v1/tag/{}", movies["id"]), None)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_same_shape(&e, &upstream("tag_in_use"), &[]);
    assert_eq!(e["message"], format!("Tag with ID {} 'movies' cannot be deleted since it's still in use", movies["id"]));

    // renaming renames it on the indexer
    let (s, renamed, _) = call(&app, api("PUT", &format!("/api/v1/tag/{}", movies["id"]), Some(json!({ "label": "Films" })))).await;
    assert_eq!((s, renamed["label"].as_str()), (StatusCode::ACCEPTED, Some("films")));
    let tags: String = st.db.one("SELECT tags FROM indexers", p![]).unwrap().unwrap()["tags"].as_str().unwrap().into();
    assert_eq!(tags, "films,4k");

    let (s, e, _) = call(&app, api("GET", "/api/v1/tag/99", None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_same_shape(&e, &upstream("tag_not_found"), &[]);
    assert_eq!(e["message"], upstream("tag_not_found")["message"]);
    let (s, e, _) = call(&app, api("DELETE", "/api/v1/tag/0", None)).await;
    assert_eq!((s, e["message"].as_str()), (StatusCode::BAD_REQUEST, Some("BadRequest: 0 is not a valid ID")));
    let (s, _, _) = call(&app, api("DELETE", &format!("/api/v1/tag/{}", tv["id"]), None)).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn custom_filters() {
    let (app, _, _) = lantern("filters").await;
    let mut f = upstream("customfilter");
    f.as_object_mut().unwrap().remove("id");
    let (s, made, _) = call(&app, api("POST", "/api/v1/customfilter", Some(f))).await;
    assert_eq!(s, StatusCode::CREATED, "{made}");
    assert_same_shape(&made, &upstream("customfilter"), &[]);
    assert_eq!(made["filters"], upstream("customfilter")["filters"]);
    let id = made["id"].as_i64().unwrap();
    let mut changed = made.clone();
    changed["label"] = json!("Usenet");
    let (s, saved, _) = call(&app, api("PUT", &format!("/api/v1/customfilter/{id}"), Some(changed))).await;
    assert_eq!((s, saved["label"].as_str()), (StatusCode::ACCEPTED, Some("Usenet")));
    let (_, list, _) = call(&app, api("GET", "/api/v1/customfilter", None)).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (s, v, _) = call(&app, api("DELETE", &format!("/api/v1/customfilter/{id}"), None)).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    let (s, _, _) = call(&app, api("GET", &format!("/api/v1/customfilter/{id}"), None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn localization_and_health() {
    let (app, st, _) = lantern("l10n").await;
    let (_, v, _) = call(&app, api("GET", "/api/v1/localization", None)).await;
    assert_same_shape(&v, &upstream("localization"), &["Strings"]);
    assert!(v["Strings"].as_object().unwrap().len() > 500);
    assert_eq!(v["Strings"]["BackupNow"], upstream("localization")["Strings"]["BackupNow"]);
    let (_, opts, _) = call(&app, api("GET", "/api/v1/localization/options", None)).await;
    assert_same_shape(&opts, &upstream("localization_options"), &[]);
    assert_eq!(opts.as_array().unwrap().len(), 43);
    assert_eq!(opts[8], json!({ "name": "English", "value": "en" }));

    // no indexers, and a short API key: Prowlarr's checks say so
    let (_, health, _) = call(&app, api("GET", "/api/v1/health", None)).await;
    assert_same_shape(&health, &upstream("health"), &[]);
    let by = |source: &str| health.as_array().unwrap().iter().find(|h| h["source"] == source).cloned().unwrap();
    assert_eq!(by("IndexerCheck")["message"], "No indexers enabled, Lantern will not return search results");
    assert_eq!(by("IndexerCheck")["type"], "error");
    assert_eq!(by("ApiKeyValidationCheck")["wikiUrl"], "https://wiki.servarr.com/prowlarr/system#invalid-api-key");
    assert!(health.as_array().unwrap().iter().all(|h| h["source"] != "AllowedHostsCheck" && h["source"] != "IndexerStatusCheck"));

    // without sign-in for local addresses and no allowed hosts, and with indexers held back after failures
    server::prowlarr::config_set(&st.db, "AuthenticationRequired", "disabledForLocalAddresses").unwrap();
    let later = server::auth::now() + 3600.0;
    for (name, until) in [("Alpha", later), ("Beta", 0.0)] {
        st.db
            .execute(
                // failing since a minute ago: IndexerStatusCheck counts failures that began in the last 6 hours
                "INSERT INTO indexers (kind, name, disabled_until, initial_failure, added) VALUES ('torznab', ?, ?, ?, ?)",
                p![name, until, server::auth::now() - 60.0, server::auth::now()],
            )
            .unwrap();
    }
    let (_, health, _) = call(&app, api("GET", "/api/v1/health", None)).await;
    let by = |source: &str| health.as_array().unwrap().iter().find(|h| h["source"] == source).cloned().unwrap();
    assert_eq!(by("AllowedHostsCheck")["type"], "warning");
    assert!(by("AllowedHostsCheck")["message"].as_str().unwrap().starts_with("Allowed Hosts is not configured, Lantern will accept"));
    assert_eq!(
        (by("IndexerStatusCheck")["type"].as_str(), by("IndexerStatusCheck")["message"].as_str()),
        (Some("warning"), Some("Indexers unavailable due to failures: Alpha"))
    );
    assert!(health.as_array().unwrap().iter().all(|h| h["source"] != "IndexerCheck"));
    st.db.execute("UPDATE indexers SET disabled_until=?", p![later]).unwrap();
    let (_, health, _) = call(&app, api("GET", "/api/v1/health", None)).await;
    let all = health.as_array().unwrap().iter().find(|h| h["source"] == "IndexerStatusCheck").cloned().unwrap();
    assert_eq!((all["type"].as_str(), all["message"].as_str()), (Some("error"), Some("All indexers are unavailable due to failures")));
}

/// Waits until command `id` is no longer queued or running; its resource.
async fn finished(app: &Router, id: i64) -> Value {
    for _ in 0..200 {
        let (_, v, _) = call(app, api("GET", &format!("/api/v1/command/{id}"), None)).await;
        if v["status"] != "queued" && v["status"] != "started" {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("command {id} didn't finish");
}

#[tokio::test]
async fn commands_queue_run_and_report_like_prowlarr() {
    let (app, _, _) = lantern("commands").await;
    let (s, v, h) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": "test", "duration": 0 })))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(h[header::LOCATION], format!("/api/v1/command/{id}"));
    let done = finished(&app, id).await;
    assert_same_shape(&done, &upstream("command"), &[]);
    assert_same_values(&done, &upstream("command"), &["id", "queued", "started", "ended", "duration", "stateChangeTime", "body"]);
    assert_eq!(done["body"]["name"], "Test");

    // a command Lantern can't run is refused as Prowlarr refuses an unknown one
    let (s, e, _) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": "ApplicationUpdate" })))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_same_shape(&e, &upstream("command_unknown"), &[]);
    assert_eq!(e["message"], upstream("command_unknown")["message"]);
    // no name: NotNull and NotEmpty both fail
    let (s, fails, _) = call(&app, api("POST", "/api/v1/command", Some(json!({})))).await;
    assert_eq!((s, fails.as_array().unwrap().len()), (StatusCode::BAD_REQUEST, 2));
    assert_eq!(fails[0]["errorMessage"], "'Name' must not be empty.");

    // a finished command can't be cancelled
    let (s, e, _) = call(&app, api("DELETE", &format!("/api/v1/command/{id}"), None)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_same_shape(&e, &upstream("command_cancel_conflict"), &[]);

    // four run at once; the fifth waits and can be cancelled; the same command isn't queued twice
    let mut ids = Vec::new();
    for d in [1500, 1501, 1502, 1503, 1504] {
        let (_, v, _) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": "Test", "duration": d })))).await;
        ids.push(v["id"].as_i64().unwrap());
    }
    let (_, twice, _) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": "Test", "duration": 1504 })))).await;
    assert_eq!(twice["id"].as_i64(), Some(ids[4]));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (_, list, _) = call(&app, api("GET", "/api/v1/command", None)).await;
    let status = |id: i64| list.as_array().unwrap().iter().find(|c| c["id"] == id).map(|c| c["status"].as_str().unwrap().to_string());
    assert_eq!(status(ids[0]).as_deref(), Some("started"));
    assert_eq!(status(ids[4]).as_deref(), Some("queued"));
    assert_eq!(list[0]["status"], "started", "running ones come first");
    let (s, _, _) = call(&app, api("DELETE", &format!("/api/v1/command/{}", ids[4]), None)).await;
    assert_eq!(s, StatusCode::OK);
    let (_, list, _) = call(&app, api("GET", "/api/v1/command", None)).await;
    assert!(!list.as_array().unwrap().iter().any(|c| c["id"] == ids[4]));

    // CheckHealth runs and moves its task's last run
    let (_, before, _) = call(&app, api("GET", "/api/v1/system/task/4", None)).await;
    assert_eq!(before["taskName"], "CheckHealth");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (_, v, _) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": "CheckHealth" })))).await;
    let done = finished(&app, v["id"].as_i64().unwrap()).await;
    assert_eq!(done["status"], "completed");
    let (_, after, _) = call(&app, api("GET", "/api/v1/system/task/4", None)).await;
    assert_ne!(after["lastExecution"], before["lastExecution"]);

    // ResetApiKey fails while APP_API_KEY sets the key: Prowlarr's "Failed", with why
    let (_, v, _) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": "ResetApiKey" })))).await;
    let done = finished(&app, v["id"].as_i64().unwrap()).await;
    assert_eq!((done["status"].as_str(), done["message"].as_str()), (Some("failed"), Some("Failed")));
    assert!(done["exception"].as_str().unwrap().contains("APP_API_KEY"));
}

#[tokio::test]
async fn tasks_logs_backups_and_updates() {
    let (app, st, data) = lantern("system").await;
    let (_, tasks, _) = call(&app, api("GET", "/api/v1/system/task", None)).await;
    assert_same_shape(&tasks, &upstream("system_task"), &[]);
    let names: Vec<&str> = tasks.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 9);
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "{names:?}");
    let (_, first, _) = call(&app, api("GET", "/api/v1/system/task/1", None)).await;
    assert_eq!((first["taskName"].as_str(), first["interval"].as_i64()), (Some("MessagingCleanup"), Some(5)));
    assert_eq!(call(&app, api("GET", "/api/v1/system/task/999", None)).await.0, StatusCode::NO_CONTENT);

    // the log, page by page
    for (i, level) in ["Info", "Warn", "Error"].iter().enumerate() {
        st.db
            .execute(
                "INSERT INTO prowlarr_logs (message, time, logger, level) VALUES (?, ?, 'Lantern', ?)",
                p![format!("entry {i}"), 1_793_872_800.0 + i as f64, level],
            )
            .unwrap();
    }
    let (_, log, _) = call(&app, api("GET", "/api/v1/log?page=1&pageSize=50", None)).await;
    assert_same_shape(&log, &upstream("log"), &[]);
    assert_eq!((log["totalRecords"].as_i64(), log["records"][0]["message"].as_str()), (Some(3), Some("entry 2")));
    let (_, warn, _) = call(&app, api("GET", "/api/v1/log?level=warn&sortDirection=ascending", None)).await;
    assert_eq!((warn["totalRecords"].as_i64(), warn["records"][0]["level"].as_str()), (Some(2), Some("warn")));

    // log files
    std::fs::create_dir_all(data.join("logs")).unwrap();
    std::fs::write(data.join("logs/lantern.txt"), "started").unwrap();
    let (_, files, _) = call(&app, api("GET", "/api/v1/log/file", None)).await;
    assert_same_shape(&files, &upstream("log_file"), &[]);
    assert_eq!(
        (files[0]["contentsUrl"].as_str(), files[0]["downloadUrl"].as_str()),
        (Some("/api/v1//lantern.txt"), Some("/logfile/lantern.txt"))
    );
    let r = app.clone().oneshot(api("GET", "/api/v1/log/file/lantern.txt", None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(axum::body::to_bytes(r.into_body(), 1024).await.unwrap(), "started");
    assert_eq!(call(&app, api("GET", "/api/v1/log/file/missing.txt", None)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, api("GET", "/api/v1/log/file/update", None)).await.1, json!([]));

    // the commands that empty the log and the log files
    std::fs::create_dir_all(data.join("UpdateLogs")).unwrap();
    std::fs::write(data.join("UpdateLogs/update.txt"), "updated").unwrap();
    for name in ["ClearLog", "DeleteLogFiles", "DeleteUpdateLogFiles", "MessagingCleanup"] {
        let (s, v, _) = call(&app, api("POST", "/api/v1/command", Some(json!({ "name": name })))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!(finished(&app, v["id"].as_i64().unwrap()).await["status"], "completed", "{name}");
    }
    let (_, log, _) = call(&app, api("GET", "/api/v1/log", None)).await;
    assert_eq!(log["totalRecords"], 0);
    assert_eq!(call(&app, api("GET", "/api/v1/log/file", None)).await.1, json!([]));
    assert_eq!(call(&app, api("GET", "/api/v1/log/file/update", None)).await.1, json!([]));

    // backups in the backup folder, with Prowlarr's ids
    std::fs::create_dir_all(data.join("Backups/manual")).unwrap();
    std::fs::write(data.join("Backups/manual/prowlarr_backup_v1.0.0.zip"), vec![0u8; 18233]).unwrap();
    std::fs::write(data.join("Backups/manual/notes.txt"), "not a backup").unwrap();
    let (_, backups, _) = call(&app, api("GET", "/api/v1/system/backup", None)).await;
    assert_same_shape(&backups, &upstream("system_backup"), &[]);
    assert_same_values(&backups[0], &upstream("system_backup")[0], &["time"]);
    assert_eq!(backups.as_array().unwrap().len(), 1);

    let (_, updates, _) = call(&app, api("GET", "/api/v1/update", None)).await;
    assert_same_shape(&updates, &upstream("update"), &["changes"]);
    assert_eq!(updates[0]["latest"], true);
}
