// Ported from cross-seed v6.13.9 (src/clients/RTorrent.ts), Apache-2.0.
//! rTorrent through XML-RPC, as cross-seed talks to it: `download_list`, `system.multicall` batches of 500
//! torrents, and `load.raw_start` (or `load.raw` with a hash check for a recheck) of the `.torrent` with
//! `libtorrent_resume` data for the files already on disk, its folder and the label `cross-seed` in `d.custom1`.

use super::{
    client_searchee_modified, http, kept_searchee, may_resume, organize_trackers, should_recheck, DirError, Info, Injected, ResumeTiming,
    SearcheeOptions, TorrentInClient,
};
use crate::crossseed::config::RuntimeConfig;
use crate::crossseed::constants::{Decision, InjectionResult, TORRENT_TAG};
use crate::crossseed::searchee::{basename, dirname, parse_title, File, Searchee};
use crate::crossseed::torrent::{with_top_key, Metafile};
use crate::crossseed::utils::sanitize_info_hash;
use crate::hub::clients::xmlrpc;
use base64::Engine;
use kit_server_core::SqliteDb;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// rTorrent's fault for an info hash it doesn't have.
const COULD_NOT_FIND_INFO_HASH: &str = "Could not find info-hash.";

/// How many torrents one `system.multicall` asks about.
const BATCH_SIZE: usize = 500;

/// An XML-RPC value cross-seed sends.
#[derive(Debug, Clone)]
pub enum X {
    /// Text.
    Str(String),
    /// Bytes (base64).
    Bytes(Vec<u8>),
    /// A list.
    List(Vec<X>),
    /// A struct.
    Struct(Vec<(String, X)>),
}

/// The XML of one value.
fn xml_value(v: &X, out: &mut String) {
    out.push_str("<value>");
    match v {
        X::Str(s) => {
            out.push_str("<string>");
            out.push_str(&quick_xml::escape::escape(s.as_str()));
            out.push_str("</string>");
        }
        X::Bytes(b) => {
            out.push_str("<base64>");
            out.push_str(&base64::engine::general_purpose::STANDARD.encode(b));
            out.push_str("</base64>");
        }
        X::List(l) => {
            out.push_str("<array><data>");
            for x in l {
                xml_value(x, out);
            }
            out.push_str("</data></array>");
        }
        X::Struct(m) => {
            out.push_str("<struct>");
            for (k, x) in m {
                out.push_str("<member><name>");
                out.push_str(&quick_xml::escape::escape(k.as_str()));
                out.push_str("</name>");
                xml_value(x, out);
                out.push_str("</member>");
            }
            out.push_str("</struct>");
        }
    }
    out.push_str("</value>");
}

/// The body of a call.
pub fn xml_call(method: &str, params: &[X]) -> String {
    let mut out = format!("<?xml version=\"1.0\"?><methodCall><methodName>{}</methodName><params>", quick_xml::escape::escape(method));
    for p in params {
        out.push_str("<param>");
        xml_value(p, &mut out);
        out.push_str("</param>");
    }
    out.push_str("</params></methodCall>");
    out
}

/// One `{ methodName, params: [hash, …] }` of a multicall.
fn mc(method: &str, params: &[&str]) -> X {
    X::Struct(vec![
        ("methodName".into(), X::Str(method.into())),
        ("params".into(), X::List(params.iter().map(|p| X::Str(p.to_string())).collect())),
    ])
}

/// `Boolean(Number(x))` for a number or a numeric text.
fn truthy(v: &Value) -> bool {
    num(v) != 0.0
}

/// `Number(x)` for a number or a numeric text.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(f64::NAN),
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => f64::NAN,
    }
}

