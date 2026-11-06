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
- 2026-11-05 — (Replaced below: copies now follow cross-seed's categories and tags.) Cross-seed copies went into `<app category>.cross-seed` at the app's folder, tagged `lantern-xs-<grab>`,
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
- 2026-11-05 — Prowlarr's frontend fork (`ui/prowlarr/`) holds Prowlarr's `frontend/` and the root files its build
  reads (`package.json`, `yarn.lock`, `tsconfig.json`, `.yarnrc`, `.editorconfig`, `LICENSE`). `scripts/upstream-merge.sh`
  imports the commit `UPSTREAM.lock` pins as merges of the `upstream/prowlarr` snapshot branch (pushed to the
  remote); the merge base is rebuilt from the commit named in `ui/prowlarr/UPSTREAM`, so updates still merge
  three-way after a squash merge cut the link to that branch.
- 2026-11-05 — **Interim:** Prowlarr's frontend is served at `/prowlarr/` (its URL base set to `/prowlarr`, its API
  calls going to Lantern's `/api/v1`), and Lantern's own web app stays at `/`. It moves to `/` once its API rows in
  `docs/PARITY.md` are done. Its pages need a Lantern session, like Prowlarr's UI policy; without one they go to
  Lantern's sign-in at `/`. `initialize.json` hands the page the API key, as Prowlarr's does. Prowlarr's `?h=` cache
  breaker is left out: the files are answered with `no-cache`. The image drops the source maps.
- 2026-11-05 — The frontend is built with Node 22 (`node:22.23.4-trixie-slim`), not Prowlarr's Node 20, which is
  past its end of life; it builds unchanged with yarn 1 and the frozen lockfile.
- 2026-11-05 — Encore for Prowlarr's frontend: one map (`Styles/Themes/encore.js`) points every Prowlarr theme
  variable at the kit's tokens, and the theme picks `<html data-theme>`. The themes are `auto`, `encore`, `dark`,
  `oled` and `light`; Prowlarr's `auto` keeps its meaning (follow the system) as Encore on a dark system and the
  light theme on a light one. Text on the accent uses the kit's `--on-accent`, as Encore's accent is light. The
  chart palettes stay Prowlarr's, as charts need more distinct colours than the tokens have.
- 2026-11-05 — The page screenshots in `docs/parity/p-ui/` stand in for API answers Lantern does not have yet
  (localization, UI and host settings, app profiles, …) in the browser, with Prowlarr's shapes and defaults
  (`docs/parity/p-ui/shots.js`), so the look can be proven before the API is ported. They prove the theme, the
  branding and the mobile layout only; the page rows stay `todo` until their API exists.
- 2026-11-05 — cross-seed's daemon API runs on its own port, `cross_seed_port` (2468, cross-seed's default; 0 turns
  it off), not on Lantern's main port: autobrr's own `/api/webhook` would clash there, and tools already pointed at
  cross-seed keep their address. Its answers stay cross-seed's plain text with no content type (not `{"detail"}`),
  because scripts and autobrr read them as they are.
- 2026-11-05 — cross-seed's JavaScript regexes are translated for fancy-regex (`server/src/crossseed/jsre.rs`):
  `\d`, `\w` and `\b` stay ASCII as in JavaScript, and the one lookbehind of varying length (`S\d+[_\s-]{1,3}` in
  the episode pattern) becomes every fixed length it can have (up to 4 digits). A regex that gives up on a very long
  name counts as no match.
- 2026-11-05 — Until linking is ported, only a `MATCH` is added to the client (at the source's own save path, the
  files being the same). `MATCH_SIZE_ONLY` and `MATCH_PARTIAL` need linked files, so their torrents are saved as
  cross-seed's `outputDir` does (`<data>/cross-seeds/[type][tracker]name[hash].torrent`) and recorded as a
  `FAILURE` to inject, ready for the inject job. cross-seed's check that flexible and partial need `linkDirs` is
  shown as a problem on the Cross-seed page but doesn't stop searching.
