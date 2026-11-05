// Ported from autobrr v1.90.0 (internal/action/run*.go, internal/action/arr.go, internal/release/download_service.go,
// internal/downloader/connection.go), GPL-2.0-or-later.
//! Running an action on a release (autobrr's `RunAction`): the torrent is fetched once onto the release when the
//! action or its macros need it, a temporary file is written when a macro names its path, the macros are filled
//! in, then the action's type decides where the release goes. Every download client speaks its own protocol
//! ([`super::clients`]); the answers are rejections (the client said no, in its words) or an error.
//!
//! Lantern's download hub: when an action's download client is one the hub knows (same kind, same address), the
//! release goes through the hub instead, so it gets the hub app's category, folder and tracking
//! ([`Service::hub_route`]). The [`DOWNLOAD_HUB`] action type always does.

use super::action::{Action, DOWNLOAD_HUB};
use super::clients::{self, aria2, arr, deluge, nzbget, porla, qbittorrent, rtorrent, sabnzbd, transmission};
use super::downloader::Downloader;
use super::push::{DownloadHub, Grab, DEFAULT_APP};
use super::release::Release;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// A boxed future.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The prefix of a magnet link (autobrr's `MagnetURIPrefix`).
pub const MAGNET_URI_PREFIX: &str = "magnet:?";
/// Seconds between Transmission re-announces when the action says none.
pub const REANNOUNCE_INTERVAL: i64 = 7;
/// The most Transmission re-announces when the action says none.
pub const REANNOUNCE_MAX_ATTEMPTS: i64 = 50;

/// Whether the release has a magnet link (autobrr's `HasMagnetUri`).
pub fn has_magnet_uri(r: &Release) -> bool {
    r.magnet_uri.starts_with(MAGNET_URI_PREFIX)
}

/// Fetches a release's torrent and resolves its magnet link (autobrr's release download service).
pub trait ReleaseDownloader: Send + Sync {
    /// Puts the torrent's bytes, hash and size on `r`, unless they are there already.
    fn download_release<'a>(&'a self, r: &'a mut Release) -> BoxFut<'a, Result<(), String>>;
    /// Turns a magnet link given as a web address into the magnet link itself.
    fn resolve_magnet_uri<'a>(&'a self, r: &'a mut Release) -> BoxFut<'a, Result<(), String>>;
}

/// The download clients by id.
pub type Lookup = Arc<dyn Fn(i64) -> Result<Downloader, String> + Send + Sync>;

/// What runs actions: the torrent fetcher, the download clients, the hub, and where temporary files go.
#[derive(Clone)]
pub struct Service {
    /// Fetches torrents.
    pub downloads: Arc<dyn ReleaseDownloader>,
    /// The download clients.
    pub downloaders: Lookup,
    /// Lantern's download hub.
    pub hub: Arc<dyn DownloadHub>,
    /// For webhooks.
    pub http: reqwest::Client,
    /// Where temporary torrent files are written.
    pub tmp_dir: PathBuf,
}

/// The folder for temporary torrent files: `tmp` in the data folder (`APP_DATA_DIR`, else `/data`).
pub fn default_tmp_dir() -> PathBuf {
    PathBuf::from(std::env::var("APP_DATA_DIR").unwrap_or_else(|_| "/data".into())).join("tmp")
}

/// The client autobrr's webhooks go out with.
pub fn webhook_client() -> reqwest::Client {
    clients::http(Duration::from_secs(60), false, false)
}

/// Writes the release's torrent to a temporary file (autobrr's `WriteTemporaryFile`).
pub fn write_temporary_file(r: &mut Release, dir: &std::path::Path) -> Result<(), String> {
    if r.torrent_data_raw_bytes.is_empty() {
        return Err(format!("could not write temporary file: torrent data is empty for release: {}", r.torrent_name));
    }
    if !r.torrent_tmp_file.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create TMP dir: {}: {e}", dir.display()))?;
    let path = dir.join(format!("autobrr-{}", hex::encode(rand::random::<[u8; 6]>())));
    std::fs::write(&path, &r.torrent_data_raw_bytes).map_err(|e| format!("error writing tmp file: {e}"))?;
    r.torrent_tmp_file = path.to_string_lossy().into_owned();
    Ok(())
}

/// Removes the release's temporary file and lets go of its torrent (autobrr's `CleanupTemporaryFiles`).
pub fn cleanup_temporary_files(r: &mut Release) {
    if !r.torrent_tmp_file.is_empty() {
        let _ = std::fs::remove_file(&r.torrent_tmp_file);
        r.torrent_tmp_file.clear();
    }
    r.torrent_data_raw_bytes = Vec::new();
}

/// The *arr download client to use: the action's pair when it sets either, else the client's (autobrr's
/// `arrDownloadClient`).
pub fn arr_download_client(d: &Downloader, a: &Action) -> (i64, String) {
    if a.external_download_client_id > 0 || !a.external_download_client.is_empty() {
        return (a.external_download_client_id, a.external_download_client.clone());
    }
    (d.settings.external_download_client_id, d.settings.external_download_client.clone())
}

/// The publish date Sonarr and Radarr get: the release's, unless it is missing, a placeholder (on or before
/// 1970-04-01) or in the future, then now; in UTC (autobrr's `arrPublishDate`).
pub fn arr_publish_date(r: &Release, now: chrono::DateTime<chrono::Utc>) -> String {
    let cutoff = chrono::DateTime::parse_from_rfc3339("1970-04-01T00:00:00Z").map(|d| d.to_utc()).unwrap_or_default();
    match r.publish_date {
        Some(d) if d > cutoff && d <= now => d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        _ => now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }
}

/// Now as Go's `time.Now().Format(time.RFC3339)`: local time with its offset.
fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The indexer name the *arrs get (autobrr's `GetExternalIdentifier`).
fn external_identifier(r: &Release) -> String {
    if r.indexer.identifier_external.is_empty() {
        r.indexer.identifier.clone()
    } else {
        r.indexer.identifier_external.clone()
    }
}

/// The release push body for `app` (autobrr's `ReleasePushRequest`s, with their `omitempty`s).
pub fn arr_push_body(app: arr::App, d: &Downloader, a: &Action, r: &Release, now: chrono::DateTime<chrono::Utc>) -> Value {
    let mut m = Map::new();
    m.insert("title".into(), json!(r.torrent_name));
    for (k, v) in [("infoUrl", &r.info_url), ("downloadUrl", &r.download_url), ("magnetUrl", &r.magnet_uri)] {
        if !v.is_empty() {
            m.insert(k.into(), json!(v));
        }
    }
    m.insert("size".into(), json!(r.size));
    m.insert("indexer".into(), json!(external_identifier(r)));
    m.insert("downloadProtocol".into(), json!(r.protocol));
    m.insert("protocol".into(), json!(r.protocol));
    let date = match app {
        arr::App::Sonarr | arr::App::Radarr => arr_publish_date(r, now),
        _ => now_rfc3339(),
    };
    m.insert("publishDate".into(), json!(date));
    if app != arr::App::Sportarr {
        let (id, name) = arr_download_client(d, a);
        if id != 0 {
            m.insert("downloadClientId".into(), json!(id));
        }
        if !name.is_empty() {
            m.insert("downloadClient".into(), json!(name));
        }
    }
    let flags = match app {
        arr::App::Sonarr | arr::App::Sportarr => arr::sonarr_indexer_flags(r.freeleech_percent, &r.origin),
        arr::App::Radarr => arr::radarr_indexer_flags(r.freeleech_percent, &r.origin),
        _ => 0,
    };
    if flags != 0 {
        m.insert("indexerFlags".into(), json!(flags));
    }
    match app {
        arr::App::Sonarr => {
            if !r.meta_imdb.is_empty() {
                m.insert("imdbId".into(), json!(r.meta_imdb));
            }
            if r.meta_tvdb != 0 {
                m.insert("tvdbId".into(), json!(r.meta_tvdb));
            }
        }
        arr::App::Radarr if r.meta_tmdb != 0 => {
            m.insert("tmdbId".into(), json!(r.meta_tmdb));
        }
        _ => {}
    }
    Value::Object(m)
}

/// qBittorrent's add options from the action (autobrr's `prepareQbitOptions`).
pub fn qbit_options(a: &Action) -> qbittorrent::AddOptions {
    qbittorrent::AddOptions {
        paused: a.paused,
        skip_hash_check: a.skip_hash_check,
        first_last_piece_prio: a.first_last_piece_prio,
        content_layout: match a.content_layout.as_str() {
            "SUBFOLDER_CREATE" => "Subfolder".into(),
            "SUBFOLDER_NONE" => "NoSubfolder".into(),
            _ => String::new(),
        },
        save_path: a.save_path.trim().to_string(),
        download_path: a.download_path.trim().to_string(),
        category: a.category.trim().to_string(),
        tags: if a.tags.is_empty() { String::new() } else { a.tags.split(',').map(str::trim).collect::<Vec<_>>().join(",") },
        limit_upload_speed: a.limit_upload_speed.max(0),
        limit_download_speed: a.limit_download_speed.max(0),
        limit_ratio: if a.limit_ratio > 0.0 { a.limit_ratio } else { 0.0 },
        limit_seed_time: a.limit_seed_time.max(0),
    }
}

/// The rejections when the client's total speeds are at or over the rules' thresholds, in KiB/s (autobrr's
/// `qbittorrentCheckIgnoreSlow`).
pub fn qbit_check_ignore_slow(dl_threshold: i64, ul_threshold: i64, dl_speed: i64, ul_speed: i64) -> Vec<String> {
    let mut out = Vec::new();
    if dl_threshold > 0 && dl_speed / 1024 >= dl_threshold {
        out.push(format!("total download speed ({}) above threshold: ({dl_threshold}), skipping", dl_speed / 1024));
    }
    if ul_threshold > 0 && ul_speed / 1024 >= ul_threshold {
        out.push(format!("total upload speed ({}) above threshold: ({ul_threshold}), skipping", ul_speed / 1024));
    }
    out
}

/// rTorrent's extra commands from the action (autobrr's `runRTorrent`, before the ratio group).
pub fn rtorrent_args(a: &Action) -> Vec<rtorrent::FieldValue> {
    let mut args = Vec::new();
    if !a.label.is_empty() {
        args.push(rtorrent::FieldValue::set("d.custom1", &a.label));
    }
    if !a.save_path.is_empty() {
        let field = if a.content_layout == "SUBFOLDER_NONE" { "d.directory_base" } else { "d.directory" };
        args.push(rtorrent::FieldValue::set(field, &a.save_path));
    }
    match a.priority_layout.as_str() {
        "LOW" => args.push(rtorrent::FieldValue::set("d.priority", "1")),
        "NORMAL" => args.push(rtorrent::FieldValue::set("d.priority", "2")),
        "HIGH" => args.push(rtorrent::FieldValue::set("d.priority", "3")),
        _ => {}
    }
    args
}

/// Deluge's options from the action (autobrr's `prepareDelugeOptions`).
pub fn deluge_options(a: &Action) -> deluge::Options {
    deluge::Options {
        add_paused: Some(a.paused),
        download_location: (!a.save_path.is_empty()).then(|| a.save_path.clone()),
        max_download_speed: (a.limit_download_speed > 0).then_some(a.limit_download_speed),
        max_upload_speed: (a.limit_upload_speed > 0).then_some(a.limit_upload_speed),
        seed_mode: a.skip_hash_check.then_some(true),
    }
}

/// aria2's options from the action (autobrr's `aria2Options`).
pub fn aria2_options(a: &Action) -> aria2::Options {
    let mut o = aria2::Options::new();
    if !a.save_path.is_empty() {
        o.insert("dir".into(), json!(a.save_path));
    }
    if a.paused {
        o.insert("pause".into(), json!("true"));
    }
    if a.limit_download_speed > 0 {
        o.insert("max-download-limit".into(), json!(format!("{}K", a.limit_download_speed)));
    }
    if a.limit_upload_speed > 0 {
        o.insert("max-upload-limit".into(), json!(format!("{}K", a.limit_upload_speed)));
    }
    if a.limit_ratio > 0.0 {
        o.insert("seed-ratio".into(), json!(go_float(a.limit_ratio)));
    }
    if a.limit_seed_time > 0 {
        o.insert("seed-time".into(), json!(a.limit_seed_time.to_string()));
    }
    o
}

/// A float as Go's `strconv.FormatFloat(f, 'f', -1, 64)` writes it.
fn go_float(f: f64) -> String {
    format!("{f}")
}

/// Whether a tracker message says the torrent isn't registered (autobrr's `isUnregistered`).
pub fn is_unregistered(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["unregistered", "not registered", "not found", "not exist"].iter().any(|w| m.contains(w))
}

/// A hash safe to put in a file name (autobrr's `safeTorrentHashPattern`).
static SAFE_HASH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-fA-F0-9]{40}([a-fA-F0-9]{24})?$").expect("a valid pattern"));

