use std::collections::HashSet;

use anyhow::Result;

/// Runtime configuration for the Tonies-Hub.
///
/// Everything comes from `TONIES_*` / `MUSIC_API_*` env vars (with `.env`
/// support via `dotenvy`). Values stored in the `settings` table are applied as
/// a fallback overlay: env always wins. `env_keys` records which setting keys
/// were provided by the environment at boot.
#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub music_api_base: String,
    pub music_api_token: Option<String>,
    pub tonies_client_id: String,
    /// Send the session cookie with the `Secure` flag (production HTTPS).
    pub cookie_secure: bool,
    /// Setting keys that were set in the environment at startup.
    pub env_keys: HashSet<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let mut env_keys = HashSet::new();

        let host = env("TONIES_HOST").unwrap_or_else(|| "0.0.0.0".to_string());
        let port = env("TONIES_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(8090);
        let database_url = env("TONIES_DATABASE_URL")
            .unwrap_or_else(|| "sqlite:tonies.db".to_string());

        let music_api_base = match env("MUSIC_API_BASE") {
            Some(v) => {
                env_keys.insert(crate::settings::MUSIC_API_BASE.to_string());
                v
            }
            None => DEFAULT_MUSIC_API_BASE.to_string(),
        };
        let music_api_token = match env("MUSIC_API_TOKEN") {
            Some(v) => {
                env_keys.insert(crate::settings::MUSIC_API_TOKEN.to_string());
                Some(v)
            }
            None => None,
        };
        let tonies_client_id = match env("TONIES_CLIENT_ID") {
            Some(v) => {
                env_keys.insert(crate::settings::TONIES_CLIENT_ID.to_string());
                v
            }
            None => String::new(),
        };

        let cookie_secure = env("TONIES_COOKIE_SECURE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        Self {
            host,
            port,
            database_url,
            music_api_base,
            music_api_token,
            tonies_client_id,
            cookie_secure,
            env_keys,
        }
    }

    /// A self-contained config for integration tests: only the DB URL matters.
    pub fn for_test(database_url: impl Into<String>) -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
            database_url: database_url.into(),
            music_api_base: DEFAULT_MUSIC_API_BASE.to_string(),
            music_api_token: None,
            tonies_client_id: String::new(),
            cookie_secure: false,
            env_keys: HashSet::new(),
        }
    }

    /// Apply a value from the `settings` table.
    ///
    /// Empty values and unknown keys are ignored. Keys that came from the
    /// environment at startup are never overridden (env wins), so the DB is
    /// only a fallback overlay.
    pub fn apply(&mut self, key: &str, value: &str) {
        if value.trim().is_empty() || self.env_keys.contains(key) {
            return;
        }
        match key {
            crate::settings::MUSIC_API_BASE => self.music_api_base = value.to_string(),
            crate::settings::MUSIC_API_TOKEN => self.music_api_token = Some(value.to_string()),
            crate::settings::TONIES_CLIENT_ID => self.tonies_client_id = value.to_string(),
            _ => {}
        }
    }

    /// `true` when a base URL for the music API is configured.
    pub fn music_api_configured(&self) -> Result<()> {
        if self.music_api_base.trim().is_empty() {
            anyhow::bail!("music_api_base ist nicht konfiguriert");
        }
        Ok(())
    }
}

pub const DEFAULT_MUSIC_API_BASE: &str = "http://127.0.0.1:8710";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_ignores_empty_and_unknown() {
        let mut cfg = Config::for_test("sqlite::memory:");
        let before = cfg.music_api_base.clone();

        cfg.apply(crate::settings::MUSIC_API_BASE, "");
        assert_eq!(cfg.music_api_base, before);

        cfg.apply("does_not_exist", "x");
        assert_eq!(cfg.music_api_base, before);
        assert!(cfg.music_api_token.is_none());
    }

    #[test]
    fn apply_uses_db_value_without_env() {
        let mut cfg = Config::for_test("sqlite::memory:");
        cfg.apply(crate::settings::MUSIC_API_BASE, "http://db");
        assert_eq!(cfg.music_api_base, "http://db");

        cfg.apply(crate::settings::MUSIC_API_TOKEN, "tok");
        assert_eq!(cfg.music_api_token.as_deref(), Some("tok"));
    }

    #[test]
    fn env_key_wins_over_db() {
        let mut cfg = Config::for_test("sqlite::memory:");
        cfg.music_api_base = "http://env".to_string();
        cfg.env_keys
            .insert(crate::settings::MUSIC_API_BASE.to_string());

        cfg.apply(crate::settings::MUSIC_API_BASE, "http://db");
        assert_eq!(cfg.music_api_base, "http://env");
    }
}
