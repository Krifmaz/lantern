// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Applications/Sonarr/Sonarr.cs, SonarrIndexer.cs,
// SonarrField.cs, SonarrV3Proxy.cs, src/NzbDrone.Core/Applications/Radarr/Radarr.cs, RadarrIndexer.cs,
// RadarrV3Proxy.cs, src/NzbDrone.Core/Applications/Lidarr/Lidarr.cs, LidarrIndexer.cs, LidarrV1Proxy.cs,
// src/NzbDrone.Core/Applications/Readarr/Readarr.cs, ReadarrIndexer.cs, ReadarrV1Proxy.cs,
// src/NzbDrone.Core/Applications/Whisparr/Whisparr.cs, WhisparrIndexer.cs, WhisparrV3Proxy.cs,
// src/NzbDrone.Core/Applications/ApplicationBase.cs), GPL-3.0.

//! Sonarr, Radarr, Lidarr, Readarr and Whisparr: how Lantern adds, changes and removes its indexers in them, the
//! way Prowlarr does. Each indexer becomes a Newznab or Torznab indexer in the app named `<name> (Prowlarr)`,
//! pointing at `<Prowlarr server>/<indexer id>/` with Lantern's API key, with the app's sync categories the indexer
//! has, its minimum seeders and seed goals. The five apps differ only in what [`Spec`] lists.

use super::http::{self, HttpError, Request};
use super::provider::{Definition, ProviderFailure};
use super::sync::{Ctx, RemoteMap, SyncIndexer};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// How a compared field's value is read (`Convert.ToBoolean`, `ToInt32`, `ToDouble`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Conv {
    /// A switch.
    Bool,
    /// A whole number.
    Int,
    /// A number.
    Double,
}

/// The minimum version an app must report in `X-Application-Version`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinVersion {
    /// Any version (the test only has to pass).
    None,
    /// At least this version (`major.minor.build.revision`).
    AtLeast([i64; 4]),
    /// Radarr: 3.1.1 for version 3, 4.0.4 for the others.
    Radarr,
}

/// What one app does differently.
#[derive(Debug)]
pub struct Spec {
    /// Its name (`Sonarr`).
    pub name: &'static str,
    /// Its API's path (`/api/v3`).
    pub api: &'static str,
    /// The search mode an indexer needs besides basic search (`tv-search`); `None` for Whisparr, which only
    /// looks at the categories.
    pub search_mode: Option<&'static str>,
    /// What the skip message calls that search (`TV`).
    pub search_word: &'static str,
    /// Sonarr's anime categories.
    pub anime: bool,
    /// The fields Lantern sets on the app's indexer.
    pub sync_fields: &'static [&'static str],
    /// The fields also set the first time (for older app versions).
    pub first_sync_fields: &'static [&'static str],
    /// The pack seed time field (`seedCriteria.seasonPackSeedTime`), and whether the app always has it.
    pub pack_seed: Option<(&'static str, bool)>,
    /// The extra fields compared to see whether the app's indexer is up to date.
    pub compared: &'static [(&'static str, Conv)],
    /// The settings the app's own indexer keeps (Sonarr's `seasonSearchMaximumSingleEpisodeAge`).
    pub kept: &'static [&'static str],
    /// The minimum version.
    pub min_version: MinVersion,
    /// Whether a 404 in the test says the app may be too old (Sonarr v2).
    pub not_found_hint: bool,
}

/// Sonarr.
pub static SONARR: Spec = Spec {
    name: "Sonarr",
    api: "/api/v3",
    search_mode: Some("tv-search"),
    search_word: "TV",
    anime: true,
    sync_fields: &[
        "baseUrl",
        "apiPath",
        "apiKey",
        "categories",
        "animeCategories",
        "animeStandardFormatSearch",
        "minimumSeeders",
        "seedCriteria.seedRatio",
        "seedCriteria.seedTime",
        "seedCriteria.seasonPackSeedTime",
        "rejectBlocklistedTorrentHashesWhileGrabbing",
    ],
    first_sync_fields: &["additionalParameters"],
    pack_seed: Some(("seedCriteria.seasonPackSeedTime", true)),
    compared: &[
        ("animeStandardFormatSearch", Conv::Bool),
        ("minimumSeeders", Conv::Int),
        ("seedCriteria.seedTime", Conv::Int),
        ("seedCriteria.seasonPackSeedTime", Conv::Int),
        ("seedCriteria.seedRatio", Conv::Double),
        ("rejectBlocklistedTorrentHashesWhileGrabbing", Conv::Bool),
    ],
    kept: &["seasonSearchMaximumSingleEpisodeAge"],
    min_version: MinVersion::AtLeast([3, 0, 5, 0]),
    not_found_hint: true,
};

