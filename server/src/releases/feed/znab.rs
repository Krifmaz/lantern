// Ported from autobrr v1.90.0 (pkg/torznab: torznab.go, feed.go, caps.go, category.go; pkg/newznab: newznab.go,
// feed.go, caps.go, category.go), GPL-2.0-or-later.
//! autobrr's Torznab and Newznab clients: the indexer's capabilities (`t=caps`, kept for the job's life once
//! read) and one page of its newest items (`t=search` with `extended=1`, the caps' max as `limit`, an `offset`
//! for later pages and the feed's categories as `cat`). The two are the same client with the few differences
//! autobrr's two packages have: the error words, how an item's categories are read and the enclosure's length.

use super::xml::{self, Node};
use chrono::{DateTime, Utc};
use std::time::Duration;

/// Torznab or Newznab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Torznab (torrents).
    Torznab,
    /// Newznab (usenet).
    Newznab,
}

impl Kind {
    /// Its lower-case name, as autobrr's errors start.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Torznab => "torznab",
            Kind::Newznab => "newznab",
        }
    }
}

/// The page size when the caps give no max (autobrr's `defaultSearchLimit`).
pub const DEFAULT_SEARCH_LIMIT: i64 = 50;

/// Go's `url.QueryEscape`.
pub fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Go's `url.QueryUnescape`; `None` on a bad escape.
pub fn query_unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Go's `url.ParseQuery`, keeping the pairs it can read (a bad pair is dropped, as autobrr's `mergeQuery`
/// keeps the rest).
pub fn parse_query(q: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for part in q.split('&') {
        if part.is_empty() || part.contains(';') {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let (Some(k), Some(v)) = (query_unescape(k), query_unescape(v)) else { continue };
        match out.iter_mut().find(|(n, _)| *n == k) {
            Some((_, vs)) => vs.push(v),
            None => out.push((k, vec![v])),
        }
    }
    out
}

/// Go's `url.Values.Encode`: keys sorted, every value escaped.
pub fn encode_query(mut q: Vec<(String, Vec<String>)>) -> String {
    q.sort_by(|a, b| a.0.cmp(&b.0));
    let mut parts = Vec::new();
    for (k, vs) in q {
        for v in vs {
            parts.push(format!("{}={}", query_escape(&k), query_escape(&v)));
        }
    }
    parts.join("&")
}

/// The host's own query with `params` merged in, `params` winning (autobrr's `mergeQuery`).
pub fn merge_query(host_query: &str, params: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    let mut q = parse_query(host_query);
    for (k, vs) in params {
        match q.iter_mut().find(|(n, _)| n == k) {
            Some((_, old)) => *old = vs.clone(),
            None => q.push((k.clone(), vs.clone())),
        }
    }
    q
}

/// Go's `strconv.ParseInt(s, 0, 64)`: `0x`, `0o`, `0b` and a leading `0` (octal) prefixes, `_` separators.
pub fn parse_int_base0(s: &str) -> Option<i64> {
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let lower = rest.to_ascii_lowercase();
    let (radix, digits) = if let Some(d) = lower.strip_prefix("0x") {
        (16, d.to_string())
    } else if let Some(d) = lower.strip_prefix("0o") {
        (8, d.to_string())
    } else if let Some(d) = lower.strip_prefix("0b") {
        (2, d.to_string())
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, lower[1..].to_string())
    } else {
        (10, lower.clone())
    };
    let digits = if radix != 10 { digits.replace('_', "") } else { digits };
    if digits.is_empty() {
        return None;
    }
    let v = i64::from_str_radix(&digits, radix).ok()?;
    Some(if neg { -v } else { v })
}

/// A Go decode of an unsigned number element (`strconv.ParseUint`, spaces trimmed, empty is 0).
fn parse_uint(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    t.parse::<u64>().map_err(|_| format!("strconv.ParseUint: parsing {t:?}: invalid syntax"))
}

/// A Go decode of a signed number (element or attribute).
fn parse_int(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    t.parse::<i64>().map_err(|_| format!("strconv.ParseInt: parsing {t:?}: invalid syntax"))
}

