// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Search/SearchController.cs, SearchResource.cs,
// ReleaseResource.cs, src/NzbDrone.Core/Download/DownloadService.cs (SendReportToClient),
// src/NzbDrone.Core/Download/DownloadClientProvider.cs (GetDownloadClient)), GPL-3.0.

//! Prowlarr's search page and its grab button:
//!
//! ```text
//! GET  /api/v1/search?query=&type=&indexerIds=&categories=&limit=&offset=   releases (ReleaseResource)
//! POST /api/v1/search                 send one release found by a search to a download client
//! POST /api/v1/search/bulk            send several
//! ```
//!
//! The releases a search found are kept for 30 minutes (by indexer and guid), as in Prowlarr, so a grab only
//! names them. A grab goes to Prowlarr's download clients (the indexer's own one when it has one, else the best
//! enabled one for the protocol, taking turns), and the notifications on for grabs hear of it.

use super::indexers::{category_json, normalize_title};
use super::{body_json, check, fail, validation, ApiError, ApiResult, V1};
use crate::indexers::api::{host_from, proxy_link, public_base, query_of, source_from};
use crate::indexers::capabilities::Category;
use crate::indexers::release::{Protocol, Release};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The search routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/search", get(search).post(grab)).route("/api/v1/search/bulk", post(grab_bulk))
}

/// How long a release a search found can be grabbed by name (`TimeSpan.FromMinutes(30)`).
const KEEP_SECONDS: f64 = 30.0 * 60.0;

/// The releases searches found: (data folder, `{indexerId}_{guid}`) → (until when, release).
type Cache = HashMap<(std::path::PathBuf, String), (f64, Release)>;

/// The kept releases of every running Lantern.
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `GetCacheKey`: the indexer's id and the release's guid.
fn cache_key(indexer_id: i64, guid: &str) -> String {
    format!("{indexer_id}_{guid}")
}

/// A category of a release as Prowlarr's `IndexerCategory`: a standard top category with its sub-categories, a
/// standard sub-category, or one of the indexer's own (by number only, as Prowlarr maps it).
fn release_category(id: u32) -> Value {
    if let Some((_, name, subs)) = super::apps::categories::PARENTS.iter().find(|(p, _, _)| *p == id) {
        let mut c = Category::new(id, name);
        c.subs = subs.iter().map(|(s, n)| Category::new(*s, n)).collect();
        return category_json(&c);
    }
    if let Some(name) = crate::indexers::categories::name_of(id) {
        return category_json(&Category::new(id, name));
    }
    // a category of the indexer's own has only its number (`MapTrackerCatToNewznab`)
    json!({ "id": id, "subCategories": [] })
}

/// A release as Prowlarr's `ReleaseResource`, its links through Lantern (`ConvertToProxyLink`).
pub fn release_resource(st: &AppState, base: &str, apikey: &str, r: &Release) -> Value {
    let published = r.date.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
    let age = chrono::Utc::now().signed_duration_since(published);
    let minutes = age.num_milliseconds() as f64 / 60000.0;
    let mut v = Map::new();
    let mut put = |k: &str, x: Value| {
        if !x.is_null() {
            v.insert(k.into(), x);
        }
    };
    let opt = |s: &str| if s.trim().is_empty() { Value::Null } else { json!(s) };
    put("guid", json!(r.guid));
    put("age", json!(age.num_days()));
    put("ageHours", json!(minutes / 60.0));
    put("ageMinutes", json!(minutes));
    put("size", json!(r.size.unwrap_or(0)));
    put("files", json!(r.files));
    put("grabs", json!(r.grabs));
    put("indexerId", json!(r.indexer_id));
    put("indexer", json!(r.indexer));
    put("title", json!(r.title));
    put("sortTitle", json!(normalize_title(&r.title)));
    put("imdbId", json!(r.imdb.unwrap_or(0)));
    put("tmdbId", json!(r.tmdb.unwrap_or(0)));
    put("tvdbId", json!(r.tvdb.unwrap_or(0)));
    put("tvMazeId", json!(r.tvmaze.unwrap_or(0)));
    put("publishDate", json!(published.format("%Y-%m-%dT%H:%M:%SZ").to_string()));
    put("infoUrl", opt(&r.details));
    put("posterUrl", opt(&r.poster));
    put("indexerFlags", json!(r.flags));
    put("categories", Value::Array(r.categories.iter().map(|c| release_category(*c)).collect()));
    if !r.link.trim().is_empty() {
        put("downloadUrl", json!(proxy_link(st, base, apikey, r.indexer_id, &r.link, &r.title)));
    }
    if r.protocol == Protocol::Torrent {
        if !r.magnet.trim().is_empty() {
            put("magnetUrl", json!(proxy_link(st, base, apikey, r.indexer_id, &r.magnet, &r.title)));
        }
        put("infoHash", opt(&r.infohash));
        put("seeders", json!(r.seeders));
        put("leechers", json!(r.leechers));
    }
    put("protocol", json!(if r.protocol == Protocol::Usenet { "usenet" } else { "torrent" }));
    put("fileName", json!(format!("{}{}", r.title, if r.protocol == Protocol::Usenet { ".nzb" } else { ".torrent" })));
    Value::Object(v)
}

