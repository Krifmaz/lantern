// Ported from autobrr v1.90.0 (internal/domain/proxy.go, internal/proxy/service.go, internal/database/proxy.go,
// internal/irc/httpproxy.go, the proxy parts of internal/irc/service.go, internal/feed/source.go and
// internal/release/download_service.go), GPL-2.0-or-later.
//! autobrr's proxies: SOCKS5 and HTTP proxies that IRC networks, feeds and indexers can be set to go through.
//!
//! A proxy is a row of `autobrr_proxies`. An IRC network names one with `use_proxy` and `proxy_id`; a feed and
//! a torrent download use the proxy of their indexer (`autobrr_indexer_meta`). A network whose proxy is missing
//! or turned off never connects directly: it is stopped with the reason, as in autobrr. When a proxy changes,
//! the networks using it reconnect (or are turned off when the proxy was turned off) and the feeds using it get
//! a fresh job. Deleting a proxy detaches it from the indexers and networks first.

use super::definition::{is_redacted, redact};
use super::store::{ApiError, ApiResult};
use super::Releases;
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde::{Deserialize, Serialize, Serializer};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A SOCKS5 proxy (autobrr's `ProxyTypeSocks5`).
pub const PROXY_TYPE_SOCKS5: &str = "SOCKS5";
/// An HTTP proxy (autobrr's `ProxyTypeHTTP`).
pub const PROXY_TYPE_HTTP: &str = "HTTP";

/// The address a proxy test asks for through the proxy (autobrr's).
pub const TEST_URL: &str = "https://autobrr.com";

/// What `Store`, `Update` and the other failures say when the row isn't there (Go's `sql.ErrNoRows`).
pub const ERR_NO_ROWS: &str = "sql: no rows in result set";

/// One proxy (autobrr's `Proxy`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Proxy {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
    /// On.
    pub enabled: bool,
    /// [`PROXY_TYPE_SOCKS5`] or [`PROXY_TYPE_HTTP`].
    #[serde(rename = "type")]
    pub kind: String,
    /// The address, `socks5://host:port` or `http://host:port`.
    pub addr: String,
    /// The user name (empty: none).
    pub user: String,
    /// The password (sent back redacted).
    pub pass: String,
    /// Seconds (kept; autobrr's clients use their own timeouts).
    pub timeout: i64,
}

impl Serialize for Proxy {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(8))?;
        m.serialize_entry("id", &self.id)?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("enabled", &self.enabled)?;
        m.serialize_entry("type", &self.kind)?;
        m.serialize_entry("addr", &self.addr)?;
        m.serialize_entry("user", &self.user)?;
        m.serialize_entry("pass", &redact(&self.pass))?;
        m.serialize_entry("timeout", &self.timeout)?;
        m.end()
    }
}

/// One user of a proxy (autobrr's `ProxyUsageItem`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProxyUsageItem {
    /// Its id.
    pub id: i64,
    /// Its name.
    pub name: String,
}

/// What goes through a proxy (autobrr's `ProxyUsage`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProxyUsage {
    /// The indexers (not archived) set to use it.
    pub indexers: Vec<ProxyUsageItem>,
    /// The IRC networks set to use it.
    pub irc_networks: Vec<ProxyUsageItem>,
    /// The feeds whose indexer uses it.
    pub feeds: Vec<ProxyUsageItem>,
}

/// What happened to a proxy (autobrr's `events.ProxyUpdated` and `ProxyDeleted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyChange {
    /// It was changed or turned on or off.
    Updated,
    /// It was removed.
    Deleted,
}

impl Proxy {
    /// Whether its type is one autobrr knows (`ValidProxyType`).
    pub fn valid_proxy_type(&self) -> bool {
        self.kind == PROXY_TYPE_SOCKS5 || self.kind == PROXY_TYPE_HTTP
    }

