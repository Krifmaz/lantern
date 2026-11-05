//! The download hub's routes. Apps use Lantern's API key (`X-Api-Key`, or `?apikey=` for the event stream); the
//! web app uses its session. Changes need an admin.
//!
//! ```text
//! Download clients:
//!   GET    /api/hub/clients               the clients (secrets masked) and the kinds Lantern knows
//!   POST   /api/hub/clients               add {kind, name, url, username, password, api_key, root, enabled, priority}
//!   GET    /api/hub/clients/{id}          one
//!   PUT    /api/hub/clients/{id}          change (a masked secret keeps the saved one)
//!   DELETE /api/hub/clients/{id}          remove
//!   POST   /api/hub/clients/{id}/test     sign in and ask the version: {ok, message}
//! Apps:
//!   GET    /api/hub/apps                  every app with its category, folders and grab counts
//!   POST   /api/hub/apps                  register (or update by name) {name, media, import_path, callback_url}
//!   GET    /api/hub/apps/{id}             one
//!   PUT    /api/hub/apps/{id}             change {media, import_path, callback_url}
//!   DELETE /api/hub/apps/{id}             remove (its downloads stay in the clients)
//! Searching and grabbing:
//!   GET    /api/hub/search?app=&q=&type=&cat=   search every indexer: releases with a grab `token`
//!   POST   /api/hub/grabs                 grab {app, token | url, title, protocol}
//!   GET    /api/hub/grabs?app=&state=&limit=    the newest grabs
//!   GET    /api/hub/grabs/{id}            one
//!   POST   /api/hub/grabs/{id}/retry      send a failed grab again
//!   DELETE /api/hub/grabs/{id}?delete_files=    stop it and remove it from its client
//!   GET    /api/hub/events?app=           the live event stream (server-sent events)
//! ```

use super::apps::App;
use super::grabs::{protocol_name, token};
use crate::AppState;
use axum::extract::{Path, Query as Q, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result, SignedIn};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The hub's routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/hub/clients", get(clients).post(add_client))
        .route("/api/hub/clients/{id}", get(client).put(change_client).delete(remove_client))
        .route("/api/hub/clients/{id}/test", post(test_client))
        .route("/api/hub/apps", get(apps).post(register))
        .route("/api/hub/apps/{id}", get(app).put(change_app).delete(remove_app))
        .route("/api/hub/search", get(search))
        .route("/api/hub/grabs", get(grabs).post(grab))
        .route("/api/hub/grabs/{id}", get(one_grab).delete(remove_grab))
        .route("/api/hub/grabs/{id}/retry", post(retry))
        .route("/api/hub/events", get(events))
}

async fn clients(State(st): State<AppState>, _: SignedIn) -> Result<Json<Value>> {
    let list: Vec<Value> = st.hub.clients()?.iter().map(|c| st.hub.client_json(c)).collect();
    let kinds: Vec<Value> = super::clients::KINDS
        .iter()
        .map(|(k, p, n)| json!({ "kind": k, "protocol": p, "name": n, "default": super::clients::DEFAULTS.contains(k) }))
        .collect();
    Ok(Json(json!({ "clients": list, "kinds": kinds })))
}

async fn add_client(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let id = st.hub.add_client(&body)?;
    Ok(Json(st.hub.client_json(&st.hub.client(id)?)))
}

async fn client(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.hub.client_json(&st.hub.client(id)?)))
}

async fn change_client(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>> {
    st.hub.update_client(id, &body)?;
    Ok(Json(st.hub.client_json(&st.hub.client(id)?)))
}

async fn remove_client(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    st.hub.remove_client(id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_client(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.hub.test_client(id).await?))
}

async fn apps(State(st): State<AppState>, _: SignedIn) -> Result<Json<Value>> {
    let list: Vec<Value> = st.hub.apps()?.iter().map(|a| st.hub.app_json(a)).collect();
    Ok(Json(json!({ "apps": list, "downloads_root": st.hub.root() })))
}

async fn register(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let (app, problem) = st.hub.register(&body)?;
    let mut v = st.hub.app_json(&app);
    v["folder_error"] = json!(problem);
    Ok(Json(v))
}

async fn app(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.hub.app_json(&st.hub.app(id)?)))
}

async fn change_app(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>> {
    Ok(Json(st.hub.app_json(&st.hub.update_app(id, &body)?)))
}

async fn remove_app(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    st.hub.remove_app(id)?;
    Ok(Json(json!({ "ok": true })))
}

/// The app a request names in `app`, when it names one.
fn app_of(st: &AppState, q: &HashMap<String, String>) -> Result<Option<App>> {
    match q.get("app").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(a) => {
            let app = st.hub.app_named(a)?;
            st.hub.seen(app.id);
            Ok(Some(app))
        }
        None => Ok(None),
    }
}

