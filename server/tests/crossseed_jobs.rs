//! Cross-seed's jobs end to end, against local stand-ins only: Torznab sites, a qBittorrent, a Sonarr and a
//! notification webhook. The data of `dataDirs` and the `.torrent` files of `torrentDir` are searched, Sonarr's
//! ids go into the searches, matches are saved and the inject job adds them, the RSS job reads each site's newest
//! releases once, `POST /api/job` runs a job ahead of schedule and puts its next run later, and cleanup forgets
//! data that is gone.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, Request};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use kit_server_core::{p, Db, RowExt};
use serde_json::{json, Value};
use server::crossseed::config::Override;
use server::crossseed::torrent;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

/// The release on every site.
const TITLE: &str = "Show.Name.S01.1080p.WEB-DL-GRP";

/// Its files.
const FILES: [(&str, i64); 2] = [("Show.Name.S01E01.mkv", 700), ("Show.Name.S01E02.mkv", 800)];

/// What the stand-ins saw and hold.
#[derive(Default)]
struct Fake {
    /// Every site search: its query parameters.
    searches: Vec<HashMap<String, String>>,
    /// The notifications received.
    notified: Vec<Value>,
    /// The titles Sonarr was asked about.
    parsed: Vec<String>,
    /// qBittorrent's torrents: (hash, name, save path).
    torrents: Vec<(String, String, String)>,
}

type Shared = Arc<Mutex<Fake>>;

fn torrent_of(site: &str) -> Vec<u8> {
    match site {
        "c" => torrent::build(TITLE, &[("Show.Name.S01E01.mkv", 750), ("Show.Name.S01E02.mkv", 750)], "c"),
        s => torrent::build(TITLE, &FILES, s),
    }
}

/// A Torznab site: caps (TV search by TVDB id), and one page of releases, the release first.
async fn site(State(f): State<Shared>, Path(s): Path<String>, Query(q): Query<HashMap<String, String>>, h: HeaderMap) -> Response {
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if q.get("t").map(String::as_str) == Some("caps") {
        let caps = r#"<?xml version="1.0"?><caps><searching><search available="yes" supportedParams="q"/><tv-search available="yes" supportedParams="q,season,ep,tvdbid"/></searching>
<categories><category id="5000" name="TV"/></categories></caps>"#;
        return ([(header::CONTENT_TYPE, "application/xml")], caps).into_response();
    }
    f.lock().unwrap().searches.push(q.clone());
    let later_page = q.get("offset").and_then(|o| o.parse::<i64>().ok()).unwrap_or(0) > 0;
    let items = if later_page {
        String::new()
    } else {
        format!(
            r#"<item><title>{TITLE}</title><guid>{s}-1</guid><link>http://{host}/{s}/file</link><size>1500</size><pubDate>Mon, 02 Nov 2026 10:00:00 +0000</pubDate>
<enclosure url="http://{host}/{s}/file" length="1500" type="application/x-bittorrent"/><torznab:attr name="category" value="5000"/></item>
<item><title>Another.Show.S01.1080p-GRP</title><guid>{s}-2</guid><link>http://{host}/{s}/file</link><size>9000</size><pubDate>Mon, 02 Nov 2026 09:00:00 +0000</pubDate></item>"#
        )
    };
    let rss = format!(
        r#"<?xml version="1.0"?><rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>{items}</channel></rss>"#
    );
    ([(header::CONTENT_TYPE, "application/rss+xml")], rss).into_response()
}

async fn file(Path(s): Path<String>) -> Response {
    ([(header::CONTENT_TYPE, "application/x-bittorrent")], torrent_of(&s)).into_response()
}

/// The notification webhook.
async fn notify(State(f): State<Shared>, Json(v): Json<Value>) -> Response {
    f.lock().unwrap().notified.push(v);
    "".into_response()
}

