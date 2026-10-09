use anyhow::{Result, bail};

/// Runtime configuration, all from env (with `.env` support via `dotenvy`).
#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub spotify_client_id: Option<String>,
    pub spotify_client_secret: Option<String>,
    pub spotify_redirect_uri: String,
    pub spotify_api_base: String,
    pub reccobeats_base: String,
    pub cosine_base: String,
    pub cosine_api_key: Option<String>,
    pub freqblog_base: String,
    pub freqblog_api_key: Option<String>,
    pub freqblog_monthly_cap: i64,
    pub effnet_model: Option<String>,
    pub effnet_labels: Option<String>,
    pub effnet_inprocess: bool,
    pub analyzer_base: Option<String>,
    /// Directory for temporary analysis audio files (shared with the analyzer
    /// service; needed when the service uses systemd `PrivateTmp`).
    pub analyze_tmp_dir: Option<String>,
    pub lastfm_api_key: Option<String>,
    pub soundcloud_client_id: Option<String>,
    pub soundcloud_client_secret: Option<String>,
    pub youtube_client_id: Option<String>,
    pub youtube_client_secret: Option<String>,
    pub music_api_base: String,
    pub music_api_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env("HUB_HOST").unwrap_or_else(|| "0.0.0.0".to_string()),
            port: env("HUB_PORT").and_then(|p| p.parse().ok()).unwrap_or(8080),
            database_url: env("HUB_DATABASE_URL").unwrap_or_else(|| "sqlite:hub.db".to_string()),
            spotify_client_id: env("SPOTIFY_CLIENT_ID"),
            spotify_client_secret: env("SPOTIFY_CLIENT_SECRET"),
            // Loopback literal is the ONLY http:// Spotify still allows — perfect
            // for a one-time local token grab without any public HTTPS.
            spotify_redirect_uri: env("SPOTIFY_REDIRECT_URI")
                .unwrap_or_else(|| "http://127.0.0.1:8888/callback".to_string()),
            spotify_api_base: env("SPOTIFY_API_BASE")
                .unwrap_or_else(|| crate::spotify::DEFAULT_API_BASE.to_string()),
            reccobeats_base: env("RECCOBEATS_BASE")
                .unwrap_or_else(|| "https://api.reccobeats.com/v1".to_string()),
            cosine_base: env("COSINE_BASE")
                .unwrap_or_else(|| "https://cosine.club/api/v1".to_string()),
            cosine_api_key: env("COSINECLUB_API"),
            freqblog_base: env("FREQBLOG_BASE")
                .unwrap_or_else(|| "https://api.freqblog.com".to_string()),
            freqblog_api_key: env("FREQBLOG_API"),
            // Stay safely under the 1,000/month free tier.
            freqblog_monthly_cap: env("FREQBLOG_MONTHLY_CAP")
                .and_then(|v| v.parse().ok())
                .unwrap_or(950),
            effnet_model: env("EFFNET_MODEL"),
            effnet_labels: env("EFFNET_LABELS"),
            // On CPUs without AVX the prebuilt ONNX Runtime SIGILLs; there the
            // analyzer service (Essentia) computes embeddings instead.
            effnet_inprocess: env("EFFNET_INPROCESS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            analyzer_base: env("HUB_ANALYZER_URL"),
            analyze_tmp_dir: env("HUB_ANALYZE_TMP"),
            lastfm_api_key: env("LASTFM_API_KEY"),
            soundcloud_client_id: env("SOUNDCLOUD_CLIENT_ID"),
            soundcloud_client_secret: env("SOUNDCLOUD_CLIENT_SECRET"),
            youtube_client_id: env("YOUTUBE_CLIENT_ID"),
            youtube_client_secret: env("YOUTUBE_CLIENT_SECRET"),
            music_api_base: env("MUSIC_API_BASE")
                .unwrap_or_else(|| "http://127.0.0.1:8710".to_string()),
            music_api_token: env("MUSIC_API_TOKEN"),
        }
    }

    /// A self-contained config for integration tests: only the DB URL matters.
    pub fn for_test(database_url: impl Into<String>) -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
            database_url: database_url.into(),
            spotify_client_id: None,
            spotify_client_secret: None,
            spotify_redirect_uri: "http://127.0.0.1:8888/callback".to_string(),
            spotify_api_base: crate::spotify::DEFAULT_API_BASE.to_string(),
            reccobeats_base: "https://api.reccobeats.com/v1".to_string(),
            cosine_base: "https://cosine.club/api/v1".to_string(),
            cosine_api_key: None,
            freqblog_base: "https://api.freqblog.com".to_string(),
            freqblog_api_key: None,
            freqblog_monthly_cap: 950,
            effnet_model: None,
            effnet_labels: None,
            effnet_inprocess: false,
            analyzer_base: None,
            analyze_tmp_dir: None,
            lastfm_api_key: None,
            soundcloud_client_id: None,
            soundcloud_client_secret: None,
            youtube_client_id: None,
            youtube_client_secret: None,
            music_api_base: "http://127.0.0.1:8710".to_string(),
            music_api_token: None,
        }
    }

    /// Overlay a DB setting (`hub_settings`) onto this config. Empty values and
    /// unknown keys are ignored, so env remains the fallback.
    pub fn apply(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        match key {
            crate::settings::LASTFM_API_KEY => self.lastfm_api_key = Some(value.into()),
            crate::settings::RECCOBEATS_BASE => self.reccobeats_base = value.into(),
            crate::settings::COSINE_BASE => self.cosine_base = value.into(),
            crate::settings::COSINE_API_KEY => self.cosine_api_key = Some(value.into()),
            crate::settings::FREQBLOG_BASE => self.freqblog_base = value.into(),
            crate::settings::FREQBLOG_API_KEY => self.freqblog_api_key = Some(value.into()),
            crate::settings::FREQBLOG_MONTHLY_CAP => {
                if let Ok(n) = value.trim().parse() {
                    self.freqblog_monthly_cap = n;
                }
            }
            crate::settings::EFFNET_MODEL => self.effnet_model = Some(value.into()),
            crate::settings::EFFNET_LABELS => self.effnet_labels = Some(value.into()),
            crate::settings::ANALYZER_BASE => self.analyzer_base = Some(value.into()),
            crate::settings::ANALYZE_TMP => self.analyze_tmp_dir = Some(value.into()),
            crate::settings::MUSIC_API_BASE => self.music_api_base = value.into(),
            crate::settings::MUSIC_API_TOKEN => self.music_api_token = Some(value.into()),
            crate::settings::SPOTIFY_CLIENT_ID => self.spotify_client_id = Some(value.into()),
            crate::settings::SPOTIFY_CLIENT_SECRET => {
                self.spotify_client_secret = Some(value.into())
            }
            crate::settings::SOUNDCLOUD_CLIENT_ID => self.soundcloud_client_id = Some(value.into()),
            crate::settings::SOUNDCLOUD_CLIENT_SECRET => {
                self.soundcloud_client_secret = Some(value.into())
            }
            crate::settings::YOUTUBE_CLIENT_ID => self.youtube_client_id = Some(value.into()),
            crate::settings::YOUTUBE_CLIENT_SECRET => {
                self.youtube_client_secret = Some(value.into())
            }
            _ => {}
        }
    }

    pub fn spotify_creds(&self) -> Result<(&str, &str)> {
        match (&self.spotify_client_id, &self.spotify_client_secret) {
            (Some(id), Some(secret)) => Ok((id.as_str(), secret.as_str())),
            _ => bail!(
                "SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET must be set (export them or put them in mmm-hub/.env)"
            ),
        }
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}
