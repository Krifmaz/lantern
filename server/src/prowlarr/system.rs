// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/System/SystemController.cs, System/SystemResource.cs,
// System/Tasks/TaskController.cs, System/Backup/BackupController.cs, Update/UpdateController.cs,
// src/Prowlarr.Http/Ping/PingController.cs, src/NzbDrone.Core/Jobs/TaskManager.cs,
// src/NzbDrone.Core/Backup/BackupService.cs, src/NzbDrone.Common/Crypto/HashConverter.cs), GPL-3.0.

//! What is running and what it keeps: `/ping`, `/api/v1/system/status`, `/system/routes`, the scheduled tasks
//! (`/system/task`), the backups in the backup folder (`/system/backup`) and the releases (`/update`).

use super::{config, date, split_camel_case, timespan, ApiResult, APP_NAME, V1};
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::sync::OnceLock;

/// When this server started (seconds since 1970), set when its tables are opened.
static STARTED: OnceLock<f64> = OnceLock::new();

/// .NET's `DateTime.MinValue` (0001-01-01) in seconds since 1970: what a date that was never set shows.
pub const MIN_DATE: f64 = -62_135_596_800.0;

/// Prowlarr's scheduled tasks, in the order a new Prowlarr saves them (so their ids match), with their interval
/// in minutes (the backup's comes from the `BackupInterval` setting, in days).
pub const TASKS: [(&str, i64); 9] = [
    ("NzbDrone.Core.Messaging.Commands.MessagingCleanupCommand", 5),
    ("NzbDrone.Core.Caching.ClearExpiredCacheCommand", 5),
    ("NzbDrone.Core.Update.Commands.ApplicationUpdateCheckCommand", 6 * 60),
    ("NzbDrone.Core.HealthCheck.CheckHealthCommand", 6 * 60),
    ("NzbDrone.Core.Housekeeping.HousekeepingCommand", 24 * 60),
    ("NzbDrone.Core.History.CleanUpHistoryCommand", 24 * 60),
    ("NzbDrone.Core.IndexerVersions.IndexerDefinitionUpdateCommand", 24 * 60),
    ("NzbDrone.Core.Applications.ApplicationIndexerSyncCommand", 6 * 60),
    ("NzbDrone.Core.Backup.BackupCommand", 0),
];

/// The system routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ping", get(ping).head(ping))
        .route("/api/v1/system/status", get(status))
        .route("/api/v1/system/routes", get(routes))
        .route("/api/v1/system/task", get(tasks))
        .route("/api/v1/system/task/{id}", get(task))
        .route("/api/v1/system/backup", get(backups))
        .route("/api/v1/update", get(updates))
}

/// Saves Prowlarr's scheduled tasks the first time (last run: now), and sets their intervals as Prowlarr does at
/// every start.
pub fn seed_tasks(db: &SqliteDb) -> kit_server_core::Result<()> {
    let now = crate::auth::now();
    let _ = STARTED.set(now);
    for (type_name, interval) in TASKS {
        let interval = if interval == 0 { backup_interval(db) } else { interval };
        db.execute(
            "INSERT INTO prowlarr_tasks (type_name, interval, last_execution) VALUES (?, ?, ?)
             ON CONFLICT(type_name) DO UPDATE SET interval=excluded.interval",
            p![type_name, interval, now],
        )?;
    }
    Ok(())
}

/// The backup task's interval in minutes: the `BackupInterval` setting (days, 1 to 7).
pub fn backup_interval(db: &SqliteDb) -> i64 {
    let days: i64 = super::config_get(db, "backupinterval").and_then(|v| v.parse().ok()).unwrap_or(7);
    days.clamp(1, 7) * 60 * 24
}

/// `GET`/`HEAD /ping`: `{"status": "OK"}` while the database answers, else 500 `{"status": "Error"}`. No sign-in.
async fn ping(State(st): State<AppState>) -> Response {
    match st.db.one("SELECT key FROM prowlarr_config LIMIT 1", p![]) {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "OK" }))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "status": "Error" }))).into_response(),
    }
}

/// `/etc/os-release`'s `ID` and `VERSION_ID`, as Prowlarr reads the system's name and version on Linux.
fn os_release() -> (String, String) {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let value = |key: &str| {
        text.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).map(|v| v.trim().trim_matches('"').to_string()).unwrap_or_default()
    };
    let name = value("ID");
    (if name.is_empty() { std::env::consts::OS.to_string() } else { name }, value("VERSION_ID"))
}

/// Whether this runs in Docker, as Prowlarr checks it.
fn is_docker() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup").map(|c| c.contains("/docker/")).unwrap_or(false)
}

/// `GET /api/v1/system/status`: Prowlarr's `SystemResource`, with Lantern's values.
pub async fn status(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(status_json(&st)))
}

