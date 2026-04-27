# Backup & restore

> **Phase 1 MVP shipped (2026-04-26).** The `backup` table, the
> `backup` module (`archive::create` / `restore_backup_id` /
> `restore_path` / `delete_one` / scheduled task), the
> `/api/v1/backups/*` endpoints, and the Settings → Backup page are
> all live. Defaults: enabled, daily at 03:00, retention 7,
> location `{data_path}/backups/`. The setup-wizard step described
> below is **deliberately not implemented** — backups land
> on-by-default invisibly; the surface is Settings → Backup.
>
> **Phase 2 (2026-04-26):** restore now exits the process with
> `EX_TEMPFAIL` (75) after staging files. Service supervisors
> (systemd / launchd / Windows SCM / Docker `restart: always`) pick
> the binary back up against the freshly-restored database — no
> manual restart needed. Tarball / `cargo install` users see the
> clear log line + frontend toast and re-launch by hand.
>
> **Still deferred:**
> - In-process AppState rebuild (no exit, scheduler / librqbit /
>   cast workers swap their handles to the new DB pool live).
>   Operators hit restore ~once a year — the supervisor restart is
>   acceptable, the in-process variant is invasive.
> - Cron-expression schedules (only `daily` / `weekly` / `monthly`
>   / `off` presets in v1).
> - Filesystem picker for the location field (text input today).
> - Notification subsystem hookup for `BackupCreated` /
>   `BackupDeleted` / `BackupRestored` events.

Export Kino's configuration and database to a single archive; restore from that archive later. Protects against disk failure, botched upgrades, and migrating between hosts. Scheduled automatic backups with retention.

## Scope

**In scope:**
- Manual and scheduled backups to a local filesystem path
- Download/upload backups via the API
- Full-replace restore (overwrite current state)
- Automatic pre-restore snapshot (safety net)
- Retention policy on scheduled backups

**Out of scope:**
- Point-in-time recovery — backups are full-snapshot only
- Remote destinations (S3, rclone, SFTP) — future, not v1
- Differential / incremental backups — complexity not justified at our scale
- **User's media library** — the actual video files live on user-managed disks, outside Kino's data. We back up metadata and state, not content.
- Encryption at rest — backups contain API tokens and OAuth credentials, but rely on filesystem permissions. Users needing encryption use disk-level encryption (LUKS, APFS, etc.) — Kino doesn't add its own layer in v1.
- Merge/partial restore — too many edge cases with in-flight downloads, half-synced Trakt state, etc. Replace-only keeps the model simple.

## What's in the backup

| Item | Source | Rationale |
|---|---|---|
| `manifest.json` | Generated | Kino version, backup kind, timestamp, contents checksum |
| `config.toml` | Config file | All settings including API keys, Trakt tokens, indexer credentials |
| `db.sqlite` | SQLite via `VACUUM INTO` | Authoritative state: library, downloads, history, sync state |
| `secrets/` | `{data_path}/secrets/` if present | VPN configs, per-provider credentials stored outside config.toml |

**Deliberately excluded:**

- `{data_path}/trickplay/` — regenerable from media files; huge (~10 MB per movie)
- `{data_path}/fingerprints/` — regenerable; ~20 KB per episode times library size
- `{data_path}/transcode/` — ephemeral session cache
- `{data_path}/logs/` — not needed for restore
- `librqbit/` session files — in-progress torrent state is considered volatile; restoring active torrents across machines / Kino versions is error-prone. On restore, librqbit rebuilds from the downloads it finds on disk via its existing resume logic, plus any `Download` entity with state `downloading` re-enqueues through the search→grab path if the partial data is absent.

Result: backups are small — typically 1–50 MB for a library of any size.

## Archive format

Single `tar.gz` file with a stable layout:

```
kino-backup-{iso_timestamp}-v{kino_version}.tar.gz
├── manifest.json
├── config.toml
├── db.sqlite
└── secrets/
    └── ...
```

`manifest.json`:

```json
{
  "kino_version": "0.4.2",
  "schema_version": 12,
  "created_at": "2026-04-18T03:00:00Z",
  "kind": "scheduled",
  "size_bytes": 4827391,
  "checksum_sha256": "..."
}
```

Version metadata matters for restore safety (§"Restore").

## 1. Backup creation

### Trigger sources

- **Manual** — user clicks "Create backup now" in Settings
- **Scheduled** — scheduler subsystem runs a daily (configurable) job
- **Pre-restore** — automatically triggered before any restore (never lose current state)

### Flow

1. Acquire a SQLite consistency snapshot: `VACUUM INTO '/tmp/kino-backup-xxx/db.sqlite'`. WAL-safe and faster than file-copy; produces a defragmented image.
2. Copy `config.toml` and `secrets/*` into the temp dir.
3. Generate `manifest.json` with version, timestamp, SHA-256 of each file.
4. `tar -czf` the temp dir into `{backup_location}/kino-backup-{timestamp}-v{version}.tar.gz`.
5. Delete temp dir.
6. Insert a row into the `backup` table tracking the new file.
7. Apply retention: delete scheduled-kind backups beyond `backup_retention_count`.

