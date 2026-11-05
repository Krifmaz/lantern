//! The pages and the shell, drawn with kit-iced: the connect and sign-in pages, the sidebar, the banners
//! (offline, notice), Home, Bug reports, one report, Settings, Diagnostics and What's new, and the dialogs and
//! toasts laid over them. Each function draws from the [`App`] and sends [`Message`]s; nothing here changes state.

use crate::app::{App, Confirm, Message, Stage, THEMES};
use crate::route::Page;
use iced::widget::{button, column, container, row, scrollable, space, stack};
use iced::{Alignment, Length};
use kit_iced::settings::{self as s, result_line_copy};
use kit_iced::theme::{self, Btn};
use kit_iced::toast::ToastKind;
use kit_iced::widgets::{self as w, btn, btn_icon, btn_sm, El, Icon};
use kit_iced::{bug_report, details, diagnostics, form, notice, offline, toast, update, whats_new};
use serde_json::Value;

/// The Diagnostics page's line under its title.
pub const DIAGNOSTICS_HELP: &str = "What the server checks, with the raw answers. Copy report puts it all on the clipboard.";

/// A string field of a JSON object, "" when missing.
fn js(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// The whole window: the stage's page, then the dialogs, then the toasts.
pub fn view(app: &App) -> El<'_, Message> {
    let base = match app.stage {
        Stage::Connect => connect(app),
        Stage::SignIn => sign_in(app),
        Stage::Shell => shell(app),
    };
    let mut layers = vec![container(base).width(Length::Fill).height(Length::Fill).style(theme::app_bg).into()];
    layers.extend(overlay(app));
    layers.push(toast::view(app.toasts.list(), 18.0, Message::ToastDismiss, Message::ToastCopy, Message::ToastAction));
    stack(layers).width(Length::Fill).height(Length::Fill).into()
}

/// The dialogs over the page, at most one of each: What's new, New report, details, confirm.
pub fn overlay(app: &App) -> Vec<El<'_, Message>> {
    let mut out = Vec::new();
    if app.stage == Stage::Shell {
        if let Some(p) = whats_new::popup(&app.popup) {
            out.push(p.map(Message::Popup));
        }
        if let Some(d) = &app.new_report {
            out.push(bug_report::new_dialog(d, &app.server_version, Message::New, Message::CloseNewReport));
        }
    }
    if let Some(d) = &app.details {
        out.push(details::view(d).map(Message::Details));
    }
    if let Some(c) = &app.confirm {
        out.push(confirm(c));
    }
    out
}

/// The open confirm dialog (kit-iced's `confirm`: the question, Cancel and the action's button).
pub fn confirm(d: &kit_iced::confirm::Confirm<Confirm>) -> El<'_, Message> {
    kit_iced::confirm::view(d).map(Message::Dialog)
}

/// An error in full, red, with Copy (nothing when there is none).
fn error_line(e: &str) -> Option<El<'_, Message>> {
    (!e.is_empty()).then(|| result_line_copy(false, e, Message::Copy))
}

/// The card in the middle of the window the connect and sign-in pages use.
fn centred<'a>(body: impl Into<El<'a, Message>>) -> El<'a, Message> {
    container(w::card(container(body).width(Length::Fill)).max_width(420)).center(Length::Fill).padding(20).into()
}

/// The connect page: the server's address (remembered after it answered).
pub fn connect(app: &App) -> El<'_, Message> {
    let go = (!app.busy).then_some(Message::Connect);
    let mut c = column![
        row![w::logo(&crate::LOGO, 32.0), w::h2(crate::title())].spacing(10).align_y(Alignment::Center),
        w::muted("Connect to your server").size(14),
        w::field("Server address", s::input("server.example.com", &app.address, Message::Address).on_submit_maybe(go.clone())),
    ]
    .spacing(16);
    if let Some(e) = error_line(&app.error) {
        c = c.push(e);
    }
    c = c.push(btn(if app.busy { "Connecting…" } else { "Continue" }, Btn::Primary, go).width(Length::Fill));
    centred(c)
}

