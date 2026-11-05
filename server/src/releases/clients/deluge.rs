// Ported from go-deluge v1.4.0 (github.com/autobrr/go-deluge, delugeclient.go, methods.go, options.go, plugins.go,
// torrent_status.go), GPL-3.0, as autobrr v1.90.0 uses it.
//! Deluge's daemon RPC: rencode messages squeezed with zlib over TLS (the daemon's own certificate is never
//! checked, as go-deluge does). Deluge 2 puts a five byte header (version 1, then the length) in front.

use super::rencode::{self, Value};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;

/// How long one read or write may take (go-deluge's `DefaultReadWriteTimeout`).
pub const READ_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Deluge 2's protocol version byte.
pub const DELUGE2_PROTOCOL_VERSION: u8 = 1;

/// An error the daemon answered (go-deluge's `RPCError`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RpcError {
    /// The exception's type.
    pub exception_type: String,
    /// Its message (`Unknown Label` …).
    pub exception_message: String,
    /// The daemon's traceback.
    pub traceback: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RPC error {}('{}')\nTraceback: {}", self.exception_type, self.exception_message, self.traceback)
    }
}

/// Why a call failed: the daemon said no, or something else went wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The daemon answered an error.
    Rpc(RpcError),
    /// Anything else, in words.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rpc(e) => e.fmt(f),
            Error::Other(s) => f.write_str(s),
        }
    }
}

impl From<String> for Error {
    fn from(s: String) -> Error {
        Error::Other(s)
    }
}

/// Options for a new torrent (the ones autobrr sets of go-deluge's `Options`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Upload limit in KiB/s.
    pub max_upload_speed: Option<i64>,
    /// Download limit in KiB/s.
    pub max_download_speed: Option<i64>,
    /// Where to save it.
    pub download_location: Option<String>,
    /// Add it paused.
    pub add_paused: Option<bool>,
    /// Skip the hash check (Deluge 2 only).
    pub seed_mode: Option<bool>,
}

impl Options {
    /// The options as the dictionary Deluge takes, in go-deluge's field order.
    pub fn to_dictionary(&self, v2: bool) -> Value {
        let mut d = Vec::new();
        if let Some(x) = self.max_upload_speed {
            d.push(("max_upload_speed".into(), Value::Int(x)));
        }
        if let Some(x) = self.max_download_speed {
            d.push(("max_download_speed".into(), Value::Int(x)));
        }
        if let Some(x) = &self.download_location {
            d.push(("download_location".into(), x.as_str().into()));
        }
        if let Some(x) = self.add_paused {
            d.push(("add_paused".into(), Value::Bool(x)));
        }
        if v2 {
            if let Some(x) = self.seed_mode {
                d.push(("seed_mode".into(), Value::Bool(x)));
            }
        }
        Value::Dict(d)
    }
}

/// The status keys autobrr's rules query asks for (go-deluge's `commonStatusKeys`).
const STATUS_KEYS: [&str; 33] = [
    "state",
    "tracker_host",
    "tracker_status",
    "next_announce",
    "name",
    "hash",
    "total_size",
    "progress",
    "num_seeds",
    "total_seeds",
    "num_peers",
    "total_peers",
    "eta",
    "download_payload_rate",
    "upload_payload_rate",
    "all_time_download",
    "total_uploaded",
    "ratio",
    "distributed_copies",
    "num_pieces",
    "piece_length",
    "total_done",
    "files",
    "file_priorities",
    "file_progress",
    "peers",
    "is_seed",
    "is_finished",
    "active_time",
    "seeding_time",
    "time_added",
    "private",
    "save_path",
];

/// The TLS stream to the daemon.
type Stream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

/// A Deluge client (`v2` for Deluge 2).
pub struct Client {
    host: String,
    port: u16,
    login: String,
    password: String,
    v2: bool,
    serial: i64,
    conn: Option<Stream>,
}

/// Accepts any certificate: Deluge's daemon makes its own (go-deluge sets `InsecureSkipVerify`).
#[derive(Debug)]
struct AnyCert;

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

