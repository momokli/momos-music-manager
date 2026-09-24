//! Per-ISRC state of the `music-api` import flow (table `music_api_imports`).
//!
//! The table is the local mirror of what MMM ordered and imported. It makes the
//! demand loop idempotent: a settled ISRC is never ordered twice. Settled means
//! in flight or done (`ordered`/`ready`/`imported`), genuinely absent (`absent`),
//! or `failed` past [`MAX_FAILED_ATTEMPTS`] — a `failed` ISRC below the budget
//! stays in the demand and is retried.

use std::collections::{BTreeSet, HashMap, HashSet};

use sqlx::{FromRow, Pool, Sqlite};

/// How many times a `failed` ISRC is re-ordered before it settles. `absent` is
/// always terminal; `failed` usually covers transient reasons (download timeout,
/// file not found after download) and deserves a few more tries.
pub const MAX_FAILED_ATTEMPTS: i64 = 3;

/// One row of `music_api_imports`.
#[derive(Debug, Clone, FromRow)]
pub struct MusicApiImport {
    pub isrc: String,
    /// `ordered` | `ready` | `imported` | `absent` | `failed`.
    pub state: String,
    /// Failed attempts so far (drives the retry budget).
    pub attempts: i64,
    /// Placed format (`flac` | `320` | `128`).
    pub format: Option<String>,
    pub file_path: Option<String>,
    pub deezer_id: Option<String>,
    pub error: Option<String>,
    pub order_id: Option<String>,
    pub updated_at: i64,
}

// A settled ISRC must not be ordered again: re-ordering an in-flight ISRC would
// spam `music-api` every cycle and stall progress at the first batch. See
// `settled_isrcs`.

/// Convert the backpack helpers' `anyhow` error into an `sqlx::Error`.
fn as_sqlx(e: anyhow::Error) -> sqlx::Error {
    sqlx::Error::Protocol(e.to_string())
}

