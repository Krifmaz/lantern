// Ported from cross-seed v6.13.9 (src/pipeline.ts, src/action.ts, src/db.ts), Apache-2.0.
//! Cross-seed (goal 6), ported from cross-seed v6.13.9: Lantern looks for the torrents it has on the other torrent
//! indexers and, when another site has the same files, adds that site's torrent to the same download client,
//! pointed at the files already on disk, so it seeds on both sites without downloading anything again.
//!
//! | Part | Module | From cross-seed |
//! | --- | --- | --- |
//! | settings, defaults and checks | [`config`] | `runtimeConfig.ts`, `configSchema.ts`, `config.template.cjs` |
//! | release name patterns, decisions | [`constants`], [`jsre`] | `constants.ts` |
//! | name helpers | [`utils`] | `utils.ts` |
//! | what is searched for | [`searchee`] | `searchee.ts` |
//! | `.torrent` files | [`torrent`] | `parseTorrent.ts` |
//! | what is left out first | [`prefilter`] | `preFilter.ts` |
//! | match or not, and why | [`decide`] | `decide.ts` |
//! | save, link and inject a match | [`action`] | `action.ts` |
//! | qBittorrent, rTorrent, Transmission, Deluge | [`clients`] | `clients/*.ts` |
//! | the daemon API (its own port) | [`daemon`] | `server.ts`, `jobs.ts` (run ahead) |
//! | Lantern's page | [`api`] | – |
//!
//! Searchees are the torrents of cross-seed's torrent clients (by default Lantern's torrent download clients;
//! [`CrossSeed::search_grab`], the automatic pass [`CrossSeed::run`], and `POST /api/webhook` by info hash). Every candidate's decision is kept in
//! `cross_seeds`: cross-seed's decision (`match_decision`), what was done (`result`, cross-seed's injection result)
//! and Lantern's outcome word for the page and the hub (`decision`: `injected` keeps a grab's files on disk).
//!
//! A match is saved or injected as cross-seed's `performAction` does ([`action`]): with link folders its files are
//! linked first; without them it is added next to the searchee's files. A failed injection keeps the `.torrent` in
//! `cross-seeds/` in the data folder (cross-seed's `outputDir`).

pub mod action;
pub mod api;
pub mod clients;
pub mod config;
pub mod constants;
pub mod daemon;
pub mod decide;
pub mod jsre;
pub mod prefilter;
pub mod searchee;
pub mod torrent;
pub mod utils;

use crate::hub::clients::ClientConfig;
use crate::hub::grabs::Grab;
use crate::hub::Hub;
use crate::indexers::cardigann::Grabbed;
use crate::indexers::release::{Protocol, Query, SearchKind};
use action::ActionReturn;
use clients::{Client, SearcheeOptions};
use config::{Override, RuntimeConfig};
use constants::{Decision, InjectionResult};
use decide::{assess_candidate, reason, Assessment, Candidate, MetaOrCandidate, SnatchError};
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use prefilter::{filter_by_content, filter_timestamps, ContentOptions, Timestamps, MAX_INT};
use searchee::{Label, Searchee};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use torrent::Metafile;

/// The category suffix of cross-seeded torrents: apps watch their own category, so they don't import these.
pub const CATEGORY_SUFFIX: &str = constants::TORRENT_CATEGORY_SUFFIX;

/// The tables cross-seeding keeps: each candidate's outcome, when each grab was last searched, each searchee's
/// search times per indexer (cross-seed's `timestamp`), the client torrents (cross-seed's `client_searchee`) and
/// each job's last run (cross-seed's `job_log`).
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    let fail = |e: kit_server_core::Error| anyhow::anyhow!("The cross-seed tables can't be made: {}", e.detail);
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS cross_seeds (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            grab_id INTEGER NOT NULL,
            indexer_id INTEGER NOT NULL,
            indexer TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL,
            info_hash TEXT NOT NULL DEFAULT '',
            decision TEXT NOT NULL,
            detail TEXT NOT NULL DEFAULT '',
            client_id INTEGER NOT NULL DEFAULT 0,
            at REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS cross_seeds_by_grab ON cross_seeds(grab_id);
        CREATE TABLE IF NOT EXISTS cross_seed_searches (
            grab_id INTEGER PRIMARY KEY,
            at REAL NOT NULL,
            times INTEGER NOT NULL DEFAULT 1,
            found INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS cross_seed_timestamps (
            searchee TEXT NOT NULL,
            indexer_id INTEGER NOT NULL,
            first_searched REAL NOT NULL,
            last_searched REAL NOT NULL,
            PRIMARY KEY (searchee, indexer_id)
        );
        CREATE TABLE IF NOT EXISTS cross_seed_client_searchees (
            info_hash TEXT NOT NULL,
            name TEXT NOT NULL,
            title TEXT NOT NULL,
            files TEXT NOT NULL,
            length INTEGER NOT NULL,
            client_host TEXT NOT NULL,
            save_path TEXT NOT NULL,
            category TEXT,
            tags TEXT,
            trackers TEXT NOT NULL,
            PRIMARY KEY (client_host, info_hash)
        );
        CREATE TABLE IF NOT EXISTS cross_seed_job_log (
            name TEXT PRIMARY KEY,
            last_run REAL NOT NULL
        );",
    )
    .map_err(fail)?;
    let cols: Vec<String> = db.query("PRAGMA table_info(cross_seeds)", p![]).map_err(fail)?.iter().map(|r| r.s("name")).collect();
    for (col, def) in
        [("match_decision", "TEXT NOT NULL DEFAULT ''"), ("result", "TEXT NOT NULL DEFAULT ''"), ("searchee", "TEXT NOT NULL DEFAULT ''")]
    {
        if !cols.iter().any(|c| c == col) {
            db.execute_batch(&format!("ALTER TABLE cross_seeds ADD COLUMN {col} {def}")).map_err(fail)?;
        }
    }
    Ok(())
}

