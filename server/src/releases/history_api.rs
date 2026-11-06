// Ported from autobrr v1.90.0 (internal/http/release.go, params.go), GPL-2.0-or-later.
//! autobrr's release history routes, relative to its `/api` (Lantern serves them under `/autobrr/api`):
//!
//! ```text
//! GET    /release                                    a page: limit, offset, cursor, indexer…, push_status, q
//! DELETE /release                                    olderThan (hours), indexer…, releaseStatus… (204)
//! GET    /release/recent                             the newest ten
//! GET    /release/stats                              the counts by push status
//! GET    /release/stats/{activity,volume,heatmap,indexers,filters}?days=30
//! GET    /release/indexers                           every indexer name to filter by
//! GET    /release/{id}                               one, with its action statuses
//! POST   /release/{id}/actions/{statusId}/retry      run that action again (204)
//! GET    /release/profiles/duplicate                 the duplicate profiles
//! POST   /release/profiles/duplicate                 add or change one (201)
//! DELETE /release/profiles/duplicate/{profileId}     remove (204)
//! GET    /release/cleanup-jobs                       the cleanup jobs
//! POST   /release/cleanup-jobs                       add (201)
//! GET    /release/cleanup-jobs/{id}                  one
//! PUT    /release/cleanup-jobs/{id}                  change (200)
//! DELETE /release/cleanup-jobs/{id}                  remove (204)
//! PATCH  /release/cleanup-jobs/{id}/enabled          {enabled} (204)
//! POST   /release/cleanup-jobs/{id}/run              run now (204)
//! ```
//!
//! `POST /release/process` is turned off in autobrr v1.90.0 (its route is commented out), so it answers 405 here
//! too.

use super::autobrr_api::{decode, id_of, no_content, or_not_found, send, Answer, Fail};
use super::history::{CleanupJob, DeleteRequest, DuplicateProfile, Query, PUSH_APPROVED, PUSH_ERROR, PUSH_PENDING, PUSH_REJECTED};
use super::store::{ApiError, ApiResult};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use kit_server_core::{Admin, SignedIn};
use serde_json::Value;

/// The release history, duplicate profile and cleanup job routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/release", get(find).delete(remove))
        .route("/release/", get(find).delete(remove))
        .route("/release/recent", get(recent))
        .route("/release/stats", get(stats))
        .route("/release/stats/{kind}", get(stats_of))
        .route("/release/indexers", get(indexer_options))
        .route("/release/{id}", get(one))
        .route("/release/{id}/", get(one))
        .route("/release/{id}/actions/{status}/retry", post(retry))
        .route("/release/profiles/duplicate", get(profiles).post(store_profile))
        .route("/release/profiles/duplicate/", get(profiles).post(store_profile))
        .route("/release/profiles/duplicate/{id}", delete(delete_profile))
        .route("/release/cleanup-jobs", get(jobs).post(store_job))
        .route("/release/cleanup-jobs/", get(jobs).post(store_job))
        .route("/release/cleanup-jobs/{id}", get(job).put(update_job).delete(delete_job))
        .route("/release/cleanup-jobs/{id}/", get(job).put(update_job).delete(delete_job))
        .route("/release/cleanup-jobs/{id}/enabled", patch(toggle_job))
        .route("/release/cleanup-jobs/{id}/run", post(run_job))
}

/// Every value of the query, in order.
fn pairs(q: Option<&str>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.unwrap_or("").as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

/// The first value of `key` (Go's `Query().Get`).
fn first<'a>(q: &'a [(String, String)], key: &str) -> &'a str {
    q.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("")
}

/// Every value of `key` (Go's `Query()[key]`).
fn all(q: &[(String, String)], key: &str) -> Vec<String> {
    q.iter().filter(|(k, _)| k == key).map(|(_, v)| v.clone()).collect()
}

/// A whole number from the query, `default` when it is missing (autobrr's `parseQueryParamInt`).
pub(crate) fn query_int(q: &[(String, String)], key: &str, default: i64) -> ApiResult<i64> {
    let v = first(q, key);
    if v.is_empty() {
        return Ok(default);
    }
    match v.parse::<i64>() {
        Ok(n) if n >= 0 && i32::try_from(n).is_ok() => Ok(n),
        _ => Err(ApiError::BadRequest(format!("{key} parameter is invalid"))),
    }
}

async fn find(State(st): State<AppState>, _: SignedIn, RawQuery(q): RawQuery) -> Answer {
    let q = pairs(q.as_deref());
    let limit =
        query_int(&q, "limit", 20).ok().filter(|l| *l != 0).ok_or_else(|| ApiError::BadRequest("limit parameter is invalid".into()))?;
    let offset = query_int(&q, "offset", 0)?;
    let cursor = query_int(&q, "cursor", 0)?;
    let push_status = first(&q, "push_status").to_string();
    if !push_status.is_empty() && !super::history::valid_push_status(&push_status) {
        return Err(ApiError::BadRequest(format!("push_status parameter is of invalid type: {push_status}")).into());
    }
    let query = Query { limit, offset, cursor, indexers: all(&q, "indexer"), push_status, search: first(&q, "q").to_string() };
    send(StatusCode::OK, st.releases.find_history(&query)?)
}

