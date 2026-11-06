// Ported from autobrr v1.90.0 (internal/list/service.go, processor.go, scheduled_jobs.go, internal/domain/list.go,
// internal/database/list.go), GPL-2.0-or-later.
//! autobrr's lists: an *arr's library (Sonarr, Radarr, Lidarr, Readarr, Whisparr, Sportarr) or a list on the web
//! (MDBList, Trakt, Metacritic, Steam, AniList, plain text) kept in the filters it is linked to. A refresh reads
//! the list and writes its titles into each linked filter's `shows`, `match_releases`, `albums` or `artists`
//! ([`process`]); a filter a list fills shows as auto-updated. Every list that is on is refreshed every six
//! hours, after a random wait of 1 to 35 seconds, when it is saved on, and on demand.
//!
//! Lists live in `autobrr_lists`, their filters in `autobrr_list_filters`.

pub mod process;
pub mod title;

use super::definition::{is_redacted, redact};
use super::feed::go_time;
use super::scheduler::Cron;
use super::store::{ApiError, ApiResult};
use super::Releases;
use chrono::{DateTime, SecondsFormat, Utc};
use kit_server_core::{p, Db, Row, RowExt, SqliteDb};
use process::FilterUpdate;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The scheduler name of the refresh of every list (autobrr's `lists-updater`).
pub const REFRESH_JOB: &str = "lists-updater";

/// A refresh worked.
pub const STATUS_SUCCESS: &str = "SUCCESS";
/// A refresh failed.
pub const STATUS_ERROR: &str = "ERROR";

/// The list types that read an *arr (autobrr's `ArrClient`).
pub const ARR_TYPES: [&str; 7] = ["RADARR", "SONARR", "LIDARR", "READARR", "WHISPARR", "WHISPARR_V3", "SPORTARR"];
/// The list types that read a list on the web (autobrr's `RegularList`).
pub const REGULAR_TYPES: [&str; 6] = ["MDBLIST", "METACRITIC", "PLAINTEXT", "TRAKT", "STEAM", "ANILIST"];

/// A filter a list fills (autobrr's `ListFilter`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListFilter {
    /// The filter's id.
    pub id: i64,
    /// Its name.
    pub name: String,
}

/// One list (autobrr's `List`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct List {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// One of [`ARR_TYPES`] or [`REGULAR_TYPES`].
    #[serde(rename = "type")]
    pub kind: String,
    /// On.
    pub enabled: bool,
    /// The *arr download client it reads (0: none).
    pub client_id: i64,
    /// The list's address (web lists).
    pub url: String,
    /// `Name=Value` headers sent along.
    pub headers: Option<Vec<String>>,
    /// The API key (Trakt; sent back redacted).
    pub api_key: String,
    /// The filters it fills (`null` when none are linked, in the list of lists).
    pub filters: Option<Vec<ListFilter>>,
    /// Titles go to the release names, `*`-wrapped, instead of the shows.
    pub match_release: bool,
    /// Only items with one of these *arr tags.
    pub tags_included: Option<Vec<String>>,
    /// No items with one of these *arr tags.
    pub tags_excluded: Option<Vec<String>>,
    /// Unmonitored items too.
    pub include_unmonitored: bool,
    /// The *arr's alternate titles too.
    pub include_alternate_titles: bool,
    /// A movie's year joins its title (MDBList, matching releases).
    pub include_year: bool,
    /// Plain text lines are taken as they are.
    pub skip_clean_sanitize: bool,
    /// When it was last refreshed (`None`: never).
    #[serde(deserialize_with = "time_in")]
    pub last_refresh_time: Option<DateTime<Utc>>,
    /// What the last failed refresh said (autobrr sends it as `last_refresh_error`).
    #[serde(rename = "last_refresh_error")]
    pub last_refresh_data: String,
    /// `SUCCESS` or `ERROR` (empty: never refreshed).
    pub last_refresh_status: String,
    /// When it was added.
    #[serde(deserialize_with = "time_in")]
    pub created_at: Option<DateTime<Utc>>,
    /// When it last changed.
    #[serde(deserialize_with = "time_in")]
    pub updated_at: Option<DateTime<Utc>>,
}

