// Ported from autobrr v1.90.0 (internal/notification/service_test.go TestRoutingSnapshotResolve,
// TestServiceSend, TestServiceBuiltinNotification, TestServiceValidation; internal/database/notification_test.go
// FilterNotificationLifecycle; internal/database/notification_inbox_test.go), GPL-2.0-or-later, with API shape
// checks against autobrr's Go structs (tests/fixtures/upstream/autobrr/api/structs.json).
//! autobrr's notifications under `/autobrr/api`: the routes save, change, test and remove them, the built-in one
//! writes to the inbox, a filter's own picks replace the global events, and the pipeline, lists, feeds and IRC
//! tell the notifications that want it. Every sender here is a local stand-in; nothing reaches the internet.

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::Router;
use kit_server_core::{p, Db};
use serde_json::{json, Value};
use server::releases::notify::{feed_payload, irc_payload, list_payload, IrcEvent, Payload};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

const KEY: &str = "test-key";

/// Lantern with an empty data folder.
async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-abnotify-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let announce = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/announce").display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some(KEY.to_string()),
        "APP_ANNOUNCE_DIR" => Some(announce.clone()),
        "APP_DEFINITIONS_DIR" => Some("/nonexistent".to_string()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    (server::app(st.clone()), st)
}

/// Calls `path` with a raw `body`: the status and the JSON (null when there is none).
async fn raw(app: &Router, method: &str, path: &str, body: Option<&str>) -> (StatusCode, Value) {
    let b = axum::http::Request::builder().method(method).uri(path).header("x-api-key", KEY);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let bytes = to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// Calls autobrr's `path` (under `/autobrr/api`).
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = body.map(|v| v.to_string());
    raw(app, method, &format!("/autobrr/api{path}"), b.as_deref()).await
}

/// `v` has every key autobrr's Go struct `name` always sends, and none it can't.
fn assert_shape(name: &str, v: &Value) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/api/structs.json");
    let all: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let s = &all["structs"][name];
    let list = |k: &str| s[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
    let (always, maybe) = (list("always"), list("omitempty"));
    let o = v.as_object().unwrap_or_else(|| panic!("{name} is not an object: {v}"));
    for k in &always {
        assert!(o.contains_key(k), "{name} lacks {k}: {v}");
    }
    for k in o.keys() {
        assert!(always.contains(k) || maybe.contains(k), "{name} has {k}, which autobrr's hasn't: {v}");
    }
}

/// autobrr's error answer.
fn assert_error(v: &Value, code: &str, message: &str) {
    assert_eq!((v["code"].as_str(), v["message"].as_str()), (Some(code), Some(message)), "{v}");
}

/// What a webhook stand-in got: each request's `X-Autobrr-Event` header and JSON body.
type Got = Arc<Mutex<Vec<(String, Value)>>>;

