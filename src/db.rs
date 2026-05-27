#![allow(dead_code)]

use std::{fs, path::Path, time::SystemTime};

use anyhow::{Result, anyhow};
use lofty::{prelude::*, read_from_path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::audio_extensions::AudioExtension;
use crate::scan_cache;
use sqlx::{FromRow, Pool, Row, Sqlite, SqliteConnection, SqlitePool};
use tracing::{debug, info, warn};

// ============================================================================
// Scan Mode
// ============================================================================

#[derive(Debug, Clone)]
pub enum ScanMode {
    Full,
    Incremental { since: Option<i64> },
}

// ============================================================================
// Database Models (8-table schema)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TagCategory {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub prefix: String,
    pub sort_order: i32,
    pub is_default: bool,
    pub tag_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub category_id: i64,
    pub sort_order: i64,
    pub created_at: i64,
    pub followed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServiceTrack {
    pub id: i64,
    pub service: String,
    pub service_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub metadata_json: Option<String>,
    pub imported_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServicePlaylist {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub name: String,
    pub description: Option<String>,
    pub metadata_json: Option<String>,
    pub imported_at: i64,
    pub updated_at: i64,
    pub last_fetched_at: Option<i64>,
    pub remote_track_count: i64,
    pub remote_unique_count: i64,
    pub archive_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServicePlaylistTrack {
    pub playlist_id: i64,
    pub track_id: i64,
    pub position: Option<i32>,
    pub added_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct File {
    pub id: i64,
    pub file_path: String,
    pub file_hash: String,
    pub file_type: String,
    pub file_size: i64,
    pub last_modified: i64,
    pub isrc: Option<String>,
    pub last_scanned: i64,

    // Audio metadata
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<i32>,
    pub total_tracks: Option<i32>,
    pub disc_number: Option<i32>,
    pub total_discs: Option<i32>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,

    // BPM/Key from Traktor/EXIF
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,

    // Traktor stats
    pub rating: i32,
    pub play_count: i32,
    pub last_played: Option<i64>,

    // Direct service IDs
    pub spotify_id: Option<String>,
    pub soundcloud_id: Option<String>,
    pub youtube_id: Option<String>,

    // Source WAV linking (WAV source subdirectory → stem file)
    pub source_of: Option<i64>,

    // Timestamps
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfig {
    pub id: i64,
    pub service: String,
    pub refresh_token: Option<String>,
    pub metadata_json: Option<String>,
    pub access_token: Option<String>,
    pub token_expiry: Option<i64>,
    pub user_id: Option<String>,
    pub playlist_id: Option<String>,
    pub is_connected: bool,
    pub last_checked: Option<i64>,
    pub last_synced: Option<i64>,
    pub remote_playlists_count: i64,
    pub remote_tracks_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Folder {
    pub id: i64,
    pub folder_path: String,
    pub active: bool,
    pub scan_recursive: bool,
    pub fixed_extensions: bool,
    pub file_extensions: String,
    pub max_depth: i32,
    pub last_scanned: Option<i64>,
    pub scan_sources: bool,
    pub backup_path: Option<String>,
    pub auto_backup: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// File Lifecycle Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileLocation {
    pub id: i64,
    pub file_id: i64,
    pub location_type: String, // 'local' | 'backup'
    pub path: String,
    pub file_size: Option<i64>,
    pub last_verified: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneCandidate {
    pub file_id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub reason: String,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub local_file_count: i64,
    pub local_size_bytes: i64,
    pub local_stems: i64,
    pub local_flacs: i64,
    pub local_mp3s: i64,
    pub local_wavs: i64,
    pub local_other: i64,
    pub local_stems_size: i64,
    pub local_flacs_size: i64,
    pub local_wavs_size: i64,
    pub local_mp3s_size: i64,
    pub backup_count: i64,
    pub wav_source_dirs: i64,
    pub prune_candidate_count: i64,
    pub prune_candidate_bytes: i64,
    pub wav_indexed: i64,
    pub wav_backed_up: i64,
}

// ============================================================================
// Database Connection
// ============================================================================

pub async fn connect_db() -> Result<SqlitePool> {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
    use std::str::FromStr;
    use std::time::Duration;

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:app.db".to_string());
    let options = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePool::connect_with(options).await?;
    Ok(pool)
}

pub async fn init_db(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!().run(pool).await?;
    Ok(())
}

// ============================================================================
// File Scanning & Metadata Extraction
// ============================================================================

pub fn calculate_file_hash(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

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

#[allow(clippy::type_complexity)]
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
        created_at: now,
        updated_at: now,
    };

    // ── CACHE SAVE ──────────────────────────────────────────────────────────
    // In record mode, persist the extracted metadata for future replay.
    scan_cache::try_save(&scanned_file).await;

    Ok(scanned_file)
}

pub async fn scan_and_store_file(pool: &Pool<Sqlite>, path: &Path) -> Result<File> {
    let file = extract_audio_metadata_from_file(path).await?;

    let row = sqlx::query_as::<_, File>(
        r#"
        INSERT INTO files (
            file_path, file_hash, file_type, file_size, last_modified, isrc, last_scanned,
            title, artist, album, album_artist, track_number, total_tracks, disc_number, total_discs,
            genre, year, composer, comment, duration_ms, bitrate, sample_rate, channels,
            bpm, musical_key, rating, play_count, last_played,
            spotify_id, soundcloud_id, youtube_id, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
    .bind(file.created_at)
    .bind(file.updated_at)
    .fetch_one(pool)
    .await?;

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
// Basic CRUD Operations
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

pub async fn get_tag_categories(pool: &Pool<Sqlite>) -> Result<Vec<TagCategory>> {
    let categories =
        sqlx::query_as::<_, TagCategory>("SELECT * FROM v_tag_categories ORDER BY sort_order")
            .fetch_all(pool)
            .await?;
    Ok(categories)
}

/// Get a single tag category by ID
pub async fn get_tag_category_by_id(
    pool: &Pool<Sqlite>,
    category_id: i64,
) -> Result<Option<TagCategory>> {
    let category = sqlx::query_as::<_, TagCategory>("SELECT * FROM v_tag_categories WHERE id = ?")
        .bind(category_id)
        .fetch_optional(pool)
        .await?;
    Ok(category)
}

pub async fn get_default_tag_category(pool: &Pool<Sqlite>) -> Result<Option<TagCategory>> {
    let category =
        sqlx::query_as::<_, TagCategory>("SELECT * FROM v_tag_categories WHERE is_default = TRUE")
            .fetch_optional(pool)
            .await?;
    Ok(category)
}

pub async fn get_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    let tags = sqlx::query_as::<_, Tag>("SELECT * FROM tags ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(tags)
}

pub async fn get_tag_by_name(pool: &Pool<Sqlite>, name: &str) -> Result<Option<Tag>> {
    let tag = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE name = ? COLLATE NOCASE")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(tag)
}

pub async fn create_tag(pool: &Pool<Sqlite>, name: &str, category_id: i64) -> Result<Tag> {
    let tag = sqlx::query_as::<_, Tag>(
        r#"
        INSERT INTO tags (name, category_id, created_at)
        VALUES (?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(name)
    .bind(category_id)
    .bind(chrono::Utc::now().timestamp())
    .fetch_one(pool)
    .await?;
    Ok(tag)
}

pub async fn get_tag_by_id(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Option<Tag>> {
    let tag = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE id = ?")
        .bind(tag_id)
        .fetch_optional(pool)
        .await?;
    Ok(tag)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConnections {
    pub spotify: bool,
    pub soundcloud: bool,
    pub youtube: bool,
}

pub async fn get_tag_service_connections(
    pool: &Pool<Sqlite>,
    tag_name: &str,
) -> Result<ServiceConnections> {
    let services = sqlx::query_scalar::<_, String>(
        r#"SELECT DISTINCT vtp.service FROM v_tag_playlist vtp WHERE LOWER(TRIM(vtp.tag_name)) = LOWER(TRIM(?))"#
    )
    .bind(tag_name)
    .fetch_all(pool)
    .await?;

    let spotify = services.iter().any(|s| s == "spotify");
    let soundcloud = services.iter().any(|s| s == "soundcloud");
    let youtube = services.iter().any(|s| s == "youtube");

    Ok(ServiceConnections {
        spotify,
        soundcloud,
        youtube,
    })
}

pub async fn update_tag(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    name: Option<&str>,
    category_id: Option<i64>,
) -> Result<Tag> {
    let mut updates = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(name) = name {
        updates.push("name = ?");
        params.push(name.to_string());
    }

    if let Some(category_id) = category_id {
        updates.push("category_id = ?");
        params.push(category_id.to_string());
    }

    if updates.is_empty() {
        // No updates, return existing tag
        return get_tag_by_id(pool, tag_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Tag not found"));
    }

    let query_str = format!(
        "UPDATE tags SET {} WHERE id = ? RETURNING *",
        updates.join(", ")
    );

    let mut query = sqlx::query_as::<_, Tag>(&query_str);

    // Bind parameters in order
    for param in params {
        query = query.bind(param);
    }

    query = query.bind(tag_id);

    let tag = query.fetch_one(pool).await?;
    Ok(tag)
}

pub async fn delete_tag(pool: &Pool<Sqlite>, tag_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ─── Tag Parents ────────────────────────────────────────────────────────────

/// Get all parent tags for a given tag
pub async fn get_tag_parents(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Vec<Tag>> {
    let parents = sqlx::query_as::<_, Tag>(
        r#"
        SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at
        FROM tag_parents tp
        JOIN tags t ON t.id = tp.parent_tag_id
        WHERE tp.tag_id = ?
        ORDER BY t.name
        "#,
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await?;
    Ok(parents)
}

/// Get all tags that use this tag as a parent (reverse lookup)
pub async fn get_tag_children(pool: &Pool<Sqlite>, parent_tag_id: i64) -> Result<Vec<Tag>> {
    let children = sqlx::query_as::<_, Tag>(
        r#"
        SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at
        FROM tag_parents tp
        JOIN tags t ON t.id = tp.tag_id
        WHERE tp.parent_tag_id = ?
        ORDER BY t.name
        "#,
    )
    .bind(parent_tag_id)
    .fetch_all(pool)
    .await?;
    Ok(children)
}

/// Set parent tags for a tag (replaces all existing parents).
/// Only tags in the Setlist category can have parents.
/// Returns the new list of parent tags.
pub async fn set_tag_parents(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    parent_tag_ids: &[i64],
) -> Result<Vec<Tag>> {
    // Validate: the tag must be in the Setlist category
    let category_row = sqlx::query(
        "SELECT tc.name FROM tags t JOIN tag_categories tc ON tc.id = t.category_id WHERE t.id = ?",
    )
    .bind(tag_id)
    .fetch_optional(pool)
    .await?;

    match category_row {
        Some(row) => {
            let cat_name: String = row.try_get("name")?;
            if cat_name != "Setlist" {
                return Err(anyhow::anyhow!(
                    "Only Setlist tags can have parent tags. Tag is in category: {}",
                    cat_name
                ));
            }
        }
        None => return Err(anyhow::anyhow!("Tag with id {} not found", tag_id)),
    }

    // Validate: no self-reference
    if parent_tag_ids.contains(&tag_id) {
        return Err(anyhow::anyhow!("A tag cannot be its own parent"));
    }

    // Validate: all parent tags exist
    for &parent_id in parent_tag_ids {
        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM tags WHERE id = ?")
            .bind(parent_id)
            .fetch_one(pool)
            .await?;
        if !exists {
            return Err(anyhow::anyhow!(
                "Parent tag with id {} not found",
                parent_id
            ));
        }
    }

    // Validate: parent tags must not be Setlist (only P/M/V/E categories)
    // Setlist parents create indirection without resolution — just another
    // long name that would itself need parents.
    if !parent_tag_ids.is_empty() {
        let placeholders: Vec<String> = parent_tag_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT t.name FROM tags t JOIN tag_categories tc ON tc.id = t.category_id WHERE t.id IN ({}) AND tc.name = 'Setlist' LIMIT 1",
            placeholders.join(",")
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for id in parent_tag_ids {
            q = q.bind(id);
        }
        if let Ok(Some(name)) = q.fetch_optional(pool).await {
            return Err(anyhow::anyhow!(
                "Parent tag '{}' is a Setlist tag. Parent tags must be from Phase, Mood, Vibe, or Merkmal categories, not Setlist.",
                name
            ));
        }
    }

    // Delete existing parents and insert new ones in a transaction
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM tag_parents WHERE tag_id = ?")
        .bind(tag_id)
        .execute(&mut *tx)
        .await?;

    for &parent_id in parent_tag_ids {
        sqlx::query("INSERT OR IGNORE INTO tag_parents (tag_id, parent_tag_id) VALUES (?, ?)")
            .bind(tag_id)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    // Return the new parent tags
    get_tag_parents(pool, tag_id).await
}

/// Get playlists that don't have corresponding tags (case-insensitive name matching)
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
    let default_category = match get_default_tag_category(pool).await? {
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

pub async fn create_tag_category(
    pool: &Pool<Sqlite>,
    name: &str,
    prefix: &str,
    icon: &str,
    is_default: bool,
    sort_order: i64,
) -> Result<TagCategory> {
    let now = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        r#"
        INSERT INTO tag_categories (name, prefix, icon, is_default, sort_order, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(name)
    .bind(prefix)
    .bind(icon)
    .bind(is_default)
    .bind(sort_order)
    .bind(now)
    .execute(pool)
    .await?;

    let new_id = result.last_insert_rowid();
    get_tag_category_by_id(pool, new_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created tag category"))
}

pub async fn update_tag_category_metadata(
    pool: &Pool<Sqlite>,
    category_id: i64,
    name: Option<&str>,
    prefix: Option<&str>,
    icon: Option<&str>,
    is_default: Option<bool>,
    sort_order: Option<i64>,
) -> Result<TagCategory> {
    let mut updates = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(name) = name {
        updates.push("name = ?");
        params.push(name.to_string());
    }
    if let Some(prefix) = prefix {
        updates.push("prefix = ?");
        params.push(prefix.to_string());
    }
    if let Some(icon) = icon {
        updates.push("icon = ?");
        params.push(icon.to_string());
    }
    if let Some(is_default) = is_default {
        updates.push("is_default = ?");
        params.push(if is_default {
            "1".to_string()
        } else {
            "0".to_string()
        });
    }
    if let Some(sort_order) = sort_order {
        updates.push("sort_order = ?");
        params.push(sort_order.to_string());
    }

    if updates.is_empty() {
        // No updates, return existing category
        return get_tag_category_by_id(pool, category_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Tag category not found"));
    }

    let query_str = format!(
        "UPDATE tag_categories SET {} WHERE id = ?",
        updates.join(", ")
    );

    let mut db_query = sqlx::query(&query_str);
    for param in params {
        db_query = db_query.bind(param);
    }
    db_query = db_query.bind(category_id);

    db_query.execute(pool).await?;

    get_tag_category_by_id(pool, category_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Tag category not found"))
}

pub async fn delete_tag_category(pool: &Pool<Sqlite>, category_id: i64) -> Result<()> {
    // Check if category is in use
    let count: Option<i64> = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE category_id = ?")
        .bind(category_id)
        .fetch_one(pool)
        .await?;

    let count_val: i64 = count.unwrap_or_default();

    if count_val > 0 {
        return Err(anyhow::anyhow!(
            "Cannot delete category that is in use by tags"
        ));
    }

    sqlx::query("DELETE FROM tag_categories WHERE id = ?")
        .bind(category_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_service_config(
    pool: &Pool<Sqlite>,
    service: &str,
) -> Result<Option<ServiceConfig>> {
    let config =
        sqlx::query_as::<_, ServiceConfig>("SELECT * FROM service_config WHERE service = ?")
            .bind(service)
            .fetch_optional(pool)
            .await?;
    Ok(config)
}

/// Update or insert a storage settings row in service_config.
/// Uses the 'storage' service key to store JSON metadata (e.g. stem_preferred).
pub async fn update_storage_setting(pool: &Pool<Sqlite>, meta_json: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO service_config (service, metadata_json, is_connected)
         VALUES ('storage', ?, 1)
         ON CONFLICT(service) DO UPDATE SET metadata_json = excluded.metadata_json, updated_at = unixepoch()"
    )
    .bind(meta_json)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_service_config(
    pool: &Pool<Sqlite>,
    service: &str,
    user_id: Option<&str>,
    playlist_id: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        INSERT OR REPLACE INTO service_config
        (service, user_id, playlist_id, updated_at, created_at)
        VALUES (?, ?, ?, ?, COALESCE((SELECT created_at FROM service_config WHERE service = ?), ?))
        "#,
    )
    .bind(service)
    .bind(user_id)
    .bind(playlist_id)
    .bind(now)
    .bind(service)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn update_service_connection_status(
    pool: &Pool<Sqlite>,
    service: &str,
    is_connected: bool,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        UPDATE service_config
        SET is_connected = ?, last_checked = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(is_connected)
    .bind(now)
    .bind(now)
    .bind(service)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn update_service_sync_timestamp(pool: &Pool<Sqlite>, service: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        UPDATE service_config
        SET last_synced = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(now)
    .bind(now)
    .bind(service)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn update_service_tokens(
    pool: &Pool<Sqlite>,
    service: &str,
    refresh_token: Option<&str>,
    access_token: Option<&str>,
    token_expiry: Option<i64>,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        UPDATE service_config
        SET refresh_token = ?, access_token = ?, token_expiry = ?, is_connected = 1, last_checked = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(refresh_token)
    .bind(access_token)
    .bind(token_expiry)
    .bind(now)
    .bind(now)
    .bind(service)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_folders(pool: &Pool<Sqlite>) -> Result<Vec<Folder>> {
    let folders = sqlx::query_as::<_, Folder>("SELECT * FROM folders ORDER BY folder_path")
        .fetch_all(pool)
        .await?;
    Ok(folders)
}

pub async fn create_folder(pool: &Pool<Sqlite>, folder_path: &str, active: bool) -> Result<Folder> {
    create_folder_with_config(
        pool,
        folder_path,
        active,
        false,         // scan_recursive
        false,         // fixed_extensions
        String::new(), // file_extensions
        1,             // max_depth
    )
    .await
}

/// Create a folder with full configuration
pub async fn create_folder_with_config(
    pool: &Pool<Sqlite>,
    folder_path: &str,
    active: bool,
    scan_recursive: bool,
    fixed_extensions: bool,
    file_extensions: String,
    max_depth: i32,
) -> Result<Folder> {
    // Validate file_extensions if fixed_extensions is true
    if fixed_extensions && !file_extensions.trim().is_empty() {
        crate::audio_extensions::AudioExtension::parse_list(&file_extensions)
            .map_err(|e| anyhow!("Invalid file extensions: {}", e))?;
    }

    let now = chrono::Utc::now().timestamp();
    let folder = sqlx::query_as::<_, Folder>(
        r#"
        INSERT INTO folders (
            folder_path, active, scan_recursive, fixed_extensions,
            file_extensions, max_depth, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(folder_path)
    .bind(active)
    .bind(scan_recursive)
    .bind(fixed_extensions)
    .bind(file_extensions)
    .bind(max_depth)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(folder)
}

/// Get a single folder by ID
pub async fn get_folder_by_id(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Folder>> {
    let folder = sqlx::query_as::<_, Folder>("SELECT * FROM folders WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(folder)
}

/// Update folder path and active status
pub async fn update_folder(
    pool: &Pool<Sqlite>,
    id: i64,
    folder_path: Option<&str>,
    active: Option<bool>,
) -> Result<Folder> {
    update_folder_with_config(
        pool,
        id,
        folder_path,
        active,
        None, // scan_recursive
        None, // fixed_extensions
        None, // file_extensions
        None, // max_depth
    )
    .await
}

/// Update folder with full configuration
#[allow(clippy::too_many_arguments)]
pub async fn update_folder_with_config(
    pool: &Pool<Sqlite>,
    id: i64,
    folder_path: Option<&str>,
    active: Option<bool>,
    scan_recursive: Option<bool>,
    fixed_extensions: Option<bool>,
    file_extensions: Option<&str>,
    max_depth: Option<i32>,
) -> Result<Folder> {
    let now = chrono::Utc::now().timestamp();

    // Validate file_extensions if fixed_extensions is true and file_extensions is provided
    if let (Some(true), Some(extensions)) = (fixed_extensions, file_extensions)
        && !extensions.trim().is_empty()
    {
        crate::audio_extensions::AudioExtension::parse_list(extensions)
            .map_err(|e| anyhow!("Invalid file extensions: {}", e))?;
    }

    // Build dynamic query based on what's being updated
    if let Some(path) = folder_path {
        // Validate new path if provided
        let normalized_path = normalize_and_validate_folder_path(path)?;

        let folder = sqlx::query_as::<_, Folder>(
            r#"
            UPDATE folders
            SET
                folder_path = ?,
                active = COALESCE(?, active),
                scan_recursive = COALESCE(?, scan_recursive),
                fixed_extensions = COALESCE(?, fixed_extensions),
                file_extensions = COALESCE(?, file_extensions),
                max_depth = COALESCE(?, max_depth),
                updated_at = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(normalized_path)
        .bind(active)
        .bind(scan_recursive)
        .bind(fixed_extensions)
        .bind(file_extensions)
        .bind(max_depth)
        .bind(now)
        .bind(id)
        .fetch_one(pool)
        .await?;

        // If scan config changed, reset last_scanned to force a full rescan
        if scan_recursive.is_some() || max_depth.is_some() || file_extensions.is_some() {
            sqlx::query("UPDATE folders SET last_scanned = NULL WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
        }

        Ok(folder)
    } else if active.is_some()
        || scan_recursive.is_some()
        || fixed_extensions.is_some()
        || file_extensions.is_some()
        || max_depth.is_some()
    {
        let folder = sqlx::query_as::<_, Folder>(
            r#"
            UPDATE folders
            SET
                active = COALESCE(?, active),
                scan_recursive = COALESCE(?, scan_recursive),
                fixed_extensions = COALESCE(?, fixed_extensions),
                file_extensions = COALESCE(?, file_extensions),
                max_depth = COALESCE(?, max_depth),
                updated_at = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(active)
        .bind(scan_recursive)
        .bind(fixed_extensions)
        .bind(file_extensions)
        .bind(max_depth)
        .bind(now)
        .bind(id)
        .fetch_one(pool)
        .await?;

        // If scan config changed, reset last_scanned to force a full rescan
        if scan_recursive.is_some() || max_depth.is_some() || file_extensions.is_some() {
            sqlx::query("UPDATE folders SET last_scanned = NULL WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
        }

        Ok(folder)
    } else {
        // Nothing to update
        if let Some(folder) = get_folder_by_id(pool, id).await? {
            Ok(folder)
        } else {
            Err(anyhow!("Folder not found with id: {}", id))
        }
    }
}

/// Update only the active status of a folder
pub async fn update_folder_active(pool: &Pool<Sqlite>, id: i64, active: bool) -> Result<Folder> {
    update_folder(pool, id, None, Some(active)).await
}

/// Delete a folder by ID
pub async fn delete_folder(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
    let result = sqlx::query("DELETE FROM folders WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Folder not found with id: {}", id));
    }

    Ok(())
}

/// Scan a folder and return number of files processed
pub async fn scan_folder(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    scan_mode: ScanMode,
) -> Result<usize> {
    // Get folder path
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found with id: {}", folder_id))?;

    let path = std::path::Path::new(&folder.folder_path);

    // Determine effective scan mode based on folder's last_scanned
    let effective_mode = match &scan_mode {
        ScanMode::Full => ScanMode::Full,
        ScanMode::Incremental { .. } => {
            if let Some(ts) = folder.last_scanned {
                ScanMode::Incremental { since: Some(ts) }
            } else {
                // Never scanned before, do full scan
                ScanMode::Full
            }
        }
    };

    let file_count = scan_directory_with_config(
        pool,
        path,
        folder.scan_recursive,
        folder.fixed_extensions,
        folder.file_extensions,
        folder.max_depth,
        effective_mode,
    )
    .await?;

    // Update last_scanned timestamp
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE folders SET last_scanned = ?, updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(folder_id)
        .execute(pool)
        .await?;

    Ok(file_count)
}

/// Get the count of files in a folder
pub async fn get_folder_file_count(pool: &Pool<Sqlite>, folder_id: i64) -> Result<i64> {
    // Get folder path
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found with id: {}", folder_id))?;

    // Count files where file_path starts with folder path
    // Ensure folder path ends with a slash for proper matching
    let folder_path = if folder.folder_path.ends_with('/') {
        folder.folder_path.clone()
    } else {
        format!("{}/", folder.folder_path)
    };

    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM files
        WHERE file_path LIKE ? || '%'
        "#,
    )
    .bind(folder_path)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

// Helper to normalize and validate folder path
pub fn normalize_and_validate_folder_path(path: &str) -> Result<String> {
    let expanded = shellexpand::full(path)?;
    let path = Path::new(&*expanded);

    if !path.exists() {
        return Err(anyhow!("Folder does not exist: {}", path.display()));
    }

    if !path.is_dir() {
        return Err(anyhow!("Path is not a directory: {}", path.display()));
    }

    Ok(expanded.to_string())
}

/// Update the comment field for a file in the database
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

    // Get all tags for this file with category prefix via v_file_resolved_tags view
    // This resolves Setlist tags through their parent tags (if any)
    let tag_rows = sqlx::query(
        "SELECT frt.tag_name, frt.prefix
         FROM v_file_resolved_tags frt
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

/// Get all service tracks from the database
pub async fn get_service_tracks(pool: &Pool<Sqlite>) -> Result<Vec<ServiceTrack>> {
    let tracks =
        sqlx::query_as::<_, ServiceTrack>("SELECT * FROM service_tracks ORDER BY service, title")
            .fetch_all(pool)
            .await?;
    Ok(tracks)
}

/// Get tags for a service track (via playlist name matching)
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

/// Create or update a service playlist in the database
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

/// Refresh track tags by creating tags from playlist names that don't exist yet
pub async fn refresh_track_tags(pool: &Pool<Sqlite>) -> Result<()> {
    // Get default tag category (Setlist)
    let default_category = match get_default_tag_category(pool).await? {
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
        if let Ok(Some(_)) = get_tag_by_name(pool, playlist_name).await {
            continue;
        }

        // Create new tag with default category
        create_tag(pool, playlist_name, default_category.id).await?;
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
    match get_tag_by_name(pool, playlist_name).await {
        Ok(Some(existing_tag)) => return Ok(existing_tag),
        Ok(None) => (),          // Tag doesn't exist, continue
        Err(e) => return Err(e), // Propagate error
    }

    // Get default tag category (Setlist)
    let default_category = match get_default_tag_category(pool).await? {
        Some(category) => category,
        None => return Err(anyhow::anyhow!("No default tag category found")),
    };

    // Create new tag with default category
    let tag = create_tag(pool, playlist_name, default_category.id).await?;
    debug!("Created tag for playlist name: {}", playlist_name);
    Ok(tag)
}

// ─── Embedding & Review Functions ─────────────────────────────────────────────

/// Get all unreviewed tags (reviewed_at IS NULL), sorted by name
pub async fn get_unreviewed_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    let tags =
        sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE reviewed_at IS NULL ORDER BY name")
            .fetch_all(pool)
            .await?;
    Ok(tags)
}

/// Get counts of reviewed and unreviewed tags
pub async fn get_tag_review_counts(pool: &Pool<Sqlite>) -> Result<(usize, usize)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let unreviewed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE reviewed_at IS NULL")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    Ok((total as usize - unreviewed as usize, unreviewed as usize))
}

/// Set category_id and reviewed_at for a tag.
/// Returns the updated Tag.
pub async fn categorize_tag(pool: &Pool<Sqlite>, tag_id: i64, category_id: i64) -> Result<Tag> {
    let now = chrono::Utc::now().timestamp();
    let tag = sqlx::query_as::<_, Tag>(
        r#"
        UPDATE tags
        SET category_id = ?, reviewed_at = ?
        WHERE id = ?
        RETURNING *
        "#,
    )
    .bind(category_id)
    .bind(now)
    .bind(tag_id)
    .fetch_one(pool)
    .await?;
    Ok(tag)
}

/// Bulk-update category_id + reviewed_at for multiple tags in a single transaction.
/// Returns the number of tags updated.
pub async fn bulk_categorize_tags(
    pool: &Pool<Sqlite>,
    tag_ids: &[i64],
    category_id: i64,
) -> Result<u64> {
    if tag_ids.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;
    let mut count: u64 = 0;
    for &tag_id in tag_ids {
        let rows = sqlx::query("UPDATE tags SET category_id = ?, reviewed_at = ? WHERE id = ?")
            .bind(category_id)
            .bind(now)
            .bind(tag_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        count += rows;
    }
    tx.commit().await?;
    Ok(count)
}

/// Get a single tag embedding from the cache
pub async fn get_tag_embedding(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Option<Vec<u8>>> {
    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT embedding FROM tag_embeddings WHERE tag_id = ?")
            .bind(tag_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Upsert (insert or replace) a tag embedding
pub async fn upsert_tag_embedding(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    embedding_blob: &[u8],
    model_version: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        r#"
        INSERT INTO tag_embeddings (tag_id, embedding, model_version, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(tag_id) DO UPDATE SET
            embedding = excluded.embedding,
            model_version = excluded.model_version,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(tag_id)
    .bind(embedding_blob)
    .bind(model_version)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get all tag embeddings for a given category
pub async fn get_embeddings_by_category(
    pool: &Pool<Sqlite>,
    category_id: i64,
) -> Result<Vec<(i64, Vec<u8>)>> {
    let rows = sqlx::query_as::<_, (i64, Vec<u8>)>(
        r#"
        SELECT te.tag_id, te.embedding
        FROM tag_embeddings te
        JOIN tags t ON t.id = te.tag_id
        WHERE t.category_id = ?
        "#,
    )
    .bind(category_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get ALL tag embeddings (tag_id → embedding blob).
/// Returns all tags with their embedding, including tag name.
pub async fn get_all_embeddings(
    pool: &Pool<Sqlite>,
) -> Result<Vec<(i64, String, Option<Vec<u8>>)>> {
    let rows = sqlx::query_as::<_, (i64, String, Option<Vec<u8>>)>(
        r#"
        SELECT t.id, t.name, te.embedding
        FROM tags t
        LEFT JOIN tag_embeddings te ON te.tag_id = t.id
        ORDER BY t.name
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Delete all tag embeddings (used before recompute)
pub async fn clear_all_embeddings(pool: &Pool<Sqlite>) -> Result<()> {
    sqlx::query("DELETE FROM tag_embeddings")
        .execute(pool)
        .await?;
    Ok(())
}

/// Reset reviewed_at to NULL for all tags (unreview all)
pub async fn reset_all_reviewed_at(pool: &Pool<Sqlite>) -> Result<usize> {
    let result = sqlx::query("UPDATE tags SET reviewed_at = NULL")
        .execute(pool)
        .await?;
    Ok(result.rows_affected() as usize)
}

/// Check bulk tag names against DB.
/// Returns for each name: whether it exists, its current category_id (if any), and its current category name.
pub async fn bulk_check_tags(
    pool: &Pool<Sqlite>,
    names: &[String],
) -> Result<Vec<(String, Option<i64>, Option<String>)>> {
    let mut results = Vec::new();
    for name in names {
        let tag = sqlx::query_as::<_, Tag>(
            "SELECT t.* FROM tags t
             WHERE t.name = ? COLLATE NOCASE",
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;

        match tag {
            Some(t) => {
                let cat_name: Option<String> = if t.category_id > 0 {
                    let name: Option<String> =
                        sqlx::query_scalar("SELECT name FROM tag_categories WHERE id = ?")
                            .bind(t.category_id)
                            .fetch_optional(pool)
                            .await?
                            .flatten();
                    name
                } else {
                    None
                };
                results.push((name.clone(), Some(t.category_id), cat_name));
            }
            None => {
                results.push((name.clone(), None, None));
            }
        }
    }
    Ok(results)
}

/// Bulk create tags (all assign category + mark reviewed).
/// Returns created tags with their ids.
pub async fn bulk_create_tags(pool: &Pool<Sqlite>, entries: &[(String, i64)]) -> Result<Vec<Tag>> {
    let now = chrono::Utc::now().timestamp();
    let mut created = Vec::new();
    for (name, category_id) in entries {
        let tag = sqlx::query_as::<_, Tag>(
            r#"
            INSERT INTO tags (name, category_id, created_at, reviewed_at)
            VALUES (?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(name)
        .bind(category_id)
        .bind(now)
        .bind(now)
        .fetch_one(pool)
        .await?;
        created.push(tag);
    }
    Ok(created)
}

/// Bulk update tags: change category + mark reviewed.
/// Returns updated tags.
pub async fn bulk_update_tags(pool: &Pool<Sqlite>, entries: &[(String, i64)]) -> Result<Vec<Tag>> {
    let now = chrono::Utc::now().timestamp();
    let mut updated = Vec::new();
    for (name, category_id) in entries {
        let tag = sqlx::query_as::<_, Tag>(
            r#"
            UPDATE tags
            SET category_id = ?, reviewed_at = ?
            WHERE name = ? COLLATE NOCASE
            RETURNING *
            "#,
        )
        .bind(category_id)
        .bind(now)
        .bind(name)
        .fetch_one(pool)
        .await?;
        updated.push(tag);
    }
    Ok(updated)
}

/// Mark existing tags as reviewed (no category change).
pub async fn bulk_review_tags(pool: &Pool<Sqlite>, names: &[String]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let mut count = 0;
    for name in names {
        let result = sqlx::query(
            "UPDATE tags SET reviewed_at = ? WHERE name = ? COLLATE NOCASE AND reviewed_at IS NULL",
        )
        .bind(now)
        .bind(name)
        .execute(pool)
        .await?;
        count += result.rows_affected() as usize;
    }
    Ok(count)
}

// ============================================================================
// Playlist Subscriptions
// ============================================================================

/// A subscription to a playlist for periodic syncing.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
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

// ============================================================================
// Tag Similarity (Semantic Tag Matching)
// ============================================================================

// ─── Curation Queue ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CurationTag {
    pub id: i64,
    pub name: String,
    pub file_count: i64,
    pub parent_count: i64,
    pub category_id: i64,
    pub category: String,
    pub category_icon: String,
    pub parents_json: String,
}

/// Get the curation queue: all Setlist tags with file counts, parent counts,
/// and full parent tag details as a JSON string.
pub async fn get_curation_queue(
    pool: &Pool<Sqlite>,
    search: Option<&str>,
    sort: Option<&str>,
    order: Option<&str>,
    has_parents: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<CurationTag>> {
    let limit = limit.unwrap_or(200);
    let search_pattern = search.and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(format!("%{}%", s))
        }
    });

    let mut sql = String::from(
        r#"
        SELECT
            t.id,
            t.name,
            COALESCE(vfc.file_count, 0) AS file_count,
            COALESCE(tp_count.parent_count, 0) AS parent_count,
            tc.id AS category_id,
            tc.name AS category,
            tc.icon AS category_icon,
            COALESCE(pj.parents_json, '[]') AS parents_json
        FROM tags t
        JOIN tag_categories tc ON tc.id = t.category_id
        LEFT JOIN v_tag_file_counts vfc ON vfc.tag_id = t.id
        LEFT JOIN (
            SELECT tag_id, COUNT(*) AS parent_count
            FROM tag_parents
            GROUP BY tag_id
        ) tp_count ON tp_count.tag_id = t.id
        LEFT JOIN (
            SELECT tp.tag_id, json_group_array(json_object(
                'id', pt.id,
                'name', pt.name,
                'category', ptc.name,
                'categoryIcon', ptc.icon
            )) AS parents_json
            FROM tag_parents tp
            JOIN tags pt ON pt.id = tp.parent_tag_id
            JOIN tag_categories ptc ON ptc.id = pt.category_id
            GROUP BY tp.tag_id
        ) pj ON pj.tag_id = t.id
        WHERE tc.name = 'Setlist'
        "#,
    );

    if search_pattern.is_some() {
        sql.push_str(" AND t.name LIKE ?");
    }

    if let Some(has_p) = has_parents {
        match has_p {
            "yes" => sql.push_str(" AND tp_count.parent_count > 0"),
            "no" => {
                sql.push_str(" AND (tp_count.parent_count = 0 OR tp_count.parent_count IS NULL)")
            }
            _ => {} // "any" or anything else → no filter
        }
    }

    // Sort: name | length | files | parents; default length DESC
    let sort_col = match sort {
        Some("name") => "t.name",
        Some("files") => "file_count",
        Some("parents") => "parent_count",
        _ => "LENGTH(t.name)", // "length" or default
    };
    let ord = match order {
        Some("asc") => "ASC",
        _ => "DESC", // default desc
    };
    sql.push_str(&format!(" ORDER BY {} {}", sort_col, ord));
    sql.push_str(" LIMIT ?");

    let mut q = sqlx::query_as::<_, CurationTag>(&sql);
    if let Some(ref pattern) = search_pattern {
        q = q.bind(pattern);
    }
    q = q.bind(limit);
    q.fetch_all(pool).await.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Get all tags associated with a file (via service track → playlist → tag matching)
pub async fn get_tags_for_file(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<Tag>> {
    // Get all tags for this file via v_file_tags view (resolves file→track→playlist→tag)
    let tags = sqlx::query_as::<_, Tag>(
        r#"
        SELECT DISTINCT ft.tag_id as id, ft.tag_name as name, ft.category_id, ft.sort_order, ft.created_at
        FROM v_file_tags ft
        WHERE ft.file_id = ?
        ORDER BY ft.tag_name
        "#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    Ok(tags)
}

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

/// Rich detail view for a single file: Traktor metadata + linked Spotify track
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
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailFile {
    pub id: i64,
    pub file_path: String,
    pub file_type: String,
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

/// Fetch rich detail for a single file: Traktor metadata + ALL linked tracks
/// (via v_file_track_link) with audio features + tags + playlists.
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

    // 3. Fetch tags via v_file_resolved_tags
    let tags: Vec<FileDetailTag> = sqlx::query_as(
        r#"
        SELECT DISTINCT rt.tag_id as id, rt.tag_name as name,
               rt.category_name, rt.prefix
        FROM v_file_resolved_tags rt
        WHERE rt.file_id = ?
        ORDER BY rt.category_name, rt.tag_name
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

/// Fetch rich detail for a single service track: track metadata + audio features
/// + ALL linked files (via v_file_track_link) + tags + playlists.
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
    let files: Vec<TrackDetailFile> = sqlx::query_as(
        r#"
        SELECT f.id, f.file_path, f.file_type, f.file_size, f.isrc,
               f.title, f.artist, f.album, f.bpm, f.musical_key,
               f.duration_ms, f.bitrate, f.sample_rate, f.channels,
               f.comment, f.rating, f.play_count, f.last_played,
               COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
               fl_backup.path as backup_path
        FROM v_file_track_link v
        JOIN files f ON f.id = v.file_id
        LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
        WHERE v.track_id = ?
        ORDER BY f.file_path
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    // 3. Fetch tags for this track (via playlist→tag→v_resolved_tags chain)
    let tags: Vec<FileDetailTag> = sqlx::query_as(
        r#"
        SELECT DISTINCT rt.tag_id as id, rt.tag_name as name,
               rt.category_name, rt.prefix
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
        JOIN v_resolved_tags rt ON rt.source_tag_id = t.id
        WHERE spt.track_id = ?
        ORDER BY rt.category_name, rt.tag_name
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
    }))
}

/// Compare Traktor vs Spotify BPM/Key for files linked to Spotify tracks.
///
/// Filters by tag name (resolved via v_file_resolved_tags) and returns
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
            JOIN v_file_resolved_tags vft ON vft.file_id = f.id AND vft.tag_name = ?
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

/// Find tracks with semantically similar tags.
///
/// Returns Vec of (file_id, title, artist, bpm, key, similarity_score, matched_tags_json)
/// where matched_tags_json is a JSON array of {seed_tag, matched_tag, similarity}.
///
/// Algorithm:
/// 1. Get tags for the seed file
/// 2. For each seed tag, find top-8 similar tags from tag_similarities (similarity > 0.5)
/// 3. Find all files that have any of those similar tags
/// 4. Score each file by aggregating similarity matches, normalized by seed tag count
/// 5. Return top-N scored files
#[allow(clippy::type_complexity)]
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
    let mut similar_tag_map: std::collections::HashMap<i64, Vec<(i64, f32)>> =
        std::collections::HashMap::new();

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
    let seed_tag_name_map: std::collections::HashMap<i64, &str> =
        seed_tags.iter().map(|t| (t.id, t.name.as_str())).collect();

    // Build a map of tag_id -> (tag_name, category_name) for candidate tags
    let mut candidate_tag_info: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();

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
               ft.tag_name, ft.tag_id
        FROM files f
        JOIN v_file_tags ft ON ft.file_id = f.id
        WHERE ft.tag_id IN ({})
          AND f.id != ?
        ORDER BY f.id
        "#,
        tag_placeholders.join(",")
    );

    let mut file_scores: std::collections::HashMap<
        i64,
        (
            String,
            Option<String>,
            Option<f64>,
            Option<String>,
            f64,
            Vec<(String, String, f32)>,
        ),
    > = std::collections::HashMap::new();

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

// ============================================================================
// Global Poller Helpers
// ============================================================================

/// Get all Spotify playlists with their DB id, playlist_id, and snapshot_id
/// for comparison against the Spotify API response.
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

/// Mark a playlist as deleted from Spotify (set snapshot_id to NULL so the
/// global poller knows this playlist no longer exists on the service).
pub async fn mark_playlist_inactive(pool: &Pool<Sqlite>, db_id: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE service_playlists SET snapshot_id = NULL, updated_at = ?1 WHERE id = ?2")
        .bind(now)
        .bind(db_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ============================================================================
// File Lifecycle Functions
// ============================================================================

// ─── Tag following ──────────────────────────────────────────────────────────

/// Set whether a tag is "followed" — files with this tag are kept locally
pub async fn set_tag_followed(pool: &Pool<Sqlite>, tag_id: i64, followed: bool) -> Result<()> {
    sqlx::query("UPDATE tags SET followed = ? WHERE id = ?")
        .bind(followed)
        .bind(tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all followed tags
pub async fn get_followed_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    let tags = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE followed = 1")
        .fetch_all(pool)
        .await?;
    Ok(tags)
}

/// Find the "backpack" tag (a Setlist tag named "backpack")
pub async fn get_backpack_tag(pool: &Pool<Sqlite>) -> Result<Option<Tag>> {
    let tag = sqlx::query_as::<_, Tag>(
        "SELECT t.* FROM tags t JOIN tag_categories tc ON t.category_id = tc.id WHERE LOWER(t.name) = 'backpack' AND tc.name = 'Setlist'"
    )
    .fetch_optional(pool)
    .await?;
    Ok(tag)
}

/// Ensure the "backpack" tag exists, create it if missing, then return it
pub async fn ensure_backpack_tag(pool: &Pool<Sqlite>) -> Result<Tag> {
    if let Some(tag) = get_backpack_tag(pool).await? {
        return Ok(tag);
    }
    // Find Setlist category
    let cat_id: i64 = sqlx::query_scalar("SELECT id FROM tag_categories WHERE name = 'Setlist'")
        .fetch_one(pool)
        .await?;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO tags (name, category_id, created_at, followed) VALUES ('backpack', ?, ?, 1)",
    )
    .bind(cat_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(get_backpack_tag(pool).await?.unwrap())
}

/// Check if a file has ANY followed tag
pub async fn is_file_followed(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT vft.tag_id) FROM v_file_tags vft
         JOIN tags t ON t.id = vft.tag_id
         WHERE vft.file_id = ? AND t.followed = 1",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

// ─── File locations ─────────────────────────────────────────────────────────

/// Record or update a file's location (local or backup)
pub async fn set_file_location(
    pool: &Pool<Sqlite>,
    file_id: i64,
    location_type: &str,
    path: &str,
    file_size: i64,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(file_id, location_type) DO UPDATE SET
            path = excluded.path,
            file_size = excluded.file_size,
            last_verified = excluded.last_verified"
    )
    .bind(file_id)
    .bind(location_type)
    .bind(path)
    .bind(file_size)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a file location entry (e.g. after local file deletion)
pub async fn remove_file_location(
    pool: &Pool<Sqlite>,
    file_id: i64,
    location_type: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM file_locations WHERE file_id = ? AND location_type = ?")
        .bind(file_id)
        .bind(location_type)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all locations for a file
pub async fn get_file_locations(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<FileLocation>> {
    let locations = sqlx::query_as::<_, FileLocation>(
        "SELECT * FROM file_locations WHERE file_id = ? ORDER BY location_type",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    Ok(locations)
}

/// Get files in a folder that have no backup location recorded
pub async fn get_unbacked_up_files(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<File>> {
    let files = sqlx::query_as::<_, File>(
        "SELECT f.* FROM files f
         JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
         WHERE fol.id = ?
           AND f.id NOT IN (
               SELECT file_id FROM file_locations WHERE location_type = 'backup'
           )
         ORDER BY f.file_path",
    )
    .bind(folder_id)
    .fetch_all(pool)
    .await?;
    Ok(files)
}

/// Record a successful backup result
pub async fn record_backup_result(
    pool: &Pool<Sqlite>,
    file_id: i64,
    success: bool,
    file_size: i64,
    backup_path: &str,
) -> Result<()> {
    if success {
        set_file_location(pool, file_id, "backup", backup_path, file_size).await?;
    }
    Ok(())
}

/// Clear all backup locations for files in a folder (for re-backup)
pub async fn clear_backup_status(pool: &Pool<Sqlite>, folder_id: i64) -> Result<()> {
    sqlx::query(
        "DELETE FROM file_locations WHERE location_type = 'backup' AND file_id IN (
            SELECT f.id FROM files f
            JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
            WHERE fol.id = ?
        )",
    )
    .bind(folder_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ─── Source-of (WAV \u2192 stem linking) ────────────────────────────────────

/// Get subdirectory names under the stems folder (WAV source dirs)
pub async fn get_wav_source_subdirs(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<String>> {
    // Get the folder path, then scan for subdirectories with WAV files
    let folder_path: String = sqlx::query_scalar("SELECT folder_path FROM folders WHERE id = ?")
        .bind(folder_id)
        .fetch_one(pool)
        .await?;

    let mut subdirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&folder_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Check if it contains .wav files
                if let Ok(dir_entries) = std::fs::read_dir(&path) {
                    let has_wav = dir_entries.flatten().any(|e| {
                        e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| ext.eq_ignore_ascii_case("wav"))
                            .unwrap_or(false)
                    });
                    if has_wav {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            subdirs.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(subdirs)
}

/// Link a WAV source file to its parent stem file
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

// ─── Pruning ─────────────────────────────────────────────────────────────────

/// Get all files eligible for local deletion.
/// A file is prune-able if: it has a backup location recorded AND
/// it is NOT a stem AND it is NOT in any followed tag (or has no tags at all).
pub async fn get_prune_candidates(
    pool: &Pool<Sqlite>,
    stem_preferred: bool,
) -> Result<Vec<PruneCandidate>> {
    // Two-step approach to avoid the expensive v_file_tags view:
    // 1. Get all backed-up non-WAV file IDs (fast, uses indexes)
    // 2. Get all file IDs with followed tags (simple query)
    // 3. Subtract in Rust, then fetch details for remaining candidates

    // Step 1: backed-up file IDs (fast — file_locations has indexes)
    let mut backed_up: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT fl.file_id FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE fl.location_type = 'backup' AND f.file_type != 'wav'",
    )
    .fetch_all(pool)
    .await?;

    if backed_up.is_empty() {
        return Ok(vec![]);
    }

    // Stem preference filter: exclude FLACs/MP3s/WAVs that have a same-ISRC stem.m4a
    if stem_preferred {
        let backed_up_set: std::collections::HashSet<i64> = backed_up.into_iter().collect();
        let placeholders: Vec<String> = backed_up_set.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT f.id FROM files f
             WHERE f.id IN ({})
             AND f.file_type != 'stem.m4a'
             AND EXISTS (
                 SELECT 1 FROM files f2
                 WHERE f2.isrc = f.isrc AND f2.isrc IS NOT NULL
                 AND f2.file_type = 'stem.m4a'
             )",
            placeholders.join(",")
        );
        let mut q = sqlx::query_scalar(&sql);
        for id in &backed_up_set {
            q = q.bind(id);
        }
        let redundant_ids: std::collections::HashSet<i64> =
            q.fetch_all(pool).await?.into_iter().collect();
        let filtered: Vec<i64> = backed_up_set.difference(&redundant_ids).copied().collect();
        backed_up = filtered;
        if backed_up.is_empty() {
            return Ok(vec![]);
        }
    }

    // Step 2: file IDs with any followed tag (simple EXISTS query)
    let followed: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT vft.file_id FROM v_file_tags vft
         JOIN tags t ON t.id = vft.tag_id
         WHERE t.followed = 1",
    )
    .fetch_all(pool)
    .await?;

    // Build HashSet for fast lookup
    let followed_set: std::collections::HashSet<i64> = followed.into_iter().collect();

    // Step 3: filter in Rust — candidates = backed_up minus followed
    let candidate_ids: Vec<i64> = backed_up
        .into_iter()
        .filter(|id| !followed_set.contains(id))
        .collect();

    if candidate_ids.is_empty() {
        return Ok(vec![]);
    }

    // Step 4: fetch full file details for candidates (limit to avoid over-fetching)
    // Build IN clause dynamically — SQLx doesn't support arrays, so use placeholders
    let placeholders: Vec<String> = candidate_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size,
                COALESCE(f.title, '') as title, COALESCE(f.artist, '') as artist,
                f.isrc, fl.path as backup_path
         FROM files f
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
         WHERE f.id IN ({})
         ORDER BY f.file_type, f.file_path",
        placeholders.join(",")
    );

    let mut query = sqlx::query(&sql);
    for id in &candidate_ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;

    let mut candidates = Vec::new();
    for row in rows {
        let reason = "not_followed".to_string();
        candidates.push(PruneCandidate {
            file_id: row.try_get("id")?,
            file_path: row.try_get("file_path")?,
            file_type: row.try_get("file_type")?,
            file_size: row.try_get("file_size")?,
            title: row.try_get("title")?,
            artist: row.try_get("artist")?,
            isrc: row.try_get("isrc")?,
            reason,
            backup_path: row.try_get("backup_path")?,
        });
    }

    Ok(candidates)
}

/// Delete a local file and remove its 'local' file_location entry.
/// Returns true if the file was actually deleted from disk.
pub async fn delete_local_file_by_id(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool> {
    // Get file path
    let file_path: Option<String> = sqlx::query_scalar("SELECT file_path FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_optional(pool)
        .await?;

    if let Some(path) = file_path {
        let path_ref = std::path::Path::new(&path);
        if path_ref.exists() {
            std::fs::remove_file(path_ref)?;
            tracing::info!("Deleted local file: {}", path);
        }
        // Remove local location record
        sqlx::query("DELETE FROM file_locations WHERE file_id = ? AND location_type = 'local'")
            .bind(file_id)
            .execute(pool)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ─── Storage status ─────────────────────────────────────────────────────────

/// Get aggregate storage statistics
pub async fn get_storage_status(pool: &Pool<Sqlite>) -> Result<StorageStatus> {
    let local_file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let local_size_bytes: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(file_size), 0) FROM files")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let local_stems: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_type = 'stem.m4a'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let local_flacs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_type = 'flac'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let local_mp3s: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_type = 'mp3'")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let local_wavs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_type = 'wav'")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let local_other: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_type NOT IN ('stem.m4a','flac','mp3','wav')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_stems_size: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(file_size), 0) FROM files WHERE file_type = 'stem.m4a'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_flacs_size: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(file_size), 0) FROM files WHERE file_type = 'flac'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_wavs_size: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(file_size), 0) FROM files WHERE file_type = 'wav'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let local_mp3s_size: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(file_size), 0) FROM files WHERE file_type = 'mp3'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let backup_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT file_id) FROM file_locations WHERE location_type = 'backup'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let wav_source_dirs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_type = 'wav' AND source_of IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Prune candidates count & size
    // Default to stem_preferred=false for status count — frontend toggles the setting
    let candidates = get_prune_candidates(pool, false).await?;
    let prune_candidate_count = candidates.len() as i64;
    let prune_candidate_bytes = candidates.iter().map(|c| c.file_size).sum();

    let wav_indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_type = 'wav'")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let wav_backed_up: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fl.file_id) FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE f.file_type = 'wav' AND fl.location_type = 'backup'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(StorageStatus {
        local_file_count,
        local_size_bytes,
        local_stems,
        local_flacs,
        local_mp3s,
        local_wavs,
        local_other,
        local_stems_size,
        local_flacs_size,
        local_wavs_size,
        local_mp3s_size,
        backup_count,
        wav_source_dirs,
        prune_candidate_count,
        prune_candidate_bytes,
        wav_indexed,
        wav_backed_up,
    })
}

// ─── Folder backup config ───────────────────────────────────────────────────

/// Update a folder's backup path and/or scan_sources flag
pub async fn update_folder_backup_config(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    backup_path: Option<&str>,
    scan_sources: Option<bool>,
) -> Result<()> {
    if let Some(bp) = backup_path {
        sqlx::query("UPDATE folders SET backup_path = ? WHERE id = ?")
            .bind(bp)
            .bind(folder_id)
            .execute(pool)
            .await?;
    }
    if let Some(ss) = scan_sources {
        sqlx::query("UPDATE folders SET scan_sources = ? WHERE id = ?")
            .bind(ss)
            .bind(folder_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ============================================================================
// Folder Stats
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderStats {
    pub id: i64,
    pub folder_path: String,
    pub backup_path: Option<String>,
    pub scan_sources: bool,
    /// Total files in this folder
    pub total_files: i64,
    /// Total size of all files in this folder (bytes)
    pub total_size_bytes: i64,
    /// File counts by type
    pub stems: i64,
    pub flacs: i64,
    pub wavs: i64,
    pub mp3s: i64,
    pub other: i64,
    /// Number of files backed up (have a backup file_locations entry)
    pub backed_up: i64,
    /// Total size of backed up files (bytes)
    pub backed_up_size_bytes: i64,
    /// Number of WAV source subdirectories found
    pub wav_source_dirs: i64,
    /// Number of WAV files indexed from sources
    pub wav_source_files: i64,
    /// Number of WAV files that are backed up
    pub wav_backed_up: i64,
    /// When the folder was last scanned
    pub last_scanned: Option<i64>,
    /// Whether folder watching is active
    pub watch_enabled: bool,
    /// Scan config
    pub scan_recursive: bool,
    pub max_depth: i32,
}

pub async fn get_folder_stats(pool: &Pool<Sqlite>, folder_id: i64) -> Result<FolderStats> {
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found"))?;

    let folder_path_prefix = format!("{}%", folder.folder_path);

    // Total files
    let total_files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_path LIKE ?")
        .bind(&folder_path_prefix)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    // Total size
    let total_size_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(file_size), 0) FROM files WHERE file_path LIKE ?")
            .bind(&folder_path_prefix)
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    // By type
    let stems: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'stem.m4a'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let flacs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'flac'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let wavs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'wav'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let mp3s: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'mp3'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let other: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type NOT IN ('stem.m4a','flac','wav','mp3')"
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Backed up files (have backup location)
    let backed_up: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fl.file_id) FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE f.file_path LIKE ? AND fl.location_type = 'backup'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Backed up size
    let backed_up_size_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f
         JOIN file_locations fl ON fl.file_id = f.id
         WHERE f.file_path LIKE ? AND fl.location_type = 'backup'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // WAV source dirs: count subdirs that exist on filesystem
    let wav_source_dirs = if folder.scan_sources {
        let subdirs = get_wav_source_subdirs(pool, folder_id)
            .await
            .unwrap_or_default();
        let mut count = 0i64;
        for subdir in &subdirs {
            let full_path = format!("{}/{}", folder.folder_path, subdir);
            let path = std::path::Path::new(&full_path);
            if path.is_dir() {
                count += 1;
            }
        }
        count
    } else {
        0
    };

    let wav_source_files: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'wav'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let wav_backed_up: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fl.file_id) FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE f.file_path LIKE ? AND f.file_type = 'wav' AND fl.location_type = 'backup'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(FolderStats {
        id: folder.id,
        folder_path: folder.folder_path,
        backup_path: folder.backup_path,
        scan_sources: folder.scan_sources,
        total_files,
        total_size_bytes,
        stems,
        flacs,
        wavs,
        mp3s,
        other,
        backed_up,
        backed_up_size_bytes,
        wav_source_dirs,
        wav_source_files,
        wav_backed_up,
        last_scanned: folder.last_scanned,
        watch_enabled: folder.active,
        scan_recursive: folder.scan_recursive,
        max_depth: folder.max_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn test_extract_bpm_from_stem_m4a() {
        // Test BPM extraction from stem.m4a file
        let test_path = Path::new("/Users/momo/Music/stems/Cloonee - Sippin' Yak.stem.m4a");

        // Skip test if file doesn't exist
        if !test_path.exists() {
            println!("Test file not found, skipping test");
            return;
        }

        // Test exiftool extraction directly
        match extract_mp4_metadata_with_exiftool(test_path) {
            Ok((_title, _artist, _album, comment, bpm, key)) => {
                println!(
                    "exiftool results: comment={:?}, bpm={:?}, key={:?}",
                    comment, bpm, key
                );
                assert_eq!(bpm, Some(126.0), "BPM should be 126");
                assert_eq!(key, Some("6m".to_string()), "Key should be 6m");
            }
            Err(e) => panic!("exiftool extraction failed: {}", e),
        }

        // Test full metadata extraction
        match extract_audio_metadata_from_file(test_path).await {
            Ok(file) => {
                println!(
                    "Full extraction results: bpm={:?}, key={:?}",
                    file.bpm, file.musical_key
                );
                assert_eq!(file.bpm, Some(126.0), "File BPM should be 126");
                assert_eq!(
                    file.musical_key,
                    Some("6m".to_string()),
                    "File key should be 6m"
                );
            }
            Err(e) => panic!("Full metadata extraction failed: {}", e),
        }
    }

    #[test]
    fn test_parse_bpm_numeric() {
        // Test that parse_bpm handles numeric strings
        assert_eq!(parse_bpm("126"), Some(126.0));
        assert_eq!(parse_bpm("128.5"), Some(128.5));
        assert_eq!(parse_bpm("invalid"), None);
    }
}
