// Ported from cross-seed v6.13.9 (src/utils.ts), Apache-2.0.
//! cross-seed's helpers for names and sizes: file endings, titles made comparable, search queries, sizes in words
//! and lists in words. Only the ones matching and the daemon need; the file and async helpers are Rust's own.

use super::constants::*;
use super::jsre::{captures, group, replace_all, replace_first};
use super::searchee::{get_all_titles, File};

/// `path.extname`: the ending of a file name from its last dot (`.mkv`), empty when there is none or the name
/// starts with its only dot.
pub fn extname(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(0) | None => "",
        Some(at) => &base[at..],
    }
}

/// `hasExt`: whether any file ends with one of `exts` (ignoring case).
pub fn has_ext(files: &[File], exts: &[&str]) -> bool {
    files.iter().any(|f| exts.contains(&extname(&f.name.to_lowercase())))
}

/// `filesWithExt`: the files that end with one of `exts` (ignoring case).
pub fn files_with_ext<'a>(files: &'a [File], exts: &[&str]) -> Vec<&'a File> {
    files.iter().filter(|f| exts.contains(&extname(&f.name.to_lowercase()))).collect()
}

/// `stripExtension`: the name without a known ending (case matters, as in cross-seed).
pub fn strip_extension(filename: &str) -> String {
    for ext in all_extensions() {
        if filename.ends_with(ext) {
            // path.basename(filename, ext): the last path part, without the ending unless that is all of it
            let base = filename.rsplit('/').next().unwrap_or(filename);
            return match base.strip_suffix(ext) {
                Some(stem) if !stem.is_empty() => stem.to_string(),
                _ => base.to_string(),
            };
        }
    }
    filename.to_string()
}

/// `extractInt`: the first number in `s` (0 when there is none, where cross-seed would fail).
pub fn extract_int(s: &str) -> i64 {
    let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// `humanReadableSize`: `1.23 kB` (powers of 1000), or `1.21 MiB` with `binary` (powers of 1024).
pub fn human_readable_size(bytes: f64, binary: bool) -> String {
    if bytes == 0.0 {
        return "0 B".into();
    }
    let k: f64 = if binary { 1024.0 } else { 1000.0 };
    let sizes: [&str; 5] = if binary { ["B", "KiB", "MiB", "GiB", "TiB"] } else { ["B", "kB", "MB", "GB", "TB"] };
    let exponent = ((bytes.abs().ln() / k.ln()).floor() as i32).clamp(0, 4);
    let coefficient = bytes / k.powi(exponent);
    // parseFloat(coefficient.toFixed(2)): two decimals at most, no trailing zeros
    let rounded = (coefficient * 100.0).round() / 100.0;
    format!("{} {}", js_number(rounded), sizes[exponent as usize])
}

/// A number as JavaScript writes it (`1`, `1.5`, `1.23`).
pub fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `sanitizeInfoHash`: the first 8 characters and `...`, for logs.
pub fn sanitize_info_hash(info_hash: &str) -> String {
    format!("{}...", info_hash.chars().take(8).collect::<String>())
}

/// `sanitizeUrl`: the address without its query, user or password (`https://example.com/path`; a bare host
/// gets its `/`).
pub fn sanitize_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(format!("{}{}", u.origin().ascii_serialization(), u.path()))
}

/// `formatAsList` with `type: "unit"`: `a, b, c` (`long`) or `a b c` (`narrow`); `conjunction`: `a, b, and c`.
pub fn format_as_list(items: &[String], sort: bool, style: &str, kind: &str) -> String {
    let mut v = items.to_vec();
    if sort {
        v.sort_by_key(|a| a.to_lowercase());
    }
    match (kind, style) {
        ("unit", "narrow") => v.join(" "),
        ("unit", _) => v.join(", "),
        _ => match v.len() {
            0 => String::new(),
            1 => v[0].clone(),
            2 => format!("{} and {}", v[0], v[1]),
            n => format!("{}, and {}", v[..n - 1].join(", "), v[n - 1]),
        },
    }
}

/// `matchAnime`: [`ANIME_REGEX`] on names a file could have; a longer name is not matched.
pub fn match_anime(name: &str) -> Option<fancy_regex::Captures<'_>> {
    if name.chars().count() > MAX_ANIME_NAME_LENGTH {
        return None;
    }
    captures(&ANIME_REGEX, name)
}

/// `cleanseSeparators`: brackets out, `._()[]` as spaces, single spaces, no hyphens at the ends.
pub fn cleanse_separators(s: &str) -> String {
    let s = replace_all(&ALL_SQUARE_BRACKETS_REGEX, s, "");
    let s: String = s.chars().map(|c| if matches!(c, '.' | '_' | '(' | ')' | '[' | ']') { ' ' } else { c }).collect();
    let s = replace_all(&ALL_SPACES_REGEX, &s, " ");
    // /^\s*-+|-+\s*$/g
    let s = s.trim_start();
    let s = if s.starts_with('-') { s.trim_start_matches('-') } else { s };
    let t = s.trim_end();
    let s = if t.ends_with('-') { t.trim_end_matches('-') } else { s };
    s.trim().to_string()
}

