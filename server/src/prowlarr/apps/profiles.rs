// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Profiles/AppSyncProfile.cs, AppSyncProfileService.cs,
// ProfileInUseException.cs, src/Prowlarr.Api.V1/Profiles/App/AppProfileController.cs, AppProfileResource.cs),
// GPL-3.0.

//! App profiles (`/api/v1/appprofile`): which searches an indexer is used for in the apps it is synced to (RSS,
//! automatic search, interactive search) and the fewest seeders a torrent needs there. Prowlarr makes a
//! "Standard" profile when there is none; so does Lantern.
//!
//! An indexer picks its profile with its `appProfileId` setting; an indexer without one uses profile 1.

use super::super::{accepted, body_json, check, created, fail, not_found, validate_id, validation, ApiResult, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Value};

/// The table of app profiles, with Prowlarr's `AppSyncProfiles` columns.
pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS prowlarr_app_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    enable_rss INTEGER NOT NULL,
    enable_automatic_search INTEGER NOT NULL,
    enable_interactive_search INTEGER NOT NULL,
    minimum_seeders INTEGER NOT NULL
);";

/// One app profile (Prowlarr's `AppSyncProfile`).
#[derive(Debug, Clone, PartialEq)]
pub struct AppProfile {
    /// Its number (0 for one not saved yet).
    pub id: i64,
    /// Its name.
    pub name: String,
    /// The apps may use the indexer for RSS.
    pub enable_rss: bool,
    /// The apps may use the indexer for automatic searches.
    pub enable_automatic_search: bool,
    /// The apps may use the indexer for searches a person starts.
    pub enable_interactive_search: bool,
    /// The fewest seeders a torrent needs in the apps.
    pub minimum_seeders: i64,
}

impl AppProfile {
    /// Prowlarr's default profile with `name`: every search on, at least 1 seeder (`GetDefaultProfile`).
    pub fn default_named(name: &str) -> AppProfile {
        AppProfile {
            id: 0,
            name: name.to_string(),
            enable_rss: true,
            enable_automatic_search: true,
            enable_interactive_search: true,
            minimum_seeders: 1,
        }
    }

    /// The profile as Prowlarr's `AppProfileResource`.
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "enableRss": self.enable_rss,
            "enableAutomaticSearch": self.enable_automatic_search,
            "enableInteractiveSearch": self.enable_interactive_search,
            "minimumSeeders": self.minimum_seeders,
        })
    }

    /// The profile a request's body describes (missing fields as .NET's defaults: empty, off, 0).
    fn of_json(body: &Value) -> (AppProfile, Option<String>) {
        let flag = |k: &str| body.get(k).and_then(Value::as_bool).unwrap_or(false);
        let name = body.get("name").and_then(Value::as_str).map(str::to_string);
        let profile = AppProfile {
            id: body.get("id").and_then(Value::as_i64).unwrap_or(0),
            name: name.clone().unwrap_or_default(),
            enable_rss: flag("enableRss"),
            enable_automatic_search: flag("enableAutomaticSearch"),
            enable_interactive_search: flag("enableInteractiveSearch"),
            minimum_seeders: body.get("minimumSeeders").and_then(Value::as_i64).unwrap_or(0),
        };
        (profile, name)
    }

    /// A profile from its row.
    fn of_row(r: &kit_server_core::Row) -> AppProfile {
        AppProfile {
            id: r.i("id"),
            name: r.s("name"),
            enable_rss: r.i("enable_rss") != 0,
            enable_automatic_search: r.i("enable_automatic_search") != 0,
            enable_interactive_search: r.i("enable_interactive_search") != 0,
            minimum_seeders: r.i("minimum_seeders"),
        }
    }
}

/// Makes the "Standard" profile when there is no profile (Prowlarr does this at every start).
pub fn seed(db: &SqliteDb) -> kit_server_core::Result<()> {
    if db.one("SELECT id FROM prowlarr_app_profiles LIMIT 1", p![])?.is_none() {
        tracing::info!("Setting up default app profile");
        insert(db, &AppProfile::default_named("Standard"))?;
    }
    Ok(())
}

/// Every app profile, by number.
pub fn all(db: &SqliteDb) -> kit_server_core::Result<Vec<AppProfile>> {
    Ok(db.query("SELECT * FROM prowlarr_app_profiles ORDER BY id", p![])?.iter().map(AppProfile::of_row).collect())
}

/// The profile with `id`, or Prowlarr's 404.
fn get_profile(db: &SqliteDb, id: i64) -> ApiResult<AppProfile> {
    db.one("SELECT * FROM prowlarr_app_profiles WHERE id=?", p![id])?
        .map(|r| AppProfile::of_row(&r))
        .ok_or_else(|| not_found("AppSyncProfile", id))
}

