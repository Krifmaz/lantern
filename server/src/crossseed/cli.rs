// Ported from cross-seed v6.13.9 (src/cmd.ts, src/startup.ts, src/auth.ts, src/diff.ts, updateTorrentCache of
// src/decide.ts, restoreFromTorrentCache of src/inject.ts, clearIndexerFailures of src/indexers.ts and
// generateConfig of src/configuration.ts), Apache-2.0.
//! `lantern cross-seed <command>`: cross-seed's command line, with its commands, options and output, working on
//! Lantern's data folder (`APP_DATA_DIR`) as cross-seed's works on its config folder.
//!
//! | Command | What it does |
//! | --- | --- |
//! | `daemon` | the daemon API on its port and the jobs on their cadence |
//! | `rss`, `search` | one RSS scan, one search of every searchee |
//! | `inject` | adds the saved `.torrent` files (`outputDir`, or `--inject-dir`) |
//! | `restore` | copies `torrent_cache/` in the data folder to `outputDir`, for `inject` |
//! | `test-notification` | sends a test notification |
//! | `gen-config` | saves cross-seed's template values for the `cross_seed_*` settings not saved yet |
//! | `update-torrent-cache-trackers` | rewrites announce addresses in `torrent_cache/` |
//! | `diff`, `tree` | compares two `.torrent` files, prints one's files |
//! | `clear-indexer-failures` | takes the torrent indexers off hold |
//! | `clear-cache` | forgets decisions without a torrent and every search time |
//! | `clear-client-cache` | forgets the client torrents, `torrentDir` files, data and episodes |
//! | `api-key`, `reset-api-key` | shows Lantern's API key, makes a new one |
//!
//! Options are parsed as commander 8 parses them (`--opt value`, `--opt=value`, `-x value`, lists until the next
//! option, `--no-opt`). An option left out keeps Lantern's setting (cross-seed's defaults come from its
//! `config.js`; Lantern's settings are that file). The options are laid over the settings for the run
//! ([`super::Overlay`]) and checked with cross-seed's own checks before anything runs, as `withFullRuntime` does.
//!
//! [`run`] is the entry: it takes the arguments after `cross-seed`, the data folder and the environment, writes to
//! the two outputs given and answers the exit code.

use super::config::{issue, ms, RuntimeConfig, INVALID_URL, VERCEL};
use super::constants::{Action, LinkType, MatchMode, MediaType};
use super::inject::UNKNOWN_TRACKER;
use super::searchee::media_type_of;
use super::torrent::{decode_value, locale_cmp, Metafile, B};
use super::utils::sanitize_info_hash;
use super::{CrossSeed, Overlay};
use kit_server_core::{p, Db};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// `PROGRAM_NAME`.
pub const PROGRAM_NAME: &str = "cross-seed";
/// `PROGRAM_VERSION`: the cross-seed this command line follows.
pub const PROGRAM_VERSION: &str = "6.13.9";
/// `TORRENT_CACHE_FOLDER`: the folder in the data folder that `restore` and `update-torrent-cache-trackers` read.
pub const TORRENT_CACHE_FOLDER: &str = "torrent_cache";
/// What commander adds after every mistake in the command line (`showHelpAfterError`).
const AFTER_ERROR: &str = "(add --help for additional information)";

/// Reads one environment variable (a test passes its own).
pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Where a command works: the data folder, the web app's folder (unused by the commands, needed to open the
/// state) and the environment.
#[derive(Clone)]
pub struct Context {
    /// The data folder (`APP_DATA_DIR`): cross-seed's config folder.
    pub data_dir: PathBuf,
    /// The web app's folder (`APP_STATIC_DIR`).
    pub static_dir: PathBuf,
    /// The environment the settings read.
    pub env: Env,
    /// How long to wait between two tries of the client checks at start (cross-seed: one minute).
    pub retry_delay: Duration,
}

impl Context {
    /// The data folder and web app from the environment, as the server reads them.
    pub fn from_env() -> Context {
        Context {
            data_dir: PathBuf::from(std::env::var("APP_DATA_DIR").unwrap_or_else(|_| "/data".into())),
            static_dir: PathBuf::from(std::env::var("APP_STATIC_DIR").unwrap_or_else(|_| "/app/static".into())),
            env: Arc::new(|n| std::env::var(n).ok()),
            retry_delay: Duration::from_secs(60),
        }
    }
}

/// An output the commands write to (standard output or error, or a test's buffer).
pub type Out<'a> = &'a mut (dyn Write + Send);

// ---------------------------------------------------------------------------------------------------------------
// the options, as commander declares them

/// One option of a command, as `cmd.ts` declares it.
#[derive(Debug, Clone, Copy)]
pub struct Opt {
    /// Its flags as commander writes them: `-T, --torznab <urls...>`.
    pub flags: &'static str,
    /// What it does (commander's description).
    pub help: &'static str,
    /// The values it takes, when it takes only some.
    pub choices: &'static [&'static str],
    /// Left out of the help (`hideHelp`).
    pub hidden: bool,
}

/// An option with no fixed values.
const fn o(flags: &'static str, help: &'static str) -> Opt {
    Opt { flags, help, choices: &[], hidden: false }
}

/// An option that takes one of `choices`.
const fn c(flags: &'static str, help: &'static str, choices: &'static [&'static str]) -> Opt {
    Opt { flags, help, choices, hidden: false }
}

impl Opt {
    /// The short flag (`-T`), if any.
    fn short(&self) -> Option<&'static str> {
        self.flags.split([',', ' ']).find(|f| f.starts_with('-') && !f.starts_with("--"))
    }

    /// The long flag (`--torznab`).
    fn long(&self) -> &'static str {
        self.flags.split([',', ' ']).find(|f| f.starts_with("--")).unwrap_or("")
    }

    /// Whether it takes a value (`<value>`).
    fn takes_value(&self) -> bool {
        self.flags.contains('<')
    }

    /// Whether it takes several values (`<values...>`).
    fn variadic(&self) -> bool {
        self.flags.contains("...>")
    }

    /// Whether it is a `--no-` option that turns another off.
    fn negate(&self) -> bool {
        self.long().starts_with("--no-") && !self.takes_value()
    }

    /// The name its value is kept under: the long flag without `--` (and without `no-` for a negation).
    fn key(&self) -> &'static str {
        let l = self.long().trim_start_matches("--");
        if self.negate() {
            &l[3..]
        } else {
            l
        }
    }
}

/// `createCommandWithSharedOptions`: the options every running command takes.
const SHARED: &[Opt] = &[
    o("-T, --torznab <urls...>", "Torznab urls with apikey included (separated by spaces)"),
    o("--use-client-torrents", "Use torrents from your client for matching"),
    o("--no-use-client-torrents", "Don't use torrents from your client for matching"),
    o("--ignore-non-relevant-files-to-resume", "Ignore certain known irrelevant files when calculating resume size"),
    o("--no-ignore-non-relevant-files-to-resume", "Don't ignore certain known irrelevant files when calculating resume size"),
    o("--data-dirs <dirs...>", "Directories to use if searching by data instead of torrents (separated by spaces)"),
    c(
        "--match-mode <mode>",
        "\"strict\" will require all file names to match exactly. \"flexible\" allows for file renames. \"partial\" is like \"flexible\" but it ignores small files like .nfo/.srt if missing.",
        &["strict", "flexible", "partial"],
    ),
    o("--skip-recheck", "Skip rechecking torrents before resuming, unless necessary."),
    o("--no-skip-recheck", "Recheck every torrent before resuming, even if unnecessary."),
    o("--auto-resume-max-download <number>", "The maximum size in bytes remaining for a torrent to be resumed"),
    o("--link-category <cat>", "Torrent client category to set on linked torrents"),
    o("--link-dir <dir>", "Directory to link the data for matches to"),
    o("--link-dirs <dirs...>", "Directories to link the data for matches to"),
    o("--flat-linking", "Use flat linking directory structure (without individual tracker folders)"),
    c(
        "--link-type <type>",
        "Use links of this type to inject data-based matches into your client",
        &["symlink", "hardlink", "reflink", "reflinkOrCopy"],
    ),
    o("--max-data-depth <depth>", "Max depth to look for searchees in dataDirs"),
    o("-i, --torrent-dir <dir>", "Directory with torrent files"),
    o("-s, --output-dir <dir>", "Directory to save results in"),
    o("--include-non-videos", "Include torrents which contain non-video files"),
    o("--no-include-non-videos", "Don't include torrents which contain non-videos"),
    o("--include-single-episodes", "Include single episode torrents in the search"),
    o("--no-include-single-episodes", "Don't include single episode torrents in the search"),
    o("--season-from-episodes <decimal>", "Match season packs from episode torrents"),
    o("--no-season-from-episodes", "Don't match season packs from episode torrents"),
    o("--fuzzy-size-threshold <decimal>", "The size difference allowed to be considered a match."),
    o("-x, --exclude-older <cutoff>", "Exclude torrents first seen more than n minutes ago. Bypasses the -a flag."),
    o(
        "-r, --exclude-recent-search <cutoff>",
        "Exclude torrents which have been searched more recently than n minutes ago. Bypasses the -a flag.",
    ),
    o("-v, --verbose", "Log verbose output"),
    c(
        "-A, --action <action>",
        "If set to 'inject', cross-seed will attempt to add the found torrents to your torrent client.",
        &["save", "inject"],
    ),
    o("--torrent-clients <clients...>", "The the client prefix, readonly status, and urls of your torrent clients."),
    o("--rtorrent-rpc-url <url>", "The url of your rtorrent XMLRPC interface."),
    o("--qbittorrent-url <url>", "The url of your qBittorrent webui."),
    o("--transmission-rpc-url <url>", "The url of your Transmission RPC interface."),
    o("--deluge-rpc-url <url>", "The url of your Deluge JSON-RPC interface."),
    o("--duplicate-categories", "Create and inject using categories with the same save paths as your normal categories"),
    o(
        "--notification-webhook-urls <urls...>",
        "cross-seed will send POST requests to these urls with a JSON payload of { title, body, extra }",
    ),
    o(
        "--notification-webhook-url <url>",
        "cross-seed will send POST requests to this url with a JSON payload of { title, body, extra }",
    ),
    o("-d, --delay <delay>", "Pause duration (seconds) between searches"),
    o("--snatch-timeout <timeout>", "Timeout for unresponsive snatches"),
    o("--search-timeout <timeout>", "Timeout for unresponsive searches"),
    o("--search-limit <number>", "The number of searches before stops"),
    o("--block-list <strings...>", "The infohashes and/or strings in torrent name to block from cross-seed"),
    o("--sonarr <urls...>", "Sonarr API URL(s)"),
    o("--radarr <urls...>", "Radarr API URL(s)"),
];

