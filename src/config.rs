//! Service credentials and application configuration.
//!
//! # Configuration sources (priority order, highest wins)
//!
//! 1. Environment variables – override everything (useful for CI, Docker, quick dev)
//! 2. `~/.config/momos-music-manager/config.toml` – persistent user config
//! 3. Built-in defaults (e.g. `redirect_uri`)
//!
//! # Example config.toml
//!
//! ```toml
//! [spotify]
//! client_id     = "your_spotify_client_id"
//! client_secret = "your_spotify_client_secret"
//! redirect_uri  = "http://localhost:3000/callback"
//!
//! [soundcloud]
//! api_key  = "your_soundcloud_api_key"
//! user_id  = "your_soundcloud_user_id"
//!
//! [youtube]
//! api_key      = "your_youtube_api_key"
//! playlist_id  = "your_youtube_playlist_id"
//! ```
//!
//! # Backward compatibility
//!
//! A plain `.env` file in the working directory or `SPOTIFY_CLIENT_ID` etc. set
//! directly in the environment still work and **override** the TOML file.
//! This lets you keep secrets in `.env` (gitignored) during local development
//! while the TOML file is the canonical source for production / daily use.

use std::path::PathBuf;

use serde::Deserialize;
use tracing::warn;

// ── TOML config file structure ─────────────────────────────────────────────

/// Mirror of the on-disk `config.toml` schema.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
struct TomlConfig {
    spotify: Option<SpotifyToml>,
    soundcloud: Option<SoundcloudToml>,
    youtube: Option<YoutubeToml>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SpotifyToml {
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SoundcloudToml {
    api_key: Option<String>,
    user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct YoutubeToml {
    api_key: Option<String>,
    playlist_id: Option<String>,
}

// ── Runtime representation ─────────────────────────────────────────────────

/// Service credentials used throughout the application.
///
/// Construct via [`ServiceCredentials::load`] (TOML + env fallback) or
/// [`ServiceCredentials::from_env`] (env-only, for tests).
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
    /// Load credentials from config.toml, overridden by environment variables.
    ///
    /// 1. Reads `~/.config/momos-music-manager/config.toml` (if it exists).
    /// 2. Overrides every field with the corresponding `*_` environment variable
    ///    when set (even if empty — see note below).
    ///
    /// **Note on empty env vars**: Because `dotenvy` loads `.env` verbatim,
    /// `SPOTIFY_CLIENT_ID=` (equals sign with no value) yields an empty string,
    /// which will be treated as "set" and override the TOML value to `None`.
    /// To avoid surprises, either omit the variable from `.env` or use a
    /// non-empty value.
    pub fn load() -> Self {
        let toml_path = Self::config_path();
        let toml_config = Self::load_toml(&toml_path);

        Self {
            spotify_client_id: env_or_toml_opt(
                "SPOTIFY_CLIENT_ID",
                toml_config
                    .spotify
                    .as_ref()
                    .and_then(|s| s.client_id.clone()),
            ),
            spotify_client_secret: env_or_toml_opt(
                "SPOTIFY_CLIENT_SECRET",
                toml_config
                    .spotify
                    .as_ref()
                    .and_then(|s| s.client_secret.clone()),
            ),
            spotify_redirect_uri: env_or_toml(
                "SPOTIFY_REDIRECT_URI",
                toml_config
                    .spotify
                    .as_ref()
                    .and_then(|s| s.redirect_uri.clone()),
            )
            .unwrap_or_else(|| "http://localhost:3000/callback".to_string()),

            soundcloud_api_key: env_or_toml_opt(
                "SOUNDCLOUD_API_KEY",
                toml_config
                    .soundcloud
                    .as_ref()
                    .and_then(|s| s.api_key.clone()),
            ),
            soundcloud_user_id: env_or_toml_opt(
                "SOUNDCLOUD_USER_ID",
                toml_config
                    .soundcloud
                    .as_ref()
                    .and_then(|s| s.user_id.clone()),
            ),

            youtube_api_key: env_or_toml_opt(
                "YOUTUBE_API_KEY",
                toml_config.youtube.as_ref().and_then(|s| s.api_key.clone()),
            ),
            youtube_playlist_id: env_or_toml_opt(
                "YOUTUBE_PLAYLIST_ID",
                toml_config
                    .youtube
                    .as_ref()
                    .and_then(|s| s.playlist_id.clone()),
            ),
        }
    }

    /// Load credentials exclusively from environment variables (legacy path).
    ///
    /// Useful for tests or scenarios where you don't want TOML file interaction.
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

    // ── Helpers ──────────────────────────────────────────────────────────

    /// Full path to `config.toml`.
    fn config_path() -> PathBuf {
        // ~/.config/momos-music-manager/config.toml
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("momos-music-manager").join("config.toml")
    }

    /// Try to parse the TOML file. Returns an empty config on error/missing.
    fn load_toml(path: &PathBuf) -> TomlConfig {
        if !path.exists() {
            return TomlConfig::default();
        }
        match std::fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    warn!(
                        "Failed to parse {}: {e} — falling back to env vars",
                        path.display()
                    );
                    TomlConfig::default()
                }
            },
            Err(e) => {
                warn!(
                    "Failed to read {}: {e} — falling back to env vars",
                    path.display()
                );
                TomlConfig::default()
            }
        }
    }

    // ── Service status checks (unchanged) ────────────────────────────────

    pub fn is_spotify_configured(&self) -> bool {
        self.spotify_client_id.is_some() && self.spotify_client_secret.is_some()
    }

    pub fn is_soundcloud_configured(&self) -> bool {
        self.soundcloud_api_key.is_some()
    }

    pub fn is_youtube_configured(&self) -> bool {
        self.youtube_api_key.is_some()
    }

    pub fn spotify_client_id(&self) -> anyhow::Result<&str> {
        self.spotify_client_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "Spotify client ID not configured (set in config.toml or SPOTIFY_CLIENT_ID env var)"
            )
        })
    }

    pub fn spotify_client_secret(&self) -> anyhow::Result<&str> {
        self.spotify_client_secret
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Spotify client secret not configured (set in config.toml or SPOTIFY_CLIENT_SECRET env var)"))
    }

    pub fn soundcloud_api_key(&self) -> anyhow::Result<&str> {
        self.soundcloud_api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("SoundCloud API key not configured (set in config.toml or SOUNDCLOUD_API_KEY env var)"))
    }

    pub fn youtube_api_key(&self) -> anyhow::Result<&str> {
        self.youtube_api_key.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "YouTube API key not configured (set in config.toml or YOUTUBE_API_KEY env var)"
            )
        })
    }
}

// ── Helper functions ───────────────────────────────────────────────────────

/// Read a required env var.
fn env_var(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("Missing required environment variable: {}", name))
}

/// Read an optional env var.
fn env_var_optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Return the env var if set, otherwise fall back to the TOML value.
fn env_or_toml(name: &str, toml_value: Option<String>) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        Ok(_) => toml_value, // empty string → treat as "not set"
        Err(_) => toml_value,
    }
}

/// Same as `env_or_toml` but returns `Option<String>`.
fn env_or_toml_opt(name: &str, toml_value: Option<String>) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        Ok(_) => None, // explicitly emptied → clear
        Err(_) => toml_value,
    }
}