    /// autobrr's `Validate`: a known type, an address that fits it, and a name.
    pub fn validate(&self) -> Result<(), String> {
        if !self.valid_proxy_type() {
            return Err(format!("invalid proxy type: {}", self.kind));
        }
        validate_proxy_addr(&self.addr, &self.kind)?;
        if self.name.is_empty() {
            return Err("name is required".into());
        }
        Ok(())
    }

    /// The address with the user name and password in it when both are set (`GetProxiedHTTPClient`).
    pub fn url(&self) -> Result<url::Url, String> {
        let mut u = parse_addr(&self.addr)?;
        if !self.user.is_empty() && !self.pass.is_empty() {
            let _ = u.set_username(&self.user);
            let _ = u.set_password(Some(&self.pass));
        }
        Ok(u)
    }

    /// An HTTP client that goes through this proxy, with `timeout` (autobrr's `GetProxiedHTTPClient` is 60
    /// seconds) and certificates checked unless `insecure`.
    pub fn http_client(&self, timeout: Duration, insecure: bool, cookies: bool) -> Result<reqwest::Client, String> {
        let u = self.url()?;
        if !self.valid_proxy_type() {
            return Err(format!("invalid proxy type: {}", self.kind));
        }
        let proxy = reqwest::Proxy::all(u.as_str()).map_err(|e| format!("could not create proxy dialer from url: {}: {e}", self.addr))?;
        reqwest::Client::builder()
            .proxy(proxy)
            .timeout(timeout)
            .danger_accept_invalid_certs(insecure)
            .cookie_store(cookies)
            .build()
            .map_err(|e| format!("could not get http client: {e}"))
    }
}

/// `addr` read as a URL (Go's `url.Parse`).
fn parse_addr(addr: &str) -> Result<url::Url, String> {
    url::Url::parse(addr).map_err(|e| format!("could not parse proxy url: {addr}: {e}"))
}

/// autobrr's `ValidateProxyAddr`: an address, and a scheme that fits the type.
pub fn validate_proxy_addr(addr: &str, kind: &str) -> Result<(), String> {
    if addr.is_empty() {
        return Err("addr is required".into());
    }
    let u = parse_addr(addr)?;
    match kind {
        PROXY_TYPE_SOCKS5 => {
            if u.scheme() != "socks5" && u.scheme() != "socks5h" {
                return Err("proxy url scheme must be socks5 or socks5h".into());
            }
        }
        PROXY_TYPE_HTTP => {
            if u.scheme() != "http" && u.scheme() != "https" {
                return Err("proxy url scheme must be http or https".into());
            }
        }
        _ => return Err(format!("invalid proxy type: {kind}")),
    }
    Ok(())
}

/// The proxy table (autobrr's `proxy`).
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_proxies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            enabled INTEGER NOT NULL DEFAULT 0,
            name TEXT NOT NULL,
            type TEXT NOT NULL,
            addr TEXT NOT NULL,
            auth_user TEXT NOT NULL DEFAULT '',
            auth_pass TEXT NOT NULL DEFAULT '',
            timeout INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .map_err(|e| anyhow::anyhow!("The autobrr proxy table can't be made: {}", e.detail))
}

/// A proxy row.
fn proxy_of(r: &kit_server_core::Row) -> Proxy {
    Proxy {
        id: r.i("id"),
        name: r.s("name"),
        enabled: r.b("enabled"),
        kind: r.s("type"),
        addr: r.s("addr"),
        user: r.s("auth_user"),
        pass: r.s("auth_pass"),
        timeout: r.i("timeout"),
    }
}

/// Proxy `id` on `db`; [`ERR_NO_ROWS`] (as a not-found) when there is none.
pub fn find(db: &SqliteDb, id: i64) -> ApiResult<Proxy> {
    db.one("SELECT * FROM autobrr_proxies WHERE id=?", p![id])?.map(|r| proxy_of(&r)).ok_or_else(|| ApiError::NotFound(ERR_NO_ROWS.into()))
}

