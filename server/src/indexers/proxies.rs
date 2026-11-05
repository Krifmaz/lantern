// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/IndexerProxies/*, src/NzbDrone.Core/Indexers/IndexerHttpClient.cs,
// src/NzbDrone.Core/Http/CloudFlare/CloudFlareDetectionService.cs, src/NzbDrone.Common/Http/Proxy/*,
// src/NzbDrone.Core/ThingiProvider/ProviderFactory.cs, src/Prowlarr.Http/ClientSchema/SchemaBuilder.cs), GPL-3.0.

//! Indexer proxies (Prowlarr's `IndexerProxies`): an HTTP, SOCKS4 or SOCKS5 proxy, or a FlareSolverr, that the
//! indexers sharing one of its tags go through.
//!
//! - A proxy without tags is turned off (`IndexerProxyDefinition.Enable`), and one whose settings don't pass their
//!   checks is never used (`IndexerProxyFactory.Active`).
//! - An indexer without tags uses none. Otherwise it uses the first HTTP/SOCKS proxy and the first FlareSolverr
//!   that share one of its tags, the HTTP/SOCKS one first (`IndexerHttpClient.GetProxies`). An indexer that isn't
//!   saved yet (id 0) uses the first FlareSolverr when no proxy shares a tag with it.
//! - An HTTP/SOCKS proxy takes the place of the general proxy (Lantern's `site_proxy`) for that indexer.
//! - A FlareSolverr is asked only when a site answers with a Cloudflare or DDoS-Guard check
//!   (`CloudFlareDetectionService`): Lantern sends the request to its `/v1`, keeps the browser's user agent for the
//!   site and asks again with the cookies it got (`FlareSolverr.PostResponse`). When its Host is Lantern's own
//!   FlareSolverr port, Lantern's solver answers it directly, without going through the network.
//!
//! Proxies are kept in `indexer_proxies`; their settings (with the password) are sealed by the vault like the
//! indexers'.

use super::Indexers;
use crate::prowlarr::validation::{self, Failure};
use kit_server_core::{err, p, Db, Result, Row, RowExt, SqliteDb};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Where Prowlarr checks that a proxy works (`ProwlarrCloudRequestBuilder.Services` + `/ping`).
pub const PING_URL: &str = "https://prowlarr.servarr.com/v1/ping";

/// What a saved password is sent back as (`SchemaBuilder.PRIVATE_VALUE`).
pub const PRIVATE_VALUE: &str = "********";

/// The associated text a proxy's settings are sealed with.
const SETTINGS_AAD: &str = "lantern-indexer-proxy-settings";

/// The table of the proxies.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS indexer_proxies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            implementation TEXT NOT NULL,
            settings TEXT NOT NULL DEFAULT '{}',
            tags TEXT NOT NULL DEFAULT '[]'
        );",
    )
    .map_err(|e| anyhow::anyhow!("The indexer proxy table can't be made: {}", e.detail))
}

/// The kinds of proxy, in the order Prowlarr lists them (by name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A FlareSolverr, asked when a site shows a Cloudflare check.
    FlareSolverr,
    /// An HTTP(S) proxy.
    Http,
    /// A SOCKS4 proxy.
    Socks4,
    /// A SOCKS5 proxy.
    Socks5,
}

impl Kind {
    /// Every kind, by name (`GetTemplates` orders them so).
    pub const ALL: [Kind; 4] = [Kind::FlareSolverr, Kind::Http, Kind::Socks4, Kind::Socks5];

    /// Its name (`Implementation` and `ImplementationName`).
    pub fn name(self) -> &'static str {
        match self {
            Kind::FlareSolverr => "FlareSolverr",
            Kind::Http => "Http",
            Kind::Socks4 => "Socks4",
            Kind::Socks5 => "Socks5",
        }
    }

    /// The name of its settings (`ConfigContract`).
    pub fn config_contract(self) -> &'static str {
        match self {
            Kind::FlareSolverr => "FlareSolverrSettings",
            Kind::Http => "HttpSettings",
            Kind::Socks4 => "Socks4Settings",
            Kind::Socks5 => "Socks5Settings",
        }
    }

    /// The kind called `name` (case ignored, as Prowlarr finds an implementation).
    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// A proxy's settings (`FlareSolverrSettings`, `HttpSettings`, `Socks4Settings`, `Socks5Settings` in one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The proxy's host name or address; for FlareSolverr its address (`http://localhost:8191/`).
    pub host: Option<String>,
    /// The proxy's port (HTTP and SOCKS).
    pub port: i64,
    /// The user name it wants (HTTP and SOCKS).
    pub username: String,
    /// The password it wants (HTTP and SOCKS).
    pub password: String,
    /// Seconds FlareSolverr may take (its `maxTimeout`, FlareSolverr only).
    pub request_timeout: i64,
}

