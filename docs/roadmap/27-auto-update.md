# Auto-update

> **Status (2026-04-27): not started — gated on subsystem 21.** No
> `update/` module exists in `backend/crates/kino/src/`; no version
> polling, no `self-replace` integration. Manual upgrade only:
> rebuild from source, `brew upgrade kino`, `winget upgrade`,
> `docker compose pull`, etc.
>
> **Gating chain.** This subsystem verifies cargo-dist's signed
> artefacts via Sigstore + the GitHub build-provenance attestations
> (`github-attestations = true` in `[workspace.metadata.dist]`).
> Until subsystem 21 emits a real `vX.Y.Z` release with attestations
> attached, there's nothing for `verify.rs` to verify against.
> Sequence: 21 cuts a real tag → builds emit attestations →
> 27 starts implementation.
>
> **Why we don't use cargo-dist's bundled `axoupdater`.** Our config
> sets `install-updater = false` deliberately. axoupdater is a
> generic GitHub-Releases poller that's hard to extend with the
> deployment-mode detection / quiet-window apply / cgroup-aware
> rollback logic this design specifies. Implementing in-house keeps
> the surface honest about what it does (no telemetry beyond ETag
> conditional GETs to `api.github.com`).
>
> **Audit (2026-04-27):** design still aligns with ADR 0008
> (auto-update polling is the explicit telemetry carve-out, no user
> data sent) and the cargo-dist 0.31 attestations pipeline shipped
> in subsystem 21. No spec changes since the 2026-04-26 review.

Transparent, background self-update for kino. The binary polls GitHub Releases on a fixed cadence, stages a verified download, and swaps itself during a quiet window — active transcodes and downloads are allowed to finish first. Users learn about the update *after the fact* via a subtle banner linking to an in-app changelog. No prompts, no click-to-update dance, no data leaves the machine except anonymous conditional GETs to `api.github.com`.

## Scope

**In scope:**
- Background update check against GitHub Releases (ETag-cached, no user data transmitted)
- Staged download + Sigstore signature verification before any filesystem swap
- Deployment-mode detection — self-update only where kino installed itself (standalone + OS service); disabled under Docker, Homebrew, apt, winget, cargo-install, read-only filesystems
- Graceful apply window — waits for no active transcode and no active download, force-applies after 24h with a 60-second on-screen countdown
- Atomic binary swap via `self-replace` (handles the Unix inode replacement + Windows pending-rename patterns)
- Service manager integration (systemd / launchd / Windows SCM) to restart after swap
- Automatic rollback on boot-crash-loop; manual one-click rollback from settings
- Two release channels: `stable` (default) and `beta` (GitHub prereleases)
- In-app post-update banner ("kino updated to v0.5.2 — see what's new")
- In-app changelog page at `/settings/updates` — renders GitHub release notes as markdown, with a build-time baked-in fallback for offline installs
- `kino-tray` binary updated in lockstep (same archive, swapped alongside `kino`)
- Docker users see a distinct banner with a `docker compose pull` hint and Watchtower-compatible image labels

**Out of scope:**
- Telemetry, usage analytics, crash reporting — the updater transmits *nothing* user-identifying; conditional-GET ETag requests are the only outbound traffic and they carry no user state
- Downgrading to arbitrary older versions — rollback is single-step (current ↔ previous only)
- Nightly / canary / dev channels — two channels cover the self-hosted audience
- Push-initiated updates from a central server
- Differential / binary-delta downloads
- Auto-installing OS service units that don't exist — if kino isn't registered with systemd/launchd/SCM, it runs as standalone foreground and prompts the user to re-launch
- Managing updates for kino running under an orchestrator (Kubernetes, Nomad) — the orchestrator owns image/pod lifecycle
- User-configurable check frequency — hardcoded

## Architecture

### Module layout

```
backend/crates/kino/src/update/
├── mod.rs          # public API, task registration
├── mode.rs         # DeploymentMode detection
├── channel.rs      # stable/beta parsing, semver compare
├── check.rs        # GitHub Releases poll with ETag
├── download.rs     # resumable stream to staging dir
├── verify.rs       # Sigstore bundle verification
├── apply.rs        # self-replace + service restart
└── rollback.rs     # boot-crash detection + revert
```

One scheduled task in the scheduler (doc 07): `update_check`, 6h cadence plus a 60s post-boot warm-up fire.

### Deployment mode is the key decision

At startup, `mode::detect()` resolves a single `DeploymentMode` and caches it on `AppState`. This governs whether the updater runs at all:

