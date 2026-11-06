// Ported from autobrr v1.90.0 (internal/feed/torznab.go, newznab.go, rss.go, client.go, source.go),
// GPL-2.0-or-later.
//! The three feed formats as sources for a refresh: each reads one page and turns its items into releases the
//! way autobrr does (download and magnet links, sizes, seeders, freeleech, categories, ids).

use super::gofeed;
use super::refresh::{Entry, FetchResult, Source};
use super::znab::{self, Kind};
use super::{Feed, FeedCapabilities, DOWNLOAD_TYPE_MAGNET, DOWNLOAD_TYPE_NZB};
use crate::releases::humanize;
use crate::releases::release::{normalize_imdb_id, IndexerMinimal, Release};
use crate::releases::run::MAGNET_URI_PREFIX;
use futures_util::future::BoxFuture;
use regex::Regex;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// How long a feed request may take without a timeout of its own (autobrr's `defaultTimeout`).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The HTTP client of feed `f`: its timeout (60 seconds by default), certificate setting and proxy (autobrr's
/// `newHTTPClient`). A proxy that can't be used gives a client that fails every request, so the feed never
/// quietly goes direct.
pub fn http_client(f: &Feed, cookies: bool) -> reqwest::Client {
    let timeout = if f.timeout > 0 { Duration::from_secs(f.timeout as u64) } else { DEFAULT_TIMEOUT };
    if let Some(p) = f.proxy.as_ref().filter(|_| f.use_proxy) {
        return match p.http_client(timeout, f.tls_skip_verify, cookies) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("feed {}: could not get proxy client: {e}", f.name);
                // a proxy nobody answers at: every request fails instead of going around the proxy
                reqwest::Client::builder()
                    .proxy(reqwest::Proxy::all("http://127.0.0.1:9").expect("a fixed proxy address"))
                    .timeout(timeout)
                    .build()
                    .unwrap_or_default()
            }
        };
    }
    crate::releases::clients::http(timeout, f.tls_skip_verify, cookies)
}

/// A number id from a feed attribute; 0 when empty or not a number (autobrr's `parseMetaID`).
pub fn parse_meta_id(v: &str) -> i64 {
    v.parse::<i64>().ok().filter(|n| i32::try_from(*n).is_ok() || *n > 0).unwrap_or(0)
}

/// Whether `s` is an http(s) address with a host.
fn is_web_url(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some_and(|h| !h.is_empty()))
}

/// The share of the download that doesn't count, 0–100, from Torznab's `downloadvolumefactor` (autobrr's
/// `parseFreeleechTorznab`).
pub fn parse_freeleech_torznab(factor: f64) -> i64 {
    if !(0.0..=1.0).contains(&factor) {
        return 0;
    }
    100 - (factor * 100.0).round() as i64
}

/// The bonus of a freeleech percentage, and whether it is one of 25, 50, 75 or 100 (autobrr's
/// `mapFreeleechToBonus`).
pub fn map_freeleech_to_bonus(p: i64) -> (String, bool) {
    if p <= 0 || p > 100 {
        return (String::new(), false);
    }
    match p {
        25 | 50 | 75 | 100 => (format!("Freeleech{p}"), true),
        _ => (format!("Freeleech{p}"), false),
    }
}

/// A Torznab or Newznab feed.
pub struct ZnabSource {
    /// The feed.
    pub feed: Arc<Feed>,
    /// Its client (the caps are kept once read).
    pub client: znab::Client,
}

impl ZnabSource {
    /// The source of feed `f`.
    pub fn new(f: Arc<Feed>, kind: Kind) -> ZnabSource {
        let client = znab::Client::new(kind, &f.url, &f.api_key, http_client(&f, false));
        ZnabSource { feed: f, client }
    }

    /// The feed's capabilities (autobrr's `caps`).
    pub async fn caps(&self) -> Result<FeedCapabilities, String> {
        Ok(FeedCapabilities::from_caps(&self.client.fetch_caps().await?))
    }
}

