// Ported from cross-seed v6.13.9 (src/server.ts), Apache-2.0.
//! cross-seed's daemon API, on its own port as cross-seed has it (`cross_seed_port`, 2468 by default), so autobrr,
//! Sonarr, Radarr and scripts reach Lantern where they reach cross-seed. Answers are cross-seed's: plain text
//! bodies (no content type), its status codes and its messages.
//!
//! ```text
//! POST /api/webhook   search one torrent: infoHash (or path) [+ ignoreCrossSeeds, ignoreExcludeRecentSearch,
//!                     ignoreExcludeOlder, ignoreBlockList, includeSingleEpisodes, includeNonVideos]
//!                     204 (searches after answering), 400 with the reason
//! POST /api/announce  name, guid, link (the same), tracker [, cookie]: match a new release now
//!                     200 added or already there, 202 kept until the torrent finishes, 204 no match, 400, 500
//! POST /api/job       name [, ignoreExcludeRecentSearch, ignoreExcludeOlder]: run a job now
//!                     200, 400, 404 (not a job here), 409 (running, or not yet eligible)
//! GET  /api/ping      200 OK (no key)
//! GET  /api/status    200 OK
//! ```
//!
//! The API key (Lantern's `api_key`) goes in an `X-Api-Key` header or an `apikey` query parameter; without it,
//! 401. Bodies are JSON or form data. Any other path is 404 `Unknown endpoint: <path>`; another method is 405.

use super::config::Override;
use super::constants::{Decision, InjectionResult};
use super::decide::Candidate;
use super::prefilter::{human_readable_date, MAX_INT};
use super::utils::{format_as_list, sanitize_info_hash};
use super::{now_ms, CrossSeed};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::Router;
use serde_json::{Map, Number, Value};

/// The daemon API for `xs`: every path goes through [`handle`], as cross-seed's request router.
pub fn router(xs: CrossSeed) -> Router {
    Router::new().fallback(handle).with_state(xs)
}

/// A plain answer: a status and a text body, without a content type (as Node's `res.end(text)`).
fn reply(status: u16, body: impl Into<String>) -> Response {
    let mut r = Response::new(Body::from(body.into()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r
}

/// The job names cross-seed knows (`JobName`).
pub const JOB_NAMES: [&str; 5] = ["rss", "search", "updateIndexerCaps", "inject", "cleanup"];

/// `handleRequest`: the method first, then the key, then the endpoint.
pub async fn handle(State(xs): State<CrossSeed>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let endpoint = uri.path().to_string();
    let wants = match endpoint.as_str() {
        "/api/announce" | "/api/webhook" | "/api/job" => Method::POST,
        "/api/ping" | "/api/status" => Method::GET,
        _ => {
            tracing::error!("cross-seed: Unknown endpoint: {endpoint}");
            return reply(404, format!("Unknown endpoint: {endpoint}"));
        }
    };
    if method != wants {
        return reply(405, format!("Method {method} not allowed for {endpoint}"));
    }
    if endpoint != "/api/ping" && !authorized(&xs, &headers, &uri) {
        let from = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .unwrap_or("an unknown address")
            .to_string();
        tracing::error!("cross-seed: Unauthorized API access attempt to {endpoint} from {from}");
        return reply(401, "Specify the API key in an X-Api-Key header or an apikey query param.");
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    match endpoint.as_str() {
        "/api/announce" => announce(xs, &body).await,
        "/api/webhook" => webhook(xs, &body).await,
        "/api/job" => job(xs, &body).await,
        _ => reply(200, "OK"),
    }
}

/// `authorize`: the `X-Api-Key` header (when sent, even empty) or else the `apikey` query parameter is
/// Lantern's API key.
fn authorized(xs: &CrossSeed, headers: &HeaderMap, uri: &Uri) -> bool {
    let given = match headers.get("x-api-key") {
        Some(v) => Some(v.to_str().unwrap_or("").to_string()),
        None => url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()).find(|(k, _)| k == "apikey").map(|(_, v)| v.into_owned()),
    };
    let want = xs.api_key();
    !want.is_empty() && given.is_some_and(|g| crate::auth::ct_eq(&g, &want))
}

/// How a body was written, for showing it as Node does: whether it was form data (an object without a
/// prototype) and the order of its fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    /// It came as form data (Node's `querystring`).
    pub form: bool,
    /// Its top-level fields in the order they came.
    pub order: Vec<String>,
}

/// A request body as cross-seed's `parseData` reads it: JSON, else form data; how it was written; or the reason it
/// can't be read.
pub fn parse_data(data: &str) -> Result<(Value, Shape), String> {
    let (mut parsed, shape) = match serde_json::from_str::<Value>(data) {
        Ok(v) => {
            let order = serde_json::from_str::<indexmap::IndexMap<String, serde::de::IgnoredAny>>(data)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            (v, Shape { form: false, order })
        }
        Err(_) => {
            let mut m = Map::new();
            let mut order = Vec::new();
            for (k, v) in url::form_urlencoded::parse(data.as_bytes()) {
                let (k, v) = (k.into_owned(), Value::String(v.into_owned()));
                if !order.contains(&k) {
                    order.push(k.clone());
                }
                match m.get_mut(&k) {
                    Some(Value::Array(a)) => a.push(v),
                    Some(old) => *old = Value::Array(vec![old.clone(), v]),
                    None => {
                        m.insert(k, v);
                    }
                }
            }
            (Value::Object(m), Shape { form: true, order })
        }
    };
    let unable = || format!("Unable to parse request body: \"{data}\"");
    match &mut parsed {
        // `"infoHash" in parsed` throws on anything but an object
        Value::Object(m) => {
            if let Some(h) = m.get_mut("infoHash") {
                match h {
                    Value::String(s) => *s = s.to_lowercase(),
                    _ => return Err(unable()),
                }
            }
            if let Some(Value::String(s)) = m.get("size") {
                let n = js_number_of(s);
                m.insert("size".into(), Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null));
            }
        }
        Value::Array(_) => {}
        _ => return Err(unable()),
    }
    Ok((parsed, shape))
}

