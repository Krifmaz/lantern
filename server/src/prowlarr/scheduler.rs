// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Jobs/Scheduler.cs, TaskManager.cs, ScheduledTask.cs,
// src/NzbDrone.Core/HealthCheck/HealthCheckService.cs, src/NzbDrone.Core/Housekeeping/HousekeepingService.cs,
// Housekeepers/CleanupCommandQueue.cs, CleanupOrphanedApplicationStatus.cs, CleanupOrphanedDownloadClientStatus.cs,
// CleanupOrphanedNotificationStatus.cs, CleanupOrphanedHistoryItems.cs, FixFutureRunScheduledTasks.cs,
// TrimLogDatabase.cs, src/NzbDrone.Core/Instrumentation/LogRepository.cs,
// src/NzbDrone.Core/Update/History/UpdateHistoryService.cs, src/NzbDrone.Core/Notifications/NotificationService.cs
// (Handle(UpdateInstalledEvent))), GPL-3.0.

//! The scheduler: every 30 seconds it queues the command of each scheduled task whose interval is up
//! (`/api/v1/system/task` lists them), as Prowlarr's `Scheduler` does. At start it also runs the health checks once
//! and, when Lantern's version changed since the last start, tells the notifications on for application updates.
//!
//! The housekeeping task is here too: it tidies the tables Prowlarr's housekeepers tidy that Lantern has.

use super::apps::status::{APPLICATION_STATUS, DOWNLOAD_CLIENT_STATUS, NOTIFICATION_STATUS};
use super::commands;
use crate::AppState;
use kit_server_core::{p, Db, RowExt, SqliteDb};

/// How often the scheduler looks, in seconds (Prowlarr's timer: `1000 * 30` ms).
pub const EVERY: u64 = 30;

/// A scheduled task whose interval is up.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The command's name (`CheckHealth`).
    pub command: String,
    /// When it last ran, seconds since 1970.
    pub last_execution: f64,
    /// When its last run started (.NET's smallest date when it never ran).
    pub last_start: f64,
}

/// The update history table (Prowlarr's `UpdateHistory`): which versions ran, and when they first did.
pub fn migrate(db: &SqliteDb) -> kit_server_core::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS prowlarr_update_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            date REAL NOT NULL,
            version TEXT NOT NULL,
            event_type INTEGER NOT NULL
        );",
    )
}

/// The tasks due at `now` (`TaskManager.GetPending`): an interval above 0, and the last run longer ago than it.
pub fn pending(db: &SqliteDb, now: f64) -> kit_server_core::Result<Vec<Pending>> {
    Ok(db
        .query("SELECT * FROM prowlarr_tasks ORDER BY id", p![])?
        .iter()
        .filter(|r| r.i("interval") > 0 && r.f("last_execution") + r.i("interval") as f64 * 60.0 < now)
        .map(|r| Pending {
            command: r.s("type_name").rsplit('.').next().unwrap_or("").trim_end_matches("Command").to_string(),
            last_execution: r.f("last_execution"),
            last_start: r.get("last_start_time").and_then(serde_json::Value::as_f64).unwrap_or(super::system::MIN_DATE),
        })
        .collect())
}

/// One look of the scheduler (`Scheduler.ExecuteCommands`): queues each due task's command with the scheduled
/// trigger and the task's low priority. A command already queued or running is not queued again. The ids of the
/// queued commands.
pub fn tick(st: &AppState) -> Vec<i64> {
    let due = match pending(&st.db, crate::auth::now()) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("The scheduled tasks can't be read: {}", e.detail);
            return Vec::new();
        }
    };
    tracing::trace!("Pending Tasks: {}", due.len());
    let mut ids = Vec::new();
    for task in due {
        let Some(kind) = commands::kind_of(&task.command) else { continue };
        let body = commands::scheduled_body(kind, task.last_execution, task.last_start);
        match commands::push(st, kind, body, -1, 2) {
            Ok(id) => ids.push(id),
            Err(e) => tracing::warn!("The {} task can't be queued: {}", task.command, e.detail),
        }
    }
    ids
}

/// Starts the scheduler (`Scheduler.Handle(ApplicationStartedEvent)`), after the start's own work: the health
/// checks run once (`HealthCheckService.HandleAsync(ApplicationStartedEvent)`) and an update is told.
pub fn spawn(st: AppState) {
    tokio::spawn(async move {
        version_started(&st).await;
        if let Ok(findings) = super::health::results(&st).await {
            tracing::debug!("{} health checks found something at start", findings.len());
        }
        super::signalr::broadcast(&st, "health", "sync", None);
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(EVERY));
        timer.tick().await;
        loop {
            timer.tick().await;
            tick(&st);
        }
    });
}

