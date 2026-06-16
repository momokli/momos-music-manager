#![allow(dead_code)]

//! Scan cache for development — records and replays file metadata extraction
//!
//! Controlled by `SCAN_CACHE` env var:
//! - unset / `off`  → live extraction (normal operation)
//! - `record`        → full extraction + save to `dev-data/scan-cache/`
//! - `replay`        → load cached metadata, zero lofty/exiftool calls
//!
//! Each file's metadata is cached individually at:
//!   `dev-data/scan-cache/entries/{HASHED_PATH}.json`
//!
//! Cache entries are invalidated when the file's `last_modified` or `file_hash`
//! changes — so new mixes, BPM re-analyses, etc. automatically get re-extracted.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::db::File;

// ── Cache mode ─────────────────────────────────────────────────────────────────

/// How to source file metadata during folder scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanCacheMode {
    /// Real extraction via lofty + exiftool (default).
    Live,
    /// Real extraction + persist results to disk.
    Record,
    /// Load persisted results from disk, no extraction I/O.
    Replay,
}

impl ScanCacheMode {
    /// Read from the `SCAN_CACHE` env var.
    ///
    /// | Value    | Mode     |
    /// |----------|----------|
    /// | unset    | `Live`   |
    /// | `off`    | `Live`   |
    /// | `record` | `Record` |
    /// | `replay` | `Replay` |
    pub fn from_env() -> Self {
        match std::env::var("SCAN_CACHE").ok().as_deref() {
            Some("record") => {
                info!("SCAN_CACHE=record — recording file metadata");
                ScanCacheMode::Record
            }
            Some("replay") => {
                info!("SCAN_CACHE=replay — replaying cached file metadata");
                ScanCacheMode::Replay
            }
            _ => ScanCacheMode::Live,
        }
    }

    /// Returns `true` when the cache should be written.
    pub fn should_record(self) -> bool {
        self == ScanCacheMode::Record
    }

    /// Returns `true` when extraction should be skipped in favour of cache.
    pub fn should_replay(self) -> bool {
        self == ScanCacheMode::Replay
    }
}

// ── Cache entry ────────────────────────────────────────────────────────────────

/// A single file's cached metadata, keyed by file path + invalidation tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFileEntry {
    /// Absolute path at time of caching.
    pub file_path: String,
    /// `last_modified` at time of caching (used for invalidation).
    pub last_modified: i64,
    /// `file_hash` at time of caching (secondary invalidation).
    pub file_hash: String,
    /// The full metadata that was extracted.
    pub metadata: File,
}

// ── Cache directory helpers ─────────────────────────────────────────────────────

const DEFAULT_CACHE_DIR: &str = "./dev-data/scan-cache";

fn cache_root() -> PathBuf {
    std::env::var("SCAN_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR))
}

fn entries_dir() -> PathBuf {
    cache_root().join("entries")
}

/// Deterministic filename-safe hash of a file path.
fn path_hash(file_path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    file_path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn entry_path(file_path: &str) -> PathBuf {
    entries_dir().join(format!("{}.json", path_hash(file_path)))
}

// ── Cache I/O ───────────────────────────────────────────────────────────────────

/// Load a cached entry for `file_path`.
///
/// Returns `None` when the cache file doesn't exist or fails to parse.
async fn load_entry(file_path: &str) -> Option<CachedFileEntry> {
    let path = entry_path(file_path);
    if !path.exists() {
        return None;
    }

    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match serde_json::from_str::<CachedFileEntry>(&json) {
            Ok(entry) => {
                debug!("Cache HIT  for {}", file_path);
                Some(entry)
            }
            Err(e) => {
                warn!("Cache corrupt for {} ({}), re-extracting", file_path, e);
                None
            }
        },
        Err(e) => {
            debug!("Cache MISS for {}: {}", file_path, e);
            None
        }
    }
}

/// Save a new cache entry for `file_path`.
async fn save_entry(entry: &CachedFileEntry) -> Result<()> {
    let dir = entries_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .context("Failed to create scan-cache entries directory")?;

    let path = entry_path(&entry.file_path);
    let json =
        serde_json::to_string_pretty(entry).context("Failed to serialise cached file entry")?;
    tokio::fs::write(&path, &json)
        .await
        .context(format!("Failed to write {}", path.display()))?;

    debug!("Cached {}", entry.file_path);
    Ok(())
}

// ── Public API ──────────────────────────────────────────────────────────────────

/// Cached result of a metadata extraction.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum CacheResult {
    /// Cache hit — metadata was loaded from cache (no extraction needed).
    /// Contains the deserialised [`File`].
    Hit(File),

    /// Cache miss — caller should perform real extraction.
    Miss,
}

