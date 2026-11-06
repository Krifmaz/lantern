// Ported from cross-seed v6.13.9 (src/pipeline.ts, and getAndLogIndexers, makeRequests and rssPager of
// src/torznab.ts), Apache-2.0.
//! The searches: every searchee cross-seed has ([`CrossSeed::find_all_searchees`]: the client torrents or
//! `torrentDir`, and the data of `dataDirs`), the search job's pass over them ([`CrossSeed::bulk_search`]), one
//! searchee's search on the other indexers ([`CrossSeed::find_on_other_sites`]), the webhook's search by info
//! hash or path ([`CrossSeed::search_for_local_torrent_by_criteria`]), an announce or RSS candidate compared with
//! what is here ([`CrossSeed::check_new_candidate_match`]), and the RSS job ([`CrossSeed::scan_rss_feeds`]).

use super::arr::{arr_ids_equal, format_found_ids, scan_all_arrs_for_media, ExternalIds, ParsedMedia};
use super::clients::{by_client_host_priority, SearcheeOptions};
use super::config::{Override, RuntimeConfig};
use super::constants::{Decision, InjectionResult, MediaType};
use super::data::{find_potential_nested_roots, forget_paths};
use super::decide::Candidate;
use super::ensemble::{create_ensemble_searchees, get_newest_file_age, get_searchee_newest_file_age};
use super::notify::{send_results_notification, Notable};
use super::prefilter::{filter_by_content, filter_dupes_from_similar, filter_timestamps, ContentOptions};
use super::search::{caps_of, create_search_queries, get_search_string, indexer_does_support_media_type, rss_page_stop, Caps, Paged};
use super::searchee::{create_searchee_from_path, get_media_type, get_season_keys, File, Label, Searchee};
use super::utils::{format_as_list, strip_extension};
use super::{now_ms, CrossSeed, Outcome, Target};
use crate::indexers::release::{Protocol, Release};
use crate::indexers::Indexer;
use kit_server_core::{p, Db, Result, RowExt};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

/// `CachedSearch`: the last search, kept for the next searchee with the same search string.
#[derive(Debug, Clone, Default)]
pub struct CachedSearch {
    /// The search string (`None`: none kept).
    pub q: Option<String>,
    /// Each indexer's candidates from it.
    pub indexer_candidates: Vec<(i64, Vec<Release>)>,
    /// When the indexers were last searched (milliseconds since 1970).
    pub last_search: f64,
    /// The ids the search was made with.
    pub ids: Option<ExternalIds>,
}

/// One pass's searching state: how often each indexer was searched (for `searchLimit`) and the cached search.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// `indexerSearchCount`.
    pub indexer_search_count: HashMap<i64, i64>,
    /// `cachedSearch`.
    pub cached: CachedSearch,
}

/// A Lantern torrent indexer as cross-seed sees one: its capabilities and whether it may be searched now.
#[derive(Debug, Clone)]
pub struct Active {
    /// The indexer.
    pub ix: Indexer,
    /// What it can search.
    pub caps: Caps,
    /// Not on hold after failures (`getEnabledIndexers`).
    pub enabled: bool,
}

/// What [`CrossSeed::find_on_other_sites`] did, besides the outcome.
#[derive(Debug, Clone, Default)]
pub struct Found {
    /// The outcome (candidates, matches, injected, failed indexers).
    pub outcome: Outcome,
    /// Indexers asked now (not from the cache).
    pub searched_indexers: usize,
}

impl CrossSeed {
    /// `getActiveIndexers`: the enabled torrent indexers (but `except`), with their capabilities.
    pub async fn active_indexers(&self, except: i64) -> Result<Vec<Active>> {
        let now = crate::auth::now();
        let list: Vec<Indexer> = self
            .hub
            .indexers
            .list()?
            .into_iter()
            .filter(|ix| ix.enabled && ix.id != except && ix.protocol() == Protocol::Torrent)
            .collect();
        self.hub.indexers.refresh_caps_of(&list).await;
        Ok(list
            .into_iter()
            .map(|ix| {
                let caps = caps_of(&self.hub.indexers.capabilities(&ix));
                let enabled = !ix.status().is_disabled(now) && caps.search;
                Active { ix, caps, enabled }
            })
            .collect())
    }

    /// The search times of `title` per indexer: (first, last).
    fn searchee_timestamps(&self, title: &str) -> HashMap<i64, (f64, f64)> {
        self.db()
            .query("SELECT indexer_id, first_searched, last_searched FROM cross_seed_timestamps WHERE searchee=?", p![title])
            .map(|rows| rows.iter().map(|r| (r.i("indexer_id"), (r.f("first_searched"), r.f("last_searched")))).collect())
            .unwrap_or_default()
    }

    /// `filterTimestamps` over the enabled indexers that suit the searchee's media.
    pub fn filter_timestamps_for(&self, s: &Searchee, cfg: &RuntimeConfig, active: &[Active]) -> std::result::Result<(), String> {
        let media = get_media_type(s);
        let ids: Vec<i64> =
            active.iter().filter(|a| a.enabled && indexer_does_support_media_type(media, &a.caps)).map(|a| a.ix.id).collect();
        let t = self.timestamps(&s.title, &ids).map_err(|e| e.detail)?;
        filter_timestamps(s, cfg, t, now_ms(), || get_searchee_newest_file_age(s))
    }