- 2026-11-05 — Problems in the cross-seed settings (cross-seed's own messages) are shown and logged, not fatal:
  cross-seed refuses to start, but Lantern has more than cross-seed in it.
- 2026-11-05 — The `cross_seeds` table keeps its `decision` column with Lantern's words (`injected`,
  `files_differ`, ...), which the hub's cleanup reads; cross-seed's decision and injection result are new columns
  (`match_decision`, `result`). Search times move to cross-seed's per-indexer `timestamp` table, and
  `cross_seed_again_days` gives way to `excludeOlder` and `excludeRecentSearch`.
- 2026-11-05 — ~~A torrent searched by the webhook that isn't one of Lantern's grabs goes under the category
  `cross-seed`, since the clients don't tell Lantern a torrent's category (a grab's copies keep `<category>.cross-seed`).~~
  Replaced: cross-seed's own clients now read each torrent's category, so copies follow cross-seed exactly: the
  same category, tagged `cross-seed`, `<category>.cross-seed` only with `duplicateCategories`, `linkCategory` for
  linked files. The hub's cleanup still keeps a grab's files while a copy seeds from them (by the `cross_seeds`
  table, not by tags).
- 2026-11-05 — cross-seed's torrent clients (`server/src/crossseed/clients/`) are a port of its own, apart from the
  hub's download clients, so the requests are cross-seed's to the letter. `torrentClients` (`cross_seed_torrent_clients`,
  secret, one per line) left empty uses Lantern's enabled qBittorrent, rTorrent, Transmission and Deluge clients
  in priority order, written as cross-seed's entries (`/transmission/rpc`, `/RPC2` and `/json` added).
- 2026-11-05 — As in cross-seed, the clients' folders must be valid on Lantern's machine for linking and for the
  link-folder choice; Lantern writes only into the link folders you set and `cross-seeds/` in the data folder.
- 2026-11-05 — qBittorrent forms are URL-encoded (cross-seed sends them unencoded); qBittorrent reads the same
  values. Transmission's "409, new session id" answer is retried a bounded number of times (cross-seed recurses
  without a limit). Requests carry Lantern's own User-Agent.
- 2026-11-05 — Reflinks use Linux's `FICLONE` (the `libc` crate, Linux only); elsewhere `reflink` fails as on a
  file system without them, and `reflinkOrCopy` copies.
- 2026-11-05 — Announces pick the client torrents to compare by cross-seed's keys (title with season, episode,
  year) and its title comparison, without cross-seed's torrent cache and data folders (a later slice).
- 2026-11-05 — File paths are sorted by an approximation of JavaScript's `localeCompare` (punctuation, then digits,
  then letters, case ignored first); full ICU collation differs only for accented names and changes only the order
  files are listed in, not a decision.
- 2026-11-05 — Prowlarr's v1 API (track P-api) lives in `server/src/prowlarr/`. Its answers keep Prowlarr's field
  names and camelCase enums. A failed check answers Prowlarr's 400 array of failures (`propertyName`, `errorMessage`,
  …), because Prowlarr's web app reads it; every other error carries Prowlarr's `message` and the kit's `detail`.
- 2026-11-05 — Prowlarr's settings that Lantern has no setting for are kept in a `prowlarr_config` table. The ones
  Lantern already has use Lantern's: the API key (never changed by saving the general settings, as in Prowlarr; the
  ResetApiKey command changes it), the owner's username and password (shown as a mask, never as the hash), the
  instance name (Lantern's app name, which must contain "Lantern"), the application URL (Lantern's public address)
  and the proxy (Lantern's site proxy). The port is shown but read only (`APP_PORT`). Settings like SSL, bind address,
  authentication method, update mechanism and log levels are stored and checked like Prowlarr but not acted on yet.
- 2026-11-05 — A wrong `apikey` in the query is refused even when the `X-Api-Key` header is right, as in Prowlarr
  (which reads the query first); a signed-in session still works. Changes through a session need an admin.
- 2026-11-05 — Commands Lantern can't run yet (Backup, ApplicationIndexerSync, Housekeeping, …) answer as Prowlarr
  does for an unknown command (500, "Sequence contains no matching element"), so the web app shows the failure
  instead of a command that never runs. The queue, its limits and its order follow Prowlarr's `CommandQueue`.
- 2026-11-05 — `/api/v1/update` lists Lantern's own releases from its release notes (headings with "fix" go under
  fixed); updates come from the container image, so nothing is installable from the page. `appName` is "Lantern"
  and Prowlarr's wiki links are kept for health checks.
