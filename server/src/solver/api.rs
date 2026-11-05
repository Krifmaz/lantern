// Ported from FlareSolverr v3.5.2 (src/flaresolverr.py, src/flaresolverr_service.py, src/dtos.py,
// src/bottle_plugins/), MIT.
//! The solver's routes.
//!
//! ```text
//! The FlareSolverr API, on its own port (APP_SOLVER_PORT, 8191 like FlareSolverr; no sign-in, like FlareSolverr):
//!   GET  /         {msg: "FlareSolverr is ready!", version, userAgent}
//!   GET  /health   {status: "ok"}
//!   POST /v1       {cmd: request.get | request.post | sessions.create | sessions.list | sessions.destroy, url,
//!                   maxTimeout (ms), session, session_ttl_minutes, cookies [{name, value, ...}], proxy {url,
//!                   username, password}, postData, returnOnlyCookies, returnScreenshot, waitInSeconds,
//!                   disableMedia, tabs_till_verify}
//!   anything else  {error, status_code} (404, or 405 for the wrong method)
//!
//! The web app (signed in, admin):
//!   GET  /api/solver        the browser, its user agent and the sites with kept cookies
//!   POST /api/solver/test   {url}: opens the page and says how it went
//! ```
//!
//! The answers, their fields and their error texts are FlareSolverr's own, word for word, since apps read them;
//! FlareSolverr's Python reads the request loosely (`if req.session:`, `int(req.maxTimeout)`), and so does this
//! port ([`py_truthy`], [`py_int`], [`py_str`]).

use super::{chrome_version, metrics, CookieIn, CookieSpec, Job, PostForm, Proxy, SolveRequest, Solver};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// The FlareSolverr version Lantern answers as (the one it was ported from).
pub const FLARESOLVERR_VERSION: &str = "3.5.2";

/// How long a request may take when it doesn't say, in milliseconds (FlareSolverr's 60 s).
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// The largest request body read as JSON (Bottle's `MEMFILE_MAX`).
pub const MEMFILE_MAX: usize = 102_400;

/// The FlareSolverr routes, for the solver's own port. With `PROMETHEUS_ENABLED`, the metrics are served on
/// `PROMETHEUS_PORT` too.
pub fn flaresolverr(solver: Solver) -> Router {
    if solver.config().prometheus_enabled {
        metrics::serve(solver.metrics(), solver.config().prometheus_port);
    }
    Router::new()
        .route("/", get(index).fallback(|| async { not_allowed("GET") }))
        .route("/health", get(health).fallback(|| async { not_allowed("GET") }))
        .route("/v1", post(v1).fallback(|| async { not_allowed("POST") }))
        .fallback(not_found)
        .layer(middleware::from_fn(log_request))
        .with_state(solver)
}

/// The web app's routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/solver", get(status)).route("/api/solver/test", post(test))
}

/// Logs each request but `/health` (FlareSolverr's `logger_plugin`).
async fn log_request(req: Request, next: Next) -> Response {
    let (method, uri) = (req.method().clone(), req.uri().clone());
    let res = next.run(req).await;
    if !uri.path().ends_with("/health") {
        tracing::info!("{method} {uri} {}", res.status());
    }
    res
}

/// Bottle's answer for a path it doesn't know: `{error: "Not found: '/path'", status_code: 404}`.
async fn not_found(uri: Uri) -> Response {
    let path = percent_encoding::percent_decode_str(uri.path()).decode_utf8_lossy().into_owned();
    let body = json!({ "error": format!("Not found: {}", py_repr(&Value::String(path))), "status_code": 404 });
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}

/// Bottle's answer for a known path asked with the wrong method.
fn not_allowed(allow: &'static str) -> Response {
    let body = json!({ "error": "Method not allowed.", "status_code": 405 });
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, allow)], Json(body)).into_response()
}

/// FlareSolverr's `error_plugin`: an error outside the `/v1` command, as HTTP 500 `{error}`.
fn plugin_error(message: &str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": message }))).into_response()
}

/// `GET /`: the welcome message, the version and the browser's user agent (the browser starts to tell it).
async fn index(State(s): State<Solver>) -> Response {
    match s.learn_user_agent().await {
        Ok(ua) => Json(json!({ "msg": "FlareSolverr is ready!", "version": FLARESOLVERR_VERSION, "userAgent": ua })).into_response(),
        Err(e) => plugin_error(&format!("Error getting browser User-Agent. {e}")),
    }
}

