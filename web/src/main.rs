//! The web app: the kit's shell (sidebar, notice slot, view) drawn with Leptos, and every kit feature in it.
//!
//! | Where | What |
//! | --- | --- |
//! | everywhere | toasts with Copy, the details dialog, the data folder notice with Check again, the offline and connection banners, self-update after a new server build |
//! | after signing in | the What's new popup, once per release |
//! | `#/` | Home: the app's own start page (a bug report button to begin with) |
//! | `#/bug-reports` | the account's bug reports and the New report sheet |
//! | `#/settings?s=<page>` | the server settings form from the server's schema, Diagnostics and What's new |
//!
//! The app's own pages go next to [`Home`], one component each, with a link in [`Shell`].

mod pages;

use kit_leptos::api::{self, HttpBackend};
use kit_leptos::{details, dropzone, notice, offline, rt, toasts, update, whats_new};
use leptos::prelude::*;
use serde_json::Value;
use std::rc::Rc;
use wasm_bindgen::JsCast;

/// The package name, `<project>-web`: trunk names the script `<project>-web-<hash>.js`, whose hash the
/// server's `/api/health` names as the web build.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// The app's name as people see it, from `<meta name="application-name">` in index.html.
pub fn app_name() -> String {
    rt::qs(r#"meta[name="application-name"]"#).and_then(|m| m.get_attribute("content")).unwrap_or_else(|| "App".into())
}

/// The project name (`NAME` without `-web`): the server's binary, the `X-Requested-With` value, the `x-<project>`
/// mark on the server's answers and the prefix of the app's events (`<project>:server-down`). The name people
/// see ([`app_name`]) may have spaces and capitals, which a header name may not.
pub fn project() -> &'static str {
    NAME.strip_suffix("-web").unwrap_or(NAME)
}

/// This page's web build: the hash in its script's name, `<project>-web-<hash>.js`.
fn this_build() -> Option<String> {
    let head = rt::document()?.head()?.inner_html();
    kit_rules::update::build_in(&head, NAME)
}

/// The connection banner (kit-leptos `offline::watch_connection`, with the project name for the events and
/// the app's name in the words): "Not connected" while offline, "The <app> server isn't answering" while the
/// server is down.
fn watch_connection(app: &str) {
    use kit_rules::offline::mode::{server_down, NOT_CONNECTED};
    let down = server_down(app);
    rt::on_window("offline", |_| offline::conn_banner(true, NOT_CONNECTED));
    rt::on_window("online", |_| offline::conn_banner(false, ""));
    rt::on_window(&format!("{}:server-down", project()), move |_| offline::conn_banner(true, &down));
    rt::on_window(&format!("{}:server-up", project()), |_| offline::conn_banner(false, ""));
}

/// The kit's configuration for this app: its id and name, its script stem and where its What's new page is.
/// The server uses the kit's default routes, so no path is set here.
fn kit_config(app: &str) -> kit_leptos::config::AppConfig {
    let mut cfg = kit_leptos::config::AppConfig::new(kit_rules::app::App::new(project(), app));
    cfg.script = NAME.to_string();
    cfg.routes.changelog = "/settings?s=whats-new".into();
    cfg
}

/// The page the address names: its path (`/settings`) and query (`s=diagnostics`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Route {
    /// the part after `#`, before `?`
    pub path: String,
    /// the part after `?`
    pub query: String,
}

impl Route {
    /// The route of a `location.hash` (`#/settings?s=general`).
    pub fn parse(hash: &str) -> Route {
        let h = hash.trim_start_matches('#');
        let (path, query) = h.split_once('?').unwrap_or((h, ""));
        Route { path: if path.is_empty() { "/".into() } else { path.to_string() }, query: query.to_string() }
    }

    /// The value of `key` in the query, "" when it is not there.
    pub fn param(&self, key: &str) -> String {
        self.query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| v.to_string()).unwrap_or_default()
    }
}

/// The address's route now.
fn current_route() -> Route {
    Route::parse(&rt::window().and_then(|w| w.location().hash().ok()).unwrap_or_default())
}

/// Opens `path` of the app (`/settings?s=whats-new`).
pub fn navigate(path: &str) {
    if let Some(w) = rt::window() {
        let _ = w.location().set_hash(path);
    }
}

