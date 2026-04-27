# Consistency model

Kino is single-user but interacts with several actors that all hold
state independently:

- **kino's SQLite database** — single source of truth for kino state
- **librqbit** — holds torrent state (active hashes, peer info)
- **Filesystem** — holds media files, sidecar artifacts
- **TMDB** — external metadata
- **Trakt** — external watched/ratings/watchlist
- **Indexers** — external search results, capabilities

Different boundaries call for different consistency contracts.
Mixing them silently is the cause of several codex bugs (#21, #34
in particular). This doc declares the contracts so consumers can
honor them.

## The contracts

### kino DB internal — strongly consistent

All writes within kino's database are SQLite-transactional. An
operation's reads-and-writes inside one transaction see a
consistent snapshot.

**Implication for code:** within an operation's transaction, you
can trust state you just read. Outside a transaction, between
reads and writes, state can change — re-read inside the tx.

### kino DB ↔ librqbit — strongly consistent within process,
**reconciled mid-flight**

We aim for strong consistency: every active `download` row has a
matching librqbit hash, every transition is mirrored both ways.
Achieved by:

- Operations that touch both go in a defined order:
  `add_to_librqbit → if ok, write DB row → if DB write fails, call remove_from_librqbit`.
- A periodic reconciliation task (every 60s) walks both sides and
  surfaces drift.
- Startup reconciliation does the same on boot.

**Implication for code:** trust the DB for reads; mistrust it when
the consequence is "act on librqbit" — at that boundary, validate
the assumption.

### kino DB ↔ filesystem — eventually consistent

Files can disappear (manual delete, disk failure, restore). We
don't pretend otherwise.

- Every code path that opens a file MUST handle "file missing"
  gracefully (return `NoSource`, surface in health, never panic).
- Periodic invariant checks surface drift.
- File operations during operations are wrapped in retry-or-fail
  semantics, never "log + continue."

**Implication for code:** never assume `media.file_path` points to
a real file. Always probe / handle gracefully.

### kino DB ↔ TMDB — eventually consistent, with caching

TMDB is authoritative for metadata. We cache it; the cache can be
stale. Periodic refresh narrows the staleness window.

**Implication for code:** never block user-visible operations on
TMDB. The acquisition path uses cached metadata; refresh runs
asynchronously.

### kino DB ↔ Trakt — eventually consistent

Trakt is authoritative for the user's "what have I watched" truth
across devices. Kino mirrors it both ways with a delay.

**Implication for code:** consumers that depend on watched state
(e.g. `wanted_search` deciding whether to grab) MUST re-read the
local mirror at the point of decision. The mirror IS the
eventually-consistent view; the consumer doesn't wait for Trakt.

This is the fix for codex #34 (search races Trakt sync).

### kino DB ↔ indexers — eventually consistent, snapshot model

Search results are a snapshot. We never expect them to be stable.
We re-search on backoff.

**Implication for code:** results are evaluated immediately; the
grab decision happens on the snapshot we got back. If a release
disappears between search and grab, that's an expected error
state, not a bug.

## The "re-read at decision time" rule

**This is the most load-bearing rule in the consistency model.**

Any operation that makes a decision based on derived state MUST
re-read that state inside the same transaction as the action. Don't
trust:

- Event payloads (`MovieAdded { ... }` carries a snapshot, not a
  promise)
- Earlier reads in the same function (the world may have changed)
- Caller-provided assertions ("the watch state is X")

Always re-read. The cost is one indexed SELECT; the benefit is
that several entire bug classes become impossible.

## Anti-patterns this prevents

| Bug shape | Example | Why it's prevented |
|---|---|---|
| "Spawn before listener catches up" | `MovieAdded` → spawn `wanted_search` racing `trakt_sync` | re-read `watched_at` at grab time |
| "DB lies about external system" | `cleaned_up` row but torrent still seeding | reconciliation catches drift; CleanupTracker retries failed removes |
| "Cached state used after invalidation" | Stream probe returns first-episode metadata for second episode | re-key by `(download_id, file_idx)`; cache contract is per-file |
| "Optimistic concurrency without check" | Two requests update the same row, one's changes lost | transactions + re-read inside |

## Cross-references

- [`operations.md`](./operations.md) — operation shape that
  enforces re-read at decision time.
- [`invariants.md`](./invariants.md) — predicates that catch
  consistency violations after they happen.
- [`state-machines.md`](./state-machines.md) — state machines
  define the *transitions*; consistency model defines the
  *guarantees around them*.
