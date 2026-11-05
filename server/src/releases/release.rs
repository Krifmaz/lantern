// Ported from autobrr v1.90.0 (internal/domain/release.go), GPL-2.0-or-later.
//! A release as autobrr sees it: what the announce or feed said ([`Release::map_vars`]) plus what its name says
//! ([`Release::parse_string`], through [`super::rls`]), ready for the filters ([`super::filter`]).

use super::humanize;
use super::releasetags::{clean_release_tags, parse_release_tag_string};
use super::rls::{self, Collapser, TagType, Type};
use md5::{Digest, Md5};
use std::collections::BTreeMap;

/// A new release, not yet checked.
pub const STATUS_PENDING: &str = "PENDING";
/// A release a filter took.
pub const STATUS_APPROVED: &str = "FILTER_APPROVED";

/// The few things about the indexer a release came from that filters and macros use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexerMinimal {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Its short name (`redacted`, `ops`).
    pub identifier: String,
    /// Its short name elsewhere (Prowlarr's).
    pub identifier_external: String,
}

/// One release.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Release {
    /// Its id once saved.
    pub id: i64,
    /// Where the filters are with it (`PENDING`, `FILTER_APPROVED`).
    pub filter_status: String,
    /// Why filters turned it down.
    pub rejections: Vec<String>,
    /// The indexer it came from.
    pub indexer: IndexerMinimal,
    /// The filter that took it.
    pub filter_name: String,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// How it was found: `IRC`, `TORZNAB`, `NEWZNAB` or `RSS`.
    pub implementation: String,
    /// What the announce said it is: `NEW`, `CHECKED`, `PROMO`, `PROMO_GP` or `RESURRECTED`.
    pub announce_type: String,
    /// What kind of release its name says it is.
    pub r#type: Type,
    /// Its page on the site.
    pub info_url: String,
    /// Where to download it.
    pub download_url: String,
    /// Its magnet link.
    pub magnet_uri: String,
    /// When the site published it, when the source says (feeds); Sonarr and Radarr get it.
    pub publish_date: Option<chrono::DateTime<chrono::Utc>>,
    /// The site's group id.
    pub group_id: String,
    /// The site's torrent id.
    pub torrent_id: String,
    /// Where the downloaded torrent was put for a moment.
    pub torrent_tmp_file: String,
    /// The downloaded torrent.
    pub torrent_data_raw_bytes: Vec<u8>,
    /// The torrent's info hash.
    pub torrent_hash: String,
    /// The full release name.
    pub torrent_name: String,
    /// Everything the announce line gave, by name.
    pub raw_vars: BTreeMap<String, String>,
    /// The md5 of the normalized name ([`Release::hash`]).
    pub normalized_hash: String,
    /// Its size in bytes (0: not known).
    pub size: u64,
    /// The title from its name.
    pub title: String,
    /// A second title (an episode's name).
    pub sub_title: String,
    /// Its description (from feeds).
    pub description: String,
    /// The site's category.
    pub category: String,
    /// The site's categories (from feeds).
    pub categories: Vec<String>,
    /// The season.
    pub season: i64,
    /// The episode.
    pub episode: i64,
    /// The year.
    pub year: i64,
    /// The month (daily shows).
    pub month: i64,
    /// The day (daily shows).
    pub day: i64,
    /// The resolution (`1080p`).
    pub resolution: String,
    /// The source (`WEB-DL`, `BluRay`).
    pub source: String,
    /// The video codecs.
    pub codec: Vec<String>,
    /// The container (`mkv`).
    pub container: String,
    /// The HDR formats.
    pub hdr: Vec<String>,
    /// The audio formats and tags.
    pub audio: Vec<String>,
    /// The audio channels (`5.1`).
    pub audio_channels: String,
    /// The audio format from the release tags (`FLAC`).
    pub audio_format: String,
    /// The bitrate from the release tags (`Lossless`, `320`).
    pub bitrate: String,
    /// The release group.
    pub group: String,
    /// The region (`R1`).
    pub region: String,
    /// The languages.
    pub language: Vec<String>,
    /// Whether it is a PROPER.
    pub proper: bool,
    /// Whether it is a REPACK.
    pub repack: bool,
    /// The streaming site (`AMZN`, `NF`).
    pub website: String,
    /// Whether it is a HYBRiD.
    pub hybrid: bool,
    /// The editions.
    pub edition: Vec<String>,
    /// The cuts.
    pub cut: Vec<String>,
    /// `REMUX` for remuxes.
    pub media_processing: String,
    /// The artists (music) or author (books).
    pub artists: String,
    /// The rip log's score (music).
    pub log_score: i64,
    /// Whether it has a cue file (music).
    pub has_cue: bool,
    /// Whether it has a rip log (music).
    pub has_log: bool,
    /// Where it comes from: `P2P`, `SCENE`, `O-SCENE`, `INTERNAL`.
    pub origin: String,
    /// The site's tags.
    pub tags: Vec<String>,
    /// The site's release tags, as one text (`FLAC / Lossless / Log / 100% / Cue / CD`).
    pub release_tags: String,
    /// Whether it is freeleech.
    pub freeleech: bool,
    /// How much freeleech (0–100).
    pub freeleech_percent: i64,
    /// Bonuses (`Freeleech`, `Freeleech50`).
    pub bonus: Vec<String>,
    /// Who uploaded it.
    pub uploader: String,
    /// The record label (music).
    pub record_label: String,
    /// How long after the scene release it came (as the site says it).
    pub pre_time: String,
    /// Other tags (`PROPER`, `REPACK`, `REMUX`).
    pub other: Vec<String>,
    /// The cookie to download with.
    pub raw_cookie: String,
    /// The browser name to download with.
    pub user_agent: String,
    /// Seeders (feeds).
    pub seeders: i64,
    /// Leechers (feeds).
    pub leechers: i64,
    /// The announce gave no size and a filter has size limits: the torrent must be read to know.
    pub additional_size_check_required: bool,
    /// The announce gave no uploader and a filter asks (RED and OPS): the site must be asked.
    pub additional_uploader_check_required: bool,
    /// The announce gave no record label and a filter asks (RED and OPS): the site must be asked.
    pub additional_record_label_check_required: bool,
    /// Whether it is a duplicate of one already taken.
    pub is_duplicate: bool,
    /// The duplicate profile that said so.
    pub skip_duplicate_profile_id: i64,
    /// That profile's name.
    pub skip_duplicate_profile_name: String,
    /// The filter that took it.
    pub filter_id: i64,
    /// The IMDb id (`tt0133093`).
    pub meta_imdb: String,
    /// The TMDb id.
    pub meta_tmdb: i64,
    /// The TVDB id.
    pub meta_tvdb: i64,
    /// The IGDB id.
    pub meta_igdb: i64,
}

