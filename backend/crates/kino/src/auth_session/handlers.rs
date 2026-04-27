//! Session lifecycle endpoints. The cookie-based browser session
//! flow lives here, plus the QR-code device-pairing helpers and the
//! signed-URL issuer used by the cross-origin `<video>` / `<img>`
//! path.
//!
//! Endpoint summary:
//!   - `GET  /api/v1/bootstrap`               — auth-mode discovery; public
//!   - `POST /api/v1/sessions`                — exchange master key for cookie; public
//!   - `POST /api/v1/sessions/redeem`         — redeem QR token for cookie; public
//!   - `GET  /api/v1/sessions`                — list active devices; authed
//!   - `DELETE /api/v1/sessions/{id}`         — revoke one; authed
//!   - `DELETE /api/v1/sessions?except=current` — revoke all but caller; authed
//!   - `POST /api/v1/sessions/cli`            — issue named long-lived token; authed
//!   - `POST /api/v1/sessions/bootstrap-token` — issue QR token; authed
//!   - `POST /api/v1/sessions/sign-url`       — issue signed media URL; authed
//!   - `POST /api/v1/logout`                  — clear current session; authed
//!
//! "Public" endpoints carry their own auth (paste the API key /
//! redeem a one-time token); the middleware skips them so the
//! caller has somewhere to land before they have a cookie.

use axum::Json;
use axum::extract::{ConnectInfo, Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use crate::auth::AuthContext;
use crate::auth_session;
use crate::auth_session::model::{SessionSource, SessionView};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

// ─── Bootstrap ──────────────────────────────────────────────────

/// Reply to `GET /bootstrap`. Tells the SPA whether it already has a
/// valid cookie session, whether the backend has a master key
/// configured (= setup complete), and which auth mode the SPA should
/// render. The endpoint never returns credentials.
#[derive(Debug, Serialize, ToSchema)]
pub struct BootstrapReply {
    /// True when the request landed with a valid `kino-session`
    /// cookie. Frontend uses this to skip the paste-the-key screen.
    pub session_active: bool,
    /// True once the backend has a master `api_key` configured.
    /// Setup wizard renders when false.
    pub setup_complete: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/bootstrap",
    responses((status = 200, body = BootstrapReply)),
    tag = "auth"
)]
pub async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> AppResult<Response> {
    let setup_complete = sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?
        .is_some_and(|k| !k.is_empty());

    // Treat a session-cookie-bearing request as "active" when the row
    // is present and not expired/consumed. We deliberately re-resolve
    // here (instead of relying on the middleware) so the public
    // endpoint can answer correctly even before the user has any
    // session — the middleware skips it.
    let cookie_val = read_cookie(&headers, "kino-session");
    let cookie_session = if let Some(ref c) = cookie_val {
        auth_session::lookup(&state.db, c)
            .await
            .filter(auth_session::is_valid)
    } else {
        None
    };

    // Localhost auto-cookie: when bootstrap fires from a same-machine
    // browser (loopback IP, no proxy hops), the user has already
    // demonstrated control of the host kino is running on — there's
    // no meaningful auth difference between "they could read
    // config.api_key off disk" and "they get a session." Issue one
    // automatically so the dev container + single-user-on-laptop
    // flow has zero friction.
    //
    // We require BOTH a loopback `remote_addr` AND the absence of any
    // non-loopback hop in `X-Forwarded-For`, so a reverse proxy that
    // happens to bind to localhost on the kino side doesn't gift
    // every request from the public internet a session.
    if setup_complete && cookie_session.is_none() && is_localhost_request(&addr, &headers) {
        match auth_session::create(
            &state.db,
            "Local browser".into(),
            headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            Some(addr.ip().to_string()),
            SessionSource::AutoLocalhost,
            chrono::Duration::days(auth_session::LOCALHOST_SESSION_TTL_DAYS),
        )
        .await
        {
            Ok(sess) => {
                tracing::info!(
                    session_id = %sess.id,
                    "bootstrap: auto-issued localhost session"
                );
                let cookie = build_session_cookie(&headers, &sess.id, &sess.expires_at);
                return Ok((
                    [(header::SET_COOKIE, cookie)],
                    Json(BootstrapReply {
                        session_active: true,
                        setup_complete,
                    }),
                )
                    .into_response());
            }
            Err(e) => {
                tracing::warn!(error = %e, "bootstrap: localhost auto-session create failed");
            }
        }
    }

    Ok(Json(BootstrapReply {
        session_active: cookie_session.is_some(),
        setup_complete,
    })
    .into_response())
}

