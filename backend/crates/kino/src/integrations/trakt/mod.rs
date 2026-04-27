//! Trakt integration — OAuth device-code flow, scrobble, bulk +
//! incremental sync, recommendations. Spec: `docs/subsystems/16-trakt.md`.
//!
//! Public surface is deliberately flat: callers use `auth::*`,
//! `sync::*`, `scrobble::*` directly. The only cross-module type is
//! [`TraktError`]; everything else is module-private.

pub mod auth;
pub mod client;
pub mod handlers;
pub mod reconcile;
pub mod scrobble;
pub mod scrobble_state;
pub mod sync;
pub mod types;

pub use client::{TraktClient, TraktError};
pub use scrobble_state::ScrobbleState;

/// Convenience constructor for call sites that have an `AppState`
/// handy — wires the event bus so `refresh_now` can broadcast
/// `TraktDisconnected` when the stored refresh token is invalidated.
/// Tests + unit contexts that don't care about the event channel
/// keep using `TraktClient::from_db` directly.
pub async fn client_for(state: &crate::state::AppState) -> Result<TraktClient, TraktError> {
    let client = TraktClient::from_db(state.db.clone()).await?;
    Ok(client.with_event_tx(state.event_tx.clone()))
}

/// Load the user's configured Trakt app credentials from the `config`
/// table. Returns `None` if the user hasn't registered an app yet —
/// every callable in this module should bail early on `None` rather
/// than erroring, because "not configured" is a valid state (Trakt is
/// optional).
pub async fn load_app_credentials(db: &sqlx::SqlitePool) -> Option<(String, String)> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT trakt_client_id, trakt_client_secret FROM config WHERE id = 1")
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    match row {
        Some((Some(id), Some(secret))) if !id.is_empty() && !secret.is_empty() => {
            Some((id, secret))
        }
        _ => None,
    }
}

/// True when Trakt is both configured (`client_id` + secret present) and
/// currently authenticated (a `trakt_auth` row exists). Used by sync
/// tasks + scrobble pathways to short-circuit cleanly on disconnected
/// installs.
pub async fn is_connected(db: &sqlx::SqlitePool) -> bool {
    if load_app_credentials(db).await.is_none() {
        return false;
    }
    sqlx::query_scalar::<_, i64>("SELECT 1 FROM trakt_auth WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .is_some()
}
