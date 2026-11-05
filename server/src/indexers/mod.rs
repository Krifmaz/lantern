//! Indexers (goal 1): torrent trackers from the Cardigann definitions and Usenet/torrent indexers that speak
//! Newznab/Torznab, behind one search. Lantern answers apps through Torznab/Newznab (per indexer and all of them
//! at once) and the Prowlarr API ([`api`]).
//!
//! | Module | What |
//! | --- | --- |
//! | [`definition`], [`template`], [`filters`], [`json_path`] | reading the Cardigann definitions |
//! | [`cardigann`], [`newznab`], [`http`] | talking to the sites |
//! | [`library`] | the definitions on disk and their updates |
//! | [`torznab`], [`api`] | what apps get |
//!
//! Each indexer is a row of `indexers`; its settings' secrets are never sent back by the API.

pub mod api;
pub mod cardigann;
pub mod categories;
pub mod definition;
pub mod filters;
pub mod http;
pub mod json_path;
pub mod library;
pub mod newznab;
pub mod release;
pub mod template;
pub mod torznab;

use cardigann::{Cardigann, Grabbed};
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use library::Library;
use newznab::Newznab;
use release::{Protocol, Query, Release};
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
    .map_err(|e| anyhow::anyhow!("The indexer tables can't be made: {}", e.detail))
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
    /// The session cookies kept from the last sign-in.
    pub cookies: String,
    /// Failures in a row.
    pub failures: i64,
    /// Not searched until this time (after failures).
    pub disabled_until: f64,
    /// The last failure, in words.
    pub last_error: String,
    /// When it was added.
    pub added: f64,
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
            cookies,
            failures: r.i("failures"),
            disabled_until: r.f("disabled_until"),
            last_error: r.s("last_error"),
            added: r.f("added"),
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
}

