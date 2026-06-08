use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect},
    routing::{get, post, put},
};
use chrono::{DateTime, Duration};
use rspotify::clients::{BaseClient, OAuthClient};
use rspotify::model::Market;
use rspotify::{AuthCodeSpotify, Config, Credentials, OAuth, Token, scopes};
use serde::{Deserialize, Serialize};
use sqlx::Pool;
use std::sync::Arc;
use tokio_stream::StreamExt;

use crate::AppState;
use crate::api::types::{ApiResponse, CallbackParams, internal_error};
use crate::config::ServiceCredentials;
use crate::db::{
    ServiceConfig, get_service_config, update_service_config, update_service_connection_status,
};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConnection {
    pub service: String,
    pub configured: bool,
    pub connected: bool,
    pub is_syncing: bool,

    pub last_sync: Option<i64>,
    pub playlists_local: i64,
    pub tracks_local: i64,
    pub playlists_remote: i64,
    pub tracks_remote: i64,
    pub sync_current_playlist: Option<i64>,
    pub sync_current_track: Option<i64>,
    pub sync_total_playlists: Option<i64>,
    pub sync_total_tracks: Option<i64>,
    pub sync_log: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServiceConfigRequest {
    pub user_id: Option<String>,
    pub playlist_id: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn services_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_service_connections(&state.db, &state.config).await {
        Ok(services) => Json(ApiResponse { data: services }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn service_auth_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Deemix uses its own auth endpoint (/api/services/deemix/auth), not OAuth
    if service == "deemix" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Deemix auth is handled via /api/services/deemix/auth".to_string(),
            }),
        )
            .into_response();
    }

    // Check if service is configured in .env file
    match service.as_str() {
        "spotify" => {
            if !state.config.is_spotify_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
                    }),
                )
                    .into_response();
            }
        }
        "soundcloud" => {
            if !state.config.is_soundcloud_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "SoundCloud not configured. Add SOUNDCLOUD_API_KEY to .env file"
                            .to_string(),
                    }),
                )
                    .into_response();
            }
        }
        "youtube" => {
            if !state.config.is_youtube_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "YouTube not configured. Add YOUTUBE_API_KEY to .env file"
                            .to_string(),
                    }),
                )
                    .into_response();
            }
        }
        _ => unreachable!(), // Already filtered above
    }

    // Generate authorization URL based on service
    match service.as_str() {
        "spotify" => {
            // Get credentials from .env configuration
            let client_id = match state.config.spotify_client_id() {
                Ok(id) => id,
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
                Ok(secret) => secret,
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

            tracing::debug!("Spotify OAuth - Client ID: {}", client_id);
            tracing::debug!(
                "Spotify OAuth - Redirect URI: {}",
                state.config.spotify_redirect_uri
            );

            // Create OAuth credentials and generate authorization URL for Spotify
            let creds = Credentials::new(client_id, client_secret);
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

            match spotify.get_authorize_url(false) {
                Ok(url) => Json(ApiResponse {
                    data: url.to_string(),
                })
                .into_response(),
                Err(e) => {
                    tracing::error!("Failed to generate authorization URL: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to generate authorization URL: {}", e),
                        }),
                    )
                        .into_response()
                }
            }
        }
        "soundcloud" => {
            // SoundCloud OAuth not yet implemented
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "SoundCloud OAuth not yet implemented".to_string(),
                }),
            )
                .into_response()
        }
        "youtube" => {
            // YouTube OAuth not yet implemented
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "YouTube OAuth not yet implemented".to_string(),
                }),
            )
                .into_response()
        }
        _ => unreachable!(), // Already filtered above
    }
}

async fn service_callback_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Deemix does not use OAuth callbacks
    if service == "deemix" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Deemix does not use OAuth callbacks".to_string(),
            }),
        )
            .into_response();
    }

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

    // Check if service is configured in .env file and get credentials
    match service.as_str() {
        "spotify" => {
            if !state.config.is_spotify_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
                    }),
                )
                    .into_response();
            }
        }
        "soundcloud" => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "SoundCloud OAuth not yet implemented".to_string(),
                }),
            )
                .into_response();
        }
        "youtube" => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "YouTube OAuth not yet implemented".to_string(),
                }),
            )
                .into_response();
        }
        _ => unreachable!(), // Already filtered above
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

    tracing::debug!("Spotify Callback - Client ID: {}", client_id);
    tracing::debug!(
        "Spotify Callback - Redirect URI: {}",
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

                if let Err(e) = crate::db::update_service_tokens(
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

async fn update_service_config_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Json(request): Json<UpdateServiceConfigRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match update_service_config(
        &state.db,
        &service,
        request.user_id.as_deref(),
        request.playlist_id.as_deref(),
    )
    .await
    {
        Ok(_) => Json(ApiResponse {
            data: format!("Service {} configuration updated", service),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to update service config: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to update service config: {}", e),
                }),
            )
                .into_response()
        }
    }
}