/// `apiKeyOption`.
const API_KEY: Opt = o("--api-key <key>", "Provide your own API key to override the autogenerated one.");

/// The daemon's own options.
const DAEMON: &[Opt] = &[
    o("-p, --port <port>", "Listen on a custom port"),
    o("--host <host>", "Bind to a specific IP address"),
    o("--no-port", "Do not listen on any port"),
    o("--search-cadence <cadence>", "Run searches on a schedule. Format: https://github.com/vercel/ms"),
    o("--rss-cadence <cadence>", "Run an rss scan on a schedule. Format: https://github.com/vercel/ms"),
    API_KEY,
];

/// The search's own options.
const SEARCH: &[Opt] = &[
    Opt { flags: "--torrents <torrents...>", help: "torrent files separated by spaces", choices: &[], hidden: true },
    o("--no-exclude-older", "Don't Exclude torrents based on when they were first seen."),
    o("--no-exclude-recent-search", "Don't Exclude torrents based on when they were last searched."),
];

/// The inject's own options.
const INJECT: &[Opt] = &[
    o("--inject-dir <dir>", "Directory of torrent files to try to inject"),
    o(
        "--ignore-titles",
        "Searchee and candidate titles do not need to pass the fuzzy matching check (useful if `cross-seed inject` erroneously rejects by title)",
    ),
    o("--no-ignore-titles", "Searchee and candidate titles need to pass the fuzzy matching check (default)"),
];

/// One command: its name, its arguments, what it does and its options.
#[derive(Debug, Clone)]
pub struct Command {
    /// Its name.
    pub name: &'static str,
    /// Its required arguments, by name.
    pub args: &'static [&'static str],
    /// What it does.
    pub help: &'static str,
    /// Its own usage line, when commander's isn't used.
    pub usage: Option<&'static str>,
    /// Its options.
    pub opts: Vec<Opt>,
}

/// The commands, in `cmd.ts`'s order.
pub fn commands() -> Vec<Command> {
    let cmd = |name, args, help, opts: Vec<Opt>| Command { name, args, help, usage: None, opts };
    let shared = |own: &[Opt]| SHARED.iter().chain(own.iter()).copied().collect::<Vec<Opt>>();
    vec![
        cmd("gen-config", &[], "Generate a config file", vec![]),
        Command {
            name: "update-torrent-cache-trackers",
            args: &["old-announce-url", "new-announce-url"],
            help: "Update announce urls in the torrent cache",
            usage: Some("<old-announce-url> <new-announce-url>"),
            opts: vec![],
        },
        cmd("diff", &["searchee", "candidate"], "Analyze two torrent files for cross-seed compatibility", vec![]),
        cmd("tree", &["torrent"], "Print a torrent's file tree", vec![]),
        cmd("clear-indexer-failures", &[], "Clear the cached details of indexers (failures and caps)", vec![]),
        cmd(
            "clear-cache",
            &[],
            "Clear the cache without causing torrents to be re-snatched and reset the timestamps for excludeOlder and excludeRecentSearch",
            vec![],
        ),
        cmd(
            "clear-client-cache",
            &[],
            "Clear cross-seed's cache of your client's torrents. Only necessary if you have recently changed clients or modified the torrents in client and don't want to wait on the cleanup job.",
            vec![],
        ),
        cmd("api-key", &[], "Show the api key", vec![API_KEY]),
        cmd("reset-api-key", &[], "Reset the api key", vec![]),
        cmd("daemon", &[], "Start the cross-seed daemon", shared(DAEMON)),
        cmd("rss", &[], "Run an rss scan", shared(&[])),
        cmd("search", &[], "Search for cross-seeds", shared(SEARCH)),
        cmd("inject", &[], "Inject saved cross-seeds into your client (without filtering, see docs)", shared(INJECT)),
        cmd(
            "restore",
            &[],
            "Use snatched torrents from torrent_cache to attempt to restore cross seeds. Will need to run `cross-seed inject` afterwards with dataDirs configured.",
            shared(&[]),
        ),
        cmd("test-notification", &[], "Send a test notification", shared(&[])),
    ]
}

// ---------------------------------------------------------------------------------------------------------------
// parsing, as commander 8 does it

/// An option's value as given.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// A flag, or `false` from its `--no-` option.
    Bool(bool),
    /// One value.
    Str(String),
    /// Several values.
    List(Vec<String>),
}

/// A command line read: the options given (by [`Opt::key`]) and the arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// The options given.
    pub opts: BTreeMap<String, Val>,
    /// The arguments that aren't options.
    pub args: Vec<String>,
    /// `-h`/`--help` was given.
    pub help: bool,
}

impl Parsed {
    /// Option `k`, if given.
    pub fn get(&self, k: &str) -> Option<&Val> {
        self.opts.get(k)
    }