/// `GET /health`.
async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Python's truth of a JSON value: null, false, 0, "", [] and {} are false.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python's name of a JSON value's type (`int`, `str`, `list`, ...).
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python's `repr` of a JSON value (`'text'`, `[1, 'a']`, `{'k': True}`).
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
            let mut out = String::from(q);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == q => {
                        out.push('\\');
                        out.push(c);
                    }
                    c => out.push(c),
                }
            }
            out.push(q);
            out
        }
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            format!(
                "{{{}}}",
                o.iter().map(|(k, v)| format!("{}: {}", py_repr(&Value::String(k.clone())), py_repr(v))).collect::<Vec<_>>().join(", ")
            )
        }
        other => py_str(other),
    }
}

/// Python's `str` of a JSON value (`None`, `True`, `5`, `5.0`, the text itself).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) if n.is_f64() => super::py_float(n.as_f64().unwrap_or(0.0)),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python's `int(v)`, with Python's error words.
pub fn py_int(v: &Value) -> std::result::Result<i64, String> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => Ok(i),
            (None, Some(f)) if n.is_u64() => Ok(f.min(i64::MAX as f64) as i64),
            (None, Some(f)) => Ok(f.trunc().clamp(i64::MIN as f64, i64::MAX as f64) as i64),
            _ => Err("cannot convert float NaN to integer".into()),
        },
        Value::String(s) => {
            let t = s.trim();
            let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            let parsed = t.replace('_', "").parse::<i128>().ok().filter(|_| ok);
            parsed
                .map(|i| i.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
                .ok_or_else(|| format!("invalid literal for int() with base 10: {}", py_repr(v)))
        }
        other => Err(format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", py_type(other))),
    }
}

/// The value of `k` unless it is missing or null (FlareSolverr's `is not None`).
fn given<'a>(req: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    req.get(k).filter(|v| !v.is_null())
}

/// Whether `k` is set and true in Python's sense.
fn truthy(req: &Map<String, Value>, k: &str) -> bool {
    req.get(k).is_some_and(py_truthy)
}

/// Bottle's `request.json or {}`: the body as JSON when it is sent as JSON, `{}` when it isn't or is empty or
/// false; an error (Bottle's own words, as FlareSolverr's error plugin shows them) when it is too big or isn't JSON.
pub fn request_json(headers: &HeaderMap, body: &[u8]) -> std::result::Result<Value, String> {
    let ctype = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    let ctype = ctype.split(';').next().unwrap_or("");
    if ctype != "application/json" && ctype != "application/json-rpc" {
        return Ok(json!({}));
    }
    if body.len() > MEMFILE_MAX {
        return Err("(413, 'Request entity too large')".into());
    }
    if body.is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| "(400, 'Invalid JSON')".to_string())?;
    Ok(if py_truthy(&v) { v } else { json!({}) })
}

/// FlareSolverr's environment proxy (`PROXY_URL`, `PROXY_USERNAME`, `PROXY_PASSWORD`) for a request that gives
/// none, then the request as FlareSolverr's `V1RequestBase(data)` reads it (Python's errors for a body that isn't
/// an object).
pub fn request_of(mut data: Value, cfg: &super::Config) -> std::result::Result<Map<String, Value>, String> {
    if let Some(url) = &cfg.proxy_url {
        let opt = |v: &Option<String>| v.clone().map(Value::String).unwrap_or(Value::Null);
        let proxy = if cfg.proxy_username.is_none() && cfg.proxy_password.is_none() {
            json!({ "url": url })
        } else {
            json!({ "url": url, "username": opt(&cfg.proxy_username), "password": opt(&cfg.proxy_password) })
        };
        match &mut data {
            Value::Object(o) => {
                if !o.get("proxy").is_some_and(py_truthy) {
                    tracing::info!("Using proxy URL ENV");
                    o.insert("proxy".into(), proxy);
                }
            }
            Value::Array(_) => return Err("list indices must be integers or slices, not str".into()),
            Value::String(_) => return Err("'str' object does not support item assignment".into()),
            other => return Err(format!("argument of type '{}' is not iterable", py_type(other))),
        }
    }
    match data {
        Value::Object(o) => Ok(o),
        Value::Array(items) => {
            let mut o = Map::new();
            for (i, item) in items.iter().enumerate() {
                let pair: Vec<Value> = match item {
                    Value::Array(a) => a.clone(),
                    Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
                    _ => return Err(format!("cannot convert dictionary update sequence element #{i} to a sequence")),
                };
                if pair.len() != 2 {
                    return Err(format!("dictionary update sequence element #{i} has length {}; 2 is required", pair.len()));
                }
                o.insert(py_str(&pair[0]), pair[1].clone());
            }
            Ok(o)
        }
        Value::String(s) => {
            Err(format!("dictionary update sequence element #0 has length {}; 2 is required", s.chars().next().map_or(0, |_| 1)))
        }
        other => Err(format!("'{}' object is not iterable", py_type(&other))),
    }
}

