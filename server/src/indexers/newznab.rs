//! Indexers that already speak Newznab (Usenet) or Torznab (torrents): Lantern asks their API and reads the RSS
//! answer. Usenet indexers (NZBgeek, DrunkenSlug, NZBFinder …) are added this way.

use super::http::{Fetch, Http};
use super::release::{Protocol, Query, Release, SearchKind};
use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

/// Well-known Usenet indexers: (id, name, API address).
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("nzbgeek", "NZBgeek", "https://api.nzbgeek.info"),
    ("drunkenslug", "DrunkenSlug", "https://drunkenslug.com"),
    ("nzbfinder", "NZBFinder", "https://nzbfinder.ws"),
    ("nzbplanet", "NZBPlanet", "https://api.nzbplanet.net"),
    ("nzbsu", "nzb.su", "https://api.nzb.su"),
    ("dognzb", "DOGnzb", "https://api.dognzb.cr"),
    ("althub", "altHUB", "https://api.althub.co.za"),
    ("tabularasa", "Tabula Rasa", "https://www.tabula-rasa.pw/api/v1"),
    ("ninjacentral", "NinjaCentral", "https://ninjacentral.co.za"),
    ("nzblife", "NZB.life", "https://api.nzb.life"),
    ("usenetcrawler", "Usenet Crawler", "https://www.usenet-crawler.com"),
    ("miatrix", "Miatrix", "https://www.miatrix.com"),
    ("scenenzbs", "SceneNZBs", "https://scenenzbs.com"),
];

/// A Newznab or Torznab indexer.
pub struct Newznab {
    /// Its name.
    pub name: String,
    /// The API's address without `/api`.
    pub base: String,
    /// The path of the API (`/api`).
    pub api_path: String,
    /// The API key.
    pub apikey: String,
    /// Torrent (Torznab) or Usenet (Newznab).
    pub protocol: Protocol,
    /// Extra query parameters (`&attrs=…`).
    pub extra: String,
    /// The HTTP client.
    pub http: Http,
}

impl Newznab {
    /// The API address with `params` and the key.
    fn url(&self, params: &[(&str, String)]) -> String {
        let mut u = format!("{}{}", self.base.trim_end_matches('/'), if self.api_path.is_empty() { "/api" } else { &self.api_path });
        u.push('?');
        let mut parts: Vec<String> = params
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={}", url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()))
            .collect();
        if !self.apikey.is_empty() {
            parts.push(format!("apikey={}", url::form_urlencoded::byte_serialize(self.apikey.as_bytes()).collect::<String>()));
        }
        if !self.extra.is_empty() {
            parts.push(self.extra.trim_start_matches('&').to_string());
        }
        u.push_str(&parts.join("&"));
        u
    }

    /// Checks the API answers its capabilities.
    pub async fn test(&self) -> Result<(), String> {
        let page = self.http.fetch(Fetch::get(&self.url(&[("t", "caps".into())]))).await?;
        if let Some(e) = api_error(&page.text) {
            return Err(format!("{}: {e}", self.name));
        }
        if !page.text.contains("<caps") {
            return Err(format!("{} didn't answer like a Newznab/Torznab API: check its address and API path", self.name));
        }
        Ok(())
    }

    /// Searches.
    pub async fn search(&self, q: &Query) -> Result<Vec<Release>, String> {
        let cats = q.categories.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let mut params: Vec<(&str, String)> =
            vec![("t", q.kind.torznab().to_string()), ("q", q.keywords.clone()), ("cat", cats), ("extended", "1".into())];
        match q.kind {
            SearchKind::TvSearch => {
                params.extend([
                    ("season", q.season.clone()),
                    ("ep", q.ep.clone()),
                    ("tvdbid", q.tvdbid.clone()),
                    ("imdbid", q.imdb_full()),
                    ("tvmazeid", q.tvmazeid.clone()),
                ]);
            }
            SearchKind::MovieSearch => params.extend([
                ("imdbid", q.imdb_full().trim_start_matches("tt").to_string()),
                ("tmdbid", q.tmdbid.clone()),
                ("year", q.year.clone()),
            ]),
            SearchKind::MusicSearch => params.extend([("artist", q.artist.clone()), ("album", q.album.clone())]),
            SearchKind::BookSearch => params.extend([("author", q.author.clone()), ("title", q.title.clone())]),
            SearchKind::Search => {}
        }
        if q.limit > 0 {
            params.push(("limit", q.limit.to_string()));
        }
        if q.offset > 0 {
            params.push(("offset", q.offset.to_string()));
        }
        let page = self.http.fetch(Fetch::get(&self.url(&params))).await?;
        if let Some(e) = api_error(&page.text) {
            return Err(format!("{}: {e}", self.name));
        }
        Ok(parse_rss(&page.text, self.protocol))
    }

    /// A release's NZB or torrent.
    pub async fn grab(&self, link: &str) -> Result<Vec<u8>, String> {
        let page = self.http.fetch(Fetch::get(link)).await?;
        if let Some(e) = api_error(&page.text) {
            return Err(format!("{}: {e}", self.name));
        }
        Ok(page.bytes)
    }
}