- 2026-11-05 — The log endpoints answer from a `prowlarr_logs` table and the `logs` and `UpdateLogs` folders in the
  data folder; writing to them (a tracing layer in `main.rs`) is the next P-api slice.
- 2026-11-05 — autobrr reads release names with the Go library rls (autobrr/rls v0.10.0, MIT, © m. jo istari). Its
  lexers, tag table and builder are ported line by line to `server/src/releases/rls/` (licence kept beside it) and
  checked against all 456 names of rls's own tests. Go regular expressions are translated to Rust's syntax with
  Go's ASCII meaning of `\b`, `\d`, `\s` and `\w`; Go's oddities that change results are kept on purpose (the
  disc/source/year lexer skipping ahead twice, autobrr's release-tag log score never being set).
- 2026-11-05 — autobrr walks its release-tag kinds in Go map order, which is random; every autobrr test gives the
  same answer in any order, and Lantern walks them in the order autobrr declares them.
- 2026-11-05 — Filters are stored with autobrr's field names (`match_release_groups`, `match_hdr`,
  `max_downloads_unit` …). A filter saved by earlier Lantern versions, or sent by the current filter form, is read
  with its earlier names (`release_sources`, `hdr`, `match_groups`, `max_downloads_per`; source ids in `sources`
  become `indexers`), and filters are sent back with both, until the forked autobrr frontend replaces the form.
  Lantern's own extras stay: which release sources a filter watches (`indexers`, ids), skipping a name already
  taken, and its simple Sonarr/Radarr/download hub actions until autobrr's actions are ported.
