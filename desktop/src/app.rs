//! The app's state, its messages and what each message does. Every request runs in an iced `Task` with the
//! [`Client`]; its answer comes back as another message. The kit-iced components keep their own state and say
//! what to do next (send this report, save this card, copy this text); this file does it.
//!
//! The stages: [`Stage::Connect`] until a server is chosen, [`Stage::SignIn`] until someone is signed in (and
//! again whenever the server answers 401), then [`Stage::Shell`] with the pages.

use crate::client::{self, ApiError, Client};
use crate::route::Page;
use crate::store::{self, Stored};
use crate::{paths, release, signin};
use iced::{Size, Subscription, Task, Theme};
use kit_iced::bug_report::{self, NewMsg, NewOut, NewReport, Reply, ReplyMsg, ReplyOut};
use kit_iced::details::{self, Details};
use kit_iced::diagnostics;
use kit_iced::drop_zone;
use kit_iced::form::{self, Form, Schema};
use kit_iced::notice::{self, Notice, NoticePoll};
use kit_iced::theme;
use kit_iced::toast::{ToastKind, Toasts};
use kit_iced::update::{Phase, Release, UpdateMsg};
use kit_iced::whats_new::{self, Changelog, ChangelogMsg, Popup, PopupMsg};
use kit_rules::notices::CheckWords;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// Check again's words for the data folder notice (the web app's `data_notice`).
pub const DATA_WORDS: CheckWords = CheckWords {
    failing: "The data folder still can't be written",
    checking: "Still checking the data folder",
    fixed: "The data folder can be written",
};

/// Where the Updates block is, for the toast when a new version was found.
pub const UPDATES_PLACE: &str = "Settings → This computer → Updates";

/// The looks the Look select offers: the kit's themes, `(theme id, name)`.
pub const THEMES: [(&str, &str); 5] = theme::THEMES;

/// Makes the look of a theme id (one of [`THEMES`]) the one every kit-iced style reads: the theme's own accent,
/// solid background; `system` follows a dark system. The default theme is [`theme::default_look`].
pub fn apply_theme(id: &str) {
    if id == theme::THEMES[0].0 {
        theme::set(theme::default_look());
    } else {
        theme::set(theme::palette(id, "", "", "solid", true));
    }
}

/// Where the app is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Asking for the server's address.
    Connect,
    /// Asking for a username and password.
    SignIn,
    /// Signed in: the sidebar and the pages.
    Shell,
}

/// What a confirm dialog asks before doing (the action of kit-iced's [`kit_iced::confirm::Confirm`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// Sign out of this computer.
    SignOut,
    /// Forget the server and connect to another one.
    ChangeServer,
}

impl Confirm {
    /// The dialog's title, text and OK button.
    pub fn words(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Confirm::SignOut => ("Sign out?", "This computer forgets its sign-in. You'll need your password to sign in again.", "Sign out"),
            Confirm::ChangeServer => ("Change server?", "This computer signs out and asks for a server's address again.", "Change server"),
        }
    }

    /// kit-iced's confirm dialog for this action (signing out is red: it can't be undone from here).
    pub fn dialog(self) -> kit_iced::confirm::Confirm<Confirm> {
        let (title, text, ok) = self.words();
        let d = kit_iced::confirm::Confirm::new(title, text, ok, self);
        if self == Confirm::SignOut {
            d.danger()
        } else {
            d
        }
    }
}

/// Which upload a file dialog answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The New report dialog.
    NewReport,
    /// The reply box of the open report.
    Reply,
}