/// Lantern's outcome word for a candidate (the page's dot and the hub's "keep the files" rule): `injected`,
/// `already_there`, `saved`, `files_differ`, `magnet_only` or `failed`.
pub fn outcome_word(decision: Decision, result: Option<InjectionResult>) -> &'static str {
    match (decision, result) {
        (_, Some(InjectionResult::Success)) => "injected",
        (_, Some(InjectionResult::Saved)) => "saved",
        (_, Some(InjectionResult::AlreadyExists)) => "already_there",
        (Decision::SameInfoHash | Decision::InfoHashAlreadyExists, _) => "already_there",
        (Decision::MagnetLink, _) => "magnet_only",
        (Decision::DownloadFailed | Decision::RateLimited | Decision::NoDownloadLink | Decision::BlockedRelease, _) => "failed",
        (d, Some(_)) if d.is_any_match() => "failed",
        _ => "files_differ",
    }
}

/// What an outcome word means, in plain words.
pub fn words(outcome: &str) -> &'static str {
    match outcome {
        "injected" => "Seeding here too, from the files already downloaded",
        "already_there" => "The download client already has this torrent",
        "saved" => "Its .torrent file was saved in the cross-seeds folder, to add by hand",
        "files_differ" => "Not the same files, so it would need a new download",
        "magnet_only" => "This site gives only a magnet link, whose files can't be compared first",
        _ => "Couldn't be checked or added",
    }
}

/// How one search went.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// Copies found: candidates whose name and size fit, so their files were compared.
    pub candidates: usize,
    /// Of them, the ones now seeding too.
    pub injected: usize,
    /// Matches of any kind.
    pub matches: usize,
    /// Indexers that couldn't be searched: (name, why).
    pub failed: Vec<(String, String)>,
    /// Why the searchee wasn't searched at all (cross-seed's pre-filter), if it wasn't.
    pub filtered: Option<String>,
}

/// What a search is recorded under: the grab it is for and the indexer it came from.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// Lantern's download client the searchee is in (0: none or not one of Lantern's).
    pub client_id: i64,
    /// The grab the files belong to (0: none).
    pub grab_id: i64,
    /// The indexer the searchee came from (0: none), which isn't searched.
    pub indexer_id: i64,
}

/// A job that can be run ahead of schedule (cross-seed's `Job`).
#[derive(Debug, Clone)]
pub struct Job {
    /// Its name (`search`).
    pub name: &'static str,
    /// How often it runs, in milliseconds.
    pub cadence: f64,
    /// Whether it is running now.
    pub is_active: bool,
}

/// The torrent clients made from the settings, kept until the settings change.
#[derive(Default)]
struct ClientCache {
    /// The `torrentClients` entries they were made from.
    entries: Vec<String>,
    /// The clients, signed in.
    clients: Vec<Arc<Client>>,
}

/// Cross-seeding over the hub's clients and indexers.
#[derive(Clone)]
pub struct CrossSeed {
    /// The hub: its grabs, clients and indexers.
    pub hub: Hub,
    /// The jobs the daemon API can run ahead of schedule.
    pub jobs: Arc<Mutex<Vec<Job>>>,
    /// cross-seed's torrent clients.
    clients: Arc<tokio::sync::Mutex<ClientCache>>,
    /// `Mutex.CLIENT_INJECTION`: one injection at a time.
    injection: Arc<tokio::sync::Mutex<()>>,
}

/// Now in milliseconds since 1970, as cross-seed counts.
pub fn now_ms() -> f64 {
    crate::auth::now() * 1000.0
}

/// A Lantern download client as a cross-seed `torrentClients` entry: its type, and its address with the user name
/// and password in it (and the RPC path each client type needs). `None` for a Usenet client.
pub fn client_entry(c: &ClientConfig) -> Option<String> {
    let base = c.url.trim().trim_end_matches('/').to_string();
    let mut u = url::Url::parse(&base).ok()?;
    let has_path = !u.path().trim_matches('/').is_empty();
    match c.kind.as_str() {
        "qbittorrent" => {}
        "transmission" if !has_path => u.set_path("/transmission/rpc"),
        "rtorrent" if !has_path => u.set_path("/RPC2"),
        "deluge" if !u.path().ends_with("/json") => {
            let p = format!("{}/json", u.path().trim_end_matches('/'));
            u.set_path(&p);
        }
        "transmission" | "rtorrent" | "deluge" => {}
        _ => return None,
    }
    if c.kind != "deluge" && !c.username.is_empty() {
        let _ = u.set_username(&c.username);
    }
    if !c.password.is_empty() {
        let _ = u.set_password(Some(&c.password));
    }
    let mut s = u.to_string();
    if c.kind == "qbittorrent" && u.path() == "/" {
        s = s.trim_end_matches('/').to_string();
    }
    Some(format!("{}:{s}", c.kind))
}

