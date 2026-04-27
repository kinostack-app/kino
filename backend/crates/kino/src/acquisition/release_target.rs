//! [`ReleaseTarget`] — the trait that lets release-pickup,
//! blocklist checks, search-debounce stamps, and download lookup
//! work uniformly against `Movie` and `Episode`. Adding a shared
//! step here forces the compiler to require both impls.
//!
//! Generic, not `dyn`: polymorphic call sites take a single typed
//! target, so generics suffice and let the trait use native AFIT
//! (no `async_trait` macro). The trait is consequently not
//! object-safe.
//!
//! ## Out of scope
//!
//! - **Full display title** ("Show · S01E02 · Title") — composing
//!   that for an episode requires a JOIN to the parent show.
//!   Callers use the existing `events::display::episode_display_title`
//!   helper.
//! - **Score profile + scoring** — handled by
//!   [`AcquisitionPolicy::evaluate`](crate::acquisition::AcquisitionPolicy);
//!   this trait stays focused on identity + state.
//! - **Grab semantics** (season-pack handling, etc) — handled at
//!   the search/grab layer.

use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};
use utoipa::ToSchema;

use crate::content::movie::model::Movie;
use crate::content::show::episode::Episode;
use crate::download::DownloadPhase;
use crate::download::model::Download;
use crate::time::Timestamp;

/// Discriminator for runtime branching. Most code stays generic; this
/// is only for the few sites that genuinely need to differ (e.g.
/// season-pack handling on episode grabs, or per-kind log fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseTargetKind {
    Movie,
    Episode,
}

impl ReleaseTargetKind {
    /// Wire / log string. Stable across the wire; do not rename
    /// without a coordinated frontend update.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Episode => "episode",
        }
    }
}

/// One row from the `blocklist` table, narrowed to the fields the
/// release-matching path needs. Loaded in bulk per target via
/// [`ReleaseTarget::load_blocklist`] so the per-release check is
/// O(M) instead of O(N×M) round-trips.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BlocklistEntry {
    pub torrent_info_hash: Option<String>,
    pub source_title: String,
}

impl BlocklistEntry {
    /// Does this blocklist entry match the given release? Hash match
    /// (case-insensitive) takes precedence; falls back to exact
    /// `source_title` equality. Centralised here so movie + episode
    /// search paths can't drift on what "blocklisted" means.
    #[must_use]
    pub fn matches_release(&self, release_hash: Option<&str>, release_title: &str) -> bool {
        if let (Some(entry_hash), Some(rel_hash)) = (&self.torrent_info_hash, release_hash)
            && entry_hash.eq_ignore_ascii_case(rel_hash)
        {
            return true;
        }
        self.source_title == release_title
    }
}

/// What a release can be picked *for*. Implemented for `Movie` and
/// `Episode`. See module docs for the rationale and what's
/// intentionally out of scope.
pub trait ReleaseTarget: Send + Sync {
    /// Discriminator for the few sites that need per-kind branching.
    fn kind(&self) -> ReleaseTargetKind;

    /// Primary key on the underlying table (`movie.id` or `episode.id`).
    fn id(&self) -> i64;

    /// The target's own title field. For a `Movie` this is the canonical
    /// display title; for an `Episode` this is the episode's own title
    /// (or `"(untitled)"` if none recorded). For the full
    /// "Show · S01E02 · Title" composition, callers use the
    /// `events::display::episode_display_title` helper — that requires
    /// a DB JOIN this trait deliberately doesn't model.
    fn target_title(&self) -> &str;

    /// The target's release year, when one is recorded directly on
    /// the row (no JOIN).
    ///
    /// `Movie` returns its `year` column cast to `u16`. `Episode`
    /// returns the year prefix of `air_date_utc` if parseable, else
    /// `None`. The full `Show.year` is the canonical "show started"
    /// year and lives one JOIN away — out of scope here. Callers
    /// that need it load the show separately.
    fn target_year(&self) -> Option<u16>;