/// The query's values as one map (a repeated name's values joined with commas: `categories=2000&categories=5000`).
fn joined(raw: Option<String>) -> HashMap<String, String> {
    let mut q: HashMap<String, String> = HashMap::new();
    for (k, v) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        let e = q.entry(k.to_string()).or_default();
        if !e.is_empty() {
            e.push(',');
        }
        e.push_str(&v);
    }
    q
}

/// `GET /api/v1/search?query=&type=&indexerIds=&categories=&limit=&offset=`: searches the indexers asked for (all
/// without any; `-1` every Usenet one, `-2` every torrent one) as a person does from Prowlarr's search page. 400 when
/// every indexer asked for is unavailable; any other failure gives an empty list, as in Prowlarr.
async fn search(State(st): State<AppState>, _: V1, headers: HeaderMap, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let mut q = joined(raw);
    if !q.contains_key("type") {
        q.insert("type".into(), "search".into());
    }
    let ids: Vec<i64> = q.get("indexerIds").map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
    let Ok(query) = query_of(&q) else { return Ok(Json(json!([]))) };
    let found = match st.indexers.dispatch(&ids, &query, "Prowlarr", &host_from(&headers), true).await {
        Ok(f) => f,
        Err(e) if e.status == 400 => return Err(fail(400, e.detail)),
        Err(e) => {
            tracing::error!("Search failed: {}", e.detail);
            return Ok(Json(json!([])));
        }
    };
    let base = public_base(&st, &headers);
    let apikey = st.settings.str("api_key");
    let now = crate::auth::now();
    {
        let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
        c.retain(|_, (until, _)| *until > now);
        for r in &found.releases {
            c.insert((st.data_dir.clone(), cache_key(r.indexer_id, &r.guid)), (now + KEEP_SECONDS, r.clone()));
        }
    }
    Ok(Json(Value::Array(found.releases.iter().map(|r| release_resource(&st, &base, &apikey, r)).collect())))
}

/// Prowlarr's checks of a release to grab (`PostValidator`): an indexer id above 0 and a guid.
fn check_release(r: &Value) -> ApiResult<()> {
    check(vec![
        validation::greater_than("IndexerId", r.get("indexerId").and_then(Value::as_i64).unwrap_or(0), 0),
        validation::not_empty("Guid", r.get("guid").and_then(Value::as_str)),
    ])
}

/// The kept release a grab names, if it is still kept.
fn kept(st: &AppState, r: &Value) -> Option<Release> {
    let key = cache_key(r.get("indexerId").and_then(Value::as_i64).unwrap_or(0), r.get("guid").and_then(Value::as_str).unwrap_or(""));
    let now = crate::auth::now();
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(st.data_dir.clone(), key))
        .filter(|(until, _)| *until > now)
        .map(|(_, r)| r.clone())
}

/// Why a grab didn't reach a download client.
enum GrabFailure {
    /// The indexer didn't give the release's file (`ReleaseDownloadException`).
    Indexer,
    /// No download client, or the client refused it (`DownloadClientException`), in words.
    Client(ApiError),
}

