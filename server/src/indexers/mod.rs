//! Indexers (goal 1): torrent trackers from the Cardigann definitions and Usenet/torrent indexers that speak
//! Newznab/Torznab, behind one search. Lantern answers apps through Torznab/Newznab (per indexer and all of them
//! at once) and the Prowlarr API ([`api`]).
//!
//! | Module | What |
//! | --- | --- |
//! | [`definition`], [`template`], [`filters`], [`json_path`] | reading the Cardigann definitions |
//! | [`cardigann`], [`newznab`], [`http`] | talking to the sites |
//! | [`library`] | the definitions on disk and their updates |
//! | [`catalog`] | the torrent sites and Usenet indexers to add from, in one list |
//! | [`capabilities`], [`newznab_request`], [`newznab_results`], [`nabxml`], [`api`] | what apps get (ported from Prowlarr) |
//! | [`status`], [`limits`], [`rate_limit`] | holds after failures, query and grab limits, the wait between requests (ported from Prowlarr) |
//! | [`proxies`], [`proxies_api`] | indexer proxies: HTTP, SOCKS4, SOCKS5 and FlareSolverr (ported from Prowlarr) |
//! | [`newznab_caps`] | the capabilities of a Newznab/Torznab API, asked from it (ported from Prowlarr) |
//!
//! Each indexer is a row of `indexers`; its settings' secrets are never sent back by the API. The search itself
//! ([`Indexers::dispatch`]) follows Prowlarr's `ReleaseSearchService` and `HttpIndexerBase`.

pub mod api;
pub mod capabilities;
pub mod cardigann;
pub mod catalog;
pub mod categories;
pub mod definition;
pub mod filters;
pub mod http;
pub mod json_path;
pub mod library;
pub mod limits;
pub mod nabxml;
pub mod newznab;
pub mod newznab_caps;
pub mod newznab_request;
pub mod newznab_results;
pub mod proxies;
pub mod proxies_api;
pub mod rate_limit;
pub mod release;
pub mod status;
pub mod template;

use capabilities::{Capabilities, Category};
use cardigann::{Cardigann, Grabbed};
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use library::Library;
use newznab::Newznab;
use release::{Protocol, Query, Release, SearchKind};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The kinds of indexer.
pub const KINDS: [&str; 3] = ["cardigann", "newznab", "torznab"];

/// What a saved secret is sent back as.
pub const MASK: &str = "••••";

/// The tables of the indexers, their history and their stats.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS indexers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            definition TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL,
            base_url TEXT NOT NULL DEFAULT '',
            settings TEXT NOT NULL DEFAULT '{}',
            enabled INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 25,
            tags TEXT NOT NULL DEFAULT '',
            cookies TEXT NOT NULL DEFAULT '',
            failures INTEGER NOT NULL DEFAULT 0,
            disabled_until REAL NOT NULL DEFAULT 0,
            last_error TEXT NOT NULL DEFAULT '',
            added REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS indexer_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            indexer_id INTEGER NOT NULL,
            at REAL NOT NULL,
            kind TEXT NOT NULL,
            query TEXT NOT NULL DEFAULT '',
            ok INTEGER NOT NULL,
            ms INTEGER NOT NULL DEFAULT 0,
            results INTEGER NOT NULL DEFAULT 0,
            source TEXT NOT NULL DEFAULT '',
            error TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS indexer_history_by_indexer ON indexer_history(indexer_id, at);",
    )
    .map_err(|e| anyhow::anyhow!("The indexer tables can't be made: {}", e.detail))?;
    // columns added with Prowlarr's holds, redirects and history details
    let added = [
        ("indexers", "initial_failure", "REAL NOT NULL DEFAULT 0"),
        ("indexers", "most_recent_failure", "REAL NOT NULL DEFAULT 0"),
        ("indexers", "redirect", "INTEGER NOT NULL DEFAULT 0"),
        ("indexer_history", "host", "TEXT NOT NULL DEFAULT ''"),
        ("indexer_history", "data", "TEXT NOT NULL DEFAULT '{}'"),
        // Prowlarr's AppProfileId (1: its Standard profile) and DownloadClientId (0: none)
        ("indexers", "app_profile_id", "INTEGER NOT NULL DEFAULT 1"),
        ("indexers", "download_client_id", "INTEGER NOT NULL DEFAULT 0"),
        // the capabilities a Newznab/Torznab API answered (`NewznabSettings.Capabilities`), and when
        ("indexers", "capabilities", "TEXT NOT NULL DEFAULT ''"),
    ];
    for (table, col, def) in added {
        let have: Vec<String> = db
            .query(&format!("PRAGMA table_info({table})"), p![])
            .map_err(|e| anyhow::anyhow!("The indexer tables can't be read: {}", e.detail))?
            .iter()
            .map(|r| r.s("name"))
            .collect();
        if !have.iter().any(|c| c == col) {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {def}"))
                .map_err(|e| anyhow::anyhow!("The indexer tables can't be changed: {}", e.detail))?;
        }
    }
    proxies::migrate(db)
}

/// A configured indexer, as stored.
#[derive(Debug, Clone)]
pub struct Indexer {
    /// Its id.
    pub id: i64,
    /// `cardigann`, `newznab` or `torznab`.
    pub kind: String,
    /// The definition id (Cardigann) or preset (Newznab).
    pub definition: String,
    /// The name people see.
    pub name: String,
    /// The site's address (empty: the definition's first link).
    pub base_url: String,
    /// The settings (`.Config.*`; `apikey`, `api_path` for Newznab/Torznab).
    pub settings: BTreeMap<String, String>,
    /// Searched at all.
    pub enabled: bool,
    /// Lower is tried first (1–50).
    pub priority: i64,
    /// Tags (comma separated).
    pub tags: String,
    /// Apps are sent to the site for the file instead of getting it through Lantern (Prowlarr's `Redirect`; only
    /// Newznab and Torznab indexers can; Usenet indexers always do).
    pub redirect: bool,
    /// The session cookies kept from the last sign-in.
    pub cookies: String,
    /// How far its holds have grown after failures (Prowlarr's escalation level; 0: healthy).
    pub failures: i64,
    /// Not searched until this time (after failures; 0: not on hold).
    pub disabled_until: f64,
    /// When the failures in a row began (0: none).
    pub initial_failure: f64,
    /// The last failure (0: none).
    pub most_recent_failure: f64,
    /// The last failure, in words.
    pub last_error: String,
    /// When it was added.
    pub added: f64,
    /// The app profile apps sync it with (Prowlarr's `AppProfileId`; 1 is the Standard profile).
    pub app_profile_id: i64,
    /// The download client apps send its releases to (Prowlarr's `DownloadClientId`; 0: the app's own choice).
    pub download_client_id: i64,
    /// The capabilities answer (`t=caps`) its Newznab/Torznab API last gave ("" when none was fetched yet).
    pub caps_xml: String,
    /// Why its saved settings or cookies can't be opened (the vault is locked), "" when they can.
    pub locked: String,
}

/// The associated text an indexer's settings are sealed with (not its id: an import may change the id).
const SETTINGS_AAD: &str = "lantern-indexer-settings";

/// The associated text an indexer's session cookies are sealed with.
const COOKIES_AAD: &str = "lantern-indexer-cookies";

