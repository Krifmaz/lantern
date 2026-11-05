// Ported from cross-seed v6.13.9 (src/torznab.ts, src/indexers.ts), Apache-2.0.
//! How a searchee is searched on an indexer: what the indexer can do (cross-seed's `Caps`, read from Lantern's
//! capabilities of the indexer), whether it suits the searchee's media ([`indexer_does_support_media_type`]), the
//! searches sent ([`create_search_queries`]: a TV search with season and episode, a movie search, an anime or
//! video search, or plain words; with Sonarr's or Radarr's ids when the indexer takes them), the text that groups
//! searchees sharing one search ([`get_search_string`]), and the RSS paging of [`rss_page_stop`].
//!
//! cross-seed asks Torznab addresses; Lantern asks its own indexers the same searches.

use super::arr::{relevant_arr_ids, IdCaps, IdParams, ParsedMedia};
use super::constants::{MediaType, CALIBRE_INDEXNUM_REGEX, EP_REGEX, SEASON_REGEX};
use super::jsre::{captures, replace_first};
use super::searchee::{get_media_type, File, Searchee};
use super::utils::{
    clean_book_and_audio_title, clean_title, extract_int, get_anime_queries, get_video_queries, reformat_title_for_searching,
    strip_extension,
};
use crate::indexers::capabilities::Capabilities;
use crate::indexers::release::{Query, SearchKind};

/// `IndexerCategories`: the kinds of categories an indexer has, by their names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Categories {
    /// A category named with `tv`.
    pub tv: bool,
    /// With `movie`.
    pub movie: bool,
    /// With `anime`.
    pub anime: bool,
    /// With `xxx`.
    pub xxx: bool,
    /// With `audio`.
    pub audio: bool,
    /// With `book`.
    pub book: bool,
    /// Another standard category (not 8000 to 8999).
    pub additional: bool,
}

/// `Caps`: what an indexer can search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// `search`.
    pub search: bool,
    /// `tv-search`.
    pub tv_search: bool,
    /// `movie-search`.
    pub movie_search: bool,
    /// `music-search`.
    pub music_search: bool,
    /// `audio-search` (Lantern's music search).
    pub audio_search: bool,
    /// `book-search`.
    pub book_search: bool,
    /// The ids a movie search takes.
    pub movie_id_search: IdCaps,
    /// The ids a TV search takes.
    pub tv_id_search: IdCaps,
    /// The categories.
    pub categories: Categories,
    /// `limits.max`.
    pub limits_max: usize,
}

/// `ALL_CAPS`: every search with every id (used for [`get_search_string`]).
pub const ALL_CAPS: Caps = Caps {
    search: true,
    tv_search: true,
    movie_search: true,
    music_search: true,
    audio_search: true,
    book_search: true,
    movie_id_search: IdCaps { tvdb: true, tmdb: true, imdb: true, tvmaze: true },
    tv_id_search: IdCaps { tvdb: true, tmdb: true, imdb: true, tvmaze: true },
    categories: Categories { tv: true, movie: true, anime: true, xxx: true, audio: true, book: true, additional: true },
    limits_max: 100,
};

/// `getCatCaps`: the kinds of `(id, name)` categories.
pub fn cat_caps(categories: &[(u32, String)]) -> Categories {
    let mut c = Categories::default();
    for (id, name) in categories {
        let name = name.to_lowercase();
        let mut additional = true;
        for (key, slot) in [
            ("movie", &mut c.movie),
            ("tv", &mut c.tv),
            ("anime", &mut c.anime),
            ("xxx", &mut c.xxx),
            ("audio", &mut c.audio),
            ("book", &mut c.book),
        ] {
            if name.contains(key) {
                *slot = true;
                additional = false;
            }
        }
        if additional && *id < 100_000 && !(8000..=8999).contains(id) {
            c.additional = true;
        }
    }
    c
}

