// Ported from cross-seed v6.13.9 (src/dataFiles.ts), Apache-2.0.
//! Data-based searching: the folders of downloaded files in `dataDirs`, looked through `maxDataDepth` levels
//! down for releases ([`find_potential_nested_roots`]), kept in `cross_seed_data` (cross-seed's `data` table) with
//! their episodes in `cross_seed_ensemble` for seasons made from episodes, and found again by name
//! ([`get_data_by_fuzzy_name`]).
//!
//! cross-seed watches the data folders for changes; Lantern instead looks them over again, at most once a minute,
//! and indexes again only the releases whose files changed (see [`DataIndex`]). The rows are the same.

use super::config::RuntimeConfig;
use super::constants::{IGNORED_FOLDERS_SUBSTRINGS, LEVENSHTEIN_DIVISOR, VIDEO_EXTENSIONS};
use super::searchee::{basename, create_searchee_from_path, dirname, get_files_from_data_root, parse_title, File, Searchee};
use super::utils::{create_key_title, distance, extname, strip_extension};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One row of `cross_seed_ensemble` (cross-seed's `EnsembleEntry`): an episode file a season can be made from.
#[derive(Debug, Clone, PartialEq)]
pub struct EnsembleEntry {
    /// The client the file's torrent is in (`None` for data).
    pub client_host: Option<String>,
    /// The episode's largest file, on disk.
    pub path: String,
    /// The torrent's info hash (`None` for data).
    pub info_hash: Option<String>,
    /// The season's key: the key title and season (`showname.S1`).
    pub ensemble: String,
    /// The episode (`5`, or `MM.DD` for a daily show).
    pub element: String,
}

/// Keeps ensemble rows (`insert ... onConflict(["client_host", "path"]).merge()`). Data rows, which have no
/// client, are kept under the client `''` so a second indexing replaces the first.
pub fn save_ensemble(db: &SqliteDb, rows: &[EnsembleEntry]) {
    for r in rows {
        let res = db.execute(
            "INSERT INTO cross_seed_ensemble (client_host, path, info_hash, ensemble, element) VALUES (?,?,?,?,?)
             ON CONFLICT(client_host, path) DO UPDATE SET info_hash=excluded.info_hash, ensemble=excluded.ensemble, element=excluded.element",
            p![r.client_host.clone().unwrap_or_default(), r.path.clone(), r.info_hash.clone(), r.ensemble.clone(), r.element.clone()],
        );
        if let Err(e) = res {
            tracing::error!("cross-seed: the episode list can't be kept: {}", e.detail);
        }
    }
}

/// Forgets data and ensemble rows of `paths` (`db("data").whereIn("path", batch).del()` and the ensemble's).
pub fn forget_paths(db: &SqliteDb, paths: &[String]) {
    for path in paths {
        let _ = db.execute("DELETE FROM cross_seed_data WHERE path=?", p![path.clone()]);
        let _ = db.execute("DELETE FROM cross_seed_ensemble WHERE path=?", p![path.clone()]);
    }
}

/// `createEnsemblePieces`: the season keys an episode belongs to, its episode and its largest file; `None` when
/// the title isn't an episode.
pub fn create_ensemble_pieces(title: &str, files: &[File]) -> Option<Vec<(String, String, File)>> {
    let keys = super::searchee::get_episode_keys(&strip_extension(title))?;
    let largest = super::searchee::get_largest_file(files)?.clone();
    Some(
        keys.key_titles
            .iter()
            .map(|k| {
                let key = match &keys.season {
                    Some(s) => format!("{k}.{s}"),
                    None => k.clone(),
                };
                (key, keys.element.clone(), largest.clone())
            })
            .collect(),
    )
}

/// `indexEnsembleDataEntry`: the ensemble rows of one data release.
fn index_ensemble_data_entry(title: &str, path: &str, files: &[File]) -> Option<Vec<EnsembleEntry>> {
    let pieces = create_ensemble_pieces(title, files)?;
    if pieces.is_empty() {
        return None;
    }
    let parent = dirname(path);
    Some(
        pieces
            .into_iter()
            .map(|(ensemble, element, largest)| EnsembleEntry {
                client_host: None,
                path: Path::new(&parent).join(&largest.path).to_string_lossy().into_owned(),
                info_hash: None,
                ensemble,
                element,
            })
            .collect(),
    )
}

/// `indexDataPaths`: keeps the data and ensemble rows of `paths`; how many releases were kept.
pub fn index_data_paths(db: &SqliteDb, cfg: &RuntimeConfig, paths: &[PathBuf]) -> usize {
    let mut kept = 0;
    let mut ensemble = Vec::new();
    for path in paths {
        let files = get_files_from_data_root(path);
        if files.is_empty() {
            continue;
        }
        let shown = path.to_string_lossy().into_owned();
        let Some(title) = parse_title(&basename(&shown), &files, Some(&shown)) else { continue };
        let res = db.execute(
            "INSERT INTO cross_seed_data (path, title) VALUES (?,?) ON CONFLICT(path) DO UPDATE SET title=excluded.title",
            p![shown.clone(), title.clone()],
        );
        if let Err(e) = res {
            tracing::error!("cross-seed: {shown} can't be kept: {}", e.detail);
            continue;
        }
        kept += 1;
        if cfg.season_from_episodes.is_some() {
            if let Some(rows) = index_ensemble_data_entry(&title, &shown, &files) {
                ensemble.extend(rows);
            }
        }
    }
    save_ensemble(db, &ensemble);
    kept
}

/// `shouldIgnorePathHeuristically`: a folder named like a sample, proof or disc structure, or a file that isn't
/// video.
pub fn should_ignore_path_heuristically(root: &Path, is_dir: bool) -> bool {
    let base = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if is_dir {
        IGNORED_FOLDERS_SUBSTRINGS.contains(&base.to_lowercase().as_str())
    } else {
        !VIDEO_EXTENSIONS.contains(&extname(&base))
    }
}

/// The entries of folder `dir`, by name (cross-seed takes them as the system lists them).
fn read_dir_sorted(dir: &Path) -> std::io::Result<Vec<(PathBuf, bool)>> {
    let mut out: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (e.path(), is_dir)
        })
        .collect();
    out.sort();
    Ok(out)
}

