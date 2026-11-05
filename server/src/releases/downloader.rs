// Ported from autobrr v1.90.0 (internal/domain/downloader.go, internal/domain/api.go), GPL-2.0-or-later.
//! Download clients as autobrr keeps them (`/api/downloaders`): what program it is, where it answers, how
//! Lantern signs in, and the rules that hold a release back when the client is busy. The JSON is autobrr's own:
//! passwords and API keys go out as `<redacted>`, and a `<redacted>` sent back keeps the saved secret.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a secret is sent back as (autobrr's `RedactedStr`).
pub const REDACTED: &str = "<redacted>";

/// `s` hidden: `<redacted>` when it has text, else empty (autobrr's `RedactString`).
pub fn redact(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        REDACTED.to_string()
    }
}

/// Whether `s` is the hidden form of a secret (autobrr's `IsRedactedString`).
pub fn is_redacted(s: &str) -> bool {
    !s.is_empty() && s == REDACTED
}

/// Every kind of download client autobrr talks to.
pub const DOWNLOADER_TYPES: [&str; 16] = [
    "QBITTORRENT",
    "DELUGE_V1",
    "DELUGE_V2",
    "RTORRENT",
    "TRANSMISSION",
    "PORLA",
    "ARIA2",
    "RADARR",
    "SONARR",
    "LIDARR",
    "WHISPARR",
    "WHISPARR_V3",
    "READARR",
    "SPORTARR",
    "SABNZBD",
    "NZBGET",
];

/// The client's sign-in for a password protected web page in front of it (autobrr's `DownloaderAuth`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloaderAuth {
    /// Whether to send it.
    #[serde(skip_serializing_if = "is_false")]
    pub enabled: bool,
    /// `NONE`, `BASIC_AUTH` or `DIGEST_AUTH`.
    #[serde(rename = "type", skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The user name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The password (sent back redacted).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
}

/// The older form of [`DownloaderAuth`] (autobrr's deprecated `BasicAuth`), still read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BasicAuth {
    /// Whether to send it.
    #[serde(skip_serializing_if = "is_false")]
    pub auth: bool,
    /// The user name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The password.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
}

/// When a busy client turns a release down (autobrr's `DownloaderRules`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloaderRules {
    /// Whether the rules apply.
    pub enabled: bool,
    /// The most downloads running at once (0: no limit).
    pub max_active_downloads: i64,
    /// Add anyway when the running downloads are slow.
    pub ignore_slow_torrents: bool,
    /// `ALWAYS` or `MAX_DOWNLOADS_REACHED`: when the slow check runs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ignore_slow_torrents_condition: String,
    /// Slow means downloading below this many KiB/s.
    pub download_speed_threshold: i64,
    /// Slow means uploading below this many KiB/s.
    pub upload_speed_threshold: i64,
}

/// A client's extra settings (autobrr's `DownloaderSettings`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DownloaderSettings {
    /// The API key (the *arrs, SABnzbd, Porla's token, aria2's secret, qBittorrent's key); sent back redacted.
    #[serde(rename = "apikey", skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// The older sign-in form; read, and turned into [`DownloaderSettings::auth`].
    pub basic: BasicAuth,
    /// When a busy client turns a release down.
    pub rules: DownloaderRules,
    /// The *arrs: the id of their own download client a pushed release goes to (0: their choice).
    #[serde(skip_serializing_if = "is_zero")]
    pub external_download_client_id: i64,
    /// The *arrs: the name of their own download client a pushed release goes to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub external_download_client: String,
    /// The sign-in for a password protected page in front of the client.
    pub auth: DownloaderAuth,
}

