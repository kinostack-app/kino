//! `DELETE /api/v1/shows/{id}` — cascades to series + episodes +
//! `download_content`. Without a torrent client the cancellation
//! side-effects are no-ops, but the DB cascade still runs.

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn delete_show_404_for_missing_id() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/shows/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_show_cascades_to_episodes() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored)
         VALUES (1, 111, 'fake', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (1, 1, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    for ep in 1..=3_i64 {
        sqlx::query(
            "INSERT INTO episode (series_id, show_id, season_number, episode_number, title)
             VALUES (1, 1, 1, ?, 'pilot')",
        )
        .bind(ep)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let resp = app.delete("/api/v1/shows/1").await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let episodes_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episode WHERE show_id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(episodes_left, 0, "show delete cascades to episode rows");

    let series_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE show_id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(series_left, 0, "and to series rows");
}

/// Regression: a terminal-state download whose `download_content`
/// rows have already been removed (as happens after `discard_episode`
/// purges them) must still be cleaned up by `delete_show`. Without
/// the `release.show_id` fallback in the lookup query, the show
/// cascade hits FK 787 when it tries to drop `release` rows that
/// `download.release_id` still points at.
#[tokio::test]
async fn delete_show_cleans_downloads_with_missing_content_links() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored, follow_intent)
         VALUES (1, 222, 'orphan-content', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1, 'adhoc')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (1, 1, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO episode (id, series_id, show_id, season_number, episode_number, title)
         VALUES (10, 1, 1, 1, 1, 'pilot')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO release (id, guid, show_id, episode_id, title, first_seen_at)
         VALUES (100, 'guid-100', 1, 10, 'pilot.release', datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO download (id, release_id, title, state, added_at)
         VALUES (1000, 100, 'pilot.release', 'imported', datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    // Deliberately NO download_content row — mirrors the post-
    // discard_episode state that used to trip the FK.

    let resp = app.delete("/api/v1/shows/1").await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    let downloads_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download WHERE id = 1000")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(
        downloads_left, 0,
        "terminal download should be dropped even with no download_content link"
    );
    let releases_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM release WHERE show_id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(releases_left, 0, "release rows cascade-delete with show");
    let shows_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM show WHERE id = 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(shows_left, 0, "show row itself is gone");
}

/// Deleting a show should unlink the episode files AND prune the
/// now-empty `Season NN/` and show folders — bounded by the
/// configured `media_library_path`. Previously the files got
/// cleaned but the directories piled up.
#[tokio::test]
async fn delete_show_prunes_empty_library_dirs() {
    let app = TestAppBuilder::new().build().await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let lib_root = tmp.path();
    let show_dir = lib_root.join("TV").join("PrunedShow");
    let season_dir = show_dir.join("Season 01");
    tokio::fs::create_dir_all(&season_dir).await.unwrap();
    let file_path = season_dir.join("PrunedShow - S01E01.mkv");
    tokio::fs::write(&file_path, b"fake").await.unwrap();

    sqlx::query("UPDATE config SET media_library_path = ? WHERE id = 1")
        .bind(lib_root.to_string_lossy().to_string())
        .execute(&app.db)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO show (id, tmdb_id, title, quality_profile_id, added_at, monitored, follow_intent)
         VALUES (2, 333, 'PrunedShow', (SELECT id FROM quality_profile LIMIT 1), datetime('now'), 1, 'adhoc')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO series (id, show_id, season_number, monitored) VALUES (2, 2, 1, 1)")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO episode (id, series_id, show_id, season_number, episode_number, title)
         VALUES (20, 2, 2, 1, 1, 'pilot')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media (id, file_path, relative_path, size, date_added)
         VALUES (200, ?, 'TV/PrunedShow/Season 01/PrunedShow - S01E01.mkv', 4, datetime('now'))",
    )
    .bind(file_path.to_string_lossy().to_string())
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO media_episode (media_id, episode_id) VALUES (200, 20)")
        .execute(&app.db)
        .await
        .unwrap();

    let resp = app.delete("/api/v1/shows/2").await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);

    assert!(!file_path.exists(), "episode file was unlinked");
    assert!(!season_dir.exists(), "empty Season 01 dir pruned");
    assert!(!show_dir.exists(), "empty show dir pruned");
    assert!(
        lib_root.exists(),
        "library root itself never touched by prune"
    );
}