/// Saves a new profile; its number.
fn insert(db: &SqliteDb, p: &AppProfile) -> kit_server_core::Result<i64> {
    db.execute(
        "INSERT INTO prowlarr_app_profiles (name, enable_rss, enable_automatic_search, enable_interactive_search, minimum_seeders) VALUES (?,?,?,?,?)",
        p![p.name.clone(), p.enable_rss as i64, p.enable_automatic_search as i64, p.enable_interactive_search as i64, p.minimum_seeders],
    )
}

/// The app profile an indexer's settings name (`appProfileId`), 1 when they name none.
pub fn profile_of(settings: &std::collections::BTreeMap<String, String>) -> i64 {
    settings.get("appProfileId").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(1)
}

/// The app profile routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/appprofile", get(list).post(create))
        .route("/api/v1/appprofile/schema", get(schema))
        .route("/api/v1/appprofile/{id}", get(one).put(update).delete(remove))
}

/// `GET /api/v1/appprofile`: every profile.
async fn list(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(Value::Array(all(&st.db)?.iter().map(AppProfile::json).collect())))
}

/// `GET /api/v1/appprofile/{id}`.
async fn one(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(get_profile(&st.db, id)?.json()))
}

/// `GET /api/v1/appprofile/schema`: the default profile with no name.
async fn schema(_: V1) -> Json<Value> {
    Json(AppProfile::default_named("").json())
}

/// `POST /api/v1/appprofile`: saves a new profile (it needs a name). 201 with the profile.
async fn create(State(st): State<AppState>, _: V1, bytes: Bytes) -> ApiResult<Response> {
    let body = body_json(&bytes)?;
    let (profile, name) = AppProfile::of_json(&body);
    check(vec![validation::not_empty("Name", name.as_deref())])?;
    let id = insert(&st.db, &profile)?;
    Ok(created(format!("/api/v1/appprofile/{id}"), get_profile(&st.db, id)?.json()))
}

/// `PUT /api/v1/appprofile/{id}`: changes a profile (the body's id, else the address'). 202 with the profile.
async fn update(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, bytes: Bytes) -> ApiResult<Response> {
    validate_id(path_id)?;
    let body = body_json(&bytes)?;
    let (mut profile, name) = AppProfile::of_json(&body);
    if profile.id == 0 {
        profile.id = path_id;
    }
    check(vec![validation::not_empty("Name", name.as_deref()), validation::valid_id(profile.id)])?;
    st.db.execute(
        "UPDATE prowlarr_app_profiles SET name=?, enable_rss=?, enable_automatic_search=?, enable_interactive_search=?, minimum_seeders=? WHERE id=?",
        p![
            profile.name.clone(),
            profile.enable_rss as i64,
            profile.enable_automatic_search as i64,
            profile.enable_interactive_search as i64,
            profile.minimum_seeders,
            profile.id
        ],
    )?;
    Ok(accepted(format!("/api/v1/appprofile/{}", profile.id), get_profile(&st.db, profile.id)?.json()))
}

/// `DELETE /api/v1/appprofile/{id}`: removes a profile, unless an indexer uses it or it is the last one
/// (Prowlarr's `ProfileInUseException`, 500). Answers `{}`.
async fn remove(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    validate_id(id)?;
    let in_use = st.indexers.list()?.iter().any(|ix| profile_of(&ix.settings) == id);
    if in_use || all(&st.db)?.len() == 1 {
        return Err(fail(500, format!("Profile [{id}] is in use.")));
    }
    st.db.execute("DELETE FROM prowlarr_app_profiles WHERE id=?", p![id])?;
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_profile_is_prowlarrs() {
        assert_eq!(
            AppProfile::default_named("").json(),
            json!({"id": 0, "name": "", "enableRss": true, "enableAutomaticSearch": true, "enableInteractiveSearch": true, "minimumSeeders": 1})
        );
        let (p, name) = AppProfile::of_json(&json!({"name": "Fast", "enableRss": true, "minimumSeeders": 5}));
        assert_eq!(name.as_deref(), Some("Fast"));
        assert!(p.enable_rss && !p.enable_automatic_search);
        assert_eq!(p.minimum_seeders, 5);
        let mut s = std::collections::BTreeMap::new();
        assert_eq!(profile_of(&s), 1);
        s.insert("appProfileId".to_string(), "3".to_string());
        assert_eq!(profile_of(&s), 3);
    }
}
