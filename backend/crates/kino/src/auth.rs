use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;

use crate::auth_session;
use crate::playback::cast_token;
use crate::state::AppState;

/// Stamped onto `request.extensions` by the auth middleware whenever
/// the request landed via a cookie session. Handlers that care about
/// "which session am I" — `GET /sessions` (current badge),
/// `DELETE /sessions/{id}` (refusing self-revoke without confirmation),
/// `POST /logout` (need the id to delete) — pull this out of
/// extensions instead of re-parsing the cookie.
#[derive(Debug, Clone)]
#[allow(dead_code)] // session_id is read by sessions endpoints + ws auth in this same change set
pub struct AuthContext {
    pub session_id: Option<String>,
}

/// Authentication middleware.
///
/// A request is authorised if any of these match:
///   1. `?cast_token=<jwt>` on a `/play/{kind}/{id}/...` route — the
///      receiver-scoped HMAC token issued by `playback::cast_token`.
///      Cast receivers can't send custom headers, so this stays.
///   2. `?sig=<hmac>&exp=<epoch>` — short-lived signed URL covering
///      `(method, path, exp)`. Used by `<video>`/`<img>` elements in
///      cross-origin deploys where cookies aren't an option.
///   3. `Cookie: kino-session=<id>` — derived browser session,
///      validated against the `session` table. Touches
///      `last_seen_at` on every authed request (fire-and-forget).
///   4. `Authorization: Bearer <api_key>` / `X-Api-Key: <api_key>` /
///      `?api_key=<api_key>` — the master credential. CLI scripts +
///      first-time browser bootstrap.
///
/// Equality checks on credentials use constant-time comparison so a
/// timing side-channel can't leak the master key one byte at a time.
pub async fn require_api_key(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path();
    let method = request.method().clone();

    // Public endpoints — no auth required, by design.
    if is_public_path(path) {
        return Ok(next.run(request).await);
    }

    // Swagger UI / OpenAPI doc serve — gated in release builds at
    // the router; the middleware skip exists so the dev container
    // can browse it without first opening a session.
    if path.starts_with("/api/docs") || path.starts_with("/api-docs") {
        return Ok(next.run(request).await);
    }

    // WebSocket: the upgrade handshake itself is unauthenticated.
    // The post-upgrade `{"type":"auth", ...}` frame is the gate; see
    // `notification::ws_handlers::handle_socket`. URL-embedded credentials would land
    // in server access logs and browser history — explicitly out.
    if path == "/api/v1/ws" {
        return Ok(next.run(request).await);
    }

    // Cast token on /play/{kind}/{entity_id}/... routes.
    if let Some((kind, entity_id)) = path_cast_target(path)
        && let Some(token) = extract_query_param(&request, "cast_token")
        && let Ok(Some(key)) = get_api_key_from_db(&state.db).await
    {
        let secret = cast_token::derive_secret(&key);
        if cast_token::verify(&token, kind, entity_id, &secret).is_ok() {
            request
                .extensions_mut()
                .insert(AuthContext { session_id: None });
            return Ok(next.run(request).await);
        }
    }

    // Signed-URL bypass — used by `<video>` / `<img>` in cross-origin
    // deploys where cookies aren't sent. The signature commits to the
    // method + path + expiry, so a sig issued for a `GET /direct/123`
    // can't be reused for `DELETE /movies/123`.
    if let (Some(sig), Some(exp_str)) = (
        extract_query_param(&request, "sig"),
        extract_query_param(&request, "exp"),
    ) && let Ok(exp) = exp_str.parse::<i64>()
        && let Ok(secret) = auth_session::signing_secret(&state.db).await
        && auth_session::verify_signed_url(method.as_str(), path, exp, &sig, &secret)
    {
        request
            .extensions_mut()
            .insert(AuthContext { session_id: None });
        return Ok(next.run(request).await);
    }

    // Cookie session — the everyday browser path.
    if let Some(cookie_val) = extract_cookie(&request, "kino-session")
        && let Some(sess) = auth_session::lookup(&state.db, &cookie_val).await
        && auth_session::is_valid(&sess)
    {
        // Fire-and-forget refresh — UPDATE on a primary key is cheap,
        // and a transient failure here just means the Devices page's
        // "last seen" is stale, never that the request fails.
        let db = state.db.clone();
        let id = sess.id.clone();
        tokio::spawn(async move {
            auth_session::touch_last_seen(&db, &id).await;
        });
        request.extensions_mut().insert(AuthContext {
            session_id: Some(sess.id.clone()),
        });
        return Ok(next.run(request).await);
    }

    // Master API key (header / query). Constant-time-compared.
    let api_key = get_api_key_from_db(&state.db).await;
    let provided_key = extract_api_key(&request);

    match (api_key, provided_key) {
        (Ok(Some(expected)), Some(provided))
            if expected.as_bytes().ct_eq(provided.as_bytes()).into() =>
        {
            request
                .extensions_mut()
                .insert(AuthContext { session_id: None });
            Ok(next.run(request).await)
        }
        // Fresh-install setup wizard: no key configured yet, let
        // unauthenticated requests through so the wizard can seed
        // the config.
        (Ok(None), _) => Ok(next.run(request).await),
        (Err(e), _) => {
            tracing::warn!(error = %e, "auth: api-key lookup failed, denying request");
            Err(StatusCode::UNAUTHORIZED)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Endpoints that intentionally bypass auth. Each one is documented
/// with the reason it's public; adding to this list requires that
/// reasoning to be obvious from the path itself.
fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        // Health / readiness check for monitoring tools.
        "/api/v1/status"
        // Auth-mode discovery: the SPA has to know whether it
        // already has a session before it can decide to render the
        // paste-the-key screen vs the app. Returns metadata only,
        // never credentials.
        | "/api/v1/bootstrap"
        // Session creation by exchanging the master API key for a
        // cookie. The body must contain the key — that IS the auth
        // for this endpoint, so middleware-level auth would be
        // circular.
        | "/api/v1/sessions"
        // QR-code redemption: the request body carries a one-time
        // token that's been validated against the `session` table
        // separately. Middleware-level auth would block fresh
        // devices that have nothing else to present.
        | "/api/v1/sessions/redeem"
    )
}

fn path_cast_target(path: &str) -> Option<(&str, i64)> {
    let rest = path.strip_prefix("/api/v1/play/")?;
    let (kind, rest) = rest.split_once('/')?;
    let (entity, tail) = rest.split_once('/')?;
    if tail.is_empty() {
        return None;
    }
    if !matches!(kind, "movie" | "episode") {
        return None;
    }
    let entity_id = entity.parse::<i64>().ok()?;
    Some((kind, entity_id))
}

fn extract_query_param(request: &Request, key: &str) -> Option<String> {
    let query = request.uri().query()?;
    let prefix = format!("{key}=");
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix(&prefix) {
            return Some(urlencoding::decode(v).ok()?.into_owned());
        }
    }
    None
}

