//! Spotify API client wrapper
//!
//! This module provides a Spotify client that handles authentication,
//! token refresh, and API calls with proper error handling.

use anyhow::{Context, Result};
use chrono::Duration;
use rspotify::{
    AuthCodeSpotify, Config, Credentials, OAuth, Token,
    clients::{BaseClient, OAuthClient},
    model::{
        Market, PlaylistId, SimplifiedPlaylist, TrackId, UserId, playlist::FullPlaylist,
        track::FullTrack,
    },
};
use sqlx::Pool;
use tokio_stream::StreamExt;
use tracing::{error, info};

use crate::config::ServiceCredentials;
use crate::db::{get_service_config, update_service_tokens};

/// Spotify client wrapper with authentication and token refresh
pub struct SpotifyClient {
    /// Underlying rspotify client
    pub spotify: AuthCodeSpotify,

    /// Database connection pool
    pub db: Pool<sqlx::Sqlite>,

    /// Service name (always "spotify")
    #[allow(dead_code)]
    pub service: String,
}

impl SpotifyClient {
    /// Create a new Spotify client from stored tokens in the database
    ///
    /// # Arguments
    /// * `db` - Database connection pool
    /// * `config` - Service credentials from .env file
    ///
    /// # Returns
    /// * `Ok(SpotifyClient)` if tokens are available and valid
    /// * `Err` if tokens are missing or invalid
    pub async fn from_stored_tokens(
        db: Pool<sqlx::Sqlite>,
        config: &ServiceCredentials,
    ) -> Result<Self> {
        // Get Spotify credentials from .env
        let client_id = config
            .spotify_client_id()
            .context("Spotify client ID not configured in .env")?
            .to_string();
        let client_secret = config
            .spotify_client_secret()
            .context("Spotify client secret not configured in .env")?
            .to_string();
        let redirect_uri = config.spotify_redirect_uri.clone();

        // Get stored tokens from database
        let service_config = get_service_config(&db, "spotify")
            .await?
            .context("Spotify service not configured in database")?;

        let refresh_token = service_config
            .refresh_token
            .context("Spotify refresh token not found in database")?;
        let access_token = service_config
            .access_token
            .context("Spotify access token not found in database")?;
        let token_expiry = service_config.token_expiry;

        // Create OAuth configuration
        let creds = Credentials::new(&client_id, &client_secret);
        let oauth = OAuth {
            redirect_uri,
            scopes: rspotify::scopes!(
                "playlist-read-private",
                "playlist-read-collaborative",
                "playlist-modify-public",
                "playlist-modify-private",
                "user-read-playback-state"
            ),
            ..Default::default()
        };

        let spotify_config = Config {
            token_refreshing: true,
            ..Default::default()
        };

        let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

        // Set the stored token
        {
            let token_lock = spotify.token.lock().await;
            if let Ok(mut guard) = token_lock {
                *guard = Some(Token {
                    refresh_token: Some(refresh_token.clone()),
                    access_token: access_token.clone(),
                    expires_in: Duration::seconds(3600), // Default
                    expires_at: token_expiry.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
                    scopes: Default::default(),
                });
                tracing::debug!(
                    "Token set: access_token={}, refresh_token={}",
                    access_token.len(),
                    refresh_token.len()
                );
            } else {
                error!("Failed to acquire token lock");
                return Err(anyhow::anyhow!("Failed to acquire token lock"));
            }
        }

        info!("Spotify client created from stored tokens");

        Ok(Self {
            spotify,
            db,
            service: "spotify".to_string(),
        })
    }

    /// Check if the client is authenticated and tokens are valid
    #[allow(dead_code)]
    pub async fn is_authenticated(&self) -> bool {
        let token_lock = self.spotify.token.lock().await;
        if let Ok(guard) = token_lock {
            guard.is_some()
        } else {
            false
        }
    }

