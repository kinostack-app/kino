//! Wiremock-backed fake Torznab indexer for integration tests.
//!
//! Torznab responses are XML, not JSON. Fixtures live under
//! `fixtures/torznab/` as `.xml` files; the server replies with
//! them verbatim (bytes + `application/xml` content-type).
//!
//! Tests usually install one of these then INSERT an `indexer` row
//! pointing at `server.base_url()` — kino reads indexer URLs from
//! the DB per-row, so no global override is needed.

use wiremock::matchers::{any, method, query_param, query_param_contains};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct MockTorznabServer {
    inner: MockServer,
}

impl MockTorznabServer {
    pub async fn start() -> Self {
        let inner = MockServer::start().await;
        let server = Self { inner };
        // Capabilities is always queried first — stub a minimal "TV
        // and movies supported" shape so the indexer probe passes.
        server.stub_capabilities().await;
        server
    }

    pub fn base_url(&self) -> String {
        self.inner.uri()
    }

    /// Stub `?t=caps` — Torznab capabilities probe. Matches any path
    /// with `t=caps` in the query string since indexers expose the
    /// Torznab endpoint at root, `/api`, `/api/v2.0/indexers/.../results`,
    /// etc. Kino's `TorznabClient` hits `{base_url}?t=search|caps`
    /// directly, so we key on the `t=` query param, not the path.
    async fn stub_capabilities(&self) {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server version="1.0" title="mock" />
  <limits default="50" max="100" />
  <searching>
    <search available="yes" supportedParams="q" />
    <tv-search available="yes" supportedParams="q,season,ep,tvdbid,imdbid" />
    <movie-search available="yes" supportedParams="q,imdbid" />
  </searching>
  <categories>
    <category id="2000" name="Movies" />
    <category id="5000" name="TV" />
  </categories>
</caps>"#;
        Mock::given(method("GET"))
            .and(query_param("t", "caps"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(xml)
                    .insert_header("content-type", "application/xml"),
            )
            .mount(&self.inner)
            .await;
    }

    /// Stub the `t=search` (and `t=tvsearch` / `t=movie`) path with
    /// raw XML bytes. Tests load a fixture and pass it in.
    pub async fn stub_search_xml(&self, xml: &str) {
        // Match any GET on this server that *isn't* a caps probe —
        // cheaper than three separate Mock registrations (search,
        // tvsearch, movie). Wiremock later mounts take priority, so
        // caps still wins for `t=caps` requests.
        Mock::given(method("GET"))
            .and(any())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(xml.to_owned())
                    .insert_header("content-type", "application/xml"),
            )
            .mount(&self.inner)
            .await;
    }

    /// Helper that loads a fixture + installs it as the search
    /// response in one call.
    pub async fn stub_search_fixture(&self, name: &str) {
        let xml = load_fixture(name);
        self.stub_search_xml(&xml).await;
    }

    /// Stub a search response scoped to queries whose `q=` contains
    /// `needle`. Useful when one test wants two different fixtures
    /// (one per title) without racing earlier `stub_search_xml`
    /// mounts. Wiremock prioritises *later* mounts, so call this
    /// after any generic `stub_search_xml` if the per-query branch
    /// should win.
    pub async fn stub_search_for_query(&self, needle: &str, xml: &str) {
        Mock::given(method("GET"))
            .and(query_param_contains("q", needle))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(xml.to_owned())
                    .insert_header("content-type", "application/xml"),
            )
            .mount(&self.inner)
            .await;
    }
}

impl std::fmt::Debug for MockTorznabServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockTorznabServer")
            .field("uri", &self.inner.uri())
            .finish()
    }
}

fn load_fixture(name: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/test_support/fixtures/torznab")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("load Torznab fixture {}: {e}", path.display()))
}
