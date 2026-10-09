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
