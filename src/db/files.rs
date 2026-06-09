//! File-related database queries — scanning, metadata, comments, detail.

use std::collections::{HashMap, HashSet};
use std::{fs, path::Path, time::SystemTime};

use anyhow::{Result, anyhow};
use lofty::{prelude::*, read_from_path};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};
use tracing::{debug, info, warn};

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
    let mut on_disk_paths: Vec<String> = Vec::new();

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
                // Track this file as present on disk (for local presence tracking)
                on_disk_paths.push(path.to_string_lossy().to_string());

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
                        // File hasn't changed since last scan, skip metadata re-extraction
                        // but it's still on disk — file_locations.local is handled below
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

    // Batch-ensure file_locations.local for all audio files seen on disk.
    // This is critical for incremental scans where unchanged files are skipped
    // above — without this, isLocal returns false for files that ARE on disk.
    if !on_disk_paths.is_empty() {
        // Chunk to respect SQLite's max variable binding limit (~999)
        for chunk in on_disk_paths.chunks(900) {
            let placeholders: Vec<String> = chunk.iter().map(|_| "?".to_string()).collect();
            let sql = format!(
                "INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
                 SELECT f.id, 'local', f.file_path, f.file_size, unixepoch(), unixepoch()
                 FROM files f
                 WHERE f.file_path IN ({})
                   AND f.id NOT IN (SELECT file_id FROM file_locations WHERE location_type = 'local')",
                placeholders.join(",")
            );
            let mut query = sqlx::query(&sql);
            for path in chunk {
                query = query.bind(path);
            }
            if let Err(e) = query.execute(pool).await {
                warn!("Failed to batch-ensure file_locations.local: {}", e);
            }
        }
        debug!(
            "Ensured file_locations.local for {} on-disk paths",
            on_disk_paths.len()
        );
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
        // --remove-tag first: metaflac --set-tag APPENDS by default.
        // Without --remove-tag, old COMMENT tags accumulate on every write.
        let output = Command::new("metaflac")
            .arg("--remove-tag=COMMENT")
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

// ============================================================================
// Backpack Pull Candidates
// ============================================================================

/// Default format priorities: stem.m4a > flac > mp3 > wav > other
pub fn default_format_priorities() -> Vec<String> {
    vec![
        "stem.m4a".to_string(),
        "flac".to_string(),
        "mp3".to_string(),
        "wav".to_string(),
    ]
}

/// Rank a format according to a custom priority list.
/// Lower return value = better. Unknown formats get `u8::MAX`.
pub fn format_preference_with(file_type: &str, priorities: &[String]) -> u8 {
    priorities
        .iter()
        .position(|p| p == file_type)
        .map(|i| i as u8)
        .unwrap_or(u8::MAX)
}

/// Legacy wrapper: format preference using the hardcoded default order.
pub fn format_preference(file_type: &str) -> u8 {
    format_preference_with(file_type, &default_format_priorities())
}

/// Load format priorities from service_config (stored on the 'deemix' row's
/// metadata_json as a JSON string array). Falls back to default priorities.
pub async fn load_format_priorities(pool: &Pool<Sqlite>) -> Vec<String> {
    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT metadata_json FROM service_config WHERE service = 'deemix'"#)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    match row {
        Some((json_str,)) if !json_str.is_empty() => {
            match serde_json::from_str::<Vec<String>>(&json_str) {
                Ok(priorities) if !priorities.is_empty() => priorities,
                _ => default_format_priorities(),
            }
        }
        _ => default_format_priorities(),
    }
}

/// Find files that need to be pulled from backup to satisfy backpack tags.
///
/// For each track whose tags have `backpack = 1`:
/// 1. Find all file variants for the track (by ISRC)
/// 2. Pick the best available format: stem.m4a > flac > mp3 > other (WAVs excluded)
/// 3. If the best format is on backup but not local, mark it for pull
///
/// Returns candidates sorted by file type preference (best formats first).
pub async fn get_backpack_pull_candidates(
    pool: &Pool<sqlx::Sqlite>,
) -> Result<Vec<crate::db::PullCandidate>> {
    use crate::db::PullCandidate;

    // Load format priorities (could be user-configured)
    let priorities = load_format_priorities(pool).await;

    // Step 1: Get all file IDs that are in backpack tags (via file_resolved_tags)
    // A file is in a backpack tag if any of its resolved tags has backpack = 1.
    let backpack_file_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT frt.file_id
         FROM file_resolved_tags frt
         JOIN tags t ON t.id = frt.tag_id
         WHERE t.backpack = 1",
    )
    .fetch_all(pool)
    .await?;

    if backpack_file_ids.is_empty() {
        return Ok(Vec::new());
    }

    tracing::info!(
        "Backpack pull: {} backpack files, priorities={:?}",
        backpack_file_ids.len(),
        priorities
    );

    // Step 2: For each backpack file, find all variants sharing the same track_id
    // via v_file_track_link. Files not linked to any track get individual groups.
    let placeholders: Vec<String> = backpack_file_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT DISTINCT f.* FROM files f
         JOIN v_file_track_link v ON v.file_id = f.id
         WHERE v.track_id IN (
             SELECT DISTINCT v2.track_id FROM v_file_track_link v2
             WHERE v2.file_id IN ({})
         )
         UNION
         SELECT DISTINCT f.* FROM files f
         WHERE f.id IN ({})
           AND f.id NOT IN (SELECT file_id FROM v_file_track_link)
         ORDER BY file_type",
        placeholders.join(","),
        placeholders.join(",")
    );

    let mut query = sqlx::query_as::<_, File>(&sql);
    for id in &backpack_file_ids {
        query = query.bind(id);
    }
    // Second set of bindings for the UNION branch (same IDs)
    for id in &backpack_file_ids {
        query = query.bind(id);
    }

    let all_files: Vec<File> = query.fetch_all(pool).await?;

    // Step 3: Build file_id → track_id mapping from v_file_track_link
    let track_id_placeholders: Vec<String> = all_files.iter().map(|_| "?".to_string()).collect();
    let track_sql = format!(
        "SELECT file_id, track_id FROM v_file_track_link WHERE file_id IN ({})",
        track_id_placeholders.join(",")
    );
    let mut track_query = sqlx::query_as::<_, (i64, i64)>(&track_sql);
    for f in &all_files {
        track_query = track_query.bind(f.id);
    }
    let file_track_pairs: Vec<(i64, i64)> = track_query.fetch_all(pool).await?;
    let file_track_map: std::collections::HashMap<i64, i64> =
        file_track_pairs.into_iter().collect();

    // Group files by track_id. Unlinked files each get their own group (negative file_id key).
    let mut groups: std::collections::HashMap<i64, Vec<&File>> = std::collections::HashMap::new();
    for f in &all_files {
        let key = file_track_map.get(&f.id).copied().unwrap_or(-f.id);
        groups.entry(key).or_default().push(f);
    }

    // Step 4: For each ISRC group, find the best format and check if it needs pulling
    let mut candidates: Vec<PullCandidate> = Vec::new();

    // Fetch all file_locations for the relevant files in one query
    let loc_placeholders: Vec<String> = all_files.iter().map(|_| "?".to_string()).collect();
    let loc_sql = format!(
        "SELECT * FROM file_locations WHERE file_id IN ({})",
        loc_placeholders.join(",")
    );
    let mut loc_query = sqlx::query_as::<_, FileLocation>(&loc_sql);
    for f in &all_files {
        loc_query = loc_query.bind(f.id);
    }
    let all_locations: Vec<FileLocation> = loc_query.fetch_all(pool).await?;

    // Index locations by file_id
    let mut locs_by_file: std::collections::HashMap<i64, Vec<&FileLocation>> =
        std::collections::HashMap::new();
    for loc in &all_locations {
        locs_by_file.entry(loc.file_id).or_default().push(loc);
    }

    // Helper: check if a file is local
    let is_local = |file_id: i64| -> bool {
        locs_by_file
            .get(&file_id)
            .map(|locs| locs.iter().any(|l| l.location_type == "local"))
            .unwrap_or(false)
    };

    // Helper: get backup path for a file
    let get_backup_path = |file_id: i64| -> Option<String> {
        locs_by_file.get(&file_id).and_then(|locs| {
            locs.iter()
                .find(|l| l.location_type == "backup")
                .map(|l| l.path.clone())
        })
    };

    for (_track_id, group) in &groups {
        // Sort group by format preference (best first), excluding WAVs
        let mut sorted: Vec<&&File> = group
            .iter()
            .filter(|f| f.file_type != "wav") // skip WAV source files
            .collect();
        sorted.sort_by_key(|f| format_preference_with(&f.file_type, &priorities));

        if sorted.is_empty() {
            continue;
        }

        let best = sorted[0];

        // If the best format is already local, nothing to do
        if is_local(best.id) {
            continue;
        }

        // Check if the best format is on backup
        if let Some(backup_path) = get_backup_path(best.id) {
            candidates.push(PullCandidate {
                file_id: best.id,
                local_path: best.file_path.clone(),
                backup_path,
                file_type: best.file_type.clone(),
                file_size: best.file_size,
                title: best.title.clone().unwrap_or_default(),
                artist: best.artist.clone().unwrap_or_default(),
                isrc: best.isrc.clone(),
            });
        }
    }

    // Sort by format preference (best formats first)
    candidates.sort_by_key(|c| format_preference_with(&c.file_type, &priorities));

    tracing::info!(
        "Backpack pull: {} candidates from {} track groups",
        candidates.len(),
        groups.len()
    );

    Ok(candidates)
}

