// Ported from cross-seed v6.13.9 (src/clients/QBittorrent.ts), Apache-2.0.
//! qBittorrent through its Web API v2, as cross-seed talks to it: every call is a `POST` with the session cookie,
//! signing in again on a 403 and trying again on a 5xx (up to three more times, 1, 2, 4 seconds apart). A match is
//! added with `torrents/add` (skip checking unless it must be rechecked, paused or stopped when it is), tagged
//! `cross-seed`, in the searchee's category (or `linkCategory` for linked files).

use super::{
    backoff, client_searchee_modified, http, kept_searchee, may_resume, organize_trackers, should_recheck, DirError, Info, Injected,
    MetaRef, ResumeTiming, SearcheeOptions, TorrentInClient,
};
use crate::crossseed::config::RuntimeConfig;
use crate::crossseed::constants::{Decision, InjectionResult, ABS_WIN_PATH_REGEX, TORRENT_CATEGORY_SUFFIX, TORRENT_TAG};
use crate::crossseed::jsre::test;
use crate::crossseed::searchee::{basename, dirname, parse_title, File, Searchee};
use crate::crossseed::torrent::Metafile;
use crate::crossseed::utils::{extract_int, human_readable_size, sanitize_info_hash};
use kit_server_core::SqliteDb;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `TorrentInfo`: the parts of `torrents/info` cross-seed reads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentInfo {
    /// Bytes still missing.
    pub amount_left: i64,
    /// Automatic torrent management.
    pub auto_tmm: bool,
    /// Its category.
    pub category: String,
    /// Its file (one file) or folder.
    pub content_path: String,
    /// Its files (qBittorrent 5.1 with `includeFiles`).
    pub files: Option<Vec<TorrentFile>>,
    /// Its hash (v1, or v2 for a v2-only torrent).
    pub hash: String,
    /// Its v1 hash.
    pub infohash_v1: Option<String>,
    /// Its v2 hash.
    pub infohash_v2: Option<String>,
    /// Its name.
    pub name: String,
    /// Its save folder.
    pub save_path: String,
    /// Its state (`uploading`, `pausedDL`, `checkingUP`, …).
    pub state: String,
    /// Its tags, comma-separated.
    pub tags: String,
    /// Every file's size added up.
    pub total_size: i64,
    /// The tracker it is talking to now.
    pub tracker: String,
    /// Its trackers (qBittorrent 5.1 with `includeTrackers`).
    pub trackers: Option<Vec<TorrentTracker>>,
}

/// `TorrentTracker`: one tracker of `torrents/trackers`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentTracker {
    /// Its announce address.
    pub url: String,
    /// Its tier.
    pub tier: serde_json::Value,
}

/// `TorrentFile`: one file of `torrents/files`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentFile {
    /// Its path inside the save folder.
    pub name: String,
    /// Its size.
    pub size: i64,
}

/// `CategoryInfo`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CategoryInfo {
    /// Its name.
    name: String,
    /// Its folder.
    #[serde(rename = "savePath")]
    save_path: String,
}

/// A request's body, built again for each try.
#[derive(Debug, Clone)]
enum Body {
    /// No body and no content type.
    Empty,
    /// A form (`application/x-www-form-urlencoded`).
    Form(Vec<(String, String)>),
    /// A multipart form: (name, value, file name for the torrent).
    Multipart(Vec<(String, Vec<u8>, Option<String>)>),
}

/// The session and version, kept between calls.
#[derive(Debug, Default)]
struct Session {
    /// The `SID` cookie.
    cookie: String,
    /// The version (`v5.0.0`).
    version: String,
    /// Its major number.
    major: i64,
}

/// One qBittorrent.
#[derive(Clone)]
pub struct Qbittorrent {
    /// The client's host, priority and label.
    info: Info,
    /// `href`: the address with `/api/v2`; `None` when it can't be read (cross-seed's "must be percent-encoded").
    href: Option<String>,
    /// The user name.
    username: String,
    /// The password.
    password: String,
    /// The session.
    session: Arc<Mutex<Session>>,
    /// The HTTP client.
    http: reqwest::Client,
}

