// Ported from autobrr v1.90.0 (internal/irc/handler.go, service.go, channel.go, channel_state_machine.go,
// state_machine.go, casemapping.go; internal/domain/irc.go; internal/database/irc.go), GPL-2.0-or-later.
//! autobrr's IRC networks: one connection per enabled network, signed in with SASL PLAIN or NickServ (or
//! nothing), optionally through a bouncer, over TLS or not. Each announce channel of the indexers on the network's
//! server is joined, after the tracker's invite command when it has one, and what the announcers say there goes
//! to the indexer's announce reader ([`super::announce`]). Every channel keeps its last 1000 lines; lines and
//! state changes are also sent to the `/irc/events` stream.
//!
//! The connection follows autobrr's two state machines: the network's (connecting, authenticating, joining,
//! operational, error) and each channel's (awaiting invite, joining, monitoring, kicked, error …), with autobrr's
//! timeouts and retry delays. A failure that can't fix itself (a ban, bad NickServ or SASL credentials, a TLS
//! certificate that doesn't verify, a connection that keeps dropping) stops the network with the reason shown.
//!
//! Nothing sent to the server is logged (it carries the passwords and invite keys).

use super::announce::{Processor, Sink};
use super::definition::{is_redacted, redact, IndexerDefinition, AUTH_NICKSERV, AUTH_NONE, AUTH_SASL_PLAIN};
use super::store::{ApiError, ApiResult};
use super::Releases;
use base64::Engine;
use chrono::{DateTime, Utc};
use kit_server_core::{p, Db, Row, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

// ---- the records (autobrr's domain/irc.go) ----

/// How a network signs in (autobrr's `IRCAuth`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IrcAuth {
    /// `NONE`, `SASL_PLAIN`, `NICKSERV`, or empty (a network from before mechanisms: NickServ when there is a
    /// password).
    pub mechanism: String,
    /// The account.
    pub account: String,
    /// The password.
    pub password: String,
}

impl IrcAuth {
    /// An unknown mechanism, in autobrr's words (incomplete credentials are accepted).
    pub fn validate(&self) -> Result<(), String> {
        if self.mechanism.is_empty() || super::definition::auth_mechanism_is_valid(&self.mechanism) {
            return Ok(());
        }
        Err(format!("invalid IRC authentication mechanism: {:?}", self.mechanism))
    }

    /// Whether NickServ IDENTIFY is used (autobrr's `NickServEnabled`).
    pub fn nickserv_enabled(&self) -> bool {
        match self.mechanism.as_str() {
            AUTH_NICKSERV => !self.password.is_empty(),
            AUTH_SASL_PLAIN => !self.account.is_empty() && !self.password.is_empty(),
            "" => !self.password.is_empty(),
            _ => false,
        }
    }

    /// As the API sends it: empty fields left out, the password redacted.
    fn public(&self) -> Value {
        let mut m = serde_json::Map::new();
        for (k, v) in [("mechanism", self.mechanism.clone()), ("account", self.account.clone()), ("password", redact(&self.password))] {
            if !v.is_empty() {
                m.insert(k.into(), json!(v));
            }
        }
        Value::Object(m)
    }
}

/// One channel of a network (autobrr's `IrcChannel`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IrcChannel {
    /// Its id.
    pub id: i64,
    /// Joined at all.
    pub enabled: bool,
    /// `#announce`.
    pub name: String,
    /// Its key (`+k`).
    pub password: String,
    /// Kept for autobrr's shape.
    pub detached: bool,
    /// Kept for autobrr's shape.
    pub monitoring: bool,
}

impl IrcChannel {
    /// As the API sends it (the key redacted).
    fn public(&self) -> Value {
        json!({ "id": self.id, "enabled": self.enabled, "name": self.name, "password": redact(&self.password), "detached": self.detached, "monitoring": self.monitoring })
    }
}

/// One IRC network (autobrr's `IrcNetwork`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IrcNetwork {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Connected at all.
    pub enabled: bool,
    /// The server (also how the network finds its indexers).
    pub server: String,
    /// The port.
    pub port: i64,
    /// TLS.
    pub tls: bool,
    /// TLS without checking the certificate.
    pub tls_skip_verify: bool,
    /// The server (or bouncer) password.
    pub pass: String,
    /// The nick.
    pub nick: String,
    /// The sign-in.
    pub auth: IrcAuth,
    /// The invite commands, `Bot invite KEY` separated by commas.
    pub invite_command: String,
    /// Connect to `bouncer_addr` instead of the server.
    pub use_bouncer: bool,
    /// The bouncer, `host:port`.
    pub bouncer_addr: String,
    /// Through a proxy.
    pub use_proxy: bool,
    /// The proxy's id.
    pub proxy_id: i64,
    /// The proxy (autobrr's proxies aren't ported: always `null`).
    pub proxy: Option<Value>,
    /// Ask the server for IRCv3 bot mode.
    pub bot_mode: bool,
    /// The channels (`None`: not sent).
    pub channels: Option<Vec<IrcChannel>>,
    /// Kept for autobrr's shape.
    pub connected: bool,
    /// Kept for autobrr's shape.
    pub connected_since: Option<String>,
    /// Kept for autobrr's shape.
    pub connection_errors: Option<Vec<String>>,
}

impl IrcNetwork {
    /// As autobrr's API sends it (secrets redacted).
    pub fn public(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "enabled": self.enabled, "server": self.server, "port": self.port,
            "tls": self.tls, "tls_skip_verify": self.tls_skip_verify, "pass": redact(&self.pass), "nick": self.nick,
            "auth": self.auth.public(), "invite_command": self.invite_command, "use_bouncer": self.use_bouncer,
            "bouncer_addr": self.bouncer_addr, "use_proxy": self.use_proxy, "proxy_id": self.proxy_id, "proxy": self.proxy,
            "bot_mode": self.bot_mode,
            "channels": self.channels.as_ref().map(|c| c.iter().map(IrcChannel::public).collect::<Vec<_>>()),
            "connected": self.connected, "connected_since": self.connected_since, "connection_errors": self.connection_errors,
        })
    }

    /// The fields whose change needs a reconnect (autobrr's `DetermineIfRestartIsRequired`).
    pub fn restart_fields(&self, d: &IrcNetwork) -> Vec<&'static str> {
        let mut f = Vec::new();
        let mut check = |changed: bool, name: &'static str| {
            if changed {
                f.push(name);
            }
        };
        check(self.server != d.server, "server");
        check(self.port != d.port, "port");
        check(self.tls != d.tls, "tls");
        check(self.tls_skip_verify != d.tls_skip_verify, "tls skip verify");
        check(self.pass != d.pass, "pass");
        check(self.invite_command != d.invite_command, "invite command");
        check(self.use_bouncer != d.use_bouncer, "use bouncer");
        check(self.bouncer_addr != d.bouncer_addr, "bouncer addr");
        check(self.bot_mode != d.bot_mode, "bot mode");
        check(self.use_proxy != d.use_proxy, "use proxy");
        check(self.proxy_id != d.proxy_id && (self.use_proxy || d.use_proxy), "proxy id");
        check(self.proxy != d.proxy, "proxy");
        check(self.auth.mechanism != d.auth.mechanism, "auth mechanism");
        check(self.auth.account != d.auth.account, "auth account");
        check(self.auth.password != d.auth.password, "auth password");
        f
    }
}

/// One line of a channel's history (autobrr's `IrcMessage`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IrcMessage {
    /// The network.
    pub network: i64,
    /// The channel.
    pub channel: String,
    /// Who said it.
    pub nick: String,
    /// What was said.
    #[serde(rename = "msg")]
    pub message: String,
    /// `KICK` for a kick notice, else empty.
    #[serde(rename = "type")]
    pub kind: String,
    /// When.
    #[serde(serialize_with = "go_time")]
    pub time: DateTime<Utc>,
}

/// A time as Go's JSON writes it.
fn go_time<S: serde::Serializer>(t: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&time_text(Some(*t)))
}

/// A time as Go's JSON writes it; Go's zero time for none.
fn time_text(t: Option<DateTime<Utc>>) -> String {
    match t {
        Some(t) => t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        None => "0001-01-01T00:00:00Z".into(),
    }
}

/// A line to send by hand (autobrr's `SendIrcCmdRequest`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SendCmd {
    /// The network.
    pub network_id: i64,
    /// Unused (autobrr's shape).
    pub server: String,
    /// The target: a channel or a nick.
    pub channel: String,
    /// Unused (autobrr's shape).
    pub nick: String,
    /// What to say.
    pub msg: String,
}

/// One event of the `/irc/events` stream: its name (`PRIVMSG`, `STATE`, `HEALTH`) and JSON.
#[derive(Debug, Clone)]
pub struct Event {
    /// The event name.
    pub event: String,
    /// The JSON.
    pub data: String,
}

// ---- the stored networks (autobrr's database/irc.go) ----

/// The IRC tables.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_irc_networks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            enabled INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL DEFAULT '',
            server TEXT NOT NULL DEFAULT '',
            port INTEGER NOT NULL DEFAULT 0,
            tls INTEGER NOT NULL DEFAULT 0,
            tls_skip_verify INTEGER NOT NULL DEFAULT 0,
            pass TEXT NOT NULL DEFAULT '',
            nick TEXT NOT NULL DEFAULT '',
            auth_mechanism TEXT NOT NULL DEFAULT '',
            auth_account TEXT NOT NULL DEFAULT '',
            auth_password TEXT NOT NULL DEFAULT '',
            invite_command TEXT NOT NULL DEFAULT '',
            bouncer_addr TEXT NOT NULL DEFAULT '',
            use_bouncer INTEGER NOT NULL DEFAULT 0,
            bot_mode INTEGER NOT NULL DEFAULT 0,
            use_proxy INTEGER NOT NULL DEFAULT 0,
            proxy_id INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS autobrr_irc_channels (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            network_id INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            detached INTEGER NOT NULL DEFAULT 0,
            name TEXT NOT NULL DEFAULT '',
            password TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS autobrr_irc_channels_by_network ON autobrr_irc_channels(network_id);",
    )
    .map_err(|e| anyhow::anyhow!("The IRC tables can't be made: {}", e.detail))
}

/// A network from its row (without channels).
fn network_of(r: &Row) -> IrcNetwork {
    IrcNetwork {
        id: r.i("id"),
        name: r.s("name"),
        enabled: r.b("enabled"),
        server: r.s("server"),
        port: r.i("port"),
        tls: r.b("tls"),
        tls_skip_verify: r.b("tls_skip_verify"),
        pass: r.s("pass"),
        nick: r.s("nick"),
        auth: IrcAuth { mechanism: r.s("auth_mechanism"), account: r.s("auth_account"), password: r.s("auth_password") },
        invite_command: r.s("invite_command"),
        use_bouncer: r.b("use_bouncer"),
        bouncer_addr: r.s("bouncer_addr"),
        bot_mode: r.b("bot_mode"),
        use_proxy: r.b("use_proxy"),
        proxy_id: r.i("proxy_id"),
        ..IrcNetwork::default()
    }
}

/// Why IRC calls fail besides a missing record (autobrr's `ErrIRCChannelNotFound`, …).
#[derive(Debug, Clone, PartialEq)]
pub enum IrcError {
    /// The network or its handler doesn't exist.
    NotFound,
    /// The channel isn't one of the network's.
    ChannelNotFound,
    /// The channel isn't an announce channel.
    NoAnnounceProcessor,
    /// Anything else, in autobrr's words.
    Other(String),
}

// ---- timing ----

/// autobrr's delays and timeouts (tests make them short).
#[derive(Debug, Clone)]
pub struct Timing {
    /// The wait after registering before signing in and joining (autobrr's 1 s).
    pub after_connect: Duration,
    /// The first wait before connecting again after a failed attempt (15 s, doubling up to 5 min, plus up to 10 s).
    pub connect_retry: Duration,
    /// The longest such wait.
    pub connect_retry_max: Duration,
    /// The random part of it.
    pub connect_jitter: Duration,
    /// The wait before reconnecting after a dropped connection (irc-go's 15 s).
    pub reconnect: Duration,
    /// How long to wait for an invite (30 s).
    pub invite_timeout: Duration,
    /// How long an invite bot's answer is given to turn into a join (5 s).
    pub invite_grace: Duration,
    /// How long to wait for a join to be confirmed (45 s).
    pub join_timeout: Duration,
    /// The first wait before a channel in error tries again (15 min, doubling up to 6 h, 8 times).
    pub error_retry: Duration,
    /// How long registration may take (irc-go's 2 min).
    pub register_timeout: Duration,
    /// Quiet time before Lantern pings the server (irc-go's 4 min keep-alive).
    pub keepalive: Duration,
    /// How often the channel timers are looked at.
    pub tick: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            after_connect: Duration::from_secs(1),
            connect_retry: Duration::from_secs(15),
            connect_retry_max: Duration::from_secs(300),
            connect_jitter: Duration::from_secs(10),
            reconnect: Duration::from_secs(15),
            invite_timeout: Duration::from_secs(30),
            invite_grace: Duration::from_secs(5),
            join_timeout: Duration::from_secs(45),
            error_retry: Duration::from_secs(15 * 60),
            register_timeout: Duration::from_secs(120),
            keepalive: Duration::from_secs(240),
            tick: Duration::from_millis(250),
        }
    }
}

/// How long an invite bot gets before the invite is asked again (autobrr's `retryBackoff`); `None` once the
/// tries are used up.
pub fn retry_backoff(attempt: u32) -> Option<Duration> {
    let a = attempt.max(1);
    match a {
        1..=8 => Some(Duration::from_secs(15)),
        9..=68 => Some(Duration::from_secs(30)),
        69..=128 => Some(Duration::from_secs(60)),
        129..=248 => Some(Duration::from_secs(3600)),
        _ => None,
    }
}

