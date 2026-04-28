use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ImageParams {
    /// Target width in pixels.
    pub w: Option<u32>,
    /// Target height in pixels.
    pub h: Option<u32>,
    /// JPEG quality (1-100, default 85).
    pub quality: Option<u8>,
}

/// Serve a cached image with on-demand resizing.
///
/// `content_type`: "movies" or "shows"
/// `image_type`: "poster" or "backdrop"
#[utoipa::path(
    get,
    path = "/api/v1/images/{content_type}/{id}/{image_type}",
    params(
        ("content_type" = String, Path, description = "Content type: movies or shows"),
        ("id" = i64, Path, description = "Content ID (database ID)"),
        ("image_type" = String, Path, description = "Image type: poster or backdrop"),
        ImageParams,
    ),
    responses(
        (status = 200, description = "Image file", content_type = "image/jpeg"),
        (status = 404, description = "Image not found"),
        (status = 302, description = "Redirect to TMDB")
    ),
    tag = "images",
    security(("api_key" = []))
)]
pub async fn get_image(
    State(state): State<AppState>,
    Path((content_type, id, image_type)): Path<(String, i64, String)>,
    Query(params): Query<ImageParams>,
) -> AppResult<Response> {
    // Validate content_type and image_type
    if !matches!(content_type.as_str(), "movies" | "shows") {
        return Err(AppError::BadRequest(
            "content_type must be 'movies' or 'shows'".into(),
        ));
    }
    if !matches!(image_type.as_str(), "poster" | "backdrop" | "logo") {
        return Err(AppError::BadRequest(
            "image_type must be 'poster', 'backdrop' or 'logo'".into(),
        ));
    }

    // Logo path bypasses the TMDB-download + resize pipeline entirely:
    // logos are already cached to disk by the metadata sweep and
    // served as-is (SVG text or PNG bytes). See subsystem 29.
    if image_type == "logo" {
        return serve_logo(&state, &content_type, id).await;
    }

    let images = state.require_images()?;

    // Look up the TMDB path from the database
    let tmdb_path = match content_type.as_str() {
        "movies" => {
            let col = if image_type == "poster" {
                "poster_path"
            } else {
                "backdrop_path"
            };
            let query = format!("SELECT {col} FROM movie WHERE id = ?");
            sqlx::query_scalar::<_, Option<String>>(&query)
                .bind(id)
                .fetch_optional(&state.db)
                .await?
                .flatten()
        }
        "shows" => {
            let col = if image_type == "poster" {
                "poster_path"
            } else {
                "backdrop_path"
            };
            let query = format!("SELECT {col} FROM show WHERE id = ?");
            sqlx::query_scalar::<_, Option<String>>(&query)
                .bind(id)
                .fetch_optional(&state.db)
                .await?
                .flatten()
        }
        _ => None,
    };

    // Look up tmdb_id for the content
    let tmdb_id: i64 = match content_type.as_str() {
        "movies" => sqlx::query_scalar("SELECT tmdb_id FROM movie WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("{content_type} {id} not found")))?,
        "shows" => sqlx::query_scalar("SELECT tmdb_id FROM show WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("{content_type} {id} not found")))?,
        _ => return Err(AppError::BadRequest("invalid content type".into())),
    };

    // Try to get the original (downloads if not cached)
    let original = images
        .get_original(&content_type, tmdb_id, &image_type, tmdb_path.as_deref())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

    // Compute blurhash once per image if not yet cached in DB.
    if let Some(ref path) = original {
        maybe_store_blurhash(&state, &content_type, id, &image_type, images, path).await;
    }

    let Some(original_path) = original else {
        // No image available — if we have a TMDB path, redirect to TMDB
        if let Some(ref path) = tmdb_path {
            let redirect_url = format!("https://image.tmdb.org/t/p/w500{path}");
            return Ok((StatusCode::FOUND, [(header::LOCATION, redirect_url)]).into_response());
        }
        return Err(AppError::NotFound("image not available".into()));
    };

    // Resize if requested
    let serve_path = images
        .get_resized(&original_path, params.w, params.h, params.quality)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

    // Read and serve the file
    let bytes = tokio::fs::read(&serve_path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read image: {e}")))?;

    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    // Long-lived cache: 365 days, immutable
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    Ok(response)
}

/// Serve a cached clearlogo by entity. Reads `logo_path` from the
/// entity row and streams the bytes with the correct content-type.
///
/// Three states per entity:
///   - `logo_path` populated → fast path, read from disk + serve.
///   - `logo_path` NULL → never tried; lazy-fetch from TMDB now
///     (same pattern as poster caching, see `ImageCache::get_original`).
///     On success, populate the column + serve. On failure, write an
///     empty-string sentinel so we don't hammer TMDB on every view.
///   - `logo_path = ""` → negative-cache sentinel; return 404 without
///     refetching. To force a retry the entity has to be re-followed.
async fn serve_logo(state: &AppState, content_type: &str, id: i64) -> AppResult<Response> {
    let table = match content_type {
        "movies" => "movie",
        "shows" => "show",
        _ => return Err(AppError::BadRequest("invalid content type".into())),
    };
    let sql = format!("SELECT logo_path FROM {table} WHERE id = ?");
    let rel: Option<String> = sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    tracing::info!(content_type, id, ?rel, "serve_logo cache lookup");

    let rel = match rel {
        Some(s) if !s.is_empty() => s,
        Some(_) => {
            tracing::info!(
                content_type,
                id,
                "serve_logo: negative-cache hit, returning 404"
            );
            return Err(AppError::NotFound(format!(
                "no logo available for {content_type} {id}"
            )));
        }
        None => {
            tracing::info!(
                content_type,
                id,
                "serve_logo: cache miss, lazy fetching from TMDB"
            );
            lazy_fetch_logo(state, content_type, id).await?
        }
    };

    let abs = state.data_path.join("images").join(&rel);
    let bytes = tokio::fs::read(&abs)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("read logo {}: {e}", abs.display())))?;
    let mime = if std::path::Path::new(&rel)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
    {
        "image/svg+xml"
    } else {
        "image/png"
    };

    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(response)
}

/// Cache miss — call TMDB's `/images` endpoint via
/// `refresh_entity_logo`, which handles candidate selection +
/// sanitisation + disk write + DB update. Returns the populated
/// `logo_path` on success. On failure, writes the empty-string
/// sentinel into `logo_path` so subsequent requests skip the fetch.
async fn lazy_fetch_logo(state: &AppState, content_type: &str, id: i64) -> AppResult<String> {
    use crate::metadata::logos::{ContentType, refresh_entity_logo};

    let (ct_enum, table) = match content_type {
        "movies" => (ContentType::Movie, "movie"),
        "shows" => (ContentType::Show, "show"),
        _ => return Err(AppError::BadRequest("invalid content type".into())),
    };

    let tmdb = state.require_tmdb()?;
    let tmdb_id: i64 = sqlx::query_scalar(&format!("SELECT tmdb_id FROM {table} WHERE id = ?"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("{content_type} {id} not found")))?;
    tracing::info!(
        content_type,
        id,
        tmdb_id,
        "lazy_fetch_logo: hitting TMDB /images"
    );

    let http = reqwest::Client::new();
    // refresh_entity_logo is best-effort — it logs on failure and
    // returns Ok(()) without writing the column. We re-query the
    // row after the call to see whether it actually landed.
    match refresh_entity_logo(
        &state.db,
        &tmdb,
        &http,
        &state.data_path,
        ct_enum,
        id,
        tmdb_id,
    )
    .await
    {
        Ok(()) => tracing::info!(
            content_type,
            id,
            tmdb_id,
            "lazy_fetch_logo: refresh returned Ok"
        ),
        Err(e) => {
            tracing::warn!(content_type, id, tmdb_id, error = %e, "lazy_fetch_logo: refresh errored");
        }
    }

    let refreshed: Option<String> =
        sqlx::query_scalar(&format!("SELECT logo_path FROM {table} WHERE id = ?"))
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    tracing::info!(
        content_type,
        id,
        ?refreshed,
        "lazy_fetch_logo: post-refresh column value"
    );
    match refreshed {
        Some(s) if !s.is_empty() => Ok(s),
        _ => {
            // Write sentinel — prevents retry storms on entities TMDB
            // has no logo for. Only writes if the column is still NULL
            // (defence against a race with another concurrent fetch).
            sqlx::query(&format!(
                "UPDATE {table} SET logo_path = '' WHERE id = ? AND logo_path IS NULL"
            ))
            .bind(id)
            .execute(&state.db)
            .await?;
            tracing::info!(
                content_type,
                id,
                "lazy_fetch_logo: wrote negative-cache sentinel"
            );
            Err(AppError::NotFound(format!(
                "no logo available for {content_type} {id}"
            )))
        }
    }
}

/// Compute + persist a blurhash for `image_type` ("poster" or "backdrop")
/// if the DB column is NULL. Best-effort — errors are swallowed.
async fn maybe_store_blurhash(
    state: &AppState,
    content_type: &str,
    id: i64,
    image_type: &str,
    images: &crate::images::ImageCache,
    path: &std::path::Path,
) {
    let table = match content_type {
        "movies" => "movie",
        "shows" => "show",
        _ => return,
    };
    let col = match image_type {
        "poster" => "blurhash_poster",
        "backdrop" => "blurhash_backdrop",
        _ => return,
    };

    // Skip if already set.
    let existing: Option<String> =
        sqlx::query_scalar::<_, Option<String>>(&format!("SELECT {col} FROM {table} WHERE id = ?"))
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .flatten();
    if existing.is_some_and(|s| !s.is_empty()) {
        return;
    }

    if let Some(hash) = images.compute_blurhash(path).await {
        let _ = sqlx::query(&format!("UPDATE {table} SET {col} = ? WHERE id = ?"))
            .bind(hash)
            .bind(id)
            .execute(&state.db)
            .await;
    }
}
