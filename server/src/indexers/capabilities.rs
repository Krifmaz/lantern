// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Indexers/IndexerCapabilities.cs,
// src/NzbDrone.Core/Indexers/IndexerCapabilitiesCategories.cs, src/NzbDrone.Core/Indexers/IndexerCategory.cs and
// ParseCardigannCapabilities in src/NzbDrone.Core/Indexers/Definitions/Cardigann/Cardigann.cs), GPL-3.0.
//! What an indexer can search for and which categories it has: Prowlarr's `IndexerCapabilities`, the `t=caps`
//! answer apps read, and the category tree a search's categories are checked and widened against.

use super::categories;
use super::definition::Definition;
use super::nabxml::Elem;
use sha1::{Digest, Sha1};

/// The parameters a TV search can take, by Prowlarr's `TvSearchParam` names (lower case).
pub const TV_PARAMS: [&str; 12] =
    ["q", "season", "ep", "imdbid", "tvdbid", "rid", "tvmazeid", "traktid", "tmdbid", "doubanid", "genre", "year"];
/// The parameters a movie search can take (`MovieSearchParam`).
pub const MOVIE_PARAMS: [&str; 9] = ["q", "imdbid", "tmdbid", "imdbtitle", "imdbyear", "traktid", "genre", "doubanid", "year"];
/// The parameters a music search can take (`MusicSearchParam`).
pub const MUSIC_PARAMS: [&str; 7] = ["q", "album", "artist", "label", "year", "genre", "track"];
/// The parameters a book search can take (`BookSearchParam`).
pub const BOOK_PARAMS: [&str; 6] = ["q", "title", "author", "publisher", "genre", "year"];

/// One category in the tree: a standard one with the site's ones under it, or a site's own (100000+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    /// The category number.
    pub id: u32,
    /// Its name.
    pub name: String,
    /// The categories under it.
    pub subs: Vec<Category>,
}

impl Category {
    /// A category without sub-categories.
    pub fn new(id: u32, name: &str) -> Category {
        Category { id, name: name.to_string(), subs: Vec::new() }
    }

    /// Whether it is `id` or has `id` right under it (`IndexerCategory.Contains`).
    pub fn contains(&self, id: u32) -> bool {
        self.id == id || self.subs.iter().any(|s| s.id == id)
    }

    /// Standard category `id` with its standard sub-categories, as Prowlarr's `NewznabStandardCategory` has them.
    fn standard(id: u32) -> Option<Category> {
        let name = categories::name_of(id)?;
        let mut c = Category::new(id, name);
        if categories::PARENTS.contains(&id) {
            c.subs = categories::STANDARD
                .iter()
                .filter(|(i, _)| *i != id && !categories::PARENTS.contains(i) && i / 1000 * 1000 == id)
                .map(|(i, n)| Category::new(*i, n))
                .collect();
        }
        Some(c)
    }
}

/// A site category and the category Lantern shows for it (`CategoryMapping`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// The site's id for its category.
    pub tracker: String,
    /// The site's name for it, when it has one.
    pub desc: Option<String>,
    /// The standard (or 100000+) category number.
    pub cat: u32,
}

/// An indexer's categories: the site's mapped onto the standard ones, and the tree apps see
/// (`IndexerCapabilitiesCategories`).
#[derive(Debug, Clone, Default)]
pub struct CapCategories {
    mappings: Vec<Mapping>,
    tree: Vec<Category>,
}

impl CapCategories {
    /// The site categories mapped to standard ones (`GetTrackerCategories`).
    pub fn tracker_categories(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in self.mappings.iter().filter(|m| m.cat < 100000) {
            if !out.contains(&m.tracker) {
                out.push(m.tracker.clone());
            }
        }
        out
    }

    /// The tree as built (`sorted == false`), or sorted as apps see it: standard categories by number (as text),
    /// then the site's own by name, each one's sub-categories by number (`GetTorznabCategoryTree`).
    pub fn tree(&self, sorted: bool) -> Vec<Category> {
        if !sorted {
            return self.tree.clone();
        }
        let mut t: Vec<Category> = self
            .tree
            .iter()
            .map(|c| {
                let mut n = Category::new(c.id, &c.name);
                n.subs = c.subs.clone();
                n.subs.sort_by_key(|s| s.id);
                n
            })
            .collect();
        let key = |c: &Category| if c.id >= 100000 { format!("zzz{}", c.name) } else { c.id.to_string() };
        t.sort_by(|a, b| culture_cmp(&key(a), &key(b)));
        t
    }

