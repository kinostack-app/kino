//! Log query + client-log ingest endpoints.
//!
//! `GET  /api/v1/logs` — query with filters (level, subsystem, since,
//!                       until, `trace_id`, q). Cursor pagination by id DESC.
//! `POST /api/v1/client-logs` — frontend-originated events; each lands
//!                       in the same `log_entry` table with source='frontend'.

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::observability::LogRecord;
use crate::state::AppState;

/// One row from `log_entry` in a shape friendly for JSON consumers.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct LogEntryRow {
    pub id: i64,
    pub ts_us: i64,
    pub level: i64,
    pub target: String,
    pub subsystem: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub message: String,
    pub fields_json: Option<String>,
    pub source: String,
}

/// Query parameters for `GET /api/v1/logs`.
///
/// Defaults: `limit=200`, unordered otherwise. Cursor is an id — pass
/// the oldest id from the previous page as `before` to paginate back.
#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// Minimum level (numeric). 0=ERROR 1=WARN 2=INFO 3=DEBUG 4=TRACE.
    /// Rows with level <= this are returned. Default: 2 (INFO+).
    pub level: Option<i64>,
    /// Filter by subsystem ("services", "download", "indexers", …).
    pub subsystem: Option<String>,
    /// Filter by source ("backend", "frontend").
    pub source: Option<String>,
    /// Filter by trace id (exact match).
    pub trace_id: Option<String>,
    /// Substring search on message (case-insensitive).
    pub q: Option<String>,
    /// Unix micros — only rows at or after this time.
    pub since_us: Option<i64>,
    /// Pagination: only rows with id < this.
    pub before: Option<i64>,
    /// Page size (default 200, max 1000).
    pub limit: Option<i64>,
}

/// Query recent log entries.
#[utoipa::path(
    get, path = "/api/v1/logs",
    params(
        ("level" = Option<i64>, Query, description = "Max numeric level"),
        ("subsystem" = Option<String>, Query),
        ("source" = Option<String>, Query),
        ("trace_id" = Option<String>, Query),
        ("q" = Option<String>, Query, description = "Message substring"),
        ("since_us" = Option<i64>, Query, description = "Unix micros floor"),
        ("before" = Option<i64>, Query, description = "Id cursor (exclusive)"),
        ("limit" = Option<i64>, Query),
    ),
    responses((status = 200, body = Vec<LogEntryRow>)),
    tag = "logs", security(("api_key" = []))
)]
pub async fn list_logs(
    State(state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> AppResult<Json<Vec<LogEntryRow>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let level = q.level.unwrap_or(2);

    // Dynamic WHERE via QueryBuilder — sticks with bind parameters and
    // keeps the optional filters composable without format! injection.
    let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
        "SELECT id, ts_us, level, target, subsystem, trace_id, span_id, message, fields_json, source
         FROM log_entry WHERE level <= ",
    );
    builder.push_bind(level);

    if let Some(ref sub) = q.subsystem {
        builder.push(" AND subsystem = ").push_bind(sub);
    }
    if let Some(ref src) = q.source {
        builder.push(" AND source = ").push_bind(src);
    }
    if let Some(ref tid) = q.trace_id {
        builder.push(" AND trace_id = ").push_bind(tid);
    }
    if let Some(ref needle) = q.q {
        // LIKE is case-insensitive by default in SQLite for ASCII.
        let pattern = format!("%{needle}%");
        builder.push(" AND message LIKE ").push_bind(pattern);
    }
    if let Some(since) = q.since_us {
        builder.push(" AND ts_us >= ").push_bind(since);
    }
    if let Some(before) = q.before {
        builder.push(" AND id < ").push_bind(before);
    }
    builder.push(" ORDER BY id DESC LIMIT ").push_bind(limit);

    let rows = builder
        .build_query_as::<LogEntryRow>()
        .fetch_all(&state.db)
        .await?;

    Ok(Json(rows))
}

