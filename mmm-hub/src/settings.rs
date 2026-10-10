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
// Ripeness-scoring knobs (all editable in the web UI).
pub const ENGINE_TAG_WEIGHT: &str = "engine_tag_weight";
pub const ENGINE_META_WEIGHT: &str = "engine_meta_weight";
pub const ENGINE_TRAK_WEIGHT: &str = "engine_trak_weight";
pub const ENGINE_TAG_POINTS: &str = "engine_tag_points";
pub const ENGINE_TRAK_PLAYCOUNT_CAP: &str = "engine_trak_playcount_cap";
pub const ENGINE_TAG_OVERLAP_BASE: &str = "engine_tag_overlap_base";
pub const ENGINE_CROSS_GROUP_BONUS: &str = "engine_cross_group_bonus";
// Digging/overlap ranking (#223): how strongly similarity v2 and ripeness pull.
pub const ENGINE_SIM_FACTOR: &str = "engine_sim_factor";
pub const ENGINE_RIPENESS_FACTOR: &str = "engine_ripeness_factor";
// Per-meta-field ripeness weights (#217) — title/artist/album/cover/bpm/key/genre.
pub const ENGINE_META_TITLE: &str = "engine_meta_title";
pub const ENGINE_META_ARTIST: &str = "engine_meta_artist";
pub const ENGINE_META_ALBUM: &str = "engine_meta_album";
pub const ENGINE_META_COVER: &str = "engine_meta_cover";
pub const ENGINE_META_BPM: &str = "engine_meta_bpm";
pub const ENGINE_META_KEY: &str = "engine_meta_key";
pub const ENGINE_META_GENRE: &str = "engine_meta_genre";
// Co-occurrence ranking metric (#217): "lift" or "jaccard".
pub const ENGINE_COOC_METRIC: &str = "engine_cooc_metric";
// Tag-recommendation neighbourhood weights.
pub const ENGINE_REC_TAG: &str = "engine_rec_tag";
pub const ENGINE_REC_PLAYLIST: &str = "engine_rec_playlist";
pub const ENGINE_REC_ARTIST: &str = "engine_rec_artist";
pub const ENGINE_REC_ALBUM: &str = "engine_rec_album";

/// The kind of a setting value — drives the admin input widget and validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// A finite float, optionally bounded by `min`/`max`.
    Num,
    /// A boolean toggle, stored as `"1"`/`"0"`.
    Bool,
    /// A free-form string (URLs, keys, paths).
    Text,
    /// A comma-separated list of floats (e.g. the tag-point vector).
    Csv,
    /// One of a fixed set of string values.
    Enum(&'static [&'static str]),
}

/// One entry in the typed settings registry (#217): everything the engine and
/// the integrations read is declared here with a default and (for numbers) a
/// valid range, so the admin UI can render it and reject bad input.
#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub kind: SettingKind,
    pub default: &'static str,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

const fn num(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    min: f64,
    max: f64,
) -> SettingSpec {
    SettingSpec {
        key,
        label,
        secret: false,
        kind: SettingKind::Num,
        default,
        min: Some(min),
        max: Some(max),
    }
}

const fn flag(key: &'static str, label: &'static str, default: &'static str) -> SettingSpec {
    SettingSpec {
        key,
        label,
        secret: false,
        kind: SettingKind::Bool,
        default,
        min: None,
        max: None,
    }
}

const fn text(key: &'static str, label: &'static str, secret: bool) -> SettingSpec {
    SettingSpec {
        key,
        label,
        secret,
        kind: SettingKind::Text,
        default: "",
        min: None,
        max: None,
    }
}

const fn csv(key: &'static str, label: &'static str, default: &'static str) -> SettingSpec {
    SettingSpec {
        key,
        label,
        secret: false,
        kind: SettingKind::Csv,
        default,
        min: None,
        max: None,
    }
}

const fn one_of(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    allowed: &'static [&'static str],
) -> SettingSpec {
    SettingSpec {
        key,
        label,
        secret: false,
        kind: SettingKind::Enum(allowed),
        default,
        min: None,
        max: None,
    }
}

