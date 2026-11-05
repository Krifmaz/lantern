//! `#/vault`: the account vault. Admins list, add, change and remove tracker and Usenet accounts; the owner sees
//! a saved password only after typing their own password again, and makes or restores encrypted backups.
//! Secret values are never in the list: the server sends them only to Show.

use kit_leptos::api;
use kit_leptos::clipboard::copy_text;
use kit_leptos::icons::icon;
use kit_leptos::modal::{self, ModalOpts};
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::{json, Map, Value};
use wasm_bindgen::JsCast;

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The fields (`GET /api/vault`'s `fields`) that an item of `kind` offers.
pub fn fields_for(kind: &str, all: &[Value]) -> Vec<Value> {
    all.iter()
        .filter(|f| f.get("kinds").and_then(Value::as_array).is_some_and(|k| k.iter().any(|x| x.as_str() == Some(kind))))
        .cloned()
        .collect()
}

/// The words of field `key` (`password` → `Password`), the key itself when the schema doesn't know it.
pub fn label_of(key: &str, all: &[Value]) -> String {
    all.iter().find(|f| s(f, "key") == key).map(|f| s(f, "label")).unwrap_or_else(|| key.to_string())
}

/// Why a backup passphrase typed twice can't be used, `None` when it can.
pub fn passphrase_problem(first: &str, again: &str, min: usize) -> Option<String> {
    if first.chars().count() < min {
        return Some(format!("The backup passphrase must be at least {min} characters."));
    }
    (first != again).then(|| "The two passphrases aren't the same. Type it again in both boxes.".to_string())
}

/// The name a backup is saved as: `lantern-vault-2026-11-05.json`.
pub fn backup_file_name(project: &str, created: &str) -> String {
    let day: String = created.chars().take(10).collect();
    if day.len() == 10 {
        format!("{project}-vault-{day}.json")
    } else {
        format!("{project}-vault.json")
    }
}

/// The value of the input matching `sel` in modal `m`, "" when there is none.
fn input_value(m: &modal::Modal, sel: &str) -> String {
    m.qs(sel)
        .and_then(|e| {
            if let Some(i) = e.dyn_ref::<web_sys::HtmlInputElement>() {
                Some(i.value())
            } else {
                e.dyn_ref::<web_sys::HtmlSelectElement>().map(|i| i.value())
            }
        })
        .unwrap_or_default()
}

/// Saves `text` as the file `name` through the browser's download.
fn download(name: &str, text: &str) {
    let Some(doc) = kit_leptos::rt::document() else { return };
    let Ok(a) = doc.create_element("a") else { return };
    let href = format!("data:application/json;charset=utf-8,{}", js_sys::encode_uri_component(text));
    let _ = a.set_attribute("href", &href);
    let _ = a.set_attribute("download", name);
    if let (Some(body), Some(el)) = (doc.body(), a.dyn_ref::<web_sys::HtmlElement>()) {
        let _ = body.append_child(&a);
        el.click();
        let _ = body.remove_child(&a);
    }
}

/// `#/vault`. `owner`: whether the signed-in account may see secret values and make backups.
#[component]
pub fn Vault(owner: bool) -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let status = LocalResource::new(|| async { api::get("/vault").await.map_err(|e| e.message) });
    let list = LocalResource::new(move || {
        tick.track();
        async { api::get("/vault/items").await.map_err(|e| e.message) }
    });
    let schema = move || match status.get() {
        Some(Ok(v)) => v,
        _ => Value::Null,
    };
    let locked = move || match status.get() {
        Some(Ok(v)) if v.get("open").and_then(Value::as_bool) == Some(false) => Some(s(&v, "locked_because")),
        Some(Err(e)) => Some(e),
        _ => None,
    };
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let items = v.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
            if items.is_empty() {
                return view! {
                    <div class="empty" inner_html=format!(r#"{}<p class="muted">No accounts yet. Add your tracker and Usenet logins: Lantern keeps them encrypted and never shows them without your password.</p>"#, icon("settings"))></div>
                }
                .into_any();
            }
            let sc = schema();
            view! {
                <div class="table-wrap"><table class="table" id="vault-table">
                    <thead><tr><th>"Name"</th><th>"Kind"</th><th>"Username"</th><th>"Saved secrets"</th><th>""</th></tr></thead>
                    <tbody>{items.into_iter().map(|it| item_row(it, sc.clone(), owner, reload)).collect_view()}</tbody>
                </table></div>
            }
            .into_any()
        }
    };
    let add = move |_| {
        let sc = schema();
        if !sc.is_null() {
            open_edit(None, sc, reload);
        }
    };
    let backup = move |_| open_backup(schema());
    let restore = move |_| open_restore(reload);
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Vault"</h1>
            <div class="row" style="gap:8px">
                {owner.then(|| view! { <button class="btn" id="vault-restore" on:click=restore inner_html=format!("{}Restore", icon("refresh"))></button> })}
                {owner.then(|| view! { <button class="btn" id="vault-backup" on:click=backup inner_html=format!("{}Back up", icon("download"))></button> })}
                <button class="btn primary" id="vault-add" on:click=add inner_html=format!("{}Add account", icon("plus"))></button>
            </div>
        </div>
        {move || locked().map(|why| view! { <div class="error-box" style="margin-bottom:12px">{why}</div> })}
        <div class="card flat">{rows}</div>
        <p class="small muted mt">"Everything here is encrypted on disk. Keep vault.key from the data folder with your backups (or set APP_VAULT_PASSPHRASE), and use Back up for a copy you can restore anywhere."</p>
    }
}