impl<'de> Deserialize<'de> for DownloaderSettings {
    /// Reads the settings; an older `basic` sign-in becomes `auth` (autobrr's `UnmarshalJSON`).
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// The settings as they arrive.
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Raw {
            #[serde(rename = "apikey")]
            api_key: String,
            basic: BasicAuth,
            rules: DownloaderRules,
            external_download_client_id: i64,
            external_download_client: String,
            auth: DownloaderAuth,
        }
        let r = Option::<Raw>::deserialize(d)?.unwrap_or_default();
        let mut s = DownloaderSettings {
            api_key: r.api_key,
            basic: r.basic,
            rules: r.rules,
            external_download_client_id: r.external_download_client_id,
            external_download_client: r.external_download_client,
            auth: r.auth,
        };
        s.basic_to_auth();
        Ok(s)
    }
}

impl DownloaderSettings {
    /// An older `basic` sign-in with a name or password becomes `auth` (BASIC_AUTH).
    pub fn basic_to_auth(&mut self) {
        if !self.basic.username.is_empty() || !self.basic.password.is_empty() {
            self.auth = DownloaderAuth {
                enabled: self.basic.auth,
                kind: "BASIC_AUTH".into(),
                username: self.basic.username.clone(),
                password: self.basic.password.clone(),
            };
        }
    }
}

/// A download client (autobrr's `Downloader`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Downloader {
    /// Its id (0 before it is saved).
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// One of [`DOWNLOADER_TYPES`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Used at all.
    pub enabled: bool,
    /// Its address (`http://qbittorrent:8080`, or a host name for Deluge).
    pub host: String,
    /// The port, when the host has none (Deluge always).
    pub port: i64,
    /// Talk HTTPS (qBittorrent and Transmission build the address from it).
    pub tls: bool,
    /// Accept any certificate.
    pub tls_skip_verify: bool,
    /// The user name it signs in with.
    pub username: String,
    /// The password it signs in with (sent back redacted).
    pub password: String,
    /// The extra settings.
    pub settings: DownloaderSettings,
}

/// For serde: leave out a false flag (Go's `omitempty`).
fn is_false(b: &bool) -> bool {
    !*b
}

/// For serde: leave out a zero (Go's `omitempty`).
fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Downloader {
    /// The client as the API sends it: secrets redacted, `basic` turned into `auth` (autobrr's `MarshalJSON`).
    pub fn public(&self) -> Value {
        let mut settings = DownloaderSettings {
            api_key: redact(&self.settings.api_key),
            rules: self.settings.rules.clone(),
            external_download_client_id: self.settings.external_download_client_id,
            external_download_client: self.settings.external_download_client.clone(),
            auth: DownloaderAuth {
                enabled: self.settings.auth.enabled,
                kind: self.settings.auth.kind.clone(),
                username: self.settings.auth.username.clone(),
                password: redact(&self.settings.auth.password),
            },
            basic: BasicAuth {
                auth: self.settings.basic.auth,
                username: self.settings.basic.username.clone(),
                password: redact(&self.settings.basic.password),
            },
        };
        settings.basic_to_auth();
        let mut v = serde_json::to_value(self).unwrap_or_default();
        v["password"] = Value::String(redact(&self.password));
        v["settings"] = serde_json::to_value(&settings).unwrap_or_default();
        v
    }

    /// Keeps the saved secrets where `self` carries `<redacted>` (autobrr's `Update` and `Test`).
    pub fn keep_secrets(&mut self, saved: &Downloader) {
        if is_redacted(&self.password) {
            self.password = saved.password.clone();
        }
        if is_redacted(&self.settings.api_key) {
            self.settings.api_key = saved.settings.api_key.clone();
        }
        if is_redacted(&self.settings.auth.password) {
            self.settings.auth.password = saved.settings.auth.password.clone();
        }
        if is_redacted(&self.settings.basic.password) {
            self.settings.basic.password = saved.settings.basic.password.clone();
        }
    }

    /// Whether the client can be saved: it needs a host and a known type (autobrr's `Validate`).
    pub fn validate(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("validation error: missing host".into());
        }
        if !DOWNLOADER_TYPES.contains(&self.kind.as_str()) {
            return Err("invalid download client type".into());
        }
        Ok(())
    }

    /// The client's address: built from host, port and TLS for qBittorrent and Transmission (whose older
    /// settings split it), the host as it is for the rest (autobrr's `BuildLegacyHost`).
    pub fn build_legacy_host(&self) -> Result<String, String> {
        match self.kind.as_str() {
            "QBITTORRENT" => self.legacy_host(false),
            "TRANSMISSION" => self.legacy_host(true),
            _ => Ok(self.host.clone()),
        }
    }

    /// `host` with the scheme the TLS switch says and the port when it isn't 80 or 443; Transmission's ends
    /// in `/rpc`.
    fn legacy_host(&self, rpc: bool) -> Result<String, String> {
        let mut u = GoUrl::parse(&self.host)?;
        u.scheme = if self.tls { "https".into() } else { "http".into() };
        if u.host.is_empty() {
            u.host = self.host.clone();
        }
        if u.host == u.path {
            u.path = String::new();
        }
        if self.port > 0 && self.port != 80 && self.port != 443 {
            u.host = format!("{}:{}", u.host, self.port);
        }
        if rpc && !u.path.ends_with("/rpc") {
            u.path = join_path(&u.path, "/rpc");
        }
        Ok(u.string())
    }
}

