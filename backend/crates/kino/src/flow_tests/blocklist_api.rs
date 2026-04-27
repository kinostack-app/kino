//! Blocklist REST endpoints. The actual blocklist-on-failure path is
//! tested inside the download service; here we cover the direct
//! list / delete / per-movie endpoints the Settings page uses.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_blocklist_is_empty_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/blocklist").await).await;
    // Paginated response: { results: [], next_cursor?: str, has_more: bool }
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        0,
        "no entries on fresh install"
    );
}

#[tokio::test]
async fn delete_missing_blocklist_entry_returns_404() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.delete("/api/v1/blocklist/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn clear_movie_blocklist_reports_removed_count() {
    let app = TestAppBuilder::new().build().await;

    // Seed two movie rows so the FK on `blocklist.movie_id` holds.
    for (id, tmdb_id) in [(1_i64, 111_i64), (2, 222)] {
        sqlx::query(
            "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
             VALUES (?, ?, 'fake', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
        )
        .bind(id)
        .bind(tmdb_id)
        .execute(&app.db)
        .await
        .unwrap();
    }

    // Seed two entries for movie 1, one for movie 2.
    for movie_id in [1_i64, 1, 2] {
        sqlx::query(
            "INSERT INTO blocklist (movie_id, source_title, torrent_info_hash, date)
             VALUES (?, ?, ?, datetime('now'))",
        )
        .bind(movie_id)
        .bind("fake release")
        .bind(format!("hash-{movie_id}-{}", rand_suffix()))
        .execute(&app.db)
        .await
        .unwrap();
    }

    // Per-movie read: 2 for movie 1.
    let before = json_body(app.get("/api/v1/blocklist/movie/1").await).await;
    assert_eq!(before.as_array().unwrap().len(), 2);

    // Clear movie 1 → removed count == 2.
    let cleared = json_body(app.delete("/api/v1/blocklist/movie/1").await).await;
    assert_eq!(cleared["removed"], 2);

    // Movie 2 untouched.
    let m2 = json_body(app.get("/api/v1/blocklist/movie/2").await).await;
    assert_eq!(m2.as_array().unwrap().len(), 1, "other movie preserved");
}

/// `SQLite`'s `blocklist.torrent_info_hash` isn't unique in the
/// schema but we still keep hashes distinct per row so we can tell
/// them apart when debugging. Add a tiny randomiser so
/// seed rows don't collide across parallel test binaries sharing a
/// working dir (shouldn't happen with `nextest --test-threads`, but
/// cheap insurance).
fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}", N.fetch_add(1, Ordering::Relaxed))
}
