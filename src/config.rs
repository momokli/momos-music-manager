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
//!
//! [telemetry]
//! enabled = true                  # master flag for snapshot push AND event telemetry
//! # Full-DB snapshot push (periodic whole-DB send) — OFF by default.
//! # Sends the COMPLETE DB (VACUUM INTO snapshot + meta) to `base_url`
//! # every N seconds. 0 = off (periodic; one-shot via `telemetry push` CLI
//! # still works). Legacy `interval_secs` (analytics era) stays effective
//! # when this key is absent/0 — this explicit key wins when set.
//! full_db_interval_secs = 0
//! # Where event batches are POSTed. Defaults to `<base_url>/api/telemetry`
//! # when unset (snapshot `base_url` configured).
//! events_endpoint = "https://telemetry.music.klimk.es/api/telemetry"
//!
//! [telemetry_receiver]
//! bind = "127.0.0.1:8330"
//! base_dir = "~/.local/share/momos-music-manager/analytics"
//! token = "secret-collector-token"
//! # Event telemetry.db (default: <base_dir>/telemetry.db) + retention.
//! # db_path = "~/.local/share/momos-music-manager/analytics/telemetry.db"
//! # retention_days = 30
//!
//! [autoupdate]
//! enabled = true
//! # Default base_url is channel-dependent (see docs/versioning.md):
//! # dev builds -> latest-main, release builds -> releases/latest/download.
//! # Only set this to override the channel default.
//! base_url = "https://github.com/momokli/momos-music-manager/releases/download/latest-main"
//! health_grace_secs = 60
//! # Seconds between two automatic check+apply cycles (default 14400 = 4 h;
//! # 0 disables the periodic auto-apply loop — the startup check still runs).
//! interval_secs = 14400
//! # macOS only: app install directory for the DMG self-install
//! # (default /Applications).
//! app_dir = "/Applications"
//! ```
//!
//! # Backward compatibility
//!
//! A plain `.env` file in the working directory or `SPOTIFY_CLIENT_ID` etc. set
//! directly in the environment still work and **override** the TOML file.
//! This lets you keep secrets in `.env` (gitignored) during local development
//! while the TOML file is the canonical source for production / daily use.

use std::path::{Path, PathBuf};

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
    telemetry: Option<TelemetryToml>,
    telemetry_receiver: Option<TelemetryReceiverToml>,
    autoupdate: Option<AutoupdateToml>,
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

#[derive(Debug, Clone, Deserialize)]
struct TelemetryToml {
    enabled: Option<bool>,
    base_url: Option<String>,
    token: Option<String>,
    instance: Option<String>,
    /// Legacy full-DB push interval (analytics era; still honored).
    interval_secs: Option<u64>,
    /// Explicit full-DB snapshot push interval (0/absent = off, default).
    /// Sends the complete DB (VACUUM INTO) + meta to `base_url` every N
    /// seconds. Wins over legacy `interval_secs` when > 0.
    full_db_interval_secs: Option<u64>,
    /// Event-batch endpoint; defaults to `<base_url>/api/telemetry` when unset.
    events_endpoint: Option<String>,
}