    /// `getAndLogIndexers`: the indexers to search for `s` now (by their search times, its media, the cache and
    /// `searchLimit`), and Sonarr's or Radarr's ids for it; waits `delay` after the last search.
    async fn get_and_log_indexers(
        &self,
        s: &Searchee,
        state: &mut SearchState,
        media: MediaType,
        cfg: &RuntimeConfig,
        active: &[Active],
    ) -> (Vec<Active>, Option<ParsedMedia>) {
        let now = now_ms();
        let stamps = self.searchee_timestamps(&s.title);
        let skip_before = cfg.exclude_older.filter(|v| *v != 0.0).map(|v| now - v).unwrap_or(f64::NEG_INFINITY);
        let skip_after = cfg.exclude_recent_search.filter(|v| *v != 0.0).map(|v| now - v).unwrap_or(f64::INFINITY);
        let is_ensemble = cfg.season_from_episodes.is_some() && s.is_virtual();
        let newest = if is_ensemble { get_searchee_newest_file_age(s) } else { f64::INFINITY };
        let mut disabled: Vec<&Active> = Vec::new();
        let mut time_filtered: Vec<&Active> = Vec::new();
        for a in active {
            if !a.caps.search {
                continue;
            }
            let entry = stamps.get(&a.ix.id);
            // the search times decide first; an indexer they allow is then kept only when it is enabled
            let by_time = match entry {
                None => true,
                Some((_, last)) if is_ensemble && *last != 0.0 && *last < newest => true,
                Some((first, _)) if *first != 0.0 && *first < skip_before => false,
                Some((_, last)) if *last != 0.0 && *last > skip_after => false,
                Some(_) => true,
            };
            if !by_time {
                continue;
            }
            if !a.enabled {
                if indexer_does_support_media_type(media, &a.caps) {
                    disabled.push(a);
                }
                continue;
            }
            time_filtered.push(a);
        }
        let to_use: Vec<&Active> = time_filtered.iter().copied().filter(|a| indexer_does_support_media_type(media, &a.caps)).collect();
        let shown = format!("{} ({})", s.title, s.label.as_str());
        let mut should_scan_arr = true;
        let mut parsed: Option<ParsedMedia> = None;
        let search_str = get_search_string(s);
        if state.cached.q.as_deref() == Some(search_str.as_str()) {
            should_scan_arr = false;
            parsed = scan_all_arrs_for_media(cfg, &s.title, media).await;
            let ids = parsed.as_ref().and_then(|p| p.ids().cloned());
            if !arr_ids_equal(ids.as_ref(), state.cached.ids.as_ref()) {
                state.cached.indexer_candidates.clear();
                state.cached.ids = ids;
            }
        } else {
            state.cached.q = Some(search_str);
            state.cached.indexer_candidates.clear();
            state.cached.ids = None;
        }
        let mut limited: Vec<&Active> = Vec::new();
        let mut to_search: Vec<Active> = Vec::new();
        for a in to_use.iter().copied() {
            if state.cached.indexer_candidates.iter().any(|(id, _)| *id == a.ix.id) {
                continue;
            }
            if let Some(limit) = cfg.search_limit.filter(|l| *l != 0) {
                let n = state.indexer_search_count.entry(a.ix.id).or_insert(0);
                if *n >= limit {
                    limited.push(a);
                    continue;
                }
                *n += 1;
            }
            to_search.push(a.clone());
        }
        let names = |l: &[&Active]| l.iter().map(|a| a.ix.name.clone()).collect::<Vec<_>>().join(", ");
        if to_search.is_empty() && state.cached.indexer_candidates.is_empty() {
            state.cached.q = None;
            let enabled = active.iter().filter(|a| a.enabled).count();
            let mut causes: Vec<String> = Vec::new();
            if enabled > time_filtered.len() {
                causes.push("timestamps".into());
            }
            if time_filtered.len() > to_use.len() {
                causes.push("category".into());
            }
            if !disabled.is_empty() {
                causes.push("temporarily disabled indexers".into());
            }
            if !limited.is_empty() {
                causes.push("searchLimit".into());
            }
            let why = if causes.is_empty() {
                String::new()
            } else {
                format!(" (filtered by {})", format_as_list(&causes, true, "long", "conjunction"))
            };
            tracing::info!(
                "cross-seed: Skipped searching on indexers for {shown}{why} | MediaType: {} | IDs: N/A",
                media.as_str().to_uppercase()
            );
            if !limited.is_empty() {
                tracing::debug!("cross-seed: Skipping searching for {shown} due to search limit on [{}]", names(&limited));
            }
            if !disabled.is_empty() {
                tracing::debug!("cross-seed: Skipping searching for {shown} on temporarily disabled indexers [{}]", names(&disabled));
            }
            return (to_search, parsed);
        }
        if should_scan_arr {
            parsed = scan_all_arrs_for_media(cfg, &s.title, media).await;
            state.cached.ids = parsed.as_ref().and_then(|p| p.ids().cloned());
        }
        let ids = state.cached.ids.as_ref().map(format_found_ids).unwrap_or_else(|| "NONE".into());
        if !to_search.is_empty() {
            let wait_until = state.cached.last_search + cfg.delay * 1000.0;
            let now = now_ms();
            if now < wait_until {
                tokio::time::sleep(Duration::from_millis((wait_until - now) as u64)).await;
            }
            state.cached.last_search = now_ms();
        }
        tracing::info!("cross-seed: Searching for {shown} | MediaType: {} | IDs: {ids}", media.as_str().to_uppercase());
        (to_search, parsed)
    }

