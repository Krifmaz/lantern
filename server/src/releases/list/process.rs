// Ported from autobrr v1.90.0 (internal/list/process_arr_*.go, process_list_*.go, provider/*), GPL-2.0-or-later.
//! Reading a list into the filter fields it fills: the *arrs' movies, series, albums, books and leagues, and
//! the MDBList, Trakt, Metacritic, Steam, AniList and plain text lists. Each list type fills the fields autobrr's
//! does: titles go to `shows` (or `match_releases` when the list matches releases), Lidarr's to `albums` and
//! `artists`, Readarr's and Steam's to `match_releases`, Metacritic's always to `match_releases`.

use super::title::{excluded_by_tags, process_title, Titles};
use super::List;
use crate::releases::clients::{arr, req_err};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

/// The filter fields a list fills; `None` is left as it is (autobrr's `FilterUpdate`, as lists use it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterUpdate {
    /// The shows (movies and series).
    pub shows: Option<String>,
    /// The release names.
    pub match_releases: Option<String>,
    /// The albums.
    pub albums: Option<String>,
    /// The artists.
    pub artists: Option<String>,
}

impl FilterUpdate {
    /// `joined` in `shows`, or in `match_releases` with `shows` emptied when the list matches releases.
    pub fn titles(joined: String, match_release: bool) -> FilterUpdate {
        if match_release {
            FilterUpdate { shows: Some(String::new()), match_releases: Some(joined), ..FilterUpdate::default() }
        } else {
            FilterUpdate { shows: Some(joined), ..FilterUpdate::default() }
        }
    }

    /// The fields as a partial filter update's body.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (k, v) in
            [("shows", &self.shows), ("match_releases", &self.match_releases), ("albums", &self.albums), ("artists", &self.artists)]
        {
            if let Some(v) = v {
                m.insert(k.into(), json!(v));
            }
        }
        Value::Object(m)
    }
}

/// Text field `k` of `v` (empty when missing or not text).
fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Whether `v` is monitored.
fn monitored(v: &Value) -> bool {
    v.get("monitored").and_then(Value::as_bool).unwrap_or(false)
}

/// The tag ids of `v`.
fn tag_ids(v: &Value) -> Vec<i64> {
    v.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}

/// The alternate titles of `v`.
fn alternate_titles(v: &Value) -> Vec<String> {
    v.get("alternateTitles").and_then(Value::as_array).map(|a| a.iter().map(|t| s(t, "title")).collect()).unwrap_or_default()
}

/// Whether item `v` is on the list: monitored (or the list takes unmonitored ones) and not left out by its tags.
fn wanted(list: &List, tags: &[(i64, String)], v: &Value, check_tags: bool) -> bool {
    if !list.should_process_item(monitored(v)) {
        return false;
    }
    !(check_tags && excluded_by_tags(list.tags_include(), list.tags_exclude(), tags, &tag_ids(v)))
}

/// The update of a set of titles (`None` when there are none).
fn of_titles(ts: &Titles, match_release: bool) -> Option<FilterUpdate> {
    (!ts.is_empty()).then(|| FilterUpdate::titles(ts.filter_string(), match_release))
}

/// Radarr's movies: title and original title, and the alternate titles when asked (autobrr's
/// `RadarrProcessor.process`).
pub fn radarr(list: &List, movies: &[Value], tags: &[(i64, String)]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for m in movies.iter().filter(|m| wanted(list, tags, m, true)) {
        ts.add_title(&s(m, "title"));
        ts.add_title(&s(m, "originalTitle"));
        if list.include_alternate_titles {
            for t in alternate_titles(m) {
                ts.add_title(&t);
            }
        }
    }
    of_titles(&ts, list.match_release)
}