Manual backups are exempt from retention — user explicitly created them, user deletes them.

### Concurrency safety

SQLite `VACUUM INTO` is transactional — no risk of capturing a partially-written state. For the config file and secrets, we take a brief mutex (no config changes during backup). Downloads and scrobbles continue uninterrupted during backup — they write to the live DB, not the snapshot.

Typical backup time: <5 seconds.

### Retention

Default: keep 7 most recent scheduled backups. Delete older ones after each successful new backup. Manual-kind backups never expire automatically.

Configurable:
- `backup_retention_count` (default 7)
- `backup_location_path` (default `{data_path}/backups/`)
- `backup_schedule_cron` (default `0 3 * * *` — daily at 3am local)

## 2. Restore

Restore is inherently destructive. The flow bakes in safety:

### Flow

1. **Validate archive.** Open archive, read `manifest.json`, check:
   - Checksum match
   - Kino version compatibility (§"Version compatibility")
   - Schema version ≤ current (downgrades refused)
2. **Create pre-restore safety snapshot.** Silently run a full backup of current state with `kind = pre_restore`. Stored with a distinctive filename (`kino-backup-pre-restore-{timestamp}.tar.gz`). If restore fails or the user regrets it, they can restore *this* snapshot to roll back.
3. **Stop live operations.** Pause librqbit session, stop the scheduler, close DB connections.
4. **Replace files.**
   - Move current `config.toml` → `.config.toml.bak`
   - Move current DB → `.db.sqlite.bak`
   - Extract archive contents into their target locations
5. **Run migrations.** If the restored DB is at an older `schema_version`, run pending migrations to bring it to current. If the restored DB is at a *newer* schema version, abort and revert (§"Version compatibility").
6. **Insert row** into `backup` table for the pre-restore snapshot (recorded BEFORE exit so it survives the restart).
7. **Exit with `EX_TEMPFAIL` (75).** The service supervisor (systemd / launchd / Windows SCM / Docker `restart: always`) re-launches the binary against the freshly-restored state. librqbit, scheduler, cast workers, and DB pool come up from scratch — no stale handles to flush. Tarball / `cargo install` users see a clear log line + frontend toast and re-launch by hand.
8. **Post-restart health check** (in the new process) reads `.bak` markers and deletes them once the boot is confirmed healthy.

If any step fails before exit, rename `.bak` files back into place and abort without exiting.

### Version compatibility

The combination of `kino_version` + `schema_version` on the manifest vs current decides:

| Situation | Behaviour |
|---|---|
| Same version | Restore as-is |
| Older `kino_version`, older `schema_version` | Restore, then run migrations to bring DB forward |
| Newer `kino_version`, newer `schema_version` | **Abort with clear error** — downgrade not supported |
| Migration path missing between these versions | Abort — point user at release notes |

Practical effect: you can always restore a backup from an older Kino instance onto a newer one; you cannot restore a newer backup onto older Kino. Clear enough for the "I upgraded, it broke, let me downgrade and restore" scenario.

### In-flight state after restore

Things that may be affected:

- **Active downloads** at backup time: `Download` entities restore with whatever state they had. If partial files still exist in the downloads dir, librqbit resumes from bitfield. If the files are gone (new machine), the Download fails on next stall check → Search retries automatically.
- **Trakt queue**: `trakt_scrobble_queue` entries restore. If older than 24h (queue retention limit), they're dropped on next drain.
- **Fingerprint / trickplay caches**: missing after restore on a new host; the scheduler's catch-up tasks regenerate them opportunistically.
- **Current playback sessions**: lost (not backed up). User would have to re-start playback — acceptable since restore is a rare operation.

## 3. Schema

### `backup` (new)

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | |
| kind | TEXT NOT NULL | `manual` / `scheduled` / `pre_restore` |
| filename | TEXT NOT NULL | Relative to `backup_location_path` |
| size_bytes | INTEGER NOT NULL | |
| kino_version | TEXT NOT NULL | At time of backup |
| schema_version | INTEGER NOT NULL | DB migration version at time of backup |
| created_at | TEXT NOT NULL | ISO 8601 |
| checksum_sha256 | TEXT NOT NULL | Of the archive file |

### Config extensions

| Column | Type | Default |
|---|---|---|
| backup_enabled | BOOLEAN | true |
| backup_schedule_cron | TEXT | `0 3 * * *` |
| backup_location_path | TEXT | `{data_path}/backups` |
| backup_retention_count | INTEGER | 7 |

## 4. API

