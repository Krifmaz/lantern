// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Health/HealthController.cs, HealthResource.cs,
// src/NzbDrone.Core/HealthCheck/HealthCheck.cs, Checks/ApiKeyValidationCheck.cs, Checks/AllowedHostsCheck.cs,
// Checks/IndexerCheck.cs, Checks/IndexerStatusCheck.cs, Checks/IndexerLongTermStatusCheck.cs,
// Checks/IndexerNoDefinitionCheck.cs, Checks/IndexerVIPCheck.cs, Checks/IndexerVIPExpiredCheck.cs,
// Checks/IndexerDownloadClientCheck.cs, Checks/IndexerProxyStatusCheck.cs, Checks/DownloadClientStatusCheck.cs,
// Checks/NotificationStatusCheck.cs, Checks/AppDataLocationCheck.cs, Checks/ReleaseBranchCheck.cs, Checks/UpdateCheck.cs,
// Checks/ProxyCheck.cs, Checks/SystemTimeCheck.cs), GPL-3.0.

//! What needs attention (`GET /api/v1/health`): each of Prowlarr's health checks that applies to what Lantern
//! has, with Prowlarr's texts in the UI language. Only checks that found something are listed, as in Prowlarr.

use super::{config, localization, ApiResult, V1};
use crate::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

/// The health route.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/health", get(health))
}

/// One finding, as Prowlarr's `HealthCheck`.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// The check's name (`IndexerStatusCheck`).
    pub source: &'static str,
    /// `notice`, `warning` or `error`.
    pub kind: &'static str,
    /// What is wrong, in words.
    pub message: String,
    /// Where Prowlarr's wiki explains it.
    pub wiki_url: String,
}

impl Finding {
    /// A finding of `source`, its wiki link made from `fragment` (`#invalid-api-key`), or from the message when
    /// there is none, as Prowlarr makes it.
    pub fn new(source: &'static str, kind: &'static str, message: String, fragment: Option<&str>) -> Finding {
        let fragment = match fragment {
            Some(f) => f.to_string(),
            None => {
                let cleaned: String = message.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || *c == ' ').collect();
                format!("#{}", cleaned.replace(' ', "-"))
            }
        };
        Finding { source, kind, message, wiki_url: format!("https://wiki.servarr.com/prowlarr/system{fragment}") }
    }

    /// The finding as Prowlarr's `HealthResource`.
    pub fn json(&self) -> Value {
        json!({ "source": self.source, "type": self.kind, "message": self.message, "wikiUrl": self.wiki_url })
    }
}

