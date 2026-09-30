# Stream-only mode

> **Status:** Design sketch (2026-04-28). Not started; gated on
> nothing — landable any time. Two precursors are already shipped
> in the existing single binary (`watch_now` orchestration and
> `playback::stream_probe` partial-file ffprobe), which is what
> makes this proposal tractable rather than a from-scratch build.

A second top-level deployment of kino, packaged as a desktop
app, that acquires and plays content **without ever building a
persistent library**. Click play → torrent starts → librqbit
streams pieces to a bounded scratch buffer that hole-punches
behind the read head → ffmpeg JIT-transcodes that scratch into
HLS → player plays → on stop or window-close, the torrent + buffer
are evicted. The movie or episode never lands as a library file.

Crucially, **stream-only mode is a separate binary**, not a
runtime mode of the existing one:

- `kino` — today's binary. Library mode. System-service or
  desktop-tray. Unchanged.
- `kino-app` — new binary. Tauri-wrapped desktop app.
  Stream-only only.

Mode is a **build-time** truth. There is no runtime
`stream_only_mode` config flag; you cannot mix modes in the same
process. The two binaries share ~80 % of their code via a new
`kino-core` library crate, but ship as distinct artefacts to
distinct distribution channels.

---

## 1. Motivation

Today kino is *library-first*: every grab is intended to land
in `media_library_path`, get probed into `media` + `stream`
rows, get hardlinked from `download_path` for seeding, and
persist on disk until the cleanup subsystem deletes it (after
watched + delay).

That's the right default for a NAS-class host with terabytes of
storage and a "set it up once and forget" administrator. It's
the wrong default for:

- A user who wants **Netflix-style click-and-watch on their
  laptop** — open the app, watch a film, close the app.
- A user with a small SSD where the library would fill up
  quickly and the management overhead doesn't pay off.
- A user who just wants to **try kino** without committing to
  the install + service + library-management flow.
- A user thinking about an eventual TV/Cast client port where
  thin-client constraints make persistence impossible.

These users want a desktop app, not a self-hosted server. Stream-
only mode is that desktop app: a separate binary, separate
install flow, separate distribution channel, separate UX.

---

## 2. The two binaries

### `kino` (library-mode binary, unchanged)

- System service (systemd / launchd / Windows Service) or
  desktop tray (`--features tray`).
- mDNS-advertised on the LAN (`kino.local`).
- Browser-based UI at `http://localhost:8080`.
- Auto-downloads followed shows; library files persist; cleanup
  runs after watched + delay.
- Distribution: `.deb` / `.rpm` / `.msi` / Homebrew Cask / AUR /
  winget / Pi appliance / Docker.

### `kino-app` (stream-only desktop app, new)

- Foreground app: launched, used, closed, like Spotify or Steam.
- Tauri 2 native window pointing at an in-process Axum server on
  a random localhost port.
- No system service, no mDNS, no tray, no auto-download.
- DB persists between launches (watch state, follows, settings)
  but **torrent state never does** — librqbit session shuts down
  with the process.
- Distribution: `.dmg` / `.exe` (MSI) / `.AppImage` direct +
  Homebrew Cask + Microsoft Store + Flathub.

### What's shared (the user-facing 80 %)

Both binaries share: TMDB metadata, search/discovery,
calendar, up-next, follow-show, watchlists, watch progress,
Trakt scrobble, Cast, indexers, VPN, quality profile, the
entire frontend SPA. Only the *acquisition lifecycle* differs.

In stream-only mode the user can still follow a show, get it on
Up Next when an episode airs, see it on the calendar — clicking
Play just acquires the release ad-hoc rather than auto-grabbing
on air date.

### Comparison

| Aspect | `kino` (library) | `kino-app` (stream-only) |
|---|---|---|
| Process model | Background service | Foreground desktop app |
| Window | None — user opens browser | Tauri 2 native window |
| Tray | Optional (`--features tray`) | No |
| Auto-start at login | Default-on (service install) | Off (user launches) |
| mDNS | On — LAN-discoverable | Off — ephemeral |
| Shutdown handling | systemd graceful stop | Window-close → graceful exit |
| Persistence between launches | Library files + DB + everything | DB + watched + follows. **No torrent state** |
| Auto-download (followed shows) | Yes (`wanted_sweep` task) | No (click to grab) |
| Cleanup trigger | `watched_at + delay` | Playback-ended + grace |
| Storage backend | Default `FilesystemStorage` | `EphemeralStorage` (rolling buffer) |
| Subtitles | Pre-fetched at import | On-demand at play time |
| Trickplay | Pre-generated at import | None (acceptable) |
| Distribution | Server channels + Pi/Docker | Desktop-app channels |

---

## 3. Why this is tractable today

Five primitives are already in place:

1. **`watch_now` orchestration** — the "click play, get pixels"
   pattern with phase-1 placeholder + phase-2 background grab,
   `download.wn_phase` state machine.
2. **`playback::stream_probe`** — runs `ffprobe` against partial
   files at 5 MB threshold, caches results, emits
   `StreamProbeReady`. Already integrated into the prepare
   endpoint's `ByteSource::Stream` branch.
3. **librqbit `/stream/{file_idx}`** — Range-aware HTTP endpoint
   with implicit piece-prioritisation around the read head.
4. **librqbit `TorrentStorage` trait + `on_piece_completed`
   hook** — the storage trait is small and pluggable; the
   `on_piece_completed` callback gives us the eviction trigger
   for the rolling buffer (§6).
5. **Existing `--features tray` cargo feature pattern** — proves
   the compile-time-mode approach we need to mirror.

The split itself is small (~3–5 days of work, ~300 LOC of
boilerplate per the codebase audit). Most of that is workspace
shape; almost no domain-code rewriting.

---

## 4. Crate organisation

This is the architectural decision that keeps the two-mode codebase
maintainable. Three crates in the existing workspace:

