//! Database layer for BPM//key system playlists.
//!
//! Derives the desired `(BPM, key)` buckets from the library ([`derive_groups`])
//! and provides CRUD over the `service_playlists` rows that back them
//! (`system_key IS NOT NULL`, `playlist_kind = 'generated'`).

use anyhow::Result;
use sqlx::{Pool, Row, Sqlite};

use crate::bpm_key::{BpmKeySettings, FileTrackRow, Group, group_rows};
use crate::db::settings::{
    KEY_BPMKEY_ENABLED, KEY_BPMKEY_KEY_STYLE, KEY_BPMKEY_LAST_SYNC_AT, KEY_BPMKEY_MIN_TRACKS,
    KEY_BPMKEY_NAME_PREFIX, KEY_BPMKEY_NAME_TEMPLATE, KEY_BPMKEY_PUBLIC,
    KEY_BPMKEY_SCHEDULE_ENABLED, KEY_BPMKEY_SCHEDULE_INTERVAL_SECS, KEY_BPMKEY_STRICT, get_bool,
    get_setting, set_bool, set_setting,
};

/// Derive the desired buckets from every Spotify-linked file that has a usable
/// BPM and musical key. Grouping/dedup happens in [`crate::bpm_key::group_rows`].
pub async fn derive_groups(pool: &Pool<Sqlite>) -> Result<Vec<Group>> {
    let rows = sqlx::query(
        r#"
        SELECT f.id AS file_id,
               f.bpm AS bpm,
               f.musical_key AS musical_key,
               f.stem_type AS stem_type,
               st.id AS track_id,
               st.service_id AS service_id
        FROM files f
        JOIN v_file_track_link v ON v.file_id = f.id
        JOIN service_tracks st ON st.id = v.track_id AND st.service = 'spotify'
        WHERE f.bpm IS NOT NULL
          AND f.musical_key IS NOT NULL
          AND TRIM(f.musical_key) <> ''
        "#,
    )
    .fetch_all(pool)
    .await?;

    let link_rows: Vec<FileTrackRow> = rows
        .into_iter()
        .map(|r| FileTrackRow {
            file_id: r.get::<i64, _>("file_id"),
            bpm: r.get::<f64, _>("bpm"),
            musical_key: r.get::<String, _>("musical_key"),
            stem_type: r.get::<Option<String>, _>("stem_type"),
            track_id: r.get::<i64, _>("track_id"),
            service_id: r.get::<String, _>("service_id"),
        })
        .collect();

    Ok(group_rows(&link_rows))
}

/// A persisted system playlist row.
#[derive(Debug, Clone)]
pub struct SystemPlaylist {
    pub id: i64,
    pub name: String,
    pub system_key: String,
    pub playlist_id: String,
}

/// All system playlists (`system_key IS NOT NULL`), ordered by name.
pub async fn list_system_playlists(pool: &Pool<Sqlite>) -> Result<Vec<SystemPlaylist>> {
    let rows = sqlx::query(
        "SELECT id, name, system_key, playlist_id FROM service_playlists \
         WHERE service = 'spotify' AND system_key IS NOT NULL ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SystemPlaylist {
            id: r.get::<i64, _>("id"),
            name: r.get::<String, _>("name"),
            system_key: r.get::<String, _>("system_key"),
            playlist_id: r.get::<String, _>("playlist_id"),
        })
        .collect())
}

/// Find a system playlist by its stable `system_key`. Returns `(id, playlist_id)`.
pub async fn find_system_playlist(
    pool: &Pool<Sqlite>,
    system_key: &str,
) -> Result<Option<(i64, String)>> {
    let row = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, playlist_id FROM service_playlists \
         WHERE service = 'spotify' AND system_key = ?",
    )
    .bind(system_key)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Insert a new system playlist row (`playlist_kind = 'generated'`).
pub async fn insert_system_playlist(
    pool: &Pool<Sqlite>,
    name: &str,
    system_key: &str,
    playlist_id: &str,
) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO service_playlists \
             (service, playlist_id, name, playlist_kind, system_key, imported_at, updated_at) \
         VALUES ('spotify', ?, ?, 'generated', ?, unixepoch(), unixepoch()) \
         RETURNING id",
    )
    .bind(playlist_id)
    .bind(name)
    .bind(system_key)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Update the stored display name of a system playlist (template changes).
