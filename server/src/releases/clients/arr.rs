// Ported from autobrr v1.90.0 (pkg/arr: shared.go, sonarr, radarr, lidarr, readarr, whisparr, sportarr), GPL-2.0-or-later.
//! Sonarr, Radarr, Lidarr, Readarr, Whisparr and Sportarr: a release is pushed to their `release/push` and they
//! grab it with their own download client, or say why not. Each app has its own API version, error words and
//! answer shape, kept as autobrr's clients have them.

use serde_json::{Map, Value};
use std::time::Duration;

/// Which *arr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    /// Sonarr (`/api/v3`).
    Sonarr,
    /// Radarr (`/api/v3`).
    Radarr,
    /// Lidarr (`/api/v1`).
    Lidarr,
    /// Readarr (`/api/v1`).
    Readarr,
    /// Whisparr 2 (Sonarr based) or 3 (Radarr based) (`/api/v3`).
    Whisparr(i64),
    /// Sportarr (`/api`).
    Sportarr,
}

impl App {
    /// The app for a download client type.
    pub fn of(kind: &str) -> Option<App> {
        Some(match kind {
            "SONARR" => App::Sonarr,
            "RADARR" => App::Radarr,
            "LIDARR" => App::Lidarr,
            "READARR" => App::Readarr,
            "WHISPARR" => App::Whisparr(2),
            "WHISPARR_V3" => App::Whisparr(3),
            "SPORTARR" => App::Sportarr,
            _ => return None,
        })
    }

    /// Its lower-case name, as autobrr's errors start.
    pub fn name(self) -> &'static str {
        match self {
            App::Sonarr => "sonarr",
            App::Radarr => "radarr",
            App::Lidarr => "lidarr",
            App::Readarr => "readarr",
            App::Whisparr(_) => "whisparr",
            App::Sportarr => "sportarr",
        }
    }

    /// The API's path.
    fn api(self) -> &'static str {
        match self {
            App::Lidarr | App::Readarr => "/api/v1/",
            App::Sportarr => "/api/",
            _ => "/api/v3/",
        }
    }
}

/// A connection's settings (autobrr's arr `Config`).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The app's address.
    pub hostname: String,
    /// Its API key.
    pub api_key: String,
    /// Send basic auth.
    pub basic_auth: bool,
    /// Basic auth user.
    pub username: String,
    /// Basic auth password.
    pub password: String,
    /// Accept any certificate.
    pub tls_skip_verify: bool,
}

/// A client for one *arr.
pub struct Client {
    app: App,
    cfg: Config,
    http: reqwest::Client,
}

/// The first Sportarr with the native `release/push` (autobrr's `sportarr.MinimumVersion`).
pub const SPORTARR_MINIMUM_VERSION: &str = "4.0.1024";

/// A bad request answer's line (autobrr's `BadRequestResponse.String`).
fn bad_request_line(r: &Value) -> String {
    let s = |k: &str| r.get(k).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    format!("[{}: {}] {}: {} - got value: {}", s("severity"), s("errorCode"), s("propertyName"), s("errorMessage"), s("attemptedValue"))
}

/// An *arr's single error object's text (autobrr's `ErrorResponse.Error`): message, else detail, title,
/// description; `None` when the body isn't one.
pub fn parse_error_response(body: &str) -> Option<String> {
    let v: Map<String, Value> = serde_json::from_str(body).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    ["message", "detail", "title", "description"].iter().map(|k| s(k)).find(|t| !t.is_empty())
}

/// Whether Sportarr's `version` serves the native push (autobrr's `SupportsNativeAPI`).
pub fn sportarr_supports_native_api(version: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> { v.split('.').map(|p| p.parse::<u64>().ok()).collect() };
    let (Some(mut have), Some(mut want)) = (parse(version), parse(SPORTARR_MINIMUM_VERSION)) else { return false };
    let n = have.len().max(want.len());
    have.resize(n, 0);
    want.resize(n, 0);
    have >= want
}

impl Client {
    /// A client for `app` with `cfg`.
    pub fn new(app: App, cfg: Config) -> Client {
        Client { app, http: super::http(Duration::from_secs(120), cfg.tls_skip_verify, false), cfg }
    }

    /// The address of `endpoint`.
    fn url(&self, endpoint: &str) -> String {
        match url::Url::parse(&self.cfg.hostname) {
            Ok(mut u) => {
                let p = format!("{}{}{}", u.path().trim_end_matches('/'), self.app.api(), endpoint);
                u.set_path(&p);
                u.to_string()
            }
            Err(_) => format!("{}{}{}", self.cfg.hostname.trim_end_matches('/'), self.app.api(), endpoint),
        }
    }