impl Settings {
    /// The settings a new proxy of `kind` starts with.
    pub fn default_for(kind: Kind) -> Settings {
        match kind {
            Kind::FlareSolverr => {
                Settings { host: Some("http://localhost:8191/".into()), port: 0, username: String::new(), password: String::new(), request_timeout: 60 }
            }
            _ => Settings { host: None, port: 0, username: String::new(), password: String::new(), request_timeout: 0 },
        }
    }

    /// What is wrong with them for `kind` (`FlareSolverrSettingsValidator`, `HttpSettingsValidator`, …).
    pub fn validate(&self, kind: Kind) -> Vec<Failure> {
        let mut out = Vec::new();
        out.extend(validation::not_empty("Host", self.host.as_deref()));
        match kind {
            Kind::FlareSolverr => out.extend(validation::between("RequestTimeout", self.request_timeout, 1, 180)),
            _ => {
                if self.port == 0 {
                    out.push(Failure::new("Port", json!(0), "NotEmptyValidator", "'Port' must not be empty."));
                }
            }
        }
        out
    }

    /// The settings as they are kept.
    fn stored(&self) -> Value {
        json!({ "host": self.host, "port": self.port, "username": self.username, "password": self.password, "requestTimeout": self.request_timeout })
    }

    /// The settings kept as `v`, for `kind`.
    fn from_stored(kind: Kind, v: &Value) -> Settings {
        let d = Settings::default_for(kind);
        Settings {
            host: match v.get("host") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Null) => None,
                _ => d.host,
            },
            port: v.get("port").and_then(Value::as_i64).unwrap_or(d.port),
            username: v.get("username").and_then(Value::as_str).unwrap_or("").to_string(),
            password: v.get("password").and_then(Value::as_str).unwrap_or("").to_string(),
            request_timeout: v.get("requestTimeout").and_then(Value::as_i64).unwrap_or(d.request_timeout),
        }
    }

    /// The settings as Prowlarr's `fields` (`SchemaBuilder.ToSchema`): a set password is sent as `********`.
    pub fn fields(&self, kind: Kind) -> Value {
        let mut list = vec![field(0, "host", "Host", self.host.as_ref().map(|h| json!(h)), "textbox", "normal", false)];
        match kind {
            Kind::FlareSolverr => {
                let mut t = field(1, "requestTimeout", "Request Timeout", Some(json!(self.request_timeout)), "textbox", "normal", true);
                t["unit"] = json!("seconds");
                t["helpText"] = json!("FlareSolverr maxTimeout Request Parameter");
                list.push(t);
            }
            _ => {
                let password = if self.password.is_empty() { "" } else { PRIVATE_VALUE };
                list.push(field(1, "port", "Port", Some(json!(self.port)), "textbox", "normal", false));
                list.push(field(2, "username", "Username", Some(json!(self.username)), "textbox", "userName", false));
                list.push(field(3, "password", "Password", Some(json!(password)), "password", "password", false));
            }
        }
        Value::Array(list)
    }

    /// The settings of `kind` read from Prowlarr's `fields` (`SchemaBuilder.ReadFromSchema`): a field that is
    /// missing takes the new settings' value, a password sent as `********` keeps `existing`'s. A value of the
    /// wrong type is a 400 in words.
    pub fn read_fields(kind: Kind, fields: &Value, existing: Option<&Settings>) -> Result<Settings> {
        let d = Settings::default_for(kind);
        let mut s = Settings { host: None, port: 0, username: String::new(), password: String::new(), request_timeout: 0 };
        // fields that aren't given keep the type's own starting values (`Activator.CreateInstance`)
        s.username = d.username.clone();
        s.password = d.password.clone();
        s.host = d.host.clone();
        s.port = d.port;
        s.request_timeout = d.request_timeout;
        let list = fields.as_array().cloned().unwrap_or_default();
        let find = |name: &str| list.iter().find(|f| f.get("name").and_then(Value::as_str) == Some(name)).map(|f| f.get("value").cloned().unwrap_or(Value::Null));
        let text = |v: Value| -> Option<String> {
            match v {
                Value::Null => None,
                Value::String(s) => Some(s),
                other => Some(other.to_string()),
            }
        };
        let int = |name: &str, v: Value| -> Result<i64> {
            match v {
                Value::Number(n) => n.as_i64().ok_or_else(|| err(400, format!("{name} must be a whole number"))),
                Value::String(t) if t.trim().parse::<i64>().is_ok() => Ok(t.trim().parse().unwrap_or(0)),
                Value::Null => Ok(0),
                _ => Err(err(400, format!("{name} must be a whole number"))),
            }
        };
        if let Some(v) = find("host") {
            s.host = text(v);
        }
        match kind {
            Kind::FlareSolverr => {
                if let Some(v) = find("requestTimeout") {
                    s.request_timeout = int("Request Timeout", v)?;
                }
            }
            _ => {
                if let Some(v) = find("port") {
                    s.port = int("Port", v)?;
                }
                if let Some(v) = find("username") {
                    s.username = text(v).unwrap_or_default();
                }
                if let Some(v) = find("password") {
                    let given = text(v).unwrap_or_default();
                    s.password = match existing {
                        Some(e) if given == PRIVATE_VALUE => e.password.clone(),
                        _ => given,
                    };
                }
            }
        }
        Ok(s)
    }
}

