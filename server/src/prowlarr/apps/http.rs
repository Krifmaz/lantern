// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Common/Http/HttpClient.cs, HttpException.cs,
// TooManyRequestsException.cs, HttpRequestBuilder.cs), GPL-3.0.

//! The requests Lantern sends to an application, as Prowlarr's `HttpClient` sends them: JSON, the app's API key in
//! `X-Api-Key` (or the query, for Mylar and LazyLibrarian), basic auth when a user name or password is set,
//! redirects followed. An answer of 400 or more is an error with Prowlarr's message
//! (`HTTP request failed: [401:Unauthorized] [GET] at [http://sonarr:8989/api/v3/indexer]`).

use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

/// Why a request to an application failed.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpError {
    /// The app could not be reached (its name did not resolve or nothing answered): Prowlarr's connection failure,
    /// which does not make the back-off grow.
    Connect(String),
    /// The app answered with an error status (`HttpException`).
    Status {
        /// The status.
        code: u16,
        /// The request's method.
        method: String,
        /// The request's address.
        url: String,
        /// The answer's body.
        body: String,
        /// How long the app asked to wait, in seconds (`Retry-After` on a 429).
        retry_after: Option<f64>,
    },
    /// The answer was not the JSON expected.
    Json(String),
    /// Anything else (a time-out, a broken answer), or an app's own error.
    Other(String),
}

impl HttpError {
    /// The error's message, as Prowlarr writes it.
    pub fn message(&self) -> String {
        match self {
            HttpError::Connect(m) | HttpError::Json(m) | HttpError::Other(m) => m.clone(),
            HttpError::Status { code, method, url, .. } => {
                format!("HTTP request failed: [{code}:{}] [{method}] at [{url}]", status_name(*code))
            }
        }
    }

    /// The status, when the app answered with one.
    pub fn status(&self) -> Option<u16> {
        match self {
            HttpError::Status { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// .NET's name of an HTTP status (`HttpStatusCode`).
pub fn status_name(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "NoContent",
        301 => "MovedPermanently",
        302 => "Redirect",
        303 => "SeeOther",
        304 => "NotModified",
        307 => "TemporaryRedirect",
        308 => "PermanentRedirect",
        400 => "BadRequest",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        408 => "RequestTimeout",
        409 => "Conflict",
        410 => "Gone",
        415 => "UnsupportedMediaType",
        422 => "UnprocessableEntity",
        429 => "TooManyRequests",
        500 => "InternalServerError",
        501 => "NotImplemented",
        502 => "BadGateway",
        503 => "ServiceUnavailable",
        504 => "GatewayTimeout",
        _ => "Unknown",
    }
}

/// A request to an application.
#[derive(Debug, Clone)]
pub struct Request {
    /// GET, POST, PUT or DELETE.
    pub method: Method,
    /// The full address.
    pub url: String,
    /// The app's API key for `X-Api-Key` (`None` to leave it out).
    pub api_key: Option<String>,
    /// Basic auth: user name and password.
    pub auth: Option<(String, String)>,
    /// The JSON body.
    pub body: Option<String>,
}

/// What the app answered.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The status.
    pub status: u16,
    /// `X-Application-Version`, when the app sent it.
    pub version: Option<String>,
    /// The body.
    pub body: String,
}

impl Answer {
    /// The body as JSON.
    pub fn json(&self) -> Result<Value, HttpError> {
        serde_json::from_str(&self.body).map_err(|e| HttpError::Json(format!("Unexpected character encountered while parsing value: {e}")))
    }
}

/// The address `base` (its trailing `/` taken off) with `resource` after it and the query `params`, in order.
pub fn address(base: &str, resource: &str, params: &[(&str, String)]) -> String {
    let mut url = format!("{}{}", base.trim_end_matches('/'), resource);
    if !params.is_empty() {
        let q: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&q.join("&"));
    }
    url
}

/// .NET's `Uri.EscapeDataString`.
fn encode(s: &str) -> String {
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
    percent_encoding::utf8_percent_encode(s, KEEP).to_string()
}

/// The basic auth of an app's settings: set when the user name or the password is.
pub fn basic_auth(user: &str, password: &str) -> Option<(String, String)> {
    (!user.trim().is_empty() || !password.trim().is_empty()).then(|| (user.to_string(), password.to_string()))
}

/// The client every app request goes through.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(100))
                .connect_timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(5))
                .user_agent(format!("Lantern/{}", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default()
        })
        .clone()
}

/// Sends `req`; an answer of 400 or more is an error (Prowlarr's `HttpClient.Execute`).
pub async fn send(req: &Request) -> Result<Answer, HttpError> {
    let mut b = client()
        .request(req.method.clone(), &req.url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if let Some(k) = &req.api_key {
        b = b.header("X-Api-Key", k);
    }
    if let Some((u, p)) = &req.auth {
        b = b.basic_auth(u, Some(p));
    }
    if let Some(body) = &req.body {
        b = b.body(body.clone());
    }
    let r = b.send().await.map_err(|e| {
        if e.is_connect() {
            HttpError::Connect(format!("Connection refused or name not resolved ({})", req.url))
        } else if e.is_timeout() {
            HttpError::Other(format!("The operation has timed out ({})", req.url))
        } else {
            HttpError::Other(format!("Error: {e}"))
        }
    })?;
    let status = r.status().as_u16();
    let version = r.headers().get("x-application-version").and_then(|v| v.to_str().ok()).map(str::to_string);
    let retry_after = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok());
    let body = r.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(HttpError::Status { code: status, method: req.method.to_string(), url: req.url.clone(), body, retry_after });
    }
    Ok(Answer { status, version, body })
}

/// Sends `req` and reads the answer as JSON (Prowlarr's proxies' `Execute<T>`: 300 or more is an error too).
pub async fn json(req: &Request) -> Result<Value, HttpError> {
    let a = send(req).await?;
    if a.status >= 300 {
        return Err(HttpError::Status {
            code: a.status,
            method: req.method.to_string(),
            url: req.url.clone(),
            body: a.body,
            retry_after: None,
        });
    }
    a.json()
}

/// .NET's `Version` of a version text (`4.0.11.2680`): its parts, a missing part as -1; `None` when it is not one.
pub fn version(text: &str) -> Option<[i64; 4]> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.len() < 2 || parts.len() > 4 {
        return None;
    }
    let mut v = [-1i64; 4];
    for (i, p) in parts.iter().enumerate() {
        v[i] = p.parse::<i64>().ok().filter(|n| *n >= 0)?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_like_prowlarrs() {
        let e = HttpError::Status {
            code: 401,
            method: "GET".into(),
            url: "http://sonarr:8989/api/v3/indexer".into(),
            body: String::new(),
            retry_after: None,
        };
        assert_eq!(e.message(), "HTTP request failed: [401:Unauthorized] [GET] at [http://sonarr:8989/api/v3/indexer]");
    }

    #[test]
    fn addresses_and_versions() {
        assert_eq!(
            address("http://mylar:8090/", "/api", &[("cmd", "getVersion".into()), ("apikey", "a b".into())]),
            "http://mylar:8090/api?cmd=getVersion&apikey=a%20b"
        );
        assert_eq!(version("4.0.11.2680"), Some([4, 0, 11, 2680]));
        assert_eq!(version("3.0"), Some([3, 0, -1, -1]));
        assert!(version("3.0").unwrap() < [3, 0, 5, 0]);
        assert_eq!(version("abc"), None);
        assert_eq!(basic_auth("", " "), None);
    }
}
