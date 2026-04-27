//! Scrobble: tell Trakt what's being watched right now.
//!
//! Trakt's model is `/scrobble/{start|pause|stop}` — the user emits
//! `start` when playback begins, `pause` when they pause, `stop` when
//! they finish or close. Trakt auto-marks-watched on stop if progress
//! >= 80%.
//!
//! Kino v1 implements the two ends:
//!   - **start** on the first progress report of a session (and on
//!     resume after >60s idle — we infer pause from absence of
//!     reports, not an explicit signal from the player)
//!   - **stop** when `playback::progress::update_progress` tells us
//!     the user crossed the watched threshold
//!
//! `pause` is deliberately skipped in v1 — it's informational only
//! and inferring it reliably without frontend hooks is more trouble
//! than it's worth. The spec accepts this as acceptable v1 coverage.
//!
//! Failures enqueue onto `trakt_scrobble_queue` which the drain task
//! re-submits as `/sync/history` back-fills on recovery (handles the
//! spec's "watched offline, network came back 30h later" case).
// Inline row struct next to the drain logic that uses it; hoisting to
// module level would add a `QueueRow` type that only `drain` reads.
#![allow(clippy::items_after_statements)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use tokio::sync::Mutex;

use super::client::{TraktClient, TraktError};
use super::sync;
use super::types::{Episode, Movie, ScrobbleAck, ScrobbleBody, Show, TraktIds};

/// Cheap handle, cloneable. Wraps the per-media session table.
#[derive(Clone, Default)]
pub struct ScrobbleManager {
    sessions: Arc<Mutex<HashMap<i64, Session>>>,
}

impl std::fmt::Debug for ScrobbleManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't try to lock the mutex for Debug — deadlock risk under
        // contention during panic. Opaque is fine; the interesting
        // state is the per-session map, not the handle.
        f.debug_struct("ScrobbleManager").finish_non_exhaustive()
    }
}

struct Session {
    last_reported_at: Instant,
    /// Wall-clock of the most recent successful `/scrobble/start` for
    /// this session. Used to enforce Trakt's 2-min per-item dedup —
    /// we never emit `/start` more than once every 120 s for the
    /// same media, even when a seek would otherwise qualify.
    last_start_at: Option<Instant>,
    started: bool,
    /// Last progress percentage reported in this session. Lets the
    /// seek detector notice large jumps (backward, or forward by more
    /// than the seek threshold) and re-emit `/start` with the new
    /// position so Trakt's "Now watching" badge reflects reality.
    last_progress_pct: f64,
}

/// Trakt recommends at most one `/scrobble/start` per item per two
/// minutes. Tighter than that and they may silently drop events.
const SCROBBLE_START_DEDUP: Duration = Duration::from_secs(120);

/// Progress-percentage jump (absolute delta) that counts as a user
/// seek rather than natural playback. Picked empirically: 5% of a
/// 45-min episode is ~135 s, well above a single 10 s progress tick
/// but small enough to catch chapter skips.
const SCROBBLE_SEEK_THRESHOLD_PCT: f64 = 5.0;

/// Kind of library entity. Disambiguates callers that feed us
/// ambiguous `media_id` values (a media row could back either a
/// movie or an episode; we care which).
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Movie,
    Episode,
}

