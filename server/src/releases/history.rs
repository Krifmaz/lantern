// Ported from autobrr v1.90.0 (internal/database/release.go, internal/domain/release.go release history types,
// internal/release/cleanup.go), GPL-2.0-or-later.
//! autobrr's release history: every release a filter took and sent to its actions, with each action's status;
//! the duplicate profiles that say when a release is one already taken; the newer-episode check; the dashboard's
//! stats; and the cleanup jobs that delete old releases on a schedule.
//!
//! Rows of `autobrr_release`, `autobrr_release_action_status`, `autobrr_release_profile_duplicate` and
//! `autobrr_release_cleanup_job`. Times are kept as autobrr keeps them in SQLite: RFC 3339 text (UTC). Lists
//! autobrr keeps as Postgres arrays (`rejections`, `tags`, `other`) are JSON arrays here.

use super::release::Release;
use super::store::{ApiError, ApiResult};
use super::Releases;
use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Utc};
use kit_server_core::{p, Db, Row, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A push waiting to run.
pub const PUSH_PENDING: &str = "PENDING";
/// A push the client took.
pub const PUSH_APPROVED: &str = "PUSH_APPROVED";
/// A push the client turned down.
pub const PUSH_REJECTED: &str = "PUSH_REJECTED";
/// A push that failed.
pub const PUSH_ERROR: &str = "PUSH_ERROR";

/// Whether `s` is a push status (autobrr's `ValidReleasePushStatus`).
pub fn valid_push_status(s: &str) -> bool {
    [PUSH_PENDING, PUSH_APPROVED, PUSH_REJECTED, PUSH_ERROR].contains(&s)
}

/// The time now as stored.
pub fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The history tables, with autobrr's three duplicate profiles.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_release (
            id INTEGER PRIMARY KEY,
            filter_status TEXT,
            rejections TEXT NOT NULL DEFAULT '[]',
            indexer TEXT,
            filter TEXT,
            protocol TEXT,
            implementation TEXT,
            timestamp TEXT,
            announce_type TEXT DEFAULT 'NEW',
            info_url TEXT,
            download_url TEXT,
            group_id TEXT,
            torrent_id TEXT,
            torrent_name TEXT,
            normalized_hash TEXT,
            size INTEGER,
            title TEXT,
            sub_title TEXT,
            category TEXT,
            season INTEGER,
            episode INTEGER,
            year INTEGER,
            month INTEGER,
            day INTEGER,
            resolution TEXT,
            source TEXT,
            codec TEXT,
            container TEXT,
            hdr TEXT,
            audio TEXT,
            audio_channels TEXT,
            release_group TEXT,
            region TEXT,
            language TEXT,
            edition TEXT,
            cut TEXT,
            hybrid INTEGER,
            proper INTEGER,
            repack INTEGER,
            website TEXT,
            media_processing TEXT,
            type TEXT,
            origin TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            uploader TEXT,
            pre_time TEXT,
            other TEXT NOT NULL DEFAULT '[]',
            filter_id INTEGER
        );
        CREATE INDEX IF NOT EXISTS autobrr_release_filter_id ON autobrr_release (filter_id);
        CREATE INDEX IF NOT EXISTS autobrr_release_indexer ON autobrr_release (indexer);
        CREATE INDEX IF NOT EXISTS autobrr_release_timestamp ON autobrr_release (timestamp DESC);
        CREATE INDEX IF NOT EXISTS autobrr_release_title ON autobrr_release (title);
        CREATE TABLE IF NOT EXISTS autobrr_release_action_status (
            id INTEGER PRIMARY KEY,
            status TEXT,
            action TEXT NOT NULL,
            action_id INTEGER,
            type TEXT NOT NULL,
            client TEXT,
            filter TEXT,
            filter_id INTEGER,
            rejections TEXT NOT NULL DEFAULT '[]',
            timestamp TEXT,
            release_id INTEGER NOT NULL REFERENCES autobrr_release(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS autobrr_release_action_status_status ON autobrr_release_action_status (status);
        CREATE INDEX IF NOT EXISTS autobrr_release_action_status_release ON autobrr_release_action_status (release_id);
        CREATE TABLE IF NOT EXISTS autobrr_release_profile_duplicate (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            protocol INTEGER DEFAULT 0, release_name INTEGER DEFAULT 0, hash INTEGER DEFAULT 0, title INTEGER DEFAULT 0,
            sub_title INTEGER DEFAULT 0, year INTEGER DEFAULT 0, month INTEGER DEFAULT 0, day INTEGER DEFAULT 0,
            source INTEGER DEFAULT 0, resolution INTEGER DEFAULT 0, codec INTEGER DEFAULT 0, container INTEGER DEFAULT 0,
            dynamic_range INTEGER DEFAULT 0, audio INTEGER DEFAULT 0, release_group INTEGER DEFAULT 0,
            season INTEGER DEFAULT 0, episode INTEGER DEFAULT 0, website INTEGER DEFAULT 0, proper INTEGER DEFAULT 0,
            repack INTEGER DEFAULT 0, edition INTEGER DEFAULT 0, hybrid INTEGER DEFAULT 0, language INTEGER DEFAULT 0
        );
        INSERT OR IGNORE INTO autobrr_release_profile_duplicate (id, name, release_name, hash, title, year, month, day, resolution, season, episode)
        VALUES (1, 'Exact release', 1, 1, 0, 0, 0, 0, 0, 0, 0),
               (2, 'Movie', 0, 0, 1, 1, 0, 0, 1, 0, 0),
               (3, 'TV', 0, 0, 1, 1, 1, 1, 0, 1, 1);
        CREATE TABLE IF NOT EXISTS autobrr_release_cleanup_job (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            enabled INTEGER DEFAULT 0,
            schedule TEXT NOT NULL,
            older_than INTEGER NOT NULL,
            indexers TEXT,
            statuses TEXT,
            last_run TEXT,
            last_run_status TEXT,
            last_run_data TEXT,
            created_at TEXT,
            updated_at TEXT
        );",
    )
    .map_err(|e| anyhow::anyhow!("The release history tables can't be made: {}", e.detail))
}

/// One action's push of a release (autobrr's `ReleaseActionStatus`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionStatus {
    /// Its id (0 until saved).
    pub id: i64,
    /// `PENDING`, `PUSH_APPROVED`, `PUSH_REJECTED` or `PUSH_ERROR`.
    pub status: String,
    /// The action's name.
    pub action: String,
    /// The action's id.
    pub action_id: i64,
    /// The action's kind.
    #[serde(rename = "type")]
    pub kind: String,
    /// The download client's name.
    pub client: String,
    /// The filter's name.
    pub filter: String,
    /// The filter's id.
    pub filter_id: i64,
    /// Why it was turned down, or the error.
    pub rejections: Vec<String>,
    /// The release.
    pub release_id: i64,
    /// When.
    pub timestamp: String,
}

impl ActionStatus {
    /// A pending status for `action` on `r` (autobrr's `NewReleaseActionStatus`).
    pub fn new(action: &super::action::Action, r: &Release) -> ActionStatus {
        ActionStatus {
            id: 0,
            status: PUSH_PENDING.into(),
            action: action.name.clone(),
            action_id: action.id,
            kind: action.kind.clone(),
            client: action.client.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
            filter: r.filter_name.clone(),
            filter_id: r.filter_id,
            rejections: Vec::new(),
            release_id: r.id,
            timestamp: now_text(),
        }
    }
}

