//! Tidal API model definitions.
//!
//! Uses `#[derive(Deserialize)]` for the Tidal v2 API JSON responses.
//! Tidal's API returns clean, well-structured JSON unlike SoundCloud.

use serde::Deserialize;

/// A Tidal playlist as returned by `GET /playlists` or `GET /playlists/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TidalPlaylist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub track_count: i32,
    pub public: bool,
    pub owner_name: Option<String>,
}

/// A Tidal track as returned from playlist items or search results.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TidalTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub duration_ms: i64,
    pub album: Option<String>,
    pub track_number: Option<i32>,
}

/// OAuth2 token response from `POST /oauth2/token`.
#[derive(Debug, Clone, Deserialize)]
pub struct TidalTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub token_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tidal_playlist_deser() {
        let json = r#"{
            "id": "12345",
            "name": "My Playlist",
            "description": "A test playlist",
            "trackCount": 42,
            "public": true,
            "ownerName": "testuser"
        }"#;
        let pl: TidalPlaylist = serde_json::from_str(json).unwrap();
        assert_eq!(pl.id, "12345");
        assert_eq!(pl.name, "My Playlist");
        assert_eq!(pl.track_count, 42);
        assert!(pl.public);
        assert_eq!(pl.owner_name.as_deref(), Some("testuser"));
    }

    #[test]
    fn test_tidal_track_deser() {
        let json = r#"{
            "id": "track-1",
            "title": "Test Track",
            "artist": "Test Artist",
            "isrc": "USABC1234567",
            "durationMs": 240000,
            "album": "Test Album",
            "trackNumber": 5
        }"#;
        let track: TidalTrack = serde_json::from_str(json).unwrap();
        assert_eq!(track.id, "track-1");
        assert_eq!(track.title, "Test Track");
        assert_eq!(track.isrc.as_deref(), Some("USABC1234567"));
        assert_eq!(track.duration_ms, 240000);
        assert_eq!(track.album.as_deref(), Some("Test Album"));
        assert_eq!(track.track_number, Some(5));
    }

    #[test]
    fn test_tidal_token_response_deser() {
        let json = r#"{
            "access_token": "eyJhbGciOi...",
            "refresh_token": "refresh-xxx",
            "expires_in": 86400,
            "token_type": "Bearer"
        }"#;
        let token: TidalTokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(token.access_token, "eyJhbGciOi...");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-xxx"));
        assert_eq!(token.expires_in, 86400);
        assert_eq!(token.token_type, "Bearer");
    }
}