impl Indexer {
    /// The indexer of row `r`, whose settings and cookies are already opened (`settings`, `cookies`) or could not
    /// be (`locked` says why).
    fn from_row(r: &Row, settings: &str, cookies: String, locked: String) -> Indexer {
        let settings: BTreeMap<String, String> = serde_json::from_str::<Map<String, Value>>(settings)
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    match v {
                        Value::String(s) => s,
                        Value::Null => String::new(),
                        other => other.to_string(),
                    },
                )
            })
            .collect();
        Indexer {
            id: r.i("id"),
            kind: r.s("kind"),
            definition: r.s("definition"),
            name: r.s("name"),
            base_url: r.s("base_url"),
            settings,
            enabled: r.b("enabled"),
            priority: r.i("priority"),
            tags: r.s("tags"),
            redirect: r.b("redirect"),
            cookies,
            failures: r.i("failures"),
            disabled_until: r.f("disabled_until"),
            initial_failure: r.f("initial_failure"),
            most_recent_failure: r.f("most_recent_failure"),
            last_error: r.s("last_error"),
            added: r.f("added"),
            app_profile_id: r.i("app_profile_id"),
            download_client_id: r.i("download_client_id"),
            caps_xml: r.s("capabilities"),
            locked,
        }
    }

    /// Torrent or Usenet.
    pub fn protocol(&self) -> Protocol {
        if self.kind == "newznab" {
            Protocol::Usenet
        } else {
            Protocol::Torrent
        }
    }

    /// Whether apps can be sent to the site for the file (`SupportsRedirect`: Newznab and Torznab).
    pub fn supports_redirect(&self) -> bool {
        self.kind != "cardigann"
    }

    /// Whether it can give a later page of results (`SupportsPagination`: Newznab and Torznab; Cardigann sites
    /// answer nothing past the first page).
    pub fn supports_pagination(&self) -> bool {
        self.kind != "cardigann"
    }

    /// Its health, as Prowlarr keeps it.
    pub fn status(&self) -> status::Status {
        let t = |v: f64| (v > 0.0).then_some(v);
        status::Status {
            initial_failure: t(self.initial_failure),
            most_recent_failure: t(self.most_recent_failure),
            escalation_level: self.failures,
            disabled_till: t(self.disabled_until),
        }
    }

    /// Its query and grab limits.
    pub fn base_settings(&self) -> limits::BaseSettings {
        limits::BaseSettings::of(&self.settings)
    }

    /// Its torrent settings (seeding goals, magnet links first).
    pub fn torrent_settings(&self) -> limits::TorrentBaseSettings {
        limits::TorrentBaseSettings::of(&self.settings)
    }
}

/// What a search found: the releases, and the indexers that failed with why (a Lantern extra for its own pages;
/// apps get only the releases, as from Prowlarr).
#[derive(Debug, Default)]
pub struct Searched {
    /// The releases.
    pub releases: Vec<Release>,
    /// (indexer name, why) of each indexer that failed.
    pub failed: Vec<(String, String)>,
}

/// Why fetching a release's file failed, as Prowlarr tells apps apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrabError {
    /// The site no longer has it (it answered 404): `ReleaseUnavailableException`.
    Unavailable(String),
    /// The site says to slow down (429): held at least an hour.
    TooManyRequests(String),
    /// Anything else: `ReleaseDownloadException`.
    Failed(String),
}

impl GrabError {
    /// The problem in words.
    pub fn message(&self) -> &str {
        match self {
            GrabError::Unavailable(m) | GrabError::TooManyRequests(m) | GrabError::Failed(m) => m,
        }
    }
}

/// Whether a failure is the site asking Lantern to slow down (`TooManyRequestsException`,
/// `RequestLimitReachedException`), which holds the indexer at least an hour.
pub fn is_rate_limited(why: &str) -> bool {
    let w = why.to_lowercase();
    w.contains("(429)") || w.contains("limit reached") || w.contains("too many requests")
}

/// A running site client, kept so its cookies and request delay last between searches.
enum Client {
    /// A Cardigann site.
    Site(Cardigann),
    /// A Newznab/Torznab API.
    Api(Newznab),
}

/// The clients made so far: id → (settings fingerprint, client).
type ClientCache = HashMap<i64, (String, Arc<Client>)>;

/// The indexers: stored rows, running clients, the definitions.
#[derive(Clone)]
pub struct Indexers {
    /// The database.
    pub db: SqliteDb,
    /// The definitions.
    pub library: Library,
    /// id → (settings fingerprint, client).
    clients: Arc<Mutex<ClientCache>>,
    /// The settings (the site proxy), when there are any.
    pub settings: Option<kit_server_settings::Store>,
    /// The solver for Cloudflare checks, when there is one.
    pub solver: Option<crate::solver::Solver>,
    /// The vault that seals the settings and cookies at rest, when there is one (always, outside unit tests).
    pub vault: Option<crate::vault::Vault>,
    /// Held while an indexer's health is read and written, so two answers at once don't lose one.
    status_lock: Arc<Mutex<()>>,
    /// The capabilities Newznab/Torznab APIs answered: settings fingerprint → (when, answer), kept 7 days.
    caps: Arc<Mutex<newznab_caps::Cache>>,
    /// Where a proxy check asks (Prowlarr's ping address; tests point it elsewhere).
    proxy_ping: Arc<Mutex<String>>,
}

impl Indexers {
    /// The indexers on `db` with `library`.
    pub fn new(db: SqliteDb, library: Library) -> Indexers {
        status::mark_start(crate::auth::now());
        Indexers {
            db,
            library,
            clients: Default::default(),
            settings: None,
            solver: None,
            vault: None,
            status_lock: Default::default(),
            caps: Default::default(),
            proxy_ping: Arc::new(Mutex::new(proxies::PING_URL.to_string())),
        }
    }

