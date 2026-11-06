// Ported from autobrr v1.90.0 (internal/domain/notification.go, internal/notification/service.go, base.go,
// message_builder.go, inbox.go, internal/database/notification.go, notification_inbox.go), GPL-2.0-or-later.
//! autobrr's notifications: the places to tell (Discord, Gotify, LunaSea, Notifiarr, Ntfy, Pushover, Shoutrrr,
//! Telegram, Bark, a webhook and the built-in inbox), which events each wants, and the filters that pick their
//! own. An event goes to the notifications a filter names for it when the filter has any, else to every
//! notification that wants that event (autobrr's routing snapshot, read from the tables each time).
//!
//! The notifications are rows of `autobrr_notifications` (the record as JSON in `body`), the built-in inbox's
//! messages rows of `autobrr_notification_inbox`.

#[cfg(test)]
mod sender_tests;
pub mod senders;

use super::release::Release;
use super::store::{ApiError, ApiResult};
use super::Releases;
use chrono::{DateTime, SecondsFormat, Utc};
use kit_server_core::{p, Db, Row, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// autobrr's `ErrNotificationBuiltin`.
pub const ERR_BUILTIN: &str = "built-in notification can not be created, deleted or change type";
/// autobrr's `ErrNotificationInvalid`.
pub const ERR_INVALID: &str = "invalid notification settings";
/// The stream the inbox's messages and changes go out on (autobrr's `InboxStreamKey`).
pub const INBOX_STREAM: &str = "notifications";
/// How many inbox messages are kept (autobrr's `inboxMaxMessages`).
pub const INBOX_MAX_MESSAGES: i64 = 1000;
/// How old an inbox message may get, in days (autobrr's `inboxMaxAge`).
pub const INBOX_MAX_AGE_DAYS: i64 = 30;

/// Every kind of notification autobrr has.
pub const TYPES: [&str; 12] =
    ["DISCORD", "NOTIFIARR", "PUSHOVER", "SLACK", "TELEGRAM", "GOTIFY", "NTFY", "LUNASEA", "SHOUTRRR", "WEBHOOK", "BARK", "BUILTIN"];

/// One notification (autobrr's `domain.Notification`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notification {
    /// Its id.
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// The kind (`DISCORD` …).
    #[serde(rename = "type")]
    pub kind: String,
    /// Whether it sends at all.
    pub enabled: bool,
    /// The events it wants when a filter doesn't pick its own.
    pub events: Vec<String>,
    /// A token (Gotify's app token, Telegram's bot token, Pushover's user key, Bark's device key, Ntfy's token).
    pub token: String,
    /// An API key (Notifiarr's, Pushover's app token).
    pub api_key: String,
    /// The webhook address (Discord, LunaSea, webhook).
    pub webhook: String,
    /// A title (not used by the senders).
    pub title: String,
    /// An icon (not used by the senders).
    pub icon: String,
    /// A user name (Ntfy; Telegram's sender line).
    pub username: String,
    /// The server (Gotify, Ntfy, Bark, a Telegram API host, Shoutrrr's URL).
    pub host: String,
    /// A password (Ntfy).
    pub password: String,
    /// Telegram's chat id.
    pub channel: String,
    /// Rooms (not used by the senders).
    pub rooms: String,
    /// Targets (not used by the senders).
    pub targets: String,
    /// Devices (not used by the senders).
    pub devices: String,
    /// The priority (Pushover, Ntfy, Bark).
    pub priority: i32,
    /// Ntfy's tags, Telegram's topic (thread), Bark's group.
    pub topic: String,
    /// Pushover's and Bark's sound.
    pub sound: String,
    /// Pushover's sound per event.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub event_sounds: BTreeMap<String, String>,
    /// The filters that pick their own events for it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub used_by_filters: Vec<super::store::FilterNotification>,
    /// The webhook's method (POST when empty).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// The webhook's headers, `KEY=value,KEY2=value2`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub headers: String,
    /// When it was added.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

impl Notification {
    /// The notification as autobrr's API sends it: token, API key and password redacted.
    pub fn public(&self) -> Value {
        let mut n = self.clone();
        n.token = super::downloader::redact(&n.token);
        n.api_key = super::downloader::redact(&n.api_key);
        n.password = super::downloader::redact(&n.password);
        let mut v = serde_json::to_value(&n).unwrap_or_default();
        for k in ["created_at", "updated_at"] {
            if v[k].as_str().is_some_and(str::is_empty) {
                v[k] = json!("0001-01-01T00:00:00Z");
            }
        }
        v
    }

    /// Whether it can send: on, with what its kind needs (autobrr's `IsEnabled`).
    pub fn is_enabled(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let has = |s: &str| !s.is_empty();
        match self.kind.as_str() {
            "BARK" | "GOTIFY" => has(&self.host) && has(&self.token),
            "DISCORD" | "LUNASEA" | "WEBHOOK" => has(&self.webhook),
            "NOTIFIARR" => has(&self.api_key),
            "NTFY" | "SHOUTRRR" => has(&self.host),
            "PUSHOVER" => has(&self.api_key) && has(&self.token),
            "TELEGRAM" => has(&self.token) && has(&self.channel),
            "BUILTIN" => true,
            _ => false,
        }
    }

    /// Checks what its kind needs (autobrr's `Validate`); the problem in autobrr's words.
    pub fn validate(&self) -> Result<(), String> {
        let blank = |s: &str| s.trim().is_empty();
        let bad = |m: &str| Err(format!("{m}: {ERR_INVALID}"));
        match self.kind.as_str() {
            "BARK" => {
                if blank(&self.host) {
                    return bad("missing bark server url");
                }
                let ok = url::Url::parse(&self.host).ok().is_some_and(|u| {
                    u.host_str().is_some_and(|h| !h.is_empty()) && u.query().is_none() && (u.scheme() == "http" || u.scheme() == "https")
                });
                if !ok {
                    return bad("bark server url must be an http or https url without query parameters");
                }
                if blank(&self.token) {
                    return bad("missing bark device key");
                }
            }
            "DISCORD" | "LUNASEA" | "WEBHOOK" => {
                if blank(&self.webhook) {
                    return bad("missing webhook url");
                }
            }
            "GOTIFY" => {
                if blank(&self.host) {
                    return bad("missing gotify url");
                }
                if blank(&self.token) {
                    return bad("missing gotify application token");
                }
            }
            "NOTIFIARR" => {
                if blank(&self.api_key) {
                    return bad("missing notifiarr api key");
                }
                let ok =
                    self.api_key.len() == 36 && self.api_key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
                if !ok {
                    return bad("notifiarr api key must be 36 characters of a-z, 0-9 and dashes");
                }
            }
            "NTFY" | "SHOUTRRR" => {
                if blank(&self.host) {
                    return bad("missing url");
                }
            }
            "PUSHOVER" => {
                if blank(&self.api_key) {
                    return bad("missing pushover api token");
                }
                if blank(&self.token) {
                    return bad("missing pushover user key");
                }
            }
            "TELEGRAM" => {
                if blank(&self.token) {
                    return bad("missing telegram bot token");
                }
                if blank(&self.channel) {
                    return bad("missing telegram chat id");
                }
            }
            "BUILTIN" => {}
            other => return bad(&format!("unsupported notification type: '{other}'")),
        }
        Ok(())
    }
}

/// What happened, to tell (autobrr's `NotificationPayload`).
#[derive(Debug, Clone, Default)]
pub struct Payload {
    /// A subject line.
    pub subject: String,
    /// The message.
    pub message: String,
    /// The event (`PUSH_APPROVED` …).
    pub event: String,
    /// The release's name.
    pub release_name: String,
    /// The filter's name.
    pub filter: String,
    /// The filter's id.
    pub filter_id: i64,
    /// The list's name.
    pub list: String,
    /// The list's id.
    pub list_id: i64,
    /// The list's kind.
    pub list_type: String,
    /// The list's problem.
    pub list_error: String,
    /// The feed's name.
    pub feed: String,
    /// The feed's id.
    pub feed_id: i64,
    /// The feed's kind.
    pub feed_type: String,
    /// The feed's problem.
    pub feed_error: String,
    /// The IRC network.
    pub irc_network: String,
    /// What the IRC network said.
    pub irc_message: String,
    /// The indexer's name.
    pub indexer: String,
    /// The torrent's info hash.
    pub info_hash: String,
    /// The release's size.
    pub size: u64,
    /// `PUSH_APPROVED`, `PUSH_REJECTED`, `PUSH_ERROR` or `PENDING`.
    pub status: String,
    /// The action's name.
    pub action: String,
    /// The action's kind.
    pub action_type: String,
    /// The action's download client.
    pub action_client: String,
    /// Why it was turned down, or the error.
    pub rejections: Vec<String>,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// `IRC`, `TORZNAB`, `NEWZNAB` or `RSS`.
    pub implementation: String,
    /// When it happened.
    pub timestamp: Option<DateTime<Utc>>,
    /// A sender line (Telegram's user name).
    pub sender: String,
    /// The whole release (the webhook sends it).
    pub release: Option<Box<Release>>,
    /// The running version (update events).
    pub current_version: String,
    /// The new version (update events).
    pub new_version: String,
    /// A link (update events).
    pub url: String,
}

/// A push status in words (autobrr's `ReleasePushStatus.String`).
pub fn status_words(s: &str) -> &'static str {
    match s {
        "PENDING" => "Pending",
        "PUSH_APPROVED" => "Approved",
        "PUSH_REJECTED" => "Rejected",
        "PUSH_ERROR" => "Error",
        _ => "Unknown",
    }
}

