// Ported from autobrr v1.90.0 (internal/domain/indexer.go, irc_parser.go, irc.go IRCAuth parts;
// internal/indexer/service.go OpenAndProcessDefinition, deprecations.go), GPL-2.0-or-later.
//! autobrr's indexer definitions: per tracker, its site settings, its IRC network with the announce channels and
//! how to read their lines, or the feed settings of a Torznab, Newznab or RSS indexer. The files are autobrr's own
//! (`internal/indexer/definitions/*.yaml`, bundled in the image; custom ones in the data folder's `announce`), read
//! in both of autobrr's formats: version 2 (a parse per channel) and the older one (one parse for every channel).
//!
//! An announce becomes a release here: [`ParseLine::parse_line`] reads a line's values, [`Parse::parse`] maps them
//! onto the release, builds its download and info links from the definition's templates and runs the tracker's
//! own reader (GazelleGames, Orpheus and Redacted name their releases their own way).

use super::gotemplate::{Template, Tv};
use super::release::Release;
use super::releasetags::{clean_release_tags, parse_release_tag_string};
use super::rls;
use regex::Regex;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

/// autobrr's stand-in for a secret the API doesn't send back.
pub const REDACTED: &str = "<redacted>";

/// `s` as the API sends a secret: [`REDACTED`], or empty when there is none (autobrr's `RedactString`).
pub fn redact(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        REDACTED.to_string()
    }
}

/// Whether `s` is the redacted stand-in (autobrr's `IsRedactedString`).
pub fn is_redacted(s: &str) -> bool {
    !s.is_empty() && s == REDACTED
}

/// The indexer settings that hold a credential (autobrr's `secretSettingKeys`).
const SECRET_SETTING_KEYS: [&str; 13] = [
    "rsskey",
    "rss_key",
    "passkey",
    "authkey",
    "torrentpass",
    "torrent_pass",
    "api_key",
    "apikey",
    "uid",
    "userid",
    "key",
    "token",
    "cookie",
];

/// Whether indexer setting `key` holds a credential (autobrr's `IsSecretIndexerSetting`).
pub fn is_secret_indexer_setting(key: &str) -> bool {
    SECRET_SETTING_KEYS.contains(&key.to_lowercase().as_str())
}

/// An empty list sent as `null`, as Go sends a nil slice.
fn nil_vec<S: Serializer, T: Serialize>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        s.serialize_none()
    } else {
        v.serialize(s)
    }
}

/// An empty map sent as `null`, as Go sends a nil map.
fn nil_map<S: Serializer, K: Serialize, V: Serialize>(m: &BTreeMap<K, V>, s: S) -> Result<S::Ok, S::Error> {
    if m.is_empty() {
        s.serialize_none()
    } else {
        m.serialize(s)
    }
}

/// How an IRC network signs in (autobrr's `IRCAuthMechanism`).
pub const AUTH_NONE: &str = "NONE";
/// SASL PLAIN while connecting.
pub const AUTH_SASL_PLAIN: &str = "SASL_PLAIN";
/// NickServ IDENTIFY once connected.
pub const AUTH_NICKSERV: &str = "NICKSERV";

/// Whether `m` is one of the sign-in kinds autobrr knows.
pub fn auth_mechanism_is_valid(m: &str) -> bool {
    [AUTH_NONE, AUTH_SASL_PLAIN, AUTH_NICKSERV].contains(&m)
}

/// The sign-in an IRC network of a definition uses (autobrr's `IndexerIRCAuth`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IrcAuthDef {
    /// `NONE`, `SASL_PLAIN` or `NICKSERV`.
    #[serde(default)]
    pub mechanism: String,
}

impl IrcAuthDef {
    /// An unknown or missing mechanism, in autobrr's words.
    pub fn validate(&self) -> Result<(), String> {
        if !auth_mechanism_is_valid(&self.mechanism) {
            return Err(format!("invalid IRC authentication mechanism: {:?}", self.mechanism));
        }
        Ok(())
    }
}

/// One setting a definition asks for (autobrr's `IndexerSetting`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IndexerSetting {
    /// Its name (`passkey`, `auth.password`).
    pub name: String,
    /// Must be filled.
    pub required: bool,
    /// `text` or `secret`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The saved value (in a mapped definition).
    pub value: String,
    /// What people see.
    pub label: String,
    /// Its value when not filled.
    pub default: String,
    /// What it is.
    pub description: String,
    /// A hint under the field.
    pub help: String,
    /// A pattern the value must match.
    pub regex: String,
}

impl Serialize for IndexerSetting {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("name", &self.name)?;
        if self.required {
            m.serialize_entry("required", &true)?;
        }
        m.serialize_entry("type", &self.kind)?;
        let value = if self.kind.eq_ignore_ascii_case("secret") { redact(&self.value) } else { self.value.clone() };
        if !value.is_empty() {
            m.serialize_entry("value", &value)?;
        }
        m.serialize_entry("label", &self.label)?;
        for (k, v) in [("default", &self.default), ("description", &self.description), ("help", &self.help), ("regex", &self.regex)] {
            if !v.is_empty() {
                m.serialize_entry(k, v)?;
            }
        }
        m.end()
    }
}

/// The settings of a Torznab, Newznab or RSS indexer (autobrr's `FeedSettings`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FeedSettings {
    /// The shortest interval between looks, in minutes.
    #[serde(rename(serialize = "minInterval", deserialize = "mininterval"), default)]
    pub min_interval: i64,
    /// The settings it asks for.
    #[serde(default, serialize_with = "nil_vec")]
    pub settings: Vec<IndexerSetting>,
}

/// An example line and the values it must give (autobrr's `LineTest`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineTest {
    /// The line.
    pub line: String,
    /// The values.
    #[serde(serialize_with = "nil_map")]
    pub expect: BTreeMap<String, String>,
}

/// One line pattern (autobrr's `IndexerIRCParseLine`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ParseLine {
    /// The regular expression (Go's syntax).
    pub pattern: String,
    /// The names of its groups, in order (none: the pattern's named groups).
    #[serde(serialize_with = "nil_vec")]
    pub vars: Vec<String>,
    /// The line only has to be there; nothing is read from it.
    pub ignore: bool,
    /// Example lines.
    #[serde(serialize_with = "nil_vec")]
    pub tests: Vec<LineTest>,
}

impl PartialEq for ParseLine {
    fn eq(&self, o: &ParseLine) -> bool {
        self.pattern == o.pattern && self.vars == o.vars && self.ignore == o.ignore && self.tests == o.tests
    }
}

/// Compiled patterns, by their Go source (autobrr's `regexcache`).
fn compiled(pattern: &str) -> Option<Regex> {
    /// The patterns compiled so far (`None`: Go would refuse it).
    static CACHE: OnceLock<RwLock<BTreeMap<String, Option<Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(re) = cache.read().unwrap_or_else(|e| e.into_inner()).get(pattern) {
        return re.clone();
    }
    let re = rls::goregex::try_compile(pattern);
    cache.write().unwrap_or_else(|e| e.into_inner()).insert(pattern.to_string(), re.clone());
    re
}

