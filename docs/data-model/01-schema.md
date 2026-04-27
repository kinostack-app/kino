# kino — data model

Canonical source: `backend/migrations/*.sql`. This doc mirrors the
migrations on disk; if they disagree, the migrations win. Early-stage
dev means we rewrite the initial schema rather than accumulating
`ALTER` migrations — any prior kino install wipes via `just reset`.

All dates are ISO 8601 text (SQLite has no native datetime). JSON
payloads are stored as TEXT and queried via SQLite's `json_*`
functions. `BOOLEAN` columns are `INTEGER` with 0/1 values.

## Domain entities

### Movie

A movie the user has requested or has in their library.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| tmdb_id | INTEGER UNIQUE NOT NULL | |
| imdb_id | TEXT | From TMDB external_ids |
| tvdb_id | INTEGER | From TMDB external_ids |
| title | TEXT NOT NULL | |
| original_title | TEXT | |
| overview | TEXT | |
| tagline | TEXT | |
| year | INTEGER | |
| runtime | INTEGER | Minutes |
| release_date | TEXT | Theatrical (YYYY-MM-DD) |
| physical_release_date | TEXT | Blu-ray/DVD |
| digital_release_date | TEXT | Streaming/digital |
| certification | TEXT | PG-13, R, etc. |
| poster_path | TEXT | TMDB image path |
| backdrop_path | TEXT | TMDB image path |
| genres | TEXT | JSON array of strings |
| tmdb_rating | REAL | 0–10 |
| tmdb_vote_count | INTEGER | |
| popularity | REAL | TMDB popularity score |
| original_language | TEXT | ISO 639-1 |
| collection_tmdb_id | INTEGER | For franchise grouping |
| collection_name | TEXT | |
| youtube_trailer_id | TEXT | |
| quality_profile_id | INTEGER NOT NULL FK | |
| monitored | INTEGER NOT NULL DEFAULT 1 | For quality upgrades |
| added_at | TEXT NOT NULL | |
| blurhash_poster | TEXT | Blurhash placeholder for poster |
| blurhash_backdrop | TEXT | Blurhash placeholder for backdrop |
| playback_position_ticks | INTEGER DEFAULT 0 | Resume position |
| play_count | INTEGER DEFAULT 0 | |
| last_played_at | TEXT | |
| watched_at | TEXT | When marked watched (cleanup ordering) |
| preferred_audio_stream_index | INTEGER | Remembered preference |
| preferred_subtitle_stream_index | INTEGER | Remembered preference |
| last_metadata_refresh | TEXT | |
| last_searched_at | TEXT | Avoid re-searching too often |
| user_rating | INTEGER 1–10 | Trakt rating scale; NULL = unrated |
| logo_path | TEXT | Clearlogo relative path under data_path |
| logo_palette | TEXT | `mono` / `multi` — how to style in UI |

**No `status` column.** Phase (`wanted` / `downloading` / `available`
/ `watched`) is derived on read via a `CASE` over `media`,
`download_content`, and `watched_at`. Canonical derivation lives in
`services/phase.rs`.

### Show

A TV show container. `status` here is TMDB's airing status
(`returning` / `ended` / `in_production`), not an acquisition phase.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| tmdb_id | INTEGER UNIQUE NOT NULL | |
| imdb_id | TEXT | |
| tvdb_id | INTEGER | |
| title | TEXT NOT NULL | |
| original_title | TEXT | |
| overview | TEXT | |
| tagline | TEXT | |
| year | INTEGER | First air year |
| status | TEXT | TMDB airing status |
| network | TEXT | |
| runtime | INTEGER | Typical episode length, minutes |
| certification | TEXT | |
| poster_path | TEXT | |
| backdrop_path | TEXT | |
| genres | TEXT | JSON array |
| tmdb_rating | REAL | |
| tmdb_vote_count | INTEGER | |
| popularity | REAL | |
| original_language | TEXT | |
| youtube_trailer_id | TEXT | |
| quality_profile_id | INTEGER NOT NULL FK | |
| monitored | INTEGER NOT NULL DEFAULT 1 | |
| monitor_new_items | TEXT NOT NULL DEFAULT 'future' | `future` / `none` |
| monitor_specials | INTEGER NOT NULL DEFAULT 0 | Opt-in for Season 0 |
| follow_intent | TEXT NOT NULL DEFAULT 'explicit' | `explicit` / `adhoc` |
| added_at | TEXT NOT NULL | |
| blurhash_poster | TEXT | |
| blurhash_backdrop | TEXT | |
| first_air_date | TEXT | |
| last_air_date | TEXT | |
| last_metadata_refresh | TEXT | |
| user_rating | INTEGER 1–10 | |
| skip_intros | INTEGER NOT NULL DEFAULT 1 | Per-show intro-skipper toggle |
| logo_path | TEXT | Clearlogo relative path |
| logo_palette | TEXT | `mono` / `multi` |

