//! The announce definitions: per tracker, which IRC network and channels announce new releases, and how to read
//! an announce line. Lantern reads autobrr's definition files (`internal/indexer/definitions/*.yaml`, bundled in
//! the image) as they are:
//!
//! ```yaml
//! identifier: tracker
//! name: Tracker
//! urls: [https://tracker.example/]
//! settings: [{ name: passkey, type: secret, label: Passkey }]
//! irc:
//!   network: Tracker
//!   server: irc.tracker.example
//!   port: 6697
//!   tls: true
//!   channels: ["#announce"]
//!   announcers: [Announcer]
//!   settings: [{ name: nick }, { name: auth.account }, { name: auth.password }, { name: invite_command }]
//!   parse:
//!     type: single            # or multi: one announce over several lines, in order
//!     lines:
//!       - test: ["New: Some.Release-GRP [TV] 1.2 GB https://tracker.example/t/1"]
//!         pattern: 'New: (.*) \[(.*)\] (.*) (https?://.*/)t/(\d+)'
//!         vars: [torrentName, category, torrentSize, baseUrl, torrentId]
//!     match:
//!       torrenturl: "{{ .baseUrl }}dl/{{ .torrentId }}?passkey={{ .passkey }}"
//!       encode: [torrentName]
//! ```

use super::Found;
use crate::indexers::template::{self, Vars};
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// One setting a definition asks for (a passkey, the IRC nick).
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Setting {
    /// Its name (`passkey`, `auth.password`).
    pub name: String,
    /// `text`, `secret` or `bool`.
    #[serde(rename = "type")]
    pub kind: String,
    /// What people see.
    pub label: String,
    /// A hint under the field.
    pub help: String,
    /// Must be filled.
    pub required: bool,
    /// Its value when not filled.
    pub default: String,
}

impl Setting {
    /// Whether the value is a secret: never sent back, never logged.
    pub fn is_secret(&self) -> bool {
        self.kind == "secret" || self.name.contains("password") || self.name.contains("key") || self.name == "invite_command"
    }
}

/// One line pattern.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Line {
    /// Example lines the pattern must read.
    #[serde(deserialize_with = "one_or_many")]
    pub test: Vec<String>,
    /// The regular expression.
    pub pattern: String,
    /// The names of its groups, in order.
    pub vars: Vec<String>,
    /// Lantern's compiled copy of `pattern`.
    #[serde(skip)]
    pub re: Option<Regex>,
}

/// How a read announce becomes a release: its download link and the values to encode first.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Match {
    /// The download link template.
    pub torrenturl: String,
    /// The release page template.
    pub infourl: String,
    /// The name template (empty: `torrentName`).
    pub torrentname: String,
    /// Values URL-encoded before the templates see them.
    pub encode: Vec<String>,
}

/// How to read announce lines.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Parse {
    /// `single` (each line is an announce) or `multi` (the lines together, in order).
    #[serde(rename = "type")]
    pub kind: String,
    /// The line patterns.
    pub lines: Vec<Line>,
    /// The download link and name.
    #[serde(rename = "match")]
    pub matcher: Match,
    /// Values turned into others: var → its value → (var → new value).
    pub mappings: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// The unit of a size announced without one (`MB`).
    pub forcesizeunit: String,
}

/// A tracker's IRC side.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Irc {
    /// The network's name.
    pub network: String,
    /// The server's host name.
    pub server: String,
    /// The port.
    pub port: u16,
    /// Connect with TLS.
    pub tls: bool,
    /// The announce channels.
    pub channels: Vec<String>,
    /// The nicks that announce.
    pub announcers: Vec<String>,
    /// The IRC settings it asks for (nick, NickServ account and password, invite command).
    pub settings: Vec<Setting>,
    /// How to read the lines.
    pub parse: Parse,
}