/// An RFC 1123 date with a numeric zone (Go's `time.RFC1123Z`, what Torznab and Newznab dates use).
pub fn parse_rfc1123z(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_str(s, "%a, %d %b %Y %H:%M:%S %z")
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| format!("could not parse date: parsing time {s:?} as \"Mon, 02 Jan 2006 15:04:05 -0700\": cannot parse"))
}

/// A category (autobrr's `Category`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Category {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Its sub-categories.
    pub sub_categories: Vec<Category>,
}

/// The Newznab spec's parent category of `id` (autobrr's `ParentCategory`).
pub fn parent_category(id: i64) -> Category {
    let (id, name) = match id {
        i if i < 1000 => (0, "Other"),
        i if i < 2000 => (1000, "Console"),
        i if i < 3000 => (2000, "Movies"),
        i if i < 4000 => (3000, "Audio"),
        i if i < 5000 => (4000, "PC"),
        i if i < 6000 => (5000, "TV"),
        i if i < 7000 => (6000, "XXX"),
        i if i < 8000 => (7000, "Books"),
        _ => (0, "Other"),
    };
    Category { id, name: name.into(), sub_categories: Vec::new() }
}

/// One way of searching the caps offer (autobrr's `Search`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    /// `yes` or `no`.
    pub available: String,
    /// The parameters it takes.
    pub supported_params: String,
}

/// The indexer's capabilities (autobrr's `Caps`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caps {
    /// version, title, strapline, email, url, image.
    pub server: [String; 6],
    /// The largest page.
    pub limits_max: i64,
    /// The default page.
    pub limits_default: i64,
    /// search, tv-search, movie-search, audio-search, book-search.
    pub searching: [Search; 5],
    /// The categories.
    pub categories: Vec<Category>,
}

/// A category element and its `subcat`s.
fn read_category(n: &Node) -> Result<Category, String> {
    Ok(Category {
        id: parse_int(n.attr("id").unwrap_or(""))?,
        name: n.attr("name").unwrap_or("").to_string(),
        sub_categories: n.all("subcat").map(read_category).collect::<Result<_, _>>()?,
    })
}

/// Reads a caps answer (Go's `xml.Unmarshal` into `Caps`).
pub fn parse_caps(body: &str) -> Result<Caps, String> {
    let root = xml::parse(body)?;
    if root.name != "caps" {
        return Err(format!("expected element type <caps> but have <{}>", root.name));
    }
    let mut c = Caps::default();
    if let Some(s) = root.child("server") {
        for (i, k) in ["version", "title", "strapline", "email", "url", "image"].iter().enumerate() {
            c.server[i] = s.attr(k).unwrap_or("").to_string();
        }
    }
    if let Some(l) = root.child("limits") {
        c.limits_max = parse_int(l.attr("max").unwrap_or(""))?;
        c.limits_default = parse_int(l.attr("default").unwrap_or(""))?;
    }
    if let Some(s) = root.child("searching") {
        for (i, k) in ["search", "tv-search", "movie-search", "audio-search", "book-search"].iter().enumerate() {
            if let Some(x) = s.child(k) {
                c.searching[i] = Search {
                    available: x.attr("available").unwrap_or("").to_string(),
                    supported_params: x.attr("supportedParams").unwrap_or("").to_string(),
                };
            }
        }
    }
    if let Some(cats) = root.child("categories") {
        c.categories = cats.all("category").map(read_category).collect::<Result<_, _>>()?;
    }
    Ok(c)
}

/// An item's enclosure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Enclosure {
    /// The address.
    pub url: String,
    /// The length as written.
    pub length: String,
    /// The length as a number (Newznab's is one).
    pub length_n: u64,
    /// Its media type.
    pub kind: String,
}

