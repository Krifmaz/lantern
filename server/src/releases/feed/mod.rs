// Ported from autobrr v1.90.0 (internal/feed/service.go, cleanup.go, internal/domain/feed.go,
// internal/database/feed.go, feed_cache.go), GPL-2.0-or-later.
//! autobrr's feeds: a Torznab, Newznab or RSS indexer's feed, read on its own interval. Each feed belongs to one
//! feed indexer (a `feed` row of `release_sources`) and lives in `autobrr_feeds`; what a feed already showed is
//! remembered in `autobrr_feed_cache` until its time to live runs out, so nothing is sent to the filters twice.
//!
//! A feed is read only while both it and its indexer are on: every change goes through
//! [`Releases::sync_feed_job`], which reads the saved feed again and starts or stops its job to match. A forced run
//! and a scheduled run of the same feed never overlap. Once a day, at 03:05, the cache entries that ran out and
//! those of removed feeds are deleted.
//!
//! | Module | What |
//! | --- | --- |
//! | [`refresh`] | one run: page by page back to what an earlier run cached |
//! | [`sources`] | the three formats as releases |
//! | [`znab`] | the Torznab and Newznab client |
//! | [`gofeed`] | RSS, RDF, Atom and JSON Feed, read as gofeed does |
//! | [`xml`] | the small XML tree the readers share |

pub mod gofeed;
pub mod refresh;
pub mod sources;
pub mod xml;
pub mod znab;

use super::definition::{is_redacted, redact};
use super::release::Release;
use super::scheduler::{AnchoredSchedule, Cron, JobFn};
use super::store::{ApiError, ApiResult};
use super::Releases;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use kit_server_core::{p, Db, Row, RowExt, SqliteDb};
use refresh::{CacheRepo, FeedRepo, Handoff, RefreshJob, Source};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// How many days a cached item is kept when the feed doesn't say (autobrr's `DefaultFeedCacheTTLDays`).
pub const DEFAULT_FEED_CACHE_TTL_DAYS: i64 = 31;
/// How many pages a run reads when the feed doesn't say (autobrr's `DefaultFeedPages`).
pub const DEFAULT_FEED_PAGES: i64 = 1;
/// The most pages a run reads (autobrr's `MaxFeedPages`).
pub const MAX_FEED_PAGES: i64 = 10;

/// A Torznab feed.
pub const FEED_TYPE_TORZNAB: &str = "TORZNAB";
/// A Newznab feed.
pub const FEED_TYPE_NEWZNAB: &str = "NEWZNAB";
/// An RSS feed.
pub const FEED_TYPE_RSS: &str = "RSS";

/// Releases are fetched as magnet links.
pub const DOWNLOAD_TYPE_MAGNET: &str = "MAGNET";
/// Releases are fetched as torrent files.
pub const DOWNLOAD_TYPE_TORRENT: &str = "TORRENT";
/// Releases are fetched as NZB files.
pub const DOWNLOAD_TYPE_NZB: &str = "NZB";

/// New items go to the filters oldest first (also the default, empty).
pub const PROCESS_ORDER_OLDEST_FIRST: &str = "OLDEST_FIRST";
/// New items go to the filters newest first.
pub const PROCESS_ORDER_NEWEST_FIRST: &str = "NEWEST_FIRST";

/// The scheduler name of the daily cache cleanup.
pub const CLEANUP_JOB: &str = "feed-cache-cleanup";

/// The indexer a feed belongs to, as feeds send it (autobrr's `IndexerMinimal`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedIndexer {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Its identifier (`torznab-name`).
    pub identifier: String,
    /// Its identifier elsewhere.
    pub identifier_external: String,
}

/// A feed's settings (autobrr's `FeedSettingsJSON`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedSettings {
    /// `MAGNET`, `TORRENT` or `NZB`.
    pub download_type: String,
    /// Days a cached item is kept (0: 31).
    pub cache_ttl_days: i64,
    /// Pages a run may read (0: 1, at most 10).
    pub max_pages: i64,
    /// `OLDEST_FIRST` (or empty) or `NEWEST_FIRST`.
    pub process_order: String,
}

/// The indexer's own description of itself (autobrr's `FeedCapsServer`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedCapsServer {
    /// Its version.
    pub version: String,
    /// Its title.
    pub title: String,
    /// Its strapline.
    pub strapline: String,
    /// Its email address.
    pub email: String,
    /// Its address.
    pub url: String,
    /// Its image.
    pub image: String,
}

/// The page sizes (autobrr's `FeedCapabilitiesLimits`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedCapsLimits {
    /// The largest page.
    pub max: i64,
    /// The default page.
    pub default: i64,
}

/// One kind of search (autobrr's `FeedCapsSearch`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedCapsSearch {
    /// `yes` or `no`.
    pub available: String,
    /// The parameters it takes.
    #[serde(rename = "supportedParams")]
    pub supported_params: String,
    /// The search engine.
    #[serde(rename = "searchEngine")]
    pub search_engine: String,
}

/// The kinds of search (autobrr's `FeedCapabilitiesSearching`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedCapsSearching {
    /// A plain search.
    pub search: FeedCapsSearch,
    /// TV.
    #[serde(rename = "tv-search")]
    pub tv_search: FeedCapsSearch,
    /// Movies.
    #[serde(rename = "movie-search")]
    pub movie_search: FeedCapsSearch,
    /// Music.
    #[serde(rename = "audio-search")]
    pub audio_search: FeedCapsSearch,
    /// Books.
    #[serde(rename = "book-search")]
    pub book_search: FeedCapsSearch,
}

