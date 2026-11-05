//! autobrr's web frontend (the fork in `ui/autobrr/`, built) at `/autobrr/`, served as autobrr serves it with its
//! base URL set to `/autobrr/`. The frontend sends its API calls to `<base URL>api/...`, so they reach Lantern at
//! `/autobrr/api/...` and the frontend stays as autobrr wrote it; [`router`] mounts the autobrr-compatible API
//! there (ported in [`crate::releases`]). Lantern's own web app stays at `/` (docs/DECISIONS.md).

// Ported from autobrr v1.90.0 (internal/http/web.go, internal/http/server.go), GPL-2.0-or-later.
use crate::AppState;
use axum::extract::{Request, State};
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use tower_http::services::ServeDir;

/// The base URL the frontend runs under (autobrr's `baseUrl` setting; autobrr's always ends in `/`).
pub const URL_BASE: &str = "/autobrr/";

/// Where the frontend's API calls arrive: `<base URL>api` (autobrr mounts its API router there).
pub const API_BASE: &str = "/autobrr/api";

/// autobrr's `validWebRoutes`: the first part of every path that is one of the frontend's pages.
const WEB_ROUTES: [&str; 8] = ["filters", "releases", "settings", "logs", "notifications", "onboard", "login", "logout"];

/// The routes: the page (at `/autobrr/` and every page path), `manifest.webmanifest`, every built file, and `api`
/// (the autobrr-compatible API, from track A-api) at [`API_BASE`]. `/autobrr` goes to `/autobrr/`. Every API path
/// `api` does not answer gets a plain 404.
pub fn router(st: &AppState, api: Router<AppState>) -> Router<AppState> {
    let files = ServeDir::new(&st.autobrr_ui_dir).append_index_html_on_directories(false).fallback(get(other).with_state(st.clone()));
    let guard = || axum::middleware::from_fn_with_state(st.clone(), signed_in_only);
    // inside a nest, "/" would be /autobrr itself; the base URL with its slash is a route of its own
    let web = Router::new()
        .route("/", get(|| async { Redirect::permanent(URL_BASE) }))
        .route("/index.html", get(page))
        .route("/manifest.webmanifest", get(manifest))
        .fallback_service(files)
        .layer(guard());
    Router::new()
        .route(URL_BASE, get(page).layer(guard()))
        .nest(API_BASE, api.fallback(api_missing))
        .nest(URL_BASE.trim_end_matches('/'), web)
}

/// An API path Lantern does not answer (yet).
async fn api_missing() -> Response {
    kit_server_core::err(404, "Lantern doesn't answer this autobrr request. Update Lantern, then try again.").into_response()
}

/// Lantern's sign-in in front of the pages and their files (autobrr's frontend asks its own API whether someone is
/// signed in; Lantern signs people in on its own page). A page asked for without a session goes to Lantern's
/// sign-in at `/`; anything else answers 401. The browser's own files (the manifest, the icons, `robots.txt`) are
/// open to everyone, as browsers fetch them without the session.
async fn signed_in_only(State(st): State<AppState>, req: Request, next: axum::middleware::Next) -> Response {
    if is_open(req.uri().path()) {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    if crate::auth::signed_in(&st, &parts).is_ok() {
        return next.run(Request::from_parts(parts, body)).await;
    }
    if is_page(parts.uri.path()) {
        Redirect::to("/").into_response()
    } else {
        kit_server_core::err(401, "Sign in to Lantern first, then open this page again.").into_response()
    }
}

/// The part of `path` after the base URL (`filters/3` for `/autobrr/filters/3`); inside the nest the path comes
/// without it.
fn rest(path: &str) -> &str {
    let path = path.strip_prefix(URL_BASE.trim_end_matches('/')).unwrap_or(path);
    path.strip_prefix('/').unwrap_or(path)
}

/// Whether `path` (with or without the base URL) is a file browsers fetch without a session: one at the top of
/// the build that is the manifest, an icon or `robots.txt`.
pub fn is_open(path: &str) -> bool {
    let file = rest(path);
    !file.contains('/') && (file == "manifest.webmanifest" || file == "robots.txt" || file.ends_with(".ico") || file.ends_with(".png"))
}

/// autobrr's `validWebRoute`: whether `path` (with or without the base URL) is one of the frontend's pages: the
/// base itself, `index.html`, or a path that starts with one of [`WEB_ROUTES`].
pub fn is_page(path: &str) -> bool {
    let file = rest(path);
    file.is_empty() || file == "index.html" || WEB_ROUTES.iter().any(|r| file.starts_with(r))
}

/// The build's `index.html` as autobrr's template fills it: the base URL (also for the files, autobrr's
/// `AssetBaseUrl`) and the instance name. A missing build is a 404 that says how to make it.
async fn page(State(st): State<AppState>) -> Response {
    match tokio::fs::read_to_string(st.autobrr_ui_dir.join("index.html")).await {
        Ok(text) => {
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], fill(&text, &st.app_name()))
                .into_response()
        }
        Err(_) => kit_server_core::err(
            404,
            "autobrr's pages are not built into this copy of Lantern. Build ui/autobrr (pnpm build) and set \
             APP_AUTOBRR_UI_DIR to its dist folder.",
        )
        .into_response(),
    }
}

/// autobrr's `IndexParams` put into a built file: `{{.BaseUrl}}` and `{{.AssetBaseUrl}}` become [`URL_BASE`],
/// `{{.InstanceName}}` (Lantern's, inside a script's string) becomes `name`, written so it can't end the string
/// or the script.
pub fn fill(text: &str, name: &str) -> String {
    let quoted = serde_json::to_string(name)
        .unwrap_or_else(|_| "\"Lantern\"".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    text.replace("{{.BaseUrl}}", URL_BASE).replace("{{.AssetBaseUrl}}", URL_BASE).replace("\"{{.InstanceName}}\"", &quoted)
}

/// `manifest.webmanifest` (`RenderManifest`): the build's manifest with the base URL put in.
async fn manifest(State(st): State<AppState>) -> Response {
    match tokio::fs::read_to_string(st.autobrr_ui_dir.join("manifest.webmanifest")).await {
        Ok(text) => ([(header::CONTENT_TYPE, "application/manifest+json")], fill(&text, &st.app_name())).into_response(),
        Err(_) => kit_server_core::err(404, "This file is not part of Lantern's autobrr pages.").into_response(),
    }
}

/// A path that is not a built file (autobrr's `/*` route): `favicon.ico` and the Inter font from anywhere, the page
/// for every page path, else a 404.
async fn other(State(st): State<AppState>, req: Request) -> Response {
    let path = req.uri().path();
    for file in ["favicon.ico", "Inter-Variable.woff2"] {
        if path.contains(file) {
            return match tokio::fs::read(st.autobrr_ui_dir.join(file)).await {
                Ok(bytes) => {
                    let kind = if file.ends_with(".ico") { "image/x-icon" } else { "font/woff2" };
                    ([(header::CONTENT_TYPE, kind)], bytes).into_response()
                }
                Err(_) => kit_server_core::err(404, "This file is not part of Lantern's autobrr pages.").into_response(),
            };
        }
    }
    if is_page(path) {
        return page(State(st)).await;
    }
    kit_server_core::err(404, "This file is not part of Lantern's autobrr pages.").into_response()
}
