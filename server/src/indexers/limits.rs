// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Indexers/IndexerLimitService.cs,
// src/NzbDrone.Core/Indexers/IndexerBaseSettings.cs and src/NzbDrone.Core/Indexers/IndexerTorrentBaseSettings.cs),
// GPL-3.0.
//! The settings every indexer has besides its own: query and grab limits per day or hour, and for torrent indexers
//! the seeding goals apps are given and whether apps get magnet links first. They are kept with the indexer's other
//! settings, under Prowlarr's field names (`baseSettings.queryLimit` …).

use std::collections::BTreeMap;

/// The setting: how many searches in a day (or hour).
pub const QUERY_LIMIT: &str = "baseSettings.queryLimit";
/// The setting: how many grabs in a day (or hour).
pub const GRAB_LIMIT: &str = "baseSettings.grabLimit";
/// The setting: `0` the limits count per day, `1` per hour.
pub const LIMITS_UNIT: &str = "baseSettings.limitsUnit";
/// The setting: the fewest seeders apps should take.
pub const APP_MINIMUM_SEEDERS: &str = "torrentBaseSettings.appMinimumSeeders";
/// The setting: the ratio to seed to.
pub const SEED_RATIO: &str = "torrentBaseSettings.seedRatio";
/// The setting: minutes to seed.
pub const SEED_TIME: &str = "torrentBaseSettings.seedTime";
/// The setting: minutes to seed a season pack.
pub const PACK_SEED_TIME: &str = "torrentBaseSettings.packSeedTime";
/// The setting: hand apps the magnet link rather than the torrent file when there are both.
pub const PREFER_MAGNET_URL: &str = "torrentBaseSettings.preferMagnetUrl";

/// The limits of one indexer (`IndexerBaseSettings`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BaseSettings {
    /// Searches allowed per unit (none: no limit).
    pub query_limit: Option<i64>,
    /// Grabs allowed per unit (none: no limit).
    pub grab_limit: Option<i64>,
    /// `0` per day (the default), `1` per hour.
    pub limits_unit: i64,
}

/// The torrent settings of one indexer (`IndexerTorrentBaseSettings`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TorrentBaseSettings {
    /// The fewest seeders apps should take.
    pub app_minimum_seeders: Option<i64>,
    /// The ratio to seed to.
    pub seed_ratio: Option<f64>,
    /// Minutes to seed.
    pub seed_time: Option<i64>,
    /// Minutes to seed a season pack.
    pub pack_seed_time: Option<i64>,
    /// Apps get the magnet link first.
    pub prefer_magnet_url: bool,
}

/// What a download client is told about seeding a torrent from an indexer (`TorrentSeedConfiguration`, made by
/// `SeedConfigProvider` from the indexer's torrent settings).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SeedConfiguration {
    /// The ratio to seed to (none: the client's own).
    pub ratio: Option<f64>,
    /// How long to seed (none: the client's own).
    pub seed_time: Option<std::time::Duration>,
}

impl SeedConfiguration {
    /// The seeding goals of torrent settings `t` (`SeedConfigProvider.GetSeedConfiguration`): the ratio as it is,
    /// the seed time from minutes.
    pub fn of(t: &TorrentBaseSettings) -> SeedConfiguration {
        SeedConfiguration { ratio: t.seed_ratio, seed_time: t.seed_time.map(|m| std::time::Duration::from_secs((m.max(0) as u64) * 60)) }
    }
}

/// A number setting: none when missing or empty.
fn num<T: std::str::FromStr>(s: &BTreeMap<String, String>, k: &str) -> Option<T> {
    s.get(k).map(|v| v.trim()).filter(|v| !v.is_empty() && *v != "null").and_then(|v| v.parse().ok())
}

impl BaseSettings {
    /// The limits in `settings`.
    pub fn of(settings: &BTreeMap<String, String>) -> BaseSettings {
        BaseSettings {
            query_limit: num(settings, QUERY_LIMIT),
            grab_limit: num(settings, GRAB_LIMIT),
            limits_unit: num(settings, LIMITS_UNIT).unwrap_or(0),
        }
    }

    /// The hours the limits count over: 1 per hour, else 24 (`CalculateIntervalLimitHours`).
    pub fn interval_hours(&self) -> i64 {
        if self.limits_unit == 1 {
            1
        } else {
            24
        }
    }
}

impl TorrentBaseSettings {
    /// The torrent settings in `settings`.
    pub fn of(settings: &BTreeMap<String, String>) -> TorrentBaseSettings {
        TorrentBaseSettings {
            app_minimum_seeders: num(settings, APP_MINIMUM_SEEDERS),
            seed_ratio: num(settings, SEED_RATIO),
            seed_time: num(settings, SEED_TIME),
            pack_seed_time: num(settings, PACK_SEED_TIME),
            prefer_magnet_url: settings.get(PREFER_MAGNET_URL).is_some_and(|v| v.trim().eq_ignore_ascii_case("true") || v.trim() == "1"),
        }
    }
}

