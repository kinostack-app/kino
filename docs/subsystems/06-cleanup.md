# Cleanup subsystem

Removes watched content from disk after a configurable delay. Manages disk space so the library trends toward empty, not full.

## Responsibilities

- Remove media files for watched content after delay
- Remove empty show/season folders
- Clean up orphaned images, subtitles, and transcode temp files
- Monitor disk space and warn when low
- Prioritise cleanup of oldest watched content when space is tight

## Watched content removal

### Movies

When a movie is marked `watched`:
1. `watched_at` timestamp is set
2. After `auto_cleanup_movie_delay` hours (default 72, from Config), the movie's media file is eligible for deletion
3. On next cleanup cycle: delete the Media file from disk, delete associated Stream entities, delete external subtitle files, delete the Media entity
4. Movie entity stays in the database with `status: watched` — it remains in your history, just no file on disk
5. If the user wants it again later, they re-request it (status back to `wanted`)

### Episodes

Episodes are cleaned up at the **season level**, not individually. Each season is evaluated independently — finishing Season 1 triggers cleanup for Season 1 even if later seasons are still in progress.

1. Each episode is marked `watched` as the user watches it
2. A season is considered complete when every monitored episode in that Series (season) has `status: watched`
3. After `auto_cleanup_episode_delay` hours (default 72) from the **last** episode's `watched_at` in that season, all media files for the season are eligible
4. Delete all Media files, Streams, and external subtitles for the season
5. Episode entities stay with `status: watched`

Why season-level? Users often rewatch episodes within a season, or watch out of order. Cleaning up individual episodes while the user is mid-season would be frustrating.

### Opt-out

- `auto_cleanup_enabled` in Config (default true). When false, nothing is ever automatically deleted.
- Users can also manually delete individual media files via the API/UI without waiting for the delay.

## Folder cleanup

After media files are removed, empty directories are cleaned up:

```
{media_library_path}/TV/Breaking Bad/Season 05/   ← empty after cleanup
{media_library_path}/TV/Breaking Bad/              ← empty if all seasons cleaned
```

Walk up from the deleted file's directory. Remove each empty directory until hitting one that still contains files or reaching the `Movies/` or `TV/` root.

## Disk space monitoring

Periodic check (every hour) on the filesystem containing `media_library_path`:

| Free space | Action |
|---|---|
| > 50 GB | Normal — no action |
| 10–50 GB | **Warning** — fire notification |
| < 10 GB | **Critical** — fire notification, trigger aggressive cleanup |

### Aggressive cleanup

When disk space is critical, override the normal delay timers:

1. Find all content with `status: watched` regardless of how recently it was watched
2. Sort by `watched_at` ascending (oldest watched first)
3. Delete media files one at a time, checking free space after each
4. Stop when free space exceeds 50 GB or no more watched content remains
5. If still critically low after removing all watched content, pause downloads and fire a notification

Thresholds are sensible defaults. Not configurable in v1 — keep it simple.

## Transcode temp cleanup

Transcode temp files (HLS segments, progressive transcode output) in the temp directory:

- Cleaned up when the playback session ends (Playback subsystem handles this)
- As a safety net, the Cleanup subsystem scans the temp directory hourly and removes any files older than 2 hours (no active transcode should last that long)

## Image cache cleanup

When a movie or show entity is removed from the database entirely (not just media deleted — the content itself removed):
- Delete the image directory for that content (`{data_path}/images/originals/{type}/{tmdb_id}/`)
- Resized image cache entries are also removed

Resized image cache can also be purged entirely on demand (API endpoint) since it regenerates on access.

## Trigger

The Scheduler runs cleanup on a configurable interval (default: every hour). Cleanup also runs immediately when:
- Disk space drops below the critical threshold
- User manually triggers cleanup via the API

## Entities touched

- **Reads:** Movie (watched_at, status), Episode (watched_at, status), Series (to check season completion), Media (file paths), Stream (for deletion), Config (cleanup delays, enabled flag)
- **Deletes:** Media, Stream (MediaEpisode join entries cleaned up via cascade)
- **Updates:** nothing — Movie/Episode stay at `status: watched` after cleanup

## Dependencies

- Filesystem (delete files, check free space, remove empty directories)
- Scheduler (triggers periodic cleanup)
- Notification subsystem (disk space warnings, aggressive cleanup events)
- Config table (delays, enabled flag)

## Error states

- **File already gone** (manually deleted by user) → skip, clean up the Media/Stream entities
- **Permission denied on delete** → log error, skip file, try again next cycle
- **Disk space check fails** → log warning, skip monitoring this cycle
