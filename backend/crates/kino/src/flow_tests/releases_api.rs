//! `/api/v1/releases` + per-entity variants. The manual-pick drawer
//! in the UI pulls from here; a silent shape change breaks grab-from-
//! picker without breaking automatic grabs.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_releases_empty_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/releases").await).await;
    assert!(body.is_array(), "list returns array");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn grab_missing_release_returns_404() {
    use serde_json::json;
    let app = TestAppBuilder::new().build().await;
    let resp = app.post("/api/v1/releases/9999/grab", &json!({})).await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn filter_by_movie_id_returns_subset() {
    let app = TestAppBuilder::new().build().await;

    // Seed a movie + two releases, one for the movie, one orphan.
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (1, 111, 'fake', (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    // Schema requires guid NOT NULL + title NOT NULL; first_seen_at is
    // auto-stamped by the default (not present on this column; the
    // list ORDER BY just tolerates NULL via `NULLS LAST`).
    for (i, (movie_id, title)) in [(Some(1_i64), "matrix.2160p"), (None, "other.1080p")]
        .iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO release (guid, movie_id, title, first_seen_at)
             VALUES (?, ?, ?, datetime('now'))",
        )
        .bind(format!("guid-{i}"))
        .bind(movie_id)
        .bind(title)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let filtered = json_body(app.get("/api/v1/releases?movie_id=1").await).await;
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    assert_eq!(filtered[0]["title"], "matrix.2160p");
}
