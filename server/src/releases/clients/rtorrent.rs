// Ported from go-rtorrent v1.13.0 (github.com/autobrr/go-rtorrent, rtorrent.go, xmlrpc), MIT, as autobrr v1.90.0 uses it.
//! rTorrent over XML-RPC: `load.start`/`load.normal` for a magnet, `load.raw_start`/`load.raw` for a torrent file,
//! with the settings as extra commands (`d.custom1.set="tv"`). Basic or digest auth, as autobrr sets them.

use crate::hub::clients::xmlrpc::{self, Param};
use md5::{Digest, Md5};
use serde_json::Value;
use std::time::Duration;

/// A setting for a new torrent (go-rtorrent's `FieldValue`).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldValue {
    /// The field or command (`d.custom1`, `view.set_visible`).
    pub field: String,
    /// Its value.
    pub value: String,
    /// A command without a `.set` form.
    pub command: bool,
}

impl FieldValue {
    /// `field.set="value"`.
    pub fn set(field: &str, value: &str) -> FieldValue {
        FieldValue { field: field.into(), value: value.into(), command: false }
    }

    /// `cmd="value"` (go-rtorrent's `Command`).
    pub fn command(cmd: &str, value: &str) -> FieldValue {
        FieldValue { field: cmd.into(), value: value.into(), command: true }
    }

    /// The text rTorrent runs.
    pub fn as_arg(&self) -> String {
        if self.command {
            format!("{}=\"{}\"", self.field, self.value)
        } else {
            format!("{}.set=\"{}\"", self.field, self.value)
        }
    }
}

/// How the client signs in.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Auth {
    /// No sign-in.
    #[default]
    None,
    /// Basic auth.
    Basic(String, String),
    /// Digest auth.
    Digest(String, String),
}

/// An rTorrent client.
pub struct Client {
    addr: String,
    auth: Auth,
    http: reqwest::Client,
}

impl Client {
    /// A client for rTorrent's XML-RPC at `addr`.
    pub fn new(addr: &str, auth: Auth, tls_skip_verify: bool) -> Client {
        Client { addr: addr.into(), auth, http: super::http(Duration::from_secs(60), tls_skip_verify, false) }
    }

    /// Calls `method`; its answer, or `<method> XMLRPC call failed: <why>`.
    pub async fn call(&self, method: &str, params: &[Param]) -> Result<Value, String> {
        let body = xmlrpc::call(method, params);
        self.post(&body).await.map_err(|e| format!("{method} XMLRPC call failed: {e}"))
    }

    /// Posts an XML-RPC body, signing in as set.
    async fn post(&self, body: &str) -> Result<Value, String> {
        let send = |auth: Option<String>| {
            let mut r = self.http.post(&self.addr).header("Content-Type", "text/xml").body(body.to_string());
            if let Auth::Basic(u, p) = &self.auth {
                r = r.basic_auth(u, Some(p));
            }
            if let Some(a) = auth {
                r = r.header("Authorization", a);
            }
            r
        };
        let mut resp = send(None).send().await.map_err(|e| super::req_err("Post", &self.addr, &e))?;
        if let (Auth::Digest(u, p), 401) = (&self.auth, resp.status().as_u16()) {
            let challenge = resp.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let path = url::Url::parse(&self.addr).map(|x| x.path().to_string()).unwrap_or_else(|_| "/".into());
            let header = digest_header(&challenge, u, p, "POST", &path).ok_or_else(|| "digest: bad challenge".to_string())?;
            resp = send(Some(header)).send().await.map_err(|e| super::req_err("Post", &self.addr, &e))?;
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("bad response status: {}", status));
        }
        xmlrpc::parse(&text)
    }

    /// Adds `data` (a magnet as bytes, or a torrent file) with `cmd` and the settings.
    async fn add(&self, cmd: &str, data: &[u8], extra: &[FieldValue]) -> Result<(), String> {
        let mut params = vec![Param::Str(String::new()), Param::Bytes(data.to_vec())];
        params.extend(extra.iter().map(|f| Param::Str(f.as_arg())));
        self.call(cmd, &params).await.map(|_| ())
    }

    /// Adds a magnet and starts it.
    pub async fn add_url(&self, url: &str, extra: &[FieldValue]) -> Result<(), String> {
        self.add("load.start", url.as_bytes(), extra).await
    }

    /// Adds a magnet stopped.
    pub async fn add_url_stopped(&self, url: &str, extra: &[FieldValue]) -> Result<(), String> {
        self.add("load.normal", url.as_bytes(), extra).await
    }

    /// Adds a torrent file and starts it.
    pub async fn add_torrent(&self, data: &[u8], extra: &[FieldValue]) -> Result<(), String> {
        self.add("load.raw_start", data, extra).await
    }

    /// Adds a torrent file stopped.
    pub async fn add_torrent_stopped(&self, data: &[u8], extra: &[FieldValue]) -> Result<(), String> {
        self.add("load.raw", data, extra).await
    }

    /// The views (ruTorrent's ratio groups are `rat_0` …).
    pub async fn views(&self) -> Result<Vec<String>, String> {
        let mut v = self.call("view.list", &[]).await.map_err(|e| format!("view.list XMLRPC call failed: {e}"))?;
        if let Some(inner) = v.as_array().filter(|a| a.len() == 1).and_then(|a| a[0].as_array()).cloned() {
            v = Value::Array(inner);
        }
        let list = v.as_array().ok_or_else(|| format!("result isn't list: {v}"))?;
        list.iter().map(|n| n.as_str().map(str::to_string).ok_or_else(|| format!("view name isn't string: {n}"))).collect()
    }

    /// The machine's name (`system.hostname`), the connection test.
    pub async fn name(&self) -> Result<String, String> {
        let v = self.call("system.hostname", &[]).await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }
}

/// The `Authorization` header answering a digest `challenge` (RFC 2617, MD5, `qop=auth`).
pub fn digest_header(challenge: &str, user: &str, pass: &str, method: &str, uri: &str) -> Option<String> {
    let rest = challenge.trim().strip_prefix("Digest").or_else(|| challenge.trim().strip_prefix("digest"))?;
    let mut parts = std::collections::HashMap::new();
    for item in split_params(rest) {
        if let Some((k, v)) = item.split_once('=') {
            parts.insert(k.trim().to_lowercase(), v.trim().trim_matches('"').to_string());
        }
    }
    let realm = parts.get("realm").cloned().unwrap_or_default();
    let nonce = parts.get("nonce")?.clone();
    let h = |s: String| hex::encode(Md5::digest(s.as_bytes()));
    let ha1 = h(format!("{user}:{realm}:{pass}"));
    let ha2 = h(format!("{method}:{uri}"));
    let qop = parts.get("qop").map(|q| q.split(',').any(|x| x.trim() == "auth")).unwrap_or(false);
    let cnonce = hex::encode(rand::random::<[u8; 8]>());
    let nc = "00000001";
    let response = if qop { h(format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}")) } else { h(format!("{ha1}:{nonce}:{ha2}")) };
    let mut out = format!("Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\"");
    if qop {
        out.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
    }
    if let Some(o) = parts.get("opaque") {
        out.push_str(&format!(", opaque=\"{o}\""));
    }
    if parts.contains_key("algorithm") {
        out.push_str(", algorithm=MD5");
    }
    Some(out)
}

/// A challenge's `k=v` items, commas inside quotes kept.
fn split_params(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}
