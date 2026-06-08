//! API surface: domain modules, each with its own types, handlers, and sub-router.

use axum::Router;
use std::sync::Arc;

use crate::AppState;

pub mod daily;
pub mod deemix_api;
pub mod digging;
pub mod explorer;
pub mod files;
pub mod folders;
pub mod infrastructure;
pub mod playlists;
pub mod services;
pub mod spotify_sync;
pub mod storage;
pub mod tags;
pub mod tracks;
pub mod traktor;
pub mod types;
pub mod websocket;

/// Build the merged API router from all domain sub-routers.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(infrastructure::router())
        .merge(deemix_api::router())
        .merge(daily::router())
        .merge(traktor::router())
        .merge(explorer::router())
        .merge(digging::router())
        .merge(services::router())
        .merge(spotify_sync::router())
        .merge(websocket::router())
        .merge(playlists::router())
        .merge(storage::router())
        .merge(folders::router())
        .merge(tags::router())
        .merge(tracks::router())
        .merge(files::router())
}