impl Source for ZnabSource {
    fn fetch(&mut self, offset: usize) -> BoxFuture<'_, Result<FetchResult, String>> {
        Box::pin(async move {
            let cats = self.feed.categories.clone().unwrap_or_default();
            let resp = self.client.search("", &cats, offset as i64).await?;
            let mut res = FetchResult {
                raw: resp.raw,
                items: resp.items.len(),
                limit: resp.limit.max(0) as usize,
                total: resp.total.max(0) as usize,
                ..FetchResult::default()
            };
            for it in resp.items {
                if it.guid.is_empty() {
                    tracing::warn!("feed {}: an item has no guid: {}", self.feed.name, it.title);
                    continue;
                }
                let f = self.feed.clone();
                let kind = self.client.kind;
                let item = it.clone();
                res.entries.push(Entry {
                    key: it.guid.clone(),
                    title: it.title.clone(),
                    pub_date: it.pub_date,
                    release: Arc::new(move || match kind {
                        Kind::Torznab => torznab_release(&f, &item),
                        Kind::Newznab => newznab_release(&f, &item),
                    }),
                });
            }
            Ok(res)
        })
    }
}

/// A Torznab item as a release (autobrr's torznab `toRelease`).
pub fn torznab_release(f: &Feed, item: &znab::Item) -> Release {
    let mut r = Release::new(f.indexer_minimal());
    r.implementation = "TORZNAB".into();
    r.torrent_name = item.title.clone();
    r.download_url = item.link.clone();
    let comments = item.comments.trim();
    if r.info_url.is_empty() && !comments.is_empty() && is_web_url(comments) {
        r.info_url = comments.to_string();
    }
    let guid = item.guid.trim();
    if r.info_url.is_empty() && !guid.is_empty() && is_web_url(guid) {
        r.info_url = guid.to_string();
    }
    if let Some(e) = &item.enclosure {
        if e.kind == "application/x-bittorrent" {
            r.download_url = e.url.clone();
        }
    }
    if f.download_type() == DOWNLOAD_TYPE_MAGNET {
        r.magnet_uri = if item.magnet_uri.starts_with(MAGNET_URI_PREFIX) { item.magnet_uri.clone() } else { r.download_url.clone() };
    }
    if r.download_url.starts_with(MAGNET_URI_PREFIX) {
        if r.magnet_uri.is_empty() {
            r.magnet_uri = r.download_url.clone();
        }
        r.download_url.clear();
    }
    r.parse_string(&item.title);
    r.size = item.size;
    r.seeders = item.seeders;
    r.leechers = item.leechers;
    r.uploader = item.author.clone();
    r.meta_imdb = normalize_imdb_id(&item.imdb_id);
    r.meta_tmdb = parse_meta_id(&item.tmdb_id);
    r.meta_tvdb = parse_meta_id(&item.tvdb_id);
    r.publish_date = item.pub_date;
    let p = parse_freeleech_torznab(item.download_volume_factor);
    if p >= 0 {
        if p == 100 {
            r.freeleech = true;
            r.bonus = vec!["Freeleech".into()];
        }
        r.freeleech_percent = p;
        let (bonus, ok) = map_freeleech_to_bonus(p);
        if ok && !bonus.is_empty() {
            r.bonus.push(bonus);
        }
    }
    for c in &item.categories {
        r.categories.push(c.name.clone());
        r.categories.push(c.id.to_string());
    }
    r
}

/// A Newznab item as a release (autobrr's newznab `toRelease`).
pub fn newznab_release(f: &Feed, item: &znab::Item) -> Release {
    let mut r = Release::new(f.indexer_minimal());
    r.implementation = "NEWZNAB".into();
    r.protocol = "usenet".into();
    r.torrent_name = item.title.clone();
    r.info_url = item.guid.clone();
    r.parse_string(&item.title);
    r.meta_imdb = normalize_imdb_id(&item.imdb_id);
    r.meta_tmdb = parse_meta_id(&item.tmdb_id);
    r.meta_tvdb = parse_meta_id(&item.tvdb_id);
    r.publish_date = item.pub_date;
    r.size = item.size;
    if let Some(e) = &item.enclosure {
        if e.kind == "application/x-nzb" {
            r.download_url = e.url.clone();
            if r.size == 0 && e.length_n > item.size {
                r.size = e.length_n;
            }
        }
    }
    if item.categories.len() == 1 {
        r.category = item.categories[0].name.clone();
    }
    for c in &item.categories {
        r.categories.push(c.name.clone());
        r.categories.push(c.id.to_string());
    }
    r
}

