//! HTTP client for Trakt. Thin wrapper over `reqwest` that handles:
//!
//!   - required Trakt-v2 headers (`trakt-api-version`, `trakt-api-key`)
//!   - Bearer auth from the `trakt_auth` row
//!   - automatic token refresh on 401 + retry once (covers clock skew
//!     and server-side revocation in one code path — the stored
//!     `expires_at` is an optimisation, not the source of truth)
//!   - mutex-serialised refresh so two concurrent requests can't race
//!     and invalidate each other's `refresh_token`
//!   - conservative POST-rate limiting (1 request/sec, Trakt's
//!     documented write cap)
//!
//! `TraktClient` is cheap to clone — internally it's a pair of Arcs.
// Refresh-req body structs are declared next to the request they
// build; same rationale as the `sync` module.
#![allow(clippy::items_after_statements)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Serialize, de::DeserializeOwned};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, broadcast};

use super::types::AccessToken;
use crate::events::AppEvent;

/// Production Trakt base URL. Used as the default when `from_db`
/// constructs the client; `with_base_url` lets tests override.
pub const DEFAULT_TRAKT_API: &str = "https://api.trakt.tv";

/// Errors every callsite cares about. Transport-level failures are
/// `Transport`; HTTP failures carry the status so handlers can
/// map (e.g.) 404 → "not configured on Trakt" without parsing strings.
#[derive(Debug, thiserror::Error)]
pub enum TraktError {
    #[error("Trakt not configured — no client_id/secret in config")]
    NotConfigured,
    #[error("Trakt not connected — no OAuth tokens stored")]
    NotConnected,
    /// Distinct from a generic `Api{status:400}` — fired when the
    /// server returns `invalid_grant` on a refresh exchange. The
    /// stored refresh token is dead and the user must re-authorise
    /// via the device-code flow. `refresh_now` clears `trakt_auth`
    /// and emits `AppEvent::TraktDisconnected` before returning this
    /// so background callers don't need to coordinate cleanup.
    #[error("Trakt auth expired — user must reconnect")]
    AuthExpired,
    #[error("Trakt API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Clone)]
pub struct TraktClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TraktClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the client_secret — Debug output ends up in
        // logs. Also don't print the token — fetched lazily from the
        // DB, not stored on the client.
        f.debug_struct("TraktClient").finish_non_exhaustive()
    }
}

struct Inner {
    http: reqwest::Client,
    db: SqlitePool,
    client_id: String,
    client_secret: String,
    /// Base URL (no trailing slash). Normally the production
    /// constant; overridable for integration tests that route through
    /// a wiremock instance.
    base_url: String,
    /// Serialises token refresh so two concurrent 401s can't both
    /// redeem the `refresh_token` (Trakt invalidates it on first use).
    refresh_lock: Mutex<()>,
    /// Wall-clock gate between POST requests. Trakt's documented
    /// write cap is 1/sec; we enforce on the whole client since
    /// we're single-user.
    post_gate: Mutex<Option<Instant>>,
    /// Optional event bus handle so the client can emit
    /// `TraktDisconnected` when it drops expired auth. Left `None`
    /// for unit-test constructions where we don't care about
    /// broadcasts; production call sites that have an `AppState`
    /// should construct via `from_state` to wire this up.
    event_tx: Option<broadcast::Sender<AppEvent>>,
}

impl TraktClient {
    /// Build a client for the currently-configured Trakt app. Returns
    /// `NotConfigured` when the user hasn't entered app credentials;
    /// callers (scheduler tasks, endpoints) short-circuit cleanly.
    pub async fn from_db(db: SqlitePool) -> Result<Self, TraktError> {
        Self::from_db_with_base(db, DEFAULT_TRAKT_API.to_owned()).await
    }