/// Writes the torrent into the watch folder (autobrr's `runWatchFolder`): `autobrr-<hash>.torrent` in the folder,
/// or the path itself when it ends in `.torrent`.
pub fn run_watch_folder(a: &Action, r: &Release) -> Result<(), String> {
    if has_magnet_uri(r) {
        return Err(format!("action watch folder does not support magnet links: {}", r.torrent_name));
    }
    if r.torrent_data_raw_bytes.is_empty() {
        return Err(format!("watch_folder: missing torrent {}", r.torrent_name));
    }
    let (dir, file) = if a.watch_folder.ends_with(".torrent") {
        let p = std::path::Path::new(&a.watch_folder);
        (p.parent().map(|d| d.to_path_buf()).unwrap_or_default(), p.to_path_buf())
    } else {
        if !SAFE_HASH.is_match(&r.torrent_hash) {
            return Err("watch_folder: invalid torrent hash".into());
        }
        let d = PathBuf::from(&a.watch_folder);
        let f = d.join(format!("autobrr-{}.torrent", r.torrent_hash));
        (d, f)
    };
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not create new folders {}: {e}", dir.display()))?;
    }
    std::fs::write(&file, &r.torrent_data_raw_bytes).map_err(|e| format!("could not create new file {}: {e}", file.display()))
}

/// Finds a program the way Go's `exec.LookPath` does: a name with a slash as it is, else along `PATH`.
pub fn look_path(cmd: &str) -> Result<PathBuf, String> {
    let executable = |p: &std::path::Path| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    };
    if cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return if executable(&p) { Ok(p) } else { Err(format!("exec: \"{cmd}\": stat {cmd}: no such file or directory")) };
    }
    if !cmd.is_empty() {
        for dir in std::env::var("PATH").unwrap_or_default().split(':') {
            let p = PathBuf::from(if dir.is_empty() { "." } else { dir }).join(cmd);
            if executable(&p) {
                return Ok(p);
            }
        }
    }
    Err(format!("exec: \"{cmd}\": executable file not found in $PATH"))
}

/// A list as Go's `%s` prints a string slice: `[a b]`.
fn go_list(l: &[String]) -> String {
    format!("[{}]", l.join(" "))
}

/// Runs the program with its arguments split like a shell would, never through one (autobrr's `runExecCmd`).
pub async fn run_exec(a: &Action) -> Result<(), String> {
    let cmd = look_path(&a.exec_cmd).map_err(|e| format!("exec failed, could not find program: {}: {e}", a.exec_cmd))?;
    let args = super::shellquote::split(&a.exec_args).map_err(|e| format!("could not parse exec args: {}: {e}", a.exec_args))?;
    let out = tokio::process::Command::new(&cmd)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("error executing command: {} args: {}: {e}", cmd.display(), go_list(&args)))?;
    if !out.status.success() {
        let why = match out.status.code() {
            Some(c) => format!("exit status {c}"),
            None => "signal: killed".into(),
        };
        return Err(format!("error executing command: {} args: {}: {why}", cmd.display(), go_list(&args)));
    }
    tracing::debug!(command = %cmd.display(), output = %String::from_utf8_lossy(&out.stdout), "exec action ran");
    Ok(())
}

/// The hub's client kind for a download client type, when the hub has such clients.
pub fn hub_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "QBITTORRENT" => "qbittorrent",
        "TRANSMISSION" => "transmission",
        "DELUGE_V1" | "DELUGE_V2" => "deluge",
        "RTORRENT" => "rtorrent",
        "SABNZBD" => "sabnzbd",
        "NZBGET" => "nzbget",
        _ => return None,
    })
}

/// A web address's host and port, lower case (the port the scheme implies when none is given).
fn host_port(addr: &str) -> Option<(String, Option<u16>)> {
    let with_scheme = if addr.contains("://") { addr.to_string() } else { format!("http://{addr}") };
    let u = url::Url::parse(&with_scheme).ok()?;
    Some((u.host_str()?.to_lowercase(), u.port_or_known_default()))
}

/// Whether download client `d` is the hub's client at `url`: same host and, but for Deluge (whose daemon and
/// web interface listen on different ports), the same port.
pub fn same_client(d: &Downloader, hub_url: &str) -> bool {
    let addr = match d.build_legacy_host() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let (Some((h1, p1)), Some((h2, p2))) = (host_port(&addr), host_port(hub_url)) else { return false };
    if h1 != h2 {
        return false;
    }
    d.kind.starts_with("DELUGE") || p1 == p2
}

/// "client <type> <name> not enabled" and friends: the client for an action, enabled.
fn enabled(d: Downloader) -> Result<Downloader, String> {
    if !d.enabled {
        return Err(format!("client {} {} not enabled", d.kind, d.name));
    }
    Ok(d)
}

/// The basic auth pair when the client's settings turn it on.
fn basic(d: &Downloader) -> (String, String) {
    if d.settings.auth.enabled {
        (d.settings.auth.username.clone(), d.settings.auth.password.clone())
    } else {
        (String::new(), String::new())
    }
}

/// An *arr client for `d`.
pub fn arr_client(app: arr::App, d: &Downloader) -> arr::Client {
    arr::Client::new(
        app,
        arr::Config {
            hostname: d.host.clone(),
            api_key: d.settings.api_key.clone(),
            basic_auth: d.settings.auth.enabled,
            username: d.settings.auth.username.clone(),
            password: d.settings.auth.password.clone(),
            tls_skip_verify: d.tls_skip_verify,
        },
    )
}

/// An rTorrent client for `d`.
pub fn rtorrent_client(d: &Downloader) -> rtorrent::Client {
    let (u, p) = (d.settings.auth.username.clone(), d.settings.auth.password.clone());
    let auth = if d.settings.auth.kind == "DIGEST_AUTH" {
        rtorrent::Auth::Digest(u, p)
    } else if !u.is_empty() {
        rtorrent::Auth::Basic(u, p)
    } else {
        rtorrent::Auth::None
    };
    rtorrent::Client::new(&d.host, auth, d.tls_skip_verify)
}