`follow_intent = 'adhoc'` means Play / Get auto-followed the show; the
row self-removes when its last acquired episode is discarded.

### Series

A season within a show.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| show_id | INTEGER NOT NULL FK → show(id) | CASCADE |
| tmdb_id | INTEGER | TMDB season ID |
| season_number | INTEGER NOT NULL | 0 = specials |
| title | TEXT | |
| overview | TEXT | |
| poster_path | TEXT | |
| air_date | TEXT | |
| monitored | INTEGER NOT NULL DEFAULT 1 | |
| episode_count | INTEGER | |

UNIQUE(`show_id`, `season_number`).

### Episode

An episode within a series. The old `monitored` flag was split into
two orthogonal axes so Play-auto-follow, Latest-only, and external-
library cases can all coexist:

- `acquire` — should the scheduler auto-search + grab releases?
- `in_scope` — is this episode part of what the user is progressing
  through (counted in Next Up, aired totals, etc.)?

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| series_id | INTEGER NOT NULL FK → series(id) | CASCADE |
| show_id | INTEGER NOT NULL FK → show(id) | CASCADE; denormalised |
| season_number | INTEGER NOT NULL | Denormalised from series |
| tmdb_id | INTEGER | |
| tvdb_id | INTEGER | |
| episode_number | INTEGER NOT NULL | |
| title | TEXT | |
| overview | TEXT | |
| air_date_utc | TEXT | |
| runtime | INTEGER | Minutes |
| still_path | TEXT | |
| tmdb_rating | REAL | |
| acquire | INTEGER NOT NULL DEFAULT 1 | Scheduler auto-acquire |
| in_scope | INTEGER NOT NULL DEFAULT 1 | Counts toward progress |
| playback_position_ticks | INTEGER DEFAULT 0 | |
| play_count | INTEGER DEFAULT 0 | |
| last_played_at | TEXT | |
| watched_at | TEXT | |
| preferred_audio_stream_index | INTEGER | |
| preferred_subtitle_stream_index | INTEGER | |
| last_searched_at | TEXT | |
| user_rating | INTEGER 1–10 | |
| intro_start_ms | INTEGER | Intro-skipper; NULL = no data |
| intro_end_ms | INTEGER | |
| credits_start_ms | INTEGER | |
| credits_end_ms | INTEGER | |
| intro_analysis_at | TEXT | When last analysed (distinguishes unanalysed from analysed-and-nothing-found) |

UNIQUE(`series_id`, `episode_number`). No `status` column — derived
the same way as movies.

### Media

A physical file on disk, belonging to one movie or one or more
episodes (via `media_episode`).

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| movie_id | INTEGER FK → movie(id) | Nullable; ON DELETE SET NULL |
| file_path | TEXT NOT NULL | Absolute path in library |
| relative_path | TEXT NOT NULL | Relative to library root |
| size | INTEGER NOT NULL | Bytes |
| container | TEXT | mkv, mp4, avi |
| resolution | INTEGER | 480, 720, 1080, 2160 |
| source | TEXT | bluray, webdl, webrip, hdtv, cam, telesync, dvd |
| video_codec | TEXT | h264, h265, av1, mpeg2, vp9, xvid |
| audio_codec | TEXT | Primary audio codec |
| hdr_format | TEXT | sdr, hdr10, hdr10plus, dolby_vision, hlg |
| is_remux | INTEGER DEFAULT 0 | |
| is_proper | INTEGER DEFAULT 0 | |
| is_repack | INTEGER DEFAULT 0 | |
| scene_name | TEXT | |
| release_group | TEXT | |
| release_hash | TEXT | |
| runtime_ticks | INTEGER | Duration for playback |
| date_added | TEXT NOT NULL | |
| original_file_path | TEXT | Path before rename/import |
| indexer_flags | TEXT | JSON: freeleech, internal, scene |
| trickplay_generated | INTEGER DEFAULT 0 | Seek thumbnails present |

