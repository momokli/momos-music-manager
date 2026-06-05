//! File-related database queries — scanning, metadata, comments, detail.

use std::collections::{HashMap, HashSet};
use std::{fs, path::Path, time::SystemTime};

use anyhow::{anyhow, Result};
use lofty::{prelude::*, read_from_path};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};
use tracing::{info, warn};

// Types are re-exported from super via `pub use legacy::*`, which has precedence
// over types.rs for the same type names (File, Tag, ScanMode, etc.).
// Importing from super::* avoids type-mismatch errors with scan_cache and
// other modules that expect the legacy::File type.
use super::*;
use crate::audio_extensions::AudioExtension;
use crate::db::calculate_file_hash;
use crate::scan_cache;

// ============================================================================
// Helper Functions
// ============================================================================

fn file_type_from_path(path: &Path) -> Option<&'static str> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| match ext.to_lowercase().as_str() {
            "flac" => Some("flac"),
            "mp3" => Some("mp3"),
            "m4a" => Some("stem.m4a"),
            "wav" => Some("wav"),
            "opus" => Some("opus"),
            _ => None,
        })
}

fn extract_tag_text(tag: &lofty::tag::Tag, key: ItemKey) -> Option<String> {
    tag.get_string(&key).map(|s| s.to_string())
}

fn parse_year(year_str: &str) -> Option<i32> {
    year_str.split('-').next()?.parse().ok()
}

fn parse_track_number(track_str: &str) -> Option<i32> {
    track_str.split('/').next()?.parse().ok()
}

fn parse_total_tracks(track_str: &str) -> Option<i32> {
    track_str.split('/').nth(1)?.parse().ok()
}

fn parse_disc_number(disc_str: &str) -> Option<i32> {
    disc_str.split('/').next()?.parse().ok()
}

fn parse_total_discs(disc_str: &str) -> Option<i32> {
    disc_str.split('/').nth(1)?.parse().ok()
}

fn parse_bpm(bpm_str: &str) -> Option<f64> {
    bpm_str.parse().ok()
}

/// Extract MP4 metadata using exiftool as fallback
#[allow(clippy::type_complexity)]
fn extract_mp4_metadata_with_exiftool(
    path: &Path,
) -> Result<(
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<f64>,
    Option<String>,
)> {
    use std::process::Command;

    tracing::debug!("Calling exiftool for: {:?}", path);

    let output = Command::new("exiftool")
        .arg("-json")
        .arg("-Title")
        .arg("-Artist")
        .arg("-Album")
        .arg("-Comment")
        .arg("-BeatsPerMinute")
        .arg("-InitialKey")
        .arg(path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("exiftool failed for {:?}: {}", path, stderr);
        return Err(anyhow!("exiftool failed: {}", stderr));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    tracing::debug!("exiftool raw output: {}", json_str);

    let json: serde_json::Value = serde_json::from_str(&json_str)?;

    let title = json[0]
        .get("Title")
        .and_then(|v| v.as_str())
        .map(String::from);
    let artist = json[0]
        .get("Artist")
        .and_then(|v| v.as_str())
        .map(String::from);
    let album = json[0]
        .get("Album")
        .and_then(|v| v.as_str())
        .map(String::from);
    let comment = json[0]
        .get("Comment")
        .and_then(|v| v.as_str())
        .map(String::from);
    let bpm = json[0].get("BeatsPerMinute").and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    });
    let key = json[0]
        .get("InitialKey")
        .and_then(|v| v.as_str())
        .map(String::from);

    tracing::debug!(
        "exiftool parsed: title={:?}, artist={:?}, album={:?}, comment={:?}, bpm={:?}, key={:?}",
        title,
        artist,
        album,
        comment,
        bpm,
        key
    );

    Ok((title, artist, album, comment, bpm, key))
}

/// Extract play count and last played date using exiftool (all file types).
/// Exiftool normalises these from iTunes `----:com.apple.iTunes:PLAY_COUNT` (M4A),
/// `POPM` frame (MP3), and similar tags across formats.
fn extract_playback_stats_with_exiftool(path: &Path) -> (Option<i32>, Option<i64>) {
    use std::process::Command;

    let output = match Command::new("exiftool")
        .arg("-json")
        .arg("-PlayCount")
        .arg("-PlayDate")
        .arg(path)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return (None, None),
    };

    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };

    let play_count = json[0]
        .get("PlayCount")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .map(|v| v as i32);

    let last_played = json[0].get("PlayDate").and_then(|v| {
        v.as_str().and_then(|s| {
            // Exiftool can return PlayDate as an epoch timestamp (iTunes-style)
            // or as "YYYY:MM:DD HH:MM:SS" — try parsing both.
            if let Ok(ts) = s.parse::<i64>() {
                return Some(ts);
            }
            // Try "YYYY:MM:DD HH:MM:SS" format (exiftool date format)
            if let Ok(ts) = chrono::NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S") {
                return Some(ts.and_utc().timestamp());
            }
            None
        })
    });

    (play_count, last_played)
}

// ============================================================================
// Types (file-specific, not yet in types.rs)
// ============================================================================

/// Rich detail view for a single file: Traktor metadata + ALL linked tracks
/// with audio features + tags + playlists.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDetail {
    // ── File (Traktor) ──
    pub id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub isrc: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub comment: Option<String>,
    pub rating: Option<i32>,
    pub play_count: Option<i32>,
    pub last_played: Option<i64>,
    // ── ALL linked tracks (not just Spotify) ──
    pub tracks: Vec<LinkedTrack>,
    // ── Tags ──
    pub tags: Vec<FileDetailTag>,
    // ── Playlists ──
    pub playlists: Vec<FileDetailPlaylist>,
}

