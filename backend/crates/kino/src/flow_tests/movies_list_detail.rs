//! `GET /api/v1/movies` (library list) + `GET /api/v1/movies/{id}`
//! (detail page backend). Both were untested — Library All-movies
//! tab and the movie-detail page depend on these shapes.

use crate::test_support::{TestAppBuilder, assert_status, json_body};

#[tokio::test]
async fn list_movies_paginated_on_fresh_install() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/movies").await).await;
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        0,
        "PaginatedResponse shape; got {body}"
    );
    assert_eq!(body["has_more"], false);
}

#[tokio::test]
async fn list_movies_returns_seeded_rows_with_status() {
    let app = TestAppBuilder::new().build().await;

    for (id, tmdb_id, title) in [(1_i64, 111_i64, "alpha"), (2, 222, "beta")] {
        sqlx::query(
            "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
             VALUES (?, ?, ?, (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
        )
        .bind(id)
        .bind(tmdb_id)
        .bind(title)
        .execute(&app.db)
        .await
        .unwrap();
    }

    let body = json_body(app.get("/api/v1/movies").await).await;
    let rows = body["results"].as_array().expect("results array");
    assert_eq!(rows.len(), 2);
    // `MOVIE_STATUS_SELECT` enriches each row with a derived `status`
    // column — part of the public contract that the Library tab filter
    // pills depend on.
    assert!(
        rows[0].get("status").is_some(),
        "status column present on list rows; got {body}"
    );
    // Neither movie has media or a download → both land in `wanted`.
    assert_eq!(rows[0]["status"], "wanted");
}

#[tokio::test]
async fn get_movie_detail_404_for_missing_id() {
    let app = TestAppBuilder::new().build().await;
    let resp = app.get("/api/v1/movies/9999").await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_movie_detail_returns_derived_status() {
    let app = TestAppBuilder::new().build().await;

    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at, watched_at)
         VALUES (1, 603, 'matrix', (SELECT id FROM quality_profile LIMIT 1),
                 datetime('now'), datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let body = json_body(app.get("/api/v1/movies/1").await).await;
    assert_eq!(body["id"], 1);
    assert_eq!(body["tmdb_id"], 603);
    assert_eq!(
        body["status"], "watched",
        "watched_at IS NOT NULL → status=watched"
    );
}
