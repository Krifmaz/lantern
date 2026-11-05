// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/IndexerSearch/NewznabRequest.cs, the criteria built in
// src/NzbDrone.Core/IndexerSearch/ReleaseSearchService.cs, IsRssSearch/IsIdSearch in
// src/NzbDrone.Core/IndexerSearch/Definitions/*.cs and ParseUtil.GetImdbId in src/NzbDrone.Core/Parser/ParseUtil.cs),
// GPL-3.0.
//! A Newznab/Torznab request as apps send it (`/{id}/api?t=…`): every parameter Prowlarr reads, read the way ASP.NET
//! reads them, and the search it becomes.

use super::release::{Query, SearchKind};
use regex::Regex;
use std::sync::LazyLock;

/// The `{imdbid:…}` style values a TV search's words may carry.
static TV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\{((?:imdbid:)(?<imdbid>[^{]+)|(?:rid:)(?<rid>[^{]+)|(?:tvdbid:)(?<tvdbid>[^{]+)|(?:tmdbid:)(?<tmdbid>[^{]+)|(?:tvmazeid:)(?<tvmazeid>[^{]+)|(?:doubanid:)(?<doubanid>[^{]+)|(?:season:)(?<season>[^{]+)|(?:episode:)(?<episode>[^{]+)|(?:year:)(?<year>[^{]+)|(?:genre:)(?<genre>[^{]+))\}").expect("a fixed pattern")
});
/// The same for a movie search.
static MOVIE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\{((?:imdbid:)(?<imdbid>[^{]+)|(?:doubanid:)(?<doubanid>[^{]+)|(?:tmdbid:)(?<tmdbid>[^{]+)|(?:traktid:)(?<traktid>[^{]+)|(?:year:)(?<year>[^{]+)|(?:genre:)(?<genre>[^{]+))\}").expect("a fixed pattern")
});
/// The same for a music search.
static MUSIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\{((?:artist:)(?<artist>[^{]+)|(?:album:)(?<album>[^{]+)|(?:track:)(?<track>[^{]+)|(?:label:)(?<label>[^{]+)|(?:year:)(?<year>[^{]+)|(?:genre:)(?<genre>[^{]+))\}").expect("a fixed pattern")
});
/// The same for a book search.
static BOOK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\{((?:author:)(?<author>[^{]+)|(?:publisher:)(?<publisher>[^{]+)|(?:title:)(?<title>[^{]+)|(?:year:)(?<year>[^{]+)|(?:genre:)(?<genre>[^{]+))\}").expect("a fixed pattern")
});
/// An IMDb id: digits, with or without `tt` (`ParseUtil.ImdbIdRegex`).
static IMDB_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:tt)?(\d{1,8})$").expect("a fixed pattern"));

/// A Newznab/Torznab request (`NewznabRequest`): text parameters are `None` when missing or empty, as ASP.NET binds
/// them.
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(missing_docs)]
pub struct NewznabRequest {
    /// The function: `caps`, `search`, `tvsearch`, `movie`, `music` or `book`.
    pub t: Option<String>,
    /// The words.
    pub q: Option<String>,
    /// The categories, comma separated.
    pub cat: Option<String>,
    /// IMDb id, with or without `tt`.
    pub imdbid: Option<String>,
    /// TMDb id.
    pub tmdbid: Option<i32>,
    /// `1` for every attribute (Prowlarr always sends them all).
    pub extended: Option<String>,
    /// The most results wanted.
    pub limit: Option<i32>,
    /// Results to skip (indexers that can't page answer nothing past the first page).
    pub offset: Option<i32>,
    /// Only releases at least this many days old.
    pub minage: Option<i32>,
    /// Only releases at most this many days old.
    pub maxage: Option<i32>,
    /// Only releases of at least this many bytes.
    pub minsize: Option<i64>,
    /// Only releases of at most this many bytes.
    pub maxsize: Option<i64>,
    /// TVRage id.
    pub rid: Option<i32>,
    /// TVmaze id.
    pub tvmazeid: Option<i32>,
    /// Trakt id.
    pub traktid: Option<i32>,
    /// TheTVDB id.
    pub tvdbid: Option<i32>,
    /// Douban id.
    pub doubanid: Option<i32>,
    /// Season (or a daily show's year).
    pub season: Option<i32>,
    /// Episode (or a daily show's `MM/DD`).
    pub ep: Option<String>,
    /// Album.
    pub album: Option<String>,
    /// Artist.
    pub artist: Option<String>,
    /// Record label.
    pub label: Option<String>,
    /// Track.
    pub track: Option<String>,
    /// Year.
    pub year: Option<i32>,
    /// Genre.
    pub genre: Option<String>,
    /// Book author.
    pub author: Option<String>,
    /// Book title.
    pub title: Option<String>,
    /// Book publisher.
    pub publisher: Option<String>,
    /// Sent by some apps; read and not used.
    pub configured: Option<String>,
    /// The app that asked (from its user agent), set by the server.
    pub source: Option<String>,
    /// The machine that asked, set by the server.
    pub host: Option<String>,
    /// Lantern's own address as the app reached it, set by the server.
    pub server: Option<String>,
}

/// A parameter that isn't a number where one is needed: ASP.NET's model-binding error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindError {
    /// The parameter's name.
    pub name: String,
    /// What was sent.
    pub value: String,
}

impl BindError {
    /// The message ASP.NET gives (`The value 'x' is not valid for tmdbid.`).
    pub fn message(&self) -> String {
        format!("The value '{}' is not valid for {}.", self.value, self.name)
    }
}

impl NewznabRequest {
    /// Reads the request from the query's pairs: names ignore case, the first value of a repeated one counts, an
    /// empty text value is none, and a number that isn't one is an error.
    pub fn from_pairs(pairs: &[(String, String)]) -> Result<NewznabRequest, BindError> {
        let get = |name: &str| pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
        let text = |name: &str| get(name).filter(|v| !v.trim().is_empty());
        let int = |name: &str| -> Result<Option<i32>, BindError> {
            match text(name) {
                None => Ok(None),
                Some(v) => v.trim().parse().map(Some).map_err(|_| BindError { name: name.into(), value: v }),
            }
        };
        let long = |name: &str| -> Result<Option<i64>, BindError> {
            match text(name) {
                None => Ok(None),
                Some(v) => v.trim().parse().map(Some).map_err(|_| BindError { name: name.into(), value: v }),
            }
        };
        Ok(NewznabRequest {
            t: text("t"),
            q: text("q"),
            cat: text("cat"),
            imdbid: text("imdbid"),
            tmdbid: int("tmdbid")?,
            extended: text("extended"),
            limit: int("limit")?,
            offset: int("offset")?,
            minage: int("minage")?,
            maxage: int("maxage")?,
            minsize: long("minsize")?,
            maxsize: long("maxsize")?,
            rid: int("rid")?,
            tvmazeid: int("tvmazeid")?,
            traktid: int("traktid")?,
            tvdbid: int("tvdbid")?,
            doubanid: int("doubanid")?,
            season: int("season")?,
            ep: text("ep"),
            album: text("album"),
            artist: text("artist"),
            label: text("label"),
            track: text("track"),
            year: int("year")?,
            genre: text("genre"),
            author: text("author"),
            title: text("title"),
            publisher: text("publisher"),
            configured: text("configured"),
            source: text("source"),
            host: text("host"),
            server: text("server"),
        })
    }

    /// Moves `{imdbid:…}`, `{season:…}` and the like out of the words into their parameters, for the request's
    /// kind of search (`QueryToParams`; Prowlarr's search API uses it).
    pub fn query_to_params(&mut self) {
        let Some(q) = self.q.clone().filter(|q| !q.trim().is_empty()) else { return };
        let int = |v: &str| v.trim().parse::<i32>().ok();
        let t = self.t.clone().unwrap_or_default();
        let re: &Regex = match t.as_str() {
            "tvsearch" => &TV_RE,
            "movie" => &MOVIE_RE,
            "music" => &MUSIC_RE,
            "book" => &BOOK_RE,
            _ => return,
        };
        let names = [
            "tvdbid",
            "tmdbid",
            "tvmazeid",
            "doubanid",
            "rid",
            "season",
            "imdbid",
            "traktid",
            "episode",
            "year",
            "genre",
            "artist",
            "album",
            "track",
            "label",
            "author",
            "title",
            "publisher",
        ];
        let found: Vec<(String, Vec<(&str, String)>)> = re
            .captures_iter(&q)
            .map(|c| (c[0].to_string(), names.iter().filter_map(|n| c.name(n).map(|v| (*n, v.as_str().to_string()))).collect()))
            .collect();
        let mut q = q.clone();
        for (whole, groups) in found {
            let g = |n: &str| groups.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone());
            if let Some(v) = g("tvdbid") {
                self.tvdbid = int(&v);
            }
            if let Some(v) = g("tmdbid") {
                self.tmdbid = int(&v);
            }
            if let Some(v) = g("tvmazeid") {
                self.tvmazeid = int(&v);
            }
            if let Some(v) = g("doubanid") {
                self.doubanid = int(&v);
            }
            if let Some(v) = g("rid") {
                self.rid = int(&v);
            }
            if let Some(v) = g("season") {
                self.season = int(&v);
            }
            if let Some(v) = g("imdbid") {
                self.imdbid = Some(v);
            }
            if let Some(v) = g("traktid") {
                self.traktid = int(&v);
            }
            if let Some(v) = g("episode") {
                self.ep = Some(v);
            }
            if let Some(v) = g("year") {
                self.year = int(&v);
            }
            if let Some(v) = g("genre") {
                self.genre = Some(v);
            }
            if let Some(v) = g("artist") {
                self.artist = Some(v);
            }
            if let Some(v) = g("album") {
                self.album = Some(v);
            }
            if let Some(v) = g("track") {
                self.track = Some(v);
            }
            if let Some(v) = g("label") {
                self.label = Some(v);
            }
            if let Some(v) = g("author") {
                self.author = Some(v);
            }
            if let Some(v) = g("title") {
                self.title = Some(v);
            }
            if let Some(v) = g("publisher") {
                self.publisher = Some(v);
            }
            q = q.replace(&whole, "");
            if t != "tvsearch" {
                q = q.trim().to_string();
            }
        }
        self.q = Some(q);
    }

    /// The search this request asks for (`ReleaseSearchService.Search`): only the parameters its kind of search
    /// takes, the IMDb id as seven digits or more, an RSS fetch when there are no words and no ids.
    pub fn to_query(&self) -> Query {
        let s = |v: &Option<String>| v.clone().unwrap_or_default();
        let n = |v: Option<i32>| v.map(|x| x.to_string()).unwrap_or_default();
        let t = self.t.clone().unwrap_or_default();
        let mut q = Query {
            kind: match t.as_str() {
                "movie" => SearchKind::MovieSearch,
                "music" => SearchKind::MusicSearch,
                "tvsearch" => SearchKind::TvSearch,
                "book" => SearchKind::BookSearch,
                _ => SearchKind::Search,
            },
            keywords: self.q.as_deref().map(str::trim).unwrap_or("").to_string(),
            categories: self.categories().into_iter().filter(|c| *c >= 0).map(|c| c as u32).collect(),
            limit: self.limit.unwrap_or(0).max(0) as usize,
            offset: self.offset.unwrap_or(0).max(0) as usize,
            min_age: self.minage.unwrap_or(0) as i64,
            max_age: self.maxage.unwrap_or(0) as i64,
            min_size: self.minsize.unwrap_or(0),
            max_size: self.maxsize.unwrap_or(0),
            ..Query::default()
        };
        let imdb = imdb_id(self.imdbid.as_deref()).map(|i| format!("{i:07}")).unwrap_or_default();
        let id_search = match q.kind {
            SearchKind::MovieSearch => {
                q.imdbid = imdb;
                q.tmdbid = n(self.tmdbid);
                q.traktid = n(self.traktid);
                q.doubanid = n(self.doubanid);
                q.year = n(self.year);
                q.genre = s(&self.genre);
                !q.imdbid.is_empty()
                    || !q.genre.is_empty()
                    || !q.tmdbid.is_empty()
                    || !q.traktid.is_empty()
                    || !q.doubanid.is_empty()
                    || !q.year.is_empty()
            }
            SearchKind::MusicSearch => {
                q.artist = s(&self.artist);
                q.album = s(&self.album);
                q.label = s(&self.label);
                q.genre = s(&self.genre);
                q.track = s(&self.track);
                q.year = n(self.year);
                !q.album.is_empty()
                    || !q.artist.is_empty()
                    || !q.label.is_empty()
                    || !q.genre.is_empty()
                    || !q.track.is_empty()
                    || !q.year.is_empty()
            }
            SearchKind::TvSearch => {
                q.imdbid = imdb;
                q.season = n(self.season);
                q.ep = s(&self.ep);
                q.tvdbid = n(self.tvdbid);
                q.traktid = n(self.traktid);
                q.tmdbid = n(self.tmdbid);
                q.doubanid = n(self.doubanid);
                q.rid = n(self.rid);
                q.tvmazeid = n(self.tvmazeid);
                q.year = n(self.year);
                q.genre = s(&self.genre);
                !q.imdbid.is_empty()
                    || !q.tvdbid.is_empty()
                    || !q.rid.is_empty()
                    || !q.traktid.is_empty()
                    || !q.tvmazeid.is_empty()
                    || !q.tmdbid.is_empty()
                    || !q.doubanid.is_empty()
            }
            SearchKind::BookSearch => {
                q.author = s(&self.author);
                q.title = s(&self.title);
                q.publisher = s(&self.publisher);
                q.year = n(self.year);
                q.genre = s(&self.genre);
                !q.author.is_empty() || !q.title.is_empty() || !q.publisher.is_empty() || !q.genre.is_empty() || !q.year.is_empty()
            }
            SearchKind::Search => false,
        };
        q.rss = q.keywords.trim().is_empty() && !id_search;
        q
    }

    /// The categories asked for (`cat`, comma separated; ones that aren't numbers are left out).
    pub fn categories(&self) -> Vec<i64> {
        self.cat.as_deref().unwrap_or("").split(',').filter(|c| !c.trim().is_empty()).filter_map(|c| c.trim().parse().ok()).collect()
    }
}

/// The number of an IMDb id (`tt0111161`, `0111161`, `111161` → 111161), `None` when it isn't one
/// (`ParseUtil.GetImdbId`).
pub fn imdb_id(value: Option<&str>) -> Option<i64> {
    let c = IMDB_RE.captures(value?)?;
    c[1].parse().ok()
}

/// The app that sent a request, from its user agent (`Sonarr/4.0 (…)` → `Sonarr`; a browser or nothing →
/// `Other`); `Prowlarr` when it carries `X-Prowlarr-Client` (`RequestExtensions.GetSource`, `UserAgentParser`).
pub fn source_of(user_agent: Option<&str>, prowlarr_client: bool) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?<agent>[a-z0-9]+)(?:/.+(?:\(.*\))?|$)").expect("a fixed pattern"));
    if prowlarr_client {
        return "Prowlarr".into();
    }
    let ua = user_agent.map(str::trim).filter(|u| !u.starts_with("Mozilla/5.0")).unwrap_or("");
    RE.captures(ua).and_then(|c| c.name("agent")).map(|m| m.as_str().to_string()).unwrap_or_else(|| "Other".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(pairs: &[(&str, &str)]) -> NewznabRequest {
        NewznabRequest::from_pairs(&pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>()).unwrap()
    }

    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/IndexerSearchTests/ReleaseSearchServiceFixture.cs):
    // should_normalize_imdbid_movie_search_criteria and should_normalize_imdbid_tv_search_criteria.
    #[test]
    fn should_normalize_imdbid_search_criteria() {
        for t in ["movie", "tvsearch"] {
            for (input, expected) in [
                ("tt0183790", "0183790"),
                ("0183790", "0183790"),
                ("183790", "0183790"),
                ("tt10001870", "10001870"),
                ("10001870", "10001870"),
            ] {
                assert_eq!(req(&[("t", t), ("imdbid", input)]).to_query().imdbid, expected, "{t} {input}");
            }
        }
    }

    #[test]
    fn reads_parameters_like_aspnet() {
        let r =
            req(&[("T", "tvsearch"), ("q", ""), ("Season", "2"), ("ep", "5"), ("tvdbid", "123"), ("cat", "5000,,5040"), ("q", "ignored")]);
        assert_eq!((r.t.as_deref(), r.q.as_deref(), r.season, r.tvdbid), (Some("tvsearch"), None, Some(2), Some(123)));
        assert_eq!(r.categories(), vec![5000, 5040]);
        let q = r.to_query();
        assert_eq!((q.kind, q.season.as_str(), q.ep.as_str(), q.rss), (SearchKind::TvSearch, "2", "5", false));
        let bad = NewznabRequest::from_pairs(&[("tmdbid".into(), "abc".into())]).unwrap_err();
        assert_eq!(bad.message(), "The value 'abc' is not valid for tmdbid.");
        // a basic search ignores ids; an empty search is an RSS fetch
        let q = req(&[("t", "search"), ("imdbid", "tt1")]).to_query();
        assert!(q.imdbid.is_empty() && q.rss);
        let q = req(&[("t", "book"), ("author", "A")]).to_query();
        assert!(!q.rss && q.author == "A");
    }

    #[test]
    fn query_to_params_takes_ids_out_of_the_words() {
        let mut r = req(&[("t", "tvsearch"), ("q", "Show {ImdbId:tt0111161}{Season:2}{Episode:3}")]);
        r.query_to_params();
        assert_eq!(
            (r.q.as_deref(), r.imdbid.as_deref(), r.season, r.ep.as_deref()),
            (Some("Show "), Some("tt0111161"), Some(2), Some("3"))
        );
        let mut r = req(&[("t", "movie"), ("q", "Film {tmdbid:42} {year:1999}")]);
        r.query_to_params();
        assert_eq!((r.q.as_deref(), r.tmdbid, r.year), (Some("Film"), Some(42), Some(1999)));
        let mut r = req(&[("t", "search"), ("q", "{imdbid:tt1}")]);
        r.query_to_params();
        assert_eq!(r.q.as_deref(), Some("{imdbid:tt1}"));
    }

    #[test]
    fn music_and_book_searches_take_every_prowlarr_parameter() {
        let q =
            req(&[("t", "music"), ("artist", "A"), ("album", "B"), ("label", "L"), ("track", "T"), ("year", "1999"), ("genre", "Rock")])
                .to_query();
        assert_eq!(q.kind, SearchKind::MusicSearch);
        assert_eq!(
            (q.artist.as_str(), q.album.as_str(), q.label.as_str(), q.track.as_str(), q.year.as_str(), q.genre.as_str()),
            ("A", "B", "L", "T", "1999", "Rock")
        );
        assert!(!q.rss);
        let q = req(&[("t", "book"), ("author", "A"), ("title", "T"), ("publisher", "P"), ("year", "2001"), ("genre", "Sci")]).to_query();
        assert_eq!(q.kind, SearchKind::BookSearch);
        assert_eq!(
            (q.author.as_str(), q.title.as_str(), q.publisher.as_str(), q.year.as_str(), q.genre.as_str()),
            ("A", "T", "P", "2001", "Sci")
        );
        // ids written into the words
        let mut r = req(&[("t", "music"), ("q", "{artist:A}{album:B}")]);
        r.query_to_params();
        assert_eq!((r.artist.as_deref(), r.album.as_deref()), (Some("A"), Some("B")));
        let mut r = req(&[("t", "book"), ("q", "{author:A}{title:T}")]);
        r.query_to_params();
        assert_eq!((r.author.as_deref(), r.title.as_deref()), (Some("A"), Some("T")));
        // nothing to search by is an RSS fetch
        assert!(req(&[("t", "music")]).to_query().rss && req(&[("t", "book")]).to_query().rss);
    }

    #[test]
    fn source_from_user_agent() {
        assert_eq!(source_of(Some("Sonarr/4.0.1 (ubuntu 22.04)"), false), "Sonarr");
        assert_eq!(source_of(Some("Mozilla/5.0 (X11)"), false), "Other");
        assert_eq!(source_of(None, false), "Other");
        assert_eq!(source_of(Some("curl"), false), "curl");
        assert_eq!(source_of(Some("x"), true), "Prowlarr");
    }
}
