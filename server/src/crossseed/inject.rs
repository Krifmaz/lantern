// Ported from cross-seed v6.13.9 (src/inject.ts), Apache-2.0.
//! The inject job (`injectSavedTorrents`): every `.torrent` file waiting in `outputDir` (or `injectDir`) is
//! compared with every searchee again; one that matches is added to a torrent client, and its file is deleted
//! once the torrent is complete. One the client already has is checked again, and one whose source isn't finished
//! yet waits for the next run.

use super::action::perform_action_without_mutex;
use super::clients::{by_client_host_priority, wait_for_torrent_to_complete, Client, ResumeTiming};
use super::config::RuntimeConfig;
use super::constants::{Decision, InjectionResult, VIDEO_DISC_EXTENSIONS};
use super::decide::{assess_candidate, MetaOrCandidate, SnatchError};
use super::index::{find_all_torrent_files_in_dir, parse_metadata_from_filename};
use super::notify::{send_results_notification, Notable};
use super::prefilter::{find_blocked_string_in_release_maybe, human_readable_date};
use super::searchee::{create_searchee_from_metafile, Label, Searchee};
use super::torrent::Metafile;
use super::utils::{are_media_titles_similar, format_as_list, has_ext, sanitize_info_hash};
use super::CrossSeed;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// `UNKNOWN_TRACKER`: the tracker of a saved torrent whose file name and trackers don't say.
pub const UNKNOWN_TRACKER: &str = "UnknownTracker";

/// One day in milliseconds.
const DAY_MS: f64 = 86_400_000.0;

/// `InjectSummary`: what one run did, counted.
#[derive(Debug, Default)]
pub struct InjectSummary {
    /// Torrent files found.
    pub total: usize,
    /// Added to a client.
    pub injected: usize,
    /// Of them, full matches.
    pub full_matches: usize,
    /// Of them, partial matches.
    pub partial_matches: usize,
    /// Left out by the block list.
    pub blocked: usize,
    /// Already in a client.
    pub already_exists: usize,
    /// Of those, not complete there.
    pub incomplete_candidates: usize,
    /// Whose source isn't finished.
    pub incomplete_searchees: usize,
    /// That failed to be added.
    pub failed: usize,
    /// That matched nothing.
    pub unmatched: usize,
    /// A file name without cross-seed's `[type][tracker]name` form was found.
    pub found_bad_format: bool,
    /// The work left running after a torrent was handled (`PROMISES`).
    pub tasks: Vec<JoinHandle<()>>,
}

/// One searchee that matches a saved torrent, and what adding from it did.
#[derive(Debug, Clone)]
pub struct InjectMatch {
    /// The searchee.
    pub searchee: Searchee,
    /// How it matches.
    pub decision: Decision,
    /// What adding from it did, once tried.
    pub action_result: Option<InjectionResult>,
}

/// `getTorrentFilePathLog`: the path, its info hash shortened.
fn file_path_log(path: &str) -> String {
    let base = super::searchee::basename(path);
    match parse_metadata_from_filename(&base).and_then(|m| m.info_hash) {
        Some(h) => path.replace(&h, &sanitize_info_hash(&h)),
        None => path.to_string(),
    }
}

/// `deleteTorrentFileIfSafe`: deletes the file only when its name says cross-seed saved it.
pub fn delete_torrent_file_if_safe(path: &str) {
    let shown = file_path_log(path);
    let safe = parse_metadata_from_filename(&super::searchee::basename(path))
        .is_some_and(|m| !m.tracker.is_empty() && !m.name.is_empty() && !m.media_type.is_empty());
    if !safe {
        tracing::warn!("cross-seed inject: Will not delete {shown}: missing metadata from filename");
        return;
    }
    tracing::debug!("cross-seed inject: Deleting {shown}");
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::error!("cross-seed inject: Failed to delete {shown}: {e}");
        }
    }
}

/// `deleteTorrentFileIfComplete`, in the background: deletes the file once the client has the whole torrent.
fn delete_torrent_file_if_complete(path: String, client: Arc<Client>, info_hash: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        if wait_for_torrent_to_complete(&client, &info_hash, 6, Duration::from_secs(1)).await {
            delete_torrent_file_if_safe(&path);
        } else {
            tracing::warn!("cross-seed inject: Will not delete {}: torrent is incomplete", file_path_log(&path));
        }
    })
}