/// The description of a Newznab `<error code="100" description="Incorrect user credentials"/>`.
pub fn api_error(text: &str) -> Option<String> {
    let head = &text[..text.len().min(2000)];
    if !head.contains("<error") {
        return None;
    }
    let mut r = Reader::from_str(head);
    loop {
        match r.read_event() {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"error" => {
                let desc = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key.as_ref() == b"description")
                    .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()));
                return Some(desc.unwrap_or_else(|| "the indexer answered with an error".into()));
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// The releases of a Newznab/Torznab RSS answer.
pub fn parse_rss(text: &str, protocol: Protocol) -> Vec<Release> {
    let mut r = Reader::from_str(text);
    r.config_mut().trim_text(true);
    let mut out = Vec::new();
    let mut cur: Option<Release> = None;
    let mut tag = String::new();
    while let Ok(ev) = r.read_event() {
        match ev {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "item" {
                    cur = Some(Release { protocol, ..Release::default() });
                } else if let Some(rel) = cur.as_mut() {
                    attr_or_enclosure(rel, &name, &e);
                }
                tag = name;
            }
            Event::Empty(e) => {
                if let Some(rel) = cur.as_mut() {
                    let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    attr_or_enclosure(rel, &name, &e);
                }
            }
            Event::Text(t) => {
                if let Some(rel) = cur.as_mut() {
                    let v = t.unescape().map(|s| s.into_owned()).unwrap_or_default();
                    text_field(rel, &tag, &v);
                }
            }
            Event::CData(t) => {
                if let Some(rel) = cur.as_mut() {
                    let v = String::from_utf8_lossy(&t).to_string();
                    text_field(rel, &tag, &v);
                }
            }
            Event::End(e) => {
                if e.name().as_ref() == b"item" {
                    if let Some(mut rel) = cur.take() {
                        if rel.guid.is_empty() {
                            rel.guid = if rel.details.is_empty() { rel.link.clone() } else { rel.details.clone() };
                        }
                        if !rel.title.is_empty() && !rel.link.is_empty() {
                            rel.categories.sort_unstable();
                            rel.categories.dedup();
                            out.push(rel);
                        }
                    }
                }
                tag.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

fn text_field(rel: &mut Release, tag: &str, v: &str) {
    match tag {
        "title" => rel.title = v.trim().to_string(),
        "guid" => rel.guid = v.trim().to_string(),
        "link" => {
            if rel.link.is_empty() {
                rel.link = v.trim().to_string()
            }
        }
        "comments" => rel.details = v.trim().to_string(),
        "pubDate" => rel.date = DateTime::parse_from_rfc2822(v.trim()).ok().map(|d| d.with_timezone(&Utc)),
        "size" => rel.size = v.trim().parse().ok(),
        "description" => rel.description = v.trim().to_string(),
        "category" => {
            if let Ok(c) = v.trim().parse() {
                rel.categories.push(c)
            }
        }
        _ => {}
    }
}

fn attr_or_enclosure(rel: &mut Release, name: &str, e: &quick_xml::events::BytesStart) {
    let get =
        |k: &[u8]| e.attributes().flatten().find(|a| a.key.as_ref() == k).and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()));
    if name == "enclosure" {
        if let Some(u) = get(b"url") {
            rel.link = u;
        }
        if rel.size.is_none() {
            rel.size = get(b"length").and_then(|l| l.parse().ok()).filter(|s| *s > 0);
        }
        return;
    }
    if !name.ends_with(":attr") {
        return;
    }
    let (Some(k), Some(v)) = (get(b"name"), get(b"value")) else { return };
    let n = |s: &str| s.trim_start_matches("tt").parse::<i64>().ok();
    match k.as_str() {
        "size" => rel.size = v.parse().ok(),
        "category" => {
            if let Ok(c) = v.parse() {
                rel.categories.push(c)
            }
        }
        "seeders" => rel.seeders = v.parse().ok(),
        "peers" => {
            if let (Some(p), Some(s)) = (v.parse::<i64>().ok(), rel.seeders) {
                rel.leechers = Some((p - s).max(0));
            }
        }
        "leechers" => rel.leechers = v.parse().ok(),
        "grabs" => rel.grabs = v.parse().ok(),
        "files" => rel.files = v.parse().ok(),
        "infohash" => rel.infohash = v,
        "magneturl" => rel.magnet = v,
        "imdb" | "imdbid" => rel.imdb = n(&v),
        "tvdbid" => rel.tvdb = n(&v),
        "tmdbid" => rel.tmdb = n(&v),
        "tvmazeid" => rel.tvmaze = n(&v),
        "downloadvolumefactor" => rel.download_factor = v.parse().ok(),
        "uploadvolumefactor" => rel.upload_factor = v.parse().ok(),
        "minimumratio" => rel.minimum_ratio = v.parse().ok(),
        "minimumseedtime" => rel.minimum_seed_time = v.parse().ok(),
        "genre" => rel.genre = v,
        "year" => rel.year = v.parse().ok(),
        "coverurl" => rel.poster = v,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rss_and_errors() {
        let xml = r#"<?xml version="1.0"?><rss xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/"><channel>
            <item><title>Show.S01E02.1080p</title><guid>abc</guid><link>https://idx.test/getnzb/abc</link>
            <pubDate>Wed, 04 Nov 2026 10:00:00 +0000</pubDate><category>5040</category>
            <enclosure url="https://idx.test/getnzb/abc.nzb" length="1000" type="application/x-nzb"/>
            <newznab:attr name="category" value="5000"/><newznab:attr name="size" value="2000"/><newznab:attr name="tvdbid" value="123"/></item>
            </channel></rss>"#;
        let r = parse_rss(xml, Protocol::Usenet);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Show.S01E02.1080p");
        assert_eq!(r[0].link, "https://idx.test/getnzb/abc.nzb");
        assert_eq!(r[0].size, Some(2000));
        assert_eq!(r[0].categories, vec![5000, 5040]);
        assert_eq!(r[0].tvdb, Some(123));
        assert_eq!(
            api_error(r#"<?xml version="1.0"?><error code="100" description="Incorrect user credentials"/>"#).as_deref(),
            Some("Incorrect user credentials")
        );
    }
}
