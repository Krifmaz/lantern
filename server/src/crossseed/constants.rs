// Ported from cross-seed v6.13.9 (src/constants.ts), Apache-2.0.
//! cross-seed's constants: the patterns that read release names, the file endings of each kind of media, and the
//! names of decisions, match modes, link types, actions, results and block list entries. The patterns are
//! cross-seed's JavaScript ones word for word, read through [`super::jsre`].

use super::jsre::{re, test};
use fancy_regex::Regex;
use std::sync::LazyLock;

/// The tag cross-seed puts on what it injects (and the Deluge label).
pub const TORRENT_TAG: &str = "cross-seed";
/// The category suffix of injected torrents: apps watch their own category, so they don't import these.
pub const TORRENT_CATEGORY_SUFFIX: &str = ".cross-seed";

/// A compiled pattern made once, the first time it is used.
type Pattern = LazyLock<Regex>;

/// `EP_REGEX`: an episode (`Show.S01E02`, `Show.2024.05.06`).
pub static EP_REGEX: Pattern = LazyLock::new(|| {
    re(
        r"^(?<title>.+?)[_.\s-]+(?:(?<season>S\d+)?[_.\s-]{0,3}(?!(?:19|20)\d{2})(?<episode>(?:E|(?<=S\d+[_\s-]{1,3}))\d+(?:[\s-]?(?!(?:19|20)\d{2})E?\d+)?(?![pix]))(?!\d+[pix])|(?<date>(?<year>(?:19|20)\d{2})[_.\s-](?<month>\d{2})[_.\s-](?<day>\d{2})))",
        "i",
    )
});
/// `BAD_EP_REGEX`: an episode with no title before it.
pub static BAD_EP_REGEX: Pattern = LazyLock::new(|| {
    re(
        r"^[_.\s-]*[^_.\s-]*?(?:(?<season>S\d+)?[_.\s-]{0,3}(?!(?:19|20)\d{2})(?<episode>(?:E|(?<=S\d+[_\s-]{1,3}))\d+(?:[\s-]?(?!(?:19|20)\d{2})E?\d+)?(?![pix]))(?!\d+[pix])|(?<date>(?<year>(?:19|20)\d{2})[_.\s-](?<month>\d{2})[_.\s-](?<day>\d{2})))",
        "i",
    )
});
/// `IS_MULTI_EP_REGEX`: more than one episode (`E01E02`, `E01-02`).
pub static IS_MULTI_EP_REGEX: Pattern = LazyLock::new(|| re(r"E\d+(?:[-.]?S\d+E\d|[-.]?E\d|[-.]\d)", "i"));
/// `SEASON_REGEX`: a season pack (`Show.S01`, `Show Season 2`).
pub static SEASON_REGEX: Pattern =
    LazyLock::new(|| re(r"^(?<title>.+?)[[(_.\s-]+(?<season>S(?:eason)?\s*\d+)(?=\b(?![_.\s~-]*E\d+))", "i"));
/// `BAD_SEASON_REGEX`: a season with no title before it.
pub static BAD_SEASON_REGEX: Pattern =
    LazyLock::new(|| re(r"^[[_.\s-]*[^[_.\s-]*?(?<season>S\d+|(?:season)\s*\d+)(?=\b(?![_.\s~-]*E\d+))", "i"));
/// `MOVIE_REGEX`: a title and a year.
pub static MOVIE_REGEX: Pattern = LazyLock::new(|| re(r"^(?<title>.+?)-?[_.\s][[(]?(?<year>(?:18|19|20)\d{2})[)\]]?(?![pix])", "i"));
/// `ANIME_REGEX`: `[Group] Title - 01`.
pub static ANIME_REGEX: Pattern = LazyLock::new(|| {
    re(
        r"^(?:\[(?<group>.*?)\][_\s-]?)?(?:\[?(?<title>.+?)[_\s-]?(?:\(?(?:\d{1,2}(?:st|nd|rd|th))?\s?Season)?[_\s-]?\]?)(?:[([~/|-]\s?(?!\d{1,4})(?<altTitle>.+?)[)\]~-]?\s?)?[_\s-]?(?:[[(]?(?<year>(?:19|20)\d{2})[)\]]?)?[[_\s-](?:S\d{1,2})?[_\s-]{0,3}(?:#|EP?|(?:SP))?[_\s-]{0,3}(?!\d+[a-uw-z])(?<release>\d{1,4})(?!\.[0-46-9])",
        "i",
    )
});
/// `RELEASE_GROUP_REGEX`: the group after the last `-`.
pub static RELEASE_GROUP_REGEX: Pattern =
    LazyLock::new(|| re(r"(?<=-)(?:\W|\b)(?!(?:\d{3,4}[ip]))(?!\d+\b)(?:\W|\b)(?<group>[\w .]+?)(?:\[.+\])?(?:\))?(?:\s\[.+\])?$", "i"));
