use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Monotonic counter used to tag each WS connection in logs.
/// Makes interleaved traces from N concurrent clients easy to
/// untangle — `client_id=17` shows up across all its own events
/// + the close log.
static CLIENT_ID: AtomicU64 = AtomicU64::new(1);

/// How long we'll wait for the post-upgrade auth frame before
/// closing the socket. Long enough for a slow network to reach
/// the handler; short enough that a buggy / malicious client
/// can't hold a file descriptor open indefinitely.
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Inbound control-frame shapes. Kept narrow — anything else
/// closes the socket. `serde(tag)` gives us a discriminated union
/// matching the one in `docs/subsystems/09-api.md`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    /// First frame after upgrade. Carries either:
    ///
    /// - `api_key`: the master credential (CLI clients, bearer-
    ///   mode browsers); validated against `config.api_key` in
    ///   constant time.
    /// - `session_id`: a cookie session id (sent by browsers when
    ///   the upgrade headers don't carry the Cookie — e.g. some
    ///   reverse-proxy strip configurations).
    ///
    /// At least one must be present; both are accepted so the
    /// frontend doesn't have to know which mode the deploy is in.
    Auth {
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
    Ping,
}

/// Outbound control-frame shapes. Events (the bulk of traffic)
/// go out as serialised `AppEvent`; these are the small control
/// messages the client needs to reason about session state.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlFrame {
    AuthOk,
    Pong,
}

/// WebSocket endpoint for real-time event push.
#[utoipa::path(
    get, path = "/api/v1/ws",
    responses((status = 101, description = "WebSocket upgrade")),
    tag = "websocket"
)]
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // The Cookie header is on the upgrade request; carry it into
    // the post-upgrade handler so cookie-mode browsers can skip the
    // explicit auth frame.
    let cookie_session = read_cookie_session(&headers);
    ws.on_upgrade(move |socket| handle_socket(socket, state, cookie_session))
}

fn read_cookie_session(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get("cookie")?.to_str().ok()?;
    for pair in raw.split(';') {
        let trimmed = pair.trim();
        if let Some((k, v)) = trimmed.split_once('=')
            && k == "kino-session"
        {
            return Some(v.to_owned());
        }
    }
    None
}

#[allow(clippy::too_many_lines)] // single sequential flow; splitting hides the loop's wiring
async fn handle_socket(mut socket: WebSocket, state: AppState, cookie_session: Option<String>) {
    let client_id = CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let opened_at = Instant::now();

    // Cookie-mode browsers carry the session id on the upgrade as
    // a `Cookie` header. Pre-validate it here so they can skip the
    // explicit auth frame entirely; bearer / CLI clients still go
    // through the post-upgrade handshake below.
    if let Some(sid) = cookie_session {
        if let Some(sess) = crate::auth_session::lookup(&state.db, &sid).await
            && crate::auth_session::is_valid(&sess)
        {
            let ok = serde_json::to_string(&ControlFrame::AuthOk).expect("auth_ok serialises");
            if socket.send(Message::Text(ok.into())).await.is_err() {
                return;
            }
            let db = state.db.clone();
            tokio::spawn(async move {
                crate::auth_session::touch_last_seen(&db, &sid).await;
            });
            tracing::debug!(client_id, "ws upgrade — cookie pre-authed");
        } else {
            tracing::debug!(
                client_id,
                "ws upgrade — cookie present but invalid; falling through to auth frame"
            );
            if let Err(reason) = authenticate(&mut socket, &state, client_id).await {
                tracing::info!(client_id, %reason, "ws auth rejected");
                return;
            }
        }
    } else {
        tracing::debug!(client_id, "ws upgrade — awaiting auth frame");
        if let Err(reason) = authenticate(&mut socket, &state, client_id).await {
            tracing::info!(client_id, %reason, "ws auth rejected");
            return;
        }
    }

    let mut rx = state.event_tx.subscribe();
    let subscribers = state.event_tx.receiver_count();
    let mut sent: u64 = 0;
    let mut serialize_errors: u64 = 0;

    tracing::info!(client_id, subscribers, "ws authenticated");

    let close_reason: &'static str = loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(e) => {
                        let Ok(json) = serde_json::to_string(&e) else {
                            // Should never happen for our AppEvent
                            // shape, but if a future variant serialises
                            // fallibly we want a signal rather than a
                            // silent empty frame.
                            serialize_errors += 1;
                            tracing::warn!(
                                client_id,
                                event_type = %e.event_type(),
                                "ws failed to serialise event",
                            );
                            continue;
                        };
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break "peer closed while sending";
                        }
                        sent += 1;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(client_id, skipped = n, "ws client lagged");
                        // Send through the typed `AppEvent::Lagged`
                        // variant so the generated TS discriminated
                        // union narrows it alongside every other
                        // event. Previously this was a hand-rolled
                        // `{event: "lagged"}` JSON object which the
                        // frontend had to handle via untyped
                        // fallback, violating the generated-contract
                        // rule.
                        let event = crate::events::AppEvent::Lagged { skipped: n };
                        let text = serde_json::to_string(&event).unwrap_or_default();
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break "peer closed after lag notice";
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break "broadcast channel closed";
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break "peer closed after ping";
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Post-auth client frames: spec currently
                        // defines `{"type":"ping"}`. Anything else is
                        // ignored (not closed) so future protocol
                        // additions stay backwards-tolerant for older
                        // servers.
                        match serde_json::from_str::<ClientFrame>(&text) {
                            Ok(ClientFrame::Ping) => {
                                let pong = serde_json::to_string(&ControlFrame::Pong)
                                    .expect("pong serialises");
                                if socket.send(Message::Text(pong.into())).await.is_err() {
                                    break "peer closed after app-level ping";
                                }
                            }
                            Ok(ClientFrame::Auth { .. }) => {
                                // Already authenticated — duplicate
                                // auth frames are a client bug. Log
                                // but don't close; the stream is
                                // already ours.
                                tracing::debug!(
                                    client_id,
                                    "ws ignored duplicate auth frame from authenticated client",
                                );
                            }
                            Err(e) => {
                                tracing::debug!(
                                    client_id,
                                    error = %e,
                                    "ws ignored unparseable client frame",
                                );
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => break "peer sent close frame",
                    None => break "peer dropped",
                    Some(Err(e)) => {
                        tracing::warn!(client_id, error = %e, "ws recv error");
                        break "peer recv error";
                    }
                    _ => {}
                }
            }
        }
    };

    let duration = opened_at.elapsed();
    tracing::info!(
        client_id,
        sent,
        serialize_errors,
        duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        reason = close_reason,
        "ws disconnected",
    );
}

