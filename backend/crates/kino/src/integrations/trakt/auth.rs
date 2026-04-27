//! Trakt OAuth device-code flow. See
//! `docs/subsystems/16-trakt.md` § Authentication.
//!
//! The device-code grant is a two-step dance: the user visits a URL
//! on any device, enters an 8-character code shown by kino, and
//! approves. Meanwhile kino polls a token endpoint every N seconds
//! until Trakt swaps the `device_code` for an access/refresh pair.
//!
//! We expose the flow as two functions:
//!   - [`begin`] — start the flow, return the user-facing code + URL
//!     the frontend displays
//!   - [`poll_token`] — called repeatedly by the frontend (or a
//!     background task) until success/failure
//!
//! No polling loop lives here — the handler in `api/trakt.rs` owns
//! the polling cadence + timeout. This keeps this module stateless.

use serde::Serialize;
use sqlx::SqlitePool;

use super::client::{TraktClient, TraktError, persist_token};
use super::types::{AccessToken, DeviceCode, UserSettings};

/// Start a device-code flow. Returns the `user_code` + `verification_url`
/// the frontend displays, plus the internal `device_code` used for
/// subsequent polls.
pub async fn begin(client: &TraktClient) -> Result<DeviceCode, TraktError> {
    #[derive(Serialize)]
    struct Req<'a> {
        client_id: &'a str,
    }
    client
        .post_public(
            "/oauth/device/code",
            &Req {
                client_id: client.client_id(),
            },
        )
        .await
}

/// Outcome of a single poll.
#[derive(Debug)]
pub enum PollOutcome {
    /// User hasn't approved yet. Caller should wait `interval`
    /// seconds and call again.
    Pending,
    /// Approved — token persisted, user identity fetched + stored.
    Connected { username: String, slug: String },
    /// Trakt said the code is no longer usable (expired / denied /
    /// already-redeemed). Caller restarts the flow from `begin`.
    Invalid(String),
}

/// Poll `/oauth/device/token` once. Translates the Trakt status-code
/// protocol into a strongly-typed outcome so the handler doesn't
/// duplicate the lookup table.
///
/// Status codes (per Trakt docs):
///   200 — success (token)
///   400 — pending (user hasn't approved)
///   404 — `device_code` not found (expired server-side)
///   409 — already used
///   410 — expired
///   418 — user denied
///   429 — slow down (we honour interval + optional Retry-After)
pub async fn poll_token(
    client: &TraktClient,
    device_code: &str,
) -> Result<PollOutcome, TraktError> {
    #[derive(Serialize)]
    struct Req<'a> {
        code: &'a str,
        client_id: &'a str,
        client_secret: &'a str,
    }
    let (status, body) = client
        .post_public_raw(
            "/oauth/device/token",
            &Req {
                code: device_code,
                client_id: client.client_id(),
                client_secret: client.client_secret(),
            },
        )
        .await?;

    match status {
        200 => {
            let tok: AccessToken = serde_json::from_slice(&body)?;
            persist_token(client.db(), &tok).await?;
            // Identity fetch is best-effort — if it fails, the
            // connection still works; we just show "Connected"
            // without a username.
            let outcome = match fetch_identity(client).await {
                Ok((username, slug)) => {
                    update_identity(client.db(), &username, &slug).await?;
                    PollOutcome::Connected { username, slug }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "trakt identity fetch failed");
                    PollOutcome::Connected {
                        username: String::new(),
                        slug: String::new(),
                    }
                }
            };
            // The Trakt-watchlist system list (subsystem 17) is
            // bootstrapped by the caller — they have the event
            // broadcaster we need for the ListAutoAdded notification.
            Ok(outcome)
        }
        // 400 = pending (user hasn't approved) and 429 = slow down
        // collapse to the same "keep polling" outcome — the caller
        // honours the interval either way. 404/409/410/418 are all
        // terminal "this code is unusable" states with slightly
        // different stories; we surface the user-visible distinction
        // in the `Invalid` reason string.
        400 | 429 => Ok(PollOutcome::Pending),
        404 | 410 => Ok(PollOutcome::Invalid("code expired".into())),
        409 => Ok(PollOutcome::Invalid("code already used".into())),
        418 => Ok(PollOutcome::Invalid("access denied".into())),
        other => Err(TraktError::Api {
            status: other,
            message: String::from_utf8_lossy(&body).into_owned(),
        }),
    }
}

/// Revoke + delete local state. Best-effort: if Trakt is unreachable
/// we still clear locally so the user isn't stuck in a "pending
/// disconnect" state. The revoke call is rate-limited as a POST.
pub async fn disconnect(client: &TraktClient) -> Result<(), TraktError> {
    let access: Option<String> =
        sqlx::query_scalar("SELECT access_token FROM trakt_auth WHERE id = 1")
            .fetch_optional(client.db())
            .await?;

    if let Some(token) = access {
        #[derive(Serialize)]
        struct Req<'a> {
            token: &'a str,
            client_id: &'a str,
            client_secret: &'a str,
        }
        // Fire-and-forget — a failed revoke server-side is a log line,
        // not an error. The local DB delete below is the thing that
        // matters for UX ("disconnect" should always succeed locally).
        if let Err(e) = client
            .post_noreply(
                "/oauth/revoke",
                &Req {
                    token: &token,
                    client_id: client.client_id(),
                    client_secret: client.client_secret(),
                },
            )
            .await
        {
            tracing::warn!(error = %e, "trakt revoke failed — clearing local state anyway");
        }
    }

    // Clear both the auth row and the sync state (pull-side tombstones
    // + recommendations cache) so a subsequent connect does a fresh
    // initial import rather than silently resuming where the previous
    // identity left off.
    sqlx::query("DELETE FROM trakt_auth WHERE id = 1")
        .execute(client.db())
        .await?;
    sqlx::query("DELETE FROM trakt_sync_state WHERE id = 1")
        .execute(client.db())
        .await?;
    sqlx::query("DELETE FROM trakt_scrobble_queue")
        .execute(client.db())
        .await?;
    Ok(())
}

async fn fetch_identity(client: &TraktClient) -> Result<(String, String), TraktError> {
    let settings: UserSettings = client.get("/users/settings").await?;
    Ok((settings.user.username, settings.user.ids.slug))
}

async fn update_identity(db: &SqlitePool, username: &str, slug: &str) -> Result<(), TraktError> {
    sqlx::query("UPDATE trakt_auth SET username = ?, slug = ? WHERE id = 1")
        .bind(username)
        .bind(slug)
        .execute(db)
        .await?;
    Ok(())
}
