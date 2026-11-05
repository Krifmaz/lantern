// Ported from cross-seed v6.13.9 (src/decide.ts), Apache-2.0.
//! Deciding whether a candidate (a release another site lists) is the same as a searchee (what we have):
//! first from its name and listed size ([`pre_download`]: release group, resolution, source, proper/repack, fuzzy
//! size, download link), then the block list, then from its `.torrent` ([`assess_metafile`]: same info hash,
//! already in the client, block list, single episodes against a season, then the file trees as the match mode
//! allows). [`assess_candidate`] runs them in cross-seed's order with a fetch in between; [`reason`] says why, as
//! cross-seed's log does.

use super::config::RuntimeConfig;
use super::constants::*;
use super::jsre::{group, test};
use super::prefilter::{find_blocked_string_in_release_maybe, is_single_episode};
use super::searchee::{get_fuzzy_size_factor, get_min_size_ratio, get_release_group, media_type_of, File, Searchee};
use super::torrent::Metafile;
use super::utils::{extract_int, strip_extension};
use std::collections::HashSet;
use std::future::Future;

/// `Candidate`: a release another site lists.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Candidate {
    /// Its name.
    pub name: String,
    /// The site's id for it.
    pub guid: String,
    /// Where its `.torrent` is fetched from.
    pub link: String,
    /// The site's name.
    pub tracker: String,
    /// Its size as the site says (corrected to the `.torrent`'s once fetched).
    pub size: Option<i64>,
    /// When it was published (milliseconds since 1970).
    pub pub_date: Option<f64>,
    /// The Lantern indexer it came from.
    pub indexer_id: Option<i64>,
    /// A cookie to fetch it with (from an announce).
    pub cookie: Option<String>,
}

/// `SnatchError`: why a `.torrent` couldn't be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnatchError {
    /// The link is a magnet link.
    MagnetLink,
    /// The site's rate limit was reached.
    RateLimited,
    /// Anything else (the words say what).
    Failed(String),
}

/// `ResultAssessment`: a decision, and the `.torrent` when it was fetched.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    /// The decision.
    pub decision: Decision,
    /// The candidate's `.torrent`.
    pub metafile: Option<Metafile>,
    /// Whether the `.torrent` is kept (cross-seed's torrent cache).
    pub meta_cached: bool,
}

impl Assessment {
    fn only(decision: Decision) -> Assessment {
        Assessment { decision, metafile: None, meta_cached: false }
    }
}

/// `compareFileTrees`: every candidate file is in the searchee with the same path and size (the same name and
/// size for a season made of episodes, whose paths are on disk).
pub fn compare_file_trees(candidate: &Metafile, searchee: &Searchee) -> bool {
    let by_name = searchee.is_virtual();
    candidate
        .files
        .iter()
        .all(|a| searchee.files.iter().any(|b| b.length == a.length && if by_name { b.name == a.name } else { b.path == a.path }))
}

/// The first searchee file of `candidate`'s size still free (by name when several have the size).
fn take_by_size(available: &mut Vec<&File>, candidate: &File) -> bool {
    let mut same: Vec<usize> = (0..available.len()).filter(|&i| available[i].length == candidate.length).collect();
    if same.len() > 1 {
        same.retain(|&i| available[i].name == candidate.name);
    }
    match same.first() {
        Some(&i) => {
            available.remove(i);
            true
        }
        None => false,
    }
}

/// `compareFileTreesIgnoringNames`: every candidate file has a searchee file of its size (each used once).
pub fn compare_file_trees_ignoring_names(candidate: &Metafile, searchee: &Searchee) -> bool {
    let mut available: Vec<&File> = searchee.files.iter().collect();
    candidate.files.iter().all(|f| take_by_size(&mut available, f))
}

/// `getPartialSizeRatio`: the share of the candidate's bytes in files whose size the searchee has.
pub fn get_partial_size_ratio(candidate: &Metafile, searchee: &Searchee) -> f64 {
    let matched: i64 = candidate.files.iter().filter(|c| searchee.files.iter().any(|s| s.length == c.length)).map(|c| c.length).sum();
    matched as f64 / candidate.length as f64
}

