# Goals

The owner's goal for Lantern, saved as given (2026-11-05).

## Lantern

Build Lantern, one Rust app that replaces Prowlarr, FlareSolverr, autobrr and cross-seed, and keeps every tracker and Usenet account safe (Docker, Encore look), started from the Krifmaz/kit template.

1. Indexers (Prowlarr): torrent trackers and Usenet indexers from the Cardigann/Jackett YAML definitions (500+ sites, auto-updated), Torznab/Newznab APIs out, search with categories, caps, rate limits per site, health and stats. Prowlarr-compatible API so Marquee, Sonarr, Radarr and Lidarr can add it unchanged; app sync pushes indexers to them.
2. Cloudflare/challenge solving (FlareSolverr) built in: headless Chromium in the same image, used per site only when needed, cookies cached and reused; FlareSolverr-compatible API on its own port for other apps.
3. Releases as they appear (autobrr): IRC announce channels per tracker, RSS, filters (size, resolution, group, freeleech, year, tags), instant push to qBittorrent or to Marquee/Sonarr/Radarr; race-friendly defaults.
4. Cross-seeding (cross-seed): match completed torrents on other trackers by exact files, inject paused-free into qBittorrent with hardlinks, never re-download data; dry run and report first.
5. Account vault: site logins, passwords, cookies, passkeys, API keys, 2FA seeds, Usenet provider logins and invite notes, encrypted at rest (argon2 key + XChaCha20-Poly1305), never shown in logs or API answers, reveal only with re-auth, encrypted export/backup and restore.
6. Account keeper: per tracker track ratio, upload/download, seed time, hit-and-run rules, minimum activity and inactivity deadlines, bonus points, login and cookie expiry; keep logins alive automatically; warn days ahead (notice banner + ntfy/Discord/email) before any account is at risk.
7. Migration: import from Prowlarr, Jackett, FlareSolverr settings, autobrr and cross-seed configs, with a dry run.
8. Android app on kit-compose: account health, alerts, recent grabs.
9. Download hub (added 2026-11-05, as given): Lantern is the single download hub for all my apps. It owns indexers, logins, download clients (qBittorrent, SABnzbd, NZBGet), categories, paths and path mappings, seeding/ratio rules, cross-seed and cleanup. Apps connect with only a Lantern URL + API key: they register themselves (name, media type, import path) and Lantern creates their category and folders automatically. Apps send "search" and "grab" requests; Lantern downloads, tracks progress, retries failures, and calls the app back (webhook + live event stream) with the finished files' exact paths, hardlink-ready. Add Soulseek as a download source too, so music sources are centralised. Changing a client, indexer or path in Lantern applies to every app instantly. Keep Prowlarr-compatible APIs for older apps.

Build on Krifmaz/kit (pin the latest kit tag; read its docs/DESIGN.md and CLAUDE.md first). Use the kit for the look (Encore theme), What's new and changelog, self-updating web app, notice banners, smart toasts with Copy, confirm dialogs, bug reports and crash logs, diagnostics, settings from a schema, offline mode and the shared release workflows. Don't rebuild what the kit has; if something is missing or generic, add it to the kit instead of the app.

Work hands-off: make your own calls (log non-obvious ones in docs/DECISIONS.md), one PR per change, fix CI until green, squash-merge yourself, delete the branch. Every merge ships a normal vX.Y.Z release (no pre-releases). Never skip tests or force-push main. Only stop to ask about deleting user data, a needed secret or paid service. One short summary per finished goal.

## Rules added 2026-11-05 (as given)

- Use the original apps (source, docs, real API responses) as reference only; write everything fresh in Rust. Reuse data files directly (indexer YAML definitions, TRaSH Guides profiles). License the repo GPL-3.0 and credit the original projects and data sources in README.md.
- Support qBittorrent, rTorrent, Deluge and Transmission for torrents, and SABnzbd and NZBGet for Usenet, all behind one download-client layer; apps never see which client is used. Default to qBittorrent + SABnzbd.
- Universal: any app can use Lantern, not only mine.
  - Indexer side: Prowlarr-compatible API plus Torznab/Newznab endpoints per indexer and an aggregate one, so every *arr, Jackett/Prowlarr-style client and Kodi/other tools can search through it; FlareSolverr-compatible API for apps that call a solver.
  - Download side: emulate the qBittorrent Web API and the SABnzbd API (and NZBGet's JSON-RPC), so any app that supports those clients can add Lantern as its download client. Lantern routes each grab to the real client (qBittorrent, rTorrent, Deluge, Transmission, SABnzbd, NZBGet), applies its own categories, paths, seeding and cleanup rules, and reports status back in the format the app expects.
  - Every connected app (native or emulated) shows up in one Apps page: what it requested, where it went, live status, and its own category and folders.
  - Contract tests with real Sonarr, Radarr, Lidarr and Readarr talking to Lantern as both indexer and download client.

## Build order

One pull request each, in this order:

1. Scaffold from the kit template, this file, CI and releases through the kit's shared workflows.
2. Indexers with the Prowlarr-compatible API, per-indexer and aggregate Torznab/Newznab; contract tests with real
   Sonarr/Radarr/Lidarr/Readarr as indexer clients.
3. Cloudflare solving.
4. Account vault.
5. Account keeper.
6. Download hub: one download-client layer (qBittorrent, rTorrent, Deluge, Transmission, SABnzbd, NZBGet;
   default qBittorrent + SABnzbd), categories, paths and path mappings, seeding and
   ratio rules, cleanup; app registration (URL + API key), search and grab API, progress, retries, webhook and live
   event stream callbacks with exact, hardlink-ready paths.
7. Emulated download clients: the qBittorrent Web API, the SABnzbd API and NZBGet JSON-RPC, routed through the hub;
   the Apps page; contract tests with real *arrs as download-client users.
8. Soulseek as a download source.
9. IRC/RSS releases (autobrr), pushing through the hub's clients.
10. Cross-seed.
11. Migration from Prowlarr/FlareSolverr (and Jackett, autobrr, cross-seed).
12. Android app.

**v0.1.0** ships when indexers, Cloudflare solving and the vault work and Marquee or Sonarr can add Lantern as an
indexer. Releases before it are 0.0.x.
