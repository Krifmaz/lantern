// Ported from cross-seed v6.13.9 (src/torrent.ts: the torrent cache, indexing and the lookups by name), Apache-2.0.
//! What cross-seed knows it has: the torrents of `torrentDir` (kept in `cross_seed_torrents`, cross-seed's
//! `torrent` table), the client torrents (`cross_seed_client_searchees`), the data of `dataDirs` and the episode
//! files seasons can be made from ([`CrossSeed::index_torrents_and_data_dirs`]); and the lookups an announce or an
//! RSS candidate needs: the searchees like a name ([`CrossSeed::get_similar_by_name`]) and the one with an info
//! hash ([`CrossSeed::get_torrent_by_criteria`]).

use super::clients::{Client, SearcheeOptions, TorrentInClient};
use super::config::{is_child_path, RuntimeConfig};
use super::constants::{
    MediaType, ALL_PARENTHESES_REGEX, ALL_SPACES_REGEX, ALL_SQUARE_BRACKETS_REGEX, LEVENSHTEIN_DIVISOR, MIN_VIDEO_QUERY_LENGTH,
};
use super::data::{
    create_ensemble_pieces, forget_paths, get_data_by_fuzzy_name, save_ensemble, should_ignore_path_heuristically, EnsembleEntry,
};
use super::jsre::{re, replace_all};
use super::searchee::{
    create_searchee_from_metafile, create_searchee_from_path, get_anime_keys, get_episode_keys, get_movie_keys, get_season_keys, Searchee,
};
use super::torrent::{sanitize_tracker_url, Metafile};
use super::utils::{create_key_title, distance, strip_extension};
use super::CrossSeed;
use kit_server_core::{p, Db, RowExt, SqliteDb};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// `FilenameMetadata`: what a saved `.torrent` file's name says (`[mediaType][tracker]name[infoHash].torrent`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilenameMetadata {
    /// The media type.
    pub media_type: String,
    /// The tracker.
    pub tracker: String,
    /// The name.
    pub name: String,
    /// The info hash, when the name has one.
    pub info_hash: Option<String>,
    /// Whether it is a `.cached.torrent`.
    pub cached: bool,
}

/// `parseMetadataFromFilename`: the parts of a saved torrent's file name, or `None` when it isn't named that way
/// (or its media type isn't one of cross-seed's).
pub fn parse_metadata_from_filename(filename: &str) -> Option<FilenameMetadata> {
    static RE: std::sync::LazyLock<fancy_regex::Regex> = std::sync::LazyLock::new(|| {
        re(r"^\[(?<mediaType>.+?)\]\[(?<tracker>.+?)\](?<name>.+?)(?:\[(?<infoHash>[a-z0-9]{40})\])?(?<cached>.cached)?\.torrent$", "i")
    });
    let c = RE.captures(filename).ok()??;
    let media_type = c.name("mediaType")?.as_str().to_string();
    if !MediaType::ALL.iter().any(|m| m.as_str() == media_type) {
        return None;
    }
    Some(FilenameMetadata {
        media_type,
        tracker: c.name("tracker")?.as_str().to_string(),
        name: c.name("name")?.as_str().to_string(),
        info_hash: c.name("infoHash").map(|m| m.as_str().to_string()),
        cached: c.name("cached").is_some(),
    })
}

/// `findAllTorrentFilesInDir`: the `.torrent` files in `dir`, by name, as absolute paths.
pub fn find_all_torrent_files_in_dir(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| super::utils::extname(n) == ".torrent")
        .collect();
    names.sort();
    names.into_iter().map(|n| super::action::absolute(&dir.join(n)).to_string_lossy().into_owned()).collect()
}

/// `updateMetafileMetadata`: the category, tags and trackers of a qBittorrent `.fastresume` file.
pub fn update_metafile_metadata(meta: &mut Metafile, fastresume: &[u8]) {
    let Some(d) = super::torrent::decode_value(fastresume) else { return };
    if let Some(c) = d.get("qBt-category") {
        meta.category = Some(c.js_string());
    }
    if let Some(t) = d.get("qBt-tags") {
        meta.tags = Some(t.js_string().split(',').map(str::to_string).collect());
    }
    if let Some(tiers) = d.get("trackers").and_then(|t| t.list()) {
        meta.trackers = tiers
            .iter()
            .flat_map(|tier| tier.list().map(|l| l.to_vec()).unwrap_or_default())
            .filter_map(|u| u.text().and_then(|u| sanitize_tracker_url(&u)))
            .collect();
    }
}

