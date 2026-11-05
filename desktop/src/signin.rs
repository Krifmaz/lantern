//! Signing in with the kit's rules, as the phone app does (`android/…/SignInRules.kt`): the request body (an app
//! token for this computer), the answer read into a [`LoginResult`], this computer's name and the words. Pure
//! functions; [`crate::client::Client::login`] sends it.

use serde_json::{json, Value};

/// The page's title ("Sign in to Notes").
pub fn title(app: &str) -> String {
    format!("Sign in to {app}")
}

/// The line above the code field when the account has two-step sign-in.
pub const CODE_PROMPT: &str = "Enter the 6-digit code from your authenticator app";
/// Shown when the sign-in failed without the server's words.
pub const FAILED: &str = "Sign-in failed";

/// What `POST /api/auth/login` came to.
#[derive(Clone, Debug, PartialEq)]
pub enum LoginResult {
    /// Signed in: `token` goes with every later request as `Authorization: Bearer`; `user` is the account.
    Success {
        /// The app token.
        token: String,
        /// The account (`/api/auth/me`'s shape).
        user: Value,
    },
    /// The account has two-step sign-in: ask for the authenticator code (the server's words).
    NeedsCode(String),
    /// Not signed in; shown in full (the server's `detail`, e.g. "Invalid username or password").
    Failed(String),
}

/// The body of `POST /api/auth/login`: the trimmed `username`, the `password`, the authenticator `code` without
/// spaces (left out when empty) and this computer (`app`), which asks the server for a long-lived app token.
pub fn body(username: &str, password: &str, code: Option<&str>, device_id: &str, device_name: &str) -> Value {
    let mut b = json!({
        "username": username.trim(),
        "password": password,
        "app": { "device_id": device_id, "device_name": device_name },
    });
    if let Some(c) = code.map(|c| c.replace(' ', "")).filter(|c| !c.is_empty()) {
        b["code"] = json!(c);
    }
    b
}

/// The answer: a token on success (`{"user": {…}, "token": "…"}`); a 401 with `two_factor` asks for the code;
/// anything else fails with the server's `detail` (or "Sign-in failed (HTTP <status>)").
pub fn result(status: u16, text: &str) -> LoginResult {
    let o: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    if !(200..300).contains(&status) {
        let detail = Some(s("detail")).filter(|d| !d.is_empty()).unwrap_or_else(|| format!("{FAILED} (HTTP {status})"));
        let two_factor = o.get("two_factor").and_then(Value::as_bool) == Some(true);
        return if status == 401 && two_factor { LoginResult::NeedsCode(detail) } else { LoginResult::Failed(detail) };
    }
    let token = s("token");
    if token.is_empty() {
        return LoginResult::Failed("The server didn't start a session".into());
    }
    LoginResult::Success { token, user: o.get("user").cloned().unwrap_or(Value::Null) }
}

/// This computer's name as the account's sessions list shows it: the host name and the system ("studio (Linux)"),
/// the system alone when the host name is unknown.
pub fn device_name(host: &str, os: &str) -> String {
    let os = match os {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    let host = host.trim();
    if host.is_empty() {
        os.to_string()
    } else {
        format!("{host} ({os})")
    }
}

/// This computer's host name from the environment (`HOSTNAME`, `COMPUTERNAME`), else `/etc/hostname`.
pub fn host_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .unwrap_or_default()
}

/// Whether Sign in can be pressed: a username and a password, and a code when the server asked for one.
pub fn ready(username: &str, password: &str, code: Option<&str>) -> bool {
    !username.trim().is_empty() && !password.is_empty() && code.is_none_or(|c| !c.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_asks_for_an_app_token() {
        assert_eq!(
            body(" ana ", "secret ", Some("123 456"), "d-1", "studio (Linux)"),
            json!({"username": "ana", "password": "secret ", "code": "123456", "app": {"device_id": "d-1", "device_name": "studio (Linux)"}})
        );
        assert!(body("ana", "pw", Some(" "), "d-1", "X").get("code").is_none());
        assert!(body("ana", "pw", None, "d-1", "X").get("code").is_none());
    }

    #[test]
    fn a_token_signs_in() {
        assert_eq!(
            result(200, r#"{"user":{"id":1,"username":"ana"},"token":"t-1"}"#),
            LoginResult::Success { token: "t-1".into(), user: json!({"id": 1, "username": "ana"}) }
        );
        assert_eq!(result(200, r#"{"user":{"id":1}}"#), LoginResult::Failed("The server didn't start a session".into()));
    }

    #[test]
    fn failures_keep_the_servers_words() {
        assert_eq!(result(401, r#"{"detail":"Invalid username or password"}"#), LoginResult::Failed("Invalid username or password".into()));
        assert_eq!(result(502, "<html>Bad Gateway</html>"), LoginResult::Failed("Sign-in failed (HTTP 502)".into()));
        assert_eq!(result(401, r#"{"detail":"Enter your code","two_factor":true}"#), LoginResult::NeedsCode("Enter your code".into()));
    }

    #[test]
    fn the_computers_name() {
        assert_eq!(device_name("studio", "linux"), "studio (Linux)");
        assert_eq!(device_name(" ", "macos"), "macOS");
        assert_eq!(device_name("PC-1", "windows"), "PC-1 (Windows)");
    }

    #[test]
    fn sign_in_needs_both_fields_and_the_code_when_asked() {
        assert!(ready("ana", "pw", None));
        assert!(!ready(" ", "pw", None));
        assert!(!ready("ana", "", None));
        assert!(!ready("ana", "pw", Some(" ")));
        assert!(ready("ana", "pw", Some("123456")));
        assert_eq!(title("Filebrary"), "Sign in to Filebrary");
    }
}
