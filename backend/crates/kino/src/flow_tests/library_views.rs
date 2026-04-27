//! Library view endpoints: stats, widget, calendar. The assertions
//! here are light — the data plane is simple DB queries and the
//! JSON shape is stable across kino versions. We mostly want
//! regression coverage that the endpoints don't break when the
//! library has content.

use serde_json::json;

use crate::test_support::{MockTmdbServer, TestAppBuilder, json_body};

#[tokio::test]
async fn stats_reports_movie_counts() {
    let tmdb = MockTmdbServer::start().await;
    tmdb.stub_movie(603).await;
    let app = TestAppBuilder::new()
        .with_tmdb(tmdb.base_url())
        .build()
        .await;

    // Fresh install: zeros everywhere.
    let before = json_body(app.get("/api/v1/stats").await).await;
    assert_eq!(before["movies_total"], 0);

    // Follow one movie → movies_total moves.
    app.post("/api/v1/movies", &json!({ "tmdb_id": 603 })).await;

    let after = json_body(app.get("/api/v1/stats").await).await;
    assert_eq!(after["movies_total"], 1, "movies_total reflects the follow");
}

#[tokio::test]
async fn widget_endpoint_returns_top_level_counters() {
    let app = TestAppBuilder::new().build().await;
    let body = json_body(app.get("/api/v1/widget").await).await;

    // Widget feeds Homepage / Dashy customapi tiles — keys are
    // part of the public contract (see doc comment on WidgetResponse).
    // Lock them in.
    for key in [
        "movies",
        "shows",
        "episodes",
        "wanted",
        "downloading",
        "queued",
        "available",
        "watched",
        "disk_bytes",
    ] {
        assert!(
            body.get(key).is_some(),
            "widget response missing {key}; body = {body}"
        );
    }
}

#[tokio::test]
async fn calendar_endpoint_returns_range_shape() {
    let app = TestAppBuilder::new().build().await;

    let body = json_body(
        app.get("/api/v1/calendar?start=2026-01-01&end=2026-12-31")
            .await,
    )
    .await;
    // Calendar returns a flat array of entries; empty library → [].
    assert!(body.is_array(), "calendar returns a JSON array; got {body}");
}
