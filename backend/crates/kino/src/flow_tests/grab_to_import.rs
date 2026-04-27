//! Full happy-path pipeline: follow → search → grab → download
//! completes → import → media linked.
//!
//! This is the most "everything wired together" flow test. It's the
//! one that would've caught the `delete_show` file-leak regression
//! and the "import-race lets two ticks both fire `do_import`" bug had
//! it existed at that moment.
//!
//! Scope deliberately small: one movie, one indexer, one release,
//! one fake torrent. Season-pack flow + multi-episode import get
//! their own tests.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::test_support::{FakeTorrentSession, MockTmdbServer, MockTorznabServer, TestAppBuilder};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn follow_movie_through_import_completes_cleanly() {
    // Set up temp directories for download + library paths. Using
    // tempfile so the test cleans up after itself; `_tmp` drops at
    // the end.
    let tmp = tempfile::tempdir().expect("tempdir");
    let download_dir = tmp.path().join("downloads");
    let library_dir = tmp.path().join("library");
    std::fs::create_dir_all(&download_dir).unwrap();
    std::fs::create_dir_all(&library_dir).unwrap();

    // TMDB + Torznab mocks + fake torrent client.
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let torznab = MockTorznabServer::start().await;
    torznab.stub_search_fixture("matrix-releases.xml").await;
    let torrents = FakeTorrentSession::new();

    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .with_torrent(Arc::new(torrents.clone()))
        .build()
        .await;

    // Point config at the temp dirs so find_media_file + hardlink
    // resolution find our fixture file.
    sqlx::query(
        "UPDATE config SET download_path = ?, media_library_path = ?, use_hardlinks = 0 WHERE id = 1",
    )
    .bind(download_dir.to_str().unwrap())
    .bind(library_dir.to_str().unwrap())
    .execute(&app.db)
    .await
    .expect("config path update");

    // Register the indexer.
    sqlx::query(
        "INSERT INTO indexer (name, url, api_key, indexer_type, enabled, priority)
         VALUES ('MockTorznab', ?, 'test-key', 'torznab', 1, 25)",
    )
    .bind(torznab.base_url())
    .execute(&app.db)
    .await
    .expect("indexer insert");

    // Follow the movie.
    let resp = app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;
    let resp_status = resp.status();
    assert!(
        resp_status.is_success(),
        "POST /api/v1/movies status: {resp_status}"
    );

    // Verify the movie row landed before the sweep — if it didn't,
    // wanted_search would have nothing to grab and the later
    // "no download row" assertion would mislead. Splits the failure
    // mode into "movie didn't insert" vs "search/grab didn't fire".
    let movie_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM movie WHERE tmdb_id = 603 AND monitored = 1")
            .fetch_one(&app.db)
            .await
            .expect("movie count");
    assert_eq!(
        movie_count, 1,
        "movie row should exist after follow (tmdb_id=603, monitored=1)"
    );

    // Verify the indexer is enabled — same diagnostic split.
    let indexer_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM indexer WHERE enabled = 1")
        .fetch_one(&app.db)
        .await
        .expect("indexer count");
    assert_eq!(indexer_count, 1, "1 enabled indexer expected");

    // Sweep → finds release → grabs it → inserts a `queued`
    // download. A separate monitor tick is what actually calls
    // add_torrent + populates the hash.
    app.run_task("wanted_search")
        .await
        .expect("wanted_search ok");
    app.run_task("stale_download_check")
        .await
        .expect("monitor tick ok");

    // A download row should now exist in 'downloading' state with a
    // hash assigned (FakeTorrent's add_torrent returned one). When
    // this assertion has fired on CI, the empty-set didn't say
    // *why* — pre-load the surrounding state into the panic message
    // so the next failure is self-diagnosing.
    let downloads: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, state, torrent_hash FROM download ORDER BY id")
            .fetch_all(&app.db)
            .await
            .expect("downloads");
    if downloads.is_empty() {
        let releases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM release")
            .fetch_one(&app.db)
            .await
            .unwrap_or(-1);
        let last_searched: Option<String> =
            sqlx::query_scalar("SELECT last_searched_at FROM movie WHERE tmdb_id = 603")
                .fetch_optional(&app.db)
                .await
                .unwrap_or(None);
        let blocklist: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM movie_release_blocklist")
            .fetch_one(&app.db)
            .await
            .unwrap_or(-1);
        panic!(
            "no download row after wanted_search + monitor tick. \
             releases_seen={releases}, last_searched_at={last_searched:?}, \
             blocklist_entries={blocklist}, torznab_url={}, tmdb_url={}",
            torznab.base_url(),
            tmdb.base_url(),
        );
    }
    let (download_id, _state, hash) = &downloads[0];
    let hash = hash
        .clone()
        .expect("a hash after start_download — FakeTorrent returned one");

    // Pre-stage the torrent with file metadata so FakeTorrent::complete
    // knows which file to write. Matches the release title the
    // sweep grabbed.
    torrents.preload(
        &hash,
        download_dir.clone(),
        vec![(
            0,
            PathBuf::from("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv"),
            1024,
        )],
        "The.Matrix.1999.1080p.BluRay.x264-GROUP",
    );

    // Drive completion — writes a placeholder file to the download
    // path.
    let written = torrents
        .complete(&hash)
        .expect("complete returns the written path");
    assert!(written.exists(), "fake wrote file to {}", written.display());

    // Mark the download as completed so the monitor calls import.
    sqlx::query("UPDATE download SET state = 'completed' WHERE id = ?")
        .bind(download_id)
        .execute(&app.db)
        .await
        .expect("mark completed");

    // Import trigger.
    crate::import::trigger::import_download(
        &app.db,
        &app.state.event_tx,
        Some(app.state.torrent.as_deref().expect("torrent set")),
        Some(&hash),
        *download_id,
        &app.state.ffprobe_path,
    )
    .await
    .expect("import succeeds");

    // Media row should now exist + be linked to the movie.
    let media_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media m JOIN movie mv ON mv.id = m.movie_id WHERE mv.tmdb_id = 603",
    )
    .fetch_one(&app.db)
    .await
    .expect("media count");
    assert_eq!(media_count, 1, "media row linked to the movie post-import");

    // Download state flipped to imported.
    let state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
        .bind(download_id)
        .fetch_one(&app.db)
        .await
        .expect("state");
    assert_eq!(
        state, "imported",
        "download should be in imported state, got {state}"
    );
}
