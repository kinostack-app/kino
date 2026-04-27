# Logging conventions

Short enough to re-read before you add a log line. The goal is that
a user hitting `/settings/logs` during an incident can understand
what happened without reading source.

## The policy

Every log event falls into one of five buckets. Pick the right level
or the viewer becomes noise.

| Level | Meaning | Examples |
|---|---|---|
| **ERROR** | Something the user needs to know or act on. An operation failed with a user-visible consequence. | `import failed`, `all indexers blocked`, `VPN handshake expired`, `ffmpeg crashed`, `database constraint violated` |
| **WARN** | Degraded path, retry happening, fallback used. One request/indexer/webhook failed but the system kept going. | `CF-blocked on 1337x (will retry)`, `TMDB 503 — retrying`, `hardlink failed, copying instead`, `webhook returned 500`, `lost subscriber due to lag` |
| **INFO** | A decision or state transition worth seeing in the event log. Not per-tick, not per-packet. | `download grabbed`, `import completed`, `session started`, `config updated`, `scheduler task started/finished`, `indexer re-enabled` |
| **DEBUG** | Implementation detail useful when debugging a specific class of issue. Off by default for most subsystems. | `polling tick`, `intermediate extraction result`, `per-segment transcode progress`, `wanted eligibility SQL` |
| **TRACE** | Fine-grained — every HTTP request, every packet, every lock acquisition. Essentially off in production. | `tunn encapsulate`, `peer connection`, `span created` |

Rules of thumb:
- If the user should see it in the UI, it's **INFO** or higher.
- If it repeats more than ~once per second per entity, it probably isn't **INFO**.
- If it's a failure you handled locally, it's **WARN**, not ERROR.
- If the operation "kept working but something's degraded," it's **WARN**.
- **Never** use `eprintln!` or `println!` outside tests — it bypasses the pipeline.

## The silent-error rule

Never write `let _ = some_operation_that_returns_Result`. Either:

```rust
// Log it:
if let Err(e) = tx.send(event) {
    tracing::warn!(error = %e, "event dispatch failed");
}

// Or document why it's safe to ignore:
// Broadcast send fails only when there are no subscribers; harmless.
let _ = state.event_tx.send(event);
```

Same for `.ok()` / `.ok().flatten()` on `Result`. An error you chose
to swallow is still a decision worth marking.

## Structured fields

Messages are for humans; **fields** are for filters. Stay consistent:

| Entity | Field name | Type |
|---|---|---|
| Movie | `movie_id`, `tmdb_id`, `title` | i64, i64, str |
| Show | `show_id`, `tmdb_id`, `title` | i64, i64, str |
| Episode | `episode_id`, `season`, `episode` | i64, i64, i64 |
| Download | `download_id`, `torrent_hash` | i64, str |
| Release | `release_id`, `indexer` | i64, str |
| Indexer | `indexer`, `indexer_id` | str, i64 |
| Request | `trace_id`, `method`, `path` | str, str, str |
| File | `path` | str (redact any secret components) |
| Error | `error` | `%e` (Display) or `?e` (Debug) |
| Duration | `duration_ms` | u64 |

Every log line about a domain entity **must** carry that entity's id.
`tracing::info!("grabbed")` is useless; `tracing::info!(download_id, movie_id, "grabbed")` is filterable.

## Spans for operations

Wrap multi-step operations in a `tracing::info_span!`. Every child
log inherits the span's fields, so the log viewer's "filter by X"
click shows the whole chain.

```rust
pub async fn import_download(pool: &SqlitePool, event_tx: …, download_id: i64) {
    let _span = tracing::info_span!("import", download_id).entered();
    // … everything inside carries download_id automatically …
    tracing::info!(path = %source.display(), "found media file");
    tracing::info!(media_id, "import completed");
}
```

Operations worth wrapping: `search_movie`, `grab_release`,
`start_download`, `import_download`, `health_sweep`, `refresh_movie`,
`refresh_show`, `refresh_season`, `hls_master`, `webhook deliver`,
`reconcile_downloads`, `reconcile_entities`.

