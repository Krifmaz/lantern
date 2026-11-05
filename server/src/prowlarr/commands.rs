// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Commands/CommandController.cs, CommandResource.cs,
// src/NzbDrone.Core/Messaging/Commands/Command.cs, CommandQueue.cs, CommandQueueManager.cs, CommandExecutor.cs,
// CommandEqualityComparer.cs, CommandPriorityComparer.cs, CommandRepository.cs, TestCommand*.cs, and the commands
// below), GPL-3.0.

//! Prowlarr's commands (`/api/v1/command`): a request to do something now (check health, update the indexer
//! definitions, reset the API key, …), queued, run in the background and followed by its status, as in Prowlarr.
//!
//! The queue follows Prowlarr's rules: the same command with the same settings is not queued twice, up to four run
//! at once, an exclusive command runs alone, and only a queued command can be cancelled. Finished commands stay in
//! the list for five minutes and in the database for a day. Commands Lantern can't run yet are refused the way
//! Prowlarr refuses a command it doesn't know.

use super::{body_json, created, date, fail, not_found, split_camel_case, system, ApiResult, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Map, Value};

/// How many commands run at once (Prowlarr's three threads, plus one).
const RUNNING_AT_ONCE: usize = 4;

/// Keeps two requests from starting the same queued command.
static PICK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A command Lantern can run, with what Prowlarr's class of it says.
#[derive(Debug, Clone, Copy)]
pub struct Kind {
    /// Its name (`CheckHealth`; Prowlarr's class name without `Command`).
    pub name: &'static str,
    /// Whether the web app follows it as it runs.
    pub send_updates_to_client: bool,
    /// The message it ends with, when it has a fixed one.
    pub completion_message: Option<&'static str>,
    /// Whether it runs alone.
    pub is_exclusive: bool,
    /// Its own settings and their defaults (`duration` for Test).
    pub extra: &'static [(&'static str, i64)],
    /// Its own yes-or-no settings and their defaults (`forceSync` for ApplicationIndexerSync).
    pub flags: &'static [(&'static str, bool)],
}

