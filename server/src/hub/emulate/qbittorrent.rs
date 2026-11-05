//! qBittorrent's Web API v2, as Sonarr, Radarr, Lidarr, Readarr and other apps use it. An app signs in with its
//! own name as the user name and Lantern's API key as the password. Its torrents are its grabs, found by their
//! info hash; their category is the one the app asked for, while the real client gets the app's Lantern category
//! and folder. Paths are in the app's own view. Calls that only tune the real client (tags, priorities, share
//! limits, pause and resume) are accepted and left alone: Lantern's own rules decide.

use crate::hub::apps::App;
use crate::hub::clients::Content;
use crate::hub::grabs::Grab;
use crate::hub::paths;
use crate::AppState;
use axum::extract::{FromRequestParts, Multipart, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The qBittorrent version Lantern says it is.
pub const VERSION: &str = "v4.6.7";

/// The Web API version Lantern says it speaks.
pub const API_VERSION: &str = "2.9.3";

/// qBittorrent's "no limit / unknown" time (100 days).
const FOREVER: i64 = 8_640_000;

/// The emulated qBittorrent's routes.
pub fn router() -> Router<AppState> {
    let ok = || post(|_: QbitApp| async { "" });
    Router::new()
        .route("/api/v2/auth/login", post(login))
        .route("/api/v2/auth/logout", post(logout))
        .route("/api/v2/app/version", get(|_: QbitApp| async { VERSION }))
        .route("/api/v2/app/webapiVersion", get(|_: QbitApp| async { API_VERSION }))
        .route("/api/v2/app/buildInfo", get(|_: QbitApp| async { Json(json!({ "qt": "6.5", "libtorrent": "2.0.10", "bitness": 64 })) }))
        .route("/api/v2/app/preferences", get(preferences))
        .route("/api/v2/app/defaultSavePath", get(default_save_path))
        .route("/api/v2/app/setPreferences", ok())
        .route(
            "/api/v2/transfer/info",
            get(|_: QbitApp| async { Json(json!({ "dl_info_speed": 0, "up_info_speed": 0, "connection_status": "connected" })) }),
        )
        .route("/api/v2/torrents/categories", get(categories))
        .route("/api/v2/torrents/createCategory", post(create_category))
        .route("/api/v2/torrents/editCategory", post(create_category))
        .route("/api/v2/torrents/removeCategories", post(remove_categories))
        .route("/api/v2/torrents/tags", get(|_: QbitApp| async { Json(json!([])) }))
        .route("/api/v2/torrents/add", post(add))
        .route("/api/v2/torrents/info", get(info))
        .route("/api/v2/torrents/properties", get(properties))
        .route("/api/v2/torrents/files", get(files))
        .route("/api/v2/torrents/delete", post(delete))
        .route("/api/v2/torrents/setCategory", post(set_category))
        .route("/api/v2/sync/maindata", get(maindata))
        .route("/api/v2/torrents/createTags", ok())
        .route("/api/v2/torrents/addTags", ok())
        .route("/api/v2/torrents/removeTags", ok())
        .route("/api/v2/torrents/setShareLimits", ok())
        .route("/api/v2/torrents/topPrio", ok())
        .route("/api/v2/torrents/bottomPrio", ok())
        .route("/api/v2/torrents/setForceStart", ok())
        .route("/api/v2/torrents/pause", ok())
        .route("/api/v2/torrents/resume", ok())
        .route("/api/v2/torrents/stop", ok())
        .route("/api/v2/torrents/start", ok())
        .route("/api/v2/torrents/recheck", ok())
        .route("/api/v2/torrents/reannounce", ok())
        .route("/api/v2/torrents/setSuperSeeding", ok())
        .route("/api/v2/torrents/setAutoManagement", ok())
}

/// The app a request signed in as, from its `SID` cookie. Without one qBittorrent answers 403 Forbidden.
pub struct QbitApp(pub App);

impl FromRequestParts<AppState> for QbitApp {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let sid = parts
            .headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|c| c.split(';'))
            .find_map(|kv| kv.trim().strip_prefix("SID=").map(str::to_string));
        let app_id = sid.and_then(|s| st.hub.qbit_sessions.lock().unwrap_or_else(|e| e.into_inner()).get(&s).copied());
        match app_id.and_then(|id| st.hub.app(id).ok()) {
            Some(a) => {
                st.hub.seen(a.id);
                Ok(QbitApp(a))
            }
            None => Err((StatusCode::FORBIDDEN, "Forbidden").into_response()),
        }
    }
}

