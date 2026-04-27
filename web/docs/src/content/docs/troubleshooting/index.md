---
title: Troubleshooting
description: Common issues and where to look first when kino isn't behaving.
---

Where to look when something's wrong, in priority order.

## Can't reach `http://localhost:8080`

1. **Is the service running?**

   - **Linux**: `sudo systemctl status kino`
   - **macOS**: `sudo launchctl print system/tv.kino.daemon`
   - **Windows**: `sc query kino`
   - **Docker**: `docker ps | grep kino`

   If it's stopped, start it. If it's failing to start, check logs.

2. **Are the logs telling you anything?**

   - **Linux**: `sudo journalctl -u kino -f`
   - **macOS**: `tail -f /var/log/kino/stderr.log`
   - **Windows**: Event Viewer → Windows Logs → Application
   - **Docker**: `docker logs -f kino`

3. **Is something else listening on port 8080?**

   ```sh
   # Linux / macOS
   sudo lsof -i :8080
   # Windows (admin PowerShell)
   Get-NetTCPConnection -LocalPort 8080
   ```

   Either stop the other process or change kino's port via
   `KINO_PORT=8090` in the environment / service descriptor.

4. **Firewall blocking the port?** See the firewall section in your
   platform's [install guide](../getting-started/install-linux).

## Setup wizard says "TMDB API error"

kino needs a TMDB Read Access Token to fetch movie / show metadata.
Get one free from
[themoviedb.org](https://www.themoviedb.org/settings/api) (Read
Access Token, not v3 API key) and paste into Settings → Metadata.

## Downloads stuck in "queued" forever

1. **Is the indexer test passing?** Settings → Indexers → click the
   indexer → "Test connection". Most failures are bad URL / API
   key / Cloudflare blocks.
2. **Is the download client running?** Settings → Downloads →
   Status should show the embedded session as healthy.
3. **Is your VPN required-but-failed?** kino fails closed — if VPN
   is enabled in your config but didn't connect, the download
   client refuses to start, to prevent IP leak. The dashboard
   surfaces this; toggle VPN off temporarily to test or fix the
   VPN config.

## "kino can't be opened" (macOS Gatekeeper)

Right-click the binary → **Open** → click **Open** in the dialog.
One-time. See [macOS install](../getting-started/install-macos/#gatekeeper)
for the longer explanation.

## "Windows protected your PC" (Windows SmartScreen)

Click **More info** → **Run anyway**. One-time. See
[Windows install](../getting-started/install-windows/#smartscreen)
for the longer explanation.

## GNOME tray icon doesn't show

GNOME 40+ removed system-tray support from the core shell.
Install the
[AppIndicator and KStatusNotifierItem extension](https://extensions.gnome.org/extension/615/appindicator-support/)
and the kino tray icon will appear after a session restart.

## Library / metadata directory full

Settings → Storage shows current usage. The image cache, trickplay
thumbnails, and backup archives can be cleared individually without
losing your library:

- `data/cache/` — image thumbnails. Safe to delete; will regenerate
- `data/trickplay/` — scrubbing previews. Safe to delete
- `data/backups/` — restore archives. Move elsewhere if you want
  long-term retention

## Where to ask for help

- [GitHub Discussions](https://github.com/kinostack-app/kino/discussions)
  for "how do I" + open-ended troubleshooting
- [GitHub Issues](https://github.com/kinostack-app/kino/issues)
  for bugs + feature requests

When reporting a bug, include:

- kino version (Settings → About, or `kino --version`)
- OS + version
- Install method (winget / brew / .deb / docker / etc.)
- The relevant log section (last ~100 lines around the failure)