/// An RSS (or Atom) feed.
pub struct RssSource {
    /// The feed.
    pub feed: Arc<Feed>,
    http: reqwest::Client,
}

impl RssSource {
    /// The source of feed `f`, with a cookie jar of its own (autobrr's `NewFeedParser`).
    pub fn new(f: Arc<Feed>) -> RssSource {
        let http = http_client(&f, true);
        RssSource { feed: f, http }
    }
}

/// gofeed's error for an answer that isn't 2xx.
fn http_error(status: reqwest::StatusCode) -> String {
    format!("http error: {} {}", status.as_u16(), status.canonical_reason().unwrap_or(""))
}

/// Reads the feed at `url` with `cookie` and `user_agent` (autobrr's `ParseURLWithContext`).
pub async fn fetch_rss(http: &reqwest::Client, url: &str, cookie: &str, user_agent: &str) -> Result<gofeed::Feed, String> {
    let ua = if user_agent.is_empty() { "Gofeed/1.0" } else { user_agent };
    let mut req = http.get(url).header("User-Agent", ua);
    if !cookie.is_empty() {
        req = req.header("Cookie", cookie);
    }
    let resp = req.send().await.map_err(|e| crate::releases::clients::req_err("Get", url, &e))?;
    if !resp.status().is_success() {
        return Err(http_error(resp.status()));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    gofeed::parse(&body)
}

impl Source for RssSource {
    fn fetch(&mut self, _offset: usize) -> BoxFuture<'_, Result<FetchResult, String>> {
        Box::pin(async move {
            let feed = fetch_rss(&self.http, &self.feed.url, &self.feed.cookie, &self.feed.user_agent).await?;
            let mut res = FetchResult { raw: feed.string(), items: feed.items.len(), ..FetchResult::default() };
            for it in feed.items {
                let key = [&it.guid, &it.link, &it.title].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
                let f = self.feed.clone();
                let item = it.clone();
                res.entries.push(Entry {
                    key,
                    title: it.title.clone(),
                    pub_date: it.published_parsed,
                    release: Arc::new(move || rss_release(&f, &item)),
                });
            }
            Ok(res)
        })
    }
}

static RXP_SIZE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?mi)(([0-9.]+)\s*(b|kb|kib|kilobyte|mb|mib|megabyte|gb|gib|gigabyte|tb|tib|terabyte))").expect("size pattern")
});
static RXP_FREELEECH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)(\bfreeleech\b)").expect("freeleech pattern"));
static RXP_HTML: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)<.*?>").expect("html pattern"));

/// Whether any of `s` says freeleech (autobrr's `isFreeleech`).
pub fn is_freeleech(s: &[&str]) -> bool {
    s.iter().any(|x| RXP_FREELEECH.is_match(x))
}

/// The largest size written in `s` (HTML tags left out), kept when bigger than `r`'s (autobrr's
/// `readSizeFromDescription`).
pub fn read_size_from_description(s: &str, r: &mut Release) -> bool {
    let clean = RXP_HTML.replace_all(s, " ");
    let mut found = false;
    for m in RXP_SIZE.find_iter(&clean) {
        if let Ok(n) = humanize::parse_bytes(m.as_str()) {
            if n > 0 && n > r.size {
                found = true;
                r.size = n;
            }
        }
    }
    found
}

/// Go's `path.Clean` of an absolute path.
fn clean_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    let mut s = format!("/{}", out.join("/"));
    if p.ends_with('/') && s.len() > 1 {
        s.push('/');
    }
    s
}

/// A link without a host made absolute on the feed's address (autobrr's relative URL handling: the feed's
/// scheme, host and query, the link as the path, then unescaped).
fn absolute(feed_url: &str, link: &str) -> Option<String> {
    let parent = url::Url::parse(feed_url).ok()?;
    let host = parent.host_str()?;
    let port = parent.port().map(|p| format!(":{p}")).unwrap_or_default();
    let path = clean_path(&format!("/{}", crate::releases::sanitize::url_encoding(link)));
    let escaped = path.replace('?', "%3F");
    let q = parent.query().map(|q| format!("?{q}")).unwrap_or_default();
    Some(znab::query_unescape(&format!("{}://{host}{port}{escaped}{q}", parent.scheme())).unwrap_or_default())
}

