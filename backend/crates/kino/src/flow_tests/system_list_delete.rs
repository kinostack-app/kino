//! `DELETE /api/v1/lists/{id}` on an `is_system = 1` row must 409 —
//! the Trakt-watchlist system list is tied to the OAuth connection
//! and can only be removed via disconnect, not via the list manager.

use crate::test_support::{TestAppBuilder, assert_status};

#[tokio::test]
async fn delete_system_list_returns_409() {
    let app = TestAppBuilder::new().build().await;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO list (source_type, source_url, source_id, title, is_system, created_at)
         VALUES ('trakt', 'trakt://watchlist', 'watchlist', 'Trakt Watchlist', 1, datetime('now'))
         RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    let resp = app.delete(&format!("/api/v1/lists/{id}")).await;
    assert_status(&resp, axum::http::StatusCode::CONFLICT);

    // Row must still be there — 409 must not side-effect.
    let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM list WHERE id = ?")
        .bind(id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(
        still_there, 1,
        "system list row preserved after rejected delete"
    );
}

#[tokio::test]
async fn delete_user_list_returns_204() {
    let app = TestAppBuilder::new().build().await;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO list (source_type, source_url, source_id, title, is_system, created_at)
         VALUES ('tmdb', 'https://...', '123', 'My list', 0, datetime('now'))
         RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    let resp = app.delete(&format!("/api/v1/lists/{id}")).await;
    assert_status(&resp, axum::http::StatusCode::NO_CONTENT);
}
