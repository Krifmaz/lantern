// Ported from autobrr v1.90.0 (internal/notification/sender_*.go and services/{discord,gotify,lunasea,notifiarr,
// ntfy,pushover,shoutrrr,telegram,webhook,bark}), internal/domain/notification.go (NewWebhookEvent),
// internal/stringutils (TruncateStr), GPL-2.0-or-later.
//! How each kind of notification is sent: the message each builds from a [`Payload`], the request it makes, the
//! retries it allows (three tries, waiting what a 429 asks within ten seconds) and the error words it gives.

use super::{html_body, plain_body, title, Notification, Payload};
use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::{Method, StatusCode};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// The image LunaSea and Bark show (autobrr's `defaultImageURL`).
pub const DEFAULT_IMAGE_URL: &str = "https://raw.githubusercontent.com/autobrr/autobrr/master/.github/images/logo.png";

/// Discord's limits per embed part.
pub const EMBED_TITLE_LIMIT: usize = 256;
/// Discord's description limit.
pub const EMBED_DESCRIPTION_LIMIT: usize = 4096;
/// Discord's field value limit.
pub const EMBED_FIELD_VALUE_LIMIT: usize = 1024;

/// The longest wait a 429 may ask for before the send gives up (autobrr's `maxRetryAfter`).
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

/// Where each kind sends and how long it waits between tries; autobrr's values by default (tests point the
/// fixed services at a local stand-in and shorten the waits).
#[derive(Debug, Clone)]
pub struct Senders {
    /// The web client.
    pub http: reqwest::Client,
    /// Notifiarr's API.
    pub notifiarr_endpoint: String,
    /// Pushover's messages API.
    pub pushover_endpoint: String,
    /// Pushover's sounds API.
    pub pushover_sounds_endpoint: String,
    /// Telegram's API when no host is set.
    pub telegram_host: String,
    /// The first wait between tries, per service: Discord, Telegram, Notifiarr (doubling), Ntfy, Pushover (fixed).
    pub discord_retry: Duration,
    /// Telegram's.
    pub telegram_retry: Duration,
    /// Notifiarr's.
    pub notifiarr_retry: Duration,
    /// Ntfy's.
    pub ntfy_retry: Duration,
    /// Pushover's.
    pub pushover_retry: Duration,
    /// The wait for a 429 that says none: Discord and Telegram.
    pub short_retry_after: Duration,
    /// The wait for a 429 that says none: Ntfy and Notifiarr.
    pub long_retry_after: Duration,
    /// The pause between test events.
    pub test_gap: Duration,
    /// The user agent where autobrr sends its own (`autobrr/<version>`).
    pub user_agent: String,
}

impl Default for Senders {
    fn default() -> Senders {
        Senders {
            http: reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_default(),
            notifiarr_endpoint: "https://notifiarr.com/api/v1/notification/autobrr".into(),
            pushover_endpoint: "https://api.pushover.net/1/messages.json".into(),
            pushover_sounds_endpoint: "https://api.pushover.net/1/sounds.json".into(),
            telegram_host: "https://api.telegram.org".into(),
            discord_retry: Duration::from_millis(500),
            telegram_retry: Duration::from_millis(500),
            notifiarr_retry: Duration::from_millis(500),
            ntfy_retry: Duration::from_secs(5),
            pushover_retry: Duration::from_secs(5),
            short_retry_after: Duration::from_secs(2),
            long_retry_after: Duration::from_secs(5),
            test_gap: Duration::from_secs(1),
            user_agent: concat!("autobrr (Lantern ", env!("CARGO_PKG_VERSION"), ")").into(),
        }
    }
}

/// One try's outcome.
enum Step {
    /// Try again (after the wait, when the server named one).
    Again(String, Option<Duration>),
    /// Stop with this error.
    Stop(String),
}

/// How the wait between tries grows.
#[derive(Clone, Copy)]
enum Delay {
    /// Doubles each time (retry-go's `BackOffDelay`).
    Backoff,
    /// Stays the same (retry-go's `FixedDelay`).
    Fixed,
}

/// Three tries of `once`, as autobrr's retry-go setup does: the last error only.
async fn retry<F, Fut>(delay: Duration, kind: Delay, mut once: F) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), Step>>,
{
    let mut last = String::new();
    for n in 0..3u32 {
        match once().await {
            Ok(()) => return Ok(()),
            Err(Step::Stop(e)) => return Err(e),
            Err(Step::Again(e, wait)) => {
                last = e;
                if n == 2 {
                    break;
                }
                let d = wait.unwrap_or(match kind {
                    Delay::Backoff => delay * (1 << n),
                    Delay::Fixed => delay,
                });
                tokio::time::sleep(d).await;
            }
        }
    }
    Err(last)
}

