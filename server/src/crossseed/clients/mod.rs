// Ported from cross-seed v6.13.9 (src/clients/TorrentClient.ts, and the client searchee cache of src/searchee.ts),
// Apache-2.0.
//! cross-seed's torrent clients: one interface ([`Client`]) over qBittorrent, rTorrent, Transmission and Deluge,
//! talking to each as cross-seed does, several at once (cross-seed's `torrentClients`, in priority order). Each
//! client lists its torrents as searchees (kept in `cross_seed_client_searchees`, cross-seed's `client_searchee`
//! table, so unchanged torrents aren't asked for their files again), says where a torrent's files are, adds a match
//! (`inject`), and rechecks and resumes it by cross-seed's rules ([`should_recheck`], [`get_max_remaining_bytes`],
//! [`should_resume_from_non_relevant_files`]).
//!
//! | Client | Module | Entry |
//! | --- | --- | --- |
//! | qBittorrent (Web API v2) | [`qbittorrent`] | `qbittorrent:http://user:pass@host:8080` |
//! | rTorrent (XML-RPC) | [`rtorrent`] | `rtorrent:http://user:pass@host/RPC2` |
//! | Transmission (RPC) | [`transmission`] | `transmission:http://user:pass@host:9091/transmission/rpc` |
//! | Deluge (Web UI JSON-RPC) | [`deluge`] | `deluge:http://:pass@host:8112/json` |
//!
//! `readonly:` after the type (`rtorrent:readonly:http://…`) makes a client searched but never added to.

pub mod deluge;
pub mod qbittorrent;
pub mod rtorrent;
pub mod transmission;

use super::config::RuntimeConfig;
use super::constants::{Decision, InjectionResult, MatchMode, RESUME_EXCLUDED_EXTS, RESUME_EXCLUDED_KEYWORDS, VIDEO_DISC_EXTENSIONS};
use super::decide::get_partial_size_ratio;
use super::searchee::{File, Searchee};
use super::torrent::{sanitize_tracker_url, Metafile};
use super::utils::{has_ext, human_readable_size};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// The client types cross-seed knows, in its order.
pub const CLIENT_TYPES: [&str; 4] = ["qbittorrent", "rtorrent", "transmission", "deluge"];

/// One `torrentClients` entry, read by [`parse_client_entry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEntry {
    /// The type before the first colon (`qbittorrent`).
    pub client_type: String,
    /// Whether `readonly:` follows the type.
    pub readonly: bool,
    /// The client's address, with its user name and password.
    pub url: String,
}

/// `parseClientEntry`: `type:[readonly:]url`; `None` without a colon.
pub fn parse_client_entry(entry: &str) -> Option<ClientEntry> {
    // ^(?<clientType>.+?):(?:(?<readonly>readonly):)?(?<url>.*)$
    let colon = entry.char_indices().skip(1).find(|(_, c)| *c == ':').map(|(i, _)| i)?;
    let client_type = entry[..colon].to_string();
    let rest = &entry[colon + 1..];
    let (readonly, url) = match rest.strip_prefix("readonly:") {
        Some(url) => (true, url),
        None => (false, rest),
    };
    Some(ClientEntry { client_type, readonly, url: url.to_string() })
}

/// `clientsAreUnique`: (every host differs, every host and path differs).
pub fn clients_are_unique(entries: &[String]) -> (bool, bool) {
    let urls: Vec<url::Url> = entries.iter().filter_map(|e| parse_client_entry(e)).filter_map(|e| url::Url::parse(&e.url).ok()).collect();
    let hosts: HashSet<String> = urls.iter().map(url_host).collect();
    if hosts.len() == entries.len() {
        return (true, true);
    }
    let with_path: HashSet<String> = urls
        .iter()
        .map(|u| {
            let mut path = u.path().to_string();
            while path.contains("//") {
                path = path.replace("//", "/");
            }
            format!("{}{}", url_host(u), path.strip_suffix('/').unwrap_or(&path))
        })
        .collect();
    (false, with_path.len() == entries.len())
}

/// `URL.host`: the host, with the port when it isn't the scheme's default.
pub fn url_host(u: &url::Url) -> String {
    let host = u.host_str().unwrap_or("");
    match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }
}

/// A client address with its user name and password taken out (`extractCredentialsFromUrl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The user name, percent-decoded.
    pub username: String,
    /// The password, percent-decoded.
    pub password: String,
    /// The address without them: the origin and path (with `base_path` joined on when given).
    pub href: String,
}