/// One category (autobrr's `FeedCapabilitiesCategory`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedCapsCategory {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Its sub-categories (`null` for a sub-category's own).
    pub subcategories: Option<Vec<FeedCapsCategory>>,
}

/// What a Torznab or Newznab indexer can do (autobrr's `FeedCapabilities`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedCapabilities {
    /// The indexer.
    pub server: FeedCapsServer,
    /// The page sizes.
    pub limits: FeedCapsLimits,
    /// The categories.
    pub categories: Option<Vec<FeedCapsCategory>>,
    /// The kinds of search.
    pub searching: FeedCapsSearching,
}

impl FeedCapabilities {
    /// The capabilities read from an indexer's caps answer; page sizes of 0 or less become 100 and 50 (autobrr's
    /// `NewFeedCapabilitiesFromTorznab` and `…FromNewznab`).
    pub fn from_caps(c: &znab::Caps) -> FeedCapabilities {
        let [version, title, strapline, email, url, image] = c.server.clone();
        let search = |s: &znab::Search| FeedCapsSearch {
            available: s.available.clone(),
            supported_params: s.supported_params.clone(),
            search_engine: String::new(),
        };
        FeedCapabilities {
            server: FeedCapsServer { version, title, strapline, email, url, image },
            limits: FeedCapsLimits {
                max: if c.limits_max > 0 { c.limits_max } else { 100 },
                default: if c.limits_default > 0 { c.limits_default } else { 50 },
            },
            categories: Some(
                c.categories
                    .iter()
                    .map(|cat| FeedCapsCategory {
                        id: cat.id,
                        name: cat.name.clone(),
                        subcategories: Some(
                            cat.sub_categories
                                .iter()
                                .map(|s| FeedCapsCategory { id: s.id, name: s.name.clone(), subcategories: None })
                                .collect(),
                        ),
                    })
                    .collect(),
            ),
            searching: FeedCapsSearching {
                search: search(&c.searching[0]),
                tv_search: search(&c.searching[1]),
                movie_search: search(&c.searching[2]),
                audio_search: search(&c.searching[3]),
                book_search: search(&c.searching[4]),
            },
        }
    }
}

/// One feed (autobrr's `Feed`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Feed {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The indexer it belongs to.
    pub indexer: FeedIndexer,
    /// `TORZNAB`, `NEWZNAB` or `RSS`.
    #[serde(rename = "type")]
    pub kind: String,
    /// On.
    pub enabled: bool,
    /// The address.
    pub url: String,
    /// Minutes between runs.
    pub interval: i64,
    /// Seconds a request may take (0: 60).
    pub timeout: i64,
    /// Items older than this many seconds are skipped (0: no limit).
    pub max_age: i64,
    /// The categories asked for (`null` when none).
    pub categories: Option<Vec<i64>>,
    /// What the indexer can do.
    pub capabilities: Option<FeedCapabilities>,
    /// The API key (sent back redacted).
    pub api_key: String,
    /// The cookie sent along (sent back redacted).
    pub cookie: String,
    /// The user agent sent (empty: the default).
    pub user_agent: String,
    /// Certificates aren't checked.
    pub tls_skip_verify: bool,
    /// Its settings.
    pub settings: Option<FeedSettings>,
    /// When it was added.
    #[serde(deserialize_with = "go_time_in")]
    pub created_at: Option<DateTime<Utc>>,
    /// When it last changed.
    #[serde(deserialize_with = "go_time_in")]
    pub updated_at: Option<DateTime<Utc>>,
    /// The indexer's id, as a new feed names it.
    pub indexer_id: i64,
    /// When it was last read (`None`: never).
    #[serde(deserialize_with = "go_time_in")]
    pub last_run: Option<DateTime<Utc>>,
    /// The first page of the last run, as it came.
    pub last_run_data: String,
    /// When it is read next (`None`: no job).
    #[serde(deserialize_with = "go_time_in")]
    pub next_run: Option<DateTime<Utc>>,
    /// Its indexer is on (never sent).
    #[serde(skip)]
    pub indexer_enabled: bool,
    /// Its indexer goes through a proxy (never sent).
    #[serde(skip)]
    pub use_proxy: bool,
    /// Its indexer's proxy (never sent).
    #[serde(skip)]
    pub proxy_id: i64,
}

/// A time as Go's JSON writes it; Go's zero time for none.
pub fn go_time(t: Option<DateTime<Utc>>) -> String {
    match t {
        Some(t) => t.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        None => "0001-01-01T00:00:00Z".into(),
    }
}

/// A Go time read from JSON: `null` and the zero time are none, anything else must be RFC 3339.
fn go_time_in<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
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

