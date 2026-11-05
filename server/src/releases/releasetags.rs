// Ported from autobrr v1.90.0 (internal/domain/releasetags.go), GPL-2.0-or-later.
//! Reading a tracker's release tags (`FLAC / Lossless / Log / 100% / Cue / CD`, `MKV | H.264 | WEB-DL | 1080p`):
//! the audio, bitrate, format, log, cue, bonus, channels, codec, container, HDR, origin, other, resolution and
//! source they name.
//!
//! autobrr walks its tag kinds in Go map order (random); every one of its tests gives the same answer in any
//! order, and Lantern walks them in the order autobrr declares them.

use super::rls::goregex;
use regex::Regex;
use std::sync::LazyLock;

/// One tag autobrr knows in release tags.
struct TagInfo {
    /// The tag.
    tag: &'static str,
    /// `(?i)(?:RE)`, compiled.
    re: Regex,
}

impl TagInfo {
    /// The tag `tag` found by `re` (empty: the tag itself).
    fn new(tag: &'static str, re: &str) -> TagInfo {
        let re = if re.is_empty() { format!(r"\Q{tag}\E") } else { re.to_string() };
        TagInfo { tag, re: goregex::compile(&format!("(?i)(?:{re})")) }
    }
}

/// The tag kinds, each with its tags in autobrr's order.
static TYPES: LazyLock<Vec<(&'static str, Vec<TagInfo>)>> = LazyLock::new(|| {
    let audio = vec![
        ("24BIT", "(?-i:24BIT)"),
        ("24BIT Lossless", "(?:24BIT lossless)"),
        ("16BIT", "(?-i:16BIT)"),
        ("320", r"320[\\-\\._ kbps]?"),
        ("256", r"256[\\-\\._ kbps]?"),
        ("192", r"192[\\-\\._ kbps]?"),
        ("128", r"128[\\-\\._ kbps]?"),
        ("AAC-LC", r"aac[\-\._ ]?lc"),
        ("AAC", ""),
        ("AC3D", r"ac[\-\._ ]?3d"),
        ("Atmos", ""),
        ("APS (VBR)", ""),
        ("APX (VBR)", ""),
        ("CBR", ""),
        ("Cue", ""),
        ("DDPA", r"dd[p\+]a"),
        ("DDP", r"dd[p\+]|e[\-\._ ]?ac3"),
        ("DD", r"dd|ac3|dolby[\-\._ ]?digital"),
        ("DTS-HD.HRA", r"dts[\-\._ ]?hd[\-\._ ]?hra"),
        ("DTS-HD.HR", r"dts[\-\._ ]?hd[\-\._ ]?hr"),
        ("DTS-HD.MA", r"dts[\-\._ ]?hd[\-\._ ]?ma"),
        ("DTS-HD", r"dts[\-\._ ]?hd[\-\._ ]?"),
        ("DTS-MA", r"dts[\-\._ ]?ma[\-\._ ]?"),
        ("DTS-X", r"dts[\-\._ ]?x"),
        ("DTS", ""),
        ("DUAL.AUDIO", r"dual(?:[\-\._ ]?audio)?"),
        ("EAC3D", ""),
        ("ES", "(?-i:ES)"),
        ("EX", "(?-i:EX)"),
        ("FLAC", ""),
        ("LiNE", "(?-i:L[iI]NE)"),
        ("Lossless", "(?i:(?:^|[^t] )Lossless)"),
        ("LogScore", r"log\s?(?:\(|\s)(\d+)%\)?"),
        ("Log", "(?:log)"),
        ("LPCM", ""),
        ("MP3", ""),
        ("OGG", ""),
        ("OPUS", ""),
        ("TrueHD", r"(?:dolby[\-\._ ]?)?true[\-\._ ]?hd"),
        ("VBR", ""),
        ("V0 (VBR)", ""),
        ("V1 (VBR)", ""),
        ("V2 (VBR)", ""),
        ("DSD", ""),
        ("DSD64", ""),
        ("DSD128", ""),
        ("DSD256", ""),
        ("DSD512", ""),
    ];
    let audio_bitrate = vec![
        ("24BIT", "(?-i:24BIT)"),
        ("24BIT Lossless", "(?:24BIT lossless)"),
        ("16BIT", "(?-i:16BIT)"),
        ("320", r"320[\\-\\._ kbps]?"),
        ("256", r"256[\\-\\._ kbps]?"),
        ("192", r"192[\\-\\._ kbps]?"),
        ("128", r"128[\\-\\._ kbps]?"),
        ("APS (VBR)", ""),
        ("APX (VBR)", ""),
        ("CBR", ""),
        ("Lossless", "(?i:(?:^|[^t] )Lossless)"),
        ("VBR", ""),
        ("V0 (VBR)", ""),
        ("V1 (VBR)", ""),
        ("V2 (VBR)", ""),
        ("DSD64", ""),
        ("DSD128", ""),
        ("DSD256", ""),
        ("DSD512", ""),
    ];
    let audio_format = vec![
        ("AAC-LC", r"aac[\-\._ ]?lc"),
        ("AAC", ""),
        ("AC3D", r"ac[\-\._ ]?3d"),
        ("Atmos", ""),
        ("DDPA", r"dd[p\+]a"),
        ("DDP", r"dd[p\+]|e[\-\._ ]?ac3"),
        ("DD", r"dd|ac3|dolby[\-\._ ]?digital"),
        ("DTS-HD.HRA", r"dts[\-\._ ]?hd[\-\._ ]?hra"),
        ("DTS-HD.HR", r"dts[\-\._ ]?hd[\-\._ ]?hr"),
        ("DTS-HD.MA", r"dts[\-\._ ]?hd[\-\._ ]?ma"),
        ("DTS-HD", r"dts[\-\._ ]?hd[\-\._ ]?"),
        ("DTS-MA", r"dts[\-\._ ]?ma[\-\._ ]?"),
        ("DTS-X", r"dts[\-\._ ]?x"),
        ("DTS", ""),
        ("EAC3D", ""),
        ("ES", "(?-i:ES)"),
        ("EX", "(?-i:EX)"),
        ("FLAC", ""),
        ("LPCM", ""),
        ("MP3", ""),
        ("OGG", ""),
        ("OPUS", ""),
        ("TrueHD", r"(?:dolby[\-\._ ]?)?true[\-\._ ]?hd"),
        ("DSD", ""),
    ];
    let audio_extra =
        vec![("Cue", ""), ("Log100", r"(log 100%|log \(100%\))"), ("LogScore", r"log\s?(?:\(|\s)(\d+)%\)?"), ("Log", "(?:log)")];
    let bonus = vec![("Freeleech", "freeleech")];
    let channels = vec![
        ("7.1", r"7\.1(?:[\-\._ ]?audios)?"),
        ("6.1", r"6\.1(?:[\-\._ ]?audios)?"),
        ("6.0", r"6\.0(?:[\-\._ ]?audios)?"),
        ("5.1", r"5\.1(?:[\-\._ ]?audios)?"),
        ("5.0", r"5\.0(?:[\-\._ ]?audios)?"),
        ("4.1", r"4\.1(?:[\-\._ ]?audios)?"),
        ("4.0", r"4\.0(?:[\-\._ ]?audios)?"),
        ("3.1", r"3\.1(?:[\-\._ ]?audios)?"),
        ("3.0", r"3\.0(?:[\-\._ ]?audios)?"),
        ("2.1", r"2\.1(?:[\-\._ ]?audios)?"),
        ("2.0", r"2\.0(?:[\-\._ ]?audios)?"),
        ("1.0", r"1\.0(?:[\-\._ ]?audios)?"),
    ];
    let codecs = vec![
        ("DiVX.SBC", r"(?:divx[\-\._ ]?)?sbc"),
        ("x264.HQ", r"x[\\-\\._ ]?264[\\-\\._ ]?hq"),
        ("MPEG-2", r"mpe?g(?:[\-\._ ]?2)?"),
        ("H.265", r"h[\-\._ ]?265"),
        ("H.264", r"h[\-\._ ]?264"),
        ("H.263", r"h[\-\._ ]?263"),
        ("H.262", r"h[\-\._ ]?2[26]2"),
        ("H.261", r"h[\-\._ ]?261"),
        ("dxva", ""),
        ("HEVC", ""),
        ("VC-1", r"vc[\-\._ ]?1"),
        ("x265", r"x[\-\._ ]?265"),
        ("x264", r"x[\-\._ ]?264"),
        ("XViD", ""),
        ("AVC", r"avc(?:[\-\._ ]?1)?"),
        ("VP9", r"vp[\-\._ ]?9"),
        ("VP8", r"vp[\-\._ ]?8"),
        ("VP7", r"vp[\-\._ ]?7"),
    ];
    let container = vec![
        ("avi", ""),
        ("img", ""),
        ("iso", r"\biso\b"),
        ("mkv", ""),
        ("mov", ""),
        ("mp4", ""),
        ("mpg", "mpe?g"),
        ("m2ts", ""),
        ("vob", ""),
    ];
    let hdr = vec![
        ("HDR10+", r"hdr[\-\.]?10\+|10\+[\-\.]?bit|hdr10plus|hi10p"),
        ("HDR10", r"hdr[\-\.]?10|10[\-\.]?bit|hi10"),
        ("HDR+", r"hdr\+"),
        ("HDR", ""),
        ("SDR", ""),
        ("DV", r"(?i:dolby[\-\._ ]vision|dovi|\Qdv\E\b)"),
    ];
    let other = vec![("HYBRID", ""), ("REMUX", ""), ("REPACK", "repack(?:ed)?"), ("REREPACK", "rerepack(?:ed)?")];
    let origin = vec![("P2P", ""), ("Scene", ""), ("O-Scene", ""), ("Internal", ""), ("User", "")];
    let source = vec![
        ("Cassette", ""),
        ("CD", r"cd[\-\._ ]?(?:album)?"),
        ("BDRiP", r"b[dr]?[\-\._ ]?rip"),
        ("BDSCR", r"b[dr][\-\._ ]?scr(?:eener)?"),
        ("BluRay3D", r"blu[\-\._ ]?ray[\-\._ ]?3d|bd3d"),
        ("BluRayRiP", ""),
        ("BluRay", r"blu[\-\._ ]?ray|bd"),
        ("BRDRip", ""),
        ("DAT", "(?-i:DAT)"),
        ("DVBRiP", r"dvb[\-\._ ]?rip"),
        ("DVDA", ""),
        ("DVDRiP", r"dvd[\-\._ ]?rip"),
        ("DVDSCRRiP", r"(?:dvd[\-\._ ]?)?scr(?:eener)?[\-\._ ]?rip"),
        ("DVDSCR", r"(?:dvd[\-\._ ]?)?scr(?:eener)?"),
        ("DVDS", "dvds(?:ingle)?"),
        ("DVD", "dvd"),
        ("SACD", ""),
        ("RADIO", "(?-i:R[aA]D[iI][oO])"),
        ("SATRiP", r"sat[\-\._ ]?rip"),
        ("SAT", "(?-i:SAT)"),
        ("SBD", "(?-i:SBD|DAB|Soundboard)"),
        ("UHD.BDRiP", r"uhd[\-\._ ]?(?:bd)?rip"),
        ("UHD.BluRay", r"uhd[\-\._ ]?(?:blu[\-\._ ]?ray|bd)"),
        ("UHDTV", ""),
        ("UMDMOVIE", ""),
        ("Vinyl", "vinyl|vl"),
        ("WEB-DL", r"web[\-\._ ]?dl"),
        ("WEB-HD", r"web[\-\._ ]?hd"),
        ("WEBFLAC", ""),
        ("WebHDRiP", ""),
        ("WEBRiP", r"web[\-\._ ]?rip"),
        ("WEBSCR", r"web[\-\._ ]?scr(?:eener)?"),
        ("WebUHD", ""),
        ("WEB", ""),
    ];
    let resolution = vec![
        ("PN", r"p(?:al)?[\-\._ ]?n(?:tsc)?[\-\._ ]selector"),
        ("DCI4K", r"dci[\-\._ ]?4k|4096x2160"),
        ("DCI2K", r"dci[\-\._ ]?2k|2048x1080"),
        ("4320p", "4320p|7680x4320"),
        ("2880p", "2880p|5k|5120x2880"),
        ("2160p", "2160p|3840x2160|uhd|4k"),
        ("1800p", "1800p|3200x1800"),
        ("1440p", "1440p|2560x1440"),
        ("1080p", "1080[ip]|1920x1080"),
        ("900p", "900[ip]|1600x900"),
        ("720p", "720[ip]|1280x720"),
        ("576p", "576[ip]|720x576|pal"),
        ("540p", "540[ip]|960x540"),
        ("480p", "480[ip]|720x480|848x480|854x480|ntsc"),
        ("360p", "360[ip]|640x360"),
        ("$1p", r"([123]\d{3})p"),
    ];
    let build = |l: Vec<(&'static str, &'static str)>| l.into_iter().map(|(t, r)| TagInfo::new(t, r)).collect::<Vec<_>>();
    vec![
        ("audio", build(audio)),
        ("audioBitrate", build(audio_bitrate)),
        ("audioFormat", build(audio_format)),
        ("audioExtra", build(audio_extra)),
        ("bonus", build(bonus)),
        ("channels", build(channels)),
        ("codecs", build(codecs)),
        ("container", build(container)),
        ("hdr", build(hdr)),
        ("other", build(other)),
        ("origin", build(origin)),
        ("source", build(source)),
        ("resolution", build(resolution)),
    ]
});