/// Attempt to load cached metadata for a file.
///
/// This is the main entry-point for the replay path. Call it **before**
/// calling `extract_audio_metadata_from_file`. If it returns `Hit`, use
/// the returned [`File`] directly (and skip extraction).
///
/// The cached `last_scanned` is updated to the current timestamp so the
/// DB record reflects when the replay happened.
pub async fn try_load(file_path: &str, last_modified: i64, file_hash: &str) -> CacheResult {
    let mode = ScanCacheMode::from_env();
    if !mode.should_replay() {
        return CacheResult::Miss;
    }

    let entry = match load_entry(file_path).await {
        Some(e) => e,
        None => return CacheResult::Miss,
    };

    // Invalidate if the file has changed since caching
    if entry.last_modified != last_modified || entry.file_hash != file_hash {
        debug!(
            "Cache INVALID for {} (mtime changed {}=>{}, or hash changed)",
            file_path, entry.last_modified, last_modified
        );
        return CacheResult::Miss;
    }

    let mut file = entry.metadata;

    // Stamp with current scan time
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    file.last_scanned = now;

    CacheResult::Hit(file)
}

/// Save extracted metadata to the cache.
///
/// Call this **after** a successful `extract_audio_metadata_from_file` to
/// persist the result for future replay sessions. Only actually writes to
/// disk when `SCAN_CACHE=record`.
pub async fn try_save(file: &File) {
    let mode = ScanCacheMode::from_env();
    if !mode.should_record() {
        return;
    }

    let entry = CachedFileEntry {
        file_path: file.file_path.clone(),
        last_modified: file.last_modified,
        file_hash: file.file_hash.clone(),
        metadata: file.clone(),
    };

    if let Err(e) = save_entry(&entry).await {
        warn!("Failed to cache {}: {:?}", file.file_path, e);
    }
}

/// Remove a single file from the cache (e.g. after re-extraction in record mode).
pub async fn invalidate(file_path: &str) {
    let path = entry_path(file_path);
    if path.exists()
        && let Err(e) = tokio::fs::remove_file(&path).await
    {
        warn!("Failed to invalidate cache for {}: {:?}", file_path, e);
    }
}

/// Delete **all** cached scan entries.
pub async fn clear_cache() -> Result<()> {
    let dir = entries_dir();
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .context("Failed to clear scan-cache entries")?;
    }
    tokio::fs::create_dir_all(&dir)
        .await
        .context("Failed to recreate scan-cache entries directory")?;
    info!("Cleared scan cache at {}", dir.display());
    Ok(())
}