/// The proxy a request gives: `{url}`, or `{url, username, password}` (all three keys) for one that asks to sign
/// in; anything else is no proxy.
pub fn proxy_of(v: Option<&Value>) -> Option<Proxy> {
    let o = v.filter(|v| py_truthy(v))?.as_object()?;
    let url = o.get("url")?;
    let credentials = match (o.get("username"), o.get("password")) {
        (Some(u), Some(p)) => Some((py_str(u), py_str(p))),
        _ => None,
    };
    Some(Proxy { url: py_str(url), credentials })
}

/// The cookies a request asks to set, each checked the way Selenium's `add_cookie` (and Python) would.
pub fn cookies_of(v: Option<&Value>) -> Vec<CookieIn> {
    let not_indexable = |item: &Value| match item {
        Value::String(_) => "string indices must be integers, not 'str'".to_string(),
        Value::Array(_) => "list indices must be integers or slices, not str".to_string(),
        other => format!("'{}' object is not subscriptable", py_type(other)),
    };
    match v {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                let Some(o) = item.as_object() else { return Err(not_indexable(item)) };
                let name = o.get("name").ok_or_else(|| "'name'".to_string())?;
                let text = |k: &str| o.get(k).filter(|v| !v.is_null()).map(py_str);
                Ok(CookieSpec {
                    name: py_str(name),
                    value: text("value"),
                    domain: text("domain"),
                    path: text("path"),
                    secure: o.get("secure").map(py_truthy),
                    http_only: o.get("httpOnly").map(py_truthy),
                    expiry: o.get("expiry").and_then(Value::as_f64),
                    same_site: o.get("sameSite").map(py_str),
                })
            })
            .collect(),
        Some(v @ (Value::Object(_) | Value::String(_))) if py_truthy(v) => vec![Err("string indices must be integers, not 'str'".into())],
        Some(Value::Object(_) | Value::String(_)) => vec![],
        Some(other) => vec![Err(format!("object of type '{}' has no len()", py_type(other)))],
    }
}

/// The browser job a `request.get` or `request.post` asks for (`post`: the method is POST), with `max_timeout`
/// in milliseconds; an error in FlareSolverr's words when a field can't be used.
pub fn job_of(req: &Map<String, Value>, post: bool, max_timeout: i64) -> std::result::Result<Job, String> {
    let wrap = |e: String| format!("Error solving the challenge. {e}");
    let session = given(req, "session").filter(|v| py_truthy(v)).map(py_str);
    let session_ttl = match (session.is_some(), req.get("session_ttl_minutes")) {
        (true, Some(v)) if py_truthy(v) => match v {
            Value::Number(n) => Some(Duration::from_secs_f64((n.as_f64().unwrap_or(0.0) * 60.0).max(0.0))),
            Value::Bool(_) => Some(Duration::from_secs(60)),
            other => return Err(wrap(format!("unsupported type for timedelta minutes component: {}", py_type(other)))),
        },
        _ => None,
    };
    let return_only_cookies = truthy(req, "returnOnlyCookies");
    let wait_in_seconds = match req.get("waitInSeconds") {
        Some(v) if py_truthy(v) && !return_only_cookies => match v {
            Value::Number(n) => n.as_f64().unwrap_or(0.0),
            Value::Bool(_) => 1.0,
            other => return Err(wrap(format!("'>' not supported between instances of '{}' and 'int'", py_type(other)))),
        },
        _ => 0.0,
    };
    let tabs_till_verify = given(req, "tabs_till_verify").map(|v| match v {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64(),
        _ => None,
    });
    Ok(Job {
        url: req.get("url").map(py_str).unwrap_or_else(|| "None".into()),
        post: post.then(|| PostForm { data: given(req, "postData").map(py_str).unwrap_or_default(), flaresolverr: true }),
        cookies: cookies_of(req.get("cookies")),
        proxy: proxy_of(req.get("proxy")),
        session,
        session_ttl,
        timeout_ms: max_timeout.max(1) as u64,
        return_only_cookies,
        return_screenshot: truthy(req, "returnScreenshot"),
        wait_in_seconds: wait_in_seconds.max(0.0),
        disable_media: given(req, "disableMedia").map(py_truthy),
        tabs_till_verify,
    })
}

