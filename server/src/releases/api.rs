//! The release routes.
//!
//! ```text
//! The web app (signed in; changes need an admin):
//!   GET    /api/releases/sources               the sources (secrets masked, IRC state)
//!   POST   /api/releases/sources               add {kind: irc|rss|indexer, name, definition, url, indexer_id, settings, interval_min, enabled}
//!   GET    /api/releases/sources/{id}          one
//!   PUT    /api/releases/sources/{id}          change (a masked secret keeps the saved one)
//!   DELETE /api/releases/sources/{id}          remove
//!   POST   /api/releases/sources/{id}/test     RSS/indexer: look now; IRC: the connection's state: {ok, message}
//!   GET    /api/releases/definitions?q=        the trackers that announce on IRC
//!   GET    /api/releases/definitions/{id}      one: its settings, network and channels
//!   GET    /api/releases/filters               the filters, highest priority first (API keys masked)
//!   POST   /api/releases/filters               add a filter
//!   GET    /api/releases/filters/{id}          one
//!   PUT    /api/releases/filters/{id}          change
//!   DELETE /api/releases/filters/{id}          remove
//!   POST   /api/releases/check                 {title, size, category, freeleech, tags, source_id}: which filters take it, sending nothing
//!   GET    /api/releases/log?limit=&status=    the newest releases seen and what happened to each
//!   POST   /api/releases/announce              a release from elsewhere ({title, download_url, …}) or an announce line
//!                                              of an IRC source ({source_id, line, channel}; one line per pattern of a
//!                                              multi-line announce); it goes through the filters now
//! ```

use super::definition::IndexerDefinition;
use super::{announce, Found};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result, SignedIn};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The release routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/releases/sources", get(sources).post(add_source))
        .route("/api/releases/sources/{id}", get(source).put(change_source).delete(remove_source))
        .route("/api/releases/sources/{id}/test", post(test_source))
        .route("/api/releases/definitions", get(definitions))
        .route("/api/releases/definitions/{id}", get(definition))
        .route("/api/releases/filters", get(filters).post(add_filter))
        .route("/api/releases/filters/{id}", get(filter).put(change_filter).delete(remove_filter))
        .route("/api/releases/check", post(check))
        .route("/api/releases/log", get(log))
        .route("/api/releases/announce", post(announce_now))
}

async fn sources(State(st): State<AppState>, _: SignedIn) -> Result<Json<Value>> {
    let r = &st.releases;
    Ok(Json(json!({ "sources": r.sources()?.iter().map(|s| r.public_source(s)).collect::<Vec<_>>() })))
}

async fn add_source(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let id = st.releases.add_source(&body)?;
    st.releases.changed(id);
    Ok(Json(st.releases.public_source(&st.releases.source(id)?)))
}

async fn source(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.releases.public_source(&st.releases.source(id)?)))
}

