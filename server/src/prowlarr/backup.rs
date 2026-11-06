// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Backup/BackupService.cs, Backup.cs, BackupCommand.cs,
// MakeDatabaseBackup.cs, RestoreBackupFailedException.cs, src/NzbDrone.Core/Datastore/DatabaseRestorationService.cs,
// src/Prowlarr.Api.V1/System/Backup/BackupController.cs, BackupResource.cs), GPL-3.0.

//! Backups of Lantern's data in Prowlarr's way: zip files in the backup folder's `scheduled`, `manual` and
//! `update` folders, made by the `Backup` command (on its schedule, or when asked), listed, downloaded, deleted
//! and restored through Prowlarr's API.
//!
//! ```text
//! GET    /api/v1/system/backup                  the backups, newest first
//! DELETE /api/v1/system/backup/{id}             deletes one
//! POST   /api/v1/system/backup/restore/{id}     restores one (applied at the next start)
//! POST   /api/v1/system/backup/restore/upload   restores an uploaded .zip or .db (applied at the next start)
//! GET    /backup/{type}/{name}                  downloads one
//! ```
//!
//! A backup zip holds what Prowlarr's holds, mapped to Lantern: `app.db` (a copy of the database, where Prowlarr
//! has `prowlarr.db`) and `INFO` (the version and the time). Lantern keeps no `config.xml`: its settings are in the
//! database. The vault's items are in the database sealed, as they are at rest; `vault.key` is not put in the zip,
//! so a stolen backup opens no saved logins (the vault's own encrypted backup is the way to move them).
//!
//! Restoring only ever happens on one of the two restore calls: the database is put aside as
//! `lantern.restore` and takes the place of `app.db` at the next start, as Prowlarr's `prowlarr.restore` does.
//! The database it replaces is kept as `app.db.before-restore`.

use super::{api_exception, config, date, fail, system, ApiError, ApiResult, V1};
use crate::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kit_server_core::{err, p, Db};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

/// The biggest backup an upload takes.
const MAX_UPLOAD: usize = 2 << 30;

/// The file a restored database waits in until the next start (Prowlarr's `prowlarr.restore`).
pub const RESTORE_FILE: &str = "lantern.restore";

/// The copy of the database a restore replaced.
pub const REPLACED_FILE: &str = "app.db.before-restore";

/// The first bytes of every SQLite database.
const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";

/// Which folder a backup is in (Prowlarr's `BackupType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Made by the scheduler; old ones are deleted after the retention.
    Scheduled,
    /// Made when asked; kept until deleted.
    Manual,
    /// Made before an update (Lantern makes none; Prowlarr's are listed).
    Update,
}

impl Kind {
    /// Every kind, in Prowlarr's order.
    pub const ALL: [Kind; 3] = [Kind::Scheduled, Kind::Manual, Kind::Update];

    /// The kind's name as Prowlarr writes the enum (`Scheduled`), used in the backup's id.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Scheduled => "Scheduled",
            Kind::Manual => "Manual",
            Kind::Update => "Update",
        }
    }

    /// The kind's folder and its name in the API (`scheduled`).
    pub fn folder(self) -> &'static str {
        match self {
            Kind::Scheduled => "scheduled",
            Kind::Manual => "manual",
            Kind::Update => "update",
        }
    }

    /// The kind named `name` (its folder name, in any case).
    fn from_folder(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.folder().eq_ignore_ascii_case(name))
    }
}

/// One backup file (Prowlarr's `Backup`).
#[derive(Debug, Clone)]
pub struct Backup {
    /// Its file name.
    pub name: String,
    /// Its folder.
    pub kind: Kind,
    /// Its size in bytes.
    pub size: u64,
    /// When it was last written, seconds since 1970.
    pub time: f64,
}

impl Backup {
    /// Prowlarr's id for it: `GetHashInt31("backup-<Type>-<name>")`.
    pub fn id(&self) -> i64 {
        system::hash_int31(&format!("backup-{}-{}", self.kind.name(), self.name))
    }

    /// The backup as Prowlarr's `BackupResource`.
    pub fn json(&self) -> Value {
        json!({
            "id": self.id(),
            "name": self.name,
            "path": format!("/backup/{}/{}", self.kind.folder(), self.name),
            "type": self.kind.folder(),
            "size": self.size,
            "time": date(self.time),
        })
    }
}

/// The backup routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/system/backup", get(list_route))
        .route("/api/v1/system/backup/{id}", delete(delete_route))
        .route("/api/v1/system/backup/restore/{id}", post(restore_route))
        .route("/api/v1/system/backup/restore/upload", post(upload_route).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/backup/{kind}/{name}", get(download_route))
}

