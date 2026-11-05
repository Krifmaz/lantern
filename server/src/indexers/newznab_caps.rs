// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Indexers/Definitions/Newznab/NewznabCapabilitiesProvider.cs),
// GPL-3.0.

//! The capabilities of a Newznab or Torznab API, asked from the API itself (`t=caps`) as Prowlarr's
//! `NewznabCapabilitiesProvider` does: its page sizes, the searches and parameters it takes, and its categories
//! mapped onto the standard ones (by name first, then by number). An answer is kept 7 days per set of settings.

use super::capabilities::{Capabilities, Category, BOOK_PARAMS, MOVIE_PARAMS, MUSIC_PARAMS, TV_PARAMS};
use super::categories;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;

/// How long an answer is kept (`TimeSpan.FromDays(7)`), in seconds.
pub const KEEP_SECS: f64 = 7.0 * 86400.0;

/// The answers kept: settings fingerprint → (when it was asked, the answer's text).
#[derive(Debug, Default)]
pub struct Cache {
    entries: HashMap<String, (f64, String)>,
}

impl Cache {
    /// The answer kept for `key` that is younger than 7 days at `now`.
    pub fn fresh(&self, key: &str, now: f64) -> Option<&str> {
        self.entries.get(key).filter(|(at, _)| now - at < KEEP_SECS).map(|(_, x)| x.as_str())
    }

    /// The answer kept for `key`, however old.
    pub fn any(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|(_, x)| x.as_str())
    }

    /// Keeps `xml` for `key`, asked at `now`.
    pub fn put(&mut self, key: &str, now: f64, xml: &str) {
        self.entries.insert(key.to_string(), (now, xml.to_string()));
    }
}

/// Why an answer can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapsError {
    /// It isn't XML, or not a capabilities answer (`XmlException`): the problem in words.
    Xml(String),
    /// The API answered with an error (`NewznabRssParser.CheckError`): its description.
    Api(String),
}

impl std::fmt::Display for CapsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapsError::Xml(m) | CapsError::Api(m) => f.write_str(m),
        }
    }
}

/// One XML element, its attributes and the elements in it.
#[derive(Debug, Default, Clone)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
}

