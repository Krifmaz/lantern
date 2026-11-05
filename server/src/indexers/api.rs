//! The indexer routes.
//!
//! ```text
//! The web app (signed in; changes need an admin):
//!   GET    /api/indexers                     the indexers (secrets masked)
//!   POST   /api/indexers                     add {kind, definition, name, base_url, settings, enabled, priority, tags}
//!   GET    /api/indexers/{id}                one
//!   PUT    /api/indexers/{id}                change (a masked secret keeps the saved one)
//!   DELETE /api/indexers/{id}                remove
//!   POST   /api/indexers/{id}/test           sign in and search: {ok, message}
//!   GET    /api/indexers/definitions?q=      the definitions to add from
//!   GET    /api/indexers/definitions/{id}    one definition's settings, links and categories
//!   POST   /api/indexers/definitions/update  download the newest definitions now
//!   GET    /api/indexers/catalog?q=&protocol=&privacy=&language=  torrent sites and Usenet indexers to add, with counts
//!   GET    /api/indexers/presets             the Usenet indexers with a known address, API path and categories
//!   GET    /api/indexers/stats?days=         queries, grabs, failures, answer time per indexer
//!   GET    /api/indexers/history?id=&limit=  the latest queries and grabs
//!   GET    /api/search?q=&type=&cat=&ids=    search: {results, failed}
//!
//! Apps (the API key as ?apikey= or X-Api-Key):
//!   GET    /{id}/api?t=caps|search|tvsearch|movie|music|book…   Torznab/Newznab, one indexer ("all": every one)
//!   GET    /api/v2.0/indexers/{id}/results/torznab/api?…        the same, at Jackett's address
//!   GET    /{id}/download?link=&sig=&file=                      a release's file, through Lantern
//!   GET    /api/v1/…                                            the Prowlarr API (system/status, indexer, search, indexerstats, tag, health)
//! ```

use super::cardigann::Grabbed;
use super::release::{Protocol, Query, Release, SearchKind};
use super::{categories, newznab, torznab, Indexer};
use crate::AppState;
use axum::extract::{Path, Query as Q, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result, SignedIn};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The indexer routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/indexers", get(list).post(add))
        .route("/api/indexers/definitions", get(definitions))
        .route("/api/indexers/definitions/update", post(update_definitions))
        .route("/api/indexers/definitions/{def}", get(definition))
        .route("/api/indexers/catalog", get(catalog))
        .route("/api/indexers/presets", get(presets))
        .route("/api/indexers/stats", get(stats))
        .route("/api/indexers/history", get(history))
        .route("/api/indexers/{id}", get(one).put(change).delete(remove))
        .route("/api/indexers/{id}/test", post(test))
        .route("/api/search", get(search))
        .route("/{id}/api", get(torznab_api))
        .route("/{id}/download", get(download))
        .route("/api/v2.0/indexers/{id}/results/torznab", get(torznab_api))
        .route("/api/v2.0/indexers/{id}/results/torznab/api", get(torznab_api))
        .route("/api/v1/indexer/{id}/newznab", get(torznab_api))
        .route("/api/v1/indexer/{id}/download", get(download))
        .route("/api/v1/system/status", get(v1_status))
        .route("/api/v1/health", get(v1_empty))
        .route("/api/v1/tag", get(v1_empty))
        .route("/api/v1/indexer", get(v1_indexers))
        .route("/api/v1/indexer/{id}", get(v1_indexer))
        .route("/api/v1/indexerstats", get(v1_stats))
        .route("/api/v1/search", get(v1_search))
}

async fn list(State(st): State<AppState>, _: SignedIn) -> Result<Json<Value>> {
    let list: Vec<Value> = st.indexers.list()?.iter().map(|i| st.indexers.public(i)).collect();
    Ok(Json(json!({ "indexers": list, "definitions": st.indexers.library.len() })))
}

async fn add(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let id = st.indexers.add(&body)?;
    Ok(Json(st.indexers.public(&st.indexers.get(id)?)))
}

async fn one(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.indexers.public(&st.indexers.get(id)?)))
}