/// An RSS item as a release (autobrr's rss `toRelease`).
pub fn rss_release(f: &Feed, item: &gofeed::Item) -> Release {
    let mut r = Release::new(f.indexer_minimal());
    r.implementation = "RSS".into();
    if let Some(p) = item.published_parsed {
        r.publish_date = Some(p);
    }
    r.parse_string(&item.title);
    let dt = f.download_type();
    if dt == DOWNLOAD_TYPE_MAGNET {
        r.magnet_uri = item.link.clone();
        r.download_url.clear();
    }
    for e in &item.enclosures {
        if e.kind == "application/x-nzb" {
            if !dt.is_empty() && dt != DOWNLOAD_TYPE_NZB {
                continue;
            }
            if !e.url.is_empty() {
                r.download_url = e.url.clone();
            }
            if !e.length.is_empty() && e.length != "1" {
                r.parse_size_bytes_string(&e.length);
            }
            r.protocol = "usenet".into();
            break;
        }
        if e.kind == "application/x-bittorrent" {
            if dt == DOWNLOAD_TYPE_NZB {
                continue;
            }
            if !e.url.is_empty() {
                r.download_url = e.url.clone();
            }
            if !e.length.is_empty() && e.length != "1" && e.length != "39399" {
                r.parse_size_bytes_string(&e.length);
            }
            if dt == DOWNLOAD_TYPE_MAGNET && !r.magnet_uri.starts_with(MAGNET_URI_PREFIX) && e.url.starts_with(MAGNET_URI_PREFIX) {
                r.magnet_uri = e.url.clone();
                r.download_url.clear();
            }
            break;
        }
    }
    if dt == DOWNLOAD_TYPE_NZB {
        r.protocol = "usenet".into();
    }
    if r.download_url.is_empty() && !item.link.is_empty() {
        r.download_url = crate::releases::sanitize::url_encoding(&item.link);
    }
    if r.download_url.starts_with(MAGNET_URI_PREFIX) {
        if r.magnet_uri.is_empty() {
            r.magnet_uri = r.download_url.clone();
        }
        r.download_url.clear();
    }
    if !r.download_url.is_empty() {
        let has_host = url::Url::parse(&r.download_url).ok().is_some_and(|u| u.host_str().is_some_and(|h| !h.is_empty()));
        if !has_host {
            if let Some(a) = absolute(&f.url, &r.download_url) {
                r.download_url = a;
            }
        }
    }
    for v in &item.categories {
        r.categories.extend(item.categories.iter().cloned());
        if !r.category.is_empty() {
            r.category.push_str(", ");
        }
        r.category.push_str(v);
    }
    for a in &item.authors {
        if !r.uploader.is_empty() {
            r.uploader.push_str(", ");
        }
        r.uploader.push_str(&a.name);
    }
    if !item.description.is_empty() {
        r.description = item.description.clone();
        read_size_from_description(&item.description, &mut r);
    }
    if let Some(size) = item.custom.get("size") {
        r.parse_size_bytes_string(size);
    }
    let content_length = item.custom.get("contentlength").or_else(|| item.custom.get("contentLength"));
    if let Some(cl) = content_length.filter(|c| !c.is_empty()) {
        if let Ok(n) = cl.parse::<u64>() {
            if n > r.size {
                r.size = n;
            }
        }
    }
    if let Some(v) = item.custom.get("infoHash") {
        r.torrent_hash = v.clone();
    }
    if let Some(v) = item.custom.get("seeds") {
        r.seeders = v.parse::<i64>().unwrap_or(0);
    }
    if let Some(v) = item.custom.get("peers") {
        r.leechers = v.parse::<i64>().unwrap_or(0) - r.seeders;
    }
    if let Some(v) = item.custom.get("magnetURI") {
        r.magnet_uri = v.clone();
    }
    if let Some(t) = item.custom.get("torrent") {
        // some feeds (the Aviztas network) put the size and hash inside a plain <torrent> element
        if let Ok(n) = super::xml::parse(&format!("<torrent>{t}</torrent>")) {
            let len =
                n.child("contentLength").or_else(|| n.child("contentlength")).and_then(|c| c.text.trim().parse::<i64>().ok()).unwrap_or(0);
            if len > 0 && len as u64 > r.size {
                r.size = len as u64;
            }
            let hash = n.child_text("infoHash");
            if r.torrent_hash.is_empty() && !hash.is_empty() {
                r.torrent_hash = hash;
            }
        }
    }
    if let Some(ext) = item.extensions.get("torrent") {
        let first = |k: &str| ext.get(k).and_then(|v| v.first()).map(|e| e.value.clone());
        if let Some(n) = first("contentLength").and_then(|v| v.parse::<u64>().ok()) {
            if n > r.size {
                r.size = n;
            }
        }
        if let Some(v) = first("contentLengthHR") {
            r.parse_size_bytes_string(&v);
        }
        if let Some(v) = first("infoHash") {
            r.torrent_hash = v;
        }
        if let Some(v) = first("magnetURI") {
            r.magnet_uri = v;
        }
        if let Some(n) = first("seeds").and_then(|v| v.parse::<u64>().ok()) {
            r.seeders = n as i64;
        }
        if let Some(n) = first("peers").and_then(|v| v.parse::<u64>().ok()) {
            r.leechers = n as i64 - r.seeders;
        }
        if let Some(n) = first("leechers").and_then(|v| v.parse::<u64>().ok()) {
            r.leechers = n as i64;
        }
    }
    if is_freeleech(&[&item.title, &item.description]) {
        r.freeleech = true;
        r.bonus = vec!["Freeleech".into()];
    }
    if !f.cookie.is_empty() {
        r.raw_cookie = f.cookie.clone();
    }
    if !f.user_agent.is_empty() {
        r.user_agent = f.user_agent.clone();
    }
    r
}