    /// Where a proxy check asks now.
    pub fn proxy_ping_url(&self) -> String {
        self.proxy_ping.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Makes proxy checks ask `url` instead of Prowlarr's ping address (tests).
    pub fn set_proxy_ping_url(&self, url: &str) {
        *self.proxy_ping.lock().unwrap_or_else(|e| e.into_inner()) = url.to_string();
    }

    /// Drops every running client, so the next request makes them again (after a proxy changed).
    pub fn forget_clients(&self) {
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// The general proxy (`site_proxy`), "" for none.
    pub fn site_proxy(&self) -> String {
        self.settings.as_ref().map(|s| s.str("site_proxy")).unwrap_or_default()
    }

    /// The same, reading the site proxy from `settings`.
    pub fn with_settings(mut self, settings: kit_server_settings::Store) -> Indexers {
        self.settings = Some(settings);
        self
    }

    /// With `solver` for the sites' Cloudflare checks.
    pub fn with_solver(mut self, solver: crate::solver::Solver) -> Indexers {
        self.solver = Some(solver);
        self
    }

    /// With `vault` sealing the settings and cookies at rest.
    pub fn with_vault(mut self, vault: crate::vault::Vault) -> Indexers {
        self.vault = Some(vault);
        self
    }

    /// `text` as kept in the table: sealed by the vault (a 503 in words when it is locked), plain without one.
    fn seal(&self, aad: &str, text: &str) -> Result<String> {
        match &self.vault {
            Some(v) => v.seal_text(aad, text),
            None => Ok(text.to_string()),
        }
    }

    /// The indexer of row `r`, its settings and cookies opened.
    fn read(&self, r: &Row) -> Indexer {
        let open = |aad: &str, stored: String| match &self.vault {
            Some(v) => v.open_text(aad, &stored),
            None => Ok(stored),
        };
        match (open(SETTINGS_AAD, r.s("settings")), open(COOKIES_AAD, r.s("cookies"))) {
            (Ok(settings), Ok(cookies)) => Indexer::from_row(r, &settings, cookies, String::new()),
            (Err(why), _) | (_, Err(why)) => Indexer::from_row(r, "{}", String::new(), why),
        }
    }

    /// Seals the settings and cookies that were saved before Lantern sealed them; how many indexers it sealed.
    /// Does nothing while the vault is locked.
    pub fn seal_saved(&self) -> Result<usize> {
        let Some(v) = &self.vault else { return Ok(0) };
        if v.seal_text(SETTINGS_AAD, "").is_err() {
            return Ok(0);
        }
        let mut n = 0;
        for r in self.db.query("SELECT id, settings, cookies FROM indexers", p![])? {
            let (settings, cookies) = (r.s("settings"), r.s("cookies"));
            let plain_settings = !crate::vault::Vault::is_sealed(&settings);
            let plain_cookies = !cookies.is_empty() && !crate::vault::Vault::is_sealed(&cookies);
            if plain_settings || plain_cookies {
                let settings = if plain_settings { v.seal_text(SETTINGS_AAD, &settings)? } else { settings };
                let cookies = if plain_cookies { v.seal_text(COOKIES_AAD, &cookies)? } else { cookies };
                self.db.execute("UPDATE indexers SET settings=?, cookies=? WHERE id=?", p![settings, cookies, r.i("id")])?;
                n += 1;
            }
        }
        if n > 0 {
            // the plain copies would stay in the database file's free pages (and its write-ahead log) otherwise
            self.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(n)
    }

    /// Every indexer, by priority then name.
    pub fn list(&self) -> Result<Vec<Indexer>> {
        Ok(self.db.query("SELECT * FROM indexers ORDER BY priority, name COLLATE NOCASE", p![])?.iter().map(|r| self.read(r)).collect())
    }

    /// Indexer `id`; 404 when there is none.
    pub fn get(&self, id: i64) -> Result<Indexer> {
        self.db
            .one("SELECT * FROM indexers WHERE id=?", p![id])?
            .map(|r| self.read(&r))
            .ok_or_else(|| err(404, "There is no indexer with that id"))
    }

    /// Adds an indexer from `body` (`kind`, `definition`, `name`, `base_url`, `settings`, `enabled`, `priority`,
    /// `tags`); its id. 400 for an unknown kind or definition.
    pub fn add(&self, body: &Value) -> Result<i64> {
        let kind = body.get("kind").and_then(Value::as_str).unwrap_or("cardigann").to_string();
        if !KINDS.contains(&kind.as_str()) {
            return Err(err(400, "The kind must be cardigann, newznab or torznab"));
        }
        let definition = body.get("definition").and_then(Value::as_str).unwrap_or("").to_string();
        let mut name = body.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut base_url = body.get("base_url").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if kind == "cardigann" {
            let def = self.library.get(&definition).ok_or_else(|| err(400, format!("There is no indexer definition {definition}")))?;
            if name.is_empty() {
                name = def.name.clone();
            }
        } else {
            if let Some(p) = newznab::preset(&definition) {
                if name.is_empty() {
                    name = p.name.to_string();
                }
                if base_url.is_empty() {
                    base_url = p.url.to_string();
                }
            }
            if base_url.is_empty() {
                return Err(err(400, "Enter the indexer's address (its API without /api)"));
            }
        }
        if name.is_empty() {
            return Err(err(400, "Enter a name for the indexer"));
        }
        let mut settings = clean_settings(body.get("settings"), &BTreeMap::new());
        if kind == "newznab" {
            if let Some(p) = newznab::preset(&definition) {
                fill_from_preset(&mut settings, p);
            }
        }
        check_limits(&settings)?;
        let app_profile_id = int_of(body, &["app_profile_id", "appProfileId"]).unwrap_or(1);
        let download_client_id = int_of(body, &["download_client_id", "downloadClientId"]).unwrap_or(0);
        self.check_links(app_profile_id, download_client_id)?;
        let tags = self.tags_text(body.get("tags"), "");
        let settings = self.seal(SETTINGS_AAD, &serde_json::to_string(&settings).unwrap_or_default())?;
        let id = self.db.execute(
            "INSERT INTO indexers (kind, definition, name, base_url, settings, enabled, priority, tags, redirect, added, app_profile_id, download_client_id) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            p![
                kind,
                definition,
                name,
                base_url,
                settings,
                body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                body.get("priority").and_then(Value::as_i64).unwrap_or(25).clamp(1, 50),
                tags,
                body.get("redirect").and_then(Value::as_bool).unwrap_or(false),
                crate::auth::now(),
                app_profile_id,
                download_client_id
            ],
        )?;
        Ok(id)
    }

    /// Changes indexer `id`: only the fields `body` has; a masked or empty secret keeps the saved one.
    pub fn update(&self, id: i64, body: &Value) -> Result<()> {
        let cur = self.get(id)?;
        if !cur.locked.is_empty() {
            return Err(err(503, format!("{}'s saved login can't be opened: {}", cur.name, cur.locked)));
        }
        let settings = match body.get("settings") {
            Some(s) => clean_settings(Some(s), &cur.settings),
            None => cur.settings.clone(),
        };
        let text =
            |k: &str, old: &str| body.get(k).and_then(Value::as_str).map(|s| s.trim().to_string()).unwrap_or_else(|| old.to_string());
        let name = text("name", &cur.name);
        if name.is_empty() {
            return Err(err(400, "Enter a name for the indexer"));
        }
        check_limits(&settings)?;
        let app_profile_id = int_of(body, &["app_profile_id", "appProfileId"]).unwrap_or(cur.app_profile_id);
        let download_client_id = int_of(body, &["download_client_id", "downloadClientId"]).unwrap_or(cur.download_client_id);
        self.check_links(app_profile_id, download_client_id)?;
        self.db.execute(
            "UPDATE indexers SET name=?, base_url=?, settings=?, enabled=?, priority=?, tags=?, redirect=?, app_profile_id=?, download_client_id=?, failures=CASE WHEN ? THEN 0 ELSE failures END, disabled_until=CASE WHEN ? THEN 0 ELSE disabled_until END WHERE id=?",
            p![
                name,
                text("base_url", &cur.base_url),
                self.seal(SETTINGS_AAD, &serde_json::to_string(&settings).unwrap_or_default())?,
                body.get("enabled").and_then(Value::as_bool).unwrap_or(cur.enabled),
                body.get("priority").and_then(Value::as_i64).unwrap_or(cur.priority).clamp(1, 50),
                self.tags_text(body.get("tags"), &cur.tags),
                body.get("redirect").and_then(Value::as_bool).unwrap_or(cur.redirect),
                app_profile_id,
                download_client_id,
                body.get("settings").is_some(),
                body.get("settings").is_some() || body.get("enabled") == Some(&json!(true)),
                id
            ],
        )?;
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        Ok(())
    }

    /// An indexer's tags as kept (names, comma separated) from a request's `tags`: names as text, or Prowlarr's tag
    /// ids (or names) as a list; `old` when there is none.
    fn tags_text(&self, v: Option<&Value>, old: &str) -> String {
        match v {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Array(a)) => {
                let ids: Vec<i64> = a.iter().filter_map(Value::as_i64).collect();
                let mut out = self.tag_labels(&ids);
                for n in a.iter().filter_map(Value::as_str).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
                    if !out.is_empty() {
                        out.push(',');
                    }
                    out.push_str(&n);
                }
                out
            }
            Some(Value::Null) => String::new(),
            _ => old.to_string(),
        }
    }

    /// Prowlarr's checks of an indexer's app profile and download client (`IndexerController`): the profile id
    /// must be above 0 and the profile must exist; a download client other than 0 must exist. Profiles and
    /// clients are only looked up once Lantern keeps them.
    fn check_links(&self, app_profile_id: i64, download_client_id: i64) -> Result<()> {
        if app_profile_id <= 0 {
            return Err(err(400, "'App Profile Id' must be greater than '0'."));
        }
        if exists_in(&self.db, &["prowlarr_app_profiles", "app_profiles"], app_profile_id) == Some(false) {
            return Err(err(400, "App Profile does not exist"));
        }
        if download_client_id != 0
            && exists_in(&self.db, &["prowlarr_download_clients", "download_clients"], download_client_id) == Some(false)
        {
            return Err(err(400, "Download Client does not exist"));
        }
        Ok(())
    }

