//! Cloudflare solving with a real headless Chromium against a site that checks browsers the way Cloudflare does
//! (tests/support/challenge.rs).
//!
//! The browser is `LANTERN_TEST_CHROMIUM`, else the first one Lantern finds by itself, else Playwright's
//! headless shell; CI has Google Chrome.

#[path = "support/challenge.rs"]
mod challenge;

use server::indexers::http::{Fetch, Http};
use server::solver::{find_program, SolveRequest, Solver};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The browser for the test (see the module's words).
fn browser() -> PathBuf {
    if let Some(p) = find_program(std::env::var("LANTERN_TEST_CHROMIUM").ok().as_deref()) {
        return p;
    }
    let shell = std::fs::read_dir("/opt/pw-browsers")
        .ok()
        .and_then(|d| d.flatten().map(|e| e.path().join("chrome-headless-shell-linux64/chrome-headless-shell")).find(|p| p.is_file()));
    shell.expect("No Chromium for the solver test: set LANTERN_TEST_CHROMIUM to a Chromium, Chrome or headless shell program")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gets_past_a_check_keeps_the_cookies_and_answers_as_flaresolverr() {
    let (base, site) = challenge::start_site_on("127.0.0.1:0").await;
    let data = std::env::temp_dir().join(format!("lantern-solver-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let db = kit_server_core::SqliteDb::open(&data.join("app.db")).unwrap();
    server::solver::migrate(&db).unwrap();
    let solver = Solver::new(Some(browser()), &data, Some(db.clone()));

    // without a solver, the check is said in words
    let bare = Http::new("Site", 0.0, "UTF-8", None);
    let e = bare.fetch(Fetch::get(&format!("{base}/page"))).await.unwrap_err();
    assert!(e.contains("Cloudflare check") && e.contains("APP_CHROMIUM"), "{e}");

    // with one: the browser gets past the check once, Lantern's own requests reuse the cookie after
    let http = Http::new("Site", 0.0, "UTF-8", None).with_solver(Some(solver.clone()));
    let p = http.fetch(Fetch::get(&format!("{base}/page"))).await.expect("first page");
    assert!(p.text.contains("Show.S01E01"), "{}", p.text);
    let browser_hits = site.browser.load(Ordering::SeqCst);
    assert!(browser_hits >= 2, "the browser saw the check and then the page: {browser_hits}");
    assert_eq!(site.plain_through.load(Ordering::SeqCst), 1, "the retry after the check went through Lantern's own client");
    let host = "127.0.0.1";
    let kept = solver.clearance(host, "").expect("cookies kept for the site");
    assert!(kept.header().contains("cf_clearance=passed"), "{kept:?}");
    assert!(!kept.user_agent.contains("Headless") && kept.user_agent.contains("Chrome/"), "{}", kept.user_agent);

    // a new client (another indexer on the same site) needs no browser at all
    let other = Http::new("Site", 0.0, "UTF-8", None).with_solver(Some(solver.clone()));
    other.fetch(Fetch::get(&format!("{base}/page"))).await.expect("second page");
    assert_eq!(site.browser.load(Ordering::SeqCst), browser_hits, "no browser for the second request");
    assert_eq!(site.plain_through.load(Ordering::SeqCst), 2);

    // the cookies outlive a restart
    let again = Solver::new(None, &data, Some(db.clone()));
    assert!(again.clearance(host, "").is_some());

    // a site that only lets browsers through: the browser's page stands in
    let p = http.fetch(Fetch::get(&format!("{base}/strict"))).await.expect("strict page");
    assert!(p.text.contains("only-browsers"), "{}", p.text);

    // an outright refusal says so
    let e = http.fetch(Fetch::get(&format!("{base}/blocked"))).await.unwrap_err();
    assert!(e.contains("refused"), "{e}");

    // the FlareSolverr API
    let api = server::solver::api::flaresolverr(solver.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fs = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, api).await.unwrap() });
    let c = reqwest::Client::new();
    let ready: serde_json::Value = c.get(&fs).send().await.unwrap().json().await.unwrap();
    assert_eq!(ready["msg"], "FlareSolverr is ready!");
    let v: serde_json::Value = c
        .post(format!("{fs}/v1"))
        .json(&serde_json::json!({ "cmd": "sessions.create", "session": "s1" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!((v["status"].as_str(), v["session"].as_str()), (Some("ok"), Some("s1")), "{v}");
    let v: serde_json::Value = c
        .post(format!("{fs}/v1"))
        .json(&serde_json::json!({ "cmd": "request.get", "url": format!("{base}/strict"), "maxTimeout": 30000, "session": "s1" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["status"], "ok", "{v}");
    assert_eq!(v["message"], "Challenge solved!", "{v}");
    assert!(v["solution"]["response"].as_str().unwrap_or("").contains("only-browsers"), "{v}");
    assert!(v["solution"]["cookies"].as_array().unwrap().iter().any(|c| c["name"] == "cf_clearance"), "{v}");
    assert!(!v["solution"]["userAgent"].as_str().unwrap().contains("Headless"));
    let v: serde_json::Value =
        c.post(format!("{fs}/v1")).json(&serde_json::json!({ "cmd": "sessions.list" })).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["sessions"], serde_json::json!(["s1"]));
    let r = c.post(format!("{fs}/v1")).json(&serde_json::json!({ "cmd": "sessions.destroy", "session": "s1" })).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = c.post(format!("{fs}/v1")).json(&serde_json::json!({ "cmd": "request.get" })).send().await.unwrap();
    assert_eq!(r.status(), 500);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["message"], "Error: Request parameter 'url' is mandatory");

    // a page that isn't there answers its status, like FlareSolverr
    let sol = solver
        .solve(&SolveRequest { url: format!("{base}/missing"), timeout: Duration::from_secs(5), ..Default::default() })
        .await
        .expect("a plain 404 opens");
    assert_eq!((sol.status, sol.challenged), (404, false));
    solver.shutdown().await;
    let _ = std::fs::remove_dir_all(&data);
}
