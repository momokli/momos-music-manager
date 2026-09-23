//! Backpack transport endpoints.
//!
//! The Backpack *set* is defined in [`crate::backpack`]; this module exposes the
//! materialised Spotify playlist (status + explicit push).

use axum::{
    Json, Router,
    extract::State,
    response::IntoResponse,
    routing::{get, post},
};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::backpack::{
    BackpackSpotifyOps, MaterializeOptions, backpack_signature, backpack_status,
    has_pending_push, materialize_backpack_playlist_with, record_push_status,
    resolve_backpack_track_uris,
};
use crate::spotify::client::SpotifyClient;

/// GET /api/backpack — Backpack set size + materialised playlist status.
async fn backpack_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match backpack_status(&state.db).await {
        Ok(status) => {
            let push_pending = has_pending_push(&state.backpack_coordinator);

            // `dirty` only means "a membership mutation is pending", which is not
            // the same as "the playlist is wrong". `inSync` compares the stored
            // signature with the current set, so the UI can show the truth
            // without changing what `dirty` means to the coordinator.
            let in_sync = match resolve_backpack_track_uris(&state.db).await {
                Ok(uris) => {
                    let current = backpack_signature(&uris);
                    status.signature.as_deref() == Some(current.as_str())
                }
                Err(_) => false,
            };

            Json(ApiResponse {
                data: serde_json::json!({
                    "trackCount": status.track_count,
                    "fileCount": status.file_count,
                    "playlistUrl": status.playlist_url,
                    "signature": status.signature,
                    "inSync": in_sync,
                    "dirty": status.dirty,
                    "dirtyAt": status.dirty_at,
                    "lastPushAt": status.last_push_at,
                    "lastPushStatus": status.last_push_status,
                    "lastPushError": status.last_push_error,
                    "pushPending": push_pending,
                }),
            })
            .into_response()
        }
        Err(e) => internal_error(format!("Failed to read Backpack status: {e:#}")).into_response(),
    }
}

/// Body of `POST /api/backpack/push`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    /// Ignore the stored signature and rebuild unconditionally.
    #[serde(default)]
    pub force: bool,
    /// Also submit the single Backpack URL to deemix. Default `true`.
    #[serde(default)]
    pub submit_to_deemix: Option<bool>,
    /// Build nothing — report what would happen. Default `false`.
    #[serde(default)]
    pub dry_run: bool,
}

/// A Spotify stub used for dry runs: any call would be a bug, so it errors.
struct DryRunSpotify;

#[async_trait]
impl BackpackSpotifyOps for DryRunSpotify {
    async fn current_user_id(&self) -> anyhow::Result<String> {
        anyhow::bail!("dry run must not call Spotify")
    }
    async fn create_playlist(
        &self,
        _u: &str,
        _n: &str,
        _p: bool,
        _d: Option<&str>,
    ) -> anyhow::Result<(String, String)> {
        anyhow::bail!("dry run must not call Spotify")
    }
    async fn add_tracks_to_playlist(&self, _p: &str, _u: &[String]) -> anyhow::Result<()> {
        anyhow::bail!("dry run must not call Spotify")
    }
    async fn replace_tracks(&self, _p: &str, _u: &[String]) -> anyhow::Result<()> {
        anyhow::bail!("dry run must not call Spotify")
    }
    async fn playlist_uris(&self, _p: &str) -> anyhow::Result<Vec<String>> {
        anyhow::bail!("dry run must not call Spotify")
    }
}

/// POST /api/backpack/push — explicitly (re)build the Backpack Spotify playlist.
async fn backpack_push_handler(
    State(state): State<Arc<AppState>>,
    body: Option<Json<PushRequest>>,
) -> impl IntoResponse {
    let req = body.map(|Json(b)| b).unwrap_or_default();

    // A dry run must never need Spotify credentials, so it uses a stub transport.
    if req.dry_run {
        let opts = MaterializeOptions {
            force: req.force,
            submit_to_deemix: false,
            dry_run: true,
        };
        let result = materialize_backpack_playlist_with::<DryRunSpotify, crate::deemix::DeemixClient>(
            &state.db,
            &DryRunSpotify,
            None,
            opts,
        )
        .await;
        return respond(result);
    }

    let client = match SpotifyClient::from_stored_tokens(state.db.clone(), &state.config).await {
        Ok(c) => c,
        Err(e) => {
            return internal_error(format!("Spotify not configured: {e:#}")).into_response();
        }
    };
    let deemix = crate::deemix::DeemixClient::from_db(state.db.clone()).await;

    let opts = MaterializeOptions {
        force: req.force,
        submit_to_deemix: req.submit_to_deemix.unwrap_or(true),
        dry_run: false,
    };

    let result =
        materialize_backpack_playlist_with(&state.db, &client, deemix.as_ref(), opts).await;
    match &result {
        Ok(_) => record_push_status(&state.db, "ok", None).await,
        Err(e) => record_push_status(&state.db, "error", Some(&format!("{e:#}"))).await,
    }
    respond(result)
}

/// Render a materialisation result (or error) as the API response.
fn respond(result: anyhow::Result<crate::backpack::MaterializeOutcome>) -> axum::response::Response {
    match result {
        Ok(outcome) => Json(ApiResponse {
            data: serde_json::json!({
                "trackCount": outcome.track_count,
                "created": outcome.created,
                "updated": outcome.updated,
                "spotifyUrl": outcome.spotify_url,
                "deemixSubmitted": outcome.deemix_submitted,
                "dryRun": outcome.dry_run,
                "verificationFailed": outcome.verification_failed,
            }),
        })
        .into_response(),
        Err(e) => internal_error(format!("Backpack push failed: {e:#}")).into_response(),
    }
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/backpack", get(backpack_status_handler))
        .route("/api/backpack/push", post(backpack_push_handler))
}
