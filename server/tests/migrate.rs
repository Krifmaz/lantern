//! Moving from Prowlarr and FlareSolverr: a Prowlarr backup (made here, with the tables Prowlarr has) and a running
//! Prowlarr's API (served here) are looked at in a dry run, then imported; a second import changes nothing; a
//! FlareSolverr compose file becomes the steps to take.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// Lantern on a data folder of its own with the test definitions, and its API key.
async fn lantern(name: &str) -> (Router, server::AppState, PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("lantern-migrate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let defs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definitions").display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| (n == "APP_DEFINITIONS_DIR").then(|| defs.clone())).await.unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    st.ensure_api_key().await.unwrap();
    let key = st.settings.str("api_key");
    (server::app(st.clone()), st, dir, key)
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

fn json_req(key: &str, method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", key);
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// A multipart upload of `file` with the form's other fields.
fn upload(key: &str, file: &[u8], fields: &[(&str, &str)]) -> Request<Body> {
    let boundary = "lantern-test-boundary";
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    body.extend(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"backup.zip\"\r\nContent-Type: application/zip\r\n\r\n")
            .as_bytes(),
    );
    body.extend(file);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::post("/api/import/prowlarr")
        .header("x-api-key", key)
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap()
}

/// A Prowlarr database at `path` with the tables (and columns) Prowlarr has, and a few of everything.
fn prowlarr_db(path: &Path) {
    use kit_server_core::{Db, SqliteDb};
    let db = SqliteDb::open(path).unwrap();
    db.execute_batch(
        r#"CREATE TABLE "Config" ("Id" INTEGER PRIMARY KEY, "Key" TEXT NOT NULL, "Value" TEXT NOT NULL);
        CREATE TABLE "Tags" ("Id" INTEGER PRIMARY KEY, "Label" TEXT NOT NULL);
        CREATE TABLE "Indexers" ("Id" INTEGER PRIMARY KEY, "Name" TEXT NOT NULL, "Implementation" TEXT NOT NULL, "Settings" TEXT,
            "ConfigContract" TEXT, "Enable" INTEGER, "Priority" INTEGER NOT NULL DEFAULT 25, "Added" DATETIME NOT NULL,
            "Redirect" INTEGER NOT NULL DEFAULT 0, "AppProfileId" INTEGER NOT NULL DEFAULT 1, "Tags" TEXT, "DownloadClientId" INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE "Applications" ("Id" INTEGER PRIMARY KEY, "Name" TEXT NOT NULL, "Implementation" TEXT NOT NULL, "Settings" TEXT,
            "ConfigContract" TEXT, "SyncLevel" INTEGER NOT NULL, "Tags" TEXT);
        CREATE TABLE "IndexerProxies" ("Id" INTEGER PRIMARY KEY, "Name" TEXT NOT NULL, "Implementation" TEXT NOT NULL, "Settings" TEXT,
            "ConfigContract" TEXT, "Tags" TEXT);
        CREATE TABLE "DownloadClients" ("Id" INTEGER PRIMARY KEY, "Enable" INTEGER NOT NULL, "Name" TEXT NOT NULL, "Implementation" TEXT NOT NULL,
            "Settings" TEXT NOT NULL, "ConfigContract" TEXT NOT NULL, "Priority" INTEGER NOT NULL DEFAULT 1, "Tags" TEXT);
        CREATE TABLE "Notifications" ("Id" INTEGER PRIMARY KEY, "Name" TEXT NOT NULL, "Implementation" TEXT NOT NULL, "Settings" TEXT NOT NULL);
        INSERT INTO Tags VALUES (1, 'flaresolverr'), (2, 'private');
        INSERT INTO Config (Key, Value) VALUES ('proxyenabled', 'True'), ('proxytype', '2'), ('proxyhostname', 'proxy.lan'), ('proxyport', '1080');
        INSERT INTO Indexers (Id, Name, Implementation, Settings, ConfigContract, Enable, Priority, Added, Tags) VALUES
          (3, 'Lantern Test Tracker', 'Cardigann',
           '{"definitionFile":"lantern-test","baseUrl":"http://127.0.0.1:1/","extraFieldData":{"username":"me","password":"hunter2","freeleech":true,"sort":1},"baseSettings":{"limitsUnit":0},"torrentBaseSettings":{}}',
           'CardigannSettings', 1, 10, '2024-01-01 00:00:00', '[1,2]'),
          (7, 'NZBgeek', 'Newznab', '{"baseUrl":"https://api.nzbgeek.info","apiPath":"/api","apiKey":"geek-key","additionalParameters":"","categories":[2000]}',
           'NewznabSettings', 0, 25, '2024-01-01 00:00:00', '[]'),
          (8, 'My Custom Site', 'Cardigann', '{"definitionFile":"my-custom-site","extraFieldData":{}}', 'CardigannSettings', 1, 25, '2024-01-01 00:00:00', '[]'),
          (9, 'Some Tracker', 'SomeBuiltInTracker', '{"baseUrl":"https://tracker.test/","apiKey":"x"}', 'SomeSettings', 1, 25, '2024-01-01 00:00:00', '[]');
        INSERT INTO Applications (Name, Implementation, Settings, ConfigContract, SyncLevel, Tags) VALUES
          ('Sonarr', 'Sonarr', '{"prowlarrUrl":"http://prowlarr:9696","baseUrl":"http://sonarr:8989","apiKey":"sonarr-key","syncCategories":[5000,5040]}', 'SonarrSettings', 2, '[]');
        INSERT INTO IndexerProxies (Name, Implementation, Settings, ConfigContract, Tags) VALUES
          ('FlareSolverr', 'FlareSolverr', '{"host":"http://flaresolverr:8191/","requestTimeout":60}', 'FlareSolverrSettings', '[1]'),
          ('Tagged proxy', 'Http', '{"host":"tagged.lan","port":3128}', 'HttpSettings', '[2]');
        INSERT INTO DownloadClients (Enable, Name, Implementation, Settings, ConfigContract) VALUES (1, 'qBittorrent', 'QBittorrent', '{}', 'QBittorrentSettings');
        INSERT INTO Notifications (Name, Implementation, Settings) VALUES ('Discord', 'Discord', '{}');"#,
    )
    .unwrap();
}

/// A Prowlarr backup: its database and config.xml in a zip.
fn prowlarr_backup(dir: &Path) -> Vec<u8> {
    std::fs::create_dir_all(dir).unwrap();
    let db = dir.join("prowlarr.db");
    prowlarr_db(&db);
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("prowlarr.db", opts).unwrap();
    zip.write_all(&std::fs::read(&db).unwrap()).unwrap();
    zip.start_file("config.xml", opts).unwrap();
    zip.write_all(b"<Config>\n  <Port>9696</Port>\n  <ApiKey>prowlarr-api-key</ApiKey>\n  <UrlBase></UrlBase>\n</Config>").unwrap();
    zip.finish().unwrap().into_inner()
}

fn step<'a>(plan: &'a Value, part: &str, name: &str) -> &'a Value {
    plan[part].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("no {name} in {part}: {plan:#}"))
}

