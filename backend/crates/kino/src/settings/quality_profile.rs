//! Quality profiles — the per-library tier ladder + cutoff that
//! `AcquisitionPolicy::evaluate` consults at every grab. This module
//! owns the row model + the HTTP CRUD that lets the user manage
//! profiles from settings.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::events::{AppEvent, IndexerAction};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct QualityProfile {
    pub id: i64,
    pub name: String,
    pub upgrade_allowed: bool,
    pub cutoff: String,
    pub items: String,
    pub accepted_languages: String,
    /// Exactly one profile is flagged as the default; newly-monitored
    /// content picks it up when the caller doesn't specify a profile.
    /// The API handler enforces the "at most one default" invariant by
    /// clearing the flag on the others when a new default is set.
    pub is_default: bool,
}

/// Profile + usage snapshot returned by the list endpoint. Settings UI
/// uses `usage_count` to show "in use by N items" and to explain why a
/// delete might be blocked.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct QualityProfileWithUsage {
    #[serde(flatten)]
    pub profile: QualityProfile,
    pub usage_count: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateQualityProfile {
    pub name: String,
    pub upgrade_allowed: Option<bool>,
    pub cutoff: String,
    pub items: String,
    pub accepted_languages: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateQualityProfile {
    pub name: Option<String>,
    pub upgrade_allowed: Option<bool>,
    pub cutoff: Option<String>,
    pub items: Option<String>,
    pub accepted_languages: Option<String>,
}

/// A single quality tier within a profile.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QualityTier {
    pub quality_id: String,
    pub name: String,
    pub allowed: bool,
    pub rank: i64,
}

/// Default quality profile items.
pub fn default_quality_items() -> String {
    let tiers: &[(&str, &str, bool, i64)] = &[
        ("remux_2160p", "Remux 2160p", true, 18),
        ("bluray_2160p", "Bluray 2160p", true, 17),
        ("web_2160p", "WEB 2160p", true, 16),
        ("hdtv_2160p", "HDTV 2160p", true, 15),
        ("remux_1080p", "Remux 1080p", true, 14),
        ("bluray_1080p", "Bluray 1080p", true, 13),
        ("web_1080p", "WEB 1080p", true, 12),
        ("hdtv_1080p", "HDTV 1080p", true, 11),
        ("bluray_720p", "Bluray 720p", true, 10),
        ("web_720p", "WEB 720p", true, 9),
        ("hdtv_720p", "HDTV 720p", true, 8),
        ("bluray_480p", "Bluray 480p", false, 7),
        ("web_480p", "WEB 480p", false, 6),
        ("dvd", "DVD", false, 5),
        ("sdtv", "SDTV", false, 4),
        ("telecine", "Telecine", false, 3),
        ("telesync", "Telesync", false, 2),
        ("cam", "CAM", false, 1),
    ];
    let items: Vec<QualityTier> = tiers
        .iter()
        .map(|(id, name, allowed, rank)| QualityTier {
            quality_id: (*id).into(),
            name: (*name).into(),
            allowed: *allowed,
            rank: *rank,
        })
        .collect();
    serde_json::to_string(&items).expect("default quality items serialize")
}

// ─── HTTP handlers ──────────────────────────────────────────────────

/// Resolve the quality profile for a new movie/show. Explicit id wins;
/// otherwise pick the row flagged `is_default`; otherwise fall back to
/// the smallest id so the FK is never violated.
pub async fn resolve_quality_profile(
    db: &sqlx::SqlitePool,
    supplied: Option<i64>,
) -> AppResult<i64> {
    if let Some(id) = supplied {
        return Ok(id);
    }
    let default: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM quality_profile WHERE is_default = 1 ORDER BY id LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if let Some(id) = default {
        return Ok(id);
    }
    sqlx::query_scalar::<_, i64>("SELECT id FROM quality_profile ORDER BY id LIMIT 1")
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError::BadRequest("no quality profile available".into()))
}