/// `isTorrentInfoComplete`.
pub fn is_torrent_info_complete(t: &TorrentInfo) -> bool {
    ["uploading", "pausedUP", "stoppedUP", "queuedUP", "stalledUP", "checkingUP", "forcedUP"].contains(&t.state.as_str())
}

/// The file of a `torrents/files` entry.
fn torrent_file_to_file(f: &TorrentFile) -> File {
    File { name: basename(&f.name), path: f.name.clone(), length: f.size }
}

/// `path.win32.dirname` for the Windows paths qBittorrent may give.
fn win_dirname(p: &str) -> String {
    let t = p.trim_end_matches(['\\', '/']);
    match t.rfind(['\\', '/']) {
        Some(at) if at > 0 => t[..at].to_string(),
        Some(_) => t[..1].to_string(),
        None => ".".into(),
    }
}

impl Qbittorrent {
    /// The client for `url` (with its user name and password).
    pub fn new(url: &str, info: &Info) -> Qbittorrent {
        let creds = super::extract_credentials_from_url(url, Some("/api/v2"));
        Qbittorrent {
            info: info.clone(),
            href: creds.as_ref().map(|c| c.href.clone()),
            username: creds.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
            password: creds.map(|c| c.password).unwrap_or_default(),
            session: Arc::new(Mutex::new(Session::default())),
            http: http(),
        }
    }

    /// The address, or cross-seed's error.
    fn href(&self) -> Result<&str, String> {
        self.href.as_deref().ok_or_else(|| format!("[{}] qBittorrent url must be percent-encoded", self.info.label))
    }

    /// The major version (0 before signing in).
    fn major(&self) -> i64 {
        self.session.lock().map(|s| s.major).unwrap_or(0)
    }

    /// `login`: signs in, keeps the cookie and checks the version (4.3.1 or later).
    pub fn login(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(self.login_inner())
    }