/// What a tracker's release tags say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReleaseTags {
    /// Every audio tag found (`Cue`, `FLAC`, `Lossless`, `Log100`, `Log`).
    pub audio: Vec<String>,
    /// The bitrate (`Lossless`, `320`, `V0 (VBR)`).
    pub audio_bitrate: String,
    /// The format (`FLAC`, `MP3`).
    pub audio_format: String,
    /// The log score (`100`).
    pub log_score: i64,
    /// Whether there is a log.
    pub has_log: bool,
    /// Whether there is a cue file.
    pub has_cue: bool,
    /// Bonuses (`Freeleech`).
    pub bonus: Vec<String>,
    /// The channels (`5.1`).
    pub channels: String,
    /// The codec.
    pub codec: String,
    /// The container (`mkv`).
    pub container: String,
    /// HDR formats.
    pub hdr: Vec<String>,
    /// The origin (`Scene`, `P2P`, `Internal`).
    pub origin: String,
    /// Other tags (`REMUX`).
    pub other: Vec<String>,
    /// The resolution.
    pub resolution: String,
    /// The source.
    pub source: String,
}

/// Notes the tag `info` of kind `typ` in `rt`; whether to keep looking at this kind's other tags.
fn note(rt: &mut ReleaseTags, typ: &str, tag: &str) -> bool {
    match typ {
        "audio" => rt.audio.push(tag.into()),
        "audioBitrate" => rt.audio_bitrate = tag.into(),
        "audioFormat" => rt.audio_format = tag.into(),
        "bonus" => rt.bonus.push(tag.into()),
        "hdr" => rt.hdr.push(tag.into()),
        "other" => rt.other.push(tag.into()),
        "channels" => {
            rt.channels = tag.into();
            return false;
        }
        "codecs" => {
            rt.codec = tag.into();
            return false;
        }
        "container" => {
            rt.container = tag.into();
            return false;
        }
        "origin" => {
            rt.origin = tag.into();
            return false;
        }
        "source" => {
            rt.source = tag.into();
            return false;
        }
        "resolution" => {
            rt.resolution = tag.into();
            return false;
        }
        _ => return false,
    }
    true
}

