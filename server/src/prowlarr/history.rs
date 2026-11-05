// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/History/HistoryController.cs, HistoryResource.cs,
// src/NzbDrone.Core/History/History.cs, HistoryService.cs, HistoryRepository.cs,
// src/Prowlarr.Http/PagingResource.cs), GPL-3.0.

//! The indexer history as Prowlarr's API gives it:
//!
//! ```text
//! GET /api/v1/history?page=&pageSize=&sortKey=date&sortDirection=&eventType=&successful=&downloadId=&indexerIds=
//! GET /api/v1/history/since?date=&eventType=      everything since a time, oldest first
//! GET /api/v1/history/indexer?indexerId=&eventType=&limit=   one indexer's, newest first
//! ```
//!
//! Lantern notes each search, newest-releases check (RSS) and grab in `indexer_history` (with Prowlarr's details
//! in `data`); here they are Prowlarr's events: `releaseGrabbed` (1), `indexerQuery` (2), `indexerRss` (3) and
//! `indexerAuth` (4). Lantern's own indexer tests are not history in Prowlarr and are left out.

use super::{date, ApiResult, V1};
use crate::AppState;
use axum::extract::{RawQuery, State};
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Map, Value};

/// The history routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/history", get(paged))
        .route("/api/v1/history/since", get(since))
        .route("/api/v1/history/indexer", get(of_indexer))
}

/// Prowlarr's `HistoryEventType` of a Lantern history kind: (number, name).
pub fn event_type(kind: &str) -> (i64, &'static str) {
    match kind {
        "grab" => (1, "releaseGrabbed"),
        "query" => (2, "indexerQuery"),
        "rss" => (3, "indexerRss"),
        "auth" => (4, "indexerAuth"),
        "info" => (5, "indexerInfo"),
        _ => (0, "unknown"),
    }
}

/// The Lantern kind of an event type given by number or name (`2`, `indexerQuery`, any case).
fn kind_of(given: &str) -> Option<&'static str> {
    let g = given.trim().to_lowercase();
    ["grab", "query", "rss", "auth", "info"].into_iter().find(|k| {
        let (n, name) = event_type(k);
        g == n.to_string() || g == name.to_lowercase()
    })
}

/// The Lantern kinds that are Prowlarr history.
const KINDS: &str = "'grab','query','rss','auth','info'";

/// One history event.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Its id.
    pub id: i64,
    /// The indexer it is about.
    pub indexer_id: i64,
    /// When (seconds since 1970).
    pub at: f64,
    /// Lantern's kind: `query`, `rss`, `grab`, `auth`.
    pub kind: String,
    /// Whether it worked.
    pub ok: bool,
    /// How long it took, in milliseconds.
    pub ms: i64,
    /// How many releases it found.
    pub results: i64,
    /// The words searched for (`(newest)` for a check of the newest releases).
    pub query: String,
    /// Prowlarr's details (`Source`, `Host`, `GrabTitle` …), as saved.
    pub data: Map<String, Value>,
}

impl Entry {
    /// The event of row `r`.
    fn of(r: &kit_server_core::Row) -> Entry {
        Entry {
            id: r.i("id"),
            indexer_id: r.i("indexer_id"),
            at: r.f("at"),
            kind: r.s("kind"),
            ok: r.b("ok"),
            ms: r.i("ms"),
            results: r.i("results"),
            query: r.s("query"),
            data: match serde_json::from_str::<Value>(&r.s("data")) {
                Ok(Value::Object(m)) => m,
                _ => Map::new(),
            },
        }
    }

    /// Its details as Prowlarr keeps them (`History.Data`): what was saved, with the answer time, the words, the
    /// number of results and the cache flag of a search, keys in camelCase as Prowlarr writes them.
    pub fn data_json(&self) -> Map<String, Value> {
        let mut d: Map<String, Value> = Map::new();
        let mut put = |k: &str, v: Value| {
            let mut c = k.chars();
            let key = c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default();
            let text = match v {
                Value::String(s) => s,
                Value::Null => return,
                other => other.to_string(),
            };
            d.insert(key, json!(text));
        };
        for (k, v) in &self.data {
            put(k, v.clone());
        }
        let has = |k: &str| self.data.keys().any(|x| x.eq_ignore_ascii_case(k));
        match self.kind.as_str() {
            "query" | "rss" => {
                if !has("ElapsedTime") {
                    put("ElapsedTime", json!(self.ms.to_string()));
                }
                if !has("Query") {
                    put("Query", json!(if self.query == "(newest)" { "" } else { self.query.as_str() }));
                }
                for k in ["QueryType", "Categories", "Source", "Host"] {
                    if !has(k) {
                        put(k, json!(if k == "QueryType" { "search" } else { "" }));
                    }
                }
                put("QueryResults", json!(self.results.to_string()));
                if !has("Url") {
                    put("Url", json!(""));
                }
                if !has("Cached") {
                    put("Cached", json!("0"));
                }
            }
            "grab" => {
                for k in ["Source", "Host", "Url"] {
                    if !has(k) {
                        put(k, json!(""));
                    }
                }
                if !has("GrabMethod") {
                    put("GrabMethod", json!("Proxy"));
                }
                if !has("GrabTitle") {
                    put("GrabTitle", json!(self.query));
                }
                if self.ms > 0 && !has("ElapsedTime") {
                    put("ElapsedTime", json!(self.ms.to_string()));
                }
            }
            _ => {
                if !has("ElapsedTime") {
                    put("ElapsedTime", json!(self.ms.to_string()));
                }
            }
        }
        d
    }

