//! SABnzbd's API (`/api` and `/sabnzbd/api`, `?mode=…&apikey=…&output=json`), as Sonarr, Radarr, Lidarr and
//! Readarr use it. The API key is Lantern's. The app is the one named by `ma_username` when the app sends a user
//! name, else the one that already uses the category it names, else a new app called `SABnzbd <category>`. NZB ids
//! are `SABnzbd_nzo_lantern<grab id>`; paths are in the app's own view.

use crate::hub::apps::App;
use crate::hub::clients::Content;
use crate::hub::grabs::Grab;
use crate::hub::paths;
use crate::AppState;
use axum::extract::{Multipart, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The SABnzbd version Lantern says it is.
pub const VERSION: &str = "4.3.3";

/// The prefix of the NZB ids Lantern hands out.
const NZO: &str = "SABnzbd_nzo_lantern";

/// The emulated SABnzbd's routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api", get(api).post(api)).route("/sabnzbd/api", get(api).post(api))
}

/// SABnzbd's error answer.
fn fail(why: &str) -> Response {
    Json(json!({ "status": false, "error": why })).into_response()
}

/// The grab id in an NZB id Lantern handed out.
fn grab_id(nzo: &str) -> Option<i64> {
    nzo.trim().strip_prefix(NZO)?.parse().ok()
}

