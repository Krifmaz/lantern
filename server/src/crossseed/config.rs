// Ported from cross-seed v6.13.9 (src/runtimeConfig.ts, src/configSchema.ts, src/config.template.cjs), Apache-2.0.
//! cross-seed's settings: [`RuntimeConfig`] with the defaults of cross-seed's config template, read from Lantern's
//! settings (the `cross_seed_*` keys in settings.toml), one-off overrides as cross-seed's API and jobs pass them,
//! and [`validate`], which checks them with cross-seed's own messages.
//!
//! | cross-seed | Lantern setting |
//! | --- | --- |
//! | `matchMode` | `cross_seed_match_mode` |
//! | `fuzzySizeThreshold` | `cross_seed_fuzzy_size_threshold` |
//! | `seasonFromEpisodes` | `cross_seed_season_from_episodes` (0: off, cross-seed's `null`) |
//! | `includeSingleEpisodes` | `cross_seed_include_single_episodes` |
//! | `includeNonVideos` | `cross_seed_include_non_videos` |
//! | `excludeOlder` | `cross_seed_exclude_older` (empty: off) |
//! | `excludeRecentSearch` | `cross_seed_exclude_recent_search` (empty: off) |
//! | `blockList` | `cross_seed_block_list` (one entry per line) |
//! | `ignoreTitles` | `cross_seed_ignore_titles` |
//! | `delay` | `cross_seed_delay` |
//! | `port` | `cross_seed_port` (0: off) |
//! | `apiKey` | Lantern's `api_key` |
//! | `action` | `cross_seed_action` |
//! | `linkDirs` | `cross_seed_link_dirs` (one folder per line) |
//! | `linkType` | `cross_seed_link_type` |
//! | `flatLinking` | `cross_seed_flat_linking` |
//! | `linkCategory` | `cross_seed_link_category` (empty: none) |
//! | `duplicateCategories` | `cross_seed_duplicate_categories` |
//! | `skipRecheck` | `cross_seed_skip_recheck` |
//! | `autoResumeMaxDownload` | `cross_seed_auto_resume_max_download` |
//! | `ignoreNonRelevantFilesToResume` | `cross_seed_ignore_non_relevant_files_to_resume` |
//! | `torrentClients` | `cross_seed_torrent_clients` (one per line; empty: Lantern's torrent download clients) |
//! | `outputDir` | `cross-seeds` in the data folder |
//! | `useClientTorrents` | `cross_seed_use_client_torrents` |
//! | `torrentDir` | `cross_seed_torrent_dir` (empty: none) |
//! | `dataDirs` | `cross_seed_data_dirs` (one folder per line) |
//! | `maxDataDepth` | `cross_seed_max_data_depth` |
//! | `rssCadence` | `cross_seed_rss_cadence` (empty: off) |
//! | `searchCadence` | `cross_seed_search_cadence` (empty: off) |
//! | `snatchTimeout` | `cross_seed_snatch_timeout` (empty: none) |
//! | `searchTimeout` | `cross_seed_search_timeout` (empty: none) |
//! | `searchLimit` | `cross_seed_search_limit` (0: none) |
//! | `notificationWebhookUrls` | `cross_seed_notification_webhook_urls` (one per line) |
//! | `sonarr` | `cross_seed_sonarr` (one address with its `apikey` per line) |
//! | `radarr` | `cross_seed_radarr` (one address with its `apikey` per line) |
//!
//! `torznab` stays empty: Lantern searches its own indexers. `host`, `verbose` and `injectDir` keep cross-seed's
//! defaults (the CLI sets `injectDir`).

use super::clients::{clients_are_unique, parse_client_entry, CLIENT_TYPES};
use super::constants::{parse_blocklist_entry, Action, BlocklistType, LinkType, MatchMode, INFO_HASH_REGEX};
use super::jsre::test;
use super::utils::format_as_list;

/// One day in milliseconds.
const DAY: f64 = 86_400_000.0;

/// `RuntimeConfig`: every cross-seed setting, in its units (durations in milliseconds).
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeConfig {
    /// `delay`: seconds between two searches.
    pub delay: f64,
    /// `torznab`: the Torznab addresses (Lantern searches its own indexers).
    pub torznab: Vec<String>,
    /// `useClientTorrents`: look at the torrents in the clients.
    pub use_client_torrents: bool,
    /// `dataDirs`: folders of data to search for.
    pub data_dirs: Vec<String>,
    /// `matchMode`.
    pub match_mode: MatchMode,
    /// `skipRecheck`: don't recheck injected torrents that need none.
    pub skip_recheck: bool,
    /// `autoResumeMaxDownload`: bytes still missing below which an injected torrent is resumed.
    pub auto_resume_max_download: i64,
    /// `ignoreNonRelevantFilesToResume`.
    pub ignore_non_relevant_files_to_resume: bool,
    /// `linkDirs`: where matched files are linked.
    pub link_dirs: Vec<String>,
    /// `linkType`.
    pub link_type: LinkType,
    /// `flatLinking`.
    pub flat_linking: bool,
    /// `maxDataDepth`.
    pub max_data_depth: i64,
    /// `linkCategory`.
    pub link_category: Option<String>,
    /// `torrentDir`.
    pub torrent_dir: Option<String>,
    /// `outputDir` (`None`: `cross-seeds` in the data folder).
    pub output_dir: Option<String>,
    /// `injectDir`.
    pub inject_dir: Option<String>,
    /// `ignoreTitles`: the inject job doesn't compare titles.
    pub ignore_titles: Option<bool>,
    /// `includeSingleEpisodes`.
    pub include_single_episodes: bool,
    /// `verbose`.
    pub verbose: bool,
    /// `includeNonVideos`.
    pub include_non_videos: bool,
    /// `seasonFromEpisodes` (`None`: off).
    pub season_from_episodes: Option<f64>,
    /// `fuzzySizeThreshold`.
    pub fuzzy_size_threshold: f64,
    /// `excludeOlder` in milliseconds (`None`: off).
    pub exclude_older: Option<f64>,
    /// `excludeRecentSearch` in milliseconds (`None`: off).
    pub exclude_recent_search: Option<f64>,
    /// `action`.
    pub action: Action,
    /// `torrentClients`.
    pub torrent_clients: Vec<String>,
    /// `duplicateCategories`.
    pub duplicate_categories: bool,
    /// `notificationWebhookUrls`.
    pub notification_webhook_urls: Vec<String>,
    /// `port` (`None`: no daemon API).
    pub port: Option<u16>,
    /// `host`.
    pub host: Option<String>,
    /// `searchCadence` in milliseconds.
    pub search_cadence: Option<f64>,
    /// `rssCadence` in milliseconds.
    pub rss_cadence: Option<f64>,
    /// `snatchTimeout` in milliseconds.
    pub snatch_timeout: Option<f64>,
    /// `searchTimeout` in milliseconds.
    pub search_timeout: Option<f64>,
    /// `searchLimit`.
    pub search_limit: Option<i64>,
    /// `blockList`.
    pub block_list: Vec<String>,
    /// `apiKey`.
    pub api_key: Option<String>,
    /// `sonarr`.
    pub sonarr: Vec<String>,
    /// `radarr`.
    pub radarr: Vec<String>,
    /// `torrents`: `.torrent` files to search for instead of the clients' torrents and `torrentDir`
    /// (`cross-seed search --torrents`; `None`: not given).
    pub torrents: Option<Vec<String>>,
}

