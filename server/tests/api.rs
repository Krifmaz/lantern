//! Every kit feature the server offers, through its HTTP API: health, sign-in, What's new, notices, bug
//! reports, crash logs, Diagnostics, settings and the web app at `/`. Each test runs on a data folder of its own.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

/// The password the tests' owner signs in with.
const PASSWORD: &str = "correct horse battery";

/// A fresh server on a data folder of its own, with an owner `admin` and the web app's folder `static`.
async fn server(name: &str) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("app-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html><title>app</title>").unwrap();
    std::fs::write(web.join(format!("{}-web-0123456789abcdef.js", server::NAME)), "").unwrap();
    let st = server::AppState::open(&dir.join("data"), web, |name| (name == "APP_API_KEY").then(|| "test-key".to_string())).await.unwrap();
    server::auth::ensure_owner(&st, None, Some(PASSWORD.into())).unwrap();
    (server::app(st.clone()), st, dir)
}

/// One request; the status and the body as JSON (`Null` when it is not JSON).
async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let r = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (r.status(), r.headers().clone());
    let b = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&b).unwrap_or(Value::Null), headers)
}

/// A request as the owner, through the API key.
fn admin(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", "test-key");
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn health_names_the_version_and_the_web_build() {
    let (app, _, _) = server("health").await;
    let (s, v, _) = call(&app, Request::get("/api/health").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({ "ok": true, "version": server::VERSION, "server": "rust", "web": "0123456789abcdef" }));
    // every answer carries the app's mark, so the web app can tell a proxy's error page from it
    let r = app.clone().oneshot(Request::get("/api/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.headers().get(format!("x-{}", server::NAME)).map(|v| v.to_str().unwrap()), Some("1"));
    assert_eq!(r.headers().get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap()), Some("no-store"));
    assert!(server::hashed_asset("/t-web-0123456789abcdef_bg.wasm"));
    assert!(!server::hashed_asset("/index.html"));
}

#[tokio::test]
async fn signing_in_gives_a_session_that_needs_the_csrf_header() {
    let (app, _, _) = server("signin").await;
    let (s, v, _) = call(&app, Request::get("/api/auth/me").body(Body::empty()).unwrap()).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::UNAUTHORIZED, Some("Not authenticated")));
    let login = |pw: &str| {
        Request::post("/api/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "username": "admin", "password": pw }).to_string()))
            .unwrap()
    };
    let (s, v, _) = call(&app, login("wrong password")).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::UNAUTHORIZED, Some("Invalid username or password")));
    let (s, v, h) = call(&app, login(PASSWORD)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["username"], "admin");
    assert_eq!(v["is_admin"], true);
    assert!(v.get("password_hash").is_none());
    let cookie = h.get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    assert!(cookie.starts_with(&server::auth::cookie_name()));
    let (s, v, _) = call(&app, Request::get("/api/auth/me").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!((s, v["username"].as_str()), (StatusCode::OK, Some("admin")));
    // a change through the cookie without X-Requested-With is refused
    let seen = |csrf: bool| {
        let b = Request::post("/api/changelog/seen").header(header::COOKIE, &cookie);
        let b = if csrf { b.header("x-requested-with", server::NAME) } else { b };
        b.body(Body::empty()).unwrap()
    };
    let (s, v, _) = call(&app, seen(false)).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::FORBIDDEN, Some("Missing CSRF header")));
    let (s, _, _) = call(&app, seen(true)).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(&app, Request::post("/api/auth/logout").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(&app, Request::get("/api/auth/me").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn whats_new_shows_the_built_in_notes_once() {
    let (app, _, _) = server("changelog").await;
    let (s, v, _) = call(&app, admin("GET", "/api/changelog", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["releases"].as_array().is_some_and(|r| r.iter().any(|r| r["version"] == "0.0.1")), "{v}");
    let (_, v, _) = call(&app, admin("GET", "/api/changelog/new", None)).await;
    assert_eq!(v["show"], true, "{v}");
    let (s, _, _) = call(&app, admin("POST", "/api/changelog/seen", None)).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v, _) = call(&app, admin("GET", "/api/changelog/new", None)).await;
    assert_eq!(v["show"], false, "{v}");
}

#[tokio::test]
async fn the_data_folder_notice_is_fine_and_checks_again() {
    let (app, _, _) = server("notices").await;
    let (s, v, _) = call(&app, admin("POST", "/api/notices/data/check", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], true, "{v}");
    let (s, v, _) = call(&app, admin("GET", "/api/notices", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["notices"][0]["id"], "data", "{v}");
    let text = server::checks::data_notice_text(std::path::Path::new("/data"), "read-only");
    assert_eq!(text, "The data folder /data is read-only – check the disk (dmesg, fsck) or the mount");
}

#[tokio::test]
async fn a_bug_report_is_filed_listed_and_exported() {
    let (app, _, _) = server("bugs").await;
    let boundary = "kitboundary";
    let mut body = String::new();
    for (k, v) in [("kind", "bug"), ("title", "The button does nothing"), ("body", "I pressed it twice."), ("page", "#/settings")] {
        body.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let req = Request::post("/api/bug-reports")
        .header("x-api-key", "test-key")
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let (s, v, _) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_i64().unwrap();
    let (_, v, _) = call(&app, admin("GET", "/api/bug-reports", None)).await;
    assert_eq!(v["reports"][0]["title"], "The button does nothing", "{v}");
    let (s, v, _) = call(&app, admin("GET", &format!("/api/bug-reports/{id}"), None)).await;
    assert_eq!((s, v["report"]["page"].as_str()), (StatusCode::OK, Some("#/settings")), "{v}");
    let r = app.clone().oneshot(admin("GET", "/api/bug-reports/export?format=txt", None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let text = String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(text.contains("The button does nothing"), "{text}");
}

#[tokio::test]
async fn a_crash_log_is_kept_once() {
    let (app, _, _) = server("crash").await;
    let crash = json!({ "kind": "crash", "device": "Pixel 8", "device_id": "d1", "app_version": "0.1.0", "os": "Android 16",
                        "time": 1_793_800_000, "summary": "NullPointerException", "text": "java.lang.NullPointerException\n\tat …", "key": "k1" });
    let (s, v, _) = call(&app, admin("POST", "/api/crash-reports", Some(crash.clone()))).await;
    assert_eq!((s, v["duplicate"].as_bool()), (StatusCode::OK, Some(false)), "{v}");
    let (_, v, _) = call(&app, admin("POST", "/api/crash-reports", Some(crash))).await;
    assert_eq!(v["duplicate"], true, "{v}");
    let (_, v, _) = call(&app, admin("GET", "/api/crash-reports", None)).await;
    assert_eq!(v["crashes"].as_array().map(Vec::len), Some(1), "{v}");
}

#[tokio::test]
async fn diagnostics_check_the_data_folder_and_the_database() {
    let (app, _, _) = server("diag").await;
    let (s, v, _) = call(&app, admin("GET", "/api/diagnostics", None)).await;
    assert_eq!(s, StatusCode::OK);
    let names: Vec<&str> = v["checks"].as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).collect();
    assert_eq!(names, ["Data folder", "Database"]);
    assert!(v["checks"].as_array().unwrap().iter().all(|c| c["ok"] == true), "{v}");
    let (s, v, _) = call(&app, admin("POST", "/api/diagnostics/nothing/test", None)).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::NOT_FOUND, Some("Unknown check")));
}

#[tokio::test]
async fn settings_come_from_the_schema_and_save() {
    let (app, st, _) = server("settings").await;
    let (s, v, _) = call(&app, admin("GET", "/api/settings/schema", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.to_string().contains("App name"), "{v}");
    let (s, _, _) = call(&app, admin("PUT", "/api/settings", Some(json!({ "app_name": "Notes" })))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st.app_name(), "Notes");
    let (_, v, _) = call(&app, admin("GET", "/api/settings", None)).await;
    assert_eq!(v["app_name"], "Notes");
    assert_ne!(v["api_key"], "test-key", "a secret never goes to a browser");
    // members may not
    let (s, _, _) = call(&app, Request::get("/api/settings").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_web_app_is_served_for_every_other_path() {
    let (app, _, _) = server("web").await;
    for path in ["/", "/settings/diagnostics"] {
        let r = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{path}");
        let b = axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap();
        assert!(String::from_utf8_lossy(&b).contains("<title>app</title>"), "{path}");
    }
}

#[tokio::test]
async fn the_health_check_command_asks_a_running_server() {
    let (app, _, _) = server("healthcheck").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert!(server::healthcheck(port).await);
    let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let nobody = free.local_addr().unwrap().port();
    drop(free);
    assert!(!server::healthcheck(nobody).await);
}

#[tokio::test]
async fn a_phone_signs_in_with_an_app_token_and_keeps_its_prefs() {
    let (app, _, _) = server("apptoken").await;
    let body = json!({ "username": "admin", "password": PASSWORD, "app": { "device_id": "phone-1", "device_name": "Pixel 8" } });
    let req = Request::post("/api/auth/login").header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap();
    let (s, v, h) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(h.get(header::SET_COOKIE).is_none(), "a phone gets a token, not a cookie");
    let token = v["token"].as_str().unwrap().to_string();
    assert_eq!(v["user"]["username"], "admin");
    let auth = format!("Bearer {token}");
    let with = |m: &str, path: &str, body: Option<Value>| {
        let b = Request::builder().method(m).uri(path).header(header::AUTHORIZATION, &auth);
        match body {
            Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        }
    };
    let (s, v, _) = call(&app, with("PUT", "/api/auth/prefs", Some(json!({ "whats_new_popup": false })))).await;
    assert_eq!((s, &v["whats_new_popup"]), (StatusCode::OK, &json!(false)));
    let (_, v, _) = call(&app, with("GET", "/api/auth/prefs", None)).await;
    assert_eq!(v, json!({ "whats_new_popup": false }));
    let (s, _, _) = call(&app, with("POST", "/api/auth/logout", None)).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(&app, with("GET", "/api/auth/me", None)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
