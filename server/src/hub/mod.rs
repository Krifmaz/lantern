//! The download hub (goal 9): Lantern downloads for every app. Apps connect with Lantern's address and API key,
//! register themselves (name, media type, import path) and get a category and a folder of their own; they search
//! and grab through Lantern, which sends each grab to a download client, follows its progress, tries again when
//! sending fails, and calls the app back (webhook and live event stream) with the finished files' exact paths.
//!
//! | Module | What |
//! | --- | --- |
//! | [`clients`] | the download-client layer (qBittorrent, SABnzbd) |
//! | [`apps`] | registered apps, their category and folders |
//! | [`grabs`] | grabs: sending, progress, retries |
//! | [`cleanup`] | seeding rules and cleanup after the app imported |
//! | [`events`] | the live event stream and the webhooks |
//! | [`paths`] | translating paths between Lantern, the clients and the apps |
//! | [`torrent`] | info hashes of torrent files and magnets |
//! | [`emulate`] | Lantern as a download client: the qBittorrent Web API |
//! | [`api`] | the routes |
//!
//! Every change to a client, a path or an app is read again at the next step, so it applies to every app at once.

pub mod api;
pub mod apps;
pub mod cleanup;
pub mod clients;
pub mod emulate;
pub mod events;
pub mod grabs;
pub mod paths;
pub mod torrent;

use crate::indexers::release::Protocol;
use crate::indexers::{Indexers, LinkSigner, MASK};
use clients::{Backend, ClientConfig};
use kit_server_core::{err, p, Db, Result, SqliteDb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The hub's tables: download clients, apps and grabs.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS download_clients (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            url TEXT NOT NULL,
            username TEXT NOT NULL DEFAULT '',
            password TEXT NOT NULL DEFAULT '',
            api_key TEXT NOT NULL DEFAULT '',
            root TEXT NOT NULL DEFAULT '',
            enabled INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 25,
            added REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS hub_apps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE COLLATE NOCASE,
            media TEXT NOT NULL DEFAULT 'other',
            category TEXT NOT NULL UNIQUE,
            import_path TEXT NOT NULL DEFAULT '',
            callback_url TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT 'native',
            callback_error TEXT NOT NULL DEFAULT '',
            added REAL NOT NULL,
            last_seen REAL NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS hub_grabs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            app_id INTEGER NOT NULL,
            title TEXT NOT NULL,
            protocol TEXT NOT NULL,
            indexer_id INTEGER NOT NULL DEFAULT 0,
            link TEXT NOT NULL DEFAULT '',
            client_id INTEGER NOT NULL DEFAULT 0,
            client_ref TEXT NOT NULL DEFAULT '',
            category TEXT NOT NULL,
            state TEXT NOT NULL,
            progress REAL NOT NULL DEFAULT 0,
            size INTEGER NOT NULL DEFAULT 0,
            speed INTEGER NOT NULL DEFAULT 0,
            eta INTEGER NOT NULL DEFAULT 0,
            path TEXT NOT NULL DEFAULT '',
            files TEXT NOT NULL DEFAULT '[]',
            error TEXT NOT NULL DEFAULT '',
            attempts INTEGER NOT NULL DEFAULT 0,
            next_try REAL NOT NULL DEFAULT 0,
            sent REAL NOT NULL DEFAULT 0,
            added REAL NOT NULL,
            updated REAL NOT NULL,
            finished REAL NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS hub_grabs_by_state ON hub_grabs(state);
        CREATE INDEX IF NOT EXISTS hub_grabs_by_app ON hub_grabs(app_id, added);",
    )
    .map_err(|e| anyhow::anyhow!("The download hub's tables can't be made: {}", e.detail))?;
    // columns added after the first release of the hub
    let added = [
        ("hub_grabs", "ratio", "REAL NOT NULL DEFAULT 0"),
        ("hub_grabs", "seed_secs", "INTEGER NOT NULL DEFAULT 0"),
        ("hub_grabs", "imported", "REAL NOT NULL DEFAULT 0"),
        ("hub_grabs", "infohash", "TEXT NOT NULL DEFAULT ''"),
        ("hub_grabs", "ext_category", "TEXT NOT NULL DEFAULT ''"),
        ("hub_apps", "ext_categories", "TEXT NOT NULL DEFAULT '[]'"),
    ];
    for (table, col, def) in added {
        let have: Vec<String> = db
            .query(&format!("PRAGMA table_info({table})"), p![])
            .map_err(|e| anyhow::anyhow!("The download hub's tables can't be read: {}", e.detail))?
            .iter()
            .map(|r| kit_server_core::RowExt::s(r, "name"))
            .collect();
        if !have.iter().any(|c| c == col) {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {def}"))
                .map_err(|e| anyhow::anyhow!("The download hub's tables can't be changed: {}", e.detail))?;
        }
    }
    Ok(())
}

