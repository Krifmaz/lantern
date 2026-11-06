// Ported from autobrr v1.90.0 (internal/database/release_test.go CheckIsDuplicateRelease,
// CheckSmartEpisodeCanDownloadShow, Delete and cleanup job cases; internal/release/cleanup_test.go;
// internal/release/service_test.go; internal/http/release_test.go), GPL-2.0-or-later, with API shape checks
// against autobrr's Go structs (tests/fixtures/upstream/autobrr/api/structs.json).
//! autobrr's release history under `/autobrr/api`: a release a filter sends is kept with each action's status,
//! the list, stats and delete routes answer as autobrr's do, an action is run again on retry, the duplicate
//! profiles and the newer-episode check read the history, and the cleanup jobs delete on a schedule.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use server::releases::history::{ActionStatus, CleanupJob, DeleteRequest, DuplicateProfile, SmartEpisodeParams, PUSH_APPROVED};
use server::releases::release::{IndexerMinimal, Release};
use std::path::PathBuf;
use tower::ServiceExt;

const KEY: &str = "test-key";

/// Lantern with an empty data folder.
async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-abhist-{name}-{}", std::process::id()));
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
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
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

/// A release a test pushed: stored with one approved action status.
fn push(st: &server::AppState, title: &str, filter_id: i64) -> Release {
    let mut r = Release::new(IndexerMinimal { name: "Mock".into(), identifier: "mock".into(), identifier_external: "Mock".into(), id: 0 });
    r.parse_string(title);
    r.filter_id = filter_id;
    st.releases.store_history_release(&mut r).unwrap();
    let mut s = ActionStatus {
        status: PUSH_APPROVED.into(),
        action: "test".into(),
        kind: "TEST".into(),
        filter: "Test filter".into(),
        filter_id,
        release_id: r.id,
        timestamp: server::releases::history::now_text(),
        ..ActionStatus::default()
    };
    st.releases.store_action_status(&mut s).unwrap();
    r
}

/// A profile from autobrr's Go field names.
fn profile_of(v: &Value) -> DuplicateProfile {
    let b = |k: &str| v[k].as_bool().unwrap_or(false);
    DuplicateProfile {
        protocol: b("Protocol"),
        release_name: b("ReleaseName"),
        hash: b("Hash"),
        title: b("Title"),
        sub_title: b("SubTitle"),
        year: b("Year"),
        month: b("Month"),
        day: b("Day"),
        source: b("Source"),
        resolution: b("Resolution"),
        codec: b("Codec"),
        container: b("Container"),
        dynamic_range: b("DynamicRange"),
        audio: b("Audio"),
        group: b("Group"),
        season: b("Season"),
        episode: b("Episode"),
        website: b("Website"),
        proper: b("Proper"),
        repack: b("Repack"),
        edition: b("Edition"),
        hybrid: b("Hybrid"),
        language: b("Language"),
        ..DuplicateProfile::default()
    }
}

