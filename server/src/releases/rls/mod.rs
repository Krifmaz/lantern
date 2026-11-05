// Ported from autobrr/rls v0.10.0 (rls.go), MIT (originally moistari/rls; see LICENSE next to this file).
//! Reading release names, as autobrr does with the rls library: a release name is split into tags (text,
//! delimiters, source, resolution, codec, group …) by lexers ([`lex`]), and a builder ([`parse`]) turns the tags
//! into a [`Release`]: its type, title, year, season and episode, and the rest. Only the parts autobrr uses are
//! ported (no scanner, sorting or searching).
//!
//! | Module | What |
//! | --- | --- |
//! | [`taginfo`] | the known words of release names (`taginfo.csv`) |
//! | [`lex`] | the lexers that cut a name into tags |
//! | [`parse`] | the parser and the builder that makes a [`Release`] |
//! | [`goregex`] | Go's regular expressions in Rust |

pub mod goregex;
pub mod lex;
pub mod parse;
pub mod taginfo;

use std::sync::LazyLock;
use taginfo::Finder;

/// What a release name says, as rls reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Release {
    /// What kind of release it is.
    pub r#type: Type,
    /// The artist (music) or author (books).
    pub artist: String,
    /// The title.
    pub title: String,
    /// The subtitle (an episode's name).
    pub subtitle: String,
    /// The other title (after ` AKA `).
    pub alt: String,
    /// The platform (apps and games).
    pub platform: String,
    /// The processor architecture (apps).
    pub arch: String,
    /// The source (`WEB-DL`, `BluRay`, `CD` …).
    pub source: String,
    /// The resolution (`1080p`).
    pub resolution: String,
    /// The streaming site or collection (`AMZN`, `NF`, `CRiT`).
    pub collection: String,
    /// The year.
    pub year: i32,
    /// The month.
    pub month: i32,
    /// The day.
    pub day: i32,
    /// The season.
    pub series: i32,
    /// The episode.
    pub episode: i32,
    /// The version (`v2`).
    pub version: String,
    /// The disc (`CD1`, `D02`).
    pub disc: String,
    /// The video codecs.
    pub codec: Vec<String>,
    /// The HDR formats.
    pub hdr: Vec<String>,
    /// The bit depth (`10bit`).
    pub bit_depth: String,
    /// The audio formats.
    pub audio: Vec<String>,
    /// The audio channels (`5.1`).
    pub channels: String,
    /// Other tags (`PROPER`, `REPACK`, `REMUX`, `HYBRiD` …).
    pub other: Vec<String>,
    /// The cuts (`Extended`, `Directors.Cut`).
    pub cut: Vec<String>,
    /// The editions (`Remastered`, `Criterion`).
    pub edition: Vec<String>,
    /// The languages.
    pub language: Vec<String>,
    /// The size (`DVD9`, `1.4GiB`).
    pub size: String,
    /// The region (`R1`).
    pub region: String,
    /// The container (`mkv`).
    pub container: String,
    /// The genre (music).
    pub genre: String,
    /// The catalogue id (music).
    pub id: String,
    /// The release group.
    pub group: String,
    /// Other `[[key:value]]` meta data.
    pub meta: Vec<String>,
    /// The site (`[site]`).
    pub site: String,
    /// The checksum (`[ABCD1234]`).
    pub sum: String,
    /// The password (`{{pass}}`).
    pub pass: String,
    /// Whether it is a request (`[REQ]`).
    pub req: bool,
    /// The file extension.
    pub ext: String,
    /// Every tag, in order.
    tags: Vec<Tag>,
    /// The positions of date tags turned back to text.
    dates: Vec<usize>,
    /// The positions of text tags not used in a title.
    unused: Vec<usize>,
    /// Where the tags at the end of the name (group, extension …) start.
    end: usize,
}

impl Release {
    /// Every tag, in order.
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    /// The name as it was given (every tag's original text).
    pub fn original(&self) -> String {
        self.tags.iter().map(|t| t.v[0].as_str()).collect()
    }

