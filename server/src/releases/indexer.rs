// Ported from autobrr v1.90.0 (internal/indexer/service.go, internal/database/indexer.go,
// internal/domain/indexer.go Indexer), GPL-2.0-or-later.
//! autobrr's indexers: a tracker set up from its definition, with the site settings it asks for (passkey, API
//! key …). Lantern's release sources are autobrr's indexer table: an autobrr indexer is a row of
//! `release_sources` (`kind` `irc`, or `feed` for Torznab, Newznab and RSS indexers, `definition` its identifier,
//! `settings` its settings) plus a row of `autobrr_indexer_meta` with what Lantern's sources don't have (the
//! external identifier, implementation, base URL, proxy and archive state). Lantern's own RSS and indexer
//! sources show up as RSS indexers, as before.
//!
//! An indexer's definition, mapped with its settings ([`Releases::mapped_definition`]), is what reads its
//! announces; the IRC networks find the indexers of their server through [`Releases::indexers_by_irc_server`].

use super::definition::{is_feed_implementation, is_redacted, is_secret_indexer_setting, redact, IndexerDefinition, IndexerDeprecation};
use super::store::{ApiError, ApiResult};
use super::{Releases, Source};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The settings of Lantern's own IRC sources that belong to their IRC network, not to the indexer: kept on the
/// source, left out of the indexer.
pub const NETWORK_SETTINGS: [&str; 9] =
    ["nick", "auth.account", "auth.password", "invite_command", "pass", "server", "port", "tls", "channels"];

/// One indexer (autobrr's `Indexer`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Indexer {
    /// Its id (the release source's).
    pub id: i64,
    /// The name.
    pub name: String,
    /// The definition's identifier (`redacted`); a feed's is `torznab-<name>`.
    pub identifier: String,
    /// The identifier elsewhere (Prowlarr's); the name by default.
    pub identifier_external: String,
    /// On.
    pub enabled: bool,
    /// `irc`, `torznab`, `newznab` or `rss`.
    pub implementation: String,
    /// The site address links are built on (IRC indexers).
    pub base_url: String,
    /// Through a proxy.
    pub use_proxy: bool,
    /// The proxy (autobrr's proxies aren't ported: always `null`).
    pub proxy: Option<serde_json::Value>,
    /// The proxy's id.
    pub proxy_id: i64,
    /// The settings, by name.
    pub settings: BTreeMap<String, String>,
    /// Its definition was removed upstream.
    pub archived: bool,
    /// When it was archived.
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Serialize for Indexer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("id", &self.id)?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("identifier", &self.identifier)?;
        m.serialize_entry("identifier_external", &self.identifier_external)?;
        m.serialize_entry("enabled", &self.enabled)?;
        m.serialize_entry("implementation", &self.implementation)?;
        if !self.base_url.is_empty() {
            m.serialize_entry("base_url", &self.base_url)?;
        }
        m.serialize_entry("use_proxy", &self.use_proxy)?;
        m.serialize_entry("proxy", &self.proxy)?;
        m.serialize_entry("proxy_id", &self.proxy_id)?;
        // Go's omitempty never drops the redacted copy, so settings are always sent
        let settings: BTreeMap<&String, String> =
            self.settings.iter().map(|(k, v)| (k, if is_secret_indexer_setting(k) { redact(v) } else { v.clone() })).collect();
        m.serialize_entry("settings", &settings)?;
        m.serialize_entry("archived", &self.archived)?;
        if let Some(at) = &self.archived_at {
            m.serialize_entry("archived_at", at)?;
        }
        m.end()
    }
}

impl Indexer {
    /// Whether it reads feeds.
    pub fn is_feed(&self) -> bool {
        is_feed_implementation(&self.implementation)
    }
}

/// gosimple/slug's `Make` for the names autobrr gives feed indexers: lowercase letters and digits, everything
/// else one `-`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

/// The indexer extras table.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_indexer_meta (
            source_id INTEGER PRIMARY KEY,
            identifier_external TEXT NOT NULL DEFAULT '',
            implementation TEXT NOT NULL DEFAULT '',
            base_url TEXT NOT NULL DEFAULT '',
            use_proxy INTEGER NOT NULL DEFAULT 0,
            proxy_id INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            archived_at TEXT NOT NULL DEFAULT ''
        );",
    )
    .map_err(|e| anyhow::anyhow!("The autobrr indexer table can't be made: {}", e.detail))
}