/// One item of a Torznab or Newznab answer (autobrr's two `FeedItem`s in one).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    /// The release name.
    pub title: String,
    /// Its id at the indexer.
    pub guid: String,
    /// When it was published.
    pub pub_date: Option<DateTime<Utc>>,
    /// The comments page.
    pub comments: String,
    /// Bytes.
    pub size: u64,
    /// The download link.
    pub link: String,
    /// Torznab: the magnet link (element or attribute).
    pub magnet_uri: String,
    /// The enclosure.
    pub enclosure: Option<Enclosure>,
    /// Torznab: the category ids it lists.
    pub category: Vec<i64>,
    /// Newznab: the category elements as written.
    pub category_text: Vec<String>,
    /// The categories with names, mapped from the caps.
    pub categories: Vec<Category>,
    /// Genres.
    pub genres: Vec<String>,
    /// TVDB id.
    pub tvdb_id: String,
    /// IMDb id (`tt…`).
    pub imdb_id: String,
    /// TMDB id.
    pub tmdb_id: String,
    /// Grabs.
    pub grabs: i64,
    /// Seeders.
    pub seeders: i64,
    /// Leechers.
    pub leechers: i64,
    /// Peers.
    pub peers: i64,
    /// Files.
    pub files: i64,
    /// Torznab: the download volume factor (0 is freeleech).
    pub download_volume_factor: f64,
    /// Torznab: the upload volume factor.
    pub upload_volume_factor: f64,
    /// Who uploaded it.
    pub author: String,
    /// Torznab: tagged freeleech.
    pub freeleech: bool,
    /// Torznab: tagged internal.
    pub internal: bool,
    /// Newznab: the season.
    pub season: i64,
    /// Newznab: the episode.
    pub episode: i64,
    /// The `attr` elements (name, value).
    pub attributes: Vec<(String, String)>,
}

/// The last child named `name`'s text (Go overwrites a single field with every element of its name).
fn last_text(n: &Node, name: &str) -> Option<String> {
    n.all(name).last().map(|c| c.text.clone())
}

impl Item {
    /// Fills in what the `attr` elements say (autobrr's `parseAttributes`).
    pub fn parse_attributes(&mut self, kind: Kind) {
        let int = |v: &str| parse_int_base0(v).filter(|n| i32::try_from(*n).is_ok());
        let float = |v: &str| v.parse::<f32>().ok().map(f64::from);
        let attrs = self.attributes.clone();
        for (name, value) in &attrs {
            match name.as_str() {
                "author" => self.author = value.clone(),
                "grabs" => self.grabs = int(value).unwrap_or(self.grabs),
                "files" => self.files = int(value).unwrap_or(self.files),
                "imdb" | "imdbid" => {
                    if self.imdb_id.is_empty() && !value.is_empty() {
                        self.imdb_id = if value.starts_with("tt") { value.clone() } else { format!("tt{value}") };
                    }
                }
                "tvdb" | "tvdbid" => {
                    if self.tvdb_id.is_empty() {
                        self.tvdb_id = value.clone();
                    }
                }
                "tmdb" | "tmdbid" => {
                    if self.tmdb_id.is_empty() {
                        self.tmdb_id = value.clone();
                    }
                }
                "genre" => self.genres = value.split(',').map(str::to_string).collect(),
                _ => {}
            }
            match kind {
                Kind::Torznab => match name.as_str() {
                    "seeders" => self.seeders = int(value).unwrap_or(self.seeders),
                    "leechers" => self.leechers = int(value).unwrap_or(self.leechers),
                    "peers" => self.peers = int(value).unwrap_or(self.peers),
                    "tag" => {
                        if value == "internal" {
                            self.internal = true;
                        }
                        if value == "freeleech" {
                            self.freeleech = true;
                        }
                    }
                    "downloadvolumefactor" => self.download_volume_factor = float(value).unwrap_or(self.download_volume_factor),
                    "uploadvolumefactor" => self.upload_volume_factor = float(value).unwrap_or(self.upload_volume_factor),
                    "magneturl" if self.magnet_uri.is_empty() => self.magnet_uri = value.clone(),
                    _ => {}
                },
                Kind::Newznab => match name.as_str() {
                    "season" => self.season = int(value).unwrap_or(self.season),
                    "episode" => self.episode = int(value).unwrap_or(self.episode),
                    _ => {}
                },
            }
        }
        if kind == Kind::Torznab && self.leechers == 0 && self.peers > 0 && self.seeders > 0 {
            self.leechers = self.peers - self.seeders;
        }
    }