    /// Every season and episode, sorted (`S01E02-E04` gives three).
    pub fn series_episodes(&self) -> Vec<(i32, i32)> {
        let mut v = Vec::new();
        let mut last: Option<usize> = None;
        for (i, tag) in self.tags.iter().enumerate() {
            if !tag.is(&[TagType::Series]) {
                continue;
            }
            let (series, _) = tag.series();
            for episode in tag.episodes() {
                let mut s = series;
                if s == 0 {
                    if let Some(l) = last {
                        let (ls, last_ep) = self.tags[l].series();
                        s = ls;
                        if 0 < i && self.tags[i - 1].v[0] == "-" {
                            for j in last_ep + 1..episode {
                                v.push((s, j));
                            }
                        }
                    }
                }
                v.push((s, episode));
            }
            if series != 0 {
                last = Some(i);
            }
        }
        v.sort();
        v
    }

    /// The text tags not used in the titles.
    pub fn unused(&self) -> Vec<&Tag> {
        self.unused.iter().map(|&i| &self.tags[i]).collect()
    }
}

/// Reads release name `src` with autobrr's parser (rls's tags plus autobrr's streaming sites).
pub fn parse_string(src: &str) -> Release {
    DEFAULT_PARSER.parse_release(src.as_bytes())
}

/// The parser autobrr uses: rls's tag list plus the streaming sites autobrr adds (autobrr's `releasetags.go`).
pub static DEFAULT_PARSER: LazyLock<parse::TagParser> =
    LazyLock::new(|| parse::TagParser::new(taginfo::all(&[autobrr_collections()]), lex::default_lexers()));

/// The streaming sites autobrr adds to rls's list (`extraCollections` in autobrr's `releasetags.go`).
fn autobrr_collections() -> taginfo::Infos {
    let extra: [[&str; 3]; 28] = [
        ["4OD", "4OD", "(?-i:4OD)"],
        ["ABEMA", "Abema", "(?-i:ABEMA)"],
        ["ADN", "Animation Digital Network", "(?-i:ADN)"],
        ["AUBC", "Australian Broadcasting Corporation", ""],
        ["AUViO", "French AUViO", "(?-i:AUViO)"],
        ["Bilibili", "Bilibili", "(?-i:Bilibili)"],
        ["CRiT", "Criterion Channel", "(?-i:CRiT)"],
        ["FOD", "Fuji Television On Demand", "(?-i:FOD)"],
        ["HIDIVE", "HIDIVE", "(?-i:HIDIVE)"],
        ["ITVX", "ITVX aka ITV", ""],
        ["MA", "Movies Anywhere", "(?-i:MA)"],
        ["MY5", "MY5 aka Channel 5", ""],
        ["MyCanal", "French Groupe Canal+", "(?-i:MyCanal)"],
        ["NOW", "Now", "(?-i:NOW)"],
        ["NLZ", "Dutch NLZiet", "(?-i:NLZ|NLZiet)"],
        ["OViD", "OViD", "(?-i:OViD)"],
        ["STRP", "Star+", "(?-i:STRP)"],
        ["U-NEXT", "U-NEXT", "(?-i:U-NEXT)"],
        ["TVer", "TVer", "(?-i:TVer)"],
        ["TVING", "TVING", "(?-i:TVING)"],
        ["VIU", "VIU", "(?-i:VIU)"],
        ["VDL", "Videoland", "(?-i:VDL)"],
        ["VRV", "VRV", "(?-i:VRV)"],
        ["Pathe", "Pathé Thuis", "(?-i:Pathe)"],
        ["SALTO", "SALTO", "(?-i:SALTO)"],
        ["SHOWTIME", "SHOWTIME", "(?-i:SHO|SHOWTIME)"],
        ["SYFY", "SYFY", "(?-i:SYFY)"],
        ["QUIBI", "QUIBI", "(?-i:QIBI|QUIBI)"],
    ];
    let list = extra
        .iter()
        .filter_map(|[tag, title, re]| taginfo::Taginfo::new(tag, title, re, "", "", "").ok().map(std::sync::Arc::new))
        .collect();
    [("collection".to_string(), list)].into_iter().collect()
}

/// One piece of a release name.
#[derive(Debug, Clone)]
pub struct Tag {
    /// What kind of piece it is.
    typ: TagType,
    /// The original text, then the captured values.
    v: Vec<String>,
    /// Finds the tag info for its normalized text.
    f: Option<Finder>,
    /// What kind it was before it was changed (rls's zero value: whitespace).
    prev: TagType,
    /// The finder it had before it was changed.
    prevf: Option<Finder>,
}

