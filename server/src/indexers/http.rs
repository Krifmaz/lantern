//! The HTTP client of one indexer: its own cookies, the site's request delay, the site's text encoding, and
//! plain-words errors (no answer, a Cloudflare check, an error page).

use reqwest::cookie::{CookieStore, Jar};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

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
    delay: Duration,
    last: Arc<Mutex<Option<Instant>>>,
    encoding: &'static encoding_rs::Encoding,
    name: String,
    timeout: Duration,
}

impl Http {
    /// A client for the site `name` with `delay` between requests and pages in `encoding` (`UTF-8` when empty or
    /// unknown).
    pub fn new(name: &str, delay_secs: f64, encoding: &str, proxy: Option<&str>) -> Http {
        let jar = Arc::new(Jar::default());
        let mut b = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_provider(jar.clone())
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60));
        if let Some(p) = proxy.filter(|p| !p.trim().is_empty()) {
            if let Ok(px) = reqwest::Proxy::all(p.trim()) {
                b = b.proxy(px);
            }
        }
        Http {
            client: b.build().unwrap_or_default(),
            jar,
            delay: Duration::from_secs_f64(delay_secs.clamp(0.0, 600.0)),
            last: Arc::new(Mutex::new(None)),
            encoding: encoding_rs::Encoding::for_label(encoding.as_bytes()).unwrap_or(encoding_rs::UTF_8),
            name: name.to_string(),
            timeout: Duration::from_secs(60),
        }
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

    /// Makes a request, waiting the site's delay since the last one.
    pub async fn fetch(&self, f: Fetch) -> Result<Page, String> {
        {
            let mut last = self.last.lock().await;
            if let Some(t) = *last {
                let since = t.elapsed();
                if since < self.delay {
                    tokio::time::sleep(self.delay - since).await;
                }
            }
            *last = Some(Instant::now());
        }
        let mut req = if f.post { self.client.post(&f.url) } else { self.client.get(&f.url) };
        if f.post {
            let mut body: String =
                f.form.iter().map(|(k, v)| format!("{}={}", enc(self.encoding, k), enc(self.encoding, v))).collect::<Vec<_>>().join("&");
            if !f.raw_body.is_empty() {
                if !body.is_empty() {
                    body.push('&');
                }
                body.push_str(f.raw_body.trim_start_matches('&'));
            }
            req = req.header("Content-Type", "application/x-www-form-urlencoded").body(body);
        }
        for (k, v) in &f.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.timeout(self.timeout).send().await.map_err(|e| self.reach_error(&f.url, &e))?;
        let status = resp.status().as_u16();
        let url = resp.url().to_string();
        let server = resp.headers().get("server").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
        let mitigated = resp.headers().contains_key("cf-mitigated");
        let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let bytes = resp.bytes().await.map_err(|e| format!("{} stopped answering mid-way: {e}", self.name))?.to_vec();
        let enc = charset(&content_type).unwrap_or(self.encoding);
        let (text, _, _) = enc.decode(&bytes);
        let text = text.into_owned();
        if mitigated
            || (server.contains("cloudflare") && matches!(status, 403 | 429 | 503) && is_challenge(&text))
            || (matches!(status, 403 | 503) && is_challenge(&text))
        {
            return Err(format!("{} {CLOUDFLARE}: Lantern can't get past it yet", self.name));
        }
        if status >= 500 {
            return Err(format!("{} is having trouble (it answered {status}); try again later", self.name));
        }
        if status == 429 {
            return Err(format!("{} says Lantern is asking too often (429); it will slow down", self.name));
        }
        if status == 401 || status == 403 {
            return Err(format!("{} refused the request ({status}): check the indexer's sign-in, API key or cookie", self.name));
        }
        if status == 404 {
            return Err(format!(
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
