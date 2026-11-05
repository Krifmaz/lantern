//! What the desktop app remembers between starts, in one JSON file in the user's config folder
//! (`<config dir>/<project>/desktop.json`: `~/.config/…` on Linux, `%APPDATA%\…` on Windows,
//! `~/Library/Application Support/…` on macOS): the server's address, this computer's app token and id, the
//! look, and whether to look for updates at start. On Unix the file is readable by its owner only.
//!
//! `<PROJECT>_SERVER` in the environment (`NOTES_SERVER=https://notes.example.com` for `notes`) overrides the
//! remembered address, for a managed install or a test run.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What is remembered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stored {
    /// The server's base URL (`https://notes.example.com`), "" before the first connect.
    pub server: String,
    /// The app token of the signed-in account, "" when signed out.
    pub token: String,
    /// This install's id, sent with the sign-in so the account's sessions tell computers apart.
    pub device_id: String,
    /// The look's theme id (one of kit-iced's `theme::THEMES`; the first is the default).
    pub theme: String,
    /// Look for a new version at start.
    pub check_updates: bool,
}

impl Default for Stored {
    fn default() -> Self {
        Stored {
            server: String::new(),
            token: String::new(),
            device_id: String::new(),
            theme: kit_iced::theme::THEMES[0].0.into(),
            check_updates: true,
        }
    }
}

/// The file in the user's config folder; `None` when the system has none.
pub fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(crate::project()).join("desktop.json"))
}

/// What the file at `path` holds; the defaults when it is missing or unreadable (a new id is made then).
pub fn load_from(path: &Path) -> Stored {
    let mut s: Stored = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if s.device_id.is_empty() {
        s.device_id = new_device_id();
    }
    s
}

/// Writes `s` to `path` (making its folder), readable by its owner only on Unix; the error in words.
pub fn save_to(path: &Path, s: &Stored) -> Result<(), String> {
    let why = |e: std::io::Error| format!("The settings couldn't be saved to {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(why)?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    use std::io::Write;
    opts.open(path).and_then(|mut f| f.write_all(text.as_bytes())).map_err(why)
}

/// The environment variable that overrides the server's address: the project name in capitals, `-` as `_`,
/// then `_SERVER` (`MY_NOTES_SERVER` for `my-notes`).
pub fn env_key(project: &str) -> String {
    format!("{}_SERVER", project.to_ascii_uppercase().replace('-', "_"))
}

/// The address from [`env_key`], when it is set and not empty.
pub fn server_override() -> Option<String> {
    std::env::var(env_key(crate::project())).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A new random install id: 32 hex digits from the standard library's random hasher keys.
pub fn new_device_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kit-desktop-store-{}-{name}", std::process::id())).join("desktop.json")
    }

    #[test]
    fn a_missing_file_gives_the_defaults_with_a_new_id() {
        let s = load_from(&scratch("missing"));
        assert_eq!((s.server.as_str(), s.token.as_str(), s.theme.as_str(), s.check_updates), ("", "", kit_iced::theme::THEMES[0].0, true));
        assert_eq!(s.device_id.len(), 32);
        assert_ne!(s.device_id, new_device_id(), "every id is new");
    }

    #[test]
    fn what_is_saved_comes_back() {
        let p = scratch("round");
        let s = Stored { server: "https://notes.example.com".into(), token: "t".into(), check_updates: false, ..load_from(&p) };
        save_to(&p, &s).unwrap();
        assert_eq!(load_from(&p), s);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "only its owner reads the token");
        }
        // an older file without the newer keys keeps their defaults
        std::fs::write(&p, r#"{"server": "http://10.0.0.2:8080"}"#).unwrap();
        let old = load_from(&p);
        assert_eq!(
            (old.server.as_str(), old.check_updates, old.theme.as_str()),
            ("http://10.0.0.2:8080", true, kit_iced::theme::THEMES[0].0)
        );
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_override_is_named_after_the_project() {
        assert_eq!(env_key("my-notes"), "MY_NOTES_SERVER");
        assert_eq!(env_key("filebrary"), "FILEBRARY_SERVER");
    }
}