async fn login(State(st): State<AppState>, Form(f): Form<HashMap<String, String>>) -> Response {
    let key = st.settings.str("api_key");
    let password = f.get("password").map(String::as_str).unwrap_or("");
    if key.is_empty() || !crate::auth::ct_eq(password, &key) {
        // qBittorrent answers a wrong password with 200 "Fails."
        return "Fails.".into_response();
    }
    let app = match st.hub.emulated_app(f.get("username").map(String::as_str).unwrap_or(""), "qbittorrent") {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let sid = crate::auth::random_token(16);
    st.hub.qbit_sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(sid.clone(), app.id);
    ([(header::SET_COOKIE, format!("SID={sid}; HttpOnly; path=/; SameSite=Strict"))], "Ok.").into_response()
}

async fn logout(State(st): State<AppState>, parts: Parts) -> &'static str {
    if let Some(c) = parts.headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        if let Some(sid) = c.split(';').find_map(|kv| kv.trim().strip_prefix("SID=")) {
            st.hub.qbit_sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(sid);
        }
    }
    ""
}

/// The app's folder as the app sees it.
fn app_folder(st: &AppState, app: &App) -> String {
    let folder = st.hub.folder(app);
    paths::to_app(&folder, &folder, &app.import_path)
}

async fn preferences(State(st): State<AppState>, QbitApp(app): QbitApp) -> Json<Value> {
    let ratio = st.settings.float("seed_ratio");
    let hours = st.settings.float("seed_hours");
    Json(json!({
        "save_path": app_folder(&st, &app), "temp_path_enabled": false, "auto_tmm_enabled": false,
        "max_ratio_enabled": ratio > 0.0, "max_ratio": if ratio > 0.0 { ratio } else { -1.0 },
        "max_seeding_time_enabled": hours > 0.0, "max_seeding_time": if hours > 0.0 { (hours * 60.0) as i64 } else { -1 },
        "max_ratio_act": 0, "queueing_enabled": false, "dht": true, "start_paused_enabled": false,
        "web_ui_username": app.name, "locale": "en",
    }))
}

async fn default_save_path(State(st): State<AppState>, QbitApp(app): QbitApp) -> String {
    app_folder(&st, &app)
}

async fn categories(State(st): State<AppState>, QbitApp(app): QbitApp) -> Json<Value> {
    let folder = app_folder(&st, &app);
    let mut m = serde_json::Map::new();
    for c in st.hub.ext_categories(app.id) {
        m.insert(c.clone(), json!({ "name": c, "savePath": folder }));
    }
    Json(Value::Object(m))
}

async fn create_category(State(st): State<AppState>, QbitApp(app): QbitApp, Form(f): Form<HashMap<String, String>>) -> Response {
    let name = f.get("category").map(String::as_str).unwrap_or("").trim().to_string();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "Category name is empty").into_response();
    }
    match st.hub.set_ext_category(app.id, &name, true) {
        Ok(()) => "".into_response(),
        Err(e) => e.into_response(),
    }
}

async fn remove_categories(State(st): State<AppState>, QbitApp(app): QbitApp, Form(f): Form<HashMap<String, String>>) -> Response {
    for c in f.get("categories").map(String::as_str).unwrap_or("").split('\n') {
        if let Err(e) = st.hub.set_ext_category(app.id, c.trim(), false) {
            return e.into_response();
        }
    }
    "".into_response()
}

