// Ported from autobrr v1.90.0 (internal/http/filter.go, action.go, downloader.go, irc.go, indexer.go, encoder.go,
// params.go), GPL-2.0-or-later.
//! autobrr's API for filters, actions, download clients, IRC networks and indexers, with autobrr's paths, JSON and status codes. The
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
//! GET    /irc                                the networks with their health
//! POST   /irc                                add a network (or its channels to the same server, port and nick) (204)
//! GET    /irc/events?stream=irc              the live stream: PRIVMSG, STATE and HEALTH events (also /events)
//! GET    /irc/network/{id}                   one network with its channels
//! PUT    /irc/network/{id}                   change (redacted secrets keep the saved ones) (204)
//! DELETE /irc/network/{id}                   remove (204)
//! POST   /irc/network/{id}/cmd               {channel, msg}: say it on the network (204)
//! POST   /irc/network/{id}/channel           add a channel (204)
//! GET    /irc/network/{id}/restart           reconnect (204)
//! GET    /irc/network/{id}/channel/{c}/history          the channel's last 1000 lines
//! POST   /irc/network/{id}/channel/{c}/announce/process {msg}: read it as an announce (204)
//! GET    /indexer/schema                     every definition
//! GET    /indexer                            every indexer's mapped definition
//! POST   /indexer                            add (201, the indexer)
//! GET    /indexer/options                    every indexer (secrets redacted)
//! GET    /indexer/deprecations               the removed definitions and the filters still using them
//! DELETE /indexer/deprecations/{id}          remove an archived indexer (204)
//! GET    /indexer/{id}                       one
//! PUT    /indexer/{id}                       change (200, the indexer)
//! DELETE /indexer/{id}                       remove (204)
//! POST   /indexer/{id}/api/test              test the site API
//! PATCH  /indexer/{id}/enabled               {enabled} (204)
//! ```
//!
//! Errors answer autobrr's `{code, message}` (`RECORD_NOT_FOUND` 404, `BAD_REQUEST_PARAMS` 400,
//! `INTERNAL_SERVER_ERROR` 500), with Lantern's `detail` holding the same message.

use super::action::Action;
use super::downloader::{ArrTag, Downloader};
use super::external::FilterExternal;
use super::indexer::Indexer;
use super::irc::{IrcChannel, IrcError, IrcNetwork, SendCmd};
use super::store::{parse_sort, ApiError, ApiResult, FilterNotification};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
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
        .route("/irc", get(irc_networks).post(store_network))
        .route("/irc/", get(irc_networks).post(store_network))
        .route("/irc/events", get(irc_events))
        .route("/events", get(irc_events))
        .route("/irc/network/{id}", get(irc_network).put(update_network).delete(delete_network))
        .route("/irc/network/{id}/", get(irc_network).put(update_network).delete(delete_network))
        .route("/irc/network/{id}/cmd", post(send_cmd))
        .route("/irc/network/{id}/channel", post(store_channel))
        .route("/irc/network/{id}/restart", get(restart_network))
        .route("/irc/network/{id}/channel/{channel}/history", get(channel_history))
        .route("/irc/network/{id}/channel/{channel}/announce/process", post(announce_process))
        .route("/indexer/schema", get(indexer_schema))
        .route("/indexer", get(indexers_mapped).post(store_indexer))
        .route("/indexer/", get(indexers_mapped).post(store_indexer))
        .route("/indexer/options", get(indexer_options))
        .route("/indexer/deprecations", get(indexer_deprecations))
        .route("/indexer/deprecations/{id}", delete(delete_archived_indexer))
        .route("/indexer/{id}", get(indexer).put(update_indexer).delete(delete_indexer))
        .route("/indexer/{id}/", get(indexer).put(update_indexer).delete(delete_indexer))
        .route("/indexer/{id}/api/test", post(test_indexer_api))
        .route("/indexer/{id}/enabled", patch(toggle_indexer))
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

// ---- IRC ----