/// One account in the list.
fn item_row(it: Value, schema: Value, owner: bool, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = s(&it, "id");
    let name = s(&it, "name");
    let fields_schema = schema.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
    let secrets: Vec<String> = it
        .get("secrets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|k| label_of(k, &fields_schema)).collect())
        .unwrap_or_default();
    let username = it.get("fields").map(|f| s(f, "username")).unwrap_or_default();
    let site = s(&it, "site");
    let (id_r, name_r) = (id.clone(), name.clone());
    let reveal = move |_| open_reveal(id_r.clone(), name_r.clone(), fields_schema.clone());
    let (id_d, name_d) = (id.clone(), name.clone());
    let remove = move |_| {
        let (id, name) = (id_d.clone(), name_d.clone());
        kit_leptos::rt::spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Remove {name}?"))
                .text("Its saved login, cookie and keys are deleted from the vault. A backup you made earlier still has them.")
                .ok("Remove")
                .danger(true);
            if modal::confirm(q).await {
                match api::del(&format!("/vault/items/{id}")).await {
                    Ok(_) => toast(&format!("{name} removed"), "success"),
                    Err(e) => toast(&e.message, "error"),
                }
                reload();
            }
        })
    };
    let edit_it = it.clone();
    let edit = move |_| open_edit(Some(edit_it.clone()), schema.clone(), reload);
    view! {
        <tr data-id=id>
            <td><div style="font-weight:650">{name}</div>{(!site.is_empty()).then(|| view! { <div class="small faint mono">{site.clone()}</div> })}</td>
            <td><span class="badge">{s(&it, "kind_label")}</span></td>
            <td class="mono small">{username}</td>
            <td class="small">{if secrets.is_empty() { "None".to_string() } else { secrets.join(", ") }}</td>
            <td style="white-space:nowrap;text-align:right">
                <span class="row" style="gap:8px;justify-content:flex-end">
                    {owner.then(|| view! { <button class="btn sm" on:click=reveal>"Show"</button> })}
                    <button class="btn sm" on:click=edit>"Edit"</button>
                    <button class="btn sm ghost" title="Remove" on:click=remove inner_html=icon("trash")></button>
                </span>
            </td>
        </tr>
    }
}