- 2026-11-05 — Filter validation answers with autobrr's own error texts (`validation: name can't be empty`) so the
  autobrr frontend and API clients see what autobrr says; the forked frontend shows them as autobrr does.
- 2026-11-05 — autobrr's macros run on Go's `text/template` with sprig's functions. Lantern implements the part of
  `text/template` macros use (fields, maps, `if`/`else if`/`with`/`range`, pipelines, variables, trimming,
  comments) and the common sprig functions (text, regex, numbers, lists, defaults, dates) in
  `server/src/releases/gotemplate.rs`; a sprig function it doesn't have is an error, as an unknown one is in Go.
- 2026-11-05 — A filter's download limit is counted from Lantern's release log (releases sent on). Fixed windows
  start where autobrr's SQLite query starts them (`weekday 0, -7 days` for weeks); rolling windows go back the
  period. RED and OPS announces without an uploader or record label are turned down with a reason when a filter
  asks for one, because Lantern can't ask those sites' APIs yet (autobrr does; it comes with the indexer API slice).
- 2026-11-05 — Track A-api builds in its own Cargo target folder, without incremental files: builds of the same crate
  from several worktrees in one shared target folder overwrote each other's files, and the disk is shared.
- 2026-11-05 — Newznab/Torznab out (track P-idx) follows Prowlarr v2.7.0.5723's `NewznabController`, `NewznabResults`
  and `IndexerCapabilities` byte for byte, with these choices: the feed's channel title and the caps' server title
  are the instance name, not "Prowlarr"; download links stay signed (`link` plus `sig`) rather than Prowlarr's
  encrypted link, because the download hub reads them; JSON errors keep Lantern's `{"detail"}` shape with
  Prowlarr's words ("Indexer Not Found", "Invalid Prowlarr link", "Failed to normalize provided link"), and a
  parameter that can't be read answers ASP.NET's problem shape plus `detail`; a failed download says "Download
  failed" as Prowlarr does, with the reason in history and on the indexer; a `cat` entry that isn't a number is
  skipped instead of failing the search.
- 2026-11-05 — The host recorded with each search or grab is the address from `X-Forwarded-For` or `X-Real-IP`, with
  no reverse DNS lookup. "On hold" dates are shown in UTC as `MM/dd/yyyy HH:mm:ss` (Prowlarr's invariant format).
  Testing an indexer from its page doesn't change its hold, as before. A site asking Lantern to slow down is
  recognised by the failure's text ("(429)", "limit reached", "too many requests") and held at least an hour.
- 2026-11-05 — Prowlarr's per-indexer base settings (query and grab limits, limits unit, minimum seeders, seed
  ratio and times, prefer magnet links) are kept in the indexer's settings under Prowlarr's field names
  (`baseSettings.queryLimit`, `torrentBaseSettings.preferMagnetUrl`, ...); Redirect is its own column. The `all`
  feed and the Jackett-style routes stay as Lantern extras. Newznab/Torznab indexers still announce the categories
  of their `categories` setting until reading the remote caps is ported.
- 2026-11-05 — autobrr's frontend fork (`ui/autobrr/`) holds the contents of autobrr's `web/` (its own Vite app,
  `package.json` and pnpm lockfile) plus `LICENSE`, not a `web/` folder inside it; `scripts/upstream-merge.sh` gained
  a case for it (a path ending in `/` brings that folder's contents) and keeps `upstream/autobrr`.
- 2026-11-05 — **Interim:** autobrr's frontend is served at `/autobrr/` with autobrr's base URL support (its
  `baseUrl` is `/autobrr/`), so its API calls go to `/autobrr/api/...` with the frontend unchanged: Lantern's own
  `/api/...` (auth, settings, health, …) would clash with autobrr's paths there. `server/src/autobrr_ui.rs` mounts the
  autobrr-compatible API router (track A-api) at `/autobrr/api`; until then every path there answers a plain 404.
  The pages and their files need a Lantern session (without one, pages go to Lantern's sign-in at `/`); the API
  prefix is not behind that check, as autobrr's API also takes API keys (webhooks, feeds) and answers 401 itself.
  The manifest, icons and `robots.txt` stay open, as browsers fetch them without the session.
- 2026-11-05 — The autobrr frontend is built as autobrr's Dockerfile builds it: Node 24.19.0
  (`node:24.19.0-trixie-slim`), pnpm at the version its `package.json` pins (`packageManager`) through corepack,
  `pnpm install --frozen-lockfile`, `pnpm run build`. CI also runs its lint and its vitest tests.
- 2026-11-05 — Encore for autobrr's frontend: Tailwind's colour scales are replaced by scales drawn from the kit's
  tokens (`ui/autobrr/src/lantern/colors.ts`), since autobrr's look is Tailwind classes, not theme variables. One class
  means a different thing in dark and light mode (`dark:bg-gray-800` is a card, `text-gray-800` is text), so each
  scale runs from the theme's lightest to its darkest colour, set per mode. autobrr's dark mode is Encore and its
  light mode the kit's light theme (`<html data-theme>` follows autobrr's `dark` class); autobrr's Theme setting
  (dark, light, system) stays as it is. Blue is the accent: fills use `--accent` with `--on-accent` text, links and
  focus rings `--accent-2`. Unlike Prowlarr's fork, the dashboard charts use the tokens too (approved `--ok`,
  matches `--accent-2`, rejected `--faint`, errored `--err`), as their bright blue was most of the dashboard.
- 2026-11-05 — The autobrr update banner and version chip are left in the frontend: they show only what the API
  answers, and Lantern's autobrr API must not point them at autobrr's releases. Help texts in autobrr's translations
  that name autobrr stay for now: several mean autobrr itself (its filter format, its hosted lists).
- 2026-11-05 — FlareSolverr is ported from v3.5.2 (`server/src/solver/`): its checks, waits, "Verify you are human"
  click, Turnstile token, sessions, POST form, error texts and HTTP codes, word for word; Lantern answers as version
  3.5.2. Two things stay Lantern's: one shared Chromium with a browser context per request and per session (instead
  of one Chrome per request), and Lantern's own indexers keep their plain-word errors, their own POST form (which
  keeps the indexer's encoding) and the page's real status (the API always says 200, as FlareSolverr does).
- 2026-11-05 — FlareSolverr's settings are read under its own names (`LOG_HTML`, `HEADLESS`, `BROWSER_WAIT_TIMEOUT`,
  `DISABLE_MEDIA`, `LANG`, `TZ`, `PROXY_URL`, `PROXY_USERNAME`, `PROXY_PASSWORD`, `PROMETHEUS_ENABLED`,
  `PROMETHEUS_PORT`) so a FlareSolverr compose file moves over as it is. `HOST` and `PORT` would clash with the main
  web server, so the API stays on `APP_SOLVER_PORT` (8191, all addresses, as FlareSolverr's defaults); `LOG_LEVEL`
  and `LOG_FILE` are Lantern's own log (`RUST_LOG`, stdout; the root is read-only). `HEADLESS=true` (the default)
  runs Chromium headless rather than in a hidden Xvfb window. The request log hides a proxy password, which
  FlareSolverr prints. Only `flaresolverr_request` and `flaresolverr_request_duration` are served as metrics
  (no Python process metrics).
- 2026-11-05 — A session the request makes by itself (`request.*` with an unknown `session`, or one past its
  `session_ttl_minutes`) has no proxy, as in FlareSolverr; a proxy with `username` and `password` signs in through the
  browser's auth handling (FlareSolverr loads an extension for it).
- 2026-11-05 — Indexer proxies (`server/src/indexers/proxies.rs`) follow Prowlarr's `IndexerHttpClient.GetProxies`:
  an indexer takes the first HTTP/SOCKS proxy and the first FlareSolverr that share one of its tags; a proxy without
  tags is off. SOCKS5 resolves names through the proxy (`socks5h`), as .NET does. Credentials are used only when
  both user name and password are set. Proxy settings are sealed with the vault. Proxy tags are kept as Prowlarr tag
  ids; indexer tags stay labels and are answered as ids.
- 2026-11-05 — A FlareSolverr proxy whose `Host` is Lantern's own solver port (localhost, a loopback address,
  0.0.0.0 or the container's host name) is answered in-process by `server/src/solver/`; any other host is called over
  HTTP like FlareSolverr.cs. A tagged FlareSolverr proxy replaces Lantern's built-in Cloudflare solving for that
  indexer; indexers without one keep the built-in solving. The call to FlareSolverr skips the general proxy for local
  and dotless hosts. An unknown proxy implementation answers 400 in words instead of Prowlarr's 500.
- 2026-11-05 — The proxy test pings Prowlarr's own `https://prowlarr.servarr.com/v1/ping`, as Prowlarr does, so a
  proxy that passes in Prowlarr passes in Lantern.