/// `cleanTitle`: [`cleanse_separators`] without a scene prefix.
pub fn clean_title(title: &str) -> String {
    let c = cleanse_separators(title);
    group(&SCENE_TITLE_REGEX, &c, "title").unwrap_or(c)
}

/// `cleanBookAndAudioTitle`: a book's or an album's name without formats, sizes and narrators.
pub fn clean_book_and_audio_title(title: &str) -> String {
    replace_first(&EBOOK_AND_MUSIC_RELEASE_REGEX, &cleanse_separators(title), "").trim().to_string()
}

/// `replaceLastOccurrence`: `s` with the last match of `re` replaced by `with`.
pub fn replace_last_occurrence(s: &str, re: &fancy_regex::Regex, with: &str) -> String {
    let last = re.find_iter(s).filter_map(|m| m.ok()).last();
    match last {
        Some(m) => format!("{}{}{}", &s[..m.start()], with, &s[m.end()..]),
        None => s.to_string(),
    }
}

/// `reformatTitleForSearching`: the series or film title to search for.
pub fn reformat_title_for_searching(name: &str) -> String {
    let series = group(&EP_REGEX, name, "title").or_else(|| group(&SEASON_REGEX, name, "title"));
    if let Some(series) = series {
        let title = clean_title(&series);
        return if title.chars().count() > 4 {
            replace_all(&ALL_SPACES_REGEX, &replace_last_occurrence(&title, &YEARS_REGEX, ""), " ").trim().to_string()
        } else {
            title
        };
    }
    let m = captures(&MOVIE_REGEX, name).and_then(|c| c.get(0).map(|m| m.as_str().to_string()));
    clean_title(m.as_deref().unwrap_or(name))
}

/// `createKeyTitle`: a title as letters and digits only, in small letters, without its last year when long.
pub fn create_key_title(title: &str) -> Option<String> {
    let key = replace_all(&NON_UNICODE_ALPHANUM_REGEX, &clean_title(title), "").to_lowercase();
    let n = key.chars().count();
    if n > 4 {
        Some(replace_last_occurrence(&key, &YEARS_REGEX, ""))
    } else if n > 0 {
        Some(key)
    } else {
        None
    }
}

/// `isBadTitle`: `season` or `ep` taken for a title.
pub fn is_bad_title(title: &str) -> bool {
    matches!(title.to_lowercase().as_str(), "season" | "ep")
}

/// `stripMetaFromName`: the name without its ending, scene prefix, group, resolution, proper/repack and source.
pub fn strip_meta_from_name(name: &str) -> String {
    let stem = strip_extension(name);
    let title = group(&SCENE_TITLE_REGEX, &stem, "title").unwrap_or(stem);
    let t = replace_first(&RELEASE_GROUP_REGEX, &title, "");
    let t = t.trim_end().strip_suffix('-').map(|s| s.trim_end().to_string()).unwrap_or(t);
    let t = replace_first(&RESOLUTION_REGEX, &t, "");
    let t = replace_first(&REPACK_PROPER_REGEX, &t, "");
    source_regex_remove(&t)
}

/// `getAnimeQueries`: the anime searches for a name: title and release, then the other title and release.
pub fn get_anime_queries(stem: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(c) = match_anime(stem) else { return out };
    let release = c.name("release").map(|m| m.as_str()).unwrap_or("undefined");
    if let Some(title) = c.name("title").map(|m| m.as_str()).filter(|t| !t.is_empty()) {
        let stripped = clean_title(title);
        out.push(format!("{} {release}", if stripped.is_empty() { title.to_string() } else { stripped }));
    }
    if let Some(alt) = c.name("altTitle").map(|m| m.as_str()).filter(|t| !t.is_empty()) {
        if is_bad_title(alt) {
            return out;
        }
        let stripped = clean_title(alt);
        out.push(format!("{} {release}", if stripped.is_empty() { alt.to_string() } else { stripped }));
    }
    out
}

/// `getVideoQueries`: the search for a video that is neither an episode, a season nor a film.
pub fn get_video_queries(stem: &str) -> Vec<String> {
    let no_parens = replace_all(&ALL_SPACES_REGEX, &replace_all(&ALL_PARENTHESES_REGEX, stem, ""), " ").trim().to_string();
    let no_parens = clean_title(&strip_meta_from_name(&no_parens));
    if no_parens.chars().count() >= MIN_VIDEO_QUERY_LENGTH {
        return vec![no_parens];
    }
    let q = clean_title(&strip_meta_from_name(stem));
    if !q.is_empty() {
        return vec![q];
    }
    let q = strip_meta_from_name(stem);
    if !q.is_empty() {
        return vec![q];
    }
    vec![stem.to_string()]
}