### Stream

A track within a media file — video, audio, or subtitle.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| media_id | INTEGER NOT NULL FK | CASCADE |
| stream_index | INTEGER NOT NULL | Position in file, or assigned for external |
| stream_type | TEXT NOT NULL | video, audio, subtitle |
| codec | TEXT | h264, aac, srt, ass, pgs, etc. |
| language | TEXT | ISO 639-1 |
| title | TEXT | Track title (e.g. "Director's Commentary") |
| is_external | INTEGER DEFAULT 0 | Sidecar file |
| is_default | INTEGER DEFAULT 0 | |
| is_forced | INTEGER DEFAULT 0 | |
| is_hearing_impaired | INTEGER DEFAULT 0 | |
| path | TEXT | Filesystem path — external files only |
| bitrate | INTEGER | bps |
| width | INTEGER | video |
| height | INTEGER | video |
| framerate | REAL | video |
| pixel_format | TEXT | video |
| color_space | TEXT | video |
| color_transfer | TEXT | video |
| color_primaries | TEXT | video |
| hdr_format | TEXT | video |
| channels | INTEGER | audio |
| channel_layout | TEXT | audio |
| sample_rate | INTEGER | audio |
| bit_depth | INTEGER | audio |

### MediaEpisode

Join table for double-episode files (S01E01E02 in one file → two rows).

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| media_id | INTEGER NOT NULL FK | CASCADE |
| episode_id | INTEGER NOT NULL FK | CASCADE |

UNIQUE(`media_id`, `episode_id`). Movies use `media.movie_id` directly
(always one-to-one).

### Download

A torrent, linked to one or many movies/episodes via
`download_content`.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| release_id | INTEGER FK → release(id) | The release that was grabbed |
| torrent_hash | TEXT | |
| title | TEXT NOT NULL | |
| state | TEXT NOT NULL DEFAULT 'queued' | see states below |
| size | INTEGER | |
| downloaded | INTEGER DEFAULT 0 | |
| uploaded | INTEGER DEFAULT 0 | |
| download_speed | INTEGER DEFAULT 0 | Bytes/s |
| upload_speed | INTEGER DEFAULT 0 | Bytes/s |
| seeders | INTEGER | |
| leechers | INTEGER | |
| eta | INTEGER | Seconds remaining |
| added_at | TEXT NOT NULL | |
| completed_at | TEXT | |
| output_path | TEXT | |
| magnet_url | TEXT | |
| error_message | TEXT | |
| seed_target_reached_at | TEXT | For ratio-then-cleanup policy |

States: `searching`, `queued`, `grabbing`, `downloading`, `paused`,
`stalled`, `completed`, `importing`, `imported`, `failed`, `seeding`,
`cleaned_up`.

### DownloadContent

Links a download to the movies/episodes it delivers (one row per
movie; N rows for a season pack).

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| download_id | INTEGER NOT NULL FK | CASCADE |
| movie_id | INTEGER FK | Nullable |
| episode_id | INTEGER FK | Nullable |

CHECK(`movie_id` IS NOT NULL OR `episode_id` IS NOT NULL).

### Release

A search result from a Torznab indexer.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| guid | TEXT NOT NULL | Indexer-assigned ID |
| indexer_id | INTEGER FK | |
| movie_id | INTEGER FK | Nullable |
| show_id | INTEGER FK | Nullable |
| season_number | INTEGER | Season pack |
| episode_id | INTEGER FK | Nullable |
| title | TEXT NOT NULL | Release title |
| size | INTEGER | Bytes |
| download_url | TEXT | |
| magnet_url | TEXT | |
| info_url | TEXT | |
| info_hash | TEXT | |
| publish_date | TEXT | |
| seeders | INTEGER | |
| leechers | INTEGER | |
| grabs | INTEGER | |
| resolution | INTEGER | Parsed |
| source | TEXT | Parsed |
| video_codec | TEXT | Parsed |
| audio_codec | TEXT | Parsed |
| hdr_format | TEXT | Parsed |
| is_remux | INTEGER DEFAULT 0 | |
| is_proper | INTEGER DEFAULT 0 | |
| is_repack | INTEGER DEFAULT 0 | |
| release_group | TEXT | |
| languages | TEXT | JSON array |
| indexer_flags | TEXT | JSON |
| quality_score | INTEGER | From profile |
| status | TEXT NOT NULL DEFAULT 'available' | `available`, `pending`, `grabbed`, `rejected` |
| pending_until | TEXT | Delay-grab window |
| first_seen_at | TEXT NOT NULL | |
| grabbed_at | TEXT | |

