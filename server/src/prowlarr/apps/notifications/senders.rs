// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Notifications/NotificationBase.cs, and each provider and its
// proxy: Apprise/Apprise.cs, AppriseProxy.cs, CustomScript/CustomScript.cs, Discord/Discord.cs, DiscordProxy.cs,
// Email/Email.cs, Gotify/Gotify.cs, GotifyProxy.cs, Join/Join.cs, JoinProxy.cs, Mailgun/Mailgun.cs, MailgunProxy.cs,
// Notifiarr/Notifiarr.cs, NotifiarrProxy.cs, Ntfy/Ntfy.cs, NtfyProxy.cs, Prowl/Prowl.cs, ProwlProxy.cs,
// PushBullet/PushBullet.cs, PushBulletProxy.cs, Pushcut/Pushcut.cs, PushcutProxy.cs, Pushover/Pushover.cs,
// PushoverProxy.cs, SendGrid/SendGrid.cs, SendGridProxy.cs, Signal/Signal.cs, SignalProxy.cs,
// Simplepush/Simplepush.cs, SimplepushProxy.cs, Slack/Slack.cs, SlackProxy.cs, Telegram/Telegram.cs,
// TelegramService.cs, Webhook/Webhook.cs, WebhookBase.cs, WebhookProxy.cs and the payload classes), GPL-3.0.

//! Sending one event to one notification, as each of Prowlarr's providers does, and each provider's test with
//! the failures Prowlarr gives. "Prowlarr" in the titles and test messages is "Lantern".

use super::super::provider::{Definition, ProviderFailure};
use super::{Event, GrabMessage};
use crate::prowlarr::health::Finding;
use crate::AppState;
use serde_json::{json, Value};
use std::sync::RwLock;

/// `RELEASE_GRABBED_TITLE`.
const GRABBED: &str = "Release Grabbed";
/// `HEALTH_ISSUE_TITLE`.
const HEALTH_ISSUE: &str = "Health Check Failure";
/// `HEALTH_RESTORED_TITLE`.
const HEALTH_RESTORED: &str = "Health Check Restored";
/// `APPLICATION_UPDATE_TITLE`.
const UPDATED: &str = "Application Updated";

/// Where the services with a fixed address (Telegram, Pushover, …) are reached instead, for tests.
static FIXED_HOSTS: RwLock<Option<String>> = RwLock::new(None);

/// Sends every service with a fixed address to `base` instead (`http://127.0.0.1:port`), keeping the path. Only
/// tests use it, so they never reach the internet.
pub fn route_fixed_hosts_to(base: &str) {
    *FIXED_HOSTS.write().unwrap_or_else(|e| e.into_inner()) = Some(base.trim_end_matches('/').to_string());
}

/// A fixed service address, or the test stand-in's.
fn fixed(url: &str) -> String {
    match FIXED_HOSTS.read().unwrap_or_else(|e| e.into_inner()).as_deref() {
        Some(base) => {
            let path = url.split_once("://").map(|(_, r)| r.find('/').map(|i| &r[i..]).unwrap_or("")).unwrap_or("");
            format!("{base}{path}")
        }
        None => url.to_string(),
    }
}

/// A failed send: the setting it is about (as the provider's test names it) and what happened.
#[derive(Debug, Clone)]
pub struct SendError {
    /// The setting (`ApiKey`, `Url`, "" for the provider as a whole).
    pub property: String,
    /// What happened, in words.
    pub message: String,
}

impl SendError {
    /// A failure of `property`.
    fn new(property: &str, message: impl Into<String>) -> SendError {
        SendError { property: property.into(), message: message.into() }
    }

    /// A failure of `property`, for the providers in modules of their own.
    pub fn new_pub(property: &str, message: impl Into<String>) -> SendError {
        SendError::new(property, message)
    }
}

/// The HTTP client notifications use.
fn http() -> reqwest::Client {
    crate::hub::clients::http_client(false)
}

