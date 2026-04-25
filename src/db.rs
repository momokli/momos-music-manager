use std::{fs, path::Path, time::SystemTime};

use anyhow::{Result, anyhow};
use chrono;
use lofty::{prelude::*, read_from_path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::audio_extensions::AudioExtension;
use sqlx::{FromRow, Pool, Row, Sqlite, SqliteConnection, SqlitePool};
use tracing::{debug, info, warn};

// ============================================================================
// Database Models (8-table schema)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TagCategory {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub prefix: String,
    pub sort_order: i32,
    pub is_default: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub category_id: i64,
    pub created_at: i64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServicePlaylistTrack {
    pub playlist_id: i64,
    pub track_id: i64,
    pub position: Option<i32>,
    pub added_at: i64,
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
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// Database Connection
// ============================================================================

pub async fn connect_db() -> Result<SqlitePool> {
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:app.db".to_string());
    let pool = SqlitePool::connect(&database_url).await?;
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

pub async fn extract_audio_metadata_from_file(path: &Path) -> Result<File> {
    // Get file metadata
    let metadata = fs::metadata(path)?;
    let file_size = metadata.len() as i64;
    let last_modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Calculate file hash
    let file_hash = calculate_file_hash(path)?;

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

    Ok(File {
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
        play_count: 0,
        last_played: None,
        spotify_id: None,
        soundcloud_id: None,
        youtube_id: None,
        created_at: now,
        updated_at: now,
    })
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
    .bind(&file.last_played)
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
    scan_directory_with_config(pool, dir_path, true, false, String::new(), 0).await
}

pub async fn scan_directory_with_config(
    pool: &Pool<Sqlite>,
    dir_path: &Path,
    scan_recursive: bool,
    fixed_extensions: bool,
    file_extensions: String,
    max_depth: i32,
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
        sqlx::query_as::<_, TagCategory>("SELECT * FROM tag_categories ORDER BY sort_order")
            .fetch_all(pool)
            .await?;
    Ok(categories)
}

/// Get a single tag category by ID
pub async fn get_tag_category_by_id(
    pool: &Pool<Sqlite>,
    category_id: i64,
) -> Result<Option<TagCategory>> {
    let category = sqlx::query_as::<_, TagCategory>("SELECT * FROM tag_categories WHERE id = ?")
        .bind(category_id)
        .fetch_optional(pool)
        .await?;
    Ok(category)
}

pub async fn get_default_tag_category(pool: &Pool<Sqlite>) -> Result<Option<TagCategory>> {
    let category =
        sqlx::query_as::<_, TagCategory>("SELECT * FROM tag_categories WHERE is_default = TRUE")
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

/// Get playlists that don't have corresponding tags (case-insensitive name matching)
pub async fn get_playlists_without_tags(pool: &Pool<Sqlite>) -> Result<Vec<ServicePlaylist>> {
    let playlists = sqlx::query_as::<_, ServicePlaylist>(
        r#"
        SELECT DISTINCT sp.*
        FROM service_playlists sp
        WHERE TRIM(sp.name) != ''
          AND NOT EXISTS (
            SELECT 1 FROM tags t
            WHERE LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
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
            SELECT 1 FROM tags t
            WHERE LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
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
    let category = sqlx::query_as::<_, TagCategory>(
        r#"
        INSERT INTO tag_categories (name, prefix, icon, is_default, sort_order, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(name)
    .bind(prefix)
    .bind(icon)
    .bind(is_default)
    .bind(sort_order)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(category)
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
        "UPDATE tag_categories SET {} WHERE id = ? RETURNING *",
        updates.join(", ")
    );

    let mut db_query = sqlx::query_as::<_, TagCategory>(&query_str);
    for param in params {
        db_query = db_query.bind(param);
    }
    db_query = db_query.bind(category_id);

    let category = db_query.fetch_one(pool).await?;
    Ok(category)
}

pub async fn delete_tag_category(pool: &Pool<Sqlite>, category_id: i64) -> Result<()> {
    // Check if category is in use
    let count = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE category_id = ?")
        .bind(category_id)
        .fetch_one(pool)
        .await?;

    let count_val: i64 = match count {
        Some(val) => val,
        None => 0,
    };

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
    if let (Some(true), Some(extensions)) = (fixed_extensions, file_extensions) {
        if !extensions.trim().is_empty() {
            crate::audio_extensions::AudioExtension::parse_list(extensions)
                .map_err(|e| anyhow!("Invalid file extensions: {}", e))?;
        }
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
        return Ok(folder);
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
        return Ok(folder);
    } else {
        // Nothing to update
        if let Some(folder) = get_folder_by_id(pool, id).await? {
            return Ok(folder);
        } else {
            return Err(anyhow!("Folder not found with id: {}", id));
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
pub async fn scan_folder(pool: &Pool<Sqlite>, folder_id: i64) -> Result<usize> {
    // Get folder path
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found with id: {}", folder_id))?;

    let path = std::path::Path::new(&folder.folder_path);
    let file_count = scan_directory_with_config(
        pool,
        path,
        folder.scan_recursive,
        folder.fixed_extensions,
        folder.file_extensions,
        folder.max_depth,
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

/// Write comment to file using exiftool
pub async fn write_comment_to_file(file_path: &str, comment: &str) -> Result<()> {
    use std::process::Command;

    let output = Command::new("exiftool")
        .arg("-overwrite_original")
        .arg(&format!("-comment={}", comment))
        .arg(file_path)
        .output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Failed to write comment: {}", error));
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

    // Step 1: Find matching service tracks
    let matching_tracks_sql = r#"
        SELECT st.id
        FROM service_tracks st
        WHERE st.isrc = ?
           OR (st.service = 'spotify' AND st.service_id = ?)
           OR (st.service = 'soundcloud' AND st.service_id = ?)
           OR (st.service = 'youtube' AND st.service_id = ?)
    "#;

    let track_ids: Vec<i64> = sqlx::query_scalar::<_, i64>(matching_tracks_sql)
        .bind(&file.isrc)
        .bind(&file.spotify_id)
        .bind(&file.soundcloud_id)
        .bind(&file.youtube_id)
        .fetch_all(pool)
        .await?;

    if track_ids.is_empty() {
        // No matching tracks, just return comment with service IDs
        return Ok(generate_target_comment(
            '_',
            '_',
            '_',
            &[],
            file.spotify_id.as_deref(),
            file.soundcloud_id.as_deref(),
            file.youtube_id.as_deref(),
        ));
    }

    // Step 2: Find playlists those tracks are in
    let placeholders: Vec<String> = track_ids.iter().map(|_| "?".to_string()).collect();
    let playlists_sql = format!(
        "SELECT DISTINCT sp.name
         FROM service_playlists sp
         JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
         WHERE spt.track_id IN ({})",
        placeholders.join(", ")
    );

    let mut playlist_query = sqlx::query_scalar::<_, String>(&playlists_sql);
    for track_id in &track_ids {
        playlist_query = playlist_query.bind(track_id);
    }

    let playlist_names: Vec<String> = playlist_query.fetch_all(pool).await?;

    if playlist_names.is_empty() {
        // No playlists found, just return comment with service IDs
        return Ok(generate_target_comment(
            '_',
            '_',
            '_',
            &[],
            file.spotify_id.as_deref(),
            file.soundcloud_id.as_deref(),
            file.youtube_id.as_deref(),
        ));
    }

    // Step 3: Find tags matching playlist names with categories
    let tag_placeholders: Vec<String> = playlist_names.iter().map(|_| "?".to_string()).collect();
    let tags_sql = format!(
        "SELECT t.name, tc.prefix, tc.sort_order
         FROM tags t
         JOIN tag_categories tc ON tc.id = t.category_id
         WHERE t.name IN ({})
         ORDER BY tc.sort_order, t.name",
        tag_placeholders.join(", ")
    );

    let mut tag_query = sqlx::query(&tags_sql);
    for playlist_name in &playlist_names {
        tag_query = tag_query.bind(playlist_name);
    }

    let tag_rows = tag_query.fetch_all(pool).await?;

    // Step 4: Determine PMV characters and collect tags
    let mut phase_present = false;
    let mut mood_present = false;
    let mut vibe_present = false;
    let mut tags: Vec<String> = Vec::new();

    for row in tag_rows {
        let tag_name: String = row.try_get("name")?;
        let prefix: String = row.try_get("prefix")?;

        match prefix.as_str() {
            "P" => phase_present = true,
            "M" => mood_present = true,
            "V" => vibe_present = true,
            _ => {}
        }

        tags.push(tag_name);
    }

    // Step 5: Tags already sorted by sort_order then name from SQL query

    // Step 6 & 7: Generate target comment
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
    // Find playlists that contain this track, then find tags with matching names
    let tags = sqlx::query_as::<_, Tag>(
        r#"
        SELECT DISTINCT t.*
        FROM tags t
        INNER JOIN service_playlists sp ON t.name = sp.name COLLATE NOCASE
        INNER JOIN service_playlist_tracks spt ON sp.id = spt.playlist_id
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

/// Create or update a service track in the database
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
    let pos = position.unwrap_or(0);
    let added_at = chrono::Utc::now().timestamp();

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
        VALUES (?, ?, ?, ?)
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
