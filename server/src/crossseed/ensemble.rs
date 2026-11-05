// Ported from cross-seed v6.13.9 (src/searchee.ts: createEnsembleSearchees and its helpers), Apache-2.0.
//! Season packs from episodes (`seasonFromEpisodes`): the episodes you have of a season, as torrents or data, are
//! put together as one virtual searchee named like the season pack (`Show.S01 1080p-GRP`), so a season pack on
//! another site can be matched and linked from them. With filters (the search job) a season needs at least three
//! episodes, the share `seasonFromEpisodes` of its highest episode number, no season pack of its own, and files at
//! least 8 days old.

use super::config::RuntimeConfig;
use super::constants::{SEASON_REGEX, VIDEO_EXTENSIONS};
use super::jsre::test;
use super::searchee::{
    dirname, get_anime_keys, get_episode_keys, get_key_meta_info, get_largest_file, get_season_keys, File, Label, Searchee,
};
use super::utils::{has_ext, strip_extension};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::Path;

/// Searchees by key, then by episode, in the order first seen.
type KeyMap = IndexMap<String, IndexMap<String, Vec<Searchee>>>;

/// When a file was last written, in milliseconds since 1970 (`None` when it can't be read).
pub fn mtime_ms(path: &str) -> Option<f64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64() * 1000.0)
}

/// `getNewestFileAge`: the newest of the files' write times (0 when none can be read).
pub fn get_newest_file_age(paths: &[String]) -> f64 {
    paths.iter().filter_map(|p| mtime_ms(p)).fold(0.0, f64::max)
}

/// `getSearcheeNewestFileAge`: for data its own write time (a file) or its files'; for a virtual season its
/// files' (their paths are on disk).
pub fn get_searchee_newest_file_age(s: &Searchee) -> f64 {
    match &s.path {
        None => get_newest_file_age(&s.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()),
        Some(path) => {
            if std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
                return mtime_ms(path).unwrap_or(0.0);
            }
            let parent = dirname(path);
            get_newest_file_age(
                &s.files.iter().map(|f| Path::new(&parent).join(&f.path).to_string_lossy().into_owned()).collect::<Vec<_>>(),
            )
        }
    }
}

/// `parseEnsembleKeys`: files the searchee under each of its keys (not those with a season pack already).
fn parse_ensemble_keys(
    s: &Searchee,
    keys: &[String],
    ensemble_titles: &[String],
    episode: &str,
    existing: &HashMap<String, Vec<Searchee>>,
    key_map: &mut KeyMap,
    titles: &mut HashMap<String, String>,
) {
    for (i, key) in keys.iter().enumerate() {
        if existing.contains_key(key) {
            continue;
        }
        titles.entry(key.clone()).or_insert_with(|| ensemble_titles[i].clone());
        key_map.entry(key.clone()).or_default().entry(episode.to_string()).or_default().push(s.clone());
    }
}

/// `organizeEnsembleKeys`: the episodes by season key and episode, and each key's season title.
pub fn organize_ensemble_keys(all: &[Searchee], use_filters: bool) -> (KeyMap, HashMap<String, String>) {
    let mut existing: HashMap<String, Vec<Searchee>> = HashMap::new();
    if use_filters {
        for s in all {
            let stem = strip_extension(&s.title);
            let Some(sk) = get_season_keys(&stem) else { continue };
            let info = get_key_meta_info(&stem);
            let season = sk.season.clone().unwrap_or_default();
            for k in &sk.key_titles {
                existing.entry(format!("{k}.{season}{info}")).or_default().push(s.clone());
            }
        }
    }
    let mut key_map = KeyMap::new();
    let mut titles = HashMap::new();
    for s in all {
        let stem = strip_extension(&s.title);
        if let Some(ek) = get_episode_keys(&stem) {
            let info = get_key_meta_info(&stem);
            let keys: Vec<String> = ek
                .key_titles
                .iter()
                .map(|k| match &ek.season {
                    Some(season) => format!("{k}.{season}{info}"),
                    None => format!("{k}{info}"),
                })
                .collect();
            let ens: Vec<String> = ek.ensemble_titles.iter().map(|t| format!("{t}{info}")).collect();
            parse_ensemble_keys(s, &keys, &ens, &ek.element, &existing, &mut key_map, &mut titles);
            if use_filters {
                continue;
            }
        }
        if use_filters && test(&SEASON_REGEX, &stem) {
            continue;
        }
        if !has_ext(&s.files, VIDEO_EXTENSIONS) {
            continue;
        }
        if let Some(ak) = get_anime_keys(&stem) {
            let info = get_key_meta_info(&stem);
            let keys: Vec<String> = ak.key_titles.iter().map(|k| format!("{k}{info}")).collect();
            let ens: Vec<String> = ak.ensemble_titles.iter().map(|t| format!("{t}{info}")).collect();
            parse_ensemble_keys(s, &keys, &ens, &ak.element, &existing, &mut key_map, &mut titles);
        }
    }
    (key_map, titles)
}

