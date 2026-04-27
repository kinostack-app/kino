# CLI companion

> **Not yet implemented.** No `kino-cli` crate in the workspace —
> the only binary today is `kino` itself (server + frontend). This
> document is design-only.

A first-class Unix CLI (`kino-cli`) shipped alongside the kino server. Same Cargo workspace, separate binary, separate release artifact. Talks to kino over the HTTP API — no direct DB access — so a single CLI binary works against a local install, a remote instance over Tailscale, or anything behind a reverse proxy. Designed to feel like `gh` / `fly` / `cargo`: composable, pipeable, premium.

## Scope

**In scope:**
- Separate binary (`kino-cli`) built from the same workspace, shipped in the same release archive as `kino`
- HTTP API client — no DB access, no process-level coupling to the server
- Subcommand surface covering the common media-server operations (search, add, list, download control, play, scan, health, logs)
- Machine-first output (`--output json`) and human-first output (tables), auto-switching based on `isatty()`
- `--jq` filter on JSON output (embedded `jaq`) so scripts don't need to pipe through `jq`
- OS-keychain-backed API key storage with TOML profile config for multi-server use
- `kino api` escape hatch — raw HTTP passthrough for commands the CLI doesn't yet wrap
- Plugin mechanism — out-of-tree executables discoverable as `kino <name>` subcommands
- Shell completions (bash, zsh, fish, nushell, elvish) and man pages generated from the argument tree
- Distribution via `cargo-dist`: Homebrew tap, shell installer, `.deb`, `.msi`, release tarballs