UNIQUE (`episode_id`, `indexer_id`, `guid`) WHERE `episode_id` IS NOT NULL.
UNIQUE (`movie_id`, `indexer_id`, `guid`) WHERE `movie_id` IS NOT NULL.

### Blocklist

Releases that should never be grabbed again.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| movie_id | INTEGER FK | Nullable |
| episode_id | INTEGER FK | Nullable |
| source_title | TEXT NOT NULL | |
| torrent_info_hash | TEXT | |
| indexer_id | INTEGER FK | |
| size | INTEGER | |
| resolution | INTEGER | |
| source | TEXT | |
| video_codec | TEXT | |
| message | TEXT | Reason for blocklisting |
| date | TEXT NOT NULL | |

### History

Event log — powers the activity feed and webhook triggers.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| movie_id | INTEGER FK | Nullable |
| episode_id | INTEGER FK | Nullable |
| event_type | TEXT NOT NULL | see AppEvent variants |
| date | TEXT NOT NULL | |
| source_title | TEXT | Release/file name |
| quality | TEXT | Resolution + source description |
| download_id | TEXT | Correlates grab → import |
| data | TEXT | JSON AppEvent blob |

### LogEntry

Persistent tracing log store. Every tracing event (INFO+ by default)
lands here via a batched mpsc writer. Row-capped retention (default
100k) handled by a scheduler task.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| ts_us | INTEGER NOT NULL | Unix micros |
| level | INTEGER NOT NULL | 0=ERROR … 4=TRACE |
| target | TEXT NOT NULL | |
| subsystem | TEXT | First `kino::` segment |
| trace_id | TEXT | |
| span_id | TEXT | |
| message | TEXT NOT NULL | |
| fields_json | TEXT | Structured fields |
| source | TEXT NOT NULL DEFAULT 'backend' | `backend` / `frontend` |

STRICT table. Indexed on `(ts_us DESC)`, `(level, ts_us DESC)`,
`(subsystem, ts_us DESC)`, and `trace_id` (partial).

---

## Configuration entities

### Config

Single-row table (`id = 1`). All system-level settings. See
`backend/migrations/20260328000001_initial_schema.sql` for the full
CREATE and defaults; grouped highlights:

- **Server:** `listen_address`, `listen_port`, `api_key`, `base_url`
- **Storage:** `data_path`, `media_library_path`, `download_path`
- **VPN (userspace WireGuard via boringtun):** `vpn_enabled`,
  `vpn_private_key`, `vpn_address`, `vpn_server_public_key`,
  `vpn_server_endpoint`, `vpn_dns`, `vpn_port_forward_provider`
  (`none` / `natpmp` / `airvpn`; PIA deferred —
  see `../subsystems/03-download.md`), `vpn_port_forward_api_key`
- **External APIs:** `tmdb_api_key`, `opensubtitles_api_key`,
  `opensubtitles_username`, `opensubtitles_password`
- **Downloads:** `max_concurrent_downloads`, `download_speed_limit`,
  `upload_speed_limit`, `seed_ratio_limit`, `seed_time_limit`
- **Media server:** `transcoding_enabled`, `ffmpeg_path`,
  `hw_acceleration` (`none` / `vaapi` / `nvenc` / `qsv`),
  `max_concurrent_transcodes`, `cast_receiver_app_id`
- **Library management:** `auto_cleanup_enabled`,
  `auto_cleanup_movie_delay` (hours), `auto_cleanup_episode_delay`
  (hours), `auto_upgrade_enabled`, `auto_search_interval` (minutes),
  `stall_timeout` (minutes),
  `dead_timeout` (minutes), `low_disk_threshold_gb`
- **File management:** `use_hardlinks`, `movie_naming_format`,
  `episode_naming_format`, `multi_episode_naming_format`,
  `season_folder_format`
