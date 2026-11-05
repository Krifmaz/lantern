//! cross-seed's regular expressions are JavaScript ones. This module turns a JavaScript pattern into one that
//! [`fancy_regex`] reads with the same meaning, so the patterns in [`super::constants`] can be copied from
//! cross-seed as they are:
//!
//! | JavaScript | Here | Why |
//! | --- | --- | --- |
//! | `\d`, `\w`, `\W`, `\b` | ASCII classes and an ASCII word boundary | JavaScript's are ASCII; Rust's are Unicode |
//! | `{3.5}` (not a count) | `\{3.5\}` | JavaScript reads a brace that isn't a count as a plain brace |
//! | `[` inside `[...]` | `\[` | Rust reads it as a class inside the class |
//! | `(?<=S\d+[_\s-]{1,3})` | one look-behind per length | Rust's look-behinds have one length (see [`fixed_lookbehind`]) |
//!
//! A match that takes too long (the backtracking limit) counts as no match, as a JavaScript match that never ends
//! is no use either.

use fancy_regex::{Captures, Regex, RegexBuilder};

/// The ASCII word characters, as JavaScript's `\w`.
const WORD: &str = "0-9A-Za-z_";

/// JavaScript's `\b`: between an ASCII word character and anything else (or either end).
const BOUNDARY: &str = "(?:(?<![0-9A-Za-z_])(?=[0-9A-Za-z_])|(?<=[0-9A-Za-z_])(?![0-9A-Za-z_]))";

/// JavaScript's `\B`: not a word boundary.
const NOT_BOUNDARY: &str = "(?:(?<![0-9A-Za-z_])(?![0-9A-Za-z_])|(?<=[0-9A-Za-z_])(?=[0-9A-Za-z_]))";

/// Whether `rest` (just after a `{`) is a JavaScript count: `n}`, `n,}` or `n,m}`.
fn is_count(rest: &str) -> bool {
    let Some(end) = rest.find('}') else { return false };
    let inner = &rest[..end];
    let mut parts = inner.splitn(2, ',');
    let a = parts.next().unwrap_or("");
    let b = parts.next();
    !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.is_none_or(|b| b.bytes().all(|c| c.is_ascii_digit()))
}

/// The JavaScript pattern `js` written for [`fancy_regex`] (see the module's table).
pub fn translate(js: &str) -> String {
    let mut out = String::with_capacity(js.len() * 2);
    let chars: Vec<char> = js.chars().collect();
    let mut i = 0;
    let mut in_class = false;
    let mut class_start = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            i += 2;
            let piece: String = match (n, in_class) {
                ('d', false) => "[0-9]".into(),
                ('D', false) => "[^0-9]".into(),
                ('w', false) => format!("[{WORD}]"),
                ('W', false) => format!("[^{WORD}]"),
                ('b', false) => BOUNDARY.into(),
                ('B', false) => NOT_BOUNDARY.into(),
                ('d', true) => "0-9".into(),
                ('w', true) => WORD.into(),
                ('D', true) => "[^0-9]".into(),
                ('W', true) => format!("[^{WORD}]"),
                // JavaScript's `\/` and other escaped punctuation: the character itself, escaped when Rust needs it
                (p, _) if p.is_ascii_punctuation() => format!("\\{p}"),
                (p, _) => format!("\\{p}"),
            };
            out.push_str(&piece);
            class_start = false;
            continue;
        }
        if in_class {
            match c {
                ']' if !class_start => {
                    in_class = false;
                    out.push(']');
                }
                '[' => out.push_str("\\["),
                '&' | '~' if chars.get(i + 1) == Some(&c) => {
                    out.push('\\');
                    out.push(c);
                }
                '^' if class_start => {
                    out.push('^');
                    i += 1;
                    continue;
                }
                _ => out.push(c),
            }
            class_start = false;
            i += 1;
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                class_start = true;
                out.push('[');
            }
            '{' => {
                let rest: String = chars[i + 1..].iter().collect();
                if is_count(&rest) {
                    let end = rest.find('}').unwrap_or(0);
                    out.push('{');
                    out.push_str(&rest[..=end]);
                    i += 1 + rest[..=end].chars().count();
                    continue;
                }
                out.push_str("\\{");
            }
            '}' => out.push_str("\\}"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// The look-behind `(?<=S\d+[_\s-]{1,3})` (one in cross-seed's episode patterns), which has many lengths, as one
/// look-behind per length: a season of 1 to 4 digits, then 1 to 3 separators (a season number has never had more
/// digits).
pub fn fixed_lookbehind() -> String {
    let mut alts = Vec::new();
    for digits in 1..=4 {
        for seps in 1..=3 {
            alts.push(format!("(?<=S{}{})", "\\d".repeat(digits), "[_\\s-]".repeat(seps)));
        }
    }
    format!("(?:{})", alts.join("|"))
}

/// A compiled JavaScript pattern: `flags` as JavaScript's (`i`, `u`, `g`; `g` and `u` change nothing here).
pub fn re(js: &str, flags: &str) -> Regex {
    let pattern = js.replace("(?<=S\\d+[_\\s-]{1,3})", &fixed_lookbehind());
    let mut p = translate(&pattern);
    if flags.contains('i') {
        p = format!("(?i){p}");
    }
    RegexBuilder::new(&p).backtrack_limit(1_000_000).build().unwrap_or_else(|e| panic!("cross-seed pattern {js}: {e}"))
}

/// JavaScript's `re.test(s)`.
pub fn test(re: &Regex, s: &str) -> bool {
    re.is_match(s).unwrap_or(false)
}

/// JavaScript's `s.match(re)` for a pattern without `g`: the first match's groups.
pub fn captures<'t>(re: &Regex, s: &'t str) -> Option<Captures<'t>> {
    re.captures(s).ok().flatten()
}