/// The running clients: id → (settings fingerprint, client).
type Backends = HashMap<i64, (String, Arc<Backend>)>;

/// The download hub: its tables, the indexers it grabs from, the running clients and the event stream.
#[derive(Clone)]
pub struct Hub {
    /// The database.
    pub db: SqliteDb,
    /// The indexers grabs come from.
    pub indexers: Indexers,
    /// Opens the signed download links apps hand back.
    pub signer: LinkSigner,
    /// The settings (downloads folder, attempts, API key for webhook signatures).
    pub settings: kit_server_settings::Store,
    /// The live events and the webhooks.
    pub events: events::Events,
    /// The running clients, kept so their sessions last.
    backends: Arc<Mutex<Backends>>,
    /// Held while a step runs, so two never overlap.
    ticking: Arc<tokio::sync::Mutex<()>>,
    /// Fetches release files from addresses apps give.
    http: reqwest::Client,
    /// The data folder: release files apps upload are kept in `grabs/` there until their grab is removed.
    pub data_dir: std::path::PathBuf,
    /// The sessions of apps signed in to the emulated qBittorrent: session id → app id.
    pub qbit_sessions: Arc<Mutex<HashMap<String, i64>>>,
}

impl Hub {
    /// The hub on `db`, grabbing through `indexers`.
    pub fn new(
        db: SqliteDb,
        indexers: Indexers,
        signer: LinkSigner,
        settings: kit_server_settings::Store,
        data_dir: &std::path::Path,
    ) -> Hub {
        Hub {
            db,
            indexers,
            signer,
            settings,
            events: events::Events::new(),
            backends: Default::default(),
            ticking: Default::default(),
            http: clients::http_client(false),
            data_dir: data_dir.to_path_buf(),
            qbit_sessions: Default::default(),
        }
    }

    /// Lantern's downloads folder (the `downloads_root` setting, `/downloads` by default), without a slash at the end.
    pub fn root(&self) -> String {
        let r = self.settings.str("downloads_root");
        let r = r.trim().trim_end_matches('/');
        if r.is_empty() {
            "/downloads".into()
        } else {
            r.to_string()
        }
    }

    /// How many times a grab is sent before it counts as failed (the `grab_attempts` setting, at least 1).
    pub fn max_attempts(&self) -> i64 {
        self.settings.int("grab_attempts").max(1)
    }

    // ---- download clients ----

    /// Every download client, by priority then name.
    pub fn clients(&self) -> Result<Vec<ClientConfig>> {
        Ok(self
            .db
            .query("SELECT * FROM download_clients ORDER BY priority, name COLLATE NOCASE", p![])?
            .iter()
            .map(ClientConfig::from_row)
            .collect())
    }

    /// Download client `id`; 404 when there is none.
    pub fn client(&self, id: i64) -> Result<ClientConfig> {
        self.db
            .one("SELECT * FROM download_clients WHERE id=?", p![id])?
            .map(|r| ClientConfig::from_row(&r))
            .ok_or_else(|| err(404, "There is no download client with that id"))
    }

    /// The enabled client that takes `protocol` first, if any.
    pub fn client_for(&self, protocol: Protocol) -> Result<Option<ClientConfig>> {
        Ok(self.clients()?.into_iter().find(|c| c.enabled && c.protocol() == protocol))
    }

    /// Adds a download client from `body` (`kind`, `name`, `url`, `username`, `password`, `api_key`, `root`,
    /// `enabled`, `priority`); its id. The kind defaults to qBittorrent.
    pub fn add_client(&self, body: &Value) -> Result<i64> {
        let kind = text(body, "kind").unwrap_or_else(|| "qbittorrent".into()).to_lowercase();
        let Some(&(_, _, label)) = clients::KINDS.iter().find(|(k, _, _)| *k == kind) else {
            let known: Vec<&str> = clients::KINDS.iter().map(|(_, _, n)| *n).collect();
            return Err(err(400, format!("Lantern doesn't know the download client {kind}. It knows {}.", known.join(", "))));
        };
        let url = text(body, "url").unwrap_or_default();
        check_url(&url)?;
        let name = text(body, "name").filter(|n| !n.is_empty()).unwrap_or_else(|| label.to_string());
        let id = self.db.execute(
            "INSERT INTO download_clients (kind, name, url, username, password, api_key, root, enabled, priority, added) VALUES (?,?,?,?,?,?,?,?,?,?)",
            p![
                kind,
                name,
                clients::trim_url(&url),
                text(body, "username").unwrap_or_default(),
                secret(body, "password", ""),
                secret(body, "api_key", ""),
                paths::clean(&text(body, "root").unwrap_or_default()),
                body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                body.get("priority").and_then(Value::as_i64).unwrap_or(25).clamp(1, 50),
                crate::auth::now()
            ],
        )?;
        Ok(id)
    }

