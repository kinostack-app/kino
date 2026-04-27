---
title: Install on Windows
description: Install kino on Windows 10 / 11 via winget, MSI, or portable archive. Native Service Control Manager integration.
---

## winget (recommended)

```powershell
winget install kinostack-app.kino
```

Installs the binary, registers `kino` as a Windows Service via the
Service Control Manager (SCM), and starts it. Open
`http://localhost:8080` and run through the setup wizard.

Service control:

```powershell
sc query kino
sc stop kino
sc start kino
```

Logs:

- **Event Viewer** → Windows Logs → Application (provider: kino)
- kino's own log SQLite table (Settings → Diagnostics in the UI)

## MSI installer

Download `kino-<version>-x86_64-pc-windows-msvc.msi` from the latest
[GitHub release](https://github.com/kinostack-app/kino/releases/latest)
and double-click to install. Same SCM service registration as the
winget route.

## Portable archive

```powershell
# In an Admin PowerShell window
Invoke-WebRequest https://github.com/kinostack-app/kino/releases/latest/download/kino-x86_64-pc-windows-msvc.zip -OutFile kino.zip
Expand-Archive .\kino.zip
cd kino
.\kino.exe serve
```

Register as a service from the portable archive (requires Admin):

```powershell
.\kino.exe install-service
```

You'll see the standard Windows UAC elevation prompt. To remove:

```powershell
.\kino.exe uninstall-service
```

## SmartScreen

Direct-download builds aren't yet signed with an Authenticode
certificate (paid CA — deferred to v1+). Windows SmartScreen will
warn "Microsoft Defender SmartScreen prevented an unrecognized app
from starting" on first run.

Workaround:

1. Click **More info**
2. Click **Run anyway**
3. SmartScreen remembers — subsequent launches just work

The `winget install kinostack-app.kino` route avoids this; winget
packages have their own reputation chain.

## FFmpeg

The MSI bundles FFmpeg. Portable / `cargo install` users need
FFmpeg on `PATH`:

```powershell
winget install ffmpeg
# or
choco install ffmpeg
```

## Firewall

kino listens on port `8080` by default. The MSI / winget install
adds a Windows Defender Firewall rule automatically. Manual route:

```powershell
New-NetFirewallRule -DisplayName "Kino" -Direction Inbound -Protocol TCP -LocalPort 8080 -Action Allow
```

## Tray + autostart

The MSI ships the desktop tray (system tray icon + menu) and adds a
per-user autostart shortcut so the tray launches at login.

To toggle the tray autostart manually:

```powershell
.\kino.exe install-tray     # enable
.\kino.exe uninstall-tray   # disable
```

## Configuring the data directory

| Mode | Path |
|---|---|
| Windows Service | `%ProgramData%\kino\data\` |
| User-mode | `%LocalAppData%\kino\data\` |

Override via `--data-path C:\path\to\dir` or `KINO_DATA_PATH=C:\path\to\dir`.