impl Serialize for Feed {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("id", &self.id)?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("indexer", &self.indexer)?;
        m.serialize_entry("type", &self.kind)?;
        m.serialize_entry("enabled", &self.enabled)?;
        m.serialize_entry("url", &self.url)?;
        m.serialize_entry("interval", &self.interval)?;
        m.serialize_entry("timeout", &self.timeout)?;
        m.serialize_entry("max_age", &self.max_age)?;
        m.serialize_entry("categories", &self.categories)?;
        m.serialize_entry("capabilities", &self.capabilities)?;
        m.serialize_entry("user_agent", &self.user_agent)?;
        m.serialize_entry("tls_skip_verify", &self.tls_skip_verify)?;
        m.serialize_entry("settings", &self.settings)?;
        m.serialize_entry("created_at", &go_time(self.created_at))?;
        m.serialize_entry("updated_at", &go_time(self.updated_at))?;
        if self.indexer_id != 0 {
            m.serialize_entry("indexer_id", &self.indexer_id)?;
        }
        m.serialize_entry("last_run", &go_time(self.last_run))?;
        m.serialize_entry("last_run_data", &self.last_run_data)?;
        m.serialize_entry("next_run", &go_time(self.next_run))?;
        // Go's MarshalJSON puts the redacted copies after the rest
        m.serialize_entry("api_key", &redact(&self.api_key))?;
        m.serialize_entry("cookie", &redact(&self.cookie))?;
        m.end()
    }
}

/// Whether `v` can be sent as a header value: no control characters but tab (Go's
/// `httpguts.ValidHeaderFieldValue`).
fn valid_header_value(v: &str) -> bool {
    !v.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f)
}

impl Feed {
    /// Rejects what would make every request fail (autobrr's `Validate`).
    pub fn validate(&self) -> ApiResult<()> {
        if !valid_header_value(&self.user_agent) {
            return Err(ApiError::Internal("user agent must not contain control characters".into()));
        }
        if let Some(s) = &self.settings {
            match s.process_order.as_str() {
                "" | PROCESS_ORDER_OLDEST_FIRST | PROCESS_ORDER_NEWEST_FIRST => {}
                o => return Err(ApiError::Internal(format!("invalid process order: {o}"))),
            }
        }
        Ok(())
    }

    /// The download type (empty without settings).
    pub fn download_type(&self) -> &str {
        self.settings.as_ref().map(|s| s.download_type.as_str()).unwrap_or("")
    }

    /// Days a cached item is kept (autobrr's `CacheTTLDays`).
    pub fn cache_ttl_days(&self) -> i64 {
        match &self.settings {
            Some(s) if s.cache_ttl_days > 0 => s.cache_ttl_days,
            _ => DEFAULT_FEED_CACHE_TTL_DAYS,
        }
    }

    /// When an item cached now runs out (autobrr's `CacheTTL`).
    pub fn cache_ttl(&self) -> DateTime<Utc> {
        Utc::now() + Duration::days(self.cache_ttl_days())
    }

    /// Whether what the last run cached is still kept at `now` (autobrr's `CacheCoversLastRun`).
    pub fn cache_covers_last_run(&self, now: DateTime<Utc>) -> bool {
        self.last_run.is_some_and(|l| l > now - Duration::days(self.cache_ttl_days()))
    }

    /// The pages a run may read, at most [`MAX_FEED_PAGES`] (autobrr's `PaginationMaxPages`).
    pub fn pagination_max_pages(&self) -> usize {
        match &self.settings {
            Some(s) if s.max_pages > 0 => s.max_pages.min(MAX_FEED_PAGES) as usize,
            _ => DEFAULT_FEED_PAGES as usize,
        }
    }

    /// Whether new items go to the filters newest first (autobrr's `ProcessNewestFirst`).
    pub fn process_newest_first(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| s.process_order == PROCESS_ORDER_NEWEST_FIRST)
    }
}

/// One cached item (autobrr's `FeedCacheItem`).
#[derive(Debug, Clone, PartialEq)]
pub struct FeedCacheItem {
    /// The feed's id, as text.
    pub feed_id: String,
    /// What the item is known by.
    pub key: String,
    /// Its title.
    pub value: String,
    /// When it runs out.
    pub ttl: DateTime<Utc>,
}

/// The feed and cache tables.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_feeds (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            indexer_id INTEGER,
            name TEXT NOT NULL DEFAULT '',
            type TEXT NOT NULL DEFAULT '',
            enabled INTEGER NOT NULL DEFAULT 0,
            url TEXT NOT NULL DEFAULT '',
            interval INTEGER NOT NULL DEFAULT 0,
            timeout INTEGER NOT NULL DEFAULT 60,
            max_age INTEGER NOT NULL DEFAULT 0,
            categories TEXT NOT NULL DEFAULT '[]',
            capabilities TEXT NOT NULL DEFAULT '{}',
            api_key TEXT,
            cookie TEXT,
            user_agent TEXT,
            tls_skip_verify INTEGER NOT NULL DEFAULT 0,
            settings TEXT,
            last_run TEXT,
            last_run_data TEXT,
            created_at TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS autobrr_feed_cache (
            feed_id INTEGER NOT NULL,
            key TEXT,
            value TEXT,
            ttl INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS autobrr_feed_cache_feed_id_key ON autobrr_feed_cache (feed_id, key);",
    )
    .map_err(|e| anyhow::anyhow!("The autobrr feed tables can't be made: {}", e.detail))
}

/// Now, to the second, as SQL's `CURRENT_TIMESTAMP` keeps it.
fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A saved time; `None` for none.
fn time_of(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

/// A saved JSON column as autobrr reads it: no value is none, a JSON `null` the zero value.
fn json_column<T: serde::de::DeserializeOwned + Default>(r: &Row, k: &str, what: &str) -> ApiResult<Option<T>> {
    match r.get(k) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => {
            let text = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| ApiError::Internal(format!("error unmarshal {what}: {e}")))?;
            if v.is_null() {
                return Ok(Some(T::default()));
            }
            serde_json::from_value(v).map(Some).map_err(|e| ApiError::Internal(format!("error unmarshal {what}: {e}")))
        }
    }
}

