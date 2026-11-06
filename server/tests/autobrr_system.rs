// Ported from autobrr v1.90.0 (internal/http/auth_test.go, internal/database/api_test.go and proxy_test.go,
// internal/proxy/service_test.go, internal/irc/dial_test.go; the handlers of internal/http/auth.go, apikey.go,
// config.go, health.go, update.go and proxy.go), GPL-2.0-or-later, with answer shapes from autobrr's Go structs
// (tests/fixtures/upstream/autobrr/api/system.json).
//! autobrr's application routes under `/autobrr/api`: signing in with Lantern's accounts, onboarding, the
//! account, API keys, health, the config file's settings, update checks against a stand-in of Lantern's
//! releases, and proxies, which IRC networks, feeds and torrent downloads go through (stand-in SOCKS5 and HTTP
//! proxies on 127.0.0.1). Nothing here reaches the internet.

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;

const KEY: &str = "test-key";
const PASSWORD: &str = "correct horse battery";

/// Lantern with an owner (`admin`) unless `owner` is false.
async fn lantern(name: &str, owner: bool) -> (Router, server::AppState) {
    let dir = std::env::temp_dir().join(format!("lantern-absys-{name}-{}", std::process::id()));
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
    if owner {
        server::auth::ensure_owner(&st, None, Some(PASSWORD.into())).unwrap();
    }
    (server::app(st.clone()), st)
}

/// What a call answered.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