#[tokio::test]
async fn a_prowlarr_backup_is_looked_at_then_imported_once() {
    let (app, st, dir, key) = lantern("backup").await;
    let backup = prowlarr_backup(&dir.join("prowlarr"));

    // the dry run says what would happen and changes nothing
    let (s, plan) = call(&app, upload(&key, &backup, &[])).await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    assert_eq!(plan["dry_run"], true);
    let site = step(&plan, "indexers", "Lantern Test Tracker");
    assert_eq!(site["action"], "add");
    assert_eq!(site["secrets"], json!(["password"]));
    assert_eq!(step(&plan, "indexers", "NZBgeek")["action"], "add");
    assert!(step(&plan, "indexers", "My Custom Site")["detail"].as_str().unwrap().contains("no definition called my-custom-site"));
    assert_eq!(step(&plan, "indexers", "Some Tracker")["action"], "skip");
    assert_eq!(step(&plan, "apps", "Sonarr")["action"], "add");
    assert_eq!(step(&plan, "settings", "API key")["action"], "set");
    assert_eq!(step(&plan, "settings", "Port")["action"], "nothing_needed");
    assert_eq!(step(&plan, "settings", "Prowlarr's proxy")["action"], "set");
    assert_eq!(step(&plan, "settings", "Tagged proxy")["action"], "skip");
    assert!(step(&plan, "settings", "FlareSolverr")["detail"].as_str().unwrap().contains("Lantern Test Tracker"));
    assert!(plan["notes"].to_string().contains("qBittorrent"));
    assert!(plan["summary"].as_str().unwrap().ends_with("Nothing was changed yet."));
    assert!(!plan.to_string().contains("hunter2") && !plan.to_string().contains("geek-key"), "secrets never show in a plan");
    assert!(st.indexers.list().unwrap().is_empty());
    assert_eq!(st.settings.str("api_key"), key);

    // the import adds them under Prowlarr's numbers, with their settings as Lantern reads them
    let (s, plan) = call(&app, upload(&key, &backup, &[("dry_run", "false")])).await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    assert_eq!(plan["dry_run"], false);
    let site = st.indexers.get(3).unwrap();
    assert_eq!((site.kind.as_str(), site.definition.as_str(), site.priority), ("cardigann", "lantern-test", 10));
    assert_eq!(site.base_url, "", "the definition's own address stays the definition's");
    assert_eq!(site.settings.get("password").map(String::as_str), Some("hunter2"));
    assert_eq!(site.settings.get("freeleech").map(String::as_str), Some("true"));
    assert_eq!(site.settings.get("sort").map(String::as_str), Some("seeders"), "Prowlarr's 1 is the second option by value");
    assert_eq!(site.tags, "flaresolverr,private");
    let geek = st.indexers.get(7).unwrap();
    assert_eq!((geek.kind.as_str(), geek.definition.as_str(), geek.enabled), ("newznab", "nzbgeek", false));
    assert_eq!(geek.settings.get("apikey").map(String::as_str), Some("geek-key"));
    assert!(step(&plan, "indexers", "NZBgeek")["detail"].as_str().unwrap().contains("Its number is 7, as in Prowlarr"));
    assert_eq!(st.indexers.list().unwrap().len(), 2);
    assert_eq!(st.settings.str("api_key"), "prowlarr-api-key");
    assert_eq!(st.settings.str("site_proxy"), "socks5://proxy.lan:1080");
    let new_key = "prowlarr-api-key";
    let (_, apps) = call(&app, json_req(new_key, "GET", "/api/import/apps", None)).await;
    assert_eq!(apps["apps"][0]["name"], "Sonarr");
    assert_eq!(apps["apps"][0]["api_key"], "••••");
    assert_eq!(apps["apps"][0]["categories"], json!([5000, 5040]));

    // a new indexer after the import gets a number of its own
    let id = st.indexers.add(&json!({"kind": "torznab", "name": "Later", "base_url": "http://t.test"})).unwrap();
    assert!(id > 7);

    // a second import finds everything already there
    let (s, plan) = call(&app, upload(new_key, &backup, &[("dry_run", "false")])).await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    assert_eq!(step(&plan, "indexers", "Lantern Test Tracker")["action"], "already_there");
    assert_eq!(step(&plan, "apps", "Sonarr")["action"], "already_there");
    assert_eq!(step(&plan, "settings", "API key")["action"], "already_there");
    assert_eq!(st.indexers.list().unwrap().len(), 3);

    // the uploads are cleaned up
    assert_eq!(std::fs::read_dir(st.data_dir.join("import")).unwrap().count(), 0);
}

