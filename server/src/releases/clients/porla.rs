// Ported from autobrr v1.90.0 (pkg/porla), GPL-2.0-or-later.
//! Porla over JSON-RPC at `/api/v1/jsonrpc`, with its token as `X-Porla-Token: Bearer <token>`.

use super::JsonRpc;
use serde_json::{json, Value};
use std::time::Duration;

/// A Porla client.
pub struct Client {
    rpc: JsonRpc,
}

/// What `torrents.add` takes (autobrr's `TorrentsAddReq`): only the fields that are set are sent.
#[derive(Debug, Clone, Default)]
pub struct TorrentsAddReq {
    /// Download limit in bytes per second.
    pub download_limit: Option<i64>,
    /// Where the files go.
    pub save_path: String,
    /// The torrent file, base64.
    pub ti: String,
    /// A magnet link.
    pub magnet_uri: String,
    /// Upload limit in bytes per second.
    pub upload_limit: Option<i64>,
    /// The preset to use.
    pub preset: Option<String>,
}

impl Client {
    /// A client for Porla at `hostname` with `token`; basic auth when both are set.
    pub fn new(hostname: &str, token: &str, tls_skip_verify: bool, basic_user: &str, basic_pass: &str) -> Client {
        let token = if token.starts_with("Bearer ") { token.to_string() } else { format!("Bearer {token}") };
        Client {
            rpc: JsonRpc {
                endpoint: format!("{hostname}/api/v1/jsonrpc"),
                headers: vec![("X-Porla-Token".into(), token), ("User-Agent".into(), super::user_agent())],
                basic_user: basic_user.into(),
                basic_pass: basic_pass.into(),
                http: super::http(Duration::from_secs(60), tls_skip_verify, false),
            },
        }
    }

    /// Porla's version and commit (`sys.versions`).
    pub async fn version(&self) -> Result<(String, String), String> {
        let v = self.rpc.call("sys.versions", None).await?;
        let p = v.get("porla").cloned().unwrap_or(Value::Null);
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok((s("version"), s("commitish")))
    }

    /// Adds a torrent.
    pub async fn torrents_add(&self, req: &TorrentsAddReq) -> Result<(), String> {
        let mut p = serde_json::Map::new();
        if let Some(l) = req.download_limit {
            p.insert("download_limit".into(), json!(l));
        }
        if !req.save_path.is_empty() {
            p.insert("save_path".into(), json!(req.save_path));
        }
        if !req.ti.is_empty() {
            p.insert("ti".into(), json!(req.ti));
        }
        if !req.magnet_uri.is_empty() {
            p.insert("magnet_uri".into(), json!(req.magnet_uri));
        }
        if let Some(l) = req.upload_limit {
            p.insert("upload_limit".into(), json!(l));
        }
        if let Some(pr) = &req.preset {
            p.insert("preset".into(), json!(pr));
        }
        self.rpc.call("torrents.add", Some(Value::Object(p))).await.map(|_| ())
    }

    /// How many torrents match `query` (`torrents.list`).
    pub async fn torrents_list(&self, query: &str) -> Result<usize, String> {
        let v = self.rpc.call("torrents.list", Some(json!({ "filters": { "query": query } }))).await?;
        Ok(v.get("torrents").and_then(Value::as_array).map(Vec::len).unwrap_or(0))
    }
}