    /// The item element `n` (Go's `UnmarshalXML`, then `parseAttributes`).
    pub fn read(n: &Node, kind: Kind) -> Result<Item, String> {
        let mut it = Item {
            title: last_text(n, "title").unwrap_or_default(),
            guid: last_text(n, "guid").unwrap_or_default(),
            comments: last_text(n, "comments").unwrap_or_default(),
            link: last_text(n, "link").unwrap_or_default(),
            tvdb_id: last_text(n, "tvdb").unwrap_or_default(),
            imdb_id: last_text(n, "imdb").unwrap_or_default(),
            tmdb_id: last_text(n, "tmdb").unwrap_or_default(),
            genres: n.all("genre").map(|g| g.text.clone()).collect(),
            ..Item::default()
        };
        if let Some(d) = n.all("pubDate").last() {
            it.pub_date = Some(parse_rfc1123z(&d.text)?);
        }
        if let Some(s) = last_text(n, "size") {
            it.size = parse_uint(&s)?;
        }
        if let Some(e) = n.all("enclosure").last() {
            let length = e.attr("length").unwrap_or("").to_string();
            let length_n = if kind == Kind::Newznab { parse_uint(&length)? } else { 0 };
            it.enclosure = Some(Enclosure {
                url: e.attr("url").unwrap_or("").to_string(),
                length,
                length_n,
                kind: e.attr("type").unwrap_or("").to_string(),
            });
        }
        match kind {
            Kind::Torznab => {
                it.magnet_uri = last_text(n, "magneturl").unwrap_or_default();
                if let Some(l) = last_text(n, "leechers") {
                    it.leechers = parse_int(&l)?;
                }
                for c in n.all("category") {
                    it.category.push(parse_int(&c.text)?);
                }
            }
            Kind::Newznab => {
                it.category_text = n.all("category").map(|c| c.text.clone()).collect();
                if let Some(f) = last_text(n, "files") {
                    it.files = parse_int(&f)?;
                }
            }
        }
        it.attributes =
            n.all("attr").map(|a| (a.attr("name").unwrap_or("").to_string(), a.attr("value").unwrap_or("").to_string())).collect();
        it.parse_attributes(kind);
        Ok(it)
    }

    /// Torznab: the listed category ids with names, standard ones by their parent and the indexer's own from
    /// the caps (autobrr's torznab `MapCategories`).
    pub fn map_categories(&mut self, caps: &[Category]) {
        for &id in &self.category {
            if id < 10000 {
                self.categories.push(parent_category(id));
                continue;
            }
            if let Some(c) = caps.iter().find(|c| c.id == id) {
                self.categories.push(Category { id: c.id, name: c.name.clone(), sub_categories: Vec::new() });
            }
        }
    }

    /// Newznab: the `category` attributes with names (autobrr's `MapCustomCategoriesFromAttr`).
    pub fn map_custom_categories_from_attr(&mut self, caps: &[Category]) {
        for (name, value) in &self.attributes {
            if name != "category" {
                continue;
            }
            let Ok(id) = value.parse::<i64>() else { continue };
            if id > 0 && id < 10000 {
                self.categories.push(parent_category(id));
            } else if id > 10000 {
                if let Some(c) = caps.iter().find(|c| c.id == id) {
                    self.categories.push(Category { id: c.id, name: c.name.clone(), sub_categories: Vec::new() });
                }
            }
        }
    }
}

/// One page of a search (autobrr's `SearchResponse`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchResponse {
    /// The channel's title.
    pub title: String,
    /// The items.
    pub items: Vec<Item>,
    /// The answer as it came.
    pub raw: String,
    /// The page size asked for.
    pub limit: i64,
    /// The total the indexer reports (0: not reported).
    pub total: i64,
}

/// Reads a search answer: the channel's title, items and paging (Go's `xml.Unmarshal` into `Feed`).
pub fn parse_feed(body: &str, kind: Kind) -> Result<(String, Vec<Item>, i64), String> {
    let root = xml::parse(body)?;
    let Some(ch) = root.child("channel") else { return Ok((String::new(), Vec::new(), 0)) };
    let items = ch.all("item").map(|i| Item::read(i, kind)).collect::<Result<Vec<_>, _>>()?;
    let total = match ch.all("response").last() {
        Some(r) => {
            parse_int(r.attr("offset").unwrap_or(""))?;
            parse_int(r.attr("total").unwrap_or(""))?
        }
        None => 0,
    };
    Ok((last_text(ch, "title").unwrap_or_default(), items, total))
}

