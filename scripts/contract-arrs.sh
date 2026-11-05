#!/usr/bin/env bash
# Contract run: real Sonarr, Radarr, Lidarr and Readarr add Lantern as a Torznab indexer, unchanged. Each app tests
# the indexer before it saves it (Torznab caps, then a search that must return releases in its categories), so a
# saved indexer means the app can use Lantern. Lantern's history must then show each app's queries.
# Then each app adds Lantern as its qBittorrent, SABnzbd and NZBGet download client; each app tests a client before
# it saves it (signing in, the version, the settings, the categories, the list of downloads), and every app must
# then be on Lantern's Apps list.
#
# Needs: docker, curl, jq, and `cargo build --release --example mock_tracker && cargo build --release` in server/.
set -euo pipefail
cd "$(dirname "$0")/.."

KEY=contract-key
LANTERN=http://127.0.0.1:9696
BIN=server/target/release
WORK=$(mktemp -d)

cleanup() {
  for c in sonarr radarr lidarr readarr; do docker rm -f "contract-$c" >/dev/null 2>&1 || true; done
  kill "${PIDS[@]}" 2>/dev/null || true
}
PIDS=()
trap cleanup EXIT

wait_for() { # url, what
  for _ in $(seq 1 120); do curl -fs -o /dev/null "$1" && return 0; sleep 2; done
  echo "$2 didn't start"; return 1
}

"$BIN/examples/mock_tracker" 0.0.0.0:8999 & PIDS+=($!)
APP_DATA_DIR="$WORK/data" APP_STATIC_DIR="$WORK/static" APP_PORT=9696 APP_API_KEY=$KEY APP_ADMIN_PASSWORD=contract-password \
  APP_DEFINITIONS_DIR=server/tests/fixtures/definitions APP_DEFINITIONS_AUTO_UPDATE=false "$BIN/lantern" & PIDS+=($!)
wait_for "$LANTERN/api/health" Lantern

ID=$(curl -fsS -H "x-api-key: $KEY" -H 'content-type: application/json' "$LANTERN/api/indexers" \
  -d '{"definition":"lantern-test","base_url":"http://127.0.0.1:8999/","settings":{"username":"me","password":"pw"}}' | jq .id)
curl -fsS -X POST -H "x-api-key: $KEY" "$LANTERN/api/indexers/$ID/test" | tee /dev/stderr | jq -e .ok >/dev/null

# app, port, API version, image tag, categories
APPS="sonarr 8989 v3 latest [5000,5040]
radarr 7878 v3 latest [2000,2045]
lidarr 8686 v1 latest [3000,3040]
readarr 8787 v1 0.4.18-develop [7000,7020]"

while read -r app port ver tag cats; do
  docker run -d --name "contract-$app" --network host -e PUID=1000 -e PGID=1000 "linuxserver/$app:$tag" >/dev/null
done <<< "$APPS"

failed=0
while read -r app port ver tag cats; do
  echo "::group::$app"
  key=""
  for _ in $(seq 1 120); do
    key=$(docker exec "contract-$app" sh -c 'sed -n "s:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p" /config/config.xml' 2>/dev/null || true)
    [ -n "$key" ] && curl -fs -o /dev/null -H "X-Api-Key: $key" "http://127.0.0.1:$port/api/$ver/system/status" && break
    sleep 2
  done
  [ -n "$key" ] || { echo "$app didn't start"; failed=1; echo "::endgroup::"; continue; }
  schema=$(curl -fsS -H "X-Api-Key: $key" "http://127.0.0.1:$port/api/$ver/indexer/schema" | jq '.[] | select(.implementation=="Torznab")')
  body=$(jq --arg url "$LANTERN/$ID/" --arg key "$KEY" --argjson cats "$cats" '
      .name = "Lantern" | .enableRss = true | .enableAutomaticSearch = true | .enableInteractiveSearch = true | .priority = 25
      | .fields |= map(if .name == "baseUrl" then .value = $url
                       elif .name == "apiPath" then .value = "/api"
                       elif .name == "apiKey" then .value = $key
                       elif .name == "categories" then .value = $cats
                       else . end)' <<< "$schema")
  code=$(curl -sS -o "$WORK/$app.json" -w '%{http_code}' -H "X-Api-Key: $key" -H 'content-type: application/json' \
    "http://127.0.0.1:$port/api/$ver/indexer" -d "$body")
  if [ "$code" = 201 ] || [ "$code" = 200 ]; then
    echo "$app added Lantern as an indexer ($code)"
  else
    echo "$app refused Lantern ($code):"; cat "$WORK/$app.json"; echo; failed=1
  fi
  for impl in QBittorrent Sabnzbd Nzbget; do
    schema=$(curl -fsS -H "X-Api-Key: $key" "http://127.0.0.1:$port/api/$ver/downloadclient/schema" | jq --arg i "$impl" '.[] | select(.implementation==$i)')
    [ -n "$schema" ] || { echo "$app has no $impl download client"; failed=1; continue; }
    body=$(jq --arg key "$KEY" --arg user "${app^}" --arg impl "$impl" '
        .name = "Lantern \(.implementation)" | .enable = true | .priority = 1
        | .fields |= map(if .name == "host" then .value = "127.0.0.1"
                         elif .name == "port" then .value = 9696
                         elif .name == "useSsl" then .value = false
                         elif .name == "urlBase" then .value = ""
                         elif .name == "apiKey" then .value = (if $impl == "Sabnzbd" then $key else "" end)
                         elif .name == "username" then .value = (if $impl == "Sabnzbd" then "" else $user end)
                         elif .name == "password" then .value = (if $impl == "Sabnzbd" then "" else $key end)
                         else . end)' <<< "$schema")
    code=$(curl -sS -o "$WORK/$app-$impl.json" -w '%{http_code}' -H "X-Api-Key: $key" -H 'content-type: application/json' \
      "http://127.0.0.1:$port/api/$ver/downloadclient" -d "$body")
    if [ "$code" = 201 ] || [ "$code" = 200 ]; then
      echo "$app added Lantern as its $impl ($code)"
    else
      echo "$app refused Lantern as its $impl ($code):"; cat "$WORK/$app-$impl.json"; echo; failed=1
    fi
  done
  echo "::endgroup::"
done <<< "$APPS"

echo "::group::Lantern's apps"
curl -fsS -H "x-api-key: $KEY" "$LANTERN/api/hub/apps" | jq -r '.apps[] | "\(.name)\t\(.source)\t\(.category)\t\(.folder)"'
echo "::endgroup::"
for src in Sonarr Radarr Lidarr Readarr; do
  curl -fsS -H "x-api-key: $KEY" "$LANTERN/api/hub/apps" | jq -e --arg s "$src" '[.apps[] | select(.name == $s)] | length > 0' >/dev/null \
    || { echo "$src never reached Lantern as a download client"; failed=1; }
done

echo "::group::Lantern's history"
curl -fsS -H "x-api-key: $KEY" "$LANTERN/api/indexers/history?limit=50" | jq -r '.history[] | "\(.source)\t\(.kind)\t\(.ok)\t\(.results)\t\(.error)"'
echo "::endgroup::"
for src in Sonarr Radarr Lidarr Readarr; do
  curl -fsS -H "x-api-key: $KEY" "$LANTERN/api/indexers/history?limit=200" | jq -e --arg s "$src" '[.history[] | select(.source == $s and .ok)] | length > 0' >/dev/null \
    || { echo "No successful search from $src reached Lantern"; failed=1; }
done
exit $failed
