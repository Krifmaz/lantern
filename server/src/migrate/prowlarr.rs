//! Prowlarr → Lantern: its indexers (with their logins and keys), the apps it synced to, its API key, port and
//! proxy, read from a backup (`.zip`), its database (`prowlarr.db`) or its API, then matched against Lantern.
//!
//! Prowlarr keeps each indexer's settings as JSON. A Cardigann site has `definitionFile`, `baseUrl` and the site's
//! own settings in `extraFieldData`; a select there is the position of the chosen option among the options sorted
//! by value, which [`setting_text`] turns back into the value. Newznab and Torznab indexers have `baseUrl`,
//! `apiPath`, `apiKey` and `additionalParameters`. Prowlarr's API sends `********` instead of passwords and keys;
//! those come over as settings to fill in again.

use super::{masked, Action, Options, Plan, Secrets, Source, SourceApp, SourceIndexer, SourceProxy, Step};
use crate::indexers::definition::{Definition, Setting};
use crate::indexers::library::Library;
use crate::indexers::{newznab, Indexer};
use crate::AppState;
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// The most a database inside a backup may unpack to (a backup holding more is refused).
const MAX_DB_BYTES: u64 = 4 << 30;

/// What an upload is, by its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upload {
    /// A zip file (Prowlarr's backups).
    Zip,
    /// An SQLite database (`prowlarr.db`).
    Database,
}

/// What `head`, the first bytes of an upload, says it is; `None` for neither.
pub fn kind_of(head: &[u8]) -> Option<Upload> {
    if head.starts_with(b"PK\x03\x04") {
        Some(Upload::Zip)
    } else if head.starts_with(b"SQLite format 3\0") {
        Some(Upload::Database)
    } else {
        None
    }
}

/// Reads the uploaded file at `path`: a Prowlarr backup or its database. `scratch` is a file name the database
/// inside a backup can be unpacked to (the caller removes it).
pub fn read_upload(path: &Path, scratch: &Path) -> Result<Source> {
    let mut head = [0u8; 16];
    let n =
        std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).map_err(|e| err(500, format!("The upload can't be read: {e}")))?;
    match kind_of(&head[..n]) {
        Some(Upload::Zip) => read_backup(path, scratch),
        Some(Upload::Database) => read_db(path, "Prowlarr's database"),
        None => Err(err(
            400,
            "That file isn't a Prowlarr backup or database. In Prowlarr, open System → Backup, make a backup and upload its .zip file.",
        )),
    }
}

/// Reads a Prowlarr backup (`.zip` with `prowlarr.db` and `config.xml`), unpacking the database to `scratch`.
pub fn read_backup(zip_path: &Path, scratch: &Path) -> Result<Source> {
    let bad =
        |e: &dyn std::fmt::Display| err(400, format!("The backup can't be opened: {e}. Make a new backup in Prowlarr and try again."));
    let file = std::fs::File::open(zip_path).map_err(|e| err(500, format!("The upload can't be read: {e}")))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(&e))?;
    let mut config = String::new();
    let mut db_entry = None;
    for i in 0..zip.len() {
        let name = zip.by_index(i).map_err(|e| bad(&e))?.name().to_lowercase();
        let base = name.rsplit('/').next().unwrap_or(&name).to_string();
        if base == "prowlarr.db" {
            db_entry = Some(i);
        } else if base == "config.xml" {
            zip.by_index(i).map_err(|e| bad(&e))?.take(1 << 20).read_to_string(&mut config).map_err(|e| bad(&e))?;
        }
    }
    let i = db_entry.ok_or_else(|| {
        err(400, "That zip file has no prowlarr.db in it. In Prowlarr, open System → Backup, make a backup and upload its .zip file.")
    })?;
    {
        let mut entry = zip.by_index(i).map_err(|e| bad(&e))?;
        if entry.size() > MAX_DB_BYTES {
            return Err(err(413, "The database in that backup is too big to import (more than 4 GB)."));
        }
        let mut out = std::fs::File::create(scratch).map_err(|e| err(500, format!("The data folder can't be written: {e}")))?;
        std::io::copy(&mut (&mut entry).take(MAX_DB_BYTES), &mut out).map_err(|e| bad(&e))?;
    }
    let mut src = read_db(scratch, "a Prowlarr backup")?;
    let cfg = config_xml(&config);
    src.api_key = cfg.api_key.or(src.api_key);
    src.port = cfg.port.or(src.port);
    if !cfg.url_base.is_empty() {
        src.url_base = cfg.url_base;
    }
    Ok(src)
}

/// What Prowlarr's `config.xml` says.
#[derive(Debug, Default, PartialEq)]
pub struct ConfigXml {
    /// Its API key.
    pub api_key: Option<String>,
    /// The port it listened on.
    pub port: Option<u16>,
    /// The path it was served under (`/prowlarr`), "" for none.
    pub url_base: String,
}

