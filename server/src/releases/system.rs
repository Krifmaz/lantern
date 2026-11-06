// Ported from autobrr v1.90.0 (internal/api/service.go, internal/database/api.go, internal/domain/api.go,
// internal/update/update.go, pkg/version/version.go, internal/scheduler/jobs.go and service.go's app jobs,
// internal/config/config.go's defaults and dynamic settings, internal/auth/service.go), GPL-2.0-or-later.
//! autobrr's application parts: its API keys, the settings of its config file that can change while it runs
//! (log level, log file, update checks), the update check against Lantern's own releases (the kit's release
//! feed, [`kit_server_updates::feed::Feed`], stands in for autobrr's), the `APP_UPDATE_AVAILABLE`
//! notification, and the daily clean-up of old temporary torrent files.
//!
//! Lantern's accounts are autobrr's users: signing in, the onboarding check and changing the password work on
//! Lantern's `users` table ([`crate::auth`]), not on a table of autobrr's.

use super::scheduler::{Cron, JobFn};
use super::store::{ApiError, ApiResult};
use super::Releases;
use chrono::{DateTime, SecondsFormat, Utc};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The tables: API keys and the config file's settings.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_api_keys (
            key TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            scopes TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS autobrr_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| anyhow::anyhow!("The autobrr settings tables can't be made: {}", e.detail))
}

// ---- API keys ----

/// One API key (autobrr's `APIKey`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiKey {
    /// What it is for.
    pub name: String,
    /// The key itself (made by Lantern).
    pub key: String,
    /// The scopes (`null` is refused, as autobrr's table refuses it).
    pub scopes: Option<Vec<String>>,
    /// When it was made.
    #[serde(serialize_with = "go_time_out")]
    pub created_at: Option<DateTime<Utc>>,
}

/// A time as Go's JSON writes it; the zero time for none.
fn go_time_out<S: serde::Serializer>(t: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
    match t {
        Some(t) => s.serialize_str(&t.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
        None => s.serialize_str("0001-01-01T00:00:00Z"),
    }
}

/// `length` random bytes as hex (autobrr's `GenerateSecureToken`).
pub fn generate_secure_token(length: usize) -> String {
    use rand::Rng;
    let mut b = vec![0u8; length];
    rand::rng().fill(&mut b[..]);
    hex::encode(b)
}

/// An API key row.
fn key_of(r: &kit_server_core::Row) -> ApiKey {
    ApiKey {
        name: r.s("name"),
        key: r.s("key"),
        scopes: Some(serde_json::from_str(&r.s("scopes")).unwrap_or_default()),
        created_at: DateTime::parse_from_rfc3339(&r.s("created_at")).ok().map(|t| t.with_timezone(&Utc)),
    }
}

impl Releases {
    /// Every API key, oldest first (autobrr's `List`).
    pub fn api_keys(&self) -> ApiResult<Vec<ApiKey>> {
        Ok(self.db.query("SELECT * FROM autobrr_api_keys ORDER BY rowid", p![])?.iter().map(key_of).collect())
    }

    /// Makes key `k` a new key of 16 random bytes and saves it (autobrr's `Store`).
    pub fn store_api_key(&self, k: &mut ApiKey) -> ApiResult<()> {
        k.key = generate_secure_token(16);
        let Some(scopes) = &k.scopes else {
            return Err(ApiError::Internal("error executing query: NOT NULL constraint failed: api_key.scopes".into()));
        };
        let now = Utc::now().with_nanosecond_zero();
        self.db
            .execute(
                "INSERT INTO autobrr_api_keys (key, name, scopes, created_at) VALUES (?,?,?,?)",
                p![k.key, k.name, serde_json::to_string(scopes).unwrap_or_else(|_| "[]".into()), now.to_rfc3339_opts(SecondsFormat::Secs, true)],
            )
            .map_err(|e| ApiError::Internal(format!("error executing query: {}", e.detail)))?;
        k.created_at = Some(now);
        Ok(())
    }

    /// Removes API key `key`; a not-found when there is none (autobrr's `Delete`).
    pub fn delete_api_key(&self, key: &str) -> ApiResult<()> {
        if self.db.one("SELECT key FROM autobrr_api_keys WHERE key=?", p![key])?.is_none() {
            return Err(ApiError::NotFound(super::proxy::ERR_NO_ROWS.into()));
        }
        self.db.execute("DELETE FROM autobrr_api_keys WHERE key=?", p![key])?;
        Ok(())
    }

    /// Whether `key` is one of autobrr's API keys (autobrr's `ValidateAPIKey`).
    pub fn validate_api_key(&self, key: &str) -> bool {
        !key.is_empty() && self.db.one("SELECT key FROM autobrr_api_keys WHERE key=?", p![key]).ok().flatten().is_some()
    }
}

/// Whole seconds.
trait WholeSeconds {
    /// The same time without its fraction of a second.
    fn with_nanosecond_zero(self) -> Self;
}

impl WholeSeconds for DateTime<Utc> {
    fn with_nanosecond_zero(self) -> Self {
        use chrono::Timelike;
        self.with_nanosecond(0).unwrap_or(self)
    }
}

// ---- the config file's settings ----

/// The settings of autobrr's config file Lantern keeps: the ones autobrr changes while it runs (`PATCH
/// /api/config` and its `DynamicReload`). The others are Lantern's own (port, data folder) or fixed.
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    /// `TRACE`, `DEBUG`, `INFO`, `WARN` or `ERROR` (autobrr's default `INFO`).
    pub log_level: String,
    /// The log file, inside the data folder (empty: none, autobrr's default).
    pub log_path: String,
    /// Check Lantern's releases for a newer version every 2 hours (autobrr's default on).
    pub check_for_updates: bool,
}

