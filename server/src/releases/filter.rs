// Ported from autobrr v1.90.0 (internal/domain/filter.go), GPL-2.0-or-later.
//! Filters: which releases to grab. A filter lists what a release must have (names, groups, resolution, source,
//! codec, HDR, size, freeleech, years, seasons, tags, music details …) and what it must not;
//! [`Filter::check_filter`] says whether a release passes and, when it doesn't, why
//! ([`RejectionReasons`]), exactly as autobrr does.
//!
//! Lists are comma separated. A word with `*` (any text) or `?` (one character) is a wildcard that must match the
//! whole value; case never matters. Lantern keeps three things of its own: which of its release sources a
//! filter watches ([`Filter::indexers`]), skipping a name already grabbed ([`Filter::skip_duplicates`]) and its
//! simple actions ([`Action`]).

use super::rejections::RejectionReasons;
use super::release::Release;
use super::rls::goregex;
use super::{humanize, sanitize, wildcard};
use chrono::{DateTime, Datelike, Duration, Local, Months, NaiveTime, TimeZone, Timelike};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// How many releases a filter took in its current download window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FilterDownloads {
    /// The count in the window (for `EVER`, since the filter was made).
    pub period_count: i64,
}

impl std::fmt::Display for FilterDownloads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Period: {}", self.period_count)
    }
}

/// The download limit's units.
pub const MAX_DOWNLOADS_UNITS: [&str; 6] = ["MINUTE", "HOUR", "DAY", "WEEK", "MONTH", "EVER"];

/// One filter, as saved and as the API takes it (autobrr's JSON names).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Its id (0 before it is saved).
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// Used at all.
    pub enabled: bool,
    /// Smallest size (`700 MB`, `1.5 GB`).
    pub min_size: String,
    /// Largest size (`30 GB`).
    pub max_size: String,
    /// Seconds to wait before the actions run (0: at once, best for racing).
    pub delay: i64,
    /// Higher is checked first; the first filter a release passes takes it.
    pub priority: i64,
    /// The most releases to take in each download window (0: no limit).
    pub max_downloads: i64,
    /// The window's unit: `MINUTE`, `HOUR`, `DAY`, `WEEK`, `MONTH` or `EVER`.
    pub max_downloads_unit: String,
    /// How many units a rolling window spans.
    pub max_downloads_period: i64,
    /// `FIXED` (resets at the start of each unit; the default) or `ROLLING` (the last period units).
    pub max_downloads_window_type: String,
    /// The release name must match one of these.
    pub match_releases: String,
    /// The release name must match none of these.
    pub except_releases: String,
    /// Read the release names to match and skip as regular expressions.
    pub use_regex: bool,
    /// Release groups, one of.
    pub match_release_groups: String,
    /// Release groups, none of.
    pub except_release_groups: String,
    /// Announce types, one of (`NEW`, `CHECKED`, `PROMO`, `PROMO_GP`, `RESURRECTED`).
    pub announce_types: Vec<String>,
    /// Scene releases only (kept for autobrr; the check uses origins).
    pub scene: bool,
    /// Origins, one of (`P2P`, `SCENE`, `INTERNAL`).
    pub origins: Vec<String>,
    /// Origins, none of.
    pub except_origins: Vec<String>,
    /// Bonuses, one of (`Freeleech`, `Freeleech50`).
    pub bonus: Vec<String>,
    /// Freeleech releases only.
    pub freeleech: bool,
    /// Freeleech percentages (`50-100`).
    pub freeleech_percent: String,
    /// Take an episode only when it is newer than the ones already taken.
    pub smart_episode: bool,
    /// Show (or movie) titles, one of.
    pub shows: String,
    /// Seasons (`1-3, 5`).
    pub seasons: String,
    /// Episodes (`1-10`).
    pub episodes: String,
    /// Resolutions, one of.
    pub resolutions: Vec<String>,
    /// Codecs, one of.
    pub codecs: Vec<String>,
    /// Sources (`WEB-DL`, `BluRay`), one of.
    pub sources: Vec<String>,
    /// Containers, one of.
    pub containers: Vec<String>,
    /// HDR formats, one of (`DV HDR` needs both).
    pub match_hdr: Vec<String>,
    /// HDR formats, none of.
    pub except_hdr: Vec<String>,
    /// Other tags (`REMUX`, `PROPER`), one of.
    pub match_other: Vec<String>,
    /// Other tags, none of.
    pub except_other: Vec<String>,
    /// Years (`2019-2024, 2026`).
    pub years: String,
    /// Months.
    pub months: String,
    /// Days.
    pub days: String,
    /// Artists, one of (music).
    pub artists: String,
    /// Albums, one of (music).
    pub albums: String,
    /// Release types (`Album`, `Single`, `EP`), one of (music).
    pub match_release_types: Vec<String>,
    /// Release types, none of (kept for autobrr; not checked).
    pub except_release_types: String,
    /// Audio formats (`FLAC`, `MP3`), one of.
    pub formats: Vec<String>,
    /// Audio qualities (`Lossless`, `320`), one of.
    pub quality: Vec<String>,
    /// Media (`CD`, `WEB`, `Vinyl`), one of.
    pub media: Vec<String>,
    /// Perfect FLACs only.
    pub perfect_flac: bool,
    /// With a cue file only.
    pub cue: bool,
    /// With a rip log only.
    pub log: bool,
    /// With this log score only (0: any).
    pub log_score: i64,
    /// Categories, one of.
    pub match_categories: String,
    /// Categories, none of.
    pub except_categories: String,
    /// Uploaders, one of.
    pub match_uploaders: String,
    /// Uploaders, none of.
    pub except_uploaders: String,
    /// Record labels, one of.
    pub match_record_labels: String,
    /// Record labels, none of.
    pub except_record_labels: String,
    /// Languages, one of.
    pub match_language: Vec<String>,
    /// Languages, none of.
    pub except_language: Vec<String>,
    /// Tags the release must have (any or all, see [`Filter::tags_match_logic`]).
    pub tags: String,
    /// Tags the release must not have.
    pub except_tags: String,
    /// Kept for autobrr; not checked.
    pub tags_any: String,
    /// Kept for autobrr; not checked.
    pub except_tags_any: String,
    /// `ANY` (the default) or `ALL`.
    pub tags_match_logic: String,
    /// `ANY` (the default) or `ALL`.
    pub except_tags_match_logic: String,
    /// The site's release tags must match one of these.
    pub match_release_tags: String,
    /// The site's release tags must match none of these.
    pub except_release_tags: String,
    /// Read the release tags to match and skip as regular expressions.
    pub use_regex_release_tags: bool,
    /// The description must match one of these.
    pub match_description: String,
    /// The description must match none of these.
    pub except_description: String,
    /// Read the descriptions to match and skip as regular expressions.
    pub use_regex_description: bool,
    /// Fewest seeders (feeds; 0: any).
    pub min_seeders: i64,
    /// Most seeders (feeds; 0: any).
    pub max_seeders: i64,
    /// Fewest leechers (feeds; 0: any).
    pub min_leechers: i64,
    /// Most leechers (feeds; 0: any).
    pub max_leechers: i64,
    /// Whether a list keeps it up to date.
    pub is_auto_updated: bool,
    /// The duplicate profile to use (0: none).
    pub release_profile_duplicate_id: i64,
    /// How many it took in its window (filled in before checking; not saved).
    #[serde(skip)]
    pub downloads: Option<FilterDownloads>,
    /// Lantern: only releases from these release sources (ids); empty: every source. Takes ids or autobrr's
    /// indexer objects.
    #[serde(deserialize_with = "ids")]
    pub indexers: Vec<i64>,
    /// Lantern: skip a release with the same name this filter already took from another source.
    pub skip_duplicates: bool,
    /// Lantern: where a passing release goes.
    pub actions: Vec<Action>,
    /// autobrr's external filters: a program or web address that has the last word.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub external: Vec<super::external::FilterExternal>,
}

