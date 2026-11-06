// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/DownloadClient/DownloadClientController.cs,
// DownloadClientResource.cs, DownloadClientBulkResource.cs, src/NzbDrone.Core/Download/DownloadClientDefinition.cs,
// DownloadClientCategory.cs, DownloadClientFactory.cs, DownloadClientProvider.cs, DownloadClientBase.cs,
// DownloadClientStatusService.cs, DownloadService.cs, TorrentClientBase.cs, UsenetClientBase.cs,
// src/NzbDrone.Core/HealthCheck/Checks/DownloadClientStatusCheck.cs), GPL-3.0.

//! Prowlarr's download clients (`/api/v1/downloadclient`): the programs a grab from Lantern's search is sent
//! to, each with Prowlarr's settings, checks and test, its priority and its category mappings. They are the
//! download hub's clients too ([`hub`]): one store for both.
//!
//! A grab (`POST /api/v1/search`) goes to the client it names, else the indexer's own client, else the enabled
//! client of the release's protocol with the best priority that isn't held back after failures, taking turns
//! between clients of the same priority ([`pick`]).

pub mod clients;
pub mod hub;
pub mod settings;

use super::super::{fail, validation, ApiError};
use super::provider::{self, prop, Definition, Family, Implementation, ProviderFailure};
use super::status::{self, DOWNLOAD_CLIENT_STATUS};
use crate::indexers::release::{Protocol, Release};
use crate::AppState;
use kit_server_core::{Db, SqliteDb};
use serde_json::{json, Map, Value};
use settings as s;
use std::collections::HashMap;
use std::sync::Mutex;

/// Prowlarr's download clients.
pub struct DownloadClients;

/// One implementation's traits: its protocol, whether it maps categories (`SupportsCategories`).
struct Traits {
    /// The class name.
    implementation: &'static str,
    /// Torrent or Usenet.
    protocol: Protocol,
    /// Whether it has categories to map indexer categories to.
    categories: bool,
}

/// Every implementation's traits.
const TRAITS: [Traits; 18] = [
    Traits { implementation: "Aria2", protocol: Protocol::Torrent, categories: false },
    Traits { implementation: "Deluge", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "TorrentDownloadStation", protocol: Protocol::Torrent, categories: false },
    Traits { implementation: "UsenetDownloadStation", protocol: Protocol::Usenet, categories: false },
    Traits { implementation: "Flood", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "TorrentFreeboxDownload", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "Hadouken", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "Nzbget", protocol: Protocol::Usenet, categories: true },
    Traits { implementation: "NzbVortex", protocol: Protocol::Usenet, categories: true },
    Traits { implementation: "Pneumatic", protocol: Protocol::Usenet, categories: false },
    Traits { implementation: "QBittorrent", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "RTorrent", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "Sabnzbd", protocol: Protocol::Usenet, categories: true },
    Traits { implementation: "TorrentBlackhole", protocol: Protocol::Torrent, categories: false },
    Traits { implementation: "Transmission", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "UsenetBlackhole", protocol: Protocol::Usenet, categories: false },
    Traits { implementation: "UTorrent", protocol: Protocol::Torrent, categories: true },
    Traits { implementation: "Vuze", protocol: Protocol::Torrent, categories: false },
];

/// An implementation with its settings.
const fn imp(
    implementation: &'static str,
    implementation_name: &'static str,
    config_contract: &'static str,
    fields: &'static [provider::FieldSpec],
    validate: fn(&Map<String, Value>) -> Vec<ProviderFailure>,
) -> Implementation {
    Implementation { implementation, implementation_name, config_contract, fields, validate }
}