/// JavaScript's `Number(s)` (`NaN` for text that isn't a number).
fn js_number_of(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// Node's `util.inspect` of a parsed body, as cross-seed puts it in its messages (its fields in the order sent).
pub fn inspect(v: &Value, shape: &Shape) -> String {
    match v {
        Value::Object(m) => {
            let mut ordered = indexmap::IndexMap::new();
            for k in shape.order.iter().chain(m.keys()) {
                if let Some(x) = m.get(k) {
                    ordered.entry(k.clone()).or_insert_with(|| x.clone());
                }
            }
            inspect_entries(ordered.into_iter().collect(), shape.form, 0)
        }
        _ => inspect_at(v, shape.form, 0),
    }
}

/// Node's `util.inspect` of a value that isn't a body.
pub fn inspect_value(v: &Value) -> String {
    inspect_at(v, false, 0)
}

/// An object's fields as Node shows them.
fn inspect_entries(entries: Vec<(String, Value)>, null_prototype: bool, indent: usize) -> String {
    let prefix = if null_prototype { "[Object: null prototype] " } else { "" };
    if entries.is_empty() {
        return format!("{prefix}{{}}");
    }
    let items = entries.iter().map(|(k, x)| format!("{}: {}", key(k), inspect_at(x, false, indent + 2))).collect();
    wrap(format!("{prefix}{{"), "}", items, indent)
}

/// A key as Node shows it: bare when it is a name, else quoted.
fn key(k: &str) -> String {
    let ident = k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ident {
        k.to_string()
    } else {
        quote(k)
    }
}

/// Node's `reduceToSingleString`: one line when it fits in 80 columns, else one item per line.
fn wrap(open: String, close: &str, items: Vec<String>, indent: usize) -> String {
    let start = items.len() + indent + open.len() + 10;
    let total = items.len() + start + items.iter().map(|i| i.chars().count()).sum::<usize>();
    if total + items.len() <= 80 && !items.iter().any(|i| i.contains('\n')) {
        return format!("{open} {} {close}", items.join(", "));
    }
    let pad = " ".repeat(indent);
    format!("{open}\n{pad}  {}\n{pad}{close}", items.join(&format!(",\n{pad}  ")))
}

fn quote(s: &str) -> String {
    let q = if !s.contains('\'') {
        '\''
    } else if !s.contains('"') {
        '"'
    } else if !s.contains('`') {
        '`'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

fn inspect_at(v: &Value, null_prototype: bool, indent: usize) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.as_f64().map(super::utils::js_number).unwrap_or_else(|| n.to_string()),
        Value::String(s) => quote(s),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Array(a) => wrap("[".into(), "]", a.iter().map(|x| inspect_at(x, false, indent + 2)).collect(), indent),
        Value::Object(m) => inspect_entries(m.iter().map(|(k, x)| (k.clone(), x.clone())).collect(), null_prototype, indent),
    }
}

/// A boolean field as cross-seed's schema reads it: a boolean, or text (`"true"` is true, any other false).
fn flag(v: &Value) -> Result<bool, ()> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::String(s) => Ok(s == "true"),
        _ => Err(()),
    }
}

