//! A site that checks browsers the way Cloudflare does, for the solver test and the image check
//! (scripts/image-solver.sh): a "Just a moment..." page whose script sets `cf_clearance` and loads the page again.
//!
//! | Path | What |
//! | --- | --- |
//! | `/page` | lets anyone with the cookie (and a person's user agent) through |
//! | `/strict` | lets only browsers through, cookie or not |
//! | `/blocked` | Cloudflare's outright refusal |
#![allow(dead_code)]

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The check page: after a short wait its script sets the cookie and loads the page again.
const CHECK: &str = r#"<!doctype html><html><head><title>Just a moment...</title></head><body>
<div id="challenge-spinner">Checking your browser</div>
<script>setTimeout(function () { document.cookie = "cf_clearance=passed; path=/"; location.reload(); }, 700);</script>
</body></html>"#;

/// What the site counts.
#[derive(Default)]
pub struct Site {
    /// Requests from a browser (they send Sec-Fetch-Mode).
    pub browser: AtomicUsize,
    /// Requests from Lantern's own client that got through with the cookie.
    pub plain_through: AtomicUsize,
}

fn passed(h: &HeaderMap) -> bool {
    h.get(header::COOKIE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("cf_clearance=passed"))
        && h.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).is_some_and(|u| !u.contains("Headless"))
}

fn check() -> Response {
    (StatusCode::FORBIDDEN, [("server", "cloudflare"), ("cf-mitigated", "challenge")], Html(CHECK)).into_response()
}

/// `/page`: lets anyone with the cookie (and a person's user agent) through.
async fn page(State(s): State<Arc<Site>>, h: HeaderMap) -> Response {
    let from_browser = h.contains_key("sec-fetch-mode");
    if from_browser {
        s.browser.fetch_add(1, Ordering::SeqCst);
    }
    if !passed(&h) {
        return check();
    }
    if !from_browser {
        s.plain_through.fetch_add(1, Ordering::SeqCst);
    }
    Html("<html><head><title>Results</title></head><body><table id=\"torrents\"><tr><td>Show.S01E01</td></tr></table></body></html>")
        .into_response()
}

/// `/strict`: only browsers get through, cookie or not.
async fn strict(State(s): State<Arc<Site>>, h: HeaderMap) -> Response {
    if !h.contains_key("sec-fetch-mode") {
        return check();
    }
    s.browser.fetch_add(1, Ordering::SeqCst);
    if !passed(&h) {
        return check();
    }
    Html("<html><head><title>Strict</title></head><body><p class=\"only-browsers\">through</p></body></html>").into_response()
}

/// `/blocked`: Cloudflare's outright refusal.
async fn blocked() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::SERVER, "cloudflare")],
        Html("<html><head><title>Access denied</title></head><body>Access denied</body></html>"),
    )
        .into_response()
}

/// The site's routes, counting into `site`.
pub fn router(site: Arc<Site>) -> Router {
    Router::new().route("/page", get(page)).route("/strict", get(strict)).route("/blocked", get(blocked)).with_state(site)
}

/// Starts the site on `addr` (`127.0.0.1:0` for any free port): its address and its counts.
pub async fn start_site_on(addr: &str) -> (String, Arc<Site>) {
    let site = Arc::new(Site::default());
    let app = router(site.clone());
    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), site)
}
