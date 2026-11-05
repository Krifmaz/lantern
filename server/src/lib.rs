//! The server: the app's state, the kit server crates wired to it, and the web app served at `/`.
//!
//! | Route | From |
//! | --- | --- |
//! | `GET /api/health` | kit-server-health |
//! | `/api/changelog`, `/api/changelog/new`, `/api/changelog/seen` (What's new), `/api/auth/prefs` (the popup switch) | kit-server-changelog |
//! | `/api/notices`, `/api/notices/check`, `/api/notices/{id}`, `/api/notices/{id}/check` | kit-server-notices |
//! | `/api/bug-reports/*`, `/api/crash-reports` | kit-server-bugreports |
//! | `/api/diagnostics`, `/api/diagnostics/{id}/test` | kit-server-diagnostics |
//! | `/api/settings`, `/api/settings/schema`, `/api/settings/secret/{key}` | kit-server-settings |
//! | `/api/auth/login`, `/api/auth/logout`, `/api/auth/me` | [`auth`] |
//! | `/api/indexers/*`, `/api/search`, `/{id}/api` (Torznab/Newznab), `/{id}/download`, `/api/v1/*` (Prowlarr API) | [`indexers`] |
//! | `/api/solver`, `/api/solver/test`; the FlareSolverr API on its own port ([`solver_port`]) | [`solver`] |
//! | `/api/hub/*` (download clients, apps, search and grab, live events) | [`hub`] |
//! | `/api/vault`, `/api/vault/*` (the account vault) | [`vault`] |
//! | `/api/keeper`, `/api/keeper/*` (the account keeper) | [`keeper`] |
//! | `/api/import/*` (Prowlarr and FlareSolverr, dry run first) | [`migrate`] |
//! | `/api/releases/*` (IRC announces, RSS, filters) | [`releases`] |
//! | `/api/cross-seed/*` | [`crossseed`] |
//! | `GET`/`POST /api/setup` (first-run "Create your account" with the setup code from the log) | kit-server-setup |
//! | `/prowlarr/*` (Prowlarr's web frontend, forked in `ui/prowlarr/`, and its `initialize.json`) | [`prowlarr_ui`] |
//!
//! Everything the app adds of its own goes into [`app`] next to them.

pub mod auth;
pub mod checks;
pub mod crossseed;
pub mod hub;
pub mod indexers;
pub mod keeper;
pub mod migrate;
pub mod releases;
pub mod solver;
pub mod vault;

use axum::http::{header, HeaderValue};
use axum::Router;
use kit_server_changelog::{Changelog, ChangelogHost};
use kit_server_core::{Account, Host, PrefsStore, SqliteDb, SqlitePrefs};
use kit_server_settings::{DefaultValue, Field, Group, KeyDef, Section, SettingsHost, SqliteSettings, Store, Table, Values};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};

/// The settings table generated from settings.toml by build.rs: `Keys` (every setting's name), `TABLE` and
/// `SCHEMA`.
pub mod settings_gen {
    #![allow(missing_docs)]
    use super::{DefaultValue, Field, Group, KeyDef, Section};
    include!(concat!(env!("OUT_DIR"), "/settings_gen.rs"));
}

/// The release notes built in from docs/releases by build.rs: `NOTES`, (version, Markdown) pairs.
mod changelog_gen {
    include!(concat!(env!("OUT_DIR"), "/changelog_gen.rs"));
}

/// The server's settings table (settings.toml).
pub static SETTINGS: Table = Table { keys: settings_gen::TABLE, sections: settings_gen::SCHEMA };

