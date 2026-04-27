//! Batching log writer task.
//!
//! Drains the mpsc receiver in batches (up to `BATCH_SIZE` rows or
//! `BATCH_TIMEOUT` elapsed) and issues one multi-value `INSERT` per
//! batch. Runs until the sender is dropped or the cancel token fires.
//!
//! Backpressure: producers (the tracing layer, the client-log endpoint)
//! use `try_send`; when the channel is full, records are dropped and the
//! shared `drops` counter is incremented. The writer emits one synthetic
//! `WARN` log per "reporting interval" when the counter is non-zero, so
//! operators see dropped-events without the writer creating an
//! amplification loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::LogRecord;

const BATCH_SIZE: usize = 256;
const BATCH_TIMEOUT: Duration = Duration::from_millis(250);
const DROP_REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// Run the writer loop until `cancel` fires or the sender is dropped.
pub async fn run(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<LogRecord>,
    drops: Arc<AtomicU64>,
    cancel: CancellationToken,
) {
    let mut next_drop_report = Instant::now() + DROP_REPORT_INTERVAL;
    loop {
        // Wait for the first record — block until something arrives OR
        // shutdown. This avoids spinning when idle.
        let first = tokio::select! {
            () = cancel.cancelled() => break,
            r = rx.recv() => match r {
                Some(r) => r,
                None => break, // all senders dropped — orderly shutdown
            }
        };

        // Collect the rest of the batch. Break on BATCH_SIZE or timeout.
        let mut batch: Vec<LogRecord> = Vec::with_capacity(BATCH_SIZE);
        batch.push(first);
        let deadline = tokio::time::sleep(BATCH_TIMEOUT);
        tokio::pin!(deadline);

        while batch.len() < BATCH_SIZE {
            tokio::select! {
                () = &mut deadline => break,
                r = rx.recv() => match r {
                    Some(r) => batch.push(r),
                    None => break,
                }
            }
        }

        if let Err(e) = flush(&pool, &batch).await {
            // If the DB is unreachable, we must NOT log via tracing here —
            // that would feed back into the channel. Eprintln only.
            eprintln!("log writer: flush failed: {e}");
        }

        // Periodic drop report. Safe because it goes via tracing, which
        // sends back into the queue — one record, not a torrent.
        if Instant::now() >= next_drop_report {
            let d = drops.swap(0, Ordering::Relaxed);
            if d > 0 {
                tracing::warn!(dropped = d, "log writer dropped events (channel full)");
            }
            next_drop_report = Instant::now() + DROP_REPORT_INTERVAL;
        }
    }

    // Final drain on shutdown.
    let mut tail = Vec::new();
    while let Ok(r) = rx.try_recv() {
        tail.push(r);
    }
    if !tail.is_empty() {
        let _ = flush(&pool, &tail).await;
    }
}

async fn flush(pool: &SqlitePool, batch: &[LogRecord]) -> sqlx::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    let mut qb: QueryBuilder<'_, Sqlite> = QueryBuilder::new(
        "INSERT INTO log_entry (ts_us, level, target, subsystem, trace_id, span_id, message, fields_json, source) ",
    );

    qb.push_values(batch, |mut b, r| {
        b.push_bind(r.ts_us)
            .push_bind(r.level)
            .push_bind(&r.target)
            .push_bind(r.subsystem.as_deref())
            .push_bind(r.trace_id.as_deref())
            .push_bind(r.span_id.as_deref())
            .push_bind(&r.message)
            .push_bind(r.fields_json.as_deref())
            .push_bind(r.source);
    });

    qb.build().execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn flush_writes_rows() {
        let pool = db::create_test_pool().await;
        let now = chrono::Utc::now().timestamp_micros();
        let batch = vec![
            LogRecord {
                ts_us: now,
                level: 2,
                target: "kino::test".into(),
                subsystem: Some("test".into()),
                trace_id: None,
                span_id: None,
                message: "hello".into(),
                fields_json: None,
                source: "backend",
            },
            LogRecord {
                ts_us: now + 1,
                level: 1,
                target: "kino::test".into(),
                subsystem: Some("test".into()),
                trace_id: Some("abc123".into()),
                span_id: Some("s1".into()),
                message: "warn!".into(),
                fields_json: Some(r#"{"k":"v"}"#.into()),
                source: "backend",
            },
        ];
        flush(&pool, &batch).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_entry")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }
}
