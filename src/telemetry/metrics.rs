//! Aggregate metrics collected from the database for the telemetry bundle.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};

/// A generic "key → count" pair used across metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountPair {
    pub key: String,
    pub count: i64,
}

/// Aggregated, non-sensitive metrics derived from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub task_history_total: i64,
    pub task_counts_by_status: Vec<CountPair>,
    pub task_counts_by_type: Vec<CountPair>,
    pub failed_tasks_24h: i64,
    pub table_row_counts: Vec<CountPair>,
}

pub async fn collect_metrics(pool: &Pool<Sqlite>) -> Result<Metrics> {
    let task_history_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_history")
        .fetch_one(pool)
        .await?;

    let task_counts_by_status = grouped_counts(
        pool,
        "SELECT status, COUNT(*) AS n FROM task_history GROUP BY status ORDER BY n DESC",
    )
    .await?;

    let task_counts_by_type = grouped_counts(
        pool,
        "SELECT task_type, COUNT(*) AS n FROM task_history GROUP BY task_type ORDER BY n DESC",
    )
    .await?;

    let cutoff = chrono::Utc::now().timestamp() - 86400;
    let failed_tasks_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_history WHERE status = 'failed' AND completed_at >= ?",
    )
    .bind(cutoff)
    .fetch_one(pool)
    .await?;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    let mut table_row_counts = Vec::new();
    for name in tables {
        // Only count our own snake_case tables (guards against quoting issues).
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let sql = format!("SELECT COUNT(*) FROM \"{name}\"");
        let count: i64 = sqlx::query_scalar(&sql).fetch_one(pool).await.unwrap_or(0);
        table_row_counts.push(CountPair { key: name, count });
    }

    Ok(Metrics {
        task_history_total,
        task_counts_by_status,
        task_counts_by_type,
        failed_tasks_24h,
        table_row_counts,
    })
}

async fn grouped_counts(pool: &Pool<Sqlite>, sql: &str) -> Result<Vec<CountPair>> {
    let rows = sqlx::query(sql).fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| CountPair {
            key: r.get::<String, _>(0),
            count: r.get::<i64, _>(1),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    #[tokio::test]
    async fn collect_metrics_counts_tasks_and_tables() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE task_history (
                id TEXT PRIMARY KEY,
                task_type TEXT NOT NULL,
                status TEXT NOT NULL,
                service TEXT,
                result_summary TEXT,
                error_message TEXT,
                started_at INTEGER,
                completed_at INTEGER,
                created_at_secs REAL NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO task_history (id, task_type, status, completed_at, created_at_secs)
             VALUES ('1', 'ServiceSync', 'completed', ?, 1.0)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_history (id, task_type, status, completed_at, created_at_secs)
             VALUES ('2', 'ScanFolder', 'failed', ?, 2.0)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let m = collect_metrics(&pool).await.unwrap();
        assert_eq!(m.task_history_total, 2);
        assert_eq!(m.failed_tasks_24h, 1);

        let statuses: Vec<(&str, i64)> = m
            .task_counts_by_status
            .iter()
            .map(|p| (p.key.as_str(), p.count))
            .collect();
        assert!(statuses.contains(&("completed", 1)));
        assert!(statuses.contains(&("failed", 1)));

        let types: Vec<(&str, i64)> = m
            .task_counts_by_type
            .iter()
            .map(|p| (p.key.as_str(), p.count))
            .collect();
        assert!(types.contains(&("ServiceSync", 1)));
        assert!(types.contains(&("ScanFolder", 1)));

        let table_counts: Vec<(&str, i64)> = m
            .table_row_counts
            .iter()
            .map(|p| (p.key.as_str(), p.count))
            .collect();
        assert!(table_counts.contains(&("task_history", 2)));
    }
}