impl PartialEq for Tag {
    fn eq(&self, o: &Tag) -> bool {
        self.typ == o.typ && self.v == o.v && self.prev == o.prev
    }
}

impl Tag {
    /// A tag of kind `typ` with finder `f` and values `b` (the original text first; at least two).
    pub fn new(typ: TagType, f: Option<Finder>, b: Vec<String>) -> Tag {
        assert!(b.len() >= 2, "must provide at least 2 values to NewTag");
        Tag { typ, v: b, f, prev: TagType::Whitespace, prevf: None }
    }

    /// A copy of the tag as kind `typ`, remembering what it was.
    pub fn as_type(&self, typ: TagType, f: Option<Finder>) -> Tag {
        Tag { typ, f, v: self.v.clone(), prev: self.typ, prevf: self.f.clone() }
    }

    /// A copy of the tag as what it was before.
    pub fn revert(&self) -> Tag {
        Tag { typ: self.prev, f: self.prevf.clone(), v: self.v.clone(), prev: TagType::Whitespace, prevf: None }
    }

    /// Whether the tag is one of `types`.
    pub fn is(&self, types: &[TagType]) -> bool {
        types.contains(&self.typ)
    }

    /// Whether the tag was one of `types` before it was changed.
    pub fn was(&self, types: &[TagType]) -> bool {
        types.contains(&self.prev)
    }

    /// The tag info for the tag's normalized text.
    pub fn info(&self) -> Option<std::sync::Arc<taginfo::Taginfo>> {
        self.f.as_ref().and_then(|f| f.find(&self.normalize()).cloned())
    }

    /// Whether the tag is a single episode with no season (`- 02 -`).
    pub fn single_ep(&self) -> bool {
        if self.typ == TagType::Series {
            let (s, e) = self.series();
            return s == 0 && e != 0 && self.v[1].is_empty() && self.v[0] == self.v[2];
        }
        false
    }

    /// The release type of the tag's info (`Unknown` when none).
    pub fn info_type(&self) -> Type {
        self.info().map(|i| i.typ()).unwrap_or_default()
    }

    /// Whether the tag's info belongs to its release type only.
    pub fn info_excl(&self) -> bool {
        self.info().is_some_and(|i| i.excl())
    }

    /// The tag's kind.
    pub fn tag_type(&self) -> TagType {
        self.typ
    }

    /// The tag's kind before it was changed.
    pub fn prev(&self) -> TagType {
        self.prev
    }

    /// The original text.
    pub fn original(&self) -> &str {
        &self.v[0]
    }

    /// `s` replaced by the tag word its finder knows it as, with `$1`, `$2` … replaced by `v`.
    fn normalize_with(&self, s: &str, v: &[String]) -> String {
        let mut s = s.to_string();
        if let Some(f) = &self.f {
            if let Some(info) = f.find(&s) {
                s = info.tag().to_string();
            }
            for (i, x) in v.iter().enumerate() {
                s = s.replace(&format!("${}", i + 1), x);
            }
        }
        s
    }

    /// The normal form of the value.
    fn norm(&self) -> String {
        self.normalize_with(&self.v[1], &self.v[2..])
    }

    /// The normalized text of the tag, whatever its kind (rls's `Normalize`, `%s`).
    pub fn normalize(&self) -> String {
        match self.typ {
            TagType::Whitespace | TagType::Delim | TagType::Version | TagType::Group => self.v[1].clone(),
            TagType::Text => self.text(),
            TagType::Date => {
                let (y, m, d) = self.date();
                if m != 0 && d != 0 {
                    format!("{y}-{m:02}-{d:02}")
                } else {
                    y.to_string()
                }
            }
            TagType::Series => {
                let (s, e) = self.series();
                if e != 0 {
                    format!("S{s:02}E{e:02}")
                } else {
                    format!("S{s:02}")
                }
            }
            TagType::Disc => self.disc(),
            TagType::Channels => self.channels(),
            TagType::Other => self.other(),
            TagType::Size => self.size(),
            TagType::Meta => {
                let (typ, s) = self.meta();
                match typ.as_str() {
                    "site" | "sum" => format!("[{s}]"),
                    "pass" => format!("{{{{{s}}}}}"),
                    "req" => "[REQ]".into(),
                    _ => format!("[[{typ}:{s}]]"),
                }
            }
            TagType::Ext => self.v[1].to_lowercase(),
            _ => self.norm(),
        }
    }

