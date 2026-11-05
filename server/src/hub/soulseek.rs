//! Soulseek as a download source, through [slskd](https://github.com/slskd/slskd) (a free Soulseek client with a
//! web API). With slskd's address and API key under Settings → Downloads → Soulseek, a hub search for music also
//! searches Soulseek: each folder a user shares is one result, and its grab token names that user and those files.
//! A grab asks slskd to download them, follows the transfers, and once every file is in links them (hardlinks
//! where it can, copies otherwise) into the app's folder, so the app gets one folder with exact paths like any
//! other download.

use super::apps::App;
use super::grabs::{list_files, Grab};
use super::{paths, Hub};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The start of a Soulseek grab's link, before the JSON of a [`Pick`].
pub const LINK: &str = "slsk:";

/// The indexer id Soulseek results are signed with (no real indexer has it).
pub const SIGNER_ID: i64 = 0;

/// The longest Lantern waits for one slskd request.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What a Soulseek result points at: one user's folder and the files in it (their full names on the user's side
/// and their sizes).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pick {
    /// The Soulseek user who shares the files.
    pub user: String,
    /// The folder on the user's side (`@@music\Artist\Album`).
    pub dir: String,
    /// Each file's full name on the user's side and its size in bytes.
    pub files: Vec<(String, i64)>,
}

impl Pick {
    /// The grab link for this pick (`slsk:{json}`).
    pub fn link(&self) -> String {
        format!("{LINK}{}", serde_json::to_string(self).unwrap_or_default())
    }

    /// The pick in a grab link, when it is one.
    pub fn from_link(link: &str) -> Option<Pick> {
        serde_json::from_str(link.strip_prefix(LINK)?).ok()
    }

    /// A name for the pick from its folder: its last two parts (`Artist - Album`).
    pub fn title(&self) -> String {
        let parts: Vec<&str> = self.dir.split(['\\', '/']).filter(|p| !p.is_empty() && !p.starts_with("@@")).collect();
        match parts.len() {
            0 => self.user.clone(),
            1 => parts[0].to_string(),
            n => format!("{} - {}", parts[n - 2], parts[n - 1]),
        }
    }
}

/// The last part of a name on a Soulseek user's side (`@@music\Artist\Album` → `Album`).
pub fn leaf(name: &str) -> &str {
    name.rsplit(['\\', '/']).find(|p| !p.is_empty()).unwrap_or(name)
}

/// The folder part of a name on a user's side (`a\b\c.flac` → `a\b`).
pub fn parent(name: &str) -> &str {
    name.rfind(['\\', '/']).map(|i| &name[..i]).unwrap_or("")
}

/// Where slskd puts a downloaded file, under its downloads folder `root` (as Lantern sees it): a folder named
/// after the file's folder on the user's side, then the file.
pub fn local_path(root: &str, remote: &str) -> PathBuf {
    let dir = leaf(parent(remote));
    let mut p = PathBuf::from(root);
    if !dir.is_empty() {
        p.push(safe(dir));
    }
    p.push(safe(leaf(remote)));
    p
}

/// A name that is safe as one folder or file name on any system.
pub fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() {
        "Soulseek".into()
    } else {
        s
    }
}

/// Groups slskd's search responses into one result per user folder, best first (a free upload slot, then the
/// shortest queue, then the fastest upload): (pick, free slot, upload speed, queue length).
pub fn picks(responses: &[Value]) -> Vec<(Pick, bool, i64, i64)> {
    let mut out: Vec<(Pick, bool, i64, i64)> = Vec::new();
    for r in responses {
        let user = r.get("username").and_then(Value::as_str).unwrap_or("").to_string();
        if user.is_empty() {
            continue;
        }
        let free = r.get("hasFreeUploadSlot").and_then(Value::as_bool).unwrap_or(false);
        let speed = r.get("uploadSpeed").and_then(Value::as_i64).unwrap_or(0);
        let queue = r.get("queueLength").and_then(Value::as_i64).unwrap_or(0);
        let mut by_dir: Vec<Pick> = Vec::new();
        for f in r.get("files").and_then(Value::as_array).into_iter().flatten() {
            let name = f.get("filename").and_then(Value::as_str).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let size = f.get("size").and_then(Value::as_i64).unwrap_or(0);
            let dir = parent(&name).to_string();
            match by_dir.iter_mut().find(|p| p.dir == dir) {
                Some(p) => p.files.push((name, size)),
                None => by_dir.push(Pick { user: user.clone(), dir, files: vec![(name, size)] }),
            }
        }
        out.extend(by_dir.into_iter().map(|p| (p, free, speed, queue)));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)).then(b.2.cmp(&a.2)));
    out
}

