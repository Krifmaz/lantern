//! The filters a definition runs on a field's text (`filters: [{name: re_replace, args: [...]}]`), and the
//! parsers for the values they end in: dates, sizes and numbers.

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_yaml::Value as Yaml;

/// One filter of a definition: its name and arguments (text, a number or a list).
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct Filter {
    /// `re_replace`, `replace`, `dateparse` …
    pub name: String,
    /// The arguments, as the definition writes them.
    #[serde(default)]
    pub args: Option<Yaml>,
}

impl Filter {
    /// Argument `i` as text (a single argument is argument 0).
    pub fn arg(&self, i: usize) -> String {
        match &self.args {
            Some(Yaml::Sequence(s)) => s.get(i).map(yaml_text).unwrap_or_default(),
            Some(v) if i == 0 => yaml_text(v),
            _ => String::new(),
        }
    }
}

/// A YAML scalar as text.
pub fn yaml_text(v: &Yaml) -> String {
    match v {
        Yaml::String(s) => s.clone(),
        Yaml::Number(n) => n.to_string(),
        Yaml::Bool(b) => b.to_string(),
        Yaml::Null => String::new(),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

/// .NET's `$1` / `${name}` in a replacement, as the regex crate reads it (`${1}`).
fn replacement(rep: &str) -> String {
    let mut out = String::new();
    let b: Vec<char> = rep.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '$' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            out.push_str(&format!("${{{}}}", b[start..j].iter().collect::<String>()));
            i = j;
        } else if b[i] == '$' && b.get(i + 1) == Some(&'$') {
            out.push_str("$$");
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// A pattern as the definitions write them (.NET regular expressions).
pub fn regex(pat: &str) -> Result<fancy_regex::Regex, String> {
    fancy_regex::Regex::new(pat).map_err(|e| format!("The pattern {pat} can't be read: {e}"))
}

/// Every match of `pat` in `input` replaced with `rep` (`$1` for a group).
pub fn re_replace(input: &str, pat: &str, rep: &str) -> Result<String, String> {
    let re = regex(pat)?;
    Ok(re.replace_all(input, replacement(rep).as_str()).into_owned())
}

/// The first group of the first match of `pat` (the whole match when there is no group), "" for none.
pub fn regexp(input: &str, pat: &str) -> Result<String, String> {
    let re = regex(pat)?;
    Ok(match re.captures(input).ok().flatten() {
        Some(c) => c.get(1).or_else(|| c.get(0)).map(|m| m.as_str().to_string()).unwrap_or_default(),
        None => String::new(),
    })
}

/// Runs `filters` on `text` in order. `now` is the time relative dates are counted from.
pub fn apply(text: &str, filters: &[Filter], now: DateTime<Utc>) -> Result<String, String> {
    let mut s = text.to_string();
    for f in filters {
        s = apply_one(&s, f, now)?;
    }
    Ok(s)
}

fn apply_one(s: &str, f: &Filter, now: DateTime<Utc>) -> Result<String, String> {
    Ok(match f.name.as_str() {
        "replace" => s.replace(&f.arg(0), &f.arg(1)),
        "re_replace" => re_replace(s, &f.arg(0), &f.arg(1))?,
        "regexp" => regexp(s, &f.arg(0))?,
        "split" => {
            let sep = f.arg(0);
            let parts: Vec<&str> = if sep.is_empty() { vec![s] } else { s.split(sep.as_str()).collect() };
            let i: i64 = f.arg(1).trim().parse().unwrap_or(0);
            let i = if i < 0 { parts.len() as i64 + i } else { i };
            parts.get(i.max(0) as usize).map(|p| p.to_string()).unwrap_or_default()
        }
        "trim" => {
            let c = f.arg(0);
            if c.is_empty() {
                s.trim().to_string()
            } else {
                s.trim_matches(|ch| c.contains(ch)).to_string()
            }
        }
        "prepend" => format!("{}{s}", f.arg(0)),
        "append" => format!("{s}{}", f.arg(0)),
        "tolower" => s.to_lowercase(),
        "toupper" => s.to_uppercase(),
        "urldecode" => percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned(),
        "urlencode" => url::form_urlencoded::byte_serialize(s.as_bytes()).collect(),
        "htmldecode" => html_escape::decode_html_entities(s).into_owned(),
        "htmlencode" => html_escape::encode_text(s).into_owned(),
        "querystring" => {
            let key = f.arg(0);
            let q = s.split_once('?').map(|(_, q)| q).unwrap_or(s);
            url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| *k == key).map(|(_, v)| v.into_owned()).unwrap_or_default()
        }
        "dateparse" | "timeparse" => match parse_date_format(s, &f.arg(0), now) {
            Some(d) => d.to_rfc3339(),
            None => return Err(format!("The date \"{s}\" doesn't match the layout \"{}\"", f.arg(0))),
        },
        "timeago" | "reltime" => relative(s, now).map(|d| d.to_rfc3339()).unwrap_or_else(|| s.to_string()),
        "fuzzytime" => fuzzy_date(s, now).map(|d| d.to_rfc3339()).unwrap_or_else(|| s.to_string()),
        "validfilename" => s.chars().filter(|c| !"<>:\"/\\|?*".contains(*c) && !c.is_control()).collect(),
        "diacritics" => {
            use unicode_normalization::UnicodeNormalization;
            if f.arg(0) == "replace" || f.args.is_none() {
                s.nfd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).collect()
            } else {
                s.to_string()
            }
        }
        "validate" => {
            // keeps only the words of the list (comma separated), in the text's order
            let allowed: Vec<String> = f.arg(0).split(',').map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).collect();
            s.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
                .filter(|w| allowed.contains(&w.to_lowercase()))
                .collect::<Vec<_>>()
                .join(",")
        }
        "jsonjoinarray" => {
            let v: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("The text is not JSON: {e}"))?;
            match super::json_path::get(&v, &f.arg(0)) {
                Some(serde_json::Value::Array(a)) => a.iter().filter_map(super::json_path::text).collect::<Vec<_>>().join(&f.arg(1)),
                Some(other) => super::json_path::text(other).unwrap_or_default(),
                None => String::new(),
            }
        }
        "hexdump" | "strdump" => s.to_string(),
        "andmatch" => s.to_string(),
        other => return Err(format!("The filter {other} is not known")),
    })
}