/// `ANIME_GROUP_REGEX`: `[Group]` at the start.
pub static ANIME_GROUP_REGEX: Pattern = LazyLock::new(|| re(r"^\s*\[(?<group>.+?)\]", "i"));
/// `EBOOK_AND_MUSIC_RELEASE_REGEX`: what is left out of a book's or an album's name to search for it.
pub static EBOOK_AND_MUSIC_RELEASE_REGEX: Pattern = LazyLock::new(|| {
    re(
        r"(\d+k?\b|['']s\b|\[.*?]|\(.*?\)|\{.*?}|[kbps]{2,4}\b|m4b\b|pdf\b|docx\b|epub\b|mobi\b|azw3\b|m4a\b|mp3\b|flac|\sWEB\b|audiobook\b|\bebook\b|\bNew\b|[a\W]+(novela?|saga|series)|(Read|Narrated)?\W?By\W\w+\W*\w*\b|[^a-zA-Z0-9])",
        "i",
    )
});
/// `RESOLUTION_REGEX`: any resolution (`1080p`, `1920x1080`).
pub static RESOLUTION_REGEX: Pattern = LazyLock::new(|| re(r"\b(?<res>\d{3,4}[pix](?:\d{3,4}[pi]?)?)\b", "i"));
/// `RES_STRICT_REGEX`: 2160, 1080 or 720 with `p` or `i` (case matters).
pub static RES_STRICT_REGEX: Pattern = LazyLock::new(|| re(r"(?<res>(?:2160|1080|720)[pi])", ""));
/// `YEARS_REGEX`: every year in a name.
pub static YEARS_REGEX: Pattern = LazyLock::new(|| re(r"(?<year>(?:19|20)\d{2})(?![pix])", "gi"));
/// `REPACK_PROPER_REGEX`: a later release of the same thing (`REPACK`, `PROPER`, `2v2`).
pub static REPACK_PROPER_REGEX: Pattern = LazyLock::new(|| re(r"(?:\b(?<type>(?:REPACK|PROPER|\dv\d)\d?))\b", "i"));
/// `ARR_PROPER_REGEX`: what Sonarr and Radarr call a proper.
pub static ARR_PROPER_REGEX: Pattern = LazyLock::new(|| re(r"(?:\b(?<arrtype>(?:Proper|\dv\d)))\b", ""));
/// `SCENE_TITLE_REGEX`: a title after an optional scene prefix (`abc-`).
pub static SCENE_TITLE_REGEX: Pattern = LazyLock::new(|| re(r"^(?:[a-z0-9]{3,5}-)?(?<title>.*)", ""));
/// `ARR_DIR_REGEX`: a folder Sonarr or Radarr made (`Movie (2020) {tmdb-123}`).
pub static ARR_DIR_REGEX: Pattern = LazyLock::new(|| {
    re(
        r"^(?<title>(?!.*(?:(\d{3,4}[ipx])|([xh.]+26[4-6])|(dvd)|(mpeg)|(xvid)|(?:(he)|a)vc|(?:uhd)|(?:blu[_.\s-]?ray)))[\p{L}\s:\w'’!();.,&–+-]+(?:\(\d{4}\))?)(?<id>\s[{[](?:tm|tv|im)db(?:id)?-\w+?[}\]])?$",
        "iu",
    )
});
/// `SONARR_SUBFOLDERS_REGEX`: `Season 01` or `S01`.
pub static SONARR_SUBFOLDERS_REGEX: Pattern = LazyLock::new(|| re(r"^(?:S(?:eason )?(?<seasonNum>\d{1,4}))$", "i"));
/// `NON_UNICODE_ALPHANUM_REGEX`: everything but letters and digits.
pub static NON_UNICODE_ALPHANUM_REGEX: Pattern = LazyLock::new(|| re(r"[^\p{L}\p{N}]+", "giu"));
/// `CALIBRE_INDEXNUM_REGEX`: Calibre's ` (12)` after a name.
pub static CALIBRE_INDEXNUM_REGEX: Pattern = LazyLock::new(|| re(r"\s?\(\d+\)$", ""));
/// `INFO_HASH_REGEX`: 40 letters or digits.
pub static INFO_HASH_REGEX: Pattern = LazyLock::new(|| re(r"^[a-z0-9]{40}$", "i"));
/// `BAD_GROUP_PARSE_REGEX`: what the release group pattern finds that isn't a group (`1080p`, `x264`, `DD5.1`).
pub static BAD_GROUP_PARSE_REGEX: Pattern = LazyLock::new(|| {
    re(
        r"^(?<badmatch>(?:dl|DDP?|aac|eac3|atmos|dts|ma|hd|[heav.c]{3.5}|[xh][xh.]?[2456]|[0-9][ip]?|dxva|full|blu|ray|s(?:eason)?\W\d|\W){3,})$",
        "i",
    )
});
/// `ABS_WIN_PATH_REGEX`: a Windows path from a drive or a root.
pub static ABS_WIN_PATH_REGEX: Pattern = LazyLock::new(|| re(r"^[a-z]:|^\\", "i"));
/// `AKA_REGEX`: "a.k.a." between two titles.
pub static AKA_REGEX: Pattern = LazyLock::new(|| re(r"(?:[_.\s-]+|\b)a[_.\s-]?k[_.\s-]?a(?:[_.\s-]+|\b)", "i"));
/// `ALL_SPACES_REGEX`: runs of white space.
pub static ALL_SPACES_REGEX: Pattern = LazyLock::new(|| re(r"\s+", "g"));
/// `ALL_SQUARE_BRACKETS_REGEX`: bracketed text of every kind.
pub static ALL_SQUARE_BRACKETS_REGEX: Pattern = LazyLock::new(|| re(r"\[.*?\]|「.*?」|｢.*?｣|【.*?】", "g"));
/// `ALL_PARENTHESES_REGEX`: text in parentheses.
pub static ALL_PARENTHESES_REGEX: Pattern = LazyLock::new(|| re(r"\(.*?\)", "g"));

