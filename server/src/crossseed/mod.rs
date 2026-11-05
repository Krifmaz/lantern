//! Cross-seed (goal 6): once a torrent grab has finished, Lantern looks for the same release on the other torrent
//! indexers. When another site has a torrent with exactly the same files, Lantern adds it to the same download
//! client, pointed at the files already on disk, so it seeds on both sites without downloading anything again.
//!
//! | Step | What |
//! | --- | --- |
//! | pick | finished torrent grabs not searched lately ([`CrossSeed::due`]) |
//! | search | every other enabled torrent indexer, by the release's name ([`same_release`], [`close_size`]) |
//! | compare | each candidate's `.torrent`, file by file and size by size ([`torrent`], [`files_match`]) |
//! | add | to the same client and folder, in the app's category with `.cross-seed` after it, so the app doesn't import it again |
//!
//! Every candidate's outcome is kept in `cross_seeds` ([`Decision`]), so the page can show what happened.

pub mod api;
pub mod torrent;

use crate::hub::clients::{AddRequest, Content};
use crate::hub::grabs::Grab;
use crate::hub::Hub;
use crate::indexers::cardigann::Grabbed;
use crate::indexers::release::{Protocol, Query, Release, SearchKind};
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use serde_json::{json, Value};
use std::time::Duration;

/// How far a candidate's size may be from the grab's and still be fetched to compare (2 %).
pub const SIZE_TOLERANCE: f64 = 0.02;

/// The category suffix of cross-seeded torrents: apps watch their own category, so they don't import these.
pub const CATEGORY_SUFFIX: &str = ".cross-seed";

/// The tables cross-seeding keeps: each candidate's outcome, and when each grab was last searched.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
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
        );",
    )
    .map_err(|e| anyhow::anyhow!("The cross-seed tables can't be made: {}", e.detail))
}

/// What became of one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Added to the client next to the files already there.
    Injected,
    /// The client already has this very torrent.
    AlreadyThere,
    /// Same name and size, but the files differ (other names, other sizes, other layout).
    FilesDiffer,
    /// The site only gives a magnet link, which has no file list to compare before downloading.
    MagnetOnly,
    /// The site's file isn't a torrent, or it couldn't be fetched or added.
    Failed,
}

impl Decision {
    /// How it is stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Injected => "injected",
            Decision::AlreadyThere => "already_there",
            Decision::FilesDiffer => "files_differ",
            Decision::MagnetOnly => "magnet_only",
            Decision::Failed => "failed",
        }
    }

    /// What it means, in words.
    pub fn words(decision: &str) -> &'static str {
        match decision {
            "injected" => "Seeding here too, from the files already downloaded",
            "already_there" => "The download client already has this torrent",
            "files_differ" => "Same release name, but its files differ, so it would need a new download",
            "magnet_only" => "This site gives only a magnet link, whose files can't be compared first",
            _ => "Couldn't be checked or added",
        }
    }
}

/// A release name made comparable: lower case, the file extension left out, every run of punctuation and spaces
/// as one space (`Show.Name.S01E02.1080p-GRP.mkv` and `show name s01e02 1080p grp` are the same).
pub fn normalize(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    let stem = match lower.rsplit_once('.') {
        Some((stem, ext)) if (2..=4).contains(&ext.len()) && VIDEO_AND_AUDIO.contains(&ext) => stem.to_string(),
        _ => lower,
    };
    stem.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The file endings left out of a name before comparing.
const VIDEO_AND_AUDIO: [&str; 12] = ["mkv", "mp4", "avi", "ts", "m2ts", "mov", "wmv", "flac", "mp3", "m4a", "epub", "torrent"];

/// Whether `candidate` is the same release as `title` by name.
pub fn same_release(title: &str, candidate: &str) -> bool {
    let (a, b) = (normalize(title), normalize(candidate));
    !a.is_empty() && a == b
}

/// Whether a candidate's size (when the site says one) is within [`SIZE_TOLERANCE`] of `size`.
pub fn close_size(size: i64, candidate: Option<u64>) -> bool {
    match candidate {
        None => true,
        Some(c) if size <= 0 => c > 0,
        Some(c) => ((c as f64) - (size as f64)).abs() <= size as f64 * SIZE_TOLERANCE,
    }
}

/// Whether two file lists are the same files: the same paths inside the save folder with the same sizes.
pub fn files_match(have: &[(String, i64)], candidate: &[(String, i64)]) -> bool {
    let norm = |l: &[(String, i64)]| {
        let mut v: Vec<(String, i64)> = l.iter().map(|(p, s)| (p.trim_start_matches('/').replace('\\', "/"), *s)).collect();
        v.sort();
        v
    };
    !have.is_empty() && norm(have) == norm(candidate)
}

/// The paths of `files` (as the client sees them) inside `save_path`.
pub fn relative(files: &[(String, i64)], save_path: &str) -> Vec<(String, i64)> {
    let base = format!("{}/", save_path.trim_end_matches('/'));
    files.iter().map(|(p, s)| (p.strip_prefix(&base).unwrap_or(p).to_string(), *s)).collect()
}

/// How one grab's search went: the candidates looked at and the ones added.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// Candidates with the same name and a close size.
    pub candidates: usize,
    /// Of them, the ones now seeding too.
    pub injected: usize,
    /// Indexers that couldn't be searched: (name, why).
    pub failed: Vec<(String, String)>,
}