/// The `solution` FlareSolverr answers: status 200 (Selenium can't tell the real one), the cookies as
/// chromedriver gives them, `headers` and `response` only when the HTML was asked for, `screenshot` only when a
/// screenshot was.
pub fn solution_json(sol: &super::Solution, job: &Job) -> Value {
    let cookies: Vec<Value> = sol
        .cookies
        .iter()
        .map(|c| {
            let mut o = Map::new();
            o.insert("domain".into(), json!(c.domain));
            if !c.session {
                o.insert("expiry".into(), json!(c.expires as i64));
            }
            o.insert("httpOnly".into(), json!(c.http_only));
            o.insert("name".into(), json!(c.name));
            o.insert("path".into(), json!(c.path));
            if let Some(s) = &c.same_site {
                o.insert("sameSite".into(), json!(s));
            }
            o.insert("secure".into(), json!(c.secure));
            o.insert("value".into(), json!(c.value));
            Value::Object(o)
        })
        .collect();
    let mut o = Map::new();
    o.insert("url".into(), json!(sol.url));
    o.insert("status".into(), json!(200));
    o.insert("userAgent".into(), json!(sol.user_agent));
    o.insert("turnstile_token".into(), json!(sol.turnstile_token));
    if !job.return_only_cookies {
        o.insert("headers".into(), json!({}));
        o.insert("response".into(), json!(sol.response));
    }
    if let Some(s) = &sol.screenshot {
        o.insert("screenshot".into(), json!(s));
    }
    o.insert("cookies".into(), Value::Array(cookies));
    Value::Object(o)
}

/// The request with a proxy password hidden, for the log.
fn for_log(req: &Map<String, Value>) -> Value {
    let mut r = req.clone();
    if let Some(Value::Object(p)) = r.get_mut("proxy") {
        if p.contains_key("password") {
            p.insert("password".into(), json!("***"));
        }
    }
    Value::Object(r)
}