/// Reads indexers as ids, or as objects with an `id`.
fn ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<i64>, D::Error> {
    let v: Vec<Value> = Option::<Vec<Value>>::deserialize(d)?.unwrap_or_default();
    Ok(v.iter().filter_map(|x| x.as_i64().or_else(|| x.get("id").and_then(Value::as_i64))).collect())
}

/// Where a release a filter takes goes (Lantern's own actions, until autobrr's are ported).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    /// `arr` (push to Sonarr, Radarr, Lidarr or Readarr) or `client` (a download client, through the download hub).
    pub kind: String,
    /// The name people see.
    pub name: String,
    /// `arr`: `sonarr`, `radarr`, `lidarr`, `readarr` (or `whisparr`).
    pub app: String,
    /// `arr`: the app's address (`http://sonarr:8989`).
    pub url: String,
    /// `arr`: the app's API key (sent back masked).
    pub api_key: String,
    /// `client`: the app in the download hub whose category and folder the download gets (empty: "Releases").
    pub client: String,
    /// Not used yet: the hub gives each app its own category.
    pub category: String,
    /// Not used yet: the hub adds downloads started.
    pub paused: bool,
}

/// Lantern's earlier filter fields and the autobrr fields they became (the earlier form still sends them).
const LEGACY: [(&str, &str); 4] = [
    ("release_sources", "sources"),
    ("hdr", "match_hdr"),
    ("match_groups", "match_release_groups"),
    ("except_groups", "except_release_groups"),
];

/// A saved or sent filter in Lantern's earlier shape, in autobrr's: source ids become `indexers`, the renamed
/// fields take their autobrr names (an earlier name sent wins) and `max_downloads_per` (`day`) becomes
/// `max_downloads_unit` (`DAY`).
pub fn upgrade(mut v: Value) -> Value {
    let Some(o) = v.as_object_mut() else { return v };
    if let Some(Value::Array(a)) = o.get("sources") {
        if !a.is_empty() && a.iter().all(Value::is_number) {
            let ids = o.remove("sources").unwrap_or_default();
            o.insert("indexers".into(), ids);
        }
    }
    for (old, new) in LEGACY {
        if let Some(x) = o.remove(old) {
            o.insert(new.into(), x);
        }
    }
    if let Some(Value::String(per)) = o.remove("max_downloads_per") {
        o.insert("max_downloads_unit".into(), Value::String(per.to_uppercase()));
    }
    v
}

/// A filter as JSON with Lantern's earlier field names added, so the earlier form keeps showing them.
pub fn with_legacy(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        for (old, new) in LEGACY {
            if let Some(x) = o.get(new).cloned() {
                o.insert(old.into(), x);
            }
        }
        let per = o.get("max_downloads_unit").and_then(Value::as_str).unwrap_or("").to_lowercase();
        o.insert("max_downloads_per".into(), Value::String(per));
    }
    v
}

