//! Wiremock-backed fake TMDB server for integration tests.
//!
//! Spins up a local HTTP server that answers TMDB-shaped requests
//! from captured fixtures under `fixtures/tmdb/`. Tests pass its
//! URL to `TmdbClient::with_base_url`, usually via the
//! `TestAppBuilder::with_tmdb()` helper.
//!
//! Fixtures are raw JSON captured from real TMDB responses, trimmed
//! to the fields kino actually consumes. Re-capturing when TMDB's
//! schema drifts is a manual chore — no automated schema check.
//!
//! Why `wiremock` vs. a handler that serves files:
//! - `wiremock::MockServer` binds an ephemeral port → no collision
//!   across concurrent tests.
//! - Response-matching is declarative (`Mock::given(...)`), which
//!   keeps test setup readable.
//! - Fallthrough behaviour for unmatched requests is a 404 with a
//!   useful body — helps debug "I forgot to stub `/genre/movie/list`"
//!   faster than a silent 500.

use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Handle to a running mock TMDB server. Drop to stop it.
pub struct MockTmdbServer {
    inner: MockServer,
}

impl MockTmdbServer {
    /// Start a fresh server and pre-stub the endpoints most tests
    /// rely on (trending + genres lists). Fixture-specific stubs are
    /// added by the test.
    pub async fn start() -> Self {
        let inner = MockServer::start().await;
        let server = Self { inner };
        server.stub_trending_defaults().await;
        server.stub_genres_defaults().await;
        server
    }

    /// URL to pass to `TmdbClient::with_base_url`. No trailing slash.
    pub fn base_url(&self) -> String {
        self.inner.uri()
    }

    /// Stub the TMDB `/movie/{id}` response with a fixture file.
    /// Fixture is looked up as `fixtures/tmdb/movie-{id}.json`; tests
    /// that need a different name can use [`stub_movie_body`].
    pub async fn stub_movie(&self, id: i64) {
        let body = load_fixture(&format!("movie-{id}.json"));
        self.stub_movie_body(id, &body).await;
    }

    pub async fn stub_movie_body(&self, id: i64, body_json: &str) {
        let body: serde_json::Value = serde_json::from_str(body_json).expect("fixture JSON parses");
        Mock::given(method("GET"))
            .and(path(format!("/movie/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.inner)
            .await;
    }

    /// Same pattern for TV shows.
    pub async fn stub_show(&self, id: i64) {
        let body = load_fixture(&format!("show-{id}.json"));
        let val: serde_json::Value = serde_json::from_str(&body).expect("fixture JSON parses");
        Mock::given(method("GET"))
            .and(path(format!("/tv/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(val))
            .mount(&self.inner)
            .await;
    }

    /// Stub a specific season of a show. Fixture at
    /// `fixtures/tmdb/show-{id}-season-{n}.json`.
    pub async fn stub_season(&self, show_id: i64, season: i64) {
        let body = load_fixture(&format!("show-{show_id}-season-{season}.json"));
        let val: serde_json::Value = serde_json::from_str(&body).expect("fixture JSON parses");
        Mock::given(method("GET"))
            .and(path(format!("/tv/{show_id}/season/{season}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(val))
            .mount(&self.inner)
            .await;
    }

    /// Stub the `/{kind}/{id}/images` response with an empty `logos`
    /// array. Simulates TMDB having no clearlogo for the entity —
    /// the lazy-fetch path should then write the `""` sentinel to
    /// the DB so the next request returns 404 without re-hitting TMDB.
    pub async fn stub_empty_logos(&self, kind: &str, id: i64) {
        Mock::given(method("GET"))
            .and(path(format!("/{kind}/{id}/images")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "logos": [] })),
            )
            .mount(&self.inner)
            .await;
    }

    /// Make every `/tv/*` request 404 — useful when a test wants to
    /// assert on show-fetch-failure behaviour without individually
    /// stubbing each id.
    pub async fn stub_show_404_all(&self) {
        Mock::given(method("GET"))
            .and(path_regex(r"^/tv/\d+$"))
            .respond_with(ResponseTemplate::new(404).set_body_json(
                serde_json::json!({ "status_code": 34, "status_message": "not found" }),
            ))
            .mount(&self.inner)
            .await;
    }

    /// Default trending-list stubs returning an empty results page.
    /// Tests that want specific content override via a targeted stub.
    async fn stub_trending_defaults(&self) {
        let empty = serde_json::json!({
            "page": 1,
            "results": [],
            "total_pages": 0,
            "total_results": 0
        });
        for path_str in ["/trending/movie/week", "/trending/tv/week"] {
            Mock::given(method("GET"))
                .and(path(path_str))
                .respond_with(ResponseTemplate::new(200).set_body_json(empty.clone()))
                .mount(&self.inner)
                .await;
        }
    }

    async fn stub_genres_defaults(&self) {
        let body = serde_json::json!({
            "genres": [
                { "id": 28, "name": "Action" },
                { "id": 18, "name": "Drama" },
                { "id": 878, "name": "Science Fiction" }
            ]
        });
        for path_str in ["/genre/movie/list", "/genre/tv/list"] {
            Mock::given(method("GET"))
                .and(path(path_str))
                .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
                .mount(&self.inner)
                .await;
        }
    }
}

impl std::fmt::Debug for MockTmdbServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockTmdbServer")
            .field("uri", &self.inner.uri())
            .finish()
    }
}

fn load_fixture(name: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/test_support/fixtures/tmdb")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("load TMDB fixture {}: {e}", path.display()))
}
