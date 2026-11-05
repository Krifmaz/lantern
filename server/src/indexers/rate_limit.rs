// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Common/TPL/RateLimitService.cs, the `RateLimit` of
// src/NzbDrone.Core/Indexers/HttpIndexerBase.cs and src/NzbDrone.Core/Indexers/Definitions/Cardigann/Cardigann.cs),
// GPL-3.0.

//! How long Lantern waits between two requests to the same site (Prowlarr's `RateLimitService`): one shared list
//! of "not before" times, one per host name, for every indexer at once. Each request waits until its host's time
//! has come, then moves that time on by its own interval.
//!
//! The interval is Prowlarr's: at least 2 seconds for every indexer (`HttpIndexerBase.RateLimit`), or the
//! definition's `requestDelay` when that is longer (`Cardigann.RateLimit`). Lantern adds two things: an indexer's own
//! `request_delay` setting counts too, and the 2-second floor is left out for sites on this machine (a local
//! Jackett or test site), see `docs/DECISIONS.md`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Prowlarr's least time between two requests to one site (`HttpIndexerBase.RateLimit`).
pub const DEFAULT: Duration = Duration::from_secs(2);

/// The "not before" times, one per key (Prowlarr's `rateLimitStore`).
#[derive(Default)]
pub struct Store {
    until: Mutex<HashMap<String, Instant>>,
}

impl Store {
    /// How long a request for `key` has to wait now, moving the key's time on by `interval` (`GetDelay`).
    pub fn delay(&self, key: &str, interval: Duration) -> Duration {
        self.delay_at(key, interval, Instant::now())
    }

    /// [`Store::delay`] as of `now`.
    pub fn delay_at(&self, key: &str, interval: Duration, now: Instant) -> Duration {
        let mut map = self.until.lock().unwrap_or_else(|e| e.into_inner());
        let wait_until = match map.get(key) {
            None => now + interval,
            Some(t) => (*t).max(now) + interval,
        };
        map.insert(key.to_string(), wait_until);
        (wait_until - interval).saturating_duration_since(now)
    }

    /// Sets `key`'s time (tests: `GivenExisting`).
    pub fn set(&self, key: &str, at: Instant) {
        self.until.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string(), at);
    }

    /// `key`'s time, when it has one.
    pub fn get(&self, key: &str) -> Option<Instant> {
        self.until.lock().unwrap_or_else(|e| e.into_inner()).get(key).copied()
    }

    /// Waits until a request for `key` may go, and moves the key's time on by `interval` (`WaitAndPulseAsync`).
    /// Nothing is waited for an interval of zero.
    pub async fn wait_and_pulse(&self, key: &str, interval: Duration) {
        if interval.is_zero() {
            return;
        }
        let wait = self.delay(key, interval);
        if !wait.is_zero() {
            tracing::trace!("Rate Limit triggered, delaying '{key}' for {:.3} sec", wait.as_secs_f64());
            tokio::time::sleep(wait).await;
        }
    }
}

/// The one store every indexer shares (Prowlarr keeps it in a cache for the whole app).
pub fn shared() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(Store::default)
}

/// Whether `host` is this machine (`localhost`, `127.0.0.0/8`, `::1`).
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The interval of a request to `host`: the larger of Prowlarr's 2 seconds (left out for this machine) and
/// `delay_secs` (the definition's `requestDelay` or the indexer's own `request_delay`).
pub fn interval(host: &str, delay_secs: f64) -> Duration {
    let floor = if is_loopback(host) { Duration::ZERO } else { DEFAULT };
    let own = Duration::from_secs_f64(delay_secs.clamp(0.0, 600.0));
    floor.max(own)
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Common.Test/TPLTests/RateLimitServiceFixture.cs).
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[tokio::test]
    async fn should_not_delay_if_unset() {
        let s = Store::default();
        let watch = Instant::now();
        s.wait_and_pulse("me", ms(100)).await;
        assert!(watch.elapsed() < ms(100));
    }

    #[tokio::test]
    async fn should_not_delay_unrelated_key() {
        let s = Store::default();
        s.set("other", Instant::now() + ms(200));
        let watch = Instant::now();
        s.wait_and_pulse("me", ms(100)).await;
        assert!(watch.elapsed() < ms(50));
    }

    #[tokio::test]
    async fn should_wait_for_existing() {
        let s = Store::default();
        let epoch = Instant::now();
        s.set("me", epoch + ms(200));
        let watch = Instant::now();
        s.wait_and_pulse("me", ms(400)).await;
        let took = watch.elapsed();
        assert!(took >= ms(175) && took <= ms(400), "{took:?}");
    }

    #[test]
    fn should_extend_delay() {
        let s = Store::default();
        let epoch = Instant::now();
        s.set("me", epoch + ms(200));
        let _ = s.delay("me", ms(100));
        assert!(s.get("me").unwrap() - epoch >= ms(300));
    }

    #[test]
    fn should_add_delay() {
        let s = Store::default();
        let epoch = Instant::now();
        let _ = s.delay("me", ms(100));
        assert!(s.get("me").unwrap() - epoch >= ms(100));
    }

    #[test]
    fn the_interval_is_prowlarrs_two_seconds_or_the_definitions_delay() {
        assert_eq!(interval("tracker.test", 0.0), DEFAULT);
        assert_eq!(interval("tracker.test", 1.0), DEFAULT);
        assert_eq!(interval("tracker.test", 5.5), Duration::from_secs_f64(5.5));
        assert_eq!(interval("127.0.0.1", 0.0), Duration::ZERO);
        assert_eq!(interval("localhost", 3.0), Duration::from_secs(3));
        assert!(is_loopback("[::1]") && !is_loopback("10.0.0.1"));
    }
}