// ---- case mapping (autobrr's casemapping.go) ----

/// How the server compares nicks and channel names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMapping {
    /// `rfc1459`: `[]\^` are the upper case of `{}|~`.
    #[default]
    Rfc1459,
    /// `strict-rfc1459`: `[]\` only.
    StrictRfc1459,
    /// `ascii`: letters only.
    Ascii,
}

impl CaseMapping {
    /// The mapping a server's `CASEMAPPING=` names, and whether Lantern knows it.
    pub fn parse(v: &str) -> (CaseMapping, bool) {
        match v.to_lowercase().as_str() {
            "rfc1459" => (CaseMapping::Rfc1459, true),
            "rfc1459-strict" | "strict-rfc1459" => (CaseMapping::StrictRfc1459, true),
            "ascii" => (CaseMapping::Ascii, true),
            _ => (CaseMapping::Rfc1459, false),
        }
    }

    /// `s` in lower case under this mapping.
    pub fn fold(self, s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'A'..='Z' => c.to_ascii_lowercase(),
                '[' if self != CaseMapping::Ascii => '{',
                ']' if self != CaseMapping::Ascii => '}',
                '\\' if self != CaseMapping::Ascii => '|',
                '^' if self == CaseMapping::Rfc1459 => '~',
                // the IRC mappings only fold ASCII; anything else stays as it is
                c => c,
            })
            .collect()
    }

    /// Whether `a` and `b` are the same name.
    pub fn equal(self, a: &str, b: &str) -> bool {
        self.fold(a) == self.fold(b)
    }
}

/// The key a channel is kept under (autobrr's `channelMapKey`).
pub fn channel_key(c: &str) -> String {
    c.to_lowercase()
}

// ---- IRC lines ----

/// One IRC message: its sender's nick, command and parameters (the last one may have spaces).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    /// The sender's nick (empty for the server's own lines without a prefix).
    pub nick: String,
    /// `PRIVMSG`, `PING`, `001` …
    pub command: String,
    /// The parameters.
    pub params: Vec<String>,
}

/// Reads one IRC line (tags dropped).
pub fn parse_line(line: &str) -> Message {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    if rest.starts_with('@') {
        rest = rest.split_once(' ').map(|(_, r)| r).unwrap_or("");
    }
    let mut nick = String::new();
    if let Some(r) = rest.strip_prefix(':') {
        let (prefix, r) = r.split_once(' ').unwrap_or((r, ""));
        nick = prefix.split('!').next().unwrap_or("").to_string();
        rest = r;
    }
    let (head, trailing) = match rest.split_once(" :") {
        Some((h, t)) => (h, Some(t)),
        None if rest.starts_with(':') => ("", Some(&rest[1..])),
        None => (rest, None),
    };
    let mut words = head.split_whitespace();
    let command = words.next().unwrap_or("").to_uppercase();
    let mut params: Vec<String> = words.map(str::to_string).collect();
    if let Some(t) = trailing {
        params.push(t.to_string());
    }
    Message { nick, command, params }
}

/// A line to send: the command, its middle parameters and the last one (always written as trailing).
fn line_of(command: &str, params: &[&str]) -> String {
    let mut s = command.to_string();
    for (i, p) in params.iter().enumerate() {
        s.push(' ');
        if i + 1 == params.len() && (p.contains(' ') || p.starts_with(':') || p.is_empty()) {
            s.push(':');
        }
        s.push_str(p);
    }
    s.push_str("\r\n");
    s
}

/// `line` without mIRC colours, bold, italics, underline, reverse, monospace and reset codes (ircfmt's `Strip`).
pub fn clean_message(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let digits = |chars: &mut std::iter::Peekable<std::str::Chars>, n: usize, hex: bool| {
        for _ in 0..n {
            if chars.peek().is_some_and(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }) {
                chars.next();
            }
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\x03' | '\x04' => {
                let (n, hex) = if c == '\x03' { (2, false) } else { (6, true) };
                let had = chars.peek().is_some_and(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() });
                digits(&mut chars, n, hex);
                if had && chars.peek() == Some(&',') {
                    let mut look = chars.clone();
                    look.next();
                    if look.peek().is_some_and(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }) {
                        chars.next();
                        digits(&mut chars, n, hex);
                    }
                }
            }
            '\x02' | '\x0f' | '\x16' | '\x1d' | '\x1f' | '\x1e' | '\x11' => {}
            c => out.push(c),
        }
    }
    out
}

/// The invite commands of `raw` (`/msg` dropped, split at commas; autobrr's `parseInviteCommands`).
pub fn parse_invite_commands(raw: &str) -> Vec<String> {
    raw.replace("/msg", "").split(',').map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// The nick an invite command goes to: its first word.
pub fn invite_bot_nick(cmd: &str) -> String {
    cmd.split_whitespace().next().unwrap_or("").to_string()
}

/// The invite bot a definition's default invite command names.
fn definition_invite_bot(def: &IndexerDefinition) -> String {
    def.irc
        .as_ref()
        .and_then(|i| i.settings.iter().find(|s| s.name == "invite_command"))
        .map(|s| invite_bot_nick(&s.default))
        .unwrap_or_default()
}

/// The invite command of `commands` sent to `bot` (autobrr's `matchInviteCommand`).
pub fn match_invite_command(cm: CaseMapping, bot: &str, commands: &[String]) -> String {
    if bot.is_empty() {
        return String::new();
    }
    commands
        .iter()
        .find(|c| {
            let f: Vec<&str> = c.split_whitespace().collect();
            f.len() >= 2 && cm.equal(bot, f[0])
        })
        .cloned()
        .unwrap_or_default()
}

/// Whether `target` names a channel.
pub fn is_channel_target(target: &str) -> bool {
    matches!(target.as_bytes().first(), Some(b'#' | b'&' | b'+' | b'!'))
}

/// Whether mode string `modes` adds `flag`.
pub fn mode_adds(modes: &str, flag: u8) -> bool {
    let mut adding = false;
    let mut added = false;
    for c in modes.bytes() {
        match c {
            b'+' => adding = true,
            b'-' => adding = false,
            c if c == flag => added = adding,
            _ => {}
        }
    }
    added
}

/// Whether `s` contains any of `subs`, in any case (autobrr's `contains`).
fn contains_any(s: &str, subs: &[&str]) -> bool {
    let s = s.to_lowercase();
    subs.iter().any(|c| s.contains(&c.to_lowercase()))
}

/// Whether a NickServ answer means the account-qualified IDENTIFY should be tried.
pub fn notice_allows_identify_escalation(notice: &str) -> bool {
    contains_any(
        notice,
        &[
            "isn't registered",
            "is not a registered nickname",
            "account does not exist",
            "insufficient parameters",
            "invalid parameters",
            "help identify",
            "syntax: identify",
        ],
    )
}

// ---- the state machines ----

/// The network's state (autobrr's `ConnectionState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Not connected.
    Disconnected,
    /// Connecting.
    Connecting,
    /// Registered.
    Connected,
    /// Signing in with NickServ.
    Authenticating,
    /// Signed in.
    Authenticated,
    /// Joining the channels.
    JoiningChannels,
    /// Every channel monitored.
    FullyOperational,
    /// Some channels monitored.
    PartiallyOperational,
    /// Failed.
    Error,
}

impl ConnState {
    /// The moves autobrr allows.
    fn can_go(self, to: ConnState) -> bool {
        use ConnState::*;
        let allowed: &[ConnState] = match self {
            Disconnected => &[Connecting, Connected, Error],
            Connecting => &[Connected, Error, Disconnected],
            Connected => &[Authenticating, Authenticated, PartiallyOperational, Error, Disconnected],
            Authenticating => &[Authenticated, PartiallyOperational, Error, Disconnected],
            Authenticated => &[JoiningChannels, FullyOperational, PartiallyOperational, Error, Disconnected],
            JoiningChannels => &[FullyOperational, PartiallyOperational, Error, Disconnected],
            FullyOperational => &[PartiallyOperational, Error, Disconnected],
            PartiallyOperational => &[FullyOperational, Error, Disconnected],
            Error => &[Disconnected, Connecting, FullyOperational, PartiallyOperational],
        };
        allowed.contains(&to)
    }
}

/// A channel's state (autobrr's `ChannelState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChanState {
    /// Nothing yet.
    Idle,
    /// The invite command was sent.
    AwaitingInvite,
    /// Waiting to send the invite command again.
    AwaitingInviteBot,
    /// The invite bot said no.
    InviteFailed,
    /// The invite bot isn't there.
    InviteFailedNoSuchNick,
    /// JOIN sent.
    Joining,
    /// In the channel.
    Monitoring,
    /// Kicked (not rejoined by itself).
    Kicked,
    /// Left.
    Parted,
    /// Turned off.
    Disabled,
    /// Failed; tried again later.
    Error,
}

impl ChanState {
    /// autobrr's name for it.
    pub fn name(self) -> &'static str {
        match self {
            ChanState::Idle => "Idle",
            ChanState::AwaitingInvite => "AwaitingInvite",
            ChanState::AwaitingInviteBot => "AwaitingInviteBot",
            ChanState::InviteFailed => "InviteFailed",
            ChanState::InviteFailedNoSuchNick => "InviteFailedNoSuchNick",
            ChanState::Joining => "Joining",
            ChanState::Monitoring => "Monitoring",
            ChanState::Kicked => "Kicked",
            ChanState::Parted => "Parted",
            ChanState::Disabled => "Disabled",
            ChanState::Error => "Error",
        }
    }

    /// The moves autobrr allows.
    fn can_go(self, to: ChanState) -> bool {
        use ChanState::*;
        let allowed: &[ChanState] = match self {
            Idle => &[Joining, AwaitingInvite, Monitoring, Error, Kicked, Parted],
            Joining => &[Monitoring, AwaitingInvite, Error, Kicked, Parted],
            AwaitingInvite => &[Monitoring, AwaitingInviteBot, InviteFailedNoSuchNick, Joining, Error, Kicked, Parted],
            InviteFailed => &[Joining, Monitoring],
            AwaitingInviteBot => &[AwaitingInvite, InviteFailedNoSuchNick, Monitoring, Joining, Error, Kicked],
            InviteFailedNoSuchNick => &[AwaitingInviteBot, Joining, Error],
            Kicked => &[Idle, Joining, AwaitingInvite, Monitoring],
            Parted => &[Idle, Joining],
            Disabled => &[Idle],
            Error => &[Idle, Joining, AwaitingInvite, Monitoring],
            Monitoring => &[],
        };
        allowed.contains(&to)
    }
}

/// What a channel waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timer {
    /// No invite came.
    Invite,
    /// The bot answered but no join came.
    InviteGrace,
    /// Time to send the invite again.
    InviteRetry,
    /// The join wasn't confirmed.
    Join,
    /// Time to try again after an error (with the error count it was set for).
    ErrorRetry(u32),
}

/// One channel's runtime state (autobrr's `Channel` and its `ChannelStateMachine`).
struct Chan {
    id: i64,
    name: String,
    enabled: bool,
    password: String,
    default_channel: bool,
    skip_clean: bool,
    monitoring: bool,
    monitoring_since: Option<DateTime<Utc>>,
    last_announce: Option<DateTime<Utc>>,
    errors: Vec<String>,
    invite_command: String,
    invite_bot: String,
    announcers: Vec<String>,
    messages: VecDeque<IrcMessage>,
    processor: Option<Arc<Processor>>,
    state: ChanState,
    timer: Option<(Instant, Timer)>,
    auth_attempts: u32,
    error_attempts: u32,
    join_after_invite: bool,
    invite_responded: bool,
    invite_reject_reason: String,
}

/// How many lines a channel keeps.
pub const MAX_CHANNEL_MESSAGES: usize = 1000;

impl Chan {
    /// A channel named `name`, idle; `processor` reads its announces (none for a channel that isn't an announce channel).
    fn new(name: &str, default_channel: bool, skip_clean: bool, processor: Option<Arc<Processor>>) -> Chan {
        Chan {
            id: 0,
            name: name.to_string(),
            enabled: true,
            password: String::new(),
            default_channel,
            skip_clean,
            monitoring: false,
            monitoring_since: None,
            last_announce: None,
            errors: Vec::new(),
            invite_command: String::new(),
            invite_bot: String::new(),
            announcers: Vec::new(),
            messages: VecDeque::new(),
            processor,
            state: ChanState::Idle,
            timer: None,
            auth_attempts: 0,
            error_attempts: 0,
            join_after_invite: false,
            invite_responded: false,
            invite_reject_reason: String::new(),
        }
    }

    /// Keeps `m` in the history, dropping the oldest line past [`MAX_CHANNEL_MESSAGES`].
    fn add_message(&mut self, m: IrcMessage) {
        if self.messages.len() >= MAX_CHANNEL_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(m);
    }

    /// Notes problem `e` once.
    fn set_error(&mut self, e: &str) {
        if !self.errors.iter().any(|x| x == e) {
            self.errors.push(e.to_string());
        }
    }

    /// No longer watched: the watch and its problems cleared.
    fn reset_monitoring(&mut self) {
        self.monitoring = false;
        self.monitoring_since = None;
        self.errors.clear();
    }

    /// The state machine back to Idle, every timer void (autobrr's `Reset`).
    fn reset_sm(&mut self) {
        self.state = ChanState::Idle;
        self.auth_attempts = 0;
        self.error_attempts = 0;
        self.timer = None;
        self.join_after_invite = false;
        self.invite_responded = false;
        self.invite_reject_reason.clear();
    }

