//! `#/import`: moving from Prowlarr (a backup, or a running Prowlarr's address and API key) and FlareSolverr (its
//! settings pasted in). Every import is looked at first (the server's dry run); the plan shows what would happen
//! line by line, and Import does it after a confirm.

use kit_leptos::api;
use kit_leptos::modal;
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::{json, Value};

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The words and badge style of a plan line's action (`add` → "Add", `ok`).
pub fn action_badge(action: &str) -> (&'static str, &'static str) {
    match action {
        "add" => ("Add", "ok"),
        "set" => ("Set", "ok"),
        "already_there" => ("Already in Lantern", ""),
        "skip" => ("Skipped", "warn"),
        "manual" => ("For you to do", "info"),
        _ => ("Nothing needed", ""),
    }
}

/// The words of a plan line under its detail: the secrets brought over and what to fill in again.
pub fn extra_words(step: &Value) -> String {
    let list =
        |k: &str| step.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>()).unwrap_or_default();
    let mut out = Vec::new();
    let secrets = list("secrets");
    if !secrets.is_empty() {
        out.push(format!("Brings over: {}.", secrets.join(", ")));
    }
    let missing = list("missing");
    if !missing.is_empty() {
        out.push(format!("Fill in again: {}.", missing.join(", ")));
    }
    out.join(" ")
}

/// What the last look was made from, so Import sends the same again.
#[derive(Clone)]
enum Last {
    /// An uploaded backup or database.
    File(web_sys::File),
    /// A running Prowlarr: its address and API key.
    Api(String, String),
}

/// Sends `last` to the server as a look (`dry_run`) or the import, keeping or not keeping Prowlarr's API key.
async fn send(last: &Last, dry_run: bool, take_key: bool) -> Result<Value, String> {
    match last {
        Last::File(f) => {
            let form = web_sys::FormData::new().map_err(|_| "This browser can't send files.".to_string())?;
            let _ = form.append_with_str("dry_run", if dry_run { "true" } else { "false" });
            let _ = form.append_with_str("take_api_key", if take_key { "true" } else { "false" });
            let _ = form.append_with_blob_and_filename("file", f, &f.name());
            api::post_form("/import/prowlarr", form).await.map_err(|e| e.message)
        }
        Last::Api(url, key) => {
            api::post("/import/prowlarr/api", json!({ "url": url, "key": key, "dry_run": dry_run, "take_api_key": take_key }))
                .await
                .map_err(|e| e.message)
        }
    }
}

/// `#/import`.
#[component]
pub fn Import(admin: bool) -> impl IntoView {
    if !admin {
        return view! { <h1>"Import"</h1><div class="card flat"><p class="muted">"Only an admin can import."</p></div> }.into_any();
    }
    view! {
        <h1>"Import"</h1>
        <p class="muted">"Bring your indexers, apps and settings over. Lantern looks first and shows what it would do; nothing changes until you press Import."</p>
        <ProwlarrCard/>
        <FlareSolverrCard/>
    }
    .into_any()
}

/// The Prowlarr card: a backup or the address, then the plan and Import.
#[component]
fn ProwlarrCard() -> impl IntoView {
    let file = NodeRef::<leptos::html::Input>::new();
    let url = NodeRef::<leptos::html::Input>::new();
    let key = NodeRef::<leptos::html::Input>::new();
    let take = NodeRef::<leptos::html::Input>::new();
    let (busy, set_busy) = signal(false);
    let (plan, set_plan) = signal(None::<Value>);
    let last: StoredValue<Option<Last>, LocalStorage> = StoredValue::new_local(None);
    let take_key = move || take.get().map(|i| i.checked()).unwrap_or(true);
    let look = move |what: Last| {
        last.set_value(Some(what.clone()));
        set_busy.set(true);
        set_plan.set(None);
        kit_leptos::rt::spawn(async move {
            match send(&what, true, take_key()).await {
                Ok(p) => set_plan.set(Some(p)),
                Err(e) => toast(&e, "error"),
            }
            set_busy.set(false);
        });
    };
    let look_file = move |_| {
        let f = file.get().and_then(|i| i.files()).and_then(|l| l.get(0));
        match f {
            Some(f) => look(Last::File(f)),
            None => toast("Choose a Prowlarr backup (.zip) or its database (prowlarr.db) first.", "error"),
        }
    };
    let look_api = move |_| {
        let (u, k) = (url.get().map(|i| i.value()).unwrap_or_default(), key.get().map(|i| i.value()).unwrap_or_default());
        look(Last::Api(u.trim().to_string(), k.trim().to_string()));
    };
    let import = move |_| {
        let Some(what) = last.get_value() else { return };
        kit_leptos::rt::spawn(async move {
            let q = kit_rules::dialogs::Confirm::new("Import from Prowlarr?")
                .text("Lantern adds what the plan says. Nothing already in Lantern is changed or removed, except the API key and proxy if the plan sets them.")
                .ok("Import");
            if !modal::confirm(q).await {
                return;
            }
            set_busy.set(true);
            match send(&what, false, take_key()).await {
                Ok(p) => {
                    toast(&s(&p, "summary"), "success");
                    set_plan.set(Some(p));
                }
                Err(e) => toast(&e, "error"),
            }
            set_busy.set(false);
        });
    };
    view! {
        <div class="card flat mt" id="import-prowlarr">
            <h2 style="margin-top:0">"From Prowlarr"</h2>
            <p class="small muted">"A backup brings everything, logins included: in Prowlarr open System → Backup, press Backup Now and download the .zip. Prowlarr's address and API key work too, but Prowlarr hides passwords there, so you fill those in again afterwards."</p>
            <div class="row wrap" style="gap:10px;align-items:flex-end">
                <label class="field grow"><span>"Backup (.zip) or prowlarr.db"</span><input class="input" type="file" accept=".zip,.db" node_ref=file/></label>
                <button class="btn primary" disabled=busy on:click=look_file>"Look at it"</button>
            </div>
            <div class="row wrap mt" style="gap:10px;align-items:flex-end">
                <label class="field grow"><span>"Prowlarr's address"</span><input class="input" placeholder="http://prowlarr:9696" node_ref=url/></label>
                <label class="field grow"><span>"Prowlarr's API key"</span><input class="input" type="password" autocomplete="off" node_ref=key/></label>
                <button class="btn" disabled=busy on:click=look_api>"Look at it"</button>
            </div>
            <label class="switch mt"><input type="checkbox" checked=true node_ref=take/><i></i><span>"Use Prowlarr's API key in Lantern, so apps keep working without changes"</span></label>
            {move || busy.get().then(|| view! { <div class="empty"><span class="spinner"></span></div> })}
            {move || plan.get().map(|p| {
                let dry = p.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
                view! {
                    <PlanView plan=p/>
                    {dry.then(|| view! { <div class="row mt"><button class="btn primary" id="import-go" disabled=busy on:click=import>"Import"</button></div> })}
                }
            })}
        </div>
    }
}

