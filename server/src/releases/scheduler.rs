// Ported from autobrr v1.90.0 (internal/scheduler/schedule.go, service.go), GPL-2.0-or-later.
//! autobrr's job scheduler: jobs by name, each with its own schedule. A feed runs every interval anchored to its
//! last run ([`AnchoredSchedule`]); the feed cache cleanup runs daily at 03:05 and the list refresh every six
//! hours ([`Cron`], local time, as robfig/cron does). A job that is still running when it is due again is
//! skipped (cron's `SkipIfStillRunning`). Registering a name that already has a job replaces it.

use chrono::{DateTime, Duration, Local, TimeZone, Timelike, Utc};
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// How long a job that never ran, or became overdue while Lantern was down, waits after start (autobrr's
/// `catchUpGrace`).
pub const CATCH_UP_GRACE: Duration = Duration::seconds(10);

/// The largest pin wheel (autobrr's `maxPinWheel`).
pub const MAX_PIN_WHEEL: Duration = Duration::minutes(2);

/// When a job runs next after a time.
pub trait Schedule: Send + Sync {
    /// The first run strictly after `t`.
    fn next(&self, t: DateTime<Utc>) -> DateTime<Utc>;
}

/// Every interval, anchored to the last run and pinned to a slot of a wheel picked from the job's name, so jobs
/// sharing an interval never run in lockstep (autobrr's `anchoredSchedule`).
#[derive(Debug, Clone, PartialEq)]
pub struct AnchoredSchedule {
    /// The interval, in whole seconds.
    pub interval: Duration,
    /// The first run.
    pub first: DateTime<Utc>,
}

/// FNV-1a, 32 bits (Go's `hash/fnv` `New32a`).
fn fnv32a(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// `t` cut down to a multiple of `d` (Go's `Time.Truncate` for whole seconds and minutes).
fn truncate(t: DateTime<Utc>, d: Duration) -> DateTime<Utc> {
    let step = d.num_milliseconds();
    if step <= 0 {
        return t;
    }
    let ms = t.timestamp_millis();
    let cut = ms - ms.rem_euclid(step);
    Utc.timestamp_millis_opt(cut).single().unwrap_or(t)
}

/// The largest whole-minute divisor of `interval`, at most [`MAX_PIN_WHEEL`] (autobrr's `pinWheel`).
pub fn pin_wheel(interval: Duration) -> Duration {
    let minutes = interval.num_minutes();
    let mut d = MAX_PIN_WHEEL.num_minutes();
    while d > 1 {
        if minutes % d == 0 {
            return Duration::minutes(d);
        }
        d -= 1;
    }
    Duration::minutes(1)
}

/// The time nearest `t` on `pin` within the wheel, at most half a wheel away (autobrr's `pinNearest`).
fn pin_nearest(t: DateTime<Utc>, wheel: Duration, pin: Duration) -> DateTime<Utc> {
    if wheel.is_zero() {
        return t;
    }
    let pinned = truncate(t, wheel) + pin;
    let d = pinned - t;
    if d > wheel / 2 {
        pinned - wheel
    } else if d <= -(wheel / 2) {
        pinned + wheel
    } else {
        pinned
    }
}

/// The first time strictly after `t` on `pin` within the wheel (autobrr's `pinAfter`).
fn pin_after(t: DateTime<Utc>, wheel: Duration, pin: Duration) -> DateTime<Utc> {
    if wheel.is_zero() {
        return t;
    }
    let mut pinned = truncate(t, wheel) + pin;
    while pinned <= t {
        pinned += wheel;
    }
    pinned
}

impl AnchoredSchedule {
    /// The schedule of `identifier`, every `interval` after `last_run` (`None`: never ran) as seen at `now`
    /// (autobrr's `newAnchoredSchedule`).
    pub fn new(interval: Duration, last_run: Option<DateTime<Utc>>, now: DateTime<Utc>, identifier: &str) -> AnchoredSchedule {
        let mut interval = if interval < Duration::seconds(1) { Duration::seconds(1) } else { interval };
        interval = Duration::seconds(interval.num_seconds());
        let now = truncate(now, Duration::seconds(1));
        let (mut wheel, mut pin) = (Duration::zero(), Duration::zero());
        if interval.num_seconds() % 60 == 0 {
            wheel = pin_wheel(interval);
            pin = Duration::seconds(i64::from(fnv32a(identifier)) % wheel.num_seconds());
        }
        let last_run = last_run.map(|l| if l > now { now } else { l });
        if let Some(l) = last_run {
            let first = pin_nearest(truncate(l, Duration::seconds(1)), wheel, pin) + interval;
            if first > now + Duration::seconds(1) {
                return AnchoredSchedule { interval, first };
            }
        }
        AnchoredSchedule { interval, first: pin_after(now + CATCH_UP_GRACE, wheel, pin) }
    }
}

impl Schedule for AnchoredSchedule {
    fn next(&self, t: DateTime<Utc>) -> DateTime<Utc> {
        if self.first > t {
            return self.first;
        }
        let elapsed = (t - self.first).num_milliseconds();
        let step = self.interval.num_milliseconds().max(1);
        let mut next = self.first + Duration::milliseconds(elapsed - elapsed % step);
        while next <= t {
            next += self.interval;
        }
        next
    }
}

/// A fixed cron line of the two kinds autobrr's jobs use, in local time: `minute hour * * *` (daily) and
/// `0 */n * * *` (every n hours, on the hour).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cron {
    /// Every day at the hour and minute.
    Daily(u32, u32),
    /// At minute 0 of every hour divisible by n.
    EveryHours(u32),
}