/// `parseTorznabCaps`, from Lantern's capabilities of an indexer.
pub fn caps_of(c: &Capabilities) -> Caps {
    let ids = |params: &[&str]| IdCaps {
        tvdb: params.contains(&"tvdbid"),
        tmdb: params.contains(&"tmdbid"),
        imdb: params.contains(&"imdbid"),
        tvmaze: params.contains(&"tvmazeid"),
    };
    let cats: Vec<(u32, String)> = c.categories.list(false).into_iter().map(|c| (c.id, c.name)).collect();
    Caps {
        search: c.search_available(),
        tv_search: !c.tv_params.is_empty(),
        movie_search: !c.movie_params.is_empty(),
        music_search: !c.music_params.is_empty(),
        audio_search: !c.music_params.is_empty(),
        book_search: !c.book_params.is_empty(),
        movie_id_search: ids(&c.movie_params),
        tv_id_search: ids(&c.tv_params),
        categories: cat_caps(&cats),
        limits_max: c.limits_max.unwrap_or(100).max(1) as usize,
    }
}

/// `indexerDoesSupportMediaType`.
pub fn indexer_does_support_media_type(media: MediaType, i: &Caps) -> bool {
    let c = &i.categories;
    match media {
        MediaType::Episode | MediaType::Season => i.tv_search || c.xxx,
        MediaType::Movie => i.movie_search || c.xxx,
        MediaType::Anime | MediaType::Video => i.movie_search || i.tv_search || c.movie || c.tv || c.anime || c.xxx,
        MediaType::Audio => i.audio_search || i.music_search || c.audio,
        MediaType::Book => i.book_search || c.book,
        MediaType::Other => c.additional,
    }
}

/// cross-seed's `Query`: one search to send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// `t`.
    pub t: SearchKind,
    /// `q` (`None`: searched by ids only).
    pub q: Option<String>,
    /// `season`: its number, or the year of a daily show.
    pub season: Option<String>,
    /// `ep`: its number, or `MM/DD` for a daily show.
    pub ep: Option<String>,
    /// The ids.
    pub ids: IdParams,
}

impl SearchQuery {
    /// The search as Lantern's indexers take it, `limit` results at most from `offset`.
    pub fn to_query(&self, limit: usize, offset: usize) -> Query {
        Query {
            kind: self.t,
            keywords: self.q.clone().unwrap_or_default(),
            season: self.season.clone().unwrap_or_default(),
            ep: self.ep.clone().unwrap_or_default(),
            tvdbid: self.ids.tvdbid.clone().unwrap_or_default(),
            tmdbid: self.ids.tmdbid.clone().unwrap_or_default(),
            imdbid: self.ids.imdbid.clone().unwrap_or_default(),
            tvmazeid: self.ids.tvmazeid.clone().unwrap_or_default(),
            limit,
            offset,
            ..Query::default()
        }
    }
}

/// The words for an episode, season or film search: the title to search for, or the stem when that is empty.
fn words(stem: &str) -> String {
    let q = reformat_title_for_searching(stem);
    if q.is_empty() {
        stem.to_string()
    } else {
        q
    }
}