/// `extractCredentialsFromUrl`; `None` for an address that can't be read.
pub fn extract_credentials_from_url(url: &str, base_path: Option<&str>) -> Option<Credentials> {
    let u = url::Url::parse(url).ok()?;
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    let origin = u.origin().ascii_serialization();
    let path = u.path();
    let href = match base_path {
        Some(base) => format!("{origin}{}", posix_join(path, base)),
        None if path == "/" => origin,
        None => format!("{origin}{path}"),
    };
    Some(Credentials { username: decode(u.username()), password: decode(u.password().unwrap_or("")), href })
}

/// `path.posix.join(a, b)` for two absolute-ish URL paths.
fn posix_join(a: &str, b: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let both = format!("{a}/{b}");
    for seg in both.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let trailing = b.ends_with('/');
    format!("/{}{}", parts.join("/"), if trailing && !parts.is_empty() { "/" } else { "" })
}

/// Why a client can't say where a torrent's files are (`getDownloadDir`'s errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirError {
    /// `NOT_FOUND`: the client doesn't have it.
    NotFound,
    /// `TORRENT_NOT_COMPLETE`: it isn't finished.
    TorrentNotComplete,
    /// `INVALID_DATA`: its layout can't be used.
    InvalidData,
    /// `UNKNOWN_ERROR`: the client didn't answer properly.
    UnknownError,
}

/// `TorrentMetadataInClient`: a torrent's hash, category, tags and trackers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TorrentInClient {
    /// Its info hash.
    pub info_hash: String,
    /// Its category or label, when the client has them.
    pub category: Option<String>,
    /// Its tags, when the client has them.
    pub tags: Option<Vec<String>>,
    /// Its trackers' hosts, when asked.
    pub trackers: Option<Vec<String>>,
}

/// `ClientSearcheeResult`: every searchee, and the new or changed ones.
#[derive(Debug, Clone, Default)]
pub struct ClientSearchees {
    /// Every torrent of the client as a searchee.
    pub searchees: Vec<Searchee>,
    /// Those new, changed or refreshed since the last listing.
    pub new_searchees: Vec<Searchee>,
}

/// `getClientSearchees`'s options.
#[derive(Debug, Clone, Default)]
pub struct SearcheeOptions {
    /// `newSearcheesOnly`: only the new, changed or refreshed searchees.
    pub new_searchees_only: bool,
    /// `refresh`: `None` uses the cache, an empty list refreshes all, else the info hashes to refresh.
    pub refresh: Option<Vec<String>>,
    /// `includeFiles` (qBittorrent 5.1 lists files with the torrents).
    pub include_files: bool,
    /// `includeTrackers` (qBittorrent 5.1 lists trackers with the torrents).
    pub include_trackers: bool,
}

impl SearcheeOptions {
    /// Whether `info_hash` must be read again.
    fn refreshes(&self, info_hash: &str) -> bool {
        match &self.refresh {
            None => false,
            Some(list) if list.is_empty() => true,
            Some(list) => list.iter().any(|h| h == info_hash),
        }
    }
}

/// A torrent or searchee as `getDownloadDir` needs it: its info hash and files.
#[derive(Debug, Clone, Copy)]
pub struct MetaRef<'a> {
    /// Its info hash.
    pub info_hash: &'a str,
    /// Its files.
    pub files: &'a [File],
}