/// Calls `path` under `/autobrr/api` with `headers` and a JSON `body`.
async fn raw(app: &Router, method: &str, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> Answer {
    let mut b = Request::builder().method(method).uri(format!("/autobrr/api{path}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (r.status(), r.headers().clone());
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    Answer { status, headers, text: String::from_utf8_lossy(&bytes).into_owned() }
}

/// Calls `path` with Lantern's API key.
async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let a = raw(app, method, path, &[("x-api-key", KEY)], body).await;
    let v = a.json();
    (a.status, v)
}

/// The JSON keys autobrr's Go struct `name` always sends, and those it may.
fn shape(name: &str) -> (Vec<String>, Vec<String>) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream/autobrr/api/system.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let s = &v["structs"][name];
    let list = |k: &str| s[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
    (list("always"), list("omitempty"))
}

/// `v` has every key struct `name` always sends, and none it doesn't have (but Lantern's `detail`).
fn assert_shape(name: &str, v: &Value) {
    let (always, maybe) = shape(name);
    let o = v.as_object().unwrap_or_else(|| panic!("{name} is not an object: {v}"));
    for k in &always {
        assert!(o.contains_key(k), "{name} lacks {k}: {v}");
    }
    for k in o.keys() {
        assert!(always.contains(k) || maybe.contains(k) || k == "detail", "{name} has {k}, which autobrr's hasn't: {v}");
    }
}

/// The session cookie a sign-in set (`name=value`).
fn cookie_of(a: &Answer) -> String {
    let c = a.headers.get(header::SET_COOKIE).expect("handler returned no cookie").to_str().unwrap();
    c.split(';').next().unwrap().to_string()
}

/// Signs in as `user` with `password` through autobrr's login.
async fn sign_in(app: &Router, user: &str, password: &str) -> Answer {
    raw(app, "POST", "/auth/login", &[("x-requested-with", "XMLHttpRequest")], Some(json!({ "username": user, "password": password }))).await
}

/// Waits up to 10 s for `f`.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// auth_test.go `TestAuthHandlerLogin`, `TestAuthHandlerValidateOK`, `TestAuthHandlerValidateBad`,
/// `TestAuthHandlerLoginBad` and `TestAuthHandlerLogout`, on Lantern's accounts, and the account change of
/// auth/service.go `UpdateUser`.
#[tokio::test]
async fn sign_in_validate_change_the_password_and_sign_out() {
    let (app, _st) = lantern("auth", true).await;
    // TestAuthHandlerValidateBad: no session is a 403
    let a = raw(&app, "GET", "/auth/validate", &[], None).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    // TestAuthHandlerLoginBad
    let a = sign_in(&app, "admin", "notmypass").await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    assert_shape("errorResponse", &a.json());
    assert_eq!(a.json()["message"], "could not login: bad credentials");
    let a = sign_in(&app, "", "").await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "empty credentials");
    let a = raw(&app, "POST", "/auth/login", &[], Some(json!("not an object"))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);
    assert!(a.json()["message"].as_str().unwrap().starts_with("could not decode json"), "{}", a.text);

    // TestAuthHandlerLogin and TestAuthHandlerValidateOK
    let a = sign_in(&app, "admin", PASSWORD).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.text);
    let cookie = cookie_of(&a);
    assert!(!a.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age"), "a session cookie without remember_me");
    let session = [("cookie", cookie.as_str()), ("x-requested-with", "XMLHttpRequest")];
    let a = raw(&app, "GET", "/auth/validate", &session, None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_shape("validate", &a.json());
    assert_eq!(a.json(), json!({ "username": "admin", "auth_method": "password" }));
    // the session reaches the rest of autobrr's API, writes too (autobrr's frontend marks them XMLHttpRequest)
    let a = raw(&app, "POST", "/proxy", &session, Some(json!({ "name": "p", "type": "HTTP", "addr": "http://127.0.0.1:9", "enabled": false }))).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "{}", a.text);
    let a = raw(&app, "POST", "/proxy", &[("cookie", cookie.as_str())], Some(json!({ "name": "p" }))).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "a cookie write without the mark is refused");
    let a = raw(&app, "POST", "/auth/login", &[], Some(json!({ "username": "admin", "password": PASSWORD, "remember_me": true }))).await;
    assert!(a.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age"), "remember_me keeps the cookie");

    // UpdateUser: the current password first, a different new one, only the signed-in account
    let change = |b: Value| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move { raw(&app, "PATCH", "/auth/user/admin", &[("cookie", cookie.as_str()), ("x-requested-with", "XMLHttpRequest")], Some(b)).await }
    };
    let a = change(json!({ "password_new": "another long one" })).await;
    assert_eq!((a.status, a.json()["message"].as_str()), (StatusCode::FORBIDDEN, Some("validation error: empty current password supplied")));
    let a = change(json!({ "password_current": PASSWORD, "password_new": PASSWORD })).await;
    assert_eq!(a.json()["message"], "validation error: new password must be different");
    let a = change(json!({ "password_current": "wrong", "password_new": "another long one" })).await;
    assert_eq!((a.status, a.json()["message"].as_str()), (StatusCode::FORBIDDEN, Some("invalid login: admin")));
    let a = raw(&app, "PATCH", "/auth/user/someone", &session, Some(json!({ "password_current": PASSWORD, "password_new": "x" }))).await;
    assert_eq!((a.status, a.json()["message"].as_str()), (StatusCode::FORBIDDEN, Some("invalid login: someone")));
    let a = change(json!({ "password_current": PASSWORD, "password_new": "short" })).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "Lantern's password rule: {}", a.text);
    let a = change(json!({ "password_current": PASSWORD, "password_new": "another long one" })).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_shape("statusResponse", &a.json());
    assert_eq!(a.json()["message"], "user successfully updated");
    assert_eq!(sign_in(&app, "admin", PASSWORD).await.status, StatusCode::FORBIDDEN);
    assert_eq!(sign_in(&app, "admin", "another long one").await.status, StatusCode::NO_CONTENT);

    // TestAuthHandlerLogout: the session ends
    let a = raw(&app, "POST", "/auth/logout", &session, None).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT);
    let a = raw(&app, "GET", "/auth/validate", &session, None).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    // autobrr answers 404 for its OIDC routes when OIDC is off; Lantern has none
    let a = raw(&app, "GET", "/auth/oidc/config", &[], None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
}

/// auth.go `canOnboard` and `onboard`: open while there is no account, closed after.
#[tokio::test]
async fn onboarding_answers_as_autobrr_does() {
    let (app, _) = lantern("onboard", true).await;
    let a = raw(&app, "GET", "/auth/onboard", &[], None).await;
    assert_eq!((a.status, a.json()), (StatusCode::SERVICE_UNAVAILABLE, json!("onboarding unavailable: user already registered")));
    let a = raw(&app, "POST", "/auth/onboard", &[], Some(json!({ "username": "x", "password": "y" }))).await;
    assert_eq!((a.status, a.json()["message"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("onboarding unavailable: user already registered")));

    let (app, _) = lantern("onboard-empty", false).await;
    let a = raw(&app, "GET", "/auth/onboard", &[], None).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT);
    // Lantern's first account comes from its own setup page with the setup code
    let a = raw(&app, "POST", "/auth/onboard", &[], Some(json!({ "username": "first", "password": "long enough pw" }))).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    assert!(a.json()["detail"].as_str().unwrap().contains("setup code"));
    assert_eq!(raw(&app, "GET", "/auth/onboard", &[], None).await.status, StatusCode::NO_CONTENT, "still no account");
}