/// The FlareSolverr card: its settings pasted in, then the steps.
#[component]
fn FlareSolverrCard() -> impl IntoView {
    let text = NodeRef::<leptos::html::Textarea>::new();
    let (plan, set_plan) = signal(None::<Value>);
    let look = move |_| {
        let t = text.get().map(|i| i.value()).unwrap_or_default();
        kit_leptos::rt::spawn(async move {
            match api::post("/import/flaresolverr", json!({ "text": t })).await {
                Ok(p) => set_plan.set(Some(p)),
                Err(e) => toast(&e.message, "error"),
            }
        });
    };
    view! {
        <div class="card flat mt" id="import-flaresolverr">
            <h2 style="margin-top:0">"From FlareSolverr"</h2>
            <p class="small muted">"Lantern gets past Cloudflare checks by itself and answers apps that call FlareSolverr. Paste FlareSolverr's docker-compose service, .env file or docker run command to see what to change."</p>
            <label class="field"><span>"FlareSolverr's settings"</span><textarea class="input mono" rows="8" node_ref=text></textarea></label>
            <div class="row mt"><button class="btn primary" on:click=look>"Look at it"</button></div>
            {move || plan.get().map(|p| view! { <PlanView plan=p/> })}
        </div>
    }
}

/// A plan: the summary, one table per part, the notes.
#[component]
fn PlanView(plan: Value) -> impl IntoView {
    let part = |title: &'static str, key: &'static str| {
        let steps = plan.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        (!steps.is_empty()).then(|| {
            view! {
                <h3>{title}</h3>
                <div class="table-wrap"><table class="table">
                    <thead><tr><th>"Name"</th><th>"What"</th><th>""</th><th>"Details"</th></tr></thead>
                    <tbody>{steps.into_iter().map(|st| {
                        let (words, style) = action_badge(&s(&st, "action"));
                        let extra = extra_words(&st);
                        view! {
                            <tr>
                                <td style="font-weight:650">{s(&st, "name")}</td>
                                <td class="small">{s(&st, "what")}</td>
                                <td><span class=format!("badge {style}")>{words}</span></td>
                                <td class="small">{s(&st, "detail")}{(!extra.is_empty()).then(|| view! { <div class="faint">{extra}</div> })}</td>
                            </tr>
                        }
                    }).collect_view()}</tbody>
                </table></div>
            }
        })
    };
    let notes = plan.get("notes").and_then(Value::as_array).cloned().unwrap_or_default();
    view! {
        <div class="mt" data-plan="">
            <p style="font-weight:650">{s(&plan, "summary")}</p>
            {part("Indexers", "indexers")}
            {part("Apps", "apps")}
            {part("Settings and steps", "settings")}
            {(!notes.is_empty()).then(|| view! {
                <ul class="small muted">{notes.into_iter().map(|n| view! { <li>{n.as_str().unwrap_or("").to_string()}</li> }).collect_view()}</ul>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_lines_read_as_words() {
        assert_eq!(action_badge("add"), ("Add", "ok"));
        assert_eq!(action_badge("manual"), ("For you to do", "info"));
        assert_eq!(action_badge("nothing_needed").0, "Nothing needed");
        let st = json!({ "secrets": ["password"], "missing": ["API key", "Cookie"] });
        assert_eq!(extra_words(&st), "Brings over: password. Fill in again: API key, Cookie.");
        assert_eq!(extra_words(&json!({})), "");
    }
}
