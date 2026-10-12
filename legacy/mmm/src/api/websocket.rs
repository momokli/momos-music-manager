//! WebSocket / OAuth Callback API domain.
//! Handles the Spotify OAuth callback endpoint and future WebSocket connections.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::get,
};
use rspotify::clients::OAuthClient;
use rspotify::{AuthCodeSpotify, Config, Credentials, OAuth, scopes};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, CallbackParams};
use crate::db::{get_service_config, update_service_connection_status, update_service_tokens};

// ── Handlers ────────────────────────────────────────────────────────────────

async fn ws_handler() -> impl IntoResponse {
    // TODO: Implement WebSocket handler — for real-time task progress updates to frontend
    "WebSocket endpoint".into_response()
}

async fn handle_websocket() {
    // TODO: Implement WebSocket handling — upgrade connection, manage client set, broadcast task updates
}

async fn legacy_callback_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    let service = "spotify".to_string();

    // Check for OAuth errors
    if let Some(error) = params.error {
        tracing::error!("OAuth error: {}", error);
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("OAuth error: {}", error),
            }),
        )
            .into_response();
    }

    // Get authorization code
    let code = match params.code {
        Some(code) => code,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: "Missing authorization code".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Get service config from database
    let _config = match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            // Create default config for this service if it doesn't exist
            if let Err(e) = crate::db::update_service_config(&state.db, &service, None, None).await
            {
                tracing::error!("Failed to create service config: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse {
                        data: format!("Failed to create service config: {}", e),
                    }),
                )
                    .into_response();
            }
            // Try to get config again
            match get_service_config(&state.db, &service).await {
                Ok(Some(config)) => config,
                Ok(None) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to retrieve created config for {}", service),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!("Failed to get service config after creation: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to get service config: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get service config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get service config: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if Spotify is configured in .env file
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        )
            .into_response();
    }

    // Get Spotify credentials from .env
    let client_id = match state.config.spotify_client_id() {
        Ok(id) => id.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client ID: {}", e),
                }),
            )
                .into_response();
        }
    };
    let client_secret = match state.config.spotify_client_secret() {
        Ok(secret) => secret.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client secret: {}", e),
                }),
            )
                .into_response();
        }
    };

    tracing::debug!("Spotify Legacy Callback - Client ID: {}", client_id);
    tracing::debug!(
        "Spotify Legacy Callback - Redirect URI: {}",
        state.config.spotify_redirect_uri
    );

    // Create OAuth credentials and exchange code for tokens
    let creds = Credentials::new(&client_id, &client_secret);
    let oauth = OAuth {
        redirect_uri: state.config.spotify_redirect_uri.clone(),
        scopes: scopes!(
            "playlist-read-private",
            "playlist-read-collaborative",
            "playlist-modify-public",
            "playlist-modify-private",
            "user-read-playback-state"
        ),
        ..Default::default()
    };

    let spotify_config = Config::default();
    let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

    match spotify.request_token(&code).await {
        Ok(_) => {
            // Get tokens from spotify client
            let token_lock = spotify.token.lock().await;
            if let Ok(guard) = token_lock
                && let Some(token) = &*guard
            {
                // Store tokens in database
                let refresh_token = token.refresh_token.clone();
                let access_token = token.access_token.clone();
                let token_expiry = token.expires_at.map(|dt| dt.timestamp());

                if let Err(e) = update_service_tokens(
                    &state.db,
                    &service,
                    refresh_token.as_deref(),
                    Some(&access_token),
                    token_expiry,
                )
                .await
                {
                    tracing::error!("Failed to store tokens: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to store tokens: {}", e),
                        }),
                    )
                        .into_response();
                }

                // Update connection status
                if let Err(e) = update_service_connection_status(&state.db, &service, true).await {
                    tracing::warn!("Failed to update connection status: {}", e);
                }

                let redirect_url = state.public_url.clone().unwrap_or_else(|| {
                    format!(
                        "http://{}:{}",
                        state.config.server_host, state.config.server_port
                    )
                });
                return Redirect::to(&redirect_url).into_response();
            }

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: "Failed to retrieve tokens from Spotify client".to_string(),
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to exchange code for tokens: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to exchange code for tokens: {}", e),
                }),
            )
                .into_response()
        }
    }
}

// ── Router ──────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/callback", get(legacy_callback_handler))
        .route("/ws/spotify", get(ws_handler))
}