impl ParseLine {
    /// Reads `line` into `vars`: whether it matched, or why it couldn't be read (autobrr's `ParseLine`).
    pub fn parse_line(&self, vars: &mut BTreeMap<String, String>, line: &str, ignore: bool) -> Result<bool, String> {
        // an explicit vars list takes precedence and keeps the positional mapping
        if !self.vars.is_empty() {
            let Some(re) = compiled(&self.pattern) else {
                return Err(format!("could not match line: {line}: unexpected line"));
            };
            let Some(c) = re.captures(line) else { return Ok(false) };
            let groups = c.len() - 1;
            for (i, v) in self.vars.iter().enumerate() {
                if i + 1 > groups {
                    return Err("too few matches returned for rxp".into());
                }
                vars.insert(v.clone(), c.get(i + 1).map(|m| m.as_str().to_string()).unwrap_or_default());
            }
            return Ok(true);
        }
        if let Some(re) = compiled(&self.pattern) {
            if re.capture_names().any(|n| n.is_some()) {
                let Some(c) = re.captures(line) else { return Ok(false) };
                for name in re.capture_names().flatten() {
                    vars.insert(name.to_string(), c.name(name).map(|m| m.as_str().to_string()).unwrap_or_default());
                }
                return Ok(true);
            }
        }
        // a pattern without vars or named groups: every group as `raw`
        let Some(re) = compiled(&format!("(?mi){}", self.pattern)) else {
            return Err(format!("could not match line: {line}: unexpected line"));
        };
        let names: Vec<Option<&str>> = re.capture_names().collect();
        for c in re.captures_iter(line) {
            for (i, g) in c.iter().enumerate() {
                if ignore {
                    return Ok(true);
                }
                let name = names.get(i).copied().flatten().filter(|n| !n.is_empty()).unwrap_or("raw");
                vars.insert(name.to_string(), g.map(|m| m.as_str().to_string()).unwrap_or_default());
            }
        }
        Ok(true)
    }
}

/// How a read announce becomes links and a name (autobrr's `IndexerIRCV2ParseMatch`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParseMatch {
    /// The download link template.
    #[serde(rename(serialize = "downloadurl", deserialize = "downloadurl"))]
    pub download_url: String,
    /// The release name template (empty: the announced name).
    #[serde(rename(serialize = "releaseName", deserialize = "releasename"))]
    pub release_name: String,
    /// The magnet link template.
    #[serde(rename(serialize = "magnetURI", deserialize = "magneturi"))]
    pub magnet_uri: String,
    /// The release page template.
    #[serde(rename(serialize = "infoURL", deserialize = "infourl"))]
    pub info_url: String,
    /// Values URL-encoded before the templates see them.
    #[serde(serialize_with = "nil_vec")]
    pub encode: Vec<String>,
}

/// Which tracker's own reader runs after the values are mapped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Parser {
    /// The release name read as it is.
    #[default]
    Default,
    /// GazelleGames.
    GazelleGames,
    /// Orpheus.
    Orpheus,
    /// Redacted.
    Redacted,
}

impl Parser {
    /// The reader of the tracker `identifier`.
    pub fn of(identifier: &str) -> Parser {
        match identifier {
            "ggn" => Parser::GazelleGames,
            "ops" => Parser::Orpheus,
            "redacted" => Parser::Redacted,
            _ => Parser::Default,
        }
    }
}

/// Values turned into others: var → its value → (var → new value).
pub type Mappings = BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>;

/// How to read one channel's announces (autobrr's `IndexerIRCV2Parse`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parse {
    /// `single` or `multi` (the lines together, in order).
    #[serde(rename = "type")]
    pub kind: String,
    /// The unit of a size announced without one.
    pub forcesizeunit: String,
    /// `binary` when the tracker means 1024 by KB, MB, GB.
    pub sizeunits: String,
    /// Keep IRC colours and formatting in the lines.
    pub skipcleanmessage: bool,
    /// The line patterns, in order.
    #[serde(serialize_with = "nil_vec")]
    pub lines: Vec<ParseLine>,
    /// The links and name.
    #[serde(rename = "match")]
    pub matcher: ParseMatch,
    /// Values turned into others.
    #[serde(serialize_with = "nil_map")]
    pub mappings: Mappings,
    /// The tracker's own reader.
    #[serde(skip)]
    pub parser: Parser,
}

/// The size units autobrr knows: the decimal default or `binary`.
pub fn validate_size_units(s: &str) -> Result<(), String> {
    match s {
        "" | "binary" => Ok(()),
        _ => Err(format!("invalid size units: {s:?}")),
    }
}

/// One announce channel of a definition (autobrr's `IndexerIRCV2Channel`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IrcChannelDef {
    /// Its name (`#announce`).
    pub name: String,
    /// The nicks that announce.
    #[serde(serialize_with = "nil_vec")]
    pub announcers: Vec<String>,
    /// How to read its lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse: Option<Parse>,
}

/// A definition's IRC side (autobrr's `IndexerIRCV2`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IrcDef {
    /// The network's name.
    pub network: String,
    /// The server.
    pub server: String,
    /// The port.
    pub port: i64,
    /// TLS.
    pub tls: bool,
    /// The sign-in it uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<IrcAuthDef>,
    /// The saved IRC settings, by name.
    #[serde(skip)]
    pub settings_map: BTreeMap<String, String>,
    /// The IRC settings it asks for.
    #[serde(serialize_with = "nil_vec")]
    pub settings: Vec<IndexerSetting>,
    /// The announce channels.
    #[serde(serialize_with = "nil_vec")]
    pub channels: Vec<IrcChannelDef>,
}

impl IrcDef {
    /// Channel `name`, in any case (autobrr's `GetChannel`).
    pub fn channel(&self, name: &str) -> Option<&IrcChannelDef> {
        let name = name.to_lowercase();
        self.channels.iter().find(|c| c.name.to_lowercase() == name)
    }
}

/// One tracker's definition (autobrr's `IndexerDefinition`), with the indexer's own values once mapped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexerDefinition {
    /// The format's version.
    #[serde(skip_serializing)]
    pub version: i64,
    /// The indexer's id once mapped.
    #[serde(skip_serializing_if = "is_zero")]
    pub id: i64,
    /// The name.
    pub name: String,
    /// The short name (`redacted`).
    pub identifier: String,
    /// The short name elsewhere.
    #[serde(rename(serialize = "identifier_external", deserialize = "identifierexternal"))]
    pub identifier_external: String,
    /// `irc`, `torznab`, `newznab` or `rss`.
    pub implementation: String,
    /// The site's address the links are built on (the indexer's own, else the first of `urls`).
    #[serde(rename(serialize = "base_url", deserialize = "baseurl"), skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    /// The indexer is on.
    pub enabled: bool,
    /// What it is.
    pub description: String,
    /// Its language.
    pub language: String,
    /// `private`, `public` …
    pub privacy: String,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// The site's addresses.
    #[serde(serialize_with = "nil_vec")]
    pub urls: Vec<String>,
    /// What it supports (`irc`, `rss`, `api`).
    #[serde(serialize_with = "nil_vec")]
    pub supports: Vec<String>,
    /// Through a proxy.
    #[serde(rename(serialize = "use_proxy", deserialize = "useproxy"))]
    pub use_proxy: bool,
    /// The proxy.
    #[serde(rename(serialize = "proxy_id", deserialize = "proxyid"))]
    pub proxy_id: i64,
    /// The site settings it asks for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<IndexerSetting>,
    /// The saved site settings, by name.
    #[serde(skip)]
    pub settings_map: BTreeMap<String, String>,
    /// The IRC side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub irc: Option<IrcDef>,
    /// The feed side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed: Option<FeedSettings>,
}