/// The largest log file in megabytes before it rolls over (autobrr's `LogMaxSize` default).
pub const LOG_MAX_SIZE: i64 = 10;
/// How many rolled-over log files are kept (autobrr's `LogMaxBackups` default).
pub const LOG_MAX_BACKUPS: i64 = 5;

/// What `PATCH /api/config` may change (autobrr's `ConfigUpdate`; host, port and base URL are taken and
/// ignored, as autobrr ignores them).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConfigUpdate {
    /// Ignored.
    pub host: Option<String>,
    /// Ignored.
    pub port: Option<i64>,
    /// The new log level.
    pub log_level: Option<String>,
    /// The new log file.
    pub log_path: Option<String>,
    /// Ignored.
    pub base_url: Option<String>,
    /// Update checks on or off.
    pub check_for_updates: Option<bool>,
}

impl Releases {
    /// The config file's settings as saved, autobrr's defaults for the rest.
    pub fn app_config(&self) -> AppConfig {
        let get = |k: &str| self.db.one("SELECT value FROM autobrr_config WHERE key=?", p![k]).ok().flatten().map(|r| r.s("value"));
        AppConfig {
            log_level: get("log_level").unwrap_or_else(|| "INFO".into()),
            log_path: get("log_path").unwrap_or_default(),
            check_for_updates: get("check_for_updates").map(|v| v == "true").unwrap_or(true),
        }
    }

    /// Applies `u` and saves it (autobrr's `updateConfig` and `UpdateConfig`). A log file outside the data
    /// folder `data` is refused: Lantern writes nowhere else.
    pub fn update_app_config(&self, u: &ConfigUpdate, data: &Path) -> ApiResult<AppConfig> {
        let mut c = self.app_config();
        if let Some(v) = u.check_for_updates {
            c.check_for_updates = v;
        }
        if let Some(v) = &u.log_level {
            c.log_level = v.clone();
        }
        if let Some(v) = &u.log_path {
            if !v.is_empty() && log_file(data, v).is_none() {
                return Err(ApiError::Internal(format!(
                    "The log file {v} is outside the data folder {}. Pick a file inside it, such as logs/autobrr.log.",
                    data.display()
                )));
            }
            c.log_path = v.clone();
        }
        for (k, v) in [("log_level", c.log_level.clone()), ("log_path", c.log_path.clone()), ("check_for_updates", c.check_for_updates.to_string())] {
            self.db.execute("INSERT INTO autobrr_config (key, value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", p![k, v])?;
        }
        Ok(c)
    }
}