/// A not-found answer in autobrr's words.
fn not_found(m: String) -> Fail {
    Fail(ApiError::NotFound(m))
}

/// A record-not-found from the store, as `m`; other failures as they are.
fn or_not_found(e: ApiError, m: impl FnOnce() -> String) -> Fail {
    match e {
        ApiError::NotFound(_) => not_found(m()),
        e => Fail(e),
    }
}

/// A channel path parameter with its `#`.
fn channel_param(c: &str) -> String {
    if c.starts_with('#') {
        c.to_string()
    } else {
        format!("#{c}")
    }
}

async fn irc_networks(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.irc_networks_with_health()?)
}

async fn irc_network(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "networkID")?;
    let n = st.releases.irc_network(id).map_err(|e| or_not_found(e, || format!("network with id {id} not found")))?;
    send(StatusCode::OK, n.public())
}

async fn store_network(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let n: IrcNetwork = decode(&body)?;
    n.auth.validate().map_err(ApiError::BadRequest)?;
    st.releases.store_irc_network(n)?;
    no_content()
}

async fn update_network(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let n: IrcNetwork = decode(&body)?;
    n.auth.validate().map_err(ApiError::BadRequest)?;
    let id = n.id;
    st.releases.update_irc_network(n).map_err(|e| or_not_found(e, || format!("network with id {id} not found")))?;
    no_content()
}

async fn delete_network(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "networkID")?;
    st.releases.delete_irc_network(id).map_err(|e| or_not_found(e, || format!("network with id {id} does not exist")))?;
    no_content()
}

async fn send_cmd(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "networkID")?;
    let mut req: SendCmd = decode(&body)?;
    req.network_id = id;
    match st.releases.send_irc_cmd(&req) {
        Ok(()) => no_content(),
        Err(IrcError::NotFound) => Err(not_found(format!("network with id {id} not found"))),
        Err(e) => Err(Fail(ApiError::Internal(irc_error_text(&e)))),
    }
}

async fn store_channel(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "networkID")?;
    let c: IrcChannel = decode(&body)?;
    st.releases.store_irc_channel(id, c).map_err(|e| or_not_found(e, || format!("network with id {id} not found")))?;
    no_content()
}

async fn restart_network(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "networkID")?;
    st.releases.restart_irc_network(id).map_err(|e| or_not_found(e, || format!("network with id {id} not found")))?;
    no_content()
}

async fn channel_history(State(st): State<AppState>, _: SignedIn, Path((id, channel)): Path<(String, String)>) -> Answer {
    let id = id_of(&id, "networkID")?;
    match st.releases.irc_history(id, &channel_param(&channel)) {
        Ok(list) => send(StatusCode::OK, list),
        Err(IrcError::NotFound) => send(StatusCode::OK, Vec::<Value>::new()),
        Err(e) => Err(Fail(ApiError::Internal(irc_error_text(&e)))),
    }
}

/// The body of a hand-sent announce (autobrr's `IRCManualProcessRequest`).
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ManualAnnounce {
    /// The line.
    msg: String,
}

async fn announce_process(State(st): State<AppState>, _: Admin, Path((id, channel)): Path<(String, String)>, body: Bytes) -> Answer {
    let id = id_of(&id, "networkID")?;
    let req: ManualAnnounce = decode(&body)?;
    let channel = channel_param(&channel);
    match st.releases.manual_process_announce(id, &channel, &req.msg).await {
        Ok(()) => no_content(),
        Err(IrcError::ChannelNotFound) => Err(not_found(format!("channel {channel} not found on network with id {id}"))),
        Err(IrcError::NotFound) => Err(not_found(format!("network with id {id} not found"))),
        Err(IrcError::NoAnnounceProcessor) => {
            Err(Fail(ApiError::BadRequest(format!("channel {channel} is not an indexer announce channel on network with id {id}"))))
        }
        Err(e) => Err(Fail(ApiError::Internal(irc_error_text(&e)))),
    }
}

