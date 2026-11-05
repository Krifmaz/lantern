// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Applications/ApplicationService.cs, AppIndexerMap.cs,
// AppIndexerMapService.cs, AppIndexerMapRepository.cs, ApplicationFactory.cs, ApplicationIndexerSyncCommand.cs,
// ApplicationBase.cs), GPL-3.0.

//! App sync: Lantern keeps its indexers in the applications, as Prowlarr does.
//!
//! - An application that is added or changed (and on) gets every enabled indexer it should have.
//! - An indexer that is added goes to every application that syncs; one that is deleted is removed from them; one
//!   that is changed is brought up to date in every application on Full Sync.
//! - `ApplicationIndexerSync` (every six hours, and from the web app's Sync App Indexers button) does all of it,
//!   and removes from the apps the indexers Lantern no longer has or no longer sends them.
//! - A new API key is sent to every app straight away.
//!
//! An application with tags only gets the indexers that share one of its tags. An application that failed is left
//! alone for a while (see [`super::status`]). Which app indexer is which Lantern indexer is kept in
//! `prowlarr_app_indexer_maps` (Prowlarr's `ApplicationIndexerMapping`).
//!
//! Lantern's indexers have no change events of their own, so a watcher compares them every few seconds and handles
//! what changed as Prowlarr handles its indexer events (docs/DECISIONS.md).

use super::applications::{self, Applications, Client, SyncLevel};
use super::categories::Tree;
use super::http::HttpError;
use super::provider::{self, Definition};
use super::{profiles, status};
use crate::AppState;
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// One syncing app at work: the server, the app, Lantern's API key.
#[derive(Clone)]
pub struct Ctx {
    /// The server.
    pub st: AppState,
    /// The application.
    pub app: Definition,
    /// The API key the apps use to reach Lantern.
    pub api_key: String,
}

/// An app indexer that points at Lantern, as the app lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteMap {
    /// Lantern's indexer, from the indexer's address.
    pub indexer_id: i64,
    /// The app's id for it (0 for Mylar and LazyLibrarian).
    pub remote_id: i64,
    /// The app's type and name for it (Mylar and LazyLibrarian).
    pub remote_name: Option<String>,
}

/// A saved mapping (`AppIndexerMap`).
#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    /// Its id.
    pub id: i64,
    /// Lantern's indexer.
    pub indexer_id: i64,
    /// The application.
    pub app_id: i64,
    /// The app's id for the indexer.
    pub remote_id: i64,
    /// The app's type and name for it.
    pub remote_name: Option<String>,
}

/// The mappings' table.
pub const MAPS_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS prowlarr_app_indexer_maps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    indexer_id INTEGER NOT NULL,
    app_id INTEGER NOT NULL,
    remote_indexer_id INTEGER NOT NULL DEFAULT 0,
    remote_indexer_name TEXT
);";

/// The mappings of one application (`GetMappingsForApp`).
pub fn maps_for_app(db: &SqliteDb, app_id: i64) -> kit_server_core::Result<Vec<Map>> {
    Ok(db
        .query("SELECT * FROM prowlarr_app_indexer_maps WHERE app_id=? ORDER BY id", p![app_id])?
        .iter()
        .map(|r| Map {
            id: r.i("id"),
            indexer_id: r.i("indexer_id"),
            app_id: r.i("app_id"),
            remote_id: r.i("remote_indexer_id"),
            remote_name: r.get("remote_indexer_name").and_then(Value::as_str).map(str::to_string),
        })
        .collect())
}

/// Saves a mapping; its id.
pub fn insert_map(db: &SqliteDb, app_id: i64, indexer_id: i64, remote_id: i64, remote_name: Option<&str>) -> kit_server_core::Result<i64> {
    db.execute(
        "INSERT INTO prowlarr_app_indexer_maps (indexer_id, app_id, remote_indexer_id, remote_indexer_name) VALUES (?,?,?,?)",
        p![indexer_id, app_id, remote_id, remote_name.map(str::to_string)],
    )
}

