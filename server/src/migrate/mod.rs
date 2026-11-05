//! Migration (goal 7): bringing an existing Prowlarr and FlareSolverr over to Lantern, with a dry run first.
//!
//! | Module | What |
//! | --- | --- |
//! | [`prowlarr`] | reading Prowlarr (a backup, its database, or its API) and turning it into a [`Plan`] |
//! | [`flaresolverr`] | reading FlareSolverr's settings (docker-compose, `.env`, `docker run`) into a [`Plan`] |
//! | [`api`] | the routes the web app calls |
//!
//! Every import is a dry run unless asked otherwise: the [`Plan`] says, line by line, what would be added, what is
//! already in Lantern, what is skipped and why, and what a person has to do by hand. Nothing already in Lantern is
//! changed or removed. Secret values (passwords, cookies, API keys) never appear in a plan: only their names do.
//!
//! Secrets go through the [`Secrets`] trait, so the account vault can take them over without changing the
//! importers. Until the vault is wired in, [`InPlace`] keeps them where Lantern keeps them today.

pub mod api;
pub mod flaresolverr;
pub mod prowlarr;

use kit_server_core::{p, Db, Result, RowExt, SqliteDb};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// What one line of a plan does (or would do, in a dry run).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Added to Lantern.
    Add,
    /// A Lantern setting is set.
    Set,
    /// Lantern has it already; left as it is.
    AlreadyThere,
    /// Not brought over; the detail says why.
    Skip,
    /// A person has to do this by hand; the detail says what.
    Manual,
    /// Lantern does this by itself; nothing to bring over.
    NothingNeeded,
}

/// One line of a plan.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Step {
    /// What the line is about, as the old app named it (`1337x`, `Sonarr`, `PORT`).
    pub name: String,
    /// What kind of thing it is, in words (`Torrent site`, `Usenet indexer`, `App`, `Setting`).
    pub what: String,
    /// What happens to it.
    pub action: Action,
    /// The reason or the thing to do, in a plain sentence.
    pub detail: String,
    /// The names of the secrets brought over with it (never their values).
    pub secrets: Vec<String>,
    /// The settings a person has to fill in after the import (the old app didn't hand them over).
    pub missing: Vec<String>,
}

impl Step {
    /// A step with no secrets and nothing missing.
    pub fn new(name: impl Into<String>, what: impl Into<String>, action: Action, detail: impl Into<String>) -> Step {
        Step { name: name.into(), what: what.into(), action, detail: detail.into(), secrets: Vec::new(), missing: Vec::new() }
    }
}

/// What an import does: one line per indexer, app and setting, and notes on what isn't brought over at all.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Plan {
    /// Where the import read from, in words (`a Prowlarr backup`).
    pub from: String,
    /// Whether this was only a look (nothing saved).
    pub dry_run: bool,
    /// The indexers.
    pub indexers: Vec<Step>,
    /// The apps (Sonarr, Radarr …) the old app synced to.
    pub apps: Vec<Step>,
    /// Settings and the steps for a person.
    pub settings: Vec<Step>,
    /// What isn't brought over, in sentences.
    pub notes: Vec<String>,
    /// One sentence on the whole import.
    pub summary: String,
}

impl Plan {
    /// Every step of the plan.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        self.indexers.iter().chain(&self.apps).chain(&self.settings)
    }

    /// How many steps do `action`.
    pub fn count(&self, action: Action) -> usize {
        self.steps().filter(|s| s.action == action).count()
    }

    /// Writes the summary from the steps.
    pub fn summarize(&mut self) {
        let n = |a| self.count(a);
        let (add, set, there, skip, manual) = (n(Action::Add), n(Action::Set), n(Action::AlreadyThere), n(Action::Skip), n(Action::Manual));
        let verb = if self.dry_run { "would be" } else { "were" };
        let mut parts = vec![format!("{add} {} {verb} added", plural(add, "item", "items"))];
        if set > 0 {
            parts.push(format!("{set} {} {verb} set", plural(set, "setting", "settings")));
        }
        if there > 0 {
            parts.push(format!("{there} {} already in Lantern", plural(there, "is", "are")));
        }
        if skip > 0 {
            parts.push(format!("{skip} {} skipped", plural(skip, "is", "are")));
        }
        if manual > 0 {
            parts.push(format!("{manual} {} for you to do", plural(manual, "step is", "steps are")));
        }
        let mut s = join_words(&parts);
        s.push('.');
        if self.dry_run {
            s.push_str(" Nothing was changed yet.");
        }
        self.summary = capitalize(&s);
    }
}