async fn change_source(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>> {
    st.releases.update_source(id, &body)?;
    st.releases.changed(id);
    Ok(Json(st.releases.public_source(&st.releases.source(id)?)))
}

async fn remove_source(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    st.releases.remove_source(id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_source(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    let s = st.releases.source(id)?;
    if s.kind == "irc" {
        let status = st.releases.source_irc_status(&s);
        let connected = status["connected"] == true;
        let channels: Vec<&str> = status["channels"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let nick = s.settings.get("nick").cloned().unwrap_or_default();
        let last_error = status["last_error"].as_str().unwrap_or_default().to_string();
        let message = if connected {
            if channels.is_empty() {
                format!("Signed in as {nick}, not in the announce channels yet")
            } else {
                format!("Signed in as {nick}, in {}", channels.join(", "))
            }
        } else if !s.enabled {
            "This source is off".to_string()
        } else if last_error.is_empty() {
            "Connecting…".to_string()
        } else {
            last_error
        };
        return Ok(Json(json!({ "ok": connected, "message": message })));
    }
    Ok(Json(match super::rss::poll(&st.releases, &s).await {
        Ok(n) => {
            st.releases.source_result(id, "");
            json!({ "ok": true, "message": if n == 0 { "The feed answered; nothing new since the last look".to_string() } else { format!("The feed answered with {n} new releases") } })
        }
        Err(e) => {
            st.releases.source_result(id, &e);
            json!({ "ok": false, "message": e })
        }
    }))
}

/// A definition as the list shows it.
fn definition_json(d: &IndexerDefinition) -> Value {
    let irc = d.irc.as_ref();
    let channels = irc.map(|i| i.channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
    let announcers = irc.map(|i| i.channels.iter().flat_map(|c| c.announcers.clone()).collect::<Vec<_>>());
    json!({
        "id": d.identifier, "name": d.name, "description": d.description, "urls": d.urls, "privacy": d.privacy, "protocol": d.protocol,
        "network": irc.map(|i| i.network.clone()), "server": irc.map(|i| i.server.clone()), "port": irc.map(|i| i.port), "tls": irc.map(|i| i.tls),
        "channels": channels, "announcers": announcers,
    })
}

async fn definitions(State(st): State<AppState>, _: SignedIn, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let words = q.get("q").map(|s| s.to_lowercase()).unwrap_or_default();
    let list: Vec<Value> = st
        .releases
        .library
        .all()
        .iter()
        .filter(|d| d.irc.is_some())
        .filter(|d| words.is_empty() || d.name.to_lowercase().contains(&words) || d.identifier.contains(&words))
        .map(|d| definition_json(d))
        .collect();
    Json(json!({ "definitions": list, "errors": st.releases.library.errors() }))
}

async fn definition(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Result<Json<Value>> {
    let d = st.releases.library.get(&id).ok_or_else(|| err(404, "There is no announce definition with that id"))?;
    let mut v = definition_json(&d);
    v["settings"] = json!(d
        .settings
        .iter()
        .chain(d.irc.iter().flat_map(|i| i.settings.iter()))
        .map(|s| json!({ "name": s.name, "type": s.kind, "label": s.label, "help": s.help, "required": s.required, "default": s.default, "secret": s.kind == "secret" }))
        .collect::<Vec<_>>());
    Ok(Json(v))
}

async fn filters(State(st): State<AppState>, _: SignedIn) -> Result<Json<Value>> {
    let r = &st.releases;
    Ok(Json(json!({ "filters": r.filters()?.iter().map(|f| r.public_filter(f)).collect::<Vec<_>>() })))
}

async fn add_filter(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let id = st.releases.add_filter(&body)?;
    Ok(Json(st.releases.public_filter(&st.releases.filter(id)?)))
}

async fn filter(State(st): State<AppState>, _: SignedIn, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(st.releases.public_filter(&st.releases.filter(id)?)))
}

async fn change_filter(State(st): State<AppState>, _: Admin, Path(id): Path<i64>, Json(body): Json<Value>) -> Result<Json<Value>> {
    st.releases.update_filter(id, &body)?;
    Ok(Json(st.releases.public_filter(&st.releases.filter(id)?)))
}

async fn remove_filter(State(st): State<AppState>, _: Admin, Path(id): Path<i64>) -> Result<Json<Value>> {
    st.releases.remove_filter(id)?;
    Ok(Json(json!({ "ok": true })))
}

/// A release from a request body (`title`, `download_url`, `info_url`, `size` in bytes or words, `category`,
/// `freeleech`, `freeleech_percent`, `uploader`, `tags` as a list or comma separated, `year`, `source_id`).
fn found_of(st: &AppState, body: &Value) -> Result<Found> {
    let s = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let title = s("title");
    if title.is_empty() {
        return Err(err(400, "Enter a release name"));
    }
    let source_id = body.get("source_id").and_then(Value::as_i64).unwrap_or(0);
    let source_name = if source_id > 0 { st.releases.source(source_id)?.name } else { s("source_name") };
    let size = match body.get("size") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(t)) if !t.trim().is_empty() => Some(
            super::humanize::parse_bytes(t.trim())
                .map_err(|_| err(400, format!("The size \"{t}\" can't be read. Write it like 700 MB or 1.5 GB")))?,
        ),
        _ => None,
    };
    let tags = match body.get("tags") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(t)) => t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => Vec::new(),
    };
    Ok(Found {
        title,
        source_id,
        source_name: if source_name.is_empty() { "Elsewhere".into() } else { source_name },
        protocol: if s("protocol") == "usenet" { "usenet".into() } else { "torrent".into() },
        download_url: s("download_url"),
        info_url: s("info_url"),
        size,
        category: s("category"),
        freeleech: body.get("freeleech").and_then(Value::as_bool).unwrap_or(false),
        freeleech_percent: body.get("freeleech_percent").and_then(Value::as_u64).map(|p| p.min(100) as u32),
        uploader: s("uploader"),
        tags,
        year: body.get("year").and_then(Value::as_u64).map(|y| y as u32),
        ..Found::default()
    })
}

async fn check(State(st): State<AppState>, _: SignedIn, Json(body): Json<Value>) -> Result<Json<Value>> {
    let f = found_of(&st, &body)?;
    let r = f.release();
    let name = json!({
        "type": r.r#type.as_str(), "title": r.title, "year": r.year, "season": r.season, "episode": r.episode,
        "resolution": r.resolution, "source": r.source, "codec": r.codec, "hdr": r.hdr, "audio": r.audio,
        "group": r.group, "website": r.website, "other": r.other, "normalized_hash": r.normalized_hash,
    });
    Ok(Json(json!({ "name": name, "filters": st.releases.dry_run(&f)? })))
}

async fn log(State(st): State<AppState>, _: SignedIn, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>> {
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100);
    let status = q.get("status").map(String::as_str).filter(|s| !s.is_empty());
    Ok(Json(json!({ "releases": st.releases.log(limit, status)? })))
}

async fn announce_now(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let o = if let Some(line) = body.get("line").and_then(Value::as_str) {
        let id =
            body.get("source_id").and_then(Value::as_i64).ok_or_else(|| err(400, "Say which IRC source the line is from (source_id)"))?;
        st.releases.source(id)?;
        let def = st.releases.mapped_definition(id).ok_or_else(|| err(400, "This source has no announce definition"))?;
        let lines: Vec<&str> = line.lines().collect();
        let channel = body.get("channel").and_then(Value::as_str).unwrap_or_default();
        let r = announce::read_lines(&def, channel, &lines).map_err(|e| err(400, e))?;
        st.releases.process_release(r).await
    } else {
        st.releases.handle(found_of(&st, &body)?).await
    };
    let reasons: Vec<Value> = o.reasons.iter().map(|(n, why)| json!({ "filter": n, "why": why })).collect();
    Ok(Json(json!({ "status": o.status, "filter": o.filter.map(|(_, n)| n), "detail": o.detail, "reasons": reasons })))
}