async fn add(State(st): State<AppState>, QbitApp(app): QbitApp, mut m: Multipart) -> Response {
    let mut urls = String::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut category = String::new();
    let mut rename = String::new();
    while let Ok(Some(field)) = m.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "torrents" {
            let file = field.file_name().unwrap_or("").to_string();
            files.push((file, field.bytes().await.map(|b| b.to_vec()).unwrap_or_default()));
            continue;
        }
        let text = field.text().await.unwrap_or_default();
        match name.as_str() {
            "urls" => urls = text,
            "category" => category = text,
            "rename" => rename = text,
            _ => {}
        }
    }
    if !category.is_empty() {
        let _ = st.hub.set_ext_category(app.id, &category, true);
    }
    let mut added = 0;
    for u in urls.split(['\n', '|']).map(str::trim).filter(|u| !u.is_empty()) {
        let content = if u.starts_with("magnet:") {
            Some(Content::Magnet(u.to_string()))
        } else {
            match st.hub.fetch_url(u).await {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!("{}: {e}", app.name);
                    None
                }
            }
        };
        if let Some(c) = content {
            if st.hub.grab_content(&app, c, &rename, &category).is_ok() {
                added += 1;
            }
        }
    }
    for (file, bytes) in files {
        let title = if rename.is_empty() { file.trim_end_matches(".torrent").to_string() } else { rename.clone() };
        if crate::hub::torrent::info_hash(&bytes).is_some() && st.hub.grab_content(&app, Content::Torrent(bytes), &title, &category).is_ok()
        {
            added += 1;
        }
    }
    if added == 0 {
        // qBittorrent says "Fails." when nothing could be added
        return "Fails.".into_response();
    }
    let hub = st.hub.clone();
    tokio::spawn(async move { hub.tick().await });
    "Ok.".into_response()
}

/// The app's grabs that qBittorrent would list: torrents with a known hash, not removed.
fn torrents(st: &AppState, app: &App) -> Vec<Grab> {
    st.hub
        .grabs(Some(app.id), None, 1000)
        .unwrap_or_default()
        .into_iter()
        .filter(|g| g.protocol == "torrent" && !g.infohash.is_empty() && g.state != "removed")
        .collect()
}

/// qBittorrent's state for a grab.
fn state_of(g: &Grab) -> &'static str {
    match g.state.as_str() {
        "queued" | "waiting" => {
            if g.progress > 0.0 {
                "queuedDL"
            } else {
                "metaDL"
            }
        }
        "downloading" => {
            if g.speed > 0 {
                "downloading"
            } else {
                "stalledDL"
            }
        }
        "paused" => "pausedDL",
        "seeding" => "uploading",
        "completed" => "pausedUP",
        "failed" => "error",
        _ => "unknown",
    }
}

/// A grab as qBittorrent describes a torrent, with paths in the app's view.
fn torrent_json(st: &AppState, app: &App, g: &Grab) -> Value {
    let folder = st.hub.folder(app);
    let view = |p: &str| paths::to_app(p, &folder, &app.import_path);
    let (save_path, content_path) = if g.path.is_empty() {
        (view(&folder), format!("{}/{}", view(&folder), g.title))
    } else {
        let parent = g.path.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or(folder.clone());
        (view(&parent), view(&g.path))
    };
    let size = g.size.max(0);
    let done = (size as f64 * g.progress) as i64;
    let ratio_limit = st.settings.float("seed_ratio");
    let hours = st.settings.float("seed_hours");
    json!({
        "hash": g.infohash, "infohash_v1": g.infohash, "name": g.title, "size": size, "total_size": size,
        "progress": g.progress, "dlspeed": g.speed, "upspeed": 0, "eta": if g.eta > 0 { g.eta } else { FOREVER },
        "state": state_of(g), "category": if g.ext_category.is_empty() { app.category.clone() } else { g.ext_category.clone() },
        "tags": "", "save_path": save_path, "content_path": content_path, "download_path": "",
        "ratio": g.ratio, "ratio_limit": -2, "max_ratio": if ratio_limit > 0.0 { ratio_limit } else { -1.0 },
        "seeding_time": g.seed_secs, "seeding_time_limit": -2,
        "max_seeding_time": if hours > 0.0 { (hours * 60.0) as i64 } else { -1 },
        "added_on": g.added as i64, "completion_on": if g.finished > 0.0 && g.state != "failed" { g.finished as i64 } else { -1 },
        "amount_left": (size - done).max(0), "downloaded": done, "completed": done, "num_seeds": 0, "num_leechs": 0,
        "priority": 0, "auto_tmm": false, "force_start": false, "super_seeding": false, "time_active": (crate::auth::now() - g.added) as i64,
        "magnet_uri": if g.link.starts_with("magnet:") { g.link.clone() } else { String::new() }, "tracker": "",
    })
}

