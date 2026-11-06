// Ported from autobrr v1.90.0 (internal/http/list.go, webhook.go), GPL-2.0-or-later.
//! autobrr's list routes and list webhooks, relative to its `/api` (Lantern serves them under `/autobrr/api`):
//!
//! ```text
//! GET    /lists                          every list with its filters
//! POST   /lists                          add, refreshed at once when on (201, the list)
//! POST   /lists/refresh                  refresh every list that is on (204)
//! POST   /lists/{id}/refresh             refresh one (204)
//! PUT    /lists/{id}                     change, refreshed at once when on (204)
//! DELETE /lists/{id}                     remove (204)
//! GET|POST /webhook/lists/trigger        refresh every list that is on, in the background (204)
//! GET|POST /webhook/lists/trigger/arr    refresh the *arr lists (204)
//! GET|POST /webhook/lists/trigger/lists  refresh the web lists (204)
//! POST   /webhook/lists/trigger/{id}     refresh one (204)
//! ```

use super::autobrr_api::{decode, id_of, no_content, or_not_found, send, Answer, Fail};
use super::list::List;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Router;
use kit_server_core::{Admin, SignedIn};

/// The list and list webhook routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/lists", get(list).post(store))
        .route("/lists/", get(list).post(store))
        .route("/lists/refresh", post(refresh_all))
        .route("/lists/{id}/refresh", post(refresh_one))
        .route("/lists/{id}", put(update).delete(remove))
        .route("/lists/{id}/", put(update).delete(remove))
        .route("/webhook/lists/trigger", get(hook_all).post(hook_all))
        .route("/webhook/lists/trigger/arr", get(hook_arr).post(hook_arr))
        .route("/webhook/lists/trigger/lists", get(hook_other).post(hook_other))
        .route("/webhook/lists/trigger/{id}", post(hook_one))
}

/// autobrr's not-found answer for a list.
fn missing() -> String {
    "could not find list".into()
}

/// A path's list id, or autobrr's `listID parameter is invalid`.
fn list_id(raw: &str) -> Result<i64, Fail> {
    Ok(id_of(raw, "listID")?)
}

async fn list(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.lists(false)?)
}

async fn store(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let l: List = decode(&body)?;
    send(StatusCode::CREATED, st.releases.store_list(l).await?)
}

async fn update(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let l: List = decode(&body)?;
    st.releases.update_list(l).await.map_err(|e| or_not_found(e, missing))?;
    no_content()
}

async fn remove(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = list_id(&id)?;
    st.releases.delete_list(id).map_err(|e| or_not_found(e, missing))?;
    no_content()
}

async fn refresh_all(State(st): State<AppState>, _: Admin) -> Answer {
    st.releases.refresh_all_lists().await?;
    no_content()
}

async fn refresh_one(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = list_id(&id)?;
    st.releases.refresh_list(id).await.map_err(|e| or_not_found(e, missing))?;
    no_content()
}

async fn hook_all(State(st): State<AppState>, _: Admin) -> Answer {
    let rel = st.releases.clone();
    tokio::spawn(async move {
        if let Err(e) = rel.refresh_all_lists().await {
            tracing::warn!("lists: not every list could be refreshed: {}", e.message());
        }
    });
    no_content()
}

async fn hook_arr(State(st): State<AppState>, _: Admin) -> Answer {
    st.releases.refresh_arr_lists().await?;
    no_content()
}

async fn hook_other(State(st): State<AppState>, _: Admin) -> Answer {
    st.releases.refresh_other_lists().await?;
    no_content()
}

async fn hook_one(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = list_id(&id)?;
    st.releases.refresh_list(id).await?;
    no_content()
}
