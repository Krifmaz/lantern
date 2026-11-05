//! NZBGet's JSON-RPC (`/jsonrpc`, also `/<user>:<password>/jsonrpc`), as Sonarr, Radarr, Lidarr and Readarr use
//! it. The app signs in with its own name and Lantern's API key (Basic auth, or in the address). NZB ids are grab
//! ids; the parameters the app attached (Sonarr's `drone`) come back with each download; paths are in the app's
//! own view.

use crate::hub::apps::App;
use crate::hub::clients::Content;
use crate::hub::grabs::Grab;
use crate::hub::paths;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};

/// The NZBGet version Lantern says it is.
pub const VERSION: &str = "24.3";

/// The emulated NZBGet's routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/jsonrpc", post(rpc)).route("/{id}/jsonrpc", post(rpc_in_path))
}

/// The app signing in with `user` and `password`, when the password is Lantern's API key.
fn app_of(st: &AppState, user: &str, password: &str) -> Option<App> {
    let key = st.settings.str("api_key");
    if key.is_empty() || !crate::auth::ct_eq(password, &key) {
        return None;
    }
    st.hub.emulated_app(user, "nzbget").ok()
}

/// The user and password of a Basic `Authorization` header.
fn basic(h: &HeaderMap) -> Option<(String, String)> {
    let v = h.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Basic ")?;
    let raw = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(v.trim()).ok()?).ok()?;
    let (u, p) = raw.split_once(':')?;
    Some((u.to_string(), p.to_string()))
}

async fn rpc(State(st): State<AppState>, h: HeaderMap, Json(body): Json<Value>) -> Response {
    let (u, p) = basic(&h).unwrap_or_default();
    answer(&st, app_of(&st, &u, &p), body).await
}

async fn rpc_in_path(State(st): State<AppState>, Path(auth): Path<String>, h: HeaderMap, Json(body): Json<Value>) -> Response {
    let (u, p) = auth.split_once(':').map(|(u, p)| (u.to_string(), p.to_string())).or_else(|| basic(&h)).unwrap_or_default();
    answer(&st, app_of(&st, &u, &p), body).await
}

/// A JSON-RPC answer with `result`.
fn ok(id: &Value, result: Value) -> Response {
    Json(json!({ "version": "1.1", "id": id, "result": result })).into_response()
}

/// A JSON-RPC error answer.
fn error(id: &Value, why: &str) -> Response {
    Json(json!({ "version": "1.1", "id": id, "error": { "name": "JSONRPCError", "code": 1, "message": why } })).into_response()
}

/// A size in bytes as NZBGet's Lo/Hi/MB fields.
fn sizes(prefix: &str, bytes: i64, out: &mut serde_json::Map<String, Value>) {
    let b = bytes.max(0) as u64;
    out.insert(format!("{prefix}Lo"), json!(b & 0xffff_ffff));
    out.insert(format!("{prefix}Hi"), json!(b >> 32));
    out.insert(format!("{prefix}MB"), json!(b / 1_048_576));
}

/// The parameters the app attached to a grab, as NZBGet lists them.
fn params(g: &Grab) -> Value {
    serde_json::from_str::<Value>(&g.ext_params).unwrap_or(json!([]))
}

