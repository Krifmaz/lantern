//! The cross-seed routes.
//!
//! ```text
//! The web app (signed in; running needs an admin):
//!   GET  /api/cross-seed?limit=        {enabled, stats {injected, searched, waiting}, recent [outcome]}
//!   POST /api/cross-seed/run           search the grabs whose turn has come, now, in the background
//!   POST /api/cross-seed/grabs/{id}    search one finished grab now: {candidates, injected, failed, message}
//! ```

use super::CrossSeed;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{Admin, Result, SignedIn};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The cross-seed routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/cross-seed", get(overview))
        .route("/api/cross-seed/run", post(run))
        .route("/api/cross-seed/grabs/{id}", post(one))
}

async fn overview(State(st): State<AppState>, _: SignedIn, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>> {
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    Ok(Json(json!({ "enabled": st.cross_seed.enabled(), "stats": st.cross_seed.stats()?, "recent": st.cross_seed.recent(limit)? })))
}

async fn run(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    let xs: CrossSeed = st.cross_seed.clone();
    let waiting = xs.due(10_000)?.len();
    tokio::spawn(async move {
        if let Err(e) = xs.run(50).await {
            tracing::warn!("cross-seed: {}", e.detail);
        }
    });
    let message = match waiting {
        0 => "Every finished torrent was searched lately; nothing to do now".to_string(),
        1 => "Searching the other indexers for 1 finished torrent".to_string(),
        n => format!("Searching the other indexers for {} finished torrents, one at a time", n.min(50)),
    };
    Ok(Json(json!({ "message": message, "waiting": waiting })))
}

/// The words for one grab's search.
pub fn outcome_words(o: &super::Outcome) -> String {
    let base = match (o.candidates, o.injected) {
        (0, _) => "No other indexer has this release".to_string(),
        (c, 0) => format!("{c} other cop{} found, but none has the same files", if c == 1 { "y" } else { "ies" }),
        (_, 1) => "Seeding on 1 more site now, from the same files".to_string(),
        (_, i) => format!("Seeding on {i} more sites now, from the same files"),
    };
    match o.failed.len() {
        0 => base,
        1 => format!("{base} ({} couldn't be searched: {})", o.failed[0].0, o.failed[0].1),
        n => format!("{base} ({n} indexers couldn't be searched)"),
    }
}

async fn one(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    let o = st.cross_seed.search_grab(id).await?;
    let failed: Vec<Value> = o.failed.iter().map(|(n, e)| json!({ "indexer": n, "error": e })).collect();
    Ok(Json(json!({ "candidates": o.candidates, "injected": o.injected, "failed": failed, "message": outcome_words(&o) })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crossseed::Outcome;

    #[test]
    fn says_how_a_search_went() {
        assert_eq!(outcome_words(&Outcome::default()), "No other indexer has this release");
        assert_eq!(
            outcome_words(&Outcome { candidates: 2, injected: 0, failed: vec![] }),
            "2 other copies found, but none has the same files"
        );
        let o = Outcome { candidates: 2, injected: 1, failed: vec![("Site".into(), "is down".into())] };
        assert_eq!(outcome_words(&o), "Seeding on 1 more site now, from the same files (Site couldn't be searched: is down)");
    }
}