```
backend/
├── Cargo.toml                      # workspace root
└── crates/
    ├── kino-core/                  # ~80% of code, mode-blind
    │   ├── lib.rs                  # public surface incl. serve()
    │   └── … all today's domain modules …
    ├── kino/                       # binary: library mode
    │   ├── Cargo.toml              # depends on kino-core w/ library features
    │   └── src/main.rs             # CLI dispatch + server boot
    └── kino-app/                   # binary: Tauri stream-only
        ├── Cargo.toml              # depends on kino-core w/ stream features
        ├── tauri.conf.json
        ├── icons/
        ├── build.rs                # tauri_build::build()
        └── src/main.rs             # Tauri shell, spawns kino-core's serve()
```

Pattern B from Lapce, Helix, Zellij. Three principles enforce
maintainability:

### 4.1 The binary crate IS the mode

There are **no** `mode-library` / `mode-stream` cargo features
that switch behaviour. The mode is which binary you build. This
sidesteps the open-since-2016 cargo issue #2980 (mutually
exclusive features) entirely — the cargo feature mechanism is
not used to choose between modes, only to opt into capabilities.

`kino-core` exposes capability features:

```toml
[features]
default = []
tray-icon-support = ["dep:tray-icon"]
mdns-support      = ["dep:mdns-sd"]
service-install   = []
ephemeral-storage = ["dep:nix"]
auto-download     = []   # wanted_sweep + import pipeline
```

Each binary opts into the capabilities it needs:

```toml
# crates/kino/Cargo.toml — library binary
[dependencies]
kino-core = { path = "../kino-core",
              features = ["tray-icon-support", "mdns-support",
                          "service-install", "auto-download"] }

# crates/kino-app/Cargo.toml — Tauri stream-only binary
[dependencies]
kino-core = { path = "../kino-core",
              features = ["ephemeral-storage"] }
tauri = "2"
tauri-build = { version = "2", features = [] }
```

`kino-core` itself never asks "what mode am I in?". Subsystems
that exist only in one mode (e.g. the import pipeline) are gated
on a capability feature (`auto-download`), so when `kino-app`
builds, those modules don't compile. This is the same pattern
the existing `tray` feature uses today.

### 4.2 Cross-cutting decisions go through a `Lifecycle` trait

A handful of decisions span subsystems and depend on the mode:

- What happens when a download completes? (library → import
  pipeline; stream-only → start eviction-grace timer)
- What does cleanup look at? (library → watched-aged Media;
  stream-only → playback-ended sessions)
- Which scheduler tasks tick? (library → wanted_sweep, auto_cleanup;
  stream-only → idle-eviction sweep only)
- Which storage backend does `download::torrent_client` use?

Rather than sprinkle `cfg(feature)` checks at each decision
site, `kino-core` defines:

```rust
pub trait Lifecycle: Send + Sync + 'static {
    fn on_download_complete(&self, ctx: &AppState, dl: &Download) -> BoxFuture<'_, Result<()>>;
    fn cleanup_strategy(&self) -> CleanupStrategy;
    fn scheduler_tasks(&self) -> &'static [TaskKind];
    fn storage_factory(&self, scratch: &Path) -> Arc<dyn StorageFactory>;
}
```

`kino` provides `LibraryLifecycle`; `kino-app` provides
`StreamLifecycle`. Each binary's `main()` constructs the right
one and injects it into `AppState`. Domain code calls
`state.lifecycle.foo()` — completely mode-blind.

This centralises the ~6 mode-divergent decisions in two small
files (one per binary) instead of scattering cfg gates through
the codebase.

### 4.3 Three rules of engagement

Codified in `docs/architecture/`:

1. **Whole-module gating, not line-level.** If a subsystem is
   capability-gated (`#[cfg(feature = "auto-download")]`), the
   entire `pub mod foo;` declaration is gated, not lines inside
   shared functions. Small subsystems may end up entirely under
   one cfg; large shared subsystems never gain inline cfgs.
2. **`kino-core` knows nothing about modes.** No code in
   `kino-core` checks "am I library or stream-only?". Decisions
   land at the `Lifecycle` trait or as capability-feature gates.
3. **Mode-divergent code lives in mode-specific files.** A
   shared subsystem (e.g. `download/`) may have a sibling
   `download/storage/ephemeral.rs` (capability-gated). It does
   *not* gain an `if mode == … {}` branch inside an existing
   function.

These rules + Pattern B together keep the codebase from
becoming a `cfg(feature)` swamp.

### 4.4 Build & test discipline

Three CI jobs:

- `cargo nextest run -p kino-core` — runs once, with whichever
  capability features are needed for the tested code paths.
- `cargo nextest run -p kino` — library-binary-specific tests.
- `cargo nextest run -p kino-app` — stream-only-binary-specific
  tests.

No feature combinatorial explosion. No need for
`cargo-feature-combinations`.

**Critical CI footgun (per workspace research)**:
`cargo build --workspace` unifies features across all members
and would pull Tauri into `kino`'s build. Always use
`cargo build -p <crate>` in CI and locally. The justfile gets
updated accordingly.

---

## 5. The Tauri shell (`kino-app`)

Tauri 2 wraps the existing kino HTTP server in a native window.
This is "Tauri-as-installer" not "Tauri-as-framework" — we use
~30 % of Tauri's value prop (window, native menu, bundler,
updater, signing pipeline) and skip the rest (IPC commands,
custom protocol asset loader, JS plugin ecosystem) because our
SPA already talks to our Axum server over plain HTTP/WS.

### 5.1 Architecture

