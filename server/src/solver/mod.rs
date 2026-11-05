//! Cloudflare solving (goal 2): a headless Chromium that opens a page the way a person's browser does, waits
//! while Cloudflare (or DDoS-Guard) checks it, and hands back the page, its cookies and the browser's user agent.
//!
//! | Part | What |
//! | --- | --- |
//! | [`Solver`] | one browser for all requests, started when first needed and closed after five idle minutes |
//! | [`Solver::solve`] | one request: a fresh browser context (or a session's), the page, the check, the answer |
//! | [`Solver::clearance`] | the cookies a site gave after its check, kept per site (and proxy) in `solver_clearance` |
//! | [`api`] | the FlareSolverr API (`POST /v1`), for apps that call a solver |
//!
//! Indexers use the solver by themselves: when a site answers with a check, the indexer's HTTP client asks the
//! solver once, keeps the cookies and the user agent, and sends both with its later requests to that site.

pub mod api;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::cdp::browser_protocol::target::{CreateBrowserContextParams, CreateTargetParams};
use chromiumoxide::Page;
use futures_util::StreamExt;
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

/// How long the browser stays open with nothing to do.
pub const IDLE: Duration = Duration::from_secs(300);

/// How many pages the browser opens at once; more requests wait their turn.
pub const PAGES_AT_ONCE: usize = 3;

/// How long a site's cookies are kept when the site doesn't say (cf_clearance usually lasts longer).
pub const CLEARANCE_DEFAULT: Duration = Duration::from_secs(3600);

/// The page titles of a check that is still running.
const CHALLENGE_TITLES: [&str; 2] = ["Just a moment...", "DDoS-Guard"];

/// The page titles of a request Cloudflare refused outright.
const BLOCKED_TITLES: [&str; 2] = ["Access denied", "Attention Required! | Cloudflare"];

/// The parts of a page that mean a check is still running.
const CHALLENGE_SELECTORS: [&str; 10] = [
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

/// One cookie of a solved page, as FlareSolverr answers it.
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

/// A request to the solver.
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

/// What a solved request gives.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    /// The address the page ended on.
    pub url: String,
    /// The page's HTTP status.
    pub status: u16,
    /// The page's HTML ("" when only cookies were asked for).
    pub response: String,
    /// The cookies the browser has for the page.
    pub cookies: Vec<SolvedCookie>,
    /// The user agent the browser sent; requests that reuse the cookies must send the same one.
    pub user_agent: String,
    /// Whether there was a check to get past.
    #[serde(skip)]
    pub challenged: bool,
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
    browser: Arc<Browser>,
    handler: tokio::task::JoinHandle<()>,
    last_used: Instant,
}

/// A FlareSolverr session: a browser context whose cookies last between requests.
#[derive(Debug, Clone)]
struct Session {
    context: BrowserContextId,
    proxy: String,
}

/// The solver. Clones share the browser.
#[derive(Clone)]
pub struct Solver {
    inner: Arc<Inner>,
}

struct Inner {
    /// The browser program, when one was found.
    program: Option<PathBuf>,
    /// The folder for the browser's profile and its home (in the data folder).
    work: PathBuf,
    db: Option<SqliteDb>,
    running: AsyncMutex<Option<Running>>,
    pages: Semaphore,
    /// The user agent the browser sends ("HeadlessChrome" said as "Chrome"), once known.
    user_agent: Mutex<String>,
    clearances: Mutex<HashMap<(String, String), Clearance>>,
    sessions: AsyncMutex<HashMap<String, Session>>,
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

/// The user agent a person's Chrome of the same version sends: the browser's own with "HeadlessChrome" said as
/// "Chrome" (sites block the headless name).
pub fn person_agent(ua: &str) -> String {
    ua.replace("HeadlessChrome", "Chrome")
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

/// What the page says: `ok`, `challenge` (a check still running), `blocked`, or `loading`.
const STATE_JS: &str = r#"(() => {
  const t = document.title;
  if (BLOCKED.includes(t)) return 'blocked';
  if (TITLES.includes(t)) return 'challenge';
  for (const q of SELECTORS) { try { if (document.querySelector(q)) return 'challenge'; } catch (e) {} }
  return document.readyState === 'complete' ? 'ok' : 'loading';
})()"#;

/// [`STATE_JS`] with its lists filled in.
fn state_js() -> String {
    let list = |l: &[&str]| serde_json::to_string(l).unwrap_or_else(|_| "[]".into());
    STATE_JS
        .replace("BLOCKED", &list(&BLOCKED_TITLES))
        .replace("TITLES", &list(&CHALLENGE_TITLES))
        .replace("SELECTORS", &list(&CHALLENGE_SELECTORS))
}

