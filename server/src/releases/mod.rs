//! Releases as they appear (goal 3, autobrr's job): trackers' IRC announce channels and RSS feeds are watched,
//! every new release is checked against the filters, and the first filter it passes sends it on at once, to
//! Sonarr, Radarr, Lidarr, Readarr or Whisparr, or to a download client through the download hub ([`crate::hub`]).
//!
//! | Module | What |
//! | --- | --- |
//! | [`definition`], [`announce`] | the announce definitions (autobrr's files) and reading announce lines |
//! | [`indexer`] | autobrr's indexers (the release sources, with their definitions' settings) |
//! | [`irc`] | autobrr's IRC networks, one connection per enabled network |
//! | [`rss`] | the RSS feeds and Lantern's own indexers, polled |
//! | [`rls`], [`release`], [`releasetags`], [`humanize`] | reading a release (autobrr's release parsing, on a port of rls) |
//! | [`filter`], [`rejections`], [`wildcard`], [`sanitize`], [`torrent`] | deciding whether to take it (autobrr's filters) |
//! | [`macros`] | autobrr's `{{ .TorrentName }}` macros |
//! | [`push`] | where a taken release goes |
//! | [`api`] | the routes |
//!
//! Sources are rows of `release_sources`, filters of `release_filters`; every release seen and what happened to
//! it is a row of `release_log`. Secrets (passkeys, NickServ passwords, API keys) are never sent back or logged.

pub mod action;
pub mod announce;
pub mod api;
pub mod autobrr_api;
pub mod clients;
pub mod definition;
pub mod downloader;
pub mod external;
pub mod filter;
pub mod gotemplate;
pub mod humanize;
pub mod indexer;
pub mod irc;
pub mod macros;
pub mod push;
pub mod rejections;
pub mod release;
pub mod releasetags;
pub mod rls;
pub mod rss;
pub mod run;
pub mod sanitize;
pub mod shellquote;
pub mod store;
pub mod torrent;
pub mod wildcard;

use crate::indexers::{Indexers, LinkSigner, MASK};
use filter::Filter;
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use push::{DownloadHub, Grab};
use rejections::RejectionReasons;
use release::{IndexerMinimal, Release};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// How many rows the release log keeps.
pub const LOG_KEEP: i64 = 5000;

/// The tables of the sources, filters, release log and the RSS items already seen.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS release_sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            definition TEXT NOT NULL DEFAULT '',
            url TEXT NOT NULL DEFAULT '',
            indexer_id INTEGER NOT NULL DEFAULT 0,
            settings TEXT NOT NULL DEFAULT '{}',
            interval_min INTEGER NOT NULL DEFAULT 10,
            enabled INTEGER NOT NULL DEFAULT 1,
            last_at REAL NOT NULL DEFAULT 0,
            last_error TEXT NOT NULL DEFAULT '',
            added REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS release_filters (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 0,
            body TEXT NOT NULL DEFAULT '{}',
            added REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS release_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            at REAL NOT NULL,
            source_id INTEGER NOT NULL,
            source_name TEXT NOT NULL,
            title TEXT NOT NULL,
            size INTEGER,
            category TEXT NOT NULL DEFAULT '',
            info_url TEXT NOT NULL DEFAULT '',
            filter_id INTEGER NOT NULL DEFAULT 0,
            filter_name TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            detail TEXT NOT NULL DEFAULT '',
            reasons TEXT NOT NULL DEFAULT '[]'
        );
        CREATE INDEX IF NOT EXISTS release_log_by_filter ON release_log(filter_id, status, at);
        CREATE TABLE IF NOT EXISTS release_seen (
            source_id INTEGER NOT NULL,
            key TEXT NOT NULL,
            at REAL NOT NULL,
            PRIMARY KEY (source_id, key)
        );",
    )
    .map_err(|e| anyhow::anyhow!("The release tables can't be made: {}", e.detail))?;
    store::migrate(db)?;
    indexer::migrate(db)?;
    irc::migrate(db)
}

/// A release a source just announced, before the filters see it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// The release's name.
    pub title: String,
    /// The source's id.
    pub source_id: i64,
    /// The source's name (the tracker).
    pub source_name: String,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// The download link (with the passkey: never logged or sent back).
    pub download_url: String,
    /// The release's page.
    pub info_url: String,
    /// Bytes, when the announce says.
    pub size: Option<u64>,
    /// The category as the site names it.
    pub category: String,
    /// Freeleech.
    pub freeleech: bool,
    /// The freeleech percentage, when the announce says.
    pub freeleech_percent: Option<u32>,
    /// Who uploaded it.
    pub uploader: String,
    /// The site's tags.
    pub tags: Vec<String>,
    /// The year, when the announce says.
    pub year: Option<u32>,
    /// The tracker's short name (the announce definition's identifier, `redacted`); empty for feeds.
    pub identifier: String,
    /// IRC: every value the announce line gave, by autobrr's names (`torrentName`, `torrentSize` …).
    pub vars: BTreeMap<String, String>,
    /// IRC: the unit of a size announced without one (the definition's `forcesizeunit`).
    pub force_size_unit: String,
    /// IRC: the release as the indexer's definition already read it (autobrr's announce processor); used as is.
    pub prepared: Option<Box<Release>>,
}

