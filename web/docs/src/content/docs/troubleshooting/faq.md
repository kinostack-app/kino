---
title: FAQ
description: Frequently-asked questions about kino's scope, design, and roadmap.
---

## What does kino do?

kino is a single-binary self-hosted media automation and streaming
server. It covers everything from "I want to follow this show"
through to "play it on the TV downstairs" without you having to
glue together a stack of separate services.

Concretely, that means:

- **Library + automation** — follow movies and shows, kino keeps
  them up to date
- **Search** — query Torznab-compatible indexers + a built-in
  Cardigann definition catalogue
- **Downloads** — embedded BitTorrent client; no external download
  client to wire in
- **Library management** — automatic file naming + organisation
  with hardlink import
- **Streaming + transcode** — direct play when possible, hardware
  transcode when not
- **Casting** — Chromecast support via a built-in custom receiver
- **Privacy networking** — built-in WireGuard client with an
  IP-leak killswitch

One config UI, one database, one log, one upgrade path.

## Why one binary?

Smaller surface area to operate. One config file, one log to look
at, one thing to back up, one thing to update. No inter-service
network configuration, no version-skew between components, no
"is this Service A's job or Service B's job" debugging.

## Does kino support direct play / direct stream?

Yes. Direct play (no transcode) is preferred whenever the playback
device's codec, container, and audio support match the source.
Transcode kicks in when:

- The browser or casting target can't decode the source codec
- The audio channel layout exceeds what the device supports
  (e.g. 5.1 → stereo for headphones)
- The container needs swapping (e.g. MKV → MP4 for browser playback)

See [Features → Streaming](../features/streaming) for the full
decision tree.

## What hardware transcoders are supported?

| Vendor | API | Status |
|---|---|---|
| NVIDIA | NVENC | ✓ |
| Intel | QuickSync (QSV) | ✓ |
| AMD | AMF / VAAPI | ✓ |
| Apple Silicon | VideoToolbox | ✓ |
| Raspberry Pi 4/5 | V4L2 stateless | ✓ |

Auto-detected on first launch. Override in **Settings → Playback →
Transcode**.

## How do I import an existing library?

Point kino's media path at your existing library directory in the
setup wizard. kino scans the structure and matches files to TMDB
entries. The wizard previews matches and lets you correct
mismatches before committing.

## Can I run kino behind a reverse proxy?

Yes. Set `KINO_BASE_URL=https://kino.your-domain.tld` in the
service environment, terminate TLS at your proxy
(Caddy / nginx / Traefik), and reverse-proxy `:8080` to the
upstream. WebSocket upgrades on `/api/v1/ws` need the standard
`Upgrade` / `Connection` header forwarding. See
[Setup → Reverse proxy](../setup/reverse-proxy) for the full
config snippets.

## Does kino phone home?

No. kino ships no telemetry, no analytics, no crash reporting,
no anonymous usage stats. The only outbound connections are the
ones you'd expect: TMDB for metadata, your configured indexers
for search, your configured VPN endpoint, and an opt-out daily
GitHub Releases API check for update-available notifications
(opt out in **Settings → Updates**).

See [Privacy](https://kinostack.app/privacy) for the full posture.

## Is there a mobile app?

Not yet. The web UI is responsive and works on phones and tablets;
the React app is also installable as a PWA. Native mobile clients
aren't in the v1 plan but aren't ruled out post-launch — see the
[roadmap](https://kinostack.app/roadmap).

## What's the license?

kino is **GPL-3.0-or-later**. Use it for any purpose, modify it,
fork it, redistribute it. The catch: any derivative work you
distribute publicly must remain under the same license. This
protects every kino user's freedom to inspect and modify the
software they run.

In plain terms:

- ✅ Run it for yourself, your family, your business — no restrictions.
- ✅ Modify it locally — no obligations.
- ✅ Fork it publicly — your fork must also be GPL-3.0 (or later).
- ❌ Build a closed-source product on top of kino's source — not allowed.
- ❌ Repackage kino as a paid SaaS without sharing your changes — not allowed.

See [Attributions](https://kinostack.app/attributions) for the
full third-party dependency + license list.

## Can I sponsor or contribute?

Sponsorship via the
[GitHub Sponsors page](https://github.com/sponsors/kinostack-app)
is the preferred channel. Code contributions: PRs welcome — read
[CONTRIBUTING.md](https://github.com/kinostack-app/kino/blob/main/CONTRIBUTING.md)
first.

## How fast is the release cycle?

Ship-when-ready. We don't follow a fixed monthly / quarterly
schedule. Patch releases land within a day for security fixes.
Minor releases land when meaningful features are stable. Major
releases will be rare pre-1.0; v1.0 itself is the "core surface
considered stable" milestone.