/// `pushEnsembleEpisode`: adds an episode's largest file (at least half of it) when it is on disk; of two the
/// same size (a cross-seed of it), the older is kept.
fn push_ensemble_episode(
    s: &Searchee,
    episode_files: &mut Vec<File>,
    hosts: &mut IndexMap<String, usize>,
    save_paths: &HashMap<String, String>,
) {
    let save_path = match &s.path {
        Some(p) => Some(dirname(p)),
        None => s.save_path.clone().or_else(|| s.info_hash.as_ref().and_then(|h| save_paths.get(h).cloned())),
    };
    let Some(save_path) = save_path else { return };
    let Some(largest) = get_largest_file(&s.files) else { return };
    if (largest.length as f64) / (s.length as f64) < 0.5 {
        return;
    }
    let abs = File {
        length: largest.length,
        name: largest.name.clone(),
        path: Path::new(&save_path).join(&largest.path).to_string_lossy().into_owned(),
    };
    if !Path::new(&abs.path).exists() {
        return;
    }
    if let Some(i) = episode_files.iter().position(|f| f.length == abs.length) {
        let dupe_age = mtime_ms(&episode_files[i].path).unwrap_or(0.0);
        let new_age = mtime_ms(&abs.path).unwrap_or(0.0);
        if dupe_age <= new_age {
            return;
        }
        episode_files.remove(i);
    }
    episode_files.push(abs);
    if let Some(h) = &s.client_host {
        *hosts.entry(h.clone()).or_default() += 1;
    }
}

/// JavaScript's `Math.round`.
fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// Eight days in milliseconds: the youngest an episode may be to be part of a virtual season (with filters).
const MIN_AGE: f64 = 8.0 * 86_400_000.0;

/// `createVirtualSeasonSearchee`: one season from its episodes, or `None` (and why, with filters) when it can't
/// be made.
#[allow(clippy::too_many_arguments)]
pub fn create_virtual_season_searchee(
    key: &str,
    episodes: &IndexMap<String, Vec<Searchee>>,
    titles: &HashMap<String, String>,
    save_paths: &HashMap<String, String>,
    label: Label,
    use_filters: bool,
    cfg: &RuntimeConfig,
    priority: &dyn Fn(Option<&str>) -> usize,
    now: f64,
) -> Option<Searchee> {
    let sfe = cfg.season_from_episodes.unwrap_or(1.0);
    let min_episodes = 3;
    if use_filters && episodes.len() < min_episodes {
        return None;
    }
    let title = titles.get(key).cloned().unwrap_or_default();
    let numbers: Vec<i64> = episodes.keys().filter_map(|e| e.parse::<i64>().ok()).collect();
    if episodes.keys().next().is_some_and(|e| e.parse::<i64>().is_ok()) {
        let highest = numbers.iter().copied().max().unwrap_or(0) as f64;
        let avail = episodes.len() as f64 / highest;
        if use_filters && avail < sfe {
            tracing::debug!(
                "cross-seed: Skipping virtual searchee for {title} episodes as there's only {}/{highest} ({avail:.2} < {sfe:.2})",
                episodes.len()
            );
            return None;
        }
    }
    let mut season = Searchee { name: title.clone(), title: title.clone(), label, ..Searchee::default() };
    let mut newest = 0.0f64;
    let mut hosts: IndexMap<String, usize> = IndexMap::new();
    for searchees in episodes.values() {
        let mut files: Vec<File> = Vec::new();
        for s in searchees {
            push_ensemble_episode(s, &mut files, &mut hosts, save_paths);
        }
        if files.is_empty() {
            continue;
        }
        let total: i64 = files.iter().map(|f| f.length).sum();
        season.length += js_round(total as f64 / files.len() as f64);
        for f in &files {
            newest = newest.max(mtime_ms(&f.path).unwrap_or(0.0));
        }
        season.files.extend(files);
    }
    season.mtime_ms = Some(newest);
    let mut hosts: Vec<(String, usize)> = hosts.into_iter().collect();
    hosts.sort_by_key(|(h, n)| (std::cmp::Reverse(*n), priority(Some(h))));
    season.client_host = hosts.first().map(|(h, _)| h.clone());
    if season.files.len() < min_episodes {
        if use_filters {
            tracing::debug!(
                "cross-seed: Skipping virtual searchee for {title} episodes as only {} episode files were found (min: {min_episodes})",
                season.files.len()
            );
        }
        return None;
    }
    if use_filters && now - newest < MIN_AGE {
        tracing::debug!("cross-seed: Skipping virtual searchee for {title} episodes as some are below the minimum age of 8 days");
        return None;
    }
    Some(season)
}

