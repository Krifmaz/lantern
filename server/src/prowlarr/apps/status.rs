// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/ThingiProvider/Status/ProviderStatusServiceBase.cs,
// ProviderStatusBase.cs, EscalationBackOff.cs, src/NzbDrone.Core/Applications/ApplicationStatusService.cs,
// src/NzbDrone.Core/HealthCheck/Checks/ApplicationStatusCheck.cs, ApplicationLongTermStatusCheck.cs), GPL-3.0.

//! How long a provider (an application, later a download client or a notification) is left alone after it
//! failed: Prowlarr's escalating back-off. Each failure in a row waits longer (1 minute, 5, 15, 30, 1 hour, …), a
//! success steps back one level. The first minutes after a start and after the first failure are gentler, as in
//! Prowlarr.

use kit_server_core::{p, Db, RowExt, SqliteDb};
use std::sync::OnceLock;

/// The waits, in seconds, by escalation level (`EscalationBackOff.Periods`).
pub const PERIODS: [f64; 10] = [0.0, 60.0, 300.0, 900.0, 1800.0, 3600.0, 3.0 * 3600.0, 6.0 * 3600.0, 12.0 * 3600.0, 24.0 * 3600.0];

/// When this server started (seconds since 1970), for the start-up grace period.
static STARTED: OnceLock<f64> = OnceLock::new();

/// Notes when the server started (the first call wins).
pub fn mark_started(now: f64) {
    let _ = STARTED.set(now);
}

/// When the server started.
pub fn started() -> f64 {
    *STARTED.get_or_init(crate::auth::now)
}

/// One provider's failures (`ProviderStatusBase`): times in seconds since 1970.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// The provider it is about.
    pub provider_id: i64,
    /// When the failures in a row began.
    pub initial_failure: Option<f64>,
    /// The last failure.
    pub most_recent_failure: Option<f64>,
    /// How far the back-off has gone.
    pub escalation_level: i64,
    /// Not used until then.
    pub disabled_till: Option<f64>,
}

impl Status {
    /// Whether it is held back now (`IsDisabled`).
    pub fn is_disabled(&self, now: f64) -> bool {
        self.disabled_till.is_some_and(|t| t > now)
    }
}

/// The rules of one kind of provider (`ProviderStatusServiceBase` with its settings).
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    /// The highest escalation level.
    pub maximum_escalation_level: i64,
    /// How long after the first failure in a row the wait does not grow, in seconds.
    pub minimum_time_since_initial_failure: f64,
    /// How long after the start failures wait at most 5 minutes, in seconds.
    pub minimum_time_since_startup: f64,
}

/// The rules for applications (`ApplicationStatusService`): five levels, five minutes' grace.
pub const APPLICATIONS: Rules =
    Rules { maximum_escalation_level: 5, minimum_time_since_initial_failure: 300.0, minimum_time_since_startup: 900.0 };

impl Rules {
    /// The wait at the status' level (`CalculateBackOffPeriod`).
    fn back_off(&self, s: &Status) -> f64 {
        PERIODS[s.escalation_level.min(self.maximum_escalation_level).max(0) as usize]
    }

    /// The status after a success (`RecordSuccess`): one level down and no longer held back; `None` when it was
    /// fine already (nothing is saved then).
    pub fn success(&self, mut s: Status) -> Option<Status> {
        if s.escalation_level == 0 {
            return None;
        }
        s.escalation_level -= 1;
        s.disabled_till = None;
        Some(s)
    }

