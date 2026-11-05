// Ported from autobrr/rls v0.10.0 (lex.go, reutil/reutil.go), MIT (originally moistari/rls; see LICENSE next to
// this file).
//! The lexers that cut a release name into tags. Each lexer looks at the name from position `i` to `n` and, when
//! it knows what is there, adds tags at the start or the end and moves `i` or `n`. "Once" lexers run first, one
//! time each (whitespace, extension, meta data, group); the others run in turn on every remaining piece.

use super::goregex::compile_bytes;
use super::taginfo::{Finder, Infos, Taginfo};
use super::{Tag, TagType};
use regex::bytes::Regex;
use std::collections::HashSet;
use std::sync::Arc;

/// What a lexer is made with: the tag infos, the delimiter pattern and the short tags.
pub struct Init<'a> {
    /// The tag infos by kind.
    pub infos: &'a Infos,
    /// Matches delimiters at the start.
    pub delim: &'a Regex,
    /// Tag words shorter than five letters, in capitals.
    pub short: &'a HashSet<String>,
}

/// The state lexers change: the tags found at the start and at the end, and the part left (`i..n`).
pub struct State<'a> {
    /// The release name.
    pub src: &'a [u8],
    /// The name with `_`, `,` and `+` as spaces.
    pub buf: &'a [u8],
    /// Tags found from the start.
    pub start: Vec<Tag>,
    /// Tags found from the end (last first).
    pub end: Vec<Tag>,
    /// Where the part left starts.
    pub i: usize,
    /// Where the part left ends.
    pub n: usize,
}

/// A lexer function: whether it matched. A "once" lexer's changes always stay; a multi lexer changes the state
/// only when it matches.
pub type LexFn = Box<dyn Fn(&mut State) -> bool + Send + Sync>;

/// A lexer, made: its function, whether it runs once, and whether it waits until something else matched first.
pub struct Lexer {
    /// The function.
    pub lex: LexFn,
    /// Runs once, before the others.
    pub once: bool,
    /// Never matches the very first piece.
    pub not_first: bool,
}

/// Makes a lexer from what it needs.
pub type LexerFactory = Box<dyn Fn(&Init) -> Lexer + Send + Sync>;

/// A Go `[]byte` as text.
fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A tag of kind `typ` from byte values.
fn tag(typ: TagType, f: Option<Finder>, v: &[&[u8]]) -> Tag {
    Tag::new(typ, f, v.iter().map(|b| s(b)).collect())
}

/// Every group of a match as bytes (empty for a group that took no part), the whole match first.
fn groups<'h>(c: &regex::bytes::Captures<'h>) -> Vec<&'h [u8]> {
    (0..c.len()).map(|k| c.get(k).map(|m| m.as_bytes()).unwrap_or(b"")).collect()
}

/// rls's `reutil.Join`: the strings as alternatives, each quoted when `quote`.
pub fn join(quote: bool, strs: &[String]) -> String {
    strs.iter().map(|s| if quote { format!(r"\Q{s}\E") } else { s.clone() }).collect::<Vec<_>>().join("|")
}

/// rls's `reutil.Build`: `strs` as one group of alternatives, with `config` options `i` (ignore case), `^` (start
/// anchor), `a` (`\b` before), `q` (quote each), `b` (`\b` after) and `$` (end anchor).
pub fn build(config: &str, strs: &[String]) -> String {
    let mut out = String::new();
    if config.contains('i') {
        out.push_str("(?i)");
    }
    if config.contains('^') {
        out.push('^');
    }
    if config.contains('a') {
        out.push_str(r"\b");
    }
    out.push('(');
    out.push_str(&join(config.contains('q'), strs));
    out.push(')');
    if config.contains('b') {
        out.push_str(r"\b");
    }
    if config.contains('$') {
        out.push('$');
    }
    out
}

/// rls's `reutil.Taginfo`: [`build`] over the tag infos' regular expressions.
pub fn taginfo_re(config: &str, infos: &[Arc<Taginfo>]) -> String {
    build(config, &infos.iter().map(|i| i.re()).collect::<Vec<_>>())
}

/// The tag infos of `kind` (none when there are none).
fn kind<'a>(infos: &'a Infos, kind: &str) -> &'a [Arc<Taginfo>] {
    infos.get(kind).map(Vec::as_slice).unwrap_or(&[])
}

/// A finder over the tag infos of `kind`.
fn finder(infos: &Infos, kind: &str) -> Option<Finder> {
    Some(Finder::new(infos.get(kind)))
}

