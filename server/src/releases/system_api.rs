// Ported from autobrr v1.90.0 (internal/http/auth.go, middleware.go `IsAuthenticated`, health.go, config.go,
// update.go, apikey.go, proxy.go, encoder.go), GPL-2.0-or-later.
//! autobrr's application routes under `/autobrr/api`: signing in and out, onboarding, the account, health,
//! the config file's settings, update checks, API keys and proxies, plus [`guard`], autobrr's
//! `IsAuthenticated` in front of every route.
//!
//! ```text
//! POST   /auth/login              {username, password, remember_me}: Lantern's session cookie (204)
//! POST   /auth/logout             the session ends (204)
//! GET    /auth/onboard            204 while there is no account yet, else 503
//! POST   /auth/onboard            refused: Lantern's first account is made on its own setup page
//! GET    /auth/validate           {username, auth_method}
//! PATCH  /auth/user/{username}    {username_new, password_current, password_new}
//! GET    /auth/oidc/config        404: Lantern has no OIDC sign-in (autobrr answers 404 when OIDC is off)
//! GET    /healthz/liveness        "OK"
//! GET    /healthz/readiness       "OK", or 500 when the database doesn't answer
//! GET    /config                  the config file's settings and the build
//! PATCH  /config                  {log_level, log_path, check_for_updates} (204)
//! GET    /updates/latest          the newer release found (204 when none)
//! GET    /updates/check           look now (204)
//! GET    /keys                    the API keys
//! POST   /keys                    {name, scopes}: a new key (201)
//! DELETE /keys/{key}              (204)
//! GET    /proxy                   the proxies
//! POST   /proxy                   add (204)
//! POST   /proxy/test              ask a page through it (204)
//! GET    /proxy/{id}              one
//! PUT    /proxy/{id}              change (204)
//! DELETE /proxy/{id}              remove (204)
//! GET    /proxy/{id}/usage        the indexers, networks and feeds using it
//! ```
//!
//! Lantern's accounts are autobrr's users. A request is signed in by Lantern's session cookie, an autobrr API
//! key (`X-API-Token` header or `?apikey=`) or Lantern's own API key (`X-API-Key`).

use super::autobrr_api::{decode, id_of, no_content, send, Answer, Fail};
use super::proxy::{Proxy, ERR_NO_ROWS};
use super::store::ApiError;
use super::system::{ApiKey, ConfigUpdate, LOG_MAX_BACKUPS, LOG_MAX_SIZE};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Extension, Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use kit_server_core::{p, Admin, Db, RowExt, SignedIn};
use serde::Deserialize;
use serde_json::json;

/// The routes, relative to autobrr's `/api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/onboard", get(can_onboard).post(onboard))
        .route("/auth/validate", get(validate))
        .route("/auth/user/{username}", patch(update_user))
        .route("/auth/oidc/config", get(oidc_off))
        .route("/auth/oidc/callback", get(oidc_off))
        .route("/healthz/liveness", get(liveness))
        .route("/healthz/readiness", get(readiness))
        .route("/config", get(get_config).patch(update_config))
        .route("/config/", get(get_config).patch(update_config))
        .route("/updates/latest", get(latest_release))
        .route("/updates/check", get(check_updates))
        .route("/keys", get(list_keys).post(store_key))
        .route("/keys/", get(list_keys).post(store_key))
        .route("/keys/{apikey}", delete(delete_key))
        .route("/proxy", get(list_proxies).post(store_proxy))
        .route("/proxy/", get(list_proxies).post(store_proxy))
        .route("/proxy/test", post(test_proxy))
        .route("/proxy/{id}", get(find_proxy).put(update_proxy).delete(delete_proxy))
        .route("/proxy/{id}/", get(find_proxy).put(update_proxy).delete(delete_proxy))
        .route("/proxy/{id}/usage", get(proxy_usage))
}

/// How a request to autobrr's API signed in (autobrr keeps it in the session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// Lantern's session cookie, as this account.
    Session(String),
    /// An API key (autobrr's or Lantern's).
    Key,
}

/// The paths autobrr answers without a session (its `/auth` routes outside `IsAuthenticated`, and `/healthz`).
fn is_public(path: &str) -> bool {
    let rel = path.strip_prefix(crate::autobrr_ui::API_BASE).unwrap_or(path);
    let rel = rel.trim_end_matches('/');
    matches!(rel, "/auth/login" | "/auth/onboard" | "/healthz/liveness" | "/healthz/readiness") || rel.starts_with("/auth/oidc/")
}

