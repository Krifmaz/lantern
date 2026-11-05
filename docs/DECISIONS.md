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
- 2026-11-05 — The keeper's account numbers (ratio, upload, download, bonus) are typed in or sent through
  `PUT /api/keeper/{id}/stats`: tracker profile pages differ per site and the Cardigann definitions don't describe them.
  Sign-ins and activity are automatic for accounts linked to an indexer (its search, test and grab history).
- 2026-11-05 — ntfy, Discord and email alerts live in Lantern (`keeper/alerts.rs`, email through lettre) because the kit
  v0.2.1 has no notifier; they are generic and can move to the kit later.
- 2026-11-05 — Keep-alive signs in every `keeper_keepalive_days` (default 3), or every third of a site's sign-in rule
  when that is sooner, and at most once per 6 hours per account, so a failing site isn't hammered.