/// True when a request can safely be granted a localhost auto-
/// session — both halves matter:
///
/// 1. Peer socket address is loopback (127.0.0.0/8 or `::1`).
/// 2. No non-loopback hop in `X-Forwarded-For`. A reverse proxy
///    binding to localhost on the kino side would otherwise let
///    every public-internet request claim auto-cookie status. We
///    accept loopback hops in the chain (vite dev-server proxy
///    sets `X-Forwarded-For: 127.0.0.1`) — that's still same-
///    machine, which is what the policy authorises.
fn is_localhost_request(addr: &SocketAddr, headers: &HeaderMap) -> bool {
    if !addr.ip().is_loopback() {
        return false;
    }
    if let Some(xff) = headers.get("x-forwarded-for")
        && let Ok(value) = xff.to_str()
    {
        for hop in value.split(',') {
            let hop = hop.trim();
            // Strip any port-suffix (`1.2.3.4:5678`) before parsing.
            let host = hop.rsplit_once(':').map_or(hop, |(h, _)| h);
            let Ok(ip) = host.parse::<std::net::IpAddr>() else {
                // Unparseable hop — treat as non-loopback to be safe.
                return false;
            };
            if !ip.is_loopback() {
                return false;
            }
        }
    }
    true
}

// ─── Create session from API key ────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSessionRequest {
    /// The master API key. Validated constant-time against
    /// `config.api_key`.
    pub api_key: String,
    /// Optional human-readable label; defaults to the request's
    /// User-Agent if omitted.
    pub label: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateSessionReply {
    pub session: SessionView,
}

/// Exchange the master API key for a cookie session. This endpoint
/// is intentionally public — the body carries the credential — and
/// rate-limited at the layer above to slow brute-force.
#[utoipa::path(
    post,
    path = "/api/v1/sessions",
    request_body = CreateSessionRequest,
    responses(
        (status = 200, body = CreateSessionReply, description = "Session created; cookie set"),
        (status = 401, description = "Invalid API key"),
    ),
    tag = "auth"
)]
pub async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<CreateSessionRequest>,
) -> AppResult<Response> {
    let stored_key: Option<String> = sqlx::query_scalar("SELECT api_key FROM config WHERE id = 1")
        .fetch_optional(&state.db)
        .await?;
    let Some(expected) = stored_key.filter(|k| !k.is_empty()) else {
        return Err(AppError::Unauthorized(
            "no API key configured yet — finish setup first".into(),
        ));
    };
    if !bool::from(expected.as_bytes().ct_eq(body.api_key.as_bytes())) {
        tracing::warn!(
            ip = %addr.ip(),
            "session create rejected: api key mismatch"
        );
        return Err(AppError::Unauthorized("invalid API key".into()));
    }

    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let label = body
        .label
        .clone()
        .or_else(|| user_agent.clone().map(label_from_ua))
        .unwrap_or_else(|| "Browser".into());

    let sess = auth_session::create(
        &state.db,
        label,
        user_agent,
        Some(addr.ip().to_string()),
        SessionSource::Browser,
        chrono::Duration::days(auth_session::BROWSER_SESSION_TTL_DAYS),
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let cookie = build_session_cookie(&headers, &sess.id, &sess.expires_at);
    let view = sess.clone().into_view(Some(&sess.id));
    let body = CreateSessionReply { session: view };

    Ok(([(header::SET_COOKIE, cookie)], Json(body)).into_response())
}

// ─── Redeem QR-bootstrap token ──────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct RedeemRequest {
    pub token: String,
    pub label: Option<String>,
}