/// One frontend-originated log entry submitted via `/client-logs`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ClientLogEntry {
    /// "error" | "warn" | "info" | "debug"
    pub level: String,
    pub message: String,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Unix milliseconds when the event happened on the client. We
    /// accept ms because JS `Date.now()` is ms; server converts to us.
    #[serde(default)]
    pub ts_ms: Option<i64>,
    /// Optional duplicate-count when the frontend collapses a repeat.
    #[serde(default)]
    pub count: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ClientLogsPayload {
    pub entries: Vec<ClientLogEntry>,
}

/// Export matching log entries as NDJSON (one JSON object per line).
///
/// Uses the same filter set as `list_logs`. Ordered chronologically
/// (ascending `ts_us`) which is the natural shape for a log file —
/// easier to grep, easier to tail, easier to re-ingest. Capped at
/// 100k rows to keep memory predictable for a home server.
#[utoipa::path(
    get, path = "/api/v1/logs/export",
    params(
        ("level" = Option<i64>, Query),
        ("subsystem" = Option<String>, Query),
        ("source" = Option<String>, Query),
        ("trace_id" = Option<String>, Query),
        ("q" = Option<String>, Query),
        ("since_us" = Option<i64>, Query),
    ),
    responses(
        (status = 200, description = "NDJSON stream", content_type = "application/x-ndjson"),
    ),
    tag = "logs", security(("api_key" = []))
)]
pub async fn export_logs(
    State(state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> AppResult<axum::response::Response> {
    let level = q.level.unwrap_or(4); // Default: export everything DEBUG and above.

    let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
        "SELECT id, ts_us, level, target, subsystem, trace_id, span_id, message, fields_json, source
         FROM log_entry WHERE level <= ",
    );
    builder.push_bind(level);
    if let Some(ref sub) = q.subsystem {
        builder.push(" AND subsystem = ").push_bind(sub);
    }
    if let Some(ref src) = q.source {
        builder.push(" AND source = ").push_bind(src);
    }
    if let Some(ref tid) = q.trace_id {
        builder.push(" AND trace_id = ").push_bind(tid);
    }
    if let Some(ref needle) = q.q {
        let pattern = format!("%{needle}%");
        builder.push(" AND message LIKE ").push_bind(pattern);
    }
    if let Some(since) = q.since_us {
        builder.push(" AND ts_us >= ").push_bind(since);
    }
    // ASC here, not DESC — export order is chronological for readability.
    builder.push(" ORDER BY id ASC LIMIT 100000");

    let rows = builder
        .build_query_as::<LogEntryRow>()
        .fetch_all(&state.db)
        .await?;

    let mut body = String::with_capacity(rows.len() * 200);
    for row in &rows {
        match serde_json::to_string(row) {
            Ok(json) => {
                body.push_str(&json);
                body.push('\n');
            }
            Err(e) => {
                tracing::warn!(id = row.id, error = %e, "failed to serialise log row for export");
            }
        }
    }

    let now = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("kino-logs-{now}.ndjson");

    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/x-ndjson")
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            format!(r#"attachment; filename="{filename}""#),
        )
        .body(axum::body::Body::from(body))
        .expect("valid response"))
}

/// WebSocket live-tail — streams new log entries as JSON messages as
/// they're emitted. No filter server-side; the UI applies filters
/// client-side to avoid double-work and let the user toggle them live.
pub async fn stream_logs(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_stream(socket, state.log_live.subscribe()))
}

/// JSON payload the stream emits for each log line.
#[derive(Debug, Serialize)]
struct StreamRow<'a> {
    ts_us: i64,
    level: u8,
    target: &'a str,
    subsystem: Option<&'a str>,
    trace_id: Option<&'a str>,
    span_id: Option<&'a str>,
    message: &'a str,
    fields_json: Option<&'a str>,
    source: &'a str,
}

