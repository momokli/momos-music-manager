//! The unified priority work-queue.
//!
//! One persistent table, `work_items`, drives **all** background work in the
//! service. Each row is a unit of work with a `kind`, a `ref` (what it acts on)
//! and a `priority`. Workers pull from it with [`next`], which is the single
//! place priority is applied: **higher priority first, then oldest**.
//!
//! ```text
//! kind                     ref                                   produced by
//! ───────────────────────  ────────────────────────────────────  ──────────────────
//! download                 <isrc | source:id>                    touch / catalog
//! transcode                <isrc>                                (planned)
//! analyze                  <isrc>                                (planned)
//! crawl:<source>           <source>:artist:<id> (or album/track)  leads / touch
//! ```
//!
//! Priority is a *dial*, not a decision: the hub's user actions set it
//! (in-catalog/related = 0, liked/playlist ≥ 1, +1 per playlist, currently
//! playing = +100) and every kind reads the same scale, so playing a brand-new
//! artist lifts *that artist's* crawl leads as well as its tracks.
//!
//! ## Lifecycle
//!
//! `queued → running → done | failed`. A row is claimed ([`claim`], which also
//! increments `attempts`) before it is worked on, so two workers never run the
//! same item. Transient failures re-queue via [`retry_later`] with an
//! exponential backoff until `max_attempts`, then go terminal `failed`.
//!
//! Re-enqueuing a `ref` **raises** its priority (never lowers it) and re-arms a
//! terminal row — touching something again means "do it, sooner".

use anyhow::Result;
use serde::Serialize;
use sqlx::{Pool, Sqlite};

use crate::db;

/// The work kinds. `download`, `transcode` and `analyze` are singletons; crawl
/// leads are parameterised per source via [`crawl`].
pub mod kind {
    pub const DOWNLOAD: &str = "download";
    pub const TRANSCODE: &str = "transcode";
    pub const ANALYZE: &str = "analyze";

    /// A crawl **lead** for one source, e.g. `crawl:deezer`. A lead means
    /// *"complete this entity via that source"* — the spider is just a queue of
    /// these.
    pub fn crawl(source: &str) -> String {
        format!("crawl:{}", source.trim().to_lowercase())
    }
}

/// Item lifecycle states.
pub mod state {
    pub const QUEUED: &str = "queued";
    pub const RUNNING: &str = "running";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
}

/// Default bounded retries for a work item.
pub const MAX_ATTEMPTS: i64 = 5;

/// One queue row.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub id: i64,
    pub kind: String,
    #[sqlx(rename = "ref")]
    #[serde(rename = "ref")]
    pub reference: String,
    pub priority: i64,
    pub state: String,
    pub attempts: i64,
    pub max_attempts: i64,
    /// Earliest epoch at which a `queued` row becomes due (backoff/TTL).
    pub not_before: Option<i64>,
    /// Which source/analyser produced this item (`deezer`, `spotify`, `hub`, …).
    pub record_source: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One `(kind, state) → count` bucket.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CountRow {
    pub kind: String,
    pub state: String,
    pub count: i64,
}

/// Enqueue a work item, or raise/re-arm an existing one.
///
/// On conflict the incoming priority only ever *raises* the stored one, and a
/// terminal (`done`/`failed`) row is reset to `queued` with a cleared backoff —
/// so re-touching something re-arms it.
pub async fn enqueue(
    pool: &Pool<Sqlite>,
    kind: &str,
    reference: &str,
    priority: i64,
    record_source: Option<&str>,
) -> Result<()> {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO work_items
             (kind, ref, priority, state, attempts, record_source, created_at, updated_at)
         VALUES (?, ?, ?, 'queued', 0, ?, ?, ?)
         ON CONFLICT(kind, ref) DO UPDATE SET
             priority   = MAX(priority, excluded.priority),
             state      = CASE WHEN state IN ('done','failed') THEN 'queued' ELSE state END,
             attempts   = CASE WHEN state IN ('done','failed') THEN 0 ELSE attempts END,
             error      = CASE WHEN state IN ('done','failed') THEN NULL ELSE error END,
             not_before = CASE WHEN state IN ('done','failed') THEN NULL ELSE not_before END,
             updated_at = excluded.updated_at",
    )
    .bind(kind)
    .bind(reference)
    .bind(priority)
    .bind(record_source)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await?;
    Ok(())
}