/// Radarr.
pub static RADARR: Spec = Spec {
    name: "Radarr",
    api: "/api/v3",
    search_mode: Some("movie-search"),
    search_word: "movie",
    anime: false,
    sync_fields: &[
        "baseUrl",
        "apiPath",
        "apiKey",
        "categories",
        "minimumSeeders",
        "seedCriteria.seedRatio",
        "seedCriteria.seedTime",
        "rejectBlocklistedTorrentHashesWhileGrabbing",
    ],
    first_sync_fields: &["multiLanguages", "removeYear", "requiredFlags", "additionalParameters"],
    pack_seed: None,
    compared: &[
        ("minimumSeeders", Conv::Int),
        ("seedCriteria.seedRatio", Conv::Double),
        ("seedCriteria.seedTime", Conv::Int),
        ("rejectBlocklistedTorrentHashesWhileGrabbing", Conv::Bool),
    ],
    kept: &[],
    min_version: MinVersion::Radarr,
    not_found_hint: false,
};

/// The fields Lidarr and Readarr set.
const MUSIC_BOOK_FIELDS: &[&str] = &[
    "baseUrl",
    "apiPath",
    "apiKey",
    "categories",
    "minimumSeeders",
    "seedCriteria.seedRatio",
    "seedCriteria.seedTime",
    "seedCriteria.discographySeedTime",
    "rejectBlocklistedTorrentHashesWhileGrabbing",
];

/// The fields Lidarr and Readarr compare.
const MUSIC_BOOK_COMPARED: &[(&str, Conv)] = &[
    ("minimumSeeders", Conv::Int),
    ("seedCriteria.seedRatio", Conv::Double),
    ("seedCriteria.seedTime", Conv::Int),
    ("seedCriteria.discographySeedTime", Conv::Int),
    ("rejectBlocklistedTorrentHashesWhileGrabbing", Conv::Bool),
];

/// Lidarr.
pub static LIDARR: Spec = Spec {
    name: "Lidarr",
    api: "/api/v1",
    search_mode: Some("music-search"),
    search_word: "music",
    anime: false,
    sync_fields: MUSIC_BOOK_FIELDS,
    first_sync_fields: &["earlyReleaseLimit", "additionalParameters"],
    pack_seed: Some(("seedCriteria.discographySeedTime", false)),
    compared: MUSIC_BOOK_COMPARED,
    kept: &[],
    min_version: MinVersion::AtLeast([1, 0, 2, 0]),
    not_found_hint: false,
};

/// Readarr.
pub static READARR: Spec = Spec {
    name: "Readarr",
    api: "/api/v1",
    search_mode: Some("book-search"),
    search_word: "book",
    anime: false,
    sync_fields: MUSIC_BOOK_FIELDS,
    first_sync_fields: &[],
    pack_seed: Some(("seedCriteria.discographySeedTime", false)),
    compared: MUSIC_BOOK_COMPARED,
    kept: &[],
    min_version: MinVersion::None,
    not_found_hint: false,
};

/// Whisparr.
pub static WHISPARR: Spec = Spec {
    name: "Whisparr",
    api: "/api/v3",
    search_mode: None,
    search_word: "",
    anime: false,
    sync_fields: &[
        "baseUrl",
        "apiPath",
        "apiKey",
        "categories",
        "minimumSeeders",
        "seedCriteria.seedRatio",
        "seedCriteria.seedTime",
        "seedCriteria.seasonPackSeedTime",
        "rejectBlocklistedTorrentHashesWhileGrabbing",
    ],
    first_sync_fields: &[],
    pack_seed: Some(("seedCriteria.seasonPackSeedTime", false)),
    compared: &[
        ("minimumSeeders", Conv::Int),
        ("seedCriteria.seedRatio", Conv::Double),
        ("seedCriteria.seedTime", Conv::Int),
        ("seedCriteria.seasonPackSeedTime", Conv::Int),
        ("rejectBlocklistedTorrentHashesWhileGrabbing", Conv::Bool),
    ],
    kept: &[],
    min_version: MinVersion::None,
    not_found_hint: false,
};