/// A qBittorrent client for `d`.
pub fn qbit_client(d: &Downloader) -> Result<qbittorrent::Client, String> {
    let host = d.build_legacy_host().map_err(|e| format!("error building qBittorrent host url: {}: {e}", d.host))?;
    let (bu, bp) = basic(d);
    Ok(qbittorrent::Client::new(qbittorrent::Config {
        host,
        username: d.username.clone(),
        password: d.password.clone(),
        api_key: d.settings.api_key.clone(),
        tls_skip_verify: d.tls_skip_verify,
        basic_user: bu,
        basic_pass: bp,
    }))
}

/// A Transmission client for `d`.
pub fn transmission_client(d: &Downloader) -> Result<transmission::Client, String> {
    let host = d.build_legacy_host().map_err(|e| format!("error building Transmission host url: {}: {e}", d.host))?;
    Ok(transmission::Client::new(&host, &d.username, &d.password, d.tls_skip_verify))
}

/// Tests the connection to download client `d` (autobrr's `testConnection`).
pub async fn test_connection(d: &Downloader) -> Result<(), String> {
    match d.kind.as_str() {
        "QBITTORRENT" => {
            let c = qbit_client(d)?;
            c.login().await.map_err(|e| format!("error logging into client: {}: {e}", d.host))?;
            c.get_torrents("all").await.map_err(|e| format!("error getting torrents: {}: {e}", d.host))?;
            Ok(())
        }
        "DELUGE_V1" | "DELUGE_V2" => {
            let mut c = deluge::Client::new(&d.host, d.port as u16, &d.username, &d.password, d.kind == "DELUGE_V2");
            c.connect().await.map_err(|e| format!("error logging into client: {}: {e}", d.host))?;
            let r = c.daemon_version().await.map_err(|e| format!("could not get daemon version: {}: {e}", d.host));
            c.close().await;
            r.map(|_| ())
        }
        "RTORRENT" => rtorrent_client(d).name().await.map(|_| ()).map_err(|e| format!("error logging into client: {}: {e}", d.host)),
        "TRANSMISSION" => {
            let (ok, _) = transmission_client(d)?.rpc_version().await.map_err(|e| format!("error getting rpc info: {}: {e}", d.host))?;
            if !ok {
                return Err(format!("error getting rpc info: {}", d.host));
            }
            Ok(())
        }
        "PORLA" => {
            let (bu, bp) = basic(d);
            porla::Client::new(&d.host, &d.settings.api_key, d.tls_skip_verify, &bu, &bp)
                .version()
                .await
                .map(|_| ())
                .map_err(|e| format!("porla: failed to get version: {}: {e}", d.host))
        }
        "ARIA2" => aria2_client(d)?.get_version().await.map(|_| ()).map_err(|e| format!("aria2: failed to get version: {}: {e}", d.host)),
        "SABNZBD" => {
            let (bu, bp) = basic(d);
            sabnzbd::Client::new(&d.host, &d.settings.api_key, d.tls_skip_verify, &bu, &bp)
                .version()
                .await
                .map(|_| ())
                .map_err(|e| format!("error getting version from sabnzbd: {}: {e}", d.host))
        }
        "NZBGET" => nzbget::Client::new(&d.host, &d.username, &d.password, d.tls_skip_verify)
            .version()
            .await
            .map(|_| ())
            .map_err(|e| format!("error getting version from nzbget: {}: {e}", d.host)),
        "SPORTARR" => {
            let v =
                arr_client(arr::App::Sportarr, d).test().await.map_err(|e| format!("sportarr: connection test failed: {}: {e}", d.host))?;
            if !arr::sportarr_supports_native_api(&v) {
                return Err(format!("sportarr: version {v} is too old, autobrr needs Sportarr {} or newer", arr::SPORTARR_MINIMUM_VERSION));
            }
            Ok(())
        }
        k => match arr::App::of(k) {
            Some(app) => {
                arr_client(app, d).test().await.map(|_| ()).map_err(|e| format!("{}: connection test failed: {}: {e}", app.name(), d.host))
            }
            None => Err(format!("unsupported client: {k}")),
        },
    }
}

/// An aria2 client for `d`.
fn aria2_client(d: &Downloader) -> Result<aria2::Client, String> {
    aria2::Client::new(aria2::Config {
        host: d.host.clone(),
        secret: d.settings.api_key.clone(),
        tls_skip_verify: d.tls_skip_verify,
        basic_user: d.settings.auth.username.clone(),
        basic_pass: d.settings.auth.password.clone(),
    })
    .map_err(|e| format!("error creating aria2 client: {}: {e}", d.host))
}

impl Service {
    /// Gets the action's download client.
    fn client_of(&self, a: &Action) -> Result<Downloader, String> {
        (self.downloaders)(a.client_id)
    }

    /// Runs `action` on `release` (autobrr's `RunAction`): the rejections, or why it failed.
    pub async fn run_action(&self, action: &Action, release: &mut Release) -> Result<Vec<String>, String> {
        self.check_action_preconditions(action, release).await?;
        let mut action = action.clone();
        action.parse_macros(release)?;
        let a = &action;
        if a.kind == DOWNLOAD_HUB {
            return self.to_hub(a, release, None).await;
        }
        if hub_kind(&a.kind).is_some() {
            if let Some(d) = self.hub_route(a) {
                return self.to_hub(a, release, Some(&d)).await;
            }
        }
        match a.kind.as_str() {
            "TEST" => {
                tracing::info!("action TEST: release: {} triggered by filter {}", release.torrent_name, release.filter_name);
                Ok(Vec::new())
            }
            "EXEC" => run_exec(a).await.map(|_| Vec::new()),
            "WATCH_FOLDER" => run_watch_folder(a, release).map(|_| Vec::new()),
            "WEBHOOK" => self.run_webhook(a).await.map(|_| Vec::new()),
            "DELUGE_V1" | "DELUGE_V2" => self.run_deluge(a, release).await,
            "QBITTORRENT" => self.run_qbittorrent(a, release).await,
            "RTORRENT" => self.run_rtorrent(a, release).await,
            "TRANSMISSION" => self.run_transmission(a, release).await,
            "PORLA" => self.run_porla(a, release).await,
            "ARIA2" => self.run_aria2(a, release).await,
            "SABNZBD" | "NZBGET" => self.run_usenet(a, release).await,
            k => match arr::App::of(k) {
                Some(app) => self.run_arr(app, a, release).await,
                None => Err(format!("unsupported action type: {k}")),
            },
        }
    }

    /// Fetches what the action needs before its macros are filled in (autobrr's `CheckActionPreconditions`).
    pub async fn check_action_preconditions(&self, a: &Action, r: &mut Release) -> Result<(), String> {
        let magnet = r.magnet_uri.clone();
        self.downloads.resolve_magnet_uri(r).await.map_err(|e| format!("could not resolve magnet uri: {magnet}: {e}"))?;
        if !has_magnet_uri(r) && (a.needs_torrent_downloaded() || a.check_macros_need_raw_data_bytes(r)) {
            let name = r.torrent_name.clone();
            self.downloads.download_release(r).await.map_err(|e| format!("could not download torrent file for release: {name}: {e}"))?;
        }
        if a.check_macros_need_torrent_tmp_file(r) {
            let name = r.torrent_name.clone();
            write_temporary_file(r, &self.tmp_dir).map_err(|e| format!("could not write torrent file for release: {name}: {e}"))?;
        }
        Ok(())
    }

    /// The action's download client when the hub has the same client, so the release goes through the hub.
    pub fn hub_route(&self, a: &Action) -> Option<Downloader> {
        let d = self.client_of(a).ok()?;
        let kind = hub_kind(&d.kind)?;
        self.hub.clients().iter().any(|(k, url)| k == kind && same_client(&d, url)).then_some(d)
    }

    /// Hands the release to the download hub, as the app the action's category names (when the hub has it),
    /// else "Releases".
    async fn to_hub(&self, a: &Action, r: &Release, d: Option<&Downloader>) -> Result<Vec<String>, String> {
        if let Some(d) = d {
            if !d.enabled {
                return Err(format!("client {} {} not enabled", d.kind, d.name));
            }
        }
        let app = if !a.category.trim().is_empty() && self.hub.has_app(a.category.trim()) { a.category.trim() } else { DEFAULT_APP };
        let url = if has_magnet_uri(r) { r.magnet_uri.clone() } else { r.download_url.clone() };
        let g = Grab {
            title: r.torrent_name.clone(),
            download_url: url,
            info_url: r.info_url.clone(),
            size: (r.size > 0).then_some(r.size),
            protocol: if r.protocol.is_empty() { "torrent".into() } else { r.protocol.clone() },
            source_name: r.indexer.name.clone(),
            client: app.into(),
            category: a.category.clone(),
            paused: a.paused,
        };
        let done = self.hub.add(&g).await?;
        tracing::info!("{done}");
        Ok(Vec::new())
    }

