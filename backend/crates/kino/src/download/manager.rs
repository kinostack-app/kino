//! Download queue manager — coordinates torrent client, VPN, and database state.

use sqlx::SqlitePool;

use crate::download::DownloadPhase;
use crate::download::model::Download;
use crate::download::torrent_client::TorrentStatus;

/// Central download manager coordinating torrents, VPN, and queue.
#[derive(Debug)]
pub struct DownloadManager {
    db: SqlitePool,
    max_concurrent: i64,
}

impl DownloadManager {
    pub fn new(db: SqlitePool, max_concurrent: i64) -> Self {
        Self { db, max_concurrent }
    }

    /// Get all active downloads (not terminal state).
    pub async fn active_downloads(&self) -> anyhow::Result<Vec<Download>> {
        let downloads = sqlx::query_as::<_, Download>(
            "SELECT * FROM download WHERE state IN ('queued', 'grabbing', 'downloading', 'paused', 'stalled', 'seeding') ORDER BY added_at ASC",
        )
        .fetch_all(&self.db)
        .await?;
        Ok(downloads)
    }

    /// Count currently active (non-queued) downloads.
    pub async fn active_count(&self) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM download WHERE state IN ('downloading', 'grabbing', 'seeding', 'stalled')",
        )
        .fetch_one(&self.db)
        .await?;
        Ok(count)
    }

    /// Update download progress from torrent client status.
    pub async fn update_progress(
        &self,
        download_id: i64,
        status: &TorrentStatus,
    ) -> anyhow::Result<()> {
        let state = match status.state {
            crate::download::torrent_client::TorrentState::Initializing => "grabbing",
            crate::download::torrent_client::TorrentState::Downloading => "downloading",
            crate::download::torrent_client::TorrentState::Seeding => "seeding",
            crate::download::torrent_client::TorrentState::Paused => "paused",
            crate::download::torrent_client::TorrentState::Error => "failed",
        };

        sqlx::query(
            "UPDATE download SET state = ?, downloaded = ?, uploaded = ?, download_speed = ?, upload_speed = ?, seeders = ?, leechers = ?, eta = ? WHERE id = ?",
        )
        .bind(state)
        .bind(status.downloaded)
        .bind(status.uploaded)
        .bind(status.download_speed)
        .bind(status.upload_speed)
        .bind(status.seeders)
        .bind(status.leechers)
        .bind(status.eta_seconds)
        .bind(download_id)
        .execute(&self.db)
        .await?;

        // If completed, set completed_at
        if status.finished {
            let now = crate::time::Timestamp::now().to_rfc3339();
            sqlx::query(
                "UPDATE download SET state = ?, completed_at = ? WHERE id = ? AND state != ?",
            )
            .bind(DownloadPhase::Completed)
            .bind(&now)
            .bind(download_id)
            .bind(DownloadPhase::Completed)
            .execute(&self.db)
            .await?;
        }

        Ok(())
    }

    /// Transition a download to failed state with an error message.
    pub async fn mark_failed(&self, download_id: i64, error: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE download SET state = ?, error_message = ? WHERE id = ?")
            .bind(DownloadPhase::Failed)
            .bind(error)
            .bind(download_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Unpause the next queued download if under the concurrent limit.
    pub async fn maybe_start_next(&self) -> anyhow::Result<Option<Download>> {
        let active = self.active_count().await?;
        if active >= self.max_concurrent {
            return Ok(None);
        }

        // Get the next queued download (oldest first)
        let next = sqlx::query_as::<_, Download>(
            "SELECT * FROM download WHERE state = ? ORDER BY added_at ASC LIMIT 1",
        )
        .bind(DownloadPhase::Queued)
        .fetch_optional(&self.db)
        .await?;

        if let Some(ref dl) = next {
            sqlx::query("UPDATE download SET state = ? WHERE id = ?")
                .bind(DownloadPhase::Grabbing)
                .bind(dl.id)
                .execute(&self.db)
                .await?;
        }

        Ok(next)
    }

    /// Check stall detection for a download.
    ///
    /// Returns the new stall state based on speed and peer count.
    pub fn detect_stall(
        &self,
        download: &Download,
        status: &TorrentStatus,
        stall_timeout_minutes: i64,
        dead_timeout_minutes: i64,
    ) -> StallState {
        if status.download_speed > 0 {
            return StallState::Healthy;
        }

        // No speed — check how long
        let stall_half = stall_timeout_minutes * 30; // half of stall_timeout in seconds
        let stall_full = stall_timeout_minutes * 60;
        let dead_full = dead_timeout_minutes * 60;

        // Calculate time with zero speed (approximate from state)
        let zero_speed_duration =
            if DownloadPhase::parse(&download.state) == Some(DownloadPhase::Stalled) {
                // Already stalled — use duration since state change (approximate with stall_full)
                stall_full
            } else if download.download_speed == 0 {
                // Speed was already 0 last check — accumulating
                stall_half + 1 // past the slow threshold
            } else {
                0 // just dropped to 0
            };

        if zero_speed_duration > dead_full
            || (status.seeders == Some(0) && zero_speed_duration > stall_half)
        {
            StallState::Dead
        } else if zero_speed_duration > stall_half {
            StallState::Stalled
        } else {
            StallState::Slow
        }
    }
}

/// Stall detection result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallState {
    Healthy,
    Slow,
    Stalled,
    Dead,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::torrent_client::TorrentState;

    fn make_status(speed: i64, seeders: Option<i64>, finished: bool) -> TorrentStatus {
        TorrentStatus {
            downloaded: 1_000_000,
            uploaded: 500_000,
            download_speed: speed,
            upload_speed: 0,
            seeders,
            leechers: None,
            eta_seconds: None,
            finished,
            state: if finished {
                TorrentState::Seeding
            } else {
                TorrentState::Downloading
            },
        }
    }

    fn make_download(state: &str, speed: i64) -> Download {
        Download {
            id: 1,
            release_id: None,
            torrent_hash: Some("abc".into()),
            title: "test".into(),
            state: state.into(),
            size: Some(10_000_000),
            downloaded: 1_000_000,
            uploaded: 500_000,
            download_speed: speed,
            upload_speed: 0,
            seeders: Some(10),
            leechers: Some(5),
            eta: None,
            added_at: "2026-01-01T00:00:00Z".into(),
            completed_at: None,
            output_path: None,
            magnet_url: None,
            error_message: None,
        }
    }

    #[tokio::test]
    async fn stall_healthy_when_speed_positive() {
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let mgr = DownloadManager::new(pool, 3);
        let dl = make_download("downloading", 100_000);
        let status = make_status(100_000, Some(10), false);
        assert_eq!(mgr.detect_stall(&dl, &status, 30, 60), StallState::Healthy);
    }

    #[tokio::test]
    async fn stall_slow_when_just_dropped() {
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let mgr = DownloadManager::new(pool, 3);
        let dl = make_download("downloading", 100_000); // was fast before
        let status = make_status(0, Some(10), false);
        assert_eq!(mgr.detect_stall(&dl, &status, 30, 60), StallState::Slow);
    }

    #[tokio::test]
    async fn stall_stalled_when_zero_speed_persists() {
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let mgr = DownloadManager::new(pool, 3);
        let dl = make_download("downloading", 0); // already had 0 speed
        let status = make_status(0, Some(10), false);
        assert_eq!(mgr.detect_stall(&dl, &status, 30, 60), StallState::Stalled);
    }

    #[tokio::test]
    async fn stall_dead_when_no_peers() {
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let mgr = DownloadManager::new(pool, 3);
        let dl = make_download("downloading", 0);
        let status = make_status(0, Some(0), false); // no seeders
        assert_eq!(mgr.detect_stall(&dl, &status, 30, 60), StallState::Dead);
    }
}
