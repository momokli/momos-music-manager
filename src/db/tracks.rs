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
        SELECT DISTINCT t.id, t.name, t.category_id, t.sort_order, t.created_at
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