    /// The tree flattened: each category, then its sub-categories (`GetTorznabCategoryList`).
    pub fn list(&self, sorted: bool) -> Vec<Category> {
        let mut out = Vec::new();
        for c in self.tree(sorted) {
            out.push(Category::new(c.id, &c.name));
            out.extend(c.subs);
        }
        out
    }

    /// Maps site category `tracker` to standard category `cat` (with its standard name); with a `desc`, also to a
    /// category of its own, 100000 + its number (or a hash of it when it isn't one) (`AddCategoryMapping`).
    pub fn add_mapping(&mut self, tracker: &str, cat: &Category, desc: Option<&str>) {
        self.mappings.push(Mapping { tracker: tracker.to_string(), desc: desc.map(str::to_string), cat: cat.id });
        self.add_to_tree(cat.clone());
        let Some(desc) = desc else { return };
        let n: i64 = match tracker.trim().parse::<i32>() {
            Ok(n) => n as i64,
            Err(_) => {
                let h = Sha1::digest(tracker.as_bytes());
                u16::from_le_bytes([h[0], h[1]]) as i64
            }
        };
        let custom = Category::new((n + 100000).clamp(0, u32::MAX as i64) as u32, desc);
        self.mappings.push(Mapping { tracker: tracker.to_string(), desc: Some(desc.to_string()), cat: custom.id });
        self.add_to_tree(custom);
    }

    /// The site categories to ask for standard categories `query` (`MapTorznabCapsToTrackers`).
    pub fn map_to_trackers(&self, query: &[i64], children_to_parent: bool) -> Vec<String> {
        let wanted = self.expand(query, children_to_parent);
        let mut out: Vec<String> = Vec::new();
        for m in self.mappings.iter().filter(|m| wanted.contains(&(m.cat as i64))) {
            if !out.contains(&m.tracker) {
                out.push(m.tracker.clone());
            }
        }
        out
    }

    /// The categories of site category `input`, ignoring case (`MapTrackerCatToNewznab`).
    pub fn map_tracker(&self, input: &str) -> Vec<u32> {
        if input.trim().is_empty() {
            return Vec::new();
        }
        self.mappings
            .iter()
            .filter(|m| !m.tracker.trim().is_empty() && m.tracker.to_lowercase() == input.to_lowercase())
            .map(|m| m.cat)
            .collect()
    }

    /// The categories of the site category called `desc`, ignoring case (`MapTrackerCatDescToNewznab`).
    pub fn map_tracker_desc(&self, desc: &str) -> Vec<u32> {
        if desc.trim().is_empty() {
            return Vec::new();
        }
        self.mappings
            .iter()
            .filter(|m| m.desc.as_deref().is_some_and(|d| !d.trim().is_empty() && d.to_lowercase() == desc.to_lowercase()))
            .map(|m| m.cat)
            .collect()
    }

    /// Which of `query` this indexer has, as a category or sub-category (`SupportedCategories`).
    pub fn supported(&self, query: &[i64]) -> Vec<u32> {
        if query.is_empty() {
            return Vec::new();
        }
        let all = self.tree.iter().map(|c| c.id).chain(self.tree.iter().flat_map(|c| c.subs.iter().map(|s| s.id)));
        all.filter(|id| query.contains(&(*id as i64))).collect()
    }

    /// Adds the standard categories of `other` (not its site ones) (`Concat`, for all indexers at once).
    pub fn concat(&mut self, other: &CapCategories) {
        for c in other.list(false).into_iter().filter(|c| c.id < 100000) {
            self.add_to_tree(c);
        }
    }

    /// `query` with each parent category's sub-categories added (and, asked to, each sub-category's parent)
    /// (`ExpandTorznabQueryCategories`).
    pub fn expand(&self, query: &[i64], children_to_parent: bool) -> Vec<i64> {
        let mut out: Vec<i64> = Vec::new();
        let push = |v: i64, out: &mut Vec<i64>| {
            if !out.contains(&v) {
                out.push(v);
            }
        };
        for &q in query {
            push(q, &mut out);
            if q >= 100000 {
                continue;
            }
            if let Some(parent) = self.tree.iter().find(|c| c.id as i64 == q) {
                for s in &parent.subs {
                    push(s.id as i64, &mut out);
                }
            } else if children_to_parent && q >= 0 {
                if let Some(parent) = self.tree.iter().find(|c| c.contains(q as u32)) {
                    push(parent.id as i64, &mut out);
                }
            }
        }
        out
    }

