//! Prowlarr's v1 indexer, search, history, stats and status API as Lantern answers it, against Prowlarr's own
//! answers (`tests/fixtures/upstream/prowlarr/indexer_*.json`, `history*.json`, `indexerstats.json`,
//! `indexerstatus.json`, built from Prowlarr v2.7.0.5723's resource classes): the same fields, types, defaults,
//! status codes and texts. Also the tag details, the indexer health checks and the live updates (SignalR). A small
//! private tracker (tests/support/tracker.rs) and a Newznab API served here stand in for the sites; nothing reaches
//! the internet.

use axum::body::Body;
use axum::extract::RawQuery;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use kit_server_core::{p, Db};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

#[path = "support/tracker.rs"]
mod tracker;
use tracker::start_tracker;

/// The API key the tests' Lantern has.
const KEY: &str = "test-key";

/// A fresh Lantern with the test definitions.
async fn lantern(name: &str) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-pix-{name}-{}", std::process::id()));
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

/// One request: status, body as JSON (`Null` when it is not JSON), headers.
async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value, HeaderMap) {
    let r = app.clone().oneshot(req).await.unwrap();
    let (s, h) = (r.status(), r.headers().clone());
    let b = axum::body::to_bytes(r.into_body(), 1 << 26).await.unwrap();
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null), h)
}

