//! autobrr's web frontend (the fork in `ui/autobrr/`) at `/autobrr/`: the page with the base URL put in, its files,
//! the manifest, sign-in, and the API prefix, as autobrr's `internal/http/web.go` answers them. Each test runs on a
//! data folder of its own with a small stand-in build.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

/// The password the tests' owner signs in with.
const PASSWORD: &str = "correct horse battery";

/// autobrr's `web/index.html` as its production build (vite) writes it: the parts the server fills in.
const INDEX: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <title>Lantern</title>
  <style>@font-face{src:url({{.AssetBaseUrl}}Inter-Variable.woff2)format("woff2")}</style>
  <script>
      window.APP = {};
      window.APP.baseUrl = "{{.BaseUrl}}";
      window.APP.instanceName = "{{.InstanceName}}";
  </script>
  <script type="module" crossorigin src="{{.AssetBaseUrl}}assets/index-BQQ523tm.js"></script>
  <link rel="stylesheet" crossorigin href="{{.AssetBaseUrl}}assets/index-C8N7B2zS.css">
</head>
<body class="bg-color"><div id="root" class="pattern h-screen"></div></body>
</html>"#;

/// A fresh server with an owner `admin` and a stand-in autobrr build in `autobrr`.
async fn server(name: &str) -> (Router, PathBuf) {
    let dir = std::env::temp_dir().join(format!("app-test-autobrr-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ui = dir.join("autobrr");
    std::fs::create_dir_all(ui.join("assets")).unwrap();
    std::fs::write(ui.join("index.html"), INDEX).unwrap();
    std::fs::write(ui.join("assets/index-BQQ523tm.js"), "console.log(1)").unwrap();
    std::fs::write(ui.join("assets/index-C8N7B2zS.css"), "body{}").unwrap();
    std::fs::write(ui.join("favicon.ico"), "ico").unwrap();
    std::fs::write(ui.join("Inter-Variable.woff2"), "font").unwrap();
    std::fs::write(ui.join("robots.txt"), "User-agent: *").unwrap();
    std::fs::write(
        ui.join("manifest.webmanifest"),
        r#"{"name":"Lantern","start_url":"{{.BaseUrl}}","scope":"{{.BaseUrl}}","display":"standalone"}"#,
    )
    .unwrap();
    let ui_dir = ui.to_string_lossy().to_string();
    let st = server::AppState::open(&dir.join("data"), dir.join("static"), move |name| match name {
        "APP_API_KEY" => Some("test-key".into()),
        "APP_AUTOBRR_UI_DIR" => Some(ui_dir.clone()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some(PASSWORD.into())).unwrap();
    (server::app(st), dir)
}

/// The owner's session cookie, from signing in.
async fn sign_in(app: &Router) -> String {
    let req = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "username": "admin", "password": PASSWORD }).to_string()))
        .unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
}

/// A GET of `path`, signed in with `cookie` when there is one: the status, the headers and the body as text.
async fn get(app: &Router, path: &str, cookie: Option<&str>) -> (StatusCode, axum::http::HeaderMap, String) {
    let b = Request::get(path);
    let b = match cookie {
        Some(c) => b.header(header::COOKIE, c),
        None => b,
    };
    let r = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let (status, headers) = (r.status(), r.headers().clone());
    let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (status, headers, String::from_utf8_lossy(&body).to_string())
}

/// The `detail` of a JSON error answer.
fn detail(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap()["detail"].as_str().unwrap().to_string()
}