async fn service_config_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => Json(ApiResponse { data: config }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Service {} not configured", service),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn service_sync_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Handle different services
    match service.as_str() {
        "spotify" => super::spotify_sync::spotify_sync_handler(state, service)
            .await
            .into_response(),
        "soundcloud" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: "SoundCloud sync not yet implemented".to_string(),
            }),
        )
            .into_response(),
        "youtube" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: "YouTube sync not yet implemented".to_string(),
            }),
        )
            .into_response(),
        "deemix" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: "Deemix sync uses /api/services/deemix/queue".to_string(),
            }),
        )
            .into_response(),
        _ => unreachable!(), // Already filtered above
    }
}

async fn service_reset_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Clear tokens and mark as disconnected
    let now = chrono::Utc::now().timestamp();
    let result = if service == "deemix" {
        // For deemix, clear access_token, metadata_json and mark disconnected
        sqlx::query(
            r#"
            UPDATE service_config
            SET access_token = NULL, metadata_json = NULL,
                is_connected = 0, last_checked = ?, updated_at = ?
            WHERE service = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(&service)
        .execute(&state.db)
        .await
    } else {
        sqlx::query(
            r#"
            UPDATE service_config
            SET refresh_token = NULL, access_token = NULL, token_expiry = NULL,
                is_connected = 0, last_checked = ?, updated_at = ?
            WHERE service = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(&service)
        .execute(&state.db)
        .await
    };

    match result {
        Ok(_) => Json(ApiResponse {
            data: format!("Successfully reset connection for {}", service),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to reset service {}: {}", service, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to reset service: {}", e),
                }),
            )
                .into_response()
        }
    }
}

async fn service_fetch_counts_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Only implement for spotify for now
    if service != "spotify" {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: format!("Fetch counts not implemented for {}", service),
            }),
        )
            .into_response();
    }

    // Get service config from database
    let config = match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Service {} not configured", service),
                }),
            )
                .into_response();
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

    // Check if refresh_token and access_token are available
    let (refresh_token, access_token) = match (config.refresh_token, config.access_token) {
        (Some(refresh), Some(access)) => (refresh, access),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!(
                        "Tokens not configured for {}. Please authenticate first.",
                        service
                    ),
                }),
            )
                .into_response();
        }
    };

    // Create authenticated Spotify client
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

    let spotify_config = Config {
        token_refreshing: true,
        ..Default::default()
    };

    let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

    // Set the token manually
    {
        let token_lock = spotify.token.lock().await;
        if let Ok(mut guard) = token_lock {
            *guard = Some(Token {
                refresh_token: Some(refresh_token.clone()),
                access_token: access_token.clone(),
                expires_in: Duration::seconds(3600), // Default
                expires_at: config
                    .token_expiry
                    .and_then(|ts| DateTime::from_timestamp(ts, 0)),
                scopes: Default::default(),
            });
        } else {
            tracing::error!("Failed to acquire token lock");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: "Failed to initialize Spotify client".to_string(),
                }),
            )
                .into_response();
        }
    }

    // Fetch user's playlists just to count them
    let mut playlists_stream = spotify.current_user_playlists();
    let mut total_playlists = 0;
    let mut total_tracks = 0;

    while let Some(playlist_result) = playlists_stream.try_next().await.transpose() {
        match playlist_result {
            Ok(playlist) => {
                total_playlists += 1;
                tracing::debug!(
                    "Counting playlist: {} (#{})",
                    playlist.name,
                    total_playlists
                );

                // Count tracks in this playlist
                let mut items_stream =
                    spotify.playlist_items(playlist.id.clone(), None, Some(Market::FromToken));

                while let Some(item_result) = items_stream.try_next().await.transpose() {
                    match item_result {
                        Ok(item) => {
                            if item.track.is_some() {
                                total_tracks += 1;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to fetch playlist item while counting: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch playlist while counting: {}", e);
                break;
            }
        }
    }

    // Update the counts in database without clearing existing data
    let now = chrono::Utc::now().timestamp();
    if let Err(e) = sqlx::query(
        r#"
        UPDATE service_config
        SET remote_playlists_count = ?,
            remote_tracks_count = ?,
            last_synced = ?,
            updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(total_playlists as i64)
    .bind(total_tracks as i64)
    .bind(now)
    .bind(now)
    .bind(&service)
    .execute(&state.db)
    .await
    {
        tracing::warn!("Failed to update service counts: {}", e);
        // Continue anyway - we still return the counts we fetched
    }

    Json(ApiResponse {
        data: serde_json::json!({
            "service": service,
            "total_playlists": total_playlists,
            "total_tracks": total_tracks,
            "message": format!("Fetched counts: {} playlists, {} tracks", total_playlists, total_tracks)
        }),
    })
    .into_response()
}

async fn service_sync_status_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => Json(ApiResponse { data: config }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Service {} not configured", service),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse {
                data: format!("Failed to get service config: {}", e),
            }),
        )
            .into_response(),
    }
}