    /// The whitespace or delimiter text.
    pub fn delim(&self) -> &str {
        &self.v[1]
    }

    /// The text (a date or season turned to text keeps its original form).
    pub fn text(&self) -> String {
        match self.prev {
            TagType::Date | TagType::Series => self.v[0].clone(),
            TagType::Channels => self.channels(),
            _ => self.v[1].clone(),
        }
    }

    /// The text with `old` replaced by `new` (not for channels turned to text).
    pub fn text_replace(&self, old: &str, new: &str) -> String {
        let s = self.text();
        if self.prev == TagType::Channels {
            return s;
        }
        s.replace(old, new)
    }

    /// The platform, arch, source, resolution, collection, codec, HDR, bit depth, audio, cut, edition, language,
    /// region, container, genre or id value, normalized.
    pub fn value(&self) -> String {
        self.norm()
    }

    /// The year, month and day.
    pub fn date(&self) -> (i32, i32, i32) {
        (atoi(&self.v[1]), atoi(&self.v[2]), atoi(&self.v[3]))
    }

    /// The season and episode.
    pub fn series(&self) -> (i32, i32) {
        (atoi(&self.v[1]), atoi(&self.v[2]))
    }

    /// Every episode.
    pub fn episodes(&self) -> Vec<i32> {
        self.v[2..].iter().map(|b| atoi(b)).filter(|&e| e != 0).collect()
    }

    /// The disc (`CD1`, `D02`, `2DiSCS`, `2x`).
    pub fn disc(&self) -> String {
        let disc = atoi(&self.v[2]);
        match self.v[1].as_str() {
            "CD" | "DVD" => format!("{}{disc}", self.v[1]),
            "S" => format!("{disc}DiSCS"),
            "X" => format!("{disc}x"),
            _ => format!("D{disc:02}"),
        }
    }

    /// The channels (`5.1`).
    pub fn channels(&self) -> String {
        let s: String = self.normalize_with(&self.v[1], &[]).chars().filter(|c| c.is_ascii_digit()).collect();
        if s.is_empty() {
            return ".".into();
        }
        format!("{}.{}", &s[..1], &s[1..])
    }

    /// The other value (`19XX` and `20XX` in capitals).
    pub fn other(&self) -> String {
        let s = self.norm();
        let y = s.to_uppercase();
        if y == "19XX" || y == "20XX" {
            return y;
        }
        s
    }

    /// The size (`1.4GiB`).
    pub fn size(&self) -> String {
        self.norm().to_uppercase().replace('I', "i")
    }

    /// The meta key and value.
    pub fn meta(&self) -> (String, String) {
        (self.v[1].clone(), self.v[2].clone())
    }
}

/// Go's `strconv.Atoi`, 0 when it can't be read.
fn atoi(s: &str) -> i32 {
    s.parse().unwrap_or(0)
}

/// The kinds of tag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TagType {
    /// Leading or trailing whitespace.
    #[default]
    Whitespace,
    /// A delimiter (`.`, ` - `, `(`).
    Delim,
    /// Text (title words, unknown words).
    Text,
    /// A platform.
    Platform,
    /// A processor architecture.
    Arch,
    /// A source.
    Source,
    /// A resolution.
    Resolution,
    /// A streaming site or collection.
    Collection,
    /// A date or year.
    Date,
    /// A season and episodes.
    Series,
    /// A version.
    Version,
    /// A disc.
    Disc,
    /// A video codec.
    Codec,
    /// An HDR format.
    Hdr,
    /// A bit depth.
    BitDepth,
    /// An audio format.
    Audio,
    /// Audio channels.
    Channels,
    /// Something else known.
    Other,
    /// A cut.
    Cut,
    /// An edition.
    Edition,
    /// A language.
    Language,
    /// A size.
    Size,
    /// A region.
    Region,
    /// A container.
    Container,
    /// A genre.
    Genre,
    /// A catalogue id.
    Id,
    /// The release group.
    Group,
    /// File meta data.
    Meta,
    /// A file extension.
    Ext,
}

