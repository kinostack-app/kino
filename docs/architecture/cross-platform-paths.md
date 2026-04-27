# Cross-platform path resolution

How kino picks where to put its data, config, cache, and logs across
Linux / macOS / Windows in user-mode, system-service, and headless
(Docker / Pi appliance) deployments.

This is a synthesis of the paths leg of the cross-platform audit
(2026-04-26). Findings drawn from helix, lapce, espanso, and uv —
all proven cross-platform Rust shipping projects — and applied where
they fit our shape.

## Decision summary

- **Path resolution crate**: `etcetera`. Same pick as helix and uv.
- **Service-mode paths are NOT auto-detected** by the binary. Native
  packages pass `--data-path` (or set `KINO_DATA_PATH`) explicitly
  through their systemd unit / launchd plist / Windows Service
  descriptor. The binary's only fallback is the user-mode default.
- **Resolution priority** (highest first):
  1. `--data-path` CLI flag
  2. `KINO_DATA_PATH` env var
  3. Per-OS user-mode default via `etcetera::base_strategy`
  4. `./data` (when `etcetera` can't find the home dir at all — rare,
     happens in oddly-configured service contexts)

Implemented in [`backend/crates/kino/src/paths.rs`](../../backend/crates/kino/src/paths.rs)
and [`backend/crates/kino/src/main.rs`](../../backend/crates/kino/src/main.rs)
(see the resolution at the top of `fn main()`).

## Per-OS path matrix

Six logical roles map to a per-OS filesystem location. `etcetera`'s
`BaseStrategy` provides Linux + macOS + Windows defaults; the
service-mode column is what native packages set via their descriptor.

| Role | Linux user-mode | Linux system-service (`.deb` / `.rpm`) | macOS user-mode | macOS LaunchDaemon | Windows user-mode | Windows Service (LocalSystem) |
|---|---|---|---|---|---|---|
| **Data** | `$XDG_DATA_HOME/kino` (`~/.local/share/kino`) | `/var/lib/kino` | `~/Library/Application Support/Kino` | `/Library/Application Support/Kino` | `%LOCALAPPDATA%\Kino` | `%PROGRAMDATA%\Kino` |
| **Config** | `$XDG_CONFIG_HOME/kino` (`~/.config/kino`) | `/etc/kino` | `~/Library/Application Support/Kino` | `/Library/Application Support/Kino` | `%APPDATA%\Kino` | `%PROGRAMDATA%\Kino` |
| **Cache** | `$XDG_CACHE_HOME/kino` (`~/.cache/kino`) | `/var/cache/kino` | `~/Library/Caches/Kino` | `/Library/Caches/Kino` | `%LOCALAPPDATA%\Kino\Cache` | `%PROGRAMDATA%\Kino\Cache` |
| **Logs** | (journald via systemd; or `$XDG_STATE_HOME/kino/log` for user-mode) | journald | `~/Library/Logs/Kino` | `/var/log/kino` | (Event Log) or `%LOCALAPPDATA%\Kino\Logs` | (Event Log) or `%PROGRAMDATA%\Kino\Logs` |
| **Runtime state** | `$XDG_RUNTIME_DIR/kino` | `/run/kino` | `/var/tmp/kino-$USER` | `/var/run/kino` | `%TEMP%\Kino` | `C:\Windows\Temp\Kino` |
| **DB / persistence** | under data | under data | under data | under data | under data | under data |

