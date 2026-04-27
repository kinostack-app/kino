//! Diff-and-apply sync engine + first-add orchestration.
//!
//! The only writer of `list` and `list_item` rows. Resolvers stay
//! read-only; this module owns the state transitions.
//!
//! Design notes:
//!   - **add-never-subtract** — removing an item from the source list
//!     never deletes a local `Movie`/`Show`. The `list_item` row goes
//!     (mirrors current source state) but the underlying library
//!     entity is left alone. Inverse: `ignored_by_user = 1` is
//!     preserved across polls so the user's manual unmonitor isn't
//!     re-fought every refresh cycle.
//!   - List items do *not* eagerly create `Movie`/`Show` rows. That
//!     would pull hundreds of TMDB requests on first import of a
//!     big list. The `/lists/{id}/items` response joins "is this in
//!     the library?" state per-row; users click through and the
//!     normal add flow runs for items they actually want.

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use super::{ListsError, ParsedList, RawItem, SourceType, fetch_items, fetch_metadata, parser};
use crate::events::AppEvent;
use crate::integrations::lists::model::{List, ListPreview};

/// Counts returned from [`apply_poll`]. The caller uses these to
/// decide whether to fire the bulk-growth notification.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyOutcome {
    pub added: i64,
    pub removed: i64,
    pub unchanged: i64,
}

/// Preview + parse in one step. Used by `POST /api/v1/lists` to
/// populate the soft-cap dialog before the user commits.
pub async fn preview_list(
    db: &SqlitePool,
    url: &str,
) -> Result<(ParsedList, ListPreview), ListsError> {
    let parsed = parser::parse_list_url(url)?;

    // Trakt watchlist can't be user-added — it's auto-created on
    // Trakt connect. Reject explicitly so the frontend can show a
    // clear message rather than a generic error.
    if parsed.source_type == SourceType::TraktWatchlist {
        return Err(ListsError::UnsupportedUrl(
            "Trakt watchlist is auto-managed — it appears in Lists whenever Trakt is connected"
                .into(),
        ));
    }

    let meta = fetch_metadata(db, &parsed).await?;
    let preview = ListPreview {
        source_type: parsed.source_type.as_str().into(),
        source_id: parsed.source_id.clone(),
        title: meta.title,
        description: meta.description,
        item_count: meta.item_count,
        item_type: meta.item_type,
    };
    Ok((parsed, preview))
}

/// First-add of a non-system list. Writes the `list` row, pulls
/// items, applies them through [`apply_poll`]. The caller is trusted
/// to have confirmed with the user — we don't gate on item count.
pub async fn create_list(db: &SqlitePool, parsed: &ParsedList) -> Result<List, ListsError> {
    let meta = fetch_metadata(db, parsed).await?;
    let now = crate::time::Timestamp::now().to_rfc3339();

    let list_id: i64 = sqlx::query_scalar(
        "INSERT INTO list
           (source_type, source_url, source_id, title, description,
            item_count, item_type, is_system, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)
         RETURNING id",
    )
    .bind(parsed.source_type.as_str())
    .bind(&parsed.source_url)
    .bind(&parsed.source_id)
    .bind(&meta.title)
    .bind(&meta.description)
    .bind(meta.item_count)
    .bind(&meta.item_type)
    .bind(&now)
    .fetch_one(db)
    .await?;

    let items = fetch_items(db, parsed).await?;
    apply_poll(db, list_id, items).await?;

    // Re-read so the caller gets the authoritative row.
    let list: List = sqlx::query_as("SELECT * FROM list WHERE id = ?")
        .bind(list_id)
        .fetch_one(db)
        .await?;
    Ok(list)
}