/// Sends a request: the answer's status and body, or why it couldn't be sent.
async fn run(req: reqwest::RequestBuilder, what: &str) -> Result<(u16, String), SendError> {
    let r = req.send().await.map_err(|e| SendError::new("", format!("Lantern can't reach {what}: {e}")))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    Ok((status, text))
}

/// A request that must succeed: its body, or a failure naming `property` with the status.
async fn ok(req: reqwest::RequestBuilder, what: &str, property: &str) -> Result<String, SendError> {
    let (status, text) = run(req, what).await.map_err(|e| SendError::new(property, e.message))?;
    if !(200..300).contains(&status) {
        return Err(SendError::new(property, format!("{what} answered {status}: {}", text.chars().take(300).collect::<String>())));
    }
    Ok(text)
}

/// The (title, message) of an event for a provider whose titles are `branded` ("Lantern - Release Grabbed") or
/// not; `restored_branded` for the few that brand the restored title differently. `test` is the test's own.
fn title_and_message(ev: &Event, branded: bool, restored_branded: bool, test: (&str, &str)) -> (String, String) {
    let brand = |t: &str, b: bool| if b { format!("Lantern - {t}") } else { t.to_string() };
    match ev {
        Event::Grab(g) => (brand(GRABBED, branded), g.message.clone()),
        Event::HealthIssue(f) => (brand(HEALTH_ISSUE, branded), f.message.clone()),
        Event::HealthRestored(f) => {
            (brand(HEALTH_RESTORED, restored_branded), format!("The following issue is now resolved: {}", f.message))
        }
        Event::ApplicationUpdate(u) => (brand(UPDATED, branded), u.message.clone()),
        Event::Test => (test.0.to_string(), test.1.to_string()),
    }
}

/// The plain test message most providers send.
const TEST: (&str, &str) = ("Test Notification", "This is a test message from Lantern");
/// The branded test message.
const TEST_BRANDED: (&str, &str) = ("Lantern - Test Notification", "This is a test message from Lantern");