/// Saves a changed mapping.
pub fn update_map(db: &SqliteDb, m: &Map) -> kit_server_core::Result<()> {
    db.execute(
        "UPDATE prowlarr_app_indexer_maps SET indexer_id=?, app_id=?, remote_indexer_id=?, remote_indexer_name=? WHERE id=?",
        p![m.indexer_id, m.app_id, m.remote_id, m.remote_name.clone(), m.id],
    )?;
    Ok(())
}

/// Deletes a mapping.
pub fn delete_map(db: &SqliteDb, id: i64) -> kit_server_core::Result<()> {
    db.execute("DELETE FROM prowlarr_app_indexer_maps WHERE id=?", p![id])?;
    Ok(())
}

/// Deletes every mapping of an application (`DeleteAllForApp`).
pub fn delete_maps_for_app(db: &SqliteDb, app_id: i64) -> kit_server_core::Result<()> {
    db.execute("DELETE FROM prowlarr_app_indexer_maps WHERE app_id=?", p![app_id])?;
    Ok(())
}

/// The indexer id at the end of an app indexer's address (`/12/`, `/12/api`): Prowlarr's `AppIndexerRegex`.
pub fn indexer_id_in(address: &str) -> Option<i64> {
    let re = regex::Regex::new(r"(?i)/(?P<indexer>\d{1,3})(?:/(?:api)?/?)?$").expect("a fixed pattern");
    re.captures(address)?.name("indexer")?.as_str().parse().ok()
}

/// The app profile an indexer syncs with.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// RSS on in the app.
    pub enable_rss: bool,
    /// Automatic search on in the app.
    pub enable_automatic_search: bool,
    /// Interactive search on in the app.
    pub enable_interactive_search: bool,
    /// The minimum seeders the app asks for.
    pub minimum_seeders: i64,
}

/// One of Lantern's indexers as app sync sees it (Prowlarr's `IndexerDefinition` and its capabilities).
#[derive(Debug, Clone, PartialEq)]
pub struct SyncIndexer {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Whether it is on.
    pub enable: bool,
    /// Its priority.
    pub priority: i64,
    /// A torrent indexer (else Usenet).
    pub torrent: bool,
    /// Its tags, by label.
    pub tags: Vec<String>,
    /// Its search modes with parameters (`search`, `tv-search`, …).
    pub modes: Vec<String>,
    /// Its categories.
    pub categories: Tree,
    /// Its app profile.
    pub profile: Profile,
    /// Its own minimum seeders for apps (`torrentBaseSettings.appMinimumSeeders`).
    pub app_minimum_seeders: Option<i64>,
    /// Its seed ratio (`torrentBaseSettings.seedRatio`).
    pub seed_ratio: Option<f64>,
    /// Its seed time in minutes (`torrentBaseSettings.seedTime`).
    pub seed_time: Option<i64>,
    /// Its pack seed time in minutes (`torrentBaseSettings.packSeedTime`).
    pub pack_seed_time: Option<i64>,
    /// Whether its settings pass their checks (an indexer whose settings can't be opened does not).
    pub valid: bool,
}

impl SyncIndexer {
    /// Whether it has search mode `mode` (`TvSearchAvailable` and the like).
    pub fn has_mode(&self, mode: &str) -> bool {
        self.modes.iter().any(|m| m == mode)
    }

    /// The indexer Prowlarr's app test sends: `Test`, Usenet, turned off, every standard category.
    pub fn test() -> SyncIndexer {
        SyncIndexer {
            id: 0,
            name: "Test".into(),
            enable: false,
            priority: 25,
            torrent: false,
            tags: Vec::new(),
            modes: Vec::new(),
            categories: Tree::of(super::categories::all()),
            profile: Profile { enable_rss: false, enable_automatic_search: false, enable_interactive_search: false, minimum_seeders: 0 },
            app_minimum_seeders: None,
            seed_ratio: None,
            seed_time: None,
            pack_seed_time: None,
            valid: true,
        }
    }

