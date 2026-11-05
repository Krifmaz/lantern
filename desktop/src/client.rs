//! The HTTP client: the server's JSON API with this computer's app token (`Authorization: Bearer`), the connect
//! page's probe and the bug report uploads. Every call is an `async fn` that iced runs in a `Task` (on its tokio
//! executor). A failure comes back as an [`ApiError`] whose words are shown in full, with Copy.

use crate::{address, paths, signin};
use kit_rules::offline::reason::Failure;
use reqwest::multipart::{Form, Part};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// How long a call may take before it counts as no answer.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// How long the health check waits (kit-rules' offline `WAIT_MS`).
pub const HEALTH_WAIT: Duration = Duration::from_millis(kit_rules::offline::mode::WAIT_MS);

/// A call that failed: its HTTP status (0 when nothing answered), the words to show, and how it failed when
/// nothing answered (for the offline banner's reason).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    /// The HTTP status; 0 when no answer came.
    pub status: u16,
    /// The server's `detail`, or what went wrong, in full.
    pub message: String,
    /// How the call failed when it got no HTTP answer.
    pub failure: Option<Failure>,
}

impl ApiError {
    /// The app token is no longer good: sign in again.
    pub fn signed_out(&self) -> bool {
        self.status == 401
    }

    /// The server could not be reached (no answer, or a proxy saying it is down): offline.
    pub fn unreachable(&self) -> bool {
        kit_rules::offline::reason::unreachable(self.status)
    }

    /// The offline banner's reason for this failure (kit-rules' words).
    pub fn reason(&self, app: &str) -> String {
        let code = (self.status != 0).then_some(self.status);
        kit_rules::offline::reason::reason(app, true, code, self.failure.as_ref(), HEALTH_WAIT.as_secs())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The words for an answer that is not a success: the server's `detail` (or `error`), else "HTTP <status>".
pub fn error_words(status: u16, body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    ["detail", "error", "message"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .find(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("The server answered HTTP {status}"))
}

/// How a call that got no answer failed, from the error's kind and the words of its causes.
pub fn classify(timeout: bool, connect: bool, chain: &str) -> Failure {
    let c = chain.to_ascii_lowercase();
    if timeout {
        Failure::Timeout
    } else if c.contains("dns") || c.contains("resolve") || c.contains("lookup") || c.contains("name or service not known") {
        Failure::Dns
    } else if c.contains("certificate") || c.contains("tls") || c.contains("handshake") {
        Failure::Tls
    } else if connect || c.contains("refused") || c.contains("unreachable") {
        Failure::Refused
    } else {
        Failure::Other(chain.to_string())
    }
}

/// An [`ApiError`] for a call that got no answer, with the error and its causes in words.
fn no_answer(e: &reqwest::Error) -> ApiError {
    let mut chain = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        chain.push_str(": ");
        chain.push_str(&s.to_string());
        src = s.source();
    }
    ApiError { status: 0, failure: Some(classify(e.is_timeout(), e.is_connect(), &chain)), message: chain }
}

/// The MIME type of an attached file by its ending (the kinds a report takes).
pub fn mime_of(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("txt" | "log") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// The server at one address, with or without an app token. Cheap to clone (the connection pool is shared).
#[derive(Clone, Debug)]
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    /// A client for the server at `base` (`https://notes.example.com`), signed in with `token` ("" for none).
    pub fn new(base: &str, token: &str) -> Client {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(format!("{}/{}", crate::NAME, crate::VERSION))
            .build()
            .unwrap_or_default();
        Client { base: base.trim_end_matches('/').to_string(), token: token.to_string(), http }
    }

    /// The same server with another token.
    pub fn with_token(&self, token: &str) -> Client {
        Client { token: token.to_string(), ..self.clone() }
    }

    /// The server's base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Whether the client carries an app token.
    pub fn signed_in(&self) -> bool {
        !self.token.is_empty()
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(method, format!("{}{path}", self.base)).header("Accept", "application/json");
        if self.token.is_empty() {
            r
        } else {
            r.bearer_auth(&self.token)
        }
    }

    /// Sends a request and reads the JSON answer (`null` for an empty one).
    async fn send(&self, r: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let resp = r.send().await.map_err(|e| no_answer(&e))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| no_answer(&e))?;
        if !status.is_success() {
            return Err(ApiError { status: status.as_u16(), message: error_words(status.as_u16(), &text), failure: None });
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|_| ApiError {
            status: status.as_u16(),
            message: format!("The server's answer to this request isn't JSON (HTTP {})", status.as_u16()),
            failure: None,
        })
    }

    /// `GET path`.
    pub async fn get(&self, path: String) -> Result<Value, ApiError> {
        self.send(self.request(Method::GET, &path)).await
    }

    /// `POST path` with a JSON body.
    pub async fn post(&self, path: String, body: Value) -> Result<Value, ApiError> {
        self.send(self.request(Method::POST, &path).json(&body)).await
    }

    /// `PUT path` with a JSON body.
    pub async fn put(&self, path: String, body: Value) -> Result<Value, ApiError> {
        self.send(self.request(Method::PUT, &path).json(&body)).await
    }

    /// `GET /api/health` with the offline check's shorter wait.
    pub async fn health(&self) -> Result<Value, ApiError> {
        self.send(self.request(Method::GET, paths::HEALTH).timeout(HEALTH_WAIT)).await
    }

    /// `POST path` as a multipart form: the text `fields`, then each file as a `file_field` part. The files are
    /// read here (each is at most 8 MB: the bug report picker's limit).
    pub async fn multipart(
        &self,
        path: String,
        fields: Vec<(String, String)>,
        file_field: &'static str,
        files: Vec<PathBuf>,
    ) -> Result<Value, ApiError> {
        let mut form = Form::new();
        for (k, v) in fields {
            form = form.text(k, v);
        }
        for f in files {
            let bytes = tokio::fs::read(&f).await.map_err(|e| ApiError {
                status: 0,
                message: format!("{} couldn't be read: {e}", f.display()),
                failure: Some(Failure::Other(String::new())),
            })?;
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
            let part = Part::bytes(bytes).file_name(name).mime_str(mime_of(&f)).map_err(|e| ApiError {
                status: 0,
                message: e.to_string(),
                failure: None,
            })?;
            form = form.part(file_field, part);
        }
        self.send(self.request(Method::POST, &path).multipart(form)).await
    }

    /// Signs in for an app token (`POST /api/auth/login` with this computer); the answer read by
    /// [`signin::result`]. A server that can't be reached is an [`ApiError`].
    pub async fn login(&self, body: Value) -> Result<signin::LoginResult, ApiError> {
        let resp = self.request(Method::POST, paths::LOGIN).json(&body).send().await.map_err(|e| no_answer(&e))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| no_answer(&e))?;
        Ok(signin::result(status, &text))
    }
}

