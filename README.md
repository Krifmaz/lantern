# Lantern

One self-hosted Rust app that replaces Prowlarr, FlareSolverr, autobrr and cross-seed, keeps every tracker and
Usenet account safe, and is the single download hub for the other apps. The goal and build order are in
[docs/GOALS.md](docs/GOALS.md).

Made from the [Krifmaz kit](https://github.com/Krifmaz/kit) template (v0.8.0): the kit's look and its
ease-of-use features from the first commit.

| Folder | What it is |
| --- | --- |
| `server/` | Axum server with the kit server crates: health, changelog (What's new), notices, bug reports, diagnostics, settings |
| `web/` | Leptos web app with the kit components and the kit's default theme, served at `/` by the server |
| `desktop/` | Desktop app (Linux, Windows, macOS) on Iced with kit-iced: connect, app-token sign-in, every kit feature, the update notice on the app's own releases |
| `android/` | Android app (Kotlin, Jetpack Compose) on kit-compose: indexers, their health and search on the phone; see [android/README.md](android/README.md) |
| `docs/releases/` | one Markdown file per release: the release notes and What's new |

## Add Lantern to Sonarr, Radarr, Lidarr or Readarr

1. In Lantern, open **Indexers** and add your sites. **Settings → General** shows the API key.
2. In the app, add an indexer of type **Torznab** (torrent sites) or **Newznab** (Usenet):
   - URL: `http://<lantern>:9696/all/` for every indexer at once, or `http://<lantern>:9696/<id>/` for one
     (the id is on the Indexers page).
   - API path: `/api`. API key: Lantern's.
3. Apps made for Prowlarr also find Lantern's Prowlarr API at `http://<lantern>:9696/api/v1`.

## Cloudflare checks and FlareSolverr

Sites behind a Cloudflare (or DDoS-Guard) check need nothing extra: Lantern's own browser gets past the check
when it meets one and keeps the site's cookies. Apps that use FlareSolverr can point at Lantern instead:
`http://<lantern>:8191/` (publish the port with `-p 8191:8191`; `APP_SOLVER_PORT` moves it, `0` turns it off).
Outside the image, set `APP_CHROMIUM` to a Chromium or Chrome program.

## Move from Prowlarr and FlareSolverr

1. In Prowlarr, open **System → Backup**, press **Backup Now** and download the `.zip` (it keeps the logins; Prowlarr's
   API hides passwords, so its address and API key work too but you fill those in again).
2. In Lantern, open **Import**, choose the backup and press **Look at it**. The plan says what would be added, what is
   already there, what is skipped and why, and what you do by hand. Nothing changes yet.
3. Press **Import**. Indexers keep Prowlarr's numbers and Lantern takes Prowlarr's API key, so once Lantern answers at
   Prowlarr's address, apps keep searching without changes.
   No indexer is left behind. The plan says which way each one came over: Lantern's own definition, a custom
   definition from the backup (saved as yours), Newznab or Torznab underneath (Headphones VIP and similar), or an
   address that answered as Newznab or Torznab. One that none of these fit is added turned off and listed under
   **Needs your help**, where **Create definition** opens an editor started from its address and settings.
4. For FlareSolverr, paste its docker-compose service, `.env` or `docker run` command under **From FlareSolverr** to see
   what to change; Lantern solves Cloudflare checks by itself.

## Grab releases as they appear

1. Open **Releases → Sources** and add a tracker's IRC announces (Lantern knows the channels of every tracker
   autobrr knows), an RSS feed, or one of your indexers.
2. Under **Filters**, say what to grab (names, resolution, source, group, size, freeleech, year, tags …) and send it
   to Sonarr, Radarr, Lidarr, Readarr or Whisparr. **Check a release** shows which filter would take a name and why.
3. **Recent** lists every release seen and what happened to it. A release goes out seconds after it is announced.

## Cross-seed

When a torrent grab finishes, Lantern looks for the same release on your other torrent sites. A copy with exactly
the same files is added to the same download client next to the files already there, so it seeds on more sites
without downloading again. The Cross-seed page lists every copy found and what happened to it.

## First steps

1. Make the lock files and commit them (CI and the Docker build use `--locked`):
   `(cd server && cargo generate-lockfile) && (cd web && cargo generate-lockfile)`.
2. Push to GitHub. Every pull request runs the checks; every merge to main is a release.
3. The first start asks for the owner: open Lantern (web, desktop or Android) and fill in "Create your account"
   with the one-time setup code it prints in its log (`docker logs lantern`). The code stops working once your
   account exists, and a restart before then prints a new one. For an unattended install, set
   `APP_ADMIN_PASSWORD` (and `APP_ADMIN_USER`, default `admin`) instead: the owner is made at once, with no setup
   screen.

## Run it

```
cd web && trunk build && cd ..
cd server && APP_DATA_DIR=../data APP_STATIC_DIR=../web/dist cargo run
```

Then open <http://localhost:9696>. With Docker:

```
docker build -t lantern .
docker run --read-only -p 9696:9696 -p 8191:8191 -v lantern-data:/data lantern
```

| Variable | Meaning | Default |
| --- | --- | --- |
| `APP_PORT` | the port the server listens on | `9696` (Prowlarr's) |
| `APP_DATA_DIR` | where the database and uploads live (the only folder it writes to) | `/data` |
| `APP_STATIC_DIR` | the built web app | `/app/static` |
| `APP_ANNOUNCE_DIR` | the IRC announce definitions (the data folder's `announce/` wins) | `/app/announce` |
| `APP_ADMIN_USER`, `APP_ADMIN_PASSWORD` | the owner made at the first start (an unattended install) | `admin`; none: "Create your account" with the setup code from the log |
| `APP_VAULT_PASSPHRASE` | locks the account vault with your passphrase instead of `vault.key` in the data folder (set it before the first start; Lantern won't open the vault without it) | not set: `vault.key` |
| `APP_ALERT_NTFY_URL`, `APP_ALERT_DISCORD_WEBHOOK`, `APP_ALERT_EMAIL_TO`, `APP_SMTP_*` | where the account keeper sends warnings (also under Settings → Accounts → Alerts) | not set |
| `APP_NAME`, `APP_SESSION_DAYS`, `APP_API_KEY` | settings (settings.toml); set here they are locked in Settings | |
| `APP_DOWNLOADS_DIR` | the downloads folder; each app gets `<folder>/<category>` in it. Mount it (`-v /data/downloads:/downloads`) at the same place in your download clients and apps, or set each one's own path in Lantern | `/downloads` |
| `APP_GRAB_ATTEMPTS` | how many times a grab is sent before it fails | `5` |

### Download hub

Download clients: qBittorrent (default for torrents), Transmission, Deluge, rTorrent, SABnzbd (default for Usenet)
and NZBGet. Apps never see which one a grab went to.

Apps that only know download clients can add Lantern as one, at Lantern's address and port:
**qBittorrent** (user name: the app's name, password: Lantern's API key), **SABnzbd** (API key: Lantern's; URL base
empty or `/sabnzbd`) or **NZBGet** (user name: the app's name, password: Lantern's API key). Set each app's own
view of its folder on the Apps page (`import_path`) when its container mounts the downloads elsewhere.

Apps need only Lantern's address and API key (`X-Api-Key`). They register once (`POST /api/hub/apps` with `name`,
`media`, `import_path`, `callback_url`), search (`GET /api/hub/search?app=`), grab (`POST /api/hub/grabs` with a
result's `token` or any `url`), and hear back through their callback address or the live stream
(`GET /api/hub/events?app=&apikey=`) with each finished file's path as the app sees it. Webhooks carry
`X-Lantern-Signature: sha256=<HMAC-SHA256 of the body with the API key>`. All routes are listed in
`server/src/hub/api.rs`.

When an app has its files it can say so (`POST /api/hub/grabs/{id}/imported`). Under Settings → Downloads, a download
that seeded enough (`APP_SEED_RATIO`, `APP_SEED_HOURS`) is then kept, removed from its client, or removed with its
files (`APP_CLEANUP`: `off`, `remove`, `delete`; `off` by default).

**Soulseek** (music): run [slskd](https://github.com/slskd/slskd) and give Lantern its address and an API key
(`APP_SOULSEEK_URL`, `APP_SOULSEEK_API_KEY`), plus where Lantern sees slskd's downloads folder
(`APP_SOULSEEK_DOWNLOADS`, default `<downloads>/soulseek`). Music searches then also search Soulseek (one result
per shared folder), and a finished grab is hardlinked into the app's folder.

## Checks

In `server/`, `web/` and `desktop/`: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
Every merge to main is released as `vX.Y.Z` with the Docker image `ghcr.io/krifmaz/lantern:<version>` and `:latest`.
Desktop installers (Linux tar.gz, Windows zip, macOS dmg) build with Actions → release → "apps". Signing is optional,
from the secrets `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD`, `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`,
`MACOS_SIGN_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD`; without them the zip is unsigned and the
dmg is signed ad hoc.

## Credits

Lantern is a Rust port of Prowlarr, autobrr, cross-seed and FlareSolverr, at the versions in
[`UPSTREAM.lock`](UPSTREAM.lock), with their web frontends forked; [docs/PARITY.md](docs/PARITY.md) shows how far
each port is. All credit for their design and behaviour goes to their authors and contributors:

| Project | License | Used for |
| --- | --- | --- |
| [Prowlarr](https://github.com/Prowlarr/Prowlarr) | GPL-3.0 | ported: indexer manager, API, app sync and web frontend |
| [Jackett](https://github.com/Jackett/Jackett) | GPL-2.0 | Torznab behaviour and Cardigann definitions |
| [FlareSolverr](https://github.com/FlareSolverr/FlareSolverr) | MIT | ported: the challenge-solving API |
| [autobrr](https://github.com/autobrr/autobrr) | GPL-2.0-or-later | ported: IRC announces, feeds, filters, actions and web frontend |
| [cross-seed](https://github.com/cross-seed/cross-seed) | Apache-2.0 | ported: cross-seed matching, injection and API |
| [Sonarr](https://github.com/Sonarr/Sonarr), [Radarr](https://github.com/Radarr/Radarr), [Lidarr](https://github.com/Lidarr/Lidarr), [Readarr](https://github.com/Readarr/Readarr) | GPL-3.0 | the indexer and download client APIs they expect |
| [qBittorrent](https://github.com/qbittorrent/qBittorrent), [SABnzbd](https://github.com/sabnzbd/sabnzbd), [NZBGet](https://github.com/nzbgetcom/nzbget) | GPL-2.0+ | the download client APIs Lantern speaks and emulates |

Data files used directly:

| Data | Source | License |
| --- | --- | --- |
| Indexer definitions (Cardigann YAML) | [Prowlarr/Indexers](https://github.com/Prowlarr/Indexers) | GPL-3.0 |
| IRC announce definitions (YAML) | [autobrr](https://github.com/autobrr/autobrr/tree/develop/internal/indexer/definitions) | GPL-2.0-or-later |
| Quality and release profiles | [TRaSH Guides](https://github.com/TRaSH-Guides/Guides) | MIT |

## License

[GPL-3.0](LICENSE). Built on the [Krifmaz kit](https://github.com/Krifmaz/kit) (MIT).