/// The streaming sources, in cross-seed's order (the first that fits wins).
static SOURCE_REGEXES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("AMZN", re(r"\b(amzn|amazon(hd)?)\b[ ._-]web[ ._-]?(dl|rip)?\b", "i")),
        ("DSNP", re(r"\b(dsnp|dsny|disney)\b", "i")),
        ("NF", re(r"\b(nf|netflix(u?hd)?)\b", "i")),
        ("HULU", re(r"\b(hulu)\b", "i")),
        ("ATVP", re(r"\b(atvp|aptv)\b", "i")),
        ("HBO", re(r"\b(hbo)(?![ ._-]max)\b|\b(hmax|hbom|hbo[ ._-]max)\b", "i")),
        ("PCOK", re(r"\b(pcok)\b", "i")),
        ("PMTP", re(r"\b(pmtp|Paramount Plus)\b", "i")),
    ]
});

/// `parseSource`: the streaming source a name names (`NF`, `AMZN`, ...), if any.
pub fn parse_source(title: &str) -> Option<&'static str> {
    SOURCE_REGEXES.iter().find(|(_, r)| test(r, title)).map(|(s, _)| *s)
}

/// `sourceRegexRemove`: the name with its first streaming source taken out.
pub fn source_regex_remove(title: &str) -> String {
    for (_, r) in SOURCE_REGEXES.iter() {
        let new = super::jsre::replace_first(r, title, "");
        if new.len() != title.len() {
            return new;
        }
    }
    title.to_string()
}