/// The webhook's request (`WEBHOOK_SCHEMA`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebhookRequest {
    /// The torrent's info hash (small letters).
    pub info_hash: Option<String>,
    /// Or the data's path.
    pub path: Option<String>,
    /// `ignoreCrossSeeds` (true when not sent).
    pub ignore_cross_seeds: Option<bool>,
    /// `ignoreExcludeRecentSearch`.
    pub ignore_exclude_recent_search: Option<bool>,
    /// `ignoreExcludeOlder`.
    pub ignore_exclude_older: Option<bool>,
    /// `ignoreBlockList`.
    pub ignore_block_list: Option<bool>,
    /// `includeSingleEpisodes`.
    pub include_single_episodes: Option<bool>,
    /// `includeNonVideos`.
    pub include_non_videos: Option<bool>,
}

/// `WEBHOOK_SCHEMA.parse`: known fields only, an info hash of 40 characters or a path that exists, not both.
pub fn webhook_schema(v: &Value) -> Option<WebhookRequest> {
    let m = v.as_object()?;
    let mut r = WebhookRequest::default();
    for (k, x) in m {
        match k.as_str() {
            "infoHash" => {
                let s = x.as_str()?;
                if s.encode_utf16().count() != 40 {
                    return None;
                }
                r.info_hash = Some(s.to_string());
            }
            "path" => {
                let s = x.as_str()?;
                if s.is_empty() || !std::path::Path::new(s).exists() {
                    return None;
                }
                r.path = Some(s.to_string());
            }
            "ignoreCrossSeeds" => r.ignore_cross_seeds = Some(flag(x).ok()?),
            "ignoreExcludeRecentSearch" => r.ignore_exclude_recent_search = Some(flag(x).ok()?),
            "ignoreExcludeOlder" => r.ignore_exclude_older = Some(flag(x).ok()?),
            "ignoreBlockList" => r.ignore_block_list = Some(flag(x).ok()?),
            "includeSingleEpisodes" => r.include_single_episodes = Some(flag(x).ok()?),
            "includeNonVideos" => r.include_non_videos = Some(flag(x).ok()?),
            _ => return None,
        }
    }
    (r.info_hash.is_none() != r.path.is_none()).then_some(r)
}

