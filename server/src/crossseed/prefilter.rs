// Ported from cross-seed v6.13.9 (src/preFilter.ts), Apache-2.0.
//! What is left out before searching: the block list, season pack episodes, single episodes, mostly non-video
//! releases, cross-seeds themselves, folders Sonarr and Radarr made, Specials, odd episode names
//! ([`filter_by_content`]); searchees searched too long ago or too recently ([`filter_timestamps`]); and copies of
//! the same searchee ([`filter_dupes_from_similar`]).

use super::config::{ms_long, RuntimeConfig};
use super::constants::*;
use super::jsre::{group, test};
use super::searchee::{basename, dirname, get_media_type, Label, Searchee};
use super::utils::*;

/// Why a searchee was left out, in cross-seed's words (`logFilterReason`'s reason).
pub type Reason = String;

/// `isSingleEpisode`: an episode, or anime with one video file.
pub fn is_single_episode(s: &Searchee, media_type: MediaType) -> bool {
    match media_type {
        MediaType::Episode => true,
        MediaType::Anime => files_with_ext(&s.files, VIDEO_EXTENSIONS).len() == 1,
        _ => false,
    }
}

/// `isCrossSeed`: whether the searchee is itself a cross-seed (by its category or tags).
pub fn is_cross_seed(s: &Searchee, cfg: &RuntimeConfig) -> bool {
    let cat = s.category.as_deref();
    if let (Some(link), Some(c)) = (cfg.link_category.as_deref(), cat) {
        if !link.is_empty() && c == link {
            return true;
        }
    }
    if cat == Some(TORRENT_TAG) || cat.is_some_and(|c| c.ends_with(TORRENT_CATEGORY_SUFFIX)) {
        return true;
    }
    let tags = s.tags.as_deref().unwrap_or(&[]);
    tags.iter().any(|t| t == TORRENT_TAG || t.ends_with(TORRENT_CATEGORY_SUFFIX))
}

/// `filterByContent`'s options.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContentOptions {
    /// `allowSeasonPackEpisodes`: an episode inside a season folder may be searched (with
    /// `includeSingleEpisodes`).
    pub allow_season_pack_episodes: bool,
    /// `ignoreCrossSeeds`: leave out what is itself a cross-seed.
    pub ignore_cross_seeds: bool,
    /// `blockListOnly`: only the block list.
    pub block_list_only: bool,
}

/// `filterByContent`: `Ok(())` when the searchee may be searched, else why not.
pub fn filter_by_content(s: &Searchee, cfg: &RuntimeConfig, opts: ContentOptions) -> Result<(), Reason> {
    let media_type = get_media_type(s);
    if let Some(blocked) = find_blocked_string_in_release_maybe(s, &cfg.block_list) {
        return Err(format!("it matches the blocklist: {blocked}"));
    }
    if opts.block_list_only {
        return Ok(());
    }
    let search_or_webhook = matches!(s.label, Label::Search | Label::Webhook);
    if let Some(path) = &s.path {
        let parent = basename(&dirname(path));
        if (!opts.allow_season_pack_episodes || !cfg.include_single_episodes)
            && s.files.len() == 1
            && search_or_webhook
            && (test(&SEASON_REGEX, &parent) || test(&SONARR_SUBFOLDERS_REGEX, &parent))
        {
            return Err("it is a season pack episode".into());
        }
    }
    if !cfg.include_single_episodes && s.label != Label::Announce && is_single_episode(s, media_type) {
        return Err("it is a single episode".into());
    }
    let mut disc_and_video: Vec<&str> = VIDEO_EXTENSIONS.to_vec();
    disc_and_video.extend_from_slice(VIDEO_DISC_EXTENSIONS);
    let non_video: i64 = s.files.iter().filter(|f| !has_ext(std::slice::from_ref(*f), &disc_and_video)).map(|f| f.length).sum();
    let non_video_size_ratio = non_video as f64 / s.length as f64;
    if !cfg.include_non_videos && non_video_size_ratio > cfg.fuzzy_size_threshold {
        return Err(format!("nonVideoSizeRatio {non_video_size_ratio:.3} > {} fuzzySizeThreshold", js_number(cfg.fuzzy_size_threshold)));
    }
    if opts.ignore_cross_seeds && is_cross_seed(s, cfg) {
        return Err("it is a cross seed".into());
    }
    if let Some(path) = &s.path {
        let is_dir = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
        let base = basename(path);
        if is_dir
            && test(&ARR_DIR_REGEX, &base)
            && non_video_size_ratio < 0.02
            && !matches!(media_type, MediaType::Episode | MediaType::Season)
            && !(s.files.len() > 1 && test(&SONARR_SUBFOLDERS_REGEX, &base))
        {
            return Err("it looks like an arr movie/series directory".into());
        }
        if media_type == MediaType::Season && group(&SEASON_REGEX, &s.title, "season").map(|g| extract_int(&g)) == Some(0) {
            return Err("it is a Specials folder".into());
        }
    }
    if search_or_webhook && (test(&BAD_EP_REGEX, &s.title) || test(&BAD_SEASON_REGEX, &s.title)) {
        return Err("it has a non-standard episode/season naming format".into());
    }
    Ok(())
}