/// Cross-seeding over the hub's grabs.
#[derive(Clone)]
pub struct CrossSeed {
    /// The hub: its grabs, clients and indexers.
    pub hub: Hub,
}

impl CrossSeed {
    /// Cross-seeding for `hub`.
    pub fn new(hub: Hub) -> CrossSeed {
        CrossSeed { hub }
    }

    fn db(&self) -> &SqliteDb {
        &self.hub.db
    }

    /// Whether it runs by itself (the `cross_seed` setting).
    pub fn enabled(&self) -> bool {
        self.hub.settings.bool("cross_seed")
    }

    /// The wait between two grabs' searches (the `cross_seed_delay` setting, seconds), so sites aren't flooded.
    pub fn delay(&self) -> Duration {
        Duration::from_secs(self.hub.settings.int("cross_seed_delay").clamp(0, 3600) as u64)
    }

    /// Days before a grab is searched again (the `cross_seed_again_days` setting), when new copies may be up.
    pub fn again_days(&self) -> f64 {
        self.hub.settings.int("cross_seed_again_days").clamp(1, 365) as f64
    }

    /// The finished torrent grabs whose turn has come: never searched, or last searched over
    /// [`again_days`](Self::again_days) ago, at most four times in all; oldest first.
    pub fn due(&self, limit: i64) -> Result<Vec<Grab>> {
        let before = crate::auth::now() - self.again_days() * 86400.0;
        let rows = self.db().query(
            "SELECT g.* FROM hub_grabs g LEFT JOIN cross_seed_searches s ON s.grab_id = g.id
             WHERE g.protocol='torrent' AND g.state IN ('seeding','completed') AND g.client_id != 0
               AND (s.grab_id IS NULL OR (s.at < ? AND s.times < 4))
             ORDER BY COALESCE(s.at, 0), g.finished LIMIT ?",
            p![before, limit],
        )?;
        Ok(rows.iter().map(Grab::from_row).collect())
    }

