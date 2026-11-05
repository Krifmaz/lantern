//! `#/accounts`: the account keeper. One card per vault account: what is at risk, its numbers (ratio, upload,
//! download, seed time, bonus points, hit-and-runs), when Lantern last signed in, and buttons to sign in now,
//! update the numbers and set the site's rules. Test alerts sends a message through every alert channel.

use kit_leptos::api;
use kit_leptos::icons::icon;
use kit_leptos::modal::{self, ModalOpts};
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use serde_json::{json, Map, Value};
use wasm_bindgen::JsCast;

/// A gibibyte, in bytes: the numbers are typed in GB.
pub const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// A number without a trailing `.0`, "" for none.
pub fn num(v: Option<f64>) -> String {
    match v {
        None => String::new(),
        Some(x) => {
            let t = format!("{x:.2}");
            t.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    }
}

/// How long ago `at` (seconds since 1970) was at `now`, in words: "today", "yesterday", "5 days ago", "never".
pub fn ago(at: Option<f64>, now: f64) -> String {
    match at {
        None => "never".into(),
        Some(at) => match ((now - at) / 86400.0).floor() as i64 {
            d if d <= 0 => "today".into(),
            1 => "yesterday".into(),
            d => format!("{d} days ago"),
        },
    }
}

/// A date input's value (`2026-12-31`) for seconds since 1970, "" for none.
pub fn date_value(at: Option<f64>) -> String {
    let Some(at) = at else { return String::new() };
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at * 1000.0));
    let iso: String = d.to_iso_string().into();
    iso.chars().take(10).collect()
}

/// The dot class of an account: `err` with a red risk, `warn` with any risk, `ok` without.
pub fn dot_of(risks: &[Value]) -> &'static str {
    if risks.iter().any(|r| s(r, "level") == "danger") {
        "err"
    } else if risks.is_empty() {
        "ok"
    } else {
        "warn"
    }
}

/// The value of the input matching `sel` in modal `m`.
fn input(m: &modal::Modal, sel: &str) -> Option<web_sys::HtmlInputElement> {
    m.qs(sel).and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
}

/// `#/accounts`.
#[component]
pub fn Accounts() -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    let reload = move || set_tick.update(|t| *t += 1);
    let data = LocalResource::new(move || {
        tick.track();
        async { api::get("/keeper").await.map_err(|e| e.message) }
    });
    let test = move |_| {
        kit_leptos::rt::spawn(async move {
            match api::post_empty("/keeper/test-alerts").await {
                Ok(v) => {
                    for r in v.get("results").and_then(Value::as_array).cloned().unwrap_or_default() {
                        toast(&s(&r, "message"), if r.get("ok").and_then(Value::as_bool) == Some(true) { "success" } else { "error" });
                    }
                }
                Err(e) => toast(&e.message, "error"),
            }
        })
    };
    let body = move || match data.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let list = v.get("accounts").and_then(Value::as_array).cloned().unwrap_or_default();
            let channels = v.get("channels").cloned().unwrap_or(Value::Null);
            let on: Vec<&str> = [("ntfy", "ntfy"), ("discord", "Discord"), ("email", "email")]
                .iter()
                .filter(|(k, _)| channels.get(*k) == Some(&json!(true)))
                .map(|(_, l)| *l)
                .collect();
            let alerts = if on.is_empty() {
                "Warnings show as a banner. Add ntfy, Discord or email under Settings → Accounts → Alerts to get them on your phone."
                    .to_string()
            } else {
                format!("Warnings show as a banner and go to {}.", on.join(", "))
            };
            if list.is_empty() {
                return view! {
                    <div class="card flat"><div class="empty" inner_html=format!(r#"{}<p class="muted">No accounts yet. Add your tracker logins on the Vault page, then set each site's rules here.</p>"#, icon("alert"))></div></div>
                }
                .into_any();
            }
            let now = kit_leptos::rt::now_ms() as f64 / 1000.0;
            view! {
                <p class="small muted" style="margin-top:0">{alerts}</p>
                <div class="list">{list.into_iter().map(|a| account_card(a, now, reload)).collect_view()}</div>
            }
            .into_any()
        }
    };
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Accounts"</h1>
            <button class="btn" id="keeper-test" on:click=test inner_html=format!("{}Test alerts", icon("bug"))></button>
        </div>
        {body}
    }
}

