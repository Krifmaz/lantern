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
//! Secrets go through the [`Secrets`] trait: [`InVault`] saves them in the account vault as well as where indexers
//! read them today.

pub mod api;
pub mod flaresolverr;
pub mod prowlarr;

use kit_server_core::{p, Db, Result, RowExt, SqliteDb};
use serde::Serialize;
use serde_json::{json, Map, Value};
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
    /// Added turned off: it needs a person's help (a definition) before it can search.
    NeedsHelp,
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
    /// The path it took into Lantern (`Lantern's definition 1337x`, `Answered as Torznab at …`), "" for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub how: String,
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
        Step {
            name: name.into(),
            what: what.into(),
            action,
            how: String::new(),
            detail: detail.into(),
            secrets: Vec::new(),
            missing: Vec::new(),
        }
    }
}

/// What an import does: one line per indexer, app and setting, and notes on what isn't brought over at all.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Plan {
    /// Where the import read from, in words (`a Prowlarr backup`).
    pub from: String,
    /// Whether this was only a look (nothing saved).
    pub dry_run: bool,
    /// The custom definitions brought over (Prowlarr's `Definitions/Custom`).
    pub definitions: Vec<Step>,
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
        self.definitions.iter().chain(&self.indexers).chain(&self.apps).chain(&self.settings)
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
        let help = n(Action::NeedsHelp);
        if help > 0 {
            parts.push(format!("{help} {} your help (added turned off)", plural(help, "indexer needs", "indexers need")));
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

/// Who an imported secret belongs to.
#[derive(Debug, Clone, Copy)]
pub struct Owner<'a> {
    /// The vault's kind for it: `tracker`, `usenet_indexer` or `other`.
    pub kind: &'a str,
    /// Its name (`1337x`, `Sonarr`).
    pub name: &'a str,
    /// The site it is for: a definition's id or an address.
    pub site: &'a str,
    /// The user name it signs in with, "" for none.
    pub username: &'a str,
}

/// Where imported secrets (passwords, cookies, API keys) are kept: [`InVault`] saves them in the account vault,
/// [`InPlace`] only where the importer puts them anyway.
pub trait Secrets: Send + Sync {
    /// Keeps `value`, the secret `field` of `owner`, and gives back what to save in its place (the value itself while
    /// indexers still read their secrets from their own settings).
    fn keep(&self, owner: &Owner, field: &str, value: &str) -> Result<String>;

    /// A sentence for the plan's notes on where the secrets went, after an import.
    fn note(&self) -> Option<String> {
        None
    }
}

/// Keeps each secret only where the importer puts it: the indexer's settings or the app's row (tests, and a server
/// without the vault).
pub struct InPlace;

impl Secrets for InPlace {
    fn keep(&self, _owner: &Owner, _field: &str, value: &str) -> Result<String> {
        Ok(value.to_string())
    }
}

/// The vault's field for an imported secret's name (`apikey` → `api_key`), `None` for one the vault has no field for.
pub fn vault_field(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase().replace(['_', '-'], "");
    Some(match n.as_str() {
        "password" => "password",
        "cookie" | "cookies" => "cookie",
        "passkey" => "passkey",
        "apikey" => "api_key",
        "rsskey" => "rss_key",
        "2fa" | "totp" | "otp" | "totpseed" => "totp_seed",
        _ => return None,
    })
}

/// Saves imported secrets in the account vault too, one item per account (found by name and site, or made), so they
/// are encrypted, backed up with the vault and shown on the Vault page. The value is also given back, because
/// indexers still sign in with the copy in their own settings. A vault that can't be written (locked) is noted in the
/// plan, and the import goes on.
pub struct InVault<'a> {
    /// The vault.
    pub vault: &'a crate::vault::Vault,
    /// name and site (lowercase) → the vault item made or found for it.
    items: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// The vault's answer when it couldn't be written, if it couldn't.
    failed: std::sync::Mutex<Option<String>>,
}

impl<'a> InVault<'a> {
    /// Keeping secrets in `vault`.
    pub fn new(vault: &'a crate::vault::Vault) -> InVault<'a> {
        InVault { vault, items: Default::default(), failed: Default::default() }
    }

    /// Saves `field` = `value` in the item of `owner`, making it when there is none.
    fn save(&self, owner: &Owner, field: &str, value: &str) -> Result<()> {
        let key = format!("{}\n{}", owner.name.to_lowercase(), owner.site.to_lowercase());
        let known = self.items.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
        let found = match known {
            Some(id) => Some(id),
            None => self.vault.list()?.into_iter().find_map(|it| {
                let text = |k: &str| it.get(k).and_then(Value::as_str).unwrap_or("").to_lowercase();
                (text("name") == owner.name.to_lowercase() && text("site") == owner.site.to_lowercase())
                    .then(|| it.get("id").and_then(Value::as_str).map(str::to_string))
                    .flatten()
            }),
        };
        let id = match found {
            Some(id) => {
                self.vault.update(&id, &json!({ "fields": { field: value } }))?;
                id
            }
            None => {
                let mut fields = Map::new();
                if !owner.username.is_empty() {
                    fields.insert("username".into(), json!(owner.username));
                }
                fields.insert(field.into(), json!(value));
                let made = self.vault.create(&json!({ "kind": owner.kind, "name": owner.name, "site": owner.site, "fields": fields }))?;
                made.get("id").and_then(Value::as_str).unwrap_or_default().to_string()
            }
        };
        self.items.lock().unwrap_or_else(|e| e.into_inner()).insert(key, id);
        Ok(())
    }
}

impl Secrets for InVault<'_> {
    fn keep(&self, owner: &Owner, field: &str, value: &str) -> Result<String> {
        if let Some(f) = vault_field(field) {
            if let Err(e) = self.save(owner, f, value) {
                tracing::warn!("Import: {}'s {f} couldn't be saved in the vault: {}", owner.name, e.detail);
                self.failed.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(e.detail);
            }
        }
        Ok(value.to_string())
    }

    fn note(&self) -> Option<String> {
        if let Some(why) = self.failed.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Some(format!("Some logins couldn't be saved in the vault ({why}); they stay with their indexers and apps."));
        }
        let n = self.items.lock().unwrap_or_else(|e| e.into_inner()).len();
        (n > 0).then(|| {
            format!("The logins and keys of {n} {} were also saved in the vault, encrypted.", if n == 1 { "account" } else { "accounts" })
        })
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
    /// Torrent (`Some(true)`) or Usenet (`Some(false)`), when Prowlarr's settings say.
    pub torrent: Option<bool>,
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
    /// Its custom Cardigann definitions (`Definitions/Custom/*.yml`): (file name, YAML).
    pub definitions: Vec<(String, String)>,
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
        assert_eq!(vault_field("apiKey"), Some("api_key"));
        assert_eq!(vault_field("rss_key"), Some("rss_key"));
        assert_eq!(vault_field("sort"), None);
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
