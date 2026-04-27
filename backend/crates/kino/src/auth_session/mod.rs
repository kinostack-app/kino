//! Session lifecycle helpers + signed-URL primitive. The `session`
//! table is the per-device authentication record; this module owns
//! the create / lookup / touch / revoke / expire queries plus the
//! HMAC sign+verify routine used for short-lived media URLs in
//! cross-origin deploys.
//!
//! Security notes:
//!   - Session ids are 32 random URL-safe-base64 bytes (256-bit
//!     entropy). Long enough to be brute-force-resistant; we also
//!     rate-limit the session-creation endpoint at the handler
//!     layer for belt-and-braces.
//!   - Signed URLs commit to method + path + expiry, so a sig
//!     issued for `GET /direct/123` cannot be replayed against
//!     `DELETE /movies/123`.
//!   - Comparison of cookie / sig values uses constant-time
//!     equality (`subtle::ConstantTimeEq`) — naive `==` leaks
//!     timing information on each compared byte.
//!
//! ## Public API
//!
//! - `model::{Session, SessionSource, SessionView}` — DB row + the
//!   redacted view the API surfaces
//! - Lifecycle: `create`, `lookup`, `is_valid`, `touch_last_seen`,
//!   `revoke`, `revoke_all`, `revoke_all_except`,
//!   `consume_bootstrap`, `list_active`, `generate_session_id`,
//!   `ids_eq` — used by `auth.rs` middleware + `handlers`
//! - Signed URLs: `sign_url`, `verify_signed_url`, `signing_secret`
//!   — used by playback's direct + HLS routes when running
//!   cross-origin
//! - `handlers::*` — HTTP surface (bootstrap / create / redeem /
//!   revoke / list / sign-url), registered via main.rs
//! - TTL constants — exposed for handlers + the QR-pair flow

pub mod handlers;
pub mod model;

use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

use crate::auth_session::model::{Session, SessionSource};

type HmacSha256 = Hmac<Sha256>;

/// Default lifetime for browser sessions. 30 days is a sensible
/// "remember me" duration — long enough for casual use, short
/// enough that a dormant cookie won't outlive the user's interest.
pub const BROWSER_SESSION_TTL_DAYS: i64 = 30;

/// Default lifetime for QR-bootstrap pending tokens. 5 minutes is
/// long enough to scan + walk to the new device, short enough to
/// shrink the replay window if the QR image is photographed.
pub const QR_BOOTSTRAP_TTL_MINS: i64 = 5;

/// Default lifetime for an auto-issued localhost cookie. Same
/// 30-day window as a normal browser session — once the SPA is
/// running on the same machine as the backend, there's no
/// meaningful difference.
pub const LOCALHOST_SESSION_TTL_DAYS: i64 = 30;

/// Default expiry on a new signed media URL. Long enough that a
/// scrub-back-and-forth doesn't repeatedly hit `/sign-url`, short
/// enough that a leaked sig dies quickly.
pub const SIGNED_URL_TTL_SECS: i64 = 15 * 60;

// ─── Session lifecycle ──────────────────────────────────────────