    /// Option `k` as one value.
    pub fn str(&self, k: &str) -> Option<&str> {
        match self.opts.get(k) {
            Some(Val::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// Option `k` as a flag.
    pub fn bool(&self, k: &str) -> Option<bool> {
        match self.opts.get(k) {
            Some(Val::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// Option `k` as several values.
    pub fn list(&self, k: &str) -> Option<&[String]> {
        match self.opts.get(k) {
            Some(Val::List(l)) => Some(l),
            _ => None,
        }
    }
}

/// commander's `maybeOption`: an argument that looks like an option.
fn maybe_option(a: &str) -> bool {
    a.len() > 1 && a.starts_with('-')
}

/// commander's `parseOptions` and option checks: the options of `opts` and the arguments, or commander's message
/// for the first mistake (`error: …`).
pub fn parse(opts: &[Opt], argv: &[String]) -> Result<Parsed, String> {
    let find = |flag: &str| opts.iter().find(|o| o.long() == flag || o.short() == Some(flag)).copied();
    let mut out = Parsed::default();
    let mut given: Vec<&'static str> = Vec::new();
    let mut variadic: Option<Opt> = None;
    let mut args: std::collections::VecDeque<String> = argv.iter().cloned().collect();
    let set = |out: &mut Parsed, given: &mut Vec<&'static str>, o: Opt, v: Option<String>| -> Result<(), String> {
        let key = o.key();
        match v {
            None => {
                out.opts.insert(key.into(), Val::Bool(!o.negate()));
            }
            Some(v) => {
                if !o.choices.is_empty() && !o.choices.contains(&v.as_str()) {
                    return Err(format!(
                        "error: option '{}' argument '{v}' is invalid. Allowed choices are {}.",
                        o.flags,
                        o.choices.join(", ")
                    ));
                }
                if o.variadic() {
                    let fresh = !given.contains(&key);
                    match out.opts.get_mut(key) {
                        Some(Val::List(l)) if !fresh => l.push(v),
                        _ => {
                            out.opts.insert(key.into(), Val::List(vec![v]));
                        }
                    }
                } else {
                    out.opts.insert(key.into(), Val::Str(v));
                }
            }
        }
        if !given.contains(&key) {
            given.push(key);
        }
        Ok(())
    };
    while let Some(arg) = args.pop_front() {
        if arg == "--" {
            out.args.extend(args.drain(..));
            break;
        }
        if let Some(v) = variadic {
            if !maybe_option(&arg) {
                set(&mut out, &mut given, v, Some(arg))?;
                continue;
            }
        }
        variadic = None;
        if arg == "-h" || arg == "--help" {
            out.help = true;
            continue;
        }
        if maybe_option(&arg) {
            if let Some(opt) = find(&arg) {
                if opt.takes_value() {
                    let Some(v) = args.pop_front() else {
                        return Err(format!("error: option '{}' argument missing", opt.flags));
                    };
                    set(&mut out, &mut given, opt, Some(v))?;
                    if opt.variadic() {
                        variadic = Some(opt);
                    }
                } else {
                    set(&mut out, &mut given, opt, None)?;
                }
                continue;
            }
        }
        // `-abc`: `-a` then `-bc`, or `-dVALUE`
        if arg.len() > 2 && arg.starts_with('-') && !arg.starts_with("--") {
            if let Some(opt) = find(&arg[..2]) {
                if opt.takes_value() {
                    set(&mut out, &mut given, opt, Some(arg[2..].to_string()))?;
                    if opt.variadic() {
                        variadic = Some(opt);
                    }
                } else {
                    set(&mut out, &mut given, opt, None)?;
                    args.push_front(format!("-{}", &arg[2..]));
                }
                continue;
            }
        }
        // `--opt=value`
        if arg.starts_with("--") {
            if let Some(eq) = arg.find('=') {
                if let Some(opt) = find(&arg[..eq]).filter(Opt::takes_value) {
                    set(&mut out, &mut given, opt, Some(arg[eq + 1..].to_string()))?;
                    continue;
                }
            }
        }
        if maybe_option(&arg) {
            return Err(format!("error: unknown option '{arg}'"));
        }
        out.args.push(arg);
    }
    Ok(out)
}

/// commander's help for the whole program.
pub fn program_help() -> String {
    let mut s = format!("Usage: {PROGRAM_NAME} [options] [command]\n\n{PROGRAM_NAME} v{PROGRAM_VERSION}\n\nOptions:\n");
    let mut items = vec![
        ("-V, --version".to_string(), "output the current version".to_string()),
        ("-h, --help".to_string(), "display help for command".to_string()),
    ];
    let mut cmds: Vec<(String, String)> = commands()
        .iter()
        .map(|c| {
            let mut term = c.name.to_string();
            if !c.opts.is_empty() {
                term.push_str(" [options]");
            }
            for a in c.args {
                term.push_str(&format!(" <{a}>"));
            }
            (term, c.help.to_string())
        })
        .collect();
    cmds.push(("help [command]".into(), "display help for command".into()));
    let width = items.iter().chain(cmds.iter()).map(|(t, _)| t.len()).max().unwrap_or(0);
    for (t, d) in items.drain(..) {
        s.push_str(&format!("  {t:width$}  {d}\n"));
    }
    s.push_str("\nCommands:\n");
    for (t, d) in cmds {
        s.push_str(&format!("  {t:width$}  {d}\n"));
    }
    s
}

/// commander's help for one command.
pub fn command_help(c: &Command) -> String {
    let usage = match c.usage {
        Some(u) => u.to_string(),
        None => {
            let mut u = "[options]".to_string();
            for a in c.args {
                u.push_str(&format!(" <{a}>"));
            }
            u
        }
    };
    let mut s = format!("Usage: {PROGRAM_NAME} {} {usage}\n\n{}\n", c.name, c.help);
    if !c.args.is_empty() {
        s.push_str("\nArguments:\n");
        let width = c.args.iter().map(|a| a.len()).max().unwrap_or(0);
        for a in c.args {
            s.push_str(&format!("  {a:width$}\n"));
        }
    }
    let mut items: Vec<(String, String)> = c
        .opts
        .iter()
        .filter(|o| !o.hidden)
        .map(|o| {
            let mut d = o.help.to_string();
            if !o.choices.is_empty() {
                let quoted: Vec<String> = o.choices.iter().map(|x| format!("\"{x}\"")).collect();
                d.push_str(&format!(" (choices: {})", quoted.join(", ")));
            }
            (o.flags.to_string(), d)
        })
        .collect();
    items.push(("-h, --help".into(), "display help for command".into()));
    let width = items.iter().map(|(t, _)| t.len()).max().unwrap_or(0);
    s.push_str("\nOptions:\n");
    for (t, d) in items {
        s.push_str(&format!("  {t:width$}  {d}\n"));
    }
    s
}

// ---------------------------------------------------------------------------------------------------------------
// the options laid over the settings

/// JavaScript's `parseInt(s)`: the whole number at the start of `s` (`NaN` when there is none).
pub fn js_parse_int(s: &str) -> f64 {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let (radix, digits) = if rest.len() > 1 && (rest.starts_with("0x") || rest.starts_with("0X")) { (16, &rest[2..]) } else { (10, rest) };
    let end = digits.find(|ch: char| !ch.is_digit(radix)).unwrap_or(digits.len());
    if end == 0 {
        return f64::NAN;
    }
    let n = digits[..end].chars().fold(0.0, |acc, ch| acc * radix as f64 + ch.to_digit(radix).unwrap_or(0) as f64);
    if neg {
        -n
    } else {
        n
    }
}

/// JavaScript's `parseFloat(s)`: the number at the start of `s` (`NaN` when there is none).
pub fn js_parse_float(s: &str) -> f64 {
    let t = s.trim_start();
    if let Some(r) = t.strip_prefix("Infinity").or_else(|| t.strip_prefix("+Infinity")) {
        let _ = r;
        return f64::INFINITY;
    }
    if t.starts_with("-Infinity") {
        return f64::NEG_INFINITY;
    }
    let b = t.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'-' | b'+')) {
        i = 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    let mut end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'-' | b'+')) {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            end = j;
        }
    }
    t[..end].trim_end_matches('.').parse().unwrap_or(f64::NAN)
}

/// zod's message for a number that is `NaN`.
const NAN: &str = "Expected number, received nan";

/// The deprecated client address options, with the client type each stands for (`VALIDATION_SCHEMA`'s first
/// refinement takes the first one given).
const OLD_CLIENT_URLS: [(&str, &str); 4] = [
    ("qbittorrent-url", "qbittorrent"),
    ("rtorrent-rpc-url", "rtorrent"),
    ("transmission-rpc-url", "transmission"),
    ("deluge-rpc-url", "deluge"),
];

/// Lays the options of `p` over `c` (the settings read from Lantern's) and adds the problems found in them, as
/// cross-seed's schema reads its options. `explicit_clients`: torrent clients were set (the
/// `cross_seed_torrent_clients` setting or `--torrent-clients`), rather than taken from Lantern's download clients.
pub fn apply(p: &Parsed, explicit_clients: bool, c: &mut RuntimeConfig, problems: &mut Vec<String>) {
    let list = |k: &str| p.list(k).map(<[String]>::to_vec);
    let number = |k: &str, float: bool, problems: &mut Vec<String>| -> Option<f64> {
        let v = p.str(k)?;
        let n = if float { js_parse_float(v) } else { js_parse_int(v) };
        if n.is_nan() {
            problems.push(NAN.to_string());
            return None;
        }
        Some(n)
    };
    let duration = |k: &str, problems: &mut Vec<String>| -> Option<Option<f64>> {
        match p.get(k)? {
            Val::Bool(false) => Some(None),
            Val::Str(s) if s.is_empty() => {
                problems.push("cannot have an empty string. If you want to unset it, use null or undefined.".into());
                None
            }
            Val::Str(s) => match ms(s) {
                Some(v) => Some(Some(v)),
                None => {
                    problems.push(issue(s, VERCEL));
                    None
                }
            },
            _ => {
                problems.push("Expected string, received boolean (true)".into());
                None
            }
        }
    };
    // in the schema's order, so the problems come in cross-seed's order
    if let Some(n) = number("delay", true, problems) {
        c.delay = n;
    }
    if let Some(v) = list("torznab") {
        c.torznab = v;
    }
    if let Some(v) = p.bool("use-client-torrents") {
        c.use_client_torrents = v;
    }
    if let Some(v) = p.bool("ignore-non-relevant-files-to-resume") {
        c.ignore_non_relevant_files_to_resume = v;
    }
    if let Some(v) = list("data-dirs") {
        c.data_dirs = v;
    }
    if let Some(m) = p.str("match-mode").and_then(MatchMode::parse) {
        c.match_mode = m;
    }
    if let Some(v) = p.bool("skip-recheck") {
        c.skip_recheck = v;
    }
    if let Some(n) = number("auto-resume-max-download", false, problems) {
        c.auto_resume_max_download = n as i64;
    }
    if let Some(v) = p.str("link-category") {
        c.link_category = Some(v.to_string());
    }
    if let Some(v) = list("link-dirs") {
        c.link_dirs = v;
    }
    if p.bool("flat-linking") == Some(true) {
        c.flat_linking = true;
    }
    if let Some(l) = p.str("link-type").and_then(LinkType::parse) {
        c.link_type = l;
    }
    if let Some(n) = number("max-data-depth", false, problems) {
        c.max_data_depth = n as i64;
    }
    if let Some(v) = p.str("torrent-dir") {
        c.torrent_dir = Some(v.to_string());
    }
    if let Some(v) = p.str("output-dir") {
        c.output_dir = Some(v.to_string());
    }
    if let Some(v) = p.bool("include-non-videos") {
        c.include_non_videos = v;
    }
    if let Some(v) = p.bool("include-single-episodes") {
        c.include_single_episodes = v;
    }
    match p.get("season-from-episodes") {
        Some(Val::Bool(false)) => c.season_from_episodes = None,
        Some(Val::Str(_)) => {
            if let Some(n) = number("season-from-episodes", true, problems) {
                c.season_from_episodes = Some(n);
            }
        }
        _ => {}
    }
    if let Some(n) = number("fuzzy-size-threshold", true, problems) {
        c.fuzzy_size_threshold = n;
    }
    if let Some(v) = duration("exclude-older", problems) {
        c.exclude_older = v;
    }
    if let Some(v) = duration("exclude-recent-search", problems) {
        c.exclude_recent_search = v;
    }
    if p.bool("verbose") == Some(true) {
        c.verbose = true;
    }
    if let Some(a) = p.str("action").and_then(Action::parse) {
        c.action = a;
    }
    if let Some(v) = list("torrent-clients") {
        c.torrent_clients = v;
    }
    if p.bool("duplicate-categories") == Some(true) {
        c.duplicate_categories = true;
    }
    if let Some(v) = list("notification-webhook-urls") {
        c.notification_webhook_urls = v;
    }
    if let Some(v) = duration("snatch-timeout", problems) {
        c.snatch_timeout = v;
    }
    if let Some(v) = duration("search-timeout", problems) {
        c.search_timeout = v;
    }
    if let Some(n) = number("search-limit", false, problems) {
        c.search_limit = if n == 0.0 { None } else { Some(n as i64) };
    }
    if let Some(v) = list("block-list") {
        c.block_list = v;
    }
    if let Some(v) = list("sonarr") {
        c.sonarr = v;
    }
    if let Some(v) = list("radarr") {
        c.radarr = v;
    }
    // the daemon's
    match p.get("port") {
        Some(Val::Bool(false)) => c.port = None,
        Some(Val::Str(_)) => {
            if let Some(n) = number("port", false, problems) {
                if n <= 0.0 {
                    problems.push("Number must be greater than 0".into());
                } else if n > 65535.0 {
                    problems.push("Number must be less than or equal to 65535".into());
                } else {
                    c.port = Some(n as u16);
                }
            }
        }
        _ => {}
    }
    if let Some(h) = p.str("host") {
        if h.parse::<std::net::IpAddr>().is_err() {
            problems.push("Invalid ip".into());
        }
        c.host = Some(h.to_string());
    }
    if let Some(v) = duration("search-cadence", problems) {
        c.search_cadence = v;
    }
    if let Some(v) = duration("rss-cadence", problems) {
        c.rss_cadence = v;
    }
    if let Some(k) = p.str("api-key") {
        if k.chars().count() < 24 {
            problems.push("String must contain at least 24 character(s)".into());
        }
        c.api_key = Some(k.to_string());
    }
    // the search's and the inject's
    if let Some(v) = list("torrents") {
        c.torrents = Some(v);
    }
    if let Some(v) = p.str("inject-dir") {
        c.inject_dir = Some(v.to_string());
    }
    if let Some(v) = p.bool("ignore-titles") {
        c.ignore_titles = Some(v);
    }
    // the deprecated options, as the schema's refinements turn them into the new ones
    if let Some((k, label)) = OLD_CLIENT_URLS.iter().find(|(k, _)| p.str(k).is_some()) {
        let u = p.str(k).unwrap_or_default();
        if url::Url::parse(u).is_err() {
            problems.push(issue(u, INVALID_URL));
        }
        if explicit_clients {
            problems.push("Cannot use qbittorrentUrl, rtorrentRpcUrl, transmissionRpcUrl, or delugeRpcUrl with torrentClients. Only use torrentClients.".into());
        } else {
            c.torrent_clients = vec![format!("{label}:{u}")];
        }
    }
    if let Some(d) = p.str("link-dir") {
        if p.list("link-dirs").is_some() || !c.link_dirs.is_empty() {
            problems.push("You cannot have both linkDir and linkDirs, use linkDirs only.".into());
        } else {
            c.link_dirs = vec![d.to_string()];
        }
    }
    if let Some(u) = p.str("notification-webhook-url") {
        if url::Url::parse(u).is_err() {
            problems.push(issue(u, INVALID_URL));
        }
        if !c.notification_webhook_urls.is_empty() {
            problems
                .push("You cannot have both notificationWebhookUrl and notificationWebhookUrls, use notificationWebhookUrls only.".into());
        } else {
            c.notification_webhook_urls = vec![u.to_string()];
        }
    }
}

/// The warnings cross-seed's schema logs for the options of `p` (once, before the run).
pub fn option_warnings(p: &Parsed) -> Vec<String> {
    let mut w = Vec::new();
    if p.list("torznab").is_some() {
        w.push("Lantern searches its own indexers, so --torznab is not used. Add the sites as indexers instead.".to_string());
    }
    if OLD_CLIENT_URLS.iter().any(|(k, _)| p.str(k).is_some()) {
        w.push("qbittorrentUrl, rtorrentRpcUrl, transmissionRpcUrl, and delugeRpcUrl are deprecated, use torrentClients instead.".into());
    }
    if p.str("link-dir").is_some() {
        w.push("linkDir is deprecated, use linkDirs instead.".into());
    }
    if p.str("notification-webhook-url").is_some() {
        w.push("notificationWebhookUrl is deprecated, use notificationWebhookUrls instead.".into());
    }
    if p.str("output-dir").is_some() {
        w.push(
            "Set outputDir to null to prevent any issues from occurring (https://www.cross-seed.org/docs/basics/options#outputdir)".into(),
        );
    }
    w
}

/// Whether `--verbose` is among `args` (the arguments after `cross-seed`), so the log shows debug lines.
pub fn wants_verbose(args: &[String]) -> bool {
    args.iter().take_while(|a| *a != "--").any(|a| a == "-v" || a == "--verbose")
}

// ---------------------------------------------------------------------------------------------------------------
// the entry

/// Runs `lantern cross-seed <args>`: writes what the command prints to `out`, its problems to `err`, and answers
/// the exit code (0: done; 1: a mistake in the command line, the settings or the run).
pub async fn run(args: &[String], ctx: &Context, out: Out<'_>, err: Out<'_>) -> i32 {
    let Some(first) = args.first() else {
        let _ = write!(err, "{}", program_help());
        return 1;
    };
    match first.as_str() {
        "-V" | "--version" => {
            let _ = writeln!(out, "{PROGRAM_VERSION}");
            return 0;
        }
        "-h" | "--help" => {
            let _ = write!(out, "{}", program_help());
            return 0;
        }
        "help" => {
            match args.get(1).and_then(|n| commands().into_iter().find(|c| c.name == n)) {
                Some(c) => {
                    let _ = write!(out, "{}", command_help(&c));
                }
                None => {
                    let _ = write!(out, "{}", program_help());
                }
            }
            return 0;
        }
        _ => {}
    }
    if maybe_option(first) {
        let _ = writeln!(err, "error: unknown option '{first}'\n{AFTER_ERROR}");
        return 1;
    }
    let Some(cmd) = commands().into_iter().find(|c| c.name == first) else {
        let _ = writeln!(err, "error: unknown command '{first}'\n{AFTER_ERROR}");
        return 1;
    };
    let parsed = match parse(&cmd.opts, &args[1..]) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(err, "{e}\n{AFTER_ERROR}");
            return 1;
        }
    };
    if parsed.help {
        let _ = write!(out, "{}", command_help(&cmd));
        return 0;
    }
    if let Some(missing) = cmd.args.get(parsed.args.len()) {
        let _ = writeln!(err, "error: missing required argument '{missing}'\n{AFTER_ERROR}");
        return 1;
    }
    match cmd.name {
        "diff" => return diff_cmd(&parsed.args[0], &parsed.args[1], out, err),
        "tree" => return tree_cmd(&parsed.args[0], out, err),
        _ => {}
    }
    // `withMinimalRuntime`: the data folder and its tables
    let st = match crate::AppState::open(&ctx.data_dir, ctx.static_dir.clone(), {
        let env = ctx.env.clone();
        move |n| env(n)
    })
    .await
    {
        Ok(st) => st,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return 1;
        }
    };
    let res = match cmd.name {
        "gen-config" => gen_config(&st, out).await,
        "update-torrent-cache-trackers" => {
            update_torrent_cache(&st.data_dir.join(TORRENT_CACHE_FOLDER), &parsed.args[0], &parsed.args[1], out, err)
        }
        "clear-indexer-failures" => clear_indexer_failures(&st),
        "clear-cache" => clear_cache(&st, out),
        "clear-client-cache" => clear_client_cache(&st, out),
        "api-key" => get_api_key(&st).await.map(|k| {
            let _ = writeln!(out, "{k}");
        }),
        "reset-api-key" => reset_api_key(&st).await.map(|k| {
            let _ = writeln!(out, "{k}");
            // only the key goes to the output, as cross-seed prints it; the reminder goes with the problems
            let _ = writeln!(err, "Restart Lantern so a running server uses the new key, and give it to your apps.");
        }),
        _ => full_runtime(&cmd, parsed, &st, ctx, out, err).await,
    };
    match res {
        Ok(()) => 0,
        Err(e) => {
            if !e.is_empty() {
                let _ = writeln!(err, "{e}");
            }
            1
        }
    }
}

