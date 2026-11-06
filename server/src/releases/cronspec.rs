// Ported from robfig/cron v3.0.1 (parser.go, spec.go), MIT, as autobrr v1.90.0 uses it.
//! Five-field cron lines (`minute hour day-of-month month day-of-week`), read and run as robfig/cron does:
//! `*`, `?`, `*/n`, `a-b`, `a-b/n`, `a/n`, lists with commas, and month and day names. When both the
//! day of the month and the day of the week are limited, a day matching either one counts. Times are local, as
//! autobrr's scheduler runs them.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};

/// The bit that marks a field written as `*` or `?` (robfig's `starBit`).
const STAR: u64 = 1 << 63;

/// The bounds and names of one field (robfig's `bounds`).
struct Bounds {
    /// The smallest value.
    min: u32,
    /// The largest value.
    max: u32,
    /// The names it takes, lower case, with their values.
    names: &'static [(&'static str, u32)],
}

/// Minutes, 0 to 59.
const MINUTES: Bounds = Bounds { min: 0, max: 59, names: &[] };
/// Hours, 0 to 23.
const HOURS: Bounds = Bounds { min: 0, max: 23, names: &[] };
/// Days of the month, 1 to 31.
const DOM: Bounds = Bounds { min: 1, max: 31, names: &[] };
/// Months, 1 to 12 or their names.
const MONTHS: Bounds = Bounds {
    min: 1,
    max: 12,
    names: &[
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("apr", 4),
        ("may", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("dec", 12),
    ],
};
/// Days of the week, 0 (Sunday) to 6 or their names.
const DOW: Bounds = Bounds { min: 0, max: 6, names: &[("sun", 0), ("mon", 1), ("tue", 2), ("wed", 3), ("thu", 4), ("fri", 5), ("sat", 6)] };

/// A parsed cron line: one bit per allowed value of each field (robfig's `SpecSchedule`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CronSpec {
    /// The allowed minutes.
    minute: u64,
    /// The allowed hours.
    hour: u64,
    /// The allowed days of the month.
    dom: u64,
    /// The allowed months.
    month: u64,
    /// The allowed days of the week.
    dow: u64,
}

/// A whole number from a field; robfig's words when it is not one (`mustParseInt`).
fn parse_int(expr: &str) -> Result<u32, String> {
    let n: i64 = expr.parse().map_err(|_| format!("failed to parse int from {expr}: strconv.Atoi: parsing {expr:?}: invalid syntax"))?;
    if n < 0 {
        return Err(format!("negative number ({n}) not allowed: {expr}"));
    }
    u32::try_from(n).map_err(|_| format!("failed to parse int from {expr}: strconv.Atoi: parsing {expr:?}: value out of range"))
}

/// A number or, when the field has names, a name (robfig's `parseIntOrName`).
fn int_or_name(expr: &str, b: &Bounds) -> Result<u32, String> {
    let lower = expr.to_lowercase();
    if let Some((_, v)) = b.names.iter().find(|(n, _)| *n == lower) {
        return Ok(*v);
    }
    parse_int(expr)
}

/// The bits of one range such as `1-5/2` (robfig's `getRange`).
fn range(expr: &str, b: &Bounds) -> Result<u64, String> {
    let range_and_step: Vec<&str> = expr.split('/').collect();
    let low_and_high: Vec<&str> = range_and_step[0].split('-').collect();
    let single = low_and_high.len() == 1;
    let (start, mut end, mut extra);
    if low_and_high[0] == "*" || low_and_high[0] == "?" {
        start = b.min;
        end = b.max;
        extra = STAR;
    } else {
        start = int_or_name(low_and_high[0], b)?;
        end = match low_and_high.len() {
            1 => start,
            2 => int_or_name(low_and_high[1], b)?,
            _ => return Err(format!("too many hyphens: {expr}")),
        };
        extra = 0;
    }
    let step = match range_and_step.len() {
        1 => 1,
        2 => {
            let step = parse_int(range_and_step[1])?;
            if single {
                end = b.max;
            }
            if step > 1 {
                extra = 0;
            }
            step
        }
        _ => return Err(format!("too many slashes: {expr}")),
    };
    if start < b.min {
        return Err(format!("beginning of range ({start}) below minimum ({}): {expr}", b.min));
    }
    if end > b.max {
        return Err(format!("end of range ({end}) above maximum ({}): {expr}", b.max));
    }
    if start > end {
        return Err(format!("beginning of range ({start}) beyond end of range ({end}): {expr}"));
    }
    if step == 0 {
        return Err(format!("step of range should be a positive number: {expr}"));
    }
    let mut bits = 0u64;
    let mut i = start;
    while i <= end {
        bits |= 1 << i;
        i += step;
    }
    Ok(bits | extra)
}

