//! Parity with Prowlarr v2.7.0.5723's Newznab/Torznab answers and indexer runtime: Lantern's `t=caps` and search
//! documents compared byte for byte with golden files derived from Prowlarr's serializers
//! (tests/fixtures/upstream/prowlarr/), and the endpoints `/{id}/api` and `/{id}/download` answering as
//! `NewznabController` does: the API key, parameter errors, the test indexer 0, turned-off indexers, holds after
//! failures, query and grab limits, redirects, magnet links first, and the app each request came from.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use chrono::TimeZone;
use serde_json::{json, Value};
use server::indexers::capabilities::Capabilities;
use server::indexers::definition::Definition;
use server::indexers::release::{Protocol, Release};
use server::indexers::{dedupe, nabxml, newznab_results};
use std::path::PathBuf;
use tower::ServiceExt;

const KEY: &str = "test-key";

#[path = "support/tracker.rs"]
mod tracker;
use tracker::{start_tracker, TORRENT};

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/prowlarr").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn date() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2026, 11, 4, 10, 30, 0).unwrap()
}

#[test]
fn caps_of_the_test_indexer_match_prowlarr() {
    assert_eq!(Capabilities::test_indexer().to_xml("Prowlarr"), fixture("caps-indexer-0.xml"));
}

#[test]
fn caps_of_a_cardigann_definition_match_prowlarr() {
    let yaml =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definitions/lantern-test.yml")).unwrap();
    let def = Definition::parse(&yaml).unwrap();
    assert_eq!(Capabilities::from_definition(&def).unwrap().to_xml("Prowlarr"), fixture("caps-cardigann-lantern-test.xml"));
}

#[test]
fn search_answers_match_prowlarr() {
    assert_eq!(newznab_results::test_results("Prowlarr", date()), fixture("search-indexer-0.xml"));
    let first = Release {
        title: "Show.Name.S01E02.1080p.WEB-DL-GRP".into(),
        guid: "https://tracker.test/details.php?id=11".into(),
        link: "http://prowlarr.test:9696/3/download?apikey=k&link=x&file=Show.Name.S01E02.1080p.WEB-DL-GRP".into(),
        details: "https://tracker.test/details.php?id=11".into(),
        infohash: "0123456789abcdef0123456789abcdef01234567".into(),
        size: Some(1610612736),
        files: Some(2),
        grabs: Some(40),
        seeders: Some(12),
        leechers: Some(3),
        date: Some(date()),
        categories: vec![5040, 100002],
        imdb: Some(111161),
        tvdb: Some(123),
        genre: "Drama, Comedy".into(),
        poster: "https://img.test/p.jpg".into(),
        download_factor: Some(0.0),
        upload_factor: Some(1.0),
        minimum_ratio: Some(1.0),
        minimum_seed_time: Some(172800),
        protocol: Protocol::Torrent,
        indexer_id: 3,
        indexer: "Test Tracker".into(),
        indexer_privacy: "private".into(),
        flags: vec!["internal".into(), "freeleech".into()],
        ..Release::default()
    };
    let second = Release {
        title: "A & B <Live> \"2026\"".into(),
        description: "Internal & scene".into(),
        guid: "magnet:?xt=urn:btih:abc&dn=A".into(),
        magnet: "http://prowlarr.test:9696/3/download?apikey=k&link=m&file=A+%26+B".into(),
        date: Some(date()),
        rage: Some(7),
        trakt: Some(8),
        year: Some(2026),
        artist: "A".into(),
        album: "B".into(),
        languages: vec!["English".into()],
        subs: vec!["French".into()],
        download_factor: Some(0.5),
        upload_factor: Some(2.0),
        protocol: Protocol::Torrent,
        indexer_id: 3,
        indexer: "Test Tracker".into(),
        indexer_privacy: "private".into(),
        flags: vec!["halfleech".into(), "doubleupload".into()],
        ..Release::default()
    };
    let got = newznab_results::to_xml("Prowlarr", &[first, second], Some(Protocol::Torrent), false, date());
    assert_eq!(got, fixture("search-torrent.xml"));
}

#[test]
fn errors_match_prowlarr() {
    for (code, text) in [
        (200, "Missing parameter (t)"),
        (201, "Incorrect parameter (imdbid)"),
        (202, "No such function (foo)"),
        (410, "Indexer is disabled"),
    ] {
        assert_eq!(
            nabxml::error_xml(code, text),
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<error code=\"{code}\" description=\"{text}\" />")
        );
    }
}