impl Default for RuntimeConfig {
    /// cross-seed's config template (`cross-seed gen-config`).
    fn default() -> Self {
        RuntimeConfig {
            delay: 30.0,
            torznab: Vec::new(),
            use_client_torrents: true,
            data_dirs: Vec::new(),
            match_mode: MatchMode::Flexible,
            skip_recheck: true,
            auto_resume_max_download: 52_428_800,
            ignore_non_relevant_files_to_resume: false,
            link_dirs: Vec::new(),
            link_type: LinkType::Hardlink,
            flat_linking: false,
            max_data_depth: 2,
            link_category: Some("cross-seed-link".into()),
            torrent_dir: None,
            output_dir: None,
            inject_dir: None,
            ignore_titles: None,
            include_single_episodes: false,
            verbose: false,
            include_non_videos: false,
            season_from_episodes: Some(1.0),
            fuzzy_size_threshold: 0.02,
            exclude_older: Some(14.0 * DAY),
            exclude_recent_search: Some(3.0 * DAY),
            action: Action::Inject,
            torrent_clients: Vec::new(),
            duplicate_categories: false,
            notification_webhook_urls: Vec::new(),
            port: Some(2468),
            host: Some("0.0.0.0".into()),
            search_cadence: Some(DAY),
            rss_cadence: Some(30.0 * 60_000.0),
            snatch_timeout: Some(30_000.0),
            search_timeout: Some(120_000.0),
            search_limit: Some(400),
            block_list: Vec::new(),
            api_key: None,
            sonarr: Vec::new(),
            radarr: Vec::new(),
            torrents: None,
        }
    }
}

/// `Partial<RuntimeConfig>`: settings changed for one search or job (`getRuntimeConfig(configOverride)`); `None`
/// keeps the setting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Override {
    /// `includeSingleEpisodes`.
    pub include_single_episodes: Option<bool>,
    /// `includeNonVideos`.
    pub include_non_videos: Option<bool>,
    /// `excludeRecentSearch` in milliseconds.
    pub exclude_recent_search: Option<f64>,
    /// `excludeOlder` in milliseconds.
    pub exclude_older: Option<f64>,
    /// `blockList`.
    pub block_list: Option<Vec<String>>,
    /// `matchMode`.
    pub match_mode: Option<MatchMode>,
    /// `seasonFromEpisodes` (`Some(None)`: off).
    pub season_from_episodes: Option<Option<f64>>,
    /// `fuzzySizeThreshold`.
    pub fuzzy_size_threshold: Option<f64>,
}

impl RuntimeConfig {
    /// `getRuntimeConfig(configOverride)`: these settings with `o`'s changes.
    pub fn with(&self, o: &Override) -> RuntimeConfig {
        let mut c = self.clone();
        if let Some(v) = o.include_single_episodes {
            c.include_single_episodes = v;
        }
        if let Some(v) = o.include_non_videos {
            c.include_non_videos = v;
        }
        if let Some(v) = o.exclude_recent_search {
            c.exclude_recent_search = Some(v);
        }
        if let Some(v) = o.exclude_older {
            c.exclude_older = Some(v);
        }
        if let Some(v) = &o.block_list {
            c.block_list = v.clone();
        }
        if let Some(v) = o.match_mode {
            c.match_mode = v;
        }
        if let Some(v) = o.season_from_episodes {
            c.season_from_episodes = v;
        }
        if let Some(v) = o.fuzzy_size_threshold {
            c.fuzzy_size_threshold = v;
        }
        c
    }

    /// The settings from Lantern's settings (see the module's table), and the problems [`validate`] and the
    /// reading found, in cross-seed's words.
    pub fn from_settings(s: &kit_server_settings::Store) -> (RuntimeConfig, Vec<String>) {
        RuntimeConfig::from_settings_with(s, &[])
    }

    /// [`from_settings`](Self::from_settings), with `lantern_clients` (Lantern's torrent download clients as
    /// cross-seed's `torrentClients` entries) used when `cross_seed_torrent_clients` is empty.
    pub fn from_settings_with(s: &kit_server_settings::Store, lantern_clients: &[String]) -> (RuntimeConfig, Vec<String>) {
        let (mut c, mut problems) = RuntimeConfig::read_settings_with(s, lantern_clients);
        problems.extend(validate(&mut c));
        (c, problems)
    }

