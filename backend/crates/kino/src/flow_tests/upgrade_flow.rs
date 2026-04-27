//! End-to-end upgrade flow — existing 720p media, `search_movie`
//! finds a 1080p `BluRay` release via the mock indexer, `is_upgrade`
//! returns true, a new download row is created.
//!
//! The eligibility SQL is covered in `upgrade_eligibility`; this
//! test exercises the *grab* path that runs after eligibility picks
//! a candidate. Together they cover the full `wanted_search` upgrade
//! branch without needing `FFmpeg` or real network.

use crate::acquisition::search::movie::search_movie;
use crate::test_support::{FakeTorrentSession, MockTorznabServer, TestAppBuilder};
use std::sync::Arc;

#[tokio::test]
async fn upgrade_search_replaces_720p_with_1080p_bluray() {
    let torznab = MockTorznabServer::start().await;
    torznab.stub_search_fixture("matrix-releases.xml").await;

    // FakeTorrentSession so `grab_release` has a client to hand the
    // magnet to (otherwise the download row is still created but
    // `torrent_hash` stays NULL; we want the full path).
    let torrent = Arc::new(FakeTorrentSession::new());
    let app = TestAppBuilder::new()
        .with_torrent(torrent.clone())
        .build()
        .await;

    // Seed: one monitored movie with an existing 720p `BluRay` file.
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, year, imdb_id,
                            quality_profile_id, added_at, monitored)
         VALUES (1, 603, 'The Matrix', 1999, 'tt0133093',
                 (SELECT id FROM quality_profile WHERE is_default = 1),
                 datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media (id, movie_id, file_path, relative_path, size,
                            resolution, source, video_codec, date_added)
         VALUES (1, 1, '/tmp/matrix.720p.mkv', 'matrix.720p.mkv', 5_000_000_000,
                 720, 'bluray', 'h264', datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    // Register an indexer pointing at the mock.
    sqlx::query(
        "INSERT INTO indexer (name, url, indexer_type, enabled, priority)
         VALUES ('mock', ?, 'torznab', 1, 25)",
    )
    .bind(torznab.base_url())
    .execute(&app.db)
    .await
    .unwrap();

    // Run the movie search directly — what `wanted_search_sweep` calls
    // once eligibility picks up the upgrade candidate.
    search_movie(&app.state, 1).await.expect("search_movie");

    // A new download row must exist for this movie. The existing 720p
    // media stays in place (import happens on completion, not here).
    let download_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         WHERE dc.movie_id = 1",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(
        download_count, 1,
        "upgrade grab created exactly one download row"
    );

    // last_searched_at bumped so the next sweep respects the 7-day
    // upgrade backoff.
    let last_searched: Option<String> =
        sqlx::query_scalar("SELECT last_searched_at FROM movie WHERE id = 1")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(
        last_searched.is_some(),
        "last_searched_at stamped after search"
    );
}

#[tokio::test]
async fn upgrade_search_skips_grab_when_existing_media_already_at_cutoff() {
    let torznab = MockTorznabServer::start().await;
    torznab.stub_search_fixture("matrix-releases.xml").await;

    let app = TestAppBuilder::new().build().await;

    // Existing media already at bluray_1080p (the profile cutoff).
    // `is_upgrade` must reject the grab — cutoff rank is the ceiling.
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, year, imdb_id,
                            quality_profile_id, added_at, monitored)
         VALUES (1, 603, 'The Matrix', 1999, 'tt0133093',
                 (SELECT id FROM quality_profile WHERE is_default = 1),
                 datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media (id, movie_id, file_path, relative_path, size,
                            resolution, source, video_codec, date_added)
         VALUES (1, 1, '/tmp/matrix.1080p.mkv', 'matrix.1080p.mkv', 8_000_000_000,
                 1080, 'bluray', 'h264', datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO indexer (name, url, indexer_type, enabled, priority)
         VALUES ('mock', ?, 'torznab', 1, 25)",
    )
    .bind(torznab.base_url())
    .execute(&app.db)
    .await
    .unwrap();

    search_movie(&app.state, 1).await.expect("search_movie");

    // No new download — the mock offers a 2160p release but the
    // existing 1080p has already hit the profile cutoff, so the
    // scorer blocks the grab.
    let download_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM download d
         JOIN download_content dc ON dc.download_id = d.id
         WHERE dc.movie_id = 1",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(
        download_count, 0,
        "existing media at cutoff → no grab, even when 2160p is available"
    );
}