/// Sends `ev` to notification `def`.
pub async fn send(st: &AppState, def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    match def.implementation.as_str() {
        "Webhook" => webhook(st, def, ev).await,
        "Notifiarr" => notifiarr(st, def, ev).await,
        "Discord" => discord(st, def, ev).await,
        "Slack" => slack(def, ev).await,
        "Gotify" => {
            let (t, m) = title_and_message(ev, false, false, ("Test Notification", "This is a test message from Lantern"));
            let url = format!("{}/message", def.text("server").trim_end_matches('/'));
            let form = [("title", t), ("message", m), ("priority", def.setting("priority").as_i64().unwrap_or(5).to_string())];
            let (status, _) = run(http().post(url).query(&[("token", def.text("appToken"))]).form(&form), "Gotify").await?;
            match status {
                200..=299 => Ok(()),
                401 => Err(SendError::new("", "Unauthorized - AuthToken is invalid")),
                s => Err(SendError::new("", format!("Unable to connect to Gotify. Status Code: {s}"))),
            }
        }
        "Ntfy" => ntfy(def, ev).await,
        "Apprise" => apprise(def, ev).await,
        "Telegram" => telegram(def, ev).await,
        "Pushover" => {
            let (t, m) = title_and_message(ev, false, false, TEST);
            let mut form = vec![
                ("token", def.text("apiKey")),
                ("user", def.text("userKey")),
                (
                    "device",
                    def.setting("devices")
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))
                        .unwrap_or_default(),
                ),
                ("title", t),
                ("message", m),
                ("priority", def.setting("priority").as_i64().unwrap_or(0).to_string()),
            ];
            if def.setting("priority").as_i64() == Some(2) {
                form.push(("retry", def.setting("retry").as_i64().unwrap_or(0).to_string()));
                form.push(("expire", def.setting("expire").as_i64().unwrap_or(0).to_string()));
            }
            if !def.text("sound").is_empty() {
                form.push(("sound", def.text("sound")));
            }
            ok(http().post(fixed("https://api.pushover.net/1/messages.json")).form(&form), "Pushover", "ApiKey").await.map(|_| ())
        }
        "Join" => {
            let (t, m) = title_and_message(ev, true, true, ("Test Notification", "This is a test message from Lantern."));
            let mut q = vec![];
            if !def.text("deviceNames").trim().is_empty() {
                q.push(("deviceNames", def.text("deviceNames")));
            } else if !def.text("deviceIds").trim().is_empty() {
                q.push(("deviceIds", def.text("deviceIds")));
            } else {
                q.push(("deviceId", "group.all".to_string()));
            }
            // Lantern has no hosted logo, so no icon is sent
            q.extend([("apikey", def.text("apiKey")), ("title", t), ("text", m)]);
            q.push(("priority", def.setting("priority").as_i64().unwrap_or(0).to_string()));
            let text =
                ok(http().get(fixed("https://joinjoaomgcd.appspot.com/_ah/api/messaging/v1/sendPush")).query(&q), "Join", "ApiKey").await?;
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if v.get("success").and_then(Value::as_bool) == Some(false) {
                let msg = v.get("errorMessage").and_then(Value::as_str).unwrap_or("Join didn't send it.");
                if msg.contains("No devices") || msg.contains("device") {
                    return Err(SendError::new("DeviceIds", "Device IDs appear invalid."));
                }
                return Err(SendError::new("ApiKey", msg));
            }
            Ok(())
        }
        "Prowl" => {
            let (t, m) = title_and_message(ev, false, false, TEST);
            let form = [
                ("apikey", def.text("apiKey")),
                ("application", "Lantern".to_string()),
                ("event", t),
                ("description", m),
                ("priority", def.setting("priority").as_i64().unwrap_or(0).to_string()),
            ];
            ok(http().post(fixed("https://api.prowlapp.com/publicapi/add")).form(&form), "Prowl", "ApiKey").await.map(|_| ())
        }
        "PushBullet" => pushbullet(def, ev).await,
        "Pushcut" => {
            let (t, m) = title_and_message(
                ev,
                true,
                false,
                ("Lantern Test Title", "Success! You have properly configured your Pushcut notification settings."),
            );
            let url = fixed(&format!("https://api.pushcut.io/v1/notifications/{}", urlencoding(&def.text("notificationName"))));
            let mut body = json!({ "title": t, "text": m });
            if def.flag("timeSensitive") {
                body["isTimeSensitive"] = json!(true);
            }
            let (status, text) = run(http().post(url).header("API-Key", def.text("apiKey")).json(&body), "Pushcut").await?;
            match status {
                200..=299 => Ok(()),
                401 | 403 => Err(SendError::new("API Key", format!("API Key is invalid: {text}"))),
                s => Err(SendError::new("Url", format!("Unable to send test notification. Response from Pushcut: ({s}) {text}"))),
            }
        }
        "Simplepush" => {
            let (t, m) = title_and_message(ev, false, false, TEST);
            let mut form = vec![("key", def.text("key")), ("title", t), ("msg", m)];
            if !def.text("event").is_empty() {
                form.insert(1, ("event", def.text("event")));
            }
            ok(http().post(fixed("https://api.simplepush.io/send")).form(&form), "Simplepush", "ApiKey").await.map(|_| ())
        }
        "Signal" => {
            let (t, m) = title_and_message(ev, true, false, TEST);
            let scheme = if def.flag("useSsl") { "https" } else { "http" };
            let url = format!("{scheme}://{}:{}/v2/send", def.text("host"), def.setting("port").as_i64().unwrap_or(0));
            let body =
                json!({ "message": format!("{t}\n\n{m}"), "number": def.text("senderNumber"), "recipients": [def.text("receiverId")] });
            let mut req = http().post(url).json(&body);
            if !def.text("authUsername").is_empty() || !def.text("authPassword").is_empty() {
                req = req.basic_auth(def.text("authUsername"), Some(def.text("authPassword")));
            }
            let (status, text) = run(req, "Signal").await.map_err(|e| SendError::new("Host", e.message))?;
            match status {
                200..=299 => Ok(()),
                401 => Err(SendError::new("AuthUsername", "Login/Password invalid")),
                400 if text.contains("plain HTTP request was sent to HTTPS port") => {
                    Err(SendError::new("UseSsl", "SSL seems to be required"))
                }
                _ => {
                    let err =
                        serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
                    Err(SendError::new("Host", format!("Unable to send test message: {}", err.unwrap_or(text))))
                }
            }
        }
        "Mailgun" => {
            let (t, m) = title_and_message(ev, false, true, ("Test Notification", "This is a test message from Lantern, though Mailgun."));
            let base = if def.flag("useEuEndpoint") { "https://api.eu.mailgun.net/v3" } else { "https://api.mailgun.net/v3" };
            let url = fixed(&format!("{base}/{}/messages", def.text("senderDomain")));
            for to in def.setting("recipients").as_array().cloned().unwrap_or_default().iter().filter_map(Value::as_str) {
                let form = [("from", def.text("from")), ("to", to.to_string()), ("subject", t.clone()), ("text", m.clone())];
                let (status, _) = run(http().post(&url).basic_auth("api", Some(def.text("apiKey"))).form(&form), "Mailgun").await?;
                match status {
                    200..=299 => {}
                    401 => return Err(SendError::new("", "Unauthorized - ApiKey is invalid")),
                    s => return Err(SendError::new("", format!("Unable to connect to Mailgun. Status code: {s}"))),
                }
            }
            Ok(())
        }
        "SendGrid" => {
            let (t, m) = title_and_message(ev, false, false, TEST);
            let to: Vec<Value> =
                def.setting("recipients").as_array().cloned().unwrap_or_default().iter().map(|r| json!({ "email": r })).collect();
            let body = json!({
                "from": { "email": def.text("from") },
                "personalizations": [{ "to": to, "subject": t }],
                "content": [{ "type": "text/plain", "value": m }],
            });
            let (status, _) =
                run(http().post(fixed("https://api.sendgrid.com/v3/mail/send")).bearer_auth(def.text("apiKey")).json(&body), "SendGrid")
                    .await?;
            match status {
                200..=299 => Ok(()),
                401 => Err(SendError::new("", "Unauthorized - AuthToken is invalid")),
                s => Err(SendError::new("", format!("Unable to connect to SendGrid. Status Code: {s}"))),
            }
        }
        "Email" => super::email::send(def, ev).await,
        "CustomScript" => super::script::run(st, def, ev).await,
        "Twitter" => Err(SendError::new("", "Lantern can't post to Twitter yet. Use another notification for now.")),
        other => Err(SendError::new("", format!("Lantern doesn't know the notification {other}."))),
    }
}