    /// Posts the action's JSON to its address (autobrr's `runWebhook`).
    async fn run_webhook(&self, a: &Action) -> Result<(), String> {
        let resp = self
            .http
            .post(&a.webhook_host)
            .header("Content-Type", "application/json")
            .header("User-Agent", clients::user_agent())
            .body(a.webhook_data.clone())
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("could not make request for webhook: {}", clients::req_err("Post", &a.webhook_host, &e)))?;
        tracing::info!(
            "successfully ran webhook action: '{}' to: {} payload: {} status: {}",
            a.name,
            a.webhook_host,
            a.webhook_data,
            resp.status().as_u16()
        );
        Ok(())
    }

    /// The torrent's bytes, fetched onto the release when they aren't there yet.
    async fn torrent(&self, r: &mut Release) -> Result<Vec<u8>, String> {
        let name = r.torrent_name.clone();
        self.downloads.download_release(r).await.map_err(|e| format!("could not download torrent file for release: {name}: {e}"))?;
        Ok(r.torrent_data_raw_bytes.clone())
    }

    /// Deluge 1 or 2 (autobrr's `runDeluge`).
    async fn run_deluge(&self, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let mut c = deluge::Client::new(&cfg.host, cfg.port as u16, &cfg.username, &cfg.password, cfg.kind == "DELUGE_V2");
        c.connect().await.map_err(|e| format!("could not connect to client {} at {}: {e}", cfg.name, cfg.host))?;
        let out = self.deluge_add(&mut c, &cfg, a, r).await;
        c.close().await;
        out
    }

    /// Deluge's rules, add and label, on a connected client.
    async fn deluge_add(&self, c: &mut deluge::Client, cfg: &Downloader, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        if cfg.settings.rules.enabled && !a.ignore_rules {
            let n = c.downloading_count().await.map_err(|e| format!("could not fetch downloading torrents: {e}"))?;
            if cfg.settings.rules.max_active_downloads > 0 && n as i64 >= cfg.settings.rules.max_active_downloads {
                return Ok(vec!["max active downloads reached, skipping".into()]);
            }
        }
        let options = deluge_options(a);
        let hash = if has_magnet_uri(r) {
            c.add_torrent_magnet(&r.magnet_uri, &options)
                .await
                .map_err(|e| format!("could not add torrent magnet {} to client: {}: {e}", r.magnet_uri, cfg.name))?
        } else {
            let bytes = self.torrent(r).await?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            if b64.is_empty() {
                return Err(format!("could not encode torrent file for release: {}", r.torrent_name));
            }
            c.add_torrent_file(&format!("{}.torrent", r.torrent_hash), &b64, &options)
                .await
                .map_err(|e| format!("could not add torrent {} to client: {}: {e}", r.torrent_name, cfg.name))?
        };
        if !a.label.is_empty() {
            let plugins = c.enabled_plugins().await.map_err(|e| format!("could not load label plugin for client: {}: {e}", cfg.name))?;
            if plugins.iter().any(|p| p == "Label") {
                let set = |e: deluge::Error| format!("could not set label: {} on client: {}: {e}", a.label, cfg.name);
                match c.set_torrent_label(&hash, &a.label).await {
                    Ok(()) => {}
                    Err(deluge::Error::Rpc(e)) if e.exception_message == "Unknown Label" => {
                        c.add_label(&a.label).await.map_err(|e| {
                            set(deluge::Error::Other(format!("could not add label: {} on client: {}: {e}", a.label, cfg.name)))
                        })?;
                        c.set_torrent_label(&hash, &a.label).await.map_err(|e| set(deluge::Error::Other(set(e))))?;
                    }
                    Err(e) => return Err(set(deluge::Error::Other(set(e)))),
                }
            }
        }
        tracing::info!(hash = %hash, client = %cfg.name, "release successfully added to client");
        Ok(Vec::new())
    }

    /// qBittorrent (autobrr's `runQbittorrent`).
    async fn run_qbittorrent(&self, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let c = qbit_client(&cfg)?;
        let rules = &cfg.settings.rules;
        if rules.enabled && !a.ignore_rules {
            let rej = self.qbit_rules(&c, rules).await.map_err(|e| format!("error checking client rules: {}: {e}", a.name))?;
            if !rej.is_empty() {
                return Ok(rej);
            }
        }
        let options = qbit_options(a).prepare();
        if has_magnet_uri(r) {
            c.add_torrent(None, &options, &r.magnet_uri)
                .await
                .map_err(|e| format!("could not add torrent {} to client: {}: {e}", r.magnet_uri, cfg.name))?;
            return Ok(Vec::new());
        }
        let bytes = self.torrent(r).await?;
        c.add_torrent(Some(&bytes), &options, &r.torrent_name)
            .await
            .map_err(|e| format!("could not add torrent {} to client: {}: {e}", r.torrent_name, cfg.name))?;
        if !r.torrent_hash.is_empty() && (a.priority_layout == "MAX" || a.priority_layout == "MIN") {
            let on = c
                .queueing_enabled()
                .await
                .map_err(|e| format!("could not get application preferences from client: '{}': {e}", cfg.name))?;
            if !on {
                c.set_queueing_enabled().await.map_err(|e| format!("could not enable torrent queueing: {e}"))?;
            }
            let max = a.priority_layout == "MAX";
            c.set_priority(&r.torrent_hash, max)
                .await
                .map_err(|e| format!("could not set torrent {} to {} priority: {e}", r.torrent_hash, if max { "max" } else { "min" }))?;
        }
        if !a.paused && !a.re_announce_skip && !r.torrent_hash.is_empty() {
            let opts = qbittorrent::ReannounceOptions {
                interval: a.re_announce_interval,
                max_attempts: a.re_announce_max_attempts,
                delete_on_failure: a.re_announce_delete,
            };
            match c.reannounce_with_retry(&r.torrent_hash, opts).await {
                Ok(qbittorrent::Reannounce::Ok) => {}
                Ok(qbittorrent::Reannounce::TookTooLong) => {
                    return Ok(vec![format!("re-announce took too long for hash: {}", r.torrent_hash)]);
                }
                Err(e) => return Err(format!("could not reannounce torrent: {}: {e}", r.torrent_hash)),
            }
        }
        Ok(Vec::new())
    }

    /// qBittorrent's rules (autobrr's `qbittorrentCheckRulesCanDownload`).
    async fn qbit_rules(&self, c: &qbittorrent::Client, rules: &super::downloader::DownloaderRules) -> Result<Vec<String>, String> {
        if rules.max_active_downloads > 0 {
            let active = c.get_torrents_active_downloads().await.map_err(|e| format!("could not fetch active downloads: {e}"))?;
            if active as i64 >= rules.max_active_downloads {
                if !rules.ignore_slow_torrents {
                    return Ok(vec!["max active downloads reached, skipping".into()]);
                }
                if rules.ignore_slow_torrents_condition == "MAX_DOWNLOADS_REACHED" {
                    let (dl, ul) = c.get_transfer_info().await.map_err(|e| format!("could not get transfer info: {e}"))?;
                    return Ok(qbit_check_ignore_slow(rules.download_speed_threshold, rules.upload_speed_threshold, dl, ul));
                }
            }
        }
        if rules.ignore_slow_torrents && rules.ignore_slow_torrents_condition == "ALWAYS" {
            let (dl, ul) = c.get_transfer_info().await.map_err(|e| format!("could not get transfer info: {e}"))?;
            return Ok(qbit_check_ignore_slow(rules.download_speed_threshold, rules.upload_speed_threshold, dl, ul));
        }
        Ok(Vec::new())
    }

    /// rTorrent (autobrr's `runRTorrent`).
    pub async fn run_rtorrent(&self, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let c = rtorrent_client(&cfg);
        let mut args = rtorrent_args(a);
        if !a.ratio_group.is_empty() {
            let views = c.views().await.map_err(|e| format!("could not get views from client: {}: {e}", cfg.name))?;
            if !views.contains(&a.ratio_group) {
                return Err(format!(
                    "could not find ratio group '{}' on client: {}, check that the ruTorrent ratio plugin is enabled",
                    a.ratio_group, cfg.name
                ));
            }
            args.push(rtorrent::FieldValue::command("view.set_visible", &a.ratio_group));
        }
        if has_magnet_uri(r) {
            let res = if a.paused { c.add_url_stopped(&r.magnet_uri, &args).await } else { c.add_url(&r.magnet_uri, &args).await };
            res.map_err(|e| format!("could not add torrent from magnet: {}: {e}", r.magnet_uri))?;
            return Ok(Vec::new());
        }
        let bytes = self.torrent(r).await?;
        let res = if a.paused { c.add_torrent_stopped(&bytes, &args).await } else { c.add_torrent(&bytes, &args).await };
        res.map_err(|e| format!("could not add torrent file: {}: {e}", r.torrent_name))?;
        Ok(Vec::new())
    }

    /// Transmission (autobrr's `runTransmission`).
    async fn run_transmission(&self, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let c = transmission_client(&cfg)?;
        if cfg.settings.rules.enabled && !a.ignore_rules {
            let list = c
                .torrent_get(&["status"], &[])
                .await
                .map_err(|e| format!("error checking client rules: {}: could not fetch active downloads: {e}", a.name))?;
            let active = list.iter().filter(|t| t.get("status").and_then(Value::as_i64) == Some(4)).count() as i64;
            if cfg.settings.rules.max_active_downloads > 0 && active >= cfg.settings.rules.max_active_downloads {
                return Ok(vec!["max active downloads reached, skipping".into()]);
            }
        }
        let mut payload = Map::new();
        if !a.save_path.is_empty() {
            payload.insert("download-dir".into(), json!(a.save_path));
        }
        if a.paused {
            payload.insert("paused".into(), json!(true));
        }
        if has_magnet_uri(r) {
            payload.insert("filename".into(), json!(r.magnet_uri));
        } else {
            let bytes = self.torrent(r).await?;
            payload.insert("metainfo".into(), json!(base64::engine::general_purpose::STANDARD.encode(&bytes)));
        }
        let t =
            c.torrent_add(payload).await.map_err(|e| format!("could not add torrent {} to client: {}: {e}", r.torrent_name, cfg.host))?;
        if !a.label.is_empty() || a.limit_upload_speed > 0 || a.limit_download_speed > 0 || a.limit_ratio > 0.0 || a.limit_seed_time > 0 {
            let mut p = Map::new();
            p.insert("ids".into(), json!([t.id]));
            if !a.label.is_empty() {
                let labels: Vec<&str> = a.label.split(',').map(str::trim).filter(|l| !l.is_empty()).collect();
                p.insert("labels".into(), json!(labels));
            }
            if a.limit_upload_speed > 0 {
                p.insert("uploadLimit".into(), json!(a.limit_upload_speed));
                p.insert("uploadLimited".into(), json!(true));
            }
            if a.limit_download_speed > 0 {
                p.insert("downloadLimit".into(), json!(a.limit_download_speed));
                p.insert("downloadLimited".into(), json!(true));
            }
            if a.limit_ratio > 0.0 {
                p.insert("seedRatioLimit".into(), json!(a.limit_ratio));
                p.insert("seedRatioMode".into(), json!(1));
            }
            if a.limit_seed_time > 0 {
                p.insert("seedIdleLimit".into(), json!(a.limit_seed_time));
                p.insert("seedIdleMode".into(), json!(1));
            }
            c.torrent_set(p).await.map_err(|e| format!("could not set label for hash {} to client: {}: {e}", t.hash_string, cfg.host))?;
        }
        if !a.paused && !a.re_announce_skip {
            return match transmission_reannounce(&c, a, t.id).await {
                Ok(false) => Ok(Vec::new()),
                Ok(true) => Ok(vec![format!("reannounce took too long for torrent: {}, deleted", t.hash_string)]),
                Err(e) => Err(format!("could not reannounce torrent: {}: {e}", t.hash_string)),
            };
        }
        Ok(Vec::new())
    }

    /// Porla (autobrr's `runPorla`).
    async fn run_porla(&self, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let (bu, bp) = basic(&cfg);
        let c = porla::Client::new(&cfg.host, &cfg.settings.api_key, cfg.tls_skip_verify, &bu, &bp);
        if cfg.settings.rules.enabled && !a.ignore_rules {
            let n = c
                .torrents_list("state:downloading flags:~paused")
                .await
                .map_err(|e| format!("error checking Porla client rules: {}: could not fetch active downloads: {e}", a.name))?;
            if cfg.settings.rules.max_active_downloads > 0 && n as i64 >= cfg.settings.rules.max_active_downloads {
                return Ok(vec!["max active downloads reached, skipping".into()]);
            }
        }
        let mut req = porla::TorrentsAddReq {
            save_path: a.save_path.clone(),
            download_limit: (a.limit_download_speed > 0).then(|| a.limit_download_speed * 1000),
            upload_limit: (a.limit_upload_speed > 0).then(|| a.limit_upload_speed * 1000),
            preset: (!a.label.is_empty()).then(|| a.label.clone()),
            ..Default::default()
        };
        if has_magnet_uri(r) {
            req.magnet_uri = r.magnet_uri.clone();
        } else {
            let bytes = self.torrent(r).await?;
            req.ti = base64::engine::general_purpose::STANDARD.encode(&bytes);
        }
        c.torrents_add(&req).await.map_err(|e| format!("could not add torrent {} to client: {}: {e}", r.torrent_name, cfg.name))?;
        Ok(Vec::new())
    }

    /// aria2 (autobrr's `runAria2`).
    async fn run_aria2(&self, a: &Action, r: &mut Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let c = aria2_client(&cfg)?;
        let rules = &cfg.settings.rules;
        if rules.enabled && !a.ignore_rules && rules.max_active_downloads > 0 {
            let active = c
                .tell_active()
                .await
                .map_err(|e| format!("error checking aria2 client rules: {}: could not fetch active downloads: {e}", a.name))?;
            if active.iter().filter(|d| d.downloading()).count() as i64 >= rules.max_active_downloads {
                return Ok(vec!["max active downloads reached, skipping".into()]);
            }
        }
        let options = aria2_options(a);
        if has_magnet_uri(r) {
            c.add_uri(std::slice::from_ref(&r.magnet_uri), options)
                .await
                .map_err(|e| format!("could not add torrent from magnet {} to client: {}: {e}", r.magnet_uri, cfg.name))?;
        } else {
            let bytes = self.torrent(r).await?;
            c.add_torrent(&bytes, options)
                .await
                .map_err(|e| format!("could not add torrent {} to client: {}: {e}", r.torrent_name, cfg.name))?;
        }
        Ok(Vec::new())
    }

    /// SABnzbd and NZBGet (autobrr's `runSabnzbd`, `runNzbget`).
    async fn run_usenet(&self, a: &Action, r: &Release) -> Result<Vec<String>, String> {
        if r.protocol != "usenet" {
            return Err(format!("action type: {} invalid protocol: {}", a.kind, r.protocol));
        }
        let cfg = enabled(self.client_of(a)?)?;
        if a.kind == "SABNZBD" {
            let (bu, bp) = basic(&cfg);
            sabnzbd::Client::new(&cfg.host, &cfg.settings.api_key, cfg.tls_skip_verify, &bu, &bp)
                .add_from_url(&r.download_url, &a.category)
                .await
                .map_err(|e| format!("could not add nzb to sabnzbd: {e}"))?;
        } else {
            nzbget::Client::new(&cfg.host, &cfg.username, &cfg.password, cfg.tls_skip_verify)
                .add_from_url(&r.download_url, &a.category)
                .await
                .map_err(|e| format!("could not add nzb to nzbget: {e}"))?;
        }
        Ok(Vec::new())
    }

    /// Sonarr, Radarr, Lidarr, Readarr, Whisparr and Sportarr (autobrr's `runSonarr` …).
    async fn run_arr(&self, app: arr::App, a: &Action, r: &Release) -> Result<Vec<String>, String> {
        let cfg = enabled(self.client_of(a)?)?;
        let body = arr_push_body(app, &cfg, a, r, chrono::Utc::now());
        let wrap = match app {
            arr::App::Radarr => "radarr failed to push release",
            arr::App::Lidarr => "lidarr failed to push release",
            arr::App::Sonarr => "sonarr: failed to push release",
            arr::App::Readarr => "readarr: failed to push release",
            arr::App::Whisparr(_) => "whisparr: failed to push release",
            arr::App::Sportarr => "sportarr: failed to push release",
        };
        arr_client(app, &cfg).push(&body).await.map_err(|e| format!("{wrap}: {}: {e}", r.torrent_name))
    }
}

