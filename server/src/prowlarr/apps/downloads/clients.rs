// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Download/DownloadClientBase.cs, TorrentClientBase.cs,
// UsenetClientBase.cs, Clients/Aria2/Aria2.cs, Aria2Proxy.cs, Clients/Flood/Flood.cs, FloodProxy.cs,
// Clients/uTorrent/UTorrent.cs, UTorrentProxy.cs, Clients/Hadouken/Hadouken.cs, HadoukenProxy.cs,
// Clients/Blackhole/TorrentBlackhole.cs, UsenetBlackhole.cs, Clients/Pneumatic/Pneumatic.cs, Clients/Vuze/Vuze.cs),
// GPL-3.0.

//! Talking to each download client: its test (`Test`) and sending it a release (`Download`). The six the
//! download hub drives (and Vuze, which talks Transmission's RPC) go through the hub's own clients; Aria2, Flood,
//! uTorrent, Hadouken and the blackhole folders are here. NZBVortex, Download Station and Freebox are not
//! reached yet: their test and sending say so.

use super::super::provider::{Definition, ProviderFailure};
use super::hub;
use crate::hub::clients::{AddRequest, Backend, Content};
use crate::indexers::cardigann::Grabbed;
use crate::indexers::release::{Protocol, Release};
use crate::AppState;
use base64::Engine;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The address of a client's API: `http(s)://host:port` with its URL base and `path` after it.
fn api_url(def: &Definition, url_base_field: &str, path: &str) -> String {
    let scheme = if def.flag("useSsl") { "https" } else { "http" };
    let base = format!("{scheme}://{}:{}", def.text("host").trim(), def.setting("port").as_i64().unwrap_or(0));
    let ub = def.text(url_base_field);
    let ub = ub.trim().trim_matches('/');
    let mut u = base;
    if !ub.is_empty() {
        u.push('/');
        u.push_str(ub);
    }
    if !path.is_empty() {
        u.push('/');
        u.push_str(path.trim_start_matches('/'));
    }
    u
}

/// What Lantern sends a client: the release's magnet, `.torrent` or `.nzb`.
fn content_of(release: &Release, grabbed: Grabbed) -> Content {
    match grabbed {
        Grabbed::Magnet(m) => Content::Magnet(m),
        Grabbed::File(b) if release.protocol == Protocol::Usenet => Content::Nzb(b),
        Grabbed::File(b) => Content::Torrent(b),
    }
}

/// The name people see for a client's implementation.
fn kind_name(def: &Definition) -> String {
    super::super::provider::implementation_of::<super::DownloadClients>(&def.implementation)
        .map(|i| i.implementation_name.to_string())
        .unwrap_or_else(|| def.implementation.clone())
}

/// The clients Lantern doesn't reach yet.
fn not_reached(def: &Definition) -> String {
    format!("Lantern can't send downloads to {} yet. Use another download client for now.", kind_name(def))
}

/// Tests a client (`DownloadClientBase.Test`): the failures, none when it answers as it should.
pub async fn test(st: &AppState, def: &Definition) -> Vec<ProviderFailure> {
    let r = match def.implementation.as_str() {
        "Aria2" => aria2_version(def).await.map(|_| ()),
        "Flood" => flood_session(def).await.map(|_| ()),
        "UTorrent" => utorrent_session(def).await.and_then(|s| {
            if s.build > 0 && s.build < 25406 {
                Err("Old uTorrent client with unsupported API, need 3.0 or higher".into())
            } else {
                Ok(())
            }
        }),
        "Hadouken" => hadouken(def, "core.getSystemInfo", json!([])).await.map(|_| ()),
        "TorrentBlackhole" => return folder_test(st, &def.text("torrentFolder"), "TorrentFolder"),
        "UsenetBlackhole" => return folder_test(st, &def.text("nzbFolder"), "NzbFolder"),
        "Pneumatic" => {
            let mut f = folder_test(st, &def.text("nzbFolder"), "NzbFolder");
            f.extend(folder_test(st, &def.text("strmFolder"), "StrmFolder"));
            return f;
        }
        "NzbVortex" | "TorrentDownloadStation" | "UsenetDownloadStation" | "TorrentFreeboxDownload" => Err(not_reached(def)),
        _ => match hub::config_of(&st.db, def) {
            Some(cfg) => match Backend::new(&cfg) {
                Ok(b) => b.test().await.map(|_| ()),
                Err(e) => Err(e),
            },
            None => Err(not_reached(def)),
        },
    };
    match r {
        Ok(()) => Vec::new(),
        Err(e) => vec![ProviderFailure::new("Host", e)],
    }
}