/// Video file endings (cross-seed's and Sonarr's).
pub const VIDEO_EXTENSIONS: &[&str] = &[
    ".mkv", ".mp4", ".avi", ".ts", ".m4v", ".3gp", ".nsv", ".ty", ".strm", ".rm", ".rmvb", ".mov", ".qt", ".divx", ".xvid", ".bivx",
    ".pva", ".wmv", ".asf", ".asx", ".ogm", ".ogv", ".m2v", ".dvr-ms", ".mpg", ".mpeg", ".avc", ".vp3", ".svq3", ".nuv", ".viv", ".dv",
    ".fli", ".flv", ".wpl", ".wtv",
];
/// Video disc file endings.
pub const VIDEO_DISC_EXTENSIONS: &[&str] = &[".m2ts", ".ifo", ".vob", ".bup"];
/// Audio file endings.
pub const AUDIO_EXTENSIONS: &[&str] =
    &[".wav", ".aiff", ".alac", ".flac", ".ape", ".mp3", ".aac", ".m4a", ".m4b", ".m4p", ".ogg", ".wma", ".aa", ".aax"];
/// Book file endings.
pub const BOOK_EXTENSIONS: &[&str] =
    &[".epub", ".mobi", ".azw", ".azw3", ".azw4", ".pdf", ".djvu", ".html", ".chm", ".cbr", ".cbz", ".cb7", ".cbt", ".cba"];

/// Every known ending, in cross-seed's order (video, audio, book, disc).
pub fn all_extensions() -> impl Iterator<Item = &'static str> {
    VIDEO_EXTENSIONS.iter().chain(AUDIO_EXTENSIONS).chain(BOOK_EXTENSIONS).chain(VIDEO_DISC_EXTENSIONS).copied()
}

/// `MAX_PATH_BYTES`: the longest file name.
pub const MAX_PATH_BYTES: usize = 255;
/// `MAX_ANIME_NAME_LENGTH`: [`ANIME_REGEX`] is slow on long names, and a file name can't be longer.
pub const MAX_ANIME_NAME_LENGTH: usize = MAX_PATH_BYTES;
/// `LEVENSHTEIN_DIVISOR`: two titles are alike within a third of their length in edits.
pub const LEVENSHTEIN_DIVISOR: usize = 3;
/// `MIN_VIDEO_QUERY_LENGTH`: a video search shorter than this uses more of the name.
pub const MIN_VIDEO_QUERY_LENGTH: usize = 3;

/// `MediaType`: what a release is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    /// One episode.
    Episode,
    /// A season pack.
    Season,
    /// A film.
    Movie,
    /// Anime.
    Anime,
    /// Other video.
    Video,
    /// Music or an audiobook.
    Audio,
    /// A book.
    Book,
    /// Anything else.
    Other,
}

impl MediaType {
    /// cross-seed's name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Episode => "episode",
            MediaType::Season => "pack",
            MediaType::Movie => "movie",
            MediaType::Anime => "anime",
            MediaType::Video => "video",
            MediaType::Audio => "audio",
            MediaType::Book => "book",
            MediaType::Other => "unknown",
        }
    }
}

/// `Action`: what is done with a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Keep the `.torrent` file.
    Save,
    /// Add it to the download client.
    Inject,
}

/// `InjectionResult`: how adding a match to the client went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionResult {
    /// Added.
    Success,
    /// The client refused it or couldn't be reached.
    Failure,
    /// The client already has it.
    AlreadyExists,
    /// The torrent it comes from isn't finished.
    TorrentNotComplete,
}

impl InjectionResult {
    /// cross-seed's name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            InjectionResult::Success => "INJECTED",
            InjectionResult::Failure => "FAILURE",
            InjectionResult::AlreadyExists => "ALREADY_EXISTS",
            InjectionResult::TorrentNotComplete => "TORRENT_NOT_COMPLETE",
        }
    }
}

/// `Decision`: what cross-seed made of a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    /// Every file's name and size are the same.
    Match,
    /// Every file's size is the same; names differ.
    MatchSizeOnly,
    /// Most of the files are the same (partial match mode).
    MatchPartial,
    /// The site's size is too far from ours.
    FuzzySizeMismatch,
    /// Some files are missing or have other sizes.
    SizeMismatch,
    /// Too many files are missing for a partial match.
    PartialSizeMismatch,
    /// The candidate has no download link.
    NoDownloadLink,
    /// Its `.torrent` couldn't be fetched.
    DownloadFailed,
    /// It is a magnet link.
    MagnetLink,
    /// The site's rate limit was reached.
    RateLimited,
    /// It is the very torrent we have.
    SameInfoHash,
    /// The client already has it.
    InfoHashAlreadyExists,
    /// The files are laid out differently.
    FileTreeMismatch,
    /// Another release group.
    ReleaseGroupMismatch,
    /// On the block list.
    BlockedRelease,
    /// One is a proper or a repack and the other isn't.
    ProperRepackMismatch,
    /// Another resolution.
    ResolutionMismatch,
    /// Another streaming source.
    SourceMismatch,
}