/// Sonarr's series, and the alternate titles when asked (autobrr's `SonarrProcessor.process`).
pub fn sonarr(list: &List, shows: &[Value], tags: &[(i64, String)]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for m in shows.iter().filter(|m| wanted(list, tags, m, true)) {
        ts.add_title(&s(m, "title"));
        if list.include_alternate_titles {
            for t in alternate_titles(m) {
                ts.add_title(&t);
            }
        }
    }
    of_titles(&ts, list.match_release)
}

/// Readarr's books, always as release names (autobrr's `ReadarrProcessor.process`).
pub fn readarr(list: &List, books: &[Value]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for b in books.iter().filter(|b| list.should_process_item(monitored(b))) {
        ts.add_title(&s(b, "title"));
    }
    (!ts.is_empty()).then(|| FilterUpdate { match_releases: Some(ts.filter_string()), ..FilterUpdate::default() })
}

/// Sportarr's leagues (autobrr's `SportarrProcessor.process`).
pub fn sportarr(list: &List, leagues: &[Value], tags: &[(i64, String)]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for l in leagues.iter().filter(|l| wanted(list, tags, l, true)) {
        ts.add_title(&s(l, "name"));
    }
    of_titles(&ts, list.match_release)
}

/// Whisparr's series (v2) or movies with their alternate titles when asked (v3) (autobrr's
/// `WhisparrProcessor.Process`).
pub fn whisparr(list: &List, items: &[Value], tags: &[(i64, String)], v3: bool) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for m in items.iter().filter(|m| wanted(list, tags, m, true)) {
        ts.add_title(&s(m, "title"));
        if v3 && list.include_alternate_titles {
            for t in alternate_titles(m) {
                ts.add_title(&t);
            }
        }
    }
    of_titles(&ts, list.match_release)
}

/// Lidarr's monitored albums of monitored artists (`artists` holds each album's artist, or `None` when it
/// couldn't be read): the albums and artists, or release names (autobrr's `LidarrProcessor.process`).
pub fn lidarr(list: &List, albums: &[(Value, Option<Value>)]) -> Option<FilterUpdate> {
    let mut titles = Vec::new();
    let mut artists: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for (album, artist) in albums {
        if !list.should_process_item(monitored(album)) {
            continue;
        }
        let Some(artist) = artist else { continue };
        if monitored(artist) {
            titles.extend(process_title(&s(album, "title"), list.match_release));
            let name = s(artist, "artistName");
            if seen.insert(name.clone()) {
                artists.push(name);
            }
        }
    }
    let processed: Vec<String> = titles.iter().flat_map(|t| process_title(t, list.match_release)).collect();
    if processed.is_empty() {
        return None;
    }
    let joined = processed.join(",");
    if list.match_release {
        return Some(FilterUpdate { match_releases: Some(joined), ..FilterUpdate::default() });
    }
    let artists: Vec<String> = artists.iter().flat_map(|a| process_title(a, false)).collect();
    Some(FilterUpdate { albums: Some(joined), artists: Some(artists.join(",")), ..FilterUpdate::default() })
}

/// MDBList's items; a movie's year joins its title when the list matches releases and asks for it (autobrr's
/// `MDBListProcessor.process`).
pub fn mdblist(list: &List, items: &[Value]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for it in items {
        let mut title = s(it, "title");
        let year = it.get("release_year").and_then(Value::as_i64).unwrap_or(0);
        if list.include_year && list.match_release && year > 0 && s(it, "mediatype") == "movie" {
            title = format!("{title}*{year}");
        }
        ts.add_title(&title);
    }
    of_titles(&ts, list.match_release)
}

/// Trakt's items: the item's, movie's and show's titles (autobrr's `TraktProcessor.process`).
pub fn trakt(list: &List, items: &[Value]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for it in items {
        for t in [
            s(it, "title"),
            it.get("movie").map(|m| s(m, "title")).unwrap_or_default(),
            it.get("show").map(|m| s(m, "title")).unwrap_or_default(),
        ] {
            if !t.is_empty() {
                ts.add_title(&t);
            }
        }
    }
    of_titles(&ts, list.match_release)
}