/// `lantern cross-seed …` from the real command line: the data folder from the environment, standard output and
/// error.
pub async fn main(args: Vec<String>) -> i32 {
    let ctx = Context::from_env();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    run(&args, &ctx, &mut out, &mut err).await
}

// ---------------------------------------------------------------------------------------------------------------
// the commands that need only the data folder

/// `generateConfig`: makes `torrent_cache/` in the data folder and, when no `cross_seed_*` setting is saved yet,
/// saves cross-seed's template values for them (Lantern's settings are cross-seed's `config.js`).
async fn gen_config(st: &crate::AppState, out: Out<'_>) -> Result<(), String> {
    std::fs::create_dir_all(st.data_dir.join(TORRENT_CACHE_FOLDER))
        .map_err(|e| format!("The folder {} can't be made: {e}", st.data_dir.join(TORRENT_CACHE_FOLDER).display()))?;
    let saved = st.settings.saved().await.map_err(|e| e.detail)?;
    let keys: Vec<&str> = crate::SETTINGS.keys.iter().map(|k| k.key).filter(|k| k.starts_with("cross_seed_")).collect();
    if keys.iter().any(|k| saved.contains_key(*k)) {
        let _ = writeln!(out, "Configuration file already exists.");
        return Ok(());
    }
    let mut values = serde_json::Map::new();
    for k in keys {
        if let Some(d) = st.settings.def(k) {
            values.insert(k.to_string(), d.default.clone());
        }
    }
    st.settings.set_many(&serde_json::Value::Object(values)).await.map_err(|e| e.detail)?;
    let _ = writeln!(
        out,
        "Configuration file created at {} (the cross_seed_* settings; change them on the Settings page)",
        st.data_dir.join("app.db").display()
    );
    Ok(())
}