/// `one` or `many` by `n`.
fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// "a, b and c".
fn join_words(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The text with its first letter in capitals.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// What an import is asked to do.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Only look: say what would happen, change nothing.
    pub dry_run: bool,
    /// Take the old app's API key, so apps that used it keep working without changes.
    pub take_api_key: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { dry_run: true, take_api_key: true }
    }
}

/// Where imported secrets (passwords, cookies, API keys) are kept. The account vault implements this to take them
/// over; until then [`InPlace`] keeps them where Lantern keeps them today.
pub trait Secrets: Send + Sync {
    /// Keeps `value`, the secret `field` of `owner` (`indexer 1337x`, `app Sonarr`), and gives back what to save
    /// in its place (the value itself, or a reference the vault understands).
    fn keep(&self, owner: &str, field: &str, value: &str) -> Result<String>;
}

/// Keeps each secret where it was going anyway: in the indexer's settings or the app's row, as the rest of Lantern
/// does until the account vault is wired in.
pub struct InPlace;

impl Secrets for InPlace {
    fn keep(&self, _owner: &str, _field: &str, value: &str) -> Result<String> {
        Ok(value.to_string())
    }
}

/// Whether a value is a mask the old app sent instead of a secret (`********`): the real value has to be typed again.
pub fn masked(v: &str) -> bool {
    let v = v.trim();
    v.len() >= 4 && v.chars().all(|c| c == '*' || c == '•')
}

/// An indexer as the old app had it, its settings flattened to one level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceIndexer {
    /// Its id in the old app (kept in Lantern when free, so the apps' indexer addresses still work).
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The old app's kind of indexer (`Cardigann`, `Newznab`, `Torznab`, or a built-in one like `IPTorrents`).
    pub implementation: String,
    /// The definition file, for Cardigann (`1337x`).
    pub definition: String,
    /// Searched at all.
    pub enabled: bool,
    /// Lower is tried first (1–50).
    pub priority: i64,
    /// Its tags, by name.
    pub tags: Vec<String>,
    /// The site's address ("" for the definition's).
    pub base_url: String,
    /// Every other setting, by the old app's name for it.
    pub fields: BTreeMap<String, Value>,
}

/// An app (Sonarr, Radarr …) the old app pushed its indexers to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceApp {
    /// Its name.
    pub name: String,
    /// What app it is (`Sonarr`).
    pub implementation: String,
    /// The app's address.
    pub base_url: String,
    /// The app's API key.
    pub api_key: String,
    /// The address the app reached the old app at.
    pub source_url: String,
    /// `disabled`, `add_remove` or `full`.
    pub sync_level: String,
    /// Its tags, by name.
    pub tags: Vec<String>,
    /// The categories synced to it.
    pub categories: Vec<i64>,
}

/// A proxy the old app used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceProxy {
    /// Its name.
    pub name: String,
    /// `Http`, `Socks4`, `Socks5` or `FlareSolverr`.
    pub implementation: String,
    /// The proxy's host (FlareSolverr: its whole address).
    pub host: String,
    /// The proxy's port.
    pub port: i64,
    /// The proxy's user name.
    pub username: String,
    /// The proxy's password.
    pub password: String,
    /// The tags of the indexers it is for.
    pub tags: Vec<String>,
    /// Used for every request (the old app's own proxy), not only for tagged indexers.
    pub everywhere: bool,
}

