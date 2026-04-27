//! VPN killswitch — soft pause-all on tunnel disconnect, auto-resume
//! on successful reconnect.
//!
//! The hard-firewall layer (nftables) is a separate phase. This module
//! is the process-level safety net: when [`vpn::health::check_once`]
//! detects a stale handshake, it pauses every active download with the
//! `paused_by_killswitch` marker. The next health tick (or the resume
//! call inside `check_once` itself) flips them back when the tunnel is
//! healthy again.
//!
//! Gated on `config.vpn_killswitch_enabled`. When disabled, downloads
//! keep running through the disconnect — `bind_device_name` will fail
//! their peer sockets at the kernel level, but they won't be paused
//! cleanly. The user opted out.

use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::events::{AppEvent, display::download_display_title};

/// Read the killswitch toggle from config. Returns false on read error
/// — fail-open here would defeat the point, but the *caller* still
/// needs the rest of `check_once` to run, so we log and treat absence
/// as "off". Default in the schema is `1`, so a healthy DB never
/// returns false here.
pub async fn is_enabled(pool: &SqlitePool) -> bool {
    match sqlx::query_scalar::<_, bool>("SELECT vpn_killswitch_enabled FROM config WHERE id = 1")
        .fetch_optional(pool)
        .await
    {
        Ok(Some(v)) => v,
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(error = %e, "killswitch toggle read failed; treating as off");
            false
        }
    }
}

/// Pause every download that's actively talking to the network. The
/// scheduler's `vpn_health` task calls this *before* tearing the tunnel
/// down so peer sockets stop attempting reconnects through whatever
/// route the kernel falls back to.
///
/// Idempotent. A row already paused (by the user or a previous
/// killswitch sweep) is left alone — we only touch rows whose state
/// is in the "actively transferring" set.
pub async fn pause_all_active(
    pool: &SqlitePool,
    torrent: Option<&dyn crate::download::session::TorrentSession>,
    event_tx: &broadcast::Sender<AppEvent>,
) -> anyhow::Result<u64> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, title, torrent_hash FROM download
         WHERE state IN ('downloading', 'stalled', 'grabbing')",
    )
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let mut paused = 0u64;
    for (id, title, hash) in rows {
        if let (Some(client), Some(h)) = (torrent, hash.as_deref())
            && let Err(e) = client.pause(h).await
        {
            // Don't bail — the DB flip below is what actually marks
            // the download paused for the rest of the system, and
            // librqbit will lose its peer sockets the moment the
            // tunnel drops anyway. Log so the operator can see if
            // something is genuinely wrong.
            tracing::warn!(
                download_id = id, error = %e,
                "killswitch: librqbit pause failed; flipping DB anyway",
            );
        }
        let res = sqlx::query(
            "UPDATE download
                SET state = 'paused',
                    paused_by_killswitch = 1,
                    download_speed = 0,
                    upload_speed = 0,
                    seeders = NULL,
                    leechers = NULL,
                    eta = NULL
              WHERE id = ?
                AND state IN ('downloading', 'stalled', 'grabbing')",
        )
        .bind(id)
        .execute(pool)
        .await?;
        if res.rows_affected() > 0 {
            paused += 1;
            let display = download_display_title(pool, id, &title).await;
            let _ = event_tx.send(AppEvent::DownloadPaused {
                download_id: id,
                title: display,
            });
        }
    }
    Ok(paused)
}