/// The Levenshtein distance between `a` and `b` (fastest-levenshtein's `distance`, over UTF-16 units).
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The titles a name gives for comparing: from the episode, season, film or anime pattern, else the name.
fn media_titles(s: &str) -> Vec<String> {
    let caps =
        captures(&EP_REGEX, s).or_else(|| captures(&SEASON_REGEX, s)).or_else(|| captures(&MOVIE_REGEX, s)).or_else(|| match_anime(s));
    let raw: Vec<String> = match caps {
        Some(c) => {
            ["title", "altTitle"].iter().filter_map(|g| c.name(g).map(|m| m.as_str().to_string())).filter(|t| !t.is_empty()).collect()
        }
        None => vec![s.to_string()],
    };
    get_all_titles(&raw).iter().filter_map(|t| create_key_title(&strip_meta_from_name(t))).collect()
}

/// `areMediaTitlesSimilar`: whether two names are of the same thing (within a third of the title's length in
/// edits, or one inside the other).
pub fn are_media_titles_similar(a: &str, b: &str) -> bool {
    let ta = media_titles(a);
    let tb = media_titles(b);
    let max = |t: &[String]| -> usize {
        if t.is_empty() {
            return 0;
        }
        let total: usize = t.iter().map(|x| x.encode_utf16().count()).sum();
        (total as f64 / t.len() as f64 / LEVENSHTEIN_DIVISOR as f64).floor() as usize
    };
    let max_distance = max(&ta).max(max(&tb));
    ta.iter().any(|x| tb.iter().any(|y| distance(x, y) <= max_distance || x.contains(y.as_str()) || y.contains(x.as_str())))
}

#[cfg(test)]
mod tests {
    // Ported from cross-seed v6.13.9 (tests/utils.test.ts), Apache-2.0.
    use super::*;
    use crate::crossseed::searchee::{get_media_type, Searchee};
    use std::time::Instant;

    fn file(name: &str) -> File {
        File { name: name.into(), length: 0, path: format!("/tmp/{name}") }
    }

    fn searchee(title: &str, files: Vec<File>) -> Searchee {
        Searchee { title: title.into(), name: "My Show".into(), files, ..Searchee::default() }
    }

    #[test]
    fn human_readable_size_returns_a_human_readable_size() {
        assert_eq!(human_readable_size(123.0, false), "123 B");
        assert_eq!(human_readable_size(1234.0, false), "1.23 kB");
        assert_eq!(human_readable_size(1000.0 * 1234.0, false), "1.23 MB");
        assert_eq!(human_readable_size(1024.0 * 1234.0, true), "1.21 MiB");
    }

    #[test]
    fn human_readable_size_truncates_number_when_byte_size_is_exact() {
        assert_eq!(human_readable_size(1000.0, false), "1 kB");
    }

    #[test]
    fn get_media_type_episode_and_season() {
        assert_eq!(get_media_type(&searchee("My.Show.S01E01", vec![])), MediaType::Episode);
        let s1 = searchee("My.Show.S01", vec![]);
        assert_eq!(get_media_type(&s1), MediaType::Season);
        assert_eq!(extract_int(&group(&SEASON_REGEX, &s1.title, "season").unwrap()), 1);
        let s2 = searchee("My.Show.Season 2", vec![]);
        assert_eq!(get_media_type(&s2), MediaType::Season);
        assert_eq!(extract_int(&group(&SEASON_REGEX, &s2.title, "season").unwrap()), 2);
    }

    #[test]
    fn get_media_type_video_files_by_extension() {
        assert_eq!(get_media_type(&searchee("My.Movie.2021", vec![file("media.mp4")])), MediaType::Movie);
        assert_eq!(get_media_type(&searchee("[GRP] My.Anime - 001", vec![file("media.mp4")])), MediaType::Anime);
        assert_eq!(get_media_type(&searchee("My.Video", vec![file("media.mp4")])), MediaType::Video);
    }

    #[test]
    fn get_media_type_rar_archives() {
        assert_eq!(get_media_type(&searchee("My.Movie.2021", vec![file("media.rar")])), MediaType::Movie);
        assert_eq!(get_media_type(&searchee("My.Video", vec![file("media.rar"), file("media.mp3")])), MediaType::Audio);
        assert_eq!(get_media_type(&searchee("My.Video", vec![file("media.rar"), file("media.epub")])), MediaType::Book);
        assert_eq!(get_media_type(&searchee("My.Other", vec![file("media.rar")])), MediaType::Other);
    }

