// Ported from autobrr v1.90.0 (pkg/aria2), GPL-2.0-or-later.
//! aria2 over JSON-RPC: the secret goes first in every call as `token:<secret>`; torrents go as base64.

use super::JsonRpc;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// How long a call may take.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A connection's settings (autobrr's `aria2.Config`).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The address (`http://localhost:6800`, `ws://…/jsonrpc`, or `host:port`).
    pub host: String,
    /// The RPC secret (`--rpc-secret`).
    pub secret: String,
    /// Accept any certificate.
    pub tls_skip_verify: bool,
    /// Basic auth user.
    pub basic_user: String,
    /// Basic auth password.
    pub basic_pass: String,
}

/// aria2's options for a download: every value is text.
pub type Options = Map<String, Value>;

/// A running download's state (autobrr's `aria2.Status`).
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// Its id.
    pub gid: String,
    /// `active`, `waiting`, `paused` …
    pub status: String,
    /// Its size in bytes, as text.
    pub total_length: String,
    /// What is done, in bytes, as text.
    pub completed_length: String,
}

impl Status {
    /// Whether it is still downloading: a magnet without metadata has no size yet and counts (autobrr's
    /// `Downloading`).
    pub fn downloading(&self) -> bool {
        let total: i64 = self.total_length.parse().unwrap_or(0);
        if total == 0 {
            return true;
        }
        self.completed_length.parse::<i64>().unwrap_or(0) < total
    }
}

/// An aria2 client.
pub struct Client {
    cfg: Config,
    rpc: JsonRpc,
}

/// The JSON-RPC address for a host people typed (autobrr's `BuildEndpoint`): `ws`/`wss` become `http`/`https`,
/// a host without a scheme gets `http://`, and the path ends in `/jsonrpc`.
pub fn build_endpoint(host: &str) -> Result<String, String> {
    if host.is_empty() {
        return Err("aria2: missing host".into());
    }
    let full = if host.contains("://") { host.to_string() } else { format!("http://{host}") };
    let mut u = url::Url::parse(&full).map_err(|e| format!("aria2: could not parse host url: {full}: {e}"))?;
    let scheme = match u.scheme() {
        "ws" => "http",
        "wss" => "https",
        "http" => "http",
        "https" => "https",
        s => return Err(format!("aria2: unsupported scheme {s} in host url: {full}")),
    };
    // the url crate won't switch a special scheme to another, so the address is rebuilt
    if u.scheme() != scheme {
        let rest = &full[full.find("://").unwrap_or(0)..];
        u = url::Url::parse(&format!("{scheme}{rest}")).map_err(|e| format!("aria2: could not parse host url: {full}: {e}"))?;
    }
    if u.host_str().unwrap_or("").is_empty() {
        return Err(format!("aria2: missing host in url: {full}"));
    }
    let path = u.path().trim_end_matches('/').to_string();
    if !path.ends_with("/jsonrpc") {
        u.set_path(&format!("{path}/jsonrpc"));
    }
    Ok(u.to_string())
}

impl Client {
    /// A client for `cfg` (autobrr's `NewClient`).
    pub fn new(cfg: Config) -> Result<Client, String> {
        let endpoint = build_endpoint(&cfg.host)?;
        let rpc = JsonRpc {
            endpoint,
            headers: Vec::new(),
            basic_user: cfg.basic_user.clone(),
            basic_pass: cfg.basic_pass.clone(),
            http: super::http(DEFAULT_TIMEOUT, cfg.tls_skip_verify, false),
        };
        Ok(Client { cfg, rpc })
    }

    /// The parameters with the secret in front.
    fn params(&self, args: Vec<Value>) -> Value {
        let mut p = Vec::new();
        if !self.cfg.secret.is_empty() {
            p.push(json!(format!("token:{}", self.cfg.secret)));
        }
        p.extend(args);
        Value::Array(p)
    }

    /// aria2's version.
    pub async fn get_version(&self) -> Result<String, String> {
        let v = self.rpc.call("aria2.getVersion", Some(self.params(Vec::new()))).await?;
        v.get("version").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "could not decode version response".to_string())
    }

    /// Adds `uris` (a magnet); its gid.
    pub async fn add_uri(&self, uris: &[String], options: Options) -> Result<String, String> {
        let v = self.rpc.call("aria2.addUri", Some(self.params(vec![json!(uris), Value::Object(options)]))).await?;
        decode_gid(v)
    }

    /// Adds a torrent file; its gid.
    pub async fn add_torrent(&self, torrent: &[u8], options: Options) -> Result<String, String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(torrent);
        let v = self.rpc.call("aria2.addTorrent", Some(self.params(vec![json!(b64), json!([]), Value::Object(options)]))).await?;
        decode_gid(v)
    }

    /// The running downloads.
    pub async fn tell_active(&self) -> Result<Vec<Status>, String> {
        let keys = json!(["gid", "status", "totalLength", "completedLength"]);
        let v = self.rpc.call("aria2.tellActive", Some(self.params(vec![keys]))).await?;
        let list = v.as_array().ok_or_else(|| "could not decode active downloads response".to_string())?;
        let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(list
            .iter()
            .map(|x| Status {
                gid: s(x, "gid"),
                status: s(x, "status"),
                total_length: s(x, "totalLength"),
                completed_length: s(x, "completedLength"),
            })
            .collect())
    }
}