/// FlareSolverr's `_controller_v1_handler`: checks the request and runs its command. The error is the text
/// after `Error: `.
async fn handle(s: &Solver, req: &Map<String, Value>) -> std::result::Result<Map<String, Value>, String> {
    let Some(cmd) = given(req, "cmd") else { return Err("Request parameter 'cmd' is mandatory.".into()) };
    if given(req, "headers").is_some() {
        tracing::warn!("Request parameter 'headers' was removed in FlareSolverr v2.");
    }
    if given(req, "userAgent").is_some() {
        tracing::warn!("Request parameter 'userAgent' was removed in FlareSolverr v2.");
    }
    let max_timeout = match given(req, "maxTimeout") {
        None => DEFAULT_TIMEOUT_MS as i64,
        Some(v) => match py_int(v)? {
            n if n < 1 => DEFAULT_TIMEOUT_MS as i64,
            n => n,
        },
    };
    let ok = |message: &str| {
        let mut o = Map::new();
        o.insert("status".into(), json!("ok"));
        o.insert("message".into(), json!(message));
        o
    };
    match cmd.as_str() {
        Some("sessions.create") => {
            tracing::debug!("Creating new session...");
            let id = given(req, "session").filter(|v| py_truthy(v)).map(py_str);
            let (id, fresh) = s.create_session(id, proxy_of(req.get("proxy"))).await.map_err(|e| e.to_string())?;
            let mut o = ok(if fresh { "Session created successfully." } else { "Session already exists." });
            o.insert("session".into(), json!(id));
            Ok(o)
        }
        Some("sessions.list") => {
            let mut o = ok("");
            o.insert("sessions".into(), json!(s.session_ids().await));
            Ok(o)
        }
        Some("sessions.destroy") => {
            let id = given(req, "session").map(py_str);
            match id {
                Some(id) if s.destroy_session(&id).await => Ok(ok("The session has been removed.")),
                _ => Err("The session doesn't exist.".into()),
            }
        }
        Some(c @ ("request.get" | "request.post")) => {
            let post = c == "request.post";
            if post {
                if given(req, "postData").is_none() {
                    return Err("Request parameter 'postData' is mandatory in 'request.post' command.".into());
                }
            } else {
                if given(req, "url").is_none() {
                    return Err("Request parameter 'url' is mandatory in 'request.get' command.".into());
                }
                if given(req, "postData").is_some() {
                    return Err("Cannot use 'postBody' when sending a GET request.".into());
                }
            }
            if given(req, "returnRawHtml").is_some() {
                tracing::warn!("Request parameter 'returnRawHtml' was removed in FlareSolverr v2.");
            }
            if given(req, "download").is_some() {
                tracing::warn!("Request parameter 'download' was removed in FlareSolverr v2.");
            }
            let job = job_of(req, post, max_timeout)?;
            let sol = s.run(&job).await.map_err(|e| e.flaresolverr(&chrome_version(&s.user_agent())))?;
            let mut o = ok(if sol.challenged { "Challenge solved!" } else { "Challenge not detected!" });
            o.insert("solution".into(), solution_json(&sol, &job));
            Ok(o)
        }
        _ => Err(format!("Request parameter 'cmd' = '{}' is invalid.", py_str(cmd))),
    }
}

/// FlareSolverr's `controller_v1_endpoint`: the command's answer (or `status: error` with `Error: ...`), with
/// the timestamps and the version; and whether it is an error (HTTP 500).
pub async fn controller(s: &Solver, req: &Map<String, Value>) -> (Value, bool) {
    let start = now_ms();
    tracing::info!("Incoming request => POST /v1 body: {}", for_log(req));
    let (mut o, failed) = match handle(s, req).await {
        Ok(o) => (o, false),
        Err(e) => {
            let message = format!("Error: {e}");
            tracing::error!("{message}");
            let mut o = Map::new();
            o.insert("status".into(), json!("error"));
            o.insert("message".into(), json!(message));
            (o, true)
        }
    };
    let end = now_ms();
    o.insert("startTimestamp".into(), json!(start));
    o.insert("endTimestamp".into(), json!(end));
    o.insert("version".into(), json!(FLARESOLVERR_VERSION));
    tracing::info!("Response in {} s", super::py_float((end - start) as f64 / 1000.0));
    (Value::Object(o), failed)
}

/// `POST /v1`.
async fn v1(State(s): State<Solver>, headers: HeaderMap, body: Bytes) -> Response {
    let data = match request_json(&headers, &body) {
        Ok(v) => v,
        Err(e) => return plugin_error(&e),
    };
    let original = data.clone();
    let req = match request_of(data, s.config()) {
        Ok(r) => r,
        Err(e) => return plugin_error(&e),
    };
    let (res, failed) = controller(&s, &req).await;
    if s.config().prometheus_enabled {
        s.metrics().record(&original, &res);
    }
    let code = if failed { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::OK };
    (code, Json(res)).into_response()
}

async fn status(State(st): State<AppState>, _: Admin) -> Json<Value> {
    let s = &st.solver;
    let sites: Vec<Value> = s.cleared_sites().into_iter().map(|(h, until)| json!({ "host": h, "until": until })).collect();
    Json(json!({
        "browser": s.program().map(|p| p.display().to_string()),
        "running": s.is_running().await,
        "user_agent": s.user_agent(),
        "port": crate::solver_port(),
        "sites": sites,
    }))
}

