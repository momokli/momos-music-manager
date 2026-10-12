//! Extended-Mix auto-upgrade: library scan that reports candidate tracks whose
//! "Extended Mix" version is available at the source but not yet owned.
//! (Issue #29)
//!
//! The scan is read-only: it groups every `service_tracks` row into a release
//! via [`crate::extended_mix::base_title`] + artist, then reports releases
//! where a shorter version is owned but an Extended Mix is available and not
//! owned. The pure decision logic lives in [`crate::extended_mix`].

use axum::{Json, Router, extract::State, response::IntoResponse, routing::get};
use serde::Serialize;
use sqlx::{Pool, Sqlite};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::db::ServiceTrack;
use crate::extended_mix::{TrackVersion, find_upgrade_candidates};

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtendedMixCandidate {
    release_key: String,
    owned_title: String,
    artist: String,
    extended_track_id: i64,
    extended_title: String,
    extended_service: String,
    extended_service_id: String,
    extended_duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtendedMixScanResponse {
    /// Whether auto-upgrade is enabled (config toggle; see issue #29).
    enabled: bool,
    candidates: Vec<ExtendedMixCandidate>,
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/extended-mix/candidates", get(candidates_handler))
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn candidates_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match scan_candidates(&state.db).await {
        Ok(candidates) => {
            tracing::info!(
                "Extended-Mix scan: {} upgrade candidate(s), auto-upgrade enabled={}",
                candidates.len(),
                state.config.autoupgrade_enabled,
            );
            Json(ApiResponse {
                data: ExtendedMixScanResponse {
                    enabled: state.config.autoupgrade_enabled,
                    candidates,
                },
            })
            .into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Scan logic ────────────────────────────────────────────────────────────

/// Scan `service_tracks` and report releases where a shorter version is owned
/// but an Extended Mix is available and not owned.
pub async fn scan_candidates(
    pool: &Pool<Sqlite>,
) -> Result<Vec<ExtendedMixCandidate>, anyhow::Error> {
    let tracks: Vec<ServiceTrack> = sqlx::query_as::<_, ServiceTrack>("SELECT * FROM service_tracks")
        .fetch_all(pool)
        .await?;

    // Owned track ids = distinct track_id in v_file_track_link (file ↔ track).
    let owned: HashSet<i64> = sqlx::query_scalar::<_, i64>("SELECT DISTINCT track_id FROM v_file_track_link")
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();

    let meta: HashMap<i64, &ServiceTrack> = tracks.iter().map(|t| (t.id, t)).collect();

    let versions: Vec<TrackVersion> = tracks
        .iter()
        .map(|t| TrackVersion {
            track_id: t.id,
            title: t.title.clone(),
            artist: t.artist.clone(),
            owned: owned.contains(&t.id),
        })
        .collect();

    let candidates = find_upgrade_candidates(&versions)
        .into_iter()
        .filter_map(|c| {
            let ext = meta.get(&c.extended_track_id)?;
            Some(ExtendedMixCandidate {
                release_key: c.release_key,
                owned_title: c.owned_title,
                artist: c.artist,
                extended_track_id: c.extended_track_id,
                extended_title: c.extended_title,
                extended_service: ext.service.clone(),
                extended_service_id: ext.service_id.clone(),
                extended_duration_ms: ext.duration_ms,
            })
        })
        .collect();

    Ok(candidates)
}