impl Schedule for Cron {
    fn next(&self, t: DateTime<Utc>) -> DateTime<Utc> {
        let local = t.with_timezone(&Local);
        let mut c = local.with_nanosecond(0).unwrap_or(local).with_second(0).unwrap_or(local) + Duration::minutes(1);
        // at most a little over a day of minutes to look through
        for _ in 0..(60 * 25) {
            let ok = match *self {
                Cron::Daily(h, m) => c.hour() == h && c.minute() == m,
                Cron::EveryHours(n) => c.minute() == 0 && n > 0 && c.hour().is_multiple_of(n),
            };
            if ok {
                return c.with_timezone(&Utc);
            }
            c += Duration::minutes(1);
        }
        t + Duration::days(1)
    }
}

/// A job's work.
pub type JobFn = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// One registered job.
struct Entry {
    /// The running loop.
    task: tokio::task::JoinHandle<()>,
    /// When it runs next.
    next: Arc<Mutex<DateTime<Utc>>>,
}

/// The jobs by name (autobrr's scheduler `Service`).
#[derive(Clone, Default)]
pub struct Scheduler {
    jobs: Arc<Mutex<HashMap<String, Entry>>>,
}

impl Scheduler {
    /// Runs `job` on `schedule` under `identifier`, replacing a job of the same name (autobrr's `registerJob`).
    pub fn register(&self, identifier: &str, schedule: Box<dyn Schedule>, job: JobFn) {
        let next = Arc::new(Mutex::new(schedule.next(Utc::now())));
        let shared = next.clone();
        let task = tokio::spawn(async move {
            loop {
                let at = *shared.lock().unwrap_or_else(|e| e.into_inner());
                let wait = (at - Utc::now()).to_std().unwrap_or_default();
                tokio::time::sleep(wait).await;
                // the next run is known as soon as this one starts, as cron's entries say
                *shared.lock().unwrap_or_else(|e| e.into_inner()) = schedule.next(at.max(Utc::now()));
                job().await;
                let now = Utc::now();
                let mut n = shared.lock().unwrap_or_else(|e| e.into_inner());
                if *n <= now {
                    // a run that took longer than the interval skips the runs it covered
                    *n = schedule.next(now);
                }
            }
        });
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = jobs.insert(identifier.to_string(), Entry { task, next }) {
            old.task.abort();
        }
    }

