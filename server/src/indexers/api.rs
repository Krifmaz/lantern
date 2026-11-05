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
//!   GET    /{id}/api?t=caps|search|tvsearch|movie|music|book…   Newznab/Torznab as Prowlarr answers it, one indexer
//!          (also /api/v1/indexer/{id}/newznab; 0: Prowlarr's test indexer; "all": every one, a Lantern extra)
//!   GET    /api/v2.0/indexers/{id}/results/torznab/api?…        the same, at Jackett's address
//!   GET    /{id}/download?apikey=&link=&sig=&file=              a release's file through Lantern, or a redirect to it
//!          (also /api/v1/indexer/{id}/download)
//!   *      /api/v1/indexerproxy…                                the indexer proxies ([`proxies_api`](super::proxies_api))
//! ```

use super::capabilities::Capabilities;
use super::cardigann::Grabbed;
use super::newznab_request::{self, NewznabRequest};
use super::release::{Protocol, Query, Release, SearchKind};
use super::{categories, nabxml, newznab, newznab_results, GrabError};
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
        .route("/{id}/api", get(newznab_api))
        .route("/{id}/download", get(download))
        .route("/api/v2.0/indexers/{id}/results/torznab", get(newznab_api))
        .route("/api/v2.0/indexers/{id}/results/torznab/api", get(newznab_api))
        .route("/api/v1/indexer/{id}/newznab", get(newznab_api))
        .route("/api/v1/indexer/{id}/download", get(download))
        .merge(super::proxies_api::router())
}

async fn list(State(st): State<AppState>, _: SignedIn) -> Result<Json<Value>> {
    let list: Vec<Value> = st.indexers.list()?.iter().map(|i| st.indexers.public(i)).collect();
    Ok(Json(json!({ "indexers": list, "definitions": st.indexers.library.len() })))
}

async fn add(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let id = st.indexers.add(&body)?;
    let ix = st.indexers.get(id)?;
    // a Newznab/Torznab API is asked for its capabilities when it is saved turned on (`IndexerFactory.Create`)
    if ix.enabled {
        st.indexers.refresh_caps_of(std::slice::from_ref(&ix)).await;
    }
    Ok(Json(st.indexers.public(&st.indexers.get(id)?)))
}

async fn one(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.indexers.public(&st.indexers.get(id)?)))
}

