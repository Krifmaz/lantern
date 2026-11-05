// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Health/HealthController.cs, HealthResource.cs,
// src/NzbDrone.Core/HealthCheck/HealthCheck.cs, Checks/ApiKeyValidationCheck.cs, Checks/AllowedHostsCheck.cs,
// Checks/IndexerCheck.cs, Checks/IndexerStatusCheck.cs, Checks/IndexerLongTermStatusCheck.cs,
// Checks/IndexerNoDefinitionCheck.cs, Checks/IndexerVIPCheck.cs, Checks/IndexerVIPExpiredCheck.cs,
// Checks/IndexerDownloadClientCheck.cs, Checks/IndexerProxyStatusCheck.cs), GPL-3.0.

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
    let clients: Vec<i64> = st.hub.clients()?.iter().filter(|c| c.enabled).map(|c| c.id).collect();
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

    // ApplicationStatusCheck, ApplicationLongTermStatusCheck
    out.extend(super::apps::health(st, &lang)?);
    Ok(out)
}

/// `GET /api/v1/health`.
async fn health(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(Value::Array(results(&st).await?.iter().map(Finding::json).collect())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_wiki_links_like_prowlarr() {
        let f = Finding::new("IndexerCheck", "error", "No indexers enabled, Lantern will not return search results".into(), None);
        assert_eq!(f.wiki_url, "https://wiki.servarr.com/prowlarr/system#no-indexers-enabled-lantern-will-not-return-search-results");
        let f = Finding::new("ApiKeyValidationCheck", "warning", "x".into(), Some("#invalid-api-key"));
        assert_eq!(f.wiki_url, "https://wiki.servarr.com/prowlarr/system#invalid-api-key");
    }
}