#[test]
fn the_page_gets_the_base_url_like_autobrrs_template() {
    let html = server::autobrr_ui::fill(INDEX, "Lantern");
    for want in [
        r#"url(/autobrr/Inter-Variable.woff2)"#,
        r#"window.APP.baseUrl = "/autobrr/";"#,
        r#"window.APP.instanceName = "Lantern";"#,
        r#"src="/autobrr/assets/index-BQQ523tm.js""#,
        r#"href="/autobrr/assets/index-C8N7B2zS.css""#,
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert!(!html.contains("{{"));
    // an instance name can't end the script's string or the script
    let html = server::autobrr_ui::fill(INDEX, r#"My "box" </script><b>"#);
    assert!(html.contains(r#"window.APP.instanceName = "My \"box\" \u003c/script\u003e\u003cb\u003e";"#), "{html}");
}

#[test]
fn pages_are_autobrrs_valid_web_routes() {
    for page in [
        "/autobrr/",
        "/autobrr/index.html",
        "/autobrr/filters",
        "/autobrr/filters/3/actions",
        "/autobrr/releases",
        "/autobrr/settings/irc",
        "/autobrr/logs",
        "/autobrr/notifications",
        "/autobrr/onboard",
        "/autobrr/login",
        "/autobrr/logout",
    ] {
        assert!(server::autobrr_ui::is_page(page), "{page}");
    }
    for file in ["/autobrr/assets/index-1.js", "/autobrr/sw.js", "/autobrr/api/config", "/autobrr/unknown"] {
        assert!(!server::autobrr_ui::is_page(file), "{file}");
    }
}

#[tokio::test]
async fn every_page_is_the_frontend_under_autobrr() {
    let (app, _) = server("pages").await;
    let cookie = sign_in(&app).await;
    for path in ["/autobrr/", "/autobrr/index.html", "/autobrr/filters", "/autobrr/filters/1/general", "/autobrr/settings/irc"] {
        let (s, h, body) = get(&app, path, Some(&cookie)).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(h[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"), "{path}");
        assert!(body.contains(r#"src="/autobrr/assets/index-BQQ523tm.js""#), "{path}");
        assert!(body.contains(r#"window.APP.baseUrl = "/autobrr/";"#), "{path}");
    }
    // the base URL without its slash goes to the base URL, as a mounted chi router does
    let (s, h, _) = get(&app, "/autobrr", Some(&cookie)).await;
    assert_eq!((s, h[header::LOCATION].to_str().unwrap()), (StatusCode::PERMANENT_REDIRECT, "/autobrr/"));
    // the built files as they are; the favicon and the font from any path, as autobrr's "/*" route
    let (s, _, body) = get(&app, "/autobrr/assets/index-BQQ523tm.js", Some(&cookie)).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "console.log(1)"));
    let (s, _, body) = get(&app, "/autobrr/filters/Inter-Variable.woff2", Some(&cookie)).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "font"));
    // a missing file and an unknown path are 404s with a plain sentence
    for path in ["/autobrr/assets/missing-1.js", "/autobrr/unknown"] {
        let (s, _, body) = get(&app, path, Some(&cookie)).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
        assert!(detail(&body).contains("not part of"), "{path}");
    }
    // Lantern's own web app keeps `/`
    let (s, _, _) = get(&app, "/api/health", None).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn the_frontends_api_calls_arrive_under_autobrr_api() {
    let (app, _) = server("api").await;
    let cookie = sign_in(&app).await;
    // paths not ported yet: a plain 404 in Lantern's error shape, never the page
    for path in ["/autobrr/api/not-an-autobrr-path", "/autobrr/api/auth/nothing", "/autobrr/api/proxy/1/nothing"] {
        let (s, h, body) = get(&app, path, Some(&cookie)).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
        assert!(h[header::CONTENT_TYPE].to_str().unwrap().starts_with("application/json"), "{path}");
        assert!(detail(&body).contains("autobrr request"), "{path}");
    }
}

#[tokio::test]
async fn the_api_router_is_mounted_at_the_api_base() {
    let dir = std::env::temp_dir().join(format!("app-test-autobrr-ui-mount-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let st = server::AppState::open(&dir.join("data"), dir.join("static"), |_| None).await.unwrap();
    let api = Router::new().route("/config", axum::routing::get(|| async { axum::Json(json!({ "version": "x" })) }));
    let app = server::autobrr_ui::router(&st, api).with_state(st);
    let (s, _, body) = get(&app, "/autobrr/api/config", None).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, r#"{"version":"x"}"#));
    let (s, _, _) = get(&app, "/autobrr/api/missing", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_and_files_need_a_lantern_session() {
    let (app, _) = server("auth").await;
    let (s, h, _) = get(&app, "/autobrr/filters", None).await;
    assert_eq!((s, h[header::LOCATION].to_str().unwrap()), (StatusCode::SEE_OTHER, "/"));
    let (s, h, _) = get(&app, "/autobrr/", None).await;
    assert_eq!((s, h[header::LOCATION].to_str().unwrap()), (StatusCode::SEE_OTHER, "/"));
    let (s, _, body) = get(&app, "/autobrr/assets/index-BQQ523tm.js", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(detail(&body).contains("Sign in"));
    // what browsers fetch on their own stays open
    let (s, _, body) = get(&app, "/autobrr/favicon.ico", None).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "ico"));
    let (s, _, body) = get(&app, "/autobrr/robots.txt", None).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "User-agent: *"));
}

#[tokio::test]
async fn the_manifest_gets_the_base_url_like_render_manifest() {
    let (app, _) = server("manifest").await;
    let (s, h, body) = get(&app, "/autobrr/manifest.webmanifest", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CONTENT_TYPE], "application/manifest+json");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["name"].as_str(), v["start_url"].as_str(), v["scope"].as_str()), (Some("Lantern"), Some("/autobrr/"), Some("/autobrr/")));
}