impl<'a> MetaRef<'a> {
    /// The torrent's.
    pub fn of_meta(m: &'a Metafile) -> MetaRef<'a> {
        MetaRef { info_hash: &m.info_hash, files: &m.files }
    }

    /// The searchee's (an empty hash when it has none).
    pub fn of_searchee(s: &'a Searchee) -> MetaRef<'a> {
        MetaRef { info_hash: s.info_hash.as_deref().unwrap_or(""), files: &s.files }
    }
}

/// How long [`Client::resume_injection`] waits: cross-seed's 15 seconds between looks, 5 minutes after the client
/// didn't answer, for up to an hour.
#[derive(Debug, Clone, Copy)]
pub struct ResumeTiming {
    /// `resumeSleepTime`.
    pub sleep: Duration,
    /// `resumeErrSleepTime`.
    pub err_sleep: Duration,
    /// How long it keeps looking (`getResumeStopTime`).
    pub stop_after: Duration,
}

impl Default for ResumeTiming {
    /// cross-seed's times.
    fn default() -> Self {
        ResumeTiming { sleep: Duration::from_secs(15), err_sleep: Duration::from_secs(300), stop_after: Duration::from_secs(3600) }
    }
}

/// What every client keeps: its host, priority, type, whether it is read-only and its log label.
#[derive(Debug, Clone)]
pub struct Info {
    /// `clientHost`: the address's host (and path, when two clients share a host).
    pub client_host: String,
    /// `clientPriority`: its place in the list (0 first).
    pub client_priority: usize,
    /// `clientType`.
    pub client_type: &'static str,
    /// `readonly`: searched, never added to.
    pub readonly: bool,
    /// `label`: `type@host`, as in cross-seed's logs.
    pub label: String,
}

/// The program behind a [`Client`].
#[derive(Clone)]
pub enum Kind {
    /// qBittorrent.
    Qbittorrent(qbittorrent::Qbittorrent),
    /// rTorrent.
    Rtorrent(rtorrent::Rtorrent),
    /// Transmission.
    Transmission(transmission::Transmission),
    /// Deluge.
    Deluge(deluge::Deluge),
}

/// `TorrentClient`: one of cross-seed's torrent clients.
#[derive(Clone)]
pub struct Client {
    /// Its host, priority, type and label.
    pub info: Info,
    /// The program it talks to.
    pub kind: Kind,
}

/// `instantiateDownloadClients`: the clients of `entries`, in priority order. An entry that can't be read is left
/// out (its problem is in the settings check).
pub fn instantiate(entries: &[String]) -> Vec<Arc<Client>> {
    let (unique_hosts, _) = clients_are_unique(entries);
    let mut out = Vec::new();
    for (priority, raw) in entries.iter().enumerate() {
        let Some(e) = parse_client_entry(raw) else { continue };
        let Ok(u) = url::Url::parse(&e.url) else {
            tracing::error!(
                "cross-seed: the torrent client address {} can't be read",
                super::utils::sanitize_url(&e.url).unwrap_or_default()
            );
            continue;
        };
        let client_host = if unique_hosts { url_host(&u) } else { format!("{}{}", url_host(&u), u.path()) };
        let Some(client_type) = CLIENT_TYPES.iter().find(|t| **t == e.client_type).copied() else { continue };
        let info = Info {
            label: format!("{client_type}@{client_host}"),
            client_host,
            client_priority: priority,
            client_type,
            readonly: e.readonly,
        };
        let kind = match client_type {
            "qbittorrent" => Kind::Qbittorrent(qbittorrent::Qbittorrent::new(&e.url, &info)),
            "rtorrent" => Kind::Rtorrent(rtorrent::Rtorrent::new(&e.url, &info)),
            "transmission" => Kind::Transmission(transmission::Transmission::new(&e.url, &info)),
            _ => Kind::Deluge(deluge::Deluge::new(&e.url, &info)),
        };
        out.push(Arc::new(Client { info, kind }));
    }
    out
}

/// `byClientHostPriority`: the priority of the client at `client_host`, or the number of clients.
pub fn by_client_host_priority(clients: &[Arc<Client>], client_host: Option<&str>) -> usize {
    clients.iter().find(|c| Some(c.info.client_host.as_str()) == client_host).map(|c| c.info.client_priority).unwrap_or(clients.len())
}

impl Client {
    /// `validateConfig`: signs in and checks the client, or says why it can't be used.
    pub async fn validate_config(&self) -> Result<(), String> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.validate_config().await,
            Kind::Rtorrent(c) => c.validate_config().await,
            Kind::Transmission(c) => c.validate_config().await,
            Kind::Deluge(c) => c.validate_config().await,
        }
    }

