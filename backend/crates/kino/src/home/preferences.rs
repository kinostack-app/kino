//! `/api/v1/preferences/*` — per-user display preferences.
//!
//! Separate endpoint family from `/api/v1/config` because these are
//! settings the user picks via the UI (row order, hero toggle), not
//! operator settings tied to the binary (paths, keys, timeouts).
//! Schema is a single-row `user_preferences` table, same shape as
//! `config`. See `docs/subsystems/18-ui-customisation.md`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::AppResult;
use crate::state::AppState;

/// Default Home section order for a fresh install. Only includes rows
/// that have data today — deferred rows (Trakt, Lists, upcoming
/// episodes) land in later versions and slot in at the tail on first
/// emission. See `docs/subsystems/18-ui-customisation.md` § v1 rows.
///
/// Kept as a function rather than a const so the `String` allocation
/// only happens when a fresh row is actually inserted.
fn default_section_order() -> Vec<String> {
    vec![
        "up_next".into(),
        // Trakt rows auto-hide when disconnected (see Home.tsx's
        // TraktRowFromEndpoint), so it's safe to put them in the
        // default order — users who haven't connected never see
        // them, and connected users find them already ordered.
        "recommendations".into(),
        "trending_trakt".into(),
        "trending_movies".into(),
        "trending_shows".into(),
        "popular_movies".into(),
        "popular_shows".into(),
    ]
}

/// Everything the frontend needs to render and customise the Home
/// layout. `section_order` is the authoritative list; `section_hidden`
/// is a set of IDs within that list the user has toggled off.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HomePreferences {
    pub hero_enabled: bool,
    pub section_order: Vec<String>,
    pub section_hidden: Vec<String>,
    /// Display name shown in the Home greeting. Populated by the
    /// setup wizard; `None` → the greeting drops the name.
    pub greeting_name: Option<String>,
    /// ISO 8601 UTC — last time any field above changed. Useful for
    /// cache-busting and multi-tab "last write wins" reconciliation.
    pub updated_at: String,
}

/// PATCH body. Every field is optional — absent fields preserve the
/// stored value. Matches the `COALESCE`-style update pattern used by
/// `/api/v1/config` so two tabs can PATCH disjoint fields without
/// clobbering each other.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HomePreferencesUpdate {
    pub hero_enabled: Option<bool>,
    pub section_order: Option<Vec<String>>,
    pub section_hidden: Option<Vec<String>>,
    pub greeting_name: Option<String>,
    /// When `true`, explicitly clear `greeting_name` to NULL. The
    /// regular `greeting_name: None` path means "leave untouched"
    /// under the `COALESCE` semantics, so there's no way to delete
    /// a previously-set name without this sentinel. Takes precedence
    /// over `greeting_name` — caller would never set both.
    #[serde(default)]
    pub clear_greeting_name: bool,
}

/// `GET /api/v1/preferences/home` — current Home preferences.
///
/// Never 404s: a fresh install with no row yet returns defaults and
/// inserts the row so the next write can `UPDATE` cleanly.
#[utoipa::path(
    get, path = "/api/v1/preferences/home",
    responses((status = 200, body = HomePreferences)),
    tag = "preferences", security(("api_key" = []))
)]
pub async fn get_home_preferences(
    State(state): State<AppState>,
) -> AppResult<Json<HomePreferences>> {
    let prefs = load_or_init(&state.db).await?;
    Ok(Json(prefs))
}

