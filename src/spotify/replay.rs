#![allow(dead_code)]

//! Spotify API response recorder/replayer for development
//!
//! Controlled by `SPOTIFY_API_CACHE` env var:
//! - unset / `off`  → live API calls (normal operation)
//! - `record`        → make real calls + save to `dev-data/spotify-api/`
//! - `replay`        → load from cache files, zero API calls
//!
//! Uses our own [`PlaylistInfo`] / [`TrackInfo`] types (fully owned + serializable)
//! instead of rspotify's lifetime-parameterised types.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::models::{PlaylistInfo, TrackInfo};

// ── Cache mode ─────────────────────────────────────────────────────────────────

/// How to source Spotify API data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Real API calls (default).
    Live,
    /// Real API calls + persist responses to disk.
    Record,
    /// Load persisted responses from disk, no network I/O.
    Replay,
}

impl CacheMode {
    /// Read from the `SPOTIFY_API_CACHE` env var.
    ///
    /// | Value    | Mode     |
    /// |----------|----------|
    /// | unset    | `Live`   |
    /// | `off`    | `Live`   |
    /// | `record` | `Record` |
    /// | `replay` | `Replay` |
    pub fn from_env() -> Self {
        match std::env::var("SPOTIFY_API_CACHE").ok().as_deref() {
            Some("record") => {
                info!("SPOTIFY_API_CACHE=record — recording API responses");
                CacheMode::Record
            }
            Some("replay") => {
                info!("SPOTIFY_API_CACHE=replay — replaying cached responses");
                CacheMode::Replay
            }
            _ => CacheMode::Live,
        }
    }

    /// Shorthand — returns `true` when the cache directory should be writable.
    pub fn should_record(self) -> bool {
        self == CacheMode::Record
    }

    /// Shorthand — returns `true` when API calls should be skipped entirely.
    pub fn should_replay(self) -> bool {
        self == CacheMode::Replay
    }
}

// ── Cache directory helpers ─────────────────────────────────────────────────────

/// Default cache root relative to the working directory.
const DEFAULT_CACHE_DIR: &str = "./dev-data/spotify-api";

/// Resolve the cache directory (respects `SPOTIFY_API_CACHE_DIR` env var).
pub fn cache_dir() -> PathBuf {
    std::env::var("SPOTIFY_API_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR))
}

fn playlists_path(root: &Path) -> PathBuf {
    root.join("playlists.json")
}

fn playlist_tracks_dir(root: &Path) -> PathBuf {
    root.join("playlist_tracks")
}

fn playlist_tracks_path(root: &Path, playlist_id: &str) -> PathBuf {
    playlist_tracks_dir(root).join(format!("{}.json", sanitise(playlist_id)))
}

/// Sanitise a Spotify ID for use as a filename.
fn sanitise(s: &str) -> String {
    s.replace(['/', '\\', ':', ' '], "_")
}

// ── Cache file payloads ─────────────────────────────────────────────────────────

/// Playlist-name → description map (used as a lightweight cache).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPlaylists {
    pub playlists: Vec<PlaylistInfo>,
}

/// Track + position for a single playlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTrackEntry {
    pub track: TrackInfo,
    pub position: i64,
    /// When the track was added to the playlist (Spotify `added_at`, Unix timestamp).
    #[serde(default)]
    pub added_at: Option<i64>,
}

/// Tracks belonging to one playlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPlaylistTracks {
    pub playlist_id: String,
    pub tracks: Vec<CachedTrackEntry>,
}

// ── Record helpers ──────────────────────────────────────────────────────────────

/// Save a batch of playlists to the cache.
pub async fn save_playlists(playlists: &[PlaylistInfo]) -> Result<()> {
    let root = cache_dir();
    tokio::fs::create_dir_all(&root)
        .await
        .context("Failed to create spotify-api cache directory")?;

    let payload = CachedPlaylists {
        playlists: playlists.to_vec(),
    };

    let path = playlists_path(&root);
    let bytes =
        serde_json::to_vec_pretty(&payload).context("Failed to serialise cached playlists")?;
    tokio::fs::write(&path, bytes)
        .await
        .context(format!("Failed to write {}", path.display()))?;

    debug!("Cached {} playlists → {}", playlists.len(), path.display());
    Ok(())
}