impl TelemetryToml {
    /// Whether any of the UI-managed keys is set in `[telemetry]`.
    fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.base_url.is_none()
            && self.token.is_none()
            && self.instance.is_none()
            && self.interval_secs.is_none()
            && self.full_db_interval_secs.is_none()
            && self.events_endpoint.is_none()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TelemetryReceiverToml {
    bind: Option<String>,
    base_dir: Option<String>,
    token: Option<String>,
    /// Where the event telemetry.db lives (default: <base_dir>/telemetry.db).
    db_path: Option<String>,
    /// How many days events are kept before pruning (default 30).
    retention_days: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct AutoupdateToml {
    enabled: Option<bool>,
    base_url: Option<String>,
    health_grace_secs: Option<u64>,
    /// Update channel (`"rolling"` | `"release"`) — optional; default is
    /// the running build's embedded channel.
    channel: Option<String>,
    /// Seconds between two automatic check+apply cycles (`0` disables the
    /// periodic loop; the startup check still runs). Default 4 h.
    interval_secs: Option<u64>,
    /// macOS only: directory whose `Momo's Music Manager.app` the DMG
    /// self-install replaces (default `/Applications`).
    app_dir: Option<String>,
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

    // Telemetry (client push)
    pub telemetry_enabled: bool,
    pub telemetry_base_url: Option<String>,
    pub telemetry_token: Option<String>,
    pub telemetry_instance: String,
    /// Legacy periodic full-DB push interval (analytics era; 0 = off).
    /// Kept as backward-compatible alias — see `telemetry_full_db_interval_secs`.
    pub telemetry_interval_secs: u64,
    /// Explicit full-DB snapshot push interval in seconds (default 0 = OFF).
    /// When > 0 (and `telemetry_enabled`) the app periodically sends the
    /// COMPLETE DB (VACUUM INTO snapshot + meta) to `telemetry_base_url`.
    /// Env: `MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS`; TOML:
    /// `[telemetry] full_db_interval_secs`. Wins over legacy `interval_secs`.
    pub telemetry_full_db_interval_secs: u64,
    /// HTTPS endpoint for event batches (`POST /api/telemetry`). Resolved
    /// Env > TOML > derived default `<base_url>/api/telemetry` > None.
    pub telemetry_events_endpoint: Option<String>,

    /// Raw `[telemetry]` section of the loaded config.toml (if any) — the
    /// mirror the Settings UI uses to report where a value comes from
    /// (env vs toml vs default) and to persist UI edits into the file
    /// (see [`update_telemetry_toml`]). Never populated for env-only
    /// loads ([`ServiceCredentials::from_env`]).
    pub(crate) telemetry_toml: Option<TelemetryToml>,

    // Telemetry receiver (collector)
    pub telemetry_receiver_bind: String,
    pub telemetry_receiver_base_dir: String,
    pub telemetry_receiver_token: Option<String>,
    /// Event telemetry.db path (default: `<base_dir>/telemetry.db`).
    pub telemetry_receiver_db_path: String,
    /// Event retention in days (default 30).
    pub telemetry_receiver_retention_days: i64,

    // Autoupdater (M6)
    pub autoupdate_enabled: bool,
    pub autoupdate_base_url: String,
    pub autoupdate_health_grace_secs: u64,
    /// Effective auto-apply interval (env > TOML > default 4 h). The UI/DB
    /// layer sits on top — see `autoupdate::update_auto`.
    pub autoupdate_interval_secs: u64,
    /// Raw `[autoupdate] interval_secs` value from config.toml — needed for
    /// `intervalSource` resolution (the TOML struct is private).
    pub(crate) autoupdate_interval_toml: Option<u64>,
    /// macOS app install directory for the DMG self-install
    /// (`MOMOS_AUTOUPDATE_APP_DIR` / `[autoupdate] app_dir`); `None` →
    /// default `/Applications`.
    pub autoupdate_app_dir: Option<std::path::PathBuf>,
    /// Whether `[autoupdate] enabled` was set explicitly in config.toml —
    /// needed for `enabledSource` detection (the TOML struct is private).
    pub(crate) autoupdate_has_toml: bool,
    /// Raw `[autoupdate] channel` value from config.toml (may be unparseable;
    /// then the running build's embedded channel applies). Needed for
    /// `channelSource`/value resolution without re-parsing the TOML file.
    pub(crate) autoupdate_channel_toml: Option<String>,
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
        // Raw `[telemetry]` mirror for the Settings UI (source reporting +
        // persistence target) — cloned before the field borrows below.
        let telemetry_toml = toml_config.telemetry.clone();

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

        // Telemetry resolution: env > toml > defaults. `events_endpoint`
        // falls back to `<base_url>/api/telemetry` when the base URL is set.
        let telemetry_base_url = env_or_toml_opt(
            "MOMOS_TELEMETRY_BASE_URL",
            toml_config
                .telemetry
                .as_ref()
                .and_then(|t| t.base_url.clone()),
        );
        let telemetry_events_endpoint = resolve_events_endpoint(
            env_or_toml_opt(
                "MOMOS_TELEMETRY_EVENTS_ENDPOINT",
                toml_config
                    .telemetry
                    .as_ref()
                    .and_then(|t| t.events_endpoint.clone()),
            ),
            telemetry_base_url.as_deref(),
        );

        // Telemetry receiver: resolve base_dir first so the default db_path
        // derives from the *effective* base_dir (env/toml aware).
        let telemetry_receiver_base_dir = env_or_toml(
            "MOMOS_TELEMETRY_RECEIVER_BASE_DIR",
            toml_config
                .telemetry_receiver
                .as_ref()
                .and_then(|r| r.base_dir.clone()),
        )
        .unwrap_or_else(default_telemetry_base_dir);
        let telemetry_receiver_db_path = env_or_toml(
            "MOMOS_TELEMETRY_RECEIVER_DB_PATH",
            toml_config
                .telemetry_receiver
                .as_ref()
                .and_then(|r| r.db_path.clone()),
        )
        .unwrap_or_else(|| format!("{}/telemetry.db", telemetry_receiver_base_dir.trim_end_matches('/')));

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
            .unwrap_or(86400), // 1 day default

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

            // Telemetry (client push)
            telemetry_enabled: std::env::var("MOMOS_TELEMETRY_ENABLED")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .or_else(|| toml_config.telemetry.as_ref().and_then(|t| t.enabled))
                .unwrap_or(false),

            telemetry_base_url: telemetry_base_url.clone(),
            telemetry_token: env_or_toml_opt(
                "MOMOS_TELEMETRY_TOKEN",
                toml_config.telemetry.as_ref().and_then(|t| t.token.clone()),
            ),
            telemetry_instance: env_or_toml(
                "MOMOS_TELEMETRY_INSTANCE",
                toml_config
                    .telemetry
                    .as_ref()
                    .and_then(|t| t.instance.clone()),
            )
            .unwrap_or_else(|| "macbook".to_string()),

            telemetry_interval_secs: std::env::var("MOMOS_TELEMETRY_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| toml_config.telemetry.as_ref().and_then(|t| t.interval_secs))
                .unwrap_or(0),
            telemetry_full_db_interval_secs: std::env::var("MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .telemetry
                        .as_ref()
                        .and_then(|t| t.full_db_interval_secs)
                })
                .unwrap_or(0),
            telemetry_events_endpoint: telemetry_events_endpoint,
            telemetry_toml,

            // Telemetry receiver (collector)
            telemetry_receiver_bind: env_or_toml(
                "MOMOS_TELEMETRY_RECEIVER_BIND",
                toml_config
                    .telemetry_receiver
                    .as_ref()
                    .and_then(|r| r.bind.clone()),
            )
            .unwrap_or_else(|| "127.0.0.1:8330".to_string()),

            telemetry_receiver_base_dir: telemetry_receiver_base_dir,
            telemetry_receiver_token: env_or_toml_opt(
                "MOMOS_TELEMETRY_RECEIVER_TOKEN",
                toml_config
                    .telemetry_receiver
                    .as_ref()
                    .and_then(|r| r.token.clone()),
            ),

            telemetry_receiver_db_path: telemetry_receiver_db_path,

            telemetry_receiver_retention_days: std::env::var("MOMOS_TELEMETRY_RECEIVER_RETENTION_DAYS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .or_else(|| {
                    toml_config
                        .telemetry_receiver
                        .as_ref()
                        .and_then(|r| r.retention_days)
                })
                .unwrap_or(30),

            // Autoupdater (M6): env var > config.toml > built-in default.
            // The default base URL is channel-dependent (dev → latest-main,
            // release → releases/latest/download) and is resolved in
            // `autoupdate::UpdateSettings::from_config` when the value still
            // equals the built-in dev default (see docs/versioning.md).
            autoupdate_enabled: std::env::var("MOMOS_AUTOUPDATE_ENABLED")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .or_else(|| toml_config.autoupdate.as_ref().and_then(|a| a.enabled))
                .unwrap_or(true),
            autoupdate_base_url: env_or_toml(
                "MOMOS_AUTOUPDATE_BASE_URL",
                toml_config
                    .autoupdate
                    .as_ref()
                    .and_then(|a| a.base_url.clone()),
            )
            .unwrap_or_else(|| crate::autoupdate::DEFAULT_BASE_URL.to_string()),
            autoupdate_health_grace_secs: std::env::var("MOMOS_AUTOUPDATE_HEALTH_GRACE_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .autoupdate
                        .as_ref()
                        .and_then(|a| a.health_grace_secs)
                })
                .unwrap_or(crate::autoupdate::DEFAULT_HEALTH_GRACE_SECS),
            // Auto-apply interval (Phase C): env > toml > default 4 h. The
            // UI layer sits on top (DB setting) — see
            // `autoupdate::update_auto::effective_auto_apply_interval`.
            autoupdate_interval_secs: std::env::var("MOMOS_AUTOUPDATE_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    toml_config
                        .autoupdate
                        .as_ref()
                        .and_then(|a| a.interval_secs)
                })
                .unwrap_or(crate::autoupdate::DEFAULT_AUTO_APPLY_INTERVAL_SECS),
            autoupdate_interval_toml: toml_config
                .autoupdate
                .as_ref()
                .and_then(|a| a.interval_secs),
            // macOS app install directory for the DMG self-install (Phase C):
            // env > toml > default `/Applications` (resolved in
            // `autoupdate::macos::default_app_dir`).
            autoupdate_app_dir: env_var_optional("MOMOS_AUTOUPDATE_APP_DIR")
                .map(PathBuf::from)
                .or_else(|| {
                    toml_config
                        .autoupdate
                        .as_ref()
                        .and_then(|a| a.app_dir.clone())
                        .map(PathBuf::from)
                }),
            autoupdate_has_toml: toml_config
                .autoupdate
                .as_ref()
                .and_then(|a| a.enabled)
                .is_some(),
            autoupdate_channel_toml: toml_config
                .autoupdate
                .as_ref()
                .and_then(|a| a.channel.clone()),
        };

        info!(
            "Autoupdate config: enabled={}, base_url={}, health_grace_secs={}s, channel={}, auto_apply_interval={}s, app_dir={:?}",
            credentials.autoupdate_enabled,
            credentials.autoupdate_base_url,
            credentials.autoupdate_health_grace_secs,
            credentials.autoupdate_channel_source(),
            credentials.autoupdate_interval_secs,
            credentials.autoupdate_app_dir,
        );

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

        info!(
            "Telemetry config: enabled={}, base_url={:?}, events_endpoint={:?}, instance={}, \
             full_db_interval={}s, legacy_interval={}s (receiver_bind={})",
            credentials.telemetry_enabled,
            credentials.telemetry_base_url,
            credentials.telemetry_events_endpoint,
            credentials.telemetry_instance,
            credentials.telemetry_full_db_interval_secs,
            credentials.telemetry_interval_secs,
            credentials.telemetry_receiver_bind,
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
            .unwrap_or(86400),

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

            telemetry_enabled: env_var_optional("MOMOS_TELEMETRY_ENABLED")
                .and_then(|v| v.parse().ok())
                .unwrap_or(false),
            telemetry_base_url: env_var_optional("MOMOS_TELEMETRY_BASE_URL"),
            telemetry_token: env_var_optional("MOMOS_TELEMETRY_TOKEN"),
            telemetry_instance: env_var_optional("MOMOS_TELEMETRY_INSTANCE")
                .unwrap_or_else(|| "macbook".to_string()),
            telemetry_interval_secs: env_var_optional("MOMOS_TELEMETRY_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            telemetry_full_db_interval_secs: env_var_optional("MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            telemetry_events_endpoint: resolve_events_endpoint(
                env_var_optional("MOMOS_TELEMETRY_EVENTS_ENDPOINT"),
                env_var_optional("MOMOS_TELEMETRY_BASE_URL").as_deref(),
            ),
            // Env-only load (tests/CI): no config.toml mirror.
            telemetry_toml: None,
            telemetry_receiver_bind: env_var_optional("MOMOS_TELEMETRY_RECEIVER_BIND")
                .unwrap_or_else(|| "127.0.0.1:8330".to_string()),
            telemetry_receiver_base_dir: env_var_optional("MOMOS_TELEMETRY_RECEIVER_BASE_DIR")
                .unwrap_or_else(default_telemetry_base_dir),
            telemetry_receiver_token: env_var_optional("MOMOS_TELEMETRY_RECEIVER_TOKEN"),
            telemetry_receiver_db_path: env_var_optional("MOMOS_TELEMETRY_RECEIVER_DB_PATH")
                .unwrap_or_else(|| format!("{}/telemetry.db", default_telemetry_base_dir().trim_end_matches('/'))),
            telemetry_receiver_retention_days: env_var_optional("MOMOS_TELEMETRY_RECEIVER_RETENTION_DAYS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),

            autoupdate_enabled: env_var_optional("MOMOS_AUTOUPDATE_ENABLED")
                .and_then(|v| v.parse().ok())
                .unwrap_or(true),
            autoupdate_base_url: env_var_optional("MOMOS_AUTOUPDATE_BASE_URL")
                .unwrap_or_else(|| crate::autoupdate::DEFAULT_BASE_URL.to_string()),
            autoupdate_health_grace_secs: env_var_optional("MOMOS_AUTOUPDATE_HEALTH_GRACE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::autoupdate::DEFAULT_HEALTH_GRACE_SECS),
            autoupdate_interval_secs: env_var_optional("MOMOS_AUTOUPDATE_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::autoupdate::DEFAULT_AUTO_APPLY_INTERVAL_SECS),
            autoupdate_interval_toml: None,
            autoupdate_app_dir: env_var_optional("MOMOS_AUTOUPDATE_APP_DIR").map(PathBuf::from),
            autoupdate_has_toml: false,
            autoupdate_channel_toml: None,
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

    /// Path of the config.toml that [`Self::load`] would read: the first
    /// *existing* candidate (XDG-style `~/.config/…` wins over the
    /// OS-native dir), else the preferred write location (first candidate).
    ///
    /// This is also the file the Settings UI persists into
    /// ([`update_telemetry_toml`]) — writing to the file the next `load()`
    /// actually reads keeps Env > TOML > Defaults intact.
    pub fn primary_config_toml_path() -> PathBuf {
        Self::config_paths()
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| {
                Self::config_paths()
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            })
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

    /// Where the configured `autoupdate_enabled` value comes from, ignoring
    /// the UI/DB layer: `"env"` (parseable `MOMOS_AUTOUPDATE_ENABLED`),
    /// `"toml"` (`[autoupdate] enabled` in config.toml) or `"default"`
    /// (built-in true). Mirrors the resolution in [`Self::load`] — an
    /// unparseable env value falls through to TOML/default.
    pub fn autoupdate_enabled_source(&self) -> &'static str {
        if std::env::var("MOMOS_AUTOUPDATE_ENABLED")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .is_some()
        {
            return "env";
        }
        if self.autoupdate_has_toml {
            return "toml";
        }
        "default"
    }

    /// Where the configured `autoupdate.channel` value comes from, ignoring
    /// the UI/DB layer: `"env"` (parseable `MOMOS_AUTOUPDATE_CHANNEL`),
    /// `"toml"` (`[autoupdate] channel` in config.toml) or `"default"`
    /// (running build's embedded channel). Mirrors
    /// [`Self::autoupdate_enabled_source`] — an unparseable env value falls
    /// through to TOML/default.
    pub fn autoupdate_channel_source(&self) -> &'static str {
        if configured_channel_env().is_some() {
            return "env";
        }
        if self.autoupdate_channel_toml.is_some() {
            return "toml";
        }
        "default"
    }

    /// Where the configured auto-apply interval comes from, ignoring the
    /// UI/DB layer: `"env"` (parseable `MOMOS_AUTOUPDATE_INTERVAL_SECS`),
    /// `"toml"` (`[autoupdate] interval_secs`) or `"default"`
    /// ([`crate::autoupdate::DEFAULT_AUTO_APPLY_INTERVAL_SECS`]). Mirrors
    /// [`Self::autoupdate_enabled_source`] — an unparseable env value falls
    /// through to TOML/default.
    pub fn autoupdate_interval_source(&self) -> &'static str {
        if std::env::var("MOMOS_AUTOUPDATE_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .is_some()
        {
            return "env";
        }
        if self.autoupdate_interval_toml.is_some() {
            return "toml";
        }
        "default"
    }

    /// Config-level update channel: parseable env value > `[autoupdate]
    /// channel` in config.toml > default = embedded channel of the running
    /// build (dev build → rolling, release build → release). Used by the CLI
    /// (`update check|apply`) and as the fallback in the API layer once env
    /// and UI values are absent.
    pub fn configured_autoupdate_channel(&self) -> crate::autoupdate::UpdateChannel {
        if let Some(channel) = configured_channel_env() {
            return channel;
        }
        if let Some(channel) = self
            .autoupdate_channel_toml
            .as_deref()
            .and_then(crate::autoupdate::UpdateChannel::parse)
        {
            return channel;
        }
        crate::autoupdate::UpdateChannel::for_version(env!("MMM_VERSION"))
    }

    // ── Telemetry Settings sources (env > TOML > default) ─────────────
    //
    // These mirror the resolution in [`Self::load`] and feed the Settings
    // UI: a value whose source is `"env"` is pinned by an environment
    // variable (the UI disables the control); `"toml"` values are
    // editable — the UI persists into the `[telemetry]` section of
    // config.toml (see [`update_telemetry_toml`]); `"default"` values
    // come from the built-in defaults (enabled=false, instance="macbook",
    // interval 0).

    /// Where the effective `telemetry_enabled` comes from: `"env"`
    /// (parseable `MOMOS_TELEMETRY_ENABLED`), `"toml"` (`[telemetry]
    /// enabled` in config.toml) or `"default"` (false). An unparseable env
    /// value falls through, mirroring the resolution in [`Self::load`].
    pub fn telemetry_enabled_source(&self) -> &'static str {
        if std::env::var("MOMOS_TELEMETRY_ENABLED")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .is_some()
        {
            return "env";
        }
        if self
            .telemetry_toml
            .as_ref()
            .and_then(|t| t.enabled)
            .is_some()
        {
            return "toml";
        }
        "default"
    }

    /// Where the effective `telemetry.base_url` comes from: `"env"` when
    /// `MOMOS_TELEMETRY_BASE_URL` is set (an empty value counts as set — it
    /// clears the TOML value, mirroring [`env_or_toml_opt`]), `"toml"` when
    /// `[telemetry] base_url` exists in config.toml, else `"default"`.
    pub fn telemetry_base_url_source(&self) -> &'static str {
        if std::env::var("MOMOS_TELEMETRY_BASE_URL").is_ok() {
            return "env";
        }
        if self
            .telemetry_toml
            .as_ref()
            .and_then(|t| t.base_url.as_ref())
            .is_some()
        {
            return "toml";
        }
        "default"
    }

    /// Where the effective `telemetry.token` comes from (analogous to
    /// [`Self::telemetry_base_url_source`]; env var `MOMOS_TELEMETRY_TOKEN`).
    pub fn telemetry_token_source(&self) -> &'static str {
        if std::env::var("MOMOS_TELEMETRY_TOKEN").is_ok() {
            return "env";
        }
        if self
            .telemetry_toml
            .as_ref()
            .and_then(|t| t.token.as_ref())
            .is_some()
        {
            return "toml";
        }
        "default"
    }

    /// Where the effective `telemetry.instance` comes from: `"env"` when
    /// `MOMOS_TELEMETRY_INSTANCE` is set to a non-empty value (an empty env
    /// value falls through, mirroring [`env_or_toml`]), `"toml"` when
    /// `[telemetry] instance` exists, else `"default"` ("macbook").
    pub fn telemetry_instance_source(&self) -> &'static str {
        if std::env::var("MOMOS_TELEMETRY_INSTANCE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
        {
            return "env";
        }
        if self
            .telemetry_toml
            .as_ref()
            .and_then(|t| t.instance.as_ref())
            .is_some()
        {
            return "toml";
        }
        "default"
    }

    /// Where the effective full-DB push interval comes from: `"env"` when
    /// `MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS` *or* the legacy
    /// `MOMOS_TELEMETRY_INTERVAL_SECS` is parseable, `"toml"` when
    /// `[telemetry] full_db_interval_secs` *or* the legacy `interval_secs`
    /// exists in config.toml, else `"default"` (0 = off).
    pub fn telemetry_interval_source(&self) -> &'static str {
        for name in [
            "MOMOS_TELEMETRY_FULL_DB_INTERVAL_SECS",
            "MOMOS_TELEMETRY_INTERVAL_SECS",
        ] {
            if std::env::var(name)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .is_some()
            {
                return "env";
            }
        }
        let toml = self.telemetry_toml.as_ref();
        if toml
            .and_then(|t| t.full_db_interval_secs)
            .is_some()
            || toml.and_then(|t| t.interval_secs).is_some()
        {
            return "toml";
        }
        "default"
    }

    /// Effective periodic full-DB push interval in seconds (0 = off) — the
    /// same value the telemetry loop uses (explicit key wins, legacy
    /// `interval_secs` stays effective as alias when the explicit one is
    /// unset/0; see [`crate::telemetry::effective_full_db_interval_secs`]).
    pub fn telemetry_effective_full_db_interval(&self) -> u64 {
        if self.telemetry_full_db_interval_secs > 0 {
            self.telemetry_full_db_interval_secs
        } else {
            self.telemetry_interval_secs
        }
    }

    /// Whether any `[telemetry]` value is present in the loaded config.toml
    /// (used by tests and by the API to explain pinning).
    pub(crate) fn telemetry_toml_present(&self) -> bool {
        self.telemetry_toml
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false)
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
            telemetry_enabled: false,
            telemetry_base_url: None,
            telemetry_token: None,
            telemetry_instance: "macbook".to_string(),
            telemetry_interval_secs: 0,
            telemetry_full_db_interval_secs: 0,
            telemetry_events_endpoint: None,
            telemetry_toml: None,
            telemetry_receiver_bind: "127.0.0.1:8330".to_string(),
            telemetry_receiver_base_dir: "/tmp/momos-analytics".to_string(),
            telemetry_receiver_token: None,
            telemetry_receiver_db_path: "/tmp/momos-analytics/telemetry.db".to_string(),
            telemetry_receiver_retention_days: 30,
            autoupdate_enabled: false,
            autoupdate_base_url: crate::autoupdate::DEFAULT_BASE_URL.to_string(),
            autoupdate_health_grace_secs: 5,
            autoupdate_interval_secs: crate::autoupdate::DEFAULT_AUTO_APPLY_INTERVAL_SECS,
            autoupdate_interval_toml: None,
            autoupdate_app_dir: None,
            autoupdate_has_toml: false,
            autoupdate_channel_toml: None,
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