/// release_test.go `TestReleaseRepo_CheckIsDuplicateRelease`: all 40 cases, read from the Go table.
#[tokio::test]
async fn duplicate_profiles_find_releases_already_taken() {
    let (_, st) = lantern("dups").await;
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/release/duplicates.json");
    let table: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let cases = table["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 40);
    let mut wrong = Vec::new();
    for c in cases {
        for t in c["release_titles"].as_array().unwrap() {
            push(&st, t.as_str().unwrap(), 0);
        }
        let mut cmp = Release::new(IndexerMinimal { name: "Mock".into(), identifier: "mock".into(), ..IndexerMinimal::default() });
        cmp.parse_string(c["release_title"].as_str().unwrap());
        let got = st.releases.is_duplicate_release(&profile_of(&c["profile"]), &cmp).unwrap();
        if got != c["is_duplicate"].as_bool().unwrap() {
            wrong.push(format!("case {}: got {got}", c["name"]));
        }
        st.releases.delete_history(&DeleteRequest::default()).unwrap();
    }
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// release_test.go `TestReleaseRepo_CheckSmartEpisodeCanDownloadShow`, and the newer-episode cases its query
/// takes: a newer episode, a later season and a later day already pushed say no; a PROPER or a REPACK by the
/// same group is let through only when no such one was pushed.
#[tokio::test]
async fn smart_episode_reads_the_history() {
    let (_, st) = lantern("smart").await;
    push(&st, "Example.Torrent.Name.S01E01.1080p.WEB-DL-GRP", 0);
    let p = |title: &str, season, episode| SmartEpisodeParams { title: title.into(), season, episode, ..SmartEpisodeParams::default() };
    // the Go case: S01E02 after S01E01
    assert!(st.releases.smart_episode_can_download(&p("Example.Torrent.Name", 1, 2)).unwrap());
    push(&st, "Show.S02E05.1080p.WEB-DL-GRP", 0);
    assert!(!st.releases.smart_episode_can_download(&p("Show", 2, 4)).unwrap());
    assert!(!st.releases.smart_episode_can_download(&p("Show", 1, 9)).unwrap());
    assert!(st.releases.smart_episode_can_download(&p("Show", 2, 6)).unwrap());
    assert!(!st.releases.smart_episode_can_download(&p("Show", 1, 0)).unwrap(), "a season pack after a later season");
    assert!(st.releases.smart_episode_can_download(&p("Show", 3, 0)).unwrap());
    // nothing to compare: always yes
    assert!(st.releases.smart_episode_can_download(&p("Show", 0, 0)).unwrap());
    push(&st, "Daily.Show.2024.05.10.1080p.WEB-DL-GRP", 0);
    let daily = |y, m, d| SmartEpisodeParams { title: "Daily Show".into(), year: y, month: m, day: d, ..SmartEpisodeParams::default() };
    assert!(daily(2024, 5, 9).is_daily_episode());
    assert!(!st.releases.smart_episode_can_download(&daily(2024, 5, 9)).unwrap());
    assert!(!st.releases.smart_episode_can_download(&daily(2024, 4, 30)).unwrap());
    assert!(st.releases.smart_episode_can_download(&daily(2024, 5, 11)).unwrap());
    let repack = SmartEpisodeParams { repack: true, group: "OTHER".into(), ..p("Show", 2, 4) };
    assert!(st.releases.smart_episode_can_download(&repack).unwrap(), "no repack by that group was pushed");
}

/// A filter that sends everything to a TEST action; its id.
async fn test_filter(app: &Router, name: &str, extra: Value) -> i64 {
    let mut body = json!({ "name": name, "enabled": true });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let (s, f) = call(app, "POST", "/filters", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    let mut full = f.clone();
    full["actions"] = json!([{ "name": "test it", "type": "TEST", "enabled": true }]);
    let (s, v) = call(app, "PUT", &format!("/filters/{}", f["id"]), Some(full)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    f["id"].as_i64().unwrap()
}

/// Announces `title` through Lantern's own route: the outcome.
async fn announce(app: &Router, title: &str) -> Value {
    let body = json!({ "title": title, "download_url": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567" }).to_string();
    raw(app, "POST", "/api/releases/announce", Some(&body)).await.1
}

/// http/release.go: the release routes over what the pipeline kept (service.go `processFilters`): the page,
/// the newest ten, the counts, the stats (release_test.go `TestReleaseHandler_StatsEndpoints*`), one release,
/// the indexers, a retry and the deletes.
#[tokio::test]
async fn the_pipeline_keeps_each_push_and_the_routes_read_it() {
    let (app, _) = lantern("routes").await;
    let fid = test_filter(&app, "TV", json!({ "resolutions": ["1080p"] })).await;
    let o = announce(&app, "Show.S01E01.1080p.WEB-DL.H.264-GRP").await;
    assert_eq!(o["status"], "pushed", "{o}");
    let o = announce(&app, "Show.S01E01.720p.WEB-DL.H.264-GRP").await;
    assert_eq!(o["status"], "rejected", "{o}");

    let (s, page) = call(&app, "GET", "/release", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("FindReleasesResponse", &page);
    assert_eq!(page["count"], 1, "only releases a filter sends are kept");
    let r = &page["data"][0];
    assert_shape("Release", r);
    assert_eq!(
        (r["name"].as_str(), r["filter"].as_str(), r["filter_status"].as_str()),
        (Some("Show.S01E01.1080p.WEB-DL.H.264-GRP"), Some("TV"), Some("FILTER_APPROVED"))
    );
    assert_eq!(r["resolution"], "1080p");
    let status = &r["action_status"][0];
    assert_shape("ReleaseActionStatus", status);
    assert_eq!(
        (status["status"].as_str(), status["action"].as_str(), status["type"].as_str()),
        (Some("PUSH_APPROVED"), Some("test it"), Some("TEST"))
    );
    assert_eq!(status["filter_id"], fid);
    assert_eq!(page["next_cursor"], r["id"]);
    let id = r["id"].as_i64().unwrap();

    // the filters of the page
    assert_eq!(call(&app, "GET", "/release?q=Show", None).await.1["count"], 1);
    assert_eq!(call(&app, "GET", "/release?q=Other", None).await.1["count"], 0);
    assert_eq!(call(&app, "GET", "/release?q=resolution:1080p", None).await.1["count"], 1);
    assert_eq!(call(&app, "GET", "/release?push_status=PUSH_APPROVED", None).await.1["count"], 1);
    assert_eq!(call(&app, "GET", "/release?push_status=PUSH_ERROR", None).await.1["count"], 0);
    assert_eq!(call(&app, "GET", "/release?indexer=nope", None).await.1["count"], 0);
    assert_eq!(call(&app, "GET", &format!("/release?cursor={id}"), None).await.1["count"], 0);
    let (s, v) = call(&app, "GET", "/release?limit=0", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "limit parameter is invalid");
    let (s, v) = call(&app, "GET", "/release?push_status=NOPE", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "push_status parameter is of invalid type: NOPE");
    let (s, v) = call(&app, "GET", "/release?offset=-1", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "offset parameter is invalid");

    let (_, recent) = call(&app, "GET", "/release/recent", None).await;
    assert_eq!(recent["data"][0]["id"], id);
    let (_, stats) = call(&app, "GET", "/release/stats", None).await;
    assert_shape("ReleaseStats", &stats);
    assert_eq!(
        (stats["total_count"].as_i64(), stats["filtered_count"].as_i64(), stats["push_approved_count"].as_i64()),
        (Some(1), Some(1), Some(1))
    );
    for (path, field, shape) in [
        ("activity", "daily", "ReleaseActivityStats"),
        ("volume", "daily", "ReleaseVolumeStats"),
        ("heatmap", "heatmap", "ReleaseHeatmapStats"),
        ("indexers", "top", "ReleaseTopIndexersStats"),
        ("filters", "top", "ReleaseTopFiltersStats"),
    ] {
        let (s, v) = call(&app, "GET", &format!("/release/stats/{path}?days=90"), None).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert_shape(shape, &v);
        assert_eq!(v["days"], 90, "{path}");
        assert!(!v[field].is_null(), "{path}");
        assert_eq!(call(&app, "GET", &format!("/release/stats/{path}"), None).await.1["days"], 30, "{path}: the default");
        assert_eq!(call(&app, "GET", &format!("/release/stats/{path}?days=0"), None).await.1["days"], 0, "{path}: all time");
        for bad in ["abc", "-1", "9999"] {
            let (s, v) = call(&app, "GET", &format!("/release/stats/{path}?days={bad}"), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{path} {bad}");
            assert_error(&v, "BAD_REQUEST_PARAMS", "days parameter is invalid");
        }
    }
    let (_, act) = call(&app, "GET", "/release/stats/activity?days=1", None).await;
    assert_shape("ReleaseActivityDaily", &act["daily"][0]);
    assert_eq!((act["daily"][0]["matched_count"].as_i64(), act["daily"][0]["push_approved_count"].as_i64()), (Some(1), Some(1)));
    let (_, heat) = call(&app, "GET", "/release/stats/heatmap", None).await;
    assert_eq!(heat["heatmap"].as_array().unwrap().len(), 168);
    assert_eq!(heat["heatmap"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).sum::<i64>(), 1);
    let (_, top) = call(&app, "GET", "/release/stats/filters", None).await;
    assert_shape("ReleaseFilterStats", &top["top"][0]);
    assert_eq!((top["top"][0]["filter"].as_str(), top["top"][0]["push_approved_count"].as_i64()), (Some("TV"), Some(1)));

    let (s, one) = call(&app, "GET", &format!("/release/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("Release", &one);
    assert_eq!(one["name"], "Show.S01E01.1080p.WEB-DL.H.264-GRP");
    let (s, v) = call(&app, "GET", "/release/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find release with id 999");
    let (s, v) = call(&app, "GET", "/release/abc", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "releaseID parameter is invalid");
    let (s, list) = call(&app, "GET", "/release/indexers", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(list.is_array(), "{list}");

    // POST /release/process is commented out in autobrr v1.90.0
    let (s, _) = call(&app, "POST", "/release/process", Some(json!({}))).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);

    // a retry runs the action again and keeps a second status (it needs the release's indexer: Lantern's announce
    // route has none, so autobrr's not-found answer comes back)
    let status_id = status["id"].as_i64().unwrap();
    let (s, v) = call(&app, "POST", &format!("/release/{id}/actions/{status_id}/retry"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    assert_error(&v, "RECORD_NOT_FOUND", &format!("action status with id {status_id} not found for release {id}"));
    let (s, v) = call(&app, "POST", &format!("/release/{id}/actions/x/retry"), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "actionStatusID parameter is invalid");

    // the deletes
    let (s, v) = call(&app, "DELETE", "/release?releaseStatus=NOPE", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "releaseStatus contains invalid value: NOPE");
    let (s, _) = call(&app, "DELETE", "/release?olderThan=1", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/release", None).await.1["count"], 1, "it is younger than an hour");
    let (s, _) = call(&app, "DELETE", "/release?releaseStatus=PUSH_REJECTED", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/release", None).await.1["count"], 1, "an approved release stays");
    let (s, _) = call(&app, "DELETE", "/release", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/release", None).await.1["count"], 0);
}

/// service.go `Retry`: a release with its indexer gets the action run again, with a new status.
#[tokio::test]
async fn a_retry_runs_the_action_again() {
    let (app, st) = lantern("retry").await;
    let (s, idx) = call(&app, "POST", "/indexer", Some(json!({ "name": "Mock", "identifier": "lighthouse", "enabled": true, "implementation": "irc", "settings": { "passkey": "abc" } }))).await;
    assert!(s.is_success(), "{idx}");
    let fid = test_filter(&app, "Retry", json!({})).await;
    let actions = st.releases.actions_of(fid, true, false).unwrap();
    let mut r = Release::new(IndexerMinimal { identifier: idx["identifier"].as_str().unwrap().into(), ..IndexerMinimal::default() });
    r.parse_string("Some.Movie.2024.1080p.WEB-DL-GRP");
    r.filter_id = fid;
    r.filter_name = "Retry".into();
    st.releases.store_history_release(&mut r).unwrap();
    let mut first = ActionStatus::new(&actions[0], &r);
    first.status = "PUSH_ERROR".into();
    st.releases.store_action_status(&mut first).unwrap();
    let (s, v) = call(&app, "POST", &format!("/release/{}/actions/{}/retry", r.id, first.id), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    let (_, page) = call(&app, "GET", "/release", None).await;
    let statuses = page["data"][0]["action_status"].as_array().unwrap().clone();
    assert_eq!(statuses.len(), 2, "{page}");
    assert_eq!((statuses[0]["status"].as_str(), statuses[1]["status"].as_str()), (Some("PUSH_ERROR"), Some("PUSH_APPROVED")));
    assert_eq!(page["data"][0]["indexer"]["name"], "Mock");
}

/// service.go `processFilters` with `CheckFilter`'s history checks: a newer-episode filter turns down an older
/// episode, a duplicate profile turns down what was already taken, and both show why.
#[tokio::test]
async fn smart_episode_and_duplicate_profiles_turn_releases_down() {
    let (app, _) = lantern("checks").await;
    test_filter(&app, "Smart", json!({ "smart_episode": true, "shows": "Show" })).await;
    assert_eq!(announce(&app, "Show.S01E05.1080p.WEB-DL-GRP").await["status"], "pushed");
    let o = announce(&app, "Show.S01E03.1080p.WEB-DL-GRP").await;
    assert_eq!(o["status"], "rejected", "{o}");
    assert_eq!(
        o["reasons"][0]["why"][0],
        "[smart episode] not matching: got not new (Show) season: 1 ep: 3 want: expected newer than (Show) season: 1 ep: 3",
        "{o}"
    );
    assert_eq!(announce(&app, "Show.S01E06.1080p.WEB-DL-GRP").await["status"], "pushed");

    // the "Exact release" profile autobrr seeds
    let (_, profiles) = call(&app, "GET", "/release/profiles/duplicate", None).await;
    assert_eq!(profiles.as_array().unwrap().len(), 3, "{profiles}");
    for p in profiles.as_array().unwrap() {
        assert_shape("DuplicateReleaseProfile", p);
    }
    assert_eq!(profiles[0]["name"], "Exact release");
    test_filter(&app, "Dupes", json!({ "release_profile_duplicate_id": 1, "match_releases": "Movie" })).await;
    assert_eq!(announce(&app, "Movie.2024.1080p.WEB-DL-GRP").await["status"], "pushed");
    let o = announce(&app, "Movie.2024.1080p.WEB-DL-GRP").await;
    assert_eq!(o["status"], "rejected", "{o}");
    let why = o["reasons"].as_array().unwrap().iter().find(|r| r["filter"] == "Dupes").unwrap();
    assert_eq!(why["why"][0], "[duplicate] not matching: got duplicate want: not duplicate", "{o}");
}

/// http/release.go: the duplicate profile routes.
#[tokio::test]
async fn duplicate_profile_routes() {
    let (app, _) = lantern("profiles").await;
    let (s, p) = call(&app, "POST", "/release/profiles/duplicate", Some(json!({ "name": "Mine", "title": true, "year": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    assert_shape("DuplicateReleaseProfile", &p);
    let id = p["id"].as_i64().unwrap();
    assert!(id > 3);
    let (s, _) = call(&app, "POST", "/release/profiles/duplicate", Some(json!({ "id": id, "name": "Mine 2", "title": true }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, all) = call(&app, "GET", "/release/profiles/duplicate", None).await;
    let mine = all.as_array().unwrap().iter().find(|p| p["id"] == id).unwrap().clone();
    assert_eq!((mine["name"].as_str(), mine["year"].as_bool()), (Some("Mine 2"), Some(false)));
    let (s, _) = call(&app, "DELETE", &format!("/release/profiles/duplicate/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/release/profiles/duplicate", None).await.1.as_array().unwrap().len(), 3);
    let (s, v) = call(&app, "DELETE", "/release/profiles/duplicate/x", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_error(&v, "BAD_REQUEST_PARAMS", "profileId parameter is invalid");
}

/// http/release_test.go: the cleanup job routes (`TestReleaseHandler_*CleanupJob*`), with autobrr's real service
/// behind them (so a job is checked before it is saved, and an enabled one is scheduled).
#[tokio::test]
async fn cleanup_job_routes() {
    let (app, st) = lantern("jobs").await;
    let job = json!({ "name": "New Job", "enabled": false, "schedule": "0 2 * * *", "older_than": 168, "indexers": "btn,ptp", "statuses": "PUSH_REJECTED" });
    let (s, created) = call(&app, "POST", "/release/cleanup-jobs", Some(job)).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    assert_shape("ReleaseCleanupJob", &created);
    assert_eq!(created["id"], 1);
    for (k, v) in [
        ("name", json!("New Job")),
        ("enabled", json!(false)),
        ("schedule", json!("0 2 * * *")),
        ("older_than", json!(168)),
        ("indexers", json!("btn,ptp")),
        ("statuses", json!("PUSH_REJECTED")),
    ] {
        assert_eq!(created[k], v, "{k}");
    }
    assert_eq!(created["next_run"], "0001-01-01T00:00:00Z");
    let (s, list) = call(&app, "GET", "/release/cleanup-jobs", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_shape("ReleaseCleanupJob", &list[0]);
    let (s, one) = call(&app, "GET", "/release/cleanup-jobs/1", None).await;
    assert_eq!((s, one["name"].as_str()), (StatusCode::OK, Some("New Job")));
    let (s, v) = call(&app, "GET", "/release/cleanup-jobs/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find cleanup job with id 999");

    // a job that doesn't check out is refused before it is saved
    let (s, v) = call(&app, "POST", "/release/cleanup-jobs", Some(json!({ "name": "x", "schedule": "0 2 * * *", "older_than": 0 }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_error(&v, "INTERNAL_SERVER_ERROR", "older_than must be greater than 0");

    let update = json!({ "id": 1, "name": "Updated Name", "enabled": true, "schedule": "0 4 * * *", "older_than": 168, "indexers": "btn,ptp", "statuses": "PUSH_ERROR" });
    let (s, updated) = call(&app, "PUT", "/release/cleanup-jobs/1", Some(update)).await;
    assert_eq!(s, StatusCode::OK, "{updated}");
    assert_eq!(
        (updated["name"].as_str(), updated["enabled"].as_bool(), updated["statuses"].as_str()),
        (Some("Updated Name"), Some(true), Some("PUSH_ERROR"))
    );
    let (_, list) = call(&app, "GET", "/release/cleanup-jobs", None).await;
    assert_ne!(list[0]["next_run"], "0001-01-01T00:00:00Z", "an enabled job is scheduled: {list}");
    let missing = json!({ "id": 999, "name": "Does Not Exist", "schedule": "0 4 * * *", "older_than": 1 });
    let (s, v) = call(&app, "PUT", "/release/cleanup-jobs/999", Some(missing)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_error(&v, "RECORD_NOT_FOUND", "could not find cleanup job with id 999");

    let (s, _) = call(&app, "PATCH", "/release/cleanup-jobs/1/enabled", Some(json!({ "enabled": false }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/release/cleanup-jobs/1", None).await.1["enabled"], false);
    let (s, _) = call(&app, "PATCH", "/release/cleanup-jobs/999/enabled", Some(json!({ "enabled": true }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, _) = call(&app, "POST", "/release/cleanup-jobs/1/run", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let j = st.releases.cleanup_job(1).unwrap();
    assert!(!j.last_run.is_empty());
    assert_eq!(j.last_run_status, "SUCCESS");
    let data: Value = serde_json::from_str(&j.last_run_data).unwrap();
    assert_eq!(
        (data["older_than_hours"].as_i64(), data["indexers"].clone(), data["statuses"].clone()),
        (Some(168), json!(["btn", "ptp"]), json!(["PUSH_ERROR"]))
    );
    let (s, _) = call(&app, "POST", "/release/cleanup-jobs/999/run", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, _) = call(&app, "DELETE", "/release/cleanup-jobs/1", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(call(&app, "GET", "/release/cleanup-jobs", None).await.1.as_array().unwrap().is_empty());
    let (s, _) = call(&app, "DELETE", "/release/cleanup-jobs/999", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// cleanup_test.go: a run with only unknown statuses deletes nothing; known ones are kept, unknown ones dropped;
/// no statuses deletes by age alone.
#[tokio::test]
async fn cleanup_runs_check_their_statuses() {
    let (_, st) = lantern("cleanup-run").await;
    push(&st, "Old.Show.S01E01.1080p.WEB-DL-GRP", 0);
    let job = |statuses: &str| CleanupJob { name: "test".into(), older_than: 24, statuses: statuses.into(), ..CleanupJob::default() };
    let mut j = job("LEGACY_UNKNOWN");
    st.releases.run_cleanup_job(&mut j);
    assert_eq!(j.last_run_status, "ERROR");
    assert!(j.last_run_data.contains("no valid statuses"), "{}", j.last_run_data);
    let mut j = job("PENDING, LEGACY_UNKNOWN");
    st.releases.run_cleanup_job(&mut j);
    assert_eq!(j.last_run_status, "SUCCESS");
    let data: Value = serde_json::from_str(&j.last_run_data).unwrap();
    assert_eq!(data["statuses"], json!(["PENDING"]));
    let mut j = job("");
    st.releases.run_cleanup_job(&mut j);
    assert_eq!(j.last_run_status, "SUCCESS");
    assert!(serde_json::from_str::<Value>(&j.last_run_data).unwrap()["statuses"].is_null());
    // the release is younger than a day, so every run kept it
    assert_eq!(st.releases.find_history(&Default::default()).unwrap()["count"], 1);
}