/// Where a grab's transfers stand, from slskd's list of the user's downloads.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Every file is in.
    Done,
    /// A file failed: what happened, in words.
    Failed(String),
    /// Still going: (progress 0–1, bytes per second, whether a file is moving yet).
    Going(f64, i64, bool),
    /// slskd doesn't list the files (yet, or any more).
    Missing,
}

/// Where `pick`'s files stand in slskd's `transfers` of its user (`GET /api/v0/transfers/downloads/{user}`).
pub fn status(pick: &Pick, transfers: &Value) -> Status {
    let all: Vec<&Value> = transfers
        .get("directories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|d| d.get("files").and_then(Value::as_array).into_iter().flatten())
        .collect();
    let (mut got, mut total, mut speed, mut moving, mut done, mut seen) = (0i64, 0i64, 0i64, false, 0usize, 0usize);
    for (name, size) in &pick.files {
        // the newest transfer of the file counts (an earlier try may have failed)
        let Some(t) = all.iter().rev().find(|t| t.get("filename").and_then(Value::as_str) == Some(name.as_str())) else {
            total += size;
            continue;
        };
        seen += 1;
        let state = t.get("state").and_then(Value::as_str).unwrap_or("");
        total += t.get("size").and_then(Value::as_i64).unwrap_or(*size);
        got += t.get("bytesTransferred").and_then(Value::as_i64).unwrap_or(0);
        if let Some(why) = failure(state, &pick.user, leaf(name)) {
            return Status::Failed(why);
        }
        if state.contains("Succeeded") {
            done += 1;
        } else if state.contains("InProgress") || state.contains("Initializing") {
            moving = true;
            speed += t.get("averageSpeed").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        }
    }
    if seen == 0 {
        return Status::Missing;
    }
    if done == pick.files.len() {
        return Status::Done;
    }
    Status::Going(if total > 0 { (got as f64 / total as f64).clamp(0.0, 1.0) } else { 0.0 }, speed, moving)
}

/// What went wrong with a transfer in `state`, in words; `None` when nothing did.
pub fn failure(state: &str, user: &str, file: &str) -> Option<String> {
    if !state.starts_with("Completed") || state.contains("Succeeded") {
        return None;
    }
    Some(if state.contains("Cancelled") {
        format!("The download of {file} was cancelled in Soulseek.")
    } else if state.contains("TimedOut") {
        format!("{user} stopped answering while sending {file}.")
    } else if state.contains("Rejected") {
        format!("{user} turned down the download of {file}.")
    } else {
        format!("The transfer of {file} from {user} broke off.")
    })
}

impl Hub {
    /// slskd's address and API key, when Soulseek is set up.
    fn slskd(&self) -> Option<(String, String)> {
        let url = self.settings.str("soulseek_url").trim().trim_end_matches('/').to_string();
        (!url.is_empty()).then(|| (url, self.settings.str("soulseek_api_key")))
    }

    /// Whether Soulseek is set up.
    pub fn soulseek_on(&self) -> bool {
        self.slskd().is_some()
    }

    /// Where Lantern sees slskd's downloads folder.
    fn soulseek_root(&self) -> String {
        let r = paths::clean(&self.settings.str("soulseek_downloads"));
        if r.is_empty() {
            format!("{}/soulseek", self.root())
        } else {
            r
        }
    }

    /// One request to slskd's API at `path` (`/api/v0/...`): its JSON answer, `Null` for an empty one.
    async fn slskd_call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let (base, key) =
            self.slskd().ok_or_else(|| "Soulseek is not set up. Add slskd's address under Settings → Downloads.".to_string())?;
        let mut req = self.http.request(method, format!("{base}{path}")).header("X-API-Key", key).timeout(TIMEOUT);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.map_err(|e| format!("Lantern can't reach slskd at {base}: {e}"))?;
        let status = r.status().as_u16();
        if status == 401 || status == 403 {
            return Err("slskd turned down Lantern's API key. Check the key under Settings → Downloads → Soulseek.".into());
        }
        if status == 404 {
            return Ok(Value::Null);
        }
        if !(200..300).contains(&status) {
            let text = r.text().await.unwrap_or_default();
            return Err(format!("slskd answered {status}: {}", text.trim()));
        }
        let text = r.text().await.map_err(|e| format!("slskd's answer stopped halfway: {e}"))?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Checks Lantern can reach slskd with its key: `{ok, message}`.
    pub async fn test_soulseek(&self) -> Value {
        match self.slskd_call(reqwest::Method::GET, "/api/v0/application", None).await {
            Ok(v) => {
                let version = v.pointer("/version/full").or_else(|| v.get("version")).and_then(Value::as_str).unwrap_or("").to_string();
                let message = if version.is_empty() { "Connected to slskd".to_string() } else { format!("Connected to slskd {version}") };
                json!({ "ok": true, "message": message })
            }
            Err(e) => json!({ "ok": false, "message": e }),
        }
    }

    /// Searches Soulseek for `words`; one result per user folder, in the shape of the hub's search results.
    pub async fn search_soulseek(&self, words: &str) -> Result<Vec<Value>, String> {
        let secs = self.settings.int("soulseek_search_secs").clamp(3, 60);
        let started = self
            .slskd_call(reqwest::Method::POST, "/api/v0/searches", Some(json!({ "searchText": words, "searchTimeout": secs * 1000 })))
            .await?;
        let id = started.get("id").and_then(Value::as_str).ok_or("slskd didn't start the search")?.to_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(secs as u64 + 5);
        loop {
            let s = self.slskd_call(reqwest::Method::GET, &format!("/api/v0/searches/{id}"), None).await?;
            if s.get("isComplete").and_then(Value::as_bool) == Some(true) || std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let responses = self.slskd_call(reqwest::Method::GET, &format!("/api/v0/searches/{id}/responses"), None).await?;
        let _ = self.slskd_call(reqwest::Method::DELETE, &format!("/api/v0/searches/{id}"), None).await;
        let list = responses.as_array().cloned().unwrap_or_default();
        Ok(picks(&list)
            .into_iter()
            .take(100)
            .map(|(p, free, speed, queue)| {
                let link = p.link();
                let (l, sig) = self.signer.sign(SIGNER_ID, &link);
                json!({
                    "title": p.title(), "indexer": "Soulseek", "indexer_id": 0, "protocol": "soulseek",
                    "size": p.files.iter().map(|(_, s)| s).sum::<i64>(), "files": p.files.len(), "user": p.user,
                    "free_slot": free, "upload_speed": speed, "queue": queue, "categories": [3000],
                    "token": format!("{SIGNER_ID}.{l}.{sig}"),
                })
            })
            .collect())
    }

    /// The path of slskd's API for `user`'s downloads.
    fn downloads_path(user: &str) -> String {
        format!(
            "/api/v0/transfers/downloads/{}",
            url::form_urlencoded::byte_serialize(user.as_bytes()).collect::<String>().replace('+', "%20")
        )
    }

    /// Asks slskd to download grab `g`'s files.
    pub(super) async fn send_soulseek(&self, g: &Grab) -> Result<(), String> {
        let pick = Pick::from_link(&g.link).ok_or("This Soulseek grab doesn't say which files to get. Search again.")?;
        let files: Vec<Value> = pick.files.iter().map(|(f, s)| json!({ "filename": f, "size": s })).collect();
        self.slskd_call(reqwest::Method::POST, &Self::downloads_path(&pick.user), Some(json!(files))).await?;
        let size: i64 = pick.files.iter().map(|(_, s)| s).sum();
        self.set(
            g.id,
            &[
                ("state", json!("waiting")),
                ("client_id", json!(0)),
                ("client_ref", json!(pick.user)),
                ("size", json!(size)),
                ("sent", json!(crate::auth::now())),
                ("error", json!("")),
                ("progress", json!(0.0)),
            ],
        )
        .map_err(|e| e.detail)
    }

    /// Follows every Soulseek grab in slskd, and links finished ones into their app's folder.
    pub(super) async fn follow_soulseek(&self) -> kit_server_core::Result<()> {
        use kit_server_core::{p, Db};
        if !self.soulseek_on() {
            return Ok(());
        }
        let rows =
            self.db.query("SELECT * FROM hub_grabs WHERE protocol='soulseek' AND state IN ('waiting','downloading') ORDER BY id", p![])?;
        for g in rows.iter().map(Grab::from_row) {
            let Ok(app) = self.app(g.app_id) else { continue };
            let Some(pick) = Pick::from_link(&g.link) else { continue };
            let transfers = match self.slskd_call(reqwest::Method::GET, &Self::downloads_path(&pick.user), None).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("download hub: {e}");
                    continue;
                }
            };
            match status(&pick, &transfers) {
                Status::Failed(why) => self.failed(&g, &app, &why)?,
                Status::Missing if crate::auth::now() - g.sent > super::grabs::GRACE => {
                    self.failed(&g, &app, &format!("The download disappeared from slskd's list of downloads from {}.", pick.user))?
                }
                Status::Missing => {}
                Status::Going(progress, speed, moving) => {
                    let state = if moving || progress > 0.0 { "downloading" } else { "waiting" };
                    self.set(g.id, &[("state", json!(state)), ("progress", json!(progress)), ("speed", json!(speed))])?;
                    if state != g.state || (progress - g.progress).abs() >= 0.01 {
                        let g = self.get_grab(g.id)?;
                        self.emit("grab.progress", &app, self.grab_json(&g, &app));
                    }
                }
                Status::Done => match self.bring_in(&pick, &app) {
                    Ok(folder) => {
                        let files = list_files(&folder);
                        let path = folder.to_string_lossy().replace('\\', "/");
                        self.set(
                            g.id,
                            &[
                                ("state", json!("completed")),
                                ("progress", json!(1.0)),
                                ("speed", json!(0)),
                                ("eta", json!(0)),
                                ("path", json!(path)),
                                ("files", json!(files)),
                                ("finished", json!(crate::auth::now())),
                                ("error", json!("")),
                            ],
                        )?;
                        let g = self.get_grab(g.id)?;
                        self.emit("grab.completed", &app, self.grab_json(&g, &app));
                    }
                    Err(why) => self.failed(&g, &app, &why)?,
                },
            }
        }
        Ok(())
    }

    /// Links `pick`'s downloaded files from slskd's folder into a folder of their own in `app`'s folder (a
    /// hardlink where it can, a copy otherwise); that folder.
    fn bring_in(&self, pick: &Pick, app: &App) -> Result<PathBuf, String> {
        let root = self.soulseek_root();
        let to = PathBuf::from(self.folder(app)).join(safe(&pick.title()));
        std::fs::create_dir_all(&to).map_err(|e| format!("Lantern can't make the folder {}: {e}", to.display()))?;
        for (name, _) in &pick.files {
            let from = local_path(&root, name);
            if !from.is_file() {
                return Err(format!(
                    "Soulseek finished {}, but Lantern can't find it at {}. Check that Soulseek's downloads folder under Settings → Downloads is where Lantern sees slskd's downloads.",
                    leaf(name),
                    from.display()
                ));
            }
            let dest = to.join(safe(leaf(name)));
            if dest.exists() {
                continue;
            }
            if std::fs::hard_link(&from, &dest).is_err() {
                std::fs::copy(&from, &dest).map_err(|e| format!("Lantern can't put {} into {}: {e}", leaf(name), to.display()))?;
            }
        }
        Ok(to)
    }

    /// Takes grab `g`'s files out of slskd's list (best effort); with `delete_files`, deletes slskd's copies and
    /// the app's folder of them too.
    pub(super) async fn drop_soulseek(&self, g: &Grab, delete_files: bool) {
        let Some(pick) = Pick::from_link(&g.link) else { return };
        if let Ok(t) = self.slskd_call(reqwest::Method::GET, &Self::downloads_path(&pick.user), None).await {
            let ids: Vec<String> = t
                .get("directories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|d| d.get("files").and_then(Value::as_array).cloned().unwrap_or_default())
                .filter(|f| pick.files.iter().any(|(n, _)| f.get("filename").and_then(Value::as_str) == Some(n.as_str())))
                .filter_map(|f| f.get("id").and_then(Value::as_str).map(String::from))
                .collect();
            for id in ids {
                let path = format!("{}/{id}?remove=true", Self::downloads_path(&pick.user));
                if let Err(e) = self.slskd_call(reqwest::Method::DELETE, &path, None).await {
                    tracing::warn!("download hub: {e}");
                }
            }
        }
        if delete_files {
            let root = self.soulseek_root();
            for (name, _) in &pick.files {
                let _ = std::fs::remove_file(local_path(&root, name));
            }
            if !g.path.is_empty() && Path::new(&g.path).starts_with(self.root()) {
                let _ = std::fs::remove_dir_all(&g.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_on_the_users_side_become_titles_and_local_paths() {
        let p = Pick { user: "ann".into(), dir: "@@music\\Artist\\Album (2001)".into(), files: vec![] };
        assert_eq!(p.title(), "Artist - Album (2001)");
        assert_eq!(Pick::from_link(&p.link()), Some(p));
        assert_eq!(local_path("/dl/slsk", "@@music\\Artist\\Album\\01 - a.flac"), PathBuf::from("/dl/slsk/Album/01 - a.flac"));
        assert_eq!(safe("a/b: c?"), "a_b_ c_");
        assert_eq!(safe(".."), "Soulseek");
    }

    #[test]
    fn responses_become_one_result_per_folder_best_first() {
        let r = vec![
            json!({ "username": "slow", "hasFreeUploadSlot": false, "uploadSpeed": 900, "queueLength": 4,
                "files": [{ "filename": "m\\A\\B\\1.flac", "size": 10 }] }),
            json!({ "username": "fast", "hasFreeUploadSlot": true, "uploadSpeed": 100, "queueLength": 0,
                "files": [{ "filename": "m\\A\\B\\1.mp3", "size": 3 }, { "filename": "m\\A\\B\\2.mp3", "size": 4 }, { "filename": "m\\A\\C\\1.mp3", "size": 5 }] }),
        ];
        let p = picks(&r);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].0.user, "fast");
        assert_eq!(p[0].0.files, vec![("m\\A\\B\\1.mp3".to_string(), 3), ("m\\A\\B\\2.mp3".to_string(), 4)]);
        assert_eq!(p[2].0.user, "slow");
    }

    #[test]
    fn transfers_say_where_a_grab_stands() {
        let pick = Pick { user: "ann".into(), dir: "m\\A".into(), files: vec![("m\\A\\1.flac".into(), 10), ("m\\A\\2.flac".into(), 10)] };
        let t = |a: &str, b: &str, got: i64| {
            json!({ "directories": [{ "files": [
                { "filename": "m\\A\\1.flac", "size": 10, "state": a, "bytesTransferred": 10, "averageSpeed": 5.0 },
                { "filename": "m\\A\\2.flac", "size": 10, "state": b, "bytesTransferred": got, "averageSpeed": 5.0 },
            ]}]})
        };
        assert_eq!(status(&pick, &json!({})), Status::Missing);
        assert_eq!(status(&pick, &t("Completed, Succeeded", "InProgress", 5)), Status::Going(0.75, 5, true));
        assert_eq!(status(&pick, &t("Completed, Succeeded", "Queued, Remotely", 0)), Status::Going(0.5, 0, false));
        assert_eq!(status(&pick, &t("Completed, Succeeded", "Completed, Succeeded", 10)), Status::Done);
        assert_eq!(
            status(&pick, &t("Completed, Succeeded", "Completed, Rejected", 0)),
            Status::Failed("ann turned down the download of 2.flac.".into())
        );
    }
}
