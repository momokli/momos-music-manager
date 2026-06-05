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
                created_at: 1700000000,
                updated_at: 1700000000,
            },
        };

        assert_eq!(entry.file_path, "/music/test.flac");
        assert_eq!(entry.last_modified, 1700000000);
        assert_eq!(entry.file_hash, "abc123");
    }
}