/// An address part made safe for a URL.
fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Tests notification `def` (`Test`): Prowlarr's failure for each provider.
pub async fn test(st: &AppState, def: &Definition) -> Vec<ProviderFailure> {
    match send(st, def, &Event::Test).await {
        Ok(()) => Vec::new(),
        Err(e) => {
            let property = match def.implementation.as_str() {
                "Discord" | "Slack" => "Unable to post".to_string(),
                "Webhook" | "Notifiarr" if e.property.is_empty() => "Url".to_string(),
                "Gotify" | "Mailgun" | "SendGrid" => String::new(),
                "Telegram" if e.property.is_empty() => "BotToken".to_string(),
                _ => e.property.clone(),
            };
            let message = match def.implementation.as_str() {
                "Gotify" | "Mailgun" | "SendGrid" if !e.message.starts_with("Unauthorized") => "Unable to send test message".to_string(),
                _ => e.message,
            };
            vec![ProviderFailure::new(&property, message)]
        }
    }
}

// ---- webhook and Notifiarr ----

/// The webhook payload of an event (`WebhookBase.Build*Payload`).
pub fn webhook_payload(st: &AppState, ev: &Event) -> Value {
    let instance = crate::prowlarr::config::instance_name(st);
    let app_url = st.settings.str("public_url");
    let mut v = match ev {
        Event::Grab(g) => grab_payload(g),
        Event::HealthIssue(f) => health_payload(f, "Health"),
        Event::HealthRestored(f) => health_payload(f, "HealthRestored"),
        Event::ApplicationUpdate(u) => json!({
            "eventType": "ApplicationUpdate",
            "message": u.message,
            "previousVersion": u.previous_version,
            "newVersion": u.new_version,
        }),
        Event::Test => json!({ "eventType": "Test" }),
    };
    v["instanceName"] = json!(instance);
    if !matches!(ev, Event::HealthRestored(_)) && !app_url.is_empty() {
        v["applicationUrl"] = json!(app_url);
    }
    v
}