/// A Torznab or Newznab indexer (autobrr's `Client`).
pub struct Client {
    /// Which of the two.
    pub kind: Kind,
    http: reqwest::Client,
    /// The address (its own query is kept).
    pub host: String,
    /// The API key (`apikey`).
    pub api_key: String,
    /// The caps, once read.
    pub capabilities: Option<Caps>,
}

impl Client {
    /// A client for `host` with `api_key`, sending through `http`.
    pub fn new(kind: Kind, host: &str, api_key: &str, http: reqwest::Client) -> Client {
        Client { kind, http, host: host.to_string(), api_key: api_key.to_string(), capabilities: None }
    }

    /// A client with its own HTTP client (autobrr's `NewClient` defaults: 60 seconds for Newznab, none for
    /// Torznab's own).
    pub fn plain(kind: Kind, host: &str, api_key: &str) -> Client {
        let timeout = if kind == Kind::Newznab { Duration::from_secs(60) } else { Duration::from_secs(3600) };
        Client::new(kind, host, api_key, crate::releases::clients::http(timeout, false, false))
    }

    /// The address with `params` (and the API key) merged into the host's query.
    pub fn url(&self, mut params: Vec<(String, Vec<String>)>) -> Result<String, String> {
        if !self.api_key.is_empty() {
            // autobrr escapes the key before url.Values escapes it again
            params.push(("apikey".into(), vec![query_escape(&self.api_key)]));
        }
        let mut u = url::Url::parse(&self.host).map_err(|e| format!("parse {:?}: {e}", self.host))?;
        let path = u.path().trim_end_matches('/').to_string();
        u.set_path(&path);
        let q = encode_query(merge_query(u.query().unwrap_or(""), &params));
        u.set_query(if q.is_empty() { None } else { Some(&q) });
        Ok(u.to_string())
    }

    /// GETs `params`; the body, or the problem in autobrr's words.
    async fn get(&self, params: Vec<(String, Vec<String>)>) -> Result<String, String> {
        let url = self.url(params)?;
        let resp = self
            .http
            .get(&url)
            .header("User-Agent", crate::releases::clients::user_agent())
            .send()
            .await
            .map_err(|e| format!("could not make request. {}", crate::releases::clients::req_err("GET", &url, &e)))?;
        match resp.status().as_u16() {
            200 => {}
            401 => return Err("unauthorized".into()),
            404 => return Err("not found, make sure the path is correct".into()),
            s => return Err(format!("unexpected status code: {s}")),
        }
        resp.text().await.map_err(|e| format!("{}.io.Copy: {e}", self.kind.name()))
    }

    /// The caps, read from the indexer (autobrr's `getCaps`).
    async fn get_caps(&self) -> Result<Caps, String> {
        let body = self.get(vec![("t".into(), vec!["caps".into()])]).await?;
        parse_caps(&body).map_err(|e| format!("{}: could not decode feed: {e}", self.kind.name()))
    }

    /// The caps, read again (autobrr's `FetchCaps` / `GetCaps`).
    pub async fn fetch_caps(&self) -> Result<Caps, String> {
        self.get_caps().await.map_err(|e| format!("could not get caps for feed: {e}"))
    }

    /// The caps, read once (autobrr's `getAndSetCaps`).
    async fn get_and_set_caps(&mut self) -> Result<(), String> {
        if self.capabilities.is_none() {
            let caps = self.get_caps().await.map_err(|e| format!("could not get caps for feed: {e}"))?;
            self.capabilities = Some(caps);
        }
        Ok(())
    }

    /// The page size: the caps' max, else 50 (autobrr's `pageLimit`).
    pub fn page_limit(&self) -> i64 {
        match &self.capabilities {
            Some(c) if c.limits_max > 0 => c.limits_max,
            _ => DEFAULT_SEARCH_LIMIT,
        }
    }

