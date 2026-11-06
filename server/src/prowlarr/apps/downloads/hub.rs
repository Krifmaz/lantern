//! The download hub's view of Prowlarr's download clients: one store for both. The hub's clients are the
//! Prowlarr download clients it can drive (qBittorrent, Transmission, Deluge, rTorrent, SABnzbd, NZBGet); what
//! only the hub has (its exact address, where the client sees Lantern's downloads folder, when it was added) is
//! kept beside them. Clients the hub used to keep in `download_clients` are moved here once, with their ids.

use super::super::provider::{self, Definition, Implementation};
use super::DownloadClients;
use crate::hub::clients::{self, ClientConfig};
use kit_server_core::{err, p, Db, Result, RowExt, SqliteDb};
use serde_json::{json, Map, Value};

/// What only the hub keeps about a client, and the old clients already moved.
pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS prowlarr_download_client_hub (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL DEFAULT '',
        derived TEXT NOT NULL DEFAULT '',
        root TEXT NOT NULL DEFAULT '',
        added REAL NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS prowlarr_download_clients_moved (old_id INTEGER PRIMARY KEY);";

/// The hub's kinds and the implementations they are.
pub const KINDS: [(&str, &str); 6] = [
    ("qbittorrent", "QBittorrent"),
    ("transmission", "Transmission"),
    ("deluge", "Deluge"),
    ("rtorrent", "RTorrent"),
    ("sabnzbd", "Sabnzbd"),
    ("nzbget", "Nzbget"),
];

/// The hub's kind of an implementation (Vuze talks Transmission's RPC), `None` for one the hub can't drive.
pub fn hub_kind(implementation: &str) -> Option<&'static str> {
    if implementation.eq_ignore_ascii_case("Vuze") {
        return Some("transmission");
    }
    KINDS.iter().find(|(_, i)| i.eq_ignore_ascii_case(implementation)).map(|(k, _)| *k)
}

/// The address of a client from its settings, as Prowlarr builds it (`HttpRequestBuilder.BuildBaseUrl`) with the
/// path each client's API lives at.
pub fn url_of(def: &Definition) -> String {
    let scheme = if def.flag("useSsl") { "https" } else { "http" };
    let port = def.setting("port").as_i64().unwrap_or(0);
    let base = format!("{scheme}://{}:{port}", def.text("host").trim());
    let url_base = def.text("urlBase").trim().trim_matches('/').to_string();
    let with = |suffix: &str| {
        let mut u = base.clone();
        if !url_base.is_empty() {
            u.push('/');
            u.push_str(&url_base);
        }
        u.push_str(suffix);
        u
    };
    match def.implementation.to_lowercase().as_str() {
        "transmission" | "vuze" => with("/rpc"),
        "deluge" => with("/json"),
        "nzbget" => with("/jsonrpc"),
        _ => with(""),
    }
}

/// Settings from a hub address: host, port, SSL and URL base (the path, less the API's own end).
fn settings_from_url(imp: &Implementation, url: &str, settings: &mut Map<String, Value>) {
    let Ok(u) = url::Url::parse(url.trim()) else { return };
    let ssl = u.scheme() == "https";
    settings.insert("useSsl".into(), json!(ssl));
    if let Some(h) = u.host_str() {
        settings.insert("host".into(), json!(h.trim_matches(['[', ']'])));
    }
    // an address without a port is reached on its scheme's own
    let port = u.port_or_known_default().map(i64::from).unwrap_or(if ssl { 443 } else { 80 });
    settings.insert("port".into(), json!(port));
    let mut path = u.path().trim_end_matches('/').to_string();
    for end in ["/rpc", "/json", "/jsonrpc"] {
        if matches!(imp.implementation, "Transmission" | "Deluge" | "Nzbget") && path.ends_with(end) {
            path.truncate(path.len() - end.len());
        }
    }
    let url_base = match imp.implementation {
        "Transmission" if path.is_empty() => "/transmission/".to_string(),
        "RTorrent" if path.is_empty() => "RPC2".to_string(),
        "RTorrent" => path.trim_start_matches('/').to_string(),
        _ => path,
    };
    settings.insert("urlBase".into(), if url_base.is_empty() { Value::Null } else { json!(url_base) });
}