```rust
fn main() {
    tauri::Builder::default()
        .menu(build_native_menu())
        .setup(|app| {
            let handle = app.handle().clone();
            let (port_tx, port_rx) = std::sync::mpsc::channel();

            // Spawn kino-core's Axum server on a random localhost port.
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await.unwrap();
                port_tx.send(listener.local_addr().unwrap().port()).unwrap();
                let lifecycle = StreamLifecycle::new();
                kino_core::serve(listener, lifecycle, shutdown_signal()).await
            });

            let port = port_rx.recv().unwrap();
            WebviewWindowBuilder::new(
                &handle, "main",
                WebviewUrl::External(format!("http://127.0.0.1:{port}").parse().unwrap()))
                .title("Kino")
                .inner_size(1280.0, 800.0)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(|app_handle, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    kino_core::shutdown().await; // ffmpeg, torrents, DB flush
                    handle.exit(0);
                });
            }
        });
}
```

About 200 LOC including menu, error reporting, and platform
shims. Patterns documented in [Tauri 2 setup hook](https://v2.tauri.app/develop/) +
the localhost discussion thread.

### 5.2 Why we self-spawn Axum (not a Tauri plugin)

Two plugins exist; neither is right:

- `tauri-plugin-localhost` — works, but adds an extra layer +
  carries explicit security warnings. Since we already have
  `serve()` in `kino-core`, calling it directly is cleaner.
- `tauri-plugin-axum` — **breaks HLS playback**. It routes
  requests through a custom `axum://` protocol intercepted by
  the WebView. Per docs: "custom protocols currently do not
  support streaming." Fatal. Skip.

Self-spawning gives us:

- One async runtime under our control.
- Random port via `bind("127.0.0.1:0")` (different port each
  launch — prevents conflicts when the user has another kino
  process running).
- The same `serve()` function `kino` uses, no API drift.

Security: the localhost server is reachable by any process on
the box. Mitigated by (a) `127.0.0.1`-only bind, (b) random
port, (c) the existing per-install API-key cookie auth that
kino issues via `/api/v1/bootstrap`. Same threat model as kino
running on `localhost:8080` today — actually slightly stronger
(random port).

### 5.3 Shutdown semantics

`RunEvent::ExitRequested` + `api.prevent_exit()` lets us run
async cleanup before the process actually exits:

1. User closes window (or `Cmd+Q` / `Alt+F4`).
2. Tauri fires `ExitRequested`.
3. We prevent the default exit; spawn an async task.
4. Task awaits `kino_core::shutdown()`:
   - Drain the active transcode session (`q\n` to ffmpeg, then
     SIGKILL fallback — existing graceful-shutdown path).
   - Drop the active torrent via `Session::delete()`.
   - Cleanup scratch directory.
   - Flush sqlx writes; close pool.
5. Task calls `handle.exit(0)`.

Caveat from the Tauri issue tracker: this path **doesn't
always fire** (Windows OS shutdown, tiling-WM force-close, etc).
**Therefore kino must remain crash-safe** — DB writes durable,
partial transcodes resumable, no in-memory-only invariants.
This already holds for kino today (sqlx + librqbit both survive
`kill -9`). Treat the graceful path as a fast-path optimisation,
not a guarantee.

Forced window-close-equals-exit on macOS (which by default
keeps the app running with no windows): override via the
`RunEvent::ExitRequested` handler to unconditionally exit.

### 5.4 Native menu + system integration

All in Tauri 2 core, no plugins:

- `MenuBuilder` + `PredefinedMenuItem` for native menubar.
  macOS auto-promotes the first submenu to the app menu (`Kino
  > About / Quit`).
- `WindowEvent::DragDrop` for accepting `.torrent` files dropped
  onto the window — wires to existing acquisition endpoints.
- Keyboard accelerators on menu items (`Cmd+Q`, `Cmd+W`, etc.).

### 5.5 Bundle size

Without bundling jellyfin-ffmpeg (kino downloads it on first
run via Settings, per existing memory):

- Linux `.AppImage`: ~30–50 MB
- macOS `.app`: ~25–40 MB
- Windows `.msi`: ~30–45 MB

If Tauri's `bundleMediaFramework` is enabled (GStreamer for
WebKitGTK HTML5 video), Linux balloons to ~100–130 MB. **We
disable it** — the SPA never asks the WebView to play video
itself; ffmpeg handles all video, the player consumes HLS.

### 5.6 Code signing

| Platform | Path | Cost |
|---|---|---|
| macOS | Apple Developer ID + notarisation | $99/yr Developer Program |
| Windows | Azure Trusted Signing + `trusted-signing-cli` | ~$10/mo, EV-equivalent SmartScreen reputation |
| Linux | No standard; AppImage signing rarely consumed | Free |

Per ADR 0007 (no paid signing at launch), ship unsigned
initially with the right-click-Open / Run-anyway dance
documented. Add signing when traffic justifies it. Apple
Developer Program is the higher-priority spend (Gatekeeper is
scarier than SmartScreen).

### 5.7 Auto-update

Use Tauri's official updater plugin:

- Pulls a static `latest.json` from GitHub Releases — no update
  server required, no telemetry leak.
- Mandatory Ed25519 signing (private key in CI secrets, public
  key compiled into the binary).
- `tauri-action` generates the JSON on release.
- Linux: AppImage only (`.deb`/`.rpm` users update via
  apt/dnf — which is what they expect).

This dovetails with subsystem 27 (auto-update); the Tauri
plugin handles `kino-app`, the existing roadmap-27 mechanism
handles the `kino` server binary.

### 5.8 What's NOT in scope for the Tauri shell

- **MSIX / Microsoft Store native packaging.** Tauri 2 doesn't
  produce MSIX natively. The Microsoft Store path is "submit
  the MSI inside a Store listing" — works for kino-app the same
  way it works for any unsigned MSI. The in-flight MSIX branch
  for `kino` (the server binary) stays its own track.
- **Tauri IPC commands / state** — we don't use them. The SPA
  talks to kino's Axum API as it does today.
- **Custom protocol asset loader** — we don't use it. The SPA
  is served by kino's existing `rust-embed` route.
- **Mobile builds (iOS, Android)** — out of scope. Tauri 2
  supports them; kino's UX doesn't fit a phone screen.

