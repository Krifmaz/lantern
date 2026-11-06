// Written to read feeds the way mmcdole/gofeed (as autobrr v1.90.0 uses it) does; GPL-2.0-or-later with the rest
// of the autobrr port.
//! RSS 2.0, RSS 1.0 (RDF) and Atom read into gofeed's universal shape: each item's title, description, link,
//! guid, publish date, authors, categories and enclosures, its unknown plain elements as `custom` (their inner
//! markup) and its namespaced elements as `extensions` (by prefix, then name), which autobrr reads sizes,
//! seeders and magnet links from.

use super::xml::{self, Node};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// A person (gofeed's `Person`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Person {
    /// The name.
    pub name: String,
    /// The email address.
    pub email: String,
}

/// An enclosure (gofeed's `Enclosure`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Enclosure {
    /// The address.
    pub url: String,
    /// The length as written.
    pub length: String,
    /// The media type.
    pub kind: String,
}

/// A namespaced element (gofeed's `ext.Extension`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extension {
    /// Its name.
    pub name: String,
    /// Its text.
    pub value: String,
    /// Its attributes.
    pub attrs: BTreeMap<String, String>,
}

/// The extensions by prefix, then name.
pub type Extensions = BTreeMap<String, BTreeMap<String, Vec<Extension>>>;

/// An item (gofeed's `Item`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    /// The title.
    pub title: String,
    /// The description.
    pub description: String,
    /// The full content (`content:encoded`, Atom's `content`).
    pub content: String,
    /// The link.
    pub link: String,
    /// The publish date as written.
    pub published: String,
    /// The publish date, when it could be read.
    pub published_parsed: Option<DateTime<Utc>>,
    /// The authors.
    pub authors: Vec<Person>,
    /// The guid (Atom's id).
    pub guid: String,
    /// The categories.
    pub categories: Vec<String>,
    /// The enclosures.
    pub enclosures: Vec<Enclosure>,
    /// Plain elements gofeed doesn't know, by name: their inner markup.
    pub custom: BTreeMap<String, String>,
    /// Namespaced elements.
    pub extensions: Extensions,
}

/// A feed (gofeed's `Feed`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feed {
    /// The title.
    pub title: String,
    /// The description.
    pub description: String,
    /// The link.
    pub link: String,
    /// The items, as listed.
    pub items: Vec<Item>,
    /// `rss` or `atom`.
    pub feed_type: String,
    /// The format's version.
    pub feed_version: String,
}

/// gofeed's "could not tell which kind of feed this is".
pub const ERR_FEED_TYPE_NOT_DETECTED: &str = "Failed to detect feed type";

/// A date in any of the usual feed formats (gofeed's `ParseDate`, the common layouts).
pub fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(s) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    for f in ["%a, %d %b %Y %H:%M:%S %z", "%d %b %Y %H:%M:%S %z", "%Y-%m-%dT%H:%M:%S%z", "%Y-%m-%d %H:%M:%S %z"] {
        if let Ok(d) = DateTime::parse_from_str(s, f) {
            return Some(d.with_timezone(&Utc));
        }
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%a, %d %b %Y %H:%M:%S"] {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(d.and_utc());
        }
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)).map(|d| d.and_utc())
}

/// An RSS author: `email (name)`, a name, or an address (gofeed's `ParseNameAddress`).
fn person(s: &str) -> Person {
    let s = s.trim();
    if let Some(open) = s.find('(') {
        if s.ends_with(')') {
            return Person { email: s[..open].trim().to_string(), name: s[open + 1..s.len() - 1].trim().to_string() };
        }
    }
    if s.contains('@') && !s.contains(' ') {
        return Person { email: s.to_string(), ..Person::default() };
    }
    Person { name: s.to_string(), ..Person::default() }
}

/// Element `n` as an extension.
fn extension(n: &Node) -> Extension {
    Extension { name: n.name.clone(), value: n.text.trim().to_string(), attrs: n.attrs.iter().cloned().collect() }
}

