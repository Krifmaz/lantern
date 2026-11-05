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

# The indexer definitions (Prowlarr/Indexers, schema v11) at build time, so Lantern has every site from the first
# start; it updates them in the data folder by itself afterwards.
FROM --platform=$BUILDPLATFORM alpine:3.22 AS defs
RUN apk add --no-cache curl tar \
 && mkdir -p /defs && curl -fsSL https://github.com/Prowlarr/Indexers/archive/refs/heads/master.tar.gz \
    | tar -xz -C /defs --strip-components=3 --wildcards '*/definitions/v11/*.yml' \
 && ls /defs | grep -c yml

# Distroless: no shell, no package manager, only glibc and CA certificates. Runs as the image's non-root user
# (65532) and writes only to /data, so it also runs with `docker run --read-only`.
FROM gcr.io/distroless/cc-debian13:nonroot
LABEL org.opencontainers.image.title="Lantern" \
      org.opencontainers.image.source="https://github.com/Krifmaz/lantern" \
      org.opencontainers.image.url="https://github.com/Krifmaz/lantern"
COPY --from=build /app /usr/local/bin/lantern
COPY --from=build --chown=65532:65532 /data /data
COPY --from=ui /src/web/dist /app/static
COPY --from=defs /defs /app/definitions
ARG APP_VERSION=
LABEL org.opencontainers.image.version="${APP_VERSION}"
ENV APP_DATA_DIR=/data APP_STATIC_DIR=/app/static APP_DEFINITIONS_DIR=/app/definitions APP_PORT=9696
USER nonroot
VOLUME /data
EXPOSE 9696
# no curl in a distroless image: the binary asks its own /api/health
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["/usr/local/bin/lantern", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/lantern"]