impl CrossSeed {
    /// Cross-seeding for `hub`, with the jobs ported so far: `search` (Lantern's pass over finished torrents).
    pub fn new(hub: Hub) -> CrossSeed {
        let cadence = RuntimeConfig::default().search_cadence.unwrap_or(86_400_000.0);
        CrossSeed {
            hub,
            jobs: Arc::new(Mutex::new(vec![Job { name: "search", cadence, is_active: false }])),
            clients: Arc::new(tokio::sync::Mutex::new(ClientCache::default())),
            injection: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn db(&self) -> &SqliteDb {
        &self.hub.db
    }

    /// Lantern's enabled torrent download clients as `torrentClients` entries, by priority.
    fn lantern_client_entries(&self) -> Vec<String> {
        let mut list: Vec<ClientConfig> =
            self.hub.clients().unwrap_or_default().into_iter().filter(|c| c.enabled && c.protocol() == Protocol::Torrent).collect();
        list.sort_by_key(|c| (c.priority, c.id));
        list.iter().filter_map(client_entry).collect()
    }

    /// cross-seed's settings from Lantern's, and the problems found in them (cross-seed's words).
    pub fn config(&self) -> (RuntimeConfig, Vec<String>) {
        let (mut c, problems) = RuntimeConfig::from_settings_with(&self.hub.settings, &self.lantern_client_entries());
        c.output_dir = Some(self.output_dir().to_string_lossy().into_owned());
        (c, problems)
    }

    /// `outputDir`: `cross-seeds` in the data folder.
    pub fn output_dir(&self) -> std::path::PathBuf {
        self.hub.data_dir.join("cross-seeds")
    }

    /// Whether it runs by itself (the `cross_seed` setting).
    pub fn enabled(&self) -> bool {
        self.hub.settings.bool("cross_seed")
    }

    /// The wait between two searches (cross-seed's `delay`, seconds), so sites aren't flooded.
    pub fn delay(&self) -> Duration {
        Duration::from_secs_f64(self.config().0.delay.clamp(0.0, 3600.0))
    }

    /// cross-seed's torrent clients for `cfg` (made and signed in once per set of entries).
    pub async fn clients(&self, cfg: &RuntimeConfig) -> Vec<Arc<Client>> {
        let mut cache = self.clients.lock().await;
        if cache.entries != cfg.torrent_clients {
            let made = clients::instantiate(&cfg.torrent_clients);
            for c in &made {
                if let Err(e) = c.validate_config().await {
                    tracing::error!("cross-seed: {e}");
                }
            }
            *cache = ClientCache { entries: cfg.torrent_clients.clone(), clients: made };
        }
        cache.clients.clone()
    }

    /// What `performAction` needs now.
    async fn action_ctx(&self, cfg: &RuntimeConfig) -> action::Ctx {
        action::Ctx { clients: self.clients(cfg).await, db: self.db().clone(), cfg: cfg.clone(), output_dir: self.output_dir() }
    }

    /// The client torrents, refreshed (cross-seed's `indexTorrentsAndDataDirs` for client torrents): only
    /// torrents new or changed since the last look are asked for their files.
    pub async fn index_clients(&self, cfg: &RuntimeConfig) -> Vec<Searchee> {
        let mut all = Vec::new();
        for c in self.clients(cfg).await {
            all.extend(c.get_client_searchees(self.db(), cfg, &SearcheeOptions::default()).await.searchees);
        }
        all
    }

    /// The enabled torrent indexers, but `except`.
    fn torrent_indexers(&self, except: i64) -> Result<Vec<i64>> {
        Ok(self
            .hub
            .indexers
            .list()?
            .into_iter()
            .filter(|ix| ix.enabled && ix.id != except && ix.kind != "newznab")
            .map(|ix| ix.id)
            .collect())
    }

    /// A searchee's search times over `indexers` (cross-seed's `filterTimestamps` query).
    pub fn timestamps(&self, title: &str, indexers: &[i64]) -> Result<Timestamps> {
        if indexers.is_empty() {
            return Ok(Timestamps { earliest_first_search: 0.0, latest_first_search: 0.0, earliest_last_search: 0.0 });
        }
        let rows: HashMap<i64, (f64, f64)> = self
            .db()
            .query("SELECT * FROM cross_seed_timestamps WHERE searchee=?", p![title])?
            .iter()
            .map(|r| (r.i("indexer_id"), (r.f("first_searched"), r.f("last_searched"))))
            .collect();
        let firsts: Vec<f64> = indexers.iter().map(|i| rows.get(i).map(|t| t.0).unwrap_or(MAX_INT)).collect();
        let lasts: Vec<f64> = indexers.iter().map(|i| rows.get(i).map(|t| t.1).unwrap_or(0.0)).collect();
        Ok(Timestamps {
            earliest_first_search: firsts.iter().copied().fold(f64::INFINITY, f64::min),
            latest_first_search: firsts.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            earliest_last_search: lasts.iter().copied().fold(f64::INFINITY, f64::min),
        })
    }

    /// `updateSearchTimestamps`: `title` was searched on `indexers` now.
    fn update_search_timestamps(&self, title: &str, indexers: &[i64]) -> Result<()> {
        let now = now_ms();
        for id in indexers {
            self.db().execute(
                "INSERT INTO cross_seed_timestamps (searchee, indexer_id, first_searched, last_searched) VALUES (?,?,?,?)
                 ON CONFLICT(searchee, indexer_id) DO UPDATE SET last_searched=excluded.last_searched",
                p![title, *id, now, now],
            )?;
        }
        Ok(())
    }

    /// The finished torrent grabs, the least lately searched first, at most `limit`, whose search times let them
    /// be searched now (cross-seed's `excludeOlder` and `excludeRecentSearch`, changed by `o`).
    pub fn due_with(&self, limit: i64, o: &Override) -> Result<Vec<Grab>> {
        let cfg = self.config().0.with(o);
        let rows = self.db().query(
            "SELECT g.* FROM hub_grabs g LEFT JOIN cross_seed_searches s ON s.grab_id = g.id
             WHERE g.protocol='torrent' AND g.state IN ('seeding','completed') AND g.client_id != 0
             ORDER BY COALESCE(s.at, 0), g.finished",
            p![],
        )?;
        let now = now_ms();
        let mut out = Vec::new();
        for g in rows.iter().map(Grab::from_row) {
            if out.len() as i64 >= limit {
                break;
            }
            // searched under its title as cross-seed parses it, as `search_searchee` records it
            let title = searchee::parse_title(&g.title, &[], None).unwrap_or_else(|| g.title.clone());
            let s = Searchee { name: g.title.clone(), title: title.clone(), info_hash: Some(g.infohash.clone()), ..Searchee::default() };
            let t = self.timestamps(&title, &self.torrent_indexers(g.indexer_id)?)?;
            if filter_timestamps(&s, &cfg, t, now, || 0.0).is_ok() {
                out.push(g);
            }
        }
        Ok(out)
    }

    /// [`due_with`](Self::due_with) with the settings as they are.
    pub fn due(&self, limit: i64) -> Result<Vec<Grab>> {
        self.due_with(limit, &Override::default())
    }

    /// `getInfoHashesToExclude`: every torrent in the torrent clients (the client torrent cache).
    fn info_hashes_to_exclude(&self) -> HashSet<String> {
        clients::kept_searchees(self.db()).into_iter().filter_map(|s| s.info_hash).collect()
    }

    /// Searches the other torrent indexers for grab `id`'s release and adds every copy with the same files.
    pub async fn search_grab(&self, id: i64) -> Result<Outcome> {
        self.search_grab_with(id, &Override::default()).await
    }

    /// The info hash of grab `g` in its client (found by Lantern's tag when the grab has none yet).
    async fn grab_hash(&self, g: &Grab) -> Result<String> {
        if !g.client_ref.is_empty() {
            return Ok(g.client_ref.to_lowercase());
        }
        let cfg = self.hub.client(g.client_id)?;
        let backend = self.hub.backend(&cfg)?;
        let items = backend.items().await.map_err(|e| err(502, e))?;
        items
            .into_iter()
            .find(|i| i.tag == g.tag())
            .map(|i| i.id.to_lowercase())
            .ok_or_else(|| err(404, format!("{} doesn't have {} any more, so there is nothing to seed from", cfg.name, g.title)))
    }

    /// [`search_grab`](Self::search_grab) with settings changed for this search.
    pub async fn search_grab_with(&self, id: i64, o: &Override) -> Result<Outcome> {
        let g = self.hub.get_grab(id)?;
        if g.protocol != "torrent" || !matches!(g.state.as_str(), "seeding" | "completed") {
            return Err(err(409, "Only a finished torrent can be cross-seeded"));
        }
        let hash = self.grab_hash(&g).await?;
        let (cfg, _) = self.config();
        let all = self.index_clients(&cfg).await;
        let mut s = all.into_iter().find(|s| s.info_hash.as_deref() == Some(hash.as_str())).ok_or_else(|| {
            let name = self.hub.client(g.client_id).map(|c| c.name).unwrap_or_else(|_| "The download client".into());
            err(404, format!("{name} doesn't have {} any more, so there is nothing to seed from", g.title))
        })?;
        s.label = Label::Search;
        let target = Target { client_id: g.client_id, grab_id: g.id, indexer_id: g.indexer_id };
        let out = self.search_searchee(&s, &target, o, ContentOptions { ignore_cross_seeds: true, ..ContentOptions::default() }).await?;
        self.db().execute(
            "INSERT INTO cross_seed_searches (grab_id, at, times, found) VALUES (?,?,1,?)
             ON CONFLICT(grab_id) DO UPDATE SET at=excluded.at, times=times+1, found=found+excluded.found",
            p![g.id, crate::auth::now(), out.injected as i64],
        )?;
        Ok(out)
    }

    /// The candidates already decided for searchee `title` in a way that doesn't change (a match, or a decision
    /// from the name alone): (indexer, candidate name).
    fn decided(&self, title: &str, grab_id: i64) -> Result<HashSet<(i64, String)>> {
        Ok(self
            .db()
            .query(
                "SELECT indexer_id, title, match_decision, decision FROM cross_seeds WHERE (grab_id=? AND grab_id<>0) OR searchee=?",
                p![grab_id, title],
            )?
            .iter()
            .filter(|r| {
                Decision::parse(&r.s("match_decision")).map(|d| d.is_any_match() || d.is_static()).unwrap_or(r.s("decision") != "failed")
            })
            .map(|r| (r.i("indexer_id"), r.s("title")))
            .collect())
    }

    /// `findOnOtherSites` for one searchee: filters it, searches the other torrent indexers, decides on every
    /// candidate and acts on the matches (`performActions`).
    pub async fn search_searchee(&self, s: &Searchee, target: &Target, o: &Override, opts: ContentOptions) -> Result<Outcome> {
        let (base, _) = self.config();
        let cfg = base.with(o);
        let mut out = Outcome::default();
        if let Err(why) = filter_by_content(s, &cfg, opts) {
            tracing::info!("cross-seed: did not search for {}: {why}", s.title);
            out.filtered = Some(why);
            return Ok(out);
        }
        let indexers = self.torrent_indexers(target.indexer_id)?;
        if indexers.is_empty() {
            return Ok(out);
        }
        let q = Query { kind: SearchKind::Search, keywords: s.title.clone(), limit: 100, ..Query::default() };
        let (found, failed) = self.hub.indexers.search(&indexers, &q, "Lantern cross-seed").await?;
        let names: HashMap<i64, String> = self.hub.indexers.list()?.into_iter().map(|ix| (ix.id, ix.name)).collect();
        let searched: Vec<i64> = indexers.iter().copied().filter(|i| !failed.iter().any(|(n, _)| names.get(i) == Some(n))).collect();
        out.failed = failed;
        let decided = self.decided(&s.title, target.grab_id)?;
        let mut exclude = self.info_hashes_to_exclude();
        for r in found {
            if r.protocol != Protocol::Torrent || r.indexer_id == target.indexer_id || decided.contains(&(r.indexer_id, r.title.clone())) {
                continue;
            }
            let link = if r.link.is_empty() { r.magnet.clone() } else { r.link.clone() };
            let candidate = Candidate {
                name: r.title.clone(),
                guid: r.guid.clone(),
                link,
                tracker: r.indexer.clone(),
                size: r.size.and_then(|v| i64::try_from(v).ok()),
                pub_date: r.date.map(|d| d.timestamp_millis() as f64),
                indexer_id: Some(r.indexer_id),
                cookie: None,
            };
            let (assessment, result, detail) = self.assess_and_act(s, candidate, &exclude, &cfg).await;
            if !matches!(
                assessment.decision,
                Decision::ReleaseGroupMismatch
                    | Decision::ResolutionMismatch
                    | Decision::SourceMismatch
                    | Decision::ProperRepackMismatch
                    | Decision::FuzzySizeMismatch
                    | Decision::NoDownloadLink
            ) {
                out.candidates += 1;
            }
            if assessment.decision.is_any_match() {
                out.matches += 1;
            }
            let hash = assessment.metafile.as_ref().map(|m| m.info_hash.clone()).unwrap_or_else(|| r.infohash.to_lowercase());
            if result == Some(InjectionResult::Success) {
                out.injected += 1;
                exclude.insert(hash.clone());
            }
            self.record(target, s, r.indexer_id, &r.indexer, &r.title, &hash, assessment.decision, result, &detail)?;
        }
        self.update_search_timestamps(&s.title, &searched)?;
        Ok(out)
    }

    /// Keeps one candidate's outcome.
    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        target: &Target,
        s: &Searchee,
        indexer_id: i64,
        indexer: &str,
        title: &str,
        hash: &str,
        decision: Decision,
        result: Option<InjectionResult>,
        detail: &str,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO cross_seeds (grab_id, indexer_id, indexer, title, info_hash, decision, detail, client_id, at, match_decision, result, searchee)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            p![
                target.grab_id,
                indexer_id,
                indexer,
                title,
                hash,
                outcome_word(decision, result),
                detail,
                target.client_id,
                crate::auth::now(),
                decision.as_str(),
                result.map(|r| r.as_str()).unwrap_or(""),
                s.title.clone()
            ],
        )?;
        Ok(())
    }