/// Compute size statistics for backpack-tagged files.
///
/// For each track whose tags have `backpack = 1`:
/// 1. Find all file variants for the track (by ISRC)
/// 2. Pick the best available format: stem.m4a > flac > mp3 > other (WAVs excluded)
/// 3. Count the best format's size as `target_bytes`, and if local, as `local_bytes`
///
/// `needs_pull_bytes = target_bytes - local_bytes` (how much needs to be pulled from backup).
pub async fn get_backpack_size_stats(pool: &Pool<Sqlite>) -> Result<BackpackSizeStats> {
    // Load format priorities (could be user-configured)
    let priorities = load_format_priorities(pool).await;

    // Step 1: Count backpack tags
    let tag_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE backpack = 1")
        .fetch_one(pool)
        .await?;

    if tag_count == 0 {
        return Ok(BackpackSizeStats {
            tag_count: 0,
            track_count: 0,
            local_bytes: 0,
            target_bytes: 0,
            needs_pull_bytes: 0,
        });
    }

    // Step 2: Get all file IDs that are in backpack tags (via file_resolved_tags)
    let backpack_file_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT frt.file_id
         FROM file_resolved_tags frt
         JOIN tags t ON t.id = frt.tag_id
         WHERE t.backpack = 1",
    )
    .fetch_all(pool)
    .await?;

    if backpack_file_ids.is_empty() {
        return Ok(BackpackSizeStats {
            tag_count,
            track_count: 0,
            local_bytes: 0,
            target_bytes: 0,
            needs_pull_bytes: 0,
        });
    }

    // Step 3: Count distinct track_ids
    let placeholders: Vec<String> = backpack_file_ids.iter().map(|_| "?".to_string()).collect();
    let track_count_sql = format!(
        "SELECT COUNT(DISTINCT v.track_id) FROM v_file_track_link v WHERE v.file_id IN ({})",
        placeholders.join(",")
    );
    let mut track_count_query = sqlx::query_scalar::<_, i64>(&track_count_sql);
    for id in &backpack_file_ids {
        track_count_query = track_count_query.bind(id);
    }
    let track_count: i64 = track_count_query.fetch_one(pool).await?;

    // Step 4: Fetch all files linked to the same tracks as backpack files
    let sql = format!(
        "SELECT DISTINCT f.* FROM files f
         JOIN v_file_track_link v ON v.file_id = f.id
         WHERE v.track_id IN (
             SELECT DISTINCT v2.track_id FROM v_file_track_link v2
             WHERE v2.file_id IN ({})
         )
         UNION
         SELECT DISTINCT f.* FROM files f
         WHERE f.id IN ({})
           AND f.id NOT IN (SELECT file_id FROM v_file_track_link)
         ORDER BY file_type",
        placeholders.join(","),
        placeholders.join(",")
    );

    let mut query = sqlx::query_as::<_, File>(&sql);
    for id in &backpack_file_ids {
        query = query.bind(id);
    }
    for id in &backpack_file_ids {
        query = query.bind(id);
    }

    let all_files: Vec<File> = query.fetch_all(pool).await?;

    // Step 5: Build file_id → track_id mapping from v_file_track_link
    let track_placeholders: Vec<String> = all_files.iter().map(|_| "?".to_string()).collect();
    let track_sql = format!(
        "SELECT file_id, track_id FROM v_file_track_link WHERE file_id IN ({})",
        track_placeholders.join(",")
    );
    let mut track_query = sqlx::query_as::<_, (i64, i64)>(&track_sql);
    for f in &all_files {
        track_query = track_query.bind(f.id);
    }
    let file_track_pairs: Vec<(i64, i64)> = track_query.fetch_all(pool).await?;
    let file_track_map: HashMap<i64, i64> = file_track_pairs.into_iter().collect();

    // Group files by track_id
    let mut groups: HashMap<i64, Vec<&File>> = HashMap::new();
    for f in &all_files {
        let key = file_track_map.get(&f.id).copied().unwrap_or(-f.id);
        groups.entry(key).or_default().push(f);
    }

    // Step 6: Fetch all file_locations for the relevant files
    let loc_placeholders: Vec<String> = all_files.iter().map(|_| "?".to_string()).collect();
    let loc_sql = format!(
        "SELECT * FROM file_locations WHERE file_id IN ({})",
        loc_placeholders.join(",")
    );
    let mut loc_query = sqlx::query_as::<_, FileLocation>(&loc_sql);
    for f in &all_files {
        loc_query = loc_query.bind(f.id);
    }
    let all_locations: Vec<FileLocation> = loc_query.fetch_all(pool).await?;

    let mut locs_by_file: HashMap<i64, Vec<&FileLocation>> = HashMap::new();
    for loc in &all_locations {
        locs_by_file.entry(loc.file_id).or_default().push(loc);
    }

    let is_local = |file_id: i64| -> bool {
        locs_by_file
            .get(&file_id)
            .map(|locs| locs.iter().any(|l| l.location_type == "local"))
            .unwrap_or(false)
    };

    // Step 7: For each track group, pick best format and accumulate sizes
    let mut target_bytes: i64 = 0;
    let mut local_bytes: i64 = 0;

    for (_track_id, group) in &groups {
        let mut sorted: Vec<&&File> = group
            .iter()
            .filter(|f| f.file_type != "wav") // skip WAV source files
            .collect();
        sorted.sort_by_key(|f| format_preference_with(&f.file_type, &priorities));

        if sorted.is_empty() {
            continue;
        }

        let best = sorted[0];

        target_bytes += best.file_size;
        if is_local(best.id) {
            local_bytes += best.file_size;
        }
    }

    let needs_pull_bytes = (target_bytes - local_bytes).max(0);

    Ok(BackpackSizeStats {
        tag_count,
        track_count,
        local_bytes,
        target_bytes,
        needs_pull_bytes,
    })
}