    /// The settings from Lantern's settings without [`validate`]'s checks, and the problems found reading them
    /// (a command line changes them before they are checked).
    pub fn read_settings_with(s: &kit_server_settings::Store, lantern_clients: &[String]) -> (RuntimeConfig, Vec<String>) {
        let mut problems = Vec::new();
        let mut c = RuntimeConfig::default();
        let mode = s.str("cross_seed_match_mode");
        match MatchMode::parse(&mode) {
            Some(m) => c.match_mode = m,
            None => problems.push(issue(&mode, &match_mode_invalid())),
        }
        c.fuzzy_size_threshold = s.float("cross_seed_fuzzy_size_threshold");
        let sfe = s.float("cross_seed_season_from_episodes");
        c.season_from_episodes = if sfe == 0.0 { None } else { Some(sfe) };
        c.include_single_episodes = s.bool("cross_seed_include_single_episodes");
        c.include_non_videos = s.bool("cross_seed_include_non_videos");
        c.ignore_titles = Some(s.bool("cross_seed_ignore_titles"));
        c.delay = s.float("cross_seed_delay");
        let port = s.int("cross_seed_port");
        c.port = u16::try_from(port).ok().filter(|p| *p != 0);
        for (key, slot) in
            [("cross_seed_exclude_older", &mut c.exclude_older), ("cross_seed_exclude_recent_search", &mut c.exclude_recent_search)]
        {
            let text = s.str(key);
            *slot = if text.trim().is_empty() {
                None
            } else {
                match ms(&text) {
                    Some(v) => Some(v),
                    None => {
                        problems.push(issue(&text, VERCEL));
                        None
                    }
                }
            };
        }
        c.block_list = block_list_lines(&s.str("cross_seed_block_list"));
        let action = s.str("cross_seed_action");
        match Action::parse(&action) {
            Some(a) => c.action = a,
            None => problems.push(issue(&action, "Invalid enum value. Expected 'save' | 'inject'")),
        }
        c.link_dirs = block_list_lines(&s.str("cross_seed_link_dirs")).into_iter().map(|d| d.trim().to_string()).collect();
        let link_type = s.str("cross_seed_link_type");
        match LinkType::parse(&link_type) {
            Some(l) => c.link_type = l,
            None => problems.push(issue(&link_type, "Invalid enum value. Expected 'symlink' | 'hardlink' | 'reflink' | 'reflinkOrCopy'")),
        }
        c.flat_linking = s.bool("cross_seed_flat_linking");
        let link_category = s.str("cross_seed_link_category");
        c.link_category = if link_category.is_empty() { None } else { Some(link_category) };
        c.duplicate_categories = s.bool("cross_seed_duplicate_categories");
        c.skip_recheck = s.bool("cross_seed_skip_recheck");
        c.auto_resume_max_download = s.int("cross_seed_auto_resume_max_download");
        c.ignore_non_relevant_files_to_resume = s.bool("cross_seed_ignore_non_relevant_files_to_resume");
        c.torrent_clients = block_list_lines(&s.str("cross_seed_torrent_clients")).into_iter().map(|d| d.trim().to_string()).collect();
        if c.torrent_clients.is_empty() {
            c.torrent_clients = lantern_clients.to_vec();
        }
        let api_key = s.str("api_key");
        c.api_key = if api_key.is_empty() { None } else { Some(api_key) };
        c.use_client_torrents = s.bool("cross_seed_use_client_torrents");
        let torrent_dir = s.str("cross_seed_torrent_dir");
        c.torrent_dir = if torrent_dir.trim().is_empty() { None } else { Some(torrent_dir.trim().to_string()) };
        c.data_dirs = block_list_lines(&s.str("cross_seed_data_dirs")).into_iter().map(|d| d.trim().to_string()).collect();
        c.max_data_depth = s.int("cross_seed_max_data_depth");
        for (key, slot) in [
            ("cross_seed_rss_cadence", &mut c.rss_cadence),
            ("cross_seed_search_cadence", &mut c.search_cadence),
            ("cross_seed_snatch_timeout", &mut c.snatch_timeout),
            ("cross_seed_search_timeout", &mut c.search_timeout),
        ] {
            let text = s.str(key);
            *slot = if text.trim().is_empty() {
                None
            } else {
                match ms(&text) {
                    Some(v) => Some(v),
                    None => {
                        problems.push(issue(&text, VERCEL));
                        None
                    }
                }
            };
        }
        let limit = s.int("cross_seed_search_limit");
        c.search_limit = if limit == 0 { None } else { Some(limit) };
        let lines = |k: &str| block_list_lines(&s.str(k)).into_iter().map(|d| d.trim().to_string()).collect::<Vec<_>>();
        c.notification_webhook_urls = lines("cross_seed_notification_webhook_urls");
        c.sonarr = lines("cross_seed_sonarr");
        c.radarr = lines("cross_seed_radarr");
        (c, problems)
    }
}

/// The block list as written in its text box: one entry per line, blank lines left out.
pub fn block_list_lines(text: &str) -> Vec<String> {
    text.lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.trim().is_empty()).collect()
}

/// `ms(str)` (vercel/ms 2): `"2 weeks"`, `"3d"`, `"1.5 hours"`, `"500"` in milliseconds; `None` when it isn't
/// one (cross-seed's `NaN`).
pub fn ms(s: &str) -> Option<f64> {
    if s.is_empty() || s.len() > 100 {
        return None;
    }
    let lower = s.to_lowercase();
    // ^(-?(?:\d+)?\.?\d+) *(unit)?$
    let num_end = lower.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(lower.len());
    let (num, rest) = lower.split_at(num_end);
    let body = num.strip_prefix('-').unwrap_or(num);
    let ok_num = !body.is_empty()
        && !body.contains('-')
        && body.matches('.').count() <= 1
        && !body.ends_with('.')
        && body.chars().last().is_some_and(|c| c.is_ascii_digit());
    if !ok_num {
        return None;
    }
    let n: f64 = num.parse().ok()?;
    let unit = rest.trim_start_matches(' ');
    let factor = match unit {
        "years" | "year" | "yrs" | "yr" | "y" => DAY * 365.25,
        "weeks" | "week" | "w" => DAY * 7.0,
        "days" | "day" | "d" => DAY,
        "hours" | "hour" | "hrs" | "hr" | "h" => 3_600_000.0,
        "minutes" | "minute" | "mins" | "min" | "m" => 60_000.0,
        "seconds" | "second" | "secs" | "sec" | "s" => 1000.0,
        "milliseconds" | "millisecond" | "msecs" | "msec" | "ms" | "" => 1.0,
        _ => return None,
    };
    Some(n * factor)
}

