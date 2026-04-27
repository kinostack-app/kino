# CleanupTracker

`CleanupTracker` is the persistent retry queue for resource removals
that must succeed but can transiently fail: torrents in librqbit,
files on disk, directories. It replaces the fire-and-forget pattern
where a failed `client.remove(hash)` vanished into a log line and
nothing retried.

## Contract

```rust
let outcome = tracker.try_remove(
    ResourceKind::Torrent,
    &info_hash,
    || async { client.remove(&info_hash).await },
).await?;
```

- On `Ok(())` from the removal closure: returns `Removed`. Any prior
  queue row for this `(kind, target)` is deleted (the resource is
  gone, regardless of whether someone cleaned it up out-of-band).
- On `Err(_)`: upserts into `cleanup_queue` and returns
  `Queued { attempts }` (or `Exhausted { attempts }` if
  `attempts >= max_attempts`).

The scheduler ticks `tracker.retry_failed(&executor)` on a fixed
cadence. The executor is a caller-provided dispatcher that knows how
to actually remove each kind:

```rust
struct AppExecutor { torrent: Arc<dyn TorrentSession> }
impl RemovalExecutor for AppExecutor {
    async fn execute(&self, kind: ResourceKind, target: &str) -> Result<(), String> {
        match kind {
            ResourceKind::Torrent  => self.torrent.remove(target).await.map_err(|e| e.to_string()),
            ResourceKind::File     => tokio::fs::remove_file(target).await.map_err(|e| e.to_string()),
            ResourceKind::Directory => tokio::fs::remove_dir_all(target).await.map_err(|e| e.to_string()),
        }
    }
}
```

Successes delete the row. Failures bump `attempts`. Rows reaching
`max_attempts` (default 5) stay in the queue for admin attention,
queryable via `pending_exhausted_count()`.

## Schema

```sql
CREATE TABLE cleanup_queue (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    resource_kind   TEXT    NOT NULL,
    target          TEXT    NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER NOT NULL DEFAULT 5,
    last_error      TEXT,
    last_attempt_at TEXT,
    created_at      TEXT    NOT NULL,
    UNIQUE(resource_kind, target)
);
```

The `UNIQUE(resource_kind, target)` constraint means a second
failure on the same target updates the existing row rather than
creating a duplicate. A target's lifecycle: enqueued at first
failure → updated on every subsequent failure → deleted on success
(or retained as exhausted).

## Resource kinds

| Variant | Target format | Removal API |
|---|---|---|
| `Torrent` | info_hash (lowercase hex) | `client.remove(&hash)` |
| `File` | absolute filesystem path | `tokio::fs::remove_file(p)` |
| `Directory` | absolute filesystem path | `tokio::fs::remove_dir_all(p)` (executor's choice) |

The set is deliberately small. Sidecars (`.srt`, `.vtt`) and HLS
segment dirs are special cases of `File` and `Directory`
respectively; if per-kind retry policies become useful, add
variants.

## Retry policy

- Default minimum interval between retries: 5 minutes
  (`DEFAULT_RETRY_INTERVAL`).
- Default `max_attempts`: 5 (≈ 25 minutes of retries before a row
  goes Exhausted).
- The interval check is a cutoff: rows with
  `last_attempt_at IS NULL OR datetime(last_attempt_at) <= datetime(now - interval)`
  are eligible. Freshly-queued rows wait one interval before their
  first retry.

Tests inject `with_retry_interval(pool, Duration::from_secs(0))` to
make every row immediately eligible without time travel.

## Where it gets called from

The migration plan moves the silent-swallow sites onto `try_remove`:

- Torrent removals on download cancel / cascade delete (movie /
  show / blocklist+search paths).
- Library file removals after the user removes content.
- Empty-directory sweeps after media file deletion.
- HLS segment directory cleanup on transcode session eviction.

`pending_count()` and `pending_exhausted_count()` feed the `/status`
health surface so admins see "12 cleanup retries pending, 1
exhausted" as an early signal.

## Cross-references

- [`operations.md`](./operations.md) — `try_remove` is the
  "external side-effect after commit" step in the operation
  pattern. The DB write that says "this resource is gone" must
  commit before the actual removal attempt; otherwise a crash
  between commit and removal leaves an orphan with no row to
  retry.
- [`invariants.md`](./invariants.md) — the
  `no_orphan_active_torrents` invariant uses `pending_count()`
  to distinguish "actually orphaned" from "scheduled for retry".