/// api_test.go (`Store`, `Delete`, `GetAllAPIKeys`, `GetKey`) through apikey.go, and middleware.go
/// `IsAuthenticated`'s `X-API-Token` header and `apikey` query.
#[tokio::test]
async fn api_keys_are_made_listed_used_and_removed() {
    let (app, _) = lantern("keys", true).await;
    let (s, list) = call(&app, "GET", "/keys", None).await;
    assert_eq!((s, list), (StatusCode::OK, json!([])), "GetKeys_Returns_Empty_If_No_Keys");
    let (s, k) = call(&app, "POST", "/keys", Some(json!({ "name": "TestKey", "scopes": ["read", "write"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{k}");
    assert_shape("APIKey", &k);
    let key = k["key"].as_str().unwrap().to_string();
    assert!(key.len() == 32 && key.chars().all(|c| c.is_ascii_hexdigit()), "16 random bytes as hex: {key}");
    assert_ne!(k["created_at"], "0001-01-01T00:00:00Z");
    let (s, v) = call(&app, "POST", "/keys", Some(json!({ "name": "no scopes" }))).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "Store_Fails_If_No_Name_Or_Scopes: {v}");
    let (_, list) = call(&app, "GET", "/keys", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "GetKeys_Returns_Keys_If_Exists");
    assert_eq!((list[0]["name"].as_str(), list[0]["scopes"].clone()), (Some("TestKey"), json!(["read", "write"])));

    // the key signs in, by header or query; a wrong one is a 401
    let a = raw(&app, "GET", "/auth/validate", &[("x-api-token", &key)], None).await;
    assert_eq!((a.status, a.json()), (StatusCode::OK, json!({ "username": "", "auth_method": "" })));
    let a = raw(&app, "GET", &format!("/filters?apikey={key}"), &[], None).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    let a = raw(&app, "POST", "/proxy", &[("x-api-token", &key)], Some(json!({ "name": "k", "type": "HTTP", "addr": "http://127.0.0.1:9" }))).await;
    assert_eq!(a.status, StatusCode::NO_CONTENT, "a key may write: {}", a.text);
    assert_eq!(raw(&app, "GET", "/filters", &[("x-api-token", "wrong")], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(raw(&app, "GET", "/filters?apikey=wrong", &[], None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(raw(&app, "GET", "/filters", &[("x-api-token", KEY)], None).await.status, StatusCode::OK, "Lantern's own key works too");

    // Delete_Succeeds_With_Existing_Key, then the key is gone
    let (s, _) = call(&app, "DELETE", &format!("/keys/{key}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(raw(&app, "GET", "/filters", &[("x-api-token", &key)], None).await.status, StatusCode::UNAUTHORIZED);
    let (s, v) = call(&app, "DELETE", &format!("/keys/{key}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!((v["code"].as_str(), v["message"].as_str()), (Some("RECORD_NOT_FOUND"), Some(format!("api key {key} not found").as_str())));
}

/// health.go and config.go: health without a session, the config file's settings as they map onto Lantern.
#[tokio::test]
async fn health_and_the_config_files_settings() {
    let (app, st) = lantern("config", true).await;
    for path in ["/healthz/liveness", "/healthz/readiness"] {
        let a = raw(&app, "GET", path, &[], None).await;
        assert_eq!((a.status, a.text.as_str()), (StatusCode::OK, "OK"), "{path}");
        assert_eq!(a.headers[header::CONTENT_TYPE], "text/plain");
    }
    assert_eq!(raw(&app, "GET", "/config", &[], None).await.status, StatusCode::FORBIDDEN);
    let (s, c) = call(&app, "GET", "/config", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("configJson", &c);
    assert_eq!((c["database"].as_str(), c["base_url"].as_str(), c["log_level"].as_str()), (Some("sqlite"), Some("/autobrr/"), Some("INFO")));
    assert_eq!((c["check_for_updates"].as_bool(), c["log_path"].as_str()), (Some(true), Some("")));
    assert_eq!((c["log_max_size"].as_i64(), c["log_max_backups"].as_i64()), (Some(10), Some(5)));
    assert_eq!(c["port"], server::port());
    assert_eq!(PathBuf::from(c["config_dir"].as_str().unwrap()), std::path::absolute(&st.data_dir).unwrap());

    let (s, _) = call(&app, "PATCH", "/config", Some(json!({ "log_level": "DEBUG", "check_for_updates": false, "port": 1, "host": "x" }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, c) = call(&app, "GET", "/config", None).await;
    assert_eq!((c["log_level"].as_str(), c["check_for_updates"].as_bool()), (Some("DEBUG"), Some(false)));
    assert_eq!(c["port"], server::port(), "the port isn't changed here, as in autobrr");
    let (s, _) = call(&app, "PATCH", "/config", Some(json!({ "log_path": "logs/autobrr.log" }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/config", None).await.1["log_path"], "logs/autobrr.log");
    let (s, v) = call(&app, "PATCH", "/config", Some(json!({ "log_path": "/var/log/autobrr.log" }))).await;
    assert_eq!((s, v["status"].as_i64()), (StatusCode::INTERNAL_SERVER_ERROR, Some(500)), "only inside the data folder: {v}");
    let (s, v) = call(&app, "PATCH", "/config", Some(json!({ "log_level": 3 }))).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("INTERNAL_SERVER_ERROR")), "a bad body: {v}");
}

/// A stand-in HTTP server: answers every request with `body` and keeps the request lines.
async fn http_stand_in(status: u16, body: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = seen.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = l.accept().await {
            let seen = s2.clone();
            let body = body.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(sock);
                let mut first = String::new();
                if r.read_line(&mut first).await.is_err() {
                    return;
                }
                seen.lock().unwrap().push(first.trim().to_string());
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).await.unwrap_or(0) == 0 || l == "\r\n" {
                        break;
                    }
                }
                let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = r.get_mut().write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

/// A stand-in proxy: SOCKS5 (with `user`/`pass` when given) or HTTP (CONNECT, and plain requests answered
/// with 200 `ok`); every target it was asked for, and the HTTP proxy's `Proxy-Authorization` headers.
struct StandInProxy {
    addr: String,
    targets: Arc<Mutex<Vec<String>>>,
}

impl StandInProxy {
    async fn socks5(auth: Option<(&'static str, &'static str)>) -> StandInProxy {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let targets: Arc<Mutex<Vec<String>>> = Arc::default();
        let t2 = targets.clone();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = l.accept().await {
                let targets = t2.clone();
                tokio::spawn(async move {
                    let mut head = [0u8; 2];
                    c.read_exact(&mut head).await.unwrap();
                    let mut methods = vec![0u8; head[1] as usize];
                    c.read_exact(&mut methods).await.unwrap();
                    if let Some((u, p)) = auth {
                        c.write_all(&[5, 2]).await.unwrap();
                        let mut v = [0u8; 2];
                        c.read_exact(&mut v).await.unwrap();
                        let mut user = vec![0u8; v[1] as usize];
                        c.read_exact(&mut user).await.unwrap();
                        let mut pl = [0u8; 1];
                        c.read_exact(&mut pl).await.unwrap();
                        let mut pass = vec![0u8; pl[0] as usize];
                        c.read_exact(&mut pass).await.unwrap();
                        let ok = user == u.as_bytes() && pass == p.as_bytes();
                        c.write_all(&[1, if ok { 0 } else { 1 }]).await.unwrap();
                        if !ok {
                            return;
                        }
                    } else {
                        c.write_all(&[5, 0]).await.unwrap();
                    }
                    let mut req = [0u8; 4];
                    c.read_exact(&mut req).await.unwrap();
                    let host = match req[3] {
                        1 => {
                            let mut ip = [0u8; 4];
                            c.read_exact(&mut ip).await.unwrap();
                            std::net::Ipv4Addr::from(ip).to_string()
                        }
                        3 => {
                            let mut l = [0u8; 1];
                            c.read_exact(&mut l).await.unwrap();
                            let mut h = vec![0u8; l[0] as usize];
                            c.read_exact(&mut h).await.unwrap();
                            String::from_utf8(h).unwrap()
                        }
                        _ => return,
                    };
                    let mut port = [0u8; 2];
                    c.read_exact(&mut port).await.unwrap();
                    let target = format!("{host}:{}", u16::from_be_bytes(port));
                    targets.lock().unwrap().push(target.clone());
                    let Ok(mut up) = tokio::net::TcpStream::connect(&target).await else {
                        let _ = c.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                        return;
                    };
                    c.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
                });
            }
        });
        StandInProxy { addr, targets }
    }

    async fn http() -> StandInProxy {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let targets: Arc<Mutex<Vec<String>>> = Arc::default();
        let t2 = targets.clone();
        tokio::spawn(async move {
            while let Ok((c, _)) = l.accept().await {
                let targets = t2.clone();
                tokio::spawn(async move {
                    let mut r = BufReader::new(c);
                    let mut first = String::new();
                    r.read_line(&mut first).await.unwrap();
                    let mut auth = String::new();
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).await.unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                        if l.to_ascii_lowercase().starts_with("proxy-authorization:") {
                            auth = l.trim().to_string();
                        }
                    }
                    let parts: Vec<&str> = first.split_whitespace().collect();
                    targets.lock().unwrap().push(format!("{} {}{}", parts[0], parts[1], if auth.is_empty() { String::new() } else { format!(" [{auth}]") }));
                    if parts[0] == "CONNECT" {
                        let Ok(mut up) = tokio::net::TcpStream::connect(parts[1]).await else {
                            let _ = r.get_mut().write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                            return;
                        };
                        r.get_mut().write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
                        let mut c = r.into_inner();
                        let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
                    } else {
                        let _ = r.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
                    }
                });
            }
        });
        StandInProxy { addr, targets }
    }

    fn saw(&self) -> Vec<String> {
        self.targets.lock().unwrap().clone()
    }
}

/// The proxy's id by name.
async fn proxy_id(app: &Router, name: &str) -> i64 {
    let (_, list) = call(app, "GET", "/proxy", None).await;
    list.as_array().unwrap().iter().find(|p| p["name"] == name).unwrap()["id"].as_i64().unwrap()
}

/// proxy.go's routes over proxy_test.go's repository cases (`Store`, `Update`, `Delete` with
/// `Delete_Detaches_Indexer_And_Network`, `FindByID`, `Usage`), domain/proxy.go's checks and service.go's `Test`
/// through stand-in proxies.
#[tokio::test]
async fn proxies_are_kept_tested_used_and_detached() {
    let (app, st) = lantern("proxy", true).await;
    let (s, v) = call(&app, "POST", "/proxy", Some(json!({ "name": "bad", "type": "SOCKS4", "addr": "socks4://1.2.3.4:1080" }))).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("validation error: invalid proxy type: SOCKS4")));
    let (_, v) = call(&app, "POST", "/proxy", Some(json!({ "name": "bad", "type": "SOCKS5", "addr": "http://1.2.3.4:1080" }))).await;
    assert_eq!(v["message"], "validation error: proxy url scheme must be socks5 or socks5h");
    let (_, v) = call(&app, "POST", "/proxy", Some(json!({ "type": "HTTP", "addr": "http://1.2.3.4:1080" }))).await;
    assert_eq!(v["message"], "validation error: name is required");

    let socks = StandInProxy::socks5(Some(("me", "hunter2"))).await;
    let (s, v) = call(&app, "POST", "/proxy", Some(json!({ "name": "socks", "enabled": true, "type": "SOCKS5", "addr": format!("socks5://{}", socks.addr), "user": "me", "pass": "hunter2", "timeout": 30 }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "Store_Succeeds: {v}");
    let id = proxy_id(&app, "socks").await;
    let (s, p) = call(&app, "GET", &format!("/proxy/{id}"), None).await;
    assert_eq!(s, StatusCode::OK, "FindByID_Succeeds");
    assert_shape("Proxy", &p);
    assert_eq!((p["pass"].as_str(), p["user"].as_str(), p["type"].as_str(), p["timeout"].as_i64()), (Some("<redacted>"), Some("me"), Some("SOCKS5"), Some(30)));
    let (s, v) = call(&app, "GET", "/proxy/999", None).await;
    assert_eq!((s, v["code"].as_str(), v["message"].as_str()), (StatusCode::NOT_FOUND, Some("RECORD_NOT_FOUND"), Some("could not find proxy with id 999")));
    let (s, v) = call(&app, "GET", "/proxy/abc", None).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::BAD_REQUEST, Some("BAD_REQUEST_PARAMS")));

    // Update: a redacted password keeps the saved one; a proxy that isn't there is autobrr's error
    let mut changed = p.clone();
    changed["name"] = json!("socks renamed");
    let (s, v) = call(&app, "PUT", &format!("/proxy/{id}"), Some(changed.clone())).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "Update_Succeeds: {v}");
    assert_eq!(server::releases::proxy::find(&st.db, id).unwrap().pass, "hunter2");
    changed["id"] = json!(999);
    let (s, v) = call(&app, "PUT", "/proxy/999", Some(changed)).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("sql: no rows in result set")), "Update_Fails_Invalid_ID");

    // Test: a page through the proxy, the saved password used for a redacted one
    let (target, hits) = http_stand_in(200, "{}".into()).await;
    st.releases.set_proxy_test_url(&format!("{target}/"));
    let (s, v) = call(&app, "POST", "/proxy/test", Some(p.clone())).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    assert_eq!(socks.saw(), vec![target.trim_start_matches("http://").to_string()]);
    assert_eq!(hits.lock().unwrap().len(), 1);
    let (_, v) = call(&app, "POST", "/proxy/test", Some(json!({ "type": "SOCKS5", "addr": format!("socks5://{}", socks.addr), "user": "me", "pass": "wrong" }))).await;
    assert!(v["message"].as_str().unwrap().starts_with(&format!("could not connect to proxy server: socks5://{}", socks.addr)), "{v}");
    let (_, v) = call(&app, "POST", "/proxy/test", Some(json!({ "type": "FTP" }))).await;
    assert_eq!(v["message"], "invalid proxy type FTP");
    let (_, v) = call(&app, "POST", "/proxy/test", Some(json!({ "type": "HTTP" }))).await;
    assert_eq!(v["message"], "proxy addr missing");
    let (s, v) = call(&app, "POST", "/proxy/test", Some(json!({ "id": 999, "type": "HTTP", "addr": "http://x" }))).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::NOT_FOUND, Some("could not find proxy")));
    let http = StandInProxy::http().await;
    let (s, _) = call(&app, "POST", "/proxy/test", Some(json!({ "type": "HTTP", "addr": format!("http://{}", http.addr), "user": "u", "pass": "p" }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(http.saw(), vec![format!("GET {target}/ [Proxy-Authorization: Basic dTpw]")]);
    let (_, v) = call(&app, "POST", "/proxy/test", Some(json!({ "type": "HTTP", "addr": "http://127.0.0.1:1" }))).await;
    assert!(v["message"].as_str().unwrap().starts_with("could not connect to proxy server: http://127.0.0.1:1"), "{v}");

    // Usage: the indexers and networks set to use it; then Delete detaches them
    let (s, ix) = call(&app, "POST", "/indexer", Some(json!({ "name": "Torznab", "identifier": "torznab", "implementation": "torznab", "enabled": true, "use_proxy": true, "proxy_id": id, "settings": { "url": "http://127.0.0.1:1", "api_key": "k" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ix}");
    let (s, v) = call(&app, "POST", "/irc", Some(json!({ "name": "Net", "enabled": false, "server": "127.0.0.1", "port": 1, "nick": "me", "auth": {}, "use_proxy": true, "proxy_id": id, "channels": [] }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    let (s, u) = call(&app, "GET", &format!("/proxy/{id}/usage"), None).await;
    assert_eq!(s, StatusCode::OK, "Usage_Succeeds");
    assert_shape("ProxyUsage", &u);
    assert_eq!(u["indexers"], json!([{ "id": ix["id"], "name": "Torznab" }]));
    assert_eq!(u["irc_networks"].as_array().unwrap().len(), 1);
    assert_shape("ProxyUsageItem", &u["irc_networks"][0]);
    let other = proxy_id(&app, "k").await.max(0);
    let (_, u) = call(&app, "GET", &format!("/proxy/{}/usage", other + 1000), None).await;
    assert_eq!(u, json!({ "indexers": [], "irc_networks": [], "feeds": [] }), "Usage_Empty_For_Unused_Proxy");
    let (s, _) = call(&app, "DELETE", &format!("/proxy/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "Delete_Succeeds");
    let (_, i) = call(&app, "GET", &format!("/indexer/{}", ix["id"]), None).await;
    assert_eq!((i["use_proxy"].as_bool(), i["proxy_id"].as_i64()), (Some(false), Some(0)), "Delete_Detaches_Indexer_And_Network");
    let (_, nets) = call(&app, "GET", "/irc", None).await;
    assert_eq!(nets[0]["use_proxy"], false);
    let (s, v) = call(&app, "DELETE", &format!("/proxy/{id}"), None).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::BAD_REQUEST, Some("delete failed")), "Delete_Fails_No_Record");
}

/// A stand-in IRC server that welcomes whoever registers; every line it heard.
async fn ircd() -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let h2 = heard.clone();
    tokio::spawn(async move {
        while let Ok((sock, _)) = l.accept().await {
            let heard = h2.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    heard.lock().unwrap().push(line.clone());
                    if line.starts_with("USER ") {
                        let _ = w.write_all(b":irc.test 001 me :Welcome\r\n:irc.test 376 me :End of MOTD\r\n").await;
                    }
                    if let Some(t) = line.strip_prefix("PING ") {
                        let _ = w.write_all(format!("PONG {t}\r\n").as_bytes()).await;
                    }
                }
            });
        }
    });
    (port, heard)
}

/// irc/handler.go `Run` with a proxy, and service.go `onProxyChanged`: a network goes through its SOCKS5 or
/// HTTP CONNECT proxy, refuses to connect without it, and is turned off with it.
#[tokio::test]
async fn an_irc_network_connects_through_its_proxy_and_is_turned_off_with_it() {
    let (app, st) = lantern("ircproxy", true).await;
    st.releases.irc.set_timing(server::releases::irc::Timing {
        after_connect: Duration::from_millis(50),
        connect_retry: Duration::from_millis(200),
        connect_jitter: Duration::from_millis(10),
        reconnect: Duration::from_millis(200),
        tick: Duration::from_millis(20),
        ..server::releases::irc::Timing::default()
    });
    let (port, heard) = ircd().await;
    let socks = StandInProxy::socks5(None).await;
    let http = StandInProxy::http().await;
    for (name, kind, addr) in [("socks", "SOCKS5", format!("socks5h://{}", socks.addr)), ("http", "HTTP", format!("http://{}", http.addr))] {
        let (s, v) = call(&app, "POST", "/proxy", Some(json!({ "name": name, "enabled": true, "type": kind, "addr": addr }))).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    }
    let (socks_id, http_id) = (proxy_id(&app, "socks").await, proxy_id(&app, "http").await);
    let net = |name: &str, proxy: i64| {
        json!({ "name": name, "enabled": true, "server": "127.0.0.1", "port": port, "tls": false, "nick": "me", "auth": {},
                "use_proxy": true, "proxy_id": proxy, "channels": [] })
    };
    let (s, v) = call(&app, "POST", "/irc", Some(net("Through socks", socks_id))).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    until("the network to register through SOCKS5", || heard.lock().unwrap().iter().any(|l| l == "NICK me")).await;
    assert_eq!(socks.saw(), vec![format!("127.0.0.1:{port}")]);
    let (_, nets) = call(&app, "GET", "/irc", None).await;
    let id = nets[0]["id"].as_i64().unwrap();

    // a proxy turned off takes its networks down and off, never direct
    let (_, mut p) = call(&app, "GET", &format!("/proxy/{socks_id}"), None).await;
    p["enabled"] = json!(false);
    let (s, _) = call(&app, "PUT", &format!("/proxy/{socks_id}"), Some(p)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, n) = call(&app, "GET", &format!("/irc/network/{id}"), None).await;
    assert_eq!(n["enabled"], false, "proxy disabled, disabling network: {n}");

    // HTTP CONNECT
    heard.lock().unwrap().clear();
    let (s, _) = call(&app, "POST", "/irc", Some(net("Through http", http_id))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    until("the network to register through HTTP CONNECT", || heard.lock().unwrap().iter().any(|l| l == "NICK me")).await;
    assert_eq!(http.saw(), vec![format!("CONNECT 127.0.0.1:{port}")]);

    // a proxy that isn't there: the network refuses to connect
    let (s, _) = call(&app, "POST", "/irc", Some(net("Missing proxy", 999))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let mut errors = Value::Null;
    for _ in 0..250 {
        let (_, nets) = call(&app, "GET", "/irc", None).await;
        errors = nets.as_array().unwrap().iter().find(|n| n["name"] == "Missing proxy").unwrap()["connection_errors"].clone();
        if errors.as_array().is_some_and(|e| !e.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(errors, json!(["configured proxy could not be loaded"]));
}

/// feed/source.go `loadProxy` and release/download_service.go: a feed and a torrent download go through the
/// proxy of their indexer when it is on; a missing proxy stops the feed.
#[tokio::test]
async fn feeds_and_downloads_go_through_their_indexers_proxy() {
    let (app, st) = lantern("feedproxy", true).await;
    let rss = "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>t</title></channel></rss>".to_string();
    let (site, _) = http_stand_in(200, rss).await;
    let http = StandInProxy::http().await;
    let (s, _) = call(&app, "POST", "/proxy", Some(json!({ "name": "http", "enabled": true, "type": "HTTP", "addr": format!("http://{}", http.addr) }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let pid = proxy_id(&app, "http").await;
    let (s, ix) = call(&app, "POST", "/indexer", Some(json!({ "name": "Feedy", "identifier": "rss", "implementation": "rss", "enabled": true, "use_proxy": true, "proxy_id": pid, "settings": { "url": format!("{site}/rss") } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ix}");
    let body = json!({ "name": "Feedy", "indexer_id": ix["id"], "type": "RSS", "enabled": false, "url": format!("{site}/rss"), "interval": 30, "timeout": 10 });
    let (s, f) = call(&app, "POST", "/feeds", Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    let (s, v) = call(&app, "POST", "/feeds/test", Some(f.clone())).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    assert_eq!(http.saw(), vec![format!("GET {site}/rss")], "the feed went through the proxy");

    // a torrent download of the indexer's release goes through it too
    let downloader = server::releases::run::HttpDownloader::with_proxies({
        let db = st.db.clone();
        Arc::new(move |id| server::releases::proxy::indexer_proxy(&db, id))
    });
    let mut r = server::releases::release::Release { protocol: "torrent".into(), download_url: format!("{site}/t.torrent"), ..Default::default() };
    r.indexer.id = ix["id"].as_i64().unwrap();
    r.indexer.name = "Feedy".into();
    use server::releases::run::ReleaseDownloader;
    let _ = downloader.download_release(&mut r).await;
    assert!(http.saw().contains(&format!("GET {site}/t.torrent")), "{:?}", http.saw());

    // the proxy turned off: the feed goes direct, as autobrr's does (only IRC refuses)
    let (_, mut p) = call(&app, "GET", &format!("/proxy/{pid}"), None).await;
    p["enabled"] = json!(false);
    call(&app, "PUT", &format!("/proxy/{pid}"), Some(p)).await;
    let before = http.saw().len();
    let (s, _) = call(&app, "POST", "/feeds/test", Some(f.clone())).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(http.saw().len(), before, "a disabled proxy isn't used");

    // the proxy gone: the indexer is detached; one pointing at a missing proxy stops the feed
    st.db.execute("UPDATE autobrr_indexer_meta SET use_proxy=1, proxy_id=999 WHERE source_id=?", kit_server_core::p![ix["id"].as_i64().unwrap()]).unwrap();
    let (s, v) = call(&app, "POST", "/feeds/test", Some(f)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(v["message"], "could not find proxy for indexer feed: record not found");
}

/// update/update.go, pkg/version and scheduler/jobs.go `CheckUpdatesJob` against a stand-in of Lantern's
/// releases (the kit's release feed in autobrr's place): the newer release is kept, `/updates/latest` answers
/// it, and the job sends `APP_UPDATE_AVAILABLE` once per version (to the built-in inbox).
#[tokio::test]
async fn update_checks_find_a_newer_lantern_release_and_notify_once() {
    let (app, st) = lantern("updates", true).await;
    let list = json!([
        { "tag_name": "v99.1.0-rc1", "prerelease": true, "html_url": "https://example.test/rc" },
        { "id": 7, "tag_name": "v99.0.0", "name": "v99.0.0", "html_url": "https://github.com/Krifmaz/lantern/releases/tag/v99.0.0",
          "body": "notes", "draft": false, "prerelease": false, "created_at": "2026-11-01T00:00:00Z", "published_at": "2026-11-01T00:00:00Z",
          "author": { "login": "Krifmaz" }, "assets": [] }
    ]);
    let (feed, hits) = http_stand_in(200, list.to_string()).await;
    st.releases.updates.set_url(&format!("{feed}/repos/Krifmaz/lantern/releases"));
    let (s, _) = call(&app, "GET", "/updates/latest", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "nothing found yet");
    let (s, _) = call(&app, "GET", "/updates/check", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(hits.lock().unwrap().len(), 1);
    let (s, r) = call(&app, "GET", "/updates/latest", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_shape("Release", &r);
    assert_eq!((r["tag_name"].as_str(), r["html_url"].as_str()), (Some("v99.0.0"), Some("https://github.com/Krifmaz/lantern/releases/tag/v99.0.0")));

    // the job: the inbox hears of v99.0.0 once (the check above already took it, so a fresh copy is used)
    let (app, st) = lantern("updates-job", true).await;
    st.releases.updates.set_url(&format!("{feed}/repos/Krifmaz/lantern/releases"));
    st.releases.run_update_job_once().await;
    st.releases.run_update_job_once().await;
    let mut inbox = Value::Null;
    for _ in 0..250 {
        inbox = call(&app, "GET", "/notification/inbox", None).await.1;
        if inbox["data"].as_array().is_some_and(|d| !d.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let msgs: Vec<&Value> = inbox["data"].as_array().unwrap().iter().filter(|m| m["event"] == "APP_UPDATE_AVAILABLE").collect();
    assert_eq!(msgs.len(), 1, "{inbox}");

    // off in the config file: no check
    let (app, st) = lantern("updates-off", true).await;
    st.releases.updates.set_url(&format!("{feed}/repos/Krifmaz/lantern/releases"));
    call(&app, "PATCH", "/config", Some(json!({ "check_for_updates": false }))).await;
    let before = hits.lock().unwrap().len();
    st.releases.run_update_job_once().await;
    assert_eq!(hits.lock().unwrap().len(), before);
}
