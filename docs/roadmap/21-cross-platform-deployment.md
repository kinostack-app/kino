# Cross-platform deployment

> **Primary shipping model.** Native single binary per platform,
> distributed through every reasonable package channel for that
> platform — that's the supported install path for the initial public
> release. Docker is one channel among many for users who already run
> their stack in containers.
>
> **Status (2026-04-26):** code-complete for the shippable surface;
> not yet exercised end-to-end against a real release tag.
>
> What's wired:
> - cargo-dist config + `release.yml` + `channels.yml` + per-channel
>   templates (AUR PKGBUILD, pi-gen `stage-kino`, `Dockerfile`,
>   `cargo-deb` / `cargo-generate-rpm` metadata, debian/rpm postinst
>   scripts)
> - **Service install** — Linux (systemd, both user + system),
>   macOS (launchd + osascript admin prompt), Windows (SCM via the
>   `windows-service` crate, with Service Recovery actions)
> - **Windows SCM dispatcher** in `service_runner.rs` — translates
>   `SERVICE_CONTROL_STOP` into a graceful axum shutdown
> - **AppImage** — wired in `release.yml` for x86_64 + aarch64,
>   AppDir layout under `packaging/appimage/`
> - **Channel-publish gating** — winget + AUR jobs in `channels.yml`
>   skip with a friendly notice when their secrets aren't yet
>   configured (so pre-launch releases are green, not red)
> - cargo-dist 0.30.0 → 0.31.0, `github-attestations = true`,
>   `etcetera` crate for per-OS path resolution
>
> What's not yet exercised:
> - No real release tag has fired the pipeline (cut `v0.0.0` for a
>   dry-run as the next deliberate step)
> - macOS + Windows service-install code is compile-verified by
>   `cross-os.yml` (clippy on Windows + macOS runners), not yet
>   runtime-tested on a real host
> - Channel publishing secrets (`HOMEBREW_TAP_TOKEN`,
>   `WINGET_TOKEN`, `AUR_SSH_PRIVATE_KEY`, `GPG_PRIVATE_KEY`) +
>   `homebrew-kino` tap repo not created — pending org migration

Ship Kino as a native single binary on Linux, macOS, and Windows.
"Install through your package manager → it works" on every supported
platform, including built-in VPN. Direct-download `.msi` / `.dmg` /
`.deb` / `.rpm` are first-class for users who don't use package
managers.

## Design principles

- **Single binary, multiple build profiles.** One Rust codebase, cross-compiled to Linux x86_64/ARM64, macOS x86_64/ARM64, Windows x86_64. The desktop build includes the tray (`--features tray`); the headless build (Pi image, Docker) drops it
- **Native-package-first, not "download and run a script."** Each platform gets the install experience users already know — `winget install kino`, `brew install kino`, `apt install kino`, `Kino.app` in `/Applications`
- **Native service on each OS.** Native packages register the service as part of install. Tarball / `cargo install` users get `kino install-service` as a fallback
- **Tray default-on for the desktop builds.** Installer checkbox (Windows), `.app` default behaviour (macOS), per-user autostart hint (Linux). Opt-out via Settings → Desktop. Headless builds drop the tray entirely
- **Bundled FFmpeg.** Ship a platform-specific FFmpeg alongside `kino` in every archive. Follows the Jellyfin pattern (`jellyfin-ffmpeg`) — no user-side package install required
- **Built-in VPN works everywhere.** Not Linux-only. BoringTun + the `tun` crate already abstract the TUN primitive; remaining per-OS work is default-route handling, DNS, and service privileges
- **No paid signing at launch.** Direct-download users see one Gatekeeper / SmartScreen warning; package-channel users see nothing. Re-evaluate Apple Developer Program ($99/year) once user volume justifies it. See §"Unsigned-binary posture"
- **cargo-dist drives the release.** One workspace config produces the archives, MSI (Windows), Homebrew tap formula, shell + PowerShell install scripts, GitHub Release in one CI run. Auxiliary channels (winget, AUR, Flathub, Snap, etc.) are thin manifest-PR jobs that point at those release artefacts. (cargo-dist 0.31 has no `.pkg` installer; macOS users get shell or Homebrew.)