/// Linked track in file detail view
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedTrack {
    pub id: i64,
    pub service: String,
    pub service_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub popularity: Option<i32>,
}

/// Tag in file detail view
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileDetailTag {
    pub id: i64,
    pub name: String,
    pub category_name: String,
    pub prefix: String,
}

/// Playlist in file detail view
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileDetailPlaylist {
    pub id: i64,
    pub name: String,
    pub service: String,
}

/// A single file variant (same ISRC or WAV source)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVariant {
    pub id: i64,
    pub file_type: String,
    pub stem_type: Option<String>,
    pub file_path: String,
    pub file_size: i64,
    pub backed_up: bool,
}

/// All file variants for a track
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVariants {
    pub file_id: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub isrc: Option<String>,
    pub variants: Vec<FileVariant>,
}

// ============================================================================
// Extract Metadata
// ============================================================================

pub async fn extract_minimal_file_metadata(path: &Path) -> Result<File> {
    let file_type = file_type_from_path(path)
        .ok_or_else(|| anyhow!("Unsupported file type: {:?}", path.extension()))?
        .to_string();
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len() as i64;
    let last_modified = metadata
        .modified()
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        })
        .unwrap_or(0);
    // WAV source files don't need dedup — skip expensive SHA256
    let file_hash = format!("wav-{}", file_size);
    let now = chrono::Utc::now().timestamp();

    Ok(File {
        id: 0,
        file_path: path.to_string_lossy().to_string(),
        file_hash,
        file_type,
        file_size,
        last_modified,
        isrc: None,
        last_scanned: now,
        title: None,
        artist: None,
        album: None,
        album_artist: None,
        track_number: None,
        total_tracks: None,
        disc_number: None,
        total_discs: None,
        genre: None,
        year: None,
        composer: None,
        comment: None,
        duration_ms: None,
        bitrate: None,
        sample_rate: None,
        channels: None,
        bpm: None,
        musical_key: None,
        rating: 0,
        play_count: 0,
        last_played: None,
        spotify_id: None,
        soundcloud_id: None,
        youtube_id: None,
        source_of: None,
        stem_type: None,
        last_verified_local: None,
        created_at: now,
        updated_at: now,
    })
}

