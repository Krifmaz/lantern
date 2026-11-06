// Ported from autobrr v1.90.0 (internal/database/action.go, downloader.go, filter.go external parts,
// notification.go filter parts; internal/filter/service.go Store, Update, UpdatePartial, Duplicate, Delete,
// validateIndexers, validateNotifications), GPL-2.0-or-later.
//! Where autobrr's records live: download clients, actions, the filters' notifications and the notifications
//! they name, each a row whose body is the record as JSON; the filters themselves stay in `release_filters`
//! (their external filters in the filter's body). The filter service's rules are kept: saving a filter's actions
//! changes or adds the ones sent and leaves the others, a filter's indexers and notifications must exist.

use super::action::Action;
use super::downloader::Downloader;
use super::external::FilterExternal;
use super::filter::{self, Filter};
use super::Releases;
use kit_server_core::{p, Db, Row, RowExt, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Why an autobrr API call failed, as autobrr's HTTP layer sorts it.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// 404, `RECORD_NOT_FOUND`.
    NotFound(String),
    /// 400, `BAD_REQUEST_PARAMS`.
    BadRequest(String),
    /// 500, `INTERNAL_SERVER_ERROR`.
    Internal(String),
}

impl ApiError {
    /// The message.
    pub fn message(&self) -> &str {
        match self {
            ApiError::NotFound(m) | ApiError::BadRequest(m) | ApiError::Internal(m) => m,
        }
    }
}

impl From<kit_server_core::Error> for ApiError {
    fn from(e: kit_server_core::Error) -> ApiError {
        ApiError::Internal(e.detail)
    }
}

/// A filter read from a body, with the actions and notifications it sent (`None` when left out).
type FilterParts = (Filter, Option<Vec<Action>>, Option<Vec<FilterNotification>>);

/// An autobrr result.
pub type ApiResult<T> = std::result::Result<T, ApiError>;

/// A filter's notification (autobrr's `FilterNotification`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterNotification {
    /// The filter.
    pub filter_id: i64,
    /// The filter's name.
    pub filter_name: String,
    /// The notification.
    pub notification_id: i64,
    /// The events it sends (`PUSH_APPROVED` …).
    pub events: Vec<String>,
}

/// The autobrr tables.
pub fn migrate(db: &SqliteDb) -> anyhow::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS autobrr_downloaders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            body TEXT NOT NULL DEFAULT '{}'
        );
        CREATE TABLE IF NOT EXISTS autobrr_actions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            filter_id INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            body TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS autobrr_actions_by_filter ON autobrr_actions(filter_id);
        CREATE TABLE IF NOT EXISTS autobrr_notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            body TEXT NOT NULL DEFAULT '{}'
        );
        CREATE TABLE IF NOT EXISTS autobrr_filter_notifications (
            filter_id INTEGER NOT NULL,
            notification_id INTEGER NOT NULL,
            events TEXT NOT NULL DEFAULT '[]',
            PRIMARY KEY (filter_id, notification_id)
        );",
    )
    .map_err(|e| anyhow::anyhow!("The autobrr tables can't be made: {}", e.detail))
}

/// A download client from its row.
fn downloader_of(r: &Row) -> Downloader {
    let mut d: Downloader = serde_json::from_str(&r.s("body")).unwrap_or_default();
    d.id = r.i("id");
    d.name = r.s("name");
    d.kind = r.s("kind");
    d.enabled = r.b("enabled");
    d
}

/// An action from its row.
fn action_of(r: &Row) -> Action {
    let mut a: Action = serde_json::from_str(&r.s("body")).unwrap_or_default();
    a.id = r.i("id");
    a.filter_id = r.i("filter_id");
    a.enabled = r.b("enabled");
    a
}

/// An action as stored: no client, no webhook headers (autobrr's table has no place for them).
fn action_body(a: &Action) -> String {
    let mut a = a.clone();
    a.client = None;
    a.webhook_headers.clear();
    serde_json::to_string(&a).unwrap_or_default()
}

/// The sort `name-asc` … autobrr's filter list takes; the problem in autobrr's words.
pub fn parse_sort(sort: &str) -> ApiResult<Option<(String, bool)>> {
    if sort.is_empty() {
        return Ok(None);
    }
    let Some((field, order)) = sort.split_once('-') else { return Err(ApiError::BadRequest("sort parameter is invalid".into())) };
    if !["name", "priority", "created_at", "updated_at"].contains(&field) || !["asc", "desc"].contains(&order) {
        return Err(ApiError::BadRequest("sort parameter is invalid".into()));
    }
    Ok(Some((field.to_string(), order == "asc")))
}

