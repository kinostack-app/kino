# syntax=docker/dockerfile:1.7
#
# Release Dockerfile for kino. Headless build only — the tray feature
# is dropped (`--no-default-features`) because there's no GUI inside a
# container to render to. Multi-stage to keep the runtime image small.
#
# Built and pushed to ghcr.io/kinostack-app/kino by the Docker job in
# .github/workflows/channels.yml. The dev Dockerfile lives at
# `Dockerfile.dev` and is not what end users pull.

# ─── Builder ─────────────────────────────────────────────────────────
FROM rust:1.94-bookworm AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake clang mold pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy only what's needed to build the workspace. Frontend assets are
# embedded by the backend build script; copy them too so the build
# can find them.
COPY backend/ ./backend/
COPY frontend/ ./frontend/
COPY README.md LICENSE ./

WORKDIR /build/backend
ENV CARGO_TARGET_DIR=/build/target \
    CARGO_PROFILE_RELEASE_LTO=fat \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_STRIP=symbols
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
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 8080

USER kino
ENTRYPOINT ["/usr/bin/kino", "serve"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD wget -qO /dev/null http://localhost:8080/api/v1/status || exit 1
