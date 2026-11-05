# Parity with the originals

Lantern ports Prowlarr, autobrr, cross-seed and FlareSolverr (versions in [`UPSTREAM.lock`](../UPSTREAM.lock)). This
page lists everything each original has: every API endpoint, setting, page, action and background job. A row is
**done** only when its Proof column links a test or a screenshot that shows it works like the original. New features
wait until every row is done (see [DECISIONS.md](DECISIONS.md), 2026-11-05 "Port, don't reimagine").

## How to read and update this page

- **Status**: `todo` (nothing yet), `partial` (Lantern has its own version, written fresh; it still has to be
  ported and proven), `done` (ported and proven), `n/a` (left out on purpose; the reason is in the Lantern today column
  and in DECISIONS.md).
- **Proof**: a test path (`server/tests/…::name`) or a screenshot (`docs/parity/<track>/<name>.png`) next to the
  original's. A test proves an endpoint by comparing Lantern's answer with the original's for the same request (the
  original's answer is saved under `server/tests/fixtures/upstream/`).
- **Track**: who works on the row. Tracks run in parallel and never edit the same files:

| Track | Scope | Files it owns |
| --- | --- | --- |
| P-api | Prowlarr v1 API: indexers, tags, config, system, commands, history, logs, backups, filters, localization | `server/src/prowlarr/` (new) |
| P-idx | Prowlarr indexer engine: Cardigann, native indexers, Newznab/Torznab out, limits, proxies, health | `server/src/indexers/` |
| P-app | Prowlarr applications (app sync), app profiles, download clients, notifications | `server/src/prowlarr/apps/` (new) |
| P-ui | Prowlarr web frontend, forked and rethemed to Encore | `ui/prowlarr/` (new) |
| A-api | autobrr backend: IRC, feeds, filters, actions, releases, lists, macros, CLI | `server/src/releases/` |
| A-ui | autobrr web frontend, forked and rethemed to Encore | `ui/autobrr/` (new) |
| X | cross-seed: config, search, RSS, matching, injection, linking, API, CLI | `server/src/crossseed/` |
| F | FlareSolverr API and settings | `server/src/solver/` |
| L | Lantern's own pages: download hub, vault and keeper, Cloudflare solving | `web/` |

Migration (`server/src/migrate/`) is worked on in its own thread; tracks that need a change there ask first.

## Prowlarr v2.7.0.5723

### API endpoints

