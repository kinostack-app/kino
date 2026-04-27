# Operations

An **operation** in kino is a state-transitioning function with a
specific shape. It is the primary unit of behaviour. State machines
declare what's possible; operations make transitions happen.

## The shape

Every operation follows this skeleton:

```rust
pub async fn name_of_op(state: &AppState, args: Args) -> Result<Output> {
    // 1. Validate preconditions.
    //    Read-only DB checks. Reject early with a typed error
    //    (Reject::AlreadyDone, Reject::Blocklisted, etc).
    let target = load_target(state, args.id).await?;
    policy.evaluate(&target, &args)?;

    // 2. Execute atomically.
    //    Single SQL transaction. External side-effects (librqbit,
    //    TMDB, filesystem) happen INSIDE the transaction or AFTER
    //    commit — never interleaved.
    let mut tx = state.db.begin().await?;
    let row = persist_changes(&mut tx, &args).await?;
    tx.commit().await?;

    // 3. External side-effects (after commit).
    //    librqbit calls, file moves, etc. Tracked by CleanupTracker
    //    if they must succeed.
    state.torrent.add(&row.hash).await?;

    // 4. Verify invariants.
    //    Cheap checks that the world is consistent.
    debug_assert_invariants(state, &row).await;

    // 5. Emit events.
    //    Last step. Listeners can fire side-effects of their own
    //    that depend on the new state being committed.
    state.emit(AppEvent::SomethingChanged { id: row.id });

    Ok(row.into())
}
```

## Required properties

Every operation MUST satisfy:

### Idempotency
Re-running the operation with the same args produces the same
result. If the target is already in the desired state, it's a no-op
(or returns the existing result), not an error.

This makes retries trivially safe. Scheduler can re-fire, startup
reconciliation can replay, the user can double-click — none of
those break anything.

### Atomicity
Either the operation completes fully or leaves no trace. No
partial state visible to other code paths. The way we achieve
this:

- **Pre-fetch external data first** (TMDB calls, ffprobe, etc).
  No DB writes until we have everything.
- **Single SQL transaction** for all writes that must commit
  together.
- **External side-effects after commit.** If they fail, they're
  retryable via `CleanupTracker`.
- **Events last.** Listeners react to the new state; they need
  the state to actually be committed.

### Validation at preconditions
The operation enforces all relevant policy at the *start*. No
"do half the work, then realise we shouldn't have." If the
operation gets past validation, it's allowed to make the change.

### Re-read derived state inside the transaction
If the operation depends on derived state (`watched_at`, current
phase, etc) for its decision, it re-reads inside the transaction,
not from a stale snapshot.

This closes a class of races: the consumer doesn't trust an event
or a previous read; it asks the database authoritatively right
before acting.

## Anti-patterns this prevents

| Bug shape | What it looks like | Prevented because |
|---|---|---|
| Insert-then-die | `insert show; for season in ... insert season; emit Added` — TMDB error mid-loop = orphan show | Pre-fetch, then transaction, then emit |
| Spawn-and-pray | `tokio::spawn(do_work)` with no completion signal | Operations are awaited; spawn is rare and explicit |
| Read-stale-then-act | Listener queues `wanted_search` based on `MovieAdded`'s snapshot | Re-read derived state inside the operation's tx |
| Silent-swallow | `client.remove(hash).await.ok();` | External failures route through `CleanupTracker` |
| Half-validated | Some grab paths check blocklist, others don't | All grabs route through `AcquisitionPolicy::evaluate` |

## What's NOT required

Operations don't have to be a trait. They're a *shape*, not a
framework. Most are plain `async fn`s in a domain module's
`operations.rs`. Reviewers enforce the shape.

For genuinely common scaffolding (transaction begin, event emit
ordering, invariant assertion), a small helper or macro is fine.
But we resist a `trait Operation` with associated types — it adds
indirection without removing repetition.

## Cross-references

- State machines define the *valid transitions* operations can
  perform. See [`state-machines.md`](./state-machines.md).
- Invariants define the *facts that must always hold* before and
  after every operation. See [`invariants.md`](./invariants.md).
- Consistency model defines *which state operations can trust
  without re-reading*. See [`consistency-model.md`](./consistency-model.md).