impl Releases {
    // ---- download clients ----

    /// Every download client, by name.
    pub fn downloaders(&self) -> ApiResult<Vec<Downloader>> {
        Ok(self.db.query("SELECT * FROM autobrr_downloaders ORDER BY name COLLATE NOCASE", p![])?.iter().map(downloader_of).collect())
    }

    /// Download client `id`.
    pub fn downloader(&self, id: i64) -> ApiResult<Downloader> {
        self.db
            .one("SELECT * FROM autobrr_downloaders WHERE id=?", p![id])?
            .map(|r| downloader_of(&r))
            .ok_or_else(|| ApiError::NotFound(format!("download client with id {id} not found")))
    }

    /// Saves a new download client (autobrr's `Store`); it, with its id.
    pub fn add_downloader(&self, mut d: Downloader) -> ApiResult<Downloader> {
        d.validate().map_err(ApiError::Internal)?;
        d.id = self.db.execute(
            "INSERT INTO autobrr_downloaders (name, kind, enabled, body) VALUES (?,?,?,?)",
            p![d.name, d.kind, d.enabled, serde_json::to_string(&d).unwrap_or_default()],
        )?;
        Ok(d)
    }

    /// Changes a download client, keeping the saved secrets where `<redacted>` came back (autobrr's `Update`).
    pub fn update_downloader(&self, mut d: Downloader) -> ApiResult<Downloader> {
        d.validate().map_err(ApiError::Internal)?;
        let saved = self.downloader(d.id)?;
        d.keep_secrets(&saved);
        self.db.execute(
            "UPDATE autobrr_downloaders SET name=?, kind=?, enabled=?, body=? WHERE id=?",
            p![d.name, d.kind, d.enabled, serde_json::to_string(&d).unwrap_or_default(), d.id],
        )?;
        Ok(d)
    }

    /// Removes download client `id`.
    pub fn remove_downloader(&self, id: i64) -> ApiResult<()> {
        self.downloader(id)?;
        self.db.execute("DELETE FROM autobrr_downloaders WHERE id=?", p![id])?;
        Ok(())
    }

    // ---- actions ----

    /// Every action (autobrr's action `List`).
    pub fn actions(&self) -> ApiResult<Vec<Action>> {
        Ok(self.db.query("SELECT * FROM autobrr_actions ORDER BY id", p![])?.iter().map(action_of).collect())
    }

    /// Filter `filter_id`'s actions (only the enabled ones when `active`), with their clients when `with_client`.
    pub fn actions_of(&self, filter_id: i64, active: bool, with_client: bool) -> ApiResult<Vec<Action>> {
        let mut list: Vec<Action> = self
            .db
            .query("SELECT * FROM autobrr_actions WHERE filter_id=? ORDER BY id", p![filter_id])?
            .iter()
            .map(action_of)
            .filter(|a| !active || a.enabled)
            .collect();
        if with_client {
            for a in &mut list {
                if a.client_id > 0 {
                    a.client = self.downloader(a.client_id).ok();
                }
            }
        }
        Ok(list)
    }

    /// Action `id`.
    pub fn action(&self, id: i64) -> ApiResult<Action> {
        self.db
            .one("SELECT * FROM autobrr_actions WHERE id=?", p![id])?
            .map(|r| action_of(&r))
            .ok_or_else(|| ApiError::NotFound(format!("action with id {id} not found")))
    }

    /// Saves a new action; it, with its id.
    pub fn add_action(&self, mut a: Action) -> ApiResult<Action> {
        a.id = self.db.execute(
            "INSERT INTO autobrr_actions (filter_id, enabled, body) VALUES (?,?,?)",
            p![a.filter_id, a.enabled, action_body(&a)],
        )?;
        Ok(a)
    }

    /// Changes action `a.id`.
    pub fn update_action(&self, a: Action) -> ApiResult<Action> {
        self.action(a.id)?;
        self.db.execute(
            "UPDATE autobrr_actions SET filter_id=?, enabled=?, body=? WHERE id=?",
            p![a.filter_id, a.enabled, action_body(&a), a.id],
        )?;
        Ok(a)
    }

