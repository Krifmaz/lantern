// Ported from autobrr v1.90.0 (internal/domain/rejections.go), GPL-2.0-or-later.
//! Why a filter turned a release down: one line per check that failed, with what the release had and what the
//! filter wanted (`[resolution] not matching: got 1080p want: [2160p]`).

use std::fmt;

/// A value in a rejection, printed the way Go's `%v` prints it.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// Text.
    Str(String),
    /// A list of texts, printed `[a b]`.
    List(Vec<String>),
    /// A whole number.
    Int(i64),
    /// A whole number that can't be negative (sizes).
    Uint(u64),
    /// Yes or no, printed `true` or `false`.
    Bool(bool),
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Str(s) => f.write_str(s),
            Val::List(l) => write!(f, "[{}]", l.join(" ")),
            Val::Int(n) => write!(f, "{n}"),
            Val::Uint(n) => write!(f, "{n}"),
            Val::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl From<&str> for Val {
    fn from(s: &str) -> Val {
        Val::Str(s.to_string())
    }
}

impl From<String> for Val {
    fn from(s: String) -> Val {
        Val::Str(s)
    }
}

impl From<&String> for Val {
    fn from(s: &String) -> Val {
        Val::Str(s.clone())
    }
}

impl From<&[String]> for Val {
    fn from(l: &[String]) -> Val {
        Val::List(l.to_vec())
    }
}

impl From<&Vec<String>> for Val {
    fn from(l: &Vec<String>) -> Val {
        Val::List(l.clone())
    }
}

impl From<Vec<String>> for Val {
    fn from(l: Vec<String>) -> Val {
        Val::List(l)
    }
}

impl From<i64> for Val {
    fn from(n: i64) -> Val {
        Val::Int(n)
    }
}

impl From<u64> for Val {
    fn from(n: u64) -> Val {
        Val::Uint(n)
    }
}

impl From<bool> for Val {
    fn from(b: bool) -> Val {
        Val::Bool(b)
    }
}

/// One failed check.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    /// The check's name (`resolution`, `match releases`).
    pub key: String,
    /// What the release had.
    pub got: Val,
    /// What the filter wanted.
    pub want: Val,
    /// How to say it, Go `fmt` style with `%[1]s` the key, `%[2]` what it had, `%[3]` what it wanted (empty: the
    /// usual `[key] not matching: got … want: …`).
    pub format: String,
}

/// Every failed check of one filter on one release, in the order they were found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RejectionReasons {
    /// The failed checks.
    pub data: Vec<Rejection>,
}

/// Text cut to at most 1024 bytes (at a character boundary).
fn cut(s: &str) -> &str {
    if s.len() <= 1024 {
        return s;
    }
    let mut n = 1024;
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// A value with text cut to 1024 bytes.
fn cut_val(v: &Val) -> Val {
    match v {
        Val::Str(s) => Val::Str(cut(s).to_string()),
        other => other.clone(),
    }
}

/// `format` filled in Go `fmt` style: `%[n]x` takes argument n (1-based), `%x` the next one, `%%` is `%`.
fn go_format(format: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut next = 0;
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            let n: String = chars.by_ref().take_while(|&c| c != ']').collect();
            next = n.parse::<usize>().unwrap_or(1).saturating_sub(1);
        }
        // the verb (s, d, v …); every argument is already text
        chars.next();
        match args.get(next) {
            Some(a) => out.push_str(a),
            None => out.push_str("%!(MISSING)"),
        }
        next += 1;
    }
    out
}

impl RejectionReasons {
    /// No failed checks yet.
    pub fn new() -> RejectionReasons {
        RejectionReasons::default()
    }

    /// How many checks failed.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether no check failed.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Notes a failed check.
    pub fn add(&mut self, key: &str, got: impl Into<Val>, want: impl Into<Val>) {
        self.data.push(Rejection { key: key.into(), got: got.into(), want: want.into(), format: String::new() });
    }

    /// Notes a failed check said in its own words (`format`, see [`Rejection::format`]).
    pub fn addf(&mut self, key: &str, format: &str, got: impl Into<Val>, want: impl Into<Val>) {
        self.data.push(Rejection { key: key.into(), got: got.into(), want: want.into(), format: format.into() });
    }

    /// Notes a failed check, with long text in what was wanted cut to 1024 bytes.
    pub fn add_truncated(&mut self, key: &str, got: impl Into<Val>, want: impl Into<Val>) {
        let want = match want.into() {
            Val::Str(s) => Val::Str(cut(&s).to_string()),
            Val::List(l) => Val::List(l.iter().map(|s| cut(s).to_string()).collect()),
            other => other,
        };
        self.add(key, got, want);
    }

    /// Forgets every failed check.
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Every failed check in words, one line each.
    pub fn lines(&self) -> Vec<String> {
        self.data
            .iter()
            .map(|r| {
                if r.format.is_empty() {
                    format!("[{}] not matching: got {} want: {}", r.key, r.got, r.want)
                } else {
                    go_format(&r.format, &[r.key.clone(), r.got.to_string(), r.want.to_string()])
                }
            })
            .collect()
    }

    /// Every failed check in words, separated by `, ` (empty when none failed).
    pub fn string(&self) -> String {
        self.lines().join(", ")
    }

    /// Like [`RejectionReasons::string`], always in the usual form, with long texts cut to 1024 bytes.
    pub fn string_truncated(&self) -> String {
        self.data
            .iter()
            .map(|r| format!("[{}] not matching: got {} want: {}", r.key, cut_val(&r.got), cut_val(&r.want)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for RejectionReasons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_go() {
        let mut r = RejectionReasons::new();
        r.addf("max downloads", "[%[1]s] %[3]s", "Period: 2", "reached 2 per DAY");
        r.addf("release size", "release size %[2]d bytes is smaller than filter min size %[3]d bytes", 5u64, 10u64);
        assert_eq!(r.string(), "[max downloads] reached 2 per DAY, release size 5 bytes is smaller than filter min size 10 bytes");
    }
}