/// The server's version: `APP_VERSION` at build time (the release workflow sets it), else Cargo.toml's.
pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The binary's name (`CARGO_PKG_NAME`, the project name): the session cookie's prefix, the web app's script
/// name (`<name>-web-<hash>.js`), the `X-Requested-With` value the web app sends and the `x-<name>` mark on every
/// answer.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// The port from `APP_PORT`, 9696 (Prowlarr's, so apps find Lantern where they expect it) when it is not set or not
/// a number.
pub fn port() -> u16 {
    std::env::var("APP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9696)
}

/// The FlareSolverr API's port from `APP_SOLVER_PORT`: 8191 (FlareSolverr's) when not set, 0 to serve none.
pub fn solver_port() -> u16 {
    std::env::var("APP_SOLVER_PORT").ok().and_then(|p| p.trim().parse().ok()).unwrap_or(8191)
}

/// The app's state, handed to every route; clones share everything.
#[derive(Clone)]
pub struct AppState {
    /// The database (`<data>/app.db`): the app's own accounts and sessions, and the kit's `kit_*` tables.
    pub db: SqliteDb,
    /// The bug reports and crash logs, in the kit's `kit_bug_report*` and `kit_client_crashes` tables.
    pub reports: kit_server_bugreports::SqliteReports,
    /// Each account's preferences (What's new's seen version and switch), in the kit's `kit_account_prefs`.
    pub prefs: SqlitePrefs,
    /// The settings in effect: environment, then saved (the kit's `kit_settings`), then settings.toml's defaults.
    pub settings: Store,
    /// The data folder: the database, bug report files. The only folder the server writes to.
    pub data_dir: PathBuf,
    /// The built web app (trunk's dist), served at `/`.
    pub static_dir: PathBuf,
    /// Prowlarr's built web frontend (`ui/prowlarr`'s `_output/UI`), served at `/prowlarr/`:
    /// `APP_PROWLARR_UI_DIR` (default /app/ui/prowlarr).
    pub prowlarr_ui_dir: PathBuf,
    /// What's new: the release notes built into this server.
    pub changelog: Arc<Changelog>,
    /// The notices (kept: the data folder notice holds its last probe).
    pub notices: kit_server_notices::Notices<AppState>,
    /// Sign-in and bug report limits per address or account.
    pub limits: Arc<auth::Limits>,
    /// The indexers and their definitions.
    pub indexers: indexers::Indexers,
    /// Signs the download links handed to apps.
    pub signer: indexers::LinkSigner,
    /// Gets past Cloudflare checks with a headless browser (also the FlareSolverr API).
    pub solver: solver::Solver,
    /// The download hub: download clients, apps and their grabs.
    pub hub: hub::Hub,
    /// The account vault: site logins and keys, encrypted at rest.
    pub vault: vault::Vault,
    /// Releases as they appear: IRC announces, RSS, filters.
    pub releases: releases::Releases,
    /// Cross-seeding: finished torrents seeded on the other sites that have the same files.
    pub cross_seed: crossseed::CrossSeed,
    /// First-run setup's one-time code: open while there is no owner ([`auth::ensure_owner`]), closed after.
    pub setup: Arc<kit_server_setup::SetupCode>,
}

impl AppState {
    /// The state from the environment: the data folder `APP_DATA_DIR` (default /data) and the web app in
    /// `APP_STATIC_DIR` (default /app/static).
    pub async fn from_env() -> anyhow::Result<AppState> {
        let data = PathBuf::from(std::env::var("APP_DATA_DIR").unwrap_or_else(|_| "/data".into()));
        let static_dir = PathBuf::from(std::env::var("APP_STATIC_DIR").unwrap_or_else(|_| "/app/static".into()));
        let st = AppState::open(&data, static_dir, |name| std::env::var(name).ok()).await?;
        auth::ensure_owner(&st, std::env::var("APP_ADMIN_USER").ok(), std::env::var("APP_ADMIN_PASSWORD").ok())?;
        st.ensure_api_key().await?;
        Ok(st)
    }

    /// The state on the data folder `data` (made when missing) with the web app in `static_dir`; `env` reads
    /// the settings' environment variables (a test passes its own).
    pub async fn open(
        data: &Path,
        static_dir: PathBuf,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> anyhow::Result<AppState> {
        let env = Arc::new(env);
        let env_settings = {
            let env = env.clone();
            move |name: &str| env(name)
        };
        std::fs::create_dir_all(data).map_err(|e| anyhow::anyhow!("The data folder {} can't be made: {e}", data.display()))?;
        let db = SqliteDb::open(&data.join("app.db")).map_err(|e| anyhow::anyhow!("The database can't be opened: {}", e.detail))?;
        auth::migrate(&db)?;
        indexers::migrate(&db)?;
        hub::migrate(&db)?;
        keeper::migrate(&db)?;
        migrate::migrate(&db)?;
        releases::migrate(&db)?;
        solver::migrate(&db)?;
        let solver = solver::Solver::new(solver::find_program(env("APP_CHROMIUM").as_deref()), data, Some(db.clone()));
        crossseed::migrate(&db)?;
        let prowlarr_ui_dir = PathBuf::from(env("APP_PROWLARR_UI_DIR").unwrap_or_else(|| "/app/ui/prowlarr".into()));
        let bundled = PathBuf::from(env("APP_DEFINITIONS_DIR").unwrap_or_else(|| "/app/definitions".into()));
        let library = indexers::library::Library::new(Some(bundled), data);
        let signer = indexers::LinkSigner::open(data)?;
        let vault = vault::Vault::open(db.clone(), data, env("APP_VAULT_PASSPHRASE"))?;
        let kit = |e: kit_server_core::Error| anyhow::anyhow!("The kit's tables can't be made: {}", e.detail);
        let prefs = SqlitePrefs::new(db.clone()).map_err(kit)?;
        let reports = kit_server_bugreports::SqliteReports::new(db.clone()).map_err(kit)?;
        let saved = SqliteSettings::new(db.clone()).map_err(kit)?;
        let notices = checks::notices().keep_in(Arc::new(kit_server_notices::SqliteNotices::new(db.clone()).map_err(kit)?));
        notices.restore().await.map_err(kit)?;
        let settings = Store::load_with(saved, SETTINGS, env_settings)
            .await
            .map_err(|e| anyhow::anyhow!("The settings can't be read: {}", e.detail))?;
        let changelog = Arc::new(Changelog::new(&settings.str("app_name"), VERSION, changelog_gen::NOTES));
        let indexers = indexers::Indexers::new(db.clone(), library)
            .with_settings(settings.clone())
            .with_solver(solver.clone())
            .with_vault(vault.clone());
        match indexers.seal_saved() {
            Ok(0) => {}
            Ok(n) => tracing::info!("{n} indexers' logins and cookies sealed in the vault"),
            Err(e) => tracing::warn!("The indexers' logins can't be sealed yet: {}", e.detail),
        }
        let hub = hub::Hub::new(db.clone(), indexers.clone(), signer.clone(), settings.clone(), data);
        let announce_dir = PathBuf::from(env("APP_ANNOUNCE_DIR").unwrap_or_else(|| "/app/announce".into()));
        let announce = releases::announce::Library::new(vec![announce_dir, data.join("announce")]);
        let releases = releases::Releases::new(db.clone(), announce, indexers.clone(), signer.clone(), settings.clone());
        releases.set_hub(Arc::new(hub.clone()));
        Ok(AppState {
            db,
            reports,
            prefs,
            settings,
            data_dir: data.to_path_buf(),
            static_dir,
            prowlarr_ui_dir,
            changelog,
            notices,
            limits: Arc::new(auth::Limits::default()),
            indexers,
            signer,
            cross_seed: crossseed::CrossSeed::new(hub.clone()),
            setup: Arc::new(kit_server_setup::SetupCode::closed()),
            hub,
            vault,
            releases,
            solver,
        })
    }

    /// Makes the API key apps use (Torznab, the Prowlarr API) when there is none yet, so apps can be pointed at
    /// Lantern straight away. One set in the environment is left alone.
    pub async fn ensure_api_key(&self) -> anyhow::Result<()> {
        if self.settings.str("api_key").is_empty() && !self.settings.env_locked("api_key") {
            let key = {
                use rand::Rng;
                let mut b = [0u8; 16];
                rand::rng().fill(&mut b[..]);
                hex::encode(b)
            };
            self.settings
                .set_many(&serde_json::json!({ "api_key": key }))
                .await
                .map_err(|e| anyhow::anyhow!("The API key can't be saved: {}", e.detail))?;
        }
        Ok(())
    }

    /// The app's name as people see it (the `app_name` setting).
    pub fn app_name(&self) -> String {
        self.settings.str("app_name")
    }
}

impl kit_server_setup::SetupHost for AppState {
    fn setup_code(&self) -> &kit_server_setup::SetupCode {
        &self.setup
    }

    async fn has_owner(&self) -> kit_server_core::Result<bool> {
        use kit_server_core::Db;
        Ok(self.db.one("SELECT id FROM users WHERE role='owner'", kit_server_core::p![])?.is_some())
    }

    async fn create_owner(&self, owner: kit_server_setup::NewOwner) -> kit_server_core::Result<()> {
        auth::create_owner(&self.db, owner).await
    }
}

impl Host for AppState {
    async fn signed_in(&self, parts: &mut axum::http::request::Parts) -> kit_server_core::Result<Account> {
        auth::signed_in(self, parts)
    }
}

impl ChangelogHost for AppState {
    fn changelog(&self) -> &Changelog {
        &self.changelog
    }

    fn prefs(&self) -> &dyn PrefsStore {
        &self.prefs
    }
}

impl SettingsHost for AppState {
    fn settings(&self) -> &dyn Values {
        &self.settings
    }
}

impl kit_server_bugreports::BugReportsHost for AppState {
    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn app_name(&self) -> String {
        AppState::app_name(self)
    }

    fn app_version(&self) -> String {
        VERSION.to_string()
    }

    fn limited(&self, key: &str, limit: usize, window_secs: f64) -> bool {
        self.limits.limited(key, limit, window_secs, true)
    }

    fn reports(&self) -> &dyn kit_server_bugreports::BugReportStore {
        &self.reports
    }

    fn mail_address(&self, user_id: i64) -> kit_server_core::Result<String> {
        use kit_server_core::{Db, RowExt};
        Ok(self.db.one("SELECT email FROM users WHERE id=?", kit_server_core::p![user_id])?.map(|r| r.s("email")).unwrap_or_default())
    }
}

impl kit_server_bugreports::crash::CrashHost for AppState {
    fn crashes(&self) -> &dyn kit_server_bugreports::CrashStore {
        &self.reports
    }
}

impl kit_server_notices::NoticesHost for AppState {
    fn notices(&self) -> kit_server_notices::Notices<AppState> {
        self.notices.clone()
    }
}

impl kit_server_diagnostics::Diagnostics for AppState {
    fn diagnostics(&self) -> kit_server_diagnostics::Registry<AppState> {
        checks::diagnostics()
    }
}

/// The whole app: the kit's routes, the sign-in routes, and the web app's files at `/` (index.html for every
/// path that is not a file, the brotli and gzip copies the image keeps next to each file when the browser
/// takes them).
pub fn app(st: AppState) -> Router {
    let web = kit_server_health::web_build(&st.static_dir, &format!("{NAME}-web-"));
    let index = ServeFile::new(st.static_dir.join("index.html")).precompressed_br().precompressed_gzip();
    let spa = ServeDir::new(&st.static_dir).precompressed_br().precompressed_gzip().fallback(index);
    let spa = tower_http::set_header::SetResponseHeader::if_not_present(spa, header::VARY, HeaderValue::from_static("accept-encoding"));
    kit_server_notices::spawn_rechecks(st.clone());
    Router::new()
        .merge(kit_server_health::router(kit_server_health::Health::new(VERSION, web)))
        .merge(kit_server_changelog::router())
        .merge(kit_server_changelog::prefs_router("/api/auth/prefs"))
        .merge(kit_server_notices::router())
        .merge(kit_server_bugreports::router())
        .merge(kit_server_bugreports::crash::router())
        .merge(kit_server_diagnostics::router())
        .merge(kit_server_settings::router())
        .merge(kit_server_setup::router())
        .merge(auth::router())
        .merge(indexers::api::router())
        .merge(solver::api::router())
        .merge(hub::api::router())
        .merge(vault::api::router())
        .merge(keeper::api::router())
        .merge(migrate::api::router())
        .merge(releases::api::router())
        .merge(crossseed::api::router())
        .merge(prowlarr_ui::router(&st))
        .with_state(st)
        .fallback_service(spa)
        .layer(axum::middleware::from_fn(security_headers))
        .layer(tower_http::compression::CompressionLayer::new())
}

/// Prowlarr's web frontend (the fork in `ui/prowlarr/`, built) at `/prowlarr/`, served as Prowlarr serves it with
/// its URL base set to `/prowlarr`; its API calls go to Lantern's `/api/v1`. Lantern's own web app stays at `/`
/// until the frontend's API is ported (docs/DECISIONS.md).
pub mod prowlarr_ui {
    // Ported from Prowlarr v2.7.0.5723 (src/Prowlarr.Http/Frontend/StaticResourceController.cs,
    // Mappers/IndexHtmlMapper.cs, Mappers/HtmlMapperBase.cs, InitializeJsonController.cs), GPL-3.0.
    use super::{AppState, VERSION};
    use axum::extract::{Request, State};
    use axum::http::header;
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::get;
    use axum::Router;
    use std::sync::LazyLock;
    use tower_http::services::ServeDir;

    /// The URL base the frontend runs under (Prowlarr's `UrlBase` setting).
    pub const URL_BASE: &str = "/prowlarr";

    /// The routes: the page (at `/prowlarr`, `/prowlarr/` and every path that is not a file), `initialize.json`,
    /// and every built file.
    pub fn router(st: &AppState) -> Router<AppState> {
        let files = ServeDir::new(&st.prowlarr_ui_dir).append_index_html_on_directories(false).fallback(get(page).with_state(st.clone()));
        let guard = || axum::middleware::from_fn_with_state(st.clone(), signed_in_only);
        let inside = Router::new()
            .route("/", get(page))
            .route("/index.html", get(page))
            .route("/initialize.json", get(initialize))
            .route("/Content/manifest.json", get(manifest))
            .fallback_service(files)
            .layer(guard());
        Router::new().route("/prowlarr/", get(page).layer(guard())).nest(URL_BASE, inside)
    }

    /// Prowlarr's `[Authorize(Policy = "UI")]`: only `Content/` is open to everyone. A page asked for without a
    /// session goes to Lantern's sign-in at `/`; anything else answers 401. (Inside the nest the path comes
    /// without the URL base.)
    async fn signed_in_only(State(st): State<AppState>, req: Request, next: axum::middleware::Next) -> Response {
        let path = req.uri().path();
        if path.strip_prefix(URL_BASE).unwrap_or(path).to_ascii_lowercase().starts_with("/content/") {
            return next.run(req).await;
        }
        let (parts, body) = req.into_parts();
        if crate::auth::signed_in(&st, &parts).is_ok() {
            return next.run(Request::from_parts(parts, body)).await;
        }
        if is_page(parts.uri.path()) {
            Redirect::to("/").into_response()
        } else {
            kit_server_core::err(401, "Sign in to Lantern first, then open this page again.").into_response()
        }
    }

    /// Whether `path` (with or without the URL base) is one of the frontend's pages rather than a file: no dot
    /// in it, and not under `/content` or `/login` (`IndexHtmlMapper.CanHandle`).
    pub fn is_page(path: &str) -> bool {
        let rest = path.strip_prefix(URL_BASE).unwrap_or(path).to_ascii_lowercase();
        rest.is_empty() || rest == "/index.html" || (!rest.starts_with("/content") && !rest.starts_with("/login") && !rest.contains('.'))
    }

    /// The page (the build's `index.html`) for every path that is not a file, with the URL base put in front of
    /// its links and in place of `__URL_BASE__`. A missing file is a 404.
    async fn page(State(st): State<AppState>, req: Request) -> Response {
        if !is_page(req.uri().path()) {
            return kit_server_core::err(404, "This file is not part of Lantern's Prowlarr pages.").into_response();
        }
        match tokio::fs::read_to_string(st.prowlarr_ui_dir.join("index.html")).await {
            Ok(text) => {
                ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], with_url_base(&text, URL_BASE))
                    .into_response()
            }
            Err(_) => kit_server_core::err(
                404,
                "Prowlarr's pages are not built into this copy of Lantern. Build ui/prowlarr (yarn build) and set \
                 APP_PROWLARR_UI_DIR to its _output/UI folder.",
            )
            .into_response(),
        }
    }

    /// `HtmlMapperBase.GetHtmlText`: every `href` or `src` to a css, js, png, ico, ics, svg or json file gets the
    /// URL base in front, and `__URL_BASE__` becomes the URL base. Prowlarr's `?h=` cache breaker is left out:
    /// these files are answered with `no-cache`, so a browser checks them every time anyway.
    pub fn with_url_base(html: &str, url_base: &str) -> String {
        static LINK: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r#"(?i)(?P<attribute>href|src)="(?P<path>[^"]*?(?:css|js|png|ico|ics|svg|json))""#).expect("a valid pattern")
        });
        let text = LINK.replace_all(html, |c: &regex::Captures| format!("{}=\"{url_base}{}\"", &c["attribute"], &c["path"]));
        text.replace("__URL_BASE__", url_base)
    }

    /// `Content/manifest.json` (`ManifestMapper`): the build's manifest with `__URL_BASE__` and `__INSTANCE_NAME__`
    /// put in.
    async fn manifest(State(st): State<AppState>) -> Response {
        match tokio::fs::read_to_string(st.prowlarr_ui_dir.join("Content/manifest.json")).await {
            Ok(text) => {
                let text = text.replace("__URL_BASE__", URL_BASE).replace("__INSTANCE_NAME__", &st.app_name());
                ([(header::CONTENT_TYPE, "application/manifest+json")], text).into_response()
            }
            Err(_) => kit_server_core::err(404, "This file is not part of Lantern's Prowlarr pages.").into_response(),
        }
    }

    /// `initialize.json` (`InitializeJsonController`): what the frontend reads before it starts. The API root is
    /// Lantern's `/api/v1` (Prowlarr's is `<urlBase>/api/v1`); analytics are always off.
    async fn initialize(State(st): State<AppState>) -> Response {
        let body = serde_json::json!({
            "apiRoot": "/api/v1",
            "apiKey": st.settings.str("api_key"),
            "release": VERSION,
            "version": VERSION,
            "instanceName": st.app_name(),
            "theme": "auto",
            "branch": "main",
            "analytics": false,
            "userHash": "",
            "urlBase": URL_BASE,
            "isProduction": !cfg!(debug_assertions),
        });
        ([(header::CACHE_CONTROL, "no-store")], axum::Json(body)).into_response()
    }
}

