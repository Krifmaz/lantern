// Ported from cross-seed v6.13.9 (src/clients/Deluge.ts), Apache-2.0.
//! Deluge through its Web UI's JSON-RPC, as cross-seed talks to it: `auth.login` with the password (the session
//! cookie kept from `Set-Cookie`), `web.connected` and `web.connect` when the Web UI lost its daemon,
//! `web.update_ui` for torrents, and `core.add_torrent_file` (paused for a recheck, else in seed mode), labelled
//! through the Label plugin when it is on.

use super::{
    client_searchee_modified, http, kept_searchee, may_resume, organize_trackers, should_recheck, DirError, Info, Injected, ResumeTiming,
    SearcheeOptions, TorrentInClient,
};
use crate::crossseed::config::RuntimeConfig;
use crate::crossseed::constants::{Decision, InjectionResult, TORRENT_CATEGORY_SUFFIX, TORRENT_TAG};
use crate::crossseed::searchee::{basename, parse_title, File, Searchee};
use crate::crossseed::torrent::Metafile;
use crate::crossseed::utils::{human_readable_size, sanitize_info_hash};
use base64::Engine;
use kit_server_core::SqliteDb;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What one call gives: Deluge's answer or its own error inside, a failure to reach Deluge outside.
pub type CallResult = Result<Result<Value, DelugeError>, String>;

/// `DelugeErrorCode.NO_AUTH`.
const NO_AUTH: i64 = 1;

/// A Deluge error: its message and code.
#[derive(Debug, Clone, PartialEq)]
pub struct DelugeError {
    /// What Deluge said.
    pub message: String,
    /// Its code (1: not signed in).
    pub code: Option<i64>,
}

/// `TorrentInfo` as `getTorrentInfo` gives it.
#[derive(Debug, Clone, Default)]
struct TorrentInfo {
    /// Its name.
    name: String,
    /// Whether every file is there.
    complete: bool,
    /// Its state (`Seeding`, `Paused`, `Checking`, …).
    state: String,
    /// Its folder.
    save_path: String,
    /// Its label, when the Label plugin is on and it has one.
    label: Option<String>,
    /// Bytes still missing.
    total_remaining: i64,
}

/// One Deluge.
#[derive(Clone)]
pub struct Deluge {
    /// The client's host, priority and label.
    info: Info,
    /// The Web UI's JSON address with the password.
    url: String,
    /// The session cookie.
    cookie: Arc<Mutex<Option<String>>>,
    /// Whether the Label plugin is on.
    label_enabled: Arc<AtomicBool>,
    /// The next request id.
    request_id: Arc<AtomicU64>,
    /// The HTTP client.
    http: reqwest::Client,
}