    /// Fetches a candidate's `.torrent` through its Lantern indexer (or straight from its link with its cookie,
    /// for an announce), as cross-seed's `snatch`.
    async fn snatch(&self, c: Candidate) -> std::result::Result<Metafile, SnatchError> {
        if c.link.starts_with("magnet:") {
            return Err(SnatchError::MagnetLink);
        }
        let bytes = match c.indexer_id {
            Some(ix) => match self.hub.indexers.grab(ix, &c.link, &c.name, "Lantern cross-seed").await {
                Ok(Grabbed::File(b)) => b,
                Ok(Grabbed::Magnet(_)) => return Err(SnatchError::MagnetLink),
                Err(e) if e.status.as_u16() == 429 => return Err(SnatchError::RateLimited),
                Err(e) => return Err(SnatchError::Failed(e.detail)),
            },
            None => {
                let mut req = crate::hub::clients::http_client(false).get(&c.link);
                if let Some(cookie) = &c.cookie {
                    req = req.header("cookie", cookie);
                }
                let resp = req.send().await.map_err(|e| SnatchError::Failed(e.to_string()))?;
                if resp.status().as_u16() == 429 {
                    return Err(SnatchError::RateLimited);
                }
                if let Some(loc) = resp.headers().get("location").and_then(|v| v.to_str().ok()) {
                    if loc.starts_with("magnet:") {
                        return Err(SnatchError::MagnetLink);
                    }
                }
                if !resp.status().is_success() {
                    return Err(SnatchError::Failed(format!("{} answered {}", c.tracker, resp.status())));
                }
                resp.bytes().await.map_err(|e| SnatchError::Failed(e.to_string()))?.to_vec()
            }
        };
        Metafile::decode(&bytes).map_err(SnatchError::Failed)
    }