impl Decision {
    /// Every decision, in cross-seed's order.
    pub const ALL: [Decision; 18] = [
        Decision::Match,
        Decision::MatchSizeOnly,
        Decision::MatchPartial,
        Decision::FuzzySizeMismatch,
        Decision::SizeMismatch,
        Decision::PartialSizeMismatch,
        Decision::NoDownloadLink,
        Decision::DownloadFailed,
        Decision::MagnetLink,
        Decision::RateLimited,
        Decision::SameInfoHash,
        Decision::InfoHashAlreadyExists,
        Decision::FileTreeMismatch,
        Decision::ReleaseGroupMismatch,
        Decision::BlockedRelease,
        Decision::ProperRepackMismatch,
        Decision::ResolutionMismatch,
        Decision::SourceMismatch,
    ];

    /// cross-seed's name for it, as stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Match => "MATCH",
            Decision::MatchSizeOnly => "MATCH_SIZE_ONLY",
            Decision::MatchPartial => "MATCH_PARTIAL",
            Decision::FuzzySizeMismatch => "FUZZY_SIZE_MISMATCH",
            Decision::SizeMismatch => "SIZE_MISMATCH",
            Decision::PartialSizeMismatch => "PARTIAL_SIZE_MISMATCH",
            Decision::NoDownloadLink => "NO_DOWNLOAD_LINK",
            Decision::DownloadFailed => "DOWNLOAD_FAILED",
            Decision::MagnetLink => "MAGNET_LINK",
            Decision::RateLimited => "RATE_LIMITED",
            Decision::SameInfoHash => "SAME_INFO_HASH",
            Decision::InfoHashAlreadyExists => "INFO_HASH_ALREADY_EXISTS",
            Decision::FileTreeMismatch => "FILE_TREE_MISMATCH",
            Decision::ReleaseGroupMismatch => "RELEASE_GROUP_MISMATCH",
            Decision::BlockedRelease => "BLOCKED_RELEASE",
            Decision::ProperRepackMismatch => "PROPER_REPACK_MISMATCH",
            Decision::ResolutionMismatch => "RESOLUTION_MISMATCH",
            Decision::SourceMismatch => "SOURCE_MISMATCH",
        }
    }

    /// The decision stored as `s`.
    pub fn parse(s: &str) -> Option<Decision> {
        Decision::ALL.into_iter().find(|d| d.as_str() == s)
    }

    /// `isAnyMatchedDecision`: a match of any kind.
    pub fn is_any_match(self) -> bool {
        matches!(self, Decision::Match | Decision::MatchSizeOnly | Decision::MatchPartial)
    }

    /// `isStaticDecision`: decided from the name alone, so it never changes.
    pub fn is_static(self) -> bool {
        matches!(
            self,
            Decision::ReleaseGroupMismatch
                | Decision::ResolutionMismatch
                | Decision::SourceMismatch
                | Decision::ProperRepackMismatch
                | Decision::MagnetLink
        )
    }
}

/// `MatchMode`: how alike two torrents must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Every file's name and size.
    Strict,
    /// Every file's size (names may differ).
    Flexible,
    /// Most files' sizes.
    Partial,
}

impl MatchMode {
    /// The mode named `s`: cross-seed's three, and v5's `safe` (strict) and `risky` (flexible).
    pub fn parse(s: &str) -> Option<MatchMode> {
        match s {
            "strict" | "safe" => Some(MatchMode::Strict),
            "flexible" | "risky" => Some(MatchMode::Flexible),
            "partial" => Some(MatchMode::Partial),
            _ => None,
        }
    }

    /// cross-seed's name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            MatchMode::Strict => "strict",
            MatchMode::Flexible => "flexible",
            MatchMode::Partial => "partial",
        }
    }
}

