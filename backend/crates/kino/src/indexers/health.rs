//! Indexer health check — pings each enabled indexer on a schedule,
//! escalates failures with exponential backoff, restores on success.
//! On a successful probe against a Torznab indexer, also parses the
//! `?t=caps` response and persists the declared tv/movie-search
//! params + category IDs so `acquisition::search` can build narrower
//! queries (see `torznab::caps`).
//!
//! Escalation ladder (minutes): 30m → 6h → 24h → 7d → 30d.
//! Each failure bumps `escalation_level` and sets `disabled_until`.
//! A successful check resets both to zero.

use sqlx::SqlitePool;
use std::time::Duration;

use crate::events::{AppEvent, IndexerAction};
use crate::indexers::model::Indexer;
use crate::state::AppState;
use crate::torznab::caps::{TorznabCapabilities, parse_caps};

/// How long to disable an indexer after `n` consecutive failures.
fn backoff_duration(level: i64) -> chrono::Duration {
    match level {
        0 | 1 => chrono::Duration::minutes(30),
        2 => chrono::Duration::hours(6),
        3 => chrono::Duration::hours(24),
        4 => chrono::Duration::days(7),
        _ => chrono::Duration::days(30),
    }
}

/// Sweep all enabled indexers and probe their health.
///
/// `event_tx` gets an `IndexerChanged { action: HealthChanged }` event
/// whenever an indexer's escalation level transitions so the frontend
/// can refetch state without polling.
#[tracing::instrument(skip(state))]
pub async fn health_sweep(state: &AppState) -> anyhow::Result<u64> {
    let pool = &state.db;
    let event_tx = &state.event_tx;
    let indexers = sqlx::query_as::<_, Indexer>("SELECT * FROM indexer WHERE enabled = 1")
        .fetch_all(pool)
        .await?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let mut checked = 0u64;
    let mut recovered = 0u64;
    let mut still_failing = 0u64;
    let mut newly_failing = 0u64;
    for indexer in &indexers {
        let was_failing = indexer.escalation_level > 0;
        let probe = probe_indexer(&http, state, indexer).await;
        if probe.reachable {
            mark_healthy(pool, indexer.id).await?;
            if let Some(caps) = probe.caps {
                if let Err(e) = persist_caps(pool, indexer.id, &caps).await {
                    tracing::warn!(
                        indexer = %indexer.name,
                        indexer_id = indexer.id,
                        error = %e,
                        "failed to persist indexer capabilities"
                    );
                } else {
                    tracing::debug!(
                        indexer = %indexer.name,
                        indexer_id = indexer.id,
                        tv_available = caps.tv_available(),
                        movie_available = caps.movie_available(),
                        tv_params = caps.tv_search.as_ref().map_or(0, |m| m.supported_params.len()),
                        movie_params = caps.movie_search.as_ref().map_or(0, |m| m.supported_params.len()),
                        categories = caps.categories.len(),
                        "indexer capabilities refreshed"
                    );
                }
            }
            if was_failing {
                recovered += 1;
                tracing::info!(
                    indexer = %indexer.name,
                    indexer_id = indexer.id,
                    "indexer recovered — escalation cleared",
                );
                let _ = event_tx.send(AppEvent::IndexerChanged {
                    indexer_id: indexer.id,
                    action: IndexerAction::HealthChanged,
                });
            }
        } else {
            let new_level = indexer.escalation_level + 1;
            if was_failing {
                still_failing += 1;
            } else {
                newly_failing += 1;
            }
            tracing::warn!(
                indexer = %indexer.name,
                indexer_id = indexer.id,
                prev_level = indexer.escalation_level,
                new_level,
                "indexer probe failed — escalating",
            );
            mark_failed(pool, indexer.id, indexer.escalation_level).await?;
            let _ = event_tx.send(AppEvent::IndexerChanged {
                indexer_id: indexer.id,
                action: IndexerAction::HealthChanged,
            });
        }
        checked += 1;
    }
    if recovered > 0 || newly_failing > 0 || still_failing > 0 {
        tracing::info!(
            checked,
            recovered,
            newly_failing,
            still_failing,
            "indexer health sweep complete",
        );
    }
    Ok(checked)
}