/// The status answer.
pub fn status_json(st: &AppState) -> Value {
    let exe = std::env::current_exe().ok();
    let startup = exe.as_ref().and_then(|e| e.parent()).map(|p| p.display().to_string()).unwrap_or_default();
    let built = exe
        .as_ref()
        .and_then(|e| std::fs::metadata(e).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(MIN_DATE);
    let (os_name, os_version) = os_release();
    let docker = is_docker();
    let podman = std::path::Path::new("/run/.containerenv").exists() || std::env::var("container").is_ok();
    let sqlite = st.db.one("SELECT sqlite_version() AS v", p![]).ok().flatten().map(|r| r.s("v")).unwrap_or_default();
    json!({
        "appName": APP_NAME,
        "instanceName": config::instance_name(st),
        "version": crate::VERSION,
        "buildTime": date(built),
        "isDebug": cfg!(debug_assertions),
        "isProduction": !cfg!(debug_assertions),
        "isAdmin": false,
        "isUserInteractive": false,
        "startupPath": startup,
        "appData": st.data_dir.display().to_string(),
        "osName": os_name,
        "osVersion": os_version,
        "isNetCore": false,
        "isLinux": cfg!(target_os = "linux"),
        "isOsx": cfg!(target_os = "macos"),
        "isWindows": cfg!(target_os = "windows"),
        "isDocker": docker,
        "isContainerized": docker || podman,
        "mode": "console",
        "branch": config::branch(&st.db),
        "databaseType": "sqLite",
        "databaseVersion": sqlite,
        "authentication": config::authentication_method(&st.db),
        "migrationVersion": 0,
        "urlBase": config::url_base(&st.db),
        "runtimeVersion": "0.0",
        "runtimeName": "rust",
        "startTime": date(*STARTED.get().unwrap_or(&0.0)),
        "packageVersion": crate::VERSION,
        "packageAuthor": APP_NAME,
        "packageUpdateMechanism": "docker",
        "packageUpdateMechanismMessage": "Lantern is updated by pulling its new Docker image.",
    })
}

/// Every route this API answers, for `/api/v1/system/routes`.
const ROUTES: [&str; 30] = [
    "GET /ping",
    "HEAD /ping",
    "GET /api/v1/system/status",
    "GET /api/v1/system/routes",
    "GET /api/v1/system/task",
    "GET /api/v1/system/task/{id}",
    "GET /api/v1/system/backup",
    "GET /api/v1/update",
    "GET /api/v1/config/host",
    "GET /api/v1/config/host/{id}",
    "PUT /api/v1/config/host/{id}",
    "GET /api/v1/config/ui",
    "GET /api/v1/config/ui/{id}",
    "PUT /api/v1/config/ui/{id}",
    "GET /api/v1/config/development",
    "GET /api/v1/config/development/{id}",
    "PUT /api/v1/config/development/{id}",
    "GET POST /api/v1/tag",
    "GET PUT DELETE /api/v1/tag/{id}",
    "GET /api/v1/tag/detail",
    "GET /api/v1/tag/detail/{id}",
    "GET POST /api/v1/customfilter",
    "GET PUT DELETE /api/v1/customfilter/{id}",
    "GET /api/v1/localization",
    "GET /api/v1/localization/options",
    "GET /api/v1/health",
    "GET POST /api/v1/command",
    "GET DELETE /api/v1/command/{id}",
    "GET /api/v1/log",
    "GET /api/v1/log/file, /api/v1/log/file/{filename}, /api/v1/log/file/update, /api/v1/log/file/update/{filename}",
];

/// `GET /api/v1/system/routes`: the routes as a graph in plain text (Prowlarr writes ASP.NET's route graph in the
/// DOT language; this is the same kind of graph of Lantern's routes).
async fn routes(_: V1) -> Response {
    let mut out = String::from("digraph DFA {\n");
    for (i, r) in ROUTES.iter().enumerate() {
        out.push_str(&format!("{i} [label=\"{r}\"]\n"));
    }
    out.push_str("}\n");
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], out).into_response()
}

/// A scheduled task as Prowlarr's `TaskResource`.
fn task_json(r: &kit_server_core::Row) -> Value {
    let task_name = r.s("type_name").rsplit('.').next().unwrap_or("").replace("Command", "");
    let last = r.f("last_execution");
    let start = r.get("last_start_time").and_then(Value::as_f64).unwrap_or(MIN_DATE);
    json!({
        "id": r.i("id"),
        "name": split_camel_case(&task_name),
        "taskName": task_name,
        "interval": r.i("interval"),
        "lastExecution": date(last),
        "lastStartTime": date(start),
        "nextExecution": date(last + r.i("interval") as f64 * 60.0),
        "lastDuration": timespan(last - start),
    })
}

