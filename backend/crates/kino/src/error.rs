use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    BadRequest(String),

    /// Auth-related rejections that want to surface a specific
    /// reason ("invalid API key", "session expired", "token already
    /// consumed") without falling all the way to a generic 401.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Authenticated but the OS denied access — the kino service
    /// user lacks permission to read / write the requested path.
    /// Distinct from 401 (auth) so the path-picker can render a
    /// "kino can't access this drive" hint instead of "session
    /// expired".
    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("{0}")]
    Conflict(String),

    #[error("{0}")]
    Unprocessable(String),

    /// Rate-limited by an upstream service (TMDB, `OpenSubtitles`,
    /// Trakt, indexer). Mapped to HTTP 429 so clients can honour
    /// their own retry policy rather than treating it as a
    /// generic 500. The optional `retry_after_secs` is surfaced in
    /// the `Retry-After` response header when present.
    #[error("rate limited: {message}")]
    RateLimited {
        message: String,
        retry_after_secs: Option<u64>,
    },

    /// Dependency is unavailable — the ffmpeg binary is missing,
    /// librqbit isn't running (VPN required but failed), TMDB is
    /// returning 5xx. Maps to HTTP 503 so clients can distinguish
    /// "your server is broken" (500) from "a dependency is down"
    /// (503) and back off appropriately.
    #[error("service unavailable: {0}")]
    Unavailable(String),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::Conflict(_) => "conflict",
            Self::Unprocessable(_) => "unprocessable_entity",
            Self::RateLimited { .. } => "rate_limited",
            Self::Unavailable(_) => "service_unavailable",
            Self::Database(_) | Self::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let code = self.error_code();

        // Log server errors. RateLimited / Unavailable log at WARN
        // because they're common on upstream flakiness and spamming
        // ERROR during e.g. a TMDB outage drowns the log.
        match &self {
            Self::Database(e) => tracing::error!(error = %e, "database error"),
            Self::Internal(e) => tracing::error!(error = %e, "internal error"),
            Self::RateLimited {
                message,
                retry_after_secs,
            } => tracing::warn!(
                message = %message,
                retry_after_secs = ?retry_after_secs,
                "upstream rate limit",
            ),
            Self::Unavailable(message) => {
                tracing::warn!(message = %message, "dependency unavailable");
            }
            _ => {}
        }

        let body = serde_json::json!({
            "error": {
                "code": code,
                "message": self.to_string()
            }
        });

        // Surface `Retry-After` when the upstream told us how long
        // to wait. The client can then honour it verbatim instead
        // of guessing a backoff. Header value is a decimal integer
        // in seconds per RFC 7231 §7.1.3.
        let retry_after = match &self {
            Self::RateLimited {
                retry_after_secs: Some(secs),
                ..
            } => Some(*secs),
            _ => None,
        };

        let mut response = (status, axum::Json(body)).into_response();
        if let Some(secs) = retry_after
            && let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string())
        {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, value);
        }
        response
    }
}

pub type AppResult<T> = Result<T, AppError>;