/// The gid in an answer.
fn decode_gid(v: Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s),
        other => Err(format!("unexpected gid in response: {other}")),
    }
}

#[cfg(test)]
mod tests {
    //! autobrr's pkg/aria2/aria2_test.go.
    use super::*;
    use axum::routing::post;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[test]
    fn build_endpoint_cases() {
        let ok = [
            ("http://localhost:6800", "http://localhost:6800/jsonrpc"),
            ("http://localhost:6800/", "http://localhost:6800/jsonrpc"),
            ("http://localhost:6800/jsonrpc", "http://localhost:6800/jsonrpc"),
            ("https://seedbox.net/aria2", "https://seedbox.net/aria2/jsonrpc"),
            ("https://seedbox.net/aria2/jsonrpc", "https://seedbox.net/aria2/jsonrpc"),
            ("localhost:6800", "http://localhost:6800/jsonrpc"),
            ("ws://localhost:6800/jsonrpc", "http://localhost:6800/jsonrpc"),
            ("wss://seedbox.net/jsonrpc", "https://seedbox.net/jsonrpc"),
        ];
        for (host, want) in ok {
            assert_eq!(build_endpoint(host).unwrap(), want, "{host}");
        }
        assert!(build_endpoint("").is_err());
        assert!(build_endpoint("ftp://localhost:6800").is_err());
    }

    /// Serves one canned result and records every request.
    async fn server(result: Value) -> (String, Arc<Mutex<Vec<Value>>>) {
        let calls: Arc<Mutex<Vec<Value>>> = Arc::default();
        let c = calls.clone();
        let app = axum::Router::new().route(
            "/jsonrpc",
            post(move |axum::Json(v): axum::Json<Value>| {
                let c = c.clone();
                let result = result.clone();
                async move {
                    c.lock().unwrap().push(v);
                    axum::Json(json!({ "id": 1, "jsonrpc": "2.0", "result": result }))
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}"), calls)
    }

    fn client(host: &str, secret: &str) -> Client {
        Client::new(Config { host: host.into(), secret: secret.into(), ..Config::default() }).unwrap()
    }

    #[tokio::test]
    async fn add_uri_with_and_without_secret() {
        let (host, calls) = server(json!("2089b05ecca3d829")).await;
        let mut o = Options::new();
        o.insert("dir".into(), json!("/downloads"));
        let gid = client(&host, "mock-secret").add_uri(&["magnet:?xt=urn:btih:mock".into()], o).await.unwrap();
        assert_eq!(gid, "2089b05ecca3d829");
        client(&host, "").add_uri(&["magnet:?xt=urn:btih:mock".into()], Options::new()).await.unwrap();
        let c = calls.lock().unwrap();
        assert_eq!(c[0]["method"], "aria2.addUri");
        assert_eq!(c[0]["params"], json!(["token:mock-secret", ["magnet:?xt=urn:btih:mock"], {"dir": "/downloads"}]));
        assert_eq!(c[1]["params"], json!([["magnet:?xt=urn:btih:mock"], {}]));
    }

    #[tokio::test]
    async fn add_torrent() {
        let (host, calls) = server(json!("2089b05ecca3d829")).await;
        let mut o = Options::new();
        o.insert("pause".into(), json!("true"));
        assert_eq!(client(&host, "mock-secret").add_torrent(b"d4:infod4:name4:mockee", o).await.unwrap(), "2089b05ecca3d829");
        let c = calls.lock().unwrap();
        assert_eq!(c[0]["method"], "aria2.addTorrent");
        assert_eq!(c[0]["params"], json!(["token:mock-secret", "ZDQ6aW5mb2Q0Om5hbWU0Om1vY2tlZQ==", [], {"pause": "true"}]));
    }

    #[tokio::test]
    async fn tell_active() {
        let (host, _) = server(json!([
            {"gid": "1", "status": "active", "totalLength": "100", "completedLength": "50"},
            {"gid": "2", "status": "active", "totalLength": "100", "completedLength": "100"},
            {"gid": "3", "status": "active", "totalLength": "0", "completedLength": "0"},
        ]))
        .await;
        let active = client(&host, "").tell_active().await.unwrap();
        assert_eq!(active.len(), 3);
        assert!(active[0].downloading());
        assert!(!active[1].downloading(), "a seeding torrent stays active");
        assert!(active[2].downloading(), "a magnet without metadata has no size yet");
    }

    #[tokio::test]
    async fn rpc_error() {
        let app = axum::Router::new().route(
            "/jsonrpc",
            post(|| async { axum::Json(json!({"id": 1, "jsonrpc": "2.0", "error": {"code": 1, "message": "Unauthorized"}})) }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let e = client(&format!("http://{addr}"), "wrong").get_version().await.unwrap_err();
        assert!(e.contains("Unauthorized"), "{e}");
    }
}
