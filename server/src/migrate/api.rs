//! The import routes (an admin only; each answers a [`Plan`](super::Plan)).
//!
//! ```text
//! POST /api/import/prowlarr        multipart: file (a Prowlarr backup .zip or prowlarr.db), dry_run, take_api_key
//! POST /api/import/prowlarr/api    {url, key, dry_run, take_api_key}: read a running Prowlarr through its API
//! POST /api/import/flaresolverr    {text}: FlareSolverr's docker-compose service, .env or docker run command
//! GET  /api/import/apps            the apps imported for app sync (API keys masked)
//! ```
//!
//! `dry_run` is true unless it is sent as false: a look first, then the same request again to import.

use super::{flaresolverr, prowlarr, InVault, Options};
use crate::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use kit_server_core::{err, Admin, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

/// The biggest Prowlarr backup or database the upload takes.
pub const MAX_UPLOAD: usize = 2 << 30;

/// The import routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/import/prowlarr", post(prowlarr_file).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/api/import/prowlarr/api", post(prowlarr_api))
        .route("/api/import/flaresolverr", post(flaresolverr_text))
        .route("/api/import/apps", get(apps))
}

/// The port of Lantern's FlareSolverr API: `APP_SOLVER_PORT`, 8191 when it is not set.
pub fn solver_port() -> u16 {
    std::env::var("APP_SOLVER_PORT").ok().and_then(|p| p.trim().parse().ok()).unwrap_or(flaresolverr::PORT)
}

/// A yes/no from a form or JSON value: only `false`, `0`, `no` and `off` are no; missing is `default`.
fn flag(v: Option<&str>, default: bool) -> bool {
    match v.map(|s| s.trim().to_lowercase()) {
        Some(s) if s.is_empty() => default,
        Some(s) => !matches!(s.as_str(), "false" | "0" | "no" | "off"),
        None => default,
    }
}

/// Files that are removed when dropped (an upload and what was unpacked from it), with SQLite's side files.
struct Scratch(Vec<PathBuf>);

impl Drop for Scratch {
    fn drop(&mut self) {
        for p in &self.0 {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", p.display()));
            }
        }
    }
}

/// A new file name in `<data>/import`, made when missing.
fn scratch_name(st: &AppState, what: &str) -> Result<PathBuf> {
    use rand::Rng;
    let dir = st.data_dir.join("import");
    std::fs::create_dir_all(&dir).map_err(|e| err(500, format!("The data folder can't be written: {e}")))?;
    Ok(dir.join(format!("{what}-{:016x}", rand::rng().random::<u64>())))
}

async fn prowlarr_file(State(st): State<AppState>, _: Admin, mut form: Multipart) -> Result<Json<Value>> {
    let upload = scratch_name(&st, "upload")?;
    let unpacked = scratch_name(&st, "prowlarr")?;
    let _clean = Scratch(vec![upload.clone(), unpacked.clone()]);
    let mut opts = Options::default();
    let mut got_file = false;
    let bad_upload = |e: axum::extract::multipart::MultipartError| err(400, format!("The upload stopped before the end ({e}). Try again."));
    while let Some(mut field) = form.next_field().await.map_err(bad_upload)? {
        match field.name().unwrap_or("") {
            "file" => {
                let mut out =
                    tokio::fs::File::create(&upload).await.map_err(|e| err(500, format!("The data folder can't be written: {e}")))?;
                while let Some(chunk) = field.chunk().await.map_err(bad_upload)? {
                    out.write_all(&chunk).await.map_err(|e| err(500, format!("The data folder can't be written: {e}")))?;
                }
                out.flush().await.map_err(|e| err(500, format!("The data folder can't be written: {e}")))?;
                got_file = true;
            }
            "dry_run" => opts.dry_run = flag(field.text().await.ok().as_deref(), true),
            "take_api_key" => opts.take_api_key = flag(field.text().await.ok().as_deref(), true),
            _ => {}
        }
    }
    if !got_file {
        return Err(err(400, "Choose a Prowlarr backup (.zip) or its database (prowlarr.db) to import."));
    }
    let src = tokio::task::spawn_blocking(move || prowlarr::read_upload(&upload, &unpacked))
        .await
        .map_err(|e| err(500, format!("Reading the upload stopped: {e}")))??;
    Ok(Json(json!(prowlarr::run(&st, &src, opts, &InVault::new(&st.vault)).await?)))
}

async fn prowlarr_api(State(st): State<AppState>, _: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let text = |k: &str| body.get(k).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
    let opts = Options { dry_run: flag(text("dry_run").as_deref(), true), take_api_key: flag(text("take_api_key").as_deref(), true) };
    let src = prowlarr::read_api(&text("url").unwrap_or_default(), &text("key").unwrap_or_default()).await?;
    Ok(Json(json!(prowlarr::run(&st, &src, opts, &InVault::new(&st.vault)).await?)))
}

async fn flaresolverr_text(_: Admin, Json(body): Json<Value>) -> Result<Json<Value>> {
    let text = body.get("text").and_then(Value::as_str).unwrap_or("");
    Ok(Json(json!(flaresolverr::plan(text, solver_port())?)))
}

async fn apps(State(st): State<AppState>, _: Admin) -> Result<Json<Value>> {
    Ok(Json(json!({ "apps": super::imported_apps(&st.db)? })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn flags_default_and_only_clear_noes_are_no() {
        assert!(super::flag(None, true));
        assert!(super::flag(Some(""), true));
        assert!(!super::flag(Some("false"), true));
        assert!(!super::flag(Some(" Off "), true));
        assert!(super::flag(Some("true"), false));
    }
}