/// The block list entry `value` as a JavaScript `new RegExp(value)`; `None` when it isn't one.
fn block_regex(value: &str) -> Option<fancy_regex::Regex> {
    fancy_regex::Regex::new(&super::jsre::translate(value)).ok()
}

/// `findBlockedStringInReleaseMaybe`: the first block list entry the searchee (or torrent) matches.
pub fn find_blocked_string_in_release_maybe(s: &Searchee, block_list: &[String]) -> Option<String> {
    block_list
        .iter()
        .find(|entry| {
            let (kind, value) = parse_blocklist_entry(entry);
            let folder = s.path.as_deref().map(dirname);
            match kind {
                BlocklistType::Name => s.title.contains(&value),
                BlocklistType::NameRegex => block_regex(&value).is_some_and(|r| r.is_match(&s.title).unwrap_or(false)),
                BlocklistType::Folder => folder.is_some_and(|f| f.contains(&value)),
                BlocklistType::FolderRegex => folder.is_some_and(|f| block_regex(&value).is_some_and(|r| r.is_match(&f).unwrap_or(false))),
                BlocklistType::Category => s.category.as_deref() == Some(value.as_str()),
                BlocklistType::Tag => match &s.tags {
                    // data and fetched torrents have no tags to look at
                    None => false,
                    Some(tags) if value.is_empty() => tags.is_empty(),
                    Some(tags) => tags.contains(&value),
                },
                BlocklistType::Tracker => s.trackers.as_ref().is_some_and(|t| t.contains(&value)),
                BlocklistType::InfoHash => s.info_hash.as_deref() == Some(value.as_str()),
                BlocklistType::SizeBelow => (s.length as f64) < js_parse_int(&value),
                BlocklistType::SizeAbove => (s.length as f64) > js_parse_int(&value),
                BlocklistType::Legacy => {
                    s.title.contains(entry.as_str())
                        || s.info_hash.as_deref() == Some(value.as_str())
                        || folder.is_some_and(|f| f.contains(entry.as_str()))
                }
                // cross-seed throws "Unknown blocklist type"; its settings check refuses such entries first
                BlocklistType::Unknown => false,
            }
        })
        .cloned()
}

/// JavaScript's `parseInt(s)`: the leading whole number, `NaN` (which compares false) when there is none.
fn js_parse_int(s: &str) -> f64 {
    let t = s.trim_start();
    let (sign, rest) = match t.strip_prefix('-') {
        Some(r) => (-1.0, r),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<f64>().map(|n| sign * n).unwrap_or(f64::NAN)
}

/// `filterDupesFromSimilar`: the searchees without copies of one another (same title, size, files and client;
/// torrents before data).
pub fn filter_dupes_from_similar(searchees: &[Searchee]) -> Vec<Searchee> {
    let mut sorted: Vec<&Searchee> = searchees.iter().collect();
    sorted.sort_by_key(|s| s.info_hash.is_none());
    let mut out: Vec<Searchee> = Vec::new();
    for s in sorted {
        let dupe = out.iter().any(|o| {
            if s.title != o.title || s.length != o.length || s.files.len() != o.files.len() || s.client_host != o.client_host {
                return false;
            }
            let mut potential: Vec<i64> = o.files.iter().map(|f| f.length).collect();
            s.files.iter().all(|f| match potential.iter().position(|l| *l == f.length) {
                Some(i) => {
                    potential.remove(i);
                    true
                }
                None => false,
            })
        });
        if !dupe {
            out.push(s.clone());
        }
    }
    out
}

/// `Number.MAX_SAFE_INTEGER`, cross-seed's "never searched" first-search time.
pub const MAX_INT: f64 = 9_007_199_254_740_991.0;

/// A searchee's search times over the indexers that can search its media (`filterTimestamps`'s query):
/// milliseconds since 1970.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timestamps {
    /// The earliest first search ([`MAX_INT`] for an indexer never searched).
    pub earliest_first_search: f64,
    /// The latest first search ([`MAX_INT`] when an indexer was never searched: a new indexer).
    pub latest_first_search: f64,
    /// The earliest last search (0 for never).
    pub earliest_last_search: f64,
}