/// A grab's webhook payload.
fn grab_payload(g: &GrabMessage) -> Value {
    let r = &g.release;
    let mut release = json!({
        "releaseTitle": r.title,
        "indexer": r.indexer,
        "categories": r.categories.iter().map(|c| crate::indexers::categories::name_of(*c).unwrap_or("")).collect::<Vec<_>>(),
        "genres": r.genre.split(',').map(str::trim).filter(|g| !g.is_empty()).collect::<Vec<_>>(),
        "indexerFlags": r.flags,
    });
    if let Some(s) = r.size {
        release["size"] = json!(s);
    }
    if let Some(d) = r.date {
        release["publishDate"] = json!(d.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    }
    let mut v = json!({
        "eventType": "Grab",
        "release": release,
        "trigger": if g.manual { "Manual" } else { "Api" },
        "source": g.source,
        "host": g.host,
    });
    for (k, val) in
        [("downloadClient", &g.download_client_name), ("downloadClientType", &g.download_client_type), ("downloadId", &g.download_id)]
    {
        if !val.is_empty() {
            v[k] = json!(val);
        }
    }
    v
}

/// A health webhook payload.
fn health_payload(f: &Finding, event: &str) -> Value {
    json!({ "eventType": event, "level": level(f), "message": f.message, "type": f.source, "wikiUrl": f.wiki_url })
}

/// A finding's level as Prowlarr writes `HealthCheckResult` (`Warning`, `Error`).
fn level(f: &Finding) -> &'static str {
    match f.kind {
        "error" => "Error",
        "notice" => "Notice",
        _ => "Warning",
    }
}

/// `Webhook`: the payload as JSON, by POST or PUT, with basic auth when a user name or password is set.
async fn webhook(st: &AppState, def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let payload = webhook_payload(st, ev);
    let url = def.text("url");
    let mut req = match def.setting("method").as_i64().unwrap_or(1) {
        2 => http().put(&url),
        _ => http().post(&url),
    }
    .header("Accept", "application/json")
    .json(&payload);
    if !def.text("username").is_empty() || !def.text("password").is_empty() {
        req = req.basic_auth(def.text("username"), Some(def.text("password")));
    }
    ok(req, "the webhook", "Url").await.map(|_| ()).map_err(|e| SendError::new("Url", format!("Unable to post to webhook: {}", e.message)))
}

/// `Notifiarr`: the webhook payload to Notifiarr's Prowlarr endpoint, with the API key.
async fn notifiarr(st: &AppState, def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let payload = webhook_payload(st, ev);
    let req =
        http().post(fixed("https://notifiarr.com/api/v1/notification/prowlarr")).header("X-API-Key", def.text("aPIKey")).json(&payload);
    let (status, text) = run(req, "Notifiarr").await?;
    match status {
        200..=299 => Ok(()),
        401 => Err(SendError::new("APIKey", "API key is invalid")),
        s => Err(SendError::new("", format!("Unable to send notification: ({s}) {text}"))),
    }
}

// ---- Discord and Slack ----