pub async fn extract_audio_metadata_from_file(path: &Path) -> Result<File> {
    // For WAV files (nuo-stems sources), skip metadata extraction.
    // These are raw audio stems with no tags — exiftool is unnecessary overhead.
    let file_type = file_type_from_path(path)
        .ok_or_else(|| anyhow!("Unsupported file type: {:?}", path.extension()))?;

    if file_type == "wav" {
        return Ok(extract_minimal_file_metadata(path).await?);
    }
    // Get file metadata
    let metadata = fs::metadata(path)?;
    let file_size = metadata.len() as i64;
    let last_modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Calculate file hash
    // WAV source files don't need dedup — skip expensive SHA256
    let file_hash = calculate_file_hash(path)?;

    // ── CACHE CHECK ──────────────────────────────────────────────────────────
    // In replay mode, skip lofty/exiftool entirely and return cached metadata.
    // The cache is invalidated when the file's mtime or hash changes.
    let normalized_cache_path = shellexpand::full(path.to_string_lossy().as_ref())
        .unwrap_or_else(|_| path.to_string_lossy())
        .to_string();
    match scan_cache::try_load(&normalized_cache_path, last_modified, &file_hash).await {
        scan_cache::CacheResult::Hit(cached_file) => {
            tracing::debug!("CACHE HIT for {:?}, skipping extraction", path);
            return Ok(cached_file);
        }
        scan_cache::CacheResult::Miss => {
            tracing::debug!("CACHE MISS for {:?}, extracting normally", path);
        }
    }

    // Determine file type
    let file_type = file_type_from_path(path)
        .ok_or_else(|| anyhow!("Unsupported file type: {:?}", path.extension()))?
        .to_string();

    // Read audio file with lofty (gracefully handle failure)
    let tagged_file = read_from_path(path);
    let (properties, lofty_tag): (
        Option<(i64, Option<i32>, Option<i32>, Option<i32>)>,
        Option<lofty::tag::Tag>,
    );

    match tagged_file {
        Ok(file) => {
            let p = file.properties();
            let dur = p.duration().as_millis() as i64;
            let br = p.audio_bitrate().map(|b| b as i32);
            let sr = p.sample_rate().map(|s| s as i32);
            let ch = p.channels().map(|c| c as i32);
            properties = Some((dur, br, sr, ch));
            lofty_tag = file.primary_tag().cloned();
        }
        Err(e) => {
            tracing::debug!(
                "lofty failed for {:?}: {} - will rely on exiftool fallback entirely",
                path,
                e
            );
            properties = None;
            lofty_tag = None;
        }
    }

    // Extract metadata from tags
    let mut title = None;
    let mut artist = None;
    let mut album = None;
    let mut album_artist = None;
    let mut track_number = None;
    let mut total_tracks = None;
    let mut disc_number = None;
    let mut total_discs = None;
    let mut genre = None;
    let mut year = None;
    let mut composer = None;
    let mut comment = None;
    let mut bpm = None;
    let mut musical_key = None;
    let mut isrc = None;
    let mut play_count = None;
    let mut last_played = None;

    if let Some(ref tag) = lofty_tag {
        // Basic metadata
        title = extract_tag_text(tag, ItemKey::TrackTitle);
        artist = extract_tag_text(tag, ItemKey::TrackArtist);
        album = extract_tag_text(tag, ItemKey::AlbumTitle);
        album_artist = extract_tag_text(tag, ItemKey::AlbumArtist);

        // Track numbers
        if let Some(track_str) = extract_tag_text(tag, ItemKey::TrackNumber) {
            track_number = parse_track_number(&track_str);
            total_tracks = parse_total_tracks(&track_str);
        }

        // Disc numbers
        if let Some(disc_str) = extract_tag_text(tag, ItemKey::DiscNumber) {
            disc_number = parse_disc_number(&disc_str);
            total_discs = parse_total_discs(&disc_str);
        }

        // Other metadata
        genre = extract_tag_text(tag, ItemKey::Genre);

        if let Some(year_str) = extract_tag_text(tag, ItemKey::Year) {
            year = parse_year(&year_str);
        }

        composer = extract_tag_text(tag, ItemKey::Composer);
        comment = extract_tag_text(tag, ItemKey::Comment);

        // BPM and key
        if let Some(bpm_str) = extract_tag_text(tag, ItemKey::Bpm) {
            bpm = parse_bpm(&bpm_str);
        }

        musical_key = extract_tag_text(tag, ItemKey::InitialKey);

        // ISRC
        isrc = extract_tag_text(tag, ItemKey::Isrc);
    }

    // Extract play count and last played via exiftool (handles all formats)
    let (exif_play_count, exif_last_played) = extract_playback_stats_with_exiftool(path);
    if exif_play_count.is_some() {
        play_count = exif_play_count;
    }
    if exif_last_played.is_some() {
        last_played = exif_last_played;
    }

    tracing::debug!(
        "Playback stats for {:?}: play_count={:?}, last_played={:?}",
        path,
        play_count,
        last_played
    );

    // Try exiftool as fallback for MP4 files (when lofty fails entirely or for specific metadata)
    if (path.to_string_lossy().ends_with(".m4a") || path.to_string_lossy().ends_with(".mp4"))
        && (bpm.is_none()
            || comment.is_none()
            || musical_key.is_none()
            || title.is_none()
            || artist.is_none()
            || album.is_none())
    {
        tracing::debug!(
            "Trying exiftool fallback for {:?} (missing: title={:?}, artist={:?}, album={:?}, bpm={:?}, comment={:?}, key={:?})",
            path,
            title.is_none(),
            artist.is_none(),
            album.is_none(),
            bpm.is_none(),
            comment.is_none(),
            musical_key.is_none()
        );
        match extract_mp4_metadata_with_exiftool(path) {
            Ok((exif_title, exif_artist, exif_album, exif_comment, exif_bpm, exif_key)) => {
                tracing::debug!(
                    "exiftool fallback successful: title={:?}, artist={:?}, album={:?}, comment={:?}, bpm={:?}, key={:?}",
                    exif_title,
                    exif_artist,
                    exif_album,
                    exif_comment,
                    exif_bpm,
                    exif_key
                );
                if title.is_none() {
                    title = exif_title;
                }
                if artist.is_none() {
                    artist = exif_artist;
                }
                if album.is_none() {
                    album = exif_album;
                }
                if comment.is_none() {
                    comment = exif_comment;
                }
                if bpm.is_none() {
                    bpm = exif_bpm;
                }
                if musical_key.is_none() {
                    musical_key = exif_key;
                }
            }
            Err(e) => {
                tracing::debug!("exiftool failed for {:?}: {}", path, e);
            }
        }
    }

    // Audio properties (from lofty if available)
    let duration_ms = properties.map(|(d, _, _, _)| d).unwrap_or(0);
    let bitrate = properties.and_then(|(_, b, _, _)| b);
    let sample_rate = properties.and_then(|(_, _, s, _)| s);
    let channels = properties.and_then(|(_, _, _, c)| c);

    let last_scanned = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Normalize path to ~/ format if in home directory
    let normalized_path = match shellexpand::full(path.to_string_lossy().as_ref()) {
        Ok(expanded) => expanded.to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    };

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let scanned_file = File {
        id: 0, // Will be set by database
        file_path: normalized_path,
        file_hash,
        file_type,
        file_size,
        last_modified,
        isrc,
        last_scanned,
        title,
        artist,
        album,
        album_artist,
        track_number,
        total_tracks,
        disc_number,
        total_discs,
        genre,
        year,
        composer,
        comment,
        duration_ms: Some(duration_ms),
        bitrate,
        sample_rate,
        channels,
        bpm,
        musical_key,
        rating: 0,
        play_count: play_count.unwrap_or(0),
        last_played,
        spotify_id: None,
        soundcloud_id: None,
        youtube_id: None,
        source_of: None,
        stem_type: None,
        last_verified_local: None,
        created_at: now,
        updated_at: now,
    };

    // ── CACHE SAVE ──────────────────────────────────────────────────────────
    // In record mode, persist the extracted metadata for future replay.
    scan_cache::try_save(&scanned_file).await;

    Ok(scanned_file)
}

// ============================================================================
// Scan & Store
// ============================================================================