/// One tracker's announce definition.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Definition {
    /// Its id (`identifier`).
    pub identifier: String,
    /// The tracker's name.
    pub name: String,
    /// What it is.
    pub description: String,
    /// The site's addresses.
    pub urls: Vec<String>,
    /// `private`, `public` or `semi-private`.
    pub privacy: String,
    /// `torrent` or `newznab`.
    pub protocol: String,
    /// What it supports (`irc`, `rss`).
    pub supports: Vec<String>,
    /// The site settings it asks for (passkey, RSS key …).
    pub settings: Vec<Setting>,
    /// The IRC side; `None` for a definition without one.
    pub irc: Option<Irc>,
}

/// One value or a list of them, read as a list.
fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<OneOrMany>::deserialize(d)? {
        None => Vec::new(),
        Some(OneOrMany::One(s)) => vec![s],
        Some(OneOrMany::Many(v)) => v,
    })
}

impl Definition {
    /// Reads a definition file's text; the problem in words when it can't be read.
    pub fn parse(text: &str) -> Result<Definition, String> {
        let mut d: Definition = serde_yaml::from_str(text).map_err(|e| format!("The definition can't be read: {e}"))?;
        if d.identifier.is_empty() {
            return Err("The definition has no identifier".into());
        }
        if let Some(irc) = d.irc.as_mut() {
            for line in irc.parse.lines.iter_mut() {
                line.re = Some(Regex::new(&line.pattern).map_err(|_| format!("A line pattern of {} can't be read", d.name))?);
            }
        }
        Ok(d)
    }

    /// Every setting it asks for: the site's, then the IRC ones.
    pub fn all_settings(&self) -> Vec<Setting> {
        let mut out = self.settings.clone();
        if let Some(irc) = &self.irc {
            out.extend(irc.settings.iter().cloned());
        }
        out
    }

    /// Whether setting `name` is a secret.
    pub fn is_secret(&self, name: &str) -> bool {
        self.all_settings()
            .iter()
            .find(|s| s.name == name)
            .map(Setting::is_secret)
            .unwrap_or_else(|| name.contains("password") || name.contains("key"))
    }

    /// Whether its own example lines read with its patterns; the first one that doesn't, when not.
    pub fn self_test(&self) -> Result<(), String> {
        let Some(irc) = &self.irc else { return Ok(()) };
        for l in &irc.parse.lines {
            for t in &l.test {
                if !l.re.as_ref().is_some_and(|re| re.is_match(&strip_formatting(t))) {
                    return Err(format!("{}: the example line \"{t}\" doesn't match its pattern", self.name));
                }
            }
        }
        Ok(())
    }
}

/// Reads one tracker's announce lines, in order for multi-line announces.
#[derive(Debug, Clone)]
pub struct LineReader {
    def: Arc<Definition>,
    vars: HashMap<String, String>,
    next: usize,
}

impl LineReader {
    /// A reader for `def`'s announces.
    pub fn new(def: Arc<Definition>) -> LineReader {
        LineReader { def, vars: HashMap::new(), next: 0 }
    }

    /// Reads one line (IRC formatting is removed first); the announce's values when it completes one.
    pub fn feed(&mut self, line: &str) -> Option<HashMap<String, String>> {
        let irc = self.def.irc.as_ref()?;
        let lines = &irc.parse.lines;
        if lines.is_empty() {
            return None;
        }
        let line = strip_formatting(line);
        let multi = irc.parse.kind == "multi" && lines.len() > 1;
        if !multi {
            for l in lines {
                if let Some(v) = read(l, &line) {
                    return Some(v);
                }
            }
            return None;
        }
        // multi: line `next` must come; a first line starts over
        let at = if read(&lines[self.next], &line).is_some() {
            self.next
        } else if read(&lines[0], &line).is_some() {
            self.vars.clear();
            0
        } else {
            return None;
        };
        if at == 0 {
            self.vars.clear();
        }
        self.vars.extend(read(&lines[at], &line).unwrap_or_default());
        self.next = at + 1;
        if self.next == lines.len() {
            self.next = 0;
            return Some(std::mem::take(&mut self.vars));
        }
        None
    }
}

