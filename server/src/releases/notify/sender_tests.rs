// Ported from autobrr v1.90.0 (internal/notification/services/{bark,discord,gotify,lunasea,notifiarr,ntfy,pushover,
// telegram}/*_test.go, internal/notification/sender_webhook_test.go, sender_bark_test.go), GPL-2.0-or-later.
//! Every sender against a local stand-in of its service, with autobrr's own test cases: what each sends, which
//! answers it retries and how long it waits, and the words of its errors. Nothing leaves the machine.

use super::senders::Senders;
use super::{Notification, Payload};
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One request the stand-in got.
#[derive(Debug, Clone)]
struct Hit {
    /// The method.
    method: String,
    /// The path.
    path: String,
    /// The query, as sent.
    query: String,
    /// The headers.
    headers: HeaderMap,
    /// The body.
    body: String,
}

impl Hit {
    /// Header `k`, or empty.
    fn header(&self, k: &str) -> String {
        self.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
    }

    /// The body read as a form.
    fn form(&self, k: &str) -> Option<String> {
        url::form_urlencoded::parse(self.body.as_bytes()).find(|(a, _)| a == k).map(|(_, v)| v.into_owned())
    }

    /// The body read as JSON.
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_default()
    }
}

/// What the stand-in answers: a status, headers and a body.
type Reply = (u16, Vec<(&'static str, String)>, String);

/// The requests a stand-in got.
type Hits = Arc<Mutex<Vec<Hit>>>;

/// A local server answering each request with `answer(n, hit)` (`n` counts from 1); its address and what it got.
async fn stand_in(answer: impl Fn(usize, &Hit) -> Reply + Send + Sync + 'static) -> (String, Hits) {
    let hits: Hits = Arc::default();
    let h = hits.clone();
    let answer = Arc::new(answer);
    let app = axum::Router::new().fallback(move |req: Request| {
        let (h, answer) = (h.clone(), answer.clone());
        async move {
            let (parts, body) = req.into_parts();
            let body = String::from_utf8_lossy(&to_bytes(body, 1 << 20).await.unwrap_or_default()).into_owned();
            let hit = Hit {
                method: parts.method.to_string(),
                path: parts.uri.path().to_string(),
                query: parts.uri.query().unwrap_or_default().to_string(),
                headers: parts.headers,
                body,
            };
            let n = {
                let mut l = h.lock().unwrap();
                l.push(hit.clone());
                l.len()
            };
            let (status, headers, body) = answer(n, &hit);
            let mut r: Response = (StatusCode::from_u16(status).unwrap(), body).into_response();
            for (k, v) in headers {
                r.headers_mut().insert(k, v.parse().unwrap());
            }
            r
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), hits)
}

/// A plain answer.
fn ok(status: u16, body: &str) -> Reply {
    (status, Vec::new(), body.to_string())
}

/// The senders with autobrr's tests' short waits (`shortenRetryDelay`), pointed at `base` for the fixed services.
fn senders(base: &str) -> Senders {
    let ms = Duration::from_millis(10);
    Senders {
        notifiarr_endpoint: base.to_string(),
        pushover_endpoint: base.to_string(),
        pushover_sounds_endpoint: base.to_string(),
        telegram_host: base.to_string(),
        discord_retry: ms,
        telegram_retry: ms,
        notifiarr_retry: ms,
        ntfy_retry: ms,
        pushover_retry: ms,
        short_retry_after: ms,
        long_retry_after: ms,
        test_gap: Duration::ZERO,
        ..Senders::default()
    }
}

/// An HTTP date `secs` from now.
fn http_date(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// How many requests the stand-in got.
fn count(h: &Hits) -> usize {
    h.lock().unwrap().len()
}

/// The text autobrr's tests send.
const RELEASE: &str = "New release: Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP";

// ---- Discord ----

/// discord_test.go: `TestClient_SendMessage`.
#[tokio::test]
async fn discord_sends() {
    let (url, hits) = stand_in(|_, _| ok(204, "")).await;
    senders(&url).discord(&url, &json!({ "content": RELEASE })).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!(
        (h.method.as_str(), h.header("content-type").as_str(), h.header("user-agent").as_str()),
        ("POST", "application/json", "autobrr")
    );
    assert_eq!(h.json()["content"], RELEASE);
}

/// discord_test.go: `TestClient_SendMessage_RateLimitedThenSuccess`.
#[tokio::test]
async fn discord_rate_limited_then_success() {
    let (url, hits) = stand_in(|n, h| {
        assert!(!h.body.is_empty(), "retried request must not have an empty body");
        if n == 1 {
            ok(429, r#"{"message": "You are being rate limited.", "retry_after": 0.3, "global": false}"#)
        } else {
            ok(204, "")
        }
    })
    .await;
    let start = Instant::now();
    senders(&url).discord(&url, &json!({ "content": RELEASE })).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(270), "retry must honor the server-directed delay");
    assert_eq!(count(&hits), 2);
}

/// discord_test.go: `TestClient_SendMessage_RateLimitTooLong` and `RateLimitHTTPDateTooLong`.
#[tokio::test]
async fn discord_rate_limit_too_long() {
    let (url, hits) = stand_in(|_, _| ok(429, r#"{"message": "You are being rate limited.", "retry_after": 120.5, "global": true}"#)).await;
    let e = senders(&url).discord(&url, &json!({ "content": RELEASE })).await.unwrap_err();
    for want in ["unexpected status: 429", "You are being rate limited.", "exceeds the retry budget"] {
        assert!(e.contains(want), "{e}");
    }
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, _| (429, vec![("retry-after", http_date(60))], r#"{"message": "x", "global": true}"#.into())).await;
    let e = senders(&url).discord(&url, &json!({})).await.unwrap_err();
    assert!(e.contains("unexpected status: 429") && e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
}

/// discord_test.go: `TestClient_SendMessage_RateLimitHTTPDate`.
#[tokio::test]
async fn discord_rate_limit_http_date() {
    let (url, hits) = stand_in(|n, _| if n == 1 { (429, vec![("retry-after", http_date(2))], "{}".into()) } else { ok(204, "") }).await;
    let start = Instant::now();
    senders(&url).discord(&url, &json!({})).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900), "retry must honor an HTTP-date Retry-After");
    assert_eq!(count(&hits), 2);
}

/// discord_test.go: `TestClient_SendMessage_BadRequestNoRetry`, `NotFoundNoRetry` and `ServerErrorRetries`.
#[tokio::test]
async fn discord_errors() {
    let (url, hits) = stand_in(|_, _| ok(400, r#"{"message": "Cannot send an empty message", "code": 50006}"#)).await;
    let e = senders(&url).discord(&url, &json!({})).await.unwrap_err();
    assert!(e.contains("status: 400") && e.contains("Cannot send an empty message") && e.contains("50006"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, _| ok(404, r#"{"message": "Unknown Webhook", "code": 10015}"#)).await;
    let e = senders(&url).discord(&url, &json!({})).await.unwrap_err();
    assert!(e.contains("Unknown Webhook") && e.contains("check the webhook URL"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, _| ok(500, "upstream exploded")).await;
    let e = senders(&url).discord(&url, &json!({})).await.unwrap_err();
    assert!(e.contains("status: 500") && e.contains("upstream exploded"), "{e}");
    assert_eq!(count(&hits), 3);
}

/// sender_discord_test.go: `TestDiscordSender_SendThroughMock`: a payload goes out as one embed.
#[tokio::test]
async fn discord_sender_sends_an_embed() {
    let (url, hits) = stand_in(|_, _| ok(204, "")).await;
    let n = Notification { kind: "DISCORD".into(), webhook: url.clone(), enabled: true, ..Notification::default() };
    let p = Payload {
        event: "PUSH_APPROVED".into(),
        release_name: "Some.Release".into(),
        status: "PUSH_APPROVED".into(),
        ..Payload::default()
    };
    senders(&url).send(&n, &p).await.unwrap();
    let body = hits.lock().unwrap()[0].json();
    assert_eq!(body["embeds"].as_array().unwrap().len(), 1, "{body}");
}

// ---- Gotify ----

/// gotify_test.go: `TestClient_SendMessage`, `_Error` and `_ErrorRawBody`.
#[tokio::test]
async fn gotify() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let msg = format!("{RELEASE}\n");
    senders(&url).gotify(&url, "mock-token", "Push Approved", &msg).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.path.as_str()), ("POST", "/message"));
    assert_eq!(h.header("x-gotify-key"), "mock-token");
    assert!(!h.query.contains("token"));
    assert_eq!(h.header("content-type"), "application/x-www-form-urlencoded");
    assert_eq!(h.form("title").as_deref(), Some("Push Approved"));
    assert_eq!(h.form("message"), Some(msg));
    let (url, _) = stand_in(|_, _| {
        ok(401, r#"{"error":"Unauthorized","errorCode":401,"errorDescription":"you need to provide a valid access token or user credentials to access this api"}"#)
    })
    .await;
    let e = senders(&url).gotify(&url, "mock-token", "Test", "autobrr goes brr!!").await.unwrap_err();
    for want in ["check gotify application token", "unexpected status: 401", "Unauthorized", "you need to provide a valid access token"] {
        assert!(e.contains(want), "{e}");
    }
    let (url, _) = stand_in(|_, _| ok(502, "bad gateway")).await;
    let e = senders(&url).gotify(&url, "mock-token", "Test", "autobrr goes brr!!").await.unwrap_err();
    assert!(e.contains("unexpected status: 502") && e.contains("bad gateway"), "{e}");
}

// ---- LunaSea ----

/// lunasea_test.go: `TestClient_SendMessage` and `_NotFoundNoRetry`.
#[tokio::test]
async fn lunasea() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let m = json!({ "title": "Push Approved", "body": RELEASE, "image": "https://mock.local/logo.png" });
    senders(&url).lunasea(&url, &m).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.header("content-type").as_str()), ("POST", "application/json"));
    assert_eq!(h.json(), m);
    let (url, hits) = stand_in(|_, _| ok(404, r#"{"message":"Not Found"}"#)).await;
    let e = senders(&url).lunasea(&format!("{url}/v1/custom/user/bad-id"), &json!({})).await.unwrap_err();
    for want in ["unexpected status: 404", "Not Found", "device or user id is likely wrong"] {
        assert!(e.contains(want), "{e}");
    }
    assert_eq!(count(&hits), 1);
}

// ---- Notifiarr ----

/// notifiarr_test.go: `TestClient_SendMessage`.
#[tokio::test]
async fn notifiarr_sends() {
    let (url, hits) = stand_in(|_, _| ok(200, r#"{"result":"success"}"#)).await;
    let s = senders(&url);
    let m = json!({ "event": "PUSH_APPROVED", "data": { "subject": "Push Approved", "message": format!("{RELEASE}\n"), "event": "PUSH_APPROVED" } });
    s.notifiarr("mock-api-key", &m).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.header("content-type").as_str()), ("POST", "application/json"));
    assert_eq!(h.header("x-api-key"), "mock-api-key");
    assert_eq!(h.header("user-agent"), s.user_agent);
    assert_eq!(h.json(), m);
}

/// notifiarr_test.go: the rate limit cases.
#[tokio::test]
async fn notifiarr_rate_limits() {
    let limited = r#"{"result":"error","details":{"response":"rate limited"}}"#;
    let (url, hits) =
        stand_in(move |n, _| if n == 1 { (429, vec![("retry-after", "1".into())], limited.into()) } else { ok(200, "{}") }).await;
    let start = Instant::now();
    senders(&url).notifiarr("k", &json!({ "event": "TEST" })).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert_eq!(count(&hits), 2);
    let (url, hits) =
        stand_in(move |n, _| if n == 1 { (429, vec![("retry-after", "0".into())], limited.into()) } else { ok(200, "{}") }).await;
    senders(&url).notifiarr("k", &json!({})).await.unwrap();
    assert_eq!(count(&hits), 2, "fallback delay");
    let (url, hits) = stand_in(|_, _| (429, vec![("retry-after", "60".into())], String::new())).await;
    let e = senders(&url).notifiarr("k", &json!({})).await.unwrap_err();
    assert!(e.contains("unexpected status: 429") && e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) =
        stand_in(move |n, _| if n == 1 { (429, vec![("retry-after", http_date(2))], limited.into()) } else { ok(200, "{}") }).await;
    let start = Instant::now();
    senders(&url).notifiarr("k", &json!({})).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert_eq!(count(&hits), 2);
    let (url, hits) = stand_in(|_, _| (429, vec![("retry-after", http_date(60))], String::new())).await;
    let e = senders(&url).notifiarr("k", &json!({})).await.unwrap_err();
    assert!(e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
}

/// notifiarr_test.go: `UnauthorizedNoRetry`, `RequestTimeoutRetries`, `ServerErrorRetries`, `MissingAPIKey`.
#[tokio::test]
async fn notifiarr_errors() {
    let (url, hits) = stand_in(|_, _| ok(401, r#"{"result":"error","details":{"response":"invalid api key"}}"#)).await;
    let e = senders(&url).notifiarr("k", &json!({})).await.unwrap_err();
    for want in ["unexpected status: 401", "invalid api key", "check notifiarr api key"] {
        assert!(e.contains(want), "{e}");
    }
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|n, _| if n == 1 { ok(408, "") } else { ok(200, "{}") }).await;
    senders(&url).notifiarr("k", &json!({})).await.unwrap();
    assert_eq!(count(&hits), 2);
    let (url, hits) = stand_in(|_, _| ok(502, "")).await;
    let e = senders(&url).notifiarr("k", &json!({})).await.unwrap_err();
    assert!(e.contains("unexpected status: 502"), "{e}");
    assert_eq!(count(&hits), 3);
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let e = senders(&url).notifiarr("", &json!({})).await.unwrap_err();
    assert!(e.contains("missing notifiarr api key"), "{e}");
    assert_eq!(count(&hits), 0);
}

// ---- ntfy ----

/// An ntfy notification for `host`.
fn ntfy(host: &str) -> Notification {
    Notification { kind: "NTFY".into(), host: host.into(), ..Notification::default() }
}

/// ntfy_test.go: `TestClient_SendMessage` and `_BasicAuth`.
#[tokio::test]
async fn ntfy_sends() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let n = Notification { token: "mock-token".into(), priority: 3, topic: "tada".into(), ..ntfy(&format!("{url}/autobrr")) };
    let msg = format!("{RELEASE}\n");
    senders(&url).ntfy(&n, "Push Approved", &msg).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.path.as_str(), h.header("content-type").as_str()), ("POST", "/autobrr", "text/plain"));
    assert_eq!((h.header("title"), h.header("priority"), h.header("tags")), ("Push Approved".into(), "3".into(), "tada".into()));
    assert_eq!(h.header("authorization"), "Bearer mock-token");
    assert_eq!(h.body, msg);
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let n = Notification { token: "mock-token".into(), username: "mock-user".into(), password: "mock-pass".into(), ..ntfy(&url) };
    senders(&url).ntfy(&n, "Test", "autobrr goes brr!!").await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    use base64::Engine;
    let want = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("mock-user:mock-pass"));
    assert_eq!(h.header("authorization"), want);
    assert!(h.header("priority").is_empty());
}

/// ntfy_test.go: the rate limit cases, `ServerErrorRetries` and `UnauthorizedNoRetry`.
#[tokio::test]
async fn ntfy_retries() {
    let limited = r#"{"code":42901,"http":429,"error":"limit reached"}"#;
    let (url, hits) = stand_in(move |n, h| {
        assert_eq!(h.body, "autobrr goes brr!!");
        if n == 1 {
            (429, vec![("retry-after", "1".into())], limited.into())
        } else {
            ok(200, "")
        }
    })
    .await;
    let start = Instant::now();
    senders(&url).ntfy(&ntfy(&url), "Test", "autobrr goes brr!!").await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert_eq!(count(&hits), 2);
    let (url, hits) = stand_in(move |n, _| if n == 1 { ok(429, limited) } else { ok(200, "") }).await;
    senders(&url).ntfy(&ntfy(&url), "Test", "x").await.unwrap();
    assert_eq!(count(&hits), 2, "fallback delay");
    let (url, hits) = stand_in(move |_, _| (429, vec![("retry-after", "60".into())], limited.into())).await;
    let e = senders(&url).ntfy(&ntfy(&url), "Test", "x").await.unwrap_err();
    assert!(e.contains("unexpected status: 429") && e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) =
        stand_in(move |n, _| if n == 1 { (429, vec![("retry-after", http_date(2))], limited.into()) } else { ok(200, "") }).await;
    let start = Instant::now();
    senders(&url).ntfy(&ntfy(&url), "Test", "x").await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert_eq!(count(&hits), 2);
    let (url, hits) = stand_in(move |_, _| (429, vec![("retry-after", http_date(60))], limited.into())).await;
    let e = senders(&url).ntfy(&ntfy(&url), "Test", "x").await.unwrap_err();
    assert!(e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, _| ok(500, "")).await;
    let e = senders(&url).ntfy(&ntfy(&url), "Test", "x").await.unwrap_err();
    assert!(e.contains("unexpected status: 500"), "{e}");
    assert_eq!(count(&hits), 3);
    let (url, hits) = stand_in(|_, _| ok(401, r#"{"code":40101,"http":401,"error":"unauthorized"}"#)).await;
    let n = Notification { token: "bad-token".into(), ..ntfy(&url) };
    let e = senders(&url).ntfy(&n, "Test", "x").await.unwrap_err();
    for want in ["check access token or username/password", "unexpected status: 401", "unauthorized"] {
        assert!(e.contains(want), "{e}");
    }
    assert_eq!(count(&hits), 1);
}

// ---- Pushover ----

/// pushover_test.go: `TestClient_SendMessage` and `_ZeroTimestampNotSent`.
#[tokio::test]
async fn pushover_sends() {
    let (url, hits) = stand_in(|_, _| ok(200, r#"{"status":1,"request":"mock-request-id"}"#)).await;
    let sent = chrono::Utc::now();
    let msg = format!("{RELEASE}\n");
    senders(&url).pushover("mock-token", "mock-user", "Push Approved", &msg, 0, "", Some(sent)).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.header("content-type").as_str()), ("POST", "application/x-www-form-urlencoded"));
    assert_eq!(h.form("token").as_deref(), Some("mock-token"));
    assert_eq!(h.form("user").as_deref(), Some("mock-user"));
    assert_eq!(h.form("title").as_deref(), Some("Push Approved"));
    assert_eq!(h.form("message"), Some(msg));
    assert_eq!(h.form("timestamp"), Some(sent.timestamp().to_string()));
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    senders(&url).pushover("t", "u", "Test", "autobrr goes brr!!", 0, "", None).await.unwrap();
    assert!(hits.lock().unwrap()[0].form("timestamp").is_none());
}

/// pushover_test.go: `QuotaExhaustedNoRetry`, `BadRequestNoRetry` and `ServerErrorRetries`.
#[tokio::test]
async fn pushover_errors() {
    let (url, hits) = stand_in(|_, _| ok(429, r#"{"status":0,"request":"mock-request-id","errors":["message quota exceeded"]}"#)).await;
    let e = senders(&url).pushover("t", "u", "Test", "x", 0, "", None).await.unwrap_err();
    assert!(e.contains("monthly message quota exhausted") && e.contains("unexpected status: 429"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, _| {
        ok(400, r#"{"status":0,"request":"mock-request-id","user":"invalid","errors":["user identifier is not a valid user, group, or subscribed user key"]}"#)
    })
    .await;
    let e = senders(&url).pushover("t", "u", "Test", "x", 0, "", None).await.unwrap_err();
    assert!(
        e.contains("unexpected status: 400") && e.contains("user identifier is not a valid user, group, or subscribed user key"),
        "{e}"
    );
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, h| {
        assert_eq!(h.form("message").as_deref(), Some("autobrr goes brr!!"));
        ok(500, "")
    })
    .await;
    let e = senders(&url).pushover("t", "u", "Test", "autobrr goes brr!!", 0, "", None).await.unwrap_err();
    assert!(e.contains("unexpected status: 500"), "{e}");
    assert_eq!(count(&hits), 3);
}

/// http/notification.go `pushoverSounds` through services/pushover `GetSounds`.
#[tokio::test]
async fn pushover_sounds() {
    let (url, hits) = stand_in(|_, _| ok(200, r#"{"status":1,"sounds":{"pushover":"Pushover (default)","bike":"Bike"}}"#)).await;
    let v = senders(&url).pushover_sounds("app-token").await.unwrap();
    assert_eq!(v, json!({ "pushover": "Pushover (default)", "bike": "Bike" }));
    assert_eq!(hits.lock().unwrap()[0].query, "token=app-token");
    assert_eq!(senders(&url).pushover_sounds("").await.unwrap_err(), "api token is required");
}

// ---- Telegram ----

/// telegram_test.go: `TestClient_SendMessage`.
#[tokio::test]
async fn telegram_sends() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let text = "<b>New release:</b> Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP\n".to_string();
    senders(&url).telegram(&url, "mock-token", "-100123456789", 21, &text).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.path.as_str()), ("POST", "/botmock-token/sendMessage"));
    assert_eq!((h.header("content-type").as_str(), h.header("user-agent").as_str()), ("application/json", "autobrr"));
    assert_eq!(h.json(), json!({ "chat_id": "-100123456789", "text": text, "parse_mode": "HTML", "message_thread_id": 21 }));
}

/// telegram_test.go: the rate limit cases.
#[tokio::test]
async fn telegram_rate_limits() {
    let (url, hits) = stand_in(|n, _| {
        if n == 1 {
            ok(429, r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 1","parameters":{"retry_after":1}}"#)
        } else {
            ok(200, "")
        }
    })
    .await;
    let start = Instant::now();
    senders(&url).telegram(&url, "t", "c", 0, "autobrr goes brr!!").await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert_eq!(count(&hits), 2);
    assert_eq!(hits.lock().unwrap()[1].json()["text"], "autobrr goes brr!!");
    let (url, hits) = stand_in(|n, _| {
        if n == 1 {
            (
                429,
                vec![("retry-after", "0".into())],
                r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 0"}"#.into(),
            )
        } else {
            ok(200, "")
        }
    })
    .await;
    senders(&url).telegram(&url, "t", "c", 0, "x").await.unwrap();
    assert_eq!(count(&hits), 2, "fallback delay");
    let (url, hits) = stand_in(|_, _| {
        ok(429, r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 30","parameters":{"retry_after":30}}"#)
    })
    .await;
    let e = senders(&url).telegram(&url, "t", "c", 0, "x").await.unwrap_err();
    assert!(e.contains("unexpected status: 429") && e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|n, _| if n == 1 { (429, vec![("retry-after", http_date(2))], "{}".into()) } else { ok(200, "") }).await;
    let start = Instant::now();
    senders(&url).telegram(&url, "t", "c", 0, "x").await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert_eq!(count(&hits), 2);
    let (url, hits) = stand_in(|_, _| (429, vec![("retry-after", http_date(60))], "{}".into())).await;
    let e = senders(&url).telegram(&url, "t", "c", 0, "x").await.unwrap_err();
    assert!(e.contains("exceeds the retry budget"), "{e}");
    assert_eq!(count(&hits), 1);
}

/// telegram_test.go: `BadRequestNoRetry`, `ServerErrorRetries` and `MalformedHostDoesNotLeakToken`.
#[tokio::test]
async fn telegram_errors() {
    let (url, hits) = stand_in(|_, _| ok(400, r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#)).await;
    let e = senders(&url).telegram(&url, "t", "c", 0, "x").await.unwrap_err();
    assert!(e.contains("unexpected status: 400") && e.contains("Bad Request: chat not found"), "{e}");
    assert_eq!(count(&hits), 1);
    let (url, hits) = stand_in(|_, _| ok(500, "")).await;
    let e = senders(&url).telegram(&url, "t", "c", 0, "x").await.unwrap_err();
    assert!(e.contains("unexpected status: 500"), "{e}");
    assert_eq!(count(&hits), 3);
    let e = senders("http://127.0.0.1:9").telegram("https://example.com/%zz", "secret-token", "c", 0, "x").await.unwrap_err();
    assert!(!e.contains("secret-token"), "{e}");
}

// ---- Bark ----

/// bark_test.go: `TestClient_SendMessage` and `_SelfHosted`.
#[tokio::test]
async fn bark_sends() {
    let (url, hits) = stand_in(|_, _| ok(200, r#"{"code":200,"message":"success","timestamp":1700000000}"#)).await;
    let s = senders(&url);
    let m = json!({
        "device_key": "mock-key", "title": "Push Approved", "body": format!("{RELEASE}\n"), "sound": "minuet",
        "group": "autobrr", "level": "timeSensitive", "icon": "https://example.com/logo.png",
    });
    s.bark(&url, &m).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.path.as_str()), ("POST", "/push"));
    assert_eq!(h.header("content-type"), "application/json; charset=utf-8");
    assert_eq!(h.header("user-agent"), s.user_agent);
    assert_eq!(h.json(), m);
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let host = format!("http://user:pass@{}/bark/", url.trim_start_matches("http://"));
    senders(&url).bark(&host, &json!({ "device_key": "k" })).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!(h.path, "/bark/push");
    use base64::Engine;
    assert_eq!(h.header("authorization"), format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("user:pass")));
}

/// bark_test.go: `TestClient_SendMessage_Error`.
#[tokio::test]
async fn bark_errors() {
    let cases: [(u16, &str, &[&str]); 4] = [
        (
            400,
            r#"{"code":400,"message":"failed to get device token: failed to get [mock-key] device token from database","timestamp":1700000000}"#,
            &["check bark device key", "unexpected status: 400", "failed to get device token"],
        ),
        (418, "I'm a teapot", &["check bark server basic auth credentials", "unexpected status: 418", "I'm a teapot"]),
        (
            500,
            r#"{"code":500,"message":"push failed: BadDeviceToken","timestamp":1700000000}"#,
            &["unexpected status: 500", "push failed: BadDeviceToken"],
        ),
        (404, "", &["unexpected status: 404"]),
    ];
    for (status, body, wants) in cases {
        let (url, _) = stand_in(move |_, _| ok(status, body)).await;
        let e = senders(&url).bark(&url, &json!({})).await.unwrap_err();
        for want in wants {
            assert!(e.contains(want), "{status}: {e}");
        }
    }
}

/// sender_bark_test.go: `TestBarkSender_Send` and `_Level`: the payload's fields and the priority's level.
#[tokio::test]
async fn bark_sender_levels() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let n = Notification {
        name: "Bark".into(),
        kind: "BARK".into(),
        enabled: true,
        host: url.clone(),
        token: "mock-key".into(),
        sound: "minuet".into(),
        topic: "autobrr".into(),
        priority: 1,
        ..Notification::default()
    };
    assert!(n.is_enabled());
    let p = Payload {
        event: "APP_UPDATE_AVAILABLE".into(),
        subject: "New update available!".into(),
        message: "v1.2.0".into(),
        url: "https://github.com/autobrr/autobrr/releases/tag/v1.2.0".into(),
        ..Payload::default()
    };
    senders(&url).send(&n, &p).await.unwrap();
    assert_eq!(
        hits.lock().unwrap()[0].json(),
        json!({
            "device_key": "mock-key", "title": "Autobrr update available", "body": "New update available!\nv1.2.0\n",
            "sound": "minuet", "group": "autobrr", "level": "timeSensitive", "icon": super::senders::DEFAULT_IMAGE_URL,
            "url": "https://github.com/autobrr/autobrr/releases/tag/v1.2.0",
        })
    );
    // a priority left over from another service type falls back to Bark's default
    for (priority, level) in [(-1, Some("passive")), (0, None), (1, Some("timeSensitive")), (2, Some("critical")), (3, None)] {
        let (url, hits) = stand_in(|_, _| ok(200, "")).await;
        let n = Notification { kind: "BARK".into(), host: url.clone(), token: "mock-key".into(), priority, ..Notification::default() };
        senders(&url).send(&n, &Payload { event: "TEST".into(), ..Payload::default() }).await.unwrap();
        assert_eq!(hits.lock().unwrap()[0].json()["level"].as_str(), level, "{priority}");
    }
}

// ---- the webhook ----

/// sender_webhook_test.go: `TestGenericWebhookSender_Send`, `_Error`, `_CustomMethod` and `_CustomHeaders`.
#[tokio::test]
async fn webhook() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let n = Notification { kind: "WEBHOOK".into(), webhook: url.clone(), enabled: true, ..Notification::default() };
    let p = Payload {
        event: "PUSH_APPROVED".into(),
        release_name: "Some.Release".into(),
        status: "PUSH_APPROVED".into(),
        ..Payload::default()
    };
    let s = senders(&url);
    s.send(&n, &p).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.header("content-type").as_str()), ("POST", "application/json"));
    assert_eq!(h.header("x-autobrr-event"), "action.approved");
    assert_eq!(h.header("user-agent"), s.user_agent);
    let v = h.json();
    assert_eq!(v["event"], "action.approved");
    assert_eq!(v["data"]["release"]["name"], "Some.Release");
    assert!(!v["id"].as_str().unwrap_or_default().is_empty());
    let (url, _) = stand_in(|_, _| ok(500, "boom")).await;
    let n = Notification { webhook: url.clone(), ..n };
    let e = senders(&url).send(&n, &p).await.unwrap_err();
    assert!(e.contains("unexpected status: 500") && e.contains("boom"), "{e}");
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let n = Notification { webhook: url.clone(), method: "PUT".into(), headers: "Authorization=Bearer abc,X-Extra=1".into(), ..n };
    senders(&url).send(&n, &p).await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!(h.method, "PUT");
    assert_eq!((h.header("authorization"), h.header("x-extra")), ("Bearer abc".into(), "1".into()));
}

/// Shoutrrr's `generic` service posts the plain body to the address it names.
#[tokio::test]
async fn shoutrrr_generic() {
    let (url, hits) = stand_in(|_, _| ok(200, "")).await;
    let addr = format!("generic+{url}/hook");
    senders(&url).shoutrrr(&addr, "autobrr goes brr!!").await.unwrap();
    let h = hits.lock().unwrap()[0].clone();
    assert_eq!((h.method.as_str(), h.path.as_str(), h.body.as_str()), ("POST", "/hook", "autobrr goes brr!!"));
    let e = senders(&url).shoutrrr("nope://x", "y").await.unwrap_err();
    assert_eq!(e, "could not send message via shoutrrr: unknown service \"nope\"");
}