/// Everything the app does.
#[derive(Clone, Debug)]
pub enum Message {
    /// The connect page's field.
    Address(String),
    /// Continue: probe the address.
    Connect,
    /// The probe's answer: the server's base URL, or the words to show.
    Connected(Result<String, String>),
    /// The sign-in page's username.
    Username(String),
    /// The sign-in page's password.
    Password(String),
    /// The sign-in page's authenticator code.
    Code(String),
    /// Sign in.
    SignIn,
    /// The sign-in's answer.
    SignedIn(Result<signin::LoginResult, ApiError>),
    /// Use a different account (forgets the code step) or go back to the connect page.
    SignInBack,
    /// Ask before signing out or changing the server.
    Ask(Confirm),
    /// The confirm dialog's OK or Cancel (or its X, the backdrop, Escape).
    Dialog(kit_iced::confirm::Msg),
    /// A request whose answer needs nothing (sign-out, What's new seen).
    Done,
    /// `/api/auth/me`'s answer.
    Me(Result<Value, ApiError>),
    /// Open a page.
    Open(Page),
    /// The one-second tick: toasts expire, the notice and offline polls count.
    Tick,
    /// The window's size changed (the bug report's device line).
    Resized(Size),
    /// Home's toast buttons: show this toast.
    Toast(String, ToastKind),
    /// Home's Undo toast.
    UndoToast,
    /// The Undo of the Undo toast.
    Undone,
    /// A click on a toast closes it.
    ToastDismiss(u64),
    /// A toast's Copy.
    ToastCopy(u64),
    /// A toast's action button.
    ToastAction(u64),
    /// Copy this text, then check the clipboard.
    Copy(String),
    /// What the clipboard holds after a copy of the first text.
    CopyChecked(String, Option<String>),
    /// Open the details dialog with this title and text.
    ShowDetails(String, String),
    /// The details dialog's messages.
    Details(details::Msg<Page>),
    /// `/api/health`'s answer (start, Retry and the offline poll).
    Health(Result<Value, ApiError>),
    /// The offline banner's Retry.
    Retry,
    /// The offline banner's Details: why, in full.
    OfflineDetails,
    /// The data folder notice's answer.
    Notice(Result<Value, ApiError>),
    /// The notice banner's Check again.
    NoticeCheck,
    /// Check again's answer.
    NoticeChecked(Result<Value, ApiError>),
    /// `/api/changelog/new`'s answer.
    WhatsNewNew(Result<Value, ApiError>),
    /// The What's new popup.
    Popup(PopupMsg),
    /// The popup's or the page's switch was saved (or not).
    PopupSaved(bool, Result<Value, ApiError>),
    /// The What's new page.
    Changelog(ChangelogMsg),
    /// `/api/changelog`'s answer.
    ChangelogLoaded(Result<Value, ApiError>),
    /// The bug report list's answer.
    Reports(Result<Value, ApiError>),
    /// Open the New report dialog.
    OpenNewReport,
    /// Close the New report dialog.
    CloseNewReport,
    /// The New report dialog.
    New(NewMsg),
    /// One report's answer.
    ReportLoaded(i64, Result<Value, ApiError>),
    /// The reply box.
    Reply(ReplyMsg),
    /// Open a file dialog for an upload.
    PickFiles(Pick),
    /// Files dragged over or dropped on the window.
    Drop(drop_zone::Event),
    /// The settings schema's answer.
    Schema(Result<Value, ApiError>),
    /// A settings page's tab.
    SettingsTab(String),
    /// The settings form.
    Form(form::Msg),
    /// The Look select.
    Theme(String),
    /// The Diagnostics page.
    Diagnostics(diagnostics::Msg),
    /// The checks' answer.
    Checked(Result<Value, ApiError>),
    /// The Updates block.
    Update(UpdateMsg),
    /// The release check's answer.
    UpdateChecked(Result<Option<Release>, String>),
    /// The download's answer: the file and its release's SHA256SUMS.
    Downloaded(Result<(PathBuf, Option<String>), String>),
    /// The SHA256SUMS check's answer.
    Verified(Result<PathBuf, String>),
    /// Open a file or folder with the system (a checked download's folder).
    OpenPath(PathBuf),
}

/// The app's state.
pub struct App {
    /// What is remembered between starts.
    pub stored: Stored,
    store_path: Option<PathBuf>,
    /// Where the app is.
    pub stage: Stage,
    /// The server, with this computer's app token once signed in.
    pub client: Client,
    /// The connect page's field.
    pub address: String,
    /// The connect page or the sign-in page is waiting for the server.
    pub busy: bool,
    /// The connect or sign-in page's error, in full.
    pub error: String,
    /// The sign-in page's username.
    pub username: String,
    /// The sign-in page's password.
    pub password: String,
    /// The authenticator code, once the server asked for one.
    pub code: Option<String>,
    /// The signed-in account (`null` until the server said).
    pub me: Value,
    /// The server's version (`/api/health`), "" until known.
    pub server_version: String,
    /// The open page.
    pub page: Page,
    /// The toasts.
    pub toasts: Toasts<Message>,
    /// The details dialog.
    pub details: Option<Details<Page>>,
    /// The confirm dialog.
    pub confirm: Option<kit_iced::confirm::Confirm<Confirm>>,
    /// Why the server can't be reached, while it can't.
    pub offline: Option<String>,
    /// The offline banner's words ("The <app> server isn't answering").
    pub down_words: String,
    /// A Retry runs.
    pub retrying: bool,
    seconds: u64,
    /// The data folder notice.
    pub notice: Option<Notice>,
    poll: NoticePoll,
    /// Check again runs.
    pub notice_checking: bool,
    /// The What's new popup.
    pub popup: Popup,
    /// The What's new page.
    pub changelog: Changelog,
    /// The bug report list (`None` while loading), or the error.
    pub reports: Option<Result<Vec<Value>, String>>,
    /// The New report dialog, while it is open.
    pub new_report: Option<NewReport>,
    /// The open report (`{report, messages}`), or the error.
    pub report: Option<Result<Value, String>>,
    /// The reply box of the open report.
    pub reply: Reply,
    /// The settings schema, or the error.
    pub schema: Option<Result<Schema, String>>,
    /// The settings page's tab (a schema section id).
    pub settings_tab: String,
    /// The settings form.
    pub form: Form,
    /// The Diagnostics page.
    pub diagnostics: diagnostics::State,
    /// The updater.
    pub phase: Phase,
    /// The newer release found.
    pub found: Option<Release>,
    told: String,
    /// The window's size.
    pub window: Size,
}

/// Turns a request's answer into a message.
fn ask<T: Send + 'static>(
    f: impl std::future::Future<Output = T> + Send + 'static,
    then: impl Fn(T) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(f, then)
}

