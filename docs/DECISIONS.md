# Decisions

Non-obvious calls, one line each, newest last.

- 2026-11-05 — Generated from kit v0.2.1 (the latest tag) without the Android app; the Android app (goal 8, last in the build order) is added
  from the same template in its own PR, so CI stays short until then.
- 2026-11-05 — Releases before v0.1.0 are 0.0.x (`first-version: 0.0.1` in release.yml, Cargo versions 0.0.1); the
  template's first notes became `docs/releases/0.1.0.md` → `0.0.1.md`. The PR that completes indexers + Cloudflare
  solving + vault adds `docs/releases/0.1.0.md`, which makes that merge v0.1.0.
- 2026-11-05 — Download hub (goal 9, added later) goes after the account keeper and before IRC/RSS and cross-seed,
  since both push through the hub's download clients.
- 2026-11-05 — GPL-3.0-only (Cargo `license`), so the GPL indexer definitions and code learnt from GPL projects sit
  under a compatible license; the kit stays MIT, which GPL allows.
- 2026-11-05 — Port 9696 (Prowlarr's), so apps moving from Prowlarr keep their indexer address.
- 2026-11-05 — The Cardigann definitions are bundled in the image (`/app/definitions`, from Prowlarr/Indexers at
  build time) and updated daily into the data folder; the newer copy wins. Lantern never needs the network to start.
- 2026-11-05 — Download links in feeds are signed (HMAC-SHA256, key in `data/link.key`) instead of carrying the API
  key, so a link that leaks from an app's history can't be used to read anything else.
- 2026-11-05 — The *arr contract test uses Docker Hub `linuxserver/*` images (lscr.io is blocked in some CI
  networks); Readarr is pinned to `0.4.18-develop`, the last amd64 build after Readarr was retired.
- 2026-11-05 — A site whose login needs a captcha asks for a browser cookie instead; Cloudflare challenges say so in
  words until the Cloudflare solving stage lands.
- 2026-11-05 — The Android app is generated from the kit v0.2.1 template (the tag the rest of Lantern pins; kit v0.3.0
  changed nothing under android/), with Lantern's port 9696 as the address the emulator suggests.
- 2026-11-05 — Download hub: one folder per app, `<downloads folder>/<category>`, with paths swapped at whole folder
  names between Lantern, each client (its own `root`) and each app (its `import_path`), so a single shared mount
  keeps hardlinks possible. Paths are worked out when they are used, so a changed path applies to every app at once.
- 2026-11-05 — Download hub: qBittorrent torrents are found again by the tags `lantern` and `lantern-<grab id>`
  instead of computing the info hash, so magnets and files work the same and nothing else in the client is touched.
- 2026-11-05 — Download hub: Lantern retries sending a grab (and a download its client failed) with waits of 30 s,
  2, 10, 30 and 60 min, then tells the app it failed; picking another release stays the app's call.
- 2026-11-05 — Download hub: client passwords and API keys sit in the database like indexer secrets (masked in every
  answer) until the account vault lands, which moves them into it.
- 2026-11-05 — Download hub: webhooks are signed with Lantern's API key (HMAC-SHA256), which the app already holds,
  rather than a second secret per app.
- 2026-11-05 — Download hub clients without categories get the app's folder (Transmission, rTorrent) or category
  (NZBGet) with each download; NZBGet's DestDir must be Lantern's downloads folder so `<DestDir>/<category>` is the
  app's folder. rTorrent's XML-RPC can't delete files, so removing a grab there leaves its files on disk.
- 2026-11-05 — rTorrent is reached over its XML-RPC address (ruTorrent's `/RPC2` or an SCGI bridge), with a small
  XML-RPC reader of Lantern's own instead of another dependency.
- 2026-11-05 — The vault's data key is sealed with a key argon2id makes from `APP_VAULT_PASSPHRASE`, else from a random
  `<data>/vault.key` (0600). The key file is the default so Lantern can sign in to sites unattended after a restart;
  the passphrase is for people who don't want a copy of the data folder alone to open anything. A vault whose key is
  missing or wrong still starts, lists its accounts and says why it is locked; it never makes a new key over old items.
- 2026-11-05 — Only the owner (the kit's "may see saved secrets" account) can show secrets or make and restore vault
  backups, each time after typing their password (5 wrong tries per 5 minutes). Admins can add, change and remove.
- 2026-11-05 — Vault restore merges and never removes: new accounts are added, the backup's copy replaces one here
  unless it changed here after the backup.
- 2026-11-05 — Indexer logins stay in the indexer's own settings for now; moving them into the vault (sealed with the
  same key) is its own PR after Cloudflare solving merges, since that track edits the indexer code.
- 2026-11-05 — Imported indexers keep Prowlarr's number (`/{id}/api`) when it is free in Lantern, and Lantern takes
  Prowlarr's API key unless told not to, so apps moving with Prowlarr's address keep working unchanged.
- 2026-11-05 — The import never changes or removes what Lantern already has (a match by definition and name, or by
  address, is left as it is); only an empty proxy for sites and the API key are set.
- 2026-11-05 — Prowlarr's tagged indexer proxies are not imported: Lantern has one proxy for every site, and making a
  per-tag proxy global would change sites Prowlarr never sent through it. Prowlarr's own proxy is imported.
- 2026-11-05 — Imported secrets go through `migrate::Secrets`: `InVault` saves each account's logins and keys in the
  vault (one item per account, found by name and site) and also leaves them in the indexer's settings, since indexers
  still sign in from there. A locked vault doesn't stop the import; the plan says so. Imported apps wait in
  `imported_apps` for app sync (Lantern pushing indexers to apps).
- 2026-11-05 — FlareSolverr has no settings file, so its import reads pasted compose/.env/docker run text and only
  gives steps; nothing is saved, since Lantern solves by itself.
- 2026-11-05 — IRC/RSS releases were built in parallel with the download hub: pushes to a download client go through
  a small `DownloadHub` trait (`releases::push`) that the hub implements. A client push is a grab of one of the hub's
  apps (the filter names it; else an app "Releases" registered at the first push), so it gets that app's category,
  folder, retries and callbacks. Pushing to the *arrs (`release/push`) needs no hub.
- 2026-11-05 — Announce lines are read with autobrr's own definition files (bundled at `/app/announce` from autobrr's
  develop branch at image build; `<data>/announce` overrides), so every tracker autobrr knows works without Lantern
  keeping its own list. Their templates run through the Cardigann template engine.
- 2026-11-05 — Race-friendly defaults: a filter sends at once (no delay), the torrent file is only fetched when a size
  limit needs a size the announce didn't give, the first filter that takes a release wins (highest priority first),
  and a release name a filter already sent is skipped. A new RSS source's first look only notes what is there.
- 2026-11-05 — Cleanup is off by default and only ever touches grabs an app has marked imported and that have
  seeded enough, since it can delete downloaded files. Lantern checks the seeding goals itself instead of setting
  share limits in each client, so the rules work the same for every client. Per-tracker minimums (hit-and-run
  rules) come from the account keeper when it lands.
- 2026-11-05 — Cloudflare solving drives Debian's Chromium headless shell over the DevTools protocol (chromiumoxide),
  copied with the files of the packages it needs into the distroless image, so the image stays distroless and
  read-only (the browser's profile and scratch files live in `/data/browser`). It sends a person's Chrome user agent
  ("HeadlessChrome" said as "Chrome"), and indexers send that same agent with the kept cookies, as cf_clearance asks.
- 2026-11-05 — The FlareSolverr API listens on its own port 8191 without sign-in, as FlareSolverr does, so apps
  switch by address only; `APP_SOLVER_PORT=0` turns it off for anyone who doesn't need it.
- 2026-11-05 — When a site checks every plain request even with the cookies, a GET is read through the browser
  instead (pages only; files still go through Lantern's own client).
- 2026-11-05 — Cross-seed matches strictly: same release name (punctuation and file ending aside), size within 2 %,
  then every file's path and size equal in the candidate's `.torrent`. Copies whose files differ are listed, never
  added, so nothing is ever downloaded again or renamed. Magnet-only sites are skipped (no file list to compare).
- 2026-11-05 — Cross-seed copies go into `<app category>.cross-seed` at the app's folder, tagged `lantern-xs-<grab>`,
  so the app doesn't import them again and the hub's grab tracking never mistakes them for grabs. Each grab is
  searched when it finishes and again weekly, four times at most, 30 s apart by default.
- 2026-11-05 — The keeper's account numbers (ratio, upload, download, bonus) are typed in or sent through
  `PUT /api/keeper/{id}/stats`: tracker profile pages differ per site and the Cardigann definitions don't describe them.
  Sign-ins and activity are automatic for accounts linked to an indexer (its search, test and grab history).
- 2026-11-05 — ntfy, Discord and email alerts live in Lantern (`keeper/alerts.rs`, email through lettre) because the kit
  v0.2.1 has no notifier; they are generic and can move to the kit later.
- 2026-11-05 — Keep-alive signs in every `keeper_keepalive_days` (default 3), or every third of a site's sign-in rule
  when that is sooner, and at most once per 6 hours per account, so a failing site isn't hammered.
- 2026-11-05 — Indexer settings and cookies are sealed in place (`sealed:v1:` values in `indexers`, the vault's data key,
  associated text per column, not per id, since an import may renumber indexers) rather than copied into vault items:
  the indexer stays the one place its login is edited, and nothing else reads that table. The first start after the
  change seals older rows and vacuums the database so no plain copy is left in free pages.
- 2026-11-05 — The emulated qBittorrent identifies each app by the user name it signs in with (its password is
  Lantern's API key), and registers it on first sign-in. Sessions live in memory: after a restart the app signs in
  again on its own, as it does with a real qBittorrent. Calls that only tune the client (tags, priorities, share
  limits, pause/resume) are accepted and ignored, since Lantern's own rules decide.
- 2026-11-05 — Torrents an app uploads are kept in `<data>/grabs/` until the grab is removed, so a failed send can
  be retried; their info hash is worked out from the file (SHA-1 of the `info` dictionary), as apps expect.
- 2026-11-05 — The emulated SABnzbd knows the app by `ma_username` when the app sends one, else by the category it
  uses (a new app `SABnzbd <category>` the first time), since SABnzbd's API has only one key. The emulated NZBGet
  knows it by its Basic-auth user name, like qBittorrent, and keeps the parameters the app attaches (Sonarr's
  `drone`), which apps use to find their downloads again.
- 2026-11-05 — Soulseek goes through slskd's web API rather than speaking the Soulseek protocol itself: slskd is
  free, runs in Docker, and already handles the network, shares and queues. It is set up under Settings (one
  slskd per Lantern), not as a download client, because it is a source and a client at once. Each search result is
  one user folder; finished files are hardlinked (copied across file systems) into the app's folder so the app gets
  one folder with exact paths, like every other download. Only music searches ask Soulseek (`soulseek=1` forces it).
- 2026-11-05 — The emulated SABnzbd and NZBGet offer the categories Sonarr, Radarr, Lidarr and Readarr use out of the
  box (in small letters and with a capital, as Radarr's NZBGet `Movies` and Readarr's `Readarr`), besides each app's
  own, because the apps refuse to save a client whose category they don't see; Lantern takes any category anyway.
- 2026-11-05 — Import tries, for each Prowlarr indexer: a definition (Lantern's, or a custom one from the backup,
  which is saved to `<data>/definitions/custom` first and wins over the bundled one of the same id), then
  Newznab/Torznab underneath (Lantern's Usenet presets, matched by address or name, and built-ins such as
  Headphones VIP from a small table of their fixed address, path and key), then asking its address for caps at
  `/api`, `/api/torznab/api` and `/torznab/api` (all at once, 10 s each). An indexer none of these fit is added
  turned off with its settings and a `needs_help` reason, under `https://address-unknown.invalid` when Prowlarr had
  no address, so Create definition can turn it on later under the same number. Basic auth for Newznab is sent only
  to addresses under the indexer's own base.
- 2026-11-05 — First-run setup lives in the kit (kit-server-setup, kit-leptos, kit-iced, kit-compose; kit v0.8.0), as
  every kit app needs it; Lantern only makes the owner with its own name and password rules. Moving from kit v0.2.1
  to v0.8.0 at the same time was additive for Lantern (no code changes beyond setup).
- 2026-11-05 — With `APP_ADMIN_PASSWORD` unset, Lantern no longer writes `initial_admin_password.txt`: the setup code
  is kept in memory and printed in the log only, so no password sits in the data folder or its backups.
- 2026-11-05 — Usenet presets follow Prowlarr's list (address, API path, categories), plus the few Lantern had
  before (nzb.su, SceneNZBs, Usenet Crawler) so indexers added with them keep their name. AnimeTosho is marked
  public (anyone can use its feed) though Prowlarr lists every preset as private. Tabula Rasa now uses its plain
  address with `/api/v1/api`; ones saved with the old address keep working and are still recognised on import.
  A preset's categories are saved as the indexer's `categories` setting (editable), and apps see only those.
- 2026-11-05 — The Add list is filtered in the browser (the full list is fetched once), with the same rules the
  server applies to `/api/indexers/catalog?q=&protocol=&privacy=&language=` for other clients.
- 2026-11-05 — **Port, don't reimagine** (the owner's change of approach). Lantern now ports Prowlarr, autobrr,
  cross-seed and FlareSolverr module by module into Rust, keeping their behaviour, settings and edge cases, instead
  of writing its own versions. The versions are pinned in `UPSTREAM.lock`; `docs/PARITY.md` lists every endpoint,
  setting, page, action and background job, and a row is done only with a test or screenshot showing it works like
  the original. No new features until every row is done. Where a fresh Lantern module and the original differ, the
  original's behaviour wins; the earlier entries above stay as history and are replaced as each module is ported.
- 2026-11-05 — Pins are each project's latest stable release: Prowlarr v2.7.0.5723 (`master`; v3.0.0.5737 is a
  `develop` build), autobrr v1.90.0, cross-seed v6.13.9 (v7 is still pre-release) and FlareSolverr v3.5.2. The
  Cardigann and announce definitions are data, not code, so they keep following their branches (`UPSTREAM.lock`
  `[data.*]`).
- 2026-11-05 — Parity work runs in tracks that own separate folders (`docs/PARITY.md`, "Track"): Prowlarr API
  (`server/src/prowlarr/`, new), Prowlarr indexers (`server/src/indexers/`), Prowlarr apps
  (`server/src/prowlarr/apps/`), autobrr (`server/src/releases/`), cross-seed (`server/src/crossseed/`), FlareSolverr
  (`server/src/solver/`), the two forked web UIs (`ui/prowlarr/`, `ui/autobrr/`) and Lantern's own pages (`web/`).
  Migration (`server/src/migrate/`) is changed only in its own thread.
- 2026-11-05 — The Prowlarr and autobrr web frontends are forked as they are into `ui/prowlarr/` and `ui/autobrr/`
  (snapshot forks at the `UPSTREAM.lock` commit, like Photovault's), built in the image with Node and served by
  Lantern against its compatible APIs. The Encore look comes from mapping their own CSS variables to the kit's
  generated tokens, so their layouts, mobile behaviour and pages stay theirs; their logos, donation, Discord and
  wiki links are removed. Lantern's own Leptos pages remain only for the download hub, the vault and keeper, and
  Cloudflare solving.
- 2026-11-05 — cross-seed v6 has no web UI, so its settings get a Lantern page built from a settings schema (track L);
  its daemon, API and CLI behaviour are ported as they are.