/// The bits of one field, ranges joined by commas (robfig's `getField`).
fn field(f: &str, b: &Bounds) -> Result<u64, String> {
    let mut bits = 0;
    for r in f.split(',') {
        bits |= range(r, b)?;
    }
    Ok(bits)
}

impl CronSpec {
    /// Reads a five-field cron line (robfig's `Parser.Parse` with minute, hour, day of month, month and day of
    /// week); the problem in robfig's words when it can't.
    pub fn parse(spec: &str) -> Result<CronSpec, String> {
        if spec.is_empty() {
            return Err("empty spec string".into());
        }
        let mut spec = spec;
        if spec.starts_with("TZ=") || spec.starts_with("CRON_TZ=") {
            // Lantern runs every line in local time; the zone name is read past
            let i = spec.find(' ').ok_or_else(|| format!("provided bad location {spec}: unknown time zone {spec}"))?;
            spec = spec[i..].trim();
        }
        if spec.starts_with('@') {
            return Err(format!("parser does not accept descriptors: {spec}"));
        }
        let fields: Vec<&str> = spec.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!("expected exactly 5 fields, found {}: [{}]", fields.len(), fields.join(" ")));
        }
        Ok(CronSpec {
            minute: field(fields[0], &MINUTES)?,
            hour: field(fields[1], &HOURS)?,
            dom: field(fields[2], &DOM)?,
            month: field(fields[3], &MONTHS)?,
            dow: field(fields[4], &DOW)?,
        })
    }

    /// Whether a day matches: both day fields when either is `*`, else either one (robfig's `dayMatches`).
    fn day_matches(&self, d: NaiveDate) -> bool {
        let dom = self.dom & (1 << d.day()) > 0;
        let dow = self.dow & (1 << d.weekday().num_days_from_sunday()) > 0;
        if self.dom & STAR > 0 || self.dow & STAR > 0 {
            dom && dow
        } else {
            dom || dow
        }
    }

    /// The first local minute after `t` the line allows, looking at most five years ahead (robfig's `Next`).
    fn next_local(&self, t: DateTime<Local>) -> Option<DateTime<Local>> {
        let start = t.naive_local().with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let mut day = start.date();
        let until = day + Duration::days(5 * 366);
        while day <= until {
            if self.month & (1 << day.month()) > 0 && self.day_matches(day) {
                let from = if day == start.date() { start.time() } else { NaiveTime::MIN };
                for h in from.hour()..24 {
                    if self.hour & (1 << h) == 0 {
                        continue;
                    }
                    let first = if h == from.hour() { from.minute() } else { 0 };
                    for m in first..60 {
                        if self.minute & (1 << m) == 0 {
                            continue;
                        }
                        let at = day.and_hms_opt(h, m, 0)?;
                        // a minute that a clock change skips is passed over
                        if let Some(local) = Local.from_local_datetime(&at).earliest() {
                            if local > t {
                                return Some(local);
                            }
                        }
                    }
                }
            }
            day = day.succ_opt()?;
        }
        None
    }
}

impl super::scheduler::Schedule for CronSpec {
    fn next(&self, t: DateTime<Utc>) -> DateTime<Utc> {
        // robfig answers the zero time for a line that never comes; Lantern waits a year and looks again
        self.next_local(t.with_timezone(&Local)).map(|l| l.with_timezone(&Utc)).unwrap_or(t + Duration::days(365))
    }
}

#[cfg(test)]
mod tests {
    //! robfig/cron's parser_test.go and spec_test.go, the five-field cases.
    use super::*;
    use chrono::NaiveDateTime;