    /// Decides on one candidate and, for a match, acts on it: (assessment, action result, why in cross-seed's
    /// words).
    async fn assess_and_act(
        &self,
        s: &Searchee,
        candidate: Candidate,
        exclude: &HashSet<String>,
        cfg: &RuntimeConfig,
    ) -> (Assessment, Option<InjectionResult>, String) {
        let shown = candidate.clone();
        let mut input = MetaOrCandidate::Candidate(candidate);
        let a = assess_candidate(&mut input, s, exclude, &cfg.block_list, cfg, |c| self.snatch(c)).await;
        let mut detail = reason(a.decision, s, &shown, a.metafile.as_ref(), cfg);
        let result = match (&a.metafile, a.decision.is_any_match()) {
            (Some(meta), true) => {
                let r = self.perform_action(meta, a.decision, s, &shown.tracker, cfg).await;
                let note = action_words(r.action_result, r.destination_dir.as_deref(), r.linked_new_files);
                if !note.is_empty() {
                    detail = format!("{detail}. {note}");
                }
                Some(r.action_result)
            }
            _ => None,
        };
        (a, result, detail)
    }

    /// `performAction`: saves or injects a match, one at a time (cross-seed's `CLIENT_INJECTION` lock).
    pub async fn perform_action(
        &self,
        meta: &Metafile,
        decision: Decision,
        s: &Searchee,
        tracker: &str,
        cfg: &RuntimeConfig,
    ) -> ActionReturn {
        let ctx = self.action_ctx(cfg).await;
        let _one = self.injection.lock().await;
        action::perform_action_without_mutex(&ctx, meta, decision, s, tracker, None, true).await
    }