/// List all quality profiles with their usage counts so the settings
/// UI can show "in use by N items" badges and explain delete blocks.
#[utoipa::path(
    get,
    path = "/api/v1/quality-profiles",
    responses(
        (status = 200, description = "List of quality profiles", body = Vec<QualityProfileWithUsage>)
    ),
    tag = "quality_profiles",
    security(("api_key" = []))
)]
pub async fn list_quality_profiles(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<QualityProfileWithUsage>>> {
    let profiles = sqlx::query_as::<_, QualityProfile>("SELECT * FROM quality_profile ORDER BY id")
        .fetch_all(&state.db)
        .await?;

    // One cheap aggregated query per entity kind; joining via id map in
    // memory is easier than a `UNION ALL` LEFT JOIN that has to deal
    // with the profile having zero references.
    let movie_counts: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT quality_profile_id, COUNT(*) FROM movie GROUP BY quality_profile_id",
    )
    .fetch_all(&state.db)
    .await?;
    let show_counts: Vec<(i64, i64)> =
        sqlx::query_as("SELECT quality_profile_id, COUNT(*) FROM show GROUP BY quality_profile_id")
            .fetch_all(&state.db)
            .await?;

    let mut counts: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for (id, c) in movie_counts.into_iter().chain(show_counts) {
        *counts.entry(id).or_insert(0) += c;
    }

    let list = profiles
        .into_iter()
        .map(|p| QualityProfileWithUsage {
            usage_count: counts.get(&p.id).copied().unwrap_or(0),
            profile: p,
        })
        .collect();
    Ok(Json(list))
}

/// Get a quality profile by ID.
#[utoipa::path(
    get,
    path = "/api/v1/quality-profiles/{id}",
    params(("id" = i64, Path, description = "Quality profile ID")),
    responses(
        (status = 200, description = "Quality profile", body = QualityProfile),
        (status = 404, description = "Not found")
    ),
    tag = "quality_profiles",
    security(("api_key" = []))
)]
pub async fn get_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<QualityProfile>> {
    let profile = sqlx::query_as::<_, QualityProfile>("SELECT * FROM quality_profile WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("quality profile with id {id} not found")))?;

    Ok(Json(profile))
}

/// Create a new quality profile.
#[utoipa::path(
    post,
    path = "/api/v1/quality-profiles",
    request_body = CreateQualityProfile,
    responses(
        (status = 201, description = "Created quality profile", body = QualityProfile)
    ),
    tag = "quality_profiles",
    security(("api_key" = []))
)]
pub async fn create_quality_profile(
    State(state): State<AppState>,
    Json(input): Json<CreateQualityProfile>,
) -> AppResult<(StatusCode, Json<QualityProfile>)> {
    // Validate items is valid JSON array of quality tiers
    let _: Vec<crate::settings::quality_profile::QualityTier> = serde_json::from_str(&input.items)
        .map_err(|e| AppError::BadRequest(format!("invalid quality items JSON: {e}")))?;

    let upgrade_allowed = input.upgrade_allowed.unwrap_or(true);
    let accepted_languages = input
        .accepted_languages
        .unwrap_or_else(|| r#"["en"]"#.to_owned());

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO quality_profile (name, upgrade_allowed, cutoff, items, accepted_languages) VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&input.name)
    .bind(upgrade_allowed)
    .bind(&input.cutoff)
    .bind(&input.items)
    .bind(&accepted_languages)
    .fetch_one(&state.db)
    .await?;

    let profile = QualityProfile {
        id,
        name: input.name,
        upgrade_allowed,
        cutoff: input.cutoff,
        items: input.items,
        accepted_languages,
        is_default: false,
    };

    let _ = state.event_tx.send(AppEvent::QualityProfileChanged {
        profile_id: id,
        action: IndexerAction::Created,
    });

    Ok((StatusCode::CREATED, Json(profile)))
}

