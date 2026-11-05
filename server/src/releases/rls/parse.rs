// Ported from autobrr/rls v0.10.0 (parse.go), MIT (originally moistari/rls; see LICENSE next to this file).
//! The parser (runs the lexers over a name) and the builder (turns the tags into a [`Release`]: fixes badly
//! matched tags, guesses the type, finds the titles).

use super::goregex::{compile, compile_bytes};
use super::lex::{self, Init, LexFn, LexerFactory, State};
use super::taginfo::{Finder, Infos};
use super::{is_any_delim, is_break_delim, is_title_trim_delim, must_normalize, Release, Tag, TagType, Type};
use regex::Regex;
use std::collections::HashSet;

/// Reads release names into tags and releases.
pub struct TagParser {
    /// Makes releases from tags.
    builder: TagBuilder,
    /// Matches delimiters at the start.
    delim: regex::bytes::Regex,
    /// `_`, `,` and `+`, which the working copy of a name has as spaces.
    work: regex::bytes::Regex,
    /// The lexers that run once.
    once: Vec<LexFn>,
    /// The lexers that run on every piece.
    multi: Vec<LexFn>,
    /// Per multi lexer: whether it waits until something matched first.
    not_first: Vec<bool>,
}

impl TagParser {
    /// A parser with tag infos `infos` and `lexers`.
    pub fn new(infos: Infos, lexers: Vec<LexerFactory>) -> TagParser {
        // delimiters
        let v: Vec<String> = (0u8..255).map(char::from).filter(|&c| is_any_delim(c)).map(|c| c.to_string()).collect();
        let delim = compile_bytes(&format!("^((?:{})+)", lex::join(true, &v)));
        // short tags
        let mut short = HashSet::new();
        for (typ, list) in &infos {
            if typ == "hdr" {
                continue;
            }
            for info in list {
                for field in info.tag().split(is_any_delim).filter(|f| !f.is_empty()) {
                    if field.len() < 5 && !field.contains('$') {
                        short.insert(field.to_uppercase());
                    }
                }
            }
        }
        let init = Init { infos: &infos, delim: &delim, short: &short };
        let (mut once, mut multi, mut not_first) = (Vec::new(), Vec::new(), Vec::new());
        for f in lexers {
            let l = f(&init);
            if l.once {
                once.push(l.lex);
            } else {
                multi.push(l.lex);
                not_first.push(l.not_first);
            }
        }
        TagParser { builder: TagBuilder::new(&infos), delim, work: compile_bytes(r"[_,\+]"), once, multi, not_first }
    }

    /// The tags of `src`, and where the end tags start.
    pub fn parse(&self, src: &[u8]) -> (Vec<Tag>, usize) {
        let buf = self.work.replace_all(src, &b" "[..]).into_owned();
        let mut st = State { src, buf: &buf, start: Vec::new(), end: Vec::new(), i: 0, n: buf.len() };
        for f in &self.once {
            f(&mut st);
        }
        let mut not_first = false;
        while st.i < st.n {
            self.next(&mut st, &mut not_first);
        }
        let mut tags = st.start;
        let end_len = st.end.len();
        tags.extend(st.end.into_iter().rev());
        let n = tags.len() - end_len;
        (tags, n)
    }

    /// Reads the next piece: a delimiter, what a lexer knows, or text up to the next delimiter.
    fn next(&self, st: &mut State, not_first: &mut bool) {
        let (src, i, n) = (st.src, st.i, st.n);
        if src[i..n].starts_with(b"...") {
            st.start.push(Tag::new(TagType::Delim, None, vec!["...".into(), "...".into()]));
            st.i += 3;
            return;
        }
        if let Some(c) = self.delim.captures(&src[i..n]) {
            let (m0, m1) = (String::from_utf8_lossy(&c[0]).into_owned(), String::from_utf8_lossy(&c[1]).into_owned());
            st.i += c[0].len();
            st.start.push(Tag::new(TagType::Delim, None, vec![m0, m1]));
            return;
        }
        let startn = st.start.len();
        for (a, f) in self.multi.iter().enumerate() {
            if self.not_first[a] && !*not_first {
                continue;
            }
            if f(st) {
                *not_first = *not_first || startn != st.start.len();
                return;
            }
        }
        // text
        let mut j = i;
        while j < n && !self.delim.is_match(&src[j..]) {
            j += 1;
        }
        *not_first = true;
        let t = String::from_utf8_lossy(&src[i..j]).into_owned();
        st.start.push(Tag::new(TagType::Text, None, vec![t.clone(), t]));
        st.i = j;
    }

    /// The release `src` names.
    pub fn parse_release(&self, src: &[u8]) -> Release {
        let (tags, end) = self.parse(src);
        self.builder.build(tags, end)
    }
}

/// Turns tags into a release.
pub struct TagBuilder {
    /// Acronyms without periods.
    missing: Regex,
    /// Bad acronyms with periods.
    bad: Regex,
    /// Fixes acronyms.
    fix: Regex,
    /// Several spaces.
    spaces: Regex,
    /// Three or more periods.
    ellips: Regex,
    /// Pluses.
    plus: Regex,
    /// A file checksum.
    sum: Regex,
    /// Only digits.
    digits: Regex,
    /// Digits at the start.
    digpre: Regex,
    /// Digits at the end.
    digsuf: Regex,
    /// Finds containers.
    containerf: Option<Finder>,
    /// Finds audio formats.
    audiof: Option<Finder>,
}

/// The tag kinds whose infos say which release type they mean.
const EXCL_TYPES: [TagType; 20] = [
    TagType::Platform,
    TagType::Arch,
    TagType::Source,
    TagType::Resolution,
    TagType::Collection,
    TagType::Codec,
    TagType::Hdr,
    TagType::BitDepth,
    TagType::Audio,
    TagType::Channels,
    TagType::Other,
    TagType::Cut,
    TagType::Edition,
    TagType::Language,
    TagType::Size,
    TagType::Region,
    TagType::Container,
    TagType::Genre,
    TagType::Group,
    TagType::Ext,
];