/// A request with the API key, as Prowlarr's web app sends it.
fn api(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", KEY).header(header::HOST, "lantern.test:9696");
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// Prowlarr's answer `name`.
fn upstream(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/upstream/prowlarr/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// The shape of an answer: every field's name and type (an array by its first item), the fields in `skip` left out.
fn shape(v: &Value, skip: &[&str]) -> Value {
    match v {
        Value::Object(m) => {
            json!(m
                .iter()
                .filter(|(k, _)| !skip.contains(&k.as_str()))
                .map(|(k, x)| (k.clone(), shape(x, skip)))
                .collect::<BTreeMap<_, _>>())
        }
        Value::Array(a) => match a.first() {
            Some(x) => json!([shape(x, skip)]),
            None => json!([]),
        },
        Value::Null => json!("null"),
        Value::Bool(_) => json!("bool"),
        Value::Number(_) => json!("number"),
        Value::String(_) => json!("string"),
    }
}

/// The resource of the test tracker, ready to add, from the schema.
async fn tracker_template(app: &Router) -> Value {
    let (s, list, _) = call(app, api("GET", "/api/v1/indexer/schema", None)).await;
    assert_eq!(s, StatusCode::OK);
    list.as_array().unwrap().iter().find(|t| t["definitionName"] == "lantern-test").cloned().unwrap()
}

/// Sets field `name` of a resource.
fn set_field(r: &mut Value, name: &str, value: Value) {
    let f = r["fields"].as_array_mut().unwrap().iter_mut().find(|f| f["name"] == name).unwrap_or_else(|| panic!("no field {name}"));
    f["value"] = value;
}

/// Field `name` of a resource.
fn field<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap_or_else(|| panic!("no field {name} in {r}"))
}

/// The test tracker at `site`, signed in, ready to add.
async fn tracker_body(app: &Router, site: &str, name: &str) -> Value {
    let mut t = tracker_template(app).await;
    t["name"] = json!(name);
    t["appProfileId"] = json!(1);
    set_field(&mut t, "baseUrl", json!(site));
    set_field(&mut t, "username", json!("me"));
    set_field(&mut t, "password", json!("pw"));
    t.as_object_mut().unwrap().remove("presets");
    t
}

/// A Newznab API: `t=caps` gives Prowlarr's caps fixture, a search gives one release; with key `nz-key` only.
async fn start_newznab() -> String {
    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/"><channel>
<item><title>Show.S01E01.1080p.WEB-DL-GRP</title><guid>https://nz.test/details/1</guid><link>https://nz.test/get/1.nzb</link>
<pubDate>Wed, 04 Nov 2026 10:30:00 +0000</pubDate><size>1000</size><newznab:attr name="category" value="5040"/></item></channel></rss>"#;
    let caps = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/prowlarr/newznab_caps.xml"))
        .unwrap();
    let app = Router::new().route(
        "/api",
        get(move |RawQuery(q): RawQuery| {
            let caps = caps.clone();
            async move {
                let q = q.unwrap_or_default();
                if !q.contains("apikey=nz-key") {
                    return (StatusCode::OK, r#"<error code="100" description="Incorrect user credentials"/>"#.to_string());
                }
                (StatusCode::OK, if q.contains("t=caps") { caps } else { RSS.to_string() })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn the_schema_lists_definitions_and_presets_as_prowlarr_does() {
    let (app, _) = lantern("schema").await;
    let (s, list, _) = call(&app, api("GET", "/api/v1/indexer/schema", None)).await;
    assert_eq!(s, StatusCode::OK);
    let list = list.as_array().unwrap();

    // the Cardigann definition: Prowlarr's fields, in its order, with its own settings after the standard ones
    let t = list.iter().find(|t| t["definitionName"] == "lantern-test").unwrap();
    let want = upstream("indexer_cardigann");
    let names = |r: &Value| r["fields"].as_array().unwrap().iter().map(|f| f["name"].clone()).collect::<Vec<_>>();
    assert_eq!(names(t), names(&want));
    for f in want["fields"].as_array().unwrap() {
        let name = f["name"].as_str().unwrap();
        let mut got = field(t, name).clone();
        let mut f = f.clone();
        // the values are the template's (checked below), not the saved indexer's
        got.as_object_mut().unwrap().remove("value");
        f.as_object_mut().unwrap().remove("value");
        assert_eq!(got, f, "field {name}");
    }
    assert_eq!(field(t, "definitionFile")["value"], "lantern-test");
    assert_eq!(field(t, "sort")["value"], 0, "a select's value is the place of the default among the sorted options");
    assert_eq!(field(t, "freeleech")["value"], false);
    assert_eq!(t["capabilities"], want["capabilities"]);
    assert_eq!(t["presets"], json!([]));
    assert!(t.get("id").is_none(), "an id of 0 is left out");
    for k in ["implementation", "configContract", "protocol", "privacy", "language", "infoLink", "priority", "enable", "redirect"] {
        assert_eq!(t[k], want[k], "{k}");
    }
    assert!(t.get("encoding").is_none(), "a definition's encoding is only set once it is saved (MapCardigannDefinition)");

    // a Newznab preset: Prowlarr's NZBgeek, with its categories and Redirect on
    let geek = list.iter().find(|t| t["name"] == "NZBgeek").unwrap();
    assert_eq!(geek, &upstream("indexer_schema_newznab"));
    // the generic entries and the Torznab ones are there too
    for name in ["Generic Newznab", "Generic Torznab"] {
        assert!(list.iter().any(|t| t["name"] == name), "{name}");
    }

    // the standard categories
    let (s, cats, _) = call(&app, api("GET", "/api/v1/indexer/categories", None)).await;
    assert_eq!(s, StatusCode::OK);
    let tops: Vec<_> = cats.as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(tops, ["Console", "Movies", "Audio", "PC", "TV", "XXX", "Books", "Other", "Other"], "with ZedOther last");
    assert_eq!(cats[1]["subCategories"][0], json!({ "id": 2010, "name": "Movies/Foreign", "subCategories": [] }));

    // getUrls offers the definition's addresses
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer/action/getUrls", Some(t.clone()))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({ "options": [{ "value": "http://127.0.0.1:1/", "name": "http://127.0.0.1:1/" }] })));
}

#[tokio::test]
async fn indexers_are_added_checked_changed_and_removed_as_in_prowlarr() {
    let site = start_tracker().await;
    let (app, _) = lantern("crud").await;
    let body = tracker_body(&app, &site, "Lantern Test Tracker").await;

    // the checks Prowlarr makes before saving
    let mut nameless = body.clone();
    nameless["name"] = json!("");
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer", Some(nameless))).await;
    assert_eq!((s, v), (StatusCode::BAD_REQUEST, upstream("indexer_invalid")));
    let mut bad = body.clone();
    bad["priority"] = json!(0);
    bad["appProfileId"] = json!(7);
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer", Some(bad))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let msgs: Vec<_> = v.as_array().unwrap().iter().map(|f| (f["propertyName"].clone(), f["errorMessage"].clone())).collect();
    assert_eq!(
        msgs,
        [
            (json!("AppProfileId"), json!("App Profile does not exist")),
            (json!("Priority"), json!("'Priority' must be between 1 and 50. You entered 0.")),
        ]
    );
    // a site that won't sign in fails its test
    let mut wrong = body.clone();
    set_field(&mut wrong, "password", json!("nope"));
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer", Some(wrong))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v[0]["errorMessage"].as_str().unwrap().starts_with("Unable to connect to indexer."), "{v}");

    // saved: 201, where it is, Prowlarr's resource (the password masked, a Lantern choice)
    let (s, saved, h) = call(&app, api("POST", "/api/v1/indexer", Some(body.clone()))).await;
    assert_eq!(s, StatusCode::CREATED, "{saved}");
    assert_eq!(h.get(header::LOCATION).unwrap(), "/api/v1/indexer/1");
    let want = upstream("indexer_cardigann");
    assert_eq!(shape(&saved, &[]), shape(&want, &[]));
    for k in [
        "id",
        "name",
        "implementation",
        "implementationName",
        "configContract",
        "definitionName",
        "description",
        "infoLink",
        "language",
        "encoding",
        "enable",
        "redirect",
        "supportsRss",
        "supportsSearch",
        "supportsRedirect",
        "supportsPagination",
        "appProfileId",
        "protocol",
        "privacy",
        "capabilities",
        "priority",
        "downloadClientId",
        "sortName",
        "tags",
        "indexerUrls",
        "legacyUrls",
    ] {
        assert_eq!(saved[k], want[k], "{k}");
    }
    assert_eq!(field(&saved, "password")["value"], "********");
    assert_eq!(field(&saved, "username")["value"], "me");
    assert_eq!(field(&saved, "baseUrl")["value"], site.as_str());

    // the same name again
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer", Some(body.clone()))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v[0]["errorMessage"], "Should be unique");

    // changed: 202; the masked password keeps the saved one (the test still signs in)
    let mut change = saved.clone();
    change["priority"] = json!(10);
    let (s, v, h) = call(&app, api("PUT", "/api/v1/indexer/1", Some(change))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(h.get(header::LOCATION).unwrap(), "/api/v1/indexer/1");
    assert_eq!(v["priority"], 10);
    let (_, one, _) = call(&app, api("GET", "/api/v1/indexer/1", None)).await;
    assert_eq!(one["priority"], 10);
    // forceSave lets a site that fails its test through only when the failures are warnings
    let mut off = one.clone();
    set_field(&mut off, "password", json!("nope"));
    let (s, _, _) = call(&app, api("PUT", "/api/v1/indexer/1?forceSave=true", Some(off))).await;
    assert_eq!(s, StatusCode::ACCEPTED);

    // not there
    let (s, v, _) = call(&app, api("GET", "/api/v1/indexer/99", None)).await;
    assert_eq!((s, v["message"].clone()), (StatusCode::NOT_FOUND, json!("IndexerDefinition with ID 99 does not exist")));

    // Usenet indexers must redirect
    let nz = start_newznab().await;
    let (_, list, _) = call(&app, api("GET", "/api/v1/indexer/schema", None)).await;
    let mut generic = list.as_array().unwrap().iter().find(|t| t["name"] == "Generic Newznab").cloned().unwrap();
    generic["name"] = json!("My Newznab");
    generic["appProfileId"] = json!(1);
    generic.as_object_mut().unwrap().remove("presets");
    set_field(&mut generic, "baseUrl", json!(nz));
    set_field(&mut generic, "apiKey", json!("nz-key"));
    let mut no_redirect = generic.clone();
    no_redirect["redirect"] = json!(false);
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer", Some(no_redirect))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v[0]["errorMessage"], "Redirect must be enabled for Usenet indexers");
    // a wrong key fails the test, the right one saves it with the API's categories
    let mut wrong = generic.clone();
    set_field(&mut wrong, "apiKey", json!("bad"));
    let (s, _, _) = call(&app, api("POST", "/api/v1/indexer", Some(wrong))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, nzi, _) = call(&app, api("POST", "/api/v1/indexer", Some(generic))).await;
    assert_eq!(s, StatusCode::CREATED, "{nzi}");
    assert_eq!((nzi["protocol"].clone(), nzi["implementation"].clone()), (json!("usenet"), json!("Newznab")));
    assert_eq!(field(&nzi, "apiKey")["value"], "********");
    assert_eq!(nzi["capabilities"]["limitsMax"], 60);
    let nz_id = nzi["id"].as_i64().unwrap();

    // bulk: a priority for both, then testall, then remove one
    let (s, v, _) = call(&app, api("PUT", "/api/v1/indexer/bulk", Some(json!({ "ids": [1, nz_id], "priority": 5 })))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v.as_array().unwrap().iter().map(|i| i["priority"].clone()).collect::<Vec<_>>(), [json!(5), json!(5)]);
    let mut fixed = v[0].clone();
    set_field(&mut fixed, "password", json!("pw"));
    let (s, _, _) = call(&app, api("PUT", "/api/v1/indexer/1", Some(fixed))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer/testall", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v,
        json!([{ "id": 1, "isValid": true, "validationFailures": [] }, { "id": nz_id, "isValid": true, "validationFailures": [] }])
    );
    let (s, v, _) = call(&app, api("DELETE", "/api/v1/indexer/bulk", Some(json!({ "ids": [nz_id] })))).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    let (_, list, _) = call(&app, api("GET", "/api/v1/indexer", None)).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (s, v, _) = call(&app, api("DELETE", "/api/v1/indexer/1", None)).await;
    assert_eq!((s, v), (StatusCode::OK, json!({})));
    let (_, list, _) = call(&app, api("GET", "/api/v1/indexer", None)).await;
    assert_eq!(list, json!([]));
}

#[tokio::test]
async fn searches_grabs_history_and_stats_answer_as_prowlarr_does() {
    let site = start_tracker().await;
    let (app, st) = lantern("search").await;
    let (s, v, _) = call(&app, api("POST", "/api/v1/indexer", Some(tracker_body(&app, &site, "Lantern Test Tracker").await))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // a search: the release, with its flags and categories
    let (s, found, _) =
        call(&app, api("GET", "/api/v1/search?query=Show&type=search&indexerIds=1&categories=5000&limit=100&offset=0", None)).await;
    assert_eq!(s, StatusCode::OK, "{found}");
    let r = &found.as_array().unwrap()[0];
    assert_eq!(r["title"], "Show.Name.S01E02.1080p.WEB-DL-GRP");
    assert_eq!(r["indexerFlags"], json!(["freeleech"]));
    assert_eq!(
        (r["indexerId"].clone(), r["indexer"].clone(), r["protocol"].clone()),
        (json!(1), json!("Lantern Test Tracker"), json!("torrent"))
    );

    // a grab: no download client yet, and a release that isn't kept
    let (s, v, _) = call(&app, api("POST", "/api/v1/search", Some(r.clone()))).await;
    assert_eq!((s, v["message"].clone()), (StatusCode::INTERNAL_SERVER_ERROR, json!("Torrent Download client isn't configured yet")));
    let (s, v, _) = call(&app, api("POST", "/api/v1/search", Some(json!({ "indexerId": 1, "guid": "gone" })))).await;
    assert_eq!((s, v["message"].clone()), (StatusCode::NOT_FOUND, json!("Couldn't find requested release in cache, try searching again")));
    let (s, v, _) = call(&app, api("POST", "/api/v1/search", Some(json!({ "indexerId": 0 })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");

    // the search in the history, paged, since a date and by indexer
    let (s, h, _) = call(&app, api("GET", "/api/v1/history?page=1&pageSize=10&sortKey=date&sortDirection=descending", None)).await;
    assert_eq!(s, StatusCode::OK, "{h}");
    let want = upstream("history");
    assert_eq!(shape(&h, &[]), shape(&want, &[]), "{h}");
    let rec = &h["records"][0];
    assert_eq!(
        (rec["eventType"].clone(), rec["successful"].clone(), rec["indexerId"].clone()),
        (json!("indexerQuery"), json!(true), json!(1))
    );
    for k in ["query", "queryType", "categories", "source", "queryResults", "cached", "limit", "offset"] {
        assert_eq!(rec["data"][k], want["records"][0]["data"][k], "{k}");
    }
    let (_, only_grabs, _) = call(&app, api("GET", "/api/v1/history?page=1&pageSize=10&eventType=1", None)).await;
    assert_eq!(only_grabs["totalRecords"], 0);
    let (s, since, _) = call(&app, api("GET", "/api/v1/history/since?date=2000-01-01T00:00:00Z", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(shape(&since, &[]), shape(&upstream("history_since"), &[]));
    let (_, mine, _) = call(&app, api("GET", "/api/v1/history/indexer?indexerId=1", None)).await;
    assert_eq!(mine.as_array().unwrap().len(), 1, "{mine}");
    let (_, other, _) = call(&app, api("GET", "/api/v1/history/indexer?indexerId=2", None)).await;
    assert_eq!(other, json!([]));

    // the stats
    let (s, stats, _) = call(&app, api("GET", "/api/v1/indexerstats", None)).await;
    assert_eq!(s, StatusCode::OK);
    let want = upstream("indexerstats");
    assert_eq!(shape(&stats, &[]), shape(&want, &[]), "{stats}");
    for k in ["indexerId", "indexerName", "numberOfQueries", "numberOfGrabs", "numberOfFailedQueries"] {
        assert_eq!(stats["indexers"][0][k], want["indexers"][0][k], "{k}");
    }
    assert_eq!(stats["userAgents"][0]["userAgent"], "Prowlarr");

    // with a Prowlarr download client (a blackhole in the data folder) the grab goes there
    let hole = st.data_dir.join("hole");
    std::fs::create_dir_all(&hole).unwrap();
    let (_, schema, _) = call(&app, api("GET", "/api/v1/downloadclient/schema", None)).await;
    let mut bh = schema.as_array().unwrap().iter().find(|t| t["implementation"] == "TorrentBlackhole").unwrap().clone();
    bh["name"] = json!("Hole");
    bh["enable"] = json!(true);
    set_field(&mut bh, "torrentFolder", json!(hole.display().to_string()));
    let (s, v, _) = call(&app, api("POST", "/api/v1/downloadclient", Some(bh))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, v, _) = call(&app, api("POST", "/api/v1/search", Some(r.clone()))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(hole.join("Show.Name.S01E02.1080p.WEB-DL-GRP.torrent").exists());

    // a bulk grab (ReleaseController.DownloadBulk): the releases sent back, ones no longer kept skipped, 400 when
    // none could be sent
    std::fs::remove_file(hole.join("Show.Name.S01E02.1080p.WEB-DL-GRP.torrent")).unwrap();
    let gone = json!({ "indexerId": 1, "guid": "gone" });
    let (s, v, _) = call(&app, api("POST", "/api/v1/search/bulk", Some(json!([r.clone(), gone.clone()])))).await;
    assert_eq!((s, v.clone()), (StatusCode::OK, json!([r.clone()])), "{v}");
    assert!(hole.join("Show.Name.S01E02.1080p.WEB-DL-GRP.torrent").exists());
    let (s, v, _) = call(&app, api("POST", "/api/v1/search/bulk", Some(json!([gone])))).await;
    assert_eq!((s, v["message"].clone()), (StatusCode::BAD_REQUEST, json!("Failed to grab any release")), "{v}");
}

#[tokio::test]
async fn sites_on_hold_show_in_status_and_health() {
    let (app, st) = lantern("status").await;
    let down = st
        .indexers
        .add(
            &json!({ "definition": "lantern-test", "base_url": "http://127.0.0.1:9/", "settings": { "username": "me", "password": "pw" } }),
        )
        .unwrap();
    let (_, none, _) = call(&app, api("GET", "/api/v1/indexerstatus", None)).await;
    assert_eq!(none, json!([]));
    let _ = call(&app, api("GET", &format!("/api/v1/search?query=x&indexerIds={down}"), None)).await;
    let (s, v, _) = call(&app, api("GET", "/api/v1/indexerstatus", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(shape(&v, &[]), shape(&upstream("indexerstatus"), &[]), "{v}");
    assert_eq!(v[0]["indexerId"], down);

    // the health checks: all indexers down, a definition that is gone, a VIP that ended
    let gone = st.indexers.add(&json!({ "definition": "lantern-test", "name": "Gone Site", "base_url": "http://127.0.0.1:9/" })).unwrap();
    st.db.execute("UPDATE indexers SET definition='no-such-site' WHERE id=?", p![gone]).unwrap();
    st.indexers
        .add(&json!({ "kind": "newznab", "name": "Old VIP", "base_url": "http://127.0.0.1:9", "settings": { "apikey": "k", "vip_expiration": "2020-01-01" } }))
        .unwrap();
    let (_, health, _) = call(&app, api("GET", "/api/v1/health", None)).await;
    let find = |source: &str| health.as_array().unwrap().iter().find(|h| h["source"] == source).cloned();
    let status = find("IndexerStatusCheck").unwrap_or_else(|| panic!("{health}"));
    assert_eq!(status["type"], "warning");
    assert!(status["message"].as_str().unwrap().contains("Lantern Test Tracker"), "{status}");
    let gone = find("IndexerNoDefinitionCheck").unwrap_or_else(|| panic!("{health}"));
    assert_eq!(gone["type"], "error");
    assert!(gone["message"].as_str().unwrap().contains("Gone Site"), "{gone}");
    let vip = find("IndexerVIPExpiredCheck").unwrap_or_else(|| panic!("{health}"));
    assert!(vip["message"].as_str().unwrap().contains("Old VIP"), "{vip}");
    assert!(find("IndexerProxyStatusCheck").is_none(), "no proxies, nothing to say");
}

#[tokio::test]
async fn tag_details_name_the_proxies_and_apps_using_them() {
    let (app, st) = lantern("tags").await;
    let (s, t, _) = call(&app, api("POST", "/api/v1/tag", Some(json!({ "label": "shared" })))).await;
    assert_eq!(s, StatusCode::CREATED);
    let id = t["id"].as_i64().unwrap();
    st.db.execute("INSERT INTO indexer_proxies (name, implementation, tags) VALUES ('P', 'Http', ?)", p![format!("[{id}]")]).unwrap();
    let app_body = json!({
        "name": "Sonarr", "implementation": "Sonarr", "configContract": "SonarrSettings", "syncLevel": "disabled", "tags": [id],
        "fields": [{ "name": "prowlarrUrl", "value": "http://lantern:9696" }, { "name": "baseUrl", "value": "http://127.0.0.1:9" }, { "name": "apiKey", "value": "k" }],
    });
    let (s, v, _) = call(&app, api("POST", "/api/v1/applications", Some(app_body))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (_, detail, _) = call(&app, api("GET", &format!("/api/v1/tag/detail/{id}"), None)).await;
    assert_eq!((detail["indexerProxyIds"].clone(), detail["applicationIds"].clone()), (json!([1]), json!([v["id"]])), "{detail}");
    // in use, so it stays
    let (s, v, _) = call(&app, api("DELETE", &format!("/api/v1/tag/{id}"), None)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
}

/// Sends one masked WebSocket text frame.
async fn ws_send(sock: &mut tokio::net::TcpStream, text: &str) {
    let b = text.as_bytes();
    assert!(b.len() < 126);
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

#[tokio::test]
async fn live_updates_come_over_signalr() {
    let site = start_tracker().await;
    let (app, _) = lantern("signalr").await;

    // negotiate: a connection and the WebSockets transport; without a key, nothing
    let (s, v, _) = call(&app, api("POST", "/signalr/messages/negotiate?negotiateVersion=1", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["availableTransports"], json!([{ "transport": "WebSockets", "transferFormats": ["Text", "Binary"] }]));
    assert_eq!(v["negotiateVersion"], 1);
    let bare = Request::builder().method("POST").uri("/signalr/messages/negotiate").body(Body::empty()).unwrap();
    assert_eq!(call(&app, bare).await.0, StatusCode::UNAUTHORIZED);

    // the hub: handshake, version, then an indexer added
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
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    ws_send(&mut sock, "{\"protocol\":\"json\",\"version\":1}\u{1e}").await;
    assert_eq!(ws_read(&mut sock).await, "{}\u{1e}");
    let version: Value = serde_json::from_str(ws_read(&mut sock).await.trim_end_matches('\u{1e}')).unwrap();
    assert_eq!((version["type"].clone(), version["target"].clone()), (json!(1), json!("receiveMessage")));
    assert_eq!(version["arguments"][0]["name"], "version");

    let (s, _, _) = call(&app, api("POST", "/api/v1/indexer", Some(tracker_body(&app, &site, "Live").await))).await;
    assert_eq!(s, StatusCode::CREATED);
    let msg: Value = loop {
        let m: Value = serde_json::from_str(ws_read(&mut sock).await.trim_end_matches('\u{1e}')).unwrap();
        if m["arguments"][0]["name"] == "indexer" {
            break m;
        }
    };
    let body = &msg["arguments"][0]["body"];
    assert_eq!((body["action"].clone(), body["resource"]["name"].clone()), (json!("updated"), json!("Live")));
}