/// Ensure the Trakt watchlist has exactly one `list` row while Trakt
/// is connected. Called from the Trakt auth module on connect (idempotent)
/// and from the disconnect path (which calls [`remove_trakt_watchlist`]).
/// On first-time create, fires [`AppEvent::ListAutoAdded`] so the user
/// sees a "Trakt watchlist added to Lists" notification.
pub async fn ensure_trakt_watchlist(
    db: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
) -> Result<(), ListsError> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM list WHERE source_type = 'trakt_watchlist' AND is_system = 1 LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if existing.is_some() {
        return Ok(());
    }
    // Fetch the username from trakt_auth so we know the canonical URL.
    let username: Option<String> =
        sqlx::query_scalar("SELECT username FROM trakt_auth WHERE id = 1")
            .fetch_optional(db)
            .await?
            .flatten();
    let Some(user) = username else {
        return Err(ListsError::TraktNotConnected);
    };
    let now = crate::time::Timestamp::now().to_rfc3339();
    let list_id: i64 = sqlx::query_scalar(
        "INSERT INTO list
           (source_type, source_url, source_id, title, description,
            item_count, item_type, is_system, created_at)
         VALUES
           ('trakt_watchlist', ?, ?, 'Trakt watchlist',
            'Items you''ve added to your Trakt watchlist.',
            0, 'mixed', 1, ?)
         RETURNING id",
    )
    .bind(format!("https://trakt.tv/users/{user}/watchlist"))
    .bind(&user)
    .bind(&now)
    .fetch_one(db)
    .await?;
    let _ = event_tx.send(AppEvent::ListAutoAdded {
        list_id,
        title: "Trakt watchlist".into(),
    });
    Ok(())
}

/// Remove the Trakt watchlist system list. Called from the Trakt
/// disconnect path. Items cascade via ON DELETE CASCADE.
pub async fn remove_trakt_watchlist(db: &SqlitePool) -> Result<(), ListsError> {
    sqlx::query("DELETE FROM list WHERE source_type = 'trakt_watchlist' AND is_system = 1")
        .execute(db)
        .await?;
    Ok(())
}