/// Whether tag `i` (which may be out of range) is one of `types`.
fn peek(tags: &[Tag], i: isize, types: &[TagType]) -> bool {
    0 <= i && (i as usize) < tags.len() && tags[i as usize].is(types)
}

/// Whether the tag found going `inc` (-1 or +1) from `i`, past whitespace and delimiters, is text.
fn isolated(tags: &[Tag], i: usize, inc: isize) -> bool {
    let len = tags.len() as isize;
    let mut i = i as isize + inc;
    while 0 < i && i < len - 1 && tags[(i + inc) as usize].is(&[TagType::Whitespace, TagType::Delim]) {
        i += inc;
    }
    i += inc;
    0 <= i && i < len - 1 && tags[i as usize].is(&[TagType::Text])
}

/// Whether `v` has `s`.
fn contains(v: &[String], s: &str) -> bool {
    v.iter().any(|x| x == s)
}

/// `v` without `s`.
fn remove(v: &[String], s: &str) -> Vec<String> {
    v.iter().filter(|x| *x != s).cloned().collect()
}

/// Go's `strings.TrimRightFunc`.
fn trim_right(s: &str, f: fn(char) -> bool) -> String {
    s.trim_end_matches(f).to_string()
}

/// Go's `strings.TrimLeftFunc`.
fn trim_left(s: &str, f: fn(char) -> bool) -> String {
    s.trim_start_matches(f).to_string()
}

impl TagBuilder {
    /// A builder for tag infos `infos`.
    fn new(infos: &Infos) -> TagBuilder {
        TagBuilder {
            missing: compile(r"\b[A-Z][\. ][A-Z](?:[\. ][A-Z])*[\. ]?\b"),
            bad: compile(r"[^A-Z][-\. ][A-Z]\.($|[^A-Z])"),
            fix: compile(r"([A-Z])\."),
            spaces: compile(r"\s+"),
            ellips: compile(r"\.{3,}"),
            plus: compile(r"(\+)"),
            sum: compile(r"(?i)^[a-f0-9]{8}$"),
            digits: compile(r"^\d+$"),
            digpre: compile(r"^\d+"),
            digsuf: compile(r"\d+$"),
            containerf: Some(Finder::new(infos.get("container"))),
            audiof: Some(Finder::new(infos.get("audio"))),
        }
    }

    /// The release the tags make.
    pub fn build(&self, tags: Vec<Tag>, end: usize) -> Release {
        let mut r = Release { tags, end, ..Release::default() };
        self.init(&mut r);
        self.collect(&mut r);
        r.r#type = self.inspect(&mut r, true);
        self.special_date(&mut r);
        self.unset(&mut r);
        let i = self.titles(&mut r);
        self.unused(&mut r, i);
        r
    }

    /// Fixes the first set of tags.
    fn init(&self, r: &mut Release) {
        self.fix_first_date(r);
        let (m, pivot) = self.pivots(r, &[TagType::Date, TagType::Source, TagType::Series, TagType::Resolution, TagType::Version]);
        let (date, series) = (m[0], m[2]);
        if let Some(d) = date {
            r.dates.push(d);
        }
        let dates = self.reset(r, date.unwrap_or(0), &[TagType::Date]);
        r.dates.extend(dates);
        if date.is_some() || series.is_some() {
            let i = match (date, series) {
                (None, Some(s)) => s,
                (Some(d), None) => d,
                (Some(d), Some(s)) => d.min(s),
                (None, None) => 0,
            };
            self.fix_special(r, i, series.is_some());
        }
        let end = self.end(r, pivot);
        self.reset(r, end, &[TagType::Language, TagType::Arch, TagType::Platform]);
        self.fix_first(r);
        self.fix_source_web(r);
        let start = self.start(r, 0);
        self.fix_bad(r, start, end);
        self.fix_no_text(r, end);
        self.fix_isolated(r);
        self.fix_music(r);
    }

    /// A date before any text, with no other date, is text.
    fn fix_first_date(&self, r: &mut Release) {
        let mut last = None;
        for i in (1..=r.end).rev() {
            if r.tags[i - 1].is(&[TagType::Date]) {
                last = Some(i - 1);
                break;
            }
        }
        let Some(last) = last else { return };
        let mut i = 0;
        while i < r.end && r.tags[i].is(&[TagType::Whitespace, TagType::Delim]) {
            i += 1;
        }
        if i < last || (peek(&r.tags, i as isize - 1, &[TagType::Delim]) && r.tags[i - 1].delim().ends_with('(')) {
            return;
        }
        if r.tags[i].is(&[TagType::Date]) {
            r.tags[i] = r.tags[i].as_type(TagType::Text, None);
        }
    }

    /// The last position of each of `types` (searching from the end, the earliest found), and the earliest of all
    /// (the end when none).
    fn pivots(&self, r: &Release, types: &[TagType]) -> (Vec<Option<usize>>, usize) {
        let mut m: Vec<Option<usize>> = vec![None; types.len()];
        let mut j = None;
        for i in (1..=r.end).rev() {
            let typ = r.tags[i - 1].tag_type();
            if let Some(k) = types.iter().position(|t| *t == typ) {
                if m[k].is_none() {
                    m[k] = Some(i - 1);
                    j = Some(i - 1);
                }
            }
        }
        (m, j.unwrap_or(r.end))
    }

    /// Turns tags of `types` before `i` into text; their positions.
    fn reset(&self, r: &mut Release, i: usize, types: &[TagType]) -> Vec<usize> {
        let mut v = Vec::new();
        for k in (1..=i).rev() {
            if r.tags[k - 1].is(types) {
                r.tags[k - 1] = r.tags[k - 1].as_type(TagType::Text, None);
                v.push(k - 1);
            }
        }
        v
    }