/// rls's default lexers, in order.
pub fn default_lexers() -> Vec<LexerFactory> {
    vec![
        // once
        Box::new(|_| trim_whitespace_lexer()),
        Box::new(ext_lexer),
        Box::new(|init| {
            meta_lexer(
                init,
                &[
                    // [[ type:value ]]
                    ["", "[[", "]]", r"([a-zA-Z][a-zA-Z0-9_]{0,15}):\s*([^ \t\]]{1,32})"],
                    // [REQ]
                    ["req", "[", "]", r"(REQ(?:UEST)?)"],
                    // (REQ)
                    ["req", "(", ")", r"(REQ(?:UEST)?)"],
                    // {REQ}
                    ["req", "{", "}", r"(REQ(?:UEST)?)"],
                    // [ABCD1234]
                    ["sum", "[", "]", r"([0-9A-F]{8})"],
                    // [site]
                    ["site", "[", "]", r"([^ \t\]]{1,32})"],
                    // -={site}=-
                    ["site", "-={", "}=-", r"([^ \t\}]{1,32})"],
                    // {{pass}}
                    ["pass", "{{", "}}", r"([^ \t\}]{1,32})"],
                ],
            )
        }),
        Box::new(group_lexer),
        // multi
        Box::new(|init| regexp_lexer(init, TagType::Size, true)),
        Box::new(|init| regexp_lexer(init, TagType::Platform, true)),
        Box::new(|init| regexp_lexer(init, TagType::Arch, true)),
        Box::new(|init| regexp_lexer(init, TagType::Source, true)),
        Box::new(|init| regexp_lexer(init, TagType::Resolution, true)),
        Box::new(|init| regexp_source_lexer(init, TagType::Collection, true)),
        Box::new(|init| {
            series_lexer(
                init,
                &[
                    // s02, S01E01
                    r"(?i)^s(?P<s>[0-8]?\d)[\-\._ ]?(?:e(?P<e>\d{1,5}))?\b",
                    // S01E02E03, S01E02-E03, S01E03.E04.E05
                    r"(?i)^s(?P<s>[0-8]?\d)(?P<m>(?:[\-\._ ]?e\d{1,5}){1,5})\b",
                    // S01S02S03
                    r"(?i)^(?P<S>(?:s[0-8]?\d){2,4})\b",
                    // 2x1, 1x01
                    r"(?i)^(?P<s>[0-8]?\d)x(?P<e>\d{1,3})\b",
                    // S01 - 02v3, S07-06, s03-5v.9
                    r"(?i)^s(?P<s>[0-8]?\d)[\-\._ ]{1,3}(?P<e>\d{1,5})(?:[\-\._ ]{1,3}(?P<v>v\d+(?:\.\d+){0,2}))?\b",
                    // Season.01.Episode.02, Series.01.Ep.02, Series.01, Season.01
                    r"(?i)^(?:series|season|s)[\-\._ ]?(?P<s>[0-8]?\d)(?:[\-\._ ]?(?:episode|ep)(?P<e>\d{1,5}))?\b",
                    // Vol.1.No.2, vol1no2
                    r"(?i)^vol(?:ume)?[\-\._ ]?(?P<s>\d{1,3})(?:[\-\._ ]?(?:number|no)[\-\._ ]?(?P<e>\d{1,5}))\b",
                    // Episode 15, E009, Ep. 007, Ep.05-07
                    r"(?i)^e(?:p(?:isode)?[\-\._ ]{1,3})?(?P<e>\d{1,5})(?:[\-\._ ]{1,3}\d{1,3})?\b",
                    // 10v1.7, 13v2
                    r"(?i)^(?P<e>\d{1,5})(?P<v>v[\-\._ ]?\d+(?:\.\d){0,2})\b",
                    // S01.Disc02, s01D3, Series.01.Disc.02, S02DVD3
                    r"(?i)^(?:series|season|s)[\-\._ ]?(?P<s>[0-8]?\d)[\-\._ ]?(?P<d>(?:disc|disk|dvd|d)[\-\._ ]?(?:\d{1,3}))\b",
                    // s1957e01
                    r"(?i)^s(?P<s>(?:19|20)\d\d)e(?P<e>\d{2,4})\b",
                ],
            )
        }),
        Box::new(|init| {
            disc_source_year_lexer(
                init,
                &[
                    // VLS2004, 2DVD1999, 4CD2003
                    r"(?i)^(?P<d>[2-9])?(?P<s>cd|ep|lp|dvd|vls|vinyl)(?P<y>(?:19|20)\d\d)\b",
                    // WEB2007
                    r"(?i)^(?P<s>web)(?P<y>20\d\d)\b",
                ],
            )
        }),
        Box::new(|init| {
            disc_lexer(
                init,
                &[
                    // D01, Disc.1
                    r"(?i)^(?P<t>d)(?:is[ck][\-\._ ])?(?P<c>\d{1,3})\b",
                    // 12DiSCS
                    r"(?i)^(?P<c>\d{1,3})[\-\._ ]?di(?P<t>s)[ck]s?\b",
                    // CD1, CD30
                    r"(?i)^(?P<t>cd)[\-\._ ]?(?P<c>\d{1,2})\b",
                    // DVD2, DVD24 -- does not match DVD5/DVD9
                    r"(?i)^(?P<t>dvd)[\-\._ ]?(?P<c>[1-46-8]|[12]\d)\b",
                    // 2xDVD9
                    r"(?i)^(?P<c>\d{1,2})(?P<t>x(?:dvd9))\b",
                    // 2DVD9, 6DVD9
                    r"(?i)^(?P<c>[2-9])(?P<z>dvd9)\b",
                    // 2xVinyl, 3xDVD, 4xCD
                    r"(?i)^(?P<c>\d{1,2})(?P<t>x(?:cd|ep|lp|dvda|dvd|vls|vinyl)s?)\b",
                    // 2Vinyl, 6DVD
                    r"(?i)^(?P<c>\d{1,2})(?P<x>(?:cd|ep|lp|dvda|dvd|vls|vinyl)s?)\b",
                    // CDS3
                    r"(?i)^(?:(?P<x>cd)s)(?P<c>\d{1,2})\b",
                    // 2CDS
                    r"(?i)^(?P<c>[2-9])(?P<x>cds)\b",
                ],
            )
        }),
        Box::new(|_| {
            date_lexer(&[
                // 2006-01-02, 2006
                r"(?i)^(?P<2006>(?:19|20)\d{2})(?:[\-\._ ](?P<01>\d{2})[\-\._ ](?P<02>\d{2}))?\b",
                // 2006-01
                r"(?i)^(?P<2006>(?:19|20)\d{2})?:[\-\._ ](?P<01>\d{2})\b",
                // 13-02-2006
                r"(?i)^(?P<01>\d{2})[\-\._ ](?P<02>\d{2})[\-\._ ](?P<2006>(?:19|20)\d{2})\b",
                // 02-13-2006
                r"(?i)^(?P<02>\d{2})[\-\._ ](?P<01>\d{2})[\-\._ ](?P<2006>(?:19|20)\d{2})\b",
                // 2nd Jan 2006, 13 Dec 2011, Nov 1999
                r"(?i)^(?:(?P<_2>\d{1,2})(?:th|st|nd|rd)?[\-\._ ])?(?P<Jan>Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[\-\._ ](?P<2006>(?:19|20)\d{2})\b",
                // 01-August-1998
                r"(?i)^(?P<_2>\d{1,2})[\-\._ ](?P<January>January|February|March|April|May|June|July|August|September|October|November|December)[\-\._ ](?P<2006>(?:19|20)\d{2})\b",
                // MAY-30-1992
                r"(?i)^(?P<Jan>Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[\-\._ ](?P<_2>\d{1,2})[\-\._ ](?P<2006>(?:19|20)\d{2})\b",
                // 17.12.15, 20-9-9
                r"(?i)^(?P<YY>[12]\d)[\-\._ ](?P<01>\d\d?)[\-\._ ](?P<02>\d\d?)\b",
            ])
        }),
        Box::new(|_| {
            version_lexer(&[
                // v1.17, v1, v1.2a, v1b
                r"(?i)^(version[\-\._ ])?(?P<v>v[\-\._ ]?\d{1,2}(?:[\._ ]\d{1,2}[a-z]?\d*){0,3})\b",
                // v2012, v20120803, v20120803, v1999.08.08
                r"(?i)^(version[\-\._ ])?(?P<v>v[\-\._ ]?(?:19|20)\d\d(?:[\-\._ ]?\d\d?){0,2})\b",
                // v60009
                r"(?i)^(version[\-\._ ])?(?P<v>v[\-\._ ]?\d{4,10})\b",
                // Version 2004, Version 21H2, Version 22H1
                r"(?i)^version[\-\._ ](?P<V>\d{2,}|\d{2}[a-z]{1,2}\d{1,2})\b",
                // 11.09.1, 100.000.99999999999, 23.3.2.458
                r"(?i)^(?P<u>\d{1,3}\.\d{1,3}\.\d{1,16}(\.\d{1,16})?)\b",
            ])
        }),
        Box::new(|init| regexp_source_lexer(init, TagType::Codec, true)),
        Box::new(|init| regexp_source_lexer(init, TagType::Hdr, true)),
        Box::new(|init| regexp_lexer(init, TagType::BitDepth, true)),
        Box::new(audio_lexer),
        Box::new(|init| regexp_lexer(init, TagType::Channels, true)),
        Box::new(|init| regexp_lexer(init, TagType::Other, true)),
        Box::new(|init| regexp_lexer(init, TagType::Cut, true)),
        Box::new(|init| regexp_lexer(init, TagType::Edition, true)),
        Box::new(|init| regexp_lexer(init, TagType::Language, false)),
        Box::new(|init| regexp_lexer(init, TagType::Region, true)),
        Box::new(|init| regexp_lexer(init, TagType::Container, true)),
        Box::new(genre_lexer),
        Box::new(|_| id_lexer()),
        Box::new(|_| episode_lexer()),
    ]
}