/// The text of group `name` in the first match of `re` in `s` (`s.match(re)?.groups?.name`).
pub fn group(re: &Regex, s: &str, name: &str) -> Option<String> {
    captures(re, s).and_then(|c| c.name(name).map(|m| m.as_str().to_string()))
}

/// JavaScript's `s.replace(re, with)`: the first match only (a pattern without `g`).
pub fn replace_first(re: &Regex, s: &str, with: &str) -> String {
    match re.find(s) {
        Ok(Some(m)) => format!("{}{}{}", &s[..m.start()], with, &s[m.end()..]),
        _ => s.to_string(),
    }
}

/// JavaScript's `s.replace(re, with)` for a pattern with `g`: every match.
pub fn replace_all(re: &Regex, s: &str, with: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re.find_iter(s) {
        let Ok(m) = m else { return s.to_string() };
        out.push_str(&s[last..m.start()]);
        out.push_str(with);
        last = m.end();
    }
    out.push_str(&s[last..]);
    out
}

/// JavaScript's `s.split(re)` (no capture groups in `re`, and none of its matches empty).
pub fn split(re: &Regex, s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in re.find_iter(s) {
        let Ok(m) = m else { break };
        if m.start() == m.end() {
            continue;
        }
        out.push(s[last..m.start()].to_string());
        last = m.end();
    }
    out.push(s[last..].to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_classes_are_ascii() {
        let w = re(r"^\w+$", "");
        assert!(test(&w, "abc_09"));
        assert!(!test(&w, "café"));
        let b = re(r"\bnf\b", "i");
        assert!(test(&b, "Show.NF.WEB"));
        assert!(test(&b, "éNF.WEB"), "é is not a word character in JavaScript");
    }

    #[test]
    fn braces_that_are_not_counts_are_plain() {
        let r = re(r"^[ab]{3.5}$", "");
        assert!(test(&r, "a{3.5}"));
        let r = re(r"^a{2,3}$", "");
        assert!(test(&r, "aaa") && !test(&r, "a"));
    }

    #[test]
    fn brackets_inside_classes_are_plain() {
        let r = re(r"^[[(_.\s-]+$", "");
        assert!(test(&r, "[(_. -"));
        let r = re(r"^[a\W]+$", "");
        assert!(test(&r, "a.a!") && !test(&r, "ab"));
    }

    #[test]
    fn the_long_lookbehind_has_every_length() {
        let r = re(r"^S\d+[_\s-]{1,3}(?<=S\d+[_\s-]{1,3})(?<ep>\d+)$", "i");
        assert_eq!(group(&r, "S01 - 05", "ep").as_deref(), Some("05"));
        assert_eq!(group(&r, "s2_7", "ep").as_deref(), Some("7"));
        assert_eq!(group(&r, "S2024 7", "ep").as_deref(), Some("7"));
    }

    #[test]
    fn replaces_and_splits_like_javascript() {
        let r = re(r"\s+", "g");
        assert_eq!(replace_all(&r, "a  b   c", " "), "a b c");
        assert_eq!(replace_first(&r, "a  b   c", " "), "a b   c");
        assert_eq!(split(&re(r"-", ""), "a-b-c"), vec!["a", "b", "c"]);
    }
}
