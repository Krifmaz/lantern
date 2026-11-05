//! The app's pages: sign-in, Home, Bug reports and Settings (the server's settings form, Diagnostics and
//! What's new). Words and markup are the kit's, so the kit's stylesheet (static/css/app.css) draws them.

use crate::{app_name, navigate};
use kit_leptos::api;
use kit_leptos::bug_report::{self, SheetOpts};
use kit_leptos::diagnostics::DiagnosticsPage;
use kit_leptos::icons::icon;
use kit_leptos::notice::NoticeSource;
use kit_leptos::schema_form::schema::{visible_cards, Schema};
use kit_leptos::schema_form::SettingsCards;
use kit_leptos::whats_new::ChangelogPage;
use kit_rules::notices::CheckWords;
use leptos::prelude::*;
use serde_json::{json, Value};
use std::rc::Rc;

/// The data folder notice: `GET /api/notices/data`, Check again posts `/api/notices/data/check` (the server's
/// `checks.rs`). Check again's words name the data folder.
pub fn data_notice() -> NoticeSource {
    NoticeSource {
        path: "/notices/data".into(),
        check_path: "/notices/data/check".into(),
        part: String::new(),
        words: CheckWords {
            failing: "The data folder still can't be written",
            checking: "Still checking the data folder",
            fixed: "The data folder can be written",
        },
        words_for: Vec::new(),
        after_check: None,
    }
}

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The sign-in page (username and password).
#[component]
pub fn SignIn() -> impl IntoView {
    let (error, set_error) = signal(None::<String>);
    let (busy, set_busy) = signal(false);
    let username = NodeRef::<leptos::html::Input>::new();
    let password = NodeRef::<leptos::html::Input>::new();
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let (u, p) = (username.get().map(|i| i.value()).unwrap_or_default(), password.get().map(|i| i.value()).unwrap_or_default());
        set_busy.set(true);
        set_error.set(None);
        kit_leptos::rt::spawn(async move {
            match api::post("/auth/login", json!({ "username": u, "password": p })).await {
                Ok(_) => {
                    navigate("/");
                    kit_leptos::rt::reload();
                }
                Err(ex) => {
                    set_busy.set(false);
                    set_error.set(Some(ex.message));
                }
            }
        });
    };
    view! {
        <div class="auth-wrap">
            <div class="auth-card">
                <div class="brand">{app_name()}</div>
                <p class="muted" style="text-align:center;margin:0">"Sign in"</p>
                <div id="auth-err" class="error-box" class:hidden=move || error.get().is_none() style="margin-top:16px">{move || error.get()}</div>
                <form autocomplete="on" on:submit=submit>
                    <label class="field"><span>"Username"</span><input class="input" name="username" autocomplete="username" required autofocus node_ref=username/></label>
                    <label class="field"><span>"Password"</span><input class="input" type="password" name="password" autocomplete="current-password" required node_ref=password/></label>
                    <button class="btn primary" type="submit" style="padding:12px" disabled=busy>"Sign in"</button>
                </form>
            </div>
        </div>
    }
}

/// Opens the New report sheet for the page `from`, with the server's version.
fn new_report(from: &'static str) {
    kit_leptos::rt::spawn(async move {
        let version = api::get("/health").await.ok().map(|h| s(&h, "version")).unwrap_or_default();
        let mut o = SheetOpts::new(&app_name(), from, &version);
        o.on_sent = Some(Rc::new(|_| navigate("/bug-reports")));
        bug_report::open(o);
    });
}

/// Home: the app's own start page. Replace it with the app's first feature.
#[component]
pub fn Home() -> impl IntoView {
    let name = app_name();
    view! {
        <section class="hero">
            <h1>{name.clone()}</h1>
            <p class="muted" style="margin:6px 0 0">{format!("{name} is ready. Its pages go here.")}</p>
        </section>
        <div class="row wrap mt" style="gap:8px">
            <a class="btn" href="#/settings">"Settings"</a>
            <a class="btn" href="#/settings?s=diagnostics">"Diagnostics"</a>
            <a class="btn" href="#/settings?s=whats-new">"What's new"</a>
            <button class="btn primary" on:click=|_| new_report("#/") inner_html=format!("{}New report", icon("bug"))></button>
        </div>
    }
}