/// Resume every download that this subsystem paused. Called after a
/// successful reconnect and at the top of every healthy `check_once`
/// pass — the latter handles the "process restarted while paused"
/// recovery case.
pub async fn resume_killswitch_paused(
    pool: &SqlitePool,
    torrent: Option<&dyn crate::download::session::TorrentSession>,
    event_tx: &broadcast::Sender<AppEvent>,
) -> anyhow::Result<u64> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, title, torrent_hash FROM download
         WHERE state = 'paused' AND paused_by_killswitch = 1",
    )
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(0);
    }

    let mut resumed = 0u64;
    for (id, title, hash) in rows {
        if let (Some(client), Some(h)) = (torrent, hash.as_deref())
            && let Err(e) = client.resume(h).await
        {
            // Same rationale as pause: don't bail. The DB flip is
            // authoritative; if librqbit refuses, the next monitor
            // tick will surface the underlying problem.
            tracing::warn!(
                download_id = id, error = %e,
                "killswitch: librqbit resume failed; flipping DB anyway",
            );
        }
        let res = sqlx::query(
            "UPDATE download
                SET state = 'downloading',
                    paused_by_killswitch = 0
              WHERE id = ?
                AND state = 'paused'
                AND paused_by_killswitch = 1",
        )
        .bind(id)
        .execute(pool)
        .await?;
        if res.rows_affected() > 0 {
            resumed += 1;
            let display = download_display_title(pool, id, &title).await;
            let _ = event_tx.send(AppEvent::DownloadResumed {
                download_id: id,
                title: display,
            });
        }
    }
    Ok(resumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn fresh_pool() -> SqlitePool {
        let pool = db::create_test_pool().await;
        crate::init::ensure_defaults(&pool, "/tmp/kino-test")
            .await
            .expect("seed defaults");
        pool
    }

    async fn insert_download(pool: &SqlitePool, state: &str) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        let res = sqlx::query(
            "INSERT INTO download (title, state, added_at, torrent_hash)
             VALUES (?, ?, ?, ?)",
        )
        .bind("Test")
        .bind(state)
        .bind(&now)
        .bind(Some("deadbeef".to_string()))
        .execute(pool)
        .await
        .unwrap();
        res.last_insert_rowid()
    }

    #[tokio::test]
    async fn is_enabled_default_true() {
        let pool = fresh_pool().await;
        assert!(is_enabled(&pool).await);
    }

    #[tokio::test]
    async fn pause_all_flips_active_rows_only() {
        let pool = fresh_pool().await;
        let dl_active = insert_download(&pool, "downloading").await;
        let dl_paused = insert_download(&pool, "paused").await;
        let dl_done = insert_download(&pool, "imported").await;
        let (tx, _rx) = broadcast::channel(16);

        let n = pause_all_active(&pool, None, &tx).await.unwrap();
        assert_eq!(n, 1);

        let active_state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
            .bind(dl_active)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(active_state, "paused");
        let active_marker: bool =
            sqlx::query_scalar("SELECT paused_by_killswitch FROM download WHERE id = ?")
                .bind(dl_active)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(active_marker);

        // Already-paused row unchanged: state stays paused, marker
        // stays 0 (the user paused it themselves; the killswitch
        // doesn't claim ownership of pre-existing pauses).
        let user_marker: bool =
            sqlx::query_scalar("SELECT paused_by_killswitch FROM download WHERE id = ?")
                .bind(dl_paused)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!user_marker);

        let done_state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
            .bind(dl_done)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(done_state, "imported");
    }

    #[tokio::test]
    async fn resume_only_touches_killswitch_marked_rows() {
        let pool = fresh_pool().await;
        // One killswitch-paused, one user-paused.
        let dl_ks = insert_download(&pool, "downloading").await;
        let dl_user = insert_download(&pool, "paused").await;
        let (tx, _rx) = broadcast::channel(16);
        pause_all_active(&pool, None, &tx).await.unwrap();

        let n = resume_killswitch_paused(&pool, None, &tx).await.unwrap();
        assert_eq!(n, 1);

        let ks_state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
            .bind(dl_ks)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(ks_state, "downloading");
        let ks_marker: bool =
            sqlx::query_scalar("SELECT paused_by_killswitch FROM download WHERE id = ?")
                .bind(dl_ks)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!ks_marker);

        let user_state: String = sqlx::query_scalar("SELECT state FROM download WHERE id = ?")
            .bind(dl_user)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(user_state, "paused");
    }

    #[tokio::test]
    async fn pause_then_resume_is_idempotent() {
        let pool = fresh_pool().await;
        insert_download(&pool, "downloading").await;
        let (tx, _rx) = broadcast::channel(16);

        assert_eq!(pause_all_active(&pool, None, &tx).await.unwrap(), 1);
        // Second pause sweep: nothing left in active state.
        assert_eq!(pause_all_active(&pool, None, &tx).await.unwrap(), 0);

        assert_eq!(resume_killswitch_paused(&pool, None, &tx).await.unwrap(), 1);
        // Second resume sweep: nothing left flagged.
        assert_eq!(resume_killswitch_paused(&pool, None, &tx).await.unwrap(), 0);
    }
}
