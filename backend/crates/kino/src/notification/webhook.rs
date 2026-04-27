//! Webhook subsystem — model, delivery engine, retry-state, HTTP CRUD.
//!
//! `WebhookTarget` is the row model. `send_once` is the per-target
//! delivery primitive (template rendering + retry-state mutation).
//! The retry sweep that re-tries failed targets lives in
//! `webhook_retry.rs` (kept separate because it's a scheduler-driven
//! background sweep, not part of the per-event-fire path). HTTP
//! handlers (CRUD + manual fire) sit at the bottom.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::events::{AppEvent, IndexerAction};
use crate::state::AppState;

use super::Event;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct WebhookTarget {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub method: String,
    pub headers: Option<String>,
    pub body_template: Option<String>,
    pub on_grab: bool,
    pub on_download_complete: bool,
    pub on_import: bool,
    pub on_upgrade: bool,
    pub on_failure: bool,
    pub on_watched: bool,
    pub on_health_issue: bool,
    pub enabled: bool,
    pub initial_failure_time: Option<String>,
    pub most_recent_failure_time: Option<String>,
    /// Position on the retry ladder — `0` = healthy, `1..=5` = the
    /// 30s → 15min → 1h → 4h → 24h backoff rungs. Cleared on success
    /// so a recovered target starts fresh next time it fails.
    pub escalation_level: i64,
    pub disabled_until: Option<String>,
}

/// Retry ladder — backoff duration (in minutes) to apply after the
/// Nth consecutive failure. Index = next escalation level (1..=5).
/// Once a target reaches level 5 we stay at 24h and emit a
/// `HealthWarning` so the operator knows to investigate.
const RETRY_LADDER_MINUTES: [i64; 6] = [
    0,       // level 0 — healthy (unused as a backoff)
    0,       // level 1 — first failure: 30s (special-cased below)
    15,      // level 2 — 15 minutes
    60,      // level 3 — 1 hour
    4 * 60,  // level 4 — 4 hours
    24 * 60, // level 5 — 24 hours (give-up rung)
];

/// Compute the backoff duration for moving *into* the given level.
/// Level 1 is a 30-second nudge — most transient failures (DNS
/// blip, short upstream outage) clear well within that window, so
/// we don't want to hide the webhook for 15 minutes on a single
/// flake.
fn backoff_for_level(level: i64) -> chrono::Duration {
    if level <= 1 {
        return chrono::Duration::seconds(30);
    }
    let idx = usize::try_from(level.min(5)).unwrap_or(5);
    chrono::Duration::minutes(RETRY_LADDER_MINUTES[idx])
}

/// Outcome of a single successful webhook delivery — timing + the
/// target's HTTP status code. Surfaced to the settings page's Test
/// button so the user can tell "delivered to Discord" from "got a
/// 204 from my self-hosted endpoint".
#[derive(Debug, Clone)]
pub struct SendOutcome {
    pub status_code: u16,
    pub duration_ms: u64,
}

/// Send a single event to a single target, bypassing the
/// `enabled` / `disabled_until` / per-event flag checks that
/// `deliver` applies. Used by the "Test webhook" endpoint so
/// admins can verify wiring without having to trigger a real grab
/// or temporarily re-enable a backed-off target.
pub async fn send_once(target: &WebhookTarget, event: &Event) -> anyhow::Result<SendOutcome> {
    let http = Client::new();
    let body = render_body(target, event);
    send_webhook(&http, target, &body).await
}

