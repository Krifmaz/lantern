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
| `GET /api/v1/applications/{id}` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `PUT /api/v1/applications/{id}` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `DELETE /api/v1/applications/{id}` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `GET /api/v1/applications` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `POST /api/v1/applications` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `PUT /api/v1/applications/bulk` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `DELETE /api/v1/applications/bulk` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `GET /api/v1/applications/schema` | P-app | done | `server/tests/prowlarr_apps.rs::the_application_api_follows_prowlarrs_provider_controller` | ported (`prowlarr/apps/provider.rs`, `applications.rs`); `apps_schema_sonarr.json` |
| `POST /api/v1/applications/test` | P-app | done | `server/tests/prowlarr_apps.rs::an_old_sonarr_or_a_wrong_key_fails_the_test_with_prowlarrs_messages` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `POST /api/v1/applications/testall` | P-app | done | `server/tests/prowlarr_apps.rs::an_old_sonarr_or_a_wrong_key_fails_the_test_with_prowlarrs_messages` | ported (`prowlarr/apps/provider.rs`, `applications.rs`) |
| `POST /api/v1/applications/action/{name}` | P-app | done | `server/tests/prowlarr_apps.rs::an_old_sonarr_or_a_wrong_key_fails_the_test_with_prowlarrs_messages` | ported (`prowlarr/apps/provider.rs`, `applications.rs`); apps have no actions (null) |
| `POST /api/v1/appprofile` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported (`prowlarr/apps/profiles.rs`) |
| `GET /api/v1/appprofile` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported (`prowlarr/apps/profiles.rs`) |
| `DELETE /api/v1/appprofile/{id}` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported (`prowlarr/apps/profiles.rs`) |
| `PUT /api/v1/appprofile/{id}` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported (`prowlarr/apps/profiles.rs`) |
| `GET /api/v1/appprofile/{id}` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported (`prowlarr/apps/profiles.rs`) |
| `GET /api/v1/appprofile/schema` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported (`prowlarr/apps/profiles.rs`) |
| `POST /login` | P-ui | todo | |  |
| `GET /login` | P-ui | todo | |  |
| `GET /logout` | P-ui | todo | |  |
| `GET /api/v1/system/backup` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/system.rs`); updates come from Lantern's release notes |
| `DELETE /api/v1/system/backup/{id}` | P-api | todo | |  |
| `POST /api/v1/system/backup/restore/{id}` | P-api | todo | |  |
| `POST /api/v1/system/backup/restore/upload` | P-api | todo | |  |
| `GET /api/v1/command/{id}` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported queue (`prowlarr/commands.rs`); commands Lantern can't run yet answer as unknown |
| `DELETE /api/v1/command/{id}` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported queue (`prowlarr/commands.rs`); commands Lantern can't run yet answer as unknown |
| `POST /api/v1/command` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported queue (`prowlarr/commands.rs`); commands Lantern can't run yet answer as unknown |
| `GET /api/v1/command` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported queue (`prowlarr/commands.rs`); commands Lantern can't run yet answer as unknown |
| `GET /api/v1/customfilter/{id}` | P-api | done | `server/tests/prowlarr_api.rs::custom_filters` | ported (`prowlarr/filters.rs`) |
| `PUT /api/v1/customfilter/{id}` | P-api | done | `server/tests/prowlarr_api.rs::custom_filters` | ported (`prowlarr/filters.rs`) |
| `DELETE /api/v1/customfilter/{id}` | P-api | done | `server/tests/prowlarr_api.rs::custom_filters` | ported (`prowlarr/filters.rs`) |
| `GET /api/v1/customfilter` | P-api | done | `server/tests/prowlarr_api.rs::custom_filters` | ported (`prowlarr/filters.rs`) |
| `POST /api/v1/customfilter` | P-api | done | `server/tests/prowlarr_api.rs::custom_filters` | ported (`prowlarr/filters.rs`) |
| `PUT /api/v1/config/development/{id}` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported (`prowlarr/config.rs`) |
| `GET /api/v1/config/development/{id}` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported (`prowlarr/config.rs`) |
| `GET /api/v1/config/development` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported (`prowlarr/config.rs`) |
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
| `GET /api/v1/health` | P-api | partial | `server/tests/prowlarr_api.rs::localization_and_health` | ported answer; 4 of Prowlarr's 20 checks so far |
| `GET /api/v1/history` | P-api | todo | |  |
| `GET /api/v1/history/since` | P-api | todo | |  |
| `GET /api/v1/history/indexer` | P-api | todo | |  |
| `GET /api/v1/config/host/{id}` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks`, `server/src/prowlarr/config.rs::tests` | ported (`prowlarr/config.rs`) |
| `PUT /api/v1/config/host/{id}` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks`, `server/src/prowlarr/config.rs::tests` | ported (`prowlarr/config.rs`) |
| `GET /api/v1/config/host` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks`, `server/src/prowlarr/config.rs::tests` | ported (`prowlarr/config.rs`) |
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
| `GET /api/v1/indexerproxy/{id}` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` (fixture `indexerproxy.json`, 404) | `server/src/indexers/proxies_api.rs` |
| `PUT /api/v1/indexerproxy/{id}` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` (test unless `forceSave`, 202, id checks) | `server/src/indexers/proxies_api.rs` |
| `DELETE /api/v1/indexerproxy/{id}` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` | `server/src/indexers/proxies_api.rs` |
| `GET /api/v1/indexerproxy` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` | `server/src/indexers/proxies_api.rs` |
| `POST /api/v1/indexerproxy` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` (201 + Location, unique name, settings failures as `indexerproxy_invalid.json`) | `server/src/indexers/proxies_api.rs` |
| `GET /api/v1/indexerproxy/schema` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` (fixture `indexerproxy_schema.json`) | `server/src/indexers/proxies_api.rs` |
| `POST /api/v1/indexerproxy/test` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr`; `server/src/indexers/proxies.rs` tests | pings `https://prowlarr.servarr.com/v1/ping` through the proxy, as Prowlarr |
| `POST /api/v1/indexerproxy/testall` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` | `server/src/indexers/proxies_api.rs` |
| `POST /api/v1/indexerproxy/action/{name}` | P-idx | done | `server/tests/indexers_proxies.rs` `indexer_proxy_api_answers_like_prowlarr` (answers `null`) | `server/src/indexers/proxies_api.rs` |
| `GET /api/v1/indexerstats` | P-api | partial | | fresh version |
| `GET /api/v1/indexerstatus` | P-api | todo | |  |
| `GET /api/v1/localization` | P-api | done | `server/tests/prowlarr_api.rs::localization_and_health`, `server/src/prowlarr/localization.rs::tests` | ported; Prowlarr's strings copied as data (GPL-3.0) |
| `GET /api/v1/localization/options` | P-api | done | `server/tests/prowlarr_api.rs::localization_and_health`, `server/src/prowlarr/localization.rs::tests` | ported; Prowlarr's strings copied as data (GPL-3.0) |
| `GET /api/v1/log` | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported answers (`prowlarr/logs.rs`); nothing writes the log table or files yet |
| `GET /api/v1/log/file` | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported answers (`prowlarr/logs.rs`); nothing writes the log table or files yet |
| `GET /api/v1/log/file/{filename}` | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported answers (`prowlarr/logs.rs`); nothing writes the log table or files yet |
| `GET /api/v1/indexer/{id}/newznab` | P-idx | done | `server/tests/indexers_prowlarr.rs` `the_api_key_parameters_and_the_test_indexer_answer_like_prowlarr` | ported |
| `GET /{id}/api` | P-idx | done | `server/tests/indexers_prowlarr.rs` `the_api_key_parameters_and_the_test_indexer_answer_like_prowlarr`, `searches_follow_prowlarr_limits_holds_and_paging` | ported |
| `GET /api/v1/indexer/{id}/download` | P-idx | done | `server/tests/indexers_prowlarr.rs` `downloads_follow_prowlarr_grab_limits_redirects_and_file_names` | ported |
| `GET /{id}/download` | P-idx | done | `server/tests/indexers_prowlarr.rs` `downloads_follow_prowlarr_grab_limits_redirects_and_file_names`, `magnet_links_go_through_lantern_and_can_come_first` | ported |
| `GET /api/v1/notification/{id}` | P-app | todo | |  |
| `PUT /api/v1/notification/{id}` | P-app | todo | |  |
| `DELETE /api/v1/notification/{id}` | P-app | todo | |  |
| `GET /api/v1/notification` | P-app | todo | |  |
| `POST /api/v1/notification` | P-app | todo | |  |
| `GET /api/v1/notification/schema` | P-app | todo | |  |
| `POST /api/v1/notification/test` | P-app | todo | |  |
| `POST /api/v1/notification/testall` | P-app | todo | |  |
| `POST /api/v1/notification/action/{name}` | P-app | todo | |  |
| `GET /ping` | P-api | done | `server/tests/prowlarr_api.rs::ping_and_status` | ported (`prowlarr/system.rs`); appName is Lantern |
| `HEAD /ping` | P-api | done | `server/tests/prowlarr_api.rs::ping_and_status` | ported (`prowlarr/system.rs`); appName is Lantern |
| `POST /api/v1/search` | P-api | todo | |  |
| `GET /api/v1/search` | P-api | partial | | fresh version |
| `POST /api/v1/search/bulk` | P-api | todo | |  |
| `GET /content/{path}` | P-ui | partial | `server/tests/prowlarr_ui.rs::only_content_is_open_without_signing_in`, `::the_manifest_names_the_instance_like_manifest_mapper` | the built files, open to everyone, served at `/prowlarr/Content/` for now (moves to `/` later) |
| `GET /` | P-ui | partial | | fresh version |
| `GET /{path}` | P-ui | partial | `server/tests/prowlarr_ui.rs::every_page_is_the_frontend_under_prowlarr`, `::the_page_gets_the_url_base_like_html_mapper_base`, `::pages_are_paths_without_a_dot_outside_content`, `::initialize_json_has_prowlarrs_fields_with_lanterns_api` | Prowlarr's frontend with its URL base set to `/prowlarr` (moves to `/` later); no `?h=` cache breaker |
| `GET /api/v1/system/status` | P-api | done | `server/tests/prowlarr_api.rs::ping_and_status` | ported (`prowlarr/system.rs`); appName is Lantern |
| `GET /api/v1/system/routes` | P-api | done | `server/tests/prowlarr_api.rs::ping_and_status` | ported (`prowlarr/system.rs`); appName is Lantern |
| `GET /api/v1/system/routes/duplicate` | P-api | todo | |  |
| `POST /api/v1/system/shutdown` | P-api | todo | |  |
| `POST /api/v1/system/restart` | P-api | todo | |  |
| `GET /api/v1/tag/{id}` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `PUT /api/v1/tag/{id}` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `DELETE /api/v1/tag/{id}` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `GET /api/v1/tag` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `POST /api/v1/tag` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `GET /api/v1/tag/detail/{id}` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `GET /api/v1/tag/detail` | P-api | done | `server/tests/prowlarr_api.rs::tags_and_their_details` | ported (`prowlarr/tags.rs`); indexer tags are kept in step |
| `GET /api/v1/system/task` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/system.rs`); updates come from Lantern's release notes |
| `GET /api/v1/system/task/{id}` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/system.rs`); updates come from Lantern's release notes |
| `PUT /api/v1/config/ui/{id}` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported (`prowlarr/config.rs`) |
| `GET /api/v1/config/ui/{id}` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported (`prowlarr/config.rs`) |
| `GET /api/v1/config/ui` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported (`prowlarr/config.rs`) |
| `GET /api/v1/update` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/system.rs`); updates come from Lantern's release notes |
| `GET /api/v1/log/file/update` | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported answers (`prowlarr/logs.rs`); nothing writes the log table or files yet |
| `GET /api/v1/log/file/update/{filename}` | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported answers (`prowlarr/logs.rs`); nothing writes the log table or files yet |
| Newznab/Torznab `t=caps` (all of Prowlarr's parameters and answer fields) | P-idx | done | `server/tests/indexers_prowlarr.rs` `caps_of_the_test_indexer_match_prowlarr`, `caps_of_a_cardigann_definition_match_prowlarr`; `server/tests/indexers_proxies.rs` `newznab_capabilities_come_from_the_api_once`; `server/src/indexers/newznab_caps.rs` (Prowlarr's `NewznabCapabilitiesProviderFixture`) | generic Newznab/Torznab indexers read the remote caps (kept 7 days, stored with the indexer) |
| Newznab/Torznab `t=search` (all of Prowlarr's parameters and answer fields) | P-idx | done | `server/tests/indexers_prowlarr.rs` `search_answers_match_prowlarr`, `searches_follow_prowlarr_limits_holds_and_paging` | ported |
| Newznab/Torznab `t=tvsearch` (all of Prowlarr's parameters and answer fields) | P-idx | done | `server/src/indexers/newznab_request.rs` `reads_parameters_like_aspnet`, `should_normalize_imdbid_search_criteria`; `server/tests/indexers_prowlarr.rs` `searches_follow_prowlarr_limits_holds_and_paging` | ported |
| Newznab/Torznab `t=movie` (all of Prowlarr's parameters and answer fields) | P-idx | done | `server/src/indexers/newznab_request.rs` `should_normalize_imdbid_search_criteria`, `query_to_params_takes_ids_out_of_the_words` | ported |
| Newznab/Torznab `t=music` (all of Prowlarr's parameters and answer fields) | P-idx | done | `server/src/indexers/newznab_request.rs` `music_and_book_searches_take_every_prowlarr_parameter` | ported |
| Newznab/Torznab `t=book` (all of Prowlarr's parameters and answer fields) | P-idx | done | `server/src/indexers/newznab_request.rs` `music_and_book_searches_take_every_prowlarr_parameter` | ported |
| Newznab/Torznab errors (codes 100–910 as Prowlarr answers them) | P-idx | done | `server/tests/indexers_prowlarr.rs` `errors_match_prowlarr`, `the_api_key_parameters_and_the_test_indexer_answer_like_prowlarr` | ported (Prowlarr answers 200, 201, 202, 410, 429 and an empty 401) |
| `apikey` in query or `X-Api-Key` header; `/api/v1/…` behind forms/basic/external auth | P-api | done | `server/tests/prowlarr_api.rs::the_key_or_a_session_is_needed_like_prowlarr` | ported (`prowlarr::V1`); forms means Lantern's own sign-in |
| SignalR `/signalr/messages` (live updates the web UI listens to) | P-api | todo | | |

### Settings

#### General (config/host)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `BindAddress` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `Port` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | read only (APP_PORT) |
| `SslPort` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `EnableSsl` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `LaunchBrowser` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `AuthenticationMethod` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `AuthenticationRequired` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `AllowedHosts` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `AnalyticsEnabled` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `Username` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; renames the owner |
| `Password` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; sets the owner's password (shown masked) |
| `PasswordConfirmation` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; checked with Password |
| `LogLevel` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `LogSizeLimit` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `ConsoleLogLevel` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `Branch` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `ApiKey` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's API key; changed by ResetApiKey |
| `SslCertPath` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `SslCertPassword` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `UrlBase` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `TrustedNetworks` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `InstanceName` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's app name |
| `ApplicationUrl` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's public address |
| `UpdateAutomatically` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `UpdateMechanism` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `UpdateScriptPath` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `ProxyEnabled` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's site proxy |
| `ProxyType` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's site proxy |
| `ProxyHostname` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's site proxy |
| `ProxyPort` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's site proxy |
| `ProxyUsername` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's site proxy |
| `ProxyPassword` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; Lantern's site proxy |
| `ProxyBypassFilter` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `ProxyBypassLocalAddresses` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `CertificateValidation` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `BackupFolder` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `BackupInterval` | P-api | done | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | ported; the backup task's interval |
| `BackupRetention` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `HistoryCleanupDays` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |
| `TrustCgnatIpAddresses` | P-api | partial | `server/tests/prowlarr_api.rs::host_settings_have_prowlarrs_fields_defaults_and_checks` | stored and checked like Prowlarr; not acted on yet |

#### UI (config/ui)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `FirstDayOfWeek` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `CalendarWeekColumnHeader` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `ShortDateFormat` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `LongDateFormat` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `TimeFormat` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `ShowRelativeDates` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `EnableColorImpairedMode` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `UILanguage` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |
| `Theme` | P-api | done | `server/tests/prowlarr_api.rs::ui_and_development_settings` | ported; read by the web app |

#### Development (config/development)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `ConsoleLogLevel` | P-api | partial | `server/tests/prowlarr_api.rs::ui_and_development_settings` | stored like Prowlarr; not acted on yet |
| `LogSql` | P-api | partial | `server/tests/prowlarr_api.rs::ui_and_development_settings` | stored like Prowlarr; not acted on yet |
| `LogIndexerResponse` | P-api | partial | `server/tests/prowlarr_api.rs::ui_and_development_settings` | stored like Prowlarr; not acted on yet |
| `LogRotate` | P-api | partial | `server/tests/prowlarr_api.rs::ui_and_development_settings` | stored like Prowlarr; not acted on yet |
| `FilterSentryEvents` | P-api | partial | `server/tests/prowlarr_api.rs::ui_and_development_settings` | stored like Prowlarr; not acted on yet |

#### Indexer (every indexer, besides its definition's own fields)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `QueryLimit` | P-idx | done | `server/tests/indexers_prowlarr.rs` `searches_follow_prowlarr_limits_holds_and_paging`; `server/src/indexers/limits.rs` | `baseSettings.queryLimit` |
| `GrabLimit` | P-idx | done | `server/tests/indexers_prowlarr.rs` `downloads_follow_prowlarr_grab_limits_redirects_and_file_names` | `baseSettings.grabLimit` |
| `LimitsUnit` | P-idx | done | `server/tests/indexers_prowlarr.rs` `searches_follow_prowlarr_limits_holds_and_paging` (hour), `downloads_follow_prowlarr_grab_limits_redirects_and_file_names` (day) | `baseSettings.limitsUnit` |
| `AppMinimumSeeders` | P-idx | partial | `server/src/indexers/limits.rs` `reads_and_checks_the_limits` | stored and checked; used when apps sync (P-app) |
| `SeedRatio` | P-idx | partial | `server/src/indexers/limits.rs` `reads_and_checks_the_limits`, `seed_goals_follow_seed_config_provider` | stored; `SeedConfigProvider` ported (`Indexers::seed_configuration`); handed to download clients when grabs go through Prowlarr's download clients (P-app) |
| `SeedTime` | P-idx | partial | `server/src/indexers/limits.rs` `reads_and_checks_the_limits`, `seed_goals_follow_seed_config_provider` | stored; `SeedConfigProvider` ported (`Indexers::seed_configuration`); handed to download clients when grabs go through Prowlarr's download clients (P-app) |
| `PackSeedTime` | P-idx | partial | `server/src/indexers/limits.rs` `reads_and_checks_the_limits` | stored; used when apps sync (P-app) |
| `PreferMagnetUrl` | P-idx | done | `server/tests/indexers_prowlarr.rs` `magnet_links_go_through_lantern_and_can_come_first` | `torrentBaseSettings.preferMagnetUrl` |
| `Enable` | P-idx | done | `server/tests/indexers_prowlarr.rs` `the_api_key_parameters_and_the_test_indexer_answer_like_prowlarr` (410 Indexer is disabled) | `enabled` |
| `Priority` | P-idx | done | `server/tests/indexers_prowlarr.rs` `a_release_two_indexers_found_is_kept_from_the_higher_priority` | `priority` |
| `Tags` | P-idx | done | `server/tests/indexers_proxies.rs` `tags_app_profile_and_download_client_are_kept_in_prowlarrs_shape` | stored as labels, answered as Prowlarr tag ids |
| `AppProfileId` | P-idx | partial | `server/tests/indexers_proxies.rs` `tags_app_profile_and_download_client_are_kept_in_prowlarrs_shape` | stored, default 1, `ValidId` checked; "App Profile does not exist" checked once app profiles have a table (P-app) |
| `DownloadClientId` | P-idx | done | `server/tests/indexers_proxies.rs` `tags_app_profile_and_download_client_are_kept_in_prowlarrs_shape` | stored, default 0, must name a download client |
| `Redirect` | P-idx | done | `server/tests/indexers_prowlarr.rs` `downloads_follow_prowlarr_grab_limits_redirects_and_file_names` | `redirect` |

#### Applications (per app)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `SyncLevel` (Disabled, Add and Remove Only, Full Sync) | P-app | done | `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step`, `server/tests/prowlarr_apps.rs::radarr_gets_its_first_sync_fields_and_add_only_skips_changes` | ported (`prowlarr/apps/sync.rs`) |
| `ProwlarrUrl`, `BaseUrl`, `ApiKey` | P-app | done | `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step` | ported; the pushed indexer's address is `<ProwlarrUrl>/<id>/` |
| `SyncCategories` (and anime/other categories per app) | P-app | done | `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step`, `server/tests/prowlarr_apps.rs::radarr_gets_its_first_sync_fields_and_add_only_skips_changes` | ported (`prowlarr/apps/categories.rs`) |
| `SyncRejectBlocklistedTorrentHashesWhileGrabbing` (Sonarr/Radarr) | P-app | done | `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step`, `server/tests/prowlarr_apps.rs::radarr_gets_its_first_sync_fields_and_add_only_skips_changes` | ported |
| `Tags` (sync only matching indexers) | P-app | done | `server/tests/prowlarr_apps.rs::an_app_with_tags_gets_only_the_indexers_that_share_one` | ported; Lantern's indexer tags are matched by label |

#### App profiles

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `Name` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr` | ported |
| `EnableRss` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr`, `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step` | ported |
| `EnableAutomaticSearch` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr`, `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step` | ported |
| `EnableInteractiveSearch` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr`, `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step` | ported |
| `MinimumSeeders` | P-app | done | `server/tests/prowlarr_apps.rs::app_profiles_follow_prowlarr`, `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step` | ported |

#### Indexer proxies

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| FlareSolverr (`Host`, `RequestTimeout`) | P-idx | done | `server/tests/indexers_proxies.rs` `flaresolverr_is_asked_only_for_a_cloudflare_check`; `server/src/indexers/proxies.rs` (Cloudflare detection, commands, answers) | asked only after a Cloudflare check; Lantern's own solver answers in-process when `Host` points at it |
| HTTP (`Host`, `Port`, `Username`, `Password`) | P-idx | done | `server/tests/indexers_proxies.rs` `indexers_go_through_the_proxies_that_share_their_tags` | `server/src/indexers/proxies.rs` |
| SOCKS4 | P-idx | done | `server/tests/indexers_proxies.rs` `indexers_go_through_the_proxies_that_share_their_tags` | `server/src/indexers/proxies.rs` |
| SOCKS5 | P-idx | done | `server/tests/indexers_proxies.rs` `indexers_go_through_the_proxies_that_share_their_tags` (user and password, remote DNS) | `server/src/indexers/proxies.rs` |
| `Tags` (which indexers use it) | P-idx | done | `server/tests/indexers_proxies.rs` `indexers_go_through_the_proxies_that_share_their_tags`; `server/src/indexers/proxies.rs` `indexers_take_one_proxy_and_one_flaresolverr_sharing_a_tag` | `IndexerHttpClient.GetProxies` ported |

### Provider types

#### Applications

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| LazyLibrarian | P-app | partial | `server/src/prowlarr/apps/nab.rs::tests` | ported (`prowlarr/apps/nab.rs`); no stand-in LazyLibrarian test yet |
| Lidarr | P-app | partial |  | ported (`prowlarr/apps/arr.rs`, same code as Sonarr/Radarr); no stand-in Lidarr test yet |
| Mylar | P-app | partial | `server/src/prowlarr/apps/nab.rs::tests` | ported (`prowlarr/apps/nab.rs`); no stand-in Mylar test yet |
| Radarr | P-app | done | `server/tests/prowlarr_apps.rs::radarr_gets_its_first_sync_fields_and_add_only_skips_changes` | ported (`prowlarr/apps/arr.rs`) |
| Readarr | P-app | partial |  | ported (`prowlarr/apps/arr.rs`, same code as Sonarr/Radarr); no stand-in Readarr test yet |
| Sonarr | P-app | done | `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step`, `scripts/contract-arrs.sh` (real Sonarr) | ported (`prowlarr/apps/arr.rs`) |
| Whisparr | P-app | partial |  | ported (`prowlarr/apps/arr.rs`, same code as Sonarr/Radarr); no stand-in Whisparr test yet |

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
| Indexer search limits (`QueryLimit`, `GrabLimit`, per hour/day) and per-site rate limits (`RequestDelay`) | P-idx | done | `server/tests/indexers_prowlarr.rs` `searches_follow_prowlarr_limits_holds_and_paging`, `downloads_follow_prowlarr_grab_limits_redirects_and_file_names`; `server/tests/indexers_proxies.rs` `requests_to_one_site_wait_for_the_request_delay`; `server/src/indexers/rate_limit.rs` (Prowlarr's `RateLimitServiceFixture`) | per-site wait shared by host, 2 s floor (not for local sites) |
| Health: disable after failures with Prowlarr's back-off steps, `IndexerStatus` escalation | P-idx | done | `server/src/indexers/status.rs` (ported ProviderStatusServiceFixture, IndexerStatusServiceFixture); `server/tests/indexers_prowlarr.rs` `searches_follow_prowlarr_limits_holds_and_paging` | ported |
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
| `MessagingCleanupCommand` every 5 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `ClearExpiredCacheCommand` every 5 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `ApplicationUpdateCheckCommand` every 6 * 60 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `CheckHealthCommand` every 6 * 60 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `HousekeepingCommand` every 24 * 60 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `CleanUpHistoryCommand` every 24 * 60 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `IndexerDefinitionUpdateCommand` every 24 * 60 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |
| `ApplicationIndexerSyncCommand` every 6 * 60 min | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; queued every 6 h by the app sync watcher (`prowlarr/apps/sync.rs`, P-app); no test of the schedule yet |
| `BackupCommand` (backup interval setting) | P-api | partial | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | listed with Prowlarr's interval; no scheduler runs it yet |

#### Commands (`POST /api/v1/command`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `ApplicationIndexerSyncCommand` | P-api | done | `server/tests/prowlarr_apps.rs::sonarr_gets_the_indexers_prowlarr_would_push_and_keeps_them_in_step` | ported by P-app (`prowlarr/commands.rs` runs `prowlarr/apps/sync.rs`), with `forceSync` |
| `ApplicationUpdateCheckCommand` | P-api | todo | |  |
| `ApplicationUpdateCommand` | P-api | todo | |  |
| `BackupCommand` | P-api | todo | |  |
| `CheckHealthCommand` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported (`prowlarr/commands.rs`) |
| `CleanupHistoryCommand` | P-api | todo | |  |
| `ClearExpiredCacheCommand` | P-api | todo | |  |
| `ClearHistoryCommand` | P-api | todo | |  |
| `ClearLogCommand` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/commands.rs`) |
| `DeleteLogFilesCommand` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/commands.rs`) |
| `DeleteUpdateLogFilesCommand` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/commands.rs`) |
| `HousekeepingCommand` | P-api | todo | |  |
| `IndexerDefinitionUpdateCommand` | P-api | partial | | runs Lantern's definition update; no test yet (it needs the network) |
| `MessagingCleanupCommand` | P-api | done | `server/tests/prowlarr_api.rs::tasks_logs_backups_and_updates` | ported (`prowlarr/commands.rs`) |
| `ResetApiKeyCommand` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported (`prowlarr/commands.rs`) |
| `RestartCommand` | P-api | todo | |  |
| `ShutdownCommand` | P-api | todo | |  |
| `TestCommand` | P-api | done | `server/tests/prowlarr_api.rs::commands_queue_run_and_report_like_prowlarr` | ported (`prowlarr/commands.rs`) |

#### Health checks

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `AllowedHostsCheck` | P-api | done | `server/tests/prowlarr_api.rs::localization_and_health`, `server/src/prowlarr/health.rs::tests` | ported (`prowlarr/health.rs`) |
| `ApiKeyValidationCheck` | P-api | done | `server/tests/prowlarr_api.rs::localization_and_health`, `server/src/prowlarr/health.rs::tests` | ported (`prowlarr/health.rs`) |
| `AppDataLocationCheck` | P-api | todo | |  |
| `ApplicationLongTermStatusCheck` | P-api | done | `server/tests/prowlarr_apps.rs::apps_held_back_after_failures_show_in_health`, `server/src/prowlarr/apps/status.rs::tests` | ported by P-app (`prowlarr/apps/mod.rs`) |
| `ApplicationStatusCheck` | P-api | done | `server/tests/prowlarr_apps.rs::apps_held_back_after_failures_show_in_health`, `server/src/prowlarr/apps/status.rs::tests` | ported by P-app (`prowlarr/apps/mod.rs`) |
| `DownloadClientStatusCheck` | P-api | todo | |  |
| `IndexerCheck` | P-api | done | `server/tests/prowlarr_api.rs::localization_and_health`, `server/src/prowlarr/health.rs::tests` | ported (`prowlarr/health.rs`) |
| `IndexerDownloadClientCheck` | P-api | todo | |  |
| `IndexerLongTermStatusCheck` | P-api | todo | |  |
| `IndexerNoDefinitionCheck` | P-api | todo | |  |
| `IndexerProxyStatusCheck` | P-api | todo | |  |
| `IndexerStatusCheck` | P-api | done | `server/tests/prowlarr_api.rs::localization_and_health`, `server/src/prowlarr/health.rs::tests` | ported (`prowlarr/health.rs`) |
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
| `GET /api/actions` | A-api | done | server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported; served at `/autobrr/api` |
| `POST /api/actions` | A-api | done | server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported; served at `/autobrr/api` |
| `DELETE /api/actions/{actionID}` | A-api | done | server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported; served at `/autobrr/api` |
| `PUT /api/actions/{actionID}` | A-api | done | server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported; served at `/autobrr/api` |
| `PATCH /api/actions/{actionID}/toggleEnabled` | A-api | done | server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported; served at `/autobrr/api` |
| `GET /api/alerts` | A-api | todo | |  |
| `GET /api/config` | A-api | todo | |  |
| `PATCH /api/config` | A-api | todo | |  |
| `GET /api/downloaders` | A-api | done | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api` |
| `POST /api/downloaders` | A-api | done | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api` |
| `PUT /api/downloaders` | A-api | done | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api` |
| `POST /api/downloaders/test` | A-api | partial | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api`; only the error answer is proven (each client's test is ported, not run against a real client) |
| `GET /api/downloaders/{clientID}` | A-api | done | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api` |
| `DELETE /api/downloaders/{clientID}` | A-api | done | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api` |
| `GET /api/downloaders/{clientID}/arr/tags` | A-api | partial | server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported; served at `/autobrr/api`; only the empty answer for a torrent client is proven |
| `GET /api/filters` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `POST /api/filters` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `POST /api/filters/indexers/prune-deprecated` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules, server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `POST /api/filters/external/test` | A-api | done | server/tests/autobrr_api.rs::external_filter_test_route, server/src/releases/external.rs::tests::test_external_reports | ported; served at `/autobrr/api` |
| `GET /api/filters/{filterID}` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `PUT /api/filters/{filterID}` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `PATCH /api/filters/{filterID}` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `DELETE /api/filters/{filterID}` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `GET /api/filters/{filterID}/duplicate` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `PUT /api/filters/{filterID}/enabled` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `GET /api/filters/{filterID}/notifications` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `PUT /api/filters/{filterID}/notifications` | A-api | done | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | ported; served at `/autobrr/api` |
| `GET /api/feeds` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `POST /api/feeds` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `POST /api/feeds/test` | A-api | done | server/tests/autobrr_feeds.rs::feed_tests_and_caps_go_through_the_feeds_own_client | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `POST /api/feeds/caps` | A-api | done | server/tests/autobrr_feeds.rs::feed_tests_and_caps_go_through_the_feeds_own_client | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `GET /api/feeds/{feedID}` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `PUT /api/feeds/{feedID}` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `DELETE /api/feeds/{feedID}` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `DELETE /api/feeds/{feedID}/cache` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `PATCH /api/feeds/{feedID}/enabled` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `GET /api/feeds/{feedID}/latest` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `POST /api/feeds/{feedID}/forcerun` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `GET /api/feeds/{feedID}/caps` | A-api | done | server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/feed_api.rs`); served at `/autobrr/api` |
| `GET /api/irc` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `POST /api/irc` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces, server/tests/autobrr_irc.rs::a_network_signs_in_with_sasl_and_joins_without_an_invite, server/tests/autobrr_irc.rs::a_ban_stops_the_network_with_the_reason | ported; served at `/autobrr/api` |
| `GET /api/irc/network/{networkID}` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `PUT /api/irc/network/{networkID}` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `DELETE /api/irc/network/{networkID}` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `POST /api/irc/network/{networkID}/cmd` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `POST /api/irc/network/{networkID}/channel` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `GET /api/irc/network/{networkID}/restart` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `GET /api/irc/network/{networkID}/channel/{channel}/history` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `POST /api/irc/network/{networkID}/channel/{channel}/announce/process` | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported; served at `/autobrr/api` |
| `GET /api/indexer/schema` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `POST /api/indexer` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `GET /api/indexer` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `GET /api/indexer/options` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `GET /api/indexer/deprecations` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `DELETE /api/indexer/deprecations/{indexerID}` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `GET /api/indexer/{indexerID}` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `PUT /api/indexer/{indexerID}` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `DELETE /api/indexer/{indexerID}` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `POST /api/indexer/{indexerID}/api/test` | A-api | partial | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api`; the per-site API clients aren't ported yet, so a site with an API answers autobrr's "could not init api client" |
| `PATCH /api/indexer/{indexerID}/enabled` | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | ported; served at `/autobrr/api` |
| `GET /api/lists` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `POST /api/lists` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `POST /api/lists/refresh` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `POST /api/lists/{listID}/refresh` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `PUT /api/lists/{listID}` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `DELETE /api/lists/{listID}` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
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
| `POST /api/webhook/lists/trigger` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `POST /api/webhook/lists/trigger/arr` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `POST /api/webhook/lists/trigger/lists` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `GET /api/webhook/lists/trigger` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `GET /api/webhook/lists/trigger/arr` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `GET /api/webhook/lists/trigger/lists` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `POST /api/webhook/lists/trigger/{listID}` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported (`releases/list_api.rs`); served at `/autobrr/api` |
| `GET /api/auth/oidc/config` | A-api | todo | |  |
| `GET /api/auth/oidc/callback` | A-api | todo | |  |
| GET /api/events (server-sent events: IRC, logs) | A-api | partial | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | the `irc` stream (also at `/irc/events`); the `logs` stream comes with the logs slice |
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
| `Name` | A-api | done | server/tests/upstream_autobrr.rs::filter_validation | ported (autobrr `Validate`) |
| `Enabled` | A-api | partial | | stored; only enabled filters are checked (no upstream test) |
| `MinSize` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter, server/tests/upstream_autobrr.rs::filter_check_size | ported (go-humanize sizes; read from the torrent when not announced) |
| `MaxSize` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter, server/tests/upstream_autobrr.rs::filter_check_size | ported |
| `Delay` | A-api | partial | | stored; waits before the actions (no upstream test) |
| `Priority` | A-api | partial | | stored; filters are checked highest first (no upstream test) |
| `MaxDownloads` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1, server/tests/upstream_autobrr.rs::filter_validate_max_downloads | ported; counted from the release log |
| `MaxDownloadsUnit` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1, server/tests/upstream_autobrr.rs::filter_download_period | ported |
| `MaxDownloadsPeriod` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1, server/tests/upstream_autobrr.rs::filter_download_period | ported |
| `MaxDownloadsWindowType` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1, server/tests/releases_filter_fields.rs::download_windows | ported (SQLite window starts) |
| `MatchReleases` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptReleases` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `UseRegex` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter, server/tests/upstream_autobrr.rs::filter_list_helpers | ported (go-andiamo splitter, Go regex syntax) |
| `MatchReleaseGroups` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptReleaseGroups` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `AnnounceTypes` | A-api | done | server/tests/releases_filter_fields.rs::announce_types_bonus_and_freeleech_percent | ported |
| `Scene` | A-api | partial | | stored and sent back; autobrr's check doesn't read it either (origins do) |
| `Origins` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `ExceptOrigins` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `Bonus` | A-api | done | server/tests/releases_filter_fields.rs::announce_types_bonus_and_freeleech_percent | ported |
| `Freeleech` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `FreeleechPercent` | A-api | done | server/tests/releases_filter_fields.rs::announce_types_bonus_and_freeleech_percent | ported |
| `SmartEpisode` | A-api | partial | | stored; the newer-episode check needs the release history (next slice) |
| `Shows` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Seasons` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Episodes` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Resolutions` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Codecs` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Sources` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Containers` | A-api | done | server/tests/releases_filter_fields.rs::containers_and_languages | ported |
| `MatchHDR` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptHDR` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `MatchOther` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `ExceptOther` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `Years` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Months` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Days` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Artists` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Albums` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `MatchReleaseTypes` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptReleaseTypes` | A-api | partial | | stored; autobrr's check doesn't read it either |
| `Formats` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Quality` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Media` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `PerfectFlac` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter, server/tests/upstream_autobrr.rs::filter_is_perfect_flac | ported |
| `Cue` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `Log` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `LogScore` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `MatchCategories` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptCategories` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `MatchUploaders` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported; RED/OPS lookup of a missing uploader not yet (rejected with a reason) |
| `ExceptUploaders` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `MatchRecordLabels` | A-api | done | server/tests/releases_filter_fields.rs::record_labels | ported; RED/OPS lookup of a missing label not yet |
| `ExceptRecordLabels` | A-api | done | server/tests/releases_filter_fields.rs::record_labels | ported |
| `MatchLanguage` | A-api | done | server/tests/releases_filter_fields.rs::containers_and_languages | ported |
| `ExceptLanguage` | A-api | done | server/tests/releases_filter_fields.rs::containers_and_languages | ported |
| `Tags` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptTags` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `TagsAny` | A-api | partial | | stored and tidied; autobrr's check doesn't read it either |
| `ExceptTagsAny` | A-api | partial | | stored; autobrr's check doesn't read it either |
| `TagsMatchLogic` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptTagsMatchLogic` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `MatchReleaseTags` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter | ported |
| `ExceptReleaseTags` | A-api | done | server/tests/releases_filter_fields.rs::release_tags_and_descriptions | ported |
| `UseRegexReleaseTags` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `MatchDescription` | A-api | done | server/tests/upstream_autobrr.rs::filter_check_filter1 | ported |
| `ExceptDescription` | A-api | done | server/tests/releases_filter_fields.rs::release_tags_and_descriptions | ported |
| `UseRegexDescription` | A-api | done | server/tests/releases_filter_fields.rs::release_tags_and_descriptions | ported |
| `MinSeeders` | A-api | done | server/tests/releases_filter_fields.rs::seeders_and_leechers | ported |
| `MaxSeeders` | A-api | done | server/tests/releases_filter_fields.rs::seeders_and_leechers | ported |
| `MinLeechers` | A-api | done | server/tests/releases_filter_fields.rs::seeders_and_leechers | ported |
| `MaxLeechers` | A-api | done | server/tests/releases_filter_fields.rs::seeders_and_leechers | ported |
| `IsAutoUpdated` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters | ported; true while a list writes to the filter |
| `ReleaseProfileDuplicateID` | A-api | partial | | stored; duplicate profiles come with the releases slice |
| `DuplicateHandling` | A-api | todo | |  |
| External filters (exec, webhook) with expected status | A-api | done | server/src/releases/external.rs::tests::exec_exit_code_decides, server/src/releases/external.rs::tests::webhook_status_and_retries, server/tests/autobrr_api.rs::a_passing_release_runs_the_filters_autobrr_actions | ported (exec with an argument list; webhook with retries) |
| Filter notifications per event | A-api | partial | server/tests/autobrr_api.rs::filters_keep_autobrrs_shape_and_rules | stored and validated; sending comes with the notifications slice |

### Actions

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `TEST` | A-api | done | server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported |
| `EXEC` | A-api | done | server/src/releases/run.rs::tests::exec_runs_with_an_argument_list, server/src/releases/run.rs::tests::preconditions_skip_download_for_plain_exec | ported; runs with an argument list split like go-shellquote, never a shell |
| `QBITTORRENT` | A-api | partial | server/src/releases/run.rs::tests::option_builders | ported; not yet proven against a stand-in client; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |
| `DELUGE_V1` | A-api | partial | server/src/releases/clients/deluge.rs::tests::answers_and_errors | ported (rencode RPC); not yet proven against a stand-in client; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |
| `DELUGE_V2` | A-api | partial | server/src/releases/clients/deluge.rs::tests::deluge2_request_has_header_and_body | ported (rencode RPC); not yet proven against a stand-in client; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |
| `RTORRENT` | A-api | done | server/src/releases/run.rs::tests::rtorrent_ratio_group_and_priority | ported; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |
| `TRANSMISSION` | A-api | partial |  | ported; not yet proven against a stand-in client; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |
| `PORLA` | A-api | partial |  | ported; not yet proven against a stand-in client |
| `ARIA2` | A-api | done | server/src/releases/clients/aria2.rs::tests::add_torrent, server/src/releases/clients/aria2.rs::tests::add_uri_with_and_without_secret | ported |
| `WATCH_FOLDER` | A-api | done | server/src/releases/run.rs::tests::watch_folder_cases | ported |
| `WEBHOOK` | A-api | done | server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported |
| `RADARR` | A-api | partial |  | ported (same push as Sonarr); not yet proven on its own |
| `SONARR` | A-api | done | server/src/releases/run.rs::tests::sonarr_indexer_not_found_and_found, server/src/releases/run.rs::tests::sonarr_pushes_feed_metadata_and_omits_missing_tvdb | ported |
| `LIDARR` | A-api | partial |  | ported (same push as Sonarr); not yet proven on its own |
| `WHISPARR` | A-api | partial |  | ported (same push as Sonarr); not yet proven on its own |
| `WHISPARR_V3` | A-api | partial |  | ported (same push as Sonarr); not yet proven on its own |
| `READARR` | A-api | partial |  | ported (same push as Sonarr); not yet proven on its own |
| `SPORTARR` | A-api | partial |  | ported (same push as Sonarr); not yet proven on its own |
| `SABNZBD` | A-api | partial | server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported; only the wrong-protocol answer is proven; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |
| `NZBGET` | A-api | partial | server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported; only the wrong-protocol answer is proven; goes through the download hub when the hub has the same client (server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address) |

#### Action fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `Name` | A-api | done | server/src/releases/action.rs::tests::json_is_autobrrs, server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported |
| `Type` | A-api | done | server/src/releases/action.rs::tests::json_is_autobrrs, server/tests/autobrr_api.rs::actions_store_toggle_and_delete, server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported |
| `Enabled` | A-api | done | server/src/releases/action.rs::tests::json_is_autobrrs, server/tests/autobrr_api.rs::actions_store_toggle_and_delete | ported; only enabled actions run |
| `ExecCmd` | A-api | done | server/src/releases/run.rs::tests::exec_runs_with_an_argument_list | ported |
| `ExecArgs` | A-api | done | server/src/releases/run.rs::tests::exec_runs_with_an_argument_list, server/src/releases/shellquote.rs::tests::simple_split | ported (go-shellquote split) |
| `WatchFolder` | A-api | done | server/src/releases/run.rs::tests::watch_folder_cases | ported |
| `Category` | A-api | done | server/src/releases/run.rs::tests::option_builders, server/src/releases/run.rs::tests::hub_routing_matches_kind_and_address | ported; also picks the hub app |
| `Tags` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `Label` | A-api | partial | server/src/releases/run.rs::tests::option_builders | ported (rTorrent, Deluge); Deluge label not proven against a client |
| `SavePath` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `DownloadPath` | A-api | partial | server/src/releases/run.rs::tests::option_builders | ported (qBittorrent); not proven against a client |
| `Paused` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `IgnoreRules` | A-api | partial |  | ported; client rules not proven against a client |
| `FirstLastPiecePrio` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `SkipHashCheck` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `ContentLayout` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `LimitUploadSpeed` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `LimitDownloadSpeed` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `LimitRatio` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `LimitSeedTime` | A-api | done | server/src/releases/run.rs::tests::option_builders | ported |
| `PriorityLayout` | A-api | done | server/src/releases/run.rs::tests::rtorrent_ratio_group_and_priority | ported |
| `RatioGroup` | A-api | done | server/src/releases/run.rs::tests::rtorrent_ratio_group_and_priority | ported |
| `ReAnnounceSkip` | A-api | partial |  | ported; not proven against a client |
| `ReAnnounceDelete` | A-api | partial |  | ported; not proven against a client |
| `ReAnnounceInterval` | A-api | partial |  | ported; not proven against a client |
| `ReAnnounceMaxAttempts` | A-api | partial |  | ported; not proven against a client |
| `WebhookHost` | A-api | done | server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported |
| `WebhookType` | A-api | partial | server/src/releases/action.rs::tests::json_is_autobrrs, server/tests/autobrr_api.rs::actions_store_toggle_and_delete | stored; autobrr's webhook action doesn't read it either |
| `WebhookMethod` | A-api | partial | server/src/releases/action.rs::tests::json_is_autobrrs, server/tests/autobrr_api.rs::actions_store_toggle_and_delete | stored; autobrr's webhook action doesn't read it either (always POST) |
| `WebhookData` | A-api | done | server/src/releases/run.rs::tests::test_webhook_unsupported_and_usenet_protocol | ported (macros filled in) |
| `WebhookHeaders` | A-api | partial | server/src/releases/action.rs::tests::json_is_autobrrs, server/tests/autobrr_api.rs::actions_store_toggle_and_delete | taken but not stored, as in autobrr |
| `ExternalDownloadClientID` | A-api | done | server/src/releases/run.rs::tests::arr_download_client_cases | ported |

#### Macros (template variables for actions)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `{{ .Artists }}` `{{ .Audio }}` `{{ .AudioChannels }}` `{{ .AudioFormat }}` `{{ .Bitrate }}` `{{ .Bonus }}` `{{ .Categories }}` `{{ .Category }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .Codec }}` `{{ .Container }}` `{{ .Cookie }}` `{{ .CurrentDay }}` `{{ .CurrentHour }}` `{{ .CurrentMinute }}` `{{ .CurrentMonth }}` `{{ .CurrentSecond }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .CurrentYear }}` `{{ .CurrentTimeUnixMS }}` `{{ .CurrentUnixTimeMS }}` `{{ .Description }}` `{{ .DownloadUrl }}` `{{ .Episode }}` `{{ .FilterID }}` `{{ .FilterName }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .Freeleech }}` `{{ .FreeleechPercent }}` `{{ .Group }}` `{{ .GroupID }}` `{{ .HDR }}` `{{ .HasCue }}` `{{ .HasLog }}` `{{ .Implementation }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .Indexer }}` `{{ .IndexerIdentifier }}` `{{ .IndexerIdentifierExternal }}` `{{ .IndexerName }}` `{{ .InfoUrl }}` `{{ .IsDuplicate }}` `{{ .Language }}` `{{ .Leechers }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .LogScore }}` `{{ .MagnetURI }}` `{{ .MetaIMDB }}` `{{ .MetaTMDB }}` `{{ .MetaTVDB }}` `{{ .MetaIGDB }}` `{{ .Origin }}` `{{ .Other }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .PreTime }}` `{{ .Protocol }}` `{{ .Proper }}` `{{ .Region }}` `{{ .ReleaseTags }}` `{{ .Repack }}` `{{ .Resolution }}` `{{ .Season }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .Seeders }}` `{{ .Size }}` `{{ .SizeString }}` `{{ .SkipDuplicateProfileID }}` `{{ .SkipDuplicateProfileName }}` `{{ .Source }}` `{{ .Tags }}` `{{ .Title }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .TorrentDataRawBytes }}` `{{ .TorrentHash }}` `{{ .TorrentID }}` `{{ .TorrentName }}` `{{ .TorrentPathName }}` `{{ .TorrentUrl }}` `{{ .TorrentTmpFile }}` `{{ .Type }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |
| `{{ .Uploader }}` `{{ .RecordLabel }}` `{{ .RawVars }}` `{{ .Website }}` `{{ .Year }}` `{{ .Month }}` `{{ .Day }}` | A-api | done | server/tests/upstream_autobrr.rs::macros_parse, server/tests/releases_filter_fields.rs::every_macro_variable | ported (Go text/template subset with sprig functions) |

### Download clients

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `ARIA2` | A-api | done | server/src/releases/clients/aria2.rs::tests::build_endpoint_cases, server/src/releases/clients/aria2.rs::tests::tell_active, server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported |
| `DELUGE_V1` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `DELUGE_V2` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `LIDARR` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `NZBGET` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `PORLA` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `QBITTORRENT` | A-api | partial | server/src/releases/downloader.rs::tests::qbit_build_legacy_host, server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test not proven against a client |
| `RADARR` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `READARR` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `RTORRENT` | A-api | done | server/src/releases/run.rs::tests::rtorrent_ratio_group_and_priority, server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported |
| `SABNZBD` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `SONARR` | A-api | done | server/src/releases/run.rs::tests::sonarr_indexer_not_found_and_found, server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | ported |
| `SPORTARR` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `TRANSMISSION` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `WHISPARR` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |
| `WHISPARR_V3` | A-api | partial | server/src/releases/downloader.rs::tests::secrets_are_redacted_and_kept, server/tests/autobrr_api.rs::download_clients_redact_and_keep_secrets | stored as autobrr stores them; connection test ported, not proven against a client |

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
| `TORZNAB` | A-api | done | server/tests/autobrr_feeds.rs::znab_search_pages_with_the_caps_limit_and_keeps_host_params, server/tests/autobrr_feeds.rs::each_format_reads_a_page_as_releases, server/src/releases/feed/sources.rs::tests::test_torznab_source_to_release | ported (`releases/feed/`) |
| `NEWZNAB` | A-api | done | server/tests/autobrr_feeds.rs::each_format_reads_a_page_as_releases, server/src/releases/feed/sources.rs::tests::test_newznab_source_to_release, server/src/releases/feed/znab.rs::tests::test_newznab_feed_item_unmarshal_xml | ported (`releases/feed/`) |
| `RSS` | A-api | done | server/tests/autobrr_feeds.rs::each_format_reads_a_page_as_releases, server/src/releases/feed/sources.rs::tests::test_rss_source_to_release, server/src/releases/feed/gofeed.rs::tests::reads_rss_items_with_custom_elements_and_extensions | ported (`releases/feed/`, gofeed rewritten); Lantern's own RSS reader stays for its own sources |
| caps, max age, timeout, cookie, user agent, proxy per feed | A-api | partial | server/tests/autobrr_feeds.rs::feed_tests_and_caps_go_through_the_feeds_own_client, server/tests/autobrr_feeds.rs::znab_caps_read_the_fixture_and_refuse_a_bad_key, server/src/releases/feed/refresh.rs::tests::test_is_max_age | caps, max age, timeout, cookie and user agent ported; proxies are not, so a feed on an indexer that uses one stops with an error |
| first run only stores what is there | A-api | done | server/src/releases/feed/refresh.rs::tests::test_refresh_job_run_e, server/src/releases/feed/refresh.rs::tests::test_refresh_job_fetch_new_pagination | ported |

### Lists

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `RADARR` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters, server/src/releases/list/process.rs::tests::radarr_fills_shows_or_match_releases | ported (`releases/list/`) |
| `SONARR` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `LIDARR` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `READARR` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `WHISPARR` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `WHISPARR_V3` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `SPORTARR` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `MDBLIST` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `METACRITIC` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/process.rs::tests::test_metacritic_processor_process | ported (`releases/list/`) |
| `PLAINTEXT` | A-api | done | server/tests/autobrr_feeds.rs::lists_fill_their_filters, server/src/releases/list/process.rs::tests::plaintext_keeps_lines_as_they_are_when_asked | ported (`releases/list/`); `file://` and http(s) |
| `TRAKT` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/process.rs::tests::test_transform_trakt_url | ported (`releases/list/`) |
| `STEAM` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |
| `ANILIST` | A-api | done | server/tests/autobrr_feeds.rs::every_list_type_fills_its_fields, server/src/releases/list/title.rs::tests::test_process_title | ported (`releases/list/`) |

### IRC

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Networks: server, port, TLS, nick, NickServ/SASL auth, invite command, bouncer, proxy, bot mode | A-api | partial | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces, server/tests/autobrr_irc.rs::a_network_signs_in_with_sasl_and_joins_without_an_invite, server/src/releases/irc.rs::tests::test_irc_auth_validate_and_nickserv_enabled | ported; proxies aren't ported (a network using one refuses to connect); TLS and bot mode not run against a stand-in server |
| Channels with password, per indexer definition | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported |
| Announce parsing with every definition in `internal/indexer/definitions` (line patterns, multiline, vars, mappings) | A-api | done | server/tests/autobrr_irc.rs::every_bundled_definition_reads_its_own_test_lines, server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces, server/src/releases/definition.rs::tests::test_indexer_ircv2_parse_match_parse_urls, server/src/releases/definition.rs::tests::test_irc_parser_gazelle_games_parse | ported (every bundled definition's test lines) |
| Health monitor and reconnect back-off | A-api | partial | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces, server/tests/autobrr_irc.rs::a_ban_stops_the_network_with_the_reason, server/src/releases/irc.rs::tests::invite_commands_and_bots | ported; health and stopping on a ban are proven, the back-off delays only as values |
| Message history per channel | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported (1000 lines) |
| Send raw command | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported |
| Manual announce (process a pasted line) | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported |
| Pre/after-connect commands, invite flow | A-api | done | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces | ported (autobrr's only connect commands are NickServ and the invite) |

### Indexers

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Every indexer definition (IRC, torznab, newznab, RSS), settings, base URL override | A-api | partial | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules, server/tests/autobrr_irc.rs::every_bundled_definition_reads_its_own_test_lines, server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | IRC indexers ported; Torznab, Newznab and RSS indexers are polled through their feed; base URL override not checked yet |
| Custom definitions folder (`CustomDefinitions`) | A-api | partial | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules | definitions are read from `APP_ANNOUNCE_DIR` and `<data>/announce`; the `customDefinitions` config key isn't read |
| Deprecated indexers list and prune | A-api | done | server/tests/autobrr_irc.rs::indexers_keep_their_secrets_and_follow_autobrrs_rules, server/tests/autobrr_irc.rs::deprecated_indexers_have_no_live_definition | ported |
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
| mobile layout | A-ui | partial | `docs/parity/a-ui/dashboard-phone.png`, `filters-phone.png`, `settings-irc-phone.png`, `menu-open-phone.png` | autobrr's layout unchanged; the folded menu and three pages are shown, the other pages wait for their API |
| light/dark (rethemed to Encore) | A-ui | done | `docs/parity/a-ui/dashboard-desktop.png`, `filters-desktop.png`, `settings-irc-desktop.png`, `dashboard-desktop-light.png` | every Tailwind colour scale and the chart colours drawn from the kit's tokens, the kit's fonts; dark mode is Encore, light mode the kit's light theme (`ui/autobrr/LANTERN.md`) |
| branding: Lantern's name in the header, no docs, GitHub or Discord links, no autobrr icons | A-ui | done | `docs/parity/a-ui/dashboard-desktop.png`, `menu-open-phone.png` | `ui/autobrr/LANTERN.md` lists each change; served at `/autobrr/` (`server/tests/autobrr_ui.rs`) |
| toasts | A-ui | todo | |  |

### Background jobs

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Feed refresh per feed (its interval, anchored to last run) | A-api | done | server/src/releases/scheduler.rs::tests::test_anchored_schedule_next, server/src/releases/scheduler.rs::tests::test_anchored_schedule_spreads_feeds_sharing_an_interval, server/tests/autobrr_feeds.rs::feeds_are_added_changed_run_and_removed | ported (`releases/scheduler.rs`) |
| Feed cache cleanup (daily 03:05) | A-api | done | server/tests/autobrr_feeds.rs::the_cleanup_and_list_jobs_are_scheduled_and_the_cleanup_drops_old_items, server/src/releases/scheduler.rs::tests::cron_lines | ported |
| List refresh (every 6 h) | A-api | done | server/tests/autobrr_feeds.rs::the_cleanup_and_list_jobs_are_scheduled_and_the_cleanup_drops_old_items, server/src/releases/scheduler.rs::tests::cron_lines | ported, with autobrr's random 1 to 35 second wait |
| Release cleanup jobs (their cron) | A-api | todo | |  |
| Update check (every 2 h) | A-api | todo | |  |
| Temp folder cleanup (daily 04:00) | A-api | todo | |  |
| Notification inbox cleanup (every hour) | A-api | todo | |  |
| IRC health monitor and reconnect | A-api | partial | server/tests/autobrr_irc.rs::a_network_signs_in_with_nickserv_is_invited_and_reads_multi_line_announces, server/tests/autobrr_irc.rs::a_ban_stops_the_network_with_the_reason | each network reconnects on its own; no separate health job |
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
| `POST /api/webhook` (search one torrent by infoHash or path) | X | partial | `server/tests/crossseed.rs::the_cross_seed_api_answers_as_cross_seed_does`, `server/src/crossseed/daemon.rs`::webhook_schema_like_zod | answers, statuses and messages as cross-seed; an infoHash searches that client torrent; a path is searched as data and its matches are linked and added by cross-seed's `performAction` (data-based search over `dataDirs` is a later slice) |
| `POST /api/announce` (match an announce, e.g. from autobrr) | X | partial | `server/tests/crossseed.rs::the_cross_seed_api_answers_as_cross_seed_does`, `server/src/crossseed/daemon.rs`::announce_schema_like_zod, `server/src/crossseed/daemon.rs`::announce_statuses | statuses 200/202/204/400 and zod messages as cross-seed; client torrents like it are picked by cross-seed's keys and title comparison, not its full `getSimilarByName` with the torrent cache and data dirs |
| `POST /api/job` (run a job now) | X | partial | `server/tests/crossseed.rs::the_cross_seed_api_answers_as_cross_seed_does`, `server/src/crossseed/daemon.rs`::job_schema_like_zod | the messages and 404/409/200 as cross-seed; only `search` runs here (rss, inject, cleanup and updateIndexerCaps answer "disabled in config" until their jobs are ported) |
| `GET /api/ping` | X | done | `server/tests/crossseed.rs::the_cross_seed_api_answers_as_cross_seed_does` | own port, 2468 (`cross_seed_port`) |
| `GET /api/status` | X | done | `server/tests/crossseed.rs::the_cross_seed_api_answers_as_cross_seed_does` | |
| API key in `X-Api-Key` header or `apikey` query | X | done | `server/tests/crossseed.rs::the_cross_seed_api_answers_as_cross_seed_does` | Lantern's API key (`api_key`); 405 before 401 before 404, as cross-seed |

### Settings (`config.js`)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `matchMode` (default `flexible`) | X | done | `server/src/crossseed/config.rs`::settings_ranges_in_cross_seeds_words | `cross_seed_match_mode` |
| `fuzzySizeThreshold` (0.02) | X | done | `server/src/crossseed/config.rs`::settings_ranges_in_cross_seeds_words | `cross_seed_fuzzy_size_threshold` |
| `seasonFromEpisodes` (1) | X | done | `server/src/crossseed/config.rs`::settings_ranges_in_cross_seeds_words | `cross_seed_season_from_episodes` (0 = off) |
| `includeSingleEpisodes` (false) | X | done | `server/src/crossseed/config.rs`::defaults_are_cross_seeds_template | `cross_seed_include_single_episodes` |
| `includeNonVideos` (false) | X | done | `server/src/crossseed/config.rs`::defaults_are_cross_seeds_template | `cross_seed_include_non_videos` |
| `ignoreTitles` | X | done | `server/src/crossseed/config.rs`::defaults_are_cross_seeds_template | `cross_seed_ignore_titles` |
| `excludeOlder` (2 weeks) | X | done | `server/src/crossseed/config.rs`::durations_like_vercel_ms | `cross_seed_exclude_older` (replaces again_days) |
| `excludeRecentSearch` (3 days) | X | done | `server/src/crossseed/config.rs`::durations_like_vercel_ms | `cross_seed_exclude_recent_search` |
| `delay` (30 s, 30 s to 1 hour) | X | done | `server/src/crossseed/config.rs`::settings_ranges_in_cross_seeds_words | `cross_seed_delay` |
| `blockList` | X | done | `server/src/crossseed/config.rs`::block_list_checks | `cross_seed_block_list` (one per line) |
| `port` (2468) | X | done | `server/src/crossseed/config.rs`::defaults_are_cross_seeds_template | `cross_seed_port` (0 = off) |
| `apiKey` | X | done | `server/src/crossseed/config.rs`::defaults_are_cross_seeds_template | Lantern's `api_key` |
| `action`, `linkDirs`, `linkType`, `flatLinking`, `linkCategory`, `duplicateCategories`, `skipRecheck`, `autoResumeMaxDownload`, `ignoreNonRelevantFilesToResume`, `torrentClients` | X | done | `server/src/crossseed/config.rs`::torrent_client_checks, `server/src/crossseed/config.rs`::the_default_needs_link_dirs_as_in_cross_seed, `server/src/crossseed/config.rs`::settings_ranges_in_cross_seeds_words | `cross_seed_action`, `cross_seed_link_dirs` (one per line), `cross_seed_link_type`, `cross_seed_flat_linking`, `cross_seed_link_category`, `cross_seed_duplicate_categories`, `cross_seed_skip_recheck`, `cross_seed_auto_resume_max_download`, `cross_seed_ignore_non_relevant_files_to_resume`, `cross_seed_torrent_clients` (one per line; empty: Lantern's torrent download clients) |
| Other options (`dataDirs`, `rssCadence`, `searchCadence`, `snatchTimeout`, `searchTimeout`, `searchLimit`, `notificationWebhookUrls`, `sonarr`, `radarr`, `outputDir`, `torrentDir`, `maxDataDepth`, `useClientTorrents`) | X | partial | `server/src/crossseed/config.rs`::defaults_are_cross_seeds_template | cross-seed's defaults and checks in the config; no Lantern setting yet (their features are later slices); `outputDir` is `cross-seeds/` in the data folder |

### Matching and injection

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Match modes `strict`, `flexible`, `partial` | X | done | `server/src/crossseed/decide.rs`::match_size_only_when_names_differ_outside_strict, `server/src/crossseed/decide.rs`::match_partial_and_partial_size_mismatch | `cross_seed_match_mode` |
| Decisions: `MATCH`, `MATCH_SIZE_ONLY`, `MATCH_PARTIAL`, `FUZZY_SIZE_MISMATCH`, `SIZE_MISMATCH`, `PARTIAL_SIZE_MISMATCH`, `NO_DOWNLOAD_LINK`, `DOWNLOAD_FAILED`, `MAGNET_LINK`, `RATE_LIMITED`, `SAME_INFO_HASH`, `INFO_HASH_ALREADY_EXISTS`, `FILE_TREE_MISMATCH`, `RELEASE_GROUP_MISMATCH`, `BLOCKED_RELEASE`, `PROPER_REPACK_MISMATCH`, `RESOLUTION_MISMATCH`, `SOURCE_MISMATCH` | X | done | `server/src/crossseed/decide.rs` (a test per decision), `server/tests/crossseed.rs::a_finished_torrent_seeds_on_the_other_sites_that_have_the_same_files` | kept as `match_decision` on each result |
| Actions `save` and `inject`; injection results | X | done | `server/tests/crossseed_clients.rs::a_match_from_a_data_folder_is_linked_then_injected`, `server/tests/crossseed.rs::a_finished_torrent_seeds_on_the_other_sites_that_have_the_same_files`, `server/src/crossseed/action.rs`::saved_torrent_names | `server/src/crossseed/action.rs` (`performAction`): `SUCCESS`, `ALREADY_EXISTS`, `TORRENT_NOT_COMPLETE`, `FAILURE`, `SAVED`; without `linkDirs` a match goes next to the searchee's files, with them its files are linked first; failures are saved to `cross-seeds/` in the data folder |
| Link types `symlink`, `hardlink`, `reflink`, `reflinkOrCopy`; `flatLinking` | X | done | `server/src/crossseed/action.rs`::link_types_as_cross_seed_makes_them, `server/src/crossseed/action.rs`::links_every_file_of_a_match, `server/src/crossseed/action.rs`::the_link_folder_on_the_same_drive, `server/tests/crossseed_clients.rs::a_match_from_a_data_folder_is_linked_then_injected` | `linkFile`, `linkAllFilesInMetafile`, `getLinkDir` (same drive first, then a test link) and `unlinkMetafile`; reflinks through Linux's `FICLONE` |
| Block list types (name, regex, folder, category, tag, tracker, infoHash, size) | X | done | `server/src/crossseed/prefilter.rs`::every_block_list_type, `server/src/crossseed/config.rs`::block_list_checks | |
| Clients: qBittorrent, rTorrent, Transmission, Deluge (several at once) | X | done | `server/tests/crossseed_clients.rs::qbittorrent_as_cross_seed_talks_to_it`, `server/tests/crossseed_clients.rs::rtorrent_as_cross_seed_talks_to_it`, `server/tests/crossseed_clients.rs::transmission_as_cross_seed_talks_to_it`, `server/tests/crossseed_clients.rs::deluge_as_cross_seed_talks_to_it`, `server/src/crossseed/clients/mod.rs`::client_entries_as_cross_seed_reads_them | `server/src/crossseed/clients/`: each stand-in answers as the client does and the requests are compared with cross-seed's, in order (`fixtures/upstream/cross-seed/clients.json`); several clients by `clientHost` and priority, `readonly:` entries are searched but never added to |
| Data-based searching (`dataDirs`), season packs from episodes | X | todo | |  |
| Pre-filter rules (age, recent search, ignored titles, non-video, single episodes) | X | done | `server/src/crossseed/prefilter.rs`::content_rules, `server/src/crossseed/prefilter.rs`::timestamps, `server/src/crossseed/prefilter.rs`::season_pack_episodes_and_specials_from_data | |
| Arr lookups (Sonarr/Radarr ids for better searching) | X | todo | |  |
| Recheck and auto-resume rules | X | done | `server/src/crossseed/clients/mod.rs`::recheck_and_resume_rules, `server/tests/crossseed_clients.rs::qbittorrent_as_cross_seed_talks_to_it` | `shouldRecheck`, `getMaxRemainingBytes`, `shouldResumeFromNonRelevantFiles`, `estimatePausedStatus` and each client's `resumeInjection` (checks every 15 s for up to 5 minutes, an hour when only more pieces are missing) |
| Notification webhooks | X | todo | |  |
| Torrent cache and client torrent cache | X | partial | `server/src/crossseed/clients/mod.rs`::a_kept_torrent_changes_when_its_name_folder_category_or_tags_do, `server/tests/crossseed_clients.rs::qbittorrent_as_cross_seed_talks_to_it` | the client torrent cache (`client_searchee`, `cross_seed_client_searchees`) as cross-seed; the `.torrent` cache of `torrentDir` is a later slice |

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
| `GET /` (version and user agent) | F | done | server/tests/solver.rs::tests_py_requests, server/tests/solver.rs::tests_py_answers_that_need_no_page | ported (`api.rs`) |
| `GET /health` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |
| `POST /v1` `request.get` | F | done | server/tests/solver.rs::tests_py_requests, server/tests/solver.rs::tests_py_challenges | ported (`_evil_logic` in `mod.rs`) |
| `POST /v1` `request.post` | F | done | server/tests/solver.rs::tests_py_requests, server/tests/solver.rs::tests_py_challenges, server/src/solver/mod.rs::the_flaresolverr_post_form_is_built_like_post_request | ported (`_post_request`) |
| `POST /v1` `sessions.create` | F | done | server/tests/solver.rs::tests_py_sessions | ported (a browser context per session) |
| `POST /v1` `sessions.list` | F | done | server/tests/solver.rs::tests_py_sessions, server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |
| `POST /v1` `sessions.destroy` | F | done | server/tests/solver.rs::tests_py_sessions, server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |

### Request fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `cmd` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |
| `url` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page, server/tests/solver.rs::tests_py_requests | ported |
| `session` | F | done | server/tests/solver.rs::tests_py_sessions | ported |
| `session_ttl_minutes` | F | done | server/tests/solver.rs::tests_py_sessions, server/src/solver/api.rs::a_request_becomes_a_job | ported |
| `maxTimeout` | F | done | server/tests/solver.rs::tests_py_requests, server/tests/solver.rs::tests_py_answers_that_need_no_page, server/src/solver/api.rs::python_reading_of_values | ported |
| `cookies` | F | done | server/tests/solver.rs::tests_py_requests, server/src/solver/api.rs::a_request_becomes_a_job | ported |
| `proxy (url, username, password)` | F | done | server/tests/solver.rs::tests_py_proxies, server/src/solver/api.rs::the_environment_proxy_fills_in | ported |
| `headers` | F | done | server/tests/solver.rs::tests_py_requests | ported (ignored with a warning, as upstream) |
| `userAgent` | F | done | server/tests/solver.rs::tests_py_requests | ported (ignored with a warning, as upstream) |
| `postData` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page, server/tests/solver.rs::tests_py_requests | ported |
| `returnOnlyCookies` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `returnScreenshot` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `returnRawHtml` | F | done | server/tests/solver.rs::tests_py_requests | ported (ignored with a warning, as upstream) |
| `download` | F | done | server/tests/solver.rs::tests_py_requests | ported (ignored with a warning, as upstream) |
| `waitInSeconds` | F | done | server/tests/solver.rs::tests_py_requests, server/src/solver/api.rs::a_request_becomes_a_job | ported |
| `disableMedia` | F | done | server/tests/solver.rs::tests_py_requests, server/tests/solver.rs::tests_py_proxies | ported |
| `tabs_till_verify` | F | done | server/tests/solver.rs::tests_py_challenges | ported |

### Answer fields

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `status` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page, server/tests/solver.rs::tests_py_requests | ported |
| `message` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page, server/tests/solver.rs::tests_py_requests, server/tests/solver.rs::tests_py_challenges | ported, word for word |
| `startTimestamp` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |
| `endTimestamp` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |
| `version` | F | done | server/tests/solver.rs::tests_py_answers_that_need_no_page | ported (3.5.2) |
| `solution.url` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `solution.status` | F | done | server/tests/solver.rs::tests_py_requests, server/src/solver/api.rs::the_solution_has_flaresolverrs_fields | ported (always 200, as upstream) |
| `solution.headers` | F | done | server/tests/solver.rs::tests_py_requests | ported (`{}`, left out with returnOnlyCookies) |
| `solution.response` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `solution.cookies` | F | done | server/tests/solver.rs::tests_py_requests, server/src/solver/api.rs::the_solution_has_flaresolverrs_fields | ported (chromedriver's cookie shape) |
| `solution.userAgent` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `solution.screenshot` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `solution.turnstile_token` | F | done | server/tests/solver.rs::tests_py_challenges | ported |
| `session` | F | done | server/tests/solver.rs::tests_py_sessions | ported |
| `sessions` | F | done | server/tests/solver.rs::tests_py_sessions, server/tests/solver.rs::tests_py_answers_that_need_no_page | ported |

### Settings (environment)

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| `HOST` | F | n/a |  | mapped: Lantern listens on all addresses (HOST's default 0.0.0.0); see DECISIONS.md |
| `PORT` | F | n/a |  | mapped to `APP_SOLVER_PORT` (default 8191); see DECISIONS.md |
| `LOG_LEVEL` | F | n/a |  | mapped to Lantern's log (`RUST_LOG`); see DECISIONS.md |
| `LOG_FILE` | F | n/a |  | logs go to stdout and the kit's log (read-only root); see DECISIONS.md |
| `LOG_HTML` | F | partial | server/src/solver/config.rs::only_true_turns_a_switch_on | read; the debug log line isn't asserted |
| `HEADLESS` | F | partial | server/src/solver/config.rs::only_true_turns_a_switch_on | read; `false` shows a window, not tested (no screen in CI) |
| `BROWSER_WAIT_TIMEOUT` | F | done | server/tests/solver.rs::tests_py_challenges, server/src/solver/config.rs::numbers_and_texts | ported |
| `DISABLE_MEDIA` | F | done | server/tests/solver.rs::tests_py_proxies | ported |
| `LANG` | F | done | server/tests/solver.rs::tests_py_requests | ported (`--accept-lang`) |
| `TZ` | F | done | server/tests/solver.rs::tests_py_requests | ported |
| `PROXY_URL` | F | done | server/tests/solver.rs::tests_py_proxies, server/src/solver/api.rs::the_environment_proxy_fills_in | ported |
| `PROXY_USERNAME` | F | done | server/tests/solver.rs::tests_py_proxies, server/src/solver/api.rs::the_environment_proxy_fills_in | ported |
| `PROXY_PASSWORD` | F | done | server/tests/solver.rs::tests_py_proxies, server/src/solver/api.rs::the_environment_proxy_fills_in | ported |
| `PROMETHEUS_ENABLED` | F | done | server/tests/solver.rs::tests_py_requests, server/src/solver/metrics.rs::answers_are_counted_by_site_and_result | ported |
| `PROMETHEUS_PORT` | F | done | server/tests/solver.rs::tests_py_requests | ported (8192) |

### Challenges

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Cloudflare JS challenge | F | done | server/tests/solver.rs::tests_py_challenges, server/tests/solver.rs::indexers_get_past_a_check_and_keep_the_cookies | ported |
| DDoS-Guard | F | done | server/tests/solver.rs::tests_py_challenges | ported |
| Cloudflare Turnstile (click and token) | F | done | server/tests/solver.rs::tests_py_challenges | ported (`click_verify`, `tabs_till_verify`) |
| Access denied / banned detection and messages | F | done | server/tests/solver.rs::tests_py_challenges | ported |
| Prometheus metrics | F | done | server/tests/solver.rs::tests_py_requests, server/src/solver/metrics.rs::answers_are_counted_by_site_and_result | ported |

## Lantern's own pages

Kept where the merge needs them; they follow the same rule (done with a test or screenshot).

| Item | Track | Status | Proof | Lantern today |
| --- | --- | --- | --- | --- |
| Download hub (apps, clients, grabs, Soulseek) | L | partial | | own page |
| Account vault | L | partial | | own page |
| Account keeper | L | partial | | own page |
| Cloudflare solving | L | partial | | own page |