    /// `makeRequests`: every search of every indexer at once (each within `searchTimeout`): the results of each
    /// search that worked, and the indexers that failed with why.
    async fn make_requests(
        &self,
        indexers: &[Active],
        s: &Searchee,
        media: MediaType,
        parsed: Option<&ParsedMedia>,
        cfg: &RuntimeConfig,
    ) -> (Vec<(i64, Vec<Release>)>, Vec<(String, String)>) {
        let mut requests = Vec::new();
        for a in indexers {
            for q in create_search_queries(s, media, &a.caps, parsed) {
                requests.push((a, q.to_query(0, 0)));
            }
        }
        let timeout = cfg.search_timeout.map(|t| Duration::from_millis(t.max(0.0) as u64));
        let jobs = requests.iter().map(|(a, q)| async move {
            let search = self.hub.indexers.search_one(&a.ix, q, "Lantern cross-seed");
            let r = match timeout {
                Some(t) => tokio::time::timeout(t, search).await.unwrap_or_else(|_| Err("The operation was aborted due to timeout".into())),
                None => search.await,
            };
            (a, r)
        });
        let mut ok = Vec::new();
        let mut failed = Vec::new();
        for (a, r) in futures_util::future::join_all(jobs).await {
            match r {
                Ok(list) => ok.push((a.ix.id, list)),
                Err(e) => {
                    tracing::warn!("cross-seed: Failed to reach {}: {e}", a.ix.name);
                    failed.push((a.ix.name.clone(), e));
                }
            }
        }
        (ok, failed)
    }

    /// `findOnOtherSites`: searches the other indexers for `s` (or takes the cached search), decides on every
    /// candidate, acts on the matches, notes the search times and sends the notifications.
    pub async fn find_on_other_sites(
        &self,
        s: &Searchee,
        exclude: &mut HashSet<String>,
        state: &mut SearchState,
        target: &Target,
        cfg: &RuntimeConfig,
    ) -> Result<Found> {
        let active = self.active_indexers(target.indexer_id).await?;
        let media = get_media_type(s);
        let mut found = Found::default();
        let (to_search, parsed) = Box::pin(self.get_and_log_indexers(s, state, media, cfg, &active)).await;
        let (fresh, failed) = Box::pin(self.make_requests(&to_search, s, media, parsed.as_ref(), cfg)).await;
        found.outcome.failed = failed;
        found.searched_indexers = fresh.len();
        let mut response = std::mem::take(&mut state.cached.indexer_candidates);
        response.extend(fresh);
        state.cached.indexer_candidates = response.clone();
        let names: HashMap<i64, String> = active.iter().map(|a| (a.ix.id, a.ix.name.clone())).collect();
        let decided = self.decided(&s.title, target.grab_id)?;
        let mut rate_limited: HashSet<i64> = HashSet::new();
        let mut notables: Vec<Notable> = Vec::new();
        for (ix_id, list) in &response {
            for r in list {
                if r.protocol != Protocol::Torrent
                    || r.indexer_id == target.indexer_id
                    || decided.contains(&(r.indexer_id, r.title.clone()))
                {
                    continue;
                }
                let link = if r.link.is_empty() { r.magnet.clone() } else { r.link.clone() };
                let tracker = if r.indexer.is_empty() { names.get(ix_id).cloned().unwrap_or_default() } else { r.indexer.clone() };
                let candidate = Candidate {
                    name: r.title.clone(),
                    guid: r.guid.clone(),
                    link,
                    tracker: tracker.clone(),
                    size: r.size.and_then(|v| i64::try_from(v).ok()),
                    pub_date: r.date.map(|d| d.timestamp_millis() as f64),
                    indexer_id: Some(*ix_id),
                    cookie: None,
                };
                let (assessment, result, detail) = Box::pin(self.assess_and_act(s, candidate, exclude, cfg)).await;
                if assessment.decision == Decision::RateLimited {
                    rate_limited.insert(*ix_id);
                }
                if !matches!(
                    assessment.decision,
                    Decision::ReleaseGroupMismatch
                        | Decision::ResolutionMismatch
                        | Decision::SourceMismatch
                        | Decision::ProperRepackMismatch
                        | Decision::FuzzySizeMismatch
                        | Decision::NoDownloadLink
                ) {
                    found.outcome.candidates += 1;
                }
                if assessment.decision.is_any_match() {
                    found.outcome.matches += 1;
                }
                let hash = assessment.metafile.as_ref().map(|m| m.info_hash.clone()).unwrap_or_else(|| r.infohash.to_lowercase());
                if result == Some(InjectionResult::Success) {
                    found.outcome.injected += 1;
                    exclude.insert(hash.clone());
                }
                if let (Some(meta), Some(res)) = (&assessment.metafile, result) {
                    notables.push(Notable { decision: assessment.decision, meta: meta.clone(), tracker: tracker.clone(), result: res });
                }
                self.record(target, s, *ix_id, &tracker, &r.title, &hash, assessment.decision, result, &detail)?;
            }
        }
        let not_rate_limited: Vec<i64> =
            response.iter().map(|(id, _)| *id).filter(|id| !rate_limited.contains(id)).collect::<HashSet<_>>().into_iter().collect();
        self.update_search_timestamps(&s.title, &not_rate_limited)?;
        if !rate_limited.is_empty() {
            let shown: Vec<String> = rate_limited.iter().filter_map(|id| names.get(id).cloned()).collect();
            tracing::debug!("cross-seed: Snoozing indexers [{}] with RATE_LIMITED", shown.join(", "));
        }
        send_results_notification(s, &notables, cfg);
        Ok(found)
    }

