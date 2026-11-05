// Ported from FlareSolverr v3.5.2 (src/flaresolverr_service.py, src/sessions.py, src/utils.py), MIT.
//! Cloudflare solving: a Chromium that opens a page the way a person's browser does, waits while Cloudflare (or
//! DDoS-Guard) checks it, and hands back the page, its cookies and the browser's user agent — FlareSolverr's
//! logic, step by step, on one shared browser.
//!
//! | Part | What |
//! | --- | --- |
//! | [`Solver`] | one browser for all requests, started when first needed and closed after five idle minutes |
//! | [`Solver::run`] | one FlareSolverr request ([`Job`]): a fresh browser context (or a session's), the page, the check |
//! | [`Solver::solve`] | the same for Lantern's own indexers ([`SolveRequest`]), in plain words |
//! | [`Solver::clearance`] | the cookies a site gave after its check, kept per site (and proxy) in `solver_clearance` |
//! | [`api`] | the FlareSolverr API (`POST /v1`), for apps that call a solver |
//! | [`config`] | FlareSolverr's environment settings |
//! | [`metrics`] | FlareSolverr's Prometheus metrics |
//!
//! FlareSolverr starts one Chrome per request (and per session); Lantern opens a browser context in one shared
//! Chromium instead, which keeps cookies and proxies apart the same way. Indexers use the solver by themselves:
//! when a site answers with a check, the indexer's HTTP client asks the solver once, keeps the cookies and the user
//! agent, and sends both with its later requests to that site.

pub mod api;
pub mod config;
pub mod metrics;

pub use config::Config;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
use chromiumoxide::cdp::browser_protocol::network::{self, CookieParam, CookieSameSite, DeleteCookiesParams, TimeSinceEpoch};
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, NavigateParams};
use chromiumoxide::cdp::browser_protocol::target::{CreateBrowserContextParams, CreateTargetParams};
use chromiumoxide::layout::Point;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::Page;
use futures_util::StreamExt;
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

/// How long the browser stays open with nothing to do.
pub const IDLE: Duration = Duration::from_secs(300);

/// How many pages the browser opens at once; more requests wait their turn.
pub const PAGES_AT_ONCE: usize = 3;

/// How long the browser may take to start. A busy or slow machine (a small NAS, a CI runner starting several
/// browsers at once) can need far more than the usual second or two, so this is generous.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a site's cookies are kept when the site doesn't say (cf_clearance usually lasts longer).
pub const CLEARANCE_DEFAULT: Duration = Duration::from_secs(3600);

/// The page titles of a request Cloudflare refused outright (a title that starts with one).
pub const ACCESS_DENIED_TITLES: [&str; 2] = ["Access denied", "Attention Required! | Cloudflare"];

/// The parts of a page that mean Cloudflare refused the request outright.
pub const ACCESS_DENIED_SELECTORS: [&str; 2] = ["div.cf-error-title span.cf-code-label span", "#cf-error-details div.cf-error-overview h1"];

/// The page titles of a check that is still running (Cloudflare, DDoS-Guard), in any case.
pub const CHALLENGE_TITLES: [&str; 2] = ["Just a moment...", "DDoS-Guard"];

/// The parts of a page that mean a check is still running: Cloudflare's, the custom Cloudflare page of
/// EbookParadijs and friends, and Fairlane's (pararius.com).
pub const CHALLENGE_SELECTORS: [&str; 10] = [
    "#cf-challenge-running",
    ".ray_id",
    ".attack-box",
    "#cf-please-wait",
    "#challenge-spinner",
    "#trk_jschal_js",
    "#turnstile-wrapper",
    ".lds-ring",
    "td.info #js_info",
    "div.vc div.text-box h2",
];

/// The field a Turnstile box fills with its token.
pub const TURNSTILE_SELECTORS: [&str; 1] = ["input[name='cf-turnstile-response']"];

/// The images, styles and fonts `disableMedia` blocks.
pub const MEDIA_PATTERNS: [&str; 42] = [
    "*.png", "*.jpg", "*.jpeg", "*.gif", "*.webp", "*.bmp", "*.svg", "*.ico", "*.PNG", "*.JPG", "*.JPEG", "*.GIF", "*.WEBP", "*.BMP",
    "*.SVG", "*.ICO", "*.tiff", "*.tif", "*.jpe", "*.apng", "*.avif", "*.heic", "*.heif", "*.TIFF", "*.TIF", "*.JPE", "*.APNG", "*.AVIF",
    "*.HEIC", "*.HEIF", "*.css", "*.CSS", "*.woff", "*.woff2", "*.ttf", "*.otf", "*.eot", "*.WOFF", "*.WOFF2", "*.TTF", "*.OTF", "*.EOT",
];

/// The tables the solver keeps.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS solver_clearance (
            host TEXT NOT NULL,
            proxy TEXT NOT NULL DEFAULT '',
            cookies TEXT NOT NULL,
            user_agent TEXT NOT NULL,
            until REAL NOT NULL,
            PRIMARY KEY (host, proxy)
        );",
    )
    .map_err(|e| anyhow::anyhow!("The solver's table can't be made: {}", e.detail))
}

/// One cookie of a solved page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SolvedCookie {
    /// The cookie's name.
    pub name: String,
    /// Its value.
    pub value: String,
    /// The domain it is for (`.example.com`).
    pub domain: String,
    /// The path it is for.
    pub path: String,
    /// When it runs out, in seconds since 1970 (-1 for a session cookie).
    pub expires: f64,
    /// Its size in bytes.
    #[serde(default)]
    pub size: i64,
    /// Whether scripts can't read it.
    pub http_only: bool,
    /// Whether it is only sent over HTTPS.
    pub secure: bool,
    /// Whether it lasts only for the browser session.
    #[serde(default)]
    pub session: bool,
    /// `Strict`, `Lax` or `None`, when the site said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
}

/// A request from Lantern's own indexers.
#[derive(Debug, Clone, Default)]
pub struct SolveRequest {
    /// The page to open.
    pub url: String,
    /// Form fields to POST (`a=1&b=2`), or `None` for a GET.
    pub post_data: Option<String>,
    /// How long to wait for the check, at most.
    pub timeout: Duration,
    /// A proxy for this request (`http://host:port`, `socks5://host:port`), or "".
    pub proxy: String,
    /// Cookies to set before opening the page.
    pub cookies: Vec<(String, String)>,
    /// The session to use (its cookies stay between requests), or "" for a fresh one.
    pub session: String,
    /// Leave the page's HTML out of the answer (only the cookies are wanted).
    pub cookies_only: bool,
}

/// A proxy: FlareSolverr's `proxy` object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proxy {
    /// `http://host:port`, `socks5://host:port`.
    pub url: String,
    /// The user name and password, when the request gave all three of `url`, `username` and `password`.
    pub credentials: Option<(String, String)>,
}

/// A form to POST.
#[derive(Debug, Clone, PartialEq)]
pub struct PostForm {
    /// The fields, `a=1&b=2`.
    pub data: String,
    /// Build the form the way FlareSolverr does (its API), rather than Lantern's way for its own indexers.
    pub flaresolverr: bool,
}

/// One cookie a request asks to set: FlareSolverr's cookie object, checked, or the error Python would raise.
pub type CookieIn = Result<CookieSpec, String>;

/// A cookie to set (Selenium's `add_cookie` fields).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CookieSpec {
    /// Its name.
    pub name: String,
    /// Its value (`None`: the browser refuses it, as chromedriver does).
    pub value: Option<String>,
    /// Its domain; the page's own when not given.
    pub domain: Option<String>,
    /// Its path.
    pub path: Option<String>,
    /// HTTPS only.
    pub secure: Option<bool>,
    /// Hidden from scripts.
    pub http_only: Option<bool>,
    /// When it runs out, in seconds since 1970.
    pub expiry: Option<f64>,
    /// `Strict`, `Lax` or `None`.
    pub same_site: Option<String>,
}

