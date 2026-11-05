//! Deluge through its Web UI's JSON-RPC (`/json`): sign in with the Web UI password, connect to the first daemon
//! when the Web UI isn't connected, labels (when the Label plugin is on), add into a folder, list, files, remove.

use super::{trim_url, unreachable, AddRequest, ClientConfig, Content, Item, ItemState};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

/// The fields Lantern asks for each torrent.
const FIELDS: [&str; 12] = [
    "name",
    "state",
    "progress",
    "total_size",
    "download_payload_rate",
    "eta",
    "save_path",
    "ratio",
    "seeding_time",
    "message",
    "label",
    "is_finished",
];

/// One Deluge.
pub struct Deluge {
    /// The name people see.
    name: String,
    /// The JSON-RPC address (`…/json`).
    rpc: String,
    /// The Web UI password.
    password: String,
    /// The next request id.
    next: AtomicU64,
    /// Keeps the session cookie.
    http: reqwest::Client,
}

impl Deluge {
    /// The client for `cfg`, sending through `http` (which keeps cookies).
    pub fn new(cfg: &ClientConfig, http: reqwest::Client) -> Deluge {
        let url = trim_url(&cfg.url);
        let rpc = if url.ends_with("/json") { url } else { format!("{url}/json") };
        Deluge { name: cfg.name.clone(), rpc, password: cfg.password.clone(), next: AtomicU64::new(1), http }
    }

    /// One call; its `result`, or the error Deluge gave.
    async fn raw(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let r = self
            .http
            .post(&self.rpc)
            .json(&json!({ "method": method, "params": params, "id": id }))
            .send()
            .await
            .map_err(|e| unreachable(&self.name, &self.rpc, &e))?;
        if !r.status().is_success() {
            return Err(format!("{} answered {} at {}. Check the address.", self.name, r.status().as_u16(), self.rpc));
        }
        let v: Value = r.json().await.map_err(|_| format!("{} sent an answer Lantern can't read.", self.name))?;
        match v.get("error") {
            Some(e) if !e.is_null() => {
                let code = e.get("code").and_then(Value::as_i64).unwrap_or(0);
                if code == 1 {
                    return Err("not signed in".into());
                }
                Err(format!("{} couldn't do that: {}", self.name, e.get("message").and_then(Value::as_str).unwrap_or("no reason given")))
            }
            _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        }
    }

    /// Signs in and makes sure the Web UI is connected to a daemon.
    async fn login(&self) -> Result<(), String> {
        let ok = self.raw("auth.login", json!([self.password])).await?;
        if ok != Value::Bool(true) {
            return Err(format!("{} refused the password. Use the Web UI's password.", self.name));
        }
        if self.raw("web.connected", json!([])).await? != Value::Bool(true) {
            let hosts = self.raw("web.get_hosts", json!([])).await?;
            let host = hosts.as_array().and_then(|h| h.first()).and_then(|h| h.get(0)).and_then(Value::as_str).map(str::to_string);
            let Some(host) = host else {
                return Err(format!("{}'s Web UI knows no Deluge daemon. Add one in its Connection Manager.", self.name));
            };
            self.raw("web.connect", json!([host])).await?;
        }
        Ok(())
    }

