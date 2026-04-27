# Startup reconciliation

On boot, kino reconciles database state against librqbit's session and the filesystem. This handles crashes, unclean shutdowns, and external changes (files deleted manually, librqbit persistence drift).

Designs that delegate to an external download client poll the client each cycle to reconcile state. Kino's built-in BitTorrent client means the database and librqbit live in the same process and must be kept consistent across restarts.

## Phases

Phases run in strict order — each depends on the previous being complete. Every phase emits an `INFO` tracing event with its counters so `just logs` is the primary operator diagnostic surface.

### Phase 1: Database integrity

1. SQLite WAL recovery (automatic on connection open)
2. Pending schema migrations
3. Verify database is readable

### Phase 2: Orphan cleanup (DB only)

Delete rows with broken foreign-key relationships from partial writes. Each table-level delete logs at DEBUG with the count when non-zero:

```sql
DELETE FROM stream WHERE media_id NOT IN (SELECT id FROM media);
DELETE FROM media_episode WHERE media_id NOT IN (SELECT id FROM media);
DELETE FROM media_episode WHERE episode_id NOT IN (SELECT id FROM episode);
DELETE FROM download_content WHERE download_id NOT IN (SELECT id FROM download);
DELETE FROM download_content WHERE movie_id IS NOT NULL AND movie_id NOT IN (SELECT id FROM movie);
DELETE FROM download_content WHERE episode_id IS NOT NULL AND episode_id NOT IN (SELECT id FROM episode);
```

Phase 2b also resets trickplay rows stuck at claim states (`trickplay_generated IN (2, 3)`) — a killed ffmpeg child from a previous process leaves those rows pinned; clearing them lets the sweep retry.

### Phase 3: VPN + librqbit startup (external)

Handled by `main.rs` before `reconcile()` is called:

1. Connect VPN tunnel (if enabled)
2. Establish port forwarding
3. Start librqbit session — triggers its internal persistence restore
4. Wait for librqbit to finish restoring all persisted torrents

`reconcile()` takes `Option<&dyn TorrentSession>` so when VPN fails or downloads are disabled, the DB-only phases still run and the UI is coherent even without a live session.

### Phase 4a: Download state reconciliation

For each non-terminal `download` row, cross-reference librqbit's live set of info hashes. The table below is the ruleset:

| DB state      | Torrent in librqbit? | Action                                                                 |
|---------------|----------------------|------------------------------------------------------------------------|
| `grabbing`    | any                  | mark `failed` — was mid-add when we crashed; search retries            |
| `importing`   | any                  | mark `failed` — mid-import; search retries                             |
| `stalled`     | yes                  | flip to `downloading`; stall sweep re-evaluates                        |
| `stalled`     | no                   | mark `failed` — torrent gone, client can re-add                        |
| `paused`      | any                  | leave as-is                                                            |
| `downloading` | yes                  | leave as-is; live stall detection handles drift                        |
| `downloading` | no                   | mark `failed` — torrent gone, search retries                           |
| `seeding`     | yes                  | leave alone; seed-limit sweep owns the eventual `cleaned_up` transition|
| `seeding`     | no                   | mark `imported`; attempt source-files cleanup                          |
| `imported`    | any                  | if source files still on disk → remove, mark `cleaned_up`              |
| `completed`   | any                  | leave as-is; import trigger runs on next scheduler tick                |

Every transition logs one INFO line carrying `download_id`, `title`, `torrent_hash` (if any), and the fix applied.

Source-files cleanup (for `seeding → imported` and `imported → cleaned_up`) is best-effort: the library already owns its hardlinked/copied copy via the import pipeline, so removing the download-path source is lossless. A permission error or a path that already doesn't exist doesn't fail reconciliation — it just logs at WARN and continues.

When the torrent session is unavailable (VPN required but failed, or user disabled downloads), phase 4a only applies the torrent-agnostic rules (`grabbing`/`importing` → `failed`) and leaves torrent-dependent rows alone. Better to show stale state than to mark a live torrent dead based on its absence from a session that didn't boot.

### Phase 4b: Unknown-torrent cleanup

Iterate librqbit's `list_torrent_hashes()` and remove any hash that has no matching `download.torrent_hash` in the DB. These "ghost torrents" are usually left over from a mid-grab crash where `session.add_torrent` succeeded but the `download` row write didn't commit.

Removal uses `delete_files=true` — a ghost torrent has no library copy (import never ran), so the on-disk bytes under `download_path` are the only trace of it. Removing the torrent without the files would leave orphan bytes nobody cleans up.

Per-removal logs at WARN with the hash.

### Phase 5: Entity status reconciliation

Derived-status model: `movie.status` / `episode.status` are computed from `(media, active_download, watched_at)` at read time. Phase 5's job is filesystem verification — deleting `media` rows whose file is gone so the derived status reverts to `wanted` and search picks them up.

**Eager vs lazy threshold.** Libraries with ≤ 1000 media rows get an eager per-file `exists()` sweep on boot — fast for the common case, catches stale rows from a mid-disk-failure state before the UI surfaces. Libraries above the threshold skip the sweep — that many `stat()` syscalls against a networked mount can add seconds to startup. Missing files surface naturally on first playback attempt (the playback path already cleans up Media rows whose file is gone), so the cost is deferred, not lost.

The threshold lives as a `const EAGER_VERIFY_THRESHOLD` alongside the reconciliation code so it's one grep to tune.

Phase 5 also handles the upgrade-crash duplicate case at the DB level: when multiple `media` rows link to the same movie (a prior upgrade crashed between old-delete and new-insert), the highest-quality row wins, the others get deleted. [Not yet implemented — tracked as an ITERATE.]

### Phase 6: Resume normal operations

1. Start scheduler (periodic search, cleanup, metadata refresh, stall detection)
2. Start API server + WebSocket
3. Log the reconciliation summary — single INFO line carrying every counter so operators can eyeball "was anything unusual on this boot":

```
orphans=0 ghost_torrents=0 downloads=0 entities=0 files_verified=42
files_verified_lazily=false library_path=/srv/media
```

## Design principles

- **Conservative** — prefer marking things `failed` and letting Search retry over attempting complex recovery. A re-download is cheaper than a corrupt library.
- **Idempotent** — running reconciliation twice produces the same result. Safe if kino crashes during startup itself.
- **Logged** — every reconciliation *action* is logged at INFO level; supporting details (per-table orphan counts, per-download stale-claim resets) at DEBUG. Nothing silent.
- **Fast for clean startups** — if everything is consistent (the common case), each check is a quick DB query that returns zero rows. The happy path adds negligible startup time.
- **Optional torrent session** — `reconcile()` takes `Option<&dyn TorrentSession>`. When VPN is required but failed, DB-only phases still run; torrent-dependent phases are skipped with a debug log noting why.

## Testability

`reconcile()` takes a `&SqlitePool` + `&dyn TorrentSession` trait object, so every phase has coverage under `#[tokio::test]` using `db::create_test_pool()` + `test_support::FakeTorrentSession`. The helper `FakeTorrentSession::add_hash()` stages a hash into the fake session without needing a file fixture, which lets the stalled/seeding/ghost-torrent tests stay tight.