    /// Lantern's indexer `ix` for app sync, with its app profile (the `appProfileId` setting, else profile 1, else
    /// the first one).
    pub fn of(st: &AppState, ix: &crate::indexers::Indexer, profiles: &[profiles::AppProfile]) -> SyncIndexer {
        let caps = st.indexers.capabilities(ix);
        let setting = |k: &str| ix.settings.get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let profile_id = setting("appProfileId").and_then(|v| v.parse::<i64>().ok()).unwrap_or(1);
        let p = profiles.iter().find(|p| p.id == profile_id).or_else(|| profiles.first());
        let profile = p.map_or(
            Profile { enable_rss: true, enable_automatic_search: true, enable_interactive_search: true, minimum_seeders: 1 },
            |p| Profile {
                enable_rss: p.enable_rss,
                enable_automatic_search: p.enable_automatic_search,
                enable_interactive_search: p.enable_interactive_search,
                minimum_seeders: p.minimum_seeders,
            },
        );
        SyncIndexer {
            id: ix.id,
            name: ix.name.clone(),
            enable: ix.enabled,
            priority: ix.priority,
            torrent: matches!(ix.protocol(), crate::indexers::release::Protocol::Torrent),
            tags: super::super::tags::labels_of(&ix.tags),
            // SearchAvailable, TvSearchAvailable, MovieSearchAvailable, MusicSearchAvailable, BookSearchAvailable
            modes: [
                ("search", caps.search_available()),
                ("tv-search", !caps.tv_params.is_empty()),
                ("movie-search", !caps.movie_params.is_empty()),
                ("music-search", !caps.music_params.is_empty()),
                ("book-search", !caps.book_params.is_empty()),
            ]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(m, _)| m.to_string())
            .collect(),
            categories: Tree::of(caps.categories.list(false).iter().map(|c| c.id)),
            profile,
            app_minimum_seeders: setting("torrentBaseSettings.appMinimumSeeders").and_then(|v| v.parse().ok()),
            seed_ratio: setting("torrentBaseSettings.seedRatio").and_then(|v| v.parse().ok()),
            seed_time: setting("torrentBaseSettings.seedTime").and_then(|v| v.parse().ok()),
            pack_seed_time: setting("torrentBaseSettings.packSeedTime").and_then(|v| v.parse().ok()),
            valid: ix.locked.is_empty(),
        }
    }
}

/// Every one of Lantern's indexers for app sync.
pub fn indexers(st: &AppState) -> kit_server_core::Result<Vec<SyncIndexer>> {
    let profiles = profiles::all(&st.db)?;
    Ok(st.indexers.list()?.iter().map(|ix| SyncIndexer::of(st, ix, &profiles)).collect())
}

/// Lets one sync run at a time.
static SYNC: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The application's tags, by label.
fn app_tag_labels(db: &SqliteDb, app: &Definition) -> Vec<String> {
    let tags = super::super::tags::list(db).unwrap_or_default();
    app.tags.iter().filter_map(|id| tags.iter().find(|(t, _)| t == id).map(|(_, l)| l.clone())).collect()
}

/// Whether the app should have the indexer (`ShouldHandleIndexer`): its settings are fine, and the app has no
/// tags or shares one with it.
fn should_handle(db: &SqliteDb, app: &Definition, ix: &SyncIndexer) -> bool {
    if !ix.valid {
        tracing::debug!("Indexer {} [{}] has invalid settings.", ix.name, ix.id);
        return false;
    }
    if app.tags.is_empty() {
        tracing::debug!("No tags set to application {}.", app.label());
        return true;
    }
    let labels = app_tag_labels(db, app);
    let shared = labels.iter().filter(|l| ix.tags.contains(l)).count();
    if shared > 0 {
        tracing::debug!("Application {} and indexer {} [{}] have {shared} intersecting (matching) tags.", app.label(), ix.name, ix.id);
        return true;
    }
    tracing::debug!(
        "Application {} does not have any intersecting (matching) tags with {} [{}]. Indexer will neither be synced to nor removed from the application.",
        app.label(),
        ix.name,
        ix.id
    );
    false
}