**Out of scope:**
- Telemetry, usage analytics, crash reporting, update-check pings — kino is self-hosted; the binary never contacts an external service uninvited
- TUI dashboard (there's no `lazykino`) — the web UI owns rich interaction; CLI is for piping and scripting
- OpenAPI code generation (we share a types crate instead — see Architecture)
- Local-mode (talking to the DB directly). Everything goes through the HTTP API, uniform across local and remote
- Managing the server's lifecycle via SSH or orchestration plugins — use the OS service manager
- Interactive shell / REPL mode
- Embedded playback — `kino play` resolves a stream URL and launches the user's media player; it does not decode video itself

## Architecture

### Workspace layout

Four crates. The CLI is kept deliberately small; the server stays the authoritative source of API shape.

```
backend/crates/
├── kino/                 # server binary (existing)
├── kino-api/             # NEW — shared request/response types + route path constants
├── kino-client/          # NEW — thin reqwest wrapper around kino-api
└── kino-cli/             # NEW — clap-based CLI, depends on kino-client
```

- **`kino-api`** — pure-data crate. Structs for every request and response, derives for `serde` + `utoipa::ToSchema`. Route path constants (`pub const SHOWS_LIST: &str = "/api/v1/shows";`). No HTTP logic, no DB logic. Compiled into both the server (for utoipa) and the client.
- **`kino-client`** — wraps `reqwest::Client` with a method per endpoint (`client.shows().list().await`). Depends on `kino-api`. Handles auth headers, base URL, error mapping. No CLI concepts leak in.
- **`kino-cli`** — the argument parser, output formatters, prompts, progress UI, config, keychain, plugin dispatch. Thin — most work delegates into `kino-client`.

### Why a shared types crate, not OpenAPI codegen

The CLI has to stay in sync with the server forever. Two paths considered:

- **OpenAPI codegen** (Progenitor, openapi-generator, etc.). Adds a build step, produces opaque generated code, doesn't round-trip Rust-specific invariants well. Right choice when consuming a *foreign* API; wrong when you control both sides in Rust.
- **Shared types crate.** Both server and CLI compile against the same `kino-api` crate. If the server changes a response shape, the CLI literally won't compile until it's updated. Compile-time guarantees beat any codegen, and jump-to-definition still works.

The existing `openapi.json` stays a verification artifact: CI runs the server in-memory, diffs its generated spec against the checked-in file, and fails on drift. The TypeScript SDK continues regenerating from that spec (the TS side can't share a Rust crate).

### Client binary, not a subcommand of the server

`kino-cli` is its own executable, not `kino cli` as a subcommand of the server. Two reasons:

- **Startup cost** — the server binary links sqlx, librqbit, axum, ffmpeg; even with lazy init, `kino show ls` would feel sluggish. A standalone CLI binary cold-starts in ~5 ms.
- **Deployment shape** — users running the server as a system service rarely want the CLI on the same path. Two binaries lets the CLI be installed per-user, per-machine, anywhere — including on laptops that have never run the server.

Both binaries ship in the same release archive; version strings are locked together by the workspace version.

## 1. Argument parsing and help

Uses **`clap` v4 derive API**. The dominant choice for large subcommand CLIs in Rust (cargo, rustup, uv, bottom, zellij all use it). Ergonomic derives, rich help generation, free autocompletion, mature.

**Global flags** available on every subcommand:

| Flag | Env | Default | Purpose |
|---|---|---|---|
| `--profile NAME` | `KINO_PROFILE` | `default` | Which config profile to use |
| `--server URL` | `KINO_URL` | from profile | Override server URL for this invocation |
| `--api-key KEY` | `KINO_API_KEY` | from keychain | Override API key for this invocation |
| `--output FORMAT` | — | `auto` | `auto`, `human`, `json`, `jsonl`, `yaml`, `tsv` |
| `--jq EXPR` | — | — | Filter JSON output with embedded jaq |
| `--color MODE` | `NO_COLOR`, `CLICOLOR` | `auto` | `auto`, `always`, `never` |
| `--quiet` / `-q` | — | — | Suppress progress + human-readable chatter |
| `--verbose` / `-v` | — | — | Repeatable; raises log level |
| `--watch` | — | — | Supported subcommands only; repaint every 1s |
| `--yes` | — | — | Skip confirmation prompts for destructive ops |

Precedence: CLI flag > env var > profile config > default. Same rule as `fly`, `aws`, `1password`.

**Short help** (`--help`) gives a one-screen overview. **Long help** (`--help` after a subcommand) gives examples. Every subcommand's long help includes at least two example invocations. `kino help exit-codes` renders the exit code table.

## 2. Output model

Every command produces structured data internally, then formats at render time. The single biggest UX win of modern CLIs is "table when you're a human, JSON when you're a script" — this is non-negotiable.

### Formatters

| Format | When | Details |
|---|---|---|
| `human` | stdout is TTY, or `--output human` | Tables via `comfy-table`, title-case headers, right-aligned numbers, subtle colour. Falls back to plain rows when piped. |
| `json` | stdout is not a TTY, or `--output json` | Pretty single JSON value (object or array). |
| `jsonl` | `--output jsonl` | One JSON object per line. Streams for long lists. |
| `yaml` | `--output yaml` | Humans editing config, mostly. |
| `tsv` | `--output tsv` | Tab-separated. Stable, documented column order per command. For `awk` / `cut`. |

### Colour and detection

Uses `anstream` + `anstyle` (what clap already uses). Honours `NO_COLOR`, `CLICOLOR`, `--color`. Checks `isatty()` on stdout and stderr independently — progress goes to stderr even when JSON output is flowing on stdout.

### Progress

`indicatif` bars on stderr, only when:
- stderr is a TTY
- the operation takes > 500 ms
- `--quiet` is not set

Never on stdout. Destroys pipelines.

### Pipe discipline

- SIGPIPE handled cleanly — `kino library ls | head` exits without an error.
- Line-buffered stdout for list commands so `| grep` works incrementally.
- `--null` / `-0` flag on commands that emit filenames → null-separated output for `xargs -0`.

### Exit codes

Based on BSD `sysexits` conventions, documented in `kino help exit-codes`:

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Generic runtime error |
| 2 | Usage error (clap emits this) |
| 3 | Not found (no such show / movie / download / file) |
| 4 | Authentication failed |
| 5 | Server unreachable |
| 64 | Bad input data |
| 75 | Transient error — caller may retry |
| 130 | Interrupted (Ctrl-C) |

## 3. Interactive vs non-interactive

Dual-mode by stdin detection:

- **stdin is a TTY, required arg missing** → prompt interactively (`dialoguer`). Example: `kino show add` with no title → TMDB search picker.
- **stdin is piped** → consume stdin line-oriented. Example: `cat ids.txt | kino show add -`.
- **stdin is not a TTY and no data piped, required arg missing** → exit 2 with usage. No hangs, no guessing.

`--yes` short-circuits any confirmation prompt. All destructive operations (`kino show rm --purge`, `kino library cleanup`) prompt by default when interactive, fail-without-flag when non-interactive.

## 4. Command surface

Grouped by noun. One verb per noun where possible.

### Library discovery + tracking

| Command | Summary |
|---|---|
| `kino search <query> [--kind show\|movie] [--year N] [--indexer NAME]` | Metasearch across TMDB (metadata) + indexers (availability). Human: table of hits. JSON: array. |
| `kino show add <id\|title> [--quality 1080p] [-]` | Add show to library. `-` reads newline-separated IDs from stdin. |
| `kino show ls [--status monitored\|ended] [--json]` | List tracked shows. Columns: `id title seasons next_air status`. |
| `kino show rm <id>... [--purge]` | Untrack. `--purge` deletes files too. |
| `kino show show <id>` | Full detail: seasons, episodes, file status. |
| `kino movie add / ls / rm / show` | Movie equivalents, identical flags. |

### Downloads

| Command | Summary |
|---|---|
| `kino download ls [--status active\|queued\|failed] [--watch]` | List downloads. `--watch` repaints with indicatif progress. |
| `kino download retry <id>` | Restart a failed download, possibly with a different release. |
| `kino download cancel <id>` | Cancel and blocklist the release. |
| `kino download logs <id> [-f]` | Per-torrent logs from librqbit via SSE. `-f` follows. |
| `kino download blocklist <id>` | Mark a release bad so it's never re-tried. |

### Library operations

| Command | Summary |
|---|---|
| `kino library ls [path] [--tree] [--null]` | Walk library. Default columns `title path size`. `-0` for xargs. |
| `kino library scan [path]` | Trigger rescan; streams progress on stderr, summary on stdout. |
| `kino library cleanup [--dry-run] [--yes]` | Orphans + leftovers. Dry-run default; requires `--yes` to actually delete. |
| `kino library stats` | Total size, item counts, by-codec breakdown, disk free. |

### Metadata

| Command | Summary |
|---|---|
| `kino refresh [id...]` | Re-pull TMDB metadata. No args = everything due. |
| `kino match <id> --tmdb <tmdb-id>` | Manually correct a mismatched item. |

### Playback

| Command | Summary |
|---|---|
| `kino play <title\|id> [--player mpv\|vlc\|url] [--season N --episode M]` | Resolve best playable source and exec the user's player. `--player url` just prints the stream URL. Default: `mpv` if on `PATH`, else print URL. |
| `kino history [--days 30]` | Watch history for the current user. |

### Server state

| Command | Summary |
|---|---|
| `kino status [--watch]` | One-screen dashboard: active downloads, health, disk free. |
| `kino health` | Machine-readable `/health`. Exits 0 healthy, 5 unreachable, 1 degraded. Cron-friendly. |
| `kino logs [-f] [--since 10m] [--level warn]` | Tail server logs via SSE endpoint. |
| `kino version [--json]` | Server + CLI version, commit, build date. |
| `kino doctor` | Runs a battery of checks (server reachable, API key valid, ffmpeg present, disks writable, indexers responding). Emits a report. |

### Indexers

| Command | Summary |
|---|---|
| `kino indexer ls` | Configured indexers, last seen status. |
| `kino indexer add <name> --url ... --apikey ...` | Add. |
| `kino indexer test <name>` | Run a canary query. |
| `kino indexer rm <name>` | Remove. |

### Auth + config

| Command | Summary |
|---|---|
| `kino login [--profile NAME]` | Guided setup: URL + API key → keychain. |
| `kino logout [--profile NAME]` | Forget credentials for a profile. |
| `kino whoami` | Active profile, server URL, API key fingerprint. |
| `kino config get <key>` | Read a config value. |
| `kino config set <key> <value>` | Write a config value. |
| `kino config list [--profile NAME]` | Dump profile. |

### Escape hatch

| Command | Summary |
|---|---|
| `kino api <METHOD> <PATH> [-f key=val]... [--input file.json] [--jq EXPR]` | Raw API call. Inherits auth. Flag shape copied from `gh api`. |

### Discoverability

| Command | Summary |
|---|---|
| `kino completions <shell>` | Emit completion script (bash, zsh, fish, nu, elvish). |
| `kino help exit-codes` | Exit code reference. |
| `kino help examples` | Worked examples for common workflows. |

Additional short-form aliases: `kino ls` → `kino show ls` if unambiguous in context, no — explicit is better. Resist aliasing beyond `-k` for `--kind` and similar.

## 5. Config and profiles

TOML at `$XDG_CONFIG_HOME/kino/config.toml` (falls back to `~/.config/kino/config.toml`, and `%APPDATA%\kino\config.toml` on Windows).

```toml
default_profile = "home"

[profiles.home]
url = "http://localhost:8080"
# api_key lives in OS keychain under service "kino", account "home"

[profiles.media-box]
url = "https://kino.lan"
tls_insecure = false

[profiles.friend]
url = "https://kino.friend.example"
```

Profile-aware commands: `kino --profile friend show ls`. Every command inherits profile resolution.

### Secret storage

API keys go in the OS keychain via the `keyring` crate: macOS Keychain, Windows Credential Manager, libsecret on Linux. Config holds only a reference (service name + account) — never the key itself.

Fallback for headless Linux with no secret service: a `.netrc`-style file at `$XDG_CONFIG_HOME/kino/credentials` with `chmod 600`, with a stderr warning on first use.

### `kino login`

1. Prompt for URL (default `http://localhost:8080`).
2. `GET /health` on the URL — if unreachable, fail early with a clear message.
3. Prompt for API key (masked) or accept `--key-stdin` for piped secrets.
4. Verify: `GET /api/v1/whoami` or equivalent.
5. Store in keychain, update TOML.

No OAuth / device-code flow. Kino uses static API keys; browser-based pairing is spec creep and kino is self-hosted.

## 6. Plugin system

CLI plugins are out-of-tree executables that extend the command surface without needing to land code in the main repo. Shape and semantics copied from `gh extension` — a pattern that's had several years to prove itself.

### Discovery

On startup, `kino-cli` scans `$XDG_DATA_HOME/kino/extensions/` (and the Windows equivalent). Any executable named `kino-cli-<name>` is registered as a top-level subcommand `<name>`. If the user runs `kino foo bar baz` and `foo` isn't a built-in subcommand, the CLI execs `kino-cli-foo bar baz` with inherited env.

Plugins may be written in any language — they're just executables. Python, bash, Go, Rust, whatever.

### Environment passed to plugins

| Var | Purpose |
|---|---|
| `KINO_URL` | Active server URL (already-resolved, profile-aware) |
| `KINO_API_KEY` | Active API key, extracted from keychain |
| `KINO_PROFILE` | Active profile name |
| `KINO_OUTPUT` | `human` / `json` / etc., so plugins can honour format requests |
| `KINO_COLOR` | `always` / `never` |
| `KINO_VERSION` | CLI version, so plugins can refuse on mismatches |

### Install / manage

| Command | Summary |
|---|---|
| `kino extension install <git-url\|path>` | Clone/copy a plugin into the extensions dir. Warns that plugins run with your API key. |
| `kino extension ls` | List installed plugins + their sources. |
| `kino extension upgrade [name]` | Pull latest from the source. |
| `kino extension remove <name>` | Uninstall. |
| `kino extension create <name>` | Scaffold a plugin repo (shell or Rust template). |

### Security posture

Plugins are arbitrary code with full API-key access. Two mitigations:

- **No registry, no "marketplace"** — users install from a URL they chose. Same trust model as `git clone && make install`.
- **First-run warning** — on `extension install`, show the source URL and require `--yes` or interactive confirmation.

We don't sandbox plugins. Users on a homelab running their own scripts don't need that; users who want it can wrap plugin invocations in their own containment.

### Why we want this

Media libraries are intensely personal; everyone has a weird workflow. A plugin layer lets the community build the long-tail features without us having to triage every niche request. Examples that would make sense as plugins:
- `kino-cli-import-from-existing` — one-shot importer from an existing media-server library
- `kino-cli-stats` — custom charts from watch history
- `kino-cli-discord` — post new-episode announcements to a Discord webhook
- `kino-cli-cold-storage` — shuffle older titles to a secondary drive

## 7. Distribution

Built with `cargo-dist` from the same workspace. GitHub Actions workflow produces:

- `x86_64` + `aarch64` for Linux, macOS, Windows
- `.tar.xz` and `.zip` release archives (both binaries in each)
- Homebrew formula pushed to a dedicated `homebrew-tap` repo
- `curl | sh` installer script
- `.deb` packages
- Windows `.msi` installer

Plus `cargo install kino-cli` from crates.io for Rust users who want to build from source.

Auto-update is explicitly excluded — no background check, no phone-home. Package managers (Homebrew, apt, cargo) do upgrades; users who grabbed the shell installer rerun it to upgrade, or use the opt-in `kino self-update` behind a disabled-by-default cargo feature.

## Entities touched

- **Reads (via the Kino API):** every endpoint — the CLI is a client, not a privileged insider
- **Writes:** none in the database directly. All writes go through the API
- **Local filesystem:**
  - `$XDG_CONFIG_HOME/kino/config.toml` — profile config
  - `$XDG_CONFIG_HOME/kino/credentials` — fallback secret store (headless Linux only)
  - `$XDG_DATA_HOME/kino/extensions/` — installed plugins
- **OS keychain:** API key storage (service = `kino`, account = profile name)
- **Environment:** reads `KINO_URL`, `KINO_API_KEY`, `KINO_PROFILE`, `NO_COLOR`, `CLICOLOR`, `XDG_*`

## Dependencies

| Crate | Purpose |
|---|---|
| `clap` (v4, derive) | Argument parsing |
| `clap_complete`, `clap_complete_nushell` | Shell completion generation |
| `clap_mangen` | Man page generation |
| `reqwest` | HTTP client (shared with server) |
| `serde`, `serde_json` | Serialisation |
| `anstream`, `anstyle` | Colour-aware output |
| `comfy-table` | Table rendering |
| `indicatif` | Progress bars |
| `dialoguer` | Interactive prompts |
| `keyring` | OS keychain integration |
| `color-eyre`, `thiserror` | Error presentation |
| `jaq-core`, `jaq-std` | Embedded jq implementation for `--jq` |
| `directories` | XDG / platform config paths |
| `insta` (dev) | Snapshot tests for output stability |

No new system binaries. `mpv` / `vlc` are invoked if present; their absence degrades `kino play` gracefully (prints URL instead).

## Error states

- **Server unreachable** — exit 5, clear message with the URL tried and the underlying error.
- **API key invalid** — exit 4, suggest `kino login`.
- **Command argument mismatch** — clap emits usage + exits 2.
- **Structured API error** (4xx/5xx with problem-details body) — surface the server's `detail` field; exit 1.
- **SIGPIPE on piped commands** — exit cleanly, no stack trace.
- **Ctrl-C mid-operation** — cancel pending request, exit 130.
- **Keychain locked / unavailable** — fall back to plaintext credentials file with a stderr warning.
- **Plugin binary missing but referenced** — report the plugin name and its expected path; exit 1.
- **Plugin exits non-zero** — propagate its exit code.
- **JSON output requested but command doesn't produce structured data** (e.g., `kino logs -f`) — still works, emits one JSON object per log line.
- **`--jq` filter fails to compile** — exit 2 with jaq's error message.

## Known limitations

- **OpenAPI drift still possible in extreme cases** — shared types prevent *shape* drift, but semantic drift (a field's meaning changing without its type changing) can still sneak through. Testing practice, not a code guarantee.
- **Keychain fallback file is plaintext** on headless Linux without libsecret — documented, warned on use, no better option.
- **Plugins run with full API-key access** — no sandboxing. Same trust model as installing any other CLI tool from a URL.
- **`kino play` defaults to `mpv`** — users who prefer other players must set `--player` each invocation or configure `play.player` in config. We deliberately don't ship a player.
- **No shell alias configuration** — if the user wants `k` instead of `kino`, they make their own alias. We don't install one.
- **Some colour terminals (Windows legacy cmd.exe) render ANSI poorly** — `anstream` handles modern Windows Terminal well; legacy terminals fall back to plain.
- **`--watch` is a naive repaint, not a curses TUI** — keeps pipelines composable at the cost of a little flicker on slow terminals.
- **No live streaming of indicatif progress when output is JSON** — progress goes to stderr regardless, but `--output json --watch` emits one JSON object per tick rather than a continuously-updating bar.