/// `ms(n, { long: true })`: `14 days`, `1 hour`, `30 minutes` (vercel/ms 2).
pub fn ms_long(n: f64) -> String {
    let abs = n.abs();
    let plural = |unit: f64, name: &str| {
        let count = js_round(n / unit);
        format!("{count} {name}{}", if abs >= unit * 1.5 { "s" } else { "" })
    };
    if abs >= DAY {
        plural(DAY, "day")
    } else if abs >= 3_600_000.0 {
        plural(3_600_000.0, "hour")
    } else if abs >= 60_000.0 {
        plural(60_000.0, "minute")
    } else if abs >= 1000.0 {
        plural(1000.0, "second")
    } else {
        format!("{n} ms")
    }
}

/// `ms(n)`: `14d`, `1h`, `30m` (vercel/ms 2).
pub fn ms_short(n: f64) -> String {
    let abs = n.abs();
    for (unit, name) in [(DAY, "d"), (3_600_000.0, "h"), (60_000.0, "m"), (1000.0, "s")] {
        if abs >= unit {
            return format!("{}{name}", js_round(n / unit));
        }
    }
    format!("{n}ms")
}

/// JavaScript's `Math.round` (halves up).
fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

// cross-seed's validation messages (configSchema.ts `ZodErrorMessages`)
/// `vercel`: a duration that isn't one.
pub const VERCEL: &str = "format does not follow vercel's `ms` style ( https://github.com/vercel/ms#examples )";
/// `delayNegative`.
pub const DELAY_NEGATIVE: &str = "delay is in seconds, you can't travel back in time.";
/// `delayUnsupported`.
pub const DELAY_UNSUPPORTED: &str = "delay must be 30 seconds to 1 hour.\n\t\t\t\tTo even out search loads please see the following documentation:\n\t\t\t\t(https://www.cross-seed.org/docs/basics/options#delay)";
/// `searchCadenceExcludeRecent`.
pub const SEARCH_CADENCE_EXCLUDE_RECENT: &str = "excludeRecentSearch must be at least 3x searchCadence.";
/// `excludeRecentOlder`.
pub const EXCLUDE_RECENT_OLDER: &str =
    "excludeOlder and excludeRecentSearch must be defined for searching. excludeOlder must be 2-5x excludeRecentSearch.";
/// `numberMustBeRatio`.
pub const NUMBER_MUST_BE_RATIO: &str = "fuzzySizeThreshold and seasonFromEpisodes must be between 0 and 1.";
/// `fuzzySizeThresholdMax`.
pub const FUZZY_SIZE_THRESHOLD_MAX: &str = "fuzzySizeThreshold cannot be greater than 0.1 when using searchCadence or rssCadence.";
/// `seasonFromEpisodesMin`.
pub const SEASON_FROM_EPISODES_MIN: &str = "seasonFromEpisodes cannot be less than 0.5 when using searchCadence or rssCadence";
/// `matchModeNeedsLinkDirs`.
pub const MATCH_MODE_NEEDS_LINK_DIRS: &str = "When using action: \"inject\", you need to set linkDirs for flexible and partial matchMode (https://www.cross-seed.org/docs/tutorials/linking). If you cannot use linking, use matchMode: \"strict\"";
/// `ensembleNeedsLinkDirs`.
pub const ENSEMBLE_NEEDS_LINK_DIRS: &str = "When using action 'inject', you need to set linkDirs for seasonFromEpisodes (https://www.cross-seed.org/docs/tutorials/linking). If you cannot use linking, disable this option by using seasonFromEpisodes: null";
/// `ensembleNeedsPartial`.
pub const ENSEMBLE_NEEDS_PARTIAL: &str = "seasonFromEpisodes requires matchMode partial if enabled and value is below 1.";
/// `blocklistEmptyValue`.
pub const BLOCKLIST_EMPTY_VALUE: &str = "Blocklist item must have a value after the colon.";
/// `blocklistRegex`.
pub const BLOCKLIST_REGEX: &str = "Blocklist regex is not a valid regex.";
/// `blocklistFolder`.
pub const BLOCKLIST_FOLDER: &str = "Blocklist folder must not contain path separators";
/// `blocklistTracker`.
pub const BLOCKLIST_TRACKER: &str = "Blocklist tracker is not a valid URL host. If URL is https://user:pass@tracker.example.com:8080/announce/key, you must use \"tracker:tracker.example.com:8080\"";
/// `blocklistHash`.
pub const BLOCKLIST_HASH: &str = "Blocklist hash must be 40 characters and alphanumeric";
/// `blocklistSize`.
pub const BLOCKLIST_SIZE: &str = "Blocklist size must be an integer for the number of bytes. You can only have one sizeBelow, one sizeAbove, and sizeBelow <= sizeAbove.";

/// `autoResumeMaxDownloadUnsupported`.
pub const AUTO_RESUME_MAX_DOWNLOAD_UNSUPPORTED: &str =
    "autoResumeMaxDownload must be an integer of bytes between between 0 and 52428800 (50 MiB).";