    /// Changes client `id`: only the fields `body` has; a masked or empty secret keeps the saved one.
    pub fn update_client(&self, id: i64, body: &Value) -> Result<()> {
        let cur = self.client(id)?;
        let url = text(body, "url").unwrap_or(cur.url.clone());
        check_url(&url)?;
        let name = text(body, "name").unwrap_or(cur.name.clone());
        if name.is_empty() {
            return Err(err(400, "Enter a name for the download client"));
        }
        self.db.execute(
            "UPDATE download_clients SET name=?, url=?, username=?, password=?, api_key=?, root=?, enabled=?, priority=? WHERE id=?",
            p![
                name,
                clients::trim_url(&url),
                text(body, "username").unwrap_or(cur.username),
                secret(body, "password", &cur.password),
                secret(body, "api_key", &cur.api_key),
                text(body, "root").map(|r| paths::clean(&r)).unwrap_or(cur.root),
                body.get("enabled").and_then(Value::as_bool).unwrap_or(cur.enabled),
                body.get("priority").and_then(Value::as_i64).unwrap_or(cur.priority).clamp(1, 50),
                id
            ],
        )?;
        Ok(())
    }

    /// Removes client `id`. Grabs it holds stay, and are no longer followed.
    pub fn remove_client(&self, id: i64) -> Result<()> {
        self.client(id)?;
        self.db.execute("DELETE FROM download_clients WHERE id=?", p![id])?;
        self.backends.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        Ok(())
    }

    /// The running client of `cfg`, made (or made again after a change) when needed.
    pub fn backend(&self, cfg: &ClientConfig) -> Result<Arc<Backend>> {
        let fp = cfg.fingerprint();
        let mut map = self.backends.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((f, b)) = map.get(&cfg.id) {
            if *f == fp {
                return Ok(b.clone());
            }
        }
        let b = Arc::new(Backend::new(cfg).map_err(|e| err(400, e))?);
        map.insert(cfg.id, (fp, b.clone()));
        Ok(b)
    }

    /// Signs in to client `id` and asks its version: `{ok, message}`.
    pub async fn test_client(&self, id: i64) -> Result<Value> {
        let cfg = self.client(id)?;
        let b = self.backend(&cfg)?;
        Ok(match b.test().await {
            Ok(v) => json!({ "ok": true, "message": format!("Connected to {v}") }),
            Err(e) => json!({ "ok": false, "message": e }),
        })
    }

    /// A client as the API shows it: secrets masked.
    pub fn client_json(&self, c: &ClientConfig) -> Value {
        let mask = |s: &str| if s.is_empty() { "" } else { MASK };
        json!({
            "id": c.id, "kind": c.kind, "name": c.name, "url": c.url, "username": c.username,
            "password": mask(&c.password), "api_key": mask(&c.api_key), "root": c.root, "enabled": c.enabled,
            "priority": c.priority, "protocol": c.protocol(), "added": c.added,
        })
    }
}

/// Field `k` of `body` as trimmed text, when it is there.
pub(crate) fn text(body: &Value, k: &str) -> Option<String> {
    body.get(k).and_then(Value::as_str).map(|s| s.trim().to_string())
}

/// Secret field `k` of `body`: the new value, or `old` when it is missing, empty or the mask.
fn secret(body: &Value, k: &str, old: &str) -> String {
    match body.get(k).and_then(Value::as_str) {
        Some(s) if !s.is_empty() && s != MASK => s.to_string(),
        _ => old.to_string(),
    }
}

/// 400 unless `url` is an http(s) address.
fn check_url(url: &str) -> Result<()> {
    match url::Url::parse(url.trim()) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => Ok(()),
        _ => Err(err(400, "Enter the download client's address, like http://qbittorrent:8080")),
    }
}
