//! The account vault through its HTTP API: secrets stay out of answers and off the disk, reveal needs the
//! password again, backups restore, and a vault whose key is gone or wrong says so in words.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

/// The password the tests' owner signs in with.
const PASSWORD: &str = "correct horse battery";

/// A fresh data folder for test `name`.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("app-vault-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("static")).unwrap();
    std::fs::write(dir.join("static/index.html"), "<!doctype html><title>app</title>").unwrap();
    dir
}

/// The server on `dir`, with `passphrase` as APP_VAULT_PASSPHRASE when given.
async fn server_on(dir: &Path, passphrase: Option<&str>) -> (Router, server::AppState) {
    let passphrase = passphrase.map(String::from);
    let st = server::AppState::open(&dir.join("data"), dir.join("static"), move |name| match name {
        "APP_API_KEY" => Some("test-key".to_string()),
        "APP_VAULT_PASSPHRASE" => passphrase.clone(),
        _ => None,
    })
    .await
    .unwrap();
    server::auth::ensure_owner(&st, None, Some(PASSWORD.into())).unwrap();
    (server::app(st.clone()), st)
}

/// One request; the status and the body as JSON (`Null` when it is not JSON), and the raw body.
async fn call(app: &Router, req: Request<Body>) -> (StatusCode, Value, String) {
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&b).unwrap_or(Value::Null), String::from_utf8_lossy(&b).into_owned())
}

/// A request as the owner, through the API key.
fn owner(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(path).header("x-api-key", "test-key");
    match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// A restore form: the backup `file`, the account password, the backup passphrase and whether it is a dry run.
fn restore_form(file: &str, password: &str, passphrase: &str, dry_run: bool) -> Request<Body> {
    let mut body = String::new();
    for (k, v) in [("file", file), ("password", password), ("passphrase", passphrase), ("dry_run", if dry_run { "true" } else { "false" })]
    {
        body.push_str(&format!("--XX\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"));
    }
    body.push_str("--XX--\r\n");
    Request::post("/api/vault/restore")
        .header("x-api-key", "test-key")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=XX")
        .body(Body::from(body))
        .unwrap()
}

/// The account the tests save: a tracker login with every kind of secret.
fn tracker() -> Value {
    json!({
        "kind": "tracker",
        "name": "Test Tracker",
        "site": "lantern-test",
        "fields": {
            "username": "azule",
            "password": "pw-0f3c9a-secret",
            "cookie": "uid=1; pass=cookie-77ab-secret",
            "passkey": "passkey-5d1e-secret",
            "totp_seed": "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
            "invites": "2 invites left"
        }
    })
}

/// Every secret value of [`tracker`].
const SECRETS: &[&str] = &["pw-0f3c9a-secret", "cookie-77ab-secret", "passkey-5d1e-secret", "GEZDGNBVGY3TQOJQ", "2 invites left"];

#[tokio::test]
async fn secrets_stay_out_of_answers_and_off_the_disk() {
    let dir = folder("secrets");
    let (app, _) = server_on(&dir, None).await;
    let (s, v, _) = call(&app, owner("GET", "/api/vault", None)).await;
    assert_eq!((s, v["open"].as_bool(), v["key_source"].as_str()), (StatusCode::OK, Some(true), Some("file")));
    let (s, item, body) = call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let id = item["id"].as_str().unwrap().to_string();
    assert_eq!(item["fields"], json!({ "username": "azule" }));
    assert_eq!(item["secrets"], json!(["cookie", "invites", "passkey", "password", "totp_seed"]));
    let (_, _, listed) = call(&app, owner("GET", "/api/vault/items", None)).await;
    for answer in [&body, &listed] {
        for secret in SECRETS {
            assert!(!answer.contains(secret), "{secret} in {answer}");
        }
    }
    // nothing readable in the database file (or its journal)
    for f in std::fs::read_dir(dir.join("data")).unwrap() {
        let bytes = std::fs::read(f.unwrap().path()).unwrap();
        for secret in SECRETS {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()), "{secret} on disk");
        }
    }
    // a change that sends a secret back masked or empty keeps it
    let change = json!({ "name": "Renamed", "fields": { "password": "••••••", "cookie": "" } });
    let (s, v, _) = call(&app, owner("PUT", &format!("/api/vault/items/{id}"), Some(change))).await;
    assert_eq!((s, v["name"].as_str()), (StatusCode::OK, Some("Renamed")));
    let (s, v, _) = call(&app, owner("POST", &format!("/api/vault/items/{id}/reveal"), Some(json!({ "password": PASSWORD })))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["fields"]["password"], "pw-0f3c9a-secret");
    assert_eq!(v["fields"]["cookie"], "uid=1; pass=cookie-77ab-secret");
    assert_eq!(v["totp"]["code"].as_str().map(str::len), Some(6));
    assert!(v["revealed_at"].is_number());
    let (s, v, _) = call(&app, owner("DELETE", &format!("/api/vault/items/{id}"), None)).await;
    assert_eq!((s, v["ok"].as_bool()), (StatusCode::OK, Some(true)));
    let (s, v, _) = call(&app, owner("GET", &format!("/api/vault/items/{id}"), None)).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::NOT_FOUND, Some("That account isn't in the vault.")));
    let (s, v, _) = call(&app, owner("POST", "/api/vault/items", Some(json!({ "name": "x", "fields": { "pin": "1" } })))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("The vault has no field called pin.")));
}

