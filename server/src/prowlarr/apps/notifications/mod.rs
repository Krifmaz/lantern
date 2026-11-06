// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Notifications/NotificationController.cs,
// NotificationResource.cs, src/NzbDrone.Core/Notifications/NotificationDefinition.cs, NotificationFactory.cs,
// NotificationService.cs, NotificationStatusService.cs, GrabMessage.cs, ApplicationUpdateMessage.cs,
// src/NzbDrone.Core/HealthCheck/HealthCheckService.cs, Checks/NotificationStatusCheck.cs), GPL-3.0.

//! Prowlarr's notifications (`/api/v1/notification`, "Connect" in the web app): who hears about grabs, health
//! issues, health issues that went away and application updates, through Apprise, Discord, Email, Gotify, Join,
//! Mailgun, Notifiarr, ntfy, Prowl, Pushbullet, Pushcut, Pushover, SendGrid, Signal, Simplepush, Slack,
//! Telegram, Twitter, a webhook or a script of one's own. Like Prowlarr, notifications have no bulk routes.

pub mod email;
pub mod script;
pub mod senders;
pub mod settings;

use super::super::health::Finding;
use super::super::{validation, ApiError};
use super::downloads;
use super::provider::{self, prop, Definition, Family, Implementation, ProviderFailure};
use super::status::{self, NOTIFICATION_STATUS};
use crate::indexers::release::Release;
use crate::AppState;
use kit_server_core::{Db, SqliteDb};
use serde_json::{json, Map, Value};
use settings as s;
use std::collections::HashMap;
use std::sync::Mutex;

/// Prowlarr's notifications.
pub struct Notifications;

/// An implementation with its settings.
const fn imp(
    implementation: &'static str,
    implementation_name: &'static str,
    config_contract: &'static str,
    fields: &'static [provider::FieldSpec],
    validate: fn(&Map<String, Value>) -> Vec<ProviderFailure>,
) -> Implementation {
    Implementation { implementation, implementation_name, config_contract, fields, validate }
}

/// Every notification Prowlarr has.
static IMPLEMENTATIONS: [Implementation; 20] = [
    imp("Apprise", "Apprise", "AppriseSettings", s::APPRISE_SETTINGS, s::apprise),
    imp("CustomScript", "Custom Script", "CustomScriptSettings", s::CUSTOM_SCRIPT_SETTINGS, s::custom_script),
    imp("Discord", "Discord", "DiscordSettings", s::DISCORD_SETTINGS, s::webhook_url),
    imp("Email", "Email", "EmailSettings", s::EMAIL_SETTINGS, s::email),
    imp("Gotify", "Gotify", "GotifySettings", s::GOTIFY_SETTINGS, s::gotify),
    imp("Join", "Join", "JoinSettings", s::JOIN_SETTINGS, s::join),
    imp("Mailgun", "Mailgun", "MailgunSettings", s::MAILGUN_SETTINGS, s::mailgun),
    imp("Notifiarr", "Notifiarr", "NotifiarrSettings", s::NOTIFIARR_SETTINGS, s::notifiarr),
    imp("Ntfy", "ntfy.sh", "NtfySettings", s::NTFY_SETTINGS, s::ntfy),
    imp("Prowl", "Prowl", "ProwlSettings", s::PROWL_SETTINGS, s::api_key),
    imp("PushBullet", "Pushbullet", "PushBulletSettings", s::PUSH_BULLET_SETTINGS, s::api_key),
    imp("Pushcut", "Pushcut", "PushcutSettings", s::PUSHCUT_SETTINGS, s::pushcut),
    imp("Pushover", "Pushover", "PushoverSettings", s::PUSHOVER_SETTINGS, s::pushover),
    imp("SendGrid", "SendGrid", "SendGridSettings", s::SEND_GRID_SETTINGS, s::sendgrid),
    imp("Signal", "Signal", "SignalSettings", s::SIGNAL_SETTINGS, s::signal),
    imp("Simplepush", "Simplepush", "SimplepushSettings", s::SIMPLEPUSH_SETTINGS, s::simplepush),
    imp("Slack", "Slack", "SlackSettings", s::SLACK_SETTINGS, s::webhook_url),
    imp("Telegram", "Telegram", "TelegramSettings", s::TELEGRAM_SETTINGS, s::telegram),
    imp("Twitter", "Twitter", "TwitterSettings", s::TWITTER_SETTINGS, s::twitter),
    imp("Webhook", "Webhook", "WebhookSettings", s::WEBHOOK_SETTINGS, s::webhook),
];

