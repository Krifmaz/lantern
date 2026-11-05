# Lantern (server + web app, one Rust binary). Builds for linux/amd64 and linux/arm64; the arm64 binary is
# cross-compiled on the build machine (fast), not emulated.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS build
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
      && rm -rf /var/lib/apt/lists/* && rustup target add aarch64-unknown-linux-gnu; \
    fi
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
ARG APP_VERSION=
WORKDIR /src
COPY server ./server
# the release notes the server builds in for What's new (server/build.rs reads ../docs/releases)
COPY docs/releases ./docs/releases
RUN case "$TARGETARCH" in arm64) T=aarch64-unknown-linux-gnu ;; *) T=x86_64-unknown-linux-gnu ;; esac \
 && { [ -n "$APP_VERSION" ] || unset APP_VERSION; } \
 && cd server && cargo build --release --locked --target "$T" && cp "target/$T/release/lantern" /app \
 && mkdir -p /data

# The web app (Leptos → WebAssembly), built once on the build machine with trunk, served at / by the server.
# Every text file also as <file>.br and <file>.gz next to it: the server sends the one the browser takes as it is.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS ui
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates brotli && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C /usr/local/bin trunk
WORKDIR /src
COPY web ./web
RUN cd web && trunk build --release --locked \
 && find dist -type f \( -name '*.js' -o -name '*.wasm' -o -name '*.css' -o -name '*.html' -o -name '*.svg' \
      -o -name '*.webmanifest' -o -name '*.json' \) -exec brotli -f -k -q 11 {} + -exec gzip -f -k -n -9 {} + \
 && find dist -name '*.br' | grep -q . && find dist -name '*.gz' | grep -q .

# Prowlarr's web frontend (ui/prowlarr, a snapshot fork, see UPSTREAM.md), built as Prowlarr builds it (yarn and
# webpack) and served by the server at /prowlarr/. Only the built static files go into the image; the source maps
# are left out.
FROM --platform=$BUILDPLATFORM node:22.23.4-trixie-slim AS prowlarr-ui
WORKDIR /src/ui/prowlarr
COPY ui/prowlarr/package.json ui/prowlarr/yarn.lock ui/prowlarr/.yarnrc ./
RUN yarn install --frozen-lockfile --network-timeout 600000
COPY ui/prowlarr ./
RUN yarn build --env production \
 && find _output/UI -name '*.map' -delete \
 && test -f _output/UI/index.html

# autobrr's web frontend (ui/autobrr, a snapshot fork of autobrr's web/, see UPSTREAM.md), built as autobrr's own
# Dockerfile builds it: Node 24.19.0, pnpm at the version its package.json pins (packageManager), through corepack,
# from the frozen lockfile, then tsc and vite. Served by the server at /autobrr/; only the built static files go into
# the image, the source maps are left out.
FROM --platform=$BUILDPLATFORM node:24.19.0-trixie-slim AS autobrr-ui
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
WORKDIR /src/ui/autobrr
COPY ui/autobrr/package.json ui/autobrr/pnpm-lock.yaml ui/autobrr/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY ui/autobrr ./
RUN pnpm run build \
 && find dist -name '*.map' -delete \
 && test -f dist/index.html

# The indexer definitions (Prowlarr/Indexers, schema v11) at build time, so Lantern has every site from the first
# start; it updates them in the data folder by itself afterwards.
FROM --platform=$BUILDPLATFORM alpine:3.22 AS defs
RUN apk add --no-cache curl tar \
 && mkdir -p /defs && curl -fsSL https://github.com/Prowlarr/Indexers/archive/refs/heads/master.tar.gz \
    | tar -xz -C /defs --strip-components=3 --wildcards '*/definitions/v11/*.yml' \
 && ls /defs | grep -c yml

# The IRC announce definitions (autobrr's), so Lantern knows every tracker's announce channels from the first start.
FROM --platform=$BUILDPLATFORM alpine:3.22 AS announce
RUN apk add --no-cache curl tar \
 && mkdir -p /announce && curl -fsSL https://github.com/autobrr/autobrr/archive/refs/heads/develop.tar.gz \
    | tar -xz -C /announce --strip-components=4 --wildcards '*/internal/indexer/definitions/*.yaml' \
 && ls /announce | grep -c yaml

# The browser for Cloudflare checks: Debian's Chromium headless shell and every file of the packages it needs
# (libraries, fonts, their settings), gathered into /out to copy into the distroless image, plus every library
# they link that Debian's base image already had (ldd). glibc is in both images already, from the same Debian
# release, so it isn't copied.
FROM debian:trixie-slim AS browser
RUN dpkg-query -W -f='${Package}\n' | sort > /base.txt \
 && apt-get update \
 && apt-get install -y --no-install-recommends chromium-headless-shell fonts-liberation fonts-dejavu-core \
 && dpkg-query -W -f='${Package}\n' | sort > /all.txt \
 && comm -13 /base.txt /all.txt > /added.txt \
 && mkdir /out \
 && for p in $(cat /added.txt); do dpkg -L "$p"; done | sort -u \
    | grep -vE '^/(var|usr/share/(doc|man|locale|lintian|bug|info|bash-completion))/' \
    | while read -r f; do if [ -f "$f" ] || [ -L "$f" ]; then cp -a --parents "$f" /out/; fi; done \
 && shell=$(find /usr/lib/chromium -maxdepth 1 -type f -perm -u+x \( -name headless_shell -o -name 'chromium-headless-shell' \) | head -n1) \
 && test -n "$shell" && { [ "$shell" = /usr/lib/chromium/headless_shell ] || cp -a "$shell" /out/usr/lib/chromium/headless_shell; } \
 && rm -rf /out/usr/bin /out/usr/sbin /out/bin /out/sbin \
 && find /out -type f \( -name '*.so*' -o -path '/out/usr/lib/chromium/*' \) -exec ldd {} \; 2>/dev/null \
    | awk '$2 == "=>" && $3 ~ /^\// {print $3}' | sort -u \
    | grep -vE '/(libc|libm|libdl|libpthread|librt|libresolv|libutil|ld-linux[^/]*)\.so' \
    | while read -r lib; do lib="$(readlink -f "$(dirname "$lib")")/$(basename "$lib")"; \
        [ -e "/out$lib" ] || cp -L --parents "$lib" /out/; done \
 && for d in lib lib64; do if [ -d "/out/$d" ] && [ ! -L "/out/$d" ]; then \
      mkdir -p "/out/usr/$d" && cp -a "/out/$d/." "/out/usr/$d/" && rm -rf "/out/$d"; fi; done \
 && /usr/lib/chromium/headless_shell --version 2>/dev/null || "$shell" --version

# Distroless: no shell, no package manager, only glibc and CA certificates. Runs as the image's non-root user
# (65532) and writes only to /data, so it also runs with `docker run --read-only`.
FROM gcr.io/distroless/cc-debian13:nonroot
LABEL org.opencontainers.image.title="Lantern" \
      org.opencontainers.image.source="https://github.com/Krifmaz/lantern" \
      org.opencontainers.image.url="https://github.com/Krifmaz/lantern"
COPY --from=build /app /usr/local/bin/lantern
COPY --from=build --chown=65532:65532 /data /data
COPY --from=ui /src/web/dist /app/static
COPY --from=prowlarr-ui /src/ui/prowlarr/_output/UI /app/ui/prowlarr
COPY --from=autobrr-ui /src/ui/autobrr/dist /app/ui/autobrr
COPY --from=defs /defs /app/definitions
COPY --from=browser /out/ /
COPY --from=announce /announce /app/announce
ARG APP_VERSION=
LABEL org.opencontainers.image.version="${APP_VERSION}"
ENV APP_DATA_DIR=/data APP_STATIC_DIR=/app/static APP_PROWLARR_UI_DIR=/app/ui/prowlarr APP_AUTOBRR_UI_DIR=/app/ui/autobrr APP_DEFINITIONS_DIR=/app/definitions APP_ANNOUNCE_DIR=/app/announce APP_PORT=9696 \
    APP_SOLVER_PORT=8191 APP_CHROMIUM=/usr/lib/chromium/headless_shell
USER nonroot
VOLUME /data
EXPOSE 9696 8191
# no curl in a distroless image: the binary asks its own /api/health
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["/usr/local/bin/lantern", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/lantern"]
