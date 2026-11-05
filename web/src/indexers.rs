//! The indexer pages: `#/indexers` (the list, Add, Edit, Test, on/off, Remove) and `#/search` (search every
//! indexer). The settings of a site come from its definition and are drawn by the kit's schema form.

use kit_leptos::api;
use kit_leptos::icons::icon;
use kit_leptos::modal::{self, ModalOpts};
use kit_leptos::schema_form::schema::{Field, Group};
use kit_leptos::schema_form::{form_values, SchemaFields};
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Bytes in words (`1.5 GB`).
pub fn size_text(bytes: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{v:.0} {}", units[u])
    } else {
        format!("{v:.1} {}", units[u])
    }
}

/// Minutes in words (`3 h`, `2 d`).
pub fn age_text(minutes: f64) -> String {
    let m = minutes.max(0.0);
    if m < 60.0 {
        format!("{m:.0} min")
    } else if m < 48.0 * 60.0 {
        format!("{:.0} h", m / 60.0)
    } else if m < 60.0 * 24.0 * 365.0 {
        format!("{:.0} d", m / 1440.0)
    } else {
        format!("{:.1} y", m / 525_600.0)
    }
}

/// The form fields of a definition's settings (`GET /api/indexers/definitions/{id}`), as the kit's schema form
/// draws them: text, password, checkbox → switch, select; info lines become help on the field before them.
pub fn definition_fields(def: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for st in def.get("settings").and_then(Value::as_array).cloned().unwrap_or_default() {
        let kind = s(&st, "type");
        let label = s(&st, "label");
        if kind.starts_with("info") {
            let text = if kind == "info_cookie" {
                "Sign in on the site with your browser, open the developer tools (F12) → Network, reload, and copy the Cookie header of the request.".to_string()
            } else if kind == "info_flaresolverr" {
                "This site may be behind a Cloudflare check.".to_string()
            } else {
                strip_tags(&s(&st, "default"))
            };
            if let Some(last) = out.last_mut() {
                if !text.is_empty() && last.get("help").is_none() {
                    last["help"] = json!(text);
                }
            }
            continue;
        }
        let (t, options) = match kind.as_str() {
            "checkbox" => ("bool", Vec::new()),
            "password" => ("password", Vec::new()),
            "select" | "multi-select" => (
                if kind == "select" { "select" } else { "checks" },
                st.get("options")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|o| json!({ "value": o[0], "label": o[1] }))
                    .collect(),
            ),
            _ if st.get("secret").and_then(Value::as_bool) == Some(true) => ("password", Vec::new()),
            _ => ("text", Vec::new()),
        };
        out.push(
            json!({ "key": s(&st, "name"), "label": if label.is_empty() { s(&st, "name") } else { label }, "type": t, "options": options }),
        );
    }
    out
}

/// The text of a little HTML (`<b>x</b>` → `x`).
pub fn strip_tags(h: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in h.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").trim().to_string()
}

/// The form fields of a Newznab/Torznab indexer.
pub fn api_fields() -> Vec<Value> {
    vec![
        json!({ "key": "apikey", "label": "API key", "type": "password" }),
        json!({ "key": "api_path", "label": "API path", "type": "text", "help": "Usually /api" }),
        json!({ "key": "categories", "label": "Categories", "type": "text", "advanced": true, "help": "The standard category numbers it has, with commas (2000,5000); empty for all" }),
        json!({ "key": "extra", "label": "Extra parameters", "type": "text", "advanced": true, "help": "Added to every request (&attrs=…)" }),
    ]
}

/// The status of an indexer: (dot class, words).
pub fn status_of(ix: &Value) -> (&'static str, String) {
    if ix.get("enabled").and_then(Value::as_bool) != Some(true) {
        return ("", "Off".into());
    }
    let err = s(ix, "last_error");
    if ix.get("on_hold_until").is_some_and(|v| !v.is_null()) {
        return ("err", format!("On hold after failures: {err}"));
    }
    if !err.is_empty() {
        return ("warn", err);
    }
    ("ok", "Working".into())
}