/// `clientTypeReadOnly`.
pub const CLIENT_TYPE_READ_ONLY: &str = "You must have at least one non-readonly torrent client when using action: \"inject\"";
/// `duplicateClients`.
pub const DUPLICATE_CLIENTS: &str = "Duplicate torrent client URLs are not allowed.";
/// `injectNeedsClients`.
pub const INJECT_NEEDS_CLIENTS: &str = "You need to specify torrentClients when using 'inject'";
/// `needsClient`.
pub const NEEDS_CLIENT: &str = "You need to have a client configured for useClientTorrents.";
/// `torrentDirAndUseClientTorrents`.
pub const TORRENT_DIR_AND_USE_CLIENT_TORRENTS: &str = "You cannot have both torrentDir and useClientTorrents.";
/// `multipleClientsTorrentFile`.
pub const MULTIPLE_CLIENTS_TORRENT_FILE: &str =
    "torrentDir and --torrents arg does not support multiple clients, use useClientTorrents instead.";
/// `multipleClientsNoLinking`.
pub const MULTIPLE_CLIENTS_NO_LINKING: &str = "dataDirs is not supported with multiple clients if not linking. To configure linking, please read: https://www.cross-seed.org/docs/tutorials/linking";
/// `needSearchees`.
pub const NEED_SEARCHEES: &str = "You need to have torrentDir, useClientTorrents or dataDirs for search/rss/announce matching to work.";
/// `ensembleNeedsClient`.
pub const ENSEMBLE_NEEDS_CLIENT: &str =
    "seasonFromEpisodes requires a torrent client to connect to when using torrentDir or useClientTorrents.";
/// `linkDirsInOtherDirs`.
pub const LINK_DIRS_IN_OTHER_DIRS: &str =
    "You cannot have your linkDirs inside of your torrentDir/dataDirs/outputDir. Please adjust your paths to correct this.";
/// `dataDirsInOtherDirs`.
pub const DATA_DIRS_IN_OTHER_DIRS: &str =
    "You cannot have your dataDirs inside of your torrentDir/linkDirs/outputDir. Please adjust your paths to correct this.";
/// `torrentDirInOtherDirs`.
pub const TORRENT_DIR_IN_OTHER_DIRS: &str =
    "You cannot have your torrentDir inside of your dataDirs/linkDirs/outputDir. Please adjust your paths to correct this.";
/// `outputDirInOtherDirs`.
pub const OUTPUT_DIR_IN_OTHER_DIRS: &str =
    "You cannot have your outputDir inside of your torrentDir/dataDirs/linkDirs. Please adjust your paths to correct this.";
/// zod's message for an address that isn't one (`z.string().url()`).
pub const INVALID_URL: &str = "Invalid url";

/// `isChildPath`: whether `child` is `parent` or inside one of `parents`, as paths are resolved (not on disk).
pub fn is_child_path(child: &str, parents: &[String]) -> bool {
    let resolve = |p: &str| -> Vec<String> {
        let start = if p.starts_with('/') {
            std::path::PathBuf::from("/")
        } else {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"))
        };
        let mut parts: Vec<String> = start.to_string_lossy().split('/').filter(|x| !x.is_empty()).map(str::to_string).collect();
        for seg in p.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s.to_string()),
            }
        }
        parts
    };
    let c = resolve(child);
    parents.iter().any(|parent| {
        let p = resolve(parent);
        c.len() >= p.len() && c[..p.len()] == p[..]
    })
}

/// `validateUArrLs`'s first check: every Sonarr and Radarr address carries its `apikey`.
pub fn arr_url_problems(c: &RuntimeConfig) -> Vec<String> {
    let mut p = Vec::new();
    for (name, list) in [("Sonarr", &c.sonarr), ("Radarr", &c.radarr)] {
        for u in list {
            if let Ok(url) = url::Url::parse(u) {
                if !url.query_pairs().any(|(k, _)| k == "apikey") {
                    p.push(format!("{name} url {url} does not specify an apikey"));
                }
            }
        }
    }
    p
}

/// `clientType`: a torrent client entry with a type cross-seed doesn't know.
pub fn client_type_message() -> String {
    let names: Vec<String> = CLIENT_TYPES.iter().map(|t| format!("{t}:")).collect();
    format!(
        "torrentClients type prefix must be one of {} and optionally followed by readonly: (e.g torrentClients: [\"qbittorrent:http://username:password@localhost:8080\", \"rtorrent:readonly:http://username:password@localhost:1234/RPC2\"])",
        format_as_list(&names, false, "narrow", "unit")
    )
}

/// `blocklistType`: a block list entry with a prefix cross-seed doesn't know.
pub fn blocklist_type_message() -> String {
    let names: Vec<String> = BlocklistType::NAMES.iter().map(|s| s.to_string()).collect();
    format!("Blocklist item does not start with a valid prefix. Must be of {}", format_as_list(&names, false, "narrow", "unit"))
}

/// `matchModeInvalid`.
pub fn match_mode_invalid() -> String {
    let names: Vec<String> = ["strict", "flexible", "partial"].iter().map(|m| format!("\"{m}\"")).collect();
    format!("matchMode must be one of: {}", format_as_list(&names, false, "narrow", "unit"))
}

/// `addZodIssue`: `Setting: "<value>"` and the message on the next line.
pub fn issue(setting: &str, message: &str) -> String {
    format!("Setting: \"{setting}\"\n\t{message}")
}

