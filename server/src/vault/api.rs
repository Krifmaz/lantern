//! The vault's routes. Admins list, add, change and remove accounts; only the owner sees secret values, and only
//! after typing their password again (5 wrong tries per 5 minutes, then a wait).
//!
//! ```text
//! GET    /api/vault                     → {open, locked_because, key_source, items, kinds, fields}
//! GET    /api/vault/items               → {items}: plain fields with values, secrets by name only
//! POST   /api/vault/items               {kind, name, site, fields} → the item
//! GET    /api/vault/items/{id}          → the item
//! PUT    /api/vault/items/{id}          {name?, site?, fields?}: an empty or masked secret keeps the saved one, null removes
//! DELETE /api/vault/items/{id}          → {ok}
//! POST   /api/vault/items/{id}/reveal   {password} → the item with every value, and the 2FA code now (owner)
//! POST   /api/vault/backup              {password, passphrase} → the encrypted backup file (owner)
//! POST   /api/vault/restore             multipart: file, password, passphrase, dry_run → what was (or would be) done (owner)
//! ```

use super::{Restored, FIELDS, KINDS, MIN_PASSPHRASE};
use crate::AppState;
use axum::extract::{Multipart, Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, p, Account, Admin, Db, Result, RowExt};
use serde::Deserialize;
use serde_json::{json, Value};

/// The vault's routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/vault", get(status))
        .route("/api/vault/items", get(list).post(create))
        .route("/api/vault/items/{id}", get(one).put(change).delete(remove))
        .route("/api/vault/items/{id}/reveal", post(reveal))
        .route("/api/vault/backup", post(backup))
        .route("/api/vault/restore", post(restore))
}

/// How many wrong passwords an account may type in [`WINDOW`] seconds before the vault makes it wait.
const TRIES: usize = 5;

/// The window of [`TRIES`], in seconds.
const WINDOW: f64 = 300.0;

/// Checks that `account` is the owner and that `password` is their password; 403, 401 or 429 in words otherwise.
pub async fn confirm_password(st: &AppState, account: &Account, password: &str) -> Result<()> {
    if !account.owner {
        return Err(err(403, "Only the owner can see saved passwords and make or restore vault backups."));
    }
    let key = format!("vault:{}", account.id);
    if st.limits.limited(&key, TRIES, WINDOW, false) {
        return Err(err(429, "Too many wrong passwords. Wait five minutes, then try again."));
    }
    let hash = st.db.one("SELECT password_hash FROM users WHERE id=?", p![account.id])?.map(|r| r.s("password_hash")).unwrap_or_default();
    let pw = password.to_string();
    let ok = !pw.is_empty() && tokio::task::spawn_blocking(move || crate::auth::verify_password(&pw, &hash)).await.unwrap_or(false);
    if !ok {
        st.limits.record(&key);
        return Err(err(401, "That isn't your password. Type the password you sign in to Lantern with."));
    }
    Ok(())
}

/// `GET /api/vault`: whether the vault is open, and the kinds and fields it offers.
async fn status(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    let mut v = st.vault.status()?;
    v["kinds"] = json!(KINDS.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>());
    v["fields"] = json!(FIELDS);
    v["min_passphrase"] = json!(MIN_PASSPHRASE);
    Ok(Json(v))
}

/// `GET /api/vault/items`.
async fn list(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    Ok(Json(json!({ "items": st.vault.list()? })))
}

/// `POST /api/vault/items`.
async fn create(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    Ok(Json(st.vault.create(&body)?))
}

