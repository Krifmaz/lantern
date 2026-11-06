// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Config/HostConfigController.cs, HostConfigResource.cs,
// UiConfigController.cs, UiConfigResource.cs, DevelopmentConfigController.cs, DevelopmentConfigResource.cs,
// ConfigController.cs, src/NzbDrone.Core/Configuration/ConfigFileProvider.cs, ConfigService.cs), GPL-3.0.

//! Prowlarr's three settings pages: General (`/api/v1/config/host`), UI (`/api/v1/config/ui`) and Development
//! (`/api/v1/config/development`), with Prowlarr's fields, defaults and checks.
//!
//! The fields Lantern already has a setting for use it: the port (`APP_PORT`, read only), the instance name
//! (`app_name`), the application URL (`public_url`), the proxy (`site_proxy`), the API key (`api_key`, changed
//! only by the ResetApiKey command, as in Prowlarr) and the owner's username and password. The others are kept as
//! Prowlarr keeps them, in its own settings table.

use super::validation::{self, allowed_hosts, display_name, ip_network, Failure};
use super::{accepted, body_json, check, config_get, config_set, fail, localization, ApiError, ApiResult, APP_NAME, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Map, Value};

/// What the owner's saved password is sent back as; sent back unchanged, it keeps the password.
pub const PASSWORD_MASK: &str = "••••••••";

/// The settings routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/config/host", get(get_host))
        .route("/api/v1/config/host/{id}", get(get_host_by_id).put(put_host))
        .route("/api/v1/config/ui", get(get_ui))
        .route("/api/v1/config/ui/{id}", get(get_ui_by_id).put(put_ui))
        .route("/api/v1/config/development", get(get_dev))
        .route("/api/v1/config/development/{id}", get(get_dev_by_id).put(put_dev))
        .route("/api/v1/config/downloadclient", get(get_download_client).put(put_download_client))
        .route("/api/v1/config/downloadclient/{id}", get(get_download_client).put(put_download_client))
}

/// `GET /api/v1/config/downloadclient[/{id}]` (`DownloadClientConfigResource`): Prowlarr's download client
/// settings page has no settings of its own, so only its id.
async fn get_download_client(_: V1) -> Json<Value> {
    Json(json!({ "id": 1 }))
}

/// `PUT /api/v1/config/downloadclient[/{id}]`: nothing to save; the resource back. 202.
async fn put_download_client(_: V1) -> (axum::http::StatusCode, Json<Value>) {
    (axum::http::StatusCode::ACCEPTED, Json(json!({ "id": 1 })))
}

// ---- reading Prowlarr's settings, with its defaults ----

/// A saved setting as text, else `default`.
fn text(db: &SqliteDb, key: &str, default: &str) -> String {
    config_get(db, key).unwrap_or_else(|| default.to_string())
}