/// A duplicate profile: which parts of a release make two the same (autobrr's `DuplicateReleaseProfile`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuplicateProfile {
    /// Its id (0: new).
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The protocol.
    pub protocol: bool,
    /// The release name.
    pub release_name: bool,
    /// The normalized name's hash.
    pub hash: bool,
    /// The title.
    pub title: bool,
    /// The sub title.
    pub sub_title: bool,
    /// The year.
    pub year: bool,
    /// The month.
    pub month: bool,
    /// The day.
    pub day: bool,
    /// The source.
    pub source: bool,
    /// The resolution.
    pub resolution: bool,
    /// The codec.
    pub codec: bool,
    /// The container.
    pub container: bool,
    /// The HDR formats.
    pub dynamic_range: bool,
    /// The audio.
    pub audio: bool,
    /// The group.
    pub group: bool,
    /// The season.
    pub season: bool,
    /// The episode.
    pub episode: bool,
    /// The streaming site.
    pub website: bool,
    /// PROPER.
    pub proper: bool,
    /// REPACK.
    pub repack: bool,
    /// The edition and cut.
    pub edition: bool,
    /// HYBRiD.
    pub hybrid: bool,
    /// The language and region.
    pub language: bool,
}

/// A scheduled delete of old releases (autobrr's `ReleaseCleanupJob`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupJob {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Whether it runs.
    pub enabled: bool,
    /// When, as a five-field cron line.
    pub schedule: String,
    /// Releases older than this many hours go.
    pub older_than: i64,
    /// Only these indexers' releases (comma separated; empty: all).
    pub indexers: String,
    /// Only releases with one of these push statuses (comma separated; empty: all).
    pub statuses: String,
    /// When it last ran.
    pub last_run: String,
    /// `SUCCESS` or `ERROR`.
    pub last_run_status: String,
    /// What the last run did, as JSON, or its error.
    pub last_run_data: String,
    /// When it runs next.
    pub next_run: String,
    /// When it was added.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

/// Go's zero time, as its JSON writes it.
pub const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

impl CleanupJob {
    /// Checks the job (autobrr's `Validate`); the problem in autobrr's words.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("name is required".into());
        }
        if self.schedule.is_empty() {
            return Err("schedule is required".into());
        }
        if self.older_than <= 0 {
            return Err("older_than must be greater than 0".into());
        }
        super::cronspec::CronSpec::parse(&self.schedule).map_err(|e| format!("invalid cron schedule: {e}"))?;
        for s in self.statuses.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if !valid_push_status(s) {
                return Err(format!("invalid status: {s}"));
            }
        }
        Ok(())
    }

    /// The job as autobrr's JSON sends it (unset times as Go's zero time).
    pub fn public(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        for k in ["last_run", "next_run", "created_at", "updated_at"] {
            if v[k].as_str().is_some_and(str::is_empty) {
                v[k] = json!(ZERO_TIME);
            }
        }
        v
    }
}

/// What to delete (autobrr's `DeleteReleaseRequest`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeleteRequest {
    /// Only releases older than this many hours (0: any age).
    pub older_than: i64,
    /// Only these indexers' releases.
    pub indexers: Vec<String>,
    /// Only releases with one of these push statuses; approved ones stay unless `PUSH_APPROVED` is named.
    pub statuses: Vec<String>,
}

/// What a newer-episode check compares (autobrr's `SmartEpisodeParams`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartEpisodeParams {
    /// The show's title.
    pub title: String,
    /// The season.
    pub season: i64,
    /// The episode.
    pub episode: i64,
    /// The year (daily shows).
    pub year: i64,
    /// The month (daily shows).
    pub month: i64,
    /// The day (daily shows).
    pub day: i64,
    /// A REPACK.
    pub repack: bool,
    /// A PROPER.
    pub proper: bool,
    /// The group.
    pub group: String,
}

impl SmartEpisodeParams {
    /// Whether it is a daily show's episode (autobrr's `IsDailyEpisode`).
    pub fn is_daily_episode(&self) -> bool {
        self.year != 0 && self.month != 0 && self.day != 0
    }
}