---

## 6. Storage strategy: rolling buffer

This is `kino-app`'s killer technical detail. `kino-core` ships
an `EphemeralStorage` impl of `librqbit::storage::TorrentStorage`,
gated behind the `ephemeral-storage` capability feature, that
hole-punches pieces outside a bounded keep-window on every
verified piece.

### 6.1 What librqbit gives us

```rust
trait TorrentStorage: Send + Sync {
    fn init(&mut self, shared: &ManagedTorrentShared, metadata: &TorrentMetadata) -> Result<()>;
    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> Result<()>;
    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> Result<()>;
    fn remove_file(&self, file_id: usize, filename: &Path) -> Result<()>;
    fn remove_directory_if_empty(&self, path: &Path) -> Result<()>;
    fn ensure_file_length(&self, file_id: usize, length: u64) -> Result<()>;
    fn take(&self) -> Result<Box<dyn TorrentStorage>>;
    fn on_piece_completed(&self, _piece_index: ValidPieceIndex) -> Result<()> { Ok(()) }
}
```

Two methods are exactly the hooks needed:

- **`pread_exact`** — every byte ffmpeg pulls through librqbit's
  `/stream/{file_idx}` HTTP endpoint comes through here. We
  record a per-file "read head" as a side effect.
- **`on_piece_completed`** — fires after librqbit verifies a
  piece. We use this to discard pieces that have fallen outside
  the retention window.

librqbit's default `FilesystemStorage` already uses sparse
files (explicit on Windows via `FSCTL_SET_SPARSE`; implicit on
Linux/macOS), so hole-punching mid-file is well-defined.

### 6.2 `EphemeralStorage` design

```rust
pub struct EphemeralStorage {
    inner: FilesystemStorage,
    state: Arc<Mutex<HeadState>>,
    keep_behind_bytes: u64,    // default 256 MB
    keep_ahead_bytes: u64,     // default 512 MB
    pin_head_bytes: u64,       // default 8 MB — file start always-resident
    pin_tail_bytes: u64,       // default 8 MB — file end always-resident (moov-at-end)
    piece_size: u64,
}

struct HeadState {
    /// Furthest-forward byte read per file (driven by ffmpeg pread).
    read_head: HashMap<usize, u64>,
    /// File-byte ranges we've punched holes in. Reads here return Err.
    evicted: HashMap<usize, RangeSet<u64>>,
}
```

**Read path** (`pread_exact`):
1. Advance `read_head[file_id]` to `max(head, offset + buf.len())`.
2. If the read range intersects an evicted range, return
   `Err(SeekBeyondKeepBehind)` — see §6.4.
3. Otherwise delegate to `self.inner.pread_exact(...)`.

**Write path** (`pwrite_all`): plain delegation.

**Eviction** (`on_piece_completed`):
1. Compute the piece's byte range.
2. For each file the piece touches, fetch `read_head[file_id]`.
3. Define the keep-window:
   - `lo = max(0, head - keep_behind_bytes)`
   - `hi = head + keep_ahead_bytes`
   - Plus pinned regions `[0, pin_head_bytes)` and
     `[file_len - pin_tail_bytes, file_len)`.
4. If the piece falls entirely outside the keep-window AND
   outside both pinned regions: hole-punch on disk and record
   the eviction.

### 6.3 Per-OS hole-punch

A small `kino_core::download::storage::punch` module wrapping
the OS primitives (~150 LOC):

| Platform | Syscall |
|---|---|
| Linux | `fallocate(FALLOC_FL_PUNCH_HOLE \| FALLOC_FL_KEEP_SIZE)` — ext4/xfs/btrfs/tmpfs |
| macOS | `fcntl(F_PUNCHHOLE)` — APFS only (default since 10.13) |
| Windows | `DeviceIoControl(FSCTL_SET_ZERO_DATA)` — needs sparse flag, librqbit sets it |

Behind a single `hole_punch(fd, off, len) -> Result<()>` helper
using `nix` on Unix and `windows-sys` on Windows.

### 6.4 Resident-set shape under load

With the defaults above:

- **~800 MB resident scratch per active stream**, regardless
  of source-file size. Fits in a 2 GB tmpfs with headroom.
- **`stream_max_source_bytes`** (default 30 GB) is kept as
  defence-in-depth: a malformed torrent with broken
  piece-completion would otherwise grow unbounded if eviction
  silently failed.
- **Single-stream contract** (one playback session at a time
  per the existing single-user ADR). Concurrent streams scale
  linearly.

For comparison: a cap-only design (no rolling buffer) would
hold the *whole* partial file (~5–50 GB) for the duration of
playback. The rolling buffer cuts that by 30–60×.

### 6.5 Seek-back recovery

Seeks within the keep-window are free. Seeks beyond
`keep_behind_bytes` from the current head fall into evicted
territory; `pread_exact` returns `Err(SeekBeyondKeepBehind)`,
which propagates through librqbit's `/stream/{file_idx}` as a
connection error. ffmpeg sees it as a read failure on its
input.

Recovery (extends the existing transcode profile-chain machinery):

1. Detect `SeekBeyondKeepBehind` via stderr signature.
2. Don't advance the profile chain (this isn't an HW failure).
3. Restart ffmpeg at the player's current segment offset (which
   is past the eviction since it's the new read head).
4. librqbit's piece prioritiser re-orients to the new range.

Acceptable behaviour: HLS players seek to segment boundaries,
≤ 6 s ≈ 25 MB at 25 Mbps, well within the 256 MB keep-behind.
Only "scrub from minute 90 back to minute 5" lands in evicted
territory, and that's the documented edge.

### 6.6 Why this is in `kino-core` (not `kino-app`)

`EphemeralStorage` lives in `kino-core` behind the
`ephemeral-storage` feature, not in `kino-app`. Reasons:

- It's domain code (storage behaviour), not shell code.
- Tests for it run as `kino-core` unit/integration tests (no
  Tauri dependency needed for testing).
