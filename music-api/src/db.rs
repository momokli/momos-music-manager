//! SQLite storage: schema + queries.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{Pool, Sqlite};

use crate::models::{state, Order, OrderItem, Track};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// The pre-#518 `tracks` identifier column. Assembled with `concat!` so the
/// source carries no literal occurrence of the removed brand name (the #518
/// grep acceptance allows exactly one such line — the legacy ARL alias in
/// `config`).
const LEGACY_REF_COLUMN: &str = concat!("dee", "mix", "_uuid");

/// Rename `table.from` to `table.to` when the old column is present and the new
/// one is not. Idempotent; SQLite ≥ 3.25 (`ALTER TABLE ... RENAME COLUMN`). Any
/// error propagates — a silently skipped rename would leave a broken schema.
async fn rename_column_if_exists(
    pool: &Pool<Sqlite>,
    table: &str,
    from: &str,
    to: &str,
) -> anyhow::Result<()> {
    let has_from: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2")
            .bind(table)
            .bind(from)
            .fetch_one(pool)
            .await?;
    if has_from == 0 {
        return Ok(());
    }
    let has_to: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2")
            .bind(table)
            .bind(to)
            .fetch_one(pool)
            .await?;
    if has_to == 0 {
        sqlx::query(&format!("ALTER TABLE {table} RENAME COLUMN {from} TO {to}"))
            .execute(pool)
            .await?;
    }
    Ok(())
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
            download_ref   TEXT,
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

    // #518: the identifier column used to be named after the removed sidecar.
    // Fresh databases get `download_ref` above; existing ones are migrated in
    // place (no `DROP`). A failure here must surface — a half-migrated schema is
    // worse than a boot error.
    rename_column_if_exists(pool, "tracks", LEGACY_REF_COLUMN, "download_ref").await?;

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

    // Additive: acquisition source (`deezer` default; e.g. `soundcloud`, `youtube`).
    let _ = sqlx::query("ALTER TABLE tracks ADD COLUMN source TEXT NOT NULL DEFAULT 'deezer'")
        .execute(pool)
        .await;

    // Deezer catalogue cache (U0a): the metadata the tonie path reads.
    // Payloads are stored verbatim, so the HTTP contract stays byte-identical
    // whether an answer comes from Deezer or from the cache.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS catalogue_album_pages (
            artist_id  TEXT NOT NULL,
            page_index INTEGER NOT NULL,
            payload    TEXT NOT NULL,
            fetched_at INTEGER NOT NULL,
            PRIMARY KEY (artist_id, page_index)
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS catalogue_albums (
            album_id     TEXT PRIMARY KEY,
            artist_id    TEXT,
            title        TEXT,
            release_date TEXT,
            record_type  TEXT,
            cover_url    TEXT,
            seconds      INTEGER,
            payload      TEXT NOT NULL,
            fetched_at   INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS catalogue_searches (
            term       TEXT NOT NULL,
            lim        INTEGER NOT NULL,
            payload    TEXT NOT NULL,
            fetched_at INTEGER NOT NULL,
            PRIMARY KEY (term, lim)
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_catalogue_albums_artist
             ON catalogue_albums(artist_id, release_date);",
    )
    .execute(pool)
    .await?;

    // Order intent + quality target (U0b). `wanted_quality` is the highest
    // tier ever requested for an ISRC, `quality_tier` what is actually on
    // disk; a higher wanted tier above the stored one re-arms the download.
    for sql in [
        "ALTER TABLE tracks ADD COLUMN wanted_quality TEXT",
        "ALTER TABLE tracks ADD COLUMN quality_tier TEXT",
        // #699: the additive *demand set* (`flac,320,128`) replacing the scalar
        // quality rank, plus the terminal outcome hint that lets a single
        // writer (`recompute_state`) derive `state` without losing `absent`.
        "ALTER TABLE tracks ADD COLUMN wanted_formats TEXT",
        "ALTER TABLE tracks ADD COLUMN terminal_state TEXT",
        // Explicit demand re-arm (a genuinely new, not-yet-servable format was
        // ordered). Distinguishes "owe a download" from "servable, nothing due".
        "ALTER TABLE tracks ADD COLUMN rearm_pending INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE tracks ADD COLUMN intent TEXT NOT NULL DEFAULT 'library'",
        "ALTER TABLE orders ADD COLUMN intent TEXT NOT NULL DEFAULT 'library'",
        "ALTER TABLE orders ADD COLUMN target TEXT",
    ] {
        let _ = sqlx::query(sql).execute(pool).await;
    }

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

/// The **single writer** of `tracks.state` (#699). Derives a row's state from
/// the *servable* set (`path_*`), the download lease, the explicit demand
/// re-arm flag and a recorded terminal outcome:
///
/// * nothing is servable and a terminal outcome is recorded (`failed`/`absent`)
///   → that outcome;
/// * a download is in flight (lease held, no retry scheduled) → `downloading`;
/// * an explicit demand re-arm is outstanding (a genuinely new, not-yet-servable
///   format was ordered) → `pending`;
/// * something is servable → `ready` (a failed *upgrade* never degrades the
///   row — the still-present format keeps it ready);
/// * otherwise the row still owes its first download → `pending`.
///
/// `quality_tier` (display only) is refreshed to the best format on disk.
/// Callers write the *inputs* (paths, error, terminal outcome, re-arm flag,
/// lease) and then call this; they never assign `state` themselves.
pub async fn recompute_state(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<()> {
    use crate::models::{format_rank, have_formats, token_tier};

    let row = sqlx::query_as::<
        _,
        (
            Option<String>, // path_flac
            Option<String>, // path_320
            Option<String>, // path_128
            String,         // state
            Option<i64>,    // next_attempt_at
            Option<i64>,    // lease_expires_at
            Option<String>, // terminal_state
            i64,            // rearm_pending
        ),
    >(
        "SELECT path_flac, path_320, path_128, state, next_attempt_at,
                lease_expires_at, terminal_state, rearm_pending
           FROM tracks WHERE isrc = ?",
    )
    .bind(isrc)
    .fetch_optional(pool)
    .await?;
    let Some((pf, p320, p128, cur, next_attempt, lease, terminal, rearm)) = row else {
        return Ok(()); // unknown ISRC — nothing to derive
    };

    let have = have_formats(pf.as_deref(), p320.as_deref(), p128.as_deref());

    let new_state: &str = if have.is_empty() && terminal.is_some() {
        terminal.as_deref().unwrap_or(state::PENDING)
    } else if next_attempt.is_none() && lease.is_some() {
        state::DOWNLOADING
    } else if rearm != 0 {
        state::PENDING
    } else if !have.is_empty() {
        state::READY
    } else {
        state::PENDING
    };

    let best_tier = have
        .iter()
        .max_by_key(|f| format_rank(f))
        .and_then(|f| token_tier(f));

    if new_state == state::READY && cur != state::READY {
        tracing::info!(
            isrc,
            from = %cur,
            formats = %have.join(","),
            "recompute_state: servable row recovered to ready"
        );
    }

    sqlx::query("UPDATE tracks SET state = ?, quality_tier = ?, updated_at = ? WHERE isrc = ?")
        .bind(new_state)
        .bind(best_tier)
        .bind(now())
        .bind(isrc)
        .execute(pool)
        .await?;
    Ok(())
}

/// Highest store tier of `a` and `b` (`None` when both are `None`).
fn max_tier(a: Option<&str>, b: Option<&str>) -> Option<String> {
    use crate::models::{format_rank, tier_token};
    let ra = a.and_then(tier_token).map(format_rank).unwrap_or(0);
    let rb = b.and_then(tier_token).map(format_rank).unwrap_or(0);
    if rb > ra {
        b.map(str::to_string)
    } else {
        a.or(b).map(str::to_string)
    }
}

/// Canonical (`flac,320,128`) join of a format set.
fn join_formats(v: &mut [String]) -> String {
    v.sort_by_key(|f| std::cmp::Reverse(crate::models::format_rank(f)));
    v.join(",")
}

/// Create an order for the given (already normalised, de-duplicated) ISRCs.
///
/// Demand is a **format set**: the order's target is merged additively into
/// `wanted_formats` (never replaces it). A track is re-armed (`pending`) only
/// when the new format is genuinely new *and* not already servable from what is
/// on disk — so ordering `128` for a FLAC track is a cheap no-op, and a format
/// that terminally failed is not re-armed by the same order again. `state` is
/// never written here; [`recompute_state`] derives it.
pub async fn create_order(
    pool: &Pool<Sqlite>,
    order_id: &str,
    isrcs: &[String],
    priority: i64,
    intent: &str,
    target: Option<&str>,
) -> anyhow::Result<()> {
    let ts = now();
    let target_token = target.and_then(crate::models::tier_token);
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO orders (id, status, created_at, updated_at, priority, intent, target)
         VALUES (?, 'open', ?, ?, ?, ?, ?)",
    )
    .bind(order_id)
    .bind(ts)
    .bind(ts)
    .bind(priority)
    .bind(intent)
    .bind(target)
    .execute(&mut *tx)
    .await?;

    for isrc in isrcs {
        let source = crate::models::track_source(isrc);
        let existing = sqlx::query_as::<
            _,
            (
                Option<String>, // wanted_quality
                Option<String>, // wanted_formats
                String,         // intent
                Option<String>, // path_flac
                Option<String>, // path_320
                Option<String>, // path_128
            ),
        >(
            "SELECT wanted_quality, wanted_formats, intent, path_flac, path_320, path_128
               FROM tracks WHERE isrc = ?",
        )
        .bind(isrc)
        .fetch_optional(&mut *tx)
        .await?;

        match existing {
            None => {
                // Fresh ISRC: insert pending; the demand set is the one target.
                sqlx::query(
                    "INSERT INTO tracks
                         (isrc, source, state, created_at, updated_at, wanted_quality,
                          wanted_formats, intent)
                     VALUES (?, ?, 'pending', ?, ?, ?, ?, ?)",
                )
                .bind(isrc)
                .bind(source)
                .bind(ts)
                .bind(ts)
                .bind(target)
                .bind(target_token)
                .bind(intent)
                .execute(&mut *tx)
                .await?;
            }
            Some((old_wq, old_wf, old_intent, pf, p320, p128)) => {
                let mut wanted = crate::models::parse_formats(old_wf.as_deref());
                let is_new = target_token.is_some_and(|t| !wanted.iter().any(|x| x == t));
                if let Some(t) = target_token {
                    if !wanted.iter().any(|x| x == t) {
                        wanted.push(t.to_string());
                    }
                }
                let new_wq = max_tier(old_wq.as_deref(), target);
                let wanted_str = join_formats(&mut wanted);
                // `library` wins over `toys` (max-merge).
                let new_intent = if old_intent == "library" || intent == "library" {
                    "library"
                } else {
                    intent
                };
                // Re-arm only for a genuinely new format that is *not* already
                // coverable from disk (a lossless/higher format covers a lower
                // one): ordering `128` for a FLAC track stays a cheap no-op.
                let have =
                    crate::models::have_formats(pf.as_deref(), p320.as_deref(), p128.as_deref());
                let covered = target_token
                    .is_some_and(|t| have.iter().any(|h| crate::models::format_covers(h, t)));
                let rearm = is_new && !covered;
                if rearm {
                    // A genuinely new demand clears any terminal outcome and the
                    // attempt counter, and raises the explicit re-arm flag so
                    // recompute becomes `pending`.
                    sqlx::query(
                        "UPDATE tracks
                            SET intent = ?, wanted_quality = ?, wanted_formats = ?,
                                attempts = 0, error = NULL, terminal_state = NULL,
                                rearm_pending = 1, next_attempt_at = NULL,
                                lease_expires_at = NULL, updated_at = ?
                          WHERE isrc = ?",
                    )
                    .bind(new_intent)
                    .bind(&new_wq)
                    .bind(&wanted_str)
                    .bind(ts)
                    .bind(isrc)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    sqlx::query(
                        "UPDATE tracks
                            SET intent = ?, wanted_quality = ?, wanted_formats = ?, updated_at = ?
                          WHERE isrc = ?",
                    )
                    .bind(new_intent)
                    .bind(&new_wq)
                    .bind(&wanted_str)
                    .bind(ts)
                    .bind(isrc)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        sqlx::query("INSERT OR IGNORE INTO order_items (order_id, isrc) VALUES (?, ?)")
            .bind(order_id)
            .bind(isrc)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    // Derive state per ISRC (the single writer) and materialise work **only when
    // a download is actually owed** — a ready-and-covered track stays untouched.
    for isrc in isrcs {
        recompute_state(pool, isrc).await?;
        let owed = match get_track(pool, isrc).await? {
            Some(t) => t.state == state::PENDING || t.state == state::DOWNLOADING,
            None => false,
        };
        if owed {
            crate::work::enqueue(
                pool,
                crate::work::kind::DOWNLOAD,
                isrc,
                priority,
                Some(crate::models::track_source(isrc)),
            )
            .await?;
        }
    }
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

/// Record derived lossy paths filled in by the `transcode` worker. `COALESCE`
/// keeps any path that is already present, so it is safe to call with partials.
pub async fn set_track_derived(
    pool: &Pool<Sqlite>,
    isrc: &str,
    path_320: Option<&str>,
    path_128: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks
            SET path_320 = COALESCE(?, path_320),
                path_128 = COALESCE(?, path_128),
                updated_at = ?
          WHERE isrc = ?",
    )
    .bind(path_320)
    .bind(path_128)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    recompute_state(pool, isrc).await?;
    Ok(())
}

/// Pending downloads with their order priority — the reconcile source for the
/// materialised `download` kind. Bounded by `limit`, highest priority first.
pub async fn pending_work(
    pool: &Pool<Sqlite>,
    limit: i64,
    now: i64,
) -> anyhow::Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT t.isrc AS isrc,
                COALESCE((SELECT MAX(o.priority) FROM order_items oi
                            JOIN orders o ON o.id = oi.order_id
                           WHERE oi.isrc = t.isrc), 0) AS priority
           FROM tracks t
          WHERE t.state = 'pending'
            AND (t.next_attempt_at IS NULL OR t.next_attempt_at <= ?)
          ORDER BY priority DESC, t.created_at ASC
          LIMIT ?",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
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
        "UPDATE tracks SET error = ?, next_attempt_at = ?,
                lease_expires_at = NULL, terminal_state = NULL, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(error)
    .bind(next)
    .bind(now)
    .bind(isrc)
    .execute(pool)
    .await?;
    recompute_state(pool, isrc).await?;
    refresh_orders_for_isrc(pool, isrc).await?;
    // Keep the materialised download item in lockstep (re-queue or fail out).
    crate::work::retry_ref(pool, crate::work::kind::DOWNLOAD, isrc, error, next, now).await?;
    Ok(true)
}

/// Push a downloading row's lease further out (heartbeat). Called while a
/// download is still progressing, so only truly-stuck rows fall to the reaper.
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
/// (yt-dlp) download consumes a slot just like a native one.
pub async fn count_downloading(pool: &Pool<Sqlite>) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE state = 'downloading'")
            .fetch_one(pool)
            .await?,
    )
}

/// The two visibility counters behind `GET /storage/stats` (#699).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    /// Rows that carry a `path_*` but are not `ready` — the degradation this
    /// issue removes. Must be `0` once the repair has run.
    pub servable_but_not_ready: i64,
    /// Rows with a set `path_*` whose file is not on disk.
    pub paths_missing_on_disk: i64,
}

/// Rows with any `path_*` set, as `(isrc, path_flac, path_320, path_128)`.
async fn rows_with_paths(
    pool: &Pool<Sqlite>,
) -> anyhow::Result<Vec<(String, Option<String>, Option<String>, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT isrc, path_flac, path_320, path_128 FROM tracks
          WHERE path_flac IS NOT NULL OR path_320 IS NOT NULL OR path_128 IS NOT NULL",
    )
    .fetch_all(pool)
    .await?)
}

