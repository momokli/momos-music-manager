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
/// Call this after any tag/playlist/track sync. Also refresh track_resolved_tags when appropriate.
/// Returns the number of rows inserted.
pub async fn refresh_file_resolved_tags(pool: &Pool<Sqlite>) -> Result<u64> {
    // Truncate the table
    sqlx::query("DELETE FROM file_resolved_tags")
        .execute(pool)
        .await?;

    // Repopulate from the view
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
    .fetch_one(pool)
    .await?;

    let count = changed as u64;
    tracing::info!("Refreshed file_resolved_tags: {} rows", count);
    Ok(count)
}

/// Truncate and repopulate `track_resolved_tags` from the `v_track_tags` view.
/// Call this after any tag/playlist/track sync.
/// Returns the number of rows inserted.
pub async fn refresh_track_resolved_tags(pool: &Pool<Sqlite>) -> Result<u64> {
    // Truncate the table
    sqlx::query("DELETE FROM track_resolved_tags")
        .execute(pool)
        .await?;

    // Repopulate from the view
    let changed: i64 = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT OR IGNORE INTO track_resolved_tags (track_id, tag_id, tag_name, category_id, category_name, prefix, is_default)
        SELECT DISTINCT
            vtt.track_id, vtt.tag_id, vtt.tag_name, vtt.category_id, vtt.category_name, vtt.prefix, vtt.is_default
        FROM v_track_tags vtt;
        SELECT CHANGES();
        "#,
    )
    .fetch_one(pool)
    .await?;

    let count = changed as u64;
    tracing::info!("Refreshed track_resolved_tags: {} rows", count);
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