/// The sign-in page: username, password, and the code when the server asks for one.
pub fn sign_in(app: &App) -> El<'_, Message> {
    let ready = crate::signin::ready(&app.username, &app.password, app.code.as_deref());
    let go = (ready && !app.busy).then_some(Message::SignIn);
    let mut c = column![
        row![w::logo(&crate::LOGO, 32.0), w::h2(crate::signin::title(crate::title()))].spacing(10).align_y(Alignment::Center),
        s::between(w::small_muted(app.client.base().to_string()).into(), s::link_muted("Change server", Message::SignInBack)),
        w::field("Username", s::input("", &app.username, Message::Username).on_submit_maybe(go.clone())),
        w::field("Password", s::input("", &app.password, Message::Password).secure(true).on_submit_maybe(go.clone())),
    ]
    .spacing(14);
    if let Some(code) = &app.code {
        c = c.push(w::field(crate::signin::CODE_PROMPT, s::input("123456", code, Message::Code).on_submit_maybe(go.clone())));
    }
    if let Some(e) = error_line(&app.error) {
        c = c.push(e);
    }
    c = c.push(btn(if app.busy { "Signing in…" } else { "Sign in" }, Btn::Primary, go).width(Length::Fill));
    if app.code.is_some() {
        c = c.push(s::link_muted("Use a different account", Message::SignInBack));
    }
    centred(c)
}

/// The sidebar and the open page, under the offline and notice banners.
pub fn shell(app: &App) -> El<'_, Message> {
    let p = theme::look();
    let mut nav =
        column![row![w::logo(&crate::LOGO, 26.0), w::h3(crate::title())].spacing(10).align_y(Alignment::Center), space().height(12)]
            .spacing(4);
    for page in Page::NAV.into_iter().filter(|pg| app.admin() || !pg.admin_only()) {
        let on = app.page.nav() == page;
        nav = nav.push(
            button(
                row![w::icon(page.icon(), 16.0, if on { p.text } else { p.muted }), w::t(page.title()).size(14)]
                    .spacing(10)
                    .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([8, 12])
            .style(theme::btn(if on { Btn::NavOn } else { Btn::Nav }))
            .on_press(Message::Open(page)),
        );
    }
    let who = js(&app.me, "username");
    nav = nav.push(space().height(Length::Fill));
    if !who.is_empty() {
        nav = nav.push(row![w::avatar(&who, 26.0), w::small_muted(who)].spacing(8).align_y(Alignment::Center));
    }
    nav = nav.push(btn_icon(Icon::Logout, "Sign out", Btn::Ghost, Some(Message::Ask(Confirm::SignOut))));
    let sidebar = container(nav.height(Length::Fill)).width(220).height(Length::Fill).padding(16).style(theme::sidebar);

    let mut main = column![].width(Length::Fill).height(Length::Fill);
    if let Some(b) = offline_banner(app) {
        main = main.push(container(b).padding([8, 16]));
    }
    if let Some(b) = notice_banner(app) {
        main = main.push(container(b).padding([8, 16]));
    }
    let body = container(page(app)).padding(28).max_width(1040).width(Length::Fill);
    main = main.push(scrollable(container(body).center_x(Length::Fill)).height(Length::Fill).style(theme::scroll));
    row![sidebar, main].into()
}

/// The offline banner while the server can't be reached: Details (why) and Retry.
pub fn offline_banner(app: &App) -> Option<El<'_, Message>> {
    app.offline.as_ref()?;
    let retry = if app.retrying { kit_rules::offline::mode::TRYING } else { offline::RETRY };
    Some(offline::banner(&app.down_words, vec![("Details", false, Message::OfflineDetails), (retry, true, Message::Retry)]))
}

/// The data folder notice (admins), with Check again.
pub fn notice_banner(app: &App) -> Option<El<'_, Message>> {
    let n = app.notice.as_ref()?;
    let lines: Vec<String> =
        notice::banners(std::slice::from_ref(n), app.admin()).into_iter().filter_map(|n| n.banner()).map(str::to_string).collect();
    notice::banner(Icon::Download, lines, Some((app.notice_checking, Message::NoticeCheck)))
}

/// The open page.
pub fn page(app: &App) -> El<'_, Message> {
    match app.page {
        Page::Home => home(app),
        Page::BugReports => bug_reports(app),
        Page::Report(_) => report(app),
        Page::Settings => settings(app),
        Page::Diagnostics => diagnostics::view(&app.diagnostics, DIAGNOSTICS_HELP, format!("{} {}", crate::title(), app.server_version))
            .map(Message::Diagnostics),
        Page::WhatsNew => whats_new::changelog(&app.changelog, crate::title()).map(Message::Changelog),
    }
}

