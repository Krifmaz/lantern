// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/ThingiProvider/Status/ProviderStatusServiceBase.cs,
// ProviderStatusBase.cs and EscalationBackOff.cs), GPL-3.0.
//! An indexer's health: each failure puts it on hold for longer (1 minute, 5, 15, 30, an hour, 3, 6, 12, at most a
//! day), each success steps back once, and in the first 15 minutes after a start no hold is longer than 5 minutes.

use std::sync::OnceLock;

/// The holds, in seconds, by escalation level (`EscalationBackOff.Periods`).
pub const PERIODS: [f64; 10] = [0.0, 60.0, 300.0, 900.0, 1800.0, 3600.0, 10800.0, 21600.0, 43200.0, 86400.0];

/// The highest escalation level.
pub const MAX_LEVEL: i64 = PERIODS.len() as i64 - 1;

/// How long after a start holds are kept short (`MinimumTimeSinceStartup`, 15 minutes).
pub const STARTUP_GRACE: f64 = 900.0;

/// How long after a first failure the hold doesn't grow (`MinimumTimeSinceInitialFailure`, none).
pub const INITIAL_FAILURE_GRACE: f64 = 0.0;

/// When Lantern started (seconds since 1970), for the startup grace.
static STARTED: OnceLock<f64> = OnceLock::new();

/// Notes when Lantern started (the first call counts).
pub fn mark_start(now: f64) -> f64 {
    *STARTED.get_or_init(|| now)
}

/// When Lantern started.
pub fn start_time() -> f64 {
    mark_start(crate::auth::now())
}

/// One indexer's health (`ProviderStatusBase`); times are seconds since 1970.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Status {
    /// When the failures in a row began.
    pub initial_failure: Option<f64>,
    /// The last failure.
    pub most_recent_failure: Option<f64>,
    /// How far the holds have grown.
    pub escalation_level: i64,
    /// On hold until then.
    pub disabled_till: Option<f64>,
}

impl Status {
    /// Whether it is on hold at `now` (`IsDisabled`).
    pub fn is_disabled(&self, now: f64) -> bool {
        self.disabled_till.is_some_and(|t| t > now)
    }

    /// The hold of the current level (`CalculateBackOffPeriod`).
    fn back_off(&self) -> f64 {
        PERIODS[self.escalation_level.clamp(0, MAX_LEVEL) as usize]
    }

    /// A success: one level back and no hold. False when nothing changed (it was healthy) (`RecordSuccess`).
    pub fn record_success(&mut self) -> bool {
        if self.escalation_level == 0 {
            return false;
        }
        self.escalation_level -= 1;
        self.disabled_till = None;
        true
    }

    /// A failure at `now`, Lantern having started at `started`: the hold grows a level (not on the first failure,
    /// nor when `escalate` is false), and is at least `minimum` seconds when the site said how long to wait
    /// (`RecordFailure`).
    pub fn record_failure(&mut self, now: f64, started: f64, minimum: f64, escalate: bool) {
        let mut escalate = escalate;
        self.most_recent_failure = Some(now);
        if self.escalation_level == 0 {
            self.initial_failure = Some(now);
            self.escalation_level = 1;
            escalate = false;
        }
        let in_startup_grace = started + STARTUP_GRACE > now;
        let in_grace = self.initial_failure.unwrap_or(now) + INITIAL_FAILURE_GRACE > now;
        if escalate && !in_grace && !in_startup_grace {
            self.escalation_level = (self.escalation_level + 1).min(MAX_LEVEL);
        }
        if minimum != 0.0 {
            while self.escalation_level < MAX_LEVEL && self.back_off() < minimum {
                self.escalation_level += 1;
            }
        }
        if !in_grace || minimum != 0.0 {
            self.disabled_till = Some(now + self.back_off());
        }
        if in_startup_grace && minimum == 0.0 {
            if let Some(t) = self.disabled_till {
                let most = now + PERIODS[2];
                if most < t {
                    self.disabled_till = Some(most);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/ThingiProviderTests/ProviderStatusServiceFixture.cs
    // and src/NzbDrone.Core.Test/IndexerTests/IndexerStatusServiceFixture.cs).
    use super::*;

    const EPOCH: f64 = 1_800_000_000.0;
    const AN_HOUR_AGO: f64 = EPOCH - 3600.0;
    const PRECISION: f64 = 0.5;

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() <= PRECISION)
    }

    #[test]
    fn should_start_backoff_on_first_failure() {
        let mut s = Status::default();
        s.record_failure(EPOCH, AN_HOUR_AGO, 0.0, true);
        assert!(s.is_disabled(EPOCH));
        assert!(close(s.disabled_till, EPOCH + 60.0), "{s:?}");
    }

    #[test]
    fn should_cancel_backoff_on_success() {
        let mut s = Status { escalation_level: 2, ..Status::default() };
        assert!(s.record_success());
        assert!(!s.is_disabled(EPOCH));
    }

    #[test]
    fn should_not_store_update_if_already_okay() {
        let mut s = Status { escalation_level: 0, ..Status::default() };
        assert!(!s.record_success());
    }

    #[test]
    fn should_preserve_escalation_on_intermittent_success() {
        let mut s = Status {
            initial_failure: Some(EPOCH - 20.0),
            most_recent_failure: Some(EPOCH - 4.0),
            escalation_level: 3,
            ..Status::default()
        };
        s.record_success();
        s.record_success();
        s.record_failure(EPOCH, AN_HOUR_AGO, 0.0, true);
        assert!(s.is_disabled(EPOCH));
        assert!(close(s.disabled_till, EPOCH + 300.0), "{s:?}");
    }

    #[test]
    fn should_not_escalate_further_till_after_5_minutes_since_startup() {
        let recent_start = EPOCH - 12.0 * 60.0;
        let orig = Status {
            initial_failure: Some(EPOCH - 360.0),
            most_recent_failure: Some(EPOCH - 120.0),
            escalation_level: 3,
            ..Status::default()
        };
        let mut s = orig;
        for _ in 0..7 {
            s.record_failure(EPOCH, recent_start, 0.0, true);
        }
        assert!(s.is_disabled(EPOCH));
        assert_eq!(orig.escalation_level, 3);
        assert!(close(s.disabled_till, EPOCH + 300.0), "{s:?}");
    }

    #[test]
    fn a_site_that_says_wait_is_held_at_least_that_long() {
        let mut s = Status::default();
        s.record_failure(EPOCH, AN_HOUR_AGO, 3600.0, true);
        assert_eq!(s.escalation_level, 5);
        assert!(close(s.disabled_till, EPOCH + 3600.0));
        let mut s = Status::default();
        s.record_failure(EPOCH, AN_HOUR_AGO, 0.0, true);
        s.record_failure(EPOCH + 61.0, AN_HOUR_AGO, 0.0, true);
        assert_eq!(s.escalation_level, 2);
        let mut s = Status { escalation_level: 1, initial_failure: Some(EPOCH), ..Status::default() };
        s.record_failure(EPOCH + 61.0, AN_HOUR_AGO, 0.0, false);
        assert_eq!(s.escalation_level, 1, "a connection failure doesn't grow the hold");
    }
}