References:
- Linux: [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) + Filesystem Hierarchy Standard for system paths
- macOS: [Apple File System Programming Guide](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html)
- Windows: [Microsoft Known Folders](https://learn.microsoft.com/en-us/windows/win32/shell/known-folders)

## Resolution algorithm

```text
fn resolve_data_path(cli, env) -> String {
    // 1. Explicit CLI flag wins
    if cli.data_path.is_some() {
        return cli.data_path;
    }
    // 2. Env var (used by docker-compose and native packages)
    //    Note: clap reads KINO_DATA_PATH automatically into cli.data_path
    //    via #[arg(env = "KINO_DATA_PATH")], so step 1 actually
    //    covers env vars too.
    // 3. Per-OS user-mode default
    paths::default_data_dir()
        // 4. Last-resort fallback if no home dir
        .unwrap_or("./data")
}
```

Native packages bypass steps 3-4 by writing the right path into their
descriptor — e.g. `backend/crates/kino/debian/service` has
`ExecStart=/usr/bin/kino serve --data-path /var/lib/kino`. So the
binary never has to ask "am I a system service?" — the install
decided that at package time.

## What's currently consumed

Today the resolver is wired through for **data only**. The other
roles (config, cache, logs, runtime) are exposed in
[`paths.rs`](../../backend/crates/kino/src/paths.rs) for symmetry but
not yet read by anything. Future migrations:

- **Cache split** — pull `transcode-temp/`, `images/` thumbnails out
  of the data dir into the OS cache dir so users can `rm -rf` cache
  without losing library state. See [`docs/subsystems/05-playback.md`](../subsystems/05-playback.md)
  for what currently lives where
- **Log destination** — when running under systemd / launchd, stderr
  goes to journald / unified logging. The SQLite log layer captures
  the same records for the Health dashboard. A file-based fallback
  (rotating logs in `~/Library/Logs/Kino`) is on the radar for
  user-mode runs without a service supervisor — see
  [`logging.md`](./logging.md) for the current logging architecture
- **Config file overrides** — kino's config lives in the SQLite
  `config` table today. A read-only TOML override (e.g.
  `~/.config/kino/local.toml`) is a possible future addition for
  power users; the path resolver is ready for it

## Service-mode detection — why we don't

The audit considered an approach where the binary detects "am I
running as a service?" via heuristics (`INVOCATION_ID` env from
systemd, `XPC_SERVICE_NAME` from launchd, etc.) and switches paths
automatically.

We rejected it because:

1. **False positives are silent and dangerous.** A user who happens to
   have an unrelated `INVOCATION_ID` in their env (testing systemd
   units in a shell) could write to `/var/lib/kino` from a tarball
   binary
2. **Native packages already pass the right path.** The systemd unit
   knows it's a service; it should just set the path. No detection
   needed
3. **Tarball + cargo-install users want predictable behaviour.**
   Running `kino serve` from a checkout should write to the same
   place every time, regardless of what wrappers happen to be in the
   environment
4. **The `--data-path` flag is a clean override.** Any user who wants
   service-style paths in user-mode just passes the flag

## Migration story

For users coming from any pre-release version of kino (we haven't
shipped a v1 yet, so this section is essentially future-proofing):

- **Kept `./data` for dev.** The devcontainer sets
  `KINO_DATA_PATH=./data` in `.env`, so dev workflows are unchanged
- **Tarball users on previous versions** would have set
  `KINO_DATA_PATH` or `--data-path` themselves; their explicit choice
  still wins
- **No silent migration of legacy paths.** If we ever want to detect
  "you have data at `./data` but no `KINO_DATA_PATH` set, want to
  migrate?", that's an explicit UX flow, not an implicit relocation

If we ship a release that *does* need to migrate user data
(restructure the data dir, move from data → cache split), the
migration prompt belongs in the web UI, not in path resolution. The
resolver only knows where data *should* live now.

## Open follow-ups

| Item | Where |
|---|---|
| Adopt cache dir for transcode-temp + image thumbnails | [`subsystems/05-playback.md`](../subsystems/05-playback.md) follow-up |
| Adopt log dir for file-based fallback when no service supervisor | [`architecture/logging.md`](./logging.md) follow-up |
| Document Windows multi-user posture (per-user `%LOCALAPPDATA%` vs shared `%PROGRAMDATA%`) once we have actual Windows users | This doc |
| Document migration prompt UX if we ever restructure on disk | New spec when needed |