    /// Refresh access token if needed and update database
    pub async fn refresh_token_if_needed(&self) -> Result<()> {
        tracing::debug!("Checking if token needs refresh");
        let needs_refresh = {
            let token_lock = self.spotify.token.lock().await;
            if let Ok(guard) = token_lock {
                if let Some(token) = guard.as_ref() {
                    tracing::debug!(
                        "Token exists: access_token={}, refresh_token={}",
                        token.access_token.len(),
                        token.refresh_token.as_ref().map(|t| t.len()).unwrap_or(0)
                    );
                    // Check if token expires in less than 5 minutes
                    if let Some(expires_at) = token.expires_at {
                        let now = chrono::Utc::now();
                        let time_until_expiry = expires_at - now;
                        let needs = time_until_expiry < chrono::Duration::minutes(5);
                        tracing::debug!(
                            "Token expires at {:?}, time_until_expiry={:?}, needs_refresh={}",
                            expires_at,
                            time_until_expiry,
                            needs
                        );
                        needs
                    } else {
                        tracing::debug!("No expiry time, assuming needs refresh");
                        true // No expiry time, assume needs refresh
                    }
                } else {
                    tracing::debug!("No token in client");
                    true // No token
                }
            } else {
                error!("Failed to acquire token lock");
                return Err(anyhow::anyhow!("Failed to acquire token lock"));
            }
        };

        if needs_refresh {
            tracing::debug!("Refreshing access token");
            self.refresh_token().await?;
        } else {
            tracing::debug!("Token does not need refresh");
        }

        Ok(())
    }

    /// Force refresh the access token
    async fn refresh_token(&self) -> Result<()> {
        // The rspotify client should handle token refresh automatically
        // when token_refreshing is enabled in the Config.
        // We just need to get the updated token and store it in the database.

        let (refresh_token, access_token, token_expiry) = {
            let token_lock = self.spotify.token.lock().await;
            if let Ok(guard) = token_lock {
                if let Some(token) = guard.as_ref() {
                    let refresh_token = token.refresh_token.clone().unwrap_or_default();
                    let access_token = token.access_token.clone();
                    let token_expiry = token.expires_at.map(|dt| dt.timestamp());

                    (refresh_token, access_token, token_expiry)
                } else {
                    return Err(anyhow::anyhow!("No token available for refresh"));
                }
            } else {
                return Err(anyhow::anyhow!("Failed to acquire token lock"));
            }
        };

        // Update tokens in database
        update_service_tokens(
            &self.db,
            "spotify",
            Some(&refresh_token),
            Some(&access_token),
            token_expiry,
        )
        .await
        .context("Failed to update tokens in database")?;

        info!("Spotify token refreshed and saved to database");
        Ok(())
    }