/// The values line pattern `l` reads from `line`.
fn read(l: &Line, line: &str) -> Option<HashMap<String, String>> {
    let c = l.re.as_ref()?.captures(line)?;
    Some(
        l.vars
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), c.get(i + 1).map(|m| m.as_str().trim().to_string()).unwrap_or_default()))
            .collect(),
    )
}

/// `line` without mIRC colours, bold, italics, underline and reset codes.
pub fn strip_formatting(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x03' => {
                // \x03NN[,NN]
                for _ in 0..2 {
                    if chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                }
                if chars.peek() == Some(&',') {
                    let mut look = chars.clone();
                    look.next();
                    if look.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                        for _ in 0..2 {
                            if chars.peek().is_some_and(char::is_ascii_digit) {
                                chars.next();
                            }
                        }
                    }
                }
            }
            '\x02' | '\x0f' | '\x16' | '\x1d' | '\x1f' | '\x1e' | '\x11' => {}
            c => out.push(c),
        }
    }
    out
}

/// The release an announce's values `vars` describe, with the source's `settings` (passkey …) for the links.
pub fn found(def: &Definition, mut vars: HashMap<String, String>, settings: &BTreeMap<String, String>) -> Result<Found, String> {
    let Some(irc) = &def.irc else { return Err("This tracker has no IRC announces".into()) };
    let p = &irc.parse;
    // mappings: a value turned into others
    for (var, by_value) in &p.mappings {
        if let Some(set) = vars.get(var).and_then(|v| by_value.get(v)).cloned() {
            vars.extend(set);
        }
    }
    let mut t = Vars::default();
    for (k, v) in settings {
        t.set(k.clone(), v.clone());
    }
    if let Some(base) = def.urls.first() {
        t.set("baseUrl", base.clone());
    }
    for (k, v) in &vars {
        let v = if p.matcher.encode.contains(k) {
            percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string()
        } else {
            v.clone()
        };
        if !(k == "baseUrl" && v.is_empty()) {
            t.set(k.clone(), v);
        }
    }
    let title = if p.matcher.torrentname.is_empty() {
        vars.get("torrentName").cloned().unwrap_or_default()
    } else {
        template::render(&p.matcher.torrentname, &t).map_err(|e| format!("{}: {e}", def.name))?
    };
    if title.trim().is_empty() {
        return Err("The announce has no release name".into());
    }
    let download_url = template::render(&p.matcher.torrenturl, &t).map_err(|e| format!("{}: {e}", def.name))?;
    let info_url = template::render(&p.matcher.infourl, &t).map_err(|e| format!("{}: {e}", def.name))?;
    let get = |k: &str| vars.get(k).cloned().unwrap_or_default();
    let size = {
        let s = get("torrentSize");
        let s = if !p.forcesizeunit.is_empty() && !s.is_empty() && s.chars().last().is_some_and(|c| c.is_ascii_digit()) {
            format!("{s} {}", p.forcesizeunit)
        } else {
            s
        };
        super::humanize::parse_bytes(&s).ok()
    };
    let fl = get("freeleech").to_lowercase();
    let freeleech_percent = get("freeleechPercent").trim_end_matches('%').trim().parse::<u32>().ok();
    let freeleech = matches!(fl.as_str(), "freeleech" | "yes" | "true" | "1" | "fl" | "free")
        || fl.contains("freeleech")
        || freeleech_percent == Some(100);
    let mut tags: Vec<String> = Vec::new();
    for k in ["tags", "releaseTags"] {
        tags.extend(get(k).split([',', '|', '/']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    }
    Ok(Found {
        title: title.trim().to_string(),
        download_url,
        info_url,
        size,
        category: get("category"),
        freeleech,
        freeleech_percent,
        uploader: get("uploader"),
        tags,
        year: get("year").parse().ok(),
        protocol: if def.protocol == "newznab" || def.protocol == "usenet" { "usenet".into() } else { "torrent".into() },
        identifier: def.identifier.clone(),
        vars: vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        force_size_unit: p.forcesizeunit.clone(),
        ..Found::default()
    })
}

/// The announce definitions on disk: the bundled ones, then the data folder's (`<data>/announce`), which win.
#[derive(Clone, Default)]
pub struct Library {
    dirs: Vec<PathBuf>,
    defs: Arc<RwLock<BTreeMap<String, Arc<Definition>>>>,
    errors: Arc<RwLock<Vec<String>>>,
}

impl Library {
    /// The library of the folders `dirs`, read now.
    pub fn new(dirs: Vec<PathBuf>) -> Library {
        let l = Library { dirs, ..Library::default() };
        l.reload();
        l
    }

    /// Reads the folders again.
    pub fn reload(&self) {
        let mut defs = BTreeMap::new();
        let mut errors = Vec::new();
        for dir in &self.dirs {
            read_dir(dir, &mut defs, &mut errors);
        }
        *self.defs.write().unwrap_or_else(|e| e.into_inner()) = defs;
        *self.errors.write().unwrap_or_else(|e| e.into_inner()) = errors;
    }

    /// Adds one definition (tests).
    pub fn insert(&self, def: Definition) {
        self.defs.write().unwrap_or_else(|e| e.into_inner()).insert(def.identifier.clone(), Arc::new(def));
    }

    /// Definition `id`.
    pub fn get(&self, id: &str) -> Option<Arc<Definition>> {
        self.defs.read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    /// Every definition, by id.
    pub fn all(&self) -> Vec<Arc<Definition>> {
        self.defs.read().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    /// The files that couldn't be read, in words.
    pub fn errors(&self) -> Vec<String> {
        self.errors.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Reads every `.yaml`/`.yml` file of `dir` into `defs`.
fn read_dir(dir: &Path, defs: &mut BTreeMap<String, Arc<Definition>>, errors: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml")).collect();
    paths.sort();
    for p in paths {
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| Definition::parse(&t)) {
            Ok(d) => {
                defs.insert(d.identifier.clone(), Arc::new(d));
            }
            Err(e) => errors.push(format!("{}: {e}", p.file_name().unwrap_or_default().to_string_lossy())),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A definition in autobrr's format, written for these tests.
    pub const SINGLE: &str = r##"
identifier: lighthouse
name: Lighthouse
urls: [https://lighthouse.example/]
privacy: private
protocol: torrent
supports: [irc, rss]
settings:
  - { name: passkey, type: secret, required: true, label: Passkey }
irc:
  network: Lighthouse
  server: irc.lighthouse.example
  port: 6697
  tls: true
  channels: ["#announce"]
  announcers: [Keeper]
  settings:
    - { name: nick, type: text, required: true, label: Nick }
    - { name: auth.account, type: text, label: NickServ account }
    - { name: auth.password, type: secret, label: NickServ password }
  parse:
    type: single
    lines:
      - test:
          - "New Torrent: Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP [TV :: HD] 1.5 GB by uploader FREELEECH - https://lighthouse.example/torrent/123"
        pattern: 'New Torrent: (.+) \[(.+)\] ([\d.]+ \w+) by (\S+) ?(FREELEECH)? - (https?://[^/]+/)torrent/(\d+)'
        vars: [torrentName, category, torrentSize, uploader, freeleech, baseUrl, torrentId]
    match:
      torrenturl: "{{ .baseUrl }}download/{{ .torrentId }}/{{ .passkey }}/{{ .torrentName }}.torrent"
      infourl: "{{ .baseUrl }}torrent/{{ .torrentId }}"
      encode: [torrentName]
    mappings:
      category:
        "TV :: HD":
          category: TV/HD
"##;

    /// A multi-line definition.
    pub const MULTI: &str = r##"
identifier: harbour
name: Harbour
urls: [https://harbour.example/]
protocol: torrent
irc:
  network: Harbour
  server: irc.harbour.example
  port: 6667
  channels: ["#new"]
  announcers: [Bot]
  parse:
    type: multi
    forcesizeunit: MB
    lines:
      - test: ["Name: Movie.2024.2160p.BluRay-GRP"]
        pattern: 'Name: (.+)'
        vars: [torrentName]
      - test: ["Size: 2048 Tags: hdr, action"]
        pattern: 'Size: (\d+) Tags: (.*)'
        vars: [torrentSize, tags]
      - test: ["Link: https://harbour.example/t/9"]
        pattern: 'Link: (https?://.+/)t/(\d+)'
        vars: [baseUrl, torrentId]
    match:
      torrenturl: "{{ .baseUrl }}get/{{ .torrentId }}"
"##;

    #[test]
    fn reads_a_single_line_announce() {
        let d = Arc::new(Definition::parse(SINGLE).unwrap());
        d.self_test().unwrap();
        let mut r = LineReader::new(d.clone());
        let line = "\x0303New Torrent:\x0f \x02Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP\x02 [TV :: HD] 1.5 GB by uploader FREELEECH - https://lighthouse.example/torrent/123";
        let vars = r.feed(line).expect("the line reads");
        let settings = BTreeMap::from([("passkey".to_string(), "abc".to_string())]);
        let f = found(&d, vars, &settings).unwrap();
        assert_eq!(f.title, "Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP");
        assert_eq!(
            f.download_url,
            "https://lighthouse.example/download/123/abc/Show%2EName%2ES01E02%2E1080p%2EWEB%2DDL%2EH%2E264%2DGROUP.torrent"
        );
        assert_eq!(f.info_url, "https://lighthouse.example/torrent/123");
        assert_eq!(f.size, Some(1_500_000_000));
        assert_eq!(f.category, "TV/HD");
        assert!(f.freeleech);
        assert_eq!(f.uploader, "uploader");
        assert!(r.feed("someone says hi").is_none());
        assert!(d.is_secret("passkey") && d.is_secret("auth.password") && !d.is_secret("nick"));
    }

    #[test]
    fn reads_a_multi_line_announce() {
        let d = Arc::new(Definition::parse(MULTI).unwrap());
        d.self_test().unwrap();
        let mut r = LineReader::new(d.clone());
        assert!(r.feed("Name: Old.Release-X").is_none());
        // a new first line starts over
        assert!(r.feed("Name: Movie.2024.2160p.BluRay-GRP").is_none());
        assert!(r.feed("Size: 2048 Tags: hdr, action").is_none());
        let vars = r.feed("Link: https://harbour.example/t/9").expect("three lines make an announce");
        let f = found(&d, vars, &BTreeMap::new()).unwrap();
        assert_eq!(f.title, "Movie.2024.2160p.BluRay-GRP");
        assert_eq!(f.size, Some(2_048_000_000));
        assert_eq!(f.tags, vec!["hdr", "action"]);
        assert_eq!(f.download_url, "https://harbour.example/get/9");
    }

    #[test]
    fn strips_irc_formatting() {
        assert_eq!(strip_formatting("\x0304,01red\x03 \x02bold\x02 \x1funder\x1f 5,5"), "red bold under 5,5");
    }

    #[test]
    fn the_library_reads_a_folder_and_names_bad_files() {
        let dir = std::env::temp_dir().join(format!("announce-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lighthouse.yaml"), SINGLE).unwrap();
        std::fs::write(dir.join("broken.yaml"), "identifier: [").unwrap();
        let lib = Library::new(vec![dir.clone()]);
        assert!(lib.get("lighthouse").is_some());
        assert_eq!(lib.errors().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