/// Enqueue only when the `(kind, ref)` is not present — never re-arms a terminal
/// row. Used by reconcilers that seed work from existing state on every tick, so
/// a permanently failed item is not resurrected in a loop. Returns whether a new
/// row was inserted.
pub async fn enqueue_if_absent(
    pool: &Pool<Sqlite>,
    kind: &str,
    reference: &str,
    priority: i64,
    record_source: Option<&str>,
) -> Result<bool> {
    let ts = db::now();
    let n = sqlx::query(
        "INSERT INTO work_items
             (kind, ref, priority, state, attempts, record_source, created_at, updated_at)
         VALUES (?, ?, ?, 'queued', 0, ?, ?, ?)
         ON CONFLICT(kind, ref) DO NOTHING",
    )
    .bind(kind)
    .bind(reference)
    .bind(priority)
    .bind(record_source)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// The `next` function: the next `slots` due items of `kind`, **highest
/// priority first, then oldest**. This is the only place priority decides which
/// work runs. Served by `idx_work_next`.
pub async fn next(pool: &Pool<Sqlite>, kind: &str, slots: i64, now: i64) -> Result<Vec<WorkItem>> {
    if slots <= 0 {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, WorkItem>(
        "SELECT * FROM work_items
          WHERE kind = ? AND state = 'queued'
            AND (not_before IS NULL OR not_before <= ?)
          ORDER BY priority DESC, created_at ASC
          LIMIT ?",
    )
    .bind(kind)
    .bind(now)
    .bind(slots)
    .fetch_all(pool)
    .await?)
}

/// Claim a `queued` item: move it to `running` and count the attempt. Returns
/// `false` when it was already claimed (or is no longer queued), so two workers
/// never run the same item.
pub async fn claim(pool: &Pool<Sqlite>, id: i64, now: i64) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE work_items
            SET state = 'running', attempts = attempts + 1, updated_at = ?
          WHERE id = ? AND state = 'queued'",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// Mark an item done.
pub async fn finish(pool: &Pool<Sqlite>, id: i64, now: i64) -> Result<()> {
    sqlx::query(
        "UPDATE work_items
            SET state = 'done', error = NULL, not_before = NULL, updated_at = ?
          WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an item permanently failed.
pub async fn fail(pool: &Pool<Sqlite>, id: i64, error: &str, now: i64) -> Result<()> {
    sqlx::query("UPDATE work_items SET state = 'failed', error = ?, updated_at = ? WHERE id = ?")
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Re-queue a claimed item after a transient failure, or fail it once
/// `max_attempts` is reached. Returns `true` while it is still retryable.
pub async fn retry_later(
    pool: &Pool<Sqlite>,
    id: i64,
    error: &str,
    not_before: i64,
    now: i64,
) -> Result<bool> {
    let row: Option<(i64, i64)> =
        sqlx::query_as("SELECT attempts, max_attempts FROM work_items WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((attempts, max_attempts)) = row else {
        return Ok(false);
    };
    if attempts >= max_attempts {
        fail(pool, id, error, now).await?;
        return Ok(false);
    }
    sqlx::query(
        "UPDATE work_items
            SET state = 'queued', error = ?, not_before = ?, updated_at = ?
          WHERE id = ?",
    )
    .bind(error)
    .bind(not_before)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(true)
}

/// `(kind, ref)`-keyed companions used by the download ledger's transitions
/// (`mark_downloading`/`mark_ready`/`mark_terminal`/`schedule_retry`) to keep the
/// queue item in lockstep without threading an id around.
///
/// Mark a `(kind, ref)` running, creating it if needed. Called when a download is
/// dispatched, so the in-flight ledger and the queue agree.
pub async fn ensure_running_ref(
    pool: &Pool<Sqlite>,
    kind: &str,
    reference: &str,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO work_items (kind, ref, state, attempts, created_at, updated_at)
         VALUES (?, ?, 'running', 0, ?, ?)
         ON CONFLICT(kind, ref) DO UPDATE SET state = 'running', updated_at = excluded.updated_at",
    )
    .bind(kind)
    .bind(reference)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a `(kind, ref)` done (no-op when it does not exist).
pub async fn finish_ref(pool: &Pool<Sqlite>, kind: &str, reference: &str, now: i64) -> Result<()> {
    sqlx::query(
        "UPDATE work_items SET state = 'done', error = NULL, not_before = NULL, updated_at = ?
          WHERE kind = ? AND ref = ?",
    )
    .bind(now)
    .bind(kind)
    .bind(reference)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a `(kind, ref)` permanently failed.
pub async fn fail_ref(
    pool: &Pool<Sqlite>,
    kind: &str,
    reference: &str,
    error: &str,
    now: i64,
) -> Result<()> {
    sqlx::query("UPDATE work_items SET state = 'failed', error = ?, updated_at = ? WHERE kind = ? AND ref = ?")
        .bind(error)
        .bind(now)
        .bind(kind)
        .bind(reference)
        .execute(pool)
        .await?;
    Ok(())
}

/// Re-queue a `(kind, ref)` with a backoff, or fail it out at `max_attempts`.
pub async fn retry_ref(
    pool: &Pool<Sqlite>,
    kind: &str,
    reference: &str,
    error: &str,
    not_before: i64,
    now: i64,
) -> Result<()> {
    let row: Option<(i64, i64)> =
        sqlx::query_as("SELECT attempts, max_attempts FROM work_items WHERE kind = ? AND ref = ?")
            .bind(kind)
            .bind(reference)
            .fetch_optional(pool)
            .await?;
    match row {
        Some((attempts, max)) if attempts >= max => {
            fail_ref(pool, kind, reference, error, now).await?;
        }
        Some(_) => {
            sqlx::query(
                "UPDATE work_items SET state = 'queued', error = ?, not_before = ?, updated_at = ?
                  WHERE kind = ? AND ref = ?",
            )
            .bind(error)
            .bind(not_before)
            .bind(now)
            .bind(kind)
            .bind(reference)
            .execute(pool)
            .await?;
        }
        None => enqueue(pool, kind, reference, 0, None).await?,
    }
    Ok(())
}

/// Re-queue items stranded in `running` (a worker that died mid-flight). Items
/// whose `updated_at` is older than `stale_secs` go back to `queued`; the next
/// claim counts another attempt, so a poison item still fails out. Returns how
/// many were recovered.
pub async fn recover_stale(pool: &Pool<Sqlite>, now: i64, stale_secs: i64) -> Result<usize> {
    let n = sqlx::query(
        "UPDATE work_items
            SET state = 'queued', not_before = NULL, updated_at = ?
          WHERE state = 'running' AND updated_at <= ?",
    )
    .bind(now)
    .bind(now - stale_secs)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n as usize)
}

/// How many items of `kind` are currently `running` — the in-flight set a
/// bounded dispatcher caps against `MAX_INFLIGHT`.
pub async fn count_running(pool: &Pool<Sqlite>, kind: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM work_items WHERE kind = ? AND state = 'running'")
            .bind(kind)
            .fetch_one(pool)
            .await?,
    )
}

/// Free dispatch slots for `kind`, never negative.
pub async fn free_slots(pool: &Pool<Sqlite>, kind: &str, max_inflight: i64) -> Result<i64> {
    let running = count_running(pool, kind).await?;
    Ok((max_inflight - running).max(0))
}

/// `(kind, state) → count` buckets, for the queue overview.
pub async fn counts(pool: &Pool<Sqlite>) -> Result<Vec<CountRow>> {
    Ok(sqlx::query_as::<_, CountRow>(
        "SELECT kind, state, COUNT(*) AS count FROM work_items
          GROUP BY kind, state
          ORDER BY kind, state",
    )
    .fetch_all(pool)
    .await?)
}

/// List items, optionally filtered to one `kind`, highest priority first.
pub async fn items(pool: &Pool<Sqlite>, kind: Option<&str>, limit: i64) -> Result<Vec<WorkItem>> {
    let limit = limit.clamp(1, 500);
    match kind {
        Some(k) => Ok(sqlx::query_as::<_, WorkItem>(
            "SELECT * FROM work_items WHERE kind = ?
              ORDER BY (state = 'running') DESC, priority DESC, created_at ASC
              LIMIT ?",
        )
        .bind(k)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as::<_, WorkItem>(
            "SELECT * FROM work_items
              ORDER BY (state = 'running') DESC, priority DESC, created_at ASC
              LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crawl_kind_normalises_the_source() {
        assert_eq!(kind::crawl("Deezer"), "crawl:deezer");
        assert_eq!(kind::crawl("  Spotify "), "crawl:spotify");
    }
}