/// AniList's items: English and romaji titles and the synonyms (autobrr's `AnilistProcessor.process`).
pub fn anilist(list: &List, items: &[Value]) -> Option<FilterUpdate> {
    let mut ts = Titles::new(list.match_release);
    for it in items {
        ts.add_title(&s(it, "english"));
        ts.add_title(&s(it, "romaji"));
        for syn in it.get("synonyms").and_then(Value::as_array).into_iter().flatten() {
            ts.add_title(syn.as_str().unwrap_or(""));
        }
    }
    of_titles(&ts, list.match_release)
}

/// Metacritic's albums as `*artist*album*` release names, always; albums and artists emptied (autobrr's
/// `MetacriticProcessor.process`).
pub fn metacritic(albums: &[(String, String)]) -> FilterUpdate {
    let mut ts = Titles::new(true);
    for (artist, title) in albums {
        let artist = process_title(artist, true);
        let album = process_title(title, true);
        for a in &artist {
            for b in &album {
                ts.add_title(&format!("{a}{b}"));
            }
        }
    }
    FilterUpdate {
        albums: Some(String::new()),
        artists: Some(String::new()),
        match_releases: Some(ts.filter_string()),
        ..FilterUpdate::default()
    }
}

/// Steam's games as release names (autobrr's `SteamProcessor.process`). Go reads them in map order; Lantern by
/// key.
pub fn steam(list: &List, names: &[String]) -> Option<FilterUpdate> {
    let titles: Vec<String> = names.iter().flat_map(|n| process_title(n, list.match_release)).collect();
    (!titles.is_empty()).then(|| FilterUpdate { match_releases: Some(titles.join(",")), ..FilterUpdate::default() })
}

/// A plain text list, one title a line, as it is when the list says so (autobrr's
/// `PlaintextProcessor.process`).
pub fn plaintext(list: &List, body: &str) -> Option<FilterUpdate> {
    let mut titles = Vec::new();
    for line in body.split('\n') {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if list.skip_clean_sanitize {
            titles.push(t.to_string());
        } else {
            titles.extend(process_title(t, list.match_release));
        }
    }
    (!titles.is_empty()).then(|| FilterUpdate::titles(titles.join(","), list.match_release))
}

static TRAKT_SMART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:https?://)?(?:app\.|www\.)?trakt\.tv/lists/smart/view/([^/?#]+)").unwrap());
static TRAKT_USER_LIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:https?://)?(?:app\.|www\.)?trakt\.tv/users/([^/]+)/lists/([^/?#]+)").unwrap());
static TRAKT_WATCHLIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:https?://)?(?:app\.|www\.)?trakt\.tv/users/([^/]+)/watchlist").unwrap());

/// A Trakt web page's address as its API's (autobrr's `transformTraktURL`).
pub fn transform_trakt_url(raw: &str) -> String {
    if let Some(c) = TRAKT_SMART.captures(raw) {
        return format!("https://api.trakt.tv/smart-lists/{}/items", &c[1]);
    }
    if let Some(c) = TRAKT_USER_LIST.captures(raw) {
        return format!("https://api.trakt.tv/users/{}/lists/{}/items", &c[1], &c[2]);
    }
    if let Some(c) = TRAKT_WATCHLIST.captures(raw) {
        return format!("https://api.trakt.tv/users/{}/watchlist/items", &c[1]);
    }
    raw.to_string()
}

/// A list provider's answer.
pub struct Answer {
    /// The status.
    pub status: u16,
    /// The `Content-Type`.
    pub content_type: String,
    /// The body.
    pub body: String,
}