async fn change(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>> {
    st.indexers.update(id, &body)?;
    let ix = st.indexers.get(id)?;
    // asked again when it is saved turned on (`IndexerFactory.Update`)
    if ix.enabled {
        st.indexers.refresh_caps_of(std::slice::from_ref(&ix)).await;
    }
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
        ..Query::default()
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

/// A file name or title as .NET's `WebUtility.UrlEncode` writes it in a link: letters, digits and `-_.!*()` as
/// they are, a space as `+`, anything else as `%XX`.
pub fn web_url_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'*' | b'(' | b')' => o.push(b as char),
            b' ' => o.push('+'),
            b => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// The address Lantern hands out for a release's file, magnet or not (`DownloadMappingService.ConvertToProxyLink`):
/// `{base}/{id}/download?apikey=…&link=…&sig=…&file=…`. The link is signed rather than encrypted (`sig`).
pub fn proxy_link(st: &AppState, base: &str, apikey: &str, indexer: i64, link: &str, title: &str) -> String {
    let (link, sig) = st.signer.sign(indexer, link);
    format!("{base}/{indexer}/download?apikey={apikey}&link={link}&sig={sig}&file={}", web_url_encode(title))
}

/// The address Lantern hands out for a release's file.
fn download_link(st: &AppState, base: &str, apikey: &str, r: &Release) -> String {
    proxy_link(st, base, apikey, r.indexer_id, if r.link.is_empty() { &r.magnet } else { &r.link }, &r.title)
}

/// An answer of Newznab XML with `status` (`CreateResponse`).
fn xml(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/rss+xml")], body).into_response()
}

/// A Newznab error answer: `<error code=… description=… />` with `status`.
fn xml_error(status: StatusCode, code: u16, description: &str) -> Response {
    xml(status, nabxml::error_xml(code, description))
}

/// Adds `Retry-After` when there is time left to wait (`AddRetryAfterHeader`).
fn with_retry_after(mut r: Response, seconds: i64) -> Response {
    if seconds > 0 {
        if let Ok(v) = header::HeaderValue::from_str(&seconds.to_string()) {
            r.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
    r
}

/// The 429 answer for an indexer on hold until `till` (seconds since 1970), with how long to wait.
fn on_hold(till: f64) -> Response {
    let when = chrono::DateTime::from_timestamp(till as i64, 0).map(|d| d.format("%m/%d/%Y %H:%M:%S").to_string()).unwrap_or_default();
    let r = xml_error(StatusCode::TOO_MANY_REQUESTS, 429, &format!("Indexer is disabled till {when} due to recent failures."));
    with_retry_after(r, (till - crate::auth::now()).round() as i64)
}

/// The query's pairs, in order, repeated names kept.
fn pairs_of(raw: Option<String>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

/// Whether the request carries the API key as Prowlarr reads it: `?apikey=` (any case) first, else `X-Api-Key`,
/// else `Authorization: Bearer …` (`ApiKeyAuthenticationHandler.ParseApiKey`).
fn prowlarr_key_ok(st: &AppState, headers: &HeaderMap, pairs: &[(String, String)]) -> bool {
    let want = st.settings.str("api_key");
    if want.is_empty() {
        return false;
    }
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    let given = pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("apikey"))
        .map(|(_, v)| v.clone())
        .or_else(|| h("x-api-key"))
        .or_else(|| h("authorization").map(|a| a.replace("Bearer ", "")))
        .unwrap_or_default();
    !given.trim().is_empty() && crate::auth::ct_eq(&given, &want)
}

/// The app that asked (`Sonarr` …) from the request's headers (`GetSource`).
pub fn source_from(headers: &HeaderMap) -> String {
    newznab_request::source_of(headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()), headers.contains_key("x-prowlarr-client"))
}

/// The machine that asked: the first `X-Forwarded-For` hop, else `X-Real-IP` (`GetHostName`; Lantern doesn't look the
/// address up in DNS).
pub fn host_from(headers: &HeaderMap) -> String {
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
    h("x-forwarded-for").and_then(|v| v.split(',').next().map(|s| s.trim().to_string())).or_else(|| h("x-real-ip")).unwrap_or_default()
}

/// A model-binding error as ASP.NET answers it: 400 with the problem in `errors` (and in `detail`).
fn bind_error(e: &newznab_request::BindError) -> Response {
    let body = json!({
        "type": "https://tools.ietf.org/html/rfc9110#section-15.5.1",
        "title": "One or more validation errors occurred.",
        "status": 400,
        "detail": e.message(),
        "errors": { e.name.clone(): [e.message()] },
    });
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

/// `GET /{id}/api` and `/api/v1/indexer/{id}/newznab` (`NewznabController.GetNewznabResponse`): one indexer's
/// capabilities and searches as Newznab/Torznab. Indexer 0 is Prowlarr's test indexer; `all` (a Lantern extra)
/// searches every indexer at once.
async fn newznab_api(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<String>, raw: axum::extract::RawQuery) -> Response {
    let pairs = pairs_of(raw.0);
    if !prowlarr_key_ok(&st, &headers, &pairs) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut request = match NewznabRequest::from_pairs(&pairs) {
        Ok(r) => r,
        Err(e) => return bind_error(&e),
    };
    let base = public_base(&st, &headers);
    request.source = Some(source_from(&headers));
    request.server = Some(base.clone());
    request.host = Some(host_from(&headers));
    let Some(t) = request.t.clone() else {
        return xml_error(StatusCode::BAD_REQUEST, 200, "Missing parameter (t)");
    };
    request.imdbid = request.imdbid.as_deref().map(|i| i.trim_start_matches('t').to_string());
    if let Some(i) = request.imdbid.as_deref().filter(|i| !i.trim().is_empty()) {
        if !i.parse::<i32>().is_ok_and(|n| n != 0) {
            return xml_error(StatusCode::BAD_REQUEST, 201, "Incorrect parameter (imdbid)");
        }
    }
    let title = st.app_name();
    let searches = ["search", "tvsearch", "music", "book", "movie"];
    let source = request.source.clone().unwrap_or_default();
    let host = request.host.clone().unwrap_or_default();
    let apikey = st.settings.str("api_key");
    // every indexer at once (a Lantern extra, like Jackett's `all`)
    if id == "all" {
        return match t.as_str() {
            "caps" => match st.indexers.capabilities_all() {
                Ok(c) => xml(StatusCode::OK, c.to_xml(&title)),
                Err(e) => xml_error(StatusCode::INTERNAL_SERVER_ERROR, 900, &e.detail),
            },
            t if searches.contains(&t) => match st.indexers.dispatch(&[], &request.to_query(), &source, &host, false).await {
                Ok(found) => {
                    let list = proxied(&st, &base, &apikey, found.releases);
                    xml(StatusCode::OK, newznab_results::to_xml(&title, &list, None, false, chrono::Utc::now()))
                }
                Err(e) => xml_error(StatusCode::INTERNAL_SERVER_ERROR, 900, &e.detail),
            },
            _ => xml_error(StatusCode::BAD_REQUEST, 202, &format!("No such function ({t})")),
        };
    }
    let Ok(id) = id.parse::<i64>() else { return StatusCode::NOT_FOUND.into_response() };
    if id == 0 {
        return match t.as_str() {
            "caps" => xml(StatusCode::OK, Capabilities::test_indexer().to_xml(&title)),
            t if searches.contains(&t) => xml(StatusCode::OK, newznab_results::test_results(&title, chrono::Utc::now())),
            _ => (StatusCode::NOT_FOUND, Json(json!({ "detail": "Indexer Not Found" }))).into_response(),
        };
    }
    let Ok(ix) = st.indexers.get(id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "detail": "Indexer Not Found" }))).into_response();
    };
    if !ix.enabled {
        return xml_error(StatusCode::GONE, 410, "Indexer is disabled");
    }
    if let Some(till) = st.indexers.blocked_until(&ix) {
        return on_hold(till);
    }
    if st.indexers.at_query_limit(&ix) {
        let b = ix.base_settings();
        let r = xml_error(
            StatusCode::TOO_MANY_REQUESTS,
            429,
            &format!(
                "User configurable Indexer Query Limit of {} in last {} hour(s) reached.",
                b.query_limit.unwrap_or(0),
                b.interval_hours()
            ),
        );
        return with_retry_after(r, st.indexers.retry_after_query_limit(&ix));
    }
    match t.as_str() {
        "caps" => {
            st.indexers.refresh_caps_of(std::slice::from_ref(&ix)).await;
            xml(StatusCode::OK, st.indexers.capabilities(&ix).to_xml(&title))
        }
        t if searches.contains(&t) => {
            let found = match st.indexers.dispatch(&[ix.id], &request.to_query(), &source, &host, false).await {
                Ok(f) => f,
                Err(e) => return xml_error(StatusCode::INTERNAL_SERVER_ERROR, 900, &e.detail),
            };
            if let Some(till) = st.indexers.blocked_until(&ix) {
                return on_hold(till);
            }
            let list = proxied(&st, &base, &apikey, found.releases);
            let prefer_magnet = ix.protocol() == Protocol::Torrent && ix.torrent_settings().prefer_magnet_url;
            xml(StatusCode::OK, newznab_results::to_xml(&title, &list, Some(ix.protocol()), prefer_magnet, chrono::Utc::now()))
        }
        _ => xml_error(StatusCode::BAD_REQUEST, 202, &format!("No such function ({t})")),
    }
}