/// Go's `cmp.Or` for text: the first that isn't empty.
fn or_s(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        a.to_string()
    }
}

/// Go's `cmp.Or` for numbers: the first that isn't 0.
fn or_n(a: i64, b: i64) -> i64 {
    if a != 0 {
        a
    } else {
        b
    }
}

impl Release {
    /// A new release from `indexer`, with autobrr's defaults (`NewRelease`).
    pub fn new(indexer: IndexerMinimal) -> Release {
        Release {
            indexer,
            filter_status: STATUS_PENDING.into(),
            protocol: "torrent".into(),
            implementation: "IRC".into(),
            announce_type: "NEW".into(),
            ..Release::default()
        }
    }

    /// Reads `title` as a release name and fills in what it says, keeping what the announce already gave
    /// (autobrr's `ParseString`).
    pub fn parse_string(&mut self, title: &str) {
        let rel = rls::parse_string(title);
        self.r#type = rel.r#type;
        self.torrent_name = title.to_string();
        self.source = or_s(&rel.source, &self.source);
        self.resolution = or_s(&rel.resolution, &self.resolution);
        self.region = rel.region.clone();
        if !rel.language.is_empty() {
            self.language = rel.language.clone();
        }
        self.audio = rel.audio.clone();
        self.audio_channels = rel.channels.clone();
        self.codec = rel.codec.clone();
        self.container = or_s(&self.container, &rel.container);
        self.hdr = rel.hdr.clone();
        if !rel.artist.is_empty() {
            self.artists = rel.artist.clone();
        }
        if !rel.other.is_empty() {
            self.other = rel.other.clone();
        }
        let has = |o: &str| self.other.iter().any(|x| x == o);
        self.proper = has("PROPER");
        self.repack = has("REPACK") || has("REREPACK");
        self.hybrid = has("HYBRiD");
        if has("REMUX") {
            self.media_processing = "REMUX".into();
        }
        self.title = or_s(&self.title, &rel.title);
        self.sub_title = rel.subtitle.clone();
        self.season = or_n(self.season, rel.series.into());
        self.episode = or_n(self.episode, rel.episode.into());
        self.year = or_n(self.year, rel.year.into());
        self.month = or_n(self.month, rel.month.into());
        self.day = or_n(self.day, rel.day.into());
        self.group = or_s(&self.group, &rel.group);
        self.website = or_s(&self.website, &rel.collection);
        if !rel.cut.is_empty() {
            self.cut = rel.cut.clone();
        }
        if !rel.edition.is_empty() {
            self.edition = rel.edition.clone();
        }
        let tags = self.release_tags.clone();
        self.parse_release_tags_string(&tags);
        self.extra_parse_source(&rel);
        self.normalized_hash = self.hash();
    }