/// One of Prowlarr's schema fields (`Field`), null parts left out.
fn field(order: i64, name: &str, label: &str, value: Option<Value>, kind: &str, privacy: &str, advanced: bool) -> Value {
    let mut f = json!({ "order": order, "name": name, "label": label, "type": kind, "advanced": advanced, "privacy": privacy, "isFloat": false });
    if let Some(v) = value {
        f["value"] = v;
    }
    f
}

/// A saved proxy (`IndexerProxyDefinition`).
#[derive(Debug, Clone, PartialEq)]
pub struct Proxy {
    /// Its id (0: not saved).
    pub id: i64,
    /// The name people see.
    pub name: String,
    /// Its kind.
    pub kind: Kind,
    /// Its settings.
    pub settings: Settings,
    /// The ids of its tags (Prowlarr's tags, `/api/v1/tag`).
    pub tags: Vec<i64>,
}

impl Proxy {
    /// Turned on: it has a tag (`IndexerProxyDefinition.Enable`).
    pub fn enabled(&self) -> bool {
        !self.tags.is_empty()
    }

    /// Its settings pass their checks.
    pub fn valid(&self) -> bool {
        self.settings.validate(self.kind).is_empty()
    }

    /// The proxy as Prowlarr's `IndexerProxyResource` (null fields left out).
    pub fn resource(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "fields": self.settings.fields(self.kind),
            "implementationName": self.kind.name(),
            "implementation": self.kind.name(),
            "configContract": self.kind.config_contract(),
            "infoLink": format!("https://wiki.servarr.com/prowlarr/supported#{}", self.kind.name().to_lowercase().replace(' ', "-")),
            "tags": self.tags,
            "onHealthIssue": false,
            "supportsOnHealthIssue": false,
            "includeHealthWarnings": false,
        })
    }

    /// A new proxy of `kind` as the schema lists it (`GetDefaultDefinitions`, with its empty `presets`).
    pub fn template(kind: Kind) -> Value {
        let mut v = Proxy { id: 0, name: String::new(), kind, settings: Settings::default_for(kind), tags: Vec::new() }.resource();
        v["presets"] = json!([]);
        v
    }

    /// The address reqwest goes through for an HTTP or SOCKS proxy (`ManagedWebProxyFactory.GetProxyUri`), with
    /// its sign-in when both the user name and the password are set; `None` for FlareSolverr. SOCKS5 lets the proxy
    /// look up host names, as .NET does; SOCKS4 looks them up here.
    pub fn url(&self) -> Option<String> {
        let scheme = match self.kind {
            Kind::Http => "http",
            Kind::Socks4 => "socks4",
            Kind::Socks5 => "socks5h",
            Kind::FlareSolverr => return None,
        };
        // `HttpProxySettings`: an empty host is 127.0.0.1
        let host = self.settings.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).unwrap_or("127.0.0.1");
        let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
        let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
        let auth = if !self.settings.username.trim().is_empty() && !self.settings.password.trim().is_empty() {
            format!("{}:{}@", enc(&self.settings.username), enc(&self.settings.password))
        } else {
            String::new()
        };
        Some(format!("{scheme}://{auth}{host}:{}", self.settings.port))
    }
}

/// The proxies an indexer goes through (`IndexerHttpClient.GetProxies`): `available` are the proxies in use (on,
/// with valid settings), `indexer_id` 0 for an indexer not saved yet.
pub fn select(indexer_id: i64, indexer_tags: &[i64], available: &[Proxy]) -> Vec<Proxy> {
    if indexer_id > 0 && indexer_tags.is_empty() {
        return Vec::new();
    }
    let sharing: Vec<&Proxy> = available.iter().filter(|p| p.tags.iter().any(|t| indexer_tags.contains(t))).collect();
    let mut out: Vec<Proxy> = Vec::new();
    // one of each group (FlareSolverr or not), the first found; the HTTP/SOCKS one goes first
    if let Some(p) = sharing.iter().find(|p| p.kind != Kind::FlareSolverr) {
        out.push((*p).clone());
    }
    if let Some(p) = sharing.iter().find(|p| p.kind == Kind::FlareSolverr) {
        out.push((*p).clone());
    }
    if out.is_empty() && indexer_id == 0 {
        out.extend(available.iter().find(|p| p.kind == Kind::FlareSolverr).cloned());
    }
    out
}