- **Trakt (subsystem 16):** `trakt_client_id`, `trakt_client_secret`,
  `trakt_scrobble`, `trakt_sync_watched`, `trakt_sync_ratings`,
  `trakt_sync_watchlist`, `trakt_sync_collection`
- **Lists (subsystem 17):** `mdblist_api_key`,
  `list_bulk_growth_threshold`
- **Intro-skipper (subsystem 15):** `intro_detect_enabled`,
  `credits_detect_enabled`, `auto_skip_intros` (`off` / `on` /
  `smart`), `auto_skip_credits`, `intro_min_length_s`,
  `intro_analysis_limit_s`, `credits_analysis_limit_s`,
  `intro_match_score_threshold`, `max_concurrent_intro_analyses`

### UserPreferences

Display / layout state. Split from `config` so UI tweaks evolve
independently of system config. Single row (`id = 1`). See
`docs/subsystems/18-ui-customisation.md`.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | Always 1 |
| home_hero_enabled | INTEGER DEFAULT 1 | |
| home_section_order | TEXT DEFAULT '[]' | JSON array of section IDs |
| home_section_hidden | TEXT DEFAULT '[]' | JSON array of section IDs |
| greeting_name | TEXT | |
| updated_at | TEXT NOT NULL | |

### Indexer

Torznab / Cardigann endpoints with health tracking.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| name | TEXT NOT NULL | |
| url | TEXT NOT NULL DEFAULT '' | Torznab base URL (empty for Cardigann) |
| api_key | TEXT | |
| priority | INTEGER DEFAULT 25 | Lower = preferred |
| enabled | INTEGER DEFAULT 1 | |
| supports_rss | INTEGER DEFAULT 1 | |
| supports_search | INTEGER DEFAULT 1 | |
| supported_categories | TEXT | JSON array of Torznab category IDs |
| supported_search_params | TEXT | JSON array |
| initial_failure_time | TEXT | |
| most_recent_failure_time | TEXT | |
| escalation_level | INTEGER DEFAULT 0 | Exponential backoff |
| disabled_until | TEXT | |
| indexer_type | TEXT NOT NULL DEFAULT 'torznab' | `torznab` / `cardigann` |
| definition_id | TEXT | Cardigann YAML definition ID |
| settings_json | TEXT | Cardigann per-site settings |

### WebhookTarget

Generic webhook endpoints. Event filters are independent booleans.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| name | TEXT NOT NULL | |
| url | TEXT NOT NULL | |
| method | TEXT DEFAULT 'POST' | |
| headers | TEXT | JSON object |
| body_template | TEXT | JSON template with placeholders |
| on_grab | INTEGER DEFAULT 1 | |
| on_download_complete | INTEGER DEFAULT 1 | |
| on_import | INTEGER DEFAULT 1 | |
| on_upgrade | INTEGER DEFAULT 1 | |
| on_failure | INTEGER DEFAULT 1 | |
| on_watched | INTEGER DEFAULT 0 | |
| on_health_issue | INTEGER DEFAULT 1 | |
| enabled | INTEGER DEFAULT 1 | |
| initial_failure_time | TEXT | |
| most_recent_failure_time | TEXT | |
| disabled_until | TEXT | |

### QualityProfile

Quality preferences — what to accept, how to rank, when to stop
upgrading.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| name | TEXT NOT NULL | |
| upgrade_allowed | INTEGER DEFAULT 1 | |
| cutoff | TEXT NOT NULL | `quality_id` to stop upgrading at |
| items | TEXT NOT NULL | JSON array of quality tiers |
| accepted_languages | TEXT DEFAULT '["en"]' | JSON array, ordered |
| is_default | INTEGER DEFAULT 0 | Exactly one row = 1 |

**Quality tiers** (stored as JSON array in `items`): each entry has
`name`, `quality_id`, `allowed`, and `rank` (higher = better).

