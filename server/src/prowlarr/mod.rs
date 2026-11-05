// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Http/Authentication/ApiKeyAuthenticationHandler.cs,
// src/Prowlarr.Http/ErrorManagement/ProwlarrErrorPipeline.cs, src/Prowlarr.Http/REST/RestController.cs), GPL-3.0.

//! Prowlarr's v1 API, so Prowlarr's own web app and the tools that talk to Prowlarr can use Lantern. Every answer
//! keeps Prowlarr's field names, casing, defaults, status codes and error texts.
//!
//! ```text
//! GET/HEAD /ping                                     {"status": "OK"}, no sign-in needed
//! GET      /api/v1/system/status                     what is running (SystemResource)
//! GET      /api/v1/system/routes                     every route, as text
//! GET      /api/v1/system/task, /system/task/{id}    the scheduled tasks
//! GET      /api/v1/system/backup                     the backups in the backup folder
//! GET/PUT  /api/v1/config/host[/{id}]                general settings (HostConfigResource)
//! GET/PUT  /api/v1/config/ui[/{id}]                  UI settings (UiConfigResource)
//! GET/PUT  /api/v1/config/development[/{id}]         development settings (DevelopmentConfigResource)
//! *        /api/v1/tag[/{id}], GET /api/v1/tag/detail[/{id}]
//! *        /api/v1/customfilter[/{id}]
//! GET      /api/v1/localization, /api/v1/localization/options
//! GET      /api/v1/health
//! GET/POST /api/v1/command, GET/DELETE /api/v1/command/{id}
//! GET      /api/v1/log, /api/v1/log/file[/{name}], /api/v1/log/file/update[/{name}]
//! GET      /api/v1/update
//! *        /api/v1/applications[/{id}], /schema, /test, /testall, /bulk, /action/{name}
//! *        /api/v1/appprofile[/{id}], /schema
//! ```
//!
//! Every `/api/v1` route needs the API key (`?apikey=`, `X-Api-Key` or `Authorization: Bearer <key>`, as Prowlarr
//! reads them) or a signed-in Lantern session.

pub mod apps;
pub mod commands;
pub mod config;
pub mod filters;
pub mod health;
pub mod localization;
pub mod logs;
pub mod system;
pub mod tags;
pub mod validation;

use crate::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use kit_server_core::{err, p, Db, RowExt, SqliteDb};
use serde_json::{json, Value};
use validation::Failure;

/// The application's name where Prowlarr says "Prowlarr" (`appName`, `{appName}` in its texts).
pub const APP_NAME: &str = "Lantern";

/// The tables this API keeps: Prowlarr's `Config` (key and value), `Tags`, `CustomFilters`, `Commands`,
/// `ScheduledTasks` and `Logs`, with Prowlarr's columns.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS prowlarr_config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS prowlarr_tags (id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL UNIQUE);
        CREATE TABLE IF NOT EXISTS prowlarr_custom_filters (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            type TEXT NOT NULL,
            label TEXT NOT NULL,
            filters TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS prowlarr_commands (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            body TEXT NOT NULL,
            priority INTEGER NOT NULL,
            status INTEGER NOT NULL,
            queued_at REAL NOT NULL,
            started_at REAL,
            ended_at REAL,
            duration REAL,
            exception TEXT,
            trigger INTEGER NOT NULL,
            message TEXT,
            in_queue INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS prowlarr_tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            type_name TEXT NOT NULL UNIQUE,
            interval INTEGER NOT NULL,
            last_execution REAL NOT NULL,
            last_start_time REAL
        );
        CREATE TABLE IF NOT EXISTS prowlarr_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            message TEXT NOT NULL,
            time REAL NOT NULL,
            logger TEXT NOT NULL,
            exception TEXT,
            exception_type TEXT,
            level TEXT NOT NULL
        );",
    )
    .map_err(|e| anyhow::anyhow!("The Prowlarr API's tables can't be made: {}", e.detail))?;
    system::seed_tasks(db).map_err(|e| anyhow::anyhow!("The scheduled tasks can't be saved: {}", e.detail))?;
    commands::at_start(db).map_err(|e| anyhow::anyhow!("The commands can't be read: {}", e.detail))?;
    apps::migrate(db)
}