    /// The first text tag from `i`.
    fn start(&self, r: &Release, mut i: usize) -> usize {
        while i < r.end && !r.tags[i].is(&[TagType::Text]) {
            i += 1;
        }
        i
    }

    /// The position after the last text tag before `i`.
    fn end(&self, r: &Release, mut i: usize) -> usize {
        while i > 0 && !r.tags[i - 1].is(&[TagType::Text]) {
            i -= 1;
        }
        i
    }

    /// A badly matched first tag is text.
    fn fix_first(&self, r: &mut Release) {
        let mut i = 0;
        while i < r.end && r.tags[i].is(&[TagType::Whitespace, TagType::Delim]) {
            i += 1;
        }
        if i != r.end
            && r.tags[i].is(&[
                TagType::Platform,
                TagType::Arch,
                TagType::Source,
                TagType::Resolution,
                TagType::Codec,
                TagType::Hdr,
                TagType::BitDepth,
                TagType::Audio,
                TagType::Other,
                TagType::Cut,
                TagType::Edition,
                TagType::Language,
                TagType::Region,
            ])
        {
            r.tags[i] = r.tags[i].as_type(TagType::Text, None);
        }
    }

    /// Collection, language, other, arch and platform tags in the title are text.
    fn fix_bad(&self, r: &mut Release, start: usize, mut i: usize) {
        while i > start
            && r.tags[i - 1].is(&[
                TagType::Language,
                TagType::Edition,
                TagType::Cut,
                TagType::Other,
                TagType::Collection,
                TagType::Delim,
                TagType::Source,
            ])
        {
            i -= 1;
        }
        while i > start {
            let t = &r.tags[i - 1];
            let keep = (t.is(&[TagType::Collection]) && t.value() == "IMAX") || (t.is(&[TagType::Other]) && t.other() == "REMiX");
            if !keep && t.is(&[TagType::Collection, TagType::Language, TagType::Other, TagType::Arch, TagType::Platform]) {
                r.tags[i - 1] = t.as_type(TagType::Text, None);
            }
            i -= 1;
        }
    }

    /// Some collection, source, cut and other tags before a date or season are text.
    fn fix_special(&self, r: &mut Release, mut i: usize, series: bool) {
        while i > 0 {
            let t = &r.tags[i - 1];
            let (typ, c, o, txt) = (t.tag_type(), t.value(), t.other(), t.text());
            let l = txt.to_lowercase();
            if (typ == TagType::Collection && (c == "CC" || c == "RED" || (c == "AMZN" && l == "amazon")))
                || (typ == TagType::Source && txt == "Web")
                || (typ == TagType::Cut && txt == "Uncut")
                || (typ == TagType::Other && (o == "MD" || o == "RESTORATiON"))
                || (typ == TagType::Cut && l == "dc")
                || (series && (typ == TagType::Arch || typ == TagType::Platform))
            {
                r.tags[i - 1] = t.as_type(TagType::Text, None);
            }
            i -= 1;
        }
    }

    /// A lone `WEB` before another source is part of the title.
    fn fix_source_web(&self, r: &mut Release) {
        for i in 0..r.end {
            if !r.tags[i].is(&[TagType::Source]) || r.tags[i].value() != "WEB" || !isolated(&r.tags[..r.end], i, -1) {
                continue;
            }
            if (i + 1..r.end).any(|j| r.tags[j].is(&[TagType::Source])) {
                r.tags[i] = r.tags[i].as_type(TagType::Text, None);
            }
        }
    }

    /// With no text at all, collection tags are text.
    fn fix_no_text(&self, r: &mut Release, end: usize) {
        let n = (end + 1).min(r.tags.len());
        if r.tags[..n].iter().any(|t| t.is(&[TagType::Text])) {
            return;
        }
        for i in 0..n {
            if r.tags[i].is(&[TagType::Collection]) {
                r.tags[i] = r.tags[i].as_type(TagType::Text, None);
            }
        }
    }

    /// Collection, language, other, arch and platform tags with text on both sides are text.
    fn fix_isolated(&self, r: &mut Release) {
        let mut i = r.end as isize - 1;
        while i > 1 {
            let k = (i - 1) as usize;
            let t = &r.tags[k];
            if t.is(&[TagType::Collection, TagType::Language, TagType::Other, TagType::Arch, TagType::Platform])
                && isolated(&r.tags[..r.end], k, -1)
                && isolated(&r.tags[..r.end], k, 1)
                && t.other() != "REMiX"
            {
                r.tags[k] = t.as_type(TagType::Text, None);
            }
            i -= 1;
        }
    }

    /// Music fixes: one `cbr` audio tag is the comic container, `16bit` is audio in music, `bootleg` not
    /// wrapped in `-` or `()` is text.
    fn fix_music(&self, r: &mut Release) {
        let (mut count_cbr, mut pos_cbr, mut has_cbr, mut music_excl) = (0, 0, false, false);
        for i in 0..r.end {
            if r.tags[i].is(&[TagType::Audio]) {
                if r.tags[i].value() == "CBR" {
                    has_cbr = true;
                    pos_cbr = i;
                }
                music_excl = music_excl || r.tags[i].info_excl();
                count_cbr += 1;
            }
            if i != 0 && r.tags[i].is(&[TagType::Other]) && r.tags[i].other() == "BOOTLEG" {
                let ii = i as isize;
                let d = |k: isize| peek(&r.tags, k, &[TagType::Delim]);
                let wrapped = (d(ii - 1) && r.tags[i - 1].delim().ends_with('-') && d(ii + 1) && r.tags[i + 1].delim().starts_with('-'))
                    || (d(ii - 1) && r.tags[i - 1].delim().ends_with('(') && d(ii + 1) && r.tags[i + 1].delim().starts_with(')'));
                if !wrapped {
                    r.tags[i] = r.tags[i].as_type(TagType::Text, None);
                }
            }
            let single_ep = r.tags[i].single_ep();
            let sb_bit = r.tags[i].is(&[TagType::Arch]) && r.tags[i].value() == "16bit";
            if single_ep || sb_bit {
                let mut j = i + 1;
                while !music_excl && j < r.end {
                    music_excl = music_excl || (r.tags[j].is(&[TagType::Audio]) && r.tags[j].info_excl());
                    j += 1;
                }
                if music_excl && single_ep {
                    r.tags[i] = r.tags[i].as_type(TagType::Text, None);
                } else if music_excl && sb_bit {
                    r.tags[i] = r.tags[i].as_type(TagType::Audio, self.audiof.clone());
                }
            }
        }
        if has_cbr && count_cbr == 1 {
            r.tags[pos_cbr] = r.tags[pos_cbr].as_type(TagType::Container, self.containerf.clone());
        }
    }

