//! Signing in, cut down to what every app
//! needs: accounts with bcrypt passwords, an owner made at the first start, a web session cookie, the
//! server's API key (`x-api-key`) and the CSRF rule (`X-Requested-With: <project name>`). The kit's words throughout.
//!
//! ```text
//! POST /api/auth/login   {username, password} → the account, and the session cookie
//!                        {username, password, app: {device_id, device_name}} → {user, token}: a phone's app token
//!                        (`Authorization: Bearer <token>`), kept as its SHA-256 only
//! POST /api/auth/logout  → {ok}, the cookie (or the app token) removed
//! GET  /api/auth/me      → the signed-in account (401 "Not authenticated")
//! ```

use crate::AppState;
use axum::extract::State;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use kit_server_core::{err, p, Account, Db, PrefsStore, Result, Row, RowExt, SqliteDb};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// The session cookie's name: `<app>_session` (`notes_session` for `notes`).
pub fn cookie_name() -> String {
    format!("{}_session", crate::NAME.replace('-', "_"))
}

/// Seconds since 1970, how the tables keep times.
pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `bytes` random bytes as URL-safe base64.
pub fn random_token(bytes: usize) -> String {
    use base64::Engine;
    use rand::Rng;
    let mut b = vec![0u8; bytes];
    rand::rng().fill(&mut b[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// How long an app token lasts after its last use: a year.
pub const APP_SESSION_SECS: f64 = 365.0 * 86400.0;

/// The `sessions.token` of an app token: its SHA-256, so the database never holds a usable token.
pub fn app_token_key(token: &str) -> String {
    use sha2::Digest;
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(token.as_bytes())))
}

/// The token of an `Authorization: Bearer <token>` header.
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = v.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

/// The app's own account and session tables (the kit keeps its own data, preferences among it, in `kit_*`
/// tables; it never reads these).
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT UNIQUE NOT NULL,
            email TEXT,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'user',
            disabled INTEGER NOT NULL DEFAULT 0,
            created_at REAL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            kind TEXT NOT NULL DEFAULT 'web',
            created_at REAL,
            expires_at REAL
        );",
    )
    .map_err(|e| anyhow::anyhow!("The database tables can't be made: {}", e.detail))
}

/// A bcrypt hash of `pw` (its first 72 bytes, as bcrypt reads them).
pub fn hash_password(pw: &str) -> String {
    let b = &pw.as_bytes()[..pw.len().min(72)];
    bcrypt::hash(b, 12).unwrap_or_default()
}

/// Whether `pw` matches the bcrypt hash `hashed`.
pub fn verify_password(pw: &str, hashed: &str) -> bool {
    let b = &pw.as_bytes()[..pw.len().min(72)];
    bcrypt::verify(b, hashed).unwrap_or(false)
}

/// A username of 3 to 32 letters, digits, `_`, `-` and `.`, and a password of 8 characters or more; 400
/// otherwise.
pub fn validate_credentials(username: &str, password: &str) -> Result<()> {
    let ok_name = (3..=32).contains(&username.chars().count()) && username.chars().all(|c| c.is_alphanumeric() || "_-.".contains(c));
    if !ok_name {
        return Err(err(400, "Username must be 3-32 characters: letters, numbers, . _ -"));
    }
    if password.chars().count() < 8 {
        return Err(err(400, "Password must be at least 8 characters"));
    }
    Ok(())
}

/// Make an account; its id. 409 when the name is taken.
pub fn create_user(db: &SqliteDb, username: &str, password: &str, role: &str) -> Result<i64> {
    validate_credentials(username, password)?;
    if db.one("SELECT id FROM users WHERE username=?", p![username])?.is_some() {
        return Err(err(409, "Username already taken"));
    }
    db.execute(
        "INSERT INTO users (username, password_hash, role, created_at) VALUES (?,?,?,?)",
        p![username, hash_password(password), role, now()],
    )
}