async fn change(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>> {
    st.indexers.update(id, &body)?;
    Ok(Json(st.indexers.public(&st.indexers.get(id)?)))
}

async fn remove(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    st.indexers.remove(id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn test(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Json<Value> {
    Json(match st.indexers.test(id).await {
        Ok(0) => json!({ "ok": true, "message": "Signed in and searched: it answered, with nothing new right now" }),
        Ok(n) => json!({ "ok": true, "message": format!("Works: {n} results from the newest releases") }),
        Err(e) => json!({ "ok": false, "message": e.detail }),
    })
}

async fn definitions(State(st): State<AppState>, _: SignedIn, Q(q): Q<HashMap<String, String>>) -> Json<Value> {
    let words = q.get("q").map(|s| s.to_lowercase()).unwrap_or_default();
    let list: Vec<Value> = st
        .indexers
        .library
        .all()
        .iter()
        .filter(|d| words.is_empty() || d.name.to_lowercase().contains(&words) || d.id.contains(&words) || d.description.to_lowercase().contains(&words))
        .map(|d| json!({ "id": d.id, "name": d.name, "description": d.description, "language": d.language, "privacy": d.kind, "links": d.links }))
        .collect();
    Json(json!({ "definitions": list, "errors": st.indexers.library.errors().len() }))
}

async fn definition(State(st): State<AppState>, _: SignedIn, Path(def): Path<String>) -> Result<Json<Value>> {
    let d = st.indexers.library.get(&def).ok_or_else(|| err(404, "There is no indexer definition with that id"))?;
    let settings: Vec<Value> = d
        .settings
        .iter()
        .map(|s| {
            json!({
                "name": s.name, "type": s.kind, "label": s.label, "default": s.default, "defaults": s.defaults,
                "options": s.options.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(), "secret": s.is_secret(),
            })
        })
        .collect();
    let cats: Vec<Value> = d.category_map().iter().map(|m| json!({ "id": m.id, "cat": m.cat, "desc": m.desc })).collect();
    Ok(Json(json!({
        "id": d.id, "name": d.name, "description": d.description, "language": d.language, "privacy": d.kind,
        "links": d.links, "settings": settings, "categories": cats, "modes": d.caps.modes,
        "login": d.login.as_ref().map(|l| l.method.clone()),
    })))
}

async fn update_definitions(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    let source = st.settings.str("definitions_source");
    let n = st
        .indexers
        .library
        .update(if source.is_empty() { super::library::DEFAULT_SOURCE } else { &source })
        .await
        .map_err(|e| err(502, e))?;
    Ok(Json(json!({ "ok": true, "count": n, "message": format!("{n} indexer definitions updated") })))
}

async fn presets(_: SignedIn) -> Json<Value> {
    let list: Vec<Value> = newznab::PRESETS
        .iter()
        .map(|p| {
            json!({
                "id": p.id, "name": p.name, "url": p.url, "api_path": p.api_path, "categories": p.categories,
                "privacy": p.privacy, "language": p.language, "description": p.description,
            })
        })
        .collect();
    Json(json!({ "presets": list }))
}

/// Everything that can be added, narrowed by `q`, `protocol`, `privacy` and `language`; the counts are of all.
async fn catalog(State(st): State<AppState>, _: SignedIn, Q(q): Q<HashMap<String, String>>) -> Json<Value> {
    let all = super::catalog::sites(&st.indexers.library.all());
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    let f = super::catalog::Filter { words: get("q"), protocol: get("protocol"), privacy: get("privacy"), language: get("language") };
    let shown: Vec<Value> = all.iter().filter(|s| f.wants(s)).map(super::catalog::Site::json).collect();
    Json(json!({ "sites": shown, "counts": super::catalog::counts(&all), "errors": st.indexers.library.errors().len() }))
}

async fn stats(State(st): State<AppState>, _: SignedIn, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    let days = q.get("days").and_then(|d| d.parse().ok()).unwrap_or(7.0);
    Ok(Json(json!({ "stats": st.indexers.stats(days)? })))
}

async fn history(State(st): State<AppState>, _: SignedIn, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    let id = q.get("id").and_then(|d| d.parse().ok());
    let limit = q.get("limit").and_then(|d| d.parse().ok()).unwrap_or(100).clamp(1, 1000);
    Ok(Json(json!({ "history": st.indexers.history(id, limit)? })))
}

/// The search a request asks for (Torznab parameters, or the web app's and Prowlarr's names for them).
pub fn query_of(q: &HashMap<String, String>) -> std::result::Result<Query, (u16, String)> {
    let g = |k: &str| q.get(k).map(|s| s.trim().to_string()).unwrap_or_default();
    let t = if q.contains_key("t") { g("t") } else { g("type") };
    let kind = SearchKind::parse(&t).ok_or_else(|| (202u16, format!("Lantern doesn't know the search type {t}")))?;
    let cats = if q.contains_key("cat") { g("cat") } else { g("categories") };
    let keywords = if q.contains_key("q") { g("q") } else { g("query") };
    let categories = cats.split(',').filter_map(|c| c.trim().parse().ok()).collect();
    let mut query = Query {
        kind,
        keywords,
        categories,
        imdbid: g("imdbid"),
        tvdbid: g("tvdbid"),
        tmdbid: g("tmdbid"),
        tvmazeid: g("tvmazeid"),
        traktid: g("traktid"),
        doubanid: g("doubanid"),
        season: g("season"),
        ep: g("ep"),
        artist: g("artist"),
        album: g("album"),
        label: g("label"),
        track: g("track"),
        genre: g("genre"),
        author: g("author"),
        title: g("title"),
        year: g("year"),
        limit: g("limit").parse().unwrap_or(0),
        offset: g("offset").parse().unwrap_or(0),
        rss: false,
    };
    // Prowlarr's `{ImdbId:tt123}` style tokens in the query
    if query.keywords.contains('{') {
        let re = regex::Regex::new(r"\{(\w+):([^}]*)\}").expect("a fixed pattern");
        let text = query.keywords.clone();
        for c in re.captures_iter(&text) {
            let v = c[2].to_string();
            match c[1].to_lowercase().as_str() {
                "imdbid" => query.imdbid = v,
                "tvdbid" => query.tvdbid = v,
                "tmdbid" => query.tmdbid = v,
                "season" => query.season = v,
                "episode" => query.ep = v,
                _ => {}
            }
        }
        query.keywords = re.replace_all(&text, "").trim().to_string();
    }
    query.rss = query.keywords.is_empty()
        && query.imdbid.is_empty()
        && query.tvdbid.is_empty()
        && query.tmdbid.is_empty()
        && query.artist.is_empty()
        && query.author.is_empty()
        && query.title.is_empty();
    Ok(query)
}

/// Whether the request carries the API key (`?apikey=` or `X-Api-Key`).
fn key_ok(st: &AppState, headers: &HeaderMap, q: &HashMap<String, String>) -> bool {
    let want = st.settings.str("api_key");
    if want.is_empty() {
        return false;
    }
    let given =
        q.get("apikey").cloned().or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(str::to_string)).unwrap_or_default();
    crate::auth::ct_eq(&given, &want)
}

/// The address apps reached Lantern at (`https://lantern.lan`), from the `public_url` setting or the request.
pub fn public_base(st: &AppState, headers: &HeaderMap) -> String {
    let set = st.settings.str("public_url");
    if !set.trim().is_empty() {
        return set.trim().trim_end_matches('/').to_string();
    }
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let host = h("x-forwarded-host").or_else(|| h("host")).unwrap_or_else(|| format!("localhost:{}", crate::port()));
    let proto = h("x-forwarded-proto").unwrap_or_else(|| "http".into());
    let prefix = h("x-forwarded-prefix").unwrap_or_default();
    format!("{proto}://{host}{}", prefix.trim_end_matches('/'))
}

/// The address Lantern hands out for a release's file.
fn download_link(st: &AppState, base: &str, apikey: &str, r: &Release) -> String {
    if r.protocol == Protocol::Torrent && r.link.starts_with("magnet:") {
        return r.link.clone();
    }
    let (link, sig) = st.signer.sign(r.indexer_id, &r.link);
    let file: String = url::form_urlencoded::byte_serialize(r.title.as_bytes()).collect();
    format!(
        "{base}/{}/download?apikey={}&link={link}&sig={sig}&file={file}",
        r.indexer_id,
        url::form_urlencoded::byte_serialize(apikey.as_bytes()).collect::<String>()
    )
}

fn xml(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")], body).into_response()
}

/// Which indexers a path id names: `all` (every one) or a number. The error is the Torznab error answer
/// itself, which is only made once per request, so its size doesn't matter.
#[allow(clippy::result_large_err)]
fn ids_of(st: &AppState, id: &str) -> std::result::Result<(Vec<i64>, Option<Indexer>), Response> {
    if id == "all" || id == "-1" {
        return Ok((Vec::new(), None));
    }
    let n: i64 = id.parse().map_err(|_| xml(StatusCode::NOT_FOUND, torznab::error_xml(201, "There is no indexer with that id")))?;
    let ix = st.indexers.get(n).map_err(|_| xml(StatusCode::NOT_FOUND, torznab::error_xml(201, "There is no indexer with that id")))?;
    Ok((vec![n], Some(ix)))
}

async fn torznab_api(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Q(q): Q<HashMap<String, String>>) -> Response {
    if !key_ok(&st, &headers, &q) {
        return xml(StatusCode::UNAUTHORIZED, torznab::error_xml(100, "The API key is missing or wrong"));
    }
    let (ids, ix) = match ids_of(&st, &id) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let t = q.get("t").map(String::as_str).unwrap_or("search");
    if t == "caps" {
        let caps = match &ix {
            Some(ix) => st.indexers.caps(ix),
            None => match st.indexers.caps_all() {
                Ok(c) => c,
                Err(e) => return xml(StatusCode::INTERNAL_SERVER_ERROR, torznab::error_xml(900, &e.detail)),
            },
        };
        return xml(StatusCode::OK, torznab::caps_xml(&caps));
    }
    let query = match query_of(&q) {
        Ok(q) => q,
        Err((code, msg)) => return xml(StatusCode::BAD_REQUEST, torznab::error_xml(code, &msg)),
    };
    if let Some(ix) = &ix {
        if !ix.enabled {
            return xml(StatusCode::OK, torznab::error_xml(900, &format!("{} is turned off in Lantern", ix.name)));
        }
    }
    let source = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").split('/').next().unwrap_or("").to_string();
    let (releases, failed) = match st.indexers.search(&ids, &query, &source).await {
        Ok(x) => x,
        Err(e) => return xml(StatusCode::INTERNAL_SERVER_ERROR, torznab::error_xml(900, &e.detail)),
    };
    if releases.is_empty() && ix.is_some() {
        if let Some((_, why)) = failed.first() {
            return xml(StatusCode::OK, torznab::error_xml(900, why));
        }
    }
    let base = public_base(&st, &headers);
    let apikey = st.settings.str("api_key");
    let title = ix.as_ref().map(|i| i.name.clone()).unwrap_or_else(|| "Lantern".into());
    let self_url = format!("{base}/{id}/api");
    let body = torznab::results_xml(&title, &self_url, &releases, &|r| download_link(&st, &base, &apikey, r));
    xml(StatusCode::OK, body)
}

async fn download(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Q(q): Q<HashMap<String, String>>) -> Response {
    if !key_ok(&st, &headers, &q) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "detail": "The API key is missing or wrong" }))).into_response();
    }
    let link = q.get("link").map(String::as_str).unwrap_or("");
    let sig = q.get("sig").map(String::as_str).unwrap_or("");
    let Some(real) = st.signer.open_link(id, link, sig) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "detail": "This download link is not one Lantern made" }))).into_response();
    };
    let title = q.get("file").cloned().unwrap_or_default();
    let source = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").split('/').next().unwrap_or("").to_string();
    match st.indexers.grab(id, &real, &title, &source).await {
        Ok(Grabbed::Magnet(m)) => (StatusCode::FOUND, [(header::LOCATION, m)]).into_response(),
        Ok(Grabbed::File(bytes)) => {
            let nzb = super::cardigann::looks_like_nzb(&bytes);
            let ext = if nzb { "nzb" } else { "torrent" };
            let mime = if nzb { "application/x-nzb" } else { "application/x-bittorrent" };
            let safe: String = title.chars().filter(|c| c.is_alphanumeric() || " ._-()[]".contains(*c)).collect();
            let name = if safe.trim().is_empty() { format!("release.{ext}") } else { format!("{}.{ext}", safe.trim()) };
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))],
                bytes,
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// A release as the web app and the Prowlarr API show it.
fn release_json(st: &AppState, base: &str, apikey: &str, r: &Release) -> Value {
    let now = chrono::Utc::now();
    let age = r.date.map(|d| (now - d).num_minutes().max(0)).unwrap_or(0);
    let mut flags = Vec::new();
    if r.freeleech() {
        flags.push("freeleech");
    } else if r.download_factor == Some(0.5) {
        flags.push("halfleech");
    }
    if r.upload_factor == Some(2.0) {
        flags.push("doubleupload");
    }
    json!({
        "guid": r.guid,
        "title": r.title,
        "sortTitle": r.title.to_lowercase(),
        "indexerId": r.indexer_id,
        "indexer": r.indexer,
        "size": r.size.unwrap_or(0),
        "files": r.files,
        "grabs": r.grabs,
        "seeders": r.seeders,
        "leechers": r.leechers,
        "protocol": r.protocol,
        "publishDate": r.date.map(|d| d.to_rfc3339()),
        "age": age / 1440,
        "ageHours": age as f64 / 60.0,
        "ageMinutes": age as f64,
        "downloadUrl": download_link(st, base, apikey, r),
        "magnetUrl": if r.magnet.is_empty() { Value::Null } else { json!(r.magnet) },
        "infoUrl": r.details,
        "infoHash": if r.infohash.is_empty() { Value::Null } else { json!(r.infohash) },
        "imdbId": r.imdb.unwrap_or(0),
        "tmdbId": r.tmdb.unwrap_or(0),
        "tvdbId": r.tvdb.unwrap_or(0),
        "posterUrl": if r.poster.is_empty() { Value::Null } else { json!(r.poster) },
        "categories": r.categories.iter().map(|c| json!({ "id": c, "name": categories::name_of(*c).unwrap_or("Other") })).collect::<Vec<_>>(),
        "indexerFlags": flags,
        "downloadVolumeFactor": r.download_factor.unwrap_or(1.0),
        "uploadVolumeFactor": r.upload_factor.unwrap_or(1.0),
        "minimumRatio": r.minimum_ratio,
        "minimumSeedTime": r.minimum_seed_time,
    })
}

