//! REST surface for subsystem 17 (lists).
//!
//! Two-phase add flow:
//!   1. POST /api/v1/lists with `mode: null` → 200 `{ preview }`
//!      (no DB write). Frontend renders the soft-cap dialog.
//!   2. POST /api/v1/lists with `mode: "add_all"|"top_n"|"none"` →
//!      201 `{ list }` (DB write + items fetched).
//!
//! Failures:
//!   - unsupported URL        → 400 `BadRequest`
//!   - `MDBList` key missing    → 400 `BadRequest` with typed hint
//!   - Trakt not connected    → 400 `BadRequest`
//!   - source unreachable / 5xx → 502 `BadGateway`
//!   - DB errors              → 500 Internal

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::integrations::lists::model::{
    CreateListRequest, List, ListItem, ListItemView, ListPreview, ListView,
};
use crate::integrations::lists::{self, ListsError};
use crate::state::AppState;

/// Response from `POST /api/v1/lists`. Preview-phase returns
/// `{ preview }`; confirm-phase returns `{ list }` with the created
/// `ListView` (including the preview-poster strip).
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateListResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<ListView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<ListPreview>,
}

#[utoipa::path(
    get, path = "/api/v1/lists",
    responses((status = 200, body = Vec<ListView>)),
    tag = "lists", security(("api_key" = []))
)]
pub async fn list_lists(State(state): State<AppState>) -> AppResult<Json<Vec<ListView>>> {
    let rows: Vec<List> =
        sqlx::query_as("SELECT * FROM list ORDER BY is_system DESC, created_at ASC")
            .fetch_all(&state.db)
            .await?;
    // Pull top-4 poster paths per list in a single query, keyed by
    // list_id. Cheap (window-partitioned over list_item), and avoids
    // an N+1 fan-out for the grid card preview strip.
    let posters: Vec<(i64, String)> = sqlx::query_as(
        "WITH ranked AS (
             SELECT list_id, poster_path,
                    ROW_NUMBER() OVER (PARTITION BY list_id ORDER BY position ASC, id ASC) AS rn
             FROM list_item
             WHERE poster_path IS NOT NULL AND poster_path <> ''
         )
         SELECT list_id, poster_path FROM ranked WHERE rn <= 4",
    )
    .fetch_all(&state.db)
    .await?;
    let mut by_list: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    for (lid, p) in posters {
        by_list.entry(lid).or_default().push(p);
    }
    let out = rows
        .into_iter()
        .map(|l| ListView {
            preview_posters: by_list.remove(&l.id).unwrap_or_default(),
            list: l,
        })
        .collect();
    Ok(Json(out))
}

#[utoipa::path(
    post, path = "/api/v1/lists",
    request_body = CreateListRequest,
    responses(
        (status = 200, description = "Preview only — list not created yet", body = CreateListResponse),
        (status = 201, description = "List created", body = CreateListResponse),
        (status = 400, description = "Bad URL / missing key / Trakt disconnected"),
        (status = 502, description = "Source unreachable"),
    ),
    tag = "lists", security(("api_key" = []))
)]
pub async fn create_list(
    State(state): State<AppState>,
    Json(req): Json<CreateListRequest>,
) -> AppResult<(StatusCode, Json<CreateListResponse>)> {
    // Phase 1: preview (confirm=false).
    if !req.confirm {
        let (_parsed, preview) = lists::sync::preview_list(&state.db, &req.url)
            .await
            .map_err(map_lists_err)?;
        return Ok((
            StatusCode::OK,
            Json(CreateListResponse {
                list: None,
                preview: Some(preview),
            }),
        ));
    }

    // Phase 2: create.
    let parsed = lists::parser::parse_list_url(&req.url).map_err(map_lists_err)?;
    let list = lists::sync::create_list(&state.db, &parsed)
        .await
        .map_err(map_lists_err)?;

    // Kick the scheduler so the list's items show up with live
    // library status once the first add succeeds (no-op if no
    // poll task needs to run yet; just a wake-up).
    let _ = state
        .trigger_tx
        .try_send(crate::scheduler::TaskTrigger::fire("lists_poll"));

    // Build the ListView response (posters are in the DB by now —
    // apply_poll inserted the items synchronously during create_list).
    let preview_posters: Vec<String> = sqlx::query_scalar(
        "SELECT poster_path FROM list_item
         WHERE list_id = ? AND poster_path IS NOT NULL AND poster_path <> ''
         ORDER BY position ASC, id ASC LIMIT 4",
    )
    .bind(list.id)
    .fetch_all(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreateListResponse {
            list: Some(ListView {
                list,
                preview_posters,
            }),
            preview: None,
        }),
    ))
}