/// `UpdateHistoryService.Handle(ApplicationStartedEvent)` and `NotificationService.Handle(UpdateInstalledEvent)`:
/// a version that runs for the first time is noted, and when another version ran before it, the notifications on
/// for application updates are told. The version before, when there was one and it changed.
pub async fn version_started(st: &AppState) -> Option<String> {
    let version = kit_server_core::version::base(crate::VERSION);
    let last = st
        .db
        .one("SELECT version FROM prowlarr_update_history ORDER BY date DESC, id DESC LIMIT 1", p![])
        .ok()
        .flatten()
        .map(|r| r.s("version"));
    if last.as_deref() == Some(version.as_str()) {
        return None;
    }
    // EventType Installed
    let _ = st.db.execute(
        "INSERT INTO prowlarr_update_history (date, version, event_type) VALUES (?, ?, 1)",
        p![crate::auth::now(), version.clone()],
    );
    let previous = last?;
    let msg = super::apps::notifications::UpdateMessage {
        message: format!("{} updated from {previous} to {version}", super::APP_NAME),
        previous_version: previous.clone(),
        new_version: version,
    };
    super::apps::notifications::updated(st, &msg).await;
    Some(previous)
}

/// One of Prowlarr's housekeepers (`IHousekeepingTask.Clean`).
type Housekeeper<'a> = Box<dyn Fn() -> kit_server_core::Result<()> + 'a>;

/// `HousekeepingCommand` (`HousekeepingService.Clean`): each housekeeper that applies to Lantern's tables, then the
/// database is compressed. A housekeeper that fails is logged and the others still run, as in Prowlarr.
pub fn housekeeping(st: &AppState) -> kit_server_core::Result<()> {
    tracing::info!("Running housecleaning tasks");
    let now = crate::auth::now();
    let db = &st.db;
    let orphaned = |status: &str, providers: &str| {
        format!("DELETE FROM {status} WHERE id IN (SELECT s.id FROM {status} s LEFT OUTER JOIN {providers} p ON s.provider_id = p.id WHERE p.id IS NULL)")
    };
    let tasks: Vec<(&str, Housekeeper)> = vec![
        // CleanupCommandQueue: commands that ended more than a day ago
        (
            "CleanupCommandQueue",
            Box::new(move || db.execute("DELETE FROM prowlarr_commands WHERE ended_at < ?", p![now - 86_400.0]).map(|_| ())),
        ),
        (
            "CleanupOrphanedApplicationStatus",
            Box::new(move || db.execute(&orphaned(APPLICATION_STATUS.table, "prowlarr_applications"), p![]).map(|_| ())),
        ),
        (
            "CleanupOrphanedDownloadClientStatus",
            Box::new(move || db.execute(&orphaned(DOWNLOAD_CLIENT_STATUS.table, "prowlarr_download_clients"), p![]).map(|_| ())),
        ),
        (
            "CleanupOrphanedNotificationStatus",
            Box::new(move || db.execute(&orphaned(NOTIFICATION_STATUS.table, "prowlarr_notifications"), p![]).map(|_| ())),
        ),
        // CleanupOrphanedHistoryItems: the history of indexers that are gone
        (
            "CleanupOrphanedHistoryItems",
            Box::new(move || {
                db.execute(
                    "DELETE FROM indexer_history WHERE id IN (SELECT h.id FROM indexer_history h LEFT OUTER JOIN indexers i ON h.indexer_id = i.id WHERE i.id IS NULL)",
                    p![],
                )
                .map(|_| ())
            }),
        ),
        // FixFutureRunScheduledTasks: a task that says it ran in the future ran now
        (
            "FixFutureRunScheduledTasks",
            Box::new(move || db.execute("UPDATE prowlarr_tasks SET last_execution=? WHERE last_execution > ?", p![now, now]).map(|_| ())),
        ),
        // TrimLogDatabase (LogRepository.Trim): log entries from before the start of the day a week ago
        ("TrimLogDatabase", Box::new(move || trim_logs(db, now))),
    ];
    for (name, task) in tasks {
        tracing::debug!("Starting {name}");
        match task() {
            Ok(()) => tracing::debug!("Completed {name}"),
            Err(e) => tracing::error!("Error running housekeeping task: {name}: {}", e.detail),
        }
    }
    // Vacuuming the log db isn't needed since that's done in a separate housekeeping task
    tracing::debug!("Compressing main database after housekeeping");
    db.execute("VACUUM", p![])?;
    Ok(())
}