async fn search(State(st): State<AppState>, _: SignedIn, headers: HeaderMap, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    let query = query_of(&q).map_err(|(_, m)| err(400, m))?;
    let ids: Vec<i64> = q.get("ids").map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
    let (releases, failed) = st.indexers.search(&ids, &query, "Lantern").await?;
    let base = public_base(&st, &headers);
    let apikey = st.settings.str("api_key");
    let results: Vec<Value> = releases.iter().map(|r| release_json(&st, &base, &apikey, r)).collect();
    let failed: Vec<Value> = failed.iter().map(|(n, e)| json!({ "indexer": n, "error": e })).collect();
    Ok(Json(json!({ "results": results, "failed": failed })))
}

// ---- the Prowlarr API ----

fn v1_auth(st: &AppState, headers: &HeaderMap, q: &HashMap<String, String>) -> Result<()> {
    if key_ok(st, headers, q) {
        Ok(())
    } else {
        Err(err(401, "The API key is missing or wrong"))
    }
}

async fn v1_status(State(st): State<AppState>, headers: HeaderMap, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    v1_auth(&st, &headers, &q)?;
    Ok(Json(json!({
        "appName": "Lantern",
        "instanceName": st.app_name(),
        "version": crate::VERSION,
        "buildTime": Value::Null,
        "isDebug": false,
        "isProduction": true,
        "isAdmin": false,
        "isUserInteractive": false,
        "startupPath": "/app",
        "appData": st.data_dir.display().to_string(),
        "osName": "linux",
        "isDocker": true,
        "isLinux": true,
        "isOsx": false,
        "isWindows": false,
        "branch": "main",
        "authentication": "forms",
        "urlBase": "",
        "runtimeName": "rust",
    })))
}