    /// Whether `nick` may announce here (autobrr's `IsValidAnnouncer`).
    fn is_valid_announcer(&self, nick: &str, cm: CaseMapping) -> bool {
        let nick = cm.fold(nick);
        self.announcers.iter().any(|a| cm.fold(a) == nick)
    }
}

/// The connection's sign-in and identity, reset per connection.
#[derive(Default)]
struct Session {
    registered: bool,
    authenticated: bool,
    saslauthed: bool,
    identify_outstanding: bool,
    identify_account_form: bool,
    identify_escalated: bool,
    identify_retries: u32,
    identify_retry_at: Option<Instant>,
    failed_nickserv: u32,
    connected_at: Option<Instant>,
    on_connected_at: Option<Instant>,
}

/// A network's runtime state.
struct State {
    network: IrcNetwork,
    definitions: BTreeMap<String, Arc<IndexerDefinition>>,
    conn: ConnState,
    /// A connection task runs (autobrr's client is set).
    live: bool,
    /// Stopped: by hand, or by a failure that can't fix itself.
    stopped: bool,
    connected_since: Option<DateTime<Utc>>,
    current_nick: String,
    case_mapping: CaseMapping,
    bot_mode_char: String,
    connection_errors: Vec<String>,
    channels: BTreeMap<String, Chan>,
    session: Session,
    identify_form_learned_account: bool,
    short_sessions: Vec<Instant>,
    /// The last registered connection dropped unasked, so the next one is a reconnect (autobrr's
    /// `haveDisconnected`).
    have_disconnected: bool,
}

/// One network's connection (autobrr's `Handler`).
pub struct Handler {
    releases: Releases,
    events: broadcast::Sender<Event>,
    timing: Timing,
    state: Mutex<State>,
    out: Mutex<Option<mpsc::UnboundedSender<String>>>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
    gen: AtomicU64,
}

/// What a line asks to be done after the state lock is let go.
type Queued = Vec<(Arc<Processor>, String, String)>;

impl Handler {
    /// A handler for `network` with the indexers of its server.
    fn new(
        releases: Releases,
        events: broadcast::Sender<Event>,
        timing: Timing,
        network: IrcNetwork,
        defs: Vec<Arc<IndexerDefinition>>,
    ) -> Arc<Handler> {
        let h = Arc::new(Handler {
            releases,
            events,
            timing,
            state: Mutex::new(State {
                network,
                definitions: BTreeMap::new(),
                conn: ConnState::Disconnected,
                live: false,
                stopped: true,
                connected_since: None,
                current_nick: String::new(),
                case_mapping: CaseMapping::Rfc1459,
                bot_mode_char: String::new(),
                connection_errors: Vec::new(),
                channels: BTreeMap::new(),
                session: Session::default(),
                identify_form_learned_account: false,
                short_sessions: Vec::new(),
                have_disconnected: false,
            }),
            out: Mutex::new(None),
            task: Mutex::new(None),
            gen: AtomicU64::new(0),
        });
        h.init_indexers(defs);
        h
    }

    /// The runtime state, even after a panic elsewhere.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Where finished releases go.
    fn sink(&self) -> Sink {
        let r = self.releases.clone();
        Arc::new(move |rel| {
            let r = r.clone();
            tokio::spawn(async move {
                r.process_release(rel).await;
            });
        })
    }

    /// Adds the definitions not yet known and their channels configured on this network (autobrr's
    /// `InitIndexers`).
    fn init_indexers(&self, defs: Vec<Arc<IndexerDefinition>>) {
        let sink = self.sink();
        let mut st = self.lock();
        let added: Vec<Arc<IndexerDefinition>> = defs.into_iter().filter(|d| !st.definitions.contains_key(&d.identifier)).collect();
        for d in &added {
            st.definitions.insert(d.identifier.clone(), d.clone());
        }
        init_channels(&mut st, &added, &sink);
    }

    /// Rebuilds the channels from the network and `defs` (autobrr's `reloadChannels`; the handler is stopped).
    fn reload_channels(&self, defs: Vec<Arc<IndexerDefinition>>) {
        let sink = self.sink();
        let mut st = self.lock();
        st.channels.clear();
        for d in defs {
            st.definitions.insert(d.identifier.clone(), d);
        }
        let all: Vec<Arc<IndexerDefinition>> = st.definitions.values().cloned().collect();
        init_channels(&mut st, &all, &sink);
    }

    /// The network as the handler has it.
    pub fn network(&self) -> IrcNetwork {
        self.lock().network.clone()
    }

    /// Whether it is stopped.
    pub fn stopped(&self) -> bool {
        self.lock().stopped
    }

    /// Queues a line to the server; the problem when there is no connection.
    fn send(&self, command: &str, params: &[&str]) -> Result<(), String> {
        let out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        match out.as_ref() {
            Some(tx) => tx.send(line_of(command, params)).map_err(|_| "Message cannot be sent because client is disconnected".to_string()),
            None => Err("Message cannot be sent because client is disconnected".into()),
        }
    }

    /// Sends an event to the stream.
    fn broadcast(&self, event: &str, data: Value) {
        let _ = self.events.send(Event { event: event.into(), data: data.to_string() });
    }

    /// Starts connecting in the background (autobrr's `claimConnect` + `connect`).
    fn start(self: &Arc<Self>) {
        {
            let mut st = self.lock();
            if !st.stopped {
                return;
            }
            st.stopped = false;
            st.live = true;
            st.current_nick.clear();
        }
        let g = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.conn_go(ConnState::Connecting);
        let me = self.clone();
        let task = tokio::spawn(async move { me.run(g).await });
        if let Some(old) = self.task.lock().unwrap_or_else(|e| e.into_inner()).replace(task.abort_handle()) {
            old.abort();
        }
    }