    /// The searchee `s` searched by itself (the page, a grab or the webhook): the pre-filter, then
    /// [`find_on_other_sites`](Self::find_on_other_sites) with a state of its own.
    pub async fn search_searchee(&self, s: &Searchee, target: &Target, o: &Override, opts: ContentOptions) -> Result<Outcome> {
        let (base, _) = self.config();
        let cfg = base.with(o);
        if let Err(why) = filter_by_content(s, &cfg, opts) {
            tracing::info!("cross-seed: did not search for {}: {why}", s.title);
            return Ok(Outcome { filtered: Some(why), ..Outcome::default() });
        }
        let mut exclude = self.info_hashes_to_exclude_for(&cfg);
        exclude.extend(self.info_hashes_to_exclude());
        let mut state = SearchState::default();
        Ok(Box::pin(self.find_on_other_sites(s, &mut exclude, &mut state, target, &cfg)).await?.outcome)
    }

    /// `findAllSearchees`: the client torrents (`useClientTorrents`) or `torrentDir`'s, and the data of
    /// `dataDirs`, labelled `label`; only the `.torrent` files of `torrents` when it is given.
    pub async fn find_all_searchees(&self, label: Label, cfg: &RuntimeConfig) -> Vec<Searchee> {
        let _one = self.create_all.lock().await;
        let mut raw: Vec<Searchee> = Vec::new();
        if let Some(torrents) = &cfg.torrents {
            // `cross-seed search --torrents`: these files alone
            let clients = self.clients(cfg).await;
            let (infos, qbit) = self.torrent_infos(&clients).await;
            raw.extend(
                torrents.iter().filter_map(|t| super::index::create_searchee_from_torrent_file(t, &infos, qbit, !clients.is_empty()).ok()),
            );
            return raw.into_iter().map(|s| Searchee { label, ..s }).collect();
        }
        if cfg.use_client_torrents {
            let o = SearcheeOptions { include_files: true, include_trackers: true, ..SearcheeOptions::default() };
            for c in self.clients(cfg).await {
                raw.extend(c.get_client_searchees(self.db(), cfg, &o).await.searchees);
            }
        } else if let Some(dir) = &cfg.torrent_dir {
            raw.extend(self.load_torrent_dir_light(dir, cfg).await);
        }
        if !cfg.data_dirs.is_empty() {
            for path in super::data::find_searchees_from_all_data_dirs(cfg) {
                if let Ok(s) = create_searchee_from_path(&path) {
                    raw.push(s);
                }
            }
        }
        raw.into_iter().map(|s| Searchee { label, ..s }).collect()
    }

    /// `createEnsembleSearchees` with the clients' folders and priorities.
    pub async fn ensemble_searchees(&self, all: &[Searchee], use_filters: bool, cfg: &RuntimeConfig) -> Vec<Searchee> {
        if all.is_empty() || cfg.season_from_episodes.is_none() {
            return Vec::new();
        }
        let _one = self.create_all.lock().await;
        let clients = self.clients(cfg).await;
        let save_paths = if cfg.use_client_torrents {
            HashMap::new()
        } else {
            match clients.first() {
                Some(c) => {
                    let metas: Vec<_> = all.iter().filter(|s| s.info_hash.is_some()).map(super::clients::MetaRef::of_searchee).collect();
                    c.get_all_download_dirs(&metas, false, cfg).await
                }
                None => HashMap::new(),
            }
        };
        let prio = |h: Option<&str>| by_client_host_priority(&clients, h);
        create_ensemble_searchees(all, use_filters, cfg, &save_paths, &prio, now_ms())
    }

