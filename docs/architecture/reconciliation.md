# Reconciliation

Reconciliation is kino's continuous self-check loop. The scheduler
ticks `reconcile::run_continuous` every 60 seconds; each
[`ReconcileStep`] compares some piece of state against an expected
condition and either auto-repairs the drift or surfaces it for
admin attention.

The framework exists so drift gets caught within a minute, not at
the next user complaint. Tests verify code; reconciliation verifies
running systems.

## Step policies

Each step is classified at compile time:

- **`AutoRepair`** — the step's repair action is idempotent and
  safe to run unattended. Hash normalisation, orphan-row deletion,
  stuck-claim resets all qualify.
- **`SurfaceOnly`** — the step detects drift but never modifies
  state. Operator confirmation required. Used when the corrective
  action could destroy user data (deleting a media row whose file
  "appears missing" when in fact a mount flickered).

Flipping a step from `SurfaceOnly` to `AutoRepair` is a deliberate
code change, visible in review. The compiler enforces this:
`policy()` is `const fn` and the variants don't carry the
classification at runtime.

## Surface

```rust
pub enum ReconcileStep { Invariants /* future steps land here */ }

pub enum StepRepairPolicy { AutoRepair, SurfaceOnly }

pub struct StepReport {
    pub step: &'static str,
    pub policy: StepRepairPolicy,
    pub drift_found: u64,
    pub repaired: u64,
    pub surfaced: u64,
}

pub struct ReconcileReport { pub steps: Vec<StepReport> }

pub async fn run_continuous(pool: &SqlitePool) -> sqlx::Result<ReconcileReport>;
```

`run_continuous` runs every step in declaration order; side effects
fire as each step runs (logs, events, repairs); the report
aggregates the counts.

## The current step set

### `invariants` — SurfaceOnly

Runs the [`invariants`](./invariants.md) suite via
`invariants::check_all` and logs every violation. SurfaceOnly because
violations span all kinds of state — there's no single safe
repair action that fits "every imported download missing media"
*and* "show with no seasons" *and* "blocklist hash uppercased". The
follow-up steps below break that down by class.

## Step set roadmap

The framework's first wired step is the invariant suite. The
periodic-safe phases of the existing `startup::reconcile` migrate
into the framework as additional steps in the next phase:

- `download_state_sync` (AutoRepair) — DB download phases vs librqbit.
- `ghost_torrent_cleanup` (AutoRepair) — torrents in librqbit with
  no DB row.
- `orphan_db_rows` (AutoRepair) — broken FK rows across media,
  stream, media_episode, download_content.
- `stuck_trickplay_claims` (AutoRepair) — rows stuck in claim states.
- `missing_media_files` (SurfaceOnly) — media rows whose file_path
  doesn't exist on disk. Surface only — a flickering mount must
  not delete user data.
- `blocklist_hash_normalize` (AutoRepair) — auto-fix the
  `blocklist_hashes_normalized` invariant.

The migration is a per-step move that keeps the existing tests
passing and adds the AutoRepair classification to the variants
that earn it.

## Cadence

60 seconds, hard-coded. Tunable later if needed; the existing
periodic-safe steps from `startup::reconcile` already work at this
cadence in practice (per the survey that built this framework).

Steps that are too expensive for 60s (filesystem walks of large
libraries) get longer cadences via separate scheduler tasks rather
than skipped inside the reconcile loop — keeps the contract
simple: every reconcile step runs every tick.

## How reconciliation relates to other patterns

- **Invariants** declare what facts must hold. Reconciliation runs
  the predicates and acts on the violations.
- **State machines** declare what transitions are valid.
  Reconciliation catches rows that ended up in an impossible state
  (e.g. `imported` with no media row) and either fixes them (auto)
  or surfaces them.
- **Operations** must leave invariants holding when they commit.
  Reconciliation is the safety net for the case where they didn't
  (crash, partial commit, external system out-of-sync).
- **CleanupTracker** retries individual removals. Reconciliation
  detects that the underlying state is wrong; CleanupTracker is
  the mechanism for one specific repair pattern (resource removal).

## Cost

Each step: O(rows) read, near-zero write on a healthy DB. The
invariants step on a 5-invariant suite costs ~50ms on a populated
test DB. The 60s cadence is dominated by the `cargo run` loop
overhead, not the reconciliation work.