/// Starts the kit's parts that live outside the page, then draws the sign-in page or the shell.
fn main() {
    console_error();
    let app = app_name();
    kit_leptos::config::set(kit_config(&app));
    api::set_backend(HttpBackend::new(project()));
    toasts::mount();
    details::init();
    dropzone::init();
    watch_connection(&app);
    let mut cfg = update::UpdateConfig::new(&app);
    cfg.build = this_build();
    update::start(cfg);
    rt::spawn(async move {
        let me = api::get("/auth/me").await.ok();
        let Some(root) = rt::by_id("app").and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) else { return };
        match me {
            Some(me) => {
                let admin = me.get("is_admin").and_then(Value::as_bool).unwrap_or(false);
                leptos::mount::mount_to(root, move || view! { <Shell me=me.clone()/> }).forget();
                // the notice slot is part of the shell: start once it is drawn
                notice::start(pages::data_notice(), admin);
                notice::mount();
                whats_new::start(Rc::new(navigate));
            }
            None => leptos::mount::mount_to(root, pages::SignIn).forget(),
        }
    });
}

/// Panics go to the browser console with their message.
fn console_error() {
    std::panic::set_hook(Box::new(|p| web_sys::console::error_1(&p.to_string().into())));
}

/// One link of the sidebar.
#[component]
fn NavLink(href: &'static str, label: &'static str, route: ReadSignal<Route>) -> impl IntoView {
    let active = move || route.get().path == href || (href != "/" && route.get().path.starts_with(href));
    view! { <a href=format!("#{href}") data-nav=href class:active=active><span>{label}</span></a> }
}

/// The question Sign out asks first (the kit's confirm dialog; the desktop app asks the same).
pub fn sign_out_question() -> kit_rules::dialogs::Confirm {
    kit_rules::dialogs::Confirm::new("Sign out?")
        .text("This browser forgets its sign-in. You'll need your password to sign in again.")
        .ok("Sign out")
        .danger(true)
}

/// The kit's shell: the sidebar with the app's pages, the notice slot and the page.
#[component]
fn Shell(me: Value) -> impl IntoView {
    let (route, set_route) = signal(current_route());
    rt::on_window("hashchange", move |_| set_route.set(current_route()));
    let name = app_name();
    let username = me.get("username").and_then(Value::as_str).unwrap_or("").to_string();
    let role = match me.get("role").and_then(Value::as_str) {
        Some("owner") => "Owner",
        _ if me.get("is_admin").and_then(Value::as_bool) == Some(true) => "Admin",
        _ => "Member",
    };
    let sign_out = move |_| {
        rt::spawn(async {
            if kit_leptos::modal::confirm(sign_out_question()).await {
                let _ = api::post_empty("/auth/logout").await;
                rt::reload();
            }
        })
    };
    let page = move || {
        let r = route.get();
        match r.path.as_str() {
            "/bug-reports" => view! { <pages::BugReports/> }.into_any(),
            "/settings" => view! { <pages::Settings page=r.param("s")/> }.into_any(),
            _ => view! { <pages::Home/> }.into_any(),
        }
    };
    view! {
        <div class="shell">
            <aside class="sidebar" id="sidebar">
                <a class="brand" href="#/">{name}</a>
                <nav class="nav">
                    <NavLink href="/" label="Home" route=route/>
                </nav>
                <nav class="nav nav-under">
                    <NavLink href="/bug-reports" label="Bug reports" route=route/>
                </nav>
                <div class="sidebar-foot" style="margin-top:0">
                    <div class="grow" style="min-width:0">
                        <div class="ellipsis" style="font-weight:650">{username}</div>
                        <div class="small faint">{role}</div>
                    </div>
                    <a class="btn ghost icon" href="#/settings" title="Settings" aria-label="Settings" inner_html=kit_leptos::icons::icon("settings")></a>
                    <button class="btn ghost" id="logout-btn" title="Sign out" on:click=sign_out>"Sign out"</button>
                </div>
            </aside>
            <main class="main">
                <div id="server-notice"></div>
                <div id="view">{page}</div>
            </main>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::Route;

    #[test]
    fn sign_out_asks_first() {
        let q = super::sign_out_question();
        assert_eq!((q.title.as_str(), q.ok.as_str(), q.danger), ("Sign out?", "Sign out", true));
    }

    #[test]
    fn routes_come_from_the_hash() {
        assert_eq!(Route::parse(""), Route { path: "/".into(), query: String::new() });
        assert_eq!(Route::parse("#/"), Route { path: "/".into(), query: String::new() });
        let r = Route::parse("#/settings?s=diagnostics&x=1");
        assert_eq!(r.path, "/settings");
        assert_eq!(r.param("s"), "diagnostics");
        assert_eq!(r.param("missing"), "");
    }
}