impl ScrobbleManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Report a progress tick for a media. Emits `/scrobble/start` on
    /// the first tick, after an inferred pause (>60 s idle), and on a
    /// detected seek (progress moves backwards or jumps forward by
    /// more than `SCROBBLE_SEEK_THRESHOLD_PCT`). Re-emits are capped
    /// by a 2-minute per-item dedup so Trakt doesn't drop events.
    /// No-op when Trakt is disconnected or scrobbling is disabled.
    pub async fn on_progress(&self, db: &SqlitePool, media_id: i64, kind: Kind, progress_pct: f64) {
        if !scrobble_enabled(db).await {
            return;
        }
        let now = Instant::now();
        let should_emit_start = {
            let mut sessions = self.sessions.lock().await;
            let entry = sessions.entry(media_id).or_insert(Session {
                last_reported_at: now,
                last_start_at: None,
                started: false,
                last_progress_pct: progress_pct,
            });
            let idle_gap = now.duration_since(entry.last_reported_at);
            let idle_resumed = entry.started && idle_gap > Duration::from_secs(60);

            // Seek detection: progress moves backwards (user rewinds),
            // or forwards by more than the threshold in a single tick
            // (user fast-forwards past the natural 10 s advance).
            let pct_delta = progress_pct - entry.last_progress_pct;
            let seek_detected =
                entry.started && !(0.0..=SCROBBLE_SEEK_THRESHOLD_PCT).contains(&pct_delta);

            let never_started = !entry.started;
            let dedup_ok = entry
                .last_start_at
                .is_none_or(|t| now.duration_since(t) >= SCROBBLE_START_DEDUP);

            let will_emit = never_started || ((idle_resumed || seek_detected) && dedup_ok);

            entry.last_reported_at = now;
            entry.last_progress_pct = progress_pct;
            entry.started = true;
            if will_emit {
                entry.last_start_at = Some(now);
            }
            will_emit
        };
        if should_emit_start
            && let Err(e) = self.emit(db, "start", media_id, kind, progress_pct).await
        {
            tracing::debug!(media_id, error = %e, "trakt scrobble/start failed — queuing");
            let _ = enqueue(db, "start", media_id, kind, progress_pct).await;
        }
    }

    /// Mark a media as stopped (watched). Called from the playback
    /// progress path when the user crosses the watched threshold.
    /// Emits `/scrobble/stop` at 100% — Trakt treats that as a hard
    /// mark-watched regardless of whether we previously sent /start.
    pub async fn on_watched(&self, db: &SqlitePool, media_id: i64, kind: Kind) {
        if !scrobble_enabled(db).await {
            return;
        }
        self.sessions.lock().await.remove(&media_id);
        if let Err(e) = self.emit(db, "stop", media_id, kind, 100.0).await {
            tracing::debug!(media_id, error = %e, "trakt scrobble/stop failed — queuing");
            let _ = enqueue(db, "stop", media_id, kind, 100.0).await;
        }
    }

    /// User left the player mid-watch (tab close, back button, etc).
    /// Emits `/scrobble/pause` so Trakt clears the "Now watching"
    /// badge on the user's profile instead of leaving it lit for the
    /// rest of the content's runtime. Session is removed so a later
    /// re-open starts fresh with a new /scrobble/start. No-op on an
    /// unknown session (nothing to pause).
    pub async fn on_pause(&self, db: &SqlitePool, media_id: i64, kind: Kind, progress_pct: f64) {
        if !scrobble_enabled(db).await {
            return;
        }
        // Only emit pause if we actually had an active session —
        // otherwise the frontend's on-unmount beacon for a player
        // that never played anything would send a spurious pause.
        let had_session = self.sessions.lock().await.remove(&media_id).is_some();
        if !had_session {
            return;
        }
        if let Err(e) = self.emit(db, "pause", media_id, kind, progress_pct).await {
            tracing::debug!(media_id, error = %e, "trakt scrobble/pause failed — queuing");
            let _ = enqueue(db, "pause", media_id, kind, progress_pct).await;
        }
    }

    async fn emit(
        &self,
        db: &SqlitePool,
        action: &str,
        media_id: i64,
        kind: Kind,
        progress_pct: f64,
    ) -> Result<(), TraktError> {
        let client = TraktClient::from_db(db.clone()).await?;
        let body = build_body(db, media_id, kind, progress_pct).await?;
        let path = format!("/scrobble/{action}");
        let _: ScrobbleAck = client.post(&path, &body).await?;
        tracing::info!(action, media_id, progress_pct, "trakt scrobble emitted");
        Ok(())
    }
}

async fn scrobble_enabled(db: &SqlitePool) -> bool {
    // Gate on both the feature toggle AND the connected state — no
    // point in spending the lookup on a disconnected install.
    if !super::is_connected(db).await {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT trakt_scrobble FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
}

/// Build the `ScrobbleBody` for a given library entity. For episodes
/// we include the parent show's IDs so Trakt can disambiguate across
/// shows with the same S/E numbering.
async fn build_body(
    db: &SqlitePool,
    media_id: i64,
    kind: Kind,
    progress_pct: f64,
) -> Result<ScrobbleBody, TraktError> {
    match kind {
        Kind::Movie => {
            // `media_id` from the caller here is actually a movie.id,
            // not a media.id — callers pre-resolve. We keep the
            // `Kind` enum so the schema is self-documenting.
            let ids = movie_ids(db, media_id).await?;
            Ok(ScrobbleBody {
                progress: progress_pct,
                movie: Some(Movie {
                    title: String::new(),
                    year: None,
                    ids,
                }),
                episode: None,
                show: None,
            })
        }
        Kind::Episode => {
            let (show_ids, season, number) = episode_refs(db, media_id).await?;
            Ok(ScrobbleBody {
                progress: progress_pct,
                movie: None,
                episode: Some(Episode {
                    season: Some(season),
                    number: Some(number),
                    title: None,
                    ids: TraktIds::default(),
                }),
                show: Some(Show {
                    title: String::new(),
                    year: None,
                    ids: show_ids,
                }),
            })
        }
    }
}

async fn movie_ids(db: &SqlitePool, movie_id: i64) -> Result<TraktIds, TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        tmdb_id: Option<i64>,
        imdb_id: Option<String>,
        tvdb_id: Option<i64>,
    }
    let r: Row = sqlx::query_as("SELECT tmdb_id, imdb_id, tvdb_id FROM movie WHERE id = ?")
        .bind(movie_id)
        .fetch_one(db)
        .await?;
    Ok(TraktIds {
        trakt: None,
        slug: None,
        imdb: r.imdb_id,
        tmdb: r.tmdb_id,
        tvdb: r.tvdb_id,
    })
}