/// The commands Lantern runs, as Prowlarr defines them.
pub const KINDS: [Kind; 11] = [
    Kind { name: "MessagingCleanup", send_updates_to_client: false, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
    Kind { name: "CheckHealth", send_updates_to_client: false, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
    Kind {
        name: "IndexerDefinitionUpdate",
        send_updates_to_client: true,
        completion_message: None,
        is_exclusive: false,
        extra: &[],
        flags: &[],
    },
    Kind { name: "ResetApiKey", send_updates_to_client: true, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
    Kind { name: "ClearLog", send_updates_to_client: true, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
    Kind { name: "DeleteLogFiles", send_updates_to_client: true, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
    Kind {
        name: "DeleteUpdateLogFiles",
        send_updates_to_client: true,
        completion_message: None,
        is_exclusive: false,
        extra: &[],
        flags: &[],
    },
    Kind {
        name: "Test",
        send_updates_to_client: true,
        completion_message: None,
        is_exclusive: false,
        extra: &[("duration", 4000)],
        flags: &[],
    },
    Kind {
        name: "ApplicationIndexerSync",
        send_updates_to_client: true,
        completion_message: Some("Completed"),
        is_exclusive: false,
        extra: &[],
        flags: &[("forceSync", false)],
    },
    Kind { name: "CleanUpHistory", send_updates_to_client: false, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
    Kind { name: "ClearHistory", send_updates_to_client: true, completion_message: None, is_exclusive: false, extra: &[], flags: &[] },
];

/// Prowlarr's `CommandStatus`, by number.
const STATUSES: [&str; 7] = ["queued", "started", "completed", "failed", "aborted", "cancelled", "orphaned"];

/// The command routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/command", get(all).post(start)).route("/api/v1/command/{id}", get(one).delete(cancel))
}

/// The commands' table at start: ones that were running when Lantern stopped are orphaned and queued ones go
/// back in the queue, as Prowlarr does when it starts.
pub fn at_start(db: &SqliteDb) -> kit_server_core::Result<()> {
    db.execute("UPDATE prowlarr_commands SET status=6, ended_at=? WHERE status=1", p![crate::auth::now()])?;
    db.execute("UPDATE prowlarr_commands SET in_queue=1 WHERE status=0", p![])?;
    Ok(())
}

/// The command `name` names, in any case.
fn kind_of(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.name.eq_ignore_ascii_case(name))
}

/// A command as Prowlarr's `CommandResource`.
pub fn resource(r: &kit_server_core::Row) -> Value {
    let body: Value = serde_json::from_str(&r.s("body")).unwrap_or_else(|_| json!({}));
    let opt_date = |k: &str| r.get(k).and_then(Value::as_f64).map(date);
    let name = r.s("name");
    let mut v = json!({
        "id": r.i("id"),
        "name": name,
        "commandName": split_camel_case(&name),
        "body": body,
        "priority": match r.i("priority") { -1 => "low", 1 => "high", _ => "normal" },
        "status": STATUSES.get(r.i("status") as usize).copied().unwrap_or("queued"),
        "queued": date(r.f("queued_at")),
        "trigger": match r.i("trigger") { 1 => "manual", 2 => "scheduled", _ => "unspecified" },
        "sendUpdatesToClient": body["sendUpdatesToClient"].as_bool().unwrap_or(false),
        "updateScheduledTask": body["updateScheduledTask"].as_bool().unwrap_or(false),
    });
    let mut put = |k: &str, x: Option<Value>| {
        if let Some(x) = x.filter(|x| !x.is_null()) {
            v[k] = x;
        }
    };
    put("message", r.get("message").cloned());
    put("started", opt_date("started_at").map(Value::from));
    put("ended", opt_date("ended_at").map(Value::from));
    put("duration", r.get("duration").and_then(Value::as_f64).map(|d| json!(super::timespan(d))));
    put("exception", r.get("exception").cloned());
    // Prowlarr leaves out a browser's user agent
    put("clientUserAgent", body.get("clientUserAgent").filter(|u| !u.as_str().unwrap_or("").starts_with("Mozilla/5.0")).cloned());
    put("stateChangeTime", opt_date("started_at").or_else(|| opt_date("ended_at")).map(Value::from));
    put("lastExecutionTime", body.get("lastExecutionTime").cloned());
    v
}

/// The body Prowlarr keeps for a command: the `Command` class' fields, then the command's own.
fn command_body(kind: &Kind, asked: &Map<String, Value>, user_agent: Option<String>) -> Value {
    let get = |k: &str| asked.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v);
    let asked_updates = get("sendUpdatesToClient").and_then(Value::as_bool).unwrap_or(false);
    let mut body = json!({
        // Prowlarr's controller turns updates on for every command started through the API
        "sendUpdatesToClient": true,
        "updateScheduledTask": true,
        "requiresDiskAccess": false,
        "isExclusive": kind.is_exclusive,
        "isTypeExclusive": false,
        "name": kind.name,
        "trigger": "manual",
        "suppressMessages": !(asked_updates || kind.send_updates_to_client),
    });
    if let Some(m) = kind.completion_message {
        body["completionMessage"] = json!(m);
    }
    if let Some(ua) = user_agent {
        body["clientUserAgent"] = json!(ua);
    }
    for (k, default) in kind.extra {
        body[*k] = get(k).and_then(Value::as_i64).map(Value::from).unwrap_or_else(|| json!(default));
    }
    for (k, default) in kind.flags {
        body[*k] = json!(get(k).and_then(Value::as_bool).unwrap_or(*default));
    }
    body
}

/// Whether two commands' own settings are the same (Prowlarr's `CommandEqualityComparer`, which leaves out the
/// fields every command has).
fn same_settings(kind: &Kind, a: &Value, b: &Value) -> bool {
    kind.extra.iter().map(|(k, _)| *k).chain(kind.flags.iter().map(|(k, _)| *k)).all(|k| a.get(k) == b.get(k))
}

/// Queues a command (or answers the same one already queued or running) and starts what can start; its id.
pub fn push(st: &AppState, kind: &Kind, body: Value, priority: i64, trigger: i64) -> kit_server_core::Result<i64> {
    let waiting = st.db.query("SELECT id, body FROM prowlarr_commands WHERE in_queue=1 AND status IN (0,1) AND name=?", p![kind.name])?;
    if let Some(r) = waiting.iter().find(|r| same_settings(kind, &serde_json::from_str(&r.s("body")).unwrap_or_default(), &body)) {
        return Ok(r.i("id"));
    }
    let id = st.db.execute(
        "INSERT INTO prowlarr_commands (name, body, priority, status, queued_at, trigger, in_queue) VALUES (?,?,?,0,?,?,1)",
        p![kind.name, body.to_string(), priority, crate::auth::now(), trigger],
    )?;
    pump(st.clone());
    Ok(id)
}

/// Starts every queued command that may start now, by Prowlarr's `CommandQueue.TryGet` rules.
pub fn pump(st: AppState) {
    tokio::spawn(async move {
        let _guard = PICK.lock().await;
        loop {
            let Ok(rows) = st.db.query("SELECT * FROM prowlarr_commands WHERE in_queue=1 AND status IN (0,1) ORDER BY id", p![]) else {
                return;
            };
            let started: Vec<_> = rows.iter().filter(|r| r.i("status") == 1).collect();
            if started.len() >= RUNNING_AT_ONCE {
                return;
            }
            let exclusive = |r: &kit_server_core::Row| kind_of(&r.s("name")).is_some_and(|k| k.is_exclusive);
            let mut queued: Vec<_> = rows.iter().filter(|r| r.i("status") == 0).collect();
            queued.sort_by(|a, b| b.i("priority").cmp(&a.i("priority")).then(a.f("queued_at").total_cmp(&b.f("queued_at"))));
            let Some(next) = queued.first() else { return };
            if started.iter().any(|r| exclusive(r)) || (exclusive(next) && !started.is_empty()) {
                return;
            }
            let id = next.i("id");
            let now = crate::auth::now();
            if st.db.execute("UPDATE prowlarr_commands SET status=1, started_at=? WHERE id=? AND status=0", p![now, id]).is_err() {
                return;
            }
            let (name, body) = (next.s("name"), serde_json::from_str::<Value>(&next.s("body")).unwrap_or_default());
            notify(&st, id);
            let st2 = st.clone();
            tokio::spawn(async move {
                let result = execute(&st2, &name, &body, id).await;
                let ended = crate::auth::now();
                let (status, message, exception) = match result {
                    Ok(m) => (2, kind_of(&name).and_then(|k| k.completion_message).map(str::to_string).or(m), None),
                    Err(e) => (3, Some("Failed".to_string()), Some(e)),
                };
                let _ = st2.db.execute(
                    "UPDATE prowlarr_commands SET status=?, ended_at=?, duration=?, message=?, exception=? WHERE id=?",
                    p![status, ended, ended - now, message, exception, id],
                );
                if body["updateScheduledTask"].as_bool().unwrap_or(false) {
                    system::task_ran(&st2.db, &name, now, ended);
                }
                notify(&st2, id);
                pump(st2);
            });
        }
    });
}

/// Tells the connected web apps how command `id` stands, when it sends updates to them
/// (`CommandController.Handle(CommandUpdatedEvent)`).
fn notify(st: &AppState, id: i64) {
    let Ok(Some(r)) = st.db.one("SELECT * FROM prowlarr_commands WHERE id=?", p![id]) else { return };
    let v = resource(&r);
    if v["sendUpdatesToClient"] == true {
        super::signalr::broadcast(st, "command", "updated", Some(&v));
    }
}

/// Saves a command's progress message (Prowlarr's `ProgressInfo`).
fn progress(db: &SqliteDb, id: i64, message: &str) {
    let _ = db.execute("UPDATE prowlarr_commands SET message=? WHERE id=?", p![message, id]);
}

/// Runs one command; the message it ended with, or why it failed.
async fn execute(st: &AppState, name: &str, body: &Value, id: i64) -> Result<Option<String>, String> {
    let last = |db: &SqliteDb| {
        db.one("SELECT message FROM prowlarr_commands WHERE id=?", p![id])
            .ok()
            .flatten()
            .and_then(|r| r.get("message").and_then(|m| m.as_str().map(str::to_string)))
    };
    match name {
        "MessagingCleanup" => {
            // finished commands leave the list after five minutes (when it is read) and the database after a day
            let day_ago = crate::auth::now() - 24.0 * 3600.0;
            st.db
                .execute("DELETE FROM prowlarr_commands WHERE ended_at IS NOT NULL AND ended_at < ?", p![day_ago])
                .map_err(|e| e.detail)?;
        }
        "CheckHealth" => {
            super::health::results(st).await.map_err(|e| e.detail)?;
        }
        "IndexerDefinitionUpdate" => {
            let source = st.settings.str("definitions_source");
            let source = if source.is_empty() { crate::indexers::library::DEFAULT_SOURCE.to_string() } else { source };
            st.indexers.library.update(&source).await?;
        }
        "ResetApiKey" => {
            if st.settings.env_locked("api_key") {
                return Err("The API key is set by APP_API_KEY, so it can't be reset here. Change APP_API_KEY instead.".into());
            }
            let key = {
                use rand::Rng;
                let mut b = [0u8; 16];
                rand::rng().fill(&mut b[..]);
                hex::encode(b)
            };
            st.settings.set_many(&json!({ "api_key": key })).await.map_err(|e| e.detail)?;
        }
        "ClearLog" => {
            st.db.execute("DELETE FROM prowlarr_logs", p![]).map_err(|e| e.detail)?;
        }
        "DeleteLogFiles" => empty_folder(&super::logs::log_folder(st))?,
        "DeleteUpdateLogFiles" => empty_folder(&super::logs::update_log_folder(st))?,
        "Test" => {
            let ms = body["duration"].as_i64().unwrap_or(4000).max(0) as u64;
            progress(&st.db, id, &format!("Starting Test command. duration {ms}"));
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            progress(&st.db, id, "Completed Test command");
        }
        "ApplicationIndexerSync" => {
            super::apps::sync::sync_all(st, body["forceSync"].as_bool().unwrap_or(false)).await;
        }
        "CleanUpHistory" => {
            let days = super::config_get(&st.db, "historycleanupdays").and_then(|d| d.parse().ok()).unwrap_or(30);
            super::history::clean_up(&st.db, days).map_err(|e| e.detail)?;
        }
        "ClearHistory" => {
            super::history::clear(&st.db).map_err(|e| e.detail)?;
        }
        _ => {}
    }
    Ok(last(&st.db))
}

/// Deletes everything in `folder` (Prowlarr's `EmptyFolder`); nothing to do when it doesn't exist.
fn empty_folder(folder: &std::path::Path) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Ok(()) };
    for e in entries.flatten() {
        let p = e.path();
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        r.map_err(|err| format!("{} can't be deleted: {err}", p.display()))?;
    }
    Ok(())
}

/// `GET /api/v1/command`: the commands in the queue (finished ones for five minutes), running first, then by
/// status, then by priority.
async fn all(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    let cutoff = crate::auth::now() - 5.0 * 60.0;
    let mut rows =
        st.db.query("SELECT * FROM prowlarr_commands WHERE in_queue=1 AND (ended_at IS NULL OR ended_at >= ?) ORDER BY id", p![cutoff])?;
    let rank = |s: i64| if s == 1 { -1 } else { s };
    rows.sort_by(|a, b| rank(a.i("status")).cmp(&rank(b.i("status"))).then(b.i("priority").cmp(&a.i("priority"))));
    Ok(Json(Value::Array(rows.iter().map(resource).collect())))
}

/// `GET /api/v1/command/{id}`.
async fn one(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let r = st.db.one("SELECT * FROM prowlarr_commands WHERE id=?", p![id])?.ok_or_else(|| not_found("CommandModel", id))?;
    Ok(Json(resource(&r)))
}

/// `POST /api/v1/command` `{"name": "CheckHealth", …}`: queues the command; 201 with it.
async fn start(State(st): State<AppState>, _: V1, headers: HeaderMap, bytes: Bytes) -> ApiResult<Response> {
    let asked = match body_json(&bytes)? {
        Value::Object(m) => m,
        _ => return Err(fail(400, "The request body must be a JSON object")),
    };
    let name = asked.iter().find(|(k, _)| k.eq_ignore_ascii_case("name")).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
    // Prowlarr's NotBlank: not null, and not empty
    super::check(vec![super::validation::not_null("Name", &name), super::validation::not_empty("Name", name.as_str())])?;
    // a name Prowlarr's controller can't match fails as it does there
    let kind = kind_of(name.as_str().unwrap_or("")).ok_or_else(|| fail(500, "Sequence contains no matching element"))?;
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = command_body(kind, &asked, ua);
    let id = push(&st, kind, body, 0, 1)?;
    let r = st.db.one("SELECT * FROM prowlarr_commands WHERE id=?", p![id])?.ok_or_else(|| not_found("CommandModel", id))?;
    Ok(created(format!("/api/v1/command/{id}"), resource(&r)))
}

/// `DELETE /api/v1/command/{id}`: takes a queued command out of the queue; 409 `Unable to cancel task` for one
/// that already started, finished or isn't there.
async fn cancel(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Response> {
    super::validate_id(id)?;
    let _guard = PICK.lock().await;
    let queued = st.db.one("SELECT id FROM prowlarr_commands WHERE id=? AND in_queue=1 AND status=0", p![id])?.is_some();
    if !queued {
        return Err(fail(409, "Unable to cancel task"));
    }
    st.db.execute("UPDATE prowlarr_commands SET in_queue=0 WHERE id=?", p![id])?;
    Ok(StatusCode::OK.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_carry_prowlarrs_command_fields() {
        let test = kind_of("test").unwrap();
        let b = command_body(test, &Map::new(), Some("curl/8".into()));
        assert_eq!(b["name"], "Test");
        assert_eq!(b["duration"], 4000);
        assert_eq!(b["sendUpdatesToClient"], true);
        assert_eq!(b["suppressMessages"], false);
        assert_eq!(b["clientUserAgent"], "curl/8");
        let health = kind_of("CheckHealth").unwrap();
        let b = command_body(health, &Map::new(), None);
        // asked without updates: Prowlarr suppresses messages but still sends updates to the client
        assert_eq!((b["sendUpdatesToClient"].as_bool(), b["suppressMessages"].as_bool()), (Some(true), Some(true)));
        assert!(b.get("clientUserAgent").is_none());
    }

    #[test]
    fn same_settings_compare_only_the_commands_own_fields() {
        let test = kind_of("Test").unwrap();
        assert!(same_settings(test, &json!({ "duration": 5, "trigger": "manual" }), &json!({ "duration": 5, "trigger": "scheduled" })));
        assert!(!same_settings(test, &json!({ "duration": 5 }), &json!({ "duration": 6 })));
    }
}