/// Reads a list of tags, one by one (autobrr's `ParseReleaseTags`).
pub fn parse_release_tags(tags: &[String]) -> ReleaseTags {
    let mut rt = ReleaseTags::default();
    for tag in tags {
        for (typ, infos) in TYPES.iter() {
            for info in infos {
                if info.tag == "LogScore" {
                    // autobrr wants three groups here, and the pattern has two: it never sets the score
                    if let Some(c) = info.re.captures(tag) {
                        if c.len() == 3 {
                            rt.log_score = c[2].parse().unwrap_or(0);
                        }
                    }
                    continue;
                }
                if info.re.is_match(tag) && !note(&mut rt, typ, info.tag) {
                    break;
                }
            }
        }
    }
    rt
}

/// Reads a release tag string (autobrr's `ParseReleaseTagString`).
pub fn parse_release_tag_string(tags: &str) -> ReleaseTags {
    let mut rt = ReleaseTags::default();
    if tags.is_empty() {
        return rt;
    }
    for (typ, infos) in TYPES.iter() {
        for info in infos {
            if !info.re.is_match(tags) {
                continue;
            }
            if info.tag == "Log" {
                rt.has_log = true;
                rt.audio.push(info.tag.into());
                continue;
            }
            if info.tag == "LogScore" {
                if let Some(c) = info.re.captures(tags) {
                    if c.len() == 2 {
                        let score: i64 = c[1].parse().unwrap_or(0);
                        rt.has_log = true;
                        rt.log_score = score;
                        rt.audio.push(format!("Log{score}"));
                    }
                }
                continue;
            }
            if *typ == "audio" && info.tag == "Cue" {
                rt.has_cue = true;
            }
            if !note(&mut rt, typ, info.tag) {
                break;
            }
        }
    }
    rt
}

/// The delimiters between release tags, with the spaces around them.
static TAGS_DELIMITER: LazyLock<Regex> = LazyLock::new(|| goregex::compile(r"\s*[|/,]\s*"));

/// Release tags with their `|`, `/` and `,` delimiters as single spaces (autobrr's `CleanReleaseTags`).
pub fn clean_release_tags(tags: &str) -> String {
    TAGS_DELIMITER.replace_all(tags, " ").into_owned()
}
