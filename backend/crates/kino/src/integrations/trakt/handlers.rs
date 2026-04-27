//! HTTP handlers for the Trakt integration. Thin wrapper over
//! [`crate::integrations::trakt`]; the real logic lives there.
// Inline `#[derive(sqlx::FromRow)]` structs sit under `let` bindings
// in a few handlers for readability. Hoisting them to module level
// would mean three near-identical `FooRow` types scattered around
// that only exist so one handler can return a shaped struct.
#![allow(clippy::items_after_statements)]

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::integrations::trakt;
use crate::integrations::trakt::TraktError;
use crate::state::AppState;

/// Response from `GET /api/v1/integrations/trakt/status`. The UI uses
/// this to render the connect/connected/expired states.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TraktStatus {
    /// True once the user has entered `client_id` + `client_secret`.
    pub configured: bool,
    /// True once a token is stored (post device-code flow).
    pub connected: bool,
    pub username: Option<String>,
    pub slug: Option<String>,
    pub connected_at: Option<String>,
    pub token_expires_at: Option<String>,
    /// Initial bulk import has completed — the UI shows "Import
    /// data" CTA only when `connected && !initial_import_done`.
    pub initial_import_done: bool,
    pub last_full_sync_at: Option<String>,
    pub last_incremental_sync_at: Option<String>,
    /// Config toggles so the Integrations page can render switches
    /// without a second round-trip.
    pub scrobble: bool,
    pub sync_watched: bool,
    pub sync_ratings: bool,
    pub sync_watchlist: bool,
    pub sync_collection: bool,
}

