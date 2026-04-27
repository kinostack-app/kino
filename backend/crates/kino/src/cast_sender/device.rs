//! Database row + API response shapes for `cast_device` and
//! `cast_session` tables.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;

/// A discovered (or manually-added) Chromecast on the LAN.
///
/// Two `source` flavours: `mdns` rows are ephemeral (re-discovered
/// on startup, dropped from the DB if not re-announced within the
/// stale window) and `manual` rows persist (the user added it by
/// IP for a network where mDNS is blocked).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct CastDevice {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: i64,
    pub model: Option<String>,
    /// `mdns` | `manual`
    pub source: String,
    pub last_seen: Option<String>,
    pub created_at: String,
}

/// A live (or recently-ended) Cast session targeting one device.
///
/// `transport_id` + `session_id` are populated once `launch_app`
/// returns from the receiver; they're the handles the sender uses
/// to address the running app (and to rejoin it after a backend
/// restart, in Phase 2).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct CastSession {
    pub id: String,
    pub device_id: String,
    pub transport_id: Option<String>,
    pub session_id: Option<String>,
    pub media_id: Option<i64>,
    /// Stored as `TEXT` in `SQLite` so sqlx reads it as `String`;
    /// the `value_type` override tells utoipa to emit the typed
    /// [`CastSessionStatus`] enum in the `OpenAPI` schema so the
    /// frontend gets the narrow union.
    #[schema(value_type = CastSessionStatus)]
    pub status: String,
    pub last_status_json: Option<String>,
    pub last_position_ms: Option<i64>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

/// Lifecycle of a [`CastSession`]. Stored as the lower-case string
/// in `cast_session.status`; the strings are part of the API
/// contract (frontend matches on them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CastSessionStatus {
    /// `connect()` + `launch_app()` in flight; no transport id yet.
    Starting,
    /// `launch_app()` returned; player is responsive.
    Active,
    /// User stopped the session, or the receiver app exited.
    Ended,
    /// Reconnect ladder exhausted, or the receiver returned a
    /// `LAUNCH_ERROR` we couldn't recover from.
    Errored,
}

impl CastSessionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Active => "active",
            Self::Ended => "ended",
            Self::Errored => "errored",
        }
    }
}