```json
[
  { "quality_id": "remux_2160p",  "name": "Remux 2160p",   "allowed": true,  "rank": 18 },
  { "quality_id": "bluray_2160p", "name": "Bluray 2160p",  "allowed": true,  "rank": 17 },
  { "quality_id": "web_2160p",    "name": "WEB 2160p",     "allowed": true,  "rank": 16 },
  { "quality_id": "hdtv_2160p",   "name": "HDTV 2160p",    "allowed": true,  "rank": 15 },
  { "quality_id": "remux_1080p",  "name": "Remux 1080p",   "allowed": true,  "rank": 14 },
  { "quality_id": "bluray_1080p", "name": "Bluray 1080p",  "allowed": true,  "rank": 13 },
  { "quality_id": "web_1080p",    "name": "WEB 1080p",     "allowed": true,  "rank": 12 },
  { "quality_id": "hdtv_1080p",   "name": "HDTV 1080p",    "allowed": true,  "rank": 11 },
  { "quality_id": "bluray_720p",  "name": "Bluray 720p",   "allowed": true,  "rank": 10 },
  { "quality_id": "web_720p",     "name": "WEB 720p",      "allowed": true,  "rank": 9 },
  { "quality_id": "hdtv_720p",    "name": "HDTV 720p",     "allowed": true,  "rank": 8 },
  { "quality_id": "bluray_480p",  "name": "Bluray 480p",   "allowed": false, "rank": 7 },
  { "quality_id": "web_480p",     "name": "WEB 480p",      "allowed": false, "rank": 6 },
  { "quality_id": "dvd",          "name": "DVD",           "allowed": false, "rank": 5 },
  { "quality_id": "sdtv",         "name": "SDTV",          "allowed": false, "rank": 4 },
  { "quality_id": "telecine",     "name": "Telecine",      "allowed": false, "rank": 3 },
  { "quality_id": "telesync",     "name": "Telesync",      "allowed": false, "rank": 2 },
  { "quality_id": "cam",          "name": "CAM",           "allowed": false, "rank": 1 }
]
```

WEB tiers group WEBDL and WEBRip. A default profile ships on first
run; movies/shows reference it via `quality_profile_id`.

### SchedulerState

Persists last-run timestamps for background tasks.

| Column | Type | Notes |
|--------|------|-------|
| task_name | TEXT PK | e.g. `wanted_search`, `metadata_refresh`, `cleanup` |
| last_run_at | TEXT | |

---

## Trakt integration (subsystem 16)

### TraktAuth

OAuth tokens + user identity. Single row (`id = 1`).

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | Always 1 |
| access_token | TEXT NOT NULL | |
| refresh_token | TEXT NOT NULL | |
| expires_at | TEXT NOT NULL | Not trusted blindly — HTTP client refreshes on any 401 |
| token_scope | TEXT DEFAULT 'public' | |
| connected_at | TEXT NOT NULL | |
| username | TEXT | |
| slug | TEXT | |

### TraktSyncState

Incremental-sync watermarks + recommendation / trending caches.
Single row.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | Always 1 |
| last_watched_movies_at | TEXT | |
| last_watched_episodes_at | TEXT | |
| last_rated_movies_at | TEXT | |
| last_rated_shows_at | TEXT | |
| last_rated_episodes_at | TEXT | |
| last_watchlist_movies_at | TEXT | |
| last_watchlist_shows_at | TEXT | |
| last_collection_movies_at | TEXT | |
| last_collection_episodes_at | TEXT | |
| last_playback_at | TEXT | |
| initial_import_done | INTEGER DEFAULT 0 | |
| last_full_sync_at | TEXT | |
| last_incremental_sync_at | TEXT | |
| recommendations_cached_at | TEXT | |
| recommendations_json | TEXT DEFAULT '[]' | |
| trending_cached_at | TEXT | |
| trending_json | TEXT DEFAULT '[]' | |

### TraktScrobbleQueue

Offline scrobble events, drained by `trakt_scrobble_drain` scheduler
task. `stop` events beyond the scrobble window convert to
`/sync/history` backfills; stale `start` / `pause` are dropped.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| created_at | TEXT NOT NULL | |
| action | TEXT NOT NULL | `start` / `pause` / `stop` |
| kind | TEXT NOT NULL | `movie` / `episode` |
| movie_id | INTEGER FK | Nullable |
| episode_id | INTEGER FK | Nullable |
| progress | REAL NOT NULL | |
| attempts | INTEGER DEFAULT 0 | |
| last_error | TEXT | |
| last_attempt_at | TEXT | |

CHECK: exactly one of `movie_id` / `episode_id` is set.