/// How one indexer's requests go out: through an HTTP/SOCKS proxy (instead of the general one), and to a
/// FlareSolverr when a site shows a Cloudflare check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The proxy address (`http://`, `socks4://`, `socks5h://`), when an HTTP/SOCKS proxy is used.
    pub proxy: Option<String>,
    /// The FlareSolverr to ask, when one is used.
    pub flaresolverr: Option<FlareTarget>,
}

/// A FlareSolverr to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlareTarget {
    /// Its address (`http://localhost:8191/`).
    pub host: String,
    /// Its `maxTimeout`, in seconds.
    pub timeout: i64,
}

impl Plan {
    /// The plan of the proxies `selected` (from [`select`]).
    pub fn of(selected: &[Proxy]) -> Plan {
        let mut plan = Plan::default();
        for p in selected {
            match p.kind {
                Kind::FlareSolverr => {
                    plan.flaresolverr = Some(FlareTarget { host: p.settings.host.clone().unwrap_or_default(), timeout: p.settings.request_timeout })
                }
                _ => plan.proxy = p.url(),
            }
        }
        plan
    }
}

// ---- the saved proxies ----

impl Indexers {
    /// The proxy of row `r`, its settings opened (`None` while the vault is locked).
    fn read_proxy(&self, r: &Row) -> Option<Proxy> {
        let kind = Kind::parse(&r.s("implementation"))?;
        let stored = r.s("settings");
        let text = match &self.vault {
            Some(v) => v.open_text(SETTINGS_AAD, &stored).ok()?,
            None => stored,
        };
        let settings = Settings::from_stored(kind, &serde_json::from_str(&text).unwrap_or(json!({})));
        let tags: Vec<i64> = serde_json::from_str(&r.s("tags")).unwrap_or_default();
        Some(Proxy { id: r.i("id"), name: r.s("name"), kind, settings, tags })
    }