- 2026-11-05 — Per-site waits (`server/src/indexers/rate_limit.rs`) are Prowlarr's `RateLimitService`: one wait per
  host, shared by every indexer on it, at least 2 seconds or the definition's `requestDelay`. The 2-second floor is
  not applied to local sites (loopback hosts), so a local Jackett or test site isn't slowed; an indexer's own
  `request_delay` still applies there.
- 2026-11-05 — Generic Newznab/Torznab indexers ask the site for `t=caps` (Prowlarr's
  `NewznabCapabilitiesProvider`, kept 7 days and stored with the indexer). A failed fetch is logged and Lantern falls
  back to the stored answer or the `categories` setting; it never blocks adding or changing an indexer.
- 2026-11-05 — `AppProfileId` (default 1) and `DownloadClientId` (default 0) are stored on each indexer and checked as
  Prowlarr does. "App Profile does not exist" and "Download Client does not exist" are checked only once such a table
  exists (Prowlarr's app profiles and download clients come with P-app; Lantern's own download clients count until
  then).
- 2026-11-05 — Prowlarr's applications live in P-app's own tables (`prowlarr_applications`, `_application_status`,
  `_app_indexer_maps`, `_app_profiles`). The migration's `imported_apps` rows are copied in once each (remembered in
  `prowlarr_imported_apps_taken`, so a removed app does not come back); the old Prowlarr address becomes the app's
  Prowlarr Server, since Lantern takes over that address. `imported_apps` itself is left alone.
- 2026-11-05 — Lantern's indexers have no change events, so a watcher looks every 5 seconds (an indexer's name,
  switch, priority, tags, address and settings; and the API key) and runs Prowlarr's added, updated and deleted
  handlers, and a forced full sync after a new API key, as Prowlarr's event handlers do. The same watcher queues
  `ApplicationIndexerSync` when its 6 hours are up; no other scheduled task runs yet.
- 2026-11-05 — An indexer's app profile and seeding settings are read from its settings under Prowlarr's names
  (`appProfileId`, default profile 1; `torrentBaseSettings.appMinimumSeeders`, `.seedRatio`, `.seedTime`,
  `.packSeedTime`) until P-idx gives indexers those fields; an indexer whose saved login can't be opened counts as
  having invalid settings, so it is not synced.
