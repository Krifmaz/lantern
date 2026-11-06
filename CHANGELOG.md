# Changelog

## Unreleased

### Added
- Prowlarr's scheduled tasks now run on their own: every 30 seconds Lantern starts each task whose time is up
  (health checks, app sync every 6 hours, backups, clean-ups, housekeeping and the daily definition update), and
  System → Tasks shows when each ran. The health page updates by itself after each check. When Lantern starts
  on a new version, the notifications on for application updates are told.
- Backups in Prowlarr's way: System → Backup makes, lists, downloads, deletes and restores backups (a zip of
  Lantern's database). Scheduled backups older than the backup retention are deleted. A restore only happens when
  you ask for one, and takes effect the next time Lantern starts; the database it replaces is kept beside it.
- System → Logs now shows what Lantern logs, page by page and by level, and the log files (`lantern.txt`, and the
  debug and trace files when the log level asks for them). Passwords, API keys and other secrets are taken out
  first, as Prowlarr does.
- Prowlarr's download clients and notifications. Settings → Download Clients now has all of Prowlarr's
  download clients, with their settings, checks and tests. The download hub uses the same list. A grab from
  Lantern's search goes to them the way Prowlarr picks: the indexer's own client first, then the best priority,
  taking turns. Settings → Connect sends grabs, health issues and fixed health issues to Apprise, Discord, email,
  Gotify, Join, Mailgun, Notifiarr, ntfy, Prowl, Pushbullet, Pushcut, Pushover, SendGrid, Signal, Simplepush,
  Slack, Telegram, a webhook or a script of your own. A few are not finished yet, and their test says so:
  NZBVortex, Download Station, Freebox and Twitter.
- Cross-seed's command line: `lantern cross-seed <command>` runs cross-seed v6.13.9's commands on the data folder,
  with its options and output: `daemon`, `rss`, `search`, `inject`, `restore`, `test-notification`, `gen-config`,
  `update-torrent-cache-trackers`, `diff`, `tree`, `clear-indexer-failures`, `clear-cache`, `clear-client-cache`,
  `api-key` and `reset-api-key`. Options change the settings for that run only, and are checked as cross-seed
  checks them before anything runs.

### Changed
- Cross-seed now searches and runs its jobs as cross-seed v6.13.9 does. Besides your client torrents, it can search
  the `.torrent` files of a folder (Torrent folder) and the files in your data folders (Data folders, looked into
  as deep as you set). It makes whole seasons out of single episodes. It asks Sonarr and Radarr for a release's
  ids, so sites can be searched by id. It sends cross-seed's messages to your notification webhooks. Its jobs run
  on their own schedule: new releases every 30 minutes, a full search daily, saved matches added hourly, and a
  daily tidy-up. Announces and the webhook compare with all of these, and `POST /api/job` runs any job now.
- Cross-seed now adds its matches as cross-seed v6.13.9 does. It talks to qBittorrent, rTorrent, Transmission and
  Deluge itself, several at once (your torrent download clients, or the ones you list in Torrent clients). It can
  link a match's files into your link folders (hard links, symbolic links, reflinks or copies, under the site's name
  or straight in), or just save the `.torrent` (Action: save). It also follows cross-seed's recheck and auto-resume
  rules. A copy goes in the same category as the torrent it matches, tagged `cross-seed`. With linking, it goes under
  `cross-seed-link`. With "Own category for matches" turned on, it goes under `<category>.cross-seed` instead.

### Fixed
- Restarting an IRC network always says goodbye (QUIT) to the server before it reconnects. Before, the goodbye
  was sometimes lost.
- Cloudflare solving waits up to two minutes for its browser to start, and tries once more if it doesn't, so a busy
  or slow machine no longer fails a check before the browser is ready.

### Added
- autobrr's feeds and lists (served at `/autobrr/api`). Torznab, Newznab and RSS indexers added in autobrr are now
  checked on their own schedule: new items go through your filters, and items already seen are skipped. Feeds can
  be tested, run now, have their seen items cleared, and show what their last run found. Lists from Sonarr, Radarr,
  Lidarr, Readarr, Whisparr, Sportarr, MDBList, Trakt, Metacritic, Steam, AniList or a plain text file fill in the
  filters they are linked to every six hours, or right away through the list webhooks. Feeds whose indexer uses a
  proxy stop with an error until proxies are ported.