    /// Stops and forgets job `identifier`; nothing when there is none (autobrr's `RemoveJobByIdentifier`).
    pub fn remove(&self, identifier: &str) {
        if let Some(old) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).remove(identifier) {
            old.task.abort();
        }
    }

    /// When job `identifier` runs next; `None` when there is no such job (autobrr's `GetNextRun` answers the
    /// zero time then).
    pub fn next_run(&self, identifier: &str) -> Option<DateTime<Utc>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(identifier).map(|e| *e.next.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Whether job `identifier` is registered.
    pub fn has(&self, identifier: &str) -> bool {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).contains_key(identifier)
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/scheduler/schedule_test.go.
    use super::*;
    use std::collections::HashMap;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 11, 28, 14, 42, 13).unwrap() + Duration::milliseconds(500)
    }

    fn within(a: DateTime<Utc>, b: DateTime<Utc>, d: Duration) -> bool {
        (a - b).abs() <= d
    }

    #[test]
    fn test_anchored_schedule_next() {
        let now = now();
        // first run is lastRun plus interval
        let last = now - Duration::minutes(5);
        for i in 0..100 {
            let s = AnchoredSchedule::new(Duration::minutes(15), Some(last), now, &format!("feed-{i}"));
            let next = s.next(now);
            assert!(next > now, "next run must be in the future");
            assert!(within(last + Duration::minutes(15), next, pin_wheel(Duration::minutes(15)) / 2));
        }
        // keeps the interval between runs
        let s = AnchoredSchedule::new(Duration::minutes(15), Some(now - Duration::minutes(5)), now, "feed-1");
        let mut next = s.next(now);
        for _ in 0..10 {
            let following = s.next(next);
            assert_eq!(following - next, Duration::minutes(15));
            next = following;
        }
        // recovers cadence after a delayed wake
        let next = s.next(now);
        assert_eq!(s.next(next + Duration::seconds(90)) - next, Duration::minutes(15));
        // job that never ran catches up within a wheel
        for i in 0..100 {
            let s = AnchoredSchedule::new(Duration::minutes(720), None, now, &format!("feed-{i}"));
            let next = s.next(now);
            assert!(next > now + CATCH_UP_GRACE);
            assert!(next - now <= CATCH_UP_GRACE + MAX_PIN_WHEEL, "catch-up must not wait a full interval");
        }
        // overdue job catches up within a wheel
        let s = AnchoredSchedule::new(Duration::minutes(720), Some(now - Duration::hours(13)), now, "feed-1");
        let next = s.next(now);
        assert!(next > now);
        assert!(next - now <= CATCH_UP_GRACE + MAX_PIN_WHEEL);
        assert_eq!(s.next(next) - next, Duration::minutes(720), "interval resumes from the catch-up run");
        // lastRun in the future is clamped to now
        let s = AnchoredSchedule::new(Duration::minutes(15), Some(now + Duration::hours(24)), now, "feed-1");
        let next = s.next(now);
        assert!(next > now);
        assert!(within(now + Duration::minutes(15), next, pin_wheel(Duration::minutes(15)) / 2));
        // is stable for the same identifier
        let a = AnchoredSchedule::new(Duration::minutes(15), Some(now - Duration::minutes(5)), now, "feed-7");
        let b = AnchoredSchedule::new(Duration::minutes(15), Some(now - Duration::minutes(5)), now, "feed-7");
        assert_eq!(a.first, b.first);
        assert_eq!(a.next(now), b.next(now));
        // restart re-anchors to the same phase
        let s = AnchoredSchedule::new(Duration::minutes(720), Some(now - Duration::hours(5)), now, "feed-7");
        let fire = s.next(now);
        let re = AnchoredSchedule::new(Duration::minutes(720), Some(fire + Duration::seconds(4)), fire + Duration::hours(2), "feed-7");
        assert_eq!(re.next(fire + Duration::hours(2)), fire + Duration::minutes(720));
        // clamps intervals below a second
        for interval in [Duration::zero(), -Duration::minutes(1), Duration::milliseconds(500)] {
            let s = AnchoredSchedule::new(interval, None, now, "feed-1");
            assert_eq!(s.interval, Duration::seconds(1));
            assert!(s.next(now) > now);
        }
        // truncates sub-second intervals to whole seconds
        let s = AnchoredSchedule::new(Duration::seconds(90) + Duration::milliseconds(400), None, now, "feed-1");
        assert_eq!(s.interval, Duration::seconds(90));
        assert_eq!(s.next(now).timestamp_subsec_nanos(), 0);
    }

    #[test]
    fn test_pin_wheel() {
        let cases = [(1, 1), (5, 1), (15, 1), (30, 2), (60, 2), (720, 2), (7, 1), (13, 1)];
        for (interval, wheel) in cases {
            let i = Duration::minutes(interval);
            assert_eq!(pin_wheel(i), Duration::minutes(wheel), "interval {interval}m");
            assert_eq!(i.num_seconds() % pin_wheel(i).num_seconds(), 0, "wheel must divide the interval");
        }
    }

    #[test]
    fn test_anchored_schedule_spreads_feeds_sharing_an_interval() {
        let interval = Duration::minutes(15);
        let feeds = 20;
        let now = Utc.with_ymd_and_hms(2025, 11, 28, 14, 42, 13).unwrap();
        let last = now - Duration::minutes(3);
        let mut fires: HashMap<DateTime<Utc>, i32> = HashMap::new();
        let mut after: HashMap<DateTime<Utc>, i32> = HashMap::new();
        for i in 0..feeds {
            let s = AnchoredSchedule::new(interval, Some(last), now, &format!("feed-{i}"));
            *fires.entry(s.next(now)).or_default() += 1;
            *after.entry(s.next(now + interval + Duration::minutes(1))).or_default() += 1;
        }
        for m in [&fires, &after] {
            assert!(m.len() > feeds / 2, "fires should be spread across distinct seconds");
            assert!(m.values().all(|c| *c <= 3));
        }
    }

    #[test]
    fn test_anchored_schedule_spreads_overdue_feeds_at_boot() {
        let feeds = 30;
        let now = Utc.with_ymd_and_hms(2025, 11, 28, 14, 42, 13).unwrap();
        let mut fires: HashMap<DateTime<Utc>, i32> = HashMap::new();
        for i in 0..feeds {
            let s = AnchoredSchedule::new(Duration::minutes(720), Some(now - Duration::hours(13)), now, &format!("feed-{i}"));
            *fires.entry(s.next(now)).or_default() += 1;
        }
        assert!(fires.len() > feeds * 3 / 4);
        assert!(fires.values().all(|c| *c <= 2));
    }

    #[test]
    fn cron_lines() {
        let t = Local.with_ymd_and_hms(2026, 3, 1, 3, 4, 59).unwrap().with_timezone(&Utc);
        let n = Cron::Daily(3, 5).next(t).with_timezone(&Local);
        assert_eq!((n.hour(), n.minute(), n.second()), (3, 5, 0));
        let n2 = Cron::Daily(3, 5).next(n.with_timezone(&Utc)).with_timezone(&Local);
        assert_eq!(n2.date_naive(), n.date_naive().succ_opt().unwrap());
        let t = Local.with_ymd_and_hms(2026, 3, 1, 7, 30, 0).unwrap().with_timezone(&Utc);
        let n = Cron::EveryHours(6).next(t).with_timezone(&Local);
        assert_eq!((n.hour(), n.minute()), (12, 0));
    }
}
