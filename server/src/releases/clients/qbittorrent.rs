// Ported from go-qbittorrent v1.19.0 (github.com/autobrr/go-qbittorrent, methods.go, http.go, domain.go), MIT, as
// autobrr v1.90.0 uses it.
//! qBittorrent's Web API v2: signs in with the user name and password (a cookie), or sends the API key as a
//! bearer token; a 403 answer signs in again and tries once more.

use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// Seconds between re-announces when the action says none (go-qbittorrent's `ReannounceInterval`).
pub const REANNOUNCE_INTERVAL: i64 = 7;
/// The most re-announces when the action says none (go-qbittorrent's `ReannounceMaxAttempts`).
pub const REANNOUNCE_MAX_ATTEMPTS: i64 = 50;

/// A connection's settings (go-qbittorrent's `Config`).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The address (`http://qbittorrent:8080`).
    pub host: String,
    /// The user name.
    pub username: String,
    /// The password.
    pub password: String,
    /// The API key (qBittorrent 5.2), sent instead of signing in.
    pub api_key: String,
    /// Accept any certificate.
    pub tls_skip_verify: bool,
    /// Basic auth user (a password protected page in front).
    pub basic_user: String,
    /// Basic auth password.
    pub basic_pass: String,
}

/// A qBittorrent client.
pub struct Client {
    cfg: Config,
    http: reqwest::Client,
}

/// The re-announce settings (go-qbittorrent's `ReannounceOptions`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ReannounceOptions {
    /// Seconds between tries (0: the default).
    pub interval: i64,
    /// The most tries (0: the default).
    pub max_attempts: i64,
    /// Remove the torrent when they all fail.
    pub delete_on_failure: bool,
}

/// What the re-announce came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Reannounce {
    /// A tracker answers.
    Ok,
    /// They never answered and the torrent was removed (go-qbittorrent's `ErrReannounceTookTooLong`).
    TookTooLong,
}

/// Whether a tracker message says the torrent isn't on the tracker (yet).
pub fn is_unregistered(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["unregistered", "not registered", "not found", "not exist"].iter().any(|w| m.contains(w))
}

/// Whether one of the trackers works (go-qbittorrent's `isTrackerStatusOK`): disabled ones are skipped, an
/// unregistered message means no.
pub fn is_tracker_status_ok(trackers: &[Value]) -> bool {
    for t in trackers {
        let status = t.get("status").and_then(Value::as_i64).unwrap_or(0);
        if status == 0 {
            continue;
        }
        if is_unregistered(t.get("msg").and_then(Value::as_str).unwrap_or("")) {
            return false;
        }
        if status == 2 {
            return true;
        }
    }
    false
}

impl Client {
    /// A client for `cfg`.
    pub fn new(cfg: Config) -> Client {
        Client { http: super::http(Duration::from_secs(60), cfg.tls_skip_verify, true), cfg }
    }

    /// The address of `endpoint`.
    fn url(&self, endpoint: &str) -> String {
        format!("{}/api/v2/{endpoint}", self.cfg.host.trim_end_matches('/'))
    }

    /// Adds the API key, basic auth and the Referer qBittorrent wants.
    fn prepare(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut r = r.header("Referer", &self.cfg.host).header("User-Agent", super::user_agent());
        if !self.cfg.api_key.is_empty() {
            r = r.header("Authorization", format!("Bearer {}", self.cfg.api_key));
        } else if !self.cfg.basic_user.is_empty() {
            r = r.basic_auth(&self.cfg.basic_user, Some(&self.cfg.basic_pass));
        }
        r
    }