```rust
pub enum DeploymentMode {
    Docker,                    // disabled; show "pull new image" banner
    PackageManager(Manager),   // disabled; show "run `brew upgrade kino`"
    StandaloneService(Init),   // ENABLED (systemd/launchd/SCM restarts us)
    StandaloneForeground,      // ENABLED but prompt user to re-launch
    ReadOnlyFs,                // disabled silently
    Unknown,                   // disabled, logged
}
```

The rule: **self-update only when kino installed kino.** Anything else and we'd race the package manager. Precedence, first match wins:

1. **Docker** — `/.dockerenv` exists OR `/proc/self/cgroup` mentions `docker`/`containerd`/`kubepods`
2. **Read-only FS** — probe `std::env::current_exe().parent()` with a temp file; `EROFS`/`EACCES` → read-only
3. **Package manager** — match `current_exe()` against known paths: `/opt/homebrew/`, `/usr/local/Cellar/` → Homebrew; `/usr/bin/`, `/usr/sbin/` with a matching `dpkg -S` → apt; `C:\Program Files\WindowsApps\` → winget/MSIX; `~/.cargo/bin/` → cargo install
4. **Standalone service** — service-manager entry points at our `current_exe()`
5. Otherwise: standalone foreground

Kill-switch: `KINO_UPDATES=off` env var disables the whole subsystem regardless of detected mode. `KINO_DEPLOYMENT=docker|service|foreground|off` forces a mode for weird setups.

### Why this stack, not alternatives

- **`self-replace` over `self_update`** — `self_update` is synchronous, desktop-shaped, and its Windows handling is coarser. `self-replace` (Sentry) is tightly scoped to the atomic-swap problem and gets the inode / pending-rename semantics right on both platforms.
- **Hand-rolled poller over `axoupdater`** — `axoupdater` is excellent for CLI dev-tools (cargo-dist's home turf) but assumes a CLI shape: user runs `myapp update`. Kino is a long-running service; we need scheduler integration, deployment-mode awareness, graceful-drain, WebSocket events. Building on `reqwest` (already a dep) is ~400 lines and fits the service model.
- **GitHub Artifact Attestations over GPG/minisign** — attestations bind each artifact to our CI workflow via a short-lived Sigstore certificate. No long-lived key to lose, no bus-factor problem. Verification in-binary with `sigstore-rs`.

## 1. Check

Fires every 6 hours (scheduler task) plus one extra fire 60 seconds after boot. Six-hourly is polite (≈4/day, well under GitHub's 60-req/hr anonymous IP quota), captures same-day patches, and avoids the "user's machine was asleep all week" problem that daily cadence creates.

### Request

```
GET https://api.github.com/repos/kinostack-app/kino/releases/latest
  (or /releases?per_page=10 filtered by prerelease=true for the beta channel)
User-Agent: kino/{version}
If-None-Match: "{cached etag}"
```

`304 Not Modified` doesn't count against the rate limit — this keeps us indefinitely cheap.

### Storage

One row in a dedicated `update_check` table:

| Column | Purpose |
|---|---|
| `id` | PK (always 1 — single-row table) |
| `channel` | `stable` or `beta` |
| `latest_version` | Cached tag, semver-normalised |
| `etag` | For conditional GETs |
| `release_notes_md` | Markdown body of the release |
| `assets_json` | Asset names + download URLs + sigstore URLs |
| `last_checked_at` | For observability + settings UI |
| `last_error` | nullable |

### Version compare

`semver::Version` parse on `CARGO_PKG_VERSION` and the latest tag. Only `>` triggers the update flow. Equal or older = no-op.

### Backoff

On failure: 6h → 12h → 24h → 48h, capped at 48h, reset on any success. Suspend/resume avoidance: skip the scheduled fire if the machine has been awake < 60s.

## 2. Download and verification

### Picker

Match release assets against `{os}-{arch}` derived from `std::env::consts::OS` + `std::env::consts::ARCH`. Regex on asset names (e.g. `kino-{version}-x86_64-unknown-linux-gnu.tar.xz`). Fail closed if no asset matches — log a warning.

### Streaming download

```
{DATA_DIR}/updates/staging/kino-{version}.tar.xz
{DATA_DIR}/updates/staging/kino-{version}.tar.xz.sigstore
```

Streamed via `reqwest`'s body stream so a 100 MB archive doesn't allocate 100 MB of RAM. Resumable: on retry, `Range: bytes={len}-` picks up where we left off — GitHub's CDN honours range requests on release assets.

### Signature verification

Before extracting anything:

1. Fetch the `.sigstore` bundle (Sigstore-format, small JSON).
2. Verify via `sigstore-rs`:
   - Subject digest matches the SHA256 of the downloaded archive
   - Workflow identity is `https://token.actions.githubusercontent.com` and the workflow file + ref match `kino`'s `release.yml` on a `refs/tags/v*` tag