- Future: if we ever want a "stream-only-on-Pi-server"
  appliance, the same `kino` server binary could opt into the
  feature via its deps line. (Not in scope today.)

---

## 7. Per-subsystem changes

The codebase audit found the split is a small, surgical change.
Most subsystems move into `kino-core` unchanged.

### 7.1 Subsystems that move to `kino-core` unchanged

`tmdb`, `metadata`, `indexers`, `acquisition`, `download` (most
of), `playback`, `watch_now`, `vpn`, `auth_session`,
`notification`, `events`, `scheduler` (infra), `content`,
`library` (read queries), `home`, `integrations`,
`auth`, `settings`, `api`, `flow_tests`, `test_support`.

These are mode-blind. Each binary uses them.

### 7.2 Subsystems gated to one binary's feature set

| Subsystem | `kino-core` feature | Pulled in by |
|---|---|---|
| `import/` (entire pipeline, ~680 LOC) | `auto-download` | `kino` |
| `cleanup/retention.rs` (watched-aged delete) | `auto-download` | `kino` |
| `mdns/` | `mdns-support` | `kino` |
| `service_install/{linux,macos,windows}` | `service-install` | `kino` |
| `tray/` (existing) | `tray-icon-support` | `kino` |
| Library-only HTTP routes (`/api/v1/movies`, `/api/v1/shows`, `/api/v1/calendar`, etc.) | `auto-download` (covers wanted_sweep + library admin) | `kino` |
| `download/storage/ephemeral.rs` | `ephemeral-storage` | `kino-app` |
| `download/storage/punch.rs` | `ephemeral-storage` | `kino-app` |

The CLI subcommands `InstallService`, `UninstallService`,
`SetupPermissions`, `AllowFirewall`, `Tray`, `InstallTray`,
`UninstallTray` only exist on `kino`'s `clap::Subcommand` enum
(they live in `kino/src/main.rs`, not `kino-core`).

### 7.3 The `Lifecycle` trait

Defined in `kino-core`. Two impls live in their respective
binary crates:

```rust
// crates/kino/src/lifecycle.rs
pub struct LibraryLifecycle;

impl Lifecycle for LibraryLifecycle {
    fn on_download_complete(&self, ctx: &AppState, dl: &Download) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            kino_core::import::trigger::run(ctx, dl).await
        })
    }
    fn cleanup_strategy(&self) -> CleanupStrategy {
        CleanupStrategy::WatchedAged {
            movie_delay_hours: ctx.config.auto_cleanup_movie_delay,
            episode_delay_hours: ctx.config.auto_cleanup_episode_delay,
        }
    }
    fn scheduler_tasks(&self) -> &'static [TaskKind] {
        &[TaskKind::WantedSweep, TaskKind::AutoCleanup,
          TaskKind::OpenSubtitlesPrefetch, TaskKind::Reconcile]
    }
    fn storage_factory(&self, scratch: &Path) -> Arc<dyn StorageFactory> {
        Arc::new(librqbit::FilesystemStorageFactory::new(scratch))
    }
}

// crates/kino-app/src/lifecycle.rs
pub struct StreamLifecycle;

impl Lifecycle for StreamLifecycle {
    fn on_download_complete(&self, _ctx: &AppState, _dl: &Download) -> BoxFuture<'_, Result<()>> {
        // No-op; stream-only never imports.
        Box::pin(async { Ok(()) })
    }
    fn cleanup_strategy(&self) -> CleanupStrategy {
        CleanupStrategy::PlaybackEnded { grace_secs: 60 }
    }
    fn scheduler_tasks(&self) -> &'static [TaskKind] {
        &[TaskKind::IdleEvictionSweep, TaskKind::Reconcile]
    }
    fn storage_factory(&self, scratch: &Path) -> Arc<dyn StorageFactory> {
        Arc::new(EphemeralStorageFactory::new(scratch, /* defaults */))
    }
}
```

Total Lifecycle-related code: ~150 LOC in `kino-core` (trait +
shared types) + ~80 LOC per binary impl.

### 7.4 Cast token reshape (small breaking change, both modes benefit)

Today's `playback::cast::cast_token_for_media` requires
`media_id`, which doesn't exist in stream-only mode. Replace
with `cast_token_for_entity(kind, entity_id)`. The receiver
only needs the signed playback URL, which is already
`(kind, entity_id)`-routed. Library mode benefits too:
removes a needless `media_id → (kind, entity_id)` indirection.

### 7.5 Watched-status persistence (already works, no change)

Verified against current code: `watched_at` lives on the
`movie` / `episode` row, set by `playback::progress` keyed on
`(kind, entity_id)`, only nulled by explicit user-unwatch.
Cleanup deletes `media` / `stream` rows but never touches
`watched_at`.

For stream-only mode this means: a title watched and discarded
**stays watched** in History, in derived state, and in Trakt.
Re-watching the same title later (after the torrent was
evicted) increments `play_count` exactly like re-watching in
library mode after cleanup. No code change needed; covered by
existing tests + new `flow_tests/stream_only_full.rs`.

### 7.6 Subtitles (small)

Today subtitles are pre-fetched at import. `kino-app` has no
import. Change: fetch on demand at `prepare` time when the
client requests an external-subs language; cache for the
duration of the playback session; evict with the torrent.
Acceptable cold-start latency; the import-side fetch was
already async.

### 7.7 OpenAPI / generated SDK

