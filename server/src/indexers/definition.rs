//! The Cardigann indexer definitions (the YAML files of Prowlarr/Indexers, schema v11): what a site is, how to
//! sign in, how to search it and how to read its answers. Read leniently: an unknown key is ignored, so newer
//! definitions still load.

use super::filters::{yaml_text, Filter};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_yaml::Value as Yaml;
use std::collections::BTreeMap;

/// The schema version of the definitions Lantern reads (`definitions/v11/` in Prowlarr/Indexers).
pub const SCHEMA: &str = "v11";

/// A whole definition.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Definition {
    /// The definition's id, its file name without `.yml` (`1337x`).
    pub id: String,
    /// Ids of older definitions this one replaces.
    pub replaces: Vec<String>,
    /// The site's name.
    pub name: String,
    /// One line on what the site is.
    pub description: String,
    /// The site's language (`en-US`).
    pub language: String,
    /// `public`, `semi-private` or `private`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The pages' encoding (`UTF-8`, `windows-1251`).
    pub encoding: String,
    /// Seconds to wait between two requests to the site.
    #[serde(rename = "requestDelay")]
    pub request_delay: Option<f64>,
    /// The site's addresses, the first being the default.
    pub links: Vec<String>,
    /// Addresses that used to work; a saved one is moved to the first link.
    pub legacylinks: Vec<String>,
    /// Follow redirects of the search pages.
    pub followredirect: bool,
    /// What the site can search for.
    pub caps: Caps,
    /// The settings a person fills in (username, password, cookie, sort …).
    pub settings: Vec<Setting>,
    /// How to sign in, when the site needs it.
    pub login: Option<Login>,
    /// How to search.
    pub search: Search,
    /// How to get a release's file from its download link.
    pub download: Option<Download>,
}

/// What the site can search for.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Caps {
    /// site category → standard category (the short form).
    pub categories: IndexMap<String, String>,
    /// site category → standard category, with a description.
    pub categorymappings: Vec<CategoryMapping>,
    /// search mode (`search`, `tv-search`, `movie-search`, `music-search`, `book-search`) → the parameters it takes.
    pub modes: IndexMap<String, Vec<String>>,
    /// The site takes search words as typed, without cleaning.
    pub allowrawsearch: bool,
    /// The site finds TV shows by IMDb id.
    pub allowtvsearchimdb: bool,
}

/// One site category and the standard category it is.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CategoryMapping {
    /// The site's id for it.
    #[serde(deserialize_with = "text")]
    pub id: String,
    /// The standard category's name (`Movies/HD`).
    pub cat: String,
    /// The site's name for it.
    pub desc: String,
    /// Searched when no category is asked for.
    pub default: bool,
}

/// One setting of a definition.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Setting {
    /// Its name (`.Config.<name>` in templates).
    pub name: String,
    /// `text`, `password`, `checkbox`, `select`, `multi-select`, `info`, `info_cookie`, `info_flaresolverr` …
    #[serde(rename = "type")]
    pub kind: String,
    /// Its label.
    pub label: String,
    /// Its default (text, a number or a boolean).
    #[serde(deserialize_with = "opt_text")]
    pub default: Option<String>,
    /// A multi-select's defaults.
    pub defaults: Vec<String>,
    /// value → label, for a select.
    pub options: IndexMap<String, String>,
}

impl Setting {
    /// Whether the setting is only words for the person, with no value.
    pub fn is_info(&self) -> bool {
        self.kind.starts_with("info")
    }

    /// Whether its value is a secret (passwords, cookies, keys): kept in the vault, never sent back.
    pub fn is_secret(&self) -> bool {
        let n = self.name.to_lowercase();
        self.kind == "password"
            || ["password", "cookie", "apikey", "api_key", "passkey", "rsskey", "authkey", "token", "pin", "2fa", "otp"]
                .iter()
                .any(|k| n.contains(k))
    }
}

