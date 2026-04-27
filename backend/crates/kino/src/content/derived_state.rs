//! Content-state derivation for movies and episodes.
//!
//! The persistent `status` columns on movie / episode are derived
//! at read time from `download.state` + `media` / `media_episode` +
//! `watched_at`. A SQL-level CASE expression covers list endpoints;
//! Rust callers that already have the three signals to hand call
//! [`derive_content_state`].
//!
//! Both paths produce the same [`ContentStatus`] type — the enum
//! that the `OpenAPI` components register and the frontend mirrors as
//! a typed union.

use crate::download::DownloadPhase;
use crate::models::enums::ContentStatus;

/// Derive content state from the three signals that define it. The
/// precedence matches the frontend's existing `derivePhase`:
/// watched beats available beats downloading beats wanted.
#[must_use]
pub fn derive_content_state(
    has_media: bool,
    has_active_download: bool,
    watched_at: Option<&str>,
) -> ContentStatus {
    if watched_at.is_some_and(|s| !s.is_empty()) {
        return ContentStatus::Watched;
    }
    if has_media {
        return ContentStatus::Available;
    }
    if has_active_download {
        return ContentStatus::Downloading;
    }
    ContentStatus::Wanted
}

/// Drop-in SELECT clause for movie rows. Produces `movie.*` columns
/// plus a derived `status` string column matching
/// [`ContentStatus::as_str`]. Built at call time so the active-phase
/// IN list stays in lockstep with
/// [`DownloadPhase::is_pre_import_active`].
#[must_use]
pub fn movie_status_select() -> String {
    let active = DownloadPhase::sql_in_clause(DownloadPhase::is_pre_import_active);
    format!(
        "SELECT mv.*,
    CASE
      WHEN mv.watched_at IS NOT NULL AND mv.watched_at != '' THEN 'watched'
      WHEN EXISTS(SELECT 1 FROM media m WHERE m.movie_id = mv.id) THEN 'available'
      WHEN EXISTS(
        SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
        WHERE dc.movie_id = mv.id
          AND d.state IN ({active})
      ) THEN 'downloading'
      ELSE 'wanted'
    END AS status
  FROM movie mv"
    )
}

/// Drop-in SELECT clause for episode rows. Aliases `episode` as `e`.
#[must_use]
pub fn episode_status_select() -> String {
    let active = DownloadPhase::sql_in_clause(DownloadPhase::is_pre_import_active);
    format!(
        "SELECT e.*,
    CASE
      WHEN e.watched_at IS NOT NULL AND e.watched_at != '' THEN 'watched'
      WHEN EXISTS(SELECT 1 FROM media_episode me WHERE me.episode_id = e.id) THEN 'available'
      WHEN EXISTS(
        SELECT 1 FROM download_content dc JOIN download d ON d.id = dc.download_id
        WHERE dc.episode_id = e.id
          AND d.state IN ({active})
      ) THEN 'downloading'
      ELSE 'wanted'
    END AS status
  FROM episode e"
    )
}

/// Comma-joined SQL fragment of download states that count as
/// "active" (an in-flight acquisition). Equivalent to
/// `DownloadPhase::sql_in_clause(DownloadPhase::is_pre_import_active)`
/// — kept as a thin wrapper so callers can interpolate it into
/// hand-built SQL without an explicit predicate parameter.
#[must_use]
pub fn active_download_states() -> String {
    DownloadPhase::sql_in_clause(DownloadPhase::is_pre_import_active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_beats_everything() {
        assert_eq!(
            derive_content_state(true, true, Some("2026-01-01")),
            ContentStatus::Watched
        );
        assert_eq!(
            derive_content_state(false, false, Some("2026-01-01")),
            ContentStatus::Watched
        );
    }

    #[test]
    fn available_beats_downloading() {
        assert_eq!(
            derive_content_state(true, true, None),
            ContentStatus::Available
        );
    }

    #[test]
    fn downloading_when_no_media() {
        assert_eq!(
            derive_content_state(false, true, None),
            ContentStatus::Downloading
        );
    }

    #[test]
    fn wanted_default() {
        assert_eq!(
            derive_content_state(false, false, None),
            ContentStatus::Wanted
        );
        assert_eq!(
            derive_content_state(false, false, Some("")),
            ContentStatus::Wanted
        );
    }
}
