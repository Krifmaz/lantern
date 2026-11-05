// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/IndexerSearch/NewznabResults.cs), GPL-3.0.
//! The search answer apps read (`t=search`, `tvsearch`, `movie`, `music`, `book`): Prowlarr's RSS document, item by
//! item and attribute by attribute.

use super::nabxml::{double, Elem};
use super::release::{Protocol, Release};
use chrono::{DateTime, Utc};

/// Leaves out what XML can't hold and the control and non-characters Prowlarr removes from titles and descriptions
/// (`RemoveInvalidXMLChars`).
pub fn remove_invalid(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let u = *c as u32;
            !(u <= 0x08
                || u == 0x0B
                || u == 0x0C
                || (0x0E..=0x1F).contains(&u)
                || (0x7F..=0x9F).contains(&u)
                || matches!(u, 0xFEFF | 0xFFFE | 0xFFFF))
        })
        .collect()
}

/// A date as Prowlarr writes it (`Sat, 14 Mar 2015 17:10:42 +0000`), in UTC as in its container.
pub fn xml_date(d: DateTime<Utc>) -> String {
    d.format("%a, %d %b %Y %H:%M:%S +0000").to_string()
}

/// The peers of a release: what the site said, else seeders and leechers together.
fn peers(r: &Release) -> Option<i64> {
    r.peers.or(match (r.seeders, r.leechers) {
        (None, None) => None,
        (s, l) => Some(s.unwrap_or(0) + l.unwrap_or(0)),
    })
}

/// The RSS document for `releases`, whose `link` and `magnet` are already the addresses apps fetch them at
/// (`ToXml`). `protocol` picks the attribute namespace and file type for every item, as Prowlarr does for one
/// indexer; `None` (all indexers at once, a Lantern extra) declares both and uses each release's own. `title` is the
/// channel's name (Prowlarr writes its own).
pub fn to_xml(title: &str, releases: &[Release], protocol: Option<Protocol>, prefer_magnet: bool, now: DateTime<Utc>) -> String {
    let mut rss = Elem::new("rss").attr("version", "1.0").attr("xmlns:atom", "http://www.w3.org/2005/Atom");
    if protocol != Some(Protocol::Usenet) {
        rss = rss.attr("xmlns:torznab", "http://torznab.com/schemas/2015/feed");
    }
    if protocol != Some(Protocol::Torrent) {
        rss = rss.attr("xmlns:newznab", "http://www.newznab.com/DTD/2010/feeds/attributes/");
    }
    let mut channel = Elem::new("channel")
        .child(Elem::new("atom:link").attr("rel", "self").attr("type", "application/rss+xml"))
        .child(Elem::new("title").text(Some(title)));
    for r in releases {
        channel = channel.child(item(r, protocol.unwrap_or(r.protocol), prefer_magnet, now));
    }
    rss.child(channel).document()
}