/// Go's `html.EscapeString`.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('\'', "&#39;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;")
}

/// The message body as plain text (autobrr's `MessageBuilderPlainText`).
pub fn plain_body(p: &Payload) -> String {
    let mut b = String::new();
    if !p.sender.is_empty() {
        b += &format!("{}\n", p.sender);
    }
    if !p.subject.is_empty() && !p.message.is_empty() {
        b += &format!("{}\n{}\n", p.subject, p.message);
    }
    if !p.release_name.is_empty() {
        b += &format!("New release: {}\n", p.release_name);
    }
    if p.size > 0 {
        b += &format!("Size: {}\n", super::humanize::bytes(p.size));
    }
    if !p.status.is_empty() {
        b += &format!("Status: {}\n", status_words(&p.status));
    }
    if !p.indexer.is_empty() {
        b += &format!("Indexer: {}\n", p.indexer);
    }
    if !p.filter.is_empty() {
        b += &format!("Filter: {}\n", p.filter);
    }
    if !p.action.is_empty() {
        b += &format!("Action: {}: {}\n", p.action_type, p.action);
        if !p.action_client.is_empty() {
            b += &format!("Client: {}\n", p.action_client);
        }
    }
    if !p.rejections.is_empty() {
        b += &format!("Rejections: {}\n", p.rejections.join(", "));
    }
    b
}