impl Found {
    /// The release as autobrr's filters see it: an announce's values read as autobrr's `MapVars` reads them (a
    /// feed item's as its feed reader does), then the name read by [`Release::parse_string`].
    pub fn release(&self) -> Release {
        if let Some(r) = &self.prepared {
            return (**r).clone();
        }
        let mut r = Release::new(IndexerMinimal {
            id: self.source_id,
            name: self.source_name.clone(),
            identifier: self.identifier.clone(),
            identifier_external: self.source_name.clone(),
        });
        let mut mapped = false;
        if !self.vars.is_empty() {
            let mut vars = self.vars.clone();
            vars.insert("torrentName".into(), self.title.clone());
            vars.remove("releaseName");
            mapped = r.map_vars(&vars, &self.force_size_unit, "").is_ok();
        }
        if !mapped {
            r.implementation = "RSS".into();
            r.torrent_name = self.title.clone();
            r.category = self.category.clone();
            r.size = self.size.unwrap_or(0);
            r.freeleech = self.freeleech || self.freeleech_percent.is_some_and(|p| p > 0);
            r.freeleech_percent = self.freeleech_percent.map(i64::from).unwrap_or(if self.freeleech { 100 } else { 0 });
            if r.freeleech {
                r.bonus.push("Freeleech".into());
            }
            r.uploader = self.uploader.clone();
            r.tags = self.tags.clone();
            r.year = self.year.map(i64::from).unwrap_or(0);
        }
        r.protocol = if self.protocol == "usenet" { "usenet".into() } else { "torrent".into() };
        r.download_url = self.download_url.clone();
        if self.download_url.starts_with(run::MAGNET_URI_PREFIX) {
            r.magnet_uri = self.download_url.clone();
        }
        r.info_url = self.info_url.clone();
        let name = r.torrent_name.clone();
        r.parse_string(&name);
        r
    }
}

/// A place releases come from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Source {
    /// Its id.
    pub id: i64,
    /// `irc`, `rss` (a feed address), `indexer` (the newest releases of one of Lantern's indexers) or `feed` (an
    /// autobrr Torznab, Newznab or RSS indexer).
    pub kind: String,
    /// The name people see.
    pub name: String,
    /// `irc`: the announce definition's id.
    pub definition: String,
    /// `rss`: the feed's address (often with a passkey: sent back without its query).
    pub url: String,
    /// `indexer`: the indexer's id.
    pub indexer_id: i64,
    /// `irc`: the definition's settings (passkey, nick, auth.account, auth.password, invite_command) and the
    /// optional `server`, `port`, `tls` and `channels` that replace the definition's (a bouncer).
    pub settings: BTreeMap<String, String>,
    /// `rss`/`indexer`: minutes between looks.
    pub interval_min: i64,
    /// Watched at all.
    pub enabled: bool,
    /// When it last answered or was looked at.
    pub last_at: f64,
    /// The last problem, in words.
    pub last_error: String,
    /// When it was added.
    pub added: f64,
}

/// The kinds of source.
pub const SOURCE_KINDS: [&str; 3] = ["irc", "rss", "indexer"];

impl Source {
    fn from_row(r: &Row) -> Source {
        Source {
            id: r.i("id"),
            kind: r.s("kind"),
            name: r.s("name"),
            definition: r.s("definition"),
            url: r.s("url"),
            indexer_id: r.i("indexer_id"),
            settings: string_map(&r.s("settings")),
            interval_min: r.i("interval_min"),
            enabled: r.b("enabled"),
            last_at: r.f("last_at"),
            last_error: r.s("last_error"),
            added: r.f("added"),
        }
    }
}

/// A JSON object of text as a map (numbers and booleans as their text).
fn string_map(s: &str) -> BTreeMap<String, String> {
    serde_json::from_str::<Map<String, Value>>(s)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| {
            let v = match v {
                Value::String(s) => s,
                Value::Null => String::new(),
                o => o.to_string(),
            };
            (k, v)
        })
        .collect()
}

