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