/// Home: the server, and a button for every kit feature (replace "Try the kit" with the app's own first page).
pub fn home(app: &App) -> El<'_, Message> {
    let server = if app.server_version.is_empty() {
        app.client.base().to_string()
    } else {
        format!("{} · {} {}", app.client.base(), crate::title(), app.server_version)
    };
    let toast =
        |label: &'static str, words: &'static str, kind: ToastKind| btn_sm(label, Btn::Default, Some(Message::Toast(words.into(), kind)));
    let mut pages = row![
        btn_sm("What's new", Btn::Default, Some(Message::Open(Page::WhatsNew))),
        btn_sm("Bug reports", Btn::Default, Some(Message::Open(Page::BugReports))),
        btn_sm("Settings", Btn::Default, Some(Message::Open(Page::Settings))),
    ]
    .spacing(8);
    if app.admin() {
        pages = pages.push(btn_sm("Diagnostics", Btn::Default, Some(Message::Open(Page::Diagnostics))));
    }
    column![
        w::page_head(
            crate::title(),
            Some(s::help(server).into()),
            vec![btn_sm("Change server", Btn::Default, Some(Message::Ask(Confirm::ChangeServer))).into()]
        ),
        s::card(
            "Try the kit",
            vec![
                s::help("Toasts: an error stays with Copy, the same error does not pile up, Undo brings it back.").into(),
                row![
                    toast("Info", "Saved", ToastKind::Info),
                    toast("OK", "Done", ToastKind::Ok),
                    toast("Warning", "The data folder is almost full", ToastKind::Warn),
                    toast("Error", "The server answered 502 Bad Gateway – it may be restarting", ToastKind::Err),
                    btn_sm("Undo", Btn::Default, Some(Message::UndoToast)),
                ]
                .spacing(8)
                .wrap()
                .into(),
                s::help("Dialogs: any text in full with Copy, and a question before anything that can't be undone.").into(),
                row![
                    btn_sm(
                        "Details",
                        Btn::Default,
                        Some(Message::ShowDetails(
                            "Details".into(),
                            format!(
                                "reqwest::Error: error sending request for url ({}/api/health)\n  caused by: connection refused",
                                app.client.base()
                            )
                        ))
                    ),
                    btn_sm("Sign out…", Btn::Default, Some(Message::Ask(Confirm::SignOut))),
                    btn_icon(Icon::Bug, "New report", Btn::Default, Some(Message::OpenNewReport)),
                    btn_sm("Check for updates", Btn::Default, Some(Message::Update(update::UpdateMsg::Check))),
                ]
                .spacing(8)
                .wrap()
                .into(),
            ]
        ),
        s::card("Pages", vec![pages.wrap().into()]),
    ]
    .spacing(16)
    .into()
}

/// Bug reports: New report, then the reports this account may see.
pub fn bug_reports(app: &App) -> El<'_, Message> {
    let mut c = column![w::page_head(
        "Bug reports",
        Some(s::help("What went wrong, with screenshots or logs. The admin answers in each report's thread.").into()),
        vec![btn_icon(Icon::Plus, "New report", Btn::Primary, Some(Message::OpenNewReport)).into()]
    )]
    .spacing(14);
    match &app.reports {
        None => c = c.push(w::skeleton_rows(4)),
        Some(Err(e)) => c = c.push(result_line_copy(false, e, Message::Copy)),
        Some(Ok(list)) if list.is_empty() => c = c.push(w::card(w::muted("No reports yet")).width(Length::Fill)),
        Some(Ok(list)) => {
            for r in list {
                let id = r["id"].as_i64().unwrap_or(0);
                let line = bug_report::count_words(r["message_count"].as_i64().unwrap_or(0), r["file_count"].as_i64().unwrap_or(0));
                let item = row![
                    column![
                        w::t(js(r, "title")).size(15).font(theme::semibold()),
                        w::small_muted(format!("by {} · {line}", js(r, "username")))
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                    bug_report::kind_badge(&js(r, "kind")),
                    bug_report::status_badge(&js(r, "status")),
                ]
                .spacing(10)
                .align_y(Alignment::Center);
                c = c.push(
                    button(w::card(item).width(Length::Fill))
                        .padding(0)
                        .width(Length::Fill)
                        .style(theme::btn(Btn::Row))
                        .on_press(Message::Open(Page::Report(id))),
                );
            }
        }
    }
    c.into()
}