/// One account's card.
fn account_card(a: Value, now: f64, reload: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let id = s(&a, "vault_id");
    let name = s(&a, "name");
    let risks = a.get("risks").and_then(Value::as_array).cloned().unwrap_or_default();
    let st = a.get("stats").cloned().unwrap_or(Value::Null);
    let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let mut numbers = Vec::new();
    if let Some(r) = a.get("ratio").and_then(Value::as_f64) {
        numbers.push(format!("Ratio {}", num(Some(r))));
    }
    if let Some(u) = f(&st, "uploaded") {
        numbers.push(format!("Up {} GB", num(Some(u / GIB))));
    }
    if let Some(d) = f(&st, "downloaded") {
        numbers.push(format!("Down {} GB", num(Some(d / GIB))));
    }
    if let Some(h) = f(&st, "seed_hours") {
        numbers.push(format!("Seeded {} h", num(Some(h))));
    }
    if let Some(b) = f(&st, "bonus") {
        numbers.push(format!("{} bonus points", num(Some(b))));
    }
    if let Some(n) = st.get("hnr_count").and_then(Value::as_i64) {
        numbers.push(format!("{n} hit-and-runs"));
    }
    let linked = a.get("indexer_id").is_some_and(|v| !v.is_null());
    let signed = format!("Signed in {}", ago(f(&a, "signed_in_at"), now));
    let keep_err = s(&a, "keep_alive_error");
    let (busy, set_busy) = signal(false);
    let id1 = id.clone();
    let sign_in = move |_| {
        let id = id1.clone();
        set_busy.set(true);
        kit_leptos::rt::spawn(async move {
            match api::post_empty(&format!("/keeper/{id}/sign-in")).await {
                Ok(v) => toast(&format!("Signed in to {}", s(&v, "name")), "success"),
                Err(e) => toast(&e.message, "error"),
            }
            set_busy.set(false);
            reload();
        })
    };
    let id2 = id.clone();
    let mark = move |_| {
        let id = id2.clone();
        kit_leptos::rt::spawn(async move {
            match api::put(&format!("/keeper/{id}/stats"), json!({ "signed_in": true })).await {
                Ok(_) => toast("Noted: signed in today", "success"),
                Err(e) => toast(&e.message, "error"),
            }
            reload();
        })
    };
    let (a1, a2) = (a.clone(), a.clone());
    let numbers_btn = move |_| open_numbers(a1.clone(), reload);
    let rules_btn = move |_| open_rules(a2.clone(), reload);
    view! {
        <div class="card" data-account=id>
            <div class="row between wrap" style="gap:10px">
                <div class="row" style="gap:10px;min-width:0">
                    <span class=format!("dot {}", dot_of(&risks))></span>
                    <div style="min-width:0"><div style="font-weight:650">{name}</div>
                        <div class="small faint">{signed}{if linked { " · kept alive through its indexer" } else { "" }}</div></div>
                </div>
                <div class="row wrap" style="gap:8px">
                    {linked.then(|| view! { <button class="btn sm" disabled=busy on:click=sign_in.clone()>{move || if busy.get() { "Signing in…" } else { "Sign in now" }}</button> })}
                    {(!linked).then(|| view! { <button class="btn sm" on:click=mark.clone()>"Signed in today"</button> })}
                    <button class="btn sm" on:click=numbers_btn>"Numbers"</button>
                    <button class="btn sm" on:click=rules_btn>"Rules"</button>
                </div>
            </div>
            {(!numbers.is_empty()).then(|| view! { <div class="row wrap mt" style="gap:6px">{numbers.iter().map(|n| view! { <span class="badge">{n.clone()}</span> }).collect_view()}</div> })}
            {(!keep_err.is_empty()).then(|| view! { <div class="error-box mt">{format!("The last automatic sign-in didn't work: {keep_err}")}</div> })}
            {(!risks.is_empty()).then(|| view! {
                <div class="mt">{risks.iter().map(|r| {
                    let cls = if s(r, "level") == "danger" { "error-box" } else { "test-result" };
                    view! { <div class=cls style="margin-top:6px">{s(r, "text")}</div> }
                }).collect_view()}</div>
            })}
        </div>
    }
}