/// Deliver an event to all matching webhook targets.
///
/// `event_tx` is used to emit a `HealthWarning` if a target
/// exhausts the retry ladder — `None` skips that side effect
/// (used from the `test_webhook` path which bypasses the ladder
/// entirely).
pub async fn deliver(
    pool: &SqlitePool,
    event: &Event,
    event_tx: Option<&broadcast::Sender<AppEvent>>,
) {
    let targets = match sqlx::query_as::<_, WebhookTarget>(
        // `disabled_until` is stored as RFC3339 (Rust-side
        // `crate::time::Timestamp::now().to_rfc3339()`). SQLite's
        // `datetime('now')` returns `YYYY-MM-DD HH:MM:SS` — no `T`,
        // no offset — so a direct lexicographic compare wrongly
        // keeps webhooks disabled until the next `webhook_retry`
        // sweep self-heals. Bind a consistent RFC3339 `now` from
        // Rust both here and wherever else `disabled_until` is
        // read.
        "SELECT * FROM webhook_target WHERE enabled = 1 AND (disabled_until IS NULL OR disabled_until < ?)",
    )
    .bind(crate::time::Timestamp::now().to_rfc3339())
    .fetch_all(pool)
    .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch webhook targets");
            return;
        }
    };

    let http = Client::new();

    for target in &targets {
        if !should_fire(target, &event.event_type) {
            continue;
        }

        let body = render_body(target, event);
        let result = send_webhook(&http, target, &body).await;

        match result {
            Ok(_) => {
                // On success, clear any failure state so a recovered
                // target re-enters the ladder from the bottom rather
                // than carrying old timestamps forever. We only
                // bother writing if there's state to clear.
                if target.escalation_level > 0
                    || target.initial_failure_time.is_some()
                    || target.most_recent_failure_time.is_some()
                {
                    if let Err(db_err) = sqlx::query(
                        "UPDATE webhook_target SET
                             initial_failure_time = NULL,
                             most_recent_failure_time = NULL,
                             escalation_level = 0,
                             disabled_until = NULL
                         WHERE id = ?",
                    )
                    .bind(target.id)
                    .execute(pool)
                    .await
                    {
                        tracing::warn!(
                            webhook_id = target.id,
                            error = %db_err,
                            "failed to clear webhook failure state",
                        );
                    } else {
                        tracing::info!(
                            webhook_id = target.id,
                            target = %target.name,
                            prior_level = target.escalation_level,
                            "webhook recovered; failure state cleared",
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    target = %target.name,
                    event = %event.event_type,
                    error = %e,
                    "webhook delivery failed"
                );
                record_failure(pool, target, event_tx).await;
            }
        }
    }
}

/// Step the target up the retry ladder and persist the new state.
/// On the final rung (level 4 → 5, the 24h give-up) we emit a
/// `HealthWarning` once so the operator knows the target has been
/// quarantined. Subsequent tick failures stay silent — same
/// transition-only shape as the scheduler dedup.
async fn record_failure(
    pool: &SqlitePool,
    target: &WebhookTarget,
    event_tx: Option<&broadcast::Sender<AppEvent>>,
) {
    let now = chrono::Utc::now();
    let next_level = (target.escalation_level + 1).min(5);
    let backoff_until = now + backoff_for_level(next_level);
    let now_str = now.to_rfc3339();
    let backoff_str = backoff_until.to_rfc3339();

    if let Err(db_err) = sqlx::query(
        "UPDATE webhook_target SET
             most_recent_failure_time = ?,
             initial_failure_time = COALESCE(initial_failure_time, ?),
             escalation_level = ?,
             disabled_until = ?
         WHERE id = ?",
    )
    .bind(&now_str)
    .bind(&now_str)
    .bind(next_level)
    .bind(&backoff_str)
    .bind(target.id)
    .execute(pool)
    .await
    {
        tracing::warn!(
            webhook_id = target.id,
            error = %db_err,
            "failed to record webhook backoff",
        );
        return;
    }

    // Only page the operator at the give-up transition (level 4 → 5).
    // Earlier failures already tripped the per-event warn log above;
    // emitting a HealthWarning for each would re-spam across all UI
    // surfaces. Note the health warning uses the event bus, so it
    // naturally reaches any webhook with `on_health_issue = 1` —
    // including *other* healthy targets, which is the desired "Slack
    // is healthy, Discord is dead" fan-out.
    if target.escalation_level == 4 && next_level == 5 {
        tracing::error!(
            target = %target.name,
            webhook_id = target.id,
            "webhook exhausted retry ladder; backing off 24h per delivery",
        );
        if let Some(tx) = event_tx {
            let _ = tx.send(AppEvent::HealthWarning {
                message: format!(
                    "Webhook '{}' has failed 5 times in a row; further deliveries are \
                     rate-limited to one attempt per 24 hours until it recovers.",
                    target.name
                ),
            });
        }
    }
}

/// Check if a webhook target should fire for a given event type.
/// Event-type strings here match `AppEvent::event_type()` exactly —
/// no aliases or old-name fallbacks. When the backend emits a new
/// name, update both sides in lockstep.
fn should_fire(target: &WebhookTarget, event_type: &str) -> bool {
    match event_type {
        "release_grabbed" => target.on_grab,
        "download_complete" => target.on_download_complete,
        "imported" => target.on_import,
        "upgraded" => target.on_upgrade,
        "download_failed" => target.on_failure,
        "watched" => target.on_watched,
        "health_warning" => target.on_health_issue,
        _ => false,
    }
}

/// JSON-escape a string: returns it as it would appear *inside* a
/// JSON string literal (so a title `The "Movie"` becomes
/// `The \"Movie\"`). Strips the outer quotes `serde_json` adds —
/// the template already has them.
fn json_escape(s: &str) -> String {
    let encoded = serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""));
    if encoded.starts_with('"') && encoded.ends_with('"') && encoded.len() >= 2 {
        encoded[1..encoded.len() - 1].to_owned()
    } else {
        encoded
    }
}