/// The hub's own row of a client: (url, derived, root, added).
fn extras(db: &SqliteDb, id: i64) -> (String, String, String, f64) {
    db.one("SELECT * FROM prowlarr_download_client_hub WHERE id=?", p![id])
        .ok()
        .flatten()
        .map(|r| (r.s("url"), r.s("derived"), r.s("root"), r.f("added")))
        .unwrap_or_default()
}

/// Keeps the hub's own row of a client.
fn keep_extras(db: &SqliteDb, id: i64, url: &str, derived: &str, root: &str, added: f64) -> Result<()> {
    db.execute(
        "INSERT INTO prowlarr_download_client_hub (id, url, derived, root, added) VALUES (?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET url=excluded.url, derived=excluded.derived, root=excluded.root, added=excluded.added",
        p![id, url.to_string(), derived.to_string(), root.to_string(), added],
    )?;
    Ok(())
}

/// A Prowlarr download client as the hub sees it, `None` for one the hub can't drive.
pub fn config_of(db: &SqliteDb, def: &Definition) -> Option<ClientConfig> {
    let kind = hub_kind(&def.implementation)?;
    let (url, derived, root, added) = extras(db, def.id);
    let now = url_of(def);
    // the hub's own address stands while the settings still make the address it was saved with
    let url = if !url.is_empty() && derived == now { url } else { now };
    Some(ClientConfig {
        id: def.id,
        kind: kind.into(),
        name: def.label(),
        url,
        username: def.text("username"),
        password: def.text("password"),
        api_key: def.text("apiKey"),
        root,
        enabled: super::enable(def),
        priority: super::priority(def),
        added,
    })
}

