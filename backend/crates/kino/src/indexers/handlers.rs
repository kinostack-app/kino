use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::events::{AppEvent, IndexerAction};
use crate::indexers::model::{CreateIndexer, Indexer, UpdateIndexer};
use crate::state::AppState;

/// Validate an indexer URL at the edge. We fetch these server-side,
/// so non-http(s) schemes (`file://`, `gopher://`, empty) have no
/// business being accepted — they'd either fail downstream with a
/// confusing error or, worse, point at local resources. Reject early
/// with a clear message so the UI can surface it on the form.
///
/// Cheap prefix check rather than pulling in a URL parser; Torznab
/// URLs are always one of these two schemes.
fn validate_indexer_url(url: &str) -> Result<(), AppError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("indexer url cannot be empty".into()));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        // Cheap sanity: there must be something after the scheme.
        let after_scheme = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .unwrap_or("");
        if after_scheme.is_empty() {
            return Err(AppError::BadRequest("indexer url is missing a host".into()));
        }
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "indexer url must start with http:// or https://; got {trimmed:?}"
        )))
    }
}

/// List all indexers.
#[utoipa::path(
    get, path = "/api/v1/indexers",
    responses((status = 200, body = Vec<Indexer>)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn list_indexers(State(state): State<AppState>) -> AppResult<Json<Vec<Indexer>>> {
    let indexers = sqlx::query_as::<_, Indexer>("SELECT * FROM indexer ORDER BY priority, id")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(indexers))
}

/// Get an indexer by ID.
#[utoipa::path(
    get, path = "/api/v1/indexers/{id}",
    params(("id" = i64, Path)),
    responses((status = 200, body = Indexer), (status = 404)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn get_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Indexer>> {
    let indexer = sqlx::query_as::<_, Indexer>("SELECT * FROM indexer WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("indexer {id} not found")))?;
    Ok(Json(indexer))
}

/// Create an indexer.
#[utoipa::path(
    post, path = "/api/v1/indexers",
    request_body = CreateIndexer,
    responses((status = 201, body = Indexer)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn create_indexer(
    State(state): State<AppState>,
    Json(input): Json<CreateIndexer>,
) -> AppResult<(StatusCode, Json<Indexer>)> {
    validate_indexer_url(&input.url)?;
    let priority = input.priority.unwrap_or(25);
    let enabled = input.enabled.unwrap_or(true);
    let indexer_type = input.indexer_type.as_deref().unwrap_or("torznab");

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO indexer (name, url, api_key, priority, enabled, indexer_type, definition_id, settings_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&input.name)
    .bind(&input.url)
    .bind(&input.api_key)
    .bind(priority)
    .bind(enabled)
    .bind(indexer_type)
    .bind(&input.definition_id)
    .bind(&input.settings_json)
    .fetch_one(&state.db)
    .await?;

    let indexer = sqlx::query_as::<_, Indexer>("SELECT * FROM indexer WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    let _ = state.event_tx.send(AppEvent::IndexerChanged {
        indexer_id: id,
        action: IndexerAction::Created,
    });

    // Kick a wanted-search sweep so freshly-added indexers
    // immediately retry anything waiting for one. The scheduler's
    // sweep skips when no indexers are enabled, so without this
    // trigger the first real sweep would have to wait out the
    // normal search interval.
    if enabled {
        let _ = state
            .trigger_tx
            .try_send(crate::scheduler::TaskTrigger::fire("wanted_search"));
    }

    Ok((StatusCode::CREATED, Json(indexer)))
}

/// Update an indexer.
#[utoipa::path(
    put, path = "/api/v1/indexers/{id}",
    params(("id" = i64, Path)),
    request_body = UpdateIndexer,
    responses((status = 200, body = Indexer), (status = 404)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn update_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(update): Json<UpdateIndexer>,
) -> AppResult<Json<Indexer>> {
    if let Some(url) = update.url.as_deref() {
        validate_indexer_url(url)?;
    }
    let result = sqlx::query(
        r"UPDATE indexer SET
            name          = COALESCE(?, name),
            url           = COALESCE(?, url),
            api_key       = COALESCE(?, api_key),
            priority      = COALESCE(?, priority),
            enabled       = COALESCE(?, enabled),
            indexer_type  = COALESCE(?, indexer_type),
            definition_id = COALESCE(?, definition_id),
            settings_json = COALESCE(?, settings_json)
        WHERE id = ?",
    )
    .bind(update.name.as_deref())
    .bind(update.url.as_deref())
    .bind(update.api_key.as_deref())
    .bind(update.priority)
    .bind(update.enabled)
    .bind(update.indexer_type.as_deref())
    .bind(update.definition_id.as_deref())
    .bind(update.settings_json.as_deref())
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("indexer {id} not found")));
    }

    let indexer = sqlx::query_as::<_, Indexer>("SELECT * FROM indexer WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    let _ = state.event_tx.send(AppEvent::IndexerChanged {
        indexer_id: id,
        action: IndexerAction::Updated,
    });

    // An edit that (re-)enables an indexer should trigger the sweep —
    // e.g. the user fixed a broken URL or flipped `enabled` back on.
    // `update.enabled == Some(true)` is the reliable signal; any other
    // metadata change is a no-op from the wanted-sweep's POV.
    if update.enabled == Some(true) {
        let _ = state
            .trigger_tx
            .try_send(crate::scheduler::TaskTrigger::fire("wanted_search"));
    }

    Ok(Json(indexer))
}