/// Sends a release to a download client (`DownloadService.SendReportToClient`): `client_id` when given, else the
/// indexer's own client, else Prowlarr's choice among the enabled clients of the release's protocol. The
/// notifications on for grabs hear of it, also when the indexer didn't give the release's file.
async fn send(st: &AppState, r: &Release, source: &str, host: &str, client_id: Option<i64>) -> Result<(), GrabFailure> {
    use super::apps::downloads::{self, DownloadClients};
    use super::apps::notifications::{self, GrabMessage};
    use super::apps::provider;
    let ix = st.indexers.get(r.indexer_id).map_err(|_| GrabFailure::Client(super::not_found("IndexerDefinition", r.indexer_id)))?;
    let protocol = if r.protocol == Protocol::Usenet { "Usenet" } else { "Torrent" };
    let client = match client_id {
        Some(id) => Some(
            provider::find::<DownloadClients>(&st.db, id)
                .map_err(|e| GrabFailure::Client(e.into()))?
                .ok_or_else(|| GrabFailure::Client(super::not_found("DownloadClientDefinition", id)))?,
        ),
        None => downloads::pick(st, r.protocol, ix.download_client_id).map_err(GrabFailure::Client)?,
    };
    let Some(client) = client else {
        return Err(GrabFailure::Client(fail(500, format!("{protocol} Download client isn't configured yet"))));
    };
    let client_type = provider::implementation_of::<DownloadClients>(&client.implementation).map(|i| i.implementation_name).unwrap_or("");
    let manual = source == "Prowlarr";
    let message = |sent: &str, ok: bool| GrabMessage {
        release: r.clone(),
        source: source.to_string(),
        host: host.to_string(),
        successful: ok,
        download_client_name: client.label(),
        download_client_type: client_type.to_string(),
        download_id: sent.to_string(),
        redirect: false,
        manual,
        message: GrabMessage::message_for(r, manual, source, &client.label()),
    };
    let tags = st.indexers.tag_ids(&ix.tags);
    let link = if r.link.is_empty() { &r.magnet } else { &r.link };
    let grabbed = match st.indexers.download(&ix, link, &r.title, source, host).await {
        Ok(g) => g,
        Err(_) => {
            notifications::grabbed(st, &message("", false), &tags).await;
            return Err(GrabFailure::Indexer);
        }
    };
    let sent =
        downloads::send(st, &client, r, grabbed).await.map_err(|e| GrabFailure::Client(fail(500, format!("{}: {e}", client.label()))))?;
    tracing::info!("Report sent to {}. {}", client.label(), r.title);
    notifications::grabbed(st, &message(&sent.download_id, true), &tags).await;
    Ok(())
}

/// `POST /api/v1/search` (a `ReleaseResource`, `downloadClientId` optional): sends a release a search found to a
/// download client. 404 when it is no longer kept, 409 when the indexer didn't give its file. The release back.
async fn grab(State(st): State<AppState>, _: V1, headers: HeaderMap, bytes: Bytes) -> ApiResult<Json<Value>> {
    let body = body_json(&bytes)?;
    check_release(&body)?;
    let Some(r) = kept(&st, &body) else {
        return Err(fail(404, "Couldn't find requested release in cache, try searching again"));
    };
    let client = body.get("downloadClientId").and_then(Value::as_i64).filter(|c| *c != 0);
    match send(&st, &r, &source_from(&headers), &host_from(&headers), client).await {
        Ok(()) => Ok(Json(body)),
        Err(GrabFailure::Indexer) => Err(fail(409, "Getting release from indexer failed")),
        Err(GrabFailure::Client(e)) => Err(e),
    }
}

/// `POST /api/v1/search/bulk` (a list of `ReleaseResource`): sends each release still kept; the ones sent, 400
/// `Failed to grab any release` when none was.
async fn grab_bulk(State(st): State<AppState>, _: V1, headers: HeaderMap, bytes: Bytes) -> ApiResult<Json<Value>> {
    let body = body_json(&bytes)?;
    let list = body.as_array().cloned().ok_or_else(|| fail(400, "The request body must be a list of releases."))?;
    for r in &list {
        check_release(r)?;
    }
    let (source, host) = (source_from(&headers), host_from(&headers));
    let mut sent = Vec::new();
    for r in list {
        let Some(release) = kept(&st, &r) else {
            tracing::error!("Couldn't find requested release in cache, cache timeout probably expired.");
            continue;
        };
        if send(&st, &release, &source, &host, None).await.is_ok() {
            sent.push(r);
        }
    }
    if sent.is_empty() {
        return Err(fail(400, "Failed to grab any release"));
    }
    Ok(Json(Value::Array(sent)))
}