/// Every client the hub can drive, by priority then name.
pub fn list(db: &SqliteDb) -> Result<Vec<ClientConfig>> {
    let mut out: Vec<ClientConfig> = provider::all::<DownloadClients>(db)?
        .iter()
        .filter(|d| !d.implementation.eq_ignore_ascii_case("Vuze"))
        .filter_map(|d| config_of(db, d))
        .collect();
    out.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// Client `id` as the hub sees it; 404 when there is none (or it is one the hub can't drive).
pub fn get(db: &SqliteDb, id: i64) -> Result<ClientConfig> {
    provider::find::<DownloadClients>(db, id)?
        .filter(|d| !d.implementation.eq_ignore_ascii_case("Vuze"))
        .and_then(|d| config_of(db, &d))
        .ok_or_else(|| err(404, "There is no download client with that id"))
}

/// The hub's fields of a client applied to a Prowlarr download client.
pub struct HubFields {
    /// The hub's kind.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// Its address.
    pub url: String,
    /// The user name.
    pub username: String,
    /// The password.
    pub password: String,
    /// The API key.
    pub api_key: String,
    /// Where it sees Lantern's downloads folder.
    pub root: String,
    /// On or off.
    pub enabled: bool,
    /// 1 to 50.
    pub priority: i64,
    /// When it was added.
    pub added: f64,
}

/// Saves a hub client as a Prowlarr download client (`id` 0 for a new one, else that one); its id.
pub fn save(db: &SqliteDb, id: i64, f: &HubFields) -> Result<i64> {
    let implementation = KINDS
        .iter()
        .find(|(k, _)| *k == f.kind)
        .map(|(_, i)| *i)
        .ok_or_else(|| err(400, "Lantern doesn't know that kind of download client"))?;
    let imp = provider::implementation_of::<DownloadClients>(implementation)
        .ok_or_else(|| err(500, "Lantern lost a download client's settings"))?;
    let existing = if id > 0 { provider::find::<DownloadClients>(db, id)? } else { None };
    let mut def = existing.clone().unwrap_or_else(|| Definition {
        id: 0,
        name: None,
        implementation: imp.implementation.into(),
        config_contract: imp.config_contract.into(),
        settings: imp.defaults(),
        tags: Vec::new(),
        extra: Map::new(),
    });
    settings_from_url(imp, &f.url, &mut def.settings);
    let text = |v: &str| if v.is_empty() { Value::Null } else { json!(v) };
    def.settings.insert("username".into(), text(&f.username));
    def.settings.insert("password".into(), text(&f.password));
    if imp.fields.iter().any(|s| s.name == "apiKey") {
        def.settings.insert("apiKey".into(), text(&f.api_key));
    }
    def.name = Some(f.name.clone());
    def.extra.insert("enable".into(), json!(f.enabled));
    def.extra.insert("priority".into(), json!(f.priority));
    def.extra.entry("categories").or_insert(json!([]));
    if existing.is_some() {
        provider::update::<DownloadClients>(db, &def)?;
    } else {
        def.id = provider::insert::<DownloadClients>(db, &def)?;
    }
    keep_extras(db, def.id, &clients::trim_url(&f.url), &url_of(&def), &f.root, f.added)?;
    Ok(def.id)
}

/// Removes client `id`, and what the hub kept about it.
pub fn remove(db: &SqliteDb, id: i64) -> Result<()> {
    db.execute("DELETE FROM prowlarr_download_clients WHERE id=?", p![id])?;
    forget(db, id);
    Ok(())
}

/// Drops what the hub kept about client `id` (after it was deleted).
pub fn forget(db: &SqliteDb, id: i64) {
    let _ = db.execute("DELETE FROM prowlarr_download_client_hub WHERE id=?", p![id]);
}

/// Moves the clients the hub kept in its old table (`download_clients`) into Prowlarr's, with their ids, once each.
/// The old rows stay where they are.
pub fn move_old(db: &SqliteDb) -> Result<()> {
    let have = db.one("SELECT name FROM sqlite_master WHERE type='table' AND name='download_clients'", p![])?.is_some();
    if !have {
        return Ok(());
    }
    let rows =
        db.query("SELECT * FROM download_clients WHERE id NOT IN (SELECT old_id FROM prowlarr_download_clients_moved) ORDER BY id", p![])?;
    for r in rows {
        let old = ClientConfig::from_row(&r);
        db.execute("INSERT OR IGNORE INTO prowlarr_download_clients_moved (old_id) VALUES (?)", p![old.id])?;
        let taken = provider::find::<DownloadClients>(db, old.id)?.is_some();
        // the same id when it is free, so grabs that name the client still find it
        if !taken {
            db.execute(
                "INSERT INTO prowlarr_download_clients (id, name, implementation, config_contract) VALUES (?, ?, 'QBittorrent', 'QBittorrentSettings')",
                p![old.id, old.name.clone()],
            )?;
        }
        let fields = HubFields {
            kind: old.kind.clone(),
            name: old.name.clone(),
            url: old.url.clone(),
            username: old.username.clone(),
            password: old.password.clone(),
            api_key: old.api_key.clone(),
            root: old.root.clone(),
            enabled: old.enabled,
            priority: old.priority,
            added: old.added,
        };
        if !taken {
            // the placeholder row takes the client's own settings
            let imp = KINDS.iter().find(|(k, _)| *k == old.kind).map(|(_, i)| *i).unwrap_or("QBittorrent");
            let contract = provider::implementation_of::<DownloadClients>(imp).map(|i| i.config_contract).unwrap_or("QBittorrentSettings");
            let defaults =
                provider::implementation_of::<DownloadClients>(imp).map(|i| Value::Object(i.defaults()).to_string()).unwrap_or_default();
            db.execute(
                "UPDATE prowlarr_download_clients SET implementation=?, config_contract=?, settings=? WHERE id=?",
                p![imp.to_string(), contract.to_string(), defaults, old.id],
            )?;
            save(db, old.id, &fields)?;
        } else {
            save(db, 0, &fields)?;
        }
    }
    Ok(())
}