// ── Helper ────────────────────────────────────────────────────────────────

async fn get_service_connections(
    pool: &Pool<sqlx::Sqlite>,
    credentials: &ServiceCredentials,
) -> Result<Vec<ServiceConnection>> {
    // Query all service configurations
    let configs = sqlx::query_as::<_, ServiceConfig>(
        "SELECT * FROM service_config WHERE service IN ('spotify', 'soundcloud', 'youtube', 'deemix')",
    )
    .fetch_all(pool)
    .await?;

    // Create a map for quick lookup
    use std::collections::HashMap;
    let config_map: HashMap<String, ServiceConfig> = configs
        .into_iter()
        .map(|config| (config.service.clone(), config))
        .collect();

    // Expected services
    let expected_services = ["spotify", "soundcloud", "youtube", "deemix"];

    let mut connections = Vec::new();

    for service_name in &expected_services {
        let configured = match *service_name {
            "spotify" => credentials.is_spotify_configured(),
            "soundcloud" => credentials.is_soundcloud_configured(),
            "youtube" => credentials.is_youtube_configured(),
            // Deemix is configured via web UI (DB), not env vars
            "deemix" => config_map.contains_key("deemix"),
            _ => false,
        };

        // Get counts for this service
        let playlists_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM service_playlists WHERE service = ?",
        )
        .bind(*service_name)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        let tracks_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM service_tracks WHERE service = ?")
                .bind(*service_name)
                .fetch_one(pool)
                .await
                .unwrap_or(0);

        if let Some(config) = config_map.get(*service_name) {
            connections.push(ServiceConnection {
                service: config.service.clone(),
                configured,
                connected: config.is_connected,
                is_syncing: false, // Tracked in memory, not database
                last_sync: config.last_synced,
                playlists_local: playlists_count,
                tracks_local: tracks_count,
                playlists_remote: config.remote_playlists_count,
                tracks_remote: config.remote_tracks_count,
                sync_current_playlist: None, // Tracked in memory
                sync_current_track: None,    // Tracked in memory
                sync_total_playlists: None,  // Tracked in memory
                sync_total_tracks: None,     // Tracked in memory
                sync_log: None,              // Tracked in memory
            });
        } else {
            connections.push(ServiceConnection {
                service: service_name.to_string(),
                configured,
                connected: false,
                is_syncing: false,
                last_sync: None,
                playlists_local: playlists_count,
                tracks_local: tracks_count,
                playlists_remote: 0,
                tracks_remote: 0,
                sync_current_playlist: None,
                sync_current_track: None,
                sync_total_playlists: None,
                sync_total_tracks: None,
                sync_log: None,
            });
        }
    }

    Ok(connections)
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .without_v07_checks()
        .route("/api/services", get(services_handler))
        .route("/api/services/{service}/auth", post(service_auth_handler))
        .route(
            "/api/services/{service}/callback",
            get(service_callback_handler),
        )
        .route(
            "/api/services/{service}/config",
            get(service_config_handler),
        )
        .route(
            "/api/services/{service}/config",
            put(update_service_config_handler),
        )
        .route(
            "/api/services/{service}/fetch-counts",
            get(service_fetch_counts_handler),
        )
        .route(
            "/api/services/{service}/sync-status",
            get(service_sync_status_handler),
        )
        .route("/api/services/{service}/sync", post(service_sync_handler))
        .route("/api/services/{service}/reset", post(service_reset_handler))
}
