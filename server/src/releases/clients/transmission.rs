// Ported from transmissionrpc v3.1.0 (github.com/hekmon/transmissionrpc, request.go, torrent_*.go, session_arguments.go),
// MIT, and autobrr v1.90.0 (pkg/transmission), GPL-2.0-or-later.
//! Transmission's RPC: every call carries the session id Transmission handed out (a 409 answer brings a new one,
//! and the call is tried once more) and a tag the answer must repeat.

use serde_json::{json, Map, Value};
use std::sync::Mutex;
use std::time::Duration;

/// The RPC version this client speaks (transmissionrpc's `RPCVersion`).
pub const RPC_VERSION: i64 = 17;

/// A Transmission client.
pub struct Client {
    endpoint: String,
    username: String,
    password: String,
    session: Mutex<String>,
    http: reqwest::Client,
}

/// A torrent Transmission added (`torrent-added` or `torrent-duplicate`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Added {
    /// Transmission's id for it.
    pub id: i64,
    /// Its info hash.
    pub hash_string: String,
}

impl Client {
    /// A client for the RPC at `endpoint` (ends in `/rpc`); basic auth when both are set.
    pub fn new(endpoint: &str, username: &str, password: &str, tls_skip_verify: bool) -> Client {
        Client {
            endpoint: endpoint.into(),
            username: username.into(),
            password: password.into(),
            session: Mutex::new(String::new()),
            http: super::http(Duration::from_secs(60), tls_skip_verify, false),
        }
    }

    /// Calls `method` with `arguments`; the answer's arguments.
    pub async fn call(&self, method: &str, arguments: Value) -> Result<Value, String> {
        let mut retry = true;
        loop {
            let tag: i64 = rand::random::<u16>() as i64;
            let mut body = json!({ "method": method, "tag": tag });
            if !arguments.is_null() {
                body["arguments"] = arguments.clone();
            }
            let sid = self.session.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut req = self
                .http
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .header("User-Agent", "autobrr")
                .header("X-Transmission-Session-Id", sid)
                .body(body.to_string());
            if !self.username.is_empty() && !self.password.is_empty() {
                req = req.basic_auth(&self.username, Some(&self.password));
            }
            let resp =
                req.send().await.map_err(|e| format!("failed to execute HTTP request: {}", super::req_err("Post", &self.endpoint, &e)))?;
            if resp.status().as_u16() == 409 {
                let id = resp.headers().get("X-Transmission-Session-Id").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                *self.session.lock().unwrap_or_else(|e| e.into_inner()) = id;
                if retry {
                    retry = false;
                    continue;
                }
                return Err("CSRF token invalid 2 times in a row: stopping to avoid infinite loop".into());
            }
            let status = resp.status();
            if status.as_u16() != 200 {
                let text = status.canonical_reason().map(|t| format!(": {t}")).unwrap_or_default();
                return Err(format!("HTTP error {}{text}", status.as_u16()));
            }
            let answer: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
                .map_err(|e| format!("can't unmarshal request answer body: {e}"))?;
            match answer.get("tag").and_then(Value::as_i64) {
                None => return Err("http answer does not have a tag within it's payload".into()),
                Some(t) if t != tag => return Err("http request tag and answer payload tag do not match".into()),
                _ => {}
            }
            let result = answer.get("result").and_then(Value::as_str).unwrap_or("");
            if result != "success" {
                return Err(format!("http request ok but payload does not indicate success: {result}"));
            }
            return Ok(answer.get("arguments").cloned().unwrap_or(Value::Null));
        }
    }

    /// Adds a torrent from `args` (`filename` or `metainfo`, `download-dir`, `paused`).
    pub async fn torrent_add(&self, args: Map<String, Value>) -> Result<Added, String> {
        if !args.contains_key("filename") && !args.contains_key("metainfo") {
            return Err("fields Filename and MetaInfo can't be both nil".into());
        }
        let v = self.call("torrent-add", Value::Object(args)).await.map_err(|e| format!("'torrent-add' rpc method failed: {e}"))?;
        let t = v.get("torrent-added").or_else(|| v.get("torrent-duplicate")).ok_or_else(|| {
            "RPC call went fine but neither 'torrent-added' nor 'torrent-duplicate' result payload were found".to_string()
        })?;
        Ok(Added {
            id: t.get("id").and_then(Value::as_i64).unwrap_or(0),
            hash_string: t.get("hashString").and_then(Value::as_str).unwrap_or("").to_string(),
        })
    }

    /// Changes torrents (`torrent-set`).
    pub async fn torrent_set(&self, args: Map<String, Value>) -> Result<(), String> {
        self.call("torrent-set", Value::Object(args)).await.map(|_| ()).map_err(|e| format!("'torrent-set' rpc method failed: {e}"))
    }

    /// The `fields` of the torrents `ids` (all when empty).
    pub async fn torrent_get(&self, fields: &[&str], ids: &[i64]) -> Result<Vec<Value>, String> {
        let mut args = json!({ "fields": fields });
        if !ids.is_empty() {
            args["ids"] = json!(ids);
        }
        let v = self.call("torrent-get", args).await.map_err(|e| format!("'torrent-get' rpc method failed: {e}"))?;
        Ok(v.get("torrents").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    /// Asks the trackers again (`torrent-reannounce`).
    pub async fn torrent_reannounce(&self, ids: &[i64]) -> Result<(), String> {
        self.call("torrent-reannounce", json!({ "ids": ids }))
            .await
            .map(|_| ())
            .map_err(|e| format!("'torrent-reannounce' rpc method failed: {e}"))
    }

    /// Removes torrents, keeping their files.
    pub async fn torrent_remove(&self, ids: &[i64]) -> Result<(), String> {
        self.call("torrent-remove", json!({ "ids": ids, "delete-local-data": false }))
            .await
            .map(|_| ())
            .map_err(|e| format!("'torrent-remove' rpc method failed: {e}"))
    }

    /// Whether Transmission speaks a version this client knows, and its version.
    pub async fn rpc_version(&self) -> Result<(bool, i64), String> {
        let v = self
            .call("session-get", Value::Null)
            .await
            .map_err(|e| format!("can't get session values: 'session-get' rpc method failed: {e}"))?;
        let ver = v.get("rpc-version").and_then(Value::as_i64).ok_or_else(|| "payload RPC Version is nil".to_string())?;
        let min = v.get("rpc-version-minimum").and_then(Value::as_i64).ok_or_else(|| "payload RPC Version minimum is nil".to_string())?;
        Ok((RPC_VERSION >= min, ver))
    }
}
