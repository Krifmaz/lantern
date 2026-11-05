//! A site that checks browsers the way Cloudflare (and DDoS-Guard and friends) do, for the solver tests and the
//! image check (scripts/image-solver.sh): each check page's script sets the site's cookie and loads the page
//! again. The pages stand in for the live sites FlareSolverr's own `src/tests.py` uses.
//!
//! | Path | What |
//! | --- | --- |
//! | `/page` | lets anyone with `cf_clearance` (and a person's user agent) through; a POST gets nginx's 405 |
//! | `/strict` | lets only browsers through, cookie or not |
//! | `/blocked` | Cloudflare's outright refusal ("Access denied") |
//! | `/attention` | Cloudflare's other refusal title ("Attention Required! \| Cloudflare") |
//! | `/denied` | a refusal told by its error box, not its title |
//! | `/ddos` | DDoS-Guard's check (`__ddg1_`) |
//! | `/fairlane` | Fairlane's check (`fl_pass_v2_b`), told by its box |
//! | `/custom` | the custom Cloudflare check of MuziekFabriek and friends (`ct_anti_ddos_key`) |
//! | `/verify` | a check only a click on "Verify you are human" passes |
//! | `/turnstile` | a Turnstile box that fills in its token when ticked |
//! | `/plain` | an ordinary page with a cookie (FlareSolverr's tests use Google) |
//! | `/post` | echoes a form like httpbin.org/post |
//! | `/media` | a page with an image and a style sheet, counting their loads |
//! | `/env` | the page's time zone (by script) and the `Accept-Language` it was sent |
#![allow(dead_code)]

use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A check page titled `title`, with `body`, whose script sets `cookie` and loads the page again after a moment.
fn check_page(title: &str, body: &str, cookie: &str) -> String {
    check_page_then(title, body, cookie, "location.reload();")
}

/// [`check_page`] that runs `again` to load the page again.
fn check_page_then(title: &str, body: &str, cookie: &str, again: &str) -> String {
    format!(
        r#"<!doctype html><html><head><title>{title}</title></head><body>
{body}
<script>setTimeout(function () {{ document.cookie = "{cookie}; path=/"; {again} }}, 1000);</script>
</body></html>"#
    )
}

/// The Cloudflare check's cookie.
const CLEARANCE: &str = "cf_clearance=passed-the-check-0123456789abcdefghij";