/// Where log file `path` is: a relative one inside the data folder `data`, an absolute one only when it is
/// inside `data` too; `None` otherwise (or when it climbs out with `..`).
pub fn log_file(data: &Path, path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    let full = if p.is_absolute() { p.to_path_buf() } else { data.join(p) };
    full.starts_with(data).then_some(full)
}

// ---- updates ----

/// What a newer release is (autobrr's `version.Release`, GitHub's release JSON).
pub type GithubRelease = Value;

/// The kit's release feed of Lantern's own repository.
pub fn feed() -> kit_server_updates::feed::Feed {
    kit_server_updates::feed::Feed::new("Krifmaz", "lantern")
}

/// autobrr's `isDevelop`: builds that never look for updates.
pub fn is_develop(version: &str) -> bool {
    version.starts_with("pr-") || ["dev", "develop", "master", "latest", ""].contains(&version)
}

/// A version read as hashicorp/go-version reads it: the numbers and the pre-release part.
fn parse_version(v: &str) -> Result<(Vec<u64>, String), String> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let v = v.split('+').next().unwrap_or(v);
    let (nums, pre) = match v.split_once('-') {
        Some((n, p)) => (n, p.to_string()),
        None => (v, String::new()),
    };
    let parts: Result<Vec<u64>, _> = nums.split('.').map(|p| p.parse::<u64>()).collect();
    match parts {
        Ok(p) if !p.is_empty() && !nums.is_empty() => Ok((p, pre)),
        _ => Err(format!("Malformed version: {v}")),
    }
}

/// go-version's pre-release order: each dot-separated part compared, numbers as numbers, others as text; a
/// version without a pre-release is above one with.
fn compare_pre(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('.').collect(), b.split('.').collect());
    for i in 0..pa.len().max(pb.len()) {
        let o = match (pa.get(i), pb.get(i)) {
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(n), Ok(m)) => n.cmp(&m),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                _ => x.cmp(y),
            },
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// The version as go-version prints it: the numbers (at least three) and the pre-release part.
fn version_string(nums: &[u64], pre: &str) -> String {
    let mut n: Vec<String> = nums.iter().map(u64::to_string).collect();
    while n.len() < 3 {
        n.push("0".into());
    }
    if pre.is_empty() {
        n.join(".")
    } else {
        format!("{}-{pre}", n.join("."))
    }
}

/// autobrr's `checkNewVersion`: whether `tag` is newer than `version` (a pre-release never counts for a full
/// release), and the newer version.
pub fn check_new_version(version: &str, tag: &str) -> Result<(bool, String), String> {
    let (cur, cur_pre) = parse_version(version).map_err(|e| format!("error parsing current version: {e}"))?;
    let (rel, rel_pre) = parse_version(tag).map_err(|e| format!("error parsing release version: {e}"))?;
    if cur_pre.is_empty() && !rel_pre.is_empty() {
        return Ok((false, String::new()));
    }
    let len = cur.len().max(rel.len());
    let pad = |v: &[u64]| (0..len).map(|i| v.get(i).copied().unwrap_or(0)).collect::<Vec<u64>>();
    let order = pad(&rel).cmp(&pad(&cur)).then_with(|| compare_pre(&rel_pre, &cur_pre));
    if order == std::cmp::Ordering::Greater {
        return Ok((true, version_string(&rel, &rel_pre)));
    }
    Ok((false, String::new()))
}

/// The update check (autobrr's update `Service`): the newest release found, and where to ask.
#[derive(Clone)]
pub struct Updates {
    /// The newest release found newer than this one (`None`: none yet).
    latest: Arc<RwLock<Option<GithubRelease>>>,
    /// GitHub's release list of Lantern ([`feed`]); tests point it at a stand-in.
    url: Arc<RwLock<String>>,
    /// The version running.
    pub current: String,
}

