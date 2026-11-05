//! The releases page, `#/releases`: what the IRC announce channels and RSS feeds brought and what happened to
//! each release (Recent), the filters that decide what to grab (Filters, with a check of any release name), and
//! the places releases come from (Sources: IRC announces, RSS feeds, Lantern's own indexers).
//!
//! The forms are the kit's schema form; a filter's lists (`1080p, 2160p`) are drawn as checkboxes or text and
//! sent as the lists the server reads ([`filter_body`]).

use crate::indexers::size_text;
use kit_leptos::api;
use kit_leptos::icons::icon;
use kit_leptos::modal::{self, ModalOpts};
use kit_leptos::schema_form::schema::{Field, Group};
use kit_leptos::schema_form::{form_values, SchemaFields};
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::{json, Map, Value};
use wasm_bindgen::JsCast;

/// What a masked secret is sent back as: the server keeps the saved one.
const MASK: &str = "••••";

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn spawn(f: impl std::future::Future<Output = ()> + 'static) {
    kit_leptos::rt::spawn(f)
}

/// How long ago `at` (seconds since 1970) was, from `now`, in words (`just now`, `5 min ago`).
pub fn ago(at: f64, now: f64) -> String {
    if at <= 0.0 {
        return "never".into();
    }
    let m = ((now - at) / 60.0).max(0.0);
    if m < 1.0 {
        "just now".into()
    } else {
        format!("{} ago", crate::indexers::age_text(m))
    }
}

/// Seconds since 1970 now.
fn now() -> f64 {
    js_sys::Date::now() / 1000.0
}

/// A release log status as (badge class, words).
pub fn status_words(status: &str) -> (&'static str, &'static str) {
    match status {
        "pushed" => ("badge ok", "Sent"),
        "failed" => ("badge err", "Not sent"),
        _ => ("badge", "Skipped"),
    }
}

/// `#/releases`, on the tab `tab` (`recent`, `filters` or `sources`).
#[component]
pub fn Releases(admin: bool, tab: String) -> impl IntoView {
    let tab = if ["filters", "sources"].contains(&tab.as_str()) { tab } else { "recent".to_string() };
    let go = |t: &str| crate::navigate(&format!("/releases?t={t}"));
    let page = match tab.as_str() {
        "filters" => view! { <FiltersTab admin=admin/> }.into_any(),
        "sources" => view! { <SourcesTab admin=admin/> }.into_any(),
        _ => view! { <RecentTab/> }.into_any(),
    };
    let t = tab.clone();
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Releases"</h1>
            <div class="seg">
                <button class:on=t == "recent" on:click=move |_| go("recent")>"Recent"</button>
                <button class:on=t == "filters" on:click=move |_| go("filters")>"Filters"</button>
                <button class:on=t == "sources" on:click=move |_| go("sources")>"Sources"</button>
            </div>
        </div>
        {page}
    }
}