/// JavaScript's `str.replace(old, new)` with a text pattern: the first `old` only, with `$$`, `$&`, `` $` `` and
/// `$'` in `new` as JavaScript reads them.
pub fn js_replace(s: &str, old: &str, new: &str) -> String {
    let Some(at) = s.find(old) else { return s.to_string() };
    let mut rep = String::new();
    let mut chars = new.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            match chars.peek() {
                Some('$') => {
                    rep.push('$');
                    chars.next();
                }
                Some('&') => {
                    rep.push_str(old);
                    chars.next();
                }
                Some('`') => {
                    rep.push_str(&s[..at]);
                    chars.next();
                }
                Some('\'') => {
                    rep.push_str(&s[at + old.len()..]);
                    chars.next();
                }
                _ => rep.push('$'),
            }
        } else {
            rep.push(ch);
        }
    }
    format!("{}{rep}{}", &s[..at], &s[at + old.len()..])
}

/// A bencoded byte string.
fn bstr(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(format!("{}:", b.len()).as_bytes());
    out.extend_from_slice(b);
}

/// `updateTorrentCache`: in every `.torrent` of `dir` whose announce addresses hold `old`, the first `old` of each
/// replaced by `new`, each change printed.
pub fn update_torrent_cache(dir: &Path, old: &str, new: &str, out: Out<'_>, err: Out<'_>) -> Result<(), String> {
    let paths = super::index::find_all_torrent_files_in_dir(dir);
    let _ = writeln!(out, "Found {} files in cache, processing...", paths.len());
    let mut count = 0;
    for path in &paths {
        let res = (|| -> Result<(), String> {
            let bytes = std::fs::read(path).map_err(|e| format!("Error: {e}"))?;
            let top = decode_value(&bytes).filter(|t| matches!(t, B::Dict(_))).ok_or("Error: Invalid data: Missing delimiter")?;
            let announce = top.get("announce").map(B::js_string);
            let list: Option<Vec<Vec<String>>> = top
                .get("announce-list")
                .and_then(B::list)
                .map(|tiers| tiers.iter().map(|t| t.list().unwrap_or_default().iter().map(B::js_string).collect()).collect());
            let in_announce = announce.as_ref().is_some_and(|a| a.contains(old));
            let in_list = list.as_ref().is_some_and(|l| l.iter().any(|t| t.iter().any(|u| u.contains(old))));
            if !in_announce && !in_list {
                return Ok(());
            }
            count += 1;
            let _ = writeln!(out, "#{count}: {path}");
            let mut updated = false;
            let mut data = bytes.clone();
            if let Some(a) = &announce {
                let na = js_replace(a, old, new);
                if *a != na {
                    updated = true;
                    let _ = writeln!(out, "--- {a} -> {na}");
                    let mut v = Vec::new();
                    bstr(&mut v, na.as_bytes());
                    data = super::torrent::with_top_key(&data, "announce", &v).ok_or("Error: Invalid data")?;
                }
            }
            if let Some(tiers) = &list {
                let mut v = vec![b'l'];
                for tier in tiers {
                    v.push(b'l');
                    for u in tier {
                        let nu = js_replace(u, old, new);
                        if *u != nu {
                            updated = true;
                            let _ = writeln!(out, "--- {u} -> {nu}");
                        }
                        bstr(&mut v, nu.as_bytes());
                    }
                    v.push(b'e');
                }
                v.push(b'e');
                data = super::torrent::with_top_key(&data, "announce-list", &v).ok_or("Error: Invalid data")?;
            }
            if updated {
                std::fs::write(path, &data).map_err(|e| format!("Error: {e}"))?;
            }
            Ok(())
        })();
        if let Err(e) = res {
            let _ = writeln!(err, "Error reading {path}: {e}");
        }
    }
    Ok(())
}

/// `clearIndexerFailures`: every torrent indexer off hold (cross-seed's indexer `status` and `retry_after`; in
/// Lantern the indexers' failure count and hold).
fn clear_indexer_failures(st: &crate::AppState) -> Result<(), String> {
    for ix in st.indexers.list().map_err(|e| e.detail)? {
        if ix.protocol() != crate::indexers::release::Protocol::Torrent {
            continue;
        }
        st.db
            .execute("UPDATE indexers SET failures=0, disabled_until=0, initial_failure=0, most_recent_failure=0 WHERE id=?", p![ix.id])
            .map_err(|e| e.detail)?;
    }
    Ok(())
}

/// `clear-cache`: the decisions without a torrent (`decision` rows without an info hash) and every search time
/// (`timestamp`: the searchees' and the grabs').
fn clear_cache(st: &crate::AppState, out: Out<'_>) -> Result<(), String> {
    let _ = writeln!(out, "Clearing cache...");
    st.db
        .execute_batch("DELETE FROM cross_seeds WHERE info_hash=''; DELETE FROM cross_seed_timestamps; DELETE FROM cross_seed_searches;")
        .map_err(|e| e.detail)
}

/// `clear-client-cache`: the client torrents, `torrentDir`'s files, the data and the episodes.
fn clear_client_cache(st: &crate::AppState, out: Out<'_>) -> Result<(), String> {
    let _ = writeln!(out, "Clearing client cache...");
    st.db
        .execute_batch(
            "DELETE FROM cross_seed_torrents; DELETE FROM cross_seed_client_searchees; DELETE FROM cross_seed_data; DELETE FROM cross_seed_ensemble;",
        )
        .map_err(|e| e.detail)
}

/// `generateApiKey`: 24 random bytes in hex.
fn generate_api_key() -> String {
    use rand::Rng;
    let mut b = [0u8; 24];
    rand::rng().fill(&mut b[..]);
    hex::encode(b)
}

/// `resetApiKey`: a new API key, saved as Lantern's `api_key` (apps that use the old one need the new one).
async fn reset_api_key(st: &crate::AppState) -> Result<String, String> {
    if st.settings.env_locked("api_key") {
        return Err("The API key is set by APP_API_KEY in the environment, so it can't be changed here. Change APP_API_KEY instead.".into());
    }
    let key = generate_api_key();
    st.settings.set_many(&serde_json::json!({ "api_key": key })).await.map_err(|e| e.detail)?;
    Ok(key)
}

/// `getApiKey`: Lantern's API key, made first when there is none. As in cross-seed, `--api-key` here changes
/// nothing (the command doesn't read its options).
async fn get_api_key(st: &crate::AppState) -> Result<String, String> {
    let key = st.settings.str("api_key");
    if key.is_empty() {
        return reset_api_key(st).await;
    }
    Ok(key)
}

// ---------------------------------------------------------------------------------------------------------------
// the commands that run cross-seed

/// `verifyDir`: whether `dir` is a folder that can be read (and written, with `write`); cross-seed's message when
/// it isn't.
fn verify_dir(dir: &str, name: &str, write: bool) -> Result<(), String> {
    let res = (|| -> Result<(), &'static str> {
        let p = Path::new(dir);
        if !p.exists() {
            return Err("does not exist");
        }
        if !p.is_dir() {
            return Err("not a directory");
        }
        if std::fs::read_dir(p).is_err() {
            return Err("no read permissions");
        }
        if write {
            let temp = p.join(name);
            if std::fs::write(&temp, name).is_err() {
                return Err("no write permissions");
            }
            if !temp.exists() {
                return Err("no write permissions - could not verify test file");
            }
            let _ = std::fs::remove_file(&temp);
        }
        Ok(())
    })();
    res.map_err(|m| format!("\tYour {name} \"{dir}\" has invalid permissions: {m}."))
}

/// "the error" or "the errors", as cross-seed words it.
fn errors_word(n: usize) -> &'static str {
    if n > 1 {
        "errors"
    } else {
        "error"
    }
}