impl Default for Updates {
    fn default() -> Updates {
        Updates { latest: Arc::default(), url: Arc::new(RwLock::new(feed().releases_api())), current: crate::VERSION.to_string() }
    }
}

impl Updates {
    /// Asks `url` for the release list instead (tests).
    pub fn set_url(&self, url: &str) {
        *self.url.write().unwrap_or_else(|e| e.into_inner()) = url.to_string();
    }

    /// The newest release found (autobrr's `GetLatestRelease`).
    pub fn latest(&self) -> Option<GithubRelease> {
        self.latest.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The newest full release of Lantern (the kit's feed picks it from GitHub's list).
    async fn get(&self) -> Result<GithubRelease, String> {
        let url = self.url.read().unwrap_or_else(|e| e.into_inner()).clone();
        let http = super::clients::http(Duration::from_secs(30), false, false);
        let resp = http
            .get(&url)
            .header("Accept", "application/vnd.github.v3+json")
            .header("User-Agent", concat!("Lantern/", env!("CARGO_PKG_VERSION")))
            .send()
            .await
            .map_err(|e| super::clients::req_err("Get", &url, &e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("error getting releases for lantern: {status}"));
        }
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        // the list (the kit's feed) or one release (autobrr's `releases/latest`)
        let one = if v.is_array() { feed().full_release(&v).cloned() } else { Some(v) };
        one.ok_or_else(|| "error getting releases for lantern: no full release yet".to_string())
    }

    /// autobrr's `CheckUpdateAvailable`: the newer release when it is newer than the running one and not the
    /// one found last time; a failed check is only logged.
    pub async fn check_update_available(&self) -> Option<GithubRelease> {
        if is_develop(&self.current) {
            return None;
        }
        let release = match self.get().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("update: could not check for new release: {e}");
                return None;
            }
        };
        let tag = release["tag_name"].as_str().unwrap_or("");
        match check_new_version(&self.current, tag) {
            Ok((true, _)) => {
                tracing::info!("update: found newer release {tag}");
                let mut latest = self.latest.write().unwrap_or_else(|e| e.into_inner());
                if latest.as_ref().is_some_and(|l| l["tag_name"] == release["tag_name"]) {
                    return None;
                }
                *latest = Some(release.clone());
                Some(release)
            }
            Ok((false, _)) => None,
            Err(e) => {
                tracing::error!("update: error checking new release: {e}");
                None
            }
        }
    }
}

/// autobrr's `Release` fields as its JSON sends them (`omitempty` ones left out when empty).
pub fn release_json(r: &GithubRelease) -> Value {
    let mut out = serde_json::Map::new();
    for k in ["id", "node_id", "url", "html_url", "tag_name", "target_commitish", "name", "body", "draft", "prerelease"] {
        let v = &r[k];
        let empty = v.is_null() || v == &json!("") || v == &json!(0) || v == &json!(false);
        if !empty {
            out.insert(k.into(), v.clone());
        }
    }
    let zero = json!("0001-01-01T00:00:00Z");
    out.insert("created_at".into(), if r["created_at"].is_string() { r["created_at"].clone() } else { zero.clone() });
    out.insert("published_at".into(), if r["published_at"].is_string() { r["published_at"].clone() } else { zero });
    out.insert("author".into(), r["author"].clone());
    out.insert("assets".into(), r["assets"].clone());
    Value::Object(out)
}

/// The update job's state between runs (autobrr's `CheckUpdatesJob.lastCheckVersion`; empty: this version).
#[derive(Default)]
pub struct LastCheck(std::sync::Mutex<String>);

