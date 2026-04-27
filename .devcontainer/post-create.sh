#!/bin/bash
set -euo pipefail

echo "=== kino post-create setup ==="

# Upgrade the docker CLI when the Debian-packaged one (20.10.x,
# API 1.41) is too old for the host docker daemon (needs API 1.44+
# on modern installs). Dropping the static CLI binary at
# /usr/local/bin wins on PATH without disturbing apt state. Skip
# when a recent binary is already in place (rebuilds survive it,
# re-runs are cheap).
if ! /usr/local/bin/docker version >/dev/null 2>&1; then
    echo "Installing newer docker CLI (the apt one is too old for the daemon API)..."
    DOCKER_CLI_VERSION="27.3.1"
    curl -fsSL -o /tmp/docker-cli.tgz \
        "https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_CLI_VERSION}.tgz"
    tar -xzf /tmp/docker-cli.tgz -C /tmp docker/docker
    mv /tmp/docker/docker /usr/local/bin/docker
    chmod +x /usr/local/bin/docker
    rm -rf /tmp/docker /tmp/docker-cli.tgz
    echo "  Installed docker CLI $(/usr/local/bin/docker --version)"
fi

# Ensure .env exists
if [ ! -f .env ]; then
    echo "Creating default .env..."
    cat > .env << 'EOF'
KINO_PORT=8080
KINO_DATA_PATH=./data
RUST_LOG=info
# No KINO_API_KEY: backend generates a random UUID on first boot.
# The dev SPA gets an AutoLocalhost session cookie via /bootstrap.
KINO_TMDB_API_KEY=
KINO_MEDIA_PATH=/workspace/data/library
KINO_DOWNLOAD_PATH=/workspace/data/downloads
EOF
    echo "  Created .env — add your TMDB API key"
fi

# Install frontend dependencies if stale
if [ ! -d frontend/node_modules ] || [ frontend/package.json -nt frontend/node_modules ]; then
    echo "Installing frontend dependencies..."
    cd frontend && npm ci && cd ..
fi

# Install web (kinostack.app site) dependencies if stale. Same
# pattern as frontend; the volume mount keeps node_modules off the
# host filesystem.
if [ ! -d web/node_modules ] || [ web/package.json -nt web/node_modules ]; then
    echo "Installing web dependencies..."
    cd web && npm ci && cd ..
fi

# Create data directories
mkdir -p data data/library data/downloads data/definitions

# Activate the plain-shell git hooks (.githooks/) so cargo fmt + biome
# run on commit, full quality gates run on pre-push. Idempotent — `git
# config` overwrite doesn't re-trigger anything. Contributors who
# prefer the prek / pre-commit framework path can opt out by running
# `git config --unset core.hooksPath` and using `.pre-commit-config.yaml`
# instead. See CONTRIBUTING.md.
if [ -d .githooks ]; then
    ./.githooks/setup >/dev/null
    echo "  Activated git hooks (.githooks/) — pre-commit + pre-push wired"
fi

# Wait for the backend to come up so the "setup complete" banner
# isn't a lie. The kino-backend container takes ~30-90s to compile
# from a cold cache; healthcheck against /api/v1/status (the public,
# no-auth readiness endpoint) confirms it's actually serving.
echo "Waiting for backend to come up (kino-backend autobuilds via watchexec)..."
for i in $(seq 1 60); do
    if curl -sf http://localhost:8080/api/v1/status >/dev/null 2>&1; then
        echo "  ✓ backend responding on :8080 (after ${i}s)"
        break
    fi
    sleep 5
done

echo ""
echo "=== Setup complete ==="
echo ""
echo "Services:"
echo "  kino UI:        http://localhost:5173    (in-app React SPA)"
echo "  kino API:       http://localhost:8080    (Rust backend + embedded SPA)"
echo "  Swagger:        http://localhost:8080/api/docs/"
echo "  kinostack site: http://localhost:4321    (Astro: landing, docs, /cast/)"
echo ""
echo "Commands (from backend/):"
echo "  just logs           # Backend logs"
echo "  just logs-frontend  # Frontend logs"
echo "  just logs-web       # kinostack web site logs"
echo "  just restart        # Restart backend"
echo "  just restart-web    # Restart web site (picks up astro.config changes)"
echo "  just reset          # Delete all data, fresh start"
echo "  just status         # Check all services"
echo ""
echo "Indexers are configured in Settings → Indexers (500+ built-in definitions)."
echo "No external Prowlarr/Jackett needed."
echo ""