impl Element {
    /// The first element in it called `name` (`XElement.Element`).
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The elements in it called `name` (`XElement.Elements`).
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Attribute `name`, when it has one.
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// The root element of `text` (`XDocument.Parse`).
fn parse_xml(text: &str) -> Result<Element, CapsError> {
    let mut r = Reader::from_str(text.trim_start_matches('\u{feff}'));
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let bad = |e: String| CapsError::Xml(format!("The capabilities answer isn't valid XML: {e}"));
    let element = |e: &quick_xml::events::BytesStart| -> Result<Element, CapsError> {
        let mut attrs = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|e| bad(e.to_string()))?;
            let v = a.unescape_value().map_err(|e| bad(e.to_string()))?;
            attrs.push((String::from_utf8_lossy(a.key.as_ref()).into_owned(), v.into_owned()));
        }
        Ok(Element { name: String::from_utf8_lossy(e.name().as_ref()).into_owned(), attrs, children: Vec::new() })
    };
    loop {
        match r.read_event().map_err(|e| bad(e.to_string()))? {
            Event::Start(e) => stack.push(element(&e)?),
            Event::Empty(e) => {
                let el = element(&e)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None if root.is_none() => root = Some(el),
                    None => return Err(bad("there is more than one root element".into())),
                }
            }
            Event::End(_) => {
                let el = stack.pop().ok_or_else(|| bad("an element is closed that wasn't opened".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None if root.is_none() => root = Some(el),
                    None => return Err(bad("there is more than one root element".into())),
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(bad("an element isn't closed".into()));
    }
    root.ok_or_else(|| bad("there is no root element".into()))
}

/// The parameters in `list` (comma separated) that are among `allowed`, by name or number, case ignored, each once
/// (`Enum.TryParse` of each).
fn params(into: &mut Vec<&'static str>, list: &str, allowed: &[&'static str]) {
    for p in list.split(',').map(str::trim) {
        let found = allowed
            .iter()
            .find(|a| a.eq_ignore_ascii_case(p))
            .copied()
            .or_else(|| p.parse::<usize>().ok().and_then(|i| allowed.get(i).copied()));
        if let Some(f) = found {
            if !into.contains(&f) {
                into.push(f);
            }
        }
    }
}

/// The capabilities in a `t=caps` answer (`ParseCapabilities`). Text that isn't XML, an error answer or another
/// kind of answer is an error; a capabilities answer with parts that can't be read gives the capabilities every
/// indexer starts with, as in Prowlarr.
pub fn parse(text: &str) -> Result<Capabilities, CapsError> {
    let root = parse_xml(text)?;
    if root.name == "error" {
        let desc = root.attr("description").unwrap_or("").to_string();
        return Err(CapsError::Api(desc));
    }
    if root.name != "caps" {
        return Err(CapsError::Xml("The capabilities answer isn't a Newznab/Torznab capabilities answer (Unexpected XML)".into()));
    }
    Ok(read(&root).unwrap_or_default())
}

/// The capabilities of `caps`, or `None` when a part of it can't be read (`int.Parse` or a missing attribute).
fn read(caps: &Element) -> Option<Capabilities> {
    let mut c = Capabilities::default();
    if let Some(l) = caps.child("limits") {
        c.limits_default = Some(l.attr("default")?.trim().parse().ok()?);
        c.limits_max = Some(l.attr("max")?.trim().parse().ok()?);
    }
    if let Some(s) = caps.child("searching") {
        // each kind: none unless available="yes"; its listed parameters, else only `q`
        let mode = |name: &str, into: &mut Vec<&'static str>, allowed: &[&'static str]| -> Option<bool> {
            match s.child(name) {
                None => {
                    into.clear();
                    Some(false)
                }
                Some(el) if el.attr("available")? != "yes" => {
                    into.clear();
                    Some(false)
                }
                Some(el) => {
                    match el.attr("supportedParams") {
                        Some(list) => params(into, list, allowed),
                        None => *into = vec!["q"],
                    }
                    Some(el.attr("supportedParams").is_some())
                }
            }
        };
        if mode("search", &mut c.search_params, &["q"])? {
            c.supports_raw_search = s.child("search").and_then(|e| e.attr("searchEngine")) == Some("raw");
        }
        mode("movie-search", &mut c.movie_params, &MOVIE_PARAMS)?;
        mode("tv-search", &mut c.tv_params, &TV_PARAMS)?;
        mode("audio-search", &mut c.music_params, &MUSIC_PARAMS)?;
        mode("book-search", &mut c.book_params, &BOOK_PARAMS)?;
    }
    if let Some(cats) = caps.child("categories") {
        for cat in cats.all("category") {
            let parent_name = cat.attr("name")?.to_string();
            let parent_id: i64 = cat.attr("id")?.trim().parse().ok()?;
            let mapped = map_parent(&parent_name, parent_id);
            for sub in cat.all("subcat") {
                let sub_name = sub.attr("name")?;
                let sub_id: i64 = sub.attr("id")?.trim().parse().ok()?;
                let mapped_sub = map_sub(&mapped, sub_name, sub_id);
                c.categories.add_mapping(&sub_id.to_string(), &mapped_sub, Some(&format!("{parent_name}/{sub_name}")));
            }
            c.categories.add_mapping(&parent_id.to_string(), &mapped, Some(&parent_name));
        }
    }
    Some(c)
}

/// A standard category as a [`Category`].
fn standard(id: u32) -> Category {
    Category::new(id, categories::name_of(id).unwrap_or(""))
}

/// The standard parent categories (`ParentCats`).
fn parents() -> impl Iterator<Item = (u32, &'static str)> {
    categories::PARENTS.iter().filter_map(|id| categories::name_of(*id).map(|n| (*id, n)))
}

/// The standard category for an API's category `name` with number `id`: a parent by name, a standard category
/// with that number whose name contains it, a parent whose name it contains, a parent by number, else Other.
fn map_parent(name: &str, id: i64) -> Category {
    let lower = name.to_lowercase();
    let found = parents()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .or_else(|| categories::STANDARD.iter().copied().find(|(i, n)| *i as i64 == id && n.to_lowercase().contains(&lower)))
        .or_else(|| parents().find(|(_, n)| lower.contains(&n.to_lowercase())))
        .or_else(|| parents().find(|(i, _)| *i as i64 == id));
    match found {
        Some((i, n)) => Category::new(i, n),
        None => standard(8000),
    }
}

