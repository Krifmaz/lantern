//! Reading a release's name: the resolution, source, codec, year, group, season and episode that filters ask
//! about (`Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP`).

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// What a release's name says about it. Every field is empty (or `None`) when the name doesn't say.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Name {
    /// The title before the year, season or quality words, with dots as spaces (`Show Name`).
    pub title: String,
    /// The year (`2024`).
    pub year: Option<u32>,
    /// The season (`1` for `S01`).
    pub season: Option<u32>,
    /// The episode (`2` for `S01E02`).
    pub episode: Option<u32>,
    /// `2160p`, `1080p`, `1080i`, `720p`, `576p`, `480p`.
    pub resolution: String,
    /// `WEB-DL`, `WEBRip`, `BluRay`, `Remux`, `HDTV`, `DVD`, `UHD BluRay` …
    pub source: String,
    /// `H.264`, `H.265`, `AV1`, `XviD`, `MPEG-2`, `VC-1`.
    pub codec: String,
    /// `DV`, `HDR10+`, `HDR10`, `HDR`, `HLG` (each the name has).
    pub hdr: Vec<String>,
    /// `DTS-HD MA`, `TrueHD`, `Atmos`, `DDP`, `DD`, `AAC`, `FLAC`, `MP3`, `Opus` (each the name has).
    pub audio: Vec<String>,
    /// The group after the last dash (`GROUP`).
    pub group: String,
    /// A proper or repack.
    pub proper: bool,
}

/// One word list: a pattern and the name it gets.
struct Words(&'static [(&'static str, &'static str)]);

impl Words {
    /// The names of every pattern `s` has, in the list's order.
    fn all(&self, s: &str) -> Vec<String> {
        self.0.iter().filter(|(re, _)| word(re).is_match(s)).map(|(_, n)| n.to_string()).collect()
    }

    /// The name of the first pattern `s` has.
    fn first(&self, s: &str) -> String {
        self.0.iter().find(|(re, _)| word(re).is_match(s)).map(|(_, n)| n.to_string()).unwrap_or_default()
    }
}

/// A pattern matched as a whole word of a release name (between dots, spaces, dashes, brackets or the ends).
fn word(re: &str) -> Regex {
    Regex::new(&format!(r"(?i)(?:^|[\s._\-\[\(])(?:{re})(?:$|[\s._\-\]\)])")).expect("a release word pattern")
}

const RESOLUTIONS: Words =
    Words(&[("2160p|4k|uhd", "2160p"), ("1080p", "1080p"), ("1080i", "1080i"), ("720p", "720p"), ("576p", "576p"), ("480p|sd", "480p")]);

const SOURCES: Words = Words(&[
    (r"uhd[\s._\-]?blu[\s._\-]?ray", "UHD BluRay"),
    ("remux", "Remux"),
    (r"blu[\s._\-]?ray|bd[\s._\-]?rip|brrip|bdremux", "BluRay"),
    (r"web[\s._\-]?dl|webdl", "WEB-DL"),
    (r"web[\s._\-]?rip|webrip", "WEBRip"),
    ("web", "WEB"),
    (r"hdtv|pdtv|sdtv", "HDTV"),
    (r"dvd[\s._\-]?rip|dvdr|dvd[\s._\-]?5|dvd[\s._\-]?9|dvd", "DVD"),
    (r"hd[\s._\-]?dvd", "HD-DVD"),
    (r"cd|flac|web[\s._\-]?flac", "CD"),
]);

const CODECS: Words = Words(&[
    (r"[hx][\s._]?265|hevc", "H.265"),
    (r"[hx][\s._]?264|avc", "H.264"),
    ("av1", "AV1"),
    ("xvid|divx", "XviD"),
    (r"mpeg[\s._\-]?2", "MPEG-2"),
    (r"vc[\s._\-]?1", "VC-1"),
    ("vp9", "VP9"),
]);

const HDR: Words = Words(&[
    (r"dv|dovi|dolby[\s._\-]?vision", "DV"),
    (r"hdr10[\s._]?(?:\+|plus)|hdr10p", "HDR10+"),
    ("hdr10", "HDR10"),
    ("hdr", "HDR"),
    ("hlg", "HLG"),
]);

const AUDIO: Words = Words(&[
    (r"dts[\s._\-]?hd[\s._\-]?ma|dts[\s._\-]?hd", "DTS-HD MA"),
    (r"dts[\s._\-]?x", "DTS:X"),
    ("dts", "DTS"),
    ("truehd", "TrueHD"),
    ("atmos", "Atmos"),
    (r"ddp|dd\+|eac3|e[\s._\-]?ac[\s._\-]?3|ddp\d[\s._]\d", "DDP"),
    (r"dd|ac3|dd\d[\s._]\d", "DD"),
    (r"aac(?:\d[\s._]\d)?", "AAC"),
    ("flac", "FLAC"),
    (r"mp3|320|v0", "MP3"),
    ("opus", "Opus"),
]);

static SEASON_EP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[\s._\-])s(\d{1,3})(?:[\s._\-]?e(\d{1,4}))?(?:$|[\s._\-e])").unwrap());
static XEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|[\s._\-])(\d{1,2})x(\d{2,3})(?:$|[\s._\-])").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:19|20)\d{2}$").unwrap());
static GROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-([A-Za-z0-9_\.]+?)(?:\.(?:mkv|mp4|avi|torrent|nzb))?(?:\s*\[[^\]]*\])?$").unwrap());
static PROPER: LazyLock<Regex> = LazyLock::new(|| word("proper|repack|rerip"));
static QUALITY_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\s._\-\(\[](?:(?:19|20)\d{2}|s\d{1,3}(?:e\d+)?|\d{1,2}x\d{2,3}|2160p|1080[pi]|720p|576p|480p|4k|uhd|web|webrip|web-dl|bluray|blu-ray|hdtv|dvdrip|remux|complete|proper|repack)(?:$|[\s._\-\)\]])").unwrap()
});