/// The error .NET raises for a field the app's schema does not have.
fn missing(field: &str) -> HttpError {
    HttpError::Other(format!("Object reference not set to an instance of an object. (The app has no '{field}' field.)"))
}

/// The properties of an app's indexer that Prowlarr's indexer class has, in its order; the rest of what the app
/// sends is left out (the class does not keep it).
fn normalize(spec: &Spec, v: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    let get = |k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    out.insert("id".into(), json!(get("id").as_i64().unwrap_or(0)));
    for k in ["enableRss", "enableAutomaticSearch", "enableInteractiveSearch"] {
        out.insert(k.into(), json!(get(k).as_bool().unwrap_or(false)));
    }
    out.insert("priority".into(), json!(get("priority").as_i64().unwrap_or(0)));
    for k in ["name", "implementationName", "implementation", "configContract", "infoLink"] {
        if let Some(s) = get(k).as_str() {
            out.insert(k.into(), json!(s));
        }
    }
    if let Some(n) = get("downloadClientId").as_i64() {
        out.insert("downloadClientId".into(), json!(n));
    }
    for k in spec.kept {
        if let Some(n) = get(k).as_i64() {
            out.insert((*k).into(), json!(n));
        }
    }
    let mut tags: Vec<i64> = Vec::new();
    for t in get("tags").as_array().into_iter().flatten().filter_map(Value::as_i64) {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    out.insert("tags".into(), json!(tags));
    let fields: Vec<Value> = get("fields").as_array().into_iter().flatten().map(normalize_field).collect();
    out.insert("fields".into(), Value::Array(fields));
    out
}

/// A field as Prowlarr's `SonarrField` keeps it: name, value, type, advanced, section, hidden.
fn normalize_field(f: &Value) -> Value {
    let mut out = Map::new();
    for k in ["name", "value", "type"] {
        if let Some(v) = f.get(k).filter(|v| !v.is_null()) {
            out.insert(k.into(), v.clone());
        }
    }
    out.insert("advanced".into(), json!(f.get("advanced").and_then(Value::as_bool).unwrap_or(false)));
    for k in ["section", "hidden"] {
        if let Some(v) = f.get(k).filter(|v| !v.is_null()) {
            out.insert(k.into(), v.clone());
        }
    }
    Value::Object(out)
}

/// The fields of an indexer object.
fn fields_mut(ix: &mut Map<String, Value>) -> &mut Vec<Value> {
    if !matches!(ix.get("fields"), Some(Value::Array(_))) {
        ix.insert("fields".into(), json!([]));
    }
    match ix.get_mut("fields") {
        Some(Value::Array(a)) => a,
        _ => unreachable!("fields was just made a list"),
    }
}

/// The field `name` of an indexer object.
fn field<'a>(ix: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    ix.get("fields")?.as_array()?.iter().find(|f| f.get("name").and_then(Value::as_str) == Some(name))
}

/// The value of field `name` (`Value::Null` when it has none).
fn field_value<'a>(ix: &'a Map<String, Value>, name: &str) -> &'a Value {
    field(ix, name).and_then(|f| f.get("value")).unwrap_or(&Value::Null)
}

/// Sets field `name`'s value (a null leaves the value out, as Prowlarr writes it); an error when the indexer has
/// no such field.
fn set(ix: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), HttpError> {
    let f = fields_mut(ix).iter_mut().find(|f| f.get("name").and_then(Value::as_str) == Some(name)).ok_or_else(|| missing(name))?;
    if let Value::Object(m) = f {
        if value.is_null() {
            m.remove("value");
        } else {
            m.insert("value".into(), value);
        }
    }
    Ok(())
}

/// Sets field `name` when the indexer has it.
fn set_if_there(ix: &mut Map<String, Value>, name: &str, value: Value) {
    if field(ix, name).is_some() {
        let _ = set(ix, name, value);
    }
}

