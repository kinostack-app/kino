# Import subsystem

Post-download processing pipeline. Takes completed torrent files, processes them into the media library, and creates Media + Stream entities.

## Responsibilities

- Extract archives (RAR, ZIP, 7z)
- Discover video files, filter out samples and junk
- Match files to movies/episodes
- Probe media streams via ffprobe
- Rename files using naming templates
- Hardlink (or copy) to library
- Fetch subtitles from OpenSubtitles
- Import sidecar subtitle files
- Clean up source files and torrent

## Pipeline

Triggered when Download subsystem reports a torrent as `completed`. Multiple imports can run concurrently — each operates on its own download directory and writes to distinct library paths, so there's no contention. Entity creation is transactional per import.

Steps run in order per import:

```
1. Extract       — unpack archives if present
2. Discover      — find video files, reject samples/junk
3. Match         — associate each file with a movie or episode
4. Probe         — ffprobe each file for stream metadata
5. Rename        — generate library filename from template
6. Transfer      — hardlink or copy to library path
7. Subtitles     — import sidecar subs + fetch from OpenSubtitles
8. Create        — persist Media, Stream entities; update movie/episode status
9. Cleanup       — remove source files, update Download state
```

## 1. Extract

Scan the download directory for archive files. Supported formats:

- RAR (including multi-part: `.rar`, `.r00`, `.r01`, etc.)
- ZIP
- 7z (including split: `.7z.001`)

**Behaviour:**
- Extract in-place (alongside the archives in the download directory)
- Handle nested archives — scan extracted output for additional archives and extract those too
- If no archives found, skip to step 2
- If extraction fails, mark Download as `failed`, fire notification, trigger Search retry

## 2. Discover

Recursively scan the download directory for video files. Filter by known media extensions (`.mkv`, `.mp4`, `.avi`, `.ts`, `.wmv`, `.flv`, `.m4v`, `.webm`).

**Filtering:**
- **Sample detection:** probe each file's runtime via ffprobe. Reject files shorter than 5 minutes for standard content, 90 seconds for short-form content. Files in directories named `sample` or `Sample` are also rejected.
- **Junk rejection:** ignore `.nfo`, `.txt`, `.jpg`, `.png`, `.exe`, `.bat`, `.sh` and other non-video files.
- **Extras rejection:** ignore files in directories named `extras`, `featurettes`, `behind the scenes`, etc.

If zero video files remain after filtering, mark Download as `failed` with "no video files found".

## 3. Match

Associate each discovered video file with a movie or episode from the database.

**For movies (single file expected):**
- The Download's DownloadContent links tell us which movie this is for
- If multiple video files found, pick the largest (the others are likely samples that slipped through)

**For TV episodes (season packs may have many files):**
- Parse each filename through the release parser to extract season + episode numbers
- Match against Episode entities linked via DownloadContent
- Each file maps to one or more episodes (double episodes possible)
- Files that don't match any wanted episode are skipped (not imported)