    /// Searches the other torrent indexers for grab `id`'s release and adds every copy whose files match.
    pub async fn search_grab(&self, id: i64) -> Result<Outcome> {
        let g = self.hub.get_grab(id)?;
        if g.protocol != "torrent" || !matches!(g.state.as_str(), "seeding" | "completed") {
            return Err(err(409, "Only a finished torrent can be cross-seeded"));
        }
        let cfg = self.hub.client(g.client_id)?;
        let backend = self.hub.backend(&cfg)?;
        let items = backend.items().await.map_err(|e| err(502, e))?;
        let item = items
            .into_iter()
            .find(|i| if g.client_ref.is_empty() { i.tag == g.tag() } else { i.id.eq_ignore_ascii_case(&g.client_ref) })
            .ok_or_else(|| err(404, format!("{} doesn't have {} any more, so there is nothing to seed from", cfg.name, g.title)))?;
        let have = relative(&backend.files(&item).await.map_err(|e| err(502, e))?, &item.save_path);
        if have.is_empty() {
            return Err(err(409, format!("{} doesn't list the files of {}, so they can't be compared", cfg.name, g.title)));
        }
        let size: i64 = if g.size > 0 { g.size } else { have.iter().map(|(_, s)| s).sum() };
        let torrent_indexers: Vec<i64> = self
            .hub
            .indexers
            .list()?
            .into_iter()
            .filter(|ix| ix.enabled && ix.id != g.indexer_id && ix.kind != "newznab")
            .map(|ix| ix.id)
            .collect();
        let mut out = Outcome::default();
        if !torrent_indexers.is_empty() {
            let q = Query { kind: SearchKind::Search, keywords: g.title.clone(), limit: 100, ..Query::default() };
            let (found, failed) = self.hub.indexers.search(&torrent_indexers, &q, "Lantern cross-seed").await?;
            out.failed = failed;
            let seen = self.seen(g.id)?;
            let candidates: Vec<Release> = found
                .into_iter()
                .filter(|r| r.protocol == Protocol::Torrent && r.indexer_id != g.indexer_id)
                .filter(|r| same_release(&g.title, &r.title) && close_size(size, r.size))
                .filter(|r| !seen.contains(&(r.indexer_id, r.title.clone())))
                .collect();
            let mut client_hashes: Vec<String> =
                backend.items().await.unwrap_or_default().into_iter().map(|i| i.id.to_lowercase()).collect();
            client_hashes.push(item.id.to_lowercase());
            for r in candidates {
                out.candidates += 1;
                let (decision, hash, detail) = self.try_one(&g, cfg.id, &item.save_path, &have, &client_hashes, &r).await;
                if decision == Decision::Injected {
                    out.injected += 1;
                    client_hashes.push(hash.clone());
                }
                self.db().execute(
                    "INSERT INTO cross_seeds (grab_id, indexer_id, indexer, title, info_hash, decision, detail, client_id, at) VALUES (?,?,?,?,?,?,?,?,?)",
                    p![g.id, r.indexer_id, r.indexer, r.title, hash, decision.as_str(), detail, cfg.id, crate::auth::now()],
                )?;
            }
        }
        self.db().execute(
            "INSERT INTO cross_seed_searches (grab_id, at, times, found) VALUES (?,?,1,?)
             ON CONFLICT(grab_id) DO UPDATE SET at=excluded.at, times=times+1, found=found+excluded.found",
            p![g.id, crate::auth::now(), out.injected as i64],
        )?;
        Ok(out)
    }

    /// The candidates already decided for grab `id`: (indexer, title).
    fn seen(&self, id: i64) -> Result<Vec<(i64, String)>> {
        Ok(self
            .db()
            .query("SELECT indexer_id, title FROM cross_seeds WHERE grab_id=? AND decision != 'failed'", p![id])?
            .iter()
            .map(|r| (r.i("indexer_id"), r.s("title")))
            .collect())
    }

    /// Fetches candidate `r`'s torrent, compares it with the files on disk and adds it when they match:
    /// (decision, its info hash, words).
    async fn try_one(
        &self,
        g: &Grab,
        client_id: i64,
        save_path: &str,
        have: &[(String, i64)],
        client_hashes: &[String],
        r: &Release,
    ) -> (Decision, String, String) {
        if r.link.starts_with("magnet:") {
            return (Decision::MagnetOnly, r.infohash.to_lowercase(), String::new());
        }
        let bytes = match self.hub.indexers.grab(r.indexer_id, &r.link, &r.title, "Lantern cross-seed").await {
            Ok(Grabbed::File(b)) => b,
            Ok(Grabbed::Magnet(_)) => return (Decision::MagnetOnly, r.infohash.to_lowercase(), String::new()),
            Err(e) => return (Decision::Failed, String::new(), e.detail),
        };
        let Some(meta) = torrent::parse(&bytes) else {
            return (Decision::Failed, String::new(), format!("{} sent a file that isn't a torrent", r.indexer));
        };
        if client_hashes.contains(&meta.info_hash) {
            return (Decision::AlreadyThere, meta.info_hash, String::new());
        }
        if !files_match(have, &meta.files) {
            let detail = format!(
                "{} files of {} bytes here, {} files of {} bytes there",
                have.len(),
                have.iter().map(|(_, s)| s).sum::<i64>(),
                meta.files.len(),
                meta.total()
            );
            return (Decision::FilesDiffer, meta.info_hash, detail);
        }
        let added = async {
            let cfg = self.hub.client(client_id).map_err(|e| e.detail)?;
            let backend = self.hub.backend(&cfg).map_err(|e| e.detail)?;
            let category = if g.category.is_empty() { "cross-seed".to_string() } else { format!("{}{CATEGORY_SUFFIX}", g.category) };
            backend.ensure_category(&category, save_path).await?;
            let req = AddRequest {
                content: Content::Torrent(bytes),
                title: r.title.clone(),
                category,
                save_path: save_path.to_string(),
                tag: format!("lantern-xs-{}", g.id),
            };
            backend.add(&req).await.map(|_| ())
        }
        .await;
        match added {
            Ok(()) => (Decision::Injected, meta.info_hash, String::new()),
            Err(e) => (Decision::Failed, meta.info_hash, e),
        }
    }

