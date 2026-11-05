// Ported from autobrr v1.90.0 (pkg/sanitize/sanitize.go), GPL-2.0-or-later.
//! Tidying what people type into filters: lists on several lines become one comma separated list.

/// Text without the spaces around it (autobrr's `sanitize.String`).
pub fn string(s: &str) -> String {
    s.trim().to_string()
}

/// An address with JSON's escapes (`\u0026` and the like) turned back into their characters, and without the
/// spaces around it (autobrr's `sanitize.URLEncoding`).
pub fn url_encoding(s: &str) -> String {
    let mut s = s.to_string();
    for (old, new) in [
        ("\\u0026", "&"),
        ("\\u003d", "="),
        ("\\u003f", "?"),
        ("\\u002f", "/"),
        ("\\u003a", ":"),
        ("\\u0023", "#"),
        ("\\u0040", "@"),
        ("\\u0025", "%"),
        ("\\u002b", "+"),
    ] {
        s = repeated_replace_all(&s, old, new);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding_turns_escapes_back() {
        assert_eq!(url_encoding(" https://x.org/dl?id=1\\u0026key=a\\u003db "), "https://x.org/dl?id=1&key=a=b");
    }
}