/// One report: its words, its files, the thread, and the reply box (files can be dropped on it).
pub fn report(app: &App) -> El<'_, Message> {
    let mut c = column![s::link("← Bug reports", Message::Open(Page::BugReports))].spacing(14);
    let data = match &app.report {
        None => return c.push(w::skeleton_rows(3)).into(),
        Some(Err(e)) => return c.push(result_line_copy(false, e, Message::Copy)).into(),
        Some(Ok(d)) => d,
    };
    let r = &data["report"];
    let title = js(r, "title");
    let body = js(r, "body");
    c = c.push(w::page_head(
        title.clone(),
        Some(s::help(bug_report::meta_line(r)).into()),
        vec![
            bug_report::kind_badge(&js(r, "kind")),
            bug_report::status_badge(&js(r, "status")),
            btn_sm("Show in full", Btn::Default, Some(Message::ShowDetails(title, body.clone()))).into(),
        ],
    ));
    c = c.push(w::card(column![w::t(body).size(14)].extend(files(r))).width(Length::Fill));
    for m in data["messages"].as_array().into_iter().flatten() {
        let head = w::small_muted(js(m, "username")).font(theme::semibold());
        c = c.push(w::card(column![head, w::t(js(m, "body")).size(14)].spacing(6).extend(files(m))).width(Length::Fill));
    }
    c.push(bug_report::reply_view(&app.reply).map(Message::Reply)).into()
}

/// The names of a report's or a reply's files.
fn files<'a>(v: &Value) -> Option<El<'a, Message>> {
    let names: Vec<String> = v["files"].as_array().into_iter().flatten().map(|f| js(f, "name")).filter(|n| !n.is_empty()).collect();
    (!names.is_empty()).then(|| container(w::small_muted(format!("Files: {}", names.join(", ")))).padding([8, 0]).into())
}

/// Settings: the server's settings form from its schema (admins), then This computer: the look and updates.
pub fn settings(app: &App) -> El<'_, Message> {
    let mut c = column![w::page_head("Settings", None, vec![])].spacing(14);
    if app.admin() {
        match &app.schema {
            None => c = c.push(w::skeleton_rows(4)),
            Some(Err(e)) => c = c.push(result_line_copy(false, e, Message::Copy)),
            Some(Ok(schema)) => {
                if schema.sections.len() > 1 {
                    let tabs: Vec<(&str, &str)> = schema.sections.iter().map(|sec| (&*sec.id, &*sec.title)).collect();
                    c = c.push(w::tabs(&tabs, &app.settings_tab, Message::SettingsTab));
                }
                if let Some(e) = error_line(app.form.error()) {
                    c = c.push(e);
                }
                if let Some(section) = schema.find(&app.settings_tab) {
                    if app.form.values().is_none() && app.form.error().is_empty() {
                        c = c.push(w::skeleton_rows(3));
                    } else if let Some(values) = app.form.values() {
                        for (part, cards) in section.parts() {
                            if !part.is_empty() {
                                c = c.push(form::part_heading(part));
                            }
                            for card in cards.into_iter().filter(|card| card.shown(values)) {
                                c = c.push(app.form.card_view(card, app.owner(), Message::Form, form::Slots::default()));
                            }
                        }
                    }
                }
            }
        }
    }
    let found = app.found.as_ref().map(|r| r.version.as_str());
    let updates: Vec<El<'_, Message>> = update::lines(crate::title(), crate::VERSION, app.stored.check_updates, &app.phase, found)
        .into_iter()
        .map(|l| l.map(Message::Update))
        .collect();
    c.push(s::card(
        "This computer",
        vec![w::field("Look", s::select_table(&THEMES, &app.stored.theme, Message::Theme)), s::block("Updates", updates)],
    ))
    .into()
}