/// Whether a number is 0 (for `omitempty`).
fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl IndexerDefinition {
    /// The implementation as autobrr settles it (an empty one is IRC) and each channel's reader (autobrr's
    /// `Prepare`).
    pub fn prepare(&mut self) {
        if self.implementation.is_empty() {
            self.implementation = "irc".into();
        }
        let parser = Parser::of(&self.identifier);
        if let Some(irc) = self.irc.as_mut() {
            for c in irc.channels.iter_mut() {
                if let Some(p) = c.parse.as_mut() {
                    p.parser = parser;
                }
            }
        }
    }

    /// Whether the site has an API autobrr asks (`supports: [api]`).
    pub fn has_api(&self) -> bool {
        self.supports.iter().any(|s| s == "api")
    }

    /// An invalid IRC sign-in or size unit, in autobrr's words (autobrr's `ValidateIRC`).
    pub fn validate_irc(&self) -> Result<(), String> {
        let Some(irc) = &self.irc else { return Ok(()) };
        if let Some(a) = &irc.auth {
            a.validate()?;
        }
        for c in &irc.channels {
            if let Some(p) = &c.parse {
                validate_size_units(&p.sizeunits).map_err(|e| format!("invalid parse for channel: {}: {e}", c.name))?;
            }
        }
        Ok(())
    }

    /// Whether the indexer reads feeds (Torznab, Newznab or RSS).
    pub fn is_feed(&self) -> bool {
        is_feed_implementation(&self.implementation)
    }

    /// The site setting or IRC setting `name`, as declared.
    pub fn setting(&self, name: &str) -> Option<&IndexerSetting> {
        self.settings.iter().chain(self.irc.iter().flat_map(|i| i.settings.iter())).find(|s| s.name == name)
    }
}

/// Whether implementation `i` reads feeds.
pub fn is_feed_implementation(i: &str) -> bool {
    matches!(i, "rss" | "torznab" | "newznab")
}

// ---- the older format (autobrr's IndexerDefinitionCustom) ----

/// The older format's links (autobrr's `IndexerIRCParseMatch`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct V1Match {
    torrenturl: String,
    torrentname: String,
    magneturi: String,
    infourl: String,
    encode: Vec<String>,
}

/// The older format's parse (autobrr's `IndexerIRCParse`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct V1Parse {
    #[serde(rename = "type")]
    kind: String,
    forcesizeunit: String,
    sizeunits: String,
    lines: Vec<ParseLine>,
    #[serde(rename = "match")]
    matcher: V1Match,
    mappings: Mappings,
}

/// The older format's IRC side (autobrr's `IndexerIRC`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct V1Irc {
    network: String,
    server: String,
    port: i64,
    tls: bool,
    auth: Option<IrcAuthDef>,
    channels: Vec<String>,
    announcers: Vec<String>,
    settings: Vec<IndexerSetting>,
    parse: Option<V1Parse>,
}

/// The older format (autobrr's `IndexerDefinitionCustom`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct V1Definition {
    id: i64,
    name: String,
    identifier: String,
    implementation: String,
    baseurl: String,
    enabled: bool,
    description: String,
    language: String,
    privacy: String,
    protocol: String,
    urls: Vec<String>,
    supports: Vec<String>,
    settings: Vec<IndexerSetting>,
    irc: Option<V1Irc>,
    feed: Option<FeedSettings>,
    parse: Option<V1Parse>,
}

impl V1Definition {
    /// In the current format (autobrr's `ToIndexerDefinition`).
    fn upgrade(mut self) -> IndexerDefinition {
        if self.implementation.is_empty() {
            self.implementation = "irc".into();
        }
        let parser = Parser::of(&self.identifier);
        let top_parse = self.parse.take();
        let irc = self.irc.take().map(|mut i| {
            if top_parse.is_some() {
                i.parse = top_parse.clone();
            }
            let p = i.parse.clone().unwrap_or_default();
            IrcDef {
                network: i.network,
                server: i.server,
                port: i.port,
                tls: i.tls,
                auth: i.auth,
                settings_map: BTreeMap::new(),
                settings: i.settings,
                channels: i
                    .channels
                    .iter()
                    .map(|name| IrcChannelDef {
                        name: name.clone(),
                        announcers: i.announcers.clone(),
                        parse: Some(Parse {
                            kind: p.kind.clone(),
                            forcesizeunit: p.forcesizeunit.clone(),
                            sizeunits: p.sizeunits.clone(),
                            skipcleanmessage: false,
                            lines: p.lines.clone(),
                            matcher: ParseMatch {
                                download_url: p.matcher.torrenturl.clone(),
                                release_name: p.matcher.torrentname.clone(),
                                magnet_uri: p.matcher.magneturi.clone(),
                                info_url: p.matcher.infourl.clone(),
                                encode: p.matcher.encode.clone(),
                            },
                            mappings: p.mappings.clone(),
                            parser,
                        }),
                    })
                    .collect(),
            }
        });
        IndexerDefinition {
            id: self.id,
            name: self.name,
            identifier: self.identifier,
            implementation: self.implementation,
            base_url: self.baseurl,
            enabled: self.enabled,
            description: self.description,
            language: self.language,
            privacy: self.privacy,
            protocol: self.protocol,
            urls: self.urls,
            supports: self.supports,
            settings: self.settings,
            irc,
            feed: self.feed,
            ..IndexerDefinition::default()
        }
    }
}

/// Reads a definition file's text in either format (autobrr's `OpenAndProcessDefinition`); the problem in
/// autobrr's words.
pub fn parse_definition(text: &str) -> Result<IndexerDefinition, String> {
    /// The few fields read before the whole file: which format version it is in.
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Meta {
        version: i64,
    }
    let meta: Meta = serde_yaml::from_str(text).map_err(|e| format!("could not detect definition version: {e}"))?;
    if meta.version >= 2 {
        let mut d: IndexerDefinition = serde_yaml::from_str(text).map_err(|e| format!("could not decode definition file: {e}"))?;
        d.validate_irc().map_err(|e| format!("invalid definition file: {e}"))?;
        d.prepare();
        return Ok(d);
    }
    let d: Option<V1Definition> = serde_yaml::from_str(text).map_err(|e| format!("could not decode definition file: {e}"))?;
    let d = d.ok_or("empty definition file")?.upgrade();
    d.validate_irc().map_err(|e| format!("invalid definition file: {e}"))?;
    Ok(d)
}

