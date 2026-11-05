//! qBittorrent through its Web API (v2): sign in, categories, add with a category, a folder and Lantern's tags,
//! list, files, remove. Lantern marks every torrent it adds with the tags `lantern` and `lantern-<grab id>`.

use super::{trim_url, unreachable, AddRequest, ClientConfig, Content, Item, ItemState};
use reqwest::multipart::{Form, Part};
use reqwest::StatusCode;
use serde_json::Value;

/// The tag on every torrent Lantern adds.
pub const TAG: &str = "lantern";

/// One qBittorrent.
pub struct Qbittorrent {
    /// The name people see.
    name: String,
    /// Its address, without a slash at the end.
    url: String,
    /// The user name (empty: qBittorrent lets this address in without one).
    username: String,
    /// The password.
    password: String,
    /// Keeps the session cookie.
    http: reqwest::Client,
}

impl Qbittorrent {
    /// The client for `cfg`, sending through `http` (which keeps cookies).
    pub fn new(cfg: &ClientConfig, http: reqwest::Client) -> Qbittorrent {
        Qbittorrent {
            name: cfg.name.clone(),
            url: trim_url(&cfg.url),
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            http,
        }
    }

    /// Signs in; the session cookie is kept for the next requests.
    async fn login(&self) -> Result<(), String> {
        if self.username.is_empty() {
            return Ok(());
        }
        let r = self
            .http
            .post(format!("{}/api/v2/auth/login", self.url))
            .form(&[("username", self.username.as_str()), ("password", self.password.as_str())])
            .send()
            .await
            .map_err(|e| unreachable(&self.name, &self.url, &e))?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if status == StatusCode::FORBIDDEN {
            return Err(format!("{} has blocked Lantern's address after too many wrong passwords. Wait, or restart it.", self.name));
        }
        if !status.is_success() || text.trim() != "Ok." {
            return Err(format!("{} refused the user name or password. Check them in its Web UI settings.", self.name));
        }
        Ok(())
    }

    /// Sends a request made by `make`, signing in again once when the session has run out.
    async fn send(&self, make: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
        let r = make().send().await.map_err(|e| unreachable(&self.name, &self.url, &e))?;
        if r.status() != StatusCode::FORBIDDEN {
            return Ok(r);
        }
        self.login().await?;
        let r = make().send().await.map_err(|e| unreachable(&self.name, &self.url, &e))?;
        if r.status() == StatusCode::FORBIDDEN {
            return Err(format!("{} doesn't let Lantern in. Check the user name and password.", self.name));
        }
        Ok(r)
    }

    /// The answer's text, or an error in words when the status is not a success.
    async fn text(&self, r: reqwest::Response, doing: &str) -> Result<String, String> {
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if status.is_success() {
            Ok(text)
        } else {
            Err(format!(
                "{} couldn't {doing} (it answered {}{}).",
                self.name,
                status.as_u16(),
                if text.is_empty() { String::new() } else { format!(": {}", text.trim()) }
            ))
        }
    }

    /// Signs in and asks the version.
    pub async fn test(&self) -> Result<String, String> {
        self.login().await?;
        let r = self.send(|| self.http.get(format!("{}/api/v2/app/version", self.url))).await?;
        let v = self.text(r, "say its version").await?;
        Ok(format!("{} {}", self.name, v.trim()))
    }

    /// Makes category `name` saving into `save_path`, or points the existing one there.
    pub async fn ensure_category(&self, name: &str, save_path: &str) -> Result<(), String> {
        let form = [("category", name), ("savePath", save_path)];
        let r = self.send(|| self.http.post(format!("{}/api/v2/torrents/createCategory", self.url)).form(&form)).await?;
        if r.status() == StatusCode::CONFLICT {
            let r = self.send(|| self.http.post(format!("{}/api/v2/torrents/editCategory", self.url)).form(&form)).await?;
            self.text(r, &format!("change the category {name}")).await?;
            return Ok(());
        }
        self.text(r, &format!("make the category {name}")).await.map(|_| ())
    }

