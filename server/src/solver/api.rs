//! The solver's routes.
//!
//! ```text
//! The FlareSolverr API, on its own port (APP_SOLVER_PORT, 8191 like FlareSolverr; no sign-in, like FlareSolverr):
//!   GET  /         {msg: "FlareSolverr is ready!", version, userAgent}
//!   GET  /health   {status: "ok"}
//!   POST /v1       {cmd: request.get | request.post | sessions.create | sessions.list | sessions.destroy, url,
//!                   maxTimeout (ms), session, cookies [{name, value}], returnOnlyCookies, proxy {url}, postData}
//!
//! The web app (signed in, admin):
//!   GET  /api/solver        the browser, its user agent and the sites with kept cookies
//!   POST /api/solver/test   {url}: opens the page and says how it went
//! ```

use super::{SolveRequest, Solver};
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// The FlareSolverr version Lantern answers as: apps check it is 2 or newer.
pub const FLARESOLVERR_VERSION: &str = "3.3.21";

/// How long a request waits for a check when it doesn't say (FlareSolverr's 60 s).
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// The FlareSolverr routes, for the solver's own port.
pub fn flaresolverr(solver: Solver) -> Router {
    Router::new().route("/", get(ready)).route("/health", get(health)).route("/v1", post(v1)).with_state(solver)
}

/// The web app's routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/solver", get(status)).route("/api/solver/test", post(test))
}

async fn ready(State(s): State<Solver>) -> Json<Value> {
    Json(json!({ "msg": "FlareSolverr is ready!", "version": FLARESOLVERR_VERSION, "userAgent": s.user_agent() }))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// A FlareSolverr error answer: HTTP 500 with `{status: "error", message}`.
fn fail(message: &str, start: u128) -> Response {
    let body = json!({
        "status": "error",
        "message": format!("Error: {message}"),
        "startTimestamp": start,
        "endTimestamp": now_ms(),
        "version": FLARESOLVERR_VERSION,
    });
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// The text of `v[k]`, "" when missing.
fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The request the FlareSolverr body asks for.
pub fn request_of(body: &Value) -> SolveRequest {
    let timeout = body.get("maxTimeout").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1_000, 600_000);
    let cookies = body
        .get("cookies")
        .and_then(Value::as_array)
        .map(|l| l.iter().map(|c| (text(c, "name"), text(c, "value"))).filter(|(n, _)| !n.is_empty()).collect())
        .unwrap_or_default();
    let proxy = match body.get("proxy") {
        Some(Value::Object(p)) => p.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    SolveRequest {
        url: text(body, "url"),
        post_data: None,
        timeout: Duration::from_millis(timeout),
        proxy,
        cookies,
        session: text(body, "session"),
        cookies_only: body.get("returnOnlyCookies").and_then(Value::as_bool).unwrap_or(false),
    }
}

async fn v1(State(s): State<Solver>, body: Option<Json<Value>>) -> Response {
    let start = now_ms();
    let Some(Json(body)) = body else { return fail("The request must be JSON with a cmd", start) };
    let cmd = text(&body, "cmd");
    match cmd.as_str() {
        "request.get" | "request.post" => {
            let mut req = request_of(&body);
            if req.url.is_empty() {
                return fail("Request parameter 'url' is mandatory", start);
            }
            if cmd == "request.post" {
                let Some(data) = body.get("postData").and_then(Value::as_str) else {
                    return fail("Request parameter 'postData' is mandatory in 'request.post' command", start);
                };
                req.post_data = Some(data.to_string());
            }
            match s.solve(&req).await {
                Ok(sol) => {
                    let message = if sol.challenged { "Challenge solved!" } else { "Challenge not detected!" };
                    let mut solution = serde_json::to_value(&sol).unwrap_or_default();
                    solution["headers"] = json!({});
                    Json(json!({
                        "status": "ok",
                        "message": message,
                        "solution": solution,
                        "startTimestamp": start,
                        "endTimestamp": now_ms(),
                        "version": FLARESOLVERR_VERSION,
                    }))
                    .into_response()
                }
                Err(e) => fail(&e, start),
            }
        }
        "sessions.create" => {
            let req = request_of(&body);
            match s.create_session(&req.session, &req.proxy).await {
                Ok(id) => Json(json!({ "status": "ok", "message": "Session created successfully.", "session": id, "startTimestamp": start, "endTimestamp": now_ms(), "version": FLARESOLVERR_VERSION })).into_response(),
                Err(e) => fail(&e, start),
            }
        }
        "sessions.list" => {
            Json(json!({ "status": "ok", "message": "", "sessions": s.sessions().await, "startTimestamp": start, "endTimestamp": now_ms(), "version": FLARESOLVERR_VERSION })).into_response()
        }
        "sessions.destroy" => {
            if s.destroy_session(&text(&body, "session")).await {
                Json(json!({ "status": "ok", "message": "The session has been removed.", "startTimestamp": start, "endTimestamp": now_ms(), "version": FLARESOLVERR_VERSION })).into_response()
            } else {
                fail("The session doesn't exist.", start)
            }
        }
        "" => fail("Request parameter 'cmd' is mandatory", start),
        other => fail(&format!("Request parameter 'cmd' = '{other}' is invalid"), start),
    }
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
    let url = text(&body, "url");
    if url.trim().is_empty() {
        return Err(err(400, "Type the address of a page to open"));
    }
    let proxy = st.settings.str("site_proxy");
    let req =
        SolveRequest { url: url.trim().to_string(), timeout: Duration::from_secs(60), proxy, cookies_only: true, ..Default::default() };
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

    #[test]
    fn reads_a_flaresolverr_request() {
        let r = request_of(&json!({
            "cmd": "request.get", "url": "https://x.test/", "maxTimeout": 30000, "session": "s1",
            "cookies": [{ "name": "a", "value": "1" }], "returnOnlyCookies": true, "proxy": { "url": "socks5://p:1080" }
        }));
        assert_eq!(
            (r.url.as_str(), r.timeout.as_secs(), r.session.as_str(), r.proxy.as_str()),
            ("https://x.test/", 30, "s1", "socks5://p:1080")
        );
        assert_eq!(r.cookies, vec![("a".to_string(), "1".to_string())]);
        assert!(r.cookies_only);
        assert_eq!(request_of(&json!({ "url": "u" })).timeout.as_secs(), 60);
    }
}
