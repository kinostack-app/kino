use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::db;
use crate::state::AppState;

async fn test_app() -> (axum::Router, String) {
    let (app, api_key, _) = test_app_with_db().await;
    (app, api_key)
}

async fn test_app_with_db() -> (axum::Router, String, SqlitePool) {
    let pool = db::create_test_pool().await;

    crate::init::ensure_defaults(&pool, "/tmp/kino-test")
        .await
        .unwrap();

    let api_key = sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    let (log_live, _) = tokio::sync::broadcast::channel(16);
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let (state, _trigger_rx) = AppState::new(
        pool.clone(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        std::path::PathBuf::from("/tmp/kino-test"),
        0,
        log_live,
        event_tx,
        2,
    );
    let app = crate::build_router(state);
    (app, api_key, pool)
}

fn authed_get(path: &str, api_key: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {api_key}"))
        .body(Body::empty())
        .unwrap()
}

fn authed_request(method: &str, path: &str, api_key: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Insert a movie directly via SQL (bypassing TMDB).
async fn insert_test_movie(pool: &SqlitePool, tmdb_id: i64, title: &str) -> i64 {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at) VALUES (?, ?, 1, 1, ?) RETURNING id",
    )
    .bind(tmdb_id)
    .bind(title)
    .bind(&now)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Insert a show + series + episodes directly via SQL.
async fn insert_test_show(pool: &SqlitePool, tmdb_id: i64, title: &str) -> i64 {
    let now = chrono::Utc::now().to_rfc3339();
    let show_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO show (tmdb_id, title, quality_profile_id, monitored, monitor_new_items, added_at) VALUES (?, ?, 1, 1, 'future', ?) RETURNING id",
    )
    .bind(tmdb_id)
    .bind(title)
    .bind(&now)
    .fetch_one(pool)
    .await
    .unwrap();

    let series_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO series (show_id, season_number) VALUES (?, 1) RETURNING id",
    )
    .bind(show_id)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO episode (series_id, show_id, season_number, episode_number, acquire, in_scope) VALUES (?, ?, 1, 1, 1, 1)",
    )
    .bind(series_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    show_id
}

