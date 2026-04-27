//! Wiremock-backed fake Trakt server. Paired with
//! `TraktClient::from_db_with_base` to route integration-test
//! requests away from the real `api.trakt.tv`.
//!
//! Fixtures live under `fixtures/trakt/`; endpoints are stubbed
//! one-off per test (no "default" set like TMDB's trending) because
//! Trakt-using tests are fewer and each knows which endpoints it
//! needs.

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct MockTraktServer {
    inner: MockServer,
}

impl MockTraktServer {
    pub async fn start() -> Self {
        Self {
            inner: MockServer::start().await,
        }
    }

    pub fn base_url(&self) -> String {
        self.inner.uri()
    }

    /// Stub `/users/settings` — the "is connected" probe most flows
    /// run first. `username` goes in the expected JSON shape so
    /// `TraktClient` sees the user as authenticated.
    pub async fn stub_settings(&self, username: &str) {
        let body = serde_json::json!({
            "user": { "username": username, "ids": { "slug": username } },
            "account": { "timezone": "UTC" }
        });
        Mock::given(method("GET"))
            .and(path("/users/settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.inner)
            .await;
    }

    /// Stub `/sync/last_activities` — incremental sync's watermark
    /// endpoint. `activities` is the JSON shape Trakt returns.
    pub async fn stub_last_activities(&self, activities: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/sync/last_activities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(activities))
            .mount(&self.inner)
            .await;
    }

    /// Generic path stub — used when the specific endpoint doesn't
    /// warrant a helper.
    pub async fn stub_path(&self, http_method: &str, path_str: &str, body: serde_json::Value) {
        Mock::given(method(http_method))
            .and(path(path_str))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.inner)
            .await;
    }

    /// Simulate an OAuth `Bearer` requirement — any request missing
    /// the header returns 401. Tests asserting on token-refresh
    /// behaviour use this.
    pub async fn require_bearer(&self) {
        Mock::given(header("authorization", "Bearer invalid"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&self.inner)
            .await;
    }
}

impl std::fmt::Debug for MockTraktServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockTraktServer")
            .field("uri", &self.inner.uri())
            .finish()
    }
}