/// Prowlarr's `BackupFileRegex`, with Lantern's own name next to Prowlarr's.
fn is_backup_file(path: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)(nzbdrone|prowlarr|lantern)_backup_v?[._0-9]+\.zip").expect("a fixed pattern")).is_match(path)
}

/// The folder of backups of `kind`.
pub fn folder_for(st: &AppState, kind: Kind) -> PathBuf {
    system::backup_folder(st).join(kind.folder())
}

/// A 500 in words when `path` is outside the data folder: Lantern writes nowhere else.
fn inside_data(st: &AppState, path: &std::path::Path) -> kit_server_core::Result<()> {
    if path.starts_with(&st.data_dir) {
        return Ok(());
    }
    Err(err(
        500,
        format!(
            "The backup folder {} is outside Lantern's data folder {}, where Lantern can't write. Set Backup Folder to a folder inside the data folder, such as Backups.",
            path.display(),
            st.data_dir.display()
        ),
    ))
}

/// Seconds since 1970 of a file's last write.
fn modified(meta: &std::fs::Metadata) -> f64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The backup files in one folder (`GetBackupFiles`).
fn files_in(folder: &std::path::Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let Ok(dir) = std::fs::read_dir(folder) else { return Vec::new() };
    dir.flatten()
        .filter(|e| is_backup_file(&e.path().to_string_lossy()))
        .filter_map(|e| e.metadata().ok().filter(|m| m.is_file()).map(|m| (e.path(), m)))
        .collect()
}

/// Every backup (`GetBackups`), in no particular order.
pub fn all(st: &AppState) -> Vec<Backup> {
    let mut out = Vec::new();
    for kind in Kind::ALL {
        for (path, meta) in files_in(&folder_for(st, kind)) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(Backup { name, kind, size: meta.len(), time: modified(&meta) });
        }
    }
    out
}

/// The backup with Prowlarr's id `id`.
fn by_id(st: &AppState, id: i64) -> Option<Backup> {
    all(st).into_iter().find(|b| b.id() == id)
}

/// Deletes the backups of `kind` older than the `BackupRetention` setting (days), as `CleanupOldBackups` does.
pub fn clean_up_old(st: &AppState, kind: Kind, now: f64) {
    let retention = config::backup_retention(&st.db) as f64;
    tracing::debug!("Cleaning up backup files older than {retention} days");
    for (path, meta) in files_in(&folder_for(st, kind)) {
        if modified(&meta) + retention * 86_400.0 < now {
            tracing::debug!("Deleting old backup file: {}", path.display());
            let _ = std::fs::remove_file(&path);
        }
    }
    tracing::debug!("Finished cleaning up old backup files");
}

/// The temporary folder a backup is put together in.
fn temp_folder(st: &AppState, name: &str) -> PathBuf {
    st.data_dir.join("tmp").join(name)
}

/// Makes a backup of `kind` (`BackupService.Backup`): the database and `INFO`, zipped into
/// `lantern_backup_v<version>_<yyyy.MM.dd_HH.mm.ss>.zip`; a scheduled one first deletes the scheduled backups past
/// the retention. The new file's path.
pub fn make(st: &AppState, kind: Kind) -> kit_server_core::Result<PathBuf> {
    let folder = folder_for(st, kind);
    inside_data(st, &folder)?;
    let temp = temp_folder(st, "lantern_backup");
    let io = |what: &str, e: std::io::Error| err(500, format!("{what}: {e}"));
    std::fs::create_dir_all(&temp).map_err(|e| io("The backup's temporary folder can't be made", e))?;
    std::fs::create_dir_all(&folder).map_err(|e| io(&format!("Backup folder {} is not writable", folder.display()), e))?;
    let now = chrono::Local::now();
    let version = kit_server_core::version::base(crate::VERSION);
    let file_name = format!("lantern_backup_v{version}_{}.zip", now.format("%Y.%m.%d_%H.%M.%S"));
    let path = folder.join(&file_name);
    empty(&temp);
    if kind != Kind::Manual {
        clean_up_old(st, kind, crate::auth::now());
    }
    // BackupDatabase: a consistent copy of the database while it is in use
    let db_copy = temp.join("app.db");
    st.db.execute("VACUUM INTO ?", p![db_copy.display().to_string()])?;
    // CreateVersionInfo
    let info = format!("v{version}\n{}\n", now.format("%Y-%m-%d %H:%M:%S"));
    std::fs::write(temp.join("INFO"), info).map_err(|e| io("The backup's INFO can't be written", e))?;
    let zipped = zip_files(&path, &[db_copy.clone(), temp.join("INFO")]);
    empty(&temp);
    zipped.map_err(|e| {
        let _ = std::fs::remove_file(&path);
        err(500, format!("The backup zip {} can't be written: {e}", path.display()))
    })?;
    tracing::info!("Backup made: {}", path.display());
    Ok(path)
}