    /// The grab whose torrent has `info_hash`, if Lantern grabbed it.
    fn grab_of(&self, info_hash: Option<&str>) -> Result<Target> {
        let Some(h) = info_hash else { return Ok(Target::default()) };
        let grab = self
            .db()
            .one("SELECT * FROM hub_grabs WHERE lower(client_ref)=? AND protocol='torrent' ORDER BY id DESC", p![h.to_lowercase()])?
            .map(|r| Grab::from_row(&r));
        Ok(Target {
            client_id: grab.as_ref().map(|g| g.client_id).unwrap_or(0),
            grab_id: grab.as_ref().map(|g| g.id).unwrap_or(0),
            indexer_id: grab.as_ref().map(|g| g.indexer_id).unwrap_or(0),
        })
    }

    /// `searchForLocalTorrentByCriteria` by info hash: the client torrent with it, searched on the other sites.
    /// `None` when every searchee was left out by the pre-filter (cross-seed logs nothing found then).
    pub async fn search_info_hash(&self, info_hash: &str, o: &Override, ignore_cross_seeds: bool) -> Result<Option<usize>> {
        let (cfg, _) = self.config();
        let want = info_hash.to_lowercase();
        let Some(mut s) = self.index_clients(&cfg).await.into_iter().find(|s| s.info_hash.as_deref() == Some(want.as_str())) else {
            return Err(err(404, format!("No torrent client has a torrent with the info hash {}", utils::sanitize_info_hash(info_hash))));
        };
        s.label = Label::Webhook;
        let target = self.grab_of(Some(&want))?;
        let opts = ContentOptions { ignore_cross_seeds, allow_season_pack_episodes: true, block_list_only: false };
        let out = self.search_searchee(&s, &target, o, opts).await?;
        Ok(if out.filtered.is_some() { None } else { Some(out.matches) })
    }

