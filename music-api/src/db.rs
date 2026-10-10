//! SQLite storage: schema + queries.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{Pool, Sqlite};

use crate::models::{Order, OrderItem, Track, UrlOrder, UrlOrderItem, UrlTrack, state};

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
            priority       INTEGER NOT NULL DEFAULT 0,
            created_at     INTEGER NOT NULL,
            updated_at     INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;
    // Additive for pre-existing DBs.
    let _ = sqlx::query("ALTER TABLE tracks ADD COLUMN priority INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS orders (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            priority   INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;
    // Additive for pre-existing DBs.
    let _ = sqlx::query("ALTER TABLE orders ADD COLUMN priority INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await;

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

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tracks_state ON tracks(state);")
        .execute(pool)
        .await?;

    // ── URL-based orders (YouTube / SoundCloud via yt-dlp) ────────────────────
    //
    // Separate from the ISRC-keyed `tracks` table: these rows have no ISRC, so
    // they get their own table keyed by the source URL.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS url_tracks (
            id             INTEGER PRIMARY KEY,
            url            TEXT NOT NULL UNIQUE,
            provider       TEXT NOT NULL CHECK(provider IN ('youtube', 'soundcloud')),
            provider_id    TEXT,
            playlist_id    TEXT,
            title          TEXT,
            artist         TEXT,
            album          TEXT,
            duration_ms    INTEGER,
            state          TEXT NOT NULL,
            source_format  TEXT,
            path_flac      TEXT,
            path_320       TEXT,
            path_128       TEXT,
            error          TEXT,
            priority       INTEGER NOT NULL DEFAULT 0,
            created_at     INTEGER NOT NULL,
            updated_at     INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS url_orders (
            id         TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            priority   INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS url_order_items (
            order_id TEXT NOT NULL,
            url      TEXT NOT NULL,
            PRIMARY KEY (order_id, url)
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_url_tracks_state ON url_tracks(state);")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_url_tracks_priority ON url_tracks(priority DESC, created_at);")
        .execute(pool)
        .await?;

    Ok(())
}

/// Create an order for the given (already normalised, de-duplicated) ISRCs.
/// Unknown ISRCs are inserted as `pending`; known ones keep their state, so
/// re-ordering an already-downloaded track is a cheap no-op.
pub async fn create_order(
    pool: &Pool<Sqlite>,
    order_id: &str,
    isrcs: &[String],
) -> anyhow::Result<()> {
    let ts = now();
    let mut tx = pool.begin().await?;

    sqlx::query("INSERT INTO orders (id, status, created_at, updated_at) VALUES (?, 'open', ?, ?)")
        .bind(order_id)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;

    for isrc in isrcs {
        sqlx::query(
            "INSERT OR IGNORE INTO tracks (isrc, state, created_at, updated_at)
             VALUES (?, 'pending', ?, ?)",
        )
        .bind(isrc)
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
        "SELECT id, status, created_at, updated_at FROM orders WHERE id = ?",
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
            "SELECT id, status, created_at, updated_at FROM orders
              WHERE status = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(s)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as::<_, Order>(
            "SELECT id, status, created_at, updated_at FROM orders
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

pub async fn pending_tracks(pool: &Pool<Sqlite>, limit: i64) -> anyhow::Result<Vec<Track>> {
    Ok(sqlx::query_as::<_, Track>(
        "SELECT * FROM tracks WHERE state = 'pending'
          ORDER BY priority DESC, created_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn downloading_tracks(pool: &Pool<Sqlite>) -> anyhow::Result<Vec<Track>> {
    Ok(
        sqlx::query_as::<_, Track>("SELECT * FROM tracks WHERE state = 'downloading'")
            .fetch_all(pool)
            .await?,
    )
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
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE tracks SET state = 'downloading', deemix_uuid = ?, error = NULL, updated_at = ?
          WHERE isrc = ?",
    )
    .bind(deemix_uuid)
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
                error = NULL, updated_at = ?
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
    sqlx::query("UPDATE tracks SET state = ?, error = ?, updated_at = ? WHERE isrc = ?")
        .bind(new_state)
        .bind(error)
        .bind(now())
        .bind(isrc)
        .execute(pool)
        .await?;
    refresh_orders_for_isrc(pool, isrc).await
}

// ── URL-based orders (YouTube / SoundCloud) ──────────────────────────────────

/// Create a URL order. Unknown URLs are inserted as `pending`; known ones keep
/// their state, so re-ordering is a cheap no-op.
pub async fn create_url_order(
    pool: &Pool<Sqlite>,
    order_id: &str,
    urls: &[String],
) -> anyhow::Result<()> {
    let ts = now();
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO url_orders (id, status, priority, created_at, updated_at)
         VALUES (?, 'open', 0, ?, ?)",
    )
    .bind(order_id)
    .bind(ts)
    .bind(ts)
    .execute(&mut *tx)
    .await?;

    for url in urls {
        let provider = crate::ytdlp::provider_for_url(url);
        sqlx::query(
            "INSERT OR IGNORE INTO url_tracks (url, provider, state, created_at, updated_at)
             VALUES (?, ?, 'pending', ?, ?)",
        )
        .bind(url)
        .bind(provider)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;

        sqlx::query("INSERT OR IGNORE INTO url_order_items (order_id, url) VALUES (?, ?)")
            .bind(order_id)
            .bind(url)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    refresh_url_order(pool, order_id).await?;
    Ok(())
}

/// Derive a URL order's status from its items.
pub async fn refresh_url_order(pool: &Pool<Sqlite>, order_id: &str) -> anyhow::Result<String> {
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT t.state FROM url_order_items oi
           JOIN url_tracks t ON t.url = oi.url
          WHERE oi.order_id = ?",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    let status = derive_order_status(&states);
    sqlx::query("UPDATE url_orders SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(now())
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(status.to_string())
}

/// Refresh every URL order that contains this URL.
pub async fn refresh_url_orders_for_url(pool: &Pool<Sqlite>, url: &str) -> anyhow::Result<()> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT order_id FROM url_order_items WHERE url = ?")
        .bind(url)
        .fetch_all(pool)
        .await?;
    for id in ids {
        refresh_url_order(pool, &id).await?;
    }
    Ok(())
}

pub async fn get_url_order(pool: &Pool<Sqlite>, id: &str) -> anyhow::Result<Option<UrlOrder>> {
    Ok(sqlx::query_as::<_, UrlOrder>(
        "SELECT id, status, priority, created_at, updated_at FROM url_orders WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_url_orders(
    pool: &Pool<Sqlite>,
    status: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<UrlOrder>> {
    match status {
        Some(s) => Ok(sqlx::query_as::<_, UrlOrder>(
            "SELECT id, status, priority, created_at, updated_at FROM url_orders
              WHERE status = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(s)
        .bind(limit)
        .fetch_all(pool)
        .await?),
        None => Ok(sqlx::query_as::<_, UrlOrder>(
            "SELECT id, status, priority, created_at, updated_at FROM url_orders
              ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await?),
    }
}

pub async fn url_order_items(
    pool: &Pool<Sqlite>,
    order_id: &str,
) -> anyhow::Result<Vec<UrlOrderItem>> {
    let rows: Vec<UrlTrack> = sqlx::query_as::<_, UrlTrack>(
        "SELECT t.* FROM url_order_items oi
           JOIN url_tracks t ON t.url = oi.url
          WHERE oi.order_id = ?
          ORDER BY t.url",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|t| UrlOrderItem {
            url: t.url.clone(),
            provider: Some(t.provider.clone()),
            provider_id: t.provider_id.clone(),
            state: t.state.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            formats: t.formats().into_iter().map(|s| s.to_string()).collect(),
            error: t.error.clone(),
        })
        .collect())
}

pub async fn get_url_track(pool: &Pool<Sqlite>, url: &str) -> anyhow::Result<Option<UrlTrack>> {
    Ok(
        sqlx::query_as::<_, UrlTrack>("SELECT * FROM url_tracks WHERE url = ?")
            .bind(url)
            .fetch_optional(pool)
            .await?,
    )
}

/// Pending URL tracks, highest priority first, then oldest.
pub async fn pending_url_tracks(pool: &Pool<Sqlite>, limit: i64) -> anyhow::Result<Vec<UrlTrack>> {
    Ok(sqlx::query_as::<_, UrlTrack>(
        "SELECT * FROM url_tracks WHERE state = 'pending'
          ORDER BY priority DESC, created_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn downloading_url_tracks(pool: &Pool<Sqlite>) -> anyhow::Result<Vec<UrlTrack>> {
    Ok(
        sqlx::query_as::<_, UrlTrack>("SELECT * FROM url_tracks WHERE state = 'downloading'")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn mark_url_resolved(
    pool: &Pool<Sqlite>,
    url: &str,
    provider_id: &str,
    title: &str,
    artist: &str,
    duration_ms: Option<i64>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE url_tracks SET provider_id = ?, title = ?, artist = ?, duration_ms = ?, updated_at = ?
          WHERE url = ?",
    )
    .bind(provider_id)
    .bind(title)
    .bind(artist)
    .bind(duration_ms)
    .bind(now())
    .bind(url)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_url_downloading(pool: &Pool<Sqlite>, url: &str) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE url_tracks SET state = 'downloading', error = NULL, updated_at = ? WHERE url = ?",
    )
    .bind(now())
    .bind(url)
    .execute(pool)
    .await?;
    refresh_url_orders_for_url(pool, url).await
}

pub async fn mark_url_ready(
    pool: &Pool<Sqlite>,
    url: &str,
    source_format: &str,
    path_flac: Option<&str>,
    path_320: Option<&str>,
    path_128: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE url_tracks
            SET state = 'ready', source_format = ?, path_flac = ?, path_320 = ?, path_128 = ?,
                error = NULL, updated_at = ?
          WHERE url = ?",
    )
    .bind(source_format)
    .bind(path_flac)
    .bind(path_320)
    .bind(path_128)
    .bind(now())
    .bind(url)
    .execute(pool)
    .await?;
    refresh_url_orders_for_url(pool, url).await
}

pub async fn mark_url_terminal(
    pool: &Pool<Sqlite>,
    url: &str,
    new_state: &str,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE url_tracks SET state = ?, error = ?, updated_at = ? WHERE url = ?")
        .bind(new_state)
        .bind(error)
        .bind(now())
        .bind(url)
        .execute(pool)
        .await?;
    refresh_url_orders_for_url(pool, url).await
}

/// Set the priority of a URL order and all its tracks.
pub async fn set_url_order_priority(
    pool: &Pool<Sqlite>,
    order_id: &str,
    priority: i64,
) -> anyhow::Result<()> {
    let ts = now();
    sqlx::query("UPDATE url_orders SET priority = ?, updated_at = ? WHERE id = ?")
        .bind(priority)
        .bind(ts)
        .bind(order_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "UPDATE url_tracks SET priority = ?, updated_at = ?
          WHERE url IN (SELECT url FROM url_order_items WHERE order_id = ?)",
    )
    .bind(priority)
    .bind(ts)
    .bind(order_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Set the status of a URL order (`open`, `paused`, `cancelled`).
pub async fn set_url_order_status(
    pool: &Pool<Sqlite>,
    order_id: &str,
    status: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE url_orders SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(now())
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set the priority of a single URL track.
pub async fn set_url_track_priority(
    pool: &Pool<Sqlite>,
    url: &str,
    priority: i64,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE url_tracks SET priority = ?, updated_at = ? WHERE url = ?")
        .bind(priority)
        .bind(now())
        .bind(url)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_url_track_by_id(pool: &Pool<Sqlite>, id: i64) -> anyhow::Result<Option<UrlTrack>> {
    Ok(
        sqlx::query_as::<_, UrlTrack>("SELECT * FROM url_tracks WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Set the priority of an ISRC order and all its tracks.
pub async fn set_order_priority(
    pool: &Pool<Sqlite>,
    order_id: &str,
    priority: i64,
) -> anyhow::Result<()> {
    let ts = now();
    sqlx::query("UPDATE orders SET priority = ?, updated_at = ? WHERE id = ?")
        .bind(priority)
        .bind(ts)
        .bind(order_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "UPDATE tracks SET priority = ?, updated_at = ?
          WHERE isrc IN (SELECT isrc FROM order_items WHERE order_id = ?)",
    )
    .bind(priority)
    .bind(ts)
    .bind(order_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Set the status of an ISRC order (`open`, `paused`, `cancelled`).
pub async fn set_order_status(
    pool: &Pool<Sqlite>,
    order_id: &str,
    status: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE orders SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(now())
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(())
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
}