impl Filter {
    /// A new filter with Lantern's race-friendly defaults: on, actions at once, duplicates skipped, at most
    /// [`Filter::max_downloads`] a day.
    pub fn new(name: &str) -> Filter {
        Filter { name: name.into(), enabled: true, skip_duplicates: true, max_downloads_unit: "DAY".into(), ..Filter::default() }
    }

    /// The rolling window's length in units, at least 1; a fixed window is always one unit (autobrr's
    /// `MaxDownloadsPeriodNormalized`).
    pub fn max_downloads_period_normalized(&self) -> i64 {
        if self.max_downloads_window_type != "ROLLING" || self.max_downloads_period <= 0 {
            return 1;
        }
        self.max_downloads_period
    }

    /// The rolling window as a count and unit (`minutes`, `hours`, `days`, `months`; a week is 7 days) (autobrr's
    /// `DownloadPeriod`).
    pub fn download_period(&self) -> (i64, &'static str) {
        let p = self.max_downloads_period_normalized();
        match self.max_downloads_unit.as_str() {
            "MINUTE" => (p, "minutes"),
            "HOUR" => (p, "hours"),
            "DAY" => (p, "days"),
            "WEEK" => (p * 7, "days"),
            "MONTH" => (p, "months"),
            _ => (1, "days"),
        }
    }

    /// When the current download window started, in local time; `None` for `EVER` (the filter's whole life).
    /// A fixed window starts at the last minute, hour, day, Sunday a week back (as autobrr's SQLite query reads
    /// `weekday 0, -7 days`) or month; a rolling one the period before `now`.
    pub fn downloads_window_start(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        if self.max_downloads_unit == "EVER" {
            return None;
        }
        if self.max_downloads_window_type == "ROLLING" {
            let (n, unit) = self.download_period();
            return Some(match unit {
                "minutes" => now - Duration::minutes(n),
                "hours" => now - Duration::hours(n),
                "months" => now.checked_sub_months(Months::new(n as u32)).unwrap_or(now),
                _ => now - Duration::days(n),
            });
        }
        let day = |d: chrono::NaiveDate| Local.from_local_datetime(&d.and_time(NaiveTime::MIN)).earliest();
        let today = now.date_naive();
        match self.max_downloads_unit.as_str() {
            "MINUTE" => now.with_second(0).and_then(|t| t.with_nanosecond(0)),
            "HOUR" => now.with_minute(0).and_then(|t| t.with_second(0)).and_then(|t| t.with_nanosecond(0)),
            "WEEK" => {
                let to_sunday = (7 - today.weekday().num_days_from_sunday()) % 7;
                day(today + Duration::days(i64::from(to_sunday)) - Duration::days(7))
            }
            "MONTH" => day(today.with_day(1).unwrap_or(today)),
            _ => day(today),
        }
    }