/// Adds `n` to `ext` under its prefix.
fn add_extension(ext: &mut Extensions, n: &Node) {
    ext.entry(n.prefix.clone()).or_default().entry(n.name.clone()).or_default().push(extension(n));
}

/// The first value of extension `prefix:name`.
fn ext_value(ext: &Extensions, prefix: &str, name: &str) -> Option<String> {
    ext.get(prefix).and_then(|m| m.get(name)).and_then(|v| v.first()).map(|e| e.value.clone())
}

/// gofeed's `ParseText` of a custom element: its inner markup, trimmed, entities read.
fn custom_text(n: &Node) -> String {
    let inner = n.inner.trim();
    if inner.contains("<![CDATA[") || !inner.contains('<') {
        return n.text.trim().to_string();
    }
    html_escape::decode_html_entities(inner).to_string()
}

/// An RSS item.
fn rss_item(n: &Node) -> Item {
    let mut it = Item::default();
    let mut author = String::new();
    for c in &n.children {
        if !c.prefix.is_empty() {
            if c.prefix == "content" && c.name == "encoded" {
                it.content = c.text.trim().to_string();
            } else {
                add_extension(&mut it.extensions, c);
            }
            continue;
        }
        let text = c.text.trim().to_string();
        match c.name.as_str() {
            "title" => it.title = text,
            "description" => it.description = text,
            "link" => it.link = text,
            "author" => author = text,
            "pubDate" => it.published = text,
            "guid" => it.guid = text,
            "category" => it.categories.push(text),
            "enclosure" => it.enclosures.push(Enclosure {
                url: c.attr("url").unwrap_or("").to_string(),
                length: c.attr("length").unwrap_or("").to_string(),
                kind: c.attr("type").unwrap_or("").to_string(),
            }),
            "comments" | "source" => {}
            _ => {
                it.custom.insert(c.name.clone(), custom_text(c));
            }
        }
    }
    if author.is_empty() {
        author = ext_value(&it.extensions, "dc", "creator").unwrap_or_default();
    }
    if !author.is_empty() {
        it.authors.push(person(&author));
    }
    if it.published.is_empty() {
        it.published = ext_value(&it.extensions, "dc", "date").unwrap_or_default();
    }
    if let Some(subjects) = it.extensions.get("dc").and_then(|m| m.get("subject")) {
        it.categories.extend(subjects.iter().map(|e| e.value.clone()));
    }
    it.published_parsed = parse_date(&it.published);
    it
}

/// An Atom entry.
fn atom_entry(n: &Node) -> Item {
    let mut it = Item::default();
    let mut updated = String::new();
    for c in &n.children {
        if !c.prefix.is_empty() && c.prefix != "atom" {
            add_extension(&mut it.extensions, c);
            continue;
        }
        let text = c.text.trim().to_string();
        match c.name.as_str() {
            "title" => it.title = text,
            "summary" => it.description = text,
            "content" => it.content = text,
            "id" => it.guid = text,
            "published" => it.published = text,
            "updated" => updated = text,
            "author" => {
                it.authors.push(Person { name: c.child_text("name").trim().to_string(), email: c.child_text("email").trim().to_string() })
            }
            "category" => it.categories.push(c.attr("term").unwrap_or("").to_string()),
            "link" => {
                let href = c.attr("href").unwrap_or("").to_string();
                match c.attr("rel").unwrap_or("alternate") {
                    "enclosure" => it.enclosures.push(Enclosure {
                        url: href,
                        length: c.attr("length").unwrap_or("").to_string(),
                        kind: c.attr("type").unwrap_or("").to_string(),
                    }),
                    "alternate" if it.link.is_empty() => it.link = href,
                    _ => {}
                }
            }
            _ => {
                it.custom.insert(c.name.clone(), custom_text(c));
            }
        }
    }
    if it.published.is_empty() {
        it.published = updated;
    }
    it.published_parsed = parse_date(&it.published);
    it
}

