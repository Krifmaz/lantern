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