/// One FlareSolverr request, as `_resolve_challenge` and `_evil_logic` see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// The page to open.
    pub url: String,
    /// The form to POST, or `None` for a GET.
    pub post: Option<PostForm>,
    /// Cookies to set after the first load (then the page is loaded again).
    pub cookies: Vec<CookieIn>,
    /// The proxy for a request without a session.
    pub proxy: Option<Proxy>,
    /// The session to use, or `None` for a fresh browser context.
    pub session: Option<String>,
    /// A session older than this is made again before use.
    pub session_ttl: Option<Duration>,
    /// How long the page may take, in milliseconds (at least 1).
    pub timeout_ms: u64,
    /// Leave out the HTML (and the headers).
    pub return_only_cookies: bool,
    /// Add a screenshot (PNG, base64).
    pub return_screenshot: bool,
    /// Seconds to wait before reading the HTML.
    pub wait_in_seconds: f64,
    /// Block images, styles and fonts (`None`: `DISABLE_MEDIA`).
    pub disable_media: Option<bool>,
    /// Tick a Turnstile box after this many Tab presses (`Some(None)`: given, but not a whole number).
    pub tabs_till_verify: Option<Option<i64>>,
}

impl Default for Job {
    fn default() -> Job {
        Job {
            url: String::new(),
            post: None,
            cookies: vec![],
            proxy: None,
            session: None,
            session_ttl: None,
            timeout_ms: 60_000,
            return_only_cookies: false,
            return_screenshot: false,
            wait_in_seconds: 0.0,
            disable_media: None,
            tabs_till_verify: None,
        }
    }
}

/// What a solved request gives.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    /// The address the page ended on.
    pub url: String,
    /// The page's HTTP status (FlareSolverr's API always says 200).
    pub status: u16,
    /// The page's HTML ("" when only cookies were asked for).
    pub response: String,
    /// The cookies the browser has for the page.
    pub cookies: Vec<SolvedCookie>,
    /// The user agent the browser sent; requests that reuse the cookies must send the same one.
    pub user_agent: String,
    /// The Turnstile token, when a box was ticked for one.
    pub turnstile_token: Option<String>,
    /// The page as a PNG (base64), when asked for.
    pub screenshot: Option<String>,
    /// Whether there was a check to get past.
    #[serde(skip)]
    pub challenged: bool,
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// No browser program was found.
    NoBrowser,
    /// The browser didn't start or answer (the words say why).
    Browser(String),
    /// The page took longer than the request allowed (milliseconds).
    Timeout {
        /// The page's host.
        host: String,
        /// The time allowed, in milliseconds.
        ms: u64,
    },
    /// Cloudflare refused the request outright.
    Blocked {
        /// The page's host.
        host: String,
    },
    /// The browser said no (chromedriver's words, as `unknown error: net::ERR_NAME_NOT_RESOLVED`).
    Driver {
        /// The page's host.
        host: String,
        /// The browser's words.
        message: String,
    },
    /// Anything else, in the words FlareSolverr (Python) uses.
    Failed(String),
}

impl SolveError {
    /// The text after `Error: ` in FlareSolverr's answer for a failed `request.*`, with the browser's version
    /// `chrome` for the browser's own errors.
    pub fn flaresolverr(&self, chrome: &str) -> String {
        let inner = match self {
            SolveError::Timeout { ms, .. } => {
                return format!("Error solving the challenge. Timeout after {} seconds.", py_float(*ms as f64 / 1000.0))
            }
            SolveError::Blocked { .. } => {
                "Cloudflare has blocked this request. Probably your IP is banned for this site, check in your web browser.".to_string()
            }
            SolveError::Driver { message, .. } => format!("Message: {message}\n  (Session info: chrome={chrome})\n"),
            SolveError::NoBrowser | SolveError::Browser(_) => self.to_string(),
            SolveError::Failed(s) => s.clone(),
        };
        format!("Error solving the challenge. {}", inner.replace('\n', "\\n"))
    }
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::NoBrowser => f.write_str(
                "There is no browser for Cloudflare checks: set APP_CHROMIUM to a Chromium program, or use the Lantern image, which has one",
            ),
            SolveError::Browser(s) | SolveError::Failed(s) => f.write_str(s),
            SolveError::Timeout { host, ms } => {
                write!(f, "{host} didn't get past its check in {} s; try again, or give it longer", py_float(*ms as f64 / 1000.0))
            }
            SolveError::Blocked { host } => write!(
                f,
                "Cloudflare refused the request to {host} outright (Access denied): the address Lantern runs from may be blocked; try a proxy"
            ),
            SolveError::Driver { host, message } => {
                let what = if message.contains("ERR_NAME_NOT_RESOLVED") {
                    "can't be found (DNS); check its address"
                } else if message.contains("ERR_PROXY") || message.contains("ERR_TUNNEL") {
                    "can't be reached through the proxy; check the proxy"
                } else if message.contains("ERR_CONNECTION") || message.contains("ERR_ADDRESS") {
                    "can't be reached (connection refused or no route)"
                } else if message.contains("ERR_CERT") || message.contains("ERR_SSL") {
                    "has a certificate problem (HTTPS)"
                } else {
                    "didn't open in the browser"
                };
                write!(f, "{host} {what} ({message})")
            }
        }
    }
}

/// A number the way Python's `str(float)` writes it (`60.0`, `0.01`).
pub fn py_float(x: f64) -> String {
    format!("{x:?}")
}

/// A site's cookies after its check, and the user agent they belong to.
#[derive(Debug, Clone, PartialEq)]
pub struct Clearance {
    /// `name=value` pairs.
    pub cookies: Vec<(String, String)>,
    /// The user agent to send with them.
    pub user_agent: String,
    /// When they run out.
    pub until: f64,
}

impl Clearance {
    /// The cookies as a `Cookie` header (`a=1; b=2`).
    pub fn header(&self) -> String {
        self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }
}

/// The running browser.
struct Running {
    /// The browser.
    browser: Arc<Browser>,
    /// The task reading the browser's messages; finished when the browser is gone.
    handler: tokio::task::JoinHandle<()>,
    /// When a request last asked for the browser.
    last_used: Instant,
    /// Which start of the browser this is (sessions made in an earlier one need a new context).
    generation: u64,
}

/// A FlareSolverr session: a browser context whose cookies last until it is destroyed.
#[derive(Debug, Clone)]
struct Session {
    /// The browser context.
    context: BrowserContextId,
    /// The proxy it was made with.
    proxy: Option<Proxy>,
    /// When it was made (for `session_ttl_minutes`).
    created: Instant,
    /// The browser start it belongs to.
    generation: u64,
}

/// The solver. Clones share the browser.
#[derive(Clone)]
pub struct Solver {
    inner: Arc<Inner>,
}

/// What the clones of a [`Solver`] share.
struct Inner {
    /// The browser program, when one was found.
    program: Option<PathBuf>,
    /// The folder for the browser's profile and its home (in the data folder).
    work: PathBuf,
    /// Where sites' cookies are kept between restarts.
    db: Option<SqliteDb>,
    /// FlareSolverr's settings.
    config: Config,
    /// FlareSolverr's metrics.
    metrics: Arc<metrics::Metrics>,
    /// The browser while it runs.
    running: AsyncMutex<Option<Running>>,
    /// Turns for opening pages.
    pages: Semaphore,
    /// The user agent the browser sends ("Headless" taken out), once known.
    user_agent: Mutex<String>,
    /// Sites' cookies by (host, proxy).
    clearances: Mutex<HashMap<(String, String), Clearance>>,
    /// The sessions, in the order they were made.
    sessions: AsyncMutex<Vec<(String, Session)>>,
    /// How many times the browser started.
    generation: AtomicU64,
}

