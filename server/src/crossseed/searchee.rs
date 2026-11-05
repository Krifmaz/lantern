// Ported from cross-seed v6.13.9 (src/searchee.ts), Apache-2.0.
//! A searchee is what cross-seed looks for on other sites: a torrent in a download client, a `.torrent` file, a
//! folder or file of data, or a season put together from episodes. This module holds its shape, what kind of media
//! it is, its title as parsed from its files, its release group, and the keys that tell two releases of the same
//! thing apart.

use super::config::RuntimeConfig;
use super::constants::*;
use super::jsre::{captures, group, split, test};
use super::torrent::Metafile;
use super::utils::*;
use std::path::Path;

/// `File`: one file of a searchee or a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct File {
    /// Its file name (the last part of the path).
    pub name: String,
    /// Its path: inside the torrent (`Folder/a.mkv`), or on disk for a season made of episodes.
    pub path: String,
    /// Its size in bytes.
    pub length: i64,
}

/// `SearcheeLabel`: what found the searchee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Label {
    /// The search job.
    #[default]
    Search,
    /// The RSS job.
    Rss,
    /// The inject job.
    Inject,
    /// `POST /api/announce`.
    Announce,
    /// `POST /api/webhook`.
    Webhook,
}

impl Label {
    /// cross-seed's name for it, as in its logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Search => "search",
            Label::Rss => "rss",
            Label::Inject => "inject",
            Label::Announce => "announce",
            Label::Webhook => "webhook",
        }
    }
}

/// `Searchee`: what is looked for on other sites.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Searchee {
    /// The torrent's info hash, when it is a torrent (no info hash and no path: a season made of episodes).
    pub info_hash: Option<String>,
    /// The data's path on disk, when it is data.
    pub path: Option<String>,
    /// Its files.
    pub files: Vec<File>,
    /// The original name (the folder, file or torrent name).
    pub name: String,
    /// The name, or a better one parsed from the files (`Season 7` becomes `Show S7`).
    pub title: String,
    /// Every file's size added up.
    pub length: i64,
    /// When its newest file was written (milliseconds since 1970), for data and seasons made of episodes.
    pub mtime_ms: Option<f64>,
    /// The download client it is in.
    pub client_host: Option<String>,
    /// Its folder in the client.
    pub save_path: Option<String>,
    /// Its category in the client.
    pub category: Option<String>,
    /// Its tags in the client (`None` for data and fetched torrents, which have none to ask about).
    pub tags: Option<Vec<String>>,
    /// Its trackers' hosts.
    pub trackers: Option<Vec<String>>,
    /// What found it.
    pub label: Label,
}

impl Searchee {
    /// The searchee a fetched or saved torrent makes (`Metafile` as a searchee: no client, path or tags).
    pub fn from_metafile_raw(meta: &Metafile) -> Searchee {
        Searchee {
            info_hash: Some(meta.info_hash.clone()),
            files: meta.files.clone(),
            name: meta.name.clone(),
            title: meta.title.clone(),
            length: meta.length,
            category: meta.category.clone(),
            tags: meta.tags.clone(),
            trackers: Some(meta.trackers.clone()),
            ..Searchee::default()
        }
    }

    /// Whether it is a season made of episodes (`!infoHash && !path`).
    pub fn is_virtual(&self) -> bool {
        self.info_hash.is_none() && self.path.is_none()
    }
}

/// `getMediaType`: what kind of media a searchee is, from its title and its files' endings.
pub fn get_media_type(s: &Searchee) -> MediaType {
    media_type_of(&s.title, &s.files)
}

/// [`get_media_type`] from a title and files.
pub fn media_type_of(title: &str, files: &[File]) -> MediaType {
    if test(&EP_REGEX, title) {
        return MediaType::Episode;
    }
    if test(&SEASON_REGEX, title) {
        return MediaType::Season;
    }
    if has_ext(files, VIDEO_EXTENSIONS) {
        if test(&MOVIE_REGEX, title) {
            return MediaType::Movie;
        }
        if match_anime(title).is_some() {
            return MediaType::Anime;
        }
        return MediaType::Video;
    }
    if has_ext(files, VIDEO_DISC_EXTENSIONS) {
        if test(&MOVIE_REGEX, title) {
            return MediaType::Movie;
        }
        return MediaType::Video;
    }
    if has_ext(files, &[".rar"]) && test(&MOVIE_REGEX, title) {
        return MediaType::Movie;
    }
    if has_ext(files, AUDIO_EXTENSIONS) {
        return MediaType::Audio;
    }
    if has_ext(files, BOOK_EXTENSIONS) {
        return MediaType::Book;
    }
    MediaType::Other
}