/// The autobrr API key a request carries: the `X-API-Token` header, else `?apikey=`.
fn autobrr_token(req: &Request) -> Option<String> {
    if let Some(t) = req.headers().get("x-api-token").and_then(|v| v.to_str().ok()).filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).find(|(k, v)| k == "apikey" && !v.is_empty()).map(|(_, v)| v.into_owned())
}

/// autobrr's `IsAuthenticated` in front of every route but the open ones: an autobrr API key (or Lantern's own)
/// in `X-API-Token` or `?apikey=` signs in as the owner (a wrong one is a 401); otherwise Lantern's `X-API-Key`
/// or app token goes on to the route as it is; otherwise Lantern's session cookie must be valid (a 403
/// otherwise, as autobrr answers a missing session). autobrr's frontend marks its requests `X-Requested-With:
/// XMLHttpRequest`, which counts as Lantern's mark against cross-site requests (both need the browser to allow
/// a custom header).
pub async fn guard(State(st): State<AppState>, mut req: Request, next: axum::middleware::Next) -> Response {
    if is_public(req.uri().path()) {
        let csrf = req.headers().get("x-requested-with").is_some_and(|v| v == "XMLHttpRequest");
        if csrf {
            req.headers_mut().insert("x-requested-with", HeaderValue::from_static(crate::NAME));
        }
        return next.run(req).await;
    }
    if let Some(token) = autobrr_token(&req) {
        let own = st.settings.str("api_key");
        let ok = st.releases.validate_api_key(&token) || (!own.is_empty() && crate::auth::ct_eq(&token, &own));
        if !ok || own.is_empty() {
            return kit_server_core::err(401, "This API key isn't valid. Make a new key in Settings → API keys, then try again.").into_response();
        }
        let h = req.headers_mut();
        h.remove(header::COOKIE);
        h.remove(header::AUTHORIZATION);
        if let Ok(v) = HeaderValue::from_str(&own) {
            h.insert("x-api-key", v);
        }
        req.extensions_mut().insert(Via::Key);
        return next.run(req).await;
    }
    if req.headers().contains_key("x-api-key") || crate::auth::bearer(req.headers()).is_some() {
        req.extensions_mut().insert(Via::Key);
        return next.run(req).await;
    }
    if req.headers().get("x-requested-with").is_some_and(|v| v == "XMLHttpRequest") {
        req.headers_mut().insert("x-requested-with", HeaderValue::from_static(crate::NAME));
    }
    let (parts, body) = req.into_parts();
    match crate::auth::signed_in(&st, &parts) {
        Ok(a) => {
            let mut req = Request::from_parts(parts, body);
            req.extensions_mut().insert(Via::Session(a.username));
            next.run(req).await
        }
        Err(e) if e.status == StatusCode::FORBIDDEN => e.into_response(),
        Err(_) => kit_server_core::err(403, "You're signed out of Lantern. Sign in again, then reload this page.").into_response(),
    }
}

/// autobrr's `StatusError`: `{message}` with `status` (no code), and Lantern's `detail`.
fn status_error(status: u16, m: impl Into<String>) -> Response {
    let m = m.into();
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(json!({ "message": m, "detail": m }))).into_response()
}

/// autobrr's `StatusResponseMessage`: `{message}`.
fn status_message(status: StatusCode, m: &str) -> Response {
    (status, Json(json!({ "message": m }))).into_response()
}

/// A plain-text answer, as autobrr's health routes write them.
fn plain(status: StatusCode, text: &'static str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain")], text).into_response()
}

// ---- auth ----

/// The body of `POST /auth/login` (autobrr's `UserLoginRequest`).
#[derive(Deserialize, Default)]
#[serde(default)]
struct LoginRequest {
    /// The account's name.
    username: String,
    /// Its password.
    password: String,
    /// Keep the cookie after the browser closes.
    remember_me: bool,
}

/// The address a request came from, for the sign-in limit (Lantern's own rule).
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "local".into())
}