    /// Load every blocklist entry scoped to this target. Returned as
    /// a list so the caller can match N candidate releases against M
    /// entries with one query rather than N×M round-trips.
    ///
    /// Scope: rows with this target's id in the matching FK column
    /// (`blocklist.movie_id` for movies, `blocklist.episode_id` for
    /// episodes). Show-level / global blocklist is out of scope here
    /// — that lives on `Show` and is checked separately by the search
    /// path before the per-target check.
    fn load_blocklist(
        &self,
        pool: &SqlitePool,
    ) -> impl Future<Output = sqlx::Result<Vec<BlocklistEntry>>> + Send;

    /// Stamp `last_searched_at` to the wall clock right now.
    ///
    /// Takes a transaction (not a pool) so the search-debounce stamp
    /// commits atomically with whatever else the calling operation
    /// is writing. Per `architecture/operations.md`: external state
    /// changes happen *after* commit; the `last_searched_at` update
    /// is internal state that must be visible to the next operation
    /// the moment this one returns.
    fn stamp_searched(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
    ) -> impl Future<Output = sqlx::Result<()>> + Send;

    /// Clear `last_searched_at` to NULL.
    ///
    /// Called from the blocklist-event reset path: when a release is
    /// blocklisted post-grab, the next sweep re-searches immediately
    /// rather than waiting out the debounce window.
    fn clear_search_stamp(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
    ) -> impl Future<Output = sqlx::Result<()>> + Send;

    /// Load the active `Download` row for this target, if any.
    ///
    /// "Active" = a row in `download` linked via `download_content`
    /// to this target, *and* whose phase is in
    /// [`DownloadPhase::is_runtime_monitored`]. Callers read
    /// `DownloadPhase::parse(&row.state)` on the result to branch on
    /// the phase.
    ///
    /// Note: this set is wider than [`DownloadPhase::needs_startup_reconcile`]
    /// — it includes `Searching` and `Queued`, which the
    /// `active_download_has_torrent` invariant deliberately excludes
    /// (those phases pre-date a librqbit hash). Both filters are
    /// correct in their own context; if you reach for the trait
    /// method expecting "every row that should hold a `torrent_hash`",
    /// you want the invariant's filter instead.
    ///
    /// Returns `None` when no in-flight grab exists. Returns at most
    /// one row.
    fn current_active_download(
        &self,
        pool: &SqlitePool,
    ) -> impl Future<Output = sqlx::Result<Option<Download>>> + Send;
}

// ── Movie impl ────────────────────────────────────────────────────