/// Finds the server for what was typed: tries each [`address::candidates`] address's `/api/health` and gives the
/// first that answers as a server, normalised. The error is the words to show.
pub async fn probe(input: String, app: &'static str) -> Result<String, String> {
    let candidates = address::candidates(&input);
    if candidates.is_empty() {
        return Err(address::EMPTY.into());
    }
    let mut last = address::UNREACHABLE.to_string();
    for c in candidates {
        let Some(base) = address::normalise(&c) else {
            last = address::INVALID.into();
            continue;
        };
        match Client::new(&base, "").health().await {
            Ok(v) => {
                let ok = v.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let version = v.get("version").and_then(Value::as_str).unwrap_or("");
                if address::is_server(ok, version) {
                    return Ok(base);
                }
                last = address::not_server(app);
            }
            Err(e) if e.status == StatusCode::NOT_FOUND.as_u16() || e.status == 200 => last = address::not_server(app),
            Err(e) => last = e.message,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_words_win() {
        assert_eq!(error_words(401, r#"{"detail":"Not authenticated"}"#), "Not authenticated");
        assert_eq!(error_words(500, r#"{"error":"Disk full"}"#), "Disk full");
        assert_eq!(error_words(502, "<html>Bad Gateway</html>"), "The server answered HTTP 502");
    }

    #[test]
    fn failures_are_told_apart() {
        assert_eq!(classify(true, true, "operation timed out"), Failure::Timeout);
        assert_eq!(classify(false, true, "error sending request: dns error: failed to lookup address"), Failure::Dns);
        assert_eq!(classify(false, true, "invalid peer certificate: UnknownIssuer"), Failure::Tls);
        assert_eq!(classify(false, true, "tcp connect error: Connection refused (os error 111)"), Failure::Refused);
        assert_eq!(classify(false, false, "body ended early"), Failure::Other("body ended early".into()));
    }

    #[test]
    fn an_error_knows_what_it_means() {
        let gone = ApiError { status: 0, message: "x".into(), failure: Some(Failure::Refused) };
        assert!(gone.unreachable() && !gone.signed_out());
        assert!(gone.reason("Notes").starts_with("Nothing answered at the server's address"));
        let proxy = ApiError { status: 502, message: "x".into(), failure: None };
        assert!(proxy.unreachable());
        assert!(proxy.reason("Notes").contains("can't reach Notes behind it"));
        assert!(ApiError { status: 401, message: "Not authenticated".into(), failure: None }.signed_out());
    }

    #[test]
    fn attachments_name_their_type() {
        assert_eq!(mime_of(std::path::Path::new("/a/Shot.PNG")), "image/png");
        assert_eq!(mime_of(std::path::Path::new("/a/app.log")), "text/plain");
        assert_eq!(mime_of(std::path::Path::new("/a/x.bin")), "application/octet-stream");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn nothing_to_probe_says_so() {
        assert_eq!(probe("  ".into(), "Notes").await, Err(address::EMPTY.into()));
    }
}