impl TagType {
    /// The kind's name in rls's tag list (`source`, `hdr` …).
    pub fn info_key(self) -> &'static str {
        match self {
            TagType::Whitespace => "whitespace",
            TagType::Delim => "delim",
            TagType::Text => "text",
            TagType::Platform => "platform",
            TagType::Arch => "arch",
            TagType::Source => "source",
            TagType::Resolution => "resolution",
            TagType::Collection => "collection",
            TagType::Date => "date",
            TagType::Series => "series",
            TagType::Version => "version",
            TagType::Disc => "disc",
            TagType::Codec => "codec",
            TagType::Hdr => "hdr",
            TagType::BitDepth => "bitdepth",
            TagType::Audio => "audio",
            TagType::Channels => "channels",
            TagType::Other => "other",
            TagType::Cut => "cut",
            TagType::Edition => "edition",
            TagType::Language => "language",
            TagType::Size => "size",
            TagType::Region => "region",
            TagType::Container => "container",
            TagType::Genre => "genre",
            TagType::Id => "id",
            TagType::Group => "group",
            TagType::Meta => "meta",
            TagType::Ext => "ext",
        }
    }
}

/// The kinds of release.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    /// Not known.
    #[default]
    Unknown,
    /// An app.
    App,
    /// An audiobook.
    Audiobook,
    /// A book.
    Book,
    /// A comic.
    Comic,
    /// Education.
    Education,
    /// One episode (or a daily show's date).
    Episode,
    /// A game.
    Game,
    /// A magazine.
    Magazine,
    /// A movie.
    Movie,
    /// Music.
    Music,
    /// A season or series.
    Series,
}

impl Type {
    /// The type named `s` (`movie`, `episode` …); `Unknown` for anything else.
    pub fn parse(s: &str) -> Type {
        match s {
            "app" => Type::App,
            "audiobook" => Type::Audiobook,
            "book" => Type::Book,
            "comic" => Type::Comic,
            "education" => Type::Education,
            "episode" => Type::Episode,
            "game" => Type::Game,
            "magazine" => Type::Magazine,
            "movie" => Type::Movie,
            "music" => Type::Music,
            "series" => Type::Series,
            _ => Type::Unknown,
        }
    }

    /// The type's name (empty for `Unknown`).
    pub fn as_str(self) -> &'static str {
        match self {
            Type::Unknown => "",
            Type::App => "app",
            Type::Audiobook => "audiobook",
            Type::Book => "book",
            Type::Comic => "comic",
            Type::Education => "education",
            Type::Episode => "episode",
            Type::Game => "game",
            Type::Magazine => "magazine",
            Type::Movie => "movie",
            Type::Music => "music",
            Type::Series => "series",
        }
    }

    /// Whether the type is one of `types`.
    pub fn is(self, types: &[Type]) -> bool {
        types.contains(&self)
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether `r` is any delimiter.
pub fn is_any_delim(r: char) -> bool {
    matches!(r, '\t' | '\n' | '\x0c' | '\r' | ' ' | '(' | ')' | '+' | ',' | '-' | '.' | '_' | '[' | '/' | '\\' | ']' | '{' | '}' | '~')
}

/// Whether `r` is a break delimiter (any delimiter but `-`).
pub fn is_break_delim(r: char) -> bool {
    r != '-' && is_any_delim(r)
}

/// Whether `r` is a title trim delimiter (any delimiter but `.` and `+`).
pub fn is_title_trim_delim(r: char) -> bool {
    r != '.' && r != '+' && is_any_delim(r)
}

/// Changes a character given the one before and after (`None`: remove it).
pub type Transformer = fn(char, char, char) -> Option<char>;

/// Turns spaces into one `' '`, removes some characters and accents, optionally lower cases and trims, between
/// Unicode decomposition and composition (rls's `Collapser` in its `transform.Chain(norm.NFD, …, norm.NFC)`).
pub struct Collapser<'a> {
    /// Lower case every character.
    pub lower: bool,
    /// Trim leading and trailing spaces.
    pub trim: bool,
    /// The characters to remove.
    pub remove: &'a str,
    /// The characters that are spaces.
    pub space: &'a str,
    /// Changes a character given the one before and after (`None`: remove it).
    pub transformer: Option<Transformer>,
}

