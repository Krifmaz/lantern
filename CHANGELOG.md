# Changelog

## Unreleased

### Added
- Cloudflare solving: a headless Chromium (in the image) gets past Cloudflare and DDoS-Guard checks by itself when an
  indexer meets one; the site's cookies are kept and reused, so most requests need no browser.
- The FlareSolverr API on port 8191 (`APP_SOLVER_PORT`), so apps that use FlareSolverr can use Lantern instead.
- A Cloudflare checks card on the Indexers page (test any address) and a Diagnostics check for the browser.
- CI checks the image as it ships: its browser gets past a Cloudflare-style check through the FlareSolverr API.
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
- Account keeper (Accounts page): each vault account's site rules (minimum ratio, hit-and-runs, sign-in and activity
  deadlines, login and cookie end dates) and numbers (ratio, upload, download, seed time, bonus points), with what is
  at risk in words. Linked accounts are signed in to on their own before a deadline. Warnings show as a banner and go
  once to ntfy, Discord or email (Settings → Accounts → Alerts), with Test alerts.
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
- Android: Downloads (what Lantern downloads for the apps, with progress, speed, time left and what went wrong;
  Try again and Remove for admins) and Apps (every connected app with its folder and downloads by state).
- Android: Account health. Every tracker and Usenet account, the ones at risk first, with ratio, upload,
  download, seed time, bonus and hit-and-runs and each risk in words; Sign in now and I signed in today. Home shows
  how many accounts are at risk.
- Import from Prowlarr (a backup, its database or its API) and FlareSolverr (its settings pasted in), on a new
  Import page: a dry run first that says line by line what would be added, skipped or left to you, then Import.
  Indexers keep their logins (also saved in the vault) and Prowlarr's numbers, Lantern can take Prowlarr's API key and proxy, and the apps
  Prowlarr synced to are kept for app sync.
- Releases as they appear: trackers' IRC announce channels (autobrr's announce definitions, bundled), RSS feeds and
  Lantern's own indexers are watched; filters (names, resolution, source, codec, HDR, group, year, season, size,
  freeleech, category, tags, uploader, grab limits) send a new release to Sonarr, Radarr, Lidarr, Readarr or Whisparr,
  or to a download client through the download hub, within seconds. A Releases page shows what came and why each was sent or skipped, and checks any release name.
- Download hub: seeding rules (seed to a ratio or for some hours) and cleanup once an app says it imported a
  download: keep it, remove it from the client, or remove it with its files (off by default).
- Cross-seed: finished torrents are searched on the other torrent indexers; a copy with exactly the same files is
  added to the same download client next to the files already there (category `<app>.cross-seed`, so apps don't
  import it twice). A Cross-seed page shows what happened to every copy found; Settings → Downloads turns it off.
  Cleanup never deletes files a cross-seed copy seeds from.
- Apps and Download clients pages: every connected app with how it connects, its category, folders and
  downloads (live, with Try again and Stop), and the download clients with Add, Edit, Test, on/off and Remove.
- Download hub: Soulseek as a music source through slskd: music searches also search Soulseek, and a grab
  downloads the user's folder and puts it into the app's folder (Settings → Downloads → Soulseek).
- Contract test: real Sonarr, Radarr, Lidarr and Readarr also add Lantern as their qBittorrent, SABnzbd and NZBGet.
- Lantern can be added as a qBittorrent download client by any app (user name: the app's name, password: Lantern's
  API key). The app is registered on sight, its torrents go to the real client through the hub with Lantern's
  categories and folders, and it sees them by their info hash with paths in its own view.
- Lantern can also be added as a SABnzbd (`/api`, `/sabnzbd/api`) or NZBGet (`/jsonrpc`) download client; NZBs go to
  the real Usenet client through the hub and the queue and history answer in each app's own terms. A finished
  download an app removes is also taken out of the real client's history.

## 0.0.1

### Added
- Lantern made from the Krifmaz kit template (v0.2.1).
- docs/GOALS.md (the goal and build order), docs/DECISIONS.md, GPL-3.0 license and credits in README.md.
- Releases are 0.0.x until v0.1.0 (indexers, Cloudflare solving and the vault).