/// GETs `url` for a list provider, with `extra` headers and the list's `K=V` headers, in 15 seconds.
pub async fn get(url: &str, extra: &[(&str, &str)], headers: &[String]) -> Result<Answer, String> {
    let u = reqwest::Url::parse(url).map_err(|e| format!("could not make new request for URL: {url}: parse \"{url}\": {e}"))?;
    let http = crate::releases::clients::http(Duration::from_secs(15), false, false);
    let mut req = http.get(u);
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    for h in headers {
        let parts: Vec<&str> = h.split('=').collect();
        if parts.len() != 2 {
            continue;
        }
        if let (Ok(k), Ok(v)) =
            (reqwest::header::HeaderName::from_bytes(parts[0].as_bytes()), reqwest::header::HeaderValue::from_str(parts[1]))
        {
            req = req.header(k, v);
        }
    }
    let resp = req.send().await.map_err(|e| format!("failed to fetch titles from URL: {url}: {}", req_err("Get", url, &e)))?;
    let status = resp.status().as_u16();
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = resp.text().await.map_err(|e| format!("failed to read response body from URL: {url}: {e}"))?;
    Ok(Answer { status, content_type, body })
}

/// The JSON list a provider answered at `url`.
fn decode<T: serde::de::DeserializeOwned>(a: &Answer, url: &str) -> Result<T, String> {
    serde_json::from_str(&a.body).map_err(|e| format!("failed to decode JSON data from URL: {url}: {e}"))
}

/// A list of items from MDBList or AniList (`name` in the errors) (autobrr's mdblist and anilist `GetList`).
pub async fn get_items(name: &str, url: &str, headers: &[String]) -> Result<Vec<Value>, String> {
    if url.is_empty() {
        return Err(format!("no URL provided for {name}"));
    }
    let a = get(url, &[], headers).await?;
    if a.status != 200 {
        return Err(format!("failed to fetch titles from URL: {url}"));
    }
    decode(&a, url)
}

/// Trakt's items (autobrr's trakt `GetList`).
pub async fn get_trakt(url: &str, api_key: &str, headers: &[String]) -> Result<Vec<Value>, String> {
    if url.is_empty() {
        return Err("no URL provided for trakt".into());
    }
    let req_url = transform_trakt_url(url);
    let mut extra = vec![("trakt-api-version", "2")];
    if !api_key.is_empty() {
        extra.push(("trakt-api-key", api_key));
    }
    let a = get(&req_url, &extra, headers).await?;
    if a.status != 200 {
        return Err(format!("failed to fetch titles from URL: {url}"));
    }
    if !a.content_type.starts_with("application/json") {
        return Err(format!("invalid content type for URL: {req_url}, expected application/json got: {}", a.content_type));
    }
    decode(&a, &req_url)
}

/// Metacritic's albums as (artist, title) (autobrr's metacritic `GetList`).
pub async fn get_metacritic(url: &str, headers: &[String]) -> Result<Vec<(String, String)>, String> {
    if url.is_empty() {
        return Err("no URL provided for metacritic".into());
    }
    let a = get(url, &[], headers).await?;
    match a.status {
        200 => {}
        404 => return Err(format!("list or endpoint not found: {url}: list or endpoint not found")),
        _ => return Err(format!("failed to fetch titles from URL: {url}")),
    }
    if !a.content_type.starts_with("application/json") {
        return Err(format!("unexpected content type for URL: {url} expected application/json got {}", a.content_type));
    }
    let v: Value = decode(&a, url)?;
    Ok(v.get("albums").and_then(Value::as_array).into_iter().flatten().map(|al| (s(al, "artist"), s(al, "title"))).collect())
}

/// Steam's game names, by key (autobrr's steam `GetList`).
pub async fn get_steam(url: &str, headers: &[String]) -> Result<Vec<String>, String> {
    if url.is_empty() {
        return Err("no URL provided for steam".into());
    }
    let a = get(url, &[], headers).await?;
    if a.status != 200 {
        return Err(format!("failed to fetch titles from URL: {url}"));
    }
    let m: std::collections::BTreeMap<String, Value> = decode(&a, url)?;
    Ok(m.values().map(|i| s(i, "name")).collect())
}

