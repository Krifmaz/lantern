//! Transmission through its RPC (`/transmission/rpc`): the session id handshake, add with a folder and Lantern's
//! labels, list, files, remove. Transmission has no categories; each app's folder is set on every torrent.

use super::{trim_url, unreachable, AddRequest, ClientConfig, Content, Item, ItemState};
use base64::Engine;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::sync::Mutex;

/// The fields Lantern asks for each torrent.
const FIELDS: [&str; 13] = [
    "hashString",
    "name",
    "status",
    "percentDone",
    "totalSize",
    "rateDownload",
    "eta",
    "downloadDir",
    "uploadRatio",
    "secondsSeeding",
    "error",
    "errorString",
    "labels",
];

/// One Transmission.
pub struct Transmission {
    /// The name people see.
    name: String,
    /// The RPC address (`…/transmission/rpc`).
    rpc: String,
    /// The user name (empty: no sign-in).
    username: String,
    /// The password.
    password: String,
    /// The session id Transmission handed out last (`X-Transmission-Session-Id`).
    session: Mutex<String>,
    /// Sends the requests.
    http: reqwest::Client,
}

impl Transmission {
    /// The client for `cfg`; an address without a path gets `/transmission/rpc`.
    pub fn new(cfg: &ClientConfig, http: reqwest::Client) -> Transmission {
        let url = trim_url(&cfg.url);
        let has_path = url::Url::parse(&url).map(|u| !u.path().trim_matches('/').is_empty()).unwrap_or(false);
        let rpc = if has_path { url } else { format!("{url}/transmission/rpc") };
        Transmission {
            name: cfg.name.clone(),
            rpc,
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            session: Mutex::new(String::new()),
            http,
        }
    }

    /// Calls `method` with `args`; the answer's `arguments`, or an error in words.
    async fn rpc(&self, method: &str, args: Value) -> Result<Value, String> {
        let body = json!({ "method": method, "arguments": args });
        for _ in 0..2 {
            let session = self.session.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut req = self.http.post(&self.rpc).header("x-transmission-session-id", session).json(&body);
            if !self.username.is_empty() {
                req = req.basic_auth(&self.username, Some(&self.password));
            }
            let r = req.send().await.map_err(|e| unreachable(&self.name, &self.rpc, &e))?;
            match r.status() {
                StatusCode::CONFLICT => {
                    let id = r.headers().get("x-transmission-session-id").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    *self.session.lock().unwrap_or_else(|e| e.into_inner()) = id;
                    continue;
                }
                StatusCode::UNAUTHORIZED => return Err(format!("{} refused the user name or password.", self.name)),
                s if !s.is_success() => return Err(format!("{} answered {} at {}. Check the address.", self.name, s.as_u16(), self.rpc)),
                _ => {}
            }
            let v: Value = r.json().await.map_err(|_| format!("{} sent an answer Lantern can't read.", self.name))?;
            let result = v.get("result").and_then(Value::as_str).unwrap_or("");
            if result != "success" {
                return Err(format!("{} couldn't do that: {result}.", self.name));
            }
            return Ok(v.get("arguments").cloned().unwrap_or(Value::Null));
        }
        Err(format!("{} kept asking for a new session. Check that the address is Transmission's.", self.name))
    }

    /// Signs in and asks the version.
    pub async fn test(&self) -> Result<String, String> {
        let v = self.rpc("session-get", json!({ "fields": ["version"] })).await?;
        Ok(format!("{} {}", self.name, v.get("version").and_then(Value::as_str).unwrap_or("")))
    }