    /// Removes indexer `id` and its history.
    pub fn remove(&self, id: i64) -> Result<()> {
        self.get(id)?;
        self.db.execute("DELETE FROM indexers WHERE id=?", p![id])?;
        self.db.execute("DELETE FROM indexer_history WHERE indexer_id=?", p![id])?;
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        Ok(())
    }

    /// The running client of `ix`, made (or made again after a change) when needed.
    fn client(&self, ix: &Indexer) -> Result<Arc<Client>> {
        if !ix.locked.is_empty() {
            return Err(err(503, format!("{}'s saved login can't be opened: {}", ix.name, ix.locked)));
        }
        let site_proxy = self.site_proxy();
        // an HTTP/SOCKS proxy sharing a tag takes the place of the general one (`IndexerHttpClient`)
        let plan = self.plan_for(ix.id, &ix.tags);
        let proxy = plan.proxy.clone().unwrap_or_else(|| site_proxy.clone());
        let fp = format!("{}|{}|{:?}|{site_proxy}|{plan:?}", ix.base_url, ix.definition, ix.settings);
        if let Some((f, c)) = self.clients.lock().unwrap_or_else(|e| e.into_inner()).get(&ix.id) {
            if *f == fp {
                return Ok(c.clone());
            }
        }
        let c = match ix.kind.as_str() {
            "cardigann" => {
                let def = self.library.get(&ix.definition).ok_or_else(|| {
                    err(
                        409,
                        format!("{}: its definition {} is no longer in the definitions; pick another or remove it", ix.name, ix.definition),
                    )
                })?;
                let delay =
                    def.request_delay.unwrap_or(0.0).max(ix.settings.get("request_delay").and_then(|d| d.parse().ok()).unwrap_or(0.0));
                let http = http::Http::new(&ix.name, delay, &def.encoding, Some(&proxy))
                    .with_solver(self.solver.clone())
                    .with_flaresolverr(plan.flaresolverr.clone(), &site_proxy);
                let site = Cardigann::new(def, &ix.base_url, ix.settings.clone(), http);
                if !ix.cookies.is_empty() {
                    site.http.add_cookies(&site.base, &ix.cookies);
                }
                Client::Site(site)
            }
            kind => {
                let delay = ix.settings.get("request_delay").and_then(|d| d.parse().ok()).unwrap_or(0.0);
                Client::Api(Newznab {
                    name: ix.name.clone(),
                    base: ix.base_url.clone(),
                    api_path: ix.settings.get("api_path").cloned().unwrap_or_else(|| "/api".into()),
                    apikey: ix.settings.get("apikey").cloned().unwrap_or_default(),
                    protocol: if kind == "newznab" { Protocol::Usenet } else { Protocol::Torrent },
                    extra: ix.settings.get("extra").cloned().unwrap_or_default(),
                    basic: match (ix.settings.get("username"), ix.settings.get("password")) {
                        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Some((u.clone(), p.clone())),
                        _ => None,
                    },
                    http: http::Http::new(&ix.name, delay, "UTF-8", Some(&proxy))
                        .with_solver(self.solver.clone())
                        .with_flaresolverr(plan.flaresolverr.clone(), &site_proxy),
                })
            }
        };
        let c = Arc::new(c);
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).insert(ix.id, (fp, c.clone()));
        Ok(c)
    }

    /// Signs in and runs a small search: `Ok` with how many results came back, or the problem in words.
    pub async fn test(&self, id: i64) -> Result<usize> {
        let ix = self.get(id)?;
        let started = Instant::now();
        let r = match &*self.client(&ix)? {
            Client::Site(site) => {
                let out = async {
                    if site.def.login.is_some() {
                        site.login().await?;
                    }
                    site.search(&Query { rss: true, ..Query::default() }).await.map(|r| r.len())
                }
                .await;
                self.keep_cookies(&ix, site);
                out
            }
            Client::Api(api) => match api.test().await {
                Ok(()) => api.search(&Query { rss: true, ..Query::default() }).await.map(|r| r.len()),
                Err(e) => Err(e),
            },
        };
        self.record(&ix, "test", "", &r.as_ref().map(|n| *n).map_err(Clone::clone), started, "", "", json!({}));
        r.map_err(|e| err(422, e))
    }

    /// Saves a site's session cookies, so a restart doesn't sign in again.
    fn keep_cookies(&self, ix: &Indexer, site: &Cardigann) {
        let c = site.http.cookies(&site.base);
        if c != ix.cookies && !c.is_empty() {
            // never kept in the clear: with the vault locked they are only kept in memory
            if let Ok(sealed) = self.seal(COOKIES_AAD, &c) {
                let _ = self.db.execute("UPDATE indexers SET cookies=? WHERE id=?", p![sealed, ix.id]);
            }
        }
    }

    /// Notes a query, grab or test in the history (`kind`: `query`, `rss`, `grab`, `test`), with who asked
    /// (`source`, `host`) and Prowlarr's history details (`data`). A test also keeps its problem as the last error.
    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        ix: &Indexer,
        kind: &str,
        query: &str,
        r: &std::result::Result<usize, String>,
        started: Instant,
        source: &str,
        host: &str,
        data: Value,
    ) {
        let ms = started.elapsed().as_millis() as i64;
        let (ok, n, e) = match r {
            Ok(n) => (true, *n as i64, String::new()),
            Err(e) => (false, 0, e.clone()),
        };
        let _ = self.db.execute(
            "INSERT INTO indexer_history (indexer_id, at, kind, query, ok, ms, results, source, error, host, data) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            p![ix.id, crate::auth::now(), kind, query, ok, ms, n, source, e.clone(), host, data.to_string()],
        );
        if kind == "test" {
            let _ = self.db.execute("UPDATE indexers SET last_error=? WHERE id=?", p![e, ix.id]);
        }
    }

    /// Indexer `id`'s health now.
    fn read_status(&self, id: i64) -> Option<status::Status> {
        self.db.one("SELECT * FROM indexers WHERE id=?", p![id]).ok().flatten().map(|r| self.read(&r).status())
    }

    /// Saves indexer `id`'s health (and its last problem in words, when given).
    fn save_status(&self, id: i64, s: &status::Status, last_error: Option<&str>) {
        let _ = self.db.execute(
            "UPDATE indexers SET failures=?, disabled_until=?, initial_failure=?, most_recent_failure=?, last_error=COALESCE(?, last_error) WHERE id=?",
            p![
                s.escalation_level,
                s.disabled_till.unwrap_or(0.0),
                s.initial_failure.unwrap_or(0.0),
                s.most_recent_failure.unwrap_or(0.0),
                last_error.map(str::to_string),
                id
            ],
        );
    }

    /// A success for indexer `id`: its hold steps back once (`RecordSuccess`), and its last problem is cleared.
    pub fn record_success(&self, id: i64) {
        if id <= 0 {
            return;
        }
        let _g = self.status_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut s) = self.read_status(id) {
            if s.record_success() {
                self.save_status(id, &s, Some(""));
            } else {
                let _ = self.db.execute("UPDATE indexers SET last_error='' WHERE id=? AND last_error NOT LIKE 'Needs your help%'", p![id]);
            }
        }
    }

    /// A failure of indexer `id` (`why` in words): on hold for longer each time, at least an hour when the site
    /// asked Lantern to slow down (`RecordFailure`).
    pub fn record_failure(&self, id: i64, why: &str) {
        if id <= 0 {
            return;
        }
        let minimum = if is_rate_limited(why) { 3600.0 } else { 0.0 };
        let _g = self.status_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut s) = self.read_status(id) {
            s.record_failure(crate::auth::now(), status::start_time(), minimum, true);
            self.save_status(id, &s, Some(why));
        }
    }

    /// What a download client is told about seeding a torrent from indexer `indexer_id`
    /// (`SeedConfigProvider.GetSeedConfiguration`): none for indexer 0, an indexer that isn't there or a Usenet one.
    pub fn seed_configuration(&self, indexer_id: i64) -> Option<limits::SeedConfiguration> {
        if indexer_id == 0 {
            return None;
        }
        let ix = self.get(indexer_id).ok()?;
        (ix.protocol() == Protocol::Torrent).then(|| limits::SeedConfiguration::of(&ix.torrent_settings()))
    }

    /// When indexer `ix` is on hold until, if it is now (`GetBlockedProviders`).
    pub fn blocked_until(&self, ix: &Indexer) -> Option<f64> {
        let s = self.read_status(ix.id).unwrap_or_else(|| ix.status());
        s.is_disabled(crate::auth::now()).then(|| s.disabled_till.unwrap_or(0.0))
    }

    /// When indexer `id`'s history entries of `kinds` in the last `hours` hours were made.
    fn history_since(&self, id: i64, kinds: &[&str], hours: i64) -> Vec<f64> {
        let since = crate::auth::now() - hours as f64 * 3600.0;
        let list = kinds.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
        self.db
            .query(&format!("SELECT at FROM indexer_history WHERE indexer_id=? AND at>=? AND kind IN ({list})"), p![id, since])
            .map(|rows| rows.iter().map(|r| r.f("at")).collect())
            .unwrap_or_default()
    }

    /// Whether indexer `ix` has made as many searches as its query limit allows (`AtQueryLimit`).
    pub fn at_query_limit(&self, ix: &Indexer) -> bool {
        let b = ix.base_settings();
        ix.id > 0
            && b.query_limit.is_some()
            && limits::at_limit(b.query_limit, self.history_since(ix.id, &["query", "rss"], b.interval_hours()).len() as i64)
    }

    /// Whether indexer `ix` has made as many grabs as its grab limit allows (`AtDownloadLimit`).
    pub fn at_grab_limit(&self, ix: &Indexer) -> bool {
        let b = ix.base_settings();
        ix.id > 0
            && b.grab_limit.is_some()
            && limits::at_limit(b.grab_limit, self.history_since(ix.id, &["grab"], b.interval_hours()).len() as i64)
    }

    /// Seconds until indexer `ix` may search again (`CalculateRetryAfterQueryLimit`).
    pub fn retry_after_query_limit(&self, ix: &Indexer) -> i64 {
        let b = ix.base_settings();
        let Some(l) = b.query_limit else { return 0 };
        limits::retry_after(&self.history_since(ix.id, &["query", "rss"], b.interval_hours()), l, b.interval_hours(), crate::auth::now())
    }

    /// Seconds until indexer `ix` may grab again (`CalculateRetryAfterDownloadLimit`).
    pub fn retry_after_grab_limit(&self, ix: &Indexer) -> i64 {
        let b = ix.base_settings();
        let Some(l) = b.grab_limit else { return 0 };
        limits::retry_after(&self.history_since(ix.id, &["grab"], b.interval_hours()), l, b.interval_hours(), crate::auth::now())
    }

    /// Indexer `ix`'s privacy: its definition's, its preset's, else private.
    pub fn privacy(&self, ix: &Indexer) -> String {
        if ix.kind == "cardigann" {
            if let Some(d) = self.library.get(&ix.definition) {
                return d.kind.clone();
            }
        } else if let Some(p) = newznab::preset(&ix.definition) {
            return p.privacy.to_string();
        }
        "private".into()
    }

    /// Searches one indexer the way Prowlarr's `HttpIndexerBase.Fetch` does: nothing for a later page when it
    /// can't page, nothing for a parameter its capabilities don't take; else it asks, notes the query in the
    /// history and its health, keeps the releases apps can use (with a title; outside an interactive search also a
    /// size and categories) and fills in what every release carries (`CleanupReleases`).
    async fn fetch(
        &self,
        ix: &Indexer,
        caps: &Capabilities,
        q: &Query,
        source: &str,
        host: &str,
        interactive: bool,
    ) -> std::result::Result<Vec<Release>, String> {
        if !ix.supports_pagination() && q.offset > 0 {
            return Ok(Vec::new());
        }
        if !supports_params(caps, q) {
            return Ok(Vec::new());
        }
        let started = Instant::now();
        let client = self.client(ix).map_err(|e| e.detail)?;
        let r = match &*client {
            Client::Site(site) => {
                if !site.supports(q) {
                    return Ok(Vec::new());
                }
                let r = site.search(q).await;
                self.keep_cookies(ix, site);
                r
            }
            Client::Api(api) => api.search(q).await,
        };
        let words = if q.rss { "(newest)".to_string() } else { q.search_terms() };
        let data = json!({
            "Limit": (q.limit > 0).then_some(q.limit), "Offset": (q.offset > 0).then_some(q.offset), "QueryType": q.kind.torznab(),
            "Categories": q.categories.iter().map(u32::to_string).collect::<Vec<_>>().join(","), "Source": source, "Host": host,
        });
        self.record(
            ix,
            if q.rss { "rss" } else { "query" },
            &words,
            &r.as_ref().map(Vec::len).map_err(Clone::clone),
            started,
            source,
            host,
            data,
        );
        match &r {
            Ok(_) => self.record_success(ix.id),
            Err(e) => self.record_failure(ix.id, e),
        }
        let list = r?
            .into_iter()
            .filter(|rel| !rel.title.trim().is_empty() && (interactive || (rel.size.is_some() && !rel.categories.is_empty())))
            .collect();
        Ok(self.cleanup(ix, list))
    }

    /// What every release of `ix` carries: a guid, the indexer's id, name, protocol, privacy and priority, and the
    /// flags its factors mean; one per guid (`CleanupReleases`).
    fn cleanup(&self, ix: &Indexer, list: Vec<Release>) -> Vec<Release> {
        let privacy = self.privacy(ix);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for mut r in list {
            if r.guid.trim().is_empty() {
                r.guid = if !r.link.trim().is_empty() {
                    r.link.clone()
                } else if ix.protocol() == Protocol::Torrent && !r.magnet.trim().is_empty() {
                    r.magnet.clone()
                } else {
                    r.details.clone()
                };
            }
            r.indexer_id = ix.id;
            r.indexer = ix.name.clone();
            r.protocol = ix.protocol();
            r.indexer_privacy = privacy.clone();
            r.indexer_priority = ix.priority;
            let mut flags = std::mem::take(&mut r.flags);
            let mut flag = |f: &str| {
                if !flags.iter().any(|x| x == f) {
                    flags.push(f.to_string());
                }
            };
            if ix.kind == "cardigann" && r.description.starts_with("Internal") {
                flag("internal");
            }
            if r.protocol == Protocol::Torrent {
                match r.download_factor {
                    Some(0.0) => flag("freeleech"),
                    Some(0.5) => flag("halfleech"),
                    _ => {}
                }
                match r.upload_factor {
                    Some(0.0) => flag("neutralleech"),
                    Some(2.0) => flag("doubleupload"),
                    _ => {}
                }
                if r.scene == Some(true) {
                    flag("scene");
                }
            }
            r.flags = flags;
            if seen.insert(r.guid.clone()) {
                out.push(r);
            }
        }
        out
    }

    /// Searches one indexer for [`Indexers::dispatch`]: nothing at its query limit; else its releases in the
    /// categories asked for (widened to their sub-categories; ones without categories stay) and in the age and size
    /// asked for (`DispatchIndexer`).
    async fn dispatch_one(
        &self,
        ix: &Indexer,
        q: &Query,
        source: &str,
        host: &str,
        interactive: bool,
    ) -> std::result::Result<Vec<Release>, String> {
        if self.at_query_limit(ix) {
            return Ok(Vec::new());
        }
        let caps = self.capabilities(ix);
        let mut list = self.fetch(ix, &caps, q, source, host, interactive).await?;
        if !q.categories.is_empty() {
            let wanted = caps.categories.expand(&q.categories.iter().map(|c| *c as i64).collect::<Vec<_>>(), false);
            list.retain(|r| r.categories.is_empty() || r.categories.iter().any(|c| wanted.contains(&(*c as i64))));
        }
        let now = chrono::Utc::now();
        let date = |r: &Release| r.date.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
        if q.min_age > 0 {
            let cutoff = now - chrono::Duration::days(q.min_age);
            list.retain(|r| date(r) <= cutoff);
        }
        if q.max_age > 0 {
            let cutoff = now - chrono::Duration::days(q.max_age);
            list.retain(|r| date(r) >= cutoff);
        }
        if q.min_size > 0 {
            list.retain(|r| r.size.is_some_and(|s| s as i64 >= q.min_size));
        }
        if q.max_size > 0 {
            list.retain(|r| r.size.is_some_and(|s| s as i64 <= q.max_size));
        }
        Ok(list)
    }

    /// Searches the enabled indexers that aren't on hold (`ids` narrows them: an indexer's id, `-1` every Usenet
    /// one, `-2` every torrent one; empty: all), only those that have one of the categories asked for, all at
    /// once; a release two indexers found is kept from the one with the lower priority number. An interactive
    /// search (people, not apps) keeps releases without a size or categories (`ReleaseSearchService.Dispatch`).
    pub async fn dispatch(&self, ids: &[i64], q: &Query, source: &str, host: &str, interactive: bool) -> Result<Searched> {
        let now = crate::auth::now();
        let mut list: Vec<Indexer> = self.list()?.into_iter().filter(|ix| ix.enabled && !ix.status().is_disabled(now)).collect();
        if !ids.is_empty() {
            list.retain(|ix| {
                ids.contains(&ix.id)
                    || (ids.contains(&-1) && ix.protocol() == Protocol::Usenet)
                    || (ids.contains(&-2) && ix.protocol() == Protocol::Torrent)
            });
            if interactive && list.is_empty() {
                return Err(err(400, "Search failed due to all selected indexers being unavailable"));
            }
        }
        self.refresh_caps_of(&list).await;
        if !q.categories.is_empty() {
            let cats: Vec<i64> = q.categories.iter().map(|c| *c as i64).collect();
            list.retain(|ix| !self.capabilities(ix).categories.supported(&cats).is_empty());
            if list.is_empty() {
                return Ok(Searched::default());
            }
        }
        let jobs = list.iter().map(|ix| async move { (ix.name.clone(), self.dispatch_one(ix, q, source, host, interactive).await) });
        let done = futures_util::future::join_all(jobs).await;
        let mut found = Vec::new();
        let mut failed = Vec::new();
        for (name, r) in done {
            match r {
                Ok(list) => found.extend(list),
                Err(e) => failed.push((name, e)),
            }
        }
        Ok(Searched { releases: dedupe(found), failed })
    }

    /// Searches one indexer, as an RSS source does: its releases or the problem in words.
    pub async fn search_one(&self, ix: &Indexer, q: &Query, source: &str) -> std::result::Result<Vec<Release>, String> {
        self.refresh_caps_of(std::slice::from_ref(ix)).await;
        let caps = self.capabilities(ix);
        let list = self.fetch(ix, &caps, q, source, "", true).await?;
        if q.categories.is_empty() {
            return Ok(list);
        }
        let wanted = caps.categories.expand(&q.categories.iter().map(|c| *c as i64).collect::<Vec<_>>(), false);
        Ok(list.into_iter().filter(|r| r.categories.is_empty() || r.categories.iter().any(|c| wanted.contains(&(*c as i64)))).collect())
    }

    /// Lantern's own search (its pages and the download hub): every enabled indexer in `ids` (all when empty), as
    /// [`Indexers::dispatch`] does for a person, then newest first with `offset` and `limit` applied to all of them
    /// together; the releases and the indexers that failed with why.
    pub async fn search(&self, ids: &[i64], q: &Query, source: &str) -> Result<(Vec<Release>, Vec<(String, String)>)> {
        let mut one = q.clone();
        one.offset = 0;
        let Searched { releases, failed } = self.dispatch(ids, &one, source, "", true).await?;
        let mut out = releases;
        out.sort_by_key(|r| std::cmp::Reverse(r.date));
        if q.offset > 0 {
            out = out.into_iter().skip(q.offset).collect();
        }
        if q.limit > 0 {
            out.truncate(q.limit);
        }
        Ok((out, failed))
    }

    /// Fetches a release's file (or magnet) from indexer `ix` for `source` on `host`
    /// (`DownloadService.DownloadReport`): notes the grab in the history either way; a success steps the hold back,
    /// a failure other than the release being gone holds the indexer.
    pub async fn download(
        &self,
        ix: &Indexer,
        link: &str,
        title: &str,
        source: &str,
        host: &str,
    ) -> std::result::Result<Grabbed, GrabError> {
        let started = Instant::now();
        // a magnet link is handed back as it is (`HttpIndexerBase.Download` returns its text)
        let r = match self.client(ix) {
            _ if link.starts_with("magnet:") => Ok(Grabbed::Magnet(link.to_string())),
            Err(e) => Err(e.detail),
            Ok(client) => match &*client {
                Client::Site(site) => {
                    let r = site.grab(link).await;
                    self.keep_cookies(ix, site);
                    r
                }
                Client::Api(api) => api.grab(link).await.map(Grabbed::File),
            },
        };
        let data = json!({ "Source": source, "Host": host, "GrabMethod": "Proxy", "GrabTitle": title, "Url": http::strip_secrets(link) });
        self.record(ix, "grab", title, &r.as_ref().map(|_| 1).map_err(Clone::clone), started, source, host, data);
        match r {
            Ok(g) => {
                self.record_success(ix.id);
                Ok(g)
            }
            Err(e) if e.contains("(404)") => Err(GrabError::Unavailable(e)),
            Err(e) => {
                self.record_failure(ix.id, &e);
                if is_rate_limited(&e) {
                    Err(GrabError::TooManyRequests(e))
                } else {
                    Err(GrabError::Failed(e))
                }
            }
        }
    }

    /// Notes that an app was sent to the site for a release's file (`DownloadService.RecordRedirect`).
    pub fn record_redirect(&self, ix: &Indexer, link: &str, title: &str, source: &str, host: &str) {
        let data =
            json!({ "Source": source, "Host": host, "GrabMethod": "Redirect", "GrabTitle": title, "Url": http::strip_secrets(link) });
        self.record(ix, "grab", title, &Ok(1), Instant::now(), source, host, data);
    }

    /// The file (or magnet) of a release, from indexer `id` (the download hub and Lantern's pages).
    pub async fn grab(&self, id: i64, link: &str, title: &str, source: &str) -> Result<Grabbed> {
        let ix = self.get(id)?;
        self.download(&ix, link, title, source, "").await.map_err(|e| err(502, e.message().to_string()))
    }

    /// What makes a Newznab/Torznab API's capabilities answer differ (Prowlarr keys it by the settings).
    fn caps_key(ix: &Indexer) -> String {
        format!("{}|{}|{:?}", ix.kind, ix.base_url, ix.settings)
    }

    /// Asks a Newznab/Torznab indexer's API for its capabilities (`t=caps`, through its proxies) unless an answer
    /// for the same settings is younger than 7 days (`NewznabCapabilitiesProvider.GetCapabilities`); the answer is
    /// kept with the indexer too. Nothing to do for a Cardigann site. The problem in words when the API can't be
    /// asked or its answer can't be read.
    pub async fn refresh_caps(&self, ix: &Indexer) -> std::result::Result<(), String> {
        if ix.kind == "cardigann" || ix.id <= 0 {
            return Ok(());
        }
        let key = Self::caps_key(ix);
        let now = crate::auth::now();
        if self.caps.lock().unwrap_or_else(|e| e.into_inner()).fresh(&key, now).is_some() {
            return Ok(());
        }
        let client = self.client(ix).map_err(|e| e.detail)?;
        let Client::Api(api) = &*client else { return Ok(()) };
        let text = api.caps_text().await?;
        newznab_caps::parse(&text).map_err(|e| format!("{}: {e}", ix.name))?;
        self.caps.lock().unwrap_or_else(|e| e.into_inner()).put(&key, now, &text);
        if text != ix.caps_xml {
            let _ = self.db.execute("UPDATE indexers SET capabilities=? WHERE id=?", p![text, ix.id]);
        }
        Ok(())
    }

    /// [`Indexers::refresh_caps`] for each Newznab/Torznab indexer of `list` at once; a problem is only logged (the
    /// indexer keeps the capabilities it had).
    pub async fn refresh_caps_of(&self, list: &[Indexer]) {
        let jobs = list.iter().filter(|ix| ix.kind != "cardigann").map(|ix| async move {
            if let Err(e) = self.refresh_caps(ix).await {
                tracing::debug!("Failed to get Newznab API capabilities from {}: {e}", ix.name);
            }
        });
        futures_util::future::join_all(jobs).await;
    }

    /// What indexer `ix` can search for and its categories (`GetCapabilities`): a site's from its definition; a
    /// Newznab or Torznab API's from its own capabilities answer (see [`Indexers::refresh_caps`]), or, before it
    /// has given one, every kind of search with the categories of its `categories` setting.
    pub fn capabilities(&self, ix: &Indexer) -> Capabilities {
        if ix.kind == "cardigann" {
            return match self.library.get(&ix.definition).map(|d| Capabilities::from_definition(&d)) {
                Some(Ok(c)) => c,
                Some(Err(e)) => {
                    tracing::warn!("{}: its definition's capabilities can't be read: {e}", ix.name);
                    Capabilities::default()
                }
                None => Capabilities::default(),
            };
        }
        let kept = self.caps.lock().unwrap_or_else(|e| e.into_inner()).any(&Self::caps_key(ix)).map(str::to_string);
        for text in kept.iter().chain(std::iter::once(&ix.caps_xml)) {
            if text.is_empty() {
                continue;
            }
            if let Ok(c) = newznab_caps::parse(text) {
                return c;
            }
        }
        let mut c = Capabilities {
            tv_params: vec!["q", "season", "ep", "imdbid", "tvdbid"],
            movie_params: vec!["q", "imdbid", "tmdbid"],
            music_params: vec!["q", "artist", "album"],
            book_params: vec!["q", "author", "title"],
            ..Capabilities::default()
        };
        for (id, name) in categories_of(ix.settings.get("categories").map(String::as_str).unwrap_or("")) {
            c.categories.add_mapping(&id.to_string(), &Category::new(id, &name), None);
        }
        c
    }

    /// The capabilities of every enabled indexer together (all indexers at once, a Lantern extra).
    pub fn capabilities_all(&self) -> Result<Capabilities> {
        let mut c = Capabilities::default();
        for ix in self.list()?.into_iter().filter(|i| i.enabled) {
            c.concat(&self.capabilities(&ix));
        }
        if c.categories.list(false).is_empty() {
            for (id, name) in categories::STANDARD {
                c.categories.add_mapping(&id.to_string(), &Category::new(*id, name), None);
            }
        }
        Ok(c)
    }

    /// The JSON the web app and the API get for an indexer: secrets masked, with its health.
    pub fn public(&self, ix: &Indexer) -> Value {
        let def = (ix.kind == "cardigann").then(|| self.library.get(&ix.definition)).flatten();
        let secret = |k: &str| match &def {
            Some(d) => d.settings.iter().any(|s| s.name == k && s.is_secret()),
            None => matches!(k, "apikey" | "password" | "cookie"),
        };
        let settings: Map<String, Value> =
            ix.settings.iter().map(|(k, v)| (k.clone(), if secret(k) && !v.is_empty() { json!(MASK) } else { json!(v) })).collect();
        let now = crate::auth::now();
        json!({
            "id": ix.id,
            "kind": ix.kind,
            "definition": ix.definition,
            "name": ix.name,
            "base_url": if ix.base_url.is_empty() { def.as_ref().and_then(|d| d.links.first().cloned()).unwrap_or_default() } else { ix.base_url.clone() },
            "links": def.as_ref().map(|d| d.links.clone()).unwrap_or_default(),
            "settings": settings,
            "enabled": ix.enabled,
            "priority": ix.priority,
            "tags": ix.tags,
            "protocol": ix.protocol(),
            "privacy": def.as_ref().map(|d| d.kind.clone()).unwrap_or_else(|| "private".into()),
            "language": def.as_ref().map(|d| d.language.clone()).unwrap_or_default(),
            "description": def.as_ref().map(|d| d.description.clone()).unwrap_or_default(),
            "failures": ix.failures,
            "on_hold_until": if ix.disabled_until > now { json!(ix.disabled_until) } else { Value::Null },
            "last_error": ix.last_error,
            "added": ix.added,
            "app_profile_id": ix.app_profile_id,
            "download_client_id": ix.download_client_id,
        })
    }

    /// Per indexer: queries, grabs, failures, average answer time and results, over the last `days` days.
    pub fn stats(&self, days: f64) -> Result<Vec<Value>> {
        let since = crate::auth::now() - days * 86400.0;
        let rows = self.db.query(
            "SELECT i.id, i.name,
                SUM(h.kind IN ('query','rss')) queries, SUM(h.kind='grab' AND h.ok) grabs, SUM(NOT h.ok) failures,
                CAST(AVG(CASE WHEN h.kind IN ('query','rss') THEN h.ms END) AS INTEGER) avg_ms,
                SUM(CASE WHEN h.kind IN ('query','rss') THEN h.results ELSE 0 END) results
             FROM indexers i LEFT JOIN indexer_history h ON h.indexer_id=i.id AND h.at>=?
             GROUP BY i.id ORDER BY i.name COLLATE NOCASE",
            p![since],
        )?;
        Ok(rows
            .iter()
            .map(|r| {
                json!({ "id": r.i("id"), "name": r.s("name"), "queries": r.i("queries"), "grabs": r.i("grabs"), "failures": r.i("failures"), "avg_ms": r.i("avg_ms"), "results": r.i("results") })
            })
            .collect())
    }

    /// The last `limit` history entries (of indexer `id`, or all).
    pub fn history(&self, id: Option<i64>, limit: i64) -> Result<Vec<Value>> {
        let rows = match id {
            Some(id) => self.db.query("SELECT h.*, i.name FROM indexer_history h LEFT JOIN indexers i ON i.id=h.indexer_id WHERE h.indexer_id=? ORDER BY h.at DESC LIMIT ?", p![id, limit])?,
            None => self.db.query("SELECT h.*, i.name FROM indexer_history h LEFT JOIN indexers i ON i.id=h.indexer_id ORDER BY h.at DESC LIMIT ?", p![limit])?,
        };
        Ok(rows
            .iter()
            .map(|r| {
                json!({ "indexer_id": r.i("indexer_id"), "indexer": r.s("name"), "at": r.f("at"), "kind": r.s("kind"), "query": r.s("query"), "ok": r.b("ok"), "ms": r.i("ms"), "results": r.i("results"), "source": r.s("source"), "error": r.s("error") })
            })
            .collect())
    }

    /// Indexers that are on hold after failures: (name, why).
    pub fn failing(&self) -> Result<Vec<(String, String)>> {
        let now = crate::auth::now();
        Ok(self.list()?.into_iter().filter(|i| i.enabled && i.status().is_disabled(now)).map(|i| (i.name, i.last_error)).collect())
    }
}