The OpenAPI spec is generated per-binary. Library-only routes
(`/api/v1/movies/*`, `/api/v1/calendar`, etc.) appear in
`kino`'s spec but not `kino-app`'s. The frontend SDK
codegen runs once per binary (pulling each binary's
`openapi.json`) — but since they share the SPA, codegen for
the SPA targets the **library** spec (it's a superset), and
the frontend hides routes that return 404. Same SPA build,
two server APIs, gracefully degrading.

---

## 8. Frontend mode-awareness

The SPA is a single React app served by both binaries via
`rust-embed` (compiled into `kino-core`).

It learns the mode from one new field on the existing
`/api/v1/bootstrap` response:

```json
{
  "session_id": "…",
  "csrf_token": "…",
  "mode": "library" | "stream-only"
}
```

A small `useMode()` hook reads it once at app boot. Routes and
components branch off that hook:

- **Library tab**: hidden when `mode === 'stream-only'`.
- **History tab** (new): visible when `mode === 'stream-only'`.
- **Calendar / Up-Next / Follow / Watchlists**: visible in
  both. Click-Play does `watch_now` either way.
- **Settings → Library / Naming / OpenSubtitles tabs**: hidden
  when stream-only.
- **Settings → General → Mode**: read-only display;
  > Mode: Stream-only. To change, install the kino server.
- **Settings → Streaming tab**: visible only in stream-only,
  surfaces scratch path, keep-window settings, idle timeout.
- **Downloads tab**: shows `Streaming` badge instead of
  `Seeding` rows.
- **Player**: no changes — already handles
  `PlayPrepareReply.media_id: null` and polls `/prepare` on 202.

Total frontend churn: ~200 LOC of conditional rendering + the
new History route + the new Streaming settings tab. No SDK
regeneration churn — the bootstrap field is one new optional
property.

---

## 9. Lifecycle: a click-to-cleanup walkthrough

User opens `Kino.app` (the `kino-app` Tauri shell).

1. **App launch**: Tauri spawns kino-core's Axum server in a
   `tokio::task` on a random localhost port. Window opens
   pointing at it. SPA boots. `bootstrap.mode === 'stream-only'`.
2. **Discovery**: user browses to a film's detail page (TMDB
   metadata, poster, runtime). Click Play.
3. **Watch-now**: `POST /api/v1/play/movie/{tmdb_id}/watch_now`.
   PhaseOne returns a placeholder `download_id`; player
   navigates to the player route.
4. **Background grab**: `acquisition::search` finds candidate
   releases, `acquisition::policy` filters by stream-only rules
   (rejecting >30 GB, weighting current-seeders 3×). PhaseTwo
   adds the winning torrent to librqbit pointing at scratch via
   `EphemeralStorage`.
5. **Probe**: download monitor detects metadata, picks the
   right file, sets `download.file_idx`. At 5 MB,
   `playback::stream_probe` runs ffprobe; emits
   `StreamProbeReady`.
6. **Prepare**: player polls `/prepare`. First poll returns
   202 (metadata not ready). Subsequent poll returns the prepared
   response with audio/subtitle tracks from the probe cache,
   `media_id: None`.
7. **Playback**: player requests `/master.m3u8`. Transcode
   session is keyed `{kind}-{entity_id}-{tab_nonce}`. ffmpeg
   input is the librqbit `/stream/{file_idx}` URL; HLS output
   to a tmpfs scratch directory. Player plays.
8. **In-flight eviction**: as ffmpeg reads forward,
   `EphemeralStorage::pread_exact` advances the read head;
   `on_piece_completed` for older pieces hole-punches them.
   Resident scratch holds steady at ~800 MB.
9. **Watch progress**: player posts every 10s; watch state
   keyed by `(kind, entity_id)`, watched_at fires at 80 %.
10. **Stop**: user closes the player tab, or hits `Cmd+W`, or
    closes the whole app.
11. **Graceful shutdown** (window-close): Tauri fires
    `RunEvent::ExitRequested`. We `prevent_exit()`, run
    `kino_core::shutdown()`:
    - Active transcode session: `q\n` to ffmpeg, 5 s timeout,
      SIGKILL fallback.
    - Active torrent: `Session::delete()`.
    - Scratch dir: rmrf.
    - sqlx: drain + close pool.
    - `handle.exit(0)`.
12. **Next launch**: DB has the watched_at, the followed
    shows, the watchlists. User sees the film in History. No
    torrent state to restore.

---

## 10. Settings & defaults