    /// A call, signing in first when the session has run out.
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        match self.raw(method, params.clone()).await {
            Err(e) if e == "not signed in" => {
                self.login().await?;
                self.raw(method, params).await
            }
            other => other,
        }
    }

    /// Signs in and asks the daemon's version.
    pub async fn test(&self) -> Result<String, String> {
        self.login().await?;
        let v = self.call("daemon.info", json!([])).await?;
        Ok(format!("{} {}", self.name, v.as_str().unwrap_or("")))
    }

    /// Makes label `name` when the Label plugin is on (without it, the folder alone keeps apps apart).
    pub async fn ensure_category(&self, name: &str, _save_path: &str) -> Result<(), String> {
        let labels = match self.call("label.get_labels", json!([])).await {
            Ok(l) => l,
            Err(_) => return Ok(()),
        };
        if !labels.as_array().is_some_and(|l| l.iter().any(|x| x.as_str() == Some(name))) {
            let _ = self.call("label.add", json!([name])).await;
        }
        Ok(())
    }

    /// Adds the torrent into `save_path` and labels it; its info hash.
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        let opts = json!({ "download_location": req.save_path, "add_paused": false });
        let hash = match &req.content {
            Content::Magnet(m) => self.call("core.add_torrent_magnet", json!([m, opts])).await?,
            Content::Torrent(b) | Content::Nzb(b) => {
                let file = format!("{}.torrent", req.tag);
                self.call("core.add_torrent_file", json!([file, base64::engine::general_purpose::STANDARD.encode(b), opts])).await?
            }
        };
        let hash =
            hash.as_str().map(str::to_string).ok_or_else(|| format!("{} refused the torrent. It may already be there.", self.name))?;
        let _ = self.call("label.set_torrent", json!([hash, req.category])).await;
        Ok(hash)
    }

    /// Every torrent (Lantern's are found by their hash).
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        let v = self.call("core.get_torrents_status", json!([{}, FIELDS])).await?;
        Ok(v.as_object().map(|m| m.iter().map(|(hash, t)| item_of(hash, t)).collect()).unwrap_or_default())
    }

    /// The files of a torrent, as Deluge sees them.
    pub async fn files(&self, item: &Item) -> Result<Vec<(String, i64)>, String> {
        let v = self.call("core.get_torrent_status", json!([item.id, ["files", "save_path"]])).await?;
        let dir = v.get("save_path").and_then(Value::as_str).unwrap_or(&item.save_path).trim_end_matches('/').to_string();
        Ok(v.get("files")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        Some((format!("{dir}/{}", f.get("path")?.as_str()?), f.get("size").and_then(Value::as_i64).unwrap_or(0)))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Removes torrent `hash`, and its files when `delete_files`.
    pub async fn remove(&self, hash: &str, delete_files: bool) -> Result<(), String> {
        self.call("core.remove_torrent", json!([hash, delete_files])).await.map(|_| ())
    }
}

/// A torrent of `core.get_torrents_status` as an [`Item`].
fn item_of(hash: &str, t: &Value) -> Item {
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| t.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let progress = (n("progress") / 100.0).clamp(0.0, 1.0);
    let state = match s("state").as_str() {
        "Error" => ItemState::Failed,
        "Seeding" => ItemState::Seeding,
        "Paused" if progress >= 1.0 || t.get("is_finished") == Some(&Value::Bool(true)) => ItemState::Completed,
        "Paused" => ItemState::Paused,
        "Queued" | "Checking" | "Allocating" => ItemState::Queued,
        _ => ItemState::Downloading,
    };
    let dir = s("save_path").trim_end_matches('/').to_string();
    Item {
        id: hash.to_string(),
        tag: String::new(),
        name: s("name"),
        state,
        progress,
        size: n("total_size") as i64,
        speed: n("download_payload_rate") as i64,
        eta: (n("eta") as i64).max(0),
        path: format!("{dir}/{}", s("name")),
        save_path: dir,
        ratio: n("ratio").max(0.0),
        seed_secs: n("seeding_time") as i64,
        error: if state == ItemState::Failed { format!("Deluge stopped the torrent: {}", s("message")) } else { String::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_torrent_states() {
        let i = item_of("h", &json!({"name": "X", "state": "Seeding", "progress": 100.0, "save_path": "/d/tv"}));
        assert_eq!((i.state, i.progress, i.path.as_str()), (ItemState::Seeding, 1.0, "/d/tv/X"));
        assert_eq!(item_of("h", &json!({"state": "Paused", "progress": 100.0})).state, ItemState::Completed);
        assert_eq!(item_of("h", &json!({"state": "Paused", "progress": 10.0})).state, ItemState::Paused);
        assert_eq!(item_of("h", &json!({"state": "Error", "message": "Disk full"})).error, "Deluge stopped the torrent: Disk full");
    }
}