/// Render webhook body from template or use default JSON. String
/// placeholders are JSON-escaped so a title containing `"`, `\`,
/// or control chars doesn't produce invalid JSON (Discord, Slack,
/// and most well-behaved webhooks reject those with a 400).
/// Numeric placeholders pass through — they're safe inside JSON.
fn render_body(target: &WebhookTarget, event: &Event) -> String {
    if let Some(ref template) = target.body_template {
        let mut body = template.clone();
        body = body.replace("{{event}}", &json_escape(&event.event_type));
        body = body.replace(
            "{{title}}",
            &json_escape(event.title.as_deref().unwrap_or("")),
        );
        body = body.replace(
            "{{show}}",
            &json_escape(event.show.as_deref().unwrap_or("")),
        );
        body = body.replace(
            "{{quality}}",
            &json_escape(event.quality.as_deref().unwrap_or("")),
        );
        body = body.replace(
            "{{message}}",
            &json_escape(event.message.as_deref().unwrap_or("")),
        );
        body = body.replace(
            "{{indexer}}",
            &json_escape(event.indexer.as_deref().unwrap_or("")),
        );
        body = body.replace(
            "{{size}}",
            &json_escape(event.size.as_deref().unwrap_or("")),
        );
        if let Some(year) = event.year {
            body = body.replace("{{year}}", &year.to_string());
        }
        if let Some(season) = event.season {
            body = body.replace("{{season}}", &season.to_string());
        }
        if let Some(episode) = event.episode {
            body = body.replace("{{episode}}", &episode.to_string());
        }
        if let Some(movie_id) = event.movie_id {
            body = body.replace("{{movie_id}}", &movie_id.to_string());
        }
        if let Some(episode_id) = event.episode_id {
            body = body.replace("{{episode_id}}", &episode_id.to_string());
        }
        body
    } else {
        // Default: send event as JSON
        serde_json::to_string(event).unwrap_or_default()
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn json_escape_quotes_and_backslashes() {
        assert_eq!(json_escape("The \"Movie\""), "The \\\"Movie\\\"");
        assert_eq!(json_escape("C:\\path"), "C:\\\\path");
        assert_eq!(json_escape("plain"), "plain");
    }

    #[test]
    fn render_body_escapes_title_for_valid_json() {
        let target = WebhookTarget {
            id: 1,
            name: "discord".into(),
            url: "https://discord.example".into(),
            method: "POST".into(),
            headers: None,
            body_template: Some(r#"{"content": "{{title}}"}"#.into()),
            on_grab: true,
            on_download_complete: true,
            on_import: true,
            on_upgrade: true,
            on_failure: true,
            on_watched: true,
            on_health_issue: true,
            enabled: true,
            initial_failure_time: None,
            most_recent_failure_time: None,
            escalation_level: 0,
            disabled_until: None,
        };
        let event = Event {
            event_type: "imported".into(),
            movie_id: None,
            episode_id: None,
            title: Some(r#"The "Movie""#.into()),
            show: None,
            season: None,
            episode: None,
            quality: None,
            year: None,
            size: None,
            indexer: None,
            message: None,
        };
        let body = render_body(&target, &event);
        // The rendered body must parse as valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("invalid JSON: {e}\nbody: {body}"));
        assert_eq!(parsed["content"].as_str().unwrap(), r#"The "Movie""#);
    }
}

async fn send_webhook(
    http: &Client,
    target: &WebhookTarget,
    body: &str,
) -> anyhow::Result<SendOutcome> {
    let method = match target.method.to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        _ => reqwest::Method::POST,
    };

    let mut request = http
        .request(method.clone(), &target.url)
        .body(body.to_owned());

    // Add custom headers
    if let Some(ref headers_json) = target.headers
        && let Ok(headers) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(headers_json)
    {
        for (key, value) in &headers {
            if let Some(val) = value.as_str() {
                request = request.header(key.as_str(), val);
            }
        }
    }

    // Default Content-Type if not set
    request = request.header("Content-Type", "application/json");

    let start = std::time::Instant::now();
    tracing::debug!(
        webhook_id = target.id,
        method = %method,
        url = %target.url,
        body_bytes = body.len(),
        "webhook dispatch",
    );
    let resp = request.send().await?;
    let status = resp.status();
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    if !status.is_success() {
        tracing::warn!(
            webhook_id = target.id,
            status = status.as_u16(),
            duration_ms,
            "webhook non-2xx",
        );
        anyhow::bail!("webhook returned {status}");
    }
    tracing::info!(
        webhook_id = target.id,
        status = status.as_u16(),
        duration_ms,
        "webhook delivered",
    );
    Ok(SendOutcome {
        status_code: status.as_u16(),
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_replaces_placeholders() {
        let target = WebhookTarget {
            id: 1,
            name: "test".into(),
            url: "https://example.com".into(),
            method: "POST".into(),
            headers: None,
            body_template: Some(r#"{"text": "{{event}}: {{title}} - {{quality}}"}"#.into()),
            on_grab: true,
            on_download_complete: true,
            on_import: true,
            on_upgrade: true,
            on_failure: true,
            on_watched: false,
            on_health_issue: true,
            enabled: true,
            initial_failure_time: None,
            most_recent_failure_time: None,
            escalation_level: 0,
            disabled_until: None,
        };

        let mut event = Event::simple("imported", "The Matrix");
        event.quality = Some("Bluray-1080p".into());

        let body = render_body(&target, &event);
        assert_eq!(body, r#"{"text": "imported: The Matrix - Bluray-1080p"}"#);
    }

    #[test]
    fn should_fire_checks_event_flags() {
        let mut target = WebhookTarget {
            id: 1,
            name: "t".into(),
            url: "u".into(),
            method: "POST".into(),
            headers: None,
            body_template: None,
            on_grab: true,
            on_download_complete: false,
            on_import: true,
            on_upgrade: false,
            on_failure: true,
            on_watched: false,
            on_health_issue: true,
            enabled: true,
            initial_failure_time: None,
            most_recent_failure_time: None,
            escalation_level: 0,
            disabled_until: None,
        };

        assert!(should_fire(&target, "release_grabbed"));
        assert!(!should_fire(&target, "download_complete"));
        assert!(should_fire(&target, "imported"));
        assert!(!should_fire(&target, "watched"));
        assert!(should_fire(&target, "health_warning"));

        target.on_grab = false;
        assert!(!should_fire(&target, "release_grabbed"));
    }
}

// ─── HTTP handlers ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWebhook {
    pub name: String,
    pub url: String,
    pub method: Option<String>,
    pub headers: Option<String>,
    pub body_template: Option<String>,
    pub on_grab: Option<bool>,
    pub on_download_complete: Option<bool>,
    pub on_import: Option<bool>,
    pub on_upgrade: Option<bool>,
    pub on_failure: Option<bool>,
    pub on_watched: Option<bool>,
    pub on_health_issue: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateWebhook {
    pub name: Option<String>,
    pub url: Option<String>,
    pub method: Option<String>,
    pub headers: Option<String>,
    pub body_template: Option<String>,
    pub on_grab: Option<bool>,
    pub on_download_complete: Option<bool>,
    pub on_import: Option<bool>,
    pub on_upgrade: Option<bool>,
    pub on_failure: Option<bool>,
    pub on_watched: Option<bool>,
    pub on_health_issue: Option<bool>,
    pub enabled: Option<bool>,
}

/// List webhook targets.
#[utoipa::path(
    get, path = "/api/v1/webhooks",
    responses((status = 200, body = Vec<WebhookTarget>)),
    tag = "webhooks", security(("api_key" = []))
)]
pub async fn list_webhooks(State(state): State<AppState>) -> AppResult<Json<Vec<WebhookTarget>>> {
    let webhooks = sqlx::query_as::<_, WebhookTarget>("SELECT * FROM webhook_target ORDER BY id")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(webhooks))
}

/// Create a webhook target.
#[utoipa::path(
    post, path = "/api/v1/webhooks",
    request_body = CreateWebhook,
    responses((status = 201, body = WebhookTarget)),
    tag = "webhooks", security(("api_key" = []))
)]
pub async fn create_webhook(
    State(state): State<AppState>,
    Json(input): Json<CreateWebhook>,
) -> AppResult<(StatusCode, Json<WebhookTarget>)> {
    let method = input.method.as_deref().unwrap_or("POST");
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO webhook_target (name, url, method, headers, body_template, on_grab, on_download_complete, on_import, on_upgrade, on_failure, on_watched, on_health_issue) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&input.name)
    .bind(&input.url)
    .bind(method)
    .bind(&input.headers)
    .bind(&input.body_template)
    .bind(input.on_grab.unwrap_or(true))
    .bind(input.on_download_complete.unwrap_or(true))
    .bind(input.on_import.unwrap_or(true))
    .bind(input.on_upgrade.unwrap_or(true))
    .bind(input.on_failure.unwrap_or(true))
    .bind(input.on_watched.unwrap_or(false))
    .bind(input.on_health_issue.unwrap_or(true))
    .fetch_one(&state.db)
    .await?;

    let webhook = sqlx::query_as::<_, WebhookTarget>("SELECT * FROM webhook_target WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    let _ = state.event_tx.send(AppEvent::WebhookChanged {
        webhook_id: id,
        action: IndexerAction::Created,
    });

    Ok((StatusCode::CREATED, Json(webhook)))
}