/// Reads Prowlarr's `config.xml` (a flat list of `<Name>value</Name>`).
pub fn config_xml(text: &str) -> ConfigXml {
    let tag = |name: &str| {
        regex::Regex::new(&format!(r"(?s)<{name}>\s*(.*?)\s*</{name}>"))
            .ok()
            .and_then(|r| r.captures(text).map(|c| html_escape::decode_html_entities(&c[1]).trim().to_string()))
            .filter(|v| !v.is_empty())
    };
    let url_base = tag("UrlBase").unwrap_or_default();
    ConfigXml {
        api_key: tag("ApiKey"),
        port: tag("Port").and_then(|p| p.parse().ok()),
        url_base: if url_base.is_empty() || url_base.starts_with('/') { url_base } else { format!("/{url_base}") },
    }
}

/// Reads Prowlarr's database at `path`; `from` says in words what it was.
pub fn read_db(path: &Path, from: &str) -> Result<Source> {
    let not_prowlarr =
        || err(400, "That database isn't Prowlarr's. Upload prowlarr.db from Prowlarr's data folder, or a backup from System → Backup.");
    let db = SqliteDb::open(path).map_err(|_| not_prowlarr())?;
    let tables: HashSet<String> = db
        .query("SELECT name FROM sqlite_master WHERE type='table'", p![])
        .map_err(|_| not_prowlarr())?
        .iter()
        .map(|r| r.s("name"))
        .collect();
    if !tables.contains("Indexers") {
        return Err(not_prowlarr());
    }
    let rows = |table: &str| -> Vec<Row> {
        if tables.contains(table) {
            db.query(&format!("SELECT * FROM \"{table}\" ORDER BY Id"), p![]).unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    let tags: HashMap<i64, String> = rows("Tags").iter().map(|r| (r.i("Id"), r.s("Label"))).collect();
    let settings = |r: &Row| serde_json::from_str::<Value>(&r.s("Settings")).unwrap_or(json!({}));
    let tag_names = |r: &Row| tag_labels(&serde_json::from_str::<Value>(&r.s("Tags")).unwrap_or(json!([])), &tags);
    let mut src = Source { from: from.to_string(), ..Default::default() };
    for r in rows("Indexers") {
        // `Enable` came in a later Prowlarr; before it, the search switches said the same
        let enabled = if r.contains_key("Enable") {
            r.b("Enable")
        } else {
            r.b("EnableRss") || r.b("EnableAutomaticSearch") || r.b("EnableInteractiveSearch")
        };
        let priority = if r.i("Priority") > 0 { r.i("Priority") } else { 25 };
        src.indexers.push(indexer_from(r.i("Id"), &r.s("Name"), &r.s("Implementation"), enabled, priority, tag_names(&r), settings(&r)));
    }
    for r in rows("Applications") {
        let level = match r.i("SyncLevel") {
            0 => "disabled",
            1 => "add_remove",
            _ => "full",
        };
        src.apps.push(app_from(&r.s("Name"), &r.s("Implementation"), level, tag_names(&r), &settings(&r)));
    }
    let config: HashMap<String, String> = rows("Config").iter().map(|r| (r.s("Key").to_lowercase(), r.s("Value"))).collect();
    if let Some(p) = own_proxy(|k| config.get(k).cloned().unwrap_or_default()) {
        src.proxies.push(p);
    }
    for r in rows("IndexerProxies") {
        src.proxies.push(proxy_from(&r.s("Name"), &r.s("Implementation"), tag_names(&r), &settings(&r)));
    }
    src.download_clients = rows("DownloadClients").iter().map(|r| r.s("Name")).collect();
    src.notifications = rows("Notifications").len();
    Ok(src)
}

/// The tag names of a list of tag ids.
fn tag_labels(ids: &Value, tags: &HashMap<i64, String>) -> Vec<String> {
    ids.as_array().map(|a| a.iter().filter_map(Value::as_i64).filter_map(|i| tags.get(&i).cloned()).collect()).unwrap_or_default()
}

/// An indexer from Prowlarr's settings JSON (database) or fields (API), flattened.
pub fn indexer_from(
    id: i64,
    name: &str,
    implementation: &str,
    enabled: bool,
    priority: i64,
    tags: Vec<String>,
    settings: Value,
) -> SourceIndexer {
    let mut fields = BTreeMap::new();
    let mut extra = Map::new();
    if let Value::Object(m) = settings {
        for (k, v) in m {
            match k.as_str() {
                "extraFieldData" => {
                    if let Value::Object(x) = v {
                        extra = x;
                    }
                }
                // query and grab limits, seed goals: Lantern has no such settings per indexer yet
                "baseSettings" | "torrentBaseSettings" => {}
                k if k.contains('.') => {}
                _ => {
                    fields.insert(k, v);
                }
            }
        }
    }
    fields.extend(extra);
    let text = |v: Option<Value>| match v {
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    };
    let base_url = text(fields.remove("baseUrl"));
    let definition = text(fields.remove("definitionFile"));
    SourceIndexer {
        id,
        name: name.trim().to_string(),
        implementation: implementation.to_string(),
        definition,
        enabled,
        priority,
        tags,
        base_url,
        fields,
    }
}

/// An app from Prowlarr's settings JSON (or fields).
fn app_from(name: &str, implementation: &str, sync_level: &str, tags: Vec<String>, s: &Value) -> SourceApp {
    let text = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    SourceApp {
        name: name.trim().to_string(),
        implementation: implementation.to_string(),
        base_url: text("baseUrl"),
        api_key: text("apiKey"),
        source_url: text("prowlarrUrl"),
        sync_level: sync_level.to_string(),
        tags,
        categories: s
            .get("syncCategories")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default(),
    }
}

/// An indexer proxy from Prowlarr's settings JSON (or fields).
fn proxy_from(name: &str, implementation: &str, tags: Vec<String>, s: &Value) -> SourceProxy {
    let text = |k: &str| match s.get(k) {
        Some(Value::String(v)) => v.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    SourceProxy {
        name: name.trim().to_string(),
        implementation: implementation.to_string(),
        host: text("host"),
        port: text("port").parse().unwrap_or(0),
        username: text("username"),
        password: text("password"),
        tags,
        everywhere: false,
    }
}

/// Prowlarr's own proxy (Settings → General → Proxy), from its settings by lowercase name; `None` when it is off.
fn own_proxy(get: impl Fn(&str) -> String) -> Option<SourceProxy> {
    if !matches!(get("proxyenabled").to_lowercase().as_str(), "true" | "1") {
        return None;
    }
    let implementation = match get("proxytype").to_lowercase().as_str() {
        "1" | "socks4" => "Socks4",
        "2" | "socks5" => "Socks5",
        _ => "Http",
    };
    Some(SourceProxy {
        name: "Prowlarr's proxy".into(),
        implementation: implementation.into(),
        host: get("proxyhostname"),
        port: get("proxyport").parse().unwrap_or(0),
        username: get("proxyusername"),
        password: get("proxypassword"),
        tags: Vec::new(),
        everywhere: true,
    })
}

/// Reads a running Prowlarr through its API at `url` with its API key `key`.
pub async fn read_api(url: &str, key: &str) -> Result<Source> {
    let base = url.trim().trim_end_matches('/').to_string();
    let parsed = url::Url::parse(&base).map_err(|_| err(400, "Enter Prowlarr's address, like http://prowlarr:9696"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(err(400, "Enter Prowlarr's address, like http://prowlarr:9696"));
    }
    if key.trim().is_empty() {
        return Err(err(400, "Enter Prowlarr's API key. It is under Settings → General in Prowlarr."));
    }
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| err(500, format!("Lantern can't make web requests: {e}")))?;
    let get = |path: &'static str| {
        let http = http.clone();
        let base = base.clone();
        let key = key.trim().to_string();
        async move {
            let r = http
                .get(format!("{base}{path}"))
                .header("X-Api-Key", key)
                .send()
                .await
                .map_err(|e| err(400, format!("Lantern couldn't reach Prowlarr at {base}: {e}. Check the address.")))?;
            match r.status().as_u16() {
                401 | 403 => return Err(err(400, "Prowlarr didn't accept that API key. Copy it from Settings → General in Prowlarr.")),
                404 => return Ok(Value::Null),
                s if s >= 400 => return Err(err(400, format!("Prowlarr answered {path} with an error ({s})."))),
                _ => {}
            }
            let body = r.text().await.unwrap_or_default();
            serde_json::from_str::<Value>(&body)
                .map_err(|_| err(400, format!("{base} didn't answer like Prowlarr does. Check the address.")))
        }
    };
    let status = get("/api/v1/system/status").await?;
    match status.get("appName").and_then(Value::as_str) {
        Some("Prowlarr") => {}
        Some(other) => return Err(err(400, format!("That address is {other}, not Prowlarr."))),
        None => return Err(err(400, format!("{base} didn't answer like Prowlarr does. Check the address."))),
    }
    let tags: HashMap<i64, String> = get("/api/v1/tag")
        .await?
        .as_array()
        .map(|a| a.iter().filter_map(|t| Some((t.get("id")?.as_i64()?, t.get("label")?.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let list = |v: Value| v.as_array().cloned().unwrap_or_default();
    let tag_names = |v: &Value| tag_labels(v.get("tags").unwrap_or(&Value::Null), &tags);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut src = Source { from: format!("Prowlarr at {base}"), ..Default::default() };
    for ix in list(get("/api/v1/indexer").await?) {
        let mut fields = fields_of(&ix);
        if text(&ix, "implementation") == "Cardigann" && fields.get("definitionFile").and_then(Value::as_str).unwrap_or("").is_empty() {
            fields.insert("definitionFile".into(), json!(text(&ix, "definitionName")));
        }
        src.indexers.push(indexer_from(
            ix.get("id").and_then(Value::as_i64).unwrap_or(0),
            &text(&ix, "name"),
            &text(&ix, "implementation"),
            ix.get("enable").and_then(Value::as_bool).unwrap_or(true),
            ix.get("priority").and_then(Value::as_i64).unwrap_or(25),
            tag_names(&ix),
            Value::Object(fields),
        ));
    }
    for app in list(get("/api/v1/applications").await?) {
        let level = match text(&app, "syncLevel").as_str() {
            "disabled" => "disabled",
            "addOnly" => "add_remove",
            _ => "full",
        };
        src.apps.push(app_from(
            &text(&app, "name"),
            &text(&app, "implementation"),
            level,
            tag_names(&app),
            &Value::Object(fields_of(&app)),
        ));
    }
    let host = get("/api/v1/config/host").await?;
    if let Some(p) = own_proxy(|k| {
        let v = host.as_object().and_then(|m| m.iter().find(|(name, _)| name.to_lowercase() == k).map(|(_, v)| v.clone()));
        match v {
            Some(Value::String(s)) => s,
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }) {
        src.proxies.push(p);
    }
    for p in list(get("/api/v1/indexerproxy").await?) {
        src.proxies.push(proxy_from(&text(&p, "name"), &text(&p, "implementation"), tag_names(&p), &Value::Object(fields_of(&p))));
    }
    src.api_key = Some(text(&host, "apiKey")).filter(|k| !k.is_empty() && !masked(k)).or_else(|| Some(key.trim().to_string()));
    src.port = host.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok());
    let ub = text(&host, "urlBase");
    src.url_base = if ub.is_empty() || ub.starts_with('/') { ub } else { format!("/{ub}") };
    src.download_clients = list(get("/api/v1/downloadclient").await?).iter().map(|d| text(d, "name")).collect();
    src.notifications = list(get("/api/v1/notification").await?).len();
    Ok(src)
}

/// The `fields` list of a Prowlarr API answer (`[{name, value}]`) as an object.
fn fields_of(v: &Value) -> Map<String, Value> {
    v.get("fields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter().filter_map(|f| Some((f.get("name")?.as_str()?.to_string(), f.get("value").cloned().unwrap_or(Value::Null)))).collect()
        })
        .unwrap_or_default()
}

/// A Prowlarr indexer as Lantern would add it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    /// `cardigann`, `newznab` or `torznab`.
    pub kind: String,
    /// The definition (Cardigann) or preset (Newznab) id.
    pub definition: String,
    /// The name.
    pub name: String,
    /// The site's address ("" for the definition's).
    pub base_url: String,
    /// The settings.
    pub settings: BTreeMap<String, String>,
    /// Which settings are secrets.
    pub secrets: Vec<String>,
    /// The settings to fill in again (labels).
    pub missing: Vec<String>,
    /// What it is, in words.
    pub what: String,
}

/// The value of `name` in `fields`, matching case only when needed (`apiKey` for `apikey`).
fn field<'a>(fields: &'a BTreeMap<String, Value>, name: &str) -> Option<&'a Value> {
    fields.get(name).or_else(|| fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
}

/// A Prowlarr setting value as Lantern's text for definition setting `set`: a select's position becomes the
/// option's value (Prowlarr counts the options sorted by value), a checkbox `true` / `false`. `None` for no value.
pub fn setting_text(set: &Setting, v: &Value) -> Option<String> {
    let plain = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        Value::Array(a) => {
            Some(a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(","))
        }
        other => Some(other.to_string()),
    };
    match set.kind.as_str() {
        "select" => {
            let mut keys: Vec<&String> = set.options.keys().collect();
            keys.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
            let pick = |n: i64| usize::try_from(n).ok().and_then(|n| keys.get(n)).map(|k| k.to_string());
            match v {
                Value::Number(n) => n.as_i64().and_then(pick),
                Value::String(s) if !set.options.contains_key(s) => s.trim().parse::<i64>().ok().and_then(pick).or_else(|| Some(s.clone())),
                other => plain(other),
            }
        }
        "checkbox" => match v {
            Value::Bool(b) => Some(b.to_string()),
            Value::Number(n) => Some((n.as_f64().unwrap_or(0.0) != 0.0).to_string()),
            Value::String(s) => Some(matches!(s.trim().to_lowercase().as_str(), "true" | "1" | "on" | "yes").to_string()),
            _ => None,
        },
        _ => plain(v),
    }
}

/// The settings of definition `def` from Prowlarr's fields of `s`.
fn cardigann(def: &Definition, s: &SourceIndexer) -> Mapped {
    let mut settings = BTreeMap::new();
    let (mut secrets, mut missing) = (Vec::new(), Vec::new());
    for set in def.settings.iter().filter(|x| !x.is_info()) {
        let Some(text) = field(&s.fields, &set.name).and_then(|v| setting_text(set, v)) else { continue };
        if masked(&text) {
            missing.push(if set.label.is_empty() { set.name.clone() } else { set.label.clone() });
            continue;
        }
        if set.is_secret() && !text.is_empty() {
            secrets.push(set.name.clone());
        }
        settings.insert(set.name.clone(), text);
    }
    let first = def.links.first().map(|l| l.trim_end_matches('/').to_string()).unwrap_or_default();
    let base_url = if s.base_url.trim_end_matches('/') == first { String::new() } else { s.base_url.clone() };
    let what = match def.kind.as_str() {
        "public" => "Public torrent site",
        "semi-private" => "Semi-private torrent site",
        _ => "Private torrent site",
    };
    Mapped {
        kind: "cardigann".into(),
        definition: def.id.clone(),
        name: s.name.clone(),
        base_url,
        settings,
        secrets,
        missing,
        what: what.into(),
    }
}

/// The definition for Prowlarr's definition file or built-in indexer `id` (named `name`): the same id, the one that
/// replaced it, or one of the same name.
fn find_definition(lib: &Library, id: &str, name: &str) -> Option<Arc<Definition>> {
    let id = id.trim().to_lowercase();
    if id.is_empty() {
        return None;
    }
    lib.get(&id).or_else(|| {
        let all = lib.all();
        all.iter()
            .find(|d| d.replaces.iter().any(|r| r.eq_ignore_ascii_case(&id)))
            .or_else(|| all.iter().find(|d| d.name.eq_ignore_ascii_case(&id) || (!name.is_empty() && d.name.eq_ignore_ascii_case(name))))
            .cloned()
    })
}

/// How Lantern would add Prowlarr indexer `s`, or why it can't.
pub fn map_indexer(lib: &Library, s: &SourceIndexer) -> std::result::Result<Mapped, String> {
    match s.implementation.as_str() {
        "Cardigann" => {
            if s.definition.is_empty() {
                return Err("Prowlarr didn't say which site definition it uses.".into());
            }
            let def = find_definition(lib, &s.definition, "").ok_or_else(|| {
                format!("Lantern has no definition called {} (it may be a custom one in Prowlarr). Add the site by hand.", s.definition)
            })?;
            Ok(cardigann(&def, s))
        }
        "Newznab" | "Torznab" => {
            let kind = s.implementation.to_lowercase();
            if s.base_url.is_empty() {
                return Err("Prowlarr has no address for it.".into());
            }
            let text = |k: &str| field(&s.fields, k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let mut settings = BTreeMap::new();
            let (mut secrets, mut missing) = (Vec::new(), Vec::new());
            let key = text("apiKey");
            if masked(&key) {
                missing.push("API key".to_string());
            } else if !key.is_empty() {
                settings.insert("apikey".to_string(), key);
                secrets.push("apikey".to_string());
            }
            let path = text("apiPath");
            settings.insert("api_path".to_string(), if path.is_empty() { "/api".to_string() } else { path });
            let extra = text("additionalParameters");
            if !extra.is_empty() {
                settings.insert("extra".to_string(), extra);
            }
            let url = s.base_url.trim_end_matches('/');
            let definition = if kind == "newznab" {
                newznab::PRESETS.iter().find(|(_, _, u)| u.trim_end_matches('/').eq_ignore_ascii_case(url)).map(|(id, _, _)| id.to_string())
            } else {
                None
            };
            Ok(Mapped {
                what: if kind == "newznab" { "Usenet indexer".into() } else { "Torznab indexer".into() },
                kind,
                definition: definition.unwrap_or_default(),
                name: s.name.clone(),
                base_url: url.to_string(),
                settings,
                secrets,
                missing,
            })
        }
        "" => Err("Prowlarr didn't say what kind of indexer it is.".into()),
        built_in => {
            let def = find_definition(lib, built_in, &s.name).ok_or_else(|| {
                format!("Prowlarr has {built_in} built in, and Lantern has no definition for it yet. Add it by hand when one appears.")
            })?;
            Ok(cardigann(&def, s))
        }
    }
}

/// Whether Lantern's indexer `e` is the one `m` would add.
fn same(e: &Indexer, m: &Mapped) -> bool {
    e.kind == m.kind
        && match m.kind.as_str() {
            "cardigann" => e.definition == m.definition && e.name.eq_ignore_ascii_case(&m.name),
            _ => e.name.eq_ignore_ascii_case(&m.name) || e.base_url.trim_end_matches('/').eq_ignore_ascii_case(&m.base_url),
        }
}

/// Matches `src` against Lantern and, unless `opts` says it is a dry run, adds what is new. Nothing in Lantern is
/// changed or removed, except the API key and the proxy for sites when they are empty (or `opts` asks for the key).
pub async fn run(st: &AppState, src: &Source, opts: Options, secrets: &dyn Secrets) -> Result<Plan> {
    let mut plan = Plan { from: src.from.clone(), dry_run: opts.dry_run, ..Default::default() };
    let mut existing = st.indexers.list()?;
    let mut indexers: Vec<&SourceIndexer> = src.indexers.iter().collect();
    indexers.sort_by_key(|s| s.id);
    for s in indexers {
        let what = if s.implementation.is_empty() { "Indexer".to_string() } else { s.implementation.clone() };
        let m = match map_indexer(&st.indexers.library, s) {
            Ok(m) => m,
            Err(why) => {
                plan.indexers.push(Step::new(&s.name, what, Action::Skip, why));
                continue;
            }
        };
        if let Some(e) = existing.iter().find(|e| same(e, &m)) {
            let detail = format!("Lantern has it already, as {}. It was left as it is.", e.name);
            plan.indexers.push(Step::new(&s.name, &m.what, Action::AlreadyThere, detail));
            continue;
        }
        let mut step = Step::new(&s.name, &m.what, Action::Add, "");
        step.secrets = m.secrets.clone();
        step.missing = m.missing.clone();
        let mut words = Vec::new();
        if !s.enabled {
            words.push("It was turned off in Prowlarr and stays off.".to_string());
        }
        if !step.missing.is_empty() {
            words.push(format!(
                "Prowlarr didn't hand over {}; fill {} in after the import.",
                step.missing.join(", "),
                if step.missing.len() == 1 { "it" } else { "them" }
            ));
        }
        if !opts.dry_run {
            match add(st, s, &m, secrets) {
                Ok((id, kept)) => {
                    words.push(if kept || s.id <= 0 {
                        format!("Its number is {id}, as in Prowlarr, so apps' addresses for it still work.")
                    } else {
                        format!("Its number is {id} (it was {} in Prowlarr); apps that used /{}/api need /{id}/api.", s.id, s.id)
                    });
                    if let Ok(ix) = st.indexers.get(id) {
                        existing.push(ix);
                    }
                }
                Err(e) => {
                    step.action = Action::Skip;
                    words = vec![format!("It couldn't be added: {}", e.detail)];
                }
            }
        }
        step.detail = words.join(" ");
        plan.indexers.push(step);
    }
    apps(st, src, opts, secrets, &mut plan)?;
    settings(st, src, opts, &mut plan).await?;
    if !src.download_clients.is_empty() {
        plan.notes.push(format!(
            "Prowlarr's download clients ({}) weren't brought over. Add them to Lantern's download hub; apps that send grabs to Lantern then use them.",
            src.download_clients.join(", ")
        ));
    }
    if src.notifications > 0 {
        plan.notes.push(format!(
            "Prowlarr's {} notification {} weren't brought over.",
            src.notifications,
            if src.notifications == 1 { "connection" } else { "connections" }
        ));
    }
    plan.notes.push("Prowlarr's history, stats, sign-in users and per-indexer query, grab and seed limits aren't brought over.".into());
    plan.summarize();
    tracing::info!("Import from {}: {}", plan.from, plan.summary);
    Ok(plan)
}

/// Adds `m` (from `s`) to Lantern with its secrets kept by `secrets`, under Prowlarr's number when it is free; the
/// new id and whether Prowlarr's number was kept.
fn add(st: &AppState, s: &SourceIndexer, m: &Mapped, secrets: &dyn Secrets) -> Result<(i64, bool)> {
    let mut settings = m.settings.clone();
    for k in &m.secrets {
        if let Some(v) = settings.get(k).cloned() {
            settings.insert(k.clone(), secrets.keep(&format!("indexer {}", m.name), k, &v)?);
        }
    }
    let body = json!({
        "kind": m.kind,
        "definition": m.definition,
        "name": m.name,
        "base_url": m.base_url,
        "settings": settings,
        "enabled": s.enabled,
        "priority": s.priority.clamp(1, 50),
        "tags": s.tags.join(","),
    });
    let id = st.indexers.add(&body)?;
    if s.id > 0 && s.id != id && st.indexers.get(s.id).is_err() {
        st.indexers.db.execute("UPDATE indexers SET id=? WHERE id=?", p![s.id, id])?;
        return Ok((s.id, true));
    }
    Ok((id, s.id == id))
}

/// The apps' lines: each is kept for app sync, its API key through `secrets`.
fn apps(st: &AppState, src: &Source, opts: Options, secrets: &dyn Secrets, plan: &mut Plan) -> Result<()> {
    let have: Vec<String> = st.db.query("SELECT name FROM imported_apps", p![])?.iter().map(|r| r.s("name").to_lowercase()).collect();
    let mut added: Vec<String> = Vec::new();
    for a in &src.apps {
        let what = if a.implementation.is_empty() { "App".to_string() } else { format!("App ({})", a.implementation) };
        let lower = a.name.to_lowercase();
        if have.contains(&lower) || added.contains(&lower) {
            plan.apps.push(Step::new(&a.name, what, Action::AlreadyThere, "It was imported before; it was left as it is."));
            continue;
        }
        let mut step = Step::new(
            &a.name,
            what,
            Action::Add,
            format!(
                "Kept for app sync (Lantern pushing its indexers to apps), which comes later. If Lantern takes over Prowlarr's address and API key, {} searches through Lantern without changes.",
                a.name
            ),
        );
        if a.sync_level == "disabled" {
            step.detail.push_str(" Sync was turned off for it in Prowlarr.");
        }
        let key = if masked(&a.api_key) {
            step.missing.push("API key".into());
            String::new()
        } else {
            if !a.api_key.is_empty() {
                step.secrets.push("api_key".into());
            }
            a.api_key.clone()
        };
        if !opts.dry_run {
            let key = if key.is_empty() { key } else { secrets.keep(&format!("app {}", a.name), "api_key", &key)? };
            st.db.execute(
                "INSERT INTO imported_apps (name, kind, base_url, api_key, source_url, sync_level, tags, categories, imported) VALUES (?,?,?,?,?,?,?,?,?)",
                p![
                    a.name.clone(),
                    a.implementation.clone(),
                    a.base_url.clone(),
                    key,
                    a.source_url.clone(),
                    a.sync_level.clone(),
                    a.tags.join(","),
                    serde_json::to_string(&a.categories).unwrap_or_else(|_| "[]".into()),
                    crate::auth::now()
                ],
            )?;
        }
        added.push(lower);
        plan.apps.push(step);
    }
    Ok(())
}

/// The settings' lines: the API key, the port and path, the proxies.
async fn settings(st: &AppState, src: &Source, opts: Options, plan: &mut Plan) -> Result<()> {
    let mut set = Map::new();
    if let Some(key) = src.api_key.as_deref().filter(|k| !k.is_empty() && !masked(k)) {
        let what = "Setting";
        let step = if st.settings.str("api_key") == key {
            Step::new("API key", what, Action::AlreadyThere, "Lantern already uses Prowlarr's API key.")
        } else if !opts.take_api_key {
            Step::new(
                "API key",
                what,
                Action::Skip,
                "You chose to keep Lantern's own API key. Apps that used Prowlarr's key need Lantern's.",
            )
        } else if st.settings.env_locked("api_key") {
            Step::new(
                "API key",
                what,
                Action::Manual,
                "Lantern's API key is set by APP_API_KEY in its environment. Put Prowlarr's key there so apps keep working without changes.",
            )
        } else {
            set.insert("api_key".into(), json!(key));
            let mut s = Step::new(
                "API key",
                what,
                Action::Set,
                "Lantern takes Prowlarr's API key, so apps that used Prowlarr keep working without changes. Apps already using Lantern's old key need the new one.",
            );
            s.secrets.push("api_key".into());
            s
        };
        plan.settings.push(step);
    }
    if let Some(port) = src.port {
        let ours = crate::port();
        plan.settings.push(if port == ours {
            Step::new("Port", "Setting", Action::NothingNeeded, format!("Lantern listens on port {port}, as Prowlarr did."))
        } else {
            Step::new(
                "Port",
                "Setting",
                Action::Manual,
                format!("Prowlarr listened on port {port} and Lantern listens on {ours}. Set APP_PORT={port} for Lantern, or change the address in each app."),
            )
        });
    }
    if !src.url_base.is_empty() {
        plan.settings.push(Step::new(
            "Address path",
            "Setting",
            Action::Manual,
            format!(
                "Prowlarr was served under {0}, and Lantern is served at the top of its address. Take {0} out of the indexer addresses in each app, or have your reverse proxy send {0} to Lantern without it.",
                src.url_base
            ),
        ));
    }
    let mut chosen = false;
    for p in &src.proxies {
        if p.implementation.eq_ignore_ascii_case("FlareSolverr") {
            let tagged: Vec<&str> =
                src.indexers.iter().filter(|i| i.tags.iter().any(|t| p.tags.contains(t))).map(|i| i.name.as_str()).collect();
            let mut detail = "Lantern gets past Cloudflare checks by itself, for every site that needs it.".to_string();
            if !tagged.is_empty() {
                detail.push_str(&format!(" That covers {}.", super::join_words(&tagged.iter().map(|s| s.to_string()).collect::<Vec<_>>())));
            }
            plan.settings.push(Step::new(&p.name, "FlareSolverr", Action::NothingNeeded, detail));
            continue;
        }
        let what = format!("Proxy ({})", p.implementation);
        if !p.everywhere {
            let detail = if p.tags.is_empty() {
                "Prowlarr didn't use it: it had no tags.".to_string()
            } else {
                format!(
                    "Prowlarr used it only for indexers tagged {}. Lantern's proxy is for every site, so it wasn't set. Set it under Settings → Indexers if every site should use it.",
                    p.tags.join(", ")
                )
            };
            plan.settings.push(Step::new(&p.name, what, Action::Skip, detail));
            continue;
        }
        let Some(url) = p.url() else {
            plan.settings.push(Step::new(
                &p.name,
                what,
                Action::Skip,
                format!("Lantern's proxy for sites takes HTTP or SOCKS5; this one is {}.", p.implementation),
            ));
            continue;
        };
        let current = st.settings.str("site_proxy");
        let step = if chosen {
            Step::new(&p.name, what, Action::Skip, "Lantern uses one proxy for every site, and another one was taken.")
        } else if current == url {
            Step::new(&p.name, what, Action::AlreadyThere, "Lantern already uses it for sites.")
        } else if !current.is_empty() {
            Step::new(&p.name, what, Action::Skip, "Lantern already has a proxy for sites; it was left as it is.")
        } else if st.settings.env_locked("site_proxy") {
            Step::new(
                &p.name,
                what,
                Action::Manual,
                "Lantern's proxy is set by APP_SITE_PROXY in its environment. Put this proxy there if sites should use it.",
            )
        } else {
            chosen = true;
            set.insert("site_proxy".into(), json!(url));
            let mut s = Step::new(&p.name, what, Action::Set, "Lantern sends every site's requests through it, as Prowlarr did.");
            if !p.password.is_empty() {
                s.secrets.push("password".into());
            }
            s
        };
        plan.settings.push(step);
    }
    if !opts.dry_run && !set.is_empty() {
        st.settings.set_many(&Value::Object(set)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn select() -> Setting {
        let mut options = IndexMap::new();
        options.insert("time".to_string(), "Created".to_string());
        options.insert("size".to_string(), "Size".to_string());
        options.insert("seeders".to_string(), "Seeders".to_string());
        Setting { name: "sort".into(), kind: "select".into(), options, ..Default::default() }
    }

    #[test]
    fn selects_are_positions_among_sorted_options() {
        // sorted: seeders, size, time
        assert_eq!(setting_text(&select(), &json!(0)).as_deref(), Some("seeders"));
        assert_eq!(setting_text(&select(), &json!(2)).as_deref(), Some("time"));
        assert_eq!(setting_text(&select(), &json!("1")).as_deref(), Some("size"));
        assert_eq!(setting_text(&select(), &json!("size")).as_deref(), Some("size"));
        assert_eq!(setting_text(&select(), &json!(9)), None);
        let cb = Setting { name: "freeleech".into(), kind: "checkbox".into(), ..Default::default() };
        assert_eq!(setting_text(&cb, &json!(true)).as_deref(), Some("true"));
        assert_eq!(setting_text(&cb, &json!("False")).as_deref(), Some("false"));
    }

    #[test]
    fn settings_are_flattened_and_config_xml_is_read() {
        let s = indexer_from(
            3,
            " Site ",
            "Cardigann",
            true,
            25,
            vec![],
            json!({"definitionFile": "site", "baseUrl": "https://site.test/", "extraFieldData": {"username": "me"}, "baseSettings": {"queryLimit": 5}}),
        );
        assert_eq!((s.name.as_str(), s.definition.as_str(), s.base_url.as_str()), ("Site", "site", "https://site.test/"));
        assert_eq!(s.fields.get("username"), Some(&json!("me")));
        assert!(!s.fields.contains_key("baseSettings"));
        let c = config_xml("<Config><Port>9797</Port><ApiKey>abc123</ApiKey><UrlBase>prowlarr</UrlBase></Config>");
        assert_eq!(c, ConfigXml { api_key: Some("abc123".into()), port: Some(9797), url_base: "/prowlarr".into() });
        assert_eq!(kind_of(b"PK\x03\x04rest"), Some(Upload::Zip));
        assert_eq!(kind_of(b"SQLite format 3\0"), Some(Upload::Database));
        assert_eq!(kind_of(b"hello"), None);
    }

    #[test]
    fn usenet_indexers_keep_their_key_or_ask_for_it() {
        let lib = Library::new(None, &std::env::temp_dir().join("lantern-migrate-unit"));
        let mut s = indexer_from(
            1,
            "NZBgeek",
            "Newznab",
            true,
            25,
            vec![],
            json!({"baseUrl": "https://api.nzbgeek.info/", "apiPath": "/api", "apiKey": "k"}),
        );
        let m = map_indexer(&lib, &s).unwrap();
        assert_eq!((m.kind.as_str(), m.definition.as_str(), m.base_url.as_str()), ("newznab", "nzbgeek", "https://api.nzbgeek.info"));
        assert_eq!(m.settings.get("apikey").map(String::as_str), Some("k"));
        assert_eq!(m.secrets, vec!["apikey".to_string()]);
        s.fields.insert("apiKey".into(), json!("********"));
        let m = map_indexer(&lib, &s).unwrap();
        assert_eq!(m.missing, vec!["API key".to_string()]);
        assert!(!m.settings.contains_key("apikey"));
        let unknown = indexer_from(2, "X", "SomeBuiltIn", true, 25, vec![], json!({}));
        assert!(map_indexer(&lib, &unknown).unwrap_err().contains("SomeBuiltIn built in"));
    }
}