3. **TOFU pin**: on first install, record the workflow identity in Config. Subsequent updates must match; any change aborts with a critical notification. Catches the "attacker publishes from a fork" class of attack.

Also publish a `SHA256SUMS` file signed by a project-level key as a belt-and-braces artifact for users verifying manually offline.

### Extraction

Extract the archive into `{DATA_DIR}/updates/staged/{version}/`. Validate that `kino` / `kino.exe` and `kino-tray` / `kino-tray.exe` both live at the archive root. Anything else → bail, clean up, post warning.

Post a WebSocket event:

```json
{"kind": "update_staged", "version": "0.5.2"}
```

Frontend settings page reflects "ready to install".

## 3. Apply

The "magic restart" window. Picks its moment instead of asking for one.

### Quiet-window heuristic

Proceed as soon as **both**:
- No active transcode (query `playback::HlsSessionManager`)
- No torrent reports active piece activity within the last 60 seconds

If no quiet window appears within 24 hours of staging, force-apply — broadcast a 60-second countdown first:

```json
{"kind": "update_pending", "in_seconds": 60, "version": "0.5.2"}
```

### Swap

1. Copy the current `kino` binary to `{DATA_DIR}/updates/previous/kino-{current_version}` (and `kino-tray` alongside). Keep only one generation of previous.
2. `self_replace::self_replace("{DATA_DIR}/updates/staged/{version}/kino")` — atomic on Unix (rename over the running inode), two-phase on Windows (`MoveFileEx` with `MOVEFILE_DELAY_UNTIL_REBOOT` for the `.old` cleanup).
3. Replace `kino-tray` in the same directory. On Windows, the tray binary may be locked if the tray is running — use the same delayed-rename pattern; tray picks up the new binary on next start.
4. Write an `update_history` row: `from_version`, `to_version`, `applied_at`, `channel`.
5. Request a service restart (see below). Emit `{"kind": "update_applied", "version": "0.5.2"}` just before exiting so the frontend can queue the post-update banner for the user's next page load.

### Service restart

| Mode | Mechanism |
|---|---|
| systemd | `systemctl restart kino` via `zbus` — no `sudo` needed for user/self-owned units. Service unit must set `Restart=always` as a fallback |
| launchd | exit 0; `KeepAlive=true` in the plist brings us back |
| Windows SCM | Signal stop; recovery options (set at install time, doc 21) auto-restart |
| Foreground | Print "kino has been updated to v{}. Please re-run." and exit 0 |

## 4. Rollback

### Automatic (crash-loop)

The hazard case: a release has a startup bug that escaped CI. Without automatic rollback, every homelab on earth bricks in the same 24 hours. The rollback path:

1. On every boot, read a `boot_counter` file under `$DATA_DIR/updates/`. Increment it pre-init.
2. On successful startup (30 seconds of uptime with no panic), reset counter to 0.
3. On boot, if `update_history.applied_at < now - 10 minutes` AND `boot_counter >= 2`, restore the previous binary from `{DATA_DIR}/updates/previous/` and emit a `update_rollback` notification (routed through subsystem 08).

The 10-minute window plus the 2-crash threshold deliberately over-triggers false positives — it's much better to downgrade a borderline-working release than to leave a genuinely-broken one installed.

### Manual

`POST /api/v1/updates/rollback` restores the previous binary and restarts. Exposed as a button on the `/settings/updates` page under "Last update: v0.5.1 → v0.5.2 · [rollback]".

### Retention

Exactly one generation. When a new update staged-and-applied cycle completes, the "current previous" rolls off. Staging directory and the previous binary share a cleanup pass.

## 5. Channels

Two, no more.

| Channel | Source | Use case |
|---|---|---|
| `stable` | GitHub "latest release" (non-prerelease) | Default for everyone |
| `beta` | Prereleases tagged `v*-beta.*` / `v*-rc.*` | Users who want early access |

Settings UI: one `<Select>` labelled "Update channel", options `Stable (recommended)` / `Beta (early access, may break)`. Stored in `Config.update_channel`.

### Channel change behaviour

- Stable → Beta: next check may find a newer prerelease; normal update path applies
- Beta → Stable: if the currently-installed version is a prerelease, the next check picks up the latest *stable* even if it's older (downgrade via the same mechanism as rollback). This is the "I want off the beta train" exit ramp