/// A problem with one setting: its name and what is wrong, in words.
pub type Problem = (&'static str, &'static str);

/// What is wrong with the limits and torrent settings in `settings`: errors (they can't be saved) and warnings
/// (`IndexerCommonSettingsValidator`, `IndexerTorrentSettingsValidator`), each as (setting, message).
pub fn validate(settings: &BTreeMap<String, String>) -> (Vec<Problem>, Vec<Problem>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let b = BaseSettings::of(settings);
    for (k, v) in [(QUERY_LIMIT, b.query_limit), (GRAB_LIMIT, b.grab_limit)] {
        if v.is_some_and(|n| n <= 0) {
            errors.push((k, "Should be greater than zero"));
        }
    }
    let t = TorrentBaseSettings::of(settings);
    for (k, bad) in [
        (APP_MINIMUM_SEEDERS, t.app_minimum_seeders.is_some_and(|n| n <= 0)),
        (SEED_RATIO, t.seed_ratio.is_some_and(|n| n <= 0.0)),
        (SEED_TIME, t.seed_time.is_some_and(|n| n <= 0)),
        (PACK_SEED_TIME, t.pack_seed_time.is_some_and(|n| n <= 0)),
    ] {
        if bad {
            warnings.push((k, "Should be greater than zero"));
        }
    }
    (errors, warnings)
}

/// Whether `count` uses of a limit `limit` reach it (`AtQueryLimit`, `AtDownloadLimit`).
pub fn at_limit(limit: Option<i64>, count: i64) -> bool {
    limit.is_some_and(|l| count >= l)
}

/// Seconds until the oldest of the last `limit` uses (`dates`, any order) falls out of the `hours` window
/// (`CalculateRetryAfterQueryLimit`, `CalculateRetryAfterDownloadLimit`); 0 when there are none.
pub fn retry_after(dates: &[f64], limit: i64, hours: i64, now: f64) -> i64 {
    let since = now - hours as f64 * 3600.0;
    let mut recent: Vec<f64> = dates.iter().copied().filter(|d| *d >= since).collect();
    recent.sort_by(|a, b| b.total_cmp(a));
    if limit > 0 {
        recent.truncate(limit as usize);
    }
    match recent.iter().copied().reduce(f64::min) {
        Some(first) => (first + hours as f64 * 3600.0 - now).round() as i64,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn reads_and_checks_the_limits() {
        let s = settings(&[(QUERY_LIMIT, "5"), (GRAB_LIMIT, ""), (LIMITS_UNIT, "1"), (PREFER_MAGNET_URL, "true"), (SEED_RATIO, "1.5")]);
        assert_eq!(BaseSettings::of(&s), BaseSettings { query_limit: Some(5), grab_limit: None, limits_unit: 1 });
        assert_eq!(BaseSettings::of(&s).interval_hours(), 1);
        assert_eq!(BaseSettings::of(&BTreeMap::new()).interval_hours(), 24);
        let t = TorrentBaseSettings::of(&s);
        assert!(t.prefer_magnet_url && t.seed_ratio == Some(1.5));
        assert!(at_limit(Some(5), 5) && !at_limit(Some(5), 4) && !at_limit(None, 100));
        let (errors, warnings) = validate(&settings(&[(QUERY_LIMIT, "0"), (SEED_TIME, "-1")]));
        assert_eq!(errors, vec![(QUERY_LIMIT, "Should be greater than zero")]);
        assert_eq!(warnings, vec![(SEED_TIME, "Should be greater than zero")]);
    }

    #[test]
    fn seed_goals_follow_seed_config_provider() {
        let t = TorrentBaseSettings::of(&settings(&[(SEED_RATIO, "1.5"), (SEED_TIME, "90"), (PACK_SEED_TIME, "300")]));
        let c = SeedConfiguration::of(&t);
        assert_eq!(c.ratio, Some(1.5));
        assert_eq!(c.seed_time, Some(std::time::Duration::from_secs(90 * 60)));
        assert_eq!(SeedConfiguration::of(&TorrentBaseSettings::default()), SeedConfiguration { ratio: None, seed_time: None });
    }

    #[test]
    fn retry_after_counts_from_the_oldest_of_the_last_uses() {
        let now = 100_000.0;
        // three grabs in the last day, limit 2: the older of the two newest falls out first
        let dates = [now - 80_000.0, now - 3_600.0, now - 7_200.0, now - 90_000.0];
        assert_eq!(retry_after(&dates, 2, 24, now), 86_400 - 7_200);
        assert_eq!(retry_after(&[], 2, 24, now), 0);
    }
}
