# Changelog

## Unreleased

### Added
- Indexers: every site in the Prowlarr/Indexers definitions (bundled in the image and updated daily), plus
  Newznab and Torznab indexers, with login, tests, health and hold after repeated failures.
- Torznab/Newznab feeds per indexer and for all of them (`/{id}/api`, `/all/api`), Jackett and Prowlarr paths too,
  and signed download links, so Sonarr, Radarr, Lidarr and Readarr can add Lantern as an indexer.
- Prowlarr's v1 API (system status, indexers, stats, search) for apps that talk to Prowlarr.
- Indexers and Search pages, a notice and a Diagnostics check for indexers that stop working.
- Contract test in CI: real Sonarr, Radarr, Lidarr and Readarr add Lantern and search through it.
- Lantern listens on port 9696 (Prowlarr's), and makes an API key at first start.
- The Android app (`android/`, kit-compose v0.2.1, the kit's default theme): connect, sign in, What's new, updates,
  bug reports, crash logs, Server settings and Diagnostics; built and unit-tested on every pull request.
- Account vault (Vault page): tracker and Usenet logins, passwords, cookies, passkeys, API keys, 2FA seeds and invite
  notes, encrypted at rest (argon2id + XChaCha20-Poly1305). Secrets are never listed or logged; the owner sees them
  only after typing their password again, with the 2FA code. Encrypted backups, and restore with a check first.
- Android: Indexers (how every site is doing, what went wrong, the last week's numbers, Test) and Search (every
  indexer at once, by kind; open a result's page or copy its link), and the server's notices on the home page.
- Download hub: qBittorrent and SABnzbd behind one download-client layer; apps register with their name, media type
  and import path and get their own category and folder; search and grab through Lantern; progress followed, failed
  sends tried again (5 times by default), and apps called back by webhook and a live event stream with the finished
  files' exact paths in their own view.
- Download hub: Transmission, Deluge, rTorrent and NZBGet join qBittorrent and SABnzbd behind the same client
  layer (qBittorrent and SABnzbd stay the defaults).
- Android: the Vault page. Every saved account with its plain values and which secrets are saved; the owner sees
  the secrets and the 2FA code after typing their password again, for two minutes, with screenshots blocked. Every
  value can be copied, secrets marked sensitive on the clipboard.
- Import from Prowlarr (a backup, its database or its API) and FlareSolverr (its settings pasted in), on a new
  Import page: a dry run first that says line by line what would be added, skipped or left to you, then Import.
  Indexers keep their logins (also saved in the vault) and Prowlarr's numbers, Lantern can take Prowlarr's API key and proxy, and the apps
  Prowlarr synced to are kept for app sync.

## 0.0.1

### Added
- Lantern made from the Krifmaz kit template (v0.2.1).
- docs/GOALS.md (the goal and build order), docs/DECISIONS.md, GPL-3.0 license and credits in README.md.
- Releases are 0.0.x until v0.1.0 (indexers, Cloudflare solving and the vault).
