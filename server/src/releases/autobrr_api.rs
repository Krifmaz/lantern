// Ported from autobrr v1.90.0 (internal/http/filter.go, action.go, downloader.go, encoder.go, params.go),
// GPL-2.0-or-later.
//! autobrr's API for filters, actions and download clients, with autobrr's paths, JSON and status codes. The
//! paths here are relative to autobrr's `/api`; Lantern serves them under `/autobrr/api` (the forked autobrr
//! web app's base), so they never meet Lantern's own `/api/...`.
//!
//! ```text
//! GET    /filters?sort=name-asc&indexer=…    the filters (sort: name|priority|created_at|updated_at - asc|desc)
//! POST   /filters                            add (201, the filter)
//! POST   /filters/indexers/prune-deprecated  {identifiers}: {removed}
//! POST   /filters/external/test              an external filter run on a sample release
//! GET    /filters/{id}                       one, with actions, external filters and notifications
//! PUT    /filters/{id}                       replace (200, the filter)
//! PATCH  /filters/{id}                       change the fields sent (204)
//! DELETE /filters/{id}                       remove (204)
//! GET    /filters/{id}/duplicate             copy (200, the copy)
//! PUT    /filters/{id}/enabled               {enabled} (204)
//! GET    /filters/{id}/notifications         its notifications
//! PUT    /filters/{id}/notifications         [{notification_id, events}] (204)
//! GET    /actions                            every action
//! POST   /actions                            add (201)
//! PUT    /actions/{id}                       change (200)
//! DELETE /actions/{id}                       remove (204)
//! PATCH  /actions/{id}/toggleEnabled         on/off (204)
//! GET    /downloaders                        the download clients (secrets redacted)
//! POST   /downloaders                        add (201)
//! PUT    /downloaders                        change (200)
//! POST   /downloaders/test                   test the connection (204)
//! GET    /downloaders/{id}                   one
//! DELETE /downloaders/{id}                   remove (204)
//! GET    /downloaders/{id}/arr/tags          an *arr's tags
//! ```
//!
//! Errors answer autobrr's `{code, message}` (`RECORD_NOT_FOUND` 404, `BAD_REQUEST_PARAMS` 400,
//! `INTERNAL_SERVER_ERROR` 500), with Lantern's `detail` holding the same message.

use super::action::Action;
use super::downloader::{ArrTag, Downloader};
use super::external::FilterExternal;
use super::store::{parse_sort, ApiError, ApiResult, FilterNotification};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use kit_server_core::{Admin, SignedIn};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// autobrr's filter, action and download client routes, relative to its `/api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/filters", get(filters).post(store_filter))
        .route("/filters/indexers/prune-deprecated", post(prune_deprecated))
        .route("/filters/external/test", post(test_external))
        .route("/filters/{id}", get(filter).put(update_filter).patch(update_partial).delete(delete_filter))
        .route("/filters/{id}/duplicate", get(duplicate))
        .route("/filters/{id}/enabled", put(toggle_filter))
        .route("/filters/{id}/notifications", get(filter_notifications).put(update_notifications))
        .route("/filters/{id}/notifications/", get(filter_notifications).put(update_notifications))
        .route("/actions", get(actions).post(store_action))
        .route("/actions/{id}", put(update_action).delete(delete_action))
        .route("/actions/{id}/toggleEnabled", patch(toggle_action))
        .route("/downloaders", get(downloaders).post(store_downloader).put(update_downloader))
        .route("/downloaders/test", post(test_downloader))
        .route("/downloaders/{id}", get(downloader).delete(delete_downloader))
        .route("/downloaders/{id}/arr/tags", get(arr_tags))
}

/// An answer: autobrr's error shape for a failure.
type Answer = std::result::Result<Response, Fail>;

/// A failed call, answered as autobrr's encoder does.
pub struct Fail(pub ApiError);

impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Fail {
        Fail(e)
    }
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "RECORD_NOT_FOUND"),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST_PARAMS"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_SERVER_ERROR"),
        };
        let m = self.0.message();
        (status, Json(json!({ "code": code, "message": m, "detail": m }))).into_response()
    }
}

/// `v` as JSON with `status`.
fn send(status: StatusCode, v: impl serde::Serialize) -> Answer {
    Ok((status, Json(serde_json::to_value(v).unwrap_or_default())).into_response())
}

/// 204.
fn no_content() -> Answer {
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A path id, or autobrr's `<name> parameter is invalid`.
fn id_of(raw: &str, name: &str) -> ApiResult<i64> {
    raw.parse::<i64>().map_err(|_| ApiError::BadRequest(format!("{name} parameter is invalid")))
}

/// The body read as `T`; a bad body is a 500, as autobrr's `encoder.Error` answers a decode error.
fn decode<T: DeserializeOwned>(body: &Bytes) -> ApiResult<T> {
    serde_json::from_slice(body).map_err(|e| ApiError::Internal(e.to_string()))
}

/// The query's `sort` and every `indexer`.
fn query_parts(q: Option<&str>) -> (String, Vec<String>) {
    let mut sort = String::new();
    let mut indexers = Vec::new();
    for (k, v) in url::form_urlencoded::parse(q.unwrap_or("").as_bytes()) {
        match k.as_ref() {
            "sort" => sort = v.into_owned(),
            "indexer" => indexers.push(v.into_owned()),
            _ => {}
        }
    }
    (sort, indexers)
}

// ---- filters ----

async fn filters(State(st): State<AppState>, _: SignedIn, RawQuery(q): RawQuery) -> Answer {
    let (sort, indexers) = query_parts(q.as_deref());
    let sort = parse_sort(&sort)?;
    send(StatusCode::OK, st.releases.autobrr_filters(sort, &indexers)?)
}

async fn filter(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "filterID")?;
    send(StatusCode::OK, st.releases.autobrr_filter(id)?)
}