/// Parseable `MOMOS_AUTOUPDATE_CHANNEL` env value, if any.
fn configured_channel_env() -> Option<crate::autoupdate::UpdateChannel> {
    std::env::var("MOMOS_AUTOUPDATE_CHANNEL")
        .ok()
        .and_then(|v| crate::autoupdate::UpdateChannel::parse(&v))
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

/// Resolve the event-batch endpoint: explicit value wins; otherwise derive
/// `<base_url>/api/telemetry` from the configured snapshot base URL; else None.
fn resolve_events_endpoint(explicit: Option<String>, base_url: Option<&str>) -> Option<String> {
    explicit.or_else(|| {
        base_url.map(|b| format!("{}/api/telemetry", b.trim_end_matches('/')))
    })
}

/// Same as `env_or_toml` but returns `Option<String>`.
fn env_or_toml_opt(name: &str, toml_value: Option<String>) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        Ok(_) => None, // explicitly emptied → clear
        Err(_) => toml_value,
    }
}

// ── config.toml persistence (Settings UI → `[telemetry]`) ─────────────────
//
// The Settings page writes the `[telemetry]` section of config.toml so the
// user never has to edit the file by hand. Precedence stays untouched:
// Env > TOML > Defaults (an env-pinned field is simply not editable in the
// UI). The writer below does *line surgery* instead of a full TOML
// round-trip: comments, unknown sections and values the UI does not manage
// (e.g. `[telemetry] events_endpoint` or the `[autoupdate]` section)
// survive byte-for-byte.

