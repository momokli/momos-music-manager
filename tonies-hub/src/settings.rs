//! Key/value settings stored in the `settings` table (DB fallback overlay).

use anyhow::Result;
use sqlx::{Row, SqlitePool};

use crate::config::Config;

pub const MUSIC_API_BASE: &str = "music_api_base";
pub const MUSIC_API_TOKEN: &str = "music_api_token";
pub const TONIES_CLIENT_ID: &str = "tonies_client_id";

pub const ALL_KEYS: &[&str] = &[MUSIC_API_BASE, MUSIC_API_TOKEN, TONIES_CLIENT_ID];

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Read one setting. Empty/missing yields `None`.
pub async fn get(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.try_get::<String, _>("value").ok()))
}

/// Insert or update a setting.
pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(())
}

/// Every known setting, in insertion order of `ALL_KEYS`.
pub async fn all(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let key: String = r.try_get("key").ok()?;
            let value: String = r.try_get("value").ok()?;
            Some((key, value))
        })
        .collect())
}

/// Apply every DB setting to the config as a fallback (env keys are skipped by
/// `Config::apply`).
pub async fn load_overlay(pool: &SqlitePool, cfg: &mut Config) -> Result<()> {
    for (key, value) in all(pool).await? {
        cfg.apply(&key, &value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn overlay_roundtrip_and_env_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite:{}", dir.path().join("settings.db").display());
        let pool = db::connect(&url).await.unwrap();

        set(&pool, MUSIC_API_BASE, "http://db").await.unwrap();
        set(&pool, MUSIC_API_TOKEN, "secret-token").await.unwrap();
        set(&pool, TONIES_CLIENT_ID, "client-123").await.unwrap();

        assert_eq!(
            get(&pool, MUSIC_API_BASE).await.unwrap().as_deref(),
            Some("http://db")
        );
        assert_eq!(all(&pool).await.unwrap().len(), 3);

        let mut cfg = Config::for_test(&url);
        load_overlay(&pool, &mut cfg).await.unwrap();
        assert_eq!(cfg.music_api_base, "http://db");
        assert_eq!(cfg.music_api_token.as_deref(), Some("secret-token"));
        assert_eq!(cfg.tonies_client_id, "client-123");

        // Env key wins over the DB value.
        let mut cfg = Config::for_test(&url);
        cfg.music_api_base = "http://env".to_string();
        cfg.env_keys.insert(MUSIC_API_BASE.to_string());
        load_overlay(&pool, &mut cfg).await.unwrap();
        assert_eq!(cfg.music_api_base, "http://env");

        // Empty DB value changes nothing.
        set(&pool, TONIES_CLIENT_ID, "").await.unwrap();
        let mut cfg = Config::for_test(&url);
        load_overlay(&pool, &mut cfg).await.unwrap();
        assert_eq!(cfg.tonies_client_id, "");
    }
}