    /// `findSearchableTorrents`: the searchees to search now, grouped by search so one search serves a group,
    /// and the info hashes to leave out.
    pub async fn find_searchable_torrents(&self, cfg: &RuntimeConfig) -> Result<(Vec<Searchee>, HashSet<String>)> {
        tracing::info!("cross-seed: Gathering searchees...");
        let real = self.find_all_searchees(Label::Search, cfg).await;
        let ensembles = self.ensemble_searchees(&real, true, cfg).await;
        let exclude: HashSet<String> = real.iter().filter_map(|s| s.info_hash.clone()).collect();
        let block_only = ContentOptions { block_list_only: true, ..ContentOptions::default() };
        let valid: Vec<Searchee> = ensembles
            .iter()
            .filter(|s| filter_by_content(s, cfg, block_only).is_ok())
            .chain(real.iter().filter(|s| filter_by_content(s, cfg, ContentOptions::default()).is_ok()))
            .cloned()
            .collect();
        let mut grouping: indexmap::IndexMap<String, Vec<Searchee>> = indexmap::IndexMap::new();
        for s in valid {
            grouping.entry(get_search_string(&s)).or_default().push(s);
        }
        let active = self.active_indexers(0).await?;
        let clients = self.clients(cfg).await;
        let mut kept: indexmap::IndexMap<String, Vec<Searchee>> = indexmap::IndexMap::new();
        for (key, group) in grouping {
            let mut filtered = filter_dupes_from_similar(&group);
            if !filtered.iter().any(|s| self.filter_timestamps_for(s, cfg, &active).is_ok()) {
                continue;
            }
            filtered.sort_by_key(|s| {
                (by_client_host_priority(&clients, s.client_host.as_deref()), std::cmp::Reverse(s.files.len()), s.info_hash.is_none())
            });
            kept.insert(key, filtered);
        }
        let n = kept.len();
        let all: Vec<Searchee> = kept.into_values().flatten().collect();
        tracing::info!(
            "cross-seed: Found {} torrents, {} suitable to search for matches using {n} unique queries",
            real.len() + ensembles.len(),
            all.len()
        );
        if cfg.search_limit.is_some_and(|l| l != 0 && n as i64 > l) {
            tracing::info!("cross-seed: Limited to {} searches (unique queries) per indexer", cfg.search_limit.unwrap_or(0));
        }
        Ok((all, exclude))
    }

    /// `findMatchesBatch`: searches each searchee in turn until every indexer reached `searchLimit`; the matches
    /// found.
    pub async fn find_matches_batch(&self, searchees: &[Searchee], exclude: &mut HashSet<String>, cfg: &RuntimeConfig) -> usize {
        let mut state = SearchState::default();
        let mut total = 0;
        for (i, s) in searchees.iter().enumerate() {
            let prev = state.cached.last_search;
            let target = Target { indexer_id: 0, ..self.grab_of(s.info_hash.as_deref()).unwrap_or_default() };
            match Box::pin(self.find_on_other_sites(s, exclude, &mut state, &target, cfg)).await {
                Ok(f) => {
                    total += f.outcome.matches;
                    if let Some(limit) = cfg.search_limit.filter(|l| *l != 0) {
                        if !state.indexer_search_count.is_empty() && state.indexer_search_count.values().all(|n| *n >= limit) {
                            tracing::info!("cross-seed: Reached searchLimit of {limit} on all indexers");
                            break;
                        }
                    }
                    if f.searched_indexers == 0 {
                        state.cached.last_search = prev;
                    }
                }
                Err(e) => tracing::error!("cross-seed: ({}/{}) Error searching for {}: {}", i + 1, searchees.len(), s.title, e.detail),
            }
        }
        total
    }

    /// `bulkSearch`: the search job.
    pub async fn bulk_search(&self, o: &Override) -> Result<usize> {
        let cfg = self.config().0.with(o);
        let (searchees, mut exclude) = self.find_searchable_torrents(&cfg).await?;
        let total = self.find_matches_batch(&searchees, &mut exclude, &cfg).await;
        tracing::info!("cross-seed: Found {total} cross seeds from {} original torrents", searchees.len());
        Ok(total)
    }

    /// `searchForLocalTorrentByCriteria`: the client torrents with `info_hash`, or the data under `path`, searched
    /// on the other sites; the matches found, `None` when every searchee was left out by the pre-filter.
    pub async fn search_for_local_torrent_by_criteria(
        &self,
        info_hash: Option<&str>,
        path: Option<&str>,
        o: &Override,
        ignore_cross_seeds: bool,
    ) -> Result<Option<usize>> {
        let base = self.config().0;
        let cfg = base.with(o);
        let mut raw: Vec<Searchee> = match (info_hash, path) {
            (Some(h), _) => self.get_torrent_by_criteria(&h.to_lowercase(), &base).await.map_err(|e| kit_server_core::err(404, e))?,
            (None, Some(p)) => find_potential_nested_roots(Path::new(p), base.max_data_depth, None)
                .iter()
                .filter_map(|r| create_searchee_from_path(r).ok())
                .collect(),
            _ => Vec::new(),
        };
        let clients = self.clients(&base).await;
        raw.sort_by_key(|s| {
            (by_client_host_priority(&clients, s.client_host.as_deref()), std::cmp::Reverse(s.files.len()), s.info_hash.is_none())
        });
        let searchees: Vec<Searchee> = raw.into_iter().map(|s| Searchee { label: Label::Webhook, ..s }).collect();
        let mut per_host: HashMap<Option<String>, usize> = HashMap::new();
        for s in &searchees {
            *per_host.entry(s.client_host.clone()).or_default() += 1;
        }
        let allow = per_host.values().any(|v| *v == 1);
        let mut exclude = self.info_hashes_to_exclude_for(&base);
        let mut state = SearchState::default();
        let mut total = 0;
        let mut filtered = 0;
        for (i, s) in searchees.iter().enumerate() {
            let prev = state.cached.last_search;
            let opts = ContentOptions { allow_season_pack_episodes: allow, ignore_cross_seeds, block_list_only: false };
            if let Err(why) = filter_by_content(s, &cfg, opts) {
                tracing::info!("cross-seed: did not search for {}: {why}", s.title);
                filtered += 1;
                continue;
            }
            let target = self.grab_of(s.info_hash.as_deref())?;
            let target = Target { indexer_id: 0, ..target };
            match Box::pin(self.find_on_other_sites(s, &mut exclude, &mut state, &target, &cfg)).await {
                Ok(f) => {
                    total += f.outcome.matches;
                    if let Some(limit) = base.search_limit.filter(|l| *l != 0) {
                        if !state.indexer_search_count.is_empty() && state.indexer_search_count.values().all(|n| *n >= limit) {
                            tracing::info!("cross-seed: Reached searchLimit of {limit} on all indexers");
                            break;
                        }
                    }
                    if f.searched_indexers == 0 {
                        state.cached.last_search = prev;
                    }
                }
                Err(e) => tracing::error!("cross-seed: ({}/{}) Error searching for {}: {}", i + 1, searchees.len(), s.title, e.detail),
            }
        }
        Ok(if filtered == searchees.len() { None } else { Some(total) })
    }