    /// `checkNewCandidateMatch` for an announce: the client torrents like the candidate by name, decided on one at
    /// a time until one matches and is added: (decision, action result).
    pub async fn check_new_candidate_match(&self, candidate: Candidate) -> Result<(Option<Decision>, Option<InjectionResult>)> {
        let (cfg, _) = self.config();
        let clients = self.clients(&cfg).await;
        let mut searchees: Vec<Searchee> = self
            .index_clients(&cfg)
            .await
            .into_iter()
            .filter(|s| similar_by_name(&candidate.name, &s.name))
            .map(|s| Searchee { label: Label::Announce, ..s })
            .filter(|s| filter_by_content(s, &cfg, ContentOptions::default()).is_ok())
            .collect();
        let unique = prefilter::filter_dupes_from_similar(&searchees);
        searchees.retain(|s| unique.contains(s));
        searchees.sort_by_key(|s| {
            (clients::by_client_host_priority(&clients, s.client_host.as_deref()), s.info_hash.is_none(), std::cmp::Reverse(s.files.len()))
        });
        let exclude = self.info_hashes_to_exclude();
        let mut decision = None;
        let mut action_result: Option<InjectionResult> = None;
        for s in &searchees {
            let (a, result, detail) = self.assess_and_act(s, candidate.clone(), &exclude, &cfg).await;
            let hash = a.metafile.as_ref().map(|m| m.info_hash.clone()).unwrap_or_default();
            let target = Target { indexer_id: 0, ..self.grab_of(s.info_hash.as_deref())? };
            self.record(&target, s, 0, &candidate.tracker, &candidate.name, &hash, a.decision, result, &detail)?;
            if a.decision == Decision::InfoHashAlreadyExists {
                decision = Some(a.decision);
                break;
            }
            if !a.decision.is_any_match() {
                continue;
            }
            let r = result.unwrap_or(InjectionResult::Failure);
            if r == InjectionResult::Saved {
                decision = Some(a.decision);
                action_result = Some(r);
                break;
            }
            if a.decision != Decision::MatchPartial && matches!(r, InjectionResult::Success | InjectionResult::AlreadyExists) {
                decision = Some(a.decision);
                action_result = Some(InjectionResult::Success);
                break;
            }
            if action_result == Some(InjectionResult::Success) {
                continue;
            }
            if action_result.is_some() && r == InjectionResult::Failure {
                continue;
            }
            if action_result == Some(InjectionResult::AlreadyExists) && r == InjectionResult::TorrentNotComplete {
                continue;
            }
            decision = Some(a.decision);
            action_result = Some(r);
        }
        Ok((decision, action_result))
    }

    /// One pass: searches every grab whose turn has come, waiting [`delay`](Self::delay) between them.
    pub async fn run(&self, limit: i64) -> Result<Outcome> {
        self.run_with(limit, &Override::default()).await
    }

    /// [`run`](Self::run) with settings changed for this pass (`POST /api/job`'s `ignoreExclude…`).
    pub async fn run_with(&self, limit: i64, o: &Override) -> Result<Outcome> {
        let mut all = Outcome::default();
        for (n, g) in self.due_with(limit, o)?.into_iter().enumerate() {
            if n > 0 {
                tokio::time::sleep(self.delay()).await;
            }
            match self.search_grab_with(g.id, o).await {
                Ok(o) => {
                    all.candidates += o.candidates;
                    all.injected += o.injected;
                    all.matches += o.matches;
                    all.failed.extend(o.failed);
                }
                // a grab that can't be searched now (gone from its client) waits for its next turn
                Err(e) => {
                    tracing::info!("cross-seed: {}: {}", g.title, e.detail);
                    let _ = self.db().execute(
                        "INSERT INTO cross_seed_searches (grab_id, at, times) VALUES (?,?,1)
                         ON CONFLICT(grab_id) DO UPDATE SET at=excluded.at, times=times+1",
                        p![g.id, crate::auth::now()],
                    );
                }
            }
        }
        Ok(all)
    }

    /// The latest outcomes, newest first, as the page shows them.
    pub fn recent(&self, limit: i64) -> Result<Vec<Value>> {
        let rows = self.db().query(
            "SELECT x.*, g.title AS grab_title FROM cross_seeds x LEFT JOIN hub_grabs g ON g.id = x.grab_id ORDER BY x.id DESC LIMIT ?",
            p![limit.clamp(1, 500)],
        )?;
        Ok(rows.iter().map(outcome_json).collect())
    }

    /// The numbers: torrents added, grabs searched, grabs waiting.
    pub fn stats(&self) -> Result<Value> {
        let n = |sql: &str| -> Result<i64> { Ok(self.db().one(sql, p![])?.map(|r| r.i("n")).unwrap_or(0)) };
        Ok(json!({
            "injected": n("SELECT COUNT(*) n FROM cross_seeds WHERE decision='injected'")?,
            "searched": n("SELECT COUNT(*) n FROM cross_seed_searches")?,
            "waiting": self.due(10_000)?.len(),
        }))
    }
}