/// Resolve the SSH host from a file's backup path by matching against folder configs.
///
/// The backup_path from `PullCandidate` is like `/volume1/media/stems/file.stem.m4a`
/// (no `host:` prefix). We match it against each folder's `backup_path`, which is
/// like `backup:/volume1/media/stems`, to extract `("backup", "/volume1/media/stems/file.stem.m4a")`.
pub async fn resolve_backup_host(
    pool: &Pool<Sqlite>,
    backup_path: &str,
) -> Result<(String, String)> {
    // Get all folders with backup_path configured
    let folders = sqlx::query_as::<_, crate::db::Folder>(
        "SELECT * FROM folders WHERE backup_path IS NOT NULL AND backup_path != ''",
    )
    .fetch_all(pool)
    .await?;

    for folder in &folders {
        if let Some(ref bp) = folder.backup_path {
            // folder.backup_path is like "backup:/volume1/media/stems"
            if let Some((host, folder_remote_prefix)) = bp.split_once(':') {
                // Check if backup_path starts with the folder's remote prefix
                if backup_path.starts_with(folder_remote_prefix) {
                    return Ok((host.to_string(), backup_path.to_string()));
                }
            }
        }
    }

    Err(anyhow!(
        "No matching folder found for backup path: {}",
        backup_path
    ))
}