/// Update a webhook target.
#[utoipa::path(
    put, path = "/api/v1/webhooks/{id}",
    params(("id" = i64, Path)),
    request_body = UpdateWebhook,
    responses((status = 200, body = WebhookTarget), (status = 404)),
    tag = "webhooks", security(("api_key" = []))
)]
pub async fn update_webhook(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(update): Json<UpdateWebhook>,
) -> AppResult<Json<WebhookTarget>> {
    let result = sqlx::query(
        r"UPDATE webhook_target SET
            name = COALESCE(?, name), url = COALESCE(?, url), method = COALESCE(?, method),
            headers = COALESCE(?, headers), body_template = COALESCE(?, body_template),
            on_grab = COALESCE(?, on_grab), on_download_complete = COALESCE(?, on_download_complete),
            on_import = COALESCE(?, on_import), on_upgrade = COALESCE(?, on_upgrade),
            on_failure = COALESCE(?, on_failure), on_watched = COALESCE(?, on_watched),
            on_health_issue = COALESCE(?, on_health_issue), enabled = COALESCE(?, enabled)
        WHERE id = ?",
    )
    .bind(update.name.as_deref())
    .bind(update.url.as_deref())
    .bind(update.method.as_deref())
    .bind(update.headers.as_deref())
    .bind(update.body_template.as_deref())
    .bind(update.on_grab)
    .bind(update.on_download_complete)
    .bind(update.on_import)
    .bind(update.on_upgrade)
    .bind(update.on_failure)
    .bind(update.on_watched)
    .bind(update.on_health_issue)
    .bind(update.enabled)
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("webhook {id} not found")));
    }

    let webhook = sqlx::query_as::<_, WebhookTarget>("SELECT * FROM webhook_target WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    let _ = state.event_tx.send(AppEvent::WebhookChanged {
        webhook_id: id,
        action: IndexerAction::Updated,
    });

    Ok(Json(webhook))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestWebhookResult {
    pub ok: bool,
    /// Human-readable summary — either "Delivered (HTTP 200 in 123 ms)"
    /// or the reqwest / HTTP error. Safe to drop straight into the UI.
    pub message: String,
    /// HTTP status returned by the target when the request went
    /// through. Absent when the request never completed (DNS, TLS,
    /// timeout). Helps the user tell a 204-on-success from a 401
    /// webhook-URL-typo case at a glance.
    pub status_code: Option<u16>,
    pub duration_ms: i64,
}

