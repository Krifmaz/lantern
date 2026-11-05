//! SABnzbd through its API (`/api?mode=…&output=json&apikey=…`): categories with their folder, add an NZB, the
//! queue and the history, remove.

use super::{trim_url, unreachable, AddRequest, ClientConfig, Content, Item, ItemState};
use reqwest::multipart::{Form, Part};
use serde_json::Value;

/// One SABnzbd.
pub struct Sabnzbd {
    /// The name people see.
    name: String,
    /// Its address, without a slash at the end.
    url: String,
    /// Its API key (Config → General).
    api_key: String,
    /// Sends the requests.
    http: reqwest::Client,
}

impl Sabnzbd {
    /// The client for `cfg`, sending through `http`.
    pub fn new(cfg: &ClientConfig, http: reqwest::Client) -> Sabnzbd {
        Sabnzbd { name: cfg.name.clone(), url: trim_url(&cfg.url), api_key: cfg.api_key.clone(), http }
    }

    /// The API's address with `params`, the key and `output=json`.
    fn api(&self, params: &[(&str, &str)]) -> String {
        let mut u = url::Url::parse(&format!("{}/api", self.url))
            .unwrap_or_else(|_| url::Url::parse("http://invalid/api").expect("a valid address"));
        {
            let mut q = u.query_pairs_mut();
            for (k, v) in params {
                q.append_pair(k, v);
            }
            q.append_pair("output", "json").append_pair("apikey", &self.api_key);
        }
        u.to_string()
    }

    /// The JSON answer of `req`, or an error in words (SABnzbd says `{"status": false, "error": …}`).
    async fn json(&self, req: reqwest::RequestBuilder, doing: &str) -> Result<Value, String> {
        let r = req.send().await.map_err(|e| unreachable(&self.name, &self.url, &e))?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("{} couldn't {doing} (it answered {}).", self.name, status.as_u16()));
        }
        let v: Value = serde_json::from_str(&text).map_err(|_| {
            if text.to_lowercase().contains("api key") {
                format!("{} refused the API key. Copy it again from SABnzbd's Config → General.", self.name)
            } else {
                format!("{} sent an answer Lantern can't read while trying to {doing}.", self.name)
            }
        })?;
        if v.get("status") == Some(&Value::Bool(false)) {
            let why = v.get("error").and_then(Value::as_str).unwrap_or("no reason given");
            if why.to_lowercase().contains("api key") {
                return Err(format!("{} refused the API key. Copy it again from SABnzbd's Config → General.", self.name));
            }
            return Err(format!("{} couldn't {doing}: {why}.", self.name));
        }
        Ok(v)
    }

    /// Checks the key and asks the version.
    pub async fn test(&self) -> Result<String, String> {
        self.json(self.http.get(self.api(&[("mode", "queue"), ("limit", "1")])), "show its queue").await?;
        let v = self.json(self.http.get(self.api(&[("mode", "version")])), "say its version").await?;
        Ok(format!("{} {}", self.name, v.get("version").and_then(Value::as_str).unwrap_or("")))
    }

    /// Makes category `name` with the folder `save_path`, or points the existing one there.
    pub async fn ensure_category(&self, name: &str, save_path: &str) -> Result<(), String> {
        let u = self.api(&[("mode", "set_config"), ("section", "categories"), ("keyword", name), ("name", name), ("dir", save_path)]);
        self.json(self.http.get(u), &format!("make the category {name}")).await.map(|_| ())
    }

    /// Adds the NZB; its nzo id.
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        let Content::Nzb(bytes) = &req.content else {
            return Err(format!("{} downloads from Usenet only; a torrent can't go there.", self.name));
        };
        let file = Part::bytes(bytes.clone())
            .file_name(format!("{}.nzb", safe_name(&req.title)))
            .mime_str("application/x-nzb")
            .expect("a valid type");
        let form = Form::new().part("name", file);
        let u = self.api(&[("mode", "addfile"), ("cat", &req.category), ("nzbname", &req.title)]);
        let v = self.json(self.http.post(u).multipart(form), "add the NZB").await?;
        v.get("nzo_ids")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{} took the NZB but didn't say its id.", self.name))
    }

    /// The queue and the newest 200 of the history.
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        let q = self.json(self.http.get(self.api(&[("mode", "queue"), ("limit", "500")])), "show its queue").await?;
        let h = self.json(self.http.get(self.api(&[("mode", "history"), ("limit", "200")])), "show its history").await?;
        let mut out: Vec<Item> = q["queue"]["slots"].as_array().map(|a| a.iter().map(queue_item).collect()).unwrap_or_default();
        out.extend(h["history"]["slots"].as_array().map(|a| a.iter().map(history_item).collect::<Vec<_>>()).unwrap_or_default());
        Ok(out)
    }

    /// Removes `nzo_id` from the queue and the history, and its files when `delete_files`.
    pub async fn remove(&self, nzo_id: &str, delete_files: bool) -> Result<(), String> {
        let del = if delete_files { "1" } else { "0" };
        for mode in ["queue", "history"] {
            let u = self.api(&[("mode", mode), ("name", "delete"), ("value", nzo_id), ("del_files", del)]);
            self.json(self.http.get(u), "remove the download").await?;
        }
        Ok(())
    }
}