/// A saved setting as a number, else `default`.
fn int(db: &SqliteDb, key: &str, default: i64) -> i64 {
    config_get(db, key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// A saved setting as yes or no, else `default`.
fn boolean(db: &SqliteDb, key: &str, default: bool) -> bool {
    config_get(db, key).map(|v| v.trim().eq_ignore_ascii_case("true")).unwrap_or(default)
}

/// `BindAddress`: `*` (every address) when not set or empty.
pub fn bind_address(db: &SqliteDb) -> String {
    let v = text(db, "bindaddress", "*");
    if v.trim().is_empty() {
        "*".into()
    } else {
        v
    }
}

/// `AuthenticationMethod`: Lantern always signs people in with its own form, so `forms` until another one is
/// saved; the old `basic` reads as `forms`, as in Prowlarr.
pub fn authentication_method(db: &SqliteDb) -> String {
    let v = enum_name(&json!(text(db, "authenticationmethod", "forms")), AUTHENTICATION_TYPES).unwrap_or_else(|| "forms".into());
    if v == "basic" {
        "forms".into()
    } else {
        v
    }
}

/// `Branch`, in small letters (default `master`).
pub fn branch(db: &SqliteDb) -> String {
    text(db, "branch", "master").to_lowercase()
}

/// `UrlBase`: empty, or the path with one leading `/` and none at the end.
pub fn url_base(db: &SqliteDb) -> String {
    let v = text(db, "urlbase", "");
    let v = v.trim_matches('/');
    if v.trim().is_empty() {
        v.to_string()
    } else {
        format!("/{v}")
    }
}

/// `InstanceName`: Lantern's `app_name`, or `Lantern` when the name doesn't contain it (Prowlarr keeps its own
/// name in every instance name the same way).
pub fn instance_name(st: &AppState) -> String {
    let name = st.app_name();
    if name.to_lowercase().contains(&APP_NAME.to_lowercase()) {
        name
    } else {
        APP_NAME.to_string()
    }
}

/// `BackupFolder` (default `Backups`, inside the data folder).
pub fn backup_folder_setting(db: &SqliteDb) -> String {
    text(db, "backupfolder", "Backups")
}

/// `BackupRetention` in days (default 28).
pub fn backup_retention(db: &SqliteDb) -> i64 {
    int(db, "backupretention", 28)
}

/// `LogLevel` (default `debug`), in lower case.
pub fn log_level(db: &SqliteDb) -> String {
    text(db, "loglevel", "debug").to_lowercase()
}

/// `LogSizeLimit` in megabytes (default 1, 1 to 10).
pub fn log_size_limit(db: &SqliteDb) -> i64 {
    int(db, "logsizelimit", 1).clamp(1, 10)
}

/// `LogRotate`: how many old log files are kept (default 50).
pub fn log_rotate(db: &SqliteDb) -> i64 {
    int(db, "logrotate", 50)
}

/// `UILanguage` (default `en`).
pub fn ui_language(db: &SqliteDb) -> String {
    text(db, "uilanguage", "en")
}

/// `AuthenticationRequired` (default `enabled`).
pub fn authentication_required(db: &SqliteDb) -> String {
    enum_name(&json!(text(db, "authenticationrequired", "enabled")), AUTHENTICATION_REQUIRED).unwrap_or_else(|| "enabled".into())
}

/// `AllowedHosts` (default empty: every host).
pub fn allowed_hosts_setting(db: &SqliteDb) -> String {
    text(db, "allowedhosts", "")
}

// ---- Prowlarr's enums: their names as written in JSON, and their numbers ----

/// `AuthenticationType`.
const AUTHENTICATION_TYPES: &[(&str, i64)] = &[("none", 0), ("basic", 1), ("forms", 2), ("external", 3)];
/// `AuthenticationRequiredType`.
const AUTHENTICATION_REQUIRED: &[(&str, i64)] = &[("enabled", 0), ("disabledForLocalAddresses", 1)];
/// `UpdateMechanism`.
const UPDATE_MECHANISMS: &[(&str, i64)] = &[("builtIn", 0), ("script", 1), ("external", 10), ("apt", 11), ("docker", 12)];
/// `ProxyType`.
const PROXY_TYPES: &[(&str, i64)] = &[("http", 0), ("socks4", 1), ("socks5", 2)];
/// `CertificateValidationType`.
const CERTIFICATE_VALIDATION: &[(&str, i64)] = &[("enabled", 0), ("disabledForLocalAddresses", 1), ("disabled", 2)];

/// The enum value `v` names (in any case) or numbers, by its JSON name; `None` when it is neither.
fn enum_name(v: &Value, names: &[(&str, i64)]) -> Option<String> {
    match v {
        Value::String(s) => names.iter().find(|(n, _)| n.eq_ignore_ascii_case(s.trim())).map(|(n, _)| n.to_string()),
        Value::Number(n) => n.as_i64().and_then(|i| names.iter().find(|(_, x)| *x == i)).map(|(n, _)| n.to_string()),
        _ => None,
    }
}

// ---- reading a resource from a request, the way .NET fills a class from JSON ----

/// A request's resource, read field by field with .NET's defaults for what is missing (null text, 0, false, an
/// enum's first value), its field names in any case.
struct Body(Map<String, Value>);

impl Body {
    /// The body of a PUT, or Prowlarr's 400 when there is none or it is not an object.
    fn read(bytes: &[u8]) -> ApiResult<Body> {
        match body_json(bytes)? {
            Value::Object(m) => Ok(Body(m.into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect())),
            _ => Err(fail(400, "The request body must be a JSON object")),
        }
    }

    fn raw(&self, key: &str) -> &Value {
        self.0.get(&key.to_lowercase()).unwrap_or(&Value::Null)
    }

    /// A text field; `None` when missing or null.
    fn text(&self, key: &str) -> Option<String> {
        match self.raw(key) {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }

    /// A number field (0 when missing); 400 when it is not a number.
    fn int(&self, key: &str) -> ApiResult<i64> {
        match self.raw(key) {
            Value::Null => Ok(0),
            Value::Number(n) => n.as_i64().ok_or_else(|| bad_value(key)),
            Value::String(s) => s.trim().parse().map_err(|_| bad_value(key)),
            _ => Err(bad_value(key)),
        }
    }

    /// A yes-or-no field (false when missing); 400 when it is neither.
    fn bool(&self, key: &str) -> ApiResult<bool> {
        match self.raw(key) {
            Value::Null => Ok(false),
            Value::Bool(b) => Ok(*b),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(true),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(false),
            _ => Err(bad_value(key)),
        }
    }

    /// An enum field (its first value when missing); 400 when it names none.
    fn enumeration(&self, key: &str, names: &[(&str, i64)]) -> ApiResult<String> {
        match self.raw(key) {
            Value::Null => Ok(names[0].0.to_string()),
            v => enum_name(v, names).ok_or_else(|| bad_value(key)),
        }
    }

    /// The resource's id: the body's, or the address's when the body has none (as Prowlarr maps it on PUT).
    fn id(&self, path_id: i64) -> ApiResult<i64> {
        let id = self.int("id")?;
        Ok(if id == 0 { path_id } else { id })
    }
}

/// The 400 for a field whose value has the wrong type.
fn bad_value(key: &str) -> ApiError {
    fail(400, format!("The value of '{}' has the wrong type.", display_name(&upper_first(key))))
}

/// `port` → `Port`.
fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Saves the fields of a resource that changed, by Prowlarr's setting names (small letters).
fn save(db: &SqliteDb, values: &[(&str, Option<String>)]) -> ApiResult<()> {
    for (key, value) in values {
        if let Some(v) = value {
            config_set(db, key, v)?;
        }
    }
    Ok(())
}

// ---- General (config/host) ----

/// The owner's account: its username, and whether it has a password.
fn owner(db: &SqliteDb) -> Option<kit_server_core::Row> {
    db.one("SELECT * FROM users WHERE role='owner' ORDER BY id LIMIT 1", p![]).ok().flatten()
}

/// The proxy in effect: `(enabled, type, host, port, username, password)`, from Lantern's `site_proxy` when it
/// is set, else Prowlarr's saved fields with `ProxyEnabled` off.
fn proxy(st: &AppState) -> (bool, String, String, i64, String, String) {
    let saved = (
        enum_name(&json!(text(&st.db, "proxytype", "http")), PROXY_TYPES).unwrap_or_else(|| "http".into()),
        text(&st.db, "proxyhostname", ""),
        int(&st.db, "proxyport", 8080),
        text(&st.db, "proxyusername", ""),
        text(&st.db, "proxypassword", ""),
    );
    let set = st.settings.str("site_proxy");
    let Ok(u) = url::Url::parse(set.trim()) else { return (false, saved.0, saved.1, saved.2, saved.3, saved.4) };
    let kind = match u.scheme() {
        "socks5" | "socks5h" => "socks5",
        "socks4" | "socks4a" => "socks4",
        _ => "http",
    };
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    (
        true,
        kind.to_string(),
        u.host_str().unwrap_or("").trim_matches(['[', ']']).to_string(),
        u.port_or_known_default().map(i64::from).unwrap_or(8080),
        decode(u.username()),
        decode(u.password().unwrap_or("")),
    )
}

/// Lantern's `site_proxy` for Prowlarr's proxy fields: `<type>://[user[:password]@]host:port`.
fn proxy_url(kind: &str, host: &str, port: i64, user: &str, password: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let auth = match (user.is_empty(), password.is_empty()) {
        (true, _) => String::new(),
        (false, true) => format!("{}@", enc(user)),
        (false, false) => format!("{}:{}@", enc(user), enc(password)),
    };
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    format!("{kind}://{auth}{host}:{port}")
}

/// Prowlarr's `HostConfigResource`, with Lantern's values.
pub fn host_json(st: &AppState) -> Value {
    let db = &st.db;
    let owner = owner(db);
    let (proxy_enabled, proxy_type, proxy_host, proxy_port, proxy_user, proxy_password) = proxy(st);
    json!({
        "id": 1,
        "bindAddress": bind_address(db),
        "port": crate::port(),
        "sslPort": int(db, "sslport", 6969),
        "enableSsl": boolean(db, "enablessl", false),
        "launchBrowser": boolean(db, "launchbrowser", true),
        "authenticationMethod": authentication_method(db),
        "authenticationRequired": authentication_required(db),
        "allowedHosts": allowed_hosts_setting(db),
        "analyticsEnabled": boolean(db, "analyticsenabled", true),
        "username": owner.as_ref().map(|o| o.s("username")).unwrap_or_default(),
        "password": if owner.is_some() { PASSWORD_MASK } else { "" },
        "passwordConfirmation": "",
        "logLevel": text(db, "loglevel", "debug").to_lowercase(),
        "logSizeLimit": int(db, "logsizelimit", 1).clamp(0, 10),
        "consoleLogLevel": text(db, "consoleloglevel", ""),
        "branch": branch(db),
        "apiKey": st.settings.str("api_key"),
        "sslCertPath": text(db, "sslcertpath", ""),
        "sslCertPassword": text(db, "sslcertpassword", ""),
        "urlBase": url_base(db),
        "trustedNetworks": text(db, "trustednetworks", ""),
        "instanceName": instance_name(st),
        "applicationUrl": st.settings.str("public_url"),
        "updateAutomatically": boolean(db, "updateautomatically", false),
        "updateMechanism": enum_name(&json!(text(db, "updatemechanism", "builtIn")), UPDATE_MECHANISMS).unwrap_or_else(|| "builtIn".into()),
        "updateScriptPath": text(db, "updatescriptpath", ""),
        "proxyEnabled": proxy_enabled,
        "proxyType": proxy_type,
        "proxyHostname": proxy_host,
        "proxyPort": proxy_port,
        "proxyUsername": proxy_user,
        "proxyPassword": proxy_password,
        "proxyBypassFilter": text(db, "proxybypassfilter", ""),
        "proxyBypassLocalAddresses": boolean(db, "proxybypasslocaladdresses", true),
        "certificateValidation": enum_name(&json!(text(db, "certificatevalidation", "enabled")), CERTIFICATE_VALIDATION).unwrap_or_else(|| "enabled".into()),
        "backupFolder": backup_folder_setting(db),
        "backupInterval": int(db, "backupinterval", 7),
        "backupRetention": int(db, "backupretention", 28),
        "historyCleanupDays": int(db, "historycleanupdays", 30),
        // Prowlarr's mapper never fills it in, so it always reads as false
        "trustCgnatIpAddresses": false,
    })
}

async fn get_host(State(st): State<AppState>, _: V1) -> Json<Value> {
    Json(host_json(&st))
}

async fn get_host_by_id(State(st): State<AppState>, _: V1, Path(_id): Path<i64>) -> Json<Value> {
    Json(host_json(&st))
}

/// A host resource read from a request, for its checks.
struct Host {
    id: i64,
    bind_address: Option<String>,
    port: i64,
    ssl_port: i64,
    enable_ssl: bool,
    authentication_method: String,
    authentication_required: String,
    allowed_hosts: Option<String>,
    username: Option<String>,
    password: Option<String>,
    password_confirmation: Option<String>,
    log_size_limit: i64,
    branch: Option<String>,
    ssl_cert_path: Option<String>,
    url_base: Option<String>,
    trusted_networks: Option<String>,
    instance_name: Option<String>,
    update_mechanism: String,
    update_script_path: Option<String>,
    backup_folder: Option<String>,
    backup_interval: i64,
    backup_retention: i64,
}

/// Prowlarr's checks of a host resource (its `SharedValidator`, then the `PutValidator`'s id), in its order.
fn check_host(h: &Host) -> Vec<Option<Failure>> {
    let s = |v: &Option<String>| json!(v);
    let blank = |v: &Option<String>| v.as_deref().is_none_or(|x| x.trim().is_empty());
    let mut out = Vec::new();
    let bind = h.bind_address.as_deref();
    if bind != Some("*") && bind != Some("localhost") && !bind.is_some_and(validation::is_valid_ip_address) {
        let f = Failure::new("BindAddress", s(&h.bind_address), "PredicateValidator", "Must contain wildcard (*) or a valid IP Address");
        out.push(Some(f));
    }
    out.push(validation::valid_port("Port", h.port));
    out.push(validation::not_null("AllowedHosts", &s(&h.allowed_hosts)));
    if h.authentication_required != "enabled" && allowed_hosts::parse(h.allowed_hosts.as_deref()).is_empty() {
        let msg = "Allowed Hosts is required when 'Authentication Required' is not 'Enabled'";
        out.push(Some(Failure::new("AllowedHosts", s(&h.allowed_hosts), "PredicateValidator", msg)));
    }
    if !blank(&h.allowed_hosts) {
        let hosts = allowed_hosts::parse(h.allowed_hosts.as_deref());
        if hosts.is_empty() || !hosts.iter().all(|x| allowed_hosts::is_valid_host(Some(x))) {
            let msg = "Must be a comma separated list of host names or IP addresses only, without a protocol or port";
            out.push(Some(Failure::new("AllowedHosts", s(&h.allowed_hosts), "PredicateValidator", msg)));
        }
        if hosts.iter().any(|x| allowed_hosts::is_bind_address(x)) {
            let msg = "0.0.0.0 and [::] are not host names, leave empty to allow all hosts";
            out.push(Some(Failure::new("AllowedHosts", s(&h.allowed_hosts), "PredicateValidator", msg)));
        }
    }
    if let Some(u) = h.url_base.as_deref().filter(|u| !validation::is_valid_url_base(u)) {
        let msg = "Must be a valid URL path (ie: '/sonarr')";
        out.push(Some(Failure::new("UrlBase", json!(u), "RegularExpressionValidator", msg)));
    }
    if !ip_network::is_valid_list(h.trusted_networks.as_deref()) {
        let msg = "Must be a comma separated list of IP addresses or networks in CIDR notation, such as 172.17.0.1, 10.0.0.0/8 or fc00::/7";
        out.push(Some(Failure::new("TrustedNetworks", s(&h.trusted_networks), "PredicateValidator", msg)));
    }
    if let Some(name) = h.instance_name.as_deref().filter(|n| !n.trim().is_empty()) {
        if !name.to_lowercase().contains(&APP_NAME.to_lowercase()) {
            let msg = format!("Must contain {APP_NAME}");
            out.push(Some(Failure::new("InstanceName", json!(name), "RegularExpressionValidator", msg)));
        }
    }
    if h.authentication_method == "forms" {
        out.push(validation::not_empty("Username", h.username.as_deref()));
        out.push(validation::not_empty("Password", h.password.as_deref()));
    }
    if h.authentication_method == "basic" {
        let msg = "'Basic' is no longer supported, switch to 'Forms' instead.";
        out.push(Some(Failure::new("AuthenticationMethod", json!("basic"), "NotEqualValidator", msg)));
    }
    let matching = h.password.as_deref() == Some(PASSWORD_MASK) || h.password == h.password_confirmation;
    if !matching {
        let f = Failure::new("PasswordConfirmation", s(&h.password_confirmation), "PredicateValidator", "Must match Password");
        out.push(Some(f));
    }
    if h.enable_ssl {
        out.push(validation::valid_port("SslPort", h.ssl_port));
        if h.ssl_port == h.port {
            let msg = format!("'{}' must not be equal to '{}'.", display_name("SslPort"), h.port);
            out.push(Some(Failure::new("SslPort", json!(h.ssl_port), "NotEqualValidator", msg).with("ComparisonValue", json!(h.port))));
        }
        let path = h.ssl_cert_path.as_deref();
        if let Some(f) = validation::not_empty("SslCertPath", path) {
            out.push(Some(f));
        } else if !path.is_some_and(validation::is_valid_path) {
            out.push(Some(Failure::new(
                "SslCertPath",
                s(&h.ssl_cert_path),
                "PathValidator",
                format!("Invalid Path: '{}'", path.unwrap_or("")),
            )));
        } else if !path.is_some_and(|p| std::path::Path::new(p).is_file()) {
            let msg = format!("File '{}' does not exist", path.unwrap_or(""));
            out.push(Some(Failure::new("SslCertPath", s(&h.ssl_cert_path), "FileExistsValidator", msg)));
        }
    }
    out.push(validation::between("LogSizeLimit", h.log_size_limit, 1, 10));
    if blank(&h.branch) {
        let msg = "Branch name is required, 'master' is the default";
        out.push(Some(Failure::new("Branch", s(&h.branch), "NotEmptyValidator", msg)));
    }
    if h.update_mechanism == "script" && !h.update_script_path.as_deref().is_some_and(validation::is_valid_path) {
        let p = h.update_script_path.clone().unwrap_or_default();
        out.push(Some(Failure::new("UpdateScriptPath", json!(p), "PathValidator", format!("Invalid Path: '{p}'"))));
    }
    if let Some(b) = h.backup_folder.as_deref().filter(|b| b.starts_with('/')) {
        if !validation::is_valid_path(b) {
            out.push(Some(Failure::new("BackupFolder", json!(b), "PathValidator", format!("Invalid Path: '{b}'"))));
        }
    }
    out.push(validation::between("BackupInterval", h.backup_interval, 1, 7));
    out.push(validation::between("BackupRetention", h.backup_retention, 1, 90));
    out.push(validation::valid_id(h.id));
    out
}

/// `PUT /api/v1/config/host/{id}`: checks and saves the general settings; 202 with them as saved.
async fn put_host(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, bytes: Bytes) -> ApiResult<Response> {
    let b = Body::read(&bytes)?;
    let h = Host {
        id: b.id(path_id)?,
        bind_address: b.text("bindAddress"),
        port: b.int("port")?,
        ssl_port: b.int("sslPort")?,
        enable_ssl: b.bool("enableSsl")?,
        authentication_method: b.enumeration("authenticationMethod", AUTHENTICATION_TYPES)?,
        authentication_required: b.enumeration("authenticationRequired", AUTHENTICATION_REQUIRED)?,
        allowed_hosts: b.text("allowedHosts"),
        username: b.text("username"),
        password: b.text("password"),
        password_confirmation: b.text("passwordConfirmation"),
        log_size_limit: b.int("logSizeLimit")?,
        branch: b.text("branch"),
        ssl_cert_path: b.text("sslCertPath"),
        url_base: b.text("urlBase"),
        trusted_networks: b.text("trustedNetworks"),
        instance_name: b.text("instanceName"),
        update_mechanism: b.enumeration("updateMechanism", UPDATE_MECHANISMS)?,
        update_script_path: b.text("updateScriptPath"),
        backup_folder: b.text("backupFolder"),
        backup_interval: b.int("backupInterval")?,
        backup_retention: b.int("backupRetention")?,
    };
    check(check_host(&h))?;
    let db = &st.db;
    let some = |v: String| Some(v);
    save(
        db,
        &[
            ("bindaddress", h.bind_address.clone()),
            ("sslport", some(h.ssl_port.to_string())),
            ("enablessl", some(h.enable_ssl.to_string())),
            ("launchbrowser", some(b.bool("launchBrowser")?.to_string())),
            ("authenticationmethod", some(h.authentication_method.clone())),
            ("authenticationrequired", some(h.authentication_required.clone())),
            ("allowedhosts", h.allowed_hosts.clone()),
            ("analyticsenabled", some(b.bool("analyticsEnabled")?.to_string())),
            ("loglevel", b.text("logLevel")),
            ("logsizelimit", some(h.log_size_limit.to_string())),
            ("consoleloglevel", b.text("consoleLogLevel")),
            ("branch", h.branch.clone()),
            ("sslcertpath", h.ssl_cert_path.clone()),
            ("sslcertpassword", b.text("sslCertPassword")),
            ("urlbase", h.url_base.clone()),
            ("trustednetworks", some(ip_network::normalize_list(h.trusted_networks.as_deref()))),
            ("updateautomatically", some(b.bool("updateAutomatically")?.to_string())),
            ("updatemechanism", some(h.update_mechanism.clone())),
            ("updatescriptpath", h.update_script_path.clone()),
            ("proxytype", some(b.enumeration("proxyType", PROXY_TYPES)?)),
            ("proxyhostname", b.text("proxyHostname")),
            ("proxyport", some(b.int("proxyPort")?.to_string())),
            ("proxyusername", b.text("proxyUsername")),
            ("proxypassword", b.text("proxyPassword")),
            ("proxybypassfilter", b.text("proxyBypassFilter")),
            ("proxybypasslocaladdresses", some(b.bool("proxyBypassLocalAddresses")?.to_string())),
            ("certificatevalidation", some(b.enumeration("certificateValidation", CERTIFICATE_VALIDATION)?)),
            ("backupfolder", h.backup_folder.clone()),
            ("backupinterval", some(h.backup_interval.to_string())),
            ("backupretention", some(h.backup_retention.to_string())),
            ("historycleanupdays", some(b.int("historyCleanupDays")?.to_string())),
            ("trustcgnatipaddresses", some(b.bool("trustCgnatIpAddresses")?.to_string())),
        ],
    )?;
    // the backup task follows the backup interval, as Prowlarr's task manager does when settings are saved
    db.execute("UPDATE prowlarr_tasks SET interval=? WHERE type_name LIKE '%.BackupCommand'", p![super::system::backup_interval(db)])?;
    // the fields that are Lantern settings (an environment variable keeps its own)
    let mut lantern = Map::new();
    if let Some(name) = h.instance_name.clone().filter(|_| !st.settings.env_locked("app_name")) {
        lantern.insert("app_name".into(), json!(name));
    }
    if let Some(url) = b.text("applicationUrl").filter(|_| !st.settings.env_locked("public_url")) {
        lantern.insert("public_url".into(), json!(url));
    }
    if !st.settings.env_locked("site_proxy") {
        let url = if b.bool("proxyEnabled")? {
            let kind = b.enumeration("proxyType", PROXY_TYPES)?;
            let host = b.text("proxyHostname").unwrap_or_default();
            let (user, pw) = (b.text("proxyUsername").unwrap_or_default(), b.text("proxyPassword").unwrap_or_default());
            proxy_url(&kind, &host, b.int("proxyPort")?, &user, &pw)
        } else {
            String::new()
        };
        lantern.insert("site_proxy".into(), json!(url));
    }
    st.settings.set_many(&Value::Object(lantern)).await?;
    // the owner's username and password, as Prowlarr's single user
    if let (Some(user), Some(pw)) = (h.username.as_deref(), h.password.as_deref()) {
        if !user.trim().is_empty() && !pw.trim().is_empty() {
            set_owner(db, user.trim(), pw)?;
        }
    }
    Ok(accepted("/api/v1/config/host/1".into(), host_json(&st)))
}

/// Changes the owner's username, and its password unless it is the mask sent back unchanged.
fn set_owner(db: &SqliteDb, username: &str, password: &str) -> ApiResult<()> {
    let Some(o) = owner(db) else { return Ok(()) };
    let new_password = (password != PASSWORD_MASK).then_some(password);
    crate::auth::validate_credentials(username, new_password.unwrap_or("unchanged password"))?;
    if username != o.s("username") {
        if db.one("SELECT id FROM users WHERE username=? AND id<>?", p![username, o.i("id")])?.is_some() {
            return Err(fail(409, "That username is taken by another account"));
        }
        db.execute("UPDATE users SET username=? WHERE id=?", p![username, o.i("id")])?;
    }
    if let Some(pw) = new_password {
        db.execute("UPDATE users SET password_hash=? WHERE id=?", p![crate::auth::hash_password(pw), o.i("id")])?;
    }
    Ok(())
}

// ---- UI (config/ui) ----

/// Prowlarr's `UiConfigResource`.
pub fn ui_json(db: &SqliteDb) -> Value {
    json!({
        "id": 1,
        "firstDayOfWeek": int(db, "firstdayofweek", 0),
        "calendarWeekColumnHeader": text(db, "calendarweekcolumnheader", "ddd M/D"),
        "shortDateFormat": text(db, "shortdateformat", "MMM D YYYY"),
        "longDateFormat": text(db, "longdateformat", "dddd, MMMM D YYYY"),
        "timeFormat": text(db, "timeformat", "h(:mm)a"),
        "showRelativeDates": boolean(db, "showrelativedates", true),
        "enableColorImpairedMode": boolean(db, "enablecolorimpairedmode", false),
        "uiLanguage": ui_language(db),
        "theme": text(db, "theme", "auto"),
    })
}

async fn get_ui(State(st): State<AppState>, _: V1) -> Json<Value> {
    Json(ui_json(&st.db))
}

async fn get_ui_by_id(State(st): State<AppState>, _: V1, Path(_id): Path<i64>) -> Json<Value> {
    Json(ui_json(&st.db))
}

/// Prowlarr's checks of a UI resource: the language is set and is one Prowlarr offers.
pub fn check_ui_language(language: Option<&str>) -> Vec<Option<Failure>> {
    let mut out = Vec::new();
    if language.is_none_or(|l| l.trim().is_empty()) {
        out.push(Some(Failure::new("UILanguage", json!(language), "NotEmptyValidator", "The UI Language value cannot be empty")));
    }
    if !localization::OPTIONS.iter().any(|(_, v)| Some(*v) == language) {
        out.push(Some(Failure::new("UILanguage", json!(language), "", "Invalid UI Language value")));
    }
    out
}

/// `PUT /api/v1/config/ui/{id}`: checks and saves the UI settings; 202 with them as saved.
async fn put_ui(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, bytes: Bytes) -> ApiResult<Response> {
    let b = Body::read(&bytes)?;
    let language = b.text("uiLanguage");
    let mut failures = check_ui_language(language.as_deref());
    failures.push(validation::valid_id(b.id(path_id)?));
    check(failures)?;
    let some = |v: String| Some(v);
    save(
        &st.db,
        &[
            ("firstdayofweek", some(b.int("firstDayOfWeek")?.to_string())),
            ("calendarweekcolumnheader", b.text("calendarWeekColumnHeader")),
            ("shortdateformat", b.text("shortDateFormat")),
            ("longdateformat", b.text("longDateFormat")),
            ("timeformat", b.text("timeFormat")),
            ("showrelativedates", some(b.bool("showRelativeDates")?.to_string())),
            ("enablecolorimpairedmode", some(b.bool("enableColorImpairedMode")?.to_string())),
            ("uilanguage", language),
            ("theme", b.text("theme")),
        ],
    )?;
    Ok(accepted("/api/v1/config/ui/1".into(), ui_json(&st.db)))
}

// ---- Development (config/development) ----

/// Prowlarr's `DevelopmentConfigResource`.
pub fn dev_json(db: &SqliteDb) -> Value {
    json!({
        "id": 1,
        "consoleLogLevel": text(db, "consoleloglevel", ""),
        "logSql": boolean(db, "logsql", false),
        "logIndexerResponse": boolean(db, "logindexerresponse", false),
        "logRotate": int(db, "logrotate", 50),
        "filterSentryEvents": boolean(db, "filtersentryevents", true),
    })
}

async fn get_dev(State(st): State<AppState>, _: V1) -> Json<Value> {
    Json(dev_json(&st.db))
}

async fn get_dev_by_id(State(st): State<AppState>, _: V1, Path(_id): Path<i64>) -> Json<Value> {
    Json(dev_json(&st.db))
}

/// `PUT /api/v1/config/development/{id}`: saves the development settings; 202 with them as saved.
async fn put_dev(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, bytes: Bytes) -> ApiResult<Response> {
    let b = Body::read(&bytes)?;
    check(vec![validation::valid_id(b.id(path_id)?)])?;
    let some = |v: String| Some(v);
    save(
        &st.db,
        &[
            ("consoleloglevel", b.text("consoleLogLevel")),
            ("logsql", some(b.bool("logSql")?.to_string())),
            ("logindexerresponse", some(b.bool("logIndexerResponse")?.to_string())),
            ("logrotate", some(b.int("logRotate")?.to_string())),
            ("filtersentryevents", some(b.bool("filterSentryEvents")?.to_string())),
        ],
    )?;
    Ok(accepted("/api/v1/config/development/1".into(), dev_json(&st.db)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host {
            id: 1,
            bind_address: Some("*".into()),
            port: 9696,
            ssl_port: 6969,
            enable_ssl: false,
            authentication_method: "forms".into(),
            authentication_required: "enabled".into(),
            allowed_hosts: Some(String::new()),
            username: Some("admin".into()),
            password: Some(PASSWORD_MASK.into()),
            password_confirmation: Some(String::new()),
            log_size_limit: 1,
            branch: Some("master".into()),
            ssl_cert_path: Some(String::new()),
            url_base: Some(String::new()),
            trusted_networks: Some(String::new()),
            instance_name: Some("Lantern".into()),
            update_mechanism: "builtIn".into(),
            update_script_path: Some(String::new()),
            backup_folder: Some("Backups".into()),
            backup_interval: 7,
            backup_retention: 28,
        }
    }

    fn failures(h: &Host) -> Vec<(String, String)> {
        check_host(h).into_iter().flatten().map(|f| (f.property, f.message)).collect()
    }

    #[test]
    fn the_defaults_pass() {
        assert!(failures(&host()).is_empty(), "{:?}", failures(&host()));
    }

    #[test]
    fn checks_each_field_like_prowlarr() {
        let mut h = host();
        h.bind_address = Some("nope".into());
        h.port = 22;
        h.allowed_hosts = Some("http://x:1, 0.0.0.0".into());
        h.url_base = Some("http://host/x".into());
        h.trusted_networks = Some("10.1.2.3/8".into());
        h.instance_name = Some("Mine".into());
        h.authentication_method = "basic".into();
        h.password = Some("new".into());
        h.password_confirmation = Some("other".into());
        h.log_size_limit = 11;
        h.branch = Some(" ".into());
        h.backup_interval = 0;
        h.backup_retention = 91;
        h.id = 0;
        let got = failures(&h);
        let want = [
            ("BindAddress", "Must contain wildcard (*) or a valid IP Address"),
            ("Port", "The specified condition was not met for 'Port'."),
            ("AllowedHosts", "Must be a comma separated list of host names or IP addresses only, without a protocol or port"),
            ("AllowedHosts", "0.0.0.0 and [::] are not host names, leave empty to allow all hosts"),
            ("UrlBase", "Must be a valid URL path (ie: '/sonarr')"),
            (
                "TrustedNetworks",
                "Must be a comma separated list of IP addresses or networks in CIDR notation, such as 172.17.0.1, 10.0.0.0/8 or fc00::/7",
            ),
            ("InstanceName", "Must contain Lantern"),
            ("AuthenticationMethod", "'Basic' is no longer supported, switch to 'Forms' instead."),
            ("PasswordConfirmation", "Must match Password"),
            ("LogSizeLimit", "'Log Size Limit' must be between 1 and 10. You entered 11."),
            ("Branch", "Branch name is required, 'master' is the default"),
            ("BackupInterval", "'Backup Interval' must be between 1 and 7. You entered 0."),
            ("BackupRetention", "'Backup Retention' must be between 1 and 90. You entered 91."),
            ("Id", "'Id' must be greater than '0'."),
        ];
        let want: Vec<(String, String)> = want.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn allowed_hosts_are_needed_without_authentication() {
        let mut h = host();
        h.authentication_required = "disabledForLocalAddresses".into();
        assert_eq!(
            failures(&h),
            [("AllowedHosts".to_string(), "Allowed Hosts is required when 'Authentication Required' is not 'Enabled'".to_string())]
        );
        h.allowed_hosts = None;
        assert_eq!(failures(&h)[0], ("AllowedHosts".to_string(), "'Allowed Hosts' must not be empty.".to_string()));
    }

    #[test]
    fn ssl_needs_its_own_port_and_a_certificate() {
        let mut h = host();
        h.enable_ssl = true;
        h.ssl_port = 9696;
        let got = failures(&h);
        assert_eq!(got[0], ("SslPort".to_string(), "'Ssl Port' must not be equal to '9696'.".to_string()));
        assert_eq!(got[1], ("SslCertPath".to_string(), "'Ssl Cert Path' must not be empty.".to_string()));
        h.ssl_cert_path = Some("/nowhere/cert.pfx".into());
        assert_eq!(failures(&h)[1].1, "File '/nowhere/cert.pfx' does not exist");
    }

    #[test]
    fn forms_needs_a_username_and_password() {
        let mut h = host();
        h.username = None;
        h.password = Some(String::new());
        let got = failures(&h);
        assert_eq!(got[0], ("Username".to_string(), "'Username' must not be empty.".to_string()));
        assert_eq!(got[1], ("Password".to_string(), "'Password' must not be empty.".to_string()));
    }

    #[test]
    fn the_ui_language_must_be_offered() {
        assert!(check_ui_language(Some("fr")).into_iter().flatten().next().is_none());
        let got: Vec<String> = check_ui_language(Some("")).into_iter().flatten().map(|f| f.message).collect();
        assert_eq!(got, ["The UI Language value cannot be empty", "Invalid UI Language value"]);
        let got: Vec<String> = check_ui_language(Some("xx")).into_iter().flatten().map(|f| f.message).collect();
        assert_eq!(got, ["Invalid UI Language value"]);
    }

    #[test]
    fn proxy_urls_round_trip() {
        assert_eq!(proxy_url("socks5", "proxy.lan", 1080, "me", "p@ss"), "socks5://me:p%40ss@proxy.lan:1080");
        assert_eq!(proxy_url("http", "::1", 8080, "", ""), "http://[::1]:8080");
    }

    #[test]
    fn reads_enums_by_name_or_number() {
        assert_eq!(enum_name(&json!("Forms"), AUTHENTICATION_TYPES).as_deref(), Some("forms"));
        assert_eq!(enum_name(&json!(12), UPDATE_MECHANISMS).as_deref(), Some("docker"));
        assert_eq!(enum_name(&json!("nope"), PROXY_TYPES), None);
    }
}
