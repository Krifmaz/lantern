// Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Api.V1/IndexerProxies/IndexerProxyController.cs,
// src/Prowlarr.Api.V1/ProviderControllerBase.cs, src/Prowlarr.Api.V1/ProviderResource.cs,
// src/Prowlarr.Api.V1/ProviderTestAllResult.cs, src/Prowlarr.Http/REST/RestController.cs), GPL-3.0.

//! Prowlarr's indexer proxy API, as its web app and tools use it:
//!
//! ```text
//! GET    /api/v1/indexerproxy               every proxy, by name
//! POST   /api/v1/indexerproxy[?forceSave=]  add one (checked and tried first when it has a tag): 201
//! GET    /api/v1/indexerproxy/{id}          one
//! PUT    /api/v1/indexerproxy/{id}[?forceSave=]  change one (tried first unless forceSave): 202
//! DELETE /api/v1/indexerproxy/{id}          remove one: {}
//! GET    /api/v1/indexerproxy/schema        the four kinds, ready to fill in
//! POST   /api/v1/indexerproxy/test          try one without saving it
//! POST   /api/v1/indexerproxy/testall       try every one in use: 400 when one fails
//! POST   /api/v1/indexerproxy/action/{name} a kind's own action (proxies have none: null)
//! ```
//!
//! The checks answer as Prowlarr does: a 400 with the list of failures, each naming its field.

use super::proxies::{self, Kind, Proxy, Settings};
use crate::prowlarr::validation::{self, Failure};
use crate::prowlarr::{accepted, body_json, created, fail, not_found, validate_id, ApiError, V1};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The indexer proxy routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/indexerproxy", get(all).post(create))
        .route("/api/v1/indexerproxy/schema", get(schema))
        .route("/api/v1/indexerproxy/test", post(test))
        .route("/api/v1/indexerproxy/testall", post(test_all))
        .route("/api/v1/indexerproxy/action/{name}", post(action))
        .route("/api/v1/indexerproxy/{id}", get(one).put(update).delete(remove))
}

/// A failure as Prowlarr writes the ones its settings checks and proxy tests give (`NzbDroneValidationFailure`
/// written as a `ValidationFailure`): the field, the message, the value given when there was one, the severity.
fn bare(f: &Failure) -> Value {
    let mut v = json!({ "propertyName": f.property, "errorMessage": f.message, "severity": "error" });
    if !f.attempted.is_null() {
        v["attemptedValue"] = f.attempted.clone();
    }
    v
}

/// What a proxy route answers when it can't do what was asked: one of the Prowlarr API's errors, or the settings
/// or test failures (a 400 with them written as [`bare`] ones).
enum Problem {
    /// A Prowlarr API error.
    Api(ApiError),
    /// Failures of the settings' checks or of a test.
    Bare(Vec<Failure>),
}

impl From<ApiError> for Problem {
    fn from(e: ApiError) -> Problem {
        Problem::Api(e)
    }
}

impl From<kit_server_core::Error> for Problem {
    fn from(e: kit_server_core::Error) -> Problem {
        Problem::Api(ApiError::Kit(e))
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        match self {
            Problem::Api(e) => e.into_response(),
            Problem::Bare(list) => (StatusCode::BAD_REQUEST, Json(Value::Array(list.iter().map(bare).collect()))).into_response(),
        }
    }
}

/// What a proxy route returns.
type ApiResult<T> = Result<T, Problem>;

/// The 400 for `failures` written as [`bare`] ones.
fn invalid_bare(failures: &[Failure]) -> Problem {
    Problem::Bare(failures.to_vec())
}

/// Whether a query flag (`forceSave`, `forceTest`) is on.
fn flag(q: &HashMap<String, String>, name: &str) -> bool {
    q.iter().any(|(k, v)| k.eq_ignore_ascii_case(name) && v.eq_ignore_ascii_case("true"))
}