- Prowlarr's indexer pages work against Lantern as against Prowlarr v2.7.0.5723: add, change, test and remove
  indexers (one at a time or several), every Cardigann definition and Prowlarr's Newznab and Torznab presets to
  pick from, the search page and its grab button, the history (all, since a date, by indexer), the stats and the
  sites on hold. Tag details name the proxies and apps that use a tag, the health page has Prowlarr's indexer,
  proxy, VIP and download client checks, and the web app gets live updates over SignalR.
- Indexer proxies work as in Prowlarr v2.7.0.5723: HTTP, SOCKS4, SOCKS5 and FlareSolverr proxies under
  `/api/v1/indexerproxy` (add, change, remove, schema, test, test all), used by the indexers that share a tag with
  them. A FlareSolverr proxy is asked only when a site shows a Cloudflare check; pointed at Lantern, Lantern's own
  solver answers. Indexers keep their tags, app profile and download client in Prowlarr's shape, wait between
  requests to one site as Prowlarr does, and generic Newznab/Torznab indexers read the site's own capabilities.
- Apps that search through Lantern's Newznab/Torznab feeds now get exactly what Prowlarr v2.7.0.5723 answers: every
  search parameter, the same feed and capabilities, the same error messages, and Prowlarr's test indexer (number 0).
  Indexers have Prowlarr's query and grab limits per hour or day, its "Prefer magnet links" and "Redirect" options,
  and are put on hold after failures with Prowlarr's growing wait times. Downloads are named, redirected and
  recorded in history as in Prowlarr.
- autobrr's web frontend is part of the image and opens at `/autobrr/` after signing in to Lantern, in the kit's
  Encore look (dark mode) or light theme (light mode), with Lantern's name in the header and no links to autobrr's
  docs, GitHub or Discord. Its pages show errors where Lantern doesn't answer autobrr's API yet.
  `scripts/upstream-merge.sh --project autobrr` keeps it up to date (`ui/autobrr/`, see `UPSTREAM.md`), and
  `docs/parity/a-ui/` shows the Dashboard, Filters and Settings → IRC at desktop and phone width.
- autobrr's IRC networks and indexers (served at `/autobrr/api`): networks sign in with NickServ or SASL, through a
  bouncer or straight, with or without TLS; channels are joined after the tracker's invite command, and announces of
  one line or several are read with every autobrr definition and go through the filters at once. Each channel keeps
  its last 1000 lines, the live event stream shows what is said and how each channel is doing, and lines can be
  said or announces read by hand. Indexers keep their passkeys and keys hidden, and indexers whose tracker closed
  are archived, listed and can be removed once no filter uses them. A ban, wrong sign-in or a connection that keeps
  dropping stops the network with the reason shown. Lantern's own IRC sources get their network automatically.
- Prowlarr's applications: Sonarr, Radarr, Lidarr, Readarr, Whisparr, Mylar and LazyLibrarian can be added under
  `/api/v1/applications` (with Prowlarr's schema, test, test all and bulk edit), and Lantern keeps their indexer lists
  in step as Prowlarr does: Add and Remove Only or Full Sync, sync categories, tags, app profiles
  (`/api/v1/appprofile`), seeding settings, every 6 hours and when an indexer is added, changed or removed. The
  `ApplicationIndexerSync` command runs a sync now, and the health page says when apps keep failing. Apps imported
  from Prowlarr are taken over.
- autobrr's actions and its filter, action and download client API (served at `/autobrr/api`): every autobrr action
  type (qBittorrent, Deluge 1 and 2, rTorrent, Transmission, Porla, Aria2, the *arr apps, SABnzbd, NZBGet, watch
  folder, webhook, exec and test), every action field, external exec and webhook filters, and filter notification
  settings. A release goes through the download hub when its client is one of the hub's. Exec actions never use a
  shell.
- Import never skips an indexer: custom definitions in a Prowlarr backup become your custom definitions, Prowlarr's
  built-ins that are Newznab or Torznab underneath (Headphones VIP) come over as such, other addresses are asked for
  Newznab/Torznab capabilities, and the rest are added turned off under Needs your help, with a Create definition
  editor. The plan shows which way each indexer came over. Newznab indexers can sign in with a user name and password.
- `docs/parity/p-ui/` shows Prowlarr's frontend in Lantern (Indexers, Settings → General, System → Status, desktop
  and phone) next to Lantern's own look; `docs/PARITY.md` marks the theme and branding rows done.
- Prowlarr's web frontend is part of the image and opens at `/prowlarr/` after signing in to Lantern (Lantern's own
  pages stay at `/` for now). Its pages show errors where Lantern doesn't answer Prowlarr's API yet.