    /// Fixes what rls gets wrong for some video names: a source at the very end after a space is the group
    /// (`… DTS-HD MA 5.1-VHS`), and a 1080p or 2160p AVC/HEVC remux without a source is a Blu-ray.
    fn extra_parse_source(&mut self, rel: &rls::Release) {
        if !rel.r#type.is(&[Type::Movie, Type::Series, Type::Episode]) {
            return;
        }
        let tags = rel.tags();
        if tags.len() < 3 {
            return;
        }
        if self.group.is_empty() {
            let last = &tags[tags.len() - 1];
            if last.tag_type() == TagType::Source && last.prev() == TagType::Whitespace {
                let group = last.text();
                if self.source == group {
                    self.source.clear();
                }
                self.group = group;
            }
        }
        let bluray = ["WEB-DL", "BluRay", "UHD.BluRay"].map(String::from);
        if basic_contains_slice(&self.source, &bluray) {
            return;
        }
        if basic_contains_slice(&self.resolution, &["1080p".into(), "2160p".into()])
            && basic_contains_match(&self.codec, &["AVC", "H.264", "H.265", "HEVC"].map(String::from))
            && basic_contains_match(&self.other, &["REMUX".into()])
        {
            if self.resolution == "1080p" {
                self.source = "BluRay".into();
            } else if self.resolution == "2160p" {
                self.source = "UHD.BluRay".into();
            }
        }
    }

    /// Fills in what the site's release tags say (autobrr's `ParseReleaseTagsString`).
    pub fn parse_release_tags_string(&mut self, tags: &str) {
        if tags.is_empty() {
            return;
        }
        let t = parse_release_tag_string(&clean_release_tags(tags));
        if !t.audio.is_empty() {
            self.audio = t.audio.clone();
        }
        if !t.audio_bitrate.is_empty() {
            self.bitrate = t.audio_bitrate.clone();
        }
        if !t.audio_format.is_empty() {
            self.audio_format = t.audio_format.clone();
        }
        if self.audio_channels.is_empty() && !t.channels.is_empty() {
            self.audio_channels = t.channels.clone();
        }
        if t.has_log {
            self.has_log = true;
            if t.log_score > 0 {
                self.log_score = t.log_score;
            }
        }
        if t.has_cue {
            self.has_cue = true;
        }
        if !t.bonus.is_empty() {
            if super::filter::slice_contains_slice(&["Freeleech".into(), "Freeleech!".into()], &t.bonus) {
                self.freeleech = true;
            }
            self.bonus.extend(t.bonus.iter().cloned());
        }
        if !t.codec.is_empty() {
            self.codec = get_unique_tags(std::mem::take(&mut self.codec), std::slice::from_ref(&t.codec));
        }
        if !t.other.is_empty() {
            self.other = get_unique_tags(std::mem::take(&mut self.other), &t.other);
        }
        if self.origin.is_empty() && !t.origin.is_empty() {
            self.origin = t.origin.clone();
        }
        if self.container.is_empty() && !t.container.is_empty() {
            self.container = t.container.clone();
        }
        if self.resolution.is_empty() && !t.resolution.is_empty() {
            self.resolution = t.resolution.clone();
        }
        if self.source.is_empty() && !t.source.is_empty() {
            self.source = t.source.clone();
        }
    }

    /// Reads a size in words and keeps it when it is bigger than the size already known (autobrr's
    /// `ParseSizeBytesString`).
    pub fn parse_size_bytes_string(&mut self, size: &str) {
        if let Ok(s) = humanize::parse_bytes(size) {
            if s > self.size {
                self.size = s;
            }
        }
    }

    /// Whether it is a movie, a season or an episode.
    pub fn is_type_video(&self) -> bool {
        self.r#type.is(&[Type::Movie, Type::Series, Type::Episode])
    }

    /// The md5 (hex) of its normalized name: the title with its year, season, episode, resolution, source and the
    /// rest for videos, the full name for the rest (autobrr's `Hash`).
    pub fn hash(&self) -> String {
        let formatted = if self.is_type_video() { self.normalized_title() } else { self.torrent_name.clone() };
        hex::encode(Md5::digest(must_normalize(&formatted).as_bytes()))
    }

    /// The parts of the name that tell two videos apart, in a fixed order (autobrr's `NormalizedTitle`).
    pub fn normalized_title(&self) -> String {
        let mut v = vec![self.title.clone()];
        if self.year > 0 && self.month > 0 && self.day > 0 {
            v.push(format!("{} {} {}", self.year, self.month, self.day));
        } else if self.year > 0 {
            v.push(self.year.to_string());
        }
        for l in [&self.language, &self.cut, &self.edition] {
            if !l.is_empty() {
                v.push(l.join(" "));
            }
        }
        if self.season > 0 && self.episode > 0 {
            v.push(format!("S{}E{}", self.season, self.episode));
        } else if self.season > 0 && self.episode == 0 {
            v.push(format!("S{}", self.season));
        }
        if self.proper {
            v.push("PROPER".into());
        }
        if self.repack {
            v.push(self.repack_str().into());
        }
        if self.hybrid {
            v.push("HYBRiD".into());
        }
        for s in [&self.sub_title, &self.resolution, &self.website, &self.region, &self.source] {
            if !s.is_empty() {
                v.push(s.clone());
            }
        }
        if self.media_processing == "REMUX" {
            v.push("REMUX".into());
        }
        if !self.codec.is_empty() {
            v.push(self.codec.join(" "));
        }
        if !self.hdr.is_empty() {
            v.push(self.hdr.join(" "));
        }
        if !self.audio.is_empty() {
            v.push(self.audio_string());
        }
        let s = v.join(" ");
        if self.group.is_empty() {
            s
        } else {
            format!("{s}-{}", self.group)
        }
    }

    /// Which repack it is: `REPACK`, `REREPACK`, `REPACK2`, `REPACK3` or empty.
    pub fn repack_str(&self) -> &'static str {
        ["REPACK", "REREPACK", "REPACK2", "REPACK3"].into_iter().find(|r| self.other.iter().any(|o| o == r)).unwrap_or("")
    }

    /// The audio formats and channels in one text (`DDP Atmos 5.1`).
    pub fn audio_string(&self) -> String {
        let mut a = self.audio.clone();
        a.push(self.audio_channels.clone());
        a.join(" ")
    }

    /// Fills in what an announce line (or feed item) gave, by autobrr's variable names (autobrr's `MapVars`).
    /// `force_size_unit` is added to a size without a unit; `size_units` `binary` reads `GB` as `GiB`.
    pub fn map_vars(&mut self, vars: &BTreeMap<String, String>, force_size_unit: &str, size_units: &str) -> Result<(), String> {
        let get = |k: &str| get_string_map_value(vars, k);
        let name = get_string_map_value_alt(vars, &["releaseName", "torrentName"])
            .ok_or_else(|| "failed parsing required field: torrentName or releaseName".to_string())?;
        self.torrent_name = html_escape::decode_html_entities(&name).into_owned();
        if let Some(v) = get("torrentHash") {
            self.torrent_hash = v;
        }
        if let Some(v) = get("torrentId") {
            self.torrent_id = v;
        }
        if let Some(v) = get("category") {
            self.category = v;
        }
        if let Some(v) = get("subCategory") {
            if !self.category.is_empty() {
                self.category = format!("{}/{v}", self.category);
            }
        }
        if let Some(v) = get("announceType") {
            if let Ok(t) = parse_announce_type(&v) {
                self.announce_type = t.into();
            }
        }
        if let Some(v) = get("freeleech") {
            if string_equal_fold_multi(&v, &["1", "fl", "free", "freeleech", "freeleech!", "yes", "VIP", "★"]) {
                self.freeleech = true;
                self.freeleech_percent = 100;
                self.bonus.push("Freeleech".into());
            }
        }
        if let Some(v) = get("freeleechPercent") {
            if let Ok(p) = v.replace('%', "").trim_matches(' ').parse::<i64>() {
                if p > 0 {
                    self.freeleech = true;
                    self.freeleech_percent = p;
                    self.bonus.push("Freeleech".into());
                    match p {
                        25 => self.bonus.push("Freeleech25".into()),
                        50 => self.bonus.push("Freeleech50".into()),
                        75 => self.bonus.push("Freeleech75".into()),
                        100 => self.bonus.push("Freeleech100".into()),
                        _ => {}
                    }
                }
            }
        }
        if let Some(v) = vars.get("downloadVolumeFactor") {
            if let Ok(f) = v.parse::<f64>() {
                if (0.0..=1.0).contains(&f) {
                    self.freeleech_percent = 100 - (f * 100.0).round() as i64;
                    if self.freeleech_percent > 0 {
                        self.freeleech = true;
                    }
                }
            }
        }
        if let Some(v) = get("uploader") {
            self.uploader = v;
        }
        if let Some(v) = get("recordLabel") {
            self.record_label = v;
        }
        if let Some(v) = get("torrentSize") {
            let mut size = v.replacen(',', ".", 1);
            if !force_size_unit.is_empty() {
                size = format!("{size} {force_size_unit}");
            }
            if size_units == "binary" {
                size = to_binary_size_unit(&size);
            }
            if let Ok(s) = humanize::parse_bytes(&size) {
                self.size = s;
            }
        }
        if let Some(v) = get("torrentSizeBytes") {
            if let Some(s) = parse_uint(&v) {
                self.size = s;
            }
        }
        if let Some(v) = get("scene") {
            if string_equal_fold_multi(&v, &["true", "yes", "1"]) {
                self.origin = "SCENE".into();
            }
        }
        if let Some(v) = get("origin") {
            self.origin = v;
        }
        if let Some(v) = get("internal") {
            if string_equal_fold_multi(&v, &["internal", "yes", "1"]) {
                self.origin = "INTERNAL".into();
            }
        }
        if let Some(v) = get("year") {
            if let Ok(y) = v.parse() {
                self.year = y;
            }
        }
        if let Some(v) = get("tags") {
            if !v.is_empty() && v != "*" {
                self.tags = v.split(',').map(|t| t.trim_matches(' ').to_string()).collect();
            }
        }
        if let Some(v) = get("title") {
            if !v.is_empty() && v != "*" {
                self.title = v;
            }
        }
        if let Some(v) = get("author") {
            self.artists = v;
        }
        if let Some(v) = get("releaseTags") {
            self.release_tags = v;
        }
        if let Some(v) = get("resolution") {
            self.resolution = v;
        }
        if let Some(v) = get("source") {
            self.source = v;
        }
        if let Some(v) = get("releaseGroup") {
            self.group = v;
        }
        if let Some(v) = get("releaseEpisode") {
            self.episode = v.parse().unwrap_or(0);
        }
        if let Some(v) = get("language") {
            self.language.push(v);
        }
        if let Some(v) = get("container") {
            self.container = v;
        }
        if let Some(v) = get("imdb") {
            self.meta_imdb = normalize_imdb_id(&v);
        }
        let id = |keys: &[&str]| get_string_map_value_alt(vars, keys).and_then(|v| v.trim().parse::<i64>().ok()).filter(|n| *n > 0);
        if let Some(n) = id(&["tmdb", "tmdbid"]) {
            self.meta_tmdb = n;
        }
        if let Some(n) = id(&["tvdb", "tvdbid"]) {
            self.meta_tvdb = n;
        }
        if let Some(n) = id(&["igdb", "igdbid"]) {
            self.meta_igdb = n;
        }
        self.raw_vars = vars.clone();
        Ok(())
    }
}

