# Changelog

## Unreleased

### Added
- Cloudflare solving: a headless Chromium (in the image) gets past Cloudflare and DDoS-Guard checks by itself when an
  indexer meets one; the site's cookies are kept and reused, so most requests need no browser.
- The FlareSolverr API on port 8191 (`APP_SOLVER_PORT`), so apps that use FlareSolverr can use Lantern instead.
- A Cloudflare checks card on the Indexers page (test any address) and a Diagnostics check for the browser.
- CI checks the image as it ships: its browser gets past a Cloudflare-style check through the FlareSolverr API.

## 0.0.2

### Added
- Indexers: every site in the Prowlarr/Indexers definitions (bundled in the image and updated daily), plus
  Newznab and Torznab indexers, with login, tests, health and hold after repeated failures.
- Torznab/Newznab feeds per indexer and for all of them (`/{id}/api`, `/all/api`), Jackett and Prowlarr paths too,
  and signed download links, so Sonarr, Radarr, Lidarr and Readarr can add Lantern as an indexer.
- Prowlarr's v1 API (system status, indexers, stats, search) for apps that talk to Prowlarr.
- Indexers and Search pages, a notice and a Diagnostics check for indexers that stop working.
- Contract test in CI: real Sonarr, Radarr, Lidarr and Readarr add Lantern and search through it.
- Lantern listens on port 9696 (Prowlarr's), and makes an API key at first start.

## 0.0.1

### Added
- Lantern made from the Krifmaz kit template (v0.2.1).
- docs/GOALS.md (the goal and build order), docs/DECISIONS.md, GPL-3.0 license and credits in README.md.
- Releases are 0.0.x until v0.1.0 (indexers, Cloudflare solving and the vault).