- Prowlarr's web frontend says Lantern: the header shows Lantern's name instead of Prowlarr's logo, and the donate,
  translate, Discord, Reddit, wiki and GitHub links and the Analytics section are gone. It never sends errors to
  Servarr's error reporting.
- Prowlarr's web frontend (`ui/prowlarr/`) has the kit's Encore look: Prowlarr's theme colours come from the kit's
  colour tokens and its fonts are the kit's, with its own layouts kept. Its themes are Auto, Encore, Dark, OLED and
  Light, as in Lantern's other apps.
- `scripts/upstream-merge.sh` brings Prowlarr's web frontend into `ui/prowlarr/` at the commit `UPSTREAM.lock`
  pins, as merges of an `upstream/prowlarr` snapshot branch, so later Prowlarr versions merge in with Lantern's
  changes kept (see `UPSTREAM.md`).
- Cross-seeding now decides like cross-seed v6.13.9: the match modes strict, flexible (the new default) and partial,
  all of cross-seed's decisions (kept on each result), its size, file, release group, resolution, source and
  proper/repack checks, its block list types and its rules for what to leave out (single episodes, non-video,
  recently searched, searched for too long). Its settings are under Cross-seed with cross-seed's defaults, and its
  daemon API (`/api/webhook`, `/api/announce`, `/api/job`, `/api/ping`, `/api/status`) answers on port 2468 with
  Lantern's API key, word for word as cross-seed does. "Search again after (days)" is replaced by cross-seed's
  "Search again after" and "Stop searching after".
- Prowlarr's own API for its web app, ported from Prowlarr v2.7.0.5723: system status, general, UI and development
  settings (with Prowlarr's defaults and checks), tags, custom filters, Prowlarr's translations, health, scheduled
  tasks, the command queue (test, health check, new API key, clearing the log), the log, backups and updates,
  under `/api/v1/…` with the API key or a signed-in session.
- Release filters now decide exactly as autobrr v1.90.0 does: release names are read by a port of rls (the
  library autobrr uses), announces by autobrr's own rules, and every filter field autobrr checks is checked the
  same way, with autobrr's rejection reasons. Download limits count per minute, hour, day, week, month or ever, in
  fixed or rolling windows. autobrr's macros (`{{ .TorrentName }}` …) are ready for the actions. Filters saved
  earlier keep working.
- The FlareSolverr API is now a port of FlareSolverr 3.5.2: the same answers and error messages, sessions
  (`sessions.create`, `.list`, `.destroy`, `session_ttl_minutes`), proxies with a user name and password,
  `returnScreenshot`, `waitInSeconds`, `disableMedia`, Turnstile tokens (`tabs_till_verify`), the "Verify you are
  human" click, and FlareSolverr's settings (`BROWSER_WAIT_TIMEOUT`, `DISABLE_MEDIA`, `LANG`, `TZ`, `PROXY_URL`,
  `PROMETHEUS_ENABLED` and the rest) under their own names. The browser now sends a normal Accept-Language header.
- Lantern now ports Prowlarr, autobrr, cross-seed and FlareSolverr instead of rewriting them: `UPSTREAM.lock` pins
  the versions it follows and `docs/PARITY.md` lists everything each one does, with what Lantern already matches.
- First-run setup: a new Lantern asks for "Create your account" (username, password twice, optional email) on the
  web, desktop and Android apps, instead of making `admin` with a printed password. The screen needs the one-time
  setup code Lantern prints in its log at start, so nobody else on the network can claim it first; the code stops
  working once the owner exists. `APP_ADMIN_USER` and `APP_ADMIN_PASSWORD` still make the owner for unattended
  installs. Built on the kit's new first-run setup (kit v0.8.0, up from v0.2.1).
- Indexers → Add is one searchable list of every torrent site and Usenet indexer, filtered by torrent or Usenet,
  privacy (private, semi-private, public) and language, and it says how many sites there are. Usenet indexers like
  Prowlarr's (NZBgeek, DrunkenSlug, NZBPlanet, NinjaCentral, DOGnzb, NZBFinder, altHUB, Tabula Rasa, abNZB and more)
  come with their address, API path and categories filled in, so only the API key is needed.
- Indexer logins, API keys and session cookies are sealed with the vault's key at rest; ones saved earlier are
  sealed at the next start. With the vault locked, an indexer says why instead of signing in with nothing.
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
