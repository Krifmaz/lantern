//! Filters: which releases to grab. A filter lists what a release must have (words, resolution, group, size,
//! freeleech, year, tags …) and what it must not; [`Filter::check`] says whether a release passes and, when it
//! doesn't, why, in words.
//!
//! Lists are comma separated. A word with `*` (any text) or `?` (one letter) must match the whole value; a word
//! without them only needs to be part of it. Case never matters.

use super::name::Name;
use super::Found;
use serde::{Deserialize, Serialize};

/// One filter, as saved and as the API takes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Its id (0 before it is saved).
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// Used at all.
    pub enabled: bool,
    /// Higher is checked first; the first filter a release passes takes it.
    pub priority: i64,
    /// Only releases from these sources (ids); empty: every source.
    pub sources: Vec<i64>,
    /// The release's name must match one of these (`*Show.Name*`).
    pub match_releases: String,
    /// The release's name must match none of these.
    pub except_releases: String,
    /// Read `match_releases` and `except_releases` as regular expressions instead.
    pub use_regex: bool,
    /// Resolutions, any of (`1080p`, `2160p`); empty: any.
    pub resolutions: Vec<String>,
    /// Sources, any of (`WEB-DL`, `BluRay`, `Remux`); empty: any.
    pub release_sources: Vec<String>,
    /// Codecs, any of (`H.264`, `H.265`); empty: any.
    pub codecs: Vec<String>,
    /// HDR kinds, at least one of (`DV`, `HDR10`); empty: any.
    pub hdr: Vec<String>,
    /// Release groups, one of.
    pub match_groups: String,
    /// Release groups, none of.
    pub except_groups: String,
    /// Years, as a list of years and ranges (`2019-2024, 2026`).
    pub years: String,
    /// Seasons (`1-3, 5`); TV only.
    pub seasons: String,
    /// Episodes (`1-10`); TV only.
    pub episodes: String,
    /// Smallest size (`700 MB`, `1.5 GB`).
    pub min_size: String,
    /// Largest size (`30 GB`).
    pub max_size: String,
    /// Only freeleech releases.
    pub freeleech: bool,
    /// Freeleech percentages (`50-100`).
    pub freeleech_percent: String,
    /// Categories as the site names them, one of (`TV*`, `Movies/HD`).
    pub match_categories: String,
    /// Categories, none of.
    pub except_categories: String,
    /// Tags, at least one of (`hdr, action`).
    pub tags: String,
    /// Tags, none of.
    pub except_tags: String,
    /// Uploaders, one of.
    pub match_uploaders: String,
    /// Uploaders, none of.
    pub except_uploaders: String,
    /// The most grabs in each [`Filter::max_downloads_per`] (0: no limit).
    pub max_downloads: i64,
    /// `hour`, `day`, `week`, `month` or `ever`.
    pub max_downloads_per: String,
    /// Seconds to wait before pushing (0: at once, best for racing).
    pub delay: u64,
    /// Skip a release with the same name this filter already grabbed from another source.
    pub skip_duplicates: bool,
    /// Where a passing release goes.
    pub actions: Vec<Action>,
}

/// Where a release a filter takes goes.
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

impl Filter {
    /// A new filter with the race-friendly defaults: on, pushed at once, duplicates skipped.
    pub fn new(name: &str) -> Filter {
        Filter { name: name.into(), enabled: true, skip_duplicates: true, max_downloads_per: "day".into(), ..Filter::default() }
    }

    /// Whether the size is needed to decide (a size limit is set).
    pub fn wants_size(&self) -> bool {
        !self.min_size.trim().is_empty() || !self.max_size.trim().is_empty()
    }