    /// The status after a failure at `now` (`RecordFailure`); `escalate` is false for a connection failure, and
    /// `minimum_back_off` (seconds, 0 for none) is the least wait an app asked for (`Retry-After`).
    pub fn failure(&self, mut s: Status, now: f64, started: f64, minimum_back_off: f64, mut escalate: bool) -> Status {
        s.most_recent_failure = Some(now);
        if s.escalation_level == 0 {
            s.initial_failure = Some(now);
            s.escalation_level = 1;
            escalate = false;
        }
        let in_startup_grace = started + self.minimum_time_since_startup > now;
        let in_grace = s.initial_failure.unwrap_or(now) + self.minimum_time_since_initial_failure > now;
        if escalate && !in_grace && !in_startup_grace {
            s.escalation_level = self.maximum_escalation_level.min(s.escalation_level + 1);
        }
        if minimum_back_off != 0.0 {
            while s.escalation_level < self.maximum_escalation_level && self.back_off(&s) < minimum_back_off {
                s.escalation_level += 1;
            }
        }
        if !in_grace || minimum_back_off != 0.0 {
            s.disabled_till = Some(now + self.back_off(&s));
        }
        if in_startup_grace && minimum_back_off == 0.0 {
            if let Some(till) = s.disabled_till {
                let maximum = now + PERIODS[2];
                if maximum < till {
                    s.disabled_till = Some(maximum);
                }
            }
        }
        s
    }
}

/// The statuses of one kind of provider, kept in `table` (Prowlarr's `ApplicationStatus` and its siblings).
#[derive(Debug, Clone, Copy)]
pub struct Store {
    /// The table.
    pub table: &'static str,
    /// The rules.
    pub rules: Rules,
}

/// The application statuses.
pub const APPLICATION_STATUS: Store = Store { table: "prowlarr_application_status", rules: APPLICATIONS };

impl Store {
    /// The table's definition.
    pub fn schema(&self) -> String {
        format!(
            "CREATE TABLE IF NOT EXISTS {} (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_id INTEGER NOT NULL UNIQUE,
                initial_failure REAL,
                most_recent_failure REAL,
                escalation_level INTEGER NOT NULL DEFAULT 0,
                disabled_till REAL
            );",
            self.table
        )
    }

    /// The status of a row.
    fn of_row(r: &kit_server_core::Row) -> Status {
        let opt = |k: &str| r.get(k).and_then(serde_json::Value::as_f64);
        Status {
            provider_id: r.i("provider_id"),
            initial_failure: opt("initial_failure"),
            most_recent_failure: opt("most_recent_failure"),
            escalation_level: r.i("escalation_level"),
            disabled_till: opt("disabled_till"),
        }
    }

    /// Every saved status.
    pub fn all(&self, db: &SqliteDb) -> kit_server_core::Result<Vec<Status>> {
        Ok(db.query(&format!("SELECT * FROM {}", self.table), p![])?.iter().map(Self::of_row).collect())
    }

    /// The providers held back now (`GetBlockedProviders`).
    pub fn blocked(&self, db: &SqliteDb) -> kit_server_core::Result<Vec<Status>> {
        let now = crate::auth::now();
        Ok(self.all(db)?.into_iter().filter(|s| s.is_disabled(now)).collect())
    }

    /// The status of one provider (a fresh one when none is saved).
    pub fn get(&self, db: &SqliteDb, provider_id: i64) -> kit_server_core::Result<Status> {
        let r = db.one(&format!("SELECT * FROM {} WHERE provider_id=?", self.table), p![provider_id])?;
        Ok(r.map(|r| Self::of_row(&r)).unwrap_or(Status { provider_id, ..Default::default() }))
    }

    /// Saves a status (`Upsert`).
    fn save(&self, db: &SqliteDb, s: &Status) -> kit_server_core::Result<()> {
        db.execute(
            &format!(
                "INSERT INTO {} (provider_id, initial_failure, most_recent_failure, escalation_level, disabled_till) VALUES (?,?,?,?,?)
                 ON CONFLICT(provider_id) DO UPDATE SET initial_failure=excluded.initial_failure,
                 most_recent_failure=excluded.most_recent_failure, escalation_level=excluded.escalation_level,
                 disabled_till=excluded.disabled_till",
                self.table
            ),
            p![s.provider_id, s.initial_failure, s.most_recent_failure, s.escalation_level, s.disabled_till],
        )?;
        Ok(())
    }

    /// A success of provider `id` (`RecordSuccess`).
    pub fn record_success(&self, db: &SqliteDb, id: i64) {
        if id <= 0 {
            return;
        }
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(s) = self.get(db, id) {
            if let Some(s) = self.rules.success(s) {
                let _ = self.save(db, &s);
            }
        }
    }

    /// A failure of provider `id` (`RecordFailure`, or `RecordConnectionFailure` when `escalate` is false), with
    /// the least wait in seconds (0 for none).
    pub fn record_failure(&self, db: &SqliteDb, id: i64, minimum_back_off: f64, escalate: bool) {
        if id <= 0 {
            return;
        }
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(s) = self.get(db, id) {
            let s = self.rules.failure(s, crate::auth::now(), started(), minimum_back_off, escalate);
            let _ = self.save(db, &s);
        }
    }

    /// Forgets a deleted provider's status.
    pub fn delete(&self, db: &SqliteDb, id: i64) -> kit_server_core::Result<()> {
        db.execute(&format!("DELETE FROM {} WHERE provider_id=?", self.table), p![id])?;
        Ok(())
    }
}