/// The events a notification can be on for, as they are named in the resource.
const EVENTS: [&str; 4] = ["onGrab", "onHealthIssue", "onHealthRestored", "onApplicationUpdate"];

/// The switches a notification keeps besides its events.
const SWITCHES: [&str; 2] = ["includeManualGrabs", "includeHealthWarnings"];

/// Whether an implementation can send `event` (`SupportsOn…`): Slack and Mailgun have no grab notification.
pub fn supports(implementation: &str, event: &str) -> bool {
    !(event == "onGrab" && matches!(implementation, "Slack" | "Mailgun"))
}

/// One of a notification's switches.
fn switch(def: &Definition, name: &str) -> bool {
    def.extra.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A grab, as notifications hear of it (`GrabMessage`).
#[derive(Debug, Clone)]
pub struct GrabMessage {
    /// The release.
    pub release: Release,
    /// The app that grabbed it.
    pub source: String,
    /// The machine that asked.
    pub host: String,
    /// Whether it reached the download client.
    pub successful: bool,
    /// The download client's name ("" when none).
    pub download_client_name: String,
    /// The download client's kind (`qBittorrent`).
    pub download_client_type: String,
    /// The client's id for the download.
    pub download_id: String,
    /// Whether the app was sent to the indexer itself.
    pub redirect: bool,
    /// Grabbed in Lantern's own web app (`GrabTrigger.Manual`), not by an app.
    pub manual: bool,
    /// What happened, in words (`GetMessage`).
    pub message: String,
}

impl GrabMessage {
    /// The message of a grab (`NotificationService.GetMessage`).
    pub fn message_for(release: &Release, manual: bool, source: &str, download_client: &str) -> String {
        let mut m = if manual {
            format!("{} manually grabbed in Lantern from {}", release.title, release.indexer)
        } else {
            format!("{} grabbed by {source} from {}", release.title, release.indexer)
        };
        if !download_client.trim().is_empty() {
            m.push_str(&format!(" and sent to {download_client}"));
        }
        m
    }
}

/// An update of Lantern (`ApplicationUpdateMessage`).
#[derive(Debug, Clone)]
pub struct UpdateMessage {
    /// What happened, in words.
    pub message: String,
    /// The version before.
    pub previous_version: String,
    /// The version now.
    pub new_version: String,
}

/// What a notification is told.
pub enum Event<'a> {
    /// A release was grabbed.
    Grab(&'a GrabMessage),
    /// A health check found something.
    HealthIssue(&'a Finding),
    /// What a health check found went away.
    HealthRestored(&'a Finding),
    /// Lantern was updated.
    ApplicationUpdate(&'a UpdateMessage),
    /// The test.
    Test,
}

impl Family for Notifications {
    const RESOURCE: &'static str = "notification";
    const TABLE: &'static str = "prowlarr_notifications";
    const MODEL: &'static str = "NotificationDefinition";
    const BULK: bool = false;

    fn implementations() -> &'static [Implementation] {
        &IMPLEMENTATIONS
    }

    fn enabled(def: &Definition) -> bool {
        EVENTS.iter().any(|e| switch(def, e))
    }

    fn extra_from_body(body: &Map<String, Value>) -> Result<Map<String, Value>, ApiError> {
        Ok(EVENTS
            .iter()
            .chain(SWITCHES.iter())
            .map(|k| (k.to_string(), json!(prop(body, k).and_then(Value::as_bool).unwrap_or(false))))
            .collect())
    }

    fn extra_json(def: &Definition, out: &mut Map<String, Value>) {
        for k in EVENTS.iter().chain(SWITCHES.iter()) {
            out.insert(k.to_string(), json!(switch(def, k)));
        }
        for e in EVENTS {
            let name = format!("supportsOn{}", &e[2..]);
            out.insert(name, json!(supports(&def.implementation, e)));
        }
    }

    fn default_extra() -> Map<String, Value> {
        EVENTS.iter().chain(SWITCHES.iter()).map(|k| (k.to_string(), json!(false))).collect()
    }

    fn bulk_extra(_def: &mut Definition, _body: &Map<String, Value>) -> Result<(), ApiError> {
        Ok(())
    }

    fn extra_checks(_body: &Map<String, Value>) -> Vec<Option<validation::Failure>> {
        Vec::new()
    }

    fn message(def: &Definition) -> Option<Value> {
        (def.implementation == "CustomScript").then(|| {
            json!({ "message": "Testing will execute the script with the EventType set to Test, ensure your script handles this correctly", "type": "warning" })
        })
    }

    async fn test(st: &AppState, def: &Definition) -> Vec<ProviderFailure> {
        let failures = senders::test(st, def).await;
        if def.id != 0 && failures.iter().all(|f| f.is_warning) {
            NOTIFICATION_STATUS.record_success(&st.db, def.id);
        }
        failures
    }

    fn added(_st: &AppState, _def: &Definition) {}

    fn updated(_st: &AppState, _def: &Definition) {}

    fn deleted(st: &AppState, id: i64) {
        let _ = NOTIFICATION_STATUS.delete(&st.db, id);
    }

    fn bulk_updated(_st: &AppState, _defs: &[Definition]) {}

    fn bulk_deleted(st: &AppState, ids: &[i64]) {
        for id in ids {
            Self::deleted(st, *id);
        }
    }

    async fn request_action(_st: &AppState, _def: &Definition, _name: &str, _query: &HashMap<String, String>) -> Result<Value, ApiError> {
        Ok(Value::Null)
    }
}

/// The tables of the notifications and their status.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    for sql in [provider::schema(Notifications::TABLE), NOTIFICATION_STATUS.schema()] {
        db.execute_batch(&sql).map_err(|e| anyhow::anyhow!("The notifications' tables can't be made: {}", e.detail))?;
    }
    Ok(())
}

/// The notifications on for `event` whose implementation can send it.
fn on_for(st: &AppState, event: &str) -> Vec<Definition> {
    provider::all::<Notifications>(&st.db)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| switch(d, event) && supports(&d.implementation, event) && provider::settings_valid::<Notifications>(d))
        .collect()
}