/// Diff the current `list_item` rows against `fetched` and apply
/// inserts / deletes. The `ignored_by_user` flag is preserved across
/// re-insertions of the same `(list_id, tmdb_id, item_type)` tuple.
pub async fn apply_poll(
    db: &SqlitePool,
    list_id: i64,
    fetched: Vec<RawItem>,
) -> Result<ApplyOutcome, ListsError> {
    // Index by (tmdb_id, item_type) for O(n) diff. Also cache the
    // existing poster_path so we only TMDB-lookup for items we'd
    // actually persist it against (new rows, or existing rows where
    // a prior poll came up empty and a retry could fill it in).
    let existing_rows: Vec<(i64, i64, String, bool, Option<String>)> = sqlx::query_as(
        "SELECT id, tmdb_id, item_type, ignored_by_user, poster_path
         FROM list_item WHERE list_id = ?",
    )
    .bind(list_id)
    .fetch_all(db)
    .await?;
    let mut existing_map: std::collections::HashMap<(i64, String), (i64, bool, Option<String>)> =
        std::collections::HashMap::with_capacity(existing_rows.len());
    for (id, tmdb, kind, ignored, poster) in existing_rows {
        existing_map.insert((tmdb, kind), (id, ignored, poster));
    }

    let mut seen: std::collections::HashSet<(i64, String)> =
        std::collections::HashSet::with_capacity(fetched.len());
    let mut added = 0_i64;
    let mut unchanged = 0_i64;

    for it in &fetched {
        let key = (it.tmdb_id, it.item_type.clone());
        seen.insert(key.clone());
        // Poster fetch policy:
        //   - Source-provided poster (TMDB MDBList items) → use as-is.
        //   - New item → TMDB lookup once.
        //   - Existing item with stored poster → reuse, skip TMDB.
        //   - Existing item with NULL poster → retry TMDB lookup once
        //     per sweep (hidden under the is_none check below). Bulk
        //     cases (500-item lists, TMDB rate-limit flap) otherwise
        //     hammered TMDB with 500 sequential GETs every hour.
        let existing_entry = existing_map.get(&key);
        let poster_path = match &it.poster_path {
            Some(p) if !p.is_empty() => Some(p.clone()),
            _ => {
                if let Some((_, _, Some(existing_poster))) = existing_entry
                    && !existing_poster.is_empty()
                {
                    Some(existing_poster.clone())
                } else {
                    fetch_tmdb_poster(db, it.tmdb_id, &it.item_type).await
                }
            }
        };
        if existing_map.contains_key(&key) {
            // Update mutable fields (title/poster can drift upstream).
            // COALESCE preserves existing poster when TMDB lookup fails.
            sqlx::query(
                "UPDATE list_item
                   SET title       = ?,
                       poster_path = COALESCE(?, poster_path),
                       position    = ?
                 WHERE list_id = ? AND tmdb_id = ? AND item_type = ?",
            )
            .bind(&it.title)
            .bind(&poster_path)
            .bind(it.position)
            .bind(list_id)
            .bind(it.tmdb_id)
            .bind(&it.item_type)
            .execute(db)
            .await?;
            unchanged += 1;
        } else {
            sqlx::query(
                "INSERT INTO list_item
                   (list_id, tmdb_id, item_type, title, poster_path, position, added_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(list_id)
            .bind(it.tmdb_id)
            .bind(&it.item_type)
            .bind(&it.title)
            .bind(&poster_path)
            .bind(it.position)
            .bind(&it.added_at)
            .execute(db)
            .await?;
            added += 1;
        }
    }

    // Removed = existing not seen this poll. Spec §Add-never-subtract:
    // drop the list_item row, leave the Movie/Show intact.
    let mut removed = 0_i64;
    for (key, (id, _ignored, _poster)) in &existing_map {
        if !seen.contains(key) {
            sqlx::query("DELETE FROM list_item WHERE id = ?")
                .bind(id)
                .execute(db)
                .await?;
            removed += 1;
        }
    }

    let total = added + unchanged;
    // Preserve the source's declared item_count — over-writing it
    // with `added + unchanged` drops items we couldn't map to a local
    // entity, which makes the UI disagree with the source's own
    // total. The declared count came from `fetch_metadata` at insert
    // time; refresh it here so if the source list grew / shrank, the
    // stamp reflects reality.
    sqlx::query(
        "UPDATE list SET
            last_polled_at            = ?,
            last_poll_status          = 'ok',
            consecutive_poll_failures = 0
         WHERE id = ?",
    )
    .bind(crate::time::Timestamp::now().to_rfc3339())
    .bind(list_id)
    .execute(db)
    .await?;
    let _ = total;

    Ok(ApplyOutcome {
        added,
        removed,
        unchanged,
    })
}

/// Record a poll failure on the `list` row. After 3 consecutive
/// failures the scheduler emits a notification (see `notify.rs`).
pub async fn record_poll_failure(
    db: &SqlitePool,
    list_id: i64,
    reason: &str,
) -> Result<i64, ListsError> {
    sqlx::query(
        "UPDATE list SET
            last_polled_at            = ?,
            last_poll_status          = ?,
            consecutive_poll_failures = consecutive_poll_failures + 1
         WHERE id = ?",
    )
    .bind(crate::time::Timestamp::now().to_rfc3339())
    .bind(format!("error: {reason}"))
    .bind(list_id)
    .execute(db)
    .await?;
    let n: i64 = sqlx::query_scalar("SELECT consecutive_poll_failures FROM list WHERE id = ?")
        .bind(list_id)
        .fetch_one(db)
        .await?;
    Ok(n)
}

/// Scheduled poll sweep: iterate every non-system list whose
/// `last_polled_at` is older than the per-source-type interval and
/// refresh it. Trakt watchlist (system list) is intentionally
/// excluded — it rides along with the existing 5-min
/// `trakt_sync_incremental` sweep when `last_activities` reports the
/// watchlist watermark has moved.
pub async fn poll_due_lists(
    db: &SqlitePool,
    event_tx: &broadcast::Sender<AppEvent>,
) -> Result<(), ListsError> {
    let candidates: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, source_type, last_polled_at FROM list WHERE is_system = 0")
            .fetch_all(db)
            .await?;
    let now = chrono::Utc::now();
    let bulk_threshold = bulk_growth_threshold(db).await;
    for (id, source_type, last_polled_at) in candidates {
        let Some(st) = SourceType::parse(&source_type) else {
            continue;
        };
        let interval = poll_interval(st);
        if let Some(last) = last_polled_at
            && let Ok(parsed_last) = chrono::DateTime::parse_from_rfc3339(&last)
            && now.signed_duration_since(parsed_last.with_timezone(&chrono::Utc)) < interval
        {
            // Not due — skip the entire poll *and* the stagger sleep.
            // The 250 ms sleep below is meant to spread real outbound
            // requests across the sweep; firing it on a skip path
            // wastes wall-clock on a no-op.
            continue;
        }
        match poll_one(db, id).await {
            Ok(outcome) => {
                if outcome.added > bulk_threshold
                    && let Some(title) = list_title(db, id).await
                {
                    let _ = event_tx.send(AppEvent::ListBulkGrowth {
                        list_id: id,
                        title,
                        added: outcome.added,
                    });
                }
            }
            Err(e) => {
                let reason = e.to_string();
                if let Ok(failures) = record_poll_failure(db, id, &reason).await
                    // Emit exactly once on the transition to 3 — later
                    // failures stay silent so we don't spam on every
                    // sweep while a source is stuck.
                    && failures == 3
                    && let Some(title) = list_title(db, id).await
                {
                    let _ = event_tx.send(AppEvent::ListUnreachable {
                        list_id: id,
                        title,
                        reason,
                    });
                }
            }
        }
        // Stagger so 10 lists don't burst out 10 outbound requests at once.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(())
}

async fn list_title(db: &SqlitePool, list_id: i64) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT title FROM list WHERE id = ?")
        .bind(list_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

async fn bulk_growth_threshold(db: &SqlitePool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT list_bulk_growth_threshold FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(20)
}

async fn poll_one(db: &SqlitePool, list_id: i64) -> Result<ApplyOutcome, ListsError> {
    let list: crate::integrations::lists::model::List =
        sqlx::query_as("SELECT * FROM list WHERE id = ?")
            .bind(list_id)
            .fetch_one(db)
            .await?;
    let st = SourceType::parse(&list.source_type)
        .ok_or_else(|| ListsError::Parse(format!("unknown source_type: {}", list.source_type)))?;
    let parsed = ParsedList {
        source_type: st,
        source_id: list.source_id.clone(),
        source_url: list.source_url.clone(),
    };
    let items = fetch_items(db, &parsed).await?;
    apply_poll(db, list_id, items).await
}

fn poll_interval(st: SourceType) -> chrono::Duration {
    match st {
        SourceType::Mdblist | SourceType::TmdbList => chrono::Duration::hours(6),
        SourceType::TraktList => chrono::Duration::hours(1),
        // Trakt watchlist is event-driven via `last_activities` —
        // the periodic sweep never polls it. `Duration::max_value()`
        // gates it effectively forever; the `is_system = 0` filter
        // in `poll_due_lists` already excludes it in practice, but
        // returning an essentially-infinite interval keeps the
        // belt-and-braces story honest.
        SourceType::TraktWatchlist => chrono::Duration::MAX,
    }
}

/// Best-effort TMDB poster lookup. Returns `None` on any failure
/// (no key, HTTP error, not found) — the caller falls back to an
/// empty card rather than blocking the insert.
async fn fetch_tmdb_poster(db: &SqlitePool, tmdb_id: i64, item_type: &str) -> Option<String> {
    let key: String = sqlx::query_scalar("SELECT tmdb_api_key FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .filter(|s: &String| !s.is_empty())?;
    let endpoint = match item_type {
        "movie" => "movie",
        "show" => "tv",
        _ => return None,
    };
    let url = format!("https://api.themoviedb.org/3/{endpoint}/{tmdb_id}");
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&key)
        .header("User-Agent", concat!("kino/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: TmdbPosterOnly = resp.json().await.ok()?;
    body.poster_path.filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn seed_list(db: &SqlitePool) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO list
               (source_type, source_url, source_id, title, description,
                item_count, item_type, is_system, created_at)
             VALUES
               ('tmdb_list', 'https://example.test/list/1', '1', 'Test list',
                NULL, 0, 'mixed', 0, datetime('now'))
             RETURNING id",
        )
        .fetch_one(db)
        .await
        .unwrap()
    }

    fn item(tmdb: i64, kind: &str, title: &str, poster: Option<&str>) -> RawItem {
        RawItem {
            tmdb_id: tmdb,
            item_type: kind.to_owned(),
            title: title.to_owned(),
            poster_path: poster.map(str::to_owned),
            position: Some(tmdb),
            added_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    async fn count_items(db: &SqlitePool, list_id: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM list_item WHERE list_id = ?")
            .bind(list_id)
            .fetch_one(db)
            .await
            .unwrap()
    }

    async fn ignored(db: &SqlitePool, list_id: i64, tmdb: i64) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT ignored_by_user FROM list_item
             WHERE list_id = ? AND tmdb_id = ? AND item_type = 'movie'",
        )
        .bind(list_id)
        .bind(tmdb)
        .fetch_one(db)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn apply_poll_inserts_all_on_empty_list() {
        let pool = db::create_test_pool().await;
        let list_id = seed_list(&pool).await;
        let out = apply_poll(
            &pool,
            list_id,
            vec![
                item(1, "movie", "First", Some("/a.jpg")),
                item(2, "movie", "Second", Some("/b.jpg")),
            ],
        )
        .await
        .unwrap();
        assert_eq!(out.added, 2);
        assert_eq!(out.removed, 0);
        assert_eq!(count_items(&pool, list_id).await, 2);
    }

    #[tokio::test]
    async fn apply_poll_reports_unchanged_when_nothing_moved() {
        let pool = db::create_test_pool().await;
        let list_id = seed_list(&pool).await;
        let items = vec![item(1, "movie", "First", Some("/a.jpg"))];
        apply_poll(&pool, list_id, items.clone()).await.unwrap();
        let out = apply_poll(&pool, list_id, items).await.unwrap();
        assert_eq!(out.added, 0);
        assert_eq!(out.removed, 0);
        assert_eq!(out.unchanged, 1);
    }

    #[tokio::test]
    async fn apply_poll_drops_items_removed_from_source() {
        let pool = db::create_test_pool().await;
        let list_id = seed_list(&pool).await;
        apply_poll(
            &pool,
            list_id,
            vec![
                item(1, "movie", "Keep", Some("/a.jpg")),
                item(2, "movie", "Drop", Some("/b.jpg")),
            ],
        )
        .await
        .unwrap();
        let out = apply_poll(
            &pool,
            list_id,
            vec![item(1, "movie", "Keep", Some("/a.jpg"))],
        )
        .await
        .unwrap();
        assert_eq!(out.removed, 1);
        assert_eq!(count_items(&pool, list_id).await, 1);
    }

    #[tokio::test]
    async fn apply_poll_preserves_ignored_flag_across_re_insert() {
        let pool = db::create_test_pool().await;
        let list_id = seed_list(&pool).await;
        // Insert, set ignored, re-apply the same set.
        apply_poll(
            &pool,
            list_id,
            vec![item(42, "movie", "Ig", Some("/i.jpg"))],
        )
        .await
        .unwrap();
        sqlx::query("UPDATE list_item SET ignored_by_user = 1 WHERE list_id = ? AND tmdb_id = 42")
            .bind(list_id)
            .execute(&pool)
            .await
            .unwrap();
        apply_poll(
            &pool,
            list_id,
            vec![item(42, "movie", "Ig renamed", Some("/i.jpg"))],
        )
        .await
        .unwrap();
        assert!(ignored(&pool, list_id, 42).await);
        // Title updated, but ignored stayed true.
        let title: String =
            sqlx::query_scalar("SELECT title FROM list_item WHERE list_id = ? AND tmdb_id = 42")
                .bind(list_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "Ig renamed");
    }

    #[tokio::test]
    async fn apply_poll_separates_movie_and_show_with_same_tmdb() {
        // TMDB movie + show IDs can collide; the unique key is
        // (list_id, tmdb_id, item_type), so both must coexist.
        let pool = db::create_test_pool().await;
        let list_id = seed_list(&pool).await;
        apply_poll(
            &pool,
            list_id,
            vec![
                item(100, "movie", "Movie 100", Some("/m.jpg")),
                item(100, "show", "Show 100", Some("/s.jpg")),
            ],
        )
        .await
        .unwrap();
        assert_eq!(count_items(&pool, list_id).await, 2);
    }
}

#[derive(serde::Deserialize)]
struct TmdbPosterOnly {
    poster_path: Option<String>,
}