/// A list kept as JSON text.
fn list_text(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

/// A list kept as JSON text, read back.
fn list_of(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

/// Go's `strings.Split(s, ",")`: one empty part for an empty text.
fn go_split(s: &str) -> Vec<String> {
    s.split(',').map(str::to_string).collect()
}

/// A comma list, empty for an empty text (autobrr's `Get`).
fn split_or_none(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        go_split(s)
    }
}

/// An action status from its row.
fn status_of(r: &Row) -> ActionStatus {
    ActionStatus {
        id: r.i("id"),
        status: r.s("status"),
        action: r.s("action"),
        action_id: r.i("action_id"),
        kind: r.s("type"),
        client: r.s("client"),
        filter: r.s("filter"),
        filter_id: r.i("filter_id"),
        rejections: list_of(&r.s("rejections")),
        release_id: r.i("release_id"),
        timestamp: r.s("timestamp"),
    }
}

/// A cleanup job from its row.
fn job_of(r: &Row) -> CleanupJob {
    CleanupJob {
        id: r.i("id"),
        name: r.s("name"),
        enabled: r.b("enabled"),
        schedule: r.s("schedule"),
        older_than: r.i("older_than"),
        indexers: r.s("indexers"),
        statuses: r.s("statuses"),
        last_run: r.s("last_run"),
        last_run_status: r.s("last_run_status"),
        last_run_data: r.s("last_run_data"),
        next_run: String::new(),
        created_at: r.s("created_at"),
        updated_at: r.s("updated_at"),
    }
}

/// A profile from its row (autobrr reads neither `edition` nor `language` back).
fn profile_of(r: &Row) -> DuplicateProfile {
    DuplicateProfile {
        id: r.i("id"),
        name: r.s("name"),
        protocol: r.b("protocol"),
        release_name: r.b("release_name"),
        hash: r.b("hash"),
        title: r.b("title"),
        sub_title: r.b("sub_title"),
        year: r.b("year"),
        month: r.b("month"),
        day: r.b("day"),
        source: r.b("source"),
        resolution: r.b("resolution"),
        codec: r.b("codec"),
        container: r.b("container"),
        dynamic_range: r.b("dynamic_range"),
        audio: r.b("audio"),
        group: r.b("release_group"),
        season: r.b("season"),
        episode: r.b("episode"),
        website: r.b("website"),
        proper: r.b("proper"),
        repack: r.b("repack"),
        hybrid: r.b("hybrid"),
        edition: false,
        language: false,
    }
}

/// The names `search` can be narrowed by (autobrr's `reservedSearch`), with their columns.
const RESERVED_SEARCH: [(&str, &str); 12] = [
    ("title", "r.title"),
    ("group", "r.release_group"),
    ("category", "r.category"),
    ("season", "r.season"),
    ("episode", "r.episode"),
    ("year", "r.year"),
    ("resolution", "r.resolution"),
    ("source", "r.source"),
    ("codec", "r.codec"),
    ("hdr", "r.hdr"),
    ("type", "r.type"),
    ("filter", "r.filter"),
];

/// The `where` parts and values for a search (autobrr's `findReleases` search): each `name:value` (quoted
/// values may hold spaces) narrows its column by prefix, the rest of the text the release name.
pub fn search_where(search: &str) -> (Vec<String>, Vec<Value>) {
    let mut parts = Vec::new();
    let mut args = Vec::new();
    let mut rest = search.trim().to_string();
    for (name, col) in RESERVED_SEARCH {
        let re = regex::Regex::new(&format!(r#"(?i)(?:{name}:)('.*?'|".*?"|\S+)"#)).expect("search pattern");
        let found: Vec<String> = re.captures_iter(&rest).map(|c| c[1].to_string()).collect();
        if found.is_empty() {
            continue;
        }
        let ors: Vec<String> = found.iter().map(|_| format!("{col} LIKE ?")).collect();
        for f in &found {
            args.push(json!(format!("{}%", f.trim_matches('"').trim_matches('\'').replace('.', "_"))));
        }
        parts.push(format!("({})", ors.join(" OR ")));
        rest = re.replace_all(&rest, "").trim().to_string();
    }
    if !rest.is_empty() {
        if parts.len() > 1 {
            args.push(json!(format!("%{rest}%")));
        } else {
            args.push(json!(format!("{rest}%")));
        }
        parts.push("r.torrent_name LIKE ?".into());
    }
    (parts, args)
}

/// How a page of releases is asked for (autobrr's `ReleaseQueryParams`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// At most this many (0: 20).
    pub limit: i64,
    /// Skip this many.
    pub offset: i64,
    /// Only releases with a smaller id (0: any).
    pub cursor: i64,
    /// Only these indexers' releases.
    pub indexers: Vec<String>,
    /// Only releases with an action of this push status.
    pub push_status: String,
    /// The search text.
    pub search: String,
}

impl Releases {
    // ---- releases (autobrr's ReleaseRepo) ----

    /// Saves release `r` (autobrr's `Store`); its id is set.
    pub fn store_history_release(&self, r: &mut Release) -> ApiResult<()> {
        let id = self.db.execute(
            "INSERT INTO autobrr_release (filter_status, rejections, indexer, filter, protocol, implementation, timestamp, announce_type, group_id, torrent_id, info_url, download_url, torrent_name, normalized_hash, size, title, sub_title, category, season, episode, year, month, day, resolution, source, codec, container, hdr, audio, audio_channels, release_group, proper, repack, region, language, cut, edition, hybrid, media_processing, website, type, origin, tags, uploader, pre_time, other, filter_id) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            vec![
                json!(r.filter_status),
                json!(list_text(&r.rejections)),
                json!(r.indexer.identifier),
                json!(r.filter_name),
                json!(r.protocol),
                json!(r.implementation),
                json!(now_text()),
                json!(r.announce_type),
                json!(r.group_id),
                json!(r.torrent_id),
                json!(r.info_url),
                json!(r.download_url),
                json!(r.torrent_name),
                json!(r.normalized_hash),
                json!(r.size),
                json!(r.title),
                json!(r.sub_title),
                json!(r.category),
                json!(r.season),
                json!(r.episode),
                json!(r.year),
                json!(r.month),
                json!(r.day),
                json!(r.resolution),
                json!(r.source),
                json!(r.codec.join(",")),
                json!(r.container),
                json!(r.hdr.join(",")),
                json!(r.audio.join(",")),
                json!(r.audio_channels),
                json!(r.group),
                json!(r.proper),
                json!(r.repack),
                json!(r.region),
                json!(r.language.join(",")),
                json!(r.cut.join(",")),
                json!(r.edition.join(",")),
                json!(r.hybrid),
                json!(r.media_processing),
                json!(r.website),
                json!(r.r#type.as_str()),
                json!(r.origin),
                json!(list_text(&r.tags)),
                json!(r.uploader),
                json!(r.pre_time),
                json!(list_text(&r.other)),
                if r.filter_id > 0 { json!(r.filter_id) } else { Value::Null },
            ],
        )?;
        r.id = id;
        Ok(())
    }

    /// Saves the release's size (autobrr's `Update`).
    pub fn update_history_release(&self, r: &Release) -> ApiResult<()> {
        self.db.execute("UPDATE autobrr_release SET size=? WHERE id=?", p![r.size, r.id])?;
        Ok(())
    }

    /// Saves an action status, adding it when it has no id yet (autobrr's `StoreReleaseActionStatus`).
    pub fn store_action_status(&self, s: &mut ActionStatus) -> ApiResult<()> {
        if s.id != 0 {
            self.db.execute(
                "UPDATE autobrr_release_action_status SET status=?, rejections=?, timestamp=? WHERE id=? AND release_id=?",
                p![s.status, list_text(&s.rejections), s.timestamp, s.id, s.release_id],
            )?;
        } else {
            s.id = self.db.execute(
                "INSERT INTO autobrr_release_action_status (status, action, action_id, type, client, filter, filter_id, rejections, timestamp, release_id) VALUES (?,?,?,?,?,?,?,?,?,?)",
                p![
                    s.status,
                    s.action,
                    if s.action_id > 0 { json!(s.action_id) } else { Value::Null },
                    s.kind,
                    s.client,
                    s.filter,
                    if s.filter_id > 0 { json!(s.filter_id) } else { Value::Null },
                    list_text(&s.rejections),
                    s.timestamp,
                    s.release_id
                ],
            )?;
        }
        Ok(())
    }

    /// The indexer names by identifier: Lantern's indexers, then the removed definitions.
    fn indexer_names(&self) -> BTreeMap<String, (i64, String, String)> {
        let mut m = BTreeMap::new();
        for d in self.library.deprecations() {
            m.insert(d.identifier.clone(), (0, d.name.clone(), String::new()));
        }
        for i in self.autobrr_indexers().unwrap_or_default() {
            m.insert(i.identifier.clone(), (i.id, i.name.clone(), i.identifier_external.clone()));
        }
        m
    }

    /// A page of releases with their action statuses (autobrr's `Find`).
    pub fn find_history(&self, q: &Query) -> ApiResult<Value> {
        let mut parts: Vec<String> = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        if q.cursor > 0 {
            parts.push("r.id < ?".into());
            args.push(json!(q.cursor));
        }
        if !q.search.is_empty() {
            let (p2, a2) = search_where(&q.search);
            parts.extend(p2);
            args.extend(a2);
        }
        if !q.indexers.is_empty() {
            let ors: Vec<&str> = q.indexers.iter().map(|_| "r.indexer = ?").collect();
            parts.push(format!("({})", ors.join(" OR ")));
            args.extend(q.indexers.iter().map(|i| json!(i)));
        }
        let mut join = String::new();
        if !q.push_status.is_empty() {
            join = " INNER JOIN autobrr_release_action_status ras ON r.id = ras.release_id".into();
            parts.push("ras.status = ?".into());
            args.push(json!(q.push_status));
        }
        let where_sql = if parts.is_empty() { "1=1".to_string() } else { parts.join(" AND ") };
        let total = self
            .db
            .one(&format!("SELECT COUNT(*) AS n FROM autobrr_release r{join} WHERE {where_sql}"), args.clone())?
            .map(|r| r.i("n"))
            .unwrap_or(0);
        let limit = if q.limit > 0 { q.limit } else { 20 };
        let mut page_args = args.clone();
        page_args.push(json!(limit));
        page_args.push(json!(q.offset.max(0)));
        let ids: Vec<i64> = self
            .db
            .query(&format!("SELECT DISTINCT r.id FROM autobrr_release r{join} WHERE {where_sql} ORDER BY r.id DESC LIMIT ? OFFSET ?"), page_args)?
            .iter()
            .map(|r| r.i("id"))
            .collect();
        let names = self.indexer_names();
        let mut data = Vec::new();
        for id in &ids {
            let Some(row) = self.db.one("SELECT * FROM autobrr_release WHERE id=?", p![id])? else { continue };
            let statuses: Vec<ActionStatus> =
                self.db.query("SELECT * FROM autobrr_release_action_status WHERE release_id=? ORDER BY id", p![id])?.iter().map(status_of).collect();
            data.push(release_json(&row, &names, statuses, true));
        }
        let next = ids.last().copied().unwrap_or(0);
        Ok(json!({ "data": data, "count": total, "next_cursor": next }))
    }

    /// Release `id` without its action statuses, as autobrr's `Get` answers it.
    pub fn history_release(&self, id: i64) -> ApiResult<Value> {
        let row = self.db.one("SELECT * FROM autobrr_release WHERE id=?", p![id])?.ok_or_else(|| ApiError::NotFound(format!("could not find release with id {id}")))?;
        Ok(release_json(&row, &BTreeMap::new(), Vec::new(), false))
    }

    /// Release `id` as a release to run an action on again (autobrr's `Get`, read into its `Release`).
    pub fn history_release_for_retry(&self, id: i64) -> ApiResult<Release> {
        let r = self.db.one("SELECT * FROM autobrr_release WHERE id=?", p![id])?.ok_or_else(|| ApiError::NotFound("sql: no rows in result set".into()))?;
        let mut rel = Release::new(super::release::IndexerMinimal { identifier: r.s("indexer"), ..Default::default() });
        rel.id = r.i("id");
        rel.filter_status = r.s("filter_status");
        rel.rejections = list_of(&r.s("rejections"));
        rel.filter_name = r.s("filter");
        rel.filter_id = r.i("filter_id");
        rel.protocol = r.s("protocol");
        rel.implementation = r.s("implementation");
        rel.announce_type = r.s("announce_type");
        rel.info_url = r.s("info_url");
        rel.download_url = r.s("download_url");
        rel.title = r.s("title");
        rel.sub_title = r.s("sub_title");
        rel.torrent_name = r.s("torrent_name");
        rel.normalized_hash = r.s("normalized_hash");
        rel.category = r.s("category");
        rel.size = r.i("size").max(0) as u64;
        rel.group_id = r.s("group_id");
        rel.torrent_id = r.s("torrent_id");
        rel.season = r.i("season");
        rel.episode = r.i("episode");
        rel.year = r.i("year");
        rel.month = r.i("month");
        rel.day = r.i("day");
        rel.resolution = r.s("resolution");
        rel.source = r.s("source");
        rel.codec = split_or_none(&r.s("codec"));
        rel.container = r.s("container");
        rel.hdr = split_or_none(&r.s("hdr"));
        rel.audio = split_or_none(&r.s("audio"));
        rel.audio_channels = r.s("audio_channels");
        rel.group = r.s("release_group");
        rel.proper = r.b("proper");
        rel.repack = r.b("repack");
        rel.region = r.s("region");
        rel.language = split_or_none(&r.s("language"));
        rel.cut = split_or_none(&r.s("cut"));
        rel.edition = split_or_none(&r.s("edition"));
        rel.hybrid = r.b("hybrid");
        rel.media_processing = r.s("media_processing");
        rel.website = r.s("website");
        rel.r#type = super::rls::Type::parse(&r.s("type"));
        rel.origin = r.s("origin");
        rel.tags = list_of(&r.s("tags"));
        rel.uploader = r.s("uploader");
        rel.pre_time = r.s("pre_time");
        rel.other = list_of(&r.s("other"));
        if rel.download_url.starts_with(super::run::MAGNET_URI_PREFIX) {
            rel.magnet_uri = rel.download_url.clone();
        }
        Ok(rel)
    }

    /// Action status `id` (autobrr's `GetActionStatus`).
    pub fn action_status(&self, id: i64) -> ApiResult<ActionStatus> {
        self.db
            .one("SELECT * FROM autobrr_release_action_status WHERE id=?", p![id])?
            .map(|r| status_of(&r))
            .ok_or_else(|| ApiError::NotFound("sql: no rows in result set".into()))
    }

    /// Every indexer releases came from, and every indexer there is (autobrr's `GetIndexerOptions`).
    pub fn history_indexer_options(&self) -> ApiResult<Vec<String>> {
        let mut set: std::collections::BTreeSet<String> =
            self.db.query("SELECT DISTINCT indexer FROM autobrr_release", p![])?.iter().map(|r| r.s("indexer")).collect();
        for i in self.autobrr_indexers()? {
            set.insert(i.identifier);
        }
        Ok(set.into_iter().collect())
    }

    /// The totals (autobrr's `Stats`).
    pub fn history_stats(&self) -> ApiResult<Value> {
        let (mut total, mut filtered, mut rejected) = (0, 0, 0);
        for r in self.db.query("SELECT filter_status, COUNT(*) AS n FROM autobrr_release GROUP BY filter_status", p![])? {
            let n = r.i("n");
            total += n;
            match r.s("filter_status").as_str() {
                "FILTER_APPROVED" => filtered = n,
                "FILTER_REJECTED" => rejected = n,
                _ => {}
            }
        }
        let (mut approved, mut push_rejected, mut push_error) = (0, 0, 0);
        for r in self.db.query("SELECT status, COUNT(*) AS n FROM autobrr_release_action_status GROUP BY status", p![])? {
            match r.s("status").as_str() {
                PUSH_APPROVED => approved = r.i("n"),
                PUSH_REJECTED => push_rejected = r.i("n"),
                PUSH_ERROR => push_error = r.i("n"),
                _ => {}
            }
        }
        Ok(json!({
            "total_count": total, "filtered_count": filtered, "filter_rejected_count": rejected,
            "push_approved_count": approved, "push_rejected_count": push_rejected, "push_error_count": push_error,
        }))
    }

    /// Releases and pushes per day over the last `days` (0: all time) (autobrr's `StatsActivity`).
    pub fn stats_activity(&self, days: i64) -> ApiResult<Value> {
        let w = Window::new(days);
        let mut daily: BTreeMap<String, [i64; 4]> = BTreeMap::new();
        for r in self.db.query(&format!("SELECT substr(timestamp, 1, 10) AS day, COUNT(*) AS n FROM autobrr_release WHERE {} GROUP BY 1", w.sql("timestamp")), w.args())? {
            if !r.s("day").is_empty() {
                daily.entry(r.s("day")).or_default()[0] = r.i("n");
            }
        }
        for r in self.db.query(
            &format!("SELECT substr(timestamp, 1, 10) AS day, status, COUNT(*) AS n FROM autobrr_release_action_status WHERE {} GROUP BY 1, 2", w.sql("timestamp")),
            w.args(),
        )? {
            if r.s("day").is_empty() {
                continue;
            }
            let i = match r.s("status").as_str() {
                PUSH_APPROVED => 1,
                PUSH_REJECTED => 2,
                PUSH_ERROR => 3,
                _ => continue,
            };
            daily.entry(r.s("day")).or_default()[i] = r.i("n");
        }
        let out: Vec<Value> = w
            .dates(daily.keys().next().map(String::as_str))
            .into_iter()
            .map(|d| {
                let c = daily.get(&d).copied().unwrap_or_default();
                json!({ "date": d, "matched_count": c[0], "push_approved_count": c[1], "push_rejected_count": c[2], "push_error_count": c[3] })
            })
            .collect();
        Ok(json!({ "days": days, "daily": out }))
    }

    /// Bytes of approved releases per day (autobrr's `StatsVolume`).
    pub fn stats_volume(&self, days: i64) -> ApiResult<Value> {
        let w = Window::new(days);
        let mut daily: BTreeMap<String, i64> = BTreeMap::new();
        let mut args = vec![json!(PUSH_APPROVED)];
        args.extend(w.args());
        for r in self.db.query(
            &format!(
                "SELECT substr(timestamp, 1, 10) AS day, SUM(size) AS n FROM autobrr_release WHERE id IN (SELECT DISTINCT release_id FROM autobrr_release_action_status WHERE status = ?) AND {} GROUP BY 1",
                w.sql("timestamp")
            ),
            args,
        )? {
            if !r.s("day").is_empty() {
                daily.insert(r.s("day"), r.i("n"));
            }
        }
        let out: Vec<Value> = w
            .dates(daily.keys().next().map(String::as_str))
            .into_iter()
            .map(|d| json!({ "date": d, "downloaded_bytes": daily.get(&d).copied().unwrap_or(0) }))
            .collect();
        Ok(json!({ "days": days, "daily": out }))
    }

    /// Releases per weekday and hour, 168 cells from Sunday 00 (autobrr's `StatsHeatmap`).
    pub fn stats_heatmap(&self, days: i64) -> ApiResult<Value> {
        let w = Window::new(days);
        let mut cells = vec![0i64; 7 * 24];
        for r in self.db.query(
            &format!(
                "SELECT substr(timestamp, 1, 10) AS day, CAST(substr(timestamp, 12, 2) AS INTEGER) AS hour, COUNT(*) AS n FROM autobrr_release WHERE {} GROUP BY 1, 2",
                w.sql("timestamp")
            ),
            w.args(),
        )? {
            let hour = r.i("hour");
            let Ok(date) = NaiveDate::parse_from_str(&r.s("day"), "%Y-%m-%d") else { continue };
            if !(0..=23).contains(&hour) {
                continue;
            }
            cells[date.weekday().num_days_from_sunday() as usize * 24 + hour as usize] += r.i("n");
        }
        Ok(json!({ "days": days, "heatmap": cells }))
    }

    /// The ten indexers with the most approved pushes (autobrr's `StatsTopIndexers`).
    pub fn stats_top_indexers(&self, days: i64) -> ApiResult<Value> {
        let w = Window::new(days);
        let mut m: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for r in self.db.query(
            &format!("SELECT indexer, COUNT(*) AS n FROM autobrr_release WHERE indexer IS NOT NULL AND indexer != '' AND {} GROUP BY 1", w.sql("timestamp")),
            w.args(),
        )? {
            m.entry(r.s("indexer")).or_default().0 = r.i("n");
        }
        let mut args = vec![json!(PUSH_APPROVED)];
        args.extend(w.args());
        for r in self.db.query(
            &format!(
                "SELECT r.indexer AS indexer, COUNT(*) AS n FROM autobrr_release_action_status ras INNER JOIN autobrr_release r ON r.id = ras.release_id WHERE ras.status = ? AND r.indexer IS NOT NULL AND r.indexer != '' AND {} GROUP BY 1",
                w.sql("ras.timestamp")
            ),
            args,
        )? {
            m.entry(r.s("indexer")).or_default().1 = r.i("n");
        }
        let mut top: Vec<(String, i64, i64)> = m.into_iter().map(|(k, (a, b))| (k, a, b)).collect();
        top.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
        top.truncate(10);
        let list: Vec<Value> = top.into_iter().map(|(k, a, b)| json!({ "indexer": k, "matched_count": a, "push_approved_count": b })).collect();
        Ok(json!({ "days": days, "top": list }))
    }

    /// The ten filters with the most matches (autobrr's `StatsTopFilters`).
    pub fn stats_top_filters(&self, days: i64) -> ApiResult<Value> {
        let w = Window::new(days);
        let mut m: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for r in self.db.query(
            &format!("SELECT filter, COUNT(*) AS n FROM autobrr_release WHERE filter IS NOT NULL AND filter != '' AND {} GROUP BY 1", w.sql("timestamp")),
            w.args(),
        )? {
            m.entry(r.s("filter")).or_default().0 = r.i("n");
        }
        let mut args = vec![json!(PUSH_APPROVED)];
        args.extend(w.args());
        for r in self.db.query(
            &format!(
                "SELECT filter, COUNT(*) AS n FROM autobrr_release_action_status WHERE status = ? AND filter IS NOT NULL AND filter != '' AND {} GROUP BY 1",
                w.sql("timestamp")
            ),
            args,
        )? {
            m.entry(r.s("filter")).or_default().1 = r.i("n");
        }
        let mut top: Vec<(String, i64, i64)> = m.into_iter().map(|(k, (a, b))| (k, a, b)).collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        top.truncate(10);
        let list: Vec<Value> = top.into_iter().map(|(k, a, b)| json!({ "filter": k, "matched_count": a, "push_approved_count": b })).collect();
        Ok(json!({ "days": days, "top": list }))
    }

    /// Deletes releases (autobrr's `Delete`): older than, on the indexers, with the statuses; a release pushed to
    /// a client stays unless `PUSH_APPROVED` is named. Their action statuses go with them.
    pub fn delete_history(&self, req: &DeleteRequest) -> ApiResult<()> {
        let mut parts = Vec::new();
        let mut args = Vec::new();
        if req.older_than > 0 {
            parts.push(format!("datetime(timestamp) < datetime('now','-{} hours')", req.older_than));
        }
        if !req.indexers.is_empty() {
            parts.push(format!("indexer IN ({})", vec!["?"; req.indexers.len()].join(",")));
            args.extend(req.indexers.iter().map(|i| json!(i)));
        }
        if !req.statuses.is_empty() {
            parts.push(format!(
                "id IN (SELECT release_id FROM autobrr_release_action_status WHERE status IN ({}))",
                vec!["?"; req.statuses.len()].join(",")
            ));
            args.extend(req.statuses.iter().map(|s| json!(s)));
            if !req.statuses.iter().any(|s| s == PUSH_APPROVED) {
                parts.push("id NOT IN (SELECT release_id FROM autobrr_release_action_status WHERE status = ?)".into());
                args.push(json!(PUSH_APPROVED));
            }
        }
        let where_sql = if parts.is_empty() { String::new() } else { format!(" WHERE {}", parts.join(" AND ")) };
        self.db.execute(&format!("DELETE FROM autobrr_release{where_sql}"), args)?;
        self.db.execute("DELETE FROM autobrr_release_action_status WHERE release_id NOT IN (SELECT id FROM autobrr_release)", p![])?;
        Ok(())
    }

    /// Whether a newer episode of the show was already pushed is false (autobrr's
    /// `CheckSmartEpisodeCanDownload`): whether this one may be taken.
    pub fn smart_episode_can_download(&self, p: &SmartEpisodeParams) -> ApiResult<bool> {
        let mut parts = vec!["r.title LIKE ?".to_string(), "ras.status = 'PUSH_APPROVED'".to_string()];
        let mut args = vec![json!(format!("{}%", p.title))];
        if p.proper {
            parts.push("r.proper = ?".into());
            args.push(json!(p.proper));
        }
        if p.repack {
            parts.push("(r.repack = ? AND r.release_group LIKE ?)".into());
            args.push(json!(p.repack));
            args.push(json!(p.group));
        }
        if p.season > 0 && p.episode > 0 {
            parts.push("((r.season = ? AND r.episode > ?) OR r.season > ?)".into());
            args.extend([json!(p.season), json!(p.episode), json!(p.season)]);
        } else if p.season > 0 && p.episode == 0 {
            parts.push("r.season > ?".into());
            args.push(json!(p.season));
        } else if p.year > 0 && p.month > 0 && p.day > 0 {
            parts.push("((r.year = ? AND r.month = ? AND r.day > ?) OR (r.year = ? AND r.month > ?) OR r.year > ?)".into());
            args.extend([json!(p.year), json!(p.month), json!(p.day), json!(p.year), json!(p.month), json!(p.year)]);
        } else {
            return Ok(true);
        }
        let n = self
            .db
            .one(
                &format!("SELECT COUNT(*) AS n FROM autobrr_release r LEFT JOIN autobrr_release_action_status ras ON r.id = ras.release_id WHERE {}", parts.join(" AND ")),
                args,
            )?
            .map(|r| r.i("n"))
            .unwrap_or(0);
        Ok(n == 0)
    }

    /// Whether `r` is a release already pushed by the parts `profile` names (autobrr's `CheckIsDuplicateRelease`).
    pub fn is_duplicate_release(&self, profile: &DuplicateProfile, r: &Release) -> ApiResult<bool> {
        let mut parts = vec!["ras.status = 'PUSH_APPROVED'".to_string()];
        let mut args: Vec<Value> = Vec::new();
        let mut eq = |parts: &mut Vec<String>, col: &str, v: Value| {
            parts.push(format!("{col} = ?"));
            args.push(v);
        };
        let likes = |parts: &mut Vec<String>, args: &mut Vec<Value>, col: &str, vals: &[String]| {
            for v in vals {
                parts.push(format!("{col} LIKE ?"));
                args.push(json!(format!("%{v}%")));
            }
        };
        let mut a: Vec<Value> = Vec::new();
        if profile.release_name && profile.hash {
            eq(&mut parts, "r.normalized_hash", json!(r.normalized_hash));
        } else {
            let video = r.is_type_video();
            let mut like = |parts: &mut Vec<String>, col: &str, v: &str| {
                parts.push(format!("{col} LIKE ?"));
                a.push(json!(v));
            };
            // the parts are added in autobrr's order; the values with them
            let mut ordered: Vec<(String, Vec<Value>)> = Vec::new();
            let mut push = |sql: String, vals: Vec<Value>| ordered.push((sql, vals));
            if profile.title {
                push("r.title LIKE ?".into(), vec![json!(r.title)]);
            }
            if profile.sub_title {
                push("r.sub_title LIKE ?".into(), vec![json!(r.sub_title)]);
            }
            if profile.year {
                push("r.year = ?".into(), vec![json!(r.year)]);
            }
            if profile.month {
                push("r.month = ?".into(), vec![json!(r.month)]);
            }
            if profile.day {
                push("r.day = ?".into(), vec![json!(r.day)]);
            }
            if profile.source {
                push("r.source = ?".into(), vec![json!(r.source)]);
            }
            if profile.container {
                push("r.container = ?".into(), vec![json!(r.container)]);
            }
            if profile.edition {
                for c in &r.cut {
                    push("r.cut LIKE ?".into(), vec![json!(format!("%{c}%"))]);
                }
                for e in &r.edition {
                    push("r.edition LIKE ?".into(), vec![json!(format!("%{e}%"))]);
                }
            }
            if profile.hybrid && video {
                push("r.hybrid = ?".into(), vec![json!(r.hybrid)]);
            }
            if video {
                push("r.media_processing = ?".into(), vec![json!(r.media_processing)]);
            }
            if profile.language {
                push("r.region = ?".into(), vec![json!(r.region)]);
                if r.language.is_empty() {
                    push("r.language = ?".into(), vec![json!("")]);
                } else {
                    for l in &r.language {
                        push("r.language LIKE ?".into(), vec![json!(format!("%{l}%"))]);
                    }
                }
            }
            if profile.codec {
                if r.codec.len() > 1 {
                    for c in &r.codec {
                        push("r.codec LIKE ?".into(), vec![json!(format!("%{c}%"))]);
                    }
                } else if r.codec.is_empty() {
                    // squirrel's `IN ()` for an empty list is false
                    push("(1=0)".into(), vec![]);
                } else {
                    push("r.codec IN (?)".into(), vec![json!(r.codec[0])]);
                }
            }
            if profile.resolution {
                push("r.resolution = ?".into(), vec![json!(r.resolution)]);
            }
            if profile.dynamic_range {
                push("r.hdr = ?".into(), vec![json!(r.hdr.join(","))]);
            }
            if profile.audio {
                push("r.audio = ?".into(), vec![json!(r.audio.join(","))]);
                push("r.audio_channels = ?".into(), vec![json!(r.audio_channels)]);
            }
            if profile.group {
                push("r.release_group LIKE ?".into(), vec![json!(r.group)]);
            }
            if profile.season {
                push("r.season = ?".into(), vec![json!(r.season)]);
            }
            if profile.episode {
                push("r.episode = ?".into(), vec![json!(r.episode)]);
            }
            if profile.website {
                push("r.website = ?".into(), vec![json!(r.website)]);
            }
            if profile.proper {
                push("r.proper = ?".into(), vec![json!(r.proper)]);
            }
            if profile.repack {
                push("(r.repack = ? AND r.release_group LIKE ?)".into(), vec![json!(r.repack), json!(r.group)]);
            }
            let _ = &mut like;
            let _ = likes;
            for (sql, vals) in ordered {
                parts.push(sql);
                a.extend(vals);
            }
        }
        args.extend(a);
        let found = self.db.query(
            &format!("SELECT r.id FROM autobrr_release r LEFT JOIN autobrr_release_action_status ras ON r.id = ras.release_id WHERE {}", parts.join(" AND ")),
            args,
        )?;
        Ok(!found.is_empty())
    }

    // ---- duplicate profiles ----

    /// Every duplicate profile (autobrr's `FindDuplicateReleaseProfiles`).
    pub fn duplicate_profiles(&self) -> ApiResult<Vec<DuplicateProfile>> {
        Ok(self.db.query("SELECT * FROM autobrr_release_profile_duplicate", p![])?.iter().map(profile_of).collect())
    }

    /// Duplicate profile `id`, if there is one.
    pub fn duplicate_profile(&self, id: i64) -> ApiResult<Option<DuplicateProfile>> {
        Ok(self.db.one("SELECT * FROM autobrr_release_profile_duplicate WHERE id=?", p![id])?.map(|r| profile_of(&r)))
    }

    /// Adds or changes a duplicate profile (autobrr's `StoreDuplicateProfile`; `edition` and `language` aren't
    /// saved, as in autobrr).
    pub fn store_duplicate_profile(&self, p: &mut DuplicateProfile) -> ApiResult<()> {
        let vals = vec![
            json!(p.name),
            json!(p.protocol),
            json!(p.release_name),
            json!(p.hash),
            json!(p.title),
            json!(p.sub_title),
            json!(p.season),
            json!(p.episode),
            json!(p.year),
            json!(p.month),
            json!(p.day),
            json!(p.resolution),
            json!(p.source),
            json!(p.codec),
            json!(p.container),
            json!(p.dynamic_range),
            json!(p.audio),
            json!(p.group),
            json!(p.website),
            json!(p.proper),
            json!(p.repack),
            json!(p.hybrid),
        ];
        const COLS: &str = "name, protocol, release_name, hash, title, sub_title, season, episode, year, month, day, resolution, source, codec, container, dynamic_range, audio, release_group, website, proper, repack, hybrid";
        if p.id == 0 {
            p.id = self.db.execute(&format!("INSERT INTO autobrr_release_profile_duplicate ({COLS}) VALUES ({})", vec!["?"; 22].join(",")), vals)?;
        } else {
            let sets: Vec<String> = COLS.split(", ").map(|c| format!("{c}=?")).collect();
            let mut vals = vals;
            vals.push(json!(p.id));
            self.db.execute(&format!("UPDATE autobrr_release_profile_duplicate SET {} WHERE id=?", sets.join(", ")), vals)?;
        }
        Ok(())
    }

    /// Removes duplicate profile `id` (autobrr's `DeleteReleaseProfileDuplicate`).
    pub fn delete_duplicate_profile(&self, id: i64) -> ApiResult<()> {
        self.db.execute("DELETE FROM autobrr_release_profile_duplicate WHERE id=?", p![id])?;
        Ok(())
    }

    // ---- cleanup jobs ----

    /// Every cleanup job by name, with when the enabled ones run next (autobrr's `ListCleanupJobs`).
    pub fn cleanup_jobs(&self) -> ApiResult<Vec<CleanupJob>> {
        let mut list: Vec<CleanupJob> = self.db.query("SELECT * FROM autobrr_release_cleanup_job ORDER BY name ASC", p![])?.iter().map(job_of).collect();
        for j in &mut list {
            if j.enabled {
                if let Some(t) = self.scheduler.next_run(&cleanup_key(j.id)) {
                    j.next_run = t.to_rfc3339_opts(SecondsFormat::Secs, true);
                }
            }
        }
        Ok(list)
    }

    /// Cleanup job `id`; autobrr's `sql: no rows in result set` when there is none.
    pub fn cleanup_job(&self, id: i64) -> ApiResult<CleanupJob> {
        self.db
            .one("SELECT * FROM autobrr_release_cleanup_job WHERE id=?", p![id])?
            .map(|r| job_of(&r))
            .ok_or_else(|| ApiError::NotFound("sql: no rows in result set".into()))
    }

    /// Adds a cleanup job, scheduled when enabled (autobrr's `StoreCleanupJob`).
    pub fn store_cleanup_job(&self, j: &mut CleanupJob) -> ApiResult<()> {
        j.validate().map_err(ApiError::Internal)?;
        let now = now_text();
        j.id = self.db.execute(
            "INSERT INTO autobrr_release_cleanup_job (name, enabled, schedule, older_than, indexers, statuses, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?)",
            p![
                j.name,
                j.enabled,
                j.schedule,
                j.older_than,
                if j.indexers.is_empty() { Value::Null } else { json!(j.indexers) },
                if j.statuses.is_empty() { Value::Null } else { json!(j.statuses) },
                now,
                now
            ],
        )?;
        if j.enabled {
            self.start_cleanup_job(j)?;
        }
        Ok(())
    }

    /// Changes a cleanup job, rescheduling it when it is or was enabled (autobrr's `UpdateCleanupJob`).
    pub fn update_cleanup_job(&self, j: &CleanupJob) -> ApiResult<()> {
        j.validate().map_err(ApiError::Internal)?;
        let cur = self.cleanup_job(j.id).map_err(|_| ApiError::NotFound(format!("could not find cleanup job with id {}", j.id)))?;
        self.db.execute(
            "UPDATE autobrr_release_cleanup_job SET name=?, enabled=?, schedule=?, older_than=?, indexers=?, statuses=?, updated_at=? WHERE id=?",
            p![
                j.name,
                j.enabled,
                j.schedule,
                j.older_than,
                if j.indexers.is_empty() { Value::Null } else { json!(j.indexers) },
                if j.statuses.is_empty() { Value::Null } else { json!(j.statuses) },
                now_text(),
                j.id
            ],
        )?;
        if cur.enabled || j.enabled {
            self.scheduler.remove(&cleanup_key(j.id));
            if j.enabled {
                self.start_cleanup_job(j)?;
            }
        }
        Ok(())
    }

    /// Removes a cleanup job and stops it (autobrr's `DeleteCleanupJob`).
    pub fn delete_cleanup_job(&self, id: i64) -> ApiResult<()> {
        let cur = self.cleanup_job(id).map_err(|_| ApiError::NotFound(format!("could not find cleanup job with id {id}")))?;
        if cur.enabled {
            self.scheduler.remove(&cleanup_key(id));
        }
        self.db.execute("DELETE FROM autobrr_release_cleanup_job WHERE id=?", p![id])?;
        Ok(())
    }

    /// Turns a cleanup job on or off (autobrr's `ToggleCleanupJobEnabled`).
    pub fn toggle_cleanup_job(&self, id: i64, enabled: bool) -> ApiResult<()> {
        let mut j = self.cleanup_job(id).map_err(|_| ApiError::NotFound(format!("could not find cleanup job with id {id}")))?;
        if j.enabled == enabled {
            return Ok(());
        }
        self.db.execute("UPDATE autobrr_release_cleanup_job SET enabled=?, updated_at=? WHERE id=?", p![enabled, now_text(), id])?;
        if enabled {
            j.enabled = true;
            self.start_cleanup_job(&j)?;
        } else {
            self.scheduler.remove(&cleanup_key(id));
        }
        Ok(())
    }

    /// Runs a cleanup job now (autobrr's `ForceRunCleanupJob`).
    pub fn force_run_cleanup_job(&self, id: i64) -> ApiResult<()> {
        let mut j = self.cleanup_job(id).map_err(|_| ApiError::NotFound(format!("could not find cleanup job with id {id}")))?;
        self.run_cleanup_job(&mut j);
        Ok(())
    }

    /// Schedules `j` on its cron line (autobrr's `startCleanupJob`).
    fn start_cleanup_job(&self, j: &CleanupJob) -> ApiResult<()> {
        if !j.enabled {
            return Err(ApiError::Internal(format!("cleanup job {} not enabled", j.name)));
        }
        let key = cleanup_key(j.id);
        let spec = super::cronspec::CronSpec::parse(&j.schedule)
            .map_err(|e| ApiError::Internal(format!("add cleanup job {key} failed: could not parse cron spec: {}: {e}", j.schedule)))?;
        let me = self.clone();
        let id = j.id;
        let work: super::scheduler::JobFn = std::sync::Arc::new(move || {
            let me = me.clone();
            Box::pin(async move {
                if let Ok(mut j) = me.cleanup_job(id) {
                    me.run_cleanup_job(&mut j);
                }
            })
        });
        self.scheduler.register(&key, Box::new(spec), work);
        Ok(())
    }

    /// Schedules every enabled cleanup job (autobrr's `StartCleanupJobs`).
    pub fn start_cleanup_jobs(&self) {
        for j in self.cleanup_jobs().unwrap_or_default().iter().filter(|j| j.enabled) {
            if let Err(e) = self.start_cleanup_job(j) {
                tracing::warn!("release cleanup: failed to initialize cleanup job {}: {}", j.name, e.message());
            }
        }
    }

    /// One run of a cleanup job (autobrr's `CleanupJob.Run`): deletes, then notes how it went.
    pub fn run_cleanup_job(&self, j: &mut CleanupJob) {
        let start = std::time::Instant::now();
        j.last_run = now_text();
        let indexers: Vec<String> = j.indexers.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        let statuses: Vec<String> =
            j.statuses.split(',').map(str::trim).filter(|s| !s.is_empty() && valid_push_status(s)).map(str::to_string).collect();
        let outcome = if !j.statuses.is_empty() && statuses.is_empty() {
            Err(format!("no valid statuses in configured filter: {}", j.statuses))
        } else {
            let req = DeleteRequest { older_than: j.older_than, indexers: indexers.clone(), statuses: statuses.clone() };
            self.delete_history(&req).map_err(|e| e.message().to_string())
        };
        match outcome {
            Ok(()) => {
                j.last_run_status = "SUCCESS".into();
                let null_or = |v: &Vec<String>| if v.is_empty() { Value::Null } else { json!(v) };
                j.last_run_data = json!({
                    "duration_ms": start.elapsed().as_millis() as i64,
                    "indexers": null_or(&indexers),
                    "older_than_hours": j.older_than,
                    "statuses": null_or(&statuses),
                })
                .to_string();
            }
            Err(e) => {
                j.last_run_status = "ERROR".into();
                j.last_run_data = e;
            }
        }
        let _ = self.db.execute(
            "UPDATE autobrr_release_cleanup_job SET last_run=?, last_run_status=?, last_run_data=? WHERE id=?",
            p![j.last_run, j.last_run_status, j.last_run_data, j.id],
        );
    }

    // ---- running actions (autobrr's release service) ----

    /// Runs `action` on `r`, settling `status` and telling the notifications (autobrr's `runAction`): an error
    /// is `PUSH_ERROR` with the error as its rejection, a client's no is `PUSH_REJECTED`, else `PUSH_APPROVED`.
    pub async fn run_history_action(&self, action: &super::action::Action, r: &mut Release, mut status: ActionStatus) -> ActionStatus {
        let runner = self.runner();
        let res = runner.run_action(action, r).await;
        let event = match res {
            Err(e) => {
                tracing::warn!("release {}: filter {}: error running actions: {e}", r.torrent_name, r.filter_name);
                status.status = PUSH_ERROR.into();
                status.rejections = vec![e];
                PUSH_ERROR
            }
            Ok(rej) if !rej.is_empty() => {
                status.status = PUSH_REJECTED.into();
                status.rejections = rej;
                PUSH_REJECTED
            }
            Ok(_) => {
                status.status = PUSH_APPROVED.into();
                PUSH_APPROVED
            }
        };
        status.timestamp = now_text();
        self.notify(super::notify::push_payload(event, r, action, &status));
        status
    }

    /// Runs an action of a stored release again (autobrr's `Retry` and `retryAction`): a new status, pending,
    /// then settled. A missing release, indexer, status or action answers not found.
    pub async fn retry_action(&self, release_id: i64, status_id: i64) -> ApiResult<()> {
        let mut r = self
            .history_release_for_retry(release_id)
            .map_err(|e| wrap_err(e, &format!("retry error: could not find release by id: {release_id}")))?;
        let ident = r.indexer.identifier.clone();
        let indexer = self
            .autobrr_indexers()?
            .into_iter()
            .find(|i| i.identifier == ident)
            .ok_or_else(|| ApiError::NotFound(format!("retry error: could not get indexer by identifier: {ident}: record not found")))?;
        r.indexer = super::release::IndexerMinimal {
            id: indexer.id,
            name: indexer.name.clone(),
            identifier: indexer.identifier.clone(),
            identifier_external: indexer.identifier_external.clone(),
        };
        let status = self.action_status(status_id).map_err(|e| wrap_err(e, "retry error: could not get release action"))?;
        let mut action = self.action(status.action_id).map_err(|e| wrap_err(e, "retry error: could not get filter action for release"))?;
        if action.client_id > 0 {
            action.client = self.downloader(action.client_id).ok();
        }
        let mut pending = ActionStatus::new(&action, &r);
        if let Err(e) = self.store_action_status(&mut pending) {
            tracing::warn!("release.retryAction: error storing action status: {}", e.message());
        }
        let mut done = self.run_history_action(&action, &mut r, pending).await;
        super::run::cleanup_temporary_files(&mut r);
        self.store_action_status(&mut done)?;
        if done.status == PUSH_ERROR {
            return Err(ApiError::Internal(done.rejections.first().cloned().unwrap_or_default()));
        }
        Ok(())
    }
}

/// `e` with `context` in front, as Go's `errors.Wrap` words it; a missing record stays a not-found.
fn wrap_err(e: ApiError, context: &str) -> ApiError {
    match e {
        ApiError::NotFound(m) => ApiError::NotFound(format!("{context}: {m}")),
        ApiError::BadRequest(m) => ApiError::BadRequest(format!("{context}: {m}")),
        ApiError::Internal(m) => ApiError::Internal(format!("{context}: {m}")),
    }
}

/// A cleanup job's name in the scheduler (autobrr's `cleanupJobKey.ToString`).
pub fn cleanup_key(id: i64) -> String {
    format!("release-cleanup-{id}")
}

/// The last `days` days ending today, UTC (0 or less: all time) (autobrr's `statsWindow`).
struct Window {
    /// The days asked for.
    days: i64,
    /// Now.
    now: DateTime<Utc>,
    /// The first day's midnight.
    cutoff: DateTime<Utc>,
}

impl Window {
    fn new(days: i64) -> Window {
        let now = Utc::now();
        let cutoff = (now - Duration::days((days - 1).max(0))).date_naive().and_hms_opt(0, 0, 0).map(|d| d.and_utc()).unwrap_or(now);
        Window { days, now, cutoff }
    }

    /// The time condition on `col`.
    fn sql(&self, col: &str) -> String {
        if self.days > 0 {
            format!("{col} >= ?")
        } else {
            "1=1".into()
        }
    }

    /// The time condition's value.
    fn args(&self) -> Vec<Value> {
        if self.days > 0 {
            vec![json!(self.cutoff.format("%Y-%m-%d %H:%M:%S").to_string())]
        } else {
            Vec::new()
        }
    }

    /// Every day to answer: the window, or from the first day seen through today for all time.
    fn dates(&self, first: Option<&str>) -> Vec<String> {
        let start = if self.days <= 0 {
            match first.and_then(|f| NaiveDate::parse_from_str(f, "%Y-%m-%d").ok()) {
                Some(d) => d,
                None => return Vec::new(),
            }
        } else {
            self.cutoff.date_naive()
        };
        let mut out = Vec::new();
        let mut d = start;
        while d <= self.now.date_naive() {
            out.push(d.format("%Y-%m-%d").to_string());
            d = d.succ_opt().unwrap_or(d);
            if out.len() > 40_000 {
                break;
            }
        }
        out
    }
}

/// A stored release as autobrr's JSON sends it; `found` for the list (which fills the indexer's name and splits
/// empty lists into `[""]`, as autobrr's `Find` does).
fn release_json(r: &Row, names: &BTreeMap<String, (i64, String, String)>, statuses: Vec<ActionStatus>, found: bool) -> Value {
    let ident = r.s("indexer");
    let (iid, iname, iext) = if found {
        names.get(&ident).cloned().map(|(a, b, c)| (a, if b.is_empty() { ident.clone() } else { b }, c)).unwrap_or((0, ident.clone(), String::new()))
    } else {
        (0, String::new(), String::new())
    };
    let lists = |k: &str| if found { json!(go_split(&r.s(k))) } else { json!(split_or_none(&r.s(k))) };
    let nullable = |k: &str| if found || !r.s(k).is_empty() { lists(k) } else { Value::Null };
    let mut head = json!({
        "id": r.i("id"),
        "filter_status": r.s("filter_status"),
        "rejections": list_of(&r.s("rejections")),
        "indexer": { "id": iid, "name": iname, "identifier": ident, "identifier_external": iext },
        "filter": r.s("filter"),
        "protocol": r.s("protocol"),
        "implementation": if found { String::new() } else { r.s("implementation") },
        "timestamp": r.s("timestamp"),
        "announce_type": r.s("announce_type"),
        "type": r.s("type"),
        "info_url": r.s("info_url"),
        "download_url": r.s("download_url"),
        "group_id": if found { String::new() } else { r.s("group_id") },
        "torrent_id": if found { String::new() } else { r.s("torrent_id") },
        "name": r.s("torrent_name"),
    });
    let rest = json!({
        "normalized_hash": r.s("normalized_hash"),
        "size": r.i("size"),
        "title": r.s("title"),
        "sub_title": r.s("sub_title"),
        "category": r.s("category"),
        "season": r.i("season"),
        "episode": r.i("episode"),
        "year": r.i("year"),
        "month": if found { 0 } else { r.i("month") },
        "day": if found { 0 } else { r.i("day") },
        "resolution": r.s("resolution"),
        "source": r.s("source"),
        "codec": nullable("codec"),
        "container": r.s("container"),
        "hdr": nullable("hdr"),
        "group": r.s("release_group"),
        "proper": r.b("proper"),
        "repack": r.b("repack"),
        "website": r.s("website"),
        "hybrid": r.b("hybrid"),
        "edition": nullable("edition"),
        "cut": nullable("cut"),
        "media_processing": r.s("media_processing"),
        "origin": if found { String::new() } else { r.s("origin") },
        "uploader": if found { String::new() } else { r.s("uploader") },
        "record_label": "",
        "pre_time": if found { String::new() } else { r.s("pre_time") },
        "action_status": statuses,
    });
    if let (Some(o), Value::Object(more)) = (head.as_object_mut(), rest) {
        o.extend(more);
    }
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The search parts autobrr's `reservedSearch` makes.
    #[test]
    fn search() {
        let (parts, args) = search_where("title:\"The Show\" group:GRP S01");
        assert_eq!(parts, vec!["(r.title LIKE ?)", "(r.release_group LIKE ?)", "r.torrent_name LIKE ?"]);
        assert_eq!(args, vec![json!("The Show%"), json!("GRP%"), json!("%S01%")]);
        let (parts, args) = search_where("Some.Release");
        assert_eq!(parts, vec!["r.torrent_name LIKE ?"]);
        assert_eq!(args, vec![json!("Some.Release%")]);
        let (_, args) = search_where("year:2024 title:a.b");
        assert_eq!(args, vec![json!("a_b%"), json!("2024%")]);
    }

    /// autobrr's `ReleaseCleanupJob.Validate`.
    #[test]
    fn job_validation() {
        let j = |name: &str, s: &str, o: i64, st: &str| CleanupJob {
            name: name.into(),
            schedule: s.into(),
            older_than: o,
            statuses: st.into(),
            ..CleanupJob::default()
        };
        assert_eq!(j("", "0 3 * * *", 1, "").validate().unwrap_err(), "name is required");
        assert_eq!(j("a", "", 1, "").validate().unwrap_err(), "schedule is required");
        assert_eq!(j("a", "0 3 * * *", 0, "").validate().unwrap_err(), "older_than must be greater than 0");
        assert!(j("a", "bad", 1, "").validate().unwrap_err().starts_with("invalid cron schedule: "));
        assert_eq!(j("a", "0 3 * * *", 1, "PUSH_APPROVED,NOPE").validate().unwrap_err(), "invalid status: NOPE");
        assert!(j("a", "0 3 * * *", 1, "PUSH_REJECTED, PUSH_ERROR").validate().is_ok());
        assert_eq!(cleanup_key(7), "release-cleanup-7");
    }
}