    /// Puts `cat` in the tree: a standard parent at the top, a standard sub-category under its parent, a category
    /// of the site's own between 1001 and 9999 under the standard parent of its thousand, any other at the top
    /// (`AddTorznabCategoryTree`).
    fn add_to_tree(&mut self, cat: Category) {
        if categories::PARENTS.contains(&cat.id) {
            if !self.tree.iter().any(|c| c.id == cat.id) {
                self.tree.push(Category::new(cat.id, &cat.name));
            }
            return;
        }
        let parent = categories::PARENTS.iter().filter_map(|p| Category::standard(*p)).find(|p| p.contains(cat.id));
        let parent = match parent {
            Some(p) => Some(p),
            None if cat.id > 1000 && cat.id < 10000 => {
                categories::PARENTS.iter().find(|p| **p / 1000 == cat.id / 1000).and_then(|p| Category::standard(*p))
            }
            None => None,
        };
        match parent {
            Some(p) => match self.tree.iter_mut().find(|c| c.id == p.id) {
                Some(node) => {
                    if !node.contains(cat.id) {
                        node.subs.push(cat);
                    }
                }
                None => {
                    let mut node = Category::new(p.id, &p.name);
                    node.subs.push(cat);
                    self.tree.push(node);
                }
            },
            None => self.tree.push(cat),
        }
    }
}

/// .NET's culture-aware string order (`Comparer<string>.Default`), close enough for category names: letters and
/// digits compared ignoring case first, lower case before upper case on a tie.
fn culture_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

/// What an indexer can search for (`IndexerCapabilities`).
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// The most results one request gives.
    pub limits_max: Option<u32>,
    /// The results one request gives unless asked for fewer.
    pub limits_default: Option<u32>,
    /// The site takes search words as typed.
    pub supports_raw_search: bool,
    /// The parameters of a plain search (always `q`).
    pub search_params: Vec<&'static str>,
    /// The parameters of a TV search (none: no TV search).
    pub tv_params: Vec<&'static str>,
    /// The parameters of a movie search.
    pub movie_params: Vec<&'static str>,
    /// The parameters of a music search.
    pub music_params: Vec<&'static str>,
    /// The parameters of a book search.
    pub book_params: Vec<&'static str>,
    /// The categories.
    pub categories: CapCategories,
    /// The release flags the indexer marks: (name, description).
    pub flags: Vec<(String, String)>,
}

impl Default for Capabilities {
    fn default() -> Capabilities {
        Capabilities {
            limits_max: Some(100),
            limits_default: Some(100),
            supports_raw_search: false,
            search_params: vec!["q"],
            tv_params: Vec::new(),
            movie_params: Vec::new(),
            music_params: Vec::new(),
            book_params: Vec::new(),
            categories: CapCategories::default(),
            flags: Vec::new(),
        }
    }
}

/// `names` read as parameters of `allowed` (case ignored), each once (`ParseTvSearchParams` and the others).
fn parse_params(into: &mut Vec<&'static str>, names: &[String], allowed: &[&'static str], mode: &str) -> Result<(), String> {
    for n in names {
        let Some(p) = allowed.iter().find(|a| a.eq_ignore_ascii_case(n)) else {
            return Err(format!("Not supported {mode} param: {n}"));
        };
        if into.contains(p) {
            return Err(format!("Duplicate {} param: {n}", mode.to_lowercase()));
        }
        into.push(p);
    }
    Ok(())
}

impl Capabilities {
    /// Whether it has a plain search.
    pub fn search_available(&self) -> bool {
        !self.search_params.is_empty()
    }

    /// Whether a TV search takes `p` (`TvSearchImdbAvailable` and the others).
    pub fn tv(&self, p: &str) -> bool {
        self.tv_params.contains(&p)
    }

    /// Whether a movie search takes `p`.
    pub fn movie(&self, p: &str) -> bool {
        self.movie_params.contains(&p)
    }

    /// Whether a music search takes `p`.
    pub fn music(&self, p: &str) -> bool {
        self.music_params.contains(&p)
    }