impl Indexers {
    /// The indexers on `db` with `library`.
    pub fn new(db: SqliteDb, library: Library) -> Indexers {
        Indexers { db, library, clients: Default::default(), settings: None, solver: None, vault: None }
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
            if let Some((_, n, url)) = newznab::PRESETS.iter().find(|(id, _, _)| *id == definition) {
                if name.is_empty() {
                    name = n.to_string();
                }
                if base_url.is_empty() {
                    base_url = url.to_string();
                }
            }
            if base_url.is_empty() {
                return Err(err(400, "Enter the indexer's address (its API without /api)"));
            }
        }
        if name.is_empty() {
            return Err(err(400, "Enter a name for the indexer"));
        }
        let settings = clean_settings(body.get("settings"), &BTreeMap::new());
        let settings = self.seal(SETTINGS_AAD, &serde_json::to_string(&settings).unwrap_or_default())?;
        let id = self.db.execute(
            "INSERT INTO indexers (kind, definition, name, base_url, settings, enabled, priority, tags, added) VALUES (?,?,?,?,?,?,?,?,?)",
            p![
                kind,
                definition,
                name,
                base_url,
                settings,
                body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                body.get("priority").and_then(Value::as_i64).unwrap_or(25).clamp(1, 50),
                body.get("tags").and_then(Value::as_str).unwrap_or(""),
                crate::auth::now()
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
        self.db.execute(
            "UPDATE indexers SET name=?, base_url=?, settings=?, enabled=?, priority=?, tags=?, failures=CASE WHEN ? THEN 0 ELSE failures END, disabled_until=CASE WHEN ? THEN 0 ELSE disabled_until END WHERE id=?",
            p![
                name,
                text("base_url", &cur.base_url),
                self.seal(SETTINGS_AAD, &serde_json::to_string(&settings).unwrap_or_default())?,
                body.get("enabled").and_then(Value::as_bool).unwrap_or(cur.enabled),
                body.get("priority").and_then(Value::as_i64).unwrap_or(cur.priority).clamp(1, 50),
                text("tags", &cur.tags),
                body.get("settings").is_some(),
                body.get("settings").is_some() || body.get("enabled") == Some(&json!(true)),
                id
            ],
        )?;
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
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
        let proxy = self.settings.as_ref().map(|s| s.str("site_proxy")).unwrap_or_default();
        let fp = format!("{}|{}|{:?}|{proxy}", ix.base_url, ix.definition, ix.settings);
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
                let http = http::Http::new(&ix.name, delay, &def.encoding, Some(&proxy)).with_solver(self.solver.clone());
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
                    http: http::Http::new(&ix.name, delay, "UTF-8", Some(&proxy)).with_solver(self.solver.clone()),
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
        self.record(&ix, "test", "", &r.as_ref().map(|n| *n).map_err(Clone::clone), started, "");
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

    /// Notes a query, grab or test in the history, and the indexer's health: failures in a row put it on hold for
    /// longer each time (5 minutes doubling, at most a day); a success clears that.
    fn record(&self, ix: &Indexer, kind: &str, query: &str, r: &std::result::Result<usize, String>, started: Instant, source: &str) {
        let ms = started.elapsed().as_millis() as i64;
        let (ok, n, e) = match r {
            Ok(n) => (true, *n as i64, String::new()),
            Err(e) => (false, 0, e.clone()),
        };
        let _ = self.db.execute(
            "INSERT INTO indexer_history (indexer_id, at, kind, query, ok, ms, results, source, error) VALUES (?,?,?,?,?,?,?,?,?)",
            p![ix.id, crate::auth::now(), kind, query, ok, ms, n, source, e.clone()],
        );
        if ok {
            if ix.failures > 0 || !ix.last_error.is_empty() {
                let _ = self.db.execute("UPDATE indexers SET failures=0, disabled_until=0, last_error='' WHERE id=?", p![ix.id]);
            }
        } else if kind != "test" {
            let failures = ix.failures + 1;
            let hold = if failures >= 2 { (300.0 * 2f64.powi((failures - 2).min(9) as i32)).min(86400.0) } else { 0.0 };
            let until = if hold > 0.0 { crate::auth::now() + hold } else { 0.0 };
            let _ =
                self.db.execute("UPDATE indexers SET failures=?, disabled_until=?, last_error=? WHERE id=?", p![failures, until, e, ix.id]);
        } else {
            let _ = self.db.execute("UPDATE indexers SET last_error=? WHERE id=?", p![e, ix.id]);
        }
    }

    /// Searches one indexer (`Err` in words when it failed).
    pub async fn search_one(&self, ix: &Indexer, q: &Query, source: &str) -> std::result::Result<Vec<Release>, String> {
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
        let r = r.map(|mut list| {
            for rel in &mut list {
                rel.indexer_id = ix.id;
                rel.indexer = ix.name.clone();
                rel.protocol = ix.protocol();
            }
            if !q.categories.is_empty() {
                list.retain(|rel| {
                    rel.categories.is_empty() || rel.categories.iter().any(|c| q.categories.iter().any(|w| categories::matches(*c, *w)))
                });
            }
            list
        });
        let words = if q.rss { "(newest)".to_string() } else { q.search_terms() };
        self.record(ix, if q.rss { "rss" } else { "query" }, &words, &r.as_ref().map(Vec::len).map_err(Clone::clone), started, source);
        r
    }

    /// Searches every enabled indexer in `ids` (all when empty) at once; the releases, newest first, and the
    /// indexers that failed with why.
    pub async fn search(&self, ids: &[i64], q: &Query, source: &str) -> Result<(Vec<Release>, Vec<(String, String)>)> {
        let now = crate::auth::now();
        let list: Vec<Indexer> = self
            .list()?
            .into_iter()
            .filter(|ix| ix.enabled && (ids.is_empty() || ids.contains(&ix.id)) && ix.disabled_until <= now)
            .collect();
        let jobs = list.iter().map(|ix| async move { (ix.name.clone(), self.search_one(ix, q, source).await) });
        let done = futures_util::future::join_all(jobs).await;
        let mut out = Vec::new();
        let mut failed = Vec::new();
        for (name, r) in done {
            match r {
                Ok(list) => out.extend(list),
                Err(e) => failed.push((name, e)),
            }
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.date));
        if q.offset > 0 {
            out = out.into_iter().skip(q.offset).collect();
        }
        if q.limit > 0 {
            out.truncate(q.limit);
        }
        Ok((out, failed))
    }

    /// The file (or magnet) of a release, from indexer `id`.
    pub async fn grab(&self, id: i64, link: &str, title: &str, source: &str) -> Result<Grabbed> {
        let ix = self.get(id)?;
        let started = Instant::now();
        let client = self.client(&ix)?;
        let r = match &*client {
            Client::Site(site) => {
                let r = site.grab(link).await;
                self.keep_cookies(&ix, site);
                r
            }
            Client::Api(api) => api.grab(link).await.map(Grabbed::File),
        };
        self.record(&ix, "grab", title, &r.as_ref().map(|_| 1).map_err(Clone::clone), started, source);
        r.map_err(|e| err(502, e))
    }

    /// The Torznab capabilities of indexer `ix`.
    pub fn caps(&self, ix: &Indexer) -> torznab::Caps {
        let mut c = torznab::Caps { title: ix.name.clone(), ..Default::default() };
        if ix.kind == "cardigann" {
            if let Some(def) = self.library.get(&ix.definition) {
                c.modes = def.caps.modes.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                for m in def.category_map() {
                    if let Some(id) = categories::id_of(&m.cat) {
                        c.categories.push((id, categories::name_of(id).unwrap_or(&m.cat).to_string()));
                    }
                    if let Ok(n) = m.id.parse::<u32>() {
                        c.categories.push((100000 + n, m.desc.clone()));
                    }
                }
                return c;
            }
        }
        c.modes = vec![
            ("search".into(), vec!["q".into()]),
            ("tv-search".into(), vec!["q".into(), "season".into(), "ep".into(), "imdbid".into(), "tvdbid".into()]),
            ("movie-search".into(), vec!["q".into(), "imdbid".into(), "tmdbid".into()]),
            ("music-search".into(), vec!["q".into(), "artist".into(), "album".into()]),
            ("book-search".into(), vec!["q".into(), "author".into(), "title".into()]),
        ];
        c.categories = categories::STANDARD.iter().map(|(id, n)| (*id, n.to_string())).collect();
        c
    }

    /// The capabilities of all indexers together (the aggregate endpoint).
    pub fn caps_all(&self) -> Result<torznab::Caps> {
        let mut c = torznab::Caps { title: "Lantern (all indexers)".into(), ..Default::default() };
        let mut modes: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for ix in self.list()?.into_iter().filter(|i| i.enabled) {
            let one = self.caps(&ix);
            for (m, ps) in one.modes {
                let e = modes.entry(m).or_default();
                for p in ps {
                    if !e.contains(&p) {
                        e.push(p);
                    }
                }
            }
            c.categories.extend(one.categories.into_iter().filter(|(id, _)| *id < 100000));
        }
        modes.entry("search".into()).or_insert_with(|| vec!["q".into()]);
        c.modes = modes.into_iter().collect();
        if c.categories.is_empty() {
            c.categories = categories::STANDARD.iter().map(|(id, n)| (*id, n.to_string())).collect();
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

    /// Indexers that are failing (on hold, or failed last time): (name, why).
    pub fn failing(&self) -> Result<Vec<(String, String)>> {
        Ok(self.list()?.into_iter().filter(|i| i.enabled && i.failures > 0).map(|i| (i.name, i.last_error)).collect())
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