/// The app's indexer for Lantern's indexer `ix` (`BuildSonarrIndexer` and its siblings): `id` is the app's id for
/// it (0 for a new one); `schemas` is the app's indexer schema.
pub fn build(
    spec: &Spec,
    app: &Definition,
    ix: &SyncIndexer,
    schemas: &[Value],
    api_key: &str,
    id: i64,
) -> Result<Map<String, Value>, HttpError> {
    let mut sync_fields: Vec<&str> = spec.sync_fields.to_vec();
    if id == 0 {
        sync_fields.extend(spec.first_sync_fields);
    }
    let pick = |implementation: &str| {
        schemas
            .iter()
            .find(|s| s.get("implementation").and_then(Value::as_str) == Some(implementation))
            .ok_or_else(|| HttpError::Other("Sequence contains no matching element".into()))
    };
    let newznab = pick("Newznab")?;
    let torznab = pick("Torznab")?;
    let schema = normalize(spec, if ix.torrent { torznab } else { newznab });
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("enableRss".into(), json!(ix.enable && ix.profile.enable_rss));
    out.insert("enableAutomaticSearch".into(), json!(ix.enable && ix.profile.enable_automatic_search));
    out.insert("enableInteractiveSearch".into(), json!(ix.enable && ix.profile.enable_interactive_search));
    out.insert("priority".into(), json!(ix.priority));
    out.insert("name".into(), json!(format!("{} (Prowlarr)", ix.name)));
    out.insert("implementation".into(), json!(if ix.torrent { "Torznab" } else { "Newznab" }));
    if let Some(c) = schema.get("configContract") {
        out.insert("configContract".into(), c.clone());
    }
    out.insert("tags".into(), json!([]));
    let fields: Vec<Value> = schema
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|f| f.get("name").and_then(Value::as_str).is_some_and(|n| sync_fields.contains(&n)))
        .cloned()
        .collect();
    out.insert("fields".into(), Value::Array(fields));

    let prowlarr_url = app.text("prowlarrUrl");
    set(&mut out, "baseUrl", json!(format!("{}/{}/", prowlarr_url.trim_end_matches('/'), ix.id)))?;
    set(&mut out, "apiPath", json!("/api"))?;
    set(&mut out, "apiKey", json!(api_key))?;
    set(&mut out, "categories", json!(ix.categories.supported(&app.ints("syncCategories"))))?;
    if spec.anime {
        set(&mut out, "animeCategories", json!(ix.categories.supported(&app.ints("animeSyncCategories"))))?;
        set_if_there(&mut out, "animeStandardFormatSearch", json!(app.flag("syncAnimeStandardFormatSearch")));
    }
    if ix.torrent {
        set(&mut out, "minimumSeeders", json!(ix.app_minimum_seeders.unwrap_or(ix.profile.minimum_seeders)))?;
        set(&mut out, "seedCriteria.seedRatio", json!(ix.seed_ratio))?;
        set(&mut out, "seedCriteria.seedTime", json!(ix.seed_time))?;
        if let Some((name, always)) = spec.pack_seed {
            let value = json!(ix.pack_seed_time.or(ix.seed_time));
            if always {
                set(&mut out, name, value)?;
            } else {
                set_if_there(&mut out, name, value);
            }
        }
        set_if_there(
            &mut out,
            "rejectBlocklistedTorrentHashesWhileGrabbing",
            json!(app.flag("syncRejectBlocklistedTorrentHashesWhileGrabbing")),
        );
    }
    Ok(out)
}

/// A compared field's value, as .NET's `Convert` reads it (`None` for a missing field or a null).
fn converted(v: &Value, conv: Conv) -> Option<Value> {
    if v.is_null() {
        return None;
    }
    Some(match conv {
        Conv::Bool => json!(match v {
            Value::Bool(b) => *b,
            Value::String(s) => s.eq_ignore_ascii_case("true"),
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            _ => false,
        }),
        Conv::Int => json!(match v {
            Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0).round_ties_even() as i64),
            Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
            Value::Bool(b) => i64::from(*b),
            _ => 0,
        }),
        Conv::Double => json!(match v {
            Value::Number(n) => n.as_f64().unwrap_or(0.0),
            Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        }),
    })
}