/// After backpack pull completes, delete redundant local files within each ISRC group.
///
/// For each ISRC group with backpack-tagged files:
/// - Keep the BEST format (by configured priorities) that is local
/// - Delete all other local files in the group — ONLY if they're backed up
/// - Never delete a file that isn't backed up
/// - Skip WAV source files
///
/// Returns (deleted_count, freed_bytes).
pub async fn cleanup_redundant_backpack_files(pool: &Pool<Sqlite>) -> Result<(usize, i64)> {
    let priorities = load_format_priorities(pool).await;

    // Step 1: Get all file IDs that are in backpack tags
    let backpack_file_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT frt.file_id
         FROM file_resolved_tags frt
         JOIN tags t ON t.id = frt.tag_id
         WHERE t.backpack = 1",
    )
    .fetch_all(pool)
    .await?;

    if backpack_file_ids.is_empty() {
        return Ok((0, 0));
    }

    // Step 2: Find all track-mates of backpack files via v_file_track_link (same query as get_backpack_pull_candidates)
    let placeholders: Vec<String> = backpack_file_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT DISTINCT f.* FROM files f
         JOIN v_file_track_link v ON v.file_id = f.id
         WHERE v.track_id IN (
             SELECT DISTINCT v2.track_id FROM v_file_track_link v2
             WHERE v2.file_id IN ({})
         )
         UNION
         SELECT DISTINCT f.* FROM files f
         WHERE f.id IN ({})
           AND f.id NOT IN (SELECT file_id FROM v_file_track_link)
         ORDER BY file_type",
        placeholders.join(","),
        placeholders.join(",")
    );

    let mut query = sqlx::query_as::<_, File>(&sql);
    for id in &backpack_file_ids {
        query = query.bind(id);
    }
    for id in &backpack_file_ids {
        query = query.bind(id);
    }

    let all_files: Vec<File> = query.fetch_all(pool).await?;

    // Step 3: Build file_id → track_id mapping from v_file_track_link, then group by track_id
    let track_id_placeholders: Vec<String> = all_files.iter().map(|_| "?".to_string()).collect();
    let track_sql = format!(
        "SELECT file_id, track_id FROM v_file_track_link WHERE file_id IN ({})",
        track_id_placeholders.join(",")
    );
    let mut track_query = sqlx::query_as::<_, (i64, i64)>(&track_sql);
    for f in &all_files {
        track_query = track_query.bind(f.id);
    }
    let file_track_pairs: Vec<(i64, i64)> = track_query.fetch_all(pool).await?;
    let file_track_map: std::collections::HashMap<i64, i64> =
        file_track_pairs.into_iter().collect();

    // Group files by track_id. Unlinked files each get their own group (negative file_id key).
    let mut groups: std::collections::HashMap<i64, Vec<&File>> = std::collections::HashMap::new();
    for f in &all_files {
        let key = file_track_map.get(&f.id).copied().unwrap_or(-f.id);
        groups.entry(key).or_default().push(f);
    }

    // Step 4: Fetch all file_locations for all files at once
    let loc_placeholders: Vec<String> = all_files.iter().map(|_| "?".to_string()).collect();
    let loc_sql = format!(
        "SELECT * FROM file_locations WHERE file_id IN ({})",
        loc_placeholders.join(",")
    );
    let mut loc_query = sqlx::query_as::<_, FileLocation>(&loc_sql);
    for f in &all_files {
        loc_query = loc_query.bind(f.id);
    }
    let all_locations: Vec<FileLocation> = loc_query.fetch_all(pool).await?;

    // Index locations by file_id
    let mut locs_by_file: std::collections::HashMap<i64, Vec<&FileLocation>> =
        std::collections::HashMap::new();
    for loc in &all_locations {
        locs_by_file.entry(loc.file_id).or_default().push(loc);
    }

    let is_local = |file_id: i64| -> bool {
        locs_by_file
            .get(&file_id)
            .map(|locs| locs.iter().any(|l| l.location_type == "local"))
            .unwrap_or(false)
    };

    let is_backed_up = |file_id: i64| -> bool {
        locs_by_file
            .get(&file_id)
            .map(|locs| locs.iter().any(|l| l.location_type == "backup"))
            .unwrap_or(false)
    };

    let mut deleted = 0usize;
    let mut freed_bytes: i64 = 0;

    for (_key, group) in &groups {
        // Sort group by format preference (best first), excluding WAVs
        let mut sorted: Vec<&&File> = group
            .iter()
            .filter(|f| f.file_type != "wav") // skip WAV source files
            .collect();
        sorted.sort_by_key(|f| format_preference_with(&f.file_type, &priorities));

        if sorted.len() <= 1 {
            // Need at least 2 files in the group to have a redundant one
            continue;
        }

        // Find the best format that is local → this is the "keeper"
        let keeper = sorted.iter().find(|f| is_local(f.id));

        let Some(keeper) = keeper else {
            // No local file at all — nothing to clean up yet (pull must happen first)
            continue;
        };

        // For all OTHER local files in the group, check if they're redundant
        for file in sorted
            .iter()
            .filter(|f| f.id != keeper.id) // not the keeper
            .filter(|f| is_local(f.id)) // is local
            .filter(|f| is_backed_up(f.id)) // is backed up (safety check)
            .filter(|f| f.file_type != keeper.file_type)
        // different format than keeper
        {
            // Delete from filesystem
            let path_ref = std::path::Path::new(&file.file_path);
            if path_ref.exists() {
                match tokio::fs::remove_file(path_ref).await {
                    Ok(()) => {
                        tracing::info!(
                            "Backpack cleanup: deleted redundant local {} ({})",
                            file.file_path,
                            file.file_type
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to delete redundant local file {}: {}",
                            file.file_path,
                            e
                        );
                        continue;
                    }
                }
            }

            // Remove local location record
            let _ = crate::db::remove_file_location(pool, file.id, "local").await;

            freed_bytes += file.file_size;
            deleted += 1;
        }
    }

    Ok((deleted, freed_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Create the `files` table with all columns matching the current schema.
    /// Also creates `file_locations` and `file_resolved_tags` for dependent tests.
    async fn create_files_table(pool: &SqlitePool) {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                file_hash TEXT NOT NULL,
                file_type TEXT NOT NULL CHECK (file_type IN ('flac', 'mp3', 'stem.m4a', 'wav', 'opus')),
                file_size INTEGER NOT NULL,
                last_modified INTEGER NOT NULL,
                isrc TEXT,
                last_scanned INTEGER DEFAULT (unixepoch()),
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
                rating INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played INTEGER,
                spotify_id TEXT,
                soundcloud_id TEXT,
                youtube_id TEXT,
                source_of INTEGER REFERENCES files(id),
                stem_type TEXT,
                last_verified_local INTEGER,
                created_at INTEGER DEFAULT (unixepoch()),
                updated_at INTEGER DEFAULT (unixepoch())
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn create_file_locations_table(pool: &SqlitePool) {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS file_locations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                location_type TEXT NOT NULL CHECK (location_type IN ('local', 'backup')),
                path TEXT NOT NULL,
                file_size INTEGER,
                last_verified INTEGER,
                created_at INTEGER DEFAULT (unixepoch()),
                UNIQUE(file_id, location_type)
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn create_file_resolved_tags_table(pool: &SqlitePool) {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS file_resolved_tags (
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL,
                tag_name TEXT NOT NULL,
                category_id INTEGER NOT NULL,
                category_name TEXT NOT NULL,
                prefix TEXT NOT NULL,
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER,
                is_default BOOLEAN DEFAULT FALSE,
                PRIMARY KEY (file_id, tag_id)
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// Insert a minimal file row for testing. Returns the generated ID.
    async fn insert_test_file(
        pool: &SqlitePool,
        file_path: &str,
        file_type: &str,
        overrides: &[(&str, &str)],
    ) -> i64 {
        let mut fields = vec![
            "file_path".to_string(),
            "file_hash".to_string(),
            "file_type".to_string(),
            "file_size".to_string(),
            "last_modified".to_string(),
            "rating".to_string(),
            "play_count".to_string(),
        ];
        let mut values: Vec<String> = vec![
            format!("'{}'", file_path.replace('\'', "''")),
            format!("'hash-{}'", file_path.replace('\'', "''")),
            format!("'{}'", file_type),
            "12345".to_string(),
            "1000000".to_string(),
            "0".to_string(),
            "0".to_string(),
        ];

        for (key, val) in overrides {
            fields.push(key.to_string());
            values.push(if *val == "NULL" {
                "NULL".to_string()
            } else {
                format!("'{}'", val.replace('\'', "''"))
            });
        }

        let sql = format!(
            "INSERT INTO files ({}) VALUES ({}) RETURNING id",
            fields.join(", "),
            values.join(", ")
        );

        sqlx::query_scalar::<_, i64>(&sql)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // ========================================================================
    // Pure Function Tests
    // ========================================================================

    #[test]
    fn test_file_type_from_path_all_types() {
        assert_eq!(file_type_from_path(Path::new("song.flac")), Some("flac"));
        assert_eq!(file_type_from_path(Path::new("song.mp3")), Some("mp3"));
        assert_eq!(file_type_from_path(Path::new("song.m4a")), Some("stem.m4a"));
        assert_eq!(file_type_from_path(Path::new("song.wav")), Some("wav"));
        assert_eq!(file_type_from_path(Path::new("song.opus")), Some("opus"));
    }

    #[test]
    fn test_file_type_from_path_unknown_and_edges() {
        assert_eq!(file_type_from_path(Path::new("song.aiff")), None);
        assert_eq!(file_type_from_path(Path::new("song")), None); // no extension
        assert_eq!(file_type_from_path(Path::new("")), None);
        // Uppercase
        assert_eq!(file_type_from_path(Path::new("song.FLAC")), Some("flac"));
        assert_eq!(file_type_from_path(Path::new("song.M4A")), Some("stem.m4a"));
        // Hidden file with extension
        assert_eq!(file_type_from_path(Path::new(".config.flac")), Some("flac"));
    }

    #[test]
    fn test_parse_year_normal_and_edge() {
        assert_eq!(parse_year("2024"), Some(2024));
        assert_eq!(parse_year("2024-01-15"), Some(2024)); // ISO date with parts
        assert_eq!(parse_year("1999-12-31"), Some(1999));
        assert_eq!(parse_year(""), None); // empty
        assert_eq!(parse_year("not-a-year"), None); // non-numeric
        assert_eq!(parse_year("-"), None); // just dash
    }

    #[test]
    fn test_parse_track_number_and_total() {
        assert_eq!(parse_track_number("3/12"), Some(3));
        assert_eq!(parse_track_number("1/10"), Some(1));
        assert_eq!(parse_track_number("5"), Some(5)); // no total
        assert_eq!(parse_track_number(""), None);
        assert_eq!(parse_track_number("abc/12"), None);

        assert_eq!(parse_total_tracks("3/12"), Some(12));
        assert_eq!(parse_total_tracks("5"), None); // no total part
        assert_eq!(parse_total_tracks("/10"), Some(10));
        assert_eq!(parse_total_tracks(""), None);
    }

    #[test]
    fn test_parse_disc_number_and_total() {
        assert_eq!(parse_disc_number("1/2"), Some(1));
        assert_eq!(parse_disc_number("2/3"), Some(2));
        assert_eq!(parse_disc_number("1"), Some(1));
        assert_eq!(parse_disc_number(""), None);

        assert_eq!(parse_total_discs("1/2"), Some(2));
        assert_eq!(parse_total_discs("1"), None);
        assert_eq!(parse_total_discs("/3"), Some(3));
        assert_eq!(parse_total_discs(""), None);
    }

    #[test]
    fn test_parse_bpm_variants() {
        assert_eq!(parse_bpm("128"), Some(128.0));
        assert_eq!(parse_bpm("128.5"), Some(128.5));
        assert_eq!(parse_bpm("140.00"), Some(140.0));
        assert_eq!(parse_bpm(""), None);
        assert_eq!(parse_bpm("abc"), None);
        assert_eq!(parse_bpm("128.5.5"), None);
    }

    #[test]
    fn test_format_preference_ordering() {
        // Lower = better. Stem is best.
        assert!(format_preference("stem.m4a") < format_preference("flac"));
        assert!(format_preference("flac") < format_preference("mp3"));
        assert!(format_preference("mp3") < format_preference("wav"));
        assert!(format_preference("wav") < format_preference("opus"));
        assert!(format_preference("wav") < format_preference("unknown"));
    }

    #[test]
    fn test_format_preference_exact_values() {
        assert_eq!(format_preference("stem.m4a"), 0);
        assert_eq!(format_preference("flac"), 1);
        assert_eq!(format_preference("mp3"), 2);
        assert_eq!(format_preference("wav"), 3);
        assert_eq!(format_preference("opus"), u8::MAX);
        assert_eq!(format_preference("aiff"), u8::MAX);
    }

    #[test]
    fn test_format_preference_with_config() {
        let prio = vec![
            "mp3".to_string(),
            "flac".to_string(),
            "stem.m4a".to_string(),
        ];
        assert!(
            format_preference_with("mp3", &prio) < format_preference_with("flac", &prio),
            "mp3 should rank higher than flac"
        );
        assert!(
            format_preference_with("flac", &prio) < format_preference_with("stem.m4a", &prio),
            "flac should rank higher than stem.m4a"
        );
        assert_eq!(
            format_preference_with("wav", &prio),
            u8::MAX,
            "unknown format should return MAX"
        );
    }

    #[test]
    fn test_default_priorities() {
        let defaults = default_format_priorities();
        assert_eq!(defaults[0], "stem.m4a");
        assert_eq!(defaults[1], "flac");
        assert_eq!(defaults[2], "mp3");
        assert_eq!(defaults[3], "wav");
    }

    // ========================================================================
    // DB Function Tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_files_empty() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let files = get_files(&pool).await.unwrap();
        assert!(files.is_empty(), "expected empty files list");
    }

    #[tokio::test]
    async fn test_get_files_populated() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let id1 = insert_test_file(&pool, "/music/song1.flac", "flac", &[]).await;
        let id2 = insert_test_file(&pool, "/music/song2.mp3", "mp3", &[]).await;

        let files = get_files(&pool).await.unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].file_path, "/music/song1.flac");
        assert_eq!(files[1].file_path, "/music/song2.mp3");
    }

    #[tokio::test]
    async fn test_get_file_by_id_found() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let id = insert_test_file(&pool, "/music/test.flac", "flac", &[]).await;

        let file = get_file_by_id(&pool, id).await.unwrap();
        assert!(file.is_some());
        assert_eq!(file.unwrap().file_path, "/music/test.flac");
    }

    #[tokio::test]
    async fn test_get_file_by_id_not_found() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let file = get_file_by_id(&pool, 999).await.unwrap();
        assert!(file.is_none());
    }

    #[tokio::test]
    async fn test_get_file_by_path_found() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        insert_test_file(&pool, "/music/test.flac", "flac", &[]).await;

        let file = get_file_by_path(&pool, "/music/test.flac").await.unwrap();
        assert!(file.is_some());
        assert_eq!(file.unwrap().file_type, "flac");
    }

    #[tokio::test]
    async fn test_get_file_by_path_not_found() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let file = get_file_by_path(&pool, "/nonexistent.flac").await.unwrap();
        assert!(file.is_none());
    }

    #[tokio::test]
    async fn test_update_file_comment() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let id = insert_test_file(&pool, "/music/test.flac", "flac", &[]).await;

        update_file_comment(&pool, id, "[PMV] groovy techno")
            .await
            .unwrap();

        let file = get_file_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(file.comment, Some("[PMV] groovy techno".to_string()));
    }

    #[tokio::test]
    async fn test_update_file_comment_overwrites() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/test.flac",
            "flac",
            &[("comment", "old comment")],
        )
        .await;

        update_file_comment(&pool, id, "new comment").await.unwrap();

        let file = get_file_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(file.comment, Some("new comment".to_string()));
    }

    #[tokio::test]
    async fn test_set_file_source_of() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let stem_id = insert_test_file(&pool, "/music/track.stem.m4a", "stem.m4a", &[]).await;
        let wav_id = insert_test_file(&pool, "/music/track_vocals.wav", "wav", &[]).await;

        set_file_source_of(&pool, wav_id, stem_id).await.unwrap();

        let wav = get_file_by_id(&pool, wav_id).await.unwrap().unwrap();
        assert_eq!(wav.source_of, Some(stem_id));
    }

    #[tokio::test]
    async fn test_get_files_by_source() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let stem_id = insert_test_file(&pool, "/music/track.stem.m4a", "stem.m4a", &[]).await;
        let wav1 = insert_test_file(&pool, "/music/track_vocals.wav", "wav", &[]).await;
        let wav2 = insert_test_file(&pool, "/music/track_bass.wav", "wav", &[]).await;

        set_file_source_of(&pool, wav1, stem_id).await.unwrap();
        set_file_source_of(&pool, wav2, stem_id).await.unwrap();

        // Also add an unrelated WAV (should not appear)
        let _other_id = insert_test_file(&pool, "/music/other.wav", "wav", &[]).await;

        let sources = get_files_by_source(&pool, stem_id).await.unwrap();
        assert_eq!(sources.len(), 2);
        assert!(
            sources
                .iter()
                .any(|f| f.file_path == "/music/track_vocals.wav")
        );
        assert!(
            sources
                .iter()
                .any(|f| f.file_path == "/music/track_bass.wav")
        );
    }

    #[tokio::test]
    async fn test_get_files_by_source_empty() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let sources = get_files_by_source(&pool, 999).await.unwrap();
        assert!(sources.is_empty());
    }

    #[tokio::test]
    async fn test_get_file_variants_not_found() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let result = get_file_variants(&pool, 999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_file_variants_single() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        let result = get_file_variants(&pool, id).await.unwrap();
        assert!(result.is_some());
        let variants = result.unwrap();
        assert_eq!(variants.file_id, id);
        assert_eq!(variants.variants.len(), 1);
        assert_eq!(variants.variants[0].file_type, "flac");
    }

    #[tokio::test]
    async fn test_get_file_variants_same_isrc() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let id1 = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[("isrc", "USABC1234567")],
        )
        .await;
        let id2 = insert_test_file(
            &pool,
            "/music/track.stem.m4a",
            "stem.m4a",
            &[("isrc", "USABC1234567")],
        )
        .await;

        let result = get_file_variants(&pool, id1).await.unwrap();
        assert!(result.is_some());
        let variants = result.unwrap();
        // Should find both variants (same ISRC)
        assert_eq!(variants.variants.len(), 2);
        assert_eq!(variants.isrc, Some("USABC1234567".to_string()));
    }

    #[tokio::test]
    async fn test_get_file_variants_wav_sources() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let stem_id = insert_test_file(
            &pool,
            "/music/track.stem.m4a",
            "stem.m4a",
            &[("isrc", "USABC1234567")],
        )
        .await;
        let _wav_vocals = insert_test_file(
            &pool,
            "/music/track_vocals.wav",
            "wav",
            &[
                ("source_of", &format!("{}", stem_id)),
                ("stem_type", "vocals"),
            ],
        )
        .await;
        let _wav_bass = insert_test_file(
            &pool,
            "/music/track_bass.wav",
            "wav",
            &[
                ("source_of", &format!("{}", stem_id)),
                ("stem_type", "bass"),
            ],
        )
        .await;

        // Querying the stem should find the WAV sources via source_of
        let result = get_file_variants(&pool, stem_id).await.unwrap();
        assert!(result.is_some());
        let variants = result.unwrap();
        // Should include stem + 2 WAV variants
        assert_eq!(
            variants.variants.len(),
            3,
            "expected stem + 2 WAVs, got {:?}",
            variants
                .variants
                .iter()
                .map(|v| &v.file_type)
                .collect::<Vec<_>>()
        );
        let wavs: Vec<_> = variants
            .variants
            .iter()
            .filter(|v| v.file_type == "wav")
            .collect();
        assert_eq!(wavs.len(), 2);
    }

    #[tokio::test]
    async fn test_get_file_variants_wav_with_source() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let stem_id = insert_test_file(&pool, "/music/track.stem.m4a", "stem.m4a", &[]).await;
        let wav_id = insert_test_file(
            &pool,
            "/music/track_vocals.wav",
            "wav",
            &[("source_of", &format!("{}", stem_id))],
        )
        .await;
        let wav_sibling = insert_test_file(
            &pool,
            "/music/track_bass.wav",
            "wav",
            &[("source_of", &format!("{}", stem_id))],
        )
        .await;

        // Querying the WAV file should include the stem + sibling WAVs
        let result = get_file_variants(&pool, wav_id).await.unwrap();
        assert!(result.is_some());
        let variants = result.unwrap();
        assert_eq!(
            variants.variants.len(),
            3,
            "expected stem + 2 WAVs (self + sibling), got {}",
            variants.variants.len()
        );
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_empty() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_file_resolved_tags_table(&pool).await;

        let result = get_file_resolved_tags_batch(&pool, &[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_single() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'groovy', 1, 'Mood', 'M', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let result = get_file_resolved_tags_batch(&pool, &[id]).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, id);
        assert_eq!(result[0].1, "groovy");
        assert_eq!(result[0].2, "M");
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_multiple() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id1 = insert_test_file(&pool, "/music/track1.flac", "flac", &[]).await;
        let id2 = insert_test_file(&pool, "/music/track2.flac", "flac", &[]).await;

        // Tag for file 1
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 10, 'deep', 3, 'Vibe', 'V', 1, 1000000)",
        )
        .bind(id1)
        .execute(&pool)
        .await
        .unwrap();

        // Tag for file 2
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 11, 'dark', 3, 'Vibe', 'V', 1, 1000000)",
        )
        .bind(id2)
        .execute(&pool)
        .await
        .unwrap();

        let result = get_file_resolved_tags_batch(&pool, &[id1, id2])
            .await
            .unwrap();
        assert_eq!(result.len(), 2);

        let tags_for_1: Vec<_> = result.iter().filter(|r| r.0 == id1).collect();
        let tags_for_2: Vec<_> = result.iter().filter(|r| r.0 == id2).collect();
        assert_eq!(tags_for_1.len(), 1);
        assert_eq!(tags_for_2.len(), 1);
        assert_eq!(tags_for_1[0].1, "deep");
        assert_eq!(tags_for_2[0].1, "dark");
    }

    #[tokio::test]
    async fn test_get_tags_for_file() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 5, 'techno', 2, 'Setlist', 'S', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 6, 'warehouse', 3, 'Vibe', 'V', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let tags = get_tags_for_file(&pool, id).await.unwrap();
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().any(|t| t.name == "techno"));
        assert!(tags.iter().any(|t| t.name == "warehouse"));
        // Tags should be ordered by name
        assert_eq!(tags[0].name, "techno");
        assert_eq!(tags[1].name, "warehouse");
    }

    #[tokio::test]
    async fn test_get_tags_for_file_empty() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        let tags = get_tags_for_file(&pool, id).await.unwrap();
        assert!(tags.is_empty());
    }

    // ========================================================================
    // Pure Function Edge Cases
    // ========================================================================

    #[test]
    fn test_parse_year_leading_zeros() {
        assert_eq!(parse_year("002024"), Some(2024));
        assert_eq!(parse_year("0000"), Some(0));
        assert_eq!(parse_year("0999"), Some(999));
    }

    #[test]
    fn test_parse_year_negative_and_invalid() {
        assert_eq!(parse_year("-2024"), None); // negative year
        assert_eq!(parse_year("not-a-year"), None);
        assert_eq!(parse_year("  2024  "), None); // whitespace not trimmed
        assert_eq!(parse_year("2024-13-01"), Some(2024)); // valid month range not checked
    }

    #[test]
    fn test_parse_track_number_leading_zeros() {
        assert_eq!(parse_track_number("03/12"), Some(3));
        assert_eq!(parse_track_number("007/100"), Some(7));
        assert_eq!(parse_track_number("0/5"), Some(0));
    }

    #[test]
    fn test_parse_track_number_negative_and_invalid() {
        // parse accepts negative numbers (valid i32)
        assert_eq!(parse_track_number("-1/12"), Some(-1));
        assert_eq!(parse_track_number("1/-2"), Some(1)); // only first part matters
        assert_eq!(parse_track_number("abc"), None);
        assert_eq!(parse_track_number("1.5/10"), None); // non-integer
    }

    #[test]
    fn test_parse_bpm_zero_and_extreme() {
        assert_eq!(parse_bpm("0"), Some(0.0));
        assert_eq!(parse_bpm("999.9"), Some(999.9));
        assert_eq!(parse_bpm("0.5"), Some(0.5));
        assert_eq!(parse_bpm("00.00"), Some(0.0));
    }

    #[test]
    fn test_parse_bpm_precision_and_truncation() {
        let bpm = parse_bpm("128.123456789").unwrap();
        // BPM is an f64, so just check it's approximately correct
        assert!((bpm - 128.123456789).abs() < 1e-9);
        assert_eq!(parse_bpm("140.00"), Some(140.0));
        // parse accepts negative BPM strings (valid f64), even if metadata doesn't have them
        assert_eq!(parse_bpm("-5.0"), Some(-5.0));
    }

    #[test]
    fn test_parse_disc_number_edge_cases() {
        assert_eq!(parse_disc_number("0/2"), Some(0));
        assert_eq!(parse_disc_number("2/0"), Some(2)); // total 0 is weird but parsed
        assert_eq!(parse_disc_number("abc"), None);
        // parse accepts negative numbers (valid i32)
        assert_eq!(parse_disc_number("-1/2"), Some(-1));
        assert_eq!(parse_total_discs("1/0"), Some(0)); // zero total discs
        assert_eq!(parse_total_discs("/"), None); // empty parts
    }

    // ========================================================================
    // compute_target_comment Tests
    // ========================================================================

    #[tokio::test]
    async fn test_compute_target_comment_no_tags_no_source() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[
                ("spotify_id", "NULL"),
                ("soundcloud_id", "NULL"),
                ("youtube_id", "NULL"),
            ],
        )
        .await;

        let comment = compute_target_comment(&pool, id).await.unwrap();
        // No PMV tags, no service IDs → empty string
        assert_eq!(
            comment, "",
            "expected empty comment for file with no tags and no service IDs"
        );
    }

    #[tokio::test]
    async fn test_compute_target_comment_no_tags_with_source() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[("spotify_id", "spotify:track:abc123")],
        )
        .await;

        let comment = compute_target_comment(&pool, id).await.unwrap();
        // No PMV tags → [___], spotify ID appended
        assert!(comment.contains("[___]"), "should have empty PMV");
        assert!(
            comment.contains("sp:spotify:track:abc123"),
            "should contain spotify ID"
        );
    }

    #[tokio::test]
    async fn test_compute_target_comment_all_pmv() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[("spotify_id", "spotify:track:abc123")],
        )
        .await;

        // Add tags for all three PMV categories
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'build', 2, 'Phase', 'P', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 2, 'groovy', 3, 'Mood', 'M', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 3, 'dark', 4, 'Vibe', 'V', 2, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let comment = compute_target_comment(&pool, id).await.unwrap();
        assert!(comment.contains("[PMV]"), "should have all three PMV chars");
        assert!(comment.contains("build"), "should contain Phase tag");
        assert!(comment.contains("groovy"), "should contain Mood tag");
        assert!(comment.contains("dark"), "should contain Vibe tag");
        assert!(
            comment.contains("sp:spotify:track:abc123"),
            "should contain spotify ID"
        );
        // Tags should be sorted alphabetically
        let build_pos = comment.find("build").unwrap();
        let dark_pos = comment.find("dark").unwrap();
        let groovy_pos = comment.find("groovy").unwrap();
        assert!(build_pos < dark_pos, "build should come before dark");
        assert!(dark_pos < groovy_pos, "dark should come before groovy");
    }

    #[tokio::test]
    async fn test_compute_target_comment_partial_pmv() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        // Only add a Mood tag
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 2, 'groovy', 3, 'Mood', 'M', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Also add a Setlist tag (category S, not PMV)
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 5, 'some-playlist', 1, 'Setlist', 'S', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let comment = compute_target_comment(&pool, id).await.unwrap();
        // Only Mood present → [_M_]
        assert!(comment.contains("[_M_]"), "should have only M in PMV");
        assert!(comment.contains("groovy"), "should contain Mood tag");
        assert!(
            comment.contains("some-playlist"),
            "should contain Setlist tag"
        );
        assert!(!comment.contains("P"), "should NOT have Phase");
        assert!(!comment.contains("V"), "should NOT have Vibe");
    }

    #[tokio::test]
    async fn test_compute_target_comment_setlist_only() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[("spotify_id", "spotify:track:xyz")],
        )
        .await;

        // Only Setlist tag (category 'S', not P/M/V)
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 10, 'my-playlist', 1, 'Setlist', 'S', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let comment = compute_target_comment(&pool, id).await.unwrap();
        // No PMV tags → [___], but Setlist tags still included
        assert!(comment.contains("[___]"), "should have empty PMV");
        assert!(
            comment.contains("my-playlist"),
            "should contain Setlist tag"
        );
        assert!(
            comment.contains("sp:spotify:track:xyz"),
            "should contain spotify ID"
        );
    }

    #[tokio::test]
    async fn test_compute_target_comment_multiple_source_ids() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[
                ("spotify_id", "spotify:track:s1"),
                ("soundcloud_id", "sc:12345"),
                ("youtube_id", "yt:abcdef"),
            ],
        )
        .await;

        // Add a Mood tag to give non-empty PMV
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 2, 'groovy', 3, 'Mood', 'M', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let comment = compute_target_comment(&pool, id).await.unwrap();
        assert!(comment.contains("[_M_]"), "should have only M");
        assert!(comment.contains("groovy"), "should contain tag");
        assert!(
            comment.contains("sp:spotify:track:s1"),
            "should have spotify ID"
        );
        assert!(comment.contains("sc:sc:12345"), "should have soundcloud ID");
        assert!(comment.contains("yt:yt:abcdef"), "should have youtube ID");
    }

    // ========================================================================
    // compute_target_comments_batch Tests
    // ========================================================================

    #[tokio::test]
    async fn test_compute_target_comments_batch_empty_input() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let results = compute_target_comments_batch(&pool, &[]).await.unwrap();
        assert!(results.is_empty(), "expected empty map for empty input");
    }

    #[tokio::test]
    async fn test_compute_target_comments_batch_nonexistent_ids() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let results = compute_target_comments_batch(&pool, &[999, 1000])
            .await
            .unwrap();
        assert!(results.is_empty(), "expected empty map for nonexistent IDs");
    }

    #[tokio::test]
    async fn test_compute_target_comments_batch_single_file() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[("spotify_id", "spotify:track:abc123")],
        )
        .await;

        // Add a Mood tag
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'deep', 3, 'Mood', 'M', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let results = compute_target_comments_batch(&pool, &[id]).await.unwrap();
        assert_eq!(results.len(), 1, "expected 1 result");
        let comment = results.get(&id).unwrap();
        assert!(comment.contains("[_M_]"), "should have M mood");
        assert!(comment.contains("deep"), "should contain tag");
        assert!(
            comment.contains("sp:spotify:track:abc123"),
            "should contain source"
        );
    }

    #[tokio::test]
    async fn test_compute_target_comments_batch_multiple_files() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id1 = insert_test_file(
            &pool,
            "/music/track1.flac",
            "flac",
            &[("spotify_id", "spotify:track:one")],
        )
        .await;
        let id2 = insert_test_file(
            &pool,
            "/music/track2.flac",
            "flac",
            &[("spotify_id", "spotify:track:two")],
        )
        .await;

        // File 1: Phase tag
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'build', 2, 'Phase', 'P', 0, 1000000)",
        )
        .bind(id1)
        .execute(&pool)
        .await
        .unwrap();

        // File 2: Vibe tag
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 2, 'dark', 4, 'Vibe', 'V', 0, 1000000)",
        )
        .bind(id2)
        .execute(&pool)
        .await
        .unwrap();

        let results = compute_target_comments_batch(&pool, &[id1, id2])
            .await
            .unwrap();
        assert_eq!(results.len(), 2, "expected 2 results");

        let c1 = results.get(&id1).unwrap();
        assert!(c1.contains("[P__]"), "file1 should have Phase");
        assert!(c1.contains("build"), "file1 should contain build tag");

        let c2 = results.get(&id2).unwrap();
        assert!(c2.contains("[__V]"), "file2 should have Vibe");
        assert!(c2.contains("dark"), "file2 should contain dark tag");
    }

    // ========================================================================
    // Additional DB Edge Cases
    // ========================================================================

    #[tokio::test]
    async fn test_get_files_ordered_by_file_path() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        // Insert in non-alphabetical order
        let _c = insert_test_file(&pool, "/music/z_last.flac", "flac", &[]).await;
        let _a = insert_test_file(&pool, "/music/a_first.mp3", "mp3", &[]).await;
        let _b = insert_test_file(&pool, "/music/b_middle.wav", "wav", &[]).await;

        let files = get_files(&pool).await.unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].file_path, "/music/a_first.mp3");
        assert_eq!(files[1].file_path, "/music/b_middle.wav");
        assert_eq!(files[2].file_path, "/music/z_last.flac");
    }

    #[tokio::test]
    async fn test_get_file_variants_with_backup_location() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        // Add backup location
        sqlx::query(
            "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
             VALUES (?, 'backup', '/backup/track.flac', 12345, 1000000, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let result = get_file_variants(&pool, id).await.unwrap().unwrap();
        assert_eq!(result.variants.len(), 1);
        assert!(
            result.variants[0].backed_up,
            "file should be marked as backed up"
        );
        assert_eq!(result.variants[0].file_size, 12345);
    }

    #[tokio::test]
    async fn test_get_file_variants_stem_with_wav_sources_backed_up() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let stem_id = insert_test_file(&pool, "/music/track.stem.m4a", "stem.m4a", &[]).await;

        let _wav = insert_test_file(
            &pool,
            "/music/track_vocals.wav",
            "wav",
            &[
                ("source_of", &format!("{}", stem_id)),
                ("stem_type", "vocals"),
            ],
        )
        .await;

        // Back up the stem
        sqlx::query(
            "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
             VALUES (?, 'backup', '/backup/track.stem.m4a', 50000, 1000000, 1000000)",
        )
        .bind(stem_id)
        .execute(&pool)
        .await
        .unwrap();

        let result = get_file_variants(&pool, stem_id).await.unwrap().unwrap();
        // stem + 1 WAV = 2 variants
        assert_eq!(result.variants.len(), 2);

        let stem_variant = result
            .variants
            .iter()
            .find(|v| v.file_type == "stem.m4a")
            .unwrap();
        assert!(stem_variant.backed_up, "stem should be backed up");

        let wav_variant = result
            .variants
            .iter()
            .find(|v| v.file_type == "wav")
            .unwrap();
        assert!(
            !wav_variant.backed_up,
            "WAV should NOT be backed up (no backup location for it)"
        );
        assert_eq!(wav_variant.stem_type.as_deref(), Some("vocals"));
    }

    #[tokio::test]
    async fn test_get_file_variants_different_isrc_no_grouping() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let id1 = insert_test_file(
            &pool,
            "/music/track1.flac",
            "flac",
            &[("isrc", "USAAA0000001")],
        )
        .await;
        let _id2 = insert_test_file(
            &pool,
            "/music/track2.stem.m4a",
            "stem.m4a",
            &[("isrc", "USBBB0000002")],
        )
        .await;

        // Different ISRCs — should NOT group together
        let result = get_file_variants(&pool, id1).await.unwrap().unwrap();
        assert_eq!(result.variants.len(), 1, "different ISRC should not group");
        assert_eq!(result.variants[0].file_type, "flac");
    }

    #[tokio::test]
    async fn test_get_file_variants_no_isrc_no_grouping() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_locations_table(&pool).await;

        let id1 = insert_test_file(&pool, "/music/track1.flac", "flac", &[("isrc", "NULL")]).await;
        let _id2 = insert_test_file(
            &pool,
            "/music/track2.stem.m4a",
            "stem.m4a",
            &[("isrc", "NULL")],
        )
        .await;

        // NULL ISRC files should not group together
        let result = get_file_variants(&pool, id1).await.unwrap().unwrap();
        assert_eq!(result.variants.len(), 1, "null ISRC should not group");
    }

    #[tokio::test]
    async fn test_get_tags_for_file_multiple_categories() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        // Insert tags from different categories
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'build', 2, 'Phase', 'P', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 2, 'groovy', 3, 'Mood', 'M', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 3, 'dark', 4, 'Vibe', 'V', 2, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let tags = get_tags_for_file(&pool, id).await.unwrap();
        assert_eq!(tags.len(), 3);
        // Should be ordered alphabetically by tag_name
        assert_eq!(tags[0].name, "build");
        assert_eq!(tags[1].name, "dark");
        assert_eq!(tags[2].name, "groovy");
    }

    #[tokio::test]
    async fn test_update_file_comment_to_empty_string() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;

        let id = insert_test_file(
            &pool,
            "/music/track.flac",
            "flac",
            &[("comment", "old comment")],
        )
        .await;

        update_file_comment(&pool, id, "").await.unwrap();

        let file = get_file_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(file.comment, Some("".to_string()));
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_duplicate_ids() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'groovy', 3, 'Mood', 'M', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Passing same ID twice — SQL `WHERE file_id IN (1,1)` returns the row only once
        let result = get_file_resolved_tags_batch(&pool, &[id, id])
            .await
            .unwrap();
        // SQL IN deduplicates its arguments, so we only get one row
        assert_eq!(
            result.len(),
            1,
            "duplicate IDs in IN clause should yield one row"
        );
        assert_eq!(result[0].0, id);
        assert_eq!(result[0].1, "groovy");
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_single_file_no_tags() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        let result = get_file_resolved_tags_batch(&pool, &[id]).await.unwrap();
        assert!(result.is_empty(), "file with no tags should return empty");
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_mixed_existing_and_not() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'deep', 3, 'Mood', 'M', 0, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Mix of existing and non-existing file IDs
        let result = get_file_resolved_tags_batch(&pool, &[id, 9999])
            .await
            .unwrap();
        assert_eq!(result.len(), 1, "should only return tags for existing file");
        assert_eq!(result[0].0, id);
    }

    #[tokio::test]
    async fn test_get_file_resolved_tags_batch_multiple_tags_per_file() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        create_file_resolved_tags_table(&pool).await;

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        // Two tags for the same file
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'deep', 3, 'Mood', 'M', 1, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 2, 'dark', 4, 'Vibe', 'V', 2, 1000000)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let result = get_file_resolved_tags_batch(&pool, &[id]).await.unwrap();
        assert_eq!(result.len(), 2, "should return 2 tags for the file");
        assert_eq!(result[0].0, id);
        assert_eq!(result[1].0, id);
        let names: Vec<&str> = result.iter().map(|r| r.1.as_str()).collect();
        assert!(names.contains(&"deep"));
        assert!(names.contains(&"dark"));
    }

    #[tokio::test]
    async fn test_get_tags_for_file_distinct_deduplication() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        create_files_table(&pool).await;
        // Use a table without the UNIQUE(file_id, tag_id) constraint
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS file_tags_dup (
                file_id INTEGER NOT NULL,
                tag_id INTEGER NOT NULL,
                tag_name TEXT NOT NULL,
                category_id INTEGER NOT NULL,
                category_name TEXT NOT NULL,
                prefix TEXT NOT NULL,
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let id = insert_test_file(&pool, "/music/track.flac", "flac", &[]).await;

        // Same tag inserted twice (no PK to prevent it)
        sqlx::query("INSERT INTO file_tags_dup (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'groovy', 3, 'Mood', 'M', 0, 1000000)")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO file_tags_dup (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at)
             VALUES (?, 1, 'groovy', 3, 'Mood', 'M', 0, 1000000)")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        // Query using file_tags_dup to verify DISTINCT works
        let tags = sqlx::query_as::<_, Tag>(
            "SELECT DISTINCT tag_id as id, tag_name as name, category_id, sort_order, created_at, 0 as backpack
             FROM file_tags_dup WHERE file_id = ? ORDER BY tag_name",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(tags.len(), 1, "DISTINCT should deduplicate same tag_id");
        assert_eq!(tags[0].name, "groovy");
    }

    // ========================================================================
    // write_comment_to_file regression tests — metaflac --set-tag appends
    // ========================================================================

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
    }

    fn test_flac_path() -> std::path::PathBuf {
        fixtures_dir().join("test.flac")
    }

    /// Helper: count how many COMMENT tags a FLAC file has.
    fn count_comment_tags(path: &std::path::Path) -> usize {
        let output = std::process::Command::new("metaflac")
            .arg("--list")
            .arg(path)
            .output()
            .expect("metaflac not found");
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().filter(|l| l.contains("COMMENT=")).count()
    }

    /// Helper: get the first COMMENT tag value from a FLAC file.
    fn get_comment_tag(path: &std::path::Path) -> Option<String> {
        let output = std::process::Command::new("metaflac")
            .arg("--list")
            .arg(path)
            .output()
            .expect("metaflac not found");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(idx) = line.find("COMMENT=") {
                return Some(line[idx + "COMMENT=".len()..].to_string());
            }
        }
        None
    }

    /// Test: write_comment_to_file on FLAC must REPLACE the existing comment,
    /// not append a duplicate. This is the regression test for the metaflac
    /// --set-tag append behaviour.
    #[tokio::test]
    async fn test_write_comment_to_file_replaces_not_appends_flac() {
        let src = test_flac_path();
        assert!(src.exists(), "test fixture missing: {:?}", src);

        // Work on a copy so we do not corrupt the fixture.
        let tmp = std::env::temp_dir().join("mmm_test_replace.flac");
        std::fs::copy(&src, &tmp).unwrap();

        // Sanity: fixture should have exactly 1 COMMENT tag.
        let before_count = count_comment_tags(&tmp);
        assert_eq!(
            before_count, 1,
            "test fixture should have exactly 1 COMMENT tag, found {}",
            before_count
        );
        let before_val = get_comment_tag(&tmp);
        assert_eq!(before_val.as_deref(), Some("old comment value"));

        // Write a DIFFERENT comment.
        let new_comment = "[PMV] new tag value";
        write_comment_to_file(&tmp.to_string_lossy(), new_comment)
            .await
            .expect("write_comment_to_file should succeed");

        // After writing, there must be exactly 1 COMMENT tag with the new value.
        let after_count = count_comment_tags(&tmp);
        assert_eq!(
            after_count, 1,
            "write_comment_to_file must replace, not append; expected 1 COMMENT tag, found {}",
            after_count
        );
        let after_val = get_comment_tag(&tmp);
        assert_eq!(
            after_val.as_deref(),
            Some(new_comment),
            "COMMENT tag should be the new value"
        );

        // Cleanup.
        let _ = std::fs::remove_file(&tmp);
    }

    /// Test: writing the same comment twice should be idempotent.
    #[tokio::test]
    async fn test_write_comment_to_file_idempotent() {
        let src = test_flac_path();
        assert!(src.exists());

        let tmp = std::env::temp_dir().join("mmm_test_idem.flac");
        std::fs::copy(&src, &tmp).unwrap();

        let comment = "[PMV] idempotent test";

        // Write twice.
        write_comment_to_file(&tmp.to_string_lossy(), comment)
            .await
            .unwrap();
        write_comment_to_file(&tmp.to_string_lossy(), comment)
            .await
            .unwrap();

        let count = count_comment_tags(&tmp);
        assert_eq!(count, 1, "idempotent: should still have 1 COMMENT tag");
        let val = get_comment_tag(&tmp);
        assert_eq!(val.as_deref(), Some(comment));

        let _ = std::fs::remove_file(&tmp);
    }
}
