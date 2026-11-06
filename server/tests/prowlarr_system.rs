//! Prowlarr's scheduler, backups, log writer and update notification as Lantern runs them: the scheduled tasks
//! queued when due (with the health checks pushed over SignalR), backups made, listed, downloaded, deleted and
//! restored only when asked, the log page and files filled by the log writer, and the notifications told of an
//! update. Every service is a local stand-in (never the internet).

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use kit_server_core::{p, Db, RowExt};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

/// The API key the tests' Lantern has.
const KEY: &str = "test-key";

/// A fresh Lantern on a data folder of its own.
async fn lantern(name: &str) -> (Router, server::AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-psystem-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    reopen(&dir).await
}

/// Lantern on the data folder of `dir` as it is (a start after a restart).
async fn reopen(dir: &std::path::Path) -> (Router, server::AppState, PathBuf) {
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let st = server::AppState::open(&dir.join("data"), web, |n| (n == "APP_API_KEY").then(|| KEY.to_string())).await.unwrap();
    (server::app(st.clone()), st, dir.join("data"))
}

/// One request: status, body (as JSON, `Null` when it is not JSON), the raw body.
async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value, Bytes) {
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 26).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null), b)
}

/// A request with the API key, and a JSON body when there is one.
fn api(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY);
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// One JSON call: status and body.
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (s, v, _) = send(app, api(method, path, body)).await;
    (s, v)
}

/// A form upload of one file, as Prowlarr's web app sends a backup to restore.
fn upload(path: &str, field: &str, file_name: &str, bytes: &[u8]) -> Request<Body> {
    let boundary = "lanternboundary42";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method("POST")
        .uri(path)
        .header("x-api-key", KEY)
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap()
}