    /// Whether a book search takes `p`.
    pub fn book(&self, p: &str) -> bool {
        self.book_params.contains(&p)
    }

    /// Reads a definition's search modes (`ParseCardigannSearchModes`); the error says what is wrong with them.
    pub fn parse_modes(&mut self, modes: &[(String, Vec<String>)]) -> Result<(), String> {
        if modes.is_empty() {
            return Err("At least one search mode is required".into());
        }
        if !modes.iter().any(|(k, _)| k == "search") {
            return Err("The search mode 'search' is mandatory".into());
        }
        for (k, v) in modes {
            match k.as_str() {
                "search" => {
                    if v.len() != 1 || v[0] != "q" {
                        return Err("In search mode 'search' only 'q' parameter is supported and it's mandatory".into());
                    }
                    self.search_params.push("q");
                }
                "tv-search" => parse_params(&mut self.tv_params, v, &TV_PARAMS, "tv-search")?,
                "movie-search" => parse_params(&mut self.movie_params, v, &MOVIE_PARAMS, "movie-search")?,
                "music-search" => parse_params(&mut self.music_params, v, &MUSIC_PARAMS, "Music-search")?,
                "book-search" => parse_params(&mut self.book_params, v, &BOOK_PARAMS, "book-search")?,
                other => return Err(format!("Unsupported search mode: {other}")),
            }
        }
        Ok(())
    }

    /// A Cardigann definition's capabilities (`ParseCardigannCapabilities`).
    pub fn from_definition(def: &Definition) -> Result<Capabilities, String> {
        let mut c = Capabilities::default();
        let modes: Vec<(String, Vec<String>)> = def.caps.modes.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        c.parse_modes(&modes)?;
        c.supports_raw_search = def.caps.allowrawsearch;
        for (id, cat) in &def.caps.categories {
            if let Some((n, name)) = categories::by_name(cat) {
                c.categories.add_mapping(id, &Category::new(n, name), None);
            }
        }
        for m in &def.caps.categorymappings {
            let Some((n, name)) = categories::by_name(&m.cat) else { continue };
            let desc = if m.desc.is_empty() { None } else { Some(m.desc.as_str()) };
            c.categories.add_mapping(&m.id, &Category::new(n, name), desc);
        }
        Ok(c)
    }

    /// `q` and the parameters in `have`, in the order `order` names them (`SupportedTvSearchParams` and the others).
    fn supported(have: &[&str], order: &[&str]) -> String {
        let mut out = vec!["q"];
        out.extend(order.iter().filter(|p| have.contains(p)));
        out.join(",")
    }

    /// The `t=caps` document, its server called `title` (Prowlarr writes its own name there) (`GetXDocument`).
    pub fn to_xml(&self, title: &str) -> String {
        let raw = |e: Elem| if self.supports_raw_search { e.attr("searchEngine", "raw") } else { e };
        let mode = |tag: &str, available: bool, params: String| {
            raw(Elem::new(tag).attr("available", if available { "yes" } else { "no" }).attr("supportedParams", &params))
        };
        let tv = Self::supported(
            &self.tv_params,
            &["season", "ep", "imdbid", "tvdbid", "rid", "tvmazeid", "traktid", "tmdbid", "doubanid", "genre", "year"],
        );
        let movie = Self::supported(&self.movie_params, &["imdbid", "tmdbid", "traktid", "genre", "doubanid", "year"]);
        let music = Self::supported(&self.music_params, &["album", "artist", "label", "track", "year", "genre"]);
        let book = Self::supported(&self.book_params, &["title", "author", "publisher", "genre", "year"]);
        let mut caps = Elem::new("caps").child(Elem::new("server").attr("title", title));
        if self.limits_default.is_some() || self.limits_max.is_some() {
            let mut l = Elem::new("limits");
            if let Some(d) = self.limits_default {
                l = l.attr("default", &d.to_string());
            }
            if let Some(m) = self.limits_max {
                l = l.attr("max", &m.to_string());
            }
            caps = caps.child(l);
        }
        caps = caps.child(
            Elem::new("searching")
                .child(mode("search", self.search_available(), "q".into()))
                .child(mode("tv-search", !self.tv_params.is_empty(), tv))
                .child(mode("movie-search", !self.movie_params.is_empty(), movie))
                .child(mode("music-search", !self.music_params.is_empty(), music.clone()))
                .child(mode("audio-search", !self.music_params.is_empty(), music))
                .child(mode("book-search", !self.book_params.is_empty(), book)),
        );
        let mut cats = Elem::new("categories");
        for c in self.categories.tree(true) {
            let mut e = Elem::new("category").attr("id", &c.id.to_string()).attr("name", &c.name);
            for s in &c.subs {
                e = e.child(Elem::new("subcat").attr("id", &s.id.to_string()).attr("name", &s.name));
            }
            cats = cats.child(e);
        }
        caps = caps.child(cats);
        let mut tags = Elem::new("tags");
        for (n, d) in &self.flags {
            tags = tags.child(Elem::new("tag").attr("name", n).attr("description", d));
        }
        caps.child(tags).document()
    }