/// Every download client Prowlarr has.
static IMPLEMENTATIONS: [Implementation; 18] = [
    imp("Aria2", "Aria2", "Aria2Settings", s::ARIA2_SETTINGS, s::aria2),
    imp("Deluge", "Deluge", "DelugeSettings", s::DELUGE_SETTINGS, s::deluge),
    imp("TorrentDownloadStation", "Download Station", "DownloadStationSettings", s::DOWNLOAD_STATION_SETTINGS, s::download_station),
    imp("UsenetDownloadStation", "Download Station", "DownloadStationSettings", s::DOWNLOAD_STATION_SETTINGS, s::download_station),
    imp("Flood", "Flood", "FloodSettings", s::FLOOD_SETTINGS, s::host_and_port),
    imp("TorrentFreeboxDownload", "Freebox Download", "FreeboxDownloadSettings", s::FREEBOX_DOWNLOAD_SETTINGS, s::freebox),
    imp("Hadouken", "Hadouken", "HadoukenSettings", s::HADOUKEN_SETTINGS, s::hadouken),
    imp("Nzbget", "NZBGet", "NzbgetSettings", s::NZBGET_SETTINGS, s::nzbget),
    imp("NzbVortex", "NZBVortex", "NzbVortexSettings", s::NZB_VORTEX_SETTINGS, s::nzbvortex),
    imp("Pneumatic", "Pneumatic", "PneumaticSettings", s::PNEUMATIC_SETTINGS, s::pneumatic),
    imp("QBittorrent", "qBittorrent", "QBittorrentSettings", s::Q_BITTORRENT_SETTINGS, s::qbittorrent),
    imp("RTorrent", "rTorrent", "RTorrentSettings", s::R_TORRENT_SETTINGS, s::rtorrent),
    imp("Sabnzbd", "SABnzbd", "SabnzbdSettings", s::SABNZBD_SETTINGS, s::sabnzbd),
    imp("TorrentBlackhole", "Torrent Blackhole", "TorrentBlackholeSettings", s::TORRENT_BLACKHOLE_SETTINGS, s::torrent_blackhole),
    imp("Transmission", "Transmission", "TransmissionSettings", s::TRANSMISSION_SETTINGS, s::transmission),
    imp("UsenetBlackhole", "Usenet Blackhole", "UsenetBlackholeSettings", s::USENET_BLACKHOLE_SETTINGS, s::usenet_blackhole),
    imp("UTorrent", "uTorrent", "UTorrentSettings", s::U_TORRENT_SETTINGS, s::utorrent),
    imp("Vuze", "Vuze", "TransmissionSettings", s::TRANSMISSION_SETTINGS, s::transmission),
];

/// The traits of an implementation.
fn traits(implementation: &str) -> Option<&'static Traits> {
    TRAITS.iter().find(|t| t.implementation.eq_ignore_ascii_case(implementation))
}

/// The protocol a client downloads.
pub fn protocol_of(def: &Definition) -> Protocol {
    traits(&def.implementation).map(|t| t.protocol).unwrap_or(Protocol::Torrent)
}

/// Whether a client is on (`Enable`).
pub fn enable(def: &Definition) -> bool {
    def.extra.get("enable").and_then(Value::as_bool).unwrap_or(false)
}

/// A client's priority (1 is used first).
pub fn priority(def: &Definition) -> i64 {
    def.extra.get("priority").and_then(Value::as_i64).unwrap_or(1)
}

/// A client's category mappings: (client category, indexer categories).
pub fn categories(def: &Definition) -> Vec<(String, Vec<i64>)> {
    def.extra
        .get("categories")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .map(|c| {
                    let name = c.get("clientCategory").and_then(Value::as_str).unwrap_or("").to_string();
                    let cats = c
                        .get("categories")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_i64).collect())
                        .unwrap_or_default();
                    (name, cats)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The protocol's name in JSON (`torrent`, `usenet`).
fn protocol_name(p: Protocol) -> &'static str {
    if p == Protocol::Usenet {
        "usenet"
    } else {
        "torrent"
    }
}

/// The category mappings of a request body, as `DownloadClientCategory` reads them.
fn categories_from(v: Option<&Value>) -> Result<Value, ApiError> {
    let mut out = Vec::new();
    match v {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for c in list {
                let Value::Object(m) = c else { return Err(fail(400, "Each category mapping must be an object.")) };
                let name = prop(m, "clientCategory").and_then(Value::as_str).map(str::to_string);
                let cats: Vec<i64> = prop(m, "categories")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_i64).collect())
                    .unwrap_or_default();
                out.push(json!({ "clientCategory": name, "categories": cats }));
            }
        }
        Some(_) => return Err(fail(400, "The categories must be a list of category mappings.")),
    }
    Ok(Value::Array(out))
}

impl Family for DownloadClients {
    const RESOURCE: &'static str = "downloadclient";
    const TABLE: &'static str = "prowlarr_download_clients";
    const MODEL: &'static str = "DownloadClientDefinition";