/// Go's `time.Duration.String` for the waits in errors (`2m0.5s`, `300ms`).
pub fn go_duration(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns == 0 {
        return "0s".into();
    }
    if ns < 1_000_000_000 {
        let (v, unit) = if ns < 1_000 {
            return format!("{ns}ns");
        } else if ns < 1_000_000 {
            (ns as f64 / 1e3, "µs")
        } else {
            (ns as f64 / 1e6, "ms")
        };
        return format!("{}{unit}", trim_float(v));
    }
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    let frac = d.subsec_nanos();
    let secs = if frac == 0 { format!("{s}") } else { format!("{s}.{}", format!("{frac:09}").trim_end_matches('0')) };
    if h > 0 {
        format!("{h}h{m}m{secs}s")
    } else if m > 0 {
        format!("{m}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

/// A float without trailing zeros.
fn trim_float(v: f64) -> String {
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The wait a `Retry-After` header names (seconds, or an HTTP date), else `fallback`.
fn retry_after_header(h: Option<&str>, fallback: Duration, fractional: bool) -> Duration {
    let Some(v) = h.filter(|v| !v.is_empty()) else { return fallback };
    if fractional {
        if let Ok(s) = v.parse::<f64>() {
            if s > 0.0 {
                return Duration::from_secs_f64(s);
            }
        }
    } else if let Ok(s) = v.parse::<i64>() {
        if s > 0 {
            return Duration::from_secs(s as u64);
        }
    }
    if let Ok(at) = DateTime::parse_from_rfc2822(v) {
        if let Ok(until) = (at.with_timezone(&Utc) - Utc::now()).to_std() {
            if !until.is_zero() {
                return until;
            }
        }
    }
    fallback
}

/// A response's status and body (the first 4 KiB).
async fn read(res: reqwest::Response) -> (StatusCode, Option<String>, String) {
    let status = res.status();
    let retry_after = res.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = res.bytes().await.map(|b| String::from_utf8_lossy(&b[..b.len().min(4096)]).into_owned()).unwrap_or_default();
    (status, retry_after, body)
}

/// A request error in Go's words.
fn request_error(e: reqwest::Error) -> String {
    format!("client request error: {e}")
}

/// `s` cut to `limit` characters by dropping the middle (autobrr's `TruncateStr`).
pub fn truncate_str(s: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let runes: Vec<char> = s.chars().collect();
    if runes.len() <= limit {
        return s.to_string();
    }
    const MARKER: &str = " [...] ";
    let marker = MARKER.chars().count();
    if limit <= marker {
        return runes[..limit].iter().collect();
    }
    let keep = limit - marker;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    format!("{}{MARKER}{}", runes[..head].iter().collect::<String>(), runes[runes.len() - tail..].iter().collect::<String>())
}

/// Go's `url.JoinPath(base, elems...)` for the simple cases the senders use.
fn join_path(base: &str, elems: &[&str]) -> Result<String, String> {
    let mut u = url::Url::parse(base).map_err(|e| e.to_string())?;
    let mut path = u.path().trim_end_matches('/').to_string();
    for e in elems {
        path.push('/');
        path.push_str(e.trim_matches('/'));
    }
    u.set_path(&path);
    Ok(u.to_string())
}

impl Senders {
    /// Sends `p` through `n` by its kind.
    pub async fn send(&self, n: &Notification, p: &Payload) -> Result<(), String> {
        match n.kind.as_str() {
            "DISCORD" => {
                let m = discord_message(p)?;
                self.discord(&n.webhook, &m).await
            }
            "GOTIFY" => self.gotify(&n.host, &n.token, title(&p.event), &plain_body(p)).await,
            "LUNASEA" => {
                let m = json!({ "title": title(&p.event), "body": plain_body(p), "image": DEFAULT_IMAGE_URL });
                self.lunasea(&n.webhook, &m).await
            }
            "NOTIFIARR" => self.notifiarr(&n.api_key, &notifiarr_message(p)).await,
            "NTFY" => self.ntfy(n, title(&p.event), &plain_body(p)).await,
            "PUSHOVER" => {
                let sound = n.event_sounds.get(&p.event).filter(|s| !s.is_empty()).unwrap_or(&n.sound).clone();
                self.pushover(&n.api_key, &n.token, title(&p.event), &html_body(p), n.priority, &sound, Some(Utc::now())).await
            }
            "SHOUTRRR" => self.shoutrrr(&n.host, &plain_body(p)).await,
            "TELEGRAM" => {
                let thread = match n.topic.as_str() {
                    "" => 0,
                    t => t.parse::<i64>().unwrap_or_else(|e| {
                        tracing::warn!("telegram: could not parse specified topic as an integer: {t}: {e}");
                        0
                    }),
                };
                let mut p = p.clone();
                p.sender.clone_from(&n.username);
                self.telegram(&n.host, &n.token, &n.channel, thread, &html_body(&p)).await
            }
            "WEBHOOK" => {
                let ev = webhook_event(p, &uuid_v7());
                self.webhook(&n.webhook, &n.method, &n.headers, &ev).await
            }
            "BARK" => {
                let level = match n.priority {
                    -1 => "passive",
                    1 => "timeSensitive",
                    2 => "critical",
                    _ => "",
                };
                let mut m = Map::new();
                m.insert("device_key".into(), json!(n.token));
                let t = title(&p.event);
                if !t.is_empty() {
                    m.insert("title".into(), json!(t));
                }
                m.insert("body".into(), json!(plain_body(p)));
                for (k, v) in [("sound", n.sound.as_str()), ("group", n.topic.as_str()), ("level", level), ("icon", DEFAULT_IMAGE_URL), ("url", &p.url)] {
                    if !v.is_empty() {
                        m.insert(k.into(), json!(v));
                    }
                }
                self.bark(&n.host, &Value::Object(m)).await
            }
            other => Err(format!("unsupported notification type: {other}")),
        }
    }

    /// Discord's webhook (services/discord `SendMessage`).
    pub async fn discord(&self, webhook: &str, m: &Value) -> Result<(), String> {
        let body = m.to_string();
        retry(self.discord_retry, Delay::Backoff, || async {
            let res = self
                .http
                .post(webhook)
                .header("Content-Type", "application/json")
                .header("User-Agent", "autobrr")
                .body(body.clone())
                .send()
                .await
                .map_err(|e| Step::Again(request_error(e), None))?;
            let (status, ra, body) = read(res).await;
            match status.as_u16() {
                200 | 204 => Ok(()),
                429 => {
                    let v: Value = serde_json::from_str(&body).unwrap_or_default();
                    let err = discord_status_error(status.as_u16(), &body);
                    let delay = match v["retry_after"].as_f64() {
                        Some(s) if s > 0.0 => Duration::from_secs_f64(s),
                        _ => retry_after_header(ra.as_deref(), self.short_retry_after, true),
                    };
                    if delay > MAX_RETRY_AFTER {
                        return Err(Step::Stop(format!("server demanded {} wait which exceeds the retry budget: {err}", go_duration(delay))));
                    }
                    Err(Step::Again(err, Some(delay)))
                }
                400 => Err(Step::Stop(format!("discord rejected the message payload: {}", discord_status_error(400, &body)))),
                401 | 403 | 404 => Err(Step::Stop(format!(
                    "discord webhook not usable - check the webhook URL, it may have been removed: {}",
                    discord_status_error(status.as_u16(), &body)
                ))),
                s => Err(Step::Again(discord_status_error(s, &body), None)),
            }
        })
        .await
    }

    /// Gotify's message API (services/gotify).
    pub async fn gotify(&self, host: &str, token: &str, title: &str, message: &str) -> Result<(), String> {
        let endpoint = join_path(host, &["message"]).map_err(|e| format!("could not build url from host: '{host}': {e}"))?;
        let form = url::form_urlencoded::Serializer::new(String::new()).append_pair("message", message).append_pair("title", title).finish();
        let res = self
            .http
            .post(endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "autobrr")
            .header("X-Gotify-Key", token)
            .body(form)
            .send()
            .await
            .map_err(request_error)?;
        let (status, _, body) = read(res).await;
        if status == StatusCode::OK {
            return Ok(());
        }
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        let err = match (v["error"].as_str().filter(|s| !s.is_empty()), v["errorDescription"].as_str().filter(|s| !s.is_empty())) {
            (Some(e), Some(d)) => format!("unexpected status: {} error: {e}: {d}", status.as_u16()),
            (Some(e), None) => format!("unexpected status: {} error: {e}", status.as_u16()),
            _ if !body.is_empty() => format!("unexpected status: {} body: {body}", status.as_u16()),
            _ => format!("unexpected status: {}", status.as_u16()),
        };
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(format!("check gotify application token: {err}"));
        }
        Err(err)
    }

    /// LunaSea's custom webhook (services/lunasea); an *arr address is pointed at `/custom/`.
    pub async fn lunasea(&self, webhook: &str, m: &Value) -> Result<(), String> {
        let url = rewrite_lunasea_url(webhook);
        let res = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "autobrr")
            .body(m.to_string())
            .send()
            .await
            .map_err(request_error)?;
        let (status, _, body) = read(res).await;
        if status == StatusCode::OK {
            return Ok(());
        }
        let err = if body.is_empty() {
            format!("unexpected status: {}", status.as_u16())
        } else {
            format!("unexpected status: {} body: {body}", status.as_u16())
        };
        if status == StatusCode::NOT_FOUND {
            return Err(format!("webhook not found: check the webhook URL, the device or user id is likely wrong: {err}"));
        }
        Err(err)
    }

    /// Notifiarr's API (services/notifiarr).
    pub async fn notifiarr(&self, api_key: &str, m: &Value) -> Result<(), String> {
        if api_key.is_empty() {
            return Err("missing notifiarr api key".into());
        }
        let body = m.to_string();
        retry(self.notifiarr_retry, Delay::Backoff, || async {
            let res = self
                .http
                .post(&self.notifiarr_endpoint)
                .header("Content-Type", "application/json")
                .header("User-Agent", &self.user_agent)
                .header("X-API-Key", api_key)
                .body(body.clone())
                .send()
                .await
                .map_err(|e| Step::Again(request_error(e), None))?;
            let (status, ra, body) = read(res).await;
            let s = status.as_u16();
            let err = || {
                let v: Value = serde_json::from_str(&body).unwrap_or_default();
                let detail = v["details"]["response"].as_str().filter(|d| !d.is_empty()).map(str::to_string).unwrap_or_else(|| body.trim().to_string());
                if detail.is_empty() {
                    format!("unexpected status: {s}")
                } else {
                    format!("unexpected status: {s}: {detail}")
                }
            };
            match s {
                200 => Ok(()),
                429 => {
                    let delay = retry_after_header(ra.as_deref(), self.long_retry_after, false);
                    if delay > MAX_RETRY_AFTER {
                        return Err(Step::Stop(format!("server demanded {} wait which exceeds the retry budget: {}", go_duration(delay), err())));
                    }
                    Err(Step::Again(err(), Some(delay)))
                }
                401 | 403 => Err(Step::Stop(format!("check notifiarr api key: {}", err()))),
                400..=499 if s != 408 => Err(Step::Stop(err())),
                _ => Err(Step::Again(err(), None)),
            }
        })
        .await
    }

    /// An ntfy topic (services/ntfy).
    pub async fn ntfy(&self, n: &Notification, title: &str, message: &str) -> Result<(), String> {
        retry(self.ntfy_retry, Delay::Fixed, || async {
            let mut req = self
                .http
                .post(&n.host)
                .header("Content-Type", "text/plain")
                .header("User-Agent", "autobrr")
                .header("Title", title)
                .body(message.to_string());
            if n.priority > 0 {
                req = req.header("Priority", n.priority.to_string());
            }
            if !n.topic.is_empty() {
                req = req.header("Tags", &n.topic);
            }
            if !n.username.is_empty() && !n.password.is_empty() {
                req = req.basic_auth(&n.username, Some(&n.password));
            } else if !n.token.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", n.token));
            }
            let res = req.send().await.map_err(|e| Step::Again(request_error(e), None))?;
            let (status, ra, body) = read(res).await;
            let s = status.as_u16();
            let err = || {
                let v: Value = serde_json::from_str(&body).unwrap_or_default();
                match v["error"].as_str().filter(|e| !e.is_empty()) {
                    Some(e) => format!("unexpected status: {s} error: {e} code: {}", v["code"].as_i64().unwrap_or(0)),
                    None if !body.trim().is_empty() => format!("unexpected status: {s} body: {}", body.trim()),
                    None => format!("unexpected status: {s}"),
                }
            };
            match s {
                200 => Ok(()),
                401 | 403 => Err(Step::Stop(format!("could not authenticate with ntfy: check access token or username/password: {}", err()))),
                413 => Err(Step::Stop(format!("message too large for ntfy: {}", err()))),
                429 => {
                    let e = format!("rate limited by ntfy: {}", err());
                    let delay = retry_after_header(ra.as_deref(), self.long_retry_after, false);
                    if delay > MAX_RETRY_AFTER {
                        return Err(Step::Stop(format!("Retry-After {} exceeds the retry budget: {e}", go_duration(delay))));
                    }
                    Err(Step::Again(e, Some(delay)))
                }
                500.. => Err(Step::Again(err(), None)),
                _ => Err(Step::Stop(err())),
            }
        })
        .await
    }

    /// Pushover's messages API (services/pushover).
    #[allow(clippy::too_many_arguments)]
    pub async fn pushover(
        &self,
        token: &str,
        user: &str,
        title: &str,
        message: &str,
        priority: i32,
        sound: &str,
        at: Option<DateTime<Utc>>,
    ) -> Result<(), String> {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        let mut pairs: Vec<(&str, String)> = vec![
            ("token", token.into()),
            ("user", user.into()),
            ("message", message.into()),
            ("title", title.into()),
            ("priority", priority.to_string()),
        ];
        if let Some(t) = at {
            pairs.push(("timestamp", t.timestamp().to_string()));
        }
        pairs.push(("html", "1".into()));
        if !sound.is_empty() {
            pairs.push(("sound", sound.into()));
        }
        if priority == 2 {
            pairs.push(("expire", "3600".into()));
            pairs.push(("retry", "60".into()));
        }
        // Go's url.Values.Encode sorts by key
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in &pairs {
            form.append_pair(k, v);
        }
        let encoded = form.finish();
        retry(self.pushover_retry, Delay::Fixed, || async {
            let res = self
                .http
                .post(&self.pushover_endpoint)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("User-Agent", "autobrr")
                .body(encoded.clone())
                .send()
                .await
                .map_err(|e| Step::Again(request_error(e), None))?;
            let (status, _, body) = read(res).await;
            let s = status.as_u16();
            let err = pushover_status_error(s, &body);
            match s {
                200 => Ok(()),
                429 => Err(Step::Stop(format!("monthly message quota exhausted, resets on the 1st of the month: {err}"))),
                500.. => Err(Step::Again(err, None)),
                401 | 403 => Err(Step::Stop(format!("check pushover api token and user key: {err}"))),
                _ => Err(Step::Stop(err)),
            }
        })
        .await
    }

    /// Pushover's sounds for an app token (services/pushover `GetSounds`).
    pub async fn pushover_sounds(&self, token: &str) -> Result<Value, String> {
        if token.is_empty() {
            return Err("api token is required".into());
        }
        let url = format!("{}?token={}", self.pushover_sounds_endpoint, url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>());
        let res = self.http.get(url).header("User-Agent", "autobrr").send().await.map_err(request_error)?;
        let (status, _, body) = read(res).await;
        if status != StatusCode::OK {
            return Err(pushover_status_error(status.as_u16(), &body));
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| format!("could not decode response: {e}"))?;
        if v["status"].as_i64() != Some(1) {
            return Err(format!("pushover api returned error status: {}", v["status"].as_i64().unwrap_or(0)));
        }
        Ok(if v["sounds"].is_object() { v["sounds"].clone() } else { Value::Null })
    }

    /// Telegram's bot API (services/telegram); the token never shows in an error.
    pub async fn telegram(&self, host: &str, token: &str, chat: &str, thread: i64, text: &str) -> Result<(), String> {
        let host = if host.is_empty() { self.telegram_host.as_str() } else { host };
        let mut m = Map::new();
        m.insert("chat_id".into(), json!(chat));
        m.insert("text".into(), json!(text));
        m.insert("parse_mode".into(), json!("HTML"));
        if thread != 0 {
            m.insert("message_thread_id".into(), json!(thread));
        }
        let body = Value::Object(m).to_string();
        let endpoint = join_path(host, &[&format!("bot{token}"), "sendMessage"])
            .map_err(|_| format!("could not build telegram api url: check custom host: {host}"))?;
        retry(self.telegram_retry, Delay::Backoff, || async {
            let res = self
                .http
                .post(&endpoint)
                .header("Content-Type", "application/json")
                .header("User-Agent", "autobrr")
                .body(body.clone())
                .send()
                .await
                .map_err(|e| Step::Again(format!("client request error: {}", e.without_url()), None))?;
            let (status, ra, body) = read(res).await;
            let s = status.as_u16();
            let v: Value = serde_json::from_str(&body).unwrap_or_default();
            let err = {
                let detail = v["description"].as_str().filter(|d| !d.is_empty()).map(str::to_string).unwrap_or_else(|| body.trim().to_string());
                if detail.is_empty() {
                    format!("unexpected status: {s}")
                } else {
                    format!("unexpected status: {s}: {detail}")
                }
            };
            match s {
                200 => Ok(()),
                429 => {
                    let delay = match v["parameters"]["retry_after"].as_i64() {
                        Some(n) if n > 0 => Duration::from_secs(n as u64),
                        _ => retry_after_header(ra.as_deref(), self.short_retry_after, false),
                    };
                    if delay > MAX_RETRY_AFTER {
                        return Err(Step::Stop(format!("server demanded {} wait which exceeds the retry budget: {err}", go_duration(delay))));
                    }
                    Err(Step::Again(err, Some(delay)))
                }
                400 | 401 | 403 | 404 => Err(Step::Stop(format!("check bot token and chat id: {err}"))),
                _ => Err(Step::Again(err, None)),
            }
        })
        .await
    }

    /// A webhook with autobrr's event JSON (services/webhook).
    pub async fn webhook(&self, url: &str, method: &str, headers: &str, ev: &Value) -> Result<(), String> {
        let method = if method.is_empty() { "POST" } else { method };
        let m = Method::from_bytes(method.as_bytes()).map_err(|e| format!("could not create request: {e}"))?;
        let mut req = self.http.request(m, url);
        for (k, v) in parse_headers(headers) {
            req = req.header(k, v);
        }
        let event = ev["event"].as_str().unwrap_or_default().to_string();
        let res = req
            .header("Content-Type", "application/json")
            .header("User-Agent", &self.user_agent)
            .header("X-Autobrr-Event", event)
            .body(ev.to_string())
            .send()
            .await
            .map_err(request_error)?;
        let (status, _, body) = read(res).await;
        if !status.is_success() {
            return Err(format!("unexpected status: {} body: {body}", status.as_u16()));
        }
        Ok(())
    }

    /// A Bark server (services/bark).
    pub async fn bark(&self, host: &str, m: &Value) -> Result<(), String> {
        let endpoint = join_path(host, &["push"]).map_err(|e| format!("could not build url from host: '{host}': {e}"))?;
        let res = self
            .http
            .post(endpoint)
            .header("Content-Type", "application/json; charset=utf-8")
            .header("User-Agent", &self.user_agent)
            .body(m.to_string())
            .send()
            .await
            .map_err(request_error)?;
        let (status, _, body) = read(res).await;
        if status == StatusCode::OK {
            return Ok(());
        }
        let s = status.as_u16();
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        let err = match v["message"].as_str().filter(|m| !m.is_empty()) {
            Some(m) => format!("unexpected status: {s} error: {m}"),
            None if !body.trim().is_empty() => format!("unexpected status: {s} body: {}", body.trim()),
            None => format!("unexpected status: {s}"),
        };
        match s {
            400 => Err(format!("check bark device key: {err}")),
            418 => Err(format!("check bark server basic auth credentials: {err}")),
            _ => Err(err),
        }
    }

    /// Shoutrrr: the service named by the address's scheme. Lantern knows `generic`, `gotify`, `ntfy`,
    /// `telegram` and `pushover` addresses; any other answers shoutrrr's unknown service error.
    pub async fn shoutrrr(&self, address: &str, message: &str) -> Result<(), String> {
        let wrap = |e: String| format!("could not send message via shoutrrr: {e}");
        let u = url::Url::parse(address).map_err(|e| wrap(e.to_string()))?;
        let host = u.host_str().unwrap_or_default().to_string();
        let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
        let path = u.path().trim_matches('/').to_string();
        let query: Vec<(String, String)> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let q = |k: &str| query.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()).unwrap_or_default();
        match u.scheme() {
            "generic" | "generic+http" | "generic+https" => {
                let scheme = if u.scheme() == "generic+http" || q("disabletls") == "yes" { "http" } else { "https" };
                let target = format!("{scheme}://{host}{port}/{path}");
                let res = self
                    .http
                    .post(target)
                    .header("Content-Type", "application/json")
                    .body(message.to_string())
                    .send()
                    .await
                    .map_err(|e| wrap(e.to_string()))?;
                if !res.status().is_success() {
                    return Err(wrap(format!("server returned response status code {}", res.status())));
                }
                Ok(())
            }
            "gotify" => {
                let (base, token) = path.rsplit_once('/').map(|(a, b)| (format!("/{a}"), b.to_string())).unwrap_or((String::new(), path.clone()));
                let scheme = if q("disabletls") == "yes" { "http" } else { "https" };
                self.gotify(&format!("{scheme}://{host}{port}{base}"), &token, &q("title"), message).await.map_err(wrap)
            }
            "ntfy" => {
                let scheme = if q("scheme").is_empty() { "https".to_string() } else { q("scheme") };
                let n = Notification {
                    host: format!("{scheme}://{host}{port}/{path}"),
                    username: u.username().to_string(),
                    password: u.password().unwrap_or_default().to_string(),
                    ..Notification::default()
                };
                self.ntfy(&n, &q("title"), message).await.map_err(wrap)
            }
            "telegram" => {
                let token = format!("{}:{}", u.username(), u.password().unwrap_or_default());
                let mut last = Ok(());
                for chat in q("chats").split(',').filter(|c| !c.is_empty()) {
                    last = self.telegram("", &token, chat, 0, message).await;
                }
                last.map_err(wrap)
            }
            "pushover" => {
                let token = u.password().unwrap_or_default().to_string();
                self.pushover(&token, &host, &q("title"), message, q("priority").parse().unwrap_or(0), "", None).await.map_err(wrap)
            }
            other => Err(wrap(format!("unknown service \"{other}\""))),
        }
    }
}