/// Sends `ev` to `def`, noting the success or failure (`NotificationService`'s try/catch).
async fn deliver(st: &AppState, def: &Definition, ev: &Event<'_>) {
    match senders::send(st, def, ev).await {
        Ok(()) => NOTIFICATION_STATUS.record_success(&st.db, def.id),
        Err(e) => {
            NOTIFICATION_STATUS.record_failure(&st.db, def.id, 0.0, true);
            tracing::warn!("Lantern couldn't send a notification to {}: {}", def.label(), e.message);
        }
    }
}

/// `Handle(IndexerDownloadEvent)`: tells the notifications on for grabs, the ones whose tags (if any) the
/// indexer shares, and for a grab in Lantern's own web app only those that include manual grabs.
pub async fn grabbed(st: &AppState, msg: &GrabMessage, indexer_tags: &[i64]) {
    let blocked: Vec<i64> = NOTIFICATION_STATUS.blocked(&st.db).unwrap_or_default().iter().map(|s| s.provider_id).collect();
    for def in on_for(st, "onGrab") {
        if blocked.contains(&def.id) {
            continue;
        }
        let tags_ok = def.tags.is_empty() || def.tags.iter().any(|t| indexer_tags.contains(t));
        if tags_ok && (!msg.manual || switch(&def, "includeManualGrabs")) {
            deliver(st, &def, &Event::Grab(msg)).await;
        }
    }
}

/// Tells the notifications on for application updates that Lantern was updated.
pub async fn updated(st: &AppState, msg: &UpdateMessage) {
    for def in on_for(st, "onApplicationUpdate") {
        deliver(st, &def, &Event::ApplicationUpdate(msg)).await;
    }
}

/// What each data folder's health checks found last time, by check (`HealthCheckService._healthCheckResults`).
static LAST_FINDINGS: Mutex<Option<HashMap<String, HashMap<String, Finding>>>> = Mutex::new(None);

/// The health changes between two runs of the checks: (new findings, findings that went away).
pub fn changes(before: &HashMap<String, Finding>, now: &[Finding]) -> (Vec<Finding>, Vec<Finding>) {
    let failed: Vec<Finding> = now.iter().filter(|f| !before.contains_key(f.source)).cloned().collect();
    let restored: Vec<Finding> = before.values().filter(|b| !now.iter().any(|f| f.source == b.source)).cloned().collect();
    (failed, restored)
}