## Platform matrix

| Platform | Status | Service | VPN | HW transcode | Notes |
|---|---|---|---|---|---|
| Linux x86_64 | Tier 1 | systemd | boringtun | NVENC / VAAPI / QSV | Primary development target |
| Linux ARM64 | Tier 1 | systemd | boringtun | V4L2 (Pi) / VAAPI | Pi 5, Graviton, Apple Silicon Linux; Pi also ships an appliance image (§7) |
| macOS ARM64 | Tier 1 | launchd | boringtun + utun | VideoToolbox | Apple Silicon |
| macOS x86_64 | Tier 1 | launchd | boringtun + utun | VideoToolbox | Intel Macs |
| Windows x86_64 | Tier 1 | Windows Service | boringtun + wintun | NVENC / QSV / AMF | Native, no WSL2 |
| Windows ARM64 | Tier 2 | — | — | — | Build from source, no official binary v1 |
| FreeBSD / OpenBSD | Tier 3 | — | — | — | Community-maintained |

"Tier 1" = built and published on every release through CI, install
flow documented, tested before release.

## 1. Shared architecture

Re-used across all platforms with zero per-OS variation:

- **BoringTun** userspace WireGuard — pure Rust, no platform dependency
- **`tun` crate** for TUN device creation — abstracts Linux `/dev/net/tun`, macOS `utun`, Windows `wintun`
- **librqbit** with `bind_device_name` for torrent socket binding — Linux supported upstream; macOS/Windows verification flagged in §"Known unknowns"
- **NAT-PMP / port forwarding** — protocol-level, not OS-level. Same everywhere once tunnel is up
- **FFmpeg invocation** — we shell out; binary location is per-platform config, command surface is identical
- **SQLite** — bundled via `rusqlite`/`sqlx`

The existing VPN code in `backend/crates/kino/src/download/vpn/` is
~750 LOC and has a single `#[cfg(target_os = "linux")]` gate on
`configure_default_route()`. That's the only Linux-exclusive piece
today.

## 2. Per-OS specifics

### Linux

Already working. Minor adjustments:

- **Socket binding**: `SO_BINDTODEVICE` via `bind_device_name`. Needs `CAP_NET_RAW` or root. Systemd unit grants via `AmbientCapabilities=CAP_NET_RAW CAP_NET_ADMIN`
- **Default route**: `ip route replace default dev wg0 table ...` via iproute2. Current implementation is good
- **DNS**: write to `/etc/resolv.conf` if unmanaged; D-Bus to `systemd-resolved` if present. v1: optional — most VPN providers don't require DNS override
- **Service install**: native `.deb`/`.rpm`/AUR packages drop the unit at `/etc/systemd/system/kino.service` and run `systemctl enable --now`. Tarball users run `kino install-service`
- **Paths**: `/etc/kino/` + `/var/lib/kino/` for system service; `$XDG_CONFIG_HOME/kino/` + `$XDG_DATA_HOME/kino/` for user-mode runs

### macOS

- **Socket binding**: `IP_BOUND_IF` setsockopt (single syscall, takes interface index). Add a macOS variant in librqbit's socket-binding path — upstream PR or Kino-side fork
- **TUN device**: `utun` via the `tun` crate. Requires admin to create
- **Default route**: `route add default -interface utunN`
- **DNS**: `networksetup -setdnsservers` or `scutil`. v1: optional
- **Service install**: `.dmg` ships `Kino.app` containing the binary. First launch of the `.app` writes a `LaunchDaemon` plist at `/Library/LaunchDaemons/tv.kino.daemon.plist` (one admin-password prompt) and a per-user `LaunchAgent` for the tray. Tarball / Homebrew Formula users run `kino install-service`
- **Paths**: `~/Library/Application Support/Kino/` for user config + data; `/Library/Application Support/Kino/` for the system service

### Windows

