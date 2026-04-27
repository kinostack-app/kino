//! `tracing_subscriber::Layer` that converts events into `LogRecord`s
//! and pushes them at the writer task via a bounded mpsc.
//!
//! Guarantees:
//!   * Non-blocking on the hot path — `try_send` only. On channel full
//!     the record is dropped and the shared counter is bumped.
//!   * No DB / disk I/O inside `on_event` — writer task owns that.
//!   * Cheap enough to run on every event without measurably affecting
//!     the app (regex redaction is the most expensive part; see
//!     `redact.rs`).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use tokio::sync::{broadcast, mpsc};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use super::{LogRecord, level_int, redact, subsystem_from_target};

/// Trace-id stored as a span extension so descendants inherit it.
#[derive(Debug, Clone)]
pub struct TraceId(pub String);

/// Layer that ships every event into both the persistent writer
/// (mpsc → `SQLite`) and the live-tail broadcast (lossy). Construct with
/// `SqliteLogLayer::from_bus(&bus)`.
pub struct SqliteLogLayer {
    tx: mpsc::Sender<LogRecord>,
    drops: Arc<AtomicU64>,
    live: broadcast::Sender<LogRecord>,
}

impl std::fmt::Debug for SqliteLogLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteLogLayer").finish()
    }
}

impl SqliteLogLayer {
    pub fn from_bus(bus: &super::LogBus) -> Self {
        Self {
            tx: bus.tx.clone(),
            drops: bus.drops.clone(),
            live: bus.live.clone(),
        }
    }
}

impl<S> Layer<S> for SqliteLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        // If the span declares a `trace_id` field, stash it in extensions
        // so descendant events can walk up and find it.
        let mut visitor = TraceIdVisitor(None);
        attrs.record(&mut visitor);
        if let Some(trace) = visitor.0
            && let Some(span) = ctx.span(id)
        {
            span.extensions_mut().insert(TraceId(trace));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();

        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        // Redact message + each field value before enqueueing so the
        // record on the channel is already safe to persist.
        let message = redact::redact(&visitor.message);
        let mut fields = visitor.fields;
        for v in fields.values_mut() {
            *v = redact::redact(v);
        }

        // Walk span scope root→leaf; deepest TraceId wins (child spans
        // can override). `lookup_current` is None outside any span.
        let (trace_id, span_id) = ctx.lookup_current().map_or((None, None), |span| {
            let span_id = format!("{:x}", span.id().into_u64());
            let mut tid: Option<String> = None;
            for s in span.scope().from_root() {
                if let Some(t) = s.extensions().get::<TraceId>() {
                    tid = Some(t.0.clone());
                }
            }
            (tid, Some(span_id))
        });

        let fields_json = if fields.is_empty() {
            None
        } else {
            serde_json::to_string(&fields).ok()
        };

        let record = LogRecord {
            ts_us: Utc::now().timestamp_micros(),
            level: level_int(*meta.level()),
            target: meta.target().to_owned(),
            subsystem: subsystem_from_target(meta.target()),
            trace_id,
            span_id,
            message,
            fields_json,
            source: "backend",
        };

        // Fan out: persistent writer (may drop on full) + live broadcast
        // (may return Err if there are zero subscribers, which is fine).
        let _ = self.live.send(record.clone());
        if self.tx.try_send(record).is_err() {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Collect the `message` field + everything else into an ordered map of
/// strings (we stringify everything; the exact-precision dance isn't
/// worth it for log consumption).
#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl EventVisitor {
    fn set(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = value;
        } else {
            self.fields.insert(field.name().to_owned(), value);
        }
    }
}

impl Visit for EventVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field, value.to_owned());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.set(field, value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.set(field, value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.set(field, value.to_string());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.set(field, value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.set(field, format!("{value:?}"));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.set(field, value.to_string());
    }
}

/// Extract a `trace_id` field from a span's attributes.
struct TraceIdVisitor(Option<String>);

impl Visit for TraceIdVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "trace_id" {
            self.0 = Some(value.to_owned());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "trace_id" {
            // Handle `tracing::field::display(x)` where Display is used.
            // Strip surrounding quotes if any.
            let s = format!("{value:?}");
            self.0 = Some(s.trim_matches('"').to_owned());
        }
    }
}