/// `loadMetafile`: the torrent and its tracker (from its file name, else from the indexers' hosts, else
/// [`UNKNOWN_TRACKER`]).
fn load_metafile(path: &str, progress: &str, summary: &mut InjectSummary, host_to_name: &[(String, String)]) -> Option<(Metafile, String)> {
    let meta = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| Metafile::decode(&b)) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("cross-seed inject: {progress} Failed to parse {}: {e}", file_path_log(path));
            return None;
        }
    };
    if let Some(m) = parse_metadata_from_filename(&super::searchee::basename(path)) {
        return Some((meta, m.tracker));
    }
    summary.found_bad_format = true;
    let tracker = meta
        .trackers
        .iter()
        .find_map(|h| host_to_name.iter().find(|(host, _)| host == h).map(|(_, n)| n.clone()))
        .unwrap_or_else(|| UNKNOWN_TRACKER.to_string());
    Some((meta, tracker))
}

/// `logInjectSummary`.
fn log_summary(s: &InjectSummary, flat_linking: bool) {
    let incomplete = format!(
        "{} existed in client{}",
        s.already_exists,
        if s.incomplete_candidates > 0 { format!(" ({} were incomplete)", s.incomplete_candidates) } else { String::new() }
    );
    let mut parts = vec![format!("Injected {}/{} torrents", s.injected, s.total)];
    let mut add = |n: usize, text: String| {
        if n > 0 {
            parts.push(text);
        }
    };
    add(s.full_matches, format!("{} were full matches", s.full_matches));
    add(s.partial_matches, format!("{} were partial matches", s.partial_matches));
    add(s.incomplete_searchees, format!("{} had incomplete sources", s.incomplete_searchees));
    add(s.already_exists, incomplete);
    add(s.blocked, format!("{} were possibly blocklisted", s.blocked));
    add(s.failed, format!("{} failed to inject", s.failed));
    add(s.unmatched, format!("{} had no matches", s.unmatched));
    tracing::info!("cross-seed inject: {}", format_as_list(&parts, false, "long", "unit"));
    if s.unmatched > 0 {
        tracing::info!("cross-seed inject: Use \"cross-seed diff\" to get the reasons two torrents are not considered matches");
    }
    if s.found_bad_format && !flat_linking {
        tracing::warn!(
            "cross-seed inject: Some torrents could be linked to linkDir/{UNKNOWN_TRACKER} - follow .torrent naming format in the docs to avoid this"
        );
    }
}

/// The order matches are tried in: client priority, then full, size-only and partial matches, then torrents,
/// then data, then seasons made of episodes, then more files first.
pub fn sort_matches(matches: &mut [InjectMatch], priority: &dyn Fn(Option<&str>) -> usize) {
    let rank = |d: Decision| -> i32 {
        match d {
            Decision::Match => -1,
            Decision::MatchSizeOnly => 0,
            _ => 1,
        }
    };
    matches.sort_by_key(|m| {
        (
            priority(m.searchee.client_host.as_deref()),
            rank(m.decision),
            m.searchee.info_hash.is_none(),
            m.searchee.path.is_none(),
            std::cmp::Reverse(m.searchee.files.len()),
        )
    });
}

/// `isSimilar` of `whichSearcheesMatchTorrent`: the titles or names are alike.
pub fn is_similar(s: &Searchee, meta: &Metafile) -> bool {
    are_media_titles_similar(&s.title, &meta.title)
        || are_media_titles_similar(&s.title, &meta.name)
        || are_media_titles_similar(&s.name, &meta.name)
        || are_media_titles_similar(&s.name, &meta.title)
}

impl CrossSeed {
    /// `getHostToNameMap`: each tracker host Lantern knows, with its indexer's name. Lantern doesn't keep the
    /// tracker hosts of each indexer's torrents, so the hosts of the indexers' site addresses stand in.
    pub(super) fn host_to_name(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for ix in self.hub.indexers.list().unwrap_or_default() {
            if let Some(host) = url::Url::parse(&ix.base_url).ok().and_then(|u| u.host_str().map(|h| h.to_string())) {
                if !out.iter().any(|(h, _)| *h == host) {
                    out.push((host, ix.name.clone()));
                }
            }
        }
        out
    }