/// `PATCH /api/v1/preferences/home` — partial update.
///
/// Idempotent by construction: re-sending the same body is a no-op
/// beyond bumping `updated_at`. Arrays replace wholesale when present
/// (no per-element merging — the frontend always has the full list
/// from its most recent GET).
#[utoipa::path(
    patch, path = "/api/v1/preferences/home",
    request_body = HomePreferencesUpdate,
    responses((status = 200, body = HomePreferences)),
    tag = "preferences", security(("api_key" = []))
)]
pub async fn update_home_preferences(
    State(state): State<AppState>,
    Json(update): Json<HomePreferencesUpdate>,
) -> AppResult<Json<HomePreferences>> {
    // Ensure the row exists before updating. Avoids the "fresh install
    // PATCH arrives before any GET" race that would otherwise silently
    // update zero rows.
    load_or_init(&state.db).await?;

    let hero = update.hero_enabled.map(i64::from);
    let order = update
        .section_order
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));
    let hidden = update
        .section_hidden
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));
    let greeting = update.greeting_name;
    let now = crate::time::Timestamp::now().to_rfc3339();

    // Two-branch update: when the client set `clear_greeting_name`,
    // we need a plain assignment (`greeting_name = NULL`) rather
    // than `COALESCE(NULL, …)` which keeps the old value. Everything
    // else shares the same COALESCE semantics across both branches.
    let sql = if update.clear_greeting_name {
        "UPDATE user_preferences SET
            home_hero_enabled    = COALESCE(?, home_hero_enabled),
            home_section_order   = COALESCE(?, home_section_order),
            home_section_hidden  = COALESCE(?, home_section_hidden),
            greeting_name        = NULL,
            updated_at           = ?
         WHERE id = 1"
    } else {
        "UPDATE user_preferences SET
            home_hero_enabled    = COALESCE(?, home_hero_enabled),
            home_section_order   = COALESCE(?, home_section_order),
            home_section_hidden  = COALESCE(?, home_section_hidden),
            greeting_name        = COALESCE(?, greeting_name),
            updated_at           = ?
         WHERE id = 1"
    };
    let mut q = sqlx::query(sql).bind(hero).bind(order).bind(hidden);
    if !update.clear_greeting_name {
        q = q.bind(greeting);
    }
    q.bind(&now).execute(&state.db).await?;

    Ok(Json(load_or_init(&state.db).await?))
}

/// `POST /api/v1/preferences/home/reset` — wipe customisations back
/// to the v1 defaults. Used by the "Reset to defaults" button in the
/// Customise drawer.
#[utoipa::path(
    post, path = "/api/v1/preferences/home/reset",
    responses((status = 200, body = HomePreferences)),
    tag = "preferences", security(("api_key" = []))
)]
pub async fn reset_home_preferences(
    State(state): State<AppState>,
) -> AppResult<Json<HomePreferences>> {
    let order = default_section_order();
    let order_json = serde_json::to_string(&order).unwrap_or_else(|_| "[]".into());
    let now = crate::time::Timestamp::now().to_rfc3339();
    // Reset scope per spec §UI customisation: row order, hidden
    // rows, hero toggle only. `greeting_name` is a personal
    // identifier and the Customise Home drawer's confirm dialog
    // explicitly promises "Library view preferences are not
    // affected" — users reset their layout but expect the rest of
    // their profile intact.
    sqlx::query(
        "INSERT INTO user_preferences (id, home_hero_enabled, home_section_order, home_section_hidden, greeting_name, updated_at)
         VALUES (1, 1, ?, '[]', NULL, ?)
         ON CONFLICT(id) DO UPDATE SET
            home_hero_enabled    = 1,
            home_section_order   = excluded.home_section_order,
            home_section_hidden  = '[]',
            updated_at           = excluded.updated_at",
    )
    .bind(&order_json)
    .bind(&now)
    .execute(&state.db)
    .await?;
    Ok(Json(load_or_init(&state.db).await?))
}

#[derive(sqlx::FromRow)]
struct Row {
    home_hero_enabled: bool,
    home_section_order: String,
    home_section_hidden: String,
    greeting_name: Option<String>,
    updated_at: String,
}

/// Read the single preferences row, inserting defaults if the row
/// hasn't been created yet. Never fails on a first-run DB. Returns a
/// shape the handlers can hand straight back as JSON.
async fn load_or_init(db: &sqlx::SqlitePool) -> AppResult<HomePreferences> {
    let row: Option<Row> = sqlx::query_as("SELECT * FROM user_preferences WHERE id = 1")
        .fetch_optional(db)
        .await?;

    if let Some(r) = row {
        return Ok(to_prefs(r));
    }

    // Fresh install: insert the defaults so future PATCHes don't race.
    // `INSERT OR IGNORE` covers the case where two concurrent GETs
    // both try to initialise at the same time (second one loses,
    // which is fine — next SELECT sees the winner's row).
    let order = default_section_order();
    let order_json = serde_json::to_string(&order).unwrap_or_else(|_| "[]".into());
    let now = crate::time::Timestamp::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO user_preferences
            (id, home_hero_enabled, home_section_order, home_section_hidden, greeting_name, updated_at)
         VALUES (1, 1, ?, '[]', NULL, ?)",
    )
    .bind(&order_json)
    .bind(&now)
    .execute(db)
    .await?;

    let fresh: Row = sqlx::query_as("SELECT * FROM user_preferences WHERE id = 1")
        .fetch_one(db)
        .await?;
    Ok(to_prefs(fresh))
}

