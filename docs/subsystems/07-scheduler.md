# Scheduler subsystem

Orchestrates all periodic background work. A simple internal task runner — no external job queue or cron dependency.

## Responsibilities

- Run periodic tasks at configurable intervals
- Track when each task last ran (survives restarts)
- Ensure only one instance of each task runs at a time
- Allow manual triggering of any task via API

## Tasks

| Task | Default interval | What it does |
|---|---|---|
| **Wanted search** | `auto_search_interval` from Config (default 15 min) | Search indexers for all content in `wanted` state, respecting retry backoff. Also checks for quality upgrades on existing content. |
| **Metadata refresh** | Fixed 30-min tick | SQL-level tiered cadence in `services::metadata::refresh_sweep`: hot rows (airing shows, movies within 60 days of release) re-fetch at 1h staleness, cold rows (ended/canceled shows, older movies) at 72h. One sweep per tick walks both tables. Detects new episodes/seasons. |
| **Cleanup** | 1 hour | Remove media files for watched content past their delay. Emits `ContentRemoved`. |
| **Disk-space check** | 5 minutes | `df` on `media_library_path` + `download_path`; emits `HealthWarning` / `HealthRecovered` on transitions against `low_disk_threshold_gb`. |
| **Orphan scan** | 1 week | Walk `media_library_path`, warn on video files with no matching `media` row. Doesn't auto-delete. |
| **Indexer health** | 30 minutes | Probe each indexer via `?t=caps`; escalate failures, refresh capabilities on success. |
| **Webhook retry** | 15 minutes | Check disabled webhook targets — if `disabled_until` has passed, re-enable them. |
| **VPN health** | 5 minutes | Verify tunnel handshake is fresh; reconnect if stale. |
| **Stale download check** | 1 second | Poll active downloads for progress + stall/dead transitions. |
| **Transcode cleanup** | 1 hour | Kill idle FFmpeg sessions + orphan transcode temp dirs. |
| **Trickplay generation** | 5 minutes (plus event-driven kick on each `Imported` / `Upgraded`) | Generate seek thumbnails for new media. |
| **Log retention** | 1 hour | Cap the `log_entry` table row count. |
| **Trakt sync incremental / home refresh / scrobble drain** | 5 min / 24h / 1 min | No-op when Trakt isn't connected. |
| **Lists poll** | Per-list interval | See subsystem 17. |
| **Intro catch-up** | 15 minutes | Pick up episodes that missed the per-import hook (subsystem 15). |

## Design

Each task is a function with a name and an interval. The scheduler maintains a simple table of last-run timestamps.

### Persistence

Last-run timestamps are stored in a `scheduler_state` table (or could be a simple key-value in the Config singleton — lightweight enough either way):

| task_name | last_run_at |
|---|---|
| wanted_search | 2026-03-28T14:30:00Z |
| metadata_refresh | 2026-03-28T06:00:00Z |
| cleanup | 2026-03-28T14:00:00Z |
| ... | ... |

On startup, the scheduler reads last-run timestamps and calculates when each task is next due. Tasks that were overdue during downtime run immediately.

### Execution

- Each task runs in its own async task, spawned on a
  `tokio_util::task::TaskTracker` shared with the outer process
  shutdown path — the 10 s graceful-shutdown window covers
  in-flight tasks instead of the runtime aborting them
  mid-DB-write.
- Only one instance of a given task runs at a time. The claim is
  atomic: `try_claim` flips `running = true` + writes `last_run_at`
  under the same write lock. A second tick that arrives before a
  long-running task finishes sees `running = true` and skips.
- Manual triggers (see below) go through `try_claim_manual`, which
  bypasses the interval check but still respects the running
  guard. A manual trigger for an already-running task bails with
  an error on its completion channel rather than racing.
- Tasks don't have dependencies on each other — they can run
  concurrently.
- Panics in a task body are caught via `AssertUnwindSafe +
  catch_unwind`, logged, surfaced via `HealthWarning`, and
  `mark_done` runs so the task can fire again next cycle.

### Intervals

- **`auto_search_interval`** is user-configurable in Settings →
  Automation and drives the `wanted_search` task.
- **Metadata refresh** runs on a fixed 30 min scheduler tick; the
  user-facing cadence knob lives inside
  `services::metadata::refresh_sweep` as per-row tiering
  (1h hot / 72h cold based on show status / movie release
  window). The old single-knob `metadata_refresh_interval`
  config column was dropped when tiering landed.
- Everything else is a fixed interval baked into
  `register_defaults`.

### Startup stagger

When many tasks are simultaneously overdue on boot (e.g. kino was
off for a day and `auto_search_interval` + `metadata_refresh` +
`cleanup` + `indexer_health` all qualify at `t=0`), the first
tick stagger-spreads them `0, 1, 2 … 10 s` apart so we don't hit
the DB / TMDB / indexers in a single burst. Scoped to a single
tick — steady-state ticks where one task happens to be due get
zero delay.

### Manual trigger

Every task can be triggered immediately via the API:

```
POST /api/tasks/{task_name}/run
```

This runs the task now regardless of when it last ran. The scheduler updates `last_run_at` and resets the interval timer. Returns immediately — the task runs in the background.

### Listing tasks

```
GET /api/tasks
```

Returns all tasks with their name, interval, last_run_at, next_run_at, and whether currently running.

## What the scheduler is NOT

- Not a generic job queue — there's no concept of enqueuing arbitrary work
- Not a cron system — intervals only, no cron expressions
- Not distributed — single instance, single process

It's deliberately minimal. Each task is a function call to the relevant subsystem. The scheduler just decides when to call it.

## Dependencies

- Config table (intervals)
- All other subsystems (the scheduler calls into them, not the other way around)
- Persistence for last-run timestamps

## Error states

- **Task fails** → log error, retry next cycle. No retry loop within a cycle.
- **Task takes longer than its interval** → skip the next run, log a warning. Don't pile up.
- **Startup after long downtime** → overdue tasks run immediately, but staggered (not all at once) to avoid a thundering herd on TMDB/indexers.