    /// `whichSearcheesMatchTorrent`: the searchees the torrent matches, in the order to try them; whether one was
    /// left out by the block list; whether one was left out for its title.
    async fn which_searchees_match_torrent(
        &self,
        meta: &Metafile,
        searchees: &[Searchee],
        cfg: &RuntimeConfig,
        ignore_titles: bool,
    ) -> (Vec<InjectMatch>, bool, bool) {
        let clients = self.clients(cfg).await;
        let mut is_complete: Option<bool> = None;
        let mut found_blocked = false;
        let mut fuzzy_fail = false;
        let mut matches = Vec::new();
        let none = HashSet::new();
        for s in searchees {
            let mut input = MetaOrCandidate::Meta(meta.clone());
            let a = assess_candidate(&mut input, s, &none, &[], cfg, |_| async { Err::<Metafile, _>(SnatchError::MagnetLink) }).await;
            if !a.decision.is_any_match() {
                continue;
            }
            if let Some(note) = find_blocked_string_in_release_maybe(s, &cfg.block_list) {
                tracing::info!("cross-seed inject: did not search for {}: it matches the blocklist: {note}", s.title);
                found_blocked = true;
                continue;
            }
            if !is_similar(s, meta) {
                if ignore_titles {
                    tracing::warn!("cross-seed inject: Ignoring title mismatch for {} with {}", meta.name, s.name);
                } else {
                    if is_complete.is_none() {
                        let mut any = false;
                        for c in &clients {
                            if c.is_torrent_complete(&meta.info_hash).await.unwrap_or(false) {
                                any = true;
                                break;
                            }
                        }
                        is_complete = Some(any);
                    }
                    if is_complete != Some(true) {
                        tracing::warn!(
                            "cross-seed inject: Skipping match for {} with {} due to title mismatch (use \"cross-seed inject --ignore-titles\" if this is an erroneous rejection)",
                            meta.name,
                            s.name
                        );
                        fuzzy_fail = true;
                        continue;
                    }
                }
            }
            matches.push(InjectMatch { searchee: s.clone(), decision: a.decision, action_result: None });
        }
        let prio = |h: Option<&str>| by_client_host_priority(&clients, h);
        sort_matches(&mut matches, &prio);
        (matches, found_blocked, fuzzy_fail)
    }

