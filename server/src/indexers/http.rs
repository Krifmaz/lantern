//! The HTTP client of one indexer: its own cookies, the wait between requests to a site (Prowlarr's rate limit,
//! [`rate_limit`](super::rate_limit)), the site's text encoding, its proxies ([`proxies`](super::proxies)) and
//! plain-words errors (no answer, a Cloudflare check, an error page).
//!
//! An indexer with a FlareSolverr proxy goes to it when a site answers with a Cloudflare or DDoS-Guard check, as
//! Prowlarr's `IndexerHttpClient` does. Without one, the check goes to Lantern's [solver](crate::solver) once: the
//! client then sends the cookies the browser got, with the browser's user agent, until they run out. When the
//! site still checks plain requests, a page is read through the browser instead.

use super::proxies::{self, FlareTarget};
use super::rate_limit;
use crate::solver::{host_of, SolveRequest, Solver};
use reqwest::cookie::{CookieStore, Jar};
use std::sync::Arc;
use std::time::Duration;

/// The browser Lantern says it is: sites block unknown clients.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// The sentence start of a Cloudflare check error, so callers can tell it apart.
pub const CLOUDFLARE: &str = "is behind a Cloudflare check";

/// One request.
#[derive(Debug, Clone)]
pub struct Fetch {
    /// The address.
    pub url: String,
    /// `GET` or `POST`.
    pub post: bool,
    /// Form fields for a POST.
    pub form: Vec<(String, String)>,
    /// Text added to a POST body as it is (`$raw`).
    pub raw_body: String,
    /// Extra headers.
    pub headers: Vec<(String, String)>,
    /// Follow redirects (always on; kept for the definitions' wording).
    pub follow: bool,
}

impl Fetch {
    /// A GET.
    pub fn get(url: &str) -> Fetch {
        Fetch { url: url.to_string(), post: false, form: Vec::new(), raw_body: String::new(), headers: Vec::new(), follow: true }
    }

    /// A form POST.
    pub fn post_form(url: &str, form: Vec<(String, String)>) -> Fetch {
        Fetch { post: true, form, ..Fetch::get(url) }
    }

    /// With these headers too.
    pub fn headers(mut self, h: Vec<(String, String)>) -> Fetch {
        self.headers.extend(h);
        self
    }

    /// With a Referer.
    pub fn referer(mut self, r: &str) -> Fetch {
        self.headers.push(("Referer".into(), r.to_string()));
        self
    }

    /// Follow redirects or not.
    pub fn follow(mut self, f: bool) -> Fetch {
        self.follow = f;
        self
    }
}

/// An answer.
#[derive(Debug, Clone, Default)]
pub struct Page {
    /// The address it came from in the end.
    pub url: String,
    /// Whether it was redirected there.
    pub redirected: bool,
    /// The HTTP status.
    pub status: u16,
    /// The body as text, in the site's encoding.
    pub text: String,
    /// The body as it came.
    pub bytes: Vec<u8>,
    /// The content type.
    pub content_type: String,
}

/// The client of one indexer.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    jar: Arc<Jar>,
    /// The indexer's own least seconds between requests (the definition's `requestDelay`, or its `request_delay`).
    delay_secs: f64,
    encoding: &'static encoding_rs::Encoding,
    name: String,
    timeout: Duration,
    /// The solver for Cloudflare checks, when there is one.
    solver: Option<Solver>,
    /// The proxy the client goes through ("" for none); the solver's browser uses the same one.
    proxy: String,
    /// The FlareSolverr asked when a site shows a Cloudflare check, when the indexer has one.
    flaresolverr: Option<FlareTarget>,
    /// The general proxy (`site_proxy`), which FlareSolverr is told to use.
    site_proxy: String,
}