/// Records how a call to the app went (`ExecuteAction`): a success steps the back-off down; a failure steps it up
/// (a connection failure only starts it; a 429 waits at least what the app asked, or an hour). The result, `None`
/// when it failed.
fn record<T>(ctx: &Ctx, result: Result<T, HttpError>) -> Option<T> {
    let db = &ctx.st.db;
    let store = status::APPLICATION_STATUS;
    let name = ctx.app.implementation.clone();
    match result {
        Ok(v) => {
            store.record_success(db, ctx.app.id);
            Some(v)
        }
        Err(e) => {
            match &e {
                HttpError::Connect(_) => store.record_failure(db, ctx.app.id, 0.0, false),
                HttpError::Status { code: 429, retry_after, .. } => {
                    store.record_failure(db, ctx.app.id, retry_after.filter(|s| *s > 0.0).unwrap_or(3600.0), true);
                    tracing::warn!("API Request Limit reached for {name}");
                    return None;
                }
                _ => store.record_failure(db, ctx.app.id, 0.0, true),
            }
            let m = e.message();
            if m.contains("502") || m.contains("503") || m.contains("timed out") {
                tracing::warn!("{name} server is currently unavailable. {m}");
            } else {
                tracing::warn!("{name} {m}");
            }
            None
        }
    }
}

/// The context of an app.
fn ctx_of(st: &AppState, app: &Definition) -> Ctx {
    Ctx { st: st.clone(), app: app.clone(), api_key: st.settings.str("api_key") }
}

/// The applications that sync now (`SyncEnabled`): on, with settings that pass their checks, not held back.
pub fn sync_enabled(st: &AppState) -> kit_server_core::Result<Vec<Definition>> {
    let blocked = status::APPLICATION_STATUS.blocked(&st.db)?;
    Ok(provider::all::<Applications>(&st.db)?
        .into_iter()
        .filter(|a| SyncLevel::of(a) != SyncLevel::Disabled && provider::settings_valid::<Applications>(a))
        .filter(|a| {
            let held = blocked.iter().any(|b| b.provider_id == a.id && b.disabled_till.is_some());
            if held {
                tracing::debug!("Temporarily ignoring application {} due to recent failures.", a.label());
            }
            !held
        })
        .collect())
}