impl Releases {
    /// One run of the update job (autobrr's `CheckUpdatesJob.Run`): a newer release than the one seen last
    /// sends `APP_UPDATE_AVAILABLE`. Turned off by the config file's `checkForUpdates`, at once.
    async fn run_update_check(&self, last: &LastCheck) {
        if !self.app_config().check_for_updates {
            return;
        }
        let Some(r) = self.updates.check_update_available().await else { return };
        let tag = r["tag_name"].as_str().unwrap_or("").to_string();
        let mut l = last.0.lock().unwrap_or_else(|e| e.into_inner());
        if *l != tag {
            tracing::info!("update: new release available {tag}");
            self.notify(super::notify::Payload {
                event: "APP_UPDATE_AVAILABLE".into(),
                subject: "New update available!".into(),
                message: tag.clone(),
                current_version: self.updates.current.clone(),
                new_version: tag.clone(),
                url: r["html_url"].as_str().unwrap_or("").to_string(),
                timestamp: Some(Utc::now()),
                ..super::notify::Payload::default()
            });
        }
        *l = tag;
    }

    /// Runs the update check once now, as the job does (tests).
    pub async fn run_update_job_once(&self) {
        self.run_update_check(&self.update_last).await;
    }

    /// autobrr's app jobs (`addAppJobs`, 5 seconds after the start): the update check every 2 hours when it is
    /// on, and the temporary folder's clean-up every day at 04:00.
    pub fn start_app_jobs(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if me.app_config().check_for_updates {
                let job_me = me.clone();
                let work: JobFn = Arc::new(move || {
                    let me = job_me.clone();
                    Box::pin(async move { me.run_update_check(&me.update_last).await })
                });
                me.scheduler.register("app-check-updates", Box::new(super::notify::Every(chrono::Duration::hours(2))), work);
            }
            let dir = super::run::default_tmp_dir();
            let work: JobFn = Arc::new(move || {
                let dir = dir.clone();
                Box::pin(async move {
                    let _ = tokio::task::spawn_blocking(move || temp_dir_cleanup(&dir, std::time::SystemTime::now())).await;
                })
            });
            me.scheduler.register("temp-dir-cleanup", Box::new(Cron::Daily(4, 0)), work);
        });
    }
}

// ---- the temporary folder ----

/// autobrr's `TempDirCleanupJob`: removes the regular files named `autobrr-…` in `dir` that this user owns and
/// that were last changed more than 24 hours before `now`; how many, and their size.
pub fn temp_dir_cleanup(dir: &Path, now: std::time::SystemTime) -> (u64, u64) {
    let (mut deleted, mut total) = (0u64, 0u64);
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::debug!("temp-dir-cleanup: failed to read temporary directory {}", dir.display());
        return (0, 0);
    };
    let cutoff = now.checked_sub(Duration::from_secs(24 * 3600)).unwrap_or(std::time::UNIX_EPOCH);
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with("autobrr-") {
            continue;
        }
        // the entry itself, never what a link points at
        let Ok(meta) = e.metadata() else { continue };
        if !meta.file_type().is_file() || !owned_by_me(&meta) {
            continue;
        }
        if meta.modified().is_ok_and(|m| m < cutoff) {
            match std::fs::remove_file(e.path()) {
                Ok(()) => {
                    deleted += 1;
                    total += meta.len();
                }
                Err(err) => tracing::error!("temp-dir-cleanup: failed to remove temporary file {}: {err}", e.path().display()),
            }
        }
    }
    tracing::debug!("temp-dir-cleanup: completed, {deleted} files, {total} bytes");
    (deleted, total)
}

/// Whether the file is this process's user's (autobrr's `isOwnedByCurrentUser`).
#[cfg(target_os = "linux")]
fn owned_by_me(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and can't fail
    meta.uid() == unsafe { libc::geteuid() }
}