/// `LinkType`: how matched files are linked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    /// A symbolic link.
    Symlink,
    /// A hard link.
    Hardlink,
    /// A copy-on-write link.
    Reflink,
    /// A copy-on-write link, or a copy where the file system has none.
    ReflinkOrCopy,
}

impl LinkType {
    /// The link type named `s`.
    pub fn parse(s: &str) -> Option<LinkType> {
        match s {
            "symlink" => Some(LinkType::Symlink),
            "hardlink" => Some(LinkType::Hardlink),
            "reflink" => Some(LinkType::Reflink),
            "reflinkOrCopy" => Some(LinkType::ReflinkOrCopy),
            _ => None,
        }
    }
}

/// `BlocklistType`: what a block list entry looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlocklistType {
    /// `name:` text in the name.
    Name,
    /// `nameRegex:` a pattern on the name.
    NameRegex,
    /// `folder:` text in the folder.
    Folder,
    /// `folderRegex:` a pattern on the folder.
    FolderRegex,
    /// `category:` the client category.
    Category,
    /// `tag:` a client tag.
    Tag,
    /// `tracker:` a tracker's host.
    Tracker,
    /// `infoHash:` one torrent.
    InfoHash,
    /// `sizeBelow:` smaller than this many bytes.
    SizeBelow,
    /// `sizeAbove:` larger than this many bytes.
    SizeAbove,
    /// No prefix (v5): name, info hash or folder.
    Legacy,
    /// A prefix cross-seed doesn't know.
    Unknown,
}

impl BlocklistType {
    /// The prefixes, in cross-seed's order.
    pub const NAMES: [&'static str; 11] =
        ["name", "nameRegex", "folder", "folderRegex", "category", "tag", "tracker", "infoHash", "sizeBelow", "sizeAbove", "legacy"];

    fn from_prefix(s: &str) -> BlocklistType {
        match s {
            "name" => BlocklistType::Name,
            "nameRegex" => BlocklistType::NameRegex,
            "folder" => BlocklistType::Folder,
            "folderRegex" => BlocklistType::FolderRegex,
            "category" => BlocklistType::Category,
            "tag" => BlocklistType::Tag,
            "tracker" => BlocklistType::Tracker,
            "infoHash" => BlocklistType::InfoHash,
            "sizeBelow" => BlocklistType::SizeBelow,
            "sizeAbove" => BlocklistType::SizeAbove,
            "legacy" => BlocklistType::Legacy,
            _ => BlocklistType::Unknown,
        }
    }
}

/// `parseBlocklistEntry`: an entry's type (what is before the first `:`) and value (the rest); an entry with no
/// `:` is a legacy one.
pub fn parse_blocklist_entry(entry: &str) -> (BlocklistType, String) {
    // `^(?<blocklistType>.+?):(?<blocklistValue>.*)$`: at least one character before the colon; `.` stops at a
    // line break, so an entry with one is legacy
    if entry.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return (BlocklistType::Legacy, entry.to_string());
    }
    match entry.char_indices().skip(1).find(|(_, c)| *c == ':') {
        Some((at, _)) => (BlocklistType::from_prefix(&entry[..at]), entry[at + 1..].to_string()),
        None => (BlocklistType::Legacy, entry.to_string()),
    }
}

/// `IGNORED_FOLDERS_SUBSTRINGS`: folders data-based searching leaves out.
pub const IGNORED_FOLDERS_SUBSTRINGS: &[&str] = &["sample", "proof", "bdmv", "bdrom", "certificate", "video_ts"];
/// `RESUME_EXCLUDED_KEYWORDS`: files that don't count when deciding to resume.
pub const RESUME_EXCLUDED_KEYWORDS: &[&str] = &["sample", "trailer", "extras", "bonus"];
/// `RESUME_EXCLUDED_EXTS`: file endings that don't count when deciding to resume.
pub const RESUME_EXCLUDED_EXTS: &[&str] = &[".nfo", ".srr", ".srt", ".txt", ".ass"];

#[cfg(test)]
mod tests {
    // Ported from cross-seed v6.13.9 (tests/constants.test.ts), Apache-2.0.
    use super::*;
    use std::time::Instant;

    #[test]
    fn ep_regex_matches_episode_titles() {
        assert!(test(&EP_REGEX, "My.Show.S01E01"));
    }