    /// The body of [`login`](Self::login).
    async fn login_inner(&self) -> Result<(), String> {
        let href = self.href()?.to_string();
        let r = self
            .http
            .post(format!("{href}/auth/login"))
            .form(&[("username", self.username.as_str()), ("password", self.password.as_str())])
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("qBittorrent login failed: {e}"))?;
        if !r.status().is_success() {
            return Err(format!("qBittorrent login failed with code {}", r.status().as_u16()));
        }
        let cookie = r
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .next()
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").to_string())
            .unwrap_or_default();
        if cookie.is_empty() {
            return Err("qBittorrent login failed: Invalid username or password".into());
        }
        if let Ok(mut s) = self.session.lock() {
            s.cookie = cookie;
        }
        let version = self.request("/app/version", Body::Form(Vec::new()), 3).await.unwrap_or_default();
        if version.is_empty() {
            return Err("qBittorrent login failed: Unable to retrieve version".into());
        }
        let parts: Vec<&str> = version.split('.').collect();
        let (major, minor, patch) =
            (extract_int(&version), parts.get(1).map(|p| extract_int(p)).unwrap_or(0), parts.get(2).map(|p| extract_int(p)).unwrap_or(0));
        if major < 4 || (major == 4 && minor < 3) || (major == 4 && minor == 3 && patch < 1) {
            return Err(format!("qBittorrent minimum supported version is v4.3.1, current version is {version}"));
        }
        if let Ok(mut s) = self.session.lock() {
            s.version = version.clone();
            s.major = major;
        }
        tracing::info!(
            "cross-seed [{}]: Logged in to {version} successfully{}",
            self.info.label,
            if self.info.readonly { " (readonly)" } else { "" }
        );
        Ok(())
    }

    /// `validateConfig`: signs in and makes the `cross-seed` tag.
    pub async fn validate_config(&self) -> Result<(), String> {
        self.login().await.map_err(|e| format!("[{}] {e}", self.info.label))?;
        self.create_tag().await;
        Ok(())
    }

    /// `request`: `POST` to `path` with the cookie, trying again as cross-seed does; the answer's text, or `None`
    /// when no answer came.
    async fn request(&self, path: &str, body: Body, retries: u32) -> Option<String> {
        let href = self.href().ok()?.to_string();
        let mut last: Option<reqwest::Response> = None;
        for i in 0..=retries {
            tracing::debug!("cross-seed [{}]: Making request ({}) to {path}", self.info.label, retries - i);
            let cookie = self.session.lock().map(|s| s.cookie.clone()).unwrap_or_default();
            let mut req = self.http.post(format!("{href}{path}")).header("Cookie", cookie).timeout(Duration::from_secs(600));
            req = match &body {
                Body::Empty => req,
                Body::Form(f) => req
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(f).finish()),
                Body::Multipart(parts) => {
                    let mut form = reqwest::multipart::Form::new();
                    for (name, value, file) in parts {
                        form = match file {
                            Some(f) => form.part(
                                name.clone(),
                                reqwest::multipart::Part::bytes(value.clone())
                                    .file_name(f.clone())
                                    .mime_str("application/x-bittorrent")
                                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(value.clone())),
                            ),
                            None => form.text(name.clone(), String::from_utf8_lossy(value).into_owned()),
                        };
                    }
                    req.multipart(form)
                }
            };
            match req.send().await {
                Ok(r) if r.status().as_u16() == 403 => {
                    last = Some(r);
                    if i >= retries {
                        tracing::error!("cross-seed [{}]: Received 403 from API after {retries} retries", self.info.label);
                        break;
                    }
                    if let Err(e) = self.login().await {
                        tracing::debug!("cross-seed [{}]: {e}", self.info.label);
                    }
                    backoff(i).await;
                }
                Ok(r) if r.status().is_server_error() => {
                    let status = r.status().as_u16();
                    last = Some(r);
                    if i >= retries {
                        tracing::error!("cross-seed [{}]: Received {status} from API after {retries} retries", self.info.label);
                        break;
                    }
                    backoff(i).await;
                }
                Ok(r) => {
                    last = Some(r);
                    break;
                }
                Err(e) => {
                    if i >= retries {
                        tracing::error!("cross-seed [{}]: Request failed after {retries} retries: {e}", self.info.label);
                        break;
                    }
                    backoff(i).await;
                }
            }
        }
        match last {
            Some(r) => r.text().await.ok(),
            None => None,
        }
    }

    /// A form body.
    fn form(pairs: &[(&str, &str)]) -> Body {
        Body::Form(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    /// `createTag`.
    async fn create_tag(&self) {
        self.request("/torrents/createTags", Self::form(&[("tags", TORRENT_TAG)]), 3).await;
    }

    /// `createCategory`.
    async fn create_category(&self, category: &str, save_path: &str) {
        self.request("/torrents/createCategory", Self::form(&[("category", category), ("savePath", save_path)]), 3).await;
    }

    /// `editCategory`.
    async fn edit_category(&self, category: &str, save_path: &str) {
        self.request("/torrents/editCategory", Self::form(&[("category", category), ("savePath", save_path)]), 3).await;
    }

    /// `getAllCategories`.
    async fn get_all_categories(&self) -> Vec<CategoryInfo> {
        let text = self.request("/torrents/categories", Body::Empty, 3).await.unwrap_or_default();
        serde_json::from_str::<HashMap<String, CategoryInfo>>(&text).map(|m| m.into_values().collect()).unwrap_or_default()
    }

    /// `getFiles`.
    async fn get_files(&self, info_hash: &str) -> Option<Vec<File>> {
        let text = self.request("/torrents/files", Self::form(&[("hash", info_hash)]), 3).await?;
        serde_json::from_str::<Vec<TorrentFile>>(&text).ok().map(|f| f.iter().map(torrent_file_to_file).collect())
    }

    /// `getTrackers`.
    async fn get_trackers(&self, info_hash: &str) -> Option<Vec<String>> {
        let text = self.request("/torrents/trackers", Self::form(&[("hash", info_hash)]), 3).await?;
        serde_json::from_str::<Vec<TorrentTracker>>(&text).ok().map(|t| organize_trackers(t.iter().map(|t| t.url.as_str())))
    }

    /// `recheckTorrent`: stops (pauses before v5) the torrent, then rechecks it.
    pub async fn recheck_torrent(&self, info_hash: &str) {
        let stop = if self.major() >= 5 { "/torrents/stop" } else { "/torrents/pause" };
        self.request(stop, Self::form(&[("hashes", info_hash)]), 3).await;
        self.request("/torrents/recheck", Self::form(&[("hashes", info_hash)]), 3).await;
    }

    /// `getAllTorrentInfo`.
    pub async fn get_all_torrent_info(&self, include_files: bool, include_trackers: bool) -> Vec<TorrentInfo> {
        let mut form = Vec::new();
        if include_files {
            form.push(("includeFiles".to_string(), "true".to_string()));
        }
        if include_trackers {
            form.push(("includeTrackers".to_string(), "true".to_string()));
        }
        let text = self.request("/torrents/info", Body::Form(form), 3).await.unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// `getTorrentInfo`: the torrent with this hash (v1, v2 or either), asking up to `retries + 1` times.
    pub async fn get_torrent_info(&self, hash: Option<&str>, retries: u32) -> Option<TorrentInfo> {
        let hash = hash?;
        for i in 0..=retries {
            if let Some(text) = self.request("/torrents/info", Self::form(&[("hashes", hash)]), 3).await {
                if let Ok(list) = serde_json::from_str::<Vec<TorrentInfo>>(&text) {
                    if let Some(t) = list.into_iter().next() {
                        return Some(t);
                    }
                }
            }
            let all = self.get_all_torrent_info(false, false).await;
            if let Some(t) =
                all.into_iter().find(|t| t.hash == hash || t.infohash_v1.as_deref() == Some(hash) || t.infohash_v2.as_deref() == Some(hash))
            {
                return Some(t);
            }
            if i < retries {
                backoff(i).await;
            }
        }
        None
    }

    /// `isSubfolderContentLayout`: a one-file torrent added with "Create subfolder" (only for `.torrent` files,
    /// never with the client's own torrents).
    fn is_subfolder_content_layout(files: &[File], t: &TorrentInfo, cfg: &RuntimeConfig) -> bool {
        if cfg.use_client_torrents || files.len() > 1 {
            return false;
        }
        let Some(f) = files.first() else { return false };
        let win = test(&ABS_WIN_PATH_REGEX, &t.content_path);
        let dir = |p: &str| if win { win_dirname(p) } else { dirname(p) };
        if dirname(&f.path) != "." {
            return false;
        }
        dir(&t.content_path).trim_end_matches(['/', '\\']) != t.save_path.trim_end_matches(['/', '\\'])
    }

    /// `isNoSubfolderContentLayout`: a torrent added with "Don't create subfolder" (only for `.torrent` files).
    fn is_no_subfolder_content_layout(files: &[File], t: &TorrentInfo, cfg: &RuntimeConfig) -> bool {
        if cfg.use_client_torrents {
            return false;
        }
        if files.len() > 1 {
            return t.content_path == t.save_path;
        }
        let Some(f) = files.first() else { return false };
        if dirname(&f.path) == "." {
            return false;
        }
        let rel = t.content_path.strip_prefix(t.save_path.trim_end_matches(['/', '\\'])).unwrap_or(&t.content_path);
        let parts = |p: &str| p.split(['/', '\\']).filter(|s| !s.is_empty()).count();
        parts(rel) < parts(&f.path)
    }

    /// `getCorrectSavePath`: the save folder, or the content's folder for "Create subfolder".
    fn get_correct_save_path(files: &[File], t: &TorrentInfo, cfg: &RuntimeConfig) -> String {
        if Self::is_subfolder_content_layout(files, t, cfg) {
            return if test(&ABS_WIN_PATH_REGEX, &t.content_path) { win_dirname(&t.content_path) } else { dirname(&t.content_path) };
        }
        t.save_path.clone()
    }

    /// `getDownloadDir`.
    pub async fn get_download_dir(&self, meta: MetaRef<'_>, only_completed: bool, cfg: &RuntimeConfig) -> Result<String, DirError> {
        let Some(t) = self.get_torrent_info(Some(meta.info_hash), 0).await else { return Err(DirError::NotFound) };
        if cfg.torrent_dir.is_some() && Self::is_no_subfolder_content_layout(meta.files, &t, cfg) {
            tracing::error!(
                "cross-seed [{}]: NoSubfolder content layout is not supported with torrentDir, use https://www.cross-seed.org/docs/basics/options#useclienttorrents: {} [{}]",
                self.info.label,
                t.name,
                sanitize_info_hash(&t.hash)
            );
            return Err(DirError::InvalidData);
        }
        if only_completed && !is_torrent_info_complete(&t) {
            return Err(DirError::TorrentNotComplete);
        }
        Ok(Self::get_correct_save_path(meta.files, &t, cfg))
    }

    /// `getAllDownloadDirs`.
    pub async fn get_all_download_dirs(
        &self,
        metas: &[MetaRef<'_>],
        only_completed: bool,
        v1_hash_only: bool,
        cfg: &RuntimeConfig,
    ) -> Result<HashMap<String, String>, String> {
        let by_hash: HashMap<&str, &MetaRef> = metas.iter().map(|m| (m.info_hash, m)).collect();
        let mut out = HashMap::new();
        for t in self.get_all_torrent_info(false, false).await {
            let meta = by_hash
                .get(t.hash.as_str())
                .or_else(|| t.infohash_v2.as_deref().and_then(|h| by_hash.get(h)))
                .or_else(|| t.infohash_v1.as_deref().and_then(|h| by_hash.get(h)));
            if let (Some(_), Some(m)) = (&cfg.torrent_dir, meta) {
                if Self::is_no_subfolder_content_layout(m.files, &t, cfg) {
                    return Err(format!(
                        "[{}] NoSubfolder content layout is not supported with torrentDir, use https://www.cross-seed.org/docs/basics/options#useclienttorrents: {} [{}]",
                        self.info.label,
                        t.name,
                        sanitize_info_hash(&t.hash)
                    ));
                }
            }
            if only_completed && !is_torrent_info_complete(&t) {
                continue;
            }
            let save = match meta {
                Some(m) => Self::get_correct_save_path(m.files, &t, cfg),
                None => t.save_path.clone(),
            };
            if let Some(v1) = t.infohash_v1.as_deref().filter(|h| !h.is_empty()) {
                out.insert(v1.to_string(), save.clone());
            }
            if v1_hash_only {
                continue;
            }
            out.insert(t.hash.clone(), save.clone());
            if let Some(v2) = t.infohash_v2.as_deref().filter(|h| !h.is_empty()) {
                out.insert(v2.to_string(), save);
            }
        }
        Ok(out)
    }

    /// `getAllTorrents`.
    pub async fn get_all_torrents(&self) -> Vec<TorrentInClient> {
        self.get_all_torrent_info(false, true)
            .await
            .into_iter()
            .map(|t| TorrentInClient {
                info_hash: t.hash.clone(),
                category: Some(t.category.clone()),
                tags: Some(if t.tags.is_empty() { Vec::new() } else { t.tags.split(',').map(String::from).collect() }),
                trackers: match &t.trackers {
                    Some(list) => Some(organize_trackers(list.iter().map(|t| t.url.as_str()))),
                    None if !t.tracker.is_empty() => Some(vec![t.tracker.clone()]),
                    None => None,
                },
            })
            .collect()
    }

    /// `getClientSearchees`' listing: (every searchee, the new ones, every info hash); `None` when it failed.
    pub async fn list_torrents(
        &self,
        db: &SqliteDb,
        _cfg: &RuntimeConfig,
        o: &SearcheeOptions,
    ) -> Option<(Vec<Searchee>, Vec<Searchee>, HashSet<String>)> {
        let (mut all, mut new, mut hashes) = (Vec::new(), Vec::new(), HashSet::new());
        let torrents = self.get_all_torrent_info(o.include_files, o.include_trackers).await;
        if torrents.is_empty() {
            tracing::error!("cross-seed [{}]: No torrents found in client", self.info.label);
            return Some((all, new, hashes));
        }
        for t in torrents {
            let info_hash = t.infohash_v1.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| t.hash.clone()).to_lowercase();
            hashes.insert(info_hash.clone());
            let kept = kept_searchee(db, &self.info.client_host, &info_hash);
            let tags: Vec<String> = t.tags.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let modified = client_searchee_modified(kept.as_ref(), &t.name, &t.save_path, Some(&t.category), Some(&tags));
            if !modified && !o.refreshes(&info_hash) {
                if !o.new_searchees_only {
                    all.extend(kept);
                }
                continue;
            }
            let files = match &t.files {
                Some(f) => Some(f.iter().map(torrent_file_to_file).collect()),
                None => self.get_files(&t.hash).await,
            };
            let Some(files) = files else {
                tracing::debug!(
                    "cross-seed [{}]: Failed to get files for {} [{}] (likely transient)",
                    self.info.label,
                    t.name,
                    sanitize_info_hash(&t.hash)
                );
                continue;
            };
            if files.is_empty() {
                tracing::debug!(
                    "cross-seed [{}]: No files found for {} [{}]: skipping",
                    self.info.label,
                    t.name,
                    sanitize_info_hash(&t.hash)
                );
                continue;
            }
            let trackers = match &t.trackers {
                Some(list) => Some(organize_trackers(list.iter().map(|t| t.url.as_str()))),
                None => self.get_trackers(&t.hash).await,
            };
            let Some(trackers) = trackers else {
                tracing::debug!(
                    "cross-seed [{}]: Failed to get trackers for {} [{}] (likely transient)",
                    self.info.label,
                    t.name,
                    sanitize_info_hash(&t.hash)
                );
                continue;
            };
            let title = parse_title(&t.name, &files, None).unwrap_or_else(|| t.name.clone());
            let s = Searchee {
                info_hash: Some(info_hash),
                name: t.name.clone(),
                title,
                files,
                length: t.total_size,
                client_host: Some(self.info.client_host.clone()),
                save_path: Some(t.save_path.clone()),
                category: Some(t.category.clone()),
                tags: Some(tags),
                trackers: Some(trackers),
                ..Searchee::default()
            };
            new.push(s.clone());
            all.push(s);
        }
        Some((all, new, hashes))
    }

    /// `isTorrentInClient`.
    pub async fn is_torrent_in_client(&self, info_hash: &str) -> Result<bool, String> {
        let h = info_hash.to_lowercase();
        let torrents = self.get_all_torrent_info(false, false).await;
        if torrents.is_empty() {
            return Err("No torrents found".into());
        }
        Ok(torrents.iter().any(|t| {
            t.hash.to_lowercase() == h
                || t.infohash_v1.as_deref().map(str::to_lowercase).as_deref() == Some(h.as_str())
                || t.infohash_v2.as_deref().map(str::to_lowercase).as_deref() == Some(h.as_str())
        }))
    }

    /// `isTorrentComplete`.
    pub async fn is_torrent_complete(&self, info_hash: &str) -> Option<bool> {
        self.get_torrent_info(Some(info_hash), 0).await.map(|t| is_torrent_info_complete(&t))
    }

    /// `isTorrentChecking`.
    pub async fn is_torrent_checking(&self, info_hash: &str) -> Option<bool> {
        self.get_torrent_info(Some(info_hash), 0).await.map(|t| ["checkingDL", "checkingUP"].contains(&t.state.as_str()))
    }

    /// `resumeInjection`.
    pub async fn resume_injection(&self, meta: &Metafile, decision: Decision, check_once: bool, cfg: &RuntimeConfig, timing: ResumeTiming) {
        let info_hash = &meta.info_hash;
        let mut sleep = timing.sleep;
        let stop_at = Instant::now() + timing.stop_after;
        let mut stop = false;
        while Instant::now() < stop_at {
            if check_once {
                if stop {
                    return;
                }
                stop = true;
            }
            tokio::time::sleep(sleep).await;
            let Some(t) = self.get_torrent_info(Some(info_hash), 0).await else {
                sleep = timing.err_sleep;
                continue;
            };
            if ["checkingDL", "checkingUP"].contains(&t.state.as_str()) {
                continue;
            }
            let torrent_log = format!("{} [{}]", t.name, sanitize_info_hash(info_hash));
            if !["pausedDL", "stoppedDL", "pausedUP", "stoppedUP"].contains(&t.state.as_str()) {
                tracing::warn!("cross-seed [{}]: Will not resume {torrent_log}: state is {}", self.info.label, t.state);
                return;
            }
            if !may_resume(&self.info.label, &torrent_log, meta, decision, t.amount_left, cfg) {
                return;
            }
            tracing::info!(
                "cross-seed [{}]: Resuming {torrent_log}: {} remaining",
                self.info.label,
                human_readable_size(t.amount_left as f64, true)
            );
            let start = if self.major() >= 5 { "/torrents/start" } else { "/torrents/resume" };
            self.request(start, Self::form(&[("hashes", info_hash)]), 3).await;
            return;
        }
        tracing::warn!("cross-seed [{}]: Will not resume torrent {info_hash}: timeout", self.info.label);
    }

    /// `getCategoryForNewTorrent`.
    async fn get_category_for_new_torrent(&self, category: &str, save_path: &str, auto_tmm: bool, cfg: &RuntimeConfig) -> String {
        if !cfg.duplicate_categories || category.is_empty() || Some(category) == cfg.link_category.as_deref() {
            return category.to_string();
        }
        let dupe =
            if category.ends_with(TORRENT_CATEGORY_SUFFIX) { category.to_string() } else { format!("{category}{TORRENT_CATEGORY_SUFFIX}") };
        if !auto_tmm {
            return dupe;
        }
        let categories = self.get_all_categories().await;
        match categories.iter().find(|c| c.name == dupe) {
            None => self.create_category(&dupe, save_path).await,
            Some(c) if c.save_path != save_path => self.edit_category(&dupe, save_path).await,
            Some(_) => {}
        }
        dupe
    }

    /// `getTagsForNewTorrent`.
    fn get_tags_for_new_torrent(searchee_info: Option<&TorrentInfo>, destination_dir: Option<&str>, cfg: &RuntimeConfig) -> String {
        let (Some(info), Some(_), true) = (searchee_info, destination_dir, cfg.duplicate_categories) else {
            return TORRENT_TAG.to_string();
        };
        let c = &info.category;
        if c.is_empty() || Some(c.as_str()) == cfg.link_category.as_deref() {
            return TORRENT_TAG.to_string();
        }
        if c.ends_with(TORRENT_CATEGORY_SUFFIX) {
            return format!("{TORRENT_TAG},{c}");
        }
        format!("{TORRENT_TAG},{c}{TORRENT_CATEGORY_SUFFIX}")
    }

    /// `inject`.
    pub async fn inject(
        &self,
        meta: &Metafile,
        searchee: &Searchee,
        decision: Decision,
        only_completed: bool,
        destination_dir: Option<&str>,
        cfg: &RuntimeConfig,
    ) -> Injected {
        match self.inject_inner(meta, searchee, decision, only_completed, destination_dir, cfg).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    "cross-seed [{}]: Injection failed for {} [{}]: {e}",
                    self.info.label,
                    meta.name,
                    sanitize_info_hash(&meta.info_hash)
                );
                (InjectionResult::Failure, false)
            }
        }
    }

    /// The body of [`inject`](Self::inject), whose errors are logged as a failure.
    async fn inject_inner(
        &self,
        meta: &Metafile,
        searchee: &Searchee,
        decision: Decision,
        only_completed: bool,
        destination_dir: Option<&str>,
        cfg: &RuntimeConfig,
    ) -> Result<Injected, String> {
        match self.is_torrent_in_client(&meta.info_hash).await {
            Err(_) => return Ok((InjectionResult::Failure, false)),
            Ok(true) => return Ok((InjectionResult::AlreadyExists, false)),
            Ok(false) => {}
        }
        let searchee_info = self.get_torrent_info(searchee.info_hash.as_deref(), 0).await;
        if searchee_info.is_none() {
            if destination_dir.is_none() {
                return Err(format!("Searchee torrent may have been deleted: {}", searchee.name));
            } else if searchee.info_hash.is_some() {
                tracing::warn!(
                    "cross-seed [{}]: Searchee torrent may have been deleted, tagging may not meet expectations: {}",
                    self.info.label,
                    searchee.name
                );
            }
        }
        let (save_path, is_complete, auto_tmm, category) = match (destination_dir, &searchee_info) {
            (Some(d), _) => (d.to_string(), true, false, cfg.link_category.clone()),
            (None, Some(i)) => (i.save_path.clone(), is_torrent_info_complete(i), i.auto_tmm, Some(i.category.clone())),
            (None, None) => return Err(format!("Searchee torrent may have been deleted: {}", searchee.name)),
        };
        if only_completed && !is_complete {
            return Ok((InjectionResult::TorrentNotComplete, false));
        }
        let filename = format!("{}.{TORRENT_TAG}.torrent", meta.file_system_safe_name());
        let to_recheck = should_recheck(meta, decision, cfg);
        let mut parts: Vec<(String, Vec<u8>, Option<String>)> = vec![("torrents".into(), meta.raw.clone(), Some(filename))];
        let mut text = |k: &str, v: String| parts.push((k.to_string(), v.into_bytes(), None));
        if !auto_tmm {
            text("downloadPath", save_path.clone());
            text("savepath", save_path.clone());
        }
        text("autoTMM", auto_tmm.to_string());
        if let Some(c) = category.as_deref().filter(|c| !c.is_empty()) {
            let c = self.get_category_for_new_torrent(c, &save_path, auto_tmm, cfg).await;
            parts.push(("category".into(), c.into_bytes(), None));
        }
        let mut text = |k: &str, v: String| parts.push((k.to_string(), v.into_bytes(), None));
        text("tags", Self::get_tags_for_new_torrent(searchee_info.as_ref(), destination_dir, cfg));
        // getLayoutForNewTorrent
        let layout = match (destination_dir, &searchee_info) {
            (None, Some(i)) if Self::is_subfolder_content_layout(&searchee.files, i, cfg) => "Subfolder",
            _ => "Original",
        };
        text("contentLayout", layout.into());
        text("skip_checking", (!to_recheck).to_string());
        text(if self.major() >= 5 { "stopped" } else { "paused" }, to_recheck.to_string());
        // cross-seed's guard: some parsers join the last pair's value with the boundary
        text("foo", "bar".into());
        if self.request("/torrents/add", Body::Multipart(parts), 3).await.is_none() {
            tracing::error!("cross-seed [{}]: Failed to add torrent (polling client to confirm)", self.info.label);
        }
        let Some(new_info) = self.get_torrent_info(Some(&meta.info_hash), 5).await else {
            return Err("Failed to retrieve torrent after adding".into());
        };
        if to_recheck {
            self.recheck_torrent(&new_info.hash).await;
        }
        Ok((InjectionResult::Success, to_recheck))
    }
}