/// The Numbers dialog: what the account's page on the site shows.
fn open_numbers(a: Value, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = s(&a, "vault_id");
    let st = a.get("stats").cloned().unwrap_or(Value::Null);
    let (problem, set_problem) = signal(None::<String>);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let fields: Vec<(&'static str, &'static str, f64)> = vec![
        ("uploaded", "Uploaded (GB)", GIB),
        ("downloaded", "Downloaded (GB)", GIB),
        ("ratio", "Ratio (as the site shows it)", 1.0),
        ("seed_hours", "Seed time (hours)", 1.0),
        ("bonus", "Bonus points", 1.0),
        ("hnr_count", "Hit-and-runs now", 1.0),
    ];
    let f2 = fields.clone();
    let body = move || {
        view! {
            <p class="muted" style="margin-top:0">"Copy these from your profile page on the site. Leave a box empty if the site doesn't show it."</p>
            <div class="form-grid">{f2.iter().map(|(k, label, unit)| {
                let v = st.get(*k).and_then(Value::as_f64).map(|x| x / unit);
                view! { <label class="field"><span>{*label}</span><input class="input" type="number" min="0" step="any" data-kn=*k value=num(v)/></label> }
            }).collect_view()}</div>
            {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
        }
    };
    let save = move |_| {
        let Some(m) = holder.get_value() else { return };
        let mut body = Map::new();
        for (k, _, unit) in &fields {
            let raw = input(&m, &format!("[data-kn=\"{k}\"]")).map(|i| i.value()).unwrap_or_default();
            let v = match raw.trim().parse::<f64>() {
                Ok(x) => json!(x * unit),
                Err(_) => Value::Null,
            };
            body.insert(k.to_string(), v);
        }
        let id = id.clone();
        kit_leptos::rt::spawn(async move {
            match api::put(&format!("/keeper/{id}/stats"), Value::Object(body)).await {
                Ok(_) => {
                    toast("Numbers saved", "success");
                    reload();
                    m.close();
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
        });
    };
    let footer = move || view! { <button class="btn primary" on:click=save.clone()>"Save"</button> };
    let m = modal::open(ModalOpts::with_view(&format!("{}: numbers", s(&a, "name")), body).footer_view(footer));
    holder.set_value(Some(m));
}

/// The Rules dialog: the site's rules and the indexer Lantern signs in through.
fn open_rules(a: Value, reload: impl Fn() + Copy + Send + Sync + 'static) {
    let id = s(&a, "vault_id");
    let rules = a.get("rules").cloned().unwrap_or(Value::Null);
    let indexers = LocalResource::new(|| async { api::get("/indexers").await.map_err(|e| e.message) });
    let (problem, set_problem) = signal(None::<String>);
    let holder: StoredValue<Option<modal::Modal>, LocalStorage> = StoredValue::new_local(None);
    let linked = a.get("indexer_id").and_then(Value::as_i64);
    let keep_alive = a.get("keep_alive").and_then(Value::as_bool).unwrap_or(true);
    let numbers: Vec<(&'static str, &'static str, &'static str)> = vec![
        ("min_ratio", "Minimum ratio", "0 or empty: the site has none."),
        ("max_hnr", "Hit-and-runs allowed", "How many the site allows before it warns or bans."),
        ("hnr_hours", "Seed each torrent (hours)", "The site's seed time to avoid a hit-and-run."),
        ("login_days", "Sign in at least every (days)", "Accounts that don't sign in for this long are closed."),
        ("activity_days", "Download or seed every (days)", "The site's minimum activity."),
    ];
    let dates: Vec<(&'static str, &'static str)> =
        vec![("login_expires", "Login or account ends on"), ("cookie_expires", "Saved cookie ends on")];
    let (n2, d2) = (numbers.clone(), dates.clone());
    let body = move || {
        let rules = rules.clone();
        let (n2, d2) = (n2.clone(), d2.clone());
        let pick = move || {
            let list = match indexers.get() {
                Some(Ok(v)) => v.get("indexers").and_then(Value::as_array).cloned().unwrap_or_default(),
                _ => Vec::new(),
            };
            view! {
                <select class="input" id="keeper-indexer">
                    <option value="" selected=linked.is_none()>"Not linked"</option>
                    {list.into_iter().map(|ix| { let i = ix.get("id").and_then(Value::as_i64).unwrap_or(0); view! { <option value=i.to_string() selected=linked == Some(i)>{s(&ix, "name")}</option> } }).collect_view()}
                </select>
            }
        };
        view! {
            <div class="form-grid">
                <label class="field"><span>"Indexer"</span>{pick}<small>"Lantern signs in through it to keep the account alive, and counts its grabs as activity."</small></label>
                <label class="field"><span>"Keep alive"</span><label class="switch"><input type="checkbox" id="keeper-alive" checked=keep_alive/><i></i></label>
                    <small>"Sign in on its own before the sign-in rule comes near."</small></label>
                {n2.iter().map(|(k, label, help)| {
                    let v = rules.get(*k).and_then(Value::as_f64).filter(|x| *x > 0.0);
                    view! { <label class="field"><span>{*label}</span><input class="input" type="number" min="0" step="any" data-kr=*k value=num(v)/><small>{*help}</small></label> }
                }).collect_view()}
                {d2.iter().map(|(k, label)| {
                    let v = rules.get(*k).and_then(Value::as_f64);
                    view! { <label class="field"><span>{*label}</span><input class="input" type="date" data-kd=*k value=date_value(v)/></label> }
                }).collect_view()}
            </div>
            {move || problem.get().map(|p| view! { <div class="error-box mt">{p}</div> })}
        }
    };
    let save = move |_| {
        let Some(m) = holder.get_value() else { return };
        let mut body = Map::new();
        let ix =
            m.qs("#keeper-indexer").and_then(|e| e.dyn_into::<web_sys::HtmlSelectElement>().ok()).map(|s| s.value()).unwrap_or_default();
        body.insert("indexer_id".into(), ix.parse::<i64>().map(|i| json!(i)).unwrap_or(Value::Null));
        body.insert("keep_alive".into(), json!(input(&m, "#keeper-alive").is_some_and(|i| i.checked())));
        for (k, _, _) in &numbers {
            let raw = input(&m, &format!("[data-kr=\"{k}\"]")).map(|i| i.value()).unwrap_or_default();
            body.insert(k.to_string(), json!(raw.trim()));
        }
        for (k, _) in &dates {
            let raw = input(&m, &format!("[data-kd=\"{k}\"]")).map(|i| i.value()).unwrap_or_default();
            body.insert(k.to_string(), json!(raw));
        }
        let id = id.clone();
        kit_leptos::rt::spawn(async move {
            match api::put(&format!("/keeper/{id}/rules"), Value::Object(body)).await {
                Ok(_) => {
                    toast("Rules saved", "success");
                    reload();
                    m.close();
                }
                Err(e) => set_problem.set(Some(e.message)),
            }
        });
    };
    let footer = move || view! { <button class="btn primary" on:click=save.clone()>"Save"</button> };
    let m = modal::open(ModalOpts::with_view(&format!("{}: rules", s(&a, "name")), body).footer_view(footer).wide(true));
    holder.set_value(Some(m));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_times_in_words() {
        assert_eq!(num(Some(1.0)), "1");
        assert_eq!(num(Some(0.854)), "0.85");
        assert_eq!(num(None), "");
        let now = 100.0 * 86400.0;
        assert_eq!(ago(None, now), "never");
        assert_eq!(ago(Some(now - 3600.0), now), "today");
        assert_eq!(ago(Some(now - 86400.0), now), "yesterday");
        assert_eq!(ago(Some(now - 5.5 * 86400.0), now), "5 days ago");
    }

    #[test]
    fn the_dot_shows_the_worst_risk() {
        assert_eq!(dot_of(&[]), "ok");
        assert_eq!(dot_of(&[json!({ "level": "warn" })]), "warn");
        assert_eq!(dot_of(&[json!({ "level": "warn" }), json!({ "level": "danger" })]), "err");
    }
}