/// `parseTorrentWithMetadata`: a `.torrent` with its category, tags and trackers: from qBittorrent's
/// `.fastresume` next to it when the first client is qBittorrent, else from what the client says of it.
pub fn parse_torrent_with_metadata(
    path: &str,
    infos: &[TorrentInClient],
    first_is_qbittorrent: bool,
    have_clients: bool,
) -> Result<Metafile, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut meta = Metafile::decode(&bytes)?;
    if first_is_qbittorrent {
        let fast = Path::new(path).with_extension("fastresume");
        if let Ok(b) = std::fs::read(&fast) {
            update_metafile_metadata(&mut meta, &b);
            return Ok(meta);
        }
        tracing::debug!(
            "cross-seed: No .fastresume found at {} from {path}, using client to get metadata without trackers",
            fast.display()
        );
    }
    if let Some(info) = infos.iter().find(|t| t.info_hash == meta.info_hash) {
        meta.category = info.category.clone();
        meta.tags = info.tags.clone();
        if meta.trackers.is_empty() {
            if let Some(t) = &info.trackers {
                meta.trackers = t.clone();
            }
        }
    } else if have_clients {
        tracing::debug!("cross-seed: No torrent info found for {} from {path}", meta.info_hash);
    }
    Ok(meta)
}

/// `createSearcheeFromTorrentFile`.
pub fn create_searchee_from_torrent_file(
    path: &str,
    infos: &[TorrentInClient],
    first_is_qbittorrent: bool,
    have_clients: bool,
) -> Result<Searchee, String> {
    match parse_torrent_with_metadata(path, infos, first_is_qbittorrent, have_clients).and_then(|m| create_searchee_from_metafile(&m)) {
        Ok(s) => Ok(s),
        Err(e) => {
            tracing::error!("cross-seed: Failed to parse {}: {e}", super::searchee::basename(path));
            Err(e)
        }
    }
}

/// The element a name's keys come with: the episode (with its season), the season, or the anime release;
/// a number and a text are never equal, as in cross-seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
    /// `S1.5`, `5`, `S1`.
    Text(String),
    /// An anime release number.
    Num(i64),
}

impl Element {
    /// JavaScript's truthiness of it (`0` and `""` are false).
    fn truthy(&self) -> bool {
        match self {
            Element::Text(t) => !t.is_empty(),
            Element::Num(n) => *n != 0,
        }
    }

    /// As text.
    fn text(&self) -> String {
        match self {
            Element::Text(t) => t.clone(),
            Element::Num(n) => n.to_string(),
        }
    }
}

/// `getKeysFromName`: the key titles of a name and its element, or the fuzzy fallback when it has none.
pub fn get_keys_from_name(stem: &str) -> (Vec<String>, Option<Element>, bool) {
    if let Some(k) = get_episode_keys(stem) {
        let element = match &k.season {
            Some(s) => format!("{s}.{}", k.element),
            None => k.element.clone(),
        };
        return (k.key_titles, Some(Element::Text(element)), false);
    }
    if let Some(k) = get_season_keys(stem) {
        return (k.key_titles, k.season.map(Element::Text), false);
    }
    if let Some(k) = get_movie_keys(stem) {
        return (k.key_titles, None, false);
    }
    if let Some(k) = get_anime_keys(stem) {
        return (k.key_titles, Some(Element::Num(k.element.parse().unwrap_or(0))), false);
    }
    (Vec::new(), None, true)
}

/// What [`CrossSeed::get_similar_by_name`] found.
#[derive(Debug, Clone, Default)]
pub struct Similar {
    /// The keys looked up with (`title.element`), empty for the fuzzy fallback.
    pub keys: Vec<String>,
    /// Torrents in the clients or `torrentDir`.
    pub client_searchees: Vec<Searchee>,
    /// Data from `dataDirs`.
    pub data_searchees: Vec<Searchee>,
}

