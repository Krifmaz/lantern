//! Prowlarr's web frontend (the fork in `ui/prowlarr/`) at `/prowlarr/`: the page with the URL base put in, its
//! files, `initialize.json`, and sign-in, as Prowlarr's `StaticResourceController`, `HtmlMapperBase` and
//! `InitializeJsonController` answer them. Each test runs on a data folder of its own with a small stand-in build.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

/// The password the tests' owner signs in with.
const PASSWORD: &str = "correct horse battery";

/// Prowlarr's `frontend/src/index.ejs` as its production build writes it (the parts the server rewrites).
const INDEX: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <link rel="apple-touch-icon" sizes="180x180" href="/Content/Images/Icons/apple-touch-icon.png" />
    <link rel="manifest" href="/Content/manifest.json" crossorigin="use-credentials" />
    <link rel="shortcut icon" type="image/ico" href="/favicon.ico" data-no-hash />
    <link rel="stylesheet" type="text/css" href="/Content/Fonts/fonts.css">
    <script>
      window.Prowlarr = {
        urlBase: '__URL_BASE__'
      };
    </script>
    <script type="text/javascript" src="/index-6e6fa5d4dad52aa117d5.js" data-no-hash></script>
    <link href="/Content/styles.css" rel="stylesheet"></link>
  </head>
  <body><div id="root" class="root"></div></body>
</html>"#;

/// A fresh server with an owner `admin` and a stand-in Prowlarr build in `prowlarr`.
async fn server(name: &str) -> (Router, PathBuf) {
    let dir = std::env::temp_dir().join(format!("app-test-prowlarr-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ui = dir.join("prowlarr");
    std::fs::create_dir_all(ui.join("Content/Images")).unwrap();
    std::fs::write(ui.join("index.html"), INDEX).unwrap();
    std::fs::write(ui.join("index-6e6fa5d4dad52aa117d5.js"), "console.log(1)").unwrap();
    std::fs::write(ui.join("Content/styles.css"), "body{}").unwrap();
    std::fs::write(ui.join("Content/Images/logo.svg"), "<svg/>").unwrap();
    let ui_dir = ui.to_string_lossy().to_string();
    let st = server::AppState::open(&dir.join("data"), dir.join("static"), move |name| match name {
        "APP_API_KEY" => Some("test-key".into()),
        "APP_PROWLARR_UI_DIR" => Some(ui_dir.clone()),
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

#[test]
fn the_page_gets_the_url_base_like_html_mapper_base() {
    let html = server::prowlarr_ui::with_url_base(INDEX, "/prowlarr");
    for want in [
        r#"href="/prowlarr/Content/Images/Icons/apple-touch-icon.png""#,
        r#"href="/prowlarr/Content/manifest.json""#,
        r#"href="/prowlarr/favicon.ico" data-no-hash"#,
        r#"href="/prowlarr/Content/Fonts/fonts.css""#,
        r#"src="/prowlarr/index-6e6fa5d4dad52aa117d5.js" data-no-hash"#,
        r#"href="/prowlarr/Content/styles.css""#,
        "urlBase: '/prowlarr'",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert!(!html.contains("__URL_BASE__"));
}

#[test]
fn pages_are_paths_without_a_dot_outside_content() {
    for page in ["/prowlarr", "/prowlarr/", "/prowlarr/index.html", "/prowlarr/settings/general", "/prowlarr/system/status"] {
        assert!(server::prowlarr_ui::is_page(page), "{page}");
    }
    for file in ["/prowlarr/Content/styles.css", "/prowlarr/content/images", "/prowlarr/index-1.js", "/prowlarr/login"] {
        assert!(!server::prowlarr_ui::is_page(file), "{file}");
    }
}

#[tokio::test]
async fn every_page_is_the_frontend_under_prowlarr() {
    let (app, _) = server("pages").await;
    let cookie = sign_in(&app).await;
    for path in ["/prowlarr", "/prowlarr/", "/prowlarr/indexers", "/prowlarr/settings/general", "/prowlarr/system/status"] {
        let (s, h, body) = get(&app, path, Some(&cookie)).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(h[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"), "{path}");
        assert!(body.contains(r#"src="/prowlarr/index-6e6fa5d4dad52aa117d5.js""#), "{path}");
        assert!(body.contains("urlBase: '/prowlarr'"), "{path}");
    }
    // the built files as they are, a missing one is a 404 with a plain sentence
    let (s, _, body) = get(&app, "/prowlarr/index-6e6fa5d4dad52aa117d5.js", Some(&cookie)).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "console.log(1)"));
    let (s, _, body) = get(&app, "/prowlarr/missing-1.js", Some(&cookie)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(serde_json::from_str::<Value>(&body).unwrap()["detail"].as_str().unwrap().contains("not part of"));
    // Lantern's own web app keeps `/`
    let (s, _, _) = get(&app, "/api/health", None).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn only_content_is_open_without_signing_in() {
    let (app, _) = server("auth").await;
    let (s, _, body) = get(&app, "/prowlarr/Content/styles.css", None).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "body{}"));
    let (s, _, body) = get(&app, "/prowlarr/Content/Images/logo.svg", None).await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "<svg/>"));
    let (s, h, _) = get(&app, "/prowlarr/indexers", None).await;
    assert_eq!((s, h[header::LOCATION].to_str().unwrap()), (StatusCode::SEE_OTHER, "/"));
    for path in ["/prowlarr/initialize.json", "/prowlarr/index-6e6fa5d4dad52aa117d5.js"] {
        let (s, _, body) = get(&app, path, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{path}");
        assert!(!body.contains("test-key"), "{path}");
    }
}

#[tokio::test]
async fn the_manifest_names_the_instance_like_manifest_mapper() {
    let (app, dir) = server("manifest").await;
    std::fs::write(
        dir.join("prowlarr/Content/manifest.json"),
        r#"{"name": "__INSTANCE_NAME__", "start_url": "__URL_BASE__/", "display": "standalone"}"#,
    )
    .unwrap();
    let (s, _, body) = get(&app, "/prowlarr/Content/manifest.json", None).await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["name"].as_str(), v["start_url"].as_str()), (Some("Lantern"), Some("/prowlarr/")));
}

#[tokio::test]
async fn initialize_json_has_prowlarrs_fields_with_lanterns_api() {
    let (app, _) = server("init").await;
    let cookie = sign_in(&app).await;
    let (s, h, body) = get(&app, "/prowlarr/initialize.json?t=1", Some(&cookie)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    let v: Value = serde_json::from_str(&body).unwrap();
    // the same fields as Prowlarr's InitializeJsonController
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["analytics", "apiKey", "apiRoot", "branch", "instanceName", "isProduction", "release", "theme", "urlBase", "userHash", "version"]
    );
    assert_eq!(v["apiRoot"], "/api/v1");
    assert_eq!(v["apiKey"], "test-key");
    assert_eq!(v["urlBase"], "/prowlarr");
    assert_eq!(v["analytics"], false);
    assert_eq!(v["instanceName"], "Lantern");
    assert_eq!(v["version"], server::VERSION);
}