/// Record that an ISRC was placed in a `music-api` order.
///
/// Never rewinds a track that already has a delivered file: a `ready` row keeps
/// its state, and `imported`/`absent`/settled-`failed` rows are left untouched.
/// A `failed` row below [`MAX_FAILED_ATTEMPTS`] is retried — it goes back to
/// `ordered` (keeping its attempt count) so the loop can consume it again.
pub async fn upsert_ordered(
    pool: &Pool<Sqlite>,
    isrc: &str,
    order_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO music_api_imports (isrc, state, order_id, updated_at)
           VALUES (?, 'ordered', ?, unixepoch())
           ON CONFLICT(isrc) DO UPDATE SET
               state = 'ordered',
               order_id = excluded.order_id,
               error = NULL,
               updated_at = excluded.updated_at
           WHERE music_api_imports.state NOT IN ('ready', 'imported', 'absent')
             AND NOT (music_api_imports.state = 'failed'
                      AND music_api_imports.attempts >= ?)"#,
    )
    .bind(isrc)
    .bind(order_id)
    .bind(MAX_FAILED_ATTEMPTS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Set the state (and optional details) of an ISRC, inserting a row if needed.
pub async fn set_state(
    pool: &Pool<Sqlite>,
    isrc: &str,
    state: &str,
    format: Option<&str>,
    file_path: Option<&str>,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO music_api_imports (isrc, state, format, file_path, error, updated_at)
           VALUES (?, ?, ?, ?, ?, unixepoch())
           ON CONFLICT(isrc) DO UPDATE SET
               state = excluded.state,
               format = excluded.format,
               file_path = excluded.file_path,
               error = excluded.error,
               updated_at = excluded.updated_at"#,
    )
    .bind(isrc)
    .bind(state)
    .bind(format)
    .bind(file_path)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a `failed` state and bump the retry counter. `absent` uses
/// [`set_state`] directly — it is terminal and has no retry budget.
pub async fn record_failure(
    pool: &Pool<Sqlite>,
    isrc: &str,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO music_api_imports (isrc, state, error, attempts, updated_at)
           VALUES (?, 'failed', ?, 1, unixepoch())
           ON CONFLICT(isrc) DO UPDATE SET
               state = 'failed',
               error = excluded.error,
               attempts = music_api_imports.attempts + 1,
               updated_at = unixepoch()"#,
    )
    .bind(isrc)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Read one ISRC's row (best-effort — DB errors become `None`).
pub async fn get(pool: &Pool<Sqlite>, isrc: &str) -> Option<MusicApiImport> {
    sqlx::query_as::<_, MusicApiImport>("SELECT * FROM music_api_imports WHERE isrc = ?")
        .bind(isrc)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Count rows grouped by state (absent keys mean zero).
pub async fn status_counts(pool: &Pool<Sqlite>) -> HashMap<String, i64> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT state, COUNT(*) FROM music_api_imports GROUP BY state")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    rows.into_iter().collect()
}

/// `service_tracks.id` of Backpack tracks that have at least one linked file.
///
/// Uses the *file_id* direction of `v_file_track_link` (fast, ~50 ms) on the
/// file ids returned by [`crate::backpack::get_file_ids_for_track_ids`]; the
/// view's `track_id IN (...)` direction degenerates into a full materialisation
/// and must not be used here.
async fn covered_track_ids(
    pool: &Pool<Sqlite>,
    file_ids: &[i64],
) -> Result<BTreeSet<i64>, sqlx::Error> {
    if file_ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let placeholders = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT DISTINCT track_id FROM v_file_track_link WHERE file_id IN ({placeholders})"
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in file_ids {
        query = query.bind(id);
    }
    Ok(query.fetch_all(pool).await?.into_iter().collect())
}

/// Backpack track ids that have no linked file.
async fn missing_backpack_track_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>, sqlx::Error> {
    let track_ids = crate::backpack::get_backpack_track_ids(pool)
        .await
        .map_err(as_sqlx)?;
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let linked_files = crate::backpack::get_file_ids_for_track_ids(pool, &track_ids)
        .await
        .map_err(as_sqlx)?;
    let covered = covered_track_ids(pool, &linked_files).await?;
    Ok(track_ids
        .into_iter()
        .filter(|id| !covered.contains(id))
        .collect())
}

/// Distinct ISRCs of the given tracks, ordered stably.
async fn isrcs_for_tracks(
    pool: &Pool<Sqlite>,
    track_ids: &[i64],
) -> Result<Vec<String>, sqlx::Error> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = track_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT DISTINCT isrc FROM service_tracks
          WHERE isrc IS NOT NULL AND id IN ({placeholders})
          ORDER BY isrc"
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql);
    for id in track_ids {
        query = query.bind(id);
    }
    query.fetch_all(pool).await
}

/// ISRCs that are settled and must never be ordered again: in flight or done
/// (`ordered`/`ready`/`imported`), genuinely absent (`absent`), or `failed` past
/// the retry budget. A `failed` ISRC *below* the budget stays in the demand.
async fn settled_isrcs(pool: &Pool<Sqlite>) -> Result<HashSet<String>, sqlx::Error> {
    let sql = "SELECT isrc FROM music_api_imports
               WHERE state IN ('ordered', 'ready', 'imported', 'absent')
                  OR (state = 'failed' AND attempts >= ?)";
    Ok(sqlx::query_scalar::<_, String>(sql)
        .bind(MAX_FAILED_ATTEMPTS)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect())
}

/// Distinct ISRCs of *every* service track that has no linked file — the whole
/// library backlog, ordered stably by ISRC. The Backpack is a subset of this;
/// [`demand_isrcs`] drains the Backpack part first.
async fn missing_track_isrcs(pool: &Pool<Sqlite>) -> Result<Vec<String>, sqlx::Error> {
    // A plain scan of the `track_id` column of the view is cheap (~40 ms on a
    // production library); only a `track_id IN (<huge list>)` predicate
    // degenerates, and this query has none.
    sqlx::query_scalar(
        "SELECT DISTINCT st.isrc
           FROM service_tracks st
          WHERE st.isrc IS NOT NULL
            AND st.id NOT IN (SELECT track_id FROM v_file_track_link)
          ORDER BY st.isrc",
    )
    .fetch_all(pool)
    .await
}

