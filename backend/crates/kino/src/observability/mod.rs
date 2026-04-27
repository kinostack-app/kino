//! Observability — persistent structured logs written to `SQLite`.
//!
//! On every `tracing` event a `LogRecord` is built (redacted) and
//! `try_send` into a bounded mpsc channel. A dedicated writer task batches
//! records (256 rows / 250 ms, whichever first) and issues one
//! multi-value `INSERT` into `log_entry`. A drop counter tracks events
//! lost to channel pressure; the writer periodically emits a synthetic
//! "dropped N log events" line when the counter is non-zero.
//!
//! Frontend errors arrive via `POST /api/v1/client-logs` and land in the
//! same table with `source = 'frontend'`, so `/settings/logs` is the one
//! place to look for incidents anywhere in the app.
//!
//! ## Public API
//!
//! - `LogBus`, `LogRecord` — the mpsc + broadcast shape `AppState`
//!   carries; producers send `LogRecord`s through
//! - `new_bus`, `level_int`, `subsystem_from_target` — boot helpers
//!   used by main.rs to wire the pipeline
//! - `CHANNEL_CAPACITY`, `BROADCAST_CAPACITY` — pipeline tuning
//!   constants exposed for the writer task and the WS subscriber
//! - `layer` — the tracing Layer producers go through
//! - `writer` — the persistence task spawned at boot
//! - `redact` — string-level secret redaction
//! - `trace` — trace-id span helper
//! - `log_retention::sweep` — scheduler-driven cap on row count
//! - `handlers` — `/logs` HTTP surface (read + tail-via-WS)

pub mod handlers;
pub mod layer;
pub mod log_retention;
pub mod redact;
pub mod trace;
pub mod writer;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::{broadcast, mpsc};

/// Channel capacity for the log pipeline. At ~256 bytes/record this is
/// ~1 MB of buffered logs — enough to smooth over brief DB write hiccups,
/// small enough to surface real backpressure.
pub const CHANNEL_CAPACITY: usize = 4096;

/// Live-tail broadcast buffer. Short on purpose — subscribers that
/// lag drop records (`broadcast::Receiver` returns Lagged). The /logs
/// UI treats this as best-effort; persisted rows are the source of
/// truth for anything that matters.
pub const BROADCAST_CAPACITY: usize = 512;

/// A log record ready to be persisted. `try_send` across threads — must
/// be `Send` + owned strings (no borrowed lifetimes from tracing).
#[derive(Debug, Clone)]
pub struct LogRecord {
    /// Unix microseconds when the event fired.
    pub ts_us: i64,
    /// 0=ERROR 1=WARN 2=INFO 3=DEBUG 4=TRACE — ints sort cheaply.
    pub level: u8,
    /// Module path, e.g. `"kino::acquisition::search"`.
    pub target: String,
    /// First segment under `kino::` — "services", "download", "playback".
    pub subsystem: Option<String>,
    /// Correlation id (short hex) inherited from the enclosing span.
    pub trace_id: Option<String>,
    /// The current span id at emission time (diagnostic aid).
    pub span_id: Option<String>,
    /// Human-readable message. Already redacted.
    pub message: String,
    /// Structured fields as JSON object (already redacted), or None.
    pub fields_json: Option<String>,
    /// `"backend"` or `"frontend"`.
    pub source: &'static str,
}

/// Handles to pass around. The tracing Layer holds one of these; ad-hoc
/// producers (e.g. `POST /client-logs`) also emit through it; the WS
/// log-stream handler subscribes to `live`.
#[derive(Clone)]
pub struct LogBus {
    /// Bounded channel → persistent writer (cares about every record).
    pub tx: mpsc::Sender<LogRecord>,
    /// Atomic drop counter for the writer channel. Surfaces in
    /// operational metrics when non-zero.
    pub drops: Arc<AtomicU64>,
    /// Lossy broadcast for live-tail subscribers. Slow consumers lag.
    pub live: broadcast::Sender<LogRecord>,
}

impl std::fmt::Debug for LogBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogBus")
            .field("capacity", &self.tx.capacity())
            .field(
                "drops",
                &self.drops.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field("live_subs", &self.live.receiver_count())
            .finish()
    }
}

/// Create the mpsc channel + drop counter + live broadcast. The sender
/// goes into a tracing Layer; the receiver goes into the writer task
/// (spawned once the DB pool is ready). The broadcast is subscribed
/// to by WS clients via `GET /api/v1/logs/stream`.
pub fn new_bus() -> (LogBus, mpsc::Receiver<LogRecord>) {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
    let drops = Arc::new(AtomicU64::new(0));
    (LogBus { tx, drops, live }, rx)
}

/// Convert a `tracing::Level` to our compact integer encoding.
pub fn level_int(l: tracing::Level) -> u8 {
    match l {
        tracing::Level::ERROR => 0,
        tracing::Level::WARN => 1,
        tracing::Level::INFO => 2,
        tracing::Level::DEBUG => 3,
        tracing::Level::TRACE => 4,
    }
}

/// Derive a human-friendly subsystem tag from a module path. We map
/// `"kino::acquisition::search"` to `"acquisition"` so the UI can filter on
/// the top-level area without regex. Dependencies' logs (`librqbit`,
/// `sqlx`, …) are grouped under their crate name.
pub fn subsystem_from_target(target: &str) -> Option<String> {
    target
        .strip_prefix("kino::")
        .and_then(|rest| rest.split("::").next())
        .map(str::to_owned)
        .or_else(|| target.split("::").next().map(str::to_owned))
}