    /// Stops the connection (autobrr's `stop`): QUIT, every channel reset.
    pub fn stop(&self) {
        self.gen.fetch_add(1, Ordering::SeqCst);
        let had_client = {
            let mut st = self.lock();
            st.connected_since = None;
            st.short_sessions.clear();
            st.have_disconnected = false;
            reset_session(&mut st);
            st.current_nick.clear();
            let had = st.live;
            st.live = false;
            st.stopped = true;
            had
        };
        if self.lock().conn != ConnState::Disconnected {
            self.conn_go(ConnState::Disconnected);
        }
        if had_client {
            self.reset_channel_state();
            let _ = self.send("QUIT", &["bye from autobrr"]);
        }
        // the writer gets the QUIT out before the task ends
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        let out = self.out.lock().unwrap_or_else(|e| e.into_inner()).take();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(out);
            if let Some(t) = task {
                t.abort();
            }
        });
    }

    /// Whether connection `g` is still the current one.
    fn current(&self, g: u64) -> bool {
        self.gen.load(Ordering::SeqCst) == g && !self.lock().stopped
    }

    /// Connects and reconnects until stopped.
    async fn run(self: Arc<Self>, g: u64) {
        let mut attempt: u32 = 0;
        loop {
            if !self.current(g) {
                return;
            }
            let (registered, err) = self.session(g).await;
            if !self.current(g) {
                return;
            }
            if registered {
                attempt = 0;
                if !self.on_disconnect() {
                    return;
                }
                tokio::time::sleep(self.timing.reconnect).await;
                continue;
            }
            if let Some(e) = err {
                tracing::warn!("IRC {}: {e}", self.lock().network.name);
            }
            attempt += 1;
            let base = self.timing.connect_retry.saturating_mul(1u32 << (attempt - 1).min(16)).min(self.timing.connect_retry_max);
            let jitter = if self.timing.connect_jitter.is_zero() {
                Duration::ZERO
            } else {
                Duration::from_millis(rand::random::<u64>() % self.timing.connect_jitter.as_millis().max(1) as u64)
            };
            tokio::time::sleep(base + jitter).await;
        }
    }

    /// A dropped registered connection (autobrr's `onClientDisconnect`); false when the flapping breaker stopped
    /// the network.
    fn on_disconnect(&self) -> bool {
        let lifetime = {
            let mut st = self.lock();
            let lifetime = st.session.connected_at.map(|t| t.elapsed()).unwrap_or_default();
            st.connected_since = None;
            st.current_nick.clear();
            reset_session(&mut st);
            lifetime
        };
        let flapping = {
            let mut st = self.lock();
            let now = Instant::now();
            if lifetime >= Duration::from_secs(30) {
                st.short_sessions.clear();
                false
            } else {
                st.short_sessions.retain(|t| now.duration_since(*t) < Duration::from_secs(15 * 60));
                st.short_sessions.push(now);
                if st.short_sessions.len() >= 5 {
                    st.short_sessions.clear();
                    true
                } else {
                    false
                }
            }
        };
        if flapping {
            let e = "connection flapping: 5 sessions lasted under 30s within 15m0s; network stopped to avoid repeated reconnects";
            self.add_connect_error(e);
            self.reset_channel_state();
            self.conn_error();
            self.gen.fetch_add(1, Ordering::SeqCst);
            let name = {
                let mut st = self.lock();
                st.live = false;
                st.stopped = true;
                st.have_disconnected = false;
                st.network.name.clone()
            };
            let msg = format!(
                "Network: {name} stopped after repeated short-lived connections; restart it after resolving the connection issue"
            );
            self.releases.notify(super::notify::irc_payload(super::notify::IrcEvent::Flapping, &name, &msg));
            return false;
        }
        let name = {
            let mut st = self.lock();
            st.have_disconnected = true;
            st.network.name.clone()
        };
        self.reset_channel_state();
        // only a drop Lantern didn't ask for gets here (a stop ends the run first)
        self.releases.notify(super::notify::irc_payload(super::notify::IrcEvent::Disconnected, &name, &format!("Network: {name}")));
        self.conn_go(ConnState::Disconnected);
        true
    }

    /// One connection: connect, register, then read until it drops. Whether it registered, and the problem.
    async fn session(self: &Arc<Self>, g: u64) -> (bool, Option<String>) {
        let net = self.network();
        if net.use_proxy && net.proxy_id != 0 {
            // autobrr's proxies aren't ported, so the proxy can't be loaded: autobrr refuses to connect then
            self.refuse_connect("configured proxy could not be loaded");
            return (false, None);
        }
        let addr =
            if net.use_bouncer && !net.bouncer_addr.is_empty() { net.bouncer_addr.clone() } else { format!("{}:{}", net.server, net.port) };
        let wire = match connect(&addr, net.tls, net.tls_skip_verify).await {
            Ok(w) => w,
            Err(ConnectError::Certificate(e)) => {
                let e = format!("TLS certificate verification failed: {e}");
                self.add_connect_error(&e);
                self.conn_error();
                self.stop();
                return (false, Some(e));
            }
            Err(ConnectError::Other(e)) => return (false, Some(e)),
        };
        if !self.current(g) {
            return (false, None);
        }
        let (r, mut w) = tokio::io::split(wire);
        let mut lines = BufReader::new(r).lines();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        *self.out.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        {
            let mut st = self.lock();
            reset_session(&mut st);
            st.session.failed_nickserv = 0;
        }
        let sasl = net.auth.mechanism == AUTH_SASL_PLAIN && !net.auth.account.is_empty() && !net.auth.password.is_empty();
        let user = if net.auth.account.is_empty() { net.nick.clone() } else { net.auth.account.clone() };
        let mut nick = net.nick.clone();
        if !net.pass.is_empty() {
            let _ = self.send("PASS", &[&net.pass]);
        }
        if sasl {
            let _ = self.send("CAP", &["LS", "302"]);
        }
        let _ = self.send("NICK", &[&nick]);
        let _ = self.send("USER", &[&user, "0", "*", &user]);
        let started = Instant::now();
        let mut tick = tokio::time::interval(self.timing.tick);
        let mut pinged = false;
        let mut last_read = Instant::now();
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let line = match line {
                        Ok(Some(l)) => l,
                        Ok(None) => return (self.lock().session.registered, Some("The IRC server closed the connection".into())),
                        Err(e) => return (self.lock().session.registered, Some(format!("The IRC connection broke: {e}"))),
                    };
                    last_read = Instant::now();
                    pinged = false;
                    let m = parse_line(&line);
                    if !self.lock().session.registered {
                        match self.registration(&m, &net, &mut nick, sasl) {
                            Ok(()) => {}
                            Err(e) => return (false, Some(e)),
                        }
                    }
                    if m.command == "ERROR" {
                        let why = m.params.last().cloned().unwrap_or_default();
                        return (self.lock().session.registered, Some(format!("The IRC server ended the connection: {why}")));
                    }
                    let queued = self.on_line(&m);
                    for (p, c, l) in queued {
                        let _ = p.add_line(&c, &l).await;
                    }
                }
                Some(out) = rx.recv() => {
                    let quit = out.starts_with("QUIT ");
                    if w.write_all(out.as_bytes()).await.is_err() || w.flush().await.is_err() {
                        return (self.lock().session.registered, Some("The IRC connection broke".into()));
                    }
                    if quit {
                        return (self.lock().session.registered, None);
                    }
                }
                _ = tick.tick() => {
                    if !self.current(g) {
                        // a stop queues its QUIT just after it makes this connection old: send it before ending
                        while let Ok(Some(out)) = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await {
                            let quit = out.starts_with("QUIT ");
                            if w.write_all(out.as_bytes()).await.is_err() || w.flush().await.is_err() || quit {
                                break;
                            }
                        }
                        return (self.lock().session.registered, None);
                    }
                    let registered = self.lock().session.registered;
                    if !registered && started.elapsed() > self.timing.register_timeout {
                        return (false, Some(format!("{addr} didn't finish signing in")));
                    }
                    if last_read.elapsed() > self.timing.keepalive {
                        if pinged {
                            return (registered, Some("The IRC server stopped answering".into()));
                        }
                        pinged = true;
                        last_read = Instant::now();
                        let _ = self.send("PING", &["lantern"]);
                    }
                    self.on_tick();
                }
            }
        }
    }

    /// The registration lines irc-go handles: CAP and SASL, a taken nick, the end of the MOTD.
    fn registration(&self, m: &Message, net: &IrcNetwork, nick: &mut String, sasl: bool) -> Result<(), String> {
        match m.command.as_str() {
            "CAP" if sasl => {
                let sub = m.params.get(1).map(|s| s.to_uppercase()).unwrap_or_default();
                let caps = m.params.last().cloned().unwrap_or_default();
                match sub.as_str() {
                    "LS" => {
                        if caps.split_whitespace().any(|c| c == "sasl" || c.starts_with("sasl=")) {
                            let _ = self.send("CAP", &["REQ", "sasl"]);
                        } else {
                            let _ = self.send("CAP", &["END"]);
                        }
                    }
                    "ACK" => {
                        let _ = self.send("AUTHENTICATE", &["PLAIN"]);
                    }
                    "NAK" => {
                        let _ = self.send("CAP", &["END"]);
                    }
                    _ => {}
                }
            }
            "AUTHENTICATE" if m.params.first().is_some_and(|p| p == "+") => {
                let a = &net.auth.account;
                let token = base64::engine::general_purpose::STANDARD.encode(format!("{a}\0{a}\0{}", net.auth.password));
                let _ = self.send("AUTHENTICATE", &[&token]);
            }
            "903" | "907" => {
                let _ = self.send("CAP", &["END"]);
            }
            "904" | "905" | "906" => {
                let _ = self.send("CAP", &["END"]);
            }
            "433" | "437" => {
                nick.push('_');
                let _ = self.send("NICK", &[nick]);
            }
            "376" | "422" => {
                let reconnected = {
                    let mut st = self.lock();
                    st.session.registered = true;
                    st.session.connected_at = Some(Instant::now());
                    st.connected_since = Some(Utc::now());
                    st.session.on_connected_at = Some(Instant::now() + self.timing.after_connect);
                    let again = st.have_disconnected && st.live;
                    if again {
                        st.have_disconnected = false;
                    }
                    again.then(|| st.network.name.clone())
                };
                // autobrr's onConnect: told outside the lock
                if let Some(name) = reconnected {
                    self.releases.notify(super::notify::irc_payload(super::notify::IrcEvent::Reconnected, &name, &format!("Network: {name}")));
                }
            }
            "PING" => {}
            _ => {}
        }
        Ok(())
    }

    /// Handles one line (autobrr's callbacks); the announce lines to queue.
    fn on_line(&self, m: &Message) -> Queued {
        let mut queued = Vec::new();
        match m.command.as_str() {
            "PING" => {
                let _ = self.send("PONG", &[m.params.last().map(String::as_str).unwrap_or("")]);
            }
            "001" => {
                if let Some(n) = m.params.first() {
                    let mut st = self.lock();
                    st.current_nick = n.clone();
                    st.case_mapping = CaseMapping::Rfc1459;
                }
                self.refresh_invite_commands();
            }
            "005" => self.handle_isupport(m),
            "NICK" => self.on_nick(m),
            "MODE" => self.handle_mode(m),
            "501" if self.lock().network.bot_mode => self.authenticate(),
            "INVITE" => self.handle_invite(m),
            "PART" => self.handle_part(m),
            "PRIVMSG" => self.on_privmsg(m, &mut queued),
            "NOTICE" => self.on_notice(m),
            "KICK" => self.on_kick(m),
            "JOIN" => self.handle_join(m),
            "366" => self.handle_joined(m),
            "900" => {
                if m.params.len() >= 3 && self.is_our_nick(&m.params[0]) {
                    self.set_authenticated();
                }
            }
            "903" => {
                let mut st = self.lock();
                st.session.saslauthed = true;
                st.session.identify_outstanding = false;
            }
            "904" => {
                self.add_connect_error("authentication failed: SASL negotiation failed");
                self.conn_error();
                self.lock().session.saslauthed = false;
                self.stop();
            }
            "465" => {
                let reason = m.params.last().map(|r| r.trim().to_string()).unwrap_or_default();
                let e = if reason.is_empty() { "banned from network".to_string() } else { format!("banned from network: {reason}") };
                self.add_connect_error(&e);
                self.conn_error();
                self.stop();
            }
            "471" | "473" | "474" | "475" | "477" => self.handle_join_error(m),
            "401" => self.handle_no_such_nick(m),
            "TOPIC" | "332" => {}
            _ => {}
        }
        queued
    }

    /// Runs what is due: the sign-in after registering, NickServ retries, channel timers.
    fn on_tick(&self) {
        let now = Instant::now();
        let connected = {
            let mut st = self.lock();
            match st.session.on_connected_at {
                Some(t) if t <= now => {
                    st.session.on_connected_at = None;
                    true
                }
                _ => false,
            }
        };
        if connected {
            self.on_connected();
        }
        let retry = {
            let mut st = self.lock();
            match st.session.identify_retry_at {
                Some(t) if t <= now => {
                    st.session.identify_retry_at = None;
                    !st.session.identify_outstanding
                        && !st.session.authenticated
                        && !st.session.saslauthed
                        && st.network.auth.nickserv_enabled()
                }
                _ => false,
            }
        };
        if retry {
            let _ = self.nickserv_identify();
        }
        let due: Vec<(String, Timer)> = {
            let mut st = self.lock();
            st.channels
                .iter_mut()
                .filter_map(|(k, c)| match c.timer {
                    Some((t, kind)) if t <= now => {
                        c.timer = None;
                        Some((k.clone(), kind))
                    }
                    _ => None,
                })
                .collect()
        };
        for (k, kind) in due {
            self.on_timer(&k, kind);
        }
    }

    // ---- the network state machine (autobrr's state_machine.go) ----

    /// Moves the network to `to` when allowed, then runs what the state starts.
    fn conn_go(&self, to: ConnState) {
        {
            let mut st = self.lock();
            if !st.conn.can_go(to) {
                return;
            }
            st.conn = to;
        }
        match to {
            ConnState::Authenticating => {
                let needs = {
                    let st = self.lock();
                    st.network.auth.nickserv_enabled() && !st.session.saslauthed
                };
                if needs {
                    let _ = self.nickserv_identify();
                } else {
                    self.sm_on_authenticated();
                }
            }
            ConnState::Authenticated => self.conn_go(ConnState::JoiningChannels),
            ConnState::JoiningChannels => self.join_channels(),
            ConnState::FullyOperational | ConnState::PartiallyOperational => {
                self.lock().connection_errors.clear();
                self.broadcast_health();
            }
            ConnState::Error | ConnState::Disconnected => self.broadcast_health(),
            _ => {}
        }
    }

    /// The network failed (autobrr's `OnError`).
    fn conn_error(&self) {
        if self.lock().conn == ConnState::Error {
            return;
        }
        self.conn_go(ConnState::Error);
    }

    /// After registering: bot mode, then NickServ, then the channels (autobrr's `OnConnected`).
    fn on_connected(&self) {
        self.conn_go(ConnState::Connected);
        let (bot_mode, needs_auth, char, nick) = {
            let st = self.lock();
            (
                st.network.bot_mode,
                st.network.auth.nickserv_enabled() && !st.session.saslauthed,
                st.bot_mode_char.clone(),
                st.current_nick.clone(),
            )
        };
        if bot_mode && !char.is_empty() {
            let _ = self.send("MODE", &[&nick, &format!("+{char}")]);
        } else if needs_auth {
            self.conn_go(ConnState::Authenticating);
        } else {
            self.conn_go(ConnState::Authenticated);
        }
    }

    /// Signed in: the network moves on to joining (autobrr's `OnAuthenticated`).
    fn sm_on_authenticated(&self) {
        let cur = self.lock().conn;
        if cur == ConnState::Authenticating || cur == ConnState::Connected {
            self.conn_go(ConnState::Authenticated);
        }
    }

    /// Settles operational or error from the channels (autobrr's `updateOperationalState`).
    fn update_operational_state(&self) {
        let (enabled, monitoring, errored) = {
            let st = self.lock();
            let mut e = 0;
            let mut m = 0;
            let mut er = 0;
            for c in st.channels.values().filter(|c| c.enabled && c.default_channel) {
                e += 1;
                if c.monitoring && c.errors.is_empty() {
                    m += 1;
                } else if !c.errors.is_empty() {
                    er += 1;
                }
            }
            (e, m, er)
        };
        let go = |to: ConnState| {
            if self.lock().conn != to {
                self.conn_go(to);
            }
        };
        if enabled == 0 {
            return go(ConnState::FullyOperational);
        }
        if enabled - monitoring - errored > 0 {
            return;
        }
        if monitoring == enabled {
            go(ConnState::FullyOperational)
        } else if monitoring > 0 {
            go(ConnState::PartiallyOperational)
        } else {
            go(ConnState::Error)
        }
    }

    /// Whether the network is healthy: operational (or joining) and every enabled announce channel monitored
    /// without errors (autobrr's `computeHealthy`).
    fn compute_healthy(st: &State) -> bool {
        let channels_ok = st.channels.values().filter(|c| c.enabled && c.default_channel).all(|c| c.monitoring && c.errors.is_empty());
        matches!(st.conn, ConnState::FullyOperational | ConnState::JoiningChannels) && channels_ok
    }

    /// Sends the network's health to the event stream (autobrr's `HEALTH` event).
    fn broadcast_health(&self) {
        let data = {
            let st = self.lock();
            json!({ "network": st.network.id, "healthy": Self::compute_healthy(&st), "connection_errors": st.connection_errors })
        };
        self.broadcast("HEALTH", data);
    }

    /// Notes connection problem `e` once, for the health report.
    fn add_connect_error(&self, e: &str) {
        let mut st = self.lock();
        if !st.connection_errors.iter().any(|x| x == e) {
            st.connection_errors.push(e.to_string());
        }
    }

    /// Records why the network won't connect and stops it.
    fn refuse_connect(&self, reason: &str) {
        self.add_connect_error(reason);
        self.conn_error();
        self.stop();
    }

    // ---- signing in ----

    /// Whether `nick` is the nick Lantern has now.
    fn is_our_nick(&self, nick: &str) -> bool {
        let st = self.lock();
        !nick.is_empty() && !st.current_nick.is_empty() && st.case_mapping.equal(&st.current_nick, nick)
    }

    /// Whether `a` and `b` are the same name under the server's case mapping.
    fn identifiers_equal(&self, a: &str, b: &str) -> bool {
        self.lock().case_mapping.equal(a, b)
    }

    /// The IDENTIFY for the form in use.
    fn identify_command(st: &State) -> String {
        if st.session.identify_account_form && !st.network.auth.account.is_empty() {
            format!("IDENTIFY {} {}", st.network.auth.account, st.network.auth.password)
        } else {
            format!("IDENTIFY {}", st.network.auth.password)
        }
    }

    /// Sends NickServ IDENTIFY (autobrr's `NickServIdentify`).
    fn nickserv_identify(&self) -> Result<(), String> {
        let cmd = {
            let mut st = self.lock();
            if st.session.authenticated || st.session.saslauthed || !st.network.auth.nickserv_enabled() {
                st.session.identify_outstanding = false;
                return Ok(());
            }
            st.session.identify_outstanding = true;
            Self::identify_command(&st)
        };
        self.send("PRIVMSG", &["NickServ", &cmd]).inspect_err(|_| self.lock().session.identify_outstanding = false)
    }

    /// Signs in when needed (autobrr's `authenticate`).
    fn authenticate(&self) {
        let (authed, outstanding, nickserv) = {
            let st = self.lock();
            (st.session.authenticated || st.session.saslauthed, st.session.identify_outstanding, st.network.auth.nickserv_enabled())
        };
        if authed {
            self.set_authenticated();
        } else if outstanding {
        } else if nickserv {
            let _ = self.nickserv_identify();
        } else {
            self.set_authenticated();
        }
    }

    /// Signed in: errors cleared, then the channels (autobrr's `setAuthenticated`).
    fn set_authenticated(&self) {
        {
            let mut st = self.lock();
            st.session.identify_outstanding = false;
            if st.session.authenticated {
                return;
            }
            st.session.authenticated = true;
            st.connection_errors.clear();
            st.session.failed_nickserv = 0;
            st.identify_form_learned_account = st.session.identify_account_form;
        }
        self.sm_on_authenticated();
    }

    /// Reads the server's `005` lines: its case mapping and its bot mode letter.
    fn handle_isupport(&self, m: &Message) {
        if m.params.len() < 3 {
            return;
        }
        for token in &m.params[1..m.params.len() - 1] {
            if token == "-CASEMAPPING" {
                self.lock().case_mapping = CaseMapping::Rfc1459;
                self.refresh_invite_commands();
                continue;
            }
            let (name, value) = token.split_once('=').map(|(a, b)| (a, Some(b))).unwrap_or((token.as_str(), None));
            match name {
                "CASEMAPPING" => {
                    let (cm, known) = value.map(CaseMapping::parse).unwrap_or((CaseMapping::Ascii, false));
                    self.lock().case_mapping = if known { cm } else { CaseMapping::Ascii };
                    self.refresh_invite_commands();
                }
                "BOT" => self.lock().bot_mode_char = value.unwrap_or("").to_string(),
                _ => {}
            }
        }
    }

    /// Follows a nick change, Lantern's own included.
    fn on_nick(&self, m: &Message) {
        let Some(new) = m.params.first() else { return };
        let ours = {
            let mut st = self.lock();
            let cm = st.case_mapping;
            let ours = !st.current_nick.is_empty() && (cm.equal(&st.current_nick, &m.nick) || cm.equal(&st.current_nick, new));
            if ours {
                st.current_nick = new.clone();
            }
            ours
        };
        if ours {
            self.authenticate();
        }
    }

    /// Reads mode changes on Lantern's nick (`+r`: signed in to services; the bot mode).
    fn handle_mode(&self, m: &Message) {
        if m.params.len() < 2 {
            return;
        }
        let (target, modes) = (&m.params[0], &m.params[1]);
        if self.is_our_nick(target) && mode_adds(modes, b'r') {
            self.set_authenticated();
            return;
        }
        let (bot, char) = {
            let st = self.lock();
            (st.network.bot_mode, st.bot_mode_char.clone())
        };
        if bot && char.len() == 1 && self.is_our_nick(target) && mode_adds(modes, char.as_bytes()[0]) {
            self.authenticate();
        }
    }

    /// Reads a NOTICE: NickServ's answers and the invite bot's.
    fn on_notice(&self, m: &Message) {
        if self.identifiers_equal(&m.nick, "NickServ") {
            self.handle_nickserv(m);
            return;
        }
        self.handle_invite_response(m);
    }

    /// NickServ's answers (autobrr's `handleNickServ`).
    fn handle_nickserv(&self, m: &Message) {
        let Some(text) = m.params.get(1) else { return };
        let expecting = {
            let st = self.lock();
            st.session.identify_outstanding && !st.session.authenticated && !st.session.saslauthed && st.network.auth.nickserv_enabled()
        };
        if !expecting {
            // NickServ is back: an identify waiting for it goes now
            let pending = self.lock().session.identify_retries > 0;
            if pending {
                let mut st = self.lock();
                st.session.identify_retry_at = Some(Instant::now());
            }
            return;
        }
        if contains_any(
            text,
            &[
                "you're now logged in as",
                "password accepted",
                "you are now recognized",
                "you are now identified",
                "you are already logged in",
            ],
        ) {
            self.set_authenticated();
            return;
        }
        let can_escalate = {
            let st = self.lock();
            !st.session.authenticated
                && !st.session.saslauthed
                && !st.session.identify_escalated
                && !st.session.identify_account_form
                && st.network.auth.nickserv_enabled()
                && !st.network.auth.account.is_empty()
                && !st.case_mapping.equal(&st.network.auth.account, &st.current_nick)
        };
        if notice_allows_identify_escalation(text) && can_escalate {
            {
                let mut st = self.lock();
                st.session.identify_account_form = true;
                st.session.identify_escalated = true;
            }
            let _ = self.nickserv_identify();
            return;
        }
        if contains_any(
            text,
            &[
                "Invalid account credentials",
                "Authentication failed: Invalid account credentials",
                "password incorrect",
                "invalid password for",
            ],
        ) {
            let escalated = self.lock().session.identify_account_form;
            let e = if escalated {
                "authentication failed: NickServ rejected the account-qualified IDENTIFY. Either the password is wrong, or this network only supports 'IDENTIFY <password>' and needs the nick grouped to the account instead"
            } else {
                "authentication failed: Bad account credentials"
            };
            self.add_connect_error(e);
            self.unlearn_identify_form();
            self.conn_error();
            self.stop();
            return;
        }
        if contains_any(
            text,
            &[
                "Account does not exist",
                "Authentication failed: Account does not exist",
                "isn't registered.",
                "is not a registered nickname",
            ],
        ) {
            let same = {
                let st = self.lock();
                st.case_mapping.equal(&st.current_nick, &st.network.nick)
            };
            if same {
                self.add_connect_error("authentication failed: account does not exist");
                self.unlearn_identify_form();
                self.conn_error();
                self.stop();
                return;
            }
        }
        if contains_any(text, &["This nickname is registered and protected", "please choose a different nick", "choose a different nick"]) {
            self.authenticate();
            let attempts = {
                let mut st = self.lock();
                st.session.failed_nickserv += 1;
                st.session.failed_nickserv
            };
            if attempts >= 3 {
                self.add_connect_error("authentication failed: nick in use and not authenticated");
                self.conn_error();
                self.stop();
            }
        }
    }

    /// Goes back to the IDENTIFY form with the password alone.
    fn unlearn_identify_form(&self) {
        let mut st = self.lock();
        if st.session.identify_account_form {
            st.identify_form_learned_account = false;
        }
    }

    /// The outstanding IDENTIFY met no NickServ: try again later (autobrr's `handleNickServAbsent`).
    fn handle_nickserv_absent(&self) {
        let attempt = {
            let mut st = self.lock();
            if !st.session.identify_outstanding || st.session.authenticated || st.session.saslauthed {
                return;
            }
            st.session.identify_outstanding = false;
            st.session.identify_retries += 1;
            let e = "NickServ is not online: retrying identify";
            if !st.connection_errors.iter().any(|x| x == e) {
                st.connection_errors.push(e.into());
            }
            st.session.identify_retries
        };
        let mut delay = self.timing.connect_retry;
        for _ in 1..attempt {
            if delay >= self.timing.connect_retry_max {
                break;
            }
            delay *= 2;
        }
        let delay = delay.min(self.timing.connect_retry_max);
        self.lock().session.identify_retry_at = Some(Instant::now() + delay);
        if attempt == 1 {
            self.broadcast_health();
        }
    }

    // ---- channels ----

    /// Starts every enabled channel's join (autobrr's `JoinChannels`).
    fn join_channels(&self) {
        let keys: Vec<String> = self.lock().channels.iter().filter(|(_, c)| c.enabled).map(|(k, _)| k.clone()).collect();
        for k in keys {
            self.chan_start(&k);
        }
    }

    /// A channel's join workflow (autobrr's `ChannelStateMachine.Start`).
    fn chan_start(&self, key: &str) {
        let (state, enabled, invite) = {
            let st = self.lock();
            let Some(c) = st.channels.get(key) else { return };
            (c.state, c.enabled, !c.invite_command.is_empty())
        };
        if state == ChanState::Monitoring {
            return;
        }
        if !enabled {
            self.chan_go(key, ChanState::Disabled);
            return;
        }
        self.chan_go(key, if invite { ChanState::AwaitingInvite } else { ChanState::Joining });
    }

    /// Moves channel `key` to `to` when allowed, then runs what the state starts.
    fn chan_go(&self, key: &str, to: ChanState) {
        {
            let mut st = self.lock();
            let Some(c) = st.channels.get_mut(key) else { return };
            if !c.state.can_go(to) {
                return;
            }
            c.state = to;
            c.timer = None;
        }
        self.chan_entered(key, to);
    }

    /// What entering `state` does (autobrr's `onStateEntry`).
    fn chan_entered(&self, key: &str, state: ChanState) {
        if state == ChanState::Monitoring {
            self.chan_monitoring(key);
        }
        self.broadcast_state(key, state);
        match state {
            ChanState::Joining => self.chan_run_join(key),
            ChanState::AwaitingInvite => self.chan_awaiting_invite(key),
            ChanState::AwaitingInviteBot => {
                let attempts = self.lock().channels.get(key).map(|c| c.auth_attempts).unwrap_or(0);
                match retry_backoff(attempts) {
                    Some(d) => self.set_timer(key, d, Timer::InviteRetry),
                    None => self.chan_error(key, "invite retries exhausted"),
                }
            }
            ChanState::InviteFailedNoSuchNick => self.chan_go(key, ChanState::AwaitingInviteBot),
            _ => {}
        }
    }

    /// Makes channel `key` wait `after` for `t`.
    fn set_timer(&self, key: &str, after: Duration, t: Timer) {
        if let Some(c) = self.lock().channels.get_mut(key) {
            c.timer = Some((Instant::now() + after, t));
        }
    }

    /// Sends channel `key`'s new state to the event stream (autobrr's `STATE` event).
    fn broadcast_state(&self, key: &str, state: ChanState) {
        let data = {
            let st = self.lock();
            let Some(c) = st.channels.get(key) else { return };
            json!({
                "network": st.network.id, "channel": c.name, "type": "STATE", "state": state.name(),
                "connection_errors": c.errors, "healthy": Self::compute_healthy(&st), "time": time_text(Some(Utc::now())),
            })
        };
        self.broadcast("STATE", data);
    }

    /// Sends channel `key`'s invite command and waits for the invite; joins at once without one.
    fn chan_awaiting_invite(&self, key: &str) {
        let cmd = {
            let mut st = self.lock();
            let Some(c) = st.channels.get_mut(key) else { return };
            if !c.enabled {
                return;
            }
            if c.invite_command.is_empty() {
                None
            } else {
                c.auth_attempts += 1;
                c.invite_responded = false;
                c.invite_reject_reason.clear();
                Some(c.invite_command.clone())
            }
        };
        let Some(cmd) = cmd else { return self.chan_go(key, ChanState::Joining) };
        let Some((target, msg)) = cmd.split_once(' ') else { return self.chan_error(key, "invalid invite command") };
        if let Err(e) = self.send("PRIVMSG", &[target, msg]) {
            return self.chan_error(key, &format!("failed to send invite command: {e}"));
        }
        self.set_timer(key, self.timing.invite_timeout, Timer::Invite);
    }

    /// Sends the JOIN for channel `key` (with its key) and waits for it to be confirmed.
    fn chan_run_join(&self, key: &str) {
        let (name, password, invite, after_invite) = {
            let mut st = self.lock();
            let Some(c) = st.channels.get_mut(key) else { return };
            if !c.enabled {
                return;
            }
            let a = c.join_after_invite;
            c.join_after_invite = false;
            (c.name.clone(), c.password.clone(), c.invite_command.clone(), a)
        };
        if !invite.is_empty() && !after_invite {
            return self.chan_go(key, ChanState::AwaitingInvite);
        }
        let r = if password.is_empty() { self.send("JOIN", &[&name]) } else { self.send("JOIN", &[&name, &password]) };
        if let Err(e) = r {
            return self.chan_error(key, &e);
        }
        self.set_timer(key, self.timing.join_timeout, Timer::Join);
    }

    /// A due channel timer.
    fn on_timer(&self, key: &str, t: Timer) {
        let state = match self.lock().channels.get(key) {
            Some(c) => c.state,
            None => return,
        };
        match t {
            Timer::Invite if state == ChanState::AwaitingInvite => {
                let responded = self.lock().channels.get(key).is_some_and(|c| c.invite_responded);
                if !responded {
                    if let Some(c) = self.lock().channels.get_mut(key) {
                        c.state = ChanState::AwaitingInviteBot;
                    }
                    self.chan_entered(key, ChanState::AwaitingInviteBot);
                }
            }
            Timer::InviteGrace if state == ChanState::AwaitingInvite => {
                {
                    let mut st = self.lock();
                    let Some(c) = st.channels.get_mut(key) else { return };
                    let reason = if c.invite_reject_reason.is_empty() {
                        "invite bot responded but did not grant access".to_string()
                    } else {
                        c.invite_reject_reason.clone()
                    };
                    c.state = ChanState::InviteFailed;
                    c.set_error(&reason);
                }
                self.broadcast_state(key, ChanState::InviteFailed);
            }
            Timer::InviteRetry if state == ChanState::AwaitingInviteBot => self.chan_go(key, ChanState::AwaitingInvite),
            Timer::Join if state == ChanState::Joining => self.chan_error(key, "join not confirmed"),
            Timer::ErrorRetry(n) if state == ChanState::Error => {
                let (attempts, invite) = {
                    let st = self.lock();
                    let c = &st.channels[key];
                    (c.error_attempts, !c.invite_command.is_empty())
                };
                if attempts == n {
                    self.chan_go(key, if invite { ChanState::AwaitingInvite } else { ChanState::Joining });
                }
            }
            _ => {}
        }
    }

    /// The channel failed: noted, tried again later (autobrr's `enterError`).
    fn chan_error(&self, key: &str, reason: &str) {
        let attempt = {
            let mut st = self.lock();
            let Some(c) = st.channels.get_mut(key) else { return };
            c.state = ChanState::Error;
            c.error_attempts += 1;
            c.set_error(reason);
            c.error_attempts
        };
        self.broadcast_state(key, ChanState::Error);
        if attempt <= 8 {
            let delay = self.timing.error_retry.saturating_mul(1u32 << (attempt - 1)).min(Duration::from_secs(6 * 3600));
            self.set_timer(key, delay, Timer::ErrorRetry(attempt));
        }
        self.update_operational_state();
    }

    /// Channel `key` is joined: its announces are read from now on.
    fn chan_monitoring(&self, key: &str) {
        {
            let mut st = self.lock();
            let Some(c) = st.channels.get_mut(key) else { return };
            if c.state != ChanState::Monitoring {
                return;
            }
            c.auth_attempts = 0;
            c.error_attempts = 0;
            c.invite_responded = false;
            c.invite_reject_reason.clear();
            c.monitoring = true;
            c.monitoring_since = Some(Utc::now());
            c.errors.clear();
        }
        self.update_operational_state();
    }

    /// Every channel back to Idle, not monitoring (autobrr's `resetChannelState`).
    fn reset_channel_state(&self) {
        let keys: Vec<String> = {
            let mut st = self.lock();
            for c in st.channels.values_mut() {
                c.reset_monitoring();
                c.reset_sm();
            }
            st.channels.keys().cloned().collect()
        };
        for k in keys {
            self.broadcast_state(&k, ChanState::Idle);
        }
    }

    /// The channel `name` is kept under: its key, or one the server's case mapping says is the same.
    fn channel_key_of(&self, name: &str) -> Option<String> {
        let st = self.lock();
        let k = channel_key(name);
        if st.channels.contains_key(&k) {
            return Some(k);
        }
        st.channels.iter().find(|(_, c)| st.case_mapping.equal(&c.name, name)).map(|(k, _)| k.clone())
    }

    /// A JOIN: a channel Lantern didn't ask for is left again.
    fn handle_join(&self, m: &Message) {
        let Some(c) = m.params.first() else { return };
        if !self.is_our_nick(&m.nick) {
            return;
        }
        if self.channel_key_of(c).is_none() {
            let _ = self.part_channel(c);
        }
    }

    /// The end of a channel's names (`366`): the join is confirmed.
    fn handle_joined(&self, m: &Message) {
        if m.params.len() < 2 {
            return;
        }
        let target = &m.params[0];
        let ours = self.is_our_nick(target) || (target == "*" && self.lock().network.use_bouncer);
        if !ours {
            return;
        }
        if let Some(k) = self.channel_key_of(&m.params[1]) {
            let state = self.lock().channels.get(&k).map(|c| c.state);
            if state != Some(ChanState::Monitoring) {
                self.chan_go(&k, ChanState::Monitoring);
            }
            self.update_operational_state();
        }
    }

    /// Lantern left a channel.
    fn handle_part(&self, m: &Message) {
        let Some(c) = m.params.first() else { return };
        if !self.is_our_nick(&m.nick) {
            return;
        }
        let Some(k) = self.channel_key_of(c) else { return };
        let parted = {
            let mut st = self.lock();
            let Some(ch) = st.channels.get_mut(&k) else { return };
            ch.reset_monitoring();
            if ch.state == ChanState::Monitoring {
                ch.state = ChanState::Parted;
                true
            } else {
                false
            }
        };
        if parted {
            self.broadcast_state(&k, ChanState::Parted);
        }
    }

    /// Leaves channel `c`.
    fn part_channel(&self, c: &str) -> Result<(), String> {
        if self.lock().network.use_bouncer {
            return Ok(());
        }
        self.send("PART", &[c])
    }

    /// An invite to one of the channels: it is joined.
    fn handle_invite(&self, m: &Message) {
        if m.params.len() < 2 || !self.is_our_nick(&m.params[0]) {
            return;
        }
        let Some(k) = self.channel_key_of(&m.params[1]) else { return };
        let ok = {
            let mut st = self.lock();
            let Some(c) = st.channels.get_mut(&k) else { return };
            if matches!(c.state, ChanState::Monitoring | ChanState::Joining | ChanState::Disabled | ChanState::Parted) {
                false
            } else {
                c.join_after_invite = true;
                true
            }
        };
        if ok {
            self.chan_go(&k, ChanState::Joining);
        }
    }

    /// A refused join (full, invite only, banned, wrong key, needs a registered nick).
    fn handle_join_error(&self, m: &Message) {
        if m.params.len() < 2 {
            return;
        }
        let channel = &m.params[1];
        let server_reason = if m.params.len() > 2 { m.params.last().cloned().unwrap_or_default() } else { String::new() };
        let cause = match m.command.as_str() {
            "471" => "channel is full (+l)",
            "473" => "channel is invite-only (+i)",
            "474" => "banned from channel (+b)",
            "475" => "wrong or missing channel password (+k)",
            "477" => "must be identified with services to join (+r)",
            _ => "join rejected",
        };
        let mut e = format!("could not join {channel}: {cause}");
        if !server_reason.is_empty() {
            e = format!("{e} ({server_reason})");
        }
        if let Some(k) = self.channel_key_of(channel) {
            if let Some(c) = self.lock().channels.get_mut(&k) {
                c.set_error(&e);
            }
            self.chan_error(&k, &e);
        }
    }

    /// `401`: the invite bot or NickServ isn't on the network.
    fn handle_no_such_nick(&self, m: &Message) {
        let Some(nick) = m.params.get(1) else { return };
        if self.identifiers_equal(nick, "NickServ") {
            self.handle_nickserv_absent();
        }
        let keys: Vec<String> = {
            let st = self.lock();
            st.channels
                .iter()
                .filter(|(_, c)| c.enabled && st.case_mapping.equal(&invite_bot_nick(&c.invite_command), nick))
                .map(|(k, _)| k.clone())
                .collect()
        };
        for k in keys {
            self.chan_go(&k, ChanState::InviteFailedNoSuchNick);
        }
    }

    /// A NOTICE or DM from an invite bot while its channel waits (autobrr's `handleInviteResponse`).
    fn handle_invite_response(&self, m: &Message) {
        let Some(target) = m.params.first() else { return };
        if is_channel_target(target) || !self.is_our_nick(target) {
            return;
        }
        if m.nick.is_empty() || self.identifiers_equal(&m.nick, "NickServ") {
            return;
        }
        let reason = m.params.last().map(|r| r.trim().to_string()).unwrap_or_default();
        let grace = self.timing.invite_grace;
        let mut st = self.lock();
        let cm = st.case_mapping;
        for c in st.channels.values_mut() {
            if !c.enabled || c.state != ChanState::AwaitingInvite {
                continue;
            }
            let bot = invite_bot_nick(&c.invite_command);
            if bot.is_empty() || !cm.equal(&bot, &m.nick) {
                continue;
            }
            let mut e = format!("invite rejected by {}", m.nick);
            if !reason.is_empty() {
                e = format!("{e}: {reason}");
            }
            c.invite_reject_reason = e;
            if !c.invite_responded {
                c.invite_responded = true;
                c.timer = Some((Instant::now() + grace, Timer::InviteGrace));
            }
        }
    }

    /// A kick from a channel: noted in its history, and joined again when it was Lantern.
    fn on_kick(&self, m: &Message) {
        if m.params.len() < 2 || !self.is_our_nick(&m.params[1]) {
            return;
        }
        let Some(k) = self.channel_key_of(&m.params[0]) else { return };
        let reason = m.params[2..].join(" ");
        let msg = {
            let mut st = self.lock();
            let id = st.network.id;
            let Some(c) = st.channels.get_mut(&k) else { return };
            c.state = ChanState::Kicked;
            c.timer = None;
            c.reset_monitoring();
            let msg = IrcMessage {
                network: id,
                channel: c.name.clone(),
                nick: "<-*".into(),
                message: format!("{} was kicked from {} by {} ({reason})", m.params[1], c.name, m.nick),
                kind: "KICK".into(),
                time: Utc::now(),
            };
            c.add_message(msg.clone());
            msg
        };
        self.broadcast("PRIVMSG", serde_json::to_value(&msg).unwrap_or_default());
        self.broadcast_state(&k, ChanState::Kicked);
    }

    /// A channel message: kept in the history, and queued for reading when an announcer said it.
    fn on_privmsg(&self, m: &Message, queued: &mut Queued) {
        if m.params.len() < 2 {
            return;
        }
        let (target, text) = (&m.params[0], &m.params[1]);
        if text == "CLIENTINFO" {
            return;
        }
        if self.is_our_nick(target) {
            self.handle_invite_response(m);
            return;
        }
        let Some(k) = self.channel_key_of(target) else { return };
        let msg = {
            let mut st = self.lock();
            let cm = st.case_mapping;
            let id = st.network.id;
            let Some(c) = st.channels.get_mut(&k) else { return };
            let cleaned = if c.skip_clean { text.clone() } else { clean_message(text) };
            let msg = IrcMessage {
                network: id,
                channel: c.name.clone(),
                nick: m.nick.clone(),
                message: cleaned.clone(),
                kind: String::new(),
                time: Utc::now(),
            };
            c.add_message(msg.clone());
            if c.is_valid_announcer(&m.nick, cm) {
                if let Some(p) = &c.processor {
                    queued.push((p.clone(), c.name.clone(), cleaned));
                    c.last_announce = Some(Utc::now());
                }
            }
            msg
        };
        self.broadcast("PRIVMSG", serde_json::to_value(&msg).unwrap_or_default());
    }

    /// Matches every channel's invite command to its bot again (autobrr's `refreshInviteCommands`).
    fn refresh_invite_commands(&self) {
        let mut st = self.lock();
        let cm = st.case_mapping;
        let commands = parse_invite_commands(&st.network.invite_command);
        for c in st.channels.values_mut() {
            if !c.invite_bot.is_empty() {
                c.invite_command = match_invite_command(cm, &c.invite_bot, &commands);
            }
        }
    }

    /// Adds or turns on a channel without reconnecting (autobrr's `AddChannel`).
    fn add_channel(&self, ch: &IrcChannel) {
        let k = channel_key(&ch.name);
        let start = {
            let mut st = self.lock();
            let c = st.channels.entry(k.clone()).or_insert_with(|| Chan::new(&k, false, false, None));
            c.id = ch.id;
            c.enabled = ch.enabled;
            c.password = ch.password.clone();
            if !c.enabled || c.monitoring {
                false
            } else {
                c.reset_sm();
                true
            }
        };
        if start {
            self.chan_start(&k);
        }
    }

    /// Leaves channel `name` and forgets it.
    fn remove_channel(&self, name: &str) {
        let k = channel_key(name);
        let _ = self.part_channel(&k);
        self.lock().channels.remove(&k);
    }

    /// A channel's key or on/off changed (autobrr's `UpdateChannel`).
    fn update_channel(&self, ch: &IrcChannel) {
        let k = channel_key(&ch.name);
        let (was_enabled, old_pass, monitoring) = {
            let st = self.lock();
            let Some(c) = st.channels.get(&k) else {
                drop(st);
                return self.add_channel(ch);
            };
            (c.enabled, c.password.clone(), c.monitoring)
        };
        {
            let mut st = self.lock();
            if let Some(c) = st.channels.get_mut(&k) {
                c.id = ch.id;
                c.enabled = ch.enabled;
                c.password = ch.password.clone();
            }
        }
        let enabled_changed = ch.enabled != was_enabled;
        let pass_changed = ch.password != old_pass;
        if !enabled_changed && !pass_changed {
            return;
        }
        if !ch.enabled {
            let _ = self.part_channel(&k);
            if let Some(c) = self.lock().channels.get_mut(&k) {
                c.reset_monitoring();
                c.reset_sm();
            }
            self.broadcast_state(&k, ChanState::Idle);
        } else if !monitoring {
            if let Some(c) = self.lock().channels.get_mut(&k) {
                c.reset_sm();
            }
            self.chan_start(&k);
        }
    }

    /// The network and its channels with their health (autobrr's `ReportStatus` and the channel snapshots).
    fn report(&self, n: &IrcNetwork) -> Value {
        let st = self.lock();
        let mut v = health_base(n);
        if n.enabled {
            v["connection_errors"] = json!(st.connection_errors);
            if st.live {
                let connected = st.connected_since.is_some();
                v["connected"] = json!(connected);
                v["connected_since"] = json!(time_text(st.connected_since));
                v["current_nick"] = json!(st.current_nick);
                v["preferred_nick"] = json!(st.network.nick);
                if connected {
                    v["healthy"] = json!(Self::compute_healthy(&st));
                }
            }
            let mut chans: Vec<Value> = st
                .channels
                .values()
                .map(|c| {
                    json!({
                        "id": c.id, "enabled": c.enabled, "name": c.name, "password": redact(&c.password), "detached": false,
                        "state": c.state.name(), "monitoring": c.monitoring, "monitoring_since": time_text(c.monitoring_since),
                        "last_announce": time_text(c.last_announce), "connection_errors": c.errors,
                    })
                })
                .collect();
            chans.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            v["channels"] = Value::Array(chans);
        }
        v
    }
}