    /// Get the current user's playlists (streaming)
    ///
    /// # Returns
    /// * Stream of simplified playlists
    pub async fn get_user_playlists<'a>(
        &'a self,
    ) -> Result<impl tokio_stream::Stream<Item = Result<SimplifiedPlaylist>> + 'a> {
        tracing::debug!("Getting user playlists");
        self.refresh_token_if_needed().await?;
        tracing::debug!("Token refresh check done, calling current_user_playlists()");

        let stream = self.spotify.current_user_playlists();
        tracing::debug!("Got playlist stream");

        // Convert the stream to our result type
        Ok(stream.map(|item| match item {
            Ok(playlist) => {
                tracing::debug!("Fetched playlist: {}", playlist.name);
                Ok(playlist)
            }
            Err(e) => {
                error!("Failed to fetch playlist: {}", e);
                Err(anyhow::Error::from(e)).context("Spotify API error")
            }
        }))
    }

    /// Get a specific playlist by ID
    pub async fn get_playlist(&self, playlist_id: &str) -> Result<FullPlaylist> {
        self.refresh_token_if_needed().await?;

        let playlist_id = PlaylistId::from_id(playlist_id)
            .map_err(|e| anyhow::anyhow!("Invalid playlist ID: {}", e))?;

        self.spotify
            .playlist(playlist_id, None, None)
            .await
            .context("Failed to fetch playlist")
    }

    /// Get tracks from a playlist (streaming)
    ///
    /// # Arguments
    /// * `playlist_id` - Spotify playlist ID
    ///
    /// # Returns
    /// * Stream of playlist items
    pub async fn get_playlist_tracks<'a>(
        &'a self,
        playlist_id: &'a str,
    ) -> Result<impl tokio_stream::Stream<Item = Result<rspotify::model::PlaylistItem>> + 'a> {
        self.refresh_token_if_needed().await?;

        let playlist_id = PlaylistId::from_id(playlist_id)
            .map_err(|e| anyhow::anyhow!("Invalid playlist ID: {}", e))?;

        let stream = self
            .spotify
            .playlist_items(playlist_id, None, Some(Market::FromToken));

        Ok(stream.map(|item| match item {
            Ok(item) => Ok(item),
            Err(e) => {
                error!("Failed to fetch playlist track: {}", e);
                Err(anyhow::Error::from(e)).context("Spotify API error")
            }
        }))
    }

    /// Get a track by ID
    #[allow(dead_code)]
    pub async fn get_track(&self, track_id: &str) -> Result<FullTrack> {
        self.refresh_token_if_needed().await?;

        let track_id =
            TrackId::from_id(track_id).map_err(|e| anyhow::anyhow!("Invalid track ID: {}", e))?;

        self.spotify
            .track(track_id, None)
            .await
            .context("Failed to fetch track")
    }

    /// Get the current user's profile
    #[allow(dead_code)]
    pub async fn get_current_user(&self) -> Result<rspotify::model::PrivateUser> {
        self.refresh_token_if_needed().await?;

        self.spotify
            .current_user()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch current user: {}", e))
    }

    /// Get current user's Spotify ID.
    pub async fn get_current_user_id(&self) -> Result<String> {
        self.refresh_token_if_needed().await?;
        let user = self
            .spotify
            .current_user()
            .await
            .context("Failed to get current user")?;
        Ok(user.id.to_string())
    }

    /// Create a Spotify playlist. Returns (playlist_id, spotify_url).
    pub async fn create_playlist(
        &self,
        user_id: &str,
        name: &str,
        public: bool,
        description: Option<&str>,
    ) -> Result<(String, String)> {
        self.refresh_token_if_needed().await?;
        let uid =
            UserId::from_id(user_id).map_err(|e| anyhow::anyhow!("Invalid user ID: {}", e))?;
        let playlist = self
            .spotify
            .user_playlist_create(uid, name, Some(public), None, description)
            .await
            .context("Failed to create Spotify playlist")?;
        let url = playlist
            .external_urls
            .get("spotify")
            .cloned()
            .unwrap_or_default();
        Ok((playlist.id.to_string(), url))
    }

    /// Add tracks to a Spotify playlist in batches of 100.
    /// track_uris should be "spotify:track:XXX" format.
    pub async fn add_tracks_to_playlist(
        &self,
        playlist_id: &str,
        track_uris: &[String],
    ) -> Result<()> {
        self.refresh_token_if_needed().await?;
        let pid = PlaylistId::from_id(playlist_id)
            .map_err(|e| anyhow::anyhow!("Invalid playlist ID: {}", e))?;

        for chunk in track_uris.chunks(100) {
            let items: Vec<rspotify::model::PlayableId> = chunk
                .iter()
                .filter_map(|uri| {
                    TrackId::from_uri(uri)
                        .ok()
                        .map(|id| rspotify::model::PlayableId::Track(id))
                })
                .collect();
            if !items.is_empty() {
                self.spotify
                    .playlist_add_items(pid.clone(), items, None)
                    .await
                    .context("Failed to add tracks to playlist")?;
            }
        }
        Ok(())
    }

    /// Save current tokens to database
    #[allow(dead_code)]
    pub async fn save_tokens_to_db(&self) -> Result<()> {
        let (refresh_token, access_token, token_expiry) = {
            let token_lock = self.spotify.token.lock().await;
            if let Ok(guard) = token_lock {
                if let Some(token) = guard.as_ref() {
                    let refresh_token = token.refresh_token.clone().unwrap_or_default();
                    let access_token = token.access_token.clone();
                    let token_expiry = token.expires_at.map(|dt| dt.timestamp());

                    (refresh_token, access_token, token_expiry)
                } else {
                    return Err(anyhow::anyhow!("No token available to save"));
                }
            } else {
                return Err(anyhow::anyhow!("Failed to acquire token lock"));
            }
        };

        update_service_tokens(
            &self.db,
            "spotify",
            Some(&refresh_token),
            Some(&access_token),
            token_expiry,
        )
        .await
        .context("Failed to save tokens to database")
    }
}
