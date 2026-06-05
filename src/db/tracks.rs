#![allow(dead_code)]

//! Track-related database queries — track detail, key comparison, playlist snapshots.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Sqlite};

use super::types::*;

// ── Track Detail ────────────────────────────────────────────────────────

/// Rich detail view for a single track: service track metadata + linked files
/// with audio features + tags + playlists + backpack status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetail {
    pub id: i64,
    pub service: String,
    pub service_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub popularity: Option<i32>,
    // Linked files + tags + playlists
    pub files: Vec<TrackDetailFile>,
    pub tags: Vec<FileDetailTag>,
    pub playlists: Vec<FileDetailPlaylist>,
    pub in_backpack: bool,
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailFile {
    pub id: i64,
    pub file_path: String,
    pub file_type: String,
    pub stem_type: Option<String>, // For WAV source files
    pub file_size: i64,
    pub isrc: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub comment: Option<String>,
    pub rating: Option<i32>,
    pub play_count: Option<i32>,
    pub last_played: Option<i64>,
    pub backed_up: bool,
    pub backup_path: Option<String>,
    pub is_local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileDetailTag {
    pub id: i64,
    pub name: String,
    pub category_name: String,
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileDetailPlaylist {
    pub id: i64,
    pub name: String,
    pub service: String,
}

/// Fetch rich detail for a single service track: metadata + linked files
/// (from v_file_track_link) with WAV source variants + tags + playlists.
pub async fn get_track_detail(pool: &Pool<Sqlite>, track_id: i64) -> Result<Option<TrackDetail>> {
    // 1. Fetch the service track
    #[derive(Debug, FromRow)]
    struct TrackRow {
        id: i64,
        service: String,
        service_id: String,
        title: String,
        artist: String,
        album: Option<String>,
        isrc: Option<String>,
        duration_ms: Option<i64>,
        metadata_json: Option<String>,
        imported_at: i64,
        updated_at: i64,
    }

    let track = sqlx::query_as::<_, TrackRow>(
        r#"SELECT id, service, service_id, title, artist, album,
                  isrc, duration_ms, metadata_json, imported_at, updated_at
           FROM service_tracks WHERE id = ?"#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;

    let Some(track) = track else {
        return Ok(None);
    };

    let popularity = track
        .metadata_json
        .as_ref()
        .and_then(|json_str| {
            serde_json::from_str::<serde_json::Value>(json_str)
                .ok()
                .and_then(|v| v.get("popularity").and_then(|p| p.as_i64()))
        })
        .map(|p| p as i32);

    // 2. Fetch ALL linked files via v_file_track_link
    let mut files: Vec<TrackDetailFile> = sqlx::query_as(
        r#"
        SELECT f.id, f.file_path, f.file_type, f.stem_type, f.file_size, f.isrc,
               f.title, f.artist, f.album, f.bpm, f.musical_key,
               f.duration_ms, f.bitrate, f.sample_rate, f.channels,
               f.comment, f.rating, f.play_count, f.last_played,
               COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
               fl_backup.path as backup_path,
               COALESCE(fl_local.id IS NOT NULL, 0) as is_local
        FROM v_file_track_link v
        JOIN files f ON f.id = v.file_id
        LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
        LEFT JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
        WHERE v.track_id = ?
        ORDER BY f.file_path
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    // Step 2b: For each linked stem file, fetch WAV source files
    let stem_ids: Vec<i64> = files.iter().map(|f| f.id).collect();
    if !stem_ids.is_empty() {
        let placeholders: Vec<String> = stem_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT f.id, f.file_path, f.file_type, f.stem_type, f.file_size, f.isrc,
                    f.title, f.artist, f.album, f.bpm, f.musical_key,
                    f.duration_ms, f.bitrate, f.sample_rate, f.channels,
                    f.comment, f.rating, f.play_count, f.last_played,
                    COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
                    fl_backup.path as backup_path,
                    COALESCE(fl_local.id IS NOT NULL, 0) as is_local
             FROM files f
             LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
             LEFT JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
             WHERE f.source_of IN ({}) AND f.file_type = 'wav'
             ORDER BY f.stem_type",
            placeholders.join(",")
        );
        let mut query = sqlx::query_as::<_, TrackDetailFile>(&sql);
        for id in &stem_ids {
            query = query.bind(id);
        }
        let wav_files = query.fetch_all(pool).await.unwrap_or_default();
        files.extend(wav_files);
    }

    // 3. Fetch tags for this track (via playlist→tag→v_resolved_tags chain)
    let tags: Vec<FileDetailTag> = sqlx::query_as(
        r#"
        SELECT DISTINCT trt.tag_id as id, trt.tag_name as name,
               trt.category_name, trt.prefix
        FROM track_resolved_tags trt
        WHERE trt.track_id = ?
        ORDER BY trt.category_name, trt.tag_name
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // 4. Fetch linked playlists
    let playlists: Vec<FileDetailPlaylist> = sqlx::query_as(
        r#"
        SELECT DISTINCT sp.id, sp.name, sp.service
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        WHERE spt.track_id = ?
        ORDER BY sp.name
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // 5. Compute in_backpack: track has any tag with backpack=true, OR is in "backpack" playlist
    let in_backpack: bool = {
        let tag_backpack: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM service_playlist_tracks spt
             JOIN service_playlists sp ON sp.id = spt.playlist_id
             JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
             WHERE spt.track_id = ? AND t.backpack = 1"#,
        )
        .bind(track_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        let playlist_backpack: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM service_playlist_tracks spt
             JOIN service_playlists sp ON sp.id = spt.playlist_id
             WHERE spt.track_id = ? AND LOWER(TRIM(sp.name)) = 'backpack'"#,
        )
        .bind(track_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        tag_backpack > 0 || playlist_backpack > 0
    };

    Ok(Some(TrackDetail {
        id: track.id,
        service: track.service,
        service_id: track.service_id,
        title: track.title,
        artist: track.artist,
        album: track.album,
        isrc: track.isrc,
        duration_ms: track.duration_ms,
        popularity,
        files,
        tags,
        playlists,
        in_backpack,
    }))
}

// ── Key Comparison ──────────────────────────────────────────────────────

/// Comparison row: side-by-side Traktor vs Spotify BPM/Key for a linked file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyComparisonRow {
    pub file_id: i64,
    pub title: String,
    pub artist: Option<String>,
    /// Traktor BPM (from files.bpm tag)
    pub traktor_bpm: Option<f64>,
    /// Traktor key in Camelot notation (from files.musical_key tag, e.g. "3m")
    pub traktor_key: Option<String>,
    /// Spotify BPM (from audio features tempo)
    pub spotify_bpm: Option<f64>,
    /// Spotify key converted to Camelot notation
    pub spotify_key: Option<String>,
    /// Spotify raw key (0=C..11=B)
    pub spotify_key_raw: Option<i32>,
    /// Spotify mode (0=minor, 1=major)
    pub spotify_mode: Option<i32>,
    /// Do Traktor and Spotify agree on BPM? (±1 BPM tolerance)
    pub bpm_match: Option<bool>,
    /// Do Traktor and Spotify agree on Camelot key?
    pub key_match: Option<bool>,
    /// Audio features: danceability
    pub spotify_danceability: Option<f64>,
    /// Audio features: energy
    pub spotify_energy: Option<f64>,
    /// Audio features: valence
    pub spotify_valence: Option<f64>,
}

/// Comparison summary: aggregated statistics from a key comparison.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyComparisonSummary {
    pub total_compared: usize,
    pub bpm_match_count: usize,
    pub bpm_mismatch_count: usize,
    pub key_match_count: usize,
    pub key_mismatch_count: usize,
}

/// Compare Traktor vs Spotify BPM/Key for files linked to Spotify tracks.
///
/// Filters by tag name (resolved via file_resolved_tags) and returns
/// side-by-side comparison with match/mismatch stats.
pub async fn get_key_comparison(
    pool: &Pool<Sqlite>,
    tag: Option<&str>,
    limit: Option<i64>,
) -> Result<(Vec<KeyComparisonRow>, KeyComparisonSummary)> {
    let limit = limit.unwrap_or(500);

    // Query: join files → v_file_track_link → service_tracks, optionally filter by tag
    let rows = if let Some(tag_name) = tag {
        sqlx::query_as::<
            _,
            (
                i64,
                String,
                Option<String>,
                Option<f64>,
                Option<String>,
                Option<f64>,
                Option<String>,
                Option<i32>,
                Option<i32>,
                Option<f64>,
                Option<f64>,
                Option<f64>,
            ),
        >(
            r#"
            SELECT DISTINCT
                f.id, COALESCE(f.title, '') as title, f.artist,
                f.bpm, f.musical_key,
                st.spotify_tempo, st.spotify_key_camelot,
                st.spotify_key_raw, st.spotify_mode,
                st.spotify_danceability, st.spotify_energy, st.spotify_valence
            FROM files f
            JOIN file_resolved_tags frt ON frt.file_id = f.id AND frt.tag_name = ?
            JOIN v_file_track_link v ON v.file_id = f.id
            JOIN service_tracks st ON st.id = v.track_id AND st.service = 'spotify'
            ORDER BY f.title
            LIMIT ?
            "#,
        )
        .bind(tag_name)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<
            _,
            (
                i64,
                String,
                Option<String>,
                Option<f64>,
                Option<String>,
                Option<f64>,
                Option<String>,
                Option<i32>,
                Option<i32>,
                Option<f64>,
                Option<f64>,
                Option<f64>,
            ),
        >(
            r#"
            SELECT DISTINCT
                f.id, COALESCE(f.title, '') as title, f.artist,
                f.bpm, f.musical_key,
                st.spotify_tempo, st.spotify_key_camelot,
                st.spotify_key_raw, st.spotify_mode,
                st.spotify_danceability, st.spotify_energy, st.spotify_valence
            FROM files f
            JOIN v_file_track_link v ON v.file_id = f.id
            JOIN service_tracks st ON st.id = v.track_id AND st.service = 'spotify'
            ORDER BY f.title
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    let mut results = Vec::with_capacity(rows.len());
    let mut bpm_match_count = 0usize;
    let mut bpm_mismatch_count = 0usize;
    let mut key_match_count = 0usize;
    let mut key_mismatch_count = 0usize;

    for (
        file_id,
        title,
        artist,
        traktor_bpm,
        traktor_key,
        spotify_bpm,
        spotify_key,
        spotify_key_raw,
        spotify_mode,
        spotify_danceability,
        spotify_energy,
        spotify_valence,
    ) in rows
    {
        // Compare BPM: match if within ±1
        let bpm_match = match (traktor_bpm, spotify_bpm) {
            (Some(t), Some(s)) => {
                let m = (t - s).abs() <= 1.0;
                if m {
                    bpm_match_count += 1;
                } else {
                    bpm_mismatch_count += 1;
                }
                Some(m)
            }
            _ => None,
        };

        // Compare key: match if Camelot notation identical
        let key_match = match (&traktor_key, &spotify_key) {
            (Some(t), Some(s)) => {
                let m = t == s;
                if m {
                    key_match_count += 1;
                } else {
                    key_mismatch_count += 1;
                }
                Some(m)
            }
            _ => None,
        };

        results.push(KeyComparisonRow {
            file_id,
            title,
            artist,
            traktor_bpm,
            traktor_key,
            spotify_bpm,
            spotify_key,
            spotify_key_raw,
            spotify_mode,
            bpm_match,
            key_match,
            spotify_danceability,
            spotify_energy,
            spotify_valence,
        });
    }

    let summary = KeyComparisonSummary {
        total_compared: results.len(),
        bpm_match_count,
        bpm_mismatch_count,
        key_match_count,
        key_mismatch_count,
    };

    Ok((results, summary))
}

// ── Service Track Queries ──────────────────────────────────────────────

/// Get all service tracks, ordered by service then title.
pub async fn get_service_tracks(pool: &Pool<Sqlite>) -> Result<Vec<ServiceTrack>> {
    let tracks =
        sqlx::query_as::<_, ServiceTrack>("SELECT * FROM service_tracks ORDER BY service, title")
            .fetch_all(pool)
            .await?;
    Ok(tracks)
}

/// Get tags for a service track (via playlist name matching).
pub async fn get_tags_for_service_track(pool: &Pool<Sqlite>, track_id: i64) -> Result<Vec<Tag>> {
    // Find tags linked to this track via v_tag_playlist (playlist → tag name matching)
    let tags = sqlx::query_as::<_, Tag>(
        r#"
        SELECT DISTINCT t.id, t.name, t.category_id, t.sort_order, t.created_at, t.backpack
        FROM tags t
        JOIN v_tag_playlist vtp ON vtp.tag_id = t.id
        JOIN service_playlist_tracks spt ON spt.playlist_id = vtp.playlist_id
        WHERE spt.track_id = ?
        ORDER BY t.name
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Create an in-memory SQLite DB with the minimal schema for track tests.
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        // service_tracks: core table
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
                spotify_tempo REAL,
                spotify_key_camelot TEXT,
                spotify_key_raw INTEGER,
                spotify_mode INTEGER,
                spotify_danceability REAL,
                spotify_energy REAL,
                spotify_valence REAL,
                UNIQUE(service, service_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // tags: for tag resolution tests
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE,
                category_id INTEGER NOT NULL DEFAULT 1,
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (unixepoch()),
                backpack INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // service_playlists: for tag matching
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

        // service_playlist_tracks: track membership in playlists
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

        // v_tag_playlist view for tag→playlist matching
        sqlx::query(
            "CREATE VIEW IF NOT EXISTS v_tag_playlist AS
             SELECT DISTINCT t.id AS tag_id, t.name AS tag_name,
                    sp.id AS playlist_id, sp.name AS playlist_name,
                    sp.service
             FROM tags t
             JOIN service_playlists sp ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))",
        )
        .execute(&pool)
        .await
        .unwrap();

        // tag_categories: needed for TagCategory queries
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tag_categories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                icon TEXT NOT NULL DEFAULT 'fa-tag',
                prefix TEXT NOT NULL DEFAULT 'T',
                sort_order INTEGER NOT NULL DEFAULT 0,
                is_default INTEGER NOT NULL DEFAULT 0,
                tag_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER DEFAULT (unixepoch())
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // v_tag_categories view
        sqlx::query(
            "CREATE VIEW IF NOT EXISTS v_tag_categories AS
             SELECT * FROM tag_categories",
        )
        .execute(&pool)
        .await
        .unwrap();

        // files: for key_comparison tests
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                file_hash TEXT NOT NULL DEFAULT '',
                file_type TEXT NOT NULL,
                file_size INTEGER NOT NULL DEFAULT 0,
                last_modified INTEGER NOT NULL DEFAULT 0,
                last_scanned INTEGER NOT NULL DEFAULT 0,
                isrc TEXT,
                title TEXT,
                artist TEXT,
                album TEXT,
                album_artist TEXT,
                track_number INTEGER,
                total_tracks INTEGER,
                disc_number INTEGER,
                total_discs INTEGER,
                genre TEXT,
                year INTEGER,
                composer TEXT,
                comment TEXT,
                duration_ms INTEGER,
                bitrate INTEGER,
                sample_rate INTEGER,
                channels INTEGER,
                bpm REAL,
                musical_key TEXT,
                rating INTEGER NOT NULL DEFAULT 0,
                play_count INTEGER NOT NULL DEFAULT 0,
                last_played INTEGER,
                spotify_id TEXT,
                soundcloud_id TEXT,
                youtube_id TEXT,
                source_of INTEGER,
                stem_type TEXT,
                last_verified_local INTEGER,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // file_locations: for linking files to backup/local
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS file_locations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                location_type TEXT NOT NULL CHECK (location_type IN ('local', 'backup')),
                path TEXT NOT NULL,
                file_size INTEGER,
                last_verified INTEGER,
                created_at INTEGER DEFAULT (unixepoch()),
                UNIQUE(file_id, location_type)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // file_resolved_tags: materialized tag resolution
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS file_resolved_tags (
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL,
                tag_name TEXT NOT NULL,
                category_id INTEGER NOT NULL,
                category_name TEXT NOT NULL,
                prefix TEXT NOT NULL,
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER,
                is_default INTEGER DEFAULT 0,
                PRIMARY KEY (file_id, tag_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // v_file_track_link view
        sqlx::query(
            "CREATE VIEW IF NOT EXISTS v_file_track_link AS
             SELECT f.id AS file_id, st.id AS track_id
             FROM files f
             JOIN service_tracks st ON (
                 st.isrc = f.isrc
                 OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
                 OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
                 OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
                 OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    // ── get_service_tracks ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_service_tracks_empty() {
        let pool = test_db().await;
        let tracks = get_service_tracks(&pool).await.unwrap();
        assert!(tracks.is_empty());
    }

    #[tokio::test]
    async fn test_get_service_tracks_returns_all() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO service_tracks (service, service_id, title, artist, isrc, imported_at, updated_at)
             VALUES ('spotify', 's-1', 'Track One', 'Artist 1', 'ISRC-1', 100, 100),
                    ('spotify', 's-2', 'Track Two', 'Artist 2', 'ISRC-2', 200, 200),
                    ('soundcloud', 'sc-1', 'SC Track', 'SC Artist', NULL, 300, 300)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let tracks = get_service_tracks(&pool).await.unwrap();
        assert_eq!(tracks.len(), 3);

        // Verify ordering: service first, then title
        assert_eq!(tracks[0].service, "soundcloud");
        assert_eq!(tracks[1].service, "spotify");
        assert_eq!(tracks[2].service, "spotify");
        assert_eq!(tracks[1].title, "Track One");
        assert_eq!(tracks[2].title, "Track Two");
    }

    // ── get_tags_for_service_track ─────────────────────────────────────

    #[tokio::test]
    async fn test_get_tags_for_service_track_returns_matching_tags() {
        let pool = test_db().await;

        // Create a tag with a name matching the playlist
        sqlx::query(
            "INSERT INTO tags (id, name, category_id) VALUES (1, 'Groovy', 1), (2, 'Dark', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Create a playlist with the same name as the tag
        let pl_id: i64 = sqlx::query_scalar(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-1', 'Groovy', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Create a track and link it to the playlist
        let tr_id: i64 = sqlx::query_scalar(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 'st-1', 'Test Track', 'Test Artist', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?, ?, 0, 0)",
        )
        .bind(pl_id)
        .bind(tr_id)
        .execute(&pool)
        .await
        .unwrap();

        let tags = get_tags_for_service_track(&pool, tr_id).await.unwrap();
        assert_eq!(tags.len(), 1, "should find 1 matching tag");
        assert_eq!(tags[0].name, "Groovy");
    }

    #[tokio::test]
    async fn test_get_tags_for_service_track_no_match() {
        let pool = test_db().await;

        // Create a tag and a playlist with different names
        sqlx::query("INSERT INTO tags (id, name, category_id) VALUES (1, 'Groovy', 1)")
            .execute(&pool)
            .await
            .unwrap();

        let pl_id: i64 = sqlx::query_scalar(
            "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at)
             VALUES ('spotify', 'pl-other', 'Other Name', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let tr_id: i64 = sqlx::query_scalar(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, imported_at, updated_at)
             VALUES (1, 'spotify', 'st-1', 'Test', 'Artist', 0, 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?, ?, 0, 0)",
        )
        .bind(pl_id)
        .bind(tr_id)
        .execute(&pool)
        .await
        .unwrap();

        let tags = get_tags_for_service_track(&pool, tr_id).await.unwrap();
        assert!(tags.is_empty(), "no tags should match 'Other Name'");
    }

    // ── TrackDetail struct ────────────────────────────────────────────────

    #[test]
    fn test_track_detail_in_backpack_false_by_default() {
        let detail = TrackDetail {
            id: 1,
            service: "spotify".to_string(),
            service_id: "s-1".to_string(),
            title: "Test".to_string(),
            artist: "Artist".to_string(),
            album: None,
            isrc: None,
            duration_ms: Some(240000),
            popularity: None,
            files: vec![],
            tags: vec![],
            playlists: vec![],
            in_backpack: false,
        };
        assert!(!detail.in_backpack);
        assert_eq!(detail.title, "Test");
    }

    // ── TrackDetail edge cases ─────────────────────────────────────────

    #[test]
    fn test_track_detail_default_fields() {
        let detail = TrackDetail {
            id: 42,
            service: "soundcloud".to_string(),
            service_id: "sc-abc".to_string(),
            title: "Cloud Track".to_string(),
            artist: "SC Artist".to_string(),
            album: None,
            isrc: Some("ISRC-SC-1".to_string()),
            duration_ms: None,
            popularity: None,
            files: vec![],
            tags: vec![],
            playlists: vec![],
            in_backpack: true,
        };
        assert!(detail.in_backpack);
        assert_eq!(detail.service_id, "sc-abc");
        assert_eq!(detail.isrc.as_deref(), Some("ISRC-SC-1"));
        assert!(detail.duration_ms.is_none());
        assert!(detail.files.is_empty());
    }

    #[test]
    fn test_track_detail_file_defaults() {
        let file = TrackDetailFile {
            id: 1,
            file_path: "/music/track.flac".to_string(),
            file_type: "flac".to_string(),
            stem_type: None,
            file_size: 5000i64,
            isrc: None,
            title: Some("Track".to_string()),
            artist: Some("Artist".to_string()),
            album: None,
            bpm: Some(128.0),
            musical_key: Some("8A".to_string()),
            duration_ms: Some(300000),
            bitrate: None,
            sample_rate: None,
            channels: None,
            comment: None,
            rating: Some(0),
            play_count: Some(5),
            last_played: Some(1000),
            backed_up: true,
            backup_path: Some("/backup/track.flac".to_string()),
            is_local: true,
        };
        assert_eq!(file.file_type, "flac");
        assert!(file.stem_type.is_none());
        assert!(file.backed_up);
        assert!(file.is_local);
        assert_eq!(file.play_count, Some(5));
    }

    // ── Key Comparison ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_key_comparison_empty() {
        let pool = test_db().await;
        let (results, summary) = get_key_comparison(&pool, None, Some(10)).await.unwrap();
        assert!(results.is_empty(), "no data should return empty results");
        assert_eq!(summary.total_compared, 0);
        assert_eq!(summary.bpm_match_count, 0);
        assert_eq!(summary.key_match_count, 0);
    }

    #[tokio::test]
    async fn test_get_key_comparison_with_tag() {
        let pool = test_db().await;

        // Insert a file
        sqlx::query(
            "INSERT INTO files (id, file_path, file_hash, file_type, file_size, bpm, musical_key, title, artist, isrc, spotify_id, created_at, updated_at)
             VALUES (1, '/test/track.flac', '', 'flac', 1000, 124.0, '8A', 'Test Track', 'Test Artist', 'ISRC-MATCH', 'spotify:track:abc', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Insert a service track with Spotify audio features
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, isrc, spotify_tempo, spotify_key_camelot, spotify_key_raw, spotify_mode, imported_at, updated_at)
             VALUES (1, 'spotify', 'spotify:track:abc', 'Test Track', 'Test Artist', 'ISRC-MATCH', 124.0, '8A', 5, 0, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Insert a tag and file_resolved_tags entry for filtering
        sqlx::query("INSERT INTO tags (id, name, category_id) VALUES (1, 'Groovy', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix)
             VALUES (1, 1, 'Groovy', 1, 'Setlist', 'S')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Test with tag filter
        let (results, summary) = get_key_comparison(&pool, Some("Groovy"), Some(10))
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "should find 1 matching file");
        assert_eq!(results[0].title, "Test Track");

        // BPM should match (124 vs 124, diff 0 <= 1)
        assert_eq!(results[0].bpm_match, Some(true));
        // Key should match (8A == 8A)
        assert_eq!(results[0].key_match, Some(true));

        assert_eq!(summary.total_compared, 1);
        assert_eq!(summary.bpm_match_count, 1);
        assert_eq!(summary.key_match_count, 1);
        assert_eq!(summary.bpm_mismatch_count, 0);
        assert_eq!(summary.key_mismatch_count, 0);
    }

    #[tokio::test]
    async fn test_get_key_comparison_mismatch() {
        let pool = test_db().await;

        // File with mismatched BPM and key
        sqlx::query(
            "INSERT INTO files (id, file_path, file_hash, file_type, file_size, bpm, musical_key, title, artist, isrc, spotify_id, created_at, updated_at)
             VALUES (1, '/test/mismatch.flac', '', 'flac', 1000, 140.0, '8A', 'Mismatch', 'Artist', 'ISRC-MM', 'spotify:track:mm', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Spotify says 128 BPM, 6A
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, isrc, spotify_tempo, spotify_key_camelot, spotify_key_raw, spotify_mode, imported_at, updated_at)
             VALUES (1, 'spotify', 'spotify:track:mm', 'Mismatch', 'Artist', 'ISRC-MM', 128.0, '6A', 3, 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let (results, summary) = get_key_comparison(&pool, None, Some(10)).await.unwrap();
        assert_eq!(results.len(), 1);

        // BPM mismatch: |140 - 128| = 12 > 1
        assert_eq!(results[0].bpm_match, Some(false));
        // Key mismatch: 8A != 6A
        assert_eq!(results[0].key_match, Some(false));

        assert_eq!(summary.total_compared, 1);
        assert_eq!(summary.bpm_mismatch_count, 1);
        assert_eq!(summary.key_mismatch_count, 1);
    }

    #[tokio::test]
    async fn test_get_key_comparison_missing_fields() {
        let pool = test_db().await;

        // File with NULL BPM and no linked track (no v_file_track_link match)
        sqlx::query(
            "INSERT INTO files (id, file_path, file_hash, file_type, file_size, title, artist, created_at, updated_at)
             VALUES (1, '/test/nobpm.flac', '', 'flac', 500, 'No BPM', 'Artist', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // This file has no matching service track — should not appear
        let (results, summary) = get_key_comparison(&pool, None, Some(10)).await.unwrap();
        assert!(
            results.is_empty(),
            "file without linked Spotify track should not appear"
        );
        assert_eq!(summary.total_compared, 0);
    }

    // ── get_service_tracks edge cases ──────────────────────────────────

    #[tokio::test]
    async fn test_get_service_tracks_returns_only_service_tracks() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO service_tracks (service, service_id, title, artist, imported_at, updated_at)
             VALUES  ('spotify', 's-1', 'A Track', 'A Artist', 0, 0),
                     ('local', 'l-1', 'Local Track', 'Local Artist', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let tracks = get_service_tracks(&pool).await.unwrap();
        assert_eq!(tracks.len(), 2, "should return all services");

        // Verify ordering: by service then title
        assert_eq!(tracks[0].service, "local");
        assert_eq!(tracks[1].service, "spotify");
    }
}