async fn answer(st: &AppState, app: Option<App>, body: Value) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let Some(app) = app else {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Basic realm=\"NZBGet\"")], "Unauthorized").into_response();
    };
    let p = body.get("params").cloned().unwrap_or(json!([]));
    let folder = st.hub.folder(&app);
    let view = |x: &str| paths::to_app(x, &folder, &app.import_path);
    let grabs: Vec<Grab> = st
        .hub
        .grabs(Some(app.id), None, 1000)
        .unwrap_or_default()
        .into_iter()
        .filter(|g| g.protocol == "usenet" && g.state != "removed")
        .collect();
    let cat = |g: &Grab| if g.ext_category.is_empty() { app.category.clone() } else { g.ext_category.clone() };
    match body.get("method").and_then(Value::as_str).unwrap_or("") {
        "version" => ok(&id, json!(VERSION)),
        "config" | "loadconfig" => {
            let mut c = vec![
                json!({ "Name": "MainDir", "Value": view(&folder) }),
                json!({ "Name": "DestDir", "Value": view(&folder) }),
                json!({ "Name": "KeepHistory", "Value": "30" }),
                json!({ "Name": "AppendCategoryDir", "Value": "no" }),
            ];
            for (i, name) in st.hub.offered_categories(std::slice::from_ref(&app)).iter().enumerate() {
                c.push(json!({ "Name": format!("Category{}.Name", i + 1), "Value": name }));
                c.push(json!({ "Name": format!("Category{}.DestDir", i + 1), "Value": view(&folder) }));
            }
            ok(&id, Value::Array(c))
        }
        "status" => ok(&id, json!({ "DownloadRate": 0, "DownloadPaused": false, "ServerStandBy": true, "FreeDiskSpaceMB": 0 })),
        "append" => {
            // NZBGet 16+: name, content, category, priority, add to top, add paused, dupe key, dupe score, dupe mode, parameters
            let name = p.get(0).and_then(Value::as_str).unwrap_or("release.nzb").to_string();
            let content = p.get(1).and_then(Value::as_str).unwrap_or("");
            let category = p.get(2).and_then(Value::as_str).unwrap_or("").trim().to_string();
            let bytes = if content.starts_with("http://") || content.starts_with("https://") {
                match st.hub.fetch_url(content).await {
                    Ok(Content::Nzb(b)) => b,
                    _ => return ok(&id, json!(0)),
                }
            } else {
                base64::engine::general_purpose::STANDARD.decode(content).unwrap_or_default()
            };
            if !crate::indexers::cardigann::looks_like_nzb(&bytes) {
                return ok(&id, json!(0));
            }
            if !category.is_empty() {
                let _ = st.hub.set_ext_category(app.id, &category, true);
            }
            match st.hub.grab_content(&app, Content::Nzb(bytes), name.trim_end_matches(".nzb"), &category) {
                Ok(g) => {
                    let _ = st.hub.set_ext_params(g.id, p.get(9).cloned().unwrap_or(json!([])));
                    let hub = st.hub.clone();
                    tokio::spawn(async move { hub.tick().await });
                    ok(&id, json!(g.id))
                }
                Err(_) => ok(&id, json!(0)),
            }
        }
        "listgroups" => {
            let list: Vec<Value> = grabs
                .iter()
                .filter(|g| matches!(g.state.as_str(), "queued" | "waiting" | "downloading" | "paused"))
                .map(|g| {
                    let mut m = serde_json::Map::new();
                    sizes("FileSize", g.size, &mut m);
                    sizes("RemainingSize", (g.size as f64 * (1.0 - g.progress)) as i64, &mut m);
                    sizes("DownloadedSize", (g.size as f64 * g.progress) as i64, &mut m);
                    let status = match g.state.as_str() {
                        "downloading" => "DOWNLOADING",
                        "paused" => "PAUSED",
                        _ => "QUEUED",
                    };
                    m.extend([
                        ("NZBID".to_string(), json!(g.id)),
                        ("NZBName".to_string(), json!(g.title)),
                        ("NZBFilename".to_string(), json!(format!("{}.nzb", g.title))),
                        ("Category".to_string(), json!(cat(g))),
                        ("Status".to_string(), json!(status)),
                        ("DestDir".to_string(), json!(view(&folder))),
                        ("ActiveDownloads".to_string(), json!(if g.state == "downloading" { 1 } else { 0 })),
                        ("MaxPriority".to_string(), json!(0)),
                        ("PostTotalTimeSec".to_string(), json!(0)),
                        ("Parameters".to_string(), params(g)),
                    ]);
                    Value::Object(m)
                })
                .collect();
            ok(&id, Value::Array(list))
        }
        "history" => {
            let list: Vec<Value> = grabs
                .iter()
                .filter(|g| matches!(g.state.as_str(), "completed" | "seeding" | "failed"))
                .map(|g| {
                    let mut m = serde_json::Map::new();
                    sizes("FileSize", g.size, &mut m);
                    let failed = g.state == "failed";
                    let dir = if g.path.is_empty() { view(&folder) } else { view(&g.path) };
                    m.extend([
                        ("NZBID".to_string(), json!(g.id)),
                        ("Name".to_string(), json!(g.title)),
                        ("NZBName".to_string(), json!(g.title)),
                        ("Category".to_string(), json!(cat(g))),
                        ("Status".to_string(), json!(if failed { "FAILURE/HEALTH" } else { "SUCCESS/ALL" })),
                        ("ParStatus".to_string(), json!(if failed { "FAILURE" } else { "SUCCESS" })),
                        ("UnpackStatus".to_string(), json!(if failed { "NONE" } else { "SUCCESS" })),
                        ("MoveStatus".to_string(), json!(if failed { "NONE" } else { "SUCCESS" })),
                        ("ScriptStatus".to_string(), json!("NONE")),
                        ("DeleteStatus".to_string(), json!(if failed { "HEALTH" } else { "NONE" })),
                        ("MarkStatus".to_string(), json!("NONE")),
                        ("DestDir".to_string(), json!(dir)),
                        ("FinalDir".to_string(), json!(dir)),
                        ("HistoryTime".to_string(), json!(g.finished as i64)),
                        ("FailMessage".to_string(), json!(g.error)),
                        ("Parameters".to_string(), params(g)),
                    ]);
                    Value::Object(m)
                })
                .collect();
            ok(&id, Value::Array(list))
        }
        "editqueue" => {
            let command = p.get(0).and_then(Value::as_str).unwrap_or("");
            // NZBGet 16+: command, param, ids; older: command, offset, param, ids
            let ids: Vec<i64> = p
                .as_array()
                .and_then(|a| a.iter().rev().find_map(Value::as_array))
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let delete = matches!(command, "GroupFinalDelete" | "HistoryFinalDelete");
            if matches!(
                command,
                "GroupDelete" | "GroupFinalDelete" | "HistoryDelete" | "HistoryFinalDelete" | "GroupDupeDelete" | "HistoryMarkBad"
            ) {
                for g in grabs.iter().filter(|g| ids.contains(&g.id)) {
                    if let Err(e) = st.hub.remove_grab(g.id, delete).await {
                        tracing::warn!("NZBGet: {}", e.detail);
                    }
                }
            }
            ok(&id, json!(true))
        }
        "pausedownload" | "resumedownload" | "rate" | "scheduleresume" | "writelog" => ok(&id, json!(true)),
        m => error(&id, &format!("Invalid procedure {m}")),
    }
}
