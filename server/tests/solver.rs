//! Cloudflare solving with a real headless Chromium against a site that checks browsers the way Cloudflare does
//! (tests/support/challenge.rs), and the FlareSolverr API answering FlareSolverr's own test cases.
//!
//! The `tests_py_*` tests port FlareSolverr v3.5.2's `src/tests.py` (MIT): the same requests, and the same fields
//! and words asserted, against the local test site instead of the live ones (Google, httpbin.org, nowsecure.nl,
//! litres.ru, pararius.com, muziekfabriek.org, a site that blocks).
//!
//! The browser is `LANTERN_TEST_CHROMIUM`, else the first one Lantern finds by itself, else Playwright's
//! headless shell; CI has Google Chrome.

#[path = "support/challenge.rs"]
mod challenge;

use serde_json::{json, Value};
use server::indexers::http::{Fetch, Http};
use server::solver::{find_program, Config, SolveRequest, Solver};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// FlareSolverr's version, as `utils.get_flaresolverr_version()` gives it.
const VERSION: &str = "3.5.2";

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

/// A FlareSolverr API on a free port, with its solver.
struct Fs {
    /// `http://127.0.0.1:port`.
    url: String,
    /// The solver behind it.
    solver: Solver,
    /// A client that doesn't go through the machine's proxy.
    c: reqwest::Client,
    /// The solver's data folder.
    data: PathBuf,
}

impl Fs {
    /// Starts the API for a solver running `program` with `cfg`.
    async fn start(name: &str, program: Option<PathBuf>, cfg: Config) -> Fs {
        let data = std::env::temp_dir().join(format!("lantern-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let solver = Solver::with_config(program, &data, None, cfg);
        let api = server::solver::api::flaresolverr(solver.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, api).await.unwrap() });
        Fs { url, solver, c: reqwest::Client::builder().no_proxy().build().unwrap(), data }
    }