/// `createTorznabSearchQueries`: the searches for `s` on an indexer with `caps`, with the ids of `parsed` it takes.
pub fn create_search_queries(s: &Searchee, media: MediaType, caps: &Caps, parsed: Option<&ParsedMedia>) -> Vec<SearchQuery> {
    let stem = strip_extension(&s.title);
    let ids = parsed.map(|p| relevant_arr_ids(caps.movie_id_search, caps.tv_id_search, p)).unwrap_or_default();
    let use_ids = ids.any();
    let q = |stem: &str| if use_ids { None } else { Some(words(stem)) };
    if media == MediaType::Episode && caps.tv_search {
        if let Some(c) = captures(&EP_REGEX, &stem) {
            let g = |n: &str| c.name(n).map(|m| m.as_str().to_string());
            let season = match g("season") {
                Some(sn) => Some(extract_int(&sn).to_string()),
                None => g("year"),
            };
            let ep = match g("episode") {
                Some(e) => Some(extract_int(&e).to_string()),
                None => Some(format!("{}/{}", g("month").unwrap_or_default(), g("day").unwrap_or_default())),
            };
            return vec![SearchQuery { t: SearchKind::TvSearch, q: q(&stem), season, ep, ids }];
        }
    } else if media == MediaType::Season && caps.tv_search {
        if let Some(c) = captures(&SEASON_REGEX, &stem) {
            let season = c.name("season").map(|m| extract_int(m.as_str()).to_string());
            return vec![SearchQuery { t: SearchKind::TvSearch, q: q(&stem), season, ep: None, ids }];
        }
    } else if media == MediaType::Movie && caps.movie_search {
        return vec![SearchQuery { t: SearchKind::MovieSearch, q: q(&stem), season: None, ep: None, ids }];
    }
    if use_ids && caps.tv_search && parsed.is_some_and(|p| p.series.is_some()) {
        let eps = &parsed.map(|p| p.episodes.clone()).unwrap_or_default();
        let season = eps.first().map(|e| e.0.to_string());
        let ep = if eps.len() == 1 { Some(eps[0].1.to_string()) } else { None };
        return vec![SearchQuery { t: SearchKind::TvSearch, q: None, season, ep, ids }];
    } else if use_ids && caps.movie_search && parsed.is_some_and(|p| p.movie.is_some()) {
        return vec![SearchQuery { t: SearchKind::MovieSearch, q: None, season: None, ep: None, ids }];
    } else if media == MediaType::Anime {
        return get_anime_queries(&stem).into_iter().map(|a| SearchQuery { q: Some(a), ..SearchQuery::default() }).collect();
    } else if media == MediaType::Video {
        return get_video_queries(&stem).into_iter().map(|v| SearchQuery { q: Some(v), ..SearchQuery::default() }).collect();
    } else if media == MediaType::Book && caps.book_search {
        let q = clean_book_and_audio_title(&replace_first(&CALIBRE_INDEXNUM_REGEX, &stem, ""));
        return vec![SearchQuery { q: Some(q), ..SearchQuery::default() }];
    } else if media == MediaType::Audio && caps.audio_search {
        return vec![SearchQuery { q: Some(clean_book_and_audio_title(&stem)), ..SearchQuery::default() }];
    }
    let query = clean_title(&stem);
    vec![SearchQuery { q: Some(if query.is_empty() { stem } else { query }), ..SearchQuery::default() }]
}

/// `getSearchString`: the first search's words, season and episode, in small letters; searchees with the same
/// one share their search.
pub fn get_search_string(s: &Searchee) -> String {
    let media = get_media_type(s);
    let Some(p) = create_search_queries(s, media, &ALL_CAPS, None).into_iter().next() else { return String::new() };
    let season = p.season.map(|v| format!(".S{v}")).unwrap_or_default();
    let ep = p.ep.map(|v| format!(".E{v}")).unwrap_or_default();
    format!("{}{season}{ep}", p.q.unwrap_or_else(|| "undefined".into())).to_lowercase()
}

/// `estimateSearchString`: the search a searchee of this name would get.
pub fn estimate_search_string(name: &str) -> String {
    let s = Searchee {
        name: name.into(),
        title: name.into(),
        length: 1,
        files: vec![File { name: "a.mkv".into(), path: "a.mkv".into(), length: 1 }],
        ..Searchee::default()
    };
    get_search_string(&s)
}

/// One RSS candidate: its guid and when it was published (milliseconds since 1970).
#[derive(Debug, Clone, PartialEq)]
pub struct Paged {
    /// `guid`.
    pub guid: String,
    /// `pubDate`.
    pub pub_date: f64,
}