/// The proxy indexer `id`'s downloads go through (autobrr's `DownloadService.DownloadRelease`): none when the
/// indexer isn't set to use one or its proxy is off; its lookup error when its proxy is gone.
pub fn indexer_proxy(db: &SqliteDb, id: i64) -> Result<Option<Proxy>, String> {
    let Some(m) = db.one("SELECT use_proxy, proxy_id FROM autobrr_indexer_meta WHERE source_id=?", p![id]).map_err(|e| e.detail)? else {
        return Ok(None);
    };
    if !m.b("use_proxy") {
        return Ok(None);
    }
    let p = find(db, m.i("proxy_id")).map_err(|e| e.message().to_string())?;
    if !p.enabled {
        tracing::debug!("indexer {id}: proxy {} disabled, skipping", p.name);
    }
    Ok(p.enabled.then_some(p))
}

impl Releases {
    /// Every proxy, by name (autobrr's `List`).
    pub fn proxies(&self) -> ApiResult<Vec<Proxy>> {
        Ok(self.db.query("SELECT * FROM autobrr_proxies ORDER BY name ASC", p![])?.iter().map(proxy_of).collect())
    }

    /// Proxy `id`; [`ERR_NO_ROWS`] (as a not-found) when there is none (autobrr's `FindByID`).
    pub fn proxy(&self, id: i64) -> ApiResult<Proxy> {
        find(&self.db, id)
    }

    /// Whether proxy `id` is there.
    fn proxy_exists(&self, id: i64) -> ApiResult<bool> {
        Ok(self.db.one("SELECT id FROM autobrr_proxies WHERE id=?", p![id])?.is_some())
    }

    /// Adds proxy `p` after autobrr's checks; its id is set (autobrr's `Store`).
    pub fn store_proxy(&self, p: &mut Proxy) -> ApiResult<()> {
        p.validate().map_err(|e| ApiError::Internal(format!("validation error: {e}")))?;
        p.id = self.db.execute(
            "INSERT INTO autobrr_proxies (enabled, name, type, addr, auth_user, auth_pass, timeout) VALUES (?,?,?,?,?,?,?)",
            p![p.enabled, p.name, p.kind, p.addr, p.user, p.pass, p.timeout],
        )?;
        Ok(())
    }

    /// Changes proxy `p` (a redacted password keeps the saved one), then reconnects what uses it (autobrr's
    /// `Update`). `Ok(false)` when no row changed (autobrr's `update failed`).
    pub fn update_proxy(&self, p: &mut Proxy) -> ApiResult<bool> {
        p.validate().map_err(|e| ApiError::Internal(format!("validation error: {e}")))?;
        let existing = self.proxy(p.id).map_err(|e| ApiError::Internal(e.message().to_string()))?;
        if is_redacted(&p.pass) {
            p.pass = existing.pass;
        }
        if !self.proxy_exists(p.id)? {
            return Ok(false);
        }
        self.db.execute(
            "UPDATE autobrr_proxies SET enabled=?, name=?, type=?, addr=?, auth_user=?, auth_pass=?, timeout=?, updated_at=? WHERE id=?",
            p![p.enabled, p.name, p.kind, p.addr, p.user, p.pass, p.timeout, super::notify::now_text(), p.id],
        )?;
        self.proxy_changed(ProxyChange::Updated, p.id);
        Ok(true)
    }

    /// Turns proxy `id` on or off, then reconnects (or turns off) what uses it (autobrr's `ToggleEnabled`).
    pub fn toggle_proxy(&self, id: i64, enabled: bool) -> ApiResult<()> {
        if !self.proxy_exists(id)? {
            return Err(ApiError::Internal("update failed".into()));
        }
        self.db.execute("UPDATE autobrr_proxies SET enabled=?, updated_at=? WHERE id=?", p![enabled, super::notify::now_text(), id])?;
        self.proxy_changed(ProxyChange::Updated, id);
        Ok(())
    }