    /// One page of results from `offset` (autobrr's `Search`).
    pub async fn search(&mut self, query: &str, categories: &[i64], offset: i64) -> Result<SearchResponse, String> {
        self.get_and_set_caps().await?;
        let one = |k: &str, v: String| (k.to_string(), vec![v]);
        let mut params = vec![one("t", "search".into())];
        if !query.is_empty() {
            params.push(one("q", query.into()));
        }
        params.push(one("extended", "1".into()));
        params.push(one("limit", self.page_limit().to_string()));
        if offset > 0 {
            params.push(one("offset", offset.to_string()));
        }
        params.push(one("cat", categories.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")));
        let raw = self.get(params).await.map_err(|e| format!("could not search feed: {e}"))?;
        let (title, mut items, total) =
            parse_feed(&raw, self.kind).map_err(|e| format!("could not search feed: {}: could not decode feed: {e}", self.kind.name()))?;
        let caps = self.capabilities.as_ref().map(|c| c.categories.clone()).unwrap_or_default();
        for it in items.iter_mut() {
            match self.kind {
                Kind::Torznab => it.map_categories(&caps),
                Kind::Newznab => it.map_custom_categories_from_attr(&caps),
            }
        }
        Ok(SearchResponse { title, items, raw, limit: self.page_limit(), total })
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's pkg/torznab/feed_test.go and pkg/newznab/feed_test.go.
    use super::*;

    fn item_with(attrs: &[(&str, &str)]) -> Item {
        Item { attributes: attrs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), ..Item::default() }
    }

    #[test]
    fn test_feed_item_parse_attributes() {
        for kind in [Kind::Torznab, Kind::Newznab] {
            for (value, want) in [("1234567", "tt1234567"), ("tt1234567", "tt1234567"), ("", "")] {
                let mut it = item_with(&[("imdb", value)]);
                it.parse_attributes(kind);
                assert_eq!(it.imdb_id, want);
            }
            for name in ["tvdbid", "tvdb"] {
                let mut it = item_with(&[(name, "12345")]);
                it.parse_attributes(kind);
                assert_eq!(it.tvdb_id, "12345");
            }
            let mut it = item_with(&[("tvdbid", "1"), ("tvdb", "2")]);
            it.parse_attributes(kind);
            assert_eq!(it.tvdb_id, "1", "first value wins");
        }
    }

    #[test]
    fn test_feed_item_parse_attributes_magnet_url() {
        let mut it = item_with(&[("magneturl", "magnet:?xt=urn:btih:abc")]);
        it.parse_attributes(Kind::Torznab);
        assert_eq!(it.magnet_uri, "magnet:?xt=urn:btih:abc");
        let mut it = item_with(&[]);
        it.parse_attributes(Kind::Torznab);
        assert_eq!(it.magnet_uri, "");
        let mut it = item_with(&[("magneturl", "magnet:?xt=urn:btih:attr")]);
        it.magnet_uri = "magnet:?xt=urn:btih:element".into();
        it.parse_attributes(Kind::Torznab);
        assert_eq!(it.magnet_uri, "magnet:?xt=urn:btih:element", "element takes precedence over attr");
    }

    #[test]
    fn test_newznab_feed_item_unmarshal_xml() {
        let body = r#"<item><title>T</title><newznab:attr xmlns:newznab="x" name="imdb" value="1234567"/><newznab:attr xmlns:newznab="x" name="tvdbid" value="99"/></item>"#;
        let n = xml::parse(body).unwrap();
        let it = Item::read(&n, Kind::Newznab).unwrap();
        assert_eq!((it.imdb_id.as_str(), it.tvdb_id.as_str()), ("tt1234567", "99"));
    }

    #[test]
    fn merge_and_encode_like_go() {
        let q = merge_query("foo=bar&bad=%zz&apikey=hostkey", &[("apikey".into(), vec!["fieldkey".into()])]);
        assert_eq!(encode_query(q), "apikey=fieldkey&foo=bar");
        assert_eq!(query_escape("a b&c~"), "a+b%26c~");
        assert_eq!(parse_int_base0("0x10"), Some(16));
        assert_eq!(parse_int_base0("010"), Some(8));
    }
}