async fn v1_empty(State(st): State<AppState>, headers: HeaderMap, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    v1_auth(&st, &headers, &q)?;
    Ok(Json(json!([])))
}

/// An indexer as the Prowlarr API describes it.
fn v1_resource(st: &AppState, ix: &Indexer) -> Value {
    let caps = st.indexers.caps(ix);
    let p = st.indexers.public(ix);
    let mut tops: Vec<Value> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (id, _) in caps.categories.iter().filter(|(id, _)| *id < 100000) {
        let parent = categories::parent(*id);
        if seen.insert(parent) {
            let subs: Vec<Value> = caps
                .categories
                .iter()
                .filter(|(c, _)| *c != parent && *c < 100000 && categories::parent(*c) == parent)
                .map(|(c, n)| json!({ "id": c, "name": n, "subCategories": [] }))
                .collect();
            tops.push(json!({ "id": parent, "name": categories::name_of(parent).unwrap_or("Other"), "subCategories": subs }));
        }
    }
    let params = |m: &str| caps.modes.iter().find(|(k, _)| k == m).map(|(_, v)| v.clone()).unwrap_or_default();
    json!({
        "id": ix.id,
        "name": ix.name,
        "enable": ix.enabled,
        "redirect": false,
        "priority": ix.priority,
        "protocol": ix.protocol(),
        "privacy": p["privacy"],
        "language": p["language"],
        "description": p["description"],
        "implementation": if ix.kind == "cardigann" { "Cardigann" } else if ix.kind == "newznab" { "Newznab" } else { "Torznab" },
        "implementationName": if ix.kind == "cardigann" { "Cardigann" } else if ix.kind == "newznab" { "Newznab" } else { "Torznab" },
        "definitionName": ix.definition,
        "indexerUrls": p["links"],
        "fields": [{ "name": "baseUrl", "value": p["base_url"] }],
        "tags": [],
        "added": chrono::DateTime::from_timestamp(ix.added as i64, 0).map(|d| d.to_rfc3339()),
        "capabilities": {
            "limitsMax": 100,
            "limitsDefault": 100,
            "categories": tops,
            "supportsRawSearch": false,
            "searchParams": params("search"),
            "tvSearchParams": params("tv-search"),
            "movieSearchParams": params("movie-search"),
            "musicSearchParams": params("music-search"),
            "bookSearchParams": params("book-search"),
        },
    })
}