/// Keyword filters (`keywordsfilters`): the same filters on the search words.
pub fn apply_keywords(text: &str, filters: &[Filter], now: DateTime<Utc>) -> Result<String, String> {
    apply(text, filters, now)
}

/// One piece of a .NET date layout.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Year(usize),
    Month(usize),
    Day(usize),
    DayName,
    Hour24(usize),
    Hour12(usize),
    Minute(usize),
    Second(usize),
    Frac(usize),
    AmPm,
    Zone,
    Lit(char),
    Space,
}

fn layout(fmt: &str) -> Vec<Tok> {
    let b: Vec<char> = fmt.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '\'' || c == '"' {
            i += 1;
            while i < b.len() && b[i] != c {
                out.push(Tok::Lit(b[i]));
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < b.len() {
            out.push(Tok::Lit(b[i + 1]));
            i += 2;
            continue;
        }
        let mut n = 1;
        while i + n < b.len() && b[i + n] == c {
            n += 1;
        }
        let tok = match c {
            'y' => Tok::Year(n),
            'M' => Tok::Month(n),
            'd' if n >= 3 => Tok::DayName,
            'd' => Tok::Day(n),
            'H' => Tok::Hour24(n),
            'h' => Tok::Hour12(n),
            'm' => Tok::Minute(n),
            's' => Tok::Second(n),
            'f' | 'F' => Tok::Frac(n),
            't' => Tok::AmPm,
            'z' | 'K' => Tok::Zone,
            c if c.is_whitespace() => Tok::Space,
            _ => {
                for _ in 0..n {
                    out.push(Tok::Lit(c));
                }
                i += n;
                continue;
            }
        };
        out.push(tok);
        i += n;
    }
    out
}

fn numeric(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Year(_) | Tok::Month(1..=2) | Tok::Day(_) | Tok::Hour24(_) | Tok::Hour12(_) | Tok::Minute(_) | Tok::Second(_) | Tok::Frac(_)
    )
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// The month (1–12) a name starts with, and how many characters of `s` the name takes.
fn month_name(s: &[char]) -> Option<(u32, usize)> {
    let word: String = s.iter().take_while(|c| c.is_alphabetic()).collect();
    let lower = word.to_lowercase();
    if lower.len() < 3 {
        return None;
    }
    // English, then the most common other spellings the sites use
    let extra: [(&str, u32); 14] = [
        ("janv", 1),
        ("févr", 2),
        ("fevr", 2),
        ("mars", 3),
        ("avr", 4),
        ("mai", 5),
        ("juin", 6),
        ("juil", 7),
        ("août", 8),
        ("aout", 8),
        ("déc", 12),
        ("okt", 10),
        ("dez", 12),
        ("mär", 3),
    ];
    for (p, m) in extra {
        if lower.starts_with(p) {
            return Some((m, word.chars().count()));
        }
    }
    MONTHS.iter().position(|m| lower.starts_with(m)).map(|i| (i as u32 + 1, word.chars().count()))
}

/// `s` read with the .NET layout `fmt` ("yyyy-MM-dd HH:mm:ss zzz"); a missing year is this year, a missing time
/// midnight, a missing zone UTC.
pub fn parse_date_format(s: &str, fmt: &str, now: DateTime<Utc>) -> Option<DateTime<FixedOffset>> {
    let toks = layout(fmt);
    let b: Vec<char> = s.trim().chars().collect();
    let mut i = 0;
    let (mut year, mut month, mut day) = (None::<i32>, None::<u32>, None::<u32>);
    let (mut hour, mut minute, mut second, mut nanos) = (0u32, 0u32, 0u32, 0u32);
    let mut pm: Option<bool> = None;
    let mut zone = 0i32;
    let read_num = |i: &mut usize, max: usize, exact: bool| -> Option<u32> {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() && *i - start < max {
            *i += 1;
        }
        if *i == start || (exact && *i - start != max) {
            return None;
        }
        b[start..*i].iter().collect::<String>().parse().ok()
    };
    for (k, t) in toks.iter().enumerate() {
        let next_numeric = toks.get(k + 1).is_some_and(numeric);
        match t {
            Tok::Space => {
                while i < b.len() && b[i].is_whitespace() {
                    i += 1;
                }
            }
            Tok::Lit(c) => {
                if i < b.len() && b[i].to_lowercase().eq(c.to_lowercase()) {
                    i += 1;
                }
            }
            Tok::Year(n) => {
                let width = if *n <= 2 { 2 } else { 4 };
                let y = read_num(&mut i, width, next_numeric)? as i32;
                year = Some(if width == 2 { 2000 + y } else { y });
            }
            Tok::Month(n) if *n >= 3 => {
                let (m, len) = month_name(&b[i..])?;
                month = Some(m);
                i += len;
            }
            Tok::Month(n) => month = Some(read_num(&mut i, 2, next_numeric && *n == 2)?),
            Tok::Day(n) => day = Some(read_num(&mut i, 2, next_numeric && *n == 2)?),
            Tok::DayName => {
                while i < b.len() && b[i].is_alphabetic() {
                    i += 1;
                }
            }
            Tok::Hour24(n) | Tok::Hour12(n) => hour = read_num(&mut i, 2, next_numeric && *n == 2)?,
            Tok::Minute(n) => minute = read_num(&mut i, 2, next_numeric && *n == 2)?,
            Tok::Second(n) => second = read_num(&mut i, 2, next_numeric && *n == 2)?,
            Tok::Frac(n) => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() && i - start < *n {
                    i += 1;
                }
                let digits: String = b[start..i].iter().collect();
                nanos = format!("{digits:0<9}").get(..9).and_then(|d| d.parse().ok()).unwrap_or(0);
            }
            Tok::AmPm => {
                let word: String = b[i..].iter().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
                if word.starts_with('a') {
                    pm = Some(false);
                } else if word.starts_with('p') {
                    pm = Some(true);
                }
                i += word.chars().count();
            }
            Tok::Zone => {
                while i < b.len() && b[i].is_whitespace() {
                    i += 1;
                }
                let rest: String = b[i..].iter().collect();
                let (offset, len) = zone_offset(&rest)?;
                zone = offset;
                i += len;
            }
        }
    }
    if let Some(pm) = pm {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    let tz = FixedOffset::east_opt(zone)?;
    let today = now.with_timezone(&tz);
    let mut date = NaiveDate::from_ymd_opt(year.unwrap_or(today.year()), month.unwrap_or(today.month()), day.unwrap_or(today.day()))?;
    // "Sep. 14" without a year that would be in the future is last year's
    if year.is_none() && date > today.date_naive() + Duration::days(1) {
        date = NaiveDate::from_ymd_opt(date.year() - 1, date.month(), date.day())?;
    }
    let t = date.and_hms_nano_opt(hour, minute, second, nanos)?;
    tz.from_local_datetime(&t).single()
}

/// A zone at the start of `s`: `+08:00`, `+0800`, `-05`, `UTC`, `GMT`, `Z`, `MSK`, `CET`…; seconds east of UTC and
/// the characters it takes.
fn zone_offset(s: &str) -> Option<(i32, usize)> {
    let b: Vec<char> = s.chars().collect();
    if let Some(&sign) = b.first().filter(|c| **c == '+' || **c == '-') {
        let digits: String = b[1..].iter().take_while(|c| c.is_ascii_digit() || **c == ':').collect();
        let plain = digits.replace(':', "");
        let (h, m) = match plain.len() {
            1 | 2 => (plain.parse::<i32>().ok()?, 0),
            3 => (plain[..1].parse().ok()?, plain[1..].parse().ok()?),
            4 => (plain[..2].parse().ok()?, plain[2..].parse().ok()?),
            _ => return None,
        };
        let secs = h * 3600 + m * 60;
        return Some((if sign == '-' { -secs } else { secs }, 1 + digits.chars().count()));
    }
    let word: String = b.iter().take_while(|c| c.is_alphabetic()).collect();
    let offset = match word.to_uppercase().as_str() {
        "UTC" | "GMT" | "Z" | "WET" => 0,
        "CET" | "BST" | "WEST" => 3600,
        "CEST" | "EET" => 7200,
        "MSK" | "EEST" => 3 * 3600,
        "EST" => -5 * 3600,
        "EDT" => -4 * 3600,
        "PST" => -8 * 3600,
        "PDT" => -7 * 3600,
        "CST" => 8 * 3600,
        "JST" | "KST" => 9 * 3600,
        _ => return None,
    };
    Some((offset, word.chars().count()))
}

/// "2 hours ago", "3 days", "1 week 2 days ago", "yesterday", "just now": `now` less that much.
pub fn relative(s: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let l = s.trim().to_lowercase();
    if l.is_empty() {
        return None;
    }
    if l.contains("now") || l.contains("just") || l == "today" {
        return Some(now);
    }
    if l.starts_with("yesterday") {
        return Some(now - Duration::days(1));
    }
    let re = regex::Regex::new(r"(\d+(?:[.,]\d+)?)\s*([a-zа-яё]+)").ok()?;
    let mut total = 0f64;
    let mut any = false;
    for c in re.captures_iter(&l) {
        let n: f64 = c[1].replace(',', ".").parse().ok()?;
        let unit = &c[2];
        let secs = if unit.starts_with("sec") || unit == "s" || unit.starts_with("сек") {
            1.0
        } else if unit.starts_with("min") || unit == "m" || unit.starts_with("мин") {
            60.0
        } else if unit.starts_with('h') || unit.starts_with("час") {
            3600.0
        } else if unit.starts_with('d') || unit.starts_with("дн") || unit.starts_with("ден") || unit.starts_with("jour") {
            86400.0
        } else if unit.starts_with('w') || unit.starts_with("нед") || unit.starts_with("sem") {
            7.0 * 86400.0
        } else if unit.starts_with("mo") || unit.starts_with("мес") || unit.starts_with("mois") {
            30.0 * 86400.0
        } else if unit.starts_with('y') || unit.starts_with("год") || unit.starts_with("лет") || unit.starts_with("an") {
            365.0 * 86400.0
        } else {
            continue;
        };
        total += n * secs;
        any = true;
    }
    any.then(|| now - Duration::milliseconds((total * 1000.0) as i64))
}

/// Any common way of writing a date: a Unix time, RFC 3339 / 2822, ISO, "today 12:25", "yesterday", "12:25am",
/// "2 hours ago", "Apr 18 2011" and the usual numeric layouts.
pub fn fuzzy_date(s: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(n) = t.parse::<i64>() {
        // seconds, or milliseconds when it is that large
        let secs = if n > 100_000_000_000 { n / 1000 } else { n };
        return Utc.timestamp_opt(secs, 0).single();
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(t) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(t) {
        return Some(d.with_timezone(&Utc));
    }
    let l = t.to_lowercase();
    for (word, days) in
        [("today", 0), ("yesterday", 1), ("сегодня", 0), ("вчера", 1), ("aujourd'hui", 0), ("hier", 1), ("heute", 0), ("gestern", 1)]
    {
        if let Some(rest) = l.strip_prefix(word) {
            let day = now - Duration::days(days);
            let rest = rest.trim().trim_start_matches([',', 'a', 't', ' ', 'в']).trim();
            let time = ["HH:mm:ss", "HH:mm", "h:mmtt", "h:mm tt"].iter().find_map(|f| parse_date_format(rest, f, day));
            return Some(time.map(|d| d.with_timezone(&Utc)).unwrap_or(day));
        }
    }
    if l.contains("ago") || l.contains("назад") || l.contains("il y a") || l.contains("vor ") {
        return relative(&l, now);
    }
    const LAYOUTS: [&str; 24] = [
        "yyyy-MM-dd HH:mm:ss zzz",
        "yyyy-MM-ddTHH:mm:ss",
        "yyyy-MM-dd HH:mm:ss",
        "yyyy-MM-dd HH:mm",
        "yyyy-MM-dd",
        "yyyy/MM/dd HH:mm:ss",
        "yyyy/MM/dd",
        "dd.MM.yyyy HH:mm:ss",
        "dd.MM.yyyy HH:mm",
        "dd.MM.yyyy",
        "dd/MM/yyyy HH:mm:ss",
        "dd/MM/yyyy HH:mm",
        "dd/MM/yyyy",
        "dd-MM-yyyy HH:mm",
        "dd-MM-yyyy",
        "MMM d yyyy, HH:mm",
        "MMM d, yyyy HH:mm",
        "MMM d yyyy",
        "MMM d, yyyy",
        "d MMM yyyy HH:mm",
        "d MMM yyyy",
        "h:mmtt",
        "HH:mm:ss",
        "HH:mm",
    ];
    for f in LAYOUTS {
        if let Some(d) = parse_date_format(t, f, now) {
            if layout_fits(t, f) {
                return Some(d.with_timezone(&Utc));
            }
        }
    }
    relative(&l, now)
}

/// Whether `t` looks like it was written with the layout `f` (the same count of digit groups), so a loose layout
/// doesn't take a date meant for a longer one.
fn layout_fits(t: &str, f: &str) -> bool {
    let groups = |s: &str, digit: &dyn Fn(char) -> bool| s.split(|c: char| !digit(c)).filter(|g| !g.is_empty()).count();
    let digit_groups = groups(t, &|c: char| c.is_ascii_digit());
    let layout_groups = groups(f, &|c: char| "yMdHhmsf".contains(c)) - f.matches("MMM").count();
    digit_groups == layout_groups
}

/// A size in bytes: `1.5 GB`, `1,234.5 MiB`, `700 Mo`, `1.2 ГБ`, or a plain number of bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let t = s.trim().replace('\u{a0}', " ");
    if t.is_empty() {
        return None;
    }
    let re = regex::Regex::new(r"(?i)([\d][\d.,\s]*)\s*([kmgtp]?i?[bo]|[кмгт]?и?б|bytes?)?").ok()?;
    let c = re.captures(&t)?;
    let num = c.get(1)?.as_str().replace(' ', "");
    // "1,234.5" and "1.234,5" and "1,5"
    let num = if num.contains(',') && num.contains('.') {
        if num.rfind(',') > num.rfind('.') {
            num.replace('.', "").replace(',', ".")
        } else {
            num.replace(',', "")
        }
    } else if num.contains(',') {
        let after = num.rsplit(',').next().unwrap_or("");
        if after.len() == 3 && num.matches(',').count() >= 1 && !num.starts_with("0,") {
            num.replace(',', "")
        } else {
            num.replace(',', ".")
        }
    } else {
        num
    };
    let n: f64 = num.trim_end_matches('.').parse().ok()?;
    let unit = c.get(2).map(|u| u.as_str().to_lowercase()).unwrap_or_default();
    let first = unit.chars().next().unwrap_or('b');
    let pow = match first {
        'k' | 'к' => 1,
        'm' | 'м' => 2,
        'g' | 'г' => 3,
        't' | 'т' => 4,
        'p' => 5,
        _ => 0,
    };
    Some((n * 1024f64.powi(pow)).round() as u64)
}

/// A count: "1,234" → 1234, "" → None.
pub fn parse_count(s: &str) -> Option<i64> {
    let digits: String = s.trim().chars().filter(|c| c.is_ascii_digit() || *c == '-').collect();
    digits.parse().ok()
}

/// A factor: "0", "0.5", "1", "50%" → 0.5.
pub fn parse_factor(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if let Some(p) = t.strip_suffix('%') {
        return p.trim().parse::<f64>().ok().map(|v| v / 100.0);
    }
    t.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).unwrap()
    }

    #[test]
    fn filters_change_text_like_the_definitions_expect() {
        let f = |name: &str, args: serde_yaml::Value| Filter { name: name.into(), args: Some(args) };
        let y = |s: &str| serde_yaml::from_str::<serde_yaml::Value>(s).unwrap();
        assert_eq!(apply("a-b-c", &[f("replace", y("[\"-\", \" \"]"))], now()).unwrap(), "a b c");
        assert_eq!(apply("Show.S01E02", &[f("re_replace", y(r#"["(?i)s(\\d+)e(\\d+)", "$1x$2"]"#))], now()).unwrap(), "Show.01x02");
        assert_eq!(apply("/torrent/123/name/", &[f("split", y("[\"/\", 2]"))], now()).unwrap(), "123");
        assert_eq!(apply("details.php?id=42&x=1", &[f("querystring", y("id"))], now()).unwrap(), "42");
        assert_eq!(apply("hash: ABCDEF0123", &[f("regexp", y(r#""([A-F0-9]+)$""#))], now()).unwrap(), "ABCDEF0123");
        assert_eq!(apply("x", &[f("append", y("\"y\"")), f("prepend", y("\"w\""))], now()).unwrap(), "wxy");
        assert!(apply("x", &[f("nope", y("1"))], now()).is_err());
    }

    #[test]
    fn dates_parse_with_dotnet_layouts() {
        let d = parse_date_format("2026-09-19 16:50 +08:00", "yyyy-MM-dd HH:mm zzz", now()).unwrap();
        assert_eq!(d.to_rfc3339(), "2026-09-19T16:50:00+08:00");
        let d = parse_date_format("7am Sep. 14", "htt MMM. d", now()).unwrap();
        assert_eq!(d.to_rfc3339(), "2026-09-14T07:00:00+00:00");
        let d = parse_date_format("Apr. 18 11", "MMM. d yy", now()).unwrap();
        assert_eq!(d.date_naive().to_string(), "2011-04-18");
        let d = parse_date_format("2026-11-0412:30:00 +03:00", "yyyy-MM-ddHH:mm:ss zzz", now()).unwrap();
        assert_eq!(d.to_rfc3339(), "2026-11-04T12:30:00+03:00");
        let d = parse_date_format("Dec 24", "MMM d", now()).unwrap();
        assert_eq!(d.year(), 2025, "a date without a year that would be ahead is last year's");
        assert_eq!(fuzzy_date("2 hours ago", now()).unwrap(), now() - Duration::hours(2));
        assert_eq!(fuzzy_date("yesterday 10:00", now()).unwrap().to_rfc3339(), "2026-11-04T10:00:00+00:00");
        assert_eq!(fuzzy_date("1700000000", now()).unwrap().timestamp(), 1_700_000_000);
        assert_eq!(fuzzy_date("19.09.2026", now()).unwrap().date_naive().to_string(), "2026-09-19");
    }

    #[test]
    fn sizes_and_numbers() {
        assert_eq!(parse_size("1.5 GB"), Some(1_610_612_736));
        assert_eq!(parse_size("1,234.5 MiB"), Some(1_294_467_072));
        assert_eq!(parse_size("700 Mo"), Some(734_003_200));
        assert_eq!(parse_size("1,5 ГБ"), Some(1_610_612_736));
        assert_eq!(parse_size("123456"), Some(123_456));
        assert_eq!(parse_count("1,234"), Some(1234));
        assert_eq!(parse_factor("50%"), Some(0.5));
    }
}