/// A kept torrent or data row's name, for the lookups by key.
#[derive(Debug, Clone)]
struct Entry {
    /// `title` (client torrents, data) or `name` (`torrentDir`).
    title: String,
    /// The row's key: the info hash and client (client torrents), the file (`torrentDir`), the path (data).
    key: String,
    /// The client (client torrents).
    client_host: String,
}

/// The rows whose keys are like `key_titles` with the same element (`filterEntries`).
fn filter_entries(entries: Vec<Entry>, key_titles: &[String], element: &Option<Element>) -> Vec<Entry> {
    let len = |s: &str| s.encode_utf16().count();
    let candidate_max = key_titles.iter().map(|t| len(t)).min().unwrap_or(0) / LEVENSHTEIN_DIVISOR;
    entries
        .into_iter()
        .filter(|e| {
            let (keys, el, _) = get_keys_from_name(&e.title);
            if &el != element || keys.is_empty() {
                return false;
            }
            let max = candidate_max.min(keys.iter().map(|t| len(t)).min().unwrap_or(0) / LEVENSHTEIN_DIVISOR);
            keys.iter().any(|k| key_titles.iter().any(|t| distance(t, k) <= max))
        })
        .collect()
}

/// The kept torrents of `torrentDir`.
fn torrent_rows(db: &SqliteDb) -> Vec<(String, String, String)> {
    db.query("SELECT info_hash, name, file_path FROM cross_seed_torrents ORDER BY id", p![])
        .map(|rows| rows.iter().map(|r| (r.s("info_hash"), r.s("name"), r.s("file_path"))).collect())
        .unwrap_or_default()
}

impl CrossSeed {
    /// The first client and whether it is qBittorrent, and the torrents it has when it isn't (what
    /// `loadTorrentDirLight` and the lookups read `.torrent` files with).
    pub(super) async fn torrent_infos(&self, clients: &[Arc<Client>]) -> (Vec<TorrentInClient>, bool) {
        match clients.first() {
            Some(c) if c.info.client_type == "qbittorrent" => (Vec::new(), true),
            Some(c) => (c.get_all_torrents().await, false),
            None => (Vec::new(), false),
        }
    }

    /// `loadTorrentDirLight`: every `.torrent` of `dir` as a searchee.
    pub async fn load_torrent_dir_light(&self, dir: &str, cfg: &RuntimeConfig) -> Vec<Searchee> {
        let clients = self.clients(cfg).await;
        let (infos, qbit) = self.torrent_infos(&clients).await;
        find_all_torrent_files_in_dir(Path::new(dir))
            .iter()
            .filter_map(|p| create_searchee_from_torrent_file(p, &infos, qbit, !clients.is_empty()).ok())
            .collect()
    }