pub async fn update_system_playlist_name(pool: &Pool<Sqlite>, id: i64, name: &str) -> Result<()> {
    sqlx::query("UPDATE service_playlists SET name = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Record the mirrored remote track count for a system playlist.
pub async fn update_system_playlist_counts(
    pool: &Pool<Sqlite>,
    id: i64,
    track_count: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE service_playlists \
         SET remote_track_count = ?, remote_unique_count = ?, last_fetched_at = unixepoch(), \
             updated_at = unixepoch() \
         WHERE id = ?",
    )
    .bind(track_count)
    .bind(track_count)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a system playlist row (strict mode only).
pub async fn delete_system_playlist(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM service_playlists WHERE id = ? AND system_key IS NOT NULL")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Settings ────────────────────────────────────────────────────────────────

/// Load the feature settings, falling back to defaults for absent keys.
pub async fn load_bpm_key_settings(pool: &Pool<Sqlite>) -> Result<BpmKeySettings> {
    let mut s = BpmKeySettings::default();

    if let Some(v) = get_bool(pool, KEY_BPMKEY_ENABLED).await? {
        s.enabled = v;
    }
    if let Some(v) = get_setting(pool, KEY_BPMKEY_NAME_TEMPLATE).await? {
        if !v.trim().is_empty() {
            s.name_template = v;
        }
    }
    if let Some(v) = get_setting(pool, KEY_BPMKEY_NAME_PREFIX).await? {
        s.name_prefix = v;
    }
    if let Some(v) = get_setting(pool, KEY_BPMKEY_MIN_TRACKS).await? {
        if let Ok(n) = v.parse::<i64>() {
            s.min_tracks = n;
        }
    }
    if let Some(v) = get_bool(pool, KEY_BPMKEY_PUBLIC).await? {
        s.public = v;
    }
    if let Some(v) = get_setting(pool, KEY_BPMKEY_KEY_STYLE).await? {
        s.key_style = v;
    }
    if let Some(v) = get_bool(pool, KEY_BPMKEY_STRICT).await? {
        s.strict = v;
    }
    if let Some(v) = get_bool(pool, KEY_BPMKEY_SCHEDULE_ENABLED).await? {
        s.schedule_enabled = v;
    }
    if let Some(v) = get_setting(pool, KEY_BPMKEY_SCHEDULE_INTERVAL_SECS).await? {
        if let Ok(n) = v.parse::<i64>() {
            s.schedule_interval_secs = n;
        }
    }

    s.sanitize();
    Ok(s)
}

/// Persist the full settings set.
pub async fn save_bpm_key_settings(pool: &Pool<Sqlite>, s: &BpmKeySettings) -> Result<()> {
    set_bool(pool, KEY_BPMKEY_ENABLED, s.enabled).await?;
    set_setting(pool, KEY_BPMKEY_NAME_TEMPLATE, &s.name_template).await?;
    set_setting(pool, KEY_BPMKEY_NAME_PREFIX, &s.name_prefix).await?;
    set_setting(pool, KEY_BPMKEY_MIN_TRACKS, &s.min_tracks.to_string()).await?;
    set_bool(pool, KEY_BPMKEY_PUBLIC, s.public).await?;
    set_setting(pool, KEY_BPMKEY_KEY_STYLE, &s.key_style).await?;
    set_bool(pool, KEY_BPMKEY_STRICT, s.strict).await?;
    set_bool(pool, KEY_BPMKEY_SCHEDULE_ENABLED, s.schedule_enabled).await?;
    set_setting(
        pool,
        KEY_BPMKEY_SCHEDULE_INTERVAL_SECS,
        &s.schedule_interval_secs.to_string(),
    )
    .await?;
    Ok(())
}

/// Whether the feature is enabled (auto-trigger on).
pub async fn bpm_key_sync_enabled(pool: &Pool<Sqlite>) -> Result<bool> {
    Ok(get_bool(pool, KEY_BPMKEY_ENABLED).await?.unwrap_or(false))
}

/// Unix seconds of the last scheduled/manual sync enqueue (`0` when never).
pub async fn last_sync_enqueued_at(pool: &Pool<Sqlite>) -> Result<i64> {
    Ok(get_setting(pool, KEY_BPMKEY_LAST_SYNC_AT)
        .await?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0))
}

/// Record the unix seconds of the last sync enqueue.
pub async fn set_last_sync_enqueued_at(pool: &Pool<Sqlite>, ts: i64) -> Result<()> {
    set_setting(pool, KEY_BPMKEY_LAST_SYNC_AT, &ts.to_string()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Minimal schema: only what derivation/CRUD touch.
    async fn test_pool() -> Pool<Sqlite> {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        for stmt in [
            r#"CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                bpm REAL,
                musical_key TEXT,
                stem_type TEXT,
                isrc TEXT,
                spotify_id TEXT,
                soundcloud_id TEXT,
                youtube_id TEXT
            )"#,
            r#"CREATE TABLE service_tracks (
                id INTEGER PRIMARY KEY,
                service TEXT NOT NULL,
                service_id TEXT NOT NULL,
                isrc TEXT,
                UNIQUE(service, service_id)
            )"#,
            r#"CREATE VIEW v_file_track_link AS
               SELECT f.id AS file_id, st.id AS track_id
               FROM files f
               JOIN service_tracks st ON (
                   st.isrc = f.isrc
                   OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
               )"#,
            r#"CREATE TABLE service_playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                playlist_kind TEXT NOT NULL DEFAULT 'curated',
                system_key TEXT,
                remote_track_count INTEGER NOT NULL DEFAULT 0,
                remote_unique_count INTEGER NOT NULL DEFAULT 0,
                last_fetched_at INTEGER,
                imported_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                UNIQUE(service, playlist_id)
            )"#,
            r#"CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT 0
            )"#,
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        pool
    }

    #[tokio::test]
    async fn derive_groups_excludes_missing_key_and_no_link() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id) VALUES (1, 'spotify', 'aaa')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO files (id, bpm, musical_key, spotify_id) VALUES \
             (1, 124.0, '12m', 'aaa'), \
             (2, NULL, '12m', NULL), \
             (3, 124.0, '', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let groups = derive_groups(&pool).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].bpm, 124);
        assert_eq!(groups[0].canonical_key, "12A");
        assert_eq!(groups[0].uris, vec!["spotify:track:aaa"]);
    }

    #[tokio::test]
    async fn system_playlist_crud_roundtrip() {
        let pool = test_pool().await;
        let id = insert_system_playlist(&pool, "124bpm // 12m", "bpm_key:124:12A", "abc")
            .await
            .unwrap();
        let found = find_system_playlist(&pool, "bpm_key:124:12A")
            .await
            .unwrap();
        assert_eq!(found, Some((id, "abc".to_string())));

        let all = list_system_playlists(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "124bpm // 12m");

        update_system_playlist_name(&pool, id, "124bpm // 12A")
            .await
            .unwrap();
        assert_eq!(
            find_system_playlist(&pool, "bpm_key:124:12A")
                .await
                .unwrap()
                .unwrap()
                .1,
            "abc"
        );
        let all = list_system_playlists(&pool).await.unwrap();
        assert_eq!(all[0].name, "124bpm // 12A");

        delete_system_playlist(&pool, id).await.unwrap();
        assert!(list_system_playlists(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn settings_load_defaults_and_roundtrip() {
        let pool = test_pool().await;
        let s = load_bpm_key_settings(&pool).await.unwrap();
        assert_eq!(s, BpmKeySettings::default());

        let mut s = s;
        s.enabled = true;
        s.min_tracks = 3;
        s.key_style = "camelot".to_string();
        save_bpm_key_settings(&pool, &s).await.unwrap();

        let loaded = load_bpm_key_settings(&pool).await.unwrap();
        assert_eq!(loaded, s);
        assert!(bpm_key_sync_enabled(&pool).await.unwrap());
    }
}