/// The message body as HTML (autobrr's `MessageBuilderHTML`).
pub fn html_body(p: &Payload) -> String {
    let e = html_escape;
    let mut b = String::new();
    if !p.sender.is_empty() {
        b += &format!("<b>{}</b>\n", e(&p.sender));
    }
    if !p.subject.is_empty() && !p.message.is_empty() {
        b += &format!("<b>{}</b> {}\n", e(&p.subject), e(&p.message));
    }
    if !p.release_name.is_empty() {
        b += &format!("<b>New release:</b> {}\n", e(&p.release_name));
    }
    if p.size > 0 {
        b += &format!("<b>Size:</b> {}\n", super::humanize::bytes(p.size));
    }
    if !p.status.is_empty() {
        b += &format!("<b>Status:</b> {}\n", e(status_words(&p.status)));
    }
    if !p.indexer.is_empty() {
        b += &format!("<b>Indexer:</b> {}\n", e(&p.indexer));
    }
    if !p.filter.is_empty() {
        b += &format!("<b>Filter:</b> {}\n", e(&p.filter));
    }
    if !p.action.is_empty() {
        b += &format!("<b>Action:</b> {}: {}\n", p.action_type, e(&p.action));
        if !p.action_client.is_empty() {
            b += &format!("<b>Client:</b> {}\n", e(&p.action_client));
        }
    }
    if !p.rejections.is_empty() {
        b += &format!("<b>Rejections:</b> {}\n", e(&p.rejections.join(", ")));
    }
    b
}

/// The title for an event (autobrr's `BuildTitle`).
pub fn title(event: &str) -> &'static str {
    match event {
        "APP_UPDATE_AVAILABLE" => "Autobrr update available",
        "PUSH_APPROVED" => "Push Approved",
        "PUSH_REJECTED" => "Push Rejected",
        "PUSH_ERROR" => "Push Error",
        "IRC_DISCONNECTED" => "IRC Disconnected",
        "IRC_RECONNECTED" => "IRC Reconnected",
        "IRC_UNHEALTHY" => "IRC Unhealthy",
        "IRC_HEALTHY" => "IRC Healthy",
        "LIST_REFRESH_SUCCESS" => "List Refresh Success",
        "LIST_REFRESH_ERROR" => "List Refresh Error",
        "FEED_REFRESH_SUCCESS" => "Feed Refresh Success",
        "FEED_REFRESH_ERROR" => "Feed Refresh Error",
        "RELEASE_NEW" => "New Release",
        "TEST" => "Test",
        _ => "New Event",
    }
}

/// An inbox message (autobrr's `InboxMessage`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InboxMessage {
    /// Its id.
    pub id: i64,
    /// The event.
    pub event: String,
    /// The title.
    pub title: String,
    /// The message (empty when it only repeats the release's name).
    pub message: String,
    /// The release's name.
    pub release_name: String,
    /// The indexer.
    pub indexer: String,
    /// The filter's name.
    pub filter_name: String,
    /// The filter's id.
    pub filter_id: i64,
    /// The action.
    pub action: String,
    /// The action's client.
    pub action_client: String,
    /// Why it was turned down.
    pub rejections: Vec<String>,
    /// A link.
    pub url: String,
    /// When it was read (`null`: not yet).
    pub read_at: Option<String>,
    /// When it came.
    pub created_at: String,
}

/// The time as autobrr's JSON writes it.
pub fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// The notification tables, with the built-in notification autobrr seeds.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_notification_inbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event TEXT NOT NULL,
            title TEXT NOT NULL,
            message TEXT NOT NULL,
            release_name TEXT,
            indexer TEXT,
            filter_name TEXT,
            filter_id INTEGER,
            action TEXT,
            action_client TEXT,
            rejections TEXT NOT NULL DEFAULT '[]',
            url TEXT,
            read_at TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS autobrr_notification_inbox_created_at ON autobrr_notification_inbox (created_at);",
    )
    .map_err(|e| anyhow::anyhow!("The notification tables can't be made: {}", e.detail))?;
    let has = db.one("SELECT 1 AS y FROM autobrr_notifications WHERE kind='BUILTIN'", p![]).map_err(|e| anyhow::anyhow!(e.detail))?;
    if has.is_none() {
        let now = now_text();
        let body = json!({ "events": ["PUSH_ERROR", "IRC_DISCONNECTED", "APP_UPDATE_AVAILABLE"], "created_at": now, "updated_at": now });
        db.execute(
            "INSERT INTO autobrr_notifications (name, kind, enabled, body) VALUES ('Built-in', 'BUILTIN', 1, ?)",
            p![body.to_string()],
        )
        .map_err(|e| anyhow::anyhow!(e.detail))?;
    }
    Ok(())
}

/// A notification from its row.
fn notification_of(r: &Row) -> Notification {
    let mut n: Notification = serde_json::from_str(&r.s("body")).unwrap_or_default();
    n.id = r.i("id");
    n.name = r.s("name");
    n.kind = r.s("kind");
    n.enabled = r.b("enabled");
    n.used_by_filters.clear();
    n
}

/// The row body of `n`: everything but the columns.
fn body_of(n: &Notification) -> String {
    let mut v = serde_json::to_value(n).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        for k in ["id", "name", "type", "enabled", "used_by_filters"] {
            o.remove(k);
        }
    }
    v.to_string()
}

/// Sends what happened to the notifications that want it, and keeps the built-in inbox's live stream.
#[derive(Clone)]
pub struct Notifier {
    /// How each kind is sent.
    pub senders: Arc<senders::Senders>,
    /// The inbox's live stream (`NOTIFICATION` and `INBOX_CHANGED` events).
    pub inbox_events: broadcast::Sender<super::irc::Event>,
}

impl Default for Notifier {
    fn default() -> Notifier {
        Notifier { senders: Arc::new(senders::Senders::default()), inbox_events: broadcast::channel(256).0 }
    }
}

impl Releases {
    // ---- the notifications (autobrr's NotificationRepo and Service) ----