/// `findPotentialNestedRoots`: `root` and the folders and video files under it, `depth` levels down, deepest
/// first; samples, proofs, disc folders and non-video files are left out.
pub fn find_potential_nested_roots(root: &Path, depth: i64, is_dir_hint: Option<bool>) -> Vec<PathBuf> {
    let is_dir = match is_dir_hint {
        Some(d) => d,
        None => match std::fs::metadata(root) {
            Ok(m) => m.is_dir(),
            Err(e) => {
                tracing::debug!("cross-seed: Failed to process path {}: {e}", root.display());
                return Vec::new();
            }
        },
    };
    if depth <= 0 || should_ignore_path_heuristically(root, is_dir) {
        return Vec::new();
    }
    if !is_dir {
        return vec![root.to_path_buf()];
    }
    let children = match read_dir_sorted(root) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("cross-seed: Failed to process path {}: {e}", root.display());
            return Vec::new();
        }
    };
    let mut out: Vec<PathBuf> = children.into_iter().flat_map(|(p, d)| find_potential_nested_roots(&p, depth - 1, Some(d))).collect();
    out.push(root.to_path_buf());
    out
}

/// `findSearcheesFromAllDataDirs`: the releases of every data folder.
pub fn find_searchees_from_all_data_dirs(cfg: &RuntimeConfig) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in &cfg.data_dirs {
        match read_dir_sorted(Path::new(dir)) {
            Ok(entries) => {
                for (p, _) in entries {
                    out.extend(find_potential_nested_roots(&p, cfg.max_data_depth, None));
                }
            }
            Err(e) => tracing::error!("cross-seed: the data folder {dir} can't be read: {e}"),
        }
    }
    out
}

/// What a release's files looked like when it was indexed: their count, sizes and newest change.
type Signature = (usize, u64, i128);

/// The signature of `root`'s files (`None` when it is gone).
fn signature(root: &Path) -> Option<Signature> {
    fn walk(p: &Path, sig: &mut Signature, depth: usize) -> std::io::Result<()> {
        let m = std::fs::metadata(p)?;
        let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i128).unwrap_or(0);
        sig.2 = sig.2.max(mtime);
        if m.is_dir() {
            if depth > 64 {
                return Ok(());
            }
            for e in std::fs::read_dir(p)?.filter_map(|e| e.ok()) {
                walk(&e.path(), sig, depth + 1)?;
            }
        } else {
            sig.0 += 1;
            sig.1 += m.len();
        }
        Ok(())
    }
    let mut sig = (0, 0, 0);
    walk(root, &mut sig, 0).ok()?;
    Some(sig)
}

/// The data folders' state between indexings (cross-seed's watchers and `modifiedPaths`).
#[derive(Debug, Default)]
pub struct DataIndex {
    /// Each release's signature when it was last indexed.
    seen: HashMap<PathBuf, Signature>,
    /// When the folders were last looked over.
    last: Option<Instant>,
}

/// The shortest time between two looks over the data folders.
const RESCAN_EVERY: Duration = Duration::from_secs(60);