/// Redeem a one-time pairing token (issued via
/// `POST /sessions/bootstrap-token` and rendered as a QR code on the
/// originating device). On success: marks the bootstrap row consumed,
/// creates a fresh `qr-bootstrap` session, sets the cookie.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/redeem",
    request_body = RedeemRequest,
    responses(
        (status = 200, body = CreateSessionReply),
        (status = 401, description = "Token unknown, expired, or already consumed"),
    ),
    tag = "auth"
)]
pub async fn redeem(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<RedeemRequest>,
) -> AppResult<Response> {
    // Look up the pending token. Validate it's the right source +
    // not expired before attempting the consume — the consume itself
    // is idempotent on `consumed_at IS NULL` so a race is safe, but
    // the early return gives a clean 401 path.
    let Some(pending) = auth_session::lookup(&state.db, &body.token).await else {
        tracing::warn!(ip = %addr.ip(), "redeem rejected: unknown token");
        return Err(AppError::Unauthorized("unknown or expired token".into()));
    };
    if pending.source != "bootstrap-pending" || !auth_session::is_valid(&pending) {
        tracing::warn!(
            ip = %addr.ip(),
            source = %pending.source,
            "redeem rejected: not a valid pending token"
        );
        return Err(AppError::Unauthorized("unknown or expired token".into()));
    }
    let consumed = auth_session::consume_bootstrap(&state.db, &body.token)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if !consumed {
        tracing::warn!(ip = %addr.ip(), "redeem rejected: token already consumed");
        return Err(AppError::Unauthorized(
            "token already used — request a new one".into(),
        ));
    }

    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let label = body
        .label
        .clone()
        .or_else(|| user_agent.clone().map(label_from_ua))
        .unwrap_or_else(|| "Paired device".into());

    let sess = auth_session::create(
        &state.db,
        label,
        user_agent,
        Some(addr.ip().to_string()),
        SessionSource::QrBootstrap,
        chrono::Duration::days(auth_session::BROWSER_SESSION_TTL_DAYS),
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let cookie = build_session_cookie(&headers, &sess.id, &sess.expires_at);
    let body = CreateSessionReply {
        session: sess.clone().into_view(Some(&sess.id)),
    };
    Ok(([(header::SET_COOKIE, cookie)], Json(body)).into_response())
}

// ─── List + revoke ──────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct ListSessionsReply {
    pub sessions: Vec<SessionView>,
}