/// Whether the app's indexer `other` already is what Lantern would send (`SonarrIndexer.Equals` and its
/// siblings): the switches, name, implementation, priority, id, address, API key (or the app's mask), API path,
/// categories and the app's compared fields.
pub fn same(spec: &Spec, ours: &Map<String, Value>, other: &Map<String, Value>) -> bool {
    let top = |k: &str| (ours.get(k).cloned().unwrap_or(Value::Null), other.get(k).cloned().unwrap_or(Value::Null));
    let flags =
        ["enableRss", "enableAutomaticSearch", "enableInteractiveSearch", "name", "implementation", "priority", "id"].iter().all(|k| {
            let (a, b) = top(k);
            a == b
        });
    let base_url = field_value(ours, "baseUrl").as_str() == field_value(other, "baseUrl").as_str();
    let cats = field_value(ours, "categories") == field_value(other, "categories");
    let anime = !spec.anime || field_value(ours, "animeCategories") == field_value(other, "animeCategories");
    let key = field_value(ours, "apiKey").as_str();
    let other_key = field_value(other, "apiKey").as_str();
    let key_same = key == other_key || other_key == Some(super::provider::MASK);
    let path_same = field_value(ours, "apiPath") == field_value(other, "apiPath");
    let compared =
        spec.compared.iter().all(|(name, conv)| converted(field_value(ours, name), *conv) == converted(field_value(other, name), *conv));
    flags && key_same && path_same && base_url && cats && anime && compared
}

/// An app's indexer schema and when it was fetched.
type CachedSchema = (f64, Vec<Value>);

/// The app's indexer schema, kept seven days per app address (Prowlarr's schema cache).
static SCHEMAS: Mutex<Option<HashMap<String, CachedSchema>>> = Mutex::new(None);

/// A request to the app.
fn request(app: &Definition, method: Method, resource: &str, body: Option<&Map<String, Value>>) -> Request {
    Request {
        method,
        url: http::address(&app.text("baseUrl"), resource, &[]),
        api_key: Some(app.text("apiKey")),
        auth: http::basic_auth(&app.text("authUsername"), &app.text("authPassword")),
        body: body.map(|b| Value::Object(b.clone()).to_string()),
    }
}

/// The app's indexer schema (`GetIndexerSchema`), from the cache when it is there.
async fn schemas(spec: &Spec, app: &Definition) -> Result<Vec<Value>, HttpError> {
    let key = format!("{}|{}", spec.name, app.text("baseUrl"));
    let now = crate::auth::now();
    if let Some((at, list)) = SCHEMAS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key).cloned()) {
        if now - at < 7.0 * 24.0 * 3600.0 {
            return Ok(list);
        }
    }
    let list = match http::json(&request(app, Method::GET, &format!("{}/indexer/schema", spec.api), None)).await? {
        Value::Array(a) => a,
        _ => return Err(HttpError::Json("The app's indexer schema is not a list.".into())),
    };
    SCHEMAS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, (now, list.clone()));
    Ok(list)
}