impl App {
    /// A new app from what was remembered (and the server override in the environment), with no tasks.
    pub fn new(stored: Stored, store_path: Option<PathBuf>, server_override: Option<String>) -> App {
        apply_theme(&stored.theme);
        let server = server_override.or_else(|| Some(stored.server.clone()).filter(|s| !s.is_empty()));
        let stage = match &server {
            None => Stage::Connect,
            Some(_) if stored.token.is_empty() => Stage::SignIn,
            Some(_) => Stage::Shell,
        };
        let server = server.unwrap_or_default();
        App {
            client: Client::new(&server, &stored.token),
            address: server,
            stored,
            store_path,
            stage,
            busy: false,
            error: String::new(),
            username: String::new(),
            password: String::new(),
            code: None,
            me: Value::Null,
            server_version: String::new(),
            page: Page::Home,
            toasts: Toasts::default(),
            details: None,
            confirm: None,
            offline: None,
            down_words: kit_rules::offline::mode::server_down(crate::title()),
            retrying: false,
            seconds: 0,
            notice: None,
            poll: NoticePoll::default(),
            notice_checking: false,
            popup: Popup::default(),
            changelog: Changelog::default(),
            reports: None,
            new_report: None,
            report: None,
            reply: Reply::default(),
            schema: None,
            settings_tab: String::new(),
            form: Form::new(),
            diagnostics: diagnostics::State::default(),
            phase: Phase::Idle,
            found: None,
            told: String::new(),
            window: Size::new(1200.0, 800.0),
        }
    }

    /// The app at start: what the config folder remembers, then the shell's first requests when signed in.
    pub fn boot() -> (App, Task<Message>) {
        let path = store::path();
        let stored = path.as_deref().map(store::load_from).unwrap_or_default();
        let mut app = App::new(stored, path, store::server_override());
        app.save();
        let task = if app.stage == Stage::Shell { app.start_shell() } else { Task::none() };
        (app, task)
    }

    /// The window's title.
    pub fn title(&self) -> String {
        crate::title().to_string()
    }

    /// The iced theme from the kit's look.
    pub fn theme(&self) -> Theme {
        theme::iced_theme(&theme::look())
    }