    #[test]
    fn ep_regex_does_not_match_non_episode_titles() {
        assert!(!test(&EP_REGEX, "My.Show.S01"));
    }

    #[test]
    fn bad_group_parse_regex_matches_what_is_not_a_release_group() {
        for text in ["1080p", "x264", "h.264", "DD5.1", "DL.DDP5.1", "eac3 5.1", "ma hd", "full blu ray", "Season 1 720p", "x2 6"] {
            assert!(test(&BAD_GROUP_PARSE_REGEX, text), "{text}");
        }
    }

    #[test]
    fn bad_group_parse_regex_does_not_match_release_groups() {
        for text in ["NTb", "FLUX", "SPARKS", "AMZN", "WEB-DL", "dts", "12", "season 1", "1080p.WEB.DL"] {
            assert!(!test(&BAD_GROUP_PARSE_REGEX, text), "{text}");
        }
    }

    #[test]
    fn bad_group_parse_regex_returns_quickly() {
        for text in ["1".repeat(30) + "x", ".4".repeat(30) + "x", ".x4".repeat(30) + "!z"] {
            let start = Instant::now();
            assert!(!test(&BAD_GROUP_PARSE_REGEX, &text));
            assert!(start.elapsed().as_millis() < 1000, "{text}");
        }
    }

    #[test]
    fn episodes_seasons_and_dates() {
        let c = super::super::jsre::captures(&EP_REGEX, "Show.Name.S01E02.1080p.WEB-GRP").unwrap();
        assert_eq!(&c["title"], "Show.Name");
        assert_eq!(&c["season"], "S01");
        assert_eq!(&c["episode"], "E02");
        let c = super::super::jsre::captures(&EP_REGEX, "Show S01 - 05 [1080p]").unwrap();
        assert_eq!(&c["episode"], "05", "a bare episode after a season");
        let c = super::super::jsre::captures(&EP_REGEX, "Daily.Show.2024.05.06.720p").unwrap();
        assert_eq!((&c["year"], &c["month"], &c["day"]), ("2024", "05", "06"));
        assert!(test(&SEASON_REGEX, "My.Show.Season 2"));
        assert!(!test(&SEASON_REGEX, "My.Show.S01E01"));
        assert!(test(&MOVIE_REGEX, "My.Movie.2021"));
        assert!(test(&IS_MULTI_EP_REGEX, "Show.S01E01E02"));
    }

    #[test]
    fn sources() {
        assert_eq!(parse_source("Show.S01E01.1080p.AMZN.WEB-DL-GRP"), Some("AMZN"));
        assert_eq!(parse_source("Show.S01E01.1080p.NF.WEB-DL-GRP"), Some("NF"));
        assert_eq!(parse_source("Show.S01E01.1080p.HBO.MAX.WEB-DL"), Some("HBO"));
        assert_eq!(parse_source("Show.S01E01.1080p.WEB-DL-GRP"), None);
        assert_eq!(source_regex_remove("Show NF WEB"), "Show  WEB");
    }

    #[test]
    fn block_list_entries() {
        assert_eq!(parse_blocklist_entry("name:x264"), (BlocklistType::Name, "x264".into()));
        assert_eq!(parse_blocklist_entry("tracker:a.b:8080"), (BlocklistType::Tracker, "a.b:8080".into()));
        assert_eq!(parse_blocklist_entry("category:"), (BlocklistType::Category, "".into()));
        assert_eq!(parse_blocklist_entry(":x"), (BlocklistType::Legacy, ":x".into()));
        assert_eq!(parse_blocklist_entry("Show.Name"), (BlocklistType::Legacy, "Show.Name".into()));
        assert_eq!(parse_blocklist_entry("bogus:x").0, BlocklistType::Unknown);
    }

    #[test]
    fn decisions_round_trip() {
        for d in Decision::ALL {
            assert_eq!(Decision::parse(d.as_str()), Some(d));
        }
        assert_eq!(Decision::ALL.iter().filter(|d| d.is_any_match()).count(), 3);
        assert_eq!(MatchMode::parse("safe"), Some(MatchMode::Strict));
        assert_eq!(MatchMode::parse("risky"), Some(MatchMode::Flexible));
        assert_eq!(MatchMode::parse("loose"), None);
    }
}