/// The text of `x[0]` in a multicall answer.
fn first_str(v: &Value) -> String {
    match v.get(0) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Whether a multicall answer is a list of faults.
fn is_fault(v: &Value) -> bool {
    v.get(0).is_some_and(|f| f.get("faultString").is_some())
}

/// What `checkOriginalTorrent` reads.
#[derive(Debug, Clone)]
struct Original {
    /// `d.name`.
    name: String,
    /// `d.directory`.
    directory_base: String,
    /// `d.left_bytes`.
    bytes_left: i64,
    /// `d.hashing`.
    hashing: bool,
    /// `d.is_multi_file`.
    is_multi_file: bool,
    /// `d.is_active`.
    is_active: bool,
}

/// One rTorrent.
#[derive(Clone)]
pub struct Rtorrent {
    /// The client's host, priority and label.
    info: Info,
    /// The XML-RPC address without credentials; `None` when it can't be read.
    href: Option<String>,
    /// The user name for basic sign-in.
    username: String,
    /// The password for basic sign-in.
    password: String,
    /// The HTTP client.
    http: reqwest::Client,
}

/// `createLibTorrentResumeTree`: the `libtorrent_resume` dictionary, bencoded, for the files already on disk under
/// `base_path` (files missing or of another size are left out, as cross-seed does).
pub fn create_libtorrent_resume_tree(meta: &Metafile, base_path: &Path) -> Vec<u8> {
    let mut files = String::new();
    for f in &meta.files {
        let rest: Vec<&str> = f.path.split('/').skip(1).collect();
        let p = if rest.is_empty() { base_path.to_path_buf() } else { base_path.join(rest.join("/")) };
        let Ok(st) = std::fs::metadata(&p) else { continue };
        if !st.is_file() || st.len() as i64 != f.length {
            continue;
        }
        let mtime = st.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let completed = (f.length as f64 / meta.piece_length as f64).ceil() as i64;
        files.push_str(&format!("d9:completedi{completed}e5:mtimei{mtime}e8:priorityi1ee"));
    }
    let bitfield = (meta.length as f64 / meta.piece_length as f64).ceil() as i64;
    format!("d8:bitfieldi{bitfield}e5:filesl{files}ee").into_bytes()
}

/// `path.resolve(a, b)` for the folders sent to rTorrent.
fn resolve(a: &str, b: &str) -> PathBuf {
    let joined = Path::new(a).join(b);
    let mut out = PathBuf::from("/");
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(s) => out.push(s),
            _ => {}
        }
    }
    out
}

impl Rtorrent {
    /// The client for `url`.
    pub fn new(url: &str, info: &Info) -> Rtorrent {
        let creds = super::extract_credentials_from_url(url, None);
        Rtorrent {
            info: info.clone(),
            href: creds.as_ref().map(|c| c.href.clone()),
            username: creds.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
            password: creds.map(|c| c.password).unwrap_or_default(),
            http: http(),
        }
    }