/// `Discord`: an embed for each event, a plain message for the test.
async fn discord(st: &AppState, def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    const DANGER: i64 = 15749200;
    const SUCCESS: i64 = 2605644;
    const WARNING: i64 = 16753920;
    const STANDARD: i64 = 16761392;
    let author = if def.text("author").trim().is_empty() { crate::prowlarr::config::instance_name(st) } else { def.text("author") };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let embed = |title: String, description: Option<String>, color: i64, fields: Vec<Value>| {
        let mut e = json!({ "author": { "name": author }, "title": title, "timestamp": now, "color": color });
        if let Some(d) = description {
            e["description"] = json!(d);
        }
        if !fields.is_empty() {
            e["fields"] = json!(fields);
        }
        e
    };
    let (content, embeds) = match ev {
        Event::Grab(g) => {
            let mut fields = Vec::new();
            for f in def.ints("grabFields") {
                let (name, value) = match f {
                    0 => ("Release", format!("```{}```", g.release.title)),
                    1 => ("Indexer", g.release.indexer.clone()),
                    2 => ("Download Client", g.download_client_name.clone()),
                    3 => ("Grab Trigger", if g.manual { "Manual".into() } else { "Api".into() }),
                    4 => ("Source", g.source.clone()),
                    5 => ("Host", g.host.clone()),
                    _ => continue,
                };
                if !value.trim().is_empty() {
                    fields.push(json!({ "name": name, "value": value, "inline": false }));
                }
            }
            (None, vec![embed(GRABBED.into(), Some(g.message.clone()), if g.successful { SUCCESS } else { DANGER }, fields)])
        }
        Event::HealthIssue(f) => {
            (None, vec![embed(f.source.into(), Some(f.message.clone()), if f.kind == "warning" { WARNING } else { DANGER }, vec![])])
        }
        Event::HealthRestored(f) => (
            None,
            vec![embed(
                format!("Health Issue Resolved: {}", f.source),
                Some(format!("The following issue is now resolved: {}", f.message)),
                SUCCESS,
                vec![],
            )],
        ),
        Event::ApplicationUpdate(u) => {
            let fields = vec![
                json!({ "name": "Previous Version", "value": u.previous_version, "inline": false }),
                json!({ "name": "New Version", "value": u.new_version, "inline": false }),
            ];
            (None, vec![embed(UPDATED.into(), None, STANDARD, fields)])
        }
        Event::Test => {
            (Some(format!("Test message from Lantern posted at {}", chrono::Local::now().format("%-m/%-d/%Y %-I:%M:%S %p"))), vec![])
        }
    };
    let mut payload = json!({});
    if let Some(c) = content {
        payload["content"] = json!(c);
    }
    if !embeds.is_empty() {
        payload["embeds"] = json!(embeds);
    }
    if !def.text("username").trim().is_empty() {
        payload["username"] = json!(def.text("username"));
    }
    if !def.text("avatar").trim().is_empty() {
        payload["avatar_url"] = json!(def.text("avatar"));
    }
    ok(http().post(def.text("webHookUrl")).json(&payload), "Discord", "")
        .await
        .map(|_| ())
        .map_err(|_| SendError::new("", "Unable to post payload"))
}

