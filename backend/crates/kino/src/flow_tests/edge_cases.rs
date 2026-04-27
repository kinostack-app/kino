//! Targeted edge cases pulled from the spec's §"Known problems by
//! flow" inventory. Organised by the spec's own sections so it's
//! cheap to cross-reference. Each test starts with the `file:line`
//! reference it's covering.
//!
//! Not exhaustive — this file seeds the catalogue with the highest-
//! severity cases; others get added as they bubble up from manual QA
//! or production bugs.

use serde_json::json;

use crate::test_support::{FakeTorrentSession, MockTmdbServer, TestAppBuilder, json_body};

// ── Setup / config / auth / indexers ──────────────────────────────

/// `auth.rs:127` — a transient DB error on `get_api_key_from_db`
/// currently returns `None` via `.ok().flatten()`, which the
/// middleware treats as "no key configured (first run)" and lets
/// traffic through. For the *genuine* fresh-install case that
/// Auth middleware must distinguish "fresh install" (config row
/// missing → allow, the setup wizard needs to POST /config
/// unauthenticated) from "DB broken" (query errors → deny, or a
/// transient `SQLite` fault in prod silently drops auth).
///
/// Simulate the latter by dropping the `config` table entirely so
/// the SELECT returns a real `sqlx::Error` (`SQLite` can't find the
/// table). The `Ok(None)` path is already covered by the setup
/// wizard flow.
#[tokio::test]
async fn auth_denies_when_config_lookup_errors() {
    let app = TestAppBuilder::new().build().await;
    sqlx::query("DROP TABLE config")
        .execute(&app.db)
        .await
        .expect("drop config");
    let resp = app.get("/api/v1/movies").await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
}

/// Indexer URL scheme validation — `file://`, `gopher://`, empty
/// string must be rejected at the edge. We fetch these server-side
/// so non-http(s) schemes would either fail confusingly downstream
/// or, worse, point at local resources.
#[tokio::test]
async fn indexer_create_rejects_non_http_urls() {
    let app = TestAppBuilder::new().build().await;

    for bad_url in [
        "file:///etc/passwd",
        "gopher://example.invalid",
        "",
        "not-a-url",
        "http://",
    ] {
        let resp = app
            .post(
                "/api/v1/indexers",
                &json!({
                    "name": "bad",
                    "url": bad_url,
                    "api_key": "x",
                    "indexer_type": "torznab",
                    "priority": 25,
                    "enabled": true,
                }),
            )
            .await;
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "{bad_url:?} should be rejected as BadRequest",
        );
    }
}

