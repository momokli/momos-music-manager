//! Database connection and initialization.

use anyhow::Result;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::str::FromStr;
use std::time::Duration;

pub async fn connect_db() -> Result<SqlitePool> {
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:app.db".to_string());
    let options = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(30))
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePool::connect_with(options).await?;
    Ok(pool)
}

pub async fn init_db(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!().run(pool).await?;

    // Post-migration schema fixes.
    // Migration 016 was a no-op (the ALTER TABLE was run manually on the
    // production DB). Fresh DBs still have tags.followed and need the rename.
    ensure_backpack_column(pool).await?;

    Ok(())
}

/// Ensure the tags table has a `backpack` column instead of `followed`.
/// Idempotent — safe for both production DBs (already have backpack) and
/// fresh DBs (still have followed).
pub async fn ensure_backpack_column(pool: &SqlitePool) -> Result<()> {
    let has_backpack: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('tags') WHERE name = 'backpack'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if has_backpack {
        tracing::debug!("tags.backpack column already exists");
        // The column was renamed in a previous run, but the view may still be stale.
        // Check and recreate the view if it's missing the backpack column.
        let view_has_backpack: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('v_tags_with_categories') WHERE name = 'backpack'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false);
        if !view_has_backpack {
            tracing::info!("Recreating v_tags_with_categories to include backpack column");
            recreate_tags_view(pool).await?;
        }
        return Ok(());
    }

    let has_followed: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('tags') WHERE name = 'followed'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if has_followed {
        tracing::info!("Renaming tags.followed → tags.backpack (fresh DB)");
        sqlx::query("ALTER TABLE tags RENAME COLUMN followed TO backpack")
            .execute(pool)
            .await?;
        tracing::info!("tags.backpack rename complete");
        recreate_tags_view(pool).await?;
    } else {
        tracing::debug!("Neither tags.followed nor tags.backpack found — adding backpack column");
        sqlx::query("ALTER TABLE tags ADD COLUMN backpack BOOLEAN NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
    }

    Ok(())
}

/// Recreate v_tags_with_categories to include the backpack column.
/// Needed after the followed→backpack rename, since SQLite views don't auto-update
/// when the underlying table columns are renamed.
async fn recreate_tags_view(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "DROP VIEW IF EXISTS v_tags_with_categories; \
         CREATE VIEW v_tags_with_categories AS \
         SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at, t.reviewed_at, t.backpack, \
                tc.name as category, tc.icon as category_icon \
         FROM tags t \
         LEFT JOIN tag_categories tc ON t.category_id = tc.id",
    )
    .execute(pool)
    .await?;
    tracing::info!("v_tags_with_categories recreated with backpack column");
    Ok(())
}
