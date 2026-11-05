// Ported from autobrr v1.90.0 (pkg/sanitize/sanitize.go), GPL-2.0-or-later.
//! Tidying what people type into filters: lists on several lines become one comma separated list.

/// Text without the spaces around it (autobrr's `sanitize.String`).
pub fn string(s: &str) -> String {
    s.trim().to_string()
}

/// A typed list as one comma separated line: line breaks become commas, tabs spaces, and doubled spaces and
/// commas, and those at either end, go (autobrr's `sanitize.FilterString`).
pub fn filter_string(s: &str) -> String {
    let mut s = s.replace(['\r', '\n', '\u{0b}'], ",").replace('\t', " ").replace('\u{0c}', "");
    for (old, new) in [("  ", " "), (", ", ","), (" ,", ","), (",,", ",")] {
        s = repeated_replace_all(&s, old, new);
    }
    s.trim_matches([',', ' ']).to_string()
}

/// Replaces `old` with `new` until the text stops getting shorter.
fn repeated_replace_all(src: &str, old: &str, new: &str) -> String {
    let mut src = src.to_string();
    loop {
        let before = src.len();
        src = src.replace(old, new);
        if src.len() == before {
            return src;
        }
    }
}