/// Whether a search uses only parameters `caps` takes for its kind of search (`HttpIndexerBase.Fetch`).
pub fn supports_params(caps: &Capabilities, q: &Query) -> bool {
    let has = |v: &str| !v.trim().is_empty();
    let uses = |p: &str| match p {
        "imdbid" => has(&q.imdbid),
        "tmdbid" => has(&q.tmdbid),
        "tvdbid" => has(&q.tvdbid),
        "rid" => has(&q.rid),
        "tvmazeid" => has(&q.tvmazeid),
        "traktid" => has(&q.traktid),
        "doubanid" => has(&q.doubanid),
        "genre" => has(&q.genre),
        "year" => has(&q.year),
        "album" => has(&q.album),
        "artist" => has(&q.artist),
        "label" => has(&q.label),
        "track" => has(&q.track),
        "title" => has(&q.title),
        "author" => has(&q.author),
        "publisher" => has(&q.publisher),
        _ => false,
    };
    let (params, checked): (&Vec<&'static str>, &[&str]) = match q.kind {
        SearchKind::TvSearch => {
            (&caps.tv_params, &["imdbid", "tvdbid", "rid", "tvmazeid", "traktid", "tmdbid", "doubanid", "genre", "year"])
        }
        SearchKind::MovieSearch => (&caps.movie_params, &["imdbid", "tmdbid", "traktid", "doubanid", "genre", "year"]),
        SearchKind::MusicSearch => (&caps.music_params, &["album", "artist", "label", "track", "genre", "year"]),
        SearchKind::BookSearch => (&caps.book_params, &["title", "author", "publisher", "genre", "year"]),
        SearchKind::Search => return true,
    };
    checked.iter().all(|p| !uses(p) || params.contains(p))
}