**If matching fails** for a file (parser can't extract episode numbers, or no matching episode in DB), log a warning and skip that file. If no files match at all, mark Download as `failed`.

## 4. Probe

Run ffprobe on each matched video file to extract stream metadata.

**Command:**
```
ffprobe -i {file} -v warning -print_format json -show_streams -show_format -show_chapters
```

**Extracted per stream:**

| Stream type | Fields |
|---|---|
| **Video** | codec, profile, width, height, framerate, pixel_format, bit_depth, bitrate, color_space, color_transfer, color_primaries, HDR format (HDR10/HDR10+/Dolby Vision/HLG — derived from color metadata and side data) |
| **Audio** | codec, channels, channel_layout, sample_rate, bit_depth, bitrate, language, title, default/forced flags |
| **Subtitle** (embedded) | codec, language, title, forced flag, hearing_impaired flag |

**HDR detection logic:**
- bit depth < 10 → SDR
- DOVI configuration record in side data → Dolby Vision (with sub-variant based on compatibility ID)
- bt2020 primaries + smpte2084 transfer + HDR10+ side data → HDR10+
- bt2020 primaries + smpte2084 transfer → HDR10
- bt2020 primaries + arib-std-b67 transfer → HLG

Probe results are used to populate Media quality fields and Stream entities.

### Language verification

After probing, check the audio track languages against the QualityProfile's `accepted_languages`:

- If at least one audio track matches an accepted language → pass
- If no audio tracks match any accepted language → flag as **language mismatch**
- On mismatch: log a warning, fire a notification, and still import (the user may want it anyway). The UI shows a warning badge on the media file. The user can manually delete and re-search.

This catches the case where search accepted a release with no language info in the title, but the actual file turns out to be in the wrong language.

## 5. Rename

Generate the library filename using the naming templates from Config:

- `movie_naming_format` — default: `{title} ({year}) [{quality}]`
- `episode_naming_format` — default: `{show} - S{season:00}E{episode:00} - {title} [{quality}]`
- `season_folder_format` — default: `Season {season:00}`

**Available tokens:**

| Token | Example output |
|---|---|
| `{title}` | The Matrix / Ozymandias |
| `{show}` | Breaking Bad |
| `{year}` | 1999 |
| `{season}` / `{season:00}` | 5 / 05 |
| `{episode}` / `{episode:00}` | 14 / 14 |
| `{quality}` | Bluray-1080p |
| `{resolution}` | 1080p |
| `{source}` | Bluray |
| `{codec}` | H.265 |
| `{hdr}` | HDR10 |
| `{audio}` | DTS-HD MA 5.1 |
| `{group}` | GROUP |
| `{imdb}` | tt0133093 |
| `{tmdb}` | 603 |

Token replacement is a simple string substitution. Invalid filesystem characters are stripped. Repeated separators are collapsed. Max path length enforced by truncating the title portion.

**Library structure:**

```
{media_library_path}/
  Movies/
    The Matrix (1999) [Bluray-1080p].mkv
  TV/
    Breaking Bad/
      Season 05/
        Breaking Bad - S05E14 - Ozymandias [Bluray-1080p].mkv
```

**Folder creation:** directories are created on demand during transfer. If `TV/Breaking Bad/Season 05/` doesn't exist, it's created recursively before the file is placed. Empty show/season folders are cleaned up when the last media file in them is removed (by the Cleanup subsystem).

## 6. Transfer

Move the file from the download directory to the library.

**Strategy: hardlink first, copy fallback.**

1. Attempt `link()` (POSIX hardlink) from source to library destination
2. If hardlink succeeds → done. Source file and library file share the same disk blocks — zero extra disk space used. Source stays intact for seeding.
3. If hardlink fails with `EXDEV` (cross-device) → fall back to copy
4. If copy succeeds → source stays intact for seeding

No explicit same-filesystem check needed — try the hardlink, catch the error. Standard pattern for media-import pipelines.

When `use_hardlinks` is disabled in Config, always copy.

After seeding completes (handled by Download subsystem), source files are deleted. The library copy persists independently.

## 7. Subtitles

Two sources of subtitles:

### Sidecar files (from the download)
Scan the download directory for subtitle files alongside the video (`.srt`, `.ass`, `.ssa`, `.sub`, `.idx`, `.vtt`). Parse language and flags from the filename:

```
Movie.en.srt             → English
Movie.en.forced.srt      → English, forced
Movie.en.hi.srt          → English, hearing impaired
Movie.fr.srt             → French
```

Copy sidecar subtitles to the library alongside the media file with matching filenames. Create Stream entities with `is_external = true`.

### OpenSubtitles (automatic download)
After import, if the media file has no subtitle track matching the user's preferred language (from QualityProfile `accepted_languages`):

1. Search OpenSubtitles API by IMDB ID + language
2. Pick the best match (highest download count)
3. Download and save alongside the media file
4. Create Stream entity with `is_external = true`

If OpenSubtitles credentials aren't configured, skip silently.

## 8. Create entities

For each imported file:

1. **Media** — create entity with file path, size, quality fields (from probe + release parser), scene name, release group, date added. For movies, set `movie_id`. For episodes, leave `movie_id` null.
2. **MediaEpisode** — for TV, create join table entries linking Media to Episode(s). One row per episode. Two rows for double-episode files (S01E01E02).
3. **Streams** — create one entity per stream from ffprobe (video, audio, embedded subtitles) plus one per external subtitle file
4. **Movie/Episode** — update status from `downloading` to `available`

## 9. Cleanup

After all files are imported:

1. Delete extracted archive output (if archives were extracted in step 1)
2. Torrent continues seeding from source files (Download subsystem manages seeding lifecycle)
3. When seeding completes, Download subsystem deletes source files and marks Download as `cleaned_up`

If the download directory contains only junk after import (no video files left, just `.nfo`, small `.rar` parts, etc.), delete the entire directory immediately.

## Upgrade handling

When import is triggered for content that already has a Media entity (quality upgrade):

1. Complete the normal import pipeline (probe, rename, transfer, subtitles)
2. Delete the old media file from the library
3. Delete old Stream entities
4. Delete old external subtitle files
5. Create new Media + Stream entities
6. Log History event (`upgraded`)

The old file is deleted after the new one is confirmed in place — never leave the user without a copy.

## Entities touched

- **Reads:** Download + DownloadContent (what was downloaded and for which content), Movie/Episode (matching targets), Config (naming formats, library paths, hardlink preference, OpenSubtitles credentials), QualityProfile (accepted languages for subtitle fetch)
- **Creates:** Media, Stream (new file and all its tracks)
- **Updates:** Movie/Episode (status → `available`), Download (state → `importing` → `imported`)
- **Deletes:** Media, Stream (on upgrade — old file replaced)

## Dependencies

- ffprobe (external binary, installed alongside ffmpeg — bundled in platform packages, or picked up from PATH)
- OpenSubtitles API (external, optional)
- Download subsystem (triggers import, handles seeding/cleanup)
- Release parser (shared utility — for matching filenames to content)
- Notification subsystem (import complete, import failed events)
- Filesystem (hardlink, copy, rename)

## Error states

- **Extraction fails** → Download marked `failed`, retry with different release
- **No video files found** → Download marked `failed`, retry
- **File matching fails** → warning logged, unmatched files skipped
- **ffprobe fails** → import continues with limited metadata (quality fields from release parser only)
- **Hardlink fails and copy fails** → import failed, likely disk full → notification
- **OpenSubtitles unavailable** → subtitle fetch skipped silently, can retry manually later
- **Disk full during transfer** → import failed, notification fired, downloads paused