    /// Every notification by name, with the filters that pick their own events (autobrr's `Find`).
    pub fn notifications(&self) -> ApiResult<Vec<Notification>> {
        let mut list: Vec<Notification> =
            self.db.query("SELECT * FROM autobrr_notifications ORDER BY name", p![])?.iter().map(notification_of).collect();
        for n in &mut list {
            n.title.clear();
            n.icon.clear();
            n.rooms.clear();
            n.targets.clear();
            n.devices.clear();
            n.used_by_filters = self.notification_filters(n.id)?;
        }
        Ok(list)
    }

    /// The filters that pick their own events for notification `id` (autobrr's `GetNotificationFilters`).
    fn notification_filters(&self, id: i64) -> ApiResult<Vec<super::store::FilterNotification>> {
        Ok(self
            .db
            .query(
                "SELECT fn.filter_id, fn.notification_id, fn.events, f.name AS filter_name FROM autobrr_filter_notifications fn \
                 JOIN release_filters f ON f.id = fn.filter_id WHERE fn.notification_id=? ORDER BY f.name",
                p![id],
            )?
            .iter()
            .map(|r| super::store::FilterNotification {
                filter_id: r.i("filter_id"),
                filter_name: r.s("filter_name"),
                notification_id: r.i("notification_id"),
                events: serde_json::from_str(&r.s("events")).unwrap_or_default(),
            })
            .collect())
    }

    /// Notification `id`; autobrr's `sql: no rows in result set` when there is none.
    pub fn notification(&self, id: i64) -> ApiResult<Notification> {
        self.db
            .one("SELECT * FROM autobrr_notifications WHERE id=?", p![id])?
            .map(|r| notification_of(&r))
            .ok_or_else(|| ApiError::NotFound("sql: no rows in result set".into()))
    }

    /// Saves a new notification (autobrr's `Store`): never a second built-in one, checked when it is on.
    pub fn store_notification(&self, mut n: Notification) -> ApiResult<Notification> {
        if n.kind == "BUILTIN" {
            return Err(ApiError::BadRequest(ERR_BUILTIN.into()));
        }
        if n.enabled {
            n.validate().map_err(ApiError::BadRequest)?;
        }
        let now = now_text();
        n.created_at.clone_from(&now);
        n.updated_at = now;
        n.id = self.db.execute(
            "INSERT INTO autobrr_notifications (name, kind, enabled, body) VALUES (?,?,?,?)",
            p![n.name, n.kind, n.enabled, body_of(&n)],
        )?;
        Ok(n)
    }

    /// Changes a notification (autobrr's `Update`): redacted secrets keep the saved ones, the built-in one keeps
    /// its kind, checked when it is on.
    pub fn update_notification(&self, mut n: Notification) -> ApiResult<Notification> {
        let cur = self.notification(n.id).map_err(|_| ApiError::NotFound(format!("notification with id {} not found", n.id)))?;
        if n.kind != cur.kind && (n.kind == "BUILTIN" || cur.kind == "BUILTIN") {
            return Err(ApiError::BadRequest(ERR_BUILTIN.into()));
        }
        keep_secrets(&mut n, &cur);
        if n.enabled {
            n.validate().map_err(ApiError::BadRequest)?;
        }
        n.created_at = cur.created_at;
        n.updated_at = now_text();
        self.db.execute(
            "UPDATE autobrr_notifications SET name=?, kind=?, enabled=?, body=? WHERE id=?",
            p![n.name, n.kind, n.enabled, body_of(&n), n.id],
        )?;
        Ok(n)
    }

    /// Removes a notification and the filters' picks of it (autobrr's `Delete`); never the built-in one.
    pub fn delete_notification(&self, id: i64) -> ApiResult<()> {
        let cur = self.notification(id).map_err(|_| ApiError::NotFound(format!("notification with id {id} not found")))?;
        if cur.kind == "BUILTIN" {
            return Err(ApiError::BadRequest(ERR_BUILTIN.into()));
        }
        self.db.execute("DELETE FROM autobrr_filter_notifications WHERE notification_id=?", p![id])?;
        self.db.execute("DELETE FROM autobrr_notifications WHERE id=?", p![id])?;
        Ok(())
    }

    /// The notifications that get `event` from filter `filter_id` (0: no filter): those the filter picked for it
    /// when the filter picked any, else those that want it (autobrr's `routingSnapshot.resolve`). Only those
    /// that can send.
    pub fn notifications_for(&self, event: &str, filter_id: i64) -> Vec<Notification> {
        let all: Vec<Notification> = match self.db.query("SELECT * FROM autobrr_notifications ORDER BY id", p![]) {
            Ok(rows) => rows.iter().map(notification_of).collect(),
            Err(_) => return Vec::new(),
        };
        let senders = |ids: &dyn Fn(&Notification) -> bool| all.iter().filter(|n| n.is_enabled() && ids(n)).cloned().collect::<Vec<_>>();
        if filter_id > 0 {
            let routes = self
                .db
                .query("SELECT notification_id, events FROM autobrr_filter_notifications WHERE filter_id=?", p![filter_id])
                .unwrap_or_default();
            if !routes.is_empty() {
                let wanted: Vec<i64> = routes
                    .iter()
                    .filter(|r| serde_json::from_str::<Vec<String>>(&r.s("events")).unwrap_or_default().iter().any(|e| e == event))
                    .map(|r| r.i("notification_id"))
                    .collect();
                return senders(&|n| wanted.contains(&n.id));
            }
        }
        senders(&|n| n.events.iter().any(|e| e == event))
    }