    /// Signs in (nothing to do with an API key or without a user name and password).
    pub async fn login(&self) -> Result<(), String> {
        if !self.cfg.api_key.is_empty() || (self.cfg.username.is_empty() && self.cfg.password.is_empty()) {
            return Ok(());
        }
        let form = [("username", self.cfg.username.as_str()), ("password", self.cfg.password.as_str())];
        let resp = self
            .prepare(self.http.post(self.url("auth/login")))
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("login error: {}", super::req_err("Post", &self.url("auth/login"), &e)))?;
        match resp.status().as_u16() {
            403 => Err("User's IP is banned for too many failed login attempts".into()),
            200 | 204 => {
                let body = resp.text().await.unwrap_or_default();
                if body.trim() == "Fails." {
                    return Err("login error: bad credentials".into());
                }
                Ok(())
            }
            s => Err(format!("login error; status code: {s}: unexpected status")),
        }
    }

    /// Sends a request built by `make`, signing in again once on a 403.
    async fn send(&self, make: impl Fn() -> reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response, String> {
        let resp = self.prepare(make()).send().await.map_err(|e| format!("{what}: {}", e.without_url()))?;
        if resp.status().as_u16() == 403 && self.cfg.api_key.is_empty() {
            self.login().await.map_err(|e| format!("qbit re-login failed: {e}"))?;
            return self.prepare(make()).send().await.map_err(|e| format!("{what}: {}", e.without_url()));
        }
        if resp.status().as_u16() >= 500 {
            return Err(format!("{what}: unrecoverable status: {}", resp.status().as_u16()));
        }
        Ok(resp)
    }

    /// POSTs a form to `endpoint`.
    async fn post(&self, endpoint: &str, form: &BTreeMap<String, String>, what: &str) -> Result<reqwest::Response, String> {
        let url = self.url(endpoint);
        self.send(|| self.http.post(&url).form(form), what).await
    }

    /// GETs `endpoint` with `query`.
    async fn get(&self, endpoint: &str, query: &[(&str, &str)], what: &str) -> Result<reqwest::Response, String> {
        let url = self.url(endpoint);
        self.send(|| self.http.get(&url).query(query), what).await
    }

    /// The torrents with `filter` (`all`, `downloading` …).
    pub async fn get_torrents(&self, filter: &str) -> Result<Vec<Value>, String> {
        let resp = self.get("torrents/info", &[("filter", filter)], "could not get filtered torrents").await?;
        if resp.status().as_u16() != 200 {
            return Err(format!("could not get filtered torrents; status code: {}: unexpected status", resp.status().as_u16()));
        }
        resp.json().await.map_err(|e| format!("could not unmarshal body: {e}"))
    }

    /// The torrents really downloading: `downloading` and `stalledDL` (paused ones count as downloading in
    /// qBittorrent's filter).
    pub async fn get_torrents_active_downloads(&self) -> Result<usize, String> {
        let list = self.get_torrents("downloading").await?;
        Ok(list.iter().filter(|t| matches!(t.get("state").and_then(Value::as_str), Some("downloading") | Some("stalledDL"))).count())
    }

    /// The whole client's download and upload speed in bytes per second (`transfer/info`).
    pub async fn get_transfer_info(&self) -> Result<(i64, i64), String> {
        let resp = self.get("transfer/info", &[], "could not get transfer info").await?;
        if resp.status().as_u16() != 200 {
            return Err(format!("could not get transfer info; status code: {}: unexpected status", resp.status().as_u16()));
        }
        let v: Value = resp.json().await.map_err(|e| format!("could not unmarshal body: {e}"))?;
        Ok((v.get("dl_info_speed").and_then(Value::as_i64).unwrap_or(0), v.get("up_info_speed").and_then(Value::as_i64).unwrap_or(0)))
    }

    /// Adds a torrent: `torrent` bytes as the `torrents` file, or the magnet in `options["urls"]`.
    pub async fn add_torrent(&self, torrent: Option<&[u8]>, options: &BTreeMap<String, String>, what: &str) -> Result<(), String> {
        let url = self.url("torrents/add");
        let resp = match torrent {
            Some(bytes) => {
                let make = || {
                    let mut form = reqwest::multipart::Form::new();
                    for (k, v) in options {
                        form = form.text(k.clone(), v.clone());
                    }
                    let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name("upload.torrent");
                    self.http.post(&url).multipart(form.part("torrents", part))
                };
                self.send(make, "could not add torrent").await?
            }
            None => self.post("torrents/add", options, "could not add torrent").await?,
        };
        match resp.status().as_u16() {
            200 | 202 | 204 => {
                let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let body = resp.text().await.unwrap_or_default();
                if ct.starts_with("text/plain") && body.trim() == "Fails." {
                    return Err(format!("could not add torrent{what} | torrent(s) failed to be added"));
                }
                Ok(())
            }
            409 => Err(format!("could not add torrent{what} | conflicts detected: torrent(s) failed to be added")),
            415 => Err(format!("could not add torrent{what} | torrent file not valid: torrent(s) failed to be added")),
            s => Err(format!("could not add torrent{what} | unexpected status code: {s}: unexpected status")),
        }
    }

    /// Whether torrent queueing is on (`app/preferences`).
    pub async fn queueing_enabled(&self) -> Result<bool, String> {
        let resp = self.get("app/preferences", &[], "could not get app preferences").await?;
        let v: Value = resp.json().await.map_err(|e| format!("could not unmarshal body: {e}"))?;
        Ok(v.get("queueing_enabled").and_then(Value::as_bool).unwrap_or(false))
    }

    /// Turns torrent queueing on.
    pub async fn set_queueing_enabled(&self) -> Result<(), String> {
        let mut form = BTreeMap::new();
        form.insert("json".to_string(), "{\"queueing_enabled\":true}".to_string());
        self.post("app/setPreferences", &form, "could not set preferences").await.map(|_| ())
    }

    /// Moves `hash` to the top (`topPrio`) or bottom (`bottomPrio`) of the queue.
    pub async fn set_priority(&self, hash: &str, top: bool) -> Result<(), String> {
        let mut form = BTreeMap::new();
        form.insert("hashes".to_string(), hash.to_string());
        let (ep, what) = if top {
            ("torrents/topPrio", "could not set maximum priority")
        } else {
            ("torrents/bottomPrio", "could not set minimum priority")
        };
        let resp = self.post(ep, &form, what).await?;
        match resp.status().as_u16() {
            200 | 204 => Ok(()),
            409 => Err("torrent queueing is not enabled, could not change priority".into()),
            s => Err(format!("{what}; status code: {s}: unexpected status")),
        }
    }

    /// The trackers of `hash`; `None` when qBittorrent doesn't know it yet.
    pub async fn get_torrent_trackers(&self, hash: &str) -> Result<Option<Vec<Value>>, String> {
        let resp = self.get("torrents/trackers", &[("hash", hash)], &format!("could not get torrent trackers for hash: {hash}")).await?;
        match resp.status().as_u16() {
            200 => {}
            403 | 404 => return Ok(None),
            s => return Err(format!("could not get torrent trackers; status code: {s}: unexpected status")),
        }
        resp.json().await.map(Some).map_err(|e| format!("could not unmarshal body: {e}"))
    }

    /// Asks the trackers of `hash` again.
    pub async fn reannounce(&self, hash: &str) -> Result<(), String> {
        let mut form = BTreeMap::new();
        form.insert("hashes".to_string(), hash.to_string());
        self.post("torrents/reannounce", &form, &format!("could not re-announce torrents; hashes: [{hash}]")).await.map(|_| ())
    }

    /// Removes `hash`, keeping its files.
    pub async fn delete(&self, hash: &str) -> Result<(), String> {
        let mut form = BTreeMap::new();
        form.insert("hashes".to_string(), hash.to_string());
        form.insert("deleteFiles".to_string(), "false".to_string());
        self.post("torrents/delete", &form, &format!("could not delete torrents; hashes: [{hash}]")).await.map(|_| ())
    }

    /// Re-announces until a tracker works, waiting `interval` seconds before each look; removes the torrent
    /// when they never do and the options say so (go-qbittorrent's `ReannounceTorrentWithRetry`).
    pub async fn reannounce_with_retry(&self, hash: &str, opts: ReannounceOptions) -> Result<Reannounce, String> {
        let interval = if opts.interval > 0 { opts.interval } else { REANNOUNCE_INTERVAL };
        let max = if opts.max_attempts > 0 { opts.max_attempts } else { REANNOUNCE_MAX_ATTEMPTS };
        let mut attempts = 0;
        while attempts < max {
            tokio::time::sleep(Duration::from_secs(interval as u64)).await;
            let trackers =
                self.get_torrent_trackers(hash).await.map_err(|e| format!("could not get trackers for torrent with hash: {hash}: {e}"))?;
            let Some(trackers) = trackers else {
                attempts += 1;
                continue;
            };
            if is_tracker_status_ok(&trackers) {
                return Ok(Reannounce::Ok);
            }
            self.reannounce(hash).await.map_err(|e| format!("could not re-announce torrent with hash: {hash}: {e}"))?;
            attempts += 1;
        }
        if opts.delete_on_failure {
            self.delete(hash).await.map_err(|e| format!("could not delete torrent with hash: {hash}: {e}"))?;
            return Ok(Reannounce::TookTooLong);
        }
        Ok(Reannounce::Ok)
    }
}