pub const COOC_METRICS: &[&str] = &["lift", "jaccard"];

/// The typed registry of every known setting (secrets + ranking engine).
pub const SETTINGS: &[SettingSpec] = &[
    text(LASTFM_API_KEY, "Last.fm API-Key", true),
    text(SOUNDCLOUD_CLIENT_ID, "SoundCloud Client-ID", false),
    text(SOUNDCLOUD_CLIENT_SECRET, "SoundCloud Client-Secret", true),
    text(YOUTUBE_CLIENT_ID, "YouTube/Google Client-ID", false),
    text(YOUTUBE_CLIENT_SECRET, "YouTube/Google Client-Secret", true),
    text(SPOTIFY_CLIENT_ID, "Spotify Client-ID", false),
    text(SPOTIFY_CLIENT_SECRET, "Spotify Client-Secret", true),
    text(MUSIC_API_BASE, "music-api Base-URL", false),
    text(MUSIC_API_TOKEN, "music-api Token", true),
    text(RECCOBEATS_BASE, "ReccoBeats Base-URL", false),
    text(COSINE_API_KEY, "cosine.club API-Key", true),
    text(COSINE_BASE, "cosine.club Base-URL", false),
    text(FREQBLOG_API_KEY, "FreqBlog API-Key", true),
    text(FREQBLOG_BASE, "FreqBlog Base-URL", false),
    num(
        FREQBLOG_MONTHLY_CAP,
        "FreqBlog Monats-Budget (Requests)",
        "1000",
        0.0,
        1_000_000.0,
    ),
    text(EFFNET_MODEL, "EffNet ONNX-Modellpfad", false),
    text(EFFNET_LABELS, "EffNet Genre-Labels (JSON)", false),
    text(
        ANALYZER_BASE,
        "BPM/Key-Analyzer URL (z.B. http://127.0.0.1:8711)",
        false,
    ),
    text(ANALYZE_TMP, "Verzeichnis fuer Analyse-Temp-Dateien", false),
    // ── Ranking engine ──────────────────────────────────────────────────────
    num(
        ENGINE_SHARED_FACTOR,
        "Engine: Faktor Seed-Match (shared)",
        "3",
        0.0,
        100.0,
    ),
    num(
        ENGINE_CANDIDATE_FACTOR,
        "Engine: Faktor Kandidaten-Tags",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_BASE_USERS,
        "Engine: Basis User-Uebereinstimmung",
        "10",
        0.0,
        10_000.0,
    ),
    num(
        ENGINE_BASE_PLAYLISTS,
        "Engine: Basis Playlist-Treffer",
        "3",
        0.0,
        10_000.0,
    ),
    num(ENGINE_BASE_LIKES, "Engine: Basis Likes", "2", 0.0, 10_000.0),
    num(
        ENGINE_BASE_SOURCES,
        "Engine: Basis Quellen",
        "1",
        0.0,
        10_000.0,
    ),
    flag(ENGINE_PARENT_MATCH, "Engine: Parent-Tag-Match", "0"),
    num(
        ENGINE_TAG_WEIGHT,
        "Scoring: Gewicht Human-Tags",
        "3",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_WEIGHT,
        "Scoring: Gewicht Track-Meta",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_TRAK_WEIGHT,
        "Scoring: Gewicht Traktor-Signal",
        "0.5",
        0.0,
        100.0,
    ),
    csv(
        ENGINE_TAG_POINTS,
        "Scoring: Tag-Punkte je Position (CSV)",
        "100,50,25,10,5,1",
    ),
    num(
        ENGINE_TRAK_PLAYCOUNT_CAP,
        "Scoring: Playcount-Cap fuer Traktor-Signal",
        "100",
        1.0,
        100_000.0,
    ),
    num(
        ENGINE_META_TITLE,
        "Scoring: Meta-Gewicht Titel",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_ARTIST,
        "Scoring: Meta-Gewicht Artist",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_ALBUM,
        "Scoring: Meta-Gewicht Album",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_COVER,
        "Scoring: Meta-Gewicht Cover",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_BPM,
        "Scoring: Meta-Gewicht BPM",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_KEY,
        "Scoring: Meta-Gewicht Key",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_META_GENRE,
        "Scoring: Meta-Gewicht Genre",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_TAG_OVERLAP_BASE,
        "Similarity: Basis je gemeinsamem Tag",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_CROSS_GROUP_BONUS,
        "Similarity: Bonus fuer Tag-Paare aus verschiedenen Gruppen",
        "1.5",
        0.0,
        100.0,
    ),
    num(
        ENGINE_SIM_FACTOR,
        "Engine: Faktor Similarity v2 (Digging/Overlap)",
        "5",
        0.0,
        100.0,
    ),
    num(
        ENGINE_RIPENESS_FACTOR,
        "Engine: Faktor Ripeness (Digging/Overlap)",
        "0.02",
        0.0,
        10.0,
    ),
    one_of(
        ENGINE_COOC_METRIC,
        "Insights: Co-Occurrence-Metrik",
        "lift",
        COOC_METRICS,
    ),
    num(
        ENGINE_REC_TAG,
        "Empfehlung: Gewicht geteilte Tags",
        "2",
        0.0,
        100.0,
    ),
    num(
        ENGINE_REC_PLAYLIST,
        "Empfehlung: Gewicht gleiche Playlists",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_REC_ARTIST,
        "Empfehlung: Gewicht gleicher Artist",
        "1",
        0.0,
        100.0,
    ),
    num(
        ENGINE_REC_ALBUM,
        "Empfehlung: Gewicht gleiches Album",
        "1",
        0.0,
        100.0,
    ),
];