/// `base` and `elem` joined with one slash between, like Go's `url.JoinPath` for one element.
pub fn join_path(base: &str, elem: &str) -> String {
    let joined = format!("{}/{}", base.trim_end_matches('/'), elem.trim_start_matches('/'));
    let trailing = elem.ends_with('/');
    let mut out = String::new();
    for part in joined.split('/').filter(|p| !p.is_empty() && *p != ".") {
        out.push('/');
        out.push_str(part);
    }
    if trailing || out.is_empty() {
        out.push('/');
    }
    out
}

/// The address `base` with `elem` joined to its path (Go's `url.JoinPath`); `base` as it is with `elem` added
/// when it isn't an address.
pub fn join_path_url(base: &str, elem: &str) -> String {
    match url::Url::parse(base) {
        Ok(mut u) if u.has_host() => {
            let p = join_path(u.path(), elem);
            u.set_path(&p);
            u.to_string()
        }
        _ => format!("{}/{}", base.trim_end_matches('/'), elem.trim_start_matches('/')),
    }
}

/// The parts of an address as Go's `url.Parse` splits them, enough for the legacy host rules.
#[derive(Debug, Clone, Default)]
struct GoUrl {
    scheme: String,
    host: String,
    path: String,
    query: String,
}

impl GoUrl {
    /// `raw` split like Go does: `scheme://host/path?query`, `scheme:opaque` (the opaque part is dropped, as the
    /// legacy host rules reset it), or a bare path; a first path part with a colon can't be read.
    fn parse(raw: &str) -> Result<GoUrl, String> {
        let (rest, query) = match raw.split_once('?') {
            Some((a, b)) => (a, b.to_string()),
            None => (raw, String::new()),
        };
        let scheme_end = rest.find(':').filter(|&i| {
            let s = &rest[..i];
            !s.is_empty()
                && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        });
        let mut u = GoUrl { query, ..GoUrl::default() };
        let rest = match scheme_end {
            Some(i) => {
                u.scheme = rest[..i].to_lowercase();
                let after = &rest[i + 1..];
                if !after.starts_with('/') {
                    // scheme:opaque, as `qbit.domain.ltd:8080` reads
                    return Ok(u);
                }
                after
            }
            None => rest,
        };
        if let Some(r) = rest.strip_prefix("//") {
            let (host, path) = match r.find('/') {
                Some(i) => (&r[..i], &r[i..]),
                None => (r, ""),
            };
            u.host = host.to_string();
            u.path = path.to_string();
        } else {
            if u.scheme.is_empty() && rest.split('/').next().is_some_and(|seg| seg.contains(':')) {
                return Err(format!("parse \"{raw}\": first path segment in URL cannot contain colon"));
            }
            u.path = rest.to_string();
        }
        Ok(u)
    }

