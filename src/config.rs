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
//! [database]
//! url = "sqlite:~/.local/share/momos-music-manager/library.db"
//!
//! [server]
//! host = "127.0.0.1"
//! port = 3000
//! public_url = "https://mmm.mydomain.de"
//!
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
use tracing::{info, warn};

// ── TOML config file structure ─────────────────────────────────────────────

/// Mirror of the on-disk `config.toml` schema.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
struct TomlConfig {
    spotify: Option<SpotifyToml>,
    soundcloud: Option<SoundcloudToml>,
    youtube: Option<YoutubeToml>,
    database: Option<DatabaseToml>,
    server: Option<ServerToml>,
    polling: Option<PollingToml>,
    maintainer: Option<MaintainerToml>,
}

#[derive(Debug, Clone, Deserialize)]
struct DatabaseToml {
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerToml {
    host: Option<String>,
    port: Option<u16>,
    public_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SpotifyToml {
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SoundcloudToml {
    api_key: Option<String>,
    user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct YoutubeToml {
    api_key: Option<String>,
    playlist_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PollingToml {
    global_interval_secs: Option<u64>,
    cold_start_threshold_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct MaintainerToml {
    interval_secs: Option<u64>,
    full_scan_max_age_secs: Option<u64>,
    backup_discovery_interval_secs: Option<u64>,
    auto_prune: Option<bool>,
    auto_cleanup_dirs: Option<bool>,
    traktor_import_enabled: Option<bool>,
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
    #[allow(dead_code)]
    pub soundcloud_user_id: Option<String>,

    // YouTube API credentials
    pub youtube_api_key: Option<String>,
    #[allow(dead_code)]
    pub youtube_playlist_id: Option<String>,

    // Database configuration
    pub database_url: String,

    // Server configuration
    pub server_host: String,
    pub server_port: u16,
    #[allow(dead_code)]
    pub server_public_url: Option<String>,

    // Polling configuration
    pub global_poll_interval_secs: u64,
    pub cold_start_threshold_secs: u64,

    // Maintainer configuration
    pub maintainer_interval_secs: u64,
    pub maintainer_full_scan_max_age_secs: u64,
    pub maintainer_backup_discovery_interval_secs: u64,
    pub maintainer_auto_prune: bool,
    pub maintainer_auto_cleanup_dirs: bool,
    pub maintainer_traktor_import_enabled: bool,
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
        let toml_config = Self::load_toml();

        let has_toml_spotify = toml_config.spotify.is_some();

        let spotify_id = env_or_toml_opt(
            "SPOTIFY_CLIENT_ID",
            toml_config
                .spotify
                .as_ref()
                .and_then(|s| s.client_id.clone()),
        );
        let spotify_secret = env_or_toml_opt(
            "SPOTIFY_CLIENT_SECRET",
            toml_config
                .spotify
                .as_ref()
                .and_then(|s| s.client_secret.clone()),
        );
        let spotify_redirect = env_or_toml(
            "SPOTIFY_REDIRECT_URI",
            toml_config
                .spotify
                .as_ref()
                .and_then(|s| s.redirect_uri.clone()),
        )
        .unwrap_or_else(|| "http://localhost:3000/callback".to_string());

        // Log where each Spotify credential came from
        let sid_src =
            Self::credential_source("SPOTIFY_CLIENT_ID", spotify_id.as_deref(), has_toml_spotify);
        let ssec_src = Self::credential_source(
            "SPOTIFY_CLIENT_SECRET",
            spotify_secret.as_deref(),
            has_toml_spotify,
        );
        let sredir_src = Self::credential_source(
            "SPOTIFY_REDIRECT_URI",
            Some(&spotify_redirect),
            has_toml_spotify,
        );
        info!(
            "Spotify config: client-id={sid_src}, client-secret={ssec_src}, redirect-uri={sredir_src}"
        );

        let credentials = Self {
            spotify_client_id: spotify_id,
            spotify_client_secret: spotify_secret,
            spotify_redirect_uri: spotify_redirect,

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

            // Database: env var > config.toml > built-in default
            database_url: env_or_toml(
                "DATABASE_URL",
                toml_config.database.as_ref().and_then(|d| d.url.clone()),
            )
            .unwrap_or_else(default_database_url),

            // Server host: env var > config.toml > built-in default
            server_host: env_or_toml(
                "HOST",
                toml_config.server.as_ref().and_then(|s| s.host.clone()),
            )
            .unwrap_or_else(|| "127.0.0.1".to_string()),

            // Server port: env var > config.toml > built-in default
            server_port: env_or_toml_port("PORT", toml_config.server.as_ref().and_then(|s| s.port))
                .unwrap_or(3000),

            // Server public URL: env var > config.toml > None
            server_public_url: env_or_toml_opt(
                "PUBLIC_URL",
                toml_config
                    .server
                    .as_ref()
                    .and_then(|s| s.public_url.clone()),
            ),

            // Polling config: env var > config.toml > built-in default
            global_poll_interval_secs: std::env::var("MOMOS_GLOBAL_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .polling
                        .as_ref()
                        .and_then(|p| p.global_interval_secs)
                })
                .unwrap_or(900), // 15 minutes default

            cold_start_threshold_secs: std::env::var("MOMOS_COLD_START_THRESHOLD_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .polling
                        .as_ref()
                        .and_then(|p| p.cold_start_threshold_secs)
                })
                .unwrap_or(86400), // 24 hours default

            // Maintainer config: env var > config.toml > built-in default
            maintainer_interval_secs: std::env::var("MOMOS_MAINTAINER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .maintainer
                        .as_ref()
                        .and_then(|m| m.interval_secs)
                })
                .unwrap_or(3600), // 1 hour default

            maintainer_full_scan_max_age_secs: std::env::var("MOMOS_MAINTAINER_FULL_SCAN_MAX_AGE")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .maintainer
                        .as_ref()
                        .and_then(|m| m.full_scan_max_age_secs)
                })
                .unwrap_or(86400), // 24 hours default

            maintainer_backup_discovery_interval_secs: std::env::var(
                "MOMOS_MAINTAINER_BACKUP_DISCOVERY_INTERVAL",
            )
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or_else(|| {
                toml_config
                    .maintainer
                    .as_ref()
                    .and_then(|m| m.backup_discovery_interval_secs)
            })
            .unwrap_or(604800), // 7 days default

            maintainer_auto_prune: std::env::var("MOMOS_MAINTAINER_AUTO_PRUNE")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .or_else(|| toml_config.maintainer.as_ref().and_then(|m| m.auto_prune))
                .unwrap_or(false),

            maintainer_auto_cleanup_dirs: std::env::var("MOMOS_MAINTAINER_AUTO_CLEANUP_DIRS")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .or_else(|| {
                    toml_config
                        .maintainer
                        .as_ref()
                        .and_then(|m| m.auto_cleanup_dirs)
                })
                .unwrap_or(false),

            maintainer_traktor_import_enabled: std::env::var(
                "MOMOS_MAINTAINER_TRAKTOR_IMPORT_ENABLED",
            )
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .or_else(|| {
                toml_config
                    .maintainer
                    .as_ref()
                    .and_then(|m| m.traktor_import_enabled)
            })
            .unwrap_or(true),
        };

        info!(
            "Polling config: global_interval={}s, cold_start_threshold={}s",
            credentials.global_poll_interval_secs, credentials.cold_start_threshold_secs,
        );

        info!(
            "Maintainer config: interval={}s, full_scan_max_age={}s, backup_discovery_interval={}s, \
             auto_prune={}, auto_cleanup_dirs={}, traktor_import={}",
            credentials.maintainer_interval_secs,
            credentials.maintainer_full_scan_max_age_secs,
            credentials.maintainer_backup_discovery_interval_secs,
            credentials.maintainer_auto_prune,
            credentials.maintainer_auto_cleanup_dirs,
            credentials.maintainer_traktor_import_enabled,
        );

        credentials
    }