    /// `POST /v1` with `body`: the HTTP status and the answer.
    async fn v1(&self, body: Value) -> (u16, Value) {
        let r = self.c.post(format!("{}/v1", self.url)).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    /// `GET path`: the HTTP status and the answer.
    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self.c.get(format!("{}{path}", self.url)).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    async fn stop(self) {
        self.solver.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

/// The checks every `/v1` answer gets in tests.py: the timestamps and the version.
fn assert_meta(v: &Value) {
    let start = v["startTimestamp"].as_i64().unwrap_or(0);
    assert!(start > 10000, "{v}");
    assert!(v["endTimestamp"].as_i64().unwrap_or(0) >= start, "{v}");
    assert_eq!(v["version"], VERSION, "{v}");
}

/// The cookie named `name` in a solution.
fn cookie<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    v["solution"]["cookies"].as_array()?.iter().find(|c| c["name"] == name)
}

/// tests.py's checks of a solved request: ok, `message`, the timestamps, and the solution's fields.
fn assert_solution(v: &Value, message: &str, url: &str, title: &str) {
    assert_eq!(v["status"], "ok", "{v}");
    assert_eq!(v["message"], message, "{v}");
    assert_meta(v);
    let s = &v["solution"];
    assert!(s["url"].as_str().unwrap_or("").contains(url), "{v}");
    assert_eq!(s["status"], 200, "{v}");
    assert_eq!(s["headers"], json!({}), "{v}");
    assert!(s["response"].as_str().unwrap_or("").contains(title), "{v}");
    assert!(s["userAgent"].as_str().unwrap_or("").contains("Chrome/"), "{v}");
}

/// A free port on this machine.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tests_py_answers_that_need_no_page() {
    let fs = Fs::start("plain", None, Config::default()).await;

    // test_wrong_endpoint
    let (code, v) = fs.get("/wrong").await;
    assert_eq!((code, v), (404, json!({ "error": "Not found: '/wrong'", "status_code": 404 })));
    let (code, v) = fs.get("/v1").await;
    assert_eq!((code, v), (405, json!({ "error": "Method not allowed.", "status_code": 405 })));

    // test_health_endpoint
    assert_eq!(fs.get("/health").await, (200, json!({ "status": "ok" })));

    // test_v1_endpoint_wrong_cmd
    let (code, v) = fs.v1(json!({ "cmd": "request.bad", "url": "https://www.google.com" })).await;
    assert_eq!(code, 500);
    assert_eq!(v["status"], "error");
    assert_eq!(v["message"], "Error: Request parameter 'cmd' = 'request.bad' is invalid.");
    assert_meta(&v);
    assert!(v.get("solution").is_none(), "{v}");

    // test_v1_endpoint_request_post_fail_no_post_data
    let (code, v) = fs.v1(json!({ "cmd": "request.post", "url": "https://www.google.com" })).await;
    assert_eq!((code, v["status"].as_str()), (500, Some("error")));
    assert!(v["message"].as_str().unwrap().contains("Request parameter 'postData' is mandatory in 'request.post' command"), "{v}");

    // test_v1_endpoint_sessions_destroy_non_existing_session
    let (code, v) = fs.v1(json!({ "cmd": "sessions.destroy", "session": "non_existing_session_name" })).await;
    assert_eq!((code, v["status"].as_str()), (500, Some("error")));
    assert_eq!(v["message"], "Error: The session doesn't exist.");

    // the rest of FlareSolverr's checks, word for word
    let cases = [
        (json!({}), "Error: Request parameter 'cmd' is mandatory."),
        (json!({ "cmd": null }), "Error: Request parameter 'cmd' is mandatory."),
        (json!({ "cmd": 5 }), "Error: Request parameter 'cmd' = '5' is invalid."),
        (json!({ "cmd": "request.get" }), "Error: Request parameter 'url' is mandatory in 'request.get' command."),
        (json!({ "cmd": "request.get", "url": "https://x.test/", "postData": "a=1" }), "Error: Cannot use 'postBody' when sending a GET request."),
        (json!({ "cmd": "request.get", "url": "https://x.test/", "maxTimeout": "abc" }), "Error: invalid literal for int() with base 10: 'abc'"),
        (json!({ "cmd": "sessions.destroy" }), "Error: The session doesn't exist."),
        (
            json!({ "cmd": "request.get", "url": "https://x.test/" }),
            "Error: Error solving the challenge. There is no browser for Cloudflare checks: set APP_CHROMIUM to a Chromium program, or use the Lantern image, which has one",
        ),
    ];
    for (body, message) in cases {
        let (code, v) = fs.v1(body.clone()).await;
        assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (500, Some("error"), Some(message)), "{body}");
        assert_meta(&v);
    }
    let (code, v) = fs.v1(json!({ "cmd": "sessions.list" })).await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str(), v["sessions"].clone()), (200, Some("ok"), Some(""), json!([])));
    assert_meta(&v);

    // a body that isn't JSON: Bottle reads none; one that is broken: Bottle's error
    let r = fs.c.post(format!("{}/v1", fs.url)).body("cmd=sessions.list").send().await.unwrap();
    assert_eq!(r.status(), 500);
    assert_eq!(r.json::<Value>().await.unwrap()["message"], "Error: Request parameter 'cmd' is mandatory.");
    let r = fs.c.post(format!("{}/v1", fs.url)).header("content-type", "application/json").body("{bad").send().await.unwrap();
    assert_eq!(r.status(), 500);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({ "error": "(400, 'Invalid JSON')" }));

    // no browser: the welcome page says so, as FlareSolverr's error plugin would
    let (code, v) = fs.get("/").await;
    assert_eq!(code, 500);
    assert!(v["error"].as_str().unwrap().starts_with("Error getting browser User-Agent. There is no browser"), "{v}");
    fs.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tests_py_requests() {
    let (site, counts) = challenge::start_site_on("127.0.0.1:0").await;
    let metrics_port = free_port();
    let cfg = Config {
        lang: Some("fr-FR".into()),
        tz: Some("Asia/Tokyo".into()),
        prometheus_enabled: true,
        prometheus_port: metrics_port,
        ..Config::default()
    };
    let fs = Fs::start("requests", Some(browser()), cfg).await;
    let plain = format!("{site}/plain");

    // test_index_endpoint
    let (code, v) = fs.get("/").await;
    assert_eq!(code, 200, "{v}");
    assert_eq!((v["msg"].as_str(), v["version"].as_str()), (Some("FlareSolverr is ready!"), Some(VERSION)));
    let ua = v["userAgent"].as_str().unwrap();
    assert!(ua.contains("Chrome/") && !ua.to_lowercase().contains("headless"), "{ua}");

    // test_v1_endpoint_request_get_no_cloudflare
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": plain })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", &plain, "<title>Google</title>");
    assert!(!v["solution"]["cookies"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["solution"]["turnstile_token"], Value::Null);
    let nid = cookie(&v, "NID").expect("the page's cookie");
    assert!(nid["expiry"].as_i64().unwrap() > 1_700_000_000 && nid["path"] == "/" && nid["domain"] == "127.0.0.1", "{nid}");

    // test_v1_endpoint_request_get_disable_resources
    let media = format!("{site}/media");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": media, "disableMedia": true })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", &media, "<title>Media</title>");
    assert_eq!(counts.media.load(Ordering::SeqCst), 0, "no image or style sheet loaded");
    let (code, _) = fs.v1(json!({ "cmd": "request.get", "url": media })).await;
    assert_eq!(code, 200);
    assert!(counts.media.load(Ordering::SeqCst) >= 1, "they load by default");

    // test_v1_endpoint_request_get_cookies_param
    let (code, v) = fs
        .v1(json!({ "cmd": "request.get", "url": plain, "cookies": [
            { "name": "testcookie1", "value": "testvalue1" }, { "name": "testcookie2", "value": "testvalue2" }] }))
        .await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", &plain, "<title>Google</title>");
    assert!(v["solution"]["cookies"].as_array().unwrap().len() > 1);
    assert_eq!(cookie(&v, "testcookie1").expect("user cookie 1")["value"], "testvalue1");
    assert_eq!(cookie(&v, "testcookie2").expect("user cookie 2")["value"], "testvalue2");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": plain, "cookies": [{ "value": "x" }] })).await;
    assert_eq!((code, v["message"].as_str()), (500, Some("Error: Error solving the challenge. 'name'")));

    // test_v1_endpoint_request_get_returnOnlyCookies_param
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": plain, "returnOnlyCookies": true })).await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("Challenge not detected!")));
    assert_meta(&v);
    let s = &v["solution"];
    assert!(s["url"].as_str().unwrap().contains(&plain) && s["status"] == 200);
    assert!(s.get("headers").is_none() && s.get("response").is_none(), "{v}");
    assert!(!s["cookies"].as_array().unwrap().is_empty() && s["userAgent"].as_str().unwrap().contains("Chrome/"));

    // test_v1_endpoint_request_get_fail_timeout
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": plain, "maxTimeout": 10 })).await;
    assert_eq!((code, v["status"].as_str()), (500, Some("error")));
    assert_eq!(v["message"], "Error: Error solving the challenge. Timeout after 0.01 seconds.");
    assert_meta(&v);

    // test_v1_endpoint_request_get_fail_bad_domain (a name that can't exist; behind a machine-wide proxy the
    // browser says so through the proxy instead)
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": "https://www.google.combad" })).await;
    assert_eq!((code, v["status"].as_str()), (500, Some("error")));
    let m = v["message"].as_str().unwrap();
    assert!(m.starts_with("Error: Error solving the challenge. Message: unknown error: net::ERR_"), "{m}");
    if std::env::var("HTTPS_PROXY").or(std::env::var("https_proxy")).is_err() {
        assert!(m.contains("Message: unknown error: net::ERR_NAME_NOT_RESOLVED"), "{m}");
    }
    assert!(m.contains("\\n  (Session info: chrome="), "{m}");

    // test_v1_endpoint_request_get_deprecated_param (and the others FlareSolverr v2 removed)
    let (code, v) = fs
        .v1(json!({ "cmd": "request.get", "url": plain, "userAgent": "Test User-Agent", "headers": [], "returnRawHtml": true, "download": true }))
        .await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("Challenge not detected!")), "{v}");
    assert!(!v["solution"]["userAgent"].as_str().unwrap().contains("Test User-Agent"));

    // test_v1_endpoint_request_post_no_cloudflare
    let post_url = format!("{site}/post");
    let (code, v) = fs.v1(json!({ "cmd": "request.post", "url": post_url, "postData": "param1=value1&param2=value2" })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", &post_url, "\"form\": {\n    \"param1\": \"value1\", \n    \"param2\": \"value2\"\n  }");
    assert_eq!(v["solution"]["cookies"].as_array().unwrap().len(), 0, "{v}");
    let (code, v) = fs.v1(json!({ "cmd": "request.post", "url": post_url, "postData": "?q=a%20b%26c&submit=Go" })).await;
    assert_eq!(code, 200, "{v}");
    assert!(v["solution"]["response"].as_str().unwrap().contains("\"q\": \"a b&amp;c\"\n"), "{v}");

    // test_v1_endpoint_request_post_deprecated_param
    let (code, v) = fs
        .v1(json!({ "cmd": "request.post", "url": plain, "postData": "param1=value1&param2=value2", "userAgent": "Test User-Agent" }))
        .await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("Challenge not detected!")), "{v}");

    // returnScreenshot and waitInSeconds
    let start = Instant::now();
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": plain, "returnScreenshot": true, "waitInSeconds": 1 })).await;
    assert_eq!(code, 200, "{v}");
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert!(v["solution"]["screenshot"].as_str().unwrap().starts_with("iVBORw0KGgo"), "a base64 PNG");

    // LANG and TZ reach the browser
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": format!("{site}/env") })).await;
    assert_eq!(code, 200, "{v}");
    let html = v["solution"]["response"].as_str().unwrap();
    assert!(html.contains("lang=fr-FR"), "{html}");
    assert!(html.contains("tz=Asia/Tokyo"), "{html}");

    // PROMETHEUS_ENABLED: the answers are counted on PROMETHEUS_PORT
    let text = fs.c.get(format!("http://127.0.0.1:{metrics_port}/metrics")).send().await.unwrap().text().await.unwrap();
    assert!(text.contains("flaresolverr_request_total{domain=\"127.0.0.1\",result=\"not_detected\"}"), "{text}");
    assert!(text.contains("flaresolverr_request_total{domain=\"127.0.0.1\",result=\"error\"} "), "{text}");
    assert!(text.contains("flaresolverr_request_duration_bucket{domain=\"127.0.0.1\",le=\"+Inf\"}"), "{text}");
    fs.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tests_py_challenges() {
    let (site, _) = challenge::start_site_on("127.0.0.1:0").await;
    // BROWSER_WAIT_TIMEOUT: the test site's checks take a second; each wait for them to go lasts three
    let fs = Fs::start("challenges", Some(browser()), Config { browser_wait_timeout: 3, ..Config::default() }).await;

    // test_v1_endpoint_request_get_cloudflare_js_1 and _2
    let url = format!("{site}/page");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": url })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge solved!", &url, "<title>Results</title>");
    assert!(cookie(&v, "cf_clearance").expect("Cloudflare cookie")["value"].as_str().unwrap().len() > 30);

    // test_v1_endpoint_request_get_ddos_guard_js
    let url = format!("{site}/ddos");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": url })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge solved!", &url, "<title>Литрес");
    assert!(cookie(&v, "__ddg1_").expect("DDOS-Guard cookie")["value"].as_str().unwrap().len() > 10);

    // test_v1_endpoint_request_get_fairlane_js
    let url = format!("{site}/fairlane");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": url })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge solved!", &url, "<title>Rental Apartments Amsterdam</title>");
    assert!(cookie(&v, "fl_pass_v2_b").expect("Fairlane cookie")["value"].as_str().unwrap().len() > 50);

    // test_v1_endpoint_request_get_custom_cloudflare_js
    let url = format!("{site}/custom");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": url })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge solved!", &url, "<title>MuziekFabriek : Aanmelden</title>");
    assert!(cookie(&v, "ct_anti_ddos_key").expect("Custom Cloudflare cookie")["value"].as_str().unwrap().len() > 10);

    // test_v1_endpoint_request_get_cloudflare_blocked (by either title, or by the error box)
    for path in ["/blocked", "/attention", "/denied"] {
        let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": format!("{site}{path}") })).await;
        assert_eq!((code, v["status"].as_str()), (500, Some("error")), "{path}: {v}");
        assert_eq!(
            v["message"],
            "Error: Error solving the challenge. Cloudflare has blocked this request. Probably your IP is banned for this site, check in your web browser.",
            "{path}"
        );
        assert_meta(&v);
    }

    // test_v1_endpoint_request_post_cloudflare
    let url = format!("{site}/page");
    let (code, v) = fs.v1(json!({ "cmd": "request.post", "url": url, "postData": "param1=value1&param2=value2" })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge solved!", &url, "<title>405 Not Allowed</title>");
    assert!(cookie(&v, "cf_clearance").expect("Cloudflare cookie")["value"].as_str().unwrap().len() > 30);

    // a check that waits for "Verify you are human" (click_verify)
    let url = format!("{site}/verify");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": url })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge solved!", &url, "<title>Verified</title>");

    // a Turnstile box ticked after tabs_till_verify Tab presses gives its token
    let url = format!("{site}/turnstile");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": url, "tabs_till_verify": 1 })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", &url, "<title>Turnstile demo</title>");
    assert!(v["solution"]["turnstile_token"].as_str().unwrap_or("").starts_with("0.turnstile-token-"), "{v}");
    let (_, v) = fs.v1(json!({ "cmd": "request.get", "url": url })).await;
    assert_eq!(v["solution"]["turnstile_token"], Value::Null, "{v}");
    fs.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tests_py_sessions() {
    let (site, _) = challenge::start_site_on("127.0.0.1:0").await;
    let fs = Fs::start("sessions", Some(browser()), Config::default()).await;
    let plain = format!("{site}/plain");

    // test_v1_endpoint_sessions_create_without_session
    let (code, v) = fs.v1(json!({ "cmd": "sessions.create" })).await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("Session created successfully.")), "{v}");
    assert_eq!(v["session"].as_str().unwrap().len(), 36, "{v}");
    assert_meta(&v);

    // test_v1_endpoint_sessions_create_with_session
    let (code, v) = fs.v1(json!({ "cmd": "sessions.create", "session": "test_create_session" })).await;
    assert_eq!(
        (code, v["message"].as_str(), v["session"].as_str()),
        (200, Some("Session created successfully."), Some("test_create_session"))
    );
    let (code, v) = fs.v1(json!({ "cmd": "sessions.create", "session": "test_create_session" })).await;
    assert_eq!((code, v["message"].as_str(), v["session"].as_str()), (200, Some("Session already exists."), Some("test_create_session")));

    // test_v1_endpoint_sessions_create_with_proxy
    let (code, v) = fs.v1(json!({ "cmd": "sessions.create", "proxy": { "url": "http://127.0.0.1:8888" } })).await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("Session created successfully.")));
    assert!(v["session"].is_string());

    // test_v1_endpoint_sessions_list
    fs.v1(json!({ "cmd": "sessions.create", "session": "test_list_sessions" })).await;
    let (code, v) = fs.v1(json!({ "cmd": "sessions.list" })).await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("")));
    let ids = v["sessions"].as_array().unwrap();
    assert!(!ids.is_empty() && ids.contains(&json!("test_list_sessions")), "{v}");
    assert_eq!(ids[1], "test_create_session", "oldest first: {v}");

    // test_v1_endpoint_sessions_destroy_existing_session
    fs.v1(json!({ "cmd": "sessions.create", "session": "test_destroy_sessions" })).await;
    let (code, v) = fs.v1(json!({ "cmd": "sessions.destroy", "session": "test_destroy_sessions" })).await;
    assert_eq!((code, v["status"].as_str(), v["message"].as_str()), (200, Some("ok"), Some("The session has been removed.")));
    assert!(!fs.solver.session_ids().await.contains(&"test_destroy_sessions".to_string()));

    // test_v1_endpoint_request_get_with_session
    fs.v1(json!({ "cmd": "sessions.create", "session": "test_request_sessions" })).await;
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "session": "test_request_sessions", "url": plain })).await;
    assert_eq!((code, v["status"].as_str()), (200, Some("ok")), "{v}");

    // a session keeps its cookies; an unknown one is made by the request; session_ttl_minutes makes it again
    let (code, _) =
        fs.v1(json!({ "cmd": "request.get", "session": "keep", "url": plain, "cookies": [{ "name": "mine", "value": "1" }] })).await;
    assert_eq!(code, 200);
    assert!(fs.solver.session_ids().await.contains(&"keep".to_string()));
    let (_, v) = fs.v1(json!({ "cmd": "request.get", "session": "keep", "url": plain })).await;
    assert!(cookie(&v, "mine").is_some(), "the session kept the cookie: {v}");
    let (_, v) = fs.v1(json!({ "cmd": "request.get", "session": "keep", "url": plain, "session_ttl_minutes": 60 })).await;
    assert!(cookie(&v, "mine").is_some(), "a young session is kept: {v}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, v) = fs.v1(json!({ "cmd": "request.get", "session": "keep", "url": plain, "session_ttl_minutes": 0.0001 })).await;
    assert_eq!(v["status"], "ok", "{v}");
    assert!(cookie(&v, "mine").is_none(), "an expired session starts again: {v}");
    assert!(fs.solver.session_ids().await.contains(&"keep".to_string()));
    fs.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tests_py_proxies() {
    let (site, counts) = challenge::start_site_on("127.0.0.1:0").await;
    let (proxy, hits) = challenge::start_proxy(site.clone(), "testuser", "testpass").await;
    // a name only the proxy knows, so the browser has to go through it
    let page = "http://lantern.test/plain";
    let fs = Fs::start("proxies", Some(browser()), Config::default()).await;

    // test_v1_endpoint_request_get_proxy_wrong_param
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": page, "proxy": { "url": "http://127.0.0.1:43210" } })).await;
    assert_eq!((code, v["status"].as_str()), (500, Some("error")), "{v}");
    let m = v["message"].as_str().unwrap();
    assert!(m.contains("Error: Error solving the challenge. Message: unknown error: net::ERR_PROXY_CONNECTION_FAILED"), "{m}");
    assert_meta(&v);

    // test_v1_endpoint_request_get_proxy_http_param_with_credentials
    let (code, v) = fs
        .v1(json!({ "cmd": "request.get", "url": page, "proxy": { "url": proxy, "username": "testuser", "password": "testpass" } }))
        .await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", page, "<title>Google</title>");
    assert!(hits.refused.load(Ordering::SeqCst) >= 1 && hits.passed.load(Ordering::SeqCst) >= 1, "the proxy asked, the browser signed in");

    // a session made with a proxy uses it for its requests
    let creds = json!({ "url": proxy, "username": "testuser", "password": "testpass" });
    let (code, _) = fs.v1(json!({ "cmd": "sessions.create", "session": "via-proxy", "proxy": creds })).await;
    assert_eq!(code, 200);
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": page, "session": "via-proxy" })).await;
    assert_eq!(code, 200, "{v}");
    assert!(v["solution"]["response"].as_str().unwrap().contains("<title>Google</title>"), "{v}");
    fs.stop().await;

    // PROXY_URL, PROXY_USERNAME and PROXY_PASSWORD stand in for a request without a proxy; DISABLE_MEDIA blocks
    // images and style sheets unless the request says otherwise
    let cfg = Config {
        proxy_url: Some(proxy.clone()),
        proxy_username: Some("testuser".into()),
        proxy_password: Some("testpass".into()),
        disable_media: true,
        ..Config::default()
    };
    let fs = Fs::start("env-proxy", Some(browser()), cfg).await;
    let before = hits.passed.load(Ordering::SeqCst);
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": page })).await;
    assert_eq!(code, 200, "{v}");
    assert_solution(&v, "Challenge not detected!", page, "<title>Google</title>");
    assert!(hits.passed.load(Ordering::SeqCst) > before);
    let media = "http://lantern.test/media";
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": media })).await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(counts.media.load(Ordering::SeqCst), 0, "DISABLE_MEDIA blocked them");
    let (code, v) = fs.v1(json!({ "cmd": "request.get", "url": media, "disableMedia": false })).await;
    assert_eq!(code, 200, "{v}");
    assert!(counts.media.load(Ordering::SeqCst) >= 1, "the request's disableMedia wins");
    fs.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn indexers_get_past_a_check_and_keep_the_cookies() {
    let (base, site) = challenge::start_site_on("127.0.0.1:0").await;
    let data = std::env::temp_dir().join(format!("lantern-solver-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let db = kit_server_core::SqliteDb::open(&data.join("app.db")).unwrap();
    server::solver::migrate(&db).unwrap();
    let solver = Solver::with_config(Some(browser()), &data, Some(db.clone()), Config { browser_wait_timeout: 3, ..Config::default() });

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
    let again = Solver::with_config(None, &data, Some(db.clone()), Config::default());
    assert!(again.clearance(host, "").is_some());

    // a site that only lets browsers through: the browser's page stands in
    let p = http.fetch(Fetch::get(&format!("{base}/strict"))).await.expect("strict page");
    assert!(p.text.contains("only-browsers"), "{}", p.text);

    // an outright refusal says so
    let e = http.fetch(Fetch::get(&format!("{base}/blocked"))).await.unwrap_err();
    assert!(e.contains("refused"), "{e}");

    // a page that isn't there keeps its status for Lantern's own use
    let sol = solver
        .solve(&SolveRequest { url: format!("{base}/missing"), timeout: Duration::from_secs(5), ..Default::default() })
        .await
        .expect("a plain 404 opens");
    assert_eq!((sol.status, sol.challenged), (404, false));
    solver.shutdown().await;
    let _ = std::fs::remove_dir_all(&data);
}