async fn episode_refs(
    db: &SqlitePool,
    episode_id: i64,
) -> Result<(TraktIds, i64, i64), TraktError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        season_number: i64,
        episode_number: i64,
        show_tmdb_id: Option<i64>,
        show_imdb_id: Option<String>,
        show_tvdb_id: Option<i64>,
    }
    let r: Row = sqlx::query_as(
        "SELECT e.season_number, e.episode_number,
                s.tmdb_id as show_tmdb_id, s.imdb_id as show_imdb_id, s.tvdb_id as show_tvdb_id
         FROM episode e JOIN show s ON s.id = e.show_id
         WHERE e.id = ?",
    )
    .bind(episode_id)
    .fetch_one(db)
    .await?;
    let ids = TraktIds {
        trakt: None,
        slug: None,
        imdb: r.show_imdb_id,
        tmdb: r.show_tmdb_id,
        tvdb: r.show_tvdb_id,
    };
    Ok((ids, r.season_number, r.episode_number))
}

async fn enqueue(
    db: &SqlitePool,
    action: &str,
    media_id: i64,
    kind: Kind,
    progress_pct: f64,
) -> Result<(), TraktError> {
    let (movie_id, episode_id) = match kind {
        Kind::Movie => (Some(media_id), None),
        Kind::Episode => (None, Some(media_id)),
    };
    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO trakt_scrobble_queue
            (created_at, action, kind, movie_id, episode_id, progress)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&now)
    .bind(action)
    .bind(match kind {
        Kind::Movie => "movie",
        Kind::Episode => "episode",
    })
    .bind(movie_id)
    .bind(episode_id)
    .bind(progress_pct)
    .execute(db)
    .await?;
    Ok(())
}

/// Drain the offline queue. Called by the `trakt_scrobble_drain`
/// scheduler task every 30s. Logic:
///   - `stop` events within 24h → re-emit as `/scrobble/stop`, or if
///     older than 5 min, convert to `/sync/history` back-fill so the
///     watch is still recorded even after long outages
///   - `start`/`pause` older than 5 min → drop silently (no value
///     backfilling "I started watching 3 hours ago")
///   - anything >24h old → drop with a WARN
pub async fn drain(db: &SqlitePool) -> Result<u64, TraktError> {
    if !super::is_connected(db).await {
        return Ok(0);
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        id: i64,
        created_at: String,
        action: String,
        kind: String,
        movie_id: Option<i64>,
        episode_id: Option<i64>,
        progress: f64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, created_at, action, kind, movie_id, episode_id, progress
         FROM trakt_scrobble_queue ORDER BY created_at ASC LIMIT 50",
    )
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let client = TraktClient::from_db(db.clone()).await?;
    let now = chrono::Utc::now();
    let mut handled = 0u64;
    for row in rows {
        let created_at = chrono::DateTime::parse_from_rfc3339(&row.created_at)
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or(now);
        let age = now.signed_duration_since(created_at);
        let over_24h = age > chrono::Duration::hours(24);
        let over_5min = age > chrono::Duration::minutes(5);

        // Drop stale "start"/"pause" and anything over 24h — not
        // recoverable as meaningful data.
        if over_24h || (over_5min && (row.action == "start" || row.action == "pause")) {
            tracing::warn!(
                id = row.id,
                action = %row.action,
                age_secs = age.num_seconds(),
                "dropping stale trakt scrobble queue entry",
            );
            sqlx::query("DELETE FROM trakt_scrobble_queue WHERE id = ?")
                .bind(row.id)
                .execute(db)
                .await?;
            continue;
        }

        let kind = if row.kind == "movie" {
            Kind::Movie
        } else {
            Kind::Episode
        };
        let target_id = row.movie_id.or(row.episode_id).unwrap_or(0);
        if target_id == 0 {
            sqlx::query("DELETE FROM trakt_scrobble_queue WHERE id = ?")
                .bind(row.id)
                .execute(db)
                .await?;
            continue;
        }

        let result = if over_5min && row.action == "stop" {
            // Convert to history back-fill with the original
            // watched_at so the play is still recorded.
            let watched_at = Some(row.created_at.clone());
            sync::push_watched(&client, row.movie_id, row.episode_id, watched_at).await
        } else {
            // Live path — re-emit as the original scrobble verb.
            let body = build_body(db, target_id, kind, row.progress).await?;
            let path = format!("/scrobble/{}", row.action);
            client
                .post::<_, ScrobbleAck>(&path, &body)
                .await
                .map(|_| ())
        };

        match result {
            Ok(()) => {
                sqlx::query("DELETE FROM trakt_scrobble_queue WHERE id = ?")
                    .bind(row.id)
                    .execute(db)
                    .await?;
                handled += 1;
            }
            Err(e) => {
                let err_str = e.to_string();
                let now_str = now.to_rfc3339();
                sqlx::query(
                    "UPDATE trakt_scrobble_queue SET
                        attempts        = attempts + 1,
                        last_error      = ?,
                        last_attempt_at = ?
                     WHERE id = ?",
                )
                .bind(&err_str)
                .bind(&now_str)
                .bind(row.id)
                .execute(db)
                .await?;
                tracing::debug!(id = row.id, error = %err_str, "scrobble drain retry failed");
                // Stop after the first failure this tick so we don't
                // hammer Trakt when it's down.
                break;
            }
        }
    }
    Ok(handled)
}
