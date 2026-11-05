# app

Lantern's Android app (`dev.krifmaz.lantern`; debug builds install beside it as `dev.krifmaz.lantern.debug`).
One activity in the kit's default theme; every page is a kit-compose part, wired to this repository's server.

| File | What it is |
| --- | --- |
| `App.kt` | The process: crash log, `KitApp.init`, where updates come from, the server and its calls |
| `MainActivity.kt` | The theme, the pages and the toasts; the daily update check |
| `AppShell.kt` | The pages as a stack (Back goes down), and the bug report screens' host |
| `ConnectPage.kt` | First start: the server's address |
| `SignInPage.kt` | Username and password (and the authenticator code), the kit's phone sign-in |
| `HomePage.kt` | Offline banner, What's new, the server's notices, how the indexers are doing, the entries, bug reports, crash log, updates |
| `IndexersPage.kt` | Every indexer: working, failing, on hold or off, what went wrong, the last 7 days' numbers, Test (admins) |
| `SearchPage.kt` | One search through every indexer, by kind; open a result's page, copy its download or magnet link |
| `VaultPage.kt` | The vault's accounts; the owner shows secrets and the 2FA code with their password, for two minutes |
| `DownloadsPage.kt` | Downloads (refreshed every few seconds while something moves; Try again, Remove) and Apps |
| `AccountsPage.kt` | Account health: every kept account's numbers and risks, Sign in now, I signed in today |
| `Phone.kt` | Opening links in the browser and copying to the clipboard (secrets marked sensitive) |
| `IndexerRules.kt`, `SearchRules.kt`, `VaultRules.kt`, `DownloadRules.kt`, `KeeperRules.kt` | Reading `/api/indexers`, `/api/indexers/stats`, `/api/search`, `/api/vault…`, `/api/hub/grabs`, `/api/hub/apps`, `/api/keeper` and the words shown (unit-tested in `src/test`) |
| `AdminPages.kt` | Server settings (forms from the server's schema) and Diagnostics (with the app's crash reports) |
| `Server.kt` | The address and token, the server's API (`KitApi`), What's new calls, the network check |
| `ServerAddress.kt` | What is tried for a typed address (unit-tested in `src/test`) |
| `SignInRules.kt` | The sign-in's body, its answer and words (unit-tested in `src/test`) |

| kit-compose part | Where | Server |
| --- | --- | --- |
| Theme, toasts | `MainActivity`, every page | — |
| Notice banners | `HomePage` (`NoticeBanners`) | `/api/notices…` |
| What's new popup and changelog | `HomePage` (`WhatsNewOnStart`), `AppShell` (`ChangelogScreen`) | `/api/changelog…`, `PUT /api/auth/prefs` |
| In-app updates | `App` (`UpdateRules.source`), `MainActivity`, `HomePage` (`UpdatesSection`) | GitHub releases of Krifmaz/lantern |
| Bug reports and crash logs | `App` (`CrashLog`), `AppShell`, `HomePage` | `/api/bug-reports…`, `/api/crash-reports` |
| Offline banner | `HomePage` (`probeHealth`, `OfflineBanner`) | `/api/health` |
| Settings forms | `AdminPages` (`SchemaPage`) | `/api/settings…`, `/api/diagnostics/…/test` |
| Diagnostics | `AdminPages` (`DiagnosticsScreen`, `AppCrashesSection`) | `/api/diagnostics` |

Signing in is the kit's phone sign-in: `POST /api/auth/login` with `{username, password, app: {device_id,
device_name}}` answers `{user, token}`; the token goes with every request as `Authorization: Bearer <token>` (only
to the server's own address), with the User-Agent `<app name>-Android/<version>`. A 401 from any later call goes
back to the sign-in. Admin pages show unless `GET /api/auth/me` says the account isn't one.
Lantern's own pages call `GET /api/indexers`, `GET /api/indexers/stats?days=7`, `POST /api/indexers/{id}/test` and
`GET /api/search?q=&type=`, `GET /api/vault`, `GET /api/vault/items` and `POST /api/vault/items/{id}/reveal`,
with the same sign-in.