    fn implementations() -> &'static [Implementation] {
        &IMPLEMENTATIONS
    }

    fn enabled(def: &Definition) -> bool {
        enable(def)
    }

    fn extra_from_body(body: &Map<String, Value>) -> Result<Map<String, Value>, ApiError> {
        let mut m = Map::new();
        m.insert("enable".into(), json!(prop(body, "enable").and_then(Value::as_bool).unwrap_or(false)));
        m.insert("priority".into(), json!(prop(body, "priority").and_then(Value::as_i64).unwrap_or(0)));
        m.insert("categories".into(), categories_from(prop(body, "categories"))?);
        Ok(m)
    }

    fn extra_json(def: &Definition, out: &mut Map<String, Value>) {
        out.insert("enable".into(), json!(enable(def)));
        out.insert("protocol".into(), json!(protocol_name(protocol_of(def))));
        out.insert("priority".into(), json!(priority(def)));
        out.insert("categories".into(), def.extra.get("categories").cloned().unwrap_or(json!([])));
        out.insert("supportsCategories".into(), json!(traits(&def.implementation).is_some_and(|t| t.categories)));
    }

    fn default_extra() -> Map<String, Value> {
        [("enable".to_string(), json!(false)), ("priority".to_string(), json!(1)), ("categories".to_string(), json!([]))]
            .into_iter()
            .collect()
    }

    fn bulk_extra(def: &mut Definition, body: &Map<String, Value>) -> Result<(), ApiError> {
        if let Some(e) = prop(body, "enable").and_then(Value::as_bool) {
            def.extra.insert("enable".into(), json!(e));
        }
        if let Some(p) = prop(body, "priority").and_then(Value::as_i64) {
            def.extra.insert("priority".into(), json!(p));
        }
        Ok(())
    }

    fn extra_checks(body: &Map<String, Value>) -> Vec<Option<validation::Failure>> {
        vec![validation::between("Priority", prop(body, "priority").and_then(Value::as_i64).unwrap_or(0), 1, 50)]
    }

    fn message(def: &Definition) -> Option<Value> {
        let text = match def.implementation.as_str() {
            "Flood" => "Lantern is unable to remove torrents that have finished seeding when using Flood".to_string(),
            "RTorrent" => "Lantern is unable to remove torrents that have finished seeding when using rTorrent".to_string(),
            "TorrentDownloadStation" | "UsenetDownloadStation" => {
                "Lantern is unable to connect to Download Station if 2-Factor Authentication is enabled on your DSM account".to_string()
            }
            "UTorrent" => super::super::localization::localized("en", "DownloadClientUTorrentProviderMessage", &[]).unwrap_or_default(),
            _ => return None,
        };
        Some(json!({ "message": text, "type": "warning" }))
    }

    async fn test(st: &AppState, def: &Definition) -> Vec<ProviderFailure> {
        let failures = clients::test(st, def).await;
        // a passed test steps the back-off back (`DownloadClientFactory.Test`); a failed one doesn't add to it
        if def.id != 0 && failures.iter().all(|f| f.is_warning) {
            DOWNLOAD_CLIENT_STATUS.record_success(&st.db, def.id);
        }
        failures
    }

    fn added(_st: &AppState, _def: &Definition) {}

    fn updated(_st: &AppState, _def: &Definition) {}

    fn deleted(st: &AppState, id: i64) {
        hub::forget(&st.db, id);
        let _ = DOWNLOAD_CLIENT_STATUS.delete(&st.db, id);
    }

    fn bulk_updated(_st: &AppState, _defs: &[Definition]) {}

    fn bulk_deleted(st: &AppState, ids: &[i64]) {
        for id in ids {
            Self::deleted(st, *id);
        }
    }

    async fn request_action(_st: &AppState, _def: &Definition, _name: &str, _query: &HashMap<String, String>) -> Result<Value, ApiError> {
        Ok(Value::Null)
    }
}

/// The tables of the download clients and their status, and the hub's clients moved in.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    for sql in [provider::schema(DownloadClients::TABLE), DOWNLOAD_CLIENT_STATUS.schema(), hub::SCHEMA.to_string()] {
        db.execute_batch(&sql).map_err(|e| anyhow::anyhow!("The download clients' tables can't be made: {}", e.detail))?;
    }
    hub::move_old(db).map_err(|e| anyhow::anyhow!("The download hub's clients can't be moved: {}", e.detail))?;
    Ok(())
}