    /// Tells every notification that wants it, in the background, one after another, each within 30 seconds
    /// (autobrr's `Send`).
    pub fn notify(&self, p: Payload) {
        let list = self.notifications_for(&p.event, p.filter_id);
        if list.is_empty() {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            for n in list {
                let r = tokio::time::timeout(Duration::from_secs(30), me.send_notification(&n, &p)).await;
                match r {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::warn!("notification {}: could not send notification: {e}", n.name),
                    Err(_) => tracing::warn!("notification {}: could not send notification: context deadline exceeded", n.name),
                }
            }
        });
    }

    /// Sends `p` through `n` now (the built-in one writes to the inbox).
    pub async fn send_notification(&self, n: &Notification, p: &Payload) -> Result<(), String> {
        if n.kind == "BUILTIN" {
            return self.store_inbox_message(p).map(|_| ()).map_err(|e| e.message().to_string());
        }
        self.notifier.senders.send(n, p).await
    }

    /// Sends autobrr's test events that `n` wants, a second apart (autobrr's `Test`); the first problem.
    pub async fn test_notification(&self, mut n: Notification) -> ApiResult<()> {
        if n.id > 0 {
            let cur = self.notification(n.id)?;
            keep_secrets(&mut n, &cur);
        }
        n.validate().map_err(ApiError::BadRequest)?;
        if !TYPES.contains(&n.kind.as_str()) || n.kind == "SLACK" {
            return Err(ApiError::Internal("unsupported notification type".into()));
        }
        let mut first = true;
        for p in test_events() {
            if !n.events.contains(&p.event) {
                continue;
            }
            if !first {
                tokio::time::sleep(self.notifier.senders.test_gap).await;
            }
            first = false;
            self.send_notification(&n, &p).await.map_err(ApiError::Internal)?;
        }
        Ok(())
    }

    // ---- the built-in inbox (autobrr's NotificationInboxRepo) ----

    /// Writes `p` to the inbox and tells the live stream (autobrr's builtin sender and `StoreInboxMessage`).
    pub fn store_inbox_message(&self, p: &Payload) -> ApiResult<InboxMessage> {
        let mut m = InboxMessage {
            event: p.event.clone(),
            title: title(&p.event).into(),
            message: p.message.clone(),
            release_name: p.release_name.clone(),
            indexer: p.indexer.clone(),
            filter_name: p.filter.clone(),
            filter_id: p.filter_id,
            action: p.action.clone(),
            action_client: p.action_client.clone(),
            rejections: p.rejections.clone(),
            url: p.url.clone(),
            read_at: None,
            created_at: now_text(),
            ..InboxMessage::default()
        };
        if m.message == m.release_name {
            m.message.clear();
        }
        let null = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
        m.id = self.db.execute(
            "INSERT INTO autobrr_notification_inbox (event, title, message, release_name, indexer, filter_name, filter_id, action, action_client, rejections, url, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            p![
                m.event,
                m.title,
                m.message,
                null(&m.release_name),
                null(&m.indexer),
                null(&m.filter_name),
                if m.filter_id == 0 { Value::Null } else { json!(m.filter_id) },
                null(&m.action),
                null(&m.action_client),
                serde_json::to_string(&m.rejections).unwrap_or_default(),
                null(&m.url),
                m.created_at
            ],
        )?;
        let _ = self
            .notifier
            .inbox_events
            .send(super::irc::Event { event: "NOTIFICATION".into(), data: serde_json::to_string(&m).unwrap_or_default() });
        Ok(m)
    }

    /// Tells the live stream the inbox changed (autobrr's `publishInboxChanged`).
    fn inbox_changed(&self) {
        let _ = self.notifier.inbox_events.send(super::irc::Event { event: "INBOX_CHANGED".into(), data: "{}".into() });
    }

    /// One page of the inbox, newest first, with the counts (autobrr's inbox `Find`).
    pub fn find_inbox(&self, limit: i64, offset: i64, unread: bool, events: &[String]) -> ApiResult<Value> {
        let (ev_sql, mut args) = in_list("event", events);
        let stats = self.db.one(
            &format!("SELECT COUNT(*) AS total, COUNT(CASE WHEN read_at IS NULL THEN 1 END) AS unread FROM autobrr_notification_inbox WHERE {ev_sql}"),
            args.clone(),
        )?;
        let (all, unread_count) = stats.map(|r| (r.i("total"), r.i("unread"))).unwrap_or((0, 0));
        let read_sql = if unread { "read_at IS NULL" } else { "1=1" };
        args.push(json!(limit));
        args.push(json!(offset));
        let rows = self.db.query(
            &format!("SELECT * FROM autobrr_notification_inbox WHERE {read_sql} AND {ev_sql} ORDER BY id DESC LIMIT ? OFFSET ?"),
            args,
        )?;
        let data: Vec<InboxMessage> = rows
            .iter()
            .map(|r| InboxMessage {
                id: r.i("id"),
                event: r.s("event"),
                title: r.s("title"),
                message: r.s("message"),
                release_name: r.s("release_name"),
                indexer: r.s("indexer"),
                filter_name: r.s("filter_name"),
                filter_id: r.i("filter_id"),
                action: r.s("action"),
                action_client: r.s("action_client"),
                rejections: serde_json::from_str(&r.s("rejections")).unwrap_or_default(),
                url: r.s("url"),
                read_at: Some(r.s("read_at")).filter(|s| !s.is_empty()),
                created_at: r.s("created_at"),
            })
            .collect();
        Ok(json!({ "data": data, "count": if unread { unread_count } else { all }, "all_count": all, "unread_count": unread_count }))
    }

    /// Marks messages `ids` read, or every unread one when `ids` is empty (autobrr's `MarkRead`).
    pub fn mark_inbox_read(&self, ids: &[i64]) -> ApiResult<()> {
        let now = now_text();
        if ids.is_empty() {
            self.db.execute("UPDATE autobrr_notification_inbox SET read_at=? WHERE read_at IS NULL", p![now])?;
        } else {
            let (sql, mut args) = in_list("id", ids);
            args.insert(0, json!(now));
            self.db.execute(&format!("UPDATE autobrr_notification_inbox SET read_at=? WHERE read_at IS NULL AND {sql}"), args)?;
        }
        self.inbox_changed();
        Ok(())
    }

    /// Removes messages `ids` (autobrr's `Delete`).
    pub fn delete_inbox_messages(&self, ids: &[i64]) -> ApiResult<()> {
        let (sql, args) = in_list("id", ids);
        self.db.execute(&format!("DELETE FROM autobrr_notification_inbox WHERE {sql}"), args)?;
        self.inbox_changed();
        Ok(())
    }

    /// Empties the inbox (autobrr's `DeleteAll`); ids are never reused.
    pub fn delete_inbox(&self) -> ApiResult<()> {
        self.db.execute("DELETE FROM autobrr_notification_inbox", p![])?;
        self.inbox_changed();
        Ok(())
    }

    /// Removes messages older than 30 days and all but the newest 1000 (autobrr's `InboxCleanupJob`); how many.
    pub fn cleanup_inbox(&self) -> i64 {
        let before =
            self.db.one("SELECT COUNT(*) AS n FROM autobrr_notification_inbox", p![]).ok().flatten().map(|r| r.i("n")).unwrap_or(0);
        let cutoff = (Utc::now() - chrono::Duration::days(INBOX_MAX_AGE_DAYS)).to_rfc3339_opts(SecondsFormat::AutoSi, true);
        let _ = self.db.execute(
            "DELETE FROM autobrr_notification_inbox WHERE created_at < ? OR id NOT IN (SELECT id FROM autobrr_notification_inbox ORDER BY id DESC LIMIT ?)",
            p![cutoff, INBOX_MAX_MESSAGES],
        );
        let after = self.db.one("SELECT COUNT(*) AS n FROM autobrr_notification_inbox", p![]).ok().flatten().map(|r| r.i("n")).unwrap_or(0);
        let deleted = before - after;
        if deleted > 0 {
            self.inbox_changed();
        }
        deleted
    }

    /// Starts the hourly inbox cleanup, and runs it once now (autobrr's `startInboxCleanupJob`).
    pub fn start_notifications(&self) {
        let me = self.clone();
        let work: super::scheduler::JobFn = Arc::new(move || {
            let me = me.clone();
            Box::pin(async move {
                me.cleanup_inbox();
            })
        });
        self.scheduler.register("notification-inbox-cleanup", Box::new(Every(chrono::Duration::hours(1))), work);
        self.cleanup_inbox();
    }
}