/// Sonarr's `/api/v3/parse`, with its key.
async fn sonarr(State(f): State<Shared>, Query(q): Query<HashMap<String, String>>, h: HeaderMap) -> Response {
    if h.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("k") {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    f.lock().unwrap().parsed.push(q.get("title").cloned().unwrap_or_default());
    Json(json!({ "title": q.get("title"), "series": { "title": "Show Name", "tvdbId": 81189 }, "episodes": [] })).into_response()
}

async fn qbit_add(State(s): State<Shared>, mut m: Multipart) -> Response {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut bytes = Vec::new();
    while let Ok(Some(f)) = m.next_field().await {
        let name = f.name().unwrap_or("").to_string();
        if name == "torrents" {
            bytes = f.bytes().await.unwrap().to_vec();
        } else {
            fields.insert(name, f.text().await.unwrap());
        }
    }
    let Some(meta) = torrent::parse(&bytes) else { return "Fails.".into_response() };
    let mut s = s.lock().unwrap();
    if s.torrents.iter().any(|t| t.0 == meta.info_hash) {
        return "Fails.".into_response();
    }
    s.torrents.push((meta.info_hash, meta.name, fields.get("savepath").cloned().unwrap_or_default()));
    "Ok.".into_response()
}

fn form_of(f: Result<Form<HashMap<String, String>>, axum::extract::rejection::FormRejection>) -> HashMap<String, String> {
    f.map(|Form(f)| f).unwrap_or_default()
}

async fn qbit_info(State(s): State<Shared>, f: Result<Form<HashMap<String, String>>, axum::extract::rejection::FormRejection>) -> Response {
    let f = form_of(f);
    let wanted: Option<Vec<String>> = f.get("hashes").map(|h| h.split('|').map(str::to_lowercase).collect());
    let list: Vec<Value> = s
        .lock()
        .unwrap()
        .torrents
        .iter()
        .filter(|t| wanted.as_ref().is_none_or(|w| w.contains(&t.0)))
        .map(|(hash, name, save)| {
            json!({ "hash": hash, "infohash_v1": hash, "name": name, "category": "", "save_path": save, "content_path": format!("{save}/{name}"),
                    "tags": "", "state": "uploading", "progress": 1.0, "size": 1500, "total_size": 1500, "amount_left": 0, "auto_tmm": false,
                    "tracker": "https://tracker.b/announce" })
        })
        .collect();
    Json(Value::Array(list)).into_response()
}

async fn qbit_files(
    State(s): State<Shared>,
    f: Result<Form<HashMap<String, String>>, axum::extract::rejection::FormRejection>,
) -> Response {
    let q = form_of(f);
    let s = s.lock().unwrap();
    let Some(t) = s.torrents.iter().find(|t| Some(&t.0) == q.get("hash")) else { return axum::http::StatusCode::NOT_FOUND.into_response() };
    let list: Vec<Value> = FILES.iter().map(|(f, size)| json!({ "name": format!("{}/{f}", t.1), "size": size })).collect();
    Json(Value::Array(list)).into_response()
}

async fn fakes() -> (String, Shared) {
    let shared = Shared::default();
    let app = Router::new()
        .route("/{site}/api", get(site))
        .route("/{site}/file", get(file))
        .route("/notify", post(notify))
        .route("/sonarr/api/v3/parse", get(sonarr))
        .route("/qbit/api/v2/auth/login", post(|| async { ([(header::SET_COOKIE, "SID=s1; path=/")], "Ok.") }))
        .route("/qbit/api/v2/app/version", get(|| async { "v5.0.0" }).post(|| async { "v5.0.0" }))
        .route("/qbit/api/v2/torrents/createTags", post(|| async { "" }))
        .route("/qbit/api/v2/torrents/trackers", get(|| async { "[]" }).post(|| async { "[]" }))
        .route("/qbit/api/v2/torrents/categories", get(|| async { "{}" }).post(|| async { "{}" }))
        .route("/qbit/api/v2/torrents/createCategory", post(|| async { "" }))
        .route("/qbit/api/v2/torrents/editCategory", post(|| async { "" }))
        .route("/qbit/api/v2/torrents/recheck", post(|| async { "" }))
        .route("/qbit/api/v2/torrents/start", post(|| async { "" }))
        .route("/qbit/api/v2/torrents/stop", post(|| async { "" }))
        .route("/qbit/api/v2/torrents/add", post(qbit_add))
        .route("/qbit/api/v2/torrents/info", get(qbit_info).post(qbit_info))
        .route("/qbit/api/v2/torrents/files", get(qbit_files).post(qbit_files))
        .with_state(shared.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, shared)
}

/// A fresh Lantern in `name` under the temporary folder, with the sites `sites` and `settings` applied.
async fn lantern(name: &str, url: &str, sites: &[&str], settings: Value) -> (server::AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lantern-xsj-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let vars: HashMap<String, String> =
        [("APP_API_KEY", "test-key"), ("APP_CROSS_SEED_DELAY", "0")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let st = server::AppState::open(&dir.join("data"), web, move |n| vars.get(n).cloned()).await.unwrap();
    st.settings.set_many(&settings).await.unwrap();
    for s in sites {
        let ix = json!({ "kind": "torznab", "name": format!("Site {}", s.to_uppercase()), "base_url": url, "settings": { "api_path": format!("/{s}/api") } });
        st.indexers.add(&ix).unwrap();
    }
    (st, dir)
}

/// The release's files on disk, in `root`.
fn write_release(root: &std::path::Path) -> std::path::PathBuf {
    let folder = root.join(TITLE);
    std::fs::create_dir_all(&folder).unwrap();
    for (f, size) in FILES {
        std::fs::write(folder.join(f), vec![0u8; size as usize]).unwrap();
    }
    folder
}

/// The `.torrent` files saved in the cross-seeds folder.
fn saved(st: &server::AppState) -> Vec<String> {
    let mut list: Vec<String> = std::fs::read_dir(st.cross_seed.output_dir())
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    list.sort();
    list
}

/// Waits (up to five seconds) until `f` holds.
async fn until(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn data_is_searched_with_sonarrs_ids_saved_notified_then_injected() {
    let (url, fake) = fakes().await;
    let tmp = std::env::temp_dir().join(format!("lantern-xsj-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let release = write_release(&tmp);
    let (st, dir) = lantern(
        "data",
        &url,
        &["b", "c"],
        json!({
            "cross_seed_use_client_torrents": false,
            "cross_seed_data_dirs": tmp.to_string_lossy(),
            "cross_seed_action": "save",
            "cross_seed_notification_webhook_urls": format!("{url}/notify"),
            "cross_seed_sonarr": format!("{url}/sonarr?apikey=k"),
        }),
    )
    .await;
    let (cfg, problems) = st.cross_seed.config();
    assert_eq!(cfg.data_dirs, vec![tmp.to_string_lossy().into_owned()], "{problems:?}");

    // the search job: the data is a searchee; Sonarr's TVDB id goes into a TV search
    let found = st.cross_seed.bulk_search(&Override::default()).await.unwrap();
    assert_eq!(found, 1, "site B has the same files; site C's differ");
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.parsed, vec!["Show.Name.S01.1080p.WEB-DL-GRP".to_string()], "Sonarr is asked about the season");
        let tv: Vec<&HashMap<String, String>> = f.searches.iter().filter(|q| q.get("t").map(String::as_str) == Some("tvsearch")).collect();
        assert_eq!(tv.len(), 2, "one search on each site: {:?}", f.searches);
        assert!(tv
            .iter()
            .all(|q| q.get("tvdbid").map(String::as_str) == Some("81189") && q.get("season").map(String::as_str) == Some("1")));
        assert!(tv.iter().all(|q| q.get("q").is_none_or(|v| v.is_empty())), "searched by id, not by words: {tv:?}");
    }
    let b_hash = torrent::parse(&torrent_of("b")).unwrap().info_hash;
    assert_eq!(saved(&st), vec![format!("[pack][Site B]{TITLE}[{b_hash}].torrent")]);
    assert!(until(|| !fake.lock().unwrap().notified.is_empty()).await, "notified");
    {
        let n = fake.lock().unwrap().notified[0].clone();
        assert_eq!(n["title"], "cross-seed");
        assert_eq!(n["body"], format!("search: Saved {TITLE} on 1 tracker by MATCH from dataDir: Site B"));
        assert_eq!(n["extra"]["event"], "RESULTS");
        assert_eq!(n["extra"]["result"], "SAVED");
        assert_eq!(n["extra"]["paused"], true);
        assert_eq!(n["extra"]["infoHashes"], json!([b_hash]));
        assert_eq!(n["extra"]["searchee"]["source"], "dataDir");
        assert_eq!(n["extra"]["searchee"]["path"], release.to_string_lossy().as_ref());
    }

    // searched just now, so the next pass leaves it out (excludeRecentSearch)
    let before = fake.lock().unwrap().searches.len();
    assert_eq!(st.cross_seed.bulk_search(&Override::default()).await.unwrap(), 0);
    assert_eq!(fake.lock().unwrap().searches.len(), before, "nothing searched again");

    // the inject job adds the saved torrent next to the data, and deletes its file once complete
    st.settings.set_many(&json!({ "cross_seed_action": "inject" })).await.unwrap();
    let qbit = json!({ "kind": "qbittorrent", "name": "qBittorrent", "url": format!("{url}/qbit"), "username": "admin", "password": "pw" });
    st.hub.add_client(&qbit).unwrap();
    // qBittorrent has another torrent already (cross-seed takes an empty list for a client it can't read)
    fake.lock().unwrap().torrents.push(("f".repeat(40), "Other".into(), "/elsewhere".into()));
    st.cross_seed.refresh_jobs();
    assert!(st.cross_seed.job("inject").is_some(), "the inject job is there once the action is inject");
    assert!(st.cross_seed.run_job("inject").await);
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.torrents.len(), 2, "added to qBittorrent");
        assert_eq!(
            (f.torrents[1].0.as_str(), f.torrents[1].2.as_str()),
            (b_hash.as_str(), tmp.to_string_lossy().as_ref()),
            "next to the data"
        );
    }
    assert!(until(|| saved(&st).is_empty()).await, "the saved file is deleted once the torrent is complete");
    assert!(until(|| fake.lock().unwrap().notified.len() == 2).await);
    let n = fake.lock().unwrap().notified[1].clone();
    assert_eq!(n["body"], format!("inject: Injected {TITLE} on 1 tracker by MATCH from dataDir: Site B"));

    // cleanup forgets data that is gone
    let n = |st: &server::AppState| st.db.query("SELECT path FROM cross_seed_data", p![]).unwrap().len();
    st.cross_seed.index_torrents_and_data_dirs(&st.cross_seed.config().0, true).await;
    assert_eq!(n(&st), 3, "the data is indexed: the folder and, within maxDataDepth, each episode");
    std::fs::remove_dir_all(&release).unwrap();
    assert!(st.cross_seed.run_job("cleanup").await);
    assert_eq!(n(&st), 0, "and forgotten once deleted");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn torrent_dir_files_are_searched() {
    let (url, _fake) = fakes().await;
    let torrents = std::env::temp_dir().join(format!("lantern-xsj-torrents-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&torrents);
    std::fs::create_dir_all(&torrents).unwrap();
    std::fs::write(torrents.join("mine.torrent"), torrent_of("a")).unwrap();
    let (st, dir) = lantern(
        "tdir",
        &url,
        &["b"],
        json!({ "cross_seed_use_client_torrents": false, "cross_seed_torrent_dir": torrents.to_string_lossy(), "cross_seed_action": "save" }),
    )
    .await;
    // the .torrent files are kept in the table on every look but the first (cross-seed's start-up look only reads them)
    st.cross_seed.index_torrents_and_data_dirs(&st.cross_seed.config().0, false).await;
    let a_hash = torrent::parse(&torrent_of("a")).unwrap().info_hash;
    let kept = st.db.query("SELECT info_hash FROM cross_seed_torrents", p![]).unwrap();
    assert_eq!(kept.iter().map(|r| r.s("info_hash")).collect::<Vec<_>>(), vec![a_hash], "the .torrent is indexed");
    assert_eq!(st.cross_seed.bulk_search(&Override::default()).await.unwrap(), 1);
    let b_hash = torrent::parse(&torrent_of("b")).unwrap().info_hash;
    assert_eq!(saved(&st), vec![format!("[pack][Site B]{TITLE}[{b_hash}].torrent")]);
    let _ = std::fs::remove_dir_all(&torrents);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A request to cross-seed's daemon API: (status, body).
async fn daemon(app: &Router, path: &str, body: &str) -> (u16, String) {
    let req = Request::builder().method("POST").uri(path).header("x-api-key", "test-key").body(Body::from(body.to_string())).unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn rss_reads_each_site_once_and_a_job_run_ahead_waits_a_cadence_more() {
    let (url, fake) = fakes().await;
    let tmp = std::env::temp_dir().join(format!("lantern-xsj-rss-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    write_release(&tmp);
    let (st, dir) = lantern(
        "rss",
        &url,
        &["b"],
        json!({
            "cross_seed": false,
            "cross_seed_use_client_torrents": false,
            "cross_seed_data_dirs": tmp.to_string_lossy(),
            "cross_seed_action": "save",
            "cross_seed_rss_cadence": "10 minutes",
            "cross_seed_search_cadence": "",
        }),
    )
    .await;
    st.cross_seed.refresh_jobs();
    let names: Vec<&str> = st.cross_seed.jobs.lock().unwrap().iter().map(|j| j.name).collect();
    assert_eq!(names, ["rss", "updateIndexerCaps", "cleanup"], "no search job without a search cadence; no inject job for save");

    // the first RSS run: the release is compared with the data and saved; the newest guid is kept
    st.cross_seed.scan_rss_feeds().await.unwrap();
    let b_hash = torrent::parse(&torrent_of("b")).unwrap().info_hash;
    assert_eq!(saved(&st), vec![format!("[pack][Site B]{TITLE}[{b_hash}].torrent")]);
    let guid = st.db.one("SELECT last_seen_guid FROM cross_seed_rss", p![]).unwrap().map(|r| r.s("last_seen_guid"));
    assert_eq!(guid.as_deref(), Some("b-1"));
    let rss = |f: &Fake| f.searches.iter().filter(|q| q.get("t").map(String::as_str) == Some("search")).count();
    assert_eq!(rss(&fake.lock().unwrap()), 2, "every release was new, so the next page was asked for too (it was empty)");
    let decided = |st: &server::AppState| st.db.query("SELECT id FROM cross_seeds", p![]).unwrap().len();
    let after_first = decided(&st);
    assert_eq!(after_first, 1, "the other release is like nothing here, so it isn't decided on");

    // the second run stops at the guid seen last time: nothing new to compare
    st.cross_seed.scan_rss_feeds().await.unwrap();
    assert_eq!(decided(&st), after_first);

    // POST /api/job runs it ahead of schedule, and puts the next run a cadence later
    let app = server::crossseed::daemon::router(st.cross_seed.clone());
    assert_eq!(daemon(&app, "/api/job", "name=rss").await, (200, "rss: running ahead of schedule".into()));
    assert!(until(|| st.cross_seed.job_last_run("rss").is_some()).await, "it ran");
    let last = st.cross_seed.job_last_run("rss").unwrap();
    let now = server::auth::now() * 1000.0;
    assert!(last > now + 9.0 * 60_000.0, "delayNextRun: the log says now plus a cadence");
    let (status, body) = daemon(&app, "/api/job", "name=rss").await;
    assert_eq!(status, 409);
    assert!(body.starts_with("rss: not eligible to run ahead of schedule, next scheduled run is at "), "{body}");
    assert_eq!(daemon(&app, "/api/job", "name=search").await, (404, "search: unable to run, disabled in config".into()));

    // an announce needs something to compare with
    st.settings.set_many(&json!({ "cross_seed_data_dirs": "" })).await.unwrap();
    let ann = format!(r#"{{"name":"{TITLE}","guid":"{url}/b/file","link":"{url}/b/file","tracker":"Site B"}}"#);
    assert_eq!(
        daemon(&app, "/api/announce", &ann).await,
        (500, "Announce requires at least one of useClientTorrents, torrentDir, or dataDirs to be set".into())
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&dir);
}