impl Releases {
    /// Source `s` as an autobrr indexer.
    fn indexer_of(&self, s: &Source) -> Indexer {
        let meta = self.db.one("SELECT * FROM autobrr_indexer_meta WHERE source_id=?", p![s.id]).ok().flatten();
        let lantern_kind = s.kind == "rss" || s.kind == "indexer";
        let identifier = if s.definition.is_empty() { s.name.to_lowercase() } else { s.definition.clone() };
        let mut i = Indexer {
            id: s.id,
            name: s.name.clone(),
            identifier,
            identifier_external: s.name.clone(),
            enabled: s.enabled,
            implementation: if s.kind == "irc" { "irc".into() } else { "rss".into() },
            settings: if lantern_kind {
                BTreeMap::new()
            } else {
                s.settings.iter().filter(|(k, _)| !NETWORK_SETTINGS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
            },
            ..Indexer::default()
        };
        if let Some(m) = meta {
            i.identifier_external = m.s("identifier_external");
            i.implementation = m.s("implementation");
            i.base_url = m.s("base_url");
            i.use_proxy = m.b("use_proxy");
            i.proxy_id = m.i("proxy_id");
            i.archived = m.b("archived");
            i.archived_at = chrono::DateTime::parse_from_rfc3339(&m.s("archived_at")).ok().map(|t| t.with_timezone(&chrono::Utc));
        }
        i
    }

    /// Every indexer, by name (autobrr's `List`).
    pub fn autobrr_indexers(&self) -> ApiResult<Vec<Indexer>> {
        let mut list: Vec<Indexer> = self.sources()?.iter().map(|s| self.indexer_of(s)).collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    /// Indexer `id`; autobrr's `record not found` when there is none.
    pub fn autobrr_indexer(&self, id: i64) -> ApiResult<Indexer> {
        let s = self.source(id).map_err(|_| ApiError::NotFound("record not found".into()))?;
        Ok(self.indexer_of(&s))
    }

    /// Saves `i`'s row and extras (a new row when its id is 0); its id.
    fn write_indexer(&self, i: &Indexer) -> ApiResult<i64> {
        let kind = if i.implementation == "irc" || i.implementation.is_empty() { "irc" } else { "feed" };
        let mut all = i.settings.clone();
        if i.id != 0 {
            if let Ok(cur) = self.source(i.id) {
                all.extend(cur.settings.into_iter().filter(|(k, _)| NETWORK_SETTINGS.contains(&k.as_str())));
            }
        }
        let settings = serde_json::to_string(&all).unwrap_or_default();
        let id = if i.id == 0 {
            self.db.execute(
                "INSERT INTO release_sources (kind, name, definition, settings, interval_min, enabled, added) VALUES (?,?,?,?,?,?,?)",
                p![kind, i.name, i.identifier, settings, 10, i.enabled, crate::auth::now()],
            )?
        } else {
            self.db.execute(
                "UPDATE release_sources SET kind=?, name=?, definition=?, settings=?, enabled=? WHERE id=?",
                p![kind, i.name, i.identifier, settings, i.enabled, i.id],
            )?;
            i.id
        };
        let archived_at = i.archived_at.map(|t| t.to_rfc3339()).unwrap_or_default();
        self.db.execute(
            "INSERT OR REPLACE INTO autobrr_indexer_meta (source_id, identifier_external, implementation, base_url, use_proxy, proxy_id, archived, archived_at) VALUES (?,?,?,?,?,?,?,?)",
            p![id, i.identifier_external, i.implementation, i.base_url, i.use_proxy, i.proxy_id, i.archived, archived_at],
        )?;
        Ok(id)
    }

    /// The definition indexer `i` is set up from (a feed's is its implementation's generic one).
    fn template_of(&self, i: &Indexer) -> Option<Arc<IndexerDefinition>> {
        let name = if i.is_feed() { i.implementation.as_str() } else { i.identifier.as_str() };
        self.library.get(name)
    }

    /// `i`'s definition with its own values (autobrr's `mapIndexer`); `None` without a definition.
    pub fn map_indexer(&self, i: &Indexer) -> Option<IndexerDefinition> {
        let mut d = (*self.template_of(i)?).clone();
        d.id = i.id;
        d.name = i.name.clone();
        d.identifier = i.identifier.clone();
        d.identifier_external = i.identifier_external.clone();
        d.implementation = if i.implementation.is_empty() { "irc".into() } else { i.implementation.clone() };
        d.base_url = i.base_url.clone();
        d.enabled = i.enabled;
        d.use_proxy = i.use_proxy;
        d.proxy_id = i.proxy_id;
        for set in d.settings.iter_mut() {
            if let Some(v) = i.settings.get(&set.name) {
                set.value = v.clone();
                d.settings_map.insert(set.name.clone(), v.clone());
            }
        }
        Some(d)
    }

    /// Indexer `id`'s mapped definition.
    pub fn mapped_definition(&self, id: i64) -> Option<IndexerDefinition> {
        let i = self.autobrr_indexer(id).ok()?;
        self.map_indexer(&i)
    }

    /// Every mapped definition, by name in any case (autobrr's `GetAll`).
    pub fn mapped_definitions(&self) -> ApiResult<Vec<IndexerDefinition>> {
        let mut out: Vec<IndexerDefinition> = self.autobrr_indexers()?.iter().filter_map(|i| self.map_indexer(i)).collect();
        out.sort_by_key(|d| d.name.to_lowercase());
        Ok(out)
    }

    /// Every definition, by name in any case (autobrr's `GetTemplates`).
    pub fn indexer_templates(&self) -> Vec<Arc<IndexerDefinition>> {
        let mut out = self.library.all();
        out.sort_by_key(|d| d.name.to_lowercase());
        out
    }

    /// The mapped IRC definitions whose network is `server` (autobrr's `GetIndexersByIRCNetwork`).
    pub fn indexers_by_irc_server(&self, server: &str) -> Vec<Arc<IndexerDefinition>> {
        let server = server.to_lowercase();
        self.mapped_definitions()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.implementation == "irc" && d.irc.as_ref().is_some_and(|irc| irc.server.to_lowercase() == server))
            .map(Arc::new)
            .collect()
    }

    /// Adds indexer `i` (autobrr's `Store`): its name and settings trimmed, a feed's identifier made from its
    /// name. The saved indexer, or the problem in autobrr's words.
    pub fn store_indexer(&self, mut i: Indexer) -> ApiResult<Indexer> {
        i.name = super::sanitize::string(&i.name);
        for v in i.settings.values_mut() {
            *v = super::sanitize::string(v);
        }
        if i.is_feed() {
            i.identifier = slug(&format!("{}-{}", i.implementation, i.name.to_lowercase()));
        }
        if i.identifier_external.is_empty() {
            i.identifier_external = i.name.clone();
        }
        i.id = 0;
        i.id = self.write_indexer(&i)?;
        if self.map_indexer(&i).is_none() {
            return Err(ApiError::Internal("indexer not found".into()));
        }
        Ok(i)
    }

    /// Changes indexer `i` (autobrr's `Update`): a redacted setting keeps the saved one, a saved secret must be
    /// sent back, an IRC indexer needs a base URL. What was saved, or the problem in autobrr's words.
    pub fn update_indexer(&self, mut i: Indexer) -> ApiResult<Indexer> {
        let cur = self
            .autobrr_indexer(i.id)
            .map_err(|_| ApiError::NotFound(format!("could not find indexer by id: {}: record not found", i.id)))?;
        if cur.archived {
            return Err(ApiError::BadRequest("indexer is archived".into()));
        }
        i.name = super::sanitize::string(&i.name);
        for (k, v) in i.settings.iter_mut() {
            if is_redacted(v) {
                *v = cur.settings.get(k).cloned().ok_or_else(|| ApiError::Internal("could not find setting in current indexer".into()))?;
            } else {
                *v = super::sanitize::string(v);
            }
        }
        let def = self.map_indexer(&cur);
        for (k, v) in &cur.settings {
            if v.is_empty() || !is_secret_indexer_setting(k) {
                continue;
            }
            if def.as_ref().is_some_and(|d| !d.settings.iter().any(|s| &s.name == k)) {
                continue;
            }
            if !i.settings.contains_key(k) {
                return Err(ApiError::Internal(format!("update omits saved secret setting '{k}'")));
            }
        }
        if i.implementation == "irc" && i.base_url.is_empty() {
            return Err(ApiError::Internal("indexer baseURL must not be empty".into()));
        }
        i.archived = cur.archived;
        i.archived_at = cur.archived_at;
        self.write_indexer(&i)?;
        if self.map_indexer(&i).is_none() {
            return Err(ApiError::Internal("indexer not found".into()));
        }
        if cur.is_feed() {
            self.feed_indexer_toggled(i.id);
        }
        Ok(i)
    }

    /// Removes indexer `id` (autobrr's `Delete`).
    pub fn delete_indexer(&self, id: i64) -> ApiResult<()> {
        let i = self.autobrr_indexer(id)?;
        if i.archived {
            return Err(ApiError::BadRequest("indexer is archived".into()));
        }
        self.remove_source(id).map_err(|_| ApiError::NotFound("record not found".into()))?;
        self.db.execute("DELETE FROM autobrr_indexer_meta WHERE source_id=?", p![id])?;
        Ok(())
    }

    /// Turns indexer `id` on or off (autobrr's `ToggleEnabled`).
    pub fn toggle_indexer(&self, id: i64, enabled: bool) -> ApiResult<()> {
        let i = self.autobrr_indexer(id)?;
        if i.archived {
            return Err(ApiError::BadRequest("indexer is archived".into()));
        }
        self.db.execute("UPDATE release_sources SET enabled=? WHERE id=?", p![enabled, id])?;
        if i.is_feed() {
            self.feed_indexer_toggled(id);
        }
        Ok(())
    }

    /// How many filters watch indexer `id`.
    fn filters_using(&self, ids: &[i64]) -> i64 {
        self.filters().unwrap_or_default().iter().filter(|f| f.indexers.iter().any(|i| ids.contains(i))).count() as i64
    }

    /// The tombstones of removed indexers, by name, with how many filters still use each (autobrr's
    /// `ListDeprecations`).
    pub fn indexer_deprecations(&self) -> ApiResult<Vec<IndexerDeprecation>> {
        let indexers = self.autobrr_indexers()?;
        let mut out: Vec<IndexerDeprecation> = self
            .library
            .deprecations()
            .into_iter()
            .map(|mut d| {
                let ids: Vec<i64> = indexers.iter().filter(|i| i.identifier == d.identifier).map(|i| i.id).collect();
                d.filter_count = self.filters_using(&ids);
                d
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Archives the indexers whose definition was removed and brings back those that have one again (autobrr's
    /// `reconcileDeprecations`).
    pub fn reconcile_deprecations(&self) -> ApiResult<()> {
        let deprecated: Vec<String> = self.library.deprecations().into_iter().map(|d| d.identifier).collect();
        for mut i in self.autobrr_indexers()? {
            let is_lantern = self.source(i.id).is_ok_and(|s| s.kind == "rss" || s.kind == "indexer");
            if is_lantern {
                continue;
            }
            let live = self.template_of(&i).is_some();
            if live && i.archived {
                i.archived = false;
                i.archived_at = None;
                self.write_indexer(&i)?;
            } else if !live && !i.archived && deprecated.contains(&i.identifier) {
                i.archived = true;
                i.archived_at = Some(chrono::Utc::now());
                self.write_indexer(&i)?;
            }
        }
        Ok(())
    }

    /// Removes archived indexer `id` once no filter uses it (autobrr's `DeleteArchived`).
    pub fn delete_archived_indexer(&self, id: i64) -> ApiResult<()> {
        let i = self.autobrr_indexer(id)?;
        if !i.archived {
            return Err(ApiError::BadRequest("indexer is not archived".into()));
        }
        if self.filters_using(&[id]) > 0 {
            return Err(ApiError::BadRequest("indexer is still used by filters".into()));
        }
        self.remove_source(id).map_err(|_| ApiError::NotFound("record not found".into()))?;
        self.db.execute("DELETE FROM autobrr_indexer_meta WHERE source_id=?", p![id])?;
        Ok(())
    }

    /// Tests indexer `id`'s site API (autobrr's `TestApi`). The site API clients aren't ported yet, so an indexer
    /// with an API answers as autobrr does when it has no client for it.
    pub fn test_indexer_api(&self, id: i64) -> ApiResult<()> {
        let i = self.autobrr_indexer(id)?;
        if i.archived {
            return Err(ApiError::BadRequest("indexer is archived".into()));
        }
        let def = self.map_indexer(&i).ok_or_else(|| ApiError::Internal(format!("could not find indexer definition: {}", i.identifier)))?;
        if !def.has_api() {
            return Err(ApiError::Internal(format!("indexer ({}) does not support api", i.identifier)));
        }
        Err(ApiError::Internal(format!("could not init api client: {}", def.identifier)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_like_gosimple() {
        assert_eq!(slug("torznab-My Indexer!"), "torznab-my-indexer");
        assert_eq!(slug("rss--a  b"), "rss-a-b");
    }

    #[test]
    fn redacts_secret_settings_by_key() {
        let i = Indexer {
            settings: BTreeMap::from([("passkey".into(), "x".into()), ("uploader".into(), "me".into()), ("api_key".into(), String::new())]),
            ..Indexer::default()
        };
        let v = serde_json::to_value(&i).unwrap();
        assert_eq!(v["settings"], serde_json::json!({"api_key": "", "passkey": "<redacted>", "uploader": "me"}));
        assert!(v.get("base_url").is_none() && v.get("archived_at").is_none());
        assert_eq!(v["proxy"], serde_json::Value::Null);
    }
}