    /// Removes proxy `id`, detaching it from every indexer and network first; `Ok(false)` when there was no
    /// such proxy (autobrr's `delete failed`).
    pub fn delete_proxy(&self, id: i64) -> ApiResult<bool> {
        let usage = self.proxy_usage(id)?;
        if !self.proxy_exists(id)? {
            return Ok(false);
        }
        self.db.execute("UPDATE autobrr_indexer_meta SET use_proxy=0, proxy_id=0 WHERE proxy_id=?", p![id])?;
        self.db.execute("UPDATE autobrr_irc_networks SET use_proxy=0, proxy_id=0 WHERE proxy_id=?", p![id])?;
        self.db.execute("DELETE FROM autobrr_proxies WHERE id=?", p![id])?;
        self.apply_proxy_change(ProxyChange::Deleted, &usage);
        Ok(true)
    }

    /// The indexers, IRC networks and feeds set to go through proxy `id`, each by name (autobrr's `Usage`).
    pub fn proxy_usage(&self, id: i64) -> ApiResult<ProxyUsage> {
        let items = |sql: &str| -> ApiResult<Vec<ProxyUsageItem>> {
            Ok(self.db.query(sql, p![id])?.iter().map(|r| ProxyUsageItem { id: r.i("id"), name: r.s("name") }).collect())
        };
        Ok(ProxyUsage {
            indexers: items(
                "SELECT s.id AS id, s.name AS name FROM release_sources s JOIN autobrr_indexer_meta m ON m.source_id = s.id \
                 WHERE m.proxy_id=? AND m.use_proxy=1 AND m.archived=0 ORDER BY s.name ASC",
            )?,
            irc_networks: items("SELECT id, name FROM autobrr_irc_networks WHERE proxy_id=? AND use_proxy=1 ORDER BY name ASC")?,
            feeds: items(
                "SELECT f.id AS id, f.name AS name FROM autobrr_feeds f JOIN autobrr_indexer_meta m ON m.source_id = f.indexer_id \
                 WHERE m.proxy_id=? AND m.use_proxy=1 AND m.archived=0 ORDER BY f.name ASC",
            )?,
        })
    }

    /// Asks for [`TEST_URL`] (or the address set for tests) through proxy `p`; a saved proxy's redacted
    /// password is the saved one (autobrr's `Test`).
    pub async fn test_proxy(&self, p: &mut Proxy) -> ApiResult<()> {
        if !p.valid_proxy_type() {
            return Err(ApiError::Internal(format!("invalid proxy type {}", p.kind)));
        }
        if p.id > 0 {
            let existing = self.proxy(p.id)?;
            if is_redacted(&p.pass) {
                p.pass = existing.pass;
            }
        }
        if p.addr.is_empty() {
            return Err(ApiError::Internal("proxy addr missing".into()));
        }
        let client = p.http_client(Duration::from_secs(60), false, false).map_err(|e| ApiError::Internal(format!("could not get http client: {e}")))?;
        let target = self.proxy_test_url.read().unwrap_or_else(|e| e.into_inner()).clone();
        let resp = client
            .get(&target)
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("could not connect to proxy server: {}: {}", p.addr, super::clients::req_err("Get", &target, &e))))?;
        if resp.status().as_u16() != 200 {
            return Err(ApiError::Internal(format!("got unexpected status code: {}", resp.status().as_u16())));
        }
        tracing::debug!("proxy test ok: {}", p.addr);
        Ok(())
    }

    /// Asks proxy tests for `url` instead of [`TEST_URL`] (tests).
    pub fn set_proxy_test_url(&self, url: &str) {
        *self.proxy_test_url.write().unwrap_or_else(|e| e.into_inner()) = url.to_string();
    }

    /// The proxy an indexer, feed or network set to `use_proxy` with `proxy_id` goes through: `None` when it
    /// isn't set to use one or its proxy is off; an error when its proxy can't be found.
    pub fn proxy_for(&self, use_proxy: bool, proxy_id: i64) -> ApiResult<Option<Proxy>> {
        if !use_proxy {
            return Ok(None);
        }
        let p = self.proxy(proxy_id)?;
        Ok(p.enabled.then_some(p))
    }

    /// Tells what uses proxy `id` that it changed (autobrr's `publishEventProxyUpdated`; a failed lookup only
    /// logs, the change is saved).
    fn proxy_changed(&self, change: ProxyChange, id: i64) {
        match self.proxy_usage(id) {
            Ok(usage) => self.apply_proxy_change(change, &usage),
            Err(e) => tracing::error!("proxy {id}: could not find proxy usage, consumers not reconciled: {}", e.message()),
        }
    }

    /// What a proxy change does (the IRC and feed services' `onProxyChanged`): every enabled network that used
    /// it reconnects with the new settings, or is stopped and turned off when the proxy was turned off; every
    /// feed that used it gets a fresh job.
    pub fn apply_proxy_change(&self, change: ProxyChange, usage: &ProxyUsage) {
        for item in &usage.irc_networks {
            let Ok(n) = self.irc_network(item.id) else {
                tracing::error!("proxy: could not load network {} for changed proxy", item.id);
                continue;
            };
            if !n.enabled {
                continue;
            }
            let proxy_off = change == ProxyChange::Updated && n.use_proxy && self.proxy(n.proxy_id).is_ok_and(|p| !p.enabled);
            if proxy_off {
                tracing::warn!("IRC network {}: proxy disabled, disabling network", n.name);
                self.irc.stop_and_remove(n.id);
                let _ = self.db.execute("UPDATE autobrr_irc_networks SET enabled=0 WHERE id=?", p![n.id]);
                continue;
            }
            if let Err(e) = self.restart_irc_network(n.id) {
                tracing::error!("IRC network {}: could not restart for changed proxy: {}", n.name, e.message());
            }
        }
        for item in &usage.feeds {
            if let Err(e) = self.sync_feed_job(item.id) {
                tracing::error!("feed {}: could not reconcile feed job for changed proxy: {}", item.id, e.message());
            }
        }
    }
}