pub async fn scan_and_store_file(pool: &Pool<Sqlite>, path: &Path) -> Result<File> {
    let file = extract_audio_metadata_from_file(path).await?;

    let row = sqlx::query_as::<_, File>(
        r#"
        INSERT INTO files (
            file_path, file_hash, file_type, file_size, last_modified, isrc, last_scanned,
            title, artist, album, album_artist, track_number, total_tracks, disc_number, total_discs,
            genre, year, composer, comment, duration_ms, bitrate, sample_rate, channels,
            bpm, musical_key, rating, play_count, last_played,
            spotify_id, soundcloud_id, youtube_id, source_of, stem_type, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(file_path) DO UPDATE SET
            file_hash = excluded.file_hash,
            file_type = excluded.file_type,
            file_size = excluded.file_size,
            last_modified = excluded.last_modified,
            isrc = excluded.isrc,
            last_scanned = excluded.last_scanned,
            title = excluded.title,
            artist = excluded.artist,
            album = excluded.album,
            album_artist = excluded.album_artist,
            track_number = excluded.track_number,
            total_tracks = excluded.total_tracks,
            disc_number = excluded.disc_number,
            total_discs = excluded.total_discs,
            genre = excluded.genre,
            year = excluded.year,
            composer = excluded.composer,
            comment = excluded.comment,
            duration_ms = excluded.duration_ms,
            bitrate = excluded.bitrate,
            sample_rate = excluded.sample_rate,
            channels = excluded.channels,
            bpm = excluded.bpm,
            musical_key = excluded.musical_key,
            rating = excluded.rating,
            play_count = excluded.play_count,
            last_played = excluded.last_played,
            spotify_id = excluded.spotify_id,
            soundcloud_id = excluded.soundcloud_id,
            youtube_id = excluded.youtube_id,
            source_of = COALESCE(excluded.source_of, files.source_of),
            stem_type = COALESCE(excluded.stem_type, files.stem_type),
            updated_at = excluded.updated_at
        RETURNING *
        "#,
    )
    .bind(&file.file_path)
    .bind(&file.file_hash)
    .bind(&file.file_type)
    .bind(file.file_size)
    .bind(file.last_modified)
    .bind(&file.isrc)
    .bind(file.last_scanned)
    .bind(&file.title)
    .bind(&file.artist)
    .bind(&file.album)
    .bind(&file.album_artist)
    .bind(file.track_number)
    .bind(file.total_tracks)
    .bind(file.disc_number)
    .bind(file.total_discs)
    .bind(&file.genre)
    .bind(file.year)
    .bind(&file.composer)
    .bind(&file.comment)
    .bind(file.duration_ms)
    .bind(file.bitrate)
    .bind(file.sample_rate)
    .bind(file.channels)
    .bind(file.bpm)
    .bind(&file.musical_key)
    .bind(file.rating)
    .bind(file.play_count)
    .bind(file.last_played)
    .bind(&file.spotify_id)
    .bind(&file.soundcloud_id)
    .bind(&file.youtube_id)
    .bind(&file.source_of)
    .bind(&file.stem_type)
    .bind(file.created_at)
    .bind(file.updated_at)
    .fetch_one(pool)
    .await?;

    // Track local presence
    let _ = sqlx::query(
        "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
         VALUES (?, 'local', ?, ?, unixepoch(), unixepoch())
         ON CONFLICT(file_id, location_type) DO UPDATE SET
             file_size = excluded.file_size,
             last_verified = excluded.last_verified"
    )
    .bind(row.id)
    .bind(&row.file_path)
    .bind(row.file_size)
    .execute(pool)
    .await;

    let _ = sqlx::query("UPDATE files SET last_verified_local = unixepoch() WHERE id = ?")
        .bind(row.id)
        .execute(pool)
        .await;

    Ok(row)
}

pub async fn scan_directory(pool: &Pool<Sqlite>, dir_path: &Path) -> Result<usize> {
    // Use default configuration: recursive, all audio extensions
    scan_directory_with_config(
        pool,
        dir_path,
        true,
        false,
        String::new(),
        0,
        ScanMode::Full,
    )
    .await
}

pub async fn scan_directory_with_config(
    pool: &Pool<Sqlite>,
    dir_path: &Path,
    scan_recursive: bool,
    fixed_extensions: bool,
    file_extensions: String,
    max_depth: i32,
    scan_mode: ScanMode,
) -> Result<usize> {
    use walkdir::WalkDir;

    // Check if directory exists
    if !dir_path.exists() {
        return Err(anyhow!("Directory does not exist: {}", dir_path.display()));
    }
    if !dir_path.is_dir() {
        return Err(anyhow!("Path is not a directory: {}", dir_path.display()));
    }

    info!(
        "Starting directory scan with config: recursive={}, fixed_extensions={}, max_depth={}, path={}",
        scan_recursive,
        fixed_extensions,
        max_depth,
        dir_path.display()
    );
    match &scan_mode {
        ScanMode::Full => info!("Scan mode: full — scanning all files"),
        ScanMode::Incremental { since: Some(ts) } => {
            info!("Scan mode: incremental — skipping files older than {}", ts)
        }
        ScanMode::Incremental { since: None } => {
            info!("Scan mode: incremental — no previous scan, doing full scan")
        }
    }

    // Parse allowed extensions if fixed_extensions is true
    let allowed_extensions = if fixed_extensions && !file_extensions.trim().is_empty() {
        AudioExtension::parse_list(&file_extensions).map_err(|e| {
            anyhow!(
                "Failed to parse file extensions '{}': {}",
                file_extensions,
                e
            )
        })?
    } else {
        Vec::new() // Empty means all audio extensions
    };

    // Configure walkdir based on scan_recursive and max_depth
    let walker = if scan_recursive {
        if max_depth > 0 {
            WalkDir::new(dir_path).max_depth(max_depth as usize)
        } else {
            WalkDir::new(dir_path)
        }
    } else {
        // Not recursive = depth 1 (top-level only)
        WalkDir::new(dir_path).max_depth(1)
    };

    let mut count = 0;
    let mut total_files = 0;
    let mut skipped_files = 0;

    for entry in walker.follow_links(true).into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                warn!("Failed to access directory entry: {}", e);
                continue;
            }
        };
        let path = entry.path();
        if path.is_file() {
            total_files += 1;

            // Check if file has an audio extension we should process
            let should_process = if fixed_extensions && !allowed_extensions.is_empty() {
                // Check if file matches any of the allowed extensions
                allowed_extensions
                    .iter()
                    .any(|ext| ext.matches_file(&path.to_string_lossy()))
            } else {
                // Wildcard mode: check if file has any audio extension
                AudioExtension::from_file_path(&path.to_string_lossy()).is_some()
            };

            if should_process {
                // Check if we can skip this file (incremental scan)
                if let ScanMode::Incremental {
                    since: Some(cutoff),
                } = &scan_mode
                    && let Ok(metadata) = entry.metadata()
                    && let Ok(modified) = metadata.modified()
                {
                    let mtime = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if mtime <= *cutoff {
                        // File hasn't changed since last scan, skip it
                        skipped_files += 1;
                        continue;
                    }
                }

                match scan_and_store_file(pool, path).await {
                    Ok(_) => {
                        count += 1;
                        if count % 10 == 0 {
                            info!("Scanned {} files...", count);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to scan file {:?}: {}", path, e);
                        skipped_files += 1;
                    }
                }
            } else {
                // Log skipped extensions at debug level to avoid noise
                tracing::debug!("Skipping file with unsupported extension: {:?}", path);
                skipped_files += 1;
            }
        }
    }

    info!(
        "Scan complete. Found {} total files, scanned {}, skipped {}.",
        total_files, count, skipped_files
    );
    Ok(count)
}