/// Reads `body` as a feed (gofeed's `Parser.Parse`).
pub fn parse(body: &str) -> Result<Feed, String> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        return parse_json(trimmed);
    }
    let root = xml::parse(body).map_err(|_| ERR_FEED_TYPE_NOT_DETECTED.to_string())?;
    match root.name.as_str() {
        "rss" => {
            let ch = root.child("channel").cloned().unwrap_or_default();
            let link = ch.all("link").find(|l| l.prefix.is_empty()).map(|l| l.text.trim().to_string()).unwrap_or_default();
            let items = ch.all("item").map(rss_item).collect();
            Ok(Feed {
                title: ch.child_text("title").trim().to_string(),
                description: ch.child_text("description").trim().to_string(),
                link,
                items,
                feed_type: "rss".into(),
                feed_version: root.attr("version").unwrap_or("2.0").to_string(),
            })
        }
        "RDF" => {
            let ch = root.child("channel").cloned().unwrap_or_default();
            Ok(Feed {
                title: ch.child_text("title").trim().to_string(),
                description: ch.child_text("description").trim().to_string(),
                link: ch.child_text("link").trim().to_string(),
                items: root.all("item").map(rss_item).collect(),
                feed_type: "rss".into(),
                feed_version: "1.0".into(),
            })
        }
        "feed" => Ok(Feed {
            title: root.child_text("title").trim().to_string(),
            description: root.child_text("subtitle").trim().to_string(),
            link: root
                .all("link")
                .find(|l| l.attr("rel").unwrap_or("alternate") == "alternate")
                .and_then(|l| l.attr("href"))
                .unwrap_or("")
                .to_string(),
            items: root.all("entry").map(atom_entry).collect(),
            feed_type: "atom".into(),
            feed_version: "1.0".into(),
        }),
        _ => Err(ERR_FEED_TYPE_NOT_DETECTED.into()),
    }
}

/// A JSON Feed.
fn parse_json(body: &str) -> Result<Feed, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| ERR_FEED_TYPE_NOT_DETECTED.to_string())?;
    if !v.get("version").and_then(Value::as_str).is_some_and(|s| s.contains("jsonfeed.org")) {
        return Err(ERR_FEED_TYPE_NOT_DETECTED.into());
    }
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let items = v
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|i| {
            let published = s(i, "date_published");
            Item {
                title: s(i, "title"),
                description: s(i, "summary"),
                content: s(i, "content_html"),
                link: s(i, "url"),
                published_parsed: parse_date(&published),
                published,
                guid: s(i, "id"),
                categories: i
                    .get("tags")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect(),
                enclosures: i
                    .get("attachments")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|a| Enclosure {
                        url: s(a, "url"),
                        length: a.get("size_in_bytes").map(|n| n.to_string()).unwrap_or_default(),
                        kind: s(a, "mime_type"),
                    })
                    .collect(),
                ..Item::default()
            }
        })
        .collect();
    Ok(Feed {
        title: s(&v, "title"),
        description: s(&v, "description"),
        link: s(&v, "home_page_url"),
        items,
        feed_type: "json".into(),
        feed_version: "1.1".into(),
    })
}

/// Inserts `v` under `k` unless it is empty (Go's `omitempty`).
fn put(m: &mut Map<String, Value>, k: &str, v: Value) {
    let empty = match &v {
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Null => true,
        _ => false,
    };
    if !empty {
        m.insert(k.into(), v);
    }
}