/// `GET /api/v1/system/task`: the scheduled tasks by name.
async fn tasks(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    let mut list: Vec<Value> = st.db.query("SELECT * FROM prowlarr_tasks", p![])?.iter().map(task_json).collect();
    list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(Json(Value::Array(list)))
}

/// `GET /api/v1/system/task/{id}`: one task; nothing (204) when there is no such task, as Prowlarr answers.
async fn task(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Response> {
    Ok(match st.db.one("SELECT * FROM prowlarr_tasks WHERE id=?", p![id])? {
        Some(r) => Json(task_json(&r)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

/// Marks a scheduled task as run (by its command's name, `CheckHealth`), as Prowlarr does after a command that
/// updates its task.
pub fn task_ran(db: &SqliteDb, command: &str, started: f64, ended: f64) {
    let _ = db.execute(
        "UPDATE prowlarr_tasks SET last_execution=?, last_start_time=? WHERE type_name LIKE ?",
        p![ended, started, format!("%.{command}Command")],
    );
}

/// The backup folder: the `BackupFolder` setting, inside the data folder unless it is a full path.
pub fn backup_folder(st: &AppState) -> std::path::PathBuf {
    let folder = config::backup_folder_setting(&st.db);
    let p = std::path::Path::new(&folder);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        st.data_dir.join(p)
    }
}

/// Prowlarr's `HashConverter.GetHashInt31`: the first four bytes of the SHA-1 of `text`, little-endian, as a
/// positive number.
pub fn hash_int31(text: &str) -> i64 {
    let h = Sha1::digest(text.as_bytes());
    (i32::from_le_bytes([h[0], h[1], h[2], h[3]]) & 0x7fff_ffff) as i64
}

/// `GET /api/v1/system/backup`: the backup files (Prowlarr's and Lantern's) in the backup folder's `scheduled`,
/// `manual` and `update` folders, newest first.
async fn backups(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    let re = regex::Regex::new(r"(?i)(nzbdrone|prowlarr|lantern)_backup_v?[._0-9]+\.zip").expect("a fixed pattern");
    let root = backup_folder(&st);
    let mut list: Vec<(f64, Value)> = Vec::new();
    for (kind, type_name) in [("scheduled", "Scheduled"), ("manual", "Manual"), ("update", "Update")] {
        let Ok(dir) = std::fs::read_dir(root.join(kind)) else { continue };
        for entry in dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !re.is_match(&path.to_string_lossy()) {
                continue;
            }
            let meta = entry.metadata().ok();
            let time = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            list.push((
                time,
                json!({
                    "id": hash_int31(&format!("backup-{type_name}-{name}")),
                    "name": name,
                    "path": format!("/backup/{kind}/{name}"),
                    "type": kind,
                    "size": meta.map(|m| m.len()).unwrap_or(0),
                    "time": date(time),
                }),
            ));
        }
    }
    list.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(Json(Value::Array(list.into_iter().map(|(_, v)| v).collect())))
}

/// `GET /api/v1/update`: Lantern's releases (from the release notes built into this server) as Prowlarr's
/// `UpdateResource`, newest first; the newest is `latest` and the running one `installed`.
async fn updates(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    let branch = config::branch(&st.db);
    let current = kit_server_core::version::base(crate::VERSION);
    let list: Vec<Value> = st
        .changelog
        .releases
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (mut new, mut fixed) = (Vec::new(), Vec::new());
            let mut in_fixes = false;
            for b in &r.blocks {
                match b {
                    kit_server_changelog::Block::H { text } => in_fixes = text.to_lowercase().contains("fix"),
                    kit_server_changelog::Block::Li { lead, text } => {
                        let line = if lead.is_empty() { text.clone() } else { format!("{lead} {text}") };
                        if in_fixes {
                            fixed.push(line)
                        } else {
                            new.push(line)
                        }
                    }
                    _ => {}
                }
            }
            let newer = kit_server_core::version::compare(&r.version, &current) == std::cmp::Ordering::Greater;
            json!({
                "version": r.version,
                "branch": branch,
                "releaseDate": date(MIN_DATE),
                "fileName": format!("lantern-{}.tar", r.version),
                "url": format!("https://github.com/Krifmaz/lantern/releases/tag/v{}", r.version),
                "installed": r.version == current,
                "installable": i == 0 && newer,
                "latest": i == 0,
                "changes": { "new": new, "fixed": fixed },
                "hash": "",
            })
        })
        .collect();
    Ok(Json(Value::Array(list)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_ids_like_prowlarr() {
        // worked out separately: the first four bytes of the SHA-1, little-endian, without the sign bit
        assert_eq!(hash_int31("backup-Manual-prowlarr_backup_v1.0.0.zip"), 844_208_909);
    }
}