#[test]
fn a_release_two_indexers_found_is_kept_from_the_higher_priority() {
    let r = |guid: &str, indexer: i64, priority: i64| Release {
        guid: guid.into(),
        indexer_id: indexer,
        indexer_priority: priority,
        ..Release::default()
    };
    let kept = dedupe(vec![r("a", 1, 25), r("b", 1, 25), r("a", 2, 10), r("a", 3, 10)]);
    assert_eq!(kept.iter().map(|x| (x.guid.as_str(), x.indexer_id)).collect::<Vec<_>>(), vec![("a", 2), ("b", 1)]);
}

// ---- the endpoints ----

async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-ixp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let web = dir.join("static");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
    let defs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definitions").display().to_string();
    let st = server::AppState::open(&dir.join("data"), web, move |n| match n {
        "APP_API_KEY" => Some(KEY.to_string()),
        "APP_DEFINITIONS_DIR" => Some(defs.clone()),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some("correct horse battery".into())).unwrap();
    (server::app(st.clone()), st)
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let r = app.clone().oneshot(req).await.unwrap();
    let (s, h) = (r.status(), r.headers().clone());
    let b = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
    (s, String::from_utf8_lossy(&b).into_owned(), h)
}

fn get(path: &str) -> Request<Body> {
    Request::get(path)
        .header(header::HOST, "lantern.test:9696")
        .header(header::USER_AGENT, "Sonarr/4.0.11 (ubuntu 22.04)")
        .body(Body::empty())
        .unwrap()
}

fn add(st: &server::AppState, body: Value) -> i64 {
    st.indexers.add(&body).unwrap()
}

