// Ported from autobrr v1.90.0 (pkg/jsonrpc, pkg/sharedhttp, internal/downloader), GPL-2.0-or-later.
//! The programs autobrr's actions talk to, each as autobrr's own client library talks to it:
//!
//! | Module | Program | Protocol |
//! | --- | --- | --- |
//! | [`qbittorrent`] | qBittorrent | Web API v2 (go-qbittorrent) |
//! | [`deluge`] | Deluge 1 and 2 | the daemon's own RPC: rencode over TLS (go-deluge, [`rencode`]) |
//! | [`rtorrent`] | rTorrent | XML-RPC (go-rtorrent) |
//! | [`transmission`] | Transmission | RPC with a session id (transmissionrpc) |
//! | [`porla`] | Porla | JSON-RPC with a token |
//! | [`aria2`] | aria2 | JSON-RPC with a secret |
//! | [`arr`] | Sonarr, Radarr, Lidarr, Readarr, Whisparr, Sportarr | `release/push` |
//! | [`sabnzbd`] | SABnzbd | its HTTP API |
//! | [`nzbget`] | NZBGet | JSON-RPC |
//!
//! Errors are autobrr's own texts, so the release log reads like autobrr's.

pub mod aria2;
pub mod arr;
pub mod deluge;
pub mod nzbget;
pub mod porla;
pub mod qbittorrent;
pub mod rencode;
pub mod rtorrent;
pub mod sabnzbd;
pub mod transmission;

use serde_json::{json, Value};
use std::time::Duration;

/// The user agent autobrr sends (`meta.GetUserAgent`), with Lantern's name.
pub fn user_agent() -> String {
    format!("autobrr (Lantern {})", env!("CARGO_PKG_VERSION"))
}

/// An HTTP client with `timeout`, accepting any certificate when `insecure` (autobrr's `TransportTLSInsecure`),
/// keeping cookies when `cookies`.
pub fn http(timeout: Duration, insecure: bool, cookies: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(30))
        .danger_accept_invalid_certs(insecure)
        .cookie_store(cookies)
        .build()
        .unwrap_or_default()
}

/// A failed request in Go's words (`Post "<url>": <why>`), the address without its query.
pub fn req_err(method: &str, url: &str, e: &reqwest::Error) -> String {
    let why = if e.is_timeout() {
        "context deadline exceeded (Client.Timeout exceeded while awaiting headers)".to_string()
    } else if e.is_connect() {
        "connection refused".to_string()
    } else {
        let mut e = e.to_string();
        if let Some(i) = e.find(" for url (") {
            e.truncate(i);
        }
        e
    };
    let shown = url.split('?').next().unwrap_or(url);
    format!("{method} \"{shown}\": {why}")
}

/// A JSON-RPC 2.0 client (autobrr's `pkg/jsonrpc`): headers and basic auth on every call.
#[derive(Clone)]
pub struct JsonRpc {
    /// Where calls go.
    pub endpoint: String,
    /// Extra headers.
    pub headers: Vec<(String, String)>,
    /// Basic auth user (sent when both are set).
    pub basic_user: String,
    /// Basic auth password.
    pub basic_pass: String,
    /// The HTTP client.
    pub http: reqwest::Client,
}

/// A JSON-RPC answer's error (autobrr's `RPCError`): `code:message`.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    /// The code.
    pub code: i64,
    /// The message.
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.code, self.message)
    }
}

impl JsonRpc {
    /// Calls `method` with `params` (already in autobrr's `Params` shape: one object or list as it is, else a
    /// list); the result, or the call's error in words.
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let mut body = json!({ "jsonrpc": "2.0", "method": method, "id": 1 });
        if let Some(p) = params {
            body["params"] = p;
        }
        let mut req = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body.to_string());
        if !self.basic_user.is_empty() && !self.basic_pass.is_empty() {
            req = req.basic_auth(&self.basic_user, Some(&self.basic_pass));
        }
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| format!("error during rpc http request: {}", req_err("Post", &self.endpoint, &e)))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let v: Option<Value> = serde_json::from_str(&text).ok();
        let Some(v) = v.filter(Value::is_object) else {
            if status >= 400 {
                return Err(format!(
                    "rpc call {method}() on {} status code: {status}. Could not decode body to rpc response",
                    self.endpoint
                ));
            }
            return Err(format!("rpc call {method}() on {} status code: {status}. rpc response missing", self.endpoint));
        };
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            let err = RpcError {
                code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: e.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
            };
            return Err(err.to_string());
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}