/// The pages drawn headless (iced_test), each with the button that matters clicked.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Message as M;
    use crate::store::Stored;
    use kit_iced::bug_report::{NewMsg, ReplyMsg};
    use serde_json::json;

    fn app(admin: bool) -> App {
        let stored = Stored { server: "https://x.example".into(), token: "t".into(), device_id: "d".into(), ..Stored::default() };
        let mut a = App::new(stored, None, None);
        a.me = json!({ "username": "ana", "is_admin": admin, "role": if admin { "owner" } else { "user" } });
        a
    }

    fn clicked(a: &App, label: &str) -> Vec<Message> {
        let mut ui = iced_test::simulator(view(a));
        ui.click(label).unwrap_or_else(|e| panic!("{label}: {e:?}"));
        ui.into_messages().collect()
    }

    fn shows(a: &App, words: &str) -> bool {
        iced_test::simulator(view(a)).find(words).is_ok()
    }

    #[test]
    fn the_connect_page_asks_for_the_address() {
        let a = App::new(Stored::default(), None, None);
        assert!(shows(&a, "Connect to your server"));
        assert!(matches!(clicked(&a, "Continue")[..], [M::Connect]));
        let mut busy = App::new(Stored::default(), None, None);
        busy.busy = true;
        busy.error = "Can't reach that server".into();
        assert!(shows(&busy, "Connecting…") && shows(&busy, "Can't reach that server"));
        assert!(clicked(&busy, "Copy").iter().any(|m| matches!(m, M::Copy(t) if t == "Can't reach that server")), "the error has Copy");
    }

    #[test]
    fn sign_in_waits_for_both_fields() {
        let mut a = App::new(Stored { server: "https://x.example".into(), ..Stored::default() }, None, None);
        assert!(shows(&a, &crate::signin::title(crate::title())));
        assert!(clicked(&a, "Sign in").is_empty(), "nothing typed: Sign in is off");
        a.username = "ana".into();
        a.password = "secret".into();
        assert!(matches!(clicked(&a, "Sign in")[..], [M::SignIn]));
        a.code = Some(String::new());
        assert!(shows(&a, crate::signin::CODE_PROMPT));
        assert!(matches!(clicked(&a, "Use a different account")[..], [M::SignInBack]));
    }

    #[test]
    fn home_tries_every_feature() {
        let a = app(true);
        assert!(shows(&a, "Try the kit"));
        assert!(matches!(&clicked(&a, "Error")[..], [M::Toast(_, ToastKind::Err)]));
        assert!(matches!(clicked(&a, "Undo")[..], [M::UndoToast]));
        assert!(matches!(clicked(&a, "Details")[..], [M::ShowDetails(..)]));
        assert!(matches!(clicked(&a, "Sign out…")[..], [M::Ask(Confirm::SignOut)]));
        assert!(matches!(clicked(&a, "New report")[..], [M::OpenNewReport]));
        assert!(matches!(clicked(&a, "Check for updates")[..], [M::Update(update::UpdateMsg::Check)]));
    }

    #[test]
    fn the_admin_pages_are_for_admins() {
        assert!(shows(&app(true), "Diagnostics"));
        assert!(!shows(&app(false), "Diagnostics"));
        assert!(matches!(clicked(&app(false), "What's new")[..], [M::Open(Page::WhatsNew)]));
    }

    #[test]
    fn the_banners_offer_retry_and_check_again() {
        let mut a = app(true);
        a.offline = Some("The server didn't answer within 8 s.".into());
        a.notice = serde_json::from_value(json!({ "ok": false, "text": "The data folder /data is read-only" })).ok();
        assert!(shows(&a, &a.down_words) && shows(&a, "The data folder /data is read-only"));
        assert!(matches!(clicked(&a, offline::RETRY)[..], [M::Retry]));
        assert!(matches!(clicked(&a, notice::CHECK_AGAIN)[..], [M::NoticeCheck]));
        let mut user = app(false);
        user.notice = a.notice.clone();
        assert!(!shows(&user, "The data folder /data is read-only"), "the notice is for admins");
    }

    #[test]
    fn bug_reports_list_and_open() {
        let mut a = app(false);
        a.page = Page::BugReports;
        a.reports = Some(Ok(vec![
            json!({ "id": 3, "title": "Crash on start", "kind": "bug", "status": "new", "username": "ana", "message_count": 1 }),
        ]));
        assert!(shows(&a, "by ana · 1 reply"));
        assert!(matches!(clicked(&a, "Crash on start")[..], [M::Open(Page::Report(3))]));
        a.page = Page::Report(3);
        a.report = Some(Ok(json!({
            "report": { "id": 3, "title": "Crash on start", "body": "It closes", "kind": "bug", "status": "looking", "username": "ana", "created_at": 0, "files": [{ "name": "shot.png" }] },
            "messages": [{ "username": "admin", "body": "Looking into it" }],
        })));
        assert!(shows(&a, "It closes") && shows(&a, "Looking into it") && shows(&a, "Files: shot.png"));
        assert!(matches!(clicked(&a, "Send")[..], [M::Reply(ReplyMsg::Send)]));
        assert!(matches!(clicked(&a, "Show in full")[..], [M::ShowDetails(..)]));
    }

    #[test]
    fn the_new_report_dialog_takes_files() {
        let mut a = app(false);
        let _ = a.update(M::OpenNewReport);
        assert!(shows(&a, "Drop images, .txt or .log files to attach them") || shows(&a, "Add images"));
        assert!(matches!(clicked(&a, "Add images")[..], [M::New(NewMsg::AddFiles)]));
    }

    #[test]
    fn settings_come_from_the_schema() {
        let mut a = app(true);
        a.page = Page::Settings;
        let schema = json!({ "sections": [{ "id": "general", "title": "General", "groups": [
            { "id": "site", "title": "Site", "fields": [{ "key": "site_name", "label": "Site name", "type": "text" }] },
        ] }] });
        let _ = a.update(M::Schema(Ok(schema)));
        assert_eq!(a.settings_tab, "general");
        let _ = a.update(M::Form(form::Msg::Values(Ok(json!({ "site_name": "Notes" })))));
        assert!(shows(&a, "Site") && shows(&a, "Site name"));
        assert!(matches!(&clicked(&a, "Save")[..], [M::Form(form::Msg::Save(card))] if card == "site"));
        assert!(shows(&a, "This computer") && shows(&a, &format!("Version {}", crate::VERSION)));
        assert!(matches!(clicked(&a, "Check for updates")[..], [M::Update(update::UpdateMsg::Check)]));
    }

    #[test]
    fn diagnostics_and_whats_new_draw_the_servers_answers() {
        let mut a = app(true);
        a.page = Page::Diagnostics;
        a.diagnostics
            .checked(Ok(json!({ "checks": [{ "name": "Data folder", "kind": "path", "ok": true, "detail": "/data · writable" }] })));
        assert!(shows(&a, "Data folder") && shows(&a, diagnostics::ALL_GOOD));
        assert!(matches!(clicked(&a, diagnostics::COPY_REPORT)[..], [M::Diagnostics(diagnostics::Msg::CopyReport)]));
        a.page = Page::WhatsNew;
        a.changelog.reset();
        a.changelog.loaded(Ok(json!({ "version": "0.2.0", "popup": true, "releases": [{ "title": "Faster start", "version": "0.2.0", "blocks": [{ "k": "p", "text": "It starts in a second." }] }] })));
        assert!(shows(&a, "Faster start") && shows(&a, "It starts in a second."));
    }

    #[test]
    fn the_dialogs_and_toasts_lie_over_the_page() {
        let mut a = app(true);
        let _ = a.update(M::WhatsNewNew(Ok(json!({ "show": true, "releases": [{ "title": "0.2.0", "highlights": ["Faster start"] }] }))));
        assert!(shows(&a, "Faster start"));
        assert!(matches!(clicked(&a, whats_new::FULL_CHANGELOG)[..], [M::Popup(whats_new::PopupMsg::Full)]));
        a.popup.close();

        a.toasts.push("Disk full", ToastKind::Err);
        assert!(matches!(clicked(&a, details::COPY)[..], [M::ToastCopy(_)]));
        a.toasts = Default::default();

        let _ = a.update(M::ShowDetails("What the server said".into(), "line one".into()));
        assert!(shows(&a, "What the server said"), "the text itself is in a selectable editor");
        assert!(matches!(clicked(&a, details::COPY)[..], [M::Details(details::Msg::Copy)]));
        a.details = None;

        let mut c = App::new(Stored::default(), None, None);
        c.confirm = Some(Confirm::ChangeServer.dialog());
        assert!(shows(&c, "Change server?"));
        assert!(matches!(clicked(&c, "Change server")[..], [M::Dialog(kit_iced::confirm::Msg::Ok)]));
        assert!(matches!(clicked(&c, kit_iced::confirm::CANCEL)[..], [M::Dialog(kit_iced::confirm::Msg::Cancel)]));
    }
}