/// Save tracks for a single playlist to the cache.
pub async fn save_playlist_tracks(playlist_id: &str, tracks: Vec<CachedTrackEntry>) -> Result<()> {
    let root = cache_dir();
    let _dir = playlist_tracks_dir(&root);
    tokio::fs::create_dir_all(&_dir)
        .await
        .context("Failed to create playlist_tracks cache directory")?;

    let track_count = tracks.len();

    let payload = CachedPlaylistTracks {
        playlist_id: playlist_id.to_string(),
        tracks,
    };

    let path = playlist_tracks_path(&root, playlist_id);
    let bytes = serde_json::to_vec_pretty(&payload)
        .context("Failed to serialise cached playlist tracks")?;
    tokio::fs::write(&path, bytes)
        .await
        .context(format!("Failed to write {}", path.display()))?;

    debug!(
        "Cached {} tracks for playlist {} → {}",
        track_count,
        playlist_id,
        path.display()
    );
    Ok(())
}

// ── Replay helpers ──────────────────────────────────────────────────────────────

/// Load playlists from the cache. Returns `None` when the cache file is missing.
pub async fn load_playlists() -> Result<Option<Vec<PlaylistInfo>>> {
    let path = playlists_path(&cache_dir());
    if !path.exists() {
        warn!("Cache file not found: {}", path.display());
        return Ok(None);
    }

    let bytes = tokio::fs::read(&path)
        .await
        .context(format!("Failed to read {}", path.display()))?;
    let payload: CachedPlaylists =
        serde_json::from_slice(&bytes).context(format!("Failed to parse {}", path.display()))?;

    info!("Loaded {} playlists from cache", payload.playlists.len());
    Ok(Some(payload.playlists))
}

/// Load tracks for a single playlist from the cache. Returns `None` when missing.
pub async fn load_playlist_tracks(playlist_id: &str) -> Result<Option<Vec<CachedTrackEntry>>> {
    let path = playlist_tracks_path(&cache_dir(), playlist_id);
    if !path.exists() {
        warn!(
            "Cache file not found for playlist {}: {}",
            playlist_id,
            path.display()
        );
        return Ok(None);
    }

    let bytes = tokio::fs::read(&path)
        .await
        .context(format!("Failed to read {}", path.display()))?;
    let payload: CachedPlaylistTracks =
        serde_json::from_slice(&bytes).context(format!("Failed to parse {}", path.display()))?;

    debug!(
        "Loaded {} tracks for playlist {} from cache",
        payload.tracks.len(),
        playlist_id
    );
    Ok(Some(payload.tracks))
}

/// Check whether the full cache for a given set of playlists is available.
/// Returns the list of missing playlist IDs.
pub async fn missing_tracks_caches(playlist_ids: &[String]) -> Vec<String> {
    let root = cache_dir();
    let _dir = playlist_tracks_dir(&root);
    let mut missing = Vec::new();

    for pid in playlist_ids {
        let path = playlist_tracks_path(&root, pid);
        if !path.exists() {
            missing.push(pid.clone());
        }
    }

    if !missing.is_empty() {
        warn!(
            "{} of {} track caches are missing (first: {})",
            missing.len(),
            playlist_ids.len(),
            missing.first().unwrap()
        );
    }

    missing
}

// ── Housekeeping ────────────────────────────────────────────────────────────────

/// Delete all cached files for a clean recording session.
pub async fn clear_cache() -> Result<()> {
    let root = cache_dir();
    if root.exists() {
        tokio::fs::remove_dir_all(&root)
            .await
            .context("Failed to clear spotify-api cache")?;
    }
    tokio::fs::create_dir_all(&root)
        .await
        .context("Failed to recreate spotify-api cache directory")?;
    info!("Cleared Spotify API cache at {}", root.display());
    Ok(())
}