/// `Slack`: an attachment for each event, a plain message for the test (Slack has no grab notification).
async fn slack(def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let (text, attachments) = match ev {
        Event::HealthIssue(f) => (
            "Health Issue".to_string(),
            json!([{ "title": f.source, "text": f.message, "color": if f.kind == "warning" { "warning" } else { "danger" } }]),
        ),
        Event::HealthRestored(f) => (
            "Health Issue Resolved".to_string(),
            json!([{ "title": f.source, "text": format!("The following issue is now resolved: {}", f.message), "color": "good" }]),
        ),
        Event::ApplicationUpdate(u) => {
            ("Application Updated".to_string(), json!([{ "title": hostname(), "text": u.message, "color": "good" }]))
        }
        Event::Grab(_) => return Ok(()),
        Event::Test => {
            (format!("Test message from Lantern posted at {}", chrono::Local::now().format("%-m/%-d/%Y %-I:%M:%S %p")), Value::Null)
        }
    };
    let mut payload = json!({ "text": text });
    if !attachments.is_null() {
        payload["attachments"] = attachments;
    }
    if !def.text("username").is_empty() {
        payload["username"] = json!(def.text("username"));
    }
    let icon = def.text("icon");
    if !icon.trim().is_empty() {
        if icon.starts_with(':') && icon.ends_with(':') {
            payload["icon_emoji"] = json!(icon);
        } else {
            payload["icon_url"] = json!(icon);
        }
    }
    if !def.text("channel").trim().is_empty() {
        payload["channel"] = json!(def.text("channel"));
    }
    ok(http().post(def.text("webHookUrl")).json(&payload), "Slack", "")
        .await
        .map(|_| ())
        .map_err(|_| SendError::new("", "Unable to post payload"))
}

/// The machine's name (`Environment.MachineName`).
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Lantern".into())
}

// ---- ntfy, Apprise, Telegram, Pushbullet ----

/// `Ntfy`: one POST per topic with the title, message, priority, tags and click address in the query.
async fn ntfy(def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let (t, m) = title_and_message(ev, true, true, TEST_BRANDED);
    let server = def.text("serverUrl");
    let server = if server.trim().is_empty() { fixed("https://ntfy.sh") } else { server };
    let mut failed = None;
    for topic in def.setting("topics").as_array().cloned().unwrap_or_default().iter().filter_map(Value::as_str) {
        let mut q =
            vec![("title", t.clone()), ("message", m.clone()), ("priority", def.setting("priority").as_i64().unwrap_or(3).to_string())];
        let tags: Vec<String> =
            def.setting("tags").as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if !tags.is_empty() {
            q.push(("tags", tags.join(",")));
        }
        if !def.text("clickUrl").trim().is_empty() {
            q.push(("click", def.text("clickUrl")));
        }
        let mut req = http().post(format!("{}/{topic}", server.trim_end_matches('/'))).query(&q);
        if !def.text("accessToken").trim().is_empty() {
            req = req.bearer_auth(def.text("accessToken"));
        } else if !def.text("userName").trim().is_empty() && !def.text("password").trim().is_empty() {
            req = req.basic_auth(def.text("userName"), Some(def.text("password")));
        }
        let (status, _) = run(req, "ntfy").await?;
        if status == 401 || status == 403 {
            let e = if !def.text("accessToken").trim().is_empty() {
                SendError::new("AccessToken", "Invalid token")
            } else if !def.text("userName").trim().is_empty() && !def.text("password").trim().is_empty() {
                SendError::new("UserName", "Invalid username or password")
            } else {
                SendError::new("AccessToken", "Authorization is required")
            };
            return Err(e);
        }
        if !(200..300).contains(&status) {
            failed = Some(SendError::new("ServerUrl", "Unable to send test message"));
        }
    }
    failed.map_or(Ok(()), Err)
}

/// `Apprise`: the title, body and type as JSON to `/notify/{key}` or (with stateless URLs) `/notify`.
async fn apprise(def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let (t, m) = title_and_message(
        ev,
        true,
        false,
        ("Lantern - Test Notification", "Success! You have properly configured your apprise notification settings."),
    );
    let types = ["info", "success", "warning", "failure"];
    let kind = types.get(def.setting("notificationType").as_i64().unwrap_or(0) as usize).copied().unwrap_or("info");
    let mut payload = json!({ "title": t, "body": m, "type": kind });
    let base = def.text("serverUrl").trim_end_matches(['/', ' ']).to_string();
    let url = if !def.text("configurationKey").trim().is_empty() {
        format!("{base}/notify/{}", urlencoding(&def.text("configurationKey")))
    } else {
        payload["urls"] = json!(def.text("statelessUrls"));
        format!("{base}/notify")
    };
    let tags: Vec<String> =
        def.setting("tags").as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if !tags.is_empty() {
        payload["tag"] = json!(tags.join(","));
    }
    let mut req = http().post(url).header("Accept", "application/json").json(&payload);
    if !def.text("authUsername").is_empty() || !def.text("authPassword").is_empty() {
        req = req.basic_auth(def.text("authUsername"), Some(def.text("authPassword")));
    }
    let (status, text) =
        run(req, "Apprise").await.map_err(|e| SendError::new("Url", format!("Unable to send test message: {}", e.message)))?;
    match status {
        200..=299 => Ok(()),
        401 => Err(SendError::new("AuthUsername", format!("HTTP Auth credentials are invalid: ({status})"))),
        _ if !text.trim().is_empty() => {
            let err = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                .unwrap_or(text);
            Err(SendError::new("", format!("Unable to send test message. Response from API: {err}")))
        }
        s => Err(SendError::new("Url", format!("Unable to connect to Apprise API. Server connection failed: ({s})"))),
    }
}