/// First start: make sure there is an owner. `user` defaults to `admin`; without a `password` one is made,
/// printed once and written to `<data>/initial_admin_password.txt` (readable by the server's user only).
pub fn ensure_owner(st: &AppState, user: Option<String>, password: Option<String>) -> anyhow::Result<()> {
    if st.db.one("SELECT id FROM users WHERE role='owner'", p![]).map_err(|e| anyhow::anyhow!(e.detail))?.is_some() {
        return Ok(());
    }
    let username = user.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "admin".into());
    let (password, generated) = match password {
        Some(p) if !p.trim().is_empty() => (p, false),
        _ => (random_token(12), true),
    };
    create_user(&st.db, &username, &password, "owner").map_err(|e| anyhow::anyhow!(e.detail))?;
    if generated {
        let f = st.data_dir.join("initial_admin_password.txt");
        std::fs::write(&f, format!("{username}\n{password}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))?;
        }
        println!("{}\nCreated owner account '{username}' with password: {password}\n{}", "=".repeat(70), "=".repeat(70));
    }
    tracing::info!("Created owner account '{username}'");
    Ok(())
}

/// Events per key in a sliding window: sign-in tries per address, bug reports per
/// account.
#[derive(Default)]
pub struct Limits {
    /// the times of each key's recent events
    events: Mutex<HashMap<String, Vec<f64>>>,
}

impl Limits {
    /// True when `key` already had `limit` events in the last `window` seconds. With `record`, an event that
    /// is let through counts at once (bug reports); without, the caller records failures itself ([`Limits::record`]).
    pub fn limited(&self, key: &str, limit: usize, window: f64, record: bool) -> bool {
        let t = now();
        let mut m = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let v = m.entry(key.to_string()).or_default();
        v.retain(|&at| t - at < window);
        if v.len() >= limit {
            return true;
        }
        if record {
            v.push(t);
        }
        false
    }

    /// Count one event for `key` now (a failed sign-in).
    pub fn record(&self, key: &str) {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).entry(key.to_string()).or_default().push(now());
    }
}

/// Whether `a` and `b` are equal, in time that does not depend on where they differ.
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The signed-in account of a request: the API key (as the owner), an app token, else the session cookie. A cookie
/// request that changes something must carry `X-Requested-With: <app>` (CSRF).
pub fn signed_in(st: &AppState, parts: &Parts) -> Result<Account> {
    let (mut row, cookie) = user_of(st, &parts.headers)?.ok_or_else(|| err(401, "Not authenticated"))?;
    if cookie && !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        let ok = parts.headers.get("x-requested-with").and_then(|v| v.to_str().ok()) == Some(crate::NAME);
        if !ok {
            return Err(err(403, "Missing CSRF header"));
        }
    }
    row.remove("password_hash");
    Ok(Account::from_row(&row))
}

/// The account these headers sign in as, and whether through the cookie; `None` when they don't.
fn user_of(st: &AppState, headers: &HeaderMap) -> Result<Option<(Row, bool)>> {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        let want = st.settings.str("api_key");
        if !want.is_empty() && ct_eq(key, &want) {
            let u = st
                .db
                .one("SELECT * FROM users WHERE role IN ('owner','admin') AND disabled=0 ORDER BY role='owner' DESC, id LIMIT 1", p![])?;
            return Ok(u.map(|u| (u, false)));
        }
    }
    // a bearer token is used on its own: a wrong or expired one is a 401, never a fall back to the cookie
    if let Some(token) = bearer(headers) {
        let Some(s) = st.db.one("SELECT * FROM sessions WHERE token=? AND kind='app'", p![app_token_key(&token)])? else { return Ok(None) };
        if s.f("expires_at") < now() {
            return Ok(None);
        }
        return Ok(st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![s.i("user_id")])?.map(|u| (u, false)));
    }
    let jar = CookieJar::from_headers(headers);
    let Some(token) = jar.get(&cookie_name()).map(|c| c.value().to_string()) else { return Ok(None) };
    let Some(s) = st.db.one("SELECT * FROM sessions WHERE token=? AND kind='web'", p![token])? else { return Ok(None) };
    if s.f("expires_at") < now() {
        return Ok(None);
    }
    Ok(st.db.one("SELECT * FROM users WHERE id=? AND disabled=0", p![s.i("user_id")])?.map(|u| (u, true)))
}