/// `getFuzzySizeFactor`: how far a candidate's size may be from the searchee's (a share of it).
pub fn get_fuzzy_size_factor(s: &Searchee, cfg: &RuntimeConfig) -> f64 {
    match cfg.season_from_episodes {
        Some(sfe) if sfe != 0.0 && s.is_virtual() => 1.0 - sfe,
        _ => cfg.fuzzy_size_threshold,
    }
}

/// `getMinSizeRatio`: the share of a candidate that must be on disk for a partial match.
pub fn get_min_size_ratio(s: &Searchee, cfg: &RuntimeConfig) -> f64 {
    match cfg.season_from_episodes {
        Some(sfe) if sfe != 0.0 && s.is_virtual() => sfe,
        _ => 1.0 - cfg.fuzzy_size_threshold,
    }
}

/// `path.dirname` (POSIX).
pub fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.starts_with('/') { "/".into() } else { ".".into() };
    }
    match t.rfind('/') {
        Some(0) => "/".into(),
        Some(at) => match t[..at].trim_end_matches('/') {
            "" => "/".into(),
            d => d.to_string(),
        },
        None => ".".into(),
    }
}

/// `path.basename` (POSIX).
pub fn basename(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

/// `getRoot`: a torrent file's top folder (or the file itself); a path from a root is refused.
pub fn get_root(file: &File) -> Result<String, String> {
    let p = &file.path;
    if p.starts_with('/') || test(&ABS_WIN_PATH_REGEX, p) {
        return Err(format!(
            "absolute paths for the torrent file tree are not supported. File tree paths must be relative to the torrent save path: {p}"
        ));
    }
    let mut root = p.clone();
    let mut parent = dirname(&root);
    while parent != "." {
        root = parent;
        parent = dirname(&root);
    }
    Ok(root)
}

/// `getRootFolder`: the top folder of a file, `None` for a file at the top.
pub fn get_root_folder(file: &File) -> Result<Option<String>, String> {
    let root = get_root(file)?;
    Ok(if root == file.path { None } else { Some(root) })
}

/// `getLargestFile`: the biggest file (of equal sizes, the last one, as cross-seed's reduce keeps).
pub fn get_largest_file(files: &[File]) -> Option<&File> {
    files.iter().reduce(|a, b| if a.length > b.length { a } else { b })
}

/// `parseMetaInfo`: what all video files agree on (repack, resolution, source, group), added to a parsed title.
fn parse_meta_info(video_file_names: &[&str]) -> String {
    let mut meta = String::new();
    let stems: Vec<String> = video_file_names.iter().map(|n| strip_extension(n)).collect();
    if stems.iter().any(|s| group(&REPACK_PROPER_REGEX, s, "type").is_some_and(|t| !t.is_empty())) {
        meta.push_str(" REPACK");
    }
    let res: Vec<String> = stems
        .iter()
        .filter_map(|s| group(&RES_STRICT_REGEX, s, "res").map(|r| r.trim().to_lowercase()))
        .filter(|r| !r.is_empty())
        .collect();
    if res.len() == stems.len() && res.iter().all(|r| *r == res[0]) && !res.is_empty() {
        meta.push_str(&format!(" {}", res[0]));
    }
    let sources: Vec<&str> = stems.iter().filter_map(|s| parse_source(s)).collect();
    if sources.len() == stems.len() && !sources.is_empty() && sources.iter().all(|s| *s == sources[0]) {
        meta.push_str(&format!(" {}", sources[0]));
    }
    let groups: Vec<String> = stems.iter().filter_map(|s| get_release_group(s)).filter(|g| !g.is_empty()).collect();
    if groups.len() == stems.len() && !groups.is_empty() && groups.iter().all(|g| g.to_lowercase() == groups[0].to_lowercase()) {
        meta.push_str(&format!("-{}", groups[0]));
    }
    meta
}

/// `parseTitle`: the title of a release from its name, or from its files when the name is only `Season 1` or has
/// no digits; `None` when a season folder's files say nothing.
pub fn parse_title(name: &str, files: &[File], path: Option<&str>) -> Option<String> {
    let season_match = if name.encode_utf16().count() < 12 { captures(&SONARR_SUBFOLDERS_REGEX, name) } else { None };
    let season_num = season_match.as_ref().and_then(|c| c.name("seasonNum").map(|m| m.as_str().to_string()));
    if season_match.is_none() && (name.chars().any(|c| c.is_ascii_digit()) || !has_ext(files, VIDEO_EXTENSIONS)) {
        return Some(name.to_string());
    }
    let video_files = files_with_ext(files, VIDEO_EXTENSIONS);
    let names: Vec<&str> = video_files.iter().map(|f| f.name.as_str()).collect();
    for vf in &video_files {
        if let Some(ep) = captures(&EP_REGEX, &vf.name) {
            let g = |n: &str| ep.name(n).map(|m| m.as_str().to_string());
            let season_val = g("season").or_else(|| g("year")).or_else(|| season_num.clone());
            let season = season_val.map(|v| format!("S{}", extract_int(&v))).unwrap_or_default();
            let episode = if video_files.len() == 1 {
                match g("episode") {
                    Some(e) => format!("E{}", extract_int(&e)),
                    None => format!("E{}.{}", g("month").unwrap_or_default(), g("day").unwrap_or_default()),
                }
            } else {
                String::new()
            };
            if !season.is_empty() || !episode.is_empty() || season_match.is_none() {
                let meta = parse_meta_info(&names);
                return Some(format!("{} {season}{episode}{meta}", g("title").unwrap_or_default()).trim().to_string());
            }
        }
        if let (Some(p), Some(num)) = (path, season_num.as_ref()) {
            if let Some(title) = group(&ARR_DIR_REGEX, &basename(&dirname(p)), "title").filter(|t| !t.is_empty()) {
                let meta = parse_meta_info(&names);
                return Some(format!("{title} S{num}{meta}"));
            }
        }
        if let Some(anime) = match_anime(&vf.name) {
            let season = season_num.as_ref().map(|n| format!("S{n}")).unwrap_or_default();
            if !season.is_empty() || season_match.is_none() {
                let meta = parse_meta_info(&names);
                let title = anime.name("title").map(|m| m.as_str()).unwrap_or("");
                return Some(format!("{title} {season}{meta}").trim().to_string());
            }
        }
    }
    if season_match.is_none() {
        Some(name.to_string())
    } else {
        None
    }
}

/// `getAllTitles`: the titles, and each part of one written as `A aka B`.
pub fn get_all_titles(titles: &[String]) -> Vec<String> {
    let mut all = titles.to_vec();
    for t in titles {
        if test(&AKA_REGEX, t) && t.trim().to_lowercase() != "aka" {
            all.extend(split(&AKA_REGEX, t));
        }
    }
    all
}

/// The titles the episode, season, film or anime pattern finds in `stem`.
fn pattern_titles(stem: &str) -> Vec<String> {
    let caps = captures(&EP_REGEX, stem)
        .or_else(|| captures(&SEASON_REGEX, stem))
        .or_else(|| captures(&MOVIE_REGEX, stem))
        .or_else(|| match_anime(stem));
    match caps {
        Some(c) => {
            ["title", "altTitle"].iter().filter_map(|g| c.name(g).map(|m| m.as_str().to_string())).filter(|t| !t.is_empty()).collect()
        }
        None => Vec::new(),
    }
}

/// `getReleaseGroup`: the group after the last `-`, unless it is part of the title or isn't a group (`x264`).
pub fn get_release_group(stem: &str) -> Option<String> {
    let parsed = group(&RELEASE_GROUP_REGEX, stem, "group")?.trim().to_string();
    if test(&BAD_GROUP_PARSE_REGEX, &parsed) {
        return None;
    }
    for title in get_all_titles(&pattern_titles(stem)) {
        if let Some(g) = group(&RELEASE_GROUP_REGEX, &title, "group") {
            let g = g.trim();
            if parsed.contains(g) {
                return None;
            }
        }
    }
    Some(parsed)
}

/// `getKeyMetaInfo`: `.1080p.NF-grp`: resolution, source and group in small letters, to tell releases apart.
pub fn get_key_meta_info(stem: &str) -> String {
    let res = group(&RES_STRICT_REGEX, stem, "res").map(|r| format!(".{r}")).unwrap_or_default();
    let source = parse_source(stem).map(|s| format!(".{s}")).unwrap_or_default();
    if let Some(g) = get_release_group(stem) {
        return format!("{res}{source}-{g}").to_lowercase();
    }
    if let Some(g) = group(&ANIME_GROUP_REGEX, stem, "group") {
        return format!("{res}{source}-{g}").to_lowercase();
    }
    format!("{res}{source}").to_lowercase()
}

/// The keys of a release: the titles to look up with, and the titles to name a season by.
#[derive(Debug, Clone, PartialEq)]
pub struct Keys {
    /// The titles named with their year, season or nothing (`Show.S1`).
    pub ensemble_titles: Vec<String>,
    /// The titles as [`create_key_title`] makes them.
    pub key_titles: Vec<String>,
    /// The year, `S1`, the episode or the release, as text.
    pub element: String,
    /// The season (`S1`), for episodes and seasons.
    pub season: Option<String>,
}

fn keys_from(titles: Vec<String>, suffix: impl Fn(&str) -> String, skip_bad: bool) -> Option<(Vec<String>, Vec<String>)> {
    let mut keys = Vec::new();
    let mut ens = Vec::new();
    for t in get_all_titles(&titles) {
        if t.is_empty() || (skip_bad && is_bad_title(&t)) {
            continue;
        }
        let Some(k) = create_key_title(&t) else { continue };
        keys.push(k);
        ens.push(suffix(&t));
    }
    if keys.is_empty() {
        None
    } else {
        Some((ens, keys))
    }
}

/// `getMovieKeys`.
pub fn get_movie_keys(stem: &str) -> Option<Keys> {
    let c = captures(&MOVIE_REGEX, stem)?;
    let year = extract_int(c.name("year")?.as_str());
    let (ensemble_titles, key_titles) = keys_from(vec![c.name("title")?.as_str().to_string()], |t| format!("{t}.{year}"), false)?;
    Some(Keys { ensemble_titles, key_titles, element: year.to_string(), season: None })
}

/// `getSeasonKeys`.
pub fn get_season_keys(stem: &str) -> Option<Keys> {
    let c = captures(&SEASON_REGEX, stem)?;
    let season = format!("S{}", extract_int(c.name("season")?.as_str()));
    let (ensemble_titles, key_titles) = keys_from(vec![c.name("title")?.as_str().to_string()], |t| format!("{t}.{season}"), false)?;
    Some(Keys { ensemble_titles, key_titles, element: season.clone(), season: Some(season) })
}

/// `getEpisodeKeys`.
pub fn get_episode_keys(stem: &str) -> Option<Keys> {
    let c = captures(&EP_REGEX, stem)?;
    let g = |n: &str| c.name(n).map(|m| m.as_str().to_string());
    let season = match (g("season"), g("year")) {
        (Some(s), _) => Some(format!("S{}", extract_int(&s))),
        (None, Some(y)) => Some(format!("S{y}")),
        _ => None,
    };
    let (ensemble_titles, key_titles) = keys_from(
        vec![g("title")?],
        |t| match &season {
            Some(s) => format!("{t}.{s}"),
            None => t.to_string(),
        },
        false,
    )?;
    let element = match g("episode") {
        Some(e) => extract_int(&e).to_string(),
        None => format!("{}.{}", g("month").unwrap_or_default(), g("day").unwrap_or_default()),
    };
    Some(Keys { ensemble_titles, key_titles, element, season })
}

/// `getAnimeKeys`.
pub fn get_anime_keys(stem: &str) -> Option<Keys> {
    let c = match_anime(stem)?;
    let titles: Vec<String> = ["title", "altTitle"].iter().filter_map(|g| c.name(g).map(|m| m.as_str().to_string())).collect();
    let (ensemble_titles, key_titles) = keys_from(titles, |t| t.to_string(), true)?;
    let release = extract_int(c.name("release")?.as_str());
    Some(Keys { ensemble_titles, key_titles, element: release.to_string(), season: None })
}

/// `createSearcheeFromMetafile`: a torrent as a searchee, titled by [`parse_title`].
pub fn create_searchee_from_metafile(meta: &Metafile) -> Result<Searchee, String> {
    match parse_title(&meta.name, &meta.files, None) {
        Some(title) => Ok(Searchee { title, ..Searchee::from_metafile_raw(meta) }),
        None => Err(format!("Could not find title for {} from child files", meta.name)),
    }
}

/// `getFilesFromDataRoot`: every file under `root` (or `root` itself), with paths from `root`'s folder.
pub fn get_files_from_data_root(root: &Path) -> Vec<File> {
    fn walk(p: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        if std::fs::metadata(p)?.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(p)?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            entries.sort();
            for e in entries {
                walk(&e, out)?;
            }
        } else {
            out.push(p.to_path_buf());
        }
        Ok(())
    }
    let parent = root.parent().unwrap_or(Path::new(""));
    let mut paths = Vec::new();
    if walk(root, &mut paths).is_err() {
        return Vec::new();
    }
    paths
        .into_iter()
        .filter_map(|f| {
            let length = std::fs::metadata(&f).ok()?.len() as i64;
            Some(File {
                path: f.strip_prefix(parent).unwrap_or(&f).to_string_lossy().into_owned(),
                name: f.file_name()?.to_string_lossy().into_owned(),
                length,
            })
        })
        .collect()
}

