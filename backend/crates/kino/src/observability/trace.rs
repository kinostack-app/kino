//! Per-request trace id middleware.
//!
//! Generates a short random id for each incoming HTTP request and opens
//! a root `tracing::Span` carrying it. All handler-level `info!`/`warn!`
//! events inherit the span; the log-writing layer reads the trace id
//! from span extensions and writes it onto the `log_entry` row. Result:
//! every HTTP request gets a correlated trail in the log UI, and users
//! can deep-link from one row to "all logs for this request."

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

/// Header the response carries so clients (and tests) can correlate
/// themselves to server-side logs.
pub const TRACE_ID_HEADER: &str = "x-trace-id";

/// Generate a short 12-hex-char id. Sub-ns-collision-proof is not a
/// requirement — at home-server rates 2^48 is plenty. We use the top
/// bytes of a v4 UUID so we're piggy-backing on the already-linked
/// system RNG without pulling in `rand` as a direct dep.
fn new_trace_id() -> String {
    let id = uuid::Uuid::new_v4();
    let bytes = id.as_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
}

/// Axum middleware. Opens a `request` span with the generated `trace_id`
/// and runs the rest of the pipeline inside it.
pub async fn trace_layer(mut request: Request, next: Next) -> Response {
    // Reuse any upstream-provided id (reverse proxy, test harness) so
    // requests from the frontend can be correlated across processes.
    let tid = request
        .headers()
        .get(TRACE_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .map_or_else(new_trace_id, str::to_owned);

    // Make the id available to handlers via request extensions too, in
    // case a handler wants to include it in a response body.
    request.extensions_mut().insert(TraceId(tid.clone()));

    // `trace_id` field is picked up by SqliteLogLayer::on_new_span and
    // stored in span extensions, so every log event inside this scope
    // carries it on its row.
    let span = tracing::info_span!(
        "request",
        trace_id = %tid,
        method = %request.method(),
        path = %request.uri().path(),
    );

    let mut response = next.run(request).instrument(span).await;

    // Echo the trace id in the response header.
    if let Ok(v) = HeaderValue::from_str(&tid) {
        response.headers_mut().insert(TRACE_ID_HEADER, v);
    }

    response
}

/// Extension type for passing the trace id from middleware to handlers.
#[derive(Debug, Clone)]
pub struct TraceId(pub String);