    /// `indexTorrentDir`: keeps the new `.torrent` files of `dir` and forgets the gone ones; the new ones as
    /// searchees.
    fn index_torrent_dir(&self, dir: &str, qbit: bool, have_clients: bool) -> Vec<Searchee> {
        let files: HashSet<String> = find_all_torrent_files_in_dir(Path::new(dir)).into_iter().collect();
        let mut sorted: Vec<&String> = files.iter().collect();
        sorted.sort();
        let mut new = Vec::new();
        for path in sorted {
            if self.db().one("SELECT id FROM cross_seed_torrents WHERE file_path=?", p![path.clone()]).ok().flatten().is_some() {
                continue;
            }
            let meta = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Metafile::decode(&b)) {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!("cross-seed: Failed to parse {path}: {e}");
                    continue;
                }
            };
            let _ = self.db().execute(
                "INSERT INTO cross_seed_torrents (file_path, info_hash, name) VALUES (?,?,?) ON CONFLICT(file_path) DO NOTHING",
                p![path.clone(), meta.info_hash.clone(), meta.name.clone()],
            );
            if let Ok(s) = create_searchee_from_torrent_file(path, &[], qbit, have_clients) {
                new.push(s);
            }
        }
        for (hash, _, path) in torrent_rows(self.db()) {
            if !files.contains(&path) {
                let _ = self.db().execute("DELETE FROM cross_seed_torrents WHERE file_path=?", p![path]);
                let _ = self.db().execute("DELETE FROM cross_seed_ensemble WHERE info_hash=?", p![hash]);
            }
        }
        new
    }

    /// `cacheEnsembleTorrentEntry`: the ensemble rows of a torrent's episode, at its folder in the client.
    pub(super) async fn cache_ensemble_torrent_entry(
        &self,
        s: &Searchee,
        save_paths: Option<&HashMap<String, String>>,
        clients: &[Arc<Client>],
        cfg: &RuntimeConfig,
    ) -> Option<Vec<EnsembleEntry>> {
        let pieces = create_ensemble_pieces(&s.title, &s.files)?;
        if pieces.is_empty() {
            return None;
        }
        let hash = s.info_hash.clone()?;
        let save_path = if let Some(sp) = &s.save_path {
            Some(sp.clone())
        } else if let Some(map) = save_paths {
            map.get(&hash).cloned()
        } else {
            match clients.first()?.get_download_dir(super::clients::MetaRef::of_searchee(s), false, cfg).await {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::error!("cross-seed: Failed to get download dir for {}: {e:?}", s.name);
                    return None;
                }
            }
        };
        let Some(save_path) = save_path else {
            tracing::error!("cross-seed: Failed to get save path for {}", s.name);
            return None;
        };
        Some(
            pieces
                .into_iter()
                .map(|(ensemble, element, largest)| EnsembleEntry {
                    client_host: s.client_host.clone(),
                    path: Path::new(&save_path).join(&largest.path).to_string_lossy().into_owned(),
                    info_hash: Some(hash.clone()),
                    ensemble,
                    element,
                })
                .collect(),
        )
    }

    /// `indexTorrents`: at `startup` every torrent of `torrentDir` or the clients; afterwards the new ones only;
    /// then their episodes for seasons.
    async fn index_torrents(&self, cfg: &RuntimeConfig, startup: bool) {
        if !cfg.use_client_torrents && cfg.torrent_dir.is_none() {
            return;
        }
        let clients = self.clients(cfg).await;
        let qbit = clients.first().is_some_and(|c| c.info.client_type == "qbittorrent");
        let mut save_paths: Option<HashMap<String, String>> = None;
        let searchees: Vec<Searchee> = if startup {
            if let Some(dir) = &cfg.torrent_dir {
                tracing::info!("cross-seed: Indexing torrentDir for reverse lookup...");
                let list = self.load_torrent_dir_light(dir, cfg).await;
                if let Some(c) = clients.first() {
                    let metas: Vec<_> = list.iter().map(super::clients::MetaRef::of_searchee).collect();
                    save_paths = Some(c.get_all_download_dirs(&metas, false, cfg).await);
                }
                list
            } else {
                tracing::info!("cross-seed: Indexing client torrents for reverse lookup...");
                let mut all = Vec::new();
                let o = SearcheeOptions { include_files: true, include_trackers: true, ..SearcheeOptions::default() };
                for c in &clients {
                    all.extend(c.get_client_searchees(self.db(), cfg, &o).await.searchees);
                }
                all
            }
        } else if let Some(dir) = &cfg.torrent_dir {
            self.index_torrent_dir(dir, qbit, !clients.is_empty())
        } else {
            let mut all = Vec::new();
            let o = SearcheeOptions { new_searchees_only: true, include_files: true, include_trackers: true, ..SearcheeOptions::default() };
            for c in &clients {
                all.extend(c.get_client_searchees(self.db(), cfg, &o).await.new_searchees);
            }
            all
        };
        if cfg.season_from_episodes.is_none() {
            return;
        }
        let mut rows = Vec::new();
        for s in &searchees {
            if let Some(r) = self.cache_ensemble_torrent_entry(s, save_paths.as_ref(), &clients, cfg).await {
                rows.extend(r);
            }
        }
        save_ensemble(self.db(), &rows);
    }

    /// The tidying `indexTorrentsAndDataDirs` does at start: forgets what belongs to settings no longer in use.
    fn forget_unused(&self, cfg: &RuntimeConfig, client_hosts: &[String]) {
        let db = self.db();
        if !cfg.use_client_torrents {
            let _ =
                db.execute("DELETE FROM cross_seed_ensemble WHERE info_hash IN (SELECT info_hash FROM cross_seed_client_searchees)", p![]);
            let _ = db.execute("DELETE FROM cross_seed_client_searchees", p![]);
        } else {
            let hosts: HashSet<&String> = client_hosts.iter().collect();
            let kept: Vec<String> = db
                .query("SELECT DISTINCT client_host FROM cross_seed_client_searchees", p![])
                .map(|r| r.iter().map(|r| r.s("client_host")).collect())
                .unwrap_or_default();
            for h in kept.iter().filter(|h| !hosts.contains(h)) {
                let _ = db.execute("DELETE FROM cross_seed_client_searchees WHERE client_host=?", p![h.clone()]);
            }
            let kept: Vec<String> = db
                .query("SELECT DISTINCT client_host FROM cross_seed_ensemble WHERE client_host<>''", p![])
                .map(|r| r.iter().map(|r| r.s("client_host")).collect())
                .unwrap_or_default();
            for h in kept.iter().filter(|h| !hosts.contains(h)) {
                let _ = db.execute("DELETE FROM cross_seed_ensemble WHERE client_host=?", p![h.clone()]);
            }
        }
        if cfg.torrent_dir.is_none() {
            let _ = db.execute("DELETE FROM cross_seed_ensemble WHERE info_hash IN (SELECT info_hash FROM cross_seed_torrents)", p![]);
            let _ = db.execute("DELETE FROM cross_seed_torrents", p![]);
        }
        let paths: Vec<String> =
            db.query("SELECT path FROM cross_seed_data", p![]).map(|r| r.iter().map(|r| r.s("path")).collect()).unwrap_or_default();
        let gone: Vec<String> =
            if cfg.data_dirs.is_empty() { paths } else { paths.into_iter().filter(|p| !is_child_path(p, &cfg.data_dirs)).collect() };
        for path in &gone {
            let _ = db.execute("DELETE FROM cross_seed_data WHERE path=?", p![path.clone()]);
            let _ = db.execute("DELETE FROM cross_seed_ensemble WHERE path=? AND client_host=''", p![path.clone()]);
        }
        if cfg.season_from_episodes.is_none() {
            let _ = db.execute("DELETE FROM cross_seed_ensemble", p![]);
        }
    }

    /// `indexTorrentsAndDataDirs`: indexes the data folders, then the torrents (up to three tries); one indexing
    /// at a time, a second call waits for the running one.
    pub async fn index_torrents_and_data_dirs(&self, cfg: &RuntimeConfig, startup: bool) {
        if startup {
            let hosts: Vec<String> = self.clients(cfg).await.iter().map(|c| c.info.client_host.clone()).collect();
            self.forget_unused(cfg, &hosts);
        }
        let _guard = match self.indexing.try_lock() {
            Ok(g) => g,
            Err(_) => {
                let _wait = self.indexing.lock().await;
                return;
            }
        };
        if let Ok(mut d) = self.data_index.lock() {
            d.index(self.db(), cfg, startup);
        }
        self.index_torrents(cfg, startup).await;
    }

    /// `getInfoHashesToExclude`: the info hashes of every torrent in the clients (or `torrentDir`).
    pub fn info_hashes_to_exclude_for(&self, cfg: &RuntimeConfig) -> HashSet<String> {
        let sql = if cfg.use_client_torrents {
            "SELECT info_hash FROM cross_seed_client_searchees"
        } else {
            "SELECT info_hash FROM cross_seed_torrents"
        };
        self.db().query(sql, p![]).map(|r| r.iter().map(|r| r.s("info_hash")).collect()).unwrap_or_default()
    }

    /// The kept client torrent rows as entries.
    fn client_entries(&self) -> Vec<Entry> {
        self.db()
            .query("SELECT info_hash, title, client_host FROM cross_seed_client_searchees", p![])
            .map(|rows| {
                rows.iter().map(|r| Entry { title: r.s("title"), key: r.s("info_hash"), client_host: r.s("client_host") }).collect()
            })
            .unwrap_or_default()
    }

    /// The client torrents (or `torrentDir` torrents) for entries found.
    async fn searchees_of(&self, entries: &[Entry], cfg: &RuntimeConfig) -> Vec<Searchee> {
        if entries.is_empty() {
            return Vec::new();
        }
        if cfg.use_client_torrents {
            return entries.iter().filter_map(|e| super::clients::kept_searchee(self.db(), &e.client_host, &e.key)).collect();
        }
        let clients = self.clients(cfg).await;
        let (infos, qbit) = self.torrent_infos(&clients).await;
        entries.iter().filter_map(|e| create_searchee_from_torrent_file(&e.key, &infos, qbit, !clients.is_empty()).ok()).collect()
    }

    /// The kept `torrentDir` torrents as entries.
    fn torrent_dir_entries(&self) -> Vec<Entry> {
        torrent_rows(self.db()).into_iter().map(|(_, name, path)| Entry { title: name, key: path, client_host: String::new() }).collect()
    }

    /// `getTorrentByFuzzyName`: the torrents within a third of the name's length in edits of `stem`.
    async fn get_torrent_by_fuzzy_name(&self, stem: &str, cfg: &RuntimeConfig) -> Vec<Searchee> {
        let all = if cfg.use_client_torrents { self.client_entries() } else { self.torrent_dir_entries() };
        let full = create_key_title(stem);
        let mut filtered = Vec::new();
        if let Some(full) = &full {
            filtered = all.iter().filter(|e| create_key_title(&strip_extension(&e.title)).as_ref() == Some(full)).cloned().collect();
        }
        if filtered.is_empty() {
            filtered = all;
        }
        let len = |s: &str| s.encode_utf16().count();
        let candidate_max = len(stem) / LEVENSHTEIN_DIVISOR;
        let matches: Vec<Entry> = filtered
            .into_iter()
            .filter(|e| {
                let t = strip_extension(&e.title);
                distance(stem, &t) <= candidate_max.min(len(&t) / LEVENSHTEIN_DIVISOR)
            })
            .collect();
        self.searchees_of(&matches, cfg).await
    }

    /// `getSimilarByName`: the torrents and data like `name`: by its key titles and element, else by the whole
    /// name within a third of its length in edits.
    pub async fn get_similar_by_name(&self, name: &str, cfg: &RuntimeConfig) -> Similar {
        let stem = strip_extension(name);
        let (key_titles, element, fallback) = get_keys_from_name(&stem);
        let mut out = Similar::default();
        if fallback {
            out.client_searchees = self.get_torrent_by_fuzzy_name(&stem, cfg).await;
            out.data_searchees = get_data_by_fuzzy_name(self.db(), &stem);
        }
        if key_titles.is_empty() {
            let no_brackets = replace_all(&ALL_SQUARE_BRACKETS_REGEX, &stem, "");
            let no_parens = replace_all(&ALL_SPACES_REGEX, &replace_all(&ALL_PARENTHESES_REGEX, &no_brackets, ""), " ").trim().to_string();
            if no_parens.encode_utf16().count() >= MIN_VIDEO_QUERY_LENGTH && no_parens != stem {
                out.client_searchees.extend(self.get_torrent_by_fuzzy_name(&no_parens, cfg).await);
                out.data_searchees.extend(get_data_by_fuzzy_name(self.db(), &no_parens));
            }
            return out;
        }
        if cfg.use_client_torrents {
            let found = filter_entries(self.client_entries(), &key_titles, &element);
            out.client_searchees.extend(self.searchees_of(&found, cfg).await);
        } else if cfg.torrent_dir.is_some() {
            let found = filter_entries(self.torrent_dir_entries(), &key_titles, &element);
            out.client_searchees.extend(self.searchees_of(&found, cfg).await);
        }
        let data: Vec<Entry> = self
            .db()
            .query("SELECT path, title FROM cross_seed_data ORDER BY rowid", p![])
            .map(|rows| rows.iter().map(|r| Entry { title: r.s("title"), key: r.s("path"), client_host: String::new() }).collect())
            .unwrap_or_default();
        let mut gone = Vec::new();
        let found: Vec<Entry> = filter_entries(data, &key_titles, &element)
            .into_iter()
            .filter(|e| {
                let keep =
                    std::fs::metadata(&e.key).map(|m| !should_ignore_path_heuristically(Path::new(&e.key), m.is_dir())).unwrap_or(false);
                if !keep {
                    gone.push(e.key.clone());
                }
                keep
            })
            .collect();
        forget_paths(self.db(), &gone);
        out.data_searchees.extend(found.iter().filter_map(|e| create_searchee_from_path(Path::new(&e.key)).ok()));
        out.keys = match &element {
            Some(el) if el.truthy() => key_titles.iter().map(|k| format!("{k}.{}", el.text())).collect(),
            _ => key_titles,
        };
        out
    }

    /// `getTorrentByCriteria` by info hash: the client torrents (or the `torrentDir` torrent) with it.
    pub async fn get_torrent_by_criteria(&self, info_hash: &str, cfg: &RuntimeConfig) -> Result<Vec<Searchee>, String> {
        let not_found = || format!("Torrent client does not have any torrent with criteria {{ infoHash: '{info_hash}' }}");
        if cfg.use_client_torrents {
            let rows: Vec<Entry> = self
                .db()
                .query("SELECT info_hash, title, client_host FROM cross_seed_client_searchees WHERE info_hash=?", p![info_hash])
                .map(|rows| {
                    rows.iter().map(|r| Entry { title: r.s("title"), key: r.s("info_hash"), client_host: r.s("client_host") }).collect()
                })
                .unwrap_or_default();
            if rows.is_empty() {
                return Err(not_found());
            }
            return Ok(self.searchees_of(&rows, cfg).await);
        }
        let row = self.db().one("SELECT file_path FROM cross_seed_torrents WHERE info_hash=?", p![info_hash]).ok().flatten();
        let Some(row) = row else { return Err(not_found()) };
        let path = row.s("file_path");
        let clients = self.clients(cfg).await;
        let (infos, qbit) = self.torrent_infos(&clients).await;
        create_searchee_from_torrent_file(&path, &infos, qbit, !clients.is_empty())
            .map(|s| vec![s])
            .map_err(|_| format!("Failed to create searchee from {path}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_torrent_names_as_cross_seed_reads_them() {
        let h = "a".repeat(40);
        let m = parse_metadata_from_filename(&format!("[movie][Site B]Movie.2020.1080p-GRP[{h}].torrent")).unwrap();
        assert_eq!(
            (m.media_type.as_str(), m.tracker.as_str(), m.name.as_str(), m.info_hash.as_deref(), m.cached),
            ("movie", "Site B", "Movie.2020.1080p-GRP", Some(h.as_str()), false)
        );
        let m = parse_metadata_from_filename(&format!("[episode][T]Show.S01E01[{h}].cached.torrent")).unwrap();
        assert!(m.cached);
        assert!(parse_metadata_from_filename("[nope][T]x.torrent").is_none(), "not a media type");
        assert!(parse_metadata_from_filename("Movie.2020.torrent").is_none());
        assert!(parse_metadata_from_filename("[season][T]x.torrent").is_none(), "a season pack is `pack`");
        assert_eq!(parse_metadata_from_filename("[pack][T]Show.S01.torrent").unwrap().media_type, "pack");
        let m = parse_metadata_from_filename("[video][T]Some.Name.torrent").unwrap();
        assert_eq!((m.name.as_str(), m.info_hash), ("Some.Name", None));
    }

    #[test]
    fn keys_from_names() {
        let (k, e, f) = get_keys_from_name("The.Show.S01E05.1080p-GRP");
        assert_eq!((k[0].as_str(), e, f), ("theshow", Some(Element::Text("S1.5".into())), false));
        let (_, e, _) = get_keys_from_name("The.Show.S02.1080p-GRP");
        assert_eq!(e, Some(Element::Text("S2".into())));
        let (k, e, _) = get_keys_from_name("A.Movie.2020.1080p-GRP");
        assert_eq!((k[0].as_str(), e), ("amovie", None));
        let (k, e, f) = get_keys_from_name("[Grp] Some Anime - 12 [1080p]");
        assert_eq!((k.is_empty(), e, f), (false, Some(Element::Num(12)), false));
        assert!(get_keys_from_name("readme").2, "fuzzy fallback");
        let entries = vec![
            Entry { title: "The.Show.S01E05.720p-OTHER".into(), key: "1".into(), client_host: String::new() },
            Entry { title: "The.Show.S01E06.1080p-GRP".into(), key: "2".into(), client_host: String::new() },
            Entry { title: "The.Shows.S01E05.1080p-GRP".into(), key: "3".into(), client_host: String::new() },
        ];
        let (k, e, _) = get_keys_from_name("The.Show.S01E05.1080p-GRP");
        let found: Vec<String> = filter_entries(entries, &k, &e).into_iter().map(|e| e.key).collect();
        assert_eq!(found, vec!["1", "3"], "the same episode, a title within a third of its length in edits");
    }
}