/// `createSearcheeFromPath`: data on disk as a searchee.
pub fn create_searchee_from_path(root: &Path) -> Result<Searchee, String> {
    let files = get_files_from_data_root(root);
    let shown = root.to_string_lossy().into_owned();
    if files.is_empty() {
        return Err(format!("Failed to retrieve files in {shown}"));
    }
    let length = files.iter().map(|f| f.length).sum();
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(title) = parse_title(&name, &files, Some(&shown)) else {
        return Err(format!("Could not find title for {shown} in parent directory or child files"));
    };
    let parent = root.parent().unwrap_or(Path::new(""));
    let mtime_ms = files
        .iter()
        .filter_map(|f| std::fs::metadata(parent.join(&f.path)).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))));
    Ok(Searchee { path: Some(shown), files, name, title, length, mtime_ms, ..Searchee::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, length: i64) -> File {
        File { name: basename(path), path: path.into(), length }
    }

    #[test]
    fn paths_like_node() {
        assert_eq!(dirname("a/b/c.mkv"), "a/b");
        assert_eq!(dirname("c.mkv"), ".");
        assert_eq!(dirname("/c.mkv"), "/");
        assert_eq!(basename("/data/Show.S01/"), "Show.S01");
        assert_eq!(get_root(&f("Show/Sub/a.mkv", 1)).unwrap(), "Show");
        assert_eq!(get_root_folder(&f("a.mkv", 1)).unwrap(), None);
        assert!(get_root(&f("/etc/a.mkv", 1)).unwrap_err().starts_with("absolute paths"));
    }

    #[test]
    fn titles_from_files() {
        let files = vec![f("Season 1/Show.Name.S01E01.1080p.WEB-GRP.mkv", 1), f("Season 1/Show.Name.S01E02.1080p.WEB-GRP.mkv", 1)];
        assert_eq!(parse_title("Season 1", &files, None).as_deref(), Some("Show.Name S1 1080p-GRP"));
        let one = vec![f("Show.Name.S01E05.720p-GRP.mkv", 1)];
        assert_eq!(parse_title("Show", &one, None).as_deref(), Some("Show.Name S1E5 720p-GRP"));
        assert_eq!(parse_title("Show.Name.S01E05", &one, None).as_deref(), Some("Show.Name.S01E05"));
        assert_eq!(parse_title("Season 1", &[f("Season 1/readme.mkv", 1)], None), None);
    }

    #[test]
    fn release_groups() {
        assert_eq!(get_release_group("Show.Name.S01E01.1080p.WEB-DL-GRP").as_deref(), Some("GRP"));
        assert_eq!(get_release_group("Show.Name.S01E01.1080p.x264"), None);
        assert_eq!(get_release_group("Movie.2020.1080p.BluRay.x264-SPARKS").as_deref(), Some("SPARKS"));
        assert_eq!(get_release_group("Movie.2020.1080p-x264"), None, "x264 is not a group");
        assert_eq!(get_key_meta_info("Show.S01E01.1080p.NF.WEB-DL-GRP"), ".1080p.nf-grp");
    }

    #[test]
    fn keys() {
        let k = get_episode_keys("The.Show.S01E05.1080p-GRP").unwrap();
        assert_eq!((k.key_titles[0].as_str(), k.season.as_deref(), k.element.as_str()), ("theshow", Some("S1"), "5"));
        let k = get_season_keys("The.Show.S02.1080p-GRP").unwrap();
        assert_eq!(k.ensemble_titles, vec!["The.Show.S2"]);
        let k = get_movie_keys("A.Movie.aka.Other.Name.2020.1080p").unwrap();
        assert_eq!(k.key_titles, vec!["amovieakaothername", "amovie", "othername"]);
    }
}