/// Whether the file is this process's user's: always on systems without owners.
#[cfg(not(target_os = "linux"))]
fn owned_by_me(_: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// autobrr's pkg/version/version_test.go `TestGitHubReleaseChecker_checkNewVersion`.
    #[test]
    fn test_github_release_checker_check_new_version() {
        let cases = [
            ("outdated new available", "v0.2.0", "v0.3.0", true, "0.3.0"),
            ("same version", "v0.2.0", "v0.2.0", false, ""),
            ("no new version", "v0.3.0", "v0.2.0", false, ""),
            ("new rc available", "v0.3.0", "v0.3.0-rc1", false, ""),
            ("new rc available", "v0.3.0-RC1", "v0.3.0-RC2", true, "0.3.0-RC2"),
        ];
        for (name, version, tag, want_new, want_version) in cases {
            let (got, v) = check_new_version(version, tag).unwrap();
            assert_eq!((got, v.as_str()), (want_new, want_version), "{name}");
        }
    }

    /// autobrr's `Test_isDevelop`.
    #[test]
    fn test_is_develop() {
        for (v, want) in [("dev", true), ("develop", true), ("master", true), ("latest", true), ("v1.0.1", false), ("1.0.1", false)] {
            assert_eq!(is_develop(v), want, "{v}");
        }
    }

    #[test]
    fn log_files_stay_inside_the_data_folder() {
        let data = Path::new("/data");
        assert_eq!(log_file(data, "logs/autobrr.log"), Some(PathBuf::from("/data/logs/autobrr.log")));
        assert_eq!(log_file(data, "/data/x.log"), Some(PathBuf::from("/data/x.log")));
        assert_eq!(log_file(data, "/var/log/x.log"), None);
        assert_eq!(log_file(data, "../x.log"), None);
    }

    /// A fresh folder for one test.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lantern-tmpclean-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `path`'s change time put back 25 hours.
    fn backdate(path: &Path) {
        let old = std::time::SystemTime::now() - Duration::from_secs(25 * 3600);
        std::fs::File::open(path).unwrap().set_modified(old).unwrap();
    }

    /// autobrr's internal/scheduler/jobs_test.go `TestTempDirCleanupJob_RemovesStaleFiles`.
    #[test]
    fn test_temp_dir_cleanup_job_removes_stale_files() {
        let dir = folder("stale");
        let (stale, recent, unrelated) = (dir.join("autobrr-stale"), dir.join("autobrr-recent"), dir.join("someone-elses-file"));
        for p in [&stale, &recent, &unrelated] {
            std::fs::write(p, b"torrent").unwrap();
        }
        backdate(&stale);
        backdate(&unrelated);
        assert_eq!(temp_dir_cleanup(&dir, std::time::SystemTime::now()), (1, 7));
        assert!(!stale.exists(), "stale autobrr tmp file should be removed");
        assert!(recent.exists(), "recent autobrr tmp file should be kept");
        assert!(unrelated.exists(), "files without the autobrr prefix should be left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// autobrr's `TestTempDirCleanupJob_LeavesDirectoriesAlone`.
    #[test]
    fn test_temp_dir_cleanup_job_leaves_directories_alone() {
        let dir = folder("dirs");
        let sub = dir.join("autobrr-dir");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("child"), b"x").unwrap();
        backdate(&sub);
        temp_dir_cleanup(&dir, std::time::SystemTime::now());
        assert!(sub.exists(), "a directory sharing the prefix must be left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// autobrr's jobs_unix_test.go `TestTempDirCleanupJob_DoesNotFollowSymlinks`.
    #[cfg(unix)]
    #[test]
    fn test_temp_dir_cleanup_job_does_not_follow_symlinks() {
        let dir = folder("links");
        let other = folder("links-target");
        let target = other.join("real-file");
        std::fs::write(&target, b"not a torrent").unwrap();
        backdate(&target);
        let link = dir.join("autobrr-symlink");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        temp_dir_cleanup(&dir, std::time::SystemTime::now());
        assert!(std::fs::symlink_metadata(&link).is_ok(), "a symlink must not be followed or removed");
        assert!(target.exists(), "the symlink target must be untouched");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    /// autobrr's `TestIsOwnedByCurrentUser` and `TestIsOwnedByCurrentUser_OtherUser`.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_owned_by_current_user() {
        let dir = folder("owner");
        let p = dir.join("autobrr-owned");
        std::fs::write(&p, b"torrent").unwrap();
        assert!(owned_by_me(&std::fs::metadata(&p).unwrap()), "a file we just created is owned by us");
        // SAFETY: geteuid has no preconditions
        if unsafe { libc::geteuid() } != 0 {
            assert!(!owned_by_me(&std::fs::metadata("/etc/passwd").unwrap()), "root's file is not ours");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