/// Keys of the `[telemetry]` section the Settings UI manages.
const TELEMETRY_TOML_SECTION: &str = "telemetry";
/// Managed `[telemetry]` keys (in the order they are written for new files).
const TELEMETRY_TOML_KEYS: &[&str] = &[
    "enabled",
    "base_url",
    "token",
    "instance",
    "full_db_interval_secs",
];
/// Legacy alias key that the UI silently retires when it writes
/// `full_db_interval_secs` (the explicit key is authoritative; keeping both
/// would make a UI "0 = off" save ineffective when a legacy value exists).
const TELEMETRY_TOML_LEGACY_INTERVAL_KEY: &str = "interval_secs";

/// Failure of a config.toml settings write.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum TelemetryTomlError {
    #[error("cannot write config.toml: {0}")]
    Io(#[from] std::io::Error),
    #[error("config.toml at {0} is not valid TOML — refusing to modify it: {1}")]
    InvalidToml(PathBuf, String),
}

/// New values for the `[telemetry]` section, written from the Settings UI.
///
/// `None` leaves the key untouched. String fields: `Some("")` deletes the
/// key (the effective value becomes the default / env override);
/// `Some(non-empty)` sets it.
#[derive(Debug, Clone, Default)]
#[allow(missing_docs)]
pub struct TelemetryTomlPatch {
    pub enabled: Option<bool>,
    pub base_url: Option<String>,
    pub token: Option<String>,
    pub instance: Option<String>,
    pub full_db_interval_secs: Option<u64>,
}

