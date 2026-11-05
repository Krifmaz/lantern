//! The app's desktop app: Iced 0.14 with the kit's components ([kit_iced]) and rules ([kit_rules]), on the
//! server's API with an app token for this computer.
//!
//! | Module | What it holds |
//! | --- | --- |
//! | [`app`] | the state, the messages and what each one does (the requests it starts) |
//! | [`ui`] | the pages and the shell, drawn with kit-iced |
//! | [`route`] | the pages and the web paths that name them |
//! | [`paths`] | every server path the app calls |
//! | [`client`] | the HTTP client (JSON, the app token, multipart uploads) and the connect page's probe |
//! | [`address`] | the server address rules (what is tried for what was typed) |
//! | [`signin`] | the sign-in rules (the body, the answer, this computer's name) |
//! | [`store`] | what is remembered between starts, in the user's config folder |
//! | [`release`] | the update notice's GitHub side: this app's releases and the file for this computer |
//!
//! The names come from Cargo.toml (`CARGO_PKG_NAME`, `CARGO_PKG_REPOSITORY`) and app.json (the title people see,
//! and the release files' prefix), so the Rust files hold no app name.

pub mod address;
pub mod app;
pub mod client;
pub mod paths;
pub mod release;
pub mod route;
pub mod signin;
pub mod store;
pub mod ui;

use serde::Deserialize;
use std::sync::LazyLock;

/// The app's logo in the sidebar and on the connect and sign-in pages, drawn in the look's accent gradient: the
/// kit's neutral mark until the app has its own (`Logo { view_box, paths, gradient }`).
pub const LOGO: kit_iced::widgets::Logo = kit_iced::widgets::Logo::KIT;

/// The package name, `<project>-desktop`: the binary's name.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// This build's version: `APP_VERSION` at build time (the release's package script sets it), else Cargo.toml's.
/// The update notice compares the releases with it.
pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The project name ([`NAME`] without `-desktop`): the config folder's name and the server override's prefix.
pub fn project() -> &'static str {
    NAME.strip_suffix("-desktop").unwrap_or(NAME)
}

/// app.json: the names the template filled in.
#[derive(Deserialize)]
struct Meta {
    /// The app's name as people see it ("Kit Demo").
    title: String,
    /// The title without spaces, the release files' prefix ("KitDemo").
    file: String,
}

static META: LazyLock<Meta> =
    LazyLock::new(|| serde_json::from_str(include_str!("../app.json")).unwrap_or(Meta { title: project().into(), file: project().into() }));

/// The app's name as people see it (app.json's `title`).
pub fn title() -> &'static str {
    &META.title
}

/// The release files' prefix (app.json's `file`: the title without spaces).
pub fn file_stem() -> &'static str {
    &META.file
}

/// Opens the window and runs the app until it is closed.
pub fn run() -> iced::Result {
    iced::application(app::App::boot, app::App::update, app::App::view)
        .title(app::App::title)
        .subscription(app::App::subscription)
        .theme(app::App::theme)
        .window_size((1200.0, 800.0))
        .run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_names_are_filled_in() {
        assert!(!super::title().is_empty() && !super::title().contains('{'));
        assert!(!super::file_stem().contains(' '));
        assert_eq!(format!("{}-desktop", super::project()), super::NAME);
        assert!(semver_like(super::VERSION), "{}", super::VERSION);
    }

    fn semver_like(v: &str) -> bool {
        v.split('.').count() == 3 && v.split('.').all(|p| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty())
    }
}