- 2026-11-05 — App sync keeps Prowlarr's texts word for word where apps or their users see them: pushed indexers are
  named `<name> (Prowlarr)` and fields say "Prowlarr Server", so an app moved from Prowlarr sees no change and its
  existing indexers are matched by address instead of being added twice. Texts with `{appName}` say Lantern.
- 2026-11-05 — The provider API (`server/src/prowlarr/apps/provider.rs`, Prowlarr's `ProviderControllerBase` and
  `SchemaBuilder`) is generic over a `Family` (table, fields, test, hooks), so download clients, notifications and
  indexer proxies reuse it. Removing apps in bulk also removes their indexer maps and status, which Prowlarr leaves
  to its housekeeping.
- 2026-11-05 — autobrr's `/api` is served at `/autobrr/api` (`releases::autobrr_api::router()`, paths relative to
  autobrr's `/api`), where the forked autobrr frontend calls it: `lib.rs` passes it to `autobrr_ui::router(&st, api)`,
  so paths Lantern doesn't answer yet keep the frontend's plain 404.
- 2026-11-05 — autobrr API errors answer autobrr's `{"code", "message"}` with its status codes (RECORD_NOT_FOUND 404,
  BAD_REQUEST_PARAMS 400, INTERNAL_SERVER_ERROR 500, including for a body that can't be read, as autobrr does) and
  also `detail` with the same sentence, so the kit's toasts show it.
- 2026-11-05 — The download hub stays: an autobrr action whose download client is also one of the hub's clients
  (same kind, same host, same port; Deluge by host only) goes through the hub instead of straight to the client. The
  hub app is the action's category when the hub has an app of that name, else "Releases". On this route autobrr's
  per-download client options (limits, paths, reannounce) are not applied; the hub's own settings are. An extra
  action type `DOWNLOAD_HUB` always sends through the hub.
- 2026-11-05 — Lantern's own filter actions (from before the port) stay in the filter and still run next to the
  autobrr actions, but the autobrr API doesn't show them; nothing is migrated. Lantern's release sources stand in
  for autobrr's indexers, so `prune-deprecated` always removes 0. (Superseded below: archived indexers are pruned.)
- 2026-11-05 — autobrr actions, download clients and filter notifications are stored in their own tables
  (`autobrr_actions`, `autobrr_downloaders`, `autobrr_filter_notifications`, and a small `autobrr_notifications`
  until the notifications slice), each row's body in autobrr's JSON. External filters live in the filter's body.
  `created_at` and `updated_at` both come from the row's added time. Filter notifications are stored and checked
  but not sent until the notifications slice.
- 2026-11-05 — Torrent files the actions need on disk go to `$APP_DATA_DIR/tmp` (`/data/tmp` in the image), the only
  folder Lantern may write to; they are removed after the filter's actions ran. WATCH_FOLDER writes where the action
  says, so that folder has to be a mounted volume.
- 2026-11-05 — EXEC actions and exec external filters run the program with an argument list split the way
  go-shellquote splits autobrr's ExecArgs, never through a shell. Webhook headers are taken but not stored, as in
  autobrr; webhook type and method are stored but, as in autobrr, the webhook action always POSTs JSON.
- 2026-11-05 — Cross-seed's data folders are looked at again at most once a minute, by each folder's file count,
  sizes and newest time, instead of cross-seed's file watcher; no new dependency is needed and only changed folders
  are read again.
- 2026-11-05 — Cross-seed searches Lantern's own torrent indexers in place of Torznab addresses. Their capabilities
  come from Lantern's caps (music search counts as audio search), and a site on hold after failures counts as
  cross-seed's disabled indexer; a `RATE_LIMITED` site is left to Lantern's holds rather than snoozed again.
- 2026-11-05 — Cross-seed's cache of fetched `.torrent` files and decisions (`torrent_cache`, `decision`) is not
  ported: each candidate's decision stays in `cross_seeds`, and cleanup has nothing of it to prune. Saved torrents
  whose file names don't name a tracker get the indexer whose site host matches one of their trackers, since
  Lantern doesn't keep each indexer's tracker hosts.