impl Collapser<'_> {
    /// `s`, transformed.
    pub fn apply(&self, s: &str) -> String {
        use unicode_normalization::UnicodeNormalization;
        let src: Vec<char> = s.nfd().collect();
        let (mut out, mut last) = (String::new(), 0usize);
        let mut prev = '\0';
        let mut i = 0;
        if self.trim {
            while i < src.len() && (self.remove.contains(src[i]) || self.space.contains(src[i])) {
                i += 1;
            }
        }
        while i < src.len() {
            let mut r = src[i];
            let next = src.get(i + 1).copied().unwrap_or('\0');
            i += 1;
            let is_space = self.space.contains(r);
            if (is_space && prev == ' ') || self.remove.contains(r) || is_mark(r) {
                continue;
            }
            if is_space {
                r = ' ';
            }
            if let Some(t) = self.transformer {
                match t(r, prev, next) {
                    Some(x) => r = x,
                    None => continue,
                }
            }
            if self.lower {
                r = to_lower(r);
            }
            out.push(r);
            prev = r;
            if r != ' ' {
                last = out.len();
            }
        }
        if self.trim {
            out.truncate(last);
        }
        out.nfc().collect()
    }
}

/// Whether `c` is a non-spacing mark (Unicode `Mn`).
fn is_mark(c: char) -> bool {
    static MN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\p{Mn}$").expect("the Mn pattern"));
    if c.is_ascii() {
        return false;
    }
    MN.is_match(c.encode_utf8(&mut [0; 4]))
}

/// Go's `unicode.ToLower`: one character in, one out.
pub fn to_lower(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        (Some(x), Some(_)) if c == '\u{130}' => x,
        _ => c,
    }
}

/// Go's `unicode.IsLetter`.
pub fn is_letter(c: char) -> bool {
    static L: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\p{L}$").expect("the letter pattern"));
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    L.is_match(c.encode_utf8(&mut [0; 4]))
}

/// Go's `unicode.IsSpace`.
pub fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ' | '\u{85}' | '\u{a0}') || (!c.is_ascii() && c.is_whitespace())
}

/// The `$` and `£` rule rls's and autobrr's normalizers share: `$` and `£` next to a letter become `S` and `L`,
/// elsewhere they go; a `-` after a space goes.
pub fn money_letters(r: char, prev: char, next: char) -> Option<char> {
    match r {
        '-' if is_space(prev) => None,
        '$' if is_letter(prev) || is_letter(next) => Some('S'),
        '£' if is_letter(prev) || is_letter(next) => Some('L'),
        '$' | '£' => None,
        _ => Some(r),
    }
}

/// rls's `MustNormalize`: a lower case, clean form of `s` for matching titles.
pub fn must_normalize(s: &str) -> String {
    Collapser { lower: true, trim: true, remove: "`':;~!@#%^*=+()[]{}<>/?|\\\",", space: " \t\r\n\x0c._", transformer: Some(money_letters) }
        .apply(s)
}

/// rls's `MustClean`: `s` without accents and `'`, with spaces collapsed.
pub fn must_clean(s: &str) -> String {
    Collapser { lower: false, trim: true, remove: "'", space: " \t\r\n\x0c", transformer: None }.apply(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rls's TestCollapser.
    #[test]
    fn collapser() {
        let tests = [
            ("''\t\tAmélie\r\r1998\n\nMKV\x0c\x0c''", "Amelie 1998 MKV", "amelie 1998 mkv"),
            ("\t Star Trek  -  Lower  Decks \t", "Star Trek - Lower Decks", "star trek lower decks"),
            ("Star Trek-Lower DECKS", "Star Trek-Lower DECKS", "star trek-lower decks"),
            ("   t-pain  rappa   ", "t-pain rappa", "t-pain rappa"),
            ("\t\x0c\x0c[PMEDIA] \u{2b50}\u{fe0f}\u{0009}\t", "[PMEDIA] \u{2b50}", "pmedia \u{2b50}"),
            ("$elfie.Shootout", "$elfie.Shootout", "selfie shootout"),
            ("LAWLE$$", "LAWLE$$", "lawless"),
            ("$100 TAXI RIDE", "$100 TAXI RIDE", "100 taxi ride"),
            ("Mr. & Mrs. Smith", "Mr. & Mrs. Smith", "mr & mrs smith"),
            ("Bob's Burgers", "Bobs Burgers", "bobs burgers"),
        ];
        for (s, c, n) in tests {
            assert_eq!(must_clean(s), c, "clean {s:?}");
            assert_eq!(must_normalize(s), n, "normalize {s:?}");
        }
    }
}