/// A local webhook answering 200 to everything; its address and what it got.
async fn webhook() -> (String, Got) {
    let got: Got = Arc::default();
    let g = got.clone();
    let app = Router::new().fallback(move |req: Request| {
        let g = g.clone();
        async move {
            let event = req.headers().get("X-Autobrr-Event").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            let body = to_bytes(req.into_body(), 1 << 22).await.unwrap_or_default();
            g.lock().unwrap().push((event, serde_json::from_slice(&body).unwrap_or(Value::Null)));
            StatusCode::OK
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}/hook"), got)
}

/// Waits up to two seconds for `ok`.
async fn eventually(ok: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        if ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    ok()
}

/// A webhook notification saved through the routes; its id.
async fn store_webhook(app: &Router, name: &str, url: &str, events: &[&str]) -> i64 {
    let (s, v) = call(app, "POST", "/notification", Some(json!({ "name": name, "type": "WEBHOOK", "enabled": true, "webhook": url, "events": events }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_i64().unwrap()
}

/// A filter with a TEST action; its id.
async fn test_filter(app: &Router, name: &str) -> i64 {
    let (s, f) = call(app, "POST", "/filters", Some(json!({ "name": name, "enabled": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    let mut full = f.clone();
    full["actions"] = json!([{ "name": "test it", "type": "TEST", "enabled": true }]);
    let (s, v) = call(app, "PUT", &format!("/filters/{}", f["id"]), Some(full)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    f["id"].as_i64().unwrap()
}

/// The names of the notifications that get `event` from filter `filter_id`, sorted.
fn routed(st: &server::AppState, event: &str, filter_id: i64) -> Vec<String> {
    let mut names: Vec<String> = st.releases.notifications_for(event, filter_id).into_iter().map(|n| n.name).collect();
    names.sort();
    names
}

/// http/notification.go and service.go `Store`/`Update`/`Delete`/`Validate`: the built-in one is seeded and can't be
/// added, removed or retyped; one that is on is checked; redacted secrets keep the saved ones.
#[tokio::test]
async fn the_routes_save_change_and_remove_notifications() {
    let (app, st) = lantern("crud").await;
    let (s, list) = call(&app, "GET", "/notification", None).await;
    assert_eq!(s, StatusCode::OK);
    let list = list.as_array().unwrap().clone();
    assert_eq!(list.len(), 1, "the migrations seed the built-in one");
    let builtin = &list[0];
    assert_shape("Notification", builtin);
    assert_eq!((builtin["name"].as_str(), builtin["type"].as_str(), builtin["enabled"].as_bool()), (Some("Built-in"), Some("BUILTIN"), Some(true)));
    assert_eq!(builtin["events"], json!(["PUSH_ERROR", "IRC_DISCONNECTED", "APP_UPDATE_AVAILABLE"]));
    let bid = builtin["id"].as_i64().unwrap();

    // can not be stored, deleted or change type
    let (s, v) = call(&app, "POST", "/notification", Some(json!({ "name": "Another", "type": "BUILTIN", "enabled": true }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "built-in notification can not be created, deleted or change type");
    let (s, v) = call(&app, "DELETE", &format!("/notification/{bid}"), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "built-in notification can not be created, deleted or change type");
    let mut changed = builtin.clone();
    changed["type"] = json!("WEBHOOK");
    changed["webhook"] = json!("http://127.0.0.1:9/hook");
    let (s, v) = call(&app, "PUT", &format!("/notification/{bid}"), Some(changed)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let mut events = builtin.clone();
    events["events"] = json!(["PUSH_REJECTED"]);
    let (s, v) = call(&app, "PUT", &format!("/notification/{bid}"), Some(events)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["events"], json!(["PUSH_REJECTED"]));

    // TestServiceValidation: one that is on is checked, one that is off is a draft
    let (s, v) = call(&app, "POST", "/notification", Some(json!({ "name": "notifiarr", "type": "NOTIFIARR", "enabled": true }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "missing notifiarr api key: invalid notification settings");
    let (s, v) = call(&app, "POST", "/notification/test", Some(json!({ "name": "notifiarr", "type": "NOTIFIARR", "enabled": true }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "missing notifiarr api key: invalid notification settings");
    let (s, draft) = call(&app, "POST", "/notification", Some(json!({ "name": "notifiarr", "type": "NOTIFIARR", "enabled": false }))).await;
    assert_eq!(s, StatusCode::CREATED, "disabled notifications are saved as drafts: {draft}");
    let id = draft["id"].as_i64().unwrap();
    let mut on = draft.clone();
    on["enabled"] = json!(true);
    let (s, _) = call(&app, "PUT", &format!("/notification/{id}"), Some(on.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    on["api_key"] = json!("0b6c2a3e-9f1d-4c8a-b7e2-5d4f3a2b1c0d");
    let (s, v) = call(&app, "PUT", &format!("/notification/{id}"), Some(on)).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // the secret comes back redacted, and a redacted one sent back keeps the saved one
    let (s, one) = call(&app, "GET", &format!("/notification/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("Notification", &one);
    assert_ne!(one["api_key"], "0b6c2a3e-9f1d-4c8a-b7e2-5d4f3a2b1c0d", "the key is redacted: {one}");
    let mut renamed = one.clone();
    renamed["name"] = json!("Notifiarr");
    let (s, _) = call(&app, "PUT", &format!("/notification/{id}"), Some(renamed)).await;
    assert_eq!(s, StatusCode::OK);
    let saved = st.releases.notification(id).unwrap();
    assert_eq!((saved.name.as_str(), saved.api_key.as_str()), ("Notifiarr", "0b6c2a3e-9f1d-4c8a-b7e2-5d4f3a2b1c0d"));

    // a webhook can't become the built-in one
    let (s, hook) = call(&app, "POST", "/notification", Some(json!({ "name": "webhook", "type": "WEBHOOK", "enabled": true, "webhook": "http://127.0.0.1:9/hook" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");
    let mut retyped = hook.clone();
    retyped["type"] = json!("BUILTIN");
    let (s, _) = call(&app, "PUT", &format!("/notification/{}", hook["id"]), Some(retyped)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = call(&app, "POST", "/notification", Some(json!({ "name": "w", "type": "WEBHOOK", "enabled": true }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "missing webhook url: invalid notification settings");

    // not found and bad ids
    let (s, v) = call(&app, "GET", "/notification/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    let (s, v) = call(&app, "DELETE", "/notification/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "notification with id 999 not found");
    let (s, _) = call(&app, "GET", "/notification/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, _) = call(&app, "DELETE", &format!("/notification/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, "GET", "/notification", None).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Built-in", "webhook"], "by name");

    // pushover sounds need a token
    let (s, v) = call(&app, "GET", "/notification/pushover/sounds", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "token parameter is required");
}

/// service_test.go `TestRoutingSnapshotResolve` and notification_test.go `FilterNotificationLifecycle`: a filter
/// that picks its own events replaces the global ones, even an empty pick or a pick of one that can't send.
#[tokio::test]
async fn a_filter_s_own_picks_replace_the_global_events() {
    let (app, st) = lantern("routing").await;
    let one = store_webhook(&app, "one", "http://127.0.0.1:9/one", &["PUSH_APPROVED"]).await;
    store_webhook(&app, "two", "http://127.0.0.1:9/two", &["PUSH_REJECTED"]).await;
    let (s, off) = call(&app, "POST", "/notification", Some(json!({ "name": "three", "type": "WEBHOOK", "enabled": false, "events": ["PUSH_APPROVED"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{off}");
    let three = off["id"].as_i64().unwrap();

    let (f9, f10, f11, f12) = (test_filter(&app, "nine").await, test_filter(&app, "ten").await, test_filter(&app, "eleven").await, test_filter(&app, "twelve").await);
    for (fid, routes) in [
        (f10, json!([{ "notification_id": one, "events": ["PUSH_REJECTED"] }])),
        (f11, json!([{ "notification_id": one, "events": [] }])),
        (f12, json!([{ "notification_id": three, "events": ["PUSH_APPROVED"] }])),
    ] {
        let (s, v) = call(&app, "PUT", &format!("/filters/{fid}/notifications"), Some(routes)).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    }
    let (s, v) = call(&app, "PUT", &format!("/filters/{f9}/notifications"), Some(json!([{ "notification_id": 999, "events": [] }]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "notification with id 999 does not exist: notification not found");

    let none: Vec<String> = Vec::new();
    assert_eq!(routed(&st, "PUSH_APPROVED", 0), vec!["one"], "global event without filter");
    assert_eq!(routed(&st, "PUSH_APPROVED", f9), vec!["one"], "filter without override inherits global");
    assert_eq!(routed(&st, "PUSH_REJECTED", f10), vec!["one"], "custom event replaces global");
    assert_eq!(routed(&st, "PUSH_APPROVED", f10), none, "custom route suppresses global event");
    assert_eq!(routed(&st, "PUSH_APPROVED", f11), none, "empty route mutes filter");
    assert_eq!(routed(&st, "PUSH_APPROVED", f12), none, "disabled or missing sender does not fall back");
    assert_eq!(routed(&st, "RELEASE_NEW", 0), none, "TestServiceSend: does not send without a matching route");

    // the notification lists the filters that picked it, and removing it removes their picks
    let (_, list) = call(&app, "GET", "/notification", None).await;
    let used = list.as_array().unwrap().iter().find(|n| n["id"] == one).unwrap()["used_by_filters"].clone();
    assert_eq!(used.as_array().map(Vec::len), Some(2), "{used}");
    let (_, picks) = call(&app, "GET", &format!("/filters/{f10}/notifications"), None).await;
    assert_eq!(picks[0]["notification_id"], one, "{picks}");
    let (s, _) = call(&app, "DELETE", &format!("/notification/{one}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, picks) = call(&app, "GET", &format!("/filters/{f10}/notifications"), None).await;
    assert_eq!(picks, json!([]));
}

/// service.go `Test`: the test events a notification wants reach it.
#[tokio::test]
async fn a_test_reaches_the_webhook() {
    let (app, _) = lantern("test").await;
    let (url, got) = webhook().await;
    let body = json!({ "name": "hook", "type": "WEBHOOK", "enabled": true, "webhook": url, "events": ["PUSH_APPROVED"] });
    let (s, v) = call(&app, "POST", "/notification/test", Some(body)).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    let got = got.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0].0.as_str(), got[0].1["event"].as_str()), ("action.approved", Some("action.approved")), "autobrr's webhook event names");
    assert_eq!(got[0].1["data"]["release"]["name"], "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP", "{}", got[0].1);

    // a webhook that can't be reached is the test's error
    let body = json!({ "name": "hook", "type": "WEBHOOK", "enabled": true, "webhook": "http://127.0.0.1:9/hook", "events": ["PUSH_APPROVED"] });
    let (s, v) = call(&app, "POST", "/notification/test", Some(body)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{v}");
}

/// service_test.go `TestServiceBuiltinNotification` "send stores and publishes inbox message": the built-in one
/// writes the push error to the inbox, without a message that only repeats the release's name, and tells the
/// live stream.
#[tokio::test]
async fn the_built_in_one_writes_to_the_inbox() {
    let (app, st) = lantern("builtin").await;
    let mut live = st.releases.notifier.inbox_events.subscribe();
    let builtin = st.releases.notifications_for("PUSH_ERROR", 0);
    assert_eq!(builtin.len(), 1);
    let name = "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP";
    let p = Payload {
        event: "PUSH_ERROR".into(),
        message: name.into(),
        release_name: name.into(),
        filter: "TV".into(),
        filter_id: 4,
        action_client: "Sonarr".into(),
        rejections: vec!["error pushing to client".into()],
        ..Payload::default()
    };
    st.releases.send_notification(&builtin[0], &p).await.unwrap();
    let (s, inbox) = call(&app, "GET", "/notification/inbox", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("FindInboxResponse", &inbox);
    let m = &inbox["data"][0];
    assert_shape("InboxMessage", m);
    assert_eq!(m["title"], "Push Error");
    assert!(m.get("message").is_none_or(|v| v == ""), "{m}");
    assert_eq!((m["release_name"].as_str(), m["filter_name"].as_str(), m["filter_id"].as_i64()), (Some(name), Some("TV"), Some(4)));
    assert_eq!(m["action_client"], "Sonarr");
    assert_eq!(m["rejections"], json!(["error pushing to client"]));
    assert_eq!(live.try_recv().unwrap().event, "NOTIFICATION");

    // "inbox changes publish change event"
    for (method, path, body) in [("POST", "/notification/inbox/read", Some(json!({ "ids": [1] }))), ("POST", "/notification/inbox/delete", Some(json!({ "ids": [1] }))), ("DELETE", "/notification/inbox", None)] {
        let (s, v) = call(&app, method, path, body).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{path}: {v}");
        assert_eq!(live.try_recv().unwrap().event, "INBOX_CHANGED", "{path}");
    }
}

/// notification_inbox_test.go `TestNotificationInboxRepo_Lifecycle` and `IDsNotReusedAfterDeleteAll`, through the
/// routes.
#[tokio::test]
async fn the_inbox_lifecycle() {
    let (app, st) = lantern("inbox").await;
    let name = "Best.Show.Ever.S18E21.1080p.AMZN.WEB-DL.DDP2.0.H.264-GROUP";
    let store = |event: &str, message: &str, rest: Payload| {
        st.releases.store_inbox_message(&Payload { event: event.into(), message: message.into(), ..rest }).unwrap().id
    };
    let old = store("PUSH_ERROR", "old", Payload::default());
    let middle = store("IRC_DISCONNECTED", "middle", Payload::default());
    let new = store(
        "PUSH_ERROR",
        "new",
        Payload { release_name: name.into(), filter: "TV".into(), filter_id: 4, rejections: vec!["error pushing to client".into()], ..Payload::default() },
    );
    assert!(old > 0 && middle > old && new > middle);

    let (_, r) = call(&app, "GET", "/notification/inbox?limit=2", None).await;
    assert_eq!((r["count"].as_i64(), r["all_count"].as_i64(), r["unread_count"].as_i64()), (Some(3), Some(3), Some(3)), "{r}");
    assert_eq!(r["data"].as_array().unwrap().len(), 2);
    assert_eq!((r["data"][0]["message"].as_str(), r["data"][0]["release_name"].as_str()), (Some("new"), Some(name)));
    assert_eq!((r["data"][0]["filter_name"].as_str(), r["data"][0]["filter_id"].as_i64()), (Some("TV"), Some(4)));
    assert_eq!(r["data"][0]["rejections"], json!(["error pushing to client"]));
    assert_eq!(r["data"][1]["rejections"], json!([]));
    assert!(r["data"][0].get("read_at").is_none_or(Value::is_null));

    let (_, r) = call(&app, "GET", "/notification/inbox?limit=10&event=PUSH_ERROR", None).await;
    assert_eq!(r["all_count"], 2);
    assert_eq!(r["data"][1]["event"], "PUSH_ERROR");

    for _ in 0..2 {
        let (s, _) = call(&app, "POST", "/notification/inbox/read", Some(json!({ "ids": [new] }))).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }
    let (_, r) = call(&app, "GET", "/notification/inbox?limit=10&unread=true", None).await;
    assert_eq!((r["count"].as_i64(), r["all_count"].as_i64(), r["unread_count"].as_i64()), (Some(2), Some(3), Some(2)), "{r}");
    assert_eq!(r["data"][0]["message"], "middle");
    let (_, r) = call(&app, "GET", "/notification/inbox?limit=1", None).await;
    assert!(r["data"][0]["read_at"].is_string(), "{r}");

    // an empty body marks every one read
    let (s, _) = raw(&app, "POST", "/autobrr/api/notification/inbox/read", Some("")).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/notification/inbox", None).await.1["unread_count"], 0);

    // the cleanup drops messages older than 30 days
    st.db.execute("UPDATE autobrr_notification_inbox SET created_at='2000-01-01T00:00:00Z' WHERE id=?", p![old]).unwrap();
    assert_eq!(st.releases.cleanup_inbox(), 1);

    // deleting needs ids
    let (s, v) = call(&app, "POST", "/notification/inbox/delete", Some(json!({}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "ids is required");
    let (s, v) = raw(&app, "POST", "/autobrr/api/notification/inbox/delete", Some("")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "EOF");
    let (s, _) = call(&app, "POST", "/notification/inbox/delete", Some(json!({ "ids": [middle, new] }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/notification/inbox", None).await.1["data"], json!([]));

    // IDsNotReusedAfterDeleteAll
    let before = store("PUSH_ERROR", "old", Payload::default());
    let (s, _) = call(&app, "DELETE", "/notification/inbox", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let after = store("PUSH_ERROR", "new", Payload::default());
    assert!(after > before);
    let (s, _) = call(&app, "POST", "/notification/inbox/delete", Some(json!({ "ids": [before] }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, r) = call(&app, "GET", "/notification/inbox?limit=10", None).await;
    assert_eq!(r["data"].as_array().unwrap().len(), 1);
    assert_eq!(r["data"][0]["id"], after, "a stale delete must not hit the new one");
}

/// service_test.go "list refresh events", "feed refresh events" and the IRC listener: what each tells, in
/// autobrr's words, reaching the built-in inbox only for the events it wants.
#[tokio::test]
async fn lists_feeds_and_irc_tell_their_events() {
    let (app, st) = lantern("events").await;
    let bid = st.releases.notifications_for("PUSH_ERROR", 0)[0].id;
    let mut builtin = st.releases.notification(bid).unwrap();
    builtin.events = vec!["LIST_REFRESH_ERROR".into(), "FEED_REFRESH_ERROR".into(), "IRC_DISCONNECTED".into(), "IRC_RECONNECTED".into()];
    st.releases.update_notification(builtin).unwrap();

    let list_err = "client sonarr Sonarr not enabled";
    st.releases.notify(list_payload("Sonarr TV", 1, "SONARR", "", None));
    st.releases.notify(list_payload("Sonarr TV", 1, "SONARR", list_err, None));
    st.releases.notify(feed_payload("Mock Indexer", 4, "TORZNAB", "mock", ""));
    st.releases.notify(feed_payload("Mock Indexer", 4, "TORZNAB", "mock", "unexpected status code: 503"));
    st.releases.notify(irc_payload(IrcEvent::Disconnected, "P2P-Network", ""));
    st.releases.notify(irc_payload(IrcEvent::Reconnected, "P2P-Network", ""));
    let words = "Network: P2P-Network stopped after repeated short-lived connections; restart it after resolving the connection issue";
    st.releases.notify(irc_payload(IrcEvent::Flapping, "P2P-Network", words));

    let count = || st.releases.find_inbox(50, 0, false, &[]).unwrap()["all_count"].as_i64().unwrap_or(0);
    assert!(eventually(|| count() == 5).await, "{}", count());
    let (_, r) = call(&app, "GET", "/notification/inbox?limit=10", None).await;
    let mut got: Vec<(String, String, String)> = r["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["event"].as_str().unwrap().into(), m["title"].as_str().unwrap().into(), m["message"].as_str().unwrap_or_default().into()))
        .collect();
    got.sort();
    let want = |e: &str, t: &str, m: &str| (e.to_string(), t.to_string(), m.to_string());
    let mut expect = vec![
        want("LIST_REFRESH_ERROR", "List Refresh Error", "List: Sonarr TV\nError: client sonarr Sonarr not enabled"),
        want("FEED_REFRESH_ERROR", "Feed Refresh Error", "Feed: Mock Indexer\nError: unexpected status code: 503"),
        want("IRC_DISCONNECTED", "IRC Disconnected", "P2P-Network"),
        want("IRC_RECONNECTED", "IRC Reconnected", "P2P-Network"),
        want("IRC_DISCONNECTED", "IRC Disconnected", words),
    ];
    expect.sort();
    assert_eq!(got, expect);

    // the subjects the other senders show
    assert_eq!(irc_payload(IrcEvent::Flapping, "P2P-Network", words).subject, "IRC Stopped");
    assert_eq!(list_payload("Sonarr TV", 1, "SONARR", "", None).subject, "List Refresh Success");
    assert_eq!(feed_payload("Mock Indexer", 4, "TORZNAB", "mock", "").message, "Feed: Mock Indexer");
}

/// service.go `processFilters` with the notification listeners: a filter's push tells RELEASE_NEW and
/// PUSH_APPROVED to the webhook that wants them, with the release in the body.
#[tokio::test]
async fn a_push_reaches_the_webhook() {
    let (app, _) = lantern("push").await;
    let (url, got) = webhook().await;
    store_webhook(&app, "hook", &url, &["RELEASE_NEW", "PUSH_APPROVED"]).await;
    let fid = test_filter(&app, "TV").await;
    let body = json!({ "title": "Show.S01E01.1080p.WEB-DL.H.264-GRP", "download_url": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567" }).to_string();
    let (_, o) = raw(&app, "POST", "/api/releases/announce", Some(&body)).await;
    assert_eq!(o["status"], "pushed", "{o}");
    assert!(eventually(|| got.lock().unwrap().len() >= 2).await, "{:?}", got.lock().unwrap());
    let all = got.clone();
    let got = got.lock().unwrap().clone();
    let events: Vec<&str> = got.iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(events, vec!["release.new", "action.approved"]);
    let push = &got[1].1["data"];
    assert_eq!(push["release"]["name"], "Show.S01E01.1080p.WEB-DL.H.264-GRP", "{push}");
    assert_eq!(push["filter"], json!({ "id": fid, "name": "TV" }), "{push}");
    assert_eq!((push["action"]["name"].as_str(), push["action"]["type"].as_str()), (Some("test it"), Some("TEST")), "{push}");
    assert_eq!(push["result"]["status"], "PUSH_APPROVED", "{push}");

    // the filter picks no events for the hook: its pushes are muted, the new release (no filter) still tells
    let hook = call(&app, "GET", "/notification", None).await.1.as_array().unwrap().iter().find(|n| n["name"] == "hook").unwrap()["id"].clone();
    let (s, _) = call(&app, "PUT", &format!("/filters/{fid}/notifications"), Some(json!([{ "notification_id": hook, "events": [] }]))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let body = json!({ "title": "Show.S01E02.1080p.WEB-DL.H.264-GRP", "download_url": "magnet:?xt=urn:btih:1123456789abcdef0123456789abcdef01234567" }).to_string();
    let (_, o) = raw(&app, "POST", "/api/releases/announce", Some(&body)).await;
    assert_eq!(o["status"], "pushed", "{o}");
    assert!(eventually(|| all.lock().unwrap().len() >= 3).await);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let events: Vec<String> = all.lock().unwrap().iter().map(|(e, _)| e.clone()).collect();
    assert_eq!(events, vec!["release.new", "action.approved", "release.new"]);
}