/// `createEnsembleSearchees`: the virtual seasons made from `all` (none without `seasonFromEpisodes`).
/// `save_paths` holds the clients' folders by info hash for torrents that don't carry theirs.
pub fn create_ensemble_searchees(
    all: &[Searchee],
    use_filters: bool,
    cfg: &RuntimeConfig,
    save_paths: &HashMap<String, String>,
    priority: &dyn Fn(Option<&str>) -> usize,
    now: f64,
) -> Vec<Searchee> {
    if all.is_empty() || cfg.season_from_episodes.is_none() {
        return Vec::new();
    }
    let label = all[0].label;
    let (key_map, titles) = organize_ensemble_keys(all, use_filters);
    let out: Vec<Searchee> = key_map
        .iter()
        .filter_map(|(key, eps)| create_virtual_season_searchee(key, eps, &titles, save_paths, label, use_filters, cfg, priority, now))
        .collect();
    if use_filters {
        tracing::debug!("cross-seed: Created {} virtual season searchees...", out.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(dir: &Path, n: u32, size: usize) -> Searchee {
        let name = format!("Show.Name.S01E{n:02}.1080p.WEB-GRP.mkv");
        std::fs::write(dir.join(&name), vec![0u8; size]).unwrap();
        Searchee {
            info_hash: Some(format!("{n:040}")),
            name: name.clone(),
            title: name.clone(),
            files: vec![File { name: name.clone(), path: name.clone(), length: size as i64 }],
            length: size as i64,
            save_path: Some(dir.to_string_lossy().into_owned()),
            client_host: Some("qbit:8080".into()),
            ..Searchee::default()
        }
    }

    #[test]
    fn a_season_from_its_episodes() {
        let dir = std::env::temp_dir().join(format!("lantern-xs-ens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let eps: Vec<Searchee> = (1..=3).map(|n| episode(&dir, n, 100 + n as usize)).collect();
        let cfg = RuntimeConfig::default();
        let prio = |_: Option<&str>| 0usize;
        let now = crate::crossseed::now_ms();
        // without filters (inject): the season is made
        let made = create_ensemble_searchees(&eps, false, &cfg, &HashMap::new(), &prio, now);
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].title, "Show.Name.S1.1080p-grp");
        assert_eq!(made[0].files.len(), 3);
        assert_eq!(made[0].length, 101 + 102 + 103);
        assert_eq!(made[0].client_host.as_deref(), Some("qbit:8080"));
        assert!(made[0].is_virtual());
        // with filters (search): the files are too new
        assert!(create_ensemble_searchees(&eps, true, &cfg, &HashMap::new(), &prio, now).is_empty());
        assert_eq!(create_ensemble_searchees(&eps, true, &cfg, &HashMap::new(), &prio, now + MIN_AGE + 60_000.0).len(), 1);
        // two of three episodes: below the share of seasonFromEpisodes (and below three)
        assert!(create_ensemble_searchees(&eps[..2], true, &cfg, &HashMap::new(), &prio, now + MIN_AGE * 2.0).is_empty());
        // a season pack already there: not made with filters
        let mut with_pack = eps.clone();
        with_pack.push(Searchee {
            title: "Show.Name.S01.1080p.WEB-GRP".into(),
            name: "Show.Name.S01.1080p.WEB-GRP".into(),
            ..eps[0].clone()
        });
        assert!(create_ensemble_searchees(&with_pack, true, &cfg, &HashMap::new(), &prio, now + MIN_AGE * 2.0).is_empty());
        let off = RuntimeConfig { season_from_episodes: None, ..RuntimeConfig::default() };
        assert!(create_ensemble_searchees(&eps, false, &off, &HashMap::new(), &prio, now).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
