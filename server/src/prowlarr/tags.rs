// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/Tags/TagController.cs, TagDetailsController.cs,
// TagResource.cs, TagDetailsResource.cs, src/NzbDrone.Core/Tags/TagService.cs, TagRepository.cs), GPL-3.0.

//! Tags (`/api/v1/tag`) and what uses each one (`/api/v1/tag/detail`).
//!
//! Lantern's indexers keep their tags by name (`indexers.tags`, comma separated); Prowlarr's tags have numbers.
//! Every name an indexer uses is a tag here too (made when it is first listed), and renaming a tag renames it on
//! the indexers that use it. Indexer proxies and applications keep tag ids, as Prowlarr does.

use super::{body_json, check, created, fail, not_found, validate_id, validation, ApiResult, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Value};

/// The tag routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/tag", get(all).post(create))
        .route("/api/v1/tag/{id}", get(one).put(update).delete(remove))
        .route("/api/v1/tag/detail", get(details))
        .route("/api/v1/tag/detail/{id}", get(detail))
}

/// The tag names in an indexer's `tags` text.
pub fn labels_of(tags: &str) -> Vec<String> {
    tags.split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect()
}

/// Makes a tag for every name an indexer uses that has none yet.
fn sync(db: &SqliteDb) -> kit_server_core::Result<()> {
    for r in db.query("SELECT tags FROM indexers WHERE tags<>''", p![])? {
        for label in labels_of(&r.s("tags")) {
            db.execute("INSERT OR IGNORE INTO prowlarr_tags (label) VALUES (?)", p![label])?;
        }
    }
    Ok(())
}

/// Every tag, by label.
pub fn list(db: &SqliteDb) -> kit_server_core::Result<Vec<(i64, String)>> {
    sync(db)?;
    let mut tags: Vec<(i64, String)> =
        db.query("SELECT id, label FROM prowlarr_tags", p![])?.iter().map(|r| (r.i("id"), r.s("label"))).collect();
    tags.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(tags)
}

/// The tag with `id`, or Prowlarr's 404.
fn get_tag(db: &SqliteDb, id: i64) -> ApiResult<(i64, String)> {
    sync(db)?;
    db.one("SELECT id, label FROM prowlarr_tags WHERE id=?", p![id])?.map(|r| (r.i("id"), r.s("label"))).ok_or_else(|| not_found("Tag", id))
}

/// A tag as Prowlarr's `TagResource`.
fn tag_json((id, label): &(i64, String)) -> Value {
    json!({ "id": id, "label": label })
}

/// The ids of the indexers that use `label`.
fn indexers_with(db: &SqliteDb, label: &str) -> kit_server_core::Result<Vec<i64>> {
    Ok(db
        .query("SELECT id, tags FROM indexers ORDER BY id", p![])?
        .iter()
        .filter(|r| labels_of(&r.s("tags")).iter().any(|t| t == label))
        .map(|r| r.i("id"))
        .collect())
}

/// The ids of the providers in `table` (tags kept as a JSON list of tag ids) that have tag `id`; none when the
/// table isn't there.
fn providers_with(db: &SqliteDb, table: &str, id: i64) -> Vec<i64> {
    db.query(&format!("SELECT id, tags FROM {table} ORDER BY id"), p![])
        .map(|rows| {
            rows.iter()
                .filter(|r| serde_json::from_str::<Vec<i64>>(&r.s("tags")).unwrap_or_default().contains(&id))
                .map(|r| r.i("id"))
                .collect()
        })
        .unwrap_or_default()
}

/// A tag as Prowlarr's `TagDetailsResource`: the indexers, indexer proxies and applications that use it (Lantern
/// has no notifications, so that list is empty).
fn details_json(db: &SqliteDb, tag: &(i64, String)) -> kit_server_core::Result<Value> {
    Ok(json!({
        "id": tag.0,
        "label": tag.1,
        "notificationIds": [],
        "indexerIds": indexers_with(db, &tag.1)?,
        "indexerProxyIds": providers_with(db, "indexer_proxies", tag.0),
        "applicationIds": providers_with(db, "prowlarr_applications", tag.0),
    }))
}

/// Whether anything uses the tag (`TagDetails.InUse`).
fn in_use(db: &SqliteDb, tag: &(i64, String)) -> kit_server_core::Result<bool> {
    let d = details_json(db, tag)?;
    Ok(["notificationIds", "indexerIds", "indexerProxyIds", "applicationIds"]
        .iter()
        .any(|k| d[*k].as_array().is_some_and(|a| !a.is_empty())))
}