    /// A request with autobrr's headers and basic auth.
    fn req(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, self.url(endpoint)).header("User-Agent", "autobrr").header("X-Api-Key", &self.cfg.api_key);
        if self.cfg.basic_auth {
            r = r.basic_auth(&self.cfg.username, Some(&self.cfg.password));
        }
        r
    }

    /// GETs `endpoint`: the status and body.
    async fn get(&self, endpoint: &str) -> Result<(u16, String), String> {
        let resp = self
            .req(reqwest::Method::GET, endpoint)
            .send()
            .await
            .map_err(|e| format!("could not make request: {}", super::req_err("Get", &self.url(endpoint), &e)))?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await.unwrap_or_default()))
    }

    /// Checks the API key (`system/status`); the app's version.
    pub async fn test(&self) -> Result<String, String> {
        let (status, body) = self.get("system/status").await.map_err(|e| format!("could not make Test: {e}"))?;
        match status {
            200 => {}
            401 => return Err("unauthorized: bad credentials".into()),
            s => return Err(format!("unexpected status code: {s}")),
        }
        let v: Value = serde_json::from_str(&body).map_err(|e| format!("could not unmarshal data: {e}"))?;
        let version = v.get("version").and_then(Value::as_str).unwrap_or("").to_string();
        if let App::Whisparr(want) = self.app {
            let major: i64 = version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
            if major != 0 && major != want {
                return Err(format!(
                    "client is configured for Whisparr v{want} but the server reports version {version}, change the client type to Whisparr (v{major})"
                ));
            }
        }
        Ok(version)
    }

    /// GETs `endpoint` read as `T` (autobrr's `getJSON`): the status isn't looked at, so an answer that isn't a
    /// `T` (a sign-in error too) is "could not unmarshal data". The lists read movies, series, albums, artists,
    /// books and leagues this way.
    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, endpoint: &str) -> Result<T, String> {
        let (_, body) = self.get(endpoint).await?;
        serde_json::from_str(&body).map_err(|e| format!("could not unmarshal data: {e}"))
    }

    /// The app's tags.
    pub async fn get_tags(&self) -> Result<Vec<(i64, String)>, String> {
        let (_, body) = self.get("tag").await?;
        let v: Vec<Value> = serde_json::from_str(&body).map_err(|e| format!("could not unmarshal data: {e}"))?;
        Ok(v.iter()
            .map(|t| (t.get("id").and_then(Value::as_i64).unwrap_or(0), t.get("label").and_then(Value::as_str).unwrap_or("").to_string()))
            .collect())
    }

    /// Pushes `release` (the request body as the app takes it); the app's rejections (empty: taken), or an
    /// error in autobrr's words.
    pub async fn push(&self, release: &Value) -> Result<Vec<String>, String> {
        let name = self.app.name();
        let wrap = match self.app {
            App::Sonarr => "could not push release to sonarr",
            App::Radarr => "error push release",
            App::Lidarr => "lidarr Client post error",
            App::Readarr => "could not push release to readarr",
            App::Whisparr(_) => "could not push release to whisparr",
            App::Sportarr => "could not push release to sportarr",
        };
        let resp = self
            .req(reqwest::Method::POST, "release/push")
            .header("Content-Type", "application/json")
            .body(release.to_string())
            .send()
            .await
            .map_err(|e| format!("{wrap}: could not make request: {}", super::req_err("Post", &self.url("release/push"), &e)))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let code = status.as_u16();
        if code != 400 && !(200..=401).contains(&code) {
            return Err(format!("{wrap}: {name}: bad request:  (status: {} {}): {body}", code, status.canonical_reason().unwrap_or("")));
        }
        if code == 400 {
            let list: Vec<Value> = match serde_json::from_str(&body) {
                Ok(l) => l,
                Err(e) => {
                    return Err(parse_error_response(&body).unwrap_or_else(|| format!("could not unmarshal data: {e}")));
                }
            };
            let lines: Vec<String> = list.iter().map(bad_request_line).collect();
            let config_problem = matches!(self.app, App::Sonarr | App::Radarr | App::Whisparr(_))
                && list.iter().any(|r| {
                    let p = r.get("propertyName").and_then(Value::as_str).unwrap_or("").to_lowercase();
                    p == "downloadclient" || p == "downloadclientid"
                });
            if config_problem {
                return Err(format!("{name} push failed due to invalid configuration: {}", lines.join("; ")));
            }
            return Ok(lines);
        }
        let first = match self.app {
            App::Lidarr | App::Readarr => {
                let what = if self.app == App::Lidarr { "lidarr Client error json unmarshal" } else { "could not unmarshal data" };
                let v: Map<String, Value> = serde_json::from_str(&body).map_err(|e| format!("{what}: {e}"))?;
                Value::Object(v)
            }
            _ => {
                let v: Vec<Value> = serde_json::from_str(&body).map_err(|e| format!("could not unmarshal data: {e}"))?;
                match v.into_iter().next() {
                    Some(f) => f,
                    None if matches!(self.app, App::Whisparr(_)) => return Err("whisparr push returned an empty response".into()),
                    None => return Err(format!("{name} release/push returned an empty response")),
                }
            }
        };
        let flag = |k: &str| first.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("rejected") || flag("temporarilyRejected") {
            return Ok(first
                .get("rejections")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default());
        }
        Ok(Vec::new())
    }
}

/// Radarr's indexer flags for a release (autobrr's `radarr.BuildIndexerFlags`).
pub fn radarr_indexer_flags(freeleech_percent: i64, origin: &str) -> i64 {
    let mut f = match freeleech_percent {
        100 => 1,
        75 => 256,
        50 => 2,
        25 => 512,
        _ => 0,
    };
    match origin.trim().to_lowercase().as_str() {
        "internal" => f |= 32,
        "scene" => f |= 128,
        _ => {}
    }
    f
}

/// Sonarr's (and Sportarr's) indexer flags for a release (autobrr's `sonarr.BuildIndexerFlags`).
pub fn sonarr_indexer_flags(freeleech_percent: i64, origin: &str) -> i64 {
    let mut f = match freeleech_percent {
        100 => 1,
        75 => 32,
        50 => 2,
        25 => 64,
        _ => 0,
    };
    match origin.trim().to_lowercase().as_str() {
        "internal" => f |= 8,
        "scene" => f |= 16,
        _ => {}
    }
    f
}