/// Patch the `[telemetry]` section of the config.toml that
/// [`ServiceCredentials::load`] reads (first existing candidate path, else
/// the preferred XDG location `~/.config/momos-music-manager/config.toml`).
///
/// Returns the path that was written. Creating the file when it does not
/// exist yet (including its parent directory).
pub fn update_telemetry_toml(
    patch: &TelemetryTomlPatch,
) -> Result<PathBuf, TelemetryTomlError> {
    let path = ServiceCredentials::primary_config_toml_path();
    update_telemetry_toml_at(&path, patch)?;
    Ok(path)
}

/// [`update_telemetry_toml`] at an explicit path (testable without touching
/// the real home directory).
pub fn update_telemetry_toml_at(
    path: &Path,
    patch: &TelemetryTomlPatch,
) -> Result<(), TelemetryTomlError> {
    // Read the current content (empty when the file does not exist yet).
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };

    // Never rewrite a file we cannot fully understand: validate the current
    // content parses as TOML (an empty file is a valid empty table).
    if !content.trim().is_empty() {
        if let Err(e) = content.parse::<toml::Value>() {
            return Err(TelemetryTomlError::InvalidToml(
                path.to_path_buf(),
                e.to_string(),
            ));
        }
    }

    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    // Split marker: the file content ends with a newline iff the last
    // element is empty (split keeps the trailing empty element).
    let ends_with_newline = lines.last().map(|l| l.is_empty()).unwrap_or(true);

    // Locate the `[telemetry]` section (header line = trimmed `[name]`).
    fn is_section_header(line: &str) -> bool {
        let t = line.trim();
        t.starts_with('[') && t.ends_with(']') && !t.starts_with("[[")
    }
    let section_header = format!("[{TELEMETRY_TOML_SECTION}]");
    let mut telemetry_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == section_header {
            telemetry_idx = Some(i);
            break;
        }
    }

    let mut section_end = match telemetry_idx {
        Some(idx) => {
            // End = next section header after [telemetry], else EOF.
            let mut end = lines.len();
            for (i, line) in lines.iter().enumerate().skip(idx + 1) {
                if is_section_header(line) {
                    end = i;
                    break;
                }
            }
            end
        }
        None => {
            // No [telemetry] yet — append it at the end (after a
            // separating blank line when the file has content).
            if !lines.is_empty()
                && !ends_with_newline
                && lines.iter().any(|l| !l.trim().is_empty())
            {
                lines.push(String::new());
            }
            lines.push(section_header);
            telemetry_idx = Some(lines.len() - 1);
            lines.len() // empty section, keys are inserted below
        }
    };

    // Token of a key line: text before the first `=` (trimmed). Comments
    // and non-key lines return None.
    fn line_key(line: &str) -> Option<&str> {
        let t = line.trim();
        if t.starts_with('#') {
            return None; // comment
        }
        t.split('=').next().map(str::trim).filter(|k| !k.is_empty())
    }

    // Find the line index of `key` inside the [telemetry] section.
    fn find_key(lines: &[String], start: usize, end: usize, key: &str) -> Option<usize> {
        lines[start..end]
            .iter()
            .position(|l| line_key(l) == Some(key))
            .map(|p| p + start)
    }

    // Value rendering: TOML-literal strings (proper quoting/escaping),
    // plain scalars otherwise.
    fn value_line(key: &str, value: &toml::Value) -> String {
        format!("{key} = {value}")
    }

    // Replace or insert one key inside the [telemetry] section. Missing
    // keys are appended at the section end (before the next header);
    // `section_end` tracks the insertion point.
    let start = telemetry_idx.expect("telemetry section index set") + 1;
    let mut set_key = |lines: &mut Vec<String>,
                       section_end: &mut usize,
                       key: &str,
                       value: &toml::Value| {
        match find_key(lines, start, *section_end, key) {
            Some(i) => lines[i] = value_line(key, value),
            None => {
                lines.insert(*section_end, value_line(key, value));
                *section_end += 1;
            }
        }
    };
    let mut remove_key = |lines: &mut Vec<String>, section_end: &mut usize, key: &str| {
        if let Some(i) = find_key(lines, start, *section_end, key) {
            lines.remove(i);
            *section_end -= 1;
        }
    };

    // 1. Enabled toggle.
    if let Some(v) = patch.enabled {
        set_key(&mut lines, &mut section_end, "enabled", &toml::Value::Boolean(v));
    }
    // 2. String fields — Some("") clears the key.
    for (key, value) in [
        ("base_url", patch.base_url.as_ref()),
        ("token", patch.token.as_ref()),
        ("instance", patch.instance.as_ref()),
    ] {
        match value {
            None => {}
            Some(v) if v.is_empty() => remove_key(&mut lines, &mut section_end, key),
            Some(v) => {
                set_key(
                    &mut lines,
                    &mut section_end,
                    key,
                    &toml::Value::String(v.clone()),
                );
            }
        }
    }
    // 3. Full-DB interval — writing it retires the legacy alias so the UI
    //    value stays authoritative (0 = off must mean off).
    if let Some(secs) = patch.full_db_interval_secs {
        remove_key(&mut lines, &mut section_end, TELEMETRY_TOML_LEGACY_INTERVAL_KEY);
        set_key(
            &mut lines,
            &mut section_end,
            "full_db_interval_secs",
            &toml::Value::Integer(secs as i64),
        );
    }

    let mut new_content = lines.join("\n");
    if !ends_with_newline && !new_content.is_empty() {
        new_content.push('\n');
    }

    // Validate the result before writing: it must stay valid TOML.
    if let Err(e) = new_content.parse::<toml::Value>() {
        return Err(TelemetryTomlError::InvalidToml(
            path.to_path_buf(),
            e.to_string(),
        ));
    }

    // Atomic write (temp + rename) so a crash never leaves a torn file.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &new_content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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

