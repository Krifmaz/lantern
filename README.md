# Lantern

One self-hosted Rust app that replaces Prowlarr, FlareSolverr, autobrr and cross-seed, keeps every tracker and
Usenet account safe, and is the single download hub for the other apps. The goal and build order are in
[docs/GOALS.md](docs/GOALS.md).

Made from the [Krifmaz kit](https://github.com/Krifmaz/kit) template (v0.2.1): the kit's look and its
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

## First steps

1. Make the lock files and commit them (CI and the Docker build use `--locked`):
   `(cd server && cargo generate-lockfile) && (cd web && cargo generate-lockfile)`.
2. Push to GitHub. Every pull request runs the checks; every merge to main is a release.
3. The first start makes the owner account: set `APP_ADMIN_PASSWORD` (and `APP_ADMIN_USER`, default `admin`),
   or read the password it prints (also in `initial_admin_password.txt` in the data folder).

## Run it

```
cd web && trunk build && cd ..
cd server && APP_DATA_DIR=../data APP_STATIC_DIR=../web/dist cargo run
```

Then open <http://localhost:9696>. With Docker:

```
docker build -t lantern .
docker run --read-only -p 9696:9696 -v lantern-data:/data lantern
```

| Variable | Meaning | Default |
| --- | --- | --- |
| `APP_PORT` | the port the server listens on | `9696` (Prowlarr's) |
| `APP_DATA_DIR` | where the database and uploads live (the only folder it writes to) | `/data` |
| `APP_STATIC_DIR` | the built web app | `/app/static` |
| `APP_ADMIN_USER`, `APP_ADMIN_PASSWORD` | the owner made at the first start | `admin`, a printed password |
| `APP_NAME`, `APP_SESSION_DAYS`, `APP_API_KEY` | settings (settings.toml); set here they are locked in Settings | |

## Checks

In `server/`, `web/` and `desktop/`: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
Every merge to main is released as `vX.Y.Z` with the Docker image `ghcr.io/krifmaz/lantern:<version>` and `:latest`.
Desktop installers (Linux tar.gz, Windows zip, macOS dmg) build with Actions → release → "apps". Signing is optional,
from the secrets `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD`, `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`,
`MACOS_SIGN_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD`; without them the zip is unsigned and the
dmg is signed ad hoc.

## Credits

Lantern is written fresh in Rust. These projects were used as reference only (their docs, source and real API
answers), so that apps made for them work with Lantern unchanged:

| Project | License | Used for |
| --- | --- | --- |
| [Prowlarr](https://github.com/Prowlarr/Prowlarr) | GPL-3.0 | the indexer manager API and app sync |
| [Jackett](https://github.com/Jackett/Jackett) | GPL-2.0 | Torznab behaviour and Cardigann definitions |
| [FlareSolverr](https://github.com/FlareSolverr/FlareSolverr) | MIT | the challenge-solving API |
| [autobrr](https://github.com/autobrr/autobrr) | GPL-2.0 | IRC announce parsing and filters |
| [cross-seed](https://github.com/cross-seed/cross-seed) | Apache-2.0 | cross-seed matching |
| [Sonarr](https://github.com/Sonarr/Sonarr), [Radarr](https://github.com/Radarr/Radarr), [Lidarr](https://github.com/Lidarr/Lidarr), [Readarr](https://github.com/Readarr/Readarr) | GPL-3.0 | the indexer and download client APIs they expect |
| [qBittorrent](https://github.com/qbittorrent/qBittorrent), [SABnzbd](https://github.com/sabnzbd/sabnzbd), [NZBGet](https://github.com/nzbgetcom/nzbget) | GPL-2.0+ | the download client APIs Lantern speaks and emulates |

Data files used directly:

| Data | Source | License |
| --- | --- | --- |
| Indexer definitions (Cardigann YAML) | [Prowlarr/Indexers](https://github.com/Prowlarr/Indexers) | GPL-3.0 |
| Quality and release profiles | [TRaSH Guides](https://github.com/TRaSH-Guides/Guides) | MIT |

## License

[GPL-3.0](LICENSE). Built on the [Krifmaz kit](https://github.com/Krifmaz/kit) (MIT).