/// Go's `strconv.ParseUint(s, 10, 64)`: digits only.
fn parse_uint(s: &str) -> Option<u64> {
    if s.starts_with('+') {
        return None;
    }
    s.parse().ok()
}

/// The announce type named `s` (`NEW`, `CHECKED`, `PROMO`, `PROMO_GP`, `RESURRECTED`).
pub fn parse_announce_type(s: &str) -> Result<&'static str, String> {
    ["NEW", "CHECKED", "PROMO", "PROMO_GP", "RESURRECTED"].into_iter().find(|t| *t == s).ok_or_else(|| format!("invalid AnnounceType: {s}"))
}

/// A size's decimal unit as its binary one: `7.63 GB` becomes `7.63 GiB`; `7.63 G` stays (autobrr's
/// `toBinarySizeUnit`).
pub fn to_binary_size_unit(size: &str) -> String {
    let size = size.trim();
    let lower = size.to_lowercase();
    let b = lower.as_bytes();
    if b.len() < 2 || !lower.ends_with('b') || lower.ends_with("ib") {
        return size.to_string();
    }
    if !b"kmgtpe".contains(&b[b.len() - 2]) {
        return size.to_string();
    }
    format!("{}iB", &size[..size.len() - 1])
}

/// An IMDb id as `tt` and at least seven digits; empty when it is empty, 0 or not a number (autobrr's
/// `NormalizeIMDBID`).
pub fn normalize_imdb_id(id: &str) -> String {
    let lower = id.trim().to_lowercase();
    match parse_uint(lower.strip_prefix("tt").unwrap_or(&lower)) {
        Some(n) if n > 0 => format!("tt{n:07}"),
        _ => String::new(),
    }
}