/// The session's sign-in reset (autobrr's `resetSessionLocked`).
fn reset_session(st: &mut State) {
    let learned = st.identify_form_learned_account;
    let failed = st.session.failed_nickserv;
    st.session = Session { identify_account_form: learned, failed_nickserv: failed, ..Session::default() };
    st.connection_errors.retain(|e| e != "NickServ is not online: retrying identify");
}

/// The channels of `defs` configured on the network, then the network's other channels (autobrr's
/// `initChannels`).
fn init_channels(st: &mut State, defs: &[Arc<IndexerDefinition>], sink: &Sink) {
    let configured: Vec<String> = st.network.channels.iter().flatten().map(|c| channel_key(&c.name)).collect();
    let commands = parse_invite_commands(&st.network.invite_command);
    for def in defs {
        let bot = definition_invite_bot(def);
        let invite = match_invite_command(st.case_mapping, &bot, &commands);
        let processor = Arc::new(Processor::new(def.clone(), sink.clone()));
        for c in def.irc.iter().flat_map(|i| i.channels.iter()) {
            let k = channel_key(&c.name);
            if !configured.contains(&k) {
                continue;
            }
            let skip = c.parse.as_ref().is_some_and(|p| p.skipcleanmessage);
            let mut ch = Chan::new(&k, true, skip, Some(processor.clone()));
            ch.invite_bot = bot.clone();
            ch.invite_command = invite.trim().to_string();
            ch.announcers = c.announcers.clone();
            st.channels.insert(k, ch);
        }
    }
    let net_channels: Vec<IrcChannel> = st.network.channels.clone().unwrap_or_default();
    for c in net_channels {
        let k = channel_key(&c.name);
        let ch = st.channels.entry(k.clone()).or_insert_with(|| Chan::new(&k, false, false, None));
        ch.id = c.id;
        ch.enabled = c.enabled;
        ch.password = c.password.clone();
    }
}