#[utoipa::path(
    get, path = "/api/v1/lists/{id}",
    params(("id" = i64, Path)),
    responses((status = 200, body = ListView), (status = 404)),
    tag = "lists", security(("api_key" = []))
)]
pub async fn get_list(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ListView>> {
    let list: List = sqlx::query_as("SELECT * FROM list WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("list {id} not found")))?;
    let preview_posters: Vec<String> = sqlx::query_scalar(
        "SELECT poster_path FROM list_item
         WHERE list_id = ? AND poster_path IS NOT NULL AND poster_path <> ''
         ORDER BY position ASC, id ASC LIMIT 4",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(ListView {
        list,
        preview_posters,
    }))
}

#[utoipa::path(
    delete, path = "/api/v1/lists/{id}",
    params(("id" = i64, Path)),
    responses((status = 204), (status = 404), (status = 409, description = "System list can't be unfollowed")),
    tag = "lists", security(("api_key" = []))
)]
pub async fn delete_list(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let row: Option<bool> = sqlx::query_scalar("SELECT is_system FROM list WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let Some(is_system) = row else {
        return Err(AppError::NotFound(format!("list {id} not found")));
    };
    if is_system {
        return Err(AppError::Conflict(
            "system list can't be unfollowed — disconnect Trakt to remove it".into(),
        ));
    }
    let title: String = sqlx::query_scalar("SELECT title FROM list WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    sqlx::query("DELETE FROM list WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    // Scrub the pseudo-row from every user's home preferences. With
    // single-user kino this is the one row, but the pattern matches
    // the principle: `list:<id>` IDs in section_order / hidden_rows
    // are stale once the list is gone.
    scrub_list_row_from_prefs(&state.db, id).await;
    state.emit(crate::events::AppEvent::ListDeleted { list_id: id, title });
    Ok(StatusCode::NO_CONTENT)
}

/// Remove `list:<id>` from `section_order` and `hidden_rows` in the
/// stored home preferences JSON. `SQLite` JSON1 array filtering is
/// awkward, so we read/rewrite — cheap with single-row prefs.
async fn scrub_list_row_from_prefs(db: &sqlx::SqlitePool, list_id: i64) {
    let marker = format!("list:{list_id}");
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT home_section_order, home_section_hidden FROM user_preferences WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let Some((so, hr)) = row else {
        return;
    };
    let strip = |raw: &str| -> Option<String> {
        let mut v: Vec<String> = serde_json::from_str(raw).ok()?;
        let before = v.len();
        v.retain(|x| x != &marker);
        if v.len() == before {
            return None;
        }
        serde_json::to_string(&v).ok()
    };
    let new_so = strip(&so);
    let new_hr = strip(&hr);
    if new_so.is_none() && new_hr.is_none() {
        return;
    }
    let _ = sqlx::query(
        "UPDATE user_preferences SET
            home_section_order  = COALESCE(?, home_section_order),
            home_section_hidden = COALESCE(?, home_section_hidden)
         WHERE id = 1",
    )
    .bind(new_so)
    .bind(new_hr)
    .execute(db)
    .await;
}

#[utoipa::path(
    post, path = "/api/v1/lists/{id}/refresh",
    params(("id" = i64, Path)),
    responses((status = 200, body = List), (status = 404), (status = 502)),
    tag = "lists", security(("api_key" = []))
)]
pub async fn refresh_list(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<List>> {
    let list: List = sqlx::query_as("SELECT * FROM list WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("list {id} not found")))?;
    let Some(source_type) = lists::SourceType::parse(&list.source_type) else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "unknown source_type: {}",
            list.source_type
        )));
    };
    let parsed = lists::ParsedList {
        source_type,
        source_id: list.source_id.clone(),
        source_url: list.source_url.clone(),
    };
    let items = lists::fetch_items(&state.db, &parsed)
        .await
        .map_err(map_lists_err)?;
    let outcome = lists::sync::apply_poll(&state.db, list.id, items)
        .await
        .map_err(map_lists_err)?;

    // Match the scheduler's bulk-growth behaviour for manual refreshes
    // — curator adds 50 items between polls, user hits Refresh, sees a
    // "List 'X' added 50 items" notification rather than silent magic.
    let threshold: i64 =
        sqlx::query_scalar("SELECT list_bulk_growth_threshold FROM config WHERE id = 1")
            .fetch_optional(&state.db)
            .await?
            .unwrap_or(20);
    if outcome.added > threshold {
        state.emit(crate::events::AppEvent::ListBulkGrowth {
            list_id: list.id,
            title: list.title.clone(),
            added: outcome.added,
        });
    }
    let refreshed: List = sqlx::query_as("SELECT * FROM list WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(refreshed))
}