/// A file name without characters that are not safe in one.
fn safe_name(s: &str) -> String {
    let n: String = s.chars().filter(|c| c.is_alphanumeric() || " ._-()[]".contains(*c)).collect();
    if n.trim().is_empty() {
        "release".into()
    } else {
        n.trim().to_string()
    }
}

/// Text or number field `k` of `v` as a number.
fn num(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(n) => n.as_f64().unwrap_or(0.0),
        None => 0.0,
    }
}

/// A queue slot as an [`Item`].
fn queue_item(s: &Value) -> Item {
    let status = s["status"].as_str().unwrap_or("");
    let mb = num(s, "mb");
    let eta = s["timeleft"].as_str().unwrap_or("0:00:00").split(':').fold(0i64, |a, p| a * 60 + p.parse::<i64>().unwrap_or(0));
    Item {
        id: s["nzo_id"].as_str().unwrap_or("").to_string(),
        tag: String::new(),
        name: s["filename"].as_str().unwrap_or("").to_string(),
        state: match status {
            "Paused" => ItemState::Paused,
            "Queued" | "Grabbing" | "Fetching" | "Propagating" => ItemState::Queued,
            _ => ItemState::Downloading,
        },
        progress: (num(s, "percentage") / 100.0).clamp(0.0, 1.0),
        size: (mb * 1_048_576.0) as i64,
        speed: 0,
        eta,
        path: String::new(),
        save_path: String::new(),
        ratio: 0.0,
        seed_secs: 0,
        error: String::new(),
    }
}

/// A history slot as an [`Item`].
fn history_item(s: &Value) -> Item {
    let status = s["status"].as_str().unwrap_or("");
    let state = match status {
        "Completed" => ItemState::Completed,
        "Failed" => ItemState::Failed,
        _ => ItemState::Downloading,
    };
    let storage = s["storage"].as_str().unwrap_or("").to_string();
    Item {
        id: s["nzo_id"].as_str().unwrap_or("").to_string(),
        tag: String::new(),
        name: s["name"].as_str().unwrap_or("").to_string(),
        state,
        progress: if state == ItemState::Downloading { 0.99 } else { 1.0 },
        size: num(s, "bytes") as i64,
        speed: 0,
        eta: 0,
        save_path: storage.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default(),
        path: storage,
        ratio: 0.0,
        seed_secs: 0,
        error: if state == ItemState::Failed {
            let m = s["fail_message"].as_str().unwrap_or("").trim();
            if m.is_empty() {
                "SABnzbd couldn't finish the download".into()
            } else {
                format!("SABnzbd couldn't finish the download: {m}")
            }
        } else {
            String::new()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_queue_and_the_history() {
        let q = queue_item(
            &json!({"nzo_id": "n1", "filename": "Show", "status": "Downloading", "percentage": "45", "mb": "100", "timeleft": "0:01:05"}),
        );
        assert_eq!((q.state, q.eta, q.progress), (ItemState::Downloading, 65, 0.45));
        let h = history_item(&json!({"nzo_id": "n1", "status": "Failed", "fail_message": "Out of retention"}));
        assert_eq!(h.state, ItemState::Failed);
        assert!(h.error.ends_with("Out of retention"));
        let h = history_item(&json!({"nzo_id": "n2", "status": "Completed", "storage": "/d/tv/Show", "bytes": 5}));
        assert_eq!((h.state, h.save_path.as_str(), h.size), (ItemState::Completed, "/d/tv", 5));
    }
}