    /// Variant for tests: same construction path but routes requests
    /// at `base_url` instead of the production default.
    pub async fn from_db_with_base(db: SqlitePool, base_url: String) -> Result<Self, TraktError> {
        let (client_id, client_secret) = super::load_app_credentials(&db)
            .await
            .ok_or(TraktError::NotConfigured)?;
        // Timeout is generous because `/sync/*` bulk endpoints can be
        // multi-megabyte on heavy users. The OAuth + scrobble paths
        // are sub-second.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("kino/0.1.0")
            .build()
            .map_err(TraktError::Transport)?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                db,
                client_id,
                client_secret,
                base_url,
                refresh_lock: Mutex::new(()),
                post_gate: Mutex::new(None),
                event_tx: None,
            }),
        })
    }

    /// Wire an event bus onto the client so `refresh_now` can emit
    /// `TraktDisconnected` when the stored refresh token is rejected.
    /// Call sites with an `AppState` available should use this; the
    /// client still functions without it (silent cleanup), which
    /// keeps test harness callers free of the event channel.
    #[must_use]
    pub fn with_event_tx(mut self, tx: broadcast::Sender<AppEvent>) -> Self {
        // We own the only Arc right after `from_db` returns, so the
        // `Arc::get_mut` here is infallible for the standard
        // construction flow. If an owner has already cloned the
        // client, we fall back to a no-op — the cost is just silent
        // refresh cleanup, which matches pre-wire behaviour.
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.event_tx = Some(tx);
        } else {
            tracing::warn!(
                "TraktClient::with_event_tx called on cloned Arc — \
                 TraktDisconnected will not broadcast for this client"
            );
        }
        self
    }

    pub fn client_id(&self) -> &str {
        &self.inner.client_id
    }

    pub fn client_secret(&self) -> &str {
        &self.inner.client_secret
    }

    pub fn db(&self) -> &SqlitePool {
        &self.inner.db
    }

    // ── Public request helpers ────────────────────────────────────

    /// GET with Bearer auth. Refreshes + retries once on 401.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, TraktError> {
        let bytes = self
            .request_bytes(reqwest::Method::GET, path, None::<&()>)
            .await?;
        serde_json::from_slice(&bytes).map_err(TraktError::Serde)
    }

    /// POST with Bearer auth. Subject to the 1/sec write gate.
    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, TraktError> {
        self.wait_post_gate().await;
        let bytes = self
            .request_bytes(reqwest::Method::POST, path, Some(body))
            .await?;
        if bytes.is_empty() {
            // Some endpoints (e.g. `/oauth/revoke`) return 200 with no
            // body — deserialise to an empty JSON object so `()` /
            // `SyncResult` callers don't error. Fine because the only
            // T used for no-body responses is `()` which serialises
            // from `null` via serde's default behaviour.
            return serde_json::from_slice(b"null").map_err(TraktError::Serde);
        }
        serde_json::from_slice(&bytes).map_err(TraktError::Serde)
    }

    /// POST that discards the response. Same rate-limiting applies.
    pub async fn post_noreply<B: Serialize>(&self, path: &str, body: &B) -> Result<(), TraktError> {
        self.wait_post_gate().await;
        self.request_bytes(reqwest::Method::POST, path, Some(body))
            .await?;
        Ok(())
    }

    /// Public-data GET: sets the API key header but skips the Bearer
    /// token. Used by trending + connect-flow endpoints that don't
    /// need user auth. Not subject to 401-retry since there's no
    /// token to refresh.
    pub async fn get_public<T: DeserializeOwned>(&self, path: &str) -> Result<T, TraktError> {
        let url = format!("{}{path}", self.inner.base_url);
        let resp = self
            .inner
            .http
            .get(&url)
            .header("Content-Type", "application/json")
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &self.inner.client_id)
            .send()
            .await?;
        check_status(resp).await?.json().await.map_err(Into::into)
    }

    /// Public-data POST for the device-code bootstrap
    /// (`/oauth/device/code` + `/oauth/device/token`). Same rationale
    /// as `get_public`: no Bearer yet.
    pub async fn post_public<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, TraktError> {
        let url = format!("{}{path}", self.inner.base_url);
        let resp = self
            .inner
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &self.inner.client_id)
            .json(body)
            .send()
            .await?;
        check_status(resp).await?.json().await.map_err(Into::into)
    }

    /// Low-level POST that returns the raw status + body without
    /// treating non-2xx as an error. Used by the device-code poller
    /// which needs to distinguish 400 (pending), 404 (code not
    /// found), 409 (already used), etc.
    pub async fn post_public_raw<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(u16, bytes::Bytes), TraktError> {
        let url = format!("{}{path}", self.inner.base_url);
        let resp = self
            .inner
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &self.inner.client_id)
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await?;
        Ok((status, body))
    }

    // ── Core request flow ─────────────────────────────────────────

    async fn request_bytes<B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<bytes::Bytes, TraktError> {
        let url = format!("{}{path}", self.inner.base_url);
        let token = self.current_token().await?;

        let send = |tok: String| {
            let mut req = self
                .inner
                .http
                .request(method.clone(), &url)
                .header("Content-Type", "application/json")
                .header("trakt-api-version", "2")
                .header("trakt-api-key", &self.inner.client_id)
                .header("Authorization", format!("Bearer {tok}"));
            if let Some(b) = body {
                req = req.json(b);
            }
            req.send()
        };

        let mut tok = token;
        // Small retry budget for 429s. Trakt caps the whole client at
        // 200 req/min; we already serialise POSTs through `post_gate`
        // but GETs share the same bucket on their side and a genuine
        // rate-limit still surfaces as a 429. Honour `Retry-After`
        // when present (seconds, per RFC 7231) and otherwise back off
        // a conservative 2s. Cap at 2 retries: past that, fail the
        // call so the caller decides whether to retry at the task
        // level.
        for attempt in 0..3 {
            let resp = send(tok.clone()).await?;
            let status = resp.status().as_u16();

            if status == 401 {
                tracing::info!(path, "trakt 401 — refreshing token");
                tok = self.refresh_now().await?;
                let retry = send(tok.clone()).await?;
                return check_status(retry).await?.bytes().await.map_err(Into::into);
            }

            if status == 429 {
                let retry_after_secs = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(2);
                if attempt < 2 {
                    tracing::warn!(
                        path,
                        retry_after_secs,
                        attempt,
                        "trakt 429 — sleeping before retry"
                    );
                    tokio::time::sleep(Duration::from_secs(retry_after_secs.min(30))).await;
                    continue;
                }
                tracing::warn!(
                    path,
                    retry_after_secs,
                    "trakt 429 — retries exhausted, bubbling error"
                );
                return Err(TraktError::Api {
                    status: 429,
                    message: format!("rate limited, retry-after {retry_after_secs}s"),
                });
            }

            return check_status(resp).await?.bytes().await.map_err(Into::into);
        }
        Err(TraktError::Api {
            status: 429,
            message: "trakt rate limit retries exhausted".into(),
        })
    }

    /// Read the current `access_token`, or error if we've never
    /// connected. Does NOT auto-refresh on `expires_at` because the
    /// 401-retry loop is the authoritative refresh trigger.
    async fn current_token(&self) -> Result<String, TraktError> {
        sqlx::query_scalar::<_, String>("SELECT access_token FROM trakt_auth WHERE id = 1")
            .fetch_optional(&self.inner.db)
            .await?
            .ok_or(TraktError::NotConnected)
    }

    /// Exchange the `refresh_token` for a new `access_token`, serialised
    /// against other concurrent refreshers. Persists the result.
    pub async fn refresh_now(&self) -> Result<String, TraktError> {
        let _guard = self.inner.refresh_lock.lock().await;

        // Re-read the token under the lock — if another task beat us
        // to it, we skip the network call entirely.
        let stored: Option<(String, String)> =
            sqlx::query_as("SELECT access_token, refresh_token FROM trakt_auth WHERE id = 1")
                .fetch_optional(&self.inner.db)
                .await?;
        let (access, refresh) = stored.ok_or(TraktError::NotConnected)?;

        // Cheap no-op: caller already has a valid token because the
        // 401 came from a stale copy. The retry will get the fresh
        // one. In practice this path only hits when two tasks racing
        // on the same 401 arrive at the lock at the same time.
        //
        // We can't verify without a round-trip, so we just always
        // refresh. The cost is an extra HTTP call in the rare race
        // case; no correctness issue.
        let _ = access;

        #[derive(Serialize)]
        struct RefreshReq<'a> {
            refresh_token: &'a str,
            client_id: &'a str,
            client_secret: &'a str,
            redirect_uri: &'a str,
            grant_type: &'a str,
        }
        let req = RefreshReq {
            refresh_token: &refresh,
            client_id: &self.inner.client_id,
            client_secret: &self.inner.client_secret,
            // Device-code grant uses this "out-of-band" redirect even
            // for refresh. Matches Trakt's documented behaviour.
            redirect_uri: "urn:ietf:wg:oauth:2.0:oob",
            grant_type: "refresh_token",
        };
        let resp = self
            .inner
            .http
            .post(format!("{}/oauth/token", self.inner.base_url))
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            // Trakt returns 400 with `{"error":"invalid_grant", ...}`
            // when the stored refresh token is no longer valid —
            // usually because the user revoked the app, or because
            // another Trakt session refreshed first and invalidated
            // this refresh token. Either way we can't recover; wipe
            // the auth row and fire `TraktDisconnected` so the UI
            // prompts the user to reconnect.
            if code == 400 && body.to_ascii_lowercase().contains("invalid_grant") {
                tracing::warn!(
                    status = code,
                    body = %body,
                    "trakt refresh rejected — clearing stored auth and firing TraktDisconnected"
                );
                // Best-effort wipe. Errors here aren't fatal; the next
                // call will still return NotConnected once the row is
                // gone, and if the wipe fails the user can disconnect
                // manually from settings.
                if let Err(e) = sqlx::query("DELETE FROM trakt_auth WHERE id = 1")
                    .execute(&self.inner.db)
                    .await
                {
                    tracing::warn!(error = %e, "failed to wipe trakt_auth after invalid_grant");
                }
                if let Some(tx) = &self.inner.event_tx {
                    let _ = tx.send(AppEvent::TraktDisconnected);
                }
                return Err(TraktError::AuthExpired);
            }
            return Err(TraktError::Api {
                status: code,
                message: body,
            });
        }
        let tok: AccessToken = resp.json().await?;

        persist_token(&self.inner.db, &tok).await?;
        tracing::info!("trakt token refreshed");
        Ok(tok.access_token)
    }

    async fn wait_post_gate(&self) {
        let mut gate = self.inner.post_gate.lock().await;
        let now = Instant::now();
        if let Some(last) = *gate {
            let elapsed = now.duration_since(last);
            if elapsed < Duration::from_millis(1000) {
                tokio::time::sleep(Duration::from_millis(1000).checked_sub(elapsed).unwrap()).await;
            }
        }
        *gate = Some(Instant::now());
    }
}