/// The columns every feed read takes, with its indexer's.
const FEED_SELECT: &str = "SELECT f.*, s.id AS i_id, s.name AS i_name, s.definition AS i_definition, s.enabled AS i_enabled,
    m.source_id AS i_meta, m.identifier_external AS i_external, m.use_proxy AS i_use_proxy, m.proxy_id AS i_proxy_id
    FROM autobrr_feeds f";

/// Which feed [`Releases::feed_find_one`] looks for (autobrr's `FindOneParams`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FindOne {
    /// By the feed's id.
    Feed(i64),
    /// By its indexer's id.
    Indexer(i64),
}

/// A feed from its row; `indexer_id` kept only when `with_indexer_id` (autobrr's `FindOne` reads it, `Find` and
/// `FindByID` don't).
fn feed_of(r: &Row, with_indexer_id: bool) -> ApiResult<Feed> {
    let categories: Vec<i64> = serde_json::from_str(&r.s("categories")).unwrap_or_default();
    let mut f = Feed {
        id: r.i("id"),
        name: r.s("name"),
        kind: r.s("type"),
        enabled: r.b("enabled"),
        url: r.s("url"),
        interval: r.i("interval"),
        timeout: r.i("timeout"),
        max_age: r.i("max_age"),
        categories: (!categories.is_empty()).then_some(categories),
        capabilities: json_column(r, "capabilities", "capabilities")?,
        api_key: r.s("api_key"),
        cookie: r.s("cookie"),
        user_agent: r.s("user_agent"),
        tls_skip_verify: r.b("tls_skip_verify"),
        settings: json_column(r, "settings", "settings")?,
        created_at: time_of(&r.s("created_at")),
        updated_at: time_of(&r.s("updated_at")),
        indexer_id: if with_indexer_id { r.i("indexer_id") } else { 0 },
        last_run: time_of(&r.s("last_run")),
        ..Feed::default()
    };
    if !r.get("i_id").is_none_or(|v| v.is_null()) {
        let name = r.s("i_name");
        let definition = r.s("i_definition");
        let has_meta = !r.get("i_meta").is_none_or(|v| v.is_null());
        f.indexer = FeedIndexer {
            id: r.i("i_id"),
            identifier: if definition.is_empty() { name.to_lowercase() } else { definition },
            identifier_external: if has_meta { r.s("i_external") } else { name.clone() },
            name,
        };
        f.indexer_enabled = r.b("i_enabled");
        f.use_proxy = r.b("i_use_proxy");
        f.proxy_id = r.i("i_proxy_id");
    }
    Ok(f)
}

/// The scheduler name of feed `id`'s job (autobrr's `feedKey`).
pub fn feed_key(id: i64) -> String {
    format!("feed-{id}")
}

/// One feed's locks: `schedule` keeps two syncs from applying out of order, `run` keeps a forced run and a
/// scheduled one apart (autobrr's `feedGuards`).
#[derive(Default)]
pub struct FeedGuard {
    /// Held while the job is started or stopped.
    pub schedule: Mutex<()>,
    /// Held while the feed is read.
    pub run: Arc<tokio::sync::Mutex<()>>,
}

/// Every feed's locks, by feed id.
#[derive(Clone, Default)]
pub struct FeedGuards(Arc<Mutex<HashMap<i64, Arc<FeedGuard>>>>);

impl FeedGuards {
    /// Feed `id`'s locks.
    pub fn of(&self, id: i64) -> Arc<FeedGuard> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).entry(id).or_default().clone()
    }

    /// Forgets feed `id`'s locks.
    pub fn forget(&self, id: i64) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }
}

/// The feed tables as a refresh job sees them.
#[derive(Clone)]
pub struct DbRepo(pub SqliteDb);

impl FeedRepo for DbRepo {
    fn update_last_run_with_data(&self, feed_id: i64, data: &str) -> Result<(), String> {
        self.0
            .execute("UPDATE autobrr_feeds SET last_run=?, last_run_data=? WHERE id=?", p![now_text(), data, feed_id])
            .map(|_| ())
            .map_err(|e| format!("error executing query: {}", e.detail))
    }
}

impl CacheRepo for DbRepo {
    fn existing_items(&self, feed_id: i64, keys: &[String]) -> Result<HashSet<String>, String> {
        let mut out = HashSet::new();
        // a few hundred at a time stays far below SQLite's parameter limit
        for chunk in keys.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut params = p![feed_id];
            params.extend(chunk.iter().map(|k| serde_json::json!(k)));
            let rows = self
                .0
                .query(&format!("SELECT key FROM autobrr_feed_cache WHERE feed_id=? AND key IN ({marks})"), params)
                .map_err(|e| format!("error executing query: {}", e.detail))?;
            out.extend(rows.iter().map(|r| r.s("key")));
        }
        Ok(out)
    }

    fn put_many(&self, items: &[FeedCacheItem]) -> Result<(), String> {
        for it in items {
            self.0
                .execute(
                    "INSERT INTO autobrr_feed_cache (feed_id, key, value, ttl) VALUES (?,?,?,?)",
                    p![it.feed_id.parse::<i64>().unwrap_or(0), it.key, it.value, it.ttl.timestamp_millis()],
                )
                .map_err(|e| format!("error executing query: {}", e.detail))?;
        }
        Ok(())
    }
}