/// `checkConfigPaths`: the folders exist and can be used (`outputDir` and `linkDirs` are made when missing), and
/// files can be linked from each data folder.
fn check_config_paths(cfg: &RuntimeConfig, out: Out<'_>, err: Out<'_>) -> Result<(), String> {
    let mut failures = 0;
    let fail = |m: String, err: &mut (dyn Write + Send)| {
        let _ = writeln!(err, "{m}");
    };
    if let Some(t) = &cfg.torrent_dir {
        if let Err(m) = verify_dir(t, "torrentDir", false) {
            fail(m, err);
            failures += 1;
        }
    }
    if let Some(o) = &cfg.output_dir {
        if !Path::new(o).exists() {
            let _ = writeln!(out, "Creating outputDir: {o}");
            let _ = std::fs::create_dir_all(o);
        }
        if let Err(m) = verify_dir(o, "outputDir", true) {
            fail(m, err);
            failures += 1;
        }
    }
    for (i, l) in cfg.link_dirs.iter().enumerate() {
        let name = format!("linkDir{i}");
        if !Path::new(l).exists() {
            let _ = writeln!(out, "Creating {name}: {l}");
            let _ = std::fs::create_dir_all(l);
        }
        if let Err(m) = verify_dir(l, &name, true) {
            fail(m, err);
            failures += 1;
        }
    }
    for (i, d) in cfg.data_dirs.iter().enumerate() {
        if let Err(m) = verify_dir(d, &format!("dataDir{i}"), false) {
            fail(m, err);
            failures += 1;
        }
    }
    if let Some(d) = &cfg.inject_dir {
        if let Err(m) = verify_dir(d, "injectDir", true) {
            fail(m, err);
            failures += 1;
        }
    }
    if !cfg.link_dirs.is_empty() {
        for (i, d) in cfg.data_dirs.iter().enumerate() {
            let name = format!("dataDir{i}");
            match super::action::test_linking(Path::new(d), &format!("{name}Src.cross-seed"), &format!("{name}Dest.cross-seed"), cfg) {
                Ok(true) => {}
                Ok(false) => fail("Failed to link from dataDirs to linkDirs.".into(), err),
                Err(e) => {
                    fail(e, err);
                    fail("Failed to link from dataDirs to linkDirs.".into(), err);
                    failures += 1;
                }
            }
        }
    }
    if failures > 0 {
        return Err(format!("\tYour configuration is invalid, please see the {} above for details.", errors_word(failures)));
    }
    Ok(())
}

/// `doStartupValidation`'s client check with its `retry`: every client answers, tried again up to five times.
async fn validate_clients(xs: &CrossSeed, cfg: &RuntimeConfig, ctx: &Context, err: Out<'_>) -> Result<(), String> {
    let retries = 5;
    for i in 0..=retries {
        let mut failed = None;
        for client in xs.clients(cfg).await {
            if let Err(e) = client.validate_config().await {
                failed = Some(e);
                break;
            }
        }
        let Some(e) = failed else { return Ok(()) };
        let retry_msg = if i < retries { format!(", retrying in {}s", ctx.retry_delay.as_secs()) } else { String::new() };
        let _ = writeln!(err, "Attempt {}/{} failed{retry_msg}: {e}", i + 1, retries + 1);
        if i >= retries {
            break;
        }
        tokio::time::sleep(ctx.retry_delay).await;
    }
    Err("\tYour configuration is invalid, please see the error above for details.".into())
}