/// `#/indexers`.
#[component]
pub fn Indexers(admin: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let list = LocalResource::new(move || {
        tick.track();
        async { api::get("/indexers").await.map_err(|e| e.message) }
    });
    let update_defs = move |_| {
        rt_spawn(async move {
            toast("Downloading the newest indexer definitions…", "info");
            match api::post_empty("/indexers/definitions/update").await {
                Ok(v) => toast(&s(&v, "message"), "success"),
                Err(e) => toast(&e.message, "error"),
            }
        })
    };
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let items = v.get("indexers").and_then(Value::as_array).cloned().unwrap_or_default();
            if items.is_empty() {
                let n = v.get("definitions").and_then(Value::as_i64).unwrap_or(0);
                return view! {
                    <div class="empty" inner_html=format!(r#"{}<p class="muted">No indexers yet. Add one of the {n} sites Lantern knows, or a Usenet indexer.</p>"#, icon("search"))></div>
                }
                .into_any();
            }
            view! {
                <div class="table-wrap"><table class="table" id="indexers-table">
                    <thead><tr><th>""</th><th>"Name"</th><th>"Kind"</th><th>"Priority"</th><th>"Status"</th><th>""</th></tr></thead>
                    <tbody>{items.into_iter().map(|ix| indexer_row(ix, admin, reload)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    let add = move |_| open_add(reload);
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Indexers"</h1>
            <div class="row" style="gap:8px">
                {admin.then(|| view! { <button class="btn" on:click=update_defs inner_html=format!("{}Update definitions", icon("refresh"))></button> })}
                {admin.then(|| view! { <button class="btn primary" id="indexer-add" on:click=add inner_html=format!("{}Add indexer", icon("plus"))></button> })}
            </div>
        </div>
        <div class="card flat">{rows}</div>
        <p class="small muted mt">"Apps add each indexer as Torznab (Newznab for Usenet) at "<span class="mono">"http://<lantern>:9696/<id>/"</span>", or every indexer at once at "<span class="mono">"/all/"</span>", with Lantern's API key (Settings → General → Apps)."</p>
        {admin.then(|| view! { <Cloudflare/> })}
    }
}

/// The words for the solver's state (`GET /api/solver`): the browser and how many sites have kept cookies.
pub fn solver_words(v: &Value) -> String {
    let Some(browser) = v.get("browser").and_then(Value::as_str) else {
        return "No browser for Cloudflare checks: use the Lantern image, or set APP_CHROMIUM.".into();
    };
    let running = if v.get("running").and_then(Value::as_bool) == Some(true) { "open now" } else { "starts when a site needs it" };
    let sites = v.get("sites").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    let kept = match sites {
        0 => "no site's cookies kept yet".to_string(),
        1 => "cookies kept for 1 site".to_string(),
        n => format!("cookies kept for {n} sites"),
    };
    format!("{browser} · {running} · {kept}")
}

/// The Cloudflare checks card: the browser, the sites it got past, a test of any address, and the FlareSolverr
/// address for other apps.
#[component]
fn Cloudflare() -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let state = LocalResource::new(move || {
        tick.track();
        async { api::get("/solver").await.map_err(|e| e.message) }
    });
    let (url, set_url) = signal(String::new());
    let (busy, set_busy) = signal(false);
    let test = move |_| {
        let u = url.get_untracked();
        set_busy.set(true);
        rt_spawn(async move {
            match api::post("/solver/test", json!({ "url": u })).await {
                Ok(r) if r.get("ok").and_then(Value::as_bool) == Some(true) => toast(&s(&r, "message"), "success"),
                Ok(r) => toast(&s(&r, "message"), "error"),
                Err(e) => toast(&e.message, "error"),
            }
            set_busy.set(false);
            set_tick.update(|t| *t += 1);
        })
    };
    let words = move || match state.get() {
        None => String::new(),
        Some(Err(e)) => e,
        Some(Ok(v)) => solver_words(&v),
    };
    let port = move || match state.get() {
        Some(Ok(v)) => v.get("port").and_then(Value::as_u64).unwrap_or(0),
        _ => 0,
    };
    view! {
        <h2 class="mt">"Cloudflare checks"</h2>
        <div class="card flat" id="cloudflare">
            <p class="small" style="margin-top:0">{words}</p>
            <div class="row wrap" style="gap:8px">
                <input class="input grow" style="min-width:240px" placeholder="https://site.example/" prop:value=url on:input=move |e| set_url.set(event_target_value(&e))/>
                <button class="btn" disabled=move || busy.get() || url.get().trim().is_empty() on:click=test>{move || if busy.get() { "Opening…" } else { "Test address" }}</button>
            </div>
            <p class="small muted" style="margin-bottom:0">{move || match port() {
                0 => "The FlareSolverr API for other apps is off (APP_SOLVER_PORT=0).".to_string(),
                p => format!("Other apps that use FlareSolverr can use Lantern instead at http://<lantern>:{p}/."),
            }}</p>
        </div>
    }
}

fn rt_spawn(f: impl std::future::Future<Output = ()> + 'static) {
    kit_leptos::rt::spawn(f)
}

fn indexer_row(ix: Value, admin: bool, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = ix.get("id").and_then(Value::as_i64).unwrap_or(0);
    let (dot, words) = status_of(&ix);
    let kind = format!("{} · {}", if s(&ix, "protocol") == "usenet" { "Usenet" } else { "Torrent" }, s(&ix, "privacy"));
    let enabled = ix.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let name = s(&ix, "name");
    let (testing, set_testing) = signal(false);
    let test = move |_| {
        set_testing.set(true);
        rt_spawn(async move {
            match api::post_empty(&format!("/indexers/{id}/test")).await {
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
        rt_spawn(async move {
            if let Err(e) = api::put(&format!("/indexers/{id}"), json!({ "enabled": on })).await {
                toast(&e.message, "error");
            }
            reload();
        })
    };
    let name2 = name.clone();
    let remove = move |_| {
        let name = name2.clone();
        rt_spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Remove {name}?"))
                .text("Lantern stops searching it and apps lose it. Its history goes too.")
                .ok("Remove")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/indexers/{id}")).await {
                    Ok(_) => toast(&format!("{name} removed"), "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let edit_ix = ix.clone();
    let edit = move |_| open_edit(edit_ix.clone(), reload);
    view! {
        <tr data-id=id>
            <td style="width:24px"><span class=format!("dot {dot}")></span></td>
            <td><div style="font-weight:650">{name}</div><div class="small faint mono">{format!("#{id} · {}", s(&ix, "base_url"))}</div></td>
            <td><span class="badge">{kind}</span></td>
            <td>{s(&ix, "priority")}</td>
            <td class="small" style="max-width:340px"><span class="ellipsis" title=words.clone() style="display:block">{words.clone()}</span></td>
            <td style="white-space:nowrap;text-align:right">
                {admin.then(|| view! {
                    <span class="row" style="gap:8px;justify-content:flex-end">
                        <label class="switch" title="On"><input type="checkbox" checked=enabled on:change=toggle/><i></i></label>
                        <button class="btn sm" disabled=testing on:click=test>{move || if testing.get() { "Testing…" } else { "Test" }}</button>
                        <button class="btn sm" on:click=edit>"Edit"</button>
                        <button class="btn sm ghost" title="Remove" on:click=remove inner_html=icon("trash")></button>
                    </span>
                })}
            </td>
        </tr>
    }
}

/// What the Add dialog is narrowed to: words, protocol (`torrent`/`usenet`), privacy and language; "" is any.
#[derive(Clone, Default, PartialEq)]
pub struct CatalogFilter {
    /// Words found in the name, id or description.
    pub words: String,
    /// `torrent` or `usenet`.
    pub protocol: String,
    /// `private`, `semi-private` or `public`.
    pub privacy: String,
    /// A language (`en-US`).
    pub language: String,
}

impl CatalogFilter {
    /// Whether a site of `/api/indexers/catalog` is wanted (the same rules as the server's).
    pub fn wants(&self, site: &Value) -> bool {
        let w = self.words.trim().to_lowercase();
        (w.is_empty()
            || s(site, "name").to_lowercase().contains(&w)
            || s(site, "id").to_lowercase().contains(&w)
            || s(site, "description").to_lowercase().contains(&w))
            && (self.protocol.is_empty() || s(site, "protocol") == self.protocol)
            && (self.privacy.is_empty() || s(site, "privacy") == self.privacy)
            && (self.language.is_empty() || s(site, "language") == self.language)
    }
}

/// How many sites a filter shows, in words: "24 of 560 sites (536 torrent, 24 Usenet)".
pub fn catalog_count_text(shown: usize, counts: &Value) -> String {
    let n = |k: &str| counts.get("protocol").and_then(|p| p.get(k)).and_then(Value::as_u64).unwrap_or(0);
    let all = counts.get("all").and_then(Value::as_u64).unwrap_or(0);
    format!("{shown} of {all} sites ({} torrent, {} Usenet)", n("torrent"), n("usenet"))
}

/// The words for a privacy (`semi-private` → "Semi-private").
fn privacy_text(p: &str) -> String {
    let mut c = p.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
}

/// What Add hands to the Edit dialog for a picked site: a Usenet preset comes with its address, API path and
/// categories, so only the API key is left to fill in.
pub fn add_body(site: &Value) -> Value {
    let kind = s(site, "kind");
    if kind == "cardigann" {
        return json!({ "kind": "cardigann", "definition": s(site, "id"), "name": s(site, "name") });
    }
    let cats: Vec<String> = site
        .get("categories")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).map(|c| c.to_string()).collect())
        .unwrap_or_default();
    let custom = site.get("custom").and_then(Value::as_bool) == Some(true);
    let path = s(site, "api_path");
    json!({
        "kind": kind, "definition": s(site, "id"), "name": if custom { String::new() } else { s(site, "name") },
        "base_url": s(site, "url"),
        "settings": { "api_path": if path.is_empty() { "/api".to_string() } else { path }, "categories": cats.join(",") },
    })
}

/// The Add dialog: one searchable list of torrent sites and Usenet indexers, narrowed by protocol, privacy and
/// language, with how many there are.
fn open_add(reload: impl Fn() + Copy + Send + Sync + 'static) {
    let (filter, set_filter) = signal(CatalogFilter::default());
    let catalog = LocalResource::new(|| async { api::get("/indexers/catalog").await.map_err(|e| e.message) });
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let pick = move |body: Value| {
        if let Some(m) = holder.get_value() {
            m.close();
        }
        open_edit(body, reload);
    };
    let body = move || {
        let languages = move || {
            let mut langs: Vec<(String, u64)> = match catalog.get() {
                Some(Ok(v)) => v
                    .pointer("/counts/language")
                    .and_then(Value::as_object)
                    .map(|m| m.iter().map(|(k, n)| (k.clone(), n.as_u64().unwrap_or(0))).collect())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            langs.sort();
            langs.into_iter().map(|(l, n)| view! { <option value=l.clone()>{format!("{l} ({n})")}</option> }).collect_view()
        };
        let list = move || match catalog.get() {
            None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
            Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
            Some(Ok(v)) => {
                let f = filter.get();
                let all = v.get("sites").and_then(Value::as_array).cloned().unwrap_or_default();
                let wanted: Vec<Value> = all.into_iter().filter(|x| f.wants(x)).collect();
                let count = catalog_count_text(
                    wanted.iter().filter(|x| x.get("custom").and_then(Value::as_bool) != Some(true)).count(),
                    &v["counts"],
                );
                let more = wanted.len().saturating_sub(100);
                let rows = wanted.into_iter().take(100).map(|x| {
                    let x2 = x.clone();
                    let usenet = s(&x, "protocol") == "usenet";
                    let sub = if usenet && !s(&x, "url").is_empty() { format!("{}{}", s(&x, "url"), s(&x, "api_path")) } else { s(&x, "description") };
                    view! {
                        <button class="list-item" style="width:100%;text-align:left;gap:10px" on:click=move |_| pick(add_body(&x2))>
                            <div class="grow" style="min-width:0"><div style="font-weight:650">{s(&x, "name")}</div><div class=if usenet { "small faint ellipsis mono" } else { "small faint ellipsis" }>{sub}</div></div>
                            <span class="badge">{if usenet { "Usenet" } else { "Torrent" }}</span>
                            {(!s(&x, "privacy").is_empty()).then(|| view! { <span class="badge">{privacy_text(&s(&x, "privacy"))}</span> })}
                            <span class="small faint">{s(&x, "language")}</span>
                        </button>
                    }
                }).collect_view();
                view! {
                    <p class="small muted" style="margin:0 0 8px">{count}</p>
                    <div class="list">{rows}</div>
                    {(more > 0).then(|| view! { <p class="small muted">{format!("{more} more: search or filter to find them.")}</p> })}
                    <p class="small muted">"Missing a site? Update definitions on the Indexers page gets the newest ones."</p>
                }
                .into_any()
            }
        };
        let seg = |value: &'static str, label: &'static str| {
            view! { <button class:on=move || filter.with(|f| f.protocol == value) on:click=move |_| set_filter.update(|f| f.protocol = value.into())>{label}</button> }
        };
        view! {
            <div class="row wrap" style="gap:10px;margin-bottom:12px">
                <label class="search grow" style="min-width:200px"><span inner_html=icon("search")></span>
                    <input class="input" placeholder="Search sites" autofocus on:input=move |e| set_filter.update(|f| f.words = event_target_value(&e))/></label>
                <div class="seg">{seg("", "All")}{seg("torrent", "Torrent")}{seg("usenet", "Usenet")}</div>
                <select class="input" style="width:auto" title="Who can join" on:change=move |e| set_filter.update(|f| f.privacy = event_target_value(&e))>
                    <option value="">"Any privacy"</option><option value="private">"Private"</option>
                    <option value="semi-private">"Semi-private"</option><option value="public">"Public"</option>
                </select>
                <select class="input" style="width:auto" title="Language" on:change=move |e| set_filter.update(|f| f.language = event_target_value(&e))>
                    <option value="">"Any language"</option>{languages}
                </select>
            </div>
            <div style="max-height:60vh;overflow:auto">{list}</div>
        }
    };
    let m = modal::open(ModalOpts::with_view("Add indexer", body).wide(true));
    holder.set_value(Some(m));
}

/// The Edit dialog (also the second step of Add): name, address, the site's settings, priority; Save and Test.
fn open_edit(ix: Value, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = ix.get("id").and_then(Value::as_i64);
    let kind = s(&ix, "kind");
    let def_id = s(&ix, "definition");
    let title = if id.is_some() {
        format!("Edit {}", s(&ix, "name"))
    } else {
        format!("Add {}", if s(&ix, "name").is_empty() { "indexer".to_string() } else { s(&ix, "name") })
    };
    let def = LocalResource::new({
        let def_id = def_id.clone();
        let kind = kind.clone();
        move || {
            let (def_id, kind) = (def_id.clone(), kind.clone());
            async move {
                if kind == "cardigann" {
                    api::get(&format!("/indexers/definitions/{def_id}")).await.map_err(|e| e.message)
                } else {
                    Ok(Value::Null)
                }
            }
        }
    });
    let (busy, set_busy) = signal(false);
    let (result, set_result) = signal(None::<(bool, String)>);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let saved_id: StoredValue<Option<i64>> = StoredValue::new(id);
    let ix2 = ix.clone();
    let body = move || {
        let ix = ix2.clone();
        let kind = kind.clone();
        move || match def.get() {
            None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
            Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
            Some(Ok(d)) => {
                let fields: Vec<Value> = if kind == "cardigann" { definition_fields(&d) } else { api_fields() };
                let fields: Vec<Field> = fields.iter().map(Field::from_json).collect();
                let mut values = ix.get("settings").cloned().unwrap_or_else(|| json!({}));
                if let (Some(m), Some(list)) = (values.as_object_mut(), d.get("settings").and_then(Value::as_array)) {
                    for st in list {
                        let k = s(st, "name");
                        if !m.contains_key(&k) {
                            let def = st.get("default").cloned().unwrap_or(Value::Null);
                            let v =
                                if s(st, "type") == "checkbox" { json!(def.as_str() == Some("true") || def == json!(true)) } else { def };
                            m.insert(k, v);
                        }
                    }
                }
                values["_meta"] = json!({});
                let links: Vec<String> = d
                    .get("links")
                    .and_then(Value::as_array)
                    .map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let base = s(&ix, "base_url");
                let base = if base.is_empty() { links.first().cloned().unwrap_or_default() } else { base };
                let desc = s(&d, "description");
                view! {
                    {(!desc.is_empty()).then(|| view! { <p class="small muted" style="margin-top:0">{desc.clone()}</p> })}
                    <div class="form-grid">
                        <label class="field"><span>"Name"</span><input class="input" data-ix="name" value=s(&ix, "name")/></label>
                        <label class="field"><span>{if kind == "cardigann" { "Site address" } else { "API address (without /api)" }}</span>
                            <input class="input" data-ix="base_url" list="ix-links" value=base/>
                            <datalist id="ix-links">{links.into_iter().map(|l| view! { <option value=l/> }).collect_view()}</datalist></label>
                        <label class="field"><span>"Priority"</span><input class="input" type="number" min="1" max="50" data-ix="priority" value=ix.get("priority").and_then(Value::as_i64).unwrap_or(25).to_string()/>
                            <small>"1 is searched first; apps also see it."</small></label>
                    </div>
                    <div class="mt" id="ix-settings"><SchemaFields fields=fields values=values/></div>
                    {move || result.get().map(|(ok, msg)| view! { <div class=if ok { "test-result ok mt" } else { "error-box mt" }>{msg}</div> })}
                }
                .into_any()
            }
        }
    };
    let kind2 = s(&ix, "kind");
    let def2 = def_id.clone();
    let save = move |test_after: bool| {
        let (kind, def_id) = (kind2.clone(), def2.clone());
        set_busy.set(true);
        set_result.set(None);
        rt_spawn(async move {
            let Some(m) = holder.get_value() else { return };
            let read = |k: &str| {
                m.qs(&format!("[data-ix=\"{k}\"]"))
                    .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
                    .map(|i| i.value())
                    .unwrap_or_default()
            };
            let settings = match (m.qs("#ix-settings"), def.get()) {
                (Some(root), Some(Ok(d))) => {
                    let fields = if kind == "cardigann" { definition_fields(&d) } else { api_fields() };
                    let g = Group::from_json(&json!({ "id": "ix", "fields": fields }));
                    Value::Object(form_values(&g, &root, &json!({})))
                }
                _ => json!({}),
            };
            let mut body = json!({ "name": read("name"), "base_url": read("base_url"), "priority": read("priority").parse::<i64>().unwrap_or(25), "settings": settings });
            let r = match saved_id.get_value() {
                Some(id) => api::put(&format!("/indexers/{id}"), body).await,
                None => {
                    body["kind"] = json!(kind);
                    body["definition"] = json!(def_id);
                    api::post("/indexers", body).await
                }
            };
            match r {
                Ok(v) => {
                    let id = v.get("id").and_then(Value::as_i64);
                    saved_id.set_value(id);
                    reload();
                    if test_after {
                        if let Some(id) = id {
                            match api::post_empty(&format!("/indexers/{id}/test")).await {
                                Ok(t) => set_result.set(Some((t.get("ok").and_then(Value::as_bool) == Some(true), s(&t, "message")))),
                                Err(e) => set_result.set(Some((false, e.message))),
                            }
                            reload();
                        }
                    } else {
                        toast("Indexer saved", "success");
                        m.close();
                    }
                }
                Err(e) => set_result.set(Some((false, e.message))),
            }
            set_busy.set(false);
        });
    };
    let save2 = save.clone();
    let footer = move || {
        let (a, b) = (save.clone(), save2.clone());
        view! {
            <button class="btn" disabled=busy on:click=move |_| a(true)>{move || if busy.get() { "Working…" } else { "Save and test" }}</button>
            <button class="btn primary" disabled=busy on:click=move |_| b(false)>"Save"</button>
        }
    };
    let m = modal::open(ModalOpts::with_view(&title, body).footer_view(footer).wide(true));
    holder.set_value(Some(m));
}

/// `#/search`: one search over every indexer (or the ones picked).
#[component]
pub fn Search() -> impl IntoView {
    let (q, set_q) = signal(String::new());
    let (kind, set_kind) = signal("search".to_string());
    let (cat, set_cat) = signal(String::new());
    let (busy, set_busy) = signal(false);
    let (answer, set_answer) = signal(None::<Result<Value, String>>);
    let run = move || {
        let path = format!(
            "/search?q={}&type={}&cat={}",
            js_sys::encode_uri_component(&q.get_untracked()),
            kind.get_untracked(),
            js_sys::encode_uri_component(&cat.get_untracked())
        );
        set_busy.set(true);
        rt_spawn(async move {
            set_answer.set(Some(api::get(&path).await.map_err(|e| e.message)));
            set_busy.set(false);
        });
    };
    let results = move || match answer.get() {
        None => view! { <p class="muted">"Search every indexer at once. Empty words show each one's newest releases."</p> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let list = v.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
            let failed = v.get("failed").and_then(Value::as_array).cloned().unwrap_or_default();
            view! {
                {failed.into_iter().map(|f| view! { <div class="error-box" style="margin-bottom:8px">{format!("{}: {}", s(&f, "indexer"), s(&f, "error"))}</div> }).collect_view()}
                <p class="small muted">{format!("{} results", list.len())}</p>
                <div class="card flat"><div class="table-wrap"><table class="table" id="search-results">
                    <thead><tr><th>"Title"</th><th>"Indexer"</th><th>"Size"</th><th>"Age"</th><th>"Peers"</th><th>""</th></tr></thead>
                    <tbody>{list.into_iter().map(result_row).collect_view()}</tbody>
                </table></div></div>
            }
            .into_any()
        }
    };
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        run();
    };
    view! {
        <h1>"Search"</h1>
        <form class="row wrap" style="gap:10px" on:submit=submit>
            <label class="search grow" style="min-width:240px"><span inner_html=icon("search")></span>
                <input class="input" id="search-q" placeholder="Words, or tt0111161 with Movies" on:input=move |e| set_q.set(event_target_value(&e))/></label>
            <select class="input" style="width:auto" on:change=move |e| set_kind.set(event_target_value(&e))>
                <option value="search">"Anything"</option><option value="tvsearch">"TV"</option><option value="movie">"Movies"</option>
                <option value="music">"Music"</option><option value="book">"Books"</option>
            </select>
            <select class="input" style="width:auto" on:change=move |e| set_cat.set(event_target_value(&e))>
                <option value="">"All categories"</option><option value="2000">"Movies"</option><option value="5000">"TV"</option>
                <option value="3000">"Audio"</option><option value="7000">"Books"</option><option value="4000">"PC"</option><option value="1000">"Console"</option>
            </select>
            <button class="btn primary" type="submit" disabled=busy>{move || if busy.get() { "Searching…" } else { "Search" }}</button>
        </form>
        <div class="mt">{results}</div>
    }
}

fn result_row(r: Value) -> impl IntoView {
    let size = r.get("size").and_then(Value::as_f64).unwrap_or(0.0);
    let age = r.get("ageMinutes").and_then(Value::as_f64).unwrap_or(0.0);
    let peers = match (r.get("seeders").and_then(Value::as_i64), r.get("leechers").and_then(Value::as_i64)) {
        (Some(sd), Some(l)) => format!("{sd} / {l}"),
        _ => "—".into(),
    };
    let flags: Vec<String> = r
        .get("indexerFlags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let link = s(&r, "downloadUrl");
    let info = s(&r, "infoUrl");
    view! {
        <tr>
            <td style="max-width:520px"><div style="font-weight:600;word-break:break-word">{s(&r, "title")}</div>
                <div class="row" style="gap:6px;margin-top:4px">{flags.into_iter().map(|f| view! { <span class="badge ok">{f}</span> }).collect_view()}</div></td>
            <td class="small">{s(&r, "indexer")}</td>
            <td class="small" style="white-space:nowrap">{size_text(size)}</td>
            <td class="small" style="white-space:nowrap">{age_text(age)}</td>
            <td class="small">{peers}</td>
            <td style="white-space:nowrap;text-align:right">
                {(!info.is_empty()).then(|| view! { <a class="btn sm ghost" href=info.clone() target="_blank" rel="noreferrer" title="Open on the site" inner_html=icon("external")></a> })}
                <a class="btn sm" href=link download="">"Download"</a>
            </td>
        </tr>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_for_the_solver() {
        assert!(solver_words(&json!({ "browser": null })).contains("APP_CHROMIUM"));
        let w = solver_words(&json!({ "browser": "/usr/lib/chromium/headless_shell", "running": false, "sites": [{ "host": "a" }] }));
        assert_eq!(w, "/usr/lib/chromium/headless_shell · starts when a site needs it · cookies kept for 1 site");
    }

    #[test]
    fn words_for_sizes_ages_and_status() {
        assert_eq!(size_text(1_610_612_736.0), "1.5 GB");
        assert_eq!(size_text(512.0), "512 B");
        assert_eq!(age_text(30.0), "30 min");
        assert_eq!(age_text(180.0), "3 h");
        assert_eq!(age_text(4320.0), "3 d");
        assert_eq!(status_of(&json!({ "enabled": false })).1, "Off");
        assert_eq!(status_of(&json!({ "enabled": true, "last_error": "", "on_hold_until": null })).0, "ok");
        assert_eq!(status_of(&json!({ "enabled": true, "last_error": "down", "on_hold_until": 5 })).0, "err");
    }

    #[test]
    fn one_add_list_for_torrent_and_usenet() {
        let sites = [
            json!({ "id": "alpha", "kind": "cardigann", "protocol": "torrent", "name": "Alpha", "description": "x", "privacy": "private", "language": "en-US" }),
            json!({ "id": "nzbgeek", "kind": "newznab", "protocol": "usenet", "name": "NZBgeek", "description": "Usenet indexer", "privacy": "private", "language": "en-US",
                    "url": "https://api.nzbgeek.info", "api_path": "/api", "categories": [2000, 5000] }),
            json!({ "id": "beta", "kind": "cardigann", "protocol": "torrent", "name": "Beta", "description": "y", "privacy": "public", "language": "fr-FR" }),
        ];
        let names = |f: CatalogFilter| sites.iter().filter(|x| f.wants(x)).map(|x| s(x, "name")).collect::<Vec<_>>();
        assert_eq!(names(CatalogFilter::default()).len(), 3);
        assert_eq!(names(CatalogFilter { protocol: "usenet".into(), ..Default::default() }), ["NZBgeek"]);
        assert_eq!(names(CatalogFilter { privacy: "public".into(), ..Default::default() }), ["Beta"]);
        assert_eq!(names(CatalogFilter { language: "en-US".into(), protocol: "torrent".into(), ..Default::default() }), ["Alpha"]);
        assert_eq!(names(CatalogFilter { words: "GEEK".into(), ..Default::default() }), ["NZBgeek"]);
        let counts = json!({ "all": 560, "protocol": { "torrent": 536, "usenet": 24 } });
        assert_eq!(catalog_count_text(24, &counts), "24 of 560 sites (536 torrent, 24 Usenet)");
        // a Usenet preset arrives filled in: only the API key is left
        let b = add_body(&sites[1]);
        assert_eq!(b["base_url"], "https://api.nzbgeek.info");
        assert_eq!(b["settings"], json!({ "api_path": "/api", "categories": "2000,5000" }));
        assert_eq!(add_body(&sites[0]), json!({ "kind": "cardigann", "definition": "alpha", "name": "Alpha" }));
        assert_eq!(privacy_text("semi-private"), "Semi-private");
    }

    #[test]
    fn definition_settings_become_form_fields() {
        let def = json!({ "settings": [
            { "name": "username", "type": "text", "label": "Username" },
            { "name": "password", "type": "password", "label": "Password" },
            { "name": "info", "type": "info", "label": "About", "default": "Use <b>100</b> per page" },
            { "name": "freeleech", "type": "checkbox", "label": "Freeleech only" },
            { "name": "sort", "type": "select", "label": "Sort", "options": [["added", "created"], ["seeders", "seeders"]] },
            { "name": "apikey", "type": "text", "label": "API key", "secret": true },
        ]});
        let f = definition_fields(&def);
        assert_eq!(f.len(), 5);
        assert_eq!(f[1]["type"], "password");
        assert_eq!(f[1]["help"], "Use 100 per page");
        assert_eq!(f[2]["type"], "bool");
        assert_eq!(f[3]["options"][0], json!({ "value": "added", "label": "created" }));
        assert_eq!(f[4]["type"], "password", "a secret text setting is a password field");
    }
}