/// Keeps two failures of the same provider from overwriting each other (`_syncRoot`).
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What a status health check found: `None` when nothing, else whether all are held back and their names.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    /// Every enabled provider is held back (an error).
    All,
    /// These are held back (a warning).
    Some(Vec<String>),
}

/// `ApplicationStatusCheck` (`long_term` false: failing for less than six hours) and
/// `ApplicationLongTermStatusCheck` (`long_term` true: for more): the enabled providers (id, name) that are held
/// back.
pub fn check(enabled: &[(i64, String)], blocked: &[Status], now: f64, long_term: bool) -> Option<Check> {
    let six_hours_ago = now - 6.0 * 3600.0;
    let held: Vec<&String> = enabled
        .iter()
        .filter(|(id, _)| {
            blocked.iter().any(|s| {
                s.provider_id == *id && s.initial_failure.is_some_and(|f| if long_term { f < six_hours_ago } else { f > six_hours_ago })
            })
        })
        .map(|(_, n)| n)
        .collect();
    if held.is_empty() {
        return None;
    }
    if held.len() == enabled.len() {
        return Some(Check::All);
    }
    Some(Check::Some(held.into_iter().cloned().collect()))
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/ThingiProviderTests/ProviderStatusServiceFixture.cs,
    // src/NzbDrone.Core.Test/HealthCheck/Checks/ApplicationStatusCheckFixture.cs), GPL-3.0.
    use super::*;

    /// The fixture's service: Prowlarr's default rules (every level, no grace after the first failure).
    const MOCK: Rules = Rules { maximum_escalation_level: 9, minimum_time_since_initial_failure: 0.0, minimum_time_since_startup: 900.0 };
    const EPOCH: f64 = 1_800_000_000.0;
    /// Started an hour before the epoch.
    const STARTED_LONG_AGO: f64 = EPOCH - 3600.0;

    fn close(a: Option<f64>, b: f64) {
        assert!((a.unwrap() - b).abs() < 0.5, "{a:?} vs {b}");
    }

    #[test]
    fn should_start_backoff_on_first_failure() {
        let s = MOCK.failure(Status { provider_id: 1, ..Default::default() }, EPOCH, STARTED_LONG_AGO, 0.0, true);
        assert!(s.is_disabled(EPOCH));
        close(s.disabled_till, EPOCH + 60.0);
    }

    #[test]
    fn should_cancel_backoff_on_success() {
        let s = MOCK.success(Status { provider_id: 1, escalation_level: 2, ..Default::default() }).unwrap();
        assert!(!s.is_disabled(EPOCH));
    }

    #[test]
    fn should_not_store_update_if_already_okay() {
        assert!(MOCK.success(Status { provider_id: 1, escalation_level: 0, ..Default::default() }).is_none());
    }

    #[test]
    fn should_preserve_escalation_on_intermittent_success() {
        let s = Status {
            provider_id: 1,
            initial_failure: Some(EPOCH - 20.0),
            most_recent_failure: Some(EPOCH - 4.0),
            escalation_level: 3,
            disabled_till: None,
        };
        let s = MOCK.success(s).unwrap();
        let s = MOCK.success(s).unwrap();
        let s = MOCK.failure(s, EPOCH, STARTED_LONG_AGO, 0.0, true);
        assert!(s.is_disabled(EPOCH));
        close(s.disabled_till, EPOCH + 300.0);
    }

    #[test]
    fn should_not_escalate_further_till_after_5_minutes_since_startup() {
        let recent = EPOCH - 12.0 * 60.0;
        let mut s = Status {
            provider_id: 1,
            initial_failure: Some(EPOCH - 6.0 * 60.0),
            most_recent_failure: Some(EPOCH - 120.0),
            escalation_level: 3,
            disabled_till: None,
        };
        for _ in 0..7 {
            s = MOCK.failure(s, EPOCH, recent, 0.0, true);
        }
        assert!(s.is_disabled(EPOCH));
        assert_eq!(s.escalation_level, 3);
        close(s.disabled_till, EPOCH + 300.0);
    }

    #[test]
    fn applications_wait_five_minutes_after_the_first_failure_before_escalating() {
        let s = APPLICATIONS.failure(Status { provider_id: 1, ..Default::default() }, EPOCH, STARTED_LONG_AGO, 0.0, true);
        // the first failure is in its grace period: not held back yet
        assert_eq!((s.escalation_level, s.disabled_till), (1, None));
        let s = APPLICATIONS.failure(s, EPOCH + 301.0, STARTED_LONG_AGO, 0.0, true);
        assert_eq!(s.escalation_level, 2);
        close(s.disabled_till, EPOCH + 301.0 + 300.0);
        // a Retry-After of an hour goes straight to an hour's wait, but never past the highest level
        let s = APPLICATIONS.failure(s, EPOCH + 400.0, STARTED_LONG_AGO, 3600.0, true);
        assert_eq!(s.escalation_level, 5);
        close(s.disabled_till, EPOCH + 400.0 + 3600.0);
    }

    fn blocked(id: i64, backoff_hours: f64, failure_hours: f64, now: f64) -> Status {
        Status {
            provider_id: id,
            initial_failure: Some(now - failure_hours * 3600.0),
            most_recent_failure: Some(now - 0.1 * 3600.0),
            escalation_level: 5,
            disabled_till: Some(now + backoff_hours * 3600.0),
        }
    }

    #[test]
    fn status_check_follows_application_status_check_fixture() {
        let now = EPOCH;
        let apps = |n: i64| (1..=n).map(|i| (i, format!("App{i}"))).collect::<Vec<_>>();
        // should_not_return_error_when_no_indexers
        assert_eq!(check(&[], &[], now, false), None);
        // should_return_warning_if_indexer_unavailable
        let b = vec![blocked(1, 2.0, 4.0, now)];
        assert_eq!(check(&apps(2), &b, now, false), Some(Check::Some(vec!["App1".into()])));
        // should_return_error_if_all_indexers_unavailable
        assert_eq!(check(&apps(1), &b, now, false), Some(Check::All));
        // should_return_warning_if_few_indexers_unavailable
        let b = vec![blocked(1, 2.0, 4.0, now), blocked(2, 2.0, 4.0, now)];
        assert_eq!(check(&apps(3), &b, now, false), Some(Check::Some(vec!["App1".into(), "App2".into()])));
        // the long-term check only counts failures that began more than six hours ago
        assert_eq!(check(&apps(3), &b, now, true), None);
        let b = vec![blocked(1, 2.0, 7.0, now)];
        assert_eq!(check(&apps(1), &b, now, true), Some(Check::All));
        assert_eq!(check(&apps(1), &b, now, false), None);
    }
}