/// Re-announces until a tracker has peers (autobrr's `transmissionReannounce`); whether it gave up and removed
/// the torrent.
async fn transmission_reannounce(c: &transmission::Client, a: &Action, id: i64) -> Result<bool, String> {
    let interval = if a.re_announce_interval > 0 { a.re_announce_interval } else { REANNOUNCE_INTERVAL };
    let max = if a.re_announce_max_attempts > 0 { a.re_announce_max_attempts } else { REANNOUNCE_MAX_ATTEMPTS };
    let mut attempts = 0;
    while attempts <= max {
        tokio::time::sleep(Duration::from_secs(interval as u64)).await;
        let t = c.torrent_get(&["trackerStats"], &[id]).await.map_err(|e| format!("reannounced, failed to find torrentid: {e}"))?;
        let Some(t) = t.first() else { return Err("reannounced, failed to get torrent from id".into()) };
        for tr in t.get("trackerStats").and_then(Value::as_array).cloned().unwrap_or_default() {
            if tr.get("isBackup").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            if is_unregistered(tr.get("lastAnnounceResult").and_then(Value::as_str).unwrap_or("")) {
                continue;
            }
            let n = |k: &str| tr.get(k).and_then(Value::as_i64).unwrap_or(0);
            if n("seederCount") > 0 || n("leecherCount") > 0 {
                return Ok(false);
            }
        }
        c.torrent_reannounce(&[id]).await.map_err(|e| format!("failed to reannounce: {e}"))?;
        attempts += 1;
    }
    if attempts == max && a.re_announce_delete {
        c.torrent_remove(&[id])
            .await
            .map_err(|e| format!("could not delete torrent: {id} from client after max re-announce attempts reached: {e}"))?;
        return Ok(true);
    }
    Ok(false)
}

/// Fetches torrents over HTTP as autobrr's download service does: three tries, three seconds apart (or as a
/// `Retry-After` says), the file checked to be a torrent and its hash and size read.
pub struct HttpDownloader {
    /// The client used.
    pub http: reqwest::Client,
}

impl HttpDownloader {
    /// A downloader that accepts any certificate, as autobrr's does.
    pub fn new() -> HttpDownloader {
        HttpDownloader { http: clients::http(Duration::from_secs(30), true, false) }
    }
}

impl Default for HttpDownloader {
    fn default() -> Self {
        HttpDownloader::new()
    }
}

/// What one try at fetching a torrent ended with.
enum Try {
    /// Try again after this long.
    Again(String, Duration),
    /// Don't try again.
    Stop(String),
}