// ---- links and names (autobrr's ParseURLs, ParseReleaseName, Parse) ----

/// Go's `url.QueryEscape`: letters, digits and `-_.~` stay, a space is `+`, everything else `%XX`.
pub fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Go's path escaping (`url.PathEscape` for a whole path: `/` stays).
fn path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'@' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Go's path unescaping: `%XX` decoded (`+` stays); `None` for a broken escape.
fn path_unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Go's `path.Clean`.
fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// Go's `path.Join`.
fn join_path(elems: &[&str]) -> String {
    let parts: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    clean_path(&parts.join("/"))
}

/// A URL in parts: scheme, authority (`//host`, without the slashes), path, query.
#[derive(Debug, Default)]
struct UrlParts {
    scheme: String,
    host: String,
    path: String,
    query: Option<String>,
}

/// Splits `s` as Go's `url.Parse` would, enough for announce links; the path decoded.
fn split_url(s: &str) -> Option<UrlParts> {
    let s = s.split('#').next().unwrap_or("");
    let (rest, query) = match s.split_once('?') {
        Some((r, q)) => (r, Some(q.to_string())),
        None => (s, None),
    };
    let mut u = UrlParts { query, ..UrlParts::default() };
    let mut rest = rest;
    if let Some(i) = rest.find(':') {
        let scheme = &rest[..i];
        if !scheme.is_empty()
            && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            u.scheme = scheme.to_lowercase();
            rest = &rest[i + 1..];
        }
    }
    if let Some(r) = rest.strip_prefix("//") {
        let end = r.find('/').unwrap_or(r.len());
        u.host = r[..end].to_string();
        rest = &r[end..];
    }
    u.path = path_unescape(rest)?;
    Some(u)
}

/// Renders template `src` (named `name`) with `vars`; autobrr's error words.
fn render(name: &str, src: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    let t = Template::parse(name, src).map_err(|_| format!("could not create {name} template"))?;
    let data = Tv::Map(vars.iter().map(|(k, v)| (k.clone(), Tv::Str(v.clone()))).collect());
    t.execute(&data).map_err(|_| format!("could not write {name} url template output"))
}

/// The link template `src` rendered and put on `base_url`: the rendered link's own scheme and host are dropped
/// and its path joined to the base's, its query kept (autobrr's `parseTemplateURL`).
pub fn parse_template_url(base_url: &str, src: &str, vars: &BTreeMap<String, String>, name: &str) -> Result<String, String> {
    let out = render(name, src, vars)?;
    let parsed = split_url(&out).ok_or_else(|| format!("could not parse template url: {out:?}"))?;
    let base = split_url(base_url).ok_or_else(|| format!("could not join {name} url"))?;
    // url.JoinPath(base, path): the base's escaped path, then the path, joined and cleaned
    let base_path = path_escape(&base.path);
    let mut p = if base_path.starts_with('/') {
        join_path(&[&base_path, &parsed.path])
    } else {
        let first = format!("/{base_path}");
        join_path(&[&first, &parsed.path]).get(1..).unwrap_or("").to_string()
    };
    let last = if parsed.path.is_empty() { &base_path } else { &parsed.path };
    if last.ends_with('/') && !p.ends_with('/') {
        p.push('/');
    }
    let p = path_unescape(&p).unwrap_or(p);
    let mut s = String::new();
    if !base.scheme.is_empty() {
        s.push_str(&base.scheme);
        s.push(':');
    }
    if !base.host.is_empty() || !p.is_empty() {
        s.push_str("//");
        s.push_str(&base.host);
    }
    if !p.is_empty() && !p.starts_with('/') && !base.host.is_empty() {
        s.push('/');
    }
    s.push_str(&path_escape(&p));
    if let Some(q) = parsed.query.filter(|q| !q.is_empty()) {
        s.push('?');
        s.push_str(&q);
    }
    Ok(s)
}

impl ParseMatch {
    /// The release's info, download and magnet links (autobrr's `ParseURLs`); `vars` gets the encoded values.
    pub fn parse_urls(&self, base_url: &str, vars: &mut BTreeMap<String, String>, rls: &mut Release) -> Result<(), String> {
        for e in &self.encode {
            if let Some(v) = vars.get(e).cloned() {
                vars.insert(e.clone(), query_escape(&v));
            }
        }
        if !self.info_url.is_empty() {
            rls.info_url = parse_template_url(base_url, &self.info_url, vars, "infourl")?;
        }
        if !self.download_url.is_empty() {
            rls.download_url = parse_template_url(base_url, &self.download_url, vars, "downloadurl")?;
        }
        if !self.magnet_uri.is_empty() {
            rls.magnet_uri = parse_template_url("magnet:", &self.magnet_uri, vars, "magneturi")?;
        }
        Ok(())
    }

    /// The release name from the template, when the definition has one (autobrr's `ParseReleaseName`).
    pub fn parse_release_name(&self, vars: &BTreeMap<String, String>, rls: &mut Release) -> Result<(), String> {
        if !self.release_name.is_empty() {
            let t = Template::parse("releasename", &self.release_name).map_err(|_| "could not create releasename template".to_string())?;
            let data = Tv::Map(vars.iter().map(|(k, v)| (k.clone(), Tv::Str(v.clone()))).collect());
            rls.torrent_name = t.execute(&data).map_err(|_| "could not write torrent name template output".to_string())?;
        }
        Ok(())
    }
}

impl Parse {
    /// Applies the mappings: a value that has one adds its values (autobrr's `MapCustomVariables`).
    pub fn map_custom_variables(&self, vars: &mut BTreeMap<String, String>) {
        for (key, by_value) in &self.mappings {
            let Some(v) = vars.get(key) else { continue };
            let Some(set) = by_value.get(v).cloned() else { continue };
            vars.extend(set);
        }
    }

    /// Fills `rls` from an announce's values `vars` read on channel `channel` of `def` (autobrr's `Parse`).
    pub fn parse(
        &self,
        def: &IndexerDefinition,
        channel: &str,
        vars: &mut BTreeMap<String, String>,
        rls: &mut Release,
    ) -> Result<(), String> {
        let ch = def
            .irc
            .as_ref()
            .and_then(|i| i.channel(channel))
            .and_then(|c| c.parse.as_ref())
            .ok_or_else(|| format!("could not find channel: {channel}"))?;
        self.map_custom_variables(vars);
        rls.map_vars(vars, &ch.forcesizeunit, &ch.sizeunits).map_err(|e| format!("could not map variables for release: {e}"))?;
        let mut merged = vars.clone();
        merged.extend(def.settings_map.iter().map(|(k, v)| (k.clone(), v.clone())));
        let base_url = if def.base_url.is_empty() {
            def.urls.first().cloned().ok_or("could not find a valid indexer baseUrl")?
        } else {
            def.base_url.clone()
        };
        ch.matcher.parse_urls(&base_url, &mut merged, rls).map_err(|e| format!("could not parse urls for release: {e}"))?;
        self.matcher.parse_release_name(&merged, rls).map_err(|e| format!("could not parse release name: {e}"))?;
        run_parser(self.parser, rls, vars).map_err(|e| format!("could not parse release: {e}"))?;
        if let Some(c) = def.settings_map.get("cookie") {
            rls.raw_cookie = c.clone();
        }
        Ok(())
    }
}