// ============================================================================
// File Queries
// ============================================================================

pub async fn get_files(pool: &Pool<Sqlite>) -> Result<Vec<File>> {
    let files = sqlx::query_as::<_, File>("SELECT * FROM files ORDER BY file_path")
        .fetch_all(pool)
        .await?;
    Ok(files)
}

pub async fn get_file_by_id(pool: &Pool<Sqlite>, id: i64) -> Result<Option<File>> {
    let file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(file)
}

pub async fn get_file_by_path(pool: &Pool<Sqlite>, file_path: &str) -> Result<Option<File>> {
    let file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE file_path = ?")
        .bind(file_path)
        .fetch_optional(pool)
        .await?;
    Ok(file)
}

pub async fn get_tags_for_file(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<Tag>> {
    // Get all tags for this file from file_resolved_tags table (materialized)
    let tags = sqlx::query_as::<_, Tag>(
        r#"
        SELECT DISTINCT frt.tag_id as id, frt.tag_name as name, frt.category_id, frt.sort_order, frt.created_at, 0 as backpack
        FROM file_resolved_tags frt
        WHERE frt.file_id = ?
        ORDER BY frt.tag_name
        "#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    Ok(tags)
}

// ============================================================================
// Comment Operations
// ============================================================================

pub async fn update_file_comment(pool: &Pool<Sqlite>, file_id: i64, comment: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query("UPDATE files SET comment = ?, updated_at = ? WHERE id = ?")
        .bind(comment)
        .bind(now)
        .bind(file_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Read the comment tag from a file on disk using exiftool.
/// Returns `None` if the file has no comment tag.
pub async fn read_comment_from_file(file_path: &str) -> Result<Option<String>> {
    use std::process::Command;

    let output = Command::new("exiftool")
        .arg("-json")
        .arg("-Comment")
        .arg(file_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("exiftool failed: {}", stderr));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&json_str)?;

    let comment = json[0]
        .get("Comment")
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(comment)
}

/// Write comment to file using exiftool.
///
/// FLAC files use `metaflac` because the macOS build of exiftool does
/// not include FLAC write support.  All other formats use exiftool.
pub async fn write_comment_to_file(file_path: &str, comment: &str) -> Result<()> {
    use std::process::Command;

    let is_flac = file_path.to_lowercase().ends_with(".flac");

    if is_flac {
        let output = Command::new("metaflac")
            .arg("--set-tag")
            .arg(format!("COMMENT={}", comment))
            .arg(file_path)
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to write FLAC comment: {}", error));
        }
    } else {
        let comment_tag = format!("-Comment={}", comment);
        let output = Command::new("exiftool")
            .arg("-overwrite_original")
            .arg(&comment_tag)
            .arg(file_path)
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to write comment: {}", error));
        }
    }

    Ok(())
}

/// Compute the target comment for a file based on its service track associations
/// Algorithm:
/// 1. Find matching service tracks via ISRC or service IDs
/// 2. Find playlists those tracks belong to
/// 3. Find tags matching playlist names (case-insensitive)
/// 4. Determine PMV characters from tag categories (Phase, Mood, Vibe)
/// 5. Sort tags by category sort_order then alphabetically
/// 6. Collect service IDs from file columns
/// 7. Format as "[{pmv}] {sorted_tags} {service_ids}"
pub async fn compute_target_comment(pool: &Pool<Sqlite>, file_id: i64) -> Result<String> {
    use crate::comment::generate_target_comment;

    // Get file with service IDs
    let file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_one(pool)
        .await?;

    // Get all tags for this file with category prefix via file_resolved_tags table
    // (materialized from v_file_resolved_tags view — parent-resolved)
    let tag_rows = sqlx::query(
        "SELECT frt.tag_name, frt.prefix
         FROM file_resolved_tags frt
         WHERE frt.file_id = ?
         ORDER BY frt.sort_order, frt.tag_name",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    let mut phase_present = false;
    let mut mood_present = false;
    let mut vibe_present = false;
    let mut tags: Vec<String> = Vec::new();

    for row in tag_rows {
        let tag_name: String = row.try_get("tag_name")?;
        let prefix: String = row.try_get("prefix")?;

        match prefix.as_str() {
            "P" => phase_present = true,
            "M" => mood_present = true,
            "V" => vibe_present = true,
            _ => {}
        }

        tags.push(tag_name);
    }

    let phase_char = if phase_present { 'P' } else { '_' };
    let mood_char = if mood_present { 'M' } else { '_' };
    let vibe_char = if vibe_present { 'V' } else { '_' };

    Ok(generate_target_comment(
        phase_char,
        mood_char,
        vibe_char,
        &tags,
        file.spotify_id.as_deref(),
        file.soundcloud_id.as_deref(),
        file.youtube_id.as_deref(),
    ))
}

/// Batch version of `compute_target_comment`. Fetches ALL resolved tags for ALL given
/// file IDs in a single query, then computes target comments in Rust.
pub async fn compute_target_comments_batch(
    pool: &Pool<Sqlite>,
    file_ids: &[i64],
) -> Result<HashMap<i64, String>> {
    use crate::comment::generate_target_comment;

    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Build parameterized IN clause
    let mut file_id_params: Vec<String> = Vec::new();
    for _ in file_ids {
        file_id_params.push("?".to_string());
    }
    let in_clause = file_id_params.join(", ");

    // 1. Fetch source IDs for all files
    let files_sql = format!(
        "SELECT id, spotify_id, soundcloud_id, youtube_id FROM files WHERE id IN ({})",
        in_clause
    );
    let mut files_query =
        sqlx::query_as::<_, (i64, Option<String>, Option<String>, Option<String>)>(&files_sql);
    for fid in file_ids {
        files_query = files_query.bind(fid);
    }
    let file_rows: Vec<(i64, Option<String>, Option<String>, Option<String>)> =
        files_query.fetch_all(pool).await?;

    // 2. Fetch all tags for all file_ids from file_resolved_tags in one query
    let tags_sql = format!(
        "SELECT file_id, tag_name, prefix FROM file_resolved_tags WHERE file_id IN ({}) ORDER BY file_id, sort_order, tag_name",
        in_clause
    );
    let mut tags_query = sqlx::query_as::<_, (i64, String, String)>(&tags_sql);
    for fid in file_ids {
        tags_query = tags_query.bind(fid);
    }
    let tag_rows: Vec<(i64, String, String)> = tags_query.fetch_all(pool).await?;

    // 3. Group tags by file_id
    let mut tags_by_file: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    for (file_id, tag_name, prefix) in tag_rows {
        tags_by_file
            .entry(file_id)
            .or_default()
            .push((tag_name, prefix));
    }

    // 4. Build file_id → source_ids lookup
    let mut source_by_file: HashMap<i64, (Option<String>, Option<String>, Option<String>)> =
        HashMap::new();
    for (fid, spotify, soundcloud, youtube) in file_rows {
        source_by_file.insert(fid, (spotify, soundcloud, youtube));
    }

    // 5. Compute comments for each file
    let mut results: HashMap<i64, String> = HashMap::new();
    for fid in file_ids {
        let tags = match tags_by_file.get(fid) {
            Some(t) => t,
            None => continue,
        };

        let sources = match source_by_file.get(fid) {
            Some(s) => s,
            None => continue,
        };

        let mut phase_present = false;
        let mut mood_present = false;
        let mut vibe_present = false;
        let mut tag_names: Vec<String> = Vec::new();

        for (tag_name, prefix) in tags {
            match prefix.as_str() {
                "P" => phase_present = true,
                "M" => mood_present = true,
                "V" => vibe_present = true,
                _ => {}
            }
            tag_names.push(tag_name.clone());
        }

        let phase_char = if phase_present { 'P' } else { '_' };
        let mood_char = if mood_present { 'M' } else { '_' };
        let vibe_char = if vibe_present { 'V' } else { '_' };

        let comment = generate_target_comment(
            phase_char,
            mood_char,
            vibe_char,
            &tag_names,
            sources.0.as_deref(),
            sources.1.as_deref(),
            sources.2.as_deref(),
        );

        results.insert(*fid, comment);
    }

    Ok(results)
}

/// Fetch raw resolved tag rows for a batch of file IDs. Returns (file_id, tag_name, prefix, category_name, sort_order).
pub async fn get_file_resolved_tags_batch(
    pool: &Pool<Sqlite>,
    file_ids: &[i64],
) -> Result<Vec<(i64, String, String, String, i64)>> {
    if file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut params: Vec<String> = Vec::new();
    for _ in file_ids {
        params.push("?".to_string());
    }
    let in_clause = params.join(", ");

    let sql = format!(
        r#"SELECT file_id, tag_name, prefix, category_name, sort_order
           FROM file_resolved_tags
           WHERE file_id IN ({})
           ORDER BY file_id, sort_order, tag_name"#,
        in_clause
    );

    let mut query = sqlx::query_as::<_, (i64, String, String, String, i64)>(&sql);
    for fid in file_ids {
        query = query.bind(fid);
    }

    let rows = query.fetch_all(pool).await?;
    Ok(rows)
}

// ============================================================================
// File Detail & Variants
// ============================================================================

/// Fetch rich detail for a single file: Traktor metadata + ALL linked tracks
/// with audio features + tags + playlists.
pub async fn get_file_detail(pool: &Pool<Sqlite>, file_id: i64) -> Result<Option<FileDetail>> {
    // 1. Fetch the file
    let file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_optional(pool)
        .await?;

    let Some(file) = file else {
        return Ok(None);
    };

    // 2. Fetch ALL linked tracks via v_file_track_link
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
    }

    let track_rows: Vec<TrackRow> = sqlx::query_as(
        r#"
        SELECT st.id, st.service, st.service_id, st.title, st.artist, st.album,
               st.isrc, st.duration_ms, st.metadata_json
        FROM v_file_track_link v
        JOIN service_tracks st ON st.id = v.track_id
        WHERE v.file_id = ?
        "#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    let tracks: Vec<LinkedTrack> = track_rows
        .into_iter()
        .map(|r| {
            let popularity = r
                .metadata_json
                .as_ref()
                .and_then(|json_str| {
                    serde_json::from_str::<serde_json::Value>(json_str)
                        .ok()
                        .and_then(|v| v.get("popularity").and_then(|p| p.as_i64()))
                })
                .map(|p| p as i32);

            LinkedTrack {
                id: r.id,
                service: r.service,
                service_id: r.service_id,
                title: r.title,
                artist: r.artist,
                album: r.album,
                isrc: r.isrc,
                duration_ms: r.duration_ms,
                popularity,
            }
        })
        .collect();

    // 3. Fetch tags via file_resolved_tags
    let tags: Vec<FileDetailTag> = sqlx::query_as(
        r#"
        SELECT DISTINCT frt.tag_id as id, frt.tag_name as name,
               frt.category_name, frt.prefix
        FROM file_resolved_tags frt
        WHERE frt.file_id = ?
        ORDER BY frt.category_name, frt.tag_name
        "#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // 4. Fetch linked playlists
    let playlists: Vec<FileDetailPlaylist> = sqlx::query_as(
        r#"
        SELECT DISTINCT sp.id, sp.name, sp.service
        FROM v_file_track_link v
        JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        WHERE v.file_id = ?
        ORDER BY sp.name
        "#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    Ok(Some(FileDetail {
        id: file.id,
        file_path: file.file_path,
        file_type: file.file_type,
        file_size: file.file_size,
        isrc: file.isrc,
        title: file.title,
        artist: file.artist,
        album: file.album,
        genre: file.genre,
        year: file.year,
        duration_ms: file.duration_ms,
        bitrate: file.bitrate,
        sample_rate: file.sample_rate,
        channels: file.channels,
        bpm: file.bpm,
        musical_key: file.musical_key,
        comment: file.comment,
        rating: if file.rating > 0 {
            Some(file.rating)
        } else {
            None
        },
        play_count: Some(file.play_count),
        last_played: file.last_played,
        tracks,
        tags,
        playlists,
    }))
}

/// Fetch all file variants for a track: same ISRC files + WAV source files
/// belonging to the same stem.
pub async fn get_file_variants(pool: &Pool<Sqlite>, file_id: i64) -> Result<Option<FileVariants>> {
    // 1. Fetch the file
    let file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_optional(pool)
        .await?;

    let Some(file) = file else {
        return Ok(None);
    };

    // 2. Collect all variant IDs
    let mut variant_ids = HashSet::new();
    variant_ids.insert(file.id);

    // Same ISRC (if ISRC is not null)
    if let Some(ref isrc) = file.isrc {
        let same_isrc: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM files WHERE isrc = ? AND id != ?")
                .bind(isrc)
                .bind(file.id)
                .fetch_all(pool)
                .await?;
        variant_ids.extend(same_isrc);
    }

    // WAV source files (source_of points to this stem file)
    let wav_sources: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM files WHERE source_of = ? AND file_type = 'wav'")
            .bind(file.id)
            .fetch_all(pool)
            .await?;
    variant_ids.extend(wav_sources);

    // If this file is a WAV, include its stem parent and siblings
    if let Some(source_of) = file.source_of {
        variant_ids.insert(source_of);
        let sibling_wavs: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM files WHERE source_of = ? AND id != ?")
                .bind(source_of)
                .bind(file.id)
                .fetch_all(pool)
                .await?;
        variant_ids.extend(sibling_wavs);
    }

    // 3. Fetch variant details
    let ids: Vec<i64> = variant_ids.into_iter().collect();
    if ids.is_empty() {
        return Ok(Some(FileVariants {
            file_id: file.id,
            title: file.title,
            artist: file.artist,
            isrc: file.isrc,
            variants: vec![],
        }));
    }

    let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size, f.stem_type,
                CASE WHEN fl.id IS NOT NULL THEN 1 ELSE 0 END as backed_up
         FROM files f
         LEFT JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
         WHERE f.id IN ({})
         ORDER BY f.file_type, f.stem_type",
        placeholders.join(",")
    );

    let mut query = sqlx::query_as::<_, (i64, String, String, i64, Option<String>, bool)>(&sql);
    for id in &ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;

    let variants: Vec<FileVariant> = rows
        .into_iter()
        .map(
            |(id, file_path, file_type, file_size, stem_type, backed_up)| FileVariant {
                id,
                file_type,
                stem_type,
                file_path,
                file_size,
                backed_up,
            },
        )
        .collect();

    Ok(Some(FileVariants {
        file_id: file.id,
        title: file.title,
        artist: file.artist,
        isrc: file.isrc,
        variants,
    }))
}