impl ReleaseTarget for Movie {
    fn kind(&self) -> ReleaseTargetKind {
        ReleaseTargetKind::Movie
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn target_title(&self) -> &str {
        &self.title
    }

    fn target_year(&self) -> Option<u16> {
        let year = self.year.and_then(|y| u16::try_from(y).ok())?;
        (1900..=2100).contains(&year).then_some(year)
    }

    async fn load_blocklist(&self, pool: &SqlitePool) -> sqlx::Result<Vec<BlocklistEntry>> {
        sqlx::query_as::<_, BlocklistEntry>(
            "SELECT torrent_info_hash, source_title FROM blocklist WHERE movie_id = ?",
        )
        .bind(self.id)
        .fetch_all(pool)
        .await
    }

    async fn stamp_searched(&self, tx: &mut Transaction<'_, Sqlite>) -> sqlx::Result<()> {
        sqlx::query("UPDATE movie SET last_searched_at = ? WHERE id = ?")
            .bind(Timestamp::now())
            .bind(self.id)
            .execute(&mut **tx)
            .await
            .map(|_| ())
    }

    async fn clear_search_stamp(&self, tx: &mut Transaction<'_, Sqlite>) -> sqlx::Result<()> {
        sqlx::query("UPDATE movie SET last_searched_at = NULL WHERE id = ?")
            .bind(self.id)
            .execute(&mut **tx)
            .await
            .map(|_| ())
    }

    async fn current_active_download(&self, pool: &SqlitePool) -> sqlx::Result<Option<Download>> {
        let sql = format!(
            "SELECT d.* FROM download d
             JOIN download_content dc ON dc.download_id = d.id
             WHERE dc.movie_id = ? AND d.state IN ({})
             LIMIT 1",
            DownloadPhase::sql_in_clause(DownloadPhase::is_runtime_monitored)
        );
        sqlx::query_as::<_, Download>(&sql)
            .bind(self.id)
            .fetch_optional(pool)
            .await
    }
}

// ── Episode impl ──────────────────────────────────────────────────

impl ReleaseTarget for Episode {
    fn kind(&self) -> ReleaseTargetKind {
        ReleaseTargetKind::Episode
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn target_title(&self) -> &str {
        self.title.as_deref().unwrap_or("(untitled)")
    }

    fn target_year(&self) -> Option<u16> {
        // Air date is the only date on the episode row; parse the
        // year prefix. The full `Show.year` is the canonical "show
        // started" year and lives one JOIN away — out of scope here.
        // Sanity-bounded so a malformed date (`"99999-..."`) doesn't
        // round-trip a nonsense year.
        let air = self.air_date_utc.as_deref()?;
        let year = air.get(..4)?.parse::<u16>().ok()?;
        (1900..=2100).contains(&year).then_some(year)
    }

    async fn load_blocklist(&self, pool: &SqlitePool) -> sqlx::Result<Vec<BlocklistEntry>> {
        sqlx::query_as::<_, BlocklistEntry>(
            "SELECT torrent_info_hash, source_title FROM blocklist WHERE episode_id = ?",
        )
        .bind(self.id)
        .fetch_all(pool)
        .await
    }

    async fn stamp_searched(&self, tx: &mut Transaction<'_, Sqlite>) -> sqlx::Result<()> {
        sqlx::query("UPDATE episode SET last_searched_at = ? WHERE id = ?")
            .bind(Timestamp::now())
            .bind(self.id)
            .execute(&mut **tx)
            .await
            .map(|_| ())
    }

    async fn clear_search_stamp(&self, tx: &mut Transaction<'_, Sqlite>) -> sqlx::Result<()> {
        sqlx::query("UPDATE episode SET last_searched_at = NULL WHERE id = ?")
            .bind(self.id)
            .execute(&mut **tx)
            .await
            .map(|_| ())
    }

    async fn current_active_download(&self, pool: &SqlitePool) -> sqlx::Result<Option<Download>> {
        let sql = format!(
            "SELECT d.* FROM download d
             JOIN download_content dc ON dc.download_id = d.id
             WHERE dc.episode_id = ? AND d.state IN ({})
             LIMIT 1",
            DownloadPhase::sql_in_clause(DownloadPhase::is_runtime_monitored)
        );
        sqlx::query_as::<_, Download>(&sql)
            .bind(self.id)
            .fetch_optional(pool)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serialises_snake_case() {
        assert_eq!(
            serde_json::to_string(&ReleaseTargetKind::Movie).unwrap(),
            "\"movie\""
        );
        assert_eq!(
            serde_json::to_string(&ReleaseTargetKind::Episode).unwrap(),
            "\"episode\""
        );
    }

    #[test]
    fn kind_as_str_matches_serde() {
        assert_eq!(ReleaseTargetKind::Movie.as_str(), "movie");
        assert_eq!(ReleaseTargetKind::Episode.as_str(), "episode");
    }

    #[test]
    fn blocklist_entry_matches_by_hash_case_insensitive() {
        let entry = BlocklistEntry {
            torrent_info_hash: Some("ABCDEF1234".into()),
            source_title: "irrelevant".into(),
        };
        assert!(entry.matches_release(Some("abcdef1234"), "completely different"));
        assert!(entry.matches_release(Some("ABCDEF1234"), "completely different"));
    }

    #[test]
    fn blocklist_entry_matches_by_title_when_no_hash() {
        let entry = BlocklistEntry {
            torrent_info_hash: None,
            source_title: "The Movie 2026 1080p WEBRip".into(),
        };
        assert!(entry.matches_release(None, "The Movie 2026 1080p WEBRip"));
        assert!(entry.matches_release(Some("any_hash"), "The Movie 2026 1080p WEBRip"));
    }

    #[test]
    fn blocklist_entry_no_match_when_both_mismatch() {
        let entry = BlocklistEntry {
            torrent_info_hash: Some("aaa".into()),
            source_title: "X".into(),
        };
        assert!(!entry.matches_release(Some("bbb"), "Y"));
        assert!(!entry.matches_release(None, "Y"));
    }

    #[test]
    fn blocklist_entry_title_match_when_hash_mismatch_still_returns_true() {
        // Falls through hash-mismatch to title; title matches → blocklisted.
        let entry = BlocklistEntry {
            torrent_info_hash: Some("aaa".into()),
            source_title: "X".into(),
        };
        assert!(entry.matches_release(Some("bbb"), "X"));
    }

    #[test]
    fn blocklist_entry_title_match_is_exact_not_substring() {
        let entry = BlocklistEntry {
            torrent_info_hash: None,
            source_title: "The Movie".into(),
        };
        assert!(!entry.matches_release(None, "The Movie 2026"));
        assert!(!entry.matches_release(None, "the movie"), "case-sensitive");
    }

    #[test]
    fn blocklist_entry_hash_takes_precedence_over_title() {
        // Hash matches → true regardless of title mismatch.
        let entry = BlocklistEntry {
            torrent_info_hash: Some("hash".into()),
            source_title: "different title".into(),
        };
        assert!(entry.matches_release(Some("hash"), "release title"));
    }

    // ── Movie sync getters ────────────────────────────────────────

    fn empty_movie() -> Movie {
        Movie {
            id: 42,
            tmdb_id: 0,
            imdb_id: None,
            tvdb_id: None,
            title: "The Movie".into(),
            original_title: None,
            overview: None,
            tagline: None,
            year: Some(2026),
            runtime: None,
            release_date: None,
            physical_release_date: None,
            digital_release_date: None,
            certification: None,
            poster_path: None,
            backdrop_path: None,
            genres: None,
            tmdb_rating: None,
            tmdb_vote_count: None,
            popularity: None,
            original_language: None,
            collection_tmdb_id: None,
            collection_name: None,
            youtube_trailer_id: None,
            quality_profile_id: 1,
            status: String::new(),
            monitored: true,
            added_at: String::new(),
            last_searched_at: None,
            blurhash_poster: None,
            blurhash_backdrop: None,
            playback_position_ticks: 0,
            play_count: 0,
            last_played_at: None,
            watched_at: None,
            preferred_audio_stream_index: None,
            preferred_subtitle_stream_index: None,
            last_metadata_refresh: None,
            user_rating: None,
            logo_path: None,
            logo_palette: None,
        }
    }

    #[test]
    fn movie_sync_getters() {
        let m = empty_movie();
        assert_eq!(m.kind(), ReleaseTargetKind::Movie);
        assert_eq!(m.id(), 42);
        assert_eq!(m.target_title(), "The Movie");
        assert_eq!(m.target_year(), Some(2026));
    }

    #[test]
    fn movie_target_year_none_when_year_missing() {
        let mut m = empty_movie();
        m.year = None;
        assert_eq!(m.target_year(), None);
    }

    #[test]
    fn movie_target_year_none_when_year_doesnt_fit_u16() {
        let mut m = empty_movie();
        m.year = Some(i64::from(u16::MAX) + 1);
        assert_eq!(m.target_year(), None);
    }

    // ── Episode sync getters ──────────────────────────────────────

    fn empty_episode() -> Episode {
        Episode {
            id: 7,
            series_id: 1,
            show_id: 1,
            season_number: 1,
            tmdb_id: None,
            tvdb_id: None,
            episode_number: 2,
            title: Some("Pilot".into()),
            overview: None,
            air_date_utc: Some("2026-04-24T20:00:00Z".into()),
            runtime: None,
            still_path: None,
            tmdb_rating: None,
            status: String::new(),
            acquire: true,
            in_scope: true,
            playback_position_ticks: 0,
            play_count: 0,
            last_played_at: None,
            watched_at: None,
            preferred_audio_stream_index: None,
            preferred_subtitle_stream_index: None,
            last_searched_at: None,
            intro_start_ms: None,
            intro_end_ms: None,
            credits_start_ms: None,
            credits_end_ms: None,
            intro_analysis_at: None,
        }
    }

    #[test]
    fn episode_sync_getters() {
        let e = empty_episode();
        assert_eq!(e.kind(), ReleaseTargetKind::Episode);
        assert_eq!(e.id(), 7);
        assert_eq!(e.target_title(), "Pilot");
        assert_eq!(e.target_year(), Some(2026));
    }

    #[test]
    fn episode_target_title_falls_back_when_none() {
        let mut e = empty_episode();
        e.title = None;
        assert_eq!(e.target_title(), "(untitled)");
    }

    #[test]
    fn episode_target_year_none_when_air_date_missing() {
        let mut e = empty_episode();
        e.air_date_utc = None;
        assert_eq!(e.target_year(), None);
    }

    #[test]
    fn episode_target_year_none_on_unparseable_air_date() {
        let mut e = empty_episode();
        e.air_date_utc = Some("not-a-date".into());
        assert_eq!(e.target_year(), None);
    }

    // ── DB-backed impl tests ─────────────────────────────────────
    //
    // These use a fresh in-memory SQLite pool with all migrations
    // applied. Each test inserts the rows it needs and asserts the
    // trait impl reads / writes the right columns.

    use crate::db;

    async fn fresh_pool() -> sqlx::SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .expect("seed defaults (quality_profile + config)");
        pool
    }

    async fn insert_movie(pool: &sqlx::SqlitePool, title: &str) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at)
             VALUES (?, ?, 1, 1, ?) RETURNING id",
        )
        .bind(rand::random::<i32>())
        .bind(title)
        .bind(&now)
        .fetch_one(pool)
        .await
        .expect("insert movie")
    }

    async fn insert_show_and_episode(pool: &sqlx::SqlitePool) -> (i64, i64) {
        let now = chrono::Utc::now().to_rfc3339();
        let show_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, monitor_new_items, added_at)
             VALUES (?, 'Test Show', 1, 1, 'future', ?) RETURNING id",
        )
        .bind(rand::random::<i32>())
        .bind(&now)
        .fetch_one(pool)
        .await
        .expect("insert show");
        let series_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO series (show_id, season_number) VALUES (?, 1) RETURNING id",
        )
        .bind(show_id)
        .fetch_one(pool)
        .await
        .expect("insert series");
        let ep_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, acquire, in_scope)
             VALUES (?, ?, 1, 1, 1, 1) RETURNING id",
        )
        .bind(series_id)
        .bind(show_id)
        .fetch_one(pool)
        .await
        .expect("insert episode");
        (show_id, ep_id)
    }

    async fn load_movie(pool: &sqlx::SqlitePool, id: i64) -> Movie {
        sqlx::query_as::<_, Movie>("SELECT *, '' AS status FROM movie WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("load movie")
    }

    async fn load_episode(pool: &sqlx::SqlitePool, id: i64) -> Episode {
        sqlx::query_as::<_, Episode>("SELECT *, '' AS status FROM episode WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("load episode")
    }

    #[tokio::test]
    async fn movie_load_blocklist_scopes_to_my_rows() {
        let pool = fresh_pool().await;
        let mine = insert_movie(&pool, "Mine").await;
        let other = insert_movie(&pool, "Other").await;

        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO blocklist (movie_id, torrent_info_hash, source_title, date)
             VALUES (?, 'aaa', 'My Release', ?)",
        )
        .bind(mine)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO blocklist (movie_id, torrent_info_hash, source_title, date)
             VALUES (?, 'bbb', 'Other Release', ?)",
        )
        .bind(other)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let m = load_movie(&pool, mine).await;
        let entries = m.load_blocklist(&pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].torrent_info_hash.as_deref(), Some("aaa"));
        assert_eq!(entries[0].source_title, "My Release");
    }

    #[tokio::test]
    async fn movie_stamp_searched_then_clear_round_trip() {
        let pool = fresh_pool().await;
        let id = insert_movie(&pool, "X").await;
        let m = load_movie(&pool, id).await;
        assert_eq!(m.last_searched_at, None);

        let mut tx = pool.begin().await.unwrap();
        m.stamp_searched(&mut tx).await.unwrap();
        tx.commit().await.unwrap();

        let after_stamp = load_movie(&pool, id).await;
        assert!(after_stamp.last_searched_at.is_some());

        let mut tx = pool.begin().await.unwrap();
        after_stamp.clear_search_stamp(&mut tx).await.unwrap();
        tx.commit().await.unwrap();

        let after_clear = load_movie(&pool, id).await;
        assert_eq!(after_clear.last_searched_at, None);
    }

    #[tokio::test]
    async fn movie_current_active_download_returns_active_then_none_when_terminal() {
        let pool = fresh_pool().await;
        let movie_id = insert_movie(&pool, "X").await;
        let now = chrono::Utc::now().to_rfc3339();

        // Insert an active (downloading) download linked to the movie.
        let dl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO download (title, state, added_at) VALUES (?, 'downloading', ?) RETURNING id",
        )
        .bind("dl")
        .bind(&now)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, ?)")
            .bind(dl_id)
            .bind(movie_id)
            .execute(&pool)
            .await
            .unwrap();

        let m = load_movie(&pool, movie_id).await;
        let active = m.current_active_download(&pool).await.unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().state, "downloading");

        // Move to a terminal phase — must drop out of "active".
        sqlx::query("UPDATE download SET state = 'cleaned_up' WHERE id = ?")
            .bind(dl_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(m.current_active_download(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn episode_load_blocklist_scopes_to_my_rows() {
        let pool = fresh_pool().await;
        let (_, ep_id) = insert_show_and_episode(&pool).await;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO blocklist (episode_id, torrent_info_hash, source_title, date)
             VALUES (?, 'eee', 'Ep Release', ?)",
        )
        .bind(ep_id)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let e = load_episode(&pool, ep_id).await;
        let entries = e.load_blocklist(&pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_title, "Ep Release");
    }

    #[tokio::test]
    async fn episode_stamp_searched_then_clear_round_trip() {
        let pool = fresh_pool().await;
        let (_, ep_id) = insert_show_and_episode(&pool).await;
        let e = load_episode(&pool, ep_id).await;
        assert_eq!(e.last_searched_at, None);

        let mut tx = pool.begin().await.unwrap();
        e.stamp_searched(&mut tx).await.unwrap();
        tx.commit().await.unwrap();

        assert!(load_episode(&pool, ep_id).await.last_searched_at.is_some());

        let mut tx = pool.begin().await.unwrap();
        e.clear_search_stamp(&mut tx).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(load_episode(&pool, ep_id).await.last_searched_at, None);
    }

    #[tokio::test]
    async fn episode_current_active_download_returns_active_then_none_when_terminal() {
        let pool = fresh_pool().await;
        let (_, ep_id) = insert_show_and_episode(&pool).await;
        let now = chrono::Utc::now().to_rfc3339();
        let dl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO download (title, state, added_at) VALUES (?, 'downloading', ?) RETURNING id",
        )
        .bind("ep dl")
        .bind(&now)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO download_content (download_id, episode_id) VALUES (?, ?)")
            .bind(dl_id)
            .bind(ep_id)
            .execute(&pool)
            .await
            .unwrap();

        let e = load_episode(&pool, ep_id).await;
        assert!(e.current_active_download(&pool).await.unwrap().is_some());

        sqlx::query("UPDATE download SET state = 'failed' WHERE id = ?")
            .bind(dl_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(e.current_active_download(&pool).await.unwrap().is_none());
    }

    /// Symmetric pair smoke test — proves the trait actually unifies
    /// the two paths. A generic helper called once per type works on
    /// both. If this stops compiling, the trait surface diverged and
    /// the symmetry guarantee is broken.
    async fn assert_no_active_download<T: ReleaseTarget>(pool: &sqlx::SqlitePool, t: &T) {
        assert!(t.current_active_download(pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn generic_helper_works_on_both_kinds() {
        let pool = fresh_pool().await;
        let movie_id = insert_movie(&pool, "M").await;
        let (_, ep_id) = insert_show_and_episode(&pool).await;
        let m = load_movie(&pool, movie_id).await;
        let e = load_episode(&pool, ep_id).await;
        assert_no_active_download(&pool, &m).await;
        assert_no_active_download(&pool, &e).await;
    }
}
