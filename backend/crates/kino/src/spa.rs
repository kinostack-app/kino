//! In-app SPA serving.
//!
//! `frontend/dist/` (the `npm run build` output of the React/Vite
//! SPA) is compiled into the kino binary via `rust-embed` so a
//! single-file deploy ships both the API and the UI. At request
//! time we look up the path in the embedded archive; if it's
//! present we stream it back, if not we fall back to
//! `index.html` so the SPA's client-side router (`TanStack` Router)
//! can take over for deep links like `/library` or
//! `/play/movie/4`.
//!
//! ## Cache headers
//!
//! Vite emits two flavours of asset:
//!
//! - **Hashed**: `assets/index-<hash>.js`, `assets/<name>-<hash>.css`.
//!   Filename changes whenever content changes, so we serve them
//!   with `public, max-age=31536000, immutable` — browser caches
//!   forever, no revalidation.
//! - **Unhashed**: `index.html`, `favicon.ico`, etc. Filename
//!   stable across deploys; we serve with `no-cache` so the
//!   browser revalidates each load and picks up new asset
//!   filenames immediately after a release.
//!
//! ## Build dependency
//!
//! `frontend/dist/` must exist at compile time. `build.rs` runs
//! `npm ci && npm run build` if `dist/index.html` is missing,
//! falling back to a clear error if `npm` isn't on PATH.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::{Embed, EmbeddedFile};

// `folder` is resolved by rust-embed relative to this crate's
// Cargo.toml (CARGO_MANIFEST_DIR). From `backend/crates/kino/`,
// `../../../frontend/dist/` reaches the repo-root frontend build
// output. v8's derive is `Embed` (renamed from RustEmbed in v7).
#[derive(Embed)]
#[folder = "../../../frontend/dist/"]
struct Assets;

/// Catch-all handler used as the axum router's `.fallback(...)`.
/// API routes are matched first by axum; anything that falls
/// through (a SPA route, a static asset, or `/`) lands here.
pub async fn handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // The fallback runs for unmatched API routes too — surface
    // those as 404 instead of pretending /api/v1/typo is a SPA
    // route. swagger-ui lives at `/api/docs/` and is registered
    // explicitly, so it doesn't reach this branch.
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Empty path = index.html. Helps so `GET /` works without
    // the browser needing to redirect.
    let lookup_path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(lookup_path) {
        return serve_asset(lookup_path, file);
    }

    // SPA fallback: any path that doesn't match a file becomes
    // `index.html` so client-side routing works. Vite's hashed
    // asset paths can never miss in practice (any miss there
    // means a stale-cached SPA hitting a deploy that already
    // ran past the cache horizon — index.html will reload, the
    // SPA bootstraps fresh assets).
    let Some(index) = Assets::get("index.html") else {
        // Compile-time invariant: index.html must be in the bundle.
        // If it's missing, the build is broken.
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "kino: SPA bundle missing index.html — backend was built without a frontend/dist/. \
             Re-run `npm run build` in frontend/ and rebuild.",
        )
            .into_response();
    };
    serve_index(index)
}

fn serve_asset(path: &str, file: EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache_control = if is_hashed_asset(path) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let body = Body::from(file.data.into_owned());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control),
        )
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn serve_index(file: EmbeddedFile) -> Response {
    let body = Body::from(file.data.into_owned());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Vite-bundled hashed assets land under `assets/` with an 8-char
/// hash in the basename, e.g. `assets/index-abc12345.js`. Anything
/// outside `assets/` (favicon, robots, top-level html) is unhashed
/// and gets `no-cache`.
fn is_hashed_asset(path: &str) -> bool {
    path.starts_with("assets/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_asset_detection() {
        assert!(is_hashed_asset("assets/index-abc12345.js"));
        assert!(is_hashed_asset("assets/style-deadbeef.css"));
        assert!(!is_hashed_asset("index.html"));
        assert!(!is_hashed_asset("favicon.ico"));
        assert!(!is_hashed_asset("robots.txt"));
    }
}