    /// One pass: searches every grab whose turn has come, waiting [`delay`](Self::delay) between them.
    pub async fn run(&self, limit: i64) -> Result<Outcome> {
        let mut all = Outcome::default();
        for (n, g) in self.due(limit)?.into_iter().enumerate() {
            if n > 0 {
                tokio::time::sleep(self.delay()).await;
            }
            match self.search_grab(g.id).await {
                Ok(o) => {
                    all.candidates += o.candidates;
                    all.injected += o.injected;
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

fn outcome_json(r: &Row) -> Value {
    json!({
        "id": r.i("id"),
        "grab_id": r.i("grab_id"),
        "grab_title": r.s("grab_title"),
        "indexer_id": r.i("indexer_id"),
        "indexer": r.s("indexer"),
        "title": r.s("title"),
        "info_hash": r.s("info_hash"),
        "decision": r.s("decision"),
        "words": Decision::words(&r.s("decision")),
        "detail": r.s("detail"),
        "at": r.f("at"),
    })
}

/// Runs cross-seeding by itself while the `cross_seed` setting is on: a pass every ten minutes, a handful of
/// grabs at a time.
pub fn spawn(xs: CrossSeed) {
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
    fn names_compare_without_punctuation_or_ending() {
        assert!(same_release("Show.Name.S01E02.1080p.WEB-DL-GRP", "Show Name S01E02 1080p WEB DL GRP"));
        assert!(same_release("Movie.2025.2160p-GRP.mkv", "movie 2025 2160p grp"));
        assert!(!same_release("Show.Name.S01E02.1080p-GRP", "Show.Name.S01E03.1080p-GRP"));
        assert!(!same_release("", ""));
        assert_eq!(normalize("Some.Band - Album (2025) [FLAC]"), "some band album 2025 flac");
    }

    #[test]
    fn sizes_and_files() {
        assert!(close_size(1000, Some(1015)) && !close_size(1000, Some(1100)) && close_size(1000, None));
        let have = relative(&[("/dl/tv/Show.S01/e1.mkv".to_string(), 10), ("/dl/tv/Show.S01/e2.mkv".to_string(), 11)], "/dl/tv/");
        assert_eq!(have[0].0, "Show.S01/e1.mkv");
        assert!(files_match(&have, &[("Show.S01/e2.mkv".to_string(), 11), ("Show.S01/e1.mkv".to_string(), 10)]));
        assert!(!files_match(&have, &[("Show.S01/e1.mkv".to_string(), 10)]));
        assert!(!files_match(&have, &[("Other/e1.mkv".to_string(), 10), ("Other/e2.mkv".to_string(), 11)]));
        assert!(!files_match(&[], &[]));
    }

    #[test]
    fn every_decision_has_words() {
        for d in [Decision::Injected, Decision::AlreadyThere, Decision::FilesDiffer, Decision::MagnetOnly, Decision::Failed] {
            assert!(!Decision::words(d.as_str()).is_empty());
        }
        assert_ne!(Decision::words("injected"), Decision::words("failed"));
    }
}
