//! The download hub pages: `#/apps` (every app that downloads through Lantern, its category and folders, and
//! what it asked for, live) and `#/downloads` (the download clients: add, edit, test, on/off, remove).

use crate::indexers::{age_text, size_text};
use kit_leptos::api;
use kit_leptos::icons::icon;
use kit_leptos::modal::{self, ModalOpts};
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use wasm_bindgen::JsCast;

/// How often the Apps page asks for fresh progress, in milliseconds.
const REFRESH_MS: u64 = 4000;

/// The media types an app can say it handles: (value, words).
pub const MEDIA: [(&str, &str); 5] = [("tv", "TV"), ("movie", "Movies"), ("music", "Music"), ("book", "Books"), ("other", "Other")];

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// How an app reaches Lantern, in words: its own API, or Lantern standing in for a client it knows.
pub fn source_text(source: &str) -> String {
    match source {
        "" | "native" => "Lantern API".into(),
        "qbittorrent" => "as qBittorrent".into(),
        "sabnzbd" => "as SABnzbd".into(),
        "nzbget" => "as NZBGet".into(),
        other => other.to_string(),
    }
}

/// The words for a media type (`tv` → `TV`).
pub fn media_text(media: &str) -> String {
    MEDIA.iter().find(|(v, _)| *v == media).map(|(_, w)| w.to_string()).unwrap_or_else(|| media.to_string())
}

/// An app's downloads by state (`{"downloading": 2, "failed": 1}`) in words: `2 downloading · 1 failed`.
pub fn counts_text(by_state: &Value) -> String {
    let order = ["queued", "waiting", "downloading", "paused", "seeding", "completed", "failed", "removed"];
    let parts: Vec<String> =
        order.iter().filter_map(|k| by_state.get(*k).and_then(Value::as_i64).filter(|n| *n > 0).map(|n| format!("{n} {k}"))).collect();
    if parts.is_empty() {
        "Nothing yet".into()
    } else {
        parts.join(" · ")
    }
}

/// A grab's state in words, with its progress while it downloads: (badge class, words).
pub fn state_text(g: &Value) -> (&'static str, String) {
    let state = s(g, "state");
    let progress = g.get("progress").and_then(Value::as_f64).unwrap_or(0.0);
    match state.as_str() {
        "downloading" => ("", format!("Downloading {:.0}%", (progress * 100.0).clamp(0.0, 100.0))),
        "queued" => ("", "Waiting to send".into()),
        "waiting" => ("", "Waiting in the client".into()),
        "paused" => ("warn", "Paused".into()),
        "seeding" => ("ok", "Done, seeding".into()),
        "completed" => ("ok", "Done".into()),
        "failed" if g.get("next_try").and_then(Value::as_f64).unwrap_or(0.0) > 0.0 => ("warn", "Trying again soon".into()),
        "failed" => ("err", "Failed".into()),
        "removed" => ("", "Removed".into()),
        other => ("", other.to_string()),
    }
}

/// When something happened, from a time in seconds, in words (`5 min ago`, `never`).
pub fn ago_text(at: f64, now_secs: f64) -> String {
    if at <= 0.0 {
        "never".into()
    } else {
        format!("{} ago", age_text((now_secs - at) / 60.0))
    }
}

/// Now, in seconds.
fn now_secs() -> f64 {
    kit_leptos::rt::now_ms() as f64 / 1000.0
}

/// Runs a task on the page.
fn rt_spawn(f: impl std::future::Future<Output = ()> + 'static) {
    kit_leptos::rt::spawn(f)
}

/// The value of the input or select marked `data-f="<k>"` in an open dialog.
fn field(m: &modal::Modal, k: &str) -> String {
    let Some(e) = m.qs(&format!("[data-f=\"{k}\"]")) else { return String::new() };
    if let Some(i) = e.dyn_ref::<web_sys::HtmlInputElement>() {
        return i.value();
    }
    e.dyn_ref::<web_sys::HtmlSelectElement>().map(|i| i.value()).unwrap_or_default()
}

