// Ported from cross-seed v6.13.9 (src/arr.ts), Apache-2.0.
//! Sonarr and Radarr lookups: a searchee's title is sent to the apps' `/api/v3/parse`, and the show's or film's
//! ids (TheTVDB, TMDb, IMDb, TVmaze) come back, so indexers that take ids are searched by them (see
//! [`super::search`]). The addresses are cross-seed's `sonarr` and `radarr` settings, each with its `apikey`.

use super::config::RuntimeConfig;
use super::constants::{MediaType, SCENE_TITLE_REGEX};
use super::jsre::group;
use super::utils::{cleanse_separators, sanitize_url, strip_meta_from_name};
use serde_json::Value;
use std::time::Duration;

/// `ExternalIds`: a show's or film's ids, as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExternalIds {
    /// `imdbId` (`tt0111161`).
    pub imdb_id: Option<String>,
    /// `tmdbId`.
    pub tmdb_id: Option<String>,
    /// `tvdbId`.
    pub tvdb_id: Option<String>,
    /// `tvMazeId`.
    pub tv_maze_id: Option<String>,
}

/// `ParsedMedia`: what an app's parse answered: a film's ids, or a show's ids and the episodes in the title.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedMedia {
    /// The film's ids, when it is a film (Radarr).
    pub movie: Option<ExternalIds>,
    /// The show's ids, when it is a show (Sonarr).
    pub series: Option<ExternalIds>,
    /// The episodes: (season, episode).
    pub episodes: Vec<(i64, i64)>,
}

impl ParsedMedia {
    /// `parsedMedia.movie ?? parsedMedia.series`.
    pub fn ids(&self) -> Option<&ExternalIds> {
        self.movie.as_ref().or(self.series.as_ref())
    }
}

/// `arrIdsEqual`.
pub fn arr_ids_equal(a: Option<&ExternalIds>, b: Option<&ExternalIds>) -> bool {
    let f = |x: Option<&ExternalIds>| x.cloned().unwrap_or_default();
    f(a) == f(b)
}