/// `compareFileTreesPartial`: enough whole pieces of the candidate are on disk (at least the minimum size ratio).
pub fn compare_file_trees_partial(candidate: &Metafile, searchee: &Searchee, cfg: &RuntimeConfig) -> bool {
    let mut matched: i64 = 0;
    let mut available: Vec<&File> = searchee.files.iter().collect();
    for f in &candidate.files {
        if take_by_size(&mut available, f) {
            matched += f.length;
        }
    }
    let piece = candidate.piece_length as f64;
    let total_pieces = (candidate.length as f64 / piece).ceil();
    let available_pieces = (matched as f64 / piece).floor();
    available_pieces / total_pieces >= get_min_size_ratio(searchee, cfg)
}

/// `fuzzySizeDoesMatch`: the size is within the fuzzy size factor of the searchee's.
pub fn fuzzy_size_does_match(size: i64, searchee: &Searchee, cfg: &RuntimeConfig) -> bool {
    let factor = get_fuzzy_size_factor(searchee, cfg);
    let length = searchee.length as f64;
    let (lower, upper) = (length - factor * length, length + factor * length);
    (size as f64) >= lower && (size as f64) <= upper
}

/// The strict resolution (`1080p`) in a name, as found.
fn strict_res(name: &str) -> Option<String> {
    group(&RES_STRICT_REGEX, name, "res")
}

/// `resolutionDoesMatch`: the same 2160/1080/720 resolution, or one of them has none.
pub fn resolution_does_match(searchee_title: &str, candidate_name: &str) -> bool {
    let a = strict_res(searchee_title).map(|r| r.trim().to_lowercase());
    let b = strict_res(candidate_name).map(|r| r.trim().to_lowercase());
    match (a, b) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => extract_int(&a) == extract_int(&b),
        _ => true,
    }
}

/// `releaseGroupDoesMatch`: the same group (one starting with the other), or one of them has none; anime groups
/// in brackets count too.
pub fn release_group_does_match(searchee_title: &str, candidate_name: &str) -> bool {
    let a = get_release_group(&strip_extension(searchee_title)).map(|g| g.to_lowercase()).filter(|g| !g.is_empty());
    let b = get_release_group(&strip_extension(candidate_name)).map(|g| g.to_lowercase()).filter(|g| !g.is_empty());
    let (Some(a), Some(b)) = (a, b) else { return true };
    if a.starts_with(&b) || b.starts_with(&a) {
        return true;
    }
    let anime = |t: &str| group(&ANIME_GROUP_REGEX, t, "group").map(|g| g.trim().to_lowercase()).filter(|g| !g.is_empty());
    let (sa, ca) = (anime(searchee_title), anime(candidate_name));
    if sa.is_none() && ca.is_none() {
        return false;
    }
    if sa == ca {
        return true;
    }
    if sa.as_deref() == Some(b.as_str()) {
        return true;
    }
    if ca.is_some() && ca.as_deref() == Some(a.as_str()) {
        return true;
    }
    false
}