/// The Add and Edit dialog: kind, name, site and the kind's fields. A saved secret's box stays empty: leave it
/// empty to keep it, type to replace it.
fn open_edit(item: Option<Value>, schema: Value, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = item.as_ref().map(|i| s(i, "id"));
    let item = item.unwrap_or_else(|| json!({ "kind": "tracker", "fields": {}, "secrets": [] }));
    let all = schema.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
    let kinds = schema.get("kinds").and_then(Value::as_array).cloned().unwrap_or_default();
    let (kind, set_kind) = signal(s(&item, "kind"));
    let (problem, set_problem) = signal(None::<String>);
    let (busy, set_busy) = signal(false);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let title = match &id {
        Some(_) => format!("Edit {}", s(&item, "name")),
        None => "Add account".to_string(),
    };
    let saved: Vec<String> = item
        .get("secrets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let plain = item.get("fields").cloned().unwrap_or_else(|| json!({}));
    let all_b = all.clone();
    let item_b = item.clone();
    let editing = id.is_some();
    let body = move || {
        let (all, plain, saved, item) = (all_b.clone(), plain.clone(), saved.clone(), item_b.clone());
        let fields = move || {
            let (plain, saved) = (plain.clone(), saved.clone());
            fields_for(&kind.get(), &all)
                .into_iter()
                .map(|f| {
                    let key = s(&f, "key");
                    let secret = f.get("secret").and_then(Value::as_bool) == Some(true);
                    let help = s(&f, "help");
                    let is_saved = saved.contains(&key);
                    let placeholder = if secret && is_saved { "Saved. Leave empty to keep it." } else { "" };
                    let input = view! {
                        <input class="input" data-vf=key.clone() type=if secret { "password" } else { "text" } autocomplete="off"
                            placeholder=placeholder value=if secret { String::new() } else { s(&plain, &key) }/>
                    };
                    view! {
                        <label class="field"><span>{s(&f, "label")}</span>{input}
                            {(!help.is_empty()).then(|| view! { <small>{help.clone()}</small> })}</label>
                    }
                })
                .collect_view()
        };
        let kinds = kinds.clone();
        view! {
            <div class="form-grid">
                <label class="field"><span>"Kind"</span>
                    <select class="input" data-vk="kind" disabled=editing on:change=move |e| set_kind.set(event_target_value(&e))>
                        {kinds.iter().map(|k| { let v = s(k, "id"); view! { <option value=v.clone() selected=v == kind.get_untracked()>{s(k, "label")}</option> } }).collect_view()}
                    </select></label>
                <label class="field"><span>"Name"</span><input class="input" data-vk="name" value=s(&item, "name") placeholder="The site's name"/></label>
                <label class="field"><span>"Site"</span><input class="input" data-vk="site" value=s(&item, "site") placeholder="Address or indexer"/>
                    <small>"The site's address, or the indexer it belongs to."</small></label>
            </div>
            <div class="form-grid mt">{fields}</div>
            {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
        }
    };
    let save = move |_| {
        let Some(m) = holder.get_value() else { return };
        let id = id.clone();
        let mut fields = Map::new();
        for f in fields_for(&kind.get_untracked(), &all) {
            let key = s(&f, "key");
            let v = input_value(&m, &format!("[data-vf=\"{key}\"]"));
            fields.insert(key, json!(v));
        }
        let mut body =
            json!({ "name": input_value(&m, "[data-vk=\"name\"]"), "site": input_value(&m, "[data-vk=\"site\"]"), "fields": fields });
        set_busy.set(true);
        set_problem.set(None);
        kit_leptos::rt::spawn(async move {
            let r = match id {
                Some(id) => api::put(&format!("/vault/items/{id}"), body).await,
                None => {
                    body["kind"] = json!(kind.get_untracked());
                    api::post("/vault/items", body).await
                }
            };
            set_busy.set(false);
            match r {
                Ok(v) => {
                    toast(&format!("{} saved", s(&v, "name")), "success");
                    reload();
                    m.close();
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
        });
    };
    let footer = move || view! { <button class="btn primary" id="vault-save" disabled=busy on:click=save.clone()>{move || if busy.get() { "Saving…" } else { "Save" }}</button> };
    let m = modal::open(ModalOpts::with_view(&title, body).footer_view(footer).wide(true));
    holder.set_value(Some(m));
}

/// The Show dialog: the owner types their password, then sees every value with Copy, and the 2FA code.
fn open_reveal(id: String, name: String, all: Vec<Value>) {
    let (shown, set_shown) = signal(None::<Value>);
    let (problem, set_problem) = signal(None::<String>);
    let (busy, set_busy) = signal(false);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let ask = move || {
        let Some(m) = holder.get_value() else { return };
        let password = input_value(&m, "#vault-password");
        let id = id.clone();
        set_busy.set(true);
        kit_leptos::rt::spawn(async move {
            match api::post(&format!("/vault/items/{id}/reveal"), json!({ "password": password })).await {
                Ok(v) => {
                    set_problem.set(None);
                    set_shown.set(Some(v));
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    let ask2 = ask.clone();
    let body = move || {
        let all = all.clone();
        let ask = ask2.clone();
        move || match shown.get() {
            None => {
                let ask = ask.clone();
                let submit = move |e: leptos::ev::SubmitEvent| {
                    e.prevent_default();
                    ask();
                };
                view! {
                    <form on:submit=submit>
                        <p class="muted" style="margin-top:0">"Type your password to see what is saved for this account."</p>
                        <label class="field"><span>"Your password"</span><input class="input" id="vault-password" type="password" autocomplete="current-password" autofocus/></label>
                        {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
                        <div class="row mt" style="justify-content:flex-end"><button class="btn primary" type="submit" disabled=busy>"Show"</button></div>
                    </form>
                }
                .into_any()
            }
            Some(v) => {
                let fields = v.get("fields").and_then(Value::as_object).cloned().unwrap_or_default();
                let code = v.get("totp").filter(|t| !t.is_null()).map(|t| (s(t, "code"), s(t, "seconds_left")));
                view! {
                    {code.map(|(c, left)| { let c2 = c.clone(); view! {
                        <div class="row between card flat" style="padding:10px 12px;margin-bottom:12px">
                            <div><div class="small muted">{format!("2FA code, {left} s left")}</div><div class="mono" style="font-weight:650">{c}</div></div>
                            <button class="btn sm" on:click=move |_| copy_text(&c2) inner_html=format!("{}Copy", icon("copy"))></button>
                        </div>
                    } })}
                    <div class="list">{fields.into_iter().map(|(k, val)| {
                        let text = val.as_str().unwrap_or_default().to_string();
                        let copy = text.clone();
                        view! {
                            <div class="list-item" style="gap:10px">
                                <div class="grow" style="min-width:0"><div class="small muted">{label_of(&k, &all)}</div>
                                    <div class="mono small" style="word-break:break-all;white-space:pre-wrap">{text}</div></div>
                                <button class="btn sm" on:click=move |_| copy_text(&copy) inner_html=format!("{}Copy", icon("copy"))></button>
                            </div>
                        }
                    }).collect_view()}</div>
                    <p class="small muted mt">"Close this window when you're done: the values aren't kept in the page."</p>
                }
                .into_any()
            }
        }
    };
    let m = modal::open(ModalOpts::with_view(&name, body).wide(true));
    holder.set_value(Some(m));
}

/// The Back up dialog: the owner's password and a backup passphrase typed twice; the file downloads.
fn open_backup(schema: Value) {
    let min = schema.get("min_passphrase").and_then(Value::as_u64).unwrap_or(12) as usize;
    let (problem, set_problem) = signal(None::<String>);
    let (busy, set_busy) = signal(false);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let body = move || {
        view! {
            <p class="muted" style="margin-top:0">"The backup holds every account, encrypted with a passphrase you choose. Lantern can't open it without that passphrase, so keep it somewhere safe."</p>
            <div class="form-grid">
                <label class="field"><span>"Your password"</span><input class="input" id="vault-bk-password" type="password" autocomplete="current-password"/></label>
                <label class="field"><span>"Backup passphrase"</span><input class="input" id="vault-bk-pass" type="password" autocomplete="new-password"/>
                    <small>{format!("At least {min} characters.")}</small></label>
                <label class="field"><span>"Passphrase again"</span><input class="input" id="vault-bk-pass2" type="password" autocomplete="new-password"/></label>
            </div>
            {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
        }
    };
    let go = move |_| {
        let Some(m) = holder.get_value() else { return };
        let (pass, again) = (input_value(&m, "#vault-bk-pass"), input_value(&m, "#vault-bk-pass2"));
        if let Some(p) = passphrase_problem(&pass, &again, min) {
            set_problem.set(Some(p));
            return;
        }
        let password = input_value(&m, "#vault-bk-password");
        set_busy.set(true);
        set_problem.set(None);
        kit_leptos::rt::spawn(async move {
            match api::post("/vault/backup", json!({ "password": password, "passphrase": pass })).await {
                Ok(file) => {
                    let text = serde_json::to_string_pretty(&file).unwrap_or_default();
                    download(&backup_file_name(crate::project(), &s(&file, "created")), &text);
                    toast("Backup saved", "success");
                    m.close();
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };
    let footer = move || view! { <button class="btn primary" disabled=busy on:click=go>{move || if busy.get() { "Encrypting…" } else { "Back up" }}</button> };
    let m = modal::open(ModalOpts::with_view("Back up the vault", body).footer_view(footer));
    holder.set_value(Some(m));
}

/// The Restore dialog: a backup file, the owner's password and the backup's passphrase. Check shows what would
/// change; Restore does it. Nothing here is ever removed.
fn open_restore(reload: impl Fn() + Copy + Send + Sync + 'static) {
    let (answer, set_answer) = signal(None::<(bool, String)>);
    let (busy, set_busy) = signal(false);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let body = move || {
        view! {
            <p class="muted" style="margin-top:0">"Accounts in the backup are added. One that is here too gets the backup's copy, unless you changed it here after the backup. Nothing is removed."</p>
            <form id="vault-restore-form" class="form-grid">
                <label class="field"><span>"Backup file"</span><input class="input" name="file" type="file" accept=".json,application/json"/></label>
                <label class="field"><span>"Your password"</span><input class="input" name="password" type="password" autocomplete="current-password"/></label>
                <label class="field"><span>"Backup passphrase"</span><input class="input" name="passphrase" type="password" autocomplete="off"/></label>
                <input type="hidden" name="dry_run" id="vault-dry-run" value="true"/>
            </form>
            {move || answer.get().map(|(ok, msg)| view! { <div class=if ok { "test-result ok mt" } else { "error-box mt" }>{msg}</div> })}
        }
    };
    let run = move |dry: bool| {
        let Some(m) = holder.get_value() else { return };
        let Some(form) = m.qs("#vault-restore-form").and_then(|f| f.dyn_into::<web_sys::HtmlFormElement>().ok()) else { return };
        if let Some(d) = m.qs("#vault-dry-run").and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            d.set_value(if dry { "true" } else { "false" });
        }
        let Ok(data) = web_sys::FormData::new_with_form(&form) else { return };
        set_busy.set(true);
        kit_leptos::rt::spawn(async move {
            match api::post_form("/vault/restore", data).await {
                Ok(v) => {
                    set_answer.set(Some((true, s(&v, "message"))));
                    if !dry {
                        toast(&s(&v, "message"), "success");
                        reload();
                    }
                }
                Err(e) => set_answer.set(Some((false, e.message))),
            }
            set_busy.set(false);
        });
    };
    let footer = move || {
        view! {
            <button class="btn" disabled=busy on:click=move |_| run(true)>"Check first"</button>
            <button class="btn primary" disabled=busy on:click=move |_| run(false)>{move || if busy.get() { "Working…" } else { "Restore" }}</button>
        }
    };
    let m = modal::open(ModalOpts::with_view("Restore the vault", body).footer_view(footer));
    holder.set_value(Some(m));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_shows_its_own_fields() {
        let all = vec![
            json!({ "key": "username", "label": "Username", "kinds": ["tracker", "usenet_provider"] }),
            json!({ "key": "host", "label": "Server", "kinds": ["usenet_provider"] }),
            json!({ "key": "passkey", "label": "Passkey", "kinds": ["tracker"] }),
        ];
        let keys = |k: &str| fields_for(k, &all).iter().map(|f| s(f, "key")).collect::<Vec<_>>();
        assert_eq!(keys("tracker"), ["username", "passkey"]);
        assert_eq!(keys("usenet_provider"), ["username", "host"]);
        assert_eq!(label_of("passkey", &all), "Passkey");
        assert_eq!(label_of("unknown", &all), "unknown");
    }

    #[test]
    fn backup_passphrases_are_checked_before_sending() {
        assert_eq!(passphrase_problem("short", "short", 12).as_deref(), Some("The backup passphrase must be at least 12 characters."));
        assert!(passphrase_problem("a long passphrase", "a long passphrasf", 12).unwrap().starts_with("The two passphrases"));
        assert_eq!(passphrase_problem("a long passphrase", "a long passphrase", 12), None);
        assert_eq!(backup_file_name("lantern", "2026-11-05T01:00:00Z"), "lantern-vault-2026-11-05.json");
        assert_eq!(backup_file_name("lantern", ""), "lantern-vault.json");
    }
}
