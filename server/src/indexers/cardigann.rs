//! Runs a Cardigann definition against its site: signs in, searches, reads the answer (HTML, JSON or XML) into
//! [`Release`]s and fetches a release's file. Written from the definitions' documented behaviour.

use super::categories;
use super::definition::{Definition, ErrorBlock, SelectorBlock};
use super::filters::{self, yaml_text};
use super::http::{Fetch, Http, Page};
use super::json_path;
use super::release::{Protocol, Query, Release, SearchKind};
use super::template::{render, Val, Vars};
use chrono::{DateTime, Datelike, Utc};
use dom_query::{Document, Selection};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Safe CSS selection: a selector that can't be read selects nothing (dom_query's own `select` panics on one).
pub trait Css<'a> {
    /// The elements under this that match `sel`.
    fn css(&self, sel: &str) -> Selection<'a>;
}

/// `sel` as dom_query reads it: the argument of `:contains(…)` decoded from CSS escapes (`\00a0`) and quoted.
pub fn normalize_css(sel: &str) -> std::borrow::Cow<'_, str> {
    if !sel.contains(":contains(") {
        return std::borrow::Cow::Borrowed(sel);
    }
    let mut out = String::new();
    let mut rest = sel;
    while let Some(at) = rest.find(":contains(") {
        out.push_str(&rest[..at + 10]);
        let after = &rest[at + 10..];
        let Some(end) = after.find(')') else {
            out.push_str(after);
            return std::borrow::Cow::Owned(out);
        };
        let arg = after[..end].trim();
        if arg.starts_with('"') || arg.starts_with('\'') {
            out.push_str(&after[..end]);
        } else {
            let mut decoded = String::new();
            let mut it = arg.chars().peekable();
            while let Some(c) = it.next() {
                if c == '\\' {
                    let hex: String = std::iter::from_fn(|| it.next_if(|h| h.is_ascii_hexdigit())).take(6).collect();
                    if hex.is_empty() {
                        if let Some(n) = it.next() {
                            decoded.push(n);
                        }
                    } else {
                        decoded.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                        if it.peek() == Some(&' ') {
                            it.next();
                        }
                    }
                } else {
                    decoded.push(c);
                }
            }
            out.push('"');
            out.push_str(&decoded.replace('"', "\\\""));
            out.push('"');
        }
        out.push(')');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// The elements of `doc` that match `sel` (nothing for a selector that can't be read).
pub fn css_doc<'a>(doc: &'a Document, sel: &str) -> Selection<'a> {
    match dom_query::Matcher::new(&normalize_css(sel)) {
        Ok(m) => doc.select_matcher(&m),
        Err(_) => Selection::default(),
    }
}

impl<'a> Css<'a> for Selection<'a> {
    fn css(&self, sel: &str) -> Selection<'a> {
        match dom_query::Matcher::new(&normalize_css(sel)) {
            Ok(m) => self.select_matcher(&m),
            Err(_) => Selection::default(),
        }
    }
}

/// A selector for an XML answer read as HTML: `link` is `x-link` there.
fn xml_selector(sel: &str) -> String {
    let re = regex::Regex::new(r"(^|[\s>+~,(])link\b").expect("a fixed pattern");
    re.replace_all(sel, "${1}x-link").into_owned()
}

/// Whether `sel` is a CSS selector Lantern can read.
pub fn valid_css(sel: &str) -> bool {
    dom_query::Matcher::new(&normalize_css(sel)).is_ok()
}

/// What a grab gives: a magnet link or the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Grabbed {
    /// A magnet link.
    Magnet(String),
    /// The `.torrent` or `.nzb` file.
    File(Vec<u8>),
}

/// One site, ready to search: its definition, address and settings.
pub struct Cardigann {
    /// The definition.
    pub def: Arc<Definition>,
    /// The site's address, ending in `/`.
    pub base: String,
    /// The settings (`.Config.*`), checkboxes as `true` / `false`.
    pub config: BTreeMap<String, String>,
    /// The HTTP client with this site's cookies.
    pub http: Http,
}

impl Cardigann {
    /// A site on `base` (the first link when empty) with `config`.
    pub fn new(def: Arc<Definition>, base: &str, config: BTreeMap<String, String>, http: Http) -> Cardigann {
        let mut base = if base.trim().is_empty() { def.links.first().cloned().unwrap_or_default() } else { base.trim().to_string() };
        // a saved address the definition has retired moves to its first link
        if def.legacylinks.iter().any(|l| l.trim_end_matches('/') == base.trim_end_matches('/')) {
            base = def.links.first().cloned().unwrap_or(base);
        }
        if !base.ends_with('/') {
            base.push('/');
        }
        Cardigann { def, base, config, http }
    }

