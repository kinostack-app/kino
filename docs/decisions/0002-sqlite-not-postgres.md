# ADR-0002: SQLite, not Postgres (or anything else)

**Status:** accepted
**Date:** 2026-04-25 (recorded retroactively)

## Context

Persistent storage for kino covers: entity metadata (movies,
shows, episodes, media), lifecycle state (downloads, releases,
blocklists), user preferences, sessions, scheduler state, history,
and integration state (Trakt auth, list sync). Workload is
heavily-read, occasionally-written, single-process,
single-machine.

## Decision

**SQLite in WAL mode**, single `.db` file in the data directory.
All persistence goes through it.

## Alternatives considered

- **Postgres.** Standard pick for "real" web apps. Adds an entire
  service to run, configure, back up, and version-bump.
  Justified for multi-user / multi-process / horizontal-scaling.
  Justified for none of those for kino.
- **Embedded key-value (sled, redb).** Simpler than SQLite for
  point reads, but the codebase is full of joins, range queries,
  ordering, aggregations. SQL is the right shape.
- **JSON files on disk.** The hobby-project default. Doesn't
  survive concurrent writes, doesn't index, doesn't support the
  query shapes we need. Painful by year two.

## Consequences

- **Win:** zero ops cost. Backup = copy a file. Restore = copy
  back. Inspect via `sqlite3` CLI.
- **Win:** transactions are local + fast. ACID for free.
- **Win:** WAL mode handles our concurrency (one writer, many
  readers) without configuration.
- **Win:** single-file DB plays well with the single-binary story.
- **Cost:** no horizontal scaling. Already a non-goal (ADR-0001).
- **Cost:** schema migrations need discipline (sqlx migrations).
  Mitigated: pre-launch we rewrite the initial schema rather than
  accumulate migrations; post-launch we use forward-only
  migrations.
- **Cost:** no streaming subscribe / change-data-capture. We
  emit `AppEvent` from the application layer instead. Adequate.

## Supersedes

None.