/// The account `username` (any case) when `password` is its password; `None` otherwise (autobrr's `Login`).
async fn check_login(st: &AppState, username: &str, password: &str) -> Option<kit_server_core::Row> {
    if username.is_empty() || password.is_empty() {
        return None;
    }
    let u = st.db.one("SELECT * FROM users WHERE username=? COLLATE NOCASE", p![username.trim()]).ok().flatten()?;
    let (pw, hash) = (password.to_string(), u.s("password_hash"));
    let ok = tokio::task::spawn_blocking(move || crate::auth::verify_password(&pw, &hash)).await.unwrap_or(false);
    (ok && !u.b("disabled")).then_some(u)
}

/// `POST /auth/login`: Lantern's session cookie for the account, a cookie that outlives the browser only with
/// `remember_me` (autobrr's `RememberMe`). 10 tries per address in 5 minutes, as Lantern's own sign-in.
async fn login(State(st): State<AppState>, headers: HeaderMap, jar: CookieJar, body: Bytes) -> Response {
    let req: LoginRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return status_error(400, format!("could not decode json: {e}")),
    };
    let key = format!("login:{}", client_ip(&headers));
    if st.limits.limited(&key, 10, 300.0, false) {
        return status_error(429, "Too many sign-in attempts. Wait a few minutes, then try again.");
    }
    let Some(user) = check_login(&st, &req.username, &req.password).await else {
        st.limits.record(&key);
        tracing::error!("autobrr: failed login attempt for {}", req.username);
        return status_error(403, "could not login: bad credentials");
    };
    let days = st.settings.int("session_days").max(1);
    let max_age = days * 86400;
    let token = crate::auth::random_token(32);
    let now = crate::auth::now();
    if let Err(e) = st.db.execute(
        "INSERT INTO sessions (token, user_id, kind, created_at, expires_at) VALUES (?,?,'web',?,?)",
        p![token, user.i("id"), now, now + max_age as f64],
    ) {
        tracing::error!("autobrr: could not save the session: {}", e.detail);
        return status_error(500, "could not renew session token");
    }
    let https = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let mut cookie = Cookie::build((crate::auth::cookie_name(), token)).path("/").http_only(true).same_site(SameSite::Lax).secure(https);
    if req.remember_me {
        cookie = cookie.max_age(cookie::time::Duration::seconds(max_age));
    }
    (jar.add(cookie.build()), StatusCode::NO_CONTENT).into_response()
}

/// `POST /auth/logout`: ends this session (autobrr's `Destroy`).
async fn logout(State(st): State<AppState>, jar: CookieJar) -> Response {
    if let Some(t) = jar.get(&crate::auth::cookie_name()) {
        if let Err(e) = st.db.execute("DELETE FROM sessions WHERE token=? AND kind='web'", p![t.value()]) {
            return status_error(500, e.detail);
        }
    }
    (jar.remove(Cookie::build(crate::auth::cookie_name()).path("/")), StatusCode::NO_CONTENT).into_response()
}

/// Whether onboarding could run: no account yet (autobrr's `onboardEligible`).
fn onboard_eligible(st: &AppState) -> Result<(), (u16, String)> {
    match st.db.one("SELECT COUNT(*) AS n FROM users", p![]) {
        Ok(r) if r.as_ref().map(|r| r.i("n")).unwrap_or(0) > 0 => Err((503, "onboarding unavailable: user already registered".into())),
        Ok(_) => Ok(()),
        Err(_) => Err((500, "could not get user count".into())),
    }
}

/// `GET /auth/onboard`: 204 while there is no account yet; 503 with autobrr's words as a JSON string once there
/// is one (autobrr's `canOnboard`).
async fn can_onboard(State(st): State<AppState>) -> Response {
    match onboard_eligible(&st) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((503, m)) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!(m))).into_response(),
        Err((s, m)) => status_error(s, m),
    }
}

/// `POST /auth/onboard`: with an account there, autobrr's 503; without one, refused, as Lantern's first account
/// is made on its own "Create your account" page with the setup code from the log (a page anyone could reach
/// first must not make the owner).
async fn onboard(State(st): State<AppState>) -> Response {
    match onboard_eligible(&st) {
        Ok(()) => status_error(
            403,
            "Create the first account on Lantern's own page with the setup code from the log, then sign in here.",
        ),
        Err((s, m)) => status_error(s, m),
    }
}