    fn at(s: &str) -> DateTime<Local> {
        Local.from_local_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()).earliest().unwrap()
    }

    #[test]
    fn reads_ranges() {
        let all = |b: &Bounds| (b.min..=b.max).fold(0u64, |a, i| a | 1 << i);
        assert_eq!(range("5", &MINUTES), Ok(1 << 5));
        assert_eq!(range("0", &MINUTES), Ok(1));
        assert_eq!(range("5-5", &MINUTES), Ok(1 << 5));
        assert_eq!(range("5-6", &MINUTES), Ok(1 << 5 | 1 << 6));
        assert_eq!(range("5-7", &MINUTES), Ok(1 << 5 | 1 << 6 | 1 << 7));
        assert_eq!(range("5-6/2", &MINUTES), Ok(1 << 5));
        assert_eq!(range("5-7/2", &MINUTES), Ok(1 << 5 | 1 << 7));
        assert_eq!(range("5-7/1", &MINUTES), Ok(1 << 5 | 1 << 6 | 1 << 7));
        assert_eq!(range("*", &MINUTES), Ok(all(&MINUTES) | STAR));
        assert_eq!(range("*/2", &MINUTES), Ok((0..60).step_by(2).fold(0u64, |a, i| a | 1 << i)));
        assert_eq!(range("5--5", &MINUTES), Err("too many hyphens: 5--5".into()));
        assert_eq!(range("jan-x", &MONTHS), Err("failed to parse int from x: strconv.Atoi: parsing \"x\": invalid syntax".into()));
        assert_eq!(range("2-x", &MINUTES).unwrap_err(), "failed to parse int from x: strconv.Atoi: parsing \"x\": invalid syntax");
        assert_eq!(range("*/-12", &MINUTES), Err("negative number (-12) not allowed: -12".into()));
        assert_eq!(range("*//2", &MINUTES), Err("too many slashes: *//2".into()));
        assert_eq!(range("1", &DOM), Ok(1 << 1));
        assert_eq!(range("0", &DOM), Err("beginning of range (0) below minimum (1): 0".into()));
        assert_eq!(range("61", &MINUTES), Err("end of range (61) above maximum (59): 61".into()));
        assert_eq!(range("5-3", &MINUTES), Err("beginning of range (5) beyond end of range (3): 5-3".into()));
        assert_eq!(range("*/0", &MINUTES), Err("step of range should be a positive number: */0".into()));
    }

    #[test]
    fn reads_fields() {
        assert_eq!(field("5", &MINUTES), Ok(1 << 5));
        assert_eq!(field("5,6", &MINUTES), Ok(1 << 5 | 1 << 6));
        assert_eq!(field("5,6,7", &MINUTES), Ok(1 << 5 | 1 << 6 | 1 << 7));
        assert_eq!(field("1,5-7/2,3", &MINUTES), Ok(1 << 1 | 1 << 5 | 1 << 7 | 1 << 3));
        assert_eq!(field("jan,feb", &MONTHS), Ok(1 << 1 | 1 << 2));
        assert_eq!(field("MON-fri", &DOW), Ok((1..=5).fold(0u64, |a, i| a | 1 << i)));
    }

    #[test]
    fn reads_lines() {
        assert!(CronSpec::parse("0 3 * * *").is_ok());
        assert!(CronSpec::parse("*/15 * * * mon-fri").is_ok());
        assert_eq!(CronSpec::parse(""), Err("empty spec string".into()));
        assert_eq!(CronSpec::parse("* * * *"), Err("expected exactly 5 fields, found 4: [* * * *]".into()));
        assert_eq!(CronSpec::parse("0 * * * * *"), Err("expected exactly 5 fields, found 6: [0 * * * * *]".into()));
        assert_eq!(CronSpec::parse("@daily"), Err("parser does not accept descriptors: @daily".into()));
        assert_eq!(CronSpec::parse("60 * * * *"), Err("end of range (60) above maximum (59): 60".into()));
    }

    #[test]
    fn runs_next() {
        let next =
            |spec: &str, from: &str| CronSpec::parse(spec).unwrap().next_local(at(from)).map(|t| t.format("%Y-%m-%d %H:%M %a").to_string());
        // simple cases
        assert_eq!(next("* * * * *", "2012-07-09 14:45").as_deref(), Some("2012-07-09 14:46 Mon"));
        assert_eq!(next("0 * * * *", "2012-07-09 14:45").as_deref(), Some("2012-07-09 15:00 Mon"));
        assert_eq!(next("0 3 * * *", "2012-07-09 14:45").as_deref(), Some("2012-07-10 03:00 Tue"));
        // wrap around hours, days, months and years
        assert_eq!(next("15 10-11 * * *", "2012-07-09 23:35").as_deref(), Some("2012-07-10 10:15 Tue"));
        assert_eq!(next("0 0 * jul *", "2012-07-31 23:35").as_deref(), Some("2013-07-01 00:00 Mon"));
        assert_eq!(next("0 0 1 jan *", "2012-12-31 23:59").as_deref(), Some("2013-01-01 00:00 Tue"));
        // day of week only
        assert_eq!(next("0 0 * * mon", "2012-07-09 14:45").as_deref(), Some("2012-07-16 00:00 Mon"));
        // either day field when both are limited
        assert_eq!(next("0 0 1 * mon", "2012-07-09 14:45").as_deref(), Some("2012-07-16 00:00 Mon"));
        assert_eq!(next("0 0 10 * mon", "2012-07-09 14:45").as_deref(), Some("2012-07-10 00:00 Tue"));
        // a stepped star is no star, so either day field counts
        assert_eq!(next("0 0 */2 * mon", "2012-07-09 14:45").as_deref(), Some("2012-07-11 00:00 Wed"));
        // leap day
        assert_eq!(next("0 0 29 2 *", "2012-03-01 00:00").as_deref(), Some("2016-02-29 00:00 Mon"));
        // never
        assert_eq!(next("0 0 30 2 *", "2012-03-01 00:00"), None);
    }
}