/// `seconds` as SABnzbd writes time left (`1:02:03`).
fn timeleft(seconds: i64) -> String {
    let s = seconds.max(0);
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// The app a request is for, registering it when it is new (see the module's notes).
fn app_of(st: &AppState, q: &HashMap<String, String>) -> kit_server_core::Result<App> {
    let user = q.get("ma_username").map(|s| s.trim()).unwrap_or("");
    if !user.is_empty() {
        return st.hub.emulated_app(user, "sabnzbd");
    }
    let cat = q.get("cat").or_else(|| q.get("category")).map(|s| s.trim()).filter(|c| !c.is_empty() && *c != "*").unwrap_or("default");
    for a in st.hub.apps()? {
        if a.source == "sabnzbd" && st.hub.ext_categories(a.id).iter().any(|c| c == cat) {
            st.hub.seen(a.id);
            return Ok(a);
        }
    }
    let app = st.hub.emulated_app(&format!("SABnzbd {cat}"), "sabnzbd")?;
    st.hub.set_ext_category(app.id, cat, true)?;
    Ok(app)
}

/// The apps whose downloads a request may see: the one it names, or every app that uses the emulated SABnzbd.
fn apps_of(st: &AppState, q: &HashMap<String, String>) -> Vec<App> {
    let user = q.get("ma_username").map(|s| s.trim()).unwrap_or("");
    let all = st.hub.apps().unwrap_or_default();
    if user.is_empty() {
        all.into_iter().filter(|a| a.source == "sabnzbd").collect()
    } else {
        all.into_iter().filter(|a| a.name.eq_ignore_ascii_case(user)).collect()
    }
}

/// The Usenet grabs of `apps` that SABnzbd would still list.
fn nzbs(st: &AppState, apps: &[App]) -> Vec<(App, Grab)> {
    let mut out = Vec::new();
    for a in apps {
        for g in st.hub.grabs(Some(a.id), None, 1000).unwrap_or_default() {
            if g.protocol == "usenet" && g.state != "removed" {
                out.push((a.clone(), g));
            }
        }
    }
    out
}

/// The category a grab shows.
fn cat_of(app: &App, g: &Grab) -> String {
    if g.ext_category.is_empty() {
        app.category.clone()
    } else {
        g.ext_category.clone()
    }
}

async fn api(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>, m: Option<Multipart>) -> Response {
    let key = st.settings.str("api_key");
    let given = q.get("apikey").map(String::as_str).unwrap_or("");
    let mode = q.get("mode").map(String::as_str).unwrap_or("");
    if mode == "version" {
        return Json(json!({ "version": VERSION })).into_response();
    }
    if key.is_empty() || !crate::auth::ct_eq(given, &key) {
        return fail("API Key Incorrect");
    }
    let name = q.get("name").map(String::as_str).unwrap_or("");
    match mode {
        "auth" => Json(json!({ "auth": "apikey" })).into_response(),
        "get_config" => config(&st, &q),
        "get_cats" => {
            let mut cats = vec!["*".to_string()];
            cats.extend(st.hub.offered_categories(&apps_of(&st, &q)));
            Json(json!({ "categories": cats })).into_response()
        }
        "fullstatus" | "status" => {
            let folder = apps_of(&st, &q).first().map(|a| view_folder(&st, a)).unwrap_or_else(|| st.hub.root());
            Json(json!({ "status": { "completedir": folder, "paused": false, "speedlimit": "0" } })).into_response()
        }
        "queue" if name == "delete" => remove(&st, &q).await,
        "history" if name == "delete" => remove(&st, &q).await,
        "queue" => queue(&st, &q),
        "history" => history(&st, &q),
        "addfile" => add_file(&st, &q, m).await,
        "addurl" => add_url(&st, &q).await,
        "set_config" | "switch" | "change_cat" | "change_script" | "change_opts" | "pause" | "resume" | "retry" => {
            Json(json!({ "status": true })).into_response()
        }
        _ => fail("not implemented"),
    }
}

/// The app's folder as the app sees it.
fn view_folder(st: &AppState, app: &App) -> String {
    let folder = st.hub.folder(app);
    paths::to_app(&folder, &folder, &app.import_path)
}

fn config(st: &AppState, q: &HashMap<String, String>) -> Response {
    let apps = apps_of(st, q);
    let mut cats = vec![json!({ "name": "*", "order": 0, "pp": "3", "script": "Default", "dir": "", "priority": -100 })];
    let dir = apps.first().map(|a| view_folder(st, a)).unwrap_or_default();
    for (i, c) in st.hub.offered_categories(&apps).iter().enumerate() {
        cats.push(json!({ "name": c, "order": i + 1, "pp": "", "script": "Default", "dir": dir, "priority": -100 }));
    }
    let complete = apps.first().map(|a| view_folder(st, a)).unwrap_or_else(|| st.hub.root());
    Json(json!({ "config": {
        "misc": { "complete_dir": complete, "download_dir": complete, "enable_tv_sorting": false, "enable_movie_sorting": false,
                  "enable_date_sorting": false, "pre_check": false, "history_retention": "", "history_retention_option": "all" },
        "categories": cats, "sorters": [],
    } }))
    .into_response()
}

fn queue(st: &AppState, q: &HashMap<String, String>) -> Response {
    let slots: Vec<Value> = nzbs(st, &apps_of(st, q))
        .iter()
        .filter(|(_, g)| matches!(g.state.as_str(), "queued" | "waiting" | "downloading" | "paused"))
        .enumerate()
        .map(|(i, (a, g))| {
            let mb = g.size as f64 / 1_048_576.0;
            let status = match g.state.as_str() {
                "downloading" => "Downloading",
                "paused" => "Paused",
                _ => "Queued",
            };
            json!({
                "index": i, "nzo_id": format!("{NZO}{}", g.id), "filename": g.title, "cat": cat_of(a, g), "status": status,
                "mb": format!("{mb:.2}"), "mbleft": format!("{:.2}", mb * (1.0 - g.progress)),
                "percentage": format!("{}", (g.progress * 100.0) as i64), "timeleft": timeleft(g.eta), "priority": "Normal",
            })
        })
        .collect();
    Json(json!({ "queue": { "paused": false, "noofslots": slots.len(), "slots": slots } })).into_response()
}

fn history(st: &AppState, q: &HashMap<String, String>) -> Response {
    let want = q.get("category").filter(|c| !c.is_empty() && *c != "*");
    let slots: Vec<Value> = nzbs(st, &apps_of(st, q))
        .iter()
        .filter(|(_, g)| matches!(g.state.as_str(), "completed" | "seeding" | "failed"))
        .filter(|(a, g)| want.is_none_or(|c| *c == cat_of(a, g)))
        .map(|(a, g)| {
            let folder = st.hub.folder(a);
            json!({
                "nzo_id": format!("{NZO}{}", g.id), "name": g.title, "nzb_name": format!("{}.nzb", g.title), "category": cat_of(a, g),
                "status": if g.state == "failed" { "Failed" } else { "Completed" }, "fail_message": g.error, "bytes": g.size,
                "storage": if g.path.is_empty() { String::new() } else { paths::to_app(&g.path, &folder, &a.import_path) },
                "download_time": (g.finished - g.added).max(0.0) as i64, "completed": g.finished as i64,
            })
        })
        .collect();
    Json(json!({ "history": { "noofslots": slots.len(), "slots": slots } })).into_response()
}

async fn remove(st: &AppState, q: &HashMap<String, String>) -> Response {
    let delete = q.get("del_files").is_some_and(|v| v == "1");
    let value = q.get("value").map(String::as_str).unwrap_or("");
    let apps = apps_of(st, q);
    for (_, g) in nzbs(st, &apps) {
        if value == "all" || value.split(',').filter_map(grab_id).any(|id| id == g.id) {
            if let Err(e) = st.hub.remove_grab(g.id, delete).await {
                tracing::warn!("SABnzbd: {}", e.detail);
            }
        }
    }
    Json(json!({ "status": true })).into_response()
}

/// Queues `bytes` for the request's app; SABnzbd's answer.
fn queue_nzb(st: &AppState, q: &HashMap<String, String>, bytes: Vec<u8>, file_name: &str) -> Response {
    if !crate::indexers::cardigann::looks_like_nzb(&bytes) {
        return fail("This is not an NZB file");
    }
    let app = match app_of(st, q) {
        Ok(a) => a,
        Err(e) => return fail(&e.detail),
    };
    let cat = q.get("cat").map(|c| c.trim().to_string()).filter(|c| !c.is_empty() && c != "*").unwrap_or_default();
    if !cat.is_empty() {
        let _ = st.hub.set_ext_category(app.id, &cat, true);
    }
    let title =
        q.get("nzbname").filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| file_name.trim_end_matches(".nzb").to_string());
    match st.hub.grab_content(&app, Content::Nzb(bytes), &title, &cat) {
        Ok(g) => {
            let hub = st.hub.clone();
            tokio::spawn(async move { hub.tick().await });
            Json(json!({ "status": true, "nzo_ids": [format!("{NZO}{}", g.id)] })).into_response()
        }
        Err(e) => fail(&e.detail),
    }
}

async fn add_file(st: &AppState, q: &HashMap<String, String>, m: Option<Multipart>) -> Response {
    let Some(mut m) = m else { return fail("No NZB file was sent") };
    while let Ok(Some(field)) = m.next_field().await {
        if matches!(field.name(), Some("name" | "nzbfile")) {
            let file = field.file_name().unwrap_or("release.nzb").to_string();
            let bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            return queue_nzb(st, q, bytes, &file);
        }
    }
    fail("No NZB file was sent")
}

async fn add_url(st: &AppState, q: &HashMap<String, String>) -> Response {
    let url = q.get("name").map(String::as_str).unwrap_or("");
    match st.hub.fetch_url(url).await {
        Ok(Content::Nzb(bytes)) => queue_nzb(st, q, bytes, url.rsplit('/').next().unwrap_or("release.nzb")),
        Ok(_) => fail("The address is not an NZB file"),
        Err(e) => fail(&e),
    }
}