/// Built-in default telemetry collector directory.
fn default_telemetry_base_dir() -> String {
    shellexpand::tilde("~/.local/share/momos-music-manager/analytics").to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Serializes every test that reads/mutates `MOMOS_TELEMETRY_*` env
    /// vars (the source helpers read the process env live). Acquired by the
    /// telemetry source-helper tests here and by the Settings-API env-pin
    /// matrix (`api::telemetry_settings::tests`) — parallel tests inside
    /// one binary would otherwise race on the process-global environment.
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        // Full-DB snapshot option defaults OFF (0) for tests.
        assert_eq!(creds.telemetry_full_db_interval_secs, 0);
        assert_eq!(creds.telemetry_interval_secs, 0);
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

    // ── Telemetry TOML parsing (full-DB option) ───────────────────────

    #[test]
    fn test_telemetry_toml_full_db_interval_parses() {
        let src = "[telemetry]\nenabled = true\nbase_url = \"https://telemetry.example.com\"\nfull_db_interval_secs = 86400\n";
        let cfg: TomlConfig = toml::from_str(src).unwrap();
        let tel = cfg.telemetry.expect("telemetry section should parse");
        assert_eq!(tel.full_db_interval_secs, Some(86400));
        assert_eq!(tel.interval_secs, None);
    }

    #[test]
    fn test_telemetry_toml_full_db_interval_defaults_absent() {
        // Legacy analytics-era key must still parse; new key absent → None.
        let src = "[telemetry]\nenabled = true\ninterval_secs = 3600\n";
        let cfg: TomlConfig = toml::from_str(src).unwrap();
        let tel = cfg.telemetry.unwrap();
        assert_eq!(tel.interval_secs, Some(3600));
        assert_eq!(tel.full_db_interval_secs, None);
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
    fn defaults_for_test_events_endpoint_none() {
        let creds = ServiceCredentials::defaults_for_test();
        assert!(creds.telemetry_events_endpoint.is_none());
        assert!(!creds.telemetry_enabled);
    }

    #[test]
    fn resolve_events_endpoint_explicit_wins() {
        let r = resolve_events_endpoint(
            Some("https://explicit.example/api/telemetry".to_string()),
            Some("https://telemetry.example"),
        );
        assert_eq!(r.unwrap(), "https://explicit.example/api/telemetry");
    }

    #[test]
    fn resolve_events_endpoint_derives_from_base_url() {
        let r = resolve_events_endpoint(None, Some("https://telemetry.example"));
        assert_eq!(r.unwrap(), "https://telemetry.example/api/telemetry");

        // trailing slash must not produce a double slash
        let r = resolve_events_endpoint(None, Some("https://telemetry.example/"));
        assert_eq!(r.unwrap(), "https://telemetry.example/api/telemetry");
    }

    #[test]
    fn resolve_events_endpoint_none_without_base_url() {
        assert_eq!(resolve_events_endpoint(None, None), None);
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

    // ── Telemetry source helpers (env > toml > default) ──────────────
    //
    // The env branches are exercised via the Settings-API pin matrix
    // (api::telemetry_settings tests, single test fn because env mutation
    // must not overlap). These tests cover the toml/default branches with
    // a clean process env (CI never sets MOMOS_TELEMETRY_*).

    fn creds_with_toml(telemetry: Option<TelemetryToml>) -> ServiceCredentials {
        let mut creds = ServiceCredentials::defaults_for_test();
        // Mirror what `load()` resolves from the TOML section into the
        // runtime fields (env is not involved in these tests).
        if let Some(t) = &telemetry {
            creds.telemetry_enabled = t.enabled.unwrap_or(false);
            creds.telemetry_base_url = t.base_url.clone();
            creds.telemetry_token = t.token.clone();
            creds.telemetry_instance = t
                .instance
                .clone()
                .unwrap_or_else(|| "macbook".to_string());
            creds.telemetry_interval_secs = t.interval_secs.unwrap_or(0);
            creds.telemetry_full_db_interval_secs = t.full_db_interval_secs.unwrap_or(0);
            creds.telemetry_events_endpoint = match (&t.events_endpoint, &t.base_url) {
                (Some(e), _) => Some(e.clone()),
                (None, Some(u)) => Some(format!("{}/api/telemetry", u.trim_end_matches('/'))),
                (None, None) => None,
            };
        }
        creds.telemetry_toml = telemetry;
        creds
    }

    #[test]
    fn telemetry_sources_default_without_env_or_toml() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let creds = creds_with_toml(None);
        assert_eq!(creds.telemetry_enabled_source(), "default");
        assert_eq!(creds.telemetry_base_url_source(), "default");
        assert_eq!(creds.telemetry_token_source(), "default");
        assert_eq!(creds.telemetry_instance_source(), "default");
        assert_eq!(creds.telemetry_interval_source(), "default");
        assert_eq!(creds.telemetry_effective_full_db_interval(), 0);
        assert!(!creds.telemetry_toml_present());
    }

    #[test]
    fn telemetry_sources_toml_mirror() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let toml = TelemetryToml {
            enabled: Some(false),
            base_url: Some("https://collector.example".into()),
            token: Some("tok".into()),
            instance: Some("studio".into()),
            interval_secs: None,
            full_db_interval_secs: Some(3600),
            events_endpoint: None,
        };
        let creds = creds_with_toml(Some(toml));
        assert_eq!(creds.telemetry_enabled_source(), "toml");
        assert_eq!(creds.telemetry_base_url_source(), "toml");
        assert_eq!(creds.telemetry_token_source(), "toml");
        assert_eq!(creds.telemetry_instance_source(), "toml");
        assert_eq!(creds.telemetry_interval_source(), "toml");
        assert_eq!(creds.telemetry_effective_full_db_interval(), 3600);
        assert!(creds.telemetry_toml_present());
    }

    #[test]
    fn telemetry_sources_legacy_interval_alias() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Legacy `interval_secs` (no explicit key) keeps working and is
        // reported as toml source.
        let toml = TelemetryToml {
            enabled: Some(true),
            base_url: None,
            token: None,
            instance: None,
            interval_secs: Some(7200),
            full_db_interval_secs: None,
            events_endpoint: None,
        };
        let creds = creds_with_toml(Some(toml));
        assert_eq!(creds.telemetry_interval_source(), "toml");
        assert_eq!(creds.telemetry_effective_full_db_interval(), 7200);
    }

    #[test]
    fn telemetry_effective_prefers_explicit_over_legacy() {
        let creds = {
            let mut c = ServiceCredentials::defaults_for_test();
            c.telemetry_full_db_interval_secs = 60;
            c.telemetry_interval_secs = 7200;
            c
        };
        assert_eq!(creds.telemetry_effective_full_db_interval(), 60);
    }

    // ── [telemetry] config.toml writer ─────────────────────────────────

    const SAMPLE_TOML: &str = r#"# Momo's Music Manager config