    /// Whether the filter's settings can be read; the problem when not (autobrr's `Validate`, plus Lantern's
    /// actions).
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("validation: name can't be empty".into());
        }
        if let Err(e) = self.parsed_size_limits() {
            return Err(format!("error validating filter size limits: {e}"));
        }
        if !self.max_downloads_unit.is_empty() && !MAX_DOWNLOADS_UNITS.contains(&self.max_downloads_unit.as_str()) {
            return Err(format!("validation: invalid max downloads unit: {}", self.max_downloads_unit));
        }
        if !["", "FIXED", "ROLLING"].contains(&self.max_downloads_window_type.as_str()) {
            return Err(format!("validation: invalid max downloads window type: {}", self.max_downloads_window_type));
        }
        for e in self.external.iter().filter(|e| e.kind == "EXEC" && !e.exec_cmd.is_empty() && e.enabled) {
            if let Err(err) = super::run::look_path(&e.exec_cmd) {
                return Err(format!("could not find external exec command: {}: {err}", e.exec_cmd));
            }
        }
        for a in &self.actions {
            match a.kind.as_str() {
                "arr" => {
                    if !["sonarr", "radarr", "lidarr", "readarr", "whisparr"].contains(&a.app.as_str()) {
                        return Err("Pick which app gets the release: Sonarr, Radarr, Lidarr, Readarr or Whisparr".into());
                    }
                    if !(a.url.starts_with("http://") || a.url.starts_with("https://")) {
                        return Err(format!("The address of {} must start with http:// or https://", a.app));
                    }
                }
                "client" => {}
                _ => return Err("A filter sends releases to an app (Sonarr, Radarr …) or a download client".into()),
            }
        }
        Ok(())
    }

    /// Tidies the typed lists: line breaks become commas, doubled spaces and commas go (autobrr's `Sanitize`).
    pub fn sanitize(&mut self) {
        let f = sanitize::filter_string;
        self.shows = f(&self.shows);
        if !self.use_regex {
            self.match_releases = f(&self.match_releases);
            self.except_releases = f(&self.except_releases);
        }
        if !self.use_regex_description {
            self.match_description = f(&self.match_description);
            self.except_description = f(&self.except_description);
        }
        for s in [
            &mut self.match_release_groups,
            &mut self.except_release_groups,
            &mut self.match_categories,
            &mut self.except_categories,
            &mut self.match_uploaders,
            &mut self.except_uploaders,
            &mut self.tags_any,
            &mut self.except_tags,
        ] {
            *s = f(s);
        }
        if !self.use_regex_release_tags {
            self.match_release_tags = f(&self.match_release_tags);
            self.except_release_tags = f(&self.except_release_tags);
        }
        for s in [
            &mut self.years,
            &mut self.months,
            &mut self.days,
            &mut self.artists,
            &mut self.albums,
            &mut self.match_record_labels,
            &mut self.except_record_labels,
        ] {
            *s = f(s);
        }
    }

    /// Whether the release passes, and every reason it doesn't (autobrr's `CheckFilter`). A size limit with no
    /// size known passes and sets [`Release::additional_size_check_required`]; an uploader or record label
    /// limit on RED or OPS with none known sets the matching flag.
    pub fn check_filter(&self, r: &mut Release) -> (RejectionReasons, bool) {
        let mut rr = RejectionReasons::new();
        if self.is_max_downloads_limit_enabled() && !self.check_max_downloads() {
            let period = self.max_downloads_period_normalized();
            let msg = if period > 1 {
                format!("reached {} per {} {}", self.max_downloads, period, self.max_downloads_unit)
            } else {
                format!("reached {} per {}", self.max_downloads, self.max_downloads_unit)
            };
            let downloads = self.downloads.clone().unwrap_or_default().to_string();
            rr.addf("max downloads", "[%[1]s] %[3]s", downloads, msg);
            return (rr, false);
        }
        if !self.bonus.is_empty() && !slice_contains_slice(&r.bonus, &self.bonus) {
            rr.add("bonus", &r.bonus, &self.bonus);
        }
        if self.freeleech && r.freeleech != self.freeleech {
            rr.add("freeleech", r.freeleech, self.freeleech);
        }
        if !self.freeleech_percent.is_empty() && !check_freeleech_percent(r.freeleech_percent, &self.freeleech_percent) {
            rr.add("freeleech percent", r.freeleech_percent, &self.freeleech_percent);
        }
        if !self.announce_types.is_empty() && !basic_contains_slice(&r.announce_type, &self.announce_types) {
            rr.add("match announce type", &r.announce_type, &self.announce_types);
        }
        if !self.origins.is_empty() && !contains_slice(&r.origin, &self.origins) {
            rr.add("match origin", &r.origin, &self.origins);
        }
        if !self.except_origins.is_empty() && contains_slice(&r.origin, &self.except_origins) {
            rr.add("except origin", &r.origin, &self.except_origins);
        }
        if !self.shows.is_empty() && !contains(&r.title, &self.shows) {
            rr.add("shows", &r.title, &self.shows);
        }
        if !self.seasons.is_empty() && !contains_int_strings(r.season, &self.seasons) {
            rr.add("season", r.season, &self.seasons);
        }
        if !self.episodes.is_empty() && !contains_int_strings(r.episode, &self.episodes) {
            rr.add("episodes", r.episode, &self.episodes);
        }
        if self.use_regex {
            if !self.match_releases.is_empty() && !match_regex(&r.torrent_name, &self.match_releases) {
                rr.add("match releases: REGEX", &r.torrent_name, &self.match_releases);
            }
            if !self.except_releases.is_empty() && match_regex(&r.torrent_name, &self.except_releases) {
                rr.add("except releases: REGEX", &r.torrent_name, &self.except_releases);
            }
        } else {
            if !self.match_releases.is_empty() && !contains_fuzzy(&r.torrent_name, &self.match_releases) {
                rr.add("match releases", &r.torrent_name, &self.match_releases);
            }
            if !self.except_releases.is_empty() && contains_fuzzy(&r.torrent_name, &self.except_releases) {
                rr.add("except releases", &r.torrent_name, &self.except_releases);
            }
        }
        if !self.match_release_groups.is_empty() && !contains(&r.group, &self.match_release_groups) {
            rr.add("match release groups", &r.group, &self.match_release_groups);
        }
        if !self.except_release_groups.is_empty() && contains(&r.group, &self.except_release_groups) {
            rr.add("except release groups", &r.group, &self.except_release_groups);
        }
        if self.use_regex_release_tags {
            if !self.match_release_tags.is_empty() && !match_regex(&r.release_tags, &self.match_release_tags) {
                rr.add("match release tags: REGEX", &r.release_tags, &self.match_release_tags);
            }
            if !self.except_release_tags.is_empty() && match_regex(&r.release_tags, &self.except_release_tags) {
                rr.add("except release tags: REGEX", &r.release_tags, &self.except_release_tags);
            }
        } else {
            if !self.match_release_tags.is_empty() && !contains_fuzzy(&r.release_tags, &self.match_release_tags) {
                rr.add("match release tags", &r.release_tags, &self.match_release_tags);
            }
            if !self.except_release_tags.is_empty() && contains_fuzzy(&r.release_tags, &self.except_release_tags) {
                rr.add("except release tags", &r.release_tags, &self.except_release_tags);
            }
        }
        if !self.match_uploaders.is_empty() || !self.except_uploaders.is_empty() {
            self.check_uploader(r, &mut rr);
        }
        if !self.match_record_labels.is_empty() || !self.except_record_labels.is_empty() {
            self.check_record_label(r, &mut rr);
        }
        if !self.match_language.is_empty() && !slice_contains_slice(&r.language, &self.match_language) {
            rr.add("match language", &r.language, &self.match_language);
        }
        if !self.except_language.is_empty() && slice_contains_slice(&r.language, &self.except_language) {
            rr.add("except language", &r.language, &self.except_language);
        }
        if !self.resolutions.is_empty() && !contains_slice(&r.resolution, &self.resolutions) {
            rr.add("resolution", &r.resolution, &self.resolutions);
        }
        if !self.codecs.is_empty() && !slice_contains_slice(&r.codec, &self.codecs) {
            rr.add("codec", &r.codec, &self.codecs);
        }
        if !self.sources.is_empty() && !contains_slice(&r.source, &self.sources) {
            rr.add("source", &r.source, &self.sources);
        }
        if !self.containers.is_empty() && !contains_slice(&r.container, &self.containers) {
            rr.add("container", &r.container, &self.containers);
        }
        if !self.match_hdr.is_empty() && !match_hdr(&r.hdr, &self.match_hdr) {
            rr.add("match hdr", r.hdr.join(" "), &self.match_hdr);
        }
        if !self.except_hdr.is_empty() && match_hdr(&r.hdr, &self.except_hdr) {
            rr.add("except hdr", r.hdr.join(" "), &self.except_hdr);
        }
        if !self.match_other.is_empty() && !slice_contains_slice(&r.other, &self.match_other) {
            rr.add("match other", &r.other, &self.match_other);
        }
        if !self.except_other.is_empty() && slice_contains_slice(&r.other, &self.except_other) {
            rr.add("except other", &r.other, &self.except_other);
        }
        if !self.years.is_empty() && !contains_int_strings(r.year, &self.years) {
            rr.add("year", r.year, &self.years);
        }
        if !self.months.is_empty() && !contains_int_strings(r.month, &self.months) {
            rr.add("month", r.month, &self.months);
        }
        if !self.days.is_empty() && !contains_int_strings(r.day, &self.days) {
            rr.add("day", r.day, &self.days);
        }
        let categories = || {
            let mut c = r.categories.clone();
            if !r.category.is_empty() {
                c.push(r.category.clone());
            }
            c
        };
        if !self.match_categories.is_empty() {
            let c = categories();
            if !contains(&r.category, &self.match_categories) && !contains_any(&c, &self.match_categories) {
                rr.add("match category", c.join(","), &self.match_categories);
            }
        }
        if !self.except_categories.is_empty() {
            let c = categories();
            if contains(&r.category, &self.except_categories) && contains_any(&c, &self.except_categories) {
                rr.add("except category", c.join(","), &self.except_categories);
            }
        }
        if !self.match_release_types.is_empty() && !contains_slice(&r.category, &self.match_release_types) {
            rr.add("release type", &r.category, &self.match_release_types);
        }
        if !self.min_size.is_empty() && !self.check_size_filter(r, &mut rr) {
            rr.add("min size", r.size, &self.min_size);
        }
        if !self.max_size.is_empty() && !self.check_size_filter(r, &mut rr) {
            rr.add("max size", r.size, &self.max_size);
        }
        if !self.tags.is_empty() {
            if self.tags_match_logic == "ALL" && !contains_all(&r.tags, &self.tags) {
                rr.add("match tags: ALL", &r.tags, &self.tags);
            } else if !contains_any(&r.tags, &self.tags) {
                rr.add("match tags: ANY", &r.tags, &self.tags);
            }
        }
        if !self.except_tags.is_empty() {
            if self.except_tags_match_logic == "ALL" && contains_all(&r.tags, &self.except_tags) {
                rr.add("except tags: ALL", &r.tags, &self.except_tags);
            } else if contains_any(&r.tags, &self.except_tags) {
                rr.add("except tags: ANY", &r.tags, &self.except_tags);
            }
        }
        if !self.artists.is_empty() && !contains(&r.artists, &self.artists) {
            rr.add("artists", &r.artists, &self.artists);
        }
        if !self.albums.is_empty() && !contains(&r.title, &self.albums) {
            rr.add("albums", &r.title, &self.albums);
        }
        if self.perfect_flac {
            let (rejections, ok) = self.is_perfect_flac(r);
            if !ok {
                rr.add(
                    "perfect flac",
                    format!("{} {} {} (log: {}, score: {})", r.source, r.audio_format, r.bitrate, r.has_log, r.log_score),
                    rejections.join(", "),
                );
            }
        }
        if !self.formats.is_empty() && !slice_contains_slice(&r.audio, &self.formats) {
            rr.add("formats", &r.audio, &self.formats);
        }
        if !self.quality.is_empty() && !contains_match_basic(&r.audio, &self.quality) {
            rr.add("quality", &r.audio, &self.quality);
        }
        if !self.media.is_empty() && !contains_slice(&r.source, &self.media) {
            rr.add("media", &r.source, &self.media);
        }
        if self.cue && !contains_any(&r.audio, "Cue") {
            rr.add("cue", &r.audio, "Cue");
        }
        if self.log && !contains_any(&r.audio, "Log") {
            rr.add("log", &r.audio, "Log");
        }
        if self.log && self.log_score != 0 && r.log_score != self.log_score {
            rr.add("log score", r.log_score, self.log_score);
        }
        if self.use_regex_description {
            if !self.match_description.is_empty() && !match_regex(&r.description, &self.match_description) {
                rr.add("match description: REGEX", &r.description, &self.match_description);
            }
            if !self.except_description.is_empty() && match_regex(&r.description, &self.except_description) {
                rr.add("except description: REGEX", &r.description, &self.except_description);
            }
        } else {
            if !self.match_description.is_empty() && !contains_fuzzy(&r.description, &self.match_description) {
                rr.add("match description", &r.description, &self.match_description);
            }
            if !self.except_description.is_empty() && contains_fuzzy(&r.description, &self.except_description) {
                rr.add("except description", &r.description, &self.except_description);
            }
        }
        if self.min_seeders > 0 && self.min_seeders > r.seeders {
            rr.add("min seeders", r.seeders, self.min_seeders);
        }
        if self.max_seeders > 0 && self.max_seeders < r.seeders {
            rr.add("max seeders", r.seeders, self.max_seeders);
        }
        if self.min_leechers > 0 && self.min_leechers > r.leechers {
            rr.add("min leechers", r.leechers, self.min_leechers);
        }
        if self.max_leechers > 0 && self.max_leechers < r.leechers {
            rr.add("max leechers", r.leechers, self.max_leechers);
        }
        let ok = rr.is_empty();
        (rr, ok)
    }

    /// Whether a download limit is set: a number and a unit (autobrr's `IsMaxDownloadsLimitEnabled`).
    pub fn is_max_downloads_limit_enabled(&self) -> bool {
        self.max_downloads > 0 && !self.max_downloads_unit.is_empty()
    }

    /// Whether the window still has room; with no count known it hasn't.
    fn check_max_downloads(&self) -> bool {
        self.downloads.as_ref().is_some_and(|d| d.period_count < self.max_downloads)
    }

    /// The size check inside [`Filter::check_filter`]: passes with no size known (and asks for a later check).
    fn check_size_filter(&self, r: &mut Release, rr: &mut RejectionReasons) -> bool {
        if r.size == 0 {
            r.additional_size_check_required = true;
            return true;
        }
        r.additional_size_check_required = false;
        match self.check_release_size(r.size, rr) {
            Ok(ok) => ok,
            Err(e) => {
                rr.add("size: ERROR", format!("error checking release size against filter: {e}"), &self.min_size);
                false
            }
        }
    }

    /// Whether `size` is inside the filter's size limits, both ends left out; a reason is noted when not
    /// (autobrr's `CheckReleaseSize`).
    pub fn check_release_size(&self, size: u64, rr: &mut RejectionReasons) -> Result<bool, String> {
        let (min, max) = self.parsed_size_limits()?;
        if let Some(min) = min {
            if size <= min {
                rr.addf("release size", "release size %[2]d bytes is smaller than filter min size %[3]d bytes", size, min);
                return Ok(false);
            }
        }
        if let Some(max) = max {
            if size >= max {
                rr.addf("release size", "release size %[2]d bytes is larger than filter max size %[3]d bytes", size, max);
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The uploader check inside [`Filter::check_filter`]; RED and OPS announce no uploader, so it is asked later.
    fn check_uploader(&self, r: &mut Release, rr: &mut RejectionReasons) {
        if r.uploader.is_empty() && (r.indexer.identifier == "redacted" || r.indexer.identifier == "ops") {
            r.additional_uploader_check_required = true;
            return;
        }
        if !self.match_uploaders.is_empty() && !contains(&r.uploader, &self.match_uploaders) {
            rr.add("match uploaders", &r.uploader, &self.match_uploaders);
        }
        if !self.except_uploaders.is_empty() && contains(&r.uploader, &self.except_uploaders) {
            rr.add("except uploaders", &r.uploader, &self.except_uploaders);
        }
    }

    /// The record label check inside [`Filter::check_filter`]; asked later on RED and OPS when none is known.
    fn check_record_label(&self, r: &mut Release, rr: &mut RejectionReasons) {
        if r.record_label.is_empty() && (r.indexer.identifier == "redacted" || r.indexer.identifier == "ops") {
            r.additional_record_label_check_required = true;
            return;
        }
        if !self.match_record_labels.is_empty() && !contains(&r.record_label, &self.match_record_labels) {
            rr.add("match record labels", &r.record_label, &self.match_record_labels);
        }
        if !self.except_record_labels.is_empty() && contains(&r.record_label, &self.except_record_labels) {
            rr.add("except record labels", &r.record_label, &self.except_record_labels);
        }
    }

    /// The later uploader check, once the site said who uploaded it (autobrr's `CheckUploader`).
    pub fn check_uploader_later(&self, uploader: &str, rr: &mut RejectionReasons) -> bool {
        if !self.match_uploaders.is_empty() && !contains(uploader, &self.match_uploaders) {
            rr.add("match uploader", uploader, &self.match_uploaders);
            return false;
        }
        if !self.except_uploaders.is_empty() && contains(uploader, &self.except_uploaders) {
            rr.add("except uploader", uploader, &self.except_uploaders);
            return false;
        }
        true
    }

    /// The later record label check, once the site said which (autobrr's `CheckRecordLabel`).
    pub fn check_record_label_later(&self, label: &str, rr: &mut RejectionReasons) -> bool {
        if !self.match_record_labels.is_empty() && !contains(label, &self.match_record_labels) {
            rr.add("match record label", label, &self.match_record_labels);
            return false;
        }
        if !self.except_record_labels.is_empty() && contains(label, &self.except_record_labels) {
            rr.add("except record label", label, &self.except_record_labels);
            return false;
        }
        true
    }

    /// Whether it is a perfect FLAC as RED and OPS define it: FLAC, lossless, and either a CD rip with a 100%
    /// log or from other media; what is missing when not (autobrr's `IsPerfectFLAC`).
    pub fn is_perfect_flac(&self, r: &Release) -> (Vec<String>, bool) {
        let mut why = Vec::new();
        if r.audio_format != "FLAC" {
            why.push(format!("wanted Format FLAC, got {}", r.audio_format));
        }
        if !contains_slice(&r.bitrate, &["Lossless".into(), "24bit Lossless".into()]) {
            why.push(format!("wanted Bitrate Lossless / 24bit Lossless, got {}", r.bitrate));
        }
        let other = ["Vinyl", "WEB", "DVD", "Soundboard", "Cassette", "SACD", "Blu-Ray", "Blu-ray", "BD", "DAT"];
        if contains_slice(&r.source, &["CD".into()]) {
            if !r.has_log {
                why.push(format!("wanted Log, got {}", r.has_log));
            }
            if r.log_score != 100 {
                why.push(format!("wanted Log Score 100, got {}", r.log_score));
            }
        } else if !contains_slice(&r.source, &other.map(String::from)) {
            why.push(format!("wanted Source CD (100% log) or Vinyl/WEB/DVD/Soundboard/Cassette/SACD/Blu-ray/DAT, got {}", r.source));
        }
        let ok = why.is_empty();
        (why, ok)
    }

    /// The size limits in bytes (`None`: no limit); the problem when one can't be read.
    pub fn parsed_size_limits(&self) -> Result<(Option<u64>, Option<u64>), String> {
        let min = parse_bytes(&self.min_size).map_err(|e| format!("could not parse filter min size: {e}"))?;
        let max = parse_bytes(&self.max_size).map_err(|e| format!("could not parse filter max size: {e}"))?;
        Ok((min, max))
    }
}

/// A size limit in bytes; `None` when empty.
fn parse_bytes(s: &str) -> Result<Option<u64>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    humanize::parse_bytes(s).map(Some)
}

/// Go's `strings.ToLower`.
fn lower(s: &str) -> String {
    s.chars().map(super::rls::to_lower).collect()
}

/// Whether a filter word is a wildcard (`*`, `?` or `|`).
fn is_wild(f: &str) -> bool {
    f.contains(['?', '|', '*'])
}

/// The regular expressions already compiled for [`match_regex`] (`None`: it could not be).
static REGEXES: LazyLock<Mutex<HashMap<String, Option<Regex>>>> = LazyLock::new(Default::default);

/// Whether `tag` matches any of the comma separated regular expressions in `filter_list`, in any case. Commas
/// inside quotes or brackets don't separate; a list or pattern that can't be read matches nothing.
pub fn match_regex(tag: &str, filter_list: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    let Some(filters) = split_enclosed(filter_list) else { return false };
    for f in filters {
        if f.is_empty() {
            continue;
        }
        let mut cache = REGEXES.lock().unwrap_or_else(|e| e.into_inner());
        let re = cache.entry(f.clone()).or_insert_with(|| goregex::try_compile(&format!("(?i)(?:{f})")));
        match re {
            Some(re) if re.is_match(tag) => return true,
            Some(_) => {}
            None => return false,
        }
    }
    false
}

/// `s` split at commas outside `"…"`, `(…)`, `{…}` and `[…]` (go-andiamo/splitter as autobrr sets it up);
/// `None` when a quote or bracket is left open.
fn split_enclosed(s: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut stack: Vec<char> = Vec::new();
    for c in s.chars() {
        match stack.last() {
            Some('"') => {
                if c == '"' {
                    stack.pop();
                }
            }
            top => match c {
                '"' | '(' | '{' | '[' => stack.push(c),
                ')' | '}' | ']' => {
                    let open = match c {
                        ')' => '(',
                        '}' => '{',
                        _ => '[',
                    };
                    if top == Some(&open) {
                        stack.pop();
                    }
                }
                ',' if top.is_none() => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            },
        }
        cur.push(c);
    }
    if !stack.is_empty() {
        return None;
    }
    parts.push(cur);
    Some(parts)
}

/// Whether `value` is one of the numbers or `min-max` ranges in `filter_list` (`1,2,3-20`); a word that isn't a
/// number makes it fail (autobrr's `containsIntStrings`).
pub fn contains_int_strings(value: i64, filter_list: &str) -> bool {
    for f in filter_list.split(',') {
        let f = f.replace('%', "");
        let f = f.trim();
        if f.contains('-') {
            let mm: Vec<&str> = f.split('-').collect();
            if mm.len() == 2 {
                let (Ok(min), Ok(max)) = (mm[0].parse::<i32>(), mm[1].parse::<i32>()) else { return false };
                if min > max {
                    return false;
                }
                if value >= i64::from(min) && value <= i64::from(max) {
                    return true;
                }
            }
            continue;
        }
        match f.parse::<i32>() {
            Ok(n) if i64::from(n) == value => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Whether the freeleech percentage is one of the numbers or ranges in `filter_percent`; as in autobrr, a range
/// that doesn't hold ends the search (autobrr's `checkFreeleechPercent`).
pub fn check_freeleech_percent(percent: i64, filter_percent: &str) -> bool {
    for f in filter_percent.split(',') {
        let f = f.replace('%', "");
        let f = f.trim();
        if f.contains('-') {
            let mm: Vec<&str> = f.split('-').collect();
            if mm.len() == 2 {
                let (Ok(min), Ok(max)) = (mm[0].parse::<i32>(), mm[1].parse::<i32>()) else { return false };
                if min > max {
                    return false;
                }
                if percent >= i64::from(min) && percent <= i64::from(max) {
                    return true;
                }
            }
        }
        match f.parse::<i32>() {
            Ok(n) if i64::from(n) == percent => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Whether `tag` is one of the comma separated words or wildcards in `filter` (autobrr's `contains`).
pub fn contains(tag: &str, filter: &str) -> bool {
    contains_match(&[tag.to_string()], &split(filter))
}

/// Whether `tag` contains one of the comma separated words or matches one of the wildcards (autobrr's
/// `containsFuzzy`).
pub fn contains_fuzzy(tag: &str, filter: &str) -> bool {
    contains_match_fuzzy(&[tag.to_string()], &split(filter))
}

/// Whether `tag` is one of `filters` (words or wildcards) (autobrr's `containsSlice`).
pub fn contains_slice(tag: &str, filters: &[String]) -> bool {
    contains_match(&[tag.to_string()], filters)
}

/// Whether any of `tags` is one of the comma separated words or wildcards (autobrr's `containsAny`).
pub fn contains_any(tags: &[String], filter: &str) -> bool {
    contains_match(tags, &split(filter))
}

/// Whether every comma separated word or wildcard is one of `tags` (autobrr's `containsAll`).
pub fn contains_all(tags: &[String], filter: &str) -> bool {
    contains_all_match(tags, &split(filter))
}

/// Whether any of `tags` is exactly one of `filters`, in any case (autobrr's `sliceContainsSlice`).
pub fn slice_contains_slice(tags: &[String], filters: &[String]) -> bool {
    contains_match_basic(tags, filters)
}

/// A comma separated list, as Go's `strings.Split` (an empty text is one empty word).
fn split(s: &str) -> Vec<String> {
    s.split(',').map(String::from).collect()
}

/// Whether any of `tags` contains one of the plain `filters` or matches one of the wildcards (autobrr's
/// `containsMatchFuzzy`).
pub fn contains_match_fuzzy(tags: &[String], filters: &[String]) -> bool {
    contains_match_with(tags, filters, |tag, f| tag.contains(f))
}

/// Whether any of `tags` is one of the plain `filters` or matches one of the wildcards (autobrr's
/// `containsMatch`).
pub fn contains_match(tags: &[String], filters: &[String]) -> bool {
    contains_match_with(tags, filters, |tag, f| tag == f)
}

/// The two above: `plain` compares a lower case tag to a lower case plain filter word.
fn contains_match_with(tags: &[String], filters: &[String], plain: impl Fn(&str, &str) -> bool) -> bool {
    for tag in tags {
        if tag.is_empty() {
            continue;
        }
        let tag = lower(tag);
        let mut advanced = Vec::new();
        for f in filters {
            if f.is_empty() {
                continue;
            }
            let f = lower(f.trim());
            if is_wild(&f) {
                advanced.push(f);
            } else if plain(&tag, &f) {
                return true;
            }
        }
        if wildcard::match_slice(&advanced, &tag) {
            return true;
        }
    }
    false
}

/// Whether every one of `filters` is one of `tags`, a wildcard matching one (autobrr's `containsAllMatch`).
pub fn contains_all_match(tags: &[String], filters: &[String]) -> bool {
    for f in filters {
        if f.is_empty() {
            continue;
        }
        let f = lower(f.trim());
        let wild = is_wild(&f);
        let found = tags.iter().filter(|t| !t.is_empty()).any(|t| {
            let t = lower(t);
            t == f || (wild && wildcard::is_match(&f, &t))
        });
        if !found {
            return false;
        }
    }
    true
}

/// Whether any of `tags` is exactly one of `filters`, in any case, no wildcards (autobrr's
/// `containsMatchBasic`).
pub fn contains_match_basic(tags: &[String], filters: &[String]) -> bool {
    tags.iter().filter(|t| !t.is_empty()).map(|t| lower(t)).any(|t| filters.iter().filter(|f| !f.is_empty()).any(|f| lower(f.trim()) == t))
}

/// Whether any of `tags` matches one of `filters` (autobrr's `containsAnySlice`, same as [`contains_match`]).
pub fn contains_any_slice(tags: &[String], filters: &[String]) -> bool {
    contains_match(tags, filters)
}

/// Whether `tag` is exactly one of `filters`, case and all (autobrr's `basicContainsSlice`).
pub fn basic_contains_slice(tag: &str, filters: &[String]) -> bool {
    super::release::basic_contains_slice(tag, filters)
}

/// Whether the release's HDR formats match one of the filter's: `DV HDR` needs both, and a release with two
/// formats matches a single one only when both are it (autobrr's `matchHDR`).
pub fn match_hdr(values: &[String], filters: &[String]) -> bool {
    for f in filters {
        if f.is_empty() {
            continue;
        }
        let f = lower(f.trim());
        let parts: Vec<&str> = f.split(' ').collect();
        if parts.len() == 2 {
            let mut matched = 0;
            for part in &parts {
                for tag in values {
                    if tag.is_empty() {
                        continue;
                    }
                    if lower(tag) == *part {
                        matched += 1;
                    }
                    if parts.len() == matched {
                        return true;
                    }
                }
            }
        } else if values.len() == 2 {
            let matches = values.iter().filter(|t| !t.is_empty() && lower(t) == f).count();
            if matches == values.len() {
                return true;
            }
        } else if values.iter().any(|t| !t.is_empty() && lower(t) == f) {
            return true;
        }
    }
    false
}
