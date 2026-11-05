//! What a search asks for and what it finds: [`Query`] and [`Release`], shared by every kind of indexer and every
//! API Lantern answers (Torznab, Newznab, the Prowlarr API).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The kind of search (Torznab's `t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SearchKind {
    /// Any words (`t=search`).
    #[default]
    Search,
    /// TV: show, season, episode (`t=tvsearch`).
    TvSearch,
    /// Movies (`t=movie`).
    MovieSearch,
    /// Music: artist, album (`t=music`).
    MusicSearch,
    /// Books: author, title (`t=book`).
    BookSearch,
}

impl SearchKind {
    /// The definition's mode name (`tv-search`).
    pub fn mode(self) -> &'static str {
        match self {
            SearchKind::Search => "search",
            SearchKind::TvSearch => "tv-search",
            SearchKind::MovieSearch => "movie-search",
            SearchKind::MusicSearch => "music-search",
            SearchKind::BookSearch => "book-search",
        }
    }

    /// Torznab's `t` value (`tvsearch`).
    pub fn torznab(self) -> &'static str {
        match self {
            SearchKind::Search => "search",
            SearchKind::TvSearch => "tvsearch",
            SearchKind::MovieSearch => "movie",
            SearchKind::MusicSearch => "music",
            SearchKind::BookSearch => "book",
        }
    }

    /// From Torznab's `t` or the Prowlarr API's `type` (`tvsearch`, `tv-search`, `movie`, `moviesearch` …).
    pub fn parse(t: &str) -> Option<SearchKind> {
        Some(match t.to_lowercase().replace('-', "").as_str() {
            "search" | "" => SearchKind::Search,
            "tvsearch" | "tv" => SearchKind::TvSearch,
            "movie" | "moviesearch" => SearchKind::MovieSearch,
            "music" | "musicsearch" | "audio" => SearchKind::MusicSearch,
            "book" | "booksearch" => SearchKind::BookSearch,
            _ => return None,
        })
    }
}

/// One search.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    /// The kind of search.
    pub kind: SearchKind,
    /// The words (`q`).
    pub keywords: String,
    /// Standard categories (empty: all).
    pub categories: Vec<u32>,
    /// `tt0111161` or `111161`.
    pub imdbid: String,
    /// TheTVDB id.
    pub tvdbid: String,
    /// TMDb id.
    pub tmdbid: String,
    /// TVmaze id.
    pub tvmazeid: String,
    /// Trakt id.
    pub traktid: String,
    /// Douban id.
    pub doubanid: String,
    /// Season number or year.
    pub season: String,
    /// Episode number, or `MM/DD` for daily shows.
    pub ep: String,
    /// Artist.
    pub artist: String,
    /// Album.
    pub album: String,
    /// Label.
    pub label: String,
    /// Track.
    pub track: String,
    /// Genre.
    pub genre: String,
    /// Author.
    pub author: String,
    /// Book or movie title.
    pub title: String,
    /// Year.
    pub year: String,
    /// The most results to send back (0: the default).
    pub limit: usize,
    /// How many results to skip.
    pub offset: usize,
    /// An RSS fetch (no words): the newest releases.
    pub rss: bool,
}

impl Query {
    /// The IMDb id with its `tt` (`tt0111161`), "" when none.
    pub fn imdb_full(&self) -> String {
        let d = self.imdbid.trim().trim_start_matches("tt");
        if d.is_empty() {
            String::new()
        } else {
            format!("tt{:0>7}", d)
        }
    }

    /// The words a site gets: the keywords, then season/episode (`S01E02`) for a TV search.
    pub fn search_terms(&self) -> String {
        let mut s = self.keywords.trim().to_string();
        if self.kind == SearchKind::TvSearch {
            let ep = self.episode_text();
            if !ep.is_empty() {
                s = format!("{s} {ep}").trim().to_string();
            }
        }
        s
    }

    /// `S01E02`, `S01` or a daily date (`2026.11.05`), "" when no season was asked for.
    pub fn episode_text(&self) -> String {
        let season = self.season.trim();
        let ep = self.ep.trim();
        if season.is_empty() {
            return String::new();
        }
        if season.len() == 4 && ep.contains('/') {
            // daily: season is the year, ep is MM/DD
            let (m, d) = ep.split_once('/').unwrap_or(("", ""));
            return format!("{season}.{m:0>2}.{d:0>2}");
        }
        match (season.parse::<u32>(), ep.parse::<u32>()) {
            (Ok(s), Ok(e)) => format!("S{s:02}E{e:02}"),
            (Ok(s), Err(_)) if ep.is_empty() => format!("S{s:02}"),
            _ => format!("{season} {ep}").trim().to_string(),
        }
    }
}

/// Torrent or Usenet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// A torrent.
    #[default]
    Torrent,
    /// An NZB.
    Usenet,
}

/// One release a search found.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Release {
    /// The release's name.
    pub title: String,
    /// A stable id (the details page, else the download link).
    pub guid: String,
    /// The site's download link (sent to apps through Lantern, never directly).
    pub link: String,
    /// The release's page on the site.
    pub details: String,
    /// A magnet link, when the site gives one.
    pub magnet: String,
    /// The info hash.
    pub infohash: String,
    /// Bytes.
    pub size: Option<u64>,
    /// Number of files.
    pub files: Option<i64>,
    /// Times downloaded.
    pub grabs: Option<i64>,
    /// Seeders.
    pub seeders: Option<i64>,
    /// Leechers (peers less seeders).
    pub leechers: Option<i64>,
    /// When it was published.
    pub date: Option<DateTime<Utc>>,
    /// Standard categories (and site ones as 100000+).
    pub categories: Vec<u32>,
    /// IMDb id (number).
    pub imdb: Option<i64>,
    /// TheTVDB id.
    pub tvdb: Option<i64>,
    /// TMDb id.
    pub tmdb: Option<i64>,
    /// TVmaze id.
    pub tvmaze: Option<i64>,
    /// Douban id.
    pub douban: Option<i64>,
    /// Genres, comma separated.
    pub genre: String,
    /// Year.
    pub year: Option<i64>,
    /// A cover image.
    pub poster: String,
    /// A short description.
    pub description: String,
    /// How much of the download counts against the ratio (0 = freeleech).
    pub download_factor: Option<f64>,
    /// How much of the upload counts (2 = double upload).
    pub upload_factor: Option<f64>,
    /// The ratio the site wants seeded to.
    pub minimum_ratio: Option<f64>,
    /// Seconds the site wants it seeded.
    pub minimum_seed_time: Option<i64>,
    /// Torrent or Usenet.
    pub protocol: Protocol,
    /// The Lantern indexer id that found it.
    pub indexer_id: i64,
    /// That indexer's name.
    pub indexer: String,
}

impl Release {
    /// Whether nothing counts against the ratio.
    pub fn freeleech(&self) -> bool {
        self.download_factor == Some(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_words() {
        let q = Query { kind: SearchKind::TvSearch, keywords: "Show".into(), season: "1".into(), ep: "2".into(), ..Query::default() };
        assert_eq!(q.search_terms(), "Show S01E02");
        let q = Query { kind: SearchKind::TvSearch, keywords: "Show".into(), season: "2026".into(), ep: "11/5".into(), ..Query::default() };
        assert_eq!(q.episode_text(), "2026.11.05");
        let q = Query { imdbid: "111161".into(), ..Query::default() };
        assert_eq!(q.imdb_full(), "tt0111161");
        assert_eq!(SearchKind::parse("tvsearch"), Some(SearchKind::TvSearch));
    }
}