- **Socket binding**: `IP_UNICAST_IF` setsockopt. Same shape as macOS
- **TUN device**: `wintun` driver via the `wintun` Rust crate. Ships as `wintun.dll` bundled alongside `kino.exe` in the archive. DLL embeds the driver and auto-installs it on first adapter creation
- **Default route**: `route.exe add 0.0.0.0 mask 0.0.0.0 ...`, or IP Helper API
- **DNS**: `netsh interface ipv4 set dnsservers` or WMI. v1: optional
- **Service install**: `.msi` installer registers the Windows Service via SCM (using Mullvad's `windows-service-rs`). Service runs as `LocalSystem` so it can create wintun adapters without per-boot elevation. Tarball users run `kino install-service` from an elevated prompt
- **Tray autostart**: installer ticks "Start Kino tray at login" by default; writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for the installing user
- **Windows Firewall**: installer adds inbound/outbound rules for the listener port and torrent port
- **Paths**: `%PROGRAMDATA%\Kino\` for service-mode data, `%LOCALAPPDATA%\Kino\` for user-mode runs

## 3. Service + tray installation UX

**Native package channels (Tier 1) — the common path.** End users
never touch `kino install-service` or `kino install-tray`. The package
handles both:

| Channel | What the package does |
|---|---|
| Windows `.msi` | Registers the Windows Service; ticks tray autostart for the installing user; adds firewall rules |
| macOS `.dmg` (`Kino.app`) | First `.app` launch installs LaunchDaemon (with admin prompt) + per-user LaunchAgent for the tray |
| Homebrew Cask | Installs `Kino.app`, behaves like `.dmg` |
| Homebrew Formula (CLI/headless) | Installs binary; user runs `brew services start kino` for the LaunchDaemon. No tray (formula targets headless) |
| `.deb` / `.rpm` | postinst registers and starts the systemd unit. Tray autostart written on first GUI launch |
| AUR (`kino-bin`) | Same as `.deb` |
| winget | Wraps the `.msi`; identical UX |
| Scoop | Bucket manifest, headless install (developer audience) |

**Tarball / `cargo install` (Tier 2) — power-user fallback.** The
explicit subcommands exist for users who download the raw archive or
build from source:

```
kino install-service           # writes the systemd unit / launchd plist / Windows Service
kino install-service --user    # per-user service where supported
kino install-tray              # writes the per-user autostart entry
kino uninstall-service
kino uninstall-tray
kino status                    # "running" / "stopped" / "not installed"
```

Behind the scenes, `install-service` wraps the `service-manager`
crate; `install-tray` wraps the `auto-launch` crate. See
`22-desktop-tray.md` for the tray-side detail.

## 4. First-run flow per platform

What a brand-new user sees.

### Windows

```
[Download kino-x.y.z-x86_64.msi from kinostack.app or winget install kino]
  [SmartScreen: "Windows protected your PC" → More info → Run anyway]   ← unsigned, one-time
  [UAC prompt — Yes]
  ✓ Kino installed to C:\Program Files\Kino
  ✓ Service registered and started
  ✓ Tray icon appears in system tray
  Open http://localhost:8080 to finish setup.
```

### macOS

```
[Download kino-x.y.z-arm64.dmg from kinostack.app or brew install --cask kino]
  [Drag Kino.app to Applications]
  [Double-click Kino.app]
  [Gatekeeper: "Cannot be opened, developer can't be verified" →    ← unsigned
   right-click → Open]                                                  one-time
  [Admin password prompt — install background service]
  ✓ Service installed
  ✓ Tray icon appears in menu bar
  Open http://localhost:8080 to finish setup.
```

### Linux

```
$ sudo apt install kino
   ✓ /etc/systemd/system/kino.service installed
   ✓ kino.service started
   Open http://localhost:8080 to finish setup.
$ kino install-tray         # optional, GUI users only
   ✓ Tray autostart enabled
```

Or for the appliance experience: flash the Pi image, boot, browse to
`http://kino.local:8080`. (See §7.)

Tarball users on any platform:

```
$ tar xzf kino-linux-x86_64.tar.gz && cd kino
$ sudo ./kino install-service
$ ./kino install-tray
```

## 5. FFmpeg bundling

Each release archive includes platform-specific FFmpeg binaries:

```
kino-linux-x86_64/        kino-macos-arm64/        kino-windows-x86_64/
  kino                      kino                     kino.exe
  ffmpeg                    ffmpeg                   ffmpeg.exe
  ffprobe                   ffprobe                  ffprobe.exe
                                                     wintun.dll
```

Kino invokes the bundled binaries preferentially (relative to its own
executable path) and falls back to system `ffmpeg` if not present.
Users override via `ffmpeg_path` in config (e.g. for a custom GPU
build).

Each bundle adds ~60 MB. Total release archive: 80–100 MB. Tiny by
media-server standards.

Bundled FFmpeg sources:
- **Linux**: BtbN's static builds
- **macOS**: BtbN's VideoToolbox-enabled builds
- **Windows**: BtbN's Windows builds

We mirror the binaries we use into our releases for supply-chain
control — don't fetch from third parties at install time.

## 6. Unsigned-binary posture

We've chosen not to pay for code-signing certs at launch. The user-side
consequence:

| Install path | First-run friction |
|---|---|
| Homebrew / winget / AUR / Scoop / `.deb` / `.rpm` / Docker / Pi image | **None** — package managers bypass OS gatekeepers entirely |
| Direct download `.msi` (Windows) | SmartScreen "More info" → "Run anyway" once |
| Direct download `.dmg` (macOS) | Right-click → Open once, OR `xattr -d com.apple.quarantine /Applications/Kino.app` |
| Direct download tarball + `kino install-service` | None on Linux; same as `.dmg`/`.msi` on macOS/Windows |

The marketing site + install docs steer users toward
package-manager channels precisely because they sidestep the
unsigned-binary friction. The `.msi` / `.dmg` direct-download paths
work but document one screenshot of the click-through.

When traffic justifies it, the spend order is:

1. **Apple Developer Program — $99/year.** Bigger UX win (Gatekeeper is scarier than SmartScreen) and the cheaper of the two
2. **Windows Authenticode cert.** EV (~$300–500/year) for instant SmartScreen reputation; OV (~$100–300/year) builds reputation over weeks. Defer until macOS signing is settled

Linux has no equivalent ecosystem-wide signing requirement. We GPG-sign
release tarballs + checksums (cargo-dist does this); distro
repositories (AUR, Homebrew, etc.) handle their own signing.

## 7. Distribution channels

Five tiers of effort, all sourcing artefacts from the canonical GitHub
Release.

### Tier 1 — automated day-one

| Channel | Platform(s) | Mechanism |
|---|---|---|
| GitHub Releases | All | cargo-dist publishes archives + checksums + GPG sigs + MSI |
| `.dmg` (`Kino.app`) | macOS | cargo-dist (or `create-dmg` step) |
| `.msi` (or NSIS `.exe`) | Windows | cargo-dist (`wix` plugin) |
| `.deb` | Debian/Ubuntu | `cargo-deb` step in CI |
| `.rpm` | Fedora/RHEL | `cargo-generate-rpm` step in CI |
| AppImage | Any Linux desktop | `appimagetool` step (deferred — needs AppDir layout + .desktop + icon bundle. Tracked under §"Known limitations") |
| Homebrew Cask (`.app`) | macOS desktop | cargo-dist tap (`{owner}/homebrew-kino`) |
| Homebrew Formula (CLI/headless) | macOS / Linuxbrew | cargo-dist tap |
| winget | Windows | `vedantmgoyal9/winget-releaser` PR to `microsoft/winget-pkgs` |
| AUR (`kino-bin`) | Arch Linux | `KSXGitHub/github-actions-deploy-aur` PKGBUILD push |
| Universal install script | Linux/macOS | `curl -fsSL https://kinostack.app/install.sh \| sh` — cargo-dist generates this |
| Universal install script | Windows | `irm https://kinostack.app/install.ps1 \| iex` — cargo-dist generates this |
| `cargo install kino` | Anywhere with Rust | Automatic on `cargo publish` |
| Docker / OCI (`ghcr.io/kinostack-app/kino`) | Any Linux | Multi-arch (amd64/arm64) via `docker/build-push-action`; headless build (`--no-default-features`) |
| Raspberry Pi appliance image | Pi 4/5/3B+/Zero 2 W | `usimd/pi-gen-action` builds `kino-rpi-arm64.img.xz`; ships the headless build |

End-user commands across these channels:

```
brew install kino                    # macOS Homebrew Formula (CLI)
brew install --cask kino             # macOS Homebrew Cask (.app + tray)
winget install kino                  # Windows
sudo apt install kino                # Debian/Ubuntu (after adding our repo)
sudo dnf install kino                # Fedora (after adding our repo)
yay -S kino-bin                      # Arch
docker pull ghcr.io/kinostack-app/kino   # Anywhere
curl -fsSL https://kinostack.app/install.sh | sh    # Linux/macOS
irm https://kinostack.app/install.ps1 | iex         # Windows
# Plus direct download .msi / .dmg / .deb / .rpm / .tar.gz from GitHub Releases
```

### Tier 2 — add when traffic stabilises

Not day-one, but natural follow-ups once there's an established user base:

| Channel | Mechanism | Notes |
|---|---|---|
| Chocolatey | nuspec PR | Older Windows pkg manager; winget covers most users |
| Scoop (own bucket) | JSON manifest in a bucket repo | Windows developer audience |
| Flatpak (Flathub) | Manifest PR; tray plugs require finish-args (see `22-desktop-tray.md`) | Sandboxed; slower release pipeline |
| Snap | `snapcraft.yaml`; tray needs `system-tray` plug auto-connection | Sandboxed; auto-connect needs Snap Store review |
| Own APT repo on GH/Cloudflare Pages | `dpkg-scanpackages` over the `.deb` from Tier 1 | Lets users `apt install kino` without trusting a third-party PPA |
| Own RPM repo | `createrepo_c` over the `.rpm` from Tier 1 | Same idea for `dnf` |
| Fedora Copr | Copr webhook on release | Fedora-hosted RPM builds |
| Unraid Community Apps | Manual template submission | Community-maintained after one-time submission |
| TrueNAS apps | Helm chart, Docker-based | Same |

### Tier 3 — community-maintained, we don't gate releases on these

- **`nixpkgs`** — the Nix community maintains its own packaging. We accept PRs but don't own the manifest
- **MacPorts** — small audience; community can submit a Portfile
- **Launchpad PPA** — superseded by our own APT repo

### Why this matrix

The aim: every user finds Kino through a channel they already know.
Windows users discover it via winget. Mac users via Homebrew or the
`.dmg`. Debian users via `apt`. Arch users via the AUR. Pi users via
the imager. Power users via `curl | sh`. Containerised setups via
Docker. The `kino install-tray` command exists but most users never
type it.

### Raspberry Pi appliance image

A bootable Pi SD-card image that flashes, boots, and auto-starts Kino
on `http://kino.local:8080` — no terminal required. The "Home
Assistant OS" UX path for users who want a dedicated Pi as their Kino
box. A distribution format, not a new platform: the binary inside is
the same Linux ARM64 artefact (built `--no-default-features`, no tray)
produced by the main release pipeline.

**Approach: pi-gen + one stage.** [pi-gen](https://github.com/RPi-Distro/pi-gen)
is the official Raspberry Pi tool that builds Raspberry Pi OS itself —
shell scripts + `debootstrap` + numbered chroot stages, driven via
Docker. We layer a single `stage-kino/` on top of the standard Pi OS
Lite stages:

- `00-packages` — runtime deps (FFmpeg is bundled inside the Kino archive, so this is mostly `avahi-daemon`, `ca-certificates`, `unattended-upgrades`)
- `00-run-chroot.sh` — installs the Kino `.deb` produced earlier in the release pipeline, enables `kino.service`, sets `HOSTNAME=kino`, enables `unattended-upgrades`

Umbrel OS follows this exact pattern. The
[Hassbian pi-gen fork](https://github.com/home-assistant/pi-gen)
(Home Assistant's pre-HAOS image) is a reference for "bespoke pi-gen
stage that installs a daemon."

**One image, four boards.** Pi 4, Pi 5, Pi 3B+, and Zero 2 W all boot
the same 64-bit image — the firmware selects the right kernel
(`kernel8.img` vs `kernel_2712.img`) at boot. No separate artefacts
per board.

**First-boot UX.** Raspberry Pi OS Trixie (Nov 2025+) supports
**cloud-init** via Pi Imager 2.0.6+. Our image declares
`init_format: cloudinit-rpi` in its Imager repository entry; the user
configures Wi-Fi / hostname / SSH / user in Pi Imager *before*
flashing, and cloud-init applies them on first boot. pi-gen's built-in
`init_resize.sh` expands the root partition to fill the SD card on
first boot. Avahi publishes `kino.local` on mDNS.

**Build pipeline.** GitHub Actions via
[`usimd/pi-gen-action`](https://github.com/usimd/pi-gen-action) on
`ubuntu-latest`. `qemu-user-static` + `binfmt_misc` handle the ARM
chroot on x86 runners. Wall-clock ~20–30 min per build, with
`apt-cacher-ng` cached across runs. Output: `kino-rpi-arm64.img.xz`,
~500 MB–1 GB compressed.

**Distribution.**

- Uploaded to GitHub Releases alongside the other archives. Release file-size limit is 2 GiB — fits comfortably
- Advertised via a self-hosted **Raspberry Pi Imager Repository JSON v4** served from the same Cloudflare Pages host as the universal install script. Users paste the URL into Pi Imager → custom repository → Kino appears in the OS list
- A follow-up submission gets Kino into the *official* Pi Imager catalogue (manual review). Not a day-one dependency

**Maintenance.** One image rebuild per Kino release, automated. One
base-OS bump per Pi OS major release (~2-year cadence). Debian
security team handles CVE patching upstream; `unattended-upgrades`
inside the image picks up patches between our releases.

**Non-Pi ARM SBCs** (Orange Pi, Rock Pi, Radxa) are out of scope —
different bootloaders, DTBs, and vendor kernels. Those users run the
generic Linux ARM64 binary on Armbian or their distro of choice.

## 8. Release engineering

GitHub Release is the canonical source of truth. Every other channel
pulls from there.

### Build engine: cargo-dist

[cargo-dist](https://github.com/astral-sh/cargo-dist) drives the
release. One config in `Cargo.toml` produces:

- Platform archives for all five Tier 1 OSes
- Windows `.msi` (`wix` installer)
- Homebrew tap formula
- Shell + PowerShell install scripts
- GitHub Release with checksums + GPG signatures + SLSA build provenance attestations
- A reproducible CI workflow

(cargo-dist 0.31 has no `.pkg` installer; macOS users get the shell installer or Homebrew.)

Channels not natively supported by cargo-dist (`.deb`, `.rpm`,
AppImage, AUR, winget, Flathub, Snap) plug in as additional CI steps
or follow-on jobs that consume the cargo-dist artefacts.

### CI workflow structure

| Workflow | Trigger | Purpose |
|---|---|---|
| `.github/workflows/ci.yml` | Every push/PR | Lint, type-check, test on Linux. Never publishes |
| `.github/workflows/release.yml` | Git tag `v*` | cargo-dist build matrix; produces archives + MSI + .deb + .rpm + AppImage; publish step is `workflow_dispatch`-gated (`dispatch-releases = true` in `[workspace.metadata.dist]`) — a maintainer clicks Run after reviewing the build to upload to GitHub Release |
| `.github/workflows/channels.yml` | GitHub Release published | Fan-out: Homebrew tap bump, winget PR, AUR push, Pi image build, GHCR push, install script upload |

Decoupling matters: if the winget PR fails, the GitHub Release is
still live. Re-run the one failed job rather than re-cutting a
release.

### Actions minutes

**Public repo**: unlimited Linux minutes, large free ceiling on
macOS + Windows. Per release: ~30 min wall-clock end-to-end (most
jobs parallel). Negligible cost.

**Private repo**: 2000 min/month free tier. Per release: ~60–90 min.
macOS runners are 10× Linux cost; Windows runners are 2×.

We deliberately avoid expensive patterns: no per-PR macOS/Windows full
build (CI on PRs is Linux-only for lint/test), no nightly channel
re-submission (channels update on tag, not cron).

### Release runbook

1. Bump version in `Cargo.toml` (workspace) and commit
2. Push tag `vX.Y.Z`
3. `release.yml` runs: ~30 min
4. Draft GitHub Release appears with all artefacts attached. Review release notes (auto-generated, manually polished)
5. Publish the release (one click). This triggers `channels.yml`
6. `channels.yml` fans out: Homebrew bump merges; winget PR auto-merges within hours; AUR + Scoop update immediately; Pi image attaches; Docker images appear on GHCR
7. Monitor the channel jobs; re-run any flakes (AUR SSH occasionally)
8. Post to the project's release channel (Discord/forum/RSS) with the canonical GitHub URL

Maintainer time per release: ~10 minutes of review + monitoring.

### Secrets

| Secret | Purpose |
|---|---|
| `GITHUB_TOKEN` | Built-in; covers GitHub Release, GHCR push, PR comments |
| `HOMEBREW_TAP_TOKEN` | PAT with write access to `homebrew-kino` |
| `WINGET_TOKEN` | PAT for winget-releaser |
| `AUR_SSH_PRIVATE_KEY` | SSH key registered with the AUR account |
| `GPG_PRIVATE_KEY` + passphrase | Signs release archives + APT/RPM repo metadata |

All rotated yearly. Revocation workflow documented in the internal
runbook (not this spec).

## 9. Paths and conventions

Config + data directory resolution uses the `directories` crate (per-OS
defaults):

| OS | Config dir | Data dir |
|---|---|---|
| Linux (user) | `$XDG_CONFIG_HOME/kino/` | `$XDG_DATA_HOME/kino/` |
| Linux (system service) | `/etc/kino/` | `/var/lib/kino/` |
| macOS | `~/Library/Application Support/Kino/config/` | `~/Library/Application Support/Kino/data/` |
| Windows (service) | `%PROGRAMDATA%\Kino\config\` | `%PROGRAMDATA%\Kino\data\` |
| Windows (user) | `%APPDATA%\Kino\config\` | `%LOCALAPPDATA%\Kino\data\` |

Override via `KINO_CONFIG_PATH` and `KINO_DATA_PATH` env vars (already
supported).

## 10. Schema / config changes

No new tables. Two new Config fields:

| Column | Type | Default | Notes |
|---|---|---|---|
| `ffmpeg_path` | TEXT | null | Null = autodetect (bundled → system PATH). Override for custom builds |
| `tray_autostart_enabled` | INT | 1 (desktop builds), 0 (headless) | Mirrors the `Settings → Desktop` toggle. Backend wraps `kino install-tray` / `kino uninstall-tray` when flipped |

## Entities touched

- **Reads:** Config (VPN, paths, `tray_autostart_enabled`), VpnConfig
- **Creates / Updates:** `VpnManager` state (existing, platform-agnostic)
- **System-level creates:** platform service entry (systemd unit / launchd plist / Windows service registration); per-user autostart entry on tray enable
- **System-level creates on Windows:** wintun adapter on first VPN connect; Defender firewall rules on install

## Dependencies

Existing:
- `boringtun`, `tun`, `librqbit`

New:
- `cargo-dist` — release/build engine (CI-only, not a runtime dep)
- `service-manager` — cross-platform service install
- `windows-service-rs` (Mullvad's) — Windows SCM integration
- `wintun` — wintun driver bindings
- `directories` — per-OS path conventions
- `auto-launch` — per-user autostart (also used by tray, see `22-desktop-tray.md`)

No new system binaries beyond bundled FFmpeg.

## Error states

- **Not admin/root on `kino install-service`** → clear error + instructions to re-run elevated
- **wintun.dll missing** (Windows, archive extracted incompletely) → error on VPN start: "wintun.dll not found next to kino.exe". Docs show how to fix
- **TUN interface creation fails** (macOS utun permissions denied, Windows wintun driver install failed) → surface via health dashboard and logs; VPN toggle stays off until resolved
- **Port binding fails** (firewall blocking) → detect and guide: "Add firewall rule for port 8080" with copy-paste command per OS
- **Direct-download Gatekeeper / SmartScreen block** → documented: right-click → Open (macOS), More info → Run anyway (Windows). One-time
- **librqbit `bind_device_name` silently no-ops on macOS/Windows** → see §"Known unknowns"; if it doesn't work, we implement socket binding above librqbit or contribute upstream

## Known unknowns

**Verify before committing to Tier 1 for macOS/Windows:**

1. **librqbit `bind_device_name` behaviour on macOS and Windows.** Linux uses `SO_BINDTODEVICE`. On other OSes it may silently no-op or error. Mitigation: bind sockets at our layer, or upstream a PR adding `IP_BOUND_IF` / `IP_UNICAST_IF` variants. **Day-one verification task**
2. **`tun` crate behaviour on Windows ARM64.** Documented as supported but less tested. Tier 2 status reflects this
3. **DNS override on macOS without breaking user's system DNS.** `scutil` ordering needs care so we don't strand the user's resolver after VPN disconnect
4. **Sleep/wake cycles on macOS.** `utun` interfaces survive sleep inconsistently — may need reconnect logic specifically tuned for macOS wake events
5. **Windows Defender false positives.** Fresh-signed (or unsigned) binaries sometimes flagged until reputation builds. Mitigation: submit to Microsoft for analysis pre-release

## Known limitations

- **No ARM64 Windows binary in v1** — Surface Pro X / Snapdragon PCs build from source. Small audience; revisit later
- **Direct-download macOS/Windows users see one Gatekeeper / SmartScreen warning.** Mitigated by steering toward package channels (Homebrew, winget). Re-evaluate paid signing once volume justifies it
- **FFmpeg version pinned per release.** Override via `ffmpeg_path`. We don't auto-update FFmpeg outside a Kino release cycle
- **Docker image wraps the headless Linux binary** — no tray, smaller image. Users running Docker who want a tray run a desktop install instead
- **Pi appliance image targets Raspberry Pi only.** Other ARM SBCs use the generic Linux ARM64 binary on Armbian / their own distro
- **`service-manager` crate may need augmentation** for advanced features (user-mode systemd with sysusers, etc.). Acceptable — drop to platform-specific code paths for features it doesn't cover
- **Flatpak / Snap are Tier 2, not Tier 1.** Sandboxing complicates VPN networking and tray plugs. Doable but extra manifest work; defer to once direct-download channels are stable
- **GPG-signed release artefacts but no Authenticode/Apple Developer signing.** See §6
- **AppImage doesn't bundle ffmpeg.** ffmpeg is a host runtime dep on AppImage like on the .deb / .rpm. Bundling would push the image past 100 MB and put us on the hook for re-bundling every CVE; documented in the install guide as a one-line `apt install ffmpeg` step
- **Tray left-click-to-browser deferred.** `tray-icon` 0.21 doesn't expose a left-click event separate from menu activation. v0 uses click-to-menu on every platform; the original Dropbox/Discord-style "left-click opens browser" is a follow-up gated on an upstream fix or hand-rolled per-OS hooks. See `22-desktop-tray.md` §1
- **Tray icon is colour-disc, not template-style on macOS.** Programmatic RGBA disc shows on every platform but doesn't tint with the macOS menu-bar theme. Acceptable as MVP; production `.icns` (macOS) + dark/light `.ico` (Windows) bundle when cargo-dist `.app` packaging lands
- **Firewall rules not auto-added on Linux.** Distro firewall variation (ufw/firewalld/nftables) means we don't ship a one-size-fits-all rule. Documented in install guide; user adds the `:8080` rule themselves
- **Windows Event Log provider not registered.** `Get-WinEvent -ProviderName Kino` won't work; SQLite `log_entry` table is the operator-facing log everywhere. Adding the ETW provider would mean `eventlog` crate + manifest XML + per-install registration; deferred to v1.x if ops demand it
- **Browser launch failures are logged, not surfaced in UI.** If `webbrowser::open()` fails (no default handler, missing `xdg-open` on minimal distros), the tray logs a warning but no UI toast. v0 acceptable