/// How to sign in.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Login {
    /// The page with the form, or the address to post to.
    pub path: String,
    /// Where the form is sent, when not the form's own `action`.
    pub submitpath: String,
    /// `form` (fill the page's form), `post`, `get` or `cookie` (a cookie the person pasted).
    pub method: String,
    /// The form's selector (`form[action="takelogin.php"]`).
    pub form: String,
    /// Read the form's hidden inputs too.
    pub selectors: bool,
    /// name → template of what to send.
    pub inputs: IndexMap<String, Yaml>,
    /// name → selector on the login page whose text is sent as that input.
    pub selectorinputs: IndexMap<String, SelectorBlock>,
    /// name → selector on the login page, sent with GET.
    pub getselectorinputs: IndexMap<String, SelectorBlock>,
    /// Cookies to send with every request (`["JAVA=OK"]`).
    pub cookies: Vec<String>,
    /// Headers to send with the login.
    pub headers: IndexMap<String, Yaml>,
    /// Signs the login failed and why.
    pub error: Vec<ErrorBlock>,
    /// How to check the login worked.
    pub test: Option<PageTest>,
    /// A captcha on the login form (Lantern asks for the cookie instead).
    pub captcha: Option<Yaml>,
}

/// A page and a selector that must be on it.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PageTest {
    /// The page.
    pub path: String,
    /// What must be on it.
    pub selector: String,
}

/// A sign that something went wrong, and the message.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ErrorBlock {
    /// What shows the error.
    pub selector: String,
    /// The message (a selector or a text).
    pub message: Option<SelectorBlock>,
}

/// How to search.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Search {
    /// One page to search (older definitions).
    pub path: String,
    /// The pages to search, each read in turn.
    pub paths: Vec<SearchPath>,
    /// Headers to send (name → one or more templates).
    pub headers: IndexMap<String, Yaml>,
    /// name → template of the query parameters (`$raw` is added as it is).
    pub inputs: IndexMap<String, Yaml>,
    /// Filters on the search words.
    pub keywordsfilters: Vec<Filter>,
    /// Filters on the page before it is read.
    pub preprocessingfilters: Vec<Filter>,
    /// Signs the search failed.
    pub error: Vec<ErrorBlock>,
    /// The rows of the results.
    pub rows: Rows,
    /// field name → how to read it from a row, in order (a field may use the ones before it).
    pub fields: IndexMap<String, SelectorBlock>,
}

/// One page to search.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearchPath {
    /// The path (a template).
    pub path: String,
    /// `get` or `post`.
    pub method: String,
    /// This page's own inputs.
    pub inputs: IndexMap<String, Yaml>,
    /// Also send the search's inputs.
    pub inheritinputs: bool,
    /// Only for these site categories.
    pub categories: Vec<String>,
    /// How to read the answer.
    pub response: Option<Response>,
    /// Follow redirects.
    pub followredirect: bool,
}

impl Default for SearchPath {
    fn default() -> SearchPath {
        SearchPath {
            path: String::new(),
            method: String::new(),
            inputs: IndexMap::new(),
            inheritinputs: true,
            categories: Vec::new(),
            response: None,
            followredirect: false,
        }
    }
}

/// How to read a search answer.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Response {
    /// `html` (default), `json` or `xml`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The text the site answers when nothing was found.
    #[serde(rename = "noResultsMessage")]
    pub no_results_message: String,
}

/// The rows of a results page.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Rows {
    /// What one row is.
    pub selector: String,
    /// Add this many following rows to each row (a result over two table rows).
    pub after: usize,
    /// Rows that are a date header for the rows under them.
    pub dateheaders: Option<SelectorBlock>,
    /// JSON: where the count of results is.
    pub count: Option<SelectorBlock>,
    /// JSON: the rows are inside each of these.
    pub attribute: String,
    /// JSON: each row holds several results.
    pub multiple: bool,
    /// JSON: a missing `attribute` means no results.
    #[serde(rename = "missingAttributeEqualsNoResults")]
    pub missing_attribute_equals_no_results: bool,
    /// Filters on the row list (`andmatch`).
    pub filters: Vec<Filter>,
}