async fn v1_indexers(State(st): State<AppState>, headers: HeaderMap, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    v1_auth(&st, &headers, &q)?;
    Ok(Json(Value::Array(st.indexers.list()?.iter().map(|i| v1_resource(&st, i)).collect())))
}

async fn v1_indexer(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Q(q): Q<HashMap<String, String>>,
) -> Result<Json<Value>> {
    v1_auth(&st, &headers, &q)?;
    Ok(Json(v1_resource(&st, &st.indexers.get(id)?)))
}

async fn v1_stats(State(st): State<AppState>, headers: HeaderMap, Q(q): Q<HashMap<String, String>>) -> Result<Json<Value>> {
    v1_auth(&st, &headers, &q)?;
    let list: Vec<Value> = st
        .indexers
        .stats(30.0)?
        .iter()
        .map(|s| {
            json!({
                "indexerId": s["id"], "indexerName": s["name"], "averageResponseTime": s["avg_ms"], "numberOfQueries": s["queries"],
                "numberOfGrabs": s["grabs"], "numberOfFailedQueries": s["failures"], "numberOfRssQueries": 0, "numberOfAuthQueries": 0,
                "numberOfFailedGrabs": 0, "numberOfFailedRssQueries": 0, "numberOfFailedAuthQueries": 0,
            })
        })
        .collect();
    Ok(Json(json!({ "indexers": list, "userAgents": [], "hosts": [] })))
}

