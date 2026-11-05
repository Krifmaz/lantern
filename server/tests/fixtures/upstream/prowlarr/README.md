# Prowlarr v2.7.0.5723 answers

Answers of Prowlarr's v1 API as Prowlarr writes them (camelCase, enums as camelCase text, dates as
`yyyy-MM-ddTHH:mm:ssZ`, null fields left out), built from its resource classes in `src/Prowlarr.Api.V1` and
`src/Prowlarr.Http`, its `openapi.json` and its JSON writer (`src/NzbDrone.Common/Serializer/System.Text.Json`).
The values are those of a new Prowlarr in Docker. `server/tests/prowlarr_api.rs` checks that Lantern answers with the
same fields and types (and, for the settings, the same defaults). Errors also carry the kit's `detail`; Prowlarr's
`description` (a .NET stack trace) is left out.

The `apps_*` files are for app sync (`server/tests/prowlarr_apps.rs`):

- `apps_schema_sonarr.json`: Sonarr's entry of `GET /api/v1/applications/schema`, from `SonarrSettings`'
  `FieldDefinition`s through `SchemaBuilder` and `NewznabCategoryFieldConverter` (`{appName}` texts say Lantern).
- `apps_sonarr_indexer_schema.json`, `apps_radarr_indexer_schema.json`: what a Sonarr v4 and a Radarr v5 answer to
  `GET /api/v3/indexer/schema`, cut down to the implementations and fields Prowlarr reads.
- `apps_sonarr_push_*.json`, `apps_radarr_push_torznab.json`: the indexer Prowlarr sends to the app, from
  `BuildSonarrIndexer` and `BuildRadarrIndexer` (`SonarrIndexer` and `RadarrIndexer` written by STJson: camelCase,
  nulls left out; each field as `SonarrField` keeps it), for the tests' indexers with Prowlarr Server
  `http://lantern:9696` and API key `test-key`. `_test` is the indexer of the app's test (`Test`, id 0, turned off).

The `indexer*` and `history*` files are for the indexer API (`server/tests/prowlarr_indexers.rs`):

- `indexer_cardigann.json`: `GET /api/v1/indexer/1` for the test tracker (`tests/fixtures/definitions/lantern-test.yml`)
  signed in as `me`/`pw`, from `IndexerResourceMapper.ToResource` (the standard fields through `SchemaBuilder`, the
  definition's own settings through `MapCardigannField`: a select's value is the place of the default among the
  options sorted by key) and `IndexerCapabilityResource`. Prowlarr sends the password in the clear; Lantern masks it.
- `indexer_schema_newznab.json`: the NZBgeek entry of `GET /api/v1/indexer/schema` (`Newznab.DefaultDefinitions`
  through `SetProviderCharacteristics`; `{appName}` texts say Lantern).
- `indexer_invalid.json`: a `POST /api/v1/indexer` without a name (the shared `NotEmpty` rule, as FluentValidation
  writes a `ValidationFailure`).
- `indexerstatus.json`, `indexerstats.json`: `IndexerStatusResource` of a site on hold, and `IndexerStatsResource`
  (`IndexerStatisticsService`) after one search from the web app.
- `history.json`, `history_since.json`: `PagingResource<HistoryResource>` and the `/since` list after that search
  (`HistoryService.Handle(IndexerQueryEvent)`: the details are text, their names camelCase).