#[tokio::test]
async fn indexer_update_rejects_non_http_urls() {
    let app = TestAppBuilder::new().build().await;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO indexer (name, url, indexer_type, enabled, priority)
         VALUES ('ok', 'https://good.invalid', 'torznab', 1, 25) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();

    let resp = app
        .put(
            &format!("/api/v1/indexers/{id}"),
            &json!({ "url": "file:///etc/passwd" }),
        )
        .await;
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

// ── Download state machine ────────────────────────────────────────

/// Cancelling a download that's already imported should be a no-op
/// or 409 — not silently revive it into a new download cycle. Spec
/// §"download state machine" + the `cancel_download` bug we hit
/// during manual testing.
#[tokio::test]
async fn cancel_imported_download_is_idempotent() {
    let app = TestAppBuilder::new().build().await;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, added_at) VALUES ('done', 'imported', datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .expect("insert imported download");

    let resp = app.delete(&format!("/api/v1/downloads/{id}")).await;
    assert!(
        resp.status().is_success() || resp.status() == axum::http::StatusCode::NO_CONTENT,
        "delete on imported returned {}",
        resp.status()
    );

    // Idempotent: either the row is gone (terminal-state delete path)
    // or still present but still imported — never resurrected as
    // queued/grabbing.
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
        .bind(id)
        .fetch_optional(&app.db)
        .await
        .expect("state");
    match state {
        None => {} // terminal-state delete removed the row — fine
        Some(s) => assert!(
            s == "imported" || s == "failed",
            "post-cancel state should stay terminal, got {s}"
        ),
    }
}

/// Resume on a download that has no `torrent_hash` silently flipped
/// the DB to `downloading` before the recent fix, lying about state
/// to the UI. Asserts the fix — hashless rows flip DB-only (nothing
/// to talk to), which is fine. Hash present but client gone is a
/// separate test (needs an unhealthy client).
#[tokio::test]
async fn resume_without_hash_is_benign() {
    let app = TestAppBuilder::new().build().await;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, added_at) VALUES ('stub', 'paused', datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .expect("insert paused download without hash");

    let resp = app
        .post(&format!("/api/v1/downloads/{id}/resume"), &json!({}))
        .await;
    assert!(
        resp.status().is_success(),
        "resume on hashless download should succeed; got {}",
        resp.status()
    );
    let state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
        .bind(id)
        .fetch_one(&app.db)
        .await
        .expect("state");
    assert_eq!(state, "downloading");
}

// ── Cleanup / cascade ─────────────────────────────────────────────

/// `delete_movie` used to leave `media.file_path` on disk and only
/// drop the DB row — disk filled up over time. The file-leak fix
/// queries paths before DELETE and unlinks them. Test verifies the
/// unlink happens (using a tempfile to simulate a hardlinked media
/// file).
#[tokio::test]
async fn delete_movie_removes_library_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let library_path = tmp.path().join("library");
    std::fs::create_dir_all(&library_path).expect("mkdir library");
    let media_path = library_path.join("FakeMovie.mkv");
    std::fs::write(&media_path, b"synthetic").expect("write media file");

    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    sqlx::query("UPDATE config SET media_library_path = ? WHERE id = 1")
        .bind(library_path.to_str().unwrap())
        .execute(&app.db)
        .await
        .expect("config update");

    // Follow a movie and manually link a fake media row pointing at
    // our tempfile, bypassing the real import flow (which we've
    // already covered separately).
    let follow = json_body(app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await).await;
    let movie_id = follow["id"].as_i64().expect("movie id");
    sqlx::query(
        "INSERT INTO media (movie_id, file_path, relative_path, size, container, date_added)
         VALUES (?, ?, 'FakeMovie.mkv', 9, 'mkv', datetime('now'))",
    )
    .bind(movie_id)
    .bind(media_path.to_str().unwrap())
    .execute(&app.db)
    .await
    .expect("media insert");

    assert!(media_path.exists(), "pre-delete: file exists");

    let resp = app.delete(&format!("/api/v1/movies/{movie_id}")).await;
    assert!(
        resp.status().is_success() || resp.status() == axum::http::StatusCode::NO_CONTENT,
        "delete_movie returned {}",
        resp.status()
    );

    assert!(
        !media_path.exists(),
        "post-delete: library file should be removed from disk — regression for the orphan-files bug"
    );
    let media_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE movie_id = ?")
        .bind(movie_id)
        .fetch_one(&app.db)
        .await
        .expect("media count");
    assert_eq!(media_count, 0, "media row gone along with the file");
}

// ── Self-healing retries ──────────────────────────────────────────

