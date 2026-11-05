// Ported from autobrr v1.90.0 (pkg/wildcard/match.go), GPL-2.0-or-later.
//! Wildcard patterns, as autobrr matches release names against them: `*` stands for any text and, outside the
//! simple form, `?` for one character. A name matches the whole pattern, on any of its lines.

use regex::Regex;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Whether `name` matches `pattern`, where only `*` is a wildcard (autobrr's `MatchSimple`).
pub fn match_simple(pattern: &str, name: &str) -> bool {
    matches(pattern, name, true)
}

/// Whether `name` matches `pattern`, where `*` and `?` are wildcards (autobrr's `Match`).
pub fn is_match(pattern: &str, name: &str) -> bool {
    matches(pattern, name, false)
}

/// Whether `name` matches any of `patterns`, where only `*` is a wildcard (autobrr's `MatchSliceSimple`).
pub fn match_slice_simple(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| matches(p, name, true))
}

/// Whether `name` matches any of `patterns`, where `*` and `?` are wildcards (autobrr's `MatchSlice`).
pub fn match_slice(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| matches(p, name, false))
}

/// The matching itself: quick answers for the common shapes, a regular expression for the rest.
fn matches(pattern: &str, name: &str, simple: bool) -> bool {
    let p = pattern.as_bytes();
    if pattern.is_empty() {
        return name.is_empty();
    }
    if pattern == "*" {
        return true;
    }
    if !simple && pattern == "?" {
        return name.len() == 1;
    }
    let Some(idx) = p.iter().position(|&c| c == b'*' || c == b'?') else {
        return name == pattern;
    };
    if simple && p[idx] == b'?' && !pattern.contains('*') {
        return name == pattern;
    }
    if idx == p.len() - 1 && p[idx] == b'*' {
        return name.as_bytes().starts_with(&p[..idx]);
    }
    let wild_end = p[p.len() - 1] == b'*';
    let stars = pattern.matches('*').count();
    if !simple && ((wild_end && stars == 1) || (pattern.len() == name.len() && !pattern.contains('*'))) {
        return match_complex(name.as_bytes(), p, wild_end);
    }
    if pattern.starts_with('*') && pattern.ends_with('*') && (simple || !pattern.contains('?')) && stars == 2 {
        return name.contains(&pattern[1..pattern.len() - 1]);
    }
    deep_match(name, pattern, simple)
}

/// `egg?bert?` and `egg?bert*`: compares the pattern piece by piece, one character for each `?` (autobrr's
/// `matchComplex`, byte by byte as Go does).
fn match_complex(name: &[u8], pattern: &[u8], wild_end: bool) -> bool {
    let mut base = 0;
    let mut consumed = 0;
    while base < name.len() && consumed < pattern.len() {
        let Some(i) = pattern.get(base..).and_then(|rest| rest.iter().position(|&c| c == b'?')) else {
            let rest_name = &name[base..];
            let ok = if wild_end {
                pattern.get(base..pattern.len() - 1).is_some_and(|want| rest_name.starts_with(want))
            } else {
                pattern.get(base..).is_some_and(|want| rest_name == want)
            };
            if !ok {
                break;
            }
            base = name.len();
            consumed = pattern.len();
            continue;
        };
        let offset = base + i;
        if name.len() < offset || name[base..offset] != pattern[base..offset] {
            break;
        }
        base = offset + 1;
        consumed = base;
    }
    base == name.len() && consumed == pattern.len()
}

/// The patterns already turned into regular expressions, by pattern and form (`None`: it could not be).
type Cache = HashMap<(String, bool), Option<Regex>>;

/// The regular expressions made so far.
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

/// Any other pattern, as the regular expression `(?m)^pattern$` with `*` as `.*` and `?` as `.` (autobrr's
/// `deepMatchRune`).
fn deep_match(name: &str, pattern: &str, simple: bool) -> bool {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let re = cache.entry((pattern.to_string(), simple)).or_insert_with(|| {
        let mut re = format!("(?m)^{}$", regex::escape(pattern)).replace(r"\*", ".*");
        if !simple {
            re = re.replace(r"\?", ".");
        }
        Regex::new(&re).ok()
    });
    re.as_ref().is_some_and(|re| re.is_match(name))
}