```
GET    /api/v1/backups                List all backups (filter by kind)
POST   /api/v1/backups                Create backup now (manual kind). Returns 202 + handle
GET    /api/v1/backups/{id}           Metadata for one backup
GET    /api/v1/backups/{id}/download  Stream the archive file
DELETE /api/v1/backups/{id}           Delete (refuses pre_restore kind unless confirmed)

POST   /api/v1/backups/restore        Multipart upload or { "backup_id": N }
                                      Returns 202 + task handle for progress
```

Restore endpoint accepts either a file upload (for backups from another machine) or a reference to an existing local backup. Returns a task handle so the frontend can poll progress and report success/failure without holding a long HTTP connection.

## 5. UX

### Settings → Backup

Three sections:

**Backups list.** Table of existing backups: date, kind badge (Manual / Scheduled / Pre-restore), size, Kino version, download / delete actions. Sorted newest first.

**Create backup.** Button "Create backup now" — runs manual backup. Toast on completion: "Backup created (4.7 MB)".

**Schedule.** Toggle for scheduled backups, cron expression (with a plain-English translator: "Daily at 3:00 AM"), retention count, location path.

### Restore modal

Accessible from the backups list ("Restore" button on each row). Opens a modal:

> **Restore from backup?**
>
> Created: 2026-04-15 03:00
> Kino version: 0.4.1 (current: 0.4.2)
> Schema version: 12 (current: 12)
>
> ⚠ This will replace your current config and database. Current state will be saved as a pre-restore backup.
>
> Active downloads will be interrupted and may need to be re-grabbed.
>
> [ Restore ] [ Cancel ]

The modal additionally supports **uploading a file from elsewhere** — drag-and-drop zone or file picker at the top, for cross-machine restores.

### Progress

Restore runs as a scheduler task — the UI polls the task endpoint and shows a spinner + status ("Stopping services...", "Extracting archive...", "Running migrations...", "Restarting...").

On completion: full-page success or error state; the UI refreshes all queries to pick up the new DB state.

### Onboarding wizard step

Backups get a **brief confirmation step near the end of the first-run setup wizard** — not a configuration screen, an informational one. Surfaces the defaults so users know backups are happening:

> **Automatic backups**
>
> Kino will back up your config and database daily at 3:00 AM to `{data_path}/backups/`. Typical size: a few MB.
> Keeps the 7 most recent backups.
>
> [ ✓ Sounds good ]  [ Customise ]

- **Sounds good** → wizard advances with defaults applied
- **Customise** → opens the full Settings → Backup page inline (schedule, location, retention) then returns to wizard

The goal is *awareness without friction*: the user learns backups exist and trusts the defaults, rather than being asked to make decisions about retention counts during onboarding. Power users who want custom paths (e.g. NFS mount) use the Customise path.

Defaults applied if the user never visits this step (skip-ahead wizard): `backup_enabled = true`, schedule `0 3 * * *`, retention 7, location `{data_path}/backups/`.

## Entities touched

- **Reads:** Config, all subsystem state (for backup snapshot)
- **Creates:** `backup`, new DB file at restore
- **Updates:** Config (after restore), all tables (after restore)
- **Deletes:** Old `backup` rows + files per retention; stopped services during restore

## Dependencies

- Scheduler subsystem — runs the cron job
- Notification subsystem — reports backup success/failure, retention deletions
- SQLite `VACUUM INTO` (built into sqlx / rusqlite)
- `tar` + `flate2` crates for archive compression (pure Rust)
- Migrations infrastructure — used during cross-version restore

No new system binaries.

## Error states

- **Disk full at backup location** → skip this backup run, notification fired, retry next cycle
- **Checksum mismatch on restore** → abort immediately, no files touched
- **Newer schema version in archive** → abort with "This backup was made with a newer Kino version. Upgrade Kino and try again." error
- **Migration failure during restore** → revert via `.bak` files, notification, logs preserved
- **Archive corruption** (truncated tar, invalid gzip) → abort, point user at integrity check
- **Restore succeeds but startup health check fails** → `.bak` files *preserved* (don't auto-delete), surface error with "manual recovery needed" message + path to the bak files

## Known limitations

- **No cross-major-version restore.** If we ever introduce a breaking schema change (0.x → 1.0), pre-1.0 backups won't restore. We'd ship an external migration tool if that ever happens.
- **Media library path in restored config must match.** If a user restores onto a new machine where media paths differ, they'll need to edit `config.toml` post-restore. Not worth auto-remapping — path-fixing heuristics are dangerous.
- **Trakt-queued scrobble events older than 24h are lost on restore** regardless. Same behaviour as normal running; no special handling.
- **Concurrent manual + scheduled backup** — a mutex serialises; the later-arriving request waits or no-ops if one is already running within 60s.
- **Backup of the backup location** — recursive if the user's backup location is inside `{data_path}`. We detect this and exclude `backup_location_path` from the archive.
