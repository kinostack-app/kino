# syntax=docker/dockerfile:1.7
#
# Release Dockerfile for kino. Headless build only — the tray feature
# is dropped (`--no-default-features`) because there's no GUI inside a
# container to render to. Multi-stage to keep the runtime image small.
#
# Built and pushed to ghcr.io/kinostack-app/kino by the Docker job in
# .github/workflows/channels.yml. The dev Dockerfile lives at
# `Dockerfile.dev` and is not what end users pull.

# ─── Frontend build ──────────────────────────────────────────────────
# kino's binary embeds `frontend/dist/` via rust-embed (see
# backend/crates/kino/src/spa.rs). Build the SPA in a Node stage
# and copy the dist output into the backend build context.
FROM node:22-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci
COPY frontend/ ./
RUN npm run build && test -f dist/index.html

# ─── Backend build ───────────────────────────────────────────────────
FROM rust:1.94-bookworm AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake clang mold pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Backend source + the pre-built frontend dist (NOT the frontend
# source — npm install would re-run otherwise). KINO_SKIP_FRONTEND_BUILD
# tells build.rs to trust the dist we just baked.
COPY backend/ ./backend/
COPY --from=frontend /build/frontend/dist /build/frontend/dist
COPY README.md LICENSE ./

WORKDIR /build/backend
ENV CARGO_TARGET_DIR=/build/target \
    CARGO_PROFILE_RELEASE_LTO=fat \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_STRIP=symbols \
    KINO_SKIP_FRONTEND_BUILD=1
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --no-default-features -p kino \
    && cp /build/target/release/kino /usr/local/bin/kino

# ─── Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ffmpeg ca-certificates iproute2 wget \
    && rm -rf /var/lib/apt/lists/*

# Service user — same UID/GID convention as the .deb postinst.
RUN groupadd --system kino \
    && useradd --system --gid kino --home-dir /data --no-create-home --shell /usr/sbin/nologin kino

COPY --from=builder /usr/local/bin/kino /usr/bin/kino

ENV KINO_PORT=8080 \
    KINO_DATA_PATH=/data \
    KINO_NO_OPEN_BROWSER=1 \
    KINO_INSTALL_KIND=docker \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 8080

USER kino
ENTRYPOINT ["/usr/bin/kino", "serve"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD wget -qO /dev/null http://localhost:8080/api/v1/status || exit 1