Don't span trivial getters or helpers. The span itself is an event —
it costs a few bytes and shows up in the log viewer as "span_id" for
that record.

## Duration logging

For anything that touches a slow external (HTTP, FFmpeg, disk), log
the duration on completion:

```rust
let start = std::time::Instant::now();
let result = tmdb.movie_details(id).await;
tracing::info!(
    tmdb_id = id,
    duration_ms = start.elapsed().as_millis() as u64,
    ok = result.is_ok(),
    "tmdb movie_details",
);
```

For the scheduler, every task entry already logs start/finish — we
should add `duration_ms` to the finish event.

## Redaction

The `observability::redact` module catches the common leaks (magnet
passkeys, bearer tokens, `api_key=…`, WG base64 keys, JSON passwords)
in the log writer. Don't rely on it — **don't log secrets in the
first place**:

- Credential types should be `secrecy::SecretString` (or similar);
  manual `Debug` impl that prints `[REDACTED]`.
- If you must log a URL with credentials embedded, redact at the
  format site: `url.replace(token, "[REDACTED]")`.
- Don't log full HTTP request/response bodies. Log `content_length`
  and status instead.

## What NOT to log

- Per-tick polling at INFO (use DEBUG).
- Successful cache hits at INFO (use DEBUG).
- Every HTTP request at INFO (the trace-id middleware already opens
  a span; handlers add their own INFO events at decision points).
- Config dump at startup — secrets leak even with `SecretString`
  wrappers when the surrounding struct auto-derives `Debug`.
- PII like user-watched timestamps beyond what the `history` table
  already stores.

## Per-OS log destinations

The SQLite `log_entry` table is the **single operator-facing source**
on every platform — surfaced at `/settings/logs` and via the
`/api/v1/logs/export` endpoint. The OS-level destination (journald,
launchd file, Event Log) is an additional rendering for sysadmins who
prefer their native tooling (`journalctl`, `log show`, `Get-WinEvent`).