/// `GET /api/vault/items/{id}`.
async fn one(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Result<Json<Value>> {
    Ok(Json(st.vault.get(&id)?))
}

/// `PUT /api/vault/items/{id}`.
async fn change(State(st): State<AppState>, _: Admin, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>> {
    Ok(Json(st.vault.update(&id, &body)?))
}

/// `DELETE /api/vault/items/{id}`.
async fn remove(State(st): State<AppState>, _: Admin, Path(id): Path<String>) -> Result<Json<Value>> {
    st.vault.remove(&id)?;
    Ok(Json(json!({ "ok": true })))
}

/// The body of the routes that ask for the password again.
#[derive(Deserialize)]
struct Reauth {
    /// the signed-in account's password
    #[serde(default)]
    password: String,
    /// the backup's passphrase
    #[serde(default)]
    passphrase: String,
}

/// `POST /api/vault/items/{id}/reveal`.
async fn reveal(State(st): State<AppState>, Admin(a): Admin, Path(id): Path<String>, Json(b): Json<Reauth>) -> Result<Json<Value>> {
    st.vault.get(&id)?;
    confirm_password(&st, &a, &b.password).await?;
    Ok(Json(st.vault.reveal(&id, &a.username)?))
}

/// `POST /api/vault/backup`: the backup as a download, `lantern-vault-<date>.json`.
async fn backup(State(st): State<AppState>, Admin(a): Admin, Json(b): Json<Reauth>) -> Result<Response> {
    confirm_password(&st, &a, &b.password).await?;
    let vault = st.vault.clone();
    let file = tokio::task::spawn_blocking(move || vault.export(&b.passphrase)).await.map_err(|e| err(500, e.to_string()))??;
    let body = serde_json::to_string_pretty(&file).map_err(|e| err(500, e.to_string()))?;
    let name = format!("{}-vault-{}.json", crate::NAME, chrono::Utc::now().format("%Y-%m-%d"));
    Ok((
        [(header::CONTENT_TYPE, "application/json".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))],
        body,
    )
        .into_response())
}

/// `POST /api/vault/restore`: a multipart form with the backup `file`, `password`, `passphrase` and `dry_run`
/// (`true` to only see what would change).
async fn restore(State(st): State<AppState>, Admin(a): Admin, mut form: Multipart) -> Result<Json<Value>> {
    let (mut file, mut password, mut passphrase, mut dry_run) = (None, String::new(), String::new(), false);
    let unreadable = |_| err(400, "The form couldn't be read. Choose the backup file again and retry.");
    while let Some(part) = form.next_field().await.map_err(unreadable)? {
        let name = part.name().unwrap_or_default().to_string();
        let text = part.text().await.map_err(unreadable)?;
        match name.as_str() {
            "file" => file = Some(text),
            "password" => password = text,
            "passphrase" => passphrase = text,
            "dry_run" => dry_run = matches!(text.as_str(), "true" | "1" | "on"),
            _ => {}
        }
    }
    confirm_password(&st, &a, &password).await?;
    let file = file.filter(|f| !f.trim().is_empty()).ok_or_else(|| err(400, "Choose the backup file to restore."))?;
    let backup: Value = serde_json::from_str(&file)
        .map_err(|_| err(400, "This file isn't a Lantern vault backup. Choose the .json file that Back up made."))?;
    let vault = st.vault.clone();
    let done: Restored =
        tokio::task::spawn_blocking(move || vault.restore(&backup, &passphrase, dry_run)).await.map_err(|e| err(500, e.to_string()))??;
    Ok(Json(json!({ "dry_run": dry_run, "result": done, "message": restore_words(&done, dry_run) })))
}

/// What a restore did (or would do), in a sentence.
pub fn restore_words(r: &Restored, dry_run: bool) -> String {
    let mut parts = Vec::new();
    let n = |k: usize| if k == 1 { "1 account".to_string() } else { format!("{k} accounts") };
    if !r.added.is_empty() {
        parts.push(format!("{} added", n(r.added.len())));
    }
    if !r.updated.is_empty() {
        parts.push(format!("{} replaced by the backup's copy", n(r.updated.len())));
    }
    if !r.kept_newer.is_empty() {
        parts.push(format!("{} kept because they changed here after the backup", n(r.kept_newer.len())));
    }
    if !r.unchanged.is_empty() {
        parts.push(format!("{} already the same", n(r.unchanged.len())));
    }
    if parts.is_empty() {
        return "The backup is empty: nothing to restore.".into();
    }
    let what = parts.join(", ");
    if dry_run {
        format!("A restore would do this: {what}. Nothing has changed yet.")
    } else {
        format!("Restored: {what}.")
    }
}
