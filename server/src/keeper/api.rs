//! The account keeper's routes (admins; the API key too, so a script can send an account's numbers).
//!
//! ```text
//! GET  /api/keeper                    → {accounts, warn_days, channels}: every vault account with its rules, numbers and risks
//! GET  /api/keeper/{id}               → one account
//! PUT  /api/keeper/{id}/rules         {indexer_id, keep_alive, min_ratio, max_hnr, hnr_hours, login_days, activity_days, login_expires, cookie_expires}
//! PUT  /api/keeper/{id}/stats         {uploaded, downloaded (bytes), ratio, seed_hours, bonus, hnr_count, signed_in: true, active: true}
//! GET  /api/keeper/{id}/history       → {history}: the numbers each time they changed
//! POST /api/keeper/{id}/sign-in       → the account, after Lantern signed in through its indexer
//! POST /api/keeper/test-alerts        → {results: [{channel, ok, message}]}: a test message on every alert channel
//! ```

use super::alerts::Channels;
use crate::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result};
use serde_json::{json, Value};

/// The keeper's routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/keeper", get(list))
        .route("/api/keeper/test-alerts", post(test_alerts))
        .route("/api/keeper/{id}", get(one))
        .route("/api/keeper/{id}/rules", put(rules))
        .route("/api/keeper/{id}/stats", put(stats))
        .route("/api/keeper/{id}/history", get(history))
        .route("/api/keeper/{id}/sign-in", post(sign_in))
}

/// The warning days in effect.
fn warn_days(st: &AppState) -> f64 {
    st.settings.int("keeper_warn_days").max(1) as f64
}

/// Account `a` as answered now.
fn answer(st: &AppState, a: &super::Account) -> Value {
    super::public(a, crate::auth::now(), warn_days(st))
}

/// `GET /api/keeper`.
async fn list(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    let accounts: Vec<Value> = super::accounts(&st)?.iter().map(|a| answer(&st, a)).collect();
    let ch = Channels::from_settings(&super::alert_settings(&st));
    Ok(Json(json!({
        "accounts": accounts,
        "warn_days": warn_days(&st),
        "channels": { "ntfy": !ch.ntfy_url.is_empty(), "discord": !ch.discord.is_empty(), "email": ch.email.is_some() },
    })))
}

/// `GET /api/keeper/{id}`.
async fn one(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(answer(&st, &super::get(&st, &id)?)))
}

/// `PUT /api/keeper/{id}/rules`.
async fn rules(State(st): State<AppState>, _: Admin, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>> {
    let a = super::set_rules(&st, &id, &body)?;
    Ok(Json(answer(&st, &a)))
}

/// `PUT /api/keeper/{id}/stats`.
async fn stats(State(st): State<AppState>, _: Admin, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>> {
    let a = super::set_stats(&st, &id, &body)?;
    Ok(Json(answer(&st, &a)))
}

/// `GET /api/keeper/{id}/history`.
async fn history(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Result<Json<Value>> {
    st.vault.get(&id)?;
    Ok(Json(json!({ "history": super::history(&st, &id, 365)? })))
}

/// `POST /api/keeper/{id}/sign-in`.
async fn sign_in(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Result<Json<Value>> {
    let a = super::get(&st, &id)?;
    super::sign_in(&st, &a).await?;
    Ok(Json(answer(&st, &super::get(&st, &id)?)))
}

/// `POST /api/keeper/test-alerts`: a test message through every channel that is set; 400 when none is.
async fn test_alerts(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    let ch = Channels::from_settings(&super::alert_settings(&st));
    if !ch.any() {
        return Err(err(
            400,
            "No alert channel is set. Add an ntfy topic, a Discord webhook or an email address under Settings → Accounts → Alerts.",
        ));
    }
    let sent = ch.send("Lantern: test alert", "This is how Lantern tells you when an account is at risk.", false).await;
    let results: Vec<Value> = sent
        .into_iter()
        .map(|(who, r)| match r {
            Ok(()) => json!({ "channel": who, "ok": true, "message": format!("Sent through {who}.") }),
            Err(why) => json!({ "channel": who, "ok": false, "message": why }),
        })
        .collect();
    Ok(Json(json!({ "results": results })))
}
