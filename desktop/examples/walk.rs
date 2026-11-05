//! Walks the desktop app's requests against a running server, the way the app makes them: the connect page's
//! probe, the sign-in for an app token, then every call the pages make (What's new, the notice, a bug report
//! with a file and a reply, the settings schema and values, the checks) with that token. The kit's CI runs it
//! after starting the generated app's server (`scripts/template-check.sh`).
//!
//! ```sh
//! cargo run --example walk -- http://127.0.0.1:8080 admin <password>
//! ```

use desktop::client::{self, Client};
use desktop::{paths, signin};
use kit_iced::bug_report::FILE_FIELD;
use serde_json::{json, Value};

/// One step: its name and whether it worked, or the run stops with the error.
fn step<T, E: std::fmt::Display>(name: &str, r: Result<T, E>) -> T {
    match r {
        Ok(v) => {
            println!("ok   {name}");
            v
        }
        Err(e) => {
            eprintln!("FAIL {name}: {e}");
            std::process::exit(1);
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [address, username, password] = &args[..] else {
        eprintln!("usage: walk <server address> <username> <password>");
        std::process::exit(2);
    };
    let base = step("the connect page finds the server", client::probe(address.clone(), desktop::title()).await);
    let anon = Client::new(&base, "");
    let body = signin::body(username, "wrong password", None, "walk", "walk (CI)");
    let wrong = step("a wrong password is turned away", anon.login(body).await.map_err(|e| e.message));
    assert!(matches!(wrong, signin::LoginResult::Failed(ref w) if w == "Invalid username or password"), "{wrong:?}");
    let body = signin::body(username, password, None, &desktop::store::new_device_id(), "walk (CI)");
    let token = match step("sign in for an app token", anon.login(body).await.map_err(|e| e.message)) {
        signin::LoginResult::Success { token, .. } => token,
        other => step::<String, String>("sign in for an app token", Err(format!("{other:?}"))),
    };
    let c = Client::new(&base, &token);
    let me = step("who is signed in", c.get(paths::ME.into()).await);
    assert_eq!(me["is_admin"], json!(true), "{me}");
    let health = step("the health check", c.health().await);
    assert!(health["version"].as_str().is_some_and(|v| !v.is_empty()), "{health}");
    step("What's new: is there anything new", c.get(paths::CHANGELOG_NEW.into()).await);
    step("What's new: seen", c.post(paths::CHANGELOG_SEEN.into(), json!({})).await);
    step("What's new: the switch", c.put(paths::PREFS.into(), json!({ "whats_new_popup": true })).await);
    step("What's new: every release", c.get(paths::CHANGELOG.into()).await);
    step("the data folder notice", c.get(paths::STORAGE.into()).await);
    step("Check again", c.post(paths::STORAGE_CHECK.into(), json!({})).await);

    let log = std::env::temp_dir().join(format!("walk-{}.log", std::process::id()));
    std::fs::write(&log, "a line of the app's log\n").expect("the temporary folder is writable");
    let fields: Vec<(String, String)> = [
        ("kind", "bug"),
        ("title", "Walk"),
        ("body", "Sent by the desktop walk"),
        ("page", "#/"),
        ("app_version", desktop::VERSION),
        ("device", "walk (CI)"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let sent = step("a new bug report with a file", c.multipart(paths::BUG_REPORTS.into(), fields, FILE_FIELD, vec![log.clone()]).await);
    let id = sent["id"].as_i64().expect("the new report's id");
    let list = step("the bug report list", c.get(paths::BUG_REPORTS.into()).await);
    assert!(list["reports"].as_array().is_some_and(|l| l.iter().any(|r| r["id"] == json!(id))), "{list}");
    step("a reply", c.multipart(paths::bug_report_messages(id), vec![("body".into(), "And a reply".into())], FILE_FIELD, vec![]).await);
    let report = step("the report with its thread", c.get(paths::bug_report(id)).await);
    assert_eq!(report["report"]["files"].as_array().map(Vec::len), Some(1), "{report}");
    assert_eq!(report["messages"].as_array().map(Vec::len), Some(1), "{report}");
    let _ = std::fs::remove_file(&log);

    let schema = step("the settings schema", c.get(paths::SETTINGS_SCHEMA.into()).await);
    step("the schema draws as a form", kit_iced::form::Schema::from_json(&schema));
    let values: Value = step("the settings' values", c.get(paths::SETTINGS.into()).await);
    assert!(values.is_object(), "{values}");
    let checks = step("Diagnostics' checks", c.get(paths::DIAGNOSTICS.into()).await);
    assert!(!kit_iced::diagnostics::checks(&checks).is_empty(), "{checks}");
    step("sign out", c.post(paths::LOGOUT.into(), json!({})).await);
    let gone = c.get(paths::ME.into()).await.expect_err("the token is gone");
    assert!(gone.signed_out(), "{gone:?}");
    println!("The desktop app's requests all work against {base}.");
}