/// A plain text list from a `file://` path or an http(s) address that answers `text/plain` (autobrr's
/// `PlaintextProcessor.Process`).
pub async fn get_plaintext(url: &str, headers: &[String]) -> Result<String, String> {
    if url.is_empty() {
        return Err("no URL provided for plaintext".into());
    }
    let parsed = url::Url::parse(url).ok();
    let scheme = parsed.as_ref().map(|u| u.scheme().to_string()).unwrap_or_default();
    match (scheme.as_str(), parsed) {
        ("file", Some(u)) => {
            let path =
                u.to_file_path().unwrap_or_else(|_| percent_encoding::percent_decode_str(u.path()).decode_utf8_lossy().to_string().into());
            std::fs::read_to_string(&path).map_err(|e| format!("failed to read file: {}: {e}", path.display()))
        }
        ("http" | "https", _) => {
            let a = get(url, &[], headers).await?;
            if a.status != 200 {
                return Err(format!("failed to fetch list from URL: {url}"));
            }
            if !a.content_type.starts_with("text/plain") {
                return Err(format!("unexpected content type for URL: {url} expected text/plain got {}", a.content_type));
            }
            Ok(a.body)
        }
        (s, _) => Err(format!("unsupported URL scheme: {s}")),
    }
}

/// The *arr's tags when the list looks at tags; none when they can't be read (autobrr ignores that).
async fn tags_for(list: &List, c: &arr::Client) -> Vec<(i64, String)> {
    if list.tags_include().is_empty() && list.tags_exclude().is_empty() {
        return Vec::new();
    }
    c.get_tags().await.unwrap_or_default()
}

/// Reads *arr list `list` through `c` (each type's `Process`).
pub async fn arr_list(list: &List, c: &arr::Client) -> Result<Option<FilterUpdate>, String> {
    let wrap = |what: &'static str| move |e: String| format!("{what}: {e}");
    match list.kind.as_str() {
        "RADARR" => {
            let movies: Vec<Value> = c.get_json("movie").await.map_err(wrap("could not get tags"))?;
            Ok(radarr(list, &movies, &tags_for(list, c).await))
        }
        "SONARR" => {
            let shows: Vec<Value> = c.get_json("series").await.map_err(wrap("could not get tags"))?;
            Ok(sonarr(list, &shows, &tags_for(list, c).await))
        }
        "READARR" => {
            let books: Vec<Value> = c.get_json("book").await.map_err(wrap("could not get tags"))?;
            Ok(readarr(list, &books))
        }
        "SPORTARR" => {
            let leagues: Vec<Value> = c.get_json("leagues").await.map_err(wrap("could not get leagues"))?;
            Ok(sportarr(list, &leagues, &tags_for(list, c).await))
        }
        "LIDARR" => {
            let albums: Vec<Value> = c.get_json("album").await.map_err(wrap("could not get tags"))?;
            let mut with_artists = Vec::with_capacity(albums.len());
            for a in albums {
                let artist = if list.should_process_item(monitored(&a)) {
                    let id = a.get("artistId").and_then(Value::as_i64).unwrap_or(0);
                    c.get_json::<Value>(&format!("artist/{id}")).await.ok()
                } else {
                    None
                };
                with_artists.push((a, artist));
            }
            Ok(lidarr(list, &with_artists))
        }
        "WHISPARR" | "WHISPARR_V3" => {
            c.test().await?;
            let tags = tags_for(list, c).await;
            if list.kind == "WHISPARR" {
                let series: Vec<Value> = c.get_json("series").await.map_err(wrap("could not get series"))?;
                Ok(whisparr(list, &series, &tags, false))
            } else {
                let movies: Vec<Value> = c.get_json("movie").await.map_err(wrap("could not get movies"))?;
                Ok(whisparr(list, &movies, &tags, true))
            }
        }
        t => Err(format!("client {t} is not a whisparr client")),
    }
}