/// Every interval from now (autobrr's `ScheduleJob` with a plain interval).
pub struct Every(pub chrono::Duration);

impl super::scheduler::Schedule for Every {
    fn next(&self, t: DateTime<Utc>) -> DateTime<Utc> {
        t + self.0
    }
}

/// `col IN (?, ?)` for `values`, and the values; `1=1` for none.
fn in_list<T: Serialize>(col: &str, values: &[T]) -> (String, Vec<Value>) {
    if values.is_empty() {
        return ("1=1".into(), Vec::new());
    }
    let marks = vec!["?"; values.len()].join(",");
    (format!("{col} IN ({marks})"), values.iter().map(|v| serde_json::to_value(v).unwrap_or_default()).collect())
}

/// Puts the saved token, API key and password back where `n` sent them redacted.
fn keep_secrets(n: &mut Notification, cur: &Notification) {
    use super::downloader::is_redacted;
    if is_redacted(&n.password) {
        n.password.clone_from(&cur.password);
    }
    if is_redacted(&n.token) {
        n.token.clone_from(&cur.token);
    }
    if is_redacted(&n.api_key) {
        n.api_key.clone_from(&cur.api_key);
    }
}

/// What an action's push of `r` tells (autobrr's `OnReleasePush` listener): `event` is `PUSH_APPROVED`,
/// `PUSH_REJECTED` or `PUSH_ERROR`; only the last two carry the status's rejections.
pub fn push_payload(event: &str, r: &Release, action: &super::action::Action, status: &super::history::ActionStatus) -> Payload {
    Payload {
        event: event.into(),
        release_name: r.torrent_name.clone(),
        filter: r.filter_name.clone(),
        filter_id: r.filter_id,
        indexer: r.indexer.name.clone(),
        info_hash: r.torrent_hash.clone(),
        size: r.size,
        status: event.into(),
        action: action.name.clone(),
        action_type: action.kind.clone(),
        action_client: action.client.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
        rejections: if event == "PUSH_APPROVED" { Vec::new() } else { status.rejections.clone() },
        protocol: r.protocol.clone(),
        implementation: r.implementation.clone(),
        timestamp: Some(Utc::now()),
        release: Some(Box::new(r.clone())),
        ..Payload::default()
    }
}

/// What a new release tells (autobrr's `OnReleaseNew` listener).
pub fn release_new_payload(r: &Release) -> Payload {
    Payload {
        event: "RELEASE_NEW".into(),
        release_name: r.torrent_name.clone(),
        indexer: r.indexer.name.clone(),
        info_hash: r.torrent_hash.clone(),
        size: r.size,
        protocol: r.protocol.clone(),
        implementation: r.implementation.clone(),
        timestamp: Some(Utc::now()),
        release: Some(Box::new(r.clone())),
        ..Payload::default()
    }
}

/// The IRC events autobrr sends (its `events.IRCEvent` types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrcEvent {
    /// The network dropped without being asked to.
    Disconnected,
    /// The network came back after dropping.
    Reconnected,
    /// The network was stopped after too many short connections.
    Flapping,
    /// The health check found problems.
    Unhealthy,
    /// The health check found the network well again.
    Healthy,
}