    /// `methodCallP`: one call; its answer, or the fault or failure.
    pub async fn method_call(&self, method: &str, params: &[X]) -> Result<Value, String> {
        let href = self.href.as_deref().ok_or_else(|| format!("[{}] rTorrent url must be percent-encoded", self.info.label))?;
        let body = xml_call(method, params);
        tracing::debug!("cross-seed [{}]: Calling method {method}", self.info.label);
        let mut req = self.http.post(href).header("Content-Type", "text/xml").timeout(Duration::from_secs(300)).body(body);
        if !self.username.is_empty() && !self.password.is_empty() {
            req = req.basic_auth(&self.username, Some(&self.password));
        }
        let r = req.send().await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("Unknown XML-RPC tag 'html' (HTTP {})", r.status().as_u16()));
        }
        xmlrpc::parse(&r.text().await.map_err(|e| e.to_string())?)
    }

    /// `download_list`.
    async fn download_list(&self) -> Result<Vec<String>, String> {
        let v = self.method_call("download_list", &[]).await?;
        Ok(v.as_array().map(|l| l.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default())
    }

    /// `fromBatches` of multicalls: `methods` for each hash, answers flattened; an empty list on a fault.
    async fn batched(&self, hashes: &[String], methods: &[(&str, &[&str])], what: &str) -> Vec<Value> {
        let mut out = Vec::new();
        for batch in hashes.chunks(BATCH_SIZE) {
            let calls: Vec<X> = batch
                .iter()
                .flat_map(|h| {
                    methods.iter().map(move |(m, extra)| {
                        let mut params = vec![h.as_str()];
                        params.extend_from_slice(extra);
                        mc(m, &params)
                    })
                })
                .collect();
            match self.method_call("system.multicall", &[X::List(calls)]).await {
                Ok(v) if is_fault(&v) => {
                    tracing::error!("cross-seed [{}]: Fault while getting {what}", self.info.label);
                }
                Ok(v) => out.extend(v.as_array().cloned().unwrap_or_default()),
                Err(e) => tracing::error!("cross-seed [{}]: Failed to get {what}: {e}", self.info.label),
            }
        }
        out
    }

    /// `isTorrentInClient`.
    pub async fn is_torrent_in_client(&self, info_hash: &str) -> Result<bool, String> {
        let h = info_hash.to_lowercase();
        Ok(self.download_list().await?.iter().any(|x| x.to_lowercase() == h))
    }

    /// `checkOriginalTorrent`.
    async fn check_original_torrent(&self, info_hash: &str, only_completed: bool) -> Result<Original, &'static str> {
        let hash = info_hash.to_uppercase();
        let calls: Vec<X> = ["d.name", "d.directory", "d.left_bytes", "d.hashing", "d.complete", "d.is_multi_file", "d.is_active"]
            .iter()
            .map(|m| mc(m, &[&hash]))
            .collect();
        let v = match self.method_call("system.multicall", &[X::List(calls)]).await {
            Ok(v) => v,
            Err(e) if e == COULD_NOT_FIND_INFO_HASH => return Err("NOT_FOUND"),
            Err(e) => {
                tracing::debug!("cross-seed [{}]: {e}", self.info.label);
                return Err("FAILURE");
            }
        };
        if is_fault(&v) {
            if v[0].get("faultString").and_then(Value::as_str) == Some(COULD_NOT_FIND_INFO_HASH) {
                return Err("NOT_FOUND");
            }
            tracing::error!("cross-seed [{}]: Unknown rTorrent fault while checking original torrent", self.info.label);
            return Err("FAILURE");
        }
        let Some(r) = v.as_array().filter(|r| r.len() == 7) else {
            tracing::error!("cross-seed [{}]: rTorrent's answer can't be read", self.info.label);
            return Err("FAILURE");
        };
        if only_completed && !truthy(&r[4].get(0).cloned().unwrap_or(Value::Null)) {
            return Err("TORRENT_NOT_COMPLETE");
        }
        let at = |i: usize| r[i].get(0).cloned().unwrap_or(Value::Null);
        Ok(Original {
            name: first_str(&r[0]),
            directory_base: first_str(&r[1]),
            bytes_left: num(&at(2)) as i64,
            hashing: truthy(&at(3)),
            is_multi_file: truthy(&at(5)),
            is_active: truthy(&at(6)),
        })
    }

    /// `validateConfig`.
    pub async fn validate_config(&self) -> Result<(), String> {
        self.download_list()
            .await
            .map_err(|e| format!("[{}] Failed to reach rTorrent at {}: {e}", self.info.label, self.info.client_host))?;
        tracing::info!("cross-seed [{}]: Logged in successfully{}", self.info.label, if self.info.readonly { " (readonly)" } else { "" });
        Ok(())
    }

    /// `getDownloadDir`.
    pub async fn get_download_dir(&self, info_hash: &str, only_completed: bool) -> Result<String, DirError> {
        match self.is_torrent_in_client(info_hash).await {
            Err(_) => return Err(DirError::UnknownError),
            Ok(false) => return Err(DirError::NotFound),
            Ok(true) => {}
        }
        match self.check_original_torrent(info_hash, only_completed).await {
            Ok(o) => Ok(if o.is_multi_file { dirname(&o.directory_base) } else { o.directory_base }),
            Err("TORRENT_NOT_COMPLETE") => Err(DirError::TorrentNotComplete),
            Err("NOT_FOUND") => Err(DirError::NotFound),
            Err(_) => Err(DirError::UnknownError),
        }
    }

    /// `getAllDownloadDirs`.
    pub async fn get_all_download_dirs(&self, only_completed: bool) -> HashMap<String, String> {
        let Ok(hashes) = self.download_list().await else { return HashMap::new() };
        let r = self
            .batched(
                &hashes,
                &[("d.directory", &[]), ("d.is_multi_file", &[]), ("d.complete", &[])],
                "download directories for all torrents",
            )
            .await;
        if r.is_empty() || r.len() != hashes.len() * 3 {
            tracing::error!("cross-seed [{}]: Unexpected number of results: {} for {} hashes", self.info.label, r.len(), hashes.len());
            return HashMap::new();
        }
        let mut out = HashMap::new();
        for (i, h) in hashes.iter().enumerate() {
            let dir = first_str(&r[i * 3]);
            let multi = truthy(&r[i * 3 + 1].get(0).cloned().unwrap_or(Value::Null));
            let complete = truthy(&r[i * 3 + 2].get(0).cloned().unwrap_or(Value::Null));
            if !only_completed || complete {
                out.insert(h.clone(), if multi { dirname(&dir) } else { dir });
            }
        }
        out
    }

    /// `isTorrentComplete`.
    pub async fn is_torrent_complete(&self, info_hash: &str) -> Option<bool> {
        let v = self.method_call("d.complete", &[X::Str(info_hash.into())]).await.ok()?;
        Some(truthy(match &v {
            Value::Array(a) => a.first()?,
            v => v,
        }))
    }

    /// `isTorrentChecking`.
    pub async fn is_torrent_checking(&self, info_hash: &str) -> Option<bool> {
        let v = self.method_call("d.hashing", &[X::Str(info_hash.into())]).await.ok()?;
        Some(truthy(match &v {
            Value::Array(a) => a.first()?,
            v => v,
        }))
    }

    /// `getAllTorrents`.
    pub async fn get_all_torrents(&self) -> Vec<TorrentInClient> {
        let Ok(hashes) = self.download_list().await else { return Vec::new() };
        let r = self.batched(&hashes, &[("d.custom1", &[])], "torrent info for all torrents").await;
        if r.len() != hashes.len() {
            tracing::error!("cross-seed [{}]: Unexpected number of results: {} for {} hashes", self.info.label, r.len(), hashes.len());
            return Vec::new();
        }
        hashes
            .iter()
            .zip(r)
            .map(|(h, v)| TorrentInClient {
                info_hash: h.to_lowercase(),
                tags: Some(tags_of(&first_str(&v))),
                ..TorrentInClient::default()
            })
            .collect()
    }

    /// `getClientSearchees`' listing.
    pub async fn list_torrents(&self, db: &SqliteDb, o: &SearcheeOptions) -> Option<(Vec<Searchee>, Vec<Searchee>, HashSet<String>)> {
        let (mut all, mut new, mut hashes_seen) = (Vec::new(), Vec::new(), HashSet::new());
        let hashes = match self.download_list().await {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("cross-seed [{}]: Failed to get client torrents: {e}", self.info.label);
                return None;
            }
        };
        let methods: [(&str, &[&str]); 7] = [
            ("d.name", &[]),
            ("d.size_bytes", &[]),
            ("d.directory", &[]),
            ("d.is_multi_file", &[]),
            ("d.custom1", &[]),
            ("f.multicall", &["", "f.path=", "f.size_bytes="]),
            ("t.multicall", &["", "t.url=", "t.group="]),
        ];
        let n = methods.len();
        let r = self.batched(&hashes, &methods, "client torrents").await;
        if r.is_empty() || r.len() != hashes.len() * n {
            tracing::error!("cross-seed [{}]: Unexpected number of results: {} for {} hashes", self.info.label, r.len(), hashes.len());
            return Some((all, new, hashes_seen));
        }
        for (i, h) in hashes.iter().enumerate() {
            let info_hash = h.to_lowercase();
            hashes_seen.insert(info_hash.clone());
            let kept = kept_searchee(db, &self.info.client_host, &info_hash);
            let name = first_str(&r[i * n]);
            let directory = first_str(&r[i * n + 2]);
            let multi = truthy(&r[i * n + 3].get(0).cloned().unwrap_or(Value::Null));
            let tags = tags_of(&first_str(&r[i * n + 4]));
            let save_path = if multi { dirname(&directory) } else { directory.clone() };
            if !client_searchee_modified(kept.as_ref(), &name, &save_path, None, Some(&tags)) && !o.refreshes(&info_hash) {
                if !o.new_searchees_only {
                    all.extend(kept);
                }
                continue;
            }
            let length = num(&r[i * n + 1].get(0).cloned().unwrap_or(Value::Null)) as i64;
            let files: Vec<File> = r[i * n + 5]
                .get(0)
                .and_then(Value::as_array)
                .map(|fs| {
                    fs.iter()
                        .map(|f| {
                            let p = f.get(0).and_then(Value::as_str).unwrap_or("").to_string();
                            File {
                                name: basename(&p),
                                path: if multi { format!("{}/{p}", basename(&directory)) } else { p },
                                length: num(f.get(1).unwrap_or(&Value::Null)) as i64,
                            }
                        })
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
            let urls: Vec<String> = r[i * n + 6]
                .get(0)
                .and_then(Value::as_array)
                .map(|ts| ts.iter().filter_map(|t| t.get(0).and_then(Value::as_str)).map(String::from).collect())
                .unwrap_or_default();
            let title = parse_title(&name, &files, None).unwrap_or_else(|| name.clone());
            let s = Searchee {
                info_hash: Some(info_hash),
                name,
                title,
                files,
                length,
                client_host: Some(self.info.client_host.clone()),
                save_path: Some(save_path),
                tags: Some(tags),
                trackers: Some(organize_trackers(urls.iter().map(String::as_str))),
                ..Searchee::default()
            };
            new.push(s.clone());
            all.push(s);
        }
        Some((all, new, hashes_seen))
    }

    /// `recheckTorrent`.
    pub async fn recheck_torrent(&self, info_hash: &str) {
        let _ = self.method_call("d.pause", &[X::Str(info_hash.into())]).await;
        let _ = self.method_call("d.check_hash", &[X::Str(info_hash.into())]).await;
    }

    /// `resumeInjection`. Like cross-seed, it keeps looking after resuming until the hour is over.
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
            let Ok(t) = self.check_original_torrent(info_hash, false).await else {
                sleep = timing.err_sleep;
                continue;
            };
            if t.hashing {
                continue;
            }
            let torrent_log = format!("{} [{}]", t.name, sanitize_info_hash(info_hash));
            if t.is_active {
                tracing::warn!("cross-seed [{}]: Will not resume torrent {torrent_log}: active", self.info.label);
                return;
            }
            if !may_resume(&self.info.label, &torrent_log, meta, decision, t.bytes_left, cfg) {
                return;
            }
            tracing::info!("cross-seed [{}]: Resuming torrent {torrent_log}", self.info.label);
            let _ = self.method_call("d.resume", &[X::Str(info_hash.clone())]).await;
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
        // getDownloadLocation
        let (directory_base, base_path) = match destination_dir {
            Some(d) => {
                let base = resolve(d, &meta.name);
                let dir_base = if meta.is_single_file_torrent { PathBuf::from(d) } else { base.clone() };
                (dir_base, base)
            }
            None => match self.check_original_torrent(searchee.info_hash.as_deref().unwrap_or(""), only_completed).await {
                Ok(o) => {
                    let base = if meta.is_single_file_torrent {
                        Path::new(&o.directory_base).join(&searchee.name)
                    } else {
                        PathBuf::from(&o.directory_base)
                    };
                    (PathBuf::from(o.directory_base), base)
                }
                Err("TORRENT_NOT_COMPLETE") => return (InjectionResult::TorrentNotComplete, false),
                Err(_) => return (InjectionResult::Failure, false),
            },
        };
        let resume = create_libtorrent_resume_tree(meta, &base_path);
        let Some(raw) = with_top_key(&meta.raw, "libtorrent_resume", &resume) else { return (InjectionResult::Failure, false) };
        let to_recheck = should_recheck(meta, decision, cfg);
        let load = if to_recheck { "load.raw" } else { "load.raw_start" };
        let mut params = vec![
            X::Str(String::new()),
            X::Bytes(raw),
            X::Str(format!("d.directory_base.set=\"{}\"", directory_base.to_string_lossy())),
            X::Str(format!("d.custom1.set=\"{TORRENT_TAG}\"")),
            X::Str(format!("d.custom.set=addtime,{}", (crate::auth::now()).round() as i64)),
        ];
        if to_recheck {
            params.push(X::Str(format!("d.check_hash={}", meta.info_hash.to_uppercase())));
        }
        let mut resume_after = false;
        for i in 0..5u32 {
            match self.method_call(load, &params).await {
                Ok(_) => {
                    resume_after = to_recheck;
                    break;
                }
                Err(e) => {
                    tracing::debug!("cross-seed [{}]: Failed to inject torrent {} on attempt {}/5: {e}", self.info.label, meta.name, i + 1);
                    tokio::time::sleep(Duration::from_millis(1000 * 2u64.pow(i))).await;
                }
            }
        }
        for i in 0..5u32 {
            if self.is_torrent_in_client(&meta.info_hash).await.unwrap_or(false) {
                return (InjectionResult::Success, resume_after);
            }
            tokio::time::sleep(Duration::from_millis(100 * 2u64.pow(i))).await;
        }
        (InjectionResult::Failure, resume_after)
    }
}

/// `d.custom1`'s labels: percent-decoded, split at commas.
fn tags_of(labels: &str) -> Vec<String> {
    if labels.is_empty() {
        return Vec::new();
    }
    percent_encoding::percent_decode_str(labels).decode_utf8_lossy().split(',').map(|t| t.trim().to_string()).collect()
}