impl Name {
    /// Reads `release`.
    pub fn parse(release: &str) -> Name {
        let s = release.trim();
        let (season, episode) = match SEASON_EP.captures(s) {
            Some(c) => (c.get(1).and_then(|m| m.as_str().parse().ok()), c.get(2).and_then(|m| m.as_str().parse().ok())),
            None => match XEP.captures(s) {
                Some(c) => (c.get(1).and_then(|m| m.as_str().parse().ok()), c.get(2).and_then(|m| m.as_str().parse().ok())),
                None => (None, None),
            },
        };
        // the last year-like number that isn't the start of the name (`2012.2009.1080p` is the film 2012 from 2009)
        let year = s
            .split(|c: char| c.is_whitespace() || "._-()[]".contains(c))
            .filter(|w| !w.is_empty())
            .skip(1)
            .filter(|w| YEAR.is_match(w))
            .last()
            .and_then(|w| w.parse().ok());
        let title_end = QUALITY_START.find(s).map(|m| m.start()).unwrap_or(s.len());
        let title = s[..title_end].replace(['.', '_'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
        let group = GROUP.captures(s).map(|c| c[1].trim_end_matches('.').to_string()).unwrap_or_default();
        let mut hdr = HDR.all(s);
        if hdr.iter().any(|h| h.starts_with("HDR10")) {
            hdr.retain(|h| h != "HDR");
        }
        if hdr.contains(&"HDR10+".to_string()) {
            hdr.retain(|h| h != "HDR10");
        }
        let mut audio = AUDIO.all(s);
        if audio.iter().any(|a| a.starts_with("DTS-") || a == "DTS:X") {
            audio.retain(|a| a != "DTS");
        }
        if audio.contains(&"DDP".to_string()) {
            audio.retain(|a| a != "DD");
        }
        let mut source = SOURCES.first(s);
        if source == "Remux" && RESOLUTIONS.first(s) == "2160p" {
            source = "UHD BluRay".into();
        }
        Name {
            title,
            year,
            season,
            episode,
            resolution: RESOLUTIONS.first(s),
            source,
            codec: CODECS.first(s),
            hdr,
            audio,
            group,
            proper: PROPER.is_match(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_tv_release() {
        let n = Name::parse("Show.Name.S01E02.1080p.WEB-DL.DDP5.1.H.264-GROUP");
        assert_eq!(n.title, "Show Name");
        assert_eq!((n.season, n.episode), (Some(1), Some(2)));
        assert_eq!(n.resolution, "1080p");
        assert_eq!(n.source, "WEB-DL");
        assert_eq!(n.codec, "H.264");
        assert_eq!(n.audio, vec!["DDP"]);
        assert_eq!(n.group, "GROUP");
        assert_eq!(n.year, None);
    }

    #[test]
    fn reads_a_movie_release() {
        let n = Name::parse("Movie.Title.2023.2160p.UHD.BluRay.REMUX.DV.HDR10.HEVC.TrueHD.Atmos-FraMeSToR");
        assert_eq!(n.title, "Movie Title");
        assert_eq!(n.year, Some(2023));
        assert_eq!(n.resolution, "2160p");
        assert_eq!(n.source, "UHD BluRay");
        assert_eq!(n.codec, "H.265");
        assert_eq!(n.hdr, vec!["DV", "HDR10"]);
        assert_eq!(n.audio, vec!["TrueHD", "Atmos"]);
        assert_eq!(n.group, "FraMeSToR");
        let n = Name::parse("2012.2009.1080p.BluRay.x264-GRP");
        assert_eq!((n.title.as_str(), n.year), ("2012", Some(2009)));
    }

    #[test]
    fn reads_season_packs_and_propers() {
        let n = Name::parse("Show Name S03 720p HDTV x265 PROPER-grp");
        assert_eq!((n.season, n.episode), (Some(3), None));
        assert!(n.proper);
        assert_eq!(n.codec, "H.265");
        let n = Name::parse("Show.Name.2x05.HDTV.XviD-LOL");
        assert_eq!((n.season, n.episode), (Some(2), Some(5)));
        assert_eq!(n.codec, "XviD");
    }
}