/// The request bytes for one call: `[[serial, method, args, kwargs]]`, rencoded and zlib squeezed, with
/// Deluge 2's header in front.
pub fn request_bytes(serial: i64, method: &str, args: Vec<Value>, kwargs: Vec<(Value, Value)>, v2: bool) -> Result<Vec<u8>, String> {
    let payload = Value::List(vec![Value::List(vec![Value::Int(serial), method.into(), Value::List(args), Value::Dict(kwargs)])]);
    let raw = rencode::to_bytes(&payload)?;
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&raw).map_err(|e| e.to_string())?;
    let body = z.finish().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if v2 {
        out.push(DELUGE2_PROTOCOL_VERSION);
        out.extend((body.len() as u32).to_be_bytes());
    }
    out.extend(body);
    Ok(out)
}

/// Reads an answer to call `serial` from its decoded message (go-deluge's `handleRPCResponse`): the return values.
pub fn handle_response(msg: Value, serial: i64, v2: bool) -> Result<Vec<Value>, Error> {
    let Value::List(mut l) = msg else { return Err(Error::Other("expected list".into())) };
    if l.is_empty() {
        return Err(Error::Other("expected list".into()));
    }
    let mt = l.remove(0).as_i64().unwrap_or(0);
    if mt == 3 {
        return Err(Error::Other("event support not available".into()));
    }
    let id = if l.is_empty() { 0 } else { l.remove(0).as_i64().unwrap_or(0) };
    if id != serial {
        return Err(Error::Other(format!("request/response serial id mismatch: got {id} but {serial} expected")));
    }
    match mt {
        1 => Ok(l),
        2 => {
            let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or_default();
            let e = if v2 {
                let msg = match l.get(1) {
                    Some(Value::List(a)) => s(a.first()),
                    _ => String::new(),
                };
                RpcError { exception_type: s(l.first()), exception_message: msg, traceback: s(l.get(3)) }
            } else {
                let inner = match l.first() {
                    Some(Value::List(a)) => a.clone(),
                    _ => Vec::new(),
                };
                RpcError { exception_type: s(inner.first()), exception_message: s(inner.get(1)), traceback: s(inner.get(2)) }
            };
            Err(Error::Rpc(e))
        }
        _ => Err(Error::Other("unknown message type".into())),
    }
}

/// Unsqueezes an answer body and decodes it.
pub fn decode_body(body: &[u8]) -> Result<Value, String> {
    let mut raw = Vec::new();
    ZlibDecoder::new(body).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    rencode::from_bytes(&raw)
}

impl Client {
    /// A client for the daemon at `host:port`.
    pub fn new(host: &str, port: u16, login: &str, password: &str, v2: bool) -> Client {
        Client { host: host.into(), port, login: login.into(), password: password.into(), v2, serial: 0, conn: None }
    }