impl Feed {
    /// The feed as gofeed's `Feed.String` writes it (indented JSON), kept as the last run's data.
    pub fn string(&self) -> String {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|i| {
                let mut m = Map::new();
                put(&mut m, "title", json!(i.title));
                put(&mut m, "description", json!(i.description));
                put(&mut m, "content", json!(i.content));
                put(&mut m, "link", json!(i.link));
                put(&mut m, "published", json!(i.published));
                if let Some(p) = i.published_parsed {
                    m.insert("publishedParsed".into(), json!(p.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)));
                }
                put(
                    &mut m,
                    "authors",
                    Value::Array(
                        i.authors
                            .iter()
                            .map(|a| {
                                let mut p = Map::new();
                                put(&mut p, "name", json!(a.name));
                                put(&mut p, "email", json!(a.email));
                                Value::Object(p)
                            })
                            .collect(),
                    ),
                );
                put(&mut m, "guid", json!(i.guid));
                put(&mut m, "categories", json!(i.categories));
                put(
                    &mut m,
                    "enclosures",
                    Value::Array(
                        i.enclosures
                            .iter()
                            .map(|e| {
                                let mut p = Map::new();
                                put(&mut p, "url", json!(e.url));
                                put(&mut p, "length", json!(e.length));
                                put(&mut p, "type", json!(e.kind));
                                Value::Object(p)
                            })
                            .collect(),
                    ),
                );
                let ext: Map<String, Value> = i
                    .extensions
                    .iter()
                    .map(|(p, names)| {
                        let inner: Map<String, Value> = names
                            .iter()
                            .map(|(n, list)| {
                                let v: Vec<Value> = list
                                    .iter()
                                    .map(|e| {
                                        let mut o = Map::new();
                                        o.insert("name".into(), json!(e.name));
                                        o.insert("value".into(), json!(e.value));
                                        o.insert("attrs".into(), json!(e.attrs));
                                        o.insert("children".into(), json!({}));
                                        Value::Object(o)
                                    })
                                    .collect();
                                (n.clone(), Value::Array(v))
                            })
                            .collect();
                        (p.clone(), Value::Object(inner))
                    })
                    .collect();
                put(&mut m, "extensions", Value::Object(ext));
                put(&mut m, "custom", json!(i.custom));
                Value::Object(m)
            })
            .collect();
        let mut m = Map::new();
        put(&mut m, "title", json!(self.title));
        put(&mut m, "description", json!(self.description));
        put(&mut m, "link", json!(self.link));
        m.insert("items".into(), Value::Array(items));
        m.insert("feedType".into(), json!(self.feed_type));
        m.insert("feedVersion".into(), json!(self.feed_version));
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        let _ = serde::Serialize::serialize(&Value::Object(m), &mut ser);
        String::from_utf8(out).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rss_items_with_custom_elements_and_extensions() {
        let body = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:torrent="https://example/torrent" xmlns:dc="http://purl.org/dc/elements/1.1/">
<channel><title>T</title>
<item><title>A</title><guid>g</guid><link>https://x/1</link><pubDate>Mon, 05 Oct 2026 10:00:00 GMT</pubDate>
<size>1234</size><torrent><contentLength>99</contentLength><infoHash>abc</infoHash></torrent>
<torrent:seeds>5</torrent:seeds><dc:creator>me</dc:creator><category>TV</category>
<enclosure url="https://x/1.torrent" length="10" type="application/x-bittorrent"/></item>
</channel></rss>"#;
        let f = parse(body).unwrap();
        let it = &f.items[0];
        assert_eq!(it.custom["size"], "1234");
        assert_eq!(it.custom["torrent"], "<contentLength>99</contentLength><infoHash>abc</infoHash>");
        assert_eq!(it.extensions["torrent"]["seeds"][0].value, "5");
        assert_eq!(it.authors[0].name, "me");
        assert_eq!(it.categories, vec!["TV"]);
        assert!(it.published_parsed.is_some());
        assert!(f.string().contains("\"title\": \"A\""));
        assert_eq!(parse("<html/>").unwrap_err(), ERR_FEED_TYPE_NOT_DETECTED);
    }

    #[test]
    fn reads_atom_entries() {
        let body = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>T</title><entry><title>B</title><id>i</id>
<link href="https://x/b"/><link rel="enclosure" href="https://x/b.torrent" type="application/x-bittorrent"/>
<updated>2026-10-05T10:00:00Z</updated></entry></feed>"#;
        let f = parse(body).unwrap();
        assert_eq!((f.items[0].link.as_str(), f.items[0].enclosures[0].url.as_str()), ("https://x/b", "https://x/b.torrent"));
        assert!(f.items[0].published_parsed.is_some());
    }
}