/// `LogRepository.Trim`: deletes the log entries at or before midnight (UTC) seven days ago.
pub fn trim_logs(db: &SqliteDb, now: f64) -> kit_server_core::Result<()> {
    let week_ago = now - 7.0 * 86_400.0;
    let trim_date = (week_ago / 86_400.0).floor() * 86_400.0;
    db.execute("DELETE FROM prowlarr_logs WHERE time <= ?", p![trim_date])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/JobTests/JobRepositoryFixture.cs: the pending tasks
    // and the start's task list), GPL-3.0.
    use super::*;

    fn db() -> SqliteDb {
        let dir = std::env::temp_dir().join(format!("lantern-scheduler-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = SqliteDb::open(&dir.join("app.db")).unwrap();
        super::super::migrate(&db).unwrap();
        db
    }

    #[test]
    fn pending_job_should_get_jobs_that_have_matured() {
        let db = db();
        let now = crate::auth::now();
        db.execute("UPDATE prowlarr_tasks SET interval=1, last_execution=? WHERE id=1", p![now - 5.0 * 60.0]).unwrap();
        let due = pending(&db, now).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].command, "MessagingCleanup");
    }

    #[test]
    fn pending_job_should_not_get_jobs_that_havent_matured() {
        let db = db();
        let now = crate::auth::now();
        db.execute("UPDATE prowlarr_tasks SET interval=60, last_execution=?", p![now - 5.0 * 60.0]).unwrap();
        assert!(pending(&db, now).unwrap().is_empty());
        // an interval of 0 never runs
        db.execute("UPDATE prowlarr_tasks SET interval=0, last_execution=0", p![]).unwrap();
        assert!(pending(&db, now).unwrap().is_empty());
    }

    #[test]
    fn init_should_remove_tasks_that_no_longer_exist_and_keep_the_last_run() {
        let db = db();
        db.execute("INSERT INTO prowlarr_tasks (type_name, interval, last_execution) VALUES ('NzbDrone.Core.Old.GoneCommand', 5, 0)", p![])
            .unwrap();
        db.execute("UPDATE prowlarr_tasks SET interval=0, last_execution=1000 WHERE type_name LIKE '%.HousekeepingCommand'", p![]).unwrap();
        super::super::system::seed_tasks(&db).unwrap();
        let rows = db.query("SELECT * FROM prowlarr_tasks", p![]).unwrap();
        assert_eq!(rows.len(), 9);
        let house = rows.iter().find(|r| r.s("type_name").ends_with(".HousekeepingCommand")).unwrap();
        assert_eq!((house.i("interval"), house.f("last_execution")), (24 * 60, 1000.0));
    }

    #[test]
    fn the_backup_task_follows_the_backup_interval_in_days() {
        // TaskManager.GetBackupInterval: BackupInterval days, held between 1 and 7
        let db = db();
        let interval = |db: &SqliteDb| {
            db.one("SELECT interval FROM prowlarr_tasks WHERE type_name LIKE '%.BackupCommand'", p![]).unwrap().unwrap().i("interval")
        };
        assert_eq!(interval(&db), 7 * 24 * 60);
        for (days, minutes) in [("2", 2 * 24 * 60), ("0", 24 * 60), ("30", 7 * 24 * 60)] {
            super::super::config_set(&db, "backupinterval", days).unwrap();
            super::super::system::seed_tasks(&db).unwrap();
            assert_eq!(interval(&db), minutes, "{days} days");
        }
    }

    #[test]
    fn logs_are_trimmed_from_midnight_a_week_ago() {
        let db = db();
        // 2026-11-06 12:00 UTC; the trim date is 2026-10-30 00:00 UTC
        let now = 1_793_966_400.0;
        let midnight = 1_793_318_400.0;
        for t in [midnight - 1.0, midnight, midnight + 1.0] {
            db.execute("INSERT INTO prowlarr_logs (message, time, logger, level) VALUES ('x', ?, 'L', 'Info')", p![t]).unwrap();
        }
        trim_logs(&db, now).unwrap();
        let left: Vec<f64> = db.query("SELECT time FROM prowlarr_logs", p![]).unwrap().iter().map(|r| r.f("time")).collect();
        assert_eq!(left, vec![midnight + 1.0]);
    }
}