/// Forgets the cached schemas (tests start each app afresh).
pub fn forget_schemas() {
    *SCHEMAS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The app's indexers (`GetIndexers`).
async fn indexers(spec: &Spec, app: &Definition) -> Result<Vec<Map<String, Value>>, HttpError> {
    match http::json(&request(app, Method::GET, &format!("{}/indexer", spec.api), None)).await? {
        Value::Array(a) => Ok(a.iter().map(|v| normalize(spec, v)).collect()),
        _ => Err(HttpError::Json("The app's indexers are not a list.".into())),
    }
}

/// One of the app's indexers (`GetIndexer`), `None` when the app has no such indexer (404).
async fn indexer(spec: &Spec, app: &Definition, id: i64) -> Result<Option<Map<String, Value>>, HttpError> {
    match http::json(&request(app, Method::GET, &format!("{}/indexer/{id}", spec.api), None)).await {
        Ok(Value::Null) => Ok(None),
        Ok(v) => Ok(Some(normalize(spec, &v))),
        Err(e) if e.status() == Some(404) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Removes one of the app's indexers (`RemoveIndexer`).
async fn remove(spec: &Spec, app: &Definition, id: i64) -> Result<(), HttpError> {
    http::send(&request(app, Method::DELETE, &format!("{}/indexer/{id}", spec.api), None)).await.map(|_| ())
}

/// Adds (POST) or changes (PUT) an indexer in the app; a 400 is tried again with `forceSave=true`, as Prowlarr
/// does. What the app saved.
async fn save(spec: &Spec, app: &Definition, ix: &Map<String, Value>, method: Method) -> Result<Option<Map<String, Value>>, HttpError> {
    let resource = if method == Method::PUT {
        format!("{}/indexer/{}", spec.api, ix.get("id").and_then(Value::as_i64).unwrap_or(0))
    } else {
        format!("{}/indexer", spec.api)
    };
    let mut req = request(app, method, &resource, Some(ix));
    let first = http::json(&req).await;
    let answer = match first {
        Err(e) if e.status() == Some(400) => {
            tracing::debug!("{}: retrying to save the indexer forcefully", spec.name);
            req.url = http::address(&app.text("baseUrl"), &resource, &[("forceSave", "true".into())]);
            http::json(&req).await
        }
        other => other,
    };
    match answer {
        Ok(Value::Null) => Ok(None),
        Ok(v) => Ok(Some(normalize(spec, &v))),
        Err(e) => {
            if let HttpError::Status { code: 400, body, .. } = &e {
                if body
                    .to_lowercase()
                    .contains("query successful, but no results in the configured categories were returned from your indexer.")
                {
                    tracing::warn!(
                        "{}: no results in the configured categories (Prowlarr's FAQ: Prowlarr will not sync X indexer to app)",
                        spec.name
                    );
                }
            }
            Err(e)
        }
    }
}

/// Whether Lantern's indexer may be in the app: it has the app's search (or basic search) and at least one of its
/// sync categories (or, for Sonarr, anime categories).
fn fits(spec: &Spec, app: &Definition, ix: &SyncIndexer) -> bool {
    let search = spec.search_mode.is_none_or(|m| ix.has_mode(m) || ix.has_mode("search"));
    let cats = !ix.categories.supported(&app.ints("syncCategories")).is_empty()
        || (spec.anime && !ix.categories.supported(&app.ints("animeSyncCategories")).is_empty());
    search && cats
}

/// The indexers in the app that point at Lantern (`GetIndexerMappings`): Newznab and Torznab indexers whose address
/// starts with the Prowlarr server setting (or that carry Lantern's API key), by the indexer id in their address.
pub async fn mappings(spec: &Spec, ctx: &Ctx) -> Result<Vec<RemoteMap>, HttpError> {
    let prowlarr_url = ctx.app.text("prowlarrUrl");
    let prefix = prowlarr_url.trim_end_matches('/');
    let mut out = Vec::new();
    for ix in indexers(spec, &ctx.app).await? {
        let implementation = ix.get("implementation").and_then(Value::as_str).unwrap_or("");
        if implementation != "Newznab" && implementation != "Torznab" {
            continue;
        }
        let base_url = field_value(&ix, "baseUrl").as_str().unwrap_or("").to_string();
        if !base_url.starts_with(prefix) && field_value(&ix, "apiKey").as_str() != Some(ctx.api_key.as_str()) {
            continue;
        }
        if let Some(id) = super::sync::indexer_id_in(&base_url) {
            out.push(RemoteMap { indexer_id: id, remote_id: ix.get("id").and_then(Value::as_i64).unwrap_or(0), remote_name: None });
        }
    }
    Ok(out)
}

/// Adds Lantern's indexer to the app (`AddIndexer`), when it has the app's search and categories.
pub async fn add(spec: &Spec, ctx: &Ctx, ix: &SyncIndexer) -> Result<(), HttpError> {
    let app = &ctx.app;
    if let Some(mode) = spec.search_mode {
        if !ix.has_mode(mode) && !ix.has_mode("search") {
            tracing::debug!(
                "Skipping add for indexer {} [{}] due to missing {} or basic search support by the indexer",
                ix.name,
                ix.id,
                spec.search_word
            );
            return Ok(());
        }
    }
    let none = ix.categories.supported(&app.ints("syncCategories")).is_empty()
        && (!spec.anime || ix.categories.supported(&app.ints("animeSyncCategories")).is_empty());
    if none {
        tracing::debug!("Skipping add for indexer {} [{}] due to no app Sync Categories supported by the indexer", ix.name, ix.id);
        return Ok(());
    }
    tracing::trace!("Adding indexer {} [{}]", ix.name, ix.id);
    let ours = build(spec, app, ix, &schemas(spec, app).await?, &ctx.api_key, 0)?;
    let Some(remote) = save(spec, app, &ours, Method::POST).await? else {
        tracing::debug!("Failed to add {} [{}]", ix.name, ix.id);
        return Ok(());
    };
    let remote_id = remote.get("id").and_then(Value::as_i64).unwrap_or(0);
    super::sync::insert_map(&ctx.st.db, app.id, ix.id, remote_id, None).map_err(|e| HttpError::Other(e.detail))?;
    Ok(())
}

/// Removes Lantern's indexer `indexer_id` from the app (`RemoveIndexer`), when the app has it.
pub async fn remove_indexer(spec: &Spec, ctx: &Ctx, indexer_id: i64) -> Result<(), HttpError> {
    let maps = super::sync::maps_for_app(&ctx.st.db, ctx.app.id).map_err(|e| HttpError::Other(e.detail))?;
    if let Some(m) = maps.iter().find(|m| m.indexer_id == indexer_id) {
        remove(spec, &ctx.app, m.remote_id).await?;
        super::sync::delete_map(&ctx.st.db, m.id).map_err(|e| HttpError::Other(e.detail))?;
    }
    Ok(())
}

/// Brings the app's copy of Lantern's indexer up to date (`UpdateIndexer`): changed when it differs (or `force`),
/// removed when the indexer no longer fits the app, added again when the app lost it.
pub async fn update(spec: &Spec, ctx: &Ctx, ix: &SyncIndexer, force: bool) -> Result<(), HttpError> {
    let app = &ctx.app;
    let db = &ctx.st.db;
    tracing::debug!("Updating indexer {} [{}]", ix.name, ix.id);
    let maps = super::sync::maps_for_app(db, app.id).map_err(|e| HttpError::Other(e.detail))?;
    let mapping = maps.iter().find(|m| m.indexer_id == ix.id).cloned().ok_or_else(|| missing("mapping"))?;
    let schemas = schemas(spec, app).await?;
    let mut ours = build(spec, app, ix, &schemas, &ctx.api_key, mapping.remote_id)?;
    match indexer(spec, app, mapping.remote_id).await? {
        Some(remote) => {
            tracing::debug!(
                "Remote indexer {} [{}] found",
                remote.get("name").and_then(serde_json::Value::as_str).unwrap_or(""),
                mapping.remote_id
            );
            if !same(spec, &ours, &remote) || force {
                tracing::debug!("Syncing remote indexer with current settings");
                if fits(spec, app, ix) {
                    // keep the app's own fields, tags and settings
                    let names: Vec<String> =
                        fields_mut(&mut ours).iter().filter_map(|f| f.get("name").and_then(Value::as_str).map(str::to_string)).collect();
                    let theirs: Vec<Value> = remote
                        .get("fields")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|f| f.get("name").and_then(Value::as_str).is_none_or(|n| !names.iter().any(|x| x == n)))
                        .cloned()
                        .collect();
                    fields_mut(&mut ours).extend(theirs);
                    let mut tags: Vec<i64> =
                        ours.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_i64).collect();
                    for t in remote.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_i64) {
                        if !tags.contains(&t) {
                            tags.push(t);
                        }
                    }
                    ours.insert("tags".into(), json!(tags));
                    let mut keep = |k: &str| match remote.get(k) {
                        Some(v) => {
                            ours.insert(k.into(), v.clone());
                        }
                        None => {
                            ours.remove(k);
                        }
                    };
                    keep("downloadClientId");
                    for k in spec.kept {
                        keep(k);
                    }
                    save(spec, app, &ours, Method::PUT).await?;
                } else {
                    remove(spec, app, mapping.remote_id).await?;
                    super::sync::delete_map(db, mapping.id).map_err(|e| HttpError::Other(e.detail))?;
                }
            }
        }
        None => {
            super::sync::delete_map(db, mapping.id).map_err(|e| HttpError::Other(e.detail))?;
            if fits(spec, app, ix) {
                tracing::debug!("Remote indexer not found, re-adding {} [{}] to {}", ix.name, ix.id, spec.name);
                ours.insert("id".into(), json!(0));
                if let Some(new) = save(spec, app, &ours, Method::POST).await? {
                    let remote_id = new.get("id").and_then(Value::as_i64).unwrap_or(0);
                    super::sync::insert_map(db, app.id, ix.id, remote_id, None).map_err(|e| HttpError::Other(e.detail))?;
                }
            } else {
                tracing::debug!(
                    "Remote indexer not found for {} [{}], skipping re-add to {} due to indexer capabilities",
                    ix.name,
                    ix.id,
                    spec.name
                );
            }
        }
    }
    Ok(())
}

