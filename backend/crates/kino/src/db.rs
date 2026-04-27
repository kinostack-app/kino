use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// Create and configure the `SQLite` connection pool.
///
/// - WAL journal mode for concurrent reads during writes
/// - `busy_timeout` of 5 seconds
/// - Foreign keys enforced
/// - Pool: 1 writer + N readers
pub async fn create_pool(data_path: &str) -> sqlx::Result<SqlitePool> {
    let db_dir = Path::new(data_path);
    tokio::fs::create_dir_all(db_dir)
        .await
        .expect("failed to create data directory");

    let db_path = db_dir.join("kino.db");
    let db_url = format!("sqlite:{}", db_path.display());

    let options = db_url
        .parse::<SqliteConnectOptions>()?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .create_if_missing(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    Ok(pool)
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Run embedded sqlx migrations.
pub async fn run_migrations(pool: &SqlitePool) -> sqlx::Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

#[cfg(test)]
pub async fn create_test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            "sqlite::memory:"
                .parse::<SqliteConnectOptions>()
                .unwrap()
                .journal_mode(SqliteJournalMode::Wal)
                .foreign_keys(true),
        )
        .await
        .expect("failed to create test pool");
    run_migrations(&pool).await.expect("migrations failed");
    pool
}