/// Outcome of a single indexer probe. `reachable` controls the
/// escalation ladder; `caps` is `Some` only when the probe returned
/// valid Torznab caps XML (cardigann probes return `None`).
struct ProbeResult {
    reachable: bool,
    caps: Option<TorznabCapabilities>,
}

async fn probe_indexer(http: &reqwest::Client, state: &AppState, indexer: &Indexer) -> ProbeResult {
    if indexer.indexer_type == "cardigann" {
        // Run an actual search through the production engine —
        // template + auth + cookie jar + cf_solver — instead of a
        // bare HEAD. A site that returns 200 on HEAD but fails the
        // login flow or the search template is "down" for kino's
        // purposes; the prod-path probe catches that.
        let reachable = probe_cardigann(state, indexer).await;
        ProbeResult {
            reachable,
            caps: None,
        }
    } else {
        // Torznab: hit the caps endpoint — doubles as both health
        // probe and capability refresh.
        let mut url = format!("{}?t=caps", indexer.url.trim_end_matches('/'));
        if let Some(ref key) = indexer.api_key {
            use std::fmt::Write;
            let _ = write!(url, "&apikey={key}");
        }
        match http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(body) => {
                    let caps = parse_caps(&body);
                    ProbeResult {
                        reachable: true,
                        caps: Some(caps),
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        indexer = %indexer.name,
                        error = %e,
                        "caps body read failed after 2xx"
                    );
                    ProbeResult {
                        reachable: true,
                        caps: None,
                    }
                }
            },
            Ok(resp) => {
                tracing::debug!(
                    indexer = %indexer.name,
                    status = resp.status().as_u16(),
                    "caps probe non-2xx"
                );
                ProbeResult {
                    reachable: false,
                    caps: None,
                }
            }
            Err(e) => {
                tracing::debug!(indexer = %indexer.name, error = %e, "caps probe failed");
                ProbeResult {
                    reachable: false,
                    caps: None,
                }
            }
        }
    }
}

/// Run a real "test" search through the Cardigann engine (same
/// path the search loop uses). Returns true when the search
/// returned without erroring — including zero-result responses,
/// since "indexer up but no hits for `test`" is a healthy state.
async fn probe_cardigann(state: &AppState, indexer: &Indexer) -> bool {
    use crate::indexers::request::IndexerClient;
    use crate::indexers::template::SearchQuery;

    let Ok(definitions) = state.require_definitions() else {
        tracing::warn!(
            indexer = %indexer.name,
            "cardigann probe skipped — definitions not loaded",
        );
        return false;
    };
    let Some(definition_id) = indexer.definition_id.as_deref() else {
        tracing::warn!(
            indexer = %indexer.name,
            "cardigann probe skipped — no definition_id",
        );
        return false;
    };
    let Some(definition) = definitions.get(definition_id) else {
        tracing::warn!(
            indexer = %indexer.name,
            definition_id,
            "cardigann probe skipped — definition not found",
        );
        return false;
    };
    let settings: std::collections::HashMap<String, String> = indexer
        .settings_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let query = SearchQuery {
        q: "test".to_string(),
        keywords: "test".to_string(),
        ..Default::default()
    };
    let client = IndexerClient::new(state.cf_solver.clone());
    match crate::indexers::search(&client, &definition, &settings, &query).await {
        Ok(_) => true,
        Err(e) => {
            tracing::debug!(
                indexer = %indexer.name,
                error = %e,
                "cardigann probe failed",
            );
            false
        }
    }
}