/// The indexer Prowlarr's test sends: `Test`, Usenet, every standard category, turned off.
pub fn test_indexer() -> SyncIndexer {
    SyncIndexer::test()
}

/// Sends the test indexer to the app's indexer test and checks the version it reports (`TestConnection`).
async fn test_connection(spec: &Spec, ctx: &Ctx) -> Result<Option<ProviderFailure>, HttpError> {
    let app = &ctx.app;
    let ours = build(spec, app, &test_indexer(), &schemas(spec, app).await?, &ctx.api_key, 0)?;
    let answer = http::send(&request(app, Method::POST, &format!("{}/indexer/test", spec.api), Some(&ours))).await?;
    let min = match spec.min_version {
        MinVersion::None => return Ok(None),
        MinVersion::AtLeast(v) => v,
        MinVersion::Radarr => [0; 4],
    };
    let Some(reported) = answer.version else {
        return Ok(Some(ProviderFailure::new("", format!("Failed to fetch {} version", spec.name))));
    };
    let v = http::version(&reported)
        .ok_or_else(|| HttpError::Other(format!("Version string portion was too short or too long. ({reported})")))?;
    let min = match spec.min_version {
        MinVersion::Radarr if v[0] == 3 => [3, 1, 1, 0],
        MinVersion::Radarr => [4, 0, 4, 0],
        _ => min,
    };
    if v < min {
        let shown = format!("{}.{}.{}", min[0], min[1], min[2]);
        return Ok(Some(
            ProviderFailure::new("", format!("{} version should be at least {shown}. Version reported is {reported}", spec.name))
                .with_value(json!(reported)),
        ));
    }
    Ok(None)
}