#[utoipa::path(
    get, path = "/api/v1/lists/{id}/items",
    params(("id" = i64, Path)),
    responses((status = 200, body = Vec<ListItemView>), (status = 404)),
    tag = "lists", security(("api_key" = []))
)]
pub async fn list_items(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<ListItemView>>> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM list WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound(format!("list {id} not found")));
    }
    let rows: Vec<ListItem> =
        sqlx::query_as("SELECT * FROM list_item WHERE list_id = ? ORDER BY position ASC, id ASC")
            .bind(id)
            .fetch_all(&state.db)
            .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let state_str = derive_state(&state.db, r.tmdb_id, &r.item_type, r.ignored_by_user).await?;
        out.push(ListItemView {
            id: r.id,
            list_id: r.list_id,
            tmdb_id: r.tmdb_id,
            item_type: r.item_type,
            title: r.title,
            poster_path: r.poster_path,
            position: r.position,
            added_at: r.added_at,
            ignored_by_user: r.ignored_by_user,
            state: state_str,
        });
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct IgnoreItemRequest {
    pub ignored: bool,
}

#[utoipa::path(
    post, path = "/api/v1/lists/{id}/items/{item_id}/ignore",
    params(("id" = i64, Path), ("item_id" = i64, Path)),
    request_body = IgnoreItemRequest,
    responses((status = 204), (status = 404)),
    tag = "lists", security(("api_key" = []))
)]
pub async fn ignore_item(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(i64, i64)>,
    Json(req): Json<IgnoreItemRequest>,
) -> AppResult<StatusCode> {
    let res = sqlx::query("UPDATE list_item SET ignored_by_user = ? WHERE id = ? AND list_id = ?")
        .bind(req.ignored)
        .bind(item_id)
        .bind(id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "list_item {item_id} on list {id} not found"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Derive the state string for a list item based on whether the
/// underlying movie/show exists locally + its watch state.
async fn derive_state(
    db: &sqlx::SqlitePool,
    tmdb_id: i64,
    item_type: &str,
    ignored: bool,
) -> AppResult<String> {
    if ignored {
        return Ok("ignored".into());
    }
    match item_type {
        "movie" => {
            #[derive(sqlx::FromRow)]
            struct Row {
                monitored: bool,
                watched_at: Option<String>,
                has_media: bool,
            }
            let row: Option<Row> = sqlx::query_as(
                "SELECT mv.monitored, mv.watched_at,
                        EXISTS(SELECT 1 FROM media m WHERE m.movie_id = mv.id) AS has_media
                 FROM movie mv WHERE mv.tmdb_id = ?",
            )
            .bind(tmdb_id)
            .fetch_optional(db)
            .await?;
            Ok(match row {
                None => "not_in_library".into(),
                Some(r) if r.watched_at.is_some() => "watched".into(),
                Some(r) if r.has_media => "acquired".into(),
                Some(r) if r.monitored => "monitoring".into(),
                Some(_) => "in_library".into(),
            })
        }
        "show" => {
            #[derive(sqlx::FromRow)]
            struct Row {
                monitored: bool,
            }
            let row: Option<Row> = sqlx::query_as("SELECT monitored FROM show WHERE tmdb_id = ?")
                .bind(tmdb_id)
                .fetch_optional(db)
                .await?;
            Ok(match row {
                None => "not_in_library".into(),
                Some(r) if r.monitored => "monitoring".into(),
                Some(_) => "in_library".into(),
            })
        }
        _ => Ok("not_in_library".into()),
    }
}

fn map_lists_err(e: ListsError) -> AppError {
    match e {
        ListsError::UnsupportedUrl(s) => AppError::BadRequest(s),
        ListsError::MissingMdblistKey => AppError::BadRequest(
            "MDBList API key required — add it in Settings → Integrations".into(),
        ),
        ListsError::TraktNotConnected => {
            AppError::BadRequest("Trakt must be connected to use Trakt lists".into())
        }
        ListsError::NotFound(s) => AppError::NotFound(s),
        ListsError::Auth(s) => AppError::BadRequest(format!("auth error: {s}")),
        ListsError::Network(s) => AppError::Internal(anyhow::anyhow!("source unreachable: {s}")),
        ListsError::Parse(s) => AppError::Internal(anyhow::anyhow!("parse: {s}")),
        ListsError::Db(e) => AppError::Internal(e.into()),
    }
}