/// Every check's finding, in the UI language.
pub async fn results(st: &AppState) -> kit_server_core::Result<Vec<Finding>> {
    let lang = config::ui_language(&st.db);
    let say = |phrase: &str, tokens: &[(&str, String)]| localization::localized(&lang, phrase, tokens).unwrap_or_default();
    let mut out = Vec::new();

    // ApiKeyValidationCheck
    const MINIMUM_LENGTH: usize = 20;
    if st.settings.str("api_key").chars().count() < MINIMUM_LENGTH {
        let msg = say("ApiKeyValidationHealthCheckMessage", &[("length", MINIMUM_LENGTH.to_string())]);
        out.push(Finding::new("ApiKeyValidationCheck", "warning", msg, Some("#invalid-api-key")));
    }

    // AllowedHostsCheck
    let hosts: Vec<String> = super::validation::allowed_hosts::parse(Some(&config::allowed_hosts_setting(&st.db)))
        .into_iter()
        .filter(|h| !super::validation::allowed_hosts::is_bind_address(h))
        .collect();
    let configured = !hosts.is_empty() && !hosts.iter().any(|h| h == "*");
    if !configured && config::authentication_required(&st.db) != "enabled" {
        let msg = say("AllowedHostsNotConfiguredHealthCheckMessage", &[]);
        out.push(Finding::new("AllowedHostsCheck", "warning", msg, Some("#allowed-hosts-not-configured")));
    }

    let indexers = st.indexers.list()?;
    let enabled: Vec<_> = indexers.iter().filter(|i| i.enabled).collect();

    // IndexerCheck
    if enabled.is_empty() {
        out.push(Finding::new("IndexerCheck", "error", say("IndexerHealthCheckNoIndexers", &[]), None));
    }

    // IndexerStatusCheck and IndexerLongTermStatusCheck: enabled indexers held back after failures that began in
    // the last 6 hours, and before that
    let now = crate::auth::now();
    let held: Vec<_> = enabled.iter().filter(|i| i.status().is_disabled(now) && i.initial_failure > 0.0).collect();
    let fragment = Some("#indexers-are-unavailable-due-to-failures");
    for (source, recent, all_key, some_key) in [
        ("IndexerStatusCheck", true, "IndexerStatusAllUnavailableHealthCheckMessage", "IndexerStatusUnavailableHealthCheckMessage"),
        (
            "IndexerLongTermStatusCheck",
            false,
            "IndexerLongTermStatusAllUnavailableHealthCheckMessage",
            "IndexerLongTermStatusUnavailableHealthCheckMessage",
        ),
    ] {
        let list: Vec<_> = held.iter().filter(|i| (i.initial_failure > now - 6.0 * 3600.0) == recent).collect();
        if list.is_empty() {
            continue;
        }
        if list.len() == enabled.len() {
            out.push(Finding::new(source, "error", say(all_key, &[]), fragment));
        } else {
            let names = list.iter().map(|i| i.name.clone()).collect::<Vec<_>>().join(", ");
            out.push(Finding::new(source, "warning", say(some_key, &[("indexerNames", names)]), fragment));
        }
    }

    // IndexerNoDefinitionCheck: Cardigann indexers whose definition is gone
    let no_definition: Vec<String> = indexers
        .iter()
        .filter(|i| i.kind == "cardigann" && st.indexers.library.get(&i.definition).is_none())
        .map(|i| i.name.clone())
        .collect();
    if !no_definition.is_empty() {
        let msg = say("IndexerNoDefinitionCheckHealthCheckMessage", &[("indexerNames", no_definition.join(", "))]);
        out.push(Finding::new("IndexerNoDefinitionCheck", "error", msg, Some("#indexers-have-no-definition")));
    }

    // IndexerVIPCheck and IndexerVIPExpiredCheck: Newznab VIP benefits ending within 7 days, or ended
    let vip = |i: &&&crate::indexers::Indexer| {
        i.settings.get("vip_expiration").filter(|v| !v.trim().is_empty()).and_then(|v| super::indexers::query_time(v))
    };
    let local_now = now + chrono::Local::now().offset().local_minus_utc() as f64;
    let expiring: Vec<String> = enabled
        .iter()
        .filter(|i| vip(i).is_some_and(|t| t >= local_now && t <= local_now + 7.0 * 86400.0))
        .map(|i| i.name.clone())
        .collect();
    if !expiring.is_empty() {
        let msg = say("IndexerVipExpiringHealthCheckMessage", &[("indexerNames", expiring.join(", "))]);
        out.push(Finding::new("IndexerVIPCheck", "warning", msg, Some("#indexer-vip-expiring")));
    }
    let expired: Vec<String> = enabled.iter().filter(|i| vip(i).is_some_and(|t| t < local_now)).map(|i| i.name.clone()).collect();
    if !expired.is_empty() {
        let msg = say("IndexerVipExpiredHealthCheckMessage", &[("indexerNames", expired.join(", "))]);
        out.push(Finding::new("IndexerVIPExpiredCheck", "error", msg, Some("#indexer-vip-expired")));
    }

    // IndexerDownloadClientCheck: enabled indexers sent to a download client that is gone or off
    let clients: Vec<i64> = super::apps::provider::all::<super::apps::downloads::DownloadClients>(&st.db)?
        .iter()
        .filter(|c| super::apps::downloads::enable(c))
        .map(|c| c.id)
        .collect();
    let bad_client: Vec<String> =
        enabled.iter().filter(|i| i.download_client_id > 0 && !clients.contains(&i.download_client_id)).map(|i| i.name.clone()).collect();
    if !bad_client.is_empty() {
        let msg = say("IndexerDownloadClientHealthCheckMessage", &[("indexerNames", bad_client.join(", "))]);
        out.push(Finding::new("IndexerDownloadClientCheck", "warning", msg, Some("#invalid-indexer-download-client-setting")));
    }

    // IndexerProxyStatusCheck: the proxies in use that fail their test
    let proxies = st.indexers.available_proxies();
    if !proxies.is_empty() {
        let mut bad = Vec::new();
        for p in &proxies {
            if !crate::indexers::proxies::test(p, &st.indexers.proxy_ping_url(), st.indexers.solver.as_ref(), &st.indexers.site_proxy())
                .await
                .is_empty()
            {
                bad.push(p.name.clone());
            }
        }
        let fragment = Some("#indexer-proxies-are-unavailable-due-to-failures");
        if !bad.is_empty() && bad.len() == proxies.len() {
            out.push(Finding::new(
                "IndexerProxyStatusCheck",
                "error",
                say("IndexerProxyStatusAllUnavailableHealthCheckMessage", &[]),
                fragment,
            ));
        } else if !bad.is_empty() {
            let msg = say("IndexerProxyStatusUnavailableHealthCheckMessage", &[("indexerProxyNames", bad.join(", "))]);
            out.push(Finding::new("IndexerProxyStatusCheck", "warning", msg, fragment));
        }
    }

    // AppDataLocationCheck
    let startup = startup_folder();
    if app_data_inside_startup(&startup, &st.data_dir) {
        let msg = say("AppDataLocationHealthCheckMessage", &[]);
        out.push(Finding::new(
            "AppDataLocationCheck",
            "warning",
            msg,
            Some("#updating-will-not-be-possible-to-prevent-deleting-appdata-on-update"),
        ));
    }

    // ReleaseBranchCheck
    out.extend(release_branch_check(&config::branch_as_saved(&st.db), &lang));

    // UpdateCheck
    let update = UpdateInputs {
        update_automatically: config::update_automatically(&st.db),
        update_mechanism: config::update_mechanism(&st.db),
        is_docker: super::system::is_docker(),
        startup_writable: writable(&startup),
        ui_writable: writable(&startup.join("UI")),
        startup_folder: startup.display().to_string(),
        user_name: std::env::var("USER").unwrap_or_default(),
        build_age_days: super::system::build_age_days(),
        available_update: available_update(st),
    };
    out.extend(update_check(&update, &lang));

    // ProxyCheck
    out.extend(proxy_check(st, &lang).await);

    // SystemTimeCheck (not in debug builds, as in Prowlarr)
    if !cfg!(debug_assertions) {
        out.extend(system_time_check(&lang).await);
    }

    // ApplicationStatusCheck, ApplicationLongTermStatusCheck
    out.extend(super::apps::health(st, &lang)?);
    // DownloadClientStatusCheck, NotificationStatusCheck
    out.extend(super::apps::downloads::health(st, &lang)?);
    out.extend(super::apps::notifications::health(st, &lang)?);
    // what changed since the last run goes to the notifications on for health issues
    super::apps::notifications::health_checked(st, &out);
    Ok(out)
}