impl<'a> From<&'a LogRecord> for StreamRow<'a> {
    fn from(r: &'a LogRecord) -> Self {
        Self {
            ts_us: r.ts_us,
            level: r.level,
            target: &r.target,
            subsystem: r.subsystem.as_deref(),
            trace_id: r.trace_id.as_deref(),
            span_id: r.span_id.as_deref(),
            message: &r.message,
            fields_json: r.fields_json.as_deref(),
            source: r.source,
        }
    }
}

async fn handle_stream(mut socket: WebSocket, mut rx: broadcast::Receiver<LogRecord>) {
    loop {
        tokio::select! {
            // Pump new log records downstream.
            msg = rx.recv() => match msg {
                Ok(record) => {
                    let row = StreamRow::from(&record);
                    let Ok(json) = serde_json::to_string(&row) else { continue };
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Tell the client we skipped some — UI can indicate
                    // "log stream lagged, reload to see the full tail".
                    let msg = format!(r#"{{"lagged":{n}}}"#);
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Read incoming frames so keepalive/close messages flow.
            frame = socket.recv() => match frame {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                _ => {}
            }
        }
    }
}

/// Ingest a batch of frontend-originated log entries.
#[utoipa::path(
    post, path = "/api/v1/client-logs",
    request_body = ClientLogsPayload,
    responses((status = 204), (status = 413, description = "Too many entries")),
    tag = "logs", security(("api_key" = []))
)]
pub async fn ingest_client_logs(
    State(state): State<AppState>,
    Json(payload): Json<ClientLogsPayload>,
) -> AppResult<StatusCode> {
    // Rate limit: cap per-request at 100 entries. Frontend batches in
    // groups of 20; anything past 100 is almost certainly a runaway loop.
    if payload.entries.len() > 100 {
        return Err(AppError::Unprocessable(
            "too many entries in one batch (max 100)".into(),
        ));
    }

    for entry in payload.entries {
        let level = match entry.level.to_ascii_lowercase().as_str() {
            "error" => 0_i64,
            "warn" | "warning" => 1,
            "debug" => 3,
            "trace" => 4,
            _ => 2, // info / unknown
        };
        // Prefer client-supplied ts; else record arrival time.
        let ts_us = entry
            .ts_ms
            .map_or_else(|| chrono::Utc::now().timestamp_micros(), |ms| ms * 1000);

        // Message: original + optional stack + url + count annotations,
        // redacted before INSERT. The writer task's own redaction only
        // sees layer-emitted records; for the client path we redact here.
        let mut message = entry.message.clone();
        if let Some(n) = entry.count
            && n > 1
        {
            message = format!("{message} (x{n})");
        }
        if let Some(ref u) = entry.url {
            message = format!("{message} [{u}]");
        }
        let message = crate::observability::redact::redact(&message);
        let fields_json = entry.stack.as_ref().map(|s| {
            let redacted = crate::observability::redact::redact(s);
            serde_json::json!({ "stack": redacted }).to_string()
        });

        sqlx::query(
            "INSERT INTO log_entry (ts_us, level, target, subsystem, trace_id, span_id, message, fields_json, source)
             VALUES (?, ?, 'frontend', 'frontend', NULL, NULL, ?, ?, 'frontend')",
        )
        .bind(ts_us)
        .bind(level)
        .bind(&message)
        .bind(&fields_json)
        .execute(&state.db)
        .await?;

        // Also publish to the live WS bus so the /settings/logs live tail
        // shows frontend entries without waiting for the next REST poll.
        // Broadcast send fails only when there are no subscribers;
        // harmless.
        let _ = state.log_live.send(crate::observability::LogRecord {
            ts_us,
            level: u8::try_from(level).unwrap_or(2),
            target: "frontend".into(),
            subsystem: Some("frontend".into()),
            trace_id: None,
            span_id: None,
            message,
            fields_json,
            source: "frontend",
        });
    }

    Ok(StatusCode::NO_CONTENT)
}
