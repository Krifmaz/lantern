//! What happens to grabs, told to apps two ways: the live event stream (`GET /api/hub/events`, server-sent
//! events) and a webhook — a POST of the same JSON to the app's callback address, signed with Lantern's API key
//! (`X-Lantern-Signature: sha256=<HMAC-SHA256 of the body>`) and tried three times.
//!
//! The events: `grab.queued`, `grab.sent`, `grab.progress` (stream only), `grab.retrying`, `grab.completed`,
//! `grab.failed`, `grab.removed`.

use super::apps::App;
use super::Hub;
use kit_server_core::{p, Db};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::broadcast;

/// One event.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// What happened (`grab.completed`).
    pub event: String,
    /// When, in seconds since 1970.
    pub at: f64,
    /// The app it is for.
    pub app_id: i64,
    /// The grab as the app sees it.
    pub grab: Value,
}

/// The live events: every listener gets every event and keeps those it wants.
#[derive(Clone)]
pub struct Events {
    /// The channel the events go out on.
    tx: broadcast::Sender<Event>,
}

impl Default for Events {
    fn default() -> Self {
        Events::new()
    }
}

impl Events {
    /// A channel keeping the last 512 events for slow listeners.
    pub fn new() -> Events {
        Events { tx: broadcast::channel(512).0 }
    }

    /// A new listener.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Sends `e` to every listener (none listening is fine).
    pub fn send(&self, e: Event) {
        let _ = self.tx.send(e);
    }
}

/// The signature header's value for `body` with `key`.
pub fn signature(key: &str, body: &[u8]) -> String {
    use hmac::Mac;
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).expect("any key length works");
    m.update(body);
    format!("sha256={}", hex::encode(m.finalize().into_bytes()))
}

impl Hub {
    /// Tells `app` that `event` happened to `grab`: on the stream, and by webhook unless it is progress.
    pub fn emit(&self, event: &str, app: &App, grab: Value) {
        let e = Event { event: event.to_string(), at: crate::auth::now(), app_id: app.id, grab };
        self.events.send(e.clone());
        if event != "grab.progress" && !app.callback_url.is_empty() {
            let hub = self.clone();
            let (url, id) = (app.callback_url.clone(), app.id);
            tokio::spawn(async move { hub.deliver(id, &url, &e).await });
        }
    }

    /// Posts `e` to `url`, trying three times (after 1 and 5 seconds); the outcome is kept on app `app_id`.
    async fn deliver(&self, app_id: i64, url: &str, e: &Event) {
        let body = serde_json::to_vec(e).unwrap_or_default();
        let sig = signature(&self.settings.str("api_key"), &body);
        let mut problem = String::new();
        for (n, wait) in [0u64, 1, 5].into_iter().enumerate() {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            let r = self
                .http
                .post(url)
                .header("content-type", "application/json")
                .header("x-lantern-event", &e.event)
                .header("x-lantern-signature", &sig)
                .timeout(Duration::from_secs(15))
                .body(body.clone())
                .send()
                .await;
            match r {
                Ok(r) if r.status().is_success() => {
                    problem.clear();
                    break;
                }
                Ok(r) => {
                    problem = format!("The app's callback address {url} answered {} to {} (try {}).", r.status().as_u16(), e.event, n + 1)
                }
                Err(err) => problem = format!("Lantern couldn't reach the app's callback address {url}: {err}"),
            }
        }
        if !problem.is_empty() {
            tracing::warn!("{problem}");
        }
        let _ = self.db.execute("UPDATE hub_apps SET callback_error=? WHERE id=?", p![problem, app_id]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn signs_bodies() {
        let s = super::signature("key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(s, "sha256=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");
    }
}