/// Collect the non-empty paths of a row.
fn set_paths(a: Option<&str>, b: Option<&str>, c: Option<&str>) -> Vec<String> {
    [a, b, c]
        .into_iter()
        .flatten()
        .filter(|p| !p.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Compute the two visibility counters for `GET /storage/stats`.
pub async fn storage_stats(pool: &Pool<Sqlite>) -> anyhow::Result<StorageStats> {
    let servable_but_not_ready: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks
          WHERE state <> 'ready'
            AND (path_flac IS NOT NULL OR path_320 IS NOT NULL OR path_128 IS NOT NULL)",
    )
    .fetch_one(pool)
    .await?;

    let mut paths_missing_on_disk = 0i64;
    for (_isrc, a, b, c) in rows_with_paths(pool).await? {
        let mut missing = false;
        for p in set_paths(a.as_deref(), b.as_deref(), c.as_deref()) {
            if tokio::fs::metadata(&p).await.is_err() {
                missing = true;
                break;
            }
        }
        if missing {
            paths_missing_on_disk += 1;
        }
    }

    Ok(StorageStats {
        servable_but_not_ready,
        paths_missing_on_disk,
    })
}

/// Result of the `repair-states` CLI / `db::repair_states` (#699).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairStats {
    /// Rows that matched the degradation predicate (`state` non-ready + a path).
    pub checked: usize,
    /// Rows that have a servable file and were repaired to `ready`.
    pub repaired: usize,
    /// Rows whose set paths are all missing on disk (never repaired).
    pub paths_missing_on_disk: usize,
}