/// `search`: `POST /api/webhook`.
async fn webhook(xs: CrossSeed, body: &str) -> Response {
    let (data, shape) = match parse_data(body) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("cross-seed webhook: {e}");
            return reply(400, e);
        }
    };
    let Some(req) = webhook_schema(&data) else {
        let message = format!(
            "A valid infoHash or an accessible path must be provided (infoHash is recommended: see https://www.cross-seed.org/docs/reference/api#post-apiwebhook): {}",
            inspect(&data, &shape)
        );
        tracing::error!("cross-seed webhook: {message}");
        return reply(400, message);
    };
    let criteria = match &req.info_hash {
        Some(h) => inspect(&data, &shape).replace(h.as_str(), &sanitize_info_hash(h)),
        None => inspect(&data, &shape),
    };
    tracing::info!("cross-seed webhook: Received search request: {criteria}");
    let o = Override {
        include_single_episodes: req.include_single_episodes,
        include_non_videos: req.include_non_videos,
        exclude_recent_search: req.ignore_exclude_recent_search.filter(|b| *b).map(|_| 1.0),
        exclude_older: req.ignore_exclude_older.filter(|b| *b).map(|_| MAX_INT),
        block_list: req.ignore_block_list.filter(|b| *b).map(|_| Vec::new()),
        ..Override::default()
    };
    let ignore_cross_seeds = req.ignore_cross_seeds.unwrap_or(true);
    tokio::spawn(async move {
        // a saved torrent may now have what it was waiting for
        if xs.job(super::jobs::INJECT).is_some() {
            xs.update_job(super::jobs::INJECT, |j| j.run_ahead = true);
            let me = xs.clone();
            tokio::spawn(async move { me.check_jobs(false, true).await });
        }
        let cfg = xs.config().0;
        xs.index_torrents_and_data_dirs(&cfg, false).await;
        let found = xs.search_for_local_torrent_by_criteria(req.info_hash.as_deref(), req.path.as_deref(), &o, ignore_cross_seeds).await;
        match found {
            Ok(Some(n)) => tracing::info!("cross-seed webhook: Found {n} torrents for {criteria}"),
            Ok(None) => {}
            Err(e) => tracing::error!("cross-seed webhook: {}", e.detail),
        }
    });
    reply(204, "")
}

