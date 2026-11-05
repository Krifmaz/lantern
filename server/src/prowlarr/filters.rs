// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/CustomFilters/CustomFilterController.cs,
// CustomFilterResource.cs, src/NzbDrone.Core/CustomFilters/CustomFilterService.cs), GPL-3.0.

//! The filters people save in Prowlarr's web app (on the indexer, search and history lists): `/api/v1/customfilter`.

use super::{accepted, body_json, created, fail, not_found, validate_id, validation, ApiError, ApiResult, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use kit_server_core::{p, Db, RowExt, SqliteDb};
use serde_json::{json, Value};

/// The custom filter routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/customfilter", get(all).post(create))
        .route("/api/v1/customfilter/{id}", get(one).put(update).delete(remove))
}

/// A saved filter as Prowlarr's `CustomFilterResource` (its `filters` kept as the JSON they were saved as).
fn filter_json(r: &kit_server_core::Row) -> Value {
    let mut v = json!({ "id": r.i("id"), "type": r.s("type"), "label": r.s("label") });
    // no filters (null) is left out, as Prowlarr's JSON writer leaves out every null
    if let Ok(filters) = serde_json::from_str::<Value>(&r.s("filters")) {
        if !filters.is_null() {
            v["filters"] = filters;
        }
    }
    v
}

/// The filter with `id`, or Prowlarr's 404.
fn get_filter(db: &SqliteDb, id: i64) -> ApiResult<Value> {
    db.one("SELECT * FROM prowlarr_custom_filters WHERE id=?", p![id])?
        .map(|r| filter_json(&r))
        .ok_or_else(|| not_found("CustomFilter", id))
}

/// The `type`, `label` and `filters` of a request; Prowlarr's 409 for a missing type or label (its table refuses
/// them).
fn fields(body: &Value) -> ApiResult<(String, String, String)> {
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
    let type_ = text("type").ok_or_else(|| fail(409, "constraint failed\r\nNOT NULL constraint failed: CustomFilters.Type"))?;
    let label = text("label").ok_or_else(|| fail(409, "constraint failed\r\nNOT NULL constraint failed: CustomFilters.Label"))?;
    let filters = body.get("filters").cloned().unwrap_or(Value::Null);
    Ok((type_, label, filters.to_string()))
}

/// `GET /api/v1/customfilter`.
async fn all(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(Value::Array(st.db.query("SELECT * FROM prowlarr_custom_filters ORDER BY id", p![])?.iter().map(filter_json).collect())))
}

/// `GET /api/v1/customfilter/{id}`.
async fn one(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(get_filter(&st.db, id)?))
}

/// `POST /api/v1/customfilter`: saves a filter; 201 with it.
async fn create(State(st): State<AppState>, _: V1, bytes: Bytes) -> ApiResult<Response> {
    let (type_, label, filters) = fields(&body_json(&bytes)?)?;
    let id = st.db.execute("INSERT INTO prowlarr_custom_filters (type, label, filters) VALUES (?,?,?)", p![type_, label, filters])?;
    Ok(created(format!("/api/v1/customfilter/{id}"), get_filter(&st.db, id)?))
}

/// `PUT /api/v1/customfilter/{id}`: changes a filter; 202 with it.
async fn update(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, bytes: Bytes) -> ApiResult<Response> {
    validate_id(path_id)?;
    let body = body_json(&bytes)?;
    let id = match body.get("id").and_then(Value::as_i64).unwrap_or(0) {
        0 => path_id,
        id => id,
    };
    if let Some(f) = validation::valid_id(id) {
        return Err(ApiError::Invalid(vec![f]));
    }
    let (type_, label, filters) = fields(&body)?;
    get_filter(&st.db, id)?;
    st.db.execute("UPDATE prowlarr_custom_filters SET type=?, label=?, filters=? WHERE id=?", p![type_, label, filters, id])?;
    Ok(accepted(format!("/api/v1/customfilter/{id}"), get_filter(&st.db, id)?))
}

/// `DELETE /api/v1/customfilter/{id}`: `{}`, as Prowlarr answers.
async fn remove(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    validate_id(id)?;
    st.db.execute("DELETE FROM prowlarr_custom_filters WHERE id=?", p![id])?;
    Ok(Json(json!({})))
}