// ---- the trackers' own readers (autobrr's irc_parser.go) ----

/// Runs reader `p` on `rls` with the announce's values.
pub fn run_parser(p: Parser, rls: &mut Release, vars: &BTreeMap<String, String>) -> Result<(), String> {
    match p {
        Parser::Default => {
            let name = rls.torrent_name.clone();
            rls.parse_string(&name);
            Ok(())
        }
        Parser::GazelleGames => parse_ggn(rls, vars),
        Parser::Orpheus => parse_music(rls, vars, true),
        Parser::Redacted => parse_music(rls, vars, false),
    }
}

/// The named groups of `re` in `text` (autobrr's `GetNamedGroups`); `None` without a match.
fn named_groups(re: &Regex, text: &str) -> Option<BTreeMap<String, String>> {
    let c = re.captures(text)?;
    Some(re.capture_names().flatten().map(|n| (n.to_string(), c.name(n).map(|m| m.as_str().to_string()).unwrap_or_default())).collect())
}

/// GazelleGames' iOS names.
fn ggn_ios() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| rls::goregex::compile(r"^(?P<releaseName>.+?)\s*-\s*Version\s+(?P<version>[a-z0-9.-]+)(?:\s+in\s+(?P<title>.+))?$"))
}

/// GazelleGames' Switch and Windows names.
fn ggn_switch_windows() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        rls::goregex::compile(
            r"^(?P<releaseName>.+?)(?:\s*-\s*(?P<update>Update))?(?:\s*-\s*(?P<version>Version\s.+))?\s+in\s+(?P<title>.+)$",
        )
    })
}

/// GazelleGames' Windows names without a title.
fn ggn_windows_fallback() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| rls::goregex::compile(r"^(?P<releaseName>.+?)(?:\s*-\s*(?P<version>Version\s.+))?$"))
}

/// `s` split at the middle `sep` (GazelleGames' `Name in Title`, where titles may say "in" too).
pub fn split_in_middle(s: &str, sep: &str) -> (String, String) {
    if s.is_empty() {
        return (String::new(), String::new());
    }
    let parts: Vec<&str> = s.split(sep).collect();
    if parts.len() == 1 {
        return (s.to_string(), String::new());
    }
    let mid = parts.len() / 2;
    (parts[..mid].join(sep), parts[mid..].join(sep))
}

/// GazelleGames: the release name and the game's title from `Name in Title` (autobrr's `IRCParserGazelleGames`).
fn parse_ggn(rls: &mut Release, vars: &BTreeMap<String, String>) -> Result<(), String> {
    let torrent_name = vars.get("releaseName").ok_or("ggn irc parser: releaseName is missing from vars")?;
    let category = vars.get("category").map(String::as_str).unwrap_or("");
    let (release_name, title) = match category {
        "OST" => (torrent_name.clone(), String::new()),
        "Switch" | "Windows" => match named_groups(ggn_switch_windows(), torrent_name) {
            Some(g) if !g.is_empty() => (g["releaseName"].clone(), g["title"].clone()),
            _ => {
                let g = named_groups(ggn_windows_fallback(), torrent_name)
                    .ok_or_else(|| format!("failed to parse Switch/Windows torrentName: {torrent_name}"))?;
                let name = g["releaseName"].clone();
                let title = if name.is_empty() { String::new() } else { name.clone() };
                (name, title)
            }
        },
        "iOS" => {
            let g = named_groups(ggn_ios(), torrent_name).ok_or_else(|| format!("failed to parse iOS torrentName: {torrent_name}"))?;
            (g["releaseName"].clone(), g["title"].clone())
        }
        _ => {
            let (mut name, title) = split_in_middle(torrent_name, " in ");
            if name.is_empty() && !title.is_empty() {
                name = torrent_name.clone();
            }
            (name, title)
        }
    };
    rls.parse_string(&release_name);
    if !title.is_empty() {
        rls.title = title;
    }
    Ok(())
}

/// Orpheus and Redacted: a release name built from the title, year and release tags, so the tags read the same
/// every time (autobrr's `IRCParserOrpheus`, `IRCParserRedacted`). Orpheus' en dashes become hyphens and a bare
/// last log score gets its `%`.
fn parse_music(rls: &mut Release, vars: &BTreeMap<String, String>, orpheus: bool) -> Result<(), String> {
    let get = |k: &str| vars.get(k).cloned().unwrap_or_default();
    let mut title = get("title");
    let year = get("year");
    let mut tags_string = get("releaseTags");
    if orpheus {
        title = title.replace('–', "-");
        let mut split: Vec<String> = tags_string.split('/').map(str::to_string).collect();
        if let Some(last) = split.last_mut() {
            let is_score = (last.len() <= 2 && !last.is_empty() && last.chars().all(|c| c.is_ascii_digit())) || last == "100";
            if is_score {
                last.push('%');
            }
        }
        tags_string = split.join(" ");
    }
    let clean = clean_release_tags(&tags_string);
    let tags = parse_release_tag_string(&clean);
    if orpheus {
        rls.release_tags = clean.clone();
    }
    let mut audio: Vec<String> = Vec::new();
    for t in [&tags.source, &tags.audio_format, &tags.audio_bitrate] {
        if !t.is_empty() {
            audio.push(t.clone());
        }
    }
    rls.bitrate = tags.audio_bitrate.clone();
    rls.audio_format = tags.audio_format.clone();
    rls.has_log = tags.has_log;
    rls.log_score = tags.log_score;
    rls.has_cue = tags.has_cue;
    let name = format!("{title} [{year}] ({})", audio.join(" "));
    rls.parse_string(&name);
    let raw = rls::parse_string(&name);
    rls.artists = raw.artist.clone();
    rls.title = raw.title.clone();
    Ok(())
}

// ---- deprecated indexers (autobrr's deprecations.go) ----

/// An indexer whose definition was removed (autobrr's `IndexerDeprecation`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexerDeprecation {
    /// Its identifier.
    pub identifier: String,
    /// Its name.
    pub name: String,
    /// Why it went.
    pub reason: String,
    /// Where it was decided.
    pub issue_url: String,
    /// The identifier it lives on as, if any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias_of: String,
    /// When.
    pub deprecated_at: chrono::DateTime<chrono::Utc>,
    /// How many filters still use it.
    #[serde(default, skip_deserializing)]
    pub filter_count: i64,
}