/// Reads list `list` that isn't an *arr's (each type's `Process`).
pub async fn other_list(list: &List) -> Result<Option<FilterUpdate>, String> {
    let headers = list.headers.clone().unwrap_or_default();
    match list.kind.as_str() {
        "MDBLIST" => Ok(mdblist(list, &get_items("mdblist", &list.url, &headers).await?)),
        "ANILIST" => {
            let items = get_items("anilist", &list.url, &headers).await.map_err(|e| format!("could not get anilist list: {e}"))?;
            Ok(anilist(list, &items))
        }
        "TRAKT" => {
            let items =
                get_trakt(&list.url, &list.api_key, &headers).await.map_err(|e| format!("could not make new request for URL: {e}"))?;
            Ok(trakt(list, &items))
        }
        "METACRITIC" => {
            let albums = get_metacritic(&list.url, &headers).await.map_err(|e| format!("could not make new request for URL: {e}"))?;
            Ok(Some(metacritic(&albums)))
        }
        "STEAM" => Ok(steam(list, &get_steam(&list.url, &headers).await?)),
        "PLAINTEXT" => Ok(plaintext(list, &get_plaintext(&list.url, &headers).await?)),
        t => Err(format!("unknown list type: {t}")),
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's process_list_metacritic_test.go and provider/trakt/trakt_test.go, and the processors' rules.
    use super::*;

    #[test]
    fn test_metacritic_processor_process() {
        let albums: Vec<(String, String)> = [
            ("Dinosaur Jr.", "There Near"),
            ("Little Big Town", "It's A Dying Art"),
            ("Mike D 5D", "Thank You"),
            ("Pulp", "Live!"),
            ("Angus & Julia Stone", "Karaoke Bar"),
            ("Arab Strap", "Half-Told Tales"),
        ]
        .iter()
        .map(|(a, t)| (a.to_string(), t.to_string()))
        .collect();
        assert_eq!(
            metacritic(&albums),
            FilterUpdate {
                artists: Some(String::new()),
                albums: Some(String::new()),
                match_releases: Some("*Angus?Julia?Stone*Karaoke?Bar*,*Arab?Strap*Half?Told?Tales*,*Dinosaur?Jr*There?Near*,*Little?Big?Town*It?s?A?Dying?Art*,*Little?Big?Town*Its?A?Dying?Art*,*Mike?D?5D*Thank?You*,*Pulp*Live*".into()),
                shows: None,
            }
        );
    }

    #[test]
    fn test_transform_trakt_url() {
        for (input, want) in [
            (
                "https://app.trakt.tv/lists/smart/view/series-ece60ab98f5dd862?mode=media",
                "https://api.trakt.tv/smart-lists/series-ece60ab98f5dd862/items",
            ),
            (
                "https://app.trakt.tv/lists/smart/view/series-ece60ab98f5dd862",
                "https://api.trakt.tv/smart-lists/series-ece60ab98f5dd862/items",
            ),
            (
                "https://trakt.tv/lists/smart/view/series-ece60ab98f5dd862?mode=media",
                "https://api.trakt.tv/smart-lists/series-ece60ab98f5dd862/items",
            ),
            (
                "https://api.trakt.tv/smart-lists/series-ece60ab98f5dd862/items",
                "https://api.trakt.tv/smart-lists/series-ece60ab98f5dd862/items",
            ),
            ("https://trakt.tv/users/johndoe/lists/my-cool-list", "https://api.trakt.tv/users/johndoe/lists/my-cool-list/items"),
            ("https://trakt.tv/users/johndoe/watchlist", "https://api.trakt.tv/users/johndoe/watchlist/items"),
            ("https://api.autobrr.com/lists/trakt/anticipated-tv", "https://api.autobrr.com/lists/trakt/anticipated-tv"),
        ] {
            assert_eq!(transform_trakt_url(input), want, "{input}");
        }
    }

    fn list(kind: &str, match_release: bool) -> List {
        List { kind: kind.into(), match_release, ..List::default() }
    }

    #[test]
    fn radarr_fills_shows_or_match_releases() {
        let movies = vec![
            json!({"title": "The Office (US)", "originalTitle": "The Office", "monitored": true, "tags": [1]}),
            json!({"title": "Skipped", "monitored": false}),
        ];
        let u = radarr(&list("RADARR", false), &movies, &[]).unwrap();
        assert_eq!(u.shows.as_deref(), Some("The?Office,The?Office*US,The?Office*US?"));
        assert_eq!(u.match_releases, None);
        let u = radarr(&list("RADARR", true), &movies, &[]).unwrap();
        assert_eq!((u.shows.as_deref(), u.match_releases.as_deref()), (Some(""), Some("*The?Office*,*The?Office*US*")));
        let mut l = list("RADARR", false);
        l.tags_excluded = Some(vec!["skip".into()]);
        assert_eq!(radarr(&l, &movies, &[(1, "skip".into())]), None);
        l.include_unmonitored = true;
        assert_eq!(radarr(&l, &movies, &[(1, "skip".into())]).unwrap().shows.as_deref(), Some("Skipped"));
    }

    #[test]
    fn lidarr_fills_albums_and_artists() {
        let albums = vec![
            (json!({"title": "Live!", "monitored": true, "artistId": 1}), Some(json!({"artistName": "Pulp", "monitored": true}))),
            (json!({"title": "Gone", "monitored": true, "artistId": 2}), Some(json!({"artistName": "Off", "monitored": false}))),
        ];
        let u = lidarr(&list("LIDARR", false), &albums).unwrap();
        assert_eq!((u.albums.as_deref(), u.artists.as_deref()), (Some("Live,Live,Live?"), Some("Pulp")));
        let u = lidarr(&list("LIDARR", true), &albums).unwrap();
        assert_eq!((u.match_releases.as_deref(), u.albums.as_deref()), (Some("*Live*"), None));
    }

    #[test]
    fn readarr_and_steam_fill_match_releases() {
        let u = readarr(&list("READARR", false), &[json!({"title": "Dune", "monitored": true})]).unwrap();
        assert_eq!((u.match_releases.as_deref(), u.shows.as_deref()), (Some("Dune"), None));
        let u = steam(&list("STEAM", false), &["Hades II".into()]).unwrap();
        assert_eq!(u.match_releases.as_deref(), Some("Hades?II"));
    }

    #[test]
    fn mdblist_adds_the_year_to_movies() {
        let mut l = list("MDBLIST", true);
        l.include_year = true;
        let items = vec![
            json!({"title": "Dune", "release_year": 2021, "mediatype": "movie"}),
            json!({"title": "Lost", "release_year": 2004, "mediatype": "show"}),
        ];
        assert_eq!(mdblist(&l, &items).unwrap().match_releases.as_deref(), Some("*Dune?2021*,*Lost*"));
    }

    #[test]
    fn plaintext_keeps_lines_as_they_are_when_asked() {
        let mut l = list("PLAINTEXT", false);
        assert_eq!(plaintext(&l, "Monsters, Inc.\n\n").unwrap().shows.as_deref(), Some("Monsters*Inc,Monsters*Inc?"));
        l.skip_clean_sanitize = true;
        assert_eq!(plaintext(&l, " Monsters, Inc. \n").unwrap().shows.as_deref(), Some("Monsters, Inc."));
        assert_eq!(plaintext(&l, "\n \n"), None);
    }

    #[test]
    fn trakt_and_anilist_read_every_title() {
        let items = vec![json!({"movie": {"title": "Dune"}}), json!({"show": {"title": "Lost"}, "title": ""})];
        assert_eq!(trakt(&list("TRAKT", false), &items).unwrap().shows.as_deref(), Some("Dune,Lost"));
        let items = vec![json!({"english": "Frieren", "romaji": "Sousou no Frieren", "synonyms": ["Frieren BtJE"]})];
        assert_eq!(anilist(&list("ANILIST", false), &items).unwrap().shows.as_deref(), Some("Frieren,Frieren?BtJE,Sousou?no?Frieren"));
    }
}