/// Find the registry entry for a key, if any.
pub fn spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Validate + normalise a raw admin input for `spec`. Returns the value to store
/// (e.g. `"1"`/`"0"` for booleans) or a human-readable error.
pub fn validate(spec: &SettingSpec, raw: &str) -> Result<String, String> {
    let v = raw.trim();
    match spec.kind {
        SettingKind::Num => {
            let n: f64 = v
                .parse()
                .map_err(|_| format!("{}: keine Zahl", spec.label))?;
            if !n.is_finite() {
                return Err(format!("{}: ungueltige Zahl", spec.label));
            }
            if let Some(min) = spec.min {
                if n < min {
                    return Err(format!("{}: muss >= {min} sein", spec.label));
                }
            }
            if let Some(max) = spec.max {
                if n > max {
                    return Err(format!("{}: muss <= {max} sein", spec.label));
                }
            }
            Ok(v.to_string())
        }
        SettingKind::Bool => Ok(
            if matches!(v.to_lowercase().as_str(), "1" | "true" | "on" | "yes") {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        SettingKind::Csv => {
            if v.is_empty() {
                return Err(format!("{}: darf nicht leer sein", spec.label));
            }
            for part in v.split(',') {
                part.trim()
                    .parse::<f64>()
                    .map_err(|_| format!("{}: '{}' ist keine Zahl", spec.label, part.trim()))?;
            }
            Ok(v.to_string())
        }
        SettingKind::Enum(allowed) => {
            if allowed.contains(&v) {
                Ok(v.to_string())
            } else {
                Err(format!(
                    "{}: muss einer von {} sein",
                    spec.label,
                    allowed.join(", ")
                ))
            }
        }
        SettingKind::Text => Ok(v.to_string()),
    }
}

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
    /// Ripeness-scoring weights.
    pub tag_weight: f64,
    pub meta_weight: f64,
    pub trak_weight: f64,
    /// Tag points per position within a group (index 0 = first tag).
    pub tag_points: Vec<f64>,
    pub trak_playcount_cap: f64,
    /// Similarity v2 knobs.
    pub tag_overlap_base: f64,
    pub cross_group_bonus: f64,
    /// Digging/overlap ranking: weight of similarity v2 and ripeness signals.
    pub sim_factor: f64,
    pub ripeness_factor: f64,
    /// Per-meta-field ripeness weights (title/artist/album/cover/bpm/key/genre).
    pub meta_title: f64,
    pub meta_artist: f64,
    pub meta_album: f64,
    pub meta_cover: f64,
    pub meta_bpm: f64,
    pub meta_key: f64,
    pub meta_genre: f64,
    /// Co-occurrence ranking metric ("lift" | "jaccard").
    pub cooc_metric: String,
    /// Tag-recommendation neighbourhood weights.
    pub rec_tag: f64,
    pub rec_playlist: f64,
    pub rec_artist: f64,
    pub rec_album: f64,
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
            tag_weight: 3.0,
            meta_weight: 1.0,
            trak_weight: 0.5,
            tag_points: vec![100.0, 50.0, 25.0, 10.0, 5.0, 1.0],
            trak_playcount_cap: 100.0,
            tag_overlap_base: 1.0,
            cross_group_bonus: 1.5,
            sim_factor: 5.0,
            ripeness_factor: 0.02,
            meta_title: 1.0,
            meta_artist: 1.0,
            meta_album: 1.0,
            meta_cover: 1.0,
            meta_bpm: 1.0,
            meta_key: 1.0,
            meta_genre: 1.0,
            cooc_metric: "lift".to_string(),
            rec_tag: 2.0,
            rec_playlist: 1.0,
            rec_artist: 1.0,
            rec_album: 1.0,
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
    let points = s
        .get(ENGINE_TAG_POINTS)
        .map(|v| {
            v.split(',')
                .filter_map(|p| p.trim().parse::<f64>().ok())
                .collect::<Vec<f64>>()
        })
        .filter(|v: &Vec<f64>| !v.is_empty())
        .unwrap_or_else(|| Engine::default().tag_points);
    Engine {
        shared_factor: num(ENGINE_SHARED_FACTOR, 3.0),
        candidate_factor: num(ENGINE_CANDIDATE_FACTOR, 1.0),
        base_users: num(ENGINE_BASE_USERS, 10.0),
        base_playlists: num(ENGINE_BASE_PLAYLISTS, 3.0),
        base_likes: num(ENGINE_BASE_LIKES, 2.0),
        base_sources: num(ENGINE_BASE_SOURCES, 1.0),
        parent_match: flag(ENGINE_PARENT_MATCH, false),
        tag_weight: num(ENGINE_TAG_WEIGHT, 3.0),
        meta_weight: num(ENGINE_META_WEIGHT, 1.0),
        trak_weight: num(ENGINE_TRAK_WEIGHT, 0.5),
        tag_points: points,
        trak_playcount_cap: num(ENGINE_TRAK_PLAYCOUNT_CAP, 100.0),
        tag_overlap_base: num(ENGINE_TAG_OVERLAP_BASE, 1.0),
        cross_group_bonus: num(ENGINE_CROSS_GROUP_BONUS, 1.5),
        sim_factor: num(ENGINE_SIM_FACTOR, 5.0),
        ripeness_factor: num(ENGINE_RIPENESS_FACTOR, 0.02),
        meta_title: num(ENGINE_META_TITLE, 1.0),
        meta_artist: num(ENGINE_META_ARTIST, 1.0),
        meta_album: num(ENGINE_META_ALBUM, 1.0),
        meta_cover: num(ENGINE_META_COVER, 1.0),
        meta_bpm: num(ENGINE_META_BPM, 1.0),
        meta_key: num(ENGINE_META_KEY, 1.0),
        meta_genre: num(ENGINE_META_GENRE, 1.0),
        cooc_metric: s
            .get(ENGINE_COOC_METRIC)
            .map(|v| v.trim().to_lowercase())
            .filter(|v| v == "lift" || v == "jaccard")
            .unwrap_or_else(|| "lift".to_string()),
        rec_tag: num(ENGINE_REC_TAG, 2.0),
        rec_playlist: num(ENGINE_REC_PLAYLIST, 1.0),
        rec_artist: num(ENGINE_REC_ARTIST, 1.0),
        rec_album: num(ENGINE_REC_ALBUM, 1.0),
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
pub async fn overlay_config(
    pool: &SqlitePool,
    mut cfg: crate::config::Config,
) -> crate::config::Config {
    for (k, v) in load_all(pool).await {
        cfg.apply(&k, &v);
    }
    cfg
}