/// What an IRC event tells (autobrr's `OnIRC` listener): `network` is the network's name (or names), `message`
/// the event's own words.
pub fn irc_payload(kind: IrcEvent, network: &str, message: &str) -> Payload {
    let (event, subject, msg, irc_message) = match kind {
        IrcEvent::Reconnected => ("IRC_RECONNECTED", "IRC Reconnected", network, ""),
        IrcEvent::Disconnected => ("IRC_DISCONNECTED", "IRC Disconnected", network, ""),
        IrcEvent::Flapping => ("IRC_DISCONNECTED", "IRC Stopped", message, message),
        IrcEvent::Unhealthy => ("IRC_UNHEALTHY", "IRC Unhealthy", message, message),
        IrcEvent::Healthy => ("IRC_HEALTHY", "IRC Healthy", network, ""),
    };
    Payload {
        event: event.into(),
        subject: subject.into(),
        message: msg.into(),
        irc_network: network.into(),
        irc_message: irc_message.into(),
        timestamp: Some(Utc::now()),
        ..Payload::default()
    }
}

/// What a feed's refresh tells (autobrr's `OnFeedRefresh` listener): `error` is empty when it went well.
pub fn feed_payload(name: &str, id: i64, kind: &str, indexer: &str, error: &str) -> Payload {
    let base = Payload {
        feed: name.into(),
        feed_id: id,
        feed_type: kind.into(),
        indexer: indexer.into(),
        timestamp: Some(Utc::now()),
        ..Payload::default()
    };
    if error.is_empty() {
        Payload { event: "FEED_REFRESH_SUCCESS".into(), subject: "Feed Refresh Success".into(), message: format!("Feed: {name}"), ..base }
    } else {
        Payload {
            event: "FEED_REFRESH_ERROR".into(),
            subject: "Feed Refresh Error".into(),
            message: format!("Feed: {name}\nError: {error}"),
            feed_error: error.into(),
            ..base
        }
    }
}

/// What a list's refresh tells (autobrr's `OnListRefresh` listener): `error` is empty when it went well.
pub fn list_payload(name: &str, id: i64, kind: &str, error: &str, at: Option<DateTime<Utc>>) -> Payload {
    let base = Payload { list: name.into(), list_id: id, list_type: kind.into(), timestamp: at, ..Payload::default() };
    if error.is_empty() {
        Payload { event: "LIST_REFRESH_SUCCESS".into(), subject: "List Refresh Success".into(), message: format!("List: {name}"), ..base }
    } else {
        Payload {
            event: "LIST_REFRESH_ERROR".into(),
            subject: "List Refresh Error".into(),
            message: format!("List: {name}\nError: {error}"),
            list_error: error.into(),
            ..base
        }
    }
}

/// The sample release autobrr's test events carry.
fn sample_release(kind: super::rls::Type) -> Option<Box<Release>> {
    Some(Box::new(Release {
        r#type: kind,
        torrent_name: SAMPLE.into(),
        title: "Best Show Ever".into(),
        season: 18,
        episode: 21,
        year: 2026,
        resolution: "1080p".into(),
        source: "WEB-DL".into(),
        codec: vec!["H.264".into()],
        container: "mkv".into(),
        audio: vec!["DDP2.0".into()],
        group: "GROUP".into(),
        size: 1_500_000_000,
        ..Release::default()
    }))
}

/// The sample release name.
const SAMPLE: &str = "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP";