/// Sends a release to a client (`Download`): the client's id for the download when it says one.
pub async fn add(st: &AppState, def: &Definition, release: &Release, grabbed: Grabbed, category: Option<String>) -> Result<String, String> {
    let category = category.unwrap_or_else(|| def.text("category"));
    let content = content_of(release, grabbed);
    match def.implementation.as_str() {
        "Aria2" => aria2_add(def, &content).await,
        "Flood" => flood_add(def, release, &content).await,
        "UTorrent" => utorrent_add(def, &content, &category).await,
        "Hadouken" => {
            let (kind, data) = match &content {
                Content::Magnet(m) => ("url", m.clone()),
                Content::Torrent(b) | Content::Nzb(b) => ("file", base64::engine::general_purpose::STANDARD.encode(b)),
            };
            hadouken(def, "webui.addTorrent", json!([kind, data, { "label": category }])).await.map(|_| String::new())
        }
        "TorrentBlackhole" => blackhole_torrent(st, def, release, &content),
        "UsenetBlackhole" => match &content {
            Content::Nzb(b) => {
                write_into(st, &def.text("nzbFolder"), &format!("{}.nzb", clean_file_name(&release.title)), b).map(|_| String::new())
            }
            _ => Err("Usenet Blackhole only takes .nzb files.".into()),
        },
        "Pneumatic" => pneumatic(st, def, release, &content),
        "NzbVortex" | "TorrentDownloadStation" | "UsenetDownloadStation" | "TorrentFreeboxDownload" => Err(not_reached(def)),
        _ => {
            let cfg = hub::config_of(&st.db, def).ok_or_else(|| not_reached(def))?;
            let backend = st.hub.backend(&cfg).map_err(|e| e.detail)?;
            if def.implementation == "Deluge" && !category.is_empty() {
                backend.ensure_category(&category, "").await?;
            }
            let save_path = def.text("directory");
            let req = AddRequest { content, title: release.title.clone(), category, save_path, tag: String::new() };
            backend.add(&req).await
        }
    }
}

// ---- folders (blackholes, Pneumatic) ----

/// A folder Lantern may write in: inside its data folder (Lantern writes nowhere else).
fn writable_folder(st: &AppState, folder: &str) -> Result<PathBuf, String> {
    let p = Path::new(folder.trim());
    let data = st.data_dir.canonicalize().unwrap_or_else(|_| st.data_dir.clone());
    let full = p.canonicalize().map_err(|_| "Folder does not exist".to_string())?;
    if !full.starts_with(&data) {
        return Err(format!("Lantern only writes inside its data folder ({}). Pick a folder there.", data.display()));
    }
    Ok(full)
}

/// `TestFolder`: the folder exists, is inside the data folder and can be written to.
fn folder_test(st: &AppState, folder: &str, property: &str) -> Vec<ProviderFailure> {
    let r = writable_folder(st, folder).and_then(|dir| {
        let probe = dir.join(format!(".lantern-test-{}", std::process::id()));
        std::fs::write(&probe, b"").map_err(|_| "Unable to write to folder".to_string())?;
        let _ = std::fs::remove_file(&probe);
        Ok(())
    });
    match r {
        Ok(()) => Vec::new(),
        Err(e) => vec![ProviderFailure::new(property, e)],
    }
}

/// `FileNameBuilder.CleanFileName`: a release title made safe as a file name.
pub fn clean_file_name(title: &str) -> String {
    let bad = ['\\', '/', '<', '>', '?', '*', ':', '|', '"'];
    let good = ["+", "+", "", "", "!", "-", "-", "", ""];
    let mut out = String::new();
    for c in title.chars() {
        match bad.iter().position(|b| *b == c) {
            Some(i) => out.push_str(good[i]),
            None if c.is_control() => {}
            None => out.push(c),
        }
    }
    out.trim().to_string()
}

/// Writes `bytes` as `name` into `folder`; the file's path.
fn write_into(st: &AppState, folder: &str, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = writable_folder(st, folder)?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|e| format!("Lantern can't write {}: {e}", path.display()))?;
    Ok(path)
}