    /// Every saved proxy, by name (`GetAll`); empty when the table isn't there.
    pub fn proxies(&self) -> Vec<Proxy> {
        let mut list: Vec<Proxy> = self
            .db
            .query("SELECT * FROM indexer_proxies ORDER BY id", p![])
            .map(|rows| rows.iter().filter_map(|r| self.read_proxy(r)).collect())
            .unwrap_or_default();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Proxy `id`, when there is one (`Find`). A proxy whose settings can't be opened is a 503 in words.
    pub fn find_proxy(&self, id: i64) -> Result<Option<Proxy>> {
        match self.db.one("SELECT * FROM indexer_proxies WHERE id=?", p![id])? {
            None => Ok(None),
            Some(r) => self
                .read_proxy(&r)
                .map(Some)
                .ok_or_else(|| err(503, format!("The proxy {}'s saved settings can't be opened while the vault is locked", r.s("name")))),
        }
    }

    /// The proxies in use: on (with a tag) and with settings that pass their checks (`GetAvailableProviders`).
    pub fn available_proxies(&self) -> Vec<Proxy> {
        self.proxies().into_iter().filter(|p| p.enabled() && p.valid()).collect()
    }

    /// The settings as kept: sealed by the vault (a 503 in words when it is locked), plain without one.
    fn seal_proxy(&self, s: &Settings) -> Result<String> {
        let text = s.stored().to_string();
        match &self.vault {
            Some(v) => v.seal_text(SETTINGS_AAD, &text),
            None => Ok(text),
        }
    }

    /// Saves a new proxy; its id (`Create`).
    pub fn insert_proxy(&self, p: &Proxy) -> Result<i64> {
        self.db.execute(
            "INSERT INTO indexer_proxies (name, implementation, settings, tags) VALUES (?,?,?,?)",
            p![p.name.clone(), p.kind.name(), self.seal_proxy(&p.settings)?, json!(p.tags).to_string()],
        )
    }

    /// Saves a changed proxy (`Update`; nothing happens when there is no proxy `p.id`).
    pub fn update_proxy(&self, p: &Proxy) -> Result<()> {
        self.db.execute(
            "UPDATE indexer_proxies SET name=?, implementation=?, settings=?, tags=? WHERE id=?",
            p![p.name.clone(), p.kind.name(), self.seal_proxy(&p.settings)?, json!(p.tags).to_string(), p.id],
        )?;
        self.forget_clients();
        Ok(())
    }

    /// Removes proxy `id` (`Delete`).
    pub fn delete_proxy(&self, id: i64) -> Result<()> {
        self.db.execute("DELETE FROM indexer_proxies WHERE id=?", p![id])?;
        self.forget_clients();
        Ok(())
    }

    /// The ids of the Prowlarr tags named in an indexer's `tags` text (tags it names that have no id yet get
    /// one, as `/api/v1/tag` does); empty when the tags can't be read.
    pub fn tag_ids(&self, tags: &str) -> Vec<i64> {
        let labels = crate::prowlarr::tags::labels_of(tags);
        if labels.is_empty() {
            return Vec::new();
        }
        let all = crate::prowlarr::tags::list(&self.db).unwrap_or_default();
        labels.iter().filter_map(|l| all.iter().find(|(_, label)| label == l).map(|(id, _)| *id)).collect()
    }

    /// The labels of the Prowlarr tags with `ids`, comma separated (ids without a tag are left out).
    pub fn tag_labels(&self, ids: &[i64]) -> String {
        let all = crate::prowlarr::tags::list(&self.db).unwrap_or_default();
        ids.iter().filter_map(|id| all.iter().find(|(i, _)| i == id).map(|(_, l)| l.clone())).collect::<Vec<_>>().join(",")
    }

    /// How indexer `id` (with `tags`) sends its requests.
    pub fn plan_for(&self, id: i64, tags: &str) -> Plan {
        let tag_ids = self.tag_ids(tags);
        if id > 0 && tag_ids.is_empty() {
            return Plan::default();
        }
        Plan::of(&select(id, &tag_ids, &self.available_proxies()))
    }
}

// ---- FlareSolverr ----

/// Whether an answer is a Cloudflare or DDoS-Guard check (`CloudFlareDetectionService.IsCloudflareProtected`):
/// `server` is its `Server` header, `vary` and `content_encoding` its `Vary` and `Content-Encoding`.
pub fn is_cloudflare_protected(status: u16, server: &str, vary: &str, content_encoding: &str, body: &str) -> bool {
    if !["cloudflare", "cloudflare-nginx", "ddos-guard"].contains(&server.to_lowercase().as_str()) {
        return false;
    }
    if (status == 503 || status == 403)
        && (body.contains("<title>Just a moment...</title>")
            || body.contains("<title>Access denied</title>")
            || body.contains("<title>Attention Required! | Cloudflare</title>")
            || body.trim() == "error code: 1020"
            || body.to_lowercase().contains("<title>ddos-guard</title>"))
    {
        return true;
    }
    // EbookParadijs, Film-Paleis, MuziekFabriek and Puur-Hollands
    vary == "Accept-Encoding,User-Agent" && content_encoding.trim().is_empty() && body.to_lowercase().contains("ddos")
}

/// The `proxy` FlareSolverr is told to use: the general proxy (`HttpProxySettingsProvider.GetProxySettings`,
/// Lantern's `site_proxy`) as `{url, username, password}`, with only the parts that are set.
pub fn flaresolverr_proxy(site_proxy: &str) -> Value {
    let mut o = Map::new();
    if let Ok(u) = url::Url::parse(site_proxy.trim()) {
        let scheme = match u.scheme() {
            "http" | "https" => Some("http"),
            "socks4" | "socks4a" => Some("socks4"),
            "socks5" | "socks5h" => Some("socks5"),
            _ => None,
        };
        if let (Some(scheme), Some(host)) = (scheme, u.host_str()) {
            let port = u.port_or_known_default().unwrap_or(8080);
            o.insert("url".into(), json!(format!("{scheme}://{host}:{port}")));
        }
        let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
        let user = decode(u.username());
        let pass = decode(u.password().unwrap_or(""));
        if !user.trim().is_empty() {
            o.insert("username".into(), json!(user));
        }
        if !pass.trim().is_empty() {
            o.insert("password".into(), json!(pass));
        }
    }
    Value::Object(o)
}

/// The command Prowlarr sends FlareSolverr for a request (`GenerateFlareSolverrRequest`): `post` is the POST's
/// content type and body. A POST that isn't a form is refused in Prowlarr's words.
pub fn flaresolverr_request(url: &str, post: Option<(&str, &str)>, timeout_secs: i64, site_proxy: &str) -> std::result::Result<Value, String> {
    let max_timeout = timeout_secs * 1000;
    let proxy = flaresolverr_proxy(site_proxy);
    match post {
        None => Ok(json!({ "cmd": "request.get", "url": url, "maxTimeout": max_timeout, "proxy": proxy })),
        Some((content_type, body)) => {
            let lower = content_type.to_lowercase();
            if lower.contains("application/x-www-form-urlencoded") {
                Ok(json!({
                    "cmd": "request.post", "url": url, "postData": body,
                    "headers": { "contentType": content_type }, "maxTimeout": max_timeout, "proxy": proxy,
                }))
            } else if lower.contains("multipart/form-data") || lower.contains("text/html") {
                Err(format!("Unimplemented POST Content-Type: {content_type}"))
            } else {
                Err(format!("Unsupported POST Content-Type: {content_type}"))
            }
        }
    }
}

/// The user agent FlareSolverr's browser had, per site, sent again on the site's next requests (Prowlarr's
/// `UserAgent` cache).
pub fn user_agents() -> &'static Mutex<HashMap<String, String>> {
    static UA: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    UA.get_or_init(Default::default)
}