/// Waits for command `id` to end; its resource.
async fn finished(app: &Router, id: i64) -> Value {
    for _ in 0..400 {
        let (_, v) = call(app, "GET", &format!("/api/v1/command/{id}"), None).await;
        if v["status"] != "queued" && v["status"] != "started" {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("command {id} didn't finish");
}

/// Sends one WebSocket text frame (masked, as a client does).
async fn ws_send(sock: &mut tokio::net::TcpStream, text: &str) {
    let b = text.as_bytes();
    let mask = [1u8, 2, 3, 4];
    let mut out = vec![0x81, 0x80 | b.len() as u8];
    out.extend_from_slice(&mask);
    out.extend(b.iter().enumerate().map(|(i, x)| x ^ mask[i % 4]));
    sock.write_all(&out).await.unwrap();
}

/// Reads one WebSocket frame's text.
async fn ws_read(sock: &mut tokio::net::TcpStream) -> String {
    let mut head = [0u8; 2];
    sock.read_exact(&mut head).await.unwrap();
    let mut len = (head[1] & 0x7f) as usize;
    if len == 126 {
        let mut l = [0u8; 2];
        sock.read_exact(&mut l).await.unwrap();
        len = u16::from_be_bytes(l) as usize;
    } else if len == 127 {
        let mut l = [0u8; 8];
        sock.read_exact(&mut l).await.unwrap();
        len = u64::from_be_bytes(l) as usize;
    }
    let mut body = vec![0u8; len];
    sock.read_exact(&mut body).await.unwrap();
    String::from_utf8(body).unwrap()
}

/// Connects to the SignalR hub of `app` over a real socket, past the handshake and the version message.
async fn hub(app: &Router) -> tokio::net::TcpStream {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /signalr/messages?id=x&access_token={KEY} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        sock.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    ws_send(&mut sock, "{\"protocol\":\"json\",\"version\":1}\u{1e}").await;
    assert_eq!(ws_read(&mut sock).await, "{}\u{1e}");
    ws_read(&mut sock).await;
    sock
}

/// Reads hub messages until one named `name` comes; its body.
async fn next_message(sock: &mut tokio::net::TcpStream, name: &str) -> Value {
    let wait = async {
        loop {
            let text = ws_read(sock).await;
            for part in text.split('\u{1e}').filter(|p| !p.is_empty()) {
                let m: Value = serde_json::from_str(part).unwrap();
                if m["arguments"][0]["name"] == name {
                    return m["arguments"][0]["body"].clone();
                }
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), wait).await.unwrap_or_else(|_| panic!("no {name} message"))
}

/// Sets a task's last run back so that it is due.
fn make_due(st: &server::AppState, command: &str) {
    st.db
        .execute(
            "UPDATE prowlarr_tasks SET last_execution = ? - (interval + 1) * 60.0 WHERE type_name LIKE ?",
            p![server::auth::now(), format!("%.{command}Command")],
        )
        .unwrap();
}

/// A task as `/api/v1/system/task` lists it.
async fn task(app: &Router, name: &str) -> Value {
    let (_, list) = call(app, "GET", "/api/v1/system/task", None).await;
    list.as_array().unwrap().iter().find(|t| t["taskName"] == name).cloned().unwrap_or_else(|| panic!("no task {name}"))
}

#[tokio::test]
async fn the_scheduler_queues_due_tasks_and_pushes_health() {
    let (app, st, data) = lantern("scheduler").await;
    let mut sock = hub(&app).await;

    // nothing is due right after the first start (every task's last run is the start)
    assert!(server::prowlarr::scheduler::pending(&st.db, server::auth::now()).unwrap().is_empty());
    assert!(server::prowlarr::scheduler::tick(&st).is_empty());

    // the tasks Prowlarr runs on a timer, with Prowlarr's intervals
    let intervals: Vec<(String, i64)> = {
        let (_, list) = call(&app, "GET", "/api/v1/system/task", None).await;
        list.as_array().unwrap().iter().map(|t| (t["taskName"].as_str().unwrap().to_string(), t["interval"].as_i64().unwrap())).collect()
    };
    for (name, minutes) in [
        ("ApplicationIndexerSync", 360),
        ("ApplicationUpdateCheck", 360),
        ("Backup", 7 * 24 * 60),
        ("CheckHealth", 360),
        ("CleanUpHistory", 1440),
        ("ClearExpiredCache", 5),
        ("Housekeeping", 1440),
        ("IndexerDefinitionUpdate", 1440),
        ("MessagingCleanup", 5),
    ] {
        assert!(intervals.contains(&(name.to_string(), minutes)), "{name} every {minutes}: {intervals:?}");
    }

    // the six-hourly app sync, the health checks, a scheduled backup and the clean-ups come due
    let due =
        ["ApplicationIndexerSync", "CheckHealth", "Backup", "MessagingCleanup", "ClearExpiredCache", "Housekeeping", "CleanUpHistory"];
    for name in due {
        make_due(&st, name);
    }
    let before = task(&app, "CheckHealth").await["lastExecution"].clone();
    let ids = server::prowlarr::scheduler::tick(&st);
    assert_eq!(ids.len(), due.len());
    // looking again while they are queued or running queues nothing new
    let again = server::prowlarr::scheduler::tick(&st);
    assert!(again.iter().all(|id| ids.contains(id)), "{again:?} {ids:?}");
    for id in &ids {
        let v = finished(&app, *id).await;
        assert_eq!(
            (v["status"].as_str(), v["trigger"].as_str(), v["priority"].as_str()),
            (Some("completed"), Some("scheduled"), Some("low")),
            "{v}"
        );
        assert_eq!(v["body"]["trigger"], "scheduled");
        assert!(v["body"]["lastExecutionTime"].is_string());
        if v["name"] == "ApplicationIndexerSync" {
            assert_eq!((v["body"]["forceSync"].clone(), v["message"].clone()), (json!(false), json!("Completed")));
        }
        if v["name"] == "Backup" {
            assert_eq!((v["body"]["type"].clone(), v["body"]["updateScheduledTask"].clone()), (json!("scheduled"), json!(true)));
        }
    }
    // each task ran: its last run moved on, and none is due now
    let after = task(&app, "CheckHealth").await;
    assert_ne!(after["lastExecution"], before);
    assert_ne!(after["lastStartTime"], json!("0001-01-01T00:00:00Z"));
    assert!(server::prowlarr::scheduler::pending(&st.db, server::auth::now()).unwrap().is_empty());

    // the health checks' end goes to the web app, which reads /api/v1/health again
    assert_eq!(next_message(&mut sock, "health").await, json!({ "action": "sync" }));

    // the scheduled backup is in the scheduled folder
    let names: Vec<String> = std::fs::read_dir(data.join("Backups/scheduled"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1);
    assert!(names[0].starts_with("lantern_backup_v") && names[0].ends_with(".zip"), "{names:?}");

    // a backup asked for doesn't move the backup task on (BackupCommand.UpdateScheduledTask)
    let backup_before = task(&app, "Backup").await["lastExecution"].clone();
    let (s, v) = call(&app, "POST", "/api/v1/command", Some(json!({ "name": "Backup" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((v["body"]["type"].clone(), v["body"]["updateScheduledTask"].clone()), (json!("manual"), json!(false)));
    finished(&app, v["id"].as_i64().unwrap()).await;
    assert_eq!(task(&app, "Backup").await["lastExecution"], backup_before);
}

#[tokio::test]
async fn housekeeping_tidies_prowlarrs_tables() {
    let (app, st, _) = lantern("housekeeping").await;
    let now = server::auth::now();
    // a command that ended two days ago, a task that says it ran tomorrow, an old log entry, a status left over
    st.db
        .execute(
            "INSERT INTO prowlarr_commands (name, body, priority, status, queued_at, ended_at, trigger, in_queue) VALUES ('Test', '{}', 0, 2, ?, ?, 1, 0)",
            p![now - 2.0 * 86_400.0, now - 2.0 * 86_400.0],
        )
        .unwrap();
    st.db.execute("UPDATE prowlarr_tasks SET last_execution=? WHERE id=1", p![now + 86_400.0]).unwrap();
    st.db
        .execute("INSERT INTO prowlarr_logs (message, time, logger, level) VALUES ('old', ?, 'x', 'Info')", p![now - 10.0 * 86_400.0])
        .unwrap();
    st.db.execute("INSERT INTO prowlarr_logs (message, time, logger, level) VALUES ('new', ?, 'x', 'Info')", p![now]).unwrap();
    st.db.execute("INSERT INTO prowlarr_notification_status (provider_id, escalation_level) VALUES (99, 1)", p![]).unwrap();

    let (s, v) = call(&app, "POST", "/api/v1/command", Some(json!({ "name": "Housekeeping" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(finished(&app, v["id"].as_i64().unwrap()).await["status"], "completed");

    let n = |sql: &str| st.db.one(sql, p![]).unwrap().unwrap().i("n");
    assert_eq!(n("SELECT COUNT(*) AS n FROM prowlarr_commands WHERE name='Test'"), 0);
    assert!(st.db.one("SELECT last_execution AS t FROM prowlarr_tasks WHERE id=1", p![]).unwrap().unwrap().f("t") <= server::auth::now());
    let left: Vec<String> = st.db.query("SELECT message FROM prowlarr_logs", p![]).unwrap().iter().map(|r| r.s("message")).collect();
    assert_eq!(left, ["new"]);
    assert_eq!(n("SELECT COUNT(*) AS n FROM prowlarr_notification_status"), 0);
}

/// The names of the files in a zip.
fn zip_names(bytes: &[u8]) -> Vec<String> {
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    zip.file_names().map(str::to_string).collect()
}

/// A zip with one file in it.
fn zip_of(name: &str, bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut out = std::io::Cursor::new(Vec::new());
    let mut z = zip::ZipWriter::new(&mut out);
    z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(bytes).unwrap();
    z.finish().unwrap();
    out.into_inner()
}

#[tokio::test]
async fn backups_are_made_listed_deleted_and_restored_only_when_asked() {
    let (app, st, data) = lantern("backups").await;
    let dir = data.parent().unwrap().to_path_buf();
    let (s, v) = call(&app, "POST", "/api/v1/tag", Some(json!({ "label": "before" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // a backup asked for: Prowlarr's zip layout, Lantern's database in place of prowlarr.db
    let (_, v) = call(&app, "POST", "/api/v1/command", Some(json!({ "name": "Backup" }))).await;
    assert_eq!(finished(&app, v["id"].as_i64().unwrap()).await["status"], "completed");
    let (s, list) = call(&app, "GET", "/api/v1/system/backup", None).await;
    assert_eq!(s, StatusCode::OK);
    let b = list[0].clone();
    assert_eq!((b["type"].as_str(), list.as_array().unwrap().len()), (Some("manual"), 1));
    let name = b["name"].as_str().unwrap().to_string();
    assert!(
        regex::Regex::new(r"^lantern_backup_v[0-9.]+_\d{4}\.\d{2}\.\d{2}_\d{2}\.\d{2}\.\d{2}\.zip$").unwrap().is_match(&name),
        "{name}"
    );
    assert_eq!(b["path"], format!("/backup/manual/{name}"));
    assert_eq!(b["id"], server::prowlarr::system::hash_int31(&format!("backup-Manual-{name}")));
    let (s, _, zip) = send(&app, api("GET", &format!("/backup/manual/{name}"), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(zip.len() as u64, b["size"].as_u64().unwrap());
    let mut names = zip_names(&zip);
    names.sort();
    assert_eq!(names, ["INFO", "app.db"]);
    let mut info = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(zip.to_vec())).unwrap().by_name("INFO").unwrap().read_to_string(&mut info).unwrap();
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], format!("v{}", kit_server_core::version::base(server::VERSION)));
    assert!(regex::Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$").unwrap().is_match(lines[1]), "{info}");
    assert_eq!(send(&app, api("GET", "/backup/manual/app.db", None)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&app, api("GET", "/backup/manual/..%2Fapp.db", None)).await.0, StatusCode::NOT_FOUND);

    // scheduled backups older than the retention go when the next one is made; manual ones stay
    std::fs::create_dir_all(data.join("Backups/scheduled")).unwrap();
    let old = data.join("Backups/scheduled/lantern_backup_v0.0.1_2020.01.01_00.00.00.zip");
    std::fs::write(&old, b"old").unwrap();
    let f = std::fs::File::options().write(true).open(&old).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(29 * 86_400)).unwrap();
    drop(f);
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || server::prowlarr::backup::make(&st2, server::prowlarr::backup::Kind::Scheduled))
        .await
        .unwrap()
        .unwrap();
    assert!(!old.exists());
    let (_, list) = call(&app, "GET", "/api/v1/system/backup", None).await;
    assert_eq!(list.as_array().unwrap().iter().filter(|b| b["type"] == "manual").count(), 1);
    assert_eq!(list.as_array().unwrap().iter().filter(|b| b["type"] == "scheduled").count(), 1);

    // a change after the backup; nothing is restored until it is asked for
    call(&app, "POST", "/api/v1/tag", Some(json!({ "label": "after" }))).await;
    assert!(!data.join("lantern.restore").exists());

    // restoring by id: put aside for the next start, Prowlarr's answer
    let (s, v) = call(&app, "POST", &format!("/api/v1/system/backup/restore/{}", b["id"]), Some(json!({ "id": b["id"] }))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({ "restartRequired": true })));
    assert!(data.join("lantern.restore").exists());
    let (_, tags) = call(&app, "GET", "/api/v1/tag", None).await;
    assert_eq!(tags.as_array().unwrap().len(), 2, "nothing changes before the restart");
    let (s, _) = call(&app, "POST", "/api/v1/system/backup/restore/12345", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // the next start: the backup's database, the replaced one kept aside
    drop(app);
    let (app, _st, data) = reopen(&dir).await;
    assert!(!data.join("lantern.restore").exists());
    assert!(data.join("app.db.before-restore").exists());
    let (_, tags) = call(&app, "GET", "/api/v1/tag", None).await;
    let labels: Vec<&str> = tags.as_array().unwrap().iter().map(|t| t["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["before"]);

    // uploads: a backup zip, a database, and Prowlarr's refusals
    let (s, v, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", &name, &zip)).await;
    assert_eq!((s, v), (StatusCode::OK, json!({ "restartRequired": true })));
    assert!(data.join("lantern.restore").exists());
    assert!(!data.join("tmp/lantern_backup_restore.zip").exists(), "the upload is deleted after");
    std::fs::remove_file(data.join("lantern.restore")).unwrap();
    let db_bytes = std::fs::read(data.join("app.db.before-restore")).unwrap();
    let (s, _, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", "app.db", &db_bytes)).await;
    assert_eq!(s, StatusCode::OK);
    std::fs::remove_file(data.join("lantern.restore")).unwrap();
    let (s, v, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", "notes.txt", b"x")).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(v["message"], "UnsupportedMediaType: Invalid extension, must be one of: .zip, .db, .xml");
    let none = Request::builder()
        .method("POST")
        .uri("/api/v1/system/backup/restore/upload")
        .header("x-api-key", KEY)
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=b")
        .body(Body::from("--b--\r\n"))
        .unwrap();
    let (s, v, _) = send(&app, none).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::BAD_REQUEST, Some("BadRequest: file must be provided")));
    let (s, v, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", "x.zip", &zip_of("readme.txt", b"x"))).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::NOT_FOUND, Some("Unable to restore database file from backup")));
    let (s, v, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", "p.zip", &zip_of("prowlarr.db", &db_bytes))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["message"].as_str().unwrap().contains("Prowlarr backup"), "{v}");
    let (s, _, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", "x.db", b"not a database at all")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = send(&app, upload("/api/v1/system/backup/restore/upload", "restore", "config.xml", b"<Config/>")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(!data.join("lantern.restore").exists(), "a refused upload restores nothing");

    // deleting: `{}`, then 404
    let (_, list) = call(&app, "GET", "/api/v1/system/backup", None).await;
    let id = list.as_array().unwrap().iter().find(|b| b["type"] == "manual").unwrap()["id"].clone();
    assert_eq!(call(&app, "DELETE", &format!("/api/v1/system/backup/{id}"), None).await, (StatusCode::OK, json!({})));
    assert_eq!(call(&app, "DELETE", &format!("/api/v1/system/backup/{id}"), None).await.0, StatusCode::NOT_FOUND);

    // a backup folder outside the data folder is refused in words (Lantern writes nowhere else)
    server::prowlarr::config_set(&_st.db, "backupfolder", "/elsewhere").unwrap();
    let st3 = _st.clone();
    let e = tokio::task::spawn_blocking(move || server::prowlarr::backup::make(&st3, server::prowlarr::backup::Kind::Manual))
        .await
        .unwrap()
        .unwrap_err();
    assert!(e.detail.contains("outside Lantern's data folder"), "{}", e.detail);
}

#[tokio::test]
async fn the_log_writer_fills_the_log_page_and_files() {
    use server::prowlarr::logwriter::{self, Entry, Level, Writer};
    let (app, st, data) = lantern("logwriter").await;
    let writer = Writer { db: st.db.clone(), folder: server::prowlarr::logs::log_folder(&st) };
    let entry = |level, message: &str| Entry {
        time: server::auth::now(),
        level,
        logger: "prowlarr::commands".into(),
        message: message.into(),
        exception: None,
        exception_type: None,
    };
    writer.write(&[
        entry(Level::Debug, "Starting Housekeeping"),
        entry(Level::Info, "Signed in from 8.8.4.4 apikey=abcdef0123456789"),
        entry(Level::Warn, "An indexer is slow"),
        entry(Level::Error, "An indexer failed"),
    ]);
    let (_, log) = call(&app, "GET", "/api/v1/log?page=1&pageSize=2&sortKey=time&sortDirection=descending", None).await;
    assert_eq!((log["totalRecords"].as_i64(), log["records"].as_array().unwrap().len()), (Some(3), 2));
    assert_eq!((log["records"][0]["level"].as_str(), log["records"][0]["message"].as_str()), (Some("error"), Some("An indexer failed")));
    let (_, log) = call(&app, "GET", "/api/v1/log?level=info&sortDirection=ascending", None).await;
    assert_eq!(log["records"][0]["message"], "Signed in from 8.*.*.4 apikey=(removed)");
    assert_eq!(log["records"][0]["logger"], "prowlarr::commands");
    let (_, files) = call(&app, "GET", "/api/v1/log/file", None).await;
    let mut names: Vec<&str> = files.as_array().unwrap().iter().map(|f| f["filename"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["lantern.debug.txt", "lantern.txt"]);
    let (s, _, text) = send(&app, api("GET", "/api/v1/log/file/lantern.txt", None)).await;
    assert_eq!(s, StatusCode::OK);
    let text = String::from_utf8(text.to_vec()).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d\|Info\|prowlarr::commands\|Signed in").unwrap().is_match(&text),
        "{text}"
    );
    assert!(!text.contains("abcdef0123456789"));

    // the tracing layer: what Lantern logs reaches the writer once it is attached
    use tracing_subscriber::layer::SubscriberExt;
    logwriter::attach(writer.clone());
    let subscriber = tracing_subscriber::registry().with(logwriter::Layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "server::prowlarr::scheduler", "Running housecleaning tasks");
        tracing::info!(target: "hyper::proto", "a library's info is left out");
    });
    let mut found = Vec::new();
    for _ in 0..100 {
        found = st.db.query("SELECT logger FROM prowlarr_logs WHERE message='Running housecleaning tasks'", p![]).unwrap();
        if !found.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].s("logger"), "prowlarr::scheduler");
    assert_eq!(st.db.query("SELECT id FROM prowlarr_logs WHERE logger LIKE 'hyper%'", p![]).unwrap().len(), 0);

    // the commands that empty them
    for name in ["ClearLog", "DeleteLogFiles"] {
        let (_, v) = call(&app, "POST", "/api/v1/command", Some(json!({ "name": name }))).await;
        finished(&app, v["id"].as_i64().unwrap()).await;
    }
    assert_eq!(call(&app, "GET", "/api/v1/log", None).await.1["totalRecords"], 0);
    assert!(std::fs::read_dir(data.join("logs")).unwrap().next().is_none());
}

/// What the webhook stand-in was sent, and whether it fails.
#[derive(Clone, Default)]
struct Hook {
    /// (path, body) of each request.
    got: Arc<Mutex<Vec<(String, String)>>>,
    /// Whether `/broken` answers 500.
    broken: Arc<AtomicBool>,
}

/// The webhook stand-in.
async fn hook_stand_in(State(h): State<Hook>, uri: Uri, _: HeaderMap, body: Bytes) -> Response {
    h.got.lock().unwrap().push((uri.path().to_string(), String::from_utf8_lossy(&body).into_owned()));
    if uri.path() == "/broken" && h.broken.load(Ordering::SeqCst) {
        return (StatusCode::INTERNAL_SERVER_ERROR, "down").into_response();
    }
    StatusCode::OK.into_response()
}

/// Starts the stand-in: its address and what it was sent.
async fn start_hook() -> (String, Hook) {
    let h = Hook::default();
    let app = Router::new().fallback(hook_stand_in).with_state(h.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), h)
}

/// Saves a webhook notification to `url`, on for the events in `on`.
async fn webhook(app: &Router, name: &str, url: &str, on: &[&str]) -> Value {
    let (_, list) = call(app, "GET", "/api/v1/notification/schema", None).await;
    let mut t = list.as_array().unwrap().iter().find(|t| t["implementation"] == "Webhook").cloned().unwrap();
    t["name"] = json!(name);
    for e in on {
        t[*e] = json!(true);
    }
    for f in t["fields"].as_array_mut().unwrap() {
        if f["name"] == "url" {
            f["value"] = json!(url);
        }
    }
    let (s, v) = call(app, "POST", "/api/v1/notification", Some(t)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v
}

#[tokio::test]
async fn an_update_is_told_and_testall_tests_every_notification() {
    let (base, hook) = start_hook().await;
    let (app, st, _) = lantern("update").await;
    let updates = webhook(&app, "Updates", &format!("{base}/updates"), &["onApplicationUpdate"]).await;
    let broken = webhook(&app, "Broken", &format!("{base}/broken"), &["onHealthIssue"]).await;

    // the first start of a version only notes it
    assert_eq!(server::prowlarr::scheduler::version_started(&st).await, None);
    assert_eq!(server::prowlarr::scheduler::version_started(&st).await, None);
    // a start after an update: UpdateInstalledEvent goes to the notifications on for it
    st.db.execute("UPDATE prowlarr_update_history SET version='0.0.0'", p![]).unwrap();
    assert_eq!(server::prowlarr::scheduler::version_started(&st).await.as_deref(), Some("0.0.0"));
    let got = hook.got.lock().unwrap().clone();
    let sent: Vec<Value> = got.iter().filter(|(p, _)| p == "/updates").map(|(_, b)| serde_json::from_str(b).unwrap()).collect();
    let update = sent.iter().find(|v| v["eventType"] == "ApplicationUpdate").expect("an ApplicationUpdate");
    let version = kit_server_core::version::base(server::VERSION);
    assert_eq!(update["message"], format!("Lantern updated from 0.0.0 to {version}"));
    assert_eq!((update["previousVersion"].as_str(), update["newVersion"].as_str()), (Some("0.0.0"), Some(version.as_str())));
    assert!(!got.iter().any(|(p, b)| p == "/broken" && b.contains("ApplicationUpdate")), "only the ones on for updates");

    // testall: every enabled notification tested, 200 while all work
    let (s, v) = call(&app, "POST", "/api/v1/notification/testall", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let mut ids: Vec<i64> = v.as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, [updates["id"].as_i64().unwrap(), broken["id"].as_i64().unwrap()]);
    assert!(v.as_array().unwrap().iter().all(|r| r["isValid"] == true && r["validationFailures"] == json!([])));
    // one failing: 400, with its failure
    hook.broken.store(true, Ordering::SeqCst);
    let (s, v) = call(&app, "POST", "/api/v1/notification/testall", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let failed = v.as_array().unwrap().iter().find(|r| r["id"] == broken["id"]).unwrap();
    assert_eq!(failed["isValid"], false);
    assert!(!failed["validationFailures"].as_array().unwrap().is_empty());
    assert!(v.as_array().unwrap().iter().find(|r| r["id"] == updates["id"]).unwrap()["isValid"] == true);
}
