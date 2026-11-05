//! The app's notices and Diagnostics checks.
//!
//! - Notice `data`: the data folder can't be written (the kit's storage notice, for the one folder this app
//!   writes to), checked every minute and on Check again. Served at `/api/notices/data` (and in `/api/notices`).
//! - Diagnostics: the data folder and the database. An app adds its own services, folders and tools here.

use crate::AppState;
use kit_server_core::Db;
use kit_server_diagnostics::{Check, Kind, Registry};
use kit_server_notices::{Notice, NoticeCheck, Notices, Severity};
use std::path::Path;
use std::time::Duration;

/// The id of the data folder notice.
pub const DATA_NOTICE: &str = "data";

/// Whether a file can be made in `dir`: `Ok`, or the reason in words.
pub fn writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("missing".into()),
        Err(e) if e.raw_os_error() == Some(30) => Err("read-only".into()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err("not writable by the server's user".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// The data folder notice's sentence for a folder that can't be written, for example "The data folder
/// /data is read-only – check the disk (dmesg, fsck) or the mount".
pub fn data_notice_text(dir: &Path, why: &str) -> String {
    format!("The data folder {} is {why} – check the disk (dmesg, fsck) or the mount", dir.display())
}

/// The app's notices: the data folder, probed every minute.
pub fn notices() -> Notices<AppState> {
    Notices::new().register(
        NoticeCheck::kept(DATA_NOTICE, Severity::Warn, |st: AppState| async move {
            let dir = st.data_dir.clone();
            match tokio::task::spawn_blocking(move || writable(&dir).map_err(|why| (dir, why))).await {
                Ok(Ok(())) => Notice::fine(),
                Ok(Err((dir, why))) => Notice::failing(data_notice_text(&dir, &why)),
                Err(e) => Notice::failing(e.to_string()),
            }
        })
        .every(Duration::from_secs(60)),
    )
}

/// The Diagnostics page's checks: the data folder and the database.
pub fn diagnostics() -> Registry<AppState> {
    Registry::new()
        .check("data", |st: AppState| async move {
            let dir = st.data_dir.clone();
            let shown = dir.display().to_string();
            match tokio::task::spawn_blocking(move || writable(&dir)).await.unwrap_or_else(|e| Err(e.to_string())) {
                Ok(()) => Check::new("Data folder", Kind::Path, true, format!("{shown} · writable")).request(shown),
                Err(why) => Check::new("Data folder", Kind::Path, false, format!("{shown} · {why}"))
                    .request(shown)
                    .fix("Check the disk (dmesg, fsck) or the mount, and that the server's user may write to it"),
            }
        })
        .check("database", |st: AppState| async move {
            match st.db.one("SELECT COUNT(*) n FROM users", vec![]) {
                Ok(_) => Check::new("Database", Kind::Path, true, "OK"),
                Err(e) => Check::new("Database", Kind::Path, false, e.detail),
            }
        })
}