/// `GET /auth/validate`: who is signed in and how (`password` for Lantern's session; empty for an API key, which
/// autobrr's session doesn't hold either).
async fn validate(_: SignedIn, via: Option<Extension<Via>>) -> Response {
    let (username, method) = match via.map(|Extension(v)| v) {
        Some(Via::Session(u)) => (u, "password"),
        _ => (String::new(), ""),
    };
    (StatusCode::OK, Json(json!({ "username": username, "auth_method": method }))).into_response()
}

/// The body of `PATCH /auth/user/{username}` (autobrr's `UpdateUserRequest`).
#[derive(Deserialize, Default)]
#[serde(default)]
struct UpdateUserRequest {
    /// The new name (empty: unchanged).
    username_new: String,
    /// The password now.
    password_current: String,
    /// The new password (empty: unchanged).
    password_new: String,
}

/// `PATCH /auth/user/{username}`: a new name or password for the signed-in account, after its current password
/// (autobrr's `UpdateUser`). Only the signed-in account itself can be changed here, and the new name and
/// password follow Lantern's rules.
async fn update_user(State(st): State<AppState>, SignedIn(me): SignedIn, Path(username): Path<String>, body: Bytes) -> Response {
    let req: UpdateUserRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return status_error(400, format!("could not decode json: {e}")),
    };
    if req.password_current.is_empty() {
        return status_error(403, "validation error: empty current password supplied");
    }
    if !req.password_new.is_empty() && req.password_new == req.password_current {
        return status_error(403, "validation error: new password must be different");
    }
    if !username.eq_ignore_ascii_case(&me.username) {
        return status_error(403, format!("invalid login: {username}"));
    }
    let Some(user) = check_login(&st, &username, &req.password_current).await else {
        return status_error(403, format!("invalid login: {username}"));
    };
    let new_name = if req.username_new.is_empty() { user.s("username") } else { req.username_new.clone() };
    let check_pw = if req.password_new.is_empty() { "unchanged-password" } else { req.password_new.as_str() };
    if let Err(e) = crate::auth::validate_credentials(&new_name, check_pw) {
        return status_error(403, e.detail);
    }
    if !req.username_new.is_empty() {
        let taken = st.db.one("SELECT id FROM users WHERE username=? COLLATE NOCASE AND id<>?", p![new_name.as_str(), user.i("id")]);
        if taken.ok().flatten().is_some() {
            return status_error(403, "Username already taken");
        }
    }
    let hash = if req.password_new.is_empty() {
        user.s("password_hash")
    } else {
        let pw = req.password_new.clone();
        tokio::task::spawn_blocking(move || crate::auth::hash_password(&pw)).await.unwrap_or_default()
    };
    if let Err(e) = st.db.execute("UPDATE users SET username=?, password_hash=? WHERE id=?", p![new_name, hash, user.i("id")]) {
        tracing::error!("autobrr: could not change the account: {}", e.detail);
        return status_error(403, "failed to change password");
    }
    status_message(StatusCode::OK, "user successfully updated")
}

/// `/auth/oidc/*`: autobrr mounts these only with OIDC on; Lantern has no OIDC sign-in, so they answer 404 as
/// autobrr's do then (its frontend reads a 404 as "OIDC is off").
async fn oidc_off() -> Response {
    kit_server_core::err(404, "Lantern doesn't sign in with OIDC. Sign in with your Lantern account.").into_response()
}

// ---- health ----

/// `GET /healthz/liveness`: `OK`.
async fn liveness() -> Response {
    plain(StatusCode::OK, "OK")
}

/// `GET /healthz/readiness`: `OK` when the database answers, else autobrr's 500 text.
async fn readiness(State(st): State<AppState>) -> Response {
    match st.db.one("SELECT 1 AS ok", p![]) {
        Ok(Some(_)) => plain(StatusCode::OK, "OK"),
        _ => plain(StatusCode::INTERNAL_SERVER_ERROR, "Unhealthy. Database unreachable"),
    }
}

// ---- config ----

