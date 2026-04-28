-- kino initial schema — consolidated single-file canonical form.
-- Early-stage dev means we rewrite rather than accumulate ALTER
-- migrations; any prior kino install clears its DB via `just reset`.

PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;

-- ============================================================
-- Configuration entities
-- ============================================================

CREATE TABLE IF NOT EXISTS config (
    id                          INTEGER PRIMARY KEY CHECK (id = 1),
    -- server
    listen_address              TEXT    NOT NULL DEFAULT '0.0.0.0',
    listen_port                 INTEGER NOT NULL DEFAULT 8080,
    api_key                     TEXT    NOT NULL,
    base_url                    TEXT    NOT NULL DEFAULT '',
    -- storage
    data_path                   TEXT    NOT NULL,
    media_library_path          TEXT    NOT NULL DEFAULT '',
    download_path               TEXT    NOT NULL DEFAULT '',
    -- vpn
    vpn_enabled                 INTEGER NOT NULL DEFAULT 0,
    vpn_private_key             TEXT,
    vpn_address                 TEXT,
    vpn_server_public_key       TEXT,
    vpn_server_endpoint         TEXT,
    vpn_dns                     TEXT,
    vpn_port_forward_provider   TEXT    NOT NULL DEFAULT 'none',
    vpn_port_forward_api_key    TEXT,
    -- VPN killswitch (subsystem 33). When VPN is on AND this is on, a
    -- handshake-loss event soft-pauses every active download with a
    -- `paused_by_killswitch` marker; on successful reconnect those
    -- downloads automatically resume. Default on so a user who enables
    -- the VPN gets the protection without a second toggle. Ignored
    -- when vpn_enabled = 0.
    vpn_killswitch_enabled      INTEGER NOT NULL DEFAULT 1,
    -- Subsystem 33 Phase B: URL the periodic IP-leak self-test hits.
    -- Compared against the VPN endpoint's resolved IP. Self-hostable
    -- by users running their own probe (e.g. an internal `whoami`
    -- echo service) for air-gapped or paranoid setups.
    vpn_killswitch_check_url    TEXT    NOT NULL DEFAULT 'https://api.ipify.org',
    -- external apis
    tmdb_api_key                TEXT    NOT NULL DEFAULT '',
    opensubtitles_api_key       TEXT,
    opensubtitles_username      TEXT,
    opensubtitles_password      TEXT,
    -- downloads
    max_concurrent_downloads    INTEGER NOT NULL DEFAULT 3,
    download_speed_limit        INTEGER NOT NULL DEFAULT 0,
    upload_speed_limit          INTEGER NOT NULL DEFAULT 0,
    seed_ratio_limit            REAL    NOT NULL DEFAULT 1.0,
    seed_time_limit             INTEGER NOT NULL DEFAULT 0,
    -- media server
    transcoding_enabled         INTEGER NOT NULL DEFAULT 1,
    ffmpeg_path                 TEXT    NOT NULL DEFAULT 'ffmpeg',
    hw_acceleration             TEXT    NOT NULL DEFAULT 'none',
    max_concurrent_transcodes   INTEGER NOT NULL DEFAULT 2,
    cast_receiver_app_id        TEXT,
    -- Cardigann definitions cache freshness — written by the
    -- `definitions_refresh` scheduler task + the manual refresh
    -- endpoint on success. NULL when the binary has booted but
    -- no refresh has ever completed (first-run state); the
    -- setup wizard treats NULL as "definitions not yet
    -- downloaded" and surfaces a download CTA.
    definitions_last_refreshed_at TEXT,
    -- Explicit user consent for the indexer-catalogue auto-
    -- refresh. The catalogue lives in the third-party
    -- Prowlarr/Indexers GitHub repo; kino must NEVER reach out
    -- to it without an explicit user signal. The first manual
    -- click on "Download catalogue" sets this to 1 — that's the
    -- consent. The daily `definitions_refresh` scheduler task
    -- short-circuits when this is 0 (i.e., the user has never
    -- asked us to fetch). Settings → Indexers will eventually
    -- surface a toggle so users can opt back out.
    definitions_auto_refresh_enabled INTEGER NOT NULL DEFAULT 0,
    -- library management
    auto_cleanup_enabled        INTEGER NOT NULL DEFAULT 1,
    auto_cleanup_movie_delay    INTEGER NOT NULL DEFAULT 72,
    auto_cleanup_episode_delay  INTEGER NOT NULL DEFAULT 72,
    auto_upgrade_enabled        INTEGER NOT NULL DEFAULT 1,
    auto_search_interval        INTEGER NOT NULL DEFAULT 15,
    -- Metadata refresh cadence is per-row tiered in SQL (see
    -- `services::metadata::refresh_sweep`: 1h hot / 72h cold) with
    -- a fixed 30-min scheduler tick. The old single-knob
    -- `metadata_refresh_interval` column was dropped when tiering
    -- landed — no per-install override needed.
    stall_timeout               INTEGER NOT NULL DEFAULT 30,
    dead_timeout                INTEGER NOT NULL DEFAULT 60,
    -- file management
    use_hardlinks               INTEGER NOT NULL DEFAULT 1,
    movie_naming_format         TEXT    NOT NULL DEFAULT '{title} ({year}) [{quality}]',
    episode_naming_format       TEXT    NOT NULL DEFAULT '{show} - S{season:00}E{episode:00} - {title} [{quality}]',
    multi_episode_naming_format TEXT    NOT NULL DEFAULT '{show} - S{season:00}E{episode:00}E{episode_end:00} - {title} [{quality}]',
    season_folder_format        TEXT    NOT NULL DEFAULT 'Season {season:00}',
    -- Trakt integration (docs/subsystems/16-trakt.md § Configuration)
    -- Credentials: user registers an app at trakt.tv/oauth/applications
    -- with "urn:ietf:wg:oauth:2.0:oob" as the redirect URL (device-code
    -- grant). These are per-install, not baked into the binary.
    trakt_client_id             TEXT,
    trakt_client_secret         TEXT,
    -- Feature toggles. Each is independently switchable from the
    -- Integrations settings page, honouring the spec's "compose your
    -- own sync" model. Defaults are "on" across the board — if the
    -- user connected Trakt they almost certainly want the full
    -- sync; forcing them to flip five extra switches after connect
    -- is needless friction. Watchlist is safe-on because it only
    -- marks *already-in-library* items as monitored; it never
    -- triggers discovery of titles the user hasn't added.
    trakt_scrobble              INTEGER NOT NULL DEFAULT 1,
    trakt_sync_watched          INTEGER NOT NULL DEFAULT 1,
    trakt_sync_ratings          INTEGER NOT NULL DEFAULT 1,
    trakt_sync_watchlist        INTEGER NOT NULL DEFAULT 1,
    trakt_sync_collection       INTEGER NOT NULL DEFAULT 1,
    -- Incremental-sync bucket flags for the two surfaces users commonly
    -- want to opt out of independently: resume-point mirroring
    -- (Trakt.play_progress → kino.resume_position) and recommendations
    -- refresh (Trakt.recommendations → Home row cache). Spec §3.
    trakt_resume_sync_enabled   INTEGER NOT NULL DEFAULT 1,
    trakt_recommendations_enabled INTEGER NOT NULL DEFAULT 1,
    -- lists (subsystem 17) — MDBList requires a user-provided API key
    -- (TMDB lists reuse our key, Trakt lists reuse OAuth).
    mdblist_api_key             TEXT,
    -- Threshold for bulk-growth notification: if apply_poll adds more
    -- than this many items in a single poll, fire a notification.
    list_bulk_growth_threshold  INTEGER NOT NULL DEFAULT 20,
    -- Intro-skipper (subsystem 15). Detection limits are in seconds,
    -- thresholds in their natural units. auto_skip_intros is a mode:
    -- 'off' / 'on' / 'smart' (show button on first ep of a season,
    -- auto-skip the rest).
    intro_detect_enabled        INTEGER NOT NULL DEFAULT 1,
    credits_detect_enabled      INTEGER NOT NULL DEFAULT 1,
    auto_skip_intros            TEXT    NOT NULL DEFAULT 'smart',
    auto_skip_credits           INTEGER NOT NULL DEFAULT 0,
    intro_min_length_s          INTEGER NOT NULL DEFAULT 15,
    intro_analysis_limit_s      INTEGER NOT NULL DEFAULT 600,
    credits_analysis_limit_s    INTEGER NOT NULL DEFAULT 450,
    intro_match_score_threshold REAL    NOT NULL DEFAULT 10.0,
    max_concurrent_intro_analyses INTEGER NOT NULL DEFAULT 2,
    -- HMAC secret for short-lived signed media URLs (cross-origin
    -- deploys where cookies aren't available). Lazily initialised on
    -- first `/sign-url` call when NULL — see `session::signing_secret`.
    -- Distinct from `api_key` so rotating the master credential
    -- doesn't break in-flight signed URLs.
    session_signing_key         TEXT,
    -- Per-install low-disk warning threshold (GB). Surfaced in /status
    -- and the health banner when free space at the download path
    -- drops below this.
    low_disk_threshold_gb       INTEGER NOT NULL DEFAULT 5,
    -- mDNS responder (subsystem 25). Advertises {hostname}.local +
    -- _http._tcp on the LAN so users reach kino at e.g.
    -- http://kino.local:8080 without knowing the host's IP. Default
    -- on; users on networks where mDNS would conflict with another
    -- responder can flip it off.
    mdns_enabled                INTEGER NOT NULL DEFAULT 1,
    mdns_hostname               TEXT    NOT NULL DEFAULT 'kino',
    mdns_service_name           TEXT    NOT NULL DEFAULT 'Kino',
    -- Backup & restore (subsystem 19). Default-on so kino has a
    -- safety net out of the box; the schedule is preset-driven from
    -- the UI (`daily` / `weekly` / `monthly` / `off`) with the time
    -- field separate. `cron` is reserved for a future advanced-mode
    -- escape hatch.
    backup_enabled              INTEGER NOT NULL DEFAULT 1,
    backup_schedule             TEXT    NOT NULL DEFAULT 'daily',
    backup_time                 TEXT    NOT NULL DEFAULT '03:00',
    -- Where archives land. Defaults to `{data_path}/backups/`,
    -- which `init::ensure_defaults` populates on first boot when
    -- `data_path` is known.
    backup_location_path        TEXT    NOT NULL DEFAULT '',
    -- Scheduled-kind backups beyond this count are pruned after each
    -- successful new backup. Manual + pre-restore backups are exempt.
    backup_retention_count      INTEGER NOT NULL DEFAULT 7
);

-- Display / layout preferences the user sets through the UI. Split
-- from `config` (which holds system-level settings tied to the binary:
-- paths, API keys, timeouts, codec choices) so we can evolve one
-- without touching the other. Single row enforced the same way as
-- `config` — kino is single-user by design. See
-- `docs/subsystems/18-ui-customisation.md` § Schema.
CREATE TABLE IF NOT EXISTS user_preferences (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    home_hero_enabled    INTEGER NOT NULL DEFAULT 1,
    -- JSON arrays of section IDs (strings). The server treats unknown
    -- IDs as harmless no-ops so a future Kino version that removes a
    -- row doesn't need a migration.
    home_section_order   TEXT    NOT NULL DEFAULT '[]',
    home_section_hidden  TEXT    NOT NULL DEFAULT '[]',
    greeting_name        TEXT,
    updated_at           TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS quality_profile (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    name               TEXT    NOT NULL,
    upgrade_allowed    INTEGER NOT NULL DEFAULT 1,
    cutoff             TEXT    NOT NULL,
    items              TEXT    NOT NULL,
    accepted_languages TEXT    NOT NULL DEFAULT '["en"]',
    -- Exactly one row has is_default=1. The API handler clears the
    -- flag on others when a new default is set; relying on "id = 1"
    -- as "default" was brittle after renames/reordering.
    is_default         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS indexer (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    name                     TEXT    NOT NULL,
    url                      TEXT    NOT NULL DEFAULT '',
    api_key                  TEXT,
    priority                 INTEGER NOT NULL DEFAULT 25,
    enabled                  INTEGER NOT NULL DEFAULT 1,
    supports_rss             INTEGER NOT NULL DEFAULT 1,
    supports_search          INTEGER NOT NULL DEFAULT 1,
    supported_categories     TEXT,
    supported_search_params  TEXT,
    initial_failure_time     TEXT,
    most_recent_failure_time TEXT,
    escalation_level         INTEGER NOT NULL DEFAULT 0,
    disabled_until           TEXT,
    indexer_type             TEXT    NOT NULL DEFAULT 'torznab',
    definition_id            TEXT,
    settings_json            TEXT
);

CREATE TABLE IF NOT EXISTS webhook_target (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    name                     TEXT    NOT NULL,
    url                      TEXT    NOT NULL,
    method                   TEXT    NOT NULL DEFAULT 'POST',
    headers                  TEXT,
    body_template            TEXT,
    on_grab                  INTEGER NOT NULL DEFAULT 1,
    on_download_complete     INTEGER NOT NULL DEFAULT 1,
    on_import                INTEGER NOT NULL DEFAULT 1,
    on_upgrade               INTEGER NOT NULL DEFAULT 1,
    on_failure               INTEGER NOT NULL DEFAULT 1,
    on_watched               INTEGER NOT NULL DEFAULT 0,
    on_health_issue          INTEGER NOT NULL DEFAULT 1,
    enabled                  INTEGER NOT NULL DEFAULT 1,
    initial_failure_time     TEXT,
    most_recent_failure_time TEXT,
    -- Retry ladder position. 0 = healthy; 1..=5 = rungs on the
    -- 30s → 15min → 1h → 4h → 24h backoff. Cleared on successful
    -- delivery so a recovered target re-enters the ladder from
    -- the bottom rather than staying pinned at a long backoff.
    escalation_level         INTEGER NOT NULL DEFAULT 0,
    disabled_until           TEXT
);

CREATE TABLE IF NOT EXISTS scheduler_state (
    task_name   TEXT PRIMARY KEY,
    last_run_at TEXT
);

-- Per-device authentication sessions. The master credential lives on
-- `config.api_key` (issued at first boot, rotatable from Settings);
-- everything else — browser cookies, named CLI tokens, QR-code
-- bootstrap exchanges — issues a row here. That gives us per-device
-- visibility ("this is my Firefox on PopOS, last seen 3 days ago")
-- and surgical revocation: lose your phone, kill just that row,
-- the rest of your devices keep working.
--
-- `id` is the cookie value (or the bearer-style token returned to a
-- CLI client). It's a 32-byte URL-safe random string — long enough
-- to brute-force-resist, but ALSO compared in constant time at the
-- middleware layer.
--
-- `source` distinguishes how the session was created so the Devices
-- page can render different icons / labels. `bootstrap-pending`
-- rows are short-lived QR-code tokens awaiting redemption.
--
-- `consumed_at` is set on `bootstrap-pending` rows the moment they're
-- redeemed; the row stays around for audit but `consumed_at NOT NULL`
-- gates re-redemption to prevent replay.
--
-- `last_seen_at` is touched on every authed request — cheap UPDATE
-- against the PRIMARY KEY index. Drives the "last seen" column on
-- the Devices page and lets us prune long-inactive sessions.
CREATE TABLE IF NOT EXISTS session (
    id              TEXT PRIMARY KEY,
    label           TEXT NOT NULL,
    user_agent      TEXT,
    ip              TEXT,
    source          TEXT NOT NULL CHECK (source IN ('browser', 'cli', 'qr-bootstrap', 'bootstrap-pending', 'auto-localhost')),
    created_at      TEXT NOT NULL,
    last_seen_at    TEXT NOT NULL,
    expires_at      TEXT NOT NULL,
    consumed_at     TEXT
);

CREATE INDEX IF NOT EXISTS idx_session_expires_at ON session(expires_at);
CREATE INDEX IF NOT EXISTS idx_session_source ON session(source);

-- ============================================================
-- Domain entities
-- ============================================================

CREATE TABLE IF NOT EXISTS movie (
    id                               INTEGER PRIMARY KEY AUTOINCREMENT,
    tmdb_id                          INTEGER UNIQUE NOT NULL,
    imdb_id                          TEXT,
    tvdb_id                          INTEGER,
    title                            TEXT    NOT NULL,
    original_title                   TEXT,
    overview                         TEXT,
    tagline                          TEXT,
    year                             INTEGER,
    runtime                          INTEGER,
    release_date                     TEXT,
    physical_release_date            TEXT,
    digital_release_date             TEXT,
    certification                    TEXT,
    poster_path                      TEXT,
    backdrop_path                    TEXT,
    genres                           TEXT,
    tmdb_rating                      REAL,
    tmdb_vote_count                  INTEGER,
    popularity                       REAL,
    original_language                TEXT,
    collection_tmdb_id               INTEGER,
    collection_name                  TEXT,
    youtube_trailer_id               TEXT,
    quality_profile_id               INTEGER NOT NULL REFERENCES quality_profile(id),
    monitored                        INTEGER NOT NULL DEFAULT 1,
    added_at                         TEXT    NOT NULL,
    blurhash_poster                  TEXT,
    blurhash_backdrop                TEXT,
    playback_position_ticks          INTEGER NOT NULL DEFAULT 0,
    play_count                       INTEGER NOT NULL DEFAULT 0,
    last_played_at                   TEXT,
    watched_at                       TEXT,
    preferred_audio_stream_index     INTEGER,
    preferred_subtitle_stream_index  INTEGER,
    last_metadata_refresh            TEXT,
    last_searched_at                 TEXT,
    -- 1..10 Trakt rating scale. NULL = unrated. Source-agnostic: set
    -- by the user in kino or mirrored down from Trakt sync; both
    -- directions update this column and (when sync-ratings is on) the
    -- other side.
    user_rating                      INTEGER CHECK (user_rating IS NULL OR (user_rating BETWEEN 1 AND 10)),
    -- Clearlogo / wordmark art. `logo_path` is a relative path under
    -- `{data_path}/images/logos/movie/{tmdb_id}.{ext}`; NULL means no
    -- logo is stored. `logo_palette` is 'mono' (single opaque fill →
    -- retint via CSS) or 'multi' (preserve colours); NULL when no logo.
    logo_path                        TEXT,
    logo_palette                     TEXT
);
-- NOTE: `movie.status` was dropped. Phase is derived on read via a
-- CASE over media / download_content / watched_at. See
-- `services/phase.rs` for the canonical derivation.

-- Shows: `status` here is TMDB's airing status (returning/ended/in_production),
-- NOT an acquisition phase — that's metadata, not state, so it stays.
CREATE TABLE IF NOT EXISTS show (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    tmdb_id                  INTEGER UNIQUE NOT NULL,
    imdb_id                  TEXT,
    tvdb_id                  INTEGER,
    title                    TEXT    NOT NULL,
    original_title           TEXT,
    overview                 TEXT,
    tagline                  TEXT,
    year                     INTEGER,
    status                   TEXT,
    network                  TEXT,
    runtime                  INTEGER,
    certification            TEXT,
    poster_path              TEXT,
    backdrop_path            TEXT,
    genres                   TEXT,
    tmdb_rating              REAL,
    tmdb_vote_count          INTEGER,
    popularity               REAL,
    original_language        TEXT,
    youtube_trailer_id       TEXT,
    quality_profile_id       INTEGER NOT NULL REFERENCES quality_profile(id),
    monitored                INTEGER NOT NULL DEFAULT 1,
    -- 'future' (default): auto-acquire new episodes as they air.
    -- 'none': track the show but don't auto-download anything.
    monitor_new_items        TEXT    NOT NULL DEFAULT 'future',
    -- Season 0 ("Specials") is opt-in: shows like The Boys drop a
    -- short weekly that would otherwise clog Next Up / calendar for
    -- users who don't care. Default 0 means specials stay out of
    -- scope on follow; users toggle in the Follow / Manage dialog.
    -- When 0, `seed_acquire_in_scope` emits (0, 0) for season 0 rows
    -- regardless of `monitor_new_items`.
    monitor_specials         INTEGER NOT NULL DEFAULT 0,
    -- 'explicit': user deliberately followed via Follow dialog.
    -- 'adhoc': auto-followed by Play/Get/acquire-by-tmdb. Adhoc
    -- shows self-remove when their last acquired episode is
    -- discarded; explicit ones stick around.
    follow_intent            TEXT    NOT NULL DEFAULT 'explicit',
    added_at                 TEXT    NOT NULL,
    blurhash_poster          TEXT,
    blurhash_backdrop        TEXT,
    first_air_date           TEXT,
    last_air_date            TEXT,
    last_metadata_refresh    TEXT,
    -- 1..10 Trakt rating scale. See movie.user_rating for semantics.
    user_rating              INTEGER CHECK (user_rating IS NULL OR (user_rating BETWEEN 1 AND 10)),
    -- Intro-skipper (subsystem 15): when false, the player never
    -- surfaces the Skip Intro button for any episode of this show.
    -- Covers the "I like the theme song on Succession" case without a
    -- per-episode timing editor.
    skip_intros              INTEGER NOT NULL DEFAULT 1,
    -- Clearlogo / wordmark art. Same convention as `movie.logo_*`;
    -- path is `{data_path}/images/logos/show/{tmdb_id}.{ext}`.
    logo_path                TEXT,
    logo_palette             TEXT,
    -- 1 = follow is mid-fanout (show row inserted, season+episode
    -- loop has not yet committed all seasons). Reads filter
    -- `partial = 0` so the show is invisible until the fanout
    -- finishes. The reconcile loop retries stuck partials.
    partial                  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS series (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    show_id        INTEGER NOT NULL REFERENCES show(id) ON DELETE CASCADE,
    tmdb_id        INTEGER,
    season_number  INTEGER NOT NULL,
    title          TEXT,
    overview       TEXT,
    poster_path    TEXT,
    air_date       TEXT,
    monitored      INTEGER NOT NULL DEFAULT 1,
    episode_count  INTEGER,
    UNIQUE(show_id, season_number)
);

-- Episodes: `monitored` split into two orthogonal axes.
--   acquire  = "should the scheduler auto-search + grab releases?"
--   in_scope = "is this part of what I'm progressing through?"
-- These usually move together, but can legitimately diverge —
-- example: user has S1–S3 on a different server and doesn't want kino
-- to re-download them, but still wants S1E1 to show up in Next Up's
-- count so "X of Y aired watched" is meaningful. Default both to 1
-- (normal Follow = acquire + track everything); Play-auto-follow
-- sets acquire=0, in_scope=1 so the scheduler stays quiet while the
-- user still progresses; Follow-with-Latest-Only sets both to 0 for
-- the excluded seasons (scheduler skips AND Next Up ignores).
CREATE TABLE IF NOT EXISTS episode (
    id                               INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id                        INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    show_id                          INTEGER NOT NULL REFERENCES show(id) ON DELETE CASCADE,
    season_number                    INTEGER NOT NULL,
    tmdb_id                          INTEGER,
    tvdb_id                          INTEGER,
    episode_number                   INTEGER NOT NULL,
    title                            TEXT,
    overview                         TEXT,
    air_date_utc                     TEXT,
    runtime                          INTEGER,
    still_path                       TEXT,
    tmdb_rating                      REAL,
    acquire                          INTEGER NOT NULL DEFAULT 1,
    in_scope                         INTEGER NOT NULL DEFAULT 1,
    playback_position_ticks          INTEGER NOT NULL DEFAULT 0,
    play_count                       INTEGER NOT NULL DEFAULT 0,
    last_played_at                   TEXT,
    watched_at                       TEXT,
    preferred_audio_stream_index     INTEGER,
    preferred_subtitle_stream_index  INTEGER,
    last_searched_at                 TEXT,
    -- 1..10 Trakt rating scale. See movie.user_rating for semantics.
    user_rating                      INTEGER CHECK (user_rating IS NULL OR (user_rating BETWEEN 1 AND 10)),
    -- Intro/credits timestamps in milliseconds (subsystem 15). NULL =
    -- either not-analysed or analysed-but-not-detected; `intro_analysis_at`
    -- distinguishes the two.
    intro_start_ms                   INTEGER,
    intro_end_ms                     INTEGER,
    credits_start_ms                 INTEGER,
    credits_end_ms                   INTEGER,
    intro_analysis_at                TEXT,
    UNIQUE(series_id, episode_number)
);

CREATE TABLE IF NOT EXISTS media (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    movie_id           INTEGER REFERENCES movie(id) ON DELETE SET NULL,
    file_path          TEXT    NOT NULL,
    relative_path      TEXT    NOT NULL,
    size               INTEGER NOT NULL,
    container          TEXT,
    resolution         INTEGER,
    source             TEXT,
    video_codec        TEXT,
    audio_codec        TEXT,
    hdr_format         TEXT,
    is_remux           INTEGER NOT NULL DEFAULT 0,
    is_proper          INTEGER NOT NULL DEFAULT 0,
    is_repack          INTEGER NOT NULL DEFAULT 0,
    scene_name         TEXT,
    release_group      TEXT,
    release_hash       TEXT,
    runtime_ticks      INTEGER,
    date_added         TEXT    NOT NULL,
    original_file_path TEXT,
    indexer_flags      TEXT,
    trickplay_generated INTEGER NOT NULL DEFAULT 0,
    -- Counter bumped on each transient trickplay failure (IO
    -- blip, ffmpeg non-zero exit, probe failure). When the count
    -- hits TRICKPLAY_MAX_ATTEMPTS the sweep stops retrying by
    -- flipping `trickplay_generated` to 1 — otherwise a broken
    -- file would loop every sweep forever. Permanent failures
    -- (`TrickplayError::TooShort`) skip the counter and mark
    -- done directly because re-trying a 4-second clip will never
    -- succeed. Reset to 0 on success so re-imports of the same
    -- media start with a clean budget.
    trickplay_attempts INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS stream (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id         INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    stream_index     INTEGER NOT NULL,
    stream_type      TEXT    NOT NULL,
    codec            TEXT,
    language         TEXT,
    title            TEXT,
    is_external      INTEGER NOT NULL DEFAULT 0,
    is_default       INTEGER NOT NULL DEFAULT 0,
    is_forced        INTEGER NOT NULL DEFAULT 0,
    is_hearing_impaired INTEGER NOT NULL DEFAULT 0,
    path             TEXT,
    bitrate          INTEGER,
    -- video fields
    width            INTEGER,
    height           INTEGER,
    framerate        REAL,
    pixel_format     TEXT,
    color_space      TEXT,
    color_transfer   TEXT,
    color_primaries  TEXT,
    hdr_format       TEXT,
    -- audio fields
    channels         INTEGER,
    channel_layout   TEXT,
    sample_rate      INTEGER,
    bit_depth        INTEGER,
    -- True when the ffprobe `profile` string reports Dolby
    -- Atmos (EAC-3 with Joint Object Coding, or TrueHD with
    -- Atmos extensions). Surfaced on AudioTrack so the picker
    -- can label the track + future passthrough code can route
    -- Atmos-bearing streams to capable clients. Nothing here
    -- implies the client can decode Atmos — the stream rides
    -- inside the base codec either way.
    is_atmos         INTEGER NOT NULL DEFAULT 0,
    -- Raw ffprobe `profile` string (e.g. "DTS-HD MA", "LC",
    -- "Main 10"). Mostly carried for the DTS family so the
    -- HLS `CODECS` emitter can promote DTS-HD MA to `dtsh`
    -- instead of the conservative `dtsc`; orthogonal to the
    -- `is_atmos` boolean which pre-parses the Atmos variants.
    profile          TEXT
);

-- Container-authored chapter markers (MKV / MP4). Populated
-- during import from ffprobe's `-show_chapters` output; the
-- player uses them to render a chapter list + prev/next
-- navigation. Orthogonal to the intro/credits auto-skip
-- system — chapters are authored, skip timestamps are
-- heuristically detected per-show.
CREATE TABLE IF NOT EXISTS chapter (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id     INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    -- Sequential per-media index, assigned from the probe's
    -- time-ordered list. Stable across re-fetches so the
    -- frontend can key React lists off it without
    -- re-ordering jank.
    idx          INTEGER NOT NULL,
    -- Start time in seconds from the beginning of the media.
    -- REAL to match ffprobe's decimal precision.
    start_secs   REAL    NOT NULL,
    -- End time in seconds. `NULL` for the final chapter when
    -- ffprobe didn't surface an end.
    end_secs     REAL,
    -- Authored chapter title. MKV almost always has one;
    -- MP4 chapters often omit it, in which case the
    -- frontend falls back to "Chapter N".
    title        TEXT,
    UNIQUE (media_id, idx)
);
CREATE INDEX IF NOT EXISTS chapter_media_idx ON chapter(media_id);

CREATE TABLE IF NOT EXISTS media_episode (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id   INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    episode_id INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
    UNIQUE(media_id, episode_id)
);

CREATE TABLE IF NOT EXISTS download (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    release_id             INTEGER REFERENCES release(id),
    torrent_hash           TEXT,
    title                  TEXT    NOT NULL,
    state                  TEXT    NOT NULL DEFAULT 'queued',
    -- Watch-now lifecycle phase (`watch_now::WatchNowPhase`):
    -- 'phase_one' / 'phase_two' / 'settled' / 'cancelled'. NULL for
    -- downloads not driven by the watch-now flow. Persisted (not
    -- in-memory) so a restart preserves the background-loop's
    -- resume + cancel decision.
    wn_phase               TEXT,
    size                   INTEGER,
    downloaded             INTEGER NOT NULL DEFAULT 0,
    uploaded               INTEGER NOT NULL DEFAULT 0,
    download_speed         INTEGER NOT NULL DEFAULT 0,
    upload_speed           INTEGER NOT NULL DEFAULT 0,
    seeders                INTEGER,
    leechers               INTEGER,
    eta                    INTEGER,
    added_at               TEXT    NOT NULL,
    completed_at           TEXT,
    output_path            TEXT,
    magnet_url             TEXT,
    error_message          TEXT,
    seed_target_reached_at TEXT,
    -- Set to 1 when the VPN killswitch (subsystem 33) paused this row
    -- on a handshake-loss event. The killswitch's resume sweep only
    -- touches paused rows that carry this flag, so a user who
    -- explicitly paused mid-VPN-outage doesn't get auto-resumed when
    -- the tunnel comes back. Cleared on user resume too.
    paused_by_killswitch   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS download_content (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    download_id INTEGER NOT NULL REFERENCES download(id) ON DELETE CASCADE,
    movie_id    INTEGER REFERENCES movie(id) ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episode(id) ON DELETE CASCADE,
    CHECK (movie_id IS NOT NULL OR episode_id IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS release (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    guid            TEXT    NOT NULL,
    indexer_id      INTEGER REFERENCES indexer(id),
    movie_id        INTEGER REFERENCES movie(id) ON DELETE CASCADE,
    show_id         INTEGER REFERENCES show(id) ON DELETE CASCADE,
    season_number   INTEGER,
    episode_id      INTEGER REFERENCES episode(id) ON DELETE CASCADE,
    title           TEXT    NOT NULL,
    size            INTEGER,
    download_url    TEXT,
    magnet_url      TEXT,
    info_url        TEXT,
    info_hash       TEXT,
    publish_date    TEXT,
    seeders         INTEGER,
    leechers        INTEGER,
    grabs           INTEGER,
    resolution      INTEGER,
    source          TEXT,
    video_codec     TEXT,
    audio_codec     TEXT,
    hdr_format      TEXT,
    is_remux        INTEGER NOT NULL DEFAULT 0,
    is_proper       INTEGER NOT NULL DEFAULT 0,
    is_repack       INTEGER NOT NULL DEFAULT 0,
    release_group   TEXT,
    languages       TEXT,
    indexer_flags    TEXT,
    quality_score   INTEGER,
    status          TEXT    NOT NULL DEFAULT 'available',
    pending_until   TEXT,
    first_seen_at   TEXT    NOT NULL,
    grabbed_at      TEXT
);

CREATE TABLE IF NOT EXISTS blocklist (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    movie_id          INTEGER REFERENCES movie(id) ON DELETE CASCADE,
    episode_id        INTEGER REFERENCES episode(id) ON DELETE CASCADE,
    source_title      TEXT    NOT NULL,
    torrent_info_hash TEXT,
    indexer_id        INTEGER REFERENCES indexer(id),
    size              INTEGER,
    resolution        INTEGER,
    source            TEXT,
    video_codec       TEXT,
    message           TEXT,
    date              TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS history (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    movie_id     INTEGER REFERENCES movie(id) ON DELETE CASCADE,
    episode_id   INTEGER REFERENCES episode(id) ON DELETE CASCADE,
    event_type   TEXT    NOT NULL,
    date         TEXT    NOT NULL,
    source_title TEXT,
    quality      TEXT,
    download_id  TEXT,
    data         TEXT
);

-- Persistent tracing log store. Every tracing event (INFO+ by default)
-- lands here via a batched mpsc writer. Row-capped retention (default
-- 100k) handled by a scheduler task; nothing keys off this table's
-- liveness.
--   * Level stored as INT (0=ERROR..4=TRACE) so ordering is cheap.
--   * trace_id/span_id are small opaque blobs (hex-encoded-as-text to
--     make ad-hoc queries readable; collation doesn't matter).
--   * fields_json is optional — most lines don't have structured fields.
--   * subsystem is derived from target (first module segment under
--     `kino::`) so the frontend can filter without regex.
CREATE TABLE IF NOT EXISTS log_entry (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_us       INTEGER NOT NULL,          -- unix micros
    level       INTEGER NOT NULL,          -- 0=ERROR 1=WARN 2=INFO 3=DEBUG 4=TRACE
    target      TEXT    NOT NULL,
    subsystem   TEXT,
    trace_id    TEXT,
    span_id     TEXT,
    message     TEXT    NOT NULL,
    fields_json TEXT,
    source      TEXT    NOT NULL DEFAULT 'backend'
) STRICT;

-- ============================================================
-- Trakt integration (docs/subsystems/16-trakt.md)
-- ============================================================

-- OAuth tokens + user identity. Single row; kino is single-user so
-- there's no concept of multiple Trakt connections.
CREATE TABLE IF NOT EXISTS trakt_auth (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    access_token  TEXT    NOT NULL,
    refresh_token TEXT    NOT NULL,
    -- ISO 8601 UTC. Not trusted blindly — the HTTP client refreshes
    -- on any 401 response regardless of this value (handles clock
    -- skew + server-side revocation in one code path).
    expires_at    TEXT    NOT NULL,
    token_scope   TEXT    NOT NULL DEFAULT 'public',
    connected_at  TEXT    NOT NULL,
    username      TEXT,
    slug          TEXT
);

-- Incremental sync timestamps. Each column tracks the last time we
-- observed Trakt change the corresponding bucket (via `/sync/last_activities`)
-- so we only re-pull categories where the remote clock moved.
CREATE TABLE IF NOT EXISTS trakt_sync_state (
    id                          INTEGER PRIMARY KEY CHECK (id = 1),
    -- Remote last-activity watermarks (mirrored from `/sync/last_activities`).
    last_watched_movies_at      TEXT,
    last_watched_episodes_at    TEXT,
    last_rated_movies_at        TEXT,
    last_rated_shows_at         TEXT,
    last_rated_episodes_at      TEXT,
    last_watchlist_movies_at    TEXT,
    last_watchlist_shows_at     TEXT,
    last_collection_movies_at   TEXT,
    last_collection_episodes_at TEXT,
    last_playback_at            TEXT,
    -- Lifecycle markers.
    initial_import_done         INTEGER NOT NULL DEFAULT 0,
    last_full_sync_at           TEXT,
    last_incremental_sync_at    TEXT,
    -- Recommendations + trending caches (daily refresh; JSON payload
    -- keeps the Home row render as one query regardless of how many
    -- items Trakt returns).
    recommendations_cached_at   TEXT,
    recommendations_json        TEXT    NOT NULL DEFAULT '[]',
    trending_cached_at          TEXT,
    trending_json               TEXT    NOT NULL DEFAULT '[]'
);

-- Scrobble queue: stores events emitted while the network / Trakt
-- were unreachable. Drained by the `trakt_scrobble_drain` scheduler
-- task. `action = stop` events are converted to `/sync/history`
-- backfills on drain so a 24h offline window still records the watch
-- accurately. `start` / `pause` events are only useful in-session —
-- beyond a few minutes stale they're dropped.
CREATE TABLE IF NOT EXISTS trakt_scrobble_queue (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at      TEXT    NOT NULL,
    action          TEXT    NOT NULL CHECK (action IN ('start','pause','stop')),
    kind            TEXT    NOT NULL CHECK (kind IN ('movie','episode')),
    -- Stable reference to the local entity so we re-derive trakt IDs
    -- at drain time (handles a library fetch updating trakt_id after
    -- the scrobble was enqueued).
    movie_id        INTEGER REFERENCES movie(id) ON DELETE CASCADE,
    episode_id      INTEGER REFERENCES episode(id) ON DELETE CASCADE,
    progress        REAL    NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    last_attempt_at TEXT,
    CHECK ((movie_id IS NOT NULL) != (episode_id IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_trakt_scrobble_queue_created
    ON trakt_scrobble_queue(created_at);

-- ============================================================
-- Indexes
-- ============================================================

CREATE INDEX IF NOT EXISTS idx_movie_tmdb_id ON movie(tmdb_id);

CREATE INDEX IF NOT EXISTS idx_show_tmdb_id ON show(tmdb_id);

CREATE INDEX IF NOT EXISTS idx_series_show_id ON series(show_id);

CREATE INDEX IF NOT EXISTS idx_episode_series_id ON episode(series_id);
CREATE INDEX IF NOT EXISTS idx_episode_show_id ON episode(show_id);
CREATE INDEX IF NOT EXISTS idx_episode_air_date ON episode(air_date_utc);
-- Fast gate for the wanted sweep: acquire=1 + aired + not recently searched.
CREATE INDEX IF NOT EXISTS idx_episode_acquire ON episode(acquire);

CREATE INDEX IF NOT EXISTS idx_media_movie_id ON media(movie_id);

CREATE INDEX IF NOT EXISTS idx_media_episode_media_id ON media_episode(media_id);
CREATE INDEX IF NOT EXISTS idx_media_episode_episode_id ON media_episode(episode_id);

CREATE INDEX IF NOT EXISTS idx_stream_media_id ON stream(media_id);

-- ============================================================
-- Lists (subsystem 17)
-- ============================================================

-- A `list` is any external URL that resolves to a set of TMDB IDs.
-- Sources: MDBList, TMDB lists, Trakt custom lists, Trakt watchlist.
-- The Trakt watchlist is a *system list* (is_system=1) auto-created
-- on Trakt connect and removed on disconnect — it can't be unfollowed
-- from the UI while Trakt is connected.
CREATE TABLE IF NOT EXISTS list (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    source_type                 TEXT    NOT NULL,
    source_url                  TEXT    NOT NULL,
    source_id                   TEXT    NOT NULL,
    title                       TEXT    NOT NULL,
    description                 TEXT,
    item_count                  INTEGER NOT NULL DEFAULT 0,
    item_type                   TEXT    NOT NULL DEFAULT 'mixed',
    last_polled_at              TEXT,
    last_poll_status            TEXT,
    consecutive_poll_failures   INTEGER NOT NULL DEFAULT 0,
    -- Pinning-to-home + ordering is governed by HomePreferences
    -- (section_order / hidden_rows) — the pseudo-row ID is
    -- `list:<id>`. Keeping ordering in the preferences payload
    -- avoids mirroring state between two tables that could drift.
    is_system                   INTEGER NOT NULL DEFAULT 0,
    created_at                  TEXT    NOT NULL,
    UNIQUE (source_type, source_id)
);

CREATE TABLE IF NOT EXISTS list_item (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    list_id                     INTEGER NOT NULL REFERENCES list(id) ON DELETE CASCADE,
    tmdb_id                     INTEGER NOT NULL,
    item_type                   TEXT    NOT NULL,
    title                       TEXT    NOT NULL,
    poster_path                 TEXT,
    position                    INTEGER,
    added_at                    TEXT    NOT NULL,
    ignored_by_user             INTEGER NOT NULL DEFAULT 0,
    UNIQUE (list_id, tmdb_id, item_type)
);

CREATE INDEX IF NOT EXISTS idx_list_source ON list(source_type, source_id);
CREATE INDEX IF NOT EXISTS idx_list_polled ON list(last_polled_at);
CREATE INDEX IF NOT EXISTS idx_list_item_list ON list_item(list_id);
CREATE INDEX IF NOT EXISTS idx_list_item_tmdb ON list_item(tmdb_id, item_type);

CREATE INDEX IF NOT EXISTS idx_download_state ON download(state);
CREATE INDEX IF NOT EXISTS idx_download_content_download_id ON download_content(download_id);
CREATE INDEX IF NOT EXISTS idx_download_content_movie_id ON download_content(movie_id);
CREATE INDEX IF NOT EXISTS idx_download_content_episode_id ON download_content(episode_id);

CREATE INDEX IF NOT EXISTS idx_release_movie_id ON release(movie_id);
CREATE INDEX IF NOT EXISTS idx_release_show_id ON release(show_id);
CREATE INDEX IF NOT EXISTS idx_release_episode_id ON release(episode_id);
CREATE INDEX IF NOT EXISTS idx_release_status ON release(status);
-- Unique per-content release guid (partial: movie or episode, not both).
-- Protects against concurrent searches inserting duplicates that could
-- race into two downloads.
CREATE UNIQUE INDEX IF NOT EXISTS idx_release_unique_episode_guid
  ON release (episode_id, indexer_id, guid)
  WHERE episode_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_release_unique_movie_guid
  ON release (movie_id, indexer_id, guid)
  WHERE movie_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_blocklist_movie_id ON blocklist(movie_id);
CREATE INDEX IF NOT EXISTS idx_blocklist_episode_id ON blocklist(episode_id);

CREATE INDEX IF NOT EXISTS idx_history_movie_id ON history(movie_id);
CREATE INDEX IF NOT EXISTS idx_history_episode_id ON history(episode_id);
CREATE INDEX IF NOT EXISTS idx_history_event_type ON history(event_type);
CREATE INDEX IF NOT EXISTS idx_history_date ON history(date);

CREATE INDEX IF NOT EXISTS log_entry_ts_idx        ON log_entry(ts_us DESC);
CREATE INDEX IF NOT EXISTS log_entry_level_ts_idx  ON log_entry(level, ts_us DESC);
CREATE INDEX IF NOT EXISTS log_entry_subsystem_idx ON log_entry(subsystem, ts_us DESC);
CREATE INDEX IF NOT EXISTS log_entry_trace_idx     ON log_entry(trace_id) WHERE trace_id IS NOT NULL;

-- Persistent retry queue for resource removals (torrents, files,
-- directories) that must succeed but can transiently fail. Owned by
-- `cleanup::tracker::CleanupTracker`. Failed removals enqueue here;
-- the scheduler retries on a fixed cadence until success or until
-- `max_attempts` is reached, at which point admin UI surfaces the
-- orphan for manual action.
CREATE TABLE IF NOT EXISTS cleanup_queue (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    -- One of `cleanup::tracker::ResourceKind` (snake_case wire string).
    -- Stored as TEXT so adding a variant is a code-only change; sqlx
    -- Decode rejects unknown values at read time.
    resource_kind   TEXT    NOT NULL,
    -- The thing to remove. Interpretation depends on `resource_kind`:
    --   torrent   → info_hash (lowercase hex)
    --   file      → absolute filesystem path
    --   directory → absolute filesystem path
    target          TEXT    NOT NULL,
    -- Number of removal attempts so far. Incremented BEFORE the
    -- attempt so a crash mid-attempt still counts.
    attempts        INTEGER NOT NULL DEFAULT 0,
    -- Maximum attempts before the row is marked exhausted.
    max_attempts    INTEGER NOT NULL DEFAULT 5,
    -- Most recent error message, truncated to 4 KiB.
    last_error      TEXT,
    -- Wall clock of the most recent attempt. Drives the cadence
    -- check in `retry_failed` (skip rows newer than `now - retry_interval`).
    last_attempt_at TEXT,
    created_at      TEXT    NOT NULL,
    -- One row per (kind, target). A second failure on the same
    -- target updates the existing row instead of creating a duplicate.
    UNIQUE(resource_kind, target)
);

CREATE INDEX IF NOT EXISTS idx_cleanup_queue_retry
    ON cleanup_queue (last_attempt_at);

-- ─── Cast sender (subsystem 32) ────────────────────────────────────
-- Server-side Chromecast sender. Replaces the browser-native
-- chrome.cast.* SDK so Firefox / Safari users can cast too. See
-- docs/roadmap/32-cast-sender.md.

CREATE TABLE IF NOT EXISTS cast_device (
    -- mDNS service-instance name for discovered devices
    -- ('Chromecast-abc123._googlecast._tcp.local.') or
    -- 'manual:<uuid>' for manually-added IPs.
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    ip            TEXT NOT NULL,
    port          INTEGER NOT NULL DEFAULT 8009,
    model         TEXT,
    -- 'mdns' | 'manual'. mDNS rows are ephemeral (re-discovered on
    -- startup); manual rows persist across restarts.
    source        TEXT NOT NULL,
    last_seen     TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cast_device_source ON cast_device(source);

CREATE TABLE IF NOT EXISTS cast_session (
    id                  TEXT PRIMARY KEY,
    device_id           TEXT NOT NULL REFERENCES cast_device(id) ON DELETE CASCADE,
    -- transport_id + session_id are the Cast-protocol handles needed
    -- to reattach to a running receiver app after a backend restart.
    -- NULL until launch_app returns.
    transport_id        TEXT,
    session_id          TEXT,
    media_id            INTEGER REFERENCES media(id) ON DELETE SET NULL,
    -- 'starting' | 'active' | 'ended' | 'errored'.
    status              TEXT NOT NULL,
    -- Most recent MEDIA_STATUS payload, JSON. Lets a freshly-
    -- connected sender (other browser tab, restored client) rehydrate
    -- the player UI immediately without waiting for the next status
    -- broadcast.
    last_status_json    TEXT,
    last_position_ms    INTEGER,
    started_at          TEXT NOT NULL,
    ended_at            TEXT
);
CREATE INDEX IF NOT EXISTS idx_cast_session_status ON cast_session(status);
CREATE INDEX IF NOT EXISTS idx_cast_session_device ON cast_session(device_id);

-- ─── Backup tracker (subsystem 19) ─────────────────────────────────
-- Row per generated archive. The archive itself lives on disk at
-- `{backup_location_path}/{filename}`; this table is the index +
-- metadata used by the Settings → Backup page.

CREATE TABLE IF NOT EXISTS backup (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 'manual' | 'scheduled' | 'pre_restore'
    -- pre_restore rows are auto-created before any restore so the
    -- user can always undo. UI marks them visually distinct so
    -- they're harder to accidentally delete.
    kind            TEXT    NOT NULL,
    -- Filename relative to `config.backup_location_path`. Includes
    -- the kino version + timestamp, e.g.
    -- `kino-backup-2026-04-26T03-00-00-v0.4.2.tar.gz`.
    filename        TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL,
    -- Captured at backup time so a restore-onto-newer-kino path can
    -- decide whether the archive is compatible.
    kino_version    TEXT    NOT NULL,
    schema_version  INTEGER NOT NULL,
    checksum_sha256 TEXT    NOT NULL,
    created_at      TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_backup_kind_created ON backup (kind, created_at DESC);