/// The places a Chromium is looked for when `APP_CHROMIUM` doesn't name one: the image's, then the usual
/// system ones, real programs before the launchers that may stand in their place.
pub const PROGRAMS: [&str; 7] = [
    "/usr/lib/chromium/headless_shell",
    "/usr/lib/chromium/chromium",
    "/opt/google/chrome/chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium-headless-shell",
    "/usr/bin/chromium",
];

/// Whether `p` is a program that can be run as the browser itself: a file that isn't a launcher script (a
/// script, or Ubuntu's snap, starts a sandboxed copy that can't run here).
fn runnable(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut head = [0u8; 2];
    use std::io::Read;
    let snap = std::fs::canonicalize(p).is_ok_and(|real| real.starts_with("/snap") || real.ends_with("snap"));
    p.is_file() && !snap && f.read_exact(&mut head).is_ok() && &head != b"#!"
}

/// The browser program: `named` when it is a file, otherwise the first of [`PROGRAMS`] that is a program
/// itself (not a launcher script).
pub fn find_program(named: Option<&str>) -> Option<PathBuf> {
    if let Some(n) = named.map(str::trim).filter(|n| !n.is_empty()) {
        let p = PathBuf::from(n);
        return p.is_file().then_some(p);
    }
    PROGRAMS.iter().map(PathBuf::from).find(|p| runnable(p))
}