#[tokio::test]
async fn uploads_that_are_not_prowlarr_are_refused_in_words() {
    let (app, _, dir, key) = lantern("refused").await;
    let (s, v) = call(&app, upload(&key, b"just some text", &[])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["detail"].as_str().unwrap().contains("isn't a Prowlarr backup"));
    // a database that isn't Prowlarr's
    let other = dir.join("other.db");
    {
        use kit_server_core::{Db, SqliteDb};
        SqliteDb::open(&other).unwrap().execute_batch("CREATE TABLE things (id INTEGER);").unwrap();
    }
    let (s, v) = call(&app, upload(&key, &std::fs::read(&other).unwrap(), &[])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["detail"].as_str().unwrap().contains("isn't Prowlarr's"));
    // without a file
    let (s, v) = call(&app, upload(&key, b"", &[])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
}

/// A Prowlarr that answers its API like the real one (passwords and keys masked), on a free port; its address.
async fn fake_prowlarr() -> String {
    let field = |n: &str, v: Value| json!({ "name": n, "value": v });
    let indexers = json!([
        { "id": 4, "name": "Lantern Test Tracker", "implementation": "Cardigann", "definitionName": "lantern-test", "enable": true, "priority": 20, "tags": [1],
          "fields": [field("definitionFile", json!("lantern-test")), field("baseUrl", json!("http://127.0.0.1:1/")), field("username", json!("me")),
                     field("password", json!("********")), field("sort", json!(0)), field("baseSettings.queryLimit", Value::Null)] }
    ]);
    let apps = json!([{ "name": "Radarr", "implementation": "Radarr", "syncLevel": "addOnly", "tags": [],
        "fields": [field("prowlarrUrl", json!("http://prowlarr:9696")), field("baseUrl", json!("http://radarr:7878")), field("apiKey", json!("********"))] }]);
    let app = Router::new()
        .route(
            "/api/v1/system/status",
            get(|h: axum::http::HeaderMap| async move {
                if h.get("x-api-key").map(|v| v.as_bytes()) != Some(b"real-key") {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                Ok(Json(json!({ "appName": "Prowlarr", "version": "1.24.3" })))
            }),
        )
        .route("/api/v1/tag", get(|| async { Json(json!([{ "id": 1, "label": "movies" }])) }))
        .route("/api/v1/indexer", get(move || async move { Json(indexers) }))
        .route("/api/v1/applications", get(move || async move { Json(apps) }))
        .route("/api/v1/indexerproxy", get(|| async { Json(json!([])) }))
        .route(
            "/api/v1/config/host",
            get(|| async { Json(json!({ "port": 9797, "urlBase": "/prowlarr", "apiKey": "real-key", "proxyEnabled": false })) }),
        )
        .route("/api/v1/downloadclient", get(|| async { Json(json!([])) }))
        .route("/api/v1/notification", get(|| async { Json(json!([])) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_running_prowlarr_is_read_through_its_api() {
    let (app, st, _, key) = lantern("api").await;
    let url = fake_prowlarr().await;

    let (s, v) = call(&app, json_req(&key, "POST", "/api/import/prowlarr/api", Some(json!({ "url": url, "key": "wrong" })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["detail"].as_str().unwrap().contains("didn't accept that API key"));

    let body = json!({ "url": url, "key": "real-key", "dry_run": false, "take_api_key": false });
    let (s, plan) = call(&app, json_req(&key, "POST", "/api/import/prowlarr/api", Some(body))).await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    let site = step(&plan, "indexers", "Lantern Test Tracker");
    assert_eq!(site["missing"], json!(["Password"]), "a masked password is asked for again");
    let ix = st.indexers.get(4).unwrap();
    assert_eq!(ix.settings.get("sort").map(String::as_str), Some("added"));
    assert!(!ix.settings.contains_key("password"));
    assert_eq!(ix.tags, "movies");
    assert_eq!(step(&plan, "apps", "Radarr")["missing"], json!(["API key"]));
    assert_eq!(step(&plan, "settings", "API key")["action"], "skip");
    assert_eq!(st.settings.str("api_key"), key, "Lantern's key stays when asked");
    assert_eq!(step(&plan, "settings", "Port")["action"], "manual");
    assert!(step(&plan, "settings", "Address path")["detail"].as_str().unwrap().contains("/prowlarr"));
}

#[tokio::test]
async fn flaresolverr_settings_become_steps() {
    let (app, _, _, key) = lantern("flaresolverr").await;
    let text = "services:\n  flaresolverr:\n    environment:\n      - LOG_LEVEL=info\n      - CAPTCHA_SOLVER=hcaptcha-solver\n    ports:\n      - \"8191:8191\"\n";
    let (s, plan) = call(&app, json_req(&key, "POST", "/api/import/flaresolverr", Some(json!({ "text": text })))).await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    assert_eq!(step(&plan, "settings", "Port")["action"], "nothing_needed");
    assert_eq!(step(&plan, "settings", "CAPTCHA_SOLVER")["action"], "skip");
    assert_eq!(step(&plan, "settings", "Apps that used FlareSolverr")["action"], "manual");
    let (s, v) = call(&app, json_req(&key, "POST", "/api/import/flaresolverr", Some(json!({ "text": "" })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["detail"].as_str().unwrap().contains("FlareSolverr"));
}