/// Persist parsed caps to the indexer row. Stored as JSON so the
/// frontend can render "this indexer accepts IMDb IDs" hints without
/// re-parsing; `search.rs` reads the same JSON back via `serde_json`.
#[allow(clippy::doc_markdown)] // prose IMDb, not a code identifier
async fn persist_caps(
    pool: &SqlitePool,
    id: i64,
    caps: &TorznabCapabilities,
) -> anyhow::Result<()> {
    // Store only the `tv_search` / `movie_search` fields of the
    // capabilities struct — `categories` lives in its own column.
    // `serde_json` on the struct excludes the `categories` Vec via
    // a manual strip here to keep the column shape small.
    #[derive(serde::Serialize)]
    struct StoredSearch<'a> {
        tv_search: Option<&'a crate::torznab::caps::SearchMode>,
        movie_search: Option<&'a crate::torznab::caps::SearchMode>,
    }
    let search_json = serde_json::to_string(&StoredSearch {
        tv_search: caps.tv_search.as_ref(),
        movie_search: caps.movie_search.as_ref(),
    })?;
    let categories_json = serde_json::to_string(&caps.categories)?;
    sqlx::query(
        "UPDATE indexer SET supported_search_params = ?, supported_categories = ? WHERE id = ?",
    )
    .bind(&search_json)
    .bind(&categories_json)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_healthy(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE indexer SET
             escalation_level = 0,
             initial_failure_time = NULL,
             most_recent_failure_time = NULL,
             disabled_until = NULL
         WHERE id = ?",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_failed(pool: &SqlitePool, id: i64, current_level: i64) -> anyhow::Result<()> {
    let new_level = current_level + 1;
    let disabled_until = crate::time::Timestamp::now_plus(backoff_duration(new_level)).to_rfc3339();
    let now = crate::time::Timestamp::now().to_rfc3339();

    sqlx::query(
        "UPDATE indexer SET
             escalation_level = ?,
             initial_failure_time = COALESCE(initial_failure_time, ?),
             most_recent_failure_time = ?,
             disabled_until = ?
         WHERE id = ?",
    )
    .bind(new_level)
    .bind(&now)
    .bind(&now)
    .bind(&disabled_until)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn backoff_ladder() {
        assert_eq!(backoff_duration(0), chrono::Duration::minutes(30));
        assert_eq!(backoff_duration(1), chrono::Duration::minutes(30));
        assert_eq!(backoff_duration(2), chrono::Duration::hours(6));
        assert_eq!(backoff_duration(3), chrono::Duration::hours(24));
        assert_eq!(backoff_duration(4), chrono::Duration::days(7));
        assert_eq!(backoff_duration(100), chrono::Duration::days(30));
    }

    #[tokio::test]
    async fn mark_healthy_clears_failure_state() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO indexer (name, url, indexer_type, enabled, escalation_level, initial_failure_time, most_recent_failure_time, disabled_until)
             VALUES ('test', 'http://localhost', 'torznab', 1, 2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2100-01-01T00:00:00Z') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        mark_healthy(&pool, id).await.unwrap();

        let row: (i64, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as("SELECT escalation_level, initial_failure_time, most_recent_failure_time, disabled_until FROM indexer WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, 0);
        assert!(row.1.is_none());
        assert!(row.2.is_none());
        assert!(row.3.is_none());
    }

    #[tokio::test]
    async fn mark_failed_escalates_and_sets_disabled_until() {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO indexer (name, url, indexer_type, enabled, escalation_level) VALUES ('t', 'http://localhost', 'torznab', 1, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        mark_failed(&pool, id, 0).await.unwrap();
        let (level, disabled, initial): (i64, Option<String>, Option<String>) =
            sqlx::query_as("SELECT escalation_level, disabled_until, initial_failure_time FROM indexer WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(level, 1);
        assert!(disabled.is_some());
        assert!(initial.is_some());

        // Second failure — initial_failure_time stays, level bumps
        mark_failed(&pool, id, level).await.unwrap();
        let (level2, initial2): (i64, Option<String>) = sqlx::query_as(
            "SELECT escalation_level, initial_failure_time FROM indexer WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(level2, 2);
        assert_eq!(initial2, initial, "initial_failure_time should not change");
    }
}
