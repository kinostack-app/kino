---
title: Install with Docker
description: Run kino as a container via Docker / Compose. Multi-arch (amd64 + arm64) image on GitHub Container Registry.
---

The Docker image wraps kino's headless Linux build (no system tray;
that lives outside the container). Multi-arch — the same image tag
works on amd64 + arm64 (Pi, Apple Silicon hosts running Docker).

## Quick start

```sh
docker run -d \
  --name kino \
  -p 8080:8080 \
  -v ./data:/data \
  -v /path/to/media:/media \
  -e TZ=Europe/London \
  ghcr.io/kinostack-app/kino:latest
```

Open `http://localhost:8080` and run through the setup wizard. Set
the media path to `/media` (the mountpoint inside the container).

## docker-compose

```yaml
services:
  kino:
    image: ghcr.io/kinostack-app/kino:latest
    container_name: kino
    restart: unless-stopped
    ports:
      - "8080:8080"
    volumes:
      - ./data:/data
      - /path/to/media:/media
    environment:
      - TZ=Europe/London
      # Uncomment for verbose logging
      # - RUST_LOG=info,kino=debug
```

```sh
docker compose up -d
docker compose logs -f
```

## Tags

| Tag | Points at |
|---|---|
| `latest` | Most recent stable release |
| `<version>` (e.g. `0.2.0`) | Pinned release — recommended for prod |
| `<major>.<minor>` (e.g. `0.2`) | Minor-pinned, picks up patch releases |

We don't ship `nightly` / `dev` tags pre-1.0 — image tags only land
when a real GitHub Release fires.

## VPN inside Docker

kino's built-in WireGuard client works inside the container if you
grant it `NET_ADMIN`:

```yaml
services:
  kino:
    # ...
    cap_add:
      - NET_ADMIN
    devices:
      - /dev/net/tun
```

The VPN stays internal to the container — your host's networking
isn't touched. Configure the VPN provider in Settings → Downloads →
VPN once kino is running.

If you'd rather route through an external WireGuard / VPN sidecar
container instead, that works too — set `network_mode:
"service:<your-vpn-container>"` and disable kino's internal VPN.

## Hardware transcode

The Docker image is built without GPU drivers by default (NVENC,
VAAPI, QSV add ~200 MB and aren't useful on most user systems). For
hardware transcode in containers, mount the host's
`/dev/dri/renderD128` device and run with the host's vendor driver
already installed:

```yaml
services:
  kino:
    devices:
      - /dev/dri:/dev/dri
    group_add:
      - "104"   # video group; check `getent group video` on your host
```

Verify in Settings → Playback → Transcode that the expected encoder
shows up.

## Updating

```sh
docker compose pull
docker compose up -d
```

kino runs database migrations on startup; the new container picks
up the existing `/data` volume and migrates in place.

## Backup

Volume mount the `/data` directory — that's the entire kino state
(SQLite DB, librqbit session, image cache, backups). Snapshot the
host volume + your media path is the full backup.
