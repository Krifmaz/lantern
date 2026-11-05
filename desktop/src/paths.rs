//! Every server path the desktop app calls, in one place: when the kit moves a route, this is the only file to
//! change. The server side is `server/src/lib.rs`; the shapes are the kit's contracts (`contracts/*.md`).

/// `GET`: `{ok, version, …}`, the server's health (the connect page's probe and the offline check).
pub const HEALTH: &str = "/api/health";
/// `POST {username, password, app: {device_id, device_name}}` → `{user, token}`: an app token for this computer.
pub const LOGIN: &str = "/api/auth/login";
/// `POST`: ends this computer's app token.
pub const LOGOUT: &str = "/api/auth/logout";
/// `GET`: the signed-in account (`is_admin`, `role`, `prefs`).
pub const ME: &str = "/api/auth/me";
/// `PUT {key: value}`: merges into the account's preferences (What's new's switch: `whats_new_popup`).
pub const PREFS: &str = "/api/auth/prefs";
/// `GET`: every release's notes, newest first (the What's new page).
pub const CHANGELOG: &str = "/api/changelog";
/// `GET`: whether the What's new popup shows, and the releases it lists.
pub const CHANGELOG_NEW: &str = "/api/changelog/new";
/// `POST`: this account has seen this version's What's new.
pub const CHANGELOG_SEEN: &str = "/api/changelog/seen";
/// `GET`: the data folder notice (one notice object).
pub const STORAGE: &str = "/api/notices/data";
/// `POST` (admins): checks the data folder again and answers the notice.
pub const STORAGE_CHECK: &str = "/api/notices/data/check";
/// `GET`: the reports this account may see; `POST` multipart: a new report.
pub const BUG_REPORTS: &str = "/api/bug-reports";
/// `GET`: the server's checks (admins).
pub const DIAGNOSTICS: &str = "/api/diagnostics";
/// `GET`: the settings' values; `PUT`: saves some (admins).
pub const SETTINGS: &str = "/api/settings";
/// `GET`: the settings form's schema (admins).
pub const SETTINGS_SCHEMA: &str = "/api/settings/schema";

/// `GET`: one report with its thread.
pub fn bug_report(id: i64) -> String {
    format!("{BUG_REPORTS}/{id}")
}

/// `POST` multipart: a reply in a report's thread.
pub fn bug_report_messages(id: i64) -> String {
    format!("{BUG_REPORTS}/{id}/messages")
}

/// `GET` (the server's owner): a saved secret in clear.
pub fn settings_secret(key: &str) -> String {
    format!("{SETTINGS}/secret/{key}")
}

/// `POST` (admins): runs a settings card's test.
pub fn settings_test(test: &str) -> String {
    format!("{DIAGNOSTICS}/{test}/test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_with_an_id() {
        assert_eq!(bug_report(12), "/api/bug-reports/12");
        assert_eq!(bug_report_messages(12), "/api/bug-reports/12/messages");
        assert_eq!(settings_secret("api_key"), "/api/settings/secret/api_key");
        assert_eq!(settings_test("smtp"), "/api/diagnostics/smtp/test");
    }
}