/// Reads one tombstone file named `file` (autobrr's `validateDeprecation`); the problem in autobrr's words.
pub fn parse_deprecation(file: &str, text: &str) -> Result<IndexerDeprecation, String> {
    let d: IndexerDeprecation =
        serde_yaml::from_str(text).map_err(|e| format!("could not decode deprecated indexer definition: {file}: {e}"))?;
    if d.identifier.is_empty() {
        return Err(format!("deprecated indexer definition {file} has no identifier"));
    }
    if file != format!("{}.yaml", d.identifier) {
        return Err(format!("deprecated indexer definition {file} must be named {}.yaml", d.identifier));
    }
    if d.name.is_empty() {
        return Err(format!("deprecated indexer {} has no name", d.identifier));
    }
    if d.reason.is_empty() {
        return Err(format!("deprecated indexer {} has no reason", d.identifier));
    }
    let ok = url::Url::parse(&d.issue_url).is_ok_and(|u| !u.scheme().is_empty() && u.host_str().is_some_and(|h| !h.is_empty()));
    if !ok {
        return Err(format!("deprecated indexer {} has an invalid issue URL", d.identifier));
    }
    Ok(d)
}

// ---- the library ----

/// The definitions on disk: the bundled ones, then the data folder's (`<data>/announce`), which win; and the
/// bundled tombstones of removed indexers (`deprecated/`).
#[derive(Clone, Default)]
pub struct Library {
    dirs: Vec<PathBuf>,
    defs: Arc<RwLock<BTreeMap<String, Arc<IndexerDefinition>>>>,
    deprecations: Arc<RwLock<Vec<IndexerDeprecation>>>,
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
        let mut deps: BTreeMap<String, IndexerDeprecation> = BTreeMap::new();
        for dir in &self.dirs {
            read_dir(dir, &mut defs, &mut errors);
            for (name, text) in yaml_files(&dir.join("deprecated")) {
                match parse_deprecation(&name, &text) {
                    Ok(d) => {
                        deps.insert(d.identifier.clone(), d);
                    }
                    Err(e) => errors.push(e),
                }
            }
        }
        *self.defs.write().unwrap_or_else(|e| e.into_inner()) = defs;
        *self.deprecations.write().unwrap_or_else(|e| e.into_inner()) = deps.into_values().collect();
        *self.errors.write().unwrap_or_else(|e| e.into_inner()) = errors;
    }

    /// Adds one definition (tests, and definitions made elsewhere).
    pub fn insert(&self, mut def: IndexerDefinition) {
        def.prepare();
        self.defs.write().unwrap_or_else(|e| e.into_inner()).insert(def.identifier.clone(), Arc::new(def));
    }

    /// Adds a definition from its text; the problem in words.
    pub fn insert_text(&self, text: &str) -> Result<(), String> {
        let d = parse_definition(text)?;
        self.defs.write().unwrap_or_else(|e| e.into_inner()).insert(d.identifier.clone(), Arc::new(d));
        Ok(())
    }

    /// Definition `id`.
    pub fn get(&self, id: &str) -> Option<Arc<IndexerDefinition>> {
        self.defs.read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    /// Every definition, by id.
    pub fn all(&self) -> Vec<Arc<IndexerDefinition>> {
        self.defs.read().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    /// The tombstones of removed indexers, by identifier.
    pub fn deprecations(&self) -> Vec<IndexerDeprecation> {
        self.deprecations.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The files that couldn't be read, in words.
    pub fn errors(&self) -> Vec<String> {
        self.errors.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The `.yaml`/`.yml` files of `dir`, by name, in name order.
fn yaml_files(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "yaml" || x == "yml")).collect();
    paths.sort();
    paths.into_iter().filter_map(|p| Some((p.file_name()?.to_string_lossy().into_owned(), std::fs::read_to_string(&p).ok()?))).collect()
}

/// Reads every definition file of `dir` into `defs`.
fn read_dir(dir: &Path, defs: &mut BTreeMap<String, Arc<IndexerDefinition>>, errors: &mut Vec<String>) {
    for (name, text) in yaml_files(dir) {
        match parse_definition(&text) {
            Ok(d) => {
                defs.insert(d.identifier.clone(), Arc::new(d));
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // internal/domain/indexer_test.go TestIndexerIRCV2ParseMatch_ParseUrls
    #[test]
    fn test_indexer_ircv2_parse_match_parse_urls() {
        let tv = vars(&[
            ("category", "TV :: Episodes HD"),
            ("releaseName", "The Show 2019 S03E08 2160p DV WEBRip 6CH x265 HEVC-GROUP"),
            ("uploader", "Anonymous"),
            ("freeleech", ""),
            ("baseUrl", "https://mock.local/"),
            ("torrentId", "240860011"),
            ("rsskey", "00000000000000000000"),
        ]);
        let want = "https://mock.local/rss/download/240860011/00000000000000000000/The+Show+2019+S03E08+2160p+DV+WEBRip+6CH+x265+HEVC-GROUP.torrent";
        let anime = vars(&[
            ("releaseName", "Great BluRay SoftSubbed Anime"),
            ("category", "TV Series"),
            ("year", "2020"),
            ("releaseTags", "Blu-ray / MKV / h264 10-bit / 1080p / FLAC 2.0 / Dual Audio / Softsubs (Sub Group) / Freeleech"),
            ("releaseGroup", "Softsubs"),
            ("releaseEpisode", ""),
            ("freeleech", "freeleech"),
            ("baseUrl", "https://mock.local"),
            ("torrentId", "240860011"),
            ("tags", "comedy, drama, school.life, sports"),
            ("uploader", "Uploader"),
            ("passkey", "00000000000000000000"),
        ]);
        let mut remux = tv.clone();
        remux.insert("category".into(), "Movies/Remux".into());
        remux.insert("torrentSize".into(), "".into());
        remux.insert("key".into(), "KEY".into());
        remux.insert("token".into(), "TOKEN".into());
        let enc = vec!["releaseName".to_string()];
        // the match, the base URL, the values, then two of the links wanted
        type Case<'a> = (ParseMatch, &'a str, BTreeMap<String, String>, &'a str, &'a str);
        let cases: Vec<Case> = vec![
            (
                ParseMatch { download_url: "rss/download/{{ .torrentId }}/{{ .rsskey }}/{{ .releaseName }}.torrent".into(), encode: enc.clone(), ..Default::default() },
                "https://mock.local/",
                tv.clone(),
                want,
                "",
            ),
            (
                ParseMatch { download_url: "/torrent/{{ .torrentId }}/download/{{ .passkey }}".into(), ..Default::default() },
                "https://mock.local/",
                anime,
                "https://mock.local/torrent/240860011/download/00000000000000000000",
                "",
            ),
            (
                ParseMatch {
                    download_url: "{{ .baseUrl }}rss/download/{{ .torrentId }}/{{ .rsskey }}/{{ .releaseName }}.torrent".into(),
                    encode: enc.clone(),
                    ..Default::default()
                },
                "https://mock.local/",
                tv.clone(),
                want,
                "",
            ),
            (
                ParseMatch {
                    download_url: "https://mock.local/rss/download/{{ .torrentId }}/{{ .rsskey }}/{{ .releaseName }}.torrent".into(),
                    encode: enc.clone(),
                    ..Default::default()
                },
                "https://mock.local/",
                tv.clone(),
                want,
                "",
            ),
            (
                ParseMatch {
                    download_url: "/rss/?action=download&key={{ .key }}&token={{ .token }}&hash={{ .torrentId }}&title={{ .releaseName }}".into(),
                    encode: enc,
                    ..Default::default()
                },
                "https://mock.local/",
                remux,
                "https://mock.local/rss/?action=download&key=KEY&token=TOKEN&hash=240860011&title=The+Show+2019+S03E08+2160p+DV+WEBRip+6CH+x265+HEVC-GROUP",
                "",
            ),
            (
                ParseMatch { magnet_uri: "magnet:?xt=urn:btih:{{ .torrentHash }}&dn={{ urlquery .releaseName }}".into(), ..Default::default() },
                "",
                vars(&[
                    ("torrentHash", "81c758d0eca5372d59e43879ecf2e2bce33a06c4"),
                    ("releaseName", "The Show 2019 S03E08 2160p DV WEBRip 6CH x265 HEVC-GROUP"),
                ]),
                "",
                "magnet:?xt=urn:btih:81c758d0eca5372d59e43879ecf2e2bce33a06c4&dn=The+Show+2019+S03E08+2160p+DV+WEBRip+6CH+x265+HEVC-GROUP",
            ),
        ];
        for (m, base, mut v, dl, magnet) in cases {
            let mut r = Release::default();
            m.parse_urls(base, &mut v, &mut r).unwrap();
            assert_eq!(r.download_url, dl, "{m:?}");
            assert_eq!(r.magnet_uri, magnet);
            assert_eq!(r.info_url, "");
        }
    }

    // internal/domain/indexer_test.go TestIndexerIRCV2ParseMatch_ParseTorrentName
    #[test]
    fn test_indexer_ircv2_parse_match_parse_torrent_name() {
        let mut r = Release::default();
        ParseMatch::default()
            .parse_release_name(&vars(&[("releaseName", "The Show 2019 S03E08 2160p DV WEBRip 6CH x265 HEVC-GROUP")]), &mut r)
            .unwrap();
        assert_eq!(r.torrent_name, "");
        let m = ParseMatch {
            release_name: r#"{{ if .releaseGroup }}[{{ .releaseGroup }}] {{ end }}{{ .releaseName }} [{{ .year }}] {{ if .releaseEpisode }}{{ printf "- %02s " .releaseEpisode }}{{ end }}{{ print "[" .releaseTags "]" | replace " / " "][" }}"#.into(),
            ..Default::default()
        };
        let v = vars(&[
            ("releaseName", "Great BluRay SoftSubbed Anime"),
            ("category", "TV Series"),
            ("year", "2020"),
            ("releaseTags", "Blu-ray / MKV / h264 10-bit / 1080p / FLAC 2.0 / Dual Audio / Softsubs (Sub Group) / Freeleech"),
            ("releaseGroup", "Softsubs"),
            ("releaseEpisode", ""),
            ("freeleech", "freeleech"),
            ("baseUrl", "https://mock.local"),
            ("torrentId", "240860011"),
            ("tags", "comedy, drama, school.life, sports"),
            ("uploader", "Uploader"),
            ("passkey", "00000000000000000000"),
        ]);
        m.parse_release_name(&v, &mut r).unwrap();
        assert_eq!(
            r.torrent_name,
            "[Softsubs] Great BluRay SoftSubbed Anime [2020] [Blu-ray][MKV][h264 10-bit][1080p][FLAC 2.0][Dual Audio][Softsubs (Sub Group)][Freeleech]"
        );
    }

    // internal/domain/indexer_test.go TestIndexerIRCV2Parse_MapCustomVariables
    #[test]
    fn test_indexer_ircv2_parse_map_custom_variables() {
        let m = |pairs: &[(&str, &[(&str, &str)])]| -> BTreeMap<String, BTreeMap<String, String>> {
            pairs.iter().map(|(k, v)| (k.to_string(), vars(v))).collect()
        };
        let mut mappings = Mappings::new();
        mappings.insert("announceType".into(), m(&[("0", &[("announceType", "NEW")]), ("1", &[("announceType", "PROMO")])]));
        mappings.insert(
            "categoryEnum".into(),
            m(&[
                ("0", &[("category", "Feature Film")]),
                ("1", &[("category", "Short Film")]),
                ("2", &[("category", "Miniseries")]),
                ("3", &[("category", "Stand-up Comedy")]),
                ("4", &[("category", "Live Performance")]),
                ("5", &[("category", "Movie Collection")]),
            ]),
        );
        mappings.insert(
            "freeleechEnum".into(),
            m(&[
                ("0", &[("downloadVolumeFactor", "1.0"), ("uploadVolumeFactor", "1.0")]),
                ("1", &[("downloadVolumeFactor", "0"), ("uploadVolumeFactor", "1.0")]),
                ("2", &[("downloadVolumeFactor", "0.5"), ("uploadVolumeFactor", "1.0")]),
                ("3", &[("downloadVolumeFactor", "0"), ("uploadVolumeFactor", "0")]),
            ]),
        );
        let p = Parse { mappings, ..Parse::default() };
        let mut v = vars(&[("announceType", "1"), ("categoryEnum", "0"), ("freeleechEnum", "1")]);
        p.map_custom_variables(&mut v);
        assert_eq!(
            v,
            vars(&[
                ("announceType", "PROMO"),
                ("category", "Feature Film"),
                ("categoryEnum", "0"),
                ("freeleechEnum", "1"),
                ("downloadVolumeFactor", "0"),
                ("uploadVolumeFactor", "1.0"),
            ])
        );
    }

    // internal/domain/indexer_channel_test.go TestIndexerIRCV2_GetChannel_CaseInsensitive
    #[test]
    fn test_get_channel_case_insensitive() {
        let mut d = parse_definition(
            "version: 2\nname: T\nidentifier: t\nurls: [https://t.test/]\nirc:\n  server: irc.t\n  channels:\n    - name: \"#MixedCase\"\n      parse: { type: single }\n",
        )
        .unwrap();
        d.prepare();
        let irc = d.irc.as_ref().unwrap();
        for n in ["#MixedCase", "#mixedcase", "#MIXEDCASE"] {
            assert_eq!(irc.channel(n).unwrap().name, "#MixedCase");
        }
        assert!(irc.channel("#other").is_none());
    }

    // internal/domain/irc_auth_test.go TestIRCAuthValidate (the definition side)
    #[test]
    fn test_definition_irc_auth() {
        let def = |version: &str, mechanism: &str| {
            format!(
                "---\n{version}name: Test\nidentifier: test\nimplementation: irc\nirc:\n  network: Test\n  server: irc.test\n  port: 6697\n  tls: true\n  auth:\n    mechanism: {mechanism}\n"
            )
        };
        let d = parse_definition(&def("", "NICKSERV")).unwrap();
        assert_eq!(d.irc.unwrap().auth.unwrap().mechanism, AUTH_NICKSERV);
        let d = parse_definition(&def("version: 2\n", "NONE")).unwrap();
        assert_eq!(d.irc.unwrap().auth.unwrap().mechanism, AUTH_NONE);
        assert!(parse_definition(&def("version: 2\n", "TYPO")).is_err());
        assert!(parse_definition(&def("version: 2\n", "")).is_err());
    }

    // internal/indexer/definition_test.go TestDefinitionIRCSizeUnits
    #[test]
    fn test_definition_irc_size_units() {
        let v1 = "name: Test\nidentifier: test\nimplementation: irc\nirc:\n  network: Test\n  server: irc.test\n  port: 6697\n  channels:\n    - \"#announce\"\n  parse:\n    type: single\n    sizeunits: ";
        let v2 = "version: 2\nname: Test\nidentifier: test\nimplementation: irc\nirc:\n  network: Test\n  server: irc.test\n  port: 6697\n  channels:\n    - name: \"#announce\"\n      parse:\n        type: single\n        sizeunits: ";
        for (data, units, err) in [
            (v1, "binary", false),
            (v1, "binray", true),
            (v2, "\"\"", false),
            (v2, "binary", false),
            (v2, "Binary", true),
            (v2, "decimal", true),
        ] {
            let r = parse_definition(&format!("{data}{units}\n"));
            if err {
                assert!(r.unwrap_err().contains("invalid size units"), "{units}");
                continue;
            }
            let d = r.unwrap();
            let irc = d.irc.unwrap();
            assert_eq!(irc.channels.len(), 1);
            assert_eq!(irc.channels[0].parse.as_ref().unwrap().sizeunits, units.trim_matches('"'));
        }
    }

    // internal/domain/irc_parser_test.go TestIRCParserGazelleGames_Parse
    #[test]
    fn test_irc_parser_gazelle_games_parse() {
        let cases: [(&str, &str, &str, &str); 16] = [
            ("Trouble.in.Paradise-GROUP in Trouble in Paradise", "", "Trouble in Paradise", "Trouble.in.Paradise-GROUP"),
            ("F.I.L.F. Game Walkthrough v.0.18 in F.I.L.F.", "", "F.I.L.F.", "F.I.L.F. Game Walkthrough v.0.18"),
            (
                "Ni no Kuni: Dominion of the Dark Djinn in Ni no Kuni: Dominion of the Dark Djinn",
                "",
                "Ni no Kuni: Dominion of the Dark Djinn",
                "Ni no Kuni: Dominion of the Dark Djinn",
            ),
            (
                "Year 2 Remastered by Insaneintherainmusic",
                "OST",
                "Year 2 Remastered by Insaneintherainmusic",
                "Year 2 Remastered by Insaneintherainmusic",
            ),
            ("Lanota - Version 2.23.1 in Lanota", "iOS", "Lanota", "Lanota"),
            ("After Inc. - Version 1.7.1", "iOS", "After Inc", "After Inc."),
            ("Through the Ages - Version 2.19.1128", "iOS", "Through the Ages", "Through the Ages"),
            ("Through the Ages - Version 2.19.1128 in Through the Ages", "iOS", "Through the Ages", "Through the Ages"),
            ("Asphalt 8: Airborne+ - Version 2.6.0b in Asphalt 8: Airborne", "iOS", "Asphalt 8: Airborne", "Asphalt 8: Airborne+"),
            ("Through the Ages - Version 2025-10-12 in Group", "iOS", "Group", "Through the Ages"),
            (
                "Korean_Drone_Flying_Tour_Han_River_NSW-SUXXORS in Korean Drone Flying Tour Han River",
                "Switch",
                "Korean Drone Flying Tour Han River",
                "Korean_Drone_Flying_Tour_Han_River_NSW-SUXXORS",
            ),
            (
                "Carmen_Sandiego_Update_v1.4.0_NSW-VENOM - Update - Version 1.4.0 in Carmen Sandiego",
                "Switch",
                "Carmen Sandiego",
                "Carmen_Sandiego_Update_v1.4.0_NSW-VENOM",
            ),
            ("Colin McRae Rally 3 - Version 1.1 in Colin McRae Rally 3", "Windows", "Colin McRae Rally 3", "Colin McRae Rally 3"),
            ("Soulstone Survivors - Version 1.1.5 (83772) in Soulstone Survivors", "Windows", "Soulstone Survivors", "Soulstone Survivors"),
            ("Digger: Galactic Treasures - Version 1.07", "Windows", "Digger: Galactic Treasures", "Digger: Galactic Treasures"),
            (
                "Bee.Simulator.The.Hive-RUNE - Version Unknown in Bee Simulator: The Hive",
                "Windows",
                "Bee Simulator: The Hive",
                "Bee.Simulator.The.Hive-RUNE",
            ),
        ];
        for (name, category, title, release) in cases {
            let mut r = Release::new(super::super::release::IndexerMinimal {
                id: 0,
                name: "GazelleGames".into(),
                identifier: "ggn".into(),
                identifier_external: "GazelleGames".into(),
            });
            let mut v = vars(&[("releaseName", name)]);
            if !category.is_empty() {
                v.insert("category".into(), category.into());
            }
            run_parser(Parser::GazelleGames, &mut r, &v).unwrap();
            assert_eq!(r.torrent_name, release, "{name}");
            assert_eq!(r.title, title, "{name}");
        }
    }

    // internal/domain/irc_parser_test.go TestIRCParserOrpheus_Parse
    #[test]
    fn test_irc_parser_orpheus_parse() {
        for (tags, release) in [
            ("WEB/FLAC/24bit Lossless", "Busta Rhymes - BEACH BALL (feat. BIA) [2023] (WEB FLAC 24BIT Lossless)"),
            ("CD/FLAC/Lossless", "Busta Rhymes - BEACH BALL (feat. BIA) [2023] (CD FLAC Lossless)"),
        ] {
            let mut r = Release::default();
            let v = vars(&[
                ("releaseName", &format!("Busta Rhymes – BEACH BALL (feat. BIA) – [2023] [Single] {tags}")),
                ("title", "Busta Rhymes – BEACH BALL (feat. BIA)"),
                ("year", "2023"),
                ("releaseTags", tags),
            ]);
            run_parser(Parser::Orpheus, &mut r, &v).unwrap();
            assert_eq!(r.torrent_name, release);
            assert_eq!(r.title, "BEACH BALL");
        }
    }

    // internal/domain/irc_parser_test.go Test_splitInMiddle
    #[test]
    fn test_split_in_middle() {
        for (s, a, b) in [
            ("Trouble.in.Paradise-GROUP in Trouble in Paradise", "Trouble.in.Paradise-GROUP", "Trouble in Paradise"),
            ("Trouble.in.Paradise-GROUP", "Trouble.in.Paradise-GROUP", ""),
            ("Best.Game.Ever-GROUP in Best Game Ever", "Best.Game.Ever-GROUP", "Best Game Ever"),
        ] {
            assert_eq!(split_in_middle(s, " in "), (a.to_string(), b.to_string()));
        }
    }

    #[test]
    fn query_escape_is_gos() {
        assert_eq!(query_escape("a b+c/d~e*"), "a+b%2Bc%2Fd~e%2A");
        assert_eq!(clean_path("/a//b/../c/"), "/a/c");
        assert_eq!(join_path(&["/", "/torrent/1"]), "/torrent/1");
    }
}
