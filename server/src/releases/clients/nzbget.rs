// Ported from autobrr v1.90.0 (pkg/nzbget), GPL-2.0-or-later.
//! NZBGet over its JSON-RPC at `/jsonrpc`, with basic auth.

use serde_json::{json, Value};
use std::time::Duration;

/// An NZBGet client.
pub struct Client {
    host: String,
    username: String,
    password: String,
    http: reqwest::Client,
}

impl Client {
    /// A client for NZBGet at `host`.
    pub fn new(host: &str, username: &str, password: &str, tls_skip_verify: bool) -> Client {
        Client {
            host: host.into(),
            username: username.into(),
            password: password.into(),
            http: super::http(Duration::from_secs(60), tls_skip_verify, false),
        }
    }

    /// Calls `method`; its result.
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let endpoint = super::super::downloader::join_path_url(&self.host, "/jsonrpc");
        let body = json!({ "method": method, "params": params, "id": 1 });
        let mut req = self
            .http
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .header("User-Agent", super::user_agent())
            .body(body.to_string());
        if !self.username.is_empty() || !self.password.is_empty() {
            req = req.basic_auth(&self.username, Some(&self.password));
        }
        let res = req.send().await.map_err(|e| format!("could not make request to nzbget: {}", super::req_err("Post", &endpoint, &e)))?;
        if res.status().as_u16() != 200 {
            return Err(format!("nzbget returned status {}", res.status().as_u16()));
        }
        let v: Value =
            serde_json::from_str(&res.text().await.unwrap_or_default()).map_err(|e| format!("could not decode rpc response: {e}"))?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(format!(
                "nzbget rpc error {}: {}",
                e.get("code").and_then(Value::as_i64).unwrap_or(0),
                e.get("message").and_then(Value::as_str).unwrap_or("")
            ));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    /// NZBGet's version.
    pub async fn version(&self) -> Result<String, String> {
        let v = self.call("version", Value::Null).await?;
        v.as_str().map(str::to_string).ok_or_else(|| "could not unmarshal rpc result".to_string())
    }

    /// Adds the NZB at `url` in `category`; its id.
    pub async fn add_from_url(&self, url: &str, category: &str) -> Result<i64, String> {
        let params = json!(["", url, category, 0, false, false, "", 0, "SCORE", []]);
        let id = self
            .call("append", params)
            .await
            .map_err(|e| format!("could not add nzb to nzbget: {e}"))?
            .as_i64()
            .ok_or_else(|| "could not add nzb to nzbget: could not unmarshal rpc result".to_string())?;
        if id <= 0 {
            return Err(format!("nzbget returned invalid nzb id: {id}"));
        }
        Ok(id)
    }
}