    /// `getSearcheesForCandidate`: the searchees like the candidate by name that pass the pre-filter, one of each.
    async fn get_searchees_for_candidate(
        &self,
        candidate: &Candidate,
        label: Label,
        cfg: &RuntimeConfig,
    ) -> Option<(Vec<Searchee>, String)> {
        let sim = self.get_similar_by_name(&candidate.name, cfg).await;
        let method = if sim.keys.is_empty() { "fuzzy fallback".to_string() } else { format!("[{}]", sim.keys.join(",")) };
        if sim.client_searchees.is_empty() && sim.data_searchees.is_empty() {
            tracing::debug!("cross-seed: Did not find an existing entry using {method} for {} from {}", candidate.name, candidate.tracker);
            return None;
        }
        let list: Vec<Searchee> = sim
            .client_searchees
            .into_iter()
            .chain(sim.data_searchees)
            .map(|s| Searchee { label, ..s })
            .filter(|s| filter_by_content(s, cfg, ContentOptions::default()).is_ok())
            .collect();
        let list = filter_dupes_from_similar(&list);
        if list.is_empty() {
            tracing::debug!("cross-seed: No valid entries found using {method} for {} from {}", candidate.name, candidate.tracker);
            return None;
        }
        Some((list, method))
    }

    /// `getEnsembleForCandidate`: a season candidate's virtual season from the episode files kept for it.
    async fn get_ensemble_for_candidate(
        &self,
        candidate: &Candidate,
        label: Label,
        cfg: &RuntimeConfig,
    ) -> Option<(Vec<Searchee>, String)> {
        cfg.season_from_episodes?;
        let sk = get_season_keys(&strip_extension(&candidate.name))?;
        let season = sk.season.clone().unwrap_or_default();
        let keys: Vec<String> = sk.key_titles.iter().map(|k| format!("{k}.{season}")).collect();
        let mut rows = Vec::new();
        for k in &keys {
            if let Ok(found) =
                self.db().query("SELECT client_host, path, element FROM cross_seed_ensemble WHERE ensemble=? ORDER BY rowid", p![k.clone()])
            {
                rows.extend(found.iter().map(|r| (r.s("client_host"), r.s("path"), r.s("element"))));
            }
        }
        if rows.is_empty() {
            tracing::debug!(
                "cross-seed: Did not find an ensemble [{}] for {} from {}",
                sk.ensemble_titles.join(","),
                candidate.name,
                candidate.tracker
            );
            return None;
        }
        let mut seen = HashSet::new();
        let mut gone = Vec::new();
        let mut hosts: indexmap::IndexMap<String, usize> = indexmap::IndexMap::new();
        let mut files: Vec<(File, String)> = Vec::new();
        for (host, path, element) in rows {
            let Ok(m) = std::fs::metadata(&path) else {
                gone.push(path);
                continue;
            };
            let length = m.len() as i64;
            if !seen.insert(format!("{host}-{element}-{length}")) {
                continue;
            }
            if !host.is_empty() {
                *hosts.entry(host.clone()).or_default() += 1;
            }
            files.push((File { length, name: super::searchee::basename(&path), path }, element));
        }
        forget_paths(self.db(), &gone);
        if files.is_empty() {
            tracing::debug!(
                "cross-seed: Did not find any files for ensemble [{}] for {} from {}: sources may be incomplete or missing",
                sk.ensemble_titles.join(","),
                candidate.name,
                candidate.tracker
            );
            return None;
        }
        let mut elements: Vec<&String> = Vec::new();
        for (_, e) in &files {
            if !elements.contains(&e) {
                elements.push(e);
            }
        }
        let total: f64 = elements
            .iter()
            .map(|e| {
                let l: Vec<i64> = files.iter().filter(|(_, x)| x == *e).map(|(f, _)| f.length).collect();
                l.iter().sum::<i64>() as f64 / l.len() as f64
            })
            .sum();
        let length = (total + 0.5).floor() as i64;
        let files: Vec<File> = files.into_iter().map(|(f, _)| f).collect();
        let mtime = get_newest_file_age(&files.iter().map(|f| f.path.clone()).collect::<Vec<_>>());
        let clients = self.clients(cfg).await;
        let mut hosts: Vec<(String, usize)> = hosts.into_iter().collect();
        hosts.sort_by_key(|(h, n)| (std::cmp::Reverse(*n), by_client_host_priority(&clients, Some(h))));
        let client_host = hosts.first().map(|(h, _)| h.clone());
        let list: Vec<Searchee> = sk
            .ensemble_titles
            .iter()
            .map(|t| Searchee {
                name: t.clone(),
                title: t.clone(),
                files: files.clone(),
                length,
                mtime_ms: Some(mtime),
                client_host: client_host.clone(),
                label,
                ..Searchee::default()
            })
            .collect();
        Some((list, "ensemble".into()))
    }

