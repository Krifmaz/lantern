// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Applications/Mylar/Mylar.cs, MylarIndexer.cs,
// MylarV3Proxy.cs, MylarStatus.cs, MylarError.cs, src/NzbDrone.Core/Applications/LazyLibrarian/LazyLibrarian.cs,
// LazyLibrarianIndexer.cs, LazyLibrarianV1Proxy.cs, LazyLibrarianStatus.cs, LazyLibrarianError.cs), GPL-3.0.

//! Mylar and LazyLibrarian: their API is one address with a `cmd` and the app's API key in the query. Lantern's
//! indexers become their Newznab or Torznab providers, found again by type and name (`Torznab,Site (Prowlarr)`).

use super::http::{self, HttpError, Request};
use super::provider::{Definition, ProviderFailure};
use super::sync::{Ctx, RemoteMap, SyncIndexer};
use reqwest::Method;
use serde_json::Value;

/// What one of the two apps does differently.
#[derive(Debug)]
pub struct Spec {
    /// Its name.
    pub name: &'static str,
    /// The command that lists its providers.
    pub list_command: &'static str,
    /// LazyLibrarian also takes the priority and the seed settings.
    pub seeds: bool,
}

/// Mylar.
pub static MYLAR: Spec = Spec { name: "Mylar", list_command: "listProviders", seeds: false };

/// LazyLibrarian.
pub static LAZYLIBRARIAN: Spec = Spec { name: "LazyLibrarian", list_command: "listNabProviders", seeds: true };

/// Prowlarr's highest indexer priority: LazyLibrarian's `dlpriority` is 51 minus the indexer's priority.
const HIGHEST_PRIORITY: i64 = 50;

/// One of the app's providers (`MylarIndexer`, `LazyLibrarianIndexer`).
#[derive(Debug, Clone, PartialEq)]
pub struct NabIndexer {
    /// Its name in the app.
    pub name: String,
    /// The address of its API.
    pub host: String,
    /// Its API key.
    pub apikey: String,
    /// Its categories, comma separated.
    pub categories: String,
    /// Whether it is on.
    pub enabled: bool,
    /// The name it should have.
    pub altername: String,
    /// `Newznab` or `Torznab`.
    pub kind: &'static str,
    /// Its priority (LazyLibrarian).
    pub priority: i64,
    /// Its seed ratio (LazyLibrarian).
    pub seed_ratio: f64,
    /// Its seed time (LazyLibrarian).
    pub seed_time: i64,
    /// Its minimum seeders (LazyLibrarian).
    pub minimum_seeders: i64,
}

impl NabIndexer {
    /// Whether the app's provider already is what Lantern would send (`Equals`).
    pub fn same(&self, spec: &Spec, other: &NabIndexer) -> bool {
        let base = other.host == self.host
            && other.apikey == self.apikey
            && other.name == self.name
            && other.categories == self.categories
            && other.enabled == self.enabled
            && other.altername == self.altername;
        base && (!spec.seeds
            || (other.priority == self.priority
                && other.seed_ratio == self.seed_ratio
                && other.seed_time == self.seed_time
                && other.minimum_seeders == self.minimum_seeders))
    }
}

/// A property of the app's JSON, its name in any case (Prowlarr's reader ignores case).
fn get<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    v.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// A provider from the app's list.
fn of_json(v: &Value, kind: &'static str) -> NabIndexer {
    let s = |k: &str| get(v, k).and_then(Value::as_str).unwrap_or("").to_string();
    let i = |k: &str| get(v, k).and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
    let name = s("name");
    NabIndexer {
        host: s("host"),
        apikey: s("apikey"),
        categories: s("categories"),
        enabled: get(v, "enabled")
            .and_then(|x| x.as_bool().or_else(|| x.as_str().map(|s| s.eq_ignore_ascii_case("true"))))
            .unwrap_or(false),
        altername: name.clone(),
        name,
        kind,
        priority: i("priority"),
        seed_ratio: get(v, "seedRatio").and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0.0),
        seed_time: i("seedTime"),
        minimum_seeders: i("minimumSeeders"),
    }
}

