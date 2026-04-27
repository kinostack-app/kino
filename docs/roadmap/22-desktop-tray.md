# Desktop tray

> **Status (2026-04-27): ~70% shipped.** `kino tray` runs live —
> single-instance lock (`tray/lock.rs`), 5s `/api/v1/status` poll,
> health-coloured icon, two-item menu — all behind the `tray` Cargo
> feature in `tray/run.rs` (~275 LOC). Wired in `main.rs` subcommand
> dispatch. **Outstanding:** `install-tray` / `uninstall-tray` are
> bail stubs in `tray/stub.rs` pending the `auto-launch` integration.
> Doc moves to `subsystems/` once those land.

A system-tray / menu-bar surface for Kino on Windows, macOS, and Linux.
Lives **inside the main `kino` binary** behind a Cargo feature, not a
separate companion executable. Runs in the user's GUI session, talks to
the local Kino server over the existing HTTP + WebSocket APIs.

## Scope

**In scope:**
- `kino tray` subcommand inside the single binary, gated by the `tray` Cargo feature
- Auto-spawn of the tray on GUI sessions when the user runs `kino` with no subcommand (default-on)
- System-tray / menu-bar icon with health-derived status colour
- Two-item menu (`Open Kino in browser`, `Quit tray`) plus a non-interactive info line
- Native OS toasts bridged from the existing notification subsystem (`08-notification.md`)
- Single-instance enforcement (one tray per user session)
- Native-installer integration: tray autostart enabled by default in the Windows / macOS / `.deb` packages, opt-out via installer checkbox or post-install Settings toggle
- `kino install-tray` / `kino uninstall-tray` for tarball + `cargo install` users (the only path where installers don't handle it)

**Out of scope:**
- Wrapped webview / Tauri-style desktop app — browser stays the frontend
- Service start/stop in the tray menu — service control stays in the OS service manager
- Settings UI in the tray — everything configurable lives in the web UI
- Submenus for queue / activity / library — the web UI covers these
- Custom icon badges (counts, glyphs beyond colour) — keep the tray visually minimal
- Mobile equivalent — phones have no tray
- Bundled custom browser — system default handler is the target

## Architecture

### Single binary, multiple modes

Kino ships as one executable per platform. Mode is selected by subcommand:

| Invocation | Behaviour |
|---|---|
| `kino` (no args, GUI session detected) | Starts the tray; if no local server is reachable, spawns `kino serve` as a child process |
| `kino` (no args, headless session) | Equivalent to `kino serve` — runs the server in the foreground |
| `kino serve` | Runs the HTTP server only, no tray. What the systemd unit / launchd plist / Windows Service calls |
| `kino tray` | Runs the tray only, attaches to a server reachable on `http://localhost:{KINO_PORT}` |
| `kino install-service` | Tarball-fallback: writes the platform service descriptor and starts it |
| `kino install-tray` | Tarball-fallback: writes the per-user autostart entry and starts the tray now |
| `kino uninstall-service` / `kino uninstall-tray` | Inverse of the above |

GUI detection (`kino` with no args):

- **Windows**: `GetConsoleProcessList` — the binary was launched from a GUI shell vs a console.
- **macOS**: presence of a windowing context (always assume GUI for `kino`; users wanting headless run `kino serve` explicitly or use the LaunchDaemon).
- **Linux**: `DISPLAY` or `WAYLAND_DISPLAY` set in the environment.

### Cargo feature gating

```toml
[features]
default = ["tray"]
tray = ["dep:tray-icon", "dep:notify-rust", "dep:auto-launch", "dep:tao"]
```

The `tray` feature pulls in the GUI crates. Users who only need the
server (Pi appliance image, Docker container, headless self-hosters)
build with `--no-default-features` and the binary loses the tray
subcommands at compile time. Same source tree, different artefacts:

| Artefact | Build |
|---|---|
| `kino` (desktop, all installers) | default features → tray included |
| `kino` (Docker image, Pi image) | `--no-default-features` → server-only |

The `kino tray` and `kino install-tray` subcommands are `#[cfg(feature = "tray")]`
and absent from `--help` in server-only builds.

### Talking to the server

Tray polls `GET /api/v1/status` every 5 seconds (the public,
unauthenticated readiness endpoint also used by Docker healthchecks)
and subscribes to the existing WebSocket event stream for
notifications. No bespoke IPC, no new protocol.

**Why `/api/v1/status` and not `/api/v1/health`?** The Health
dashboard endpoint is auth-protected and the tray has no clean way to
discover the API key today (env-var passthrough from the desktop
session is unreliable; a per-user "tray token" file is a follow-up
design). `/api/v1/status` carries enough — `status`, `setup_required`,
`warnings[]` — to derive the four-state colour. Promotion to the
richer `/api/v1/health` payload is gated on the credential-pickup
mechanism landing.

## 1. Icon and menu

### Icon states

Derived from the `/api/v1/status` payload:

| Condition | Icon |
|---|---|
| `status == "ok"`, no warnings, setup complete | Green |
| `status == "ok"` with warnings or `setup_required` | Amber |
| `status != "ok"` (HTTP 5xx, payload reports an error) | Red (static; gentle pulse deferred — see below) |
| Service unreachable (connection refused / timeout) | Grey / outlined |

**Pulse animation deferred.** The audited `tray-icon` 0.21 API surface
exposes `set_icon(Some(Icon))` for swaps but no built-in animation
loop, so a pulse needs a custom timer + alpha interpolation. Static
red is the v0 behaviour. When pulse lands it must respect
`prefers-reduced-motion`.

### Menu

```
┌─────────────────────────────┐
│ Kino                        │
│ Status: Running ✓           │   ← info lines, non-interactive
│ Version 0.4.2               │
├─────────────────────────────┤
│ Open Kino in browser        │   ← primary action
├─────────────────────────────┤
│ Quit tray                   │   ← exits tray process only
└─────────────────────────────┘
```

**Left-click behaviour (audit verdict 2026-04-26):** the `tray-icon` 0.21
API exposes menu-item activation via the global `MenuEvent` channel
but does not expose a separate "left-click on icon, no menu shown"
event. So the spec's Dropbox/Discord-style "left-click opens browser
directly" isn't achievable without dropping to platform-native hooks
(`WM_LBUTTONUP` on Windows, X11/Wayland directly on Linux).

For v0 we ship the macOS-style "click shows menu" behaviour on
**every** platform — the `Open Kino in browser` menu item is the
primary action. This adds one extra click on Windows / Linux vs the
spec's original intent. Revisit if that friction matters; the fix is
either (a) a `tray-icon` PR adding a `LeftClick` event variant, or
(b) hand-rolled per-OS hooks bypassing the crate.

The browser target is `http://localhost:{port}` where `port` comes from
the health payload, not a hardcoded constant. Falls back to `8080` when
the service is unreachable.

"Quit tray" exits the tray process only. Users who want to stop the
service use the OS service manager (`systemctl stop kino`,
`launchctl unload`, `services.msc`).

## 2. Native OS notifications

The tray subscribes to the WebSocket event stream and surfaces matching
events as OS toasts via `notify-rust` (DBus on Linux,
`UNUserNotificationCenter` on macOS, Toast XML on Windows).

| Event | Example toast |
|---|---|
| Import succeeded | `Kino · Added: Succession S04E08` |
| Download failed | `Kino · Failed: The Bear S03E01` |
| New episode aired | `Kino · New: Severance S02E05 is out` |
| VPN reconnected | `Kino · VPN reconnected` |
| Token expired (Trakt, etc.) | `Kino · Action needed: Trakt disconnected` |

Clicking a toast opens the browser to the relevant context via
`/api/v1/notifications/{id}/target` (deep-link URL). Per-event-type
filtering and the desktop-toasts master switch live in
`Settings → Notifications` — not in the tray menu.

If the tray isn't running, notifications fall back to the existing
web-UI / Discord / ntfy channels — users without tray still get
notified through their browser push subscription or webhooks.

## 3. Platform specifics

### Windows

- **Tray API**: shell notification area via `tray-icon` (StatusNotifier style)
- **Icon format**: `.ico` with multiple resolutions (16, 24, 32, 48)
- **Autostart**: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` via the `auto-launch` crate. The `.msi` installer ticks this on by default; `kino install-tray` writes the same key for tarball users
- **Toasts**: Toast Notifications API via `notify-rust`
- **Behaviour**: click shows menu (see "Left-click behaviour" caveat above — `tray-icon` 0.21 limitation)

### macOS

- **Menu-bar API**: `tray-icon` (uses `NSStatusBar`)
- **Info.plist**: `LSUIElement = true` so the binary has no Dock icon, no app-switcher entry. The `.dmg` ships `Kino.app` with this set
- **Autostart**: user LaunchAgent at `~/Library/LaunchAgents/tv.kino.tray.plist`. The `.app` registers this on first launch; `kino install-tray` writes it for tarball users
- **Toasts**: `UNUserNotificationCenter` via `notify-rust`. First toast triggers the OS permission prompt, granted once
- **Behaviour**: click shows menu (menu-bar convention)
- **Icon format**: template-style monochrome PNG, OS-tinted for light/dark mode

### Linux

The asymmetric case. Tray API is not a single standard.

- **Tray API**: `tray-icon` uses StatusNotifierItem (SNI) over D-Bus, falling back to XEmbed
- **Works out of the box on**: KDE Plasma, Cinnamon, XFCE, MATE, Budgie, Unity
- **Requires an extension on**: vanilla GNOME (`AppIndicator and KStatusNotifierItem Support`). Documented; we don't promise tray on stock GNOME
- **Doesn't work on**: tiling WMs without a panel bar (i3, sway, hyprland) unless the user runs a tray applet (`waybar`, `stalonetray`, etc.)
- **Autostart**: `~/.config/autostart/kino-tray.desktop` — freedesktop.org standard
- **Toasts**: D-Bus notifications via `notify-rust` — universally supported
- **Icon format**: PNG at multiple sizes; freedesktop.org theme-aware naming

### Sandboxed packaging caveats (Flatpak / Snap)

If we ship through Flatpak or Snap (Tier 2 channels in doc 21), the
manifest must:

- **Flatpak**: declare `--talk-name=org.kde.StatusNotifierWatcher` and `--talk-name=org.freedesktop.Notifications` finish-args
- **Snap**: plug the `system-tray` and `desktop-notifications` interfaces

Both work but require manifest tweaks per-channel. Doesn't change the
binary.

## 4. Install and lifecycle

### Default install paths (Tier 1 channels — the common case)

Each native package handles tray autostart as part of the install:

| Channel | Tray autostart on install | Opt-out |
|---|---|---|
| Windows `.msi` / `.exe` | Yes (installer checkbox, default ✓) | Uncheck during install, or Settings → Desktop |
| macOS `.dmg` (`Kino.app`) | Yes — opening the `.app` registers the LaunchAgent | Settings → Desktop, or `launchctl unload` |
| Homebrew Cask | Yes (Cask installs the `.app`) | Same as `.dmg` |
| Linux `.deb` / `.rpm` | postinst registers the systemd service. Tray autostart is per-user → first GUI launch of `kino` (e.g. via `.desktop` shortcut) prompts to enable | Settings → Desktop |
| AUR (`kino-bin`) | Same as `.deb` | Same |
| Pi appliance image | No (headless by default; `--no-default-features` build) | N/A |
| Docker / OCI | No (headless build) | N/A |

End users on these channels never run a tray-install command — the
package handles it.

### Fallback path (Tier 2: tarball / `cargo install`)

Power users who download the raw archive or `cargo install kino` use
the explicit subcommands:

```
kino install-service     # writes the systemd unit / launchd plist / Windows Service
kino install-tray        # writes the per-user autostart entry, starts the tray now
```

What `kino install-tray` does:

1. Detects the OS.
2. Creates the platform-appropriate autostart entry pointing at the running binary.
3. Starts the tray now (no need to log out/in).
4. Prints `Tray installed — look for the Kino icon in your {menu bar / system tray}.`

`kino uninstall-tray` removes the autostart and kills the running tray.

### Single-instance enforcement

Tray acquires a per-user lock on startup:

- **Linux/macOS**: `flock` on `$XDG_RUNTIME_DIR/kino-tray.lock` (or `/tmp` fallback)
- **Windows**: named mutex `Global\KinoTray-{username}`

If the lock is held, the new invocation exits cleanly with
`Tray is already running.` Prevents duplicate icons on multi-session
systems (Windows Fast User Switching, macOS multi-user, Linux SSH + GUI
both running).

### Startup and shutdown

- On start: lock acquired → icon created → health poll started → WebSocket connected → notification subscriber armed
- On service unreachable: icon goes grey-outlined, status line `Disconnected from service`. Poll continues with exponential backoff up to 60s. "Open Kino in browser" still attempts the default URL
- On user logout: tray process exits cleanly (OS handles this). Service, running as SYSTEM/root, is unaffected
- On explicit Quit: lock released, process exits. No autostart change — tray returns on next login unless the user ran `kino uninstall-tray` or unticked the Settings toggle

## 5. Settings integration

The web UI gains one section under `Settings → Desktop`:

> **Show Kino in your menu bar / system tray?**
>
> [ Toggle ] Enable tray
> [ Toggle ] Start tray automatically when I sign in
>
> *Tip: on GNOME you may need the [AppIndicator extension](...) to see the icon.*

The toggles call backend endpoints that wrap `kino install-tray` /
`kino uninstall-tray`, so the user-side experience is "flip the
switch." On platforms where the installer already enabled it, both
toggles are on by default; flipping off removes the autostart entry
without uninstalling the package.

Per-event-type notification filters live in
`Settings → Notifications` alongside Discord/ntfy/webhook toggles —
the tray is one consumer of that shared notification routing, not its
own settings surface.

## 6. Configuration

Minimal. No tray-specific Config table — everything configurable lives
in the existing web UI's notification settings.

| Setting | Location | Default |
|---|---|---|
| Tray autostart | `Settings → Desktop` | on (Tier 1 channels), off (headless builds) |
| Desktop toasts enabled | `Settings → Notifications → Desktop toasts` | on (when tray is installed), off otherwise |
| Per-event filters | `Settings → Notifications` (shared) | sensible defaults |
| Health-poll frequency | not configurable — hardcoded 5s | — |

Tray itself is stateless. Polls the service for everything, renders,
exits on quit. Survives service restarts (icon goes grey-outlined
briefly, then green).

## Entities touched

- **Reads (via Kino API):** `/api/v1/health`, notification events from the WebSocket stream
- **Writes:** none in Kino's DB. Per-user autostart entry on the OS (filesystem/registry)
- **Creates (on install):** autostart file/registry entry, lock file path

## Dependencies

New (all behind the `tray` feature):

- `tray-icon` — cross-platform tray/menu-bar icon (Tauri team)
- `notify-rust` — cross-platform native notifications
- `auto-launch` — cross-platform per-user autostart management
- `tao` — windowing event loop required by `tray-icon`

Existing (already in the workspace): `reqwest`, `tokio-tungstenite`, `clap`, `tokio`.

No new system binaries. No new daemons.

## Error states

- **`tray-icon` fails to initialise** (Linux SNI unavailable, GNOME without extension) → log warning, exit silently rather than hanging. Document the prerequisite
- **Service unreachable on startup** → tray still appears grey-outlined with "Disconnected" status. Retries with backoff
- **WebSocket disconnected mid-session** → fall back to poll-only mode; no notifications until reconnect; icon doesn't change
- **Notification permission denied (macOS)** → log once, don't re-prompt, don't crash. Toast delivery silently no-ops
- **Autostart entry points at a moved binary** (user relocated install after `install-tray`) → tray exits on launch with a clear error; user re-runs `kino install-tray` from the new location, or re-runs the platform installer
- **Duplicate tray instance attempt** → second invocation exits cleanly. No zombie state

## Known limitations

- **Vanilla GNOME requires an extension.** Documented; not our problem to solve. AppIndicator has 2M+ users — it's the norm for GNOME self-hosters
- **Tiling WMs need a tray applet configured.** Also documented; self-selection (if you run Hyprland you know what a tray applet is)
- **No custom icon artwork per-status beyond colour.** No "3 downloads active" badges; web UI surfaces detail
- **Tray process isn't restarted automatically if it crashes mid-session.** Next login brings it back via autostart. A watchdog is a possible later addition
- **Wayland + SNI occasional glitches.** KDE Wayland generally fine; some compositors show stale icons. Mostly out of our hands
- **Linux icon theming.** Icon respects system theme on KDE, less consistent elsewhere. Ship a reasonable default, don't over-engineer
- **No deep-link targets for notification clicks** until the service's notification subsystem includes a `target_url` field per notification — minor follow-up to `08-notification.md`
- **Unsigned-binary friction on macOS/Windows direct-download path.** Package-manager channels (Homebrew, winget, AUR) bypass it; `.dmg` and `.msi` direct-download users hit Gatekeeper / SmartScreen once. See `21-cross-platform-deployment.md` for the full unsigned-binary posture