/// How to read one value: a selector (with an attribute, removals and filters) or a text template.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SelectorBlock {
    /// The CSS selector (HTML) or path (JSON).
    pub selector: String,
    /// Read this attribute instead of the text.
    pub attribute: String,
    /// Remove these elements first.
    pub remove: String,
    /// A fixed text (a template) instead of a selector.
    #[serde(deserialize_with = "opt_text")]
    pub text: Option<String>,
    /// The field may be missing.
    pub optional: bool,
    /// The value when it is missing (a template).
    #[serde(deserialize_with = "opt_text")]
    pub default: Option<String>,
    /// The filters on the value.
    pub filters: Vec<Filter>,
    /// selector → value: the first that matches gives the value.
    pub case: IndexMap<String, Yaml>,
    /// Download: read this from the page that `before` fetched.
    pub usebeforeresponse: bool,
}

/// How to get a release's file.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Download {
    /// Selectors on the details page that give the real download link, tried in order.
    pub selectors: Vec<SelectorBlock>,
    /// `get` or `post`.
    pub method: String,
    /// A request to make first.
    pub before: Option<BeforeRequest>,
    /// The info hash on the page, for a magnet link.
    pub infohash: Option<InfoHash>,
    /// Headers for the download.
    pub headers: IndexMap<String, Yaml>,
}

/// A request made before the download.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BeforeRequest {
    /// Its path.
    pub path: String,
    /// `get` or `post`.
    pub method: String,
    /// Its inputs.
    pub inputs: IndexMap<String, Yaml>,
    /// A selector giving the path instead.
    pub pathselector: Option<SelectorBlock>,
}

/// Where a magnet link's hash and title are.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct InfoHash {
    /// The hash.
    pub hash: Option<SelectorBlock>,
    /// The title.
    pub title: Option<SelectorBlock>,
    /// Read them from the `before` page.
    pub usebeforeresponse: bool,
}

fn text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(yaml_text(&Yaml::deserialize(d)?))
}

fn opt_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Option::<Yaml>::deserialize(d)?;
    Ok(v.map(|v| yaml_text(&v)))
}

impl Definition {
    /// Reads a definition from its YAML.
    pub fn parse(yaml: &str) -> Result<Definition, String> {
        let mut d: Definition =
            serde_yaml::from_str(yaml.trim_start_matches('\u{feff}')).map_err(|e| format!("The definition can't be read: {e}"))?;
        if d.id.is_empty() {
            return Err("The definition has no id".into());
        }
        if d.search.paths.is_empty() && !d.search.path.is_empty() {
            d.search.paths.push(SearchPath { path: d.search.path.clone(), ..SearchPath::default() });
        }
        Ok(d)
    }

    /// Whether the site needs an account.
    pub fn private(&self) -> bool {
        self.kind != "public"
    }

    /// The site's categories and the standard categories they map to, in the definition's order.
    pub fn category_map(&self) -> Vec<CategoryMapping> {
        let mut out: Vec<CategoryMapping> = self
            .caps
            .categories
            .iter()
            .map(|(id, cat)| CategoryMapping { id: id.clone(), cat: cat.clone(), desc: cat.clone(), default: false })
            .collect();
        out.extend(self.caps.categorymappings.iter().cloned());
        out
    }

    /// The defaults of the settings, by name (checkboxes as `true`/`false`).
    pub fn default_settings(&self) -> BTreeMap<String, String> {
        self.settings
            .iter()
            .filter(|s| !s.is_info())
            .map(|s| {
                (
                    s.name.clone(),
                    s.default.clone().unwrap_or_else(|| if s.kind == "multi-select" { s.defaults.join(",") } else { String::new() }),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_definition() {
        let d = Definition::parse(include_str!("../../tests/fixtures/definitions/lantern-test.yml")).unwrap();
        assert_eq!(d.id, "lantern-test");
        assert!(d.private());
        assert!(d.category_map().iter().any(|c| c.cat == "TV/HD"));
        assert_eq!(d.search.fields.keys().next().map(String::as_str), Some("title"));
        assert!(d.settings.iter().any(|s| s.name == "sort"));
    }
}
