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
    backfill_tag_types(&pool).await?;
    // NOTE: the artist/album entity backfill deliberately does **not** run here.
    // It is far too slow to sit in the boot path (per-row statements on a 78k-track
    // DB blocked startup for ~15 min). Run it explicitly via
    // `music-hub backfill-artists` (batched + transactional + resumable).
    Ok(pool)
}

/// One-time backfill: seed default tag types for all existing users.
/// Controlled by hub_settings.tag_types_backfill_done (global flag).
async fn backfill_tag_types(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM hub_settings WHERE key = 'tag_types_backfill_done'")
            .fetch_optional(pool)
            .await?;
    if done.as_deref() == Some("1") {
        return Ok(());
    }
    let user_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM hub_users")
        .fetch_all(pool)
        .await?;
    for uid in user_ids {
        crate::tagtypes::seed_types(pool, uid).await?;
    }
    sqlx::query(
        "UPDATE hub_settings SET value = '1', updated_at = ?1 WHERE key = 'tag_types_backfill_done'"
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
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