/// `TorrentBlackhole`: the `.torrent` file, or the magnet in a file of its own when magnet files are saved.
fn blackhole_torrent(st: &AppState, def: &Definition, release: &Release, content: &Content) -> Result<String, String> {
    let title = clean_file_name(&release.title);
    let folder = def.text("torrentFolder");
    match content {
        Content::Magnet(m) if def.flag("saveMagnetFiles") => {
            let ext = def.text("magnetFileExtension");
            let ext = ext.trim();
            let ext = if ext.starts_with('.') { ext.to_string() } else { format!(".{ext}") };
            write_into(st, &folder, &format!("{title}{ext}"), m.as_bytes())?;
        }
        Content::Magnet(_) => return Err("Blackhole does not support magnet links.".into()),
        Content::Torrent(b) | Content::Nzb(b) => {
            write_into(st, &folder, &format!("{title}.torrent"), b)?;
        }
    }
    Ok(String::new())
}

/// `Pneumatic`: the `.nzb` in its folder and a `.strm` file for Kodi that points at it.
fn pneumatic(st: &AppState, def: &Definition, release: &Release, content: &Content) -> Result<String, String> {
    let Content::Nzb(b) = content else { return Err("Pneumatic only takes .nzb files.".into()) };
    let title = clean_file_name(&release.title);
    let nzb = write_into(st, &def.text("nzbFolder"), &format!("{title}.nzb"), b)?;
    let strm = format!("plugin://plugin.program.pneumatic/?mode=strm&type=add_file&nzb={}&nzbname={title}", nzb.display());
    let file = write_into(st, &def.text("strmFolder"), &format!("{title}.strm"), strm.as_bytes())?;
    Ok(format!("Pneumatic_{}", file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
}

// ---- Aria2 (XML-RPC) ----

/// One XML-RPC value: text, a list of texts, a file or a struct of texts (what `XmlRpcRequestBuilder` writes).
enum Xml {
    /// `<string>`.
    Str(String),
    /// `<array>` of strings.
    List(Vec<String>),
    /// `<base64>`.
    Bytes(Vec<u8>),
    /// `<struct>` of strings.
    Struct(Vec<(String, String)>),
}

/// Text made safe inside XML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// An XML-RPC call's body.
fn xml_call(method: &str, params: &[Xml]) -> String {
    let value = |p: &Xml| match p {
        Xml::Str(s) => format!("<value><string>{}</string></value>", esc(s)),
        Xml::List(l) => format!(
            "<value><array><data>{}</data></array></value>",
            l.iter().map(|s| format!("<value><string>{}</string></value>", esc(s))).collect::<String>()
        ),
        Xml::Bytes(b) => format!("<value><base64>{}</base64></value>", base64::engine::general_purpose::STANDARD.encode(b)),
        Xml::Struct(m) => format!(
            "<value><struct>{}</struct></value>",
            m.iter()
                .map(|(k, v)| format!("<member><name>{}</name><value><string>{}</string></value></member>", esc(k), esc(v)))
                .collect::<String>()
        ),
    };
    let ps: String = params.iter().map(|p| format!("<param>{}</param>", value(p))).collect();
    format!("<?xml version=\"1.0\"?><methodCall><methodName>{method}</methodName><params>{ps}</params></methodCall>")
}

/// Calls Aria2: the answer's text, or why it failed.
async fn aria2_call(def: &Definition, method: &str, mut params: Vec<Xml>) -> Result<String, String> {
    let url = api_url(def, "rpcPath", "");
    params.insert(0, Xml::Str(format!("token:{}", def.text("secretToken"))));
    let http = crate::hub::clients::http_client(false);
    let r = http
        .post(&url)
        .header("Content-Type", "text/xml")
        .body(xml_call(method, &params))
        .send()
        .await
        .map_err(|e| crate::hub::clients::unreachable("Aria2", &url, &e))?;
    let status = r.status();
    let text = r.text().await.map_err(|e| format!("Aria2 didn't answer properly: {e}"))?;
    if !status.is_success() {
        return Err(format!("Aria2 answered {status}. Check the address and the secret token."));
    }
    if text.contains("<fault>") {
        let msg = between(&text, "<name>faultString</name>", "</value>").map(strip_tags).unwrap_or_default();
        return Err(format!("Aria2 refused it: {}", msg.trim()));
    }
    Ok(text)
}

/// The text between two marks.
fn between<'a>(s: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let i = s.find(from)? + from.len();
    let j = s[i..].find(to)? + i;
    Some(&s[i..j])
}

/// XML without its tags.
fn strip_tags(s: &str) -> String {
    regex::Regex::new("<[^>]*>").expect("a fixed pattern").replace_all(s, "").to_string()
}

/// Aria2's version (`aria2.getVersion`).
async fn aria2_version(def: &Definition) -> Result<String, String> {
    let text = aria2_call(def, "aria2.getVersion", vec![]).await?;
    let v = between(&text, "<name>version</name>", "</value>").map(strip_tags).unwrap_or_default();
    let parts: Vec<u32> = v.trim().split('.').filter_map(|p| p.parse().ok()).collect();
    if parts.len() >= 2 && (parts[0], parts[1]) < (1, 34) {
        return Err(format!("Aria2 version should be at least 1.34.0. Version reported is {}", v.trim()));
    }
    Ok(v.trim().to_string())
}

/// Adds to Aria2 (`aria2.addUri` or `aria2.addTorrent`, into the directory when one is set): its gid.
async fn aria2_add(def: &Definition, content: &Content) -> Result<String, String> {
    let mut options = Vec::new();
    if !def.text("directory").trim().is_empty() {
        options.push(("dir".to_string(), def.text("directory")));
    }
    let text = match content {
        Content::Magnet(m) => aria2_call(def, "aria2.addUri", vec![Xml::List(vec![m.clone()]), Xml::Struct(options)]).await?,
        Content::Torrent(b) | Content::Nzb(b) => {
            aria2_call(def, "aria2.addTorrent", vec![Xml::Bytes(b.clone()), Xml::List(vec![]), Xml::Struct(options)]).await?
        }
    };
    Ok(between(&text, "<string>", "</string>").unwrap_or("").to_string())
}

// ---- Flood ----

/// A signed-in Flood session: its address and HTTP client (holding the cookie).
async fn flood_session(def: &Definition) -> Result<(String, reqwest::Client), String> {
    let base = format!("{}/api", api_url(def, "urlBase", ""));
    let http = crate::hub::clients::http_client(true);
    let body = json!({ "username": def.text("username"), "password": def.text("password") });
    let r = http
        .post(format!("{base}/auth/authenticate"))
        .json(&body)
        .send()
        .await
        .map_err(|e| crate::hub::clients::unreachable("Flood", &base, &e))?;
    if r.status().as_u16() == 401 {
        return Err("Flood didn't take the user name or password.".into());
    }
    if !r.status().is_success() {
        return Err(format!("Flood answered {}. Check the address.", r.status()));
    }
    let v = http.get(format!("{base}/auth/verify")).send().await.map_err(|e| crate::hub::clients::unreachable("Flood", &base, &e))?;
    if !v.status().is_success() {
        return Err("Flood didn't take the user name or password.".into());
    }
    Ok((base, http))
}

/// Adds to Flood (`add-urls` or `add-files`) with its tags (and the indexer's name when that tag is chosen).
async fn flood_add(def: &Definition, release: &Release, content: &Content) -> Result<String, String> {
    let (base, http) = flood_session(def).await?;
    let mut tags: Vec<String> =
        def.setting("tags").as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if def.ints("additionalTags").contains(&5) && !release.indexer.is_empty() {
        tags.push(release.indexer.clone());
    }
    let (path, mut body) = match content {
        Content::Magnet(m) => ("torrents/add-urls", json!({ "urls": [m], "tags": tags })),
        Content::Torrent(b) | Content::Nzb(b) => {
            ("torrents/add-files", json!({ "files": [base64::engine::general_purpose::STANDARD.encode(b)], "tags": tags }))
        }
    };
    if let Some(d) = def.setting("destination").as_str() {
        body["destination"] = json!(d);
    }
    if !def.flag("addPaused") {
        body["start"] = json!(true);
    }
    let r =
        http.post(format!("{base}/{path}")).json(&body).send().await.map_err(|e| crate::hub::clients::unreachable("Flood", &base, &e))?;
    if !r.status().is_success() {
        return Err(format!("Flood refused the torrent ({}).", r.status()));
    }
    Ok(String::new())
}

// ---- uTorrent ----

/// A uTorrent session: its address, token, HTTP client (holding the GUID cookie) and build.
struct UTorrentSession {
    /// `…/gui/`.
    base: String,
    /// The CSRF token from `token.html`.
    token: String,
    /// The client, with the cookie.
    http: reqwest::Client,
    /// uTorrent's build number (0 when it didn't say).
    build: i64,
}

/// Signs in to uTorrent (`token.html`) and asks its settings for the build.
async fn utorrent_session(def: &Definition) -> Result<UTorrentSession, String> {
    let ub = def.text("urlBase");
    let ub = if ub.trim().is_empty() { "gui".to_string() } else { ub.trim().trim_matches('/').to_string() };
    let scheme = if def.flag("useSsl") { "https" } else { "http" };
    let base = format!("{scheme}://{}:{}/{ub}/", def.text("host").trim(), def.setting("port").as_i64().unwrap_or(0));
    let http = crate::hub::clients::http_client(true);
    let auth = |b: reqwest::RequestBuilder| b.basic_auth(def.text("username"), Some(def.text("password")));
    let r =
        auth(http.get(format!("{base}token.html"))).send().await.map_err(|e| crate::hub::clients::unreachable("uTorrent", &base, &e))?;
    if r.status().as_u16() == 401 {
        return Err("uTorrent didn't take the user name or password.".into());
    }
    let html = r.text().await.unwrap_or_default();
    let token = regex::Regex::new(r#"<div[^>]*id=['"]token['"][^>]*>([^<]+)</div>"#)
        .expect("a fixed pattern")
        .captures(&html)
        .map(|c| c[1].to_string())
        .ok_or("uTorrent didn't give a token. Check the address.")?;
    let settings = auth(http.get(format!("{base}?token={token}&action=getsettings")))
        .send()
        .await
        .map_err(|e| crate::hub::clients::unreachable("uTorrent", &base, &e))?
        .json::<Value>()
        .await
        .unwrap_or(Value::Null);
    let build = settings.get("build").and_then(Value::as_i64).unwrap_or(0);
    Ok(UTorrentSession { base, token, http, build })
}

/// Adds to uTorrent (`add-url` or `add-file`) and labels it with the category.
async fn utorrent_add(def: &Definition, content: &Content, category: &str) -> Result<String, String> {
    let s = utorrent_session(def).await?;
    let auth = |b: reqwest::RequestBuilder| b.basic_auth(def.text("username"), Some(def.text("password")));
    let r = match content {
        Content::Magnet(m) => {
            let url = reqwest::Url::parse_with_params(&s.base, &[("token", s.token.as_str()), ("action", "add-url"), ("s", m.as_str())])
                .map_err(|e| e.to_string())?;
            auth(s.http.get(url)).send().await
        }
        Content::Torrent(b) | Content::Nzb(b) => {
            let part = reqwest::multipart::Part::bytes(b.clone()).file_name("torrent.torrent");
            let form = reqwest::multipart::Form::new().part("torrent_file", part);
            auth(s.http.post(format!("{}?token={}&action=add-file", s.base, s.token)).multipart(form)).send().await
        }
    }
    .map_err(|e| crate::hub::clients::unreachable("uTorrent", &s.base, &e))?;
    if !r.status().is_success() {
        return Err(format!("uTorrent refused the torrent ({}).", r.status()));
    }
    let hash = match content {
        Content::Magnet(m) => crate::hub::torrent::magnet_hash(m).unwrap_or_default(),
        Content::Torrent(b) | Content::Nzb(b) => crate::hub::torrent::info_hash(b).unwrap_or_default(),
    }
    .to_uppercase();
    if !hash.is_empty() && !category.is_empty() {
        let url = reqwest::Url::parse_with_params(
            &s.base,
            &[("token", s.token.as_str()), ("action", "setprops"), ("hash", hash.as_str()), ("s", "label"), ("v", category)],
        )
        .map_err(|e| e.to_string())?;
        let _ = auth(s.http.get(url)).send().await;
    }
    Ok(hash)
}

// ---- Hadouken (JSON-RPC) ----

/// Calls Hadouken's JSON-RPC (`/api`): the result, or why it failed.
async fn hadouken(def: &Definition, method: &str, params: Value) -> Result<Value, String> {
    let url = api_url(def, "urlBase", "api");
    let http = crate::hub::clients::http_client(false);
    let body = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": &uuid_short() });
    let r = http
        .post(&url)
        .basic_auth(def.text("username"), Some(def.text("password")))
        .json(&body)
        .send()
        .await
        .map_err(|e| crate::hub::clients::unreachable("Hadouken", &url, &e))?;
    if r.status().as_u16() == 401 {
        return Err("Hadouken didn't take the user name or password.".into());
    }
    if !r.status().is_success() {
        return Err(format!("Hadouken answered {}. Check the address.", r.status()));
    }
    let v: Value = r.json().await.map_err(|e| format!("Hadouken didn't answer properly: {e}"))?;
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        return Err(format!("Hadouken refused it: {}", e.get("message").and_then(Value::as_str).unwrap_or("unknown error")));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// A request id as Prowlarr's JSON-RPC builder makes one (8 hex characters).
fn uuid_short() -> String {
    format!("{:08x}", rand::random::<u32>())
}