Config columns in `kino-core` (used by `kino-app` only since
`kino` doesn't enable `ephemeral-storage`):

| Setting | Default | Rationale |
|---|---|---|
| `stream_keep_behind_bytes` | 256 MB | Rolling buffer behind read head; covers casual scrub-back |
| `stream_keep_ahead_bytes` | 512 MB | Rolling buffer ahead; covers HLS lookahead |
| `stream_pin_head_bytes` | 8 MB | File start always resident |
| `stream_pin_tail_bytes` | 8 MB | File end always resident (moov-at-end MP4/MKV) |
| `stream_scratch_max_bytes` | 2 GB | Defence-in-depth cap |
| `stream_max_source_bytes` | 30 GB | Sanity cap on accepted releases |
| `stream_cleanup_grace_secs` | 60 | "I'll be right back" buffer |
| `stream_idle_torrent_timeout_secs` | 1800 | 30 min idle torrent → drop |
| `stream_seeder_weight` | 3.0 | Stream-only release scoring multiplier |

Per-platform default scratch path:

| Platform | Default | Notes |
|---|---|---|
| macOS | `~/Library/Caches/Kino/scratch` | APFS — supports `F_PUNCHHOLE` |
| Windows | `%LOCALAPPDATA%\Kino\scratch` | NTFS sparse — supports `FSCTL_SET_ZERO_DATA` |
| Linux | `$XDG_CACHE_HOME/kino/scratch` | ext4 / xfs / btrfs all support `fallocate(PUNCH_HOLE)` |

Configurable in Settings → Streaming if the user wants to point
at a different volume (e.g. an external SSD).

---

## 11. Build & distribution

### 11.1 `kino` (library-mode binary) — unchanged

Continues using subsystem 21's existing pipeline:
`.deb` / `.rpm` / `.msi` / Homebrew Cask / AUR / winget /
Pi appliance image / Docker. cargo-dist drives.

### 11.2 `kino-app` (Tauri stream-only) — new pipeline

cargo-dist already handles multi-binary workspaces from v0.31.
Adding `kino-app` to the workspace gives us:

| Channel | Format | Mechanism |
|---|---|---|
| Direct download | `.dmg` (macOS) | `cargo tauri build` → DMG bundler |
| Direct download | `.exe` / `.msi` (Windows) | `cargo tauri build` → WiX bundler |
| Direct download | `.AppImage` (Linux) | `cargo tauri build` → AppImage bundler |
| Direct download | `.deb` / `.rpm` (Linux) | `cargo tauri build` → bundler |
| Homebrew Cask | `Kino.app` (macOS) | Tap formula |
| Microsoft Store | MSI inside Store listing | (Tauri 2 doesn't produce native MSIX) |
| Flathub | Flatpak | Manifest PR |

Code-signing per §5.6; deferred per ADR 0007.

### 11.3 CI shape

Add to existing `release.yml`:

- `cargo build -p kino-app` on each Tier-1 platform (matches
  the existing `kino` matrix).
- `cargo tauri build` step per platform to produce bundles.
- Updater plugin: generate `latest.json` per release, sign with
  Ed25519 key in CI secrets, attach to the GitHub Release.
- Smoke test: launch `kino-app` headless on each platform,
  verify the bootstrap response, exit cleanly. (Tauri's tauri-action
  ships a simulator for this.)

The existing `channels.yml` fan-out gains:

- Homebrew Cask formula bump for `kino-app`
- Flathub manifest PR
- Microsoft Store submission (manual, gated on signing being in
  place — deferred)

### 11.4 Versioning

Lockstep versioning with workspace.package.version inheritance.
Both binaries share a release tag. release-please handles
this via `release-type: rust-workspace`.

If desktop-app polish releases need to ship without bumping the
server binary, switch to independent versioning later. Don't
commit to that complexity day-one.

---

## 12. Testing

### 12.1 Unit tests

- `kino_core::download::storage::ephemeral` unit tests: pread
  advances read head; `on_piece_completed` punches holes for
  out-of-window pieces; pinned head/tail never punched; pread
  to evicted range returns `Err(SeekBeyondKeepBehind)`.
  Linux-only assertion that `stat.st_blocks` decreases.
- `kino_core::download::storage::punch` per-OS tests behind
  `cfg(target_os)`.

### 12.2 Integration tests (in `kino-core`)

- `flow_tests/stream_only_full.rs`: against the librqbit fake,
  assert prepare returns from probe cache, no `media` row,
  post-stop cleanup removes scratch, `download.state =
  cleaned_up`.
- `flow_tests/stream_only_resident_set.rs`: synthetic 4 GB
  source through 800 MB keep-window; assert resident scratch
  stays bounded.
- `flow_tests/stream_only_seek_back.rs`: small backward seek
  succeeds; seek beyond keep-behind triggers documented
  session-restart.
- `flow_tests/stream_only_grace.rs`: pause < grace ⇒ torrent
  alive, partial buffer reused; pause > grace ⇒ torrent
  evicted, re-grab on resume.

### 12.3 Binary-specific tests

- `kino` keeps its existing `flow_tests/`. Library-mode
  pipeline assertions.
- `kino-app` adds Tauri smoke tests: window opens, server is
  reachable, graceful shutdown completes within 5 s with active
  transcode + torrent.

### 12.4 CI matrix

Three nextest jobs — `-p kino-core`, `-p kino`, `-p kino-app`.
No feature combinatorial explosion. Tauri builds are gated on
the bundle workflow, not the unit-test workflow.

### 12.5 Manual smoke before release

Short runbook in `docs/runbooks/`: launch `kino-app`, click
play on a popular film, watch 30 s, close window, verify
scratch dir empties within 90 s, `download.state =
cleaned_up`. Repeat on macOS / Windows / Linux.

---

## 13. Phasing

Five phases. Each phase is shippable in isolation; subsequent
phases never landing is acceptable.

### Phase 1 — Crate split (no functional changes)

- Create `kino-core` crate; move existing modules into it.
- `kino` binary becomes a thin shim depending on `kino-core`.
- Capability features wired up (`auto-download`,
  `mdns-support`, etc.).
- `Lifecycle` trait + `LibraryLifecycle` impl.
- Existing tests pass; existing CI green; existing
  distribution unchanged.

Outcome: workspace shape ready for `kino-app` to slot in.
~3–5 days of work, ~300 LOC of boilerplate, zero behaviour
change.

### Phase 2 — `EphemeralStorage` + stream-only domain code

- `kino_core::download::storage::ephemeral` + `punch`.
- `StreamLifecycle` trait impl.
- `flow_tests/stream_only_*` against the librqbit fake.
- All gated behind `ephemeral-storage` capability feature; no
  binary depends on it yet.

Outcome: stream-only behaviour testable in `kino-core` without
shipping anything.

### Phase 3 — `kino-app` binary

- New `kino-app` crate with Tauri shell.
- `tauri.conf.json`, icons, native menu.
- Spawn-Axum-in-setup wiring.
- `RunEvent::ExitRequested` graceful shutdown.
- Bootstrap response carries `mode: "stream-only"`.
- Frontend mode-awareness (one new hook + conditional
  rendering for the ~5 mode-divergent surfaces).

Outcome: developer running `cargo tauri build -p kino-app`
gets a working `.app` / `.exe` / `.AppImage` they can launch
and watch a film in.

### Phase 4 — Distribution + signing

- cargo-dist multi-binary release pipeline.
- `kino-app` published to direct download, Homebrew Cask,
  Flathub.
- Tauri updater wired to GitHub Releases.
- macOS Developer Program enrolment + notarisation pipeline.
- Azure Trusted Signing for Windows.

Outcome: `kino-app` is a real shipped product on the desktop.

### Phase 5 — Polish

- Drag-and-drop `.torrent` files onto the window.
- Native menu shortcuts (`Cmd+,` for settings, etc.).
- Microsoft Store submission (MSI listing — Tauri 2 can't do
  native MSIX).
- Usage telemetry: explicitly **off** per memory ("never add
  telemetry").
- Diagnostics view (per-session swap log if roadmap #35 ships).

---

## 14. Future enhancements

- **Re-fetch evicted pieces on seek-back** instead of
  session-restart. Likely needs a small librqbit upstream PR
  to expose a "forget piece" hook on the bitfield.
- **In-session trickplay generation** — sprites in background
  as pieces land, served from per-session in-memory cache,
  evicted with the torrent.
- **Hybrid mode** ("library-with-aggressive-cleanup") — if
  real users on Pi-class hardware want kino-the-server but
  don't want library persistence, allow `kino` to opt into
  `ephemeral-storage` via its deps line. Not in scope today.
- **Mobile builds** (iOS via Tauri 2). UX problem: kino's
  existing SPA isn't designed for phone screens. Out of scope.

Tracked separately:

- **Adaptive multi-source streaming** (#35) — orthogonal
  feature that benefits both binaries.
- **Auto-update** (#27) — Tauri's updater plugin handles
  `kino-app`; existing roadmap-27 mechanism handles `kino`.

---

## 15. Open questions / verify in Phase 1

1. **`cargo tauri build` inside our workspace.** Run the actual
   command on a stub `kino-app` crate and confirm Cargo.lock at
   the workspace root works cleanly with `tauri-build`.
2. **`pread_exact` error semantics through librqbit's
   `/stream`.** When `EphemeralStorage` returns
   `Err(SeekBeyondKeepBehind)`, what does the HTTP handler do
   — drop the connection, return 5xx, hang? Determines whether
   the recovery path (§6.5) needs a session-restart trigger or
   just an ffmpeg-side reconnect.
3. **librqbit bitfield staleness after eviction.** Confirm
   librqbit doesn't crash or corrupt internal state when a
   piece it considers complete returns Err on read.
4. **Hole-punch on tmpfs.** Verify `st_blocks` actually
   decreases — tmpfs has had quirks here in older kernels.
5. **macOS APFS hole-punch alignment.** `F_PUNCHHOLE` requires
   block-aligned ranges; piece sizes are typically 256 KB–4 MB
   which align cleanly, but verify on the smallest piece-size
   we accept.
6. **MOOV-at-end MP4s + tail-pin sufficiency.** Empirical test
   — does pinning the last 8 MB cover the moov atom for the
   common case?
7. **Tauri 2 ARM64 build matrix.** Confirm `cargo tauri build`
   produces working artefacts on Linux ARM64 and Windows ARM64
   (the `windows-11-arm` GHA runner is recent).
8. **WebKitGTK 4.1 floor.** Decide which Linux distros we
   officially support given WebKitGTK 4.1 fragmentation
   (Ubuntu 22.04 ships 4.0 by default; bumping that may push us
   to Ubuntu 24.04 / Debian 12 minimum). Flathub sidesteps
   entirely if we go that route.
9. **`RunEvent::ExitRequested` reliability.** Empirical test:
   force-quit `kino-app` via Activity Monitor / Task Manager;
   verify DB integrity + scratch cleanup. (Should pass — kino
   is already crash-safe — but worth confirming.)
10. **Existing `watch_now` codex bugs** (#7 cancel race, #8
    retry suppression timing — referenced in
    `state-machines.md`). Re-pass when watch-now becomes the
    default flow for `kino-app`.

---

## 16. References

Internal:

- `docs/architecture/crate-layout.md` — domain-module
  organisation. The split adds `kino-core` as a sibling of
  `kino` per Pattern B.
- `docs/architecture/state-machines.md` — `WatchNowPhase`
  sub-machine, the precursor we extend.
- `docs/decisions/0001-single-binary.md` — note: this becomes
  "two-binaries-from-one-workspace" when stream-only ships;
  worth a new ADR to amend.
- `docs/decisions/0007-no-paid-signing-at-launch.md` — applies
  unchanged to `kino-app`.
- `docs/subsystems/03-download.md`, `04-import.md`,
  `05-playback.md`, `06-cleanup.md`, `25-mdns-discovery.md` —
  capability-feature gates land in these subsystems.
- `docs/roadmap/21-cross-platform-deployment.md` — `kino`
  pipeline; `kino-app` adds a parallel pipeline.
- `docs/roadmap/35-adaptive-source-streaming.md` — orthogonal
  feature; both binaries benefit.

External:

- librqbit: [GitHub](https://github.com/ikatson/rqbit),
  [`TorrentStorage` trait](https://docs.rs/librqbit/latest/librqbit/storage/trait.TorrentStorage.html)
- Tauri 2: [Project Structure](https://v2.tauri.app/start/project-structure/),
  [Localhost Plugin](https://v2.tauri.app/plugin/localhost/),
  [Updater Plugin](https://v2.tauri.app/plugin/updater/),
  [Window Menu](https://v2.tauri.app/learn/window-menu/),
  [Microsoft Store distribution](https://v2.tauri.app/distribute/microsoft-store/),
  [Webview Versions](https://v2.tauri.app/reference/webview-versions/)
- Workspace pattern precedents:
  [Lapce](https://github.com/lapce/lapce),
  [Helix](https://github.com/helix-editor/helix),
  [Zellij](https://github.com/zellij-org/zellij)
- [Cargo issue #2980](https://github.com/rust-lang/cargo/issues/2980) — mutually-exclusive features (sidestepped by Pattern B)
- [RFC 3143](https://rust-lang.github.io/rfcs/3143-cargo-weak-namespaced-features.html) — `dep:` syntax
- [cargo-dist multi-binary workspaces](https://axodotdev.github.io/cargo-dist/book/workspaces/workspace-guide.html)