/// Tests the app (`Test`): its indexer test with a test indexer, with Prowlarr's messages for what went wrong.
pub async fn test(spec: &Spec, ctx: &Ctx) -> Vec<ProviderFailure> {
    let n = spec.name;
    match test_connection(spec, ctx).await {
        Ok(f) => f.into_iter().collect(),
        Err(e) => {
            tracing::warn!("{n}: unable to complete application test: {e}");
            let f = match (&e, e.status()) {
                (_, Some(401)) => ProviderFailure::new("ApiKey", "API Key is invalid"),
                (_, Some(400)) => ProviderFailure::new("ProwlarrUrl", format!("Prowlarr URL is invalid, {n} cannot connect to Prowlarr")),
                (_, Some(303 | 307)) => ProviderFailure::new(
                    "BaseUrl",
                    format!("{n} URL is invalid, Prowlarr cannot connect to {n} - are you missing a URL base?"),
                ),
                (_, Some(404)) if spec.not_found_hint => ProviderFailure::new(
                    "BaseUrl",
                    format!("{n} URL is invalid, Prowlarr cannot connect to {n}. Is {n} running and accessible? {n} v2 is not supported."),
                ),
                (HttpError::Json(m), _) => ProviderFailure::new("BaseUrl", format!("Unable to parse JSON response from application. {m}")),
                _ => {
                    ProviderFailure::new("BaseUrl", format!("Unable to complete application test, cannot connect to {n}. {}", e.message()))
                }
            };
            vec![f]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexers_compare_like_sonarr_indexer_equals() {
        let mut a = Map::new();
        a.insert("id".into(), json!(3));
        a.insert("name".into(), json!("Site (Prowlarr)"));
        a.insert(
            "fields".into(),
            json!([
                { "name": "baseUrl", "value": "http://lantern:9696/1/" },
                { "name": "apiPath", "value": "/api" },
                { "name": "apiKey", "value": "key" },
                { "name": "categories", "value": [5000, 5040] },
                { "name": "animeCategories", "value": [] },
                { "name": "minimumSeeders", "value": 1 },
            ]),
        );
        let mut b = a.clone();
        // the app masks the key, and sends numbers as it likes
        set(&mut b, "apiKey", json!(super::super::provider::MASK)).unwrap();
        set(&mut b, "minimumSeeders", json!(1.0)).unwrap();
        assert!(same(&SONARR, &a, &b));
        set(&mut b, "categories", json!([5040, 5000])).unwrap();
        assert!(!same(&SONARR, &a, &b));
        let mut c = a.clone();
        set(&mut c, "animeCategories", json!([5070])).unwrap();
        assert!(!same(&SONARR, &a, &c));
        // Radarr does not look at anime categories
        assert!(same(&RADARR, &a, &c));
    }
}