/// `transformBlocklist`: the problems in a block list (infoHash entries are put in small letters).
pub fn check_block_list(block_list: &mut [String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut size_below: i64 = 0;
    let mut size_above: i64 = 0;
    let size_test = |value: &str, existing: i64, problems: &mut Vec<String>| -> i64 {
        if existing == 0 && !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(n) = value.parse() {
                return n;
            }
        }
        problems.push(issue(value, BLOCKLIST_SIZE));
        existing
    };
    for entry in block_list.iter_mut() {
        if entry.trim().is_empty() {
            problems.push(issue(entry, &blocklist_type_message()));
            continue;
        }
        let (kind, value) = parse_blocklist_entry(entry);
        match kind {
            BlocklistType::Name => {
                if value.is_empty() {
                    problems.push(issue(entry, BLOCKLIST_EMPTY_VALUE));
                }
            }
            BlocklistType::Folder => {
                if value.is_empty() {
                    problems.push(issue(entry, BLOCKLIST_EMPTY_VALUE));
                }
                if value.contains(['/', '\\']) {
                    problems.push(issue(entry, BLOCKLIST_FOLDER));
                }
            }
            BlocklistType::NameRegex | BlocklistType::FolderRegex => {
                if value.is_empty() {
                    problems.push(issue(entry, BLOCKLIST_EMPTY_VALUE));
                }
                if fancy_regex::Regex::new(&super::jsre::translate(&value)).is_err() {
                    problems.push(issue(entry, BLOCKLIST_REGEX));
                }
            }
            // an empty category or tag blocks the torrents without one (cross-seed logs it)
            BlocklistType::Category | BlocklistType::Tag => {}
            BlocklistType::Tracker => {
                if value.is_empty() {
                    problems.push(issue(entry, BLOCKLIST_EMPTY_VALUE));
                }
                if value.contains(['/', '@']) || !value.contains('.') {
                    problems.push(issue(entry, BLOCKLIST_TRACKER));
                }
            }
            BlocklistType::InfoHash => {
                if !test(&INFO_HASH_REGEX, &value) {
                    problems.push(issue(entry, BLOCKLIST_HASH));
                }
                *entry = format!("infoHash:{}", value.to_lowercase());
            }
            BlocklistType::SizeBelow => size_below = size_test(&value, size_below, &mut problems),
            BlocklistType::SizeAbove => size_above = size_test(&value, size_above, &mut problems),
            BlocklistType::Legacy => {
                if value.is_empty() {
                    problems.push(issue(entry, BLOCKLIST_EMPTY_VALUE));
                }
                tracing::warn!(
                    "Legacy style blocklist is deprecated, specify a specific type followed by a colon (e.g infoHash:) for {entry}"
                );
            }
            BlocklistType::Unknown => problems.push(issue(entry, &blocklist_type_message())),
        }
    }
    if size_below != 0 && size_above != 0 && size_below > size_above {
        problems.push(issue("sizeBelow > sizeAbove", BLOCKLIST_SIZE));
    }
    problems
}

/// `VALIDATION_SCHEMA`'s checks of the settings Lantern has, in cross-seed's order and words (an empty list: all
/// good). Like cross-seed's schema it also tidies the block list.
pub fn validate(c: &mut RuntimeConfig) -> Vec<String> {
    let mut p = Vec::new();
    if c.delay < 0.0 {
        p.push(DELAY_NEGATIVE.to_string());
    }
    if !(30.0..=3600.0).contains(&c.delay) {
        p.push(DELAY_UNSUPPORTED.to_string());
    }
    if c.fuzzy_size_threshold <= 0.0 {
        p.push("Number must be greater than 0".to_string());
    }
    if c.fuzzy_size_threshold > 1.0 {
        p.push(NUMBER_MUST_BE_RATIO.to_string());
    }
    if let Some(sfe) = c.season_from_episodes {
        if sfe <= 0.0 {
            p.push("Number must be greater than 0".to_string());
        }
        if sfe > 1.0 {
            p.push(NUMBER_MUST_BE_RATIO.to_string());
        }
    }
    if let Some(r) = c.rss_cadence {
        if !(600_000.0..=7_200_000.0).contains(&r) {
            p.push("rssCadence must be 10-120 minutes".to_string());
        }
    }
    if let Some(s) = c.search_cadence {
        if s < DAY {
            p.push("searchCadence must be at least 1 day.".to_string());
        }
    }
    if !(0..=52_428_800).contains(&c.auto_resume_max_download) {
        p.push(AUTO_RESUME_MAX_DOWNLOAD_UNSUPPORTED.to_string());
    }
    if c.max_data_depth < 1 {
        p.push("Number must be greater than or equal to 1".to_string());
    } else if c.max_data_depth > 3 {
        tracing::warn!("Your maxDataDepth is most likely incorrect, please read: https://www.cross-seed.org/docs/tutorials/data-based-matching#setting-up-data-based-matching");
    }
    for u in &c.notification_webhook_urls {
        if url::Url::parse(u).is_err() {
            p.push(issue(u, INVALID_URL));
        }
    }
    for entry in &c.torrent_clients {
        if !parse_client_entry(entry).is_some_and(|e| CLIENT_TYPES.contains(&e.client_type.as_str())) {
            p.push(issue(entry, &client_type_message()));
        }
    }
    if c.search_limit.is_some_and(|l| l < 0) {
        p.push("Number must be greater than or equal to 0".to_string());
    }
    p.extend(check_block_list(&mut c.block_list));
    for u in c.radarr.iter().chain(c.sonarr.iter()) {
        if url::Url::parse(u).is_err() {
            p.push(issue(u, INVALID_URL));
        }
    }
    if c.torrent_dir.is_some() && c.use_client_torrents {
        p.push(TORRENT_DIR_AND_USE_CLIENT_TORRENTS.to_string());
    }
    if c.use_client_torrents && c.torrent_clients.is_empty() {
        p.push(NEEDS_CLIENT.to_string());
    }
    if c.action == Action::Inject && !c.torrent_clients.iter().any(|t| !t.contains(":readonly:")) {
        p.push(CLIENT_TYPE_READ_ONLY.to_string());
    }
    if c.torrent_clients.len() > 1 && (c.torrent_dir.is_some() || c.torrents.as_ref().is_some_and(|t| !t.is_empty())) {
        p.push(MULTIPLE_CLIENTS_TORRENT_FILE.to_string());
    }
    if c.torrent_clients.len() > 1 && c.link_dirs.is_empty() && !c.data_dirs.is_empty() {
        p.push(MULTIPLE_CLIENTS_NO_LINKING.to_string());
    }
    let entries_ok = c.torrent_clients.iter().all(|t| parse_client_entry(t).is_some_and(|e| url::Url::parse(&e.url).is_ok()));
    if entries_ok {
        let (hosts, with_path) = clients_are_unique(&c.torrent_clients);
        if !hosts && !with_path {
            p.push(DUPLICATE_CLIENTS.to_string());
        }
    }
    if let (Some(s), Some(r)) = (c.search_cadence, c.exclude_recent_search) {
        if 3.0 * s > r {
            p.push(SEARCH_CADENCE_EXCLUDE_RECENT.to_string());
        }
    }
    if c.search_cadence.is_some() {
        let ok = matches!((c.exclude_older, c.exclude_recent_search), (Some(o), Some(r)) if o >= 2.0 * r && o <= 5.0 * r);
        if !ok {
            p.push(EXCLUDE_RECENT_OLDER.to_string());
        }
    }
    let cadence = c.search_cadence.is_some() || c.rss_cadence.is_some();
    if c.fuzzy_size_threshold > 0.1 && cadence {
        p.push(FUZZY_SIZE_THRESHOLD_MAX.to_string());
    }
    if cadence && c.season_from_episodes.is_some_and(|s| s < 0.5) {
        p.push(SEASON_FROM_EPISODES_MIN.to_string());
    }
    if c.action == Action::Save && !c.link_dirs.is_empty() {
        tracing::error!("You cannot use action 'save' with linkDirs, use 'inject' for cross-seed to perform linking.");
    }
    if c.torrent_clients.is_empty() && c.action == Action::Inject {
        p.push(INJECT_NEEDS_CLIENTS.to_string());
    }
    if !c.use_client_torrents
        && c.torrent_dir.is_none()
        && c.data_dirs.is_empty()
        && (c.rss_cadence.is_some() || c.search_cadence.is_some())
    {
        p.push(NEED_SEARCHEES.to_string());
    }
    if c.link_dirs.is_empty() && c.match_mode != MatchMode::Strict && c.action != Action::Save {
        p.push(MATCH_MODE_NEEDS_LINK_DIRS.to_string());
    }
    if c.season_from_episodes.is_some() && c.torrent_dir.is_some() && c.torrent_clients.is_empty() {
        p.push(ENSEMBLE_NEEDS_CLIENT.to_string());
    }
    if c.season_from_episodes.is_some() && c.action != Action::Save && c.link_dirs.is_empty() {
        p.push(ENSEMBLE_NEEDS_LINK_DIRS.to_string());
    }
    if c.season_from_episodes.is_some_and(|s| s < 1.0) && c.match_mode != MatchMode::Partial {
        p.push(ENSEMBLE_NEEDS_PARTIAL.to_string());
    }
    p.extend(dir_problems(c));
    p
}

