//! Explorer API domain — seed-based track matching, presets, and bulk tagging.
//!
//! The explorer lets users select seed tracks and find similar tracks using
//! configurable matching criteria (BPM, harmonic keys, shared tags, etc.).
//! Saved presets store matching configurations for reuse.

use axum::{
    Json, Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, SimilarityMatch, Tag, Track, internal_error};
use anyhow::Result;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerSeed {
    pub id: i64,
    pub track: Track,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerPreset {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub bpm_tolerance: i64,
    pub bpm_ignore: bool,
    pub harmonic_relative: bool,
    pub harmonic_ignore: bool,
    pub match_setlist: String,
    pub match_artist: String,
    pub match_album: String,
    pub shared_mood_req: i64,
    pub shared_phase_req: i64,
    pub shared_vibe_req: i64,
    pub shared_merkmal_req: i64,
    pub shared_any_req: i64,
    pub config_json: String,
    pub harmonic_modes: Vec<String>,
    pub mandatory_tag_ids: Vec<i64>,
    pub is_default: bool,
    pub created_at: i64,
    pub last_used: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSeedRequest {
    pub track_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkTagRequest {
    pub track_ids: Vec<i64>,
    pub tag_names: Vec<String>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateExplorerPresetRequest {
    pub name: String,
    pub description: Option<String>,
    pub bpm_tolerance: Option<i64>,
    pub bpm_ignore: Option<bool>,
    pub harmonic_relative: Option<bool>,
    pub harmonic_ignore: Option<bool>,
    pub match_setlist: Option<String>,
    pub match_artist: Option<String>,
    pub match_album: Option<String>,
    pub shared_mood_req: Option<i64>,
    pub shared_phase_req: Option<i64>,
    pub shared_vibe_req: Option<i64>,
    pub shared_merkmal_req: Option<i64>,
    pub shared_any_req: Option<i64>,
    pub harmonic_modes: Option<Vec<String>>,
    pub mandatory_tag_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateExplorerPresetRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub bpm_tolerance: Option<i64>,
    pub bpm_ignore: Option<bool>,
    pub harmonic_relative: Option<bool>,
    pub harmonic_ignore: Option<bool>,
    pub match_setlist: Option<String>,
    pub match_artist: Option<String>,
    pub match_album: Option<String>,
    pub shared_mood_req: Option<i64>,
    pub shared_phase_req: Option<i64>,
    pub shared_vibe_req: Option<i64>,
    pub shared_merkmal_req: Option<i64>,
    pub shared_any_req: Option<i64>,
    pub harmonic_modes: Option<Vec<String>>,
    pub mandatory_tag_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerMatchesRequest {
    pub bpm_tolerance: Option<i64>,
    pub bpm_ignore: Option<bool>,
    pub harmonic_relative: Option<bool>,
    pub harmonic_ignore: Option<bool>,
    pub match_setlist: Option<String>,
    pub match_artist: Option<String>,
    pub match_album: Option<String>,
    pub shared_mood_req: Option<i64>,
    pub shared_phase_req: Option<i64>,
    pub shared_vibe_req: Option<i64>,
    pub shared_merkmal_req: Option<i64>,
    pub shared_any_req: Option<i64>,
    pub harmonic_modes: Option<Vec<String>>,
    pub mandatory_tag_ids: Option<Vec<i64>>,
}

// ── Helper Functions ─────────────────────────────────────────────────────

async fn get_explorer_seeds(pool: &Pool<Sqlite>) -> Result<Vec<ExplorerSeed>> {
    let rows = sqlx::query(
        "SELECT es.id, es.source_type, es.source_id, es.added_at,
                ut.*
         FROM explorer_seeds es
         LEFT JOIN unified_tracks ut ON
             (es.source_type = 'file' AND ut.source_type = 'file' AND ut.id = es.source_id) OR
             (es.source_type = 'service' AND ut.source_type = 'service' AND ut.id = es.source_id)",
    )
    .fetch_all(pool)
    .await?;

    let mut seeds = Vec::new();
    for row in rows {
        let tags_json: String = row.try_get("tags_json")?;
        let tags: Vec<Tag> = serde_json::from_str(&tags_json).unwrap_or_default();

        let track = Track {
            id: row.try_get("id")?,
            source_type: row.try_get("source_type")?,
            identifier: row.try_get("identifier")?,
            title: row.try_get("title")?,
            artist: row.try_get("artist")?,
            bpm: row.try_get("bpm").ok(),
            key: row.try_get("musical_key").ok(),
            tags,
            isrc: row.try_get("isrc").ok(),
            duration_ms: row.try_get("duration_ms").ok(),
            rating: row.try_get("rating").ok(),
        };

        seeds.push(ExplorerSeed {
            id: row.try_get("id")?,
            track,
            added_at: row.try_get("added_at")?,
        });
    }

    Ok(seeds)
}

async fn remove_explorer_seed(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM explorer_seeds WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

async fn find_similarity_matches(_pool: &Pool<Sqlite>) -> Result<Vec<SimilarityMatch>> {
    // Explorer feature disabled for now
    Ok(Vec::new())
}

async fn check_key_compatibility(
    pool: &Pool<Sqlite>,
    seed_key: &str,
    candidate_key: &str,
) -> Result<String> {
    let row = sqlx::query(
        "SELECT relationship FROM key_compatibility
         WHERE original_key = ? AND compatible_key = ?",
    )
    .bind(seed_key)
    .bind(candidate_key)
    .fetch_optional(pool)
    .await?;

    Ok(row
        .and_then(|r| r.try_get("relationship").ok())
        .unwrap_or_else(|| "none".to_string()))
}

async fn apply_bulk_tags(
    _pool: &Pool<Sqlite>,
    _track_ids: &[i64],
    _tag_names: &[String],
    _category: Option<&str>,
) -> Result<()> {
    // TODO: Implement bulk tagging — batch assign tags to multiple tracks at once
    Ok(())
}

// ── Handlers ─────────────────────────────────────────────────────────────

async fn explorer_seeds_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_explorer_seeds(&state.db).await {
        Ok(seeds) => Json(ApiResponse { data: seeds }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn add_seed_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<AddSeedRequest>,
) -> impl IntoResponse {
    // TODO: Implement add seed — needs source type detection (file vs service track)
    Json(ApiResponse {
        data: "add_seed_handler not implemented",
    })
}

async fn explorer_matches_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match find_similarity_matches(&state.db).await {
        Ok(matches) => Json(ApiResponse { data: matches }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn explorer_matches_with_config_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<ExplorerMatchesRequest>,
) -> impl IntoResponse {
    // TODO: Implement matches with config — apply user's filter/preset configuration to matching
    Json(ApiResponse {
        data: "explorer_matches_with_config not implemented",
    })
}

async fn explorer_presets_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // TODO: Implement get explorer presets — CRUD for saved match configurations
    Json(ApiResponse {
        data: "explorer_presets not implemented",
    })
}

async fn create_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<CreateExplorerPresetRequest>,
) -> impl IntoResponse {
    // TODO: Implement create explorer preset — save current match config as named preset
    Json(ApiResponse {
        data: "create_explorer_preset not implemented",
    })
}

async fn update_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(_request): Json<UpdateExplorerPresetRequest>,
) -> impl IntoResponse {
    // TODO: Implement update explorer preset — rename or change config of saved preset
    Json(ApiResponse {
        data: format!("update_explorer_preset not implemented for id {}", id),
    })
}

async fn delete_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement delete explorer preset — remove saved preset by id
    Json(ApiResponse {
        data: format!("delete_explorer_preset not implemented for id {}", id),
    })
}

async fn use_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement use explorer preset — apply preset config and return matches
    Json(ApiResponse {
        data: format!("use_explorer_preset not implemented for id {}", id),
    })
}

async fn remove_seed_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match remove_explorer_seed(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: "ok" }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn bulk_tag_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkTagRequest>,
) -> impl IntoResponse {
    match apply_bulk_tags(
        &state.db,
        &request.track_ids,
        &request.tag_names,
        request.category.as_deref(),
    )
    .await
    {
        Ok(_) => Json(ApiResponse { data: "ok" }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/explorer/seeds",
            get(explorer_seeds_handler).post(add_seed_handler),
        )
        .route("/api/explorer/seeds/remove", post(remove_seed_handler))
        .route("/api/explorer/matches", get(explorer_matches_handler))
        .route(
            "/api/explorer/matches/configure",
            post(explorer_matches_with_config_handler),
        )
        .route(
            "/api/explorer/presets",
            get(explorer_presets_handler).post(create_explorer_preset_handler),
        )
        .route(
            "/api/explorer/presets/{id}",
            put(update_explorer_preset_handler).delete(delete_explorer_preset_handler),
        )
        .route(
            "/api/explorer/presets/{id}/use",
            post(use_explorer_preset_handler),
        )
        .route("/api/tracks/bulk-tag", post(bulk_tag_handler))
}