/// A feed's source: the Torznab and Newznab ones also read capabilities.
pub enum AnySource {
    /// Torznab or Newznab.
    Znab(Box<sources::ZnabSource>),
    /// RSS.
    Rss(sources::RssSource),
}

impl AnySource {
    /// As a refresh job's source.
    pub fn boxed(self) -> Box<dyn Source> {
        match self {
            AnySource::Znab(s) => s,
            AnySource::Rss(s) => Box::new(s),
        }
    }
}

/// The source of feed `f`, the only way a feed is reached (autobrr's `newSource`). autobrr's proxies aren't
/// part of Lantern, so a feed whose indexer asks for one is refused.
pub fn new_source(f: &Feed) -> ApiResult<AnySource> {
    if f.use_proxy {
        return Err(ApiError::Internal("could not find proxy for indexer feed: record not found".into()));
    }
    let f = Arc::new(f.clone());
    match f.kind.as_str() {
        FEED_TYPE_TORZNAB => Ok(AnySource::Znab(Box::new(sources::ZnabSource::new(f, znab::Kind::Torznab)))),
        FEED_TYPE_NEWZNAB => Ok(AnySource::Znab(Box::new(sources::ZnabSource::new(f, znab::Kind::Newznab)))),
        FEED_TYPE_RSS => Ok(AnySource::Rss(sources::RssSource::new(f))),
        t => Err(ApiError::Internal(format!("unsupported feed type: {t}"))),
    }
}

/// Wraps `e`'s message after `what`, keeping a not-found a not-found (autobrr's `errors.Wrap`).
fn wrap(e: ApiError, what: &str) -> ApiError {
    match e {
        ApiError::NotFound(m) => ApiError::NotFound(format!("{what}: {m}")),
        ApiError::BadRequest(m) => ApiError::BadRequest(format!("{what}: {m}")),
        ApiError::Internal(m) => ApiError::Internal(format!("{what}: {m}")),
    }
}

/// The record-not-found error.
fn not_found() -> ApiError {
    ApiError::NotFound("record not found".into())
}