/// The standard category for sub-category `name` (number `id`) of `parent`: `<parent>/<name>` by name, a standard
/// category by number, `<parent>/Other` (unless the parent is Other), else Other/Misc.
fn map_sub(parent: &Category, name: &str, id: i64) -> Category {
    let by_name = |want: String| categories::STANDARD.iter().copied().find(|(_, n)| n.to_lowercase() == want);
    let found = by_name(format!("{}/{}", parent.name, name).to_lowercase())
        .or_else(|| categories::STANDARD.iter().copied().find(|(i, _)| *i as i64 == id))
        .or_else(|| if parent.id != 8000 { by_name(format!("{}/other", parent.name.to_lowercase())) } else { None });
    match found {
        Some((i, n)) => Category::new(i, n),
        None => standard(8010),
    }
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/IndexerTests/NewznabTests/NewznabCapabilitiesProviderFixture.cs).
    use super::*;

    const CAPS: &str = include_str!("../../tests/fixtures/upstream/prowlarr/newznab_caps.xml");
    const ANIMETOSHO: &str = include_str!("../../tests/fixtures/upstream/prowlarr/torznab_animetosho_caps.xml");

    #[test]
    fn should_report_pagesize() {
        let caps = parse(CAPS).unwrap();
        assert_eq!(caps.limits_default, Some(25));
        assert_eq!(caps.limits_max, Some(60));
    }

    #[test]
    fn should_map_different_categories() {
        let caps = parse(CAPS).unwrap();
        let book_cats = caps.categories.map_to_trackers(&[7000], false);
        assert_eq!(book_cats.len(), 2);
        assert!(book_cats.contains(&"8000".to_string()));
    }

    #[test]
    fn should_find_sub_categories_as_main_categories() {
        for category in [5070u32, 2020] {
            let caps = parse(ANIMETOSHO).unwrap();
            let found = caps.categories.map_tracker(&category.to_string());
            assert_eq!(found.len(), 2, "{category}");
            assert_eq!(found[0], category);
        }
    }

    #[test]
    fn should_map_by_name_when_available() {
        let caps = parse(CAPS).unwrap();
        let found = caps.categories.map_tracker("5999");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], 5050);
    }

    #[test]
    fn should_use_default_pagesize_if_missing() {
        let caps = parse(&CAPS.replace("<limits", "<abclimits")).unwrap();
        assert_eq!(caps.limits_default, Some(100));
        assert_eq!(caps.limits_max, Some(100));
    }

    #[test]
    fn should_throw_if_xml_invalid() {
        assert!(matches!(parse(&CAPS.replace("<limits", "<>")), Err(CapsError::Xml(_))));
    }

    #[test]
    fn should_not_throw_on_xml_data_unexpected() {
        assert!(parse(&CAPS.replace("5030", "asdf")).is_ok());
    }

    #[test]
    fn searches_and_parameters_follow_the_answer() {
        let caps = parse(CAPS).unwrap();
        // available without a list: only q
        assert_eq!((caps.search_params.clone(), caps.tv_params.clone(), caps.movie_params.clone()), (vec!["q"], vec!["q"], vec!["q"]));
        assert_eq!((caps.music_params.clone(), caps.book_params.clone()), (vec!["q"], vec![]));
        let caps = parse(ANIMETOSHO).unwrap();
        assert!(caps.tv_params.is_empty() && caps.movie_params.is_empty());
        let caps = parse(
            r#"<caps><searching><search available="yes" supportedParams="q" searchEngine="raw"/>
            <tv-search available="yes" supportedParams="q,season,ep,tvdbid,TVMazeId,nope"/></searching></caps>"#,
        )
        .unwrap();
        assert!(caps.supports_raw_search);
        assert_eq!(caps.tv_params, vec!["q", "season", "ep", "tvdbid", "tvmazeid"]);
        assert_eq!(
            parse(r#"<error code="100" description="Incorrect user credentials"/>"#).unwrap_err(),
            CapsError::Api("Incorrect user credentials".into())
        );
        assert!(matches!(parse("<rss><channel/></rss>"), Err(CapsError::Xml(_))));
    }

    #[test]
    fn answers_are_kept_seven_days() {
        let mut c = Cache::default();
        c.put("k", 1000.0, "<caps/>");
        assert_eq!(c.fresh("k", 1000.0 + KEEP_SECS - 1.0), Some("<caps/>"));
        assert_eq!(c.fresh("k", 1000.0 + KEEP_SECS), None);
        assert_eq!(c.any("k"), Some("<caps/>"));
    }
}