/// `GET /config` (autobrr's `configJson`): the config file's settings as they map onto Lantern: its port, data
/// folder (`config_dir`), SQLite database and base URL, the log and update settings, and the build.
async fn get_config(State(st): State<AppState>, _: SignedIn) -> Answer {
    let c = st.releases.app_config();
    let application = std::env::current_exe().map_err(|e| ApiError::Internal(e.to_string()))?;
    let data = std::path::absolute(&st.data_dir).unwrap_or_else(|_| st.data_dir.clone());
    send(
        StatusCode::OK,
        json!({
            "application": application.display().to_string(),
            "config_dir": data.display().to_string(),
            "database": "sqlite",
            "host": "0.0.0.0",
            "port": crate::port(),
            "log_level": c.log_level,
            "log_path": c.log_path,
            "log_max_size": LOG_MAX_SIZE,
            "log_max_backups": LOG_MAX_BACKUPS,
            "base_url": crate::autobrr_ui::URL_BASE,
            "check_for_updates": c.check_for_updates,
            "version": crate::VERSION,
            "commit": option_env!("APP_COMMIT").unwrap_or(""),
            "date": option_env!("APP_BUILD_DATE").unwrap_or(""),
        }),
    )
}

/// `PATCH /config`: the log level, log file and update checks (204); a failure is autobrr's `{message, status}`.
async fn update_config(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let u: ConfigUpdate = decode(&body)?;
    match st.releases.update_app_config(&u, &st.data_dir) {
        Ok(_) => no_content(),
        Err(e) => Ok((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "message": e.message(), "status": 500, "detail": e.message() }))).into_response()),
    }
}

// ---- updates ----

/// `GET /updates/latest`: the newer Lantern release found, as autobrr's `Release`; 204 when none.
async fn latest_release(State(st): State<AppState>, _: SignedIn) -> Answer {
    match st.releases.updates.latest() {
        Some(r) => send(StatusCode::OK, super::system::release_json(&r)),
        None => no_content(),
    }
}

/// `GET /updates/check`: looks for a newer release now (204; a failure is only logged).
async fn check_updates(State(st): State<AppState>, _: SignedIn) -> Answer {
    st.releases.updates.check_update_available().await;
    no_content()
}

// ---- API keys ----

async fn list_keys(State(st): State<AppState>, _: Admin) -> Answer {
    send(StatusCode::OK, st.releases.api_keys()?)
}

async fn store_key(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let mut k: ApiKey = decode(&body)?;
    st.releases.store_api_key(&mut k)?;
    send(StatusCode::CREATED, k)
}

async fn delete_key(State(st): State<AppState>, _: Admin, Path(key): Path<String>) -> Answer {
    match st.releases.delete_api_key(&key) {
        Ok(()) => no_content(),
        Err(ApiError::NotFound(_)) => Err(Fail(ApiError::NotFound(format!("api key {key} not found")))),
        Err(e) => Err(Fail(e)),
    }
}

// ---- proxies ----

async fn list_proxies(State(st): State<AppState>, _: SignedIn) -> Answer {
    send(StatusCode::OK, st.releases.proxies()?)
}

async fn store_proxy(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let mut p: Proxy = decode(&body)?;
    st.releases.store_proxy(&mut p)?;
    no_content()
}

async fn update_proxy(State(st): State<AppState>, _: Admin, Path(_id): Path<String>, body: Bytes) -> Answer {
    let mut p: Proxy = decode(&body)?;
    match st.releases.update_proxy(&mut p)? {
        true => no_content(),
        false => Ok(status_error(400, "update failed")),
    }
}

async fn find_proxy(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "proxyID")?;
    match st.releases.proxy(id) {
        Ok(p) => send(StatusCode::OK, p),
        Err(ApiError::NotFound(_)) => Err(Fail(ApiError::NotFound(format!("could not find proxy with id {id}")))),
        Err(e) => Err(Fail(e)),
    }
}

async fn proxy_usage(State(st): State<AppState>, _: SignedIn, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "proxyID")?;
    send(StatusCode::OK, st.releases.proxy_usage(id)?)
}

async fn delete_proxy(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Answer {
    let id = id_of(&id, "proxyID")?;
    match st.releases.delete_proxy(id)? {
        true => no_content(),
        false => Ok(status_error(400, "delete failed")),
    }
}

async fn test_proxy(State(st): State<AppState>, _: Admin, body: Bytes) -> Answer {
    let mut p: Proxy = decode(&body)?;
    match st.releases.test_proxy(&mut p).await {
        Ok(()) => no_content(),
        Err(ApiError::NotFound(m)) if m == ERR_NO_ROWS => Err(Fail(ApiError::NotFound("could not find proxy".into()))),
        Err(e) => Err(Fail(ApiError::Internal(e.message().to_string()))),
    }
}
