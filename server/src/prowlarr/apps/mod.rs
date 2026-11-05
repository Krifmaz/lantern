// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/HealthCheck/Checks/ApplicationStatusCheck.cs,
// ApplicationLongTermStatusCheck.cs), GPL-3.0.

//! Prowlarr's applications: the *arr apps (Sonarr, Radarr, Lidarr, Readarr, Whisparr) and Mylar and
//! LazyLibrarian that Lantern pushes its indexers to, the app profiles those indexers carry, and the sync that
//! keeps the apps' indexer lists in step.
//!
//! ```text
//! *    /api/v1/applications[/{id}], /schema, /test, /testall, /bulk, /action/{name}
//! *    /api/v1/appprofile[/{id}], /schema
//! ```
//!
//! [`provider`] is the generic provider API (Prowlarr's `ProviderControllerBase`), ready for download clients,
//! notifications and indexer proxies.

pub mod applications;
pub mod arr;
pub mod categories;
pub mod http;
pub mod nab;
pub mod profiles;
pub mod provider;
pub mod status;
pub mod sync;

use super::health::Finding;
use super::localization;
use crate::AppState;
use applications::Applications;
use axum::Router;
use kit_server_core::{Db, SqliteDb};
use provider::Family;

/// The tables of the applications: the apps, their status, which indexer each app holds, the app profiles and
/// which of the migration's imported apps were taken over. Makes the "Standard" app profile when there is none.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    let tables = [
        provider::schema(Applications::TABLE),
        status::APPLICATION_STATUS.schema(),
        sync::MAPS_SCHEMA.to_string(),
        profiles::SCHEMA.to_string(),
        applications::TAKEN_SCHEMA.to_string(),
    ];
    for sql in tables {
        db.execute_batch(&sql).map_err(|e| anyhow::anyhow!("The applications' tables can't be made: {}", e.detail))?;
    }
    profiles::seed(db).map_err(|e| anyhow::anyhow!("The app profiles can't be set up: {}", e.detail))?;
    status::mark_started(crate::auth::now());
    Ok(())
}

/// The application and app profile routes.
pub fn router() -> Router<AppState> {
    Router::new().merge(provider::routes::<Applications>()).merge(profiles::router())
}

/// `ApplicationStatusCheck` and `ApplicationLongTermStatusCheck`: the enabled apps with valid settings that are
/// held back after failures, for less and for more than six hours.
pub fn health(st: &AppState, lang: &str) -> kit_server_core::Result<Vec<Finding>> {
    let enabled: Vec<(i64, String)> = provider::all::<Applications>(&st.db)?
        .iter()
        .filter(|d| Applications::enabled(d) && provider::settings_valid::<Applications>(d))
        .map(|d| (d.id, d.name.clone().unwrap_or_default()))
        .collect();
    let blocked = status::APPLICATION_STATUS.blocked(&st.db)?;
    let now = crate::auth::now();
    let fragment = Some("#applications-are-unavailable-due-to-failures");
    let mut out = Vec::new();
    for (source, long_term) in [("ApplicationStatusCheck", false), ("ApplicationLongTermStatusCheck", true)] {
        let say = |phrase: &str, tokens: &[(&str, String)]| localization::localized(lang, phrase, tokens).unwrap_or_default();
        match status::check(&enabled, &blocked, now, long_term) {
            None => {}
            Some(status::Check::All) => {
                out.push(Finding::new(source, "error", say(&format!("{source}AllClientMessage"), &[]), fragment));
            }
            Some(status::Check::Some(names)) => {
                let msg = say(&format!("{source}SingleClientMessage"), &[("0", names.join(", "))]);
                out.push(Finding::new(source, "warning", msg, fragment));
            }
        }
    }
    Ok(out)
}