    /// Adds `other`'s parameters and standard categories (`Concat`, for all indexers at once).
    pub fn concat(&mut self, other: &Capabilities) {
        for (mine, theirs) in [
            (&mut self.search_params, &other.search_params),
            (&mut self.tv_params, &other.tv_params),
            (&mut self.movie_params, &other.movie_params),
            (&mut self.music_params, &other.music_params),
            (&mut self.book_params, &other.book_params),
        ] {
            let mut u: Vec<&'static str> = Vec::new();
            for p in mine.iter().chain(theirs.iter()) {
                if !u.contains(p) {
                    u.push(p);
                }
            }
            *mine = u;
        }
        self.categories.concat(&other.categories);
    }

    /// The capabilities Prowlarr answers for indexer 0 (its test indexer): TV with season and episode, plain
    /// movie, music and book searches, every standard category.
    pub fn test_indexer() -> Capabilities {
        let mut c = Capabilities {
            tv_params: vec!["q", "season", "ep"],
            movie_params: vec!["q"],
            music_params: vec!["q"],
            book_params: vec!["q"],
            ..Capabilities::default()
        };
        for (id, name) in categories::STANDARD {
            c.categories.add_mapping("1", &Category::new(*id, name), None);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/IndexerTests/IndexerCapabilitiesCategoriesFixture.cs).
    use super::*;

    fn movies() -> CapCategories {
        let mut c = CapCategories::default();
        c.add_mapping("1", &Category::new(2030, "Movies/SD"), Some("Filme SD"));
        c
    }

    #[test]
    fn should_support_parent_if_child_mapping() {
        assert_eq!(movies().supported(&[2000]).len(), 1);
    }

    #[test]
    fn should_support_category_if_mapped() {
        assert_eq!(movies().supported(&[2030]).len(), 1);
    }

    #[test]
    fn should_not_support_category_if_not_mapped() {
        assert_eq!(movies().supported(&[2040]).len(), 0);
    }

    #[test]
    fn should_get_tracker_category_list() {
        let mut c = movies();
        c.add_mapping("2", &Category::new(2040, "Movies/HD"), Some("Filme HD"));
        assert_eq!(c.tracker_categories(), vec!["1", "2"]);
    }

    #[test]
    fn should_get_category_by_tracker_id() {
        let mut c = movies();
        c.add_mapping("2", &Category::new(2040, "Movies/HD"), Some("Filme HD"));
        let got = c.map_tracker("2");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], 2040);
    }

    #[test]
    fn should_get_category_by_tracker_desc() {
        let mut c = movies();
        c.add_mapping("2", &Category::new(2040, "Movies/HD"), Some("Filme HD"));
        let got = c.map_tracker_desc("Filme HD");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], 2040);
    }

    #[test]
    fn expands_parents_and_sorts_the_tree_like_prowlarr() {
        let mut c = movies();
        c.add_mapping("b", &Category::new(5040, "TV/HD"), Some("Serien"));
        c.add_mapping("2", &Category::new(2040, "Movies/HD"), Some("Alpha"));
        assert_eq!(c.expand(&[2000], false), vec![2000, 2030, 2040]);
        assert_eq!(c.expand(&[2040], true), vec![2040, 2000]);
        assert_eq!(c.map_to_trackers(&[2000], false), vec!["1", "2"]);
        let ids: Vec<u32> = c.tree(true).iter().map(|x| x.id).collect();
        // the hashed id of "b": SHA-1's first two bytes, little endian
        let h = Sha1::digest(b"b");
        let b = 100000 + u16::from_le_bytes([h[0], h[1]]) as u32;
        assert_eq!(ids, vec![2000, 5000, 100002, 100001, b]);
    }
}