async fn duplicate(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "filterID")?;
    send(StatusCode::OK, st.releases.autobrr_duplicate_filter(id)?)
}

async fn prune_deprecated(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let ids: Vec<String> = if body.is_empty() {
        Vec::new()
    } else {
        let v: Value = serde_json::from_slice(&body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
        serde_json::from_value(v.get("identifiers").cloned().unwrap_or(Value::Null)).unwrap_or_default()
    };
    send(StatusCode::OK, json!({ "removed": st.releases.prune_deprecated_indexers(&ids) }))
}

async fn test_external(_: State<AppState>, _: Admin, body: Bytes) -> Answer {
    let ext: Option<FilterExternal> = serde_json::from_slice(&body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let Some(ext) = ext else { return Err(ApiError::BadRequest("external filter is required".into()).into()) };
    let http = super::clients::http(std::time::Duration::from_secs(30), false, false);
    match super::external::test_external(&http, &ext).await {
        Ok(r) => send(StatusCode::OK, r),
        Err(e) if e.ends_with(super::external::ERR_TYPE_UNSUPPORTED) => Err(ApiError::BadRequest(e).into()),
        Err(e) => Err(ApiError::Internal(e).into()),
    }
}

async fn store_filter(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let v: Value = decode(&body)?;
    send(StatusCode::CREATED, st.releases.autobrr_store_filter(&v)?)
}

async fn update_filter(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let _ = id_of(&id, "filterID");
    let v: Value = decode(&body)?;
    send(StatusCode::OK, st.releases.autobrr_update_filter(&v)?)
}

async fn update_partial(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "filterID")?;
    let v: Value = decode(&body)?;
    st.releases.autobrr_update_partial(id, &v)?;
    no_content()
}

async fn toggle_filter(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "filterID")?;
    let v: Value = decode(&body)?;
    st.releases.autobrr_toggle_filter(id, v.get("enabled").and_then(Value::as_bool).unwrap_or(false))?;
    no_content()
}

async fn delete_filter(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "filterID")?;
    st.releases.autobrr_delete_filter(id)?;
    no_content()
}

async fn filter_notifications(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "filterID")?;
    st.releases.autobrr_filter(id)?;
    send(StatusCode::OK, st.releases.filter_notifications(id)?)
}

async fn update_notifications(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "filterID")?;
    let list: Vec<FilterNotification> = decode(&body)?;
    st.releases.update_filter_notifications(id, &list)?;
    no_content()
}

// ---- actions ----

async fn actions(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.actions()?)
}

async fn store_action(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let a: Action = decode(&body)?;
    send(StatusCode::CREATED, st.releases.add_action(a)?)
}

async fn update_action(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "actionID")?;
    let mut a: Action = decode(&body)?;
    a.id = id;
    send(StatusCode::OK, st.releases.update_action(a)?)
}

async fn delete_action(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "actionID")?;
    st.releases.remove_action(id)?;
    no_content()
}

async fn toggle_action(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "actionID")?;
    st.releases.toggle_action(id)?;
    no_content()
}

// ---- download clients ----

async fn downloaders(State(st): State<AppState>, _: SignedIn) -> Answer {
    let list: Vec<Value> = st.releases.downloaders()?.iter().map(Downloader::public).collect();
    send(StatusCode::OK, list)
}

async fn downloader(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "clientID")?;
    send(StatusCode::OK, st.releases.downloader(id)?.public())
}

async fn store_downloader(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let d: Downloader = decode(&body)?;
    send(StatusCode::CREATED, st.releases.add_downloader(d)?.public())
}

async fn update_downloader(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let d: Downloader = decode(&body)?;
    send(StatusCode::OK, st.releases.update_downloader(d)?.public())
}

async fn test_downloader(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let mut d: Downloader = decode(&body)?;
    d.validate().map_err(ApiError::Internal)?;
    if d.id > 0 {
        let saved = st.releases.downloader(d.id)?;
        d.keep_secrets(&saved);
    }
    super::run::test_connection(&d).await.map_err(ApiError::Internal)?;
    no_content()
}

async fn delete_downloader(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "clientID")?;
    st.releases.remove_downloader(id)?;
    no_content()
}

async fn arr_tags(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "clientID")?;
    let d = st.releases.downloader(id)?;
    let app = super::clients::arr::App::of(&d.kind)
        .filter(|_| ["RADARR", "SONARR", "WHISPARR", "WHISPARR_V3", "SPORTARR"].contains(&d.kind.as_str()));
    let tags: Vec<ArrTag> = match app {
        Some(app) => super::run::arr_client(app, &d)
            .get_tags()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(id, label)| ArrTag { id, label })
            .collect(),
        None => Vec::new(),
    };
    send(StatusCode::OK, tags)
}