#[tokio::test]
async fn revealing_needs_the_owners_password() {
    let dir = folder("reveal");
    let (app, st) = server_on(&dir, None).await;
    let (_, item, _) = call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    let reveal = format!("/api/vault/items/{}/reveal", item["id"].as_str().unwrap());
    let (s, v, _) = call(&app, owner("POST", &reveal, Some(json!({})))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["detail"], "That isn't your password. Type the password you sign in to Lantern with.");
    // an admin who is not the owner can list but never reveal
    server::auth::create_user(&st.db, "helper", "helper-password", "admin").unwrap();
    let login = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "username": "helper", "password": "helper-password" }).to_string()))
        .unwrap();
    let r = app.clone().oneshot(login).await.unwrap();
    let cookie = r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let as_helper = |method: &str, path: &str, body: Value| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, &cookie)
            .header("x-requested-with", server::NAME)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (s, v, _) = call(&app, as_helper("GET", "/api/vault/items", Value::Null)).await;
    assert_eq!((s, v["items"].as_array().map(Vec::len)), (StatusCode::OK, Some(1)));
    let (s, _, _) = call(&app, as_helper("POST", &reveal, json!({ "password": "helper-password" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // five wrong passwords, then a wait even for the right one
    for _ in 0..4 {
        let (s, _, _) = call(&app, owner("POST", &reveal, Some(json!({ "password": "wrong" })))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, v, _) = call(&app, owner("POST", &reveal, Some(json!({ "password": PASSWORD })))).await;
    assert_eq!(
        (s, v["detail"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("Too many wrong passwords. Wait five minutes, then try again."))
    );
}

#[tokio::test]
async fn backups_are_encrypted_and_restore_with_a_dry_run_first() {
    let dir = folder("backup");
    let (app, _) = server_on(&dir, None).await;
    let (_, item, _) = call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    let id = item["id"].as_str().unwrap().to_string();
    let (s, v, _) = call(&app, owner("POST", "/api/vault/backup", Some(json!({ "password": PASSWORD, "passphrase": "short" })))).await;
    assert_eq!((s, v["detail"].as_str()), (StatusCode::BAD_REQUEST, Some("The backup passphrase must be at least 12 characters.")));
    let passphrase = "a long backup passphrase";
    let r = app
        .clone()
        .oneshot(owner("POST", "/api/vault/backup", Some(json!({ "password": PASSWORD, "passphrase": passphrase }))))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let disposition = r.headers().get(header::CONTENT_DISPOSITION).unwrap().to_str().unwrap().to_string();
    assert!(disposition.starts_with(&format!("attachment; filename=\"{}-vault-", server::NAME)), "{disposition}");
    let file = String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
    let parsed: Value = serde_json::from_str(&file).unwrap();
    assert_eq!((parsed["format"].as_str(), parsed["items"].as_u64()), (Some("lantern-vault-backup"), Some(1)));
    for secret in SECRETS.iter().chain(&["Test Tracker", "azule"]) {
        assert!(!file.contains(secret), "{secret} in the backup");
    }
    // everything gone here, then the backup brings it back: a dry run first changes nothing
    call(&app, owner("DELETE", &format!("/api/vault/items/{id}"), None)).await;
    let (s, v, _) = call(&app, restore_form(&file, PASSWORD, "the wrong passphrase", true)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["detail"].as_str().unwrap().starts_with("The passphrase doesn't open this backup."));
    let (s, v, _) = call(&app, restore_form(&file, PASSWORD, passphrase, true)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["result"]["added"], json!(["Test Tracker"]));
    assert_eq!(v["message"], "A restore would do this: 1 account added. Nothing has changed yet.");
    let (_, v, _) = call(&app, owner("GET", "/api/vault/items", None)).await;
    assert_eq!(v["items"], json!([]));
    let (s, v, _) = call(&app, restore_form(&file, PASSWORD, passphrase, false)).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::OK, Some("Restored: 1 account added.")));
    let (_, v, _) = call(&app, owner("POST", &format!("/api/vault/items/{id}/reveal"), Some(json!({ "password": PASSWORD })))).await;
    assert_eq!(v["fields"]["passkey"], "passkey-5d1e-secret");
    // a second restore finds it the same; a change made here later is kept
    let (_, v, _) = call(&app, restore_form(&file, PASSWORD, passphrase, false)).await;
    assert_eq!(v["result"]["unchanged"], json!(["Test Tracker"]));
    call(&app, owner("PUT", &format!("/api/vault/items/{id}"), Some(json!({ "fields": { "password": "newer" } })))).await;
    let (_, v, _) = call(&app, restore_form(&file, PASSWORD, passphrase, false)).await;
    assert_eq!(v["result"]["kept_newer"], json!(["Test Tracker"]));
    let (s, v, _) = call(&app, restore_form("{\"hello\": 1}", PASSWORD, passphrase, false)).await;
    assert_eq!(
        (s, v["detail"].as_str()),
        (StatusCode::BAD_REQUEST, Some("This file isn't a Lantern vault backup. Choose the .json file that Back up made."))
    );
}

#[tokio::test]
async fn a_backup_restores_on_another_server() {
    let (a, b) = (folder("move-a"), folder("move-b"));
    let (app, _) = server_on(&a, None).await;
    call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    let r = app
        .oneshot(owner("POST", "/api/vault/backup", Some(json!({ "password": PASSWORD, "passphrase": "moving to a new box" }))))
        .await
        .unwrap();
    let file = String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
    let (other, _) = server_on(&b, Some("another server's passphrase")).await;
    let (s, v, _) = call(&other, restore_form(&file, PASSWORD, "moving to a new box", false)).await;
    assert_eq!((s, v["message"].as_str()), (StatusCode::OK, Some("Restored: 1 account added.")), "{v}");
}

#[tokio::test]
async fn a_missing_key_or_wrong_passphrase_locks_the_vault_in_words() {
    // the key file: gone after a restart → locked, items still listed, nothing new can be saved
    let dir = folder("keyfile");
    let (app, _) = server_on(&dir, None).await;
    call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    let key = std::fs::read(dir.join("data/vault.key")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("data/vault.key")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    std::fs::remove_file(dir.join("data/vault.key")).unwrap();
    let (app, _) = server_on(&dir, None).await;
    let (_, v, _) = call(&app, owner("GET", "/api/vault", None)).await;
    assert_eq!(v["open"], false);
    assert!(v["locked_because"].as_str().unwrap().starts_with("The vault key file "), "{v}");
    let (_, v, _) = call(&app, owner("GET", "/api/vault/items", None)).await;
    assert_eq!(v["items"][0]["name"], "Test Tracker");
    let (s, v, _) = call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert!(v["detail"].as_str().unwrap().contains("is missing. Put back the copy from your backup"));
    // the key put back opens it again
    std::fs::write(dir.join("data/vault.key"), key).unwrap();
    let (app, _) = server_on(&dir, None).await;
    let (_, v, _) = call(&app, owner("GET", "/api/vault", None)).await;
    assert_eq!(v["open"], true);

    // the passphrase: no key file is made, a wrong one locks, the right one opens
    let dir = folder("passphrase");
    let (app, _) = server_on(&dir, Some("the vault passphrase")).await;
    let (_, v, _) = call(&app, owner("GET", "/api/vault", None)).await;
    assert_eq!((v["open"].as_bool(), v["key_source"].as_str()), (Some(true), Some("passphrase")));
    assert!(!dir.join("data/vault.key").exists());
    call(&app, owner("POST", "/api/vault/items", Some(tracker()))).await;
    let (app, _) = server_on(&dir, Some("not the passphrase")).await;
    let (_, v, _) = call(&app, owner("GET", "/api/vault", None)).await;
    assert_eq!(
        v["locked_because"],
        "The vault can't be opened: APP_VAULT_PASSPHRASE isn't the passphrase it was made with. Set the right one and restart Lantern."
    );
    let (app, _) = server_on(&dir, None).await;
    let (_, v, _) = call(&app, owner("GET", "/api/vault", None)).await;
    assert!(v["locked_because"].as_str().unwrap().starts_with("The vault key file "), "{v}");
    let (app, _) = server_on(&dir, Some("the vault passphrase")).await;
    let (_, v, _) = call(&app, owner("GET", "/api/vault/items", None)).await;
    let id = v["items"][0]["id"].as_str().unwrap().to_string();
    let (s, v, _) = call(&app, owner("POST", &format!("/api/vault/items/{id}/reveal"), Some(json!({ "password": PASSWORD })))).await;
    assert_eq!((s, v["fields"]["username"].as_str()), (StatusCode::OK, Some("azule")));
}

#[tokio::test]
async fn indexer_logins_and_cookies_are_sealed_at_rest() {
    let dir = folder("indexers");
    let (app, st) = server_on(&dir, None).await;
    let body =
        json!({ "kind": "newznab", "name": "Usenet", "base_url": "http://127.0.0.1:9", "settings": { "apikey": "apikey-9c1d-secret" } });
    let (s, v, _) = call(&app, owner("POST", "/api/indexers", Some(body))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_i64().unwrap();
    assert_eq!(st.indexers.get(id).unwrap().settings.get("apikey").map(String::as_str), Some("apikey-9c1d-secret"));
    // one saved before Lantern sealed them is sealed at the next start
    use kit_server_core::Db;
    st.db.execute_batch(
        "INSERT INTO indexers (kind, definition, name, base_url, settings, cookies, added) VALUES ('newznab','','Old','http://127.0.0.1:9','{\"apikey\":\"old-apikey-secret\"}','uid=1; pass=old-cookie-secret',0)",
    )
    .unwrap();
    let (_, st) = server_on(&dir, None).await;
    let old = st.indexers.list().unwrap().into_iter().find(|i| i.name == "Old").unwrap();
    assert_eq!(old.settings.get("apikey").map(String::as_str), Some("old-apikey-secret"));
    assert_eq!(old.cookies, "uid=1; pass=old-cookie-secret");
    for f in std::fs::read_dir(dir.join("data")).unwrap() {
        let bytes = std::fs::read(f.unwrap().path()).unwrap();
        for secret in ["apikey-9c1d-secret", "old-apikey-secret", "old-cookie-secret"] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()), "{secret} on disk");
        }
    }
    // with the vault locked, the indexer says why instead of signing in with nothing
    std::fs::remove_file(dir.join("data/vault.key")).unwrap();
    let (app, st) = server_on(&dir, None).await;
    assert!(!st.indexers.get(id).unwrap().locked.is_empty());
    let (_, v, _) = call(&app, owner("POST", &format!("/api/indexers/{id}/test"), None)).await;
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().starts_with("Usenet's saved login can't be opened: The vault key file"), "{v}");
}