/// A time-ordered random id (a version 7 UUID, as autobrr's `uuid.NewV7`).
pub fn uuid_v7() -> String {
    let ms = Utc::now().timestamp_millis() as u64;
    let r: u128 = rand::random();
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6..].copy_from_slice(&r.to_be_bytes()[..10]);
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Discord's error words for a status and body.
fn discord_status_error(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    let msg = v["message"].as_str().unwrap_or_default();
    let code = v["code"].as_i64().unwrap_or(0);
    if !msg.is_empty() && code == 0 {
        return format!("unexpected status: {status} error: {msg}");
    }
    if !msg.is_empty() {
        return format!("unexpected status: {status} error: {msg} code: {code}");
    }
    if !body.trim().is_empty() {
        return format!("unexpected status: {status} body: {}", body.trim());
    }
    format!("unexpected status: {status}")
}

/// Pushover's error words for a status and body.
fn pushover_status_error(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    let errs: Vec<String> = v["errors"].as_array().into_iter().flatten().filter_map(|e| e.as_str().map(str::to_string)).collect();
    if !errs.is_empty() {
        return format!("unexpected status: {status} api errors: {}", errs.join(", "));
    }
    if !body.is_empty() {
        return format!("unexpected status: {status} body: {body}");
    }
    format!("unexpected status: {status}")
}