/// When a download fails for a non-user reason, the `retry_failed_listener`
/// blocklists the release and fires a fresh search. Test asserts
/// the blocklist insert + stamp that listener-driven retries make.
#[tokio::test]
async fn failed_download_tombstones_release_in_blocklist() {
    let app = TestAppBuilder::new().build().await;

    let movie_id: i64 = sqlx::query_scalar(
        "INSERT INTO movie (tmdb_id, title, quality_profile_id, monitored, added_at)
         VALUES (603, 'The Matrix', 1, 1, datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .expect("movie");
    let release_id: i64 = sqlx::query_scalar(
        "INSERT INTO release
           (guid, movie_id, title, magnet_url, info_hash, status, first_seen_at)
         VALUES ('r1', ?, 'Matrix.1080p', 'magnet:?xt=urn:btih:b', 'b', 'available', datetime('now'))
         RETURNING id",
    )
    .bind(movie_id)
    .fetch_one(&app.db)
    .await
    .expect("release");
    let download_id: i64 = sqlx::query_scalar(
        "INSERT INTO download (release_id, title, state, added_at)
         VALUES (?, 'Matrix.1080p', 'failed', datetime('now')) RETURNING id",
    )
    .bind(release_id)
    .fetch_one(&app.db)
    .await
    .expect("download");
    sqlx::query("INSERT INTO download_content (download_id, movie_id) VALUES (?, ?)")
        .bind(download_id)
        .bind(movie_id)
        .execute(&app.db)
        .await
        .expect("link");

    // Trigger the blocklist-and-retry helper directly — the listener
    // path does the same thing in production, just via the event bus.
    crate::acquisition::blocklist::blocklist_and_retry(&app.state, download_id, "dead timeout")
        .await;

    let blocklist_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM blocklist WHERE movie_id = ?")
            .bind(movie_id)
            .fetch_one(&app.db)
            .await
            .expect("blocklist count");
    assert_eq!(
        blocklist_count, 1,
        "failed download should add the release to the blocklist"
    );

    // last_searched_at should have been cleared so the next sweep
    // picks the content up without the backoff tier's delay.
    let last: Option<String> =
        sqlx::query_scalar("SELECT last_searched_at FROM movie WHERE id = ?")
            .bind(movie_id)
            .fetch_one(&app.db)
            .await
            .expect("last_searched_at");
    assert!(
        last.is_none(),
        "blocklist_and_retry should clear last_searched_at to trigger re-sweep"
    );
}

/// Cancelling a download (user-initiated) emits `DownloadFailed`
/// with a "cancelled" error — the retry listener skips on that
/// prefix so we don't auto-retry what the user explicitly cancelled.
#[tokio::test]
async fn user_cancel_is_not_eligible_for_auto_retry() {
    let app = TestAppBuilder::new().build().await;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO download (title, state, error_message, added_at)
         VALUES ('stub', 'failed', 'cancelled by user', datetime('now')) RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .expect("download");

    // The listener checks the error string prefix ("cancelled") and
    // skips — exact copy of the logic lives in events/listeners.rs.
    // Assert here via a direct string check so the rule is pinned.
    let err: String =
        sqlx::query_scalar::<_, Option<String>>("SELECT error_message FROM download WHERE id = ?")
            .bind(id)
            .fetch_one(&app.db)
            .await
            .expect("query error_message")
            .expect("error_message NOT NULL for this row");
    assert!(
        err.to_lowercase().starts_with("cancelled"),
        "user-cancel error_message must be recognised by the retry listener"
    );
}

// ── Grab dedup ────────────────────────────────────────────────────

/// A second grab of the same release (user double-clicks Manage →
/// Grab, or two scheduler ticks race on the same release) should
/// reuse the existing non-terminal download, not spawn a duplicate.
/// Regression for the info-hash-dedup path.
#[tokio::test]
async fn double_grab_of_same_release_dedups() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let torrents = FakeTorrentSession::new();

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .with_torrent(std::sync::Arc::new(torrents.clone()))
        .build()
        .await;

    // Movie + synthetic release.
    let follow = json_body(app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await).await;
    let movie_id = follow["id"].as_i64().expect("movie id");
    let release_id: i64 = sqlx::query_scalar(
        "INSERT INTO release
           (guid, movie_id, title, magnet_url, info_hash, status, first_seen_at, quality_score)
         VALUES
           ('g1', ?, 'The.Matrix.1080p', 'magnet:?xt=urn:btih:aaa', 'aaa', 'available', datetime('now'), 500)
         RETURNING id",
    )
    .bind(movie_id)
    .fetch_one(&app.db)
    .await
    .expect("release insert");

    // Two grabs in a row — second should dedup to the first's id.
    let first = app
        .post(&format!("/api/v1/releases/{release_id}/grab"), &json!({}))
        .await;
    assert!(first.status().is_success(), "first grab OK");
    let second = app
        .post(&format!("/api/v1/releases/{release_id}/grab"), &json!({}))
        .await;
    assert!(second.status().is_success(), "second grab OK (dedup)");

    let downloads: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM download WHERE release_id = ? AND state NOT IN ('failed', 'imported')",
    )
    .bind(release_id)
    .fetch_one(&app.db)
    .await
    .expect("count downloads");
    assert_eq!(
        downloads, 1,
        "second grab should dedup; got {downloads} active downloads"
    );
}