/// A page that POSTs `post_data` (`a=1&b=2`) to `url` as a form as soon as it opens, the way FlareSolverr does.
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

impl Solver {
    /// A solver that runs `program` (none: every request answers that there is no browser), with its profile in
    /// `<data>/browser`, keeping sites' cookies in `db`.
    pub fn new(program: Option<PathBuf>, data: &Path, db: Option<SqliteDb>) -> Solver {
        let s = Solver {
            inner: Arc::new(Inner {
                program,
                work: data.join("browser"),
                db,
                running: AsyncMutex::new(None),
                pages: Semaphore::new(PAGES_AT_ONCE),
                user_agent: Mutex::new(String::new()),
                clearances: Mutex::new(HashMap::new()),
                sessions: AsyncMutex::new(HashMap::new()),
            }),
        };
        s.load_clearances();
        s
    }

    /// The browser program, when there is one.
    pub fn program(&self) -> Option<&Path> {
        self.inner.program.as_deref()
    }

    /// The user agent the browser sends, "" before it first ran.
    pub fn user_agent(&self) -> String {
        self.inner.user_agent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Whether the browser is open now.
    pub async fn is_running(&self) -> bool {
        self.inner.running.lock().await.is_some()
    }

    /// The browser, started when it isn't running.
    async fn browser(&self) -> Result<Arc<Browser>, String> {
        let mut running = self.inner.running.lock().await;
        if let Some(r) = running.as_mut() {
            if !r.handler.is_finished() {
                r.last_used = Instant::now();
                return Ok(r.browser.clone());
            }
            *running = None;
        }
        let Some(program) = self.inner.program.clone() else {
            return Err("There is no browser for Cloudflare checks: set APP_CHROMIUM to a Chromium program, or use the Lantern image, which has one".into());
        };
        if self.user_agent().is_empty() {
            // a first short start only to learn the browser's user agent, which the real start then sends
            let (mut b, h) = self.launch(&program, None).await?;
            let ua = b.user_agent().await.map_err(|e| format!("The browser for Cloudflare checks started but didn't answer: {e}"))?;
            let _ = b.close().await;
            let _ = b.wait().await;
            h.abort();
            *self.inner.user_agent.lock().unwrap_or_else(|e| e.into_inner()) = person_agent(&ua);
        }
        let ua = self.user_agent();
        let (b, handler) = self.launch(&program, Some(&ua)).await?;
        let browser = Arc::new(b);
        *running = Some(Running { browser: browser.clone(), handler, last_used: Instant::now() });
        let me = self.clone();
        tokio::spawn(async move { me.close_when_idle().await });
        Ok(browser)
    }

    /// Starts the browser, sending `ua` when given.
    async fn launch(&self, program: &Path, ua: Option<&str>) -> Result<(Browser, tokio::task::JoinHandle<()>), String> {
        let profile = self.inner.work.join("profile");
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).map_err(|e| format!("The browser's folder {} can't be made: {e}", profile.display()))?;
        let home = self.inner.work.display().to_string();
        // the image's root is read-only: the browser's scratch files go to the data folder too
        let tmp = self.inner.work.join("tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| format!("The browser's folder {} can't be made: {e}", tmp.display()))?;
        let mut cfg = BrowserConfig::builder()
            .chrome_executable(program)
            .user_data_dir(&profile)
            .no_sandbox()
            .hide()
            .launch_timeout(Duration::from_secs(30))
            .request_timeout(Duration::from_secs(60))
            .window_size(1920, 1080)
            .env("HOME", home.clone())
            .env("XDG_CONFIG_HOME", home.clone())
            .env("XDG_CACHE_HOME", home)
            .env("TMPDIR", tmp.display().to_string())
            .args([
                "--disable-dev-shm-usage",
                "--disable-gpu",
                "--no-first-run",
                "--lang=en-US",
                "--disable-blink-features=AutomationControlled",
                "--disable-breakpad",
                "--disable-crash-reporter",
            ]);
        if let Some(ua) = ua {
            cfg = cfg.arg(format!("--user-agent={ua}"));
        }
        let cfg = cfg.build().map_err(|e| format!("The browser for Cloudflare checks can't be set up: {e}"))?;
        let (browser, mut handler) = Browser::launch(cfg)
            .await
            .map_err(|e| format!("The browser for Cloudflare checks ({}) didn't start: {e}", program.display()))?;
        let task = tokio::spawn(async move { while handler.next().await.is_some() {} });
        Ok((browser, task))
    }

    /// Closes the browser once nobody used it for [`IDLE`].
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

    /// Opens `req.url` in the browser, waits while a check runs and answers the page, its cookies and the user
    /// agent. The cookies are kept for the site (see [`Solver::clearance`]).
    pub async fn solve(&self, req: &SolveRequest) -> Result<Solution, String> {
        let host = host_of(&req.url);
        if host.is_empty() || !(req.url.starts_with("http://") || req.url.starts_with("https://")) {
            return Err(format!("{} isn't a web address (it must start with http:// or https://)", req.url));
        }
        let _turn = self.inner.pages.acquire().await.map_err(|_| "The solver is shutting down".to_string())?;
        let browser = self.browser().await?;
        let (context, own) = if req.session.is_empty() {
            (new_context(&browser, &req.proxy).await?, true)
        } else {
            let mut sessions = self.inner.sessions.lock().await;
            match sessions.get(&req.session) {
                Some(s) => (s.context.clone(), false),
                None => {
                    let c = new_context(&browser, &req.proxy).await?;
                    sessions.insert(req.session.clone(), Session { context: c.clone(), proxy: req.proxy.clone() });
                    (c, false)
                }
            }
        };
        let proxy = if req.session.is_empty() {
            req.proxy.clone()
        } else {
            self.inner.sessions.lock().await.get(&req.session).map(|s| s.proxy.clone()).unwrap_or_default()
        };
        let res =
            tokio::time::timeout(req.timeout.max(Duration::from_secs(5)) + Duration::from_secs(10), self.open(&browser, &context, req))
                .await;
        if own {
            let _ = browser.dispose_browser_context(context).await;
        }
        let sol = match res {
            Ok(r) => r?,
            Err(_) => return Err(format!("{host} didn't finish loading in time; try again, or give it longer")),
        };
        self.keep(&host, &proxy, &sol);
        Ok(sol)
    }

    async fn open(&self, browser: &Browser, context: &BrowserContextId, req: &SolveRequest) -> Result<Solution, String> {
        let host = host_of(&req.url);
        let target = CreateTargetParams::builder()
            .url("about:blank")
            .browser_context_id(context.clone())
            .build()
            .map_err(|e| format!("The browser can't open a page: {e}"))?;
        let page = browser.new_page(target).await.map_err(|e| format!("The browser can't open a page: {e}"))?;
        let out = self.open_on(&page, req, &host).await;
        let _ = page.close().await;
        out
    }

    async fn open_on(&self, page: &Page, req: &SolveRequest, host: &str) -> Result<Solution, String> {
        let ua = self.user_agent();
        if !ua.is_empty() {
            let _ = page.set_user_agent(ua.as_str()).await;
        }
        if !req.cookies.is_empty() {
            let list: Vec<CookieParam> = req
                .cookies
                .iter()
                .filter_map(|(k, v)| CookieParam::builder().name(k.clone()).value(v.clone()).url(req.url.clone()).build().ok())
                .collect();
            let _ = page.set_cookies(list).await;
        }
        let nav = match &req.post_data {
            Some(data) => page.set_content(post_page(&req.url, data)).await.map(|_| ()),
            None => page.goto(req.url.as_str()).await.map(|_| ()),
        };
        // an error status with an empty page is still an answer (its status goes back as it is)
        if let Err(e) = nav {
            let s = e.to_string();
            if s.contains("ERR_HTTP_RESPONSE_CODE_FAILURE") {
                return self.finish(page, req, false).await;
            }
            let what = if s.contains("ERR_NAME_NOT_RESOLVED") {
                "can't be found (DNS); check its address"
            } else if s.contains("ERR_CONNECTION") || s.contains("ERR_ADDRESS") {
                "can't be reached (connection refused or no route)"
            } else if s.contains("ERR_PROXY") || s.contains("ERR_TUNNEL") {
                "can't be reached through the proxy; check the proxy"
            } else if s.contains("ERR_CERT") || s.contains("ERR_SSL") {
                "has a certificate problem (HTTPS)"
            } else {
                "didn't open in the browser"
            };
            return Err(format!("{host} {what} ({s})"));
        }
        let js = state_js();
        let started = Instant::now();
        let deadline = req.timeout.max(Duration::from_secs(5));
        let mut challenged = false;
        let mut tries = 0u32;
        loop {
            let state: String = match page.evaluate(js.as_str()).await {
                Ok(v) => v.into_value().unwrap_or_default(),
                // the page is between two documents (the check reloads it)
                Err(_) => "loading".into(),
            };
            match state.as_str() {
                "ok" => break,
                "blocked" => {
                    return Err(format!(
                        "Cloudflare refused the request to {host} outright (Access denied): the address Lantern runs from may be blocked; try a proxy"
                    ))
                }
                "challenge" => challenged = true,
                _ => {}
            }
            if started.elapsed() >= deadline {
                return Err(if challenged {
                    format!(
                        "{host}'s Cloudflare check didn't let the browser through in {} s; try again later, or use a proxy",
                        deadline.as_secs()
                    )
                } else {
                    format!("{host} didn't finish loading in {} s", deadline.as_secs())
                });
            }
            // a check that waits for a click (Turnstile): after a few seconds, move to its box and tick it
            if challenged && started.elapsed() > Duration::from_secs(3 + 3 * tries as u64) && tries < 6 {
                tries += 1;
                tick_box(page).await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.finish(page, req, challenged).await
    }

    /// The answer for the page as it is now.
    async fn finish(&self, page: &Page, req: &SolveRequest, challenged: bool) -> Result<Solution, String> {
        let ua = self.user_agent();
        let url = page.url().await.ok().flatten().unwrap_or_else(|| req.url.clone());
        let status: u16 = page
            .evaluate("(performance.getEntriesByType('navigation')[0] || {}).responseStatus || 200")
            .await
            .ok()
            .and_then(|v| v.into_value::<u16>().ok())
            .unwrap_or(200);
        let response = if req.cookies_only { String::new() } else { page.content().await.unwrap_or_default() };
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
        Ok(Solution { url, status, response, cookies, user_agent: ua, challenged })
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

    /// Makes a session (FlareSolverr's `sessions.create`): its cookies last until it is destroyed.
    pub async fn create_session(&self, id: &str, proxy: &str) -> Result<String, String> {
        let id = if id.trim().is_empty() { uuid_like() } else { id.trim().to_string() };
        if self.inner.sessions.lock().await.contains_key(&id) {
            return Ok(id);
        }
        let browser = self.browser().await?;
        let context = new_context(&browser, proxy).await?;
        self.inner.sessions.lock().await.insert(id.clone(), Session { context, proxy: proxy.to_string() });
        Ok(id)
    }

    /// The sessions' ids.
    pub async fn sessions(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.inner.sessions.lock().await.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Ends a session: `false` when there was none by that id.
    pub async fn destroy_session(&self, id: &str) -> bool {
        let Some(s) = self.inner.sessions.lock().await.remove(id) else { return false };
        let browser = self.inner.running.lock().await.as_ref().map(|r| r.browser.clone());
        if let Some(b) = browser {
            let _ = b.dispose_browser_context(s.context).await;
        }
        true
    }
}

/// A fresh browser context (its own cookies), through `proxy` when given.
async fn new_context(browser: &Browser, proxy: &str) -> Result<BrowserContextId, String> {
    let mut params = CreateBrowserContextParams::builder().dispose_on_detach(true);
    if !proxy.trim().is_empty() {
        params = params.proxy_server(proxy.trim().to_string());
    }
    browser.create_browser_context(params.build()).await.map_err(|e| format!("The browser can't open a fresh window: {e}"))
}

/// Ticks a Turnstile box the way a person does with the keyboard: Tab to it, then Space.
async fn tick_box(page: &Page) {
    for key in ["Tab", " "] {
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
        tokio::time::sleep(Duration::from_millis(150)).await;
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

/// A random session id.
fn uuid_like() -> String {
    use rand::Rng;
    let b: [u8; 16] = rand::rng().random();
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
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
        assert_eq!(host_of("https://Tracker.Example.org/x?y=1"), "tracker.example.org");
        assert!(domain_matches(".example.org", "tracker.example.org"));
        assert!(domain_matches("example.org", "example.org"));
        assert!(!domain_matches("example.org", "badexample.org"));
    }

    #[test]
    fn a_post_is_a_form_that_sends_itself() {
        let p = post_page("https://x.test/login?a=\"1\"", "user=me&pass=a%26b");
        assert!(p.contains(r#"action="https://x.test/login?a=&quot;1&quot;""#), "{p}");
        assert!(p.contains(r#"name="pass" value="a&amp;b""#), "{p}");
        assert!(p.contains("submit()"));
    }

    #[test]
    fn the_state_script_has_its_lists() {
        let js = state_js();
        assert!(js.contains(r#""Just a moment...""#) && js.contains("#turnstile-wrapper") && !js.contains("SELECTORS"));
    }

    #[test]
    fn a_missing_browser_says_what_to_do() {
        let dir = std::env::temp_dir().join(format!("lantern-solver-{}", std::process::id()));
        let s = Solver::new(None, &dir, None);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let e = rt
            .block_on(s.solve(&SolveRequest { url: "https://x.test/".into(), timeout: Duration::from_secs(5), ..Default::default() }))
            .unwrap_err();
        assert!(e.contains("APP_CHROMIUM"), "{e}");
        let e = rt.block_on(s.solve(&SolveRequest { url: "ftp://x".into(), ..Default::default() })).unwrap_err();
        assert!(e.contains("isn't a web address"), "{e}");
    }
}