Every path in Prowlarr's `src/Prowlarr.Api.V1/openapi.json`, plus the Newznab/Torznab query types.

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `GET /api` | P-api | partial | | fresh version |
| `GET /api/v1/applications/{id}` | P-app | todo | |  |
| `PUT /api/v1/applications/{id}` | P-app | todo | |  |
| `DELETE /api/v1/applications/{id}` | P-app | todo | |  |
| `GET /api/v1/applications` | P-app | todo | |  |
| `POST /api/v1/applications` | P-app | todo | |  |
| `PUT /api/v1/applications/bulk` | P-app | todo | |  |
| `DELETE /api/v1/applications/bulk` | P-app | todo | |  |
| `GET /api/v1/applications/schema` | P-app | todo | |  |
| `POST /api/v1/applications/test` | P-app | todo | |  |
| `POST /api/v1/applications/testall` | P-app | todo | |  |
| `POST /api/v1/applications/action/{name}` | P-app | todo | |  |
| `POST /api/v1/appprofile` | P-app | todo | |  |
| `GET /api/v1/appprofile` | P-app | todo | |  |
| `DELETE /api/v1/appprofile/{id}` | P-app | todo | |  |
| `PUT /api/v1/appprofile/{id}` | P-app | todo | |  |
| `GET /api/v1/appprofile/{id}` | P-app | todo | |  |
| `GET /api/v1/appprofile/schema` | P-app | todo | |  |
| `POST /login` | P-ui | todo | |  |
| `GET /login` | P-ui | todo | |  |
| `GET /logout` | P-ui | todo | |  |
| `GET /api/v1/system/backup` | P-api | todo | |  |
| `DELETE /api/v1/system/backup/{id}` | P-api | todo | |  |
| `POST /api/v1/system/backup/restore/{id}` | P-api | todo | |  |
| `POST /api/v1/system/backup/restore/upload` | P-api | todo | |  |
| `GET /api/v1/command/{id}` | P-api | todo | |  |
| `DELETE /api/v1/command/{id}` | P-api | todo | |  |
| `POST /api/v1/command` | P-api | todo | |  |
| `GET /api/v1/command` | P-api | todo | |  |
| `GET /api/v1/customfilter/{id}` | P-api | todo | |  |
| `PUT /api/v1/customfilter/{id}` | P-api | todo | |  |
| `DELETE /api/v1/customfilter/{id}` | P-api | todo | |  |
| `GET /api/v1/customfilter` | P-api | todo | |  |
| `POST /api/v1/customfilter` | P-api | todo | |  |
| `PUT /api/v1/config/development/{id}` | P-api | todo | |  |
| `GET /api/v1/config/development/{id}` | P-api | todo | |  |
| `GET /api/v1/config/development` | P-api | todo | |  |
| `GET /api/v1/downloadclient/{id}` | P-app | todo | |  |
| `PUT /api/v1/downloadclient/{id}` | P-app | todo | |  |
| `DELETE /api/v1/downloadclient/{id}` | P-app | todo | |  |
| `GET /api/v1/downloadclient` | P-app | todo | |  |
| `POST /api/v1/downloadclient` | P-app | todo | |  |
| `PUT /api/v1/downloadclient/bulk` | P-app | todo | |  |
| `DELETE /api/v1/downloadclient/bulk` | P-app | todo | |  |
| `GET /api/v1/downloadclient/schema` | P-app | todo | |  |
| `POST /api/v1/downloadclient/test` | P-app | todo | |  |
| `POST /api/v1/downloadclient/testall` | P-app | todo | |  |
| `POST /api/v1/downloadclient/action/{name}` | P-app | todo | |  |
| `GET /api/v1/config/downloadclient/{id}` | P-app | todo | |  |
| `PUT /api/v1/config/downloadclient/{id}` | P-app | todo | |  |
| `GET /api/v1/config/downloadclient` | P-app | todo | |  |
| `GET /api/v1/filesystem` | P-api | todo | |  |
| `GET /api/v1/filesystem/type` | P-api | todo | |  |
| `GET /api/v1/health` | P-api | partial | | fresh version |
| `GET /api/v1/history` | P-api | todo | |  |
| `GET /api/v1/history/since` | P-api | todo | |  |
| `GET /api/v1/history/indexer` | P-api | todo | |  |
| `GET /api/v1/config/host/{id}` | P-api | todo | |  |
| `PUT /api/v1/config/host/{id}` | P-api | todo | |  |
| `GET /api/v1/config/host` | P-api | todo | |  |
| `GET /api/v1/indexer/{id}` | P-api | partial | | fresh version |
| `PUT /api/v1/indexer/{id}` | P-api | todo | |  |
| `DELETE /api/v1/indexer/{id}` | P-api | todo | |  |
| `GET /api/v1/indexer` | P-api | partial | | fresh version |
| `POST /api/v1/indexer` | P-api | todo | |  |
| `PUT /api/v1/indexer/bulk` | P-api | todo | |  |
| `DELETE /api/v1/indexer/bulk` | P-api | todo | |  |
| `GET /api/v1/indexer/schema` | P-api | todo | |  |
| `POST /api/v1/indexer/test` | P-api | todo | |  |
| `POST /api/v1/indexer/testall` | P-api | todo | |  |
| `POST /api/v1/indexer/action/{name}` | P-api | todo | |  |
| `GET /api/v1/indexer/categories` | P-api | todo | |  |
| `GET /api/v1/indexerproxy/{id}` | P-idx | todo | |  |
| `PUT /api/v1/indexerproxy/{id}` | P-idx | todo | |  |
| `DELETE /api/v1/indexerproxy/{id}` | P-idx | todo | |  |
| `GET /api/v1/indexerproxy` | P-idx | todo | |  |
| `POST /api/v1/indexerproxy` | P-idx | todo | |  |
| `GET /api/v1/indexerproxy/schema` | P-idx | todo | |  |
| `POST /api/v1/indexerproxy/test` | P-idx | todo | |  |
| `POST /api/v1/indexerproxy/testall` | P-idx | todo | |  |
| `POST /api/v1/indexerproxy/action/{name}` | P-idx | todo | |  |
| `GET /api/v1/indexerstats` | P-api | partial | | fresh version |
| `GET /api/v1/indexerstatus` | P-api | todo | |  |
| `GET /api/v1/localization` | P-api | todo | |  |
| `GET /api/v1/localization/options` | P-api | todo | |  |
| `GET /api/v1/log` | P-api | todo | |  |
| `GET /api/v1/log/file` | P-api | todo | |  |
| `GET /api/v1/log/file/{filename}` | P-api | todo | |  |
| `GET /api/v1/indexer/{id}/newznab` | P-idx | partial | | fresh version |
| `GET /{id}/api` | P-idx | partial | | fresh version |
| `GET /api/v1/indexer/{id}/download` | P-idx | partial | | fresh version |
| `GET /{id}/download` | P-idx | partial | | fresh version |
| `GET /api/v1/notification/{id}` | P-app | todo | |  |
| `PUT /api/v1/notification/{id}` | P-app | todo | |  |
| `DELETE /api/v1/notification/{id}` | P-app | todo | |  |
| `GET /api/v1/notification` | P-app | todo | |  |
| `POST /api/v1/notification` | P-app | todo | |  |
| `GET /api/v1/notification/schema` | P-app | todo | |  |
| `POST /api/v1/notification/test` | P-app | todo | |  |
| `POST /api/v1/notification/testall` | P-app | todo | |  |
| `POST /api/v1/notification/action/{name}` | P-app | todo | |  |
| `GET /ping` | P-api | todo | |  |
| `HEAD /ping` | P-api | todo | |  |
| `POST /api/v1/search` | P-api | todo | |  |
| `GET /api/v1/search` | P-api | partial | | fresh version |
| `POST /api/v1/search/bulk` | P-api | todo | |  |
| `GET /content/{path}` | P-ui | partial | `server/tests/prowlarr_ui.rs::only_content_is_open_without_signing_in`, `::the_manifest_names_the_instance_like_manifest_mapper` | the built files, open to everyone, served at `/prowlarr/Content/` for now (moves to `/` later) |
| `GET /` | P-ui | partial | | fresh version |
| `GET /{path}` | P-ui | partial | `server/tests/prowlarr_ui.rs::every_page_is_the_frontend_under_prowlarr`, `::the_page_gets_the_url_base_like_html_mapper_base`, `::pages_are_paths_without_a_dot_outside_content`, `::initialize_json_has_prowlarrs_fields_with_lanterns_api` | Prowlarr's frontend with its URL base set to `/prowlarr` (moves to `/` later); no `?h=` cache breaker |
| `GET /api/v1/system/status` | P-api | partial | | fresh version |
| `GET /api/v1/system/routes` | P-api | todo | |  |
| `GET /api/v1/system/routes/duplicate` | P-api | todo | |  |
| `POST /api/v1/system/shutdown` | P-api | todo | |  |
| `POST /api/v1/system/restart` | P-api | todo | |  |
| `GET /api/v1/tag/{id}` | P-api | todo | |  |
| `PUT /api/v1/tag/{id}` | P-api | todo | |  |
| `DELETE /api/v1/tag/{id}` | P-api | todo | |  |
| `GET /api/v1/tag` | P-api | partial | | fresh version |
| `POST /api/v1/tag` | P-api | todo | |  |
| `GET /api/v1/tag/detail/{id}` | P-api | todo | |  |
| `GET /api/v1/tag/detail` | P-api | todo | |  |
| `GET /api/v1/system/task` | P-api | todo | |  |
| `GET /api/v1/system/task/{id}` | P-api | todo | |  |
| `PUT /api/v1/config/ui/{id}` | P-api | todo | |  |
| `GET /api/v1/config/ui/{id}` | P-api | todo | |  |
| `GET /api/v1/config/ui` | P-api | todo | |  |
| `GET /api/v1/update` | P-api | todo | |  |
| `GET /api/v1/log/file/update` | P-api | todo | |  |
| `GET /api/v1/log/file/update/{filename}` | P-api | todo | |  |
| Newznab/Torznab `t=caps` (all of Prowlarr's parameters and answer fields) | P-idx | partial | | fresh version |
| Newznab/Torznab `t=search` (all of Prowlarr's parameters and answer fields) | P-idx | partial | | fresh version |
| Newznab/Torznab `t=tvsearch` (all of Prowlarr's parameters and answer fields) | P-idx | partial | | fresh version |
| Newznab/Torznab `t=movie` (all of Prowlarr's parameters and answer fields) | P-idx | partial | | fresh version |
| Newznab/Torznab `t=music` (all of Prowlarr's parameters and answer fields) | P-idx | partial | | fresh version |
| Newznab/Torznab `t=book` (all of Prowlarr's parameters and answer fields) | P-idx | partial | | fresh version |
| Newznab/Torznab errors (codes 100–910 as Prowlarr answers them) | P-idx | partial | | fresh version |
| `apikey` in query or `X-Api-Key` header; `/api/v1/…` behind forms/basic/external auth | P-api | partial | | fresh version |
| SignalR `/signalr/messages` (live updates the web UI listens to) | P-api | todo | | |

### Settings

#### General (config/host)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `BindAddress` | P-api | todo | |  |
| `Port` | P-api | todo | |  |
| `SslPort` | P-api | todo | |  |
| `EnableSsl` | P-api | todo | |  |
| `LaunchBrowser` | P-api | todo | |  |
| `AuthenticationMethod` | P-api | todo | |  |
| `AuthenticationRequired` | P-api | todo | |  |
| `AllowedHosts` | P-api | todo | |  |
| `AnalyticsEnabled` | P-api | todo | |  |
| `Username` | P-api | todo | |  |
| `Password` | P-api | todo | |  |
| `PasswordConfirmation` | P-api | todo | |  |
| `LogLevel` | P-api | todo | |  |
| `LogSizeLimit` | P-api | todo | |  |
| `ConsoleLogLevel` | P-api | todo | |  |
| `Branch` | P-api | todo | |  |
| `ApiKey` | P-api | todo | |  |
| `SslCertPath` | P-api | todo | |  |
| `SslCertPassword` | P-api | todo | |  |
| `UrlBase` | P-api | todo | |  |
| `TrustedNetworks` | P-api | todo | |  |
| `InstanceName` | P-api | todo | |  |
| `ApplicationUrl` | P-api | todo | |  |
| `UpdateAutomatically` | P-api | todo | |  |
| `UpdateMechanism` | P-api | todo | |  |
| `UpdateScriptPath` | P-api | todo | |  |
| `ProxyEnabled` | P-api | todo | |  |
| `ProxyType` | P-api | todo | |  |
| `ProxyHostname` | P-api | todo | |  |
| `ProxyPort` | P-api | todo | |  |
| `ProxyUsername` | P-api | todo | |  |
| `ProxyPassword` | P-api | todo | |  |
| `ProxyBypassFilter` | P-api | todo | |  |
| `ProxyBypassLocalAddresses` | P-api | todo | |  |
| `CertificateValidation` | P-api | todo | |  |
| `BackupFolder` | P-api | todo | |  |
| `BackupInterval` | P-api | todo | |  |
| `BackupRetention` | P-api | todo | |  |
| `HistoryCleanupDays` | P-api | todo | |  |
| `TrustCgnatIpAddresses` | P-api | todo | |  |

#### UI (config/ui)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `FirstDayOfWeek` | P-api | todo | |  |
| `CalendarWeekColumnHeader` | P-api | todo | |  |
| `ShortDateFormat` | P-api | todo | |  |
| `LongDateFormat` | P-api | todo | |  |
| `TimeFormat` | P-api | todo | |  |
| `ShowRelativeDates` | P-api | todo | |  |
| `EnableColorImpairedMode` | P-api | todo | |  |
| `UILanguage` | P-api | todo | |  |
| `Theme` | P-api | todo | |  |

#### Development (config/development)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `ConsoleLogLevel` | P-api | todo | |  |
| `LogSql` | P-api | todo | |  |
| `LogIndexerResponse` | P-api | todo | |  |
| `LogRotate` | P-api | todo | |  |
| `FilterSentryEvents` | P-api | todo | |  |

#### Indexer (every indexer, besides its definition's own fields)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `QueryLimit` | P-idx | todo | |  |
| `GrabLimit` | P-idx | todo | |  |
| `LimitsUnit` | P-idx | todo | |  |
| `AppMinimumSeeders` | P-idx | todo | |  |
| `SeedRatio` | P-idx | todo | |  |
| `SeedTime` | P-idx | todo | |  |
| `PackSeedTime` | P-idx | todo | |  |
| `PreferMagnetUrl` | P-idx | todo | |  |
| `Enable` | P-idx | todo | |  |
| `Priority` | P-idx | todo | |  |
| `Tags` | P-idx | todo | |  |
| `AppProfileId` | P-idx | todo | |  |
| `DownloadClientId` | P-idx | todo | |  |
| `Redirect` | P-idx | todo | |  |

#### Applications (per app)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `SyncLevel` (Disabled, Add and Remove Only, Full Sync) | P-app | todo | |  |
| `ProwlarrUrl`, `BaseUrl`, `ApiKey` | P-app | todo | |  |
| `SyncCategories` (and anime/other categories per app) | P-app | todo | |  |
| `SyncRejectBlocklistedTorrentHashesWhileGrabbing` (Sonarr/Radarr) | P-app | todo | |  |
| `Tags` (sync only matching indexers) | P-app | todo | |  |

#### App profiles

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `Name` | P-app | todo | |  |
| `EnableRss` | P-app | todo | |  |
| `EnableAutomaticSearch` | P-app | todo | |  |
| `EnableInteractiveSearch` | P-app | todo | |  |
| `MinimumSeeders` | P-app | todo | |  |

#### Indexer proxies

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| FlareSolverr (`Host`, `RequestTimeout`) | P-idx | todo | |  |
| HTTP (`Host`, `Port`, `Username`, `Password`) | P-idx | todo | |  |
| SOCKS4 | P-idx | todo | |  |
| SOCKS5 | P-idx | todo | |  |
| `Tags` (which indexers use it) | P-idx | todo | |  |

### Provider types

#### Applications

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| LazyLibrarian | P-app | todo | |  |
| Lidarr | P-app | todo | |  |
| Mylar | P-app | todo | |  |
| Radarr | P-app | todo | |  |
| Readarr | P-app | todo | |  |
| Sonarr | P-app | todo | |  |
| Whisparr | P-app | todo | |  |

#### Download clients

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Aria2 | P-app | todo | |  |
| Blackhole | P-app | todo | |  |
| Deluge | P-app | partial | | hub client |
| DownloadStation | P-app | todo | |  |
| Flood | P-app | todo | |  |
| FreeboxDownload | P-app | todo | |  |
| Hadouken | P-app | todo | |  |
| NzbVortex | P-app | todo | |  |
| Nzbget | P-app | partial | | hub client |
| Pneumatic | P-app | todo | |  |
| QBittorrent | P-app | partial | | hub client |
| Sabnzbd | P-app | partial | | hub client |
| Transmission | P-app | partial | | hub client |
| Vuze | P-app | todo | |  |
| rTorrent | P-app | partial | | hub client |
| uTorrent | P-app | todo | |  |

#### Notifications

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Apprise | P-app | todo | |  |
| CustomScript | P-app | todo | |  |
| Discord | P-app | todo | |  |
| Email | P-app | todo | |  |
| Gotify | P-app | todo | |  |
| Join | P-app | todo | |  |
| Mailgun | P-app | todo | |  |
| Notifiarr | P-app | todo | |  |
| Ntfy | P-app | todo | |  |
| Prowl | P-app | todo | |  |
| PushBullet | P-app | todo | |  |
| Pushcut | P-app | todo | |  |
| Pushover | P-app | todo | |  |
| SendGrid | P-app | todo | |  |
| Signal | P-app | todo | |  |
| Simplepush | P-app | todo | |  |
| Slack | P-app | todo | |  |
| Telegram | P-app | todo | |  |
| Twitter | P-app | todo | |  |
| Webhook | P-app | todo | |  |

#### Native indexers (C#, not Cardigann)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| AlphaRatio | P-idx | todo | |  |
| Anidex | P-idx | todo | |  |
| Anidub | P-idx | todo | |  |
| AnimeBytes | P-idx | todo | |  |
| AnimeZ | P-idx | todo | |  |
| Animedia | P-idx | todo | |  |
| AvistaZ | P-idx | todo | |  |
| BakaBT | P-idx | todo | |  |
| BeyondHD | P-idx | todo | |  |
| BinSearch | P-idx | todo | |  |
| BitHDTV | P-idx | todo | |  |
| BrokenStones | P-idx | todo | |  |
| CGPeers | P-idx | todo | |  |
| CinemaZ | P-idx | todo | |  |
| DICMusic | P-idx | todo | |  |
| DesiGaane | P-idx | todo | |  |
| ExoticaZ | P-idx | todo | |  |
| FunFile | P-idx | todo | |  |
| GazelleGames | P-idx | todo | |  |
| GreatPosterWall | P-idx | todo | |  |
| HDSpace | P-idx | todo | |  |
| HDTorrents | P-idx | todo | |  |
| IPTorrents | P-idx | todo | |  |
| ImmortalSeed | P-idx | todo | |  |
| Knaben | P-idx | todo | |  |
| Libble | P-idx | todo | |  |
| MTeamTp | P-idx | todo | |  |
| MyAnonamouse | P-idx | todo | |  |
| Nebulance | P-idx | todo | |  |
| NorBits | P-idx | todo | |  |
| NzbIndex | P-idx | todo | |  |
| Orpheus | P-idx | todo | |  |
| PixelHD | P-idx | todo | |  |
| PornoLab | P-idx | todo | |  |
| PreToMe | P-idx | todo | |  |
| PrivateHD | P-idx | todo | |  |
| Redacted | P-idx | todo | |  |
| RetroFlix | P-idx | todo | |  |
| RevolutionTT | P-idx | todo | |  |
| RuTracker | P-idx | todo | |  |
| SceneHD | P-idx | todo | |  |
| SceneTime | P-idx | todo | |  |
| SecretCinema | P-idx | todo | |  |
| Shazbat | P-idx | todo | |  |
| Shizaproject | P-idx | todo | |  |
| SpeedApp | P-idx | todo | |  |
| SpeedCD | P-idx | todo | |  |
| SubsPlease | P-idx | todo | |  |
| Toloka | P-idx | todo | |  |
| TorrentBytes | P-idx | todo | |  |
| TorrentDay | P-idx | todo | |  |
| TorrentSyndikat | P-idx | todo | |  |
| TorrentsCSV | P-idx | todo | |  |
| Uniotaku | P-idx | todo | |  |
| XSpeeds | P-idx | todo | |  |
| ZonaQ | P-idx | todo | |  |

#### Cardigann

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Every filter, template function, login method (form, post, cookie, get, oneurl), captcha and `test` block of definition schema v11 | P-idx | partial | | fresh version |
| Definitions updated from Prowlarr/Indexers every day (`IndexerDefinitionUpdateCommand`) | P-idx | partial | | fresh version |
| Indexer search limits (`QueryLimit`, `GrabLimit`, per hour/day) and per-site rate limits (`RequestDelay`) | P-idx | todo | |  |
| Health: disable after failures with Prowlarr's back-off steps, `IndexerStatus` escalation | P-idx | todo | |  |
| Redirect (`Redirect` = apps download straight from the site) | P-idx | todo | |  |

### Pages and actions (web UI)

#### Indexers (`/`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| list with columns, sort and custom filters | P-ui | todo | |  |
| Add indexer (search, protocol, language, privacy, categories filters) | P-ui | todo | |  |
| Edit indexer, test, delete | P-ui | todo | |  |
| Test all | P-ui | todo | |  |
| Sync app indexers | P-ui | todo | |  |
| Select mode: bulk edit (enable, priority, app profile, seed ratio/time, tags), bulk delete | P-ui | todo | |  |
| Indexer info (details, categories, history) | P-ui | todo | |  |

#### Search (`/search`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| query with type, indexers and categories | P-ui | todo | |  |
| results table with columns, sort and custom filters | P-ui | todo | |  |
| Grab (to download client) and Download (.torrent/.nzb) | P-ui | todo | |  |
| Select mode: grab several | P-ui | todo | |  |
| search via URL parameters | P-ui | todo | |  |

#### Stats (`/indexers/stats`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| charts: response time, queries, grabs, user agents, hosts per indexer | P-ui | todo | |  |
| filter by date, indexers and tags | P-ui | todo | |  |

#### History (`/history`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| table with columns and filters | P-ui | todo | |  |
| details | P-ui | todo | |  |
| clear history | P-ui | todo | |  |

#### Settings → Indexers (`/settings/indexers`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| indexer proxies: add, edit, test, delete | P-ui | todo | |  |

#### Settings → Apps (`/settings/applications`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| applications: add, edit, test, delete, test all | P-ui | todo | |  |
| app profiles: add, edit, delete | P-ui | todo | |  |
| Sync App Indexers | P-ui | todo | |  |
| Manage applications (bulk) | P-ui | todo | |  |

#### Settings → Download Clients (`/settings/downloadclients`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, test, delete | P-ui | todo | |  |
| manage (bulk) | P-ui | todo | |  |

#### Settings → Connect (`/settings/connect`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| notifications: add, edit, test, delete | P-ui | todo | |  |

#### Settings → Tags (`/settings/tags`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| list with where each tag is used | P-ui | todo | |  |
| delete unused | P-ui | todo | |  |

#### Settings → General (`/settings/general`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| host, security, proxy, logging, analytics, updates, backups sections | P-ui | todo | |  |
| reset API key | P-ui | todo | |  |
| save and restart notice | P-ui | todo | |  |

#### Settings → UI (`/settings/ui`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| dates, times, theme, language, colour-impaired mode | P-ui | todo | |  |

#### Settings → Development (`/settings/development`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| log levels, SQL and indexer response logging | P-ui | todo | |  |

#### System → Status (`/system/status`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| health with fixes | P-ui | todo | |  |
| about (version, paths, runtime) | P-ui | todo | |  |
| more info links (to strip or point to Lantern) | P-ui | done | `docs/parity/p-ui/system-status-desktop.png` | More info and Donations removed (`ui/prowlarr/LANTERN.md`) |

#### System → Tasks (`/system/tasks`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| scheduled tasks with last/next run, run now | P-ui | todo | |  |
| queue | P-ui | todo | |  |

#### System → Backup (`/system/backup`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| backup now | P-ui | todo | |  |
| restore (upload) | P-ui | todo | |  |
| download, delete | P-ui | todo | |  |

#### System → Updates (`/system/updates`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| changes per version | P-ui | todo | |  |
| install update (becomes Lantern's kit updater) | P-ui | todo | |  |

#### System → Events (`/system/events`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| log table, filter, clear | P-ui | todo | |  |

#### System → Log files (`/system/logs/files`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| list, download, clear; update logs | P-ui | todo | |  |

#### Login and first run

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| forms login page | P-ui | todo | |  |
| authentication required modal (first run) | P-ui | todo | |  |

#### Everywhere

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| mobile layout (sidebar collapses, tables scroll) | P-ui | partial | `docs/parity/p-ui/indexers-phone.png`, `sidebar-open-phone.png`, `settings-general-phone.png`, `system-status-phone.png` | Prowlarr's layout unchanged; the folding sidebar is shown, scrolling tables wait for pages with data |
| keyboard shortcuts | P-ui | todo | |  |
| live updates over SignalR | P-ui | todo | |  |
| theme (rethemed to Encore) | P-ui | done | `docs/parity/p-ui/indexers-desktop.png`, `settings-general-desktop.png`, `system-status-desktop.png`, `indexers-desktop-light.png` next to `kit-reference-desktop.png` | every theme variable mapped to the kit's tokens, the kit's fonts (`ui/prowlarr/LANTERN.md`) |
| branding: Lantern's name in the header, no donate, translate, Discord, Reddit, wiki or GitHub links, no Analytics section or Sentry reports | P-ui | done | `docs/parity/p-ui/indexers-desktop.png`, `system-status-desktop.png` | `ui/prowlarr/LANTERN.md` lists each change |

### Background jobs

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `MessagingCleanupCommand` every 5 min | P-api | todo | |  |
| `ClearExpiredCacheCommand` every 5 min | P-api | todo | |  |
| `ApplicationUpdateCheckCommand` every 6 * 60 min | P-api | todo | |  |
| `CheckHealthCommand` every 6 * 60 min | P-api | todo | |  |
| `HousekeepingCommand` every 24 * 60 min | P-api | todo | |  |
| `CleanUpHistoryCommand` every 24 * 60 min | P-api | todo | |  |
| `IndexerDefinitionUpdateCommand` every 24 * 60 min | P-api | partial | | fresh version |
| `ApplicationIndexerSyncCommand` every 6 * 60 min | P-api | todo | |  |
| `BackupCommand` (backup interval setting) | P-api | todo | |  |

#### Commands (`POST /api/v1/command`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `ApplicationIndexerSyncCommand` | P-api | todo | |  |
| `ApplicationUpdateCheckCommand` | P-api | todo | |  |
| `ApplicationUpdateCommand` | P-api | todo | |  |
| `BackupCommand` | P-api | todo | |  |
| `CheckHealthCommand` | P-api | todo | |  |
| `CleanupHistoryCommand` | P-api | todo | |  |
| `ClearExpiredCacheCommand` | P-api | todo | |  |
| `ClearHistoryCommand` | P-api | todo | |  |
| `ClearLogCommand` | P-api | todo | |  |
| `DeleteLogFilesCommand` | P-api | todo | |  |
| `DeleteUpdateLogFilesCommand` | P-api | todo | |  |
| `HousekeepingCommand` | P-api | todo | |  |
| `IndexerDefinitionUpdateCommand` | P-api | todo | |  |
| `MessagingCleanupCommand` | P-api | todo | |  |
| `ResetApiKeyCommand` | P-api | todo | |  |
| `RestartCommand` | P-api | todo | |  |
| `ShutdownCommand` | P-api | todo | |  |
| `TestCommand` | P-api | todo | |  |

#### Health checks

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `AllowedHostsCheck` | P-api | todo | |  |
| `ApiKeyValidationCheck` | P-api | todo | |  |
| `AppDataLocationCheck` | P-api | todo | |  |
| `ApplicationLongTermStatusCheck` | P-api | todo | |  |
| `ApplicationStatusCheck` | P-api | todo | |  |
| `DownloadClientStatusCheck` | P-api | todo | |  |
| `IndexerCheck` | P-api | todo | |  |
| `IndexerDownloadClientCheck` | P-api | todo | |  |
| `IndexerLongTermStatusCheck` | P-api | todo | |  |
| `IndexerNoDefinitionCheck` | P-api | todo | |  |
| `IndexerProxyStatusCheck` | P-api | todo | |  |
| `IndexerStatusCheck` | P-api | todo | |  |
| `IndexerVIPCheck` | P-api | todo | |  |
| `IndexerVIPExpiredCheck` | P-api | todo | |  |
| `NotificationStatusCheck` | P-api | todo | |  |
| `OutdatedDefinitionCheck` | P-api | todo | |  |
| `ProxyCheck` | P-api | todo | |  |
| `ReleaseBranchCheck` | P-api | todo | |  |
| `SystemTimeCheck` | P-api | todo | |  |
| `UpdateCheck` | P-api | todo | |  |

## autobrr v1.90.0

### API endpoints

Every route mounted in `internal/http/server.go` (under the base URL).

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `POST /api/auth/login` | A-api | todo | |  |
| `POST /api/auth/onboard` | A-api | todo | |  |
| `GET /api/auth/onboard` | A-api | todo | |  |
| `POST /api/auth/logout` | A-api | todo | |  |
| `GET /api/auth/validate` | A-api | todo | |  |
| `PATCH /api/auth/user/{username}` | A-api | todo | |  |
| `GET /api/healthz/liveness` | A-api | todo | |  |
| `GET /api/healthz/readiness` | A-api | todo | |  |
| `GET /api/actions` | A-api | todo | |  |
| `POST /api/actions` | A-api | todo | |  |
| `DELETE /api/actions/{actionID}` | A-api | todo | |  |
| `PUT /api/actions/{actionID}` | A-api | todo | |  |
| `PATCH /api/actions/{actionID}/toggleEnabled` | A-api | todo | |  |
| `GET /api/alerts` | A-api | todo | |  |
| `GET /api/config` | A-api | todo | |  |
| `PATCH /api/config` | A-api | todo | |  |
| `GET /api/downloaders` | A-api | todo | |  |
| `POST /api/downloaders` | A-api | todo | |  |
| `PUT /api/downloaders` | A-api | todo | |  |
| `POST /api/downloaders/test` | A-api | todo | |  |
| `GET /api/downloaders/{clientID}` | A-api | todo | |  |
| `DELETE /api/downloaders/{clientID}` | A-api | todo | |  |
| `GET /api/downloaders/{clientID}/arr/tags` | A-api | todo | |  |
| `GET /api/filters` | A-api | todo | |  |
| `POST /api/filters` | A-api | todo | |  |
| `POST /api/filters/indexers/prune-deprecated` | A-api | todo | |  |
| `POST /api/filters/external/test` | A-api | todo | |  |
| `GET /api/filters/{filterID}` | A-api | todo | |  |
| `PUT /api/filters/{filterID}` | A-api | todo | |  |
| `PATCH /api/filters/{filterID}` | A-api | todo | |  |
| `DELETE /api/filters/{filterID}` | A-api | todo | |  |
| `GET /api/filters/{filterID}/duplicate` | A-api | todo | |  |
| `PUT /api/filters/{filterID}/enabled` | A-api | todo | |  |
| `GET /api/filters/{filterID}/notifications` | A-api | todo | |  |
| `PUT /api/filters/{filterID}/notifications` | A-api | todo | |  |
| `GET /api/feeds` | A-api | todo | |  |
| `POST /api/feeds` | A-api | todo | |  |
| `POST /api/feeds/test` | A-api | todo | |  |
| `POST /api/feeds/caps` | A-api | todo | |  |
| `GET /api/feeds/{feedID}` | A-api | todo | |  |
| `PUT /api/feeds/{feedID}` | A-api | todo | |  |
| `DELETE /api/feeds/{feedID}` | A-api | todo | |  |
| `DELETE /api/feeds/{feedID}/cache` | A-api | todo | |  |
| `PATCH /api/feeds/{feedID}/enabled` | A-api | todo | |  |
| `GET /api/feeds/{feedID}/latest` | A-api | todo | |  |
| `POST /api/feeds/{feedID}/forcerun` | A-api | todo | |  |
| `GET /api/feeds/{feedID}/caps` | A-api | todo | |  |
| `GET /api/irc` | A-api | todo | |  |
| `POST /api/irc` | A-api | todo | |  |
| `GET /api/irc/network/{networkID}` | A-api | todo | |  |
| `PUT /api/irc/network/{networkID}` | A-api | todo | |  |
| `DELETE /api/irc/network/{networkID}` | A-api | todo | |  |
| `POST /api/irc/network/{networkID}/cmd` | A-api | todo | |  |
| `POST /api/irc/network/{networkID}/channel` | A-api | todo | |  |
| `GET /api/irc/network/{networkID}/restart` | A-api | todo | |  |
| `GET /api/irc/network/{networkID}/channel/{channel}/history` | A-api | todo | |  |
| `POST /api/irc/network/{networkID}/channel/{channel}/announce/process` | A-api | todo | |  |
| `GET /api/indexer/schema` | A-api | todo | |  |
| `POST /api/indexer` | A-api | todo | |  |
| `GET /api/indexer` | A-api | todo | |  |
| `GET /api/indexer/options` | A-api | todo | |  |
| `GET /api/indexer/deprecations` | A-api | todo | |  |
| `DELETE /api/indexer/deprecations/{indexerID}` | A-api | todo | |  |
| `GET /api/indexer/{indexerID}` | A-api | todo | |  |
| `PUT /api/indexer/{indexerID}` | A-api | todo | |  |
| `DELETE /api/indexer/{indexerID}` | A-api | todo | |  |
| `POST /api/indexer/{indexerID}/api/test` | A-api | todo | |  |
| `PATCH /api/indexer/{indexerID}/enabled` | A-api | todo | |  |
| `GET /api/lists` | A-api | todo | |  |
| `POST /api/lists` | A-api | todo | |  |
| `POST /api/lists/refresh` | A-api | todo | |  |
| `POST /api/lists/{listID}/refresh` | A-api | todo | |  |
| `PUT /api/lists/{listID}` | A-api | todo | |  |
| `DELETE /api/lists/{listID}` | A-api | todo | |  |
| `GET /api/keys` | A-api | todo | |  |
| `POST /api/keys` | A-api | todo | |  |
| `DELETE /api/keys/{apikey}` | A-api | todo | |  |
| `GET /api/logs/files` | A-api | todo | |  |
| `GET /api/logs/files/{logFile}` | A-api | todo | |  |
| `GET /api/notification` | A-api | todo | |  |
| `POST /api/notification` | A-api | todo | |  |
| `POST /api/notification/test` | A-api | todo | |  |
| `GET /api/notification/pushover/sounds` | A-api | todo | |  |
| `GET /api/notification/inbox` | A-api | todo | |  |
| `DELETE /api/notification/inbox` | A-api | todo | |  |
| `POST /api/notification/inbox/read` | A-api | todo | |  |
| `POST /api/notification/inbox/delete` | A-api | todo | |  |
| `GET /api/notification/{notificationID}` | A-api | todo | |  |
| `PUT /api/notification/{notificationID}` | A-api | todo | |  |
| `DELETE /api/notification/{notificationID}` | A-api | todo | |  |
| `GET /api/proxy` | A-api | todo | |  |
| `POST /api/proxy` | A-api | todo | |  |
| `POST /api/proxy/test` | A-api | todo | |  |
| `GET /api/proxy/{proxyID}` | A-api | todo | |  |
| `PUT /api/proxy/{proxyID}` | A-api | todo | |  |
| `DELETE /api/proxy/{proxyID}` | A-api | todo | |  |
| `GET /api/proxy/{proxyID}/usage` | A-api | todo | |  |
| `GET /api/release` | A-api | todo | |  |
| `GET /api/release/recent` | A-api | todo | |  |
| `GET /api/release/stats` | A-api | todo | |  |
| `GET /api/release/stats/activity` | A-api | todo | |  |
| `GET /api/release/stats/volume` | A-api | todo | |  |
| `GET /api/release/stats/heatmap` | A-api | todo | |  |
| `GET /api/release/stats/indexers` | A-api | todo | |  |
| `GET /api/release/stats/filters` | A-api | todo | |  |
| `GET /api/release/indexers` | A-api | todo | |  |
| `DELETE /api/release` | A-api | todo | |  |
| `POST /api/release/process` | A-api | todo | |  |
| `GET /api/release/{releaseID}` | A-api | todo | |  |
| `POST /api/release/{releaseID}/actions/{actionStatusID}/retry` | A-api | todo | |  |
| `GET /api/release/profiles/duplicate` | A-api | todo | |  |
| `POST /api/release/profiles/duplicate` | A-api | todo | |  |
| `DELETE /api/release/profiles/duplicate/{profileId}` | A-api | todo | |  |
| `GET /api/release/cleanup-jobs` | A-api | todo | |  |
| `POST /api/release/cleanup-jobs` | A-api | todo | |  |
| `GET /api/release/cleanup-jobs/{jobID}` | A-api | todo | |  |
| `PUT /api/release/cleanup-jobs/{jobID}` | A-api | todo | |  |
| `DELETE /api/release/cleanup-jobs/{jobID}` | A-api | todo | |  |
| `PATCH /api/release/cleanup-jobs/{jobID}/enabled` | A-api | todo | |  |
| `POST /api/release/cleanup-jobs/{jobID}/run` | A-api | todo | |  |
| `GET /api/updates/latest` | A-api | todo | |  |
| `GET /api/updates/check` | A-api | todo | |  |
| `POST /api/webhook/lists/trigger` | A-api | todo | |  |
| `POST /api/webhook/lists/trigger/arr` | A-api | todo | |  |
| `POST /api/webhook/lists/trigger/lists` | A-api | todo | |  |
| `GET /api/webhook/lists/trigger` | A-api | todo | |  |
| `GET /api/webhook/lists/trigger/arr` | A-api | todo | |  |
| `GET /api/webhook/lists/trigger/lists` | A-api | todo | |  |
| `POST /api/webhook/lists/trigger/{listID}` | A-api | todo | |  |
| `GET /api/auth/oidc/config` | A-api | todo | |  |
| `GET /api/auth/oidc/callback` | A-api | todo | |  |
| GET /api/events (server-sent events: IRC, logs) | A-api | todo | |  |
| GET /metrics (metrics port, basic auth users) | A-api | todo | |  |

### Settings (`config.toml` and environment `AUTOBRR__*`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `BaseURL` | A-api | todo | |  |
| `BaseURLModeLegacy` | A-api | todo | |  |
| `CheckForUpdates` | A-api | todo | |  |
| `CorsAllowedOrigins` | A-api | todo | |  |
| `CustomDefinitions` | A-api | todo | |  |
| `DatabaseAutoMigrate` | A-api | todo | |  |
| `DatabaseDSN` | A-api | todo | |  |
| `DatabaseMaxBackups` | A-api | todo | |  |
| `DatabaseType` | A-api | todo | |  |
| `Host` | A-api | todo | |  |
| `LogLevel` | A-api | todo | |  |
| `LogMaxBackups` | A-api | todo | |  |
| `LogMaxSize` | A-api | todo | |  |
| `LogPath` | A-api | todo | |  |
| `MetricsBasicAuthUsers` | A-api | todo | |  |
| `MetricsEnabled` | A-api | todo | |  |
| `MetricsHost` | A-api | todo | |  |
| `MetricsPort` | A-api | todo | |  |
| `OIDCClientID` | A-api | todo | |  |
| `OIDCClientSecret` | A-api | todo | |  |
| `OIDCDisableBuiltInLogin` | A-api | todo | |  |
| `OIDCEnabled` | A-api | todo | |  |
| `OIDCIssuer` | A-api | todo | |  |
| `OIDCRedirectURL` | A-api | todo | |  |
| `OIDCScopes` | A-api | todo | |  |
| `Port` | A-api | todo | |  |
| `PostgresDatabase` | A-api | todo | |  |
| `PostgresExtraParams` | A-api | todo | |  |
| `PostgresHost` | A-api | todo | |  |
| `PostgresPass` | A-api | todo | |  |
| `PostgresPort` | A-api | todo | |  |
| `PostgresSSLMode` | A-api | todo | |  |
| `PostgresSocket` | A-api | todo | |  |
| `PostgresUser` | A-api | todo | |  |
| `ProfilingEnabled` | A-api | todo | |  |
| `ProfilingHost` | A-api | todo | |  |
| `ProfilingPort` | A-api | todo | |  |

### Filter fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `Name` | A-api | todo | |  |
| `Enabled` | A-api | todo | |  |
| `MinSize` | A-api | partial | | fresh version |
| `MaxSize` | A-api | partial | | fresh version |
| `Delay` | A-api | partial | | fresh version |
| `Priority` | A-api | partial | | fresh version |
| `MaxDownloads` | A-api | todo | |  |
| `MaxDownloadsUnit` | A-api | todo | |  |
| `MaxDownloadsPeriod` | A-api | todo | |  |
| `MaxDownloadsWindowType` | A-api | todo | |  |
| `MatchReleases` | A-api | partial | | fresh version |
| `ExceptReleases` | A-api | partial | | fresh version |
| `UseRegex` | A-api | todo | |  |
| `MatchReleaseGroups` | A-api | partial | | fresh version |
| `ExceptReleaseGroups` | A-api | todo | |  |
| `AnnounceTypes` | A-api | todo | |  |
| `Scene` | A-api | todo | |  |
| `Origins` | A-api | todo | |  |
| `ExceptOrigins` | A-api | todo | |  |
| `Bonus` | A-api | todo | |  |
| `Freeleech` | A-api | partial | | fresh version |
| `FreeleechPercent` | A-api | todo | |  |
| `SmartEpisode` | A-api | todo | |  |
| `Shows` | A-api | todo | |  |
| `Seasons` | A-api | todo | |  |
| `Episodes` | A-api | todo | |  |
| `Resolutions` | A-api | partial | | fresh version |
| `Codecs` | A-api | todo | |  |
| `Sources` | A-api | todo | |  |
| `Containers` | A-api | todo | |  |
| `MatchHDR` | A-api | todo | |  |
| `ExceptHDR` | A-api | todo | |  |
| `MatchOther` | A-api | todo | |  |
| `ExceptOther` | A-api | todo | |  |
| `Years` | A-api | partial | | fresh version |
| `Months` | A-api | todo | |  |
| `Days` | A-api | todo | |  |
| `Artists` | A-api | todo | |  |
| `Albums` | A-api | todo | |  |
| `MatchReleaseTypes` | A-api | todo | |  |
| `ExceptReleaseTypes` | A-api | todo | |  |
| `Formats` | A-api | todo | |  |
| `Quality` | A-api | todo | |  |
| `Media` | A-api | todo | |  |
| `PerfectFlac` | A-api | todo | |  |
| `Cue` | A-api | todo | |  |
| `Log` | A-api | todo | |  |
| `LogScore` | A-api | todo | |  |
| `MatchCategories` | A-api | todo | |  |
| `ExceptCategories` | A-api | todo | |  |
| `MatchUploaders` | A-api | todo | |  |
| `ExceptUploaders` | A-api | todo | |  |
| `MatchRecordLabels` | A-api | todo | |  |
| `ExceptRecordLabels` | A-api | todo | |  |
| `MatchLanguage` | A-api | todo | |  |
| `ExceptLanguage` | A-api | todo | |  |
| `Tags` | A-api | partial | | fresh version |
| `ExceptTags` | A-api | todo | |  |
| `TagsAny` | A-api | todo | |  |
| `ExceptTagsAny` | A-api | todo | |  |
| `TagsMatchLogic` | A-api | todo | |  |
| `ExceptTagsMatchLogic` | A-api | todo | |  |
| `MatchReleaseTags` | A-api | todo | |  |
| `ExceptReleaseTags` | A-api | todo | |  |
| `UseRegexReleaseTags` | A-api | todo | |  |
| `MatchDescription` | A-api | todo | |  |
| `ExceptDescription` | A-api | todo | |  |
| `UseRegexDescription` | A-api | todo | |  |
| `MinSeeders` | A-api | todo | |  |
| `MaxSeeders` | A-api | todo | |  |
| `MinLeechers` | A-api | todo | |  |
| `MaxLeechers` | A-api | todo | |  |
| `IsAutoUpdated` | A-api | todo | |  |
| `ReleaseProfileDuplicateID` | A-api | todo | |  |
| `DuplicateHandling` | A-api | todo | |  |
| External filters (exec, webhook) with expected status | A-api | todo | |  |
| Filter notifications per event | A-api | todo | |  |

### Actions

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `TEST` | A-api | todo | |  |
| `EXEC` | A-api | todo | |  |
| `QBITTORRENT` | A-api | partial | | fresh version |
| `DELUGE_V1` | A-api | todo | |  |
| `DELUGE_V2` | A-api | todo | |  |
| `RTORRENT` | A-api | todo | |  |
| `TRANSMISSION` | A-api | todo | |  |
| `PORLA` | A-api | todo | |  |
| `ARIA2` | A-api | todo | |  |
| `WATCH_FOLDER` | A-api | partial | | fresh version |
| `WEBHOOK` | A-api | todo | |  |
| `RADARR` | A-api | partial | | fresh version |
| `SONARR` | A-api | partial | | fresh version |
| `LIDARR` | A-api | partial | | fresh version |
| `WHISPARR` | A-api | todo | |  |
| `WHISPARR_V3` | A-api | todo | |  |
| `READARR` | A-api | partial | | fresh version |
| `SPORTARR` | A-api | todo | |  |
| `SABNZBD` | A-api | todo | |  |
| `NZBGET` | A-api | todo | |  |

#### Action fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `Name` | A-api | todo | |  |
| `Type` | A-api | todo | |  |
| `Enabled` | A-api | todo | |  |
| `ExecCmd` | A-api | todo | |  |
| `ExecArgs` | A-api | todo | |  |
| `WatchFolder` | A-api | todo | |  |
| `Category` | A-api | todo | |  |
| `Tags` | A-api | todo | |  |
| `Label` | A-api | todo | |  |
| `SavePath` | A-api | todo | |  |
| `DownloadPath` | A-api | todo | |  |
| `Paused` | A-api | todo | |  |
| `IgnoreRules` | A-api | todo | |  |
| `FirstLastPiecePrio` | A-api | todo | |  |
| `SkipHashCheck` | A-api | todo | |  |
| `ContentLayout` | A-api | todo | |  |
| `LimitUploadSpeed` | A-api | todo | |  |
| `LimitDownloadSpeed` | A-api | todo | |  |
| `LimitRatio` | A-api | todo | |  |
| `LimitSeedTime` | A-api | todo | |  |
| `PriorityLayout` | A-api | todo | |  |
| `RatioGroup` | A-api | todo | |  |
| `ReAnnounceSkip` | A-api | todo | |  |
| `ReAnnounceDelete` | A-api | todo | |  |
| `ReAnnounceInterval` | A-api | todo | |  |
| `ReAnnounceMaxAttempts` | A-api | todo | |  |
| `WebhookHost` | A-api | todo | |  |
| `WebhookType` | A-api | todo | |  |
| `WebhookMethod` | A-api | todo | |  |
| `WebhookData` | A-api | todo | |  |
| `WebhookHeaders` | A-api | todo | |  |
| `ExternalDownloadClientID` | A-api | todo | |  |

#### Macros (template variables for actions)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `{{ .Artists }}` `{{ .Audio }}` `{{ .AudioChannels }}` `{{ .AudioFormat }}` `{{ .Bitrate }}` `{{ .Bonus }}` `{{ .Categories }}` `{{ .Category }}` | A-api | todo | |  |
| `{{ .Codec }}` `{{ .Container }}` `{{ .Cookie }}` `{{ .CurrentDay }}` `{{ .CurrentHour }}` `{{ .CurrentMinute }}` `{{ .CurrentMonth }}` `{{ .CurrentSecond }}` | A-api | todo | |  |
| `{{ .CurrentYear }}` `{{ .CurrentTimeUnixMS }}` `{{ .CurrentUnixTimeMS }}` `{{ .Description }}` `{{ .DownloadUrl }}` `{{ .Episode }}` `{{ .FilterID }}` `{{ .FilterName }}` | A-api | todo | |  |
| `{{ .Freeleech }}` `{{ .FreeleechPercent }}` `{{ .Group }}` `{{ .GroupID }}` `{{ .HDR }}` `{{ .HasCue }}` `{{ .HasLog }}` `{{ .Implementation }}` | A-api | todo | |  |
| `{{ .Indexer }}` `{{ .IndexerIdentifier }}` `{{ .IndexerIdentifierExternal }}` `{{ .IndexerName }}` `{{ .InfoUrl }}` `{{ .IsDuplicate }}` `{{ .Language }}` `{{ .Leechers }}` | A-api | todo | |  |
| `{{ .LogScore }}` `{{ .MagnetURI }}` `{{ .MetaIMDB }}` `{{ .MetaTMDB }}` `{{ .MetaTVDB }}` `{{ .MetaIGDB }}` `{{ .Origin }}` `{{ .Other }}` | A-api | todo | |  |
| `{{ .PreTime }}` `{{ .Protocol }}` `{{ .Proper }}` `{{ .Region }}` `{{ .ReleaseTags }}` `{{ .Repack }}` `{{ .Resolution }}` `{{ .Season }}` | A-api | todo | |  |
| `{{ .Seeders }}` `{{ .Size }}` `{{ .SizeString }}` `{{ .SkipDuplicateProfileID }}` `{{ .SkipDuplicateProfileName }}` `{{ .Source }}` `{{ .Tags }}` `{{ .Title }}` | A-api | todo | |  |
| `{{ .TorrentDataRawBytes }}` `{{ .TorrentHash }}` `{{ .TorrentID }}` `{{ .TorrentName }}` `{{ .TorrentPathName }}` `{{ .TorrentUrl }}` `{{ .TorrentTmpFile }}` `{{ .Type }}` | A-api | todo | |  |
| `{{ .Uploader }}` `{{ .RecordLabel }}` `{{ .RawVars }}` `{{ .Website }}` `{{ .Year }}` `{{ .Month }}` `{{ .Day }}` | A-api | todo | |  |

### Download clients

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `ARIA2` | A-api | todo | |  |
| `DELUGE_V1` | A-api | todo | |  |
| `DELUGE_V2` | A-api | todo | |  |
| `LIDARR` | A-api | todo | |  |
| `NZBGET` | A-api | todo | |  |
| `PORLA` | A-api | todo | |  |
| `QBITTORRENT` | A-api | todo | |  |
| `RADARR` | A-api | todo | |  |
| `READARR` | A-api | todo | |  |
| `RTORRENT` | A-api | todo | |  |
| `SABNZBD` | A-api | todo | |  |
| `SONARR` | A-api | todo | |  |
| `SPORTARR` | A-api | todo | |  |
| `TRANSMISSION` | A-api | todo | |  |
| `WHISPARR` | A-api | todo | |  |
| `WHISPARR_V3` | A-api | todo | |  |

### Notifications

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `DISCORD` | A-api | todo | |  |
| `NOTIFIARR` | A-api | todo | |  |
| `PUSHOVER` | A-api | todo | |  |
| `SLACK` | A-api | todo | |  |
| `TELEGRAM` | A-api | todo | |  |
| `GOTIFY` | A-api | todo | |  |
| `NTFY` | A-api | todo | |  |
| `LUNASEA` | A-api | todo | |  |
| `SHOUTRRR` | A-api | todo | |  |
| `WEBHOOK` | A-api | todo | |  |
| `BARK` | A-api | todo | |  |
| `BUILTIN` | A-api | todo | |  |
| Events: push approved, rejected, error; IRC disconnected, reconnected; app update available | A-api | todo | |  |

### Feeds

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `TORZNAB` | A-api | todo | |  |
| `NEWZNAB` | A-api | todo | |  |
| `RSS` | A-api | partial | | fresh version |
| caps, max age, timeout, cookie, user agent, proxy per feed | A-api | todo | |  |
| first run only stores what is there | A-api | todo | |  |

### Lists

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `RADARR` | A-api | todo | |  |
| `SONARR` | A-api | todo | |  |
| `LIDARR` | A-api | todo | |  |
| `READARR` | A-api | todo | |  |
| `WHISPARR` | A-api | todo | |  |
| `WHISPARR_V3` | A-api | todo | |  |
| `SPORTARR` | A-api | todo | |  |
| `MDBLIST` | A-api | todo | |  |
| `METACRITIC` | A-api | todo | |  |
| `PLAINTEXT` | A-api | todo | |  |
| `TRAKT` | A-api | todo | |  |
| `STEAM` | A-api | todo | |  |
| `ANILIST` | A-api | todo | |  |

### IRC

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Networks: server, port, TLS, nick, NickServ/SASL auth, invite command, bouncer, proxy, bot mode | A-api | partial | | fresh version |
| Channels with password, per indexer definition | A-api | partial | | fresh version |
| Announce parsing with every definition in `internal/indexer/definitions` (line patterns, multiline, vars, mappings) | A-api | partial | | fresh version |
| Health monitor and reconnect back-off | A-api | todo | |  |
| Message history per channel | A-api | todo | |  |
| Send raw command | A-api | todo | |  |
| Manual announce (process a pasted line) | A-api | todo | |  |
| Pre/after-connect commands, invite flow | A-api | todo | |  |

### Indexers

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Every indexer definition (IRC, torznab, newznab, RSS), settings, base URL override | A-api | partial | | fresh version |
| Custom definitions folder (`CustomDefinitions`) | A-api | todo | |  |
| Deprecated indexers list and prune | A-api | todo | |  |
| Indexer API test (per-site APIs for size and freeleech: BTN, PTP, RED, OPS, MAM, GGn, ...) | A-api | todo | |  |

### Releases

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Release log with every action status | A-api | partial | | fresh version |
| Duplicate release profiles (skip if already grabbed by title, year, quality, ...) | A-api | todo | |  |
| Cleanup jobs (scheduled delete of old releases) | A-api | todo | |  |
| Retry an action | A-api | todo | |  |
| Stats: activity, volume, heatmap, top indexers and filters | A-api | todo | |  |
| Delete releases older than / by status / by indexer | A-api | todo | |  |

### Pages and actions (web UI)

#### Login, Onboarding

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| sign in (built-in or OIDC) | A-ui | todo | |  |
| create the first user | A-ui | todo | |  |

#### Dashboard (`/`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| stat tiles | A-ui | todo | |  |
| activity and volume charts, hour heatmap, top lists | A-ui | todo | |  |
| recent activity table | A-ui | todo | |  |
| edit the grid (add, move, remove widgets) | A-ui | todo | |  |

#### Filters (`/filters`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| list with search, sort, indexer filter | A-ui | todo | |  |
| add, enable toggle, duplicate, delete | A-ui | todo | |  |
| import filter (JSON / autobrr text) | A-ui | todo | |  |
| export filter (JSON, Discord) | A-ui | todo | |  |

#### Filter details (`/filters/$filterId`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| General | A-ui | todo | |  |
| Movies and TV | A-ui | todo | |  |
| Music | A-ui | todo | |  |
| Books | A-ui | todo | |  |
| Advanced | A-ui | todo | |  |
| External | A-ui | todo | |  |
| Actions (add, edit, toggle, delete, per type form) | A-ui | todo | |  |
| Notifications | A-ui | todo | |  |
| save, delete, duplicate | A-ui | todo | |  |

#### Releases (`/releases`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| table with filters (indexer, status, search), paging | A-ui | todo | |  |
| details with action statuses, retry action | A-ui | todo | |  |

#### Logs (`/logs`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| live log over SSE, level filter, search | A-ui | todo | |  |
| download log files | A-ui | todo | |  |

#### Notification inbox (`/notifications`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| list, mark read, delete | A-ui | todo | |  |

#### Settings → Application

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| version, update check, log level and settings, check for updates toggle | A-ui | todo | |  |

#### Settings → Logs

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| log level, max size, backups, files | A-ui | todo | |  |

#### Settings → Indexers

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, toggle, delete, test API | A-ui | todo | |  |

#### Settings → IRC

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| networks with live status, expand channels | A-ui | todo | |  |
| add, edit, delete, restart network | A-ui | todo | |  |
| channel history, send command, manual announce | A-ui | todo | |  |

#### Settings → Lists

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, refresh, delete; webhook trigger URLs | A-ui | todo | |  |

#### Settings → Feeds

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, toggle, force run, clear cache, latest run, delete | A-ui | todo | |  |

#### Settings → Clients

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, test, delete (each client type form) | A-ui | todo | |  |

#### Settings → Notifications

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, test, delete | A-ui | todo | |  |

#### Settings → API keys

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, delete, copy | A-ui | todo | |  |

#### Settings → Proxies

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| add, edit, test, delete, usage | A-ui | todo | |  |

#### Settings → Releases

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| delete releases (age, indexer, status) | A-ui | todo | |  |
| duplicate profiles | A-ui | todo | |  |
| cleanup jobs | A-ui | todo | |  |
| regex playground | A-ui | todo | |  |

#### Settings → Account

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| change username and password | A-ui | todo | |  |

#### Everywhere

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| mobile layout | A-ui | todo | |  |
| light/dark (rethemed to Encore) | A-ui | todo | |  |
| toasts | A-ui | todo | |  |

### Background jobs

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Feed refresh per feed (its interval, anchored to last run) | A-api | todo | |  |
| Feed cache cleanup (daily 03:05) | A-api | todo | |  |
| List refresh (every 6 h) | A-api | todo | |  |
| Release cleanup jobs (their cron) | A-api | todo | |  |
| Update check (every 2 h) | A-api | todo | |  |
| Temp folder cleanup (daily 04:00) | A-api | todo | |  |
| Notification inbox cleanup (every hour) | A-api | todo | |  |
| IRC health monitor and reconnect | A-api | todo | |  |
| Max downloads per period counting | A-api | partial | | fresh version |

### CLI (`autobrrctl`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `version` | A-api | todo | |  |
| `create-user` | A-api | todo | |  |
| `change-password` | A-api | todo | |  |
| `db:convert` | A-api | todo | |  |
| `db:seed` | A-api | todo | |  |
| `htpasswd` | A-api | todo | |  |
| `export-filters` | A-api | todo | |  |

## cross-seed v6.13.9

cross-seed has no web UI in v6; its daemon, API and CLI are ported. Its settings become a Lantern settings page (track L).

### API endpoints

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `POST /api/webhook` (search one torrent by infoHash or path) | X | partial | | fresh version |
| `POST /api/announce` (match an announce, e.g. from autobrr) | X | todo | |  |
| `POST /api/job` (run a job now) | X | todo | |  |
| `GET /api/ping` | X | todo | |  |
| `GET /api/status` | X | todo | |  |
| API key in `X-Api-Key` header or `apikey` query | X | todo | |  |

### Settings (`config.js`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |

### Matching and injection

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Match modes `strict`, `flexible`, `partial` | X | partial | | strict only |
| Decisions: `MATCH`, `MATCH_SIZE_ONLY`, `MATCH_PARTIAL`, `FUZZY_SIZE_MISMATCH`, `SIZE_MISMATCH`, `PARTIAL_SIZE_MISMATCH`, `NO_DOWNLOAD_LINK`, `DOWNLOAD_FAILED`, `MAGNET_LINK`, `RATE_LIMITED`, `SAME_INFO_HASH`, `INFO_HASH_ALREADY_EXISTS`, `FILE_TREE_MISMATCH`, `RELEASE_GROUP_MISMATCH`, `BLOCKED_RELEASE`, `PROPER_REPACK_MISMATCH`, `RESOLUTION_MISMATCH`, `SOURCE_MISMATCH` | X | todo | |  |
| Actions `save` and `inject`; injection results | X | todo | |  |
| Link types `symlink`, `hardlink`, `reflink`, `reflinkOrCopy`; `flatLinking` | X | todo | |  |
| Block list types (name, regex, folder, category, tag, tracker, infoHash, size) | X | todo | |  |
| Clients: qBittorrent, rTorrent, Transmission, Deluge (several at once) | X | todo | |  |
| Data-based searching (`dataDirs`), season packs from episodes | X | todo | |  |
| Pre-filter rules (age, recent search, ignored titles, non-video, single episodes) | X | todo | |  |
| Arr lookups (Sonarr/Radarr ids for better searching) | X | todo | |  |
| Recheck and auto-resume rules | X | todo | |  |
| Notification webhooks | X | todo | |  |
| Torrent cache and client torrent cache | X | todo | |  |

### Background jobs

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `rss` (its cadence) | X | todo | |  |
| `search` (its cadence) | X | todo | |  |
| `updateIndexerCaps` (daily) | X | todo | |  |
| `inject` saved torrents (hourly) | X | todo | |  |
| `cleanup` database (daily) | X | todo | |  |

### CLI

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `daemon` | X | todo | |  |
| `rss` | X | todo | |  |
| `search` | X | todo | |  |
| `inject` | X | todo | |  |
| `restore` | X | todo | |  |
| `gen-config` | X | todo | |  |
| `update-torrent-cache-trackers` | X | todo | |  |
| `diff` | X | todo | |  |
| `tree` | X | todo | |  |
| `clear-indexer-failures` | X | todo | |  |
| `clear-cache` | X | todo | |  |
| `clear-client-cache` | X | todo | |  |
| `api-key` | X | todo | |  |
| `reset-api-key` | X | todo | |  |

## FlareSolverr v3.5.2

### API endpoints

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `GET /` (version and user agent) | F | partial | | fresh version |
| `GET /health` | F | partial | | fresh version |
| `POST /v1` `request.get` | F | partial | | fresh version |
| `POST /v1` `request.post` | F | partial | | fresh version |
| `POST /v1` `sessions.create` | F | todo | |  |
| `POST /v1` `sessions.list` | F | todo | |  |
| `POST /v1` `sessions.destroy` | F | todo | |  |

### Request fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `cmd` | F | todo | |  |
| `url` | F | todo | |  |
| `session` | F | todo | |  |
| `session_ttl_minutes` | F | todo | |  |
| `maxTimeout` | F | todo | |  |
| `cookies` | F | todo | |  |
| `proxy (url, username, password)` | F | todo | |  |
| `headers` | F | todo | |  |
| `userAgent` | F | todo | |  |
| `postData` | F | todo | |  |
| `returnOnlyCookies` | F | todo | |  |
| `returnScreenshot` | F | todo | |  |
| `returnRawHtml` | F | todo | |  |
| `download` | F | todo | |  |
| `waitInSeconds` | F | todo | |  |
| `disableMedia` | F | todo | |  |
| `tabs_till_verify` | F | todo | |  |

### Answer fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `status` | F | todo | |  |
| `message` | F | todo | |  |
| `startTimestamp` | F | todo | |  |
| `endTimestamp` | F | todo | |  |
| `version` | F | todo | |  |
| `solution.url` | F | todo | |  |
| `solution.status` | F | todo | |  |
| `solution.headers` | F | todo | |  |
| `solution.response` | F | todo | |  |
| `solution.cookies` | F | todo | |  |
| `solution.userAgent` | F | todo | |  |
| `solution.screenshot` | F | todo | |  |
| `solution.turnstile_token` | F | todo | |  |
| `session` | F | todo | |  |
| `sessions` | F | todo | |  |

### Settings (environment)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `HOST` | F | todo | |  |
| `PORT` | F | todo | |  |
| `LOG_LEVEL` | F | todo | |  |
| `LOG_FILE` | F | todo | |  |
| `LOG_HTML` | F | todo | |  |
| `HEADLESS` | F | todo | |  |
| `BROWSER_WAIT_TIMEOUT` | F | todo | |  |
| `DISABLE_MEDIA` | F | todo | |  |
| `LANG` | F | todo | |  |
| `TZ` | F | todo | |  |
| `PROXY_URL` | F | todo | |  |
| `PROXY_USERNAME` | F | todo | |  |
| `PROXY_PASSWORD` | F | todo | |  |
| `PROMETHEUS_ENABLED` | F | todo | |  |
| `PROMETHEUS_PORT` | F | todo | |  |

### Challenges

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Cloudflare JS challenge | F | partial | | fresh version |
| DDoS-Guard | F | partial | | fresh version |
| Cloudflare Turnstile (click and token) | F | todo | |  |
| Access denied / banned detection and messages | F | todo | |  |
| Prometheus metrics | F | todo | |  |

## Lantern's own pages

Kept where the merge needs them; they follow the same rule (done with a test or screenshot).

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Download hub (apps, clients, grabs, Soulseek) | L | partial | | own page |
| Account vault | L | partial | | own page |
| Account keeper | L | partial | | own page |
| Cloudflare solving | L | partial | | own page |