### Tag convention

Enforced in `release.yml`:
- `vX.Y.Z` → stable (GitHub release not marked prerelease)
- `vX.Y.Z-beta.N`, `vX.Y.Z-rc.N` → prerelease

Mismatches fail CI.

## 6. Migrations and downgrade safety

Rollback is only safe if the previous binary can talk to the newer schema. Two rules, enforced by convention + review:

1. **Additive-only migrations** between minor versions. New columns, new tables — yes. Renames, drops, type changes — only in major-version bumps, and those may gate rollback (see next).
2. **Destructive / irreversible migrations** require user consent. The updater refuses to auto-apply such releases and posts a banner:

   > Kino v1.0 includes library changes that cannot be automatically reversed. Back up your data, then click Confirm to install.

   Determined by a manifest flag on the release (`irreversible: true` in a release metadata file we publish alongside assets).

If a migration fails at startup, that's a boot failure → crash-loop rollback kicks in → previous binary runs the *previous* schema (which is still in the DB since migrations are additive) and the user sees a critical notification.

## 7. In-app UI

Three surfaces, in the order a user encounters them.

### 7.1 Pre-update banner (rare)

Only appears during the 60-second force-apply countdown, or if the user has opted out of auto-apply. Top-of-page bar:

> kino v0.5.2 is installing in **0:42** · [install now] [remind me tomorrow]

Most users never see this.

### 7.2 Post-update banner

Top-of-page bar, dismissable, auto-hides after 24 hours:

> kino updated to v0.5.2 · [see what's new](/settings/updates#v0-5-2)

Rendered when `update_history.applied_at > now - 24h AND !dismissed_by_user(version)`. One banner per version — dismissing v0.5.2's banner doesn't suppress v0.5.3's.

### 7.3 Changelog page (`/settings/updates`)

Single page with:

- Current version, channel, last-checked time, next-check ETA
- Channel switcher
- "Check now" button (manual trigger, useful for debugging)
- "Install now" button (visible only when an update is staged and awaiting a quiet window)
- "Rollback to v0.5.1" button (visible only when a previous binary exists)
- A scrollable list of recent releases — tag, date, markdown-rendered body, signature verification status ("✓ built by kino CI on 2026-04-15")

**Source of truth**: `GET /api/v1/updates/changelog?channel=stable` fetches live from GitHub Releases (cached in `update_check.release_notes_md` + a separate `release_history` table of the last 20), with a build-time baked-in snapshot as fallback for offline installs. Built by a `build.rs` step that curls `/releases?per_page=20` at release time and bakes the JSON into the binary via `include_str!`.

## 8. Docker and package-manager variants

### Docker

Detected via `/.dockerenv` or cgroup string. Self-update fully disabled. Banner copy:

> Kino 0.5.2 is available. You're running under Docker — run `docker compose pull && docker compose up -d kino`. Or enable [Watchtower](https://containrrr.dev/watchtower/) for automatic pulls.

Image publishes the Watchtower opt-in label:

```
LABEL com.centurylinklabs.watchtower.enable=true
LABEL org.opencontainers.image.source=https://github.com/kinostack-app/kino
```

### Homebrew / apt / winget / cargo-install

Detected via `current_exe()` path. Banner copy adapts:

- Homebrew: `brew upgrade kino`
- apt: `apt upgrade kino`
- winget: `winget upgrade kino`
- cargo: `cargo install kino --force`

No filesystem writes. No nagging — banner appears once per version and is dismissable.

## 9. kino-tray integration

Tray updates in lockstep. The server's updater swaps `kino-tray` at the same moment it swaps `kino`, and they're guaranteed to come from the same archive. Tray does not poll GitHub on its own.

Version-skew handling: after a server update, the tray may still be running the old binary (its process memory is the old version even though its on-disk binary is new). On next health poll, the tray notices `server.version != self.version` and surfaces a menu entry:

> Kino updated to v0.5.2 — [restart tray]

Clicking it exits the tray; autostart + `KeepAlive` bring it back on the new binary.

## 10. Frequency and politeness

All hardcoded — no user-configurable knobs for the update cadence.

| Behaviour | Setting |
|---|---|
| Check interval | 6 hours |
| Post-boot warm-up before first check | 60 seconds |
| Quiet-window wait before force-apply | 24 hours |
| Force-apply countdown | 60 seconds |
| Post-update banner auto-hide | 24 hours |
| Backoff on check failure | 6h → 12h → 24h → 48h |
| Rate-limit floor | Well under GitHub's 60/hr anonymous quota |

## Entities touched

- **Reads:** `Config` (channel, TOFU workflow identity, update_apply_mode), `PlaybackSession` (transcode activity), `Download` (torrent activity)
- **Writes:** `update_check` (1 row), `update_history` (append), `Notification` (on rollback or critical failure)
- **Filesystem:**
  - `{DATA_DIR}/updates/staging/` — in-flight downloads
  - `{DATA_DIR}/updates/staged/{version}/` — verified, extracted, ready
  - `{DATA_DIR}/updates/previous/kino-{version}` — one generation back, for rollback
  - `{DATA_DIR}/updates/boot_counter` — crash-loop detection
- **Network:** `api.github.com/repos/kinostack-app/kino/releases/*`, release asset CDN URLs, Sigstore bundle URLs. No other outbound.
- **OS:** service manager (systemd DBus, launchd, Windows SCM) for restart

## Dependencies

| Crate | Purpose |
|---|---|
| `self-replace` | Atomic binary swap, Unix + Windows |
| `sigstore` | Attestation verification |
| `semver` | Version compare and channel filtering |
| `reqwest` | HTTP (already a dep) |
| `zbus` | systemd restart signalling |
| `service-manager` | Reuse from `kino install-service`, for mode detection + restart |
| `tar`, `xz2`, `zip` | Archive extraction |

No new system binaries. GPG / minisign / cosign CLI not required — verification is in-binary.

## Error states

- **Network unreachable on check** → backoff, leave cached `update_check` row intact. UI shows "last checked N hours ago". No banner, no nag.
- **ETag match (304)** → cheap no-op. Doesn't count against rate limit.
- **No matching asset for this platform** → log warning, disable further checks for this version.
- **Signature verification failure** → abort download, log error at critical level, surface notification. *Never* apply an unsigned or mis-signed artifact. TOFU workflow-identity change → same handling.
- **Download interrupted** → resume via `Range:` on next scheduled check.
- **Extraction fails** → clean staging, log error, retry on next check.
- **Swap fails mid-apply** → if the new binary is in place but service restart fails, the next boot still runs the new binary (it's already swapped). Crash-loop rollback catches the bad case.
- **Migration fails on new binary** → process exits with error → crash-loop rollback restores previous binary and previous schema.
- **Previous binary missing** when a rollback is requested → expose a clear error; no magic recovery.
- **Read-only filesystem mid-run** (user remounted) → `self_replace` returns `EROFS`; stage remains, apply aborts, user sees "unable to install — filesystem is read-only".
- **Service manager unreachable** (systemd DBus denied, etc.) → swap completes but restart doesn't happen; post a notification asking the user to restart. Next boot picks up the new binary regardless.
- **Docker detected mid-flight** (shouldn't happen, but env vars lie) → abort apply, surface notification.
- **Clock skew causing Sigstore verification to fail** → surface explicit error ("your system clock appears incorrect") instead of a generic signature-invalid message.

## Known limitations

- **Anonymous GitHub rate limit is 60/hr per IP.** Kino is nowhere near this in normal operation, but a homelab NAT'd behind a shared IP with many other GitHub consumers could in principle share-and-exhaust the quota. Our ETag-cached requests are cheap but still count.
- **Windows binary replacement leaves a `kino.old.exe` until reboot.** Visible to the user as a stray file. Documented; unavoidable given the platform.
- **Tray version skew is user-visible** until they restart the tray. No way around this without making the server kill and respawn the tray process, which is more intrusive than it's worth.
- **Sigstore verification requires a working system clock** — badly-skewed machines will fail verification. Error message must be clear.
- **Resumable downloads depend on GitHub's CDN honouring `Range:`** — which it does today, but it's not a contractual guarantee. Fallback to a full redownload on range-not-satisfied.
- **Crash-loop detection is heuristic.** If the previous binary also crashes in the same window, both are broken and the user has a manual-intervention situation. The notification is explicit about this.
- **No differential updates.** Each release is a full archive download (~40 MB for the server + tray + frontend bundle). Fine for most connections; users on very slow links will feel it.
- **Homebrew / apt / winget paths are detected by string matching on `current_exe()`.** Unusual installs (e.g. a user manually copied the Homebrew binary to `/opt/kino/`) won't match and will fall back to standalone mode, which may race with a subsequent `brew upgrade`. The `KINO_DEPLOYMENT` override covers this case.
- **Destructive migrations break auto-apply by design.** Users on major version bumps must click through a confirmation banner. This is a feature, not a regression.