impl HttpDownloader {
    /// One try at fetching `r`'s torrent.
    async fn try_once(&self, r: &mut Release) -> Result<(), Try> {
        let ua = "autobrr";
        let resp = self.http.get(&r.download_url).header("User-Agent", ua).send().await.map_err(|e| {
            let m = clients::req_err("Get", &r.download_url, &e);
            if e.is_builder() {
                Try::Stop(format!("url parse error: {m}"))
            } else if e.is_connect() {
                Try::Stop(format!("issue from proxy: {m}"))
            } else {
                Try::Again(format!("error downloading file: {m}"), Duration::from_secs(3))
            }
        })?;
        let code = resp.status().as_u16();
        let (n, url, idx) = (r.torrent_name.clone(), r.download_url.clone(), r.indexer.name.clone());
        match code {
            200 => {}
            401 | 403 => {
                return Err(Try::Stop(format!(
                    "unrecoverable error downloading torrent ({n}) file ({url}) - status code: {code} - check indexer keys for {idx}"
                )))
            }
            405 => {
                return Err(Try::Stop(format!(
                    "unrecoverable error downloading torrent ({n}) file ({url}) from '{idx}' - status code: {code}. Check if the request method is correct"
                )))
            }
            404 => return Err(Try::Again(format!("torrent {n} not found on {idx} ({code}) - retrying"), Duration::from_secs(3))),
            502..=504 => {
                return Err(Try::Again(
                    format!("server error ({code}) encountered while downloading torrent ({n}) file ({url}) from '{idx}' - retrying"),
                    Duration::from_secs(3),
                ))
            }
            500 => {
                return Err(Try::Again(
                    format!("server error ({code}) encountered while downloading torrent ({n}) file ({url}) - check indexer keys for {idx}"),
                    Duration::from_secs(3),
                ))
            }
            429 => {
                let after = resp.headers().get("Retry-After").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                if after.is_empty() {
                    return Err(Try::Again(
                        format!("rate-limit reached ({code}) while downloading torrent ({n}) file ({url}) indexer ({idx}), retrying in 3 seconds..."),
                        Duration::from_secs(3),
                    ));
                }
                if let Ok(secs) = after.parse::<i64>() {
                    if secs <= 0 {
                        return Err(Try::Stop(format!("rate-limit reached ({code}) while downloading torrent ({n}) file ({url}) indexer ({idx})")));
                    }
                    if secs > 7200 {
                        return Err(Try::Stop(format!(
                            "rate-limit reached ({code}) while downloading torrent ({n}) file ({url}) indexer ({idx}) retry-after {secs} seconds is higher than allowed limit of 2h, aborting"
                        )));
                    }
                    return Err(Try::Again(
                        format!("rate-limit reached ({code}) while downloading torrent ({n}) file ({url}) indexer ({idx}), retrying in {secs} seconds"),
                        Duration::from_secs(secs as u64),
                    ));
                }
            }
            _ => return Err(Try::Stop(format!("unexpected status code {code}: check indexer keys for {idx}"))),
        }
        let body = resp.bytes().await.map_err(|e| Try::Again(format!("error reading response body: {e}"), Duration::from_secs(3)))?;
        if body.first() != Some(&b'd') {
            return Err(Try::Again(
                format!("metainfo unexpected content type, got HTML expected a bencoded torrent. check indexer keys for {idx} - {n}"),
                Duration::from_secs(3),
            ));
        }
        let hash = crate::hub::torrent::info_hash(&body)
            .ok_or_else(|| Try::Stop(format!("metainfo unexpected content type. check indexer keys for {idx} - {n}")))?;
        if let Some(size) = super::torrent::size(&body).filter(|s| *s > 0) {
            r.size = size;
        }
        r.torrent_data_raw_bytes = body.to_vec();
        r.torrent_hash = hash;
        Ok(())
    }
}