/// The value of `key`, exactly or else in any case.
fn get_string_map_value(vars: &BTreeMap<String, String>, key: &str) -> Option<String> {
    if let Some(v) = vars.get(key) {
        return Some(v.clone());
    }
    let key = key.to_lowercase();
    vars.iter().find(|(k, _)| k.to_lowercase() == key).map(|(_, v)| v.clone())
}

/// The value of the first of `keys` that is there.
fn get_string_map_value_alt(vars: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| get_string_map_value(vars, k))
}

/// `s` split at any of the characters in `seps`, without empty parts (autobrr's `SplitAny`).
pub fn split_any(s: &str, seps: &str) -> Vec<String> {
    s.split(|c| seps.contains(c)).filter(|p| !p.is_empty()).map(String::from).collect()
}

/// Whether `s` is one of `values`, in any case.
pub fn string_equal_fold_multi(s: &str, values: &[&str]) -> bool {
    let s = s.to_lowercase();
    values.iter().any(|v| v.to_lowercase() == s)
}

/// `target` with the tags of `source` it doesn't have yet, compared in rls's normalized form (autobrr's
/// `getUniqueTags`).
pub fn get_unique_tags(mut target: Vec<String>, source: &[String]) -> Vec<String> {
    let have: Vec<String> = target.iter().map(|s| rls::must_normalize(s)).collect();
    let add: Vec<String> = source.iter().filter(|t| !have.contains(&rls::must_normalize(t))).cloned().collect();
    target.extend(add);
    target
}

/// A lower case, clean form of `s` for comparing names; unlike rls's it keeps `+` (`HDR10+`) (autobrr's
/// `MustNormalize`).
pub fn must_normalize(s: &str) -> String {
    Collapser {
        lower: true,
        trim: true,
        remove: "`':;~!@#%^*=()[]{}<>/?|\\\",",
        space: " \t\r\n\x0c._",
        transformer: Some(rls::money_letters),
    }
    .apply(s)
}

/// Whether `tag` is one of `filters`, exactly (autobrr's `basicContainsSlice`).
pub fn basic_contains_slice(tag: &str, filters: &[String]) -> bool {
    basic_contains_match(&[tag.to_string()], filters)
}

/// Whether any of `tags` is one of `filters`, exactly; empty ones never count (autobrr's `basicContainsMatch`).
pub fn basic_contains_match(tags: &[String], filters: &[String]) -> bool {
    tags.iter().filter(|t| !t.is_empty()).any(|t| filters.iter().any(|f| !f.is_empty() && f == t))
}