    /// Whether the filter's own settings can be read; the problem in words when not.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Give the filter a name".into());
        }
        for (label, v) in [("smallest size", &self.min_size), ("largest size", &self.max_size)] {
            if !v.trim().is_empty() && parse_size(v).is_none() {
                return Err(format!("The {label} \"{v}\" can't be read. Write it like 700 MB or 1.5 GB"));
            }
        }
        for (label, v) in [
            ("years", &self.years),
            ("seasons", &self.seasons),
            ("episodes", &self.episodes),
            ("freeleech percentages", &self.freeleech_percent),
        ] {
            if !v.trim().is_empty() && ranges(v).is_none() {
                return Err(format!("The {label} \"{v}\" can't be read. Write numbers and ranges like 2019-2024, 2026"));
            }
        }
        if self.use_regex {
            for v in [&self.match_releases, &self.except_releases] {
                for w in list(v) {
                    regex::Regex::new(&format!("(?i){w}")).map_err(|_| format!("\"{w}\" is not a pattern Lantern can read"))?;
                }
            }
        }
        if !["", "hour", "day", "week", "month", "ever"].contains(&self.max_downloads_per.as_str()) {
            return Err("The download limit counts per hour, day, week, month or ever".into());
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

    /// Whether `r` (named `n`) passes; the reasons it doesn't, in words, when not. A size limit with no size
    /// known passes here: the caller reads the size from the torrent and asks again.
    pub fn check(&self, r: &Found, n: &Name) -> Result<(), Vec<String>> {
        let mut why = Vec::new();
        if !self.sources.is_empty() && !self.sources.contains(&r.source_id) {
            why.push(format!("it comes from {}, which this filter doesn't watch", r.source_name));
        }
        if !self.match_releases.trim().is_empty() && !self.name_matches(&self.match_releases, &r.title) {
            why.push("its name doesn't match the names to grab".into());
        }
        if !self.except_releases.trim().is_empty() && self.name_matches(&self.except_releases, &r.title) {
            why.push("its name matches a name to skip".into());
        }
        one_of(&mut why, "resolution", &n.resolution, &self.resolutions);
        one_of(&mut why, "source", &n.source, &self.release_sources);
        one_of(&mut why, "codec", &n.codec, &self.codecs);
        if !self.hdr.is_empty() && !n.hdr.iter().any(|h| self.hdr.iter().any(|w| w.eq_ignore_ascii_case(h))) {
            why.push(if n.hdr.is_empty() {
                "it has no HDR".into()
            } else {
                format!("its HDR ({}) isn't one of the ones to grab", n.hdr.join(", "))
            });
        }
        if !self.match_groups.trim().is_empty() && !any_matches(&self.match_groups, &n.group, true) {
            why.push(if n.group.is_empty() {
                "its group can't be read from the name".into()
            } else {
                format!("the group {} isn't one to grab", n.group)
            });
        }
        if !self.except_groups.trim().is_empty() && any_matches(&self.except_groups, &n.group, true) {
            why.push(format!("the group {} is one to skip", n.group));
        }
        let year = n.year.or(r.year);
        in_ranges(&mut why, "year", year, &self.years);
        in_ranges(&mut why, "season", n.season, &self.seasons);
        in_ranges(&mut why, "episode", n.episode, &self.episodes);
        if let Some(size) = r.size {
            if let Some(min) = parse_size(&self.min_size) {
                if size < min {
                    why.push(format!("it is {}, smaller than {}", size_text(size), self.min_size.trim()));
                }
            }
            if let Some(max) = parse_size(&self.max_size) {
                if size > max {
                    why.push(format!("it is {}, larger than {}", size_text(size), self.max_size.trim()));
                }
            }
        }
        if self.freeleech && !r.freeleech {
            why.push("it isn't freeleech".into());
        }
        in_ranges(&mut why, "freeleech percentage", r.freeleech_percent.or(r.freeleech.then_some(100)), &self.freeleech_percent);
        if !self.match_categories.trim().is_empty() && !any_matches(&self.match_categories, &r.category, false) {
            why.push(format!("its category ({}) isn't one to grab", if r.category.is_empty() { "none" } else { &r.category }));
        }
        if !self.except_categories.trim().is_empty() && any_matches(&self.except_categories, &r.category, false) {
            why.push(format!("its category ({}) is one to skip", r.category));
        }
        if !self.tags.trim().is_empty() && !r.tags.iter().any(|t| any_matches(&self.tags, t, true)) {
            why.push("it has none of the tags to grab".into());
        }
        if !self.except_tags.trim().is_empty() && r.tags.iter().any(|t| any_matches(&self.except_tags, t, true)) {
            why.push("it has a tag to skip".into());
        }
        if !self.match_uploaders.trim().is_empty() && !any_matches(&self.match_uploaders, &r.uploader, true) {
            why.push("its uploader isn't one to grab".into());
        }
        if !self.except_uploaders.trim().is_empty() && any_matches(&self.except_uploaders, &r.uploader, true) {
            why.push("its uploader is one to skip".into());
        }
        if why.is_empty() {
            Ok(())
        } else {
            Err(why)
        }
    }

    /// Whether `title` matches one of `words` (wildcards or, with `use_regex`, patterns).
    fn name_matches(&self, words: &str, title: &str) -> bool {
        if self.use_regex {
            list(words).iter().any(|w| regex::Regex::new(&format!("(?i){w}")).is_ok_and(|re| re.is_match(title)))
        } else {
            any_matches(words, title, false)
        }
    }

    /// Seconds a window of [`Filter::max_downloads_per`] lasts (`None`: ever).
    pub fn window_secs(&self) -> Option<f64> {
        match self.max_downloads_per.as_str() {
            "hour" => Some(3600.0),
            "week" => Some(7.0 * 86400.0),
            "month" => Some(30.0 * 86400.0),
            "ever" => None,
            _ => Some(86400.0),
        }
    }
}