/// ISRCs to order from `music-api`: every library track that has `isrc` set, no
/// linked file and no ledger row yet — **Backpack tracks first** so they never
/// wait behind the backlog. Deduplicated, capped at `limit`.
pub async fn demand_isrcs(pool: &Pool<Sqlite>, limit: usize) -> Result<Vec<String>, sqlx::Error> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let known = settled_isrcs(pool).await?;
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // (1) Priority: missing Backpack ISRCs jump the queue.
    let missing_backpack = missing_backpack_track_ids(pool).await?;
    for isrc in isrcs_for_tracks(pool, &missing_backpack).await? {
        if known.contains(&isrc) || !seen.insert(isrc.clone()) {
            continue;
        }
        out.push(isrc);
        if out.len() >= limit {
            return Ok(out);
        }
    }

    // (2) Backlog: everything else in the library, in ISRC order.
    for isrc in missing_track_isrcs(pool).await? {
        if known.contains(&isrc) || !seen.insert(isrc.clone()) {
            continue;
        }
        out.push(isrc);
        if out.len() >= limit {
            return Ok(out);
        }
    }

    Ok(out)
}

/// Cheap count of [`demand_isrcs`] (no limit): every library ISRC without a
/// linked file that is not settled yet. Covers both the priority and the
/// backlog, so it agrees with the sum of what [`demand_isrcs`] would return.
pub async fn demand_count(pool: &Pool<Sqlite>) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT DISTINCT st.isrc FROM service_tracks st
             WHERE st.isrc IS NOT NULL
               AND st.id NOT IN (SELECT track_id FROM v_file_track_link)
         ) WHERE isrc NOT IN (
            SELECT isrc FROM music_api_imports
             WHERE state IN ('ordered', 'ready', 'imported', 'absent')
                OR (state = 'failed' AND attempts >= ?)
         )",
    )
    .bind(MAX_FAILED_ATTEMPTS)
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