/// One `<item>`.
fn item(r: &Release, protocol: Protocol, prefer_magnet: bool, now: DateTime<Utc>) -> Elem {
    let torrent = r.protocol == Protocol::Torrent;
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let (download, magnet) = (opt(&r.link), if torrent { opt(&r.magnet) } else { None });
    let link = if prefer_magnet { magnet.clone().or(download.clone()) } else { download.clone().or(magnet.clone()) };
    let link = link.unwrap_or_default();
    let ns = if protocol == Protocol::Torrent { "torznab:attr" } else { "newznab:attr" };
    let attr = |name: &str, value: &str| Elem::new(ns).attr("name", name).attr("value", value);
    // Prowlarr's IndexerPrivacy: Public (its default), SemiPrivate, Private
    let privacy = match r.indexer_privacy.as_str() {
        "private" => "private",
        "semi-private" | "semiprivate" => "semi-private",
        _ => "public",
    };
    let mut e = Elem::new("item")
        .child(Elem::new("title").text(Some(&remove_invalid(&r.title))))
        .child(Elem::new("description").text(opt(&r.description).map(|d| remove_invalid(&d)).as_deref()))
        .child(Elem::new("guid").text(Some(&r.guid)))
        .child(Elem::new("prowlarrindexer").attr("id", &r.indexer_id.to_string()).attr("type", privacy).text(opt(&r.indexer).as_deref()));
    if !r.details.is_empty() {
        e = e.child(Elem::new("comments").text(Some(&r.details)));
    }
    e = e
        .child(Elem::new("pubDate").text(Some(&xml_date(r.date.unwrap_or(now)))))
        .child(Elem::new("size").text(r.size.map(|s| s.to_string()).as_deref()))
        .child(Elem::new("link").text(Some(&link)))
        .children(r.categories.iter().map(|c| Elem::new("category").text(Some(&c.to_string()))));
    let mut enc = Elem::new("enclosure").attr("url", &link);
    if let Some(s) = r.size {
        enc = enc.attr("length", &s.to_string());
    }
    e = e.child(enc.attr("type", if protocol == Protocol::Torrent { "application/x-bittorrent" } else { "application/x-nzb" }));
    let mut attrs: Vec<Elem> = Vec::new();
    attrs.extend(r.categories.iter().map(|c| attr("category", &c.to_string())));
    attrs.extend(r.flags.iter().map(|f| attr("tag", f)));
    attrs.extend(r.languages.iter().map(|l| attr("language", l)));
    attrs.extend(r.subs.iter().map(|l| attr("subs", l)));
    attrs.push(attr("genre", &r.genre));
    let id = |name: &str, v: Option<i64>| v.filter(|n| *n != 0).map(|n| attr(name, &n.to_string()));
    attrs.extend(id("rageid", r.rage));
    attrs.extend(id("tvdbid", r.tvdb));
    attrs.extend(r.imdb.filter(|n| *n != 0).map(|n| attr("imdb", &format!("{n:07}"))));
    attrs.extend(id("tmdbid", r.tmdb));
    attrs.extend(id("traktid", r.trakt));
    attrs.extend(id("tvmazeid", r.tvmaze));
    attrs.extend(id("doubanid", r.douban));
    let t = |v: Option<i64>| if torrent { v } else { None };
    attrs.extend(t(r.seeders).map(|v| attr("seeders", &v.to_string())));
    attrs.extend(r.files.map(|v| attr("files", &v.to_string())));
    attrs.extend(r.grabs.map(|v| attr("grabs", &v.to_string())));
    attrs.extend(t(peers(r)).map(|v| attr("peers", &v.to_string())));
    attrs.extend(id("year", r.year));
    for (name, v) in [
        ("author", &r.author),
        ("booktitle", &r.book_title),
        ("artist", &r.artist),
        ("album", &r.album),
        ("label", &r.label),
        ("track", &r.track),
    ] {
        if !v.is_empty() {
            attrs.push(attr(name, &remove_invalid(v)));
        }
    }
    if torrent {
        if !r.infohash.is_empty() {
            attrs.push(attr("infohash", &remove_invalid(&r.infohash)));
        }
        attrs.extend(r.minimum_ratio.map(|v| attr("minimumratio", &double(v))));
        attrs.extend(r.minimum_seed_time.map(|v| attr("minimumseedtime", &v.to_string())));
        attrs.extend(r.download_factor.map(|v| attr("downloadvolumefactor", &double(v))));
        attrs.extend(r.upload_factor.map(|v| attr("uploadvolumefactor", &double(v))));
    }
    if !r.poster.is_empty() {
        attrs.push(attr("coverurl", &r.poster));
    }
    e.children(attrs)
}

/// The answer Prowlarr gives to a search of indexer 0 (its test indexer): one `Test Release`.
pub fn test_results(title: &str, now: DateTime<Utc>) -> String {
    let r = Release {
        title: "Test Release".into(),
        guid: "https://prowlarr.com".into(),
        link: "https://prowlarr.com".into(),
        date: Some(now),
        protocol: Protocol::Usenet,
        ..Release::default()
    };
    to_xml(title, &[r], Some(Protocol::Usenet), false, now)
}