#[utoipa::path(
    get, path = "/api/v1/integrations/trakt/status",
    responses((status = 200, body = TraktStatus)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn status(State(state): State<AppState>) -> AppResult<Json<TraktStatus>> {
    let configured = trakt::load_app_credentials(&state.db).await.is_some();
    #[derive(sqlx::FromRow)]
    struct AuthRow {
        username: Option<String>,
        slug: Option<String>,
        connected_at: String,
        expires_at: String,
    }
    let auth: Option<AuthRow> = sqlx::query_as(
        "SELECT username, slug, connected_at, expires_at FROM trakt_auth WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?;
    #[derive(sqlx::FromRow, Default)]
    struct StateRow {
        initial_import_done: bool,
        last_full_sync_at: Option<String>,
        last_incremental_sync_at: Option<String>,
    }
    let sync_state: StateRow = sqlx::query_as(
        "SELECT initial_import_done, last_full_sync_at, last_incremental_sync_at
         FROM trakt_sync_state WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_default();
    #[derive(sqlx::FromRow, Default)]
    struct CfgRow {
        trakt_scrobble: bool,
        trakt_sync_watched: bool,
        trakt_sync_ratings: bool,
        trakt_sync_watchlist: bool,
        trakt_sync_collection: bool,
    }
    let cfg: CfgRow = sqlx::query_as(
        "SELECT trakt_scrobble, trakt_sync_watched, trakt_sync_ratings,
                trakt_sync_watchlist, trakt_sync_collection
         FROM config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_default();

    Ok(Json(TraktStatus {
        configured,
        connected: auth.is_some(),
        username: auth.as_ref().and_then(|a| a.username.clone()),
        slug: auth.as_ref().and_then(|a| a.slug.clone()),
        connected_at: auth.as_ref().map(|a| a.connected_at.clone()),
        token_expires_at: auth.as_ref().map(|a| a.expires_at.clone()),
        initial_import_done: sync_state.initial_import_done,
        last_full_sync_at: sync_state.last_full_sync_at,
        last_incremental_sync_at: sync_state.last_incremental_sync_at,
        scrobble: cfg.trakt_scrobble,
        sync_watched: cfg.trakt_sync_watched,
        sync_ratings: cfg.trakt_sync_ratings,
        sync_watchlist: cfg.trakt_sync_watchlist,
        sync_collection: cfg.trakt_sync_collection,
    }))
}

/// Kick off a device-code flow. Returns the user-facing verification
/// URL + code; frontend polls [`device_status`] until it resolves.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BeginReply {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval_secs: u64,
    pub expires_in_secs: u64,
}

#[utoipa::path(
    post, path = "/api/v1/integrations/trakt/device-code",
    responses((status = 200, body = BeginReply), (status = 400)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn begin_device(State(state): State<AppState>) -> AppResult<Json<BeginReply>> {
    let client = trakt::client_for(&state).await.map_err(map_err)?;
    let dc = trakt::auth::begin(&client).await.map_err(map_err)?;
    Ok(Json(BeginReply {
        device_code: dc.device_code,
        user_code: dc.user_code,
        verification_url: dc.verification_url,
        interval_secs: dc.interval,
        expires_in_secs: dc.expires_in,
    }))
}

/// Poll for token completion. Frontend calls this on the interval
/// given by [`begin_device`]. Returns one of:
///   - 200 with `{state: "pending"}` → keep polling
///   - 200 with `{state: "connected", username, slug}` → done
///   - 410 gone → user denied / code expired; restart flow
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PollReply {
    Pending,
    Connected { username: String, slug: String },
    Invalid { reason: String },
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PollReq {
    pub device_code: String,
}

#[utoipa::path(
    post, path = "/api/v1/integrations/trakt/device-poll",
    request_body = PollReq,
    responses((status = 200, body = PollReply)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn poll_device(
    State(state): State<AppState>,
    Json(body): Json<PollReq>,
) -> AppResult<Json<PollReply>> {
    let client = trakt::client_for(&state).await.map_err(map_err)?;
    let outcome = trakt::auth::poll_token(&client, &body.device_code)
        .await
        .map_err(map_err)?;
    let reply = match outcome {
        trakt::auth::PollOutcome::Pending => PollReply::Pending,
        trakt::auth::PollOutcome::Connected { username, slug } => {
            state.emit(crate::events::AppEvent::TraktConnected);
            // Idempotent bootstrap of the Trakt-watchlist system list
            // (subsystem 17). Fire-and-forget; a failure never blocks
            // the OAuth response. Emits ListAutoAdded on first create.
            if let Err(e) =
                crate::integrations::lists::sync::ensure_trakt_watchlist(&state.db, &state.event_tx)
                    .await
            {
                tracing::warn!(error = %e, "trakt watchlist system-list bootstrap failed");
            }
            PollReply::Connected { username, slug }
        }
        trakt::auth::PollOutcome::Invalid(reason) => PollReply::Invalid { reason },
    };
    Ok(Json(reply))
}

#[utoipa::path(
    post, path = "/api/v1/integrations/trakt/disconnect",
    responses((status = 204), (status = 400)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn disconnect(State(state): State<AppState>) -> AppResult<StatusCode> {
    // Disconnect is idempotent: if Trakt isn't configured/connected we
    // still return 204 so the UI's "Disconnect" button isn't
    // misleading when hit twice.
    if let Ok(client) = trakt::client_for(&state).await {
        let _ = trakt::auth::disconnect(&client).await;
    }
    // Belt-and-braces: clear the auth row even if the client couldn't
    // be built (e.g. credentials were wiped in a previous failed
    // disconnect attempt).
    let _ = sqlx::query("DELETE FROM trakt_auth WHERE id = 1")
        .execute(&state.db)
        .await;
    let _ = sqlx::query("DELETE FROM trakt_sync_state WHERE id = 1")
        .execute(&state.db)
        .await;
    // Remove the Trakt-watchlist system list (subsystem 17) — it's
    // tied to the connection and can't be reachable without OAuth.
    let _ = crate::integrations::lists::sync::remove_trakt_watchlist(&state.db).await;
    state.emit(crate::events::AppEvent::TraktDisconnected);
    Ok(StatusCode::NO_CONTENT)
}

// ── Sync ──────────────────────────────────────────────────────────

#[utoipa::path(
    get, path = "/api/v1/integrations/trakt/dry-run",
    responses((status = 200, body = trakt::sync::DryRunCounts), (status = 400)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn dry_run(State(state): State<AppState>) -> AppResult<Json<trakt::sync::DryRunCounts>> {
    let client = trakt::client_for(&state).await.map_err(map_err)?;
    let counts = trakt::sync::dry_run(&client).await.map_err(map_err)?;
    Ok(Json(counts))
}

#[utoipa::path(
    post, path = "/api/v1/integrations/trakt/import",
    responses((status = 200, body = trakt::sync::DryRunCounts), (status = 400)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn import(State(state): State<AppState>) -> AppResult<Json<trakt::sync::DryRunCounts>> {
    let client = trakt::client_for(&state).await.map_err(map_err)?;
    let counts = trakt::sync::import_all(&client, trakt::sync::Respect::All)
        .await
        .map_err(map_err)?;
    state.emit(crate::events::AppEvent::TraktSynced {
        kind: "initial_import".into(),
    });
    Ok(Json(counts))
}

#[utoipa::path(
    post, path = "/api/v1/integrations/trakt/sync",
    responses((status = 204), (status = 400)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn sync_now(State(state): State<AppState>) -> AppResult<StatusCode> {
    let client = trakt::client_for(&state).await.map_err(map_err)?;
    trakt::sync::incremental_sweep(&client)
        .await
        .map_err(map_err)?;
    state.emit(crate::events::AppEvent::TraktSynced {
        kind: "incremental".into(),
    });
    Ok(StatusCode::NO_CONTENT)
}

// ── Home rows ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HomeRow {
    pub items: Vec<HomeItem>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HomeItem {
    /// "movie" or "show"
    pub kind: String,
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub year: Option<i64>,
}

#[utoipa::path(
    get, path = "/api/v1/integrations/trakt/recommendations",
    responses((status = 200, body = HomeRow)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn recommendations(State(state): State<AppState>) -> AppResult<Json<HomeRow>> {
    let row = home_row_from_cache(&state.db, "recommendations_json").await;
    Ok(Json(row))
}

#[utoipa::path(
    get, path = "/api/v1/integrations/trakt/trending",
    responses((status = 200, body = HomeRow)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn trending(State(state): State<AppState>) -> AppResult<Json<HomeRow>> {
    let row = home_row_from_cache(&state.db, "trending_json").await;
    Ok(Json(row))
}

async fn home_row_from_cache(db: &sqlx::SqlitePool, column: &str) -> HomeRow {
    let json: String = sqlx::query_scalar(&format!(
        "SELECT {column} FROM trakt_sync_state WHERE id = 1"
    ))
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "{}".to_string());

    #[derive(serde::Deserialize, Default)]
    struct Payload {
        #[serde(default)]
        movies: Vec<serde_json::Value>,
        #[serde(default)]
        shows: Vec<serde_json::Value>,
    }
    let payload: Payload = serde_json::from_str(&json).unwrap_or_default();

    // Trending wraps each entity in `{watchers, movie|show: {...}}`;
    // recommendations return the entity directly. We handle both
    // shapes here by looking for the nested object first.
    fn entity_from(v: &serde_json::Value, kind: &str) -> Option<HomeItem> {
        let root = v.get(kind).unwrap_or(v);
        let title = root.get("title")?.as_str()?.to_string();
        let year = root.get("year").and_then(serde_json::Value::as_i64);
        let tmdb_id = root
            .get("ids")
            .and_then(|ids| ids.get("tmdb"))
            .and_then(serde_json::Value::as_i64);
        Some(HomeItem {
            kind: kind.to_string(),
            tmdb_id,
            title,
            year,
        })
    }

    let mut items: Vec<HomeItem> = payload
        .movies
        .iter()
        .filter_map(|v| entity_from(v, "movie"))
        .chain(payload.shows.iter().filter_map(|v| entity_from(v, "show")))
        .collect();
    // Interleave movies + shows so the row mixes kinds visually.
    if !items.is_empty() {
        let movies: Vec<_> = items
            .iter()
            .filter(|i| i.kind == "movie")
            .cloned()
            .collect();
        let shows: Vec<_> = items.iter().filter(|i| i.kind == "show").cloned().collect();
        let mut mixed: Vec<HomeItem> = Vec::with_capacity(items.len());
        let max = movies.len().max(shows.len());
        for i in 0..max {
            if i < movies.len() {
                mixed.push(movies[i].clone());
            }
            if i < shows.len() {
                mixed.push(shows[i].clone());
            }
        }
        items = mixed;
    }
    HomeRow { items }
}

// ── Rating ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct RateReq {
    /// 1..10 Trakt scale. Set to `null` to unrate.
    pub rating: Option<i64>,
}

#[utoipa::path(
    post, path = "/api/v1/integrations/trakt/rate/{kind}/{id}",
    request_body = RateReq,
    params(("kind" = String, Path), ("id" = i64, Path)),
    responses((status = 204), (status = 400)),
    tag = "integrations", security(("api_key" = []))
)]
pub async fn rate(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
    Json(body): Json<RateReq>,
) -> AppResult<StatusCode> {
    let rating = body.rating;
    // Write locally regardless of Trakt state — users can rate
    // without being connected; we push up when syncing is on.
    let col_table = match kind.as_str() {
        "movie" => ("movie", trakt::sync::RatingKind::Movie),
        "show" => ("show", trakt::sync::RatingKind::Show),
        "episode" => ("episode", trakt::sync::RatingKind::Episode),
        _ => {
            return Err(AppError::BadRequest(
                "kind must be movie|show|episode".into(),
            ));
        }
    };
    sqlx::query(&format!(
        "UPDATE {} SET user_rating = ? WHERE id = ?",
        col_table.0
    ))
    .bind(rating)
    .bind(id)
    .execute(&state.db)
    .await?;

    // Best-effort push to Trakt. `None` means unrate (POST
    // /sync/ratings/remove); a value pushes via /sync/ratings.
    if let Ok(client) = trakt::client_for(&state).await {
        let push_result = match rating {
            Some(r) => trakt::sync::push_rating(&client, col_table.1, id, r).await,
            None => trakt::sync::push_unrate(&client, col_table.1, id).await,
        };
        if let Err(e) = push_result {
            tracing::warn!(error = %e, kind, id, "trakt rating push failed");
        }
    }

    // Rating changes the input to Trakt's recommendation engine.
    // Clear the local cache timestamp so the next /home refresh
    // (daily task or manual sync) re-pulls; without this, a user
    // who just rated a handful of films keeps seeing yesterday's
    // recommendations for up to 24h.
    sqlx::query("UPDATE trakt_sync_state SET recommendations_cached_at = NULL WHERE id = 1")
        .execute(&state.db)
        .await
        .ok();

    state.emit(crate::events::AppEvent::Rated {
        kind,
        id,
        value: rating,
    });

    Ok(StatusCode::NO_CONTENT)
}

// ── Error mapping ─────────────────────────────────────────────────

fn map_err(e: TraktError) -> AppError {
    match e {
        TraktError::NotConfigured => {
            AppError::BadRequest("Trakt not configured — add API credentials in Settings".into())
        }
        TraktError::NotConnected => {
            AppError::BadRequest("Trakt not connected — run the device-code flow first".into())
        }
        TraktError::AuthExpired => {
            AppError::BadRequest("Trakt authorisation expired — reconnect from Settings".into())
        }
        TraktError::Api { status, message } => {
            AppError::Internal(anyhow::anyhow!("trakt api {status}: {message}"))
        }
        TraktError::Transport(e) => AppError::Internal(anyhow::anyhow!("transport: {e}")),
        TraktError::Db(e) => AppError::Internal(anyhow::anyhow!(e)),
        TraktError::Serde(e) => AppError::Internal(anyhow::anyhow!(e)),
        TraktError::Other(m) => AppError::Internal(anyhow::anyhow!(m)),
    }
}