/// Every route of the Prowlarr API this module answers.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(system::router())
        .merge(config::router())
        .merge(tags::router())
        .merge(filters::router())
        .merge(localization::router())
        .merge(health::router())
        .merge(commands::router())
        .merge(logs::router())
        .merge(apps::router())
}

/// A request that may use the Prowlarr API: it carries the API key (`?apikey=`, then `X-Api-Key`, then
/// `Authorization: Bearer`, the order Prowlarr reads them in) or comes from a signed-in Lantern session (a change
/// through the session cookie needs Lantern's `X-Requested-With` header, and an admin).
pub struct V1;

impl FromRequestParts<AppState> for V1 {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<V1, ApiError> {
        if key_given(st, parts) {
            return Ok(V1);
        }
        // A key that was given but is wrong does not count, as in Prowlarr (which reads `apikey` before the
        // header): only a session can still sign in, so the key header Lantern's own sign-in also reads is set
        // aside while it looks.
        let set_aside = parts.headers.remove("x-api-key");
        let account = crate::auth::signed_in(st, parts);
        if let Some(value) = set_aside {
            parts.headers.insert("x-api-key", value);
        }
        let account = account.map_err(|e| if e.status == StatusCode::UNAUTHORIZED { unauthorized() } else { ApiError::Kit(e) })?;
        if !matches!(parts.method, axum::http::Method::GET | axum::http::Method::HEAD) && !account.admin {
            return Err(ApiError::Kit(err(403, "Only an admin can change this")));
        }
        Ok(V1)
    }
}

/// Whether the request carries Lantern's API key, where Prowlarr looks for it.
fn key_given(st: &AppState, parts: &Parts) -> bool {
    let want = st.settings.str("api_key");
    if want.is_empty() {
        return false;
    }
    let query = parts.uri.query().unwrap_or("");
    let from_query = url::form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == "apikey").map(|(_, v)| v.into_owned());
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let given = from_query
        .or_else(|| header("x-api-key"))
        .or_else(|| header("authorization").map(|a| a.replace("Bearer ", "")))
        .unwrap_or_default();
    !given.trim().is_empty() && crate::auth::ct_eq(&given, &want)
}

/// The 401 Prowlarr answers without a valid key or session.
fn unauthorized() -> ApiError {
    ApiError::Kit(err(401, "The API key is missing or wrong, or you are not signed in"))
}

/// What a Prowlarr route answers when it can't do what was asked.
#[derive(Debug)]
pub enum ApiError {
    /// An error with a status and a message: `{"message": …, "detail": …}` (Prowlarr's `ErrorModel` and the kit's
    /// error in one body).
    Kit(kit_server_core::Error),
    /// The resource failed its checks: a 400 with the list of failures, as Prowlarr answers it.
    Invalid(Vec<Failure>),
}

impl From<kit_server_core::Error> for ApiError {
    fn from(e: kit_server_core::Error) -> ApiError {
        ApiError::Kit(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Kit(e) => {
                let message = e.extra.as_ref().and_then(|x| x.get("message")).cloned().unwrap_or_else(|| json!(e.detail));
                let mut extra = e.extra.clone().unwrap_or_else(|| json!({}));
                extra["message"] = message;
                kit_server_core::Error { status: e.status, detail: e.detail, extra: Some(extra) }.into_response()
            }
            ApiError::Invalid(list) => {
                (StatusCode::BAD_REQUEST, Json(Value::Array(list.iter().map(Failure::json).collect()))).into_response()
            }
        }
    }
}

/// What a Prowlarr route returns.
pub type ApiResult<T> = Result<T, ApiError>;

/// An error whose `message` is Prowlarr's text for it.
pub fn fail(status: u16, message: impl Into<String>) -> ApiError {
    let m = message.into();
    ApiError::Kit(err(status, m.clone()).with(json!({ "message": m })))
}

/// Prowlarr's `ApiException` (BadRequestException, NotFoundException, …): the message is the status' name, then
/// what was wrong (`BadRequest: 0 is not a valid ID`), and `content` is what was wrong on its own.
pub fn api_exception(status: u16, content: Option<&str>) -> ApiError {
    let name = match status {
        400 => "BadRequest",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        409 => "Conflict",
        415 => "UnsupportedMediaType",
        _ => "InternalServerError",
    };
    let message = match content {
        Some(c) => format!("{name}: {c}"),
        None => name.to_string(),
    };
    let mut extra = json!({ "message": message });
    if let Some(c) = content {
        extra["content"] = json!(c);
    }
    ApiError::Kit(err(status, message).with(extra))
}

