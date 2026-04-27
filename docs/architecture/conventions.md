# Conventions

The disciplines this codebase enforces beyond `cargo clippy --deny
warnings`. Each one closes a bug class that's structural rather
than syntactic — the kind clippy can't see and a code reviewer can.

## SQL timestamp comparisons wrap both sides in `datetime()`

```sql
-- BAD
WHERE last_searched_at < ?

-- GOOD
WHERE datetime(last_searched_at) < datetime(?)
```

Plain text comparison works only when both sides happen to be
identical RFC3339 forms. The moment one side carries sub-seconds,
omits the `Z`, or uses local time, lexicographic ordering diverges
from chronological. `datetime()` parses both sides as datetimes and
compares those.

Enforced by
`conventions::sql_timestamp_compare::timestamp_comparisons_use_datetime_function`
— a CI test that walks `src/`, finds known timestamp columns next
to ordered comparison operators, and fails if `datetime(` isn't on
the same line. Adding a new TEXT timestamp column = add it to
`TIMESTAMP_COLUMNS`.

In Rust code, prefer the [`Timestamp`](../../backend/crates/kino/src/time.rs)
newtype + `Timestamp::now_minus(Duration::hours(N))` for the cutoff
side; bind the resulting value directly via the sqlx Encode impl.

## Wall-clock timestamps go through `Timestamp::now()`

```rust
// BAD
let now = chrono::Utc::now().to_rfc3339();

// GOOD
let now = crate::time::Timestamp::now().to_rfc3339();
// or, when binding:
.bind(crate::time::Timestamp::now())
```

`Timestamp::now()` is the canonical "what is the current wall-clock
moment" call. It's interchangeable with `chrono::Utc::now()` at the
chrono level (it's a thin newtype), but using it consistently means:

- the `now_minus` / `now_plus` arithmetic helpers are the obvious
  next step at any cutoff site
- bind sites can use `.bind(Timestamp::now())` directly — the sqlx
  Encode impl writes RFC3339 without a temp `String`
- `Timestamp::parse` tolerates both RFC3339 and SQLite-native
  `YYYY-MM-DD HH:MM:SS`, so a column written by raw `datetime('now')`
  SQL still round-trips cleanly through Rust
- the codebase has one place to change if we ever swap chrono out

Test code can keep using `chrono::Utc::now()` directly when test
fixtures don't care about the typed origin.

## Operations follow the validate-execute-verify-emit shape

See [`operations.md`](./operations.md). The shape is enforced by
review, not lint — but the trait surfaces support it: `ReleaseTarget`
methods take `&mut Transaction` for writes that must commit
atomically, `AcquisitionPolicy::evaluate` is pure and synchronous so
preconditions check cheaply, `CleanupTracker::try_remove` wraps the
external side-effect after commit.

If you find yourself writing `// TODO: race condition here`, you're
fighting the operation shape. Re-read the doc, then refactor.

## Errors propagate; never swallow

```rust
// BAD: silent failure
client.remove(&hash).await.ok();
let _ = some_fallible_call().await;

// GOOD: tracker-mediated retry
let outcome = tracker.try_remove(ResourceKind::Torrent, &hash, || async {
    client.remove(&hash).await
}).await?;
```

Two acceptable cases for `let _ = ...`:

1. **Broadcast send.** `let _ = event_tx.send(event)` is fine — a
   broadcast channel with zero subscribers returns `Err` and
   that's the documented "no-op when nobody's listening" semantic.
2. **Intentional discard with comment.** Rare; explain *why* the
   result is discardable.

Everything else routes through `?` (operation errors) or
`CleanupTracker::try_remove` (resource removals that must succeed
eventually).

This isn't lint-enforced today — the existing `let _ = ...` sites
are dominated by case 1 (event emits), and a `let_underscore_must_use`
warning would noise out the meaningful violations. Reviewer
discipline catches new cases.

## State machine enums own their classification

```rust
// BAD: inline `matches!` chain at every call site
if matches!(phase, DownloadPhase::Downloading | DownloadPhase::Stalled
                 | DownloadPhase::Completed | ...)

// GOOD: classification on the enum
if phase.is_runtime_monitored()
```

Adding a new `DownloadPhase` variant must force the compiler to
surface every inexhaustive match. Adding a new classification
predicate (e.g. `is_paused_recoverable`) goes on the enum once, not
inline at every site.

`DownloadPhase`, `WatchNowPhase`, `TranscodeSessionState`,
`ScrobbleState`, `ResourceKind`, `ReleaseTargetKind`, `RejectReason`,
`StandardInvariant`, `ReconcileStep`, `StepRepairPolicy` all follow
this shape. New state-shaped enums should too.

## Code is authoritative, not narrative

Module docstrings describe what a thing IS, not how it differs from
what came before. Refactor metadata, "we used to do X", "Phase B
will" — none of those belong in code. They belong in commit
messages.

The bar: a reader who has never seen the previous version of the
file should not need to know there was one to understand the
current behaviour.

## Migrations stay merged into the initial schema (during pre-release)

Until the first tagged release, all schema changes go into
`migrations/20260328000001_initial_schema.sql`. There is no
migration ladder; `just reset` is the recovery path during dev.

Post-release, this flips: every schema change becomes an additive
migration.

## Adding a convention

1. State the rule + its rationale in this file.
2. Add a CI test that mechanically catches the violation, if
   feasible. If not (e.g. operation shape), state that and leave
   it to review.
3. Cross-link from any module whose docstring would otherwise
   re-explain the rule.