/// After the health checks ran (`HealthCheckService.PerformHealthCheck`): a check that newly found something is a
/// health issue, one whose finding went away is restored. In the first 15 minutes after a start nothing is sent
/// (the checks run again later). Errors always count; warnings only for notifications that include them.
pub fn health_checked(st: &AppState, findings: &[Finding]) {
    let key = st.data_dir.display().to_string();
    let (failed, restored) = {
        let mut all = LAST_FINDINGS.lock().unwrap_or_else(|e| e.into_inner());
        let map = all.get_or_insert_with(HashMap::new).entry(key).or_default();
        let r = changes(map, findings);
        *map = findings.iter().map(|f| (f.source.to_string(), f.clone())).collect();
        r
    };
    if crate::auth::now() < status::started() + 15.0 * 60.0 || (failed.is_empty() && restored.is_empty()) {
        return;
    }
    let st = st.clone();
    tokio::spawn(async move {
        for (event, list) in [("onHealthIssue", &failed), ("onHealthRestored", &restored)] {
            for def in on_for(&st, event) {
                for f in list.iter().filter(|f| f.kind == "error" || switch(&def, "includeHealthWarnings")) {
                    let ev = if event == "onHealthIssue" { Event::HealthIssue(f) } else { Event::HealthRestored(f) };
                    deliver(&st, &def, &ev).await;
                }
            }
        }
    });
}

/// `NotificationStatusCheck`: the enabled notifications held back after failures.
pub fn health(st: &AppState, lang: &str) -> kit_server_core::Result<Vec<Finding>> {
    use super::super::localization::localized;
    let enabled: Vec<(i64, String)> =
        provider::all::<Notifications>(&st.db)?.iter().filter(|d| Notifications::enabled(d)).map(|d| (d.id, d.label())).collect();
    let blocked: Vec<i64> = NOTIFICATION_STATUS.blocked(&st.db)?.iter().map(|s| s.provider_id).collect();
    let fragment = Some("#notifications-are-unavailable-due-to-failures");
    let source = "NotificationStatusCheck";
    Ok(match downloads::held_back(&enabled, &blocked) {
        None => Vec::new(),
        Some(status::Check::All) => {
            vec![Finding::new(
                source,
                "error",
                localized(lang, "NotificationStatusAllClientHealthCheckMessage", &[]).unwrap_or_default(),
                fragment,
            )]
        }
        Some(status::Check::Some(names)) => {
            let msg = localized(lang, "NotificationStatusSingleClientHealthCheckMessage", &[("notificationNames", names.join(", "))]);
            vec![Finding::new(source, "warning", msg.unwrap_or_default(), fragment)]
        }
    })
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/NotificationTests/NotificationBaseFixture.cs,
    // src/NzbDrone.Core.Test/HealthCheck/Checks/NotificationStatusCheckFixture.cs), GPL-3.0.
    use super::*;

    #[test]
    fn should_support_events_the_provider_implements() {
        // NotificationBaseFixture: a provider that implements every event supports every one; Slack and Mailgun
        // have no OnGrab
        for i in IMPLEMENTATIONS.iter() {
            for e in EVENTS {
                let expected = !(e == "onGrab" && (i.implementation == "Slack" || i.implementation == "Mailgun"));
                assert_eq!(supports(i.implementation, e), expected, "{} {e}", i.implementation);
            }
        }
    }

    #[test]
    fn notification_status_check() {
        let enabled = vec![(1, "Mail".to_string()), (2, "Chat".to_string())];
        // should_not_return_error_when_no_notifications / should_return_ok_when_not_in_backoff
        assert_eq!(downloads::held_back(&[], &[]), None);
        assert_eq!(downloads::held_back(&enabled, &[]), None);
        // should_return_error_if_notification_unavailable (all of them)
        assert_eq!(downloads::held_back(&enabled[..1], &[1]), Some(status::Check::All));
        // should_return_warning_if_one_notification_unavailable
        assert_eq!(downloads::held_back(&enabled, &[2]), Some(status::Check::Some(vec!["Chat".into()])));
    }

    #[test]
    fn grab_messages_read_like_prowlarrs() {
        let r = Release { title: "Show.S01E01".into(), indexer: "Tracker".into(), ..Default::default() };
        assert_eq!(GrabMessage::message_for(&r, false, "Sonarr", ""), "Show.S01E01 grabbed by Sonarr from Tracker");
        assert_eq!(
            GrabMessage::message_for(&r, true, "Prowlarr", "qBit"),
            "Show.S01E01 manually grabbed in Lantern from Tracker and sent to qBit"
        );
    }

    #[test]
    fn health_changes_are_new_and_gone_checks() {
        let f = |s: &'static str| Finding::new(s, "warning", format!("{s} found"), None);
        let before: HashMap<String, Finding> = [("A".to_string(), f("A")), ("B".to_string(), f("B"))].into_iter().collect();
        let (failed, restored) = changes(&before, &[f("B"), f("C")]);
        assert_eq!(failed.iter().map(|x| x.source).collect::<Vec<_>>(), vec!["C"]);
        assert_eq!(restored.iter().map(|x| x.source).collect::<Vec<_>>(), vec!["A"]);
    }
}