async fn info(State(st): State<AppState>, QbitApp(app): QbitApp, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let hashes: Vec<String> =
        q.get("hashes").map(|h| h.split('|').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    let category = q.get("category");
    let list: Vec<Value> = torrents(&st, &app)
        .iter()
        .filter(|g| hashes.is_empty() || hashes.contains(&g.infohash))
        .map(|g| torrent_json(&st, &app, g))
        .filter(|t| category.is_none_or(|c| t["category"] == json!(c)))
        .collect();
    Json(Value::Array(list))
}

/// The grab of app `app` with info hash `hash`, if any.
fn find(st: &AppState, app: &App, hash: &str) -> Option<Grab> {
    let h = hash.trim().to_lowercase();
    torrents(st, app).into_iter().find(|g| g.infohash == h)
}

/// qBittorrent's answer for a hash it doesn't have.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

async fn properties(State(st): State<AppState>, QbitApp(app): QbitApp, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(g) = find(&st, &app, q.get("hash").map(String::as_str).unwrap_or("")) else { return not_found() };
    let t = torrent_json(&st, &app, &g);
    Json(json!({
        "save_path": t["save_path"], "seeding_time": g.seed_secs, "share_ratio": g.ratio, "total_size": t["size"],
        "addition_date": t["added_on"], "completion_date": t["completion_on"], "eta": t["eta"], "dl_speed": g.speed,
        "time_elapsed": t["time_active"], "pieces_have": 0, "pieces_num": 0, "comment": "", "is_private": false,
    }))
    .into_response()
}

async fn files(State(st): State<AppState>, QbitApp(app): QbitApp, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(g) = find(&st, &app, q.get("hash").map(String::as_str).unwrap_or("")) else { return not_found() };
    let parent = g.path.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default();
    let list: Vec<Value> = g
        .files
        .iter()
        .enumerate()
        .map(|(i, (p, size))| {
            let name = p.strip_prefix(&format!("{parent}/")).unwrap_or(p);
            json!({ "index": i, "name": name, "size": size, "progress": g.progress, "priority": 1, "is_seed": g.progress >= 1.0 })
        })
        .collect();
    Json(Value::Array(list)).into_response()
}

async fn delete(State(st): State<AppState>, QbitApp(app): QbitApp, Form(f): Form<HashMap<String, String>>) -> Response {
    let delete_files = f.get("deleteFiles").is_some_and(|v| v == "true");
    let hashes = f.get("hashes").map(String::as_str).unwrap_or("");
    for g in torrents(&st, &app) {
        if hashes == "all" || hashes.split('|').any(|h| h.trim().eq_ignore_ascii_case(&g.infohash)) {
            if let Err(e) = st.hub.remove_grab(g.id, delete_files).await {
                tracing::warn!("{}: {}", app.name, e.detail);
            }
        }
    }
    "".into_response()
}

async fn set_category(State(st): State<AppState>, QbitApp(app): QbitApp, Form(f): Form<HashMap<String, String>>) -> Response {
    let category = f.get("category").cloned().unwrap_or_default();
    let hashes = f.get("hashes").map(String::as_str).unwrap_or("");
    for g in torrents(&st, &app) {
        if hashes == "all" || hashes.split('|').any(|h| h.trim().eq_ignore_ascii_case(&g.infohash)) {
            let _ = st.hub.set_ext_category_of(g.id, &category);
        }
    }
    "".into_response()
}

async fn maindata(State(st): State<AppState>, QbitApp(app): QbitApp) -> Json<Value> {
    let mut t = serde_json::Map::new();
    for g in torrents(&st, &app) {
        t.insert(g.infohash.clone(), torrent_json(&st, &app, &g));
    }
    let Json(cats) = categories(State(st.clone()), QbitApp(app)).await;
    Json(
        json!({ "rid": 1, "full_update": true, "torrents": t, "categories": cats, "tags": [], "server_state": { "connection_status": "connected" } }),
    )
}