/// Adds a reason when `value` (called `what`) isn't one of `wanted`.
fn one_of(why: &mut Vec<String>, what: &str, value: &str, wanted: &[String]) {
    if wanted.is_empty() || wanted.iter().any(|w| w.eq_ignore_ascii_case(value)) {
        return;
    }
    why.push(if value.is_empty() {
        format!("its {what} can't be read from the name")
    } else {
        format!("its {what} ({value}) isn't one to grab")
    });
}

/// Adds a reason when `value` (called `what`) isn't in the ranges `spec` (nothing set: no reason).
fn in_ranges(why: &mut Vec<String>, what: &str, value: Option<u32>, spec: &str) {
    let Some(r) = ranges(spec) else { return };
    if r.is_empty() {
        return;
    }
    match value {
        Some(v) if r.iter().any(|(a, b)| (*a..=*b).contains(&v)) => {}
        Some(v) => why.push(format!("its {what} ({v}) isn't one to grab")),
        None => why.push(format!("its {what} can't be read")),
    }
}

/// The comma-separated words of `s`, trimmed, without empty ones.
pub fn list(s: &str) -> Vec<String> {
    s.split(',').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect()
}

/// Whether `value` matches one of the comma-separated `words`: a word with `*` or `?` as a whole-value wildcard,
/// else as part of it (`exact`: as the whole value).
pub fn any_matches(words: &str, value: &str, exact: bool) -> bool {
    let value = value.to_lowercase();
    list(words).iter().any(|w| {
        let w = w.to_lowercase();
        if w.contains('*') || w.contains('?') {
            wildcard(&w, &value)
        } else if exact {
            w == value
        } else {
            value.contains(&w)
        }
    })
}

/// Whether `text` is matched as a whole by `pattern`, where `*` is any text and `?` one character.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// `2019-2024, 2026` as ranges; `None` when it can't be read; empty for nothing set.
pub fn ranges(s: &str) -> Option<Vec<(u32, u32)>> {
    list(s)
        .iter()
        .map(|w| match w.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?);
                Some((a.min(b), a.max(b)))
            }
            None => w.parse::<u32>().ok().map(|v| (v, v)),
        })
        .collect()
}

/// `700 MB`, `1.5GB`, `30 GiB`, `2048` (bytes) as bytes; `None` when it can't be read. `KB`/`MB`/`GB`/`TB` count
/// in thousands, `KiB`/`MiB`/`GiB`/`TiB` in 1024s.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim().to_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024f64.powi(2),
        "gib" => 1024f64.powi(3),
        "tib" => 1024f64.powi(4),
        _ => return None,
    };
    Some((num * mult) as u64)
}

