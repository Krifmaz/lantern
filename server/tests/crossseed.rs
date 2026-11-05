//! Cross-seed end to end: a finished grab in a fake qBittorrent, three fake Torznab sites and Lantern in the
//! middle. Site A is where the grab came from, site B has the same release with the same files (another hash),
//! site C has a release of the same name and size whose files differ, and site D only gives a magnet.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde_json::{json, Value};
use server::crossseed::torrent;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// The release on every site.
const TITLE: &str = "Show.Name.S01.1080p.WEB-DL-GRP";

/// Its files, as on disk and in site A's and site B's torrents.
const FILES: [(&str, i64); 2] = [("Show.Name.S01E01.mkv", 700), ("Show.Name.S01E02.mkv", 800)];

#[derive(Default)]
struct Fake {
    /// qBittorrent's torrents: (hash, name, category, save path, tags).
    torrents: Vec<(String, String, String, String, String)>,
    /// qBittorrent's categories: name → folder.
    categories: HashMap<String, String>,
    /// Torrents removed: (hashes, whether their files were deleted too).
    removed: Vec<(String, bool)>,
}

type Shared = Arc<Mutex<Fake>>;

fn torrent_of(site: &str) -> Vec<u8> {
    match site {
        "c" => torrent::build(TITLE, &[("Show.Name.S01E01.mkv", 750), ("Show.Name.S01E02.mkv", 750)], "c"),
        s => torrent::build(TITLE, &FILES, s),
    }
}

/// A Torznab site: caps, a search answering the release, and its file.
async fn site(Path(s): Path<String>, Query(q): Query<HashMap<String, String>>, h: axum::http::HeaderMap) -> Response {
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if q.get("t").map(String::as_str) == Some("caps") {
        let caps = r#"<?xml version="1.0"?><caps><searching><search available="yes" supportedParams="q"/></searching>
<categories><category id="5000" name="TV"/></categories></caps>"#;
        return ([(header::CONTENT_TYPE, "application/xml")], caps).into_response();
    }
    let link = if s == "d" {
        "magnet:?xt=urn:btih:dddddddddddddddddddddddddddddddddddddddd".to_string()
    } else {
        format!("http://{host}/{s}/file")
    };
    let link = html_escape::encode_double_quoted_attribute(&link).into_owned();
    let rss = format!(
        r#"<?xml version="1.0"?><rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>
<item><title>{TITLE}</title><guid>{s}-1</guid><link>{link}</link><size>1500</size><enclosure url="{link}" length="1500" type="application/x-bittorrent"/>
<torznab:attr name="category" value="5000"/><torznab:attr name="seeders" value="5"/></item>
<item><title>Another.Show.S01.1080p-GRP</title><guid>{s}-2</guid><link>http://{host}/{s}/file</link><size>1500</size></item>
</channel></rss>"#
    );
    ([(header::CONTENT_TYPE, "application/rss+xml")], rss).into_response()
}

async fn file(Path(s): Path<String>) -> Response {
    ([(header::CONTENT_TYPE, "application/x-bittorrent")], torrent_of(&s)).into_response()
}

async fn qbit_login() -> Response {
    ([(header::SET_COOKIE, "SID=s1; path=/")], "Ok.").into_response()
}

async fn qbit_category(State(s): State<Shared>, Form(f): Form<HashMap<String, String>>) -> Response {
    s.lock().unwrap().categories.insert(f["category"].clone(), f["savePath"].clone());
    "".into_response()
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
    s.torrents.push((meta.info_hash, meta.name, fields["category"].clone(), fields["savepath"].clone(), fields["tags"].clone()));
    "Ok.".into_response()
}

async fn qbit_delete(State(s): State<Shared>, Form(f): Form<HashMap<String, String>>) -> Response {
    let mut s = s.lock().unwrap();
    s.torrents.retain(|t| t.0 != f["hashes"]);
    s.removed.push((f["hashes"].clone(), f["deleteFiles"] == "true"));
    "".into_response()
}

async fn qbit_info(State(s): State<Shared>) -> Response {
    let list: Vec<Value> = s
        .lock()
        .unwrap()
        .torrents
        .iter()
        .map(|(hash, name, cat, save, tags)| {
            json!({ "hash": hash, "name": name, "category": cat, "save_path": save, "content_path": format!("{save}/{name}"),
                    "tags": tags, "state": "uploading", "progress": 1.0, "size": 1500, "dlspeed": 0, "eta": 8640000, "ratio": 0.5, "seeding_time": 60 })
        })
        .collect();
    axum::Json(Value::Array(list)).into_response()
}

async fn qbit_files(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let s = s.lock().unwrap();
    let Some(t) = s.torrents.iter().find(|t| t.0 == q["hash"]) else { return StatusCode::NOT_FOUND.into_response() };
    let list: Vec<Value> = FILES.iter().map(|(f, size)| json!({ "name": format!("{}/{f}", t.1), "size": size })).collect();
    axum::Json(Value::Array(list)).into_response()
}