- 2026-11-05 — Season episodes found in data folders are kept with an empty client host (not NULL) so the same
  path is stored once. The search job no longer leaves out the site a grab came from (cross-seed doesn't know it);
  searching one grab from Lantern's page still does.
- 2026-11-05 — The cross-seed jobs run on their schedule only while the `cross_seed` setting is on; a run asked for
  by `POST /api/job` or the webhook always happens. The job list follows the settings without a restart, and
  `updateIndexerCaps` is always there since Lantern always has its own indexers. The notification title stays
  `cross-seed`, so tools made for cross-seed read it. The `lantern cross-seed <command>` CLI is the next slice.
- 2026-11-05 — Prowlarr's v1 indexer resource (`prowlarr/indexers.rs`) maps Lantern's indexers both ways. Only
  Cardigann, Newznab and Torznab indexers exist: Prowlarr's native C# indexers are not in the schema, and an
  implementation Lantern doesn't have answers 400. A Cardigann site's secret settings are masked as `********` like
  every other secret (Prowlarr sends them in the clear), and a masked value keeps the saved one. A PUT can't change
  an indexer's kind or definition (add a new one instead). A Newznab/Torznab indexer that hasn't answered its caps
  yet shows Lantern's fallback capabilities. A definition template has no encoding until saved, as in Prowlarr.
- 2026-11-05 — The search page's grab (`POST /api/v1/search`) sends to Lantern's own download clients (the
  indexer's client, else the first enabled one for the protocol); Prowlarr's download client API is a later slice.
  Found releases are kept in memory for 30 minutes per data folder, as Prowlarr's cache is.
- 2026-11-05 — v1 history leaves out Lantern's own indexer tests and keeps no download ids. Lantern's searches
  don't keep whether an offset was given, so a query's history has an `offset` whenever it has a `limit` (the web
  app and the apps always send both).
- 2026-11-05 — SignalR (`/signalr/messages`) offers WebSockets only (no server-sent events or long polling); the
  web app connects that way. One broadcast channel per data folder carries indexer and command updates;
  the access token is the API key, or the signed-in session.
- 2026-11-05 — autobrr's indexers are rows of `release_sources` (kind `irc`, or `feed` for Torznab, Newznab and RSS
  indexers; `definition` = identifier, `settings` = the indexer's settings) plus a row of `autobrr_indexer_meta` for
  what sources lack (external identifier, implementation, base URL, proxy, archive state), so the ids filters already
  hold (`filter.indexers`) stay valid. Lantern's own `rss`/`indexer` sources show as RSS indexers. autobrr's IRC
  networks get their own tables (`autobrr_irc_networks`, `autobrr_irc_channels`).
- 2026-11-05 — Lantern's own IRC sources (from before the port) get an autobrr network made from their settings:
  the definition's server, port and TLS; `server`/`port` settings become the bouncer address; `nick`,
  `auth.account`/`auth.password` (NickServ), `pass` and `invite_command` carry over; the channels are the
  definition's (or the `channels` setting). The network is found again by server and nick. Those network settings
  stay on the source and are left out of the indexer's settings.
- 2026-11-05 — The IRC connection is Lantern's own line reader, written to behave like irc-go as autobrr uses it:
  registration ends at the end of the MOTD (376/422), CAP LS 302 and SASL PLAIN only when SASL is set, a taken nick
  gets `_`, a PING after 4 quiet minutes, and the trailing `:` only when a parameter needs it. irc-go's flood delay
  after each invite command isn't copied. TLS uses rustls, which can't speak TLS 1.0 or the old ciphers autobrr's
  `tls_skip_verify` allows; skip-verify still accepts any certificate.
- 2026-11-05 — autobrr's proxies aren't ported yet: a network or indexer with `use_proxy` and a proxy id refuses to
  connect with autobrr's "configured proxy could not be loaded", as autobrr does when the proxy is missing; `proxy`
  is always `null`.
- 2026-11-05 — The per-site indexer API clients (RED, OPS, GGn, BTN, PTP, MAM …) aren't ported yet:
  `POST /indexer/{id}/api/test` answers autobrr's "could not init api client" for a site with an API, and filters
  needing the uploader or record label from the API are refused with a plain reason. Changing an indexer's base URL
  doesn't rewrite the URLs of releases already seen (Lantern's release log keeps the page link only).
- 2026-11-05 — Only the `irc` stream of autobrr's server-sent events is served (at `/irc/events` and `/events`); the
  `logs` and notification inbox streams come with the logs and notifications slices, and until then answer
  r3labs/sse's "Stream not found!".
- 2026-11-05 — Go's regexp reads a `{` that doesn't start a repetition as itself (milkie's announce pattern starts
  with one); the Go pattern rewrite escapes such braces for Rust's regex crate.
- 2026-11-05 — The A-api IRC slice ships IRC and indexers; autobrr's feeds (Torznab, Newznab, RSS jobs and
  `/feeds*`) are the next slice. RSS polling now only looks at Lantern's own `rss` and `indexer` sources, so feed
  indexers made through autobrr's API wait for that slice.