/// What happened to one release.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `pushed` (sent on), `failed` (a filter took it, sending failed) or `rejected` (no filter took it).
    pub status: String,
    /// The filter that took it.
    pub filter: Option<(i64, String)>,
    /// What happened, in words.
    pub detail: String,
    /// Per filter that didn't take it: its name and why.
    pub reasons: Vec<(String, Vec<String>)>,
}

/// Sources, filters, the release log, the running IRC connections and the download hub.
#[derive(Clone)]
pub struct Releases {
    db: SqliteDb,
    /// The announce definitions.
    pub library: announce::Library,
    /// Lantern's indexers (the `indexer` sources).
    pub indexers: Indexers,
    /// Signs the download links of the `indexer` sources' releases.
    pub signer: LinkSigner,
    /// The settings (`public_url`, `api_key` for those links).
    pub settings: kit_server_settings::Store,
    hub: Arc<RwLock<Arc<dyn DownloadHub>>>,
    http: reqwest::Client,
    /// Fetches a release's torrent for autobrr's actions and external filters.
    downloads: Arc<dyn run::ReleaseDownloader>,
    /// The IRC connections.
    pub irc: irc::Irc,
}

impl Releases {
    /// The releases on `db`, with the announce definitions `library`.
    pub fn new(
        db: SqliteDb,
        library: announce::Library,
        indexers: Indexers,
        signer: LinkSigner,
        settings: kit_server_settings::Store,
    ) -> Releases {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Lantern/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Releases {
            db,
            library,
            indexers,
            signer,
            settings,
            hub: Arc::new(RwLock::new(Arc::new(push::NoHub))),
            http,
            downloads: Arc::new(run::HttpDownloader::new()),
            irc: irc::Irc::default(),
        }
    }

    /// Sends client pushes to `hub` from now on (the download hub, when Lantern has one).
    pub fn set_hub(&self, hub: Arc<dyn DownloadHub>) {
        *self.hub.write().unwrap_or_else(|e| e.into_inner()) = hub;
    }