/// `withFullRuntime`: the options laid over the settings and checked (`parseRuntimeConfigAndLogErrors`), the
/// folders and clients checked (`doStartupValidation`), then the command.
async fn full_runtime(
    cmd: &Command,
    parsed: Parsed,
    st: &crate::AppState,
    ctx: &Context,
    out: Out<'_>,
    err: Out<'_>,
) -> Result<(), String> {
    let xs = st.cross_seed.clone();
    let explicit_clients = !st.settings.str("cross_seed_torrent_clients").trim().is_empty() || parsed.list("torrent-clients").is_some();
    let _ = writeln!(out, "{PROGRAM_NAME} v{PROGRAM_VERSION} (Lantern {})", crate::VERSION);
    let _ = writeln!(out, "Validating your configuration...");
    for w in option_warnings(&parsed) {
        let _ = writeln!(err, "{w}");
    }
    let p = parsed.clone();
    xs.set_overlay(Some(Overlay {
        apply: Box::new(move |c, problems| apply(&p, explicit_clients, c, problems)),
        scheduled: cmd.name == "daemon",
    }));
    let (cfg, problems) = xs.config();
    if !problems.is_empty() {
        for p in &problems {
            let _ = writeln!(err, "Configuration:\n\t{}\n\t(https://www.cross-seed.org/docs/basics/options)\n", p.replace('\n', "\n\t"));
        }
        return Err(format!("Your configuration is invalid, please see the {} above for details.", errors_word(problems.len())));
    }
    check_config_paths(&cfg, out, err)?;
    validate_clients(&xs, &cfg, ctx, err).await?;
    let _ = writeln!(out, "Your configuration is valid!");
    match cmd.name {
        "daemon" => daemon(&xs, &cfg, out).await,
        "rss" => {
            xs.index_torrents_and_data_dirs(&cfg, true).await;
            xs.scan_rss_feeds().await.map_err(|e| e.detail)
        }
        "search" => xs.bulk_search(&super::config::Override::default()).await.map(|_| ()).map_err(|e| e.detail),
        "inject" => {
            xs.inject_saved_torrents(&cfg).await;
            Ok(())
        }
        "restore" => {
            restore_from_torrent_cache(&xs, &st.data_dir.join(TORRENT_CACHE_FOLDER), &cfg, out, err);
            Ok(())
        }
        "test-notification" => {
            super::notify::send_test_notification(&cfg.notification_webhook_urls).await;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The `daemon` command: the first look over the torrents and data, then the daemon API on `port` (at `host`)
/// and the jobs, until Lantern is stopped.
async fn daemon(xs: &CrossSeed, cfg: &RuntimeConfig, out: Out<'_>) -> Result<(), String> {
    xs.index_torrents_and_data_dirs(cfg, true).await;
    let listener = match cfg.port {
        Some(port) => {
            let host: std::net::IpAddr = cfg.host.as_deref().unwrap_or("0.0.0.0").parse().unwrap_or([0, 0, 0, 0].into());
            let addr = std::net::SocketAddr::new(host, port);
            let l = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
                format!("The cross-seed API can't use port {port}: {e}. Pick a free port with --port, or turn it off with --no-port.")
            })?;
            let _ = writeln!(out, "Server is running on port {port}, ^C to stop.");
            let _ = out.flush();
            Some(l)
        }
        None => None,
    };
    let jobs = xs.jobs_loop();
    let server = async {
        match listener {
            Some(l) => {
                let _ = axum::serve(l, super::daemon::router(xs.clone())).await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = jobs => {}
        _ = server => {}
        _ = crate::shutdown_signal() => {
            let _ = writeln!(out, "Server stopped");
        }
    }
    Ok(())
}

/// `restoreFromTorrentCache`: copies every `.torrent` of `cache` to `outputDir`, named as cross-seed names saved
/// torrents (`.cached.torrent`), so `inject` can add them with the data of `dataDirs`.
pub fn restore_from_torrent_cache(xs: &CrossSeed, cache: &Path, cfg: &RuntimeConfig, out: Out<'_>, err: Out<'_>) {
    let output_dir = PathBuf::from(cfg.output_dir.clone().unwrap_or_else(|| xs.output_dir().to_string_lossy().into_owned()));
    let paths = super::index::find_all_torrent_files_in_dir(cache);
    if paths.is_empty() {
        let _ = writeln!(out, "No .torrent files found to restore from cache");
        return;
    }
    let _ = writeln!(out, "Found {} .torrent files to restore from cache", paths.len());
    let host_to_name = xs.host_to_name();
    let mut copied = 0;
    let mut missing_metadata = 0;
    let total = paths.len();
    let mut copy = |from: &str, dest: &Path, info_hash: Option<&str>, out: &mut (dyn Write + Send)| -> std::io::Result<()> {
        if dest.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&output_dir)?;
        std::fs::copy(from, dest)?;
        copied += 1;
        if copied % 1000 == 0 {
            let shown = dest.to_string_lossy().into_owned();
            let shown = match info_hash {
                Some(h) => shown.replace(h, &sanitize_info_hash(h)),
                None => shown,
            };
            let _ = writeln!(out, "({copied}/{total}) {shown}");
        }
        Ok(())
    };
    for path in &paths {
        let res = (|| -> std::io::Result<()> {
            match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Metafile::decode(&b)) {
                Ok(meta) => {
                    let tracker =
                        meta.trackers.iter().find_map(|h| host_to_name.iter().find(|(host, _)| host == h).map(|(_, n)| n.clone()));
                    let media = media_type_of(&meta.title, &meta.files);
                    let dest = super::action::get_torrent_save_path(
                        &meta,
                        media,
                        tracker.as_deref().unwrap_or(UNKNOWN_TRACKER),
                        &output_dir,
                        true,
                    );
                    copy(path, &dest, Some(&meta.info_hash), out)?;
                    if tracker.is_none() {
                        missing_metadata += 1;
                    }
                }
                Err(e) => {
                    let _ = writeln!(err, "Failure when processing {path}, filename metadata will be unknown: {e}");
                    let base = super::searchee::basename(path);
                    let info_hash = base.split('.').next().unwrap_or_default().to_string();
                    let dest = output_dir.join(format!("[{}][{UNKNOWN_TRACKER}]{base}", MediaType::Other.as_str()));
                    copy(path, &dest, Some(&info_hash), out)?;
                    missing_metadata += 1;
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            let _ = writeln!(err, "Failed to copy {path}: {e}");
        }
    }
    let _ = writeln!(
        out,
        "Copied {copied}/{total} .torrent files from cache to outputDir, run \"cross-seed inject\" to inject into client using your dataDirs"
    );
    if missing_metadata > 0 {
        let _ = writeln!(out, "Copied {missing_metadata} .torrent files without filename metadata");
    }
    let _ =
        writeln!(out, "Some of the restored torrents may be unregistered, you will need to remove them from your client after injecting");
}

// ---------------------------------------------------------------------------------------------------------------
// tree and diff: Node's util.inspect of a torrent's files

/// A JavaScript value as `console.log` shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Js {
    /// A string.
    Str(String),
    /// A number.
    Num(f64),
    /// An array.
    Arr(Vec<Js>),
    /// A plain object, keys in their order.
    Obj(Vec<(String, Js)>),
}

/// Node's `strEscape`: a string in single quotes (double or backticks when it holds single quotes), escaped.
fn js_quote(s: &str) -> String {
    let q = if !s.contains('\'') {
        '\''
    } else if !s.contains('"') {
        '"'
    } else if !s.contains('`') && !s.contains("${") {
        '`'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for ch in s.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\\' => out.push_str("\\\\"),
            ch if ch == q => {
                out.push('\\');
                out.push(ch);
            }
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => out.push_str(&format!("\\x{:02X}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push(q);
    out
}

/// A key as Node shows it: bare when it is a plain name (`^[a-zA-Z_][a-zA-Z_0-9]*$`), else quoted.
fn js_key(k: &str) -> String {
    let plain = k.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if plain {
        k.to_string()
    } else {
        js_quote(k)
    }
}

/// `util.inspect`'s state while it goes down a value.
struct Ctx {
    /// `indentationLvl`.
    indent: usize,
    /// `currentDepth`: the depth of the object looked at last.
    current_depth: usize,
}

/// `breakLength`.
const BREAK_LENGTH: usize = 80;
/// `compact`.
const COMPACT: usize = 3;
/// `maxArrayLength`.
const MAX_ARRAY_LENGTH: usize = 100;

/// Node's `util.inspect(v)` with its defaults (what `console.log` prints).
pub fn inspect(v: &Js) -> String {
    format_value(&mut Ctx { indent: 0, current_depth: 0 }, v, 0)
}

/// JavaScript's `Math.round`.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn format_value(ctx: &mut Ctx, v: &Js, depth: usize) -> String {
    match v {
        Js::Str(s) => js_quote(s),
        Js::Num(n) => super::utils::js_number(*n),
        Js::Arr(items) if items.is_empty() => "[]".into(),
        Js::Obj(items) if items.is_empty() => "{}".into(),
        Js::Arr(items) => {
            ctx.current_depth = depth;
            let mut output: Vec<String> = Vec::new();
            for x in items.iter().take(MAX_ARRAY_LENGTH) {
                ctx.indent += 2;
                output.push(format_value(ctx, x, depth + 1));
                ctx.indent -= 2;
            }
            if items.len() > MAX_ARRAY_LENGTH {
                let rest = items.len() - MAX_ARRAY_LENGTH;
                output.push(format!("... {rest} more item{}", if rest > 1 { "s" } else { "" }));
            }
            reduce_to_single_string(ctx, output, "[", "]", true, depth, items)
        }
        Js::Obj(items) => {
            ctx.current_depth = depth;
            let mut output = Vec::new();
            for (k, x) in items {
                ctx.indent += 2;
                let s = format_value(ctx, x, depth + 1);
                ctx.indent -= 2;
                output.push(format!("{}: {s}", js_key(k)));
            }
            reduce_to_single_string(ctx, output, "{", "}", false, depth, &[])
        }
    }
}

/// Node's `reduceToSingleString`: on one line when the entries fit in `breakLength` and few levels are nested,
/// else one entry per line (arrays of more than six entries grouped in columns first).
fn reduce_to_single_string(ctx: &Ctx, mut output: Vec<String>, open: &str, close: &str, array: bool, depth: usize, value: &[Js]) -> String {
    let entries = output.len();
    if array && entries > 6 {
        output = group_array_elements(ctx, output, value);
    }
    if ctx.current_depth - depth < COMPACT && entries == output.len() {
        let start = output.len() + ctx.indent + open.len() + 10;
        if is_below_break_length(&output, start) {
            let joined = output.join(", ");
            if !joined.contains('\n') {
                return format!("{open} {joined} {close}");
            }
        }
    }
    let ind = format!("\n{}", " ".repeat(ctx.indent));
    format!("{open}{ind}  {}{ind}{close}", output.join(&format!(",{ind}  ")))
}

/// Node's `isBelowBreakLength`.
fn is_below_break_length(output: &[String], start: usize) -> bool {
    let mut total = output.len() + start;
    if total + output.len() > BREAK_LENGTH {
        return false;
    }
    for o in output {
        total += o.chars().count();
        if total > BREAK_LENGTH {
            return false;
        }
    }
    true
}

/// Node's `groupArrayElements`: short entries of a long array in columns.
fn group_array_elements(ctx: &Ctx, output: Vec<String>, value: &[Js]) -> Vec<String> {
    let mut total = 0usize;
    let mut max_len = 0usize;
    let mut output_len = output.len();
    if value.len() > MAX_ARRAY_LENGTH {
        output_len -= 1;
    }
    let sep = 2;
    let data_len: Vec<usize> = output[..output_len].iter().map(|o| o.chars().count()).collect();
    for &l in &data_len {
        total += l + sep;
        max_len = max_len.max(l);
    }
    let actual_max = max_len + sep;
    if actual_max * 3 + ctx.indent < BREAK_LENGTH && (total as f64 / actual_max as f64 > 5.0 || max_len <= 6) {
        let average_bias = (actual_max as f64 - total as f64 / output.len() as f64).sqrt();
        let biased_max = (actual_max as f64 - 3.0 - average_bias).max(1.0);
        let columns = js_round((2.5 * biased_max * output_len as f64).sqrt() / biased_max)
            .min(((BREAK_LENGTH - ctx.indent) / actual_max) as f64)
            .min((COMPACT * 4) as f64)
            .min(15.0) as usize;
        if columns <= 1 {
            return output;
        }
        let mut max_line = Vec::new();
        for i in 0..columns {
            let mut line = 0;
            let mut j = i;
            while j < output_len {
                line = line.max(data_len[j]);
                j += columns;
            }
            max_line.push(line + sep);
        }
        let numbers = value.iter().all(|v| matches!(v, Js::Num(_)));
        let mut tmp = Vec::new();
        let mut i = 0;
        while i < output_len {
            let max = (i + columns).min(output_len);
            let mut s = String::new();
            let mut j = i;
            while j < max - 1 {
                let cell = format!("{}, ", output[j]);
                let pad = max_line[j - i].saturating_sub(cell.chars().count());
                if numbers {
                    s.push_str(&" ".repeat(pad));
                    s.push_str(&cell);
                } else {
                    s.push_str(&cell);
                    s.push_str(&" ".repeat(pad));
                }
                j += 1;
            }
            if numbers {
                let pad = (max_line[j - i] - sep).saturating_sub(data_len[j]);
                s.push_str(&" ".repeat(pad));
            }
            s.push_str(&output[j]);
            tmp.push(s);
            i += columns;
        }
        if value.len() > MAX_ARRAY_LENGTH {
            tmp.push(output[output_len].clone());
        }
        return tmp;
    }
    output
}

/// A torrent's file as cross-seed's `Metafile` holds it: `{ name, path, length }` for a one-file torrent,
/// `{ name, length, path }` for a folder's.
fn file_js(f: &super::searchee::File, single: bool) -> Js {
    let (name, path, length) = (
        ("name".to_string(), Js::Str(f.name.clone())),
        ("path".to_string(), Js::Str(f.path.clone())),
        ("length".to_string(), Js::Num(f.length as f64)),
    );
    Js::Obj(if single { vec![name, path, length] } else { vec![name, length, path] })
}

/// The `Metafile` of `tree` without `category`, `isSingleFileTorrent`, `raw` and `tags`, in its fields' order.
pub fn tree_js(m: &Metafile) -> Js {
    Js::Obj(vec![
        ("infoHash".into(), Js::Str(m.info_hash.clone())),
        ("length".into(), Js::Num(m.length as f64)),
        ("name".into(), Js::Str(m.name.clone())),
        ("title".into(), Js::Str(m.title.clone())),
        ("pieceLength".into(), Js::Num(m.piece_length as f64)),
        ("files".into(), Js::Arr(m.files.iter().map(|f| file_js(f, m.is_single_file_torrent)).collect())),
        ("trackers".into(), Js::Arr(m.trackers.iter().map(|t| Js::Str(t.clone())).collect())),
    ])
}

/// `parseTorrentFromPath`, with Node's message when the file can't be read.
fn read_torrent(path: &str) -> Result<Metafile, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        let why = match e.kind() {
            std::io::ErrorKind::NotFound => "ENOENT: no such file or directory".to_string(),
            std::io::ErrorKind::PermissionDenied => "EACCES: permission denied".to_string(),
            _ => e.to_string(),
        };
        format!("Error: {why}, open '{path}'")
    })?;
    Metafile::decode(&bytes).map_err(|e| format!("Error: {e}"))
}

/// The `tree` command.
fn tree_cmd(path: &str, out: Out<'_>, err: Out<'_>) -> i32 {
    let _ = writeln!(out, "Use `cross-seed diff` to compare two .torrent files");
    match read_torrent(path) {
        Ok(m) => {
            let _ = writeln!(out, "{}", inspect(&tree_js(&m)));
            0
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

/// The assertion module's view of a value: `compact: false` and keys sorted, one entry per line.
fn inspect_expanded(v: &Js, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match v {
        Js::Str(s) => js_quote(s),
        Js::Num(n) => super::utils::js_number(*n),
        Js::Arr(items) if items.is_empty() => "[]".into(),
        Js::Obj(items) if items.is_empty() => "{}".into(),
        Js::Arr(items) => {
            let inner: Vec<String> = items.iter().map(|x| format!("{pad}  {}", inspect_expanded(x, indent + 2))).collect();
            format!("[\n{}\n{pad}]", inner.join(",\n"))
        }
        Js::Obj(items) => {
            let mut sorted = items.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let inner: Vec<String> =
                sorted.iter().map(|(k, x)| format!("{pad}  {}: {}", js_key(k), inspect_expanded(x, indent + 2))).collect();
            format!("{{\n{}\n{pad}}}", inner.join(",\n"))
        }
    }
}

/// The lines of `a` and `b` with what each holds that the other doesn't: `+ ` (actual), `- ` (expected) or
/// `  ` (both), by their longest common run of lines.
fn line_diff(a: &[&str], b: &[&str]) -> Vec<String> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(format!("  {}", a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push(format!("+ {}", a[i]));
            i += 1;
        } else {
            out.push(format!("- {}", b[j]));
            j += 1;
        }
    }
    out
}

/// `diff`: both lists printed as Node prints them when they are deeply equal; else the assertion's error with its
/// line diff (`+` the first torrent's, `-` the second's).
pub fn diff_values(a: &Js, b: &Js) -> String {
    if strip_order(a) == strip_order(b) {
        return format!("{}\nTorrents are equal", inspect(a));
    }
    let (ea, eb) = (inspect_expanded(a, 0), inspect_expanded(b, 0));
    let la: Vec<&str> = ea.lines().collect();
    let lb: Vec<&str> = eb.lines().collect();
    let body = line_diff(&la, &lb).join("\n");
    let message = format!("Expected values to be strictly deep-equal:\n+ actual - expected\n\n{body}");
    format!(
        "AssertionError [ERR_ASSERTION]: {message} {{\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: {},\n  expected: {},\n  operator: 'deepStrictEqual'\n}}",
        inspect_nested(a),
        inspect_nested(b)
    )
}

/// `v` as Node shows it one level into an object.
fn inspect_nested(v: &Js) -> String {
    format_value(&mut Ctx { indent: 2, current_depth: 1 }, v, 1)
}

/// `v` with every object's keys sorted, as `deepStrictEqual` compares objects (key order doesn't count).
fn strip_order(v: &Js) -> Js {
    match v {
        Js::Arr(items) => Js::Arr(items.iter().map(strip_order).collect()),
        Js::Obj(items) => {
            let mut sorted: Vec<(String, Js)> = items.iter().map(|(k, x)| (k.clone(), strip_order(x))).collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            Js::Obj(sorted)
        }
        x => x.clone(),
    }
}

/// `diffCmd`: the two torrents' files, sorted as cross-seed sorts them, compared.
pub fn diff_files(first: &Metafile, second: &Metafile) -> String {
    let mut f1 = first.files.clone();
    let mut f2 = second.files.clone();
    let by: fn(&super::searchee::File, &super::searchee::File) -> std::cmp::Ordering = if f1.len() == 1 {
        |a, b| b.length.cmp(&a.length)
    } else if f2.len() == 1 {
        |a, b| a.length.cmp(&b.length)
    } else {
        |a, b| locale_cmp(&a.path, &b.path)
    };
    f1.sort_by(by);
    f2.sort_by(by);
    let a = Js::Arr(f1.iter().map(|f| file_js(f, first.is_single_file_torrent)).collect());
    let b = Js::Arr(f2.iter().map(|f| file_js(f, second.is_single_file_torrent)).collect());
    diff_values(&a, &b)
}

/// The `diff` command.
fn diff_cmd(first: &str, second: &str, out: Out<'_>, err: Out<'_>) -> i32 {
    let _ = writeln!(out, "Use `cross-seed tree` on each .torrent file to display their full structure");
    match (read_torrent(first), read_torrent(second)) {
        (Ok(a), Ok(b)) => {
            let _ = writeln!(out, "{}", diff_files(&a, &b));
            0
        }
        (Err(e), _) | (_, Err(e)) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn options_as_commander_reads_them() {
        let opts: Vec<Opt> = SHARED.iter().chain(DAEMON.iter()).copied().collect();
        let p = parse(
            &opts,
            &args(&["--data-dirs", "/a", "/b", "-A", "save", "--no-use-client-torrents", "-d30", "--port=1234", "-vx", "1 day", "extra"]),
        )
        .unwrap();
        assert_eq!(p.list("data-dirs"), Some(&["/a".to_string(), "/b".to_string()][..]));
        assert_eq!(p.str("action"), Some("save"));
        assert_eq!(p.bool("use-client-torrents"), Some(false));
        assert_eq!(p.str("delay"), Some("30"));
        assert_eq!(p.str("port"), Some("1234"));
        assert_eq!(p.bool("verbose"), Some(true));
        assert_eq!(p.str("exclude-older"), Some("1 day"));
        assert_eq!(p.args, ["extra"]);
        assert_eq!(parse(&opts, &args(&["--no-port"])).unwrap().bool("port"), Some(false));
        assert_eq!(parse(&opts, &args(&["--foo"])).unwrap_err(), "error: unknown option '--foo'");
        assert_eq!(parse(&opts, &args(&["-A"])).unwrap_err(), "error: option '-A, --action <action>' argument missing");
        assert_eq!(
            parse(&opts, &args(&["--match-mode", "safe"])).unwrap_err(),
            "error: option '--match-mode <mode>' argument 'safe' is invalid. Allowed choices are strict, flexible, partial."
        );
        let again = parse(&opts, &args(&["--sonarr", "a", "--radarr", "b", "--sonarr", "c"])).unwrap();
        assert_eq!(again.list("sonarr"), Some(&["a".to_string(), "c".to_string()][..]));
    }

    #[test]
    fn javascript_numbers() {
        assert_eq!(js_parse_int("30s"), 30.0);
        assert_eq!(js_parse_int(" -7"), -7.0);
        assert_eq!(js_parse_int("0x10"), 16.0);
        assert!(js_parse_int("abc").is_nan());
        assert_eq!(js_parse_float("0.5x"), 0.5);
        assert_eq!(js_parse_float(".5"), 0.5);
        assert_eq!(js_parse_float("1e3"), 1000.0);
        assert!(js_parse_float("x").is_nan());
        assert_eq!(js_replace("http://old.example/ann", "old", "new"), "http://new.example/ann");
        assert_eq!(js_replace("aXa", "a", "[$&$$]"), "[a$]Xa");
    }

    #[test]
    fn options_laid_over_the_settings_in_cross_seeds_words() {
        let opts: Vec<Opt> = SHARED.iter().chain(DAEMON.iter()).copied().collect();
        let mut c = RuntimeConfig::default();
        let mut problems = Vec::new();
        let p = parse(
            &opts,
            &args(&["--delay", "abc", "-x", "two weeks", "--no-season-from-episodes", "--search-limit", "0", "--api-key", "short"]),
        )
        .unwrap();
        apply(&p, false, &mut c, &mut problems);
        assert_eq!(c.season_from_episodes, None);
        assert_eq!(c.search_limit, None);
        assert_eq!(problems, [NAN.to_string(), issue("two weeks", VERCEL), "String must contain at least 24 character(s)".to_string()]);
        let mut c = RuntimeConfig { link_dirs: vec!["/l".into()], ..RuntimeConfig::default() };
        let mut problems = Vec::new();
        let p = parse(&opts, &args(&["--link-dir", "/x", "--qbittorrent-url", "http://localhost:8080"])).unwrap();
        apply(&p, false, &mut c, &mut problems);
        assert_eq!(c.torrent_clients, ["qbittorrent:http://localhost:8080"]);
        assert_eq!(problems, ["You cannot have both linkDir and linkDirs, use linkDirs only."]);
        let mut problems = Vec::new();
        apply(&p, true, &mut RuntimeConfig::default(), &mut problems);
        assert!(problems[0].starts_with("Cannot use qbittorrentUrl"));
    }

    #[test]
    fn inspect_as_node_prints() {
        let short = Js::Obj(vec![("a".into(), Js::Num(1.0)), ("b-c".into(), Js::Str("it's".into()))]);
        assert_eq!(inspect(&short), "{ a: 1, 'b-c': \"it's\" }");
        let long = Js::Obj(vec![
            ("infoHash".into(), Js::Str("0123456789abcdef0123456789abcdef01234567".into())),
            ("files".into(), Js::Arr(vec![Js::Obj(vec![("name".into(), Js::Str("a.mkv".into())), ("length".into(), Js::Num(10.0))])])),
        ]);
        assert_eq!(
            inspect(&long),
            "{\n  infoHash: '0123456789abcdef0123456789abcdef01234567',\n  files: [ { name: 'a.mkv', length: 10 } ]\n}"
        );
        let nums = Js::Arr((1..=10).map(|n| Js::Num(n as f64)).collect());
        assert_eq!(inspect(&nums), "[\n  1, 2, 3, 4,  5,\n  6, 7, 8, 9, 10\n]");
        let many = Js::Arr((0..101).map(|_| Js::Num(0.0)).collect());
        assert!(inspect(&many).ends_with("... 1 more item\n]"));
    }
}