/// A Go time read from JSON: `null` and the zero time are none.
fn time_in<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
    let v: Option<String> = Option::deserialize(d)?;
    match v {
        None => Ok(None),
        Some(s) => {
            let t = DateTime::parse_from_rfc3339(&s)
                .map_err(|_| serde::de::Error::custom(format!("parsing time \"{s}\" as RFC 3339 failed")))?
                .with_timezone(&Utc);
            Ok((t.timestamp() > -62135596800).then_some(t))
        }
    }
}

impl Serialize for List {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("id", &self.id)?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("type", &self.kind)?;
        m.serialize_entry("enabled", &self.enabled)?;
        m.serialize_entry("client_id", &self.client_id)?;
        m.serialize_entry("url", &self.url)?;
        m.serialize_entry("headers", &self.headers)?;
        m.serialize_entry("filters", &self.filters)?;
        m.serialize_entry("match_release", &self.match_release)?;
        m.serialize_entry("tags_included", &self.tags_included)?;
        m.serialize_entry("tags_excluded", &self.tags_excluded)?;
        m.serialize_entry("include_unmonitored", &self.include_unmonitored)?;
        m.serialize_entry("include_alternate_titles", &self.include_alternate_titles)?;
        m.serialize_entry("include_year", &self.include_year)?;
        m.serialize_entry("skip_clean_sanitize", &self.skip_clean_sanitize)?;
        m.serialize_entry("last_refresh_time", &go_time(self.last_refresh_time))?;
        m.serialize_entry("last_refresh_error", &self.last_refresh_data)?;
        m.serialize_entry("last_refresh_status", &self.last_refresh_status)?;
        m.serialize_entry("created_at", &go_time(self.created_at))?;
        m.serialize_entry("updated_at", &go_time(self.updated_at))?;
        // Go's MarshalJSON puts the redacted copy after the rest
        m.serialize_entry("api_key", &redact(&self.api_key))?;
        m.end()
    }
}

impl List {
    /// Whether it reads an *arr.
    pub fn is_arr(&self) -> bool {
        ARR_TYPES.contains(&self.kind.as_str())
    }

    /// Whether it reads a list on the web.
    pub fn is_regular(&self) -> bool {
        REGULAR_TYPES.contains(&self.kind.as_str())
    }

    /// The included tags.
    pub fn tags_include(&self) -> &[String] {
        self.tags_included.as_deref().unwrap_or(&[])
    }

    /// The excluded tags.
    pub fn tags_exclude(&self) -> &[String] {
        self.tags_excluded.as_deref().unwrap_or(&[])
    }

    /// Whether an item counts: monitored, or the list takes unmonitored ones (autobrr's `ShouldProcessItem`).
    pub fn should_process_item(&self, monitored: bool) -> bool {
        self.include_unmonitored || monitored
    }

    /// Checks what a list needs (autobrr's `Validate`).
    pub fn validate(&self) -> ApiResult<()> {
        let fail = |m: String| Err(ApiError::Internal(m));
        if self.name.is_empty() {
            return fail("name is required".into());
        }
        if self.kind.is_empty() {
            return fail("type is required".into());
        }
        if !self.is_arr() && !self.is_regular() {
            return fail(format!("invalid list type: {}", self.kind));
        }
        if self.is_arr() && self.client_id == 0 {
            return fail("arr client id is required".into());
        }
        if self.is_regular() {
            if self.url.is_empty() {
                return fail("list url is required".into());
            }
            // Go's url.Parse takes nearly anything; it refuses control characters
            if self.url.chars().any(|c| c.is_ascii_control()) {
                return fail(format!(
                    "could not parse list url: {}: parse \"{}\": net/url: invalid control character in URL",
                    self.url, self.url
                ));
            }
        }
        if self.filters.as_ref().is_none_or(|f| f.is_empty()) {
            return fail("at least one filter is required".into());
        }
        Ok(())
    }
}