/// An IRC failure in autobrr's words.
fn irc_error_text(e: &IrcError) -> String {
    match e {
        IrcError::NotFound => "record not found".into(),
        IrcError::ChannelNotFound => "irc channel not found".into(),
        IrcError::NoAnnounceProcessor => "irc channel has no announce processor".into(),
        IrcError::Other(m) => m.clone(),
    }
}

/// The live stream (r3labs/sse as autobrr serves it): `?stream=irc` gets every PRIVMSG, STATE and HEALTH
/// event from now on; no stream, or another one, is refused as r3labs refuses it.
async fn irc_events(State(st): State<AppState>, _: SignedIn, RawQuery(q): RawQuery) -> Response {
    use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
    let stream = url::form_urlencoded::parse(q.unwrap_or_default().as_bytes()).find(|(k, _)| k == "stream").map(|(_, v)| v.into_owned());
    match stream.as_deref() {
        None | Some("") => (StatusCode::INTERNAL_SERVER_ERROR, "Please specify a stream!\n").into_response(),
        Some("irc") => {
            let rx = st.releases.irc.subscribe();
            let events = futures_util::stream::unfold(rx, |mut rx| async move {
                loop {
                    match rx.recv().await {
                        Ok(e) => return Some((Ok::<_, std::convert::Infallible>(SseEvent::default().event(e.event).data(e.data)), rx)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    }
                }
            });
            let mut r = Sse::new(events).keep_alive(KeepAlive::default()).into_response();
            r.headers_mut().insert("x-accel-buffering", axum::http::HeaderValue::from_static("no"));
            r
        }
        Some(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Stream not found!\n").into_response(),
    }
}

// ---- indexers ----

async fn indexer_schema(State(st): State<AppState>, _: SignedIn) -> Answer {
    let list: Vec<Value> = st.releases.indexer_templates().iter().map(|d| serde_json::to_value(&**d).unwrap_or_default()).collect();
    send(StatusCode::OK, list)
}

async fn indexers_mapped(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.mapped_definitions()?)
}

async fn indexer_options(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.autobrr_indexers()?)
}

async fn indexer_deprecations(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.indexer_deprecations()?)
}

async fn delete_archived_indexer(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "indexerID")?;
    st.releases.delete_archived_indexer(id).map_err(|e| or_not_found(e, || format!("archived indexer with id {id} not found")))?;
    no_content()
}

async fn indexer(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "indexerID")?;
    send(StatusCode::OK, st.releases.autobrr_indexer(id).map_err(|e| or_not_found(e, || format!("indexer with id {id} not found")))?)
}

async fn store_indexer(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let i: Indexer = decode(&body)?;
    send(StatusCode::CREATED, st.releases.store_indexer(i)?)
}

async fn update_indexer(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let i: Indexer = decode(&body)?;
    let id = i.id;
    send(StatusCode::OK, st.releases.update_indexer(i).map_err(|e| or_not_found(e, || format!("indexer with id {id} not found")))?)
}

async fn delete_indexer(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "indexerID")?;
    st.releases.delete_indexer(id).map_err(|e| or_not_found(e, || format!("indexer with id {id} not found")))?;
    no_content()
}

async fn test_indexer_api(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "indexerID")?;
    let req: Value = decode(&body)?;
    let target = req.get("id").and_then(Value::as_i64).filter(|i| *i != 0).unwrap_or(id);
    st.releases.test_indexer_api(target).map_err(|e| or_not_found(e, || format!("indexer with id {id} not found")))?;
    send(StatusCode::OK, json!({ "message": "Indexer api test OK" }))
}

/// The body of `PATCH /indexer/{id}/enabled`.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Enabled {
    /// On or off.
    enabled: bool,
}

async fn toggle_indexer(State(st): State<AppState>, _: Admin, Path(id): Path<String>, body: Bytes) -> Answer {
    let id = id_of(&id, "indexerID")?;
    let e: Enabled = decode(&body)?;
    st.releases.toggle_indexer(id, e.enabled).map_err(|e| or_not_found(e, || format!("indexer with id {id} not found")))?;
    no_content()
}