/// Update a quality profile.
#[utoipa::path(
    put,
    path = "/api/v1/quality-profiles/{id}",
    params(("id" = i64, Path, description = "Quality profile ID")),
    request_body = UpdateQualityProfile,
    responses(
        (status = 200, description = "Updated quality profile", body = QualityProfile),
        (status = 404, description = "Not found")
    ),
    tag = "quality_profiles",
    security(("api_key" = []))
)]
pub async fn update_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(update): Json<UpdateQualityProfile>,
) -> AppResult<Json<QualityProfile>> {
    // Validate items if provided
    if let Some(ref items) = update.items {
        let _: Vec<crate::settings::quality_profile::QualityTier> = serde_json::from_str(items)
            .map_err(|e| AppError::BadRequest(format!("invalid quality items JSON: {e}")))?;
    }

    let result = sqlx::query(
        r"UPDATE quality_profile SET
            name               = COALESCE(?, name),
            upgrade_allowed    = COALESCE(?, upgrade_allowed),
            cutoff             = COALESCE(?, cutoff),
            items              = COALESCE(?, items),
            accepted_languages = COALESCE(?, accepted_languages)
        WHERE id = ?",
    )
    .bind(update.name.as_deref())
    .bind(update.upgrade_allowed)
    .bind(update.cutoff.as_deref())
    .bind(update.items.as_deref())
    .bind(update.accepted_languages.as_deref())
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "quality profile with id {id} not found"
        )));
    }

    // Re-fetch updated profile
    let profile = sqlx::query_as::<_, QualityProfile>("SELECT * FROM quality_profile WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    let _ = state.event_tx.send(AppEvent::QualityProfileChanged {
        profile_id: id,
        action: IndexerAction::Updated,
    });

    Ok(Json(profile))
}

/// Delete a quality profile.
#[utoipa::path(
    delete,
    path = "/api/v1/quality-profiles/{id}",
    params(("id" = i64, Path, description = "Quality profile ID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Profile in use")
    ),
    tag = "quality_profiles",
    security(("api_key" = []))
)]
pub async fn delete_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Reject deleting the default profile. Without this, new-follow
    // codepaths that resolve "the default" would fail on next grab
    // with a confusing FK error — and any movie/show currently
    // pointing at this row would also 409 below, so the user-visible
    // behaviour is "stuck with no default until you pick a new one".
    // Making the caller explicitly promote another profile first
    // keeps the invariant "exactly one default exists" intact.
    let is_default: Option<bool> =
        sqlx::query_scalar("SELECT is_default FROM quality_profile WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    if matches!(is_default, Some(true)) {
        return Err(AppError::Conflict(
            "can't delete the default profile — promote another profile first".into(),
        ));
    }

    // Check if any movies or shows reference this profile
    let movie_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM movie WHERE quality_profile_id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await?;

    let show_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM show WHERE quality_profile_id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await?;

    if movie_count > 0 || show_count > 0 {
        return Err(AppError::Conflict(
            "quality profile is in use by movies or shows".into(),
        ));
    }

    let result = sqlx::query("DELETE FROM quality_profile WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "quality profile with id {id} not found"
        )));
    }

    let _ = state.event_tx.send(AppEvent::QualityProfileChanged {
        profile_id: id,
        action: IndexerAction::Deleted,
    });

    Ok(StatusCode::NO_CONTENT)
}

/// Promote a quality profile to be the default. Clears the flag on
/// every other profile in the same transaction so the invariant
/// "exactly one default" is preserved.
#[utoipa::path(
    post,
    path = "/api/v1/quality-profiles/{id}/set-default",
    params(("id" = i64, Path, description = "Quality profile ID")),
    responses(
        (status = 204, description = "Default updated"),
        (status = 404, description = "Not found")
    ),
    tag = "quality_profiles",
    security(("api_key" = []))
)]
pub async fn set_default_quality_profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let mut tx = state.db.begin().await?;

    // Verify the target exists before touching anything.
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM quality_profile WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound(format!(
            "quality profile with id {id} not found"
        )));
    }

    sqlx::query("UPDATE quality_profile SET is_default = 0 WHERE id != ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE quality_profile SET is_default = 1 WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let _ = state.event_tx.send(AppEvent::QualityProfileChanged {
        profile_id: id,
        action: IndexerAction::Updated,
    });
    tracing::info!(profile_id = id, "quality profile set as default");
    Ok(StatusCode::NO_CONTENT)
}