/// Recent: the newest releases seen and what happened to each.
#[component]
fn RecentTab() -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let (only, set_only) = signal(String::new());
    let list = LocalResource::new(move || {
        tick.track();
        let only = only.get();
        async move { api::get(&format!("/releases/log?limit=200&status={only}")).await.map_err(|e| e.message) }
    });
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let items = v.get("releases").and_then(Value::as_array).cloned().unwrap_or_default();
            if items.is_empty() {
                return view! { <div class="empty" inner_html=format!(r#"{}<p class="muted">Nothing yet. Releases show here as your sources announce them.</p>"#, icon("download"))></div> }.into_any();
            }
            let t = now();
            view! {
                <div class="table-wrap"><table class="table" id="releases-table">
                    <thead><tr><th>"Release"</th><th>"From"</th><th>"Size"</th><th>"What happened"</th><th>"When"</th></tr></thead>
                    <tbody>{items.into_iter().map(|r| log_row(r, t)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    view! {
        <div class="row wrap" style="gap:10px;margin-bottom:12px">
            <select class="input" style="width:auto" on:change=move |e| set_only.set(event_target_value(&e))>
                <option value="">"Everything"</option><option value="pushed">"Sent"</option><option value="failed">"Not sent"</option><option value="rejected">"Skipped"</option>
            </select>
            <button class="btn" on:click=move |_| set_tick.update(|t| *t += 1) inner_html=format!("{}Refresh", icon("refresh"))></button>
        </div>
        <div class="card flat">{rows}</div>
    }
}

fn log_row(r: Value, now: f64) -> impl IntoView {
    let (cls, words) = status_words(&s(&r, "status"));
    let size = r.get("size").and_then(Value::as_f64).map(size_text).unwrap_or_else(|| "—".into());
    let mut detail = s(&r, "detail");
    let filter = s(&r, "filter_name");
    if !filter.is_empty() {
        detail = format!("{filter}: {detail}");
    }
    let reasons: Vec<String> = r
        .get("reasons")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    let why: Vec<String> = x
                        .get("why")
                        .and_then(Value::as_array)
                        .map(|w| w.iter().filter_map(|y| y.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    format!("{}: {}", s(x, "filter"), why.join(", "))
                })
                .collect()
        })
        .unwrap_or_default();
    let info = s(&r, "info_url");
    view! {
        <tr>
            <td style="max-width:480px"><div style="font-weight:600;word-break:break-word">
                {if info.is_empty() { s(&r, "title").into_any() } else { view! { <a href=info target="_blank" rel="noreferrer">{s(&r, "title")}</a> }.into_any() }}
            </div><div class="small faint">{s(&r, "category")}</div></td>
            <td class="small">{s(&r, "source_name")}</td>
            <td class="small" style="white-space:nowrap">{size}</td>
            <td class="small" style="max-width:380px"><span class=cls>{words}</span>" "{detail}
                {(!reasons.is_empty()).then(|| view! { <details class="mt"><summary class="faint">"Why"</summary><ul class="small">{reasons.into_iter().map(|x| view! { <li>{x}</li> }).collect_view()}</ul></details> })}
            </td>
            <td class="small faint" style="white-space:nowrap">{ago(r.get("at").and_then(Value::as_f64).unwrap_or(0.0), now)}</td>
        </tr>
    }
}

// ---- filters ----

/// The form fields of a filter.
pub fn filter_fields() -> Vec<Value> {
    let opts = |list: &[&str]| list.iter().map(|o| json!({ "value": o, "label": o })).collect::<Vec<_>>();
    vec![
        json!({ "key": "name", "label": "Name", "type": "text" }),
        json!({ "key": "priority", "label": "Priority", "type": "number", "help": "Higher is checked first; the first filter a release passes takes it." }),
        json!({ "key": "match_releases", "label": "Names to grab", "type": "text", "full": true, "help": "Comma separated. * is any text: *Show.Name*, Movie.Title.2024*. Empty: any name." }),
        json!({ "key": "except_releases", "label": "Names to skip", "type": "text", "full": true }),
        json!({ "key": "resolutions", "label": "Resolutions", "type": "checks", "options": opts(&["2160p", "1080p", "1080i", "720p", "576p", "480p"]), "full": true, "help": "None ticked: any." }),
        json!({ "key": "release_sources", "label": "Sources", "type": "checks", "options": opts(&["WEB-DL", "WEBRip", "WEB", "BluRay", "UHD BluRay", "Remux", "HDTV", "DVD", "CD"]), "full": true }),
        json!({ "key": "codecs", "label": "Codecs", "type": "checks", "options": opts(&["H.264", "H.265", "AV1", "XviD", "VC-1", "MPEG-2"]), "full": true }),
        json!({ "key": "hdr", "label": "HDR", "type": "checks", "options": opts(&["DV", "HDR10+", "HDR10", "HDR", "HLG"]), "full": true }),
        json!({ "key": "match_groups", "label": "Groups to grab", "type": "text" }),
        json!({ "key": "except_groups", "label": "Groups to skip", "type": "text" }),
        json!({ "key": "years", "label": "Years", "type": "text", "help": "Like 2019-2024, 2026" }),
        json!({ "key": "seasons", "label": "Seasons", "type": "text", "help": "Like 1-3, 5" }),
        json!({ "key": "min_size", "label": "Smallest size", "type": "text", "help": "Like 700 MB" }),
        json!({ "key": "max_size", "label": "Largest size", "type": "text", "help": "Like 30 GB" }),
        json!({ "key": "freeleech", "label": "Only freeleech", "type": "bool" }),
        json!({ "key": "skip_duplicates", "label": "Skip a name already grabbed", "type": "bool" }),
        json!({ "key": "match_categories", "label": "Categories to grab", "type": "text", "help": "As the site names them: TV*, Movies/HD" }),
        json!({ "key": "except_categories", "label": "Categories to skip", "type": "text" }),
        json!({ "key": "tags", "label": "Tags to grab", "type": "text" }),
        json!({ "key": "except_tags", "label": "Tags to skip", "type": "text" }),
        json!({ "key": "max_downloads", "label": "Most grabs", "type": "number", "help": "0: no limit" }),
        json!({ "key": "max_downloads_per", "label": "Per", "type": "select", "options": [
            { "value": "hour", "label": "hour" }, { "value": "day", "label": "day" }, { "value": "week", "label": "week" },
            { "value": "month", "label": "month" }, { "value": "ever", "label": "ever" }] }),
        json!({ "key": "delay", "label": "Wait before sending (seconds)", "type": "number", "help": "0 sends at once, best for racing." }),
        json!({ "key": "app", "label": "Send to", "type": "select", "options": [
            { "value": "sonarr", "label": "Sonarr" }, { "value": "radarr", "label": "Radarr" }, { "value": "lidarr", "label": "Lidarr" },
            { "value": "readarr", "label": "Readarr" }, { "value": "whisparr", "label": "Whisparr" }, { "value": "client", "label": "A download client" }] }),
        json!({ "key": "url", "label": "The app's address", "type": "url", "when": "app!=client", "help": "Like http://sonarr:8989" }),
        json!({ "key": "api_key", "label": "The app's API key", "type": "password", "when": "app!=client", "help": "In the app under Settings → General." }),
        json!({ "key": "client", "label": "App in the download hub", "type": "text", "when": "app=client", "help": "Its category and folder are used. Empty: an app called Releases." }),
    ]
}

/// The lists a filter keeps as lists, drawn as checkboxes.
const LIST_KEYS: [&str; 4] = ["resolutions", "release_sources", "codecs", "hdr"];
/// The numbers of a filter.
const NUMBER_KEYS: [&str; 3] = ["priority", "max_downloads", "delay"];

/// A saved filter as the form's values: lists as comma-separated text, its first action spread out.
pub fn filter_values(f: &Value) -> Value {
    let mut v = f.clone();
    for k in LIST_KEYS {
        let joined = f
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        v[k] = json!(joined);
    }
    let a = f.get("actions").and_then(|a| a.get(0)).cloned().unwrap_or(json!({ "kind": "arr", "app": "sonarr" }));
    v["app"] = json!(if s(&a, "kind") == "client" { "client".to_string() } else { s(&a, "app") });
    v["url"] = json!(s(&a, "url"));
    v["api_key"] = json!(s(&a, "api_key"));
    v["client"] = json!(s(&a, "client"));
    v
}

/// The form's values as the filter the server takes; the saved filter `cur` keeps its other actions.
pub fn filter_body(form: &Map<String, Value>, cur: &Value) -> Value {
    let mut body = Map::new();
    for (k, v) in form {
        if ["app", "url", "api_key", "client"].contains(&k.as_str()) {
            continue;
        }
        let v = if LIST_KEYS.contains(&k.as_str()) {
            json!(v.as_str().unwrap_or("").split(',').map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>())
        } else if NUMBER_KEYS.contains(&k.as_str()) {
            json!(v.as_str().and_then(|x| x.trim().parse::<i64>().ok()).or_else(|| v.as_i64()).unwrap_or(0).max(0))
        } else {
            v.clone()
        };
        body.insert(k.clone(), v);
    }
    let app = form.get("app").and_then(Value::as_str).unwrap_or("sonarr");
    let first = if app == "client" {
        json!({ "kind": "client", "name": "Download hub", "client": form.get("client").cloned().unwrap_or(json!("")) })
    } else {
        let key = form.get("api_key").and_then(Value::as_str).filter(|k| !k.is_empty()).unwrap_or(MASK);
        json!({ "kind": "arr", "app": app, "url": form.get("url").cloned().unwrap_or(json!("")), "api_key": key })
    };
    let url_empty = first.get("url").and_then(Value::as_str).is_some_and(|u| u.trim().is_empty());
    let mut actions: Vec<Value> = if app != "client" && url_empty { Vec::new() } else { vec![first] };
    actions.extend(cur.get("actions").and_then(Value::as_array).map(|a| a.iter().skip(1).cloned().collect::<Vec<_>>()).unwrap_or_default());
    body.insert("actions".into(), Value::Array(actions));
    Value::Object(body)
}

/// Filters: the list, on/off, Edit, Remove, Add, and a check of any release name.
#[component]
fn FiltersTab(admin: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let list = LocalResource::new(move || {
        tick.track();
        async { api::get("/releases/filters").await.map_err(|e| e.message) }
    });
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let items = v.get("filters").and_then(Value::as_array).cloned().unwrap_or_default();
            if items.is_empty() {
                return view! { <div class="empty" inner_html=format!(r#"{}<p class="muted">No filters yet. A filter says which releases to grab and where they go.</p>"#, icon("check"))></div> }.into_any();
            }
            view! {
                <div class="table-wrap"><table class="table" id="filters-table">
                    <thead><tr><th>"Name"</th><th>"Priority"</th><th>"Sends to"</th><th>"Grabs today"</th><th>""</th></tr></thead>
                    <tbody>{items.into_iter().map(|f| filter_row(f, admin, reload)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    view! {
        <div class="row" style="justify-content:flex-end;margin-bottom:12px">
            {admin.then(|| view! { <button class="btn primary" id="filter-add" on:click=move |_| open_filter(new_filter(), reload) inner_html=format!("{}Add filter", icon("plus"))></button> })}
        </div>
        <div class="card flat">{rows}</div>
        <CheckRelease/>
    }
}

/// A new filter as the form starts it, with the race-friendly defaults: sent at once, duplicates skipped.
fn new_filter() -> Value {
    json!({ "name": "", "enabled": true, "priority": 0, "skip_duplicates": true, "max_downloads_per": "day", "delay": 0, "max_downloads": 0, "actions": [] })
}

fn filter_row(f: Value, admin: bool, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = f.get("id").and_then(Value::as_i64).unwrap_or(0);
    let name = s(&f, "name");
    let enabled = f.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let to: Vec<String> = f
        .get("actions")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| if s(x, "kind") == "client" { "Download client".to_string() } else { s(x, "app") }).collect())
        .unwrap_or_default();
    let toggle = move |e: leptos::ev::Event| {
        let on = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()).is_some_and(|i| i.checked());
        spawn(async move {
            if let Err(e) = api::put(&format!("/releases/filters/{id}"), json!({ "enabled": on })).await {
                toast(&e.message, "error");
            }
            reload();
        })
    };
    let n2 = name.clone();
    let remove = move |_| {
        let name = n2.clone();
        spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Remove the filter {name}?"))
                .text("Releases it would have taken are skipped from now on.")
                .ok("Remove")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/releases/filters/{id}")).await {
                    Ok(_) => toast(&format!("{name} removed"), "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let f2 = f.clone();
    view! {
        <tr data-id=id>
            <td style="font-weight:650">{name}</td>
            <td>{s(&f, "priority")}</td>
            <td class="small">{if to.is_empty() { "Nowhere yet".to_string() } else { to.join(", ") }}</td>
            <td>{s(&f, "grabs_today")}</td>
            <td style="white-space:nowrap;text-align:right">
                {admin.then(|| view! {
                    <span class="row" style="gap:8px;justify-content:flex-end">
                        <label class="switch" title="On"><input type="checkbox" checked=enabled on:change=toggle/><i></i></label>
                        <button class="btn sm" on:click=move |_| open_filter(f2.clone(), reload)>"Edit"</button>
                        <button class="btn sm ghost" title="Remove" on:click=remove inner_html=icon("trash")></button>
                    </span>
                })}
            </td>
        </tr>
    }
}

/// The Add/Edit filter dialog.
fn open_filter(f: Value, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = f.get("id").and_then(Value::as_i64);
    let title = if id.is_some() { format!("Edit {}", s(&f, "name")) } else { "Add filter".to_string() };
    let (busy, set_busy) = signal(false);
    let (problem, set_problem) = signal(None::<String>);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let mut values = filter_values(&f);
    values["_meta"] = json!({});
    let fields: Vec<Field> = filter_fields().iter().map(Field::from_json).collect();
    let body = move || {
        let (fields, values) = (fields.clone(), values.clone());
        view! {
            <div id="filter-form"><SchemaFields fields=fields values=values/></div>
            {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
        }
    };
    let save = move |_| {
        let cur = f.clone();
        set_busy.set(true);
        set_problem.set(None);
        spawn(async move {
            let Some(m) = holder.get_value() else { return };
            let Some(root) = m.qs("#filter-form") else { return };
            let g = Group::from_json(&json!({ "id": "filter", "fields": filter_fields() }));
            let body = filter_body(&form_values(&g, &root, &json!({})), &cur);
            let r = match id {
                Some(id) => api::put(&format!("/releases/filters/{id}"), body).await,
                None => api::post("/releases/filters", body).await,
            };
            match r {
                Ok(_) => {
                    toast("Filter saved", "success");
                    reload();
                    m.close();
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
            set_busy.set(false);
        })
    };
    let footer = move || view! { <button class="btn primary" disabled=busy on:click=save>{move || if busy.get() { "Saving…" } else { "Save" }}</button> };
    let m = modal::open(ModalOpts::with_view(&title, body).footer_view(footer).wide(true));
    holder.set_value(Some(m));
}

/// A release name checked against every filter, sending nothing.
#[component]
fn CheckRelease() -> impl IntoView {
    let (title, set_title) = signal(String::new());
    let (size, set_size) = signal(String::new());
    let (answer, set_answer) = signal(None::<Result<Value, String>>);
    let run = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let body = json!({ "title": title.get_untracked(), "size": size.get_untracked() });
        spawn(async move { set_answer.set(Some(api::post("/releases/check", body).await.map_err(|e| e.message))) });
    };
    let result = move || match answer.get() {
        None => ().into_any(),
        Some(Err(e)) => view! { <div class="error-box mt">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let n = v.get("name").cloned().unwrap_or_default();
            let read = ["resolution", "source", "codec", "group", "year", "season", "episode"]
                .iter()
                .filter_map(|k| {
                    let x = s(&n, k);
                    (!x.is_empty()).then(|| format!("{k}: {x}"))
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let filters = v.get("filters").and_then(Value::as_array).cloned().unwrap_or_default();
            view! {
                <p class="small muted mt">{format!("Lantern reads: {read}")}</p>
                {if filters.is_empty() { view! { <p class="muted">"No filters are on."</p> }.into_any() } else {
                    view! { <ul class="list">{filters.into_iter().map(|f| {
                        let pass = f.get("passes").and_then(Value::as_bool) == Some(true);
                        let why: Vec<String> = f.get("why").and_then(Value::as_array).map(|w| w.iter().filter_map(|y| y.as_str().map(String::from)).collect()).unwrap_or_default();
                        view! { <li class="list-item"><span class=if pass { "badge ok" } else { "badge" }>{if pass { "Takes it" } else { "Skips it" }}</span>
                            " "<b>{s(&f, "name")}</b>" "<span class="small muted">{why.join(", ")}</span></li> }
                    }).collect_view()}</ul> }.into_any()
                }}
            }
            .into_any()
        }
    };
    view! {
        <div class="card mt">
            <h3 style="margin-top:0">"Check a release"</h3>
            <p class="small muted">"Paste a release name to see which filter would take it and why the others wouldn't. Nothing is sent."</p>
            <form class="row wrap" style="gap:10px" on:submit=run>
                <input class="input grow" style="min-width:260px" id="check-title" placeholder="Show.Name.S01E02.1080p.WEB-DL.H.264-GROUP" on:input=move |e| set_title.set(event_target_value(&e))/>
                <input class="input" style="width:140px" placeholder="Size (optional)" on:input=move |e| set_size.set(event_target_value(&e))/>
                <button class="btn" type="submit">"Check"</button>
            </form>
            {result}
        </div>
    }
}

// ---- sources ----

/// A source's state as (dot class, words).
pub fn source_status(src: &Value) -> (&'static str, String) {
    if src.get("enabled").and_then(Value::as_bool) != Some(true) {
        return ("", "Off".into());
    }
    if s(src, "kind") == "irc" {
        let irc = src.get("irc").cloned().unwrap_or_default();
        if irc.get("connected").and_then(Value::as_bool) == Some(true) {
            let ch: Vec<String> = irc
                .get("channels")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
                .unwrap_or_default();
            return if ch.is_empty() {
                ("warn", "Signed in, not in the announce channels yet".into())
            } else {
                ("ok", format!("In {}", ch.join(", ")))
            };
        }
        let e = s(&irc, "last_error");
        return ("warn", if e.is_empty() { "Connecting…".into() } else { e });
    }
    let e = s(src, "last_error");
    if e.is_empty() {
        ("ok", "Working".into())
    } else {
        ("err", e)
    }
}

/// Sources: IRC announces, RSS feeds and indexers; on/off, Test, Edit, Remove, Add.
#[component]
fn SourcesTab(admin: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let list = LocalResource::new(move || {
        tick.track();
        async { api::get("/releases/sources").await.map_err(|e| e.message) }
    });
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let items = v.get("sources").and_then(Value::as_array).cloned().unwrap_or_default();
            if items.is_empty() {
                return view! { <div class="empty" inner_html=format!(r#"{}<p class="muted">No sources yet. Add a tracker's IRC announce channel, an RSS feed or one of your indexers.</p>"#, icon("refresh"))></div> }.into_any();
            }
            view! {
                <div class="table-wrap"><table class="table" id="sources-table">
                    <thead><tr><th>""</th><th>"Name"</th><th>"Kind"</th><th>"Status"</th><th>""</th></tr></thead>
                    <tbody>{items.into_iter().map(|x| source_row(x, admin, reload)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    view! {
        <div class="row" style="justify-content:flex-end;margin-bottom:12px;gap:8px">
            {admin.then(|| view! {
                <button class="btn" on:click=move |_| open_source(json!({ "kind": "rss", "interval_min": 10, "enabled": true }), reload) inner_html=format!("{}Add RSS feed", icon("plus"))></button>
                <button class="btn" on:click=move |_| open_source(json!({ "kind": "indexer", "interval_min": 10, "enabled": true }), reload) inner_html=format!("{}Watch an indexer", icon("plus"))></button>
                <button class="btn primary" id="source-add" on:click=move |_| open_pick_tracker(reload) inner_html=format!("{}Add IRC announces", icon("plus"))></button>
            })}
        </div>
        <div class="card flat">{rows}</div>
        <p class="small muted mt">"IRC announces arrive within seconds; RSS feeds and indexers are looked at every few minutes. Lantern only sends what is new: adding a feed never grabs what was already in it."</p>
    }
}

fn source_row(src: Value, admin: bool, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = src.get("id").and_then(Value::as_i64).unwrap_or(0);
    let (dot, words) = source_status(&src);
    let name = s(&src, "name");
    let enabled = src.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let kind = match s(&src, "kind").as_str() {
        "irc" => format!("IRC · {}", s(&src, "definition_name")),
        "rss" => "RSS feed".to_string(),
        _ => "Indexer".to_string(),
    };
    let (testing, set_testing) = signal(false);
    let test = move |_| {
        set_testing.set(true);
        spawn(async move {
            match api::post_empty(&format!("/releases/sources/{id}/test")).await {
                Ok(r) if r.get("ok").and_then(Value::as_bool) == Some(true) => toast(&s(&r, "message"), "success"),
                Ok(r) => toast(&s(&r, "message"), "error"),
                Err(e) => toast(&e.message, "error"),
            }
            set_testing.set(false);
            reload();
        })
    };
    let toggle = move |e: leptos::ev::Event| {
        let on = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()).is_some_and(|i| i.checked());
        spawn(async move {
            if let Err(e) = api::put(&format!("/releases/sources/{id}"), json!({ "enabled": on })).await {
                toast(&e.message, "error");
            }
            reload();
        })
    };
    let n2 = name.clone();
    let remove = move |_| {
        let name = n2.clone();
        spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Remove {name}?"))
                .text("Lantern stops watching it. Releases already sent stay where they are.")
                .ok("Remove")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/releases/sources/{id}")).await {
                    Ok(_) => toast(&format!("{name} removed"), "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let s2 = src.clone();
    view! {
        <tr data-id=id>
            <td style="width:24px"><span class=format!("dot {dot}")></span></td>
            <td><div style="font-weight:650">{name}</div><div class="small faint mono">{s(&src, "url")}</div></td>
            <td><span class="badge">{kind}</span></td>
            <td class="small" style="max-width:340px"><span class="ellipsis" title=words.clone() style="display:block">{words.clone()}</span></td>
            <td style="white-space:nowrap;text-align:right">
                {admin.then(|| view! {
                    <span class="row" style="gap:8px;justify-content:flex-end">
                        <label class="switch" title="On"><input type="checkbox" checked=enabled on:change=toggle/><i></i></label>
                        <button class="btn sm" disabled=testing on:click=test>{move || if testing.get() { "Checking…" } else { "Test" }}</button>
                        <button class="btn sm" on:click=move |_| open_source(s2.clone(), reload)>"Edit"</button>
                        <button class="btn sm ghost" title="Remove" on:click=remove inner_html=icon("trash")></button>
                    </span>
                })}
            </td>
        </tr>
    }
}

/// The trackers that announce on IRC, to pick one from.
fn open_pick_tracker(reload: impl Fn() + Copy + Send + Sync + 'static) {
    let (words, set_words) = signal(String::new());
    let defs = LocalResource::new(|| async { api::get("/releases/definitions").await.map_err(|e| e.message) });
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let pick = move |id: String, name: String| {
        if let Some(m) = holder.get_value() {
            m.close();
        }
        open_source(json!({ "kind": "irc", "definition": id, "name": name, "enabled": true, "settings": {} }), reload);
    };
    let body = move || {
        let list = move || match defs.get() {
            None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
            Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
            Some(Ok(v)) => {
                let w = words.get().to_lowercase();
                let shown: Vec<Value> = v
                    .get("definitions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| w.is_empty() || s(d, "name").to_lowercase().contains(&w) || s(d, "id").contains(&w))
                    .take(80)
                    .collect();
                if shown.is_empty() {
                    return view! { <p class="muted">"No tracker matches."</p> }.into_any();
                }
                view! {
                    <div class="list">{shown.into_iter().map(|d| {
                        let (id, name) = (s(&d, "id"), s(&d, "name"));
                        view! {
                            <button class="list-item" style="width:100%;text-align:left;gap:10px" on:click=move |_| pick(id.clone(), name.clone())>
                                <div class="grow" style="min-width:0"><div style="font-weight:650">{s(&d, "name")}</div><div class="small faint ellipsis">{format!("{} · {}", s(&d, "network"), s(&d, "server"))}</div></div>
                                <span class="badge">{s(&d, "privacy")}</span>
                            </button>
                        }
                    }).collect_view()}</div>
                }
                .into_any()
            }
        };
        view! {
            <label class="search" style="margin-bottom:12px;display:flex"><span inner_html=icon("search")></span>
                <input class="input" placeholder="Search trackers" autofocus on:input=move |e| set_words.set(event_target_value(&e))/></label>
            <div style="max-height:60vh;overflow:auto">{list}</div>
        }
    };
    let m = modal::open(ModalOpts::with_view("Add IRC announces", body).wide(true));
    holder.set_value(Some(m));
}

/// The form fields of an IRC source: the tracker's settings and the connection overrides.
pub fn irc_fields(def: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = def
        .get("settings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|st| {
            let label = if s(st, "label").is_empty() { s(st, "name") } else { s(st, "label") };
            let secret = st.get("secret").and_then(Value::as_bool) == Some(true);
            let mut f = json!({ "key": s(st, "name"), "label": label, "type": if secret { "password" } else { "text" } });
            if !s(st, "help").is_empty() {
                f["help"] = json!(s(st, "help"));
            }
            f
        })
        .collect();
    if !out.iter().any(|f| s(f, "key") == "nick") {
        out.insert(0, json!({ "key": "nick", "label": "Nick", "type": "text" }));
    }
    out.push(json!({ "key": "server", "label": "Server (instead of the tracker's)", "type": "text", "advanced": true, "help": format!("Empty: {}:{}", s(def, "server"), s(def, "port")) }));
    out.push(json!({ "key": "port", "label": "Port", "type": "text", "advanced": true }));
    out.push(json!({ "key": "channels", "label": "Channels (instead of the tracker's)", "type": "text", "advanced": true, "help": "Comma separated, a key after a space: #announce key" }));
    out
}

/// The Add/Edit source dialog.
fn open_source(src: Value, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = src.get("id").and_then(Value::as_i64);
    let kind = s(&src, "kind");
    let title = match (id.is_some(), kind.as_str()) {
        (true, _) => format!("Edit {}", s(&src, "name")),
        (false, "irc") => format!("Add {} announces", s(&src, "name")),
        (false, "rss") => "Add RSS feed".to_string(),
        _ => "Watch an indexer".to_string(),
    };
    let extra = LocalResource::new({
        let (kind, def) = (kind.clone(), s(&src, "definition"));
        move || {
            let (kind, def) = (kind.clone(), def.clone());
            async move {
                match kind.as_str() {
                    "irc" => api::get(&format!("/releases/definitions/{def}")).await.map_err(|e| e.message),
                    "indexer" => api::get("/indexers").await.map_err(|e| e.message),
                    _ => Ok(Value::Null),
                }
            }
        }
    });
    let (busy, set_busy) = signal(false);
    let (problem, set_problem) = signal(None::<String>);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let fields_of = {
        let kind = kind.clone();
        move |d: &Value| -> Vec<Value> {
            match kind.as_str() {
                "irc" => irc_fields(d),
                _ => Vec::new(),
            }
        }
    };
    let src2 = src.clone();
    let k2 = kind.clone();
    let f2 = fields_of.clone();
    let body = move || {
        let (src, kind, fields_of) = (src2.clone(), k2.clone(), f2.clone());
        move || match extra.get() {
            None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
            Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
            Some(Ok(d)) => {
                let fields: Vec<Field> = fields_of(&d).iter().map(Field::from_json).collect();
                let mut values = src.get("settings").cloned().unwrap_or_else(|| json!({}));
                values["_meta"] = json!({});
                let indexers = d.get("indexers").and_then(Value::as_array).cloned().unwrap_or_default();
                let chosen = src.get("indexer_id").and_then(Value::as_i64).unwrap_or(0);
                view! {
                    <div class="form-grid">
                        <label class="field"><span>"Name"</span><input class="input" data-src="name" value=s(&src, "name")/></label>
                        {(kind == "rss").then(|| view! {
                            <label class="field full"><span>"Feed address"</span><input class="input" data-src="url" value=s(&src, "url") placeholder="https://tracker.example/rss?passkey=…"/>
                                <small>"The address with its passkey stays saved; Lantern shows it without the part after ?."</small></label>
                        })}
                        {(kind == "indexer").then(|| view! {
                            <label class="field"><span>"Indexer"</span><select class="input" data-src="indexer_id">
                                {indexers.into_iter().map(|ix| { let i = ix.get("id").and_then(Value::as_i64).unwrap_or(0); view! { <option value=i.to_string() selected=i == chosen>{s(&ix, "name")}</option> } }).collect_view()}
                            </select></label>
                        })}
                        {(kind != "irc").then(|| view! {
                            <label class="field"><span>"Look every (minutes)"</span><input class="input" type="number" min="2" data-src="interval_min" value=s(&src, "interval_min")/></label>
                        })}
                    </div>
                    {(kind == "irc").then(|| view! { <p class="small muted">{format!("Lantern joins {} on {}. The tracker's site tells you the nick to use and your IRC key or invite command.", d.get("channels").and_then(Value::as_array).map(|c| c.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(), s(&d, "server"))}</p> })}
                    <div class="mt" id="src-settings"><SchemaFields fields=fields values=values/></div>
                    {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
                }
                .into_any()
            }
        }
    };
    let save = move |_| {
        let (kind, src, fields_of) = (kind.clone(), src.clone(), fields_of.clone());
        set_busy.set(true);
        set_problem.set(None);
        spawn(async move {
            let Some(m) = holder.get_value() else { return };
            let read = |k: &str| {
                m.qs(&format!("[data-src=\"{k}\"]"))
                    .map(|e| {
                        if let Some(i) = e.dyn_ref::<web_sys::HtmlInputElement>() {
                            i.value()
                        } else if let Some(sel) = e.dyn_ref::<web_sys::HtmlSelectElement>() {
                            sel.value()
                        } else {
                            String::new()
                        }
                    })
                    .unwrap_or_default()
            };
            let mut body = json!({ "name": read("name"), "kind": kind });
            if kind == "rss" {
                body["url"] = json!(read("url"));
            }
            if kind == "indexer" {
                body["indexer_id"] = json!(read("indexer_id").parse::<i64>().unwrap_or(0));
            }
            if kind != "irc" {
                body["interval_min"] = json!(read("interval_min").parse::<i64>().unwrap_or(10));
            }
            if let (Some(root), Some(Ok(d))) = (m.qs("#src-settings"), extra.get()) {
                let g = Group::from_json(&json!({ "id": "src", "fields": fields_of(&d) }));
                let mut settings = form_values(&g, &root, &json!({}));
                // a secret left empty keeps the saved one
                if let Some(saved) = src.get("settings").and_then(Value::as_object) {
                    for (k, v) in saved {
                        if v.as_str() == Some(MASK) && !settings.contains_key(k) {
                            settings.insert(k.clone(), json!(MASK));
                        }
                    }
                }
                body["settings"] = Value::Object(settings);
                body["definition"] = json!(s(&src, "definition"));
            }
            let r = match id {
                Some(id) => api::put(&format!("/releases/sources/{id}"), body).await,
                None => api::post("/releases/sources", body).await,
            };
            match r {
                Ok(_) => {
                    toast("Source saved", "success");
                    reload();
                    m.close();
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
            set_busy.set(false);
        })
    };
    let footer = move || {
        let save = save.clone();
        view! { <button class="btn primary" disabled=busy on:click=save>{move || if busy.get() { "Saving…" } else { "Save" }}</button> }
    };
    let m = modal::open(ModalOpts::with_view(&title, body).footer_view(footer).wide(true));
    holder.set_value(Some(m));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_round_trips_through_the_form() {
        let saved = json!({
            "id": 3, "name": "HD", "resolutions": ["1080p", "2160p"], "priority": 5,
            "actions": [{ "kind": "arr", "app": "radarr", "url": "http://radarr:7878", "api_key": "••••" }, { "kind": "client", "category": "x" }]
        });
        let v = filter_values(&saved);
        assert_eq!(v["resolutions"], "1080p,2160p");
        assert_eq!(v["app"], "radarr");
        let mut form = Map::new();
        form.insert("name".into(), json!("HD"));
        form.insert("resolutions".into(), json!("1080p,2160p"));
        form.insert("priority".into(), json!("7"));
        form.insert("app".into(), json!("radarr"));
        form.insert("url".into(), json!("http://radarr:7878"));
        let b = filter_body(&form, &saved);
        assert_eq!(b["resolutions"], json!(["1080p", "2160p"]));
        assert_eq!(b["priority"], 7);
        assert_eq!(b["actions"][0]["api_key"], MASK, "an untouched API key keeps the saved one");
        assert_eq!(b["actions"][1]["kind"], "client", "other actions stay");
        form.insert("url".into(), json!(""));
        assert_eq!(filter_body(&form, &json!({}))["actions"], json!([]));
    }

    #[test]
    fn words_for_statuses_and_times() {
        assert_eq!(status_words("pushed").1, "Sent");
        assert_eq!(status_words("rejected").1, "Skipped");
        assert_eq!(ago(0.0, 100.0), "never");
        assert_eq!(ago(100.0, 110.0), "just now");
        assert_eq!(ago(0.5, 3600.5), "1 h ago");
        let irc = json!({ "kind": "irc", "enabled": true, "irc": { "connected": true, "channels": ["#a"] } });
        assert_eq!(source_status(&irc), ("ok", "In #a".to_string()));
        assert_eq!(source_status(&json!({ "kind": "rss", "enabled": true, "last_error": "down" })).0, "err");
        let f = irc_fields(
            &json!({ "server": "irc.x", "port": 6697, "settings": [{ "name": "passkey", "label": "Passkey", "secret": true }] }),
        );
        assert_eq!(f[0]["key"], "nick");
        assert_eq!(f[1]["type"], "password");
    }
}