    /// Fills in the release from the tags.
    fn collect(&self, r: &mut Release) {
        for i in 0..r.tags.len() {
            let t = r.tags[i].clone();
            match t.tag_type() {
                TagType::Platform if r.platform.is_empty() => r.platform = t.value(),
                TagType::Arch if r.arch.is_empty() => r.arch = t.value(),
                TagType::Source => {
                    let s = t.value();
                    if r.source.is_empty() || r.source == "CD" || (r.source == "DVD" && s != "CD") {
                        r.source = s;
                    }
                }
                TagType::Resolution if r.resolution.is_empty() => r.resolution = t.value(),
                TagType::Collection if r.collection.is_empty() => r.collection = t.value(),
                TagType::Date => (r.year, r.month, r.day) = t.date(),
                TagType::Series => {
                    let (s, e) = t.series();
                    if r.series == 0 {
                        r.series = s;
                    }
                    if r.episode == 0 {
                        r.episode = e;
                    }
                }
                TagType::Version if r.version.is_empty() => r.version = t.normalize(),
                TagType::Disc if r.disc.is_empty() => r.disc = t.normalize(),
                TagType::Codec => r.codec.push(t.value()),
                TagType::Hdr => r.hdr.push(t.value()),
                TagType::BitDepth if r.bit_depth.is_empty() => r.bit_depth = t.value(),
                TagType::Audio => r.audio.push(t.value()),
                TagType::Channels if r.channels.is_empty() => r.channels = t.channels(),
                TagType::Other => r.other.push(t.other()),
                TagType::Cut => r.cut.push(t.value()),
                TagType::Edition => r.edition.push(t.value()),
                TagType::Language => r.language.push(t.value()),
                TagType::Size if r.size.is_empty() => r.size = t.size(),
                TagType::Region if r.region.is_empty() => r.region = t.value(),
                TagType::Container if r.container.is_empty() => r.container = t.value(),
                TagType::Genre if r.genre.is_empty() => r.genre = t.value(),
                TagType::Id if r.id.is_empty() => r.id = t.value(),
                TagType::Group => r.group = t.normalize(),
                TagType::Meta => {
                    let (k, v) = t.meta();
                    match k.as_str() {
                        "site" if r.site.is_empty() => r.site = v,
                        "sum" if r.sum.is_empty() => r.sum = v,
                        "pass" if r.pass.is_empty() => r.pass = v,
                        "req" => r.req = true,
                        _ => r.meta.push(format!("{k}:{v}")),
                    }
                }
                TagType::Ext => r.ext = t.normalize(),
                _ => {}
            }
        }
        for i in (0..r.dates.len()).rev() {
            let (y, m, d) = r.tags[r.dates[i]].date();
            if r.year == 0 && y != 0 {
                r.year = y;
            }
            if r.month == 0 && m != 0 {
                r.month = m;
            }
            if r.day == 0 && d != 0 {
                r.day = d;
            }
        }
    }

