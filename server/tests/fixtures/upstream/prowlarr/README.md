# Prowlarr v2.7.0.5723 answers

Answers of Prowlarr's v1 API as Prowlarr writes them (camelCase, enums as camelCase text, dates as
`yyyy-MM-ddTHH:mm:ssZ`, null fields left out), built from its resource classes in `src/Prowlarr.Api.V1` and
`src/Prowlarr.Http`, its `openapi.json` and its JSON writer (`src/NzbDrone.Common/Serializer/System.Text.Json`).
The values are those of a new Prowlarr in Docker. `server/tests/prowlarr_api.rs` checks that Lantern answers with the
same fields and types (and, for the settings, the same defaults). Errors also carry the kit's `detail`; Prowlarr's
`description` (a .NET stack trace) is left out.