/// Syncs `indexers` to `apps` (`SyncIndexers`): mappings the app has but Lantern lost are kept, mapped indexers
/// are brought up to date on Full Sync, the others added; with `remove_remote` the app's copies of indexers Lantern
/// no longer has (or no longer sends it) are removed.
pub async fn sync_indexers(st: &AppState, apps: &[Definition], indexers: &[SyncIndexer], remove_remote: bool, force: bool) {
    let mut sorted: Vec<&SyncIndexer> = indexers.iter().collect();
    sorted.sort_by_key(|i| i.name.to_lowercase());
    for app in apps {
        let Some(client) = Client::of(&app.implementation) else { continue };
        let ctx = ctx_of(st, app);
        let db = &st.db;
        let mut maps = maps_for_app(db, app.id).unwrap_or_default();
        let Some(remote) = record(&ctx, client.mappings(&ctx).await) else { continue };
        for m in remote {
            let known = maps.iter().any(|x| {
                (x.remote_id > 0 && x.remote_id == m.remote_id)
                    || (x.remote_name.as_deref().is_some_and(|n| !n.trim().is_empty()) && x.remote_name == m.remote_name)
            });
            if !known {
                if let Ok(id) = insert_map(db, app.id, m.indexer_id, m.remote_id, m.remote_name.as_deref()) {
                    maps.push(Map { id, indexer_id: m.indexer_id, app_id: app.id, remote_id: m.remote_id, remote_name: m.remote_name });
                }
            }
        }
        let full = SyncLevel::of(app) == SyncLevel::FullSync;
        for ix in &sorted {
            if maps.iter().any(|m| m.indexer_id == ix.id) {
                if full && should_handle(db, app, ix) {
                    record(&ctx, client.update(&ctx, ix, force).await);
                }
            } else if ix.enable && should_handle(db, app, ix) {
                record(&ctx, client.add(&ctx, ix).await);
            }
        }
        if remove_remote {
            let all = self::indexers(st).unwrap_or_default();
            for m in &maps {
                match all.iter().find(|i| i.id == m.indexer_id) {
                    None => {
                        tracing::warn!(
                            "Indexer with the ID {} was found within {} but is no longer defined within Prowlarr, this is being removed.",
                            m.indexer_id,
                            app.label()
                        );
                        record(&ctx, client.remove_indexer(&ctx, m.indexer_id).await);
                    }
                    Some(ix) if full && !should_handle(db, app, ix) => {
                        tracing::warn!(
                            "Indexer with the ID {} was found within {} but is no longer handled by Prowlarr, this is being removed.",
                            m.indexer_id,
                            app.label()
                        );
                        record(&ctx, client.remove_indexer(&ctx, m.indexer_id).await);
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// An application was added or changed: when it is on, it gets Lantern's enabled indexers.
pub async fn app_saved(st: &AppState, app: &Definition) {
    if SyncLevel::of(app) == SyncLevel::Disabled {
        return;
    }
    let _guard = SYNC.lock().await;
    let enabled: Vec<SyncIndexer> = indexers(st).unwrap_or_default().into_iter().filter(|i| i.enable).collect();
    sync_indexers(st, std::slice::from_ref(app), &enabled, false, false).await;
}

/// An indexer was added: every syncing app that should have it gets it.
pub async fn indexer_added(st: &AppState, ix: &SyncIndexer) {
    let _guard = SYNC.lock().await;
    for app in sync_enabled(st).unwrap_or_default() {
        if should_handle(&st.db, &app, ix) {
            if let Some(client) = Client::of(&app.implementation) {
                let ctx = ctx_of(st, &app);
                record(&ctx, client.add(&ctx, ix).await);
            }
        }
    }
}

/// An indexer was deleted: every syncing app loses it.
pub async fn indexer_deleted(st: &AppState, id: i64) {
    let _guard = SYNC.lock().await;
    for app in sync_enabled(st).unwrap_or_default() {
        if let Some(client) = Client::of(&app.implementation) {
            let ctx = ctx_of(st, &app);
            record(&ctx, client.remove_indexer(&ctx, id).await);
        }
    }
}

/// An indexer was changed: every syncing app on Full Sync gets the change.
pub async fn indexer_updated(st: &AppState, ix: &SyncIndexer) {
    let _guard = SYNC.lock().await;
    let apps: Vec<Definition> =
        sync_enabled(st).unwrap_or_default().into_iter().filter(|a| SyncLevel::of(a) == SyncLevel::FullSync).collect();
    sync_indexers(st, &apps, std::slice::from_ref(ix), false, false).await;
}

/// Syncs every indexer to every syncing app, removing what Lantern no longer has (`ApplicationIndexerSyncCommand`;
/// also after a new API key, with `force`).
pub async fn sync_all(st: &AppState, force: bool) {
    let _guard = SYNC.lock().await;
    let apps = sync_enabled(st).unwrap_or_default();
    let all = indexers(st).unwrap_or_default();
    sync_indexers(st, &apps, &all, true, force).await;
}

/// What the watcher saw last: each indexer's fingerprint, and the API key.
#[derive(Default)]
struct Seen {
    /// id → fingerprint.
    indexers: HashMap<i64, String>,
    /// The API key.
    api_key: String,
    /// Whether it looked at least once.
    primed: bool,
}

/// What each server's watcher saw last, by data folder (tests run several servers in one process).
static SEEN: Mutex<Option<HashMap<std::path::PathBuf, Seen>>> = Mutex::new(None);

/// What makes an indexer different for app sync: everything but its failures and cookies.
fn fingerprint(ix: &crate::indexers::Indexer) -> String {
    let settings = serde_json::to_string(&ix.settings).unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        ix.name,
        ix.enabled,
        ix.priority,
        ix.tags,
        ix.kind,
        ix.definition,
        ix.base_url,
        settings,
        ix.locked.is_empty()
    )
}

/// Compares Lantern's indexers and API key with what it saw last and handles what changed, as Prowlarr handles its
/// indexer and API key events. The first look only remembers.
pub async fn check_changes(st: &AppState) {
    let Ok(list) = st.indexers.list() else { return };
    let api_key = st.settings.str("api_key");
    let now: HashMap<i64, String> = list.iter().map(|i| (i.id, fingerprint(i))).collect();
    let (added, changed, deleted, key_changed) = {
        let mut guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let seen = guard.get_or_insert_with(HashMap::new).entry(st.data_dir.clone()).or_default();
        if !seen.primed {
            *seen = Seen { indexers: now, api_key, primed: true };
            return;
        }
        let added: Vec<i64> = now.keys().filter(|id| !seen.indexers.contains_key(id)).copied().collect();
        let changed: Vec<i64> =
            now.iter().filter(|(id, f)| seen.indexers.get(id).is_some_and(|old| old != *f)).map(|(id, _)| *id).collect();
        let deleted: Vec<i64> = seen.indexers.keys().filter(|id| !now.contains_key(id)).copied().collect();
        let key_changed = seen.api_key != api_key;
        seen.indexers = now;
        seen.api_key = api_key;
        (added, changed, deleted, key_changed)
    };
    if added.is_empty() && changed.is_empty() && deleted.is_empty() && !key_changed {
        return;
    }
    let all = indexers(st).unwrap_or_default();
    let mut ids = added.clone();
    ids.sort();
    for id in ids {
        if let Some(ix) = all.iter().find(|i| i.id == id) {
            indexer_added(st, ix).await;
        }
    }
    let mut ids = changed.clone();
    ids.sort();
    for id in ids {
        if let Some(ix) = all.iter().find(|i| i.id == id) {
            indexer_updated(st, ix).await;
        }
    }
    let mut ids = deleted.clone();
    ids.sort();
    for id in ids {
        indexer_deleted(st, id).await;
    }
    if key_changed {
        sync_all(st, true).await;
    }
}

/// Forgets what the watcher saw for this server.
pub fn reset_watcher(st: &AppState) {
    if let Some(map) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.remove(&st.data_dir);
    }
}

/// The command's body as Prowlarr's scheduler queues it.
fn scheduled_body(last_execution: f64) -> Value {
    json!({
        "forceSync": false,
        "sendUpdatesToClient": true,
        "updateScheduledTask": true,
        "completionMessage": "Completed",
        "requiresDiskAccess": false,
        "isExclusive": false,
        "isTypeExclusive": false,
        "name": "ApplicationIndexerSync",
        "lastExecutionTime": super::super::date(last_execution),
        "trigger": "scheduled",
        "suppressMessages": false,
    })
}

/// Queues `ApplicationIndexerSync` when its six hours are up (Prowlarr's scheduler, for this task).
fn schedule(st: &AppState) {
    let row = st.db.one("SELECT interval, last_execution FROM prowlarr_tasks WHERE type_name LIKE '%.ApplicationIndexerSyncCommand'", p![]);
    let Ok(Some(r)) = row else { return };
    let last = r.f("last_execution");
    if crate::auth::now() < last + r.i("interval") as f64 * 60.0 {
        return;
    }
    let Some(kind) = super::super::commands::KINDS.iter().find(|k| k.name == "ApplicationIndexerSync") else { return };
    if let Err(e) = super::super::commands::push(st, kind, scheduled_body(last), -1, 2) {
        tracing::warn!("App sync couldn't be queued: {}", e.detail);
    }
}

/// How often the watcher looks, in seconds.
const WATCH_EVERY: u64 = 5;

/// Starts the watcher: it takes over imported apps, handles indexer changes and queues the six-hourly sync.
pub fn spawn(st: AppState) {
    tokio::spawn(async move {
        loop {
            applications::take_imported(&st);
            check_changes(&st).await;
            schedule(&st);
            tokio::time::sleep(std::time::Duration::from_secs(WATCH_EVERY)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexer_ids_come_from_the_address_like_app_indexer_regex() {
        assert_eq!(indexer_id_in("http://lantern:9696/12/"), Some(12));
        assert_eq!(indexer_id_in("http://lantern:9696/12/api"), Some(12));
        assert_eq!(indexer_id_in("http://lantern:9696/12"), Some(12));
        assert_eq!(indexer_id_in("http://lantern:9696/prowlarr/7/API/"), Some(7));
        assert_eq!(indexer_id_in("http://lantern:9696/1234/"), None);
        assert_eq!(indexer_id_in("http://lantern:9696/12/download"), None);
    }
}