/// Insert a download directly via SQL.
async fn insert_test_download(pool: &SqlitePool, title: &str, state: &str) -> i64 {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO download (title, state, added_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(title)
    .bind(state)
    .bind(&now)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Insert a release directly via SQL.
async fn insert_test_release(pool: &SqlitePool, title: &str, movie_id: Option<i64>) -> i64 {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO release (guid, title, movie_id, status, first_seen_at, magnet_url) VALUES (?, ?, ?, 'available', ?, 'magnet:?xt=urn:btih:test') RETURNING id",
    )
    .bind(title)
    .bind(title)
    .bind(movie_id)
    .bind(&now)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ========== Status ==========

#[tokio::test]
async fn status_endpoint_requires_no_auth() {
    let (app, _) = test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    // Test app has no torrent client and may be missing config → not "ok"
    assert_ne!(json["status"], "ok");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    assert!(json["warnings"].is_array());
    assert!(!json["warnings"].as_array().unwrap().is_empty());
}

// ========== Auth ==========

#[tokio::test]
async fn auth_rejects_missing_key() {
    let (app, _) = test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_rejects_wrong_key() {
    let (app, _) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/config", "wrong-key"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_accepts_bearer_token() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/config", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_accepts_x_api_key_header() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/config")
                .header("x-api-key", &api_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ========== Config ==========

#[tokio::test]
async fn get_config_masks_sensitive_fields() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/config", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    // Secrets are masked with `***`. This protects against XSS +
    // cross-origin exfiltration even though kino is single-user.
    // Non-sensitive fields still render truthfully.
    assert_eq!(json["api_key"], "***");
    assert_eq!(json["listen_address"], "0.0.0.0");
    assert_eq!(json["listen_port"], 8080);
}

#[tokio::test]
async fn update_config_partial() {
    let (app, api_key) = test_app().await;
    let resp = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/v1/config",
            &api_key,
            r#"{"max_concurrent_downloads": 5, "media_library_path": "/media"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["max_concurrent_downloads"], 5);
    assert_eq!(json["media_library_path"], "/media");
    assert_eq!(json["listen_port"], 8080); // unchanged
}

#[tokio::test]
async fn update_config_boolean_and_float_fields() {
    let (app, api_key) = test_app().await;
    let resp = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/v1/config",
            &api_key,
            r#"{"vpn_enabled": true, "seed_ratio_limit": 2.5, "auto_cleanup_enabled": false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["vpn_enabled"], true);
    assert_eq!(json["seed_ratio_limit"], 2.5);
    assert_eq!(json["auto_cleanup_enabled"], false);
}

#[tokio::test]
async fn update_config_sensitive_field_not_echoed() {
    let (app, api_key) = test_app().await;
    let resp = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            "/api/v1/config",
            &api_key,
            r#"{"tmdb_api_key": "my-secret-key"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    // Sensitive fields are masked in GET/PUT responses so XSS or a
    // cross-origin request can't exfiltrate them. The real value
    // still flows through on the write side (verified by a separate
    // roundtrip: re-PUT with "***" is a no-op).
    assert_eq!(json["tmdb_api_key"], "***");
}

// ========== Quality Profiles ==========

#[tokio::test]
async fn list_quality_profiles_returns_default() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/quality-profiles", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "Default");
}

#[tokio::test]
async fn create_quality_profile() {
    let (app, api_key) = test_app().await;
    let items = r#"[{"quality_id":"web_1080p","name":"WEB 1080p","allowed":true,"rank":12}]"#;
    let resp = app
        .oneshot(authed_request(
            "POST",
            "/api/v1/quality-profiles",
            &api_key,
            &serde_json::json!({"name": "Test", "cutoff": "web_1080p", "items": items}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = json_body(resp).await;
    assert_eq!(json["name"], "Test");
    assert!(json["id"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn create_quality_profile_invalid_items() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_request(
            "POST",
            "/api/v1/quality-profiles",
            &api_key,
            r#"{"name": "Bad", "cutoff": "web_1080p", "items": "not json"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_quality_profile_by_id() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/quality-profiles/1", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["name"], "Default");
}

#[tokio::test]
async fn get_quality_profile_not_found() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/quality-profiles/999", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_quality_profile() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_request(
            "PUT",
            "/api/v1/quality-profiles/1",
            &api_key,
            r#"{"name": "Renamed"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["name"], "Renamed");
    assert_eq!(json["cutoff"], "bluray_1080p"); // unchanged
}

#[tokio::test]
async fn delete_quality_profile_in_use() {
    let (app, api_key, pool) = test_app_with_db().await;
    // Insert a movie referencing the default profile (id=1)
    insert_test_movie(&pool, 603, "The Matrix").await;
    // Try to delete the profile — should fail with 409
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/quality-profiles/1")
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_quality_profile_success() {
    let (app, api_key) = test_app().await;
    let items = r#"[{"quality_id":"web_1080p","name":"WEB 1080p","allowed":true,"rank":12}]"#;
    // Create then delete
    let resp = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/v1/quality-profiles",
            &api_key,
            &serde_json::json!({"name": "Deletable", "cutoff": "web_1080p", "items": items})
                .to_string(),
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let id = json["id"].as_i64().unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/quality-profiles/{id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

// ========== Init ==========

#[tokio::test]
async fn ensure_defaults_is_idempotent() {
    let pool = db::create_test_pool().await;
    crate::init::ensure_defaults(&pool, "/tmp/kino-test")
        .await
        .unwrap();
    crate::init::ensure_defaults(&pool, "/tmp/kino-test")
        .await
        .unwrap();

    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM config")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let profiles = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM quality_profile")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(profiles, 1);
}

// ========== Movies (with SQL fixtures) ==========

#[tokio::test]
async fn list_movies_empty() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/movies", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
    assert!(!json["has_more"].as_bool().unwrap());
}

#[tokio::test]
async fn movie_crud_with_fixture() {
    let (app, api_key, pool) = test_app_with_db().await;
    let movie_id = insert_test_movie(&pool, 603, "The Matrix").await;

    // Get by ID
    let resp = app
        .clone()
        .oneshot(authed_get(&format!("/api/v1/movies/{movie_id}"), &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["title"], "The Matrix");
    assert_eq!(json["tmdb_id"], 603);
    assert_eq!(json["status"], "wanted");

    // List — should contain 1 movie
    let resp = app
        .clone()
        .oneshot(authed_get("/api/v1/movies", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["results"].as_array().unwrap().len(), 1);

    // Delete
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/movies/{movie_id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn movie_pagination_with_cursor() {
    let (app, api_key, pool) = test_app_with_db().await;
    // Insert 3 movies
    for i in 1..=3 {
        insert_test_movie(&pool, i * 100, &format!("Movie {i}")).await;
    }

    // First page: limit=2
    let resp = app
        .clone()
        .oneshot(authed_get("/api/v1/movies?limit=2", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(json["has_more"].as_bool().unwrap());
    let cursor = json["next_cursor"].as_str().unwrap();

    // Second page using cursor
    let resp = app
        .oneshot(authed_get(
            &format!("/api/v1/movies?limit=2&cursor={cursor}"),
            &api_key,
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(!json["has_more"].as_bool().unwrap());
}

#[tokio::test]
async fn get_movie_not_found() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/movies/999", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_movie_without_tmdb_returns_error() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_request(
            "POST",
            "/api/v1/movies",
            &api_key,
            r#"{"tmdb_id": 603, "quality_profile_id": 1}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ========== Shows (with SQL fixtures) ==========

#[tokio::test]
async fn show_crud_with_fixture() {
    let (app, api_key, pool) = test_app_with_db().await;
    let show_id = insert_test_show(&pool, 1396, "Breaking Bad").await;

    // Get by ID
    let resp = app
        .clone()
        .oneshot(authed_get(&format!("/api/v1/shows/{show_id}"), &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["title"], "Breaking Bad");

    // List seasons
    let resp = app
        .clone()
        .oneshot(authed_get(
            &format!("/api/v1/shows/{show_id}/seasons"),
            &api_key,
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let seasons = json.as_array().unwrap();
    assert_eq!(seasons.len(), 1);
    assert_eq!(seasons[0]["season_number"], 1);

    // List episodes
    let resp = app
        .clone()
        .oneshot(authed_get(
            &format!("/api/v1/shows/{show_id}/seasons/1/episodes"),
            &api_key,
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let episodes = json.as_array().unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0]["episode_number"], 1);
    assert_eq!(episodes[0]["status"], "wanted");

    // Delete cascades
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/shows/{show_id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn list_shows_empty() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/shows", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn get_show_not_found() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/shows/999", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ========== Indexers ==========

#[tokio::test]
async fn indexer_full_crud() {
    let (app, api_key) = test_app().await;

    // Create
    let resp = app
        .clone()
        .oneshot(authed_request(
            "POST",
            "/api/v1/indexers",
            &api_key,
            r#"{"name": "Test Indexer", "url": "https://example.com/api", "api_key": "secret123"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = json_body(resp).await;
    let id = json["id"].as_i64().unwrap();
    assert_eq!(json["name"], "Test Indexer");
    assert_eq!(json["priority"], 25);
    assert_eq!(json["enabled"], true);

    // Get
    let resp = app
        .clone()
        .oneshot(authed_get(&format!("/api/v1/indexers/{id}"), &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Update
    let resp = app
        .clone()
        .oneshot(authed_request(
            "PUT",
            &format!("/api/v1/indexers/{id}"),
            &api_key,
            r#"{"priority": 10, "enabled": false}"#,
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["priority"], 10);
    assert_eq!(json["enabled"], false);
    assert_eq!(json["name"], "Test Indexer"); // unchanged

    // List
    let resp = app
        .clone()
        .oneshot(authed_get("/api/v1/indexers", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);

    // Delete
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/indexers/{id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn indexer_not_found() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/indexers/999", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ========== Releases & Grab ==========

#[tokio::test]
async fn list_releases_empty() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/releases", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn grab_release_creates_download() {
    let (app, api_key, pool) = test_app_with_db().await;
    let movie_id = insert_test_movie(&pool, 603, "The Matrix").await;
    let release_id = insert_test_release(
        &pool,
        "The.Matrix.1999.1080p.BluRay.x264-GROUP",
        Some(movie_id),
    )
    .await;

    // Grab it
    let resp = app
        .clone()
        .oneshot(authed_request(
            "POST",
            &format!("/api/v1/releases/{release_id}/grab"),
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify release is now grabbed
    let status: String = sqlx::query_scalar("SELECT status FROM release WHERE id = ?")
        .bind(release_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "grabbed");

    // Verify a download was created
    let dl_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM download WHERE release_id = ?")
            .bind(release_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dl_count, 1);

    // Verify download is queued
    let dl_state: String = sqlx::query_scalar("SELECT state FROM download WHERE release_id = ?")
        .bind(release_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dl_state, "queued");
}

#[tokio::test]
async fn list_releases_filtered_by_movie() {
    let (app, api_key, pool) = test_app_with_db().await;
    let movie_id = insert_test_movie(&pool, 603, "The Matrix").await;
    insert_test_release(&pool, "Matrix.1080p", Some(movie_id)).await;
    insert_test_release(&pool, "Matrix.2160p", Some(movie_id)).await;
    insert_test_release(&pool, "Other.Movie", None).await;

    let resp = app
        .oneshot(authed_get(
            &format!("/api/v1/releases?movie_id={movie_id}"),
            &api_key,
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn grab_release_not_found() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_request(
            "POST",
            "/api/v1/releases/999/grab",
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ========== Blocklist ==========

#[tokio::test]
async fn blocklist_empty() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/blocklist", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["results"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn blocklist_delete_not_found() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/blocklist/999")
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ========== Downloads (state machine) ==========

#[tokio::test]
async fn download_lifecycle_pause_resume() {
    let (app, api_key, pool) = test_app_with_db().await;
    let dl_id = insert_test_download(&pool, "Test Download", "downloading").await;

    // Pause
    let resp = app
        .clone()
        .oneshot(authed_request(
            "POST",
            &format!("/api/v1/downloads/{dl_id}/pause"),
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
        .bind(dl_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "paused");

    // Resume
    let resp = app
        .clone()
        .oneshot(authed_request(
            "POST",
            &format!("/api/v1/downloads/{dl_id}/resume"),
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
        .bind(dl_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "downloading");
}

#[tokio::test]
async fn download_cancel_resets_content_status() {
    let (app, api_key, pool) = test_app_with_db().await;
    let movie_id = insert_test_movie(&pool, 603, "The Matrix").await;

    // Status is now derived: the movie shows 'downloading' while
    // there's an active download_content row linked to it, and
    // automatically reverts to 'wanted' when that link disappears —
    // no UPDATE SET status needed.
    let dl_id = insert_test_download(&pool, "Matrix Download", "downloading").await;
    sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, ?)")
        .bind(dl_id)
        .bind(movie_id)
        .execute(&pool)
        .await
        .unwrap();

    // Cancel
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/downloads/{dl_id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // With the download row (or its state flipped to 'failed')
    // no longer counting as "active," the derived phase of the
    // movie is 'wanted'. Check via GET which runs the CASE.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/movies/{movie_id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "wanted");
}

#[tokio::test]
async fn download_retry_from_failed() {
    let (app, api_key, pool) = test_app_with_db().await;
    let dl_id = insert_test_download(&pool, "Failed Download", "failed").await;
    sqlx::query("UPDATE download SET error_message = 'stalled' WHERE id = ?")
        .bind(dl_id)
        .execute(&pool)
        .await
        .unwrap();

    // Retry
    let resp = app
        .oneshot(authed_request(
            "POST",
            &format!("/api/v1/downloads/{dl_id}/retry"),
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
        .bind(dl_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "queued");

    // Error message should be cleared
    let err: Option<String> = sqlx::query_scalar("SELECT error_message FROM download WHERE id = ?")
        .bind(dl_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(err.is_none());
}

#[tokio::test]
async fn download_cannot_pause_queued() {
    let (app, api_key, pool) = test_app_with_db().await;
    let dl_id = insert_test_download(&pool, "Queued Download", "queued").await;

    let resp = app
        .oneshot(authed_request(
            "POST",
            &format!("/api/v1/downloads/{dl_id}/pause"),
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn download_cannot_retry_downloading() {
    let (app, api_key, pool) = test_app_with_db().await;
    let dl_id = insert_test_download(&pool, "Active Download", "downloading").await;

    let resp = app
        .oneshot(authed_request(
            "POST",
            &format!("/api/v1/downloads/{dl_id}/retry"),
            &api_key,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn download_delete_completed_removes_record() {
    let (app, api_key, pool) = test_app_with_db().await;
    let dl_id = insert_test_download(&pool, "Done Download", "completed").await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/downloads/{dl_id}"))
                .header("authorization", format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify it's gone
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM download WHERE id = ?")
        .bind(dl_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn list_downloads_ordered_by_state() {
    let (app, api_key, pool) = test_app_with_db().await;
    insert_test_download(&pool, "Queued", "queued").await;
    insert_test_download(&pool, "Active", "downloading").await;
    insert_test_download(&pool, "Failed", "failed").await;

    let resp = app
        .oneshot(authed_get("/api/v1/downloads", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let downloads = json
        .get("results")
        .and_then(serde_json::Value::as_array)
        .expect("paginated envelope");
    assert_eq!(downloads.len(), 3);
    // Downloading should come first (state priority 0)
    assert_eq!(downloads[0]["title"], "Active");
}

// ========== TMDB Proxy ==========

#[tokio::test]
async fn tmdb_search_without_client_returns_error() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/tmdb/search?q=matrix", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ========== Images ==========

#[tokio::test]
async fn image_cache_not_configured() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/images/movies/999/poster", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn image_invalid_content_type() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/images/invalid/1/poster", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ========== Library (search / calendar / stats) ==========

#[tokio::test]
async fn library_search_requires_q() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/library/search", &api_key))
        .await
        .unwrap();
    // q is required by query extractor — missing returns 400 from axum
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn library_search_empty_q_rejected() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/library/search?q=%20", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn library_search_matches_movie_and_show() {
    let (app, api_key, pool) = test_app_with_db().await;
    insert_test_movie(&pool, 1001, "The Matrix").await;
    insert_test_show(&pool, 2001, "The Matrix Show").await;
    insert_test_movie(&pool, 1002, "Unrelated").await;

    let resp = app
        .oneshot(authed_get("/api/v1/library/search?q=matrix", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    let hits = json.as_array().unwrap();
    assert_eq!(hits.len(), 2);
    let types: Vec<&str> = hits
        .iter()
        .map(|h| h["item_type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"movie"));
    assert!(types.contains(&"show"));
}

#[tokio::test]
async fn library_search_is_case_insensitive() {
    let (app, api_key, pool) = test_app_with_db().await;
    insert_test_movie(&pool, 3001, "Dune").await;

    let resp = app
        .oneshot(authed_get("/api/v1/library/search?q=DUNE", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn library_search_respects_limit() {
    let (app, api_key, pool) = test_app_with_db().await;
    for i in 0..5 {
        insert_test_movie(&pool, 4000 + i, &format!("Alpha {i}")).await;
    }
    let resp = app
        .oneshot(authed_get(
            "/api/v1/library/search?q=alpha&limit=2",
            &api_key,
        ))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn calendar_returns_episodes_in_range() {
    let (app, api_key, pool) = test_app_with_db().await;
    let show_id = insert_test_show(&pool, 5001, "Test Show").await;
    // Override the default episode to have an air date in range
    let today = chrono::Utc::now().date_naive().to_string();
    sqlx::query("UPDATE episode SET air_date_utc = ?, title = 'Pilot' WHERE show_id = ?")
        .bind(format!("{today}T00:00:00Z"))
        .bind(show_id)
        .execute(&pool)
        .await
        .unwrap();

    let resp = app
        .oneshot(authed_get("/api/v1/calendar", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    let entries = json.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["item_type"], "episode");
    assert_eq!(entries[0]["show_title"], "Test Show");
    assert_eq!(entries[0]["episode_title"], "Pilot");
}

#[tokio::test]
async fn calendar_includes_movie_releases() {
    let (app, api_key, pool) = test_app_with_db().await;
    let mid = insert_test_movie(&pool, 6001, "Upcoming Movie").await;
    let today = chrono::Utc::now().date_naive().to_string();
    sqlx::query("UPDATE movie SET release_date = ? WHERE id = ?")
        .bind(&today)
        .bind(mid)
        .execute(&pool)
        .await
        .unwrap();

    let resp = app
        .oneshot(authed_get("/api/v1/calendar", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    let entries = json.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["item_type"], "movie");
    assert_eq!(entries[0]["title"], "Upcoming Movie");
}

#[tokio::test]
async fn calendar_rejects_bad_range() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get(
            "/api/v1/calendar?start=2026-06-01&end=2026-01-01",
            &api_key,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stats_returns_zeros_on_empty_db() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/stats", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;
    assert_eq!(json["movies_total"], 0);
    assert_eq!(json["shows_total"], 0);
    assert_eq!(json["episodes_total"], 0);
    assert_eq!(json["media_files"], 0);
    assert_eq!(json["media_bytes"], 0);
    assert_eq!(json["downloads_active"], 0);
}

#[tokio::test]
async fn widget_returns_flat_counters_for_external_dashboards() {
    let (app, api_key, pool) = test_app_with_db().await;
    insert_test_movie(&pool, 9001, "A movie").await;
    insert_test_show(&pool, 9002, "A show").await;
    insert_test_download(&pool, "d1", "downloading").await;
    insert_test_download(&pool, "d2", "queued").await;

    let resp = app
        .oneshot(authed_get("/api/v1/widget", &api_key))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_body(resp).await;

    // Flat top-level counters for Homepage's customapi widget mapping.
    assert_eq!(json["movies"], 1);
    assert_eq!(json["shows"], 1);
    assert_eq!(json["episodes"], 1);
    assert_eq!(json["wanted"], 2); // 1 wanted movie + 1 wanted episode
    assert_eq!(json["downloading"], 1);
    assert_eq!(json["queued"], 1);
    assert_eq!(json["available"], 0);
    assert_eq!(json["watched"], 0);
    assert_eq!(json["disk_bytes"], 0);
}

#[tokio::test]
async fn widget_empty_db_returns_zeros() {
    let (app, api_key) = test_app().await;
    let resp = app
        .oneshot(authed_get("/api/v1/widget", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["movies"], 0);
    assert_eq!(json["shows"], 0);
    assert_eq!(json["wanted"], 0);
    assert_eq!(json["disk_bytes"], 0);
}

#[tokio::test]
async fn stats_counts_rows() {
    let (app, api_key, pool) = test_app_with_db().await;
    insert_test_movie(&pool, 7001, "A").await;
    insert_test_movie(&pool, 7002, "B").await;
    let show_id = insert_test_show(&pool, 7003, "S").await;
    let _ = show_id; // episode was inserted by helper too
    insert_test_download(&pool, "d1", "downloading").await;
    insert_test_download(&pool, "d2", "completed").await;
    insert_test_download(&pool, "d3", "failed").await;

    let resp = app
        .oneshot(authed_get("/api/v1/stats", &api_key))
        .await
        .unwrap();
    let json = json_body(resp).await;
    assert_eq!(json["movies_total"], 2);
    assert_eq!(json["movies_wanted"], 2);
    assert_eq!(json["shows_total"], 1);
    assert_eq!(json["episodes_total"], 1);
    assert_eq!(json["downloads_active"], 1);
    assert_eq!(json["downloads_completed"], 1);
    assert_eq!(json["downloads_failed"], 1);
}

// ========== OpenAPI ==========

#[tokio::test]
async fn openapi_spec_is_valid() {
    let spec = <crate::ApiDoc as utoipa::OpenApi>::openapi();
    let json = serde_json::to_string_pretty(&spec).unwrap();
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Opt-in export for frontend codegen: KINO_EXPORT_OPENAPI=1 cargo test openapi
    // Write to the backend workspace root (../../openapi.json from the
    // package dir) so the frontend's `openapi-ts` config finds it at
    // the stable `../backend/openapi.json` path it was written against.
    if std::env::var("KINO_EXPORT_OPENAPI").is_ok() {
        std::fs::write("../../openapi.json", &json).expect("write openapi.json");
    }
    assert!(json.contains("/api/v1/status"));
    assert!(json.contains("/api/v1/config"));
    assert!(json.contains("/api/v1/quality-profiles"));
    assert!(json.contains("/api/v1/movies"));
    assert!(json.contains("/api/v1/shows"));
    assert!(json.contains("/api/v1/tmdb/search"));
    assert!(json.contains("/api/v1/indexers"));
    assert!(json.contains("/api/v1/releases"));
    assert!(json.contains("/api/v1/blocklist"));
    assert!(json.contains("/api/v1/downloads"));
    assert!(json.contains("/api/v1/library/search"));
    assert!(json.contains("/api/v1/calendar"));
    assert!(json.contains("/api/v1/stats"));
    assert!(json.contains("/api/v1/widget"));
}