/// Leading and trailing whitespace (and stars).
fn trim_whitespace_lexer() -> Lexer {
    let ws = format!(r"(\t|\n|\f|\r| |⭐|{})+", '\u{fe0f}');
    let prefix = compile_bytes(&format!("^{ws}"));
    let suffix = compile_bytes(&format!("{ws}$"));
    Lexer {
        lex: Box::new(move |st| {
            if let Some(c) = prefix.captures(&st.src[st.i..st.n]) {
                let m = groups(&c);
                st.start.push(tag(TagType::Whitespace, None, &m));
                st.i += m[0].len();
            }
            if let Some(c) = suffix.captures(&st.src[st.i..st.n]) {
                let m = groups(&c);
                st.end.push(tag(TagType::Whitespace, None, &m));
                st.n -= m[0].len();
            }
            true
        }),
        once: true,
        not_first: false,
    }
}

/// Go's `time.Parse` for the one layout element `f` (`2006`, `01`, `02`, `_2`, `Jan`, `January`) and value `t`:
/// the year, month or day; `None` when Go would refuse it.
fn parse_date_part(f: &str, t: &str) -> Option<i32> {
    let two_digits = |t: &str| (t.len() == 2 && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse::<i32>().ok()).flatten();
    match f {
        "2006" => (t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok()).flatten(),
        "01" => two_digits(t).filter(|m| (1..=12).contains(m)),
        "02" => two_digits(t).filter(|d| (1..=31).contains(d)),
        "_2" => {
            let t = t.strip_prefix(' ').unwrap_or(t);
            (!t.is_empty() && t.len() <= 2 && t.bytes().all(|b| b.is_ascii_digit()))
                .then(|| t.parse::<i32>().ok())
                .flatten()
                .filter(|d| (1..=31).contains(d))
        }
        "Jan" => SHORT_MONTHS.iter().position(|m| m.eq_ignore_ascii_case(t)).map(|i| i as i32 + 1),
        "January" => month_name(t),
        _ => None,
    }
}