/// Prowlarr's checks of a tag's label: set, and only `a-z`, `0-9` and `-`.
fn check_label(label: Option<&str>) -> Option<validation::Failure> {
    if let Some(f) = validation::not_empty("Label", label) {
        return Some(f);
    }
    let ok = label.is_some_and(|l| l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    (!ok).then(|| {
        validation::Failure::new("Label", json!(label), "RegularExpressionValidator", "Allowed characters a-z, 0-9 and -")
            .with("RegularExpression", json!("^[a-z0-9-]+$"))
    })
}

/// The label of a tag request's body.
fn label_of(body: &Value) -> Option<String> {
    body.get("label").and_then(Value::as_str).map(str::to_string)
}

/// `GET /api/v1/tag`: every tag, by label.
async fn all(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(Value::Array(list(&st.db)?.iter().map(tag_json).collect())))
}

/// `GET /api/v1/tag/{id}`.
async fn one(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(tag_json(&get_tag(&st.db, id)?)))
}

/// `POST /api/v1/tag`: makes a tag (its label in small letters); one with exactly that label already is answered
/// instead. 201 with the tag.
async fn create(State(st): State<AppState>, _: V1, bytes: Bytes) -> ApiResult<Response> {
    let body = body_json(&bytes)?;
    let label = label_of(&body);
    check(vec![check_label(label.as_deref())])?;
    let label = label.unwrap_or_default();
    sync(&st.db)?;
    let id = match st.db.one("SELECT id FROM prowlarr_tags WHERE label=?", p![label])? {
        Some(r) => r.i("id"),
        None => {
            let lower = label.to_lowercase();
            if st.db.one("SELECT id FROM prowlarr_tags WHERE label=?", p![lower])?.is_some() {
                return Err(fail(409, "constraint failed\r\nUNIQUE constraint failed: Tags.Label"));
            }
            st.db.execute("INSERT INTO prowlarr_tags (label) VALUES (?)", p![lower])?
        }
    };
    Ok(created(format!("/api/v1/tag/{id}"), tag_json(&get_tag(&st.db, id)?)))
}

/// `PUT /api/v1/tag/{id}`: renames a tag (in small letters), on the indexers that use it too. 202 with the tag.
async fn update(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, bytes: Bytes) -> ApiResult<Response> {
    validate_id(path_id)?;
    let body = body_json(&bytes)?;
    let id = match body.get("id").and_then(Value::as_i64).unwrap_or(0) {
        0 => path_id,
        id => id,
    };
    let label = label_of(&body);
    check(vec![check_label(label.as_deref()), validation::valid_id(id)])?;
    let (_, old) = get_tag(&st.db, id)?;
    let new = label.unwrap_or_default().to_lowercase();
    if new != old {
        if st.db.one("SELECT id FROM prowlarr_tags WHERE label=? AND id<>?", p![new, id])?.is_some() {
            return Err(fail(409, "constraint failed\r\nUNIQUE constraint failed: Tags.Label"));
        }
        st.db.execute("UPDATE prowlarr_tags SET label=? WHERE id=?", p![new, id])?;
        for r in st.db.query("SELECT id, tags FROM indexers WHERE tags<>''", p![])? {
            let labels = labels_of(&r.s("tags"));
            if labels.contains(&old) {
                let renamed: Vec<String> = labels.into_iter().map(|t| if t == old { new.clone() } else { t }).collect();
                st.db.execute("UPDATE indexers SET tags=? WHERE id=?", p![renamed.join(","), r.i("id")])?;
            }
        }
    }
    Ok(super::accepted(format!("/api/v1/tag/{id}"), tag_json(&get_tag(&st.db, id)?)))
}

/// `DELETE /api/v1/tag/{id}`: removes a tag no one uses; 409 for one in use, as Prowlarr answers.
async fn remove(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Response> {
    validate_id(id)?;
    let tag = get_tag(&st.db, id)?;
    if in_use(&st.db, &tag)? {
        return Err(fail(409, format!("Tag with ID {id} '{}' cannot be deleted since it's still in use", tag.1)));
    }
    st.db.execute("DELETE FROM prowlarr_tags WHERE id=?", p![id])?;
    // Prowlarr's DeleteTag returns nothing: 200 with an empty body
    Ok(axum::http::StatusCode::OK.into_response())
}

/// `GET /api/v1/tag/detail`: every tag with the ids of what uses it.
async fn details(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    let list: kit_server_core::Result<Vec<Value>> = list(&st.db)?.iter().map(|t| details_json(&st.db, t)).collect();
    Ok(Json(Value::Array(list?)))
}

/// `GET /api/v1/tag/detail/{id}`.
async fn detail(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let tag = get_tag(&st.db, id)?;
    Ok(Json(details_json(&st.db, &tag)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_prowlarr() {
        assert!(check_label(Some("movies-4k")).is_none());
        assert_eq!(check_label(Some("")).unwrap().message, "'Label' must not be empty.");
        assert_eq!(check_label(None).unwrap().message, "'Label' must not be empty.");
        assert_eq!(check_label(Some("tv shows")).unwrap().message, "Allowed characters a-z, 0-9 and -");
        assert_eq!(labels_of(" Movies, tv ,,"), ["movies", "tv"]);
    }
}
