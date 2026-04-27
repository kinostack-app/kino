//! Display-title composition for event payloads.
//!
//! Every `AppEvent` variant carrying a `title` string is user-
//! facing (toasts, history, browser notifications). Episodes need
//! to read like "Breaking Bad · S01E04 · Cancer Man" — not the
//! torrent's release-parsed filename ("Breaking.Bad.S01E04.720p.x264-
//! GROUP"). Centralising the composition here guarantees every emit
//! site produces the same string.
//!
//! Takes `&SqlitePool` rather than `&AppState` so background
//! services (`import_trigger`, `download_monitor`) can call in
//! without threading state through.

use sqlx::SqlitePool;

/// "Show · `SxxExx` · Episode Title" when the episode + show exist
/// in the DB; falls back to a bare `SxxExx` when the title is null,
/// and an empty string when the episode was already deleted (the
/// caller is emitting a terminal event mid-cleanup). The empty
/// string path is deliberate — returning `None` would force every
/// caller to branch, and the frontend already tolerates blank
/// titles on `content_removed`-shaped events.
pub async fn episode_display_title(pool: &SqlitePool, episode_id: i64) -> String {
    #[derive(sqlx::FromRow)]
    struct Row {
        show_title: Option<String>,
        season_number: i64,
        episode_number: i64,
        title: Option<String>,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT s.title AS show_title, e.season_number, e.episode_number, e.title
         FROM episode e JOIN show s ON s.id = e.show_id WHERE e.id = ?",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some(r) => {
            let show = r.show_title.unwrap_or_default();
            let sxe = format!("S{:02}E{:02}", r.season_number, r.episode_number);
            match r.title {
                Some(t) if !t.is_empty() => format!("{show} · {sxe} · {t}"),
                _ => format!("{show} · {sxe}"),
            }
        }
        None => String::new(),
    }
}

/// Best-effort lookup: when a download row carries an episode id,
/// compose the display title from episode metadata; otherwise fall
/// back to the download's own `title` (release-parsed name). This
/// is the canonical "what string should this event carry?" decision
/// for download-lifecycle events (`DownloadStarted`, `DownloadPaused`
/// etc.) where the download's linked entity could be either a movie
/// or an episode.
///
/// `download_title` is the fallback — the release name — returned
/// unchanged for movies or when the episode lookup fails.
pub async fn download_display_title(
    pool: &SqlitePool,
    download_id: i64,
    download_title: &str,
) -> String {
    let episode_id: Option<i64> = sqlx::query_scalar(
        "SELECT episode_id FROM download_content WHERE download_id = ? AND episode_id IS NOT NULL LIMIT 1",
    )
    .bind(download_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some(ep_id) = episode_id {
        let composed = episode_display_title(pool, ep_id).await;
        if !composed.is_empty() {
            return composed;
        }
    }
    download_title.to_string()
}