    fn hub(&self) -> Arc<dyn DownloadHub> {
        self.hub.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// What runs autobrr's actions: the torrent fetcher, the download clients, the hub.
    pub fn runner(&self) -> run::Service {
        let me = self.clone();
        run::Service {
            downloads: self.downloads.clone(),
            downloaders: Arc::new(move |id| me.downloader(id).map_err(|e| e.message().to_string())),
            hub: self.hub(),
            http: run::webhook_client(),
            tmp_dir: run::default_tmp_dir(),
        }
    }

    // ---- sources ----

    /// Every source, by name.
    pub fn sources(&self) -> Result<Vec<Source>> {
        Ok(self.db.query("SELECT * FROM release_sources ORDER BY name COLLATE NOCASE", p![])?.iter().map(Source::from_row).collect())
    }

    /// Source `id`; 404 when there is none.
    pub fn source(&self, id: i64) -> Result<Source> {
        self.db
            .one("SELECT * FROM release_sources WHERE id=?", p![id])?
            .map(|r| Source::from_row(&r))
            .ok_or_else(|| err(404, "There is no release source with that id"))
    }

    /// Whether setting `key` of `s` is a secret.
    fn secret(&self, s: &Source, key: &str) -> bool {
        match self.library.get(&s.definition) {
            Some(d) => {
                d.settings.iter().chain(d.irc.iter().flat_map(|i| i.settings.iter())).any(|x| x.name == key && x.kind == "secret")
                    || definition::is_secret_indexer_setting(key)
                    || key == "auth.password"
                    || key == "pass"
            }
            None => key.contains("password") || key.contains("key"),
        }
    }

    /// Checks and fills in a source made from `body` over `cur` (what is saved); the problem in words.
    fn source_from(&self, body: &Value, cur: Source) -> Result<Source> {
        let mut s = cur;
        let str_of = |k: &str| body.get(k).and_then(Value::as_str).map(|v| v.trim().to_string());
        if let Some(k) = str_of("kind") {
            s.kind = k;
        }
        if !SOURCE_KINDS.contains(&s.kind.as_str()) {
            return Err(err(400, "A release source is an IRC announce channel, an RSS feed or one of Lantern's indexers"));
        }
        if let Some(v) = str_of("name") {
            s.name = v;
        }
        if let Some(v) = str_of("definition") {
            s.definition = v;
        }
        if let Some(v) = str_of("url") {
            // the address as sent back (without its query) keeps the saved one
            if v != crate::indexers::http::strip_secrets(&s.url) || s.url.is_empty() {
                s.url = v;
            }
        }
        if let Some(v) = body.get("indexer_id").and_then(Value::as_i64) {
            s.indexer_id = v;
        }
        if let Some(v) = body.get("interval_min").and_then(Value::as_i64) {
            s.interval_min = v;
        }
        s.interval_min = s.interval_min.clamp(2, 1440);
        if let Some(v) = body.get("enabled").and_then(Value::as_bool) {
            s.enabled = v;
        }
        if let Some(Value::Object(m)) = body.get("settings") {
            let mut next = BTreeMap::new();
            for (k, v) in m {
                let v = match v {
                    Value::String(x) => x.clone(),
                    Value::Null => String::new(),
                    o => o.to_string(),
                };
                let v = if v == MASK || (v.is_empty() && self.secret(&s, k)) { s.settings.get(k).cloned().unwrap_or_default() } else { v };
                if !v.is_empty() {
                    next.insert(k.clone(), v);
                }
            }
            s.settings = next;
        }
        match s.kind.as_str() {
            "irc" => {
                let def =
                    self.library.get(&s.definition).ok_or_else(|| err(400, format!("There is no announce definition {}", s.definition)))?;
                if def.irc.is_none() {
                    return Err(err(400, format!("{} doesn't announce on IRC", def.name)));
                }
                if s.name.is_empty() {
                    s.name = def.name.clone();
                }
                for set in def.settings.iter().chain(def.irc.iter().flat_map(|i| i.settings.iter())) {
                    if set.required && s.settings.get(&set.name).is_none_or(|v| v.is_empty()) {
                        return Err(err(
                            400,
                            format!("Fill in {} for {}", if set.label.is_empty() { &set.name } else { &set.label }, def.name),
                        ));
                    }
                }
                if s.settings.get("nick").is_none_or(|v| v.is_empty()) {
                    return Err(err(
                        400,
                        "Choose the nick Lantern uses on IRC (the tracker usually wants your user name, like yourname|lantern)",
                    ));
                }
            }
            "rss" => {
                if !(s.url.starts_with("http://") || s.url.starts_with("https://")) {
                    return Err(err(400, "The feed's address must start with http:// or https://"));
                }
                if s.name.is_empty() {
                    s.name =
                        url::Url::parse(&s.url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "RSS feed".into());
                }
            }
            _ => {
                let ix = self.indexers.get(s.indexer_id).map_err(|_| err(400, "Pick one of Lantern's indexers"))?;
                if s.name.is_empty() {
                    s.name = ix.name;
                }
            }
        }
        Ok(s)
    }

    /// Adds a source from `body` (`kind`, `name`, `definition`, `url`, `indexer_id`, `settings`, `interval_min`,
    /// `enabled`); its id.
    pub fn add_source(&self, body: &Value) -> Result<i64> {
        let s = self.source_from(body, Source { enabled: true, interval_min: 10, ..Source::default() })?;
        self.db.execute(
            "INSERT INTO release_sources (kind, name, definition, url, indexer_id, settings, interval_min, enabled, added) VALUES (?,?,?,?,?,?,?,?,?)",
            p![s.kind, s.name, s.definition, s.url, s.indexer_id, serde_json::to_string(&s.settings).unwrap_or_default(), s.interval_min, s.enabled, crate::auth::now()],
        )
    }

    /// Changes source `id`: only the fields `body` has; a masked secret keeps the saved one.
    pub fn update_source(&self, id: i64, body: &Value) -> Result<()> {
        let s = self.source_from(body, self.source(id)?)?;
        self.db.execute(
            "UPDATE release_sources SET kind=?, name=?, definition=?, url=?, indexer_id=?, settings=?, interval_min=?, enabled=?, last_error='' WHERE id=?",
            p![s.kind, s.name, s.definition, s.url, s.indexer_id, serde_json::to_string(&s.settings).unwrap_or_default(), s.interval_min, s.enabled, id],
        )?;
        Ok(())
    }

    /// Removes source `id` and what was seen from it.
    pub fn remove_source(&self, id: i64) -> Result<()> {
        self.source(id)?;
        let _ = self.db.execute("DELETE FROM autobrr_indexer_meta WHERE source_id=?", p![id]);
        self.db.execute("DELETE FROM release_sources WHERE id=?", p![id])?;
        self.db.execute("DELETE FROM release_seen WHERE source_id=?", p![id])?;
        Ok(())
    }

    /// Notes when `id` was last looked at and what went wrong ("" when nothing).
    pub fn source_result(&self, id: i64, error: &str) {
        let _ = self.db.execute("UPDATE release_sources SET last_at=?, last_error=? WHERE id=?", p![crate::auth::now(), error, id]);
    }

    /// A source as the API sends it: secrets masked, the feed address without its query, the IRC state.
    pub fn public_source(&self, s: &Source) -> Value {
        let settings: Map<String, Value> = s
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), json!(if self.secret(s, k) && !v.is_empty() { MASK.to_string() } else { v.clone() })))
            .collect();
        let def = self.library.get(&s.definition);
        json!({
            "id": s.id, "kind": s.kind, "name": s.name, "definition": s.definition,
            "definition_name": def.as_ref().map(|d| d.name.clone()).unwrap_or_default(),
            "url": crate::indexers::http::strip_secrets(&s.url), "indexer_id": s.indexer_id, "settings": settings,
            "interval_min": s.interval_min, "enabled": s.enabled, "last_at": s.last_at, "last_error": s.last_error, "added": s.added,
            "irc": (s.kind == "irc").then(|| self.source_irc_status(s)),
        })
    }

    // ---- filters ----

    /// Every filter, highest priority first.
    pub fn filters(&self) -> Result<Vec<Filter>> {
        Ok(self
            .db
            .query("SELECT * FROM release_filters ORDER BY priority DESC, name COLLATE NOCASE", p![])?
            .iter()
            .map(filter_of)
            .collect())
    }

    /// Filter `id`; 404 when there is none.
    pub fn filter(&self, id: i64) -> Result<Filter> {
        self.db
            .one("SELECT * FROM release_filters WHERE id=?", p![id])?
            .map(|r| filter_of(&r))
            .ok_or_else(|| err(404, "There is no filter with that id"))
    }

    /// A filter from `body` over `cur`, masked API keys keeping the saved ones; the problem in words.
    fn filter_from(body: &Value, cur: Option<&Filter>) -> Result<Filter> {
        let mut base = serde_json::to_value(cur.cloned().unwrap_or_else(|| Filter::new(""))).unwrap_or_default();
        let body = filter::upgrade(body.clone());
        if let (Value::Object(b), Value::Object(n)) = (&mut base, &body) {
            for (k, v) in n {
                b.insert(k.clone(), v.clone());
            }
        }
        let mut f: Filter = serde_json::from_value(base).map_err(|e| err(400, format!("The filter can't be read: {e}")))?;
        for (i, a) in f.actions.iter_mut().enumerate() {
            if a.api_key == MASK {
                a.api_key = cur
                    .and_then(|c| c.actions.iter().find(|o| o.url == a.url && o.app == a.app).or(c.actions.get(i)))
                    .map(|o| o.api_key.clone())
                    .unwrap_or_default();
            }
        }
        f.sanitize();
        f.validate().map_err(|e| err(400, e))?;
        Ok(f)
    }

    /// Adds a filter from `body` (a [`Filter`] as JSON; what it leaves out keeps the defaults); its id.
    pub fn add_filter(&self, body: &Value) -> Result<i64> {
        let f = Self::filter_from(body, None)?;
        self.db.execute(
            "INSERT INTO release_filters (name, enabled, priority, body, added) VALUES (?,?,?,?,?)",
            p![f.name.trim(), f.enabled, f.priority, serde_json::to_string(&f).unwrap_or_default(), crate::auth::now()],
        )
    }

    /// Changes filter `id`: the fields `body` has.
    pub fn update_filter(&self, id: i64, body: &Value) -> Result<()> {
        let cur = self.filter(id)?;
        let f = Self::filter_from(body, Some(&cur))?;
        self.db.execute(
            "UPDATE release_filters SET name=?, enabled=?, priority=?, body=? WHERE id=?",
            p![f.name.trim(), f.enabled, f.priority, serde_json::to_string(&f).unwrap_or_default(), id],
        )?;
        Ok(())
    }

    /// Removes filter `id`.
    pub fn remove_filter(&self, id: i64) -> Result<()> {
        self.filter(id)?;
        self.db.execute("DELETE FROM release_filters WHERE id=?", p![id])?;
        Ok(())
    }

    /// A filter as the API sends it: API keys masked, with its grabs today.
    pub fn public_filter(&self, f: &Filter) -> Value {
        let mut v = serde_json::to_value(f).unwrap_or_default();
        if let Some(actions) = v.get_mut("actions").and_then(Value::as_array_mut) {
            for a in actions {
                if a.get("api_key").and_then(Value::as_str).is_some_and(|k| !k.is_empty()) {
                    a["api_key"] = json!(MASK);
                }
            }
        }
        v["grabs_today"] = json!(self.grabs_since(f.id, crate::auth::now() - 86400.0));
        filter::with_legacy(v)
    }

    /// How many releases filter `id` sent on since `since`.
    fn grabs_since(&self, id: i64, since: f64) -> i64 {
        self.db
            .one("SELECT COUNT(*) AS n FROM release_log WHERE filter_id=? AND status='pushed' AND at>=?", p![id, since])
            .ok()
            .flatten()
            .map(|r| r.i("n"))
            .unwrap_or(0)
    }

    // ---- the log ----

    /// The newest `limit` rows of the release log (only `status` ones, when given).
    pub fn log(&self, limit: i64, status: Option<&str>) -> Result<Vec<Value>> {
        let rows = match status {
            Some(s) => self.db.query("SELECT * FROM release_log WHERE status=? ORDER BY id DESC LIMIT ?", p![s, limit.clamp(1, 1000)])?,
            None => self.db.query("SELECT * FROM release_log ORDER BY id DESC LIMIT ?", p![limit.clamp(1, 1000)])?,
        };
        Ok(rows
            .into_iter()
            .map(|r| {
                let reasons: Value = serde_json::from_str(&r.s("reasons")).unwrap_or(json!([]));
                json!({
                    "id": r.i("id"), "at": r.f("at"), "source_id": r.i("source_id"), "source_name": r.s("source_name"), "title": r.s("title"),
                    "size": r.get("size").cloned().unwrap_or(Value::Null), "category": r.s("category"), "info_url": r.s("info_url"),
                    "filter_id": r.i("filter_id"), "filter_name": r.s("filter_name"), "status": r.s("status"), "detail": r.s("detail"), "reasons": reasons,
                })
            })
            .collect())
    }

    fn write_log(&self, f: &Found, o: &Outcome) {
        let reasons: Vec<Value> = o.reasons.iter().map(|(n, why)| json!({ "filter": n, "why": why })).collect();
        let (fid, fname) = o.filter.clone().unwrap_or((0, String::new()));
        let _ = self.db.execute(
            "INSERT INTO release_log (at, source_id, source_name, title, size, category, info_url, filter_id, filter_name, status, detail, reasons) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            p![crate::auth::now(), f.source_id, f.source_name, f.title, f.size, f.category, crate::indexers::http::strip_secrets(&f.info_url), fid, fname, o.status, o.detail, Value::Array(reasons).to_string()],
        );
        let _ = self.db.execute("DELETE FROM release_log WHERE id <= (SELECT MAX(id) FROM release_log) - ?", p![LOG_KEEP]);
    }

    // ---- deciding and sending ----

    /// `flt` with its download count in its current window filled in, when it has a download limit.
    fn with_downloads(&self, flt: &Filter) -> Filter {
        let mut flt = flt.clone();
        if flt.is_max_downloads_limit_enabled() {
            let since = flt.downloads_window_start(chrono::Local::now()).map(|t| t.timestamp() as f64).unwrap_or(0.0);
            flt.downloads = Some(filter::FilterDownloads { period_count: self.grabs_since(flt.id, since) });
        }
        flt
    }

    /// The checks that need nothing fetched: the source the filter watches, then autobrr's `CheckFilter` on a
    /// copy of `base`. The release as the filter left it, or why it doesn't pass.
    fn first_checks(&self, flt: &Filter, f: &Found, base: &Release) -> std::result::Result<Release, Vec<String>> {
        if !flt.indexers.is_empty() && !flt.indexers.contains(&f.source_id) {
            return Err(vec![format!("it comes from {}, which this filter doesn't watch", f.source_name)]);
        }
        let mut r = base.clone();
        let (rr, ok) = self.with_downloads(flt).check_filter(&mut r);
        if ok {
            Ok(r)
        } else {
            Err(rr.lines())
        }
    }

    /// Which filters `f` passes and why the others don't, sending nothing (a size limit with no size known
    /// passes: Lantern reads the torrent then).
    pub fn dry_run(&self, f: &Found) -> Result<Vec<Value>> {
        let base = f.release();
        Ok(self
            .filters()?
            .iter()
            .filter(|flt| flt.enabled)
            .map(|flt| match self.first_checks(flt, f, &base) {
                Ok(_) => json!({ "id": flt.id, "name": flt.name, "passes": true, "why": [] }),
                Err(why) => json!({ "id": flt.id, "name": flt.name, "passes": false, "why": why }),
            })
            .collect())
    }

    /// Checks `f` against the enabled filters, highest priority first, as autobrr's filter service does; the
    /// first one it passes sends it on. Every release ends up in the log.
    pub async fn handle(&self, mut f: Found) -> Outcome {
        let filters = match self.filters() {
            Ok(l) => l.into_iter().filter(|x| x.enabled).collect::<Vec<_>>(),
            Err(e) => {
                let o = Outcome { status: "failed".into(), filter: None, detail: e.detail, reasons: Vec::new() };
                self.write_log(&f, &o);
                return o;
            }
        };
        let base = f.release();
        let mut reasons: Vec<(String, Vec<String>)> = Vec::new();
        let mut size_read = false;
        for flt in &filters {
            let r = match self.first_checks(flt, &f, &base) {
                Ok(r) => r,
                Err(why) => {
                    reasons.push((flt.name.clone(), why));
                    continue;
                }
            };
            // autobrr's AdditionalSizeCheck: the announce gave no size, so the torrent is read
            if r.additional_size_check_required {
                if !size_read {
                    size_read = true;
                    if f.size.is_none() {
                        f.size = self.read_size(&f.download_url).await;
                    }
                }
                let Some(size) = f.size else {
                    reasons.push((flt.name.clone(), vec!["its size isn't known and the torrent file couldn't be read".into()]));
                    continue;
                };
                let mut rr = RejectionReasons::new();
                match flt.check_release_size(size, &mut rr) {
                    Ok(true) => {}
                    Ok(false) => {
                        reasons.push((flt.name.clone(), rr.lines()));
                        continue;
                    }
                    Err(e) => {
                        reasons.push((flt.name.clone(), vec![e]));
                        continue;
                    }
                }
            }
            if r.additional_uploader_check_required || r.additional_record_label_check_required {
                reasons.push((
                    flt.name.clone(),
                    vec!["the announce doesn't say its uploader or record label, and Lantern can't ask this site yet".into()],
                ));
                continue;
            }
            if flt.skip_duplicates && self.already_taken(flt.id, &f.title) {
                reasons.push((flt.name.clone(), vec!["it already took a release with this name".into()]));
                continue;
            }
            let autobrr_actions = self.actions_of(flt.id, true, false).unwrap_or_default();
            if flt.actions.is_empty() && autobrr_actions.is_empty() {
                reasons.push((flt.name.clone(), vec!["it passes, but the filter has nowhere to send it".into()]));
                continue;
            }
            let mut r = r;
            r.filter_id = flt.id;
            r.filter_name = flt.name.clone();
            if r.size == 0 {
                r.size = f.size.unwrap_or(0);
            }
            let externals = Self::externals(flt);
            if !externals.is_empty() {
                let runner = self.runner();
                let mut rr = RejectionReasons::new();
                match external::run_external_filters(&externals, &mut r, runner.downloads.as_ref(), &runner.http, &runner.tmp_dir, &mut rr)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        run::cleanup_temporary_files(&mut r);
                        reasons.push((flt.name.clone(), rr.lines()));
                        continue;
                    }
                    Err(e) => {
                        run::cleanup_temporary_files(&mut r);
                        reasons.push((flt.name.clone(), vec![e]));
                        continue;
                    }
                }
            }
            if flt.delay > 0 {
                tokio::time::sleep(Duration::from_secs(flt.delay.clamp(0, 3600) as u64)).await;
            }
            let o = self.send(flt, &f, r, autobrr_actions, reasons).await;
            self.write_log(&f, &o);
            return o;
        }
        let detail = if filters.is_empty() { "No filters yet".to_string() } else { "No filter takes it".to_string() };
        let o = Outcome { status: "rejected".into(), filter: None, detail, reasons };
        self.write_log(&f, &o);
        o
    }

    /// Whether filter `id` already sent on a release named `title`.
    fn already_taken(&self, id: i64, title: &str) -> bool {
        self.db
            .one("SELECT 1 AS y FROM release_log WHERE filter_id=? AND status='pushed' AND title=? COLLATE NOCASE LIMIT 1", p![id, title])
            .ok()
            .flatten()
            .is_some()
    }

    /// Sends `f` to every action of `flt`: Lantern's own at once, autobrr's one after another on the shared
    /// release `rel` (so its torrent is fetched once), as autobrr's release service does.
    async fn send(
        &self,
        flt: &Filter,
        f: &Found,
        mut rel: Release,
        autobrr_actions: Vec<action::Action>,
        reasons: Vec<(String, Vec<String>)>,
    ) -> Outcome {
        let grab = |a: &filter::Action| Grab {
            title: f.title.clone(),
            download_url: f.download_url.clone(),
            info_url: f.info_url.clone(),
            size: f.size,
            protocol: if f.protocol.is_empty() { "torrent".into() } else { f.protocol.clone() },
            source_name: f.source_name.clone(),
            client: a.client.clone(),
            category: a.category.clone(),
            paused: a.paused,
        };
        let hub = self.hub();
        let tries = flt.actions.iter().map(|a| {
            let g = grab(a);
            let hub = hub.clone();
            let http = self.http.clone();
            let a = a.clone();
            async move {
                let r = if a.kind == "arr" { push::to_arr(&http, &a, &g).await } else { hub.add(&g).await };
                r.map_err(|e| e.replace(&g.download_url, "(download link)"))
            }
        });
        let mut results = futures_util::future::join_all(tries).await;
        let runner = self.runner();
        for a in &autobrr_actions {
            let r = match runner.run_action(a, &mut rel).await {
                Ok(rej) if rej.is_empty() => Ok(format!("{}: sent", a.name)),
                Ok(rej) => Err(format!("{}: rejected: {}", a.name, rej.join(", "))),
                Err(e) => Err(format!("{}: {e}", a.name)),
            };
            results.push(r.map_err(|e| e.replace(&f.download_url, "(download link)")));
        }
        run::cleanup_temporary_files(&mut rel);
        let ok: Vec<String> = results.iter().filter_map(|r| r.as_ref().ok().cloned()).collect();
        let bad: Vec<String> = results.iter().filter_map(|r| r.as_ref().err().cloned()).collect();
        let status = if ok.is_empty() { "failed" } else { "pushed" };
        let detail = ok.into_iter().chain(bad).collect::<Vec<_>>().join(". ");
        Outcome { status: status.into(), filter: Some((flt.id, flt.name.clone())), detail, reasons }
    }

    /// The size of the torrent at `url`, read from the file; `None` when it can't be fetched or read.
    async fn read_size(&self, url: &str) -> Option<u64> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        let resp = self.http.get(url).timeout(Duration::from_secs(20)).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().await.ok()?;
        torrent::size(&bytes)
    }

    // ---- RSS items already seen ----

    /// Whether source `id` has seen anything yet.
    pub fn has_seen(&self, id: i64) -> bool {
        self.db.one("SELECT 1 AS y FROM release_seen WHERE source_id=? LIMIT 1", p![id]).ok().flatten().is_some()
    }

    /// Notes `key` as seen for source `id`; whether it is new.
    pub fn see(&self, id: i64, key: &str) -> bool {
        if self.db.one("SELECT 1 AS y FROM release_seen WHERE source_id=? AND key=?", p![id, key]).ok().flatten().is_some() {
            return false;
        }
        let _ = self.db.execute("INSERT OR IGNORE INTO release_seen (source_id, key, at) VALUES (?,?,?)", p![id, key, crate::auth::now()]);
        true
    }

    /// Forgets items seen more than 30 days ago.
    pub fn forget_old(&self) {
        let _ = self.db.execute("DELETE FROM release_seen WHERE at < ?", p![crate::auth::now() - 30.0 * 86400.0]);
    }

    /// Starts watching: the indexers' deprecations are applied, Lantern's own IRC sources get their networks,
    /// every enabled IRC network connects, and the RSS loop starts.
    pub fn start(&self) {
        if let Err(e) = self.reconcile_deprecations() {
            tracing::warn!("indexers: the deprecated definitions couldn't be applied: {}", e.message());
        }
        if let Ok(list) = self.sources() {
            for s in list.iter().filter(|s| s.kind == "irc") {
                self.sync_source_network(s, false);
            }
        }
        self.start_irc();
        rss::spawn(self.clone());
    }

    /// Brings source `id`'s IRC network up to date after a change made through Lantern's sources.
    pub fn changed(&self, id: i64) {
        if let Ok(s) = self.source(id) {
            if s.kind == "irc" {
                self.sync_source_network(&s, true);
            }
        }
    }

    /// Reads release `r` (an announce the indexer's definition read) through the filters, as autobrr's release
    /// service does.
    pub async fn process_release(&self, r: Release) -> Outcome {
        let f = Found {
            title: r.torrent_name.clone(),
            source_id: r.indexer.id,
            source_name: r.indexer.name.clone(),
            protocol: r.protocol.clone(),
            download_url: if r.download_url.is_empty() { r.magnet_uri.clone() } else { r.download_url.clone() },
            info_url: r.info_url.clone(),
            size: (r.size > 0).then_some(r.size),
            category: r.category.clone(),
            freeleech: r.freeleech,
            freeleech_percent: u32::try_from(r.freeleech_percent).ok().filter(|p| *p > 0),
            uploader: r.uploader.clone(),
            tags: r.tags.clone(),
            year: u32::try_from(r.year).ok().filter(|y| *y > 0),
            identifier: r.indexer.identifier.clone(),
            prepared: Some(Box::new(r)),
            ..Found::default()
        };
        self.handle(f).await
    }
}

/// A filter from its row.
fn filter_of(r: &Row) -> Filter {
    let body: Value = serde_json::from_str(&r.s("body")).unwrap_or_default();
    let mut f: Filter = serde_json::from_value(filter::upgrade(body)).unwrap_or_default();
    f.id = r.i("id");
    f.name = r.s("name");
    f.enabled = r.b("enabled");
    f.priority = r.i("priority");
    f
}