    /// `isTorrentInClient`.
    pub async fn is_torrent_in_client(&self, info_hash: &str) -> Result<bool, String> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.is_torrent_in_client(info_hash).await,
            Kind::Rtorrent(c) => c.is_torrent_in_client(info_hash).await,
            Kind::Transmission(c) => c.is_torrent_in_client(info_hash).await,
            Kind::Deluge(c) => c.is_torrent_in_client(info_hash).await,
        }
    }

    /// `isTorrentComplete`: `None` for `NOT_FOUND`.
    pub async fn is_torrent_complete(&self, info_hash: &str) -> Option<bool> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.is_torrent_complete(info_hash).await,
            Kind::Rtorrent(c) => c.is_torrent_complete(info_hash).await,
            Kind::Transmission(c) => c.is_torrent_complete(info_hash).await,
            Kind::Deluge(c) => c.is_torrent_complete(info_hash).await,
        }
    }

    /// `isTorrentChecking`: `None` for `NOT_FOUND`.
    pub async fn is_torrent_checking(&self, info_hash: &str) -> Option<bool> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.is_torrent_checking(info_hash).await,
            Kind::Rtorrent(c) => c.is_torrent_checking(info_hash).await,
            Kind::Transmission(c) => c.is_torrent_checking(info_hash).await,
            Kind::Deluge(c) => c.is_torrent_checking(info_hash).await,
        }
    }

    /// `getAllTorrents`.
    pub async fn get_all_torrents(&self) -> Vec<TorrentInClient> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.get_all_torrents().await,
            Kind::Rtorrent(c) => c.get_all_torrents().await,
            Kind::Transmission(c) => c.get_all_torrents().await,
            Kind::Deluge(c) => c.get_all_torrents().await,
        }
    }

    /// `getClientSearchees`: the client's torrents as searchees, updating the cache in `db`.
    pub async fn get_client_searchees(&self, db: &SqliteDb, cfg: &RuntimeConfig, o: &SearcheeOptions) -> ClientSearchees {
        let listed = match &self.kind {
            Kind::Qbittorrent(c) => c.list_torrents(db, cfg, o).await,
            Kind::Rtorrent(c) => c.list_torrents(db, o).await,
            Kind::Transmission(c) => c.list_torrents(db, o).await,
            Kind::Deluge(c) => c.list_torrents(db, o).await,
        };
        let Some((searchees, new_searchees, info_hashes)) = listed else { return ClientSearchees::default() };
        if let Err(e) = update_searchee_client_db(db, &self.info.client_host, &new_searchees, &info_hashes) {
            tracing::error!("cross-seed [{}]: the torrent list can't be kept: {}", self.info.label, e.detail);
        }
        ClientSearchees { searchees, new_searchees }
    }

    /// `getDownloadDir`: the folder a torrent's files are in, as the client sees it.
    pub async fn get_download_dir(&self, meta: MetaRef<'_>, only_completed: bool, cfg: &RuntimeConfig) -> Result<String, DirError> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.get_download_dir(meta, only_completed, cfg).await,
            Kind::Rtorrent(c) => c.get_download_dir(meta.info_hash, only_completed).await,
            Kind::Transmission(c) => c.get_download_dir(meta.info_hash, only_completed).await,
            Kind::Deluge(c) => c.get_download_dir(meta.info_hash, only_completed).await,
        }
    }

    /// `getAllDownloadDirs`: every torrent's folder, by info hash.
    pub async fn get_all_download_dirs(&self, metas: &[MetaRef<'_>], only_completed: bool, cfg: &RuntimeConfig) -> HashMap<String, String> {
        match &self.kind {
            Kind::Qbittorrent(c) => c.get_all_download_dirs(metas, only_completed, false, cfg).await.unwrap_or_default(),
            Kind::Rtorrent(c) => c.get_all_download_dirs(only_completed).await,
            Kind::Transmission(c) => c.get_all_download_dirs(only_completed).await,
            Kind::Deluge(c) => c.get_all_download_dirs(only_completed).await,
        }
    }

    /// `recheckTorrent`: pauses the torrent, then has the client check its files.
    pub async fn recheck_torrent(&self, info_hash: &str) {
        match &self.kind {
            Kind::Qbittorrent(c) => c.recheck_torrent(info_hash).await,
            Kind::Rtorrent(c) => c.recheck_torrent(info_hash).await,
            Kind::Transmission(c) => c.recheck_torrent(info_hash).await,
            Kind::Deluge(c) => c.recheck_torrent(info_hash).await,
        }
    }

    /// `resumeInjection`: once the check is done, starts the torrent when little enough is missing.
    pub async fn resume_injection(&self, meta: &Metafile, decision: Decision, check_once: bool, cfg: &RuntimeConfig, t: ResumeTiming) {
        match &self.kind {
            Kind::Qbittorrent(c) => c.resume_injection(meta, decision, check_once, cfg, t).await,
            Kind::Rtorrent(c) => c.resume_injection(meta, decision, check_once, cfg, t).await,
            Kind::Transmission(c) => c.resume_injection(meta, decision, check_once, cfg, t).await,
            Kind::Deluge(c) => c.resume_injection(meta, decision, check_once, cfg, t).await,
        }
    }

    /// `void client.resumeInjection(...)`: [`resume_injection`](Self::resume_injection) in the background.
    pub fn spawn_resume(self: &Arc<Self>, meta: &Metafile, decision: Decision, cfg: &RuntimeConfig) {
        let (me, meta, cfg) = (self.clone(), meta.clone(), cfg.clone());
        tokio::spawn(async move { me.resume_injection(&meta, decision, false, &cfg, ResumeTiming::default()).await });
    }

    /// `inject`: adds `meta` to the client, from the files of `searchee` or those linked into `destination_dir`.
    pub async fn inject(
        self: &Arc<Self>,
        meta: &Metafile,
        searchee: &Searchee,
        decision: Decision,
        only_completed: bool,
        destination_dir: Option<&str>,
        cfg: &RuntimeConfig,
    ) -> InjectionResult {
        let (result, resume) = match &self.kind {
            Kind::Qbittorrent(c) => c.inject(meta, searchee, decision, only_completed, destination_dir, cfg).await,
            Kind::Rtorrent(c) => c.inject(meta, searchee, decision, only_completed, destination_dir, cfg).await,
            Kind::Transmission(c) => c.inject(meta, searchee, decision, only_completed, destination_dir, cfg).await,
            Kind::Deluge(c) => c.inject(meta, searchee, decision, only_completed, destination_dir, cfg).await,
        };
        if resume {
            self.spawn_resume(meta, decision, cfg);
        }
        result
    }
}

