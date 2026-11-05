// Ported from autobrr v1.90.0 (internal/http/feed.go), GPL-2.0-or-later.
//! autobrr's feed routes, relative to its `/api` (Lantern serves them under `/autobrr/api`):
//!
//! ```text
//! GET    /feeds                 every feed, with when each runs next
//! POST   /feeds                 add (201, the feed)
//! POST   /feeds/test            read the first page of a feed as sent (204)
//! POST   /feeds/caps            what the indexer of a feed as sent can do
//! GET    /feeds/{id}            one
//! PUT    /feeds/{id}            change (200, the feed)
//! DELETE /feeds/{id}            remove (204)
//! DELETE /feeds/{id}/cache      forget what it has seen (204)
//! PATCH  /feeds/{id}/enabled    {enabled} (204)
//! GET    /feeds/{id}/latest     what the last run read, as it came
//! POST   /feeds/{id}/forcerun   read it now (204)
//! GET    /feeds/{id}/caps       read and save what its indexer can do
//! ```

use super::autobrr_api::{decode, id_of, no_content, or_not_found, send, Answer, Fail};
use super::feed::Feed;
use super::store::ApiError;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use kit_server_core::{Admin, SignedIn};

/// The feed routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/feeds", get(find).post(store))
        .route("/feeds/", get(find).post(store))
        .route("/feeds/test", post(test))
        .route("/feeds/caps", post(caps))
        .route("/feeds/{id}", get(find_by_id).put(update).delete(remove))
        .route("/feeds/{id}/", get(find_by_id).put(update).delete(remove))
        .route("/feeds/{id}/cache", delete(delete_cache))
        .route("/feeds/{id}/enabled", patch(toggle))
        .route("/feeds/{id}/latest", get(latest))
        .route("/feeds/{id}/forcerun", post(force_run))
        .route("/feeds/{id}/caps", get(caps_by_id))
}

/// Feed `id`'s not-found answer in autobrr's words.
fn missing(id: i64) -> impl FnOnce() -> String {
    move || format!("could not find feed with id {id}")
}

/// A path's feed id, or autobrr's `feedID parameter is invalid`.
fn feed_id(raw: &str) -> Result<i64, Fail> {
    Ok(id_of(raw, "feedID")?)
}

async fn find(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.feeds()?)
}

async fn find_by_id(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = feed_id(&id)?;
    send(StatusCode::OK, st.releases.feed_by_id(id).map_err(|e| or_not_found(e, missing(id)))?)
}

async fn store(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let f: Feed = decode(&body)?;
    send(StatusCode::CREATED, st.releases.store_feed(f)?)
}

async fn test(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let f: Feed = decode(&body)?;
    let id = f.id;
    st.releases.test_feed(f).await.map_err(|e| or_not_found(e, missing(id)))?;
    no_content()
}

async fn caps(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let f: Option<Feed> = decode(&body)?;
    send(StatusCode::OK, st.releases.fetch_feed_caps(f).await?)
}

async fn update(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let f: Feed = decode(&body)?;
    let id = f.id;
    send(StatusCode::OK, st.releases.update_feed(f).map_err(|e| or_not_found(e, missing(id)))?)
}

async fn force_run(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = feed_id(&id)?;
    st.releases.force_run_feed(id).await.map_err(|e| or_not_found(e, missing(id)))?;
    no_content()
}

async fn caps_by_id(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = feed_id(&id)?;
    send(StatusCode::OK, st.releases.fetch_feed_caps_by_id(id).await.map_err(|e| or_not_found(e, missing(id)))?)
}

/// The body of `PATCH /feeds/{id}/enabled`.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Enabled {
    /// On or off.
    enabled: bool,
}

async fn toggle(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = feed_id(&id)?;
    let e: Enabled = decode(&body)?;
    st.releases.toggle_feed(id, e.enabled).map_err(|e| or_not_found(e, missing(id)))?;
    no_content()
}

async fn remove(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = feed_id(&id)?;
    st.releases.delete_feed(id).map_err(|e| or_not_found(e, missing(id)))?;
    no_content()
}

async fn delete_cache(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = feed_id(&id)?;
    st.releases.delete_feed_cache(id).map_err(|e| or_not_found(e, missing(id)))?;
    no_content()
}

async fn latest(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = feed_id(&id)?;
    let data = st.releases.feed_last_run_data(id).map_err(|e| or_not_found(e, missing(id)))?;
    if data.is_empty() {
        return Err(Fail(ApiError::NotFound(format!("no last run data found for feed with id {id}"))));
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], data).into_response())
}