/// A zod issue as cross-seed's announce message shows it.
fn zod_issue(fields: &[(&str, Value)]) -> Issue {
    Issue(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

/// One zod issue: its fields in zod's order.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue(pub Vec<(String, Value)>);

impl Issue {
    /// Field `k`, `null` when it has none.
    pub fn get(&self, k: &str) -> &Value {
        self.0.iter().find(|(n, _)| n == k).map(|(_, v)| v).unwrap_or(&Value::Null)
    }

    /// Its `path` joined with dots, as cross-seed lists it.
    pub fn path(&self) -> String {
        self.get("path").as_array().map(|p| p.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(".")).unwrap_or_default()
    }
}

/// Node's `util.inspect` of zod's issues.
pub fn inspect_issues(issues: &[Issue]) -> String {
    if issues.is_empty() {
        return "[]".into();
    }
    wrap("[".into(), "]", issues.iter().map(|i| inspect_entries(i.0.clone(), false, 2)).collect(), 0)
}

/// The JavaScript type name zod reports (`received`).
fn js_type(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

/// `ANNOUNCE_SCHEMA.parse`: the candidate, or zod's issues (`path` and the rest).
pub fn announce_schema(v: &Value) -> Result<Candidate, Vec<Issue>> {
    let Some(m) = v.as_object() else {
        let received = js_type(Some(v));
        return Err(vec![zod_issue(&[
            ("code", "invalid_type".into()),
            ("expected", "object".into()),
            ("received", received.into()),
            ("path", Value::Array(vec![])),
            ("message", format!("Expected object, received {received}").into()),
        ])]);
    };
    let mut issues = Vec::new();
    let path = |k: &str| Value::Array(vec![Value::String(k.into())]);
    let text = |k: &str, nullish: bool, issues: &mut Vec<Issue>| -> Option<String> {
        match m.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            None | Some(Value::Null) if nullish => None,
            other => {
                let received = js_type(other);
                let message = if other.is_none() { "Required".to_string() } else { format!("Expected string, received {received}") };
                issues.push(zod_issue(&[
                    ("code", "invalid_type".into()),
                    ("expected", "string".into()),
                    ("received", received.into()),
                    ("path", path(k)),
                    ("message", message.into()),
                ]));
                None
            }
        }
    };
    let custom = |k: &str| zod_issue(&[("code", "custom".into()), ("message", "Invalid input".into()), ("path", path(k))]);
    let url_issue = |k: &str| {
        zod_issue(&[("validation", "url".into()), ("code", "invalid_string".into()), ("message", "Invalid url".into()), ("path", path(k))])
    };
    let name = text("name", false, &mut issues).map(|s| s.trim().to_string());
    if name.as_deref() == Some("") {
        issues.push(custom("name"));
    }
    let guid = text("guid", false, &mut issues);
    if guid.as_deref().is_some_and(|g| url::Url::parse(g).is_err()) {
        issues.push(url_issue("guid"));
    }
    let link = text("link", false, &mut issues);
    if link.as_deref().is_some_and(|g| url::Url::parse(g).is_err()) {
        issues.push(url_issue("link"));
    }
    let tracker = text("tracker", false, &mut issues).map(|s| s.trim().to_string());
    if tracker.as_deref() == Some("") {
        issues.push(custom("tracker"));
    }
    let cookie = text("cookie", true, &mut issues).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let extra: Vec<Value> = m
        .keys()
        .filter(|k| !["name", "guid", "link", "tracker", "cookie"].contains(&k.as_str()))
        .map(|k| Value::String(k.clone()))
        .collect();
    if !extra.is_empty() {
        let listed: Vec<String> = extra.iter().filter_map(Value::as_str).map(|k| format!("'{k}'")).collect();
        issues.push(zod_issue(&[
            ("code", "unrecognized_keys".into()),
            ("keys", Value::Array(extra)),
            ("path", Value::Array(vec![])),
            ("message", format!("Unrecognized key(s) in object: {}", listed.join(", ")).into()),
        ]));
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let (name, guid, link, tracker) =
        (name.unwrap_or_default(), guid.unwrap_or_default(), link.unwrap_or_default(), tracker.unwrap_or_default());
    if guid != link {
        return Err(vec![zod_issue(&[("code", "custom".into()), ("message", "Invalid input".into()), ("path", Value::Array(vec![]))])]);
    }
    Ok(Candidate { name, guid, link, tracker, cookie, ..Candidate::default() })
}

/// `determineResponse`: the status for an announce's outcome; `None` for an outcome cross-seed calls unexpected.
pub fn determine_response(decision: Option<Decision>, result: Option<InjectionResult>) -> Option<(u16, &'static str)> {
    let injected = result == Some(InjectionResult::Success);
    let added = injected || result == Some(InjectionResult::Failure) || result == Some(InjectionResult::Saved);
    let exists = decision == Some(Decision::InfoHashAlreadyExists) || result == Some(InjectionResult::AlreadyExists);
    let incomplete = result == Some(InjectionResult::TorrentNotComplete);
    if added {
        Some((200, if injected { "Injected" } else { "Saved" }))
    } else if exists {
        Some((200, "Already exists"))
    } else if incomplete {
        Some((202, "Saved"))
    } else {
        None
    }
}

/// `announce`: `POST /api/announce`.
async fn announce(xs: CrossSeed, body: &str) -> Response {
    let (data, shape) = match parse_data(body) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("cross-seed announce: {e}");
            return reply(400, e);
        }
    };
    let candidate = match announce_schema(&data) {
        Ok(c) => c,
        Err(issues) => {
            let paths: Vec<String> = issues.iter().map(Issue::path).collect();
            let message = format!(
                "Missing required params (https://www.cross-seed.org/docs/v6-migration#autobrr-update): {{{}}} in {}\n{}",
                format_as_list(&paths, true, "long", "unit"),
                inspect(&data, &shape),
                inspect_issues(&issues)
            );
            tracing::error!("cross-seed announce: {message}");
            return reply(400, message);
        }
    };
    tracing::debug!("cross-seed announce: Received announce from {}: {}", candidate.tracker, candidate.name);
    let shown = format!("{} from {}", candidate.name, candidate.tracker);
    let cfg = xs.config().0;
    if !cfg.use_client_torrents && cfg.torrent_dir.is_none() && cfg.data_dirs.is_empty() {
        let message = "Announce requires at least one of useClientTorrents, torrentDir, or dataDirs to be set";
        tracing::error!("cross-seed announce: {message}");
        return reply(500, message);
    }
    xs.index_torrents_and_data_dirs(&cfg, false).await;
    match xs.check_new_candidate_match(candidate, super::searchee::Label::Announce).await {
        Ok((None, _)) => reply(204, ""),
        Ok((decision, result)) => match determine_response(decision, result) {
            Some((status, state)) => {
                if result != Some(InjectionResult::Success) {
                    tracing::info!("cross-seed announce: {state} {shown} (status: {status})");
                }
                reply(status, "")
            }
            None => {
                let message = format!(
                    "Unexpected result: {} | {}",
                    decision.map(|d| d.as_str()).unwrap_or("null"),
                    result.map(|r| r.as_str()).unwrap_or("null")
                );
                tracing::error!("cross-seed announce: {message}");
                reply(500, message)
            }
        },
        Err(e) => {
            tracing::error!("cross-seed announce: {}", e.detail);
            reply(500, e.detail)
        }
    }
}

/// The job's request (`JOB_SCHEMA`): its name and the two overrides.
fn job_schema(v: &Value) -> Option<(String, bool, bool)> {
    let m = v.as_object()?;
    let mut name = None;
    let (mut recent, mut older) = (false, false);
    for (k, x) in m {
        match k.as_str() {
            "name" => name = Some(x.as_str()?.to_string()),
            "ignoreExcludeRecentSearch" => recent = flag(x).ok()?,
            "ignoreExcludeOlder" => older = flag(x).ok()?,
            _ => return None,
        }
    }
    let name = name.filter(|n| JOB_NAMES.contains(&n.as_str()))?;
    Some((name, recent, older))
}

/// `runJob`: `POST /api/job`.
async fn job(xs: CrossSeed, body: &str) -> Response {
    let (data, shape) = match parse_data(body) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("cross-seed: {e}");
            return reply(400, e);
        }
    };
    let Some((name, ignore_recent, ignore_older)) = job_schema(&data) else {
        let names: Vec<String> = JOB_NAMES.iter().map(|s| s.to_string()).collect();
        let message =
            format!("Job name must be one of {} - received: {}", format_as_list(&names, true, "narrow", "unit"), inspect(&data, &shape));
        tracing::error!("cross-seed: {message}");
        return reply(400, message);
    };
    tracing::info!("cross-seed: Received job request: {}", inspect(&data, &shape));
    xs.refresh_jobs();
    let Some(job) = xs.job(&name) else {
        let message = format!("{name}: unable to run, disabled in config");
        tracing::error!("cross-seed: {message}");
        return reply(404, message);
    };
    if job.is_active {
        let message = format!("{name}: already running");
        tracing::error!("cross-seed: {message}");
        return reply(409, message);
    }
    let last_run = xs.job_last_run(&name).unwrap_or(0.0);
    if now_ms() < last_run {
        let message = format!(
            "{name}: not eligible to run ahead of schedule, next scheduled run is at {} (triggering an early run is allowed after {})",
            human_readable_date(last_run + job.cadence),
            human_readable_date(last_run)
        );
        tracing::error!("cross-seed: {message}");
        return reply(409, message);
    }
    let o = Override {
        exclude_recent_search: ignore_recent.then_some(1.0),
        exclude_older: ignore_older.then_some(MAX_INT),
        ..Override::default()
    };
    xs.update_job(&name, |j| {
        j.run_ahead = true;
        if j.name == super::jobs::SEARCH || j.name == super::jobs::RSS {
            j.delay_next_run = true;
        }
        j.config_override = o;
    });
    let me = xs.clone();
    tokio::spawn(async move { me.check_jobs(false, true).await });
    reply(200, format!("{name}: running ahead of schedule"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bodies_as_cross_seed_reads_them() {
        let (v, shape) = parse_data(r#"{"infoHash":"ABC"}"#).unwrap();
        assert_eq!((v, shape.form), (json!({ "infoHash": "abc" }), false));
        let (v, shape) = parse_data("infoHash=ABC&includeNonVideos=true").unwrap();
        assert_eq!((v, shape.form), (json!({ "infoHash": "abc", "includeNonVideos": "true" }), true));
        assert_eq!(parse_data("42").unwrap_err(), "Unable to parse request body: \"42\"");
        assert_eq!(parse_data(r#"{"infoHash":1}"#).unwrap_err(), r#"Unable to parse request body: "{"infoHash":1}""#);
        assert_eq!(parse_data(r#"{"size":"12"}"#).unwrap().0, json!({ "size": 12.0 }));
    }

    #[test]
    fn inspect_like_node() {
        let (v, shape) = parse_data(r#"{"infoHash":"abc","n":1,"b":true}"#).unwrap();
        assert_eq!(inspect(&v, &shape), "{ infoHash: 'abc', n: 1, b: true }");
        let (v, shape) = parse_data("").unwrap();
        assert_eq!(inspect(&v, &shape), "[Object: null prototype] {}");
        let (v, shape) = parse_data("z=1&a-b=it's").unwrap();
        assert_eq!(inspect(&v, &shape), "[Object: null prototype] { z: '1', 'a-b': \"it's\" }");
        assert_eq!(inspect_value(&json!([])), "[]");
        let issues = vec![zod_issue(&[
            ("code", "invalid_type".into()),
            ("expected", "string".into()),
            ("received", "undefined".into()),
            ("path", json!(["name"])),
            ("message", "Required".into()),
        ])];
        assert_eq!(
            inspect_issues(&issues),
            "[\n  {\n    code: 'invalid_type',\n    expected: 'string',\n    received: 'undefined',\n    path: [ 'name' ],\n    message: 'Required'\n  }\n]"
        );
    }

    #[test]
    fn webhook_schema_like_zod() {
        let h = "a".repeat(40);
        assert_eq!(webhook_schema(&json!({ "infoHash": h })).unwrap().info_hash.as_deref(), Some(h.as_str()));
        assert!(webhook_schema(&json!({ "infoHash": "abc" })).is_none(), "40 characters");
        assert!(webhook_schema(&json!({})).is_none(), "one of infoHash and path");
        assert!(webhook_schema(&json!({ "infoHash": h, "path": "/" })).is_none(), "not both");
        assert!(webhook_schema(&json!({ "path": "/no/such/folder/here" })).is_none(), "a path that exists");
        assert!(webhook_schema(&json!({ "infoHash": h, "other": 1 })).is_none(), "no other fields");
        let r = webhook_schema(&json!({ "infoHash": h, "includeNonVideos": "true", "ignoreCrossSeeds": "no" })).unwrap();
        assert_eq!((r.include_non_videos, r.ignore_cross_seeds), (Some(true), Some(false)));
        assert!(webhook_schema(&json!({ "infoHash": h, "includeNonVideos": 1 })).is_none());
    }

    #[test]
    fn announce_schema_like_zod() {
        let ok = json!({ "name": " Show.S01E01-GRP ", "guid": "https://t/1", "link": "https://t/1", "tracker": "T", "cookie": " " });
        let c = announce_schema(&ok).unwrap();
        assert_eq!((c.name.as_str(), c.cookie), ("Show.S01E01-GRP", None));
        let issues = announce_schema(&json!({ "name": "x", "guid": "nope", "link": "https://t/1", "size": 1 })).unwrap_err();
        let paths: Vec<Value> = issues.iter().map(|i| i.get("path").clone()).collect();
        assert_eq!(paths, vec![json!(["guid"]), json!(["tracker"]), json!([])]);
        assert_eq!(issues[2].get("message"), "Unrecognized key(s) in object: 'size'");
        let differ = announce_schema(&json!({ "name": "x", "guid": "https://t/1", "link": "https://t/2", "tracker": "T" })).unwrap_err();
        assert_eq!(differ[0].get("path"), &json!([]));
    }

    #[test]
    fn announce_statuses() {
        assert_eq!(determine_response(Some(Decision::Match), Some(InjectionResult::Success)), Some((200, "Injected")));
        assert_eq!(determine_response(Some(Decision::MatchSizeOnly), Some(InjectionResult::Failure)), Some((200, "Saved")));
        assert_eq!(determine_response(Some(Decision::InfoHashAlreadyExists), None), Some((200, "Already exists")));
        assert_eq!(determine_response(Some(Decision::Match), Some(InjectionResult::AlreadyExists)), Some((200, "Already exists")));
        assert_eq!(determine_response(Some(Decision::Match), Some(InjectionResult::TorrentNotComplete)), Some((202, "Saved")));
        assert_eq!(determine_response(Some(Decision::Match), None), None);
    }

    #[test]
    fn job_schema_like_zod() {
        assert_eq!(job_schema(&json!({ "name": "search", "ignoreExcludeOlder": "true" })), Some(("search".into(), false, true)));
        assert!(job_schema(&json!({ "name": "nope" })).is_none());
        assert!(job_schema(&json!({})).is_none());
        assert!(job_schema(&json!({ "name": "rss", "x": 1 })).is_none());
    }
}
