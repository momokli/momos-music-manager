//! SQLite storage: schema + queries.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{Pool, Sqlite};

use crate::models::{state, Order, OrderItem, Track};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Normalise an ISRC: strip hyphens/spaces, uppercase. Deezer expects the bare
/// 12-character form (`USQX91201487`), consumers often send it hyphenated.
pub fn normalize_isrc(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

/// Normalise a track key. A bare ISRC is upper-cased/alphanumeric-normalised;
/// a composite `<source>:<id>` key (e.g. `soundcloud:1837565340`) keeps its
/// source prefix and trims the id.
pub fn normalize_key(raw: &str) -> String {
    let raw = raw.trim();
    match raw.split_once(':') {
        Some((s, id)) => format!("{}:{}", s.trim().to_lowercase(), id.trim()),
        None => normalize_isrc(raw),
    }
}

pub async fn init(pool: &Pool<Sqlite>) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tracks (
            isrc           TEXT PRIMARY KEY,
            deezer_id      TEXT,
            title          TEXT,
            artist         TEXT,
            album          TEXT,
            state          TEXT NOT NULL,
            source_format  TEXT,
            deemix_uuid    TEXT,
            path_flac      TEXT,
            path_320       TEXT,
            path_128       TEXT,
            error          TEXT,
            created_at     INTEGER NOT NULL,
            updated_at     INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS orders (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS order_items (
            order_id TEXT NOT NULL,
            isrc     TEXT NOT NULL,
            PRIMARY KEY (order_id, isrc)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Artist orders (additive): a whole-Deezer-artist download. The operational
    // `tracks` ledger is filled by the ingest once deemix finishes.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS artist_orders (
            id          TEXT PRIMARY KEY,
            artist      TEXT NOT NULL,
            artist_url  TEXT NOT NULL,
            status      TEXT NOT NULL,
            priority    INTEGER NOT NULL DEFAULT 0,
            deemix_uuid TEXT,
            added       INTEGER NOT NULL DEFAULT 0,
            error       TEXT,
            created_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS artist_order_items (
            order_id TEXT NOT NULL,
            isrc     TEXT NOT NULL,
            PRIMARY KEY (order_id, isrc)
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_artist_orders_status ON artist_orders(status);")
        .execute(pool)
        .await?;

    // Per-ISRC metadata (BPM/key authority). Non-null fields win on merge.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS track_metadata (
            isrc        TEXT PRIMARY KEY,
            bpm         REAL,
            musical_key TEXT,
            camelot     TEXT,
            title       TEXT,
            artist      TEXT,
            album       TEXT,
            genre       TEXT,
            source      TEXT,
            updated_at  INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_state ON tracks(state);")
        .execute(pool)
        .await?;

    // The due-selection orders by a correlated MAX(priority) over `order_items`;
    // without this index that subquery scans the whole table per pending row.
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_order_items_isrc ON order_items(isrc);")
        .execute(pool)
        .await?;

    // Durable-queue columns (additive): bounded retries, backoff scheduling and a
    // download lease (visibility timeout). Ignored if already present.
    for sql in [
        "ALTER TABLE tracks ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE tracks ADD COLUMN next_attempt_at INTEGER",
        "ALTER TABLE tracks ADD COLUMN lease_expires_at INTEGER",
    ] {
        let _ = sqlx::query(sql).execute(pool).await;
    }
    // Due-selection (state+next_attempt_at) and the lease reaper stay index-only.
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_due ON tracks(state, next_attempt_at);")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_lease ON tracks(state, lease_expires_at);")
        .execute(pool)
        .await?;

    // Additive: order priority (higher = processed earlier). Ignored if present.
    let _ = sqlx::query("ALTER TABLE orders ADD COLUMN priority INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await;

    // Additive: acquisition source (`deezer` default; e.g. `soundcloud`).
    let _ = sqlx::query("ALTER TABLE tracks ADD COLUMN source TEXT NOT NULL DEFAULT 'deezer'")
        .execute(pool)
        .await;

    // The unified priority work-queue (additive, idempotent). Every background
    // kind — download, transcode, analyze, crawl:<source> leads — lives here;
    // `next` (see `crate::work`) is the single place priority is applied.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS work_items (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            kind          TEXT NOT NULL,
            ref           TEXT NOT NULL,
            priority      INTEGER NOT NULL DEFAULT 0,
            state         TEXT NOT NULL DEFAULT 'queued',
            attempts      INTEGER NOT NULL DEFAULT 0,
            max_attempts  INTEGER NOT NULL DEFAULT 5,
            not_before    INTEGER,
            record_source TEXT,
            error         TEXT,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL,
            UNIQUE (kind, ref)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Serves the `next` selection: kind + due + priority ordering.
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_work_next
            ON work_items(kind, state, priority DESC, not_before);",
    )
    .execute(pool)
    .await?;

    // Data Vault 2.0 tables (additive, idempotent). The vault is the data home;
    // the tables above stay the operational queue. See `crate::vault`.
    crate::vault::init(pool).await?;

    Ok(())
}

/// Create an order for the given (already normalised, de-duplicated) ISRCs.
/// Unknown ISRCs are inserted as `pending`; known ones keep their state, so
/// re-ordering an already-downloaded track is a cheap no-op.
pub async fn create_order(
    pool: &Pool<Sqlite>,
    order_id: &str,
    isrcs: &[String],
    priority: i64,
) -> anyhow::Result<()> {
    let ts = now();
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO orders (id, status, created_at, updated_at, priority)
         VALUES (?, 'open', ?, ?, ?)",
    )
    .bind(order_id)
    .bind(ts)
    .bind(ts)
    .bind(priority)
    .execute(&mut *tx)
    .await?;

    for isrc in isrcs {
        let source = crate::models::track_source(isrc);
        sqlx::query(
            "INSERT OR IGNORE INTO tracks (isrc, source, state, created_at, updated_at)
             VALUES (?, ?, 'pending', ?, ?)",
        )
        .bind(isrc)
        .bind(source)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;

        sqlx::query("INSERT OR IGNORE INTO order_items (order_id, isrc) VALUES (?, ?)")
            .bind(order_id)
            .bind(isrc)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    refresh_order(pool, order_id).await?;
    Ok(())
}

/// Derive an order's status from its items: `open` while anything is still
/// being worked on, `done` when everything is `ready`, otherwise `partial`.
pub async fn refresh_order(pool: &Pool<Sqlite>, order_id: &str) -> anyhow::Result<String> {
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT t.state FROM order_items oi
           JOIN tracks t ON t.isrc = oi.isrc
          WHERE oi.order_id = ?",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    let status = derive_order_status(&states);
    sqlx::query("UPDATE orders SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(now())
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(status.to_string())
}

/// Pure status derivation, unit-tested below.
pub fn derive_order_status(states: &[String]) -> &'static str {
    if states.is_empty() {
        return "open";
    }
    if states
        .iter()
        .any(|s| s == state::PENDING || s == state::DOWNLOADING)
    {
        return "open";
    }
    if states.iter().all(|s| s == state::READY) {
        "done"
    } else {
        "partial"
    }
}

/// Refresh every order that contains this ISRC.
pub async fn refresh_orders_for_isrc(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<()> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT order_id FROM order_items WHERE isrc = ?")
        .bind(isrc)
        .fetch_all(pool)
        .await?;
    for id in ids {
        refresh_order(pool, &id).await?;
    }
    Ok(())
}

pub async fn get_order(pool: &Pool<Sqlite>, id: &str) -> anyhow::Result<Option<Order>> {
    Ok(sqlx::query_as::<_, Order>(
        "SELECT id, status, created_at, updated_at, priority FROM orders WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_orders(
    pool: &Pool<Sqlite>,
    status: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<Order>> {
    match status {
        Some(s) => Ok(sqlx::query_as::<_, Order>(
            "SELECT id, status, created_at, updated_at, priority FROM orders
              WHERE status = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(s)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as::<_, Order>(
            "SELECT id, status, created_at, updated_at, priority FROM orders
              ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?),
    }
}

pub async fn order_items(pool: &Pool<Sqlite>, order_id: &str) -> anyhow::Result<Vec<OrderItem>> {
    let rows: Vec<Track> = sqlx::query_as::<_, Track>(
        "SELECT t.* FROM order_items oi
           JOIN tracks t ON t.isrc = oi.isrc
          WHERE oi.order_id = ?
          ORDER BY t.isrc",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|t| OrderItem {
            isrc: t.isrc.clone(),
            state: t.state.clone(),
            deezer_id: t.deezer_id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            formats: t.formats().into_iter().map(|s| s.to_string()).collect(),
            error: t.error.clone(),
        })
        .collect())
}

pub async fn get_track(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<Option<Track>> {
    Ok(
        sqlx::query_as::<_, Track>("SELECT * FROM tracks WHERE isrc = ?")
            .bind(isrc)
            .fetch_optional(pool)
            .await?,
    )
}

/// Due ISRC tracks: `pending` and past their backoff window, highest order
/// priority first, then oldest. Served by `idx_tracks_due` + `idx_order_items_isrc`.
pub async fn due_tracks(pool: &Pool<Sqlite>, limit: i64, now: i64) -> anyhow::Result<Vec<Track>> {
    Ok(sqlx::query_as::<_, Track>(
        "SELECT * FROM tracks t WHERE t.state = 'pending'
            AND (t.next_attempt_at IS NULL OR t.next_attempt_at <= ?)
          ORDER BY COALESCE((SELECT MAX(o.priority) FROM order_items oi
                              JOIN orders o ON o.id = oi.order_id
                             WHERE oi.isrc = t.isrc), 0) DESC,
                   t.created_at ASC
          LIMIT ?",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// The order priority for one ISRC: the highest priority among the orders it
/// belongs to (`0` when it has never been ordered). This is the single scale
/// every work kind reads.
pub async fn order_priority(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(MAX(o.priority), 0) FROM order_items oi
           JOIN orders o ON o.id = oi.order_id
          WHERE oi.isrc = ?",
    )
    .bind(isrc)
    .fetch_one(pool)
    .await?)
}

/// Retry tuning for the durable queue.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_secs: i64,
    pub max_secs: i64,
}

/// Exponential backoff for the `attempts`-th transient failure (1-based), capped
/// at `max`. Pure — unit-tested below.
pub fn backoff_delay(attempts: u32, base: i64, max: i64) -> i64 {
    let shift = attempts.saturating_sub(1).min(20);
    base.saturating_mul(1i64 << shift).min(max)
}

/// Record a transient failure for an ISRC: re-queue with a backoff delay, or go
/// terminal (`failed`) once `max_attempts` is reached. Returns `true` while the
/// row is still retryable.
pub async fn schedule_retry(
    pool: &Pool<Sqlite>,
    isrc: &str,
    error: &str,
    policy: &RetryPolicy,
    now: i64,
) -> anyhow::Result<bool> {
    sqlx::query("UPDATE tracks SET attempts = attempts + 1, updated_at = ? WHERE isrc = ?")
        .bind(now)
        .bind(isrc)
        .execute(pool)
        .await?;
    let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM tracks WHERE isrc = ?")
        .bind(isrc)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    if attempts as u32 >= policy.max_attempts {
        mark_terminal(pool, isrc, state::FAILED, error).await?;
        return Ok(false);
    }
    let next = now + backoff_delay(attempts as u32, policy.base_secs, policy.max_secs);
    sqlx::query(
        "UPDATE tracks SET state = 'pending', error = ?, next_attempt_at = ?,
                lease_expires_at = NULL, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(error)
    .bind(next)
    .bind(now)
    .bind(isrc)
    .execute(pool)
    .await?;
    refresh_orders_for_isrc(pool, isrc).await?;
    Ok(true)
}

/// Push a downloading row's lease further out (heartbeat). Called while deemix
/// still reports progress, so only truly-stuck rows fall to the reaper.
pub async fn renew_lease(pool: &Pool<Sqlite>, isrc: &str, until: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE tracks SET lease_expires_at = ? WHERE isrc = ?")
        .bind(until)
        .bind(isrc)
        .execute(pool)
        .await?;
    Ok(())
}

/// Re-queue ISRC rows stranded in `downloading` whose lease lapsed (dead process,
/// evicted item). Returns how many were recovered.
pub async fn recover_expired_leases(
    pool: &Pool<Sqlite>,
    policy: &RetryPolicy,
    now: i64,
) -> anyhow::Result<usize> {
    let isrcs: Vec<String> = sqlx::query_scalar(
        "SELECT isrc FROM tracks
          WHERE state = 'downloading'
            AND (lease_expires_at IS NULL OR lease_expires_at <= ?)",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    for isrc in &isrcs {
        schedule_retry(pool, isrc, "download lease expired", policy, now).await?;
    }
    Ok(isrcs.len())
}

/// Per-state counts (for the hub's queue overview).
pub async fn queue_counts(
    pool: &Pool<Sqlite>,
) -> anyhow::Result<std::collections::BTreeMap<String, i64>> {
    let mut out = std::collections::BTreeMap::new();
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT state, COUNT(*) FROM tracks GROUP BY state")
            .fetch_all(pool)
            .await?;
    for (state, n) in rows {
        out.insert(state, n);
    }
    Ok(out)
}

/// Number of rows currently in `downloading` — the in-flight set the
/// dispatcher caps against `MAX_INFLIGHT`. Counts every source, so a non-Deezer
/// (yt-dlp) download consumes a slot just like a deemix one.
pub async fn count_downloading(pool: &Pool<Sqlite>) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE state = 'downloading'")
            .fetch_one(pool)
            .await?,
    )
}

/// Reset a non-ready track to `pending` so the worker re-resolves/re-downloads it.
pub async fn reset_track_to_pending(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<bool> {
    let n = sqlx::query(
        "UPDATE tracks SET state = 'pending', error = NULL, deemix_uuid = NULL,
                attempts = 0, next_attempt_at = NULL, lease_expires_at = NULL, updated_at = ?
          WHERE isrc = ? AND state <> 'ready'",
    )
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?
    .rows_affected();
    refresh_orders_for_isrc(pool, isrc).await?;
    Ok(n > 0)
}

/// Set an order's priority (higher = processed earlier).
pub async fn set_order_priority(
    pool: &Pool<Sqlite>,
    order_id: &str,
    priority: i64,
) -> anyhow::Result<bool> {
    let n = sqlx::query("UPDATE orders SET priority = ?, updated_at = ? WHERE id = ?")
        .bind(priority)
        .bind(now())
        .bind(order_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// Cancel an order: mark it `cancelled` and drop its item links (tracks are global
/// and shared across orders, so their states are left untouched).
pub async fn cancel_order(pool: &Pool<Sqlite>, order_id: &str) -> anyhow::Result<bool> {
    let n = sqlx::query("UPDATE orders SET status = 'cancelled', updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(order_id)
        .execute(pool)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM order_items WHERE order_id = ?")
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(n > 0)
}

// ── Artist orders (whole-discography download) ───────────────────────────────

/// Create an artist order in the `open` state.
pub async fn create_artist_order(
    pool: &Pool<Sqlite>,
    id: &str,
    artist: &str,
    artist_url: &str,
    priority: i64,
) -> anyhow::Result<()> {
    let ts = now();
    sqlx::query(
        "INSERT INTO artist_orders
            (id, artist, artist_url, status, priority, added, created_at, updated_at)
         VALUES (?, ?, ?, 'open', ?, 0, ?, ?)",
    )
    .bind(id)
    .bind(artist)
    .bind(artist_url)
    .bind(priority)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_artist_order(
    pool: &Pool<Sqlite>,
    id: &str,
) -> anyhow::Result<Option<crate::models::ArtistOrder>> {
    Ok(
        sqlx::query_as::<_, crate::models::ArtistOrder>("SELECT * FROM artist_orders WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn list_artist_orders(
    pool: &Pool<Sqlite>,
    status: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<crate::models::ArtistOrder>> {
    match status {
        Some(s) => Ok(sqlx::query_as::<_, crate::models::ArtistOrder>(
            "SELECT * FROM artist_orders WHERE status = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(s)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as::<_, crate::models::ArtistOrder>(
            "SELECT * FROM artist_orders ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?),
    }
}

/// `open` artist orders not yet accepted by deemix, highest priority first.
pub async fn open_artist_orders(
    pool: &Pool<Sqlite>,
    limit: i64,
) -> anyhow::Result<Vec<crate::models::ArtistOrder>> {
    Ok(sqlx::query_as::<_, crate::models::ArtistOrder>(
        "SELECT * FROM artist_orders WHERE status = 'open'
          ORDER BY priority DESC, created_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// `running` artist orders to poll against the deemix queue.
pub async fn running_artist_orders(
    pool: &Pool<Sqlite>,
    limit: i64,
) -> anyhow::Result<Vec<crate::models::ArtistOrder>> {
    Ok(sqlx::query_as::<_, crate::models::ArtistOrder>(
        "SELECT * FROM artist_orders WHERE status = 'running'
          ORDER BY priority DESC, created_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Mark an artist order `running` with its deemix queue handle.
pub async fn mark_artist_order_running(
    pool: &Pool<Sqlite>,
    id: &str,
    deemix_uuid: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE artist_orders SET status = 'running', deemix_uuid = ?, error = NULL, updated_at = ?
          WHERE id = ?",
    )
    .bind(deemix_uuid)
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an artist order `done` with how many tracks were ingested.
pub async fn mark_artist_order_done(
    pool: &Pool<Sqlite>,
    id: &str,
    added: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE artist_orders SET status = 'done', added = ?, error = NULL, updated_at = ?
          WHERE id = ?",
    )
    .bind(added)
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an artist order `failed` with a reason.
pub async fn mark_artist_order_failed(
    pool: &Pool<Sqlite>,
    id: &str,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE artist_orders SET status = 'failed', error = ?, updated_at = ? WHERE id = ?",
    )
    .bind(error)
    .bind(now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record an ISRC ingested for an artist order (idempotent).
pub async fn add_artist_order_item(
    pool: &Pool<Sqlite>,
    order_id: &str,
    isrc: &str,
) -> anyhow::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO artist_order_items (order_id, isrc) VALUES (?, ?)")
        .bind(order_id)
        .bind(isrc)
        .execute(pool)
        .await?;
    Ok(())
}

/// The ISRCs ingested for an artist order.
pub async fn artist_order_items(
    pool: &Pool<Sqlite>,
    order_id: &str,
) -> anyhow::Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT isrc FROM artist_order_items WHERE order_id = ? ORDER BY isrc")
            .bind(order_id)
            .fetch_all(pool)
            .await?,
    )
}

/// Insert (or complete) a `ready` track row from an artist-order ingest. Existing
/// paths are never overwritten; the row is `ready` because the file is on disk.
/// This is what hands the file to the analysis worker (`path_flac`).
pub async fn ingest_ready_track(
    pool: &Pool<Sqlite>,
    isrc: &str,
    source_format: &str,
    path_flac: Option<&str>,
    path_320: Option<&str>,
    path_128: Option<&str>,
) -> anyhow::Result<()> {
    let ts = now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, source_format, path_flac, path_320, path_128, created_at, updated_at)
         VALUES (?, 'deezer', 'ready', ?, ?, ?, ?, ?, ?)
         ON CONFLICT(isrc) DO UPDATE SET
            source_format    = COALESCE(tracks.source_format, excluded.source_format),
            path_flac        = COALESCE(tracks.path_flac, excluded.path_flac),
            path_320         = COALESCE(tracks.path_320, excluded.path_320),
            path_128         = COALESCE(tracks.path_128, excluded.path_128),
            state            = 'ready',
            error            = NULL,
            lease_expires_at = NULL,
            next_attempt_at  = NULL,
            updated_at       = excluded.updated_at",
    )
    .bind(isrc)
    .bind(source_format)
    .bind(path_flac)
    .bind(path_320)
    .bind(path_128)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await?;
    refresh_orders_for_isrc(pool, isrc).await?;
    Ok(())
}

pub async fn downloading_tracks(pool: &Pool<Sqlite>) -> anyhow::Result<Vec<Track>> {
    // Only Deezer rows are polled against deemix; non-Deezer (yt-dlp) downloads
    // run on their own background tasks.
    Ok(sqlx::query_as::<_, Track>(
        "SELECT * FROM tracks WHERE state = 'downloading' AND source = 'deezer'",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn mark_resolved(
    pool: &Pool<Sqlite>,
    isrc: &str,
    deezer_id: &str,
    title: &str,
    artist: &str,
    album: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks SET deezer_id = ?, title = ?, artist = ?, album = ?, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(deezer_id)
    .bind(title)
    .bind(artist)
    .bind(album)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_downloading(
    pool: &Pool<Sqlite>,
    isrc: &str,
    deemix_uuid: &str,
    lease_until: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks SET state = 'downloading', deemix_uuid = ?, error = NULL,
                lease_expires_at = ?, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(deemix_uuid)
    .bind(lease_until)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    refresh_orders_for_isrc(pool, isrc).await
}

pub async fn mark_ready(
    pool: &Pool<Sqlite>,
    isrc: &str,
    source_format: &str,
    path_flac: Option<&str>,
    path_320: Option<&str>,
    path_128: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks
            SET state = 'ready', source_format = ?, path_flac = ?, path_320 = ?, path_128 = ?,
                error = NULL, lease_expires_at = NULL, next_attempt_at = NULL, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(source_format)
    .bind(path_flac)
    .bind(path_320)
    .bind(path_128)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    refresh_orders_for_isrc(pool, isrc).await
}

pub async fn mark_terminal(
    pool: &Pool<Sqlite>,
    isrc: &str,
    new_state: &str,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks SET state = ?, error = ?, lease_expires_at = NULL,
                next_attempt_at = NULL, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(new_state)
    .bind(error)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    refresh_orders_for_isrc(pool, isrc).await
}

/// Upsert per-ISRC metadata. Non-null fields win; a null field keeps the stored
/// value (merge, not replace). Returns how many rows were written.
pub async fn upsert_metadata(
    pool: &Pool<Sqlite>,
    items: &[crate::models::MetadataItem],
) -> anyhow::Result<usize> {
    let ts = now();
    let mut tx = pool.begin().await?;
    let mut n = 0usize;
    for it in items {
        let isrc = normalize_isrc(&it.isrc);
        if isrc.is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO track_metadata
                (isrc, bpm, musical_key, camelot, title, artist, album, genre, source, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(isrc) DO UPDATE SET
                bpm         = COALESCE(excluded.bpm, track_metadata.bpm),
                musical_key = COALESCE(excluded.musical_key, track_metadata.musical_key),
                camelot     = COALESCE(excluded.camelot, track_metadata.camelot),
                title       = COALESCE(excluded.title, track_metadata.title),
                artist      = COALESCE(excluded.artist, track_metadata.artist),
                album       = COALESCE(excluded.album, track_metadata.album),
                genre       = COALESCE(excluded.genre, track_metadata.genre),
                source      = COALESCE(excluded.source, track_metadata.source),
                updated_at  = excluded.updated_at",
        )
        .bind(&isrc)
        .bind(it.bpm)
        .bind(&it.key)
        .bind(&it.camelot)
        .bind(&it.title)
        .bind(&it.artist)
        .bind(&it.album)
        .bind(&it.genre)
        .bind(&it.source)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        n += 1;
    }
    tx.commit().await?;
    Ok(n)
}

/// Ready tracks that have a FLAC on disk but no Essentia analysis yet — the
/// analysis job's work set. Idempotent: once a `source='essentia'` row exists the
/// track drops out, so it is never analysed twice.
pub async fn ready_flac_unaanalyzed(pool: &Pool<Sqlite>, limit: i64) -> anyhow::Result<Vec<Track>> {
    Ok(sqlx::query_as::<_, Track>(
        "SELECT t.* FROM tracks t
          WHERE t.state = 'ready'
            AND t.path_flac IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM track_metadata m
                 WHERE m.isrc = t.isrc AND m.source = 'essentia')
          ORDER BY COALESCE((SELECT MAX(o.priority) FROM order_items oi
                              JOIN orders o ON o.id = oi.order_id
                             WHERE oi.isrc = t.isrc), 0) DESC,
                   t.updated_at ASC
          LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Metadata for a single ISRC (normalised lookup).
pub async fn get_metadata(
    pool: &Pool<Sqlite>,
    isrc: &str,
) -> anyhow::Result<Option<crate::models::TrackMetadata>> {
    Ok(sqlx::query_as::<_, crate::models::TrackMetadata>(
        "SELECT * FROM track_metadata WHERE isrc = ?",
    )
    .bind(normalize_isrc(isrc))
    .fetch_optional(pool)
    .await?
    .map(crate::models::TrackMetadata::with_key_alias))
}

/// Metadata for a range of ISRCs (already normalised/de-duplicated).
pub async fn metadata_bulk(
    pool: &Pool<Sqlite>,
    isrcs: &[String],
) -> anyhow::Result<Vec<crate::models::TrackMetadata>> {
    if isrcs.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = sqlx::QueryBuilder::new("SELECT * FROM track_metadata WHERE isrc IN (");
    let mut sep = qb.separated(", ");
    for i in isrcs {
        sep.push_bind(i.clone());
    }
    qb.push(")");
    Ok(qb
        .build_query_as::<crate::models::TrackMetadata>()
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(crate::models::TrackMetadata::with_key_alias)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isrc_normalisation() {
        assert_eq!(normalize_isrc("us-qx9-12-01487"), "USQX91201487");
        assert_eq!(normalize_isrc("  AEA0D1846146 "), "AEA0D1846146");
    }

    #[test]
    fn order_status_derivation() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        assert_eq!(derive_order_status(&s(&[state::PENDING])), "open");
        assert_eq!(derive_order_status(&s(&[state::DOWNLOADING])), "open");
        assert_eq!(
            derive_order_status(&s(&[state::READY, state::READY])),
            "done"
        );
        assert_eq!(
            derive_order_status(&s(&[state::READY, state::ABSENT])),
            "partial"
        );
        assert_eq!(
            derive_order_status(&s(&[state::READY, state::FAILED])),
            "partial"
        );
        assert_eq!(derive_order_status(&[]), "open");
    }

    #[test]
    fn backoff_delay_grows_and_caps() {
        assert_eq!(backoff_delay(1, 60, 1800), 60);
        assert_eq!(backoff_delay(2, 60, 1800), 120);
        assert_eq!(backoff_delay(3, 60, 1800), 240);
        // Capped at the maximum.
        assert_eq!(backoff_delay(10, 60, 1800), 1800);
        // Defensive: attempts = 0 behaves like the first attempt.
        assert_eq!(backoff_delay(0, 60, 1800), 60);
    }
}