    /// Removes action `id`.
    pub fn remove_action(&self, id: i64) -> ApiResult<()> {
        self.action(id)?;
        self.db.execute("DELETE FROM autobrr_actions WHERE id=?", p![id])?;
        Ok(())
    }

    /// Turns action `id` on or off, whichever it isn't.
    pub fn toggle_action(&self, id: i64) -> ApiResult<()> {
        self.action(id)?;
        self.db.execute("UPDATE autobrr_actions SET enabled = NOT enabled WHERE id=?", p![id])?;
        Ok(())
    }

    /// Saves a filter's actions (autobrr's `StoreFilterActions`): one with an id is changed, one without is
    /// added; the filter's other actions stay.
    pub fn store_filter_actions(&self, filter_id: i64, actions: Vec<Action>) -> ApiResult<Vec<Action>> {
        let mut out = Vec::new();
        for mut a in actions {
            a.filter_id = filter_id;
            if a.id > 0 {
                self.db.execute(
                    "UPDATE autobrr_actions SET filter_id=?, enabled=?, body=? WHERE id=?",
                    p![a.filter_id, a.enabled, action_body(&a), a.id],
                )?;
                out.push(a);
            } else {
                out.push(self.add_action(a)?);
            }
        }
        Ok(out)
    }

    // ---- notifications ----

    /// Adds a notification (name and type); its id. The notifications themselves come in a later slice; filters
    /// can already name them.
    pub fn add_notification(&self, name: &str, kind: &str) -> ApiResult<i64> {
        Ok(self.db.execute("INSERT INTO autobrr_notifications (name, kind, enabled, body) VALUES (?,?,1,'{}')", p![name, kind])?)
    }

    /// Whether notifications exist for every one named (autobrr's `validateNotifications`).
    fn validate_notifications(&self, list: &[FilterNotification]) -> ApiResult<()> {
        for n in list {
            if self.db.one("SELECT 1 AS y FROM autobrr_notifications WHERE id=?", p![n.notification_id])?.is_none() {
                return Err(ApiError::BadRequest(format!(
                    "notification with id {} does not exist: notification not found",
                    n.notification_id
                )));
            }
        }
        Ok(())
    }

    /// Filter `id`'s notifications.
    pub fn filter_notifications(&self, id: i64) -> ApiResult<Vec<FilterNotification>> {
        let name = self.db.one("SELECT name FROM release_filters WHERE id=?", p![id])?.map(|r| r.s("name")).unwrap_or_default();
        Ok(self
            .db
            .query("SELECT * FROM autobrr_filter_notifications WHERE filter_id=? ORDER BY notification_id", p![id])?
            .iter()
            .map(|r| FilterNotification {
                filter_id: id,
                filter_name: name.clone(),
                notification_id: r.i("notification_id"),
                events: serde_json::from_str(&r.s("events")).unwrap_or_default(),
            })
            .collect())
    }

    /// Replaces filter `id`'s notifications.
    fn store_filter_notifications(&self, id: i64, list: &[FilterNotification]) -> ApiResult<()> {
        self.validate_notifications(list)?;
        self.db.execute("DELETE FROM autobrr_filter_notifications WHERE filter_id=?", p![id])?;
        for n in list {
            self.db.execute(
                "INSERT OR REPLACE INTO autobrr_filter_notifications (filter_id, notification_id, events) VALUES (?,?,?)",
                p![id, n.notification_id, serde_json::to_string(&n.events).unwrap_or_default()],
            )?;
        }
        Ok(())
    }

    /// Changes filter `id`'s notifications (autobrr's `UpdateNotifications`).
    pub fn update_filter_notifications(&self, id: i64, list: &[FilterNotification]) -> ApiResult<()> {
        self.autobrr_filter_row(id)?;
        self.store_filter_notifications(id, list)
    }

    // ---- filters ----

    /// Filter `id` as stored; not found in autobrr's words.
    fn autobrr_filter_row(&self, id: i64) -> ApiResult<(Filter, f64)> {
        let r = self
            .db
            .one("SELECT * FROM release_filters WHERE id=?", p![id])?
            .ok_or_else(|| ApiError::NotFound(format!("filter with id {id} not found")))?;
        Ok((super::filter_of(&r), r.f("added")))
    }