    /// Adds the torrent into `save_path` with the labels `lantern` and the grab's tag; its info hash.
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        let mut args =
            json!({ "download-dir": req.save_path, "paused": false, "labels": [super::qbittorrent::TAG, req.tag, req.category] });
        match &req.content {
            Content::Magnet(m) => args["filename"] = json!(m),
            Content::Torrent(b) | Content::Nzb(b) => args["metainfo"] = json!(base64::engine::general_purpose::STANDARD.encode(b)),
        }
        let v = self.rpc("torrent-add", args).await?;
        let t = v.get("torrent-added").or_else(|| v.get("torrent-duplicate")).cloned().unwrap_or(Value::Null);
        Ok(t.get("hashString").and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// Every torrent (Lantern's are found by their hash or label).
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        let v = self.rpc("torrent-get", json!({ "fields": FIELDS })).await?;
        Ok(v.get("torrents").and_then(Value::as_array).map(|a| a.iter().map(item_of).collect()).unwrap_or_default())
    }

    /// The files of a torrent, as Transmission sees them.
    pub async fn files(&self, item: &Item) -> Result<Vec<(String, i64)>, String> {
        let v = self.rpc("torrent-get", json!({ "ids": [item.id], "fields": ["files", "downloadDir"] })).await?;
        let t = v.get("torrents").and_then(Value::as_array).and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
        let dir = t.get("downloadDir").and_then(Value::as_str).unwrap_or(&item.save_path).trim_end_matches('/').to_string();
        Ok(t.get("files")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        Some((format!("{dir}/{}", f.get("name")?.as_str()?), f.get("length").and_then(Value::as_i64).unwrap_or(0)))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Removes torrent `hash`, and its files when `delete_files`.
    pub async fn remove(&self, hash: &str, delete_files: bool) -> Result<(), String> {
        self.rpc("torrent-remove", json!({ "ids": [hash], "delete-local-data": delete_files })).await.map(|_| ())
    }
}

/// A torrent of `torrent-get` as an [`Item`].
fn item_of(t: &Value) -> Item {
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| t.get(k).and_then(Value::as_i64).unwrap_or(0);
    let done = t.get("percentDone").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
    let labels: Vec<String> = t
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    // error 3 is Transmission's own failure (disk, files); 1 and 2 are only tracker trouble
    let state = if n("error") == 3 {
        ItemState::Failed
    } else {
        match n("status") {
            0 if done >= 1.0 => ItemState::Completed,
            0 => ItemState::Paused,
            1..=3 => ItemState::Queued,
            5 | 6 => ItemState::Seeding,
            _ => ItemState::Downloading,
        }
    };
    let dir = s("downloadDir").trim_end_matches('/').to_string();
    let eta = n("eta");
    Item {
        id: s("hashString"),
        tag: labels.iter().find(|l| l.starts_with("lantern-")).cloned().unwrap_or_default(),
        name: s("name"),
        state,
        progress: done,
        size: n("totalSize"),
        speed: n("rateDownload"),
        eta: eta.max(0),
        path: format!("{dir}/{}", s("name")),
        save_path: dir,
        ratio: t.get("uploadRatio").and_then(Value::as_f64).unwrap_or(0.0).max(0.0),
        seed_secs: n("secondsSeeding"),
        error: if state == ItemState::Failed { format!("Transmission stopped the torrent: {}", s("errorString")) } else { String::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_torrent_states() {
        let i = item_of(
            &json!({"hashString": "h", "name": "X", "status": 6, "percentDone": 1.0, "labels": ["lantern", "lantern-3"], "downloadDir": "/d/tv/", "eta": -1}),
        );
        assert_eq!((i.state, i.tag.as_str(), i.path.as_str(), i.eta), (ItemState::Seeding, "lantern-3", "/d/tv/X", 0));
        assert_eq!(item_of(&json!({"status": 0, "percentDone": 1.0})).state, ItemState::Completed);
        assert_eq!(item_of(&json!({"status": 0, "percentDone": 0.5})).state, ItemState::Paused);
        assert_eq!(item_of(&json!({"status": 4, "error": 2})).state, ItemState::Downloading);
        assert_eq!(
            item_of(&json!({"status": 4, "error": 3, "errorString": "No space"})).error,
            "Transmission stopped the torrent: No space"
        );
    }
}