/// The request's resource as a proxy (`ProviderResourceMapper.ToModel`), with the saved one it changes (by id).
/// An unknown kind is a 400 in words.
fn to_model(st: &AppState, body: &Value) -> ApiResult<(Proxy, Option<Proxy>)> {
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
    let existing = if id > 0 { st.indexers.find_proxy(id)? } else { None };
    let implementation = body.get("implementation").and_then(Value::as_str).unwrap_or("");
    let kind = Kind::parse(implementation).ok_or_else(|| {
        fail(400, format!("There is no kind of indexer proxy called '{implementation}': use FlareSolverr, Http, Socks4 or Socks5"))
    })?;
    let contract = body.get("configContract").and_then(Value::as_str).unwrap_or("");
    if !contract.is_empty() && Kind::ALL.iter().all(|k| k.config_contract() != contract) {
        return Err(fail(400, format!("There are no indexer proxy settings called '{contract}'")).into());
    }
    let settings = Settings::read_fields(kind, body.get("fields").unwrap_or(&Value::Null), existing.as_ref().map(|e| &e.settings))?;
    let tags: Vec<i64> = body.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    let mut tags_unique: Vec<i64> = Vec::new();
    for t in tags {
        if !tags_unique.contains(&t) {
            tags_unique.push(t);
        }
    }
    let name = body.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    Ok((Proxy { id, name, kind, settings, tags: tags_unique }, existing))
}

/// The checks every resource gets before anything else (`RestController.ValidateResource` with the shared,
/// POST and PUT rules of `ProviderControllerBase`).
fn validate_resource(st: &AppState, body: &Value, post: bool, put: bool) -> ApiResult<()> {
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
    let name = body.get("name").and_then(Value::as_str);
    let mut list: Vec<Failure> = Vec::new();
    list.extend(validation::not_empty("Name", name));
    if let Some(n) = name {
        if st.indexers.proxies().iter().any(|p| p.name.eq_ignore_ascii_case(n) && p.id != id) {
            let mut f = validation::condition("Name", json!(n));
            f.message = "Should be unique".into();
            list.push(f);
        }
    }
    list.extend(validation::not_empty("Implementation", body.get("implementation").and_then(Value::as_str)));
    list.extend(validation::not_empty("ConfigContract", body.get("configContract").and_then(Value::as_str)));
    if post {
        list.extend(validation::not_null("Fields", body.get("fields").unwrap_or(&Value::Null)));
    }
    if put {
        list.extend(validation::valid_id(id));
    }
    if list.is_empty() {
        Ok(())
    } else {
        Err(Problem::Api(ApiError::Invalid(list)))
    }
}

/// Checks a proxy's settings (`Validate`): a 400 with the failures.
fn validate_settings(p: &Proxy) -> ApiResult<()> {
    let f = p.settings.validate(p.kind);
    if f.is_empty() {
        Ok(())
    } else {
        Err(invalid_bare(&f))
    }
}

/// Tries a proxy (`Test`): a 400 with the failures.
async fn try_proxy(st: &AppState, p: &Proxy) -> ApiResult<()> {
    let f = proxies::test(p, &st.indexers.proxy_ping_url(), st.indexers.solver.as_ref(), &st.indexers.site_proxy()).await;
    if f.is_empty() {
        Ok(())
    } else {
        Err(invalid_bare(&f))
    }
}

/// Proxy `id` as Prowlarr answers it, or its 404.
fn resource(st: &AppState, id: i64) -> ApiResult<Value> {
    st.indexers.find_proxy(id)?.map(|p| p.resource()).ok_or_else(|| not_found("IndexerProxyDefinition", id).into())
}

/// `GET /api/v1/indexerproxy`: every proxy, by name.
async fn all(State(st): State<AppState>, _: V1) -> ApiResult<Json<Value>> {
    Ok(Json(Value::Array(st.indexers.proxies().iter().map(Proxy::resource).collect())))
}

/// `GET /api/v1/indexerproxy/{id}`.
async fn one(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(resource(&st, id)?))
}