/// Prowlarr's `ModelNotFoundException`: 404 `<Model> with ID <id> does not exist`.
pub fn not_found(model: &str, id: i64) -> ApiError {
    fail(404, format!("{model} with ID {id} does not exist"))
}

/// Prowlarr's `ValidateId` for PUT and DELETE by id: 400 when the id is not above 0.
pub fn validate_id(id: i64) -> ApiResult<()> {
    if id <= 0 {
        return Err(api_exception(400, Some(&format!("{id} is not a valid ID"))));
    }
    Ok(())
}

/// The 400 for a list of failures, or nothing when it is empty.
pub fn check(failures: Vec<Option<Failure>>) -> ApiResult<()> {
    let list: Vec<Failure> = failures.into_iter().flatten().collect();
    if list.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Invalid(list))
    }
}

/// `201 Created` with the new resource and its address in `Location`, as Prowlarr's `Created(id)`.
pub fn created(location: String, body: Value) -> Response {
    with_location(StatusCode::CREATED, location, body)
}

/// `202 Accepted` with the saved resource and its address in `Location`, as Prowlarr's `Accepted(id)`.
pub fn accepted(location: String, body: Value) -> Response {
    with_location(StatusCode::ACCEPTED, location, body)
}

fn with_location(status: StatusCode, location: String, body: Value) -> Response {
    let mut r = (status, Json(body)).into_response();
    if let Ok(v) = HeaderValue::from_str(&location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

/// A time (seconds since 1970) as Prowlarr writes dates: `2026-11-05T10:30:00Z`.
pub fn date(secs: f64) -> String {
    chrono::DateTime::from_timestamp(secs.floor() as i64, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A length of time as .NET writes a `TimeSpan`: `00:00:01.2345678`, `1.02:03:04` past a day.
pub fn timespan(secs: f64) -> String {
    let ticks = (secs * 10_000_000.0).round().max(0.0) as u64;
    let (days, rest) = (ticks / 864_000_000_000, ticks % 864_000_000_000);
    let (h, rest) = (rest / 36_000_000_000, rest % 36_000_000_000);
    let (m, rest) = (rest / 600_000_000, rest % 600_000_000);
    let (s, frac) = (rest / 10_000_000, rest % 10_000_000);
    let mut out = if days > 0 { format!("{days}.") } else { String::new() };
    out.push_str(&format!("{h:02}:{m:02}:{s:02}"));
    if frac > 0 {
        out.push_str(&format!(".{frac:07}"));
    }
    out
}

/// .NET's `SplitCamelCase`: a space before every capital but the first (`CheckHealth` → `Check Health`).
pub fn split_camel_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// One of Prowlarr's saved settings (its `Config` table and `config.xml`, kept in one table here), by name in any
/// case; `None` when it was never saved.
pub fn config_get(db: &SqliteDb, key: &str) -> Option<String> {
    db.one("SELECT value FROM prowlarr_config WHERE key=?", p![key.to_lowercase()]).ok().flatten().map(|r| r.s("value"))
}

/// Saves one of Prowlarr's settings.
pub fn config_set(db: &SqliteDb, key: &str, value: &str) -> kit_server_core::Result<()> {
    db.execute(
        "INSERT INTO prowlarr_config (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        p![key.to_lowercase(), value],
    )?;
    Ok(())
}

/// The request body as JSON; Prowlarr's 400 `Request body can't be empty` when there is none.
pub fn body_json(body: &[u8]) -> ApiResult<Value> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Err(api_exception(400, Some("Request body can't be empty")));
    }
    serde_json::from_slice(body).map_err(|e| fail(400, format!("The request body is not valid JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_dates_and_timespans_like_prowlarr() {
        assert_eq!(date(1_793_788_200.0), "2026-11-04T10:30:00Z");
        assert_eq!(timespan(1.2345678), "00:00:01.2345678");
        assert_eq!(timespan(3600.0 * 26.0 + 61.0), "1.02:01:01");
        assert_eq!(timespan(0.0), "00:00:00");
    }

    #[test]
    fn splits_camel_case_like_prowlarr() {
        assert_eq!(split_camel_case("IndexerDefinitionUpdate"), "Indexer Definition Update");
        assert_eq!(split_camel_case("CheckHealth"), "Check Health");
    }
}
