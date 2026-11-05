// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Logs/LogController.cs, LogFileController.cs,
// LogFileControllerBase.cs, UpdateLogFileController.cs, LogResource.cs, LogFileResource.cs,
// src/Prowlarr.Http/PagingResource.cs), GPL-3.0.

//! The log: its entries page by page (`GET /api/v1/log`, from the `prowlarr_logs` table) and the log files in the
//! data folder's `logs` and `UpdateLogs` folders (`GET /api/v1/log/file`, `/api/v1/log/file/update`).

use super::{config, date, ApiResult, V1};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{Db, RowExt};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The log routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/log", get(log))
        .route("/api/v1/log/file", get(files))
        .route("/api/v1/log/file/{filename}", get(file))
        .route("/api/v1/log/file/update", get(update_files))
        .route("/api/v1/log/file/update/{filename}", get(update_file))
}

/// The levels a `level` filter shows: that level and every more serious one.
fn levels_for(level: &str) -> Option<&'static [&'static str]> {
    const ALL: [&str; 6] = ["Fatal", "Error", "Warn", "Info", "Debug", "Trace"];
    let n = match level {
        "fatal" => 1,
        "error" => 2,
        "warn" => 3,
        "info" => 4,
        "debug" => 5,
        "trace" => 6,
        _ => return None,
    };
    Some(&ALL[..n])
}

/// A sort direction as Prowlarr reads it: its name in any case, or its number.
fn sort_direction(v: Option<&String>) -> Option<&'static str> {
    match v?.trim().to_lowercase().as_str() {
        "default" | "0" => Some("default"),
        "ascending" | "1" => Some("ascending"),
        "descending" | "2" => Some("descending"),
        _ => None,
    }
}

/// `GET /api/v1/log?page=&pageSize=&sortKey=&sortDirection=&level=`: one page of log entries (newest first unless
/// asked otherwise), as Prowlarr's `PagingResource<LogResource>`.
async fn log(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>) -> ApiResult<Json<Value>> {
    let page: i64 = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
    let page_size: i64 = q.get("pageSize").and_then(|v| v.parse().ok()).unwrap_or(10);
    let asked = sort_direction(q.get("sortDirection")).unwrap_or("descending");
    let direction = if asked == "default" { "ascending" } else { asked };
    // only id and time can be sorted on, and time sorts by id
    let filter = q.get("level").and_then(|l| levels_for(l));
    let (where_sql, params) = match filter {
        Some(levels) => (format!("WHERE level IN ({})", vec!["?"; levels.len()].join(",")), levels.iter().map(|l| json!(l)).collect()),
        None => (String::new(), Vec::new()),
    };
    let total = st.db.one(&format!("SELECT COUNT(*) AS n FROM prowlarr_logs {where_sql}"), params.clone())?.map(|r| r.i("n")).unwrap_or(0);
    let order = if direction == "ascending" { "ASC" } else { "DESC" };
    let mut page_params = params;
    page_params.push(json!(page_size.max(0)));
    page_params.push(json!(((page - 1) * page_size).max(0)));
    let rows = st.db.query(&format!("SELECT * FROM prowlarr_logs {where_sql} ORDER BY id {order} LIMIT ? OFFSET ?"), page_params)?;
    let records: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = json!({
                "id": r.i("id"),
                "time": date(r.f("time")),
                "level": r.s("level").to_lowercase(),
                "logger": r.s("logger"),
                "message": r.s("message"),
            });
            for (key, col) in [("exception", "exception"), ("exceptionType", "exception_type")] {
                if r.get(col).is_some_and(|x| !x.is_null()) {
                    v[key] = json!(r.s(col));
                }
            }
            v
        })
        .collect();
    Ok(Json(json!({
        "page": page,
        "pageSize": page_size,
        "sortKey": "time",
        "sortDirection": direction,
        "totalRecords": total,
        "records": records,
    })))
}

/// Prowlarr's log file names: letters, digits, `-` and `.`, ending in `.txt`.
fn is_log_name(name: &str) -> bool {
    name.len() > 4 && name.ends_with(".txt") && name[..name.len() - 4].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// The files in `folder` as Prowlarr's `LogFileResource`, newest first; `resource` and `download` make their
/// addresses as Prowlarr makes them (the main log's contents address has an empty part, `/api/v1//name`, as in
/// Prowlarr).
fn list(st: &AppState, folder: &std::path::Path, only_logs: bool, resource: &str, download: &str) -> Vec<Value> {
    let base = config::url_base(&st.db);
    let mut names: Vec<(String, f64)> = std::fs::read_dir(folder)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().is_file())
                .map(|e| {
                    let modified = e
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    (e.file_name().to_string_lossy().into_owned(), modified)
                })
                .filter(|(n, _)| !only_logs || is_log_name(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<(f64, Value)> = names
        .iter()
        .enumerate()
        .map(|(i, (name, modified))| {
            (
                *modified,
                json!({
                    "id": i + 1,
                    "filename": name,
                    "lastWriteTime": date(*modified),
                    "contentsUrl": format!("{base}/api/v1/{resource}/{name}"),
                    "downloadUrl": format!("{base}/{download}/{name}"),
                }),
            )
        })
        .collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().map(|(_, v)| v).collect()
}

/// A log file's text; 404 when the name is not a log file's or there is no such file.
fn contents(folder: &std::path::Path, name: &str) -> Response {
    if !is_log_name(name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match std::fs::read(folder.join(name)) {
        Ok(bytes) => ([(header::CONTENT_TYPE, "text/plain")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The main log folder: `<data>/logs`.
pub fn log_folder(st: &AppState) -> std::path::PathBuf {
    st.data_dir.join("logs")
}

/// The update log folder: `<data>/UpdateLogs`.
pub fn update_log_folder(st: &AppState) -> std::path::PathBuf {
    st.data_dir.join("UpdateLogs")
}

/// `GET /api/v1/log/file`.
async fn files(State(st): State<AppState>, _: V1) -> Json<Value> {
    Json(Value::Array(list(&st, &log_folder(&st), false, "", "logfile")))
}

/// `GET /api/v1/log/file/{filename}`.
async fn file(State(st): State<AppState>, _: V1, Path(name): Path<String>) -> Response {
    contents(&log_folder(&st), &name)
}

/// `GET /api/v1/log/file/update`.
async fn update_files(State(st): State<AppState>, _: V1) -> Json<Value> {
    Json(Value::Array(list(&st, &update_log_folder(&st), true, "update", "updatelogfile")))
}

/// `GET /api/v1/log/file/update/{filename}`.
async fn update_file(State(st): State<AppState>, _: V1, Path(name): Path<String>) -> Response {
    contents(&update_log_folder(&st), &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_include_the_more_serious_ones() {
        assert_eq!(levels_for("warn"), Some(&["Fatal", "Error", "Warn"][..]));
        assert_eq!(levels_for("all"), None);
    }

    #[test]
    fn log_names_follow_prowlarrs_route() {
        assert!(is_log_name("prowlarr.0.txt") && is_log_name("lantern-debug.txt"));
        assert!(!is_log_name("../etc/passwd") && !is_log_name("x.log") && !is_log_name(".txt"));
    }
}
