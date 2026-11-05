//! The server's address as a person types it, turned into the base URL the app calls: the same rules and words
//! as the phone app's connect page (`android/…/ServerAddress.kt`). Pure functions; [`crate::client::probe`] does
//! the asking.

use reqwest::Url;

/// Shown when the field is empty.
pub const EMPTY: &str = "Enter your server's address";
/// Shown when the text can't be a web address.
pub const INVALID: &str = "That isn't a valid address";
/// Shown when nothing answered.
pub const UNREACHABLE: &str = "Can't reach that server";

/// The addresses to try, in order, for what was typed: as it is when it starts with `http://` or `https://`,
/// otherwise `https://` first and then `http://` (a server on the LAN is often plain http). Spaces around it and
/// trailing slashes are dropped; nothing to try for an empty field.
pub fn candidates(input: &str) -> Vec<String> {
    let raw = input.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Vec::new();
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        vec![raw.to_string()]
    } else {
        vec![format!("https://{raw}"), format!("http://{raw}")]
    }
}

/// The URL form of `candidate` without a trailing slash (lower-case host, no default port), so the requests the
/// app makes later match it; `None` when it isn't an http(s) address.
pub fn normalise(candidate: &str) -> Option<String> {
    let url = Url::parse(candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some(url.to_string().trim_end_matches('/').to_string())
}

/// Whether a `/api/health` answer comes from a server: it says `ok`, or names its version.
pub fn is_server(ok: bool, version: &str) -> bool {
    ok || !version.is_empty()
}

/// Shown when something answered but isn't this app's server ("That isn't an Orbit server").
pub fn not_server(app: &str) -> String {
    let an = app.chars().next().is_some_and(|c| "aeiou".contains(c.to_ascii_lowercase()));
    format!("That isn't {} {app} server", if an { "an" } else { "a" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_scheme_https_comes_first() {
        assert_eq!(candidates("  files.example.com/ "), ["https://files.example.com", "http://files.example.com"]);
        assert_eq!(candidates("192.168.1.10:8080"), ["https://192.168.1.10:8080", "http://192.168.1.10:8080"]);
    }

    #[test]
    fn a_typed_scheme_is_kept() {
        assert_eq!(candidates("http://10.0.2.2:8080//"), ["http://10.0.2.2:8080"]);
        assert_eq!(candidates("HTTPS://Files.example.com"), ["HTTPS://Files.example.com"]);
    }

    #[test]
    fn an_empty_field_has_nothing_to_try() {
        assert!(candidates("   ").is_empty());
        assert!(candidates("/").is_empty());
    }

    #[test]
    fn addresses_are_normalised() {
        assert_eq!(normalise("HTTPS://Files.Example.com:443").as_deref(), Some("https://files.example.com"));
        assert_eq!(normalise("http://10.0.2.2:8080").as_deref(), Some("http://10.0.2.2:8080"));
        assert_eq!(normalise("https://example.com/files/").as_deref(), Some("https://example.com/files"));
        assert_eq!(normalise("https://exa mple.com"), None);
        assert_eq!(normalise("ftp://example.com"), None);
    }

    #[test]
    fn a_server_says_ok_or_its_version() {
        assert!(is_server(true, ""));
        assert!(is_server(false, "0.1.0"));
        assert!(!is_server(false, ""));
    }

    #[test]
    fn the_words() {
        assert_eq!(not_server("Orbit"), "That isn't an Orbit server");
        assert_eq!(not_server("Filebrary"), "That isn't a Filebrary server");
        assert_eq!((EMPTY, INVALID, UNREACHABLE), ("Enter your server's address", "That isn't a valid address", "Can't reach that server"));
    }
}