/// A network in autobrr's `IrcNetworkWithHealth` shape, not connected.
fn health_base(n: &IrcNetwork) -> Value {
    json!({
        "id": n.id, "name": n.name, "enabled": n.enabled, "server": n.server, "port": n.port, "tls": n.tls,
        "tls_skip_verify": n.tls_skip_verify, "pass": redact(&n.pass), "nick": n.nick, "auth": n.auth.public(),
        "invite_command": n.invite_command, "use_bouncer": n.use_bouncer, "bouncer_addr": n.bouncer_addr, "bot_mode": n.bot_mode,
        "current_nick": "", "preferred_nick": "", "use_proxy": n.use_proxy, "proxy_id": n.proxy_id, "proxy": n.proxy,
        "channels": [], "connected": false, "connected_since": time_text(None), "connection_errors": [], "healthy": false,
    })
}

// ---- connecting ----

/// Something IRC can be spoken over: a TCP or TLS stream.
trait Wire: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Wire for T {}

/// Why a connection couldn't be made.
enum ConnectError {
    /// The server's certificate didn't verify.
    Certificate(String),
    /// Anything else, in words.
    Other(String),
}

/// A certificate check that accepts any certificate (the network's "skip TLS verification").
#[derive(Debug)]
struct NoVerify(Arc<tokio_rustls::rustls::crypto::CryptoProvider>);

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error> {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<tokio_rustls::rustls::client::danger::HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Connects to `addr` (`host:port`), with TLS when asked.
async fn connect(addr: &str, tls: bool, skip_verify: bool) -> Result<Box<dyn Wire>, ConnectError> {
    let (host, port) = addr.rsplit_once(':').ok_or_else(|| ConnectError::Other(format!("{addr} has no port")))?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let port: u16 = port.parse().map_err(|_| ConnectError::Other(format!("{addr} has no valid port")))?;
    let tcp = tokio::time::timeout(Duration::from_secs(20), tokio::net::TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| ConnectError::Other(format!("{addr} didn't answer within 20 seconds")))?
        .map_err(|e| ConnectError::Other(format!("{addr} can't be reached: {e}")))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return Ok(Box::new(tcp));
    }
    use tokio_rustls::rustls;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnectError::Other(format!("TLS can't be set up: {e}")))?;
    let config = if skip_verify {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(NoVerify(provider))).with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    let name = rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|_| ConnectError::Other(format!("{host} is not a server name TLS can check")))?;
    tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map(|s| Box::new(s) as Box<dyn Wire>).map_err(|e| {
        if let Some(rustls::Error::InvalidCertificate(c)) = e.get_ref().and_then(|x| x.downcast_ref::<rustls::Error>()) {
            return ConnectError::Certificate(format!("{c:?}"));
        }
        ConnectError::Other(format!("The secure connection to {host} failed: {e}"))
    })
}