async fn test(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let url = body.get("url").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if url.is_empty() {
        return Err(err(400, "Type the address of a page to open"));
    }
    let proxy = st.settings.str("site_proxy");
    let req = SolveRequest { url, timeout: Duration::from_secs(60), proxy, cookies_only: true, ..Default::default() };
    match st.solver.solve(&req).await {
        Ok(sol) => {
            let message = if sol.challenged {
                format!("Got past the Cloudflare check of {} – its cookies are kept for the indexers", super::host_of(&sol.url))
            } else {
                format!("{} opened without a Cloudflare check", super::host_of(&sol.url))
            };
            Ok(Json(json!({ "ok": true, "message": message, "challenged": sol.challenged })))
        }
        Err(e) => Ok(Json(json!({ "ok": false, "message": e }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::Config;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn python_reading_of_values() {
        assert_eq!(py_int(&json!(30000)), Ok(30000));
        assert_eq!(py_int(&json!(1500.9)), Ok(1500));
        assert_eq!(py_int(&json!(" 2_000 ")), Ok(2000));
        assert_eq!(py_int(&json!(true)), Ok(1));
        assert_eq!(py_int(&json!("abc")), Err("invalid literal for int() with base 10: 'abc'".into()));
        assert_eq!(py_int(&json!("1.5")), Err("invalid literal for int() with base 10: '1.5'".into()));
        assert_eq!(py_int(&json!([1])), Err("int() argument must be a string, a bytes-like object or a real number, not 'list'".into()));
        assert_eq!(py_str(&json!(5)), "5");
        assert_eq!(py_str(&json!(5.0)), "5.0");
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!([1, "a", true])), "[1, 'a', True]");
        assert_eq!(py_str(&json!({ "a": null })), "{'a': None}");
        assert_eq!(py_repr(&json!("it's")), "\"it's\"");
        assert!(!py_truthy(&json!(0)) && !py_truthy(&json!("")) && !py_truthy(&json!({})) && py_truthy(&json!("0")));
    }

    #[test]
    fn the_body_is_read_like_bottle() {
        let mut h = HeaderMap::new();
        assert_eq!(request_json(&h, b"{\"cmd\":1}"), Ok(json!({})), "not sent as JSON: {{}}");
        h.insert(header::CONTENT_TYPE, "Application/JSON; charset=utf-8".parse().unwrap());
        assert_eq!(request_json(&h, b"{\"cmd\":1}"), Ok(json!({ "cmd": 1 })));
        assert_eq!(request_json(&h, b""), Ok(json!({})));
        assert_eq!(request_json(&h, b"null"), Ok(json!({})));
        assert_eq!(request_json(&h, b"[]"), Ok(json!({})));
        assert_eq!(request_json(&h, b"{bad"), Err("(400, 'Invalid JSON')".into()));
        assert_eq!(request_json(&h, &vec![b' '; MEMFILE_MAX + 1]), Err("(413, 'Request entity too large')".into()));
        let cfg = Config::default();
        assert_eq!(request_of(json!("ab"), &cfg), Err("dictionary update sequence element #0 has length 1; 2 is required".into()));
        assert_eq!(request_of(json!(5), &cfg), Err("'int' object is not iterable".into()));
        assert_eq!(request_of(json!([["cmd", "sessions.list"]]), &cfg), Ok(obj(json!({ "cmd": "sessions.list" }))));
    }

    #[test]
    fn the_environment_proxy_fills_in() {
        let cfg = Config { proxy_url: Some("http://p:1".into()), ..Config::default() };
        let r = request_of(json!({ "cmd": "x" }), &cfg).unwrap();
        assert_eq!(r["proxy"], json!({ "url": "http://p:1" }));
        let r = request_of(json!({ "cmd": "x", "proxy": { "url": "socks5://mine:2" } }), &cfg).unwrap();
        assert_eq!(r["proxy"], json!({ "url": "socks5://mine:2" }), "the request's own proxy wins");
        let r = request_of(json!({ "proxy": {} }), &cfg).unwrap();
        assert_eq!(r["proxy"], json!({ "url": "http://p:1" }), "an empty proxy is none");
        let cfg = Config { proxy_url: Some("http://p:1".into()), proxy_username: Some("u".into()), ..Config::default() };
        let r = request_of(json!({}), &cfg).unwrap();
        assert_eq!(r["proxy"], json!({ "url": "http://p:1", "username": "u", "password": null }));
        assert_eq!(proxy_of(r.get("proxy")), Some(Proxy { url: "http://p:1".into(), credentials: Some(("u".into(), "None".into())) }));
        assert_eq!(
            proxy_of(Some(&json!({ "url": "http://q:2", "username": "u" }))),
            Some(Proxy { url: "http://q:2".into(), credentials: None })
        );
        assert_eq!(proxy_of(Some(&json!("http://q:2"))), None);
        assert_eq!(request_of(json!([1]), &cfg), Err("list indices must be integers or slices, not str".into()));
    }

    #[test]
    fn a_request_becomes_a_job() {
        let req = obj(json!({
            "cmd": "request.get", "url": "https://x.test/", "session": "s1", "session_ttl_minutes": 2,
            "cookies": [{ "name": "a", "value": "1", "domain": ".x.test", "sameSite": "Lax" }],
            "returnOnlyCookies": true, "returnScreenshot": 1, "waitInSeconds": 5, "disableMedia": false, "tabs_till_verify": 3,
            "proxy": { "url": "socks5://p:1080" }
        }));
        let j = job_of(&req, false, 30000).unwrap();
        assert_eq!((j.url.as_str(), j.timeout_ms, j.session.as_deref()), ("https://x.test/", 30000, Some("s1")));
        assert_eq!(j.session_ttl, Some(Duration::from_secs(120)));
        assert_eq!(
            j.cookies,
            vec![Ok(CookieSpec {
                name: "a".into(),
                value: Some("1".into()),
                domain: Some(".x.test".into()),
                same_site: Some("Lax".into()),
                ..Default::default()
            })]
        );
        assert!(j.return_only_cookies && j.return_screenshot);
        assert_eq!(j.wait_in_seconds, 0.0, "FlareSolverr only waits when it returns the HTML");
        assert_eq!((j.disable_media, j.tabs_till_verify), (Some(false), Some(Some(3))));
        assert_eq!(j.proxy, Some(Proxy { url: "socks5://p:1080".into(), credentials: None }));
        assert!(j.post.is_none());
        let post = job_of(&obj(json!({ "postData": "a=1", "waitInSeconds": 2 })), true, 60000).unwrap();
        assert_eq!(post.post, Some(PostForm { data: "a=1".into(), flaresolverr: true }));
        assert_eq!((post.url.as_str(), post.wait_in_seconds, post.session_ttl), ("None", 2.0, None));
        let e = job_of(&obj(json!({ "session": "s", "session_ttl_minutes": "5" })), false, 1).unwrap_err();
        assert_eq!(e, "Error solving the challenge. unsupported type for timedelta minutes component: str");
        assert_eq!(cookies_of(Some(&json!([{ "value": "1" }]))), vec![Err("'name'".into())]);
        assert_eq!(cookies_of(Some(&json!(["a"]))), vec![Err("string indices must be integers, not 'str'".into())]);
        assert_eq!(cookies_of(Some(&json!(5))), vec![Err("object of type 'int' has no len()".into())]);
        assert!(cookies_of(Some(&json!({}))).is_empty());
    }

    #[test]
    fn the_solution_has_flaresolverrs_fields() {
        let sol = super::super::Solution {
            url: "https://x.test/".into(),
            status: 404,
            response: "<html></html>".into(),
            cookies: vec![super::super::SolvedCookie {
                name: "cf_clearance".into(),
                value: "v".into(),
                domain: ".x.test".into(),
                path: "/".into(),
                expires: 1_800_000_000.5,
                size: 13,
                http_only: true,
                secure: true,
                session: false,
                same_site: Some("None".into()),
            }],
            user_agent: "Mozilla/5.0 Chrome/142".into(),
            ..Default::default()
        };
        let v = solution_json(&sol, &Job::default());
        assert_eq!(
            v,
            json!({ "url": "https://x.test/", "status": 200, "userAgent": "Mozilla/5.0 Chrome/142", "turnstile_token": null,
                "headers": {}, "response": "<html></html>",
                "cookies": [{ "domain": ".x.test", "expiry": 1800000000, "httpOnly": true, "name": "cf_clearance", "path": "/",
                    "sameSite": "None", "secure": true, "value": "v" }] })
        );
        let v = solution_json(&sol, &Job { return_only_cookies: true, ..Job::default() });
        assert!(v.get("headers").is_none() && v.get("response").is_none() && v.get("screenshot").is_none());
    }
}