/// The indexer a feed release names.
impl Feed {
    /// The feed's indexer as releases carry it.
    pub fn indexer_minimal(&self) -> IndexerMinimal {
        IndexerMinimal {
            id: self.indexer.id,
            name: self.indexer.name.clone(),
            identifier: self.indexer.identifier.clone(),
            identifier_external: self.indexer.identifier_external.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/feed/torznab_test.go, newznab_test.go and rss_test.go.
    use super::super::{FeedIndexer, FeedSettings};
    use super::*;
    use chrono::{TimeZone, Utc};

    fn mock(settings: Option<FeedSettings>) -> Feed {
        Feed {
            indexer: FeedIndexer { name: "Mock Feed".into(), identifier: "mock-feed".into(), ..FeedIndexer::default() },
            settings,
            ..Feed::default()
        }
    }

    fn dt(t: &str) -> Option<FeedSettings> {
        Some(FeedSettings { download_type: t.into(), ..FeedSettings::default() })
    }

    #[test]
    fn test_torznab_source_to_release() {
        let magnet = "magnet:?xt=urn:btih:deadbeef";
        let proxy = "http://jackett:9117/dl/mock/?jackett_apikey=key&file=Some.Release";
        let torrent = "https://fake-feed.com/download/00000.torrent";
        let enc = |u: &str| Some(znab::Enclosure { url: u.into(), kind: "application/x-bittorrent".into(), ..znab::Enclosure::default() });
        let cases: Vec<(&str, Option<FeedSettings>, znab::Item, &str, &str)> = vec![
            (
                "no feed settings keeps the torrent url",
                None,
                znab::Item { link: torrent.into(), enclosure: enc(torrent), ..Default::default() },
                torrent,
                "",
            ),
            (
                "torrent type ignores the magneturl attr",
                dt("TORRENT"),
                znab::Item { link: torrent.into(), enclosure: enc(torrent), magnet_uri: magnet.into(), ..Default::default() },
                torrent,
                "",
            ),
            ("magnet in the link", dt("MAGNET"), znab::Item { link: magnet.into(), ..Default::default() }, "", magnet),
            (
                "magnet in the enclosure",
                dt("MAGNET"),
                znab::Item { link: proxy.into(), enclosure: enc(magnet), ..Default::default() },
                "",
                magnet,
            ),
            (
                "magneturl attr wins over the proxy link",
                dt("MAGNET"),
                znab::Item { link: proxy.into(), magnet_uri: magnet.into(), ..Default::default() },
                proxy,
                magnet,
            ),
            (
                "proxy link is kept for ResolveMagnetURI to follow",
                dt("MAGNET"),
                znab::Item { link: proxy.into(), ..Default::default() },
                proxy,
                proxy,
            ),
            (
                "magnet never stays in the download url",
                dt("TORRENT"),
                znab::Item { link: proxy.into(), enclosure: enc(magnet), ..Default::default() },
                "",
                magnet,
            ),
        ];
        for (name, settings, mut item, want_dl, want_magnet) in cases {
            item.title = "Some.Release.Title.2022.09.22.720p.WEB.h264-GROUP".into();
            let r = torznab_release(&mock(settings), &item);
            assert_eq!(r.download_url, want_dl, "{name}: download url");
            assert_eq!(r.magnet_uri, want_magnet, "{name}: magnet uri");
        }
    }

    #[test]
    fn test_torznab_source_to_release_metadata() {
        let pub_date = Utc.with_ymd_and_hms(2026, 9, 24, 5, 58, 24).unwrap();
        let r = torznab_release(
            &mock(None),
            &znab::Item {
                title: "That Show S01 2160p ATVP WEB-DL DDP 5.1 Atmos DV HEVC-NOGROUP".into(),
                tvdb_id: "12345".into(),
                imdb_id: "tt1234567".into(),
                tmdb_id: "54321".into(),
                pub_date: Some(pub_date),
                ..Default::default()
            },
        );
        assert_eq!((r.meta_tvdb, r.meta_imdb.as_str(), r.meta_tmdb), (12345, "tt1234567", 54321));
        assert_eq!(r.publish_date, Some(pub_date));
    }

    #[test]
    fn test_newznab_source_to_release() {
        let title = "Some.Release.Title.2022.09.22.720p.WEB.h264-GROUP";
        let guid = "https://indexer.local/details/abc";
        let nzb = "https://indexer.local/getnzb/abc.nzb";
        let enc = |u: &str, len: u64, t: &str| Some(znab::Enclosure { url: u.into(), length_n: len, kind: t.into(), ..Default::default() });
        let cat = |id: i64, n: &str| znab::Category { id, name: n.into(), sub_categories: vec![] };
        type Case<'a> = (&'a str, znab::Item, &'a str, u64, &'a str, Vec<&'a str>, i64);
        let cases: Vec<Case> = vec![
            (
                "nzb enclosure is the download url",
                znab::Item { size: 100, enclosure: enc(nzb, 200, "application/x-nzb"), ..Default::default() },
                nzb,
                100,
                "",
                vec![],
                0,
            ),
            (
                "enclosure length fills a missing size",
                znab::Item { enclosure: enc(nzb, 200, "application/x-nzb"), ..Default::default() },
                nzb,
                200,
                "",
                vec![],
                0,
            ),
            (
                "non nzb enclosure is ignored",
                znab::Item { enclosure: enc("https://indexer.local/x.torrent", 0, "application/x-bittorrent"), ..Default::default() },
                "",
                0,
                "",
                vec![],
                0,
            ),
            (
                "single category is set as the category",
                znab::Item { categories: vec![cat(5040, "TV/HD")], ..Default::default() },
                "",
                0,
                "TV/HD",
                vec!["TV/HD", "5040"],
                0,
            ),
            (
                "several categories only fill the list",
                znab::Item { categories: vec![cat(5000, "TV"), cat(5040, "TV/HD")], ..Default::default() },
                "",
                0,
                "",
                vec!["TV", "5000", "TV/HD", "5040"],
                0,
            ),
            ("tmdb id is parsed", znab::Item { tmdb_id: "1234".into(), ..Default::default() }, "", 0, "", vec![], 1234),
        ];
        for (name, mut item, want_dl, want_size, want_cat, want_cats, want_tmdb) in cases {
            item.title = title.into();
            item.guid = guid.into();
            let r = newznab_release(&mock(None), &item);
            assert_eq!((r.implementation.as_str(), r.protocol.as_str()), ("NEWZNAB", "usenet"), "{name}");
            assert_eq!((r.torrent_name.as_str(), r.info_url.as_str()), (title, guid), "{name}");
            assert_eq!(r.download_url, want_dl, "{name}");
            assert_eq!(r.size, want_size, "{name}");
            assert_eq!(r.category, want_cat, "{name}");
            assert_eq!(r.categories, want_cats, "{name}");
            assert_eq!(r.meta_tmdb, want_tmdb, "{name}");
        }
    }

    #[test]
    fn test_newznab_source_to_release_metadata() {
        let pub_date = Utc.with_ymd_and_hms(2026, 9, 24, 5, 58, 24).unwrap();
        let t = "That.Show.S01E01.1080p.WEB-DL.H.264-GROUP".to_string();
        let r = newznab_release(
            &mock(None),
            &znab::Item { title: t.clone(), tvdb_id: "12345".into(), pub_date: Some(pub_date), ..Default::default() },
        );
        assert_eq!((r.meta_tvdb, r.publish_date), (12345, Some(pub_date)));
        let r = newznab_release(&mock(None), &znab::Item { title: t, tvdb_id: "abc".into(), ..Default::default() });
        assert_eq!((r.meta_tvdb, r.publish_date), (0, None));
    }

    fn rss_feed() -> Feed {
        Feed {
            max_age: 3600,
            url: "https://fake-feed.com/rss".into(),
            indexer: FeedIndexer {
                name: "Mock Feed".into(),
                identifier: "mock-feed".into(),
                identifier_external: "Mock Indexer".into(),
                ..FeedIndexer::default()
            },
            ..Feed::default()
        }
    }

    #[test]
    fn test_rss_source_to_release() {
        let desc =
            "Category: Example\n Size: 1.49 GB\n Status: 27 seeders and 1 leechers\n Speed: 772.16 kB/s\n Added: 2022-09-29 16:06:08\n";
        let title = "Some.Release.Title.2022.09.22.720p.WEB.h264-GROUP";
        let base = |link: &str, desc: &str| gofeed::Item {
            title: title.into(),
            description: desc.into(),
            link: link.into(),
            guid: title.into(),
            ..Default::default()
        };
        // no_baseurl
        let r = rss_release(&rss_feed(), &base("/details.php?id=00000&hit=1", desc));
        assert_eq!(r.download_url, "https://fake-feed.com/details.php?id=00000&hit=1");
        assert_eq!((r.implementation.as_str(), r.protocol.as_str(), r.announce_type.as_str()), ("RSS", "torrent", "NEW"));
        assert_eq!(r.size, 1_490_000_000);
        assert_eq!(r.description, desc);
        assert_eq!((r.title.as_str(), r.year, r.month, r.day), ("Some Release Title", 2022, 9, 22));
        assert_eq!((r.resolution.as_str(), r.source.as_str(), r.group.as_str()), ("720p", "WEB", "GROUP"));
        assert_eq!(r.codec, vec!["H.264"]);
        assert_eq!(r.normalized_hash, "edfbe552ccde335f34b801e15930bc35");
        assert_eq!(r.indexer.identifier_external, "Mock Indexer");
        assert!(!r.freeleech && r.bonus.is_empty());
        // with_baseurl
        let r = rss_release(&rss_feed(), &base("https://fake-feed.com/details.php?id=00000&hit=1", desc));
        assert_eq!(r.download_url, "https://fake-feed.com/details.php?id=00000&hit=1");
        // unicode_escaped_url_chars (Go's string literal already holds the characters)
        let link = "https://fake-feed.com/download.php?id=00000&hit=1&type=torrent&name=Some%20Movie%20Title&hash=abc123@group:name#section%test+extra";
        let r = rss_release(&rss_feed(), &base(link, "Category: Example\n Size: 1.49 GB"));
        assert_eq!(r.download_url, link);
        assert_eq!(r.size, 1_490_000_000);
        // escaped as written in a feed, sanitize.URLEncoding reads them
        let r = rss_release(&rss_feed(), &base(r"https://fake-feed.com/download.php?id=1", ""));
        assert_eq!(r.download_url, "https://fake-feed.com/download.php?id=1");
    }

    #[test]
    fn test_rss_source_to_release_magnet() {
        let magnet = "magnet:?xt=urn:btih:deadbeef";
        let detail = "https://fake-feed.com/details.php?id=00000";
        let enc = vec![gofeed::Enclosure { url: magnet.into(), length: "1".into(), kind: "application/x-bittorrent".into() }];
        let cases: Vec<(&str, &str, gofeed::Item, &str, &str)> = vec![
            (
                "magnet type keeps the detail link as fallback",
                "MAGNET",
                gofeed::Item { link: detail.into(), enclosures: enc.clone(), ..Default::default() },
                detail,
                magnet,
            ),
            (
                "magnet in the link is not mangled into a relative url",
                "MAGNET",
                gofeed::Item { link: magnet.into(), ..Default::default() },
                "",
                magnet,
            ),
            (
                "magnet in the link is rescued without the magnet download type",
                "",
                gofeed::Item { link: magnet.into(), ..Default::default() },
                "",
                magnet,
            ),
            (
                "magnet in the enclosure is rescued without the magnet download type",
                "",
                gofeed::Item { enclosures: enc.clone(), ..Default::default() },
                "",
                magnet,
            ),
        ];
        for (name, t, mut item, want_dl, want_magnet) in cases {
            let mut f = rss_feed();
            f.settings = dt(t);
            item.title = "Some.Release.Title.2022.09.22.720p.WEB.h264-GROUP".into();
            let r = rss_release(&f, &item);
            assert_eq!(r.download_url, want_dl, "{name}: download url");
            assert_eq!(r.magnet_uri, want_magnet, "{name}: magnet uri");
        }
    }

    #[test]
    fn test_read_size_from_description() {
        let cases = [
            ("Size: 12GB", "12GB"),
            ("Size: 12 GB", "12GB"),
            ("Size: 12 GiB", "12GiB"),
            ("Size: 537 MiB", "537MiB"),
            ("<strong>Size</strong>: 20.48 GiB<br>", "20.48GiB"),
            ("file.name-GROUP / 20.48 GiB / x265", "20.48GiB"),
            ("<strong>Uploaded</strong>: 38 minutes ago<br>", "0B"),
            ("<strong>Uploaded</strong>: 38B minutes ago<br>Size: 32GB", "32GB"),
            (
                r#"<p> <strong>Name</strong>: One.S01E01.German.DL.DTS.1080p.BluRay.x265.10bit-Cats<br> <strong>Category</strong>: Anime Serien<br> <strong>Type</strong>: Encode<br> <strong>Resolution</strong>: 1080p<br> <strong>Size</strong>: 2.49 GiB<br> <strong>Uploaded</strong>: vor 3 Minuten<br> <strong>Seeders</strong>: 1 | <strong>Leechers</strong>: 7 | <strong>Completed</strong>: 0<br> <strong>Uploader</strong>: Hochgeladen von xxx <br> IMDB Link:<a href="https://anon.to?http://www.imdb.com/title/tt1" target="_blank">tt1</a><br> TMDB Link: <a href="https://anon.to?https://www.themoviedb.org/tv/1" target="_blank">1</a><br> </p>"#,
                "2.49GiB",
            ),
        ];
        for (s, want) in cases {
            let mut r = Release::default();
            read_size_from_description(s, &mut r);
            assert_eq!(r.size, humanize::parse_bytes(want).unwrap(), "{s}");
        }
    }

    #[test]
    fn freeleech_bonus() {
        assert_eq!(parse_freeleech_torznab(0.0), 100);
        assert_eq!(parse_freeleech_torznab(0.5), 50);
        assert_eq!(parse_freeleech_torznab(1.5), 0);
        assert_eq!(map_freeleech_to_bonus(75), ("Freeleech75".to_string(), true));
        assert_eq!(map_freeleech_to_bonus(30), ("Freeleech30".to_string(), false));
    }
}
