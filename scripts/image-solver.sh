#!/usr/bin/env bash
# Image check: the Docker image's own browser gets past a Cloudflare-style check through the FlareSolverr API, with
# the image as it ships (distroless, non-root, read-only root filesystem).
#
# Needs: docker, curl, jq, the image (`docker build -t lantern:ci .`) and
# `cargo build --release --example mock_challenge` in server/.
set -euo pipefail
cd "$(dirname "$0")/.."

IMAGE=${IMAGE:-lantern:ci}
PIDS=()
cleanup() {
  docker logs lantern-image 2>&1 | tail -n 60 || true
  docker rm -f lantern-image >/dev/null 2>&1 || true
  kill "${PIDS[@]}" 2>/dev/null || true
}
trap cleanup EXIT

server/target/release/examples/mock_challenge 0.0.0.0:8998 & PIDS+=($!)
docker run -d --name lantern-image --network host --read-only -e APP_ADMIN_PASSWORD=image-check "$IMAGE" >/dev/null

for _ in $(seq 1 60); do curl -fs -o /dev/null http://127.0.0.1:9696/api/health && break; sleep 2; done
curl -fsS http://127.0.0.1:8191/ | tee /dev/stderr | jq -e '.msg == "FlareSolverr is ready!"' >/dev/null

answer=$(curl -sS -X POST -H 'content-type: application/json' http://127.0.0.1:8191/v1 \
  -d '{"cmd":"request.get","url":"http://127.0.0.1:8998/page","maxTimeout":60000}')
echo "$answer" | jq '{status, message, url: .solution.url, cookies: [.solution.cookies[]?.name], userAgent: .solution.userAgent}'
echo "$answer" | jq -e '.status == "ok" and .message == "Challenge solved!"' >/dev/null
echo "$answer" | jq -e '.solution.response | contains("Show.S01E01")' >/dev/null
echo "$answer" | jq -e '[.solution.cookies[].name] | index("cf_clearance") != null' >/dev/null
echo "The image's browser got past the check."