// ---- dialing through a proxy (IRC) ----

/// A stream to the IRC server through a proxy.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Connects to `addr` (`host:port`) through proxy `p`: an HTTP CONNECT for `http` and `https` proxies (autobrr's
/// `httpProxyDialer`, TLS to an `https` proxy checked unless `tls_skip_verify`), SOCKS5 for `socks5` and
/// `socks5h` (golang.org/x/net/proxy's dialer, which hands the host name to the proxy).
pub async fn dial(p: &Proxy, addr: &str, tls_skip_verify: bool) -> Result<Box<dyn Stream>, String> {
    if p.addr.is_empty() {
        return Err("proxy addr missing".into());
    }
    let u = p.url()?;
    let host = u.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_string();
    match u.scheme() {
        "http" | "https" => {
            let port = u.port().unwrap_or(8080);
            let proxy_addr = format!("{host}:{port}");
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|e| format!("failed to connect to proxy {proxy_addr}: {e}"))?;
            let mut conn: Box<dyn Stream> = Box::new(tcp);
            if u.scheme() == "https" {
                let config = super::irc::tls_client_config(tls_skip_verify)?;
                let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.clone())
                    .map_err(|_| format!("{host} is not a server name TLS can check"))?;
                conn = Box::new(
                    tokio_rustls::TlsConnector::from(std::sync::Arc::new(config))
                        .connect(name, conn)
                        .await
                        .map_err(|e| format!("failed to connect to proxy {proxy_addr}: {e}"))?,
                );
            }
            let user = (!u.username().is_empty()).then(|| (percent(u.username()), percent(u.password().unwrap_or(""))));
            tokio::time::timeout(Duration::from_secs(30), http_connect(conn, addr, user)).await.map_err(|_| "failed to read CONNECT response: i/o timeout".to_string())?
        }
        "socks5" | "socks5h" => {
            let port = u.port().unwrap_or(1080);
            let proxy_addr = format!("{host}:{port}");
            let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|e| format!("socks connect tcp {proxy_addr}->{addr}: dial tcp {proxy_addr}: {e}"))?;
            let user = (!u.username().is_empty()).then(|| (percent(u.username()), percent(u.password().unwrap_or(""))));
            tokio::time::timeout(Duration::from_secs(30), socks5_connect(tcp, &proxy_addr, addr, user))
                .await
                .map_err(|_| format!("socks connect tcp {proxy_addr}->{addr}: i/o timeout"))?
        }
        s => Err(format!("could not create proxy dialer from url: {}: proxy: unknown scheme: {s}", p.addr)),
    }
}