/// The browser's user agent with every "headless" (any case) taken out, as FlareSolverr does: sites block the
/// headless name.
pub fn person_agent(ua: &str) -> String {
    let lower = ua.to_ascii_lowercase();
    let mut out = String::with_capacity(ua.len());
    let mut i = 0;
    while i < ua.len() {
        if lower[i..].starts_with("headless") {
            i += "headless".len();
        } else {
            let c = ua[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// The browser's version from its user agent (`Chrome/142.0.7444.59`), "" when it doesn't say.
pub fn chrome_version(ua: &str) -> String {
    ua.split_whitespace().find_map(|w| w.strip_prefix("Chrome/")).unwrap_or("").to_string()
}

/// The host of `url`, lowercased ("" when it has none).
pub fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_default()
}

/// Whether a cookie for `domain` (`.example.com` or `example.com`) is sent to `host`.
pub fn domain_matches(domain: &str, host: &str) -> bool {
    let d = domain.trim_start_matches('.').to_lowercase();
    host == d || host.ends_with(&format!(".{d}"))
}

/// Python's `urllib.parse.unquote`: `%XX` decoded as UTF-8 (a bad byte becomes U+FFFD), `+` kept.
pub fn py_unquote(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// What Python's `urllib.parse.quote` leaves as it is: letters, digits, `_.-~` and `/`.
const PY_QUOTE: &percent_encoding::AsciiSet =
    &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b'/');

/// Python's `urllib.parse.quote` (UTF-8, `/` kept).
pub fn py_quote(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, PY_QUOTE).to_string()
}

/// Python's `html.escape` (quotes too).
pub fn py_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#x27;")
}

/// The `data:` address FlareSolverr opens for `request.post` (`_post_request`): a form named `hackForm` with
/// one text field per `postData` pair (a `submit` field left out) that sends itself to `url`.
pub fn flaresolverr_post_url(url: &str, post_data: &str) -> String {
    let mut form = format!(r#"<form id="hackForm" action="{url}" method="POST">"#);
    let query = post_data.strip_prefix('?').unwrap_or(post_data);
    for pair in query.split('&') {
        let (name, value) = match pair.split_once('=') {
            Some((n, v)) => (py_unquote(n), py_unquote(v)),
            None => (py_unquote(pair), String::new()),
        };
        if name == "submit" {
            continue;
        }
        let value = value.replace('"', "&quot;");
        form.push_str(&format!(
            r#"<input type="text" name="{}" value="{}"><br>"#,
            py_escape(&py_quote(&name)),
            py_escape(&py_quote(&value))
        ));
    }
    form.push_str("</form>");
    let html = format!(
        "\n        <!DOCTYPE html>\n        <html>\n        <body>\n            {form}\n            <script>document.getElementById('hackForm').submit();</script>\n        </body>\n        </html>"
    );
    format!("data:text/html;charset=utf-8,{html}")
}

/// A page that POSTs `post_data` (`a=1&b=2`) to `url` as a form as soon as it opens, for Lantern's own indexers
/// (the fields keep the encoding the indexer gave them).
pub fn post_page(url: &str, post_data: &str) -> String {
    let esc = |s: &str| html_escape::encode_double_quoted_attribute(s).into_owned();
    let fields: String = url::form_urlencoded::parse(post_data.as_bytes())
        .map(|(k, v)| format!(r#"<input type="hidden" name="{}" value="{}">"#, esc(&k), esc(&v)))
        .collect();
    format!(
        r#"<!doctype html><html><body><form id="f" method="POST" action="{}">{fields}</form><script>document.getElementById('f').submit();</script></body></html>"#,
        esc(url)
    )
}

/// The `data:` address of [`post_page`].
fn post_page_url(url: &str, post_data: &str) -> String {
    let html = post_page(url, post_data);
    format!("data:text/html;charset=utf-8,{}", percent_encoding::utf8_percent_encode(&html, percent_encoding::NON_ALPHANUMERIC))
}

/// A JavaScript string literal of `s`.
fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `Network.setBlockedURLs` with the older `urls` list that FlareSolverr sends.
#[derive(Debug, Clone, Serialize)]
struct BlockUrls {
    /// The patterns (`*.png`).
    urls: Vec<String>,
}

impl chromiumoxide::types::Method for BlockUrls {
    fn identifier(&self) -> chromiumoxide::types::MethodId {
        "Network.setBlockedURLs".into()
    }
}

impl chromiumoxide::types::Command for BlockUrls {
    type Response = serde_json::Value;
}

impl Solver {
    /// A solver that runs `program` (none: every request answers that there is no browser), with its profile in
    /// `<data>/browser`, keeping sites' cookies in `db`, with FlareSolverr's settings from the environment.
    pub fn new(program: Option<PathBuf>, data: &Path, db: Option<SqliteDb>) -> Solver {
        Solver::with_config(program, data, db, Config::from_env())
    }

    /// [`Solver::new`] with the settings given.
    pub fn with_config(program: Option<PathBuf>, data: &Path, db: Option<SqliteDb>, config: Config) -> Solver {
        let s = Solver {
            inner: Arc::new(Inner {
                program,
                work: data.join("browser"),
                db,
                config,
                metrics: Arc::new(metrics::Metrics::default()),
                running: AsyncMutex::new(None),
                pages: Semaphore::new(PAGES_AT_ONCE),
                user_agent: Mutex::new(String::new()),
                clearances: Mutex::new(HashMap::new()),
                sessions: AsyncMutex::new(Vec::new()),
                generation: AtomicU64::new(0),
            }),
        };
        s.load_clearances();
        s
    }

    /// The settings in use.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// The Prometheus metrics.
    pub fn metrics(&self) -> Arc<metrics::Metrics> {
        self.inner.metrics.clone()
    }

    /// The browser program, when there is one.
    pub fn program(&self) -> Option<&Path> {
        self.inner.program.as_deref()
    }

    /// The user agent the browser sends, "" before it first ran.
    pub fn user_agent(&self) -> String {
        self.inner.user_agent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The user agent, starting the browser to learn it when it isn't known yet (FlareSolverr's
    /// `get_user_agent`).
    pub async fn learn_user_agent(&self) -> Result<String, SolveError> {
        if self.user_agent().is_empty() {
            self.browser().await?;
        }
        Ok(self.user_agent())
    }

    /// Whether the browser is open now.
    pub async fn is_running(&self) -> bool {
        self.inner.running.lock().await.is_some()
    }

    /// The browser, started when it isn't running, and which start of it this is.
    async fn browser(&self) -> Result<(Arc<Browser>, u64), SolveError> {
        let mut running = self.inner.running.lock().await;
        if let Some(r) = running.as_mut() {
            if !r.handler.is_finished() {
                r.last_used = Instant::now();
                return Ok((r.browser.clone(), r.generation));
            }
            *running = None;
        }
        let Some(program) = self.inner.program.clone() else { return Err(SolveError::NoBrowser) };
        if self.user_agent().is_empty() {
            // a first short start only to learn the browser's user agent, which the real start then sends
            let (mut b, h) = self.launch(&program, None).await?;
            let ua = b
                .user_agent()
                .await
                .map_err(|e| SolveError::Browser(format!("The browser for Cloudflare checks started but didn't answer: {e}")))?;
            let _ = b.close().await;
            let _ = b.wait().await;
            h.abort();
            *self.inner.user_agent.lock().unwrap_or_else(|e| e.into_inner()) = person_agent(&ua);
            tracing::info!("FlareSolverr User-Agent: {}", self.user_agent());
        }
        let ua = self.user_agent();
        let (b, handler) = self.launch(&program, Some(&ua)).await?;
        let browser = Arc::new(b);
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *running = Some(Running { browser: browser.clone(), handler, last_used: Instant::now(), generation });
        let me = self.clone();
        tokio::spawn(async move { me.close_when_idle().await });
        Ok((browser, generation))
    }

    /// The program's arguments besides chromiumoxide's own (its `--window-size=1920,1080` is FlareSolverr's):
    /// FlareSolverr's (`get_webdriver`), plus the ones a
    /// read-only container needs.
    pub fn browser_args(&self, ua: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = [
            "--disable-search-engine-choice-screen",
            "--disable-setuid-sandbox",
            "--disable-dev-shm-usage",
            "--no-zygote",
            "--ignore-certificate-errors",
            "--ignore-ssl-errors",
            "--disable-features=LocalNetworkAccessChecks",
            // Lantern: no automation flag, and no crash reporter writing outside the data folder
            "--disable-blink-features=AutomationControlled",
            "--disable-breakpad",
            "--disable-crash-reporter",
            "--no-first-run",
        ]
        .map(String::from)
        .to_vec();
        if cfg!(any(target_arch = "arm", target_arch = "aarch64")) {
            args.push("--disable-gpu-sandbox".into());
        }
        if let Some(lang) = &self.inner.config.lang {
            args.push(format!("--accept-lang={lang}"));
        }
        if let Some(ua) = ua {
            args.push(format!("--user-agent={ua}"));
        }
        args
    }

    /// Starts the browser, sending `ua` when given.
    async fn launch(&self, program: &Path, ua: Option<&str>) -> Result<(Browser, tokio::task::JoinHandle<()>), SolveError> {
        let fail = |e: String| SolveError::Browser(e);
        let profile = self.inner.work.join("profile");
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).map_err(|e| fail(format!("The browser's folder {} can't be made: {e}", profile.display())))?;
        let home = self.inner.work.display().to_string();
        // the image's root is read-only: the browser's scratch files go to the data folder too
        let tmp = self.inner.work.join("tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| fail(format!("The browser's folder {} can't be made: {e}", tmp.display())))?;
        let mut cfg = BrowserConfig::builder()
            .chrome_executable(program)
            .user_data_dir(&profile)
            .no_sandbox()
            .hide()
            .launch_timeout(LAUNCH_TIMEOUT)
            .request_timeout(Duration::from_secs(60))
            .window_size(1920, 1080)
            .env("HOME", home.clone())
            .env("XDG_CONFIG_HOME", home.clone())
            .env("XDG_CACHE_HOME", home)
            .env("TMPDIR", tmp.display().to_string());
        // chromiumoxide adds the dashes itself: a flag goes in as its name, or as (name, value)
        for a in self.browser_args(ua) {
            let a = a.trim_start_matches("--");
            cfg = match a.split_once('=') {
                Some((k, v)) => cfg.arg((k, v)),
                None => cfg.arg(a),
            };
        }
        if let Some(tz) = &self.inner.config.tz {
            cfg = cfg.env("TZ", tz.clone());
        }
        if !self.inner.config.headless {
            cfg = cfg.with_head();
        }
        let cfg = cfg.build().map_err(|e| fail(format!("The browser for Cloudflare checks can't be set up: {e}")))?;
        // a first start that times out is tried once more: the second start finds the browser's files in the
        // page cache and is much quicker
        let (browser, mut handler) = match Browser::launch(cfg.clone()).await {
            Ok(b) => b,
            Err(first) => {
                tracing::warn!("The browser for Cloudflare checks didn't start, trying once more: {first}");
                Browser::launch(cfg)
                    .await
                    .map_err(|e| fail(format!("The browser for Cloudflare checks ({}) didn't start: {e}", program.display())))?
            }
        };
        let task = tokio::spawn(async move { while handler.next().await.is_some() {} });
        Ok((browser, task))
    }

    /// Closes the browser once nobody used it for [`IDLE`] and no session is open.
    async fn close_when_idle(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let mut running = self.inner.running.lock().await;
            let Some(r) = running.as_ref() else { return };
            let busy = self.inner.pages.available_permits() < PAGES_AT_ONCE || !self.inner.sessions.lock().await.is_empty();
            if r.handler.is_finished() {
                *running = None;
                return;
            }
            if !busy && r.last_used.elapsed() >= IDLE {
                if let Some(r) = running.take() {
                    stop(r).await;
                }
                return;
            }
        }
    }

    /// Closes the browser now (at shutdown and in tests).
    pub async fn shutdown(&self) {
        self.inner.sessions.lock().await.clear();
        if let Some(r) = self.inner.running.lock().await.take() {
            stop(r).await;
        }
    }

    /// Opens a page for Lantern's own indexers: [`Solver::run`] with the request's fields, the error in plain
    /// words.
    pub async fn solve(&self, req: &SolveRequest) -> Result<Solution, String> {
        if host_of(&req.url).is_empty() || !(req.url.starts_with("http://") || req.url.starts_with("https://")) {
            return Err(format!("{} isn't a web address (it must start with http:// or https://)", req.url));
        }
        let job = Job {
            url: req.url.clone(),
            post: req.post_data.clone().map(|data| PostForm { data, flaresolverr: false }),
            cookies: req
                .cookies
                .iter()
                .map(|(k, v)| Ok(CookieSpec { name: k.clone(), value: Some(v.clone()), ..Default::default() }))
                .collect(),
            proxy: (!req.proxy.trim().is_empty()).then(|| Proxy { url: req.proxy.trim().to_string(), credentials: None }),
            session: (!req.session.is_empty()).then(|| req.session.clone()),
            timeout_ms: (req.timeout.as_millis() as u64).max(1),
            return_only_cookies: req.cookies_only,
            ..Default::default()
        };
        self.run(&job).await.map_err(|e| e.to_string())
    }

    /// One FlareSolverr request (`_resolve_challenge`): the browser (started outside the time allowed, as
    /// FlareSolverr starts its driver), a fresh context or the session's, then the page within `timeout_ms`. The
    /// cookies of a site that checked are kept for later requests.
    pub async fn run(&self, job: &Job) -> Result<Solution, SolveError> {
        let _turn = self.inner.pages.acquire().await.map_err(|_| SolveError::Browser("The solver is shutting down".into()))?;
        let (browser, generation) = self.browser().await?;
        let (context, own, proxy) = match &job.session {
            None => (new_context(&browser, job.proxy.as_ref()).await?, true, job.proxy.clone()),
            Some(id) => {
                let (c, p) = self.session_context(&browser, generation, id, job.session_ttl).await?;
                (c, false, p)
            }
        };
        let res = match open_page(&browser, &context).await {
            Ok(page) => {
                let out = tokio::time::timeout(Duration::from_millis(job.timeout_ms), self.evil_logic(&page, job, proxy.as_ref())).await;
                let _ = page.close().await;
                out
            }
            Err(e) => Ok(Err(e)),
        };
        if own {
            let _ = browser.dispose_browser_context(context).await;
        }
        let host = host_of(&job.url);
        let sol = match res {
            Ok(r) => r?,
            Err(_) => return Err(SolveError::Timeout { host, ms: job.timeout_ms }),
        };
        let proxy_key = proxy.map(|p| p.url).unwrap_or_default();
        self.keep(&host, &proxy_key, &sol);
        Ok(sol)
    }

    /// FlareSolverr's `_evil_logic` on `page`: load, set cookies, tell a refusal, wait out a check (ticking its
    /// box), then the answer.
    async fn evil_logic(&self, page: &Page, job: &Job, proxy: Option<&Proxy>) -> Result<Solution, SolveError> {
        let cfg = &self.inner.config;
        let host = host_of(&job.url);
        if let Some((user, pass)) = proxy.and_then(|p| p.credentials.clone()) {
            answer_auth(page, user, pass).await;
        }
        // the user agent is the browser's own (`--user-agent`): an override here would drop Accept-Language
        let ua = self.user_agent();
        if job.disable_media.unwrap_or(cfg.disable_media) {
            block_media(page).await;
        }
        let mut turnstile_token = None;
        match (&job.post, &job.tabs_till_verify) {
            (Some(form), _) => post_request(page, &job.url, form).await?,
            (None, None) => navigate(page, &job.url).await?,
            (None, Some(tabs)) => turnstile_token = resolve_turnstile(page, &job.url, *tabs).await?,
        }
        if !job.cookies.is_empty() {
            set_cookies(page, &job.cookies, &host).await?;
            match &job.post {
                Some(form) => post_request(page, &job.url, form).await?,
                None => navigate(page, &job.url).await?,
            }
        }
        if cfg.log_html {
            tracing::debug!("Response HTML:\n{}", page.content().await.unwrap_or_default());
        }
        let mut mark = 0u64;
        mark_html(page, mark).await;
        let title = eval_string(page, "document.title").await.unwrap_or_default();
        if ACCESS_DENIED_TITLES.iter().any(|t| title.starts_with(t)) {
            return Err(SolveError::Blocked { host });
        }
        for s in ACCESS_DENIED_SELECTORS {
            if exists(page, s).await {
                return Err(SolveError::Blocked { host });
            }
        }
        let mut challenged = CHALLENGE_TITLES.iter().any(|t| t.to_lowercase() == title.to_lowercase());
        if challenged {
            tracing::info!("Challenge detected. Title found: {title}");
        } else {
            for s in CHALLENGE_SELECTORS {
                if exists(page, s).await {
                    tracing::info!("Challenge detected. Selector found: {s}");
                    challenged = true;
                    break;
                }
            }
        }
        let wait = Duration::from_secs(cfg.browser_wait_timeout);
        if challenged {
            while !challenge_gone(page, wait).await {
                tracing::debug!("Timeout waiting for selector");
                click_verify(page, Some(1)).await;
                // the page as it is now (Cloudflare reloads it every few seconds)
                mark += 1;
                mark_html(page, mark).await;
            }
            // the redirect after the check
            let stale = format!(
                "(() => {{ try {{ return document.documentElement.__lanternMark !== {mark}; }} catch (e) {{ return false; }} }})()"
            );
            if !wait_js(page, wait, &stale).await {
                tracing::debug!("Timeout waiting for redirect");
            }
            tracing::info!("Challenge solved!");
        } else {
            tracing::info!("Challenge not detected!");
        }
        let url = eval_string(page, "location.href").await.unwrap_or_else(|| job.url.clone());
        let status: u16 = page
            .evaluate("(performance.getEntriesByType('navigation')[0] || {}).responseStatus || 200")
            .await
            .ok()
            .and_then(|v| v.into_value::<u16>().ok())
            .unwrap_or(200);
        let mut response = String::new();
        if !job.return_only_cookies {
            if job.wait_in_seconds > 0.0 {
                tracing::info!("Waiting {} seconds before returning the response...", job.wait_in_seconds);
                tokio::time::sleep(Duration::from_secs_f64(job.wait_in_seconds)).await;
            }
            response = page.content().await.unwrap_or_default();
        }
        let screenshot = if job.return_screenshot {
            let png = page
                .screenshot(ScreenshotParams::builder().format(CaptureScreenshotFormat::Png).build())
                .await
                .map_err(|e| SolveError::Failed(format!("Message: {e}")))?;
            use base64::Engine;
            Some(base64::engine::general_purpose::STANDARD.encode(png))
        } else {
            None
        };
        // the cookies after all the waits, to catch ones the page's scripts set
        let cookies = page
            .get_cookies()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|c| SolvedCookie {
                name: c.name,
                value: c.value,
                domain: c.domain,
                path: c.path,
                expires: c.expires,
                size: c.size,
                http_only: c.http_only,
                secure: c.secure,
                session: c.session,
                same_site: c.same_site.map(|s| s.as_ref().to_string()),
            })
            .collect();
        Ok(Solution { url, status, response, cookies, user_agent: ua, turnstile_token, screenshot, challenged })
    }

    /// The context of session `id`, made (without a proxy, as FlareSolverr does) when there is none, and made
    /// again when it is older than `ttl` or belongs to an earlier start of the browser.
    async fn session_context(
        &self,
        browser: &Browser,
        generation: u64,
        id: &str,
        ttl: Option<Duration>,
    ) -> Result<(BrowserContextId, Option<Proxy>), SolveError> {
        let mut sessions = self.inner.sessions.lock().await;
        let i = match sessions.iter().position(|(k, _)| k == id) {
            Some(i) => {
                if ttl.is_some_and(|ttl| sessions[i].1.created.elapsed() > ttl) {
                    tracing::debug!("session's lifetime has expired, so the session is recreated (session_id={id})");
                    let (_, old) = sessions.remove(i);
                    let _ = browser.dispose_browser_context(old.context).await;
                    let context = new_context(browser, None).await?;
                    sessions.push((id.to_string(), Session { context, proxy: None, created: Instant::now(), generation }));
                    sessions.len() - 1
                } else {
                    i
                }
            }
            None => {
                tracing::debug!("new session created to perform the request (session_id={id})");
                let context = new_context(browser, None).await?;
                sessions.push((id.to_string(), Session { context, proxy: None, created: Instant::now(), generation }));
                sessions.len() - 1
            }
        };
        let s = &mut sessions[i].1;
        if s.generation != generation {
            s.context = new_context(browser, s.proxy.as_ref()).await?;
            s.generation = generation;
        }
        Ok((s.context.clone(), s.proxy.clone()))
    }

    /// Keeps the cookies of a solved `host` (behind `proxy`) for later requests.
    fn keep(&self, host: &str, proxy: &str, sol: &Solution) {
        if !sol.challenged && !sol.cookies.iter().any(|c| c.name == "cf_clearance" || c.name.starts_with("__ddg")) {
            return;
        }
        let now = crate::auth::now();
        let mine: Vec<&SolvedCookie> = sol.cookies.iter().filter(|c| domain_matches(&c.domain, host)).collect();
        let until = mine
            .iter()
            .find(|c| c.name == "cf_clearance")
            .map(|c| c.expires)
            .filter(|e| *e > now)
            .unwrap_or(now + CLEARANCE_DEFAULT.as_secs_f64());
        let c = Clearance {
            cookies: mine.iter().map(|c| (c.name.clone(), c.value.clone())).collect(),
            user_agent: sol.user_agent.clone(),
            until,
        };
        if let Some(db) = &self.inner.db {
            let cookies = serde_json::to_string(&c.cookies).unwrap_or_else(|_| "[]".into());
            let _ = db.execute(
                "INSERT OR REPLACE INTO solver_clearance (host, proxy, cookies, user_agent, until) VALUES (?,?,?,?,?)",
                p![host, proxy, cookies, c.user_agent, c.until],
            );
        }
        self.inner.clearances.lock().unwrap_or_else(|e| e.into_inner()).insert((host.to_string(), proxy.to_string()), c);
    }

    /// The cookies kept for `host` behind `proxy`, while they last.
    pub fn clearance(&self, host: &str, proxy: &str) -> Option<Clearance> {
        let now = crate::auth::now();
        let map = self.inner.clearances.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&(host.to_string(), proxy.to_string())).filter(|c| c.until > now).cloned()
    }

    /// Forgets the cookies of `host` behind `proxy` (the site checked again anyway).
    pub fn forget(&self, host: &str, proxy: &str) {
        self.inner.clearances.lock().unwrap_or_else(|e| e.into_inner()).remove(&(host.to_string(), proxy.to_string()));
        if let Some(db) = &self.inner.db {
            let _ = db.execute("DELETE FROM solver_clearance WHERE host=? AND proxy=?", p![host, proxy]);
        }
    }

    /// The sites with kept cookies now: (host, until).
    pub fn cleared_sites(&self) -> Vec<(String, f64)> {
        let now = crate::auth::now();
        let mut out: Vec<(String, f64)> = self
            .inner
            .clearances
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, c)| c.until > now)
            .map(|((h, _), c)| (h.clone(), c.until))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn load_clearances(&self) {
        let Some(db) = &self.inner.db else { return };
        let now = crate::auth::now();
        let _ = db.execute("DELETE FROM solver_clearance WHERE until <= ?", p![now]);
        let Ok(rows) = db.query("SELECT * FROM solver_clearance", p![]) else { return };
        let mut map = self.inner.clearances.lock().unwrap_or_else(|e| e.into_inner());
        for r in rows {
            let cookies: Vec<(String, String)> = serde_json::from_str(&r.s("cookies")).unwrap_or_default();
            map.insert((r.s("host"), r.s("proxy")), Clearance { cookies, user_agent: r.s("user_agent"), until: r.f("until") });
        }
    }

    /// FlareSolverr's `sessions.create`: session `id` (a new id when `None`) with `proxy`, and whether it is new
    /// (`false`: one by that id already exists, and is left as it is).
    pub async fn create_session(&self, id: Option<String>, proxy: Option<Proxy>) -> Result<(String, bool), SolveError> {
        let id = id.unwrap_or_else(uuid1);
        if self.inner.sessions.lock().await.iter().any(|(k, _)| *k == id) {
            return Ok((id, false));
        }
        let (browser, generation) = self.browser().await?;
        let context = new_context(&browser, proxy.as_ref()).await?;
        let mut sessions = self.inner.sessions.lock().await;
        if sessions.iter().any(|(k, _)| *k == id) {
            let _ = browser.dispose_browser_context(context).await;
            return Ok((id, false));
        }
        sessions.push((id.clone(), Session { context, proxy, created: Instant::now(), generation }));
        Ok((id, true))
    }

    /// The sessions' ids, oldest first.
    pub async fn session_ids(&self) -> Vec<String> {
        self.inner.sessions.lock().await.iter().map(|(k, _)| k.clone()).collect()
    }

    /// FlareSolverr's `sessions.destroy`: `false` when there was no session by that id.
    pub async fn destroy_session(&self, id: &str) -> bool {
        let removed = {
            let mut sessions = self.inner.sessions.lock().await;
            sessions.iter().position(|(k, _)| k == id).map(|i| sessions.remove(i).1)
        };
        let Some(s) = removed else { return false };
        let browser = self.inner.running.lock().await.as_ref().map(|r| r.browser.clone());
        if let Some(b) = browser {
            let _ = b.dispose_browser_context(s.context).await;
        }
        true
    }
}

/// A fresh browser context (its own cookies), through `proxy` when given.
async fn new_context(browser: &Browser, proxy: Option<&Proxy>) -> Result<BrowserContextId, SolveError> {
    let mut params = CreateBrowserContextParams::builder().dispose_on_detach(true);
    if let Some(p) = proxy.filter(|p| !p.url.trim().is_empty()) {
        tracing::debug!("Using webdriver proxy: {}", p.url);
        params = params.proxy_server(p.url.trim().to_string());
    }
    browser
        .create_browser_context(params.build())
        .await
        .map_err(|e| SolveError::Browser(format!("The browser can't open a fresh window: {e}")))
}

/// A blank page in `context`.
async fn open_page(browser: &Browser, context: &BrowserContextId) -> Result<Page, SolveError> {
    let target = CreateTargetParams::builder()
        .url("about:blank")
        .browser_context_id(context.clone())
        .build()
        .map_err(|e| SolveError::Browser(format!("The browser can't open a page: {e}")))?;
    browser.new_page(target).await.map_err(|e| SolveError::Browser(format!("The browser can't open a page: {e}")))
}

/// The browser's error `s` as chromedriver would word it.
fn driver_error(host: &str, s: &str) -> SolveError {
    let message = if let Some(i) = s.find("net::ERR_") {
        let code: String = s[i..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':').collect();
        format!("unknown error: {code}")
    } else if s.contains("invalid URL") || s.contains("Cannot navigate") {
        "invalid argument".to_string()
    } else {
        format!("unknown error: {s}")
    };
    SolveError::Driver { host: host.to_string(), message }
}

/// Opens `url` and waits for it to load, as Selenium's `driver.get` does. An error status is still a page.
async fn navigate(page: &Page, url: &str) -> Result<(), SolveError> {
    tracing::debug!("Navigating to... {url}");
    let host = host_of(url);
    let params = NavigateParams::builder().url(url).build().map_err(|e| driver_error(&host, &e))?;
    match page.execute(params).await {
        Ok(r) => {
            if let Some(e) = r.result.error_text.as_deref().filter(|e| !e.is_empty()) {
                if !e.contains("ERR_HTTP_RESPONSE_CODE_FAILURE") {
                    return Err(driver_error(&host, e));
                }
            }
        }
        Err(e) => return Err(driver_error(&host, &e.to_string())),
    }
    let target = js(url);
    let loaded = format!("(() => document.readyState === 'complete' && (location.href !== 'about:blank' || {target} === 'about:blank'))()");
    while !eval_bool(page, &loaded).await {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// FlareSolverr's `_post_request` (or Lantern's own form): opens the form page and waits for the page it sends
/// to load.
async fn post_request(page: &Page, url: &str, form: &PostForm) -> Result<(), SolveError> {
    let data_url = if form.flaresolverr { flaresolverr_post_url(url, &form.data) } else { post_page_url(url, &form.data) };
    navigate(page, &data_url).await?;
    // the form sends itself as the page opens; give it a moment to leave (an action it can't send stays put)
    let left = "(() => !location.href.startsWith('data:'))()";
    if wait_js(page, Duration::from_secs(5), left).await {
        while !eval_bool(page, "(() => document.readyState === 'complete')()").await {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

/// Sets the request's cookies on the page now open (each one deleted first, as FlareSolverr does).
async fn set_cookies(page: &Page, cookies: &[CookieIn], host: &str) -> Result<(), SolveError> {
    tracing::debug!("Setting cookies...");
    let here = eval_string(page, "location.href").await.unwrap_or_default();
    for c in cookies {
        let c = c.clone().map_err(SolveError::Failed)?;
        let mut del = DeleteCookiesParams::new(c.name.clone());
        del.url = Some(here.clone());
        let _ = page.execute(del).await;
        if let Some(s) = &c.same_site {
            if !["Strict", "Lax", "None"].contains(&s.as_str()) {
                return Err(SolveError::Failed(String::new()));
            }
        }
        let Some(value) = c.value.clone() else {
            return Err(SolveError::Driver { host: host.into(), message: "invalid argument: missing 'value'".into() });
        };
        let mut b = CookieParam::builder().name(c.name.clone()).value(value);
        b = match &c.domain {
            Some(d) => b.domain(d.clone()),
            None => b.url(here.clone()),
        };
        if let Some(p) = &c.path {
            b = b.path(p.clone());
        }
        if let Some(s) = c.secure {
            b = b.secure(s);
        }
        if let Some(h) = c.http_only {
            b = b.http_only(h);
        }
        if let Some(e) = c.expiry {
            b = b.expires(TimeSinceEpoch::new(e));
        }
        if let Some(s) = &c.same_site {
            b = b.same_site(match s.as_str() {
                "Strict" => CookieSameSite::Strict,
                "Lax" => CookieSameSite::Lax,
                _ => CookieSameSite::None,
            });
        }
        let param = b.build().map_err(|e| SolveError::Driver { host: host.into(), message: format!("invalid argument: {e}") })?;
        let set = network::SetCookieParams {
            name: param.name,
            value: param.value,
            url: param.url,
            domain: param.domain,
            path: param.path,
            secure: param.secure,
            http_only: param.http_only,
            same_site: param.same_site,
            expires: param.expires,
            priority: None,
            same_party: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        };
        if let Err(e) = page.execute(set).await {
            return Err(SolveError::Driver { host: host.into(), message: format!("unable to set cookie: {e}") });
        }
    }
    Ok(())
}

/// Blocks images, styles and fonts on the page (`disableMedia`); a browser that can't is left as it is.
async fn block_media(page: &Page) {
    let urls: Vec<String> = MEDIA_PATTERNS.iter().map(|s| s.to_string()).collect();
    tracing::debug!("Network.setBlockedURLs: {urls:?}");
    let _ = page.execute(network::EnableParams::default()).await;
    if page.execute(BlockUrls { urls: urls.clone() }).await.is_err() {
        // newer browsers take URL patterns instead
        let patterns: Vec<network::BlockPattern> = urls.iter().map(|u| network::BlockPattern::new(format!("*://*:*/{u}"), true)).collect();
        if page.execute(network::SetBlockedUrLsParams::builder().url_patterns(patterns).build()).await.is_err() {
            tracing::debug!("Network.setBlockedURLs failed or unsupported on this webdriver");
        }
    }
}

/// Answers every sign-in the page meets (the proxy's) with `user` and `pass`, as FlareSolverr's proxy extension
/// does.
async fn answer_auth(page: &Page, user: String, pass: String) {
    let _ = page.authenticate(chromiumoxide::auth::Credentials { username: user, password: pass }).await;
}

/// `js` evaluated on the page as text, `None` when it fails.
async fn eval_string(page: &Page, js: &str) -> Option<String> {
    page.evaluate(js).await.ok().and_then(|v| v.into_value::<String>().ok())
}

/// `js` evaluated on the page as true or false (false when it fails).
async fn eval_bool(page: &Page, js: &str) -> bool {
    page.evaluate(js).await.ok().and_then(|v| v.into_value::<bool>().ok()).unwrap_or(false)
}

/// Whether the page has an element matching `selector`.
async fn exists(page: &Page, selector: &str) -> bool {
    eval_bool(page, &format!("(() => {{ try {{ return !!document.querySelector({}); }} catch (e) {{ return false; }} }})()", js(selector)))
        .await
}

/// Marks the page's `<html>` with `n`, to tell later whether it was replaced (Selenium's element staleness).
async fn mark_html(page: &Page, n: u64) {
    let _ = page.evaluate(format!("(() => {{ document.documentElement.__lanternMark = {n}; return true; }})()")).await;
}

/// Selenium's `WebDriverWait(driver, timeout).until(...)`: checks `js` (true when done) every half second until it
/// holds (true) or `timeout` passed (false).
async fn wait_js(page: &Page, timeout: Duration, js: &str) -> bool {
    let start = Instant::now();
    loop {
        if eval_bool(page, js).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if start.elapsed() > timeout {
            return false;
        }
    }
}

/// One attempt of FlareSolverr's wait for a check to go: every check title gone, then every check element, each
/// within `wait`. False when one of them is still there.
async fn challenge_gone(page: &Page, wait: Duration) -> bool {
    for t in CHALLENGE_TITLES {
        if !wait_js(page, wait, &format!("(() => document.title !== {})()", js(t))).await {
            return false;
        }
    }
    for s in CHALLENGE_SELECTORS {
        let gone = format!("(() => {{ try {{ return !document.querySelector({}); }} catch (e) {{ return false; }} }})()", js(s));
        if !wait_js(page, wait, &gone).await {
            return false;
        }
    }
    true
}

/// Presses `key` (`Tab` or a space) on the page.
async fn press(page: &Page, key: &str) {
    let (code, text, vk) = if key == "Tab" { ("Tab", "", 9) } else { ("Space", " ", 32) };
    for kind in [DispatchKeyEventType::KeyDown, DispatchKeyEventType::KeyUp] {
        let mut b = DispatchKeyEventParams::builder().r#type(kind.clone()).key(key).code(code).windows_virtual_key_code(vk);
        if kind == DispatchKeyEventType::KeyDown && !text.is_empty() {
            b = b.text(text);
        }
        if let Ok(ev) = b.build() {
            let _ = page.execute(ev).await;
        }
    }
}

/// FlareSolverr's `click_verify`: after 5 s, `tabs` Tab presses and a Space tick the Turnstile box (`None`: no
/// keys, as when FlareSolverr's tab count isn't a whole number), then a "Verify you are human" button is clicked
/// when there is one, and 2 s pass.
async fn click_verify(page: &Page, tabs: Option<i64>) {
    tracing::debug!("Try to find the Cloudflare verify checkbox...");
    if let Some(n) = tabs {
        tokio::time::sleep(Duration::from_secs(5)).await;
        for _ in 0..n.max(0) {
            press(page, "Tab").await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        press(page, " ").await;
        tracing::debug!("Cloudflare verify checkbox clicked after {n} tabs!");
    }
    let find = r#"(() => {
      const b = document.evaluate("//input[@type='button' and @value='Verify you are human']", document, null,
        XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
      if (!b) return null;
      const r = b.getBoundingClientRect();
      return [r.left + r.width / 2, r.top + r.height / 2];
    })()"#;
    let at: Option<Vec<f64>> = page.evaluate(find).await.ok().and_then(|v| v.into_value::<Option<Vec<f64>>>().ok()).flatten();
    match at.as_deref() {
        Some([x, y]) => {
            let _ = page.click(Point { x: *x, y: *y }).await;
            tracing::debug!("The Cloudflare 'Verify you are human' button found and clicked!");
        }
        _ => tracing::debug!("The Cloudflare 'Verify you are human' button not found on the page."),
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

/// FlareSolverr's `_resolve_turnstile_captcha`: opens `url` and, when it has a Turnstile field, ticks the box
/// until the field holds a new token.
async fn resolve_turnstile(page: &Page, url: &str, tabs: Option<i64>) -> Result<Option<String>, SolveError> {
    tracing::debug!("Navigating to... {url} in order to pass the turnstile challenge");
    navigate(page, url).await?;
    let selector = TURNSTILE_SELECTORS[0];
    if !exists(page, selector).await {
        tracing::debug!("Turnstile challenge not found");
        return Ok(None);
    }
    tracing::info!("Turnstile challenge detected. Selector found: {selector}");
    let read = format!("(() => {{ const e = document.querySelector({}); return e ? e.value : null; }})()", js(selector));
    let value = |page: &Page| {
        let read = read.clone();
        let page = page.clone();
        async move { page.evaluate(read).await.ok().and_then(|v| v.into_value::<Option<String>>().ok()).flatten() }
    };
    let current = value(page).await.unwrap_or_default();
    loop {
        click_verify(page, tabs).await;
        let Some(token) = value(page).await else {
            return Err(SolveError::Driver {
                host: host_of(url),
                message: "stale element reference: stale element not found in the current frame".into(),
            });
        };
        if !token.is_empty() && token != current {
            tracing::info!("Turnstile token: {token}");
            return Ok(Some(token));
        }
        tracing::debug!("Failed to extract token possibly click failed");
        // move the focus back to the top of the page
        let _ = page
            .evaluate(
                "(() => { let old = document.getElementById('__focus_helper'); if (old) old.remove();
                  let el = document.createElement('button'); el.id = '__focus_helper'; el.style.position = 'fixed';
                  el.style.top = '0'; el.style.left = '0'; el.style.opacity = '0.01'; el.style.pointerEvents = 'none';
                  document.body.prepend(el); el.focus(); return true; })()",
            )
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn stop(r: Running) {
    match Arc::try_unwrap(r.browser) {
        Ok(mut b) => {
            let _ = b.close().await;
            let _ = b.wait().await;
        }
        // a request still holds it: dropping the last handle kills the program
        Err(b) => drop(b),
    }
    r.handler.abort();
}

/// A new session id in the shape of Python's `uuid1()` (time-based, version 1, random node).
pub fn uuid1() -> String {
    use rand::Rng;
    // 100 ns intervals since 1582-10-15
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or(0) as u64
        + 0x01b2_1dd2_1381_4000;
    let mut rng = rand::rng();
    let clock: u16 = rng.random::<u16>() & 0x3fff | 0x8000;
    let node: u64 = rng.random::<u64>() & 0xffff_ffff_ffff | 0x0100_0000_0000;
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", t & 0xffff_ffff, (t >> 32) & 0xffff, ((t >> 48) & 0x0fff) | 0x1000, clock, node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_scripts_arent_taken_for_the_browser() {
        let dir = std::env::temp_dir().join(format!("lantern-programs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("chromium");
        std::fs::write(&script, "#!/bin/sh\nexec /snap/bin/chromium \"$@\"\n").unwrap();
        let program = dir.join("chrome");
        std::fs::write(&program, b"\x7fELF").unwrap();
        assert!(!runnable(&script) && runnable(&program) && !runnable(&dir.join("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_hosts_and_domains() {
        assert_eq!(person_agent("Mozilla/5.0 HeadlessChrome/130.0.0.0 Safari/537.36"), "Mozilla/5.0 Chrome/130.0.0.0 Safari/537.36");
        assert_eq!(person_agent("a HEADLESS b headless"), "a  b ");
        assert_eq!(chrome_version("Mozilla/5.0 (X11) Chrome/142.0.7444.59 Safari/537.36"), "142.0.7444.59");
        assert_eq!(host_of("https://Tracker.Example.org/x?y=1"), "tracker.example.org");
        assert!(domain_matches(".example.org", "tracker.example.org"));
        assert!(domain_matches("example.org", "example.org"));
        assert!(!domain_matches("example.org", "badexample.org"));
    }

    #[test]
    fn the_flaresolverr_post_form_is_built_like_post_request() {
        let u = flaresolverr_post_url("https://httpbin.org/post", "param1=value1&param2=value2");
        assert!(
            u.starts_with("data:text/html;charset=utf-8,\n        <!DOCTYPE html>\n        <html>\n        <body>\n            "),
            "{u}"
        );
        assert!(u.contains(r#"<form id="hackForm" action="https://httpbin.org/post" method="POST"><input type="text" name="param1" value="value1"><br><input type="text" name="param2" value="value2"><br></form>"#), "{u}");
        assert!(u.ends_with("<script>document.getElementById('hackForm').submit();</script>\n        </body>\n        </html>"));
        // a leading ?, a submit field, encoded and quoted values, a pair without =
        let u = flaresolverr_post_url("https://x.test/", "?a%20b=c%2Bd+e&submit=Go&q=\"x\"&flag");
        assert!(u.contains(r#"name="a%20b" value="c%2Bd%2Be""#), "{u}");
        assert!(!u.contains("submit\""), "{u}");
        assert!(u.contains(r#"name="q" value="%26quot%3Bx%26quot%3B""#), "{u}");
        assert!(u.contains(r#"name="flag" value="""#), "{u}");
        assert_eq!(py_unquote("%E2%82%AC+%zz"), "€+%zz");
        assert_eq!(py_quote("a b/c~é"), "a%20b/c~%C3%A9");
        assert_eq!(py_escape("<a href='x'>&\"</a>"), "&lt;a href=&#x27;x&#x27;&gt;&amp;&quot;&lt;/a&gt;");
    }

    #[test]
    fn lanterns_post_page_sends_itself() {
        let p = post_page("https://x.test/login?a=\"1\"", "user=me&pass=a%26b");
        assert!(p.contains(r#"action="https://x.test/login?a=&quot;1&quot;""#), "{p}");
        assert!(p.contains(r#"name="pass" value="a&amp;b""#), "{p}");
        assert!(p.contains("submit()"));
        assert!(post_page_url("https://x.test/", "a=100%25").starts_with("data:text/html;charset=utf-8,%3C"));
    }

    #[test]
    fn errors_read_like_flaresolverrs() {
        let t = SolveError::Timeout { host: "x".into(), ms: 10 };
        assert_eq!(t.flaresolverr("1"), "Error solving the challenge. Timeout after 0.01 seconds.");
        assert_eq!(
            SolveError::Timeout { host: "x".into(), ms: 60_000 }.flaresolverr("1"),
            "Error solving the challenge. Timeout after 60.0 seconds."
        );
        assert_eq!(
            SolveError::Blocked { host: "x".into() }.flaresolverr("1"),
            "Error solving the challenge. Cloudflare has blocked this request. Probably your IP is banned for this site, check in your web browser."
        );
        let d = driver_error("x", "net::ERR_NAME_NOT_RESOLVED");
        assert_eq!(
            d.flaresolverr("142.0.1"),
            "Error solving the challenge. Message: unknown error: net::ERR_NAME_NOT_RESOLVED\\n  (Session info: chrome=142.0.1)\\n"
        );
        assert!(d.to_string().contains("can't be found"), "{d}");
        assert_eq!(SolveError::Failed("'name'".into()).flaresolverr(""), "Error solving the challenge. 'name'");
        assert!(SolveError::Blocked { host: "x".into() }.to_string().contains("refused"));
    }

    #[test]
    fn session_ids_look_like_uuid1() {
        let id = uuid1();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12], "{id}");
        assert!(parts[2].starts_with('1'), "{id}");
        assert_ne!(uuid1(), uuid1());
    }

    #[test]
    fn media_list_is_flaresolverrs() {
        assert!(MEDIA_PATTERNS.contains(&"*.png") && MEDIA_PATTERNS.contains(&"*.css") && MEDIA_PATTERNS.contains(&"*.EOT"));
    }

    #[test]
    fn a_missing_browser_says_what_to_do() {
        let dir = std::env::temp_dir().join(format!("lantern-solver-{}", std::process::id()));
        let s = Solver::with_config(None, &dir, None, Config::default());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let e = rt
            .block_on(s.solve(&SolveRequest { url: "https://x.test/".into(), timeout: Duration::from_secs(5), ..Default::default() }))
            .unwrap_err();
        assert!(e.contains("APP_CHROMIUM"), "{e}");
        let e = rt.block_on(s.solve(&SolveRequest { url: "ftp://x".into(), ..Default::default() })).unwrap_err();
        assert!(e.contains("isn't a web address"), "{e}");
    }
}