    /// Connects and signs in (`daemon.login`).
    pub async fn connect(&mut self) -> Result<(), Error> {
        let tcp = tokio::time::timeout(READ_WRITE_TIMEOUT, tokio::net::TcpStream::connect((self.host.as_str(), self.port)))
            .await
            .map_err(|_| format!("dial tcp {}:{}: i/o timeout", self.host, self.port))?
            .map_err(|e| format!("dial tcp {}:{}: {e}", self.host, self.port))?;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCert))
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(self.host.clone())
            .unwrap_or_else(|_| rustls::pki_types::ServerName::try_from("deluge").expect("a valid name"));
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| e.to_string())?;
        self.conn = Some(tls);
        let kwargs = if self.v2 { vec![("client_version".into(), "2.0.3".into())] } else { Vec::new() };
        let args = vec![self.login.as_str().into(), self.password.as_str().into()];
        self.rpc("daemon.login", args, kwargs).await.map(|_| ())
    }

    /// Calls `method`; its return values.
    pub async fn rpc(&mut self, method: &str, args: Vec<Value>, kwargs: Vec<(Value, Value)>) -> Result<Vec<Value>, Error> {
        self.serial = if self.serial == i64::MAX - 1 { 1 } else { self.serial + 1 };
        let req = request_bytes(self.serial, method, args, kwargs, self.v2)?;
        let v2 = self.v2;
        let conn = self.conn.as_mut().ok_or_else(|| Error::Other("connection is already closed".into()))?;
        let io = async {
            conn.write_all(&req).await.map_err(|e| e.to_string())?;
            conn.flush().await.map_err(|e| e.to_string())?;
            if v2 {
                let mut header = [0u8; 5];
                conn.read_exact(&mut header).await.map_err(|e| e.to_string())?;
                if header[0] != DELUGE2_PROTOCOL_VERSION {
                    return Err(format!("found protocol version {} but expected {}", header[0], DELUGE2_PROTOCOL_VERSION));
                }
                let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
                let mut body = vec![0u8; len];
                conn.read_exact(&mut body).await.map_err(|e| e.to_string())?;
                return decode_body(&body);
            }
            // Deluge 1 sends no length: read until the zlib stream is whole.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = conn.read(&mut chunk).await.map_err(|e| e.to_string())?;
                if n == 0 {
                    return decode_body(&buf);
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Ok(v) = decode_body(&buf) {
                    return Ok(v);
                }
            }
        };
        let msg = tokio::time::timeout(READ_WRITE_TIMEOUT, io).await.map_err(|_| Error::Other("i/o timeout".into()))??;
        handle_response(msg, self.serial, self.v2)
    }

    /// Closes the connection.
    pub async fn close(&mut self) {
        if let Some(mut c) = self.conn.take() {
            let _ = c.shutdown().await;
        }
    }

    /// The daemon's version (`daemon.info`).
    pub async fn daemon_version(&mut self) -> Result<String, Error> {
        let v = self.rpc("daemon.info", Vec::new(), Vec::new()).await?;
        v.first().and_then(Value::as_str).ok_or_else(|| Error::Other("invalid return value".into()))
    }

    /// Adds a magnet; its hash (empty when the daemon already has it).
    pub async fn add_torrent_magnet(&mut self, magnet: &str, options: &Options) -> Result<String, Error> {
        let args = vec![magnet.into(), options.to_dictionary(self.v2)];
        let v = self.rpc("core.add_torrent_magnet", args, Vec::new()).await?;
        hash_of(v)
    }

    /// Adds a torrent file given as base64; its hash (empty when the daemon already has it).
    pub async fn add_torrent_file(&mut self, name: &str, b64: &str, options: &Options) -> Result<String, Error> {
        let args = vec![name.into(), b64.into(), options.to_dictionary(self.v2)];
        let v = self.rpc("core.add_torrent_file", args, Vec::new()).await?;
        hash_of(v)
    }

    /// How many torrents are downloading (`core.get_torrents_status` with state `Downloading`).
    pub async fn downloading_count(&mut self) -> Result<usize, Error> {
        let mut keys: Vec<Value> = STATUS_KEYS.iter().map(|k| (*k).into()).collect();
        if self.v2 {
            keys.extend(["download_location", "completed_time", "last_seen_complete"].iter().map(|k| (*k).into()));
        }
        let filter = Value::Dict(vec![("state".into(), "Downloading".into())]);
        let v = self.rpc("core.get_torrents_status", vec![filter, Value::List(keys)], Vec::new()).await?;
        if v.len() != 1 {
            return Err(Error::Other("invalid return value".into()));
        }
        match &v[0] {
            Value::Dict(d) => Ok(d.len()),
            _ => Err(Error::Other("expected dictionary as list response".into())),
        }
    }

    /// The enabled plugins.
    pub async fn enabled_plugins(&mut self) -> Result<Vec<String>, Error> {
        let v = self.rpc("core.get_enabled_plugins", Vec::new(), Vec::new()).await?;
        match v.first() {
            Some(Value::List(l)) => Ok(l.iter().filter_map(Value::as_str).collect()),
            _ => Err(Error::Other("invalid return value".into())),
        }
    }

    /// Sets a torrent's label (`label.set_torrent`).
    pub async fn set_torrent_label(&mut self, hash: &str, label: &str) -> Result<(), Error> {
        self.rpc("label.set_torrent", vec![hash.into(), label.into()], Vec::new()).await.map(|_| ())
    }

    /// Adds a label (`label.add`).
    pub async fn add_label(&mut self, label: &str) -> Result<(), Error> {
        self.rpc("label.add", vec![label.into()], Vec::new()).await.map(|_| ())
    }
}

