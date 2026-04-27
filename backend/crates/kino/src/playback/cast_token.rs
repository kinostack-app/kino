//! Short-lived HMAC-signed tokens for casting to a Chromecast.
//!
//! The Cast receiver on the TV fetches the stream URL itself — it
//! can't add our `Authorization` header. We issue per-resource,
//! time-bound JWTs the frontend appends as `?cast_token=…` on the
//! stream URL before handing it to the Cast SDK. The auth
//! middleware (`auth::require_api_key`) accepts a valid token in
//! lieu of the API key for `/api/v1/play/{kind}/{entity_id}/*`
//! routes — so the receiver never sees the raw API key.
//!
//! The token's `sub` claim is the play-route subject (`movie/42`,
//! `episode/17`) so a token for one URL cannot be swapped onto
//! another. The signing secret is derived from the user's current
//! API key (`SHA-256("kino-cast-v1" || api_key)`) so rotating the
//! key naturally invalidates outstanding tokens — no separate
//! secret to provision.

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TOKEN_TTL_SECS: i64 = 8 * 60 * 60; // 8h — long movies + pauses
const ISSUER: &str = "kino";

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Play-route subject — `"{kind}/{entity_id}"` matching the
    /// URL path (`movie/42`, `episode/17`). A token for one
    /// subject cannot unlock another.
    pub sub: String,
    /// Expiry as a Unix timestamp (seconds).
    pub exp: i64,
    /// `iss` — issuer tag so we can reject tokens meant for some
    /// other kino instance / purpose.
    pub iss: String,
}

/// Derive the HMAC secret from the current API key. Cheap (~1µs)
/// and deterministic — callers can do this per-request rather than
/// stashing the key material anywhere.
pub fn derive_secret(api_key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"kino-cast-v1|");
    hasher.update(api_key.as_bytes());
    let out = hasher.finalize();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&out);
    secret
}

/// Canonical string form of the play-route subject. Kept as a
/// single helper so issue + verify + URL construction can't drift.
#[must_use]
pub fn subject(kind: &str, entity_id: i64) -> String {
    format!("{kind}/{entity_id}")
}

/// Issue a token authorising casts to `/api/v1/play/{kind}/{entity_id}/*`.
/// Returns the token and its Unix-timestamp expiry.
pub fn issue(
    kind: &str,
    entity_id: i64,
    secret: &[u8; 32],
) -> Result<(String, i64), CastTokenError> {
    let exp = chrono::Utc::now().timestamp() + TOKEN_TTL_SECS;
    let claims = Claims {
        sub: subject(kind, entity_id),
        exp,
        iss: ISSUER.into(),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|e| CastTokenError::Encode(e.to_string()))?;
    Ok((token, exp))
}

/// Verify a token authorises the given play-route subject. Returns
/// the decoded claims on success so callers can log the matched
/// subject + expiry.
pub fn verify(
    token: &str,
    kind: &str,
    entity_id: i64,
    secret: &[u8; 32],
) -> Result<Claims, CastTokenError> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISSUER]);
    validation.leeway = 30; // tolerate clock skew
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret), &validation)
        .map_err(|e| CastTokenError::Decode(e.to_string()))?;
    let expected = subject(kind, entity_id);
    if data.claims.sub != expected {
        return Err(CastTokenError::WrongSubject);
    }
    Ok(data.claims)
}

#[derive(Debug, thiserror::Error)]
pub enum CastTokenError {
    #[error("token encode failed: {0}")]
    Encode(String),
    #[error("token decode failed: {0}")]
    Decode(String),
    #[error("token is for a different play-route subject than the request")]
    WrongSubject,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_movie() {
        let secret = derive_secret("my-api-key");
        let (tok, _exp) = issue("movie", 42, &secret).unwrap();
        let claims = verify(&tok, "movie", 42, &secret).unwrap();
        assert_eq!(claims.sub, "movie/42");
    }

    #[test]
    fn round_trip_episode() {
        let secret = derive_secret("my-api-key");
        let (tok, _exp) = issue("episode", 17, &secret).unwrap();
        let claims = verify(&tok, "episode", 17, &secret).unwrap();
        assert_eq!(claims.sub, "episode/17");
    }

    #[test]
    fn wrong_entity_id_rejected() {
        let secret = derive_secret("my-api-key");
        let (tok, _) = issue("movie", 42, &secret).unwrap();
        let err = verify(&tok, "movie", 43, &secret).unwrap_err();
        assert!(matches!(err, CastTokenError::WrongSubject));
    }

    #[test]
    fn wrong_kind_rejected() {
        let secret = derive_secret("my-api-key");
        // Token issued for movie/42 must not unlock episode/42.
        let (tok, _) = issue("movie", 42, &secret).unwrap();
        let err = verify(&tok, "episode", 42, &secret).unwrap_err();
        assert!(matches!(err, CastTokenError::WrongSubject));
    }

    #[test]
    fn rotated_api_key_invalidates() {
        let (tok, _) = issue("movie", 42, &derive_secret("old-key")).unwrap();
        let err = verify(&tok, "movie", 42, &derive_secret("new-key")).unwrap_err();
        assert!(matches!(err, CastTokenError::Decode(_)));
    }
}