/// The list tables.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_lists (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 0,
            type TEXT NOT NULL,
            client_id INTEGER,
            url TEXT,
            headers TEXT NOT NULL DEFAULT '[]',
            api_key TEXT,
            match_release INTEGER NOT NULL DEFAULT 0,
            tags_included TEXT NOT NULL DEFAULT '[]',
            tags_excluded TEXT NOT NULL DEFAULT '[]',
            include_unmonitored INTEGER NOT NULL DEFAULT 0,
            include_alternate_titles INTEGER NOT NULL DEFAULT 0,
            include_year INTEGER NOT NULL DEFAULT 0,
            skip_clean_sanitize INTEGER NOT NULL DEFAULT 0,
            last_refresh_time TEXT,
            last_refresh_status TEXT,
            last_refresh_data TEXT,
            created_at TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS autobrr_list_filters (
            list_id INTEGER NOT NULL,
            filter_id INTEGER NOT NULL,
            PRIMARY KEY (list_id, filter_id)
        );",
    )
    .map_err(|e| anyhow::anyhow!("The autobrr list tables can't be made: {}", e.detail))
}

/// Now, as the tables keep it.
fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// A saved time; `None` for none.
fn time_of(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

/// A saved text list.
fn strings(r: &Row, k: &str) -> Vec<String> {
    serde_json::from_str(&r.s(k)).unwrap_or_default()
}

/// A list from its row, without its filters.
fn list_of(r: &Row) -> List {
    List {
        id: r.i("id"),
        name: r.s("name"),
        kind: r.s("type"),
        enabled: r.b("enabled"),
        client_id: r.i("client_id"),
        url: r.s("url"),
        headers: Some(strings(r, "headers")),
        api_key: r.s("api_key"),
        filters: None,
        match_release: r.b("match_release"),
        tags_included: Some(strings(r, "tags_included")),
        tags_excluded: Some(strings(r, "tags_excluded")),
        include_unmonitored: r.b("include_unmonitored"),
        include_alternate_titles: r.b("include_alternate_titles"),
        include_year: r.b("include_year"),
        skip_clean_sanitize: r.b("skip_clean_sanitize"),
        last_refresh_time: time_of(&r.s("last_refresh_time")),
        last_refresh_data: r.s("last_refresh_data"),
        last_refresh_status: r.s("last_refresh_status"),
        created_at: time_of(&r.s("created_at")),
        updated_at: time_of(&r.s("updated_at")),
    }
}

/// Go's record-not-found (`sql.ErrNoRows`).
fn no_rows() -> ApiError {
    ApiError::NotFound("sql: no rows in result set".into())
}

/// A text list as saved.
fn text(v: &Option<Vec<String>>) -> String {
    serde_json::to_string(&v.clone().unwrap_or_default()).unwrap_or_default()
}

impl Releases {
    /// The filters linked to lists, by list, each by name (autobrr's `GetAllListFilters`).
    fn all_list_filters(&self) -> ApiResult<BTreeMap<i64, Vec<ListFilter>>> {
        let rows = self.db.query(
            "SELECT lf.list_id, f.id, f.name FROM autobrr_list_filters lf JOIN release_filters f ON f.id = lf.filter_id ORDER BY f.name ASC",
            p![],
        )?;
        let mut m: BTreeMap<i64, Vec<ListFilter>> = BTreeMap::new();
        for r in &rows {
            m.entry(r.i("list_id")).or_default().push(ListFilter { id: r.i("id"), name: r.s("name") });
        }
        Ok(m)
    }

    /// The filters list `id` fills, by name (autobrr's `GetListFilters`).
    fn list_filters(&self, id: i64) -> ApiResult<Vec<ListFilter>> {
        Ok(self
            .db
            .query(
                "SELECT f.id, f.name FROM autobrr_list_filters lf JOIN release_filters f ON f.id = lf.filter_id WHERE lf.list_id=? ORDER BY f.name ASC",
                p![id],
            )?
            .iter()
            .map(|r| ListFilter { id: r.i("id"), name: r.s("name") })
            .collect())
    }

    /// Every list by name, only those on when `enabled_only`, with their filters (autobrr's `List`).
    pub fn lists(&self, enabled_only: bool) -> ApiResult<Vec<List>> {
        let sql = if enabled_only {
            "SELECT * FROM autobrr_lists WHERE enabled=1 ORDER BY name ASC"
        } else {
            "SELECT * FROM autobrr_lists ORDER BY name ASC"
        };
        let mut lists: Vec<List> = self.db.query(sql, p![])?.iter().map(list_of).collect();
        let mut filters = self.all_list_filters()?;
        for l in &mut lists {
            l.filters = filters.remove(&l.id);
        }
        Ok(lists)
    }

    /// List `id` with its filters (autobrr's `FindByID`).
    pub fn list_by_id(&self, id: i64) -> ApiResult<List> {
        let mut l = list_of(&self.db.one("SELECT * FROM autobrr_lists WHERE id=?", p![id])?.ok_or_else(no_rows)?);
        l.filters = Some(self.list_filters(l.id)?);
        Ok(l)
    }

    /// Links list `l` to exactly its filters (autobrr's `StoreListFilterConnection`); every filter must be
    /// there.
    fn store_list_filters(&self, l: &List) -> ApiResult<()> {
        let ids: Vec<i64> = l.filters.iter().flatten().map(|f| f.id).collect();
        for id in &ids {
            if self.db.one("SELECT id FROM release_filters WHERE id=?", p![id])?.is_none() {
                return Err(ApiError::Internal("error executing query: FOREIGN KEY constraint failed".into()));
            }
        }
        for r in self.db.query("SELECT filter_id FROM autobrr_list_filters WHERE list_id=?", p![l.id])? {
            if !ids.contains(&r.i("filter_id")) {
                self.db.execute("DELETE FROM autobrr_list_filters WHERE list_id=? AND filter_id=?", p![l.id, r.i("filter_id")])?;
            }
        }
        for id in ids {
            self.db.execute("INSERT OR IGNORE INTO autobrr_list_filters (list_id, filter_id) VALUES (?,?)", p![l.id, id])?;
        }
        Ok(())
    }

    /// Checks the download client a list names is there, as the table's foreign key does.
    fn check_list_client(&self, l: &List) -> ApiResult<()> {
        if l.client_id != 0 && self.db.one("SELECT id FROM autobrr_downloaders WHERE id=?", p![l.client_id])?.is_none() {
            return Err(ApiError::Internal("error executing query: FOREIGN KEY constraint failed".into()));
        }
        Ok(())
    }

    /// Adds list `l` and refreshes it when it is on (autobrr's `Store`).
    pub async fn store_list(&self, mut l: List) -> ApiResult<List> {
        l.validate()?;
        self.check_list_client(&l)?;
        for id in l.filters.iter().flatten().map(|f| f.id) {
            if self.db.one("SELECT id FROM release_filters WHERE id=?", p![id])?.is_none() {
                return Err(ApiError::Internal("error executing query: FOREIGN KEY constraint failed".into()));
            }
        }
        let now = now_text();
        l.id = self.db.execute(
            "INSERT INTO autobrr_lists (name, enabled, type, client_id, url, headers, api_key, match_release, tags_included, tags_excluded, include_unmonitored, include_alternate_titles, include_year, skip_clean_sanitize, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![
                l.name,
                l.enabled,
                l.kind,
                (l.client_id != 0).then_some(l.client_id),
                l.url,
                text(&l.headers),
                l.api_key,
                l.match_release,
                text(&l.tags_included),
                text(&l.tags_excluded),
                l.include_unmonitored,
                l.include_alternate_titles,
                l.include_year,
                l.skip_clean_sanitize,
                now,
                now
            ],
        )?;
        self.store_list_filters(&l)?;
        if l.enabled {
            self.process_list(&mut l).await?;
        }
        Ok(l)
    }

    /// Changes list `l`, keeping a redacted API key, and refreshes it when it is on (autobrr's `Update`).
    pub async fn update_list(&self, mut l: List) -> ApiResult<()> {
        l.validate()?;
        let cur = self.list_by_id(l.id)?;
        if is_redacted(&l.api_key) {
            l.api_key = cur.api_key.clone();
        }
        self.check_list_client(&l)?;
        if self.db.one("SELECT id FROM autobrr_lists WHERE id=?", p![l.id])?.is_none() {
            return Err(ApiError::Internal("update failed".into()));
        }
        self.db.execute(
            "UPDATE autobrr_lists SET name=?, enabled=?, type=?, client_id=?, url=?, headers=?, api_key=?, match_release=?, tags_included=?, tags_excluded=?, include_unmonitored=?, include_alternate_titles=?, include_year=?, skip_clean_sanitize=?, updated_at=? WHERE id=?",
            p![
                l.name,
                l.enabled,
                l.kind,
                (l.client_id != 0).then_some(l.client_id),
                l.url,
                text(&l.headers),
                l.api_key,
                l.match_release,
                text(&l.tags_included),
                text(&l.tags_excluded),
                l.include_unmonitored,
                l.include_alternate_titles,
                l.include_year,
                l.skip_clean_sanitize,
                now_text(),
                l.id
            ],
        )?;
        self.store_list_filters(&l)?;
        if l.enabled {
            self.process_list(&mut l).await?;
        }
        Ok(())
    }

    /// Removes list `id` and its filter links (autobrr's `Delete`).
    pub fn delete_list(&self, id: i64) -> ApiResult<()> {
        self.list_by_id(id)?;
        self.db.execute("DELETE FROM autobrr_lists WHERE id=?", p![id])?;
        self.db.execute("DELETE FROM autobrr_list_filters WHERE list_id=?", p![id])?;
        Ok(())
    }

    /// Refreshes list `id` (autobrr's `RefreshList`).
    pub async fn refresh_list(&self, id: i64) -> ApiResult<()> {
        let mut l = self.list_by_id(id)?;
        self.process_list(&mut l).await
    }

    /// Refreshes every list that is on (autobrr's `RefreshAll`).
    pub async fn refresh_all_lists(&self) -> ApiResult<()> {
        let lists = self.lists(true)?;
        self.refresh_lists(lists).await
    }

    /// Refreshes the *arr lists that are on (autobrr's `RefreshArrLists`).
    pub async fn refresh_arr_lists(&self) -> ApiResult<()> {
        let lists = self.lists(true)?.into_iter().filter(List::is_arr).collect();
        self.refresh_lists(lists).await
    }

    /// Refreshes the web lists that are on (autobrr's `RefreshOtherLists`).
    pub async fn refresh_other_lists(&self) -> ApiResult<()> {
        let lists = self.lists(true)?.into_iter().filter(List::is_regular).collect();
        self.refresh_lists(lists).await
    }

    /// Refreshes `lists` one after another; one whose client is gone is skipped, the other problems are
    /// joined, one a line (autobrr's `refreshAll`).
    async fn refresh_lists(&self, lists: Vec<List>) -> ApiResult<()> {
        let mut problems = Vec::new();
        for mut l in lists.into_iter().filter(|l| l.enabled) {
            match self.process_list(&mut l).await {
                Ok(()) => {}
                Err(ApiError::NotFound(_)) => tracing::warn!("list {}: its client isn't there, skipped", l.name),
                Err(e) => {
                    tracing::warn!("list {}: couldn't be refreshed: {}", l.name, e.message());
                    problems.push(format!("error while refreshing {}: {}", l.name, e.message()));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ApiError::Internal(problems.join("\n")))
        }
    }

    /// The *arr client of list `l`: it must be there, on and of the list's kind (autobrr's `getClientInstance`).
    fn list_client(&self, l: &List) -> ApiResult<super::clients::arr::Client> {
        let d = self.db.one("SELECT id FROM autobrr_downloaders WHERE id=?", p![l.client_id])?.ok_or_else(no_rows)?;
        let d = self.downloader(d.i("id"))?;
        if !d.enabled {
            return Err(ApiError::Internal(format!("client {} {} not enabled", d.kind, d.name)));
        }
        let want: &[&str] = match l.kind.as_str() {
            "WHISPARR" | "WHISPARR_V3" => &["WHISPARR", "WHISPARR_V3"],
            k => &[k][..],
        };
        let app = super::clients::arr::App::of(&d.kind).filter(|_| want.contains(&d.kind.as_str()));
        match app {
            Some(app) => Ok(super::run::arr_client(app, &d)),
            None => Err(ApiError::Internal(format!(
                "download client {} ({}) has runtime type *{}.Client",
                d.id,
                d.kind,
                d.kind.to_lowercase().trim_end_matches("_v3")
            ))),
        }
    }

    /// Reads list `l` and writes its titles into its filters, noting how it went (autobrr's `Process`).
    pub async fn process_list(&self, l: &mut List) -> ApiResult<()> {
        let client = if l.is_arr() {
            Some(self.list_client(l)?)
        } else if l.is_regular() {
            None
        } else {
            return Err(ApiError::Internal(format!("unknown list type: {}", l.kind)));
        };
        let res = match &client {
            Some(c) => process::arr_list(l, c).await,
            None => process::other_list(l).await,
        };
        let outcome = match res {
            Err(e) => Err(format!("could not make new request for URL: {e}")),
            Ok(None) => Ok(()),
            Ok(Some(update)) => self.apply_list_update(l, &update),
        };
        l.last_refresh_time = Some(Utc::now());
        match &outcome {
            Err(e) => {
                l.last_refresh_status = STATUS_ERROR.into();
                l.last_refresh_data = e.clone();
            }
            Ok(()) => l.last_refresh_status = STATUS_SUCCESS.into(),
        }
        self.update_last_refresh(l)?;
        outcome.map_err(ApiError::Internal)
    }

    /// Writes `update` into each of list `l`'s filters (autobrr's `process`).
    fn apply_list_update(&self, l: &List, update: &FilterUpdate) -> Result<(), String> {
        let body = update.to_json();
        for f in l.filters.iter().flatten() {
            self.autobrr_update_partial(f.id, &body).map_err(|e| format!("error updating filter: {}: {}", f.id, e.message()))?;
        }
        Ok(())
    }

    /// Notes list `l`'s last refresh (autobrr's `UpdateLastRefresh`).
    fn update_last_refresh(&self, l: &List) -> ApiResult<()> {
        if self.db.one("SELECT id FROM autobrr_lists WHERE id=?", p![l.id])?.is_none() {
            return Err(ApiError::Internal("update failed".into()));
        }
        self.db.execute(
            "UPDATE autobrr_lists SET last_refresh_time=?, last_refresh_status=?, last_refresh_data=?, updated_at=? WHERE id=?",
            p![
                l.last_refresh_time.map(|t| t.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
                l.last_refresh_status,
                l.last_refresh_data,
                now_text(),
                l.id
            ],
        )?;
        Ok(())
    }

    /// Refreshes every list that is on every six hours, after a random wait of 1 to 35 seconds (autobrr's
    /// `RefreshListsJob`).
    pub fn start_lists(&self) {
        let rel = self.clone();
        self.scheduler.register(
            REFRESH_JOB,
            Box::new(Cron::EveryHours(6)),
            Arc::new(move || {
                let rel = rel.clone();
                Box::pin(async move {
                    let delay = rand::random_range(1..=35u64);
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    if let Err(e) = rel.refresh_all_lists().await {
                        tracing::warn!("lists: not every list could be refreshed: {}", e.message());
                    }
                })
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/domain/list.go `Validate` and the list JSON.
    use super::*;

    fn valid() -> List {
        List {
            name: "l".into(),
            kind: "TRAKT".into(),
            url: "https://api.autobrr.com/lists/trakt/anticipated-tv".into(),
            filters: Some(vec![ListFilter { id: 1, name: "f".into() }]),
            ..List::default()
        }
    }

    #[test]
    fn validate() {
        assert!(valid().validate().is_ok());
        let msg = |l: List| l.validate().unwrap_err().message().to_string();
        assert_eq!(msg(List { name: String::new(), ..valid() }), "name is required");
        assert_eq!(msg(List { kind: String::new(), ..valid() }), "type is required");
        assert_eq!(msg(List { kind: "IMDB".into(), ..valid() }), "invalid list type: IMDB");
        assert_eq!(msg(List { kind: "SONARR".into(), ..valid() }), "arr client id is required");
        assert!(List { kind: "SONARR".into(), client_id: 2, url: String::new(), ..valid() }.validate().is_ok());
        assert_eq!(msg(List { url: String::new(), ..valid() }), "list url is required");
        assert_eq!(msg(List { filters: Some(vec![]), ..valid() }), "at least one filter is required");
        assert_eq!(msg(List { filters: None, ..valid() }), "at least one filter is required");
    }

    #[test]
    fn json_is_gos() {
        let v = serde_json::to_value(List { api_key: "k".into(), last_refresh_data: "boom".into(), ..valid() }).unwrap();
        assert_eq!(v["api_key"], "<redacted>");
        assert_eq!(v["last_refresh_error"], "boom");
        assert_eq!(v["last_refresh_time"], "0001-01-01T00:00:00Z");
        assert!(v["headers"].is_null());
        let back: List = serde_json::from_value(v).unwrap();
        assert_eq!(back.last_refresh_data, "boom");
        assert_eq!(back.last_refresh_time, None);
    }
}