/// What a client's `inject` did: the result, and whether `resumeInjection` follows in the background.
pub type Injected = (InjectionResult, bool);

/// `clientSearcheeModified`: whether a torrent changed since it was kept (or wasn't kept).
pub fn client_searchee_modified(
    kept: Option<&Searchee>,
    name: &str,
    save_path: &str,
    category: Option<&str>,
    tags: Option<&[String]>,
) -> bool {
    let Some(k) = kept else { return true };
    if k.name != name || k.save_path.as_deref() != Some(save_path) {
        return true;
    }
    if k.category.as_deref() != category {
        return true;
    }
    k.tags.as_deref().unwrap_or(&[]) != tags.unwrap_or(&[])
}

/// `organizeTrackers`: the trackers' hosts.
pub fn organize_trackers<'a>(urls: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    urls.into_iter().filter_map(sanitize_tracker_url).filter(|t| !t.is_empty()).collect()
}

/// `shouldRecheck`: whether a match must be checked by the client before it seeds.
pub fn should_recheck(meta: &Metafile, decision: Decision, cfg: &RuntimeConfig) -> bool {
    if !cfg.skip_recheck {
        return true;
    }
    if decision == Decision::MatchPartial {
        return true;
    }
    has_ext(&meta.files, VIDEO_DISC_EXTENSIONS)
}

/// `getMaxRemainingBytes`: how much a match may still miss and be resumed after its check.
pub fn get_max_remaining_bytes(meta: &Metafile, decision: Decision, cfg: &RuntimeConfig, log: Option<(&str, &str)>) -> i64 {
    if decision != Decision::MatchPartial || cfg.match_mode != MatchMode::Partial {
        return 0;
    }
    if has_ext(&meta.files, VIDEO_DISC_EXTENSIONS) {
        if let Some((label, torrent)) = log {
            tracing::warn!("cross-seed [{label}]: autoResumeMaxDownload will not resume {torrent}: VIDEO_DISC_EXTENSIONS");
        }
        return 0;
    }
    cfg.auto_resume_max_download
}

/// `shouldResumeFromNonRelevantFiles`: whether the missing bytes are only extras (samples, `.nfo`, subtitles) and
/// so the match may be resumed anyway.
pub fn should_resume_from_non_relevant_files(
    meta: &Metafile,
    remaining: i64,
    decision: Decision,
    cfg: &RuntimeConfig,
    log: Option<(&str, &str)>,
) -> bool {
    if !cfg.ignore_non_relevant_files_to_resume || decision != Decision::MatchPartial || cfg.match_mode != MatchMode::Partial {
        return false;
    }
    let warn = |why: String| {
        if let Some((label, torrent)) = log {
            tracing::warn!("cross-seed [{label}]: ignoreNonRelevantFilesToResume will not resume {torrent}: {why}");
        }
    };
    if has_ext(&meta.files, VIDEO_DISC_EXTENSIONS) {
        warn("VIDEO_DISC_EXTENSIONS".into());
        return false;
    }
    if remaining > 209_715_200 {
        warn("200 MiB limit".into());
        return false;
    }
    let irrelevant: i64 = meta
        .files
        .iter()
        .filter(|f| {
            let p = f.path.to_lowercase();
            RESUME_EXCLUDED_KEYWORDS.iter().any(|k| p.contains(k)) || RESUME_EXCLUDED_EXTS.iter().any(|e| p.ends_with(e))
        })
        .map(|f| f.length)
        .sum();
    if irrelevant == 0 {
        warn("all files are relevant".into());
        return false;
    }
    if remaining <= irrelevant + meta.piece_length * 2 {
        return true;
    }
    warn(format!(
        "remainingSize {} > {} irrelevantSize",
        human_readable_size(remaining as f64, true),
        human_readable_size(irrelevant as f64, true)
    ));
    false
}