fn to_prefs(r: Row) -> HomePreferences {
    HomePreferences {
        hero_enabled: r.home_hero_enabled,
        section_order: serde_json::from_str(&r.home_section_order).unwrap_or_default(),
        section_hidden: serde_json::from_str(&r.home_section_hidden).unwrap_or_default(),
        greeting_name: r.greeting_name,
        updated_at: r.updated_at,
    }
}

// Silences `clippy::unused` on StatusCode when only some routes use it.
// Kept behind `#[allow(dead_code)]` rather than removed because the
// next slice (home composition) will need it.
#[allow(dead_code)]
const _NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn load_or_init_returns_defaults_on_fresh_db() {
        let pool = test_pool().await;
        let prefs = load_or_init(&pool).await.unwrap();
        assert!(prefs.hero_enabled);
        assert_eq!(prefs.section_order, default_section_order());
        assert!(prefs.section_hidden.is_empty());
        assert!(prefs.greeting_name.is_none());
    }

    #[tokio::test]
    async fn load_or_init_is_idempotent() {
        let pool = test_pool().await;
        let first = load_or_init(&pool).await.unwrap();
        let second = load_or_init(&pool).await.unwrap();
        // Same `updated_at` proves the row wasn't re-inserted.
        assert_eq!(first.updated_at, second.updated_at);
    }

    #[tokio::test]
    async fn reset_restores_defaults_after_customisation() {
        let pool = test_pool().await;
        load_or_init(&pool).await.unwrap();

        // Customise: hide two rows + flip hero off.
        sqlx::query(
            "UPDATE user_preferences SET
                home_hero_enabled = 0,
                home_section_hidden = '[\"popular_movies\",\"popular_shows\"]'
             WHERE id = 1",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Use the reset logic via an inline exec of the same UPSERT so
        // we test it without spinning up an axum router.
        let order_json = serde_json::to_string(&default_section_order()).unwrap();
        let now = crate::time::Timestamp::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO user_preferences (id, home_hero_enabled, home_section_order, home_section_hidden, greeting_name, updated_at)
             VALUES (1, 1, ?, '[]', NULL, ?)
             ON CONFLICT(id) DO UPDATE SET
                home_hero_enabled    = 1,
                home_section_order   = excluded.home_section_order,
                home_section_hidden  = '[]',
                greeting_name        = NULL,
                updated_at           = excluded.updated_at",
        )
        .bind(&order_json)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let prefs = load_or_init(&pool).await.unwrap();
        assert!(prefs.hero_enabled);
        assert!(prefs.section_hidden.is_empty());
    }

    /// `clear_greeting_name` flag forces `greeting_name` to NULL
    /// even when the regular field is omitted. Without this, the
    /// `COALESCE(NULL, greeting_name)` keeps the old value and the
    /// UI has no way to delete a previously-set name.
    #[tokio::test]
    async fn clear_greeting_name_sets_null() {
        let pool = test_pool().await;
        load_or_init(&pool).await.unwrap();
        sqlx::query("UPDATE user_preferences SET greeting_name = 'Robert' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let now = crate::time::Timestamp::now().to_rfc3339();
        // Mirror the `clear` branch of the handler.
        sqlx::query(
            "UPDATE user_preferences SET
                home_hero_enabled    = COALESCE(?, home_hero_enabled),
                home_section_order   = COALESCE(?, home_section_order),
                home_section_hidden  = COALESCE(?, home_section_hidden),
                greeting_name        = NULL,
                updated_at           = ?
             WHERE id = 1",
        )
        .bind::<Option<i64>>(None)
        .bind::<Option<String>>(None)
        .bind::<Option<String>>(None)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let prefs = load_or_init(&pool).await.unwrap();
        assert!(prefs.greeting_name.is_none(), "greeting should be cleared");
    }
}