/// The user agent kept for `host`.
pub fn user_agent_for(host: &str) -> Option<String> {
    user_agents().lock().unwrap_or_else(|e| e.into_inner()).get(host).cloned().filter(|u| !u.trim().is_empty())
}

/// Whether the FlareSolverr at `host` (an address like `http://localhost:8191/`) is Lantern's own: this machine
/// (or this container's host name) on Lantern's FlareSolverr port.
pub fn points_at_lantern(host: &str) -> bool {
    let port = crate::solver_port();
    if port == 0 {
        return false;
    }
    let Ok(u) = url::Url::parse(host.trim()) else { return false };
    let Some(name) = u.host_str() else { return false };
    let own = std::env::var("HOSTNAME").ok().filter(|h| !h.trim().is_empty());
    let here = super::rate_limit::is_loopback(name)
        || name == "0.0.0.0"
        || name == "[::]"
        || own.is_some_and(|h| h.eq_ignore_ascii_case(name));
    here && u.port_or_known_default() == Some(port)
}

/// What a FlareSolverr answered: its HTTP status and body.
#[derive(Debug, Clone)]
pub struct FlareAnswer {
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub body: String,
}

/// Sends `command` to the FlareSolverr at `target` (`POST {Host}/v1`, waiting its timeout and 5 seconds more).
/// Lantern's own solver answers when the address is Lantern's (see [`points_at_lantern`]); otherwise the request
/// goes over the network, through the general proxy unless the FlareSolverr is on this machine or the local
/// network (Prowlarr bypasses the proxy for local addresses).
pub async fn ask_flaresolverr(
    target: &FlareTarget,
    command: &Value,
    solver: Option<&crate::solver::Solver>,
    site_proxy: &str,
) -> std::result::Result<FlareAnswer, String> {
    if let (Some(s), true) = (solver, points_at_lantern(&target.host)) {
        let req = crate::solver::api::request_of(command.clone(), s.config())?;
        let (answer, failed) = crate::solver::api::controller(s, &req).await;
        return Ok(FlareAnswer { status: if failed { 500 } else { 200 }, body: answer.to_string() });
    }
    let api = format!("{}/v1", target.host.trim_end_matches('/'));
    let mut b = reqwest::Client::builder().timeout(Duration::from_secs((target.timeout.max(0) + 5) as u64));
    let local = url::Url::parse(&api).ok().and_then(|u| u.host_str().map(|h| super::rate_limit::is_loopback(h) || !h.contains('.'))).unwrap_or(true);
    if !local && !site_proxy.trim().is_empty() {
        if let Ok(px) = reqwest::Proxy::all(site_proxy.trim()) {
            b = b.proxy(px);
        }
    } else {
        b = b.no_proxy();
    }
    let client = b.build().map_err(|e| e.to_string())?;
    let resp = client
        .post(&api)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(command.to_string())
        .send()
        .await
        .map_err(|e| plain_error(&e))?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(|e| plain_error(&e))?;
    Ok(FlareAnswer { status, body })
}

/// The cookies and user agent of a FlareSolverr answer, or why there are none in Prowlarr's words
/// (`FlareSolverr.PostResponse`).
pub fn read_solution(answer: &FlareAnswer) -> std::result::Result<(Vec<(String, String)>, String), String> {
    if answer.status != 200 && answer.status != 500 {
        return Err(format!("HTTP StatusCode not 200 or 500. Status is :{}", status_name(answer.status)));
    }
    let v: Value = serde_json::from_str(&answer.body).map_err(|e| e.to_string())?;
    let solution = v.get("solution").filter(|s| s.is_object());
    let cookies: Vec<(String, String)> = solution
        .and_then(|s| s.get("cookies"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| Some((c.get("name")?.as_str()?.to_string(), c.get("value").and_then(Value::as_str).unwrap_or("").to_string())))
                .collect()
        })
        .unwrap_or_default();
    if cookies.is_empty() {
        return Err("Empty cookies returned by FlareSolverr.".into());
    }
    let ua = solution.and_then(|s| s.get("userAgent")).and_then(Value::as_str).unwrap_or("").to_string();
    Ok((cookies, ua))
}

/// .NET's name of an HTTP status (`HttpStatusCode.ToString()`), as Prowlarr writes it in its messages.
pub fn status_name(status: u16) -> String {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "NoContent",
        301 => "MovedPermanently",
        302 => "Found",
        304 => "NotModified",
        400 => "BadRequest",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        407 => "ProxyAuthenticationRequired",
        408 => "RequestTimeout",
        429 => "TooManyRequests",
        500 => "InternalServerError",
        502 => "BadGateway",
        503 => "ServiceUnavailable",
        504 => "GatewayTimeout",
        other => return other.to_string(),
    }
    .to_string()
}

/// A request error in plain words (no Rust type names).
fn plain_error(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg = format!("{msg}: {s}");
        src = s.source();
    }
    msg
}