/// The short month names Go knows.
const SHORT_MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The month numbered by its full name in any case (Go's `time.Parse("January", s)`).
pub fn month_name(s: &str) -> Option<i32> {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    MONTHS.iter().position(|m| m.eq_ignore_ascii_case(s)).map(|i| i as i32 + 1)
}

/// A date.
fn date_lexer(strs: &[&str]) -> Lexer {
    let lexer = NamedCapture::new(strs);
    Lexer {
        lex: Box::new(move |st| {
            let Some((s, v, i, n)) = lexer.find(st) else { return false };
            let (mut year, mut month, mut day) = (String::new(), String::new(), String::new());
            let mut matched = true;
            for (f, t) in &v {
                let (f, t) = if f == "YY" { ("2006".to_string(), format!("20{t}")) } else { (f.clone(), t.clone()) };
                match parse_date_part(&f, &t) {
                    Some(x) => match f.as_str() {
                        "2006" => year = x.to_string(),
                        "01" | "Jan" | "January" => month = format!("{x:02}"),
                        _ => day = format!("{x:02}"),
                    },
                    None => matched = false,
                }
            }
            if !matched {
                return false;
            }
            st.start.push(Tag::new(TagType::Date, None, vec![s, year, month, day]));
            st.i = i;
            st.n = n;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// A season and episode (or several), with a version or disc.
fn series_lexer(init: &Init, strs: &[&str]) -> Lexer {
    let sourcef = finder(init.infos, "source");
    let lexer = NamedCapture::new(strs);
    let mlt = compile_bytes(r"(?i)s(\d?\d)");
    let mny = compile_bytes(r"(?i)[\-\._ ]?e(\d{1,5})");
    let dsc = compile_bytes(r"(?i)^disc|disk|dvd|d");
    Lexer {
        lex: Box::new(move |st| {
            let Some((mut s, v, i, n)) = lexer.find(st) else { return false };
            let (mut series, mut episode, mut version, mut disc, mut multi, mut many) =
                (String::new(), String::new(), String::new(), String::new(), String::new(), String::new());
            for (k, val) in v {
                match k.as_str() {
                    "s" => series = val,
                    "e" => episode = val,
                    "v" => version = val,
                    "d" => disc = val,
                    "S" => multi = val,
                    "m" => many = val,
                    _ => return false,
                }
            }
            let mut tags = Vec::new();
            if !series.is_empty() || !episode.is_empty() {
                if !version.is_empty() {
                    if let Some(x) = s.strip_suffix(version.as_str()) {
                        s = x.to_string();
                    }
                }
                if !disc.is_empty() {
                    if let Some(x) = s.strip_suffix(disc.as_str()) {
                        s = x.to_string();
                    }
                }
                let mut l = vec![s.clone(), series.clone()];
                if many.is_empty() {
                    l.push(episode.clone());
                } else {
                    for c in mny.captures_iter(many.as_bytes()) {
                        l.push(String::from_utf8_lossy(&c[1]).into_owned());
                    }
                }
                tags.push(Tag::new(TagType::Series, None, l));
            }
            if !version.is_empty() {
                tags.push(Tag::new(TagType::Version, None, vec![version.clone(), version.clone()]));
            }
            if !disc.is_empty() {
                let m = dsc.find(disc.as_bytes()).map(|m| m.as_bytes().len()).unwrap_or(0).min(disc.len());
                let disctyp = disc[..m].to_uppercase();
                let rest = &disc[disctyp.len().min(disc.len())..];
                let num = rest.trim().to_string();
                if disctyp == "DVD" {
                    tags.push(Tag::new(TagType::Source, sourcef.clone(), vec![disctyp.clone(), disctyp.clone()]));
                    tags.push(Tag::new(TagType::Disc, None, vec![rest.to_string(), disctyp, num]));
                } else {
                    tags.push(Tag::new(TagType::Disc, None, vec![disc.clone(), disctyp, num]));
                }
            }
            // S01S02S03 …
            for c in mlt.captures_iter(multi.as_bytes()) {
                tags.push(Tag::new(TagType::Series, None, vec![s_of(&c, 0), s_of(&c, 1), String::new()]));
            }
            if series.len() == 4 && (series.starts_with("19") || series.starts_with("20")) {
                tags.push(Tag::new(TagType::Date, None, vec![String::new(), series.clone(), String::new(), String::new()]));
            }
            st.start.extend(tags);
            st.i = i;
            st.n = n;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// Group `k` of a match as text (empty when it took no part).
fn s_of(c: &regex::bytes::Captures, k: usize) -> String {
    c.get(k).map(|m| s(m.as_bytes())).unwrap_or_default()
}

/// A music catalogue id in brackets (`(ABC-123)`).
fn id_lexer() -> Lexer {
    let alpha = compile_bytes(r"[A-Z]");
    let digit = compile_bytes(r"\d");
    let ws = compile_bytes(r"[\-\._ ]");
    let re = compile_bytes(r"^([A-Z\d\-\_\. ]{2,24})\)");
    let lb = compile_bytes(r"\([\._ ]{0,2}$");
    Lexer {
        lex: Box::new(move |st| {
            if !lb.is_match(&st.src[..st.i]) {
                return false;
            }
            let Some(c) = re.captures(&st.buf[st.i..st.n]) else { return false };
            let m = groups(&c);
            let (a, d, w) = (alpha.find_iter(m[1]).count(), digit.find_iter(m[1]).count(), ws.find_iter(m[1]).count());
            if (a == 0 && d > 4 && w < 4) || (a > 1 && d > 1 && (a + d > 4) && w < 4) {
                let l = m[0].len();
                st.start.push(tag(TagType::Id, None, &[&st.src[st.i..st.i + l], m[1]]));
                st.i += l;
                return true;
            }
            false
        }),
        once: false,
        not_first: false,
    }
}

/// A lone episode number (`- 2 -`, `- 867 (`, `- 100 [`).
fn episode_lexer() -> Lexer {
    let re = compile_bytes(r"^(\d{1,4})(\b|[\._ ]?[\-\[\]\(\)\{\}])");
    let lb = compile_bytes(r"-[\-\._ ]{1,3}$");
    Lexer {
        lex: Box::new(move |st| {
            if !lb.is_match(&st.src[..st.i]) {
                return false;
            }
            let Some(c) = re.captures(&st.src[st.i..st.n]) else { return false };
            let m = groups(&c);
            if st.src[st.i + m[1].len()..].starts_with(b",") {
                return false;
            }
            st.start.push(tag(TagType::Series, None, &[m[1], b"", m[1]]));
            if !m[2].is_empty() {
                st.start.push(tag(TagType::Delim, None, &[m[2], m[2]]));
            }
            st.i += m[0].len();
            true
        }),
        once: false,
        not_first: false,
    }
}

/// A version (`v1.2`, `Version 2004`, `11.09.1`); never the first piece.
fn version_lexer(strs: &[&str]) -> Lexer {
    let lexer = NamedCapture::new(strs);
    Lexer {
        lex: Box::new(move |st| {
            let Some((s, v, i, n)) = lexer.find(st) else { return false };
            let mut version = String::new();
            for (k, val) in v {
                match k.as_str() {
                    "v" => {
                        version = s.to_lowercase();
                        if version.starts_with("version") {
                            version = version.get("version".len() + 1..).unwrap_or_default().to_string();
                        }
                        version = version.replace(' ', ".");
                    }
                    "V" => version = val,
                    "u" => version = format!("v{val}"),
                    _ => return false,
                }
            }
            st.start.push(Tag::new(TagType::Version, None, vec![s, version]));
            st.i = i;
            st.n = n;
            true
        }),
        once: false,
        not_first: true,
    }
}

/// A disc, source and year in one (`VLS2004`, `2DVD1999`, `WEB2007`).
fn disc_source_year_lexer(init: &Init, strs: &[&str]) -> Lexer {
    let sourcef = finder(init.infos, "source");
    let lexer = NamedCapture::new(strs);
    Lexer {
        lex: Box::new(move |st| {
            let Some((s, v, i, n)) = lexer.find(st) else { return false };
            let (mut disc, mut source, mut year) = (String::new(), String::new(), String::new());
            for (k, val) in v {
                match k.as_str() {
                    "d" => disc = val,
                    "s" => source = val,
                    "y" => year = val,
                    _ => return false,
                }
            }
            if !disc.is_empty() {
                st.start.push(Tag::new(TagType::Disc, None, vec![disc.clone(), "X".into(), disc]));
            }
            st.start.push(Tag::new(TagType::Source, sourcef.clone(), vec![source.clone(), source]));
            st.start.push(Tag::new(TagType::Date, None, vec![year.clone(), year, String::new(), String::new()]));
            // rls moves past the match twice here (its `i` is already after the match)
            st.i = i + s.len();
            st.n = n;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// A disc (`D01`, `CD2`, `2xDVD9`, `2Vinyl`).
fn disc_lexer(init: &Init, strs: &[&str]) -> Lexer {
    let sourcef = finder(init.infos, "source");
    let sizef = finder(init.infos, "size");
    let lexer = NamedCapture::new(strs);
    let re = compile_bytes(r"(?i)^dvd|cd|d|s|x");
    Lexer {
        lex: Box::new(move |st| {
            let Some((s, v, j, k)) = lexer.find(st) else { return false };
            let (mut typ, mut c, mut x, mut z) = (String::new(), String::new(), String::new(), String::new());
            for (key, val) in v {
                match key.as_str() {
                    "c" => c = val,
                    "t" => typ = val.to_uppercase(),
                    "x" => x = val.to_uppercase(),
                    "z" => {
                        typ = "Z".into();
                        z = val.to_uppercase();
                    }
                    _ => return false,
                }
            }
            if !typ.is_empty() {
                let l = re.find(s.as_bytes()).map(|m| m.as_bytes().len()).unwrap_or(0).min(typ.len());
                typ = typ[..l].to_uppercase();
            }
            let cut = (c.len() + 1).min(s.len());
            let (head, tail) = (s[..cut].to_string(), s[cut..].to_string());
            let tags = match typ.as_str() {
                "D" | "S" => vec![Tag::new(TagType::Disc, None, vec![s.clone(), typ.clone(), c.clone()])],
                "DVDA" | "DVD" | "CD" => {
                    let t = typ.len().min(s.len());
                    vec![
                        Tag::new(TagType::Source, sourcef.clone(), vec![s[..t].to_string(), typ.clone(), typ.clone()]),
                        Tag::new(TagType::Disc, None, vec![s[t..].to_string(), typ.clone(), c.clone()]),
                    ]
                }
                "X" => {
                    if tail.to_uppercase() == "DVD9" {
                        vec![
                            Tag::new(TagType::Disc, None, vec![head, typ.clone(), c.clone()]),
                            Tag::new(TagType::Size, sizef.clone(), vec![tail.clone(), tail]),
                        ]
                    } else {
                        vec![
                            Tag::new(TagType::Disc, None, vec![head, typ.clone(), c.clone()]),
                            Tag::new(TagType::Source, sourcef.clone(), vec![tail.clone(), tail]),
                        ]
                    }
                }
                "Z" => vec![
                    Tag::new(TagType::Disc, None, vec![head, "X".into(), c.clone()]),
                    Tag::new(TagType::Size, sizef.clone(), vec![tail, z]),
                ],
                "" => vec![
                    Tag::new(TagType::Disc, None, vec![head, "X".into(), c.clone()]),
                    Tag::new(TagType::Source, sourcef.clone(), vec![tail, x]),
                ],
                _ => return false,
            };
            st.start.extend(tags);
            st.i = j;
            st.n = k;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// An audio format, with its channels (`DDP5.1`), leaving an attached codec for the codec lexer.
fn audio_lexer(init: &Init) -> Lexer {
    let audio = kind(init.infos, "audio");
    let channels = kind(init.infos, "channels");
    let v: Vec<String> = channels.iter().map(|i| i.tag().replace('.', r"[\._ ]?")).collect();
    let codec = taginfo_re("", kind(init.infos, "codec"));
    let re =
        compile_bytes(&format!(r"{}(?:[\-\._ ]?({}))?(?:\b|[\-\._ ]|({}(?:\b|[\-\._ ])))", taginfo_re("^i", audio), v.join("|"), codec));
    let audiof = finder(init.infos, "audio");
    let channelsf = finder(init.infos, "channels");
    Lexer {
        lex: Box::new(move |st| {
            let Some(c) = re.captures(&st.src[st.i..st.n]) else { return false };
            let m = groups(&c);
            let mut m0 = m[0];
            if !m[3].is_empty() {
                m0 = m0.strip_suffix(m[3]).unwrap_or(m0);
            }
            let l = m0.len();
            if !m[2].is_empty() {
                m0 = m0.strip_suffix(m[2]).unwrap_or(m0);
            }
            st.start.push(tag(TagType::Audio, audiof.clone(), &[m0, m[1]]));
            if !m[2].is_empty() {
                st.start.push(tag(TagType::Channels, channelsf.clone(), &[m[2], m[2]]));
            }
            st.i += l;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// A genre in brackets (`(Drama)`) or a genre word.
fn genre_lexer(init: &Init) -> Lexer {
    let genre = kind(init.infos, "genre");
    let v: Vec<String> = genre.iter().map(|i| i.re()).collect();
    let tagv: Vec<String> = genre.iter().filter(|i| !i.other().is_empty()).map(|i| i.other().to_string()).collect();
    let s = format!(r"\(?({})\s*\)", v.join("|"));
    let re = compile_bytes(&format!("(?i)^{s}"));
    let lb = compile_bytes(&format!(r"(?i)\(\s*{s}$"));
    let other = compile_bytes(&format!(r"(?i)^({})\b", tagv.join("|")));
    let genref = finder(init.infos, "genre");
    Lexer {
        lex: Box::new(move |st| {
            if let Some(c) = re.captures(&st.src[st.i..st.n]) {
                let m = groups(&c);
                if lb.is_match(&st.src[..st.i + m[0].len()]) {
                    st.start.push(tag(TagType::Genre, genref.clone(), &m));
                    st.i += m[0].len();
                    return true;
                }
            }
            if let Some(c) = other.captures(&st.buf[st.i..st.n]) {
                let m = groups(&c);
                st.start.push(tag(TagType::Genre, genref.clone(), &m));
                st.i += m[0].len();
                return true;
            }
            false
        }),
        once: false,
        not_first: false,
    }
}

/// A group name whose words are separated by spaces or underscores.
pub static SPACED_GROUP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| super::goregex::compile(r"(?i)^[a-z_ ]{2,10}$"));

/// The release group at the end (a known group, or the text after the last `-`).
fn group_lexer(init: &Init) -> Lexer {
    const DELIM: u8 = b'-';
    const INVALID: &[u8] = b" _.()[]{}+";
    let year = compile_bytes(r"\b(19|20)\d{2}\b");
    let bracket = super::goregex::compile(r"^[\]\)\}]");
    let group = kind(init.infos, "group");
    let other = kind(init.infos, "other");
    let v: Vec<String> = other.iter().filter(|i| !i.other().is_empty()).map(|i| i.other().to_string()).collect();
    let groupf = finder(init.infos, "group");
    let otherf = finder(init.infos, "other");
    let re = compile_bytes(&format!(r"(?i)[\-\._ ]+{}", taginfo_re("$", group)));
    let special = compile_bytes(&format!(r"(?i)_({})$", v.join("|")));
    let short = init.short.clone();
    Lexer {
        lex: Box::new(move |st| {
            // special end tags on groups
            if let Some(c) = special.captures(&st.src[st.i..st.n]) {
                let m = groups(&c);
                st.end.push(tag(TagType::Other, otherf.clone(), &m));
                st.n -= m[0].len();
            }
            // known groups
            if let Some(c) = re.captures(&st.src[st.i..st.n]) {
                let m = groups(&c);
                st.end.push(tag(TagType::Group, groupf.clone(), &m));
                st.n -= m[0].len();
                return true;
            }
            // clamp to the last year (rls takes the end of the first year found, from where the name is left)
            let mut l = st.i;
            if let Some(m) = year.find(&st.buf[l..st.n]) {
                l = m.end();
            }
            let n = st.n;
            if l > n {
                return false;
            }
            if let Some(j) = st.buf[l..n].iter().rposition(|&b| b == DELIM) {
                let s = &st.src[l + j + 1..n];
                let grp = trim_bytes(s, b" \t_");
                let valid = !grp.is_empty()
                    && (!s.iter().any(|b| INVALID.contains(b)) || (s.len() <= 14 && SPACED_GROUP.is_match(&String::from_utf8_lossy(grp))))
                    && !short.contains(&*String::from_utf8_lossy(grp))
                    && st.end.last().is_none_or(|t| !bracket.is_match(&t.text()));
                if valid {
                    let gs = String::from_utf8_lossy(grp).into_owned();
                    st.end.push(Tag::new(TagType::Group, None, vec![s_bytes(s), gs]));
                    st.end.push(Tag::new(TagType::Delim, None, vec![s_bytes(&st.src[l + j..l + j + 1]), "-".into()]));
                    st.n = l + j;
                }
            }
            false
        }),
        once: true,
        not_first: false,
    }
}

/// Bytes as text.
fn s_bytes(b: &[u8]) -> String {
    s(b)
}

/// `b` without the bytes of `cut` at both ends (Go's `bytes.Trim`).
fn trim_bytes<'a>(b: &'a [u8], cut: &[u8]) -> &'a [u8] {
    let start = b.iter().position(|x| !cut.contains(x)).unwrap_or(b.len());
    let end = b.iter().rposition(|x| !cut.contains(x)).map(|e| e + 1).unwrap_or(start);
    &b[start..end.max(start)]
}

/// File meta data at the start or end (`[[key:value]]`, `[REQ]`, `[ABCD1234]`, `[site]`, `{{pass}}`).
fn meta_lexer(init: &Init, strs: &[[&str; 4]]) -> Lexer {
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut has_two = Vec::new();
    for [_, open, close, pat] in strs {
        let s = format!(r"\s*\Q{open}\E\s*{pat}\s*\Q{close}\E\s*");
        let p = compile_bytes(&format!("^{s}"));
        let n = p.captures_len() - 1;
        assert!(n == 1 || n == 2, "must have exactly 1 or 2 capture groups");
        has_two.push(n == 2);
        prefix.push(p);
        suffix.push(compile_bytes(&format!("{s}$")));
    }
    let keys: Vec<String> = strs.iter().map(|x| x[0].to_string()).collect();
    let one_char: Vec<bool> = strs.iter().map(|x| x[1].len() == 1).collect();
    let delim = init.delim.clone();
    let short = init.short.clone();
    // tries every pattern at `hay`: the first that matches and may be used, with its key and value
    let try_all = move |res: &[Regex], hay: &[u8], prev: &mut HashSet<String>| -> Option<(Vec<u8>, String, Vec<u8>)> {
        for (l, re) in res.iter().enumerate() {
            if let Some(c) = re.captures(hay) {
                let m = groups(&c);
                let (k, v) = if has_two[l] { (s(m[1]), m[2]) } else { (keys[l].clone(), m[1]) };
                let short_tag = one_char[l] && short.contains(&s(v).to_uppercase());
                let ok = !prev.contains(&k) && !short_tag && !v.iter().any(|b| b"\t\r\n\x0c +".contains(b));
                prev.insert(k.clone());
                if ok {
                    return Some((m[0].to_vec(), k, v.to_vec()));
                }
            }
        }
        None
    };
    Lexer {
        lex: Box::new(move |st| {
            let mut prev = HashSet::new();
            let mut d: Vec<u8> = Vec::new();
            let (mut i, mut n) = (st.i, st.n);
            // prefixes
            while i < n {
                if let Some((m0, k, v)) = try_all(&prefix, &st.src[i..n], &mut prev) {
                    if !d.is_empty() {
                        st.start.push(Tag::new(TagType::Delim, None, vec![s(&d), s(&d)]));
                        d.clear();
                    }
                    st.start.push(Tag::new(TagType::Meta, None, vec![s(&m0), k, s(&v)]));
                    i += m0.len() - 1;
                } else if !delim.is_match(&st.src[i..i + 1]) {
                    break;
                } else {
                    d.push(st.src[i]);
                }
                i += 1;
            }
            // backtrack remaining
            if !d.is_empty() {
                i -= d.len();
                d.clear();
            }
            // suffixes
            while i < n {
                if let Some((m0, k, v)) = try_all(&suffix, &st.src[i..n], &mut prev) {
                    if !d.is_empty() {
                        st.end.push(Tag::new(TagType::Delim, None, vec![s(&d), s(&d)]));
                        d.clear();
                    }
                    st.end.push(Tag::new(TagType::Meta, None, vec![s(&m0), k, s(&v)]));
                    n = n - m0.len() + 1;
                } else if !delim.is_match(&st.src[n - 1..n]) {
                    break;
                } else {
                    d.insert(0, st.src[n - 1]);
                }
                n -= 1;
            }
            if !d.is_empty() {
                st.end.push(Tag::new(TagType::Delim, None, vec![s(&d), s(&d)]));
            }
            st.i = i;
            st.n = n;
            true
        }),
        once: true,
        not_first: false,
    }
}

/// A file extension at the end.
fn ext_lexer(init: &Init) -> Lexer {
    let re = compile_bytes(&format!(r"(?i:\.{})", taginfo_re("$", kind(init.infos, "ext"))));
    let extf = finder(init.infos, "ext");
    Lexer {
        lex: Box::new(move |st| {
            if let Some(c) = re.captures(&st.src[st.i..st.n]) {
                let m = groups(&c);
                st.end.push(tag(TagType::Ext, extf.clone(), &m));
                st.n -= m[0].len();
            }
            true
        }),
        once: true,
        not_first: false,
    }
}

/// One kind of tag from rls's list, matched as a whole word.
fn regexp_lexer(init: &Init, typ: TagType, ignore_case: bool) -> Lexer {
    let info = kind(init.infos, typ.info_key());
    let re = compile_bytes(&taginfo_re(if ignore_case { "^ib" } else { "^b" }, info));
    let f = finder(init.infos, typ.info_key());
    Lexer {
        lex: Box::new(move |st| {
            let Some(c) = re.captures(&st.buf[st.i..st.n]) else { return false };
            let m = groups(&c);
            let l = m[0].len();
            let mut v: Vec<&[u8]> = vec![&st.src[st.i..st.i + l]];
            v.extend(&m[1..]);
            st.start.push(tag(typ, f.clone(), &v));
            st.i += l;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// One kind of tag from rls's list, matched on the name as it is, followed by a word end or a delimiter.
fn regexp_source_lexer(init: &Init, typ: TagType, ignore_case: bool) -> Lexer {
    let info = kind(init.infos, typ.info_key());
    let re = compile_bytes(&(taginfo_re(if ignore_case { "^i" } else { "^" }, info) + r"(?:\b|[\-\._ ])"));
    let f = finder(init.infos, typ.info_key());
    Lexer {
        lex: Box::new(move |st| {
            let Some(c) = re.captures(&st.src[st.i..st.n]) else { return false };
            let m = groups(&c);
            let (l0, l1) = (m[0].len(), m[1].len());
            if l0 != l1 {
                let v = &st.src[st.i..st.i + l1];
                let delim = &st.src[st.i + l1..st.i + l0];
                st.start.push(tag(typ, f.clone(), &[v, v]));
                st.start.push(tag(TagType::Delim, None, &[delim, delim]));
            } else {
                let mut v: Vec<&[u8]> = vec![&st.src[st.i..st.i + l0]];
                v.extend(&m[1..]);
                st.start.push(tag(typ, f.clone(), &v));
            }
            st.i += l0;
            true
        }),
        once: false,
        not_first: false,
    }
}

/// What [`NamedCapture`] found: the matched text, the named values (name, value), and the new `i` and `n`.
type Found = (String, Vec<(String, String)>, usize, usize);

/// Patterns with named groups, tried in order (rls's `NamedCaptureLexer`). Go allows group names like `2006`;
/// Rust doesn't, so the groups are renamed and their names kept here.
struct NamedCapture {
    /// Each pattern, with the names of its named groups by group number.
    regexps: Vec<(Regex, Vec<(usize, String)>)>,
}

impl NamedCapture {
    /// The patterns `strs` (those without named groups are left out).
    fn new(strs: &[&str]) -> NamedCapture {
        let name = regex::Regex::new(r"\(\?P<([^>]+)>").expect("the group name pattern");
        let mut regexps = Vec::new();
        for p in strs {
            let mut names = Vec::new();
            let renamed = name.replace_all(p, |c: &regex::Captures| {
                names.push(c[1].to_string());
                format!("(?P<g{}>", names.len() - 1)
            });
            let re = compile_bytes(&renamed);
            let mut idx = Vec::new();
            for (j, n) in re.capture_names().enumerate() {
                if let Some(n) = n {
                    let k: usize = n[1..].parse().unwrap_or(0);
                    idx.push((j, names[k].clone()));
                }
            }
            if !idx.is_empty() {
                regexps.push((re, idx));
            }
        }
        NamedCapture { regexps }
    }

    /// The first pattern that matches `buf[i..n]` with a named group: the matched text of `src`, the named values
    /// (name, value) that aren't empty, and the new `i` and `n`.
    fn find(&self, st: &State) -> Option<Found> {
        for (re, idx) in &self.regexps {
            if let Some(c) = re.captures(&st.buf[st.i..st.n]) {
                let v: Vec<(String, String)> = idx
                    .iter()
                    .filter_map(|(k, name)| c.get(*k).filter(|m| !m.is_empty()).map(|m| (name.clone(), s(m.as_bytes()))))
                    .collect();
                if !v.is_empty() {
                    let l = c[0].len();
                    return Some((s(&st.src[st.i..st.i + l]), v, st.i + l, st.n));
                }
            }
        }
        None
    }
}