/// Send a synthetic "imported" event to a single target, bypassing
/// the enabled + per-event toggles + backoff checks. Lets users
/// verify wiring (URL, auth headers, body template) without having
/// to trigger a real grab.
#[utoipa::path(
    post, path = "/api/v1/webhooks/{id}/test",
    params(("id" = i64, Path)),
    responses((status = 200, body = TestWebhookResult), (status = 404)),
    tag = "webhooks", security(("api_key" = []))
)]
pub async fn test_webhook(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<TestWebhookResult>> {
    let target = sqlx::query_as::<_, WebhookTarget>("SELECT * FROM webhook_target WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("webhook {id} not found")))?;

    let mut event = Event::simple("imported", "kino test — The Matrix");
    event.quality = Some("Bluray-1080p".into());
    event.year = Some(1999);
    event.indexer = Some("kino".into());
    event.message = Some("This is a test notification from kino.".into());

    let start = std::time::Instant::now();
    match send_once(&target, &event).await {
        Ok(out) => Ok(Json(TestWebhookResult {
            ok: true,
            message: format!(
                "Delivered (HTTP {} in {} ms)",
                out.status_code, out.duration_ms
            ),
            status_code: Some(out.status_code),
            duration_ms: i64::try_from(out.duration_ms).unwrap_or(i64::MAX),
        })),
        Err(e) => {
            let duration_ms = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);
            Ok(Json(TestWebhookResult {
                ok: false,
                message: format!("{e}"),
                status_code: None,
                duration_ms,
            }))
        }
    }
}

/// Delete a webhook target.
#[utoipa::path(
    delete, path = "/api/v1/webhooks/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "webhooks", security(("api_key" = []))
)]
pub async fn delete_webhook(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let result = sqlx::query("DELETE FROM webhook_target WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("webhook {id} not found")));
    }

    let _ = state.event_tx.send(AppEvent::WebhookChanged {
        webhook_id: id,
        action: IndexerAction::Deleted,
    });

    Ok(StatusCode::NO_CONTENT)
}