async fn fakes() -> (String, Shared) {
    let shared = Shared::default();
    let app = Router::new()
        .route("/{site}/api", get(site))
        .route("/{site}/file", get(file))
        .route("/qbit/api/v2/auth/login", post(qbit_login))
        .route("/qbit/api/v2/app/version", get(|| async { "v5.0.0" }))
        .route("/qbit/api/v2/torrents/createCategory", post(qbit_category))
        .route("/qbit/api/v2/torrents/editCategory", post(qbit_category))
        .route("/qbit/api/v2/torrents/add", post(qbit_add))
        .route("/qbit/api/v2/torrents/info", get(qbit_info))
        .route("/qbit/api/v2/torrents/delete", post(qbit_delete))
        .route("/qbit/api/v2/torrents/files", get(qbit_files))
        .with_state(shared.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, shared)
}

async fn call(app: &Router, method: &str, path: &str) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(path).header("x-api-key", "test-key").body(Body::empty()).unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_finished_torrent_seeds_on_the_other_sites_that_have_the_same_files() {
    let (url, fake) = fakes().await;
    let dir = std::env::temp_dir().join(format!("lantern-xs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let vars: HashMap<String, String> =
        [("APP_API_KEY", "test-key"), ("APP_CROSS_SEED_DELAY", "0"), ("APP_CLEANUP", "delete"), ("APP_SEED_HOURS", "0.5")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
    let st = server::AppState::open(&dir.join("data"), web, move |n| vars.get(n).cloned()).await.unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    let app = server::app(st.clone());

    let mut ids = HashMap::new();
    for s in ["a", "b", "c", "d"] {
        let id = st.indexers.add(&json!({ "kind": "torznab", "name": format!("Site {}", s.to_uppercase()), "base_url": url, "settings": { "api_path": format!("/{s}/api") } })).unwrap();
        ids.insert(s, id);
    }
    let client = st
        .hub
        .add_client(
            &json!({ "kind": "qbittorrent", "name": "qBittorrent", "url": format!("{url}/qbit"), "username": "admin", "password": "pw" }),
        )
        .unwrap();

    // the grab from site A, finished and seeding in qBittorrent
    let source = torrent::parse(&torrent_of("a")).unwrap();
    fake.lock().unwrap().torrents.push((
        source.info_hash.clone(),
        TITLE.into(),
        "sonarr".into(),
        "/downloads/sonarr".into(),
        "lantern,lantern-1".into(),
    ));
    let now = server::auth::now();
    use kit_server_core::Db;
    st.db
        .execute(
            "INSERT INTO hub_grabs (app_id, title, protocol, indexer_id, link, client_id, client_ref, category, state, progress, size, added, updated, finished)
             VALUES (1, ?, 'torrent', ?, '', ?, ?, 'sonarr', 'seeding', 1, 1500, ?, ?, ?)",
            kit_server_core::p![TITLE, ids["a"], client, source.info_hash, now, now, now],
        )
        .unwrap();
    assert_eq!(st.cross_seed.due(10).unwrap().len(), 1);

    let (s, v) = call(&app, "POST", "/api/cross-seed/grabs/1").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["candidates"].as_i64(), v["injected"].as_i64()), (Some(3), Some(1)), "{v}");
    assert_eq!(v["message"], "Seeding on 1 more site now, from the same files");

    // site B's copy is in qBittorrent next to the files, in a category the app doesn't watch
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.torrents.len(), 2);
        let b = &f.torrents[1];
        assert_eq!(b.0, torrent::parse(&torrent_of("b")).unwrap().info_hash);
        assert_eq!((b.2.as_str(), b.3.as_str()), ("sonarr.cross-seed", "/downloads/sonarr"));
        assert!(b.4.contains("lantern-xs-1"), "{}", b.4);
        assert_eq!(f.categories.get("sonarr.cross-seed").map(String::as_str), Some("/downloads/sonarr"));
    }

    // what happened to each candidate
    let (_, v) = call(&app, "GET", "/api/cross-seed").await;
    let recent = v["recent"].as_array().unwrap();
    let by_site = |name: &str| recent.iter().find(|r| r["indexer"] == name).map(|r| r["decision"].as_str().unwrap().to_string());
    assert_eq!(by_site("Site B").as_deref(), Some("injected"));
    assert_eq!(by_site("Site C").as_deref(), Some("files_differ"));
    assert_eq!(by_site("Site D").as_deref(), Some("magnet_only"));
    assert!(by_site("Site A").is_none(), "the grab's own site isn't a candidate");
    assert_eq!(v["stats"]["injected"], 1);
    assert_eq!(v["stats"]["waiting"], 0, "searched, so not due again for a week");

    // searching again adds nothing twice
    let (_, v) = call(&app, "POST", "/api/cross-seed/grabs/1").await;
    assert_eq!(v["injected"], 0, "{v}");
    assert_eq!(fake.lock().unwrap().torrents.len(), 2);

    // only finished torrents can be cross-seeded
    st.db.execute("UPDATE hub_grabs SET state='downloading' WHERE id=1", kit_server_core::p![]).unwrap();
    let (s, v) = call(&app, "POST", "/api/cross-seed/grabs/1").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["detail"], "Only a finished torrent can be cross-seeded");

    // cleanup set to delete files still keeps them, since site B's copy seeds from them
    st.db.execute("UPDATE hub_grabs SET state='seeding', imported=?, seed_secs=3600 WHERE id=1", kit_server_core::p![now]).unwrap();
    st.hub.tick().await;
    assert_eq!(fake.lock().unwrap().removed, vec![(source.info_hash.clone(), false)]);
    let _ = std::fs::remove_dir_all(&dir);
}