/// Find tracks similar to a given file by tag similarity (uses tag_similarities table).
pub async fn find_tag_similar_tracks(
    pool: &Pool<Sqlite>,
    file_id: i64,
    limit: i64,
) -> Result<
    Vec<(
        i64,
        String,
        Option<String>,
        Option<f64>,
        Option<String>,
        f64,
        String,
    )>,
> {
    // 1. Get seed file's tags
    let seed_tags = get_tags_for_file(pool, file_id).await?;
    if seed_tags.is_empty() {
        return Ok(Vec::new());
    }

    let seed_tag_ids: Vec<i64> = seed_tags.iter().map(|t| t.id).collect();
    let seed_tag_count = seed_tag_ids.len();

    // 2. For each seed tag, find similar tags (similarity > 0.15, top 10 per tag)
    let mut similar_tag_map: HashMap<i64, Vec<(i64, f32)>> = HashMap::new();

    for &seed_tid in &seed_tag_ids {
        let similar = sqlx::query_as::<_, (i64, f32)>(
            r#"
            SELECT
                CASE WHEN tag_a_id = ? THEN tag_b_id ELSE tag_a_id END as similar_tag_id,
                similarity
            FROM tag_similarities
            WHERE (tag_a_id = ? OR tag_b_id = ?)
              AND similarity > 0.15
            ORDER BY similarity DESC
            LIMIT 10
            "#,
        )
        .bind(seed_tid)
        .bind(seed_tid)
        .bind(seed_tid)
        .fetch_all(pool)
        .await?;

        for (similar_tag_id, sim) in similar {
            similar_tag_map
                .entry(similar_tag_id)
                .or_default()
                .push((seed_tid, sim));
        }
    }

    // Remove seed tags themselves from candidates
    for tid in &seed_tag_ids {
        similar_tag_map.remove(tid);
    }

    if similar_tag_map.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_tag_ids: Vec<i64> = similar_tag_map.keys().copied().collect();

    // Build a map of tag_id -> tag_name for seed tags
    let seed_tag_name_map: HashMap<i64, &str> =
        seed_tags.iter().map(|t| (t.id, t.name.as_str())).collect();

    // Build a map of tag_id -> (tag_name, category_name) for candidate tags
    let mut candidate_tag_info: HashMap<i64, (String, String)> = HashMap::new();

    {
        let tag_ph: Vec<String> = candidate_tag_ids.iter().map(|_| "?".to_string()).collect();
        let info_sql = format!(
            "SELECT t.id, t.name, tc.name as cat FROM tags t JOIN tag_categories tc ON tc.id = t.category_id WHERE t.id IN ({})",
            tag_ph.join(",")
        );
        let mut q = sqlx::query(&info_sql);
        for tid in &candidate_tag_ids {
            q = q.bind(tid);
        }
        let rows = q.fetch_all(pool).await?;
        for row in rows {
            let tid: i64 = row.try_get("id")?;
            let name: String = row.try_get("name")?;
            let cat: String = row.try_get("cat")?;
            candidate_tag_info.insert(tid, (name, cat));
        }
    }

    // 3. Find all files that have any of these candidate tags
    let tag_placeholders: Vec<String> = candidate_tag_ids.iter().map(|_| "?".to_string()).collect();

    let files_sql = format!(
        r#"
        SELECT DISTINCT f.id, f.title, f.artist, f.bpm, f.musical_key,
               frt.tag_name, frt.tag_id
        FROM files f
        JOIN file_resolved_tags frt ON frt.file_id = f.id
        WHERE frt.tag_id IN ({})
          AND f.id != ?
        ORDER BY f.id
        "#,
        tag_placeholders.join(",")
    );

    let mut file_scores: HashMap<
        i64,
        (
            String,
            Option<String>,
            Option<f64>,
            Option<String>,
            f64,
            Vec<(String, String, f32)>,
        ),
    > = HashMap::new();

    let mut file_query = sqlx::query(&files_sql);
    for tid in &candidate_tag_ids {
        file_query = file_query.bind(tid);
    }
    file_query = file_query.bind(file_id);

    let rows = file_query.fetch_all(pool).await?;

    for row in rows {
        let fid: i64 = row.try_get("id")?;
        let title: String = row.try_get("title")?;
        let artist: Option<String> = row.try_get("artist")?;
        let bpm: Option<f64> = row.try_get("bpm")?;
        let key: Option<String> = row.try_get("musical_key")?;
        let tag_name: String = row.try_get("tag_name")?;
        let tag_id: i64 = row.try_get("tag_id")?;

        // Find matching seed tags and their similarities
        if let Some(seed_matches) = similar_tag_map.get(&tag_id) {
            let entry = file_scores
                .entry(fid)
                .or_insert_with(|| (title.clone(), artist.clone(), bpm, key, 0.0f64, Vec::new()));

            // For each seed match, add to matched_tags if not already there
            for (seed_tid, _sim) in seed_matches {
                let seed_name = seed_tag_name_map.get(seed_tid).copied().unwrap_or("?");

                // Check if we already have this pair
                let already = entry.5.iter().any(|(s, _, _)| s == seed_name);
                if !already {
                    let best_sim = seed_matches
                        .iter()
                        .filter(|(st, _)| st == seed_tid)
                        .map(|(_, s)| *s)
                        .fold(0.0f32, f32::max);

                    entry
                        .5
                        .push((seed_name.to_string(), tag_name.clone(), best_sim));
                    entry.4 += best_sim as f64;
                }
            }
        }
    }

    // 4. Normalize scores by seed tag count and sort
    let mut results: Vec<(
        i64,
        String,
        Option<String>,
        Option<f64>,
        Option<String>,
        f64,
        String,
    )> = Vec::new();

    for (fid, (title, artist, bpm, key, raw_score, matched_tags)) in file_scores {
        let normalized_score = raw_score / seed_tag_count as f64;
        // Serialize matched_tags to JSON
        let matched_json = serde_json::to_string(&matched_tags).unwrap_or_default();
        results.push((fid, title, artist, bpm, key, normalized_score, matched_json));
    }

    // Sort by score descending (higher = more similar)
    results.sort_by(|a, b| b.5.partial_cmp(&a.5).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit as usize);

    Ok(results)
}

/// Set the `source_of` relationship for a WAV source file to point to its parent stem.
pub async fn set_file_source_of(
    pool: &Pool<Sqlite>,
    file_id: i64,
    source_file_id: i64,
) -> Result<()> {
    sqlx::query("UPDATE files SET source_of = ? WHERE id = ?")
        .bind(source_file_id)
        .bind(file_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all files whose source_of points to a given file (i.e. WAVs for a stem)
pub async fn get_files_by_source(pool: &Pool<Sqlite>, source_file_id: i64) -> Result<Vec<File>> {
    let files = sqlx::query_as::<_, File>("SELECT * FROM files WHERE source_of = ?")
        .bind(source_file_id)
        .fetch_all(pool)
        .await?;
    Ok(files)
}