    /// A release source as autobrr's `Indexer`.
    fn indexer_json(&self, id: i64) -> Value {
        match self.autobrr_indexer(id) {
            Ok(i) => json!({
                "id": i.id,
                "name": i.name,
                "identifier": i.identifier,
                "identifier_external": i.identifier_external,
                "enabled": i.enabled,
                "implementation": i.implementation,
                "use_proxy": i.use_proxy,
                "proxy": null,
                "proxy_id": i.proxy_id,
                "archived": i.archived,
            }),
            Err(_) => json!({ "id": id }),
        }
    }

    /// Whether every indexer exists (autobrr's `validateIndexers`; Lantern's release sources are its indexers).
    fn validate_indexers(&self, ids: &[i64]) -> ApiResult<()> {
        for id in ids {
            if self.source(*id).is_err() {
                return Err(ApiError::BadRequest(format!("indexer with id {id} does not exist: indexer not found")));
            }
        }
        Ok(())
    }

    /// A filter as autobrr's API sends it: autobrr's actions in place of Lantern's own, its indexers as objects,
    /// its counts, notifications and download count.
    pub fn autobrr_filter_json(&self, f: &Filter, added: f64, full: bool) -> ApiResult<Value> {
        let mut v = serde_json::to_value(f).unwrap_or_default();
        let actions = self.actions_of(f.id, false, false)?;
        let o = v.as_object_mut().ok_or_else(|| ApiError::Internal("filter isn't an object".into()))?;
        o.remove("actions");
        o.remove("skip_duplicates");
        let at = chrono::DateTime::from_timestamp(added as i64, 0).unwrap_or_default().to_rfc3339();
        o.insert("created_at".into(), json!(at));
        o.insert("updated_at".into(), json!(at));
        o.insert("actions_count".into(), json!(actions.len()));
        o.insert("actions_enabled_count".into(), json!(actions.iter().filter(|a| a.enabled).count()));
        o.insert("indexers".into(), Value::Array(f.indexers.iter().map(|id| self.indexer_json(*id)).collect()));
        o.insert("release_profile_duplicate".into(), Value::Null);
        if f.release_profile_duplicate_id == 0 {
            o.remove("release_profile_duplicate_id");
        }
        if f.external.is_empty() {
            o.remove("external");
        }
        let flt = self.with_downloads(f);
        match flt.downloads {
            Some(d) if f.is_max_downloads_limit_enabled() => {
                o.insert("downloads".into(), json!(d));
            }
            _ => {
                o.remove("downloads");
            }
        }
        if full {
            if !actions.is_empty() {
                let list: Vec<Value> = actions.iter().map(|a| serde_json::to_value(a).unwrap_or_default()).collect();
                o.insert("actions".into(), Value::Array(list));
            }
            let n = self.filter_notifications(f.id)?;
            if !n.is_empty() {
                o.insert("notifications".into(), serde_json::to_value(n).unwrap_or_default());
            }
        }
        Ok(v)
    }

