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
pub const FREQBLOG_API_KEY: &str = "freqblog_api_key";
pub const FREQBLOG_BASE: &str = "freqblog_base";
pub const FREQBLOG_MONTHLY_CAP: &str = "freqblog_monthly_cap";
pub const EFFNET_MODEL: &str = "effnet_model";
pub const EFFNET_LABELS: &str = "effnet_labels";
pub const ANALYZER_BASE: &str = "analyzer_base";
pub const ANALYZE_TMP: &str = "analyze_tmp_dir";
pub const MUSIC_API_BASE: &str = "music_api_base";
pub const MUSIC_API_TOKEN: &str = "music_api_token";
pub const SPOTIFY_CLIENT_ID: &str = "spotify_client_id";
pub const SPOTIFY_CLIENT_SECRET: &str = "spotify_client_secret";
pub const SOUNDCLOUD_CLIENT_ID: &str = "soundcloud_client_id";
pub const SOUNDCLOUD_CLIENT_SECRET: &str = "soundcloud_client_secret";
pub const YOUTUBE_CLIENT_ID: &str = "youtube_client_id";
pub const YOUTUBE_CLIENT_SECRET: &str = "youtube_client_secret";
pub const REGISTRATION_OPEN: &str = "registration_open";

// Ranking-engine knobs (editable in the web UI, no secrets).
pub const ENGINE_SHARED_FACTOR: &str = "engine_shared_factor";
pub const ENGINE_CANDIDATE_FACTOR: &str = "engine_candidate_factor";
pub const ENGINE_BASE_USERS: &str = "engine_base_users";
pub const ENGINE_BASE_PLAYLISTS: &str = "engine_base_playlists";
pub const ENGINE_BASE_LIKES: &str = "engine_base_likes";
pub const ENGINE_BASE_SOURCES: &str = "engine_base_sources";
pub const ENGINE_PARENT_MATCH: &str = "engine_parent_match";

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
    (FREQBLOG_API_KEY, "FreqBlog API-Key", true),
    (FREQBLOG_BASE, "FreqBlog Base-URL", false),
    (FREQBLOG_MONTHLY_CAP, "FreqBlog Monats-Budget (Requests)", false),
    (EFFNET_MODEL, "EffNet ONNX-Modellpfad", false),
    (EFFNET_LABELS, "EffNet Genre-Labels (JSON)", false),
    (ANALYZER_BASE, "BPM/Key-Analyzer URL (z.B. http://127.0.0.1:8711)", false),
    (ANALYZE_TMP, "Verzeichnis fuer Analyse-Temp-Dateien", false),
    (ENGINE_SHARED_FACTOR, "Engine: Faktor Seed-Match (shared)", false),
    (ENGINE_CANDIDATE_FACTOR, "Engine: Faktor Kandidaten-Tags", false),
    (ENGINE_BASE_USERS, "Engine: Basis User-Uebereinstimmung", false),
    (ENGINE_BASE_PLAYLISTS, "Engine: Basis Playlist-Treffer", false),
    (ENGINE_BASE_LIKES, "Engine: Basis Likes", false),
    (ENGINE_BASE_SOURCES, "Engine: Basis Quellen", false),
    (ENGINE_PARENT_MATCH, "Engine: Parent-Tag-Match (1/0)", false),
];

/// Ranking-engine configuration, resolved from settings with sane defaults.
#[derive(Debug, Clone)]
pub struct Engine {
    pub shared_factor: f64,
    pub candidate_factor: f64,
    pub base_users: f64,
    pub base_playlists: f64,
    pub base_likes: f64,
    pub base_sources: f64,
    pub parent_match: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            shared_factor: 3.0,
            candidate_factor: 1.0,
            base_users: 10.0,
            base_playlists: 3.0,
            base_likes: 2.0,
            base_sources: 1.0,
            parent_match: false,
        }
    }
}

/// Load the ranking-engine config from `hub_settings` (falls back to defaults).
pub async fn engine(pool: &SqlitePool) -> Engine {
    let s = load_all(pool).await;
    let num = |k: &str, d: f64| {
        s.get(k)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(d)
    };
    let flag = |k: &str, d: bool| match s.get(k).map(|v| v.trim().to_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
        None => d,
    };
    Engine {
        shared_factor: num(ENGINE_SHARED_FACTOR, 3.0),
        candidate_factor: num(ENGINE_CANDIDATE_FACTOR, 1.0),
        base_users: num(ENGINE_BASE_USERS, 10.0),
        base_playlists: num(ENGINE_BASE_PLAYLISTS, 3.0),
        base_likes: num(ENGINE_BASE_LIKES, 2.0),
        base_sources: num(ENGINE_BASE_SOURCES, 1.0),
        parent_match: flag(ENGINE_PARENT_MATCH, false),
    }
}

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