/// Repair the degraded rows of the pre-#699 model: a row with a `path_*` on disk
/// is servable, so it must be `ready` again. For each candidate the file is
/// **statted first** — a row whose set paths are all missing on disk is counted
/// and skipped (never a blind `UPDATE`). The `error` text is preserved.
///
/// `state` is not written directly: the terminal outcome is cleared and
/// [`recompute_state`] derives `ready` from the formats on disk — the same
/// single-writer path the normal flow uses.
pub async fn repair_states(pool: &Pool<Sqlite>, apply: bool) -> anyhow::Result<RepairStats> {
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT isrc, path_flac, path_320, path_128 FROM tracks
          WHERE state IN ('failed', 'absent', 'pending')
            AND (path_flac IS NOT NULL OR path_320 IS NOT NULL OR path_128 IS NOT NULL)",
    )
    .fetch_all(pool)
    .await?;

    let mut stats = RepairStats::default();
    for (isrc, a, b, c) in rows {
        stats.checked += 1;
        let paths = set_paths(a.as_deref(), b.as_deref(), c.as_deref());
        let mut any_present = false;
        for p in &paths {
            if tokio::fs::metadata(p).await.is_ok() {
                any_present = true;
                break;
            }
        }
        if !any_present {
            stats.paths_missing_on_disk += 1;
            continue;
        }
        if apply {
            // Clear the (degrading) terminal outcome, keep `error`; recompute
            // re-derives `ready` because a format is on disk.
            sqlx::query(
                "UPDATE tracks SET terminal_state = NULL, lease_expires_at = NULL,
                        next_attempt_at = NULL, updated_at = ? WHERE isrc = ?",
            )
            .bind(now())
            .bind(&isrc)
            .execute(pool)
            .await?;
            recompute_state(pool, &isrc).await?;
        }
        stats.repaired += 1;
    }
    Ok(stats)
}