/// The app's error, as Prowlarr's `MylarException` says it.
fn app_error(spec: &Spec, answer: &Value) -> HttpError {
    let e = get(answer, "error");
    let code = e.and_then(|e| get(e, "code")).and_then(Value::as_i64).unwrap_or(0);
    let message = e.and_then(|e| get(e, "message")).and_then(Value::as_str).unwrap_or("");
    HttpError::Other(format!("{} Error - Code {code}: {message}", spec.name))
}

/// Whether the app's answer says it worked.
fn success(v: &Value) -> bool {
    get(v, "success").and_then(Value::as_bool).unwrap_or(false)
}

/// A command to the app: `<base>/api?cmd=<command>&apikey=<key>&<params>`.
async fn call(app: &Definition, command: &str, params: Vec<(&str, String)>) -> Result<Value, HttpError> {
    let mut query = vec![("cmd", command.to_string()), ("apikey", app.text("apiKey"))];
    query.extend(params);
    let req = Request {
        method: Method::GET,
        url: http::address(&app.text("baseUrl"), "/api", &query),
        api_key: None,
        auth: http::basic_auth(&app.text("authUsername"), &app.text("authPassword")),
        body: None,
    };
    http::json(&req).await
}

/// The app's providers (`GetIndexers`): its Torznab ones, then its Newznab ones.
async fn indexers(spec: &Spec, app: &Definition) -> Result<Vec<NabIndexer>, HttpError> {
    let answer = call(app, spec.list_command, Vec::new()).await?;
    if !success(&answer) {
        return Err(app_error(spec, &answer));
    }
    let data = get(&answer, "data");
    let list = |k: &str| data.and_then(|d| get(d, k)).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<NabIndexer> = list("torznabs").iter().map(|v| of_json(v, "Torznab")).collect();
    out.extend(list("newznabs").iter().map(|v| of_json(v, "Newznab")));
    Ok(out)
}

/// One of the app's providers by name and type (`GetIndexer`).
async fn indexer(spec: &Spec, app: &Definition, name: &str, kind: &str) -> Result<Option<NabIndexer>, HttpError> {
    let found: Vec<NabIndexer> = indexers(spec, app).await?.into_iter().filter(|i| i.name == name && i.kind == kind).collect();
    if found.len() > 1 {
        return Err(HttpError::Other("Sequence contains more than one matching element".into()));
    }
    Ok(found.into_iter().next())
}

/// Checks an answer that only says whether it worked (`CheckForError`).
fn checked(spec: &Spec, answer: Value) -> Result<(), HttpError> {
    if success(&answer) {
        Ok(())
    } else {
        Err(app_error(spec, &answer))
    }
}

/// The parameters of an add or a change, in Prowlarr's order.
fn params(spec: &Spec, ix: &NabIndexer, altername: bool) -> Vec<(&'static str, String)> {
    let mut p = vec![
        ("name", ix.name.clone()),
        ("providertype", ix.kind.to_lowercase()),
        ("host", ix.host.clone()),
        ("prov_apikey", ix.apikey.clone()),
        ("enabled", ix.enabled.to_string()),
        ("categories", ix.categories.clone()),
    ];
    if altername {
        p.push(("altername", ix.altername.clone()));
    }
    if spec.seeds {
        p.push(("dlpriority", (HIGHEST_PRIORITY - ix.priority + 1).to_string()));
        if ix.kind == "Torznab" {
            p.push(("seeders", ix.minimum_seeders.to_string()));
            p.push(("seed_ratio", dotnet_double(ix.seed_ratio)));
            p.push(("seed_duration", ix.seed_time.to_string()));
        }
    }
    p
}