impl SourceProxy {
    /// The proxy as Lantern's `site_proxy` setting takes it (`socks5://user:pass@host:1080`), or `None` for a kind
    /// Lantern can't use.
    pub fn url(&self) -> Option<String> {
        let scheme = match self.implementation.to_lowercase().as_str() {
            "http" => "http",
            "socks5" => "socks5",
            _ => return None,
        };
        let host = self.host.trim();
        if host.is_empty() {
            return None;
        }
        let enc = |s: &str| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string();
        let auth = if self.username.is_empty() { String::new() } else { format!("{}:{}@", enc(&self.username), enc(&self.password)) };
        let port = if self.port > 0 { format!(":{}", self.port) } else { String::new() };
        Some(format!("{scheme}://{auth}{host}{port}"))
    }
}

/// Everything read from the old app, before it is matched against Lantern.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Source {
    /// Where it was read from, in words.
    pub from: String,
    /// The indexers.
    pub indexers: Vec<SourceIndexer>,
    /// The apps it synced to.
    pub apps: Vec<SourceApp>,
    /// Its proxies (its own proxy for every request first, then the tagged ones).
    pub proxies: Vec<SourceProxy>,
    /// Its API key, when it was found.
    pub api_key: Option<String>,
    /// The port it listened on, when known.
    pub port: Option<u16>,
    /// The path it was served under (`/prowlarr`), when it had one.
    pub url_base: String,
    /// The names of its download clients.
    pub download_clients: Vec<String>,
    /// How many notification connections it had.
    pub notifications: usize,
}

/// The table that keeps the imported apps until app sync takes them over.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS imported_apps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            base_url TEXT NOT NULL DEFAULT '',
            api_key TEXT NOT NULL DEFAULT '',
            source_url TEXT NOT NULL DEFAULT '',
            sync_level TEXT NOT NULL DEFAULT 'full',
            tags TEXT NOT NULL DEFAULT '',
            categories TEXT NOT NULL DEFAULT '[]',
            imported REAL NOT NULL
        );",
    )
    .map_err(|e| anyhow::anyhow!("The imported apps' table can't be made: {}", e.detail))
}

/// The imported apps, as the web app shows them (API keys masked).
pub fn imported_apps(db: &SqliteDb) -> Result<Vec<Value>> {
    Ok(db
        .query("SELECT * FROM imported_apps ORDER BY name COLLATE NOCASE", p![])?
        .iter()
        .map(|r| {
            json!({
                "id": r.i("id"),
                "name": r.s("name"),
                "kind": r.s("kind"),
                "base_url": r.s("base_url"),
                "api_key": if r.s("api_key").is_empty() { "" } else { crate::indexers::MASK },
                "source_url": r.s("source_url"),
                "sync_level": r.s("sync_level"),
                "tags": r.s("tags"),
                "categories": serde_json::from_str::<Value>(&r.s("categories")).unwrap_or(json!([])),
                "imported": r.f("imported"),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_sum_up_in_words_and_masks_are_seen() {
        let mut p = Plan { dry_run: true, ..Default::default() };
        p.indexers.push(Step::new("a", "Torrent site", Action::Add, ""));
        p.indexers.push(Step::new("b", "Torrent site", Action::Add, ""));
        p.indexers.push(Step::new("c", "Torrent site", Action::Skip, ""));
        p.settings.push(Step::new("PORT", "Setting", Action::Manual, ""));
        p.summarize();
        assert_eq!(p.summary, "2 items would be added, 1 is skipped and 1 step is for you to do. Nothing was changed yet.");
        assert!(masked("********"));
        assert!(!masked("hunter2"));
        assert!(!masked(""));
    }

    #[test]
    fn proxies_become_one_address() {
        let p = SourceProxy {
            implementation: "Socks5".into(),
            host: "proxy.lan".into(),
            port: 1080,
            username: "me".into(),
            password: "p@ss".into(),
            ..Default::default()
        };
        assert_eq!(p.url().as_deref(), Some("socks5://me:p%40ss@proxy.lan:1080"));
        assert_eq!(SourceProxy { implementation: "Socks4".into(), host: "h".into(), ..Default::default() }.url(), None);
    }
}