---

## Lists (subsystem 17)

### List

Any external URL that resolves to a set of TMDB IDs. Sources:
MDBList, TMDB lists, Trakt custom lists, Trakt watchlist. The Trakt
watchlist is a *system list* (`is_system = 1`) auto-created on Trakt
connect and removed on disconnect.

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| source_type | TEXT NOT NULL | `mdblist` / `tmdb` / `trakt` / `trakt_watchlist` |
| source_url | TEXT NOT NULL | |
| source_id | TEXT NOT NULL | Per-source identifier |
| title | TEXT NOT NULL | |
| description | TEXT | |
| item_count | INTEGER DEFAULT 0 | |
| item_type | TEXT DEFAULT 'mixed' | `movie` / `show` / `mixed` |
| last_polled_at | TEXT | |
| last_poll_status | TEXT | |
| consecutive_poll_failures | INTEGER DEFAULT 0 | |
| is_system | INTEGER DEFAULT 0 | |
| created_at | TEXT NOT NULL | |

UNIQUE (`source_type`, `source_id`). Home pinning / ordering lives in
`user_preferences.home_section_order` under pseudo-IDs of the form
`list:<id>` — no mirrored state to drift.

### ListItem

| Column | Type | Notes |
|--------|------|-------|
| id | INTEGER PK | |
| list_id | INTEGER NOT NULL FK | CASCADE |
| tmdb_id | INTEGER NOT NULL | |
| item_type | TEXT NOT NULL | `movie` / `show` |
| title | TEXT NOT NULL | |
| poster_path | TEXT | |
| position | INTEGER | |
| added_at | TEXT NOT NULL | |
| ignored_by_user | INTEGER DEFAULT 0 | |

UNIQUE (`list_id`, `tmdb_id`, `item_type`).

---

## Relationships

```
QualityProfile 1 → N  Movie
QualityProfile 1 → N  Show

Show           1 → N  Series
Series         1 → N  Episode
Show           1 → N  Episode    (denormalised show_id on episode)

Movie          1 → N  Media         (usually 0 or 1, briefly 2 during upgrade)
Episode        N → N  Media         (via MediaEpisode — handles multi-episode files)

Media          1 → N  Stream

Download       N → 1  Release
Download       N → N  Movie/Episode (via DownloadContent)

Release        N → 1  Indexer
Release        N → 1  Movie | (Show + season) | Episode

Blocklist      N → 1  Movie | Episode
Blocklist      N → 1  Indexer

History        N → 1  Movie | Episode

List           1 → N  ListItem
TraktScrobbleQueue N → 1 Movie | Episode
```

---

## Notes

### Phase derivation

Movies and episodes don't store a `status` column. Phase is derived on
read via a `CASE` over `media`, `download_content.state`, and
`watched_at`. The canonical derivation is in `services/phase.rs` and
surfaces as strings — `wanted` / `downloading` / `available` /
`watched` — through the API and frontend.

### Quality parsing

Quality fields on `media` and `release` are parsed from the release
title using a release-name parser. The same parser runs on search
results and imported files. `release.quality_score` is computed at
search time against the applicable quality profile and persisted so
upgrade comparisons don't re-parse.

### Monitor axes on episodes

`acquire` and `in_scope` usually move together, but diverge in:

- **Play-auto-follow:** `acquire=0`, `in_scope=1` — scheduler stays
  quiet but Next Up still works across the series.
- **Follow with Latest-season-only:** `acquire=0`, `in_scope=0` for
  excluded seasons — scheduler skips and Next Up ignores.
- **External library cohabitation:** user might mark S1–S3 as
  `acquire=0, in_scope=1` because they're on another server, while
  letting kino acquire S4+.

### SQLite

- WAL mode, `busy_timeout = 5000 ms`.
- Migrations embedded via `sqlx` and run on startup.
- `foreign_keys = ON`.

### Security

Secret-bearing config fields (`api_key`, `vpn_private_key`,
`opensubtitles_password`, etc.) are stored in plain text. The database
file is expected to be protected by filesystem permissions and backup
policy on the host; kino does not encrypt at rest. Values are returned
verbatim from `GET /api/v1/config` — clients that shouldn't see them
shouldn't have the API key in the first place. HTTPS is recommended
for any non-localhost access (terminate via reverse proxy).