// ---- the service (autobrr's irc/service.go) ----

/// The running networks and the event stream.
#[derive(Clone)]
pub struct Irc {
    handlers: Arc<Mutex<HashMap<i64, Arc<Handler>>>>,
    events: broadcast::Sender<Event>,
    timing: Arc<RwLock<Timing>>,
}

impl Default for Irc {
    fn default() -> Irc {
        Irc { handlers: Default::default(), events: broadcast::channel(1024).0, timing: Default::default() }
    }
}

impl Irc {
    /// Uses `t` for networks started from now on (tests).
    pub fn set_timing(&self, t: Timing) {
        *self.timing.write().unwrap_or_else(|e| e.into_inner()) = t;
    }

    /// The event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Network `id`'s handler, when it has one.
    fn handler(&self, id: i64) -> Option<Arc<Handler>> {
        self.handlers.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
    }

    /// Stops and forgets network `id`'s handler.
    pub fn stop_and_remove(&self, id: i64) {
        if let Some(h) = self.handlers.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            h.stop();
        }
    }
}

impl Releases {
    /// Network `id` with its channels; autobrr's `record not found` when there is none.
    pub fn irc_network(&self, id: i64) -> ApiResult<IrcNetwork> {
        let r = self
            .db
            .one("SELECT * FROM autobrr_irc_networks WHERE id=?", p![id])?
            .ok_or_else(|| ApiError::NotFound("record not found".into()))?;
        let mut n = network_of(&r);
        n.channels = Some(self.irc_channels(id)?);
        Ok(n)
    }

    /// Network `id`'s channels.
    fn irc_channels(&self, id: i64) -> ApiResult<Vec<IrcChannel>> {
        Ok(self
            .db
            .query("SELECT * FROM autobrr_irc_channels WHERE network_id=? ORDER BY id", p![id])?
            .iter()
            .map(|r| IrcChannel {
                id: r.i("id"),
                enabled: r.b("enabled"),
                name: r.s("name"),
                password: r.s("password"),
                detached: false,
                monitoring: false,
            })
            .collect())
    }

    /// Every network with its channels, by name (autobrr's `ListNetworks`).
    pub fn irc_networks(&self) -> ApiResult<Vec<IrcNetwork>> {
        let rows = self.db.query("SELECT * FROM autobrr_irc_networks ORDER BY name ASC", p![])?;
        let mut out = Vec::new();
        for r in rows {
            let mut n = network_of(&r);
            n.channels = Some(self.irc_channels(n.id)?);
            out.push(n);
        }
        Ok(out)
    }

    /// Every network with its health (autobrr's `GetNetworksWithHealth`).
    pub fn irc_networks_with_health(&self) -> ApiResult<Vec<Value>> {
        Ok(self
            .irc_networks()?
            .iter()
            .map(|n| {
                if n.enabled {
                    match self.irc.handler(n.id) {
                        Some(h) => h.report(n),
                        None => health_base(n),
                    }
                } else {
                    let mut v = health_base(n);
                    v["channels"] = Value::Array(
                        n.channels
                            .iter()
                            .flatten()
                            .map(|c| {
                                json!({
                                    "id": c.id, "enabled": c.enabled, "name": c.name, "password": redact(&c.password), "detached": c.detached,
                                    "state": "", "monitoring": false, "monitoring_since": time_text(None), "last_announce": time_text(None),
                                    "connection_errors": [],
                                })
                            })
                            .collect(),
                    );
                    v
                }
            })
            .collect())
    }

    /// Saves network `n` without its channels (a new row when its id is 0); its id.
    fn write_network(&self, n: &IrcNetwork) -> ApiResult<i64> {
        let a = &n.auth;
        if n.id == 0 {
            Ok(self.db.execute(
                "INSERT INTO autobrr_irc_networks (enabled, name, server, port, tls, tls_skip_verify, pass, nick, auth_mechanism, auth_account, auth_password, invite_command, bouncer_addr, use_bouncer, bot_mode, use_proxy, proxy_id) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                p![n.enabled, n.name, n.server, n.port, n.tls, n.tls_skip_verify, n.pass, n.nick, a.mechanism, a.account, a.password, n.invite_command, n.bouncer_addr, n.use_bouncer, n.bot_mode, n.use_proxy, n.proxy_id],
            )?)
        } else {
            self.db.execute(
                "UPDATE autobrr_irc_networks SET enabled=?, name=?, server=?, port=?, tls=?, tls_skip_verify=?, pass=?, nick=?, auth_mechanism=?, auth_account=?, auth_password=?, invite_command=?, bouncer_addr=?, use_bouncer=?, bot_mode=?, use_proxy=?, proxy_id=? WHERE id=?",
                p![n.enabled, n.name, n.server, n.port, n.tls, n.tls_skip_verify, n.pass, n.nick, a.mechanism, a.account, a.password, n.invite_command, n.bouncer_addr, n.use_bouncer, n.bot_mode, n.use_proxy, n.proxy_id, n.id],
            )?;
            Ok(n.id)
        }
    }

    /// Saves channel `c` of network `id` (a new one when its id is 0; autobrr's `StoreChannel`).
    fn write_channel(&self, id: i64, c: &mut IrcChannel) -> ApiResult<()> {
        if c.id != 0 {
            self.db.execute(
                "UPDATE autobrr_irc_channels SET enabled=?, detached=?, name=?, password=? WHERE id=?",
                p![c.enabled, c.detached, c.name, c.password, c.id],
            )?;
        } else {
            c.id = self.db.execute(
                "INSERT INTO autobrr_irc_channels (network_id, enabled, detached, name, password) VALUES (?,?,?,?,?)",
                p![id, c.enabled, true, c.name, c.password],
            )?;
        }
        Ok(())
    }

    /// Starts network `n`'s handler, making it first when there is none (autobrr's `startNetwork`).
    fn start_network(&self, n: IrcNetwork) {
        if let Some(h) = self.irc.handler(n.id) {
            if h.stopped() {
                h.start();
            }
            return;
        }
        let defs = self.indexers_by_irc_server(&n.server);
        let timing = self.irc.timing.read().unwrap_or_else(|e| e.into_inner()).clone();
        let h = Handler::new(self.clone(), self.irc.events.clone(), timing, n.clone(), defs);
        self.irc.handlers.lock().unwrap_or_else(|e| e.into_inner()).insert(n.id, h.clone());
        h.start();
    }

    /// Starts every enabled network (autobrr's `StartHandlers`).
    pub fn start_irc(&self) {
        for n in self.irc_networks().unwrap_or_default().into_iter().filter(|n| n.enabled) {
            self.start_network(n);
        }
    }

    /// Reconnects `h` with network `n` (autobrr's `restartHandler`).
    fn restart_handler(&self, h: &Arc<Handler>, n: &IrcNetwork) {
        h.stop();
        h.lock().network = n.clone();
        h.reload_channels(self.indexers_by_irc_server(&n.server));
        h.start();
    }

    /// Applies network `n`'s change to its handler: a reconnect when it needs one, else nick and channel
    /// changes in place (autobrr's `checkIfNetworkRestartNeeded`).
    fn check_restart(&self, n: &IrcNetwork) {
        let Some(h) = self.irc.handler(n.id) else { return self.start_network(n.clone()) };
        if h.stopped() {
            return self.restart_handler(&h, n);
        }
        let cur = h.network();
        if !cur.restart_fields(n).is_empty() {
            return self.restart_handler(&h, n);
        }
        if cur.nick != n.nick {
            let _ = h.send("NICK", &[&n.nick]);
        }
        let expected: Vec<String> = n.channels.iter().flatten().map(|c| channel_key(&c.name)).collect();
        let current: Vec<String> = cur.channels.iter().flatten().map(|c| channel_key(&c.name)).collect();
        for c in cur.channels.iter().flatten() {
            if !expected.contains(&channel_key(&c.name)) {
                h.remove_channel(&c.name);
            }
        }
        for c in n.channels.iter().flatten() {
            if current.contains(&channel_key(&c.name)) {
                h.update_channel(c);
            } else {
                h.add_channel(c);
            }
        }
        h.lock().network = n.clone();
        h.init_indexers(self.indexers_by_irc_server(&n.server));
    }

    /// Adds network `n`, or adds its channels and invite command to the network with the same server, port and
    /// nick (autobrr's `StoreNetwork`).
    pub fn store_irc_network(&self, mut n: IrcNetwork) -> ApiResult<()> {
        n.auth.validate().map_err(ApiError::Internal)?;
        let existing = self
            .db
            .one("SELECT id FROM autobrr_irc_networks WHERE server=? AND port=? AND nick=?", p![n.server, n.port, n.nick])?
            .map(|r| r.i("id"));
        match existing {
            None => {
                n.id = 0;
                n.id = self.write_network(&n)?;
                for c in n.channels.iter_mut().flatten() {
                    self.write_channel(n.id, c)
                        .map_err(|e| ApiError::Internal(format!("error storing channel on network: {}", e.message())))?;
                }
                if n.enabled {
                    self.start_network(n);
                }
            }
            Some(id) => {
                let mut e = self.irc_network(id)?;
                if let Some(chans) = n.channels.as_mut() {
                    for c in chans.iter_mut() {
                        self.write_channel(id, c)?;
                    }
                    e.channels.get_or_insert_with(Vec::new).extend(chans.iter().cloned());
                }
                if !n.invite_command.is_empty() {
                    e.invite_command = [e.invite_command.clone(), n.invite_command.clone()].join(",");
                    self.db.execute("UPDATE autobrr_irc_networks SET invite_command=? WHERE id=?", p![e.invite_command, id])?;
                }
                if e.enabled {
                    self.check_restart(&e);
                }
            }
        }
        Ok(())
    }

    /// Changes network `n` (autobrr's `UpdateNetwork`): redacted secrets keep the saved ones, the channels sent
    /// replace the saved ones, then the handler reconnects, updates in place or stops.
    pub fn update_irc_network(&self, mut n: IrcNetwork) -> ApiResult<()> {
        let e = self.irc_network(n.id)?;
        if is_redacted(&n.pass) {
            n.pass = e.pass.clone();
        }
        if is_redacted(&n.auth.password) {
            n.auth.password = e.auth.password.clone();
        }
        n.auth.validate().map_err(ApiError::Internal)?;
        self.write_network(&n)?;
        if let Some(chans) = n.channels.as_mut() {
            for c in chans.iter_mut() {
                if is_redacted(&c.password) {
                    let saved = e
                        .channels
                        .iter()
                        .flatten()
                        .find(|x| x.id == c.id)
                        .ok_or_else(|| ApiError::Internal("could not find channel in existing network".into()))?;
                    c.password = saved.password.clone();
                }
            }
            self.db.execute("DELETE FROM autobrr_irc_channels WHERE network_id=?", p![n.id])?;
            for c in chans.iter_mut() {
                c.id = 0;
                self.write_channel(n.id, c)?;
            }
        }
        if n.channels.is_none() {
            n.channels = e.channels.clone();
        }
        if n.enabled {
            self.check_restart(&n);
        } else {
            self.irc.stop_and_remove(n.id);
        }
        Ok(())
    }

