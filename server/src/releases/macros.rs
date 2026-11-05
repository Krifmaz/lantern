// Ported from autobrr v1.90.0 (internal/domain/macros.go), GPL-2.0-or-later.
//! Macros: `{{ .TorrentName }}`, `{{ .Indexer }}`, `{{ .RawVars.custom }}` and the rest in action fields, filled in
//! from a release with Go's template rules ([`super::gotemplate`]).

use super::gotemplate::{Template, Tv};
use super::humanize;
use super::release::Release;
use chrono::{Datelike, Local, Timelike};
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

/// Every value a macro can name, from one release and the time it was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Macro {
    /// The fields by autobrr's names (`TorrentName`, `CurrentYear` …).
    pub fields: BTreeMap<String, Tv>,
}

/// A list of texts as a template value.
fn list(l: &[String]) -> Tv {
    Tv::List(l.iter().map(|s| Tv::Str(s.clone())).collect())
}

/// Text as a template value.
fn s(v: &str) -> Tv {
    Tv::Str(v.to_string())
}

impl Macro {
    /// The macro values of `r`, at the current local time (autobrr's `NewMacro`).
    pub fn new(r: &Release) -> Macro {
        let now = Local::now();
        let ms = now.timestamp_millis();
        let fields: BTreeMap<String, Tv> = [
            ("Artists", s(&r.artists)),
            ("Audio", list(&r.audio)),
            ("AudioChannels", s(&r.audio_channels)),
            ("AudioFormat", s(&r.audio_format)),
            ("Bitrate", s(&r.bitrate)),
            ("Bonus", list(&r.bonus)),
            ("Categories", list(&r.categories)),
            ("Category", s(&r.category)),
            ("Codec", list(&r.codec)),
            ("Container", s(&r.container)),
            ("Cookie", s(&r.raw_cookie)),
            ("CurrentDay", Tv::Int(now.day().into())),
            ("CurrentHour", Tv::Int(now.hour().into())),
            ("CurrentMinute", Tv::Int(now.minute().into())),
            ("CurrentMonth", Tv::Int(now.month().into())),
            ("CurrentSecond", Tv::Int(now.second().into())),
            ("CurrentYear", Tv::Int(now.year().into())),
            ("CurrentTimeUnixMS", Tv::Int(ms)),
            ("CurrentUnixTimeMS", Tv::Int(ms)),
            ("Description", s(&r.description)),
            ("DownloadUrl", s(&r.download_url)),
            ("Episode", Tv::Int(r.episode)),
            ("FilterID", Tv::Int(r.filter_id)),
            ("FilterName", s(&r.filter_name)),
            ("Freeleech", Tv::Bool(r.freeleech)),
            ("FreeleechPercent", Tv::Int(r.freeleech_percent)),
            ("Group", s(&r.group)),
            ("GroupID", s(&r.group_id)),
            ("HDR", s(&r.hdr.join(", "))),
            ("HasCue", Tv::Bool(r.has_cue)),
            ("HasLog", Tv::Bool(r.has_log)),
            ("Implementation", s(implementation(&r.implementation))),
            ("Indexer", s(&r.indexer.identifier)),
            ("IndexerIdentifier", s(&r.indexer.identifier)),
            ("IndexerIdentifierExternal", s(&r.indexer.identifier_external)),
            ("IndexerName", s(&r.indexer.name)),
            ("InfoUrl", s(&r.info_url)),
            ("IsDuplicate", Tv::Bool(r.is_duplicate)),
            ("Language", list(&r.language)),
            ("Leechers", Tv::Int(r.leechers)),
            ("LogScore", Tv::Int(r.log_score)),
            ("MagnetURI", s(&r.magnet_uri)),
            ("MetaIMDB", s(&r.meta_imdb)),
            ("MetaTMDB", Tv::Int(r.meta_tmdb)),
            ("MetaTVDB", Tv::Int(r.meta_tvdb)),
            ("MetaIGDB", Tv::Int(r.meta_igdb)),
            ("Origin", s(&r.origin)),
            ("Other", list(&r.other)),
            ("PreTime", s(&r.pre_time)),
            ("Protocol", s(if r.protocol == "usenet" { "usenet" } else { "torrent" })),
            ("Proper", Tv::Bool(r.proper)),
            ("Region", s(&r.region)),
            ("ReleaseTags", s(&r.release_tags)),
            ("Repack", Tv::Bool(r.repack)),
            ("Resolution", s(&r.resolution)),
            ("Season", Tv::Int(r.season)),
            ("Seeders", Tv::Int(r.seeders)),
            ("Size", Tv::Uint(r.size)),
            ("SizeString", s(&humanize::bytes(r.size))),
            ("SkipDuplicateProfileID", Tv::Int(r.skip_duplicate_profile_id)),
            ("SkipDuplicateProfileName", s(&r.skip_duplicate_profile_name)),
            ("Source", s(&r.source)),
            ("Tags", s(&r.tags.join(", "))),
            ("Title", s(&r.title)),
            ("TorrentDataRawBytes", Tv::Bytes(r.torrent_data_raw_bytes.clone())),
            ("TorrentHash", s(&r.torrent_hash)),
            ("TorrentID", s(&r.torrent_id)),
            ("TorrentName", s(&r.torrent_name)),
            ("TorrentPathName", s(&r.torrent_tmp_file)),
            ("TorrentUrl", s(&r.download_url)),
            ("TorrentTmpFile", s(&r.torrent_tmp_file)),
            ("Type", s(r.r#type.as_str())),
            ("Uploader", s(&r.uploader)),
            ("RecordLabel", s(&r.record_label)),
            ("Website", s(&r.website)),
            ("RawVars", Tv::Map(r.raw_vars.iter().map(|(k, v)| (k.clone(), s(v))).collect())),
            ("Year", Tv::Int(r.year)),
            ("Month", Tv::Int(r.month)),
            ("Day", Tv::Int(r.day)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        Macro { fields }
    }

    /// A field's value (`CurrentYear`).
    pub fn get(&self, name: &str) -> Option<&Tv> {
        self.fields.get(name)
    }

    /// `text` with its macros filled in; the problem when the template can't be read or run (autobrr's
    /// `Parse`).
    pub fn parse(&self, text: &str) -> Result<String, String> {
        if text.is_empty() {
            return Ok(String::new());
        }
        let tmpl = cached(text).map_err(|e| format!("could not parse macro template: {e}"))?;
        tmpl.execute(&Tv::Struct("domain.Macro", self.fields.clone())).map_err(|e| format!("could not parse macro: {e}"))
    }

    /// `text` with its macros filled in; empty when that can't be done (autobrr's `MustParse`).
    pub fn must_parse(&self, text: &str) -> String {
        self.parse(text).unwrap_or_default()
    }
}

/// The name autobrr gives each way of finding releases.
fn implementation(i: &str) -> &str {
    match i {
        "TORZNAB" | "NEWZNAB" | "RSS" => i,
        _ => "IRC",
    }
}

/// Templates already parsed, by their text.
static CACHE: LazyLock<Mutex<HashMap<String, Template>>> = LazyLock::new(Default::default);

/// The parsed template for `text`, from the cache when it was parsed before.
fn cached(text: &str) -> Result<Template, String> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = cache.get(text) {
        return Ok(t.clone());
    }
    let t = Template::parse("macro", text)?;
    if cache.len() > 1000 {
        cache.clear();
    }
    cache.insert(text.to_string(), t.clone());
    Ok(t)
}