    /// Every filter, as autobrr's list sends them (autobrr's `Find`): sorted as asked (priority then name by
    /// default), only those on one of `indexers` (identifiers) when given.
    pub fn autobrr_filters(&self, sort: Option<(String, bool)>, indexers: &[String]) -> ApiResult<Vec<Value>> {
        let rows = self.db.query("SELECT * FROM release_filters", p![])?;
        let mut list: Vec<(Filter, f64)> = rows.iter().map(|r| (super::filter_of(r), r.f("added"))).collect();
        match sort {
            Some((field, asc)) => list.sort_by(|a, b| {
                let o = match field.as_str() {
                    "name" => a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()),
                    "priority" => a.0.priority.cmp(&b.0.priority),
                    _ => a.1.total_cmp(&b.1),
                };
                if asc {
                    o
                } else {
                    o.reverse()
                }
            }),
            None => list.sort_by(|a, b| b.0.priority.cmp(&a.0.priority).then(a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()))),
        }
        // a filter a list keeps up to date (autobrr's `is_auto_updated`, only in this list)
        let auto: std::collections::HashSet<i64> =
            self.db.query("SELECT DISTINCT filter_id FROM autobrr_list_filters", p![])?.iter().map(|r| r.i("filter_id")).collect();
        let mut out = Vec::new();
        for (f, added) in &list {
            let mut v = self.autobrr_filter_json(f, *added, false)?;
            v["is_auto_updated"] = json!(auto.contains(&f.id));
            if !indexers.is_empty() {
                let ids: Vec<String> =
                    v["indexers"].as_array().into_iter().flatten().filter_map(|i| i["identifier"].as_str().map(str::to_string)).collect();
                if !indexers.iter().any(|i| ids.contains(i)) {
                    continue;
                }
            }
            out.push(v);
        }
        Ok(out)
    }

    /// Filter `id` with everything (autobrr's `FindByID`).
    pub fn autobrr_filter(&self, id: i64) -> ApiResult<Value> {
        let (f, added) = self.autobrr_filter_row(id)?;
        self.autobrr_filter_json(&f, added, true)
    }

    /// A filter from `body` over `base`, and the actions, external filters and notifications it sent (`None`
    /// when left out).
    fn autobrr_filter_from(body: &Value, base: Filter) -> ApiResult<FilterParts> {
        let mut body = body.clone();
        let o =
            body.as_object_mut().ok_or_else(|| ApiError::Internal("json: cannot unmarshal into Go value of type domain.Filter".into()))?;
        let actions = match o.remove("actions") {
            Some(Value::Null) | None => None,
            Some(v) => Some(serde_json::from_value::<Vec<Action>>(v).map_err(|e| ApiError::Internal(e.to_string()))?),
        };
        let notifications = match o.remove("notifications") {
            Some(Value::Null) | None => None,
            Some(v) => Some(serde_json::from_value::<Vec<FilterNotification>>(v).map_err(|e| ApiError::Internal(e.to_string()))?),
        };
        for k in ["id", "created_at", "updated_at", "actions_count", "actions_enabled_count", "downloads", "release_profile_duplicate"] {
            o.remove(k);
        }
        let mut merged = serde_json::to_value(&base).unwrap_or_default();
        if let (Value::Object(b), Value::Object(n)) = (&mut merged, filter::upgrade(body)) {
            for (k, v) in n {
                b.insert(k, v);
            }
        }
        let mut f: Filter = serde_json::from_value(merged).map_err(|e| ApiError::Internal(e.to_string()))?;
        f.id = base.id;
        f.actions = base.actions;
        if let Some(list) = &actions {
            for a in list.iter().filter(|a| a.kind == "EXEC" && a.enabled && !a.exec_cmd.is_empty()) {
                super::run::look_path(&a.exec_cmd)
                    .map_err(|e| ApiError::Internal(format!("could not find action exec command: {}: {e}", a.exec_cmd)))?;
            }
        }
        Ok((f, actions, notifications))
    }

    /// Writes filter `f`'s row.
    fn write_filter(&self, f: &Filter) -> ApiResult<()> {
        self.db.execute(
            "UPDATE release_filters SET name=?, enabled=?, priority=?, body=? WHERE id=?",
            p![f.name.trim(), f.enabled, f.priority, serde_json::to_string(f).unwrap_or_default(), f.id],
        )?;
        Ok(())
    }

    /// Saves a new filter (autobrr's `Store`): validated, `NEW` announces when none are named; it, with its id.
    pub fn autobrr_store_filter(&self, body: &Value) -> ApiResult<Value> {
        let (mut f, _, _) = Self::autobrr_filter_from(body, Filter::default())?;
        f.validate().map_err(ApiError::Internal)?;
        if f.announce_types.is_empty() {
            f.announce_types = vec!["NEW".into()];
        }
        self.validate_indexers(&f.indexers)?;
        let now = crate::auth::now();
        f.id = self.db.execute(
            "INSERT INTO release_filters (name, enabled, priority, body, added) VALUES (?,?,?,?,?)",
            p![f.name.trim(), f.enabled, f.priority, serde_json::to_string(&f).unwrap_or_default(), now],
        )?;
        self.autobrr_filter_json(&f, now, false)
    }

    /// Replaces a filter (autobrr's `Update`): its actions, external filters and notifications too; the filter as
    /// saved.
    pub fn autobrr_update_filter(&self, body: &Value) -> ApiResult<Value> {
        let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
        let mut base = Filter { id, ..Filter::default() };
        let (cur, added) = self.autobrr_filter_row(id).map_err(|_| ApiError::NotFound(format!("filter with id {id} not found")))?;
        base.actions = cur.actions.clone();
        base.skip_duplicates = cur.skip_duplicates;
        let (mut f, actions, notifications) = Self::autobrr_filter_from(body, base)?;
        f.validate().map_err(ApiError::Internal)?;
        f.sanitize();
        self.validate_indexers(&f.indexers)?;
        let notifications = notifications.unwrap_or_default();
        self.validate_notifications(&notifications)?;
        self.write_filter(&f)?;
        self.store_filter_actions(f.id, actions.unwrap_or_default())?;
        self.store_filter_notifications(f.id, &notifications)?;
        self.autobrr_filter_json(&f, added, true)
    }

    /// Changes the fields `body` has (autobrr's `UpdatePartial`).
    pub fn autobrr_update_partial(&self, id: i64, body: &Value) -> ApiResult<()> {
        let (cur, _) = self.autobrr_filter_row(id)?;
        let mut body = body.clone();
        if let Some(Value::String(s)) = body.get("shows") {
            let clean = s.replace('\n', ",").replace(",,", ",");
            body["shows"] = json!(clean);
        }
        let (f, actions, notifications) = Self::autobrr_filter_from(&body, cur)?;
        self.validate_indexers(&f.indexers)?;
        if let Some(n) = &notifications {
            self.validate_notifications(n)?;
        }
        self.write_filter(&f)?;
        if let Some(a) = actions {
            self.store_filter_actions(id, a)?;
        }
        if let Some(n) = notifications {
            self.store_filter_notifications(id, &n)?;
        }
        Ok(())
    }

    /// Copies filter `id` as "<name> Copy", off (autobrr's `Duplicate`); the copy.
    pub fn autobrr_duplicate_filter(&self, id: i64) -> ApiResult<Value> {
        let (mut f, _) = self.autobrr_filter_row(id)?;
        let actions = self.actions_of(id, false, false)?;
        let notifications = self.filter_notifications(id)?;
        f.id = 0;
        f.name = format!("{} Copy", f.name);
        f.enabled = false;
        self.validate_indexers(&f.indexers)?;
        let now = crate::auth::now();
        f.id = self.db.execute(
            "INSERT INTO release_filters (name, enabled, priority, body, added) VALUES (?,?,?,?,?)",
            p![f.name.trim(), f.enabled, f.priority, serde_json::to_string(&f).unwrap_or_default(), now],
        )?;
        let copies = actions.into_iter().map(|a| Action { id: 0, ..a }).collect();
        self.store_filter_actions(f.id, copies)?;
        self.store_filter_notifications(f.id, &notifications)?;
        self.autobrr_filter_json(&f, now, true)
    }

    /// Turns filter `id` on or off.
    pub fn autobrr_toggle_filter(&self, id: i64, enabled: bool) -> ApiResult<()> {
        self.autobrr_filter_row(id)?;
        self.db.execute("UPDATE release_filters SET enabled=? WHERE id=?", p![enabled, id])?;
        Ok(())
    }

    /// Removes filter `id` with its actions and notifications (autobrr's `Delete`).
    pub fn autobrr_delete_filter(&self, id: i64) -> ApiResult<()> {
        if id == 0 {
            return Ok(());
        }
        self.autobrr_filter_row(id)?;
        self.db.execute("DELETE FROM autobrr_actions WHERE filter_id=?", p![id])?;
        self.db.execute("DELETE FROM release_filters WHERE id=?", p![id])?;
        self.db.execute("DELETE FROM autobrr_filter_notifications WHERE filter_id=?", p![id])?;
        self.db.execute("DELETE FROM autobrr_list_filters WHERE filter_id=?", p![id])?;
        Ok(())
    }

    /// Removes filters' links to archived indexers (only those with one of `identifiers`, when given); how many
    /// links were removed (autobrr's `PruneDeprecatedIndexers`).
    pub fn prune_deprecated_indexers(&self, identifiers: &[String]) -> i64 {
        let archived: Vec<i64> = self
            .autobrr_indexers()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.archived && (identifiers.is_empty() || identifiers.contains(&i.identifier)))
            .map(|i| i.id)
            .collect();
        if archived.is_empty() {
            return 0;
        }
        let mut removed = 0;
        for mut f in self.filters().unwrap_or_default() {
            let before = f.indexers.len();
            f.indexers.retain(|i| !archived.contains(i));
            if f.indexers.len() != before {
                removed += (before - f.indexers.len()) as i64;
                let _ = self
                    .db
                    .execute("UPDATE release_filters SET body=? WHERE id=?", p![serde_json::to_string(&f).unwrap_or_default(), f.id]);
            }
        }
        removed
    }

    /// The external filters of `f`, as stored on it.
    pub fn externals(f: &Filter) -> Vec<FilterExternal> {
        let mut list = f.external.clone();
        list.sort_by_key(|e| e.index);
        for e in &mut list {
            e.filter_id = f.id;
        }
        list
    }
}