/// `filterTimestamps`: `Ok(())` when the searchee is due a search (`now` in milliseconds), else why not.
/// `newest_file_age` is asked only for a season made of episodes with `seasonFromEpisodes`.
pub fn filter_timestamps(
    s: &Searchee,
    cfg: &RuntimeConfig,
    t: Timestamps,
    now: f64,
    newest_file_age: impl FnOnce() -> f64,
) -> Result<(), Reason> {
    if cfg.season_from_episodes.is_some_and(|v| v != 0.0) && s.is_virtual() && t.earliest_last_search < newest_file_age() {
        return Ok(());
    }
    let skip_before = cfg.exclude_older.filter(|v| *v != 0.0).map(|v| now - v).unwrap_or(f64::NEG_INFINITY);
    // a new indexer (never searched) lets it through
    if t.latest_first_search != MAX_INT && t.earliest_first_search != 0.0 && t.earliest_first_search < skip_before {
        return Err(format!(
            "its first search timestamp {} is older than {} ago",
            human_readable_date(t.earliest_first_search),
            ms_long(cfg.exclude_older.unwrap_or(0.0))
        ));
    }
    let skip_after = cfg.exclude_recent_search.filter(|v| *v != 0.0).map(|v| now - v).unwrap_or(f64::INFINITY);
    if t.earliest_last_search != 0.0 && t.earliest_last_search > skip_after {
        return Err(format!(
            "its last search timestamp {} is newer than {} ago",
            human_readable_date(t.earliest_last_search),
            ms_long(cfg.exclude_recent_search.unwrap_or(0.0))
        ));
    }
    Ok(())
}

