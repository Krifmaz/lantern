//! NZBGet through its JSON-RPC (`/jsonrpc`, signed in with ControlUsername and ControlPassword): add an NZB with
//! the app's category, the queue and the history, remove. NZBGet's categories live in its own settings; a category
//! it doesn't know puts the files in `<DestDir>/<category>`, which is the app's folder when NZBGet's DestDir is
//! Lantern's downloads folder.

use super::{trim_url, unreachable, AddRequest, ClientConfig, Content, Item, ItemState};
use base64::Engine;
use serde_json::{json, Value};

/// One NZBGet.
pub struct Nzbget {
    /// The name people see.
    name: String,
    /// The JSON-RPC address (`…/jsonrpc`).
    rpc: String,
    /// ControlUsername.
    username: String,
    /// ControlPassword.
    password: String,
    /// Sends the requests.
    http: reqwest::Client,
}

impl Nzbget {
    /// The client for `cfg`; an address not ending in `/jsonrpc` gets it.
    pub fn new(cfg: &ClientConfig, http: reqwest::Client) -> Nzbget {
        let url = trim_url(&cfg.url);
        let rpc = if url.ends_with("/jsonrpc") { url } else { format!("{url}/jsonrpc") };
        Nzbget { name: cfg.name.clone(), rpc, username: cfg.username.clone(), password: cfg.password.clone(), http }
    }

    /// Calls `method` with `params`; its `result`, or an error in words.
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut req = self.http.post(&self.rpc).json(&json!({ "method": method, "params": params, "id": 1 }));
        if !self.username.is_empty() {
            req = req.basic_auth(&self.username, Some(&self.password));
        }
        let r = req.send().await.map_err(|e| unreachable(&self.name, &self.rpc, &e))?;
        if r.status().as_u16() == 401 {
            return Err(format!("{} refused the user name or password (ControlUsername and ControlPassword).", self.name));
        }
        if !r.status().is_success() {
            return Err(format!("{} answered {} at {}. Check the address.", self.name, r.status().as_u16(), self.rpc));
        }
        let v: Value = r.json().await.map_err(|_| format!("{} sent an answer Lantern can't read.", self.name))?;
        match v.get("error") {
            Some(e) if !e.is_null() => {
                Err(format!("{} couldn't do that: {}", self.name, e.get("message").and_then(Value::as_str).unwrap_or("no reason given")))
            }
            _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        }
    }

    /// Signs in and asks the version.
    pub async fn test(&self) -> Result<String, String> {
        let v = self.call("version", json!([])).await?;
        Ok(format!("{} {}", self.name, v.as_str().unwrap_or("")))
    }

    /// Adds the NZB in the app's category; its NZBID.
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        let Content::Nzb(bytes) = &req.content else {
            return Err(format!("{} downloads from Usenet only; a torrent can't go there.", self.name));
        };
        let params = json!([
            format!("{}.nzb", req.title),
            base64::engine::general_purpose::STANDARD.encode(bytes),
            req.category,
            0,
            false,
            false,
            "",
            0,
            "SCORE",
            [{ "Name": "*Lantern", "Value": req.tag }]
        ]);
        let id = self.call("append", params).await?.as_i64().unwrap_or(0);
        if id <= 0 {
            return Err(format!("{} refused the NZB. It may be broken.", self.name));
        }
        Ok(id.to_string())
    }

    /// The queue and the history.
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        let q = self.call("listgroups", json!([0])).await?;
        let h = self.call("history", json!([false])).await?;
        let mut out: Vec<Item> = q.as_array().map(|a| a.iter().map(queue_item).collect()).unwrap_or_default();
        out.extend(h.as_array().map(|a| a.iter().map(history_item).collect::<Vec<_>>()).unwrap_or_default());
        Ok(out)
    }

    /// Removes NZB `id` from the queue and the history, and its files when `delete_files`.
    pub async fn remove(&self, id: &str, delete_files: bool) -> Result<(), String> {
        let n: i64 = id.parse().unwrap_or(0);
        let (group, history) = if delete_files { ("GroupFinalDelete", "HistoryFinalDelete") } else { ("GroupDelete", "HistoryDelete") };
        // whichever list it is in takes it; the other answers false
        self.call("editqueue", json!([group, "", [n]])).await?;
        self.call("editqueue", json!([history, "", [n]])).await?;
        Ok(())
    }
}

/// Field `k` of `v` as a number.
fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A `listgroups` entry as an [`Item`].
fn queue_item(g: &Value) -> Item {
    let status = g.get("Status").and_then(Value::as_str).unwrap_or("");
    let (size, left) = (num(g, "FileSizeMB"), num(g, "RemainingSizeMB"));
    let state = match status {
        "PAUSED" => ItemState::Paused,
        "QUEUED" | "FETCHING" => ItemState::Queued,
        _ => ItemState::Downloading,
    };
    let dir = g.get("DestDir").and_then(Value::as_str).unwrap_or("").to_string();
    Item {
        id: g.get("NZBID").and_then(Value::as_i64).unwrap_or(0).to_string(),
        tag: String::new(),
        name: g.get("NZBName").and_then(Value::as_str).unwrap_or("").to_string(),
        state,
        progress: if size > 0.0 { ((size - left) / size).clamp(0.0, 1.0) } else { 0.0 },
        size: (size * 1_048_576.0) as i64,
        speed: 0,
        eta: 0,
        path: dir.clone(),
        save_path: dir,
        ratio: 0.0,
        seed_secs: 0,
        error: String::new(),
    }
}

/// A `history` entry as an [`Item`].
fn history_item(h: &Value) -> Item {
    let status = h.get("Status").and_then(Value::as_str).unwrap_or("");
    let state = if status.starts_with("SUCCESS") || status.starts_with("WARNING") { ItemState::Completed } else { ItemState::Failed };
    let final_dir = h.get("FinalDir").and_then(Value::as_str).unwrap_or("");
    let dir = if final_dir.is_empty() { h.get("DestDir").and_then(Value::as_str).unwrap_or("") } else { final_dir }.to_string();
    Item {
        id: h.get("NZBID").and_then(Value::as_i64).unwrap_or(0).to_string(),
        tag: String::new(),
        name: h.get("Name").and_then(Value::as_str).unwrap_or("").to_string(),
        state,
        progress: 1.0,
        size: (num(h, "FileSizeMB") * 1_048_576.0) as i64,
        speed: 0,
        eta: 0,
        save_path: dir.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default(),
        path: dir,
        ratio: 0.0,
        seed_secs: 0,
        error: if state == ItemState::Failed { format!("NZBGet couldn't finish the download ({status})") } else { String::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_queue_and_the_history() {
        let q = queue_item(
            &json!({"NZBID": 7, "NZBName": "A", "Status": "DOWNLOADING", "FileSizeMB": 100, "RemainingSizeMB": 25, "DestDir": "/d/books/A.#7"}),
        );
        assert_eq!((q.id.as_str(), q.state, q.progress), ("7", ItemState::Downloading, 0.75));
        let h = history_item(&json!({"NZBID": 7, "Name": "A", "Status": "SUCCESS/ALL", "DestDir": "/d/books/A"}));
        assert_eq!((h.state, h.path.as_str(), h.save_path.as_str()), (ItemState::Completed, "/d/books/A", "/d/books"));
        assert_eq!(history_item(&json!({"Status": "FAILURE/PAR"})).error, "NZBGet couldn't finish the download (FAILURE/PAR)");
    }
}