/// One report of the list (title, author and replies).
fn report_row(r: &Value) -> impl IntoView {
    let n = r.get("message_count").and_then(Value::as_i64).unwrap_or(0);
    let count = match n {
        0 => "No replies".to_string(),
        1 => "1 reply".to_string(),
        n => format!("{n} replies"),
    };
    view! {
        <div class="list-item bug-row" style="flex-wrap:wrap;gap:10px">
            <div class="grow" style="min-width:200px">
                <span class="bug-title" style="font-weight:650">{s(r, "title")}</span>
                <div class="small faint">{format!("by {}", s(r, "username"))}</div>
            </div>
            <span class="small muted bug-count">{count}</span>
        </div>
    }
}

/// `#/bug-reports`: the account's reports (an admin's: everyone's) and New report.
#[component]
pub fn BugReports() -> impl IntoView {
    let list = LocalResource::new(|| async { api::get("/bug-reports").await.map_err(|e| e.message) });
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let reports = v.get("reports").and_then(Value::as_array).cloned().unwrap_or_default();
            if reports.is_empty() {
                view! { <div class="empty" id="bugs-empty" inner_html=format!(r#"{}<p class="muted">No reports yet. New report writes the first one.</p>"#, icon("bug"))></div> }.into_any()
            } else {
                view! { <div class="list">{reports.iter().map(report_row).collect_view()}</div> }.into_any()
            }
        }
    };
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Bug reports"</h1>
            <button class="btn primary" id="bugs-new" on:click=|_| new_report("#/bug-reports") inner_html=format!("{}New report", icon("plus"))></button>
        </div>
        <div class="card flat mt" id="bugs-list">{rows}</div>
    }
}

/// `#/settings?s=<page>`: the server's settings pages from its schema, Diagnostics and What's new.
#[component]
pub fn Settings(page: String) -> impl IntoView {
    let data = LocalResource::new(|| async {
        let schema = api::get("/settings/schema").await.map_err(|e| e.message)?;
        let values = api::get("/settings").await.map_err(|e| e.message)?;
        Ok::<_, String>((Schema::from_json(&schema), values))
    });
    let page = if page.is_empty() { "general".to_string() } else { page };
    let app = app_name();
    let tabs = {
        let page = page.clone();
        move || {
            let sections: Vec<(String, String)> = match data.get() {
                Some(Ok((schema, _))) => schema.sections.iter().map(|s| (s.id.clone(), s.title.clone())).collect(),
                _ => vec![],
            };
            let fixed = [("diagnostics".to_string(), "Diagnostics".to_string()), ("whats-new".to_string(), "What's new".to_string())];
            sections
                .into_iter()
                .chain(fixed)
                .map(|(id, title)| {
                    let on = id == page;
                    view! { <a href=format!("#/settings?s={id}") class:active=on>{title}</a> }
                })
                .collect_view()
        }
    };
    let body = {
        let page = page.clone();
        move || match page.as_str() {
            "diagnostics" => view! { <DiagnosticsPage app=app.clone()/> }.into_any(),
            "whats-new" => view! { <ChangelogPage app=app.clone()/> }.into_any(),
            id => match data.get() {
                None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
                Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
                Some(Ok((schema, values))) => {
                    let cards = schema.section(id).map(|s| visible_cards(&s.groups, &values)).unwrap_or_default();
                    view! { <SettingsCards cards=cards values=values/> }.into_any()
                }
            },
        }
    };
    view! {
        <h1>"Settings"</h1>
        <div class="tabs" id="settings-tabs">{tabs}</div>
        <div class="mt">{body}</div>
    }
}