/// Run the post-upgrade auth handshake.
///
/// Success path:
/// ```text
/// client → {"type":"auth","api_key":"..."}
/// server → {"type":"auth_ok"}
/// ```
///
/// Failure paths close the socket with an RFC 6455 close code:
/// * `1008` (policy violation) — bad / missing API key
/// * `1002` (protocol error)   — unparseable / wrong-shape first frame
/// * `1001` (going away)       — client closed before sending anything
///
/// Every outcome logs one INFO-or-WARN line tagged with the
/// client id so a reviewer scanning the log for auth issues can
/// spot them without grepping across many variants.
async fn authenticate(
    socket: &mut WebSocket,
    state: &AppState,
    client_id: u64,
) -> Result<(), &'static str> {
    let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;

    let text = match first {
        Err(_) => {
            reject(socket, close_code::POLICY, "auth timeout").await;
            tracing::warn!(
                client_id,
                "ws auth timeout — no frame within {}s",
                AUTH_TIMEOUT.as_secs()
            );
            return Err("auth timeout");
        }
        Ok(None) => {
            // Peer closed before sending anything. No point trying
            // to send a close frame; just return.
            return Err("peer closed pre-auth");
        }
        Ok(Some(Err(e))) => {
            tracing::warn!(client_id, error = %e, "ws pre-auth recv error");
            return Err("pre-auth recv error");
        }
        Ok(Some(Ok(Message::Text(t)))) => t,
        Ok(Some(Ok(Message::Close(_)))) => return Err("peer sent close pre-auth"),
        Ok(Some(Ok(_))) => {
            reject(socket, close_code::PROTOCOL, "non-text auth frame").await;
            return Err("non-text auth frame");
        }
    };

    let Ok(ClientFrame::Auth {
        api_key,
        session_id,
    }) = serde_json::from_str::<ClientFrame>(&text)
    else {
        reject(socket, close_code::PROTOCOL, "invalid auth frame shape").await;
        return Err("invalid auth frame shape");
    };

    // Cookie-mode browsers send `session_id`; bearer / CLI clients
    // send `api_key`. Either is sufficient; both being present is
    // accepted (the frontend does this when it doesn't know the
    // deploy mode).
    if let Some(sid) = session_id
        && let Some(sess) = crate::auth_session::lookup(&state.db, &sid).await
        && crate::auth_session::is_valid(&sess)
    {
        let ok = serde_json::to_string(&ControlFrame::AuthOk).expect("auth_ok serialises");
        if socket.send(Message::Text(ok.into())).await.is_err() {
            return Err("peer dropped after auth accept");
        }
        // Touch last_seen so the Devices page reflects activity.
        let db = state.db.clone();
        tokio::spawn(async move {
            crate::auth_session::touch_last_seen(&db, &sid).await;
        });
        return Ok(());
    }

    let configured: Option<String> =
        match sqlx::query_scalar::<_, String>("SELECT api_key FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(client_id, error = %e, "ws auth: api-key lookup failed, denying");
                reject(socket, close_code::ERROR, "server error").await;
                return Err("api-key lookup failed");
            }
        };

    match (configured, api_key) {
        (Some(expected), Some(key))
            if bool::from(<[u8] as subtle::ConstantTimeEq>::ct_eq(
                expected.as_bytes(),
                key.as_bytes(),
            )) =>
        {
            let ok = serde_json::to_string(&ControlFrame::AuthOk).expect("auth_ok serialises");
            if socket.send(Message::Text(ok.into())).await.is_err() {
                return Err("peer dropped after auth accept");
            }
            Ok(())
        }
        (Some(_), _) => {
            reject(socket, close_code::POLICY, "invalid credential").await;
            Err("bad credential")
        }
        (None, _) => {
            // Fresh-install path — no key configured yet.
            let ok = serde_json::to_string(&ControlFrame::AuthOk).expect("auth_ok serialises");
            let _ = socket.send(Message::Text(ok.into())).await;
            Ok(())
        }
    }
}

/// Send a close frame then drop the socket. Best-effort — if the
/// peer's already gone the send error is swallowed; the caller
/// has already decided to disconnect.
async fn reject(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}
