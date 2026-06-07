//! Playlist-related database queries — CRUD, subscriptions, sync tracking, tag resolution.

use anyhow::Result;
use sqlx::{Pool, Sqlite, SqliteConnection};
use tracing::{debug, info};

use super::types::*;

// ── Playlist Queries ────────────────────────────────────────────────────

/// Get all playlists that don't have corresponding tags
pub async fn get_playlists_without_tags(pool: &Pool<Sqlite>) -> Result<Vec<ServicePlaylist>> {
    let playlists = sqlx::query_as::<_, ServicePlaylist>(
        r#"
        SELECT DISTINCT sp.*
        FROM service_playlists sp
        WHERE TRIM(sp.name) != ''
          AND NOT EXISTS (
            SELECT 1 FROM v_tag_playlist vtp WHERE vtp.playlist_id = sp.id
          )
        ORDER BY sp.name
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(playlists)
}

/// Create tags from playlists that don't have corresponding tags
/// Returns the number of tags created
pub async fn create_tags_from_playlists(pool: &Pool<Sqlite>) -> Result<usize> {
    // Get default tag category (Setlist)
    let default_category = match crate::db::get_default_tag_category(pool).await? {
        Some(category) => category,
        None => return Err(anyhow::anyhow!("No default tag category found")),
    };

    // Insert tags for playlists without tags
    let result = sqlx::query(
        r#"
        INSERT INTO tags (name, category_id, created_at)
        SELECT DISTINCT
            TRIM(sp.name) as name,
            ? as category_id,
            unixepoch() as created_at
        FROM service_playlists sp
        WHERE TRIM(sp.name) != ''
          AND NOT EXISTS (
            SELECT 1 FROM v_tag_playlist vtp WHERE vtp.playlist_id = sp.id
          )
        "#,
    )
    .bind(default_category.id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() as usize)
}

// ── Service Playlist/Track CRUD ─────────────────────────────────────────

/// Create or update a service playlist
pub async fn upsert_service_playlist(
    conn: &mut SqliteConnection,
    service: &str,
    playlist_id: &str,
    name: &str,
    description: Option<&str>,
    metadata_json: Option<&str>,
) -> Result<ServicePlaylist> {
    let now = chrono::Utc::now().timestamp();
    let row = sqlx::query_as::<_, ServicePlaylist>(
        r#"
        INSERT INTO service_playlists (service, playlist_id, name, description, metadata_json, imported_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(service, playlist_id) DO UPDATE SET
            name = excluded.name,
            description = excluded.description,
            metadata_json = excluded.metadata_json,
            updated_at = excluded.updated_at
        RETURNING *
        "#,
    )
    .bind(service)
    .bind(playlist_id)
    .bind(name)
    .bind(description)
    .bind(metadata_json)
    .bind(now)
    .bind(now)
    .fetch_one(conn)
    .await?;
    Ok(row)
}

/// Update per-playlist fetch tracking after a successful sync.
/// Sets `last_fetched_at` to now, `remote_track_count` from Spotify's
/// `tracks.total` (all items including duplicates/episodes), and
/// `remote_unique_count` computed from the actual stored track count
/// (unique tracks only).
pub async fn update_playlist_fetch_tracking(
    conn: &mut SqliteConnection,
    service: &str,
    playlist_id: &str,
    remote_track_count: i64,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    // Compute unique count from the DB (after sync, this equals distinct
    // tracks in the stream — INSERT OR IGNORE filters out duplicates)
    let unique_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        WHERE sp.service = ?1 AND sp.playlist_id = ?2 AND spt.deleted_at IS NULL
        "#,
    )
    .bind(service)
    .bind(playlist_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap_or(0);

    sqlx::query(
        r#"
        UPDATE service_playlists
        SET last_fetched_at = ?1,
            remote_track_count = ?2,
            remote_unique_count = ?3,
            updated_at = ?4
        WHERE service = ?5 AND playlist_id = ?6
        "#,
    )
    .bind(now)
    .bind(remote_track_count)
    .bind(unique_count)
    .bind(now)
    .bind(service)
    .bind(playlist_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Update only the remote_track_count for a playlist (no last_fetched_at or unique_count).
/// Used by the playlist-list sync where we get counts from SimplifiedPlaylist.tracks.total
/// but haven't actually fetched tracks yet.
pub async fn update_playlist_remote_count(
    conn: &mut SqliteConnection,
    service: &str,
    playlist_id: &str,
    remote_track_count: i64,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        r#"
        UPDATE service_playlists
        SET remote_track_count = ?1,
            updated_at = ?2
        WHERE service = ?3 AND playlist_id = ?4
        "#,
    )
    .bind(remote_track_count)
    .bind(now)
    .bind(service)
    .bind(playlist_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Create or update a service track in the database
#[allow(clippy::too_many_arguments)]
pub async fn upsert_service_track(
    conn: &mut SqliteConnection,
    service: &str,
    service_id: &str,
    title: &str,
    artist: &str,
    album: Option<&str>,
    isrc: Option<&str>,
    duration_ms: Option<i64>,
    metadata_json: Option<&str>,
) -> Result<ServiceTrack> {
    let now = chrono::Utc::now().timestamp();

    let row = sqlx::query_as::<_, ServiceTrack>(
        r#"
        INSERT INTO service_tracks (service, service_id, title, artist, album, isrc, duration_ms, metadata_json, imported_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(service, service_id) DO UPDATE SET
            title = excluded.title,
            artist = excluded.artist,
            album = excluded.album,
            isrc = excluded.isrc,
            duration_ms = excluded.duration_ms,
            metadata_json = excluded.metadata_json,
            updated_at = excluded.updated_at
        RETURNING *
        "#,
    )
    .bind(service)
    .bind(service_id)
    .bind(title)
    .bind(artist)
    .bind(album)
    .bind(isrc)
    .bind(duration_ms)
    .bind(metadata_json)
    .bind(now)
    .bind(now)
    .fetch_one(conn)
    .await?;

    Ok(row)
}

/// Add a track to a playlist with optional position
pub async fn add_track_to_playlist(
    conn: &mut SqliteConnection,
    playlist_id: i64,
    track_id: i64,
    position: Option<i32>,
) -> Result<()> {
    add_track_to_playlist_with_added_at(conn, playlist_id, track_id, position, None).await
}

/// Add a track to a playlist with an explicit `added_at` timestamp.
/// When `added_at` is `None`, defaults to the current time.
pub async fn add_track_to_playlist_with_added_at(
    conn: &mut SqliteConnection,
    playlist_id: i64,
    track_id: i64,
    position: Option<i32>,
    added_at: Option<i64>,
) -> Result<()> {
    let pos = position.unwrap_or(0);
    let added_at = added_at.unwrap_or_else(|| chrono::Utc::now().timestamp());

    sqlx::query(
        r#"
        INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at, deleted_at)
        VALUES (?, ?, ?, ?, NULL)
        ON CONFLICT(playlist_id, track_id) DO UPDATE SET
            position = excluded.position,
            added_at = excluded.added_at,
            deleted_at = NULL
        "#,
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(pos)
    .bind(added_at)
    .execute(conn)
    .await?;

    Ok(())
}

/// Mark all active tracks in a playlist as soft-deleted.
/// Used before re-syncing from Spotify — tracks no longer in the stream remain deleted.
pub async fn mark_playlist_tracks_deleted(
    conn: &mut SqliteConnection,
    playlist_id: i64,
) -> Result<u64> {
    let now = chrono::Utc::now().timestamp();
    let rows = sqlx::query(
        "UPDATE service_playlist_tracks SET deleted_at = ? WHERE playlist_id = ? AND deleted_at IS NULL"
    )
    .bind(now)
    .bind(playlist_id)
    .execute(conn)
    .await?;
    Ok(rows.rows_affected())
}

/// Toggle the archive_deleted flag for a playlist.
/// When true: deleted tracks remain active for tag resolution.
/// When false: deleted tracks are excluded from tag resolution.
pub async fn set_playlist_archive_deleted(
    pool: &Pool<Sqlite>,
    playlist_id: i64,
    archive: bool,
) -> Result<()> {
    sqlx::query("UPDATE service_playlists SET archive_deleted = ? WHERE id = ?")
        .bind(archive)
        .bind(playlist_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a playlist and its track associations (cascade).
pub async fn delete_playlist(pool: &Pool<Sqlite>, playlist_id: i64) -> Result<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM service_playlist_tracks WHERE playlist_id = ?")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM service_playlists WHERE id = ?")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Get a service playlist by service and playlist ID
pub async fn get_service_playlist_by_id(
    pool: &Pool<Sqlite>,
    service: &str,
    playlist_id: &str,
) -> Result<Option<ServicePlaylist>> {
    let row = sqlx::query_as::<_, ServicePlaylist>(
        "SELECT * FROM service_playlists WHERE service = ? AND playlist_id = ?",
    )
    .bind(service)
    .bind(playlist_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Get all tracks in a playlist
pub async fn get_playlist_tracks(
    pool: &Pool<Sqlite>,
    playlist_id: i64,
) -> Result<Vec<ServiceTrack>> {
    let rows = sqlx::query_as::<_, ServiceTrack>(
        r#"
        SELECT st.* FROM service_tracks st
        JOIN service_playlist_tracks spt ON st.id = spt.track_id
        WHERE spt.playlist_id = ?
        ORDER BY spt.position
        "#,
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get all tracks in a playlist by name
pub async fn get_playlist_tracks_by_name(
    pool: &Pool<Sqlite>,
    playlist_name: &str,
) -> Result<Vec<ServiceTrack>> {
    let rows = sqlx::query_as::<_, ServiceTrack>(
        r#"
        SELECT st.*
        FROM service_tracks st
        JOIN service_playlist_tracks spt ON st.id = spt.track_id
        JOIN service_playlists sp ON spt.playlist_id = sp.id
        WHERE sp.name = ?
        ORDER BY sp.service, spt.position
        "#,
    )
    .bind(playlist_name)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ── Tag Resolution ──────────────────────────────────────────────────────

/// Refresh track tags by creating tags from playlist names that don't exist yet
pub async fn refresh_track_tags(pool: &Pool<Sqlite>) -> Result<()> {
    // Get default tag category (Setlist)
    let default_category = match crate::db::get_default_tag_category(pool).await? {
        Some(category) => category,
        None => return Err(anyhow::anyhow!("No default tag category found")),
    };

    // Find all unique playlist names that don't have matching tags
    let unmatched_playlists = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT sp.name
        FROM service_playlists sp
        LEFT JOIN tags t ON sp.name = t.name COLLATE NOCASE
        WHERE t.id IS NULL
        ORDER BY sp.name
        "#,
    )
    .fetch_all(pool)
    .await?;

    // Create tags for unmatched playlist names
    for playlist_name in &unmatched_playlists {
        // Check if tag already exists (case-insensitive)
        if let Ok(Some(_)) = crate::db::get_tag_by_name(pool, playlist_name).await {
            continue;
        }

        // Create new tag with default category
        crate::db::create_tag(pool, playlist_name, default_category.id).await?;
        debug!("Created tag from playlist name: {}", playlist_name);
    }

    info!(
        "Refreshed track tags: created {} new tags",
        unmatched_playlists.len()
    );
    Ok(())
}

/// Ensure a tag exists for a playlist name (case-insensitive match)
pub async fn ensure_tag_for_playlist_name(pool: &Pool<Sqlite>, playlist_name: &str) -> Result<Tag> {
    // Check if tag already exists (case-insensitive)
    match crate::db::get_tag_by_name(pool, playlist_name).await {
        Ok(Some(existing_tag)) => return Ok(existing_tag),
        Ok(None) => (),          // Tag doesn't exist, continue
        Err(e) => return Err(e), // Propagate error
    }

    // Get default tag category (Setlist)
    let default_category = match crate::db::get_default_tag_category(pool).await? {
        Some(category) => category,
        None => return Err(anyhow::anyhow!("No default tag category found")),
    };

    // Create new tag with default category
    let tag = crate::db::create_tag(pool, playlist_name, default_category.id).await?;
    debug!("Created tag for playlist name: {}", playlist_name);
    Ok(tag)
}

/// Truncate and repopulate `file_resolved_tags` from the `v_file_resolved_tags` view.
/// Then resolves tag bundles transitively.
/// Call this after any tag/playlist/track sync. Also refresh track_resolved_tags when appropriate.
/// Returns the number of rows inserted.
pub async fn refresh_file_resolved_tags(pool: &Pool<Sqlite>) -> Result<u64> {
    // Wrap DELETE + INSERT in a single transaction so they run on the same
    // connection and are visible atomically to other pool connections.
    let mut tx = pool.begin().await?;

    // Truncate the table
    sqlx::query("DELETE FROM file_resolved_tags")
        .execute(&mut *tx)
        .await?;

    // Step 1: Repopulate from the view
    let changed: i64 = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT OR IGNORE INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at, is_default)
        SELECT DISTINCT
            vfr.file_id, vfr.tag_id, vfr.tag_name, vfr.category_id, vfr.category_name, vfr.prefix,
            vfr.sort_order, vfr.created_at,
            COALESCE((SELECT tc.is_default FROM tag_categories tc WHERE tc.id = vfr.category_id), 0)
        FROM v_file_resolved_tags vfr;
        SELECT CHANGES();
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;

    // Step 2: Resolve tag bundles transitively
    // For each bundle tag, find files that have any of its member tags.
    // Repeat until no new rows are inserted (handles multi-level bundles).
    let bundle_changed: i64 = {
        let mut total: i64 = 0;
        let mut iteration_count = 0u32;
        loop {
            let inserted: i64 = sqlx::query_scalar(
                r#"
                INSERT OR IGNORE INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at, is_default)
                SELECT DISTINCT
                    frt.file_id,
                    t.id AS tag_id,
                    t.name AS tag_name,
                    tc.id AS category_id,
                    tc.name AS category_name,
                    tc.prefix,
                    tc.sort_order,
                    t.created_at,
                    COALESCE(tc.is_default, 0) AS is_default
                FROM tag_bundles tb
                JOIN file_resolved_tags frt ON frt.tag_id = tb.member_tag_id
                JOIN tags t ON t.id = tb.bundle_tag_id
                JOIN tag_categories tc ON tc.id = t.category_id
                WHERE NOT EXISTS (
                    SELECT 1 FROM file_resolved_tags frt2
                    WHERE frt2.file_id = frt.file_id AND frt2.tag_id = t.id
                );
                SELECT CHANGES();
                "#,
            )
            .fetch_one(&mut *tx)
            .await?;

            if inserted == 0 {
                break; // No more bundle propagation possible
            }
            total += inserted;
            iteration_count += 1;
            if iteration_count > 20 {
                tracing::warn!(
                    "Tag bundle resolution exceeded max iterations (20). Possible deep chain or circular reference."
                );
                break;
            }
        }
        total
    };

    tx.commit().await?;

    let count = (changed + bundle_changed) as u64;
    tracing::info!(
        "Refreshed file_resolved_tags: {} rows ({} from view, {} from bundles)",
        count,
        changed,
        bundle_changed
    );
    Ok(count)
}

/// Truncate and repopulate `track_resolved_tags` from the `v_track_tags` view.
/// Then resolves tag bundles transitively.
/// Call this after any tag/playlist/track sync.
/// Returns the number of rows inserted.
pub async fn refresh_track_resolved_tags(pool: &Pool<Sqlite>) -> Result<u64> {
    // Wrap DELETE + INSERT in a single transaction so they run on the same
    // connection and are visible atomically to other pool connections.
    let mut tx = pool.begin().await?;

    // Truncate the table
    sqlx::query("DELETE FROM track_resolved_tags")
        .execute(&mut *tx)
        .await?;

    // Step 1: Repopulate from the view
    let changed: i64 = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT OR IGNORE INTO track_resolved_tags (track_id, tag_id, tag_name, category_id, category_name, prefix, is_default)
        SELECT DISTINCT
            vtt.track_id, vtt.tag_id, vtt.tag_name, vtt.category_id, vtt.category_name, vtt.prefix, vtt.is_default
        FROM v_track_tags vtt;
        SELECT CHANGES();
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;

    // Step 2: Resolve tag bundles transitively
    let bundle_changed: i64 = {
        let mut total: i64 = 0;
        let mut iteration_count = 0u32;
        loop {
            let inserted: i64 = sqlx::query_scalar(
                r#"
                INSERT OR IGNORE INTO track_resolved_tags (track_id, tag_id, tag_name, category_id, category_name, prefix, is_default)
                SELECT DISTINCT
                    trt.track_id,
                    t.id AS tag_id,
                    t.name AS tag_name,
                    tc.id AS category_id,
                    tc.name AS category_name,
                    tc.prefix,
                    COALESCE(tc.is_default, 0) AS is_default
                FROM tag_bundles tb
                JOIN track_resolved_tags trt ON trt.tag_id = tb.member_tag_id
                JOIN tags t ON t.id = tb.bundle_tag_id
                JOIN tag_categories tc ON tc.id = t.category_id
                WHERE NOT EXISTS (
                    SELECT 1 FROM track_resolved_tags trt2
                    WHERE trt2.track_id = trt.track_id AND trt2.tag_id = t.id
                );
                SELECT CHANGES();
                "#,
            )
            .fetch_one(&mut *tx)
            .await?;

            if inserted == 0 {
                break;
            }
            total += inserted;
            iteration_count += 1;
            if iteration_count > 20 {
                tracing::warn!(
                    "Tag bundle resolution for tracks exceeded max iterations (20). Possible deep chain or circular reference."
                );
                break;
            }
        }
        total
    };

    tx.commit().await?;

    let count = (changed + bundle_changed) as u64;
    tracing::info!(
        "Refreshed track_resolved_tags: {} rows ({} from view, {} from bundles)",
        count,
        changed,
        bundle_changed
    );
    Ok(count)
}

// ── Playlist Snapshot / Staleness ───────────────────────────────────────

/// Get all Spotify playlist IDs + snapshot IDs for change detection.
pub async fn get_spotify_playlist_snapshots(
    pool: &Pool<Sqlite>,
) -> Result<Vec<(i64, String, Option<String>)>> {
    let rows = sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT id, playlist_id, snapshot_id FROM service_playlists WHERE service = 'spotify'",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Update the snapshot_id for a playlist identified by its service and playlist_id.
pub async fn update_playlist_snapshot(
    pool: &Pool<Sqlite>,
    playlist_id: &str,
    snapshot_id: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE service_playlists SET snapshot_id = ?1, updated_at = ?2 \
         WHERE service = 'spotify' AND playlist_id = ?3",
    )
    .bind(snapshot_id)
    .bind(now)
    .bind(playlist_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get playlist staleness info by DB id.
/// Returns (local_count, remote_unique_count, remote_track_count, last_fetched_at).
pub async fn get_playlist_staleness(
    pool: &Pool<Sqlite>,
    db_playlist_id: i64,
) -> Result<(i64, i64, i64, Option<i64>)> {
    let row = sqlx::query_as::<_, (i64, i64, i64, Option<i64>)>(
        r#"
        SELECT
            COALESCE((SELECT COUNT(*) FROM service_playlist_tracks WHERE playlist_id = ?), 0) AS local_count,
            COALESCE(remote_unique_count, 0),
            COALESCE(remote_track_count, 0),
            last_fetched_at
        FROM service_playlists
        WHERE id = ?
        "#,
    )
    .bind(db_playlist_id)
    .bind(db_playlist_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Get snapshot and remote info for a subscription's linked service_playlist.
/// Returns (snapshot_id, remote_unique_count, remote_track_count, last_fetched_at).
pub async fn get_subscription_playlist_info(
    pool: &Pool<Sqlite>,
    service_playlist_id: i64,
) -> Result<(Option<String>, i64, i64, Option<i64>)> {
    let row = sqlx::query_as::<_, (Option<String>, i64, i64, Option<i64>)>(
        r#"
        SELECT
            snapshot_id,
            COALESCE(remote_unique_count, 0),
            COALESCE(remote_track_count, 0),
            last_fetched_at
        FROM service_playlists
        WHERE id = ?
        "#,
    )
    .bind(service_playlist_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Mark a playlist as inactive (snapshot cleared, no longer live on Spotify).
pub async fn mark_playlist_inactive(pool: &Pool<Sqlite>, db_id: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE service_playlists SET snapshot_id = NULL, updated_at = ?1 WHERE id = ?2")
        .bind(now)
        .bind(db_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Subscriptions ───────────────────────────────────────────────────────

/// A playlist subscription — tracks a remote playlist for polling.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlaylistSubscription {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub service_playlist_id: Option<i64>,
    pub subscribed_at: i64,
    pub last_polled_at: Option<i64>,
    pub poll_interval_secs: i64,
    pub is_active: bool,
    pub playlist_name: Option<String>,
    pub track_count: i64,
}

/// Subscribe to a playlist. If already subscribed (INSERT OR IGNORE),
/// returns the existing subscription id.
pub async fn subscribe_to_playlist(
    pool: &Pool<Sqlite>,
    service: &str,
    playlist_id: &str,
    db_playlist_id: Option<i64>,
) -> Result<i64> {
    let _result = sqlx::query(
        r#"
        INSERT OR IGNORE INTO playlist_subscriptions (service, playlist_id, service_playlist_id)
        VALUES (?, ?, ?)
        "#,
    )
    .bind(service)
    .bind(playlist_id)
    .bind(db_playlist_id)
    .execute(pool)
    .await?;

    // Get the id of the subscription (existing or just inserted)
    let row = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM playlist_subscriptions WHERE service = ? AND playlist_id = ?",
    )
    .bind(service)
    .bind(playlist_id)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// Unsubscribe from a playlist (delete the subscription).
pub async fn unsubscribe_from_playlist(pool: &Pool<Sqlite>, subscription_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM playlist_subscriptions WHERE id = ?")
        .bind(subscription_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// List all subscriptions, joining with service_playlists to get playlist name and track count.
pub async fn list_subscriptions(pool: &Pool<Sqlite>) -> Result<Vec<PlaylistSubscription>> {
    let rows = sqlx::query_as::<_, PlaylistSubscription>(
        "SELECT * FROM v_subscriptions ORDER BY subscribed_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get subscriptions that are due for polling (is_active AND not polled recently).
pub async fn get_due_subscriptions(pool: &Pool<Sqlite>) -> Result<Vec<PlaylistSubscription>> {
    let rows = sqlx::query_as::<_, PlaylistSubscription>(
        "SELECT * FROM v_subscriptions
         WHERE is_active = 1
           AND (last_polled_at IS NULL OR last_polled_at + poll_interval_secs < unixepoch())
         ORDER BY subscribed_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Update the last_polled_at timestamp to now.
pub async fn update_subscription_last_polled(
    pool: &Pool<Sqlite>,
    subscription_id: i64,
) -> Result<()> {
    sqlx::query("UPDATE playlist_subscriptions SET last_polled_at = unixepoch() WHERE id = ?")
        .bind(subscription_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update the service_playlist_id for a subscription.
pub async fn update_subscription_playlist_id(
    pool: &Pool<Sqlite>,
    subscription_id: i64,
    service_playlist_id: i64,
) -> Result<()> {
    sqlx::query("UPDATE playlist_subscriptions SET service_playlist_id = ? WHERE id = ?")
        .bind(service_playlist_id)
        .bind(subscription_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all playlist associations for a given track: (playlist_name, playlist_id, service).
pub async fn get_track_playlist_associations(
    pool: &Pool<Sqlite>,
    track_id: i64,
) -> Result<Vec<(String, String, String)>> {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT sp.name, sp.playlist_id, sp.service
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON spt.playlist_id = sp.id
        WHERE spt.track_id = ?
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Check if a playlist is subscribed (by service + playlist_id).
pub async fn is_playlist_subscribed(
    pool: &Pool<Sqlite>,
    service: &str,
    playlist_id: &str,
) -> Result<Option<PlaylistSubscription>> {
    row_by_service_and_playlist_id(pool, service, playlist_id).await
}

/// Get a subscription by service + playlist_id (more explicit name).
pub async fn get_subscription_by_playlist_id(
    pool: &Pool<Sqlite>,
    service: &str,
    playlist_id: &str,
) -> Result<Option<PlaylistSubscription>> {
    row_by_service_and_playlist_id(pool, service, playlist_id).await
}

/// Shared helper: fetch a subscription by service + playlist_id with JOIN.
async fn row_by_service_and_playlist_id(
    pool: &Pool<Sqlite>,
    service: &str,
    playlist_id: &str,
) -> Result<Option<PlaylistSubscription>> {
    let row = sqlx::query_as::<_, PlaylistSubscription>(
        "SELECT * FROM v_subscriptions WHERE service = ? AND playlist_id = ?",
    )
    .bind(service)
    .bind(playlist_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Create an in-memory SQLite DB with the minimal schema for playlist tests.
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        // service_playlists: core playlist table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS service_playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                metadata_json TEXT,
                imported_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                last_fetched_at INTEGER,
                remote_track_count INTEGER NOT NULL DEFAULT 0,
                remote_unique_count INTEGER NOT NULL DEFAULT 0,
                archive_deleted INTEGER NOT NULL DEFAULT 0,
                snapshot_id TEXT,
                UNIQUE(service, playlist_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // service_playlist_tracks: track membership
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS service_playlist_tracks (
                playlist_id INTEGER NOT NULL REFERENCES service_playlists(id),
                track_id INTEGER NOT NULL,
                position INTEGER,
                added_at INTEGER NOT NULL DEFAULT 0,
                deleted_at INTEGER,
                PRIMARY KEY (playlist_id, track_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // playlist_subscriptions: polling subscriptions
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS playlist_subscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                service_playlist_id INTEGER REFERENCES service_playlists(id),
                subscribed_at INTEGER NOT NULL DEFAULT (unixepoch()),
                last_polled_at INTEGER,
                poll_interval_secs INTEGER NOT NULL DEFAULT 300,
                is_active INTEGER NOT NULL DEFAULT 1,
                UNIQUE(service, playlist_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // service_tracks: needed for get_playlist_tracks / get_playlist_tracks_by_name
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS service_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                service_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT,
                isrc TEXT,
                duration_ms INTEGER,
                metadata_json TEXT,
                imported_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                UNIQUE(service, service_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // v_subscriptions view (mirrors migration 001)
        sqlx::query(
            "CREATE VIEW IF NOT EXISTS v_subscriptions AS
             SELECT ps.*, sp.name AS playlist_name,
               COALESCE((SELECT COUNT(*) FROM service_playlist_tracks spt WHERE spt.playlist_id = sp.id), 0) AS track_count
             FROM playlist_subscriptions ps
             LEFT JOIN service_playlists sp ON ps.service_playlist_id = sp.id",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    // ── Subscription CRUD ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_subscribe_and_list() {
        let pool = test_db().await;

        let id = subscribe_to_playlist(&pool, "spotify", "pl-123", None)
            .await
            .unwrap();
        assert!(id > 0, "should return a valid subscription id");

        let subs = list_subscriptions(&pool).await.unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].service, "spotify");
        assert_eq!(subs[0].playlist_id, "pl-123");
        assert!(subs[0].is_active);
    }

    #[tokio::test]
    async fn test_subscribe_idempotent() {
        let pool = test_db().await;

        let id1 = subscribe_to_playlist(&pool, "spotify", "pl-dup", None)
            .await
            .unwrap();
        let id2 = subscribe_to_playlist(&pool, "spotify", "pl-dup", None)
            .await
            .unwrap();
        // INSERT OR IGNORE should return the same id
        assert_eq!(id1, id2);

        let subs = list_subscriptions(&pool).await.unwrap();
        assert_eq!(subs.len(), 1);
    }

    #[tokio::test]
    async fn test_subscribe_with_playlist_id() {
        let pool = test_db().await;

        // Create a playlist first
        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-linked', 'Linked Playlist', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let sub_id = subscribe_to_playlist(&pool, "spotify", "pl-linked", Some(pl_id))
            .await
            .unwrap();
        assert!(sub_id > 0);

        let subs = list_subscriptions(&pool).await.unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].playlist_name.as_deref(), Some("Linked Playlist"));
        assert_eq!(subs[0].service_playlist_id, Some(pl_id));
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let pool = test_db().await;

        let id = subscribe_to_playlist(&pool, "soundcloud", "sc-1", None)
            .await
            .unwrap();

        // Verify it's there
        let subs = list_subscriptions(&pool).await.unwrap();
        assert_eq!(subs.len(), 1);

        // Unsubscribe
        unsubscribe_from_playlist(&pool, id).await.unwrap();

        // Verify gone
        let subs = list_subscriptions(&pool).await.unwrap();
        assert_eq!(subs.len(), 0);
    }

    #[tokio::test]
    async fn test_get_due_subscriptions() {
        let pool = test_db().await;

        // Subscribe — due because never polled
        let _id1 = subscribe_to_playlist(&pool, "spotify", "pl-due", None)
            .await
            .unwrap();

        // Subscribe and mark as recently polled — not due
        let _id2 = subscribe_to_playlist(&pool, "spotify", "pl-recent", None)
            .await
            .unwrap();
        update_subscription_last_polled(&pool, _id2).await.unwrap();

        let due = get_due_subscriptions(&pool).await.unwrap();
        assert_eq!(due.len(), 1, "only the unpolled sub should be due");
        assert_eq!(due[0].playlist_id, "pl-due");
    }

    #[tokio::test]
    async fn test_update_last_polled() {
        let pool = test_db().await;

        let id = subscribe_to_playlist(&pool, "spotify", "pl-poll", None)
            .await
            .unwrap();

        // Before: should be due
        let before = get_due_subscriptions(&pool).await.unwrap();
        assert_eq!(before.len(), 1);

        // Poll
        update_subscription_last_polled(&pool, id).await.unwrap();

        // After: last_polled_at is set, should no longer be 'due'
        // But for in-memory unixepoch() to work, we need to be fast —
        // check via explicit query that last_polled_at is set
        let row: (Option<i64>,) =
            sqlx::query_as("SELECT last_polled_at FROM playlist_subscriptions WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(row.0.is_some(), "last_polled_at should be set");
    }

    #[tokio::test]
    async fn test_update_subscription_playlist_id() {
        let pool = test_db().await;

        let id = subscribe_to_playlist(&pool, "spotify", "pl-update", None)
            .await
            .unwrap();

        // Create a playlist to link to
        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-target', 'Target', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        update_subscription_playlist_id(&pool, id, pl_id)
            .await
            .unwrap();

        let (sp_id,): (Option<i64>,) =
            sqlx::query_as("SELECT service_playlist_id FROM playlist_subscriptions WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sp_id, Some(pl_id));
    }

    // ── Lookup Queries ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_is_playlist_subscribed() {
        let pool = test_db().await;

        // Not subscribed yet
        let result = is_playlist_subscribed(&pool, "spotify", "pl-none")
            .await
            .unwrap();
        assert!(result.is_none());

        // Subscribe
        subscribe_to_playlist(&pool, "spotify", "pl-check", None)
            .await
            .unwrap();

        let result = is_playlist_subscribed(&pool, "spotify", "pl-check")
            .await
            .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().playlist_id, "pl-check");
    }

    #[tokio::test]
    async fn test_get_subscription_by_playlist_id() {
        let pool = test_db().await;

        let id = subscribe_to_playlist(&pool, "soundcloud", "sc-get", None)
            .await
            .unwrap();

        let result = get_subscription_by_playlist_id(&pool, "soundcloud", "sc-get")
            .await
            .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, id);
    }

    // ── Playlist Operations ────────────────────────────────────────────

    #[tokio::test]
    async fn test_mark_playlist_inactive() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, snapshot_id, imported_at, updated_at)
             VALUES ('spotify', 'pl-active', 'Active Playlist', 'snap-001', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        mark_playlist_inactive(&pool, pl_id).await.unwrap();

        let (snap,): (Option<String>,) =
            sqlx::query_as("SELECT snapshot_id FROM service_playlists WHERE id = ?")
                .bind(pl_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(snap.is_none(), "snapshot_id should be cleared");
    }

    #[tokio::test]
    async fn test_set_playlist_archive_deleted() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-arch', 'Archivable', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Toggle on
        set_playlist_archive_deleted(&pool, pl_id, true)
            .await
            .unwrap();
        let (arch,): (bool,) =
            sqlx::query_as("SELECT archive_deleted FROM service_playlists WHERE id = ?")
                .bind(pl_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(arch, "archive_deleted should be true");

        // Toggle off
        set_playlist_archive_deleted(&pool, pl_id, false)
            .await
            .unwrap();
        let (arch,): (bool,) =
            sqlx::query_as("SELECT archive_deleted FROM service_playlists WHERE id = ?")
                .bind(pl_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!arch, "archive_deleted should be false");
    }

    #[tokio::test]
    async fn test_get_playlist_staleness() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name,
                remote_unique_count, remote_track_count, imported_at, updated_at)
             VALUES ('spotify', 'pl-stale', 'Stale Check', 10, 15, 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Add a track to the playlist
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 'track-1', 'Test Track', 'Test Artist', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?, 1, 0, 0)",
        )
        .bind(pl_id)
        .execute(&pool)
        .await
        .unwrap();

        let (local_count, unique_count, remote_count, last_fetched) =
            get_playlist_staleness(&pool, pl_id).await.unwrap();

        assert_eq!(local_count, 1, "should have 1 local track");
        assert_eq!(unique_count, 10, "should have 10 unique remote");
        assert_eq!(remote_count, 15, "should have 15 total remote");
        assert!(last_fetched.is_none(), "never fetched");
    }

    #[tokio::test]
    async fn test_delete_playlist() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-del', 'To Delete', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Add a track to it
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 'track-del', 'Delete Me', 'Artist', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?, 1, 0, 0)",
        )
        .bind(pl_id)
        .execute(&pool)
        .await
        .unwrap();

        let deleted = delete_playlist(&pool, pl_id).await.unwrap();
        assert!(deleted, "should return true when deleted");

        // Tracks should cascade-delete from the join table
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM service_playlist_tracks WHERE playlist_id = ?",
        )
        .bind(pl_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "cascade should remove track associations");

        // Playlist itself should be gone
        let found = get_service_playlist_by_id(&pool, "spotify", "pl-del")
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_get_service_playlist_by_id() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('youtube', 'yt-1', 'YouTube Mix', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let found = get_service_playlist_by_id(&pool, "youtube", "yt-1")
            .await
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, pl_id);

        let not_found = get_service_playlist_by_id(&pool, "youtube", "yt-none")
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_get_track_playlist_associations() {
        let pool = test_db().await;

        // Create a playlist and a track
        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-assoc', 'Association Test', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (100, 'spotify', 'track-assoc', 'Linked Track', 'Artist', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?, 100, 1, 0)",
        )
        .bind(pl_id)
        .execute(&pool)
        .await
        .unwrap();

        let assocs = get_track_playlist_associations(&pool, 100).await.unwrap();
        assert_eq!(assocs.len(), 1);
        assert_eq!(assocs[0].0, "Association Test");
        assert_eq!(assocs[0].1, "pl-assoc");
        assert_eq!(assocs[0].2, "spotify");
    }

    #[tokio::test]
    async fn test_get_playlist_tracks_by_name() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-name-test', 'Groovy', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at, updated_at)
             VALUES (1, 'spotify', 'st-1', 'Track 1', 'Artist 1', 'ISRC-1', 0, 0),
                    (2, 'spotify', 'st-2', 'Track 2', 'Artist 2', 'ISRC-2', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?, 1, 0, 0), (?, 2, 1, 0)",
        )
        .bind(pl_id)
        .bind(pl_id)
        .execute(&pool)
        .await
        .unwrap();

        let tracks = get_playlist_tracks_by_name(&pool, "Groovy").await.unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "Track 1");
        assert_eq!(tracks[1].title, "Track 2");
    }

    #[tokio::test]
    async fn test_get_playlist_tracks_empty() {
        let pool = test_db().await;

        let pl_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-empty', 'Empty Playlist', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let tracks = get_playlist_tracks(&pool, pl_id).await.unwrap();
        assert!(tracks.is_empty());
    }

    // ── Playlist CRUD (missing coverage) ────────────────────────────────

    #[tokio::test]
    async fn test_upsert_service_playlist_create() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl = upsert_service_playlist(
            &mut conn,
            "spotify",
            "pl-new",
            "New Playlist",
            Some("A description"),
            None,
        )
        .await
        .unwrap();

        assert_eq!(pl.service, "spotify");
        assert_eq!(pl.playlist_id, "pl-new");
        assert_eq!(pl.name, "New Playlist");
        assert_eq!(pl.description.as_deref(), Some("A description"));
        assert!(pl.id > 0);
    }

    #[tokio::test]
    async fn test_add_track_to_playlist_with_position() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl = upsert_service_playlist(
            &mut conn,
            "spotify",
            "pl-with-tracks",
            "With Tracks",
            None,
            None,
        )
        .await
        .unwrap();
        let tr_id: i64 = sqlx::query_scalar(
            "INSERT INTO service_tracks (service, service_id, title, artist, imported_at, updated_at)
             VALUES ('spotify', 'st-1', 'Track One', 'Artist', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        add_track_to_playlist(&mut conn, pl.id, tr_id, Some(5))
            .await
            .unwrap();

        let tracks = get_playlist_tracks(&pool, pl.id).await.unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, tr_id);
    }

    #[tokio::test]
    async fn test_add_track_to_playlist_with_added_at() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl =
            upsert_service_playlist(&mut conn, "spotify", "pl-added-at", "Added At", None, None)
                .await
                .unwrap();
        let tr_id: i64 = sqlx::query_scalar(
            "INSERT INTO service_tracks (service, service_id, title, artist, imported_at, updated_at)
             VALUES ('spotify', 'st-added', 'Added Track', 'Artist', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        add_track_to_playlist_with_added_at(&mut conn, pl.id, tr_id, None, Some(1000))
            .await
            .unwrap();

        // Verify via the query that added_at is 1000
        let (added_at,): (i64,) = sqlx::query_as(
            "SELECT added_at FROM service_playlist_tracks WHERE playlist_id = ? AND track_id = ?",
        )
        .bind(pl.id)
        .bind(tr_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(added_at, 1000);
    }

    #[tokio::test]
    async fn test_mark_playlist_tracks_deleted() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl = upsert_service_playlist(
            &mut conn,
            "spotify",
            "pl-del-tracks",
            "Soft Delete",
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 'st-1', 'T1', 'A', 0, 0),
                    (2, 'spotify', 'st-2', 'T2', 'A', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Default added_at = now
        add_track_to_playlist_with_added_at(&mut conn, pl.id, 1, None, None)
            .await
            .unwrap();
        add_track_to_playlist_with_added_at(&mut conn, pl.id, 2, None, None)
            .await
            .unwrap();

        let count = mark_playlist_tracks_deleted(&mut conn, pl.id)
            .await
            .unwrap();
        assert_eq!(count, 2, "should soft-delete 2 tracks");

        // Verify deleted_at is set
        let deleted_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM service_playlist_tracks WHERE playlist_id = ? AND deleted_at IS NOT NULL",
        )
        .bind(pl.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(deleted_count, 2);

        // Second call should mark 0 (already deleted)
        let count2 = mark_playlist_tracks_deleted(&mut conn, pl.id)
            .await
            .unwrap();
        assert_eq!(count2, 0, "no active tracks left to delete");
    }

    #[tokio::test]
    async fn test_add_track_to_playlist_reactivates_deleted() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl = upsert_service_playlist(&mut conn, "spotify", "pl-react", "Reactive", None, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 'st-react', 'React Track', 'Artist', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Add once
        add_track_to_playlist(&mut conn, pl.id, 1, Some(0))
            .await
            .unwrap();
        // Soft-delete
        mark_playlist_tracks_deleted(&mut conn, pl.id)
            .await
            .unwrap();
        // Re-add — should set deleted_at = NULL
        add_track_to_playlist(&mut conn, pl.id, 1, Some(0))
            .await
            .unwrap();

        let deleted: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM service_playlist_tracks WHERE playlist_id = ? AND deleted_at IS NULL",
        )
        .bind(pl.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            deleted, 1,
            "re-added track should be active (deleted_at = NULL)"
        );
    }

    #[tokio::test]
    async fn test_update_playlist_fetch_tracking() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        upsert_service_playlist(&mut conn, "spotify", "pl-track", "Tracked", None, None)
            .await
            .unwrap();

        update_playlist_fetch_tracking(&mut conn, "spotify", "pl-track", 25)
            .await
            .unwrap();

        let (fetched, remote_count, unique_count): (Option<i64>, i64, i64) = sqlx::query_as(
            "SELECT last_fetched_at, remote_track_count, remote_unique_count
             FROM service_playlists WHERE service = 'spotify' AND playlist_id = 'pl-track'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert!(fetched.is_some(), "last_fetched_at should be set");
        assert_eq!(remote_count, 25);
        assert_eq!(
            unique_count, 0,
            "no playlist_tracks inserted, so unique_count = 0"
        );
    }

    #[tokio::test]
    async fn test_update_playlist_remote_count() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        upsert_service_playlist(
            &mut conn,
            "spotify",
            "pl-remote-cnt",
            "Remote Count",
            None,
            None,
        )
        .await
        .unwrap();

        update_playlist_remote_count(&mut conn, "spotify", "pl-remote-cnt", 42)
            .await
            .unwrap();

        let (remote_count,): (i64,) = sqlx::query_as(
            "SELECT remote_track_count FROM service_playlists WHERE service = 'spotify' AND playlist_id = 'pl-remote-cnt'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remote_count, 42);
    }

    #[tokio::test]
    async fn test_delete_playlist_not_found() {
        let pool = test_db().await;
        let deleted = delete_playlist(&pool, 9999).await.unwrap();
        assert!(!deleted, "non-existent playlist should return false");
    }

    // ── Snapshot / Staleness (missing coverage) ──────────────────────────

    #[tokio::test]
    async fn test_get_spotify_playlist_snapshots_empty() {
        let pool = test_db().await;
        let snapshots = get_spotify_playlist_snapshots(&pool).await.unwrap();
        assert!(snapshots.is_empty());
    }

    #[tokio::test]
    async fn test_get_spotify_playlist_snapshots_with_data() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO service_playlists (service, playlist_id, name, snapshot_id, imported_at, updated_at)
             VALUES ('spotify', 'pl-snap-1', 'Snap One', 'snap-a', 0, 0),
                    ('spotify', 'pl-snap-2', 'Snap Two', 'snap-b', 0, 0),
                    ('soundcloud', 'sc-1', 'SC', NULL, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let snapshots = get_spotify_playlist_snapshots(&pool).await.unwrap();
        assert_eq!(snapshots.len(), 2, "only spotify playlists should appear");
        assert_eq!(snapshots[0].1, "pl-snap-1");
        assert_eq!(snapshots[0].2.as_deref(), Some("snap-a"));
        assert_eq!(snapshots[1].1, "pl-snap-2");
    }

    #[tokio::test]
    async fn test_update_playlist_snapshot() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-up-snap', 'Update Snap', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        update_playlist_snapshot(&pool, "pl-up-snap", "new-snapshot-123")
            .await
            .unwrap();

        let (snap,): (Option<String>,) = sqlx::query_as(
            "SELECT snapshot_id FROM service_playlists WHERE service = 'spotify' AND playlist_id = 'pl-up-snap'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(snap.as_deref(), Some("new-snapshot-123"));
    }

    // ── Subscription Info (missing coverage) ────────────────────────────

    #[tokio::test]
    async fn test_get_subscription_playlist_info() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl =
            upsert_service_playlist(&mut conn, "spotify", "pl-sub-info", "Sub Info", None, None)
                .await
                .unwrap();

        // Set some remote counts
        sqlx::query(
            "UPDATE service_playlists SET remote_unique_count = 8, remote_track_count = 10 WHERE id = ?",
        )
        .bind(pl.id)
        .execute(&pool)
        .await
        .unwrap();

        let (snap, unique, total, fetched) =
            get_subscription_playlist_info(&pool, pl.id).await.unwrap();
        assert!(snap.is_none());
        assert_eq!(unique, 8);
        assert_eq!(total, 10);
        assert!(fetched.is_none());
    }

    // ── Empty / Edge Cases ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_playlist_tracks_ordered_by_position() {
        let pool = test_db().await;
        let mut conn = pool.acquire().await.unwrap();

        let pl = upsert_service_playlist(&mut conn, "spotify", "pl-order", "Ordered", None, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 's-a', 'Z Track', 'A', 0, 0),
                    (2, 'spotify', 's-b', 'A Track', 'A', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        add_track_to_playlist(&mut conn, pl.id, 1, Some(10))
            .await
            .unwrap();
        add_track_to_playlist(&mut conn, pl.id, 2, Some(5))
            .await
            .unwrap();

        let tracks = get_playlist_tracks(&pool, pl.id).await.unwrap();
        assert_eq!(tracks.len(), 2);
        // Should be ordered by position ascending
        assert_eq!(tracks[0].id, 2, "position 5 first");
        assert_eq!(tracks[1].id, 1, "position 10 second");
    }

    #[tokio::test]
    async fn test_get_track_playlist_associations_empty() {
        let pool = test_db().await;
        let assocs = get_track_playlist_associations(&pool, 9999).await.unwrap();
        assert!(assocs.is_empty());
    }
}
