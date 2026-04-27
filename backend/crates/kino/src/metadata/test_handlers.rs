//! Settings test endpoints — one-shot probes for the external-service
//! credentials the Metadata settings page collects.
//!
//! Each endpoint reads the *currently stored* config (not a transient
//! body) so the "Test" button always reflects what will be used at
//! runtime. That also means the user has to save their changes before
//! testing — the save bar on the settings page covers that.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{AppError, AppResult};
use crate::integrations::opensubtitles::{OpenSubtitlesClient, OsCredentials};
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
}

/// `POST /api/v1/metadata/test-tmdb` — hit a stable known endpoint
/// (The Matrix, id 603) and confirm the key is accepted. More reliable
/// than `/search?q=test` which can succeed for unrelated reasons.
#[utoipa::path(
    post, path = "/api/v1/metadata/test-tmdb",
    responses((status = 200, body = TestResult)),
    tag = "metadata",
    security(("api_key" = []))
)]
pub async fn test_tmdb(State(state): State<AppState>) -> AppResult<Json<TestResult>> {
    let Some(ref tmdb) = state.tmdb else {
        return Ok(Json(TestResult {
            ok: false,
            message: "TMDB API key is not set".into(),
        }));
    };
    match tmdb.movie_details(603).await {
        Ok(_) => Ok(Json(TestResult {
            ok: true,
            message: "TMDB credentials valid".into(),
        })),
        Err(e) => Ok(Json(TestResult {
            ok: false,
            message: format!("TMDB rejected request: {e}"),
        })),
    }
}

/// `POST /api/v1/metadata/test-opensubtitles` — attempts a login with
/// the stored `OpenSubtitles` credentials. Reports the specific HTTP
/// response verbatim on failure so the user sees whether it's the key,
/// the username, or the password that's wrong.
#[utoipa::path(
    post, path = "/api/v1/metadata/test-opensubtitles",
    responses((status = 200, body = TestResult)),
    tag = "metadata",
    security(("api_key" = []))
)]
pub async fn test_opensubtitles(State(state): State<AppState>) -> AppResult<Json<TestResult>> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT opensubtitles_api_key, opensubtitles_username, opensubtitles_password FROM config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?;

    let Some((Some(api_key), Some(username), Some(password))) = row else {
        return Ok(Json(TestResult {
            ok: false,
            message: "OpenSubtitles credentials are incomplete".into(),
        }));
    };

    if api_key.trim().is_empty() || username.trim().is_empty() || password.trim().is_empty() {
        return Ok(Json(TestResult {
            ok: false,
            message: "OpenSubtitles credentials are incomplete".into(),
        }));
    }

    let client = OpenSubtitlesClient::new(OsCredentials {
        api_key,
        username,
        password,
    });

    match client.test_login().await {
        Ok(()) => Ok(Json(TestResult {
            ok: true,
            message: "OpenSubtitles login successful".into(),
        })),
        Err(e) => Ok(Json(TestResult {
            ok: false,
            message: format!("OpenSubtitles login failed: {e}"),
        })),
    }
}

// Re-expose AppError on unused import-free path — not strictly needed
// here but keeps the module compilable if the SQL layer switches to a
// typed error in the future.
#[allow(dead_code)]
fn _compile_marker(_: AppError) {}