/// What was done with a match, in plain words, for the page.
pub fn action_words(result: InjectionResult, destination_dir: Option<&str>, linked: bool) -> String {
    match result {
        InjectionResult::Success if linked => format!("Added from files linked into {}", destination_dir.unwrap_or_default()),
        InjectionResult::Success => String::new(),
        InjectionResult::AlreadyExists if linked => "The client already had it; new files were linked and it is being checked".into(),
        InjectionResult::AlreadyExists => String::new(),
        InjectionResult::Saved => "Its .torrent file was saved in the cross-seeds folder (action save)".into(),
        InjectionResult::TorrentNotComplete => "The torrent it comes from isn't finished yet, so it was kept to add later".into(),
        InjectionResult::Failure => "Adding it failed (the log says why), so it was kept to add later".into(),
    }
}

/// `getSimilarByName`, simplified to the names of client torrents: the same key title and element (season,
/// episode, year or release) by cross-seed's keys, else cross-seed's fuzzy title comparison.
pub fn similar_by_name(candidate: &str, have: &str) -> bool {
    let a = utils::strip_extension(candidate);
    let b = utils::strip_extension(have);
    let keys = |s: &str| {
        searchee::get_episode_keys(s)
            .or_else(|| searchee::get_season_keys(s))
            .or_else(|| searchee::get_movie_keys(s))
            .or_else(|| searchee::get_anime_keys(s))
    };
    match (keys(&a), keys(&b)) {
        (Some(x), Some(y)) => x.element == y.element && x.key_titles.iter().any(|k| y.key_titles.contains(k)),
        _ => utils::are_media_titles_similar(&a, &b),
    }
}

/// The paths of `files` (as the client sees them) inside `save_path`.
pub fn relative(files: &[(String, i64)], save_path: &str) -> Vec<(String, i64)> {
    let base = format!("{}/", save_path.trim_end_matches('/'));
    files.iter().map(|(p, s)| (p.strip_prefix(&base).unwrap_or(p).to_string(), *s)).collect()
}

fn outcome_json(r: &Row) -> Value {
    json!({
        "id": r.i("id"),
        "grab_id": r.i("grab_id"),
        "grab_title": r.s("grab_title"),
        "searchee": r.s("searchee"),
        "indexer_id": r.i("indexer_id"),
        "indexer": r.s("indexer"),
        "title": r.s("title"),
        "info_hash": r.s("info_hash"),
        "decision": r.s("decision"),
        "match_decision": r.s("match_decision"),
        "result": r.s("result"),
        "words": words(&r.s("decision")),
        "detail": r.s("detail"),
        "at": r.f("at"),
    })
}

/// Runs cross-seeding: the daemon API on its own port (cross-seed's `port`, 2468 by default; 0 for none), and a
/// pass every ten minutes while the `cross_seed` setting is on, a handful of grabs at a time.
pub fn spawn(xs: CrossSeed) {
    if let Some(port) = xs.config().0.port {
        let app = daemon::router(xs.clone());
        tokio::spawn(async move {
            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => {
                    tracing::info!("cross-seed API listening on {addr}");
                    let _ = axum::serve(l, app).with_graceful_shutdown(crate::shutdown_signal()).await;
                }
                Err(e) => tracing::warn!(
                    "The cross-seed API can't use port {port}: {e}. Set APP_CROSS_SEED_PORT to a free port, or 0 to turn it off."
                ),
            }
        });
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            if xs.enabled() {
                if let Err(e) = xs.run(5).await {
                    tracing::warn!("cross-seed: {}", e.detail);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_words_for_every_decision() {
        assert_eq!(outcome_word(Decision::Match, Some(InjectionResult::Success)), "injected");
        assert_eq!(outcome_word(Decision::Match, Some(InjectionResult::AlreadyExists)), "already_there");
        assert_eq!(outcome_word(Decision::MatchSizeOnly, Some(InjectionResult::Failure)), "failed");
        assert_eq!(outcome_word(Decision::InfoHashAlreadyExists, None), "already_there");
        assert_eq!(outcome_word(Decision::MagnetLink, None), "magnet_only");
        assert_eq!(outcome_word(Decision::SizeMismatch, None), "files_differ");
        assert_eq!(outcome_word(Decision::DownloadFailed, None), "failed");
        for d in Decision::ALL {
            assert!(!words(outcome_word(d, None)).is_empty());
        }
    }

    #[test]
    fn names_alike_for_announces() {
        assert!(similar_by_name("Show.Name.S01E02.1080p.WEB-GRP", "Show Name S01E02 720p"));
        assert!(!similar_by_name("Show.Name.S01E02.1080p.WEB-GRP", "Show.Name.S01E03.1080p.WEB-GRP"));
        assert!(similar_by_name("Movie.Name.2020.1080p.BluRay-GRP", "Movie Name (2020)"));
        assert!(!similar_by_name("Movie.Name.2020.1080p", "Other.Film.2020.1080p"));
    }

    #[test]
    fn paths_inside_the_save_folder() {
        let have = relative(&[("/dl/tv/Show.S01/e1.mkv".to_string(), 10)], "/dl/tv/");
        assert_eq!(have[0].0, "Show.S01/e1.mkv");
    }

    #[test]
    fn match_modes_from_settings_text() {
        assert_eq!(constants::MatchMode::parse("partial"), Some(constants::MatchMode::Partial));
    }
}