/// `releases` with their file and magnet links going through Lantern.
fn proxied(st: &AppState, base: &str, apikey: &str, releases: Vec<Release>) -> Vec<Release> {
    releases
        .into_iter()
        .map(|mut r| {
            if !r.link.is_empty() {
                r.link = proxy_link(st, base, apikey, r.indexer_id, &r.link, &r.title);
            }
            if !r.magnet.is_empty() && r.protocol == Protocol::Torrent {
                r.magnet = proxy_link(st, base, apikey, r.indexer_id, &r.magnet, &r.title);
            }
            r
        })
        .collect()
}

/// The `Content-Disposition` ASP.NET writes for a file called `name`: the name as it is when it can be, quoted
/// when it has spaces or separators, `_` for what isn't ASCII, and the exact name again as `filename*`.
fn content_disposition(name: &str) -> String {
    let plain: String = name.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '_' }).collect();
    let token = !plain.is_empty() && plain.chars().all(|c| !"()<>@,;:\\\"/[]?={} \t".contains(c));
    let shown = if token { plain } else { format!("\"{}\"", plain.replace('\\', "\\\\").replace('"', "\\\"")) };
    let mut star = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            star.push(b as char);
        } else {
            star.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename={shown}; filename*=UTF-8''{star}")
}

/// `GET /{id}/download` and `/api/v1/indexer/{id}/download` (`NewznabController.GetDownload`): a release's file
/// through Lantern, or the app sent to the site (always for Usenet, and for indexers set to redirect).
async fn download(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<String>, raw: axum::extract::RawQuery) -> Response {
    let pairs = pairs_of(raw.0);
    if !prowlarr_key_ok(&st, &headers, &pairs) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(id) = id.parse::<i64>() else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(ix) = st.indexers.get(id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "detail": "Indexer Not Found" }))).into_response();
    };
    if !ix.enabled {
        return xml_error(StatusCode::GONE, 410, "Indexer is disabled");
    }
    if let Some(till) = st.indexers.blocked_until(&ix) {
        return on_hold(till);
    }
    if st.indexers.at_grab_limit(&ix) {
        let b = ix.base_settings();
        let r = xml_error(
            StatusCode::TOO_MANY_REQUESTS,
            429,
            &format!(
                "User configurable Indexer Grab Limit of {} in last {} hour(s) reached.",
                b.grab_limit.unwrap_or(0),
                b.interval_hours()
            ),
        );
        return with_retry_after(r, st.indexers.retry_after_grab_limit(&ix));
    }
    let get = |k: &str| pairs.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()).unwrap_or_default();
    let (link, file) = (get("link"), get("file"));
    if link.trim().is_empty() || file.trim().is_empty() {
        return err(400, "Invalid Prowlarr link").into_response();
    }
    // Prowlarr decodes the file name a second time (`WebUtility.UrlDecode`)
    let file: String = url::form_urlencoded::parse(format!("f={}", file.replace('&', "%26")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or(file);
    let Some(real) = st.signer.open_link(id, &link, &get("sig")) else {
        return err(400, "Failed to normalize provided link").into_response();
    };
    let source = source_from(&headers);
    let host = host_from(&headers);
    if ix.protocol() == Protocol::Usenet || (ix.supports_redirect() && ix.redirect) {
        st.indexers.record_redirect(&ix, &real, &file, &source, &host);
        return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, real)]).into_response();
    }
    let bytes = match st.indexers.download(&ix, &real, &file, &source, &host).await {
        Ok(Grabbed::Magnet(m)) => return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, m)]).into_response(),
        Ok(Grabbed::File(b)) => b,
        Err(GrabError::Unavailable(_)) => return xml_error(StatusCode::GONE, 410, "Download failed"),
        Err(GrabError::TooManyRequests(_)) => return xml_error(StatusCode::TOO_MANY_REQUESTS, 429, "Download failed"),
        Err(GrabError::Failed(_)) => return xml_error(StatusCode::INTERNAL_SERVER_ERROR, 500, "Download failed"),
    };
    if bytes.starts_with(b"magnet:") {
        return (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, String::from_utf8_lossy(&bytes).into_owned())]).into_response();
    }
    let torrent = ix.protocol() == Protocol::Torrent;
    let mime = if torrent { "application/x-bittorrent" } else { "application/x-nzb" };
    let name = format!("{file}.{}", if torrent { "torrent" } else { "nzb" });
    (StatusCode::OK, [(header::CONTENT_TYPE, mime.to_string()), (header::CONTENT_DISPOSITION, content_disposition(&name))], bytes)
        .into_response()
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
