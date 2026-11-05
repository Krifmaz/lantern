// Lantern's own helper for the rls port (rls and autobrr write their patterns in Go's regexp syntax).
//! Go regular expressions in Rust: rls and autobrr write their patterns for Go's `regexp`, whose `\b`, `\d`, `\s`
//! and `\w` only know ASCII and which has `\Q…\E` quoting. [`go`] rewrites a Go pattern so that Rust's `regex`
//! crate matches exactly what Go would.

use regex::bytes::{Regex as BytesRegex, RegexBuilder as BytesRegexBuilder};
use regex::{Regex, RegexBuilder};

/// The compiled size Lantern allows one pattern (the audio and collection patterns are long).
const SIZE_LIMIT: usize = 256 << 20;

/// Rewrites Go pattern `p` for Rust's `regex` crate: `\Q…\E` becomes escaped text, and `\b`, `\B`, `\d`, `\D`,
/// `\s`, `\S`, `\w`, `\W` become their ASCII forms, as Go reads them.
pub fn go(p: &str) -> String {
    let chars: Vec<char> = p.chars().collect();
    let mut out = String::with_capacity(p.len() + 16);
    let mut i = 0;
    let mut in_class = false;
    // where the current class started (to know whether `]` is its first character)
    let mut class_start = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let e = chars[i + 1];
            match e {
                'Q' => {
                    let mut lit = String::new();
                    let mut j = i + 2;
                    while j < chars.len() {
                        if chars[j] == '\\' && j + 1 < chars.len() && chars[j + 1] == 'E' {
                            j += 2;
                            break;
                        }
                        lit.push(chars[j]);
                        j += 1;
                    }
                    out.push_str(&regex::escape(&lit));
                    i = j;
                    continue;
                }
                'b' if !in_class => out.push_str(r"(?-u:\b)"),
                'B' if !in_class => out.push_str(r"(?-u:\B)"),
                'd' => out.push_str(if in_class { "0-9" } else { "[0-9]" }),
                'D' if !in_class => out.push_str("[^0-9]"),
                's' => out.push_str(if in_class { r"\t\n\f\r " } else { r"[\t\n\f\r ]" }),
                'S' if !in_class => out.push_str(r"[^\t\n\f\r ]"),
                'w' => out.push_str(if in_class { "0-9A-Za-z_" } else { "[0-9A-Za-z_]" }),
                'W' if !in_class => out.push_str("[^0-9A-Za-z_]"),
                // Go lets any punctuation be escaped; Rust only the characters that mean something
                e if e.is_ascii_punctuation() && !is_rust_escapable(e) => out.push(e),
                _ => {
                    out.push(c);
                    out.push(e);
                }
            }
            i += 2;
            continue;
        }
        if in_class {
            if c == '[' && chars.get(i + 1) == Some(&':') {
                // a POSIX class like [:alpha:]
                if let Some(end) = (i + 2..chars.len().saturating_sub(1)).find(|&j| chars[j] == ':' && chars[j + 1] == ']') {
                    out.extend(&chars[i..end + 2]);
                    i = end + 2;
                    continue;
                }
            }
            if c == ']' && i != class_start {
                in_class = false;
            } else if c == '[' {
                // a literal `[` inside a class: Rust reads it as a nested class
                out.push_str(r"\[");
                i += 1;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }
        if c == '[' {
            in_class = true;
            out.push(c);
            i += 1;
            if chars.get(i) == Some(&'^') {
                out.push('^');
                i += 1;
            }
            class_start = i;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Whether Rust's `regex` accepts `\c` for the punctuation character `c`.
fn is_rust_escapable(c: char) -> bool {
    regex_syntax_meta(c) || matches!(c, '-' | '&' | '~' | '#' | ' ')
}

/// The characters that are special in a Rust pattern.
fn regex_syntax_meta(c: char) -> bool {
    matches!(c, '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$')
}

/// Compiles Go pattern `p` for text.
pub fn compile(p: &str) -> Regex {
    RegexBuilder::new(&go(p))
        .size_limit(SIZE_LIMIT)
        .dfa_size_limit(SIZE_LIMIT)
        .build()
        .unwrap_or_else(|e| panic!("the pattern {p:?} can't be read: {e}"))
}

/// Compiles Go pattern `p` for bytes.
pub fn compile_bytes(p: &str) -> BytesRegex {
    BytesRegexBuilder::new(&go(p))
        .size_limit(SIZE_LIMIT)
        .dfa_size_limit(SIZE_LIMIT)
        .build()
        .unwrap_or_else(|e| panic!("the pattern {p:?} can't be read: {e}"))
}

/// Compiles Go pattern `p` for text; `None` when Go would refuse it too (or Rust can't read it).
pub fn try_compile(p: &str) -> Option<Regex> {
    RegexBuilder::new(&go(p)).size_limit(SIZE_LIMIT).dfa_size_limit(SIZE_LIMIT).build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_go_patterns() {
        assert_eq!(go(r"\Qa.b\E"), r"a\.b");
        assert_eq!(go(r"x\by"), r"x(?-u:\b)y");
        assert_eq!(go(r"[\d\s]"), r"[0-9\t\n\f\r ]");
        assert_eq!(go(r"\d+"), r"[0-9]+");
        assert_eq!(go(r"[A-Z\d\-\_\. ]"), r"[A-Z0-9\-_\. ]");
        assert!(compile(r"(?i)^(?:\Qdv\E\b)").is_match("DV"));
        assert!(compile(r"[\]\)\}]").is_match("]"));
        assert!(compile(r"^[\]\)\}]").is_match(")"));
        // Go's \b only knows ASCII letters: é is not a word character
        assert!(compile(r"a\b").is_match("aé"));
        assert!(!compile(r"a\B").is_match("aé"));
    }
}