/// Keeps the indexer definitions current: at start when there are none, then once a day, while the
/// `definitions_auto_update` setting is on.
pub fn spawn_definition_updates(st: AppState) {
    tokio::spawn(async move {
        let mut first = true;
        loop {
            let due = st.settings.bool("definitions_auto_update")
                && (!first || st.indexers.library.is_empty() || std::env::var("APP_UPDATE_DEFINITIONS_AT_START").is_ok());
            if due {
                let source = st.settings.str("definitions_source");
                let source = if source.is_empty() { indexers::library::DEFAULT_SOURCE.to_string() } else { source };
                match st.indexers.library.update(&source).await {
                    Ok(n) => tracing::info!("{n} indexer definitions updated"),
                    Err(e) => tracing::warn!("{e}"),
                }
            }
            first = false;
            tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
        }
    });
}

/// Moves every grab on every three seconds: sends the queued ones, follows the downloads, calls the apps back.
pub fn spawn_hub(st: AppState) {
    tokio::spawn(async move {
        loop {
            st.hub.tick().await;
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });
}

/// The account keeper's rounds: every 30 minutes it keeps logins alive and sends new warnings.
pub fn spawn_keeper(st: AppState) {
    tokio::spawn(async move {
        loop {
            keeper::tick(&st).await;
            tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
        }
    });
}

/// Middleware: the security headers, the `x-<name>` mark and the cache policy on every answer.
async fn security_headers(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.entry("x-content-type-options").or_insert(HeaderValue::from_static("nosniff"));
    h.entry("x-frame-options").or_insert(HeaderValue::from_static("DENY"));
    h.entry("referrer-policy").or_insert(HeaderValue::from_static("same-origin"));
    // every answer says it is this app's: the web app tells a proxy's error page (a 502 while the server is
    // down) from the server's own answers by it
    if let Ok(name) = header::HeaderName::try_from(format!("x-{NAME}")) {
        h.insert(name, HeaderValue::from_static("1"));
    }
    let cache = if path.starts_with("/api/") {
        "no-store"
    } else if hashed_asset(&path) {
        // trunk names the build's files by their content
        "public, max-age=31536000, immutable"
    } else {
        // index.html and the other files: cheap 304s, updates show at once
        "no-cache"
    };
    h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static(cache));
    resp
}

/// `/<name>-web-f92ab5b84b305a59_bg.wasm`: a file at the root whose name ends in a hash of its content (12 hex
/// digits or more; trunk drops leading zeros), so it never changes under that name.
pub fn hashed_asset(path: &str) -> bool {
    let Some(name) = path.strip_prefix('/') else { return false };
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    !name.contains('/') && stem.rsplit_once('-').is_some_and(|(_, h)| h.len() >= 12 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Whether the server on `port` of this machine answers `/api/health` with `ok` within five seconds: the
/// Docker image's health check (`<app> healthcheck`), which has no curl. Plain HTTP/1.0 over a TCP socket.
pub async fn healthcheck(port: u16) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let ask = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        s.write_all(b"GET /api/health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").await.ok()?;
        let mut answer = Vec::new();
        s.read_to_end(&mut answer).await.ok()?;
        let answer = String::from_utf8_lossy(&answer);
        Some(answer.starts_with("HTTP/1.0 200") || answer.starts_with("HTTP/1.1 200"))
    };
    matches!(tokio::time::timeout(std::time::Duration::from_secs(5), ask).await, Ok(Some(true)))
}

/// Resolves on Ctrl-C or SIGTERM (`docker stop`), so the server finishes the requests it has and exits.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