/// One release per guid, from the indexer with the lowest priority number, in the order first found
/// (`DeDupeReleases`).
pub fn dedupe(list: Vec<Release>) -> Vec<Release> {
    let mut order: Vec<String> = Vec::new();
    let mut best: HashMap<String, Release> = HashMap::new();
    for r in list {
        match best.get(&r.guid) {
            None => {
                order.push(r.guid.clone());
                best.insert(r.guid.clone(), r);
            }
            Some(b) if r.indexer_priority < b.indexer_priority => {
                best.insert(r.guid.clone(), r);
            }
            Some(_) => {}
        }
    }
    order.into_iter().filter_map(|g| best.remove(&g)).collect()
}

/// The whole number under the first of `keys` in `body` (a number or its text), when there is one.
fn int_of(body: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| match body.get(*k) {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// Whether row `id` is in the first of `tables` that exists; `None` when none of them does (the part of Lantern
/// that keeps them isn't there yet).
fn exists_in(db: &SqliteDb, tables: &[&str], id: i64) -> Option<bool> {
    tables.iter().find_map(|t| db.one(&format!("SELECT id FROM {t} WHERE id=?"), p![id]).ok().map(|r| r.is_some()))
}

/// The query and grab limits in `settings` must be more than zero (`IndexerCommonSettingsValidator`).
fn check_limits(settings: &BTreeMap<String, String>) -> Result<()> {
    let (errors, _) = limits::validate(settings);
    match errors.first() {
        Some((k, m)) => {
            let label = if *k == limits::QUERY_LIMIT { "Query Limit" } else { "Grab Limit" };
            Err(err(400, format!("{label}: {m}")))
        }
        None => Ok(()),
    }
}

/// Fills what a Usenet preset knows into `settings` where the person left it empty: the API path and categories.
pub fn fill_from_preset(settings: &mut BTreeMap<String, String>, p: &newznab::Preset) {
    if settings.get("api_path").is_none_or(|v| v.trim().is_empty()) {
        settings.insert("api_path".into(), p.api_path.into());
    }
    if settings.get("categories").is_none_or(|v| v.trim().is_empty()) {
        settings.insert("categories".into(), p.categories.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    }
}

/// The categories an API indexer says it has (`2000,5000`), each with its sub-categories; every standard
/// category when the list is empty or has none Lantern knows.
pub fn categories_of(list: &str) -> Vec<(u32, String)> {
    let picked: Vec<u32> = list.split(',').filter_map(|c| c.trim().parse().ok()).collect();
    let out: Vec<(u32, String)> = categories::STANDARD
        .iter()
        .filter(|(id, _)| picked.iter().any(|p| categories::matches(*id, *p)))
        .map(|(id, n)| (*id, n.to_string()))
        .collect();
    if out.is_empty() {
        categories::STANDARD.iter().map(|(id, n)| (*id, n.to_string())).collect()
    } else {
        out
    }
}

/// Settings from a request: text values; a masked or empty secret keeps `old`'s.
fn clean_settings(v: Option<&Value>, old: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = old.clone();
    if let Some(Value::Object(m)) = v {
        for (k, v) in m {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Null => String::new(),
                Value::Array(a) => {
                    a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(",")
                }
                other => other.to_string(),
            };
            if text == MASK {
                continue;
            }
            out.insert(k.clone(), text);
        }
    }
    out
}

/// Signs and checks the download links Lantern hands out, so its download address can't be used to fetch
/// anything else (`/{id}/download?link=…&sig=…`).
#[derive(Clone)]
pub struct LinkSigner {
    key: Arc<Vec<u8>>,
}

impl LinkSigner {
    /// A signer with the secret kept in `<data>/link.key` (made at the first start).
    pub fn open(data_dir: &std::path::Path) -> anyhow::Result<LinkSigner> {
        let path = data_dir.join("link.key");
        let key = match std::fs::read(&path) {
            Ok(k) if k.len() >= 32 => k,
            _ => {
                use rand::Rng;
                let mut k = vec![0u8; 32];
                rand::rng().fill(&mut k[..]);
                std::fs::write(&path, &k).map_err(|e| anyhow::anyhow!("The link key can't be written in the data folder: {e}"))?;
                k
            }
        };
        Ok(LinkSigner { key: Arc::new(key) })
    }

    /// A signer with a fixed key (tests).
    pub fn with_key(key: &[u8]) -> LinkSigner {
        LinkSigner { key: Arc::new(key.to_vec()) }
    }

    fn mac(&self, indexer: i64, link: &str) -> String {
        use hmac::Mac;
        let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(&self.key).expect("any key length works");
        m.update(format!("{indexer}\n{link}").as_bytes());
        hex::encode(&m.finalize().into_bytes()[..16])
    }

    /// The `link` and `sig` query values for a release's link.
    pub fn sign(&self, indexer: i64, link: &str) -> (String, String) {
        use base64::Engine;
        (base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(link), self.mac(indexer, link))
    }

    /// The link back from `link` and `sig`, when the signature is right.
    pub fn open_link(&self, indexer: i64, link: &str, sig: &str) -> Option<String> {
        use base64::Engine;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(link.trim()).ok()?;
        let s = String::from_utf8(raw).ok()?;
        let want = self.mac(indexer, &s);
        (want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0).then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_signed_and_masks_keep_secrets() {
        let s = LinkSigner::with_key(b"k");
        let (l, sig) = s.sign(3, "https://site.test/dl/1?passkey=x");
        assert_eq!(s.open_link(3, &l, &sig).as_deref(), Some("https://site.test/dl/1?passkey=x"));
        assert_eq!(s.open_link(4, &l, &sig), None);
        let mut old = BTreeMap::new();
        old.insert("password".to_string(), "secret".to_string());
        let n = clean_settings(Some(&json!({"password": MASK, "username": "me", "freeleech": true})), &old);
        assert_eq!(n.get("password").map(String::as_str), Some("secret"));
        assert_eq!(n.get("freeleech").map(String::as_str), Some("true"));
    }
}