    /// The event as Prowlarr's `HistoryResource`.
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "indexerId": self.indexer_id,
            "date": date(self.at),
            "successful": self.ok,
            "eventType": event_type(&self.kind).1,
            "data": self.data_json(),
        })
    }
}

/// The history between two times, oldest first (`HistoryRepository.Between`).
pub fn between(db: &SqliteDb, start: f64, end: f64) -> kit_server_core::Result<Vec<Entry>> {
    let rows =
        db.query(&format!("SELECT * FROM indexer_history WHERE at>=? AND at<=? AND kind IN ({KINDS}) ORDER BY at, id"), p![start, end])?;
    Ok(rows.iter().map(Entry::of).collect())
}

/// A query's values: every `name=value` pair, a name that repeats keeping each value.
fn pairs(raw: Option<String>) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

/// The first value of `name` (any case), when it has one.
fn first<'a>(q: &'a [(String, String)], name: &str) -> Option<&'a str> {
    q.iter().find(|(k, v)| k.eq_ignore_ascii_case(name) && !v.trim().is_empty()).map(|(_, v)| v.trim())
}

/// Every value of `name` (`indexerIds=1&indexerIds=2`, `indexerIds=1,2`).
fn all_of(q: &[(String, String)], name: &str) -> Vec<String> {
    q.iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .flat_map(|(_, v)| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>())
        .collect()
}

/// `GET /api/v1/history`: one page of the history, newest first unless asked otherwise (only `date` can be sorted
/// on), narrowed by event types, success and indexers (`PagingResource<HistoryResource>`).
async fn paged(State(st): State<AppState>, _: V1, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let q = pairs(raw);
    let page: i64 = first(&q, "page").and_then(|v| v.parse().ok()).unwrap_or(1);
    let page_size: i64 = first(&q, "pageSize").and_then(|v| v.parse().ok()).unwrap_or(10);
    let asked = match first(&q, "sortDirection").map(str::to_lowercase).as_deref() {
        Some("ascending") | Some("1") => "ascending",
        Some("default") | Some("0") | None | Some("descending") | Some("2") => "descending",
        Some(_) => "descending",
    };
    let mut wheres = vec![format!("kind IN ({KINDS})")];
    let mut params: Vec<Value> = Vec::new();
    let types: Vec<&str> =
        all_of(&q, "eventType").iter().filter_map(|t| t.parse::<i64>().ok()).filter_map(|n| kind_of(&n.to_string())).collect();
    if !all_of(&q, "eventType").is_empty() {
        if types.is_empty() {
            wheres.push("0".into());
        } else {
            wheres.push(format!("kind IN ({})", vec!["?"; types.len()].join(",")));
            params.extend(types.iter().map(|t| json!(t)));
        }
    }
    if let Some(s) = first(&q, "successful") {
        wheres.push("ok=?".into());
        params.push(json!(s.eq_ignore_ascii_case("true")));
    }
    if first(&q, "downloadId").is_some() {
        // Lantern doesn't keep download ids: nothing has the one asked for
        wheres.push("0".into());
    }
    let ids: Vec<i64> = all_of(&q, "indexerIds").iter().filter_map(|x| x.parse().ok()).collect();
    if !ids.is_empty() {
        wheres.push(format!("indexer_id IN ({})", vec!["?"; ids.len()].join(",")));
        params.extend(ids.iter().map(|i| json!(i)));
    }
    let where_sql = wheres.join(" AND ");
    let total =
        st.db.one(&format!("SELECT COUNT(*) AS n FROM indexer_history WHERE {where_sql}"), params.clone())?.map(|r| r.i("n")).unwrap_or(0);
    let order = if asked == "ascending" { "ASC" } else { "DESC" };
    let mut page_params = params;
    page_params.push(json!(page_size.max(0)));
    page_params.push(json!(((page - 1) * page_size).max(0)));
    let rows = st
        .db
        .query(&format!("SELECT * FROM indexer_history WHERE {where_sql} ORDER BY at {order}, id {order} LIMIT ? OFFSET ?"), page_params)?;
    Ok(Json(json!({
        "page": page,
        "pageSize": page_size,
        "sortKey": "date",
        "sortDirection": asked,
        "totalRecords": total,
        "records": rows.iter().map(|r| Entry::of(r).json()).collect::<Vec<_>>(),
    })))
}