/// `sourceDoesMatch`: the same streaming source, or one of them has none.
pub fn source_does_match(searchee_title: &str, candidate_name: &str) -> bool {
    match (parse_source(searchee_title), parse_source(candidate_name)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// `releaseVersionDoesMatch`: both or neither are a proper or repack.
pub fn release_version_does_match(searchee_name: &str, candidate_name: &str) -> bool {
    test(&REPACK_PROPER_REGEX, searchee_name) == test(&REPACK_PROPER_REGEX, candidate_name)
}

/// The checks before fetching a candidate's `.torrent`, in cross-seed's order: `Some(decision)` when it fails one.
pub fn pre_download(candidate: &Candidate, searchee: &Searchee, cfg: &RuntimeConfig) -> Option<Decision> {
    let name = &candidate.name;
    if !release_group_does_match(&searchee.title, name) {
        return Some(Decision::ReleaseGroupMismatch);
    }
    if !resolution_does_match(&searchee.title, name) {
        return Some(Decision::ResolutionMismatch);
    }
    if !source_does_match(&searchee.title, name) {
        return Some(Decision::SourceMismatch);
    }
    if !release_version_does_match(&searchee.title, name) {
        return Some(Decision::ProperRepackMismatch);
    }
    if let Some(size) = candidate.size.filter(|s| *s != 0) {
        if !fuzzy_size_does_match(size, searchee, cfg) {
            return Some(Decision::FuzzySizeMismatch);
        }
    }
    if candidate.link.is_empty() {
        return Some(Decision::NoDownloadLink);
    }
    None
}

/// The checks on a candidate's `.torrent`, in cross-seed's order (`assessCandidate` after the fetch).
pub fn assess_metafile(
    meta: &Metafile,
    searchee: &Searchee,
    info_hashes_to_exclude: &HashSet<String>,
    block_list: &[String],
    cfg: &RuntimeConfig,
) -> Decision {
    if searchee.info_hash.as_deref() == Some(meta.info_hash.as_str()) {
        return Decision::SameInfoHash;
    }
    if info_hashes_to_exclude.contains(&meta.info_hash) {
        return Decision::InfoHashAlreadyExists;
    }
    if find_blocked_string_in_release_maybe(&Searchee::from_metafile_raw(meta), block_list).is_some() {
        return Decision::BlockedRelease;
    }
    // a candidate episode doesn't match a season pack we have
    if !cfg.include_single_episodes && test(&SEASON_REGEX, &searchee.title) {
        let as_searchee = Searchee::from_metafile_raw(meta);
        if is_single_episode(&as_searchee, media_type_of(&meta.title, &meta.files)) {
            return Decision::FileTreeMismatch;
        }
    }
    if compare_file_trees(meta, searchee) {
        return Decision::Match;
    }
    let size_match = compare_file_trees_ignoring_names(meta, searchee);
    if size_match && cfg.match_mode != MatchMode::Strict {
        return Decision::MatchSizeOnly;
    }
    if cfg.match_mode == MatchMode::Partial {
        if get_partial_size_ratio(meta, searchee) < get_min_size_ratio(searchee, cfg) {
            return Decision::PartialSizeMismatch;
        }
        if compare_file_trees_partial(meta, searchee, cfg) {
            return Decision::MatchPartial;
        }
    } else if !size_match {
        return Decision::SizeMismatch;
    }
    Decision::FileTreeMismatch
}

/// What a candidate is: a release to fetch, or a `.torrent` already at hand (cross-seed's torrent cache).
#[derive(Debug, Clone)]
pub enum MetaOrCandidate {
    /// A `.torrent` already fetched: the checks on the name are skipped.
    Meta(Metafile),
    /// A release to check by name and then fetch.
    Candidate(Candidate),
}

/// `assessCandidate`: the decision on one candidate for one searchee. `snatch` fetches a candidate's `.torrent`
/// (cross-seed retries 4 times, a minute apart, five for announces; the caller does that).
pub async fn assess_candidate<F, Fut>(
    input: &mut MetaOrCandidate,
    searchee: &Searchee,
    info_hashes_to_exclude: &HashSet<String>,
    block_list: &[String],
    cfg: &RuntimeConfig,
    snatch: F,
) -> Assessment
where
    F: FnOnce(Candidate) -> Fut,
    Fut: Future<Output = Result<Metafile, SnatchError>>,
{
    if let MetaOrCandidate::Candidate(c) = input {
        if let Some(d) = pre_download(c, searchee, cfg) {
            return Assessment::only(d);
        }
    }
    let is_meta = matches!(input, MetaOrCandidate::Meta(_));
    if find_blocked_string_in_release_maybe(searchee, block_list).is_some() {
        let metafile = match input {
            MetaOrCandidate::Meta(m) => Some(m.clone()),
            _ => None,
        };
        return Assessment { decision: Decision::BlockedRelease, metafile, meta_cached: is_meta };
    }
    let meta = match input {
        MetaOrCandidate::Meta(m) => m.clone(),
        MetaOrCandidate::Candidate(c) => match snatch(c.clone()).await {
            Ok(m) => {
                // trackers can be wrong
                c.size = Some(m.length);
                m
            }
            Err(SnatchError::MagnetLink) => return Assessment::only(Decision::MagnetLink),
            Err(SnatchError::RateLimited) => return Assessment::only(Decision::RateLimited),
            Err(SnatchError::Failed(_)) => return Assessment::only(Decision::DownloadFailed),
        },
    };
    let decision = assess_metafile(&meta, searchee, info_hashes_to_exclude, block_list, cfg);
    Assessment { decision, metafile: Some(meta), meta_cached: true }
}

/// `logDecision`'s reason: why a candidate got its decision, in cross-seed's words.
pub fn reason(decision: Decision, searchee: &Searchee, candidate: &Candidate, meta: Option<&Metafile>, cfg: &RuntimeConfig) -> String {
    let or_undefined = |o: Option<String>| o.unwrap_or_else(|| "undefined".into());
    let or_null = |o: Option<String>| o.unwrap_or_else(|| "null".into());
    match decision {
        Decision::ReleaseGroupMismatch => format!(
            "it has a different release group: {} -> {}",
            or_null(get_release_group(&strip_extension(&searchee.title))),
            or_null(get_release_group(&strip_extension(&candidate.name)))
        ),
        Decision::ResolutionMismatch => format!(
            "its resolution does not match: {} -> {}",
            or_undefined(strict_res(&searchee.title)),
            or_undefined(strict_res(&candidate.name))
        ),
        Decision::SourceMismatch => format!(
            "it has a different source: {} -> {}",
            parse_source(&searchee.title).unwrap_or("null"),
            parse_source(&candidate.name).unwrap_or("null")
        ),
        Decision::ProperRepackMismatch => format!(
            "one is a different subsequent release: {} -> {}",
            group(&REPACK_PROPER_REGEX, &searchee.title, "type").unwrap_or_else(|| "INITIAL".into()),
            group(&REPACK_PROPER_REGEX, &candidate.name, "type").unwrap_or_else(|| "INITIAL".into())
        ),
        Decision::FuzzySizeMismatch => format!(
            "the total sizes are outside of the fuzzySizeThreshold range: {:.3} > {}",
            ((candidate.size.unwrap_or(0) - searchee.length) as f64 / searchee.length as f64).abs(),
            super::utils::js_number(get_fuzzy_size_factor(searchee, cfg))
        ),
        Decision::Match => "all file sizes and file names match: torrent progress will be 100%".into(),
        Decision::MatchSizeOnly => "all file sizes match: torrent progress will be 100%".into(),
        Decision::MatchPartial => format!(
            "most file sizes match: torrent progress will be ~{:.3}%",
            meta.map(|m| get_partial_size_ratio(m, searchee) * 100.0).unwrap_or(0.0)
        ),
        Decision::PartialSizeMismatch => format!(
            "too many files are missing or have different sizes: torrent progress would be {:.3}%",
            meta.map(|m| get_partial_size_ratio(m, searchee) * 100.0).unwrap_or(0.0)
        ),
        Decision::SizeMismatch => format!(
            "some files are missing or have different sizes{}",
            if meta.is_some_and(|m| compare_file_trees_partial(m, searchee, cfg)) { " (will match in partial match mode)" } else { "" }
        ),
        Decision::SameInfoHash => "the infoHash is the same".into(),
        Decision::InfoHashAlreadyExists => "the infoHash matches a torrent you already have".into(),
        Decision::FileTreeMismatch => format!(
            "it has a different file tree{}",
            if cfg.match_mode == MatchMode::Strict { " (will match in flexible or partial matchMode)" } else { "" }
        ),
        Decision::MagnetLink => "the torrent is a magnet link".into(),
        Decision::RateLimited => "cross-seed has reached this tracker's rate limit".into(),
        Decision::DownloadFailed => "the torrent file failed to download".into(),
        Decision::NoDownloadLink => "it doesn't have a download link".into(),
        Decision::BlockedRelease => format!(
            "it matches the blocklist: {}",
            or_undefined(
                find_blocked_string_in_release_maybe(searchee, &cfg.block_list)
                    .or_else(|| meta.and_then(|m| find_blocked_string_in_release_maybe(&Searchee::from_metafile_raw(m), &cfg.block_list)))
            )
        ),
    }
}

#[cfg(test)]
mod tests {
    //! One test per decision, with releases shaped like the ones decide.ts tells apart.
    use super::*;
    use crate::crossseed::config::Override;
    use crate::crossseed::searchee::Label;
    use crate::crossseed::torrent::build;

    fn meta(name: &str, files: &[(&str, i64)], salt: &str) -> Metafile {
        Metafile::decode(&build(name, files, salt)).unwrap()
    }

    /// A searchee from a client torrent with these files under `name`.
    fn have(name: &str, files: &[(&str, i64)]) -> Searchee {
        let m = meta(name, files, "ours");
        Searchee { client_host: Some("qbit".into()), label: Label::Search, ..Searchee::from_metafile_raw(&m) }
    }

    fn candidate(name: &str, size: Option<i64>) -> Candidate {
        Candidate {
            name: name.into(),
            guid: "g".into(),
            link: "https://site/dl/1".into(),
            tracker: "Site".into(),
            size,
            ..Default::default()
        }
    }

    fn cfg(mode: MatchMode) -> RuntimeConfig {
        RuntimeConfig::default().with(&Override { match_mode: Some(mode), ..Default::default() })
    }

    const PACK: &str = "Show.Name.S01.1080p.WEB-DL-GRP";
    const EPS: [(&str, i64); 3] =
        [("Show.Name.S01E01.mkv", 1_000_000), ("Show.Name.S01E02.mkv", 1_100_000), ("Show.Name.S01E03.mkv", 1_200_000)];

    fn decide(m: &Metafile, s: &Searchee, c: &RuntimeConfig) -> Decision {
        assess_metafile(m, s, &HashSet::new(), &c.block_list, c)
    }

    #[test]
    fn match_when_names_and_sizes_are_the_same() {
        let s = have(PACK, &EPS);
        let m = meta(PACK, &EPS, "theirs");
        for mode in [MatchMode::Strict, MatchMode::Flexible, MatchMode::Partial] {
            assert_eq!(decide(&m, &s, &cfg(mode)), Decision::Match);
        }
        assert_eq!(
            reason(Decision::Match, &s, &candidate(PACK, None), Some(&m), &cfg(MatchMode::Strict)),
            "all file sizes and file names match: torrent progress will be 100%"
        );
    }

    #[test]
    fn match_size_only_when_names_differ_outside_strict() {
        let s = have(PACK, &EPS);
        let renamed = [("show.name.s01e01.mkv", 1_000_000), ("show.name.s01e02.mkv", 1_100_000), ("show.name.s01e03.mkv", 1_200_000)];
        let m = meta(PACK, &renamed, "theirs");
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Flexible)), Decision::MatchSizeOnly);
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Partial)), Decision::MatchSizeOnly);
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Strict)), Decision::FileTreeMismatch);
        assert_eq!(
            reason(Decision::FileTreeMismatch, &s, &candidate(PACK, None), Some(&m), &cfg(MatchMode::Strict)),
            "it has a different file tree (will match in flexible or partial matchMode)"
        );
    }

    #[test]
    fn match_partial_and_partial_size_mismatch() {
        let s = have(PACK, &EPS);
        let mut with_nfo = EPS.to_vec();
        with_nfo.push(("Show.Name.S01.nfo", 1_000));
        let m = meta(PACK, &with_nfo, "theirs");
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Partial)), Decision::MatchPartial);
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Flexible)), Decision::SizeMismatch);
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Strict)), Decision::SizeMismatch);
        let r = reason(Decision::SizeMismatch, &s, &candidate(PACK, None), Some(&m), &cfg(MatchMode::Flexible));
        assert_eq!(r, "some files are missing or have different sizes (will match in partial match mode)");
        let r = reason(Decision::MatchPartial, &s, &candidate(PACK, None), Some(&m), &cfg(MatchMode::Partial));
        assert_eq!(r, "most file sizes match: torrent progress will be ~99.970%");
        let mut more = EPS.to_vec();
        more.push(("Show.Name.S01E04.mkv", 1_300_000));
        let m = meta(PACK, &more, "theirs");
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Partial)), Decision::PartialSizeMismatch);
        let r = reason(Decision::PartialSizeMismatch, &s, &candidate(PACK, None), Some(&m), &cfg(MatchMode::Partial));
        assert_eq!(r, "too many files are missing or have different sizes: torrent progress would be 71.739%");
    }

    #[test]
    fn size_mismatch() {
        let s = have(PACK, &EPS);
        let m =
            meta(PACK, &[("Show.Name.S01E01.mkv", 1_000_000), ("Show.Name.S01E02.mkv", 1_100_000), ("Show.Name.S01E03.mkv", 999_999)], "x");
        assert_eq!(decide(&m, &s, &cfg(MatchMode::Flexible)), Decision::SizeMismatch);
        assert_eq!(
            reason(Decision::SizeMismatch, &s, &candidate(PACK, None), Some(&m), &cfg(MatchMode::Flexible)),
            "some files are missing or have different sizes"
        );
    }

    #[test]
    fn same_info_hash_and_already_exists() {
        let s = have(PACK, &EPS);
        let ours = meta(PACK, &EPS, "ours");
        assert_eq!(decide(&ours, &s, &cfg(MatchMode::Strict)), Decision::SameInfoHash);
        let theirs = meta(PACK, &EPS, "theirs");
        let have_it: HashSet<String> = [theirs.info_hash.clone()].into();
        assert_eq!(assess_metafile(&theirs, &s, &have_it, &[], &cfg(MatchMode::Strict)), Decision::InfoHashAlreadyExists);
    }

    #[test]
    fn blocked_release_by_the_torrent() {
        let s = have(PACK, &EPS);
        let m = meta(PACK, &EPS, "theirs");
        let block = vec![format!("infoHash:{}", m.info_hash)];
        assert_eq!(assess_metafile(&m, &s, &HashSet::new(), &block, &cfg(MatchMode::Strict)), Decision::BlockedRelease);
        let c = RuntimeConfig { block_list: block, ..cfg(MatchMode::Strict) };
        assert_eq!(
            reason(Decision::BlockedRelease, &s, &candidate(PACK, None), Some(&m), &c),
            format!("it matches the blocklist: infoHash:{}", m.info_hash)
        );
    }

    #[test]
    fn single_episode_against_a_season_is_a_file_tree_mismatch() {
        let s = have(PACK, &EPS);
        let ep = meta("Show.Name.S01E01.1080p.WEB-DL-GRP.mkv", &[("Show.Name.S01E01.1080p.WEB-DL-GRP.mkv", 1_000_000)], "x");
        assert_eq!(decide(&ep, &s, &cfg(MatchMode::Partial)), Decision::FileTreeMismatch);
    }

    #[test]
    fn checks_on_the_name() {
        let s = have(PACK, &EPS);
        let c = cfg(MatchMode::Flexible);
        let check = |name: &str, size: Option<i64>| pre_download(&candidate(name, size), &s, &c);
        assert_eq!(check("Show.Name.S01.1080p.WEB-DL-OTHER", None), Some(Decision::ReleaseGroupMismatch));
        assert_eq!(check("Show.Name.S01.720p.WEB-DL-GRP", None), Some(Decision::ResolutionMismatch));
        let nf = have("Show.Name.S01.1080p.NF.WEB-DL-GRP", &EPS);
        assert_eq!(pre_download(&candidate("Show.Name.S01.1080p.AMZN.WEB-DL-GRP", None), &nf, &c), Some(Decision::SourceMismatch));
        assert_eq!(check("Show.Name.S01.REPACK.1080p.WEB-DL-GRP", None), Some(Decision::ProperRepackMismatch));
        assert_eq!(check(PACK, Some(3_400_000)), Some(Decision::FuzzySizeMismatch));
        assert_eq!(check(PACK, Some(3_350_000)), None, "within 2%");
        assert_eq!(check("Show.Name.S01.1080p.WEB", None), None, "no group on one side passes");
        assert_eq!(check("Show.Name.S01.1080p.WEB-DL-GRPx", None), None, "one group starts with the other");
        let mut no_link = candidate(PACK, None);
        no_link.link.clear();
        assert_eq!(pre_download(&no_link, &s, &c), Some(Decision::NoDownloadLink));
        let r = |d, name: &str, size| reason(d, &s, &candidate(name, size), None, &c);
        assert_eq!(
            r(Decision::ReleaseGroupMismatch, "Show.Name.S01.1080p.WEB-DL-OTHER", None),
            "it has a different release group: GRP -> OTHER"
        );
        assert_eq!(r(Decision::ResolutionMismatch, "Show.Name.S01.720p.WEB-DL-GRP", None), "its resolution does not match: 1080p -> 720p");
        assert_eq!(
            reason(Decision::SourceMismatch, &nf, &candidate("Show.Name.S01.1080p.AMZN.WEB-DL-GRP", None), None, &c),
            "it has a different source: NF -> AMZN"
        );
        assert_eq!(
            r(Decision::ProperRepackMismatch, "Show.Name.S01.REPACK.1080p.WEB-DL-GRP", None),
            "one is a different subsequent release: INITIAL -> REPACK"
        );
        assert_eq!(
            r(Decision::FuzzySizeMismatch, PACK, Some(3_400_000)),
            "the total sizes are outside of the fuzzySizeThreshold range: 0.030 > 0.02"
        );
        assert_eq!(r(Decision::NoDownloadLink, PACK, None), "it doesn't have a download link");
    }

    #[test]
    fn anime_groups() {
        assert!(release_group_does_match("[SubsPlease] Show - 01 (1080p)", "[SubsPlease] Show - 01 (1080p) [ABCD]"));
        assert!(!release_group_does_match("Show.S01E01.1080p-GRP", "Show.S01E01.1080p-OTHER"));
    }

    #[tokio::test]
    async fn the_fetch_and_its_failures() {
        let s = have(PACK, &EPS);
        let c = cfg(MatchMode::Strict);
        let none = HashSet::new();
        let theirs = meta(PACK, &EPS, "theirs");
        let mut input = MetaOrCandidate::Candidate(candidate(PACK, Some(1)));
        let t = theirs.clone();
        let a = assess_candidate(&mut input, &s, &none, &[], &c, |_| async move { Ok(t) }).await;
        assert_eq!(a.decision, Decision::FuzzySizeMismatch, "the site's size is checked before fetching");
        let mut input = MetaOrCandidate::Candidate(candidate(PACK, None));
        let t = theirs.clone();
        let a = assess_candidate(&mut input, &s, &none, &[], &c, |_| async move { Ok(t) }).await;
        assert_eq!((a.decision, a.meta_cached), (Decision::Match, true));
        let MetaOrCandidate::Candidate(cand) = &input else { unreachable!() };
        assert_eq!(cand.size, Some(3_300_000), "the size is corrected to the torrent's");
        for (err, want) in [
            (SnatchError::MagnetLink, Decision::MagnetLink),
            (SnatchError::RateLimited, Decision::RateLimited),
            (SnatchError::Failed("404".into()), Decision::DownloadFailed),
        ] {
            let mut input = MetaOrCandidate::Candidate(candidate(PACK, None));
            let a = assess_candidate(&mut input, &s, &none, &[], &c, |_| async move { Err(err) }).await;
            assert_eq!(a.decision, want);
        }
        let blocked = vec!["name:Show".to_string()];
        let mut input = MetaOrCandidate::Candidate(candidate(PACK, None));
        let a = assess_candidate(&mut input, &s, &none, &blocked, &c, |_| async { Err(SnatchError::MagnetLink) }).await;
        assert_eq!(a.decision, Decision::BlockedRelease, "a blocked searchee isn't fetched for");
        let mut cached = MetaOrCandidate::Meta(theirs);
        let a = assess_candidate(&mut cached, &s, &none, &[], &c, |_| async { Err(SnatchError::MagnetLink) }).await;
        assert_eq!(a.decision, Decision::Match, "a cached torrent skips the name checks and the fetch");
        assert_eq!(reason(Decision::MagnetLink, &s, &candidate(PACK, None), None, &c), "the torrent is a magnet link");
        assert_eq!(reason(Decision::RateLimited, &s, &candidate(PACK, None), None, &c), "cross-seed has reached this tracker's rate limit");
        assert_eq!(reason(Decision::DownloadFailed, &s, &candidate(PACK, None), None, &c), "the torrent file failed to download");
        assert_eq!(reason(Decision::SameInfoHash, &s, &candidate(PACK, None), None, &c), "the infoHash is the same");
        assert_eq!(
            reason(Decision::InfoHashAlreadyExists, &s, &candidate(PACK, None), None, &c),
            "the infoHash matches a torrent you already have"
        );
    }

    #[test]
    fn seasons_from_episodes_compare_by_name_with_their_own_ratio() {
        let virt = Searchee {
            name: "Show.Name.S1".into(),
            title: "Show.Name.S1".into(),
            files: EPS.iter().map(|(n, l)| File { name: n.to_string(), path: format!("/tv/{n}"), length: *l }).collect(),
            length: 3_300_000,
            ..Searchee::default()
        };
        let m = meta(PACK, &EPS, "theirs");
        assert!(compare_file_trees(&m, &virt));
        let c = RuntimeConfig { season_from_episodes: Some(0.8), ..cfg(MatchMode::Partial) };
        assert_eq!(get_fuzzy_size_factor(&virt, &c), 1.0 - 0.8);
        assert_eq!(get_min_size_ratio(&virt, &c), 0.8);
        assert!(fuzzy_size_does_match(2_700_000, &virt, &c));
    }
}