/// A number as .NET writes a `double` (`1`, `1.5`).
fn dotnet_double(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The app's provider for Lantern's indexer (`BuildMylarIndexer`); `original_name` is the name the app has for it.
pub fn build(spec: &Spec, app: &Definition, ix: &SyncIndexer, api_key: &str, original_name: Option<&str>) -> NabIndexer {
    let name = format!("{} (Prowlarr)", ix.name);
    let cats: Vec<String> = ix.categories.supported(&app.ints("syncCategories")).iter().map(i64::to_string).collect();
    let mut out = NabIndexer {
        name: original_name.map(str::to_string).unwrap_or_else(|| name.clone()),
        altername: name,
        host: format!("{}/{}/api", app.text("prowlarrUrl").trim_end_matches('/'), ix.id),
        apikey: api_key.to_string(),
        categories: cats.join(","),
        enabled: ix.enable,
        kind: if ix.torrent { "Torznab" } else { "Newznab" },
        priority: 0,
        seed_ratio: 0.0,
        seed_time: 0,
        minimum_seeders: 0,
    };
    if spec.seeds {
        out.priority = ix.priority;
        if ix.torrent {
            out.minimum_seeders = ix.app_minimum_seeders.unwrap_or(ix.profile.minimum_seeders);
            out.seed_ratio = ix.seed_ratio.unwrap_or(0.0);
            out.seed_time = ix.seed_time.unwrap_or(0);
        }
    }
    out
}

/// The providers in the app that are Lantern's (`GetIndexerMappings`): those with Lantern's API key, by the
/// indexer id in their address.
pub async fn mappings(spec: &Spec, ctx: &Ctx) -> Result<Vec<RemoteMap>, HttpError> {
    let mut out = Vec::new();
    for ix in indexers(spec, &ctx.app).await? {
        if ix.apikey == ctx.api_key {
            if let Some(id) = super::sync::indexer_id_in(&ix.host) {
                out.push(RemoteMap { indexer_id: id, remote_id: 0, remote_name: Some(format!("{},{}", ix.kind, ix.name)) });
            }
        }
    }
    Ok(out)
}

/// Adds Lantern's indexer to the app (`AddIndexer`), when it has one of the app's sync categories.
pub async fn add(spec: &Spec, ctx: &Ctx, ix: &SyncIndexer) -> Result<(), HttpError> {
    if ix.categories.supported(&ctx.app.ints("syncCategories")).is_empty() {
        tracing::trace!("Skipping add for indexer {} [{}] due to no app Sync Categories supported by the indexer", ix.name, ix.id);
        return Ok(());
    }
    tracing::trace!("Adding indexer {} [{}]", ix.name, ix.id);
    let ours = build(spec, &ctx.app, ix, &ctx.api_key, None);
    checked(spec, call(&ctx.app, "addProvider", params(spec, &ours, false)).await?)?;
    super::sync::insert_map(&ctx.st.db, ctx.app.id, ix.id, 0, Some(&format!("{},{}", ours.kind, ours.name)))
        .map_err(|e| HttpError::Other(e.detail))?;
    Ok(())
}

/// The type and name of a mapping's remote name (`Torznab,Site (Prowlarr)`).
fn split(remote_name: &str) -> (String, String) {
    let mut parts = remote_name.splitn(2, ',');
    let kind = parts.next().unwrap_or("").to_string();
    let name = parts.next().unwrap_or("").split(',').next().unwrap_or("").to_string();
    (kind, name)
}

/// Removes Lantern's indexer `indexer_id` from the app (`RemoveIndexer`).
pub async fn remove_indexer(spec: &Spec, ctx: &Ctx, indexer_id: i64) -> Result<(), HttpError> {
    let maps = super::sync::maps_for_app(&ctx.st.db, ctx.app.id).map_err(|e| HttpError::Other(e.detail))?;
    if let Some(m) = maps.iter().find(|m| m.indexer_id == indexer_id) {
        let (kind, name) = split(m.remote_name.as_deref().unwrap_or(""));
        let kind = match kind.as_str() {
            "Newznab" => "newznab",
            "Torznab" => "torznab",
            other => return Err(HttpError::Other(format!("Requested value '{other}' was not found."))),
        };
        checked(spec, call(&ctx.app, "delProvider", vec![("name", name), ("providertype", kind.into())]).await?)?;
        super::sync::delete_map(&ctx.st.db, m.id).map_err(|e| HttpError::Other(e.detail))?;
    }
    Ok(())
}

/// Brings the app's provider up to date (`UpdateIndexer`), found by the name it had (so a rename in Lantern
/// renames it), or adds it again when the app lost it.
pub async fn update(spec: &Spec, ctx: &Ctx, ix: &SyncIndexer, force: bool) -> Result<(), HttpError> {
    let db = &ctx.st.db;
    tracing::debug!("Updating indexer {} [{}]", ix.name, ix.id);
    let maps = super::sync::maps_for_app(db, ctx.app.id).map_err(|e| HttpError::Other(e.detail))?;
    let mut mapping = maps
        .iter()
        .find(|m| m.indexer_id == ix.id)
        .cloned()
        .ok_or_else(|| HttpError::Other("Object reference not set to an instance of an object.".into()))?;
    let (_, old_name) = split(mapping.remote_name.as_deref().unwrap_or(""));
    let ours = build(spec, &ctx.app, ix, &ctx.api_key, Some(&old_name));
    match indexer(spec, &ctx.app, &old_name, ours.kind).await? {
        Some(remote) => {
            tracing::debug!("Remote indexer {} found", remote.name);
            if !ours.same(spec, &remote) || force {
                tracing::debug!("Syncing remote indexer with current settings");
                checked(spec, call(&ctx.app, "changeProvider", params(spec, &ours, true)).await?)?;
                mapping.remote_name = Some(format!("{},{}", ours.kind, ours.altername));
                super::sync::update_map(db, &mapping).map_err(|e| HttpError::Other(e.detail))?;
            }
        }
        None => {
            super::sync::delete_map(db, mapping.id).map_err(|e| HttpError::Other(e.detail))?;
            if !ix.categories.supported(&ctx.app.ints("syncCategories")).is_empty() {
                tracing::debug!("Remote indexer not found, re-adding {} [{}] to {}", ix.name, ix.id, spec.name);
                checked(spec, call(&ctx.app, "addProvider", params(spec, &ours, false)).await?)?;
                super::sync::insert_map(db, ctx.app.id, ix.id, 0, Some(&format!("{},{}", ours.kind, ours.name)))
                    .map_err(|e| HttpError::Other(e.detail))?;
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

/// Tests the app (`TestConnection`): its version answer must say it worked, then its providers must list.
pub async fn test(spec: &Spec, ctx: &Ctx) -> Vec<ProviderFailure> {
    let status = match call(&ctx.app, "getVersion", Vec::new()).await {
        Ok(v) => v,
        Err(e) => return vec![failure_of(spec, e)],
    };
    if !success(&status) {
        let message = get(&status, "error").and_then(|e| get(e, "message")).and_then(Value::as_str).unwrap_or("");
        return vec![ProviderFailure::new("ApiKey", message)];
    }
    match indexers(spec, &ctx.app).await {
        Ok(_) => Vec::new(),
        Err(e) => vec![failure_of(spec, e)],
    }
}

/// The test's failure for an error.
fn failure_of(spec: &Spec, e: HttpError) -> ProviderFailure {
    tracing::error!("{}: unable to complete application test: {e}", spec.name);
    match &e {
        HttpError::Status { .. } => ProviderFailure::new("BaseUrl", "Unable to complete application test"),
        HttpError::Other(m) if m.starts_with(&format!("{} Error - Code", spec.name)) => ProviderFailure::new("", m.clone()),
        _ => ProviderFailure::new("", format!("Unable to send test message. {}", e.message())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazylibrarian_takes_priority_and_seeds() {
        let ix = NabIndexer {
            name: "Site (Prowlarr)".into(),
            host: "http://lantern:9696/3/api".into(),
            apikey: "key".into(),
            categories: "7000,7020".into(),
            enabled: true,
            altername: "Site (Prowlarr)".into(),
            kind: "Torznab",
            priority: 25,
            seed_ratio: 1.5,
            seed_time: 60,
            minimum_seeders: 1,
        };
        let p = params(&LAZYLIBRARIAN, &ix, true);
        let keys: Vec<&str> = p.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "name",
                "providertype",
                "host",
                "prov_apikey",
                "enabled",
                "categories",
                "altername",
                "dlpriority",
                "seeders",
                "seed_ratio",
                "seed_duration"
            ]
        );
        assert_eq!(p[7].1, "26");
        assert_eq!(p[9].1, "1.5");
        assert_eq!(p[4].1, "true");
        assert_eq!(params(&MYLAR, &ix, false).len(), 6);
        assert_eq!(split("Torznab,Site, with comma (Prowlarr)"), ("Torznab".into(), "Site".into()));
        let mut other = ix.clone();
        other.priority = 3;
        assert!(ix.same(&MYLAR, &other) && !ix.same(&LAZYLIBRARIAN, &other));
        assert_eq!(dotnet_double(2.0), "2");
    }
}
