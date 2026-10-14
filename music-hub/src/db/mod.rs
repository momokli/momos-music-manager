pub mod testing;

use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

/// Open (creating if needed) the hub DB, enable WAL + FKs, and run migrations.
pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(15))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    // One-time backfill of artist/album entities from the historical free-text
    // `hub_tracks.artists` / `hub_tracks.album` columns (plan P0). Runs only when
    // `hub_artists` is still empty, so it is a no-op on every later boot.
    crate::artists::backfill_artist_entities_if_empty(&pool).await?;
    Ok(pool)
}

/// A separate read-only pool for the SQL console (`mode=ro` + `query_only`).
pub async fn connect_readonly(database_url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .read_only(true)
        .busy_timeout(Duration::from_secs(15));

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await?;

    sqlx::query("PRAGMA query_only = 1").execute(&pool).await?;
    Ok(pool)
}
