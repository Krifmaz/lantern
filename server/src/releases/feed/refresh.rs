// Ported from autobrr v1.90.0 (internal/feed/refresh.go, source.go), GPL-2.0-or-later.
//! One feed refresh (autobrr's `refreshJob`): the feed is read page by page back to the first item an earlier
//! run cached, each page's new items are cached as it goes, and the new items younger than the feed's max age go
//! to the filters, oldest first (or newest first when the feed says so). A new or long-paused feed reads only its
//! first page, so its backlog never reaches the filters.

use super::{Feed, FeedCacheItem};
use crate::releases::release::Release;
use chrono::{DateTime, Duration, TimeZone, Utc};
use futures_util::future::BoxFuture;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One item of a page (autobrr's `entry`).
#[derive(Clone)]
pub struct Entry {
    /// What the cache knows it by (the guid).
    pub key: String,
    /// Its title.
    pub title: String,
    /// When it was published (`None`: unknown).
    pub pub_date: Option<DateTime<Utc>>,
    /// The item as a release, made only when it is new.
    pub release: Arc<dyn Fn() -> Release + Send + Sync>,
}

/// One page (autobrr's `fetchResult`).
#[derive(Clone, Default)]
pub struct FetchResult {
    /// The answer as it came, kept as the feed's last run data.
    pub raw: String,
    /// The items with a key.
    pub entries: Vec<Entry>,
    /// Every item in the answer, also those without a key (where the next page starts).
    pub items: usize,
    /// The page size asked for (0: no paging).
    pub limit: usize,
    /// The total the indexer reports (0: not reported).
    pub total: usize,
}

impl FetchResult {
    /// Whether the feed has items past the page read at `offset` (autobrr's `hasMore`).
    pub fn has_more(&self, offset: usize) -> bool {
        if self.items == 0 {
            return false;
        }
        if self.total > 0 {
            return offset + self.items < self.total;
        }
        self.limit > 0 && self.items >= self.limit
    }
}

/// Reads one feed format (autobrr's `source`).
pub trait Source: Send {
    /// The page starting at `offset`.
    fn fetch(&mut self, offset: usize) -> BoxFuture<'_, Result<FetchResult, String>>;
}

/// Where a run notes its last run (autobrr's `jobFeedRepo`).
pub trait FeedRepo: Send + Sync {
    /// Notes the run now, with the first page as it came.
    fn update_last_run_with_data(&self, feed_id: i64, data: &str) -> Result<(), String>;
}

/// The seen-item cache a run reads and fills (autobrr's `jobFeedCacheRepo`).
pub trait CacheRepo: Send + Sync {
    /// Which of `keys` feed `feed_id` already cached.
    fn existing_items(&self, feed_id: i64, keys: &[String]) -> Result<HashSet<String>, String>;
    /// Caches `items`.
    fn put_many(&self, items: &[FeedCacheItem]) -> Result<(), String>;
}

/// Where new releases go (autobrr's `ProcessMultipleFromIndexer`).
pub type Handoff = Arc<dyn Fn(Vec<Release>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Publish dates before this are taken as "unknown" (autobrr's `minPubDate`).
pub fn min_pub_date() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1970, 4, 1, 0, 0, 0).unwrap()
}

/// Whether `item` is younger than `max_age` seconds at `now` (autobrr's `isNewerThanMaxAge`).
pub fn is_newer_than_max_age(max_age: i64, item: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    item > now - Duration::seconds(max_age)
}

/// Whether the oldest dated entry is older than `cutoff` (autobrr's `reachedCutoff`).
pub fn reached_cutoff(entries: &[Entry], cutoff: Option<DateTime<Utc>>) -> bool {
    let Some(cutoff) = cutoff else { return false };
    let oldest = entries.iter().filter_map(|e| e.pub_date).filter(|d| *d > min_pub_date()).min();
    oldest.is_some_and(|o| o < cutoff)
}

/// A dated item.
fn dated(e: &Entry) -> Option<DateTime<Utc>> {
    e.pub_date.filter(|d| *d > min_pub_date())
}

/// One feed's refresh job; it lives as long as the feed's schedule (autobrr's `refreshJob`).
pub struct RefreshJob {
    /// The feed as it was when scheduled.
    pub feed: Feed,
    /// Reads the feed.
    pub src: Box<dyn Source>,
    /// Notes the last run.
    pub repo: Arc<dyn FeedRepo>,
    /// The seen-item cache.
    pub cache: Arc<dyn CacheRepo>,
    /// Where new releases go.
    pub handoff: Option<Handoff>,
    /// A run has cached its first page, so the next run may read further back.
    pub has_fetched: bool,
}

impl RefreshJob {
    /// Reads the new items and hands those young enough to the filters (autobrr's `Run`).
    pub async fn run(&mut self) -> Result<(), String> {
        let entries = self.fetch_new().await?;
        let now = Utc::now();
        let mut releases = Vec::with_capacity(entries.len());
        for e in &entries {
            if self.feed.max_age > 0 {
                if let Some(d) = dated(e) {
                    if !is_newer_than_max_age(self.feed.max_age, d, now) {
                        tracing::debug!("feed {}: {} is older than the feed's max age, skipped", self.feed.name, e.title);
                        continue;
                    }
                }
            }
            releases.push((e.release)());
        }
        if releases.is_empty() {
            return Ok(());
        }
        if self.feed.process_newest_first() {
            releases.reverse();
        }
        if let Some(h) = &self.handoff {
            h(releases).await;
        }
        Ok(())
    }

    /// One page unless a cached boundary from an earlier run is there (autobrr's `maxPages`).
    fn max_pages(&self) -> usize {
        let pages = self.feed.pagination_max_pages();
        if pages > 1 && !self.has_fetched && !self.feed.cache_covers_last_run(Utc::now()) {
            return 1;
        }
        pages
    }

    /// The new items, oldest first, cached page by page (autobrr's `fetchNew`).
    pub async fn fetch_new(&mut self) -> Result<Vec<Entry>, String> {
        let max_pages = self.max_pages();
        let cutoff = (self.feed.max_age > 0).then(|| Utc::now() - Duration::seconds(self.feed.max_age));
        let ttl = self.feed.cache_ttl();
        let feed_id = self.feed.id;
        let mut fresh: Vec<Entry> = Vec::new();
        let mut offset = 0usize;
        let mut seen: HashSet<String> = HashSet::new();
        for page in 1..=max_pages {
            let res = match self.src.fetch(offset).await {
                Ok(r) => r,
                Err(e) if page == 1 => return Err(format!("could not fetch feed: {e}")),
                Err(e) => {
                    tracing::warn!("feed {}: page {page} couldn't be read, stopping there: {e}", self.feed.name);
                    break;
                }
            };
            if page == 1 {
                if let Err(e) = self.repo.update_last_run_with_data(feed_id, &res.raw) {
                    tracing::warn!("feed {}: the last run couldn't be noted: {e}", self.feed.name);
                }
            }
            let unseen: Vec<String> = res.entries.iter().filter(|e| !seen.contains(&e.key)).map(|e| e.key.clone()).collect();
            let existing = match self.cache.existing_items(feed_id, &unseen) {
                Ok(x) => x,
                Err(e) if page == 1 => return Err(format!("could not check existing items: {e}")),
                Err(e) => {
                    tracing::warn!("feed {}: the cache couldn't be read for page {page}: {e}", self.feed.name);
                    break;
                }
            };
            let mut boundary = false;
            let mut page_fresh = Vec::new();
            let mut to_cache = Vec::new();
            for e in &res.entries {
                if !seen.insert(e.key.clone()) {
                    continue;
                }
                if existing.contains(&e.key) {
                    boundary = true;
                    continue;
                }
                to_cache.push(FeedCacheItem { feed_id: feed_id.to_string(), key: e.key.clone(), value: e.title.clone(), ttl });
                page_fresh.push(e.clone());
            }
            if !to_cache.is_empty() {
                if let Err(e) = self.cache.put_many(&to_cache) {
                    tracing::warn!("feed {}: page {page} couldn't be cached, left for the next run: {e}", self.feed.name);
                    break;
                }
            }
            if page == 1 {
                self.has_fetched = true;
            }
            fresh.extend(page_fresh);
            if boundary || reached_cutoff(&res.entries, cutoff) || !res.has_more(offset) || to_cache.is_empty() {
                break;
            }
            offset += res.items;
        }
        fresh.reverse();
        Ok(fresh)
    }
}

/// A cache in memory, for tests and the feed test (nothing is kept).
#[derive(Default)]
pub struct MemoryCache {
    /// The cached keys by feed.
    pub keys: std::sync::Mutex<HashMap<i64, HashSet<String>>>,
}

impl CacheRepo for MemoryCache {
    fn existing_items(&self, feed_id: i64, keys: &[String]) -> Result<HashSet<String>, String> {
        let m = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let have = m.get(&feed_id);
        Ok(keys.iter().filter(|k| have.is_some_and(|h| h.contains(*k))).cloned().collect())
    }
    fn put_many(&self, items: &[FeedCacheItem]) -> Result<(), String> {
        let mut m = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        for i in items {
            m.entry(i.feed_id.parse().unwrap_or(0)).or_default().insert(i.key.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/feed/refresh_test.go.
    use super::super::FeedSettings;
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn new_entry(key: &str, pub_date: Option<DateTime<Utc>>) -> Entry {
        let k = key.to_string();
        Entry {
            key: key.into(),
            title: key.into(),
            pub_date,
            release: Arc::new(move || Release { torrent_name: k.clone(), ..Release::default() }),
        }
    }

    struct Stub(Option<FetchResult>, Option<String>);
    impl Source for Stub {
        fn fetch(&mut self, _: usize) -> BoxFuture<'_, Result<FetchResult, String>> {
            let r = match &self.1 {
                Some(e) => Err(e.clone()),
                None => Ok(self.0.clone().unwrap_or_default()),
            };
            Box::pin(async move { r })
        }
    }

    #[derive(Default)]
    struct RecRepo(Mutex<Vec<String>>);
    impl FeedRepo for RecRepo {
        fn update_last_run_with_data(&self, _: i64, data: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(data.to_string());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecCache {
        existing: HashSet<String>,
        existing_err: Option<String>,
        stored: Mutex<Vec<FeedCacheItem>>,
    }
    impl CacheRepo for RecCache {
        fn existing_items(&self, _: i64, _: &[String]) -> Result<HashSet<String>, String> {
            match &self.existing_err {
                Some(e) => Err(e.clone()),
                None => Ok(self.existing.clone()),
            }
        }
        fn put_many(&self, items: &[FeedCacheItem]) -> Result<(), String> {
            self.stored.lock().unwrap().extend(items.iter().cloned());
            Ok(())
        }
    }

    fn handoff() -> (Handoff, Arc<Mutex<Vec<Vec<String>>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let h: Handoff = Arc::new(move |r: Vec<Release>| {
            g.lock().unwrap().push(r.iter().map(|x| x.torrent_name.clone()).collect());
            Box::pin(async {})
        });
        (h, got)
    }

    fn feed(max_age: i64) -> Feed {
        Feed { id: 4, max_age, ..Feed::default() }
    }

    #[tokio::test]
    async fn test_refresh_job_run_e() {
        let now = Utc::now();
        let m = |n: i64| Some(now - Duration::minutes(n));
        let set = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        struct Case {
            name: &'static str,
            max_age: i64,
            res: Option<Vec<Entry>>,
            fetch_err: Option<&'static str>,
            existing: HashSet<String>,
            existing_err: Option<&'static str>,
            want_err: bool,
            want_raw: Vec<&'static str>,
            want_cached: Vec<&'static str>,
            want_handed: Option<Vec<&'static str>>,
        }
        let cases = vec![
            Case {
                name: "new items are cached and handed off oldest first",
                max_age: 0,
                res: Some(vec![new_entry("c", m(0)), new_entry("b", m(1)), new_entry("a", m(2))]),
                fetch_err: None,
                existing: set(&[]),
                existing_err: None,
                want_err: false,
                want_raw: vec!["raw"],
                want_cached: vec!["c", "b", "a"],
                want_handed: Some(vec!["a", "b", "c"]),
            },
            Case {
                name: "cached items are skipped",
                max_age: 0,
                res: Some(vec![new_entry("b", m(0)), new_entry("a", m(0))]),
                fetch_err: None,
                existing: set(&["a"]),
                existing_err: None,
                want_err: false,
                want_raw: vec![""],
                want_cached: vec!["b"],
                want_handed: Some(vec!["b"]),
            },
            Case {
                name: "a key listed twice is handled once",
                max_age: 0,
                res: Some(vec![new_entry("a", m(0)), new_entry("a", m(0))]),
                fetch_err: None,
                existing: set(&[]),
                existing_err: None,
                want_err: false,
                want_raw: vec![""],
                want_cached: vec!["a"],
                want_handed: Some(vec!["a"]),
            },
            Case {
                name: "items older than max age are cached but not handed off",
                max_age: 3600,
                res: Some(vec![new_entry("new", m(1)), new_entry("old", m(120)), new_entry("undated", None)]),
                fetch_err: None,
                existing: set(&[]),
                existing_err: None,
                want_err: false,
                want_raw: vec![""],
                want_cached: vec!["new", "old", "undated"],
                want_handed: Some(vec!["undated", "new"]),
            },
            Case {
                name: "nothing new is not handed off",
                max_age: 0,
                res: Some(vec![new_entry("a", m(0))]),
                fetch_err: None,
                existing: set(&["a"]),
                existing_err: None,
                want_err: false,
                want_raw: vec![""],
                want_cached: vec![],
                want_handed: None,
            },
            Case {
                name: "fetch error stops the run",
                max_age: 0,
                res: None,
                fetch_err: Some("unexpected status code: 503"),
                existing: set(&[]),
                existing_err: None,
                want_err: true,
                want_raw: vec![],
                want_cached: vec![],
                want_handed: None,
            },
            Case {
                name: "cache lookup error stops the run",
                max_age: 0,
                res: Some(vec![new_entry("a", m(0))]),
                fetch_err: None,
                existing: set(&[]),
                existing_err: Some("database is locked"),
                want_err: true,
                want_raw: vec![""],
                want_cached: vec![],
                want_handed: None,
            },
        ];
        for c in cases {
            let raw = if c.name.starts_with("new items") { "raw" } else { "" };
            let res = c.res.map(|entries| FetchResult { raw: raw.into(), entries, ..FetchResult::default() });
            let repo = Arc::new(RecRepo::default());
            let cache =
                Arc::new(RecCache { existing: c.existing, existing_err: c.existing_err.map(str::to_string), ..RecCache::default() });
            let (h, got) = handoff();
            let mut j = RefreshJob {
                feed: feed(c.max_age),
                src: Box::new(Stub(res, c.fetch_err.map(str::to_string))),
                repo: repo.clone(),
                cache: cache.clone(),
                handoff: Some(h),
                has_fetched: false,
            };
            let r = j.run().await;
            assert_eq!(r.is_err(), c.want_err, "{}", c.name);
            assert_eq!(*repo.0.lock().unwrap(), c.want_raw, "{}: last run data", c.name);
            let stored = cache.stored.lock().unwrap();
            assert!(stored.iter().all(|i| i.feed_id == "4"));
            let cached: Vec<String> = stored.iter().map(|i| i.key.clone()).collect();
            assert_eq!(cached, c.want_cached, "{}: cached keys", c.name);
            let got = got.lock().unwrap();
            match c.want_handed {
                None => assert!(got.is_empty(), "{}: nothing should be handed off", c.name),
                Some(w) => assert_eq!(got[0], w, "{}: handed off", c.name),
            }
        }
        // the error says what failed, as autobrr's refresh event does
        let mut j = RefreshJob {
            feed: feed(0),
            src: Box::new(Stub(None, Some("unexpected status code: 503".into()))),
            repo: Arc::new(RecRepo::default()),
            cache: Arc::new(RecCache::default()),
            handoff: None,
            has_fetched: false,
        };
        assert_eq!(j.run().await.unwrap_err(), "could not fetch feed: unexpected status code: 503");
    }

    #[tokio::test]
    async fn test_refresh_job_run_e_process_order() {
        let now = Utc::now();
        for (order, want) in [("", vec!["a", "b", "c"]), ("OLDEST_FIRST", vec!["a", "b", "c"]), ("NEWEST_FIRST", vec!["c", "b", "a"])] {
            let (h, got) = handoff();
            let entries = vec![
                new_entry("c", Some(now)),
                new_entry("b", Some(now - Duration::minutes(1))),
                new_entry("a", Some(now - Duration::minutes(2))),
            ];
            let mut f = feed(0);
            f.settings = Some(FeedSettings { process_order: order.into(), ..FeedSettings::default() });
            let mut j = RefreshJob {
                feed: f,
                src: Box::new(Stub(Some(FetchResult { entries, ..FetchResult::default() }), None)),
                repo: Arc::new(RecRepo::default()),
                cache: Arc::new(RecCache::default()),
                handoff: Some(h),
                has_fetched: false,
            };
            j.run().await.unwrap();
            assert_eq!(got.lock().unwrap()[0], want, "{order}");
        }
    }

    /// One page per offset (autobrr's `pagedSource`).
    #[derive(Default)]
    struct Paged {
        limit: usize,
        total: usize,
        pages: BTreeMap<usize, Vec<Entry>>,
        fallback: Vec<Entry>,
        err_offsets: BTreeMap<usize, String>,
        offsets: Arc<Mutex<Vec<usize>>>,
    }
    impl Source for Paged {
        fn fetch(&mut self, offset: usize) -> BoxFuture<'_, Result<FetchResult, String>> {
            self.offsets.lock().unwrap().push(offset);
            let r = match self.err_offsets.get(&offset) {
                Some(e) => Err(e.clone()),
                None => {
                    let entries = self.pages.get(&offset).cloned().unwrap_or_else(|| self.fallback.clone());
                    Ok(FetchResult { items: entries.len(), entries, limit: self.limit, total: self.total, ..FetchResult::default() })
                }
            };
            Box::pin(async move { r })
        }
    }

    /// Keys written are seen by later lookups (autobrr's `statefulCacheRepo`).
    #[derive(Default)]
    struct Stateful {
        existing: Mutex<HashSet<String>>,
        put_err: Mutex<Option<String>>,
        put_calls: Mutex<Vec<Vec<String>>>,
        existing_err_on_call: usize,
        put_err_on_call: usize,
        existing_calls: Mutex<usize>,
        put_attempts: Mutex<usize>,
    }
    impl Stateful {
        fn with(keys: &[&str]) -> Stateful {
            Stateful { existing: Mutex::new(keys.iter().map(|s| s.to_string()).collect()), ..Stateful::default() }
        }
    }
    impl CacheRepo for Stateful {
        fn existing_items(&self, _: i64, keys: &[String]) -> Result<HashSet<String>, String> {
            let mut n = self.existing_calls.lock().unwrap();
            *n += 1;
            if self.existing_err_on_call == *n {
                return Err("existing items failed".into());
            }
            let have = self.existing.lock().unwrap();
            Ok(keys.iter().filter(|k| have.contains(*k)).cloned().collect())
        }
        fn put_many(&self, items: &[FeedCacheItem]) -> Result<(), String> {
            let mut n = self.put_attempts.lock().unwrap();
            *n += 1;
            if let Some(e) = self.put_err.lock().unwrap().clone() {
                return Err(e);
            }
            if self.put_err_on_call == *n {
                return Err("put many failed".into());
            }
            let mut have = self.existing.lock().unwrap();
            let keys: Vec<String> = items.iter().map(|i| i.key.clone()).collect();
            have.extend(keys.iter().cloned());
            self.put_calls.lock().unwrap().push(keys);
            Ok(())
        }
    }

    fn item(k: &str) -> Entry {
        new_entry(k, None)
    }

    fn keys(e: &[Entry]) -> Vec<String> {
        e.iter().map(|x| x.key.clone()).collect()
    }

    fn job(src: Paged, cache: Arc<Stateful>, max_pages: i64) -> RefreshJob {
        let f = Feed {
            id: 4,
            settings: Some(FeedSettings { max_pages, ..FeedSettings::default() }),
            last_run: Some(Utc::now() - Duration::hours(1)),
            ..Feed::default()
        };
        RefreshJob { feed: f, src: Box::new(src), repo: Arc::new(RecRepo::default()), cache, handoff: None, has_fetched: false }
    }

    fn two_full_pages() -> BTreeMap<usize, Vec<Entry>> {
        BTreeMap::from([
            (0, vec![item("new-1"), item("new-2"), item("new-3")]),
            (3, vec![item("new-4"), item("new-5"), item("new-6")]),
            (6, vec![item("new-7")]),
        ])
    }

    const MAX: i64 = super::super::MAX_FEED_PAGES;

    #[tokio::test]
    async fn test_refresh_job_fetch_new_pagination() {
        // stops at the cached boundary on page 2
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: BTreeMap::from([
                (0, vec![item("new-1"), item("new-2"), item("new-3")]),
                (3, vec![item("new-4"), item("old-1"), item("old-2")]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let cache = Arc::new(Stateful::with(&["old-1", "old-2"]));
        let got = job(src, cache.clone(), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-4", "new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);
        assert_eq!(*cache.put_calls.lock().unwrap(), vec![vec!["new-1", "new-2", "new-3"], vec!["new-4"]]);

        // fully cached first page stops immediately
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: BTreeMap::from([(0, vec![item("old-1"), item("old-2"), item("old-3")])]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let got = job(src, Arc::new(Stateful::with(&["old-1", "old-2", "old-3"])), MAX).fetch_new().await.unwrap();
        assert!(got.is_empty());
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // short page is the end of the feed
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: BTreeMap::from([(0, vec![item("new-1"), item("new-2")])]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // source without pagination fetches one page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { fallback: vec![item("new-1"), item("new-2")], offsets: offsets.clone(), ..Paged::default() };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // catch-up backfills until the short page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-7", "new-6", "new-5", "new-4", "new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3, 6]);

        // max pages caps the backfill
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let got = job(src, Arc::new(Stateful::default()), 2).fetch_new().await.unwrap();
        assert_eq!(got.len(), 6);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // unset max pages fetches only the first page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        job(src, Arc::new(Stateful::default()), 0).fetch_new().await.unwrap();
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // indexer ignoring the offset stops after a repeated page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src =
            Paged { limit: 3, fallback: vec![item("new-1"), item("new-2"), item("new-3")], offsets: offsets.clone(), ..Paged::default() };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // repeated page with shifted order stops on zero new items
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: BTreeMap::from([
                (0, vec![item("new-1"), item("new-2"), item("new-3")]),
                (3, vec![item("new-2"), item("new-1"), item("new-3")]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // reported total paginates past short pages
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            total: 7,
            pages: BTreeMap::from([
                (0, vec![item("new-1"), item("new-2")]),
                (2, vec![item("new-3"), item("new-4")]),
                (4, vec![item("new-5"), item("new-6")]),
                (6, vec![item("new-7")]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-7", "new-6", "new-5", "new-4", "new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 2, 4, 6]);

        // overlapping pages are deduped within the run
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: BTreeMap::from([(0, vec![item("new-1"), item("new-2"), item("new-3")]), (3, vec![item("new-3"), item("new-4")])]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-4", "new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // cache write failure drops the page and stops paginating
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let cache = Arc::new(Stateful::default());
        *cache.put_err.lock().unwrap() = Some("disk full".into());
        let got = job(src, cache.clone(), MAX).fetch_new().await.unwrap();
        assert!(got.is_empty());
        assert!(cache.put_calls.lock().unwrap().is_empty());
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // fetch error on page 2 keeps the cached first page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: two_full_pages(),
            err_offsets: BTreeMap::from([(3, "429 too many requests".to_string())]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let got = job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // cache lookup error on page 2 keeps the cached first page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let cache = Arc::new(Stateful { existing_err_on_call: 2, ..Stateful::default() });
        let got = job(src, cache, MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // cache write error on page 2 keeps the cached first page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let cache = Arc::new(Stateful { put_err_on_call: 2, ..Stateful::default() });
        let got = job(src, cache.clone(), MAX).fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(cache.put_calls.lock().unwrap().len(), 1);
        assert_eq!(*offsets.lock().unwrap(), [0, 3]);

        // fetch error on page 1 fails the run
        let src = Paged { limit: 3, err_offsets: BTreeMap::from([(0, "503".to_string())]), ..Paged::default() };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.last_run = None;
        assert!(j.fetch_new().await.is_err());
        assert!(!j.has_fetched, "a failed first fetch must retry the single page next run");

        // cache write failure on the first run keeps the one-page limit
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let cache = Arc::new(Stateful::default());
        *cache.put_err.lock().unwrap() = Some("disk full".into());
        let mut j = job(src, cache.clone(), MAX);
        j.feed.last_run = None;
        assert!(j.fetch_new().await.unwrap().is_empty());
        assert!(!j.has_fetched);
        *cache.put_err.lock().unwrap() = None;
        let got = j.fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 0]);
        assert!(j.has_fetched);

        // first run fetches only the first page, then the next run paginates
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.last_run = None;
        let got = j.fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["new-3", "new-2", "new-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0]);
        let next = Paged {
            limit: 3,
            pages: BTreeMap::from([
                (0, vec![item("next-1"), item("next-2"), item("next-3")]),
                (3, vec![item("next-4"), item("new-1"), item("new-2")]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        j.src = Box::new(next);
        let got = j.fetch_new().await.unwrap();
        assert_eq!(keys(&got), ["next-4", "next-3", "next-2", "next-1"]);
        assert_eq!(*offsets.lock().unwrap(), [0, 0, 3]);

        // last run older than the cache ttl fetches only the first page
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.last_run = Some(Utc::now() - Duration::days(40));
        j.fetch_new().await.unwrap();
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // last run within the cache ttl paginates
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged { limit: 3, pages: two_full_pages(), offsets: offsets.clone(), ..Paged::default() };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.last_run = Some(Utc::now() - Duration::days(3));
        j.fetch_new().await.unwrap();
        assert_eq!(*offsets.lock().unwrap(), [0, 3, 6]);

        // max age stops after a page reaching older items
        let now = Utc::now();
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 3,
            pages: BTreeMap::from([
                (
                    0,
                    vec![
                        new_entry("new-1", Some(now)),
                        new_entry("new-2", Some(now - Duration::hours(1))),
                        new_entry("new-3", Some(now - Duration::days(7))),
                    ],
                ),
                (3, vec![new_entry("new-4", Some(now - Duration::days(8)))]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.max_age = 3600 * 24;
        assert_eq!(j.fetch_new().await.unwrap().len(), 3);
        assert_eq!(*offsets.lock().unwrap(), [0]);

        // max age reached on page 2 stops after page 2
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let h = |n: i64| Some(now - Duration::hours(n));
        let src = Paged {
            limit: 2,
            pages: BTreeMap::from([
                (0, vec![new_entry("new-1", h(0)), new_entry("new-2", h(1))]),
                (2, vec![new_entry("new-3", h(2)), new_entry("new-4", h(48))]),
                (4, vec![new_entry("new-5", h(72)), new_entry("new-6", h(96))]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.max_age = 3600 * 24;
        j.fetch_new().await.unwrap();
        assert_eq!(*offsets.lock().unwrap(), [0, 2]);

        // max age disabled paginates past old items
        let old = Some(now - Duration::days(30));
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 2,
            pages: BTreeMap::from([(0, vec![new_entry("new-1", old), new_entry("new-2", old)]), (2, vec![new_entry("new-3", old)])]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        job(src, Arc::new(Stateful::default()), MAX).fetch_new().await.unwrap();
        assert_eq!(*offsets.lock().unwrap(), [0, 2]);

        // items without a valid pub date never reach max age
        let offsets = Arc::new(Mutex::new(Vec::new()));
        let src = Paged {
            limit: 2,
            pages: BTreeMap::from([
                (0, vec![new_entry("new-1", None), new_entry("new-2", Some(Utc.timestamp_opt(0, 0).unwrap()))]),
                (2, vec![new_entry("new-3", None)]),
            ]),
            offsets: offsets.clone(),
            ..Paged::default()
        };
        let mut j = job(src, Arc::new(Stateful::default()), MAX);
        j.feed.max_age = 3600;
        j.fetch_new().await.unwrap();
        assert_eq!(*offsets.lock().unwrap(), [0, 2]);
    }

    #[test]
    fn test_is_max_age() {
        let now = Utc::now();
        assert!(is_newer_than_max_age(3600, now - Duration::seconds(500), now));
        assert!(!is_newer_than_max_age(3600, now - Duration::seconds(5000), now));
    }
}
