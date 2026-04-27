//! `POST /api/v1/playback/cast-token` — issues a short-lived HMAC
//! token for Chromecast senders. Verifies URL shape resolves to the
//! live `/api/v1/play/{kind}/{entity_id}/…` routes, content-type
//! matches direct-vs-HLS choice, and the token is cast-only (no
//! raw API key in the URL). HMAC round-trip has its own unit
//! coverage in `playback::cast_token`.

use serde_json::json;

use crate::test_support::{TestAppBuilder, assert_status, json_body};

/// Insert a `movie` + linked `media` row so the cast-token endpoint
/// can resolve media → (kind, `entity_id`). Every production media
/// row comes from the import pipeline with one side of the link
/// populated; tests must reflect that invariant.
async fn insert_movie_with_media(
    db: &sqlx::SqlitePool,
    movie_id: i64,
    media_id: i64,
    file_path: &str,
) {
    sqlx::query(
        "INSERT INTO movie (id, tmdb_id, title, quality_profile_id, added_at)
         VALUES (?, ?, ?, (SELECT id FROM quality_profile LIMIT 1), datetime('now'))",
    )
    .bind(movie_id)
    .bind(movie_id) // tmdb_id — any unique int is fine for test
    .bind("Test Movie")
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO media (id, movie_id, file_path, relative_path, size, date_added)
         VALUES (?, ?, ?, ?, 100, datetime('now'))",
    )
    .bind(media_id)
    .bind(movie_id)
    .bind(file_path)
    .bind(
        std::path::Path::new(file_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(file_path),
    )
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn cast_token_404_on_missing_media() {
    let app = TestAppBuilder::new().build().await;
    let resp = app
        .post("/api/v1/playback/cast-token", &json!({ "media_id": 9999 }))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cast_token_404_when_media_has_no_link() {
    // A `media` row with neither `movie_id` nor a `media_episode`
    // join is an orphan (shouldn't happen in production, but the
    // endpoint must 404 cleanly rather than emit a malformed URL).
    let app = TestAppBuilder::new().build().await;
    sqlx::query(
        "INSERT INTO media (id, file_path, relative_path, size, date_added)
         VALUES (1, '/tmp/orphan.mp4', 'orphan.mp4', 100, datetime('now'))",
    )
    .execute(&app.db)
    .await
    .unwrap();
    let resp = app
        .post("/api/v1/playback/cast-token", &json!({ "media_id": 1 }))
        .await;
    assert_status(&resp, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cast_token_picks_hls_for_mkv() {
    let app = TestAppBuilder::new().build().await;
    insert_movie_with_media(&app.db, 1, 1, "/tmp/movie.mkv").await;

    let body = json_body(
        app.post("/api/v1/playback/cast-token", &json!({ "media_id": 1 }))
            .await,
    )
    .await;
    let stream_url = body["stream_url"].as_str().expect("stream_url in reply");
    assert!(
        stream_url.starts_with("/api/v1/play/movie/1/master.m3u8"),
        "MKV → HLS master URL under live play route; got {stream_url}"
    );
    assert_eq!(
        body["content_type"], "application/vnd.apple.mpegurl",
        "HLS content-type for Chromecast"
    );
    assert!(
        stream_url.contains("cast_token="),
        "URL must carry cast_token for receiver auth; got {stream_url}"
    );
    assert!(
        !stream_url.contains("api_key="),
        "URL must NOT carry raw api_key — the Chromecast receiver \
         only ever sees the HMAC token; got {stream_url}"
    );
}

#[tokio::test]
async fn cast_token_picks_direct_for_mp4() {
    let app = TestAppBuilder::new().build().await;
    insert_movie_with_media(&app.db, 1, 1, "/tmp/movie.mp4").await;

    let body = json_body(
        app.post("/api/v1/playback/cast-token", &json!({ "media_id": 1 }))
            .await,
    )
    .await;
    let stream_url = body["stream_url"].as_str().unwrap();
    assert!(
        stream_url.starts_with("/api/v1/play/movie/1/direct"),
        "MP4 → direct URL under live play route; got {stream_url}"
    );
    assert_eq!(body["content_type"], "video/mp4");
    assert!(
        stream_url.contains("cast_token="),
        "URL must carry cast_token; got {stream_url}"
    );
    assert!(
        !stream_url.contains("api_key="),
        "URL must NOT carry raw api_key; got {stream_url}"
    );
}
