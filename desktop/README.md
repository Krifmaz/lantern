# Lantern desktop

Lantern's desktop app for Linux, Windows and macOS: [Iced](https://iced.rs) 0.14 with the kit's components
(`kit-iced`) and rules (`kit-rules`), talking to the server's API with an app token for this computer. A Cargo
workspace of its own, like `server/` and `web/`.

```sh
cargo run                     # the app
cargo test                    # the rules, and every page drawn headless (iced_test)
cargo run --example walk -- http://127.0.0.1:8080 admin <password>   # every request, against a running server
```

## What it does

| Part | Where | From the kit |
| --- | --- | --- |
| Connect: the server's address (https first, then http), remembered in the config folder; `<PROJECT>_SERVER` overrides it | `address.rs`, `store.rs` | the phone app's rules |
| Sign in: an app token (`POST /api/auth/login` with `app`), sent as `Authorization: Bearer`; a 401 asks again | `signin.rs`, `client.rs` | the phone app's rules |
| The shell: sidebar, Home, Bug reports, Settings, Diagnostics, What's new | `ui.rs`, `route.rs` | `widgets` (the app's `Logo`: `LOGO` in lib.rs), `theme` |
| Toasts: errors stay with Copy, repeats don't pile up, Undo, at most 5 | `app.rs` | `toast` |
| The details dialog: any text in full, selectable, with Copy | `app.rs`, `ui.rs` | `details` |
| A question before signing out or changing the server | `ui.rs` `confirm` | `confirm` |
| The offline banner with Details and Retry (and a retry every 30 s) | `ui.rs` | `offline`, kit-rules' offline reasons |
| The data folder notice with Check again (admins) | `ui.rs` | `notice` |
| What's new: the popup once per release, and the full page | `app.rs` | `whats_new` |
| Bug reports: the list, one report's thread with the reply box, New report with the file dialog and drag and drop | `app.rs`, `ui.rs` | `bug_report`, `drop_zone` |
| Settings: the server's form from its schema (admins), the look, and Updates | `ui.rs` | `form`, `settings`, `update`, `theme` |
| Diagnostics: the server's checks with Copy report | `ui.rs` | `diagnostics` |
| The update notice: this app's GitHub releases (Cargo.toml's `repository`), the file for this computer, checked against SHA256SUMS | `release.rs` | `update` |

Every server path is in `paths.rs`. The names come from Cargo.toml (`CARGO_PKG_NAME`, `CARGO_PKG_REPOSITORY`)
and `app.json` (the title, and the release files' prefix), so the Rust files hold no app name.

## Releases

`scripts/desktop-package.sh` makes one file per system (`<App>-<version>-linux-x86_64.tar.gz`,
`-windows-x86_64.zip`, `-macos-universal.dmg`); the release workflow runs it on each system through the kit's
`kit-desktop.yml` when Actions → release → Run workflow has "apps" ticked, and attaches the files to the release.
The update notice offers a newer release when it carries this computer's file; Install downloads it, checks it
against the release's SHA256SUMS and keeps it in the Downloads folder to open.

On Linux the app loads the system's `libxkbcommon` (and `libxkbcommon-x11` on X11) at run time; building needs no
extra package.