/// `formatFoundIds`: `IMDB=tt1 TMDB=2 TVDB=N/A TVMAZE=N/A`, as cross-seed logs them.
pub fn format_found_ids(ids: &ExternalIds) -> String {
    [("IMDB", &ids.imdb_id), ("TMDB", &ids.tmdb_id), ("TVDB", &ids.tvdb_id), ("TVMAZE", &ids.tv_maze_id)]
        .iter()
        .map(|(n, v)| format!("{n}={}", v.as_deref().unwrap_or("N/A")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// An id from an app's answer as text; `0`, `"0"`, empty and missing are none (cross-seed sets them to
/// `undefined`).
fn id_text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Number(n) if n.as_f64() == Some(0.0) => None,
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if s.is_empty() || s == "0" => None,
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Whether a JSON value is truthy, as JavaScript reads it.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// The ids of an app's `movie` or `series` object, and whether any of its fields is set once the zero ids are
/// taken out (cross-seed's `Object.values(ids).some(isTruthy)`).
fn ids_of(obj: &serde_json::Map<String, Value>) -> (ExternalIds, bool) {
    let ids = ExternalIds {
        imdb_id: id_text(obj.get("imdbId")),
        tmdb_id: id_text(obj.get("tmdbId")),
        tvdb_id: id_text(obj.get("tvdbId")),
        tv_maze_id: id_text(obj.get("tvMazeId")),
    };
    let any = obj.iter().any(|(k, v)| {
        if matches!(k.as_str(), "imdbId" | "tmdbId" | "tvdbId" | "tvMazeId") {
            id_text(Some(v)).is_some()
        } else {
            truthy(v)
        }
    });
    (ids, any)
}

/// An app's parse answer as [`ParsedMedia`], and whether it found something (any field set).
pub fn parse_answer(v: &Value) -> (ParsedMedia, bool) {
    let episodes = v
        .get("episodes")
        .and_then(Value::as_array)
        .map(|eps| {
            eps.iter()
                .map(|e| {
                    (
                        e.get("seasonNumber").and_then(Value::as_i64).unwrap_or(0),
                        e.get("episodeNumber").and_then(Value::as_i64).unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(m) = v.get("movie").and_then(Value::as_object) {
        let (ids, any) = ids_of(m);
        return (ParsedMedia { movie: Some(ids), series: None, episodes: Vec::new() }, any);
    }
    if let Some(s) = v.get("series").and_then(Value::as_object) {
        let (ids, any) = ids_of(s);
        return (ParsedMedia { movie: None, series: Some(ids), episodes }, any);
    }
    (ParsedMedia::default(), false)
}

/// `getRelevantArrInstances`: Sonarr for shows, Radarr for films, both for anime and other video.
pub fn relevant_arr_instances(cfg: &RuntimeConfig, media: MediaType) -> Vec<String> {
    match media {
        MediaType::Season | MediaType::Episode => cfg.sonarr.clone(),
        MediaType::Movie => cfg.radarr.clone(),
        MediaType::Anime | MediaType::Video => cfg.sonarr.iter().chain(cfg.radarr.iter()).cloned().collect(),
        _ => Vec::new(),
    }
}

/// The title an app is asked about: the title without its scene prefix, or for other video the name without
/// its group, resolution and the like.
pub fn lookup_title(searchee_title: &str, media: MediaType) -> String {
    if media != MediaType::Video {
        group(&SCENE_TITLE_REGEX, searchee_title, "title").unwrap_or_else(|| searchee_title.to_string())
    } else {
        cleanse_separators(&strip_meta_from_name(searchee_title))
    }
}

/// `makeArrApiCall`: GET `resource` (joined to the address's path) with `params`, the address's `apikey` in an
/// `X-Api-Key` header; the JSON answer or the problem in cross-seed's words.
pub async fn make_arr_api_call(u_arr_l: &str, resource: &str, params: &[(&str, &str)]) -> Result<Value, String> {
    let parsed = url::Url::parse(u_arr_l).map_err(|e| e.to_string())?;
    let apikey = parsed.query_pairs().find(|(k, _)| k == "apikey").map(|(_, v)| v.into_owned()).unwrap_or_default();
    let base = sanitize_url(u_arr_l).ok_or("Invalid URL")?;
    let mut url = url::Url::parse(&base).map_err(|e| e.to_string())?;
    let path = format!("{}/{}", url.path().trim_end_matches('/'), resource.trim_start_matches('/'));
    url.set_path(&path);
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("Lantern/{}", crate::VERSION))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = match client.get(url).header("X-Api-Key", apikey).send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err("connection timeout".into()),
        Err(e) => return Err(e.to_string()),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let sample: String = text.chars().take(1000).collect();
    let sample = if sample.is_empty() { String::new() } else { format!("first 1000 characters: {sample}") };
    if !status.is_success() {
        let code = if status.as_u16() == 401 { "401 Unauthorized (check apikey)".to_string() } else { status.as_u16().to_string() };
        return Err(format!("{code} {} {sample}", status.canonical_reason().unwrap_or("")));
    }
    serde_json::from_str(&text).map_err(|_| format!("Arr response was non-JSON. {sample}"))
}

/// `scanAllArrsForMedia`: asks each relevant app in turn; the first answer with ids, else `None`.
pub async fn scan_all_arrs_for_media(cfg: &RuntimeConfig, searchee_title: &str, media: MediaType) -> Option<ParsedMedia> {
    let urls = relevant_arr_instances(cfg, media);
    if urls.is_empty() {
        return None;
    }
    let title = lookup_title(searchee_title, media);
    let mut error = format!("No ids found for {title} | MediaType: {}", media.as_str().to_uppercase());
    for u in &urls {
        let name = if media == MediaType::Video && cfg.sonarr.contains(u) { format!("{title} S00E00") } else { title.clone() };
        let answer = match make_arr_api_call(u, "/api/v3/parse", &[("title", &name)]).await {
            Ok(a) => a,
            Err(e) => {
                error = e;
                continue;
            }
        };
        let (parsed, any) = parse_answer(&answer);
        if any {
            let label = if parsed.movie.is_some() { "radarr" } else { "sonarr" };
            tracing::debug!("cross-seed [{label}]: Found media for {name} -> {}", parsed.ids().map(format_found_ids).unwrap_or_default());
            return Some(parsed);
        }
    }
    tracing::debug!("cross-seed [arrs]: Lookup failed for {searchee_title} - {error} - make sure the item is added to an Arr instance.");
    None
}

/// The id search kinds an indexer takes (`IdSearchCaps`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdCaps {
    /// `tvdbid`.
    pub tvdb: bool,
    /// `tmdbid`.
    pub tmdb: bool,
    /// `imdbid`.
    pub imdb: bool,
    /// `tvmazeid`.
    pub tvmaze: bool,
}

/// `IdSearchParams`: the ids a search sends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdParams {
    /// `tvdbid`.
    pub tvdbid: Option<String>,
    /// `tmdbid`.
    pub tmdbid: Option<String>,
    /// `imdbid`.
    pub imdbid: Option<String>,
    /// `tvmazeid`.
    pub tvmazeid: Option<String>,
}

impl IdParams {
    /// Whether any id is sent (`Object.values(relevantIds).some(isTruthy)`).
    pub fn any(&self) -> bool {
        [&self.tvdbid, &self.tmdbid, &self.imdbid, &self.tvmazeid].iter().any(|v| v.as_deref().is_some_and(|s| !s.is_empty()))
    }
}

/// `getRelevantArrIds`: the ids the indexer takes for this kind of media.
pub fn relevant_arr_ids(movie_ids: IdCaps, tv_ids: IdCaps, parsed: &ParsedMedia) -> IdParams {
    let caps = if parsed.movie.is_some() { movie_ids } else { tv_ids };
    let ids = parsed.ids().cloned().unwrap_or_default();
    IdParams {
        tvdbid: if caps.tvdb { ids.tvdb_id } else { None },
        tmdbid: if caps.tmdb { ids.tmdb_id } else { None },
        imdbid: if caps.imdb { ids.imdb_id } else { None },
        tvmazeid: if caps.tvmaze { ids.tv_maze_id } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_answers_as_cross_seed_reads_them() {
        let (p, any) = parse_answer(&json!({
            "title": "Show S01E02", "series": { "title": "Show", "tvdbId": 81189, "imdbId": "tt0903747", "tmdbId": 0, "tvMazeId": "0" },
            "episodes": [{ "seasonNumber": 1, "episodeNumber": 2 }]
        }));
        assert!(any);
        let s = p.series.clone().unwrap();
        assert_eq!(
            (s.tvdb_id.as_deref(), s.imdb_id.as_deref(), s.tmdb_id.clone(), s.tv_maze_id.clone()),
            (Some("81189"), Some("tt0903747"), None, None)
        );
        assert_eq!(p.episodes, vec![(1, 2)]);
        assert_eq!(format_found_ids(&s), "IMDB=tt0903747 TMDB=N/A TVDB=81189 TVMAZE=N/A");
        let (_, any) = parse_answer(&json!({ "title": "x", "episodes": [] }));
        assert!(!any, "no movie and no series");
        let (_, any) = parse_answer(&json!({ "movie": { "tmdbId": 0, "imdbId": "" } }));
        assert!(!any, "only zero ids");
    }

    #[test]
    fn lookups_go_to_the_right_apps() {
        let cfg = RuntimeConfig {
            sonarr: vec!["http://s/?apikey=1".into()],
            radarr: vec!["http://r/?apikey=2".into()],
            ..RuntimeConfig::default()
        };
        assert_eq!(relevant_arr_instances(&cfg, MediaType::Episode), vec!["http://s/?apikey=1"]);
        assert_eq!(relevant_arr_instances(&cfg, MediaType::Movie), vec!["http://r/?apikey=2"]);
        assert_eq!(relevant_arr_instances(&cfg, MediaType::Video).len(), 2);
        assert!(relevant_arr_instances(&cfg, MediaType::Audio).is_empty());
        assert_eq!(lookup_title("abc-Movie.2020.1080p-GRP", MediaType::Movie), "Movie.2020.1080p-GRP");
        let parsed = ParsedMedia {
            movie: Some(ExternalIds { imdb_id: Some("tt1".into()), tmdb_id: Some("2".into()), ..ExternalIds::default() }),
            ..ParsedMedia::default()
        };
        let ids = relevant_arr_ids(IdCaps { imdb: true, ..IdCaps::default() }, IdCaps::default(), &parsed);
        assert_eq!((ids.imdbid.as_deref(), ids.tmdbid.as_deref()), (Some("tt1"), None));
        assert!(ids.any());
        assert!(!IdParams::default().any());
    }
}