/// `rssPager`'s choice on one page (sorted newest first): how many of its candidates are new, and whether paging
/// goes on. `page_back_until` is the first page's newest date less the time since the last run; `last_seen_guid`
/// is where the last run stopped.
pub fn rss_page_stop(page: &[Paged], last_seen_guid: Option<&str>, page_back_until: f64) -> (usize, bool) {
    let mut new = Vec::new();
    let mut found = false;
    for c in page {
        if Some(c.guid.as_str()) == last_seen_guid {
            found = true;
            break;
        }
        new.push(c);
    }
    if !found {
        new.retain(|c| c.pub_date >= page_back_until);
    }
    let n = new.len();
    (n, n > 0 && n == page.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(title: &str, file: &str) -> Searchee {
        Searchee {
            name: title.into(),
            title: title.into(),
            length: 10,
            files: vec![File { name: file.into(), path: file.into(), length: 10 }],
            ..Searchee::default()
        }
    }

    #[test]
    fn search_strings_as_cross_seed_makes_them() {
        assert_eq!(estimate_search_string("The.Show.S01E05.1080p.WEB-GRP"), "the show.s1.e5");
        assert_eq!(estimate_search_string("The.Show.S01.1080p.WEB-GRP"), "the show.s1");
        assert_eq!(estimate_search_string("The.Show.2024.05.06.1080p.WEB-GRP"), "the show.s2024.e05/06");
        assert_eq!(estimate_search_string("A.Movie.2020.1080p.BluRay-GRP"), "a movie 2020");
    }

    #[test]
    fn queries_follow_what_the_indexer_can_do() {
        let ep = s("The.Show.S01E05.1080p.WEB-GRP", "The.Show.S01E05.1080p.WEB-GRP.mkv");
        let tv = Caps { tv_search: true, ..Caps { search: true, ..no_caps() } };
        let q = create_search_queries(&ep, MediaType::Episode, &tv, None);
        assert_eq!(
            q,
            vec![SearchQuery {
                t: SearchKind::TvSearch,
                q: Some("The Show".into()),
                season: Some("1".into()),
                ep: Some("5".into()),
                ids: IdParams::default()
            }]
        );
        // without a TV search: the plain words
        let q = create_search_queries(&ep, MediaType::Episode, &no_caps(), None);
        assert_eq!((q[0].t, q[0].q.as_deref()), (SearchKind::Search, Some("The Show S01E05 1080p WEB-GRP")));
        // with Sonarr's ids and an indexer that takes them: ids instead of words
        let parsed = ParsedMedia {
            series: Some(super::super::arr::ExternalIds { tvdb_id: Some("81189".into()), ..Default::default() }),
            episodes: vec![(1, 5)],
            ..ParsedMedia::default()
        };
        let ids = Caps { tv_id_search: IdCaps { tvdb: true, ..IdCaps::default() }, ..tv };
        let q = create_search_queries(&ep, MediaType::Episode, &ids, Some(&parsed));
        assert_eq!((q[0].q.clone(), q[0].ids.tvdbid.as_deref()), (None, Some("81189")));
        let lq = q[0].to_query(100, 0);
        assert_eq!((lq.kind, lq.season.as_str(), lq.ep.as_str(), lq.tvdbid.as_str()), (SearchKind::TvSearch, "1", "5", "81189"));
    }

    fn no_caps() -> Caps {
        Caps {
            search: true,
            tv_search: false,
            movie_search: false,
            music_search: false,
            audio_search: false,
            book_search: false,
            movie_id_search: IdCaps::default(),
            tv_id_search: IdCaps::default(),
            categories: Categories::default(),
            limits_max: 100,
        }
    }

    #[test]
    fn media_types_an_indexer_suits() {
        let cats = cat_caps(&[(5000, "TV".into()), (2000, "Movies".into()), (8000, "Other".into()), (100001, "Site".into())]);
        assert_eq!((cats.tv, cats.movie, cats.additional), (true, true, false));
        assert!(cat_caps(&[(7000, "Books".into()), (1000, "Console".into())]).additional);
        let plain = Caps { categories: cats, ..no_caps() };
        assert!(!indexer_does_support_media_type(MediaType::Season, &plain), "a season needs a TV search");
        assert!(indexer_does_support_media_type(MediaType::Video, &plain));
        assert!(!indexer_does_support_media_type(MediaType::Other, &plain));
        assert!(indexer_does_support_media_type(MediaType::Season, &Caps { tv_search: true, ..plain }));
    }

    #[test]
    fn rss_paging_stops_at_the_last_seen_guid_or_the_last_run() {
        let page = |gs: &[(&str, f64)]| gs.iter().map(|(g, d)| Paged { guid: g.to_string(), pub_date: *d }).collect::<Vec<_>>();
        let p = page(&[("c", 30.0), ("b", 20.0), ("a", 10.0)]);
        assert_eq!(rss_page_stop(&p, Some("b"), 0.0), (1, false), "up to the guid seen last time");
        assert_eq!(rss_page_stop(&p, None, 15.0), (2, false), "back to the last run");
        assert_eq!(rss_page_stop(&p, None, 0.0), (3, true), "a full page of new ones: the next page");
        assert_eq!(rss_page_stop(&p, Some("c"), 0.0), (0, false));
    }
}
