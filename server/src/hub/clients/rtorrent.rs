//! rTorrent through XML-RPC (the address of its SCGI bridge, such as ruTorrent's `/RPC2`): add into a folder
//! with the app's category as label (`d.custom1`, as ruTorrent shows it) and Lantern's tag (`d.custom2`), list,
//! files, remove. rTorrent's XML-RPC can't delete files, so removing leaves them on disk.

use super::xmlrpc::{self, Param};
use super::{trim_url, unreachable, AddRequest, ClientConfig, Content, Item, ItemState};
use serde_json::Value;

/// One rTorrent.
pub struct Rtorrent {
    /// The name people see.
    name: String,
    /// The XML-RPC address.
    url: String,
    /// The user name (empty: no sign-in).
    username: String,
    /// The password.
    password: String,
    /// Sends the requests.
    http: reqwest::Client,
}

impl Rtorrent {
    /// The client for `cfg`; an address without a path gets `/RPC2`.
    pub fn new(cfg: &ClientConfig, http: reqwest::Client) -> Rtorrent {
        let url = trim_url(&cfg.url);
        let has_path = url::Url::parse(&url).map(|u| !u.path().trim_matches('/').is_empty()).unwrap_or(false);
        Rtorrent {
            name: cfg.name.clone(),
            url: if has_path { url } else { format!("{url}/RPC2") },
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            http,
        }
    }

    /// Calls `method` with `params`; the answer, or an error in words.
    async fn call(&self, method: &str, params: &[Param]) -> Result<Value, String> {
        let mut req = self.http.post(&self.url).header("content-type", "text/xml").body(xmlrpc::call(method, params));
        if !self.username.is_empty() {
            req = req.basic_auth(&self.username, Some(&self.password));
        }
        let r = req.send().await.map_err(|e| unreachable(&self.name, &self.url, &e))?;
        if r.status().as_u16() == 401 {
            return Err(format!("{} refused the user name or password.", self.name));
        }
        if !r.status().is_success() {
            return Err(format!("{} answered {} at {}. Check the XML-RPC address.", self.name, r.status().as_u16(), self.url));
        }
        let body = r.text().await.unwrap_or_default();
        xmlrpc::parse(&body).map_err(|e| format!("{} couldn't do that: {e}", self.name))
    }

    /// Asks the version.
    pub async fn test(&self) -> Result<String, String> {
        let v = self.call("system.client_version", &[]).await?;
        Ok(format!("{} {}", self.name, v.as_str().unwrap_or("")))
    }

    /// Adds and starts the torrent in `save_path`; "" (it is found again by its tag).
    pub async fn add(&self, req: &AddRequest) -> Result<String, String> {
        let cmds = [
            Param::Str(format!("d.directory.set=\"{}\"", req.save_path.replace('"', ""))),
            Param::Str(format!("d.custom1.set=\"{}\"", req.category.replace('"', ""))),
            Param::Str(format!("d.custom2.set={}", req.tag)),
        ];
        let mut params = vec![Param::Str(String::new())];
        let method = match &req.content {
            Content::Magnet(m) => {
                params.push(Param::Str(m.clone()));
                "load.start"
            }
            Content::Torrent(b) | Content::Nzb(b) => {
                params.push(Param::Bytes(b.clone()));
                "load.raw_start"
            }
        };
        params.extend(cmds);
        self.call(method, &params).await?;
        Ok(String::new())
    }

    /// Every torrent in the main view.
    pub async fn items(&self) -> Result<Vec<Item>, String> {
        let fields = [
            "d.hash=",
            "d.name=",
            "d.size_bytes=",
            "d.completed_bytes=",
            "d.down.rate=",
            "d.complete=",
            "d.state=",
            "d.is_active=",
            "d.directory=",
            "d.ratio=",
            "d.custom2=",
            "d.message=",
            "d.is_hash_checking=",
            "d.is_multi_file=",
        ];
        let mut params = vec![Param::Str(String::new()), Param::Str("main".into())];
        params.extend(fields.iter().map(|f| Param::Str(f.to_string())));
        let v = self.call("d.multicall2", &params).await?;
        Ok(v.as_array().map(|a| a.iter().filter_map(item_of).collect()).unwrap_or_default())
    }

    /// The files of a torrent, as rTorrent sees them.
    pub async fn files(&self, item: &Item) -> Result<Vec<(String, i64)>, String> {
        let params =
            [Param::Str(item.id.clone()), Param::Str(String::new()), Param::Str("f.path=".into()), Param::Str("f.size_bytes=".into())];
        let v = self.call("f.multicall", &params).await?;
        let dir = item.save_path.trim_end_matches('/');
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| Some((format!("{dir}/{}", f.get(0)?.as_str()?), f.get(1).and_then(Value::as_i64).unwrap_or(0))))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Removes torrent `hash` (its files stay on disk: rTorrent can't delete them over XML-RPC).
    pub async fn remove(&self, hash: &str, _delete_files: bool) -> Result<(), String> {
        self.call("d.erase", &[Param::Str(hash.to_string())]).await.map(|_| ())
    }
}

/// A row of `d.multicall2` as an [`Item`].
fn item_of(row: &Value) -> Option<Item> {
    let a = row.as_array()?;
    let s = |i: usize| a.get(i).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    let n = |i: usize| a.get(i).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
    let (size, done) = (n(2), n(3));
    let state = if n(12) == 1 {
        ItemState::Queued
    } else if n(5) == 1 {
        if n(6) == 1 && n(7) == 1 {
            ItemState::Seeding
        } else {
            ItemState::Completed
        }
    } else if n(6) == 1 && n(7) == 1 {
        ItemState::Downloading
    } else {
        ItemState::Paused
    };
    let speed = n(4);
    let dir = s(8).trim_end_matches('/').to_string();
    Some(Item {
        id: s(0),
        tag: s(10),
        name: s(1),
        state,
        progress: if size > 0 { (done as f64 / size as f64).clamp(0.0, 1.0) } else { 0.0 },
        size,
        speed,
        eta: if speed > 0 { (size - done).max(0) / speed } else { 0 },
        // d.directory is the torrent's own folder for a multi-file torrent and the parent for a single file
        path: if n(13) == 1 { dir.clone() } else { format!("{dir}/{}", s(1)) },
        save_path: dir,
        ratio: n(9) as f64 / 1000.0,
        seed_secs: 0,
        error: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_torrent_rows() {
        let i = item_of(&json!(["H", "X", 100, 50, 10, 0, 1, 1, "/d/tv/X", 1500, "lantern-4", "", 0, 1])).unwrap();
        assert_eq!((i.state, i.progress, i.eta, i.ratio, i.tag.as_str()), (ItemState::Downloading, 0.5, 5, 1.5, "lantern-4"));
        assert_eq!(i.path, "/d/tv/X");
        let single = item_of(&json!(["H", "a.mkv", 100, 100, 0, 1, 1, 1, "/d/tv", 0, "", "", 0, 0])).unwrap();
        assert_eq!(single.path, "/d/tv/a.mkv");
        assert_eq!(item_of(&json!(["H", "X", 100, 100, 0, 1, 1, 1, "/d", 0, "", "", 0])).unwrap().state, ItemState::Seeding);
        assert_eq!(item_of(&json!(["H", "X", 100, 100, 0, 1, 0, 0, "/d", 0, "", "", 0])).unwrap().state, ItemState::Completed);
        assert_eq!(item_of(&json!(["H", "X", 100, 10, 0, 0, 0, 0, "/d", 0, "", "", 0])).unwrap().state, ItemState::Paused);
    }
}