    /// The template values every request has: `.Config.*`, `.True`, `.False`, `.Today.Year`.
    fn base_vars(&self) -> Vars {
        let mut v = Vars::default();
        v.set("True", "True");
        v.null("False");
        v.set("Today.Year", Utc::now().year().to_string());
        v.set("Config.sitelink", self.base.clone());
        for s in &self.def.settings {
            if s.is_info() {
                continue;
            }
            let raw = self.config.get(&s.name).cloned().or_else(|| s.default.clone()).unwrap_or_default();
            match s.kind.as_str() {
                "checkbox" => {
                    if matches!(raw.to_lowercase().as_str(), "true" | "1" | "on" | "yes") {
                        v.set(format!("Config.{}", s.name), "True");
                    } else {
                        v.null(format!("Config.{}", s.name));
                    }
                }
                "multi-select" => {
                    let list: Vec<String> = raw.split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect();
                    v.map.insert(format!("Config.{}", s.name), Val::List(list));
                }
                _ => v.set(format!("Config.{}", s.name), raw),
            }
        }
        v
    }

    /// An address on the site: `path` as it is when absolute, else after the site's address.
    pub fn resolve(&self, path: &str) -> String {
        resolve(&self.base, path)
    }

    /// Renders a map of inputs (`name → template`); `$raw` goes back as its own entry.
    fn inputs(&self, inputs: &indexmap::IndexMap<String, serde_yaml::Value>, vars: &Vars) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for (k, v) in inputs {
            let val = render(&yaml_text(v), vars)?;
            out.push((k.clone(), val));
        }
        Ok(out)
    }

    /// Signs in, when the site needs it and the settings allow it. Cookie sign-in only sets the cookie.
    pub async fn login(&self) -> Result<(), String> {
        let Some(login) = &self.def.login else { return Ok(()) };
        let vars = self.base_vars();
        for c in &login.cookies {
            self.http.add_cookies(&self.base, c);
        }
        let headers = self.headers(&login.headers, &vars)?;
        match login.method.as_str() {
            "cookie" => {
                let cookie = self.config.get("cookie").cloned().unwrap_or_default();
                if cookie.trim().is_empty() {
                    return Err(format!(
                        "{} signs in with a cookie: paste the cookie from your browser in the indexer's settings",
                        self.def.name
                    ));
                }
                self.http.add_cookies(&self.base, &cookie);
            }
            "get" | "post" => {
                let url = self.resolve(&render(&login.path, &vars)?);
                let inputs = self.inputs(&login.inputs, &vars)?;
                let page = if login.method == "get" {
                    self.http.fetch(Fetch::get(&with_query(&url, &inputs)).headers(headers.clone())).await?
                } else {
                    self.http.fetch(Fetch::post_form(&url, inputs).headers(headers.clone())).await?
                };
                self.check_errors(&page, &login.error, &vars)?;
            }
            "form" | "" => {
                if login.captcha.is_some() {
                    return Err(format!(
                        "{} asks for a captcha when signing in: sign in with your browser, then paste its cookie in the indexer's settings",
                        self.def.name
                    ));
                }
                let url = self.resolve(&render(&login.path, &vars)?);
                let page = self.http.fetch(Fetch::get(&url).headers(headers.clone())).await?;
                let (action, mut fields) =
                    read_form(&page.text, if login.form.is_empty() { "form" } else { &login.form }, &login.selectorinputs)?;
                for (k, v) in self.inputs(&login.inputs, &vars)? {
                    fields.retain(|(n, _)| *n != k);
                    fields.push((k, v));
                }
                let submit = if !login.submitpath.is_empty() {
                    self.resolve(&render(&login.submitpath, &vars)?)
                } else if action.is_empty() {
                    url.clone()
                } else {
                    resolve(&page.url, &action)
                };
                let page = self.http.fetch(Fetch::post_form(&submit, fields).headers(headers.clone()).referer(&url)).await?;
                self.check_errors(&page, &login.error, &vars)?;
            }
            other => return Err(format!("The sign-in method {other} is not known")),
        }
        if let Some(test) = &login.test {
            self.test_login(test, &vars).await?;
        }
        Ok(())
    }

    /// Checks a signed-in page has what `test` says it must.
    async fn test_login(&self, test: &super::definition::PageTest, vars: &Vars) -> Result<(), String> {
        let url = self.resolve(&render(&test.path, vars)?);
        let page = self.http.fetch(Fetch::get(&url)).await?;
        if !test.selector.is_empty() {
            let sel = render(&test.selector, vars)?;
            let found = {
                let doc = Document::from(page.text.as_str());
                css_doc(&doc, &sel).exists()
            };
            if !found {
                return Err(format!(
                    "Signing in to {} didn't work: check the username and password (or cookie) in its settings",
                    self.def.name
                ));
            }
        }
        Ok(())
    }

    /// An error the page shows, as a sentence.
    fn check_errors(&self, page: &Page, errors: &[ErrorBlock], vars: &Vars) -> Result<(), String> {
        if errors.is_empty() {
            return Ok(());
        }
        let doc = Document::from(page.text.as_str());
        for e in errors {
            let sel = render(&e.selector, vars)?;
            let found = css_doc(&doc, &sel);
            if found.exists() {
                let msg = match &e.message {
                    Some(m) if m.text.is_some() => render(m.text.as_deref().unwrap_or(""), vars)?,
                    Some(m) if !m.selector.is_empty() => css_doc(&doc, &m.selector).text().trim().to_string(),
                    _ => found.text().trim().to_string(),
                };
                let msg = if msg.is_empty() { "the site said no".to_string() } else { collapse(&msg) };
                return Err(format!("{}: {msg}", self.def.name));
            }
        }
        Ok(())
    }

    /// The headers of a definition block, rendered.
    fn headers(&self, headers: &indexmap::IndexMap<String, serde_yaml::Value>, vars: &Vars) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for (k, v) in headers {
            let values = match v {
                serde_yaml::Value::Sequence(s) => s.iter().map(yaml_text).collect(),
                other => vec![yaml_text(other)],
            };
            for t in values {
                out.push((k.clone(), render(&t, vars)?));
            }
        }
        Ok(out)
    }

    /// The template values of a search.
    fn search_vars(&self, q: &Query) -> Result<Vars, String> {
        let mut v = self.base_vars();
        let now = Utc::now();
        let mut terms = q.search_terms();
        if !self.def.caps.allowrawsearch {
            terms = terms.replace(['\'', '"'], " ");
        }
        let keywords = filters::apply_keywords(terms.trim(), &self.def.search.keywordsfilters, now)?;
        let opt = |s: &str| if s.trim().is_empty() { Val::Null } else { Val::Str(s.trim().to_string()) };
        v.set("Keywords", keywords.clone());
        v.map.insert("Query.Keywords".into(), opt(&keywords));
        v.map.insert("Query.Q".into(), opt(&q.keywords));
        v.set("Query.Type", q.kind.torznab());
        v.map.insert("Query.Series".into(), opt(&q.keywords));
        v.map.insert("Query.Movie".into(), opt(&q.keywords));
        v.map.insert("Query.Season".into(), opt(&q.season));
        v.map.insert("Query.Ep".into(), opt(&q.ep));
        v.map.insert("Query.Year".into(), opt(&q.year));
        v.map.insert("Query.IMDBID".into(), opt(&q.imdb_full()));
        v.map.insert("Query.IMDBIDShort".into(), opt(q.imdb_full().trim_start_matches("tt")));
        v.map.insert("Query.TMDBID".into(), opt(&q.tmdbid));
        v.map.insert("Query.TVDBID".into(), opt(&q.tvdbid));
        v.map.insert("Query.TVMazeID".into(), opt(&q.tvmazeid));
        v.map.insert("Query.TraktID".into(), opt(&q.traktid));
        v.map.insert("Query.DoubanID".into(), opt(&q.doubanid));
        v.map.insert("Query.Artist".into(), opt(&q.artist));
        v.map.insert("Query.Album".into(), opt(&q.album));
        v.map.insert("Query.Label".into(), opt(&q.label));
        v.map.insert("Query.Track".into(), opt(&q.track));
        v.map.insert("Query.Genre".into(), opt(&q.genre));
        v.map.insert("Query.Author".into(), opt(&q.author));
        v.map.insert("Query.Title".into(), opt(&q.title));
        v.set("Query.Limit", q.limit.to_string());
        v.set("Query.Offset", q.offset.to_string());
        v.map.insert("Categories".into(), Val::List(self.site_categories(&q.categories)));
        Ok(v)
    }

    /// The site's categories for standard categories `wanted` (empty: none, so the site searches all).
    pub fn site_categories(&self, wanted: &[u32]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in self.def.category_map() {
            let Some(id) = categories::id_of(&m.cat) else { continue };
            if wanted.iter().any(|w| categories::matches(id, *w)) && !out.contains(&m.id) {
                out.push(m.id.clone());
            }
        }
        out
    }

    /// The standard categories of a site category (and the site's own as 100000+ when it is a number).
    pub fn standard_categories(&self, site: &str) -> Vec<u32> {
        let mut out: Vec<u32> = self.def.category_map().iter().filter(|m| m.id == site).filter_map(|m| categories::id_of(&m.cat)).collect();
        out.dedup();
        out
    }

    /// Whether the site can do this kind of search with these parameters.
    pub fn supports(&self, q: &Query) -> bool {
        let modes = &self.def.caps.modes;
        match modes.get(q.kind.mode()) {
            Some(_) => true,
            None => q.kind == SearchKind::Search || modes.contains_key("search"),
        }
    }

    /// Searches the site.
    pub async fn search(&self, q: &Query) -> Result<Vec<Release>, String> {
        let first = self.search_once(q).await;
        match first {
            Err(e) if e == NEEDS_LOGIN => {
                self.login().await?;
                match self.search_once(q).await {
                    Err(e) if e == NEEDS_LOGIN => Err(format!(
                        "{} keeps asking to sign in: check the username and password (or cookie) in its settings",
                        self.def.name
                    )),
                    other => other,
                }
            }
            other => other,
        }
    }

    async fn search_once(&self, q: &Query) -> Result<Vec<Release>, String> {
        let vars = self.search_vars(q)?;
        let search = &self.def.search;
        let headers = self.headers(&search.headers, &vars)?;
        let wanted_site_cats = match vars.get("Categories") {
            Val::List(l) => l,
            _ => Vec::new(),
        };
        let mut releases = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for p in &search.paths {
            if !p.categories.is_empty() && !wanted_site_cats.is_empty() && !p.categories.iter().any(|c| wanted_site_cats.contains(c)) {
                continue;
            }
            let path = render(&p.path, &vars)?;
            let url = self.resolve(&path);
            let mut inputs = Vec::new();
            if p.inheritinputs {
                inputs.extend(self.inputs(&search.inputs, &vars)?);
            }
            inputs.extend(self.inputs(&p.inputs, &vars)?);
            let method = if p.method.is_empty() { "get" } else { p.method.as_str() };
            let fetch = if method == "post" {
                let (raw, plain): (Vec<_>, Vec<_>) = inputs.into_iter().partition(|(k, _)| k == "$raw");
                let mut f = Fetch::post_form(&url, plain);
                f.raw_body = raw.into_iter().map(|(_, v)| v).collect::<String>();
                f
            } else {
                Fetch::get(&with_query(&url, &inputs))
            };
            let page = self.http.fetch(fetch.headers(headers.clone()).follow(p.followredirect || self.def.followredirect)).await?;
            if self.looks_logged_out(&page) {
                return Err(NEEDS_LOGIN.into());
            }
            let kind = p.response.as_ref().map(|r| r.kind.as_str()).unwrap_or("html");
            let no_results = p.response.as_ref().map(|r| r.no_results_message.as_str()).unwrap_or("");
            if !no_results.is_empty() && page.text.contains(no_results) {
                continue;
            }
            let text = filters::apply(&page.text, &search.preprocessingfilters, Utc::now())?;
            let found = match kind {
                "json" => self.read_json(&text, &vars, q)?,
                "xml" => {
                    // an HTML parser treats <link> as empty; XML answers keep text in it
                    let text = text.replace("<link>", "<x-link>").replace("</link>", "</x-link>");
                    self.read_html(&text, &page.url, &vars, q, true)?
                }
                _ => {
                    self.check_errors(&page, &search.error, &vars)?;
                    self.read_html(&text, &page.url, &vars, q, false)?
                }
            };
            for r in found {
                if seen.insert(r.guid.clone()) {
                    releases.push(r);
                }
            }
        }
        Ok(releases)
    }

    /// Whether a search answer is the sign-in page (the site sent us there).
    fn looks_logged_out(&self, page: &Page) -> bool {
        let Some(login) = &self.def.login else { return false };
        if login.method == "cookie" || login.path.is_empty() || login.path.contains("{{") {
            return false;
        }
        let login_path = login.path.split('?').next().unwrap_or("").trim_start_matches('/');
        !login_path.is_empty() && page.redirected && page.url.split('?').next().unwrap_or("").ends_with(login_path)
    }

    /// The releases of an HTML (or XML) answer.
    fn read_html(&self, text: &str, page_url: &str, vars: &Vars, q: &Query, xml: bool) -> Result<Vec<Release>, String> {
        let rows_def = &self.def.search.rows;
        let row_sel = render(&rows_def.selector, vars)?;
        let doc = Document::from(text);
        let rows: Vec<Selection> = css_doc(&doc, &row_sel).iter().collect();
        let mut out = Vec::new();
        let now = Utc::now();
        let mut date_header: Option<String> = None;
        let mut i = 0;
        while i < rows.len() {
            let row = &rows[i];
            i += 1;
            if let Some(dh) = &rows_def.dateheaders {
                if let Ok(Some(d)) = self.read_html_value(row, dh, vars, now, xml) {
                    // a header row carries a date; the rows under it take it
                    if row.css(&dh.selector).exists() || dh.selector.is_empty() {
                        date_header = Some(d);
                    }
                }
            }
            let mut combined = row.html().to_string();
            for _ in 0..rows_def.after {
                if i < rows.len() {
                    combined.push_str(&rows[i].html());
                    i += 1;
                }
            }
            let row_doc =
                if rows_def.after > 0 { Some(Document::from(format!("<table><tbody>{combined}</tbody></table>").as_str())) } else { None };
            let target = match &row_doc {
                Some(d) => css_doc(d, "tbody"),
                None => row.clone(),
            };
            let mut fields: Vec<(String, Option<String>)> = Vec::new();
            let mut bad = false;
            for (name, block) in &self.def.search.fields {
                let mut v = vars.clone();
                for (k, val) in &fields {
                    v.map.insert(format!("Result.{k}"), val.clone().map(Val::Str).unwrap_or(Val::Null));
                }
                match self.read_html_value(&target, block, &v, now, xml) {
                    Ok(val) => fields.push((name.clone(), val)),
                    Err(_) if block.optional => {
                        fields.push((name.clone(), block.default.as_ref().map(|d| render(d, &v).unwrap_or_default())))
                    }
                    Err(_) => {
                        bad = true;
                        break;
                    }
                }
            }
            if bad {
                continue;
            }
            if let Some(r) = self.release(fields, page_url, date_header.as_deref(), q) {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// One field of an HTML row.
    fn read_html_value(
        &self,
        row: &Selection,
        b: &SelectorBlock,
        vars: &Vars,
        now: DateTime<Utc>,
        xml: bool,
    ) -> Result<Option<String>, String> {
        let raw = if let Some(t) = &b.text {
            render(t, vars)?
        } else if !b.case.is_empty() {
            let mut hit = None;
            for (sel, val) in &b.case {
                let sel = render(sel, vars)?;
                if sel == "*" || row.css(&sel).exists() || row.is(&sel) {
                    hit = Some(render(&yaml_text(val), vars)?);
                    break;
                }
            }
            match hit {
                Some(h) => h,
                None => return Err("no case matched".into()),
            }
        } else {
            let sel = render(&b.selector, vars)?;
            let sel = if xml { xml_selector(&sel) } else { sel };
            let el = if sel.is_empty() { row.clone() } else { row.css(&sel).first() };
            if !el.exists() {
                return match &b.default {
                    Some(d) => Ok(Some(render(d, vars)?)),
                    None => Err(format!("{sel} is not in the row")),
                };
            }
            if !b.remove.is_empty() {
                el.css(&render(&b.remove, vars)?).remove();
            }
            if b.attribute.is_empty() {
                el.text().trim().to_string()
            } else {
                match el.attr(&b.attribute) {
                    Some(a) => a.to_string(),
                    None => return Err(format!("{sel} has no {}", b.attribute)),
                }
            }
        };
        let v = filters::apply(&raw, &b.filters, now)?;
        Ok(if v.is_empty() && b.text.is_none() && b.default.is_some() {
            Some(render(b.default.as_deref().unwrap_or(""), vars)?)
        } else {
            Some(v)
        })
    }

    /// The releases of a JSON answer.
    fn read_json(&self, text: &str, vars: &Vars, q: &Query) -> Result<Vec<Release>, String> {
        let doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{} answered with JSON that can't be read: {e}", self.def.name))?;
        let rows_def = &self.def.search.rows;
        let row_sel = render(&rows_def.selector, vars)?;
        let mut rows = json_path::rows(&doc, &row_sel);
        if !rows_def.attribute.is_empty() {
            let mut inner = Vec::new();
            for r in rows {
                match json_path::get(&r, &rows_def.attribute) {
                    Some(Value::Array(a)) if rows_def.multiple => {
                        for x in a {
                            let mut merged = r.clone();
                            if let (Some(m), Some(xo)) = (merged.as_object_mut(), x.as_object()) {
                                for (k, v) in xo {
                                    m.insert(k.clone(), v.clone());
                                }
                            }
                            inner.push(merged);
                        }
                    }
                    Some(v) => inner.push(v.clone()),
                    None if rows_def.missing_attribute_equals_no_results => {}
                    None => {}
                }
            }
            rows = inner;
        }
        let now = Utc::now();
        let mut out = Vec::new();
        for row in rows {
            let mut fields: Vec<(String, Option<String>)> = Vec::new();
            let mut bad = false;
            for (name, block) in &self.def.search.fields {
                let mut v = vars.clone();
                for (k, val) in &fields {
                    v.map.insert(format!("Result.{k}"), val.clone().map(Val::Str).unwrap_or(Val::Null));
                }
                let raw = if let Some(t) = &block.text {
                    Some(render(t, &v)?)
                } else if !block.case.is_empty() {
                    let found =
                        block.selector.is_empty().then(String::new).or_else(|| json_path::field(&row, &render(&block.selector, &v).ok()?));
                    let mut hit = None;
                    for (k, val) in &block.case {
                        if k == "*" || found.as_deref() == Some(k.as_str()) {
                            hit = Some(render(&yaml_text(val), &v)?);
                            break;
                        }
                    }
                    hit
                } else {
                    json_path::field(&row, &render(&block.selector, &v)?)
                };
                let raw = raw.or_else(|| block.default.as_ref().map(|d| render(d, &v).unwrap_or_default()));
                match raw {
                    Some(r) => match filters::apply(&r, &block.filters, now) {
                        Ok(val) => fields.push((name.clone(), Some(val))),
                        Err(_) if block.optional => fields.push((name.clone(), None)),
                        Err(_) => {
                            bad = true;
                            break;
                        }
                    },
                    None if block.optional || name.starts_with('_') || !REQUIRED.contains(&name.as_str()) => {
                        fields.push((name.clone(), None))
                    }
                    None => {
                        bad = true;
                        break;
                    }
                }
            }
            if bad {
                continue;
            }
            if let Some(r) = self.release(fields, &self.base, None, q) {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// A release from a row's fields; `None` when it has no title or no link.
    fn release(&self, fields: Vec<(String, Option<String>)>, page_url: &str, date_header: Option<&str>, _q: &Query) -> Option<Release> {
        let now = Utc::now();
        let mut r = Release { protocol: Protocol::Torrent, ..Release::default() };
        let mut peers: Option<i64> = None;
        let mut cats: Vec<u32> = Vec::new();
        for (name, val) in fields {
            let Some(val) = val else { continue };
            let val = val.trim().to_string();
            match name.as_str() {
                "title" => r.title = collapse(&val),
                "details" | "comments" => r.details = resolve(page_url, &val),
                "download" => {
                    if val.starts_with("magnet:") {
                        r.magnet = val.clone();
                    }
                    r.link = resolve(page_url, &val)
                }
                "magnet" => r.magnet = val,
                "infohash" => r.infohash = val,
                "size" => r.size = filters::parse_size(&val),
                "files" => r.files = filters::parse_count(&val),
                "grabs" => r.grabs = filters::parse_count(&val),
                "seeders" => r.seeders = filters::parse_count(&val),
                "leechers" => r.leechers = filters::parse_count(&val),
                "peers" => peers = filters::parse_count(&val),
                "date" => r.date = filters::fuzzy_date(&val, now).or_else(|| date_header.and_then(|d| filters::fuzzy_date(d, now))),
                "category" => {
                    for c in val.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                        cats.extend(self.standard_categories(c));
                    }
                }
                "categorydesc" => {
                    for m in self.def.category_map() {
                        if m.desc.eq_ignore_ascii_case(&val) {
                            cats.extend(categories::id_of(&m.cat));
                        }
                    }
                }
                "imdb" | "imdbid" => {
                    r.imdb = val
                        .trim_start_matches("tt")
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>()
                        .parse()
                        .ok()
                        .filter(|n| *n > 0)
                }
                "tmdbid" => r.tmdb = filters::parse_count(&val).filter(|n| *n > 0),
                "tvdbid" => r.tvdb = filters::parse_count(&val).filter(|n| *n > 0),
                "tvmazeid" => r.tvmaze = filters::parse_count(&val).filter(|n| *n > 0),
                "doubanid" => r.douban = filters::parse_count(&val).filter(|n| *n > 0),
                "genre" => r.genre = val,
                "year" => r.year = filters::parse_count(&val),
                "poster" => r.poster = if val.is_empty() { val } else { resolve(page_url, &val) },
                "description" => r.description = val,
                "downloadvolumefactor" => r.download_factor = filters::parse_factor(&val),
                "uploadvolumefactor" => r.upload_factor = filters::parse_factor(&val),
                "minimumratio" => r.minimum_ratio = filters::parse_factor(&val),
                "minimumseedtime" => r.minimum_seed_time = filters::parse_count(&val),
                _ => {}
            }
        }
        if r.leechers.is_none() {
            if let (Some(p), Some(s)) = (peers, r.seeders) {
                r.leechers = Some((p - s).max(0));
            }
        }
        if r.link.is_empty() && !r.magnet.is_empty() {
            r.link = r.magnet.clone();
        }
        if r.title.is_empty() || r.link.is_empty() {
            return None;
        }
        cats.sort_unstable();
        cats.dedup();
        r.categories = cats;
        r.guid = if !r.details.is_empty() { r.details.clone() } else { r.link.clone() };
        if r.date.is_none() {
            r.date = Some(now);
        }
        Some(r)
    }

    /// A release's file (or magnet) from its download link: through the definition's `download` block when it has
    /// one (the link is the details page then), else the link itself.
    pub async fn grab(&self, link: &str) -> Result<Grabbed, String> {
        if link.starts_with("magnet:") {
            return Ok(Grabbed::Magnet(link.to_string()));
        }
        let vars = self.base_vars();
        let mut target = link.to_string();
        let mut headers = Vec::new();
        if let Some(dl) = &self.def.download {
            headers = self.headers(&dl.headers, &vars)?;
            if let Some(before) = &dl.before {
                let path = render(&before.path, &vars)?;
                let url = if path.is_empty() { link.to_string() } else { self.resolve(&path) };
                let inputs = self.inputs(&before.inputs, &vars)?;
                let f = if before.method == "post" { Fetch::post_form(&url, inputs) } else { Fetch::get(&with_query(&url, &inputs)) };
                self.http.fetch(f.headers(headers.clone())).await?;
            }
            if let Some(ih) = &dl.infohash {
                let page = self.http.fetch(Fetch::get(link).headers(headers.clone())).await?;
                let doc_text = page.text.clone();
                let (hash, title) = {
                    let doc = Document::from(doc_text.as_str());
                    let read = |b: &Option<SelectorBlock>| -> Option<String> {
                        let b = b.as_ref()?;
                        let el = css_doc(&doc, &b.selector).first();
                        let raw = if b.attribute.is_empty() { el.text().to_string() } else { el.attr(&b.attribute)?.to_string() };
                        filters::apply(raw.trim(), &b.filters, Utc::now()).ok()
                    };
                    (read(&ih.hash), read(&ih.title))
                };
                if let Some(hash) = hash.filter(|h| !h.is_empty()) {
                    let dn = title
                        .map(|t| format!("&dn={}", url::form_urlencoded::byte_serialize(t.as_bytes()).collect::<String>()))
                        .unwrap_or_default();
                    return Ok(Grabbed::Magnet(format!("magnet:?xt=urn:btih:{hash}{dn}")));
                }
            }
            if !dl.selectors.is_empty() {
                let page = self.http.fetch(Fetch::get(link).headers(headers.clone())).await?;
                let found = {
                    let doc = Document::from(page.text.as_str());
                    let mut found = None;
                    for b in &dl.selectors {
                        let Ok(sel) = render(&b.selector, &vars) else { continue };
                        let el = css_doc(&doc, &sel).first();
                        if !el.exists() {
                            continue;
                        }
                        let raw = if b.attribute.is_empty() {
                            el.text().to_string()
                        } else {
                            el.attr(&b.attribute).map(|a| a.to_string()).unwrap_or_default()
                        };
                        if let Ok(v) = filters::apply(raw.trim(), &b.filters, Utc::now()) {
                            if !v.is_empty() {
                                found = Some(v);
                                break;
                            }
                        }
                    }
                    found
                };
                match found {
                    Some(v) if v.starts_with("magnet:") => return Ok(Grabbed::Magnet(v)),
                    Some(v) => target = resolve(&page.url, &v),
                    None => return Err(format!("{} has no download link on the release's page", self.def.name)),
                }
            }
        }
        let method_post = self.def.download.as_ref().is_some_and(|d| d.method == "post");
        let f = if method_post { Fetch::post_form(&target, Vec::new()) } else { Fetch::get(&target) };
        let page = self.http.fetch(f.headers(headers).follow(true)).await?;
        if page.url.starts_with("magnet:") {
            return Ok(Grabbed::Magnet(page.url));
        }
        if !looks_like_torrent(&page.bytes) && !looks_like_nzb(&page.bytes) {
            return Err(format!("{} didn't send a torrent file (it sent a web page; the sign-in may have expired)", self.def.name));
        }
        Ok(Grabbed::File(page.bytes))
    }
}

/// The answer a search gives when the site sent it to the sign-in page.
const NEEDS_LOGIN: &str = "\u{0}needs-login";

/// Fields a row can't do without.
const REQUIRED: [&str; 2] = ["title", "download"];

/// Whether `b` is a bencoded torrent.
pub fn looks_like_torrent(b: &[u8]) -> bool {
    b.starts_with(b"d") && b.windows(5).take(4096).any(|w| w == b"4:info" || w == b"info4" || w.starts_with(b"info"))
}

/// Whether `b` is an NZB.
pub fn looks_like_nzb(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(2048)]).to_lowercase();
    head.contains("<nzb")
}

/// `path` on `base`: as it is when it is already an address.
pub fn resolve(base: &str, path: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return String::new();
    }
    if p.starts_with("magnet:") || p.starts_with("http://") || p.starts_with("https://") {
        return p.to_string();
    }
    match url::Url::parse(base).and_then(|b| b.join(p)) {
        Ok(u) => u.to_string(),
        Err(_) => format!("{}{}", base, p.trim_start_matches('/')),
    }
}

/// `url` with `inputs` added as its query (`$raw` added as it is).
pub fn with_query(url: &str, inputs: &[(String, String)]) -> String {
    if inputs.is_empty() {
        return url.to_string();
    }
    let mut q = String::new();
    for (k, v) in inputs {
        if k == "$raw" {
            q.push_str(v.trim_start_matches('&'));
            if !v.is_empty() {
                q.push('&');
            }
            continue;
        }
        q.push_str(&url::form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>());
        q.push('=');
        q.push_str(&url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>());
        q.push('&');
    }
    let q = q.trim_end_matches('&');
    if q.is_empty() {
        return url.to_string();
    }
    let sep = if url.contains('?') {
        if url.ends_with('?') || url.ends_with('&') {
            ""
        } else {
            "&"
        }
    } else {
        "?"
    };
    format!("{url}{sep}{q}")
}

/// Runs of white space as one space.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The form `selector` on `html`: its action and the fields it would send (inputs with names, selects, text
/// areas), plus `selectorinputs` read from the page.
pub fn read_form(
    html: &str,
    selector: &str,
    extra: &indexmap::IndexMap<String, SelectorBlock>,
) -> Result<(String, Vec<(String, String)>), String> {
    let doc = Document::from(html);
    let form = css_doc(&doc, selector).first();
    if !form.exists() {
        return Err(
            "The sign-in form is not on the site's sign-in page (the site may have changed, or a Cloudflare check is in the way)".into()
        );
    }
    let action = form.attr("action").map(|a| a.to_string()).unwrap_or_default();
    let mut fields = Vec::new();
    for input in form.css("input[name]").iter() {
        let kind = input.attr("type").map(|t| t.to_lowercase()).unwrap_or_default();
        if (kind == "checkbox" || kind == "radio") && !input.has_attr("checked") {
            continue;
        }
        if kind == "submit" || kind == "image" {
            continue;
        }
        let name = input.attr("name").map(|n| n.to_string()).unwrap_or_default();
        fields.push((name, input.attr("value").map(|v| v.to_string()).unwrap_or_default()));
    }
    for sel in form.css("select[name]").iter() {
        let name = sel.attr("name").map(|n| n.to_string()).unwrap_or_default();
        let opt = sel.css("option[selected]").first();
        let opt = if opt.exists() { opt } else { sel.css("option").first() };
        fields.push((name, opt.attr("value").map(|v| v.to_string()).unwrap_or_else(|| opt.text().to_string())));
    }
    for ta in form.css("textarea[name]").iter() {
        fields.push((ta.attr("name").map(|n| n.to_string()).unwrap_or_default(), ta.text().to_string()));
    }
    for (name, b) in extra {
        let el = css_doc(&doc, &b.selector).first();
        let raw =
            if b.attribute.is_empty() { el.text().to_string() } else { el.attr(&b.attribute).map(|a| a.to_string()).unwrap_or_default() };
        let v = filters::apply(raw.trim(), &b.filters, Utc::now())?;
        fields.retain(|(n, _)| n != name);
        fields.push((name.clone(), v));
    }
    Ok((action, fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_queries() {
        assert_eq!(resolve("https://site.test/browse/", "/torrent/1"), "https://site.test/torrent/1");
        assert_eq!(resolve("https://site.test/", "dl.php?id=1"), "https://site.test/dl.php?id=1");
        assert_eq!(resolve("https://site.test/", "magnet:?xt=1"), "magnet:?xt=1");
        assert_eq!(
            with_query("https://s/t", &[("q".into(), "a b".into()), ("$raw".into(), "&c=1&c=2".into())]),
            "https://s/t?q=a+b&c=1&c=2"
        );
        assert_eq!(with_query("https://s/t?x=1", &[("q".into(), "z".into())]), "https://s/t?x=1&q=z");
    }

    #[test]
    fn decodes_css_escapes_in_contains() {
        assert_eq!(normalize_css(r"td:contains(\00a0GB)"), "td:contains(\"\u{a0}GB\")");
        let doc = Document::from("<table><tr><td>1.5\u{a0}GB</td></tr></table>");
        assert_eq!(css_doc(&doc, r"td:contains(\00a0GB)").length(), 1);
        assert_eq!(css_doc(&doc, "td:nope(").length(), 0, "a selector that can't be read selects nothing");
    }

    #[test]
    fn reads_a_form() {
        let html = r#"<form id="f" action="/takelogin.php"><input name="user"><input type="hidden" name="tok" value="abc">
            <input type="checkbox" name="keep"><select name="lang"><option value="en">en</option></select></form>"#;
        let (action, fields) = read_form(html, "#f", &indexmap::IndexMap::new()).unwrap();
        assert_eq!(action, "/takelogin.php");
        assert!(fields.contains(&("tok".into(), "abc".into())) && fields.contains(&("lang".into(), "en".into())));
        assert!(!fields.iter().any(|(k, _)| k == "keep"));
    }
}