impl Releases {
    /// Every feed whose indexer is there, by name, with when each runs next (autobrr's `Find`).
    pub fn feeds(&self) -> ApiResult<Vec<Feed>> {
        let rows = self.db.query(
            &format!("{FEED_SELECT} JOIN release_sources s ON f.indexer_id = s.id LEFT JOIN autobrr_indexer_meta m ON m.source_id = s.id ORDER BY f.name ASC"),
            p![],
        )?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let mut f = feed_of(r, false)?;
            if r.get("settings").is_none_or(|v| v.is_null()) {
                f.settings = Some(FeedSettings { download_type: DOWNLOAD_TYPE_TORRENT.into(), ..FeedSettings::default() });
            }
            f.next_run = self.scheduler.next_run(&feed_key(f.id));
            out.push(f);
        }
        Ok(out)
    }

    /// One feed, whether or not its indexer is still there (autobrr's `FindOne`).
    pub fn feed_find_one(&self, by: FindOne) -> ApiResult<Feed> {
        let (filter, id) = match by {
            FindOne::Feed(0) | FindOne::Indexer(0) => return Err(ApiError::Internal("invalid params".into())),
            FindOne::Feed(id) => ("f.id", id),
            FindOne::Indexer(id) => ("f.indexer_id", id),
        };
        let r = self
            .db
            .one(
                &format!("{FEED_SELECT} LEFT JOIN release_sources s ON f.indexer_id = s.id LEFT JOIN autobrr_indexer_meta m ON m.source_id = s.id WHERE {filter}=?"),
                p![id],
            )?
            .ok_or_else(not_found)?;
        feed_of(&r, true)
    }

    /// Feed `id`, with its indexer (autobrr's `FindByID`).
    pub fn feed_by_id(&self, id: i64) -> ApiResult<Feed> {
        let r = self
            .db
            .one(
                &format!("{FEED_SELECT} JOIN release_sources s ON f.indexer_id = s.id LEFT JOIN autobrr_indexer_meta m ON m.source_id = s.id WHERE f.id=?"),
                p![id],
            )?
            .ok_or_else(not_found)?;
        feed_of(&r, false)
    }

    /// What feed `id`'s last run read (autobrr's `GetLastRunData`).
    pub fn feed_last_run_data(&self, id: i64) -> ApiResult<String> {
        let r = self.db.one("SELECT last_run_data FROM autobrr_feeds WHERE id=?", p![id])?.ok_or_else(not_found)?;
        Ok(r.s("last_run_data"))
    }

    /// Adds feed `f` (autobrr's `Store`): its name, type, address, interval, timeout, categories, capabilities,
    /// API key, user agent, certificate setting, indexer and settings; its job starts when it is turned on.
    pub fn store_feed(&self, mut f: Feed) -> ApiResult<Feed> {
        f.validate()?;
        if self.db.one("SELECT id FROM release_sources WHERE id=?", p![f.indexer_id])?.is_none() {
            return Err(ApiError::Internal("error executing query: FOREIGN KEY constraint failed".into()));
        }
        let now = now_text();
        f.id = self.db.execute(
            "INSERT INTO autobrr_feeds (name, type, enabled, url, interval, timeout, categories, capabilities, api_key, user_agent, tls_skip_verify, indexer_id, settings, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![
                f.name,
                f.kind,
                f.enabled,
                f.url,
                f.interval,
                f.timeout,
                serde_json::to_string(&f.categories.clone().unwrap_or_default()).unwrap_or_default(),
                serde_json::to_string(&f.capabilities).unwrap_or_default(),
                f.api_key,
                f.user_agent,
                f.tls_skip_verify,
                f.indexer_id,
                serde_json::to_string(&f.settings).unwrap_or_default(),
                now,
                now
            ],
        )?;
        Ok(f)
    }

    /// Changes feed `f` (autobrr's `Update`): a redacted API key or cookie keeps the saved one; the job is
    /// brought in line after.
    pub fn update_feed(&self, mut f: Feed) -> ApiResult<Feed> {
        f.validate()?;
        let cur = self.feed_find_one(FindOne::Feed(f.id))?;
        if is_redacted(&f.api_key) {
            f.api_key = cur.api_key.clone();
        }
        if is_redacted(&f.cookie) {
            f.cookie = cur.cookie.clone();
        }
        self.db.execute(
            "UPDATE autobrr_feeds SET name=?, type=?, enabled=?, url=?, interval=?, timeout=?, max_age=?, categories=?, capabilities=?, api_key=?, cookie=?, user_agent=?, tls_skip_verify=?, settings=?, updated_at=? WHERE id=?",
            p![
                f.name,
                f.kind,
                f.enabled,
                f.url,
                f.interval,
                f.timeout,
                f.max_age,
                serde_json::to_string(&f.categories.clone().unwrap_or_default()).unwrap_or_default(),
                serde_json::to_string(&f.capabilities).unwrap_or_default(),
                f.api_key,
                f.cookie,
                f.user_agent,
                f.tls_skip_verify,
                serde_json::to_string(&f.settings).unwrap_or_default(),
                now_text(),
                f.id
            ],
        )?;
        self.sync_feed_job(f.id)?;
        Ok(f)
    }

    /// Stops and removes feed `id` and its cache (autobrr's `Delete`).
    pub fn delete_feed(&self, id: i64) -> ApiResult<()> {
        let f = self.feed_find_one(FindOne::Feed(id))?;
        self.stop_feed_job(f.id);
        self.db.execute("DELETE FROM autobrr_feeds WHERE id=?", p![f.id])?;
        if let Err(e) = self.db.execute("DELETE FROM autobrr_feed_cache WHERE feed_id=?", p![id]) {
            tracing::warn!("feed {}: its cache couldn't be removed: {}", f.name, e.detail);
        }
        self.feed_guards.forget(id);
        Ok(())
    }

    /// Forgets what feed `id` has seen (autobrr's `DeleteFeedCache`).
    pub fn delete_feed_cache(&self, id: i64) -> ApiResult<()> {
        self.feed_by_id(id)?;
        self.db.execute("DELETE FROM autobrr_feed_cache WHERE feed_id=?", p![id])?;
        Ok(())
    }

    /// Turns feed `id` on or off and brings its job in line (autobrr's `ToggleEnabled`).
    pub fn toggle_feed(&self, id: i64, enabled: bool) -> ApiResult<()> {
        self.feed_find_one(FindOne::Feed(id))?;
        self.db.execute("UPDATE autobrr_feeds SET enabled=?, updated_at=? WHERE id=?", p![enabled, now_text(), id])?;
        self.sync_feed_job(id)
    }

    /// Reads the first page of feed `f` as sent, with the saved API key and cookie for redacted ones (autobrr's
    /// `Test`).
    pub async fn test_feed(&self, mut f: Feed) -> ApiResult<()> {
        f.validate()?;
        let cur = self.feed_find_one(FindOne::Feed(f.id))?;
        if is_redacted(&f.api_key) {
            f.api_key = cur.api_key.clone();
        }
        if is_redacted(&f.cookie) {
            f.cookie = cur.cookie.clone();
        }
        f.use_proxy = cur.use_proxy;
        f.proxy_id = cur.proxy_id;
        let mut src = new_source(&f)?.boxed();
        let res = src.fetch(0).await.map_err(ApiError::Internal)?;
        tracing::info!("feed {}: test read {} items", f.name, res.entries.len());
        Ok(())
    }

    /// What the indexer of feed `f` can do (autobrr's `FetchCaps`).
    pub async fn fetch_feed_caps(&self, f: Option<Feed>) -> ApiResult<FeedCapabilities> {
        let f = f.ok_or_else(|| ApiError::Internal("feed is required".into()))?;
        if f.url.is_empty() {
            return Err(ApiError::Internal("feed URL is required".into()));
        }
        match new_source(&f)? {
            AnySource::Znab(s) => s.caps().await.map_err(ApiError::Internal),
            AnySource::Rss(_) => Err(ApiError::Internal(format!("feed type {} has no capabilities", f.kind))),
        }
    }

    /// Reads and saves what the indexer of feed `id` can do (autobrr's `FetchCapsByID`).
    pub async fn fetch_feed_caps_by_id(&self, id: i64) -> ApiResult<FeedCapabilities> {
        let f = self.feed_by_id(id)?;
        let fid = f.id;
        let caps = self.fetch_feed_caps(Some(f)).await?;
        let n = self.db.one("SELECT id FROM autobrr_feeds WHERE id=?", p![fid])?;
        if n.is_none() {
            return Err(not_found());
        }
        self.db.execute(
            "UPDATE autobrr_feeds SET capabilities=?, updated_at=? WHERE id=?",
            p![serde_json::to_string(&caps).unwrap_or_default(), now_text(), fid],
        )?;
        Ok(caps)
    }

    /// The refresh job of feed `f` (autobrr's `newRefreshJob`).
    fn new_refresh_job(&self, f: &Feed) -> ApiResult<RefreshJob> {
        let src = new_source(f).map_err(|e| wrap(e, &format!("could not create source for feed: {}", f.name)))?;
        let repo = Arc::new(DbRepo(self.db.clone()));
        let rel = self.clone();
        let indexer = f.indexer.clone();
        let handoff: Handoff = Arc::new(move |list: Vec<Release>| {
            let rel = rel.clone();
            let indexer = indexer.clone();
            Box::pin(async move { rel.process_feed_releases(list, &indexer).await })
        });
        Ok(RefreshJob { feed: f.clone(), src: src.boxed(), repo: repo.clone(), cache: repo, handoff: Some(handoff), has_fetched: false })
    }

    /// Sends the new releases of a feed of `indexer` through the filters, one after another; nothing when no
    /// filter that is on watches the indexer (autobrr's `ProcessMultipleFromIndexer`).
    pub async fn process_feed_releases(&self, list: Vec<Release>, indexer: &FeedIndexer) {
        let watched = match self.filters() {
            Ok(fs) => fs.iter().any(|f| f.enabled && f.indexers.contains(&indexer.id)),
            Err(e) => {
                tracing::warn!("feed of {}: the filters couldn't be read: {}", indexer.name, e.detail);
                return;
            }
        };
        if !watched {
            tracing::debug!("feed of {}: no active filters found for indexer, {} releases skipped", indexer.name, list.len());
            return;
        }
        for r in list {
            self.process_release(r).await;
        }
    }

    /// Starts feed `f`'s job, or says why it can't (autobrr's `startJob`).
    fn start_feed_job(&self, f: &Feed) -> ApiResult<()> {
        if !f.enabled {
            return Err(ApiError::Internal(format!("feed {} not enabled", f.name)));
        }
        if !f.indexer_enabled {
            return Err(ApiError::Internal(format!("indexer for feed {} not enabled", f.name)));
        }
        if f.url.is_empty() {
            return Err(ApiError::Internal(format!("no URL provided for feed: {}", f.name)));
        }
        let job = Arc::new(tokio::sync::Mutex::new(self.new_refresh_job(f)?));
        let run = self.feed_guards.of(f.id).run.clone();
        let name = f.name.clone();
        let work: JobFn = Arc::new(move || {
            let (job, run, name) = (job.clone(), run.clone(), name.clone());
            Box::pin(async move {
                // a task of its own, so a job replaced mid-run still finishes the run it began
                let task = tokio::spawn(async move {
                    let Ok(_held) = run.try_lock_owned() else {
                        tracing::debug!("feed {name}: still running, this run is skipped");
                        return;
                    };
                    if let Err(e) = job.lock().await.run().await {
                        tracing::warn!("feed {name}: couldn't be read: {e}");
                    }
                });
                let _ = task.await;
            })
        });
        let key = feed_key(f.id);
        let schedule = AnchoredSchedule::new(Duration::minutes(f.interval), f.last_run, Utc::now(), &key);
        self.scheduler.register(&key, Box::new(schedule), work);
        tracing::debug!("feed {}: started", f.name);
        Ok(())
    }

    /// Stops feed `id`'s job.
    fn stop_feed_job(&self, id: i64) {
        self.scheduler.remove(&feed_key(id));
    }

    /// Brings feed `id`'s job in line with the saved feed and its indexer: running only while both are on
    /// (autobrr's `syncFeedJob`).
    pub fn sync_feed_job(&self, id: i64) -> ApiResult<()> {
        let guard = self.feed_guards.of(id);
        let _held = guard.schedule.lock().unwrap_or_else(|e| e.into_inner());
        let f = match self.feed_find_one(FindOne::Feed(id)) {
            Ok(f) => f,
            Err(ApiError::NotFound(_)) => {
                self.stop_feed_job(id);
                return Ok(());
            }
            Err(e) => return Err(wrap(e, &format!("could not find feed {id}"))),
        };
        if !f.enabled || !f.indexer_enabled {
            self.stop_feed_job(f.id);
            return Ok(());
        }
        self.start_feed_job(&f)
    }

    /// Reads feed `id` now, unless it is already being read, then schedules the next run a full interval
    /// later (autobrr's `ForceRun`).
    pub async fn force_run_feed(&self, id: i64) -> ApiResult<()> {
        let f = self.feed_by_id(id)?;
        let mut job = self.new_refresh_job(&f)?;
        let run = self.feed_guards.of(f.id).run.clone();
        let Ok(_held) = run.try_lock_owned() else {
            return Err(ApiError::Internal(format!("feed {} is already running", f.name)));
        };
        job.run().await.map_err(ApiError::Internal)?;
        drop(_held);
        if let Err(e) = self.sync_feed_job(id) {
            tracing::warn!("feed {}: couldn't be rescheduled after the forced run: {}", f.name, e.message());
        }
        Ok(())
    }

    /// The feed of indexer `indexer_id` is removed with it (autobrr's `onIndexerDeleted`).
    pub fn feed_indexer_deleted(&self, indexer_id: i64) {
        match self.feed_find_one(FindOne::Indexer(indexer_id)) {
            Ok(f) => {
                if let Err(e) = self.delete_feed(f.id) {
                    tracing::warn!("feed {}: couldn't be removed with its indexer: {}", f.name, e.message());
                }
            }
            Err(ApiError::NotFound(_)) => {}
            Err(e) => tracing::warn!("the feed of indexer {indexer_id} couldn't be found: {}", e.message()),
        }
    }

    /// The feed of indexer `indexer_id` follows the indexer being turned on or off (autobrr's
    /// `ToggleIndexerEnabled`).
    pub fn feed_indexer_toggled(&self, indexer_id: i64) {
        match self.feed_find_one(FindOne::Indexer(indexer_id)) {
            Ok(f) => {
                if let Err(e) = self.sync_feed_job(f.id) {
                    tracing::warn!("feed {}: its job couldn't follow its indexer: {}", f.name, e.message());
                }
            }
            Err(ApiError::NotFound(_)) => {}
            Err(e) => tracing::warn!("the feed of indexer {indexer_id} couldn't be found: {}", e.message()),
        }
    }

    /// Deletes the cached items that ran out, then those of removed feeds (autobrr's `CleanupJob`).
    pub fn clean_feed_cache(&self) -> ApiResult<()> {
        self.db.execute("DELETE FROM autobrr_feed_cache WHERE ttl < ?", p![Utc::now().timestamp_millis()])?;
        self.db.execute(
            "DELETE FROM autobrr_feed_cache WHERE NOT EXISTS (SELECT 1 FROM autobrr_feeds WHERE autobrr_feeds.id = autobrr_feed_cache.feed_id)",
            p![],
        )?;
        Ok(())
    }

    /// Starts the daily cache cleanup and, in the background five seconds apart, every feed that is on with its
    /// indexer (autobrr's `Start`).
    pub fn start_feeds(&self) {
        let rel = self.clone();
        self.scheduler.register(
            CLEANUP_JOB,
            Box::new(Cron::Daily(3, 5)),
            Arc::new(move || {
                let rel = rel.clone();
                Box::pin(async move {
                    match rel.clean_feed_cache() {
                        Ok(()) => tracing::info!("feeds: the cache was cleaned up"),
                        Err(e) => tracing::warn!("feeds: the cache couldn't be cleaned up: {}", e.message()),
                    }
                })
            }),
        );
        let feeds = match self.feeds() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("feeds: couldn't be read: {}", e.message());
                return;
            }
        };
        let rel = self.clone();
        tokio::spawn(async move {
            for f in feeds.into_iter().filter(|f| f.enabled && f.indexer_enabled) {
                if let Err(e) = rel.sync_feed_job(f.id) {
                    tracing::warn!("feed {}: couldn't be started: {}", f.name, e.message());
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/domain/feed_test.go (settings helpers and validation).
    use super::*;

    fn with(s: FeedSettings) -> Feed {
        Feed { settings: Some(s), ..Feed::default() }
    }

    #[test]
    fn cache_ttl_days_and_pages() {
        assert_eq!(Feed::default().cache_ttl_days(), 31);
        assert_eq!(with(FeedSettings { cache_ttl_days: 7, ..FeedSettings::default() }).cache_ttl_days(), 7);
        assert_eq!(Feed::default().pagination_max_pages(), 1);
        assert_eq!(with(FeedSettings { max_pages: 4, ..FeedSettings::default() }).pagination_max_pages(), 4);
        assert_eq!(with(FeedSettings { max_pages: 50, ..FeedSettings::default() }).pagination_max_pages(), 10);
        assert!(!Feed::default().process_newest_first());
        assert!(with(FeedSettings { process_order: "NEWEST_FIRST".into(), ..FeedSettings::default() }).process_newest_first());
    }

    #[test]
    fn cache_covers_last_run() {
        let now = Utc::now();
        let mut f = Feed::default();
        assert!(!f.cache_covers_last_run(now));
        f.last_run = Some(now - Duration::days(3));
        assert!(f.cache_covers_last_run(now));
        f.last_run = Some(now - Duration::days(40));
        assert!(!f.cache_covers_last_run(now));
    }

    #[test]
    fn validate() {
        assert!(Feed::default().validate().is_ok());
        let bad = Feed { user_agent: "a\r\nX: y".into(), ..Feed::default() };
        assert_eq!(bad.validate().unwrap_err().message(), "user agent must not contain control characters");
        assert!(Feed { user_agent: "Mozilla/5.0\t(é)".into(), ..Feed::default() }.validate().is_ok());
        let order = with(FeedSettings { process_order: "RANDOM".into(), ..FeedSettings::default() });
        assert_eq!(order.validate().unwrap_err().message(), "invalid process order: RANDOM");
        assert!(with(FeedSettings { process_order: "OLDEST_FIRST".into(), ..FeedSettings::default() }).validate().is_ok());
    }

    #[test]
    fn json_is_gos() {
        let f = Feed { id: 1, api_key: "secret".into(), ..Feed::default() };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["api_key"], "<redacted>");
        assert_eq!(v["cookie"], "");
        assert_eq!(v["last_run"], "0001-01-01T00:00:00Z");
        assert!(v.get("indexer_id").is_none());
        assert!(v["categories"].is_null());
        let back: Feed = serde_json::from_value(v).unwrap();
        assert_eq!(back.last_run, None);
        let caps = FeedCapabilities::default();
        let v = serde_json::to_value(&caps).unwrap();
        assert!(v["searching"]["tv-search"]["supportedParams"].is_string());
    }
}