    /// `injectSavedTorrent`: one saved torrent, compared, added and tidied up.
    async fn inject_saved_torrent(
        &self,
        progress: &str,
        path: &str,
        summary: &mut InjectSummary,
        searchees: &mut Vec<Searchee>,
        cfg: &RuntimeConfig,
        host_to_name: &[(String, String)],
    ) {
        let Some((meta, tracker)) = load_metafile(path, progress, summary, host_to_name) else { return };
        let shown = file_path_log(path);
        if let Some(blocked) = find_blocked_string_in_release_maybe(&Searchee::from_metafile_raw(&meta), &cfg.block_list) {
            tracing::warn!("cross-seed inject: {progress} {shown} is in the blockList: {blocked}");
            summary.blocked += 1;
            return;
        }
        let ignore_titles = cfg.ignore_titles.unwrap_or(false);
        let (matches, found_blocked, fuzzy_fail) = self.which_searchees_match_torrent(&meta, searchees, cfg, ignore_titles).await;
        if matches.is_empty() {
            if found_blocked {
                tracing::warn!("cross-seed inject: {progress} {shown} has all matches in the blockList");
                summary.blocked += 1;
            } else {
                tracing::error!("cross-seed inject: {progress} {shown} has no matches");
                summary.unmatched += 1;
            }
            if fuzzy_fail {
                tracing::warn!("cross-seed inject: Will not delete {shown}: is has no matches due to title mismatch");
            } else {
                delete_torrent_file_if_safe(path);
            }
            return;
        }
        let ctx = self.action_ctx(cfg).await;
        // injectInitialAction
        let mut client: Option<Arc<Client>> = None;
        let mut client_matches: Vec<InjectMatch> = Vec::new();
        let mut result = InjectionResult::Failure;
        let mut matched: Option<(Searchee, Decision)> = None;
        let mut destination_dir: Option<String> = None;
        let mut linked_new_files = false;
        for m in &matches {
            let res = perform_action_without_mutex(&ctx, &meta, m.decision, &m.searchee, &tracker, client.as_ref(), true).await;
            client_matches.push(InjectMatch { action_result: Some(res.action_result), ..m.clone() });
            if res.action_result == InjectionResult::Failure {
                if res.linked_new_files {
                    break;
                }
                continue;
            }
            let Some(rc) = res.client.clone() else { continue };
            if res.action_result == InjectionResult::Saved {
                continue;
            }
            if res.linked_new_files {
                linked_new_files = true;
            }
            if result == InjectionResult::Success {
                continue;
            }
            if res.action_result == InjectionResult::AlreadyExists {
                client = Some(rc);
                destination_dir = res.destination_dir.clone();
                result = res.action_result;
                continue;
            }
            if res.action_result == InjectionResult::TorrentNotComplete {
                if result == InjectionResult::AlreadyExists {
                    continue;
                }
                client = Some(rc);
                result = res.action_result;
                continue;
            }
            client = Some(rc);
            destination_dir = res.destination_dir.clone();
            result = InjectionResult::Success;
            matched = Some((m.searchee.clone(), m.decision));
        }
        match result {
            InjectionResult::Success => {
                let Some(client) = client else { return };
                tracing::info!("cross-seed inject: {progress} Injected {shown} - {}", result.as_str());
                if let Some((s, decision)) = &matched {
                    send_results_notification(
                        s,
                        &[Notable { decision: *decision, meta: meta.clone(), tracker: tracker.clone(), result }],
                        cfg,
                    );
                    summary.injected += 1;
                    if *decision == Decision::MatchPartial {
                        summary.partial_matches += 1;
                    } else {
                        summary.full_matches += 1;
                    }
                }
                summary.tasks.push(delete_torrent_file_if_complete(path.to_string(), client.clone(), meta.info_hash.clone()));
                if let Ok(s) = create_searchee_from_metafile(&meta) {
                    searchees.push(if cfg.use_client_torrents {
                        Searchee {
                            label: Label::Inject,
                            client_host: Some(client.info.client_host.clone()),
                            save_path: destination_dir,
                            ..s
                        }
                    } else {
                        Searchee { label: Label::Inject, ..s }
                    });
                }
            }
            InjectionResult::Failure => {
                tracing::error!("cross-seed inject: {progress} Failed to inject {shown} - {}", result.as_str());
                summary.failed += 1;
            }
            InjectionResult::AlreadyExists => {
                let Some(client) = client else { return };
                self.injection_already_exists(progress, path, &shown, &client, &client_matches, &meta, linked_new_files, summary, cfg)
                    .await;
            }
            InjectionResult::TorrentNotComplete => {
                let old = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as f64)
                    .unwrap_or(f64::INFINITY);
                if cfg.link_dirs.is_empty() || old >= super::now_ms() - DAY_MS {
                    tracing::warn!("cross-seed inject: {progress} Unable to inject {shown} - {}", result.as_str());
                } else if let Some(client) = client {
                    self.inject_from_stalled_torrent(progress, &shown, &client, &client_matches, &meta, &tracker, result, &ctx).await;
                }
                summary.incomplete_searchees += 1;
            }
            InjectionResult::Saved => {}
        }
    }

    /// `injectFromStalledTorrent`: a source that hasn't finished in a day: the torrent is added paused from
    /// every match, so it can be resumed by hand.
    #[allow(clippy::too_many_arguments)]
    async fn inject_from_stalled_torrent(
        &self,
        progress: &str,
        shown: &str,
        client: &Arc<Client>,
        client_matches: &[InjectMatch],
        meta: &Metafile,
        tracker: &str,
        result: InjectionResult,
        ctx: &super::action::Ctx,
    ) {
        let mut injected = false;
        let mut linked_new_files = false;
        for m in client_matches {
            let res = perform_action_without_mutex(ctx, meta, Decision::MatchPartial, &m.searchee, tracker, Some(client), false).await;
            if matches!(res.action_result, InjectionResult::Saved | InjectionResult::TorrentNotComplete) {
                continue;
            }
            linked_new_files = res.linked_new_files;
            if res.action_result == InjectionResult::Failure {
                tracing::error!(
                    "cross-seed inject: {progress} Failed to inject {shown} using stalled source {} - {}",
                    m.searchee.name,
                    result.as_str()
                );
                continue;
            }
            if res.action_result == InjectionResult::AlreadyExists {
                continue;
            }
            tracing::warn!(
                "cross-seed inject: {progress} Injected {shown} using stalled source {}, you will need to resume or remove from client - {}",
                m.searchee.name,
                result.as_str()
            );
            injected = true;
        }
        if injected || !client.is_torrent_in_client(&meta.info_hash).await.unwrap_or(false) {
            return;
        }
        if linked_new_files {
            tracing::info!("cross-seed inject: {progress} Rechecking {shown} as new files were linked - {}", result.as_str());
        } else {
            tracing::warn!(
                "cross-seed inject: {progress} No new files linked for {shown}, resume or remove from client - {}",
                result.as_str()
            );
        }
    }

    /// `injectionAlreadyExists`: the client has it: it is checked again or resumed when it should be, and its
    /// file is deleted once it is complete.
    #[allow(clippy::too_many_arguments)]
    async fn injection_already_exists(
        &self,
        progress: &str,
        path: &str,
        shown: &str,
        client: &Arc<Client>,
        client_matches: &[InjectMatch],
        meta: &Metafile,
        linked_new_files: bool,
        summary: &mut InjectSummary,
        cfg: &RuntimeConfig,
    ) {
        let result = InjectionResult::AlreadyExists.as_str();
        let is_checking = client.is_torrent_checking(&meta.info_hash).await.unwrap_or(false);
        let mut is_complete = client.is_torrent_complete(&meta.info_hash).await.unwrap_or(false);
        let decision = [Decision::Match, Decision::MatchSizeOnly]
            .into_iter()
            .find(|d| client_matches.iter().any(|m| m.decision == *d && m.action_result == Some(InjectionResult::AlreadyExists)))
            .unwrap_or(Decision::MatchPartial);
        let resume = |check_once: bool| {
            let (c, m, cfg) = (client.clone(), meta.clone(), cfg.clone());
            tokio::spawn(async move { c.resume_injection(&m, decision, check_once, &cfg, ResumeTiming::default()).await })
        };
        if linked_new_files {
            tracing::info!("cross-seed inject: {progress} Rechecking {shown} as new files were linked - {result}");
        } else if is_checking {
            tracing::info!("cross-seed inject: {progress} {shown} is being checked by client - {result}");
            summary.tasks.push(resume(false));
        } else if !is_complete && decision != Decision::MatchPartial {
            let modified = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as f64)
                .unwrap_or(0.0);
            let final_check = modified + DAY_MS;
            if has_ext(&meta.files, VIDEO_DISC_EXTENSIONS) {
                tracing::warn!("cross-seed inject: {progress} Skipping recheck for {shown}: VIDEO_DISC_EXTENSIONS - {result}");
            } else {
                tracing::info!(
                    "cross-seed inject: {progress} Rechecking {shown} as it's not complete but has all files (final check at {}) - {result}",
                    human_readable_date(final_check)
                );
                client.recheck_torrent(&meta.info_hash).await;
            }
            summary.tasks.push(resume(false));
            if super::now_ms() >= final_check {
                is_complete = true;
            }
        } else if is_complete {
            tracing::info!("cross-seed inject: {progress} {shown} - {result}");
        } else {
            tracing::warn!("cross-seed inject: {progress} {shown} - {result} (incomplete)");
            summary.tasks.push(resume(true));
        }
        summary.already_exists += 1;
        if !is_complete {
            summary.incomplete_candidates += 1;
        }
        if is_complete {
            delete_torrent_file_if_safe(path);
        } else {
            summary.tasks.push(delete_torrent_file_if_complete(path.to_string(), client.clone(), meta.info_hash.clone()));
        }
    }

    /// `injectSavedTorrents`: the inject job over `injectDir` (when set) or `outputDir`; the run's summary.
    pub async fn inject_saved_torrents(&self, cfg: &RuntimeConfig) -> InjectSummary {
        let target = cfg.inject_dir.clone().or_else(|| cfg.output_dir.clone()).unwrap_or_default();
        if cfg.inject_dir.is_some() {
            tracing::warn!(
                "cross-seed inject: Manually injecting torrents performs minimal filtering which slightly increases chances of false positives, see the docs for more info"
            );
        }
        if cfg.ignore_titles == Some(true) {
            tracing::warn!("cross-seed inject: Ignoring torrent titles when looking for matches, this may result in false positives");
        }
        let paths = find_all_torrent_files_in_dir(Path::new(&target));
        if paths.is_empty() {
            tracing::info!("cross-seed inject: No torrent files are awaiting injection in {target}");
            return InjectSummary::default();
        }
        tracing::info!("cross-seed inject: Found {} torrent file(s) to inject in {target}", paths.len());
        let mut summary = InjectSummary { total: paths.len(), ..InjectSummary::default() };
        let real = self.find_all_searchees(Label::Inject, cfg).await;
        let ensembles = self.ensemble_searchees(&real, false, cfg).await;
        let mut searchees: Vec<Searchee> = real.into_iter().chain(ensembles).collect();
        let host_to_name = self.host_to_name();
        for (i, path) in paths.iter().enumerate() {
            let progress = format!("({}/{})", i + 1, paths.len());
            let _one = self.injection.lock().await;
            self.inject_saved_torrent(&progress, path, &mut summary, &mut searchees, cfg, &host_to_name).await;
        }
        log_summary(&summary, cfg.flat_linking);
        if cfg.inject_dir.is_some() {
            tracing::info!("cross-seed inject: Waiting on post-injection tasks to complete...");
            for t in std::mem::take(&mut summary.tasks) {
                let _ = t.await;
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str, host: Option<&str>, hash: bool, files: usize) -> Searchee {
        Searchee {
            name: name.into(),
            title: name.into(),
            client_host: host.map(String::from),
            info_hash: hash.then(|| "a".repeat(40)),
            files: (0..files).map(|i| super::super::searchee::File { name: format!("{i}"), path: format!("{i}"), length: 1 }).collect(),
            ..Searchee::default()
        }
    }

    #[test]
    fn matches_in_cross_seeds_order() {
        let m = |s: Searchee, d| InjectMatch { searchee: s, decision: d, action_result: None };
        let mut list = vec![
            m(s("partial", Some("a"), true, 1), Decision::MatchPartial),
            m(s("size", Some("a"), true, 1), Decision::MatchSizeOnly),
            m(s("full-data", Some("a"), false, 1), Decision::Match),
            m(s("full", Some("a"), true, 1), Decision::Match),
            m(s("other-client", Some("b"), true, 1), Decision::Match),
        ];
        let prio = |h: Option<&str>| if h == Some("a") { 0 } else { 1 };
        sort_matches(&mut list, &prio);
        let names: Vec<&str> = list.iter().map(|m| m.searchee.name.as_str()).collect();
        assert_eq!(names, ["full", "full-data", "size", "partial", "other-client"]);
    }

    #[test]
    fn only_files_cross_seed_saved_are_deleted() {
        let dir = std::env::temp_dir().join(format!("lantern-inject-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine = dir.join("[movie][Site A]Movie.2020.1080p-GRP[0123456789abcdef0123456789abcdef01234567].torrent");
        let theirs = dir.join("something.torrent");
        std::fs::write(&mine, b"x").unwrap();
        std::fs::write(&theirs, b"x").unwrap();
        delete_torrent_file_if_safe(&mine.to_string_lossy());
        delete_torrent_file_if_safe(&theirs.to_string_lossy());
        assert!(!mine.exists(), "named as cross-seed saves torrents");
        assert!(theirs.exists(), "not named by cross-seed, so kept");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            file_path_log("/o/[movie][A]M[0123456789abcdef0123456789abcdef01234567].torrent"),
            format!("/o/[movie][A]M[{}].torrent", sanitize_info_hash("0123456789abcdef0123456789abcdef01234567"))
        );
    }
}