impl ReleaseDownloader for HttpDownloader {
    fn download_release<'a>(&'a self, r: &'a mut Release) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            if has_magnet_uri(r) {
                return Err(format!("downloading magnet links is not supported: {}", r.magnet_uri));
            }
            if r.protocol != "torrent" {
                return Err(format!("could not download file: protocol {} is not supported", r.protocol));
            }
            if r.download_url.is_empty() {
                return Err("download_file: url can't be empty".into());
            }
            if !r.torrent_data_raw_bytes.is_empty() {
                return Ok(());
            }
            let mut last = String::new();
            for attempt in 0..3 {
                match self.try_once(r).await {
                    Ok(()) => return Ok(()),
                    Err(Try::Stop(e)) => return Err(e),
                    Err(Try::Again(e, wait)) => {
                        last = e;
                        if attempt < 2 {
                            tokio::time::sleep(wait).await;
                        }
                    }
                }
            }
            Err(last)
        })
    }

    fn resolve_magnet_uri<'a>(&'a self, r: &'a mut Release) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            if r.magnet_uri.is_empty() || r.magnet_uri.starts_with(MAGNET_URI_PREFIX) {
                return Ok(());
            }
            let resp = self
                .http
                .get(&r.magnet_uri)
                .header("User-Agent", "autobrr")
                .timeout(Duration::from_secs(45))
                .send()
                .await
                .map_err(|e| format!("could not make request to resolve magnet uri: {e}"))?;
            if resp.status().as_u16() != 200 {
                return Err(format!("unexpected status code: {}", resp.status().as_u16()));
            }
            let body = resp.text().await.map_err(|e| format!("could not read response body: {e}"))?;
            let magnet = body.trim();
            r.magnet_uri = if magnet.starts_with(MAGNET_URI_PREFIX) { magnet.to_string() } else { String::new() };
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/action tests: arr_test.go, run_test.go, run_rtorrent_test.go, run_exec_test.go,
    //! run_watchfolder_test.go; and internal/domain/release_torrent_test.go's temporary file cases.
    use super::*;
    use crate::releases::downloader::DownloaderSettings;
    use crate::releases::push::NoHub;
    use crate::releases::release::IndexerMinimal;
    use axum::routing::post;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A fresh folder for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lantern-run-{name}-{}-{}", std::process::id(), hex::encode(rand::random::<[u8; 4]>())));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// run_test.go's `fakeDownloadService`: counts real fetches.
    #[derive(Default)]
    struct FakeDownloads {
        fetches: AtomicUsize,
    }

    impl ReleaseDownloader for FakeDownloads {
        fn download_release<'a>(&'a self, r: &'a mut Release) -> BoxFut<'a, Result<(), String>> {
            Box::pin(async move {
                if !r.torrent_data_raw_bytes.is_empty() {
                    return Ok(());
                }
                self.fetches.fetch_add(1, Ordering::SeqCst);
                r.torrent_data_raw_bytes = b"d8:announce0:4:infod4:name4:test6:lengthi1eee".to_vec();
                r.torrent_hash = "abc123".into();
                Ok(())
            })
        }

        fn resolve_magnet_uri<'a>(&'a self, _: &'a mut Release) -> BoxFut<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn service(d: Downloader, dl: Arc<FakeDownloads>) -> Service {
        Service {
            downloads: dl,
            downloaders: Arc::new(move |_| Ok(d.clone())),
            hub: Arc::new(NoHub),
            http: reqwest::Client::new(),
            tmp_dir: temp_dir("tmp"),
        }
    }

    fn torrent_release() -> Release {
        Release { torrent_name: "Test.Release-GRP".into(), protocol: "torrent".into(), ..Release::default() }
    }

    fn action(kind: &str) -> Action {
        Action { kind: kind.into(), ..Action::default() }
    }

    #[test]
    fn arr_download_client_cases() {
        let d = |id: i64, name: &str| Downloader {
            settings: DownloaderSettings { external_download_client_id: id, external_download_client: name.into(), ..Default::default() },
            ..Downloader::default()
        };
        let a =
            |id: i64, name: &str| Action { external_download_client_id: id, external_download_client: name.into(), ..Action::default() };
        assert_eq!(arr_download_client(&d(1, "qbit"), &a(0, "")), (1, "qbit".into()), "client_settings");
        assert_eq!(arr_download_client(&d(1, ""), &a(0, "qbit-2")), (0, "qbit-2".into()), "action_name_replaces_client_id");
        assert_eq!(arr_download_client(&d(0, "qbit"), &a(2, "")), (2, String::new()), "action_id_replaces_client_name");
        assert_eq!(arr_download_client(&d(1, "qbit"), &a(2, "qbit-2")), (2, "qbit-2".into()), "action_pair");
        assert_eq!(arr_download_client(&d(0, ""), &a(0, "")), (0, String::new()), "none");
    }

    #[test]
    fn arr_publish_date_cases() {
        let now = chrono::Utc::now();
        let at = |s: &str| Release { publish_date: Some(chrono::DateTime::parse_from_rfc3339(s).unwrap().to_utc()), ..Release::default() };
        assert_eq!(arr_publish_date(&at("2026-09-24T05:58:24Z"), now), "2026-09-24T05:58:24Z");
        assert_eq!(arr_publish_date(&at("2026-09-24T06:58:24+01:00"), now), "2026-09-24T05:58:24Z", "formats a non utc date in utc");
        assert_eq!(arr_publish_date(&at("1970-04-01T00:00:01Z"), now), "1970-04-01T00:00:01Z");
        let want = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        for r in [
            Release::default(),
            at("1969-12-31T00:00:00Z"),
            at("1970-01-01T00:00:00Z"),
            at("1970-04-01T00:00:00Z"),
            Release { publish_date: Some(now + chrono::Duration::hours(2)), ..Release::default() },
        ] {
            assert_eq!(arr_publish_date(&r, now), want);
        }
    }

    #[tokio::test]
    async fn preconditions_download_once_per_release() {
        let dl = Arc::new(FakeDownloads::default());
        let s = service(Downloader::default(), dl.clone());
        let mut r = torrent_release();
        for kind in ["QBITTORRENT", "DELUGE_V2", "WATCH_FOLDER"] {
            s.check_action_preconditions(&action(kind), &mut r).await.unwrap();
        }
        assert_eq!(dl.fetches.load(Ordering::SeqCst), 1, "torrent should be fetched from the indexer once for all actions");
        assert!(!r.torrent_data_raw_bytes.is_empty());
    }

    #[tokio::test]
    async fn preconditions_skip_download_for_magnet() {
        let dl = Arc::new(FakeDownloads::default());
        let s = service(Downloader::default(), dl.clone());
        let mut r = Release { magnet_uri: "magnet:?xt=urn:btih:abc123".into(), ..torrent_release() };
        s.check_action_preconditions(&action("QBITTORRENT"), &mut r).await.unwrap();
        assert_eq!(dl.fetches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preconditions_skip_download_for_arr_actions() {
        let dl = Arc::new(FakeDownloads::default());
        let s = service(Downloader::default(), dl.clone());
        s.check_action_preconditions(&action("RADARR"), &mut torrent_release()).await.unwrap();
        assert_eq!(dl.fetches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preconditions_write_tmp_file_for_path_macro() {
        let dl = Arc::new(FakeDownloads::default());
        let s = service(Downloader::default(), dl.clone());
        let mut r = torrent_release();
        let a = Action { exec_args: "--torrent {{.TorrentPathName}}".into(), ..action("EXEC") };
        s.check_action_preconditions(&a, &mut r).await.unwrap();
        assert_eq!(dl.fetches.load(Ordering::SeqCst), 1);
        assert!(!r.torrent_tmp_file.is_empty(), "path macro requires a tmp file on disk");
        cleanup_temporary_files(&mut r);
    }

    #[tokio::test]
    async fn preconditions_skip_download_for_plain_exec() {
        let dl = Arc::new(FakeDownloads::default());
        let s = service(Downloader::default(), dl.clone());
        let mut r = torrent_release();
        let a = Action { exec_args: "--name {{.TorrentName}}".into(), ..action("EXEC") };
        s.check_action_preconditions(&a, &mut r).await.unwrap();
        assert_eq!(dl.fetches.load(Ordering::SeqCst), 0);
        assert!(r.torrent_tmp_file.is_empty());
    }

    #[test]
    fn temporary_files() {
        let dir = temp_dir("tmpfile");
        let mut r =
            Release { torrent_name: "Test.Release-GRP".into(), torrent_data_raw_bytes: b"torrent contents".to_vec(), ..Release::default() };
        write_temporary_file(&mut r, &dir).unwrap();
        assert_eq!(std::fs::read(&r.torrent_tmp_file).unwrap(), b"torrent contents");
        let first = r.torrent_tmp_file.clone();
        write_temporary_file(&mut r, &dir).unwrap();
        assert_eq!(first, r.torrent_tmp_file, "writing again is a no-op so several actions share the one file");
        cleanup_temporary_files(&mut r);
        assert!(!std::path::Path::new(&first).exists(), "tmp file should be removed");
        assert!(r.torrent_tmp_file.is_empty());
        assert!(r.torrent_data_raw_bytes.is_empty(), "torrent should not be held after processing");

        let mut empty = Release { torrent_name: "Test.Release-GRP".into(), ..Release::default() };
        assert!(write_temporary_file(&mut empty, &dir).is_err(), "writing a tmp file without the torrent should fail loudly");
        assert!(empty.torrent_tmp_file.is_empty());

        let mut gone = Release { torrent_data_raw_bytes: b"x".to_vec(), ..Release::default() };
        write_temporary_file(&mut gone, &dir).unwrap();
        std::fs::remove_file(&gone.torrent_tmp_file).unwrap();
        cleanup_temporary_files(&mut gone);
        assert!(gone.torrent_tmp_file.is_empty(), "an already removed tmp file is not an error");
    }

    #[test]
    fn parse_macros_keeps_quotes() {
        let r = Release { torrent_name: "Sally Goes to the Mall S04E29".into(), ..Release::default() };
        let cases = [
            (r#"echo "{{ .TorrentName }}""#, r#"echo "Sally Goes to the Mall S04E29""#),
            (r#""{{ .TorrentName }}""#, r#""Sally Goes to the Mall S04E29""#),
            (
                r#"--header "Content-Type: application/json" --request POST --data '{"release":"{{ .TorrentName }}"}' http://localhost:3000/api/release"#,
                r#"--header "Content-Type: application/json" --request POST --data '{"release":"Sally Goes to the Mall S04E29"}' http://localhost:3000/api/release"#,
            ),
        ];
        for (args, want) in cases {
            let mut a = Action { exec_args: args.into(), ..Action::default() };
            a.parse_macros(&r).unwrap();
            assert_eq!(a.exec_args, want);
        }
    }

    #[tokio::test]
    async fn exec_runs_with_an_argument_list() {
        run_exec(&Action { name: "echo".into(), exec_cmd: "echo".into(), exec_args: "hello".into(), ..Action::default() }).await.unwrap();
        let dir = temp_dir("exec");
        let out = dir.join("out file.txt");
        // the argument with a space and a `;` reaches the program whole: no shell reads it
        let a = Action {
            exec_cmd: "touch".into(),
            exec_args: format!("'{}' '{d}/a;b' '{d}/$(id)'", out.display(), d = dir.display()),
            ..Action::default()
        };
        run_exec(&a).await.unwrap();
        assert!(out.exists());
        assert!(dir.join("a;b").exists());
        assert!(dir.join("$(id)").exists());
        let e = run_exec(&Action { exec_cmd: "no-such-program-here".into(), ..Action::default() }).await.unwrap_err();
        assert_eq!(
            e,
            "exec failed, could not find program: no-such-program-here: exec: \"no-such-program-here\": executable file not found in $PATH"
        );
        let e = run_exec(&Action { exec_cmd: "echo".into(), exec_args: "'open".into(), ..Action::default() }).await.unwrap_err();
        assert_eq!(e, "could not parse exec args: 'open: unterminated single-quoted string");
    }

    const TEST_INFO_HASH: &str = "9c8f7e6d5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d";

    fn watch_release() -> Release {
        Release {
            torrent_name: "Some.Movie.2024.1080p-GRP".into(),
            torrent_hash: TEST_INFO_HASH.into(),
            protocol: "torrent".into(),
            torrent_data_raw_bytes: b"d8:announce0:4:infod4:name4:teste".to_vec(),
            ..Release::default()
        }
    }

    #[test]
    fn watch_folder_cases() {
        let dir = temp_dir("watch");
        let r = watch_release();
        run_watch_folder(&Action { watch_folder: dir.display().to_string(), ..Action::default() }, &r).unwrap();
        let want = dir.join(format!("autobrr-{TEST_INFO_HASH}.torrent"));
        assert_eq!(std::fs::read(&want).unwrap(), r.torrent_data_raw_bytes, "the torrent must be written verbatim");

        let target = dir.join("nested").join("chosen-name.torrent");
        run_watch_folder(&Action { watch_folder: target.display().to_string(), ..Action::default() }, &r).unwrap();
        assert!(target.exists());

        let magnet = Release { magnet_uri: format!("magnet:?xt=urn:btih:{TEST_INFO_HASH}"), ..watch_release() };
        assert!(run_watch_folder(&Action { watch_folder: dir.display().to_string(), ..Action::default() }, &magnet).is_err());

        let empty_dir = temp_dir("watch-empty");
        let missing = Release { torrent_data_raw_bytes: Vec::new(), ..watch_release() };
        let e = run_watch_folder(&Action { watch_folder: empty_dir.display().to_string(), ..Action::default() }, &missing).unwrap_err();
        assert_eq!(e, "watch_folder: missing torrent Some.Movie.2024.1080p-GRP");
        assert_eq!(std::fs::read_dir(&empty_dir).unwrap().count(), 0, "no file should be created when the torrent is missing");

        let bad = Release { torrent_hash: "../../etc/passwd".into(), ..watch_release() };
        let e = run_watch_folder(&Action { watch_folder: dir.display().to_string(), ..Action::default() }, &bad).unwrap_err();
        assert_eq!(e, "watch_folder: invalid torrent hash");
    }

    /// One recorded XML-RPC call: its method and string parameters.
    type Call = (String, Vec<String>);
    /// The calls a stand-in rTorrent got.
    type Calls = Arc<Mutex<Vec<Call>>>;

    /// run_rtorrent_test.go's stand-in: answers `view.list` with the ratio views and records every call.
    async fn rtorrent_server() -> (String, Calls) {
        let calls: Calls = Arc::default();
        let c = calls.clone();
        let app = axum::Router::new().route(
            "/RPC2",
            post(move |body: String| {
                let c = c.clone();
                async move {
                    let method = regex::Regex::new(r"<methodName>(.*?)</methodName>").unwrap().captures(&body).unwrap()[1].to_string();
                    let params: Vec<String> = regex::Regex::new(r"<value><string>(.*?)</string></value>|<value><string/></value>")
                        .unwrap()
                        .captures_iter(&body)
                        .map(|m| html_escape::decode_html_entities(m.get(1).map(|x| x.as_str()).unwrap_or("")).into_owned())
                        .collect();
                    let list = method == "view.list";
                    c.lock().unwrap().push((method, params));
                    if list {
                        r#"<?xml version="1.0"?><methodResponse><params><param><value><array><data><value><string>main</string></value><value><string>rat_0</string></value><value><string>rat_1</string></value></data></array></value></param></params></methodResponse>"#
                    } else {
                        r#"<?xml version="1.0"?><methodResponse><params><param><value><i8>0</i8></value></param></params></methodResponse>"#
                    }
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}/RPC2"), calls)
    }

    #[tokio::test]
    async fn rtorrent_ratio_group_and_priority() {
        let call = |m: &str, p: &[&str]| (m.to_string(), p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let cases: Vec<(Action, Vec<Call>, &str)> = vec![
            (Action::default(), vec![call("load.start", &[""])], ""),
            (Action { priority_layout: "HIGH".into(), ..Action::default() }, vec![call("load.start", &["", r#"d.priority.set="3""#])], ""),
            (Action { priority_layout: "MAX".into(), ..Action::default() }, vec![call("load.start", &[""])], ""),
            (
                Action { label: "tv".into(), ratio_group: "rat_1".into(), ..Action::default() },
                vec![call("view.list", &[]), call("load.start", &["", r#"d.custom1.set="tv""#, r#"view.set_visible="rat_1""#])],
                "",
            ),
            (
                Action { ratio_group: "rat_5".into(), ..Action::default() },
                vec![call("view.list", &[])],
                "could not find ratio group 'rat_5' on client: rtorrent, check that the ruTorrent ratio plugin is enabled",
            ),
        ];
        for (mut a, want, want_err) in cases {
            let (host, calls) = rtorrent_server().await;
            let d = Downloader { id: 1, name: "rtorrent".into(), kind: "RTORRENT".into(), enabled: true, host, ..Downloader::default() };
            let s = service(d, Arc::new(FakeDownloads::default()));
            a.client_id = 1;
            let mut r = Release {
                torrent_name: "Test.Release-GRP".into(),
                magnet_uri: "magnet:?xt=urn:btih:3f9aac158c7de8dfcab171ea58a17aabdf7fbc93".into(),
                ..Release::default()
            };
            let res = s.run_rtorrent(&a, &mut r).await;
            match want_err {
                "" => assert!(res.is_ok(), "{res:?}"),
                e => assert_eq!(res.unwrap_err(), e),
            }
            assert_eq!(*calls.lock().unwrap(), want);
        }
    }

    /// arr_test.go's stand-in Sonarr: refuses an indexer it doesn't know, or records the raw body.
    async fn sonarr_server(known: &'static str) -> (String, Arc<Mutex<Vec<Value>>>) {
        let got: Arc<Mutex<Vec<Value>>> = Arc::default();
        let g = got.clone();
        let app = axum::Router::new().route(
            "/api/v3/release/push",
            post(move |axum::Json(v): axum::Json<Value>| {
                let g = g.clone();
                async move {
                    let indexer = v["indexer"].as_str().unwrap_or("").to_string();
                    g.lock().unwrap().push(v);
                    if !indexer.is_empty() && indexer != known {
                        let body = format!(r#"{{"message":"Indexer with name '{indexer}' could not be found"}}"#);
                        return (axum::http::StatusCode::BAD_REQUEST, body);
                    }
                    (axum::http::StatusCode::OK, r#"[{"approved":true,"rejected":false,"rejections":[]}]"#.to_string())
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}"), got)
    }

    fn sonarr_release() -> Release {
        Release {
            torrent_name: "WeCrashed.S01E01.DV.2160p.ATVP.WEB-DL.DDPA5.1.x265-NOSiViD".into(),
            download_url: "https://mock.local/download".into(),
            protocol: "torrent".into(),
            indexer: IndexerMinimal {
                id: 0,
                name: "IPTorrents".into(),
                identifier: "ipt".into(),
                identifier_external: "IPTorrents".into(),
            },
            ..Release::default()
        }
    }

    fn sonarr_service(host: String) -> Service {
        let d = Downloader { id: 1, name: "sonarr".into(), kind: "SONARR".into(), enabled: true, host, ..Downloader::default() };
        service(d, Arc::new(FakeDownloads::default()))
    }

    #[tokio::test]
    async fn sonarr_indexer_not_found_and_found() {
        let (host, pushed) = sonarr_server("IPTorrents (Prowlarr)").await;
        let a = Action { client_id: 1, kind: "SONARR".into(), ..Action::default() };
        let e = sonarr_service(host).run_action(&a, &mut sonarr_release()).await.unwrap_err();
        assert_eq!(
            e,
            "sonarr: failed to push release: WeCrashed.S01E01.DV.2160p.ATVP.WEB-DL.DDPA5.1.x265-NOSiViD: Indexer with name 'IPTorrents' could not be found"
        );
        assert_eq!(pushed.lock().unwrap()[0]["indexer"], "IPTorrents");

        let (host, _) = sonarr_server("IPTorrents").await;
        assert_eq!(sonarr_service(host).run_action(&a, &mut sonarr_release()).await.unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn sonarr_pushes_feed_metadata_and_omits_missing_tvdb() {
        let a = Action { client_id: 1, kind: "SONARR".into(), ..Action::default() };
        let (host, pushed) = sonarr_server("IPTorrents").await;
        let mut r = sonarr_release();
        r.meta_tvdb = 12345;
        r.publish_date = Some(chrono::DateTime::parse_from_rfc3339("2026-09-24T05:58:24Z").unwrap().to_utc());
        sonarr_service(host.clone()).run_action(&a, &mut r).await.unwrap();
        sonarr_service(host).run_action(&a, &mut sonarr_release()).await.unwrap();
        let got = pushed.lock().unwrap();
        assert_eq!(got[0]["tvdbId"], 12345);
        assert_eq!(got[0]["publishDate"], "2026-09-24T05:58:24Z");
        assert!(got[1].get("tvdbId").is_none());
    }

    #[test]
    fn option_builders() {
        let a = Action {
            save_path: "/data".into(),
            paused: true,
            limit_download_speed: 100,
            limit_upload_speed: 50,
            limit_ratio: 1.5,
            limit_seed_time: 60,
            skip_hash_check: true,
            ..Action::default()
        };
        let o = aria2_options(&a);
        assert_eq!(
            Value::Object(o),
            json!({"dir":"/data","pause":"true","max-download-limit":"100K","max-upload-limit":"50K","seed-ratio":"1.5","seed-time":"60"})
        );
        let d = deluge_options(&a);
        assert_eq!(
            d.to_dictionary(true),
            rencode_dict(&[
                ("max_upload_speed", 50.into()),
                ("max_download_speed", 100.into()),
                ("download_location", "/data".into()),
                ("add_paused", true.into()),
                ("seed_mode", true.into())
            ])
        );
        assert_eq!(qbit_check_ignore_slow(100, 0, 200 * 1024, 0), vec!["total download speed (200) above threshold: (100), skipping"]);
        assert!(qbit_check_ignore_slow(100, 100, 50 * 1024, 50 * 1024).is_empty());
        let args: Vec<String> =
            rtorrent_args(&Action { save_path: "/x".into(), content_layout: "SUBFOLDER_NONE".into(), ..Action::default() })
                .iter()
                .map(|f| f.as_arg())
                .collect();
        assert_eq!(args, vec![r#"d.directory_base.set="/x""#]);
    }

    /// A rencode dictionary from pairs.
    fn rencode_dict(pairs: &[(&str, RV)]) -> clients::rencode::Value {
        clients::rencode::Value::Dict(pairs.iter().map(|(k, v)| ((*k).into(), v.0.clone())).collect())
    }

    /// A rencode value made from a number, bool or text.
    struct RV(clients::rencode::Value);
    impl From<i64> for RV {
        fn from(i: i64) -> RV {
            RV(clients::rencode::Value::Int(i))
        }
    }
    impl From<bool> for RV {
        fn from(b: bool) -> RV {
            RV(clients::rencode::Value::Bool(b))
        }
    }
    impl From<&str> for RV {
        fn from(s: &str) -> RV {
            RV(s.into())
        }
    }

    #[tokio::test]
    async fn test_webhook_unsupported_and_usenet_protocol() {
        let s = service(Downloader::default(), Arc::new(FakeDownloads::default()));
        assert_eq!(s.run_action(&action("TEST"), &mut torrent_release()).await, Ok(Vec::new()));
        assert_eq!(s.run_action(&action("NOPE"), &mut torrent_release()).await, Err("unsupported action type: NOPE".into()));
        for k in ["SABNZBD", "NZBGET"] {
            assert_eq!(s.run_action(&action(k), &mut torrent_release()).await, Err(format!("action type: {k} invalid protocol: torrent")));
        }

        type Seen = Arc<Mutex<Vec<(String, String, String)>>>;
        let seen: Seen = Arc::default();
        let g = seen.clone();
        let app = axum::Router::new().route(
            "/hook",
            post(move |h: axum::http::HeaderMap, body: String| {
                let g = g.clone();
                async move {
                    let hv = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    g.lock().unwrap().push((hv("content-type"), hv("user-agent"), body));
                    "ok"
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let a = Action {
            webhook_host: format!("http://{addr}/hook"),
            webhook_data: r#"{"name":"{{ .TorrentName }}"}"#.into(),
            ..action("WEBHOOK")
        };
        assert_eq!(s.run_action(&a, &mut torrent_release()).await, Ok(Vec::new()));
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "application/json");
        assert_eq!(got[0].1, clients::user_agent());
        assert_eq!(got[0].2, r#"{"name":"Test.Release-GRP"}"#);
    }

    #[test]
    fn hub_routing_matches_kind_and_address() {
        let q = Downloader { kind: "QBITTORRENT".into(), host: "http://qbit:8080".into(), ..Downloader::default() };
        assert!(same_client(&q, "http://QBIT:8080/"));
        assert!(!same_client(&q, "http://qbit:9090"));
        let d = Downloader { kind: "DELUGE_V2".into(), host: "deluge".into(), port: 58846, ..Downloader::default() };
        assert!(same_client(&d, "http://deluge:8112"), "Deluge's daemon and web ports differ");
        assert_eq!(hub_kind("PORLA"), None);
        assert_eq!(hub_kind("DELUGE_V1"), Some("deluge"));
    }
}