- 2026-11-05 — autobrr's feeds and lists live in their own tables (`autobrr_feeds`, `autobrr_feed_cache`,
  `autobrr_lists`, `autobrr_list_filters`); a feed points at its indexer in `release_sources`, a list at its filters
  in `release_filters`. The cache's expiry is kept in unix milliseconds.
- 2026-11-05 — gofeed (RSS and Atom) is rewritten in `releases/feed/gofeed.rs` with only what autobrr reads: items,
  links, enclosures, custom elements and extensions. Lantern's own RSS reader stays for Lantern's own sources.
- 2026-11-05 — Feed releases go through the filters one at a time, and only when a filter that is on uses the feed's
  indexer, as autobrr does. A feed whose indexer uses a proxy stops with autobrr's "could not find proxy for indexer
  feed: record not found" until proxies are ported.
- 2026-11-05 — Where Go builds a list's titles from a map (random order), Lantern sorts each title's wildcards, so a
  filter's text doesn't change between refreshes. Plain text lists keep their line order. Steam's answer is read by
  its keys, as Go's map is.
- 2026-11-05 — List and feed errors follow Go: a missing list says "sql: no rows in result set" inside, and the API
  answers "could not find list"; changing a list saves it before the refresh, so a failing refresh answers 500 but
  keeps the change. A list on the wrong kind of download client says "download client N (TYPE) has runtime type
  *type.Client", close to Go's type check message.
- 2026-11-05 — The A-api feeds slice ships feeds and lists; autobrr's releases API (`/release*`, duplicate profiles,
  release cleanup jobs) is the next slice.
- 2026-11-06 — cross-seed's command line is a subcommand of the one binary: `lantern cross-seed <command> …`,
  next to `--version` and `healthcheck` in `main.rs`, with the rest of the arguments handed to
  `crossseed::cli::run` (a small parser that reads them as commander 8 does; no clap). It works on `APP_DATA_DIR`
  as cross-seed's works on its config folder. An option left out keeps Lantern's setting (Lantern's settings are
  cross-seed's `config.js`); options given are laid over the settings for that run only (`CrossSeed::set_overlay`,
  never saved) and checked with cross-seed's checks before anything runs, then the folders and clients are
  checked as `doStartupValidation` does. Problems print without cross-seed's option name and link anchor, since
  Lantern's checks name the setting in the message. `--torznab` is read but not used (Lantern searches its own
  indexers); `-v` turns on debug logging.
- 2026-11-06 — `gen-config` can't write a `config.js` Lantern would read, so it saves cross-seed's template values
  for the `cross_seed_*` settings when none is saved yet ("Configuration file already exists." otherwise) and makes
  `torrent_cache/`. `restore` and `update-torrent-cache-trackers` work on `torrent_cache/` in the data folder as
  cross-seed does; Lantern doesn't fill that folder (it keeps no cache of fetched `.torrent` files), so they help
  when cross-seed's folder is brought over. `clear-indexer-failures` takes every torrent indexer off hold in
  Lantern's indexer table (cross-seed's `status`/`retry_after`); `clear-cache` also forgets `cross_seed_searches`
  (the grabs' last search times).
- 2026-11-06 — `cross-seed daemon` runs the daemon API on its port and the jobs on their cadence whatever the
  `cross_seed` setting says, as cross-seed's daemon does; it doesn't start Lantern's web server. `reset-api-key`
  changes Lantern's `api_key` (cross-seed's `apiKey` is Lantern's), so apps using the old key need the new one,
  and a running server takes it after a restart. `diff` prints Node's `AssertionError` heading and its fields, with
  a plain line diff instead of Node's diff layout.