    /// Removes network `id` and its channels.
    pub fn delete_irc_network(&self, id: i64) -> ApiResult<()> {
        self.irc_network(id)?;
        self.irc.stop_and_remove(id);
        self.db.execute("DELETE FROM autobrr_irc_channels WHERE network_id=?", p![id])?;
        self.db.execute("DELETE FROM autobrr_irc_networks WHERE id=?", p![id])?;
        Ok(())
    }

    /// Adds channel `c` to network `id` (autobrr's `StoreChannel`; the handler isn't told, as in autobrr).
    pub fn store_irc_channel(&self, id: i64, mut c: IrcChannel) -> ApiResult<()> {
        self.irc_network(id)?;
        self.write_channel(id, &mut c)
    }

    /// Reconnects network `id` (autobrr's `RestartNetwork`).
    pub fn restart_irc_network(&self, id: i64) -> ApiResult<()> {
        let n = self.irc_network(id)?;
        if !n.enabled {
            return Err(ApiError::Internal("network disabled, could not restart".into()));
        }
        match self.irc.handler(id) {
            Some(h) => self.restart_handler(&h, &n),
            None => self.start_network(n),
        }
        Ok(())
    }

    /// Says `msg` to `channel` on network `req.network_id` (autobrr's `SendCmd`; a failure is only logged).
    pub fn send_irc_cmd(&self, req: &SendCmd) -> Result<(), IrcError> {
        let h = self.irc.handler(req.network_id).ok_or(IrcError::NotFound)?;
        if let Err(e) = h.send("PRIVMSG", &[&req.channel, &req.msg]) {
            tracing::warn!("IRC: could not send a message: {e}");
        }
        Ok(())
    }

    /// Channel `channel`'s last lines on network `id` (autobrr's `GetMessageHistory`).
    pub fn irc_history(&self, id: i64, channel: &str) -> Result<Vec<IrcMessage>, IrcError> {
        let h = self.irc.handler(id).ok_or(IrcError::NotFound)?;
        let k = h.channel_key_of(channel).ok_or_else(|| IrcError::Other("could not find channel".into()))?;
        let st = h.lock();
        Ok(st.channels.get(&k).map(|c| c.messages.iter().cloned().collect()).unwrap_or_default())
    }

    /// Reads `msg` as if it had been announced on `channel` (autobrr's `ManualProcessAnnounce`).
    pub async fn manual_process_announce(&self, id: i64, channel: &str, msg: &str) -> Result<(), IrcError> {
        self.irc_network(id).map_err(|_| IrcError::NotFound)?;
        let h = self.irc.handler(id).ok_or(IrcError::NotFound)?;
        let k = h.channel_key_of(channel).ok_or(IrcError::ChannelNotFound)?;
        let (p, name) = {
            let st = h.lock();
            let c = &st.channels[&k];
            (c.processor.clone(), c.name.clone())
        };
        let p = p.ok_or(IrcError::NoAnnounceProcessor)?;
        p.add_line(&name, msg).await.map_err(|e| IrcError::Other(format!("could not send manual announce to processor: {e}")))
    }
}

/// A network for Lantern's own IRC source `s` (its settings: `nick`, `auth.account`, `auth.password`,
/// `invite_command`, `pass`, and `server`, `port`, `tls` and `channels` for a bouncer); `None` without a
/// definition that announces on IRC or without a nick.
pub fn network_for_source(def: &IndexerDefinition, s: &super::Source) -> Option<IrcNetwork> {
    let irc = def.irc.as_ref()?;
    let set = |k: &str| s.settings.get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let nick = set("nick")?;
    let port = if irc.port == 0 { 6667 } else { irc.port };
    let bouncer = match (set("server"), set("port")) {
        (None, None) => None,
        (host, p) => Some(format!("{}:{}", host.unwrap_or_else(|| irc.server.clone()), p.unwrap_or_else(|| port.to_string()))),
    };
    let channels: Vec<IrcChannel> = match set("channels") {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| {
                let (name, key) = c.split_once(' ').unwrap_or((c, ""));
                IrcChannel { enabled: true, name: name.to_string(), password: key.trim().to_string(), ..IrcChannel::default() }
            })
            .collect(),
        None => irc.channels.iter().map(|c| IrcChannel { enabled: true, name: c.name.clone(), ..IrcChannel::default() }).collect(),
    };
    let password = set("auth.password").unwrap_or_default();
    Some(IrcNetwork {
        name: if irc.network.is_empty() { def.name.clone() } else { irc.network.clone() },
        enabled: s.enabled,
        server: irc.server.clone(),
        port,
        tls: set("tls").map(|t| t == "true" || t == "1").unwrap_or(irc.tls),
        pass: set("pass").unwrap_or_default(),
        nick,
        auth: IrcAuth {
            mechanism: if password.is_empty() { AUTH_NONE.into() } else { AUTH_NICKSERV.into() },
            account: set("auth.account").unwrap_or_default(),
            password,
        },
        invite_command: set("invite_command").unwrap_or_default(),
        use_bouncer: bouncer.is_some(),
        bouncer_addr: bouncer.unwrap_or_default(),
        channels: Some(channels),
        ..IrcNetwork::default()
    })
}

impl Releases {
    /// The network Lantern's IRC source `s` uses: the one on its definition's server with its nick.
    fn source_network_id(&self, s: &super::Source) -> Option<i64> {
        let def = self.library.get(&s.definition)?;
        let server = def.irc.as_ref()?.server.clone();
        let nick = s.settings.get("nick")?.trim().to_string();
        self.db.one("SELECT id FROM autobrr_irc_networks WHERE server=? AND nick=?", p![server, nick]).ok().flatten().map(|r| r.i("id"))
    }

    /// Saves network `n` and replaces its channels, without touching its connection.
    fn save_network(&self, n: &mut IrcNetwork) -> ApiResult<()> {
        n.id = self.write_network(n)?;
        self.db.execute("DELETE FROM autobrr_irc_channels WHERE network_id=?", p![n.id])?;
        for c in n.channels.iter_mut().flatten() {
            c.id = 0;
            self.write_channel(n.id, c)?;
        }
        Ok(())
    }

    /// Makes or updates the network of Lantern's IRC source `s` (autobrr keeps networks apart from indexers;
    /// Lantern's sources hold both). With `apply` the running connection takes the change at once; without it
    /// the network is only saved (for [`Releases::start_irc`]).
    pub fn sync_source_network(&self, s: &super::Source, apply: bool) {
        let Some(def) = self.library.get(&s.definition) else { return };
        let Some(mut n) = network_for_source(&def, s) else { return };
        n.id = self.source_network_id(s).unwrap_or(0);
        let r = match (n.id, apply) {
            (_, false) => self.save_network(&mut n),
            (0, true) => self.save_network(&mut n).map(|_| {
                if n.enabled {
                    self.start_network(n.clone());
                }
            }),
            (_, true) => self.update_irc_network(n),
        };
        if let Err(e) = r {
            tracing::warn!("IRC: the network of {} couldn't be saved: {}", s.name, e.message());
        }
    }

    /// The IRC side of Lantern's source `s`: its network's id, whether it is connected, the channels it watches
    /// and the last problem.
    pub fn source_irc_status(&self, s: &super::Source) -> Value {
        let id = self.source_network_id(s);
        let Some(h) = id.and_then(|id| self.irc.handler(id)) else {
            return json!({ "network_id": id, "connected": false, "channels": [], "last_error": "" });
        };
        let st = h.lock();
        let channels: Vec<String> = st.channels.values().filter(|c| c.monitoring).map(|c| c.name.clone()).collect();
        json!({
            "network_id": id,
            "connected": st.connected_since.is_some(),
            "channels": channels,
            "last_error": st.connection_errors.last().cloned().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_irc_lines() {
        let m = parse_line(":Keeper!bot@tracker PRIVMSG #announce :New Torrent: a b c\r\n");
        assert_eq!(
            m,
            Message { nick: "Keeper".into(), command: "PRIVMSG".into(), params: vec!["#announce".into(), "New Torrent: a b c".into()] }
        );
        let m = parse_line("PING :irc.example");
        assert_eq!((m.command.as_str(), m.params[0].as_str()), ("PING", "irc.example"));
        let m = parse_line("@time=x :srv 001 me :Welcome");
        assert_eq!((m.nick.as_str(), m.command.as_str()), ("srv", "001"));
        assert_eq!(line_of("PRIVMSG", &["NickServ", "IDENTIFY pw"]), "PRIVMSG NickServ :IDENTIFY pw\r\n");
        assert_eq!(line_of("JOIN", &["#a", "key"]), "JOIN #a key\r\n");
    }

    // internal/irc/handler_test.go TestModeAdds
    #[test]
    fn test_mode_adds() {
        for (modes, flag, want) in [
            ("+r", b'r', true),
            ("+ir", b'r', true),
            ("-r", b'r', false),
            ("+r-r", b'r', false),
            ("+i", b'r', false),
            ("-x+B", b'B', true),
            ("", b'r', false),
        ] {
            assert_eq!(mode_adds(modes, flag), want, "{modes}");
        }
    }

    // internal/irc/casemapping_test.go TestIRCCaseMapping, TestParseIRCCaseMapping, TestStrictCaseMappingAliases
    #[test]
    fn test_irc_case_mapping() {
        assert!(CaseMapping::Rfc1459.equal("Nick[]\\^", "nick{}|~"));
        assert!(CaseMapping::StrictRfc1459.equal("Nick[]\\", "nick{}|"));
        assert!(!CaseMapping::StrictRfc1459.equal("^", "~"));
        assert!(!CaseMapping::Ascii.equal("[", "{"));
        assert!(CaseMapping::Ascii.equal("ABC", "abc"));
        assert_eq!(CaseMapping::parse("RFC1459"), (CaseMapping::Rfc1459, true));
        assert_eq!(CaseMapping::parse("strict-rfc1459"), (CaseMapping::StrictRfc1459, true));
        assert_eq!(CaseMapping::parse("rfc1459-strict"), (CaseMapping::StrictRfc1459, true));
        assert_eq!(CaseMapping::parse("ascii"), (CaseMapping::Ascii, true));
        assert_eq!(CaseMapping::parse("rfc7613"), (CaseMapping::Rfc1459, false));
        // TestChannelMapKeyPreservesUnicodeLowercasing
        assert_eq!(channel_key("#ÄNNOUNCE"), "#ännounce");
    }

    // internal/domain/irc_auth_test.go
    #[test]
    fn test_irc_auth_validate_and_nickserv_enabled() {
        let a = |m: &str, acc: &str, pw: &str| IrcAuth { mechanism: m.into(), account: acc.into(), password: pw.into() };
        for (auth, err) in [
            (a("NONE", "", ""), false),
            (a("NONE", "old", "old"), false),
            (a("SASL_PLAIN", "user", "secret"), false),
            (a("SASL_PLAIN", "", "secret"), false),
            (a("SASL_PLAIN", "user", ""), false),
            (a("NICKSERV", "user", "secret"), false),
            (a("NICKSERV", "", "secret"), false),
            (a("NICKSERV", "user", ""), false),
            (a("", "", ""), false),
            (a("", "", "secret"), false),
            (a("TYPO", "", "secret"), true),
        ] {
            assert_eq!(auth.validate().is_err(), err, "{auth:?}");
        }
        for (auth, want) in [
            (a("NONE", "", "secret"), false),
            (a("NICKSERV", "", "secret"), true),
            (a("NICKSERV", "", ""), false),
            (a("SASL_PLAIN", "user", "secret"), true),
            (a("SASL_PLAIN", "", "secret"), false),
            (a("", "", "secret"), true),
            (a("TYPO", "", "secret"), false),
        ] {
            assert_eq!(auth.nickserv_enabled(), want, "{auth:?}");
        }
        assert_eq!(AUTH_NONE, "NONE");
    }

    // internal/domain/irc_test.go TestIrcNetwork_DetermineIfRestartIsRequired
    #[test]
    fn test_irc_network_determine_if_restart_is_required() {
        let base = IrcNetwork { server: "irc.test".into(), port: 6697, tls: true, nick: "me".into(), ..IrcNetwork::default() };
        assert!(base.restart_fields(&base.clone()).is_empty());
        let mut d = base.clone();
        d.nick = "other".into();
        assert!(base.restart_fields(&d).is_empty(), "a nick change is done in place");
        d.port = 7000;
        d.invite_command = "Bot x".into();
        assert_eq!(base.restart_fields(&d), vec!["port", "invite command"]);
        let mut d = base.clone();
        d.proxy_id = 3;
        assert!(base.restart_fields(&d).is_empty(), "a proxy id without the proxy on dials nothing");
        d.use_proxy = true;
        assert_eq!(base.restart_fields(&d), vec!["use proxy", "proxy id"]);
    }

    #[test]
    fn invite_commands_and_bots() {
        assert_eq!(parse_invite_commands("/msg Drone enter #a u k, ,Bot2 invite x"), vec!["Drone enter #a u k", "Bot2 invite x"]);
        let cmds = parse_invite_commands("voyager2 x y,Voyager invite k");
        assert_eq!(match_invite_command(CaseMapping::Rfc1459, "voyager", &cmds), "Voyager invite k");
        assert_eq!(match_invite_command(CaseMapping::Rfc1459, "", &cmds), "");
        assert!(is_channel_target("#a") && is_channel_target("&a") && !is_channel_target("nick") && !is_channel_target(""));
        assert_eq!(retry_backoff(1), Some(Duration::from_secs(15)));
        assert_eq!(retry_backoff(9), Some(Duration::from_secs(30)));
        assert_eq!(retry_backoff(249), None);
    }

    #[test]
    fn strips_irc_formatting() {
        assert_eq!(clean_message("\x0304,01red\x03 \x02bold\x02 \x1funder\x1f 5,5 \x04ff0000hex"), "red bold under 5,5 hex");
    }
}
