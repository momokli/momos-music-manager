use std::env;

/// Service credentials loaded from environment variables (.env file)
#[derive(Debug, Clone)]
pub struct ServiceCredentials {
    // Spotify OAuth credentials
    pub spotify_client_id: Option<String>,
    pub spotify_client_secret: Option<String>,
    pub spotify_redirect_uri: String,

    // SoundCloud API credentials
    pub soundcloud_api_key: Option<String>,
    pub soundcloud_user_id: Option<String>,

    // YouTube API credentials
    pub youtube_api_key: Option<String>,
    pub youtube_playlist_id: Option<String>,
}

impl ServiceCredentials {
    /// Create ServiceCredentials by reading environment variables
    pub fn from_env() -> Self {
        Self {
            spotify_client_id: env_var_optional("SPOTIFY_CLIENT_ID"),
            spotify_client_secret: env_var_optional("SPOTIFY_CLIENT_SECRET"),
            spotify_redirect_uri: env_var("SPOTIFY_REDIRECT_URI")
                .unwrap_or_else(|_| "http://localhost:3000/callback".to_string()),

            soundcloud_api_key: env_var_optional("SOUNDCLOUD_API_KEY"),
            soundcloud_user_id: env_var_optional("SOUNDCLOUD_USER_ID"),

            youtube_api_key: env_var_optional("YOUTUBE_API_KEY"),
            youtube_playlist_id: env_var_optional("YOUTUBE_PLAYLIST_ID"),
        }
    }

    /// Check if Spotify is configured (client ID and secret present)
    pub fn is_spotify_configured(&self) -> bool {
        self.spotify_client_id.is_some() && self.spotify_client_secret.is_some()
    }

    /// Check if SoundCloud is configured (API key present)
    pub fn is_soundcloud_configured(&self) -> bool {
        self.soundcloud_api_key.is_some()
    }

    /// Check if YouTube is configured (API key present)
    pub fn is_youtube_configured(&self) -> bool {
        self.youtube_api_key.is_some()
    }

    /// Get Spotify client ID (returns error if not configured)
    pub fn spotify_client_id(&self) -> anyhow::Result<&str> {
        self.spotify_client_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("SPOTIFY_CLIENT_ID not configured in .env file"))
    }

    /// Get Spotify client secret (returns error if not configured)
    pub fn spotify_client_secret(&self) -> anyhow::Result<&str> {
        self.spotify_client_secret
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("SPOTIFY_CLIENT_SECRET not configured in .env file"))
    }

    /// Get SoundCloud API key (returns error if not configured)
    pub fn soundcloud_api_key(&self) -> anyhow::Result<&str> {
        self.soundcloud_api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("SOUNDCLOUD_API_KEY not configured in .env file"))
    }

    /// Get YouTube API key (returns error if not configured)
    pub fn youtube_api_key(&self) -> anyhow::Result<&str> {
        self.youtube_api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("YOUTUBE_API_KEY not configured in .env file"))
    }
}

/// Read required environment variable
fn env_var(name: &str) -> anyhow::Result<String> {
    env::var(name).map_err(|_| anyhow::anyhow!("Missing required environment variable: {}", name))
}

/// Read optional environment variable
fn env_var_optional(name: &str) -> Option<String> {
    env::var(name).ok()
}