/// Writes the files to a new zip at `path`, each at the zip's top (`ArchiveService.CreateZip`).
fn zip_files(path: &std::path::Path, files: &[PathBuf]) -> std::io::Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    for f in files {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        zip.start_file(name, options).map_err(std::io::Error::other)?;
        std::io::copy(&mut std::fs::File::open(f)?, &mut zip)?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
}

/// Deletes what is in `folder` (`EmptyFolder`).
fn empty(folder: &std::path::Path) {
    if let Ok(dir) = std::fs::read_dir(folder) {
        for e in dir.flatten() {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
}

/// Whether the file starts like an SQLite database.
fn is_sqlite(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == SQLITE_HEADER
}

/// Puts the database of the backup at `path` (a `.zip` Lantern made, or a database file) aside to take the place
/// of Lantern's database at the next start (`BackupService.Restore`).
pub fn restore(st: &AppState, path: &std::path::Path) -> ApiResult<()> {
    let target = st.data_dir.join(RESTORE_FILE);
    let not_lantern = || fail(400, "This file isn't a Lantern database. Choose a backup .zip Lantern made, or its app.db.");
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        let file = std::fs::File::open(path).map_err(|e| fail(500, format!("The backup {} can't be read: {e}", path.display())))?;
        let mut zip =
            zip::ZipArchive::new(file).map_err(|_| fail(400, "This file isn't a zip file. Choose a backup .zip Lantern made."))?;
        let names: Vec<String> = zip.file_names().map(str::to_string).collect();
        let top = |n: &str| names.iter().find(|x| x.rsplit('/').next().is_some_and(|f| f.eq_ignore_ascii_case(n))).cloned();
        let Some(db_name) = top("app.db") else {
            if top("prowlarr.db").is_some() || top("nzbdrone.db").is_some() {
                return Err(fail(
                    400,
                    "This is a Prowlarr backup. Lantern brings Prowlarr's indexers, apps and settings in with its Migrate page instead; restoring it here would replace Lantern's own data.",
                ));
            }
            // RestoreBackupFailedException(HttpStatusCode.NotFound, …)
            return Err(fail(404, "Unable to restore database file from backup"));
        };
        let temp = temp_folder(st, "lantern_backup_restore");
        std::fs::create_dir_all(&temp).map_err(|e| fail(500, format!("The restore's temporary folder can't be made: {e}")))?;
        let unpacked = temp.join("app.db");
        let result = (|| -> std::io::Result<()> {
            let mut entry = zip.by_name(&db_name).map_err(std::io::Error::other)?;
            let mut out = std::fs::File::create(&unpacked)?;
            std::io::copy(&mut entry, &mut out)?;
            out.flush()
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(fail(500, format!("The database can't be taken out of the backup: {e}")));
        }
        if !is_sqlite(&unpacked) {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(not_lantern());
        }
        let moved = std::fs::rename(&unpacked, &target);
        let _ = std::fs::remove_dir_all(&temp);
        moved.map_err(|e| fail(500, format!("The restored database can't be put aside: {e}")))?;
    } else {
        if !is_sqlite(path) {
            return Err(not_lantern());
        }
        std::fs::copy(path, &target).map_err(|e| fail(500, format!("The restored database can't be put aside: {e}")))?;
    }
    tracing::info!("A backup will be restored at the next start");
    Ok(())
}

/// At start, before the database opens: when a restore is waiting, the database it replaces is kept as
/// `app.db.before-restore` and the restored one takes its place (`DatabaseRestorationService.Restore`). Nothing
/// happens when no restore was asked for.
pub fn apply_waiting_restore(data: &std::path::Path) -> anyhow::Result<()> {
    let waiting = data.join(RESTORE_FILE);
    if !waiting.is_file() {
        return Ok(());
    }
    tracing::info!("Restoring Database");
    let db = data.join("app.db");
    let failed =
        |e: std::io::Error| anyhow::anyhow!("The restored database {} can't take the place of {}: {e}", waiting.display(), db.display());
    for side in ["app.db-shm", "app.db-wal", "app.db-journal"] {
        let _ = std::fs::remove_file(data.join(side));
    }
    if db.exists() {
        std::fs::rename(&db, data.join(REPLACED_FILE)).map_err(failed)?;
    }
    std::fs::rename(&waiting, &db).map_err(failed)?;
    Ok(())
}

/// `GET /api/v1/system/backup`: the backups, newest first.
async fn list_route(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    let mut list = all(&st);
    list.sort_by(|a, b| b.time.total_cmp(&a.time));
    Ok(Json(Value::Array(list.iter().map(Backup::json).collect())))
}

/// `DELETE /api/v1/system/backup/{id}`: deletes the backup; 404 when there is no such backup or file.
async fn delete_route(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let b = by_id(&st, id).ok_or_else(|| api_exception(404, None))?;
    let path = folder_for(&st, b.kind).join(&b.name);
    if !path.is_file() {
        return Err(api_exception(404, None));
    }
    std::fs::remove_file(&path).map_err(|e| fail(500, format!("The backup {} can't be deleted: {e}", path.display())))?;
    Ok(Json(json!({})))
}

/// What a restore answers: the database is restored when Lantern starts again.
fn restart_required() -> Json<Value> {
    Json(json!({ "restartRequired": true }))
}

/// `POST /api/v1/system/backup/restore/{id}`: restores the backup at the next start.
async fn restore_route(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let b = by_id(&st, id).ok_or_else(|| api_exception(404, None))?;
    let path = folder_for(&st, b.kind).join(&b.name);
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || restore(&st2, &path)).await.map_err(|e| fail(500, e.to_string()))??;
    Ok(restart_required())
}

/// The extensions an uploaded backup may have.
const VALID_EXTENSIONS: [&str; 3] = [".zip", ".db", ".xml"];

/// `POST /api/v1/system/backup/restore/upload`: a form with the backup file (the first file in it); restores it at
/// the next start.
async fn upload_route(State(st): State<AppState>, _: V1, mut form: Multipart) -> ApiResult<Json<Value>> {
    let unreadable = |_| fail(400, "The form couldn't be read. Choose the backup file again and retry.");
    loop {
        let Some(field) = form.next_field().await.map_err(unreadable)? else {
            return Err(api_exception(400, Some("file must be provided")));
        };
        if field.file_name().is_some() {
            return restore_upload(&st, field).await;
        }
    }
}

/// Saves the uploaded backup in the data folder's `tmp`, restores it and deletes it again.
async fn restore_upload(st: &AppState, mut field: axum::extract::multipart::Field<'_>) -> ApiResult<Json<Value>> {
    let unreadable = |_| fail(400, "The form couldn't be read. Choose the backup file again and retry.");
    let file_name = field.file_name().unwrap_or("").to_string();
    let extension = std::path::Path::new(&file_name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    if !VALID_EXTENSIONS.contains(&extension.as_str()) {
        return Err(api_exception(415, Some(&format!("Invalid extension, must be one of: {}", VALID_EXTENSIONS.join(", ")))));
    }
    if extension == ".xml" {
        return Err(fail(
            400,
            "Lantern keeps its settings in its database and has no config.xml. Restore a backup .zip or a .db file instead.",
        ));
    }
    let temp = st.data_dir.join("tmp");
    std::fs::create_dir_all(&temp).map_err(|e| fail(500, format!("The upload's temporary folder can't be made: {e}")))?;
    let path = temp.join(format!("lantern_backup_restore{extension}"));
    let mut out = std::fs::File::create(&path).map_err(|e| fail(500, format!("The upload can't be saved: {e}")))?;
    while let Some(chunk) = field.chunk().await.map_err(unreadable)? {
        if let Err(e) = out.write_all(&chunk) {
            let _ = std::fs::remove_file(&path);
            return Err(fail(500, format!("The upload can't be saved: {e}")));
        }
    }
    drop(out);
    let st2 = st.clone();
    let p2 = path.clone();
    let result = tokio::task::spawn_blocking(move || restore(&st2, &p2)).await.map_err(|e| fail(500, e.to_string()));
    // Cleanup restored file
    let _ = std::fs::remove_file(&path);
    result??;
    Ok(restart_required())
}

/// `GET /backup/{type}/{name}`: the backup file, as a download.
async fn download_route(State(st): State<AppState>, _: V1, Path((kind, name)): Path<(String, String)>) -> Result<Response, ApiError> {
    let kind = Kind::from_folder(&kind).ok_or_else(|| api_exception(404, None))?;
    if name.contains('/') || name.contains('\\') || !is_backup_file(&name) {
        return Err(api_exception(404, None));
    }
    let path = folder_for(&st, kind).join(&name);
    let bytes = tokio::fs::read(&path).await.map_err(|_| api_exception(404, None))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/zip".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_follow_prowlarrs_pattern() {
        assert!(is_backup_file("/data/Backups/manual/prowlarr_backup_v1.0.0.zip"));
        assert!(is_backup_file("lantern_backup_v0.0.1_2026.11.06_10.30.00.zip"));
        assert!(is_backup_file("nzbdrone_backup_2015.01.01.zip"));
        assert!(!is_backup_file("notes.txt") && !is_backup_file("backup.zip"));
    }

    #[test]
    fn kinds_have_prowlarrs_names_and_folders() {
        assert_eq!(Kind::ALL.map(Kind::name), ["Scheduled", "Manual", "Update"]);
        assert_eq!(Kind::from_folder("MANUAL"), Some(Kind::Manual));
        assert_eq!(Kind::from_folder("other"), None);
    }
}