    /// Load credentials exclusively from environment variables (legacy path).
    ///
    /// Useful for tests or scenarios where you don't want TOML file interaction.
    #[allow(dead_code)]
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

            database_url: env_var("DATABASE_URL").unwrap_or_else(|_| default_database_url()),
            server_host: env_var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            server_port: env_var_optional("PORT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3000),
            server_public_url: env_var_optional("PUBLIC_URL"),
            global_poll_interval_secs: env_var_optional("MOMOS_GLOBAL_POLL_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(900),
            cold_start_threshold_secs: env_var_optional("MOMOS_COLD_START_THRESHOLD_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(86400),
            maintainer_interval_secs: env_var_optional("MOMOS_MAINTAINER_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            maintainer_full_scan_max_age_secs: env_var_optional(
                "MOMOS_MAINTAINER_FULL_SCAN_MAX_AGE_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(86400),
            maintainer_backup_discovery_interval_secs: env_var_optional(
                "MOMOS_MAINTAINER_BACKUP_DISCOVERY_INTERVAL_SECS",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(604800),

            maintainer_auto_prune: env_var_optional("MOMOS_MAINTAINER_AUTO_PRUNE")
                .and_then(|v| v.parse().ok())
                .unwrap_or(false),

            maintainer_auto_cleanup_dirs: env_var_optional("MOMOS_MAINTAINER_AUTO_CLEANUP_DIRS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(false),

            maintainer_traktor_import_enabled: env_var_optional(
                "MOMOS_MAINTAINER_TRAKTOR_IMPORT_ENABLED",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(true),
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    /// Log-friendly label showing where a credential came from.
    fn credential_source(env_name: &str, value: Option<&str>, has_toml: bool) -> &'static str {
        if std::env::var(env_name).is_ok_and(|v| !v.is_empty()) {
            return "env";
        }
        if value.is_some() && has_toml {
            return "toml";
        }
        if value.is_some() {
            return "default";
        }
        "missing"
    }

    /// Returns candidate config paths in priority order:
    /// 1. `~/.config/momos-music-manager/config.toml` (XDG convention)
    /// 2. `{dirs::config_dir()}/momos-music-manager/config.toml` (OS-native, e.g. macOS Library)
    fn config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // 1. XDG-style ~/.config/ (what most users actually use)
        if let Some(home) = dirs::home_dir() {
            paths.push(
                home.join(".config")
                    .join("momos-music-manager")
                    .join("config.toml"),
            );
        }

        // 2. OS-native config dir (e.g. ~/Library/Application Support/ on macOS)
        if let Some(base) = dirs::config_dir() {
            let os_path = base.join("momos-music-manager").join("config.toml");
            // Avoid duplicate if ~/.config/ happens to resolve to the same path
            if !paths.contains(&os_path) {
                paths.push(os_path);
            }
        }

        paths
    }

    /// Try to parse the TOML file. Returns an empty config on error/missing.
    /// Checks candidate paths in priority order; first readable file wins.
    fn load_toml() -> TomlConfig {
        for path in &Self::config_paths() {
            if !path.exists() {
                continue;
            }
            match std::fs::read_to_string(path) {
                Ok(content) => match toml::from_str::<TomlConfig>(&content) {
                    Ok(cfg) => {
                        info!("Loaded config from {}", path.display());
                        return cfg;
                    }
                    Err(e) => {
                        warn!("Failed to parse {}: {e} — trying next path", path.display());
                    }
                },
                Err(e) => {
                    warn!("Failed to read {}: {e} — trying next path", path.display());
                }
            }
        }

        warn!(
            "No config.toml found at ~/.config/momos-music-manager/config.toml or OS config dir — using env vars / defaults"
        );
        TomlConfig::default()
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

    #[allow(dead_code)]
    pub fn soundcloud_api_key(&self) -> anyhow::Result<&str> {
        self.soundcloud_api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("SoundCloud API key not configured (set in config.toml or SOUNDCLOUD_API_KEY env var)"))
    }

    #[allow(dead_code)]
    pub fn youtube_api_key(&self) -> anyhow::Result<&str> {
        self.youtube_api_key.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "YouTube API key not configured (set in config.toml or YOUTUBE_API_KEY env var)"
            )
        })
    }

    /// Create a default ServiceCredentials for testing (no real credentials).
    pub fn defaults_for_test() -> Self {
        Self {
            spotify_client_id: None,
            spotify_client_secret: None,
            spotify_redirect_uri: "http://localhost:3000/callback".to_string(),
            soundcloud_api_key: None,
            soundcloud_user_id: None,
            youtube_api_key: None,
            youtube_playlist_id: None,
            database_url: "sqlite::memory:".to_string(),
            server_host: "127.0.0.1".to_string(),
            server_port: 3000,
            server_public_url: None,
            global_poll_interval_secs: 0,
            cold_start_threshold_secs: 86400,
            maintainer_interval_secs: 0,
            maintainer_full_scan_max_age_secs: 86400,
            maintainer_backup_discovery_interval_secs: 604800,
            maintainer_auto_prune: false,
            maintainer_auto_cleanup_dirs: false,
            maintainer_traktor_import_enabled: false,
        }
    }
}

// ── Helper functions ───────────────────────────────────────────────────────

/// Read a required env var.
#[allow(dead_code)]
fn env_var(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("Missing required environment variable: {}", name))
}

/// Read an optional env var.
#[allow(dead_code)]
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

/// Return the env var (as a port number) if set, otherwise fall back to the TOML value.
fn env_or_toml_port(name: &str, toml_value: Option<u16>) -> Option<u16> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v.parse::<u16>().ok(),
        Ok(_) => toml_value,
        Err(_) => toml_value,
    }
}

