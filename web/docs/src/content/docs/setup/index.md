---
title: Setup
description: Configure indexers, quality profiles, VPN, storage layout, and reverse-proxy access for your kino install.
---

The first-run wizard covers the bare minimum to get kino downloading
and playing back. Everything else lives in **Settings** and is
documented here. Each page is independent — pick the bits that
match your setup.

## Pages in this section

| Page | What it covers |
|---|---|
| [Indexers](./indexers) | Adding Torznab endpoints and the 500+ built-in indexer definitions |
| [VPN](./vpn) | Routing downloads through the built-in WireGuard client |
| [Quality profiles](./quality-profiles) | Picking the resolutions, sources, and codecs kino prefers |
| [Storage](./storage) | Data path, library layout, download path, hardlinks |
| [Reverse proxy](./reverse-proxy) | Putting kino behind nginx, Caddy, or Traefik for HTTPS and a real hostname |

If you haven't installed kino yet, start with the
[Quickstart](../getting-started) and pick your platform. Once
the server's running on `http://localhost:8080`, come back here.

## Where the settings live

Open the gear icon in the top bar, or go straight to
`http://<host>:8080/settings`. The pages below mirror the order of
the settings tabs:

- **Library** — TMDB token, library path, naming templates, hardlinks
- **Indexers** — search providers
- **Quality** — quality profiles
- **Downloads** — concurrent limit, seed ratio/time, bandwidth caps
- **VPN** — WireGuard credentials, killswitch, port forwarding
- **Integrations** — Trakt, MDBList, OpenSubtitles, webhook targets
- **Backup** — schedule, retention, manual backup/restore

Every setting is editable at any time. Changes apply on save — no
restart unless the page tells you otherwise.

## Common issues

If something on a settings page won't save, check
[Troubleshooting](../troubleshooting) — the FAQ covers the
recurring traps (missing TMDB token, library path the service user
can't read, VPN handshake failures).