impl DataIndex {
    /// `indexDataDirs`: at `startup` every release is indexed; afterwards only the new and changed ones, and the
    /// gone ones are forgotten (at most once a minute).
    pub fn index(&mut self, db: &SqliteDb, cfg: &RuntimeConfig, startup: bool) {
        if cfg.data_dirs.is_empty() {
            return;
        }
        if !startup && self.last.is_some_and(|t| t.elapsed() < RESCAN_EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        if startup {
            tracing::info!("cross-seed: Indexing dataDirs for reverse lookup...");
        }
        let roots = find_searchees_from_all_data_dirs(cfg);
        let mut changed = Vec::new();
        let mut now: HashMap<PathBuf, Signature> = HashMap::new();
        for r in &roots {
            let Some(sig) = signature(r) else { continue };
            if startup || self.seen.get(r) != Some(&sig) {
                changed.push(r.clone());
            }
            now.insert(r.clone(), sig);
        }
        if !startup {
            let gone: Vec<String> = self.seen.keys().filter(|p| !now.contains_key(*p)).map(|p| p.to_string_lossy().into_owned()).collect();
            forget_paths(db, &gone);
            if !changed.is_empty() {
                tracing::debug!("cross-seed: Indexing dataDirs due to recent changes...");
            }
        }
        let n = index_data_paths(db, cfg, &changed);
        if startup {
            tracing::info!("cross-seed: Validated {n} entries from dataDirs...");
        }
        self.seen = now;
    }
}

/// A kept data row.
#[derive(Debug, Clone)]
struct DataEntry {
    /// Its title.
    title: String,
    /// Its path.
    path: String,
}

/// Every kept data row.
fn all_data(db: &SqliteDb) -> Vec<DataEntry> {
    db.query("SELECT path, title FROM cross_seed_data ORDER BY rowid", p![])
        .map(|rows| rows.iter().map(|r| DataEntry { title: r.s("title"), path: r.s("path") }).collect())
        .unwrap_or_default()
}

/// `getDataByFuzzyName`: the data whose title is within a third of the name's length in edits of `stem` (first
/// among those with the same key title, when any has it); rows whose files are gone are forgotten.
pub fn get_data_by_fuzzy_name(db: &SqliteDb, stem: &str) -> Vec<Searchee> {
    let all = all_data(db);
    let full = create_key_title(stem);
    let mut filtered: Vec<DataEntry> = Vec::new();
    if let Some(full) = &full {
        filtered = all.iter().filter(|d| create_key_title(&strip_extension(&d.title)).as_ref() == Some(full)).cloned().collect();
    }
    if filtered.is_empty() {
        filtered = all;
    }
    let len = |s: &str| s.encode_utf16().count();
    let candidate_max = len(stem) / LEVENSHTEIN_DIVISOR;
    let mut gone = Vec::new();
    let mut matches = Vec::new();
    for d in filtered {
        let title = strip_extension(&d.title);
        let max = candidate_max.min(len(&title) / LEVENSHTEIN_DIVISOR);
        if distance(stem, &title) > max {
            continue;
        }
        if Path::new(&d.path).exists() {
            matches.push(d);
        } else {
            gone.push(d.path);
        }
    }
    forget_paths(db, &gone);
    matches.iter().filter_map(|d| create_searchee_from_path(Path::new(&d.path)).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lantern-xs-data-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, n: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![0u8; n]).unwrap();
    }

    #[test]
    fn nested_roots_as_cross_seed_finds_them() {
        let d = tmp("roots");
        write(&d.join("Show.S01.1080p-GRP/Show.S01E01.mkv"), 10);
        write(&d.join("Show.S01.1080p-GRP/Sample/sample.mkv"), 1);
        write(&d.join("Show.S01.1080p-GRP/info.nfo"), 1);
        write(&d.join("Movie.2020.1080p-GRP.mkv"), 10);
        write(&d.join("notes.txt"), 1);
        let cfg = RuntimeConfig { data_dirs: vec![d.to_string_lossy().into_owned()], max_data_depth: 2, ..RuntimeConfig::default() };
        let found: Vec<String> =
            find_searchees_from_all_data_dirs(&cfg).iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().into_owned()).collect();
        // deepest first; the sample folder, the .nfo and the .txt are left out
        assert_eq!(found, vec!["Movie.2020.1080p-GRP.mkv", "Show.S01.1080p-GRP/Show.S01E01.mkv", "Show.S01.1080p-GRP"]);
        let cfg1 = RuntimeConfig { max_data_depth: 1, ..cfg };
        assert_eq!(find_searchees_from_all_data_dirs(&cfg1).len(), 2, "depth 1: only the top entries");
        assert!(should_ignore_path_heuristically(Path::new("/x/BDMV"), true));
        assert!(!should_ignore_path_heuristically(Path::new("/x/Season 1"), true));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ensemble_pieces_of_an_episode() {
        let files = vec![
            File { name: "a.nfo".into(), path: "Show.S01E05/a.nfo".into(), length: 1 },
            File { name: "Show.S01E05.mkv".into(), path: "Show.S01E05/Show.S01E05.mkv".into(), length: 9 },
        ];
        let p = create_ensemble_pieces("The.Show.S01E05.1080p-GRP", &files).unwrap();
        assert_eq!(p[0].0, "theshow.S1");
        assert_eq!(p[0].1, "5");
        assert_eq!(p[0].2.name, "Show.S01E05.mkv");
        assert!(create_ensemble_pieces("Movie.2020.1080p", &files).is_none());
        let e = index_ensemble_data_entry("The.Show.S01E05.1080p-GRP", "/data/Show.S01E05", &files).unwrap();
        assert_eq!(e[0].path, "/data/Show.S01E05/Show.S01E05.mkv");
    }
}