/// Return the number of cached entries (for diagnostics).
pub async fn entry_count() -> usize {
    let dir = entries_dir();
    if !dir.exists() {
        return 0;
    }
    let mut count = 0;
    let readdir = tokio::fs::read_dir(&dir).await;
    if let Ok(mut rd) = readdir {
        while let Ok(Some(_)) = rd.next_entry().await {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // ── ScanCacheMode tests ────────────────────────────────────────────

    #[test]
    fn test_scan_cache_mode_default_is_live() {
        unsafe { env::remove_var("SCAN_CACHE") };
        assert_eq!(ScanCacheMode::from_env(), ScanCacheMode::Live);
    }

    #[test]
    fn test_scan_cache_mode_record() {
        unsafe { env::set_var("SCAN_CACHE", "record") };
        let mode = ScanCacheMode::from_env();
        assert_eq!(mode, ScanCacheMode::Record);
        assert!(mode.should_record());
        assert!(!mode.should_replay());
        unsafe { env::remove_var("SCAN_CACHE") };
    }

    #[test]
    fn test_scan_cache_mode_replay() {
        unsafe { env::set_var("SCAN_CACHE", "replay") };
        let mode = ScanCacheMode::from_env();
        assert_eq!(mode, ScanCacheMode::Replay);
        assert!(!mode.should_record());
        assert!(mode.should_replay());
        unsafe { env::remove_var("SCAN_CACHE") };
    }

    #[test]
    fn test_scan_cache_mode_off_is_live() {
        unsafe { env::set_var("SCAN_CACHE", "off") };
        assert_eq!(ScanCacheMode::from_env(), ScanCacheMode::Live);
        unsafe { env::remove_var("SCAN_CACHE") };
    }

    #[test]
    fn test_scan_cache_mode_live_flags() {
        let mode = ScanCacheMode::Live;
        assert!(!mode.should_record());
        assert!(!mode.should_replay());
    }

    #[test]
    fn test_scan_cache_mode_clone_and_eq() {
        let a = ScanCacheMode::Record;
        let b = ScanCacheMode::Record;
        let c = ScanCacheMode::Live;
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // ── CacheResult tests ───────────────────────────────────────────────

    #[test]
    fn test_cache_result_hit_vs_miss() {
        // Verify CacheResult enum variants work as expected
        match CacheResult::Miss {
            CacheResult::Miss => {}
            _ => panic!("expected Miss"),
        }
        // Hit requires a File — create a minimal one
        let file = File {
            id: 0,
            file_path: String::new(),
            file_hash: String::new(),
            file_type: String::new(),
            file_size: 0,
            last_modified: 0,
            isrc: None,
            last_scanned: 0,
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
        folder_id: None,
            created_at: 0,
            updated_at: 0,
        };
        match CacheResult::Hit(file) {
            CacheResult::Hit(f) => {
                assert_eq!(f.id, 0);
                assert!(f.title.is_none());
            }
            _ => panic!("expected Hit"),
        }
    }

    // ── path_hash tests ────────────────────────────────────────────────

    #[test]
    fn test_path_hash_deterministic() {
        let h1 = path_hash("/music/track.flac");
        let h2 = path_hash("/music/track.flac");
        assert_eq!(h1, h2, "same input must produce same hash");
        assert_eq!(h1.len(), 16, "hash must be 16 hex chars");
    }

    #[test]
    fn test_path_hash_different_paths() {
        let h1 = path_hash("/music/track1.flac");
        let h2 = path_hash("/music/track2.flac");
        assert_ne!(h1, h2, "different paths must produce different hashes");
    }

    #[test]
    fn test_path_hash_empty_string() {
        // Empty string path should still produce a valid 16-char hex hash
        let h = path_hash("");
        assert_eq!(h.len(), 16, "empty path must produce 16-char hash");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "empty path hash must be hex"
        );
    }

    // ── cache_root tests ───────────────────────────────────────────────

    #[test]
    fn test_cache_root_default() {
        unsafe { env::remove_var("SCAN_CACHE_DIR") };
        let root = cache_root();
        assert_eq!(root, PathBuf::from("./dev-data/scan-cache"));
    }

    #[test]
    fn test_cache_root_from_env() {
        unsafe { env::set_var("SCAN_CACHE_DIR", "/tmp/test-scan-cache") };
        let root = cache_root();
        assert_eq!(root, PathBuf::from("/tmp/test-scan-cache"));
        unsafe { env::remove_var("SCAN_CACHE_DIR") };
    }

    // ── entry_path tests ───────────────────────────────────────────────

    #[test]
    fn test_entry_path_ends_with_json() {
        let p = entry_path("some_file.flac");
        assert!(p.to_string_lossy().ends_with(".json"));
    }

    #[test]
    fn test_entry_path_contains_hash() {
        let p = entry_path("/music/test.flac");
        let filename = p.file_name().unwrap().to_string_lossy();
        // filename format: HASH.json
        assert!(filename.len() > 5, "filename should be hash.json");
        assert!(filename.ends_with(".json"));
    }

    // ── CachedFileEntry construction ───────────────────────────────────

    #[test]
    fn test_cached_file_entry_fields() {
        let entry = CachedFileEntry {
            file_path: "/music/test.flac".to_string(),
            last_modified: 1700000000,
            file_hash: "abc123".to_string(),
            metadata: File {
                id: 0,
                file_path: "/music/test.flac".to_string(),
                file_hash: "abc".to_string(),
                file_type: "flac".to_string(),
                file_size: 12345,
                last_modified: 1700000000,
                isrc: None,
                last_scanned: 1700000000,
                title: Some("Test".to_string()),
                artist: Some("Artist".to_string()),
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
        folder_id: None,
                created_at: 1700000000,
                updated_at: 1700000000,
            },
        };

        assert_eq!(entry.file_path, "/music/test.flac");
        assert_eq!(entry.last_modified, 1700000000);
        assert_eq!(entry.file_hash, "abc123");
    }

    // ── Large path / special characters ─────────────────────────────

    #[test]
    fn test_path_hash_very_long_path() {
        // A path > 500 chars should still produce a valid 16-char hex hash
        let long_path = "/music/".to_string()
            + &"a".repeat(200)
            + "/"
            + &"b".repeat(200)
            + "/very_long_artist_name_-_very_long_track_title_remix_version.stem.m4a";
        let h = path_hash(&long_path);
        assert_eq!(h.len(), 16, "hash must be 16 hex chars even for long paths");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should only contain hex chars"
        );
    }

    #[test]
    fn test_path_hash_special_characters() {
        // Unicode, emoji, spaces, and symbols should all produce valid hashes
        let paths = [
            "/music/Español/Canción.flac",
            "/music/日本語/曲.flac",
            "/music/🎵/track.wav",
            "/music/artist - 'single' (2024) [WAV]/track.wav",
            "/music/symbols/!@#$%^&*().flac",
            "/music/  spaces  /  file  .flac",
            "/music/new\nline/test.flac",
        ];
        for p in &paths {
            let h = path_hash(p);
            assert_eq!(h.len(), 16, "hash must be 16 chars for path: {}", p);
            assert!(
                h.chars().all(|c| c.is_ascii_hexdigit()),
                "hash should only contain hex chars for path: {}",
                p
            );
        }
    }

    #[test]
    fn test_path_hash_deterministic_special_chars() {
        // Same special-char path always produces the same hash
        let path = "/music/Español/🎵/Canción - 'álbum'.flac";
        let h1 = path_hash(path);
        let h2 = path_hash(path);
        assert_eq!(h1, h2, "same path must produce same hash");
    }

    #[test]
    fn test_cached_file_entry_serde_roundtrip() {
        // Verify that a CachedFileEntry survives JSON serialization
        let entry = CachedFileEntry {
            file_path: "/music/Español/🎵/test.stem.m4a".to_string(),
            last_modified: 1700000000,
            file_hash: "a1b2c3d4e5f6a7b8".to_string(),
            metadata: File {
                id: 42,
                file_path: "/music/Español/🎵/test.stem.m4a".to_string(),
                file_hash: "deadbeef".to_string(),
                file_type: "stem.m4a".to_string(),
                file_size: 99999999,
                last_modified: 1700000000,
                isrc: Some("USABC1234567".to_string()),
                last_scanned: 1700000000,
                title: Some("Test Track".to_string()),
                artist: Some("Artista".to_string()),
                album: Some("Álbum".to_string()),
                album_artist: None,
                track_number: Some(1),
                total_tracks: Some(12),
                disc_number: None,
                total_discs: None,
                genre: Some("House".to_string()),
                year: Some(2024),
                composer: None,
                comment: Some("[PMV] groovy house".to_string()),
                duration_ms: Some(300000),
                bitrate: Some(1411),
                sample_rate: Some(44100),
                channels: Some(2),
                bpm: Some(128.0),
                musical_key: Some("4m".to_string()),
                rating: 3,
                play_count: 42,
                last_played: Some(1700000000),
                spotify_id: Some("spotify:track:abc".to_string()),
                soundcloud_id: None,
                youtube_id: None,
                source_of: None,
                stem_type: None,
                last_verified_local: Some(1700000000),
        folder_id: None,
                created_at: 1700000000,
                updated_at: 1700000000,
            },
        };

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&entry).expect("serialize");
        // Deserialize back
        let deserialized: CachedFileEntry = serde_json::from_str(&json).expect("deserialize");

        // Verify roundtrip
        assert_eq!(deserialized.file_path, entry.file_path);
        assert_eq!(deserialized.last_modified, entry.last_modified);
        assert_eq!(deserialized.file_hash, entry.file_hash);
        assert_eq!(deserialized.metadata.id, entry.metadata.id);
        assert_eq!(deserialized.metadata.title, entry.metadata.title);
        assert_eq!(deserialized.metadata.artist, entry.metadata.artist);
        assert_eq!(deserialized.metadata.isrc, entry.metadata.isrc);
        assert_eq!(deserialized.metadata.comment, entry.metadata.comment);
        assert_eq!(deserialized.metadata.bpm, entry.metadata.bpm);
        assert_eq!(
            deserialized.metadata.musical_key,
            entry.metadata.musical_key
        );
    }

    #[test]
    fn test_cached_file_entry_serde_null_fields() {
        // Verify that optional (None) fields survive roundtrip correctly
        let entry = CachedFileEntry {
            file_path: "/music/null-test.flac".to_string(),
            last_modified: 1700000000,
            file_hash: "hash123".to_string(),
            metadata: File {
                id: 0,
                file_path: "/music/null-test.flac".to_string(),
                file_hash: "hash123".to_string(),
                file_type: "flac".to_string(),
                file_size: 0,
                last_modified: 1700000000,
                isrc: None,
                last_scanned: 1700000000,
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
        folder_id: None,
                created_at: 0,
                updated_at: 0,
            },
        };

        let json = serde_json::to_string_pretty(&entry).expect("serialize");
        let deserialized: CachedFileEntry = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.file_path, "/music/null-test.flac");
        assert_eq!(deserialized.metadata.title, None);
        assert_eq!(deserialized.metadata.artist, None);
        assert_eq!(deserialized.metadata.isrc, None);
        assert_eq!(deserialized.metadata.bpm, None);
        assert_eq!(deserialized.metadata.spotify_id, None);
        assert_eq!(deserialized.metadata.rating, 0);
        assert_eq!(deserialized.metadata.play_count, 0);
        assert_eq!(deserialized.metadata.file_size, 0);
    }

    #[test]
    fn test_cached_file_entry_very_large_fields() {
        // Very long strings and large numeric values should survive roundtrip
        let very_long_string = "a".repeat(10_000);
        let entry = CachedFileEntry {
            file_path: "/music/very_long_path_name_test.flac".to_string(),
            last_modified: i64::MAX,
            file_hash: very_long_string.clone(),
            metadata: File {
                id: i64::MAX,
                file_path: very_long_string.clone(),
                file_hash: very_long_string.clone(),
                file_type: "flac".to_string(),
                file_size: i64::MAX,
                last_modified: i64::MAX,
                isrc: Some("US".to_string() + &"X".repeat(100)),
                last_scanned: i64::MAX,
                title: Some(very_long_string.clone()),
                artist: Some(very_long_string.clone()),
                album: Some(very_long_string.clone()),
                album_artist: None,
                track_number: Some(i32::MAX),
                total_tracks: Some(i32::MAX),
                disc_number: None,
                total_discs: None,
                genre: Some(very_long_string.clone()),
                year: Some(i32::MAX),
                composer: None,
                comment: Some(very_long_string.clone()),
                duration_ms: Some(i64::MAX),
                bitrate: Some(i32::MAX),
                sample_rate: Some(i32::MAX),
                channels: Some(i32::MAX),
                bpm: Some(f64::MAX),
                musical_key: Some(very_long_string.clone()),
                rating: i32::MAX,
                play_count: i32::MAX,
                last_played: Some(i64::MAX),
                spotify_id: Some(very_long_string.clone()),
                soundcloud_id: None,
                youtube_id: None,
                source_of: None,
                stem_type: Some("vocals".to_string()),
                last_verified_local: Some(i64::MAX),
        folder_id: None,
                created_at: i64::MAX,
                updated_at: i64::MAX,
            },
        };

        let json = serde_json::to_string_pretty(&entry).expect("serialize large entry");
        let deserialized: CachedFileEntry =
            serde_json::from_str(&json).expect("deserialize large entry");

        // Verify core fields
        assert_eq!(
            deserialized.file_path,
            "/music/very_long_path_name_test.flac"
        );
        assert_eq!(deserialized.last_modified, i64::MAX);
        assert!(
            deserialized.file_hash.len() == 10_000,
            "10k-char hash should survive roundtrip"
        );
        // Verify extreme numeric values
        assert_eq!(deserialized.metadata.id, i64::MAX);
        assert_eq!(deserialized.metadata.file_size, i64::MAX);
        assert_eq!(deserialized.metadata.duration_ms, Some(i64::MAX));
        assert_eq!(deserialized.metadata.bpm, Some(f64::MAX));
        // Verify very long strings
        assert_eq!(
            deserialized.metadata.title.as_deref(),
            Some(very_long_string.as_str())
        );
        assert_eq!(
            deserialized.metadata.artist.as_deref(),
            Some(very_long_string.as_str())
        );
        assert!(deserialized.metadata.isrc.as_ref().unwrap().len() > 100);
    }
}