/// The checks that no folder is inside another kind (`linkDirsInOtherDirs` and the three after it); `outputDir`
/// counts only when it is set.
pub fn dir_problems(c: &RuntimeConfig) -> Vec<String> {
    let mut p = Vec::new();
    let output: Vec<String> = c.output_dir.iter().cloned().collect();
    let torrent: Vec<String> = c.torrent_dir.iter().cloned().collect();
    if c.link_dirs.iter().any(|l| is_child_path(l, &output) || is_child_path(l, &c.data_dirs) || is_child_path(l, &torrent)) {
        p.push(LINK_DIRS_IN_OTHER_DIRS.to_string());
    }
    if c.data_dirs.iter().any(|d| is_child_path(d, &output) || is_child_path(d, &torrent) || is_child_path(d, &c.link_dirs)) {
        p.push(DATA_DIRS_IN_OTHER_DIRS.to_string());
    }
    if let Some(t) = &c.torrent_dir {
        if is_child_path(t, &output) || is_child_path(t, &c.data_dirs) || is_child_path(t, &c.link_dirs) {
            p.push(TORRENT_DIR_IN_OTHER_DIRS.to_string());
        }
    }
    if let Some(o) = &c.output_dir {
        if is_child_path(o, &torrent) || is_child_path(o, &c.data_dirs) || is_child_path(o, &c.link_dirs) {
            p.push(OUTPUT_DIR_IN_OTHER_DIRS.to_string());
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_cross_seeds_template() {
        let c = RuntimeConfig::default();
        assert_eq!(c.match_mode, MatchMode::Flexible);
        assert_eq!(c.fuzzy_size_threshold, 0.02);
        assert_eq!(c.season_from_episodes, Some(1.0));
        assert_eq!(c.exclude_older, Some(14.0 * DAY));
        assert_eq!(c.exclude_recent_search, Some(3.0 * DAY));
        assert_eq!((c.delay, c.port, c.search_limit), (30.0, Some(2468), Some(400)));
        assert!(!c.include_single_episodes && !c.include_non_videos);
    }

    #[test]
    fn durations_like_vercel_ms() {
        assert_eq!(ms("2 weeks"), Some(14.0 * DAY));
        assert_eq!(ms("3 days"), Some(3.0 * DAY));
        assert_eq!(ms("1.5h"), Some(5_400_000.0));
        assert_eq!(ms("30 minutes"), Some(1_800_000.0));
        assert_eq!(ms("100"), Some(100.0));
        assert_eq!(ms("1 Year"), Some(DAY * 365.25));
        assert_eq!(ms("two weeks"), None);
        assert_eq!(ms("1 fortnight"), None);
        assert_eq!(ms(""), None);
        assert_eq!(ms_long(14.0 * DAY), "14 days");
        assert_eq!(ms_long(3_600_000.0), "1 hour");
        assert_eq!(ms_long(5_400_000.0), "2 hours");
        assert_eq!(ms_short(DAY), "1d");
    }

    #[test]
    fn overrides_change_only_what_they_name() {
        let c = RuntimeConfig::default();
        let o = Override { exclude_recent_search: Some(1.0), block_list: Some(vec![]), ..Override::default() };
        let d = c.with(&o);
        assert_eq!(d.exclude_recent_search, Some(1.0));
        assert_eq!(d.exclude_older, c.exclude_older);
        assert_eq!(c.with(&Override::default()), c);
    }

    #[test]
    fn the_default_needs_link_dirs_as_in_cross_seed() {
        let mut c = RuntimeConfig::default();
        assert_eq!(
            validate(&mut c),
            vec![
                NEEDS_CLIENT.to_string(),
                CLIENT_TYPE_READ_ONLY.to_string(),
                INJECT_NEEDS_CLIENTS.to_string(),
                MATCH_MODE_NEEDS_LINK_DIRS.to_string(),
                ENSEMBLE_NEEDS_LINK_DIRS.to_string()
            ]
        );
        let mut c = RuntimeConfig { torrent_clients: vec!["qbittorrent:http://q:8080".into()], ..RuntimeConfig::default() };
        assert_eq!(validate(&mut c), vec![MATCH_MODE_NEEDS_LINK_DIRS.to_string(), ENSEMBLE_NEEDS_LINK_DIRS.to_string()]);
        c.link_dirs = vec!["/links".into()];
        assert!(validate(&mut c).is_empty());
        c.match_mode = MatchMode::Strict;
        c.link_dirs.clear();
        c.season_from_episodes = None;
        assert!(validate(&mut c).is_empty());
    }

    #[test]
    fn torrent_client_checks() {
        let base = RuntimeConfig { match_mode: MatchMode::Strict, season_from_episodes: None, ..RuntimeConfig::default() };
        let mut c = RuntimeConfig { torrent_clients: vec!["rtorrent:readonly:http://r/RPC2".into()], ..base.clone() };
        assert_eq!(validate(&mut c), vec![CLIENT_TYPE_READ_ONLY.to_string()]);
        c.action = Action::Save;
        assert!(validate(&mut c).is_empty(), "saving needs no client to add to");
        let mut c = RuntimeConfig { torrent_clients: vec!["utorrent:http://u".into()], ..base.clone() };
        assert_eq!(validate(&mut c), vec![issue("utorrent:http://u", &client_type_message())]);
        assert_eq!(
            client_type_message(),
            "torrentClients type prefix must be one of qbittorrent: rtorrent: transmission: deluge: and optionally followed by readonly: (e.g torrentClients: [\"qbittorrent:http://username:password@localhost:8080\", \"rtorrent:readonly:http://username:password@localhost:1234/RPC2\"])"
        );
        let mut c = RuntimeConfig {
            torrent_clients: vec!["qbittorrent:http://q:8080".into(), "qbittorrent:http://q:8080/".into()],
            ..base.clone()
        };
        assert_eq!(validate(&mut c), vec![DUPLICATE_CLIENTS.to_string()]);
        let mut c =
            RuntimeConfig { torrent_clients: vec!["deluge:http://:p@d:8112/json".into()], auto_resume_max_download: 52_428_801, ..base };
        assert_eq!(validate(&mut c), vec![AUTO_RESUME_MAX_DOWNLOAD_UNSUPPORTED.to_string()]);
    }

    #[test]
    fn settings_ranges_in_cross_seeds_words() {
        let mut c = RuntimeConfig {
            link_dirs: vec!["/l".into()],
            torrent_clients: vec!["qbittorrent:http://q:8080".into()],
            ..RuntimeConfig::default()
        };
        c.delay = 10.0;
        c.fuzzy_size_threshold = 0.2;
        c.season_from_episodes = Some(0.4);
        let p = validate(&mut c);
        assert!(p.contains(&DELAY_UNSUPPORTED.to_string()));
        assert!(p.contains(&FUZZY_SIZE_THRESHOLD_MAX.to_string()));
        assert!(p.contains(&SEASON_FROM_EPISODES_MIN.to_string()));
        assert!(p.contains(&ENSEMBLE_NEEDS_PARTIAL.to_string()));
        let mut c = RuntimeConfig {
            link_dirs: vec!["/l".into()],
            torrent_clients: vec!["qbittorrent:http://q:8080".into()],
            exclude_older: Some(DAY),
            ..RuntimeConfig::default()
        };
        assert_eq!(validate(&mut c), vec![EXCLUDE_RECENT_OLDER.to_string()]);
        let mut c = RuntimeConfig {
            link_dirs: vec!["/l".into()],
            torrent_clients: vec!["qbittorrent:http://q:8080".into()],
            exclude_recent_search: Some(DAY),
            ..RuntimeConfig::default()
        };
        let p = validate(&mut c);
        assert!(p.contains(&SEARCH_CADENCE_EXCLUDE_RECENT.to_string()));
        assert_eq!(match_mode_invalid(), r#"matchMode must be one of: "strict" "flexible" "partial""#);
    }

    #[test]
    fn block_list_checks() {
        let mut list: Vec<String> = [
            "name:",
            "folder:a/b",
            "nameRegex:(",
            "tracker:https://t.example/announce",
            "tracker:localhost",
            "infoHash:ABCDEF",
            "sizeBelow:10",
            "sizeBelow:20",
            "sizeAbove:5",
            "bogus:x",
            "category:",
            "infoHash:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let p = check_block_list(&mut list);
        assert_eq!(
            p,
            vec![
                issue("name:", BLOCKLIST_EMPTY_VALUE),
                issue("folder:a/b", BLOCKLIST_FOLDER),
                issue("nameRegex:(", BLOCKLIST_REGEX),
                issue("tracker:https://t.example/announce", BLOCKLIST_TRACKER),
                issue("tracker:localhost", BLOCKLIST_TRACKER),
                issue("infoHash:ABCDEF", BLOCKLIST_HASH),
                issue("20", BLOCKLIST_SIZE),
                issue("bogus:x", &blocklist_type_message()),
                issue("sizeBelow > sizeAbove", BLOCKLIST_SIZE),
            ]
        );
        assert_eq!(list[11], format!("infoHash:{}", "a".repeat(40)));
        assert_eq!(
            blocklist_type_message(),
            "Blocklist item does not start with a valid prefix. Must be of name nameRegex folder folderRegex category tag tracker infoHash sizeBelow sizeAbove legacy"
        );
        assert_eq!(block_list_lines("name:a\r\n\n  \ntag:b"), vec!["name:a", "tag:b"]);
    }
}
