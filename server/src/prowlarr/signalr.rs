// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.SignalR/MessageHub.cs, SignalRMessage.cs,
// src/Prowlarr.Http/REST/RestControllerWithSignalR.cs, src/Prowlarr.Http/ResourceChangeMessage.cs,
// src/NzbDrone.Host/Startup.cs (MapHub "/signalr/messages")), GPL-3.0.

//! The live updates Prowlarr's web app listens to (`/signalr/messages`): ASP.NET SignalR's JSON hub protocol over
//! a WebSocket. A client first asks `POST /signalr/messages/negotiate`, then opens the WebSocket, sends the
//! handshake (`{"protocol":"json","version":1}` and the record separator `\x1e`) and gets `{}` back, then Lantern's
//! version (`{"name":"version"}`), then a `receiveMessage` call each time a resource changes:
//!
//! ```text
//! {"type":1,"target":"receiveMessage","arguments":[{"name":"indexer","body":{"action":"updated","resource":{…}}}]}
//! ```
//!
//! Pings (`{"type":6}`) keep the connection open every 15 seconds, as ASP.NET does. The API key is taken as
//! `access_token` (how the web app sends it) or as for every other `/api/v1` route.

use super::V1;
use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::broadcast;

/// The end of every SignalR JSON message.
const SEPARATOR: char = '\u{1e}';

/// The SignalR routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/signalr/messages/negotiate", post(negotiate)).route("/signalr/messages", get(connect))
}

/// The message channel of each running Lantern (by its data folder; tests run several in one process).
fn channels() -> &'static Mutex<HashMap<std::path::PathBuf, broadcast::Sender<String>>> {
    static CHANNELS: OnceLock<Mutex<HashMap<std::path::PathBuf, broadcast::Sender<String>>>> = OnceLock::new();
    CHANNELS.get_or_init(Default::default)
}

/// The channel of `st`, made when first needed.
fn channel(st: &AppState) -> broadcast::Sender<String> {
    channels().lock().unwrap_or_else(|e| e.into_inner()).entry(st.data_dir.clone()).or_insert_with(|| broadcast::channel(256).0).clone()
}

/// A hub call of `receiveMessage` with one `SignalRMessage` (`name`, `body`), as written on the wire.
pub fn frame(name: &str, body: Value) -> String {
    let call = json!({ "type": 1, "target": "receiveMessage", "arguments": [{ "name": name, "body": body }] });
    format!("{call}{SEPARATOR}")
}

/// Sends `SignalRMessage` `name` with `body` to every connected web app (nothing happens when none is connected,
/// as in Prowlarr).
pub fn send(st: &AppState, name: &str, body: Value) {
    let tx = channel(st);
    if tx.receiver_count() > 0 {
        let _ = tx.send(frame(name, body));
    }
}

/// Prowlarr's `BroadcastResourceChange`: resource `name` (`indexer`, `tag`, `command` …) was `created`, `updated`,
/// `deleted` (the resource is then only its id) or needs a `sync` (no resource).
pub fn broadcast(st: &AppState, name: &str, action: &str, resource: Option<&Value>) {
    let mut body = json!({ "action": action });
    if let Some(r) = resource {
        body["resource"] = r.clone();
    }
    send(st, name, body);
}

/// Whether the request's `access_token` is Lantern's API key.
fn token_ok(st: &AppState, q: &HashMap<String, String>) -> bool {
    let want = st.settings.str("api_key");
    q.get("access_token").is_some_and(|t| !want.is_empty() && !t.is_empty() && crate::auth::ct_eq(t, &want))
}

/// `POST /signalr/messages/negotiate[?negotiateVersion=1]`: a connection id and the one transport Lantern offers
/// (WebSockets, text and binary frames).
async fn negotiate(State(st): State<AppState>, v1: Result<V1, super::ApiError>, Query(q): Query<HashMap<String, String>>) -> Response {
    if !token_ok(&st, &q) {
        if let Err(e) = v1 {
            return e.into_response();
        }
    }
    let id = {
        use rand::Rng;
        let mut b = [0u8; 16];
        rand::rng().fill(&mut b[..]);
        hex::encode(b)
    };
    let version = q.get("negotiateVersion").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).min(1);
    let mut out = json!({
        "connectionId": id,
        "availableTransports": [{ "transport": "WebSockets", "transferFormats": ["Text", "Binary"] }],
    });
    if version >= 1 {
        out["negotiateVersion"] = json!(1);
        out["connectionToken"] = json!(id);
    }
    Json(out).into_response()
}

/// `GET /signalr/messages?id=&access_token=` with a WebSocket upgrade: the hub connection.
async fn connect(
    State(st): State<AppState>,
    v1: Result<V1, super::ApiError>,
    Query(q): Query<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    if !token_ok(&st, &q) {
        if let Err(e) = v1 {
            return e.into_response();
        }
    }
    let Ok(ws) = ws else {
        return (StatusCode::BAD_REQUEST, "Connect with a WebSocket: Lantern's live updates only come that way.").into_response();
    };
    let rx = channel(&st).subscribe();
    ws.on_upgrade(move |socket| hub(socket, rx))
}

/// Reads one text message of the client (`None` when it closed).
async fn next_text(socket: &mut WebSocket) -> Option<String> {
    loop {
        match socket.recv().await? {
            Ok(Message::Text(t)) => return Some(t.to_string()),
            Ok(Message::Binary(b)) => return Some(String::from_utf8_lossy(&b).into_owned()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

/// One hub connection: the handshake, Lantern's version, then every message until the client goes.
async fn hub(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    // the handshake: {"protocol":"json","version":1}␞ → {}␞
    let Some(hello) = next_text(&mut socket).await else { return };
    let request: Value = serde_json::from_str(hello.trim_end_matches(SEPARATOR)).unwrap_or(Value::Null);
    if request["protocol"] != "json" {
        let _ =
            socket.send(Message::Text(format!("{}{SEPARATOR}", json!({ "error": "Lantern only speaks the json protocol." })).into())).await;
        return;
    }
    if socket.send(Message::Text(format!("{{}}{SEPARATOR}").into())).await.is_err() {
        return;
    }
    // MessageHub.OnConnectedAsync
    let version = frame("version", json!({ "version": crate::VERSION }));
    if socket.send(Message::Text(version.into())).await.is_err() {
        return;
    }
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(15));
    ping.tick().await;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = next_text(&mut socket) => match incoming {
                // a close message (type 7) ends it; pings and anything else need nothing back
                Some(t) if t.split(SEPARATOR).any(|m| serde_json::from_str::<Value>(m).is_ok_and(|v| v["type"] == 7)) => return,
                Some(_) => {}
                None => return,
            },
            _ = ping.tick() => {
                if socket.send(Message::Text(format!("{}{SEPARATOR}", json!({ "type": 6 })).into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_hub_invocations() {
        let f = frame("indexer", json!({ "action": "deleted", "resource": { "id": 3 } }));
        assert!(f.ends_with('\u{1e}'));
        let v: Value = serde_json::from_str(f.trim_end_matches('\u{1e}')).unwrap();
        assert_eq!(
            v,
            json!({ "type": 1, "target": "receiveMessage", "arguments": [{ "name": "indexer", "body": { "action": "deleted", "resource": { "id": 3 } } }] })
        );
    }
}
