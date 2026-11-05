// Ported from cross-seed v6.13.9 (src/jobs.ts, cleanupDB of src/db.ts and updateCaps of src/torznab.ts),
// Apache-2.0.
//! The jobs and their cadence, as cross-seed runs them: `rss` (every `rssCadence`), `search` (every
//! `searchCadence`), `updateIndexerCaps` (daily), `inject` (hourly, when the action is `inject`) and `cleanup`
//! (daily). Every minute [`CrossSeed::check_jobs`] starts each job whose time has come (its last run, kept in
//! `cross_seed_job_log`, plus its cadence), or that was asked to run ahead of schedule (`POST /api/job`, the
//! webhook). No job starts while RSS runs, and cleanup waits until nothing else runs. A job run ahead of schedule
//! by `POST /api/job` puts its next run a cadence later (`delayNextRun`).
//!
//! Lantern runs the jobs on schedule only while its `cross_seed` setting is on; a run asked for by the API always
//! happens.

use super::config::{ms_short, Override, RuntimeConfig};
use super::constants::{Action, MediaType};
use super::data::{forget_paths, save_ensemble};
use super::prefilter::human_readable_date;
use super::search::indexer_does_support_media_type;
use super::searchee::Searchee;
use super::{now_ms, CrossSeed};
use kit_server_core::{p, Db, RowExt};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// One hour in milliseconds.
const HOUR: f64 = 3_600_000.0;
/// One day in milliseconds.
const DAY: f64 = 24.0 * HOUR;

/// `JobName`: rss, search, updateIndexerCaps, inject, cleanup.
pub const RSS: &str = "rss";
/// The search job.
pub const SEARCH: &str = "search";
/// The job that reads each indexer's capabilities again.
pub const UPDATE_INDEXER_CAPS: &str = "updateIndexerCaps";
/// The job that adds saved torrents.
pub const INJECT: &str = "inject";
/// The job that tidies the tables.
pub const CLEANUP: &str = "cleanup";

/// `Job`: one job and its state.
#[derive(Debug, Clone)]
pub struct Job {
    /// Its name (one of the `JobName`s).
    pub name: &'static str,
    /// How often it runs, in milliseconds.
    pub cadence: f64,
    /// Whether it is running now.
    pub is_active: bool,
    /// `runAheadOfSchedule`: run at the next check, whatever its last run.
    pub run_ahead: bool,
    /// `delayNextRun`: after this run, the next waits a cadence more.
    pub delay_next_run: bool,
    /// `configOverride`: settings changed for the next run.
    pub config_override: Override,
}

impl Job {
    /// A job that isn't running and wasn't asked for.
    pub fn new(name: &'static str, cadence: f64) -> Job {
        Job { name, cadence, is_active: false, run_ahead: false, delay_next_run: false, config_override: Override::default() }
    }
}

/// `createJobs`: the jobs `cfg` asks for, in cross-seed's order. `updateIndexerCaps` is always there, as Lantern
/// always has its own indexers.
pub fn create_jobs(cfg: &RuntimeConfig) -> Vec<Job> {
    let mut jobs = Vec::new();
    if let Some(c) = cfg.rss_cadence.filter(|c| *c > 0.0) {
        jobs.push(Job::new(RSS, c));
    }
    if let Some(c) = cfg.search_cadence.filter(|c| *c > 0.0) {
        jobs.push(Job::new(SEARCH, c));
    }
    jobs.push(Job::new(UPDATE_INDEXER_CAPS, DAY));
    if cfg.action == Action::Inject {
        jobs.push(Job::new(INJECT, HOUR));
    }
    jobs.push(Job::new(CLEANUP, DAY));
    jobs
}

/// `logNextRun`'s words: when the job last ran and when it runs next.
pub fn next_run_words(name: &str, cadence: f64, last_run: Option<f64>, now: f64) -> String {
    let last = last_run.filter(|l| *l != 0.0);
    let eligible = last.map(|l| l + cadence).unwrap_or(now);
    let last_str = match last {
        None => "never".to_string(),
        Some(l) if now >= l => format!("{} ago", ms_short(now - l)),
        Some(l) => format!("at {}", human_readable_date(l - cadence)),
    };
    let next_str = if now >= eligible { "now".to_string() } else { format!("in {}", ms_short(eligible - now)) };
    format!("{name}: last run {last_str}, next run {next_str}")
}

/// Whether a job may start now (`checkJobs`' rules): not while RSS runs and, for cleanup, not while any job runs,
/// unless it was asked to run ahead; then when it was asked to, or its time has come.
pub fn is_due(job: &Job, all: &[Job], last_run: Option<f64>, now: f64) -> bool {
    if !job.run_ahead {
        if all.iter().any(|j| j.name == RSS && j.is_active) {
            return false;
        }
        if job.name == CLEANUP && all.iter().any(|j| j.is_active) {
            return false;
        }
    }
    let eligible = last_run.filter(|l| *l != 0.0).map(|l| l + job.cadence).unwrap_or(now);
    job.run_ahead || now >= eligible
}