// ---- checking a proxy ----

/// Prowlarr's `ProxyValidationBadRequest`.
fn bad_request(status: u16) -> Failure {
    Failure::new("Host", Value::Null, "", format!("Failed to test proxy. Status code: {}", status_name(status)))
}

/// Prowlarr's `ProxyValidationUnableToConnect`.
fn unable_to_connect(why: &str) -> Failure {
    Failure::new("Host", Value::Null, "", format!("Unable to connect to proxy: {why}. Check the log surrounding this error for details"))
}

/// Checks a proxy works (`HttpIndexerProxyBase.Test`, `FlareSolverr.Test`): an HTTP/SOCKS proxy must get an answer
/// other than 400 from `ping_url`; a FlareSolverr must answer 200 with JSON to a `request.get` of it. The problems
/// found, each on `Host`.
pub async fn test(proxy: &Proxy, ping_url: &str, solver: Option<&crate::solver::Solver>, site_proxy: &str) -> Vec<Failure> {
    let mut failures = Vec::new();
    match proxy.kind {
        Kind::FlareSolverr => {
            let target = FlareTarget { host: proxy.settings.host.clone().unwrap_or_default(), timeout: proxy.settings.request_timeout };
            let command = match flaresolverr_request(ping_url, None, target.timeout, site_proxy) {
                Ok(c) => c,
                Err(e) => return vec![unable_to_connect(&e)],
            };
            match ask_flaresolverr(&target, &command, solver, site_proxy).await {
                Ok(a) => {
                    if a.status != 200 {
                        failures.push(bad_request(a.status));
                    }
                    if let Err(e) = serde_json::from_str::<Value>(&a.body) {
                        failures.push(unable_to_connect(&e.to_string()));
                    }
                }
                Err(e) => failures.push(unable_to_connect(&e)),
            }
        }
        _ => {
            let client = proxy
                .url()
                .and_then(|u| reqwest::Proxy::all(&u).ok())
                .map(|px| reqwest::Client::builder().proxy(px).timeout(Duration::from_secs(100)).build());
            match client {
                Some(Ok(c)) => match c.get(ping_url).send().await {
                    Ok(r) if r.status().as_u16() == 400 => failures.push(bad_request(400)),
                    Ok(_) => {}
                    Err(e) => failures.push(unable_to_connect(&plain_error(&e))),
                },
                _ => failures.push(unable_to_connect("The proxy's address isn't valid")),
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(id: i64, kind: Kind, tags: &[i64]) -> Proxy {
        let mut settings = Settings::default_for(kind);
        if kind != Kind::FlareSolverr {
            settings.host = Some(format!("proxy{id}.test"));
            settings.port = 8080;
        }
        Proxy { id, name: format!("p{id}"), kind, settings, tags: tags.to_vec() }
    }

    #[test]
    fn indexers_take_one_proxy_and_one_flaresolverr_sharing_a_tag() {
        let all = vec![
            proxy(1, Kind::FlareSolverr, &[1]),
            proxy(2, Kind::Http, &[1]),
            proxy(3, Kind::Socks5, &[1, 2]),
            proxy(4, Kind::FlareSolverr, &[3]),
        ];
        let ids = |v: Vec<Proxy>| v.iter().map(|p| p.id).collect::<Vec<_>>();
        // an indexer without tags: none
        assert_eq!(ids(select(5, &[], &all)), Vec::<i64>::new());
        // the first of each group, HTTP/SOCKS first
        assert_eq!(ids(select(5, &[1], &all)), vec![2, 1]);
        assert_eq!(ids(select(5, &[2], &all)), vec![3]);
        assert_eq!(ids(select(5, &[9], &all)), Vec::<i64>::new());
        // an indexer not saved yet without a shared tag gets the first FlareSolverr
        assert_eq!(ids(select(0, &[], &all)), vec![1]);
        assert_eq!(ids(select(0, &[2], &all)), vec![3]);
        let plan = Plan::of(&select(5, &[1], &all));
        assert_eq!(plan.proxy.as_deref(), Some("http://proxy2.test:8080"));
        assert_eq!(plan.flaresolverr, Some(FlareTarget { host: "http://localhost:8191/".into(), timeout: 60 }));
    }

    #[test]
    fn proxy_addresses_follow_managed_web_proxy_factory() {
        let mut p = proxy(1, Kind::Socks5, &[1]);
        assert_eq!(p.url().as_deref(), Some("socks5h://proxy1.test:8080"));
        p.settings.username = "me".into();
        assert_eq!(p.url().as_deref(), Some("socks5h://proxy1.test:8080"), "a user name alone is not sent");
        p.settings.password = "p@ss".into();
        assert_eq!(p.url().as_deref(), Some("socks5h://me:p%40ss@proxy1.test:8080"));
        p.kind = Kind::Socks4;
        p.settings.host = None;
        assert_eq!(p.url().as_deref(), Some("socks4://me:p%40ss@127.0.0.1:8080"));
    }

    #[test]
    fn settings_are_checked_like_prowlarr() {
        let f = Settings::default_for(Kind::Http).validate(Kind::Http);
        let msgs: Vec<&str> = f.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(msgs, vec!["'Host' must not be empty.", "'Port' must not be empty."]);
        let mut s = Settings::default_for(Kind::FlareSolverr);
        assert!(s.validate(Kind::FlareSolverr).is_empty());
        s.request_timeout = 0;
        assert_eq!(s.validate(Kind::FlareSolverr)[0].message, "'Request Timeout' must be between 1 and 180. You entered 0.");
    }

    #[test]
    fn passwords_are_masked_and_kept() {
        let mut s = Settings::default_for(Kind::Http);
        s.password = "secret".into();
        let fields = s.fields(Kind::Http);
        assert_eq!(fields[3]["value"], PRIVATE_VALUE);
        let back = Settings::read_fields(Kind::Http, &fields, Some(&s)).unwrap();
        assert_eq!(back.password, "secret");
        let back = Settings::read_fields(Kind::Http, &json!([{ "name": "password", "value": "new" }]), Some(&s)).unwrap();
        assert_eq!((back.password.as_str(), back.port), ("new", 0));
        assert!(Settings::read_fields(Kind::Http, &json!([{ "name": "port", "value": "x" }]), None).is_err());
    }

    #[test]
    fn cloudflare_is_detected_like_prowlarr() {
        let page = "<html><head><title>Just a moment...</title></head></html>";
        assert!(is_cloudflare_protected(503, "cloudflare", "", "", page));
        assert!(is_cloudflare_protected(403, "DDoS-Guard", "", "", "<title>DDOS-GUARD</title>"));
        assert!(is_cloudflare_protected(403, "cloudflare", "", "", "error code: 1020"));
        assert!(!is_cloudflare_protected(503, "nginx", "", "", page), "only Cloudflare's and DDoS-Guard's servers");
        assert!(!is_cloudflare_protected(200, "cloudflare", "", "", page));
        assert!(is_cloudflare_protected(200, "cloudflare", "Accept-Encoding,User-Agent", "", "DDoS protection"));
        assert!(!is_cloudflare_protected(200, "cloudflare", "Accept-Encoding,User-Agent", "gzip", "DDoS protection"));
    }

    #[test]
    fn flaresolverr_commands_match_prowlarr() {
        assert_eq!(
            flaresolverr_request("https://site.test/", None, 60, "").unwrap(),
            json!({ "cmd": "request.get", "url": "https://site.test/", "maxTimeout": 60000, "proxy": {} })
        );
        assert_eq!(
            flaresolverr_request("https://site.test/login", Some(("application/x-www-form-urlencoded", "a=1")), 30, "socks5://me:pw@10.0.0.2:1080").unwrap(),
            json!({
                "cmd": "request.post", "url": "https://site.test/login", "postData": "a=1",
                "headers": { "contentType": "application/x-www-form-urlencoded" }, "maxTimeout": 30000,
                "proxy": { "url": "socks5://10.0.0.2:1080", "username": "me", "password": "pw" },
            })
        );
        assert_eq!(flaresolverr_request("u", Some(("application/json", "{}")), 60, "").unwrap_err(), "Unsupported POST Content-Type: application/json");
        assert_eq!(flaresolverr_request("u", Some(("multipart/form-data", "")), 60, "").unwrap_err(), "Unimplemented POST Content-Type: multipart/form-data");
        let empty = FlareAnswer { status: 500, body: json!({ "status": "error", "message": "Error: x" }).to_string() };
        assert_eq!(read_solution(&empty).unwrap_err(), "Empty cookies returned by FlareSolverr.");
        let odd = FlareAnswer { status: 403, body: String::new() };
        assert_eq!(read_solution(&odd).unwrap_err(), "HTTP StatusCode not 200 or 500. Status is :Forbidden");
        let ok = FlareAnswer {
            status: 200,
            body: json!({ "solution": { "cookies": [{ "name": "cf_clearance", "value": "v" }], "userAgent": "UA" } }).to_string(),
        };
        assert_eq!(read_solution(&ok).unwrap(), (vec![("cf_clearance".to_string(), "v".to_string())], "UA".to_string()));
    }

    #[test]
    fn lanterns_own_flaresolverr_is_found_by_its_port() {
        let port = crate::solver_port();
        assert!(points_at_lantern(&format!("http://localhost:{port}/")));
        assert!(points_at_lantern(&format!("http://127.0.0.1:{port}")));
        assert!(!points_at_lantern("http://flaresolverr.example:8191/") || port != 8191);
        assert!(!points_at_lantern(&format!("http://localhost:{}/", port.wrapping_add(1))));
    }
}