/// The add options of an action (go-qbittorrent's `TorrentAddOptions.Prepare` with autobrr's
/// `prepareQbitOptions`).
#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// Add it paused (stopped).
    pub paused: bool,
    /// Don't check the files.
    pub skip_hash_check: bool,
    /// `Original`, `Subfolder` or `NoSubfolder`; empty leaves it to the client.
    pub content_layout: String,
    /// Where the files go.
    pub save_path: String,
    /// Where unfinished files go.
    pub download_path: String,
    /// The category.
    pub category: String,
    /// Tags, comma separated.
    pub tags: String,
    /// KiB/s.
    pub limit_upload_speed: i64,
    /// KiB/s.
    pub limit_download_speed: i64,
    /// Seeding ratio.
    pub limit_ratio: f64,
    /// Seeding minutes.
    pub limit_seed_time: i64,
    /// First and last pieces first.
    pub first_last_piece_prio: bool,
}

impl AddOptions {
    /// The form fields qBittorrent takes.
    pub fn prepare(&self) -> BTreeMap<String, String> {
        let mut o = BTreeMap::new();
        let mut set = |k: &str, v: String| {
            o.insert(k.to_string(), v);
        };
        let paused = if self.paused { "true" } else { "false" };
        set("paused", paused.into());
        set("stopped", paused.into());
        if self.skip_hash_check {
            set("skip_checking", "true".into());
        }
        match self.content_layout.as_str() {
            "Subfolder" => {
                set("root_folder", "true".into());
                set("contentLayout", "Subfolder".into());
            }
            "NoSubfolder" => {
                set("root_folder", "false".into());
                set("contentLayout", "NoSubfolder".into());
            }
            _ => {}
        }
        if !self.save_path.is_empty() {
            set("savepath", self.save_path.clone());
            set("autoTMM", "false".into());
        }
        if !self.download_path.is_empty() {
            set("downloadPath", self.download_path.clone());
            set("useDownloadPath", "true".into());
            set("autoTMM", "false".into());
        }
        if !self.category.is_empty() {
            set("category", self.category.clone());
        }
        if !self.tags.is_empty() {
            set("tags", self.tags.clone());
        }
        if self.limit_upload_speed > 0 {
            set("upLimit", (self.limit_upload_speed * 1024).to_string());
        }
        if self.limit_download_speed > 0 {
            set("dlLimit", (self.limit_download_speed * 1024).to_string());
        }
        if self.limit_ratio > 0.0 {
            set("ratioLimit", format!("{:.2}", self.limit_ratio));
        }
        if self.limit_seed_time > 0 {
            set("seedingTimeLimit", self.limit_seed_time.to_string());
        }
        set("firstLastPiecePrio", self.first_last_piece_prio.to_string());
        o
    }
}
