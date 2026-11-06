// Ported from autobrr v1.90.0 (internal/http/notification.go), GPL-2.0-or-later.
//! autobrr's notification routes, relative to its `/api` (Lantern serves them under `/autobrr/api`):
//!
//! ```text
//! GET    /notification                   every notification (secrets redacted), with the filters using it
//! POST   /notification                   add (201, the notification)
//! POST   /notification/test              send the test events it wants (204)
//! GET    /notification/pushover/sounds   ?token=: Pushover's sounds for an app token
//! GET    /notification/inbox             ?limit=50&offset=0&unread=true&event=…: the built-in inbox
//! DELETE /notification/inbox             empty the inbox (204)
//! POST   /notification/inbox/read        {ids}: mark read (none: all) (204)
//! POST   /notification/inbox/delete      {ids}: remove (204)
//! GET    /notification/{id}              one
//! PUT    /notification/{id}              change (200, the notification)
//! DELETE /notification/{id}              remove (204)
//! ```

use super::autobrr_api::{decode, id_of, no_content, or_not_found, send, Answer};
use super::history_api::query_int;
use super::notify::Notification;
use super::store::ApiError;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use kit_server_core::{Admin, SignedIn};
use serde_json::Value;

/// The notification and inbox routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notification", get(list).post(store))
        .route("/notification/", get(list).post(store))
        .route("/notification/test", post(test))
        .route("/notification/pushover/sounds", get(pushover_sounds))
        .route("/notification/inbox", get(find_inbox).delete(delete_inbox))
        .route("/notification/inbox/", get(find_inbox).delete(delete_inbox))
        .route("/notification/inbox/read", post(mark_read))
        .route("/notification/inbox/delete", post(delete_messages))
        .route("/notification/{id}", get(one).put(update).delete(remove))
        .route("/notification/{id}/", get(one).put(update).delete(remove))
}

/// The query's pairs, in order.
fn pairs(q: Option<&str>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.unwrap_or("").as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

/// autobrr's not-found words for notification `id`.
fn missing(id: i64) -> impl FnOnce() -> String {
    move || format!("notification with id {id} not found")
}

async fn list(State(st): State<AppState>, _: SignedIn) -> Answer {
    let list: Vec<Value> = st.releases.notifications()?.iter().map(Notification::public).collect();
    send(StatusCode::OK, list)
}

async fn store(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let n: Notification = decode(&body)?;
    let n = st.releases.store_notification(n)?;
    send(StatusCode::CREATED, n.public())
}

async fn one(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "notificationID")?;
    let n = st.releases.notification(id).map_err(|e| or_not_found(e, missing(id)))?;
    send(StatusCode::OK, n.public())
}

async fn update(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let n: Notification = decode(&body)?;
    let id = n.id;
    let n = st.releases.update_notification(n).map_err(|e| or_not_found(e, missing(id)))?;
    send(StatusCode::OK, n.public())
}

async fn remove(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "notificationID")?;
    st.releases.delete_notification(id).map_err(|e| or_not_found(e, missing(id)))?;
    no_content()
}

async fn test(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let n: Notification = decode(&body)?;
    st.releases.test_notification(n).await?;
    no_content()
}

async fn pushover_sounds(State(st): State<AppState>, _: SignedIn, RawQuery(q): RawQuery) -> Answer {
    let q = pairs(q.as_deref());
    let token = q.iter().find(|(k, _)| k == "token").map(|(_, v)| v.clone()).unwrap_or_default();
    if token.is_empty() {
        return Err(ApiError::BadRequest("token parameter is required".into()).into());
    }
    let sounds = st.releases.notifier.senders.pushover_sounds(&token).await.map_err(ApiError::Internal)?;
    send(StatusCode::OK, sounds)
}

async fn find_inbox(State(st): State<AppState>, _: SignedIn, RawQuery(q): RawQuery) -> Answer {
    let q = pairs(q.as_deref());
    let limit = query_int(&q, "limit", 50)?.max(1);
    let offset = query_int(&q, "offset", 0)?;
    let unread = q.iter().find(|(k, _)| k == "unread").is_some_and(|(_, v)| v == "true");
    let events: Vec<String> = q.iter().filter(|(k, _)| k == "event").map(|(_, v)| v.clone()).collect();
    send(StatusCode::OK, st.releases.find_inbox(limit, offset, unread, &events)?)
}

/// The `ids` of a body (`{"ids": [1, 2]}`).
fn ids_of(body: &Bytes, empty_ok: bool) -> Result<Vec<i64>, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        if empty_ok {
            return Ok(Vec::new());
        }
        return Err(ApiError::BadRequest("EOF".into()));
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    match v.get("ids") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(ids) => serde_json::from_value(ids.clone()).map_err(|e| ApiError::BadRequest(e.to_string())),
    }
}

async fn mark_read(State(st): State<AppState>, _: SignedIn, body: Bytes) -> Answer {
    let ids = ids_of(&body, true)?;
    st.releases.mark_inbox_read(&ids)?;
    no_content()
}

async fn delete_messages(State(st): State<AppState>, _: SignedIn, body: Bytes) -> Answer {
    let ids = ids_of(&body, false)?;
    if ids.is_empty() {
        return Err(ApiError::BadRequest("ids is required".into()).into());
    }
    st.releases.delete_inbox_messages(&ids)?;
    no_content()
}

async fn delete_inbox(State(st): State<AppState>, _: SignedIn) -> Answer {
    st.releases.delete_inbox()?;
    no_content()
}

