//! API surface for the BPM//key system-playlist feature.
//!
//! - `GET  /api/bpm-key-playlists/preview`  — derived `(BPM, key)` buckets
//! - `POST /api/bpm-key-playlists/sync`     — start a reconcile task
//! - `GET  /api/bpm-key-playlists`          — the persisted system playlists
//! - `GET  /api/bpm-key-playlists/settings` — feature settings
//! - `PUT  /api/bpm-key-playlists/settings` — partial settings update

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::bpm_key::BpmKeySettings;
use crate::db::bpm_key as bpm_db;
use crate::tasks::{TaskStatus, start_sync_bpm_key_playlists_task};

/// Optional body for the sync endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncRequest {
    strict: Option<bool>,
}

/// Partial settings update body — any omitted key keeps its current value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsUpdate {
    enabled: Option<bool>,
    name_template: Option<String>,
    name_prefix: Option<String>,
    min_tracks: Option<i64>,
    public: Option<bool>,
    key_style: Option<String>,
    strict: Option<bool>,
    schedule_enabled: Option<bool>,
    schedule_interval_secs: Option<i64>,
}

impl SettingsUpdate {
    fn apply(self, s: &mut BpmKeySettings) {
        if let Some(v) = self.enabled {
            s.enabled = v;
        }
        if let Some(v) = self.name_template {
            s.name_template = v;
        }
        if let Some(v) = self.name_prefix {
            s.name_prefix = v;
        }
        if let Some(v) = self.min_tracks {
            s.min_tracks = v;
        }
        if let Some(v) = self.public {
            s.public = v;
        }
        if let Some(v) = self.key_style {
            s.key_style = v;
        }
        if let Some(v) = self.strict {
            s.strict = v;
        }
        if let Some(v) = self.schedule_enabled {
            s.schedule_enabled = v;
        }
        if let Some(v) = self.schedule_interval_secs {
            s.schedule_interval_secs = v;
        }
    }
}

fn spotify_playlist_url(playlist_id: &str) -> String {
    format!("https://open.spotify.com/playlist/{playlist_id}")
}

/// Derive the buckets that meet `min_tracks` — the same set the sync considers.
async fn desired_group_count(
    state: &Arc<AppState>,
    settings: &BpmKeySettings,
) -> anyhow::Result<usize> {
    let groups = bpm_db::derive_groups(&state.db).await?;
    Ok(groups
        .iter()
        .filter(|g| g.track_count >= settings.min_tracks)
        .count())
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn preview_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let settings = match bpm_db::load_bpm_key_settings(&state.db).await {
        Ok(s) => s,
        Err(e) => return internal_error(format!("Failed to load settings: {e}")).into_response(),
    };
    let groups = match bpm_db::derive_groups(&state.db).await {
        Ok(g) => g,
        Err(e) => return internal_error(format!("Failed to derive groups: {e}")).into_response(),
    };
    let existing = match bpm_db::list_system_playlists(&state.db).await {
        Ok(p) => p,
        Err(e) => {
            return internal_error(format!("Failed to load system playlists: {e}")).into_response();
        }
    };
    let existing_map: HashMap<String, String> = existing
        .into_iter()
        .map(|p| (p.system_key, p.playlist_id))
        .collect();

    let mut out = Vec::with_capacity(groups.len());
    for g in &groups {
        let system_key = g.system_key();
        let name = settings.name_for(g.bpm, &g.canonical_key, Some(g.track_count as usize));
        let (exists, spotify_id, spotify_url) = match existing_map.get(&system_key) {
            Some(pid) => (true, Some(pid.clone()), Some(spotify_playlist_url(pid))),
            None => (false, None, None),
        };
        out.push(json!({
            "bpm": g.bpm,
            "key": settings.display_key(&g.canonical_key),
            "name": name,
            "systemKey": system_key,
            "fileCount": g.file_count,
            "trackCount": g.track_count,
            "exists": exists,
            "spotifyPlaylistId": spotify_id,
            "spotifyUrl": spotify_url,
        }));
    }

    Json(ApiResponse {
        data: json!({ "groups": out, "total": out.len() }),
    })
    .into_response()
}