/// `estimatePausedStatus`: whether a match will stay paused after its check.
pub fn estimate_paused_status(meta: &Metafile, searchee: &Searchee, decision: Decision, cfg: &RuntimeConfig) -> bool {
    let remaining = ((1.0 - get_partial_size_ratio(meta, searchee)) * meta.length as f64) as i64;
    if remaining <= get_max_remaining_bytes(meta, decision, cfg, None) {
        return false;
    }
    !should_resume_from_non_relevant_files(meta, remaining, decision, cfg, None)
}

/// The resume rule all four clients share once the check is done: whether to resume (when not, cross-seed's
/// warning is logged).
pub fn may_resume(label: &str, torrent: &str, meta: &Metafile, decision: Decision, remaining: i64, cfg: &RuntimeConfig) -> bool {
    let max = get_max_remaining_bytes(meta, decision, cfg, Some((label, torrent)));
    if remaining > max && !should_resume_from_non_relevant_files(meta, remaining, decision, cfg, Some((label, torrent))) {
        tracing::warn!(
            "cross-seed [{label}]: autoResumeMaxDownload will not resume {torrent}: remainingSize {} > {} limit",
            human_readable_size(remaining as f64, true),
            human_readable_size(max as f64, true)
        );
        return false;
    }
    true
}

/// `waitForTorrentToComplete`: asks up to `retries + 1` times, 1, 2, 4… seconds apart, whether it is complete.
pub async fn wait_for_torrent_to_complete(client: &Client, info_hash: &str, retries: u32, unit: Duration) -> bool {
    for i in 0..=retries {
        if client.is_torrent_complete(info_hash).await.unwrap_or(false) {
            return true;
        }
        if i < retries {
            tokio::time::sleep(unit * 2u32.pow(i)).await;
        }
    }
    false
}

/// `wait(Math.min(1 second * 2 ** i, 10 seconds))`, the clients' back-off between tries.
pub(crate) async fn backoff(i: u32) {
    tokio::time::sleep(Duration::from_millis((1000u64 << i.min(4)).min(10_000))).await;
}

/// The HTTP client the torrent clients send with: cookies are handled by hand, as cross-seed does.
pub(crate) fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("Lantern/{}", crate::VERSION))
        .build()
        .unwrap_or_default()
}

/// `createSearcheeFromDB`: a kept client torrent.
fn searchee_from_row(r: &kit_server_core::Row) -> Searchee {
    let category = r.s("category");
    let tags = r.s("tags");
    Searchee {
        info_hash: Some(r.s("info_hash")),
        name: r.s("name"),
        title: r.s("title"),
        files: serde_json::from_str(&r.s("files")).unwrap_or_default(),
        length: r.i("length"),
        client_host: Some(r.s("client_host")),
        save_path: Some(r.s("save_path")),
        category: if r.get("category").is_some_and(|v| !v.is_null()) { Some(category) } else { None },
        tags: if tags.is_empty() { None } else { serde_json::from_str(&tags).ok() },
        trackers: Some(serde_json::from_str(&r.s("trackers")).unwrap_or_default()),
        ..Searchee::default()
    }
}

/// The kept torrent `info_hash` of the client at `client_host` (`db("client_searchee")…first()`).
pub fn kept_searchee(db: &SqliteDb, client_host: &str, info_hash: &str) -> Option<Searchee> {
    db.one("SELECT * FROM cross_seed_client_searchees WHERE info_hash=? AND client_host=?", p![info_hash, client_host])
        .ok()
        .flatten()
        .map(|r| searchee_from_row(&r))
}

/// Every kept torrent of every client.
pub fn kept_searchees(db: &SqliteDb) -> Vec<Searchee> {
    db.query("SELECT * FROM cross_seed_client_searchees", p![]).map(|rows| rows.iter().map(searchee_from_row).collect()).unwrap_or_default()
}