/// `GET /api/v1/history/since?date=&eventType=`: everything since a time (all without one), oldest first.
async fn since(State(st): State<AppState>, _: V1, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let q = pairs(raw);
    let from = first(&q, "date").and_then(super::indexers::query_time).unwrap_or(f64::MIN);
    let kind = first(&q, "eventType").map(kind_of);
    let list: Vec<Value> = between(&st.db, from, f64::MAX)?
        .into_iter()
        .filter(|e| match kind {
            Some(k) => Some(e.kind.as_str()) == k,
            None => true,
        })
        .map(|e| e.json())
        .collect();
    Ok(Json(Value::Array(list)))
}

/// `GET /api/v1/history/indexer?indexerId=&eventType=&limit=`: one indexer's history, newest first.
async fn of_indexer(State(st): State<AppState>, _: V1, RawQuery(raw): RawQuery) -> ApiResult<Json<Value>> {
    let q = pairs(raw);
    let id: i64 = first(&q, "indexerId").and_then(|v| v.parse().ok()).unwrap_or(0);
    let kind = first(&q, "eventType").map(kind_of);
    let limit = first(&q, "limit").and_then(|v| v.parse::<usize>().ok());
    let rows = st
        .db
        .query(&format!("SELECT * FROM indexer_history WHERE indexer_id=? AND kind IN ({KINDS}) ORDER BY at DESC, id DESC"), p![id])?;
    let mut list: Vec<Value> = rows
        .iter()
        .map(Entry::of)
        .filter(|e| match kind {
            Some(k) => Some(e.kind.as_str()) == k,
            None => true,
        })
        .map(|e| e.json())
        .collect();
    if let Some(l) = limit {
        list.truncate(l);
    }
    Ok(Json(Value::Array(list)))
}

/// Deletes the history older than `days` days (`HistoryRepository.Cleanup`); 0 keeps everything.
pub fn clean_up(db: &SqliteDb, days: i64) -> kit_server_core::Result<()> {
    if days == 0 {
        return Ok(());
    }
    let cutoff = crate::auth::now() - days as f64 * 86400.0;
    db.execute("DELETE FROM indexer_history WHERE at<=?", p![cutoff])?;
    Ok(())
}

/// Deletes all the history (`ClearHistoryCommand`).
pub fn clear(db: &SqliteDb) -> kit_server_core::Result<()> {
    db.execute("DELETE FROM indexer_history", p![])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_follow_prowlarr() {
        assert_eq!(event_type("grab"), (1, "releaseGrabbed"));
        assert_eq!(event_type("query"), (2, "indexerQuery"));
        assert_eq!(event_type("rss"), (3, "indexerRss"));
        assert_eq!(event_type("auth"), (4, "indexerAuth"));
        assert_eq!(kind_of("2"), Some("query"));
        assert_eq!(kind_of("IndexerRss"), Some("rss"));
        assert_eq!(kind_of("9"), None);
    }

    #[test]
    fn details_are_camel_case_text() {
        let e = Entry {
            id: 1,
            indexer_id: 2,
            at: 1_793_788_200.0,
            kind: "query".into(),
            ok: true,
            ms: 120,
            results: 4,
            query: "Show S01E02".into(),
            data: serde_json::from_value(json!({ "QueryType": "tvsearch", "Query": "Show", "Season": "1", "Limit": "100", "Source": "Sonarr", "Host": "h", "Categories": "5000" })).unwrap(),
        };
        let v = e.json();
        assert_eq!(v["eventType"], "indexerQuery");
        assert_eq!(v["date"], "2026-11-04T10:30:00Z");
        assert_eq!(
            v["data"],
            json!({
                "queryType": "tvsearch", "query": "Show", "season": "1", "limit": "100", "source": "Sonarr", "host": "h",
                "categories": "5000", "elapsedTime": "120", "queryResults": "4", "url": "", "cached": "0"
            })
        );
        let g = Entry { kind: "grab".into(), ms: 0, data: Map::new(), query: "Title".into(), ..e };
        assert_eq!(g.json()["data"], json!({ "source": "", "host": "", "url": "", "grabMethod": "Proxy", "grabTitle": "Title" }));
    }
}