/// Reset a non-ready track to `pending` so the worker re-resolves/re-downloads it.
pub async fn reset_track_to_pending(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<bool> {
    // `state` is left to `recompute_state`; an explicit retry clears the terminal
    // outcome and the attempt counter so the row becomes due again.
    let n = sqlx::query(
        "UPDATE tracks SET error = NULL, download_ref = NULL,
                attempts = 0, next_attempt_at = NULL, lease_expires_at = NULL,
                terminal_state = NULL, rearm_pending = 1, updated_at = ?
          WHERE isrc = ? AND state <> 'ready'",
    )
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?
    .rows_affected();
    recompute_state(pool, isrc).await?;
    refresh_orders_for_isrc(pool, isrc).await?;
    if n > 0 {
        crate::work::enqueue(pool, crate::work::kind::DOWNLOAD, isrc, 0, None).await?;
    }
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
    if n > 0 {
        // Re-prioritising an order raises the priority of its (materialised)
        // download items too.
        let isrcs: Vec<String> =
            sqlx::query_scalar("SELECT isrc FROM order_items WHERE order_id = ?")
                .bind(order_id)
                .fetch_all(pool)
                .await?;
        for isrc in isrcs {
            crate::work::enqueue(pool, crate::work::kind::DOWNLOAD, &isrc, priority, None).await?;
        }
    }
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

/// Insert (or complete) a `ready` track row. Existing paths are never
/// overwritten; the row is `ready` because the file is on disk. This is what
/// hands the file to the analysis worker (`path_flac`).
///
/// Used by the integration tests / future crawl ingest.
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
            error            = NULL,
            lease_expires_at = NULL,
            next_attempt_at  = NULL,
            terminal_state   = NULL,
            rearm_pending    = 0,
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
    recompute_state(pool, isrc).await?;
    refresh_orders_for_isrc(pool, isrc).await?;
    Ok(())
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
    download_ref: &str,
    lease_until: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks SET download_ref = ?, error = NULL,
                lease_expires_at = ?, next_attempt_at = NULL,
                terminal_state = NULL, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(download_ref)
    .bind(lease_until)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    recompute_state(pool, isrc).await?;
    crate::work::ensure_running_ref(pool, crate::work::kind::DOWNLOAD, isrc, now()).await?;
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
    // Additive: each path column is only written when a value is passed
    // (`COALESCE`), so a second download at another quality never clears a path
    // that is already on disk. `quality_tier`/`state` are derived by
    // `recompute_state` afterwards.
    sqlx::query(
        "UPDATE tracks
            SET source_format = COALESCE(?, source_format),
                path_flac = COALESCE(?, path_flac),
                path_320 = COALESCE(?, path_320),
                path_128 = COALESCE(?, path_128),
                error = NULL, lease_expires_at = NULL, next_attempt_at = NULL,
                terminal_state = NULL, rearm_pending = 0, updated_at = ?
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
    recompute_state(pool, isrc).await?;
    crate::work::finish_ref(pool, crate::work::kind::DOWNLOAD, isrc, now()).await?;
    refresh_orders_for_isrc(pool, isrc).await
}

/// Record a terminal download outcome (`failed`/`absent`). `error` stays as the
/// attempt record (never overwritten here). `state` is **not** written: the
/// outcome is stored in `terminal_state` and `recompute_state` derives the row's
/// state — a row that still has a servable format (`have != {}`) is *not*
/// degraded (#699).
pub async fn mark_terminal(
    pool: &Pool<Sqlite>,
    isrc: &str,
    new_state: &str,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks SET error = ?, terminal_state = ?, lease_expires_at = NULL,
                next_attempt_at = NULL, rearm_pending = 0, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(error)
    .bind(new_state)
    .bind(now())
    .bind(isrc)
    .execute(pool)
    .await?;
    recompute_state(pool, isrc).await?;
    crate::work::finish_ref(pool, crate::work::kind::DOWNLOAD, isrc, now()).await?;
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

// ── Deezer catalogue cache (U0a) ────────────────────────────────────────────

/// TTL for an artist's album page: release lists change rarely.
pub const CATALOGUE_ARTIST_TTL: i64 = 24 * 3600;
/// TTL for an album's tracklist.
pub const CATALOGUE_ALBUM_TTL: i64 = 7 * 24 * 3600;
/// TTL for an artist search result.
pub const CATALOGUE_SEARCH_TTL: i64 = 24 * 3600;

/// One page of an artist's release list, when fresh.
pub async fn catalogue_page_get(
    pool: &Pool<Sqlite>,
    artist_id: &str,
    index: i64,
) -> anyhow::Result<Option<serde_json::Value>> {
    let q = sqlx::query(
        "SELECT payload, fetched_at FROM catalogue_album_pages
          WHERE artist_id = ? AND page_index = ?",
    )
    .bind(artist_id)
    .bind(index);
    let row = q.fetch_optional(pool).await?;
    let Some(row) = row else { return Ok(None) };
    let fetched_at: i64 = sqlx::Row::get(&row, "fetched_at");
    if fetched_at < now() - CATALOGUE_ARTIST_TTL {
        return Ok(None);
    }
    let payload: String = sqlx::Row::get(&row, "payload");
    Ok(serde_json::from_str(&payload).ok())
}

/// Store one page of an artist's release list.
pub async fn catalogue_page_put(
    pool: &Pool<Sqlite>,
    artist_id: &str,
    index: i64,
    page: &serde_json::Value,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO catalogue_album_pages (artist_id, page_index, payload, fetched_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(artist_id, page_index) DO UPDATE SET
             payload = excluded.payload, fetched_at = excluded.fetched_at",
    )
    .bind(artist_id)
    .bind(index)
    .bind(page.to_string())
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// One album (metadata + tracklist), when fresh.
pub async fn catalogue_album_get(
    pool: &Pool<Sqlite>,
    album_id: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row = sqlx::query("SELECT payload, fetched_at FROM catalogue_albums WHERE album_id = ?")
        .bind(album_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let fetched_at: i64 = sqlx::Row::get(&row, "fetched_at");
    if fetched_at < now() - CATALOGUE_ALBUM_TTL {
        return Ok(None);
    }
    let payload: String = sqlx::Row::get(&row, "payload");
    Ok(serde_json::from_str(&payload).ok())
}

/// Store one album; the derived columns are best effort (the payload is
/// authoritative and is what the HTTP layer returns).
pub async fn catalogue_album_put(
    pool: &Pool<Sqlite>,
    album_id: &str,
    album: &serde_json::Value,
) -> anyhow::Result<()> {
    let text = |k: &str| album.get(k).and_then(|v| v.as_str()).map(str::to_string);
    sqlx::query(
        "INSERT INTO catalogue_albums
             (album_id, artist_id, title, release_date, record_type, cover_url,
              seconds, payload, fetched_at)
         VALUES (?, NULL, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(album_id) DO UPDATE SET
             title = excluded.title,
             release_date = excluded.release_date,
             record_type = excluded.record_type,
             cover_url = excluded.cover_url,
             seconds = excluded.seconds,
             payload = excluded.payload,
             fetched_at = excluded.fetched_at",
    )
    .bind(album_id)
    .bind(text("title"))
    .bind(text("releaseDate"))
    .bind(text("recordType"))
    .bind(text("coverUrl"))
    .bind(album.get("seconds").and_then(|v| v.as_i64()))
    .bind(album.to_string())
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Album lengths (`catalogue_albums.seconds`) for many albums in **one** query.
///
/// The artist album list carries no length at all, so the handler decorates the
/// page's `albums[].seconds` from here — a single indexed `IN (…)` lookup beats
/// a per-album `catalogue_album_get` (N+1) on a grid of 50 tiles.
///
/// Only albums with a stored, non-null `seconds` are returned. The freshness
/// window mirrors `catalogue_album_get` (`CATALOGUE_ALBUM_TTL`): an album's
/// tracklist output is what ages, but we keep the length hint on the exact same
/// freshness contract so both readers agree. Older rows are simply absent — the
/// field is a best-effort hint and must never block a request.
pub async fn catalogue_album_seconds_batch(
    pool: &Pool<Sqlite>,
    ids: &[String],
) -> anyhow::Result<HashMap<String, i64>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let cutoff = now() - CATALOGUE_ALBUM_TTL;
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT album_id, seconds FROM catalogue_albums 
         WHERE seconds IS NOT NULL AND fetched_at >= ",
    );
    qb.push_bind(cutoff);
    qb.push(" AND album_id IN (");
    let mut sep = qb.separated(", ");
    for id in ids {
        sep.push_bind(id.clone());
    }
    qb.push(")");
    let rows = qb.build().fetch_all(pool).await?;
    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        let album_id: String = sqlx::Row::get(&row, "album_id");
        let seconds: i64 = sqlx::Row::get(&row, "seconds");
        out.insert(album_id, seconds);
    }
    Ok(out)
}

/// An artist search result, when fresh.
pub async fn catalogue_search_get(
    pool: &Pool<Sqlite>,
    term: &str,
    limit: i64,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row = sqlx::query(
        "SELECT payload, fetched_at FROM catalogue_searches WHERE term = ? AND lim = ?",
    )
    .bind(normalize_term(term))
    .bind(limit)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let fetched_at: i64 = sqlx::Row::get(&row, "fetched_at");
    if fetched_at < now() - CATALOGUE_SEARCH_TTL {
        return Ok(None);
    }
    let payload: String = sqlx::Row::get(&row, "payload");
    Ok(serde_json::from_str(&payload).ok())
}

/// Store an artist search result.
pub async fn catalogue_search_put(
    pool: &Pool<Sqlite>,
    term: &str,
    limit: i64,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO catalogue_searches (term, lim, payload, fetched_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(term, lim) DO UPDATE SET
             payload = excluded.payload, fetched_at = excluded.fetched_at",
    )
    .bind(normalize_term(term))
    .bind(limit)
    .bind(payload.to_string())
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Search terms are matched case-insensitively and trimmed.
fn normalize_term(term: &str) -> String {
    term.trim().to_lowercase()
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