/// Interpret an HTTP response as `Ok` on 2xx, `Err(TraktError::Api)`
/// otherwise. Consumes the response when erroring so the body can be
/// included in the error message — helpful when Trakt returns a JSON
/// error envelope.
async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, TraktError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    Err(TraktError::Api {
        status: code,
        message: body,
    })
}

/// Persist a freshly-minted or freshly-refreshed token. Called by both
/// the device-code flow (on initial connect) and the refresh path.
/// `connected_at` is only set on initial connect — refreshes preserve
/// the original value so the "Connected since" string stays stable.
pub(crate) async fn persist_token(db: &SqlitePool, tok: &AccessToken) -> Result<(), TraktError> {
    let expires_at = chrono::DateTime::from_timestamp(tok.created_at + tok.expires_in, 0)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339();
    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO trakt_auth (id, access_token, refresh_token, expires_at, token_scope, connected_at)
         VALUES (1, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            access_token  = excluded.access_token,
            refresh_token = excluded.refresh_token,
            expires_at    = excluded.expires_at,
            token_scope   = excluded.token_scope",
    )
    .bind(&tok.access_token)
    .bind(&tok.refresh_token)
    .bind(&expires_at)
    .bind(&tok.scope)
    .bind(&now)
    .execute(db)
    .await?;
    Ok(())
}