/// Text made safe in Telegram's HTML.
fn html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `Telegram`: `sendmessage` with the title in bold.
async fn telegram(def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let branded = def.flag("includeAppNameInTitle");
    let test = if branded { TEST_BRANDED } else { TEST };
    let (t, m) = title_and_message(ev, branded, branded, test);
    let text = format!("<b>{}</b>\n{}", html(&t), html(&m));
    let url = fixed(&format!("https://api.telegram.org/bot{}/sendmessage", def.text("botToken")));
    let mut form = vec![
        ("chat_id", def.text("chatId")),
        ("parse_mode", "HTML".to_string()),
        ("text", text),
        ("disable_notification", if def.flag("sendSilently") { "True".into() } else { "False".into() }),
    ];
    if let Some(topic) = def.setting("topicId").as_i64() {
        form.push(("message_thread_id", topic.to_string()));
    }
    let (status, body) = run(http().post(url).form(&form), "Telegram").await.map_err(|e| SendError::new("Connection", e.message))?;
    if (200..300).contains(&status) {
        return Ok(());
    }
    let description =
        serde_json::from_str::<Value>(&body).ok().and_then(|v| v.get("description").and_then(Value::as_str).map(str::to_string));
    match description {
        Some(d) => {
            let property = if d.contains("chat not found") || d.contains("USER_ID_INVALID") { "ChatId" } else { "BotToken" };
            Err(SendError::new(property, d))
        }
        None => Err(SendError::new("BotToken", "Unable to send test message")),
    }
}

/// `PushBullet`: a note to each device (or channel), or to all when none is set.
async fn pushbullet(def: &Definition, ev: &Event<'_>) -> Result<(), SendError> {
    let (t, m) = title_and_message(ev, true, true, TEST_BRANDED);
    let list = |k: &str| -> Vec<String> {
        def.setting(k).as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    };
    let (devices, channels) = (list("deviceIds"), list("channelTags"));
    let mut targets: Vec<Vec<(&str, String)>> = Vec::new();
    for d in &devices {
        targets.push(if d.parse::<i64>().is_ok() { vec![("device_id", d.clone())] } else { vec![("device_iden", d.clone())] });
    }
    for c in &channels {
        targets.push(vec![("channel_tag", c.clone())]);
    }
    if targets.is_empty() {
        targets.push(vec![]);
    }
    for mut form in targets {
        form.extend([("type", "note".to_string()), ("title", t.clone()), ("body", m.clone())]);
        if !def.text("senderId").is_empty() {
            form.push(("source_device_iden", def.text("senderId")));
        }
        let (status, _) = run(
            http().post(fixed("https://api.pushbullet.com/v2/pushes")).basic_auth(def.text("apiKey"), Some("")).form(&form),
            "Pushbullet",
        )
        .await?;
        match status {
            200..=299 => {}
            401 => return Err(SendError::new("ApiKey", "API Key is invalid")),
            _ => return Err(SendError::new("ApiKey", "Unable to send test message")),
        }
    }
    Ok(())
}