fn extract_api_key(request: &Request) -> Option<String> {
    if let Some(auth) = request.headers().get("authorization")
        && let Ok(value) = auth.to_str()
        && let Some(key) = value.strip_prefix("Bearer ")
    {
        return Some(key.to_owned());
    }
    if let Some(key) = request.headers().get("x-api-key")
        && let Ok(value) = key.to_str()
    {
        return Some(value.to_owned());
    }
    if let Some(query) = request.uri().query() {
        for pair in query.split('&') {
            if let Some(key) = pair.strip_prefix("api_key=") {
                return urlencoding::decode(key)
                    .ok()
                    .map(std::borrow::Cow::into_owned);
            }
        }
    }
    None
}

/// Pull a single cookie value out of the `Cookie` header. Cookies are
/// `name=value; name=value; ...` — we walk the header once and return
/// the first match. Non-existent header / cookie returns `None`.
fn extract_cookie(request: &Request, name: &str) -> Option<String> {
    let header = request.headers().get("cookie")?.to_str().ok()?;
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

/// Discriminate between three states so the caller can be specific.
async fn get_api_key_from_db(db: &sqlx::SqlitePool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
}

// Suppress the unused-method warning when other compilation modes
// don't reach the helper in tests.
#[allow(dead_code)]
fn _no_op() {}