/// The hash an add call returned.
fn hash_of(v: Vec<Value>) -> Result<String, Error> {
    match v.into_iter().next() {
        None => Err(Error::Other("invalid return value".into())),
        Some(Value::None) => Ok(String::new()),
        Some(x) => Ok(x.as_str().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    //! go-deluge's framing (delugeclient.go `rpc`, `handleRPCResponse`) and options (options.go `toDictionary`).
    use super::*;

    #[test]
    fn deluge2_request_has_header_and_body() {
        let b = request_bytes(1, "daemon.login", vec!["user".into(), "pass".into()], vec![("client_version".into(), "2.0.3".into())], true)
            .unwrap();
        assert_eq!(b[0], DELUGE2_PROTOCOL_VERSION);
        let len = u32::from_be_bytes([b[1], b[2], b[3], b[4]]) as usize;
        assert_eq!(len, b.len() - 5);
        let msg = decode_body(&b[5..]).unwrap();
        assert_eq!(
            msg,
            Value::List(vec![Value::List(vec![
                Value::Int(1),
                "daemon.login".into(),
                Value::List(vec!["user".into(), "pass".into()]),
                Value::Dict(vec![("client_version".into(), "2.0.3".into())]),
            ])])
        );
        let v1 = request_bytes(1, "daemon.info", Vec::new(), Vec::new(), false).unwrap();
        assert!(decode_body(&v1).is_ok(), "Deluge 1 sends the zlib body alone");
    }

    #[test]
    fn answers_and_errors() {
        let ok = Value::List(vec![Value::Int(1), Value::Int(7), "hash".into()]);
        assert_eq!(handle_response(ok, 7, true).unwrap(), vec![Value::from("hash")]);
        let wrong = Value::List(vec![Value::Int(1), Value::Int(8)]);
        assert_eq!(handle_response(wrong, 7, true).unwrap_err().to_string(), "request/response serial id mismatch: got 8 but 7 expected");
        let v2 = Value::List(vec![
            Value::Int(2),
            Value::Int(1),
            "Exception".into(),
            Value::List(vec!["Unknown Label".into()]),
            Value::Dict(vec![]),
            "tb".into(),
        ]);
        let Error::Rpc(e) = handle_response(v2, 1, true).unwrap_err() else { panic!("an RPC error") };
        assert_eq!(e.exception_message, "Unknown Label");
        assert_eq!(e.to_string(), "RPC error Exception('Unknown Label')\nTraceback: tb");
        let v1 = Value::List(vec![
            Value::Int(2),
            Value::Int(1),
            Value::List(vec!["BadLogin".into(), "Password does not match".into(), "tb".into()]),
        ]);
        let Error::Rpc(e) = handle_response(v1, 1, false).unwrap_err() else { panic!("an RPC error") };
        assert_eq!((e.exception_type.as_str(), e.exception_message.as_str()), ("BadLogin", "Password does not match"));
        assert_eq!(handle_response(Value::List(vec![Value::Int(3)]), 1, true).unwrap_err().to_string(), "event support not available");
    }

    #[test]
    fn seed_mode_is_deluge2_only() {
        let o = Options { add_paused: Some(false), seed_mode: Some(true), ..Options::default() };
        assert_eq!(o.to_dictionary(false), Value::Dict(vec![("add_paused".into(), Value::Bool(false))]));
        assert_eq!(
            o.to_dictionary(true),
            Value::Dict(vec![("add_paused".into(), Value::Bool(false)), ("seed_mode".into(), Value::Bool(true))])
        );
    }
}
