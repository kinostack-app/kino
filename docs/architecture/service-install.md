# Service install + lifecycle

How `kino install-service` (and friends) register kino as a system or
user service across Linux / macOS / Windows. Today the
implementation is a stub
([`backend/crates/kino/src/service_install.rs`](../../backend/crates/kino/src/service_install.rs))
that returns "not yet implemented." This doc is the design that the
implementation follows when it lands.

Findings from the cross-platform audit (2026-04-26): we read espanso
(only daemon-shaped Rust project in our reference set), inspected the
`service-manager` crate (verdict: unmaintained — last release 2020,
not used by anyone we examined), and confirmed the existing
`debian/service` + `rpm/kino.service` are correct.

## Decision summary

- **Hand-roll per-OS code** in `service_install/{linux,macos,windows}.rs`.
  Don't depend on `service-manager` (abandoned) or any other
  cross-platform abstraction
- **Use `windows-service` (Mullvad's)** for Windows SCM integration —
  it's the production reference and well-maintained
- **Native packages remain the primary install path.** The `.deb`,
  `.rpm`, `.msi`, and `.dmg` postinst / installer scripts handle
  service registration. `kino install-service` is the
  tarball / `cargo install` fallback only
- **Privilege model**: each subcommand documents its required
  privilege; on Windows we trigger UAC elevation automatically; on
  macOS we use `osascript` for the admin prompt; on Linux we
  require `sudo` / `systemctl --user` and fail with a clear message
  if the user runs unprivileged when root is needed

## Per-OS install matrix

| Step | Linux system (systemd) | Linux user (systemd) | macOS system (launchd) | Windows (SCM) |
|---|---|---|---|---|
| **Write descriptor** | `/etc/systemd/system/kino.service` (root) | `~/.config/systemd/user/kino.service` (user) | `/Library/LaunchDaemons/tv.kino.daemon.plist` (root via admin prompt) | SCM registry via `windows-service` API (admin/elevated) |
| **Reload daemon** | `systemctl daemon-reload` (root) | `systemctl --user daemon-reload` (user) | n/a | n/a |
| **Enable on boot** | `systemctl enable kino` (root) | `systemctl --user enable kino` (user) | (`launchctl load` registers) | `StartType::AutoStart` flag at registration |
| **Start now** | `systemctl start kino` (root) | `systemctl --user start kino` (user) | `launchctl load /Library/LaunchDaemons/...` (root) | `service.start()` via `windows-service` |
| **Set caps / privs** | `AmbientCapabilities=CAP_NET_RAW CAP_NET_ADMIN` in unit | n/a (user mode) | Entitlements in `.plist`; `RunAtLoad`, `KeepAlive` | Run as `LocalSystem` (default for `windows-service-rs`) |

References from cloned repos:
- `ref/espanso/espanso/src/cli/service/linux.rs:36-68` — systemd unit writing + `systemctl --user enable`
- `ref/espanso/espanso/src/cli/service/macos.rs:34-99` — plist writing + `launchctl unload`/`load`
- `ref/espanso/espanso/src/cli/service/win.rs:30-45` — Windows uses startup-shortcut, NOT SCM (espanso runs as user-session daemon, not service). Our case is different — we want SCM for the system service

## Code layout we'll add

```
backend/crates/kino/src/service_install/
├── mod.rs           ← dispatcher: cfg-gated to platform module
├── linux.rs         ← systemd unit writing + systemctl shell-out
├── macos.rs         ← launchd plist writing + launchctl shell-out
└── windows.rs       ← windows-service crate integration (target-gated)
```

Public API (replaces the stubs in `service_install.rs` today):

```rust
pub fn install(user_mode: bool) -> anyhow::Result<()>;
pub fn uninstall() -> anyhow::Result<()>;
pub fn status() -> anyhow::Result<ServiceStatus>;  // running/stopped/not-installed
```

Each platform module owns:
- The descriptor template (embedded as `include_str!` from the
  `debian/`, `rpm/`, or a new `service_install/templates/` dir)
- The shell-out commands (`systemctl`, `launchctl`) or API calls
  (`windows-service`)
- Idempotent register + enable + start sequence (re-running
  `install-service` on an already-installed system is a no-op)

## Privilege model

| Subcommand | Privilege required | Elevation handling |
|---|---|---|
| `kino serve` | None — runs as the calling user | Caller picks the privilege level |
| `kino install-service` (system mode) | Root (Linux/macOS) / Admin (Windows) | Linux: error out cleanly with `please run with sudo`; macOS: trigger via `osascript` admin prompt; Windows: re-launch elevated via UAC manifest |
| `kino install-service --user` | None (per-user systemd unit) | n/a — no elevation needed; documents the limitation that the service won't auto-start at boot, only when the user logs in |
| `kino uninstall-service` | Same as install | Same as install |
| `kino install-tray` | None (per-user autostart) | n/a |
| `kino uninstall-tray` | None | n/a |

## Lifecycle considerations

### Graceful shutdown

Linux and macOS: SIGTERM. Already handled in
`backend/crates/kino/src/main.rs:shutdown_signal()` via
`tokio::signal::ctrl_c()` (which catches SIGTERM too on Unix).

Windows SCM: `SERVICE_CONTROL_STOP` arrives via the SCM dispatcher,
not as a signal. We need to wire the `windows-service` dispatcher in
`fn main()` so `kino serve` running under SCM responds correctly.
That's a separate change tracked under "Windows SCM dispatcher".

### Restart policy

| Platform | Mechanism | Status |
|---|---|---|
| Linux systemd | `Restart=on-failure`, `RestartSec=5` in unit | ✓ in `debian/service` + `rpm/kino.service` + `service_install/linux.rs` |
| macOS launchd | `<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>` in plist | ✓ in `service_install/macos.rs` |
| Windows SCM | Service Recovery actions via `windows-service` API after registration | ✓ in `service_install/windows.rs` (5s × 2 then None) |

### Exit-after-restore

A successful POST to `/api/v1/backups/{id}/restore` (or the
upload variant) schedules `std::process::exit(75)` after a 1s
delay. The exit code is `EX_TEMPFAIL` — non-zero so every
supervisor we target reads it as a failure for restart-policy
purposes:

- **systemd**: `Restart=on-failure` catches the non-zero exit and
  starts a fresh process against the restored database
- **launchd**: `KeepAlive.SuccessfulExit=false` does the same
- **Windows SCM**: Service Recovery actions (Restart, 5s) trigger

Opt-in:

- systemd / launchd unit files set `KINO_RESTART_AFTER_RESTORE=1`
  in `Environment=` / `EnvironmentVariables` so production
  packages get the behaviour out of the box
- Windows SCM dispatcher (`service_runner.rs`) sets an in-process
  `AtomicBool` marker before tokio starts (rather than mutating
  the process env, since the workspace forbids `unsafe_code` and
  `std::env::set_var` is unsafe under Rust 2024)
- Tests + tarball / `cargo install` users without an env var see
  the legacy "Restart kino to load the restored database"
  message; the process keeps running, the user picks the moment

### Sleep / wake

- **Linux** (systemd): unit doesn't suspend during system sleep. On
  resume, network reconnects work as long as our HTTP / WS handlers
  re-bind / re-establish. VPN tunnel does need explicit reconnect —
  see `download/vpn/` for the existing reconnect logic
- **macOS** (launchd): LaunchDaemons survive sleep but `utun`
  interfaces can disconnect. Document as a known v1 limitation;
  reconnect is on the VPN code's hook
- **Windows** (SCM): Service Power Events can be subscribed to, but
  for v1 we rely on the existing reconnect logic

### Single-instance enforcement

For the **service**: rely on port-binding failure. If `kino serve`
tries to bind `:8080` and another instance already holds it, axum's
listener returns `EADDRINUSE` and the binary exits with a clear
error. No explicit lock file needed.

For the **tray**: already implemented in
`backend/crates/kino/src/tray/lock.rs` via `fs4` cross-platform file
locking (different from the service — multiple users on a multi-user
system can each have their own tray).

### Watchdog (sd_notify)

systemd supports `Type=notify` units that signal readiness via
`sd_notify(READY=1)`. We use `Type=simple` today, which is less
precise (systemd assumes ready as soon as the binary starts). v1
keeps `Type=simple`; if startup-time observability becomes a
priority we can add `libsystemd` dependency + `sd_notify` calls.
Defer.

## Uninstall posture

`kino uninstall-service` removes the descriptor and stops the
service. **It does NOT remove user data** — config, the SQLite DB,
the librqbit session, image cache, backups. Standard convention for
self-hosted apps: uninstall is for the daemon, not for the data.

Document in CLI help:

> Uninstalling does NOT delete your data. To wipe everything, run
> `kino reset` first, then `kino uninstall-service`.

## Open design decisions

| Question | Status |
|---|---|
| Run Windows service as `LocalSystem` (default) vs a dedicated `kino` service account? | **LocalSystem.** Standard for self-hosted Windows services; avoids per-user permission complications. Document explicitly |
| Auto-trigger UAC elevation on `kino install-service` (Windows)? | **Yes** — embed UAC manifest so the elevation prompt fires automatically. Less friction than "please re-run as admin" |
| Auto-trigger admin prompt via `osascript` on `kino install-service` (macOS)? | **Yes**, same reasoning |
| sd_notify integration for systemd `Type=notify`? | **Defer.** `Type=simple` is fine for v1 |
| Service Recovery actions on Windows? | **Yes** — set restart-on-failure with 5s delay via `windows-service` API at registration time |

## Tasks this generates

- **Implement service_install per-OS modules** — the actual code
  behind the stubs. Sized as one task because each OS is small in
  isolation but the cross-platform testing matrix is the cost
- **Wire Windows SCM dispatcher in main.rs** — conditional path so
  `kino serve` invoked by SCM goes through `service_dispatcher::start`
  instead of straight to `tokio::Runtime::block_on`
- **Add KeepAlive to macOS plist template** — small, but needs to land
  with the macOS install impl
- **CLI elevation handling** — Windows UAC manifest in `Cargo.toml`
  build metadata + `osascript` shell-out on macOS

Tracked under tasks #523 (impl) and #524 (Windows SCM) — see task
list.