/// The last client used for each protocol, by data folder (`ICached<int> _lastUsedDownloadClient`).
static LAST_USED: Mutex<Option<HashMap<(String, &'static str), i64>>> = Mutex::new(None);

/// Prowlarr's choice of a client (`DownloadClientProvider.GetDownloadClient`) among `available` (the enabled
/// clients of the protocol): the indexer's own client when it names one (an error when that one is not
/// available), else one that isn't held back (any when all are), of the best priority, taking turns by id after
/// `last_id`. Its id.
pub fn choose(available: &[(i64, i64)], blocked: &[i64], indexer_client: i64, last_id: i64) -> Result<Option<i64>, String> {
    if available.is_empty() {
        return Ok(None);
    }
    if indexer_client > 0 {
        return available
            .iter()
            .find(|(id, _)| *id == indexer_client)
            .map(|(id, _)| Some(*id))
            .ok_or_else(|| "Indexer specified download client is not available".to_string());
    }
    let mut list: Vec<(i64, i64)> = available.to_vec();
    let free: Vec<(i64, i64)> = list.iter().copied().filter(|(id, _)| !blocked.contains(id)).collect();
    if !free.is_empty() {
        list = free;
    }
    let best = list.iter().map(|(_, p)| *p).min().unwrap_or(1);
    let mut list: Vec<i64> = list.into_iter().filter(|(_, p)| *p == best).map(|(id, _)| id).collect();
    list.sort();
    Ok(Some(list.iter().copied().find(|id| *id > last_id).unwrap_or(list[0])))
}

/// The client a release of `protocol` goes to (`GetDownloadClient`), noted as the last one used.
pub fn pick(st: &AppState, protocol: Protocol, indexer_client: i64) -> Result<Option<Definition>, ApiError> {
    let all = provider::all::<DownloadClients>(&st.db)?;
    let available: Vec<&Definition> = all.iter().filter(|d| enable(d) && protocol_of(d) == protocol).collect();
    let blocked: Vec<i64> = DOWNLOAD_CLIENT_STATUS.blocked(&st.db)?.iter().map(|s| s.provider_id).collect();
    let key = (st.data_dir.display().to_string(), protocol_name(protocol));
    let mut last = LAST_USED.lock().unwrap_or_else(|e| e.into_inner());
    let last_id = last.as_ref().and_then(|m| m.get(&key).copied()).unwrap_or(0);
    let pairs: Vec<(i64, i64)> = available.iter().map(|d| (d.id, priority(d))).collect();
    let id = choose(&pairs, &blocked, indexer_client, last_id).map_err(|e| fail(500, e))?;
    if let Some(id) = id {
        if indexer_client <= 0 {
            last.get_or_insert_with(HashMap::new).insert(key, id);
        }
    }
    Ok(id.and_then(|id| all.iter().find(|d| d.id == id).cloned()))
}

/// The client category a release goes to (`GetCategoryForRelease`): the mapping that lists one of its categories,
/// else one that lists a parent of one, else `None` (the client's own category is used).
pub fn category_for(def: &Definition, release_categories: &[u32]) -> Option<String> {
    let maps = categories(def);
    let ids: Vec<i64> = release_categories.iter().map(|c| i64::from(*c)).collect();
    if let Some((name, _)) = maps.iter().find(|(_, cats)| cats.iter().any(|c| ids.contains(c))) {
        return Some(name.clone());
    }
    maps.iter()
        .find(|(_, cats)| {
            release_categories
                .iter()
                .any(|c| crate::indexers::categories::standard_parent_of(*c).is_some_and(|p| *c != p && cats.contains(&i64::from(p))))
        })
        .map(|(name, _)| name.clone())
}

/// What a client did with a grab.
pub struct Sent {
    /// The client's id for the download, when it says one.
    pub download_id: String,
}

/// Sends a release's file or magnet to `def` (`IDownloadClient.Download`), noting the client's success.
pub async fn send(
    st: &AppState,
    def: &Definition,
    release: &Release,
    grabbed: crate::indexers::cardigann::Grabbed,
) -> Result<Sent, String> {
    let category = category_for(def, &release.categories);
    let r = clients::add(st, def, release, grabbed, category).await;
    if r.is_ok() {
        DOWNLOAD_CLIENT_STATUS.record_success(&st.db, def.id);
    }
    r.map(|download_id| Sent { download_id })
}

/// `DownloadClientStatusCheck` (also `NotificationStatusCheck`): the enabled providers held back after failures,
/// all of them (an error) or some (a warning, with their names). `None` when none is.
pub fn held_back(enabled: &[(i64, String)], blocked: &[i64]) -> Option<status::Check> {
    let held: Vec<String> = enabled.iter().filter(|(id, _)| blocked.contains(id)).map(|(_, n)| n.clone()).collect();
    if held.is_empty() {
        None
    } else if held.len() == enabled.len() {
        Some(status::Check::All)
    } else {
        Some(status::Check::Some(held))
    }
}

/// `DownloadClientStatusCheck`: the enabled clients held back after failures.
pub fn health(st: &AppState, lang: &str) -> kit_server_core::Result<Vec<super::super::health::Finding>> {
    use super::super::health::Finding;
    use super::super::localization::localized;
    let enabled: Vec<(i64, String)> =
        provider::all::<DownloadClients>(&st.db)?.iter().filter(|d| enable(d)).map(|d| (d.id, d.label())).collect();
    let blocked: Vec<i64> = DOWNLOAD_CLIENT_STATUS.blocked(&st.db)?.iter().map(|s| s.provider_id).collect();
    let fragment = Some("#download-clients-are-unavailable-due-to-failures");
    let source = "DownloadClientStatusCheck";
    Ok(match held_back(&enabled, &blocked) {
        None => Vec::new(),
        Some(status::Check::All) => {
            vec![Finding::new(
                source,
                "error",
                localized(lang, "DownloadClientStatusAllClientHealthCheckMessage", &[]).unwrap_or_default(),
                fragment,
            )]
        }
        Some(status::Check::Some(names)) => {
            let msg = localized(lang, "DownloadClientStatusSingleClientHealthCheckMessage", &[("downloadClientNames", names.join(", "))]);
            vec![Finding::new(source, "warning", msg.unwrap_or_default(), fragment)]
        }
    })
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/Download/DownloadClientProviderFixture.cs), GPL-3.0.
    use super::*;

    /// Takes `n` turns among `available`, as the fixture's `Get` calls do.
    fn turns(available: &[(i64, i64)], blocked: &[i64], n: usize) -> Vec<i64> {
        let mut last = 0;
        (0..n)
            .map(|_| {
                last = choose(available, blocked, 0, last).unwrap().unwrap();
                last
            })
            .collect()
    }

    #[test]
    fn should_roundrobin_over_clients_of_one_protocol() {
        // usenet 1, 2, 3 (torrent 4 is another protocol)
        assert_eq!(turns(&[(1, 0), (2, 0), (3, 0)], &[], 5), vec![1, 2, 3, 1, 2]);
        // torrent 2, 3, 4
        assert_eq!(turns(&[(2, 0), (3, 0), (4, 0)], &[], 5), vec![2, 3, 4, 2, 3]);
    }

    #[test]
    fn should_skip_blocked_torrent_client() {
        assert_eq!(turns(&[(2, 0), (3, 0), (4, 0)], &[3], 4), vec![2, 4, 2, 4]);
    }

    #[test]
    fn should_not_skip_blocked_torrent_client_if_all_blocked() {
        assert_eq!(turns(&[(2, 0), (3, 0), (4, 0)], &[2, 3, 4], 4), vec![2, 3, 4, 2]);
    }

    #[test]
    fn should_skip_secondary_prio_torrent_client() {
        assert_eq!(turns(&[(2, 2), (3, 0), (4, 0)], &[], 4), vec![3, 4, 3, 4]);
    }

    #[test]
    fn should_not_skip_secondary_prio_torrent_client_if_primary_blocked() {
        assert_eq!(turns(&[(2, 2), (3, 2), (4, 0)], &[4], 4), vec![2, 3, 2, 3]);
    }

    #[test]
    fn indexer_client_must_be_available() {
        assert_eq!(choose(&[(2, 1)], &[], 2, 0), Ok(Some(2)));
        assert_eq!(choose(&[(2, 1)], &[], 5, 0), Err("Indexer specified download client is not available".into()));
        assert_eq!(choose(&[], &[], 5, 0), Ok(None));
    }

    #[test]
    fn categories_map_directly_then_by_parent() {
        let def = Definition {
            id: 1,
            name: None,
            implementation: "QBittorrent".into(),
            config_contract: "QBittorrentSettings".into(),
            settings: Map::new(),
            tags: vec![],
            extra: [(
                "categories".to_string(),
                json!([{ "clientCategory": "tv", "categories": [5000] }, { "clientCategory": "hd", "categories": [5040] }]),
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(category_for(&def, &[5040]).as_deref(), Some("hd"));
        assert_eq!(category_for(&def, &[5030]).as_deref(), Some("tv"));
        assert_eq!(category_for(&def, &[2000]), None);
    }
}