/// `#/apps`: the connected apps and their downloads, refreshed every few seconds while the page is open.
#[component]
pub fn Apps(admin: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    rt_spawn(async move {
        loop {
            kit_leptos::rt::sleep(REFRESH_MS).await;
            if !alive.load(Ordering::Relaxed) {
                break;
            }
            set_tick.try_update(|t| *t += 1);
        }
    });
    let apps = LocalResource::new(move || {
        tick.track();
        async { api::get("/hub/apps").await.map_err(|e| e.message) }
    });
    let grabs = LocalResource::new(move || {
        tick.track();
        async { api::get("/hub/grabs?limit=50").await.map_err(|e| e.message) }
    });
    let clients = LocalResource::new(|| async { api::get("/hub/clients").await.map_err(|e| e.message) });
    let app_rows = move || match apps.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let list = v.get("apps").and_then(Value::as_array).cloned().unwrap_or_default();
            if list.is_empty() {
                return view! {
                    <div class="empty" inner_html=format!(r#"{}<p class="muted">No apps yet. An app shows up here the first time it registers, or signs in to Lantern as its qBittorrent, SABnzbd or NZBGet.</p>"#, icon("download"))></div>
                }
                .into_any();
            }
            let now = now_secs();
            view! {
                <div class="table-wrap"><table class="table" id="apps-table">
                    <thead><tr><th>""</th><th>"App"</th><th>"Media"</th><th>"Category and folders"</th><th>"Downloads"</th><th>"Last seen"</th><th>""</th></tr></thead>
                    <tbody>{list.into_iter().map(|a| app_row(a, admin, now, reload)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    let grab_rows = move || {
        let names: Vec<(i64, String)> = match clients.get() {
            Some(Ok(c)) => c
                .get("clients")
                .and_then(Value::as_array)
                .map(|l| l.iter().map(|c| (c.get("id").and_then(Value::as_i64).unwrap_or(0), s(c, "name"))).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        match grabs.get() {
            None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
            Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
            Some(Ok(v)) => {
                let list = v.get("grabs").and_then(Value::as_array).cloned().unwrap_or_default();
                if list.is_empty() {
                    return view! { <p class="muted" style="margin:12px">"Nothing downloaded yet."</p> }.into_any();
                }
                view! {
                    <div class="table-wrap"><table class="table" id="grabs-table">
                        <thead><tr><th>"Title"</th><th>"App"</th><th>"Status"</th><th>"Client"</th><th>"Size"</th><th>""</th></tr></thead>
                        <tbody>{list.into_iter().map(|g| {
                            let cid = g.get("client_id").and_then(Value::as_i64).unwrap_or(0);
                            let client = names.iter().find(|(i, _)| *i == cid).map(|(_, n)| n.clone()).unwrap_or_else(|| "—".into());
                            grab_row(g, client, admin, reload)
                        }).collect_view()}</tbody>
                    </table></div>
                }
                .into_any()
            }
        }
    };
    let add = move |_| open_app(None, reload);
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Apps"</h1>
            {admin.then(|| view! { <button class="btn primary" id="app-add" on:click=add inner_html=format!("{}Add app", icon("plus"))></button> })}
        </div>
        <div class="card flat">{app_rows}</div>
        <p class="small muted mt">"Apps use Lantern as their download client: qBittorrent at "<span class="mono">"http://<lantern>:9696"</span>" with the app's name as user and Lantern's API key as password, SABnzbd at "<span class="mono">"/sabnzbd"</span>" with the API key, or NZBGet with the app's name and the API key."</p>
        <h2 class="mt">"Downloads"</h2>
        <div class="card flat">{grab_rows}</div>
    }
}

/// One row of the apps table.
fn app_row(a: Value, admin: bool, now: f64, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = a.get("id").and_then(Value::as_i64).unwrap_or(0);
    let name = s(&a, "name");
    let problem = s(&a, "callback_error");
    let (dot, title) =
        if problem.is_empty() { ("ok", "Working".to_string()) } else { ("warn", format!("Calling it back failed: {problem}")) };
    let folder = s(&a, "folder");
    let import = s(&a, "import_path");
    let seen = ago_text(a.get("last_seen").and_then(Value::as_f64).unwrap_or(0.0), now);
    let counts = counts_text(a.get("grabs").unwrap_or(&Value::Null));
    let name2 = name.clone();
    let remove = move |_| {
        let name = name2.clone();
        rt_spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Remove {name}?"))
                .text("Lantern forgets this app and its download list. The files and the downloads in the clients stay.")
                .ok("Remove")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/hub/apps/{id}")).await {
                    Ok(_) => toast(&format!("{name} removed"), "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let edit_a = a.clone();
    let edit = move |_| open_app(Some(edit_a.clone()), reload);
    view! {
        <tr data-id=id>
            <td style="width:24px"><span class=format!("dot {dot}") title=title></span></td>
            <td><div style="font-weight:650">{name}</div><span class="badge">{source_text(&s(&a, "source"))}</span></td>
            <td>{media_text(&s(&a, "media"))}</td>
            <td class="small">
                <div><span class="badge">{s(&a, "category")}</span></div>
                <div class="mono faint" title="Where Lantern puts its downloads">{folder.clone()}</div>
                {(import != folder).then(|| view! { <div class="mono faint" title="The same folder as the app sees it">{format!("→ {import}")}</div> })}
                {(!problem.is_empty()).then(|| view! { <div class="error-box" style="margin-top:4px">{format!("Calling it back failed: {problem}")}</div> })}
            </td>
            <td class="small">{counts}</td>
            <td class="small" style="white-space:nowrap">{seen}</td>
            <td style="white-space:nowrap;text-align:right">
                {admin.then(|| view! {
                    <span class="row" style="gap:8px;justify-content:flex-end">
                        <button class="btn sm" on:click=edit>"Edit"</button>
                        <button class="btn sm ghost" title="Remove" on:click=remove inner_html=icon("trash")></button>
                    </span>
                })}
            </td>
        </tr>
    }
}

/// One row of the downloads table.
fn grab_row(g: Value, client: String, admin: bool, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = g.get("id").and_then(Value::as_i64).unwrap_or(0);
    let (badge, words) = state_text(&g);
    let state = s(&g, "state");
    let error = s(&g, "error");
    let path = s(&g, "path");
    let size = g.get("size").and_then(Value::as_f64).unwrap_or(0.0);
    let retry = move |_| {
        rt_spawn(async move {
            match api::post_empty(&format!("/hub/grabs/{id}/retry")).await {
                Ok(_) => toast("Trying it again", "success"),
                Err(e) => toast(&e.message, "error"),
            }
            reload();
        })
    };
    let title = s(&g, "title");
    let title2 = title.clone();
    let remove = move |_| {
        let title = title2.clone();
        rt_spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Stop {title}?"))
                .text("Lantern takes it out of the download client. Its files stay.")
                .ok("Stop")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/hub/grabs/{id}")).await {
                    Ok(_) => toast("Stopped", "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let can_retry = state == "failed";
    let can_stop = state != "removed";
    view! {
        <tr data-id=id>
            <td style="max-width:460px"><div style="font-weight:600;word-break:break-word">{title}</div>
                {(!path.is_empty()).then(|| view! { <div class="small faint mono" style="word-break:break-all">{path.clone()}</div> })}
                {(!error.is_empty()).then(|| view! { <div class="error-box" style="margin-top:4px">{error.clone()}</div> })}</td>
            <td class="small">{s(&g, "app")}</td>
            <td><span class=format!("badge {badge}")>{words}</span></td>
            <td class="small">{client}</td>
            <td class="small" style="white-space:nowrap">{if size > 0.0 { size_text(size) } else { "—".into() }}</td>
            <td style="white-space:nowrap;text-align:right">
                {admin.then(|| view! {
                    <span class="row" style="gap:8px;justify-content:flex-end">
                        {can_retry.then(|| view! { <button class="btn sm" on:click=retry>"Try again"</button> })}
                        {can_stop.then(|| view! { <button class="btn sm ghost" title="Stop" on:click=remove.clone() inner_html=icon("trash")></button> })}
                    </span>
                })}
            </td>
        </tr>
    }
}

/// The Add app dialog, or Edit when `app` is given: name (Add only), media type, the app's import path and
/// where to call it back.
fn open_app(app: Option<Value>, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let a = app.clone().unwrap_or_else(|| json!({ "media": "other" }));
    let id = a.get("id").and_then(Value::as_i64);
    let title = match id {
        Some(_) => format!("Edit {}", s(&a, "name")),
        None => "Add app".into(),
    };
    let (problem, set_problem) = signal(None::<String>);
    let (busy, set_busy) = signal(false);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let media = s(&a, "media");
    let a2 = a.clone();
    let body = move || {
        let a = a2.clone();
        let media = media.clone();
        view! {
            <div class="form-grid">
                {id.is_none().then(|| view! { <label class="field"><span>"Name"</span><input class="input" data-f="name" placeholder="Sonarr"/></label> })}
                <label class="field"><span>"Media"</span>
                    <select class="input" data-f="media">{MEDIA.iter().map(|(v, w)| view! { <option value=*v selected=media == *v>{*w}</option> }).collect_view()}</select></label>
                <label class="field"><span>"Import folder"</span><input class="input" data-f="import_path" value=s(&a, "import_path") placeholder="/data/downloads/sonarr"/>
                    <small>"The app's own path to its download folder, when it sees it at another place than Lantern does."</small></label>
                <label class="field"><span>"Call back at"</span><input class="input" data-f="callback_url" value=s(&a, "callback_url") placeholder="http://sonarr:8989/lantern"/>
                    <small>"Lantern posts every change of a download here. Leave it empty if the app asks on its own."</small></label>
            </div>
            {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
        }
    };
    let name = s(&a, "name");
    let save = move |_| {
        let name = name.clone();
        set_busy.set(true);
        rt_spawn(async move {
            let Some(m) = holder.get_value() else { return };
            let body = json!({
                "name": if id.is_some() { name } else { field(&m, "name") },
                "media": field(&m, "media"),
                "import_path": field(&m, "import_path"),
                "callback_url": field(&m, "callback_url"),
            });
            let r = match id {
                Some(id) => api::put(&format!("/hub/apps/{id}"), body).await,
                None => api::post("/hub/apps", body).await,
            };
            match r {
                Ok(v) => {
                    reload();
                    match v.get("folder_error").and_then(Value::as_str) {
                        Some(p) if !p.is_empty() => toast(p, "error"),
                        _ => toast("App saved", "success"),
                    }
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
    let m = modal::open(ModalOpts::with_view(&title, body).footer_view(footer));
    holder.set_value(Some(m));
}

/// `#/downloads`: the download clients Lantern sends to.
#[component]
pub fn DownloadClients(admin: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let list = LocalResource::new(move || {
        tick.track();
        async { api::get("/hub/clients").await.map_err(|e| e.message) }
    });
    let kinds = move || match list.get() {
        Some(Ok(v)) => v.get("kinds").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let items = v.get("clients").and_then(Value::as_array).cloned().unwrap_or_default();
            if items.is_empty() {
                return view! {
                    <div class="empty" inner_html=format!(r#"{}<p class="muted">No download clients yet. Add qBittorrent for torrents and SABnzbd for Usenet to begin.</p>"#, icon("download"))></div>
                }
                .into_any();
            }
            let ks = kinds();
            view! {
                <div class="table-wrap"><table class="table" id="clients-table">
                    <thead><tr><th>"Name"</th><th>"Kind"</th><th>"Folder"</th><th>"Priority"</th><th>""</th></tr></thead>
                    <tbody>{items.into_iter().map(|c| client_row(c, ks.clone(), admin, reload)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    let add = move |_| open_client(json!({ "kind": "qbittorrent", "priority": 25 }), kinds(), reload);
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Download clients"</h1>
            {admin.then(|| view! { <button class="btn primary" id="client-add" on:click=add inner_html=format!("{}Add client", icon("plus"))></button> })}
        </div>
        <div class="card flat">{rows}</div>
        <p class="small muted mt">"Apps never see these: they ask Lantern, and Lantern picks the client for each download (lowest priority number first). A change here applies to every app at once."</p>
    }
}

/// One row of the clients table.
fn client_row(c: Value, kinds: Vec<Value>, admin: bool, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = c.get("id").and_then(Value::as_i64).unwrap_or(0);
    let name = s(&c, "name");
    let kind = s(&c, "kind");
    let label = kinds.iter().find(|k| s(k, "kind") == kind).map(|k| s(k, "name")).unwrap_or_else(|| kind.clone());
    let protocol = if s(&c, "protocol") == "usenet" { "Usenet" } else { "Torrent" };
    let enabled = c.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let (testing, set_testing) = signal(false);
    let test = move |_| {
        set_testing.set(true);
        rt_spawn(async move {
            match api::post_empty(&format!("/hub/clients/{id}/test")).await {
                Ok(r) if r.get("ok").and_then(Value::as_bool) == Some(true) => toast(&s(&r, "message"), "success"),
                Ok(r) => toast(&s(&r, "message"), "error"),
                Err(e) => toast(&e.message, "error"),
            }
            set_testing.set(false);
        })
    };
    let toggle = move |e: leptos::ev::Event| {
        let on = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()).is_some_and(|i| i.checked());
        rt_spawn(async move {
            if let Err(e) = api::put(&format!("/hub/clients/{id}"), json!({ "enabled": on })).await {
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
                .text("Lantern stops sending downloads to it. What it already has stays in it.")
                .ok("Remove")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/hub/clients/{id}")).await {
                    Ok(_) => toast(&format!("{name} removed"), "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let (edit_c, edit_k) = (c.clone(), kinds.clone());
    let edit = move |_| open_client(edit_c.clone(), edit_k.clone(), reload);
    let root = s(&c, "root");
    view! {
        <tr data-id=id>
            <td><div style="font-weight:650">{name}</div><div class="small faint mono">{s(&c, "url")}</div></td>
            <td><span class="badge">{format!("{label} · {protocol}")}</span></td>
            <td class="small mono">{if root.is_empty() { "same as Lantern".to_string() } else { root }}</td>
            <td>{s(&c, "priority")}</td>
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

/// The Add or Edit client dialog; Save and test saves first, then checks Lantern can reach it.
fn open_client(c: Value, kinds: Vec<Value>, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = c.get("id").and_then(Value::as_i64);
    let title = match id {
        Some(_) => format!("Edit {}", s(&c, "name")),
        None => "Add download client".into(),
    };
    let (result, set_result) = signal(None::<(bool, String)>);
    let (busy, set_busy) = signal(false);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let saved_id: StoredValue<Option<i64>> = StoredValue::new(id);
    let c2 = c.clone();
    let body = move || {
        let c = c2.clone();
        let kind = s(&c, "kind");
        let kinds = kinds.clone();
        view! {
            <div class="form-grid">
                <label class="field"><span>"Kind"</span>
                    <select class="input" data-f="kind" disabled=id.is_some()>{kinds.into_iter().map(|k| {
                        let v = s(&k, "kind");
                        let sel = v == kind;
                        view! { <option value=v.clone() selected=sel>{s(&k, "name")}</option> }
                    }).collect_view()}</select></label>
                <label class="field"><span>"Name"</span><input class="input" data-f="name" value=s(&c, "name") placeholder="Same as the kind"/></label>
                <label class="field"><span>"Address"</span><input class="input" data-f="url" value=s(&c, "url") placeholder="http://qbittorrent:8080"/></label>
                <label class="field"><span>"User name"</span><input class="input" data-f="username" value=s(&c, "username")/></label>
                <label class="field"><span>"Password"</span><input class="input" type="password" data-f="password" value=s(&c, "password")/></label>
                <label class="field"><span>"API key"</span><input class="input" type="password" data-f="api_key" value=s(&c, "api_key")/>
                    <small>"SABnzbd only."</small></label>
                <label class="field"><span>"Its download folder"</span><input class="input" data-f="root" value=s(&c, "root") placeholder="/downloads"/>
                    <small>"Where the client sees Lantern's download folder. Leave empty when it is the same path."</small></label>
                <label class="field"><span>"Priority"</span><input class="input" type="number" min="1" max="50" data-f="priority" value=c.get("priority").and_then(Value::as_i64).unwrap_or(25).to_string()/>
                    <small>"1 is used first."</small></label>
            </div>
            {move || result.get().map(|(ok, msg)| view! { <div class=if ok { "test-result ok mt" } else { "error-box mt" }>{msg}</div> })}
        }
    };
    let save = move |test_after: bool| {
        set_busy.set(true);
        set_result.set(None);
        rt_spawn(async move {
            let Some(m) = holder.get_value() else { return };
            let body = json!({
                "kind": field(&m, "kind"), "name": field(&m, "name"), "url": field(&m, "url"),
                "username": field(&m, "username"), "password": field(&m, "password"), "api_key": field(&m, "api_key"),
                "root": field(&m, "root"), "priority": field(&m, "priority").parse::<i64>().unwrap_or(25),
            });
            let r = match saved_id.get_value() {
                Some(id) => api::put(&format!("/hub/clients/{id}"), body).await,
                None => api::post("/hub/clients", body).await,
            };
            match r {
                Ok(v) => {
                    let id = v.get("id").and_then(Value::as_i64);
                    saved_id.set_value(id);
                    reload();
                    if test_after {
                        if let Some(id) = id {
                            match api::post_empty(&format!("/hub/clients/{id}/test")).await {
                                Ok(t) => set_result.set(Some((t.get("ok").and_then(Value::as_bool) == Some(true), s(&t, "message")))),
                                Err(e) => set_result.set(Some((false, e.message))),
                            }
                        }
                    } else {
                        toast("Client saved", "success");
                        m.close();
                    }
                }
                Err(e) => set_result.set(Some((false, e.message))),
            }
            set_busy.set(false);
        });
    };
    let footer = move || {
        view! {
            <button class="btn" disabled=busy on:click=move |_| save(true)>{move || if busy.get() { "Working…" } else { "Save and test" }}</button>
            <button class="btn primary" disabled=busy on:click=move |_| save(false)>"Save"</button>
        }
    };
    let m = modal::open(ModalOpts::with_view(&title, body).footer_view(footer).wide(true));
    holder.set_value(Some(m));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_say_how_they_connect_and_what_they_have() {
        assert_eq!(source_text("native"), "Lantern API");
        assert_eq!(source_text("qbittorrent"), "as qBittorrent");
        assert_eq!(source_text("nzbget"), "as NZBGet");
        assert_eq!(media_text("tv"), "TV");
        assert_eq!(media_text("comics"), "comics");
        assert_eq!(counts_text(&json!({ "failed": 1, "downloading": 2, "removed": 0 })), "2 downloading · 1 failed");
        assert_eq!(counts_text(&json!({})), "Nothing yet");
        assert_eq!(ago_text(0.0, 1000.0), "never");
        assert_eq!(ago_text(700.0, 1000.0), "5 min ago");
    }

    #[test]
    fn downloads_show_their_state_in_words() {
        assert_eq!(state_text(&json!({ "state": "downloading", "progress": 0.42 })).1, "Downloading 42%");
        assert_eq!(state_text(&json!({ "state": "seeding" })), ("ok", "Done, seeding".into()));
        assert_eq!(state_text(&json!({ "state": "failed", "next_try": 5.0 })).0, "warn");
        assert_eq!(state_text(&json!({ "state": "failed", "next_try": 0.0 })), ("err", "Failed".into()));
    }
}