    /// `checkNewCandidateMatch` (an announce or an RSS candidate): the searchees like it, decided on one at a time
    /// until one matches and is added: (decision, action result).
    pub async fn check_new_candidate_match(
        &self,
        candidate: Candidate,
        label: Label,
    ) -> Result<(Option<Decision>, Option<InjectionResult>)> {
        let (cfg, _) = self.config();
        let mut searchees: Vec<Searchee> = Vec::new();
        {
            let _one = self.candidates.lock().await;
            if let Some((list, _)) = self.get_searchees_for_candidate(&candidate, label, &cfg).await {
                searchees.extend(list);
            }
            if let Some((list, _)) = self.get_ensemble_for_candidate(&candidate, label, &cfg).await {
                searchees.extend(list);
            }
        }
        if searchees.is_empty() {
            return Ok((None, None));
        }
        let clients = self.clients(&cfg).await;
        searchees.sort_by_key(|s| {
            (by_client_host_priority(&clients, s.client_host.as_deref()), s.info_hash.is_none(), std::cmp::Reverse(s.files.len()))
        });
        let exclude = self.info_hashes_to_exclude_for(&cfg);
        let mut decision = None;
        let mut action_result: Option<InjectionResult> = None;
        let mut matched: Option<(Searchee, Notable)> = None;
        for s in &searchees {
            let (a, result, detail) = Box::pin(self.assess_and_act(s, candidate.clone(), &exclude, &cfg)).await;
            let hash = a.metafile.as_ref().map(|m| m.info_hash.clone()).unwrap_or_default();
            let target = Target { indexer_id: 0, ..self.grab_of(s.info_hash.as_deref())? };
            self.record(
                &target,
                s,
                candidate.indexer_id.unwrap_or(0),
                &candidate.tracker,
                &candidate.name,
                &hash,
                a.decision,
                result,
                &detail,
            )?;
            if a.decision == Decision::InfoHashAlreadyExists {
                decision = Some(a.decision);
                break;
            }
            if !a.decision.is_any_match() {
                continue;
            }
            let r = result.unwrap_or(InjectionResult::Failure);
            let notable =
                a.metafile.clone().map(|meta| Notable { decision: a.decision, meta, tracker: candidate.tracker.clone(), result: r });
            if r == InjectionResult::Saved {
                decision = Some(a.decision);
                action_result = Some(r);
                matched = notable.map(|n| (s.clone(), n));
                break;
            }
            if a.decision != Decision::MatchPartial && matches!(r, InjectionResult::Success | InjectionResult::AlreadyExists) {
                decision = Some(a.decision);
                action_result = Some(InjectionResult::Success);
                matched = notable.map(|n| (s.clone(), Notable { result: InjectionResult::Success, ..n }));
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
            matched = notable.map(|n| (s.clone(), n));
        }
        if let Some((s, n)) = matched {
            send_results_notification(&s, &[n], &cfg);
        }
        if action_result == Some(InjectionResult::Success) {
            let me = self.clone();
            let c = cfg.clone();
            tokio::spawn(async move { me.index_torrents_and_data_dirs(&c, false).await });
        }
        Ok((decision, action_result))
    }

    /// `rssPager` for one indexer: its newest releases, page by page (up to 10 pages or 10000 candidates), until
    /// the guid seen last time or the time of the last run; the guid seen first is kept for the next run.
    pub async fn rss_pager(&self, a: &Active, time_since_last_run: f64) -> Vec<Candidate> {
        let limit = a.caps.limits_max;
        let last_seen: Option<String> = self
            .db()
            .one("SELECT last_seen_guid FROM cross_seed_rss WHERE indexer_id=?", p![a.ix.id])
            .ok()
            .flatten()
            .map(|r| r.s("last_seen_guid"));
        let mut new_last_seen = last_seen.clone();
        let mut page_back_until = 0.0;
        let mut out: Vec<Candidate> = Vec::new();
        let max_page = 10;
        let max_candidates = 10_000;
        let cfg = self.config().0;
        let timeout = cfg.search_timeout.map(|t| Duration::from_millis(t.max(0.0) as u64));
        for i in 0..max_page {
            let q = crate::indexers::release::Query { limit, offset: i * limit, rss: true, ..Default::default() };
            let search = self.hub.indexers.search_one(&a.ix, &q, "Lantern cross-seed");
            let r = match timeout {
                Some(t) => tokio::time::timeout(t, search).await.unwrap_or_else(|_| Err("The operation was aborted due to timeout".into())),
                None => search.await,
            };
            let mut page: Vec<Release> = match r {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("cross-seed: Paging {} stopped at page {}: {e}", a.ix.name, i + 1);
                    break;
                }
            };
            page.sort_by_key(|x| std::cmp::Reverse(x.date));
            if page.is_empty() {
                if i == 0 {
                    tracing::error!("cross-seed: Paging {} stopped at page {}: no results returned", a.ix.name, i + 1);
                } else {
                    tracing::debug!("cross-seed: Paging {} stopped at page {}: no results returned", a.ix.name, i + 1);
                }
                break;
            }
            let date = |r: &Release| r.date.map(|d| d.timestamp_millis() as f64).unwrap_or(f64::NAN);
            if i == 0 {
                new_last_seen = Some(page[0].guid.clone());
                page_back_until = date(&page[0]) - time_since_last_run;
            }
            let paged: Vec<Paged> = page.iter().map(|r| Paged { guid: r.guid.clone(), pub_date: date(r) }).collect();
            let (n, more) = rss_page_stop(&paged, last_seen.as_deref(), page_back_until);
            if n == 0 {
                tracing::debug!("cross-seed: Paging {} stopped at page {}: no new candidates", a.ix.name, i + 1);
                break;
            }
            let found = paged.iter().position(|p| Some(p.guid.as_str()) == last_seen.as_deref());
            let fresh: Vec<&Release> = match found {
                Some(at) => page[..at].iter().collect(),
                None => page.iter().filter(|r| date(r) >= page_back_until).collect(),
            };
            for r in fresh {
                out.push(Candidate {
                    name: r.title.clone(),
                    guid: r.guid.clone(),
                    link: if r.link.is_empty() { r.magnet.clone() } else { r.link.clone() },
                    tracker: if r.indexer.is_empty() { a.ix.name.clone() } else { r.indexer.clone() },
                    size: r.size.and_then(|v| i64::try_from(v).ok()),
                    pub_date: r.date.map(|d| d.timestamp_millis() as f64),
                    indexer_id: Some(a.ix.id),
                    cookie: None,
                });
            }
            if !more {
                tracing::debug!("cross-seed: Paging {} stopped at page {}: reached last seen guid or pageBackUntil", a.ix.name, i + 1);
                break;
            }
            if out.len() >= max_candidates {
                tracing::debug!("cross-seed: Paging {} stopped at page {}: reached max candidates {max_candidates}", a.ix.name, i + 1);
                break;
            }
        }
        let _ = self.db().execute(
            "INSERT INTO cross_seed_rss (indexer_id, last_seen_guid) VALUES (?,?) ON CONFLICT(indexer_id) DO UPDATE SET last_seen_guid=excluded.last_seen_guid",
            p![a.ix.id, new_last_seen],
        );
        out
    }