/// The folder Lantern runs from (Prowlarr's `StartUpFolder`).
fn startup_folder() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

/// `AppDataLocationCheck`: whether the data folder is the startup folder or inside it (an update would delete it).
pub fn app_data_inside_startup(startup: &std::path::Path, data: &std::path::Path) -> bool {
    let clean = |p: &std::path::Path| p.components().collect::<std::path::PathBuf>();
    let (s, d) = (clean(startup), clean(data));
    !s.as_os_str().is_empty() && d.starts_with(&s)
}

/// Whether Lantern can write in `folder` (`DiskProvider.FolderWritable`: a file can be made and deleted there).
fn writable(folder: &std::path::Path) -> bool {
    let probe = folder.join(format!(".lantern-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// `ReleaseBranchCheck`: a branch that is not `master`, `develop` or `nightly` (in any case) gets no updates.
pub fn release_branch_check(branch: &str, lang: &str) -> Option<Finding> {
    if ["master", "develop", "nightly"].contains(&branch.to_lowercase().as_str()) {
        return None;
    }
    let msg = localization::localized(lang, "ReleaseBranchCheckOfficialBranchMessage", &[("0", branch.to_string())]).unwrap_or_default();
    Some(Finding::new("ReleaseBranchCheck", "warning", msg, Some("#branch-is-not-a-valid-release-branch")))
}

/// What `UpdateCheck` looks at.
#[derive(Debug, Clone, Default)]
pub struct UpdateInputs {
    /// `UpdateAutomatically`.
    pub update_automatically: bool,
    /// `UpdateMechanism` (`builtIn`, `script`, …).
    pub update_mechanism: String,
    /// Whether Lantern runs in Docker.
    pub is_docker: bool,
    /// Whether the startup folder can be written.
    pub startup_writable: bool,
    /// Whether its `UI` folder can be written.
    pub ui_writable: bool,
    /// The startup folder.
    pub startup_folder: String,
    /// The user Lantern runs as.
    pub user_name: String,
    /// How old this build is, in days.
    pub build_age_days: f64,
    /// A newer release, when there is one.
    pub available_update: Option<String>,
}

/// `UpdateCheck`: with built-in updates on outside Docker, folders that can't be written; then, for a build older
/// than 14 days, a newer release (an error past 180 days).
pub fn update_check(i: &UpdateInputs, lang: &str) -> Option<Finding> {
    let say = |phrase: &str, tokens: &[(&str, String)]| localization::localized(lang, phrase, tokens).unwrap_or_default();
    if i.update_automatically && i.update_mechanism.eq_ignore_ascii_case("builtIn") && !i.is_docker {
        if !i.startup_writable {
            let msg = say(
                "UpdateStartupNotWritableHealthCheckMessage",
                &[("startupFolder", i.startup_folder.clone()), ("userName", i.user_name.clone())],
            );
            return Some(Finding::new(
                "UpdateCheck",
                "error",
                msg,
                Some("#cannot-install-update-because-startup-folder-is-not-writable-by-the-user"),
            ));
        }
        if !i.ui_writable {
            let ui = std::path::Path::new(&i.startup_folder).join("UI").display().to_string();
            let msg = say("UpdateUiNotWritableHealthCheckMessage", &[("uiFolder", ui), ("userName", i.user_name.clone())]);
            return Some(Finding::new(
                "UpdateCheck",
                "error",
                msg,
                Some("#cannot-install-update-because-ui-folder-is-not-writable-by-the-user"),
            ));
        }
    }
    if i.build_age_days > 14.0 {
        if let Some(v) = &i.available_update {
            let kind = if i.build_age_days > 180.0 { "error" } else { "warning" };
            let msg = say("UpdateAvailableHealthCheckMessage", &[("version", format!("v{v}"))]);
            return Some(Finding::new("UpdateCheck", kind, msg, Some("#new-update-is-available")));
        }
    }
    None
}

/// A release newer than this one among the release notes built in (`CheckUpdateService.AvailableUpdate`).
fn available_update(st: &AppState) -> Option<String> {
    let current = kit_server_core::version::base(crate::VERSION);
    st.changelog
        .releases
        .first()
        .filter(|r| kit_server_core::version::compare(&r.version, &current) == std::cmp::Ordering::Greater)
        .map(|r| r.version.clone())
}

/// `ProxyCheck`: when a proxy is set, its host must resolve, and a request through it must not answer 400.
pub async fn proxy_check(st: &AppState, lang: &str) -> Option<Finding> {
    let (enabled, _kind, host, port) = config::proxy_in_effect(st);
    if !enabled {
        return None;
    }
    let say = |phrase: &str, tokens: &[(&str, String)]| localization::localized(lang, phrase, tokens).unwrap_or_default();
    let resolved = tokio::net::lookup_host((host.as_str(), port as u16)).await.map(|mut a| a.next().is_some()).unwrap_or(false);
    if !resolved {
        let msg = say("ProxyResolveIpHealthCheckMessage", &[("proxyHostName", host.clone())]);
        return Some(Finding::new("ProxyCheck", "error", msg, Some("#proxy-failed-resolve-ip")));
    }
    let url = st.indexers.proxy_ping_url();
    let failed = |url: &str| {
        let msg = say("ProxyFailedToTestHealthCheckMessage", &[("url", url.to_string())]);
        Some(Finding::new("ProxyCheck", "error", msg, Some("#proxy-failed-test")))
    };
    let proxy = match reqwest::Proxy::all(st.settings.str("site_proxy").trim()) {
        Ok(p) => p,
        Err(_) => return failed(&url),
    };
    let client = match reqwest::Client::builder().proxy(proxy).timeout(std::time::Duration::from_secs(20)).build() {
        Ok(c) => c,
        Err(_) => return failed(&url),
    };
    match client.get(&url).send().await {
        // only a 400 counts; other answers can be ignored
        Ok(r) if r.status() == reqwest::StatusCode::BAD_REQUEST => {
            tracing::error!("Proxy Health Check failed: {}", r.status());
            let msg = say("ProxyBadRequestHealthCheckMessage", &[("statusCode", "BadRequest".to_string())]);
            Some(Finding::new("ProxyCheck", "error", msg, Some("#proxy-failed-test")))
        }
        Ok(_) => None,
        Err(e) => {
            tracing::error!("Proxy Health Check failed: {e}");
            failed(&url)
        }
    }
}

/// Prowlarr's time service (`ProwlarrCloudRequestBuilder` `/time`).
const TIME_URL: &str = "https://prowlarr.servarr.com/v1/time";

/// `SystemTimeCheck` on the time `server_utc` the time service gave: an error when this system's clock is a day or
/// more off.
pub fn system_time_finding(server_utc: f64, now: f64, lang: &str) -> Option<Finding> {
    if (server_utc - now).abs() / 86_400.0 < 1.0 {
        return None;
    }
    tracing::error!(
        "System time mismatch. SystemTime: {} Expected Time: {}. Update system time",
        super::date(now),
        super::date(server_utc)
    );
    let msg = localization::localized(lang, "SystemTimeHealthCheckMessage", &[]).unwrap_or_default();
    Some(Finding::new("SystemTimeCheck", "error", msg, Some("#system-time-off")))
}

/// `SystemTimeCheck`: asks the time service (at most every six hours; the answer in between is kept); nothing when it
/// can't be reached.
async fn system_time_check(lang: &str) -> Option<Finding> {
    static LAST: std::sync::Mutex<Option<(f64, Option<f64>)>> = std::sync::Mutex::new(None);
    let now = crate::auth::now();
    let cached = *LAST.lock().unwrap_or_else(|e| e.into_inner());
    let server = match cached {
        Some((at, server)) if now - at < 6.0 * 3600.0 => server.map(|s| s + (now - at)),
        _ => {
            let fetched = async {
                let r =
                    reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().ok()?.get(TIME_URL).send().await.ok()?;
                let v: Value = r.json().await.ok()?;
                let t = chrono::DateTime::parse_from_rfc3339(v["dateTimeUtc"].as_str()?).ok()?;
                Some(t.timestamp_millis() as f64 / 1000.0)
            }
            .await;
            if fetched.is_none() {
                tracing::warn!("Unable to verify system time");
            }
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, fetched));
            fetched
        }
    };
    system_time_finding(server?, now, lang)
}

/// `GET /api/v1/health`.
async fn health(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(Value::Array(results(&st).await?.iter().map(Finding::json).collect())))
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/HealthCheck/Checks/AppDataLocationFixture.cs,
    // ReleaseBranchCheckFixture.cs, SystemTimeCheckFixture.cs, UpdateCheckFixture.cs), GPL-3.0.
    use super::*;
    use std::path::Path;

    #[test]
    fn should_return_warning_when_app_data_is_child_of_startup_folder() {
        assert!(app_data_inside_startup(Path::new("/opt/NzbDrone"), Path::new("/opt/NzbDrone/AppData")));
    }

    #[test]
    fn should_return_warning_when_app_data_is_same_as_startup_folder() {
        assert!(app_data_inside_startup(Path::new("/opt/NzbDrone"), Path::new("/opt/NzbDrone/")));
    }

    #[test]
    fn should_return_ok_when_no_conflict() {
        assert!(!app_data_inside_startup(Path::new("/opt/NzbDrone"), Path::new("/ProgramData/NzbDrone")));
        // a folder whose name only starts the same is not inside
        assert!(!app_data_inside_startup(Path::new("/opt/NzbDrone"), Path::new("/opt/NzbDroneData")));
    }

    #[test]
    fn release_branch_check_follows_prowlarrs_fixture() {
        let f = release_branch_check("test", "en").expect("should_return_warning_when_branch_not_valid");
        assert_eq!(f.kind, "warning");
        assert_eq!(f.message, "Branch test is not a valid Lantern release branch, you will not receive updates");
        assert_eq!(f.wiki_url, "https://wiki.servarr.com/prowlarr/system#branch-is-not-a-valid-release-branch");
        for branch in ["nightly", "Nightly", "develop", "master"] {
            assert_eq!(release_branch_check(branch, "en"), None, "should_return_no_warning_when_branch_valid({branch})");
        }
    }

    #[test]
    fn system_time_check_follows_prowlarrs_fixture() {
        let now = crate::auth::now();
        // should_not_return_error_when_system_time_is_close_to_server_time
        assert_eq!(system_time_finding(now, now, "en"), None);
        // should_return_error_when_system_time_is_more_than_one_day_from_server_time
        let f = system_time_finding(now + 2.0 * 86_400.0, now, "en").unwrap();
        assert_eq!(f.kind, "error");
        assert_eq!(f.message, "System time is off by more than 1 day. Scheduled tasks may not run correctly until the time is corrected");
    }

    #[test]
    fn update_check_follows_prowlarrs_fixture() {
        let base = UpdateInputs {
            update_automatically: true,
            update_mechanism: "builtIn".into(),
            startup_writable: true,
            ui_writable: true,
            startup_folder: "/opt/NzbDrone".into(),
            user_name: "lantern".into(),
            ..Default::default()
        };
        // should_return_error_when_app_folder_is_write_protected_and_update_automatically_is_enabled
        let f = update_check(&UpdateInputs { startup_writable: false, ..base.clone() }, "en").unwrap();
        assert_eq!(f.kind, "error");
        assert_eq!(f.message, "Cannot install update because startup folder '/opt/NzbDrone' is not writable by the user 'lantern'.");
        // should_return_error_when_ui_folder_is_write_protected_and_update_automatically_is_enabled
        let f = update_check(&UpdateInputs { ui_writable: false, ..base.clone() }, "en").unwrap();
        assert_eq!(f.message, "Cannot install update because UI folder '/opt/NzbDrone/UI' is not writable by the user 'lantern'.");
        // should_not_return_error_when_app_folder_is_write_protected_and_external_script_enabled
        let script = UpdateInputs { startup_writable: false, ui_writable: false, update_mechanism: "script".into(), ..base.clone() };
        assert_eq!(update_check(&script, "en"), None);
        // in Docker the folders are not checked
        assert_eq!(update_check(&UpdateInputs { startup_writable: false, is_docker: true, ..base.clone() }, "en"), None);
        // a newer release: a warning past 14 days, an error past 180
        let newer = UpdateInputs { available_update: Some("2.0.0".into()), build_age_days: 20.0, ..base.clone() };
        let f = update_check(&newer, "en").unwrap();
        assert_eq!((f.kind, f.message.as_str()), ("warning", "New update is available: v2.0.0"));
        assert_eq!(update_check(&UpdateInputs { build_age_days: 200.0, ..newer.clone() }, "en").unwrap().kind, "error");
        assert_eq!(update_check(&UpdateInputs { build_age_days: 3.0, ..newer }, "en"), None);
    }

    #[test]
    fn makes_wiki_links_like_prowlarr() {
        let f = Finding::new("IndexerCheck", "error", "No indexers enabled, Lantern will not return search results".into(), None);
        assert_eq!(f.wiki_url, "https://wiki.servarr.com/prowlarr/system#no-indexers-enabled-lantern-will-not-return-search-results");
        let f = Finding::new("ApiKeyValidationCheck", "warning", "x".into(), Some("#invalid-api-key"));
        assert_eq!(f.wiki_url, "https://wiki.servarr.com/prowlarr/system#invalid-api-key");
    }
}
