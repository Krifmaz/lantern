// Ported from cross-seed v6.13.9 (src/clients/Transmission.ts), Apache-2.0.
//! Transmission through its RPC, as cross-seed talks to it: the `X-Transmission-Session-Id` handshake (a 409
//! hands out the id), basic sign-in, `torrent-get` for what it needs and `torrent-add` with the folder, the
//! `.torrent` in base64, paused when it must be rechecked, and the label `cross-seed`.

use super::{
    client_searchee_modified, http, kept_searchee, may_resume, organize_trackers, should_recheck, DirError, Info, Injected, ResumeTiming,
    SearcheeOptions, TorrentInClient,
};
use crate::crossseed::config::RuntimeConfig;
use crate::crossseed::constants::{Decision, InjectionResult, TORRENT_TAG};
use crate::crossseed::searchee::{basename, parse_title, File, Searchee};
use crate::crossseed::torrent::Metafile;
use crate::crossseed::utils::{human_readable_size, sanitize_info_hash};
use base64::Engine;
use kit_server_core::SqliteDb;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One Transmission.
#[derive(Clone)]
pub struct Transmission {
    /// The client's host, priority and label.
    info: Info,
    /// The RPC address with its user name and password.
    url: String,
    /// The session id Transmission handed out.
    session: Arc<Mutex<String>>,
    /// The HTTP client.
    http: reqwest::Client,
}