async fn recent(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.find_history(&Query { limit: 10, ..Query::default() })?)
}

async fn one(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "releaseID")?;
    send(StatusCode::OK, st.releases.history_release(id)?)
}

async fn indexer_options(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.history_indexer_options()?)
}

async fn stats(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.history_stats()?)
}

async fn stats_of(State(st): State<AppState>, _: SignedIn, Path(kind): Path<String>, RawQuery(q): RawQuery) -> Answer {
    let q = pairs(q.as_deref());
    let rel = &st.releases;
    let fetch: fn(&super::Releases, i64) -> ApiResult<Value> = match kind.as_str() {
        "activity" => super::Releases::stats_activity,
        "volume" => super::Releases::stats_volume,
        "heatmap" => super::Releases::stats_heatmap,
        "indexers" => super::Releases::stats_top_indexers,
        "filters" => super::Releases::stats_top_filters,
        _ => return Ok((StatusCode::NOT_FOUND, "404 page not found\n").into_response()),
    };
    let days =
        query_int(&q, "days", 30).ok().filter(|d| *d <= 3650).ok_or_else(|| ApiError::BadRequest("days parameter is invalid".into()))?;
    send(StatusCode::OK, fetch(rel, days)?)
}

async fn remove(State(st): State<AppState>, _: Admin, RawQuery(q): RawQuery) -> Answer {
    let q = pairs(q.as_deref());
    let older_than = query_int(&q, "olderThan", 0)?;
    let mut statuses = Vec::new();
    for s in all(&q, "releaseStatus") {
        if ![PUSH_APPROVED, PUSH_REJECTED, PUSH_ERROR, PUSH_PENDING].contains(&s.as_str()) {
            return Err(ApiError::BadRequest(format!("releaseStatus contains invalid value: {s}")).into());
        }
        statuses.push(s);
    }
    st.releases.delete_history(&DeleteRequest { older_than, indexers: all(&q, "indexer"), statuses })?;
    no_content()
}

async fn retry(State(st): State<AppState>, _: Admin, Path((id, status)): Path<(String, String)>) -> Answer {
    let release_id = id_of(&id, "releaseID")?;
    let status_id = id_of(&status, "actionStatusID")?;
    st.releases
        .retry_action(release_id, status_id)
        .await
        .map_err(|e| or_not_found(e, || format!("action status with id {status_id} not found for release {release_id}")))?;
    no_content()
}

// ---- duplicate profiles ----

async fn profiles(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.duplicate_profiles()?)
}

async fn store_profile(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let mut p: DuplicateProfile = decode(&body)?;
    st.releases.store_duplicate_profile(&mut p)?;
    send(StatusCode::CREATED, p)
}

async fn delete_profile(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "profileId")?;
    st.releases.delete_duplicate_profile(id)?;
    no_content()
}

// ---- cleanup jobs ----

/// A path's job id, or autobrr's `jobID parameter is invalid`.
fn job_id(raw: &str) -> Result<i64, Fail> {
    Ok(id_of(raw, "jobID")?)
}

/// autobrr's not-found answer for cleanup job `id`.
fn job_missing(id: i64) -> impl FnOnce() -> String {
    move || format!("could not find cleanup job with id {id}")
}

async fn jobs(State(st): State<AppState>, _: SignedIn) -> Answer {
    let list: Vec<Value> = st.releases.cleanup_jobs()?.iter().map(CleanupJob::public).collect();
    send(StatusCode::OK, list)
}

async fn job(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = job_id(&id)?;
    let j = st.releases.cleanup_job(id).map_err(|e| or_not_found(e, job_missing(id)))?;
    send(StatusCode::OK, j.public())
}

async fn store_job(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let mut j: CleanupJob = decode(&body)?;
    st.releases.store_cleanup_job(&mut j)?;
    send(StatusCode::CREATED, j.public())
}

async fn update_job(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let j: CleanupJob = decode(&body)?;
    st.releases.update_cleanup_job(&j).map_err(|e| or_not_found(e, job_missing(j.id)))?;
    send(StatusCode::OK, j.public())
}

async fn delete_job(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = job_id(&id)?;
    st.releases.delete_cleanup_job(id).map_err(|e| or_not_found(e, job_missing(id)))?;
    no_content()
}

async fn toggle_job(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = job_id(&id)?;
    let v: Value = decode(&body)?;
    let enabled = v.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    st.releases.toggle_cleanup_job(id, enabled).map_err(|e| or_not_found(e, job_missing(id)))?;
    no_content()
}

async fn run_job(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = job_id(&id)?;
    st.releases.force_run_cleanup_job(id).map_err(|e| or_not_found(e, job_missing(id)))?;
    no_content()
}