/// autobrr's test events, in its order.
pub fn test_events() -> Vec<Payload> {
    use super::rls::Type;
    let now = Some(Utc::now());
    let push = |event: &str, status: &str, action: &str, kind: &str, client: &str, rej: Vec<String>, t: Type| Payload {
        event: event.into(),
        subject: "New release!".into(),
        message: SAMPLE.into(),
        release_name: SAMPLE.into(),
        filter: "TV".into(),
        indexer: "MockIndexer".into(),
        status: status.into(),
        action: action.into(),
        action_type: kind.into(),
        action_client: client.into(),
        rejections: rej,
        protocol: "torrent".into(),
        implementation: "IRC".into(),
        timestamp: now,
        release: sample_release(t),
        ..Payload::default()
    };
    let simple = |event: &str, subject: &str, message: &str| Payload {
        event: event.into(),
        subject: subject.into(),
        message: message.into(),
        timestamp: now,
        ..Payload::default()
    };
    let irc = |event: &str, subject: &str, message: &str, irc_message: &str| Payload {
        irc_network: "P2P-Network".into(),
        irc_message: irc_message.into(),
        ..simple(event, subject, message)
    };
    vec![
        simple("TEST", "Test Notification", "autobrr goes brr!!"),
        push("PUSH_APPROVED", "PUSH_APPROVED", "Send to qBittorrent", "QBITTORRENT", "qBittorrent", Vec::new(), Type::Episode),
        push("PUSH_REJECTED", "PUSH_REJECTED", "Send to Sonarr", "SONARR", "Sonarr", vec!["Unknown Series".into()], Type::Unknown),
        push("PUSH_ERROR", "PUSH_ERROR", "Send to Sonarr", "SONARR", "Sonarr", vec!["error pushing to client".into()], Type::Unknown),
        irc("IRC_DISCONNECTED", "IRC Disconnected unexpectedly", "Network: P2P-Network", ""),
        irc("IRC_RECONNECTED", "IRC Reconnected", "Network: P2P-Network", ""),
        irc(
            "IRC_UNHEALTHY",
            "IRC Unhealthy",
            "P2P-Network: authentication failed: account does not exist",
            "P2P-Network: authentication failed: account does not exist",
        ),
        irc("IRC_HEALTHY", "IRC Healthy", "P2P-Network", ""),
        Payload {
            list: "Sonarr TV".into(),
            list_id: 1,
            list_type: "SONARR".into(),
            ..simple("LIST_REFRESH_SUCCESS", "List Refresh Success", "List: Sonarr TV")
        },
        Payload {
            list: "Sonarr TV".into(),
            list_id: 1,
            list_type: "SONARR".into(),
            list_error: "client sonarr Sonarr not enabled".into(),
            ..simple("LIST_REFRESH_ERROR", "List Refresh Error", "List: Sonarr TV\nError: client sonarr Sonarr not enabled")
        },
        Payload {
            feed: "Mock Indexer".into(),
            feed_id: 1,
            feed_type: "TORZNAB".into(),
            indexer: "Mock Indexer".into(),
            ..simple("FEED_REFRESH_SUCCESS", "Feed Refresh Success", "Feed: Mock Indexer")
        },
        Payload {
            feed: "Mock Indexer".into(),
            feed_id: 1,
            feed_type: "TORZNAB".into(),
            feed_error: "error fetching feed items: unexpected status code: 503".into(),
            indexer: "Mock Indexer".into(),
            ..simple(
                "FEED_REFRESH_ERROR",
                "Feed Refresh Error",
                "Feed: Mock Indexer\nError: error fetching feed items: unexpected status code: 503",
            )
        },
        Payload {
            current_version: "v1.84.0".into(),
            new_version: "v1.85.0".into(),
            url: "https://github.com/autobrr/autobrr/releases/tag/v1.85.0".into(),
            ..simple("APP_UPDATE_AVAILABLE", "New update available!", "v1.85.0")
        },
        Payload {
            event: "RELEASE_NEW".into(),
            subject: "New release received!".into(),
            message: SAMPLE.into(),
            release_name: SAMPLE.into(),
            filter: "TV".into(),
            indexer: "MockIndexer".into(),
            protocol: "torrent".into(),
            implementation: "IRC".into(),
            timestamp: now,
            release: sample_release(Type::Unknown),
            ..Payload::default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// autobrr's message_builder_test.go (`TestNotificationBuilderPlainText_BuildBody`).
    #[test]
    fn plain_text_body() {
        let p = Payload {
            subject: "New release!".into(),
            message: "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP".into(),
            event: "PUSH_APPROVED".into(),
            release_name: "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP".into(),
            filter: "TV".into(),
            indexer: "MockIndexer".into(),
            status: "PUSH_APPROVED".into(),
            action: "Send to qBittorrent".into(),
            action_type: "QBITTORRENT".into(),
            action_client: "qBittorrent".into(),
            rejections: vec!["Unknown Series".into()],
            protocol: "torrent".into(),
            implementation: "IRC".into(),
            size: 1_500_000_000,
            ..Payload::default()
        };
        assert_eq!(
            plain_body(&p),
            "New release!\nBest.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP\nNew release: Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP\nSize: 1.5 GB\nStatus: Approved\nIndexer: MockIndexer\nFilter: TV\nAction: QBITTORRENT: Send to qBittorrent\nClient: qBittorrent\nRejections: Unknown Series\n"
        );
        let empty = Payload::default();
        assert_eq!(plain_body(&empty), "");
        let html = html_body(&Payload { sender: "me & <you>".into(), ..p });
        assert!(html.starts_with("<b>me &amp; &lt;you&gt;</b>\n<b>New release!</b> Best"), "{html}");
    }

    /// autobrr's domain/notification_test.go (`TestNotification_Validate`).
    #[test]
    fn validate() {
        let n = |kind: &str| Notification { kind: kind.into(), ..Notification::default() };
        assert_eq!(n("DISCORD").validate().unwrap_err(), "missing webhook url: invalid notification settings");
        assert_eq!(n("WEBHOOK").validate().unwrap_err(), "missing webhook url: invalid notification settings");
        assert_eq!(n("GOTIFY").validate().unwrap_err(), "missing gotify url: invalid notification settings");
        assert_eq!(n("NOTIFIARR").validate().unwrap_err(), "missing notifiarr api key: invalid notification settings");
        let bad_key = Notification { api_key: "ABC".into(), ..n("NOTIFIARR") };
        assert_eq!(
            bad_key.validate().unwrap_err(),
            "notifiarr api key must be 36 characters of a-z, 0-9 and dashes: invalid notification settings"
        );
        let good_key = Notification { api_key: "0b6c2a3e-9f1d-4c8a-b7e2-5d4f3a2b1c0d".into(), ..n("NOTIFIARR") };
        assert!(good_key.validate().is_ok());
        assert_eq!(n("PUSHOVER").validate().unwrap_err(), "missing pushover api token: invalid notification settings");
        assert_eq!(n("TELEGRAM").validate().unwrap_err(), "missing telegram bot token: invalid notification settings");
        assert_eq!(n("NTFY").validate().unwrap_err(), "missing url: invalid notification settings");
        let bark = Notification { host: "https://api.day.app?x=1".into(), token: "k".into(), ..n("BARK") };
        assert_eq!(
            bark.validate().unwrap_err(),
            "bark server url must be an http or https url without query parameters: invalid notification settings"
        );
        assert!(n("BUILTIN").validate().is_ok());
        assert_eq!(n("SLACK").validate().unwrap_err(), "unsupported notification type: 'SLACK': invalid notification settings");
        assert!(!Notification { enabled: true, ..n("DISCORD") }.is_enabled());
        assert!(Notification { enabled: true, webhook: "x".into(), ..n("DISCORD") }.is_enabled());
        assert!(Notification { enabled: true, ..n("BUILTIN") }.is_enabled());
    }

    #[test]
    fn titles() {
        assert_eq!(title("PUSH_ERROR"), "Push Error");
        assert_eq!(title("NOPE"), "New Event");
        assert_eq!(test_events().len(), 14);
    }
}