    /// The release's type, as the tags suggest.
    fn inspect(&self, r: &mut Release, initial: bool) -> Type {
        if r.r#type != Type::Unknown {
            return r.r#type;
        }
        let n = r.tags.len();
        let (mut app, mut series, mut movie, mut video) = (false, false, false, false);
        for tag in &r.tags {
            series = series || tag.is(&[TagType::Series]);
            video = video || (!tag.is(&[TagType::Group]) && tag.info_type() == Type::Movie);
        }
        for i in (1..=n).rev() {
            let typ = r.tags[i - 1].info_type();
            app = app || typ == Type::App;
            movie = movie || typ == Type::Movie;
            match typ {
                Type::Book | Type::Game => {
                    if typ == Type::Game
                        && r.tags[i - 1].is(&[TagType::Group])
                        && video
                        && (series || !r.resolution.is_empty() || (r.year != 0 && r.month != 0 && r.day != 0))
                    {
                        continue;
                    }
                    for j in (1..i).rev() {
                        let t = r.tags[j - 1].info_type();
                        if t.is(&[Type::Comic, Type::Education, Type::Magazine]) {
                            return t;
                        }
                    }
                    return typ;
                }
                Type::Series | Type::Episode => {
                    if r.episode != 0 || (r.series == 0 && r.episode == 0) && !contains(&r.other, "BOXSET") {
                        return Type::Episode;
                    }
                    return Type::Series;
                }
                Type::Education => {
                    if r.series == 0 && r.episode == 0 {
                        return Type::Education;
                    }
                }
                Type::Music => {
                    for j in (1..i).rev() {
                        let t = r.tags[j - 1].info_type();
                        if t.is(&[Type::Audiobook]) {
                            return t;
                        }
                    }
                    return typ;
                }
                Type::Audiobook | Type::Comic | Type::Magazine => return typ,
                _ => {}
            }
            if r.tags[i - 1].info_excl() && r.version.is_empty() && r.series == 0 && r.episode == 0 && r.day == 0 && r.month == 0 {
                return typ;
            }
        }
        // music style tag delimiters
        let mut count = 0;
        let mut i = n as isize - 1;
        while i > 1 {
            let k = (i - 1) as usize;
            if r.tags[k].is(&[
                TagType::Date,
                TagType::Codec,
                TagType::Hdr,
                TagType::BitDepth,
                TagType::Audio,
                TagType::Resolution,
                TagType::Source,
                TagType::Language,
            ]) && peek(&r.tags, i - 2, &[TagType::Delim])
                && r.tags[k - 1].delim().ends_with('-')
                && peek(&r.tags, i, &[TagType::Delim])
                && r.tags[k + 1].delim().starts_with('-')
            {
                count += 1;
                if count > 1 {
                    return Type::Music;
                }
            }
            i -= 1;
        }
        if r.episode != 0 || (r.year != 0 && r.month != 0 && r.day != 0) {
            return Type::Episode;
        }
        if r.series != 0 || series {
            return Type::Series;
        }
        if app || (!r.version.is_empty() && r.resolution.is_empty()) {
            return Type::App;
        }
        if movie || !r.resolution.is_empty() {
            return Type::Movie;
        }
        if (r.source.is_empty() || r.source == "WEB") && r.resolution.is_empty() && r.year != 0 {
            return Type::Music;
        }
        if initial {
            let mut reinspect = false;
            for i in (1..=n).rev() {
                if r.tags[i - 1].was(&[TagType::Platform, TagType::Arch]) {
                    r.tags[i - 1] = r.tags[i - 1].revert();
                    reinspect = true;
                    match r.tags[i - 1].tag_type() {
                        TagType::Platform => r.platform = r.tags[i - 1].value(),
                        TagType::Arch => r.arch = r.tags[i - 1].value(),
                        _ => {}
                    }
                }
            }
            if reinspect {
                return self.inspect(r, false);
            }
        }
        Type::Unknown
    }

    /// On magazines, a month name just before the year.
    fn special_date(&self, r: &mut Release) {
        if r.r#type == Type::Magazine && r.year != 0 && r.month == 0 && r.day == 0 {
            let Some(&first) = r.dates.first() else { return };
            let mut i = first as isize - 1;
            while i > 0 && r.tags[i as usize].is(&[TagType::Delim]) {
                i -= 1;
            }
            if i >= 0 && r.tags[i as usize].is(&[TagType::Text]) {
                let s = r.tags[i as usize].text();
                if let Some(m) = lex::month_name(&s) {
                    r.month = m;
                    r.tags[i as usize] = Tag::new(TagType::Date, None, vec![s, r.year.to_string(), m.to_string(), String::new()]);
                    r.dates.push(i as usize);
                }
            }
        }
    }

    /// Unsets tags that belong to another type of release.
    fn unset(&self, r: &mut Release) {
        let msemg = r.r#type.is(&[Type::Movie, Type::Series, Type::Episode, Type::Music, Type::Game]);
        let mut grab_source = false;
        for i in 0..r.tags.len() {
            if grab_source && r.tags[i].is(&[TagType::Source]) && r.source.is_empty() {
                r.source = r.tags[i].value();
            }
            let ityp = r.tags[i].info_type();
            if ityp != r.r#type && r.tags[i].is(&EXCL_TYPES) && r.tags[i].info_excl() {
                let (typ, s) = (r.tags[i].tag_type(), r.tags[i].normalize());
                let text = r.tags[i].as_type(TagType::Text, None);
                let unset = match typ {
                    TagType::Platform if r.platform == s && !contains(&r.other, "Strategy.Guide") => {
                        r.platform.clear();
                        true
                    }
                    TagType::Arch if r.arch == s => {
                        r.arch.clear();
                        true
                    }
                    TagType::Source if r.source == s => {
                        r.source.clear();
                        true
                    }
                    TagType::Resolution if r.resolution == s => {
                        r.resolution.clear();
                        true
                    }
                    TagType::Collection if r.collection == s => {
                        r.collection.clear();
                        true
                    }
                    TagType::Codec if contains(&r.codec, &s) => {
                        r.codec = remove(&r.codec, &s);
                        true
                    }
                    TagType::Hdr if contains(&r.hdr, &s) => {
                        r.hdr = remove(&r.hdr, &s);
                        true
                    }
                    TagType::BitDepth if r.bit_depth == s => {
                        r.bit_depth.clear();
                        true
                    }
                    TagType::Audio if contains(&r.audio, &s) => {
                        r.audio = remove(&r.audio, &s);
                        true
                    }
                    TagType::Channels if r.channels == s => {
                        r.channels.clear();
                        true
                    }
                    TagType::Other if contains(&r.other, &s) => {
                        r.other = remove(&r.other, &s);
                        true
                    }
                    TagType::Cut if contains(&r.cut, &s) => {
                        r.cut = remove(&r.cut, &s);
                        true
                    }
                    TagType::Edition if contains(&r.edition, &s) => {
                        r.edition = remove(&r.edition, &s);
                        true
                    }
                    TagType::Language if contains(&r.language, &s) => {
                        r.language = remove(&r.language, &s);
                        true
                    }
                    TagType::Size if r.size == s => {
                        r.size.clear();
                        true
                    }
                    TagType::Region if r.region == s => {
                        r.region.clear();
                        true
                    }
                    TagType::Container if r.container == s => {
                        r.container.clear();
                        true
                    }
                    TagType::Genre if r.genre == s => {
                        r.genre.clear();
                        true
                    }
                    TagType::Group if r.group == s => {
                        r.group.clear();
                        true
                    }
                    TagType::Ext if r.ext == s => {
                        r.ext.clear();
                        true
                    }
                    _ => false,
                };
                if unset {
                    r.tags[i] = text;
                }
            } else if !msemg && r.tags[i].is(&[TagType::Source]) && ityp.is(&[Type::Movie, Type::Series, Type::Episode]) {
                if r.source == r.tags[i].normalize() {
                    r.source.clear();
                }
                r.tags[i] = r.tags[i].as_type(TagType::Text, None);
                grab_source = true;
            } else if !msemg && r.tags[i].is(&[TagType::Channels]) {
                r.tags[i] = r.tags[i].as_type(TagType::Text, None);
                r.channels.clear();
            }
        }
    }

    /// Sets the titles; where the titles end.
    fn titles(&self, r: &mut Release) -> usize {
        let (i, aka) = match r.r#type {
            Type::Movie => (self.movie_titles(r), true),
            Type::Series | Type::Episode => (self.episode_titles(r), true),
            Type::Music => (self.music_titles(r), false),
            Type::Book | Type::Audiobook => (self.book_titles(r), false),
            Type::App | Type::Game => (self.app_title(r), false),
            _ => (self.default_title(r), false),
        };
        if aka && r.alt.is_empty() {
            let s: Vec<&str> = r.title.split(" AKA ").collect();
            if s.len() == 2 && !s[0].is_empty() && !s[1].is_empty() {
                let (t, a) = (s[0].to_string(), s[1].to_string());
                r.title = t;
                r.alt = a;
            }
        }
        i
    }

    /// Titles for movies (and the start of series).
    fn movie_titles(&self, r: &mut Release) -> usize {
        let mut pos = 0;
        while pos < r.tags.len() && !r.tags[pos].is(&[TagType::Text]) {
            pos += 1;
        }
        let start = pos;
        let (title, offset) = self.title(&r.tags[start..], &[TagType::Text]);
        r.title = title;
        let Some(date) = r.tags.iter().position(|t| t.is(&[TagType::Date])) else { return self.box_title(r, start, offset) };
        let Some(resolution) = r.tags.iter().position(|t| t.is(&[TagType::Resolution])) else {
            return self.box_title(r, start, offset);
        };
        let mut has_subtitle = (date as isize + 1) < (resolution as isize - 1);
        if !has_subtitle {
            return self.box_title(r, start, offset);
        }
        let mut pos = date + 1;
        while pos < r.tags.len() && has_subtitle && pos < resolution {
            has_subtitle = has_subtitle && r.tags[pos].is(&[TagType::Delim, TagType::Text, TagType::Cut, TagType::Edition]);
            pos += 1;
        }
        if has_subtitle {
            let mut pos = date + 1;
            while pos < r.tags.len() && !r.tags[pos].is(&[TagType::Text, TagType::Cut, TagType::Edition]) {
                pos += 1;
            }
            if (pos as isize) < resolution as isize - 1 {
                r.subtitle = self.title(&r.tags[pos..resolution - 1], &[TagType::Text, TagType::Cut, TagType::Edition]).0;
            }
        }
        if let Some(i) = r.title.rfind('~') {
            if r.subtitle.is_empty() {
                let (t, s) = (trim_right(&r.title[..i], is_title_trim_delim), trim_left(&r.title[i + 1..], is_title_trim_delim));
                r.title = t;
                r.subtitle = s;
            }
        }
        (start + offset).min(resolution)
    }

    /// Titles for boxes (`The … Collection` before a cut or edition on a disc release).
    fn box_title(&self, r: &mut Release, start: usize, offset: usize) -> usize {
        let n = start + offset;
        if n == r.tags.len() || n == 0 || r.disc.is_empty() || !r.tags[n].is(&[TagType::Cut, TagType::Edition]) {
            return n;
        }
        let mut pos = n - 1;
        while pos > start + 1 && pos as isize > n as isize - 8 && pos > 0 {
            if must_normalize(&r.tags[pos - 1].text()) == "the" {
                let (prefix, _) = self.title(&r.tags[pos - 1..n], &[TagType::Text]);
                let (suffix, offset) = self.title(&r.tags[n..], &[TagType::Text, TagType::Cut, TagType::Edition]);
                let t = r.title.strip_suffix(prefix.as_str()).unwrap_or(&r.title).to_string();
                r.title = trim_right(&t, is_title_trim_delim);
                r.subtitle = format!("{prefix} {}", trim_right(&suffix, is_break_delim));
                return n + offset;
            }
            pos -= 1;
        }
        n
    }

    /// Titles for episodes and series.
    fn episode_titles(&self, r: &mut Release) -> usize {
        let mut pos = self.movie_titles(r);
        let typ = if r.month != 0 && r.day != 0 { TagType::Date } else { TagType::Series };
        while pos < r.tags.len() && !r.tags[pos].is(&[typ]) {
            if r.tags[pos].is(&[TagType::Text]) {
                r.unused.push(pos);
            }
            pos += 1;
        }
        if pos == r.tags.len() {
            return pos;
        }
        pos += 1;
        while pos < r.tags.len()
            && r.tags[pos].is(&[
                TagType::Delim,
                TagType::Source,
                TagType::Resolution,
                TagType::Collection,
                TagType::Date,
                TagType::Series,
                TagType::Version,
                TagType::Disc,
                TagType::Other,
                TagType::Cut,
                TagType::Edition,
                TagType::Language,
                TagType::Container,
            ])
        {
            pos += 1;
        }
        if pos == r.tags.len() || !r.tags[pos].is(&[TagType::Text]) {
            return pos;
        }
        let (sub, offset) = self.title(&r.tags[pos..], &[TagType::Text]);
        r.subtitle = sub;
        pos + offset
    }

    /// Titles for music: artist, title and subtitle.
    fn music_titles(&self, r: &mut Release) -> usize {
        let (title, mut i) = self.mix_title(r, 0);
        r.title = title;
        for s in [" - ", "--", "~", "-"] {
            if let Some(j) = r.title.rfind(s) {
                let (a, t) = (trim_right(&r.title[..j], is_title_trim_delim), trim_left(&r.title[j + s.len()..], is_break_delim));
                r.artist = a;
                r.title = t;
                break;
            }
        }
        let (ni, mut skipped, ok) = self.check_date(r, i);
        i = ni;
        if ok {
            let s = r.tags[i].delim().to_string();
            // Artist - (Prefix) Title
            if r.artist.is_empty() && s.ends_with('(') {
                let (title, z) = self.mix_title(r, i + 1);
                let (subtitle, z) = self.mix_title(r, z + 1);
                if !title.is_empty() && !subtitle.is_empty() {
                    r.artist = std::mem::take(&mut r.title);
                    r.title = format!("({title}) {subtitle}");
                    let (ni, sk, ok) = self.check_date(r, z);
                    i = ni;
                    skipped = sk;
                    if !ok {
                        return i;
                    }
                }
            }
            // (Artist) - Title
            if r.artist.is_empty() && (skipped || s.starts_with(')')) {
                let (title, z) = self.mix_title(r, i + 1);
                if !title.is_empty() {
                    r.artist = std::mem::take(&mut r.title);
                    r.title = title;
                    i = z;
                }
            }
            // a subtitle after '(', '__', '-' or '~'
            if r.subtitle.is_empty()
                && (s.ends_with('(') || s == "__" || s.contains(['-', '~']))
                && peek(&r.tags[..r.end], i as isize + 1, &[TagType::Text])
            {
                let (sub, z) = self.mix_title(r, i + 1);
                r.subtitle = sub;
                i = z;
            }
        }
        if r.subtitle.is_empty() && !r.artist.is_empty() {
            for s in [" - ", "--", "~"] {
                if let Some(j) = r.artist.rfind(s) {
                    let artist = trim_right(&r.artist[..j], is_title_trim_delim);
                    let title = trim_left(&r.artist[j + s.len()..], is_break_delim);
                    r.subtitle = std::mem::take(&mut r.title);
                    r.artist = artist;
                    r.title = title;
                    break;
                }
            }
        }
        i
    }

    /// The mix title from `i`, and where it ends.
    fn mix_title(&self, r: &Release, i: usize) -> (String, usize) {
        let start = self.start(r, i).min(r.tags.len());
        let mut i = start;
        while i < r.end && r.tags[i].is(&[TagType::Delim, TagType::Text, TagType::Other]) {
            if r.tags[i].is(&[TagType::Other]) && r.tags[i].other() != "REMiX" {
                break;
            }
            i += 1;
        }
        let i = i.max(start);
        let (title, offset) = self.title(&r.tags[start..i], &[TagType::Text, TagType::Other]);
        (title, start + offset)
    }

    /// Whether `i` is not at the end, skipping a date (`Artist (2003) - Title`), and is a delimiter.
    fn check_date(&self, r: &Release, mut i: usize) -> (usize, bool, bool) {
        if i == r.end {
            return (i, false, false);
        }
        let mut skipped = false;
        if r.tags.get(i).is_some_and(|t| t.is(&[TagType::Date])) {
            i += 1;
            skipped = true;
        }
        if i == r.end || !r.tags.get(i).is_some_and(|t| t.is(&[TagType::Delim])) {
            return (i, skipped, false);
        }
        (i, skipped, true)
    }

    /// Titles for books.
    fn book_titles(&self, r: &mut Release) -> usize {
        let mut pos = 0;
        while pos < r.tags.len() {
            while pos < r.tags.len() && !r.tags[pos].is(&[TagType::Text, TagType::Platform, TagType::Arch, TagType::Other, TagType::Region])
            {
                pos += 1;
            }
            if pos == r.tags.len() {
                break;
            }
            let is_other = r.tags[pos].is(&[TagType::Other]);
            let (s, offset) = if is_other && r.tags[pos].info_type() != Type::Book {
                pos += 1;
                continue;
            } else if is_other && r.tags[pos].other() == "Strategy.Guide" {
                (r.tags[pos].text().replace('.', " "), 2)
            } else {
                self.title(&r.tags[pos..], &[TagType::Text, TagType::Platform, TagType::Arch, TagType::Region])
            };
            if !r.title.is_empty() && !s.is_empty() {
                r.title.push(' ');
            }
            r.title.push_str(&s);
            // rls would loop forever on an empty title here; Lantern moves on
            pos += offset.max(1);
        }
        if let Some(i) = r.title.rfind(';') {
            let (t, s) = (trim_right(&r.title[..i], is_title_trim_delim), trim_left(&r.title[i + 1..], is_title_trim_delim));
            r.title = t;
            r.subtitle = s;
        }
        if r.artist.is_empty() {
            for s in [" - ", "--", "~"] {
                if let Some(i) = r.title.find(s) {
                    let (a, t) = (trim_right(&r.title[..i], is_title_trim_delim), trim_left(&r.title[i + s.len()..], is_break_delim));
                    r.artist = a;
                    r.title = t;
                    break;
                }
            }
        }
        if r.subtitle.is_empty() {
            for s in [" - ", "--", "~"] {
                if let Some(i) = r.title.find(s) {
                    let (t, sub) = (trim_right(&r.title[..i], is_break_delim), trim_left(&r.title[i + s.len()..], is_title_trim_delim));
                    r.title = t;
                    r.subtitle = sub;
                    break;
                }
            }
        }
        if let Some(i) = r.title.rfind('-') {
            if r.artist.is_empty() {
                let (artist, title) = (trim_right(&r.title[..i], is_title_trim_delim), trim_left(&r.title[i + 1..], is_break_delim));
                if !self.digsuf.is_match(&artist) && !self.digpre.is_match(&title) {
                    r.artist = artist;
                    r.title = title;
                }
            }
        }
        pos
    }

    /// The title of an app or game.
    fn app_title(&self, r: &mut Release) -> usize {
        let mut pos = 0;
        while pos < r.tags.len() && !r.tags[pos].is(&[TagType::Text, TagType::Date]) {
            pos += 1;
        }
        let (t, offset) = self.title(&r.tags[pos..], &[TagType::Text, TagType::Date]);
        r.title = t;
        pos + offset
    }

    /// The title of anything else.
    fn default_title(&self, r: &mut Release) -> usize {
        let mut pos = 0;
        while pos < r.tags.len() && !r.tags[pos].is(&[TagType::Text]) {
            pos += 1;
        }
        let (t, offset) = self.title(&r.tags[pos..], &[TagType::Text]);
        r.title = t;
        pos + offset
    }

    /// The text of `tags` of `types` up to a delimiter that isn't a space-like one, and where it stops.
    fn title(&self, tags: &[Tag], types: &[TagType]) -> (String, usize) {
        let mut v = Vec::new();
        let mut i = 0;
        while i < tags.len() {
            if tags[i].is(types) {
                v.push(tags[i].text_replace(".", " "));
            } else if tags[i].is(&[TagType::Delim]) {
                let s = tags[i].delim();
                if !s.contains(['(', ')', '[', ']', '{', '}', '\\', '/']) && s != "__" {
                    v.push(self.delim(s, tags, i, types));
                } else {
                    break;
                }
            } else {
                break;
            }
            i += 1;
        }
        // acronyms missing periods
        let s = self
            .missing
            .replace_all(&v.concat(), |c: &regex::Captures| {
                let a = c[0].trim().replace(' ', ".");
                format!("{}. ", a.trim_start_matches(['.', ' ']))
            })
            .into_owned();
        let s = s.replace(". .", ". ");
        // acronyms on single letters
        let s = self.bad.replace_all(&s, |c: &regex::Captures| self.fix.replace_all(&c[0], "$1").into_owned()).into_owned();
        let s = self.spaces.replace_all(&s, " ").into_owned();
        let s = self.ellips.replace_all(&s, "...").into_owned();
        let mut s = html_escape::decode_html_entities(&s).into_owned();
        if self.plus.find_iter(&s).count() > 1 {
            s = self.plus.replace_all(&s, " ").into_owned();
        }
        (s.trim_matches(is_title_trim_delim).to_string(), i)
    }

    /// Notes the unused text, and takes a checksum or group from the last of it.
    fn unused(&self, r: &mut Release, mut i: usize) {
        while i < r.tags.len() {
            if r.tags[i].is(&[TagType::Text]) {
                r.unused.push(i);
            }
            i += 1;
        }
        if let Some(&last) = r.unused.last() {
            let s = r.tags[last].text();
            if r.sum.is_empty() && self.sum.is_match(&s) && s.contains(|c: char| c.is_ascii_digit()) {
                r.sum = s;
                r.unused.pop();
            } else if r.group.is_empty() && !self.digits.is_match(&s) {
                r.group = s;
                r.unused.pop();
                self.widen_group(r, last);
            }
        }
    }

    /// Joins the words before a guessed group when single spaces separate them (`DarQ HONE`).
    fn widen_group(&self, r: &mut Release, mut i: usize) {
        let mut n = r.unused.len();
        while n > 0 && i >= 2 && r.unused[n - 1] == i - 2 && r.tags[i - 1].text() == " " {
            i = r.unused[n - 1];
            n -= 1;
        }
        if n == r.unused.len() {
            return;
        }
        let mut v: Vec<String> = r.unused[n..].iter().map(|&j| r.tags[j].text()).collect();
        v.push(r.group.clone());
        let group = v.join(" ");
        if lex::SPACED_GROUP.is_match(&group) {
            r.group = group;
            r.unused.truncate(n);
        }
    }

    /// A delimiter inside a title, as the title shows it.
    fn delim(&self, delim: &str, tags: &[Tag], i: usize, types: &[TagType]) -> String {
        match delim {
            "..." => return "...".into(),
            ".." | ". " => return ". ".into(),
            "" => return " ".into(),
            _ => {}
        }
        let mapped: String = delim
            .chars()
            .filter_map(|r| match r {
                '-' | '+' | ',' | '.' | '~' => Some(r),
                '\t' | '\n' | '\x0c' | '\r' | ' ' | '_' => Some(' '),
                _ => None,
            })
            .collect();
        let s = self.spaces.replace_all(&mapped, " ").into_owned();
        if s != "." || i == tags.len() - 1 {
            return self.spaces.replace_all(&s.replace('.', " "), " ").into_owned();
        }
        let ante = if i > 2 && tags[i - 2].is(&[TagType::Delim]) { tags[i - 2].delim().to_string() } else { String::new() };
        let prev = if i != 0 && tags[i - 1].is(types) { tags[i - 1].text() } else { String::new() };
        let next = if i < tags.len() - 1 && tags[i + 1].is(types) { tags[i + 1].text() } else { String::new() };
        if is_upper_letter(&prev) && is_upper_letter(&next) && !ante.contains(['-', '~']) {
            return ".".into();
        }
        if is_digit(&prev) && is_digit(&next) && !ante.contains(['-', '~']) {
            return ".".into();
        }
        " ".into()
    }
}

/// Whether `s` is empty or one capital letter.
fn is_upper_letter(s: &str) -> bool {
    let mut c = s.chars();
    match (c.next(), c.next()) {
        (None, _) => true,
        (Some(x), None) => x.is_uppercase(),
        _ => false,
    }
}

/// Whether `s` is one decimal digit (Go's `unicode.IsDigit`).
fn is_digit(s: &str) -> bool {
    static ND: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"^\p{Nd}$").expect("the digit pattern"));
    let mut c = s.chars();
    matches!((c.next(), c.next()), (Some(_), None)) && ND.is_match(s)
}