/// Generate a fresh 256-bit session id, URL-safe-base64 encoded.
pub fn generate_session_id() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Create + persist a session. Returns the row so the caller can
/// shape the response (Set-Cookie header / JSON body etc).
pub async fn create(
    db: &SqlitePool,
    label: String,
    user_agent: Option<String>,
    ip: Option<String>,
    source: SessionSource,
    ttl: chrono::Duration,
) -> Result<Session, sqlx::Error> {
    let now = crate::time::Timestamp::now();
    let id = generate_session_id();
    let row = Session {
        id: id.clone(),
        label,
        user_agent,
        ip,
        source: source.as_str().to_owned(),
        created_at: now.to_rfc3339(),
        last_seen_at: now.to_rfc3339(),
        expires_at: crate::time::Timestamp::now_plus(ttl).to_rfc3339(),
        consumed_at: None,
    };
    sqlx::query(
        "INSERT INTO session
            (id, label, user_agent, ip, source, created_at, last_seen_at, expires_at, consumed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(&row.id)
    .bind(&row.label)
    .bind(&row.user_agent)
    .bind(&row.ip)
    .bind(&row.source)
    .bind(&row.created_at)
    .bind(&row.last_seen_at)
    .bind(&row.expires_at)
    .execute(db)
    .await?;
    tracing::info!(
        session_id = %row.id,
        source = %row.source,
        label = %row.label,
        "session created"
    );
    Ok(row)
}

/// Fetch a session by id without any validation. Validity (expired?
/// consumed?) is the caller's responsibility — see [`is_valid`].
pub async fn lookup(db: &SqlitePool, id: &str) -> Option<Session> {
    sqlx::query_as::<_, Session>("SELECT * FROM session WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// Constant-time check that a cookie value matches a stored session
/// id. Used by handlers that resolve sessions outside the auth
/// middleware (e.g. WebSocket post-upgrade auth).
pub fn ids_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// True when the session is still active: not expired, not consumed
/// (for `bootstrap-pending` rows).
pub fn is_valid(s: &Session) -> bool {
    if s.consumed_at.is_some() {
        return false;
    }
    let Some(exp) = crate::time::Timestamp::parse(&s.expires_at) else {
        return false;
    };
    exp > crate::time::Timestamp::now()
}

/// Touch `last_seen_at` to "now". Fire-and-forget pattern — the
/// caller doesn't care about the result; failures only stale the
/// Devices page's last-seen column.
pub async fn touch_last_seen(db: &SqlitePool, id: &str) {
    let _ = sqlx::query("UPDATE session SET last_seen_at = ? WHERE id = ?")
        .bind(crate::time::Timestamp::now())
        .bind(id)
        .execute(db)
        .await;
}

/// Drop a session row by id. Returns the number of rows affected so
/// the caller can return 404 for an unknown id.
pub async fn revoke(db: &SqlitePool, id: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM session WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    if res.rows_affected() > 0 {
        tracing::info!(session_id = %id, "session revoked");
    }
    Ok(res.rows_affected())
}

/// Drop every session except the caller's own. Used by the
/// "log everything else out" affordance on the Devices page.
pub async fn revoke_all_except(db: &SqlitePool, keep_id: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM session WHERE id != ?")
        .bind(keep_id)
        .execute(db)
        .await?;
    if res.rows_affected() > 0 {
        tracing::info!(
            kept_session_id = %keep_id,
            removed = res.rows_affected(),
            "revoked all sessions except current"
        );
    }
    Ok(res.rows_affected())
}

/// Drop every session — used when the master API key rotates so a
/// stolen cookie can't outlive the credential it was derived from.
pub async fn revoke_all(db: &SqlitePool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM session").execute(db).await?;
    if res.rows_affected() > 0 {
        tracing::info!(removed = res.rows_affected(), "revoked all sessions");
    }
    Ok(res.rows_affected())
}

/// Mark a `bootstrap-pending` row consumed so it can't be redeemed
/// twice. Returns true when the row existed and was unconsumed.
pub async fn consume_bootstrap(db: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE session
            SET consumed_at = ?
          WHERE id = ?
            AND source = 'bootstrap-pending'
            AND consumed_at IS NULL",
    )
    .bind(crate::time::Timestamp::now())
    .bind(id)
    .execute(db)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// List all active sessions excluding `bootstrap-pending` (those are
/// implementation detail of the pairing flow, not "devices" the user
/// would think to revoke).
pub async fn list_active(db: &SqlitePool) -> Result<Vec<Session>, sqlx::Error> {
    sqlx::query_as::<_, Session>(
        "SELECT * FROM session
         WHERE source != 'bootstrap-pending'
           AND datetime(expires_at) > datetime(?)
         ORDER BY last_seen_at DESC",
    )
    .bind(crate::time::Timestamp::now())
    .fetch_all(db)
    .await
}

/// Best-effort sweep that drops expired rows. Run from the scheduler.
pub async fn purge_expired(db: &SqlitePool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM session WHERE datetime(expires_at) < datetime(?)")
        .bind(crate::time::Timestamp::now())
        .execute(db)
        .await?;
    if res.rows_affected() > 0 {
        tracing::info!(removed = res.rows_affected(), "purged expired sessions");
    }
    Ok(res.rows_affected())
}

// ─── Signed URLs ────────────────────────────────────────────────

/// Fetch (or lazily-initialise) the secret used to HMAC signed
/// URLs. Stored on `config.session_signing_key`; auto-generated on
/// first request when missing so a fresh install works without an
/// explicit setup step.
pub async fn signing_secret(db: &SqlitePool) -> Result<String, sqlx::Error> {
    let existing: Option<Option<String>> =
        sqlx::query_scalar("SELECT session_signing_key FROM config WHERE id = 1")
            .fetch_optional(db)
            .await?;
    if let Some(Some(s)) = existing
        && !s.is_empty()
    {
        return Ok(s);
    }
    // Lazy init: generate + persist + return. Concurrent first calls
    // race to write but the second writer's value silently wins; both
    // observers see a valid key, just not necessarily the same one
    // (the loser's URLs would reject — vanishingly rare since this
    // only happens on the first prepare-and-sign call after install).
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    sqlx::query("UPDATE config SET session_signing_key = ? WHERE id = 1")
        .bind(&key)
        .execute(db)
        .await?;
    tracing::info!("session signing key initialised");
    Ok(key)
}

/// Sign a (method, path, expiry) tuple. Output is URL-safe so it
/// drops straight into the request's query string.
pub fn sign_url(method: &str, path: &str, exp: i64, secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(method.to_ascii_uppercase().as_bytes());
    mac.update(b"|");
    mac.update(path.as_bytes());
    mac.update(b"|");
    mac.update(exp.to_string().as_bytes());
    let result = mac.finalize().into_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(result)
}

/// Verify a signed URL. Constant-time-compares the recomputed sig
/// against the provided one, so a timing attack can't recover the
/// signing secret one byte at a time.
pub fn verify_signed_url(method: &str, path: &str, exp: i64, sig: &str, secret: &str) -> bool {
    if exp < chrono::Utc::now().timestamp() {
        return false;
    }
    let expected = sign_url(method, path, exp, secret);
    expected.as_bytes().ct_eq(sig.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_url_roundtrips() {
        let secret = "test-secret";
        let exp = chrono::Utc::now().timestamp() + 600;
        let sig = sign_url("GET", "/api/v1/play/movie/42/direct", exp, secret);
        assert!(verify_signed_url(
            "GET",
            "/api/v1/play/movie/42/direct",
            exp,
            &sig,
            secret
        ));
    }

    #[test]
    fn signed_url_rejects_expired() {
        let secret = "test-secret";
        let exp = chrono::Utc::now().timestamp() - 60;
        let sig = sign_url("GET", "/x", exp, secret);
        assert!(!verify_signed_url("GET", "/x", exp, &sig, secret));
    }

    #[test]
    fn signed_url_rejects_tampered_method() {
        let secret = "test-secret";
        let exp = chrono::Utc::now().timestamp() + 60;
        let sig = sign_url("GET", "/x", exp, secret);
        assert!(!verify_signed_url("DELETE", "/x", exp, &sig, secret));
    }

    #[test]
    fn signed_url_rejects_tampered_path() {
        let secret = "test-secret";
        let exp = chrono::Utc::now().timestamp() + 60;
        let sig = sign_url("GET", "/x", exp, secret);
        assert!(!verify_signed_url("GET", "/y", exp, &sig, secret));
    }

    #[test]
    fn signed_url_rejects_wrong_secret() {
        let exp = chrono::Utc::now().timestamp() + 60;
        let sig = sign_url("GET", "/x", exp, "secret-a");
        assert!(!verify_signed_url("GET", "/x", exp, &sig, "secret-b"));
    }

    #[test]
    fn ids_eq_constant_time() {
        assert!(ids_eq("aaaa", "aaaa"));
        assert!(!ids_eq("aaaa", "aaab"));
        assert!(!ids_eq("aaaa", "aaa"));
        // Empty == empty per `ConstantTimeEq` semantics. Caller is
        // responsible for rejecting empty session ids before they
        // reach `ids_eq` — the lookup itself returns None for an
        // empty key, so this never matters in practice.
        assert!(ids_eq("", ""));
    }

    #[test]
    fn generate_session_id_is_unique_and_long() {
        let a = generate_session_id();
        let b = generate_session_id();
        assert_ne!(a, b);
        assert!(a.len() >= 40); // base64 of 32 bytes
    }
}
