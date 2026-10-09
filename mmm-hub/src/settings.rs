//! Runtime settings, editable by admins in the web backend and stored in
//! `hub_settings`. On boot they override the environment (`.env`) so keys can
//! be entered without touching the host.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::SqlitePool;

/// Known setting keys (all optional except the toggles).
pub const LASTFM_API_KEY: &str = "lastfm_api_key";
pub const RECCOBEATS_BASE: &str = "reccobeats_base";
pub const COSINE_API_KEY: &str = "cosine_api_key";
pub const COSINE_BASE: &str = "cosine_base";
pub const MUSIC_API_BASE: &str = "music_api_base";
pub const MUSIC_API_TOKEN: &str = "music_api_token";
pub const SPOTIFY_CLIENT_ID: &str = "spotify_client_id";
pub const SPOTIFY_CLIENT_SECRET: &str = "spotify_client_secret";
pub const SOUNDCLOUD_CLIENT_ID: &str = "soundcloud_client_id";
pub const SOUNDCLOUD_CLIENT_SECRET: &str = "soundcloud_client_secret";
pub const YOUTUBE_CLIENT_ID: &str = "youtube_client_id";
pub const YOUTUBE_CLIENT_SECRET: &str = "youtube_client_secret";
pub const REGISTRATION_OPEN: &str = "registration_open";

/// Keys surfaced on the admin page, with a human label and whether it's secret.
pub const ADMIN_FIELDS: &[(&str, &str, bool)] = &[
    (LASTFM_API_KEY, "Last.fm API-Key", true),
    (SOUNDCLOUD_CLIENT_ID, "SoundCloud Client-ID", false),
    (SOUNDCLOUD_CLIENT_SECRET, "SoundCloud Client-Secret", true),
    (YOUTUBE_CLIENT_ID, "YouTube/Google Client-ID", false),
    (YOUTUBE_CLIENT_SECRET, "YouTube/Google Client-Secret", true),
    (SPOTIFY_CLIENT_ID, "Spotify Client-ID", false),
    (SPOTIFY_CLIENT_SECRET, "Spotify Client-Secret", true),
    (MUSIC_API_BASE, "music-api Base-URL", false),
    (MUSIC_API_TOKEN, "music-api Token", true),
    (RECCOBEATS_BASE, "ReccoBeats Base-URL", false),
    (COSINE_API_KEY, "cosine.club API-Key", true),
    (COSINE_BASE, "cosine.club Base-URL", false),
];

pub async fn load_all(pool: &SqlitePool) -> HashMap<String, String> {
    sqlx::query_as::<_, (String, String)>("SELECT key, value FROM hub_settings")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub async fn get(pool: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT value FROM hub_settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Registration open? Defaults to closed when unset.
pub async fn registration_open(pool: &SqlitePool) -> bool {
    get(pool, REGISTRATION_OPEN).await.as_deref() == Some("1")
}

pub async fn is_admin(st: &crate::api::AppState, user_id: i64) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT is_admin FROM hub_users WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
        == 1
}

/// Overlay DB settings onto a config (env stays the fallback).
pub async fn overlay_config(pool: &SqlitePool, mut cfg: crate::config::Config) -> crate::config::Config {
    for (k, v) in load_all(pool).await {
        cfg.apply(&k, &v);
    }
    cfg
}