    /// The address put back together.
    fn string(&self) -> String {
        let mut s = String::new();
        if !self.scheme.is_empty() {
            s.push_str(&self.scheme);
            s.push(':');
        }
        if !self.host.is_empty() || !self.scheme.is_empty() {
            s.push_str("//");
            s.push_str(&self.host);
        }
        if !self.path.is_empty() && !self.path.starts_with('/') && !self.host.is_empty() {
            s.push('/');
        }
        s.push_str(&self.path);
        if !self.query.is_empty() {
            s.push('?');
            s.push_str(&self.query);
        }
        s
    }
}

/// A tag of Radarr, Sonarr, Whisparr or Sportarr (autobrr's `ArrTag`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArrTag {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub label: String,
}

#[cfg(test)]
mod tests {
    //! autobrr's internal/domain/downloader_test.go, and the redaction of `MarshalJSON`.
    use super::*;
    use serde_json::json;

    #[test]
    fn qbit_build_legacy_host() {
        let cases = [
            ("https://qbit.domain.ltd", 0, true, "https://qbit.domain.ltd"),
            ("http://qbit.domain.ltd", 0, false, "http://qbit.domain.ltd"),
            ("https://qbit.domain.ltd:8080", 0, true, "https://qbit.domain.ltd:8080"),
            ("qbit.domain.ltd:8080", 0, false, "http://qbit.domain.ltd:8080"),
            ("qbit.domain.ltd", 8080, false, "http://qbit.domain.ltd:8080"),
            ("qbit.domain.ltd", 443, true, "https://qbit.domain.ltd"),
            ("qbit.domain.ltd", 10200, false, "http://qbit.domain.ltd:10200"),
            ("https://domain.ltd/qbittorrent", 0, true, "https://domain.ltd/qbittorrent"),
            ("127.0.0.1", 8080, false, "http://127.0.0.1:8080"),
        ];
        for (i, (host, port, tls, want)) in cases.iter().enumerate() {
            let d = Downloader { kind: "QBITTORRENT".into(), host: host.to_string(), port: *port, tls: *tls, ..Downloader::default() };
            assert_eq!(d.build_legacy_host().unwrap(), *want, "build_url_{}", i + 1);
        }
        let t = Downloader { kind: "TRANSMISSION".into(), host: "localhost".into(), port: 9091, ..Downloader::default() };
        assert_eq!(t.build_legacy_host().unwrap(), "http://localhost:9091/rpc", "Transmission ends in /rpc");
    }

    #[test]
    fn secrets_are_redacted_and_kept() {
        let body = json!({
            "name": "qb", "type": "QBITTORRENT", "enabled": true, "host": "http://qb", "password": "secret",
            "settings": { "apikey": "key", "basic": { "auth": true, "username": "u", "password": "bp" } }
        });
        let d: Downloader = serde_json::from_value(body).unwrap();
        assert!(d.settings.auth.enabled, "basic becomes auth");
        let v = d.public();
        assert_eq!(v["password"], REDACTED);
        assert_eq!(v["settings"]["apikey"], REDACTED);
        assert_eq!(v["settings"]["auth"]["password"], REDACTED);
        let mut back: Downloader = serde_json::from_value(v).unwrap();
        back.keep_secrets(&d);
        assert_eq!(back.password, "secret");
        assert_eq!(back.settings.api_key, "key");
        assert_eq!(back.settings.auth.password, "bp");
    }

    #[test]
    fn validate_words() {
        assert_eq!(
            Downloader { kind: "QBITTORRENT".into(), ..Downloader::default() }.validate().unwrap_err(),
            "validation error: missing host"
        );
        assert_eq!(
            Downloader { kind: "X".into(), host: "h".into(), ..Downloader::default() }.validate().unwrap_err(),
            "invalid download client type"
        );
    }
}