async fn sync_handler(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SyncRequest>>,
) -> impl IntoResponse {
    let settings = match bpm_db::load_bpm_key_settings(&state.db).await {
        Ok(s) => s,
        Err(e) => return internal_error(format!("Failed to load settings: {e}")).into_response(),
    };
    let strict = body.and_then(|Json(b)| b.strict).unwrap_or(settings.strict);

    if !state.config.is_spotify_configured() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({"error": "Spotify not configured — set credentials on the Services page."}),
            ),
        )
            .into_response();
    }

    let group_count = match desired_group_count(&state, &settings).await {
        Ok(n) => n,
        Err(e) => return internal_error(format!("Failed to derive groups: {e}")).into_response(),
    };

    let task_id =
        start_sync_bpm_key_playlists_task(&state.task_manager, &state.db, &state.config, strict)
            .await;

    if task_id.is_empty() {
        // A sync is already pending/running — hand back its id (burst coalescing).
        let existing = state
            .task_manager
            .list_tasks()
            .await
            .into_iter()
            .find(|t| {
                t.task_type == "sync_bpm_key_playlists"
                    && matches!(t.status, TaskStatus::Pending | TaskStatus::Running)
            })
            .map(|t| t.id);
        return Json(ApiResponse {
            data: json!({
                "taskId": existing,
                "groupCount": group_count,
                "alreadyRunning": true,
            }),
        })
        .into_response();
    }

    Json(ApiResponse {
        data: json!({ "taskId": task_id, "groupCount": group_count }),
    })
    .into_response()
}

async fn list_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rows = match bpm_db::list_system_playlists(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            return internal_error(format!("Failed to load system playlists: {e}")).into_response();
        }
    };
    // Current desired track counts (derived from the library) — the Spotify
    // playlist itself is the only persisted membership.
    let counts: HashMap<String, i64> = match bpm_db::derive_groups(&state.db).await {
        Ok(groups) => groups
            .into_iter()
            .map(|g| (g.system_key(), g.track_count))
            .collect(),
        Err(_) => HashMap::new(),
    };

    let playlists: Vec<serde_json::Value> = rows
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "systemKey": p.system_key,
                "spotifyPlaylistId": p.playlist_id,
                "spotifyUrl": spotify_playlist_url(&p.playlist_id),
                "trackCount": counts.get(&p.system_key).copied().unwrap_or(0),
            })
        })
        .collect();

    Json(ApiResponse {
        data: json!({ "playlists": playlists, "total": playlists.len() }),
    })
    .into_response()
}

async fn get_settings_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match bpm_db::load_bpm_key_settings(&state.db).await {
        Ok(s) => Json(ApiResponse {
            data: json!({ "settings": s }),
        })
        .into_response(),
        Err(e) => internal_error(format!("Failed to load settings: {e}")).into_response(),
    }
}

async fn put_settings_handler(
    State(state): State<Arc<AppState>>,
    Json(update): Json<SettingsUpdate>,
) -> impl IntoResponse {
    let mut settings = match bpm_db::load_bpm_key_settings(&state.db).await {
        Ok(s) => s,
        Err(e) => return internal_error(format!("Failed to load settings: {e}")).into_response(),
    };
    update.apply(&mut settings);
    settings.sanitize();
    if let Err(e) = bpm_db::save_bpm_key_settings(&state.db, &settings).await {
        return internal_error(format!("Failed to save settings: {e}")).into_response();
    }
    Json(ApiResponse {
        data: json!({ "settings": settings }),
    })
    .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/bpm-key-playlists/preview", get(preview_handler))
        .route("/api/bpm-key-playlists/sync", post(sync_handler))
        .route(
            "/api/bpm-key-playlists/settings",
            get(get_settings_handler).put(put_settings_handler),
        )
        .route("/api/bpm-key-playlists", get(list_handler))
}