/// Rows still in flight (`ordered`/`ready`).
pub async fn active_rows(pool: &Pool<Sqlite>) -> Vec<MusicApiImport> {
    sqlx::query_as::<_, MusicApiImport>(
        "SELECT * FROM music_api_imports WHERE state IN ('ordered', 'ready')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal schema for the demand query + the state helpers. The Backpack
    /// membership and file-link helpers are exercised through their real SQL,
    /// so those tables/views must exist with the columns they read.
    async fn test_db() -> Pool<Sqlite> {
        let pool = sqlx::sqlite::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"CREATE TABLE music_api_imports (
                isrc TEXT PRIMARY KEY, state TEXT NOT NULL, format TEXT, file_path TEXT,
                deezer_id TEXT, error TEXT, order_id TEXT, updated_at INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE service_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT, service TEXT NOT NULL DEFAULT 'spotify',
                service_id TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '',
                artist TEXT NOT NULL DEFAULT '', isrc TEXT)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE files (
                id INTEGER PRIMARY KEY AUTOINCREMENT, file_path TEXT NOT NULL DEFAULT '',
                isrc TEXT, spotify_id TEXT, soundcloud_id TEXT, youtube_id TEXT)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE file_track_corrections (
                file_id INTEGER NOT NULL, track_id INTEGER NOT NULL, link_type TEXT NOT NULL)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // The view is stood up as a plain table for the file_id direction.
        sqlx::query(
            "CREATE TABLE v_file_track_link (file_id INTEGER NOT NULL, track_id INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE playlist_subscriptions (
                service TEXT NOT NULL DEFAULT 'spotify', playlist_id TEXT NOT NULL DEFAULT '',
                is_active INTEGER NOT NULL DEFAULT 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE service_playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL DEFAULT '',
                service TEXT NOT NULL DEFAULT '', playlist_id TEXT NOT NULL DEFAULT '')"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE service_playlist_tracks (
                playlist_id INTEGER NOT NULL, track_id INTEGER NOT NULL,
                deleted_at INTEGER)"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE tags (id INTEGER PRIMARY KEY, backpack INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE v_track_tags (track_id INTEGER NOT NULL, tag_id INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    /// Seed a `backpack` tag + resolved link so `get_backpack_track_ids` returns
    /// the track.
    async fn make_backpack_track(pool: &Pool<Sqlite>, track_id: i64, isrc: &str) {
        sqlx::query("INSERT OR IGNORE INTO tags (id, backpack) VALUES (1, 1)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO service_tracks (id, service, service_id, isrc) VALUES (?, 'spotify', ?, ?)")
            .bind(track_id)
            .bind(format!("sp-{track_id}"))
            .bind(isrc)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO v_track_tags (track_id, tag_id) VALUES (?, 1)")
            .bind(track_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn link_file(pool: &Pool<Sqlite>, file_id: i64, track_id: i64, isrc: &str) {
        sqlx::query("INSERT INTO files (id, file_path, isrc) VALUES (?, ?, ?)")
            .bind(file_id)
            .bind(format!("/music/f{file_id}.flac"))
            .bind(isrc)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO v_file_track_link (file_id, track_id) VALUES (?, ?)")
            .bind(file_id)
            .bind(track_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn upsert_ordered_inserts_then_get_reads_back() {
        let pool = test_db().await;
        upsert_ordered(&pool, "ISRC1", "order-1").await.unwrap();

        let row = get(&pool, "ISRC1").await.unwrap();
        assert_eq!(row.state, "ordered");
        assert_eq!(row.order_id.as_deref(), Some("order-1"));

        // Re-ordering keeps the state and replaces the order id.
        upsert_ordered(&pool, "ISRC1", "order-2").await.unwrap();
        let row = get(&pool, "ISRC1").await.unwrap();
        assert_eq!(row.state, "ordered");
        assert_eq!(row.order_id.as_deref(), Some("order-2"));
    }

    #[tokio::test]
    async fn upsert_ordered_does_not_rewind_ready_or_terminal() {
        let pool = test_db().await;
        set_state(&pool, "READY", "ready", None, None, None)
            .await
            .unwrap();
        set_state(
            &pool,
            "DONE",
            "imported",
            Some("flac"),
            Some("/x.flac"),
            None,
        )
        .await
        .unwrap();
        set_state(&pool, "GONE", "absent", None, None, Some("no match"))
            .await
            .unwrap();

        upsert_ordered(&pool, "READY", "order-x").await.unwrap();
        upsert_ordered(&pool, "DONE", "order-x").await.unwrap();
        upsert_ordered(&pool, "GONE", "order-x").await.unwrap();

        assert_eq!(get(&pool, "READY").await.unwrap().state, "ready");
        assert_eq!(get(&pool, "DONE").await.unwrap().state, "imported");
        assert_eq!(get(&pool, "GONE").await.unwrap().state, "absent");
        assert_eq!(get(&pool, "READY").await.unwrap().order_id, None);
    }

    #[tokio::test]
    async fn set_state_inserts_and_updates() {
        let pool = test_db().await;
        set_state(&pool, "ISRC2", "ordered", None, None, None)
            .await
            .unwrap();
        set_state(
            &pool,
            "ISRC2",
            "imported",
            Some("flac"),
            Some("/music/a - b.flac"),
            None,
        )
        .await
        .unwrap();

        let row = get(&pool, "ISRC2").await.unwrap();
        assert_eq!(row.state, "imported");
        assert_eq!(row.format.as_deref(), Some("flac"));
        assert_eq!(row.file_path.as_deref(), Some("/music/a - b.flac"));
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_isrc() {
        let pool = test_db().await;
        assert!(get(&pool, "NOPE").await.is_none());
    }

    #[tokio::test]
    async fn status_counts_groups_by_state() {
        let pool = test_db().await;
        set_state(&pool, "A", "ordered", None, None, None)
            .await
            .unwrap();
        set_state(&pool, "B", "ordered", None, None, None)
            .await
            .unwrap();
        set_state(&pool, "C", "absent", None, None, None)
            .await
            .unwrap();

        let counts = status_counts(&pool).await;
        assert_eq!(counts.get("ordered"), Some(&2));
        assert_eq!(counts.get("absent"), Some(&1));
        assert_eq!(counts.get("failed"), None);
    }

    #[tokio::test]
    async fn active_rows_only_in_flight() {
        let pool = test_db().await;
        set_state(&pool, "A", "ordered", None, None, None)
            .await
            .unwrap();
        set_state(&pool, "B", "ready", None, None, None)
            .await
            .unwrap();
        set_state(&pool, "C", "imported", None, None, None)
            .await
            .unwrap();

        let active = active_rows(&pool).await;
        let mut states: Vec<_> = active.iter().map(|r| r.state.clone()).collect();
        states.sort();
        assert_eq!(states, vec!["ordered", "ready"]);
    }

    #[tokio::test]
    async fn demand_isrcs_returns_unlinked_backpack_isrcs() {
        let pool = test_db().await;
        make_backpack_track(&pool, 1, "ISRC-A").await;
        make_backpack_track(&pool, 2, "ISRC-B").await;

        let demand = demand_isrcs(&pool, 100).await.unwrap();
        assert_eq!(demand, vec!["ISRC-A", "ISRC-B"]);
        assert_eq!(demand_count(&pool).await, 2);

        // A linked file removes the track from the demand.
        link_file(&pool, 10, 2, "ISRC-B").await;
        let demand = demand_isrcs(&pool, 100).await.unwrap();
        assert_eq!(demand, vec!["ISRC-A"]);
        assert_eq!(demand_count(&pool).await, 1);
    }

    #[tokio::test]
    async fn demand_skips_terminal_and_dedupes_and_limits() {
        let pool = test_db().await;
        make_backpack_track(&pool, 1, "ISRC-1").await;
        make_backpack_track(&pool, 2, "ISRC-1").await; // duplicates ISRC-1
        make_backpack_track(&pool, 3, "ISRC-3").await;

        // ISRC-2 duplicates ISRC-1 -> deduped.
        let demand = demand_isrcs(&pool, 100).await.unwrap();
        assert_eq!(demand, vec!["ISRC-1", "ISRC-3"]);

        // Terminal state removes it (both tracks share the isrc).
        set_state(&pool, "ISRC-1", "absent", None, None, Some("nope"))
            .await
            .unwrap();
        let demand = demand_isrcs(&pool, 100).await.unwrap();
        assert_eq!(demand, vec!["ISRC-3"]);

        // Limit caps the result.
        make_backpack_track(&pool, 4, "ISRC-4").await;
        let demand = demand_isrcs(&pool, 1).await.unwrap();
        assert_eq!(demand.len(), 1);
    }

    #[tokio::test]
    async fn demand_skips_in_flight_orders() {
        let pool = test_db().await;
        make_backpack_track(&pool, 1, "ISRC-A").await;
        make_backpack_track(&pool, 2, "ISRC-B").await;

        // Ordering must take ISRC-A out of the demand immediately, otherwise the
        // next cycle re-orders the same batch instead of advancing to ISRC-B.
        upsert_ordered(&pool, "ISRC-A", "order-1").await.unwrap();
        let demand = demand_isrcs(&pool, 100).await.unwrap();
        assert_eq!(demand, vec!["ISRC-B"]);
        assert_eq!(demand_count(&pool).await, 1);

        // `ready` (delivered, not yet imported) is in flight too.
        set_state(&pool, "ISRC-B", "ready", Some("flac"), None, None)
            .await
            .unwrap();
        assert!(demand_isrcs(&pool, 100).await.unwrap().is_empty());
        assert_eq!(demand_count(&pool).await, 0);
    }

    /// Seed a plain (non-Backpack) library track.
    async fn make_track(pool: &Pool<Sqlite>, track_id: i64, isrc: &str) {
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, isrc) VALUES (?, 'spotify', ?, ?)",
        )
        .bind(track_id)
        .bind(format!("sp-{track_id}"))
        .bind(isrc)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn failed_is_retried_until_the_budget_is_spent() {
        let pool = test_db().await;
        make_backpack_track(&pool, 1, "ISRC-A").await;

        // A first failure below the budget keeps the ISRC in the demand.
        upsert_ordered(&pool, "ISRC-A", "order-1").await.unwrap();
        record_failure(&pool, "ISRC-A", Some("download timeout"))
            .await
            .unwrap();
        assert_eq!(get(&pool, "ISRC-A").await.unwrap().attempts, 1);
        assert_eq!(demand_isrcs(&pool, 100).await.unwrap(), vec!["ISRC-A"]);

        // Re-ordering is allowed and does not reset the attempt count.
        upsert_ordered(&pool, "ISRC-A", "order-2").await.unwrap();
        let row = get(&pool, "ISRC-A").await.unwrap();
        assert_eq!(row.state, "ordered");
        assert_eq!(row.attempts, 1);

        // Two more failures reach the budget -> settled, out of the demand.
        record_failure(&pool, "ISRC-A", Some("download timeout"))
            .await
            .unwrap();
        record_failure(&pool, "ISRC-A", Some("download timeout"))
            .await
            .unwrap();
        assert_eq!(get(&pool, "ISRC-A").await.unwrap().attempts, 3);
        assert!(demand_isrcs(&pool, 100).await.unwrap().is_empty());
        assert_eq!(demand_count(&pool).await, 0);

        // A settled failure can no longer be re-ordered.
        upsert_ordered(&pool, "ISRC-A", "order-3").await.unwrap();
        assert_eq!(get(&pool, "ISRC-A").await.unwrap().state, "failed");
    }

    #[tokio::test]
    async fn absent_is_terminal_and_never_retried() {
        let pool = test_db().await;
        make_backpack_track(&pool, 1, "ISRC-A").await;
        set_state(&pool, "ISRC-A", "absent", None, None, Some("not streamable"))
            .await
            .unwrap();

        assert!(demand_isrcs(&pool, 100).await.unwrap().is_empty());
        upsert_ordered(&pool, "ISRC-A", "order-1").await.unwrap();
        assert_eq!(get(&pool, "ISRC-A").await.unwrap().state, "absent");
    }

    #[tokio::test]
    async fn demand_zero_when_library_empty() {
        let pool = test_db().await;
        assert!(demand_isrcs(&pool, 100).await.unwrap().is_empty());
        assert_eq!(demand_count(&pool).await, 0);
    }

    #[tokio::test]
    async fn demand_includes_non_backpack_tracks() {
        let pool = test_db().await;
        make_backpack_track(&pool, 1, "ISRC-A").await;
        make_track(&pool, 2, "ISRC-Z").await; // not in the Backpack

        // Backlog tracks are ordered too, not just Backpack ones.
        assert_eq!(demand_isrcs(&pool, 100).await.unwrap(), vec!["ISRC-A", "ISRC-Z"]);
        assert_eq!(demand_count(&pool).await, 2);
    }

    #[tokio::test]
    async fn demand_orders_backpack_before_backlog() {
        let pool = test_db().await;
        // Backlog ISRC sorts *before* the Backpack ISRC alphabetically, so a
        // plain ISRC order would put it first — the Backpack must win anyway.
        make_track(&pool, 1, "ISRC-AAA").await;
        make_backpack_track(&pool, 2, "ISRC-ZZZ").await;

        assert_eq!(demand_isrcs(&pool, 100).await.unwrap(), vec!["ISRC-ZZZ", "ISRC-AAA"]);

        // The limit is spent on the Backpack first.
        assert_eq!(demand_isrcs(&pool, 1).await.unwrap(), vec!["ISRC-ZZZ"]);
    }

    #[tokio::test]
    async fn demand_skips_linked_and_known_backlog_tracks() {
        let pool = test_db().await;
        make_track(&pool, 1, "ISRC-A").await;
        make_track(&pool, 2, "ISRC-B").await;
        make_track(&pool, 3, "ISRC-C").await;

        // A linked file removes ISRC-B from the backlog.
        link_file(&pool, 10, 2, "ISRC-B").await;
        // A ledger row removes ISRC-C (e.g. terminal/absent).
        set_state(&pool, "ISRC-C", "absent", None, None, Some("nope"))
            .await
            .unwrap();

        assert_eq!(demand_isrcs(&pool, 100).await.unwrap(), vec!["ISRC-A"]);
        assert_eq!(demand_count(&pool).await, 1);
    }
}