    #[test]
    fn get_media_type_fallback_behaviour() {
        assert_eq!(get_media_type(&searchee("unknown", vec![file("media.mp3")])), MediaType::Audio);
        assert_eq!(get_media_type(&searchee("unknown", vec![file("media.epub")])), MediaType::Book);
        assert_eq!(get_media_type(&searchee("unknown", vec![file("media.xyz")])), MediaType::Other);
    }

    #[test]
    fn sanitize_url_strips_the_query_and_adds_a_slash() {
        assert_eq!(sanitize_url("https://example.com/path").as_deref(), Some("https://example.com/path"));
        assert_eq!(sanitize_url("https://example.com").as_deref(), Some("https://example.com/"));
        assert_eq!(sanitize_url("https://example.com/path?query=string").as_deref(), Some("https://example.com/path"));
    }

    #[test]
    fn match_anime_matches_like_anime_regex() {
        for name in ["[GRP] My.Anime - 001", "[Group] Some Show Season 2 - 07 [1080p]", "My Anime - 12 (2020)"] {
            let m = match_anime(name).expect(name);
            let r = captures(&ANIME_REGEX, name).unwrap();
            for g in ["group", "title", "altTitle", "year", "release"] {
                assert_eq!(m.name(g).map(|x| x.as_str()), r.name(g).map(|x| x.as_str()), "{name} {g}");
            }
        }
    }

    #[test]
    fn match_anime_matches_a_name_of_the_longest_length_a_file_can_have() {
        let name = format!("[GRP] {} - 001", "a".repeat(MAX_ANIME_NAME_LENGTH - 17));
        assert_eq!(name.len(), MAX_ANIME_NAME_LENGTH - 5);
        assert!(match_anime(&name).is_some());
        let longest = format!("{name}-----");
        assert_eq!(longest.len(), MAX_ANIME_NAME_LENGTH);
        assert!(match_anime(&longest).is_some());
    }

    #[test]
    fn match_anime_does_not_match_a_name_longer_than_a_file_can_have() {
        let name = format!("[GRP] {} - 001", "a".repeat(MAX_ANIME_NAME_LENGTH));
        assert!(captures(&ANIME_REGEX, &name).is_some());
        assert!(match_anime(&name).is_none());
    }

    /// cross-seed's one second, with room for an unoptimised test build on a busy machine.
    const QUICK_MS: u128 = if cfg!(debug_assertions) { 5000 } else { 1000 };

    #[test]
    fn match_anime_returns_quickly_for_a_long_name_that_does_not_match() {
        let hostile = format!("{}.mkv", "[a]".repeat(800));
        let start = Instant::now();
        assert!(match_anime(&hostile).is_none());
        assert!(match_anime(&hostile[..MAX_ANIME_NAME_LENGTH]).is_none());
        assert!(start.elapsed().as_millis() < QUICK_MS, "{:?}", start.elapsed());
    }

    #[test]
    fn match_anime_keeps_its_callers_quick() {
        let hostile = format!("{}.mkv", "[a]".repeat(800));
        let start = Instant::now();
        get_media_type(&searchee(&hostile, vec![file(&hostile)]));
        are_media_titles_similar(&hostile, &format!("{hostile}b"));
        get_anime_queries(&hostile);
        assert!(start.elapsed().as_millis() < QUICK_MS, "{:?}", start.elapsed());
    }

    #[test]
    fn titles_and_queries() {
        assert_eq!(strip_extension("Movie.2020.mkv"), "Movie.2020");
        assert_eq!(strip_extension("Movie.2020.MKV"), "Movie.2020.MKV");
        assert_eq!(clean_title("abc-Show.Name.(2020)"), "Show Name 2020");
        assert_eq!(create_key_title("The.Show.2020").as_deref(), Some("theshow"));
        assert_eq!(reformat_title_for_searching("The.Show.2019.S01E01.1080p-GRP"), "The Show");
        assert_eq!(reformat_title_for_searching("Some.Movie.2020.1080p.BluRay-GRP"), "Some Movie 2020");
        assert!(are_media_titles_similar("The.Show.S01E01.1080p-GRP", "The Show S01E01 720p"));
        assert!(!are_media_titles_similar("The.Show.S01E01", "Other.Thing.S01E01"));
        assert_eq!(get_anime_queries("[GRP] My Anime - 001"), vec!["My Anime 001"]);
        assert_eq!(get_video_queries("Some.Video.(extra).1080p-GRP"), vec!["Some Video"]);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(format_as_list(&["b".into(), "a".into()], true, "narrow", "unit"), "a b");
        assert_eq!(format_as_list(&["b".into(), "a".into(), "c".into()], true, "long", "conjunction"), "a, b, and c");
    }
}