async fn search(State(st): State<AppState>, _: SignedIn, Q(mut q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    let app = app_of(&st, &q)?;
    if let Some(a) = &app {
        if !q.contains_key("cat") && !q.contains_key("categories") {
            let cats = a.categories().iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",");
            q.insert("cat".into(), cats);
        }
    }
    let query = crate::indexers::api::query_of(&q).map_err(|(_, m)| err(400, m))?;
    let ids: Vec<i64> = q.get("ids").map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
    let source = app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Lantern".into());
    let (releases, failed) = st.indexers.search(&ids, &query, &source).await?;
    let results: Vec<Value> = releases
        .iter()
        .map(|r| {
            let link = if r.link.is_empty() { &r.magnet } else { &r.link };
            json!({
                "title": r.title, "indexer": r.indexer, "indexer_id": r.indexer_id, "protocol": protocol_name(r.protocol),
                "size": r.size, "seeders": r.seeders, "leechers": r.leechers, "grabs": r.grabs,
                "date": r.date.map(|d| d.to_rfc3339()), "categories": r.categories, "freeleech": r.freeleech(),
                "infohash": r.infohash, "details": r.details, "imdb": r.imdb, "tvdb": r.tvdb, "tmdb": r.tmdb,
                "token": token(&st.signer, r.indexer_id, link),
            })
        })
        .collect();
    let failed: Vec<Value> = failed.iter().map(|(n, e)| json!({ "indexer": n, "error": e })).collect();
    Ok(Json(json!({ "results": results, "failed": failed })))
}

async fn grab(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let which = body.get("app").map(|a| a.as_str().map(str::to_string).unwrap_or_else(|| a.to_string())).unwrap_or_default();
    let app = st.hub.app_named(&which)?;
    let g = st.hub.grab(&app, &body)?;
    // send it now rather than at the next step
    let hub = st.hub.clone();
    tokio::spawn(async move { hub.tick().await });
    Ok(Json(st.hub.grab_json(&g, &app)))
}

async fn grabs(State(st): State<AppState>, _: SignedIn, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    let app = app_of(&st, &q)?;
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100);
    let list = st.hub.grabs(app.as_ref().map(|a| a.id), q.get("state").map(String::as_str), limit)?;
    let mut apps: HashMap<i64, App> = HashMap::new();
    let mut out = Vec::new();
    for g in list {
        if !apps.contains_key(&g.app_id) {
            if let Ok(a) = st.hub.app(g.app_id) {
                apps.insert(a.id, a);
            }
        }
        if let Some(a) = apps.get(&g.app_id) {
            out.push(st.hub.grab_json(&g, a));
        }
    }
    Ok(Json(json!({ "grabs": out })))
}

async fn one_grab(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    let g = st.hub.get_grab(id)?;
    Ok(Json(st.hub.grab_json(&g, &st.hub.app(g.app_id)?)))
}

async fn retry(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    let g = st.hub.retry(id)?;
    let hub = st.hub.clone();
    tokio::spawn(async move { hub.tick().await });
    Ok(Json(st.hub.grab_json(&g, &st.hub.app(g.app_id)?)))
}

async fn remove_grab(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    let delete = q.get("delete_files").is_some_and(|v| v == "true" || v == "1");
    let g = st.hub.remove_grab(id, delete).await?;
    Ok(Json(st.hub.grab_json(&g, &st.hub.app(g.app_id)?)))
}

async fn events(
    State(st): State<AppState>,
    signed: std::result::Result<SignedIn, kit_server_core::Error>,
    Q(q): Q<HashMap<String, String>>,
) -> Result<Sse<impl futures_util::Stream<Item = std::result::Result<SseEvent, std::convert::Infallible>>>> {
    // an EventSource can't send headers, so the key may come as ?apikey=
    let key = st.settings.str("api_key");
    let by_query = q.get("apikey").is_some_and(|k| !key.is_empty() && crate::auth::ct_eq(k, &key));
    if signed.is_err() && !by_query {
        return Err(err(401, "The API key is missing or wrong"));
    }
    let only = app_of(&st, &q)?.map(|a| a.id);
    let rx = st.hub.events.subscribe();
    let stream = futures_util::stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(e) if only.is_none_or(|a| a == e.app_id) => {
                    let ev = SseEvent::default().event(e.event.clone()).json_data(&e).unwrap_or_default();
                    return Some((Ok(ev), rx));
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