/// Bytes in words (`1.5 GB`).
pub fn size_text(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{v:.0} B")
    } else {
        format!("{v:.1} {}", units[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(title: &str) -> Found {
        Found { title: title.into(), source_id: 1, source_name: "Tracker".into(), ..Found::default() }
    }

    #[test]
    fn wildcards_and_words() {
        assert!(wildcard("*show.name*", "show.name.s01e02.1080p"));
        assert!(wildcard("show?name*", "show.name.s01"));
        assert!(!wildcard("*movie*", "show.name"));
        assert!(any_matches("Show Name, *Other*", "an.other.thing", false));
        assert!(any_matches("GROUP", "group", true));
        assert!(!any_matches("GRP", "GROUP", true));
    }

    #[test]
    fn sizes_and_ranges() {
        assert_eq!(parse_size("700 MB"), Some(700_000_000));
        assert_eq!(parse_size("1.5GB"), Some(1_500_000_000));
        assert_eq!(parse_size("1 GiB"), Some(1 << 30));
        assert_eq!(parse_size("lots"), None);
        assert_eq!(ranges("2019-2024, 2026"), Some(vec![(2019, 2024), (2026, 2026)]));
        assert_eq!(ranges("x"), None);
        assert_eq!(size_text(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn a_filter_says_why_it_skips() {
        let f = Filter {
            resolutions: vec!["1080p".into()],
            release_sources: vec!["WEB-DL".into()],
            except_groups: "BAD".into(),
            min_size: "1 GB".into(),
            freeleech: true,
            years: "2020-2026".into(),
            ..Filter::new("HD")
        };
        let mut r = found("Movie.2023.1080p.WEB-DL.H.264-GOOD");
        r.size = Some(4_000_000_000);
        r.freeleech = true;
        assert_eq!(f.check(&r, &Name::parse(&r.title)), Ok(()));

        let mut r = found("Movie.2010.720p.BluRay.x264-BAD");
        r.size = Some(500_000_000);
        let why = f.check(&r, &Name::parse(&r.title)).unwrap_err();
        assert!(why.contains(&"its resolution (720p) isn't one to grab".to_string()), "{why:?}");
        assert!(why.contains(&"its source (BluRay) isn't one to grab".to_string()));
        assert!(why.contains(&"the group BAD is one to skip".to_string()));
        assert!(why.contains(&"it is 500.0 MB, smaller than 1 GB".to_string()));
        assert!(why.contains(&"it isn't freeleech".to_string()));
        assert!(why.contains(&"its year (2010) isn't one to grab".to_string()));
    }

    #[test]
    fn names_categories_tags_and_sources() {
        let f = Filter {
            sources: vec![2],
            match_releases: "*Show*".into(),
            except_releases: "*German*".into(),
            match_categories: "TV*".into(),
            tags: "hdr".into(),
            ..Filter::new("TV")
        };
        let mut r = found("Show.S01E01.German.1080p.WEB.h264-X");
        r.category = "Movies".into();
        let why = f.check(&r, &Name::parse(&r.title)).unwrap_err();
        assert_eq!(why.len(), 4, "{why:?}");
        let mut r = found("Show.S01E01.1080p.WEB.h264-X");
        r.source_id = 2;
        r.category = "TV/HD".into();
        r.tags = vec!["HDR".into(), "comedy".into()];
        assert_eq!(f.check(&r, &Name::parse(&r.title)), Ok(()));
        let re = Filter { use_regex: true, match_releases: r"^show\.s01".into(), ..Filter::new("re") };
        assert_eq!(re.check(&r, &Name::parse(&r.title)), Ok(()));
    }

    #[test]
    fn checks_its_own_settings() {
        assert!(Filter::new("ok").validate().is_ok());
        assert!(Filter { min_size: "big".into(), ..Filter::new("x") }.validate().unwrap_err().contains("700 MB"));
        assert!(Filter::new(" ").validate().is_err());
        let a = Action { kind: "arr".into(), app: "sonarr".into(), url: "sonarr:8989".into(), ..Action::default() };
        assert!(Filter { actions: vec![a], ..Filter::new("x") }.validate().unwrap_err().contains("http://"));
    }
}