/// An account as the web app gets it: no password hash, `is_admin`, and its `prefs` (from the kit's store).
pub async fn public_user(st: &AppState, u: &Row) -> Result<Value> {
    let mut m = u.clone();
    m.remove("password_hash");
    m.insert("is_admin".into(), json!(matches!(u.s("role").as_str(), "admin" | "owner")));
    m.insert("prefs".into(), st.prefs.prefs(u.i("id")).await?);
    Ok(Value::Object(m))
}

/// The sign-in routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/auth/login", post(login)).route("/api/auth/logout", post(logout)).route("/api/auth/me", get(me))
}

/// `POST /api/auth/login` body.
#[derive(Deserialize)]
pub struct Login {
    /// the username
    pub username: String,
    /// the password
    pub password: String,
    /// the phone app's device, for an app token instead of a cookie
    pub app: Option<AppDevice>,
}

/// The phone or computer that signs in.
#[derive(Deserialize)]
pub struct AppDevice {
    /// the app's own id for this install
    pub device_id: String,
    /// the phone's name, shown to the account
    pub device_name: Option<String>,
}

/// The address a request came from, for the sign-in limit: the first `X-Forwarded-For` hop, else "local".
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "local".into())
}

/// `POST /api/auth/login`: sign in (10 tries per address in 5 minutes) and get the session cookie.
async fn login(State(st): State<AppState>, headers: HeaderMap, jar: CookieJar, Json(b): Json<Login>) -> Result<(CookieJar, Json<Value>)> {
    if b.app.as_ref().is_some_and(|a| a.device_id.trim().is_empty()) {
        return Err(err(400, "app.device_id is required"));
    }
    let key = format!("login:{}", client_ip(&headers));
    if st.limits.limited(&key, 10, 300.0, false) {
        return Err(err(429, "Too many attempts – wait a few minutes"));
    }
    let user = st.db.one("SELECT * FROM users WHERE username=? COLLATE NOCASE", p![b.username.trim()])?;
    let user = match user {
        Some(u) => {
            let (pw, hash) = (b.password.clone(), u.s("password_hash"));
            let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &hash)).await.unwrap_or(false);
            ok.then_some(u)
        }
        None => None,
    };
    let Some(user) = user else {
        st.limits.record(&key);
        return Err(err(401, "Invalid username or password"));
    };
    if user.b("disabled") {
        return Err(err(403, "This account is disabled"));
    }
    if b.app.is_some() {
        let token = random_token(32);
        st.db.execute(
            "INSERT INTO sessions (token, user_id, kind, created_at, expires_at) VALUES (?,?,'app',?,?)",
            p![app_token_key(&token), user.i("id"), now(), now() + APP_SESSION_SECS],
        )?;
        return Ok((jar, Json(json!({ "user": public_user(&st, &user).await?, "token": token }))));
    }
    let days = st.settings.int("session_days").max(1);
    let max_age = days * 86400;
    let token = random_token(32);
    st.db.execute(
        "INSERT INTO sessions (token, user_id, kind, created_at, expires_at) VALUES (?,?,'web',?,?)",
        p![token, user.i("id"), now(), now() + max_age as f64],
    )?;
    let https = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let cookie = Cookie::build((cookie_name(), token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(https)
        .max_age(cookie::time::Duration::seconds(max_age))
        .build();
    Ok((jar.add(cookie), Json(public_user(&st, &user).await?)))
}

/// `POST /api/auth/logout`: end this session (the app token, or the cookie's).
async fn logout(State(st): State<AppState>, headers: HeaderMap, jar: CookieJar) -> Result<(CookieJar, Json<Value>)> {
    if let Some(t) = bearer(&headers) {
        st.db.execute("DELETE FROM sessions WHERE token=? AND kind='app'", p![app_token_key(&t)])?;
        return Ok((jar, Json(json!({ "ok": true }))));
    }
    if let Some(t) = jar.get(&cookie_name()) {
        st.db.execute("DELETE FROM sessions WHERE token=? AND kind='web'", p![t.value()])?;
    }
    Ok((jar.remove(Cookie::build(cookie_name()).path("/")), Json(json!({ "ok": true }))))
}

/// `GET /api/auth/me`: the signed-in account.
async fn me(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Value>> {
    let (u, _) = user_of(&st, &headers)?.ok_or_else(|| err(401, "Not authenticated"))?;
    Ok(Json(public_user(&st, &u).await?))
}