    /// The tick, the window's size, and drag and drop while an upload is open.
    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            iced::window::resize_events().map(|(_, s)| Message::Resized(s)),
        ];
        if self.new_report.is_some() || matches!(self.page, Page::Report(_)) {
            subs.push(drop_zone::events().map(Message::Drop));
        }
        Subscription::batch(subs)
    }

    /// Whether the signed-in account is an admin (the admin pages and the notice show; until the server says,
    /// they show and the server says no there).
    pub fn admin(&self) -> bool {
        self.me.get("is_admin").and_then(Value::as_bool).unwrap_or(self.me.is_null())
    }

    /// Whether the signed-in account is the server's owner (may show saved secrets).
    pub fn owner(&self) -> bool {
        self.me.get("role").and_then(Value::as_str) == Some("owner")
    }

    /// The line a bug report sends as its `device`: this app and version, the system, and the window's size.
    pub fn device_line(&self) -> String {
        bug_report::device_line(
            &format!("{} {} ({})", crate::title(), crate::VERSION, std::env::consts::OS),
            self.window.width,
            self.window.height,
        )
    }

    fn save(&mut self) {
        if let Some(p) = &self.store_path {
            if let Err(e) = store::save_to(p, &self.stored) {
                self.toasts.push(e, ToastKind::Err);
            }
        }
    }

    fn get(&self, path: impl Into<String>, then: impl Fn(Result<Value, ApiError>) -> Message + Send + 'static) -> Task<Message> {
        let c = self.client.clone();
        let path = path.into();
        ask(async move { c.get(path).await }, then)
    }

    fn put(
        &self,
        path: impl Into<String>,
        body: Value,
        then: impl Fn(Result<Value, ApiError>) -> Message + Send + 'static,
    ) -> Task<Message> {
        let c = self.client.clone();
        let path = path.into();
        ask(async move { c.put(path, body).await }, then)
    }

    fn post(
        &self,
        path: impl Into<String>,
        body: Value,
        then: impl Fn(Result<Value, ApiError>) -> Message + Send + 'static,
    ) -> Task<Message> {
        let c = self.client.clone();
        let path = path.into();
        ask(async move { c.post(path, body).await }, then)
    }

    /// The shell's first requests: the health check, who is signed in, What's new, the notice and the update
    /// check (when switched on).
    fn start_shell(&mut self) -> Task<Message> {
        self.stage = Stage::Shell;
        self.page = Page::Home;
        let c = self.client.clone();
        let mut tasks = vec![
            ask(async move { c.health().await }, Message::Health),
            self.get(paths::ME, Message::Me),
            self.get(paths::CHANGELOG_NEW, Message::WhatsNewNew),
            self.get(paths::STORAGE, Message::Notice),
        ];
        if self.stored.check_updates {
            self.phase = Phase::Checking;
            tasks.push(ask(release::check(), Message::UpdateChecked));
        }
        Task::batch(tasks)
    }

    /// A request failed: a 401 asks for the sign-in again, no answer turns offline mode on, anything else is an
    /// error toast with the words in full.
    fn failed(&mut self, e: ApiError) -> Task<Message> {
        if e.signed_out() && self.stage == Stage::Shell {
            self.sign_out_here();
            self.error = e.message;
        } else if e.unreachable() {
            self.offline = Some(e.reason(crate::title()));
        } else {
            self.toasts.push(e.message, ToastKind::Err);
        }
        Task::none()
    }

    /// Forgets the app token and the account's pages, and shows the sign-in page.
    fn sign_out_here(&mut self) {
        self.stored.token.clear();
        self.save();
        self.client = self.client.with_token("");
        self.stage = Stage::SignIn;
        self.password.clear();
        self.code = None;
        self.me = Value::Null;
        self.details = None;
        self.new_report = None;
        self.popup.close();
        self.schema = None;
        self.reports = None;
        self.report = None;
    }

    /// Opens a page and starts its requests.
    fn open(&mut self, page: Page) -> Task<Message> {
        self.page = page;
        match page {
            Page::Home => Task::none(),
            Page::BugReports => {
                self.reports = None;
                self.get(paths::BUG_REPORTS, Message::Reports)
            }
            Page::Report(id) => {
                self.report = None;
                self.reply = Reply::default();
                self.get(paths::bug_report(id), move |r| Message::ReportLoaded(id, r))
            }
            Page::Settings => match &self.schema {
                Some(Ok(_)) => self.open_tab(self.settings_tab.clone()),
                _ => {
                    self.schema = None;
                    self.get(paths::SETTINGS_SCHEMA, Message::Schema)
                }
            },
            Page::Diagnostics => {
                self.diagnostics.open();
                self.get(paths::DIAGNOSTICS, Message::Checked)
            }
            Page::WhatsNew => {
                self.changelog.reset();
                self.get(paths::CHANGELOG, Message::ChangelogLoaded)
            }
        }
    }

    /// Opens a settings tab: its cards, then their values.
    fn open_tab(&mut self, tab: String) -> Task<Message> {
        let Some(Ok(schema)) = &self.schema else { return Task::none() };
        let Some(section) = schema.find(&tab).or(schema.sections.first()) else { return Task::none() };
        self.settings_tab = section.id.to_string();
        let ids: Vec<String> = section.cards.iter().map(|c| c.id.to_string()).collect();
        self.form.open(ids, None);
        self.get(paths::SETTINGS, |r| Message::Form(form::Msg::Values(r.map_err(|e| e.message))))
    }

    /// Copies `text`, then reads the clipboard back to say whether it took.
    fn copy(&self, text: String) -> Task<Message> {
        iced::clipboard::write::<Message>(text.clone())
            .chain(iced::clipboard::read().map(move |got| Message::CopyChecked(text.clone(), got)))
    }

    /// Opens the file dialog for an upload (the bug report's kinds); its answer goes to that upload.
    fn pick_files(target: Pick) -> Task<Message> {
        let dialog = rfd::AsyncFileDialog::new().add_filter(bug_report::FILE_FILTER, bug_report::ATTACH_FILES.exts).pick_files();
        ask(dialog, move |files| {
            let paths = files.unwrap_or_default().into_iter().map(|f| f.path().to_path_buf());
            let picked = bug_report::with_sizes(paths);
            match target {
                Pick::NewReport => Message::New(NewMsg::Picked(picked)),
                Pick::Reply => Message::Reply(ReplyMsg::Picked(picked)),
            }
        })
    }

    /// Applies a message.
    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Address(s) => {
                self.address = s;
                self.error.clear();
            }
            Message::Connect => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.error.clear();
                return ask(client::probe(self.address.clone(), crate::title()), Message::Connected);
            }
            Message::Connected(r) => {
                self.busy = false;
                match r {
                    Ok(base) => {
                        self.address = base.clone();
                        self.stored.server = base.clone();
                        self.stored.token.clear();
                        self.save();
                        self.client = Client::new(&base, "");
                        self.stage = Stage::SignIn;
                    }
                    Err(e) => self.error = e,
                }
            }
            Message::Username(s) => self.username = s,
            Message::Password(s) => self.password = s,
            Message::Code(s) => self.code = Some(s),
            Message::SignIn => {
                if self.busy || !signin::ready(&self.username, &self.password, self.code.as_deref()) {
                    return Task::none();
                }
                self.busy = true;
                self.error.clear();
                let name = signin::device_name(&signin::host_name(), std::env::consts::OS);
                let body = signin::body(&self.username, &self.password, self.code.as_deref(), &self.stored.device_id, &name);
                let c = self.client.clone();
                return ask(async move { c.login(body).await }, Message::SignedIn);
            }
            Message::SignedIn(r) => {
                self.busy = false;
                match r {
                    Ok(signin::LoginResult::Success { token, user }) => {
                        self.stored.token = token.clone();
                        self.save();
                        self.client = self.client.with_token(&token);
                        self.password.clear();
                        self.code = None;
                        let task = self.start_shell();
                        self.me = user;
                        return task;
                    }
                    Ok(signin::LoginResult::NeedsCode(words)) => {
                        self.code = Some(String::new());
                        self.error = words;
                    }
                    Ok(signin::LoginResult::Failed(words)) => self.error = words,
                    Err(e) => self.error = e.message,
                }
            }
            Message::SignInBack => {
                self.error.clear();
                if self.code.is_some() {
                    self.code = None;
                } else if crate::store::server_override().is_none() {
                    self.stage = Stage::Connect;
                }
            }
            Message::Ask(c) => self.confirm = Some(c.dialog()),
            Message::Dialog(m) => {
                let Some(c) = self.confirm.take().and_then(|d| d.update(m)) else { return Task::none() };
                let leaving = self.client.clone();
                self.sign_out_here();
                if c == Confirm::ChangeServer {
                    self.stored.server.clear();
                    self.save();
                    self.stage = Stage::Connect;
                }
                return ask(async move { leaving.post(paths::LOGOUT.into(), json!({})).await }, |_| Message::Done);
            }
            Message::Done => {}
            Message::Me(Ok(me)) => self.me = me,
            Message::Me(Err(e)) => return self.failed(e),
            Message::Open(p) => return self.open(p),
            Message::Tick => return self.tick(),
            Message::Resized(s) => self.window = s,
            Message::Toast(text, kind) => self.toasts.push(text, kind),
            Message::UndoToast => self.toasts.push_action(
                "Moved to the Trash",
                kit_rules::toasts::UNDO,
                Message::Undone,
                Duration::from_millis(kit_rules::toasts::UNDO_MS),
            ),
            Message::Undone => self.toasts.push("Restored", ToastKind::Ok),
            Message::ToastDismiss(id) => self.toasts.dismiss(id),
            Message::ToastCopy(id) => {
                if let Some(text) = self.toasts.text(id) {
                    return self.copy(text);
                }
            }
            Message::ToastAction(id) => {
                if let Some(then) = self.toasts.take_action(id) {
                    return self.update(then);
                }
            }
            Message::Copy(text) => return self.copy(text),
            Message::CopyChecked(want, got) => {
                let (words, kind) = kit_rules::toasts::copy_toast(details::copied(&want, got.as_deref()), details::COPIED);
                self.toasts.push(words, kind);
            }
            Message::ShowDetails(title, text) => self.details = Some(Details::new(title, text, None, None)),
            Message::Details(m) => {
                let Some(d) = self.details.as_mut() else { return Task::none() };
                let close = matches!(m, details::Msg::Close);
                match d.update(m) {
                    Some(details::Out::Copy(text)) => return self.copy(text),
                    Some(details::Out::Open(p)) => {
                        self.details = None;
                        return self.open(p);
                    }
                    Some(details::Out::Act(_)) => self.details = None,
                    None if close => self.details = None,
                    None => {}
                }
            }
            Message::Health(r) => {
                self.retrying = false;
                match r {
                    Ok(v) => {
                        let back = self.offline.take().is_some();
                        self.server_version = v.get("version").and_then(Value::as_str).unwrap_or("").to_string();
                        if back {
                            self.toasts.push("Back online", ToastKind::Ok);
                        }
                    }
                    // any health check that fails is offline mode, with the reason in its words
                    Err(e) => self.offline = Some(e.reason(crate::title())),
                }
            }
            Message::Retry => {
                self.retrying = true;
                let c = self.client.clone();
                return ask(async move { c.health().await }, Message::Health);
            }
            Message::OfflineDetails => {
                let why = self.offline.clone().unwrap_or_default();
                self.details = Some(Details::new(
                    kit_rules::offline::mode::server_down(crate::title()),
                    format!("{}\n\n{why}", self.client.base()),
                    None,
                    None,
                ));
            }
            Message::Notice(Ok(v)) => self.notice = serde_json::from_value(v).ok(),
            Message::Notice(Err(e)) => {
                if e.unreachable() {
                    return self.failed(e);
                }
            }
            Message::NoticeCheck => {
                self.notice_checking = true;
                return self.post(paths::STORAGE_CHECK, json!({}), Message::NoticeChecked);
            }
            Message::NoticeChecked(r) => {
                self.notice_checking = false;
                match r {
                    Ok(v) => {
                        let n: Notice = serde_json::from_value(v).unwrap_or_default();
                        let (words, kind) = notice::check_toast(&n, &DATA_WORDS);
                        self.toasts.push(words, kind);
                        self.notice = Some(n);
                    }
                    Err(e) => return self.failed(e),
                }
            }
            Message::WhatsNewNew(Ok(v)) => {
                if self.popup.loaded(&v) {
                    return self.post(paths::CHANGELOG_SEEN, json!({}), |_| Message::Done);
                }
            }
            Message::WhatsNewNew(Err(_)) => {}
            Message::Popup(m) => match m {
                PopupMsg::Close => self.popup.close(),
                PopupMsg::Full => {
                    self.popup.close();
                    return self.open(Page::WhatsNew);
                }
                PopupMsg::Popup(on) => {
                    self.popup.set_on(on);
                    return self.put(paths::PREFS, json!({ "whats_new_popup": on }), move |r| Message::PopupSaved(on, r));
                }
            },
            Message::PopupSaved(on, r) => match r {
                Ok(_) => self.toasts.push(whats_new::saved_words(on), ToastKind::Ok),
                Err(e) => {
                    if self.popup.is_open() {
                        self.popup.save_failed(on);
                    } else if self.page == Page::WhatsNew {
                        self.changelog.save_failed();
                    }
                    return self.failed(e);
                }
            },
            Message::Changelog(m) => {
                if let Some(on) = self.changelog.update(m) {
                    return self.put(paths::PREFS, json!({ "whats_new_popup": on }), move |r| Message::PopupSaved(on, r));
                }
            }
            Message::ChangelogLoaded(r) => self.changelog.loaded(r.map_err(|e| e.message)),
            Message::Reports(r) => match r {
                Ok(v) => self.reports = Some(Ok(v["reports"].as_array().cloned().unwrap_or_default())),
                Err(e) => {
                    self.reports = Some(Err(e.message.clone()));
                    return self.failed(e);
                }
            },
            Message::OpenNewReport => self.new_report = Some(NewReport { page: self.page.path(), ..NewReport::default() }),
            Message::CloseNewReport => self.new_report = None,
            Message::New(m) => return self.new_report_msg(m),
            Message::ReportLoaded(id, r) => {
                if self.page == Page::Report(id) {
                    self.report = Some(r.clone().map_err(|e| e.message.clone()));
                }
                if let Err(e) = r {
                    return self.failed(e);
                }
            }
            Message::Reply(m) => return self.reply_msg(m),
            Message::PickFiles(target) => return Self::pick_files(target),
            Message::Drop(ev) => {
                if self.new_report.is_some() {
                    return self.new_report_msg(NewMsg::Drop(ev));
                }
                if matches!(self.page, Page::Report(_)) {
                    return self.reply_msg(ReplyMsg::Drop(ev));
                }
            }
            Message::Schema(r) => match r {
                Ok(v) => {
                    self.schema = Some(Schema::from_json(&v));
                    return self.open_tab(self.settings_tab.clone());
                }
                Err(e) => {
                    self.schema = Some(Err(e.message.clone()));
                    return self.failed(e);
                }
            },
            Message::SettingsTab(t) => return self.open_tab(t),
            Message::Form(m) => return self.form_msg(m),
            Message::Theme(id) => {
                apply_theme(&id);
                self.stored.theme = id;
                self.save();
            }
            Message::Diagnostics(m) => match self.diagnostics.update(m) {
                Some(diagnostics::Action::Run) => return self.get(paths::DIAGNOSTICS, Message::Checked),
                Some(diagnostics::Action::CopyReport) => {
                    let first = format!("{} {} diagnostics", crate::title(), self.server_version);
                    return self.copy(diagnostics::report(&first, self.diagnostics.all(), &[]));
                }
                Some(diagnostics::Action::Copy(text)) => return self.copy(text),
                None => {}
            },
            Message::Checked(r) => {
                let err = r.as_ref().err().cloned();
                self.diagnostics.checked(r.map_err(|e| e.message));
                if let Some(e) = err.filter(|e| e.signed_out() || e.unreachable()) {
                    return self.failed(e);
                }
            }
            Message::Update(m) => return self.update_msg(m),
            Message::UpdateChecked(r) => match r {
                Ok(Some(rel)) => {
                    if self.told != rel.version {
                        self.told = rel.version.clone();
                        self.toasts.push(kit_iced::update::ready_words(crate::title(), &rel.version, UPDATES_PLACE), ToastKind::Info);
                    }
                    self.phase = Phase::Available(rel.clone());
                    self.found = Some(rel);
                }
                Ok(None) => self.phase = Phase::UpToDate,
                Err(e) => self.phase = Phase::Failed(e),
            },
            Message::Downloaded(r) => match r {
                Ok((path, sums)) => {
                    self.phase = Phase::Verifying(self.found.as_ref().map(|f| f.version.clone()).unwrap_or_default());
                    return ask(release::verify(path, sums), Message::Verified);
                }
                Err(e) => self.phase = Phase::Failed(e),
            },
            Message::Verified(r) => match r {
                Ok(path) => {
                    self.phase = self.found.clone().map(Phase::Available).unwrap_or_default();
                    let dir = path.parent().map(PathBuf::from).unwrap_or_else(release::download_dir);
                    self.toasts.push_action(release::downloaded_words(&path), "Show", Message::OpenPath(dir), Duration::from_secs(30));
                }
                Err(e) => self.phase = Phase::Failed(e),
            },
            Message::OpenPath(p) => {
                if let Err(e) = open::that_detached(&p) {
                    self.toasts.push(format!("{} couldn't be opened: {e}", p.display()), ToastKind::Err);
                }
            }
        }
        Task::none()
    }

    /// Every second: toasts expire; the notice is asked again on kit-rules' rhythm; offline mode tries again.
    fn tick(&mut self) -> Task<Message> {
        self.toasts.expire();
        if self.stage != Stage::Shell {
            return Task::none();
        }
        self.seconds += 1;
        let mut tasks = Vec::new();
        let notice_every = kit_rules::notices::TICK_MS / 1000;
        if self.seconds.is_multiple_of(notice_every) && self.poll.tick(self.notice.as_ref().is_some_and(Notice::failing)) {
            tasks.push(self.get(paths::STORAGE, Message::Notice));
        }
        let retry_every = kit_rules::offline::mode::RETRY_MS / 1000;
        if self.offline.is_some() && !self.retrying && self.seconds.is_multiple_of(retry_every) {
            let c = self.client.clone();
            tasks.push(ask(async move { c.health().await }, Message::Health));
        }
        Task::batch(tasks)
    }

    fn new_report_msg(&mut self, m: NewMsg) -> Task<Message> {
        let device = self.device_line();
        let Some(d) = self.new_report.as_mut() else { return Task::none() };
        let outs = bug_report::new_update(d, m, &self.server_version, &device);
        let mut tasks = Vec::new();
        for out in outs {
            match out {
                NewOut::PickFiles => tasks.push(Self::pick_files(Pick::NewReport)),
                NewOut::Error(e) => self.toasts.push(e, ToastKind::Err),
                NewOut::Done(w) => self.toasts.push(w, ToastKind::Ok),
                NewOut::Send(r) => {
                    let c = self.client.clone();
                    let fields = r.fields().iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                    tasks.push(ask(
                        async move { c.multipart(paths::BUG_REPORTS.into(), fields, bug_report::FILE_FIELD, r.files).await },
                        |res| Message::New(NewMsg::Sent(res.map(|v| v["id"].as_i64().unwrap_or(0)).map_err(|e| e.message))),
                    ));
                }
                NewOut::Show(id) => {
                    self.new_report = None;
                    tasks.push(self.open(Page::Report(id)));
                }
            }
        }
        Task::batch(tasks)
    }

    fn reply_msg(&mut self, m: ReplyMsg) -> Task<Message> {
        let Page::Report(id) = self.page else { return Task::none() };
        let mut tasks = Vec::new();
        for out in bug_report::reply_update(&mut self.reply, m) {
            match out {
                ReplyOut::PickFiles => tasks.push(Self::pick_files(Pick::Reply)),
                ReplyOut::Error(e) => self.toasts.push(e, ToastKind::Err),
                ReplyOut::Done(w) => self.toasts.push(w, ToastKind::Ok),
                ReplyOut::Send(post) => {
                    let c = self.client.clone();
                    let fields = post.fields().iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                    tasks.push(ask(
                        async move { c.multipart(paths::bug_report_messages(id), fields, bug_report::FILE_FIELD, post.files).await },
                        |res| Message::Reply(ReplyMsg::Sent(res.map(|_| ()).map_err(|e| e.message))),
                    ));
                }
                ReplyOut::Reload => tasks.push(self.get(paths::bug_report(id), move |r| Message::ReportLoaded(id, r))),
            }
        }
        Task::batch(tasks)
    }

    fn form_msg(&mut self, m: form::Msg) -> Task<Message> {
        let Some(Ok(schema)) = &self.schema else { return Task::none() };
        let outs = self.form.update(m, schema.cards());
        let mut tasks = Vec::new();
        for out in outs {
            tasks.push(match out {
                form::Out::Save { card, body } => self.put(paths::SETTINGS, Value::Object(body), move |r| {
                    Message::Form(form::Msg::Saved(card.clone(), r.map_err(|e| e.message)))
                }),
                form::Out::Test { card, test, body } => {
                    let c = self.client.clone();
                    ask(
                        async move {
                            let saved = c.put(paths::SETTINGS.into(), Value::Object(body)).await.map_err(|e| e.message)?;
                            let tested = c.post(paths::settings_test(&test), json!({})).await.map_err(|e| e.message);
                            Ok((saved, tested))
                        },
                        move |r| Message::Form(form::Msg::Tested(card.clone(), r)),
                    )
                }
                form::Out::Clear { body, .. } => {
                    self.put(paths::SETTINGS, Value::Object(body), |r| Message::Form(form::Msg::Cleared(r.map_err(|e| e.message))))
                }
                form::Out::Reveal { key } => self
                    .get(paths::settings_secret(&key), move |r| Message::Form(form::Msg::Revealed(key.clone(), r.map_err(|e| e.message)))),
                form::Out::CopySecret { key } => {
                    self.get(paths::settings_secret(&key), |r| Message::Form(form::Msg::Copied(r.map_err(|e| e.message))))
                }
                form::Out::Copy(text) | form::Out::CopyText(text) => self.copy(text),
                form::Out::ToastOk(w) => {
                    self.toasts.push(w, ToastKind::Ok);
                    Task::none()
                }
                form::Out::ToastErr(w) => {
                    self.toasts.push(w, ToastKind::Err);
                    Task::none()
                }
            });
        }
        Task::batch(tasks)
    }

    fn update_msg(&mut self, m: UpdateMsg) -> Task<Message> {
        match m {
            UpdateMsg::CheckAtStart(on) => {
                self.stored.check_updates = on;
                self.save();
            }
            UpdateMsg::Check => {
                if !self.phase.busy() {
                    self.phase = Phase::Checking;
                    return ask(release::check(), Message::UpdateChecked);
                }
            }
            UpdateMsg::Install => {
                if let Some(r) = self.found.clone().filter(|_| !self.phase.busy()) {
                    let total = r.asset.as_ref().map(|a| a.size).unwrap_or(0);
                    self.phase = Phase::Downloading { version: r.version.clone(), got: 0, total };
                    return ask(release::download(r, release::download_dir()), Message::Downloaded);
                }
            }
            UpdateMsg::OpenReleases => return self.update(Message::OpenPath(PathBuf::from(release::feed().page))),
        }
        Task::none()
    }

    /// The whole window.
    pub fn view(&self) -> iced::Element<'_, Message> {
        crate::ui::view(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(server: Option<&str>, token: &str) -> App {
        let stored = Stored { server: server.unwrap_or("").into(), token: token.into(), device_id: "d".into(), ..Stored::default() };
        App::new(stored, None, None)
    }

    #[test]
    fn the_stage_follows_what_is_remembered() {
        assert_eq!(app(None, "").stage, Stage::Connect);
        assert_eq!(app(Some("https://x.example"), "").stage, Stage::SignIn);
        assert_eq!(app(Some("https://x.example"), "t").stage, Stage::Shell);
        let over = App::new(Stored::default(), None, Some("http://10.0.0.2:8080".into()));
        assert_eq!((over.stage, over.client.base()), (Stage::SignIn, "http://10.0.0.2:8080"), "the environment's server wins");
    }

    #[test]
    fn a_401_asks_for_the_sign_in_again() {
        let mut a = app(Some("https://x.example"), "t");
        let _ = a.failed(ApiError { status: 401, message: "Not authenticated".into(), failure: None });
        assert_eq!(a.stage, Stage::SignIn);
        assert!(a.stored.token.is_empty() && !a.client.signed_in());
        assert_eq!(a.error, "Not authenticated");
    }

    #[test]
    fn no_answer_turns_offline_mode_on_and_an_answer_turns_it_off() {
        let mut a = app(Some("https://x.example"), "t");
        let _ = a.failed(ApiError { status: 0, message: "x".into(), failure: Some(kit_rules::offline::reason::Failure::Timeout) });
        assert_eq!(a.offline.as_deref(), Some("The server didn't answer within 8 s."));
        let _ = a.update(Message::Health(Ok(json!({ "ok": true, "version": "1.2.3" }))));
        assert!(a.offline.is_none());
        assert_eq!(a.server_version, "1.2.3");
        assert_eq!(a.toasts.list().last().map(|t| t.text.as_str()), Some("Back online"));
    }

    #[test]
    fn other_errors_are_toasts_that_stay() {
        let mut a = app(Some("https://x.example"), "t");
        let _ = a.failed(ApiError { status: 500, message: "Disk full".into(), failure: None });
        assert!(a.toasts.list()[0].stays());
    }

    #[test]
    fn undo_comes_back_as_its_message() {
        let mut a = app(Some("https://x.example"), "t");
        let _ = a.update(Message::UndoToast);
        let id = a.toasts.list()[0].id;
        let _ = a.update(Message::ToastAction(id));
        assert_eq!(a.toasts.list().iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["Restored"]);
    }

    #[test]
    fn a_copy_is_checked() {
        let mut a = app(None, "");
        let _ = a.update(Message::CopyChecked("abc".into(), Some("abc".into())));
        let _ = a.update(Message::CopyChecked("abc".into(), None));
        let words: Vec<&str> = a.toasts.list().iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, [details::COPIED, details::COPY_FAILED]);
    }

    #[test]
    fn confirming_sign_out_forgets_the_token_but_keeps_the_server() {
        let mut a = app(Some("https://x.example"), "t");
        let _ = a.update(Message::Ask(Confirm::SignOut));
        assert_eq!(a.confirm.as_ref().map(|d| (d.action, d.danger)), Some((Confirm::SignOut, true)));
        let _ = a.update(Message::Dialog(kit_iced::confirm::Msg::Cancel));
        assert_eq!((a.confirm.is_none(), a.stage), (true, Stage::Shell), "Cancel changes nothing");
        let _ = a.update(Message::Ask(Confirm::SignOut));
        let _ = a.update(Message::Dialog(kit_iced::confirm::Msg::Ok));
        assert_eq!((a.stage, a.stored.server.as_str(), a.stored.token.as_str()), (Stage::SignIn, "https://x.example", ""));
        let _ = a.update(Message::Ask(Confirm::ChangeServer));
        let _ = a.update(Message::Dialog(kit_iced::confirm::Msg::Ok));
        assert_eq!((a.stage, a.stored.server.as_str()), (Stage::Connect, ""));
    }

    #[test]
    fn the_notice_shows_for_admins_and_check_again_says_how_it_went() {
        let mut a = app(Some("https://x.example"), "t");
        let _ = a.update(Message::Me(Ok(json!({ "username": "ana", "is_admin": true }))));
        let _ = a.update(Message::Notice(Ok(json!({ "ok": false, "text": "The data folder /data is read-only" }))));
        assert!(a.notice.as_ref().is_some_and(Notice::failing));
        let _ = a.update(Message::NoticeChecked(Ok(json!({ "ok": true }))));
        assert_eq!(a.toasts.list()[0].text, DATA_WORDS.fixed);
        assert!(!a.notice.as_ref().unwrap().failing());
    }

    #[test]
    fn a_new_report_names_the_page_it_came_from() {
        let mut a = app(Some("https://x.example"), "t");
        a.page = Page::Settings;
        let _ = a.update(Message::OpenNewReport);
        assert_eq!(a.new_report.as_ref().unwrap().page, "#/settings");
        let _ = a.update(Message::New(NewMsg::Send));
        assert_eq!(a.toasts.list()[0].text, "Give the report a title", "the dialog's own check runs first");
        let _ = a.update(Message::CloseNewReport);
        assert!(a.new_report.is_none());
        assert!(a.device_line().contains(" · 1200x800"));
    }

    #[test]
    fn a_found_update_is_told_once() {
        let mut a = app(Some("https://x.example"), "t");
        let rel = Release { version: "9.0.0".into(), page: String::new(), notes: String::new(), asset: None, sums: None };
        let _ = a.update(Message::UpdateChecked(Ok(Some(rel.clone()))));
        let _ = a.update(Message::UpdateChecked(Ok(Some(rel))));
        assert_eq!(a.toasts.list().len(), 1);
        assert!(matches!(a.phase, Phase::Available(_)));
        let _ = a.update(Message::UpdateChecked(Err("Couldn't check for updates: offline".into())));
        assert_eq!(a.phase, Phase::Failed("Couldn't check for updates: offline".into()));
    }

    #[test]
    fn the_looks() {
        for (id, _) in THEMES {
            apply_theme(id);
        }
        apply_theme(THEMES[0].0);
        assert_eq!(theme::look().accent, theme::default_look().accent);
    }
}