/// Built-in default database URL with `~` expanded to the home directory.
fn default_database_url() -> String {
    let expanded = shellexpand::tilde("~/.local/share/momos-music-manager/library.db");
    format!("sqlite:{}", expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ServiceCredentials defaults ────────────────────────────────

    #[test]
    fn test_defaults_for_test() {
        let creds = ServiceCredentials::defaults_for_test();
        assert_eq!(creds.server_host, "127.0.0.1");
        assert_eq!(creds.server_port, 3000);
        assert_eq!(creds.database_url, "sqlite::memory:");
        assert_eq!(creds.global_poll_interval_secs, 0);
        assert_eq!(creds.cold_start_threshold_secs, 86400);
        assert_eq!(creds.maintainer_interval_secs, 0);
        assert_eq!(creds.maintainer_full_scan_max_age_secs, 86400);
        assert_eq!(creds.maintainer_backup_discovery_interval_secs, 604800);
        assert!(creds.spotify_client_id.is_none());
        assert!(creds.spotify_client_secret.is_none());
        assert_eq!(creds.spotify_redirect_uri, "http://localhost:3000/callback");
        assert!(creds.soundcloud_api_key.is_none());
        assert!(creds.soundcloud_user_id.is_none());
        assert!(creds.youtube_api_key.is_none());
        assert!(creds.youtube_playlist_id.is_none());
        assert!(creds.server_public_url.is_none());
    }

    // ── Configured checks ───────────────────────────────────────────

    #[test]
    fn test_is_spotify_configured_with_id_and_secret() {
        let creds = ServiceCredentials {
            spotify_client_id: Some("test-id".to_string()),
            spotify_client_secret: Some("test-secret".to_string()),
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(creds.is_spotify_configured());
    }

    #[test]
    fn test_is_spotify_not_configured_without_secret() {
        let creds = ServiceCredentials {
            spotify_client_id: Some("test-id".to_string()),
            spotify_client_secret: None,
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(!creds.is_spotify_configured());
    }

    #[test]
    fn test_is_spotify_not_configured_without_id() {
        let creds = ServiceCredentials {
            spotify_client_id: None,
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(!creds.is_spotify_configured());
    }

    #[test]
    fn test_is_soundcloud_configured_with_key() {
        let creds = ServiceCredentials {
            soundcloud_api_key: Some("sc-key".to_string()),
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(creds.is_soundcloud_configured());
    }

    #[test]
    fn test_is_soundcloud_not_configured_without_key() {
        let creds = ServiceCredentials {
            soundcloud_api_key: None,
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(!creds.is_soundcloud_configured());
    }

    #[test]
    fn test_is_youtube_configured_with_key() {
        let creds = ServiceCredentials {
            youtube_api_key: Some("yt-key".to_string()),
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(creds.is_youtube_configured());
    }

    #[test]
    fn test_is_youtube_not_configured_without_key() {
        let creds = ServiceCredentials {
            youtube_api_key: None,
            ..ServiceCredentials::defaults_for_test()
        };
        assert!(!creds.is_youtube_configured());
    }

    // ── Helper functions ─────────────────────────────────────────────

    #[test]
    fn test_env_or_toml_prefers_env() {
        unsafe { std::env::set_var("TEST_OR_TOML", "from-env") };
        let result = env_or_toml("TEST_OR_TOML", Some("from-toml".to_string()));
        unsafe { std::env::remove_var("TEST_OR_TOML") };
        assert_eq!(result, Some("from-env".to_string()));
    }

    #[test]
    fn test_env_or_toml_falls_back_to_toml() {
        unsafe { std::env::remove_var("TEST_OR_TOML_FALLBACK") };
        let result = env_or_toml("TEST_OR_TOML_FALLBACK", Some("from-toml".to_string()));
        assert_eq!(result, Some("from-toml".to_string()));
    }

    #[test]
    fn test_env_or_toml_returns_none_when_unset() {
        unsafe { std::env::remove_var("TEST_OR_TOML_NONE") };
        let result = env_or_toml("TEST_OR_TOML_NONE", None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_env_or_toml_empty_env_treated_as_unset() {
        unsafe { std::env::set_var("TEST_OR_TOML_EMPTY", "") };
        let result = env_or_toml("TEST_OR_TOML_EMPTY", Some("fallback".to_string()));
        unsafe { std::env::remove_var("TEST_OR_TOML_EMPTY") };
        assert_eq!(result, Some("fallback".to_string()));
    }

    #[test]
    fn test_env_or_toml_opt_prefers_env() {
        unsafe { std::env::set_var("TEST_OR_TOML_OPT", "from-env") };
        let result = env_or_toml_opt("TEST_OR_TOML_OPT", Some("from-toml".to_string()));
        unsafe { std::env::remove_var("TEST_OR_TOML_OPT") };
        assert_eq!(result, Some("from-env".to_string()));
    }

    #[test]
    fn test_env_or_toml_opt_empty_env_clears() {
        unsafe { std::env::set_var("TEST_OR_TOML_OPT_CLEAR", "") };
        let result = env_or_toml_opt("TEST_OR_TOML_OPT_CLEAR", Some("from-toml".to_string()));
        unsafe { std::env::remove_var("TEST_OR_TOML_OPT_CLEAR") };
        assert_eq!(result, None);
    }

    #[test]
    fn test_env_or_toml_port_prefers_env() {
        unsafe { std::env::set_var("TEST_PORT", "8080") };
        let result = env_or_toml_port("TEST_PORT", Some(3000));
        unsafe { std::env::remove_var("TEST_PORT") };
        assert_eq!(result, Some(8080));
    }

    #[test]
    fn test_env_or_toml_port_invalid_env_returns_none() {
        unsafe { std::env::set_var("TEST_PORT_INVALID", "not-a-port") };
        let result = env_or_toml_port("TEST_PORT_INVALID", Some(3000));
        unsafe { std::env::remove_var("TEST_PORT_INVALID") };
        // Invalid port parse returns None (does not fall back)
        assert_eq!(result, None);
    }

    #[test]
    fn test_env_or_toml_port_falls_back() {
        unsafe { std::env::remove_var("TEST_PORT_FALLBACK") };
        let result = env_or_toml_port("TEST_PORT_FALLBACK", Some(9000));
        assert_eq!(result, Some(9000));
    }

    #[test]
    fn test_credential_source_env() {
        unsafe { std::env::set_var("TEST_CRED_SRC", "val") };
        let src = ServiceCredentials::credential_source("TEST_CRED_SRC", Some("val"), true);
        unsafe { std::env::remove_var("TEST_CRED_SRC") };
        assert_eq!(src, "env");
    }

    #[test]
    fn test_credential_source_toml() {
        unsafe { std::env::remove_var("TEST_CRED_SRC_TOML") };
        let src = ServiceCredentials::credential_source("TEST_CRED_SRC_TOML", Some("val"), true);
        assert_eq!(src, "toml");
    }

    #[test]
    fn test_credential_source_default() {
        unsafe { std::env::remove_var("TEST_CRED_SRC_DEF") };
        let src = ServiceCredentials::credential_source("TEST_CRED_SRC_DEF", Some("val"), false);
        assert_eq!(src, "default");
    }

    #[test]
    fn test_credential_source_missing() {
        unsafe { std::env::remove_var("TEST_CRED_SRC_MISS") };
        let src = ServiceCredentials::credential_source("TEST_CRED_SRC_MISS", None, false);
        assert_eq!(src, "missing");
    }

    #[test]
    fn test_env_var_found() {
        unsafe { std::env::set_var("TEST_ENV_FOUND", "hello") };
        let result = env_var("TEST_ENV_FOUND");
        unsafe { std::env::remove_var("TEST_ENV_FOUND") };
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_env_var_missing() {
        unsafe { std::env::remove_var("TEST_ENV_MISSING_404") };
        let result = env_var("TEST_ENV_MISSING_404");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("TEST_ENV_MISSING_404")
        );
    }

    #[test]
    fn test_env_var_optional_found() {
        unsafe { std::env::set_var("TEST_ENV_OPT_FOUND", "opt-value") };
        let result = env_var_optional("TEST_ENV_OPT_FOUND");
        unsafe { std::env::remove_var("TEST_ENV_OPT_FOUND") };
        assert_eq!(result, Some("opt-value".to_string()));
    }

    #[test]
    fn test_env_var_optional_missing() {
        unsafe { std::env::remove_var("TEST_ENV_OPT_MISSING") };
        let result = env_var_optional("TEST_ENV_OPT_MISSING");
        assert_eq!(result, None);
    }

    #[test]
    fn test_env_var_optional_empty() {
        unsafe { std::env::set_var("TEST_ENV_OPT_EMPTY", "") };
        let result = env_var_optional("TEST_ENV_OPT_EMPTY");
        unsafe { std::env::remove_var("TEST_ENV_OPT_EMPTY") };
        assert_eq!(result, None, "empty string should be treated as unset");
    }

    #[test]
    fn test_config_paths_returns_paths() {
        let paths = ServiceCredentials::config_paths();
        assert!(
            !paths.is_empty(),
            "config_paths should return at least one path"
        );
        for path in &paths {
            let s = path.to_string_lossy();
            assert!(
                s.ends_with("config.toml"),
                "each path should end with config.toml, got: {}",
                s
            );
            assert!(
                s.contains("momos-music-manager"),
                "each path should contain momos-music-manager, got: {}",
                s
            );
        }
    }

    #[test]
    fn test_default_database_url_format() {
        let url = default_database_url();
        assert!(url.starts_with("sqlite:"));
        assert!(url.contains("momos-music-manager"));
        assert!(url.contains("library.db"));
    }

    // ── New edge-case tests ────────────────────────────────────────────

    #[test]
    fn test_env_or_toml_port_invalid_number() {
        // Non-numeric string should fail to parse as u16
        unsafe { std::env::set_var("TEST_PORT_NON_NUMERIC", "not-a-number") };
        let result = env_or_toml_port("TEST_PORT_NON_NUMERIC", Some(8080));
        unsafe { std::env::remove_var("TEST_PORT_NON_NUMERIC") };
        assert_eq!(result, None);
    }

    #[test]
    fn test_env_or_toml_port_out_of_range() {
        // A number larger than u16::MAX (65535) should fail to parse
        unsafe { std::env::set_var("TEST_PORT_OOR", "99999") };
        let result = env_or_toml_port("TEST_PORT_OOR", Some(3000));
        unsafe { std::env::remove_var("TEST_PORT_OOR") };
        assert_eq!(result, None);
    }

    #[test]
    fn test_env_or_toml_mixed_env_and_toml_priority() {
        // Set one env var, leave another unset.
        // The set one should come from env, the unset one should fall back to toml.
        unsafe { std::env::set_var("TEST_MIXED_HOST", "from-env-host") };
        unsafe { std::env::remove_var("TEST_MIXED_PORT") };

        let host = env_or_toml("TEST_MIXED_HOST", Some("from-toml-host".to_string()));
        let port = env_or_toml_port("TEST_MIXED_PORT", Some(9999));

        unsafe { std::env::remove_var("TEST_MIXED_HOST") };

        assert_eq!(host, Some("from-env-host".to_string()));
        assert_eq!(port, Some(9999));
    }

    #[test]
    fn test_credential_source_does_not_leak_value() {
        // credential_source returns "env"/"toml"/"default"/"missing", never the actual value
        unsafe { std::env::set_var("TEST_SECRET_KEY", "super-secret-value") };
        let src = ServiceCredentials::credential_source(
            "TEST_SECRET_KEY",
            Some("super-secret-value"),
            true,
        );
        unsafe { std::env::remove_var("TEST_SECRET_KEY") };
        assert_eq!(src, "env");
        assert_ne!(src, "super-secret-value");

        // When env is not set, source should be "toml" (because has_toml=true)
        unsafe { std::env::remove_var("TEST_SECRET_KEY_2") };
        let src2 =
            ServiceCredentials::credential_source("TEST_SECRET_KEY_2", Some("toml-value"), true);
        assert_eq!(src2, "toml");
        assert_ne!(src2, "toml-value");

        // When no env and no toml, source should be "missing"
        let src3 = ServiceCredentials::credential_source("TEST_SECRET_KEY_3", None, false);
        assert_eq!(src3, "missing");
    }

    #[test]
    fn test_bool_env_var_true() {
        // Set an env to "true" and verify it parses as true
        unsafe { std::env::set_var("TEST_BOOL_TRUE", "true") };
        let val = std::env::var("TEST_BOOL_TRUE")
            .ok()
            .filter(|v| !v.is_empty());
        unsafe { std::env::remove_var("TEST_BOOL_TRUE") };
        assert_eq!(val, Some("true".to_string()));

        // Verify round-trip through env_or_toml
        unsafe { std::env::set_var("TEST_BOOL_TRUE_2", "true") };
        let result = env_or_toml("TEST_BOOL_TRUE_2", Some("false".to_string()));
        unsafe { std::env::remove_var("TEST_BOOL_TRUE_2") };
        assert_eq!(result, Some("true".to_string()));
    }

    #[test]
    fn test_bool_env_var_false() {
        // Set an env to "false" and verify it's treated as a valid value
        unsafe { std::env::set_var("TEST_BOOL_FALSE", "false") };
        let val = std::env::var("TEST_BOOL_FALSE")
            .ok()
            .filter(|v| !v.is_empty());
        unsafe { std::env::remove_var("TEST_BOOL_FALSE") };
        assert_eq!(val, Some("false".to_string()));

        // Verify round-trip through env_or_toml
        unsafe { std::env::set_var("TEST_BOOL_FALSE_2", "false") };
        let result = env_or_toml("TEST_BOOL_FALSE_2", Some("true".to_string()));
        unsafe { std::env::remove_var("TEST_BOOL_FALSE_2") };
        assert_eq!(result, Some("false".to_string()));
    }
}