#[tokio::test]
async fn the_api_key_parameters_and_the_test_indexer_answer_like_prowlarr() {
    let (app, st) = lantern("params").await;
    // no key or a wrong one: 401 with nothing in it; the key also works as a header or a bearer token
    let (s, body, _) = call(&app, get("/0/api?t=caps&apikey=wrong")).await;
    assert_eq!((s, body.as_str()), (StatusCode::UNAUTHORIZED, ""));
    let (s, _, _) = call(&app, get("/0/api?t=caps")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let r = Request::get("/0/api?t=caps").header("Authorization", format!("Bearer {KEY}")).body(Body::empty()).unwrap();
    assert_eq!(call(&app, r).await.0, StatusCode::OK);
    let r = Request::get("/0/api?t=caps").header("X-Api-Key", KEY).body(Body::empty()).unwrap();
    assert_eq!(call(&app, r).await.0, StatusCode::OK);

    // the parameters
    let (s, body, h) = call(&app, get(&format!("/0/api?apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/rss+xml");
    assert_eq!(body, nabxml::error_xml(200, "Missing parameter (t)"));
    for bad in ["abc", "tt0", "0"] {
        let (s, body, _) = call(&app, get(&format!("/0/api?t=movie&imdbid={bad}&apikey={KEY}"))).await;
        assert_eq!((s, body), (StatusCode::BAD_REQUEST, nabxml::error_xml(201, "Incorrect parameter (imdbid)")), "{bad}");
    }
    let (s, body, _) = call(&app, get(&format!("/0/api?t=movie&tmdbid=x&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["errors"]["tmdbid"][0], "The value 'x' is not valid for tmdbid.");

    // indexer 0 is Prowlarr's test indexer (with Lantern's name where Prowlarr writes its own)
    let name = st.app_name();
    let (s, caps, _) = call(&app, get(&format!("/0/api?t=caps&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(caps, fixture("caps-indexer-0.xml").replace("title=\"Prowlarr\"", &format!("title=\"{name}\"")));
    let (_, rss, _) = call(&app, get(&format!("/api/v1/indexer/0/newznab?t=tvsearch&q=x&apikey={KEY}"))).await;
    assert!(rss.contains("<title>Test Release</title>") && rss.contains(r#"<prowlarrindexer id="0" type="public" />"#), "{rss}");

    // an unknown indexer, a function nobody has
    let (s, body, _) = call(&app, get(&format!("/999/api?t=caps&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(body.contains("Indexer Not Found"), "{body}");
    let id = add(&st, json!({ "kind": "torznab", "name": "Remote", "base_url": "http://127.0.0.1:9", "settings": { "apikey": "k" } }));
    let (s, body, _) = call(&app, get(&format!("/{id}/api?t=foo&apikey={KEY}"))).await;
    assert_eq!((s, body), (StatusCode::BAD_REQUEST, nabxml::error_xml(202, "No such function (foo)")));

    // a turned-off indexer is gone
    st.indexers.update(id, &json!({ "enabled": false })).unwrap();
    let (s, body, _) = call(&app, get(&format!("/{id}/api?t=caps&apikey={KEY}"))).await;
    assert_eq!((s, body), (StatusCode::GONE, nabxml::error_xml(410, "Indexer is disabled")));
    let (s, _, _) = call(&app, get(&format!("/{id}/download?link=x&file=y&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::GONE);

    // a limit of zero can't be saved
    let e = st.indexers.update(id, &json!({ "settings": { "baseSettings.queryLimit": "0" } })).unwrap_err();
    assert_eq!(e.detail, "Query Limit: Should be greater than zero");
}

#[tokio::test]
async fn searches_follow_prowlarr_limits_holds_and_paging() {
    let site = start_tracker().await;
    let (app, st) = lantern("limits").await;
    let id = add(
        &st,
        json!({ "definition": "lantern-test", "base_url": site, "settings": { "username": "me", "password": "pw", "baseSettings.queryLimit": "2", "baseSettings.limitsUnit": "1" } }),
    );

    // the app's searches: its name is kept in the history; Cardigann sites have no second page
    let (s, rss, _) = call(&app, get(&format!("/{id}/api?t=tvsearch&q=Show%20Name&season=1&ep=2&cat=5000&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::OK, "{rss}");
    assert!(
        rss.contains("<title>Show.Name.S01E02.1080p.WEB-DL-GRP</title>")
            && rss.contains(r#"<torznab:attr name="tag" value="freeleech" />"#),
        "{rss}"
    );
    assert!(
        rss.contains(&format!("<title>{}</title>", st.app_name()))
            && rss.contains(r#"type="private">Lantern Test Tracker</prowlarrindexer>"#),
        "{rss}"
    );
    let (_, rss, _) = call(&app, get(&format!("/{id}/api?t=search&q=Show&offset=100&apikey={KEY}"))).await;
    assert!(rss.contains("<channel>") && !rss.contains("<item>"), "{rss}");
    let history = st.indexers.history(Some(id), 10).unwrap();
    assert_eq!(history.len(), 1, "the page past the first never reached the site: {history:?}");
    assert_eq!((history[0]["source"].as_str(), history[0]["kind"].as_str()), (Some("Sonarr"), Some("query")));

    // a parameter the site can't search by: nothing, without asking it
    let (_, rss, _) = call(&app, get(&format!("/{id}/api?t=movie&q=Movie&tmdbid=5&apikey={KEY}"))).await;
    assert!(!rss.contains("<item>"), "{rss}");
    assert_eq!(st.indexers.history(Some(id), 10).unwrap().len(), 1);

    // categories it doesn't have: nothing
    let (_, rss, _) = call(&app, get(&format!("/{id}/api?t=search&q=Show&cat=6000&apikey={KEY}"))).await;
    assert!(!rss.contains("<item>"), "{rss}");

    // two searches an hour: the third is refused until the first is an hour old
    let (s, _, _) = call(&app, get(&format!("/{id}/api?t=search&q=Movie&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, body, h) = call(&app, get(&format!("/{id}/api?t=search&q=Movie&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body, nabxml::error_xml(429, "User configurable Indexer Query Limit of 2 in last 1 hour(s) reached."));
    let wait: i64 = h.get(header::RETRY_AFTER).unwrap().to_str().unwrap().parse().unwrap();
    assert!((3590..=3600).contains(&wait), "{wait}");

    // a site that fails is put on hold; apps are told until when
    let down = add(
        &st,
        json!({ "definition": "lantern-test", "base_url": "http://127.0.0.1:9/", "settings": { "username": "me", "password": "pw" } }),
    );
    let (s, body, h) = call(&app, get(&format!("/{down}/api?t=search&q=x&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body.contains("code=\"429\" description=\"Indexer is disabled till ") && body.contains(" due to recent failures.\""), "{body}");
    let wait: i64 = h.get(header::RETRY_AFTER).unwrap().to_str().unwrap().parse().unwrap();
    assert!((55..=60).contains(&wait), "the first failure holds it a minute: {wait}");
    let ix = st.indexers.get(down).unwrap();
    assert_eq!(ix.failures, 1);
    assert!(!ix.last_error.is_empty());
    let (s, _, _) = call(&app, get(&format!("/{down}/api?t=caps&apikey={KEY}"))).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "on hold, even its caps");
}

#[tokio::test]
async fn downloads_follow_prowlarr_grab_limits_redirects_and_file_names() {
    let site = start_tracker().await;
    let (app, st) = lantern("grabs").await;
    let id = add(
        &st,
        json!({ "definition": "lantern-test", "base_url": site, "settings": { "username": "me", "password": "pw", "baseSettings.grabLimit": "1" } }),
    );
    let (_, rss, _) = call(&app, get(&format!("/{id}/api?t=search&q=Show&apikey={KEY}"))).await;
    let link = rss.split("<link>").nth(1).and_then(|l| l.split("</link>").next()).unwrap().replace("&amp;", "&");
    assert!(
        link.starts_with(&format!("http://lantern.test:9696/{id}/download?apikey={KEY}&link="))
            && link.ends_with("&file=Show.Name.S01E02.1080p.WEB-DL-GRP"),
        "{link}"
    );
    let path = link.trim_start_matches("http://lantern.test:9696");

    // the file, named as Prowlarr names it
    let (s, file, h) = call(&app, get(path)).await;
    assert_eq!(s, StatusCode::OK, "{file}");
    assert_eq!(file.as_bytes(), TORRENT);
    assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/x-bittorrent");
    assert_eq!(
        h.get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=Show.Name.S01E02.1080p.WEB-DL-GRP.torrent; filename*=UTF-8''Show.Name.S01E02.1080p.WEB-DL-GRP.torrent"
    );
    let grabs: Vec<Value> = st.indexers.history(Some(id), 10).unwrap().into_iter().filter(|h| h["kind"] == "grab").collect();
    assert_eq!((grabs.len(), grabs[0]["source"].as_str()), (1, Some("Sonarr")));

    // one grab a day
    let (s, body, h) = call(&app, get(path)).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body, nabxml::error_xml(429, "User configurable Indexer Grab Limit of 1 in last 24 hour(s) reached."));
    assert!(h.get(header::RETRY_AFTER).is_some());

    // a link without a file name, or one Lantern didn't make
    st.indexers.update(id, &json!({ "settings": { "baseSettings.grabLimit": "" } })).unwrap();
    let (s, body, _) = call(&app, get(&path[..path.find("&file=").unwrap()])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.contains("Invalid Prowlarr link"), "{body}");
    let (s, body, _) = call(&app, get(&path.replace("sig=", "sig=00"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.contains("Failed to normalize provided link"), "{body}");

    // a Usenet indexer always sends the app to the site; a Torznab one when set to
    let nzb = add(&st, json!({ "kind": "newznab", "name": "Usenet", "base_url": "http://nzb.test", "settings": { "apikey": "k" } }));
    let tz = add(
        &st,
        json!({ "kind": "torznab", "name": "Torznab", "base_url": "http://tz.test", "redirect": true, "settings": { "apikey": "k" } }),
    );
    for (ix, real) in [(nzb, "http://nzb.test/getnzb/1.nzb?apikey=k"), (tz, "http://tz.test/dl/1.torrent")] {
        let (l, sig) = st.signer.sign(ix, real);
        let (s, _, h) = call(&app, get(&format!("/api/v1/indexer/{ix}/download?apikey={KEY}&link={l}&sig={sig}&file=Some+Release"))).await;
        assert_eq!(s, StatusCode::MOVED_PERMANENTLY);
        assert_eq!(h.get(header::LOCATION).unwrap(), real);
        let h = st.indexers.history(Some(ix), 1).unwrap();
        assert_eq!((h[0]["kind"].as_str(), h[0]["query"].as_str()), (Some("grab"), Some("Some Release")));
    }
    // a Cardigann site can't redirect, even when set to
    st.indexers.update(id, &json!({ "redirect": true })).unwrap();
    assert_eq!(call(&app, get(path)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn magnet_links_go_through_lantern_and_can_come_first() {
    let (app, st) = lantern("magnet").await;
    let id = add(&st, json!({ "kind": "torznab", "name": "Remote", "base_url": "http://127.0.0.1:9", "settings": { "apikey": "k" } }));
    let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x";
    let (l, sig) = st.signer.sign(id, magnet);
    let (s, _, h) = call(&app, get(&format!("/{id}/download?apikey={KEY}&link={l}&sig={sig}&file=x"))).await;
    assert_eq!(s, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(h.get(header::LOCATION).unwrap(), magnet);
    // preferring magnet links: the item's link is the (proxied) magnet when there is one
    let r = Release {
        title: "x".into(),
        guid: "g".into(),
        link: "http://l/dl".into(),
        magnet: "http://l/magnet".into(),
        protocol: Protocol::Torrent,
        ..Release::default()
    };
    let first = newznab_results::to_xml("P", std::slice::from_ref(&r), Some(Protocol::Torrent), true, date());
    assert!(first.contains("<link>http://l/magnet</link>"), "{first}");
    let normal = newznab_results::to_xml("P", &[r], Some(Protocol::Torrent), false, date());
    assert!(normal.contains("<link>http://l/dl</link>"), "{normal}");
}