/// `updateSearcheeClientDB`: forgets the torrents gone from the client and keeps the new or changed ones.
pub fn update_searchee_client_db(
    db: &SqliteDb,
    client_host: &str,
    new_searchees: &[Searchee],
    info_hashes: &HashSet<String>,
) -> kit_server_core::Result<()> {
    let kept: Vec<String> = db
        .query("SELECT info_hash FROM cross_seed_client_searchees WHERE client_host=?", p![client_host])?
        .iter()
        .map(|r| r.s("info_hash"))
        .collect();
    for h in kept.iter().filter(|h| !info_hashes.contains(*h)) {
        db.execute("DELETE FROM cross_seed_client_searchees WHERE info_hash=? AND client_host=?", p![h.clone(), client_host])?;
    }
    for s in new_searchees {
        let json = |v: &dyn erased_json::Json| v.to_json();
        db.execute(
            "INSERT INTO cross_seed_client_searchees (info_hash, name, title, files, length, client_host, save_path, category, tags, trackers)
             VALUES (?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(client_host, info_hash) DO UPDATE SET name=excluded.name, title=excluded.title, files=excluded.files,
               length=excluded.length, save_path=excluded.save_path, category=excluded.category, tags=excluded.tags,
               trackers=excluded.trackers",
            p![
                s.info_hash.clone().unwrap_or_default(),
                s.name.clone(),
                s.title.clone(),
                json(&s.files),
                s.length,
                s.client_host.clone().unwrap_or_default(),
                s.save_path.clone().unwrap_or_default(),
                s.category.clone(),
                s.tags.as_ref().map(|t| serde_json::to_string(t).unwrap_or_default()),
                json(&s.trackers.clone().unwrap_or_default())
            ],
        )?;
    }
    Ok(())
}

/// JSON text of a value, for the cache's columns.
mod erased_json {
    /// A value kept as JSON text.
    pub trait Json {
        /// Its JSON text.
        fn to_json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn to_json(&self) -> String {
            serde_json::to_string(self).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crossseed::torrent::build;

    fn meta(files: &[(&str, i64)]) -> Metafile {
        Metafile::decode(&build("Show.S01.1080p-GRP", files, "x")).unwrap()
    }

    #[test]
    fn client_entries_as_cross_seed_reads_them() {
        let e = parse_client_entry("rtorrent:readonly:http://u:p@localhost:1234/RPC2").unwrap();
        assert_eq!((e.client_type.as_str(), e.readonly, e.url.as_str()), ("rtorrent", true, "http://u:p@localhost:1234/RPC2"));
        let e = parse_client_entry("qbittorrent:http://localhost:8080").unwrap();
        assert_eq!((e.client_type.as_str(), e.readonly, e.url.as_str()), ("qbittorrent", false, "http://localhost:8080"));
        assert_eq!(parse_client_entry("nocolon"), None);
        assert_eq!(parse_client_entry(":x"), None, "the type has at least one character");
        let two = ["qbittorrent:http://a:8080".to_string(), "deluge:http://b:8112/json".to_string()];
        assert_eq!(clients_are_unique(&two), (true, true));
        let same_host = ["rtorrent:http://h/one/RPC2".to_string(), "rtorrent:http://h/two/RPC2/".to_string()];
        assert_eq!(clients_are_unique(&same_host), (false, true));
        let dupes = ["rtorrent:http://h/RPC2".to_string(), "rtorrent:http://h//RPC2/".to_string()];
        assert_eq!(clients_are_unique(&dupes), (false, false));
        let made = instantiate(&same_host);
        assert_eq!(made[1].info.client_host, "h/two/RPC2/");
        assert_eq!(made[1].info.label, "rtorrent@h/two/RPC2/");
        assert_eq!(made[1].info.client_priority, 1);
        assert_eq!(by_client_host_priority(&made, Some("h/two/RPC2/")), 1);
        assert_eq!(by_client_host_priority(&made, None), 2);
    }

    #[test]
    fn credentials_out_of_addresses() {
        let c = extract_credentials_from_url("http://us%40er:p%3Ass@host:8080/qbit", Some("/api/v2")).unwrap();
        assert_eq!((c.username.as_str(), c.password.as_str(), c.href.as_str()), ("us@er", "p:ss", "http://host:8080/qbit/api/v2"));
        let c = extract_credentials_from_url("http://:pw@host:8112/", None).unwrap();
        assert_eq!((c.username.as_str(), c.password.as_str(), c.href.as_str()), ("", "pw", "http://host:8112"));
        let c = extract_credentials_from_url("http://host:8080", Some("/api/v2")).unwrap();
        assert_eq!(c.href, "http://host:8080/api/v2");
        assert!(extract_credentials_from_url("not a url", None).is_none());
    }

    #[test]
    fn recheck_and_resume_rules() {
        let cfg = RuntimeConfig::default();
        let m = meta(&[("a.mkv", 1000), ("a.nfo", 10)]);
        assert!(!should_recheck(&m, Decision::Match, &cfg), "skipRecheck skips a match with every file");
        assert!(!should_recheck(&m, Decision::MatchSizeOnly, &cfg));
        assert!(should_recheck(&m, Decision::MatchPartial, &cfg), "a partial match is always checked");
        let disc = meta(&[("BDMV/STREAM/00001.m2ts", 1000)]);
        assert!(should_recheck(&disc, Decision::Match, &cfg), "disc images are always checked");
        let no_skip = RuntimeConfig { skip_recheck: false, ..RuntimeConfig::default() };
        assert!(should_recheck(&m, Decision::Match, &no_skip));

        // autoResumeMaxDownload only for partial matches in partial mode, never for discs
        assert_eq!(get_max_remaining_bytes(&m, Decision::MatchPartial, &cfg, None), 0, "flexible mode");
        let partial = RuntimeConfig { match_mode: MatchMode::Partial, ..RuntimeConfig::default() };
        assert_eq!(get_max_remaining_bytes(&m, Decision::MatchPartial, &partial, None), 52_428_800);
        assert_eq!(get_max_remaining_bytes(&m, Decision::Match, &partial, None), 0);
        assert_eq!(get_max_remaining_bytes(&disc, Decision::MatchPartial, &partial, None), 0);

        // ignoreNonRelevantFilesToResume: only extras missing, up to 200 MiB and two pieces more
        let extras = meta(&[("Show.mkv", 900_000_000), ("Sample/sample.mkv", 50_000_000), ("Show.nfo", 1000)]);
        assert!(!should_resume_from_non_relevant_files(&extras, 50_001_000, Decision::MatchPartial, &partial, None), "off by default");
        let on = RuntimeConfig { ignore_non_relevant_files_to_resume: true, ..partial.clone() };
        assert!(should_resume_from_non_relevant_files(&extras, 50_001_000, Decision::MatchPartial, &on, None));
        let piece = extras.piece_length;
        assert!(should_resume_from_non_relevant_files(&extras, 50_001_000 + 2 * piece, Decision::MatchPartial, &on, None));
        assert!(!should_resume_from_non_relevant_files(&extras, 50_001_001 + 2 * piece, Decision::MatchPartial, &on, None));
        assert!(!should_resume_from_non_relevant_files(&extras, 209_715_201, Decision::MatchPartial, &on, None), "200 MiB limit");
        assert!(!should_resume_from_non_relevant_files(&extras, 10, Decision::MatchSizeOnly, &on, None));
        let all_relevant = meta(&[("a.mkv", 1000), ("b.mkv", 1000)]);
        assert!(!should_resume_from_non_relevant_files(&all_relevant, 10, Decision::MatchPartial, &on, None));
        assert!(may_resume("l", "t", &all_relevant, Decision::MatchPartial, 52_428_800, &on));
        assert!(!may_resume("l", "t", &all_relevant, Decision::MatchPartial, 52_428_801, &on));
        assert!(may_resume("l", "t", &extras, Decision::MatchPartial, 50_001_000, &on));

        // estimatePausedStatus: what is missing against what may be missing
        let have = Searchee { files: vec![File { name: "a.mkv".into(), path: "a.mkv".into(), length: 1000 }], ..Searchee::default() };
        assert!(!estimate_paused_status(&m, &have, Decision::MatchPartial, &partial), "10 bytes missing are within 50 MiB");
        assert!(estimate_paused_status(&m, &have, Decision::MatchPartial, &cfg), "flexible mode never resumes");
    }

    #[test]
    fn a_kept_torrent_changes_when_its_name_folder_category_or_tags_do() {
        let kept = Searchee {
            name: "n".into(),
            save_path: Some("/d".into()),
            category: Some("tv".into()),
            tags: Some(vec!["a".into()]),
            ..Searchee::default()
        };
        let tags = vec!["a".to_string()];
        assert!(!client_searchee_modified(Some(&kept), "n", "/d", Some("tv"), Some(&tags)));
        assert!(client_searchee_modified(None, "n", "/d", Some("tv"), Some(&tags)));
        assert!(client_searchee_modified(Some(&kept), "m", "/d", Some("tv"), Some(&tags)));
        assert!(client_searchee_modified(Some(&kept), "n", "/e", Some("tv"), Some(&tags)));
        assert!(client_searchee_modified(Some(&kept), "n", "/d", None, Some(&tags)));
        assert!(client_searchee_modified(Some(&kept), "n", "/d", Some("tv"), None));
        let trackers = organize_trackers(["https://u:p@t.example:8080/a/announce", "udp://x.example:80", "not a url"]);
        assert_eq!(trackers, vec!["t.example:8080", "x.example:80"]);
    }
}