/// A number field.
fn num(t: &Value, k: &str) -> f64 {
    t.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A text field.
fn text(t: &Value, k: &str) -> String {
    t.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

impl Deluge {
    /// The client for `url`.
    pub fn new(url: &str, info: &Info) -> Deluge {
        Deluge {
            info: info.clone(),
            url: url.to_string(),
            cookie: Arc::new(Mutex::new(None)),
            label_enabled: Arc::new(AtomicBool::new(false)),
            request_id: Arc::new(AtomicU64::new(0)),
            http: http(),
        }
    }

    /// `validateConfig`: signs in and finds out whether the Label plugin is on.
    pub async fn validate_config(&self) -> Result<(), String> {
        self.authenticate().await?;
        let enabled = self.label_enabled().await;
        self.label_enabled.store(enabled, Ordering::Relaxed);
        tracing::info!("cross-seed [{}]: Logged in successfully{}", self.info.label, if self.info.readonly { " (readonly)" } else { "" });
        Ok(())
    }

    /// `authenticate`: signs in to the Web UI and connects it to its daemon when it isn't.
    fn authenticate(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(self.authenticate_inner())
    }

    /// The body of [`authenticate`](Self::authenticate).
    async fn authenticate_inner(&self) -> Result<(), String> {
        let creds = super::extract_credentials_from_url(&self.url, None)
            .ok_or_else(|| format!("[{}] delugeRpcUrl must be percent-encoded", self.info.label))?;
        if creds.password.is_empty() {
            return Err(format!(
                "[{}] You need to define a password in the delugeRpcUrl. (e.g. http://:<PASSWORD>@localhost:8112)",
                self.info.label
            ));
        }
        let ok = self
            .call("auth.login", json!([creds.password]), 0)
            .await?
            .map_err(|_| format!("[{}] failed to connect for authentication", self.info.label))?;
        if ok != Value::Bool(true) {
            return Err(format!("[{}] Reached Deluge, but failed to authenticate: {}", self.info.label, creds.href));
        }
        if let Ok(Ok(Value::Bool(false))) = self.call("web.connected", json!([]), 0).await {
            tracing::warn!("cross-seed [{}]: Deluge WebUI disconnected from daemon...attempting to reconnect.", self.info.label);
            let hosts = self
                .call("web.get_hosts", json!([]), 0)
                .await?
                .map_err(|_| format!("[{}] failed to get host-list for reconnect", self.info.label))?;
            let host = hosts.get(0).and_then(|h| h.get(0)).cloned().unwrap_or(Value::Null);
            match self.call("web.connect", json!([host]), 0).await {
                Ok(Ok(v)) if !v.is_null() && v != Value::Bool(false) => {
                    tracing::info!("cross-seed [{}]: Deluge WebUI connected to the daemon.", self.info.label);
                }
                _ => {
                    return Err(format!(
                        "[{}] Unable to connect WebUI to Deluge daemon. Connect to the WebUI to resolve this.",
                        self.info.label
                    ))
                }
            }
        }
        Ok(())
    }

    /// `call`: one JSON-RPC call, signing in again once when Deluge says the session is gone. The outer error is
    /// a failure to reach Deluge; the inner one is Deluge's own error.
    pub fn call<'a>(&'a self, method: &'a str, params: Value, retries: u32) -> Pin<Box<dyn Future<Output = CallResult> + Send + 'a>> {
        Box::pin(self.call_inner(method, params, retries))
    }

    /// The body of [`call`](Self::call).
    async fn call_inner(&self, method: &str, params: Value, retries: u32) -> Result<Result<Value, DelugeError>, String> {
        let msg = format!("Calling method {method} with params {params}");
        tracing::debug!("cross-seed [{}]: {}", self.info.label, msg.chars().take(1000).collect::<String>());
        let creds = super::extract_credentials_from_url(&self.url, None)
            .ok_or_else(|| format!("[{}] delugeRpcUrl must be percent-encoded", self.info.label))?;
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let mut req = self
            .http
            .post(&creds.href)
            .header("Content-Type", "application/json")
            .timeout(Duration::from_secs(10))
            .body(json!({ "method": method, "params": params, "id": id }).to_string());
        if let Some(c) = self.cookie.lock().ok().and_then(|c| c.clone()) {
            req = req.header("Cookie", c);
        }
        let r = req.send().await.map_err(|e| {
            if e.is_timeout() {
                format!("[{}] Deluge method {method} timed out after 10 seconds", self.info.label)
            } else {
                format!("[{}] Failed to connect to Deluge at {}", self.info.label, creds.href)
            }
        })?;
        let set_cookie = r.headers().get("Set-Cookie").and_then(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap_or("").to_string());
        let json: Value = r.json().await.map_err(|e| format!("[{}] Deluge method {method} response was non-JSON {e}", self.info.label))?;
        let error = json.get("error").filter(|e| !e.is_null());
        if error.and_then(|e| e.get("code")).and_then(Value::as_i64) == Some(NO_AUTH) && retries > 0 {
            if let Ok(mut c) = self.cookie.lock() {
                *c = None;
            }
            self.authenticate().await?;
            if self.cookie.lock().map(|c| c.is_some()).unwrap_or(false) {
                return self.call(method, params, 0).await;
            }
            return Err(format!("[{}] Connection lost with Deluge. Re-authentication failed.", self.info.label));
        }
        if let Some(c) = set_cookie {
            if let Ok(mut k) = self.cookie.lock() {
                *k = Some(c);
            }
        }
        if let Some(e) = error {
            return Ok(Err(DelugeError { message: text(e, "message"), code: e.get("code").and_then(Value::as_i64) }));
        }
        Ok(Ok(json.get("result").cloned().unwrap_or(Value::Null)))
    }

    /// `labelEnabled`: whether the Label plugin is on.
    async fn label_enabled(&self) -> bool {
        match self.call("core.get_enabled_plugins", json!([]), 1).await {
            Ok(Ok(v)) => v.as_array().is_some_and(|l| l.iter().any(|p| p == "Label")) || v.as_str().is_some_and(|s| s.contains("Label")),
            _ => false,
        }
    }

    /// `web.update_ui` with `fields` and `filter`: the torrents by hash, or why not.
    async fn update_ui(&self, fields: Value, filter: Value) -> Result<Option<Map<String, Value>>, String> {
        match self.call("web.update_ui", json!([fields, filter]), 1).await? {
            Ok(v) => Ok(v.get("torrents").and_then(Value::as_object).cloned()),
            Err(e) => Err(format!("{}{}", e.code.map(|c| format!("{c}: ")).unwrap_or_default(), e.message)),
        }
    }

    /// `getTorrentInfo`.
    async fn get_torrent_info(&self, info_hash: &str, verbose: bool) -> Result<TorrentInfo, String> {
        let fields = json!(["name", "state", "progress", "save_path", "label", "total_remaining"]);
        let res = async {
            let torrents = self
                .update_ui(fields, json!({ "hash": info_hash }))
                .await
                .map_err(|_| "failed to fetch the torrent list".to_string())?
                .ok_or_else(|| "Client returned unexpected response (object missing)".to_string())?;
            let t = torrents.get(info_hash).ok_or_else(|| format!("Torrent not found in client ({info_hash})"))?;
            let (state, progress, remaining) = (text(t, "state"), num(t, "progress"), num(t, "total_remaining"));
            let complete = (state == "Paused" && (progress == 100.0 || remaining == 0.0))
                || state == "Seeding"
                || progress == 100.0
                || remaining == 0.0;
            let label = text(t, "label");
            Ok::<_, String>(TorrentInfo {
                name: text(t, "name"),
                complete,
                state,
                save_path: text(t, "save_path"),
                label: if self.label_enabled.load(Ordering::Relaxed) && !label.is_empty() { Some(label) } else { None },
                total_remaining: remaining as i64,
            })
        }
        .await;
        res.map_err(|e| {
            if verbose {
                tracing::debug!("cross-seed [{}]: Failed to fetch torrent data for {info_hash}: {e}", self.info.label);
            } else {
                tracing::error!("cross-seed [{}]: Failed to fetch torrent data for {info_hash}: {e}", self.info.label);
            }
            format!("[{}] web.update_ui: failed to fetch data from client", self.info.label)
        })
    }

    /// `getDownloadDir`.
    pub async fn get_download_dir(&self, info_hash: &str, only_completed: bool) -> Result<String, DirError> {
        let torrents = self
            .update_ui(json!(["save_path", "progress"]), json!({ "hash": info_hash }))
            .await
            .map_err(|_| DirError::UnknownError)?
            .ok_or(DirError::UnknownError)?;
        let t = torrents.get(info_hash).ok_or(DirError::NotFound)?;
        if only_completed && num(t, "progress") != 100.0 {
            return Err(DirError::TorrentNotComplete);
        }
        Ok(text(t, "save_path"))
    }

    /// `getAllDownloadDirs`.
    pub async fn get_all_download_dirs(&self, only_completed: bool) -> HashMap<String, String> {
        let Ok(Some(torrents)) = self.update_ui(json!(["save_path", "progress"]), json!({})).await else { return HashMap::new() };
        torrents
            .iter()
            .filter(|(_, t)| !only_completed || num(t, "progress") == 100.0)
            .map(|(h, t)| (h.clone(), text(t, "save_path")))
            .collect()
    }

    /// `isTorrentInClient`.
    pub async fn is_torrent_in_client(&self, info_hash: &str) -> Result<bool, String> {
        let h = info_hash.to_lowercase();
        let torrents = self.update_ui(json!([]), json!({})).await?.ok_or_else(|| "No torrents found".to_string())?;
        Ok(torrents.keys().any(|k| k.to_lowercase() == h))
    }

    /// `isTorrentComplete`.
    pub async fn is_torrent_complete(&self, info_hash: &str) -> Option<bool> {
        self.get_torrent_info(info_hash, true).await.ok().map(|t| t.complete)
    }

    /// `isTorrentChecking`.
    pub async fn is_torrent_checking(&self, info_hash: &str) -> Option<bool> {
        self.get_torrent_info(info_hash, true).await.ok().map(|t| t.state == "Checking")
    }

    /// `getAllTorrents`.
    pub async fn get_all_torrents(&self) -> Vec<TorrentInClient> {
        let Ok(Some(torrents)) = self.update_ui(json!(["hash", "label"]), json!({})).await else { return Vec::new() };
        torrents
            .iter()
            .map(|(h, t)| TorrentInClient { info_hash: h.clone(), category: Some(text(t, "label")), ..TorrentInClient::default() })
            .collect()
    }

    /// `getClientSearchees`' listing.
    pub async fn list_torrents(&self, db: &SqliteDb, o: &SearcheeOptions) -> Option<(Vec<Searchee>, Vec<Searchee>, HashSet<String>)> {
        let (mut all, mut new, mut hashes) = (Vec::new(), Vec::new(), HashSet::new());
        let fields = json!(["name", "label", "save_path", "total_size", "files", "trackers"]);
        let torrents = match self.update_ui(fields, json!({})).await {
            Ok(Some(t)) if !t.is_empty() => t,
            Ok(_) => {
                tracing::debug!("cross-seed [{}]: No torrents found in client", self.info.label);
                return Some((all, new, hashes));
            }
            Err(e) => {
                tracing::error!("cross-seed [{}]: Failed to get torrents from client", self.info.label);
                tracing::debug!("cross-seed [{}]: {e}", self.info.label);
                return Some((all, new, hashes));
            }
        };
        for (hash, t) in &torrents {
            let info_hash = hash.to_lowercase();
            hashes.insert(info_hash.clone());
            let kept = kept_searchee(db, &self.info.client_host, &info_hash);
            let (name, save_path, category) = (text(t, "name"), text(t, "save_path"), text(t, "label"));
            if !client_searchee_modified(kept.as_ref(), &name, &save_path, Some(&category), None) && !o.refreshes(&info_hash) {
                if !o.new_searchees_only {
                    all.extend(kept);
                }
                continue;
            }
            let files: Vec<File> = t
                .get("files")
                .and_then(Value::as_array)
                .map(|fs| {
                    fs.iter()
                        .map(|f| File { name: basename(&text(f, "path")), path: text(f, "path"), length: num(f, "size") as i64 })
                        .collect()
                })
                .unwrap_or_default();
            if files.is_empty() {
                tracing::debug!(
                    "cross-seed [{}]: No files found for {name} [{}]: skipping",
                    self.info.label,
                    sanitize_info_hash(&info_hash)
                );
                continue;
            }
            let urls: Vec<String> =
                t.get("trackers").and_then(Value::as_array).map(|ts| ts.iter().map(|x| text(x, "url")).collect()).unwrap_or_default();
            let title = parse_title(&name, &files, None).unwrap_or_else(|| name.clone());
            let s = Searchee {
                info_hash: Some(info_hash),
                name,
                title,
                files,
                length: num(t, "total_size") as i64,
                client_host: Some(self.info.client_host.clone()),
                save_path: Some(save_path),
                category: Some(category),
                trackers: Some(organize_trackers(urls.iter().map(String::as_str))),
                ..Searchee::default()
            };
            new.push(s.clone());
            all.push(s);
        }
        Some((all, new, hashes))
    }

    /// `recheckTorrent`.
    pub async fn recheck_torrent(&self, info_hash: &str) {
        let _ = self.call("core.pause_torrent", json!([[info_hash]]), 1).await;
        let _ = self.call("core.force_recheck", json!([[info_hash]]), 1).await;
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
            let Ok(t) = self.get_torrent_info(info_hash, false).await else {
                sleep = timing.err_sleep;
                continue;
            };
            if t.state == "Checking" {
                continue;
            }
            let torrent_log = format!("{} [{}]", t.name, sanitize_info_hash(info_hash));
            if t.state != "Paused" {
                tracing::warn!("cross-seed [{}]: Will not resume {torrent_log}: state is {}", self.info.label, t.state);
                return;
            }
            if !may_resume(&self.info.label, &torrent_log, meta, decision, t.total_remaining, cfg) {
                return;
            }
            tracing::info!(
                "cross-seed [{}]: Resuming {torrent_log}: {} remaining",
                self.info.label,
                human_readable_size(t.total_remaining as f64, true)
            );
            let _ = self.call("core.resume_torrent", json!([[info_hash]]), 1).await;
            return;
        }
        tracing::warn!("cross-seed [{}]: Will not resume torrent {info_hash}: timeout", self.info.label);
    }

    /// `calculateLabel`: the label for a match (the searchee's, with `.cross-seed` after it for
    /// `duplicateCategories`; `cross-seed` without one).
    fn calculate_label(searchee: &Searchee, t: Option<&TorrentInfo>, cfg: &RuntimeConfig) -> String {
        let og = match (searchee.info_hash.as_ref(), t.and_then(|t| t.label.as_ref())) {
            (Some(_), Some(l)) => l.clone(),
            _ => return TORRENT_TAG.to_string(),
        };
        if !cfg.duplicate_categories {
            return og;
        }
        let suffix = !og.ends_with(TORRENT_CATEGORY_SUFFIX) && Some(og.as_str()) != cfg.link_category.as_deref();
        if suffix {
            format!("{og}{TORRENT_CATEGORY_SUFFIX}")
        } else {
            og
        }
    }

    /// `setLabel`: labels the match, making the label first when Deluge doesn't have it.
    async fn set_label(&self, meta: &Metafile, label: &str) {
        if !self.label_enabled.load(Ordering::Relaxed) {
            return;
        }
        let res: Result<(), String> = async {
            let labels = match self.call("label.get_labels", json!([]), 1).await? {
                Ok(l) => l,
                Err(_) => {
                    self.label_enabled.store(false, Ordering::Relaxed);
                    return Err("Labels have been disabled.".into());
                }
            };
            if !labels.as_array().is_some_and(|l| l.iter().any(|x| x == label)) {
                let _ = self.call("label.add", json!([label]), 1).await?;
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            match self.call("label.set_torrent", json!([meta.info_hash, label]), 1).await? {
                Ok(_) => Ok(()),
                Err(e) => Err(e.message),
            }
        }
        .await;
        if let Err(e) = res {
            tracing::warn!("cross-seed [{}]: Failed to label {} as {label}: {e}", self.info.label, meta.name);
        }
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
        let res: Result<Injected, String> = async {
            match self.is_torrent_in_client(&meta.info_hash).await {
                Err(_) => return Ok((InjectionResult::Failure, false)),
                Ok(true) => return Ok((InjectionResult::AlreadyExists, false)),
                Ok(false) => {}
            }
            let mut info = None;
            if let (true, Some(h)) = (only_completed, searchee.info_hash.as_deref()) {
                let t = self.get_torrent_info(h, false).await?;
                if !t.complete {
                    return Ok((InjectionResult::TorrentNotComplete, false));
                }
                info = Some(t);
            }
            if destination_dir.is_none() && (searchee.info_hash.is_none() || info.is_none()) {
                tracing::debug!("cross-seed [{}]: Injection failure: {} was missing critical data.", self.info.label, searchee.name);
                return Ok((InjectionResult::Failure, false));
            }
            let file_name = format!("{}.cross-seed.torrent", meta.file_system_safe_name());
            let dir = destination_dir.map(String::from).or_else(|| info.as_ref().map(|t| t.save_path.clone())).unwrap_or_default();
            let to_recheck = should_recheck(meta, decision, cfg);
            let params = json!([
                file_name,
                base64::engine::general_purpose::STANDARD.encode(&meta.raw),
                { "add_paused": to_recheck, "seed_mode": !to_recheck, "download_location": dir }
            ]);
            match self.call("core.add_torrent_file", params, 1).await? {
                Err(e) if e.message.contains("already") => Ok((InjectionResult::AlreadyExists, false)),
                Err(e) => {
                    tracing::debug!("cross-seed [{}]: Injection failed: {}", self.info.label, e.message);
                    Ok((InjectionResult::Failure, false))
                }
                Ok(_) => {
                    self.set_label(meta, &Self::calculate_label(searchee, info.as_ref(), cfg)).await;
                    if to_recheck {
                        // paused, libtorrent doesn't start checking by itself
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        self.recheck_torrent(&meta.info_hash).await;
                    }
                    Ok((InjectionResult::Success, to_recheck))
                }
            }
        }
        .await;
        res.unwrap_or_else(|e| {
            tracing::error!("cross-seed [{}]: Injection failed: {e}", self.info.label);
            (InjectionResult::Failure, false)
        })
    }
}