/// `humanReadableDate`: `2026-11-05 14:03:00` (Swedish locale, the server's time zone; here UTC).
pub fn human_readable_date(ms: f64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64).map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crossseed::searchee::File;

    fn file(path: &str, length: i64) -> File {
        File { name: basename(path), path: path.into(), length }
    }

    fn torrent(title: &str, files: Vec<File>) -> Searchee {
        let length = files.iter().map(|f| f.length).sum();
        Searchee {
            info_hash: Some("a".repeat(40)),
            name: title.into(),
            title: title.into(),
            files,
            length,
            tags: Some(vec![]),
            label: Label::Search,
            ..Searchee::default()
        }
    }

    #[test]
    fn content_rules() {
        let cfg = RuntimeConfig::default();
        let o = ContentOptions { ignore_cross_seeds: true, ..ContentOptions::default() };
        let pack = torrent("Show.S01.1080p-GRP", vec![file("Show.S01.1080p-GRP/e1.mkv", 100), file("Show.S01.1080p-GRP/e2.mkv", 100)]);
        assert_eq!(filter_by_content(&pack, &cfg, o), Ok(()));
        let ep = torrent("Show.S01E01.1080p-GRP", vec![file("Show.S01E01.1080p-GRP.mkv", 100)]);
        assert_eq!(filter_by_content(&ep, &cfg, o).unwrap_err(), "it is a single episode");
        let announce = Searchee { label: Label::Announce, ..ep.clone() };
        assert_eq!(filter_by_content(&announce, &cfg, o), Ok(()), "announces match single episodes");
        let with_eps = cfg.with(&super::super::config::Override { include_single_episodes: Some(true), ..Default::default() });
        assert_eq!(filter_by_content(&ep, &with_eps, o), Ok(()));
        let music = torrent("Band - Album (2020) [FLAC]", vec![file("a/01.flac", 100), file("a/cover.jpg", 1)]);
        assert_eq!(filter_by_content(&music, &cfg, o).unwrap_err(), "nonVideoSizeRatio 1.000 > 0.02 fuzzySizeThreshold");
        let with_subs = torrent("Movie.2020.1080p-GRP", vec![file("m/m.mkv", 1000), file("m/m.srt", 10)]);
        assert_eq!(filter_by_content(&with_subs, &cfg, o), Ok(()), "a little non-video is fine");
        let xs = Searchee { category: Some("tv.cross-seed".into()), ..pack.clone() };
        assert_eq!(filter_by_content(&xs, &cfg, o).unwrap_err(), "it is a cross seed");
        assert_eq!(filter_by_content(&xs, &cfg, ContentOptions::default()), Ok(()));
        let tagged = Searchee { tags: Some(vec!["cross-seed".into()]), ..pack.clone() };
        assert!(is_cross_seed(&tagged, &cfg));
        let linked = Searchee { category: Some("cross-seed-link".into()), ..pack.clone() };
        assert!(is_cross_seed(&linked, &cfg));
        let bad = torrent("S01E01.1080p-GRP", vec![file("S01E01.1080p-GRP/e.mkv", 100), file("S01E01.1080p-GRP/f.mkv", 100)]);
        let c = cfg.with(&super::super::config::Override { include_single_episodes: Some(true), ..Default::default() });
        assert_eq!(filter_by_content(&bad, &c, o).unwrap_err(), "it has a non-standard episode/season naming format");
        let blocked = cfg.with(&super::super::config::Override { block_list: Some(vec!["name:S01".into()]), ..Default::default() });
        assert_eq!(filter_by_content(&pack, &blocked, o).unwrap_err(), "it matches the blocklist: name:S01");
    }

    #[test]
    fn season_pack_episodes_and_specials_from_data() {
        let cfg = RuntimeConfig::default();
        let dir = std::env::temp_dir().join(format!("lantern-xs-prefilter-{}", std::process::id()));
        let season = dir.join("Show.S00.1080p-GRP");
        std::fs::create_dir_all(&season).unwrap();
        let ep = Searchee {
            path: Some(season.join("Show.S00E01.mkv").to_string_lossy().into_owned()),
            files: vec![file("Show.S00E01.mkv", 100)],
            title: "Show.S00E01".into(),
            name: "Show.S00E01.mkv".into(),
            length: 100,
            label: Label::Webhook,
            ..Searchee::default()
        };
        assert_eq!(filter_by_content(&ep, &cfg, ContentOptions::default()).unwrap_err(), "it is a season pack episode");
        let specials = Searchee {
            path: Some(season.to_string_lossy().into_owned()),
            files: vec![file("Show.S00.1080p-GRP/a.mkv", 100), file("Show.S00.1080p-GRP/b.mkv", 100)],
            title: "Show.S00.1080p-GRP".into(),
            name: "Show.S00.1080p-GRP".into(),
            length: 200,
            label: Label::Search,
            ..Searchee::default()
        };
        assert_eq!(filter_by_content(&specials, &cfg, ContentOptions::default()).unwrap_err(), "it is a Specials folder");
        let movie = dir.join("Some Movie (2020)");
        std::fs::create_dir_all(&movie).unwrap();
        let arr = Searchee {
            path: Some(movie.to_string_lossy().into_owned()),
            files: vec![file("Some Movie (2020)/Some.Movie.2020.1080p.mkv", 100)],
            title: "Some Movie (2020)".into(),
            name: "Some Movie (2020)".into(),
            length: 100,
            label: Label::Search,
            ..Searchee::default()
        };
        assert_eq!(filter_by_content(&arr, &cfg, ContentOptions::default()).unwrap_err(), "it looks like an arr movie/series directory");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_block_list_type() {
        let s = Searchee {
            info_hash: Some("b".repeat(40)),
            path: Some("/data/movies/Movie.2020/Movie.2020.mkv".into()),
            title: "Movie.2020.1080p.x264-GRP".into(),
            category: Some("movies".into()),
            tags: Some(vec!["keep".into()]),
            trackers: Some(vec!["tracker.example:8080".into()]),
            length: 1000,
            ..Searchee::default()
        };
        let hit = |e: &str| find_blocked_string_in_release_maybe(&s, &[e.to_string()]).is_some();
        assert!(hit("name:x264") && !hit("name:x265"));
        assert!(hit("nameRegex:\\d{4}\\.1080p") && !hit("nameRegex:^2160"));
        assert!(hit("folder:movies") && !hit("folder:tv"));
        assert!(hit("folderRegex:Movie\\.\\d+$") && !hit("folderRegex:^/tv"));
        assert!(hit("category:movies") && !hit("category:tv") && !hit("category:"));
        assert!(hit("tag:keep") && !hit("tag:other") && !hit("tag:"));
        assert!(hit("tracker:tracker.example:8080") && !hit("tracker:tracker.example"));
        assert!(hit(&format!("infoHash:{}", "b".repeat(40))) && !hit(&format!("infoHash:{}", "c".repeat(40))));
        assert!(hit("sizeBelow:1001") && !hit("sizeBelow:1000"));
        assert!(hit("sizeAbove:999") && !hit("sizeAbove:1000"));
        assert!(hit("x264") && hit(&"b".repeat(40)) && hit("movies/Movie") && !hit("tv"));
        let untagged = Searchee { tags: Some(vec![]), ..s.clone() };
        assert!(find_blocked_string_in_release_maybe(&untagged, &["tag:".into()]).is_some());
        let data = Searchee { tags: None, ..s.clone() };
        assert!(find_blocked_string_in_release_maybe(&data, &["tag:".into()]).is_none());
        assert_eq!(find_blocked_string_in_release_maybe(&s, &["name:x".into(), "name:x264".into()]).as_deref(), Some("name:x"));
    }

    #[test]
    fn dupes_from_similar() {
        let a = torrent("Show.S01", vec![file("a", 1), file("b", 2)]);
        let data = Searchee { info_hash: None, path: Some("/d/Show.S01".into()), ..a.clone() };
        let other_client = Searchee { client_host: Some("other".into()), ..a.clone() };
        let out = filter_dupes_from_similar(&[data.clone(), a.clone(), other_client.clone()]);
        assert_eq!(out, vec![a, other_client], "torrents first, then copies left out");
    }

    #[test]
    fn timestamps() {
        let day = 86_400_000.0;
        let now = 100.0 * day;
        let cfg = RuntimeConfig::default();
        let s = torrent("Show.S01", vec![file("a", 1)]);
        let never = Timestamps { earliest_first_search: MAX_INT, latest_first_search: MAX_INT, earliest_last_search: 0.0 };
        assert_eq!(filter_timestamps(&s, &cfg, never, now, || 0.0), Ok(()));
        let old = Timestamps {
            earliest_first_search: now - 20.0 * day,
            latest_first_search: now - 20.0 * day,
            earliest_last_search: now - 10.0 * day,
        };
        assert!(filter_timestamps(&s, &cfg, old, now, || 0.0).unwrap_err().ends_with("is older than 14 days ago"));
        let new_indexer = Timestamps { latest_first_search: MAX_INT, ..old };
        assert_eq!(filter_timestamps(&s, &cfg, new_indexer, now, || 0.0), Ok(()));
        let recent =
            Timestamps { earliest_first_search: now - 5.0 * day, latest_first_search: now - 5.0 * day, earliest_last_search: now - day };
        assert!(filter_timestamps(&s, &cfg, recent, now, || 0.0).unwrap_err().ends_with("is newer than 3 days ago"));
        let ignore = cfg.with(&super::super::config::Override { exclude_recent_search: Some(1.0), ..Default::default() });
        assert_eq!(filter_timestamps(&s, &ignore, recent, now, || 0.0), Ok(()));
        let virt = Searchee { info_hash: None, ..s.clone() };
        assert_eq!(filter_timestamps(&virt, &cfg, recent, now, || now), Ok(()), "a season with a newer episode is searched");
        assert_eq!(human_readable_date(0.0), "1970-01-01 00:00:00");
    }
}