/// A LunaSea address with an *arr module pointed at `/custom/` (autobrr's `rewriteWebhookURL`).
pub fn rewrite_lunasea_url(url: &str) -> String {
    let mut out = url.to_string();
    for m in ["radarr", "sonarr", "lidarr", "tautulli", "overseerr"] {
        out = out.replace(&format!("/{m}/"), "/custom/");
    }
    out
}

/// `KEY=value,KEY2=value2` as headers, skipping anything without `=` (autobrr's `parseHeaders`).
pub fn parse_headers(headers: &str) -> Vec<(String, String)> {
    headers
        .split(',')
        .filter_map(|h| h.trim().split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

/// A Discord embed field when `value` has text.
fn text_field(name: &str, value: &str, inline: bool) -> Option<Value> {
    (!value.is_empty()).then(|| field(name, &truncate_str(value, EMBED_FIELD_VALUE_LIMIT), inline))
}

/// A Discord field.
fn field(name: &str, value: &str, inline: bool) -> Value {
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("value".into(), json!(value));
    if inline {
        m.insert("inline".into(), json!(true));
    }
    Value::Object(m)
}

/// Discord's size field (binary units).
fn size_field(name: &str, value: u64, inline: bool) -> Option<Value> {
    (value > 0).then(|| field(name, &super::super::humanize::ibytes(value), inline))
}

/// Discord's code block field.
fn code_field(name: &str, value: &str, inline: bool) -> Option<Value> {
    (!value.is_empty()).then(|| field(name, &truncate_str(&format!("```\n{value}\n```"), EMBED_FIELD_VALUE_LIMIT - 12), inline))
}

/// One Discord embed being built.
struct Embed {
    /// The embed's keys.
    m: Map<String, Value>,
    /// Its fields.
    fields: Vec<Value>,
    /// Its time.
    timestamp: DateTime<Utc>,
}

impl Embed {
    /// A new embed, timed now.
    fn new() -> Embed {
        Embed { m: Map::new(), fields: Vec::new(), timestamp: Utc::now() }
    }
    fn set(&mut self, k: &str, v: Value) {
        self.m.insert(k.into(), v);
    }
    fn author(&mut self, name: &str, url: &str) {
        let mut a = Map::new();
        a.insert("name".into(), json!(name));
        if !url.is_empty() {
            a.insert("url".into(), json!(url));
        }
        self.set("author", Value::Object(a));
    }
    fn title(&mut self, t: &str) {
        if !t.is_empty() {
            self.set("title", json!(truncate_str(t, EMBED_TITLE_LIMIT)));
        }
    }
    fn description(&mut self, d: &str) {
        if !d.is_empty() {
            self.set("description", json!(truncate_str(d, EMBED_DESCRIPTION_LIMIT)));
        }
    }
    fn footer(&mut self, text: &str) {
        let mut f = Map::new();
        if !text.is_empty() {
            f.insert("text".into(), json!(text));
        }
        self.set("footer", Value::Object(f));
    }
    fn time(&mut self, t: Option<DateTime<Utc>>) {
        if let Some(t) = t {
            self.timestamp = t;
        }
    }
    fn fields(&mut self, list: Vec<Option<Value>>) {
        self.fields.extend(list.into_iter().flatten());
    }
    /// The embed as Discord takes it.
    fn done(mut self) -> Value {
        if !self.fields.is_empty() {
            self.m.insert("fields".into(), Value::Array(self.fields));
        }
        self.m.insert("timestamp".into(), json!(self.timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true)));
        Value::Object(self.m)
    }
}

/// Discord's colours.
const LIGHT_BLUE: i64 = 5814783;
/// Red.
const RED: i64 = 16711680;
/// Green.
const GREEN: i64 = 38912;
/// Yellow.
const YELLOW: i64 = 15844367;

/// The Discord message for `p` (autobrr's `buildDiscordMessage`).
pub fn discord_message(p: &Payload) -> Result<Value, String> {
    let mut e = Embed::new();
    let rel_title = p.release.as_ref().map(|r| r.title.clone()).unwrap_or_default();
    let push = |e: &mut Embed, author: &str, color: i64, reason: Option<&str>| {
        e.author(author, "");
        e.title(&rel_title);
        e.set("color", json!(color));
        e.footer(&p.action_client);
        e.time(p.timestamp);
        let mut f = vec![
            text_field("Release", &p.release_name, false),
            size_field("Size", p.size, true),
            text_field("Indexer", &p.indexer, true),
            text_field("Filter", &p.filter, true),
            text_field("Action", &p.action, true),
            text_field("Protocol", &p.protocol, true),
            text_field("Implementation", &p.implementation, true),
        ];
        if let Some(name) = reason {
            f.push(code_field(name, &p.rejections.join(", "), false));
        }
        e.fields(f);
    };
    let plain = |e: &mut Embed, t: &str, color: i64| {
        e.title(t);
        e.set("color", json!(color));
        e.description(&p.message);
        e.time(p.timestamp);
    };
    match p.event.as_str() {
        "TEST" => {
            e.author("autobrr: notification test", "");
            e.title("Test event!");
            e.set("color", json!(LIGHT_BLUE));
            e.fields(vec![text_field("Message", &p.message, true)]);
        }
        "APP_UPDATE_AVAILABLE" => {
            e.author("autobrr: New application update available", &p.url);
            e.set("color", json!(LIGHT_BLUE));
            e.fields(vec![
                text_field("Current version", &p.current_version, true),
                Some(field("New version", &format!("[{}]({})", p.new_version, p.url), true)),
                Some(field("Links", &format!("[Changelog]({})", p.url), true)),
            ]);
        }
        "PUSH_APPROVED" => push(&mut e, "autobrr: release approved", GREEN, None),
        "PUSH_REJECTED" => push(&mut e, "autobrr: release rejected", YELLOW, Some("Reason")),
        "PUSH_ERROR" => push(&mut e, "autobrr: release error", RED, Some("Error")),
        "RELEASE_NEW" => {
            e.author("autobrr: new release", "");
            e.title(&rel_title);
            e.set("color", json!(LIGHT_BLUE));
            e.time(p.timestamp);
            e.fields(vec![
                text_field("Release", &p.release_name, false),
                text_field("Indexer", &p.indexer, true),
                size_field("Size", p.size, true),
                text_field("Protocol", &p.protocol, true),
                text_field("Implementation", &p.implementation, true),
            ]);
        }
        "IRC_DISCONNECTED" => {
            e.title("IRC Disconnected unexpectedly");
            e.set("color", json!(RED));
            e.fields(vec![text_field("Network", &p.message, true)]);
        }
        "IRC_RECONNECTED" => {
            e.title("IRC Reconnected");
            e.set("color", json!(GREEN));
            e.fields(vec![text_field("Network", &p.message, true)]);
        }
        "IRC_UNHEALTHY" => plain(&mut e, "IRC Unhealthy", RED),
        "IRC_HEALTHY" => {
            e.title("IRC Healthy");
            e.set("color", json!(GREEN));
            e.fields(vec![text_field("Network", &p.message, true)]);
            e.time(p.timestamp);
        }
        "LIST_REFRESH_SUCCESS" => plain(&mut e, "List Refresh Success", GREEN),
        "LIST_REFRESH_ERROR" => plain(&mut e, "List Refresh Error", RED),
        "FEED_REFRESH_SUCCESS" => plain(&mut e, "Feed Refresh Success", GREEN),
        "FEED_REFRESH_ERROR" => plain(&mut e, "Feed Refresh Error", RED),
        other => return Err(format!("unknown event: {other}")),
    }
    Ok(json!({ "embeds": [e.done()] }))
}

/// Notifiarr's message for `p` (autobrr's `buildNotifiarrMessage`).
pub fn notifiarr_message(p: &Payload) -> Value {
    let mut d = Map::new();
    let both = !p.subject.is_empty() && !p.message.is_empty();
    d.insert("subject".into(), json!(if both { p.subject.as_str() } else { "" }));
    d.insert("message".into(), json!(if both { p.message.as_str() } else { "" }));
    d.insert("event".into(), json!(p.event));
    let mut opt = |k: &str, v: &str| {
        if !v.is_empty() {
            d.insert(k.into(), json!(v));
        }
    };
    opt("release_name", &p.release_name);
    opt("filter", &p.filter);
    opt("indexer", &p.indexer);
    opt("status", &p.status);
    if !p.action.is_empty() || !p.action_client.is_empty() {
        d.insert("action".into(), json!(p.action));
        if !p.action_client.is_empty() {
            d.insert("action_client".into(), json!(p.action_client));
        }
    }
    if !p.rejections.is_empty() {
        d.insert("rejections".into(), json!(p.rejections));
    }
    d.insert("timestamp".into(), json!(go_time(p.timestamp)));
    json!({ "event": p.event, "data": Value::Object(d) })
}

/// A time as Go's JSON writes it (the zero time when there is none).
fn go_time(t: Option<DateTime<Utc>>) -> String {
    t.map(|t| t.to_rfc3339_opts(SecondsFormat::AutoSi, true)).unwrap_or_else(|| "0001-01-01T00:00:00Z".into())
}

/// The webhook event name for a notification event (autobrr's `mapNotificationEventToWebhookEvent`).
pub fn webhook_event_type(event: &str) -> &'static str {
    match event {
        "RELEASE_NEW" => "release.new",
        "PUSH_APPROVED" => "action.approved",
        "PUSH_REJECTED" => "action.rejected",
        "PUSH_ERROR" => "action.error",
        "IRC_DISCONNECTED" => "irc.disconnected",
        "IRC_RECONNECTED" => "irc.reconnected",
        "IRC_UNHEALTHY" => "irc.unhealthy",
        "IRC_HEALTHY" => "irc.healthy",
        "LIST_REFRESH_SUCCESS" => "list.refresh_success",
        "LIST_REFRESH_ERROR" => "list.refresh_error",
        "FEED_REFRESH_SUCCESS" => "feed.refresh_success",
        "FEED_REFRESH_ERROR" => "feed.refresh_error",
        "APP_UPDATE_AVAILABLE" => "app.update_available",
        _ => "test",
    }
}

