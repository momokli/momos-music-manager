//! Spotify model definitions and re-exports
//!
//! This module provides re-exports of commonly used rspotify models
//! and defines Spotify-specific types for the sync system.

// Re-export commonly used rspotify models

use rspotify::prelude::Id;
use serde::{Deserialize, Serialize};

/// Spotify-specific sync result
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SpotifySyncResult {
    /// Number of playlists synced
    pub playlist_count: usize,
    /// Number of tracks synced
    pub track_count: usize,
    /// Names of synced playlists
    pub playlist_names: Vec<String>,
    /// Names of synced tracks
    pub track_names: Vec<String>,
    /// Detailed error information if sync failed
    pub error_details: Option<String>,
}

impl SpotifySyncResult {
    /// Create a successful sync result
    #[allow(dead_code)]
    pub fn success(
        playlist_count: usize,
        track_count: usize,
        playlist_names: Vec<String>,
        track_names: Vec<String>,
    ) -> Self {
        Self {
            playlist_count,
            track_count,
            playlist_names,
            track_names,
            error_details: None,
        }
    }

    /// Create a failed sync result
    #[allow(dead_code)]
    pub fn failed(error_details: String) -> Self {
        Self {
            playlist_count: 0,
            track_count: 0,
            playlist_names: Vec::new(),
            track_names: Vec::new(),
            error_details: Some(error_details),
        }
    }
}

/// Information about a playlist for database storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistInfo {
    /// Spotify playlist ID
    pub id: String,
    /// Playlist name
    pub name: String,
    /// Playlist description
    pub description: Option<String>,
    /// Snapshot ID for change detection (global poller)
    pub snapshot_id: String,
    /// Number of tracks in the playlist
    pub track_count: usize,
    /// Whether the playlist is collaborative
    pub collaborative: bool,
    /// Whether the playlist is public
    pub public: bool,
    /// Spotify owner ID
    pub owner_id: String,
    /// Spotify owner display name
    pub owner_name: Option<String>,
}

/// Information about a track for database storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    /// Spotify track ID
    pub id: String,
    /// Track name
    pub name: String,
    /// Artist names (comma-separated)
    pub artists: String,
    /// Album name
    pub album: Option<String>,
    /// ISRC code
    pub isrc: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: i64,
    /// Track number
    pub track_number: Option<i32>,
    /// Disc number
    pub disc_number: Option<i32>,
    /// Whether the track is explicit
    pub explicit: bool,
    /// Popularity score (0-100)
    pub popularity: Option<i32>,
}

/// Conversion from rspotify::model::SimplifiedPlaylist to PlaylistInfo
impl From<&rspotify::model::SimplifiedPlaylist> for PlaylistInfo {
    fn from(playlist: &rspotify::model::SimplifiedPlaylist) -> Self {
        Self {
            id: playlist.id.id().to_string(),
            name: playlist.name.clone(),
            description: None, // SimplifiedPlaylist doesn't have description
            snapshot_id: playlist.snapshot_id.clone(),
            track_count: playlist.tracks.total as usize,
            collaborative: playlist.collaborative,
            public: playlist.public.unwrap_or(false),
            owner_id: playlist.owner.id.id().to_string(),
            owner_name: playlist.owner.display_name.clone(),
        }
    }
}

/// Conversion from rspotify::model::track::FullTrack to TrackInfo
impl From<&rspotify::model::track::FullTrack> for TrackInfo {
    fn from(track: &rspotify::model::track::FullTrack) -> Self {
        // Combine artist names
        let artists = track
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", ");

        // Get track ID if available
        let id = track
            .id
            .as_ref()
            .map(|id| id.id().to_string())
            .unwrap_or_default();

        // Get album name if available
        let album = Some(track.album.name.clone());

        Self {
            id,
            name: track.name.clone(),
            artists,
            album,
            isrc: track
                .external_ids
                .get("isrc")
                .map(|s| s.as_str().to_string()),
            duration_ms: track.duration.num_milliseconds(),
            track_number: Some(track.track_number as i32),
            disc_number: Some(track.disc_number),
            explicit: track.explicit,
            popularity: Some(track.popularity as i32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spotify_sync_result_success() {
        let result = SpotifySyncResult::success(
            3,
            45,
            vec!["Deep House".to_string(), "Tech House".to_string()],
            vec!["Track A".to_string(), "Track B".to_string()],
        );

        assert_eq!(result.playlist_count, 3);
        assert_eq!(result.track_count, 45);
        assert_eq!(result.playlist_names.len(), 2);
        assert!(result.playlist_names.contains(&"Deep House".to_string()));
        assert!(result.track_names.contains(&"Track A".to_string()));
        assert!(result.error_details.is_none());
    }

    #[test]
    fn test_spotify_sync_result_failed() {
        let result = SpotifySyncResult::failed("Rate limited".to_string());

        assert_eq!(result.playlist_count, 0);
        assert_eq!(result.track_count, 0);
        assert!(result.playlist_names.is_empty());
        assert!(result.track_names.is_empty());
        assert_eq!(result.error_details, Some("Rate limited".to_string()));
    }

    #[test]
    fn test_playlist_info_fields() {
        let info = PlaylistInfo {
            id: "37i9dQZEVXcJZyENOWUFo7".to_string(),
            name: "Top 50 Global".to_string(),
            description: Some("Global top 50".to_string()),
            snapshot_id: "snap_abc123".to_string(),
            track_count: 50,
            collaborative: false,
            public: true,
            owner_id: "spotify".to_string(),
            owner_name: Some("Spotify".to_string()),
        };

        assert_eq!(info.id, "37i9dQZEVXcJZyENOWUFo7");
        assert_eq!(info.name, "Top 50 Global");
        assert_eq!(info.snapshot_id, "snap_abc123");
        assert!(!info.collaborative);
        assert!(info.public);
    }

    #[test]
    fn test_track_info_fields() {
        let info = TrackInfo {
            id: "7ouMYWpwJ422jRcDASZB7P".to_string(),
            name: "Strobe".to_string(),
            artists: "Deadmau5".to_string(),
            album: Some("For Lack of a Better Name".to_string()),
            isrc: Some("GBKPL0988224".to_string()),
            duration_ms: 637733,
            track_number: Some(5),
            disc_number: Some(1),
            explicit: false,
            popularity: Some(72),
        };

        assert_eq!(info.id, "7ouMYWpwJ422jRcDASZB7P");
        assert_eq!(info.name, "Strobe");
        assert!(info.explicit == false);
        assert_eq!(info.popularity, Some(72));
    }
}
