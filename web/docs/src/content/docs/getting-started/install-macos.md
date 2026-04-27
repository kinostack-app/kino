---
title: Install on macOS
description: Install kino on macOS via Homebrew, .pkg installer, or the .app bundle. Apple Silicon and Intel both supported.
---

## Homebrew (recommended)

```sh
brew tap kinostack-app/kino
brew install kino
```

Homebrew installs the binary, drops it on `PATH`, and configures the
launchd service. Start it with:

```sh
brew services start kino
```

Open `http://localhost:8080` and run through the setup wizard.

## `.pkg` installer

Download `kino-<version>-{aarch64,x86_64}-apple-darwin.pkg` from the
latest [GitHub release](https://github.com/kinostack-app/kino/releases/latest)
and double-click to launch the installer.

The .pkg installs the binary to `/usr/local/bin/kino`, drops the
`tv.kino.daemon` LaunchDaemon plist into `/Library/LaunchDaemons/`,
and starts the service. Stop / restart via:

```sh
sudo launchctl unload /Library/LaunchDaemons/tv.kino.daemon.plist
sudo launchctl load -w /Library/LaunchDaemons/tv.kino.daemon.plist
```

Logs:

```sh
tail -f /var/log/kino/stderr.log
```

## Tarball (no installer)

```sh
curl -fsSL https://github.com/kinostack-app/kino/releases/latest/download/kino-aarch64-apple-darwin.tar.gz | tar xz
./kino serve
```

To register as a system service from the tarball install:

```sh
sudo ./kino install-service
```

You'll see the standard macOS admin prompt — kino uses `osascript`
to elevate, the same way other native apps do.

## Gatekeeper

Direct-download builds aren't signed with an Apple Developer
certificate (paid program — deferred to v1+). On first run macOS
will refuse to launch with "kino can't be opened because Apple
cannot check it for malicious software."

Workaround:

1. Right-click the `.pkg` (or the binary) → **Open**
2. Click **Open** in the dialog that appears
3. macOS remembers the choice — subsequent launches just work

The `brew install kino` route avoids this; Homebrew packages don't
trigger Gatekeeper.

## FFmpeg

```sh
brew install ffmpeg
```

kino expects FFmpeg on `PATH`. The Homebrew formula declares it as
a dependency so this is automatic. If you installed via .pkg or
tarball, install FFmpeg separately.

## Configuring the data directory

| Mode | Path |
|---|---|
| LaunchDaemon (system service) | `/var/lib/kino/` |
| User-mode | `~/Library/Application Support/Kino/data/` |

Override via `--data-path /path/to/dir` or `KINO_DATA_PATH=/path/to/dir`.
