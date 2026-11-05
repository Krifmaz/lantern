// Ported from autobrr/rls v0.10.0 (taginfo/taginfo.go), MIT (originally moistari/rls; see LICENSE next to this file).
//! Tag info: the known words of release names (sources, resolutions, codecs, groups …), each with the regular
//! expression that finds it, its release type and whether it belongs to that type only. The list is rls's own
//! `taginfo.csv`, loaded once.

use super::goregex;
use super::Type;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::Arc;

/// rls's tag info list, as it ships with rls v0.10.0.
const TAGINFO_CSV: &str = include_str!("taginfo.csv");

/// One known word of release names.
#[derive(Debug)]
pub struct Taginfo {
    /// The word as rls writes it (`WEB-DL`).
    tag: String,
    /// What it means (`Web (DL)`); the tag when the list gives none.
    title: String,
    /// The regular expression that finds it (empty: the tag itself).
    regexp: String,
    /// Extra text some lexers use (genre or group suffixes).
    other: String,
    /// The release type the word belongs to (`Unknown`: any).
    typ: Type,
    /// Whether the word means that release type only.
    excl: bool,
    /// `(?i)^(?:RE)$`, compiled.
    re: Regex,
}

impl Taginfo {
    /// A tag info from its tag, title, regexp, other, release type and exclusive flag (`"1"`), as rls's
    /// `taginfo.New`; the problem in words when it can't be made.
    pub fn new(tag: &str, title: &str, re: &str, other: &str, typ: &str, excl: &str) -> Result<Taginfo, String> {
        if tag.is_empty() {
            return Err("must define tag".into());
        }
        let title = if title.is_empty() { tag } else { title };
        let typ = Type::parse(typ);
        let re_text = if re.is_empty() { format!(r"\Q{tag}\E") } else { re.to_string() };
        let compiled =
            goregex::try_compile(&format!("(?i)^(?:{re_text})$")).ok_or_else(|| format!("tag {tag:?} has invalid regexp {re:?}"))?;
        Ok(Taginfo { tag: tag.into(), title: title.into(), regexp: re.into(), other: other.into(), typ, excl: excl == "1", re: compiled })
    }

    /// The word as rls writes it.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// What it means.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Its own regular expression (empty when it is the tag itself).
    pub fn regexp(&self) -> &str {
        &self.regexp
    }

    /// The extra text.
    pub fn other(&self) -> &str {
        &self.other
    }

    /// The release type it belongs to.
    pub fn typ(&self) -> Type {
        self.typ
    }

    /// Whether it means that release type only.
    pub fn excl(&self) -> bool {
        self.excl
    }

    /// The regular expression that finds it (Go syntax): its own, or the tag quoted.
    pub fn re(&self) -> String {
        if self.regexp.is_empty() {
            format!(r"\Q{}\E", self.tag)
        } else {
            self.regexp.clone()
        }
    }

    /// Whether `s` is this word.
    pub fn is_match(&self, s: &str) -> bool {
        self.re.is_match(s)
    }
}

/// Tag infos by kind (`source`, `resolution`, `group` …), each list in the file's order.
pub type Infos = BTreeMap<String, Vec<Arc<Taginfo>>>;

/// Finds the first of a list of tag infos that a word is (rls's `taginfo.Find`).
#[derive(Debug, Clone)]
pub struct Finder(Arc<Vec<Arc<Taginfo>>>);

impl Finder {
    /// A finder over `infos` (none: never finds anything).
    pub fn new(infos: Option<&Vec<Arc<Taginfo>>>) -> Finder {
        Finder(Arc::new(infos.cloned().unwrap_or_default()))
    }

    /// The first tag info that `s` is.
    pub fn find(&self, s: &str) -> Option<&Arc<Taginfo>> {
        self.0.iter().find(|i| i.is_match(s))
    }
}

/// Every tag info of rls's list, by kind, plus `extras` added at the end of their kinds (rls's `taginfo.All`).
pub fn all(extras: &[Infos]) -> Infos {
    let mut infos = load(TAGINFO_CSV).unwrap_or_else(|e| panic!("rls's tag list can't be read: {e}"));
    for extra in extras {
        for (k, v) in extra {
            infos.entry(k.clone()).or_default().extend(v.iter().cloned());
        }
    }
    infos
}

/// Reads a tag info list in rls's CSV form (`Type,Tag,Title,Regexp,Other,ReleaseType,TypeExclusive`).
pub fn load(csv: &str) -> Result<Infos, String> {
    let mut lines = csv.lines();
    let header = lines.next().ok_or("empty csv")?;
    if header != "Type,Tag,Title,Regexp,Other,ReleaseType,TypeExclusive" {
        return Err("must have csv headers Type, Tag, Title, Regexp, Other, ReleaseType, TypeExclusive".into());
    }
    let mut m: Infos = BTreeMap::new();
    for (i, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let v: Vec<&str> = line.split(',').collect();
        if v.len() != 7 {
            return Err(format!("line {}: must have 7 columns", i + 1));
        }
        if m.get(v[0]).is_some_and(|l| l.iter().any(|t| t.tag == v[1])) {
            return Err(format!("line {}: type {:?} with tag {:?} previously defined", i + 1, v[0], v[1]));
        }
        let info = Taginfo::new(v[1], v[2], v[3], v[4], v[5], v[6]).map_err(|e| format!("line {}: {e}", i + 1))?;
        m.entry(v[0].to_string()).or_default().push(Arc::new(info));
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rls's TestTagInfo_regexp: every tag without `$` is matched by its own regexp.
    #[test]
    fn every_tag_matches_its_own_regexp() {
        let infos = all(&[]);
        for (typ, list) in &infos {
            for info in list {
                if info.tag().contains('$') {
                    continue;
                }
                let re = if info.regexp().is_empty() { info.tag().to_string() } else { info.regexp().to_string() };
                let re = goregex::compile(&format!("^(?i){re}$"));
                assert!(re.is_match(info.tag()), "type {typ} tag {} does not match {:?}", info.tag(), info.regexp());
            }
        }
    }

    /// rls's TestTagInfo_find: a genre is found whatever its case.
    #[test]
    fn finds_whatever_the_case() {
        let f = Finder::new(all(&[]).get("genre"));
        for s in ["anime", "Anime", "ANIME", "ANiME"] {
            assert_eq!(f.find(s).map(|i| i.tag()), Some("Anime"));
        }
    }
}