/// The Cloudflare check page.
fn cloudflare_check() -> String {
    check_page("Just a moment...", r#"<div id="challenge-spinner">Checking your browser</div>"#, CLEARANCE)
}

/// The Cloudflare check page for a POST: it sends the form again once the cookie is set.
fn cloudflare_post_check() -> String {
    check_page_then(
        "Just a moment...",
        r#"<div id="challenge-spinner">Checking your browser</div><form id="again" method="POST"><input type="hidden" name="again" value="1"></form>"#,
        CLEARANCE,
        "document.getElementById('again').submit();",
    )
}

/// What the site counts.
#[derive(Default)]
pub struct Site {
    /// Requests from a browser (they send Sec-Fetch-Mode).
    pub browser: AtomicUsize,
    /// Requests from Lantern's own client that got through with the cookie.
    pub plain_through: AtomicUsize,
    /// Loads of `/media`'s image and style sheet.
    pub media: AtomicUsize,
}

/// The request's cookies.
fn cookies(h: &HeaderMap) -> String {
    h.get(header::COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

fn passed(h: &HeaderMap) -> bool {
    cookies(h).contains("cf_clearance=passed")
        && h.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).is_some_and(|u| !u.contains("Headless"))
}

fn check() -> Response {
    (StatusCode::FORBIDDEN, [("server", "cloudflare"), ("cf-mitigated", "challenge")], Html(cloudflare_check())).into_response()
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

/// `POST /page`: the check, then nginx's refusal of the method (as nowsecure.nl answers FlareSolverr's test).
async fn page_post(h: HeaderMap) -> Response {
    if !passed(&h) {
        return (StatusCode::FORBIDDEN, [("server", "cloudflare")], Html(cloudflare_post_check())).into_response();
    }
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Html("<html><head><title>405 Not Allowed</title></head><body><center><h1>405 Not Allowed</h1></center></body></html>"),
    )
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

/// `/attention`: Cloudflare's refusal with its other title.
async fn attention() -> Response {
    (StatusCode::FORBIDDEN, Html("<html><head><title>Attention Required! | Cloudflare</title></head><body>Sorry</body></html>"))
        .into_response()
}

/// `/denied`: a refusal told by its error box.
async fn denied() -> Response {
    let body = r#"<html><head><title>example.org</title></head><body><div id="cf-error-details"><div class="cf-error-overview">
<h1>Sorry, you have been blocked</h1></div></div></body></html>"#;
    (StatusCode::FORBIDDEN, Html(body)).into_response()
}

/// A page that shows a check until the request has `cookie`, then `title`.
fn checked(h: &HeaderMap, cookie: &str, check: String, title: &str) -> Response {
    if cookies(h).contains(cookie) {
        Html(format!("<html><head><title>{title}</title></head><body><p>through</p></body></html>")).into_response()
    } else {
        (StatusCode::FORBIDDEN, Html(check)).into_response()
    }
}

/// `/ddos`: DDoS-Guard's check.
async fn ddos(h: HeaderMap) -> Response {
    checked(&h, "__ddg1_=", check_page("DDoS-Guard", "<p>Checking your browser</p>", "__ddg1_=ddos-guard-pass"), "Литрес")
}

/// `/fairlane`: Fairlane's check, told by its box.
async fn fairlane(h: HeaderMap) -> Response {
    let check = check_page(
        "pararius.com",
        r#"<div class="vc"><div class="text-box"><h2>One moment please...</h2></div></div>"#,
        "fl_pass_v2_b=fairlane-pass-0123456789abcdefghijklmnopqrstuvwxyz0123456789abcd",
    );
    checked(&h, "fl_pass_v2_b=", check, "Rental Apartments Amsterdam")
}

/// `/custom`: the custom Cloudflare check of MuziekFabriek and friends.
async fn custom(h: HeaderMap) -> Response {
    let check = check_page(
        "muziekfabriek.org",
        r#"<table><tr><td class="info"><div id="js_info">Please wait</div></td></tr></table>"#,
        "ct_anti_ddos_key=custom-pass-key",
    );
    checked(&h, "ct_anti_ddos_key=", check, "MuziekFabriek : Aanmelden")
}

/// `/verify`: a check only a click on its button (or the keyboard) passes.
async fn verify(h: HeaderMap) -> Response {
    let check = r#"<!doctype html><html><head><title>Just a moment...</title></head><body>
<input type="button" value="Verify you are human" style="margin:40px;width:200px;height:40px"
 onclick="document.cookie = 'cf_clearance=clicked; path=/'; location.reload();">
</body></html>"#;
    checked(&h, "cf_clearance=clicked", check.to_string(), "Verified")
}

/// `/turnstile`: a Turnstile box (the page's first focusable element) that fills in its token when ticked.
async fn turnstile() -> Html<&'static str> {
    Html(
        r#"<!doctype html><html><head><title>Turnstile demo</title></head><body>
<form><input type="hidden" name="cf-turnstile-response" value="">
<input type="checkbox" id="box" onchange="document.querySelector('input[name=cf-turnstile-response]').value = '0.turnstile-token-' + Date.now();">
</form></body></html>"#,
    )
}

/// `/plain`: an ordinary page that sets a cookie.
async fn plain() -> Response {
    (
        [(header::SET_COOKIE, "NID=plain-cookie; Path=/; Max-Age=3600")],
        Html("<html><head><title>Google</title></head><body>Search</body></html>"),
    )
        .into_response()
}

/// `POST /post`: the form back, written the way httpbin.org writes it.
async fn post(Form(form): Form<BTreeMap<String, String>>) -> Response {
    let fields: Vec<String> = form.iter().map(|(k, v)| format!("    {}: {}", serde_json::json!(k), serde_json::json!(v))).collect();
    let body = format!("{{\n  \"args\": {{}}, \n  \"form\": {{\n{}\n  }}\n}}\n", fields.join(", \n"));
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// `/media`: a page with an image and a style sheet.
async fn media() -> Html<&'static str> {
    Html(r#"<html><head><title>Media</title><link rel="stylesheet" href="/style.css"></head><body><img src="/pic.png"></body></html>"#)
}

/// `/pic.png` and `/style.css`, counted.
async fn media_file(State(s): State<Arc<Site>>) -> Response {
    s.media.fetch_add(1, Ordering::SeqCst);
    (StatusCode::OK, "x").into_response()
}

/// `/env`: the time zone the page's script sees and the `Accept-Language` it was sent.
async fn env(h: HeaderMap) -> Html<String> {
    let lang = h.get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    Html(format!(
        r#"<html><head><title>Env</title></head><body><p id="lang">lang={lang}</p><p id="tz"></p>
<script>document.getElementById('tz').textContent = 'tz=' + Intl.DateTimeFormat().resolvedOptions().timeZone;</script></body></html>"#
    ))
}

/// The site's routes, counting into `site`.
pub fn router(site: Arc<Site>) -> Router {
    Router::new()
        .route("/page", get(page).post(page_post))
        .route("/strict", get(strict))
        .route("/blocked", get(blocked))
        .route("/attention", get(attention))
        .route("/denied", get(denied))
        .route("/ddos", get(ddos))
        .route("/fairlane", get(fairlane))
        .route("/custom", get(custom))
        .route("/verify", get(verify))
        .route("/turnstile", get(turnstile))
        .route("/plain", get(plain))
        .route("/post", axum::routing::post(post))
        .route("/media", get(media))
        .route("/pic.png", get(media_file))
        .route("/style.css", get(media_file))
        .route("/env", get(env))
        .with_state(site)
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

/// What the proxy counts.
#[derive(Default)]
pub struct ProxyHits {
    /// Requests it refused for want of the right sign-in.
    pub refused: AtomicUsize,
    /// Requests it passed on.
    pub passed: AtomicUsize,
}

/// Starts an HTTP proxy that wants `user` and `pass` (Basic) and sends every request on to `site` (whatever host
/// the browser asked for): its address (`http://127.0.0.1:port`) and its counts.
pub async fn start_proxy(site: String, user: &str, pass: &str) -> (String, Arc<ProxyHits>) {
    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let want = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}")));
    let hits = Arc::new(ProxyHits::default());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let counts = hits.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        loop {
            let Ok((mut sock, _)) = l.accept().await else { return };
            let (want, site, client, counts) = (want.clone(), site.clone(), client.clone(), counts.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let auth = head.lines().find_map(|l| {
                    l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("proxy-authorization")).map(|(_, v)| v.trim().to_string())
                });
                if auth.as_deref() != Some(want.as_str()) {
                    counts.refused.fetch_add(1, Ordering::SeqCst);
                    let _ = sock
                        .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"lantern\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                }
                counts.passed.fetch_add(1, Ordering::SeqCst);
                let path = url::Url::parse(&target).map(|u| u[url::Position::BeforePath..].to_string()).unwrap_or(target);
                let (status, ctype, body) = match client.get(format!("{site}{path}")).send().await {
                    Ok(r) => {
                        let status = r.status().as_u16();
                        let ctype = r.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("text/html").to_string();
                        (status, ctype, r.bytes().await.unwrap_or_default().to_vec())
                    }
                    Err(_) => (502, "text/plain".into(), b"bad gateway".to_vec()),
                };
                let head =
                    format!("HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
            });
        }
    });
    (format!("http://{addr}"), hits)
}