    /// Adds the torrent; "" (it is found again by its tag).
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        let form = || {
            let f = Form::new()
                .text("category", req.category.clone())
                .text("savepath", req.save_path.clone())
                .text("tags", format!("{TAG},{}", req.tag))
                .text("autoTMM", "false")
                .text("paused", "false")
                .text("stopped", "false");
            match &req.content {
                Content::Magnet(m) => f.text("urls", m.clone()),
                Content::Torrent(b) | Content::Nzb(b) => f.part(
                    "torrents",
                    Part::bytes(b.clone())
                        .file_name(format!("{}.torrent", req.tag))
                        .mime_str("application/x-bittorrent")
                        .expect("a valid type"),
                ),
            }
        };
        let r = self.send(|| self.http.post(format!("{}/api/v2/torrents/add", self.url)).multipart(form())).await?;
        let text = self.text(r, "add the torrent").await?;
        if text.trim() == "Fails." {
            return Err(format!("{} refused the torrent. It may be broken, or already in {}.", self.name, self.name));
        }
        Ok(String::new())
    }

    /// The torrents tagged `lantern`.
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        let r = self.send(|| self.http.get(format!("{}/api/v2/torrents/info?tag={TAG}", self.url))).await?;
        let text = self.text(r, "list its torrents").await?;
        let list: Vec<Value> = serde_json::from_str(&text).map_err(|_| format!("{} sent a list Lantern can't read.", self.name))?;
        Ok(list.iter().filter_map(item_of).collect())
    }

    /// The files of a torrent, as qBittorrent sees them.
    pub async fn files(&self, item: &Item) -> Result<Vec<(String, i64)>, String> {
        let r = self.send(|| self.http.get(format!("{}/api/v2/torrents/files?hash={}", self.url, item.id))).await?;
        let text = self.text(r, "list the torrent's files").await?;
        let list: Vec<Value> = serde_json::from_str(&text).map_err(|_| format!("{} sent a file list Lantern can't read.", self.name))?;
        let base = item.save_path.trim_end_matches('/');
        Ok(list
            .iter()
            .filter_map(|f| Some((format!("{base}/{}", f.get("name")?.as_str()?), f.get("size").and_then(Value::as_i64).unwrap_or(0))))
            .collect())
    }

    /// Removes torrent `hash`, and its files when `delete_files`.
    pub async fn remove(&self, hash: &str, delete_files: bool) -> Result<(), String> {
        let form = [("hashes", hash.to_string()), ("deleteFiles", delete_files.to_string())];
        let r = self.send(|| self.http.post(format!("{}/api/v2/torrents/delete", self.url)).form(&form)).await?;
        self.text(r, "remove the torrent").await.map(|_| ())
    }
}

/// A torrent of `torrents/info` as an [`Item`].
fn item_of(t: &Value) -> Option<Item> {
    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| t.get(k).and_then(Value::as_i64).unwrap_or(0);
    let tags = s("tags");
    let tag = tags.split(',').map(str::trim).find(|x| x.starts_with("lantern-")).unwrap_or("").to_string();
    let qstate = s("state");
    let state = match qstate.as_str() {
        "error" | "missingFiles" => ItemState::Failed,
        "pausedUP" | "stoppedUP" => ItemState::Completed,
        "uploading" | "stalledUP" | "queuedUP" | "forcedUP" | "checkingUP" => ItemState::Seeding,
        "pausedDL" | "stoppedDL" => ItemState::Paused,
        "queuedDL" | "metaDL" | "forcedMetaDL" | "allocating" => ItemState::Queued,
        _ => ItemState::Downloading,
    };
    let eta = n("eta");
    Some(Item {
        id: s("hash"),
        tag,
        name: s("name"),
        state,
        progress: t.get("progress").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
        size: n("size"),
        speed: n("dlspeed"),
        eta: if eta >= 8_640_000 { 0 } else { eta },
        path: s("content_path"),
        save_path: s("save_path"),
        ratio: t.get("ratio").and_then(Value::as_f64).unwrap_or(0.0),
        seed_secs: n("seeding_time"),
        error: if state == ItemState::Failed {
            if qstate == "missingFiles" {
                "qBittorrent can't find the torrent's files any more".into()
            } else {
                "qBittorrent stopped the torrent with an error".into()
            }
        } else {
            String::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_torrent_states() {
        let t = json!({"hash": "ab", "name": "Show", "state": "stalledUP", "progress": 1.0, "size": 10, "tags": "lantern, lantern-7",
            "content_path": "/d/tv/Show", "save_path": "/d/tv", "eta": 8640000});
        let i = item_of(&t).unwrap();
        assert_eq!((i.state, i.tag.as_str(), i.eta), (ItemState::Seeding, "lantern-7", 0));
        let i = item_of(&json!({"state": "missingFiles"})).unwrap();
        assert_eq!(i.state, ItemState::Failed);
        assert!(i.error.contains("can't find"));
        assert_eq!(item_of(&json!({"state": "metaDL"})).unwrap().state, ItemState::Queued);
    }
}