/// A `torrent-get` torrent's field `k` as a number.
fn num(t: &Value, k: &str) -> f64 {
    t.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A `torrent-get` torrent's field `k` as text.
fn text(t: &Value, k: &str) -> String {
    t.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

impl Transmission {
    /// The client for `url`.
    pub fn new(url: &str, info: &Info) -> Transmission {
        Transmission { info: info.clone(), url: url.to_string(), session: Arc::new(Mutex::new(String::new())), http: http() }
    }

    /// `request`: calls `method` with `args`; its `arguments`, or why not. `timeout` 0 is cross-seed's 5 minutes.
    pub fn request<'a>(
        &'a self,
        method: &'a str,
        args: Value,
        retries: i32,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(self.request_inner(method, args, retries, timeout))
    }

    /// The body of [`request`](Self::request).
    async fn request_inner(&self, method: &str, args: Value, retries: i32, timeout: Duration) -> Result<Value, String> {
        let msg = format!("Calling method {method} with params {args}");
        tracing::debug!(
            "cross-seed [{}]: {}",
            self.info.label,
            if msg.len() > 1000 { format!("{}...", msg.chars().take(1000).collect::<String>()) } else { msg }
        );
        let creds = super::extract_credentials_from_url(&self.url, None)
            .ok_or_else(|| format!("[{}] Transmission rpc url must be percent-encoded", self.info.label))?;
        let session = self.session.lock().map(|s| s.clone()).unwrap_or_default();
        let mut req = self
            .http
            .post(&creds.href)
            .header("Content-Type", "application/json")
            .timeout(if timeout.is_zero() { Duration::from_secs(300) } else { timeout })
            .body(json!({ "method": method, "arguments": args }).to_string());
        if !session.is_empty() {
            req = req.header("X-Transmission-Session-Id", session);
        }
        if !creds.username.is_empty() && !creds.password.is_empty() {
            req = req.basic_auth(&creds.username, Some(&creds.password));
        }
        let r = req.send().await.map_err(|e| e.to_string())?;
        if r.status().as_u16() == 409 {
            let id = r.headers().get("X-Transmission-Session-Id").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            if let Ok(mut s) = self.session.lock() {
                *s = id;
            }
            if retries < 0 {
                return Err("Transmission kept asking for a new session id".into());
            }
            return self.request(method, args, retries - 1, timeout).await;
        }
        let body = r.text().await.unwrap_or_default();
        let v: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => {
                tracing::error!("cross-seed [{}]: Transmission returned non-JSON response", self.info.label);
                tracing::debug!("cross-seed [{}]: {body}", self.info.label);
                return Err("Transmission returned non-JSON response".into());
            }
        };
        let result = v.get("result").and_then(Value::as_str).unwrap_or("");
        if result == "success" || result == "duplicate torrent" {
            return Ok(v.get("arguments").cloned().unwrap_or(Value::Null));
        }
        let e = format!("Transmission responded with error: \"{result}\"");
        tracing::error!("cross-seed [{}]: Transmission responded with an error: {e}", self.info.label);
        Err(e)
    }

    /// [`request`](Self::request) with cross-seed's defaults (one retry, 5 minutes).
    async fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        self.request(method, args, 1, Duration::ZERO).await
    }

    /// `torrent-get`'s torrents.
    async fn torrents(&self, args: Value) -> Result<Vec<Value>, String> {
        Ok(self.call("torrent-get", args).await?.get("torrents").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    /// `validateConfig`.
    pub async fn validate_config(&self) -> Result<(), String> {
        self.request("session-get", json!({}), 1, Duration::from_secs(10))
            .await
            .map_err(|e| format!("[{}] Failed to reach Transmission at {}: {e}", self.info.label, self.info.client_host))?;
        tracing::info!("cross-seed [{}]: Logged in successfully{}", self.info.label, if self.info.readonly { " (readonly)" } else { "" });
        Ok(())
    }

    /// `checkOriginalTorrent` / `getDownloadDir`.
    pub async fn get_download_dir(&self, info_hash: &str, only_completed: bool) -> Result<String, DirError> {
        let list = self
            .torrents(json!({ "fields": ["downloadDir", "percentDone"], "ids": [info_hash] }))
            .await
            .map_err(|_| DirError::UnknownError)?;
        let Some(t) = list.first() else { return Err(DirError::UnknownError) };
        if only_completed && num(t, "percentDone") < 1.0 {
            return Err(DirError::TorrentNotComplete);
        }
        Ok(text(t, "downloadDir"))
    }

    /// `getAllDownloadDirs`.
    pub async fn get_all_download_dirs(&self, only_completed: bool) -> HashMap<String, String> {
        let list = self.torrents(json!({ "fields": ["hashString", "downloadDir", "percentDone"] })).await.unwrap_or_default();
        list.iter()
            .filter(|t| !only_completed || num(t, "percentDone") == 1.0)
            .map(|t| (text(t, "hashString"), text(t, "downloadDir")))
            .collect()
    }

    /// `isTorrentInClient`.
    pub async fn is_torrent_in_client(&self, info_hash: &str) -> Result<bool, String> {
        let h = info_hash.to_lowercase();
        let list = self.torrents(json!({ "fields": ["hashString"] })).await?;
        if list.is_empty() {
            return Err("No torrents found".into());
        }
        Ok(list.iter().any(|t| text(t, "hashString").to_lowercase() == h))
    }

    /// `isTorrentComplete`.
    pub async fn is_torrent_complete(&self, info_hash: &str) -> Option<bool> {
        let list = self.torrents(json!({ "fields": ["percentDone"], "ids": [info_hash] })).await.ok()?;
        list.first().map(|t| num(t, "percentDone") == 1.0)
    }

    /// `isTorrentChecking`.
    pub async fn is_torrent_checking(&self, info_hash: &str) -> Option<bool> {
        let list = self.torrents(json!({ "fields": ["status"], "ids": [info_hash] })).await.ok()?;
        list.first().map(|t| [1.0, 2.0].contains(&num(t, "status")))
    }

    /// `getAllTorrents`.
    pub async fn get_all_torrents(&self) -> Vec<TorrentInClient> {
        let list = self.torrents(json!({ "fields": ["hashString", "labels"] })).await.unwrap_or_default();
        list.iter()
            .map(|t| TorrentInClient { info_hash: text(t, "hashString"), tags: Some(labels(t)), ..TorrentInClient::default() })
            .collect()
    }

    /// `getClientSearchees`' listing.
    pub async fn list_torrents(&self, db: &SqliteDb, o: &SearcheeOptions) -> Option<(Vec<Searchee>, Vec<Searchee>, HashSet<String>)> {
        let (mut all, mut new, mut hashes) = (Vec::new(), Vec::new(), HashSet::new());
        let fields = json!({ "fields": ["hashString", "name", "files", "totalSize", "downloadDir", "labels", "trackers"] });
        let torrents = match self.torrents(fields).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("cross-seed [{}]: Failed to get torrents from client: {e}", self.info.label);
                return Some((all, new, hashes));
            }
        };
        if torrents.is_empty() {
            tracing::error!("cross-seed [{}]: No torrents found in client", self.info.label);
            return Some((all, new, hashes));
        }
        for t in &torrents {
            let info_hash = text(t, "hashString").to_lowercase();
            hashes.insert(info_hash.clone());
            let kept = kept_searchee(db, &self.info.client_host, &info_hash);
            let (name, save_path, tags) = (text(t, "name"), text(t, "downloadDir"), labels(t));
            if !client_searchee_modified(kept.as_ref(), &name, &save_path, None, Some(&tags)) && !o.refreshes(&info_hash) {
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
                        .map(|f| File { name: basename(&text(f, "name")), path: text(f, "name"), length: num(f, "length") as i64 })
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
            let announces: Vec<String> =
                t.get("trackers").and_then(Value::as_array).map(|ts| ts.iter().map(|x| text(x, "announce")).collect()).unwrap_or_default();
            let trackers = organize_trackers(announces.iter().map(String::as_str));
            let title = parse_title(&name, &files, None).unwrap_or_else(|| name.clone());
            let s = Searchee {
                info_hash: Some(info_hash),
                name,
                title,
                files,
                length: num(t, "totalSize") as i64,
                client_host: Some(self.info.client_host.clone()),
                save_path: Some(save_path),
                tags: Some(tags),
                trackers: Some(trackers),
                ..Searchee::default()
            };
            new.push(s.clone());
            all.push(s);
        }
        Some((all, new, hashes))
    }

    /// `recheckTorrent`.
    pub async fn recheck_torrent(&self, info_hash: &str) {
        let _ = self.call("torrent-stop", json!({ "ids": [info_hash] })).await;
        let _ = self.call("torrent-verify", json!({ "ids": [info_hash] })).await;
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
            let list =
                self.torrents(json!({ "fields": ["leftUntilDone", "name", "status"], "ids": [info_hash] })).await.unwrap_or_default();
            let Some(t) = list.first() else {
                sleep = timing.err_sleep;
                continue;
            };
            let status = num(t, "status");
            if status == 1.0 || status == 2.0 {
                continue;
            }
            let torrent_log = format!("{} [{}]", text(t, "name"), sanitize_info_hash(info_hash));
            if status != 0.0 {
                tracing::warn!("cross-seed [{}]: Will not resume {torrent_log}: status is {status}", self.info.label);
                return;
            }
            let left = num(t, "leftUntilDone") as i64;
            if !may_resume(&self.info.label, &torrent_log, meta, decision, left, cfg) {
                return;
            }
            tracing::info!(
                "cross-seed [{}]: Resuming {torrent_log}: {} remaining",
                self.info.label,
                human_readable_size(left as f64, true)
            );
            let _ = self.call("torrent-start", json!({ "ids": [info_hash] })).await;
            return;
        }
        tracing::warn!("cross-seed [{}]: Will not resume torrent {info_hash}: timeout", self.info.label);
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
        match self.is_torrent_in_client(&meta.info_hash).await {
            Err(_) => return (InjectionResult::Failure, false),
            Ok(true) => return (InjectionResult::AlreadyExists, false),
            Ok(false) => {}
        }
        let dir = match destination_dir {
            Some(d) => d.to_string(),
            None => match self.get_download_dir(searchee.info_hash.as_deref().unwrap_or(""), only_completed).await {
                Ok(d) => d,
                Err(DirError::TorrentNotComplete) => return (InjectionResult::TorrentNotComplete, false),
                Err(_) => return (InjectionResult::Failure, false),
            },
        };
        let to_recheck = should_recheck(meta, decision, cfg);
        let args = json!({
            "download-dir": dir,
            "metainfo": base64::engine::general_purpose::STANDARD.encode(&meta.raw),
            "paused": to_recheck,
            "labels": [TORRENT_TAG],
        });
        match self.call("torrent-add", args).await {
            Err(_) => (InjectionResult::Failure, false),
            Ok(a) if a.get("torrent-duplicate").is_some() => (InjectionResult::AlreadyExists, to_recheck),
            Ok(_) => (InjectionResult::Success, to_recheck),
        }
    }
}

/// A torrent's labels.
fn labels(t: &Value) -> Vec<String> {
    t.get("labels").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
}