/// `POST /api/v1/indexerproxy`: checks the proxy and, when it has a tag, its settings and that it works (even with
/// `forceSave`, as in Prowlarr); 201 with it.
async fn create(State(st): State<AppState>, _: V1, Query(q): Query<HashMap<String, String>>, bytes: Bytes) -> ApiResult<Response> {
    let body = body_json(&bytes)?;
    validate_resource(&st, &body, true, false)?;
    let (mut p, _) = to_model(&st, &body)?;
    if p.enabled() {
        validate_settings(&p)?;
        let _force = flag(&q, "forceSave");
        try_proxy(&st, &p).await?;
    }
    p.id = st.indexers.insert_proxy(&p)?;
    st.indexers.forget_clients();
    Ok(created(format!("/api/v1/indexerproxy/{}", p.id), resource(&st, p.id)?))
}

/// `PUT /api/v1/indexerproxy/{id}`: checks the proxy and, when it has a tag, its settings and (unless
/// `forceSave`) that it works; 202 with it.
async fn update(State(st): State<AppState>, _: V1, Path(path_id): Path<i64>, Query(q): Query<HashMap<String, String>>, bytes: Bytes) -> ApiResult<Response> {
    let mut body = body_json(&bytes)?;
    if body.get("id").and_then(Value::as_i64).unwrap_or(0) == 0 {
        if let Some(o) = body.as_object_mut() {
            o.insert("id".into(), json!(path_id));
        }
    }
    validate_resource(&st, &body, false, true)?;
    validate_id(path_id)?;
    let (p, _) = to_model(&st, &body)?;
    if p.enabled() {
        validate_settings(&p)?;
        if !flag(&q, "forceSave") {
            try_proxy(&st, &p).await?;
        }
    }
    st.indexers.update_proxy(&p)?;
    Ok(accepted(format!("/api/v1/indexerproxy/{}", p.id), resource(&st, p.id)?))
}

/// `DELETE /api/v1/indexerproxy/{id}`: `{}`.
async fn remove(State(st): State<AppState>, _: V1, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    validate_id(id)?;
    st.indexers.delete_proxy(id)?;
    Ok(Json(json!({})))
}

/// `GET /api/v1/indexerproxy/schema`: each kind with its starting settings.
async fn schema(_: V1) -> Json<Value> {
    Json(Value::Array(Kind::ALL.iter().map(|k| Proxy::template(*k)).collect()))
}

/// `POST /api/v1/indexerproxy/test`: checks the name and the settings, then tries the proxy; `"{}"` when it works.
async fn test(State(st): State<AppState>, _: V1, bytes: Bytes) -> ApiResult<Json<Value>> {
    let body = body_json(&bytes)?;
    validate_resource(&st, &body, false, false)?;
    let (p, _) = to_model(&st, &body)?;
    validate_settings(&p)?;
    try_proxy(&st, &p).await?;
    Ok(Json(json!("{}")))
}

/// `POST /api/v1/indexerproxy/testall`: tries every proxy in use (on, settings valid); each one's id, whether it
/// works and its failures. 400 when one fails.
async fn test_all(State(st): State<AppState>, _: V1) -> Response {
    let mut list = Vec::new();
    let mut ok = true;
    for p in st.indexers.proxies().into_iter().filter(|p| p.valid() && p.enabled()) {
        let failures = proxies::test(&p, &st.indexers.proxy_ping_url(), st.indexers.solver.as_ref(), &st.indexers.site_proxy()).await;
        ok &= failures.is_empty();
        list.push(json!({ "id": p.id, "isValid": failures.is_empty(), "validationFailures": failures.iter().map(bare).collect::<Vec<_>>() }));
    }
    let status = if ok { StatusCode::OK } else { StatusCode::BAD_REQUEST };
    (status, Json(Value::Array(list))).into_response()
}

/// `POST /api/v1/indexerproxy/action/{name}`: proxies have no actions of their own, so `null`.
async fn action(State(st): State<AppState>, _: V1, Path(_name): Path<String>, bytes: Bytes) -> ApiResult<Json<Value>> {
    let body = body_json(&bytes)?;
    to_model(&st, &body)?;
    Ok(Json(Value::Null))
}