#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    responses((status = 200, body = ListSessionsReply)),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> AppResult<Json<ListSessionsReply>> {
    let rows = auth_session::list_active(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let current_id = ctx.session_id.as_deref();
    let sessions = rows.into_iter().map(|s| s.into_view(current_id)).collect();
    Ok(Json(ListSessionsReply { sessions }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct RevokeAllParams {
    /// When `current`, every session except the caller's own is
    /// revoked. The caller stays signed in. Omitting this parameter
    /// is rejected as ambiguous to prevent foot-gun bulk-revokes.
    pub except: Option<String>,
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions",
    params(RevokeAllParams),
    responses(
        (status = 204, description = "Other sessions revoked"),
        (status = 400, description = "Missing required `?except=current` guard"),
    ),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn revoke_all(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<RevokeAllParams>,
) -> AppResult<StatusCode> {
    if params.except.as_deref() != Some("current") {
        return Err(AppError::BadRequest(
            "DELETE /sessions requires `?except=current` so it can't accidentally sign out every device".into(),
        ));
    }
    let Some(ref keep) = ctx.session_id else {
        return Err(AppError::BadRequest(
            "no current session to keep — sign in first".into(),
        ));
    };
    auth_session::revoke_all_except(&state.db, keep)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 204, description = "Revoked"),
        (status = 404, description = "Unknown session id"),
    ),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn revoke_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let n = auth_session::revoke(&state.db, &id)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    if n == 0 {
        return Err(AppError::NotFound(format!("session {id} not found")));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ─── Logout ─────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/v1/logout",
    responses((status = 204, description = "Cookie cleared")),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn logout(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
) -> AppResult<Response> {
    if let Some(ref id) = ctx.session_id {
        auth_session::revoke(&state.db, id)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    // Always emit a clearing Set-Cookie so a stale cookie that
    // didn't match a row still gets evicted from the browser.
    let cleared = clear_session_cookie(&headers);
    Ok(([(header::SET_COOKIE, cleared)], StatusCode::NO_CONTENT).into_response())
}

// ─── CLI long-lived tokens ──────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateCliTokenRequest {
    pub label: String,
    /// Lifetime in days. Capped at 365.
    pub ttl_days: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateCliTokenReply {
    /// The bearer token. Returned ONCE — the response is the only
    /// time the user will see it. Callers should treat this like a
    /// password.
    pub token: String,
    pub session: SessionView,
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions/cli",
    request_body = CreateCliTokenRequest,
    responses((status = 200, body = CreateCliTokenReply)),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn create_cli_token(
    State(state): State<AppState>,
    Json(body): Json<CreateCliTokenRequest>,
) -> AppResult<Json<CreateCliTokenReply>> {
    if body.label.trim().is_empty() {
        return Err(AppError::BadRequest("label is required".into()));
    }
    let ttl_days = body.ttl_days.unwrap_or(365).clamp(1, 365);
    let sess = auth_session::create(
        &state.db,
        body.label.trim().to_owned(),
        None,
        None,
        SessionSource::Cli,
        chrono::Duration::days(ttl_days),
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let token = sess.id.clone();
    let view = sess.into_view(None);
    Ok(Json(CreateCliTokenReply {
        token,
        session: view,
    }))
}

// ─── QR bootstrap token (issue) ─────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct BootstrapTokenReply {
    /// The one-time token, base64-url-safe. Encode into a QR code
    /// or a paste-link; the receiving device POSTs it to
    /// `/sessions/redeem`.
    pub token: String,
    pub expires_at: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions/bootstrap-token",
    responses((status = 200, body = BootstrapTokenReply)),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn create_bootstrap_token(
    State(state): State<AppState>,
) -> AppResult<Json<BootstrapTokenReply>> {
    let sess = auth_session::create(
        &state.db,
        "Pairing token".into(),
        None,
        None,
        SessionSource::BootstrapPending,
        chrono::Duration::minutes(auth_session::QR_BOOTSTRAP_TTL_MINS),
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(BootstrapTokenReply {
        token: sess.id,
        expires_at: sess.expires_at,
    }))
}

// ─── Signed media URLs ──────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SignUrlRequest {
    /// The path to sign — e.g. `/api/v1/play/movie/42/direct`.
    /// Method is currently always `GET`; if we ever sign a
    /// state-changing operation this struct will gain a `method`
    /// field.
    pub path: String,
    /// TTL in seconds. Capped at 60 minutes.
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SignUrlReply {
    /// Path with `?sig=...&exp=...` already appended. Drop straight
    /// into a `<video src=...>` or `<img src=...>` attribute.
    pub url: String,
    pub expires_at: i64,
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions/sign-url",
    request_body = SignUrlRequest,
    responses((status = 200, body = SignUrlReply)),
    tag = "auth",
    security(("api_key" = []))
)]
pub async fn sign_url(
    State(state): State<AppState>,
    Json(body): Json<SignUrlRequest>,
) -> AppResult<Json<SignUrlReply>> {
    if !body.path.starts_with('/') {
        return Err(AppError::BadRequest("path must start with `/`".into()));
    }
    // Restrict signable paths to the routes that actually make sense
    // to embed in `<video>` / `<img>` — anything else is a misuse and
    // would also expose state-changing endpoints to drive-by replay
    // if a sig leaked.
    if !signable_path(&body.path) {
        return Err(AppError::BadRequest(format!(
            "path `{}` is not eligible for signing",
            body.path
        )));
    }
    let ttl = body
        .ttl_secs
        .unwrap_or(auth_session::SIGNED_URL_TTL_SECS)
        .clamp(1, 60 * 60);
    let exp = chrono::Utc::now().timestamp() + ttl;
    let secret = auth_session::signing_secret(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let sig = auth_session::sign_url("GET", &body.path, exp, &secret);
    let separator = if body.path.contains('?') { '&' } else { '?' };
    let url = format!("{}{}sig={}&exp={}", body.path, separator, sig, exp);
    Ok(Json(SignUrlReply {
        url,
        expires_at: exp,
    }))
}

fn signable_path(path: &str) -> bool {
    path.starts_with("/api/v1/play/")
        || path.starts_with("/api/v1/images/")
        || path.starts_with("/api/v1/stream/")
}

// ─── Helpers ────────────────────────────────────────────────────

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get("cookie")?.to_str().ok()?;
    for raw in header.split(';') {
        let pair = raw.trim();
        if let Some((k, v)) = pair.split_once('=')
            && k == name
        {
            return Some(v.to_owned());
        }
    }
    None
}

/// Construct the Set-Cookie header. `Secure` is set only when the
/// request arrived over HTTPS — pure-`Secure` cookies on plain HTTP
/// are silently dropped by browsers and would force re-auth on every
/// page load. Behind a TLS-terminating reverse proxy, the
/// `X-Forwarded-Proto` header is honoured.
///
/// `SameSite=Strict` because kino is single-origin: the SPA, the API,
/// and any cast/share URLs all live under one host. Strict closes the
/// CSRF foot-gun where a cookie + a `?api_key=` query path could be
/// triggered cross-site. `Lax` was the previous default; `Strict`
/// loses nothing for a single-origin app.
fn build_session_cookie(headers: &HeaderMap, id: &str, expires_at: &str) -> HeaderValue {
    let secure = is_request_https(headers);
    let secure_attr = if secure { "; Secure" } else { "" };
    let value = format!(
        "kino-session={id}; Path=/; HttpOnly; SameSite=Strict; Expires={expires_at}{secure_attr}"
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn clear_session_cookie(headers: &HeaderMap) -> HeaderValue {
    let secure = is_request_https(headers);
    let secure_attr = if secure { "; Secure" } else { "" };
    let value = format!("kino-session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure_attr}");
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn is_request_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.eq_ignore_ascii_case("https"))
}

/// Turn a User-Agent string into the short "Firefox on `PopOS`" labels
/// the Devices page renders. Best-effort; falls through to the raw UA
/// for anything we don't recognise so the session is still labelled.
fn label_from_ua(ua: String) -> String {
    let lower = ua.to_ascii_lowercase();
    let browser = if lower.contains("firefox") {
        "Firefox"
    } else if lower.contains("edg/") {
        "Edge"
    } else if lower.contains("chrome") {
        "Chrome"
    } else if lower.contains("safari") {
        "Safari"
    } else {
        return ua;
    };
    let os = if lower.contains("windows") {
        "Windows"
    } else if lower.contains("mac os") || lower.contains("macintosh") {
        "macOS"
    } else if lower.contains("android") {
        "Android"
    } else if lower.contains("iphone") || lower.contains("ios") {
        "iOS"
    } else if lower.contains("linux") {
        "Linux"
    } else {
        "Unknown"
    };
    format!("{browser} on {os}")
}