/// The webhook's JSON for `p` (autobrr's `NewWebhookEvent`).
pub fn webhook_event(p: &Payload, id: &str) -> Value {
    let mut data = Map::new();
    let put = |m: &mut Map<String, Value>, k: &str, v: Value| {
        let empty = match &v {
            Value::String(s) => s.is_empty(),
            Value::Number(n) => n.as_f64() == Some(0.0),
            Value::Bool(b) => !b,
            Value::Array(a) => a.is_empty(),
            Value::Null => true,
            _ => false,
        };
        if !empty {
            m.insert(k.into(), v);
        }
    };
    if let Some(r) = &p.release {
        let mut m = Map::new();
        put(&mut m, "protocol", json!(r.protocol));
        put(&mut m, "implementation", json!(r.implementation));
        m.insert("timestamp".into(), json!(go_time(p.timestamp)));
        m.insert("type".into(), json!(r.r#type.as_str()));
        m.insert("announce_type".into(), json!(r.announce_type));
        for (k, v) in [
            ("download_url", json!(r.download_url)),
            ("info_url", json!(r.info_url)),
            ("magnet_uri", json!(r.magnet_uri)),
            ("category", json!(r.category)),
            ("categories", json!(r.categories)),
            ("external_id", json!(r.torrent_id)),
            ("external_group_id", json!(r.group_id)),
            ("size", json!(r.size)),
        ] {
            put(&mut m, k, v);
        }
        m.insert("name".into(), json!(r.torrent_name));
        for (k, v) in [
            ("title", json!(r.title)),
            ("sub_title", json!(r.sub_title)),
            ("season", json!(r.season)),
            ("episode", json!(r.episode)),
            ("year", json!(r.year)),
            ("month", json!(r.month)),
            ("day", json!(r.day)),
            ("resolution", json!(r.resolution)),
            ("source", json!(r.source)),
            ("codec", json!(r.codec)),
            ("container", json!(r.container)),
            ("dynamic_range", json!(r.hdr)),
            ("audio", json!(r.audio)),
            ("audio_channels", json!(r.audio_channels)),
            ("audio_format", json!(r.audio_format)),
            ("bitrate", json!(r.bitrate)),
            ("media_processing", json!(r.media_processing)),
            ("group", json!(r.group)),
            ("website", json!(r.website)),
            ("origin", json!(r.origin)),
            ("uploader", json!(r.uploader)),
            ("pre_time", json!(r.pre_time)),
            ("edition", json!(r.edition)),
            ("cut", json!(r.cut)),
            ("language", json!(r.language)),
            ("region", json!(r.region)),
            ("tags", json!(r.tags)),
            ("proper", json!(r.proper)),
            ("repack", json!(r.repack)),
            ("hybrid", json!(r.hybrid)),
            ("artists", json!(r.artists)),
            ("record_label", json!(r.record_label)),
            ("has_cue", json!(r.has_cue)),
            ("has_log", json!(r.has_log)),
            ("log_score", json!(r.log_score)),
            ("freeleech", json!(r.freeleech)),
            ("freeleech_percent", json!(r.freeleech_percent)),
            ("seeders", json!(r.seeders)),
            ("leechers", json!(r.leechers)),
            ("meta_imdb", json!(r.meta_imdb)),
            ("meta_tmdb", json!(r.meta_tmdb)),
            ("meta_tvdb", json!(r.meta_tvdb)),
            ("meta_igdb", json!(r.meta_igdb)),
        ] {
            put(&mut m, k, v);
        }
        data.insert("release".into(), Value::Object(m));
    } else if !p.release_name.is_empty() {
        let mut m = Map::new();
        m.insert("timestamp".into(), json!("0001-01-01T00:00:00Z"));
        m.insert("type".into(), json!(""));
        m.insert("announce_type".into(), json!(""));
        put(&mut m, "size", json!(p.size));
        m.insert("name".into(), json!(p.release_name));
        data.insert("release".into(), Value::Object(m));
    }
    if !p.indexer.is_empty() {
        let mut m = Map::new();
        m.insert("name".into(), json!(p.indexer));
        if let Some(r) = &p.release {
            put(&mut m, "identifier", json!(r.indexer.identifier));
        }
        put(&mut m, "protocol", json!(p.protocol));
        data.insert("indexer".into(), Value::Object(m));
    }
    if !p.filter.is_empty() || p.filter_id > 0 {
        data.insert("filter".into(), json!({ "id": p.filter_id, "name": p.filter }));
    }
    if !p.action.is_empty() || !p.action_type.is_empty() {
        let mut m = Map::new();
        put(&mut m, "name", json!(p.action));
        put(&mut m, "type", json!(p.action_type));
        put(&mut m, "client", json!(p.action_client));
        data.insert("action".into(), Value::Object(m));
    }
    let ev = p.event.as_str();
    if matches!(ev, "PUSH_APPROVED" | "PUSH_REJECTED" | "PUSH_ERROR") {
        let mut m = Map::new();
        put(&mut m, "status", json!(p.status));
        put(&mut m, "rejections", json!(p.rejections));
        data.insert("result".into(), Value::Object(m));
    }
    if matches!(ev, "LIST_REFRESH_SUCCESS" | "LIST_REFRESH_ERROR") {
        let mut m = Map::new();
        m.insert("id".into(), json!(p.list_id));
        m.insert("name".into(), json!(p.list));
        m.insert("type".into(), json!(p.list_type));
        put(&mut m, "error", json!(p.list_error));
        data.insert("list".into(), Value::Object(m));
    }
    if matches!(ev, "FEED_REFRESH_SUCCESS" | "FEED_REFRESH_ERROR") {
        let mut m = Map::new();
        m.insert("id".into(), json!(p.feed_id));
        m.insert("name".into(), json!(p.feed));
        m.insert("type".into(), json!(p.feed_type));
        put(&mut m, "error", json!(p.feed_error));
        data.insert("feed".into(), Value::Object(m));
    }
    if matches!(ev, "IRC_DISCONNECTED" | "IRC_RECONNECTED" | "IRC_UNHEALTHY" | "IRC_HEALTHY") {
        let mut m = Map::new();
        m.insert("network".into(), json!(p.irc_network));
        put(&mut m, "message", json!(p.irc_message));
        data.insert("irc".into(), Value::Object(m));
    }
    json!({
        "event": webhook_event_type(ev),
        "id": id,
        "timestamp": go_time(p.timestamp),
        "version": "1.0",
        "data": Value::Object(data),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// autobrr's services/lunasea/lunasea_test.go (`Test_rewriteWebhookURL`).
    #[test]
    fn lunasea_urls() {
        for (i, o) in [
            ("https://notify.lunasea.app/v1/radarr/user/abc", "https://notify.lunasea.app/v1/custom/user/abc"),
            ("https://notify.lunasea.app/v1/sonarr/device/abc", "https://notify.lunasea.app/v1/custom/device/abc"),
            ("https://notify.lunasea.app/v1/custom/user/abc", "https://notify.lunasea.app/v1/custom/user/abc"),
        ] {
            assert_eq!(rewrite_lunasea_url(i), o);
        }
    }

    /// autobrr's services/webhook/webhook_test.go (`Test_parseHeaders`).
    #[test]
    fn headers() {
        assert!(parse_headers("").is_empty());
        assert_eq!(parse_headers("KEY=value"), vec![("KEY".to_string(), "value".to_string())]);
        assert_eq!(parse_headers(" A = 1 , B=2,broken"), vec![("A".into(), "1".into()), ("B".into(), "2".into())]);
    }

    /// autobrr's sender_discord_test.go (`TestDiscordBuildEmbed_RespectsEmbedLimits`).
    #[test]
    fn discord_embed_limits() {
        let p = Payload {
            event: "PUSH_ERROR".into(),
            release_name: "Very Long Release Name ".repeat(50),
            release: Some(Box::new(Release { title: "Very Long Release Name".into(), ..Release::default() })),
            filter: "f".repeat(2000),
            indexer: "i".repeat(2000),
            action: "a".repeat(2000),
            action_client: "c".repeat(2000),
            status: "PUSH_ERROR".into(),
            action_type: "RADARR".into(),
            rejections: vec![format!("radarr failed to push release: {}: i/o timeout", "x".repeat(4000))],
            ..Payload::default()
        };
        let m = discord_message(&p).unwrap();
        let e = &m["embeds"][0];
        assert!(e["title"].as_str().unwrap().chars().count() <= EMBED_TITLE_LIMIT);
        let mut reasons = String::new();
        for f in e["fields"].as_array().unwrap() {
            assert!(f["value"].as_str().unwrap().chars().count() <= EMBED_FIELD_VALUE_LIMIT, "{f}");
            if f["name"] == "Error" {
                reasons = f["value"].as_str().unwrap().to_string();
            }
        }
        assert!(reasons.starts_with("```\n"), "opening code fence was cut");
        assert!(reasons.ends_with("\n```"), "closing code fence was cut");
        assert!(reasons.contains("i/o timeout"));
    }

    /// autobrr's domain/notification_test.go (`TestNewWebhookEvent*`).
    #[test]
    fn webhook_events() {
        let p = Payload {
            event: "PUSH_APPROVED".into(),
            release_name: "Test.Release".into(),
            filter: "TV".into(),
            filter_id: 5,
            indexer: "Mock".into(),
            status: "PUSH_APPROVED".into(),
            action: "qBit".into(),
            action_type: "QBITTORRENT".into(),
            action_client: "qbt".into(),
            protocol: "torrent".into(),
            release: Some(Box::new(Release { torrent_name: "Test.Release".into(), size: 5, title: "Test".into(), ..Release::default() })),
            ..Payload::default()
        };
        let v = webhook_event(&p, "id-1");
        assert_eq!(v["event"], "action.approved");
        assert_eq!(v["version"], "1.0");
        assert_eq!(v["data"]["release"]["name"], "Test.Release");
        assert_eq!(v["data"]["filter"], json!({ "id": 5, "name": "TV" }));
        assert_eq!(v["data"]["action"], json!({ "name": "qBit", "type": "QBITTORRENT", "client": "qbt" }));
        assert_eq!(v["data"]["result"], json!({ "status": "PUSH_APPROVED" }));
        assert_eq!(v["data"]["indexer"], json!({ "name": "Mock", "protocol": "torrent" }));
        let irc = Payload { event: "IRC_DISCONNECTED".into(), irc_network: "Net".into(), ..Payload::default() };
        let v = webhook_event(&irc, "x");
        assert_eq!(v["event"], "irc.disconnected");
        assert_eq!(v["data"], json!({ "irc": { "network": "Net" } }));
        let nil = Payload { event: "RELEASE_NEW".into(), release_name: "Only.Name".into(), size: 10, ..Payload::default() };
        assert_eq!(webhook_event(&nil, "x")["data"]["release"]["name"], "Only.Name");
        let list = Payload { event: "LIST_REFRESH_ERROR".into(), list: "L".into(), list_id: 2, list_type: "SONARR".into(), list_error: "boom".into(), ..Payload::default() };
        assert_eq!(webhook_event(&list, "x")["data"]["list"], json!({ "id": 2, "name": "L", "type": "SONARR", "error": "boom" }));
        assert_eq!(webhook_event_type("NOPE"), "test");
    }

    #[test]
    fn truncation_and_durations() {
        assert_eq!(truncate_str("abcdefghijklmnopqrstuvwxyz", 13), "abc [...] xyz");
        assert_eq!(truncate_str("abc", 3), "abc");
        assert_eq!(truncate_str("abcdefghij", 5), "abcde");
        assert_eq!(go_duration(Duration::from_millis(120_500)), "2m0.5s");
        assert_eq!(go_duration(Duration::from_secs(30)), "30s");
        assert_eq!(go_duration(Duration::from_millis(300)), "300ms");
        assert_eq!(go_duration(Duration::from_secs(3600)), "1h0m0s");
    }
}