impl Http {
    /// A client for the site `name` waiting at least `delay_secs` between requests (and Prowlarr's 2 seconds, see
    /// [`rate_limit::interval`]), with pages in `encoding` (`UTF-8` when empty or unknown), through `proxy`.
    pub fn new(name: &str, delay_secs: f64, encoding: &str, proxy: Option<&str>) -> Http {
        let jar = Arc::new(Jar::default());
        let mut b = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_provider(jar.clone())
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60));
        let proxy = proxy.map(str::trim).unwrap_or("").to_string();
        if !proxy.is_empty() {
            if let Ok(px) = reqwest::Proxy::all(&proxy) {
                b = b.proxy(px);
            }
        }
        Http {
            client: b.build().unwrap_or_default(),
            jar,
            delay_secs,
            encoding: encoding_rs::Encoding::for_label(encoding.as_bytes()).unwrap_or(encoding_rs::UTF_8),
            name: name.to_string(),
            timeout: Duration::from_secs(60),
            solver: None,
            proxy,
            flaresolverr: None,
            site_proxy: String::new(),
        }
    }

    /// Asking `target` (a FlareSolverr proxy) when a site shows a Cloudflare check, telling it to use
    /// `site_proxy`; Lantern's own solver is then not used for this client's checks.
    pub fn with_flaresolverr(mut self, target: Option<FlareTarget>, site_proxy: &str) -> Http {
        self.flaresolverr = target;
        self.site_proxy = site_proxy.trim().to_string();
        self
    }

    /// With `solver` for Cloudflare checks.
    pub fn with_solver(mut self, solver: Option<Solver>) -> Http {
        self.solver = solver;
        self
    }

    /// Adds cookies (`a=1; b=2`) for the site at `url`.
    pub fn add_cookies(&self, url: &str, cookies: &str) {
        let Ok(u) = url::Url::parse(url) else { return };
        for c in cookies.split(';').map(str::trim).filter(|c| c.contains('=')) {
            self.jar.add_cookie_str(&format!("{c}; Path=/"), &u);
        }
    }

    /// The cookies the site at `url` has set (`a=1; b=2`).
    pub fn cookies(&self, url: &str) -> String {
        let Ok(u) = url::Url::parse(url) else { return String::new() };
        self.jar.cookies(&u).and_then(|h| h.to_str().ok().map(str::to_string)).unwrap_or_default()
    }

    /// Makes a request, waiting the site's rate limit. A Cloudflare check goes to the indexer's FlareSolverr proxy
    /// when it has one, else it is solved once by Lantern's solver (see the module's words).
    pub async fn fetch(&self, f: Fetch) -> Result<Page, String> {
        if self.flaresolverr.is_some() {
            return match self.send(&f, None).await {
                Ok(p) => Ok(p),
                Err(Sent::Failed(e)) => Err(e),
                Err(Sent::Challenge) => Err(format!("{} {CLOUDFLARE} that FlareSolverr couldn't get past", self.name)),
            };
        }
        let host = host_of(&f.url);
        let kept = self.solver.as_ref().and_then(|s| s.clearance(&host, &self.proxy));
        if let Some(c) = &kept {
            self.add_cookies(&f.url, &c.header());
        }
        let agent = kept.as_ref().map(|c| c.user_agent.clone());
        match self.send(&f, agent.as_deref()).await {
            Err(Sent::Challenge) => {}
            Err(Sent::Failed(e)) => return Err(e),
            Ok(p) => return Ok(p),
        }
        let Some(solver) = self.solver.clone() else {
            return Err(format!(
                "{} {CLOUDFLARE}, and there is no browser to get past it: use the Lantern image, or set APP_CHROMIUM",
                self.name
            ));
        };
        if kept.is_some() {
            solver.forget(&host, &self.proxy);
        }
        let req = SolveRequest {
            url: f.url.clone(),
            post_data: f.post.then(|| self.form_body(&f)),
            timeout: Duration::from_secs(60),
            proxy: self.proxy.clone(),
            cookies: self.cookie_pairs(&f.url),
            session: String::new(),
            cookies_only: false,
        };
        let sol = solver.solve(&req).await.map_err(|e| format!("{} {CLOUDFLARE} that Lantern couldn't get past: {e}", self.name))?;
        let header: String = sol
            .cookies
            .iter()
            .filter(|c| crate::solver::domain_matches(&c.domain, &host))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        self.add_cookies(&f.url, &header);
        match self.send(&f, Some(&sol.user_agent)).await {
            Ok(p) => Ok(p),
            Err(Sent::Failed(e)) => Err(e),
            // plain requests are still checked: the browser's own page stands in (pages, not files)
            Err(Sent::Challenge) if !f.post && !sol.response.is_empty() => {
                let bytes = sol.response.clone().into_bytes();
                Ok(Page {
                    redirected: sol.url != f.url,
                    url: sol.url,
                    status: sol.status,
                    text: sol.response,
                    bytes,
                    content_type: "text/html".into(),
                })
            }
            Err(Sent::Challenge) => {
                Err(format!("{} {CLOUDFLARE} that lets the browser through but not Lantern's own requests; try a proxy", self.name))
            }
        }
    }

    /// The cookies the site at `url` has set, as name-value pairs.
    fn cookie_pairs(&self, url: &str) -> Vec<(String, String)> {
        self.cookies(url).split(';').filter_map(|c| c.trim().split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// The body of a form POST, in the site's encoding.
    fn form_body(&self, f: &Fetch) -> String {
        let mut body: String =
            f.form.iter().map(|(k, v)| format!("{}={}", enc(self.encoding, k), enc(self.encoding, v))).collect::<Vec<_>>().join("&");
        if !f.raw_body.is_empty() {
            if !body.is_empty() {
                body.push('&');
            }
            body.push_str(f.raw_body.trim_start_matches('&'));
        }
        body
    }

    /// One request, sending `agent` as the user agent when given. With a FlareSolverr proxy, the user agent its
    /// browser had for the site is sent when there is no other, and a Cloudflare check is handed to it
    /// (`FlareSolverr.PreRequest` and `PostResponse`).
    async fn send(&self, f: &Fetch, agent: Option<&str>) -> Result<Page, Sent> {
        let host = host_of(&f.url);
        let kept_agent = self.flaresolverr.as_ref().and_then(|_| proxies::user_agent_for(&host));
        let agent = agent.filter(|a| !a.is_empty()).map(str::to_string).or(kept_agent);
        let mut raw = self.raw(f, agent.as_deref()).await?;
        if let Some(target) = &self.flaresolverr {
            if proxies::is_cloudflare_protected(raw.status, &raw.server, &raw.vary, &raw.content_encoding, &raw.text) {
                raw = self.through_flaresolverr(target, f, &host).await?;
            }
        }
        self.classify(f, raw)
    }

    /// Asks the FlareSolverr `target` to get past the check on `f`'s site, keeps its user agent for the site, and
    /// asks the site again with the cookies it got (`FlareSolverr.PostResponse`).
    async fn through_flaresolverr(&self, target: &FlareTarget, f: &Fetch, host: &str) -> Result<Raw, Sent> {
        let fs_failed =
            |e: String| Sent::Failed(format!("{} {CLOUDFLARE}, and FlareSolverr at {} couldn't get past it: {e}", self.name, target.host));
        let body = f.post.then(|| self.form_body(f));
        let post = body.as_deref().map(|b| ("application/x-www-form-urlencoded", b));
        let command = proxies::flaresolverr_request(&f.url, post, target.timeout, &self.site_proxy).map_err(fs_failed)?;
        let answer = proxies::ask_flaresolverr(target, &command, self.solver.as_ref(), &self.site_proxy).await.map_err(fs_failed)?;
        let (cookies, agent) = proxies::read_solution(&answer).map_err(fs_failed)?;
        proxies::user_agents().lock().unwrap_or_else(|e| e.into_inner()).insert(host.to_string(), agent.clone());
        let header = cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
        self.add_cookies(&f.url, &header);
        self.raw(f, Some(&agent)).await
    }

    /// One request as it is, after the site's rate limit: what came back, before any check of it.
    async fn raw(&self, f: &Fetch, agent: Option<&str>) -> Result<Raw, Sent> {
        let host = host_of(&f.url);
        rate_limit::shared().wait_and_pulse(&host, rate_limit::interval(&host, self.delay_secs)).await;
        let mut req = if f.post { self.client.post(&f.url) } else { self.client.get(&f.url) };
        if f.post {
            req = req.header("Content-Type", "application/x-www-form-urlencoded").body(self.form_body(f));
        }
        for (k, v) in &f.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(a) = agent.filter(|a| !a.is_empty()) {
            req = req.header("User-Agent", a);
        }
        let resp = req.timeout(self.timeout).send().await.map_err(|e| Sent::Failed(self.reach_error(&f.url, &e)))?;
        let header = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let (server, vary, content_encoding, content_type) =
            (header("server"), header("vary"), header("content-encoding"), header("content-type"));
        let mitigated = resp.headers().contains_key("cf-mitigated");
        let status = resp.status().as_u16();
        let url = resp.url().to_string();
        let bytes = resp.bytes().await.map_err(|e| Sent::Failed(format!("{} stopped answering mid-way: {e}", self.name)))?.to_vec();
        let enc = charset(&content_type).unwrap_or(self.encoding);
        let (text, _, _) = enc.decode(&bytes);
        Ok(Raw { url, status, server, vary, content_encoding, content_type, mitigated, text: text.into_owned(), bytes })
    }

    /// What an answer means: the page, a check to solve, or an error in words.
    fn classify(&self, f: &Fetch, raw: Raw) -> Result<Page, Sent> {
        let Raw { url, status, server, content_type, mitigated, text, bytes, .. } = raw;
        let server = server.to_lowercase();
        if mitigated
            || (server.contains("cloudflare") && matches!(status, 403 | 429 | 503) && is_challenge(&text))
            || (matches!(status, 403 | 503) && is_challenge(&text))
        {
            return Err(Sent::Challenge);
        }
        let failed = |e: String| Err(Sent::Failed(e));
        if status >= 500 {
            return failed(format!("{} is having trouble (it answered {status}); try again later", self.name));
        }
        if status == 429 {
            return failed(format!("{} says Lantern is asking too often (429); it will slow down", self.name));
        }
        if status == 401 || status == 403 {
            return failed(format!("{} refused the request ({status}): check the indexer's sign-in, API key or cookie", self.name));
        }
        if status == 404 {
            return failed(format!(
                "{} has no page at {} (404): the site may have moved; check its address",
                self.name,
                strip_secrets(&f.url)
            ));
        }
        let redirected = url.split('#').next() != f.url.split('#').next();
        Ok(Page { url, redirected, status, text, bytes, content_type })
    }

    fn reach_error(&self, url: &str, e: &reqwest::Error) -> String {
        let what = if e.is_timeout() {
            "didn't answer in time"
        } else if e.is_connect() {
            let s = format!("{e:?}").to_lowercase();
            if s.contains("dns") || s.contains("resolve") || s.contains("lookup") {
                "can't be found (DNS); check its address"
            } else if s.contains("certificate") || s.contains("tls") {
                "has a certificate problem (HTTPS)"
            } else {
                "can't be reached (connection refused or no route)"
            }
        } else {
            "can't be reached"
        };
        format!("{} {what} – {}", self.name, strip_secrets(url))
    }
}

/// One answer as it came, before any check of it.
struct Raw {
    /// The address it came from in the end.
    url: String,
    /// The HTTP status.
    status: u16,
    /// Its `Server` header.
    server: String,
    /// Its `Vary` header.
    vary: String,
    /// Its `Content-Encoding` header.
    content_encoding: String,
    /// Its `Content-Type` header.
    content_type: String,
    /// It has Cloudflare's `cf-mitigated` header.
    mitigated: bool,
    /// The body as text.
    text: String,
    /// The body as it came.
    bytes: Vec<u8>,
}

/// How one request went wrong: a check to solve, or anything else in words.
enum Sent {
    Challenge,
    Failed(String),
}

/// `url` with its query left out, so keys and passkeys never end up in a message or log.
pub fn strip_secrets(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

fn enc(e: &'static encoding_rs::Encoding, s: &str) -> String {
    let (bytes, _, _) = e.encode(s);
    url::form_urlencoded::byte_serialize(&bytes).collect()
}

fn charset(content_type: &str) -> Option<&'static encoding_rs::Encoding> {
    let lower = content_type.to_lowercase();
    let cs = lower.split("charset=").nth(1)?.split(';').next()?.trim().trim_matches('"');
    encoding_rs::Encoding::for_label(cs.as_bytes())
}

/// Whether a page is a Cloudflare (or DDoS-Guard) browser check.
pub fn is_challenge(text: &str) -> bool {
    let head = &text[..text.len().min(20000)];
    [
        "<title>Just a moment...</title>",
        "challenge-platform",
        "cf-chl-",
        "cf_chl_opt",
        "<title>Attention Required! | Cloudflare</title>",
        "ddos-guard",
        "DDoS-Guard",
    ]
    .iter()
    .any(|m| head.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_challenges_and_hides_keys() {
        assert!(is_challenge("<html><head><title>Just a moment...</title>"));
        assert!(!is_challenge("<html><body>results</body>"));
        assert_eq!(strip_secrets("https://x.test/api?apikey=secret"), "https://x.test/api");
    }
}