/// Delete an indexer.
#[utoipa::path(
    delete, path = "/api/v1/indexers/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn delete_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let result = sqlx::query("DELETE FROM indexer WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("indexer {id} not found")));
    }

    let _ = state.event_tx.send(AppEvent::IndexerChanged {
        indexer_id: id,
        action: IndexerAction::Deleted,
    });

    Ok(StatusCode::NO_CONTENT)
}

// ── Indexer definition endpoints ──────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DefinitionQuery {
    pub search: Option<String>,
    #[serde(rename = "type")]
    pub indexer_type: Option<String>,
    pub language: Option<String>,
    /// Filter to indexers that declare this top-level category
    /// (Movies / TV / Audio / Books / Anime / XXX / Other). Matches any.
    pub category: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DefinitionDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    pub indexer_type: String,
    pub language: String,
    pub links: Vec<String>,
    pub settings: Vec<DefinitionSettingField>,
}

/// Settings field — mirrors the YAML definition format exactly.
#[derive(Debug, Serialize, ToSchema)]
pub struct DefinitionSettingField {
    pub name: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub field_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<std::collections::HashMap<String, String>>,
}

/// List available indexer definitions.
#[utoipa::path(
    get, path = "/api/v1/indexer-definitions",
    params(
        ("search" = Option<String>, Query),
        ("type" = Option<String>, Query),
        ("language" = Option<String>, Query),
        ("category" = Option<String>, Query),
    ),
    responses((status = 200, body = Vec<crate::indexers::loader::DefinitionSummary>)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn list_definitions(
    State(state): State<AppState>,
    Query(query): Query<DefinitionQuery>,
) -> AppResult<Json<Vec<crate::indexers::loader::DefinitionSummary>>> {
    let definitions = state.require_definitions()?;
    let mut list = definitions.list();

    // Filter by search term (case-insensitive match on name or description)
    if let Some(ref search) = query.search {
        let search_lower = search.to_lowercase();
        list.retain(|d| {
            d.name.to_lowercase().contains(&search_lower)
                || d.description.to_lowercase().contains(&search_lower)
        });
    }

    // Filter by indexer type (public, private, semi-private). The
    // typed enum serialises to a lowercase tag, so a case-insensitive
    // match on the query param against each variant's wire form is
    // enough — no need for a parallel string store on the summary.
    if let Some(ref indexer_type) = query.indexer_type {
        let type_lower = indexer_type.to_ascii_lowercase();
        list.retain(|d| {
            let variant_tag = match d.indexer_type {
                crate::indexers::loader::IndexerDefinitionType::Public => "public",
                crate::indexers::loader::IndexerDefinitionType::SemiPrivate => "semi-private",
                crate::indexers::loader::IndexerDefinitionType::Private => "private",
            };
            variant_tag == type_lower
        });
    }

    // Filter by language
    if let Some(ref language) = query.language {
        let lang_lower = language.to_lowercase();
        list.retain(|d| d.language.to_lowercase().starts_with(&lang_lower));
    }

    // Filter by top-level category (Movies / TV / Audio / ...).
    if let Some(ref category) = query.category {
        let cat_lower = category.to_lowercase();
        list.retain(|d| d.categories.iter().any(|c| c.to_lowercase() == cat_lower));
    }

    Ok(Json(list))
}

/// Get full details for an indexer definition.
#[utoipa::path(
    get, path = "/api/v1/indexer-definitions/{id}",
    params(("id" = String, Path)),
    responses((status = 200, body = DefinitionDetail), (status = 404)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn get_definition(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<DefinitionDetail>> {
    let definitions = state.require_definitions()?;
    let def = definitions
        .get(&id)
        .ok_or_else(|| AppError::NotFound(format!("definition '{id}' not found")))?;

    let detail = DefinitionDetail {
        id: def.id.clone(),
        name: def.name.clone(),
        description: def.description.clone().unwrap_or_default(),
        indexer_type: def.indexer_type.clone().unwrap_or_else(|| "public".into()),
        language: def.language.clone().unwrap_or_else(|| "en-US".into()),
        links: def.links.clone(),
        settings: def
            .settings
            .iter()
            .map(|s| DefinitionSettingField {
                name: s.name.clone(),
                field_type: s.field_type.clone(),
                label: s.label.clone(),
                default: s.default.clone(),
                options: s.options.clone(),
            })
            .collect(),
    };

    Ok(Json(detail))
}

// ── Indexer-definitions refresh ───────────────────────────────────

/// Kick off a Cardigann definitions refresh from the Prowlarr/Indexers
/// GitHub repo. Returns immediately with `202 Accepted`; progress
/// flows via `IndexerDefinitionsRefresh*` WS events + the GET state
/// snapshot below. A second call while one is running returns `409
/// Conflict`. The setup wizard's Library Sources step + Settings →
/// Indexers both use this endpoint.
#[utoipa::path(
    post, path = "/api/v1/indexer-definitions/refresh",
    responses(
        (status = 202, body = crate::indexers::refresh::DefinitionsRefreshState),
        (status = 409, description = "a refresh is already running"),
    ),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn refresh_definitions(
    State(state): State<AppState>,
) -> AppResult<(
    StatusCode,
    Json<crate::indexers::refresh::DefinitionsRefreshState>,
)> {
    use crate::indexers::refresh::{RefreshError, start_refresh};

    match start_refresh(
        state.definitions_refresh.clone(),
        state.definitions.clone(),
        state.event_tx.clone(),
        state.db.clone(),
    )
    .await
    {
        Ok(()) => Ok((
            StatusCode::ACCEPTED,
            Json(state.definitions_refresh.snapshot().await),
        )),
        Err(RefreshError::AlreadyRunning) => Err(AppError::Conflict(
            "a definitions refresh is already running".into(),
        )),
        Err(RefreshError::LoaderUnavailable) => Err(AppError::Internal(anyhow::anyhow!(
            "indexer definitions loader is not configured"
        ))),
    }
}

/// Snapshot of the current refresh state. Authoritative for
/// late-joining clients (page reload mid-refresh).
#[utoipa::path(
    get, path = "/api/v1/indexer-definitions/refresh",
    responses((status = 200, body = crate::indexers::refresh::DefinitionsRefreshState)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn get_refresh_state(
    State(state): State<AppState>,
) -> Json<crate::indexers::refresh::DefinitionsRefreshState> {
    Json(state.definitions_refresh.snapshot().await)
}

// ── Test indexer endpoint ─────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct TestIndexerResult {
    pub success: bool,
    pub message: String,
    pub result_count: i64,
}

/// Reset an indexer's escalation backoff — clears `escalation_level`,
/// `initial_failure_time`, `most_recent_failure_time` and `disabled_until`
/// so the next scheduled health sweep (or search) treats it as healthy.
/// Useful when an indexer has been disabled due to transient errors and
/// the user has fixed the root cause.
#[utoipa::path(
    post, path = "/api/v1/indexers/{id}/retry",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn retry_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let result = sqlx::query(
        "UPDATE indexer SET
             escalation_level = 0,
             initial_failure_time = NULL,
             most_recent_failure_time = NULL,
             disabled_until = NULL
         WHERE id = ?",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("indexer {id} not found")));
    }

    let _ = state.event_tx.send(AppEvent::IndexerChanged {
        indexer_id: id,
        action: IndexerAction::HealthChanged,
    });
    tracing::info!(indexer_id = id, "indexer escalation reset");
    Ok(StatusCode::NO_CONTENT)
}

/// Test an indexer by running a search for "test".
#[utoipa::path(
    post, path = "/api/v1/indexers/{id}/test",
    params(("id" = i64, Path)),
    responses((status = 200, body = TestIndexerResult), (status = 404)),
    tag = "indexers", security(("api_key" = []))
)]
pub async fn test_indexer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<TestIndexerResult>> {
    let indexer = sqlx::query_as::<_, Indexer>("SELECT * FROM indexer WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("indexer {id} not found")))?;

    match indexer.indexer_type.as_str() {
        "cardigann" => test_cardigann_indexer(&state, &indexer).await,
        // Default: torznab
        _ => test_torznab_indexer(&indexer).await,
    }
}

async fn test_torznab_indexer(indexer: &Indexer) -> AppResult<Json<TestIndexerResult>> {
    use crate::torznab::client::{TorznabClient, TorznabQuery};

    let client = TorznabClient::new();
    let query = TorznabQuery {
        q: Some("test".to_string()),
        ..Default::default()
    };

    match client
        .search(&indexer.url, indexer.api_key.as_deref(), &query)
        .await
    {
        Ok(results) => {
            let count = i64::try_from(results.len()).unwrap_or(i64::MAX);
            Ok(Json(TestIndexerResult {
                success: true,
                message: format!("Search returned {count} results"),
                result_count: count,
            }))
        }
        Err(e) => Ok(Json(TestIndexerResult {
            success: false,
            message: format!("Search failed: {e}"),
            result_count: 0,
        })),
    }
}

async fn test_cardigann_indexer(
    state: &AppState,
    indexer: &Indexer,
) -> AppResult<Json<TestIndexerResult>> {
    use crate::indexers::request::IndexerClient;
    use crate::indexers::template::SearchQuery;

    let definitions = state.require_definitions()?;
    let definition_id = indexer
        .definition_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Cardigann indexer has no definition_id".into()))?;
    let definition = definitions
        .get(definition_id)
        .ok_or_else(|| AppError::NotFound(format!("definition '{definition_id}' not found")))?;

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
        Ok(results) => {
            let count = i64::try_from(results.len()).unwrap_or(i64::MAX);
            Ok(Json(TestIndexerResult {
                success: true,
                message: format!("Search returned {count} results"),
                result_count: count,
            }))
        }
        Err(e) => Ok(Json(TestIndexerResult {
            success: false,
            message: format!("Search failed: {e}"),
            result_count: 0,
        })),
    }
}
