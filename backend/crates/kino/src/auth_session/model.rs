//! Per-device authentication sessions. The master credential is the
//! `config.api_key`; everything else (browser cookies, named CLI
//! tokens, QR-code bootstrap exchanges) lives in the `session` table
//! and is independently revocable.

use serde::Serialize;
use utoipa::ToSchema;

/// One row in `session`. `id` doubles as the cookie value / bearer
/// token — 32 random bytes URL-safe-base64-encoded — and is compared
/// in constant time on every request.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub source: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub consumed_at: Option<String>,
}

/// API-shape mirror of `Session` for the Devices page. Drops fields
/// the UI doesn't render and converts the source string into a
/// typed enum so the frontend can branch cleanly.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SessionView {
    pub id: String,
    pub label: String,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub source: SessionSource,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    /// True when this session is the one making the current request —
    /// drives the "this device" badge and disables the Revoke button
    /// on its own row.
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SessionSource {
    /// Browser session, created by exchanging the master API key via
    /// the paste-the-key flow on a new origin or device.
    Browser,
    /// Named long-lived token issued from Settings → Devices for a
    /// CLI script / external integration.
    Cli,
    /// Browser session created by scanning a QR code from another
    /// already-signed-in device. Same lifetime as `Browser`; tracked
    /// separately so the audit story shows how the device was paired.
    QrBootstrap,
    /// One-time, short-lived token row issued by
    /// `POST /sessions/bootstrap-token`, awaiting redemption by a
    /// scanning device. Hidden from the Devices list.
    BootstrapPending,
    /// Auto-issued cookie for same-machine localhost requests so the
    /// SPA "just works" the first time you visit. Lower trust than
    /// `Browser` (no key was actually presented) — limited lifetime
    /// and only created when the request originates from
    /// `127.0.0.1` / `::1`.
    AutoLocalhost,
}

impl SessionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Cli => "cli",
            Self::QrBootstrap => "qr-bootstrap",
            Self::BootstrapPending => "bootstrap-pending",
            Self::AutoLocalhost => "auto-localhost",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "browser" => Some(Self::Browser),
            "cli" => Some(Self::Cli),
            "qr-bootstrap" => Some(Self::QrBootstrap),
            "bootstrap-pending" => Some(Self::BootstrapPending),
            "auto-localhost" => Some(Self::AutoLocalhost),
            _ => None,
        }
    }
}

impl Session {
    /// Convert to the API view, marking the row as the current
    /// session when its id matches `current_id`.
    pub fn into_view(self, current_id: Option<&str>) -> SessionView {
        let current = current_id.is_some_and(|id| id == self.id);
        SessionView {
            current,
            source: SessionSource::parse(&self.source).unwrap_or(SessionSource::Browser),
            id: self.id,
            label: self.label,
            user_agent: self.user_agent,
            ip: self.ip,
            created_at: self.created_at,
            last_seen_at: self.last_seen_at,
            expires_at: self.expires_at,
        }
    }
}