/// A URL part with its percent escapes undone.
fn percent(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// The HTTP CONNECT exchange on `conn` for `addr` (autobrr's `httpProxyDialer.DialContext`).
async fn http_connect(mut conn: Box<dyn Stream>, addr: &str, user: Option<(String, String)>) -> Result<Box<dyn Stream>, String> {
    use base64::Engine;
    let mut req = format!("CONNECT {addr} HTTP/1.1\r\nHost: {addr}\r\n");
    if let Some((u, p)) = user {
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
        req.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
    }
    req.push_str("\r\n");
    conn.write_all(req.as_bytes()).await.map_err(|e| format!("failed to send CONNECT request: {e}"))?;
    // read the answer's head byte by byte, so nothing the IRC server says after it is lost
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = conn.read(&mut b).await.map_err(|e| format!("failed to read CONNECT response: {e}"))?;
        if n == 0 {
            return Err("failed to read CONNECT response: unexpected EOF".into());
        }
        head.push(b[0]);
        if head.len() > 16 * 1024 {
            return Err("failed to read CONNECT response: header too long".into());
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status_line = text.lines().next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let _proto = parts.next();
    let code: u16 = parts.next().and_then(|c| c.parse().ok()).ok_or_else(|| format!("failed to read CONNECT response: malformed HTTP response {status_line:?}"))?;
    let reason = parts.next().unwrap_or("");
    if code == 200 {
        return Ok(conn);
    }
    // the error body, when there is one quickly (autobrr reads up to 1 KiB)
    let mut body = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_millis(200), conn.read(&mut body)).await.ok().and_then(|r| r.ok()).unwrap_or(0);
    let body = String::from_utf8_lossy(&body[..n]);
    let e = format!("proxy CONNECT to {addr} failed with status: {code} {reason} body: {body}");
    Err(match code {
        403 => format!("the proxy may be blocking IRC ports (6667/6697) or the destination host: {e}"),
        407 => format!("proxy authentication required: {e}"),
        401 => format!("invalid proxy credentials: {e}"),
        _ => e,
    })
}

/// The SOCKS5 exchange (RFC 1928, user name and password by RFC 1929) on `conn` for `addr`, in the words of
/// golang.org/x/net/internal/socks.
async fn socks5_connect(mut conn: tokio::net::TcpStream, proxy_addr: &str, addr: &str, user: Option<(String, String)>) -> Result<Box<dyn Stream>, String> {
    let fail = |m: String| format!("socks connect tcp {proxy_addr}->{addr}: {m}");
    let (host, port) = addr.rsplit_once(':').ok_or_else(|| fail("missing port in address".into()))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port: u16 = port.parse().map_err(|_| fail("invalid port".into()))?;
    let io = |e: std::io::Error| fail(e.to_string());
    let methods: &[u8] = if user.is_some() { &[5, 2, 0, 2] } else { &[5, 1, 0] };
    conn.write_all(methods).await.map_err(io)?;
    let mut reply = [0u8; 2];
    conn.read_exact(&mut reply).await.map_err(io)?;
    if reply[0] != 5 {
        return Err(fail(format!("unexpected protocol version {}", reply[0])));
    }
    match reply[1] {
        0 => {}
        2 => {
            let (u, p) = user.clone().unwrap_or_default();
            if u.is_empty() || u.len() > 255 || p.len() > 255 {
                return Err(fail("invalid username/password".into()));
            }
            let mut b = vec![1, u.len() as u8];
            b.extend_from_slice(u.as_bytes());
            b.push(p.len() as u8);
            b.extend_from_slice(p.as_bytes());
            conn.write_all(&b).await.map_err(io)?;
            let mut r = [0u8; 2];
            conn.read_exact(&mut r).await.map_err(io)?;
            if r[0] != 1 {
                return Err(fail("invalid username/password version".into()));
            }
            if r[1] != 0 {
                return Err(fail("username/password authentication failed".into()));
            }
        }
        0xff => return Err(fail("no acceptable authentication methods".into())),
        m => return Err(fail(format!("unsupported authentication method {m}"))),
    }
    let mut req = vec![5, 1, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            req.push(1);
            req.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            req.push(4);
            req.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(fail("FQDN too long".into()));
            }
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    conn.write_all(&req).await.map_err(io)?;
    let mut head = [0u8; 4];
    conn.read_exact(&mut head).await.map_err(io)?;
    if head[0] != 5 {
        return Err(fail(format!("unexpected protocol version {}", head[0])));
    }
    if head[1] != 0 {
        let why = match head[1] {
            1 => "general SOCKS server failure".to_string(),
            2 => "connection not allowed by ruleset".to_string(),
            3 => "network unreachable".to_string(),
            4 => "host unreachable".to_string(),
            5 => "connection refused".to_string(),
            6 => "TTL expired".to_string(),
            7 => "command not supported".to_string(),
            8 => "address type not supported".to_string(),
            c => format!("unknown code: {c}"),
        };
        return Err(fail(why));
    }
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut l = [0u8; 1];
            conn.read_exact(&mut l).await.map_err(io)?;
            l[0] as usize
        }
        t => return Err(fail(format!("unknown address type {t}"))),
    };
    let mut rest = vec![0u8; skip + 2];
    conn.read_exact(&mut rest).await.map_err(io)?;
    Ok(Box::new(conn))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socks(addr: &str) -> Proxy {
        Proxy { id: 1, name: "socks".into(), enabled: true, kind: PROXY_TYPE_SOCKS5.into(), addr: addr.into(), ..Proxy::default() }
    }

    #[test]
    fn validate_follows_autobrrs_rules() {
        assert_eq!(socks("socks5://10.0.0.1:1080").validate(), Ok(()));
        assert_eq!(socks("socks5h://10.0.0.1:1080").validate(), Ok(()));
        assert_eq!(socks("http://10.0.0.1:1080").validate().unwrap_err(), "proxy url scheme must be socks5 or socks5h");
        assert_eq!(socks("").validate().unwrap_err(), "addr is required");
        let mut p = socks("socks5://10.0.0.1:1080");
        p.name.clear();
        assert_eq!(p.validate().unwrap_err(), "name is required");
        p.kind = "SOCKS4".into();
        assert_eq!(p.validate().unwrap_err(), "invalid proxy type: SOCKS4");
        let http = Proxy { kind: PROXY_TYPE_HTTP.into(), ..socks("socks5://10.0.0.1:1080") };
        assert_eq!(http.validate().unwrap_err(), "proxy url scheme must be http or https");
        assert_eq!(Proxy { addr: "https://p.example:3128".into(), ..http }.validate(), Ok(()));
    }

    #[test]
    fn the_password_is_redacted_and_kept_out_of_the_url_without_a_user() {
        let p = Proxy { pass: "hunter2".into(), ..socks("socks5://10.0.0.1:1080") };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["pass"], "<redacted>");
        assert_eq!(v["type"], "SOCKS5");
        assert_eq!(p.url().unwrap().as_str(), "socks5://10.0.0.1:1080");
        let p = Proxy { user: "me".into(), ..p };
        assert_eq!(p.url().unwrap().as_str(), "socks5://me:hunter2@10.0.0.1:1080");
        assert_eq!(serde_json::to_value(Proxy::default()).unwrap()["pass"], "");
    }
}