async fn v1_search(State(st): State<AppState>, headers: HeaderMap, raw: axum::extract::RawQuery) -> Result<Json<Value>> {
    // indexerIds and categories may repeat (`categories=2000&categories=5000`)
    let mut q: HashMap<String, String> = HashMap::new();
    for (k, v) in url::form_urlencoded::parse(raw.0.unwrap_or_default().as_bytes()) {
        let k = k.to_string();
        let e = q.entry(k).or_default();
        if !e.is_empty() {
            e.push(',');
        }
        e.push_str(&v);
    }
    v1_auth(&st, &headers, &q)?;
    if let Some(ids) = q.remove("indexerIds") {
        q.insert("ids".into(), ids);
    }
    let query = query_of(&q).map_err(|(_, m)| err(400, m))?;
    let ids: Vec<i64> =
        q.get("ids").map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).filter(|x| *x > 0).collect()).unwrap_or_default();
    let source = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").split('/').next().unwrap_or("").to_string();
    let (releases, _) = st.indexers.search(&ids, &query, &source).await?;
    let base = public_base(&st, &headers);
    let apikey = st.settings.str("api_key");
    Ok(Json(Value::Array(releases.iter().map(|r| release_json(&st, &base, &apikey, r)).collect())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_search_parameters() {
        let q: HashMap<String, String> = [("t", "tvsearch"), ("q", "Show"), ("season", "1"), ("ep", "2"), ("cat", "5000,5040")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let query = query_of(&q).unwrap();
        assert_eq!(query.kind, SearchKind::TvSearch);
        assert_eq!(query.categories, vec![5000, 5040]);
        assert!(!query.rss);
        let q: HashMap<String, String> =
            [("query", "{ImdbId:tt0111161} extra")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let query = query_of(&q).unwrap();
        assert_eq!((query.imdbid.as_str(), query.keywords.as_str()), ("tt0111161", "extra"));
        let q: HashMap<String, String> = [("t", "nope")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert!(query_of(&q).is_err());
    }
}