impl CrossSeed {
    /// The jobs again from the settings (they can change while Lantern runs); a job kept keeps its state.
    pub fn refresh_jobs(&self) {
        let fresh = create_jobs(&self.config().0);
        if let Ok(mut jobs) = self.jobs.lock() {
            let mut next: Vec<Job> = Vec::new();
            for f in fresh {
                match jobs.iter().find(|j| j.name == f.name) {
                    Some(old) => next.push(Job { cadence: f.cadence, ..old.clone() }),
                    None => next.push(f),
                }
            }
            // a running job that is no longer asked for stays until it ends
            let gone: Vec<Job> = jobs.iter().filter(|j| j.is_active && !next.iter().any(|n| n.name == j.name)).cloned().collect();
            next.extend(gone);
            *jobs = next;
        }
    }

    /// One job's state now.
    pub fn job(&self, name: &str) -> Option<Job> {
        self.jobs.lock().ok().and_then(|j| j.iter().find(|j| j.name == name).cloned())
    }

    /// Changes job `name`'s state.
    pub fn update_job(&self, name: &str, f: impl FnOnce(&mut Job)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.iter_mut().find(|j| j.name == name) {
                f(j);
            }
        }
    }

    /// `checkJobs`: starts every job that is due. With `use_queue` it waits for a check already running;
    /// without, it gives way to it. The jobs run in the background.
    pub async fn check_jobs(&self, is_first_run: bool, use_queue: bool) {
        let _one = if use_queue {
            self.checking.lock().await
        } else {
            match self.checking.try_lock() {
                Ok(g) => g,
                Err(_) => return,
            }
        };
        self.refresh_jobs();
        let now = now_ms();
        let scheduled = self.enabled();
        let names: Vec<&'static str> = self.jobs.lock().map(|j| j.iter().map(|j| j.name).collect()).unwrap_or_default();
        for name in names {
            let last_run = self.job_last_run(name);
            let Some(job) = self.job(name) else { continue };
            if is_first_run {
                tracing::info!("cross-seed scheduler: {}", next_run_words(name, job.cadence, last_run, now));
            }
            let all: Vec<Job> = self.jobs.lock().map(|j| j.clone()).unwrap_or_default();
            if !is_due(&job, &all, last_run, now) || (!job.run_ahead && !scheduled) {
                continue;
            }
            let me = self.clone();
            tokio::spawn(async move {
                if !me.run_job(name).await {
                    return;
                }
                let Some(job) = me.job(name) else { return };
                let delay = job.delay_next_run;
                me.update_job(name, |j| j.delay_next_run = false);
                let last_run = if delay { now + job.cadence } else { now };
                let _ = me.db().execute(
                    "INSERT INTO cross_seed_job_log (name, last_run) VALUES (?,?) ON CONFLICT(name) DO UPDATE SET last_run=excluded.last_run",
                    p![name, last_run],
                );
                let cadence = if delay { job.cadence * 2.0 } else { job.cadence };
                tracing::info!("cross-seed scheduler: {}", next_run_words(name, cadence, Some(now), now_ms()));
            });
        }
    }

    /// `Job.run`: runs job `name` unless it is running already; whether it ran.
    pub async fn run_job(&self, name: &str) -> bool {
        let mut start = None;
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(j) = jobs.iter_mut().find(|j| j.name == name) {
                if !j.is_active {
                    j.is_active = true;
                    start = Some(j.clone());
                }
            }
        }
        let Some(job) = start else { return false };
        tracing::info!("cross-seed scheduler: starting job: {name}");
        let o = if job.run_ahead { job.config_override.clone() } else { Override::default() };
        let cfg = self.config().0;
        let failed = match name {
            RSS => self.scan_rss_feeds().await.err(),
            SEARCH => self.bulk_search(&o).await.err(),
            UPDATE_INDEXER_CAPS => {
                self.update_caps().await;
                None
            }
            INJECT => {
                self.inject_saved_torrents(&cfg).await;
                None
            }
            CLEANUP => {
                self.cleanup_db(&cfg).await;
                None
            }
            _ => None,
        };
        if let Some(e) = failed {
            tracing::error!("cross-seed scheduler: {name}: {}", e.detail);
        }
        self.update_job(name, |j| {
            j.is_active = false;
            j.run_ahead = false;
            j.config_override = Override::default();
        });
        true
    }

    /// `jobsLoop`: the first check (which logs each job's next run), then one every minute.
    pub async fn jobs_loop(&self) {
        self.check_jobs(true, false).await;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            self.check_jobs(false, false).await;
        }
    }

    /// `updateCaps`: every torrent indexer's capabilities read again, and the media types each suits logged.
    pub async fn update_caps(&self) {
        let list: Vec<crate::indexers::Indexer> = self
            .hub
            .indexers
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|ix| ix.enabled && ix.protocol() == crate::indexers::release::Protocol::Torrent)
            .collect();
        for ix in &list {
            if let Err(e) = self.hub.indexers.refresh_caps(ix).await {
                tracing::error!("cross-seed torznab: Indexer {} failed to fetch caps: {e}", ix.name);
            }
        }
        for a in self.active_indexers(0).await.unwrap_or_default() {
            let (mut yes, mut no) = (Vec::new(), Vec::new());
            for m in MediaType::ALL {
                let name = format!("{m:?}").to_uppercase();
                if indexer_does_support_media_type(m, &a.caps) {
                    yes.push(name);
                } else {
                    no.push(name);
                }
            }
            tracing::debug!(
                "cross-seed torznab: {} MediaTypes: Supported [{}] | Unsupported [{}]",
                a.ix.name,
                yes.join(", "),
                no.join(", ")
            );
        }
    }

    /// `cleanupDB`: every client torrent read again (and its episodes for seasons), then the `dataDirs` entries
    /// and season episodes whose files are gone are forgotten.
    pub async fn cleanup_db(&self, cfg: &RuntimeConfig) {
        if cfg.use_client_torrents {
            tracing::debug!("cross-seed cleanup: Refreshing all client torrents...");
            let clients = self.clients(cfg).await;
            let o = super::clients::SearcheeOptions {
                refresh: Some(Vec::new()),
                include_files: true,
                include_trackers: true,
                ..Default::default()
            };
            let mut all: Vec<Searchee> = Vec::new();
            for c in &clients {
                all.extend(c.get_client_searchees(self.db(), cfg, &o).await.searchees);
            }
            if cfg.season_from_episodes.is_some() {
                tracing::debug!("cross-seed cleanup: Refreshing all ensemble torrents...");
                let mut rows = Vec::new();
                for s in &all {
                    rows.extend(self.cache_ensemble_torrent_entry(s, None, &clients, cfg).await.unwrap_or_default());
                }
                save_ensemble(self.db(), &rows);
            }
        }
        let gone = |sql: &str| -> Vec<String> {
            let paths: HashSet<String> = self.db().query(sql, p![]).map(|r| r.iter().map(|r| r.s("path")).collect()).unwrap_or_default();
            let mut list: Vec<String> = paths.into_iter().filter(|p| !Path::new(p).exists()).collect();
            list.sort();
            list
        };
        if !cfg.data_dirs.is_empty() {
            tracing::debug!("cross-seed cleanup: Pruning deleted dataDirs entries...");
            forget_paths(self.db(), &gone("SELECT path FROM cross_seed_data"));
        }
        if cfg.season_from_episodes.is_some() {
            tracing::debug!("cross-seed cleanup: Pruning deleted ensemble entries...");
            forget_paths(self.db(), &gone("SELECT path FROM cross_seed_ensemble"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_as_cross_seed_makes_them() {
        let names = |c: &RuntimeConfig| create_jobs(c).iter().map(|j| j.name).collect::<Vec<_>>();
        let c = RuntimeConfig {
            rss_cadence: Some(30.0 * 60_000.0),
            search_cadence: Some(DAY),
            action: Action::Inject,
            ..RuntimeConfig::default()
        };
        assert_eq!(names(&c), [RSS, SEARCH, UPDATE_INDEXER_CAPS, INJECT, CLEANUP]);
        let c = RuntimeConfig { rss_cadence: None, search_cadence: None, action: Action::Save, ..RuntimeConfig::default() };
        assert_eq!(names(&c), [UPDATE_INDEXER_CAPS, CLEANUP]);
        assert_eq!(
            create_jobs(&RuntimeConfig { action: Action::Inject, ..RuntimeConfig::default() })
                .iter()
                .find(|j| j.name == INJECT)
                .unwrap()
                .cadence,
            HOUR
        );
    }

    #[test]
    fn when_a_job_is_due() {
        let now = 10.0 * DAY;
        let search = Job::new(SEARCH, DAY);
        let all = vec![Job::new(RSS, HOUR), search.clone(), Job::new(CLEANUP, DAY)];
        assert!(is_due(&search, &all, None, now), "never run: now");
        assert!(!is_due(&search, &all, Some(now - HOUR), now), "ran an hour ago, runs daily");
        assert!(is_due(&search, &all, Some(now - DAY), now));
        let ahead = Job { run_ahead: true, ..search.clone() };
        assert!(is_due(&ahead, &all, Some(now - HOUR), now), "asked to run ahead");
        let rss_running = vec![Job { is_active: true, ..Job::new(RSS, HOUR) }, search.clone()];
        assert!(!is_due(&search, &rss_running, None, now), "nothing starts while RSS runs");
        assert!(is_due(&ahead, &rss_running, None, now), "unless asked to");
        let busy = vec![Job { is_active: true, ..search.clone() }, Job::new(CLEANUP, DAY)];
        assert!(!is_due(&Job::new(CLEANUP, DAY), &busy, None, now), "cleanup waits for the others");
    }

    #[test]
    fn next_run_as_cross_seed_logs_it() {
        let now = 10.0 * DAY;
        assert_eq!(next_run_words("search", DAY, None, now), "search: last run never, next run now");
        assert_eq!(next_run_words("rss", 30.0 * 60_000.0, Some(now - 10.0 * 60_000.0), now), "rss: last run 10m ago, next run in 20m");
        assert_eq!(next_run_words("search", DAY, Some(now - 2.0 * DAY), now), "search: last run 2d ago, next run now");
    }
}