| OS | Mode | Where stderr goes | Operator command for OS-level view |
|---|---|---|---|
| Linux | systemd service | journald | `journalctl -u kino` |
| Linux | systemd `--user` | journald (user instance) | `journalctl --user-unit kino` |
| Linux | tarball / foreground | terminal | tail terminal |
| macOS | LaunchDaemon | `<key>StandardErrorPath</key>` in plist (when implemented — task #523) | `tail -f <plist-configured path>` |
| macOS | foreground | terminal | tail terminal |
| Windows | Service | (lost unless we wire Windows Event Log — deferred) | SQLite layer in `/settings/logs` |
| Windows | foreground | console | tail console |
| Docker / OCI | container | stdout / stderr (Docker captures) | `docker logs kino` |

**JOURNAL_STREAM detection.** systemd sets the `JOURNAL_STREAM` env
var when stderr is wired to journald (man `systemd.exec`). When
present, `setup_tracing` in `backend/crates/kino/src/main.rs` switches
the stderr layer to `without_time().with_ansi(false)` — journald adds
its own timestamp and `journalctl` colourises on the read side, so
otherwise we'd double-print both. Falls back to the human-friendly
default in a terminal.

**File-based rolling logs in foreground/user-mode** are not currently
implemented. Foreground users see logs on stderr; if they want
persistence they re-run under `script(1)` or pipe to a file. The
SQLite log table covers the operator case. A `tracing-appender`
rolling-file layer is on the roadmap for user-mode runs without a
service supervisor (helix and lapce both ship something equivalent).

## Quick reference

```rust
// Short message + fields. Use %e for Display, ?e for Debug.
tracing::info!(movie_id, title = %title, "release grabbed");

// Suppressed error — log it:
if let Err(e) = client.remove(hash, false).await {
    tracing::warn!(download_id, error = %e, "failed to stop seeding");
}

// Span wrapping an operation:
async fn search_movie(state: &AppState, movie_id: i64) -> Result<()> {
    let _span = tracing::info_span!("search_movie", movie_id).entered();
    // …
}

// Duration:
let t = std::time::Instant::now();
let result = do_work().await;
tracing::info!(duration_ms = t.elapsed().as_millis() as u64, "work done");
```

## Checklist when adding a new subsystem

1. Every error path logs (WARN or ERROR) with the entity id as a field.
2. Every decision (grab / skip / retry) logs at INFO with the reason.
3. The top-level async function opens a span with the primary entity id.
4. External-service calls log duration on both success and failure.
5. No `let _` on `Result` without a comment.

## Per-subsystem matrix

What a developer running with `RUST_LOG=debug` should see from each
subsystem. This is the target — when adding to a subsystem, match it.

### Search / indexers

- **ERROR** — all indexers blocked; search failed with 0 results and 0 responses.
- **WARN** — site returned 4xx/5xx; cardigann CF-challenge; torznab parse error; filter blocked request; per-site timeout.
- **INFO** — `search_movie`/`search_show` started + finished with counts and total duration; indexer re-enabled after health recovery.
- **DEBUG** — per-site HTTP method + URL + status + `content_length`; cardigann selector hits/misses (field, selector, captured value); torznab field extraction; each filter's input/output; release scoring result (title, score, size).

### Download

- **ERROR** — librqbit add-torrent failed; start_download rolled back; torrent stalled past threshold.
- **WARN** — VPN unhealthy at grab time; retry scheduled with attempt count.
- **INFO** — `start_download` grabbed (`torrent_hash`, `size_bytes`, `indexer`); state transitions (queued → downloading → completed); cancellation.
- **DEBUG** — librqbit stats tick (progress %, peers, dl/ul rate) gated to every N seconds per torrent; piece-count milestones; tracker responses.

### Import

- **ERROR** — source path missing; atomic rename failed after retry; unsupported extension when strict.
- **WARN** — hardlink → copy fallback; parse confidence low; trashed source on failure.
- **INFO** — `import_download` entered; decision summary (`hardlink|copy|move`, `src`, `dest`); import completed (`media_id`, `duration_ms`).
- **DEBUG** — parser output (title, year, season, episode, quality, release_group); destination templating inputs; disk-free check; atomic rename tmp path.

### Metadata (TMDB)

- **ERROR** — TMDB unreachable for >N retries; missing API key.
- **WARN** — TMDB 4xx (not found excluded) / 5xx with endpoint.
- **INFO** — `refresh_movie/show/season` entered; cache status summary at end; poster/backdrop chosen.
- **DEBUG** — endpoint + status + cache-hit/miss + `duration_ms`; artwork ranking inputs (vote_avg, lang, aspect).

### Transcode

- **ERROR** — ffmpeg exit != 0 with tail of stderr.
- **WARN** — probe unexpected; stream copy fell back to re-encode; session evicted.
- **INFO** — session started (`session_id`, `media_id`, `codec`, `bitrate`); session stopped with `duration_ms`.
- **DEBUG** — ffmpeg argv; ffprobe JSON summary; per-segment cadence (every N segments); HLS playlist refresh.

### Scheduler

- **ERROR** — task panic caught.
- **WARN** — task skipped because previous still running; tick drift > threshold.
- **INFO** — task started/finished with `task`, `duration_ms`.
- **DEBUG** — tick reason; per-task next-run-in; skip decisions with reason.

### Webhooks

- **ERROR** — destination unreachable after max retries.
- **WARN** — 4xx/5xx response; retry attempt `n` of `max`.
- **INFO** — payload dispatched with `webhook_id`, `event_type`, `http_status`, `duration_ms`.
- **DEBUG** — payload size; template rendering result; backoff delay before retry.

### VPN (wireguard + port forward)

- **ERROR** — handshake expired; tunnel down.
- **WARN** — port-forward renewal got new port (note new value); packet loss window.
- **INFO** — tunnel up (`endpoint`, `public_ip`); port forward established (`port`); handshake cadence summary.
- **DEBUG** — wireguard handshake attempt; keepalive tick; PCP/NAT-PMP request/response; interface stats.

### HTTP API

- Per-request trace-id span already covers this. Handlers should add INFO for decisions (not every read), and DEBUG for query params they're filtering on.