    /// `scanRssFeeds`: the RSS job: every indexer's new releases since the last run, each compared with what is
    /// here.
    pub async fn scan_rss_feeds(&self) -> Result<()> {
        let cfg = self.config().0;
        if !cfg.use_client_torrents && cfg.torrent_dir.is_none() && cfg.data_dirs.is_empty() {
            tracing::error!("cross-seed: RSS requires torznab and at least one of useClientTorrents, torrentDir, or dataDirs to be set");
            return Ok(());
        }
        let active: Vec<Active> = self.active_indexers(0).await?.into_iter().filter(|a| a.enabled).collect();
        if active.is_empty() {
            tracing::error!("cross-seed: RSS requires torznab and at least one of useClientTorrents, torrentDir, or dataDirs to be set");
            return Ok(());
        }
        tracing::debug!("cross-seed: Querying RSS feeds...");
        let last_run = self.job_last_run("rss").unwrap_or(0.0);
        self.index_torrents_and_data_dirs(&cfg, false).await;
        let since = now_ms() - last_run;
        let pages = futures_util::future::join_all(active.iter().map(|a| self.rss_pager(a, since))).await;
        let mut n = 0;
        for candidates in pages {
            for c in candidates {
                if let Err(e) = self.check_new_candidate_match(c, Label::Rss).await {
                    tracing::error!("cross-seed: {}", e.detail);
                }
                n += 1;
            }
        }
        tracing::info!(
            "cross-seed: RSS scan complete - checked {n} new candidates since {}",
            super::prefilter::human_readable_date(last_run)
        );
        Ok(())
    }

    /// `getJobLastRun`.
    pub fn job_last_run(&self, name: &str) -> Option<f64> {
        self.db().one("SELECT last_run FROM cross_seed_job_log WHERE name=?", p![name]).ok().flatten().map(|r| r.f("last_run"))
    }
}