# secrets stay here

[spotify]
client_id = "abc"
client_secret = "def"  # keep me

[telemetry]
# master flag for snapshot push AND event telemetry
enabled = true
base_url = "https://telemetry.example"
# interval below is the legacy alias
interval_secs = 3600
events_endpoint = "https://telemetry.example/api/telemetry"

[autoupdate]
enabled = true
"#;

    #[test]
    fn update_telemetry_toml_preserves_comments_and_other_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, SAMPLE_TOML).unwrap();

        let patch = TelemetryTomlPatch {
            enabled: Some(false),
            base_url: Some("https://new.example".into()),
            token: Some("tok-123".into()),
            instance: Some("studio".into()),
            full_db_interval_secs: Some(0),
        };
        update_telemetry_toml_at(&path, &patch).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        // Unrelated content survives byte-for-byte.
        assert!(out.contains("client_secret = \"def\"  # keep me"));
        assert!(out.contains("[autoupdate]\nenabled = true"));
        assert!(out.contains("# master flag for snapshot push AND event telemetry"));
        // events_endpoint (not managed by the UI) survives.
        assert!(out.contains("events_endpoint = \"https://telemetry.example/api/telemetry\""));
        // Legacy alias retired when the explicit key is written (the line
        // must be gone as a key — `full_db_interval_secs` stays).
        assert!(!out.lines().any(|l| {
            let t = l.trim();
            !t.starts_with('#') && t.split('=').next().map(str::trim) == Some("interval_secs")
        }), "legacy key must be gone: {out}");
        assert!(out.contains("full_db_interval_secs = 0"));

        // Values updated (0 = off must land as a real key).
        assert!(out.contains("enabled = false"));
        assert!(out.contains("base_url = \"https://new.example\""));
        assert!(out.contains("token = \"tok-123\""));
        assert!(out.contains("instance = \"studio\""));
        assert!(out.contains("full_db_interval_secs = 0"));

        // Result must parse + resolve like a fresh load would.
        let value: toml::Value = out.parse().expect("result must be valid TOML");
        let tel = &value["telemetry"];
        assert_eq!(tel["enabled"].as_bool(), Some(false));
        assert_eq!(tel["full_db_interval_secs"].as_integer(), Some(0));
        assert_eq!(value["spotify"]["client_id"].as_str(), Some("abc"));
    }

    #[test]
    fn update_telemetry_toml_appends_section_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[spotify]\nclient_id = \"abc\"\n",
        )
        .unwrap();

        let patch = TelemetryTomlPatch {
            enabled: Some(true),
            base_url: Some("https://t.example".into()),
            ..Default::default()
        };
        update_telemetry_toml_at(&path, &patch).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("[spotify]"));
        assert!(out.contains("[telemetry]"));
        let value: toml::Value = out.parse().expect("valid TOML");
        assert_eq!(value["telemetry"]["enabled"].as_bool(), Some(true));
        assert_eq!(value["telemetry"]["base_url"].as_str(), Some("https://t.example"));
    }

    #[test]
    fn update_telemetry_toml_creates_file_with_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("config.toml");
        assert!(!path.exists());

        let patch = TelemetryTomlPatch {
            enabled: Some(true),
            instance: Some("studio".into()),
            ..Default::default()
        };
        update_telemetry_toml_at(&path, &patch).unwrap();
        assert!(path.exists());
        let out = std::fs::read_to_string(&path).unwrap();
        let value: toml::Value = out.parse().expect("valid TOML");
        assert_eq!(value["telemetry"]["enabled"].as_bool(), Some(true));
        assert_eq!(value["telemetry"]["instance"].as_str(), Some("studio"));
        assert_eq!(value["telemetry"].get("base_url"), None);
    }

    #[test]
    fn update_telemetry_toml_clears_string_keys_with_empty_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[telemetry]\nbase_url = \"https://old.example\"\ntoken = \"t\"\ninstance = \"studio\"\n",
        )
        .unwrap();

        let patch = TelemetryTomlPatch {
            base_url: Some(String::new()),
            token: Some(String::new()),
            ..Default::default()
        };
        update_telemetry_toml_at(&path, &patch).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        let value: toml::Value = out.parse().expect("valid TOML");
        let tel = &value["telemetry"];
        assert_eq!(tel.get("base_url"), None);
        assert_eq!(tel.get("token"), None);
        assert_eq!(tel["instance"].as_str(), Some("studio"), "untouched key survives");
    }

    #[test]
    fn update_telemetry_toml_rejects_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = = not toml [[[").unwrap();

        let patch = TelemetryTomlPatch {
            enabled: Some(true),
            ..Default::default()
        };
        let err = update_telemetry_toml_at(&path, &patch).unwrap_err();
        assert!(matches!(err, TelemetryTomlError::InvalidToml(..)));
        // File untouched.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is = = not toml [[["
        );
    }

    #[test]
    fn update_telemetry_toml_escapes_string_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let patch = TelemetryTomlPatch {
            instance: Some("studio \"quoted\" ✓".into()),
            ..Default::default()
        };
        update_telemetry_toml_at(&path, &patch).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        let value: toml::Value = out.parse().expect("valid TOML");
        assert_eq!(value["telemetry"]["instance"].as_str(), Some("studio \"quoted\" ✓"));
    }
}
