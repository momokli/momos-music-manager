//! momos-music-manager library crate.
//!
//! Contains all shared types, modules, and the Axum router builder.
//! The binary crate (`main.rs`) provides the CLI entry point and spawns
//! background tasks. This library crate is what integration tests import.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::Embed;
use sqlx::{Pool, Sqlite};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

pub mod api;
pub mod audio_extensions;
pub mod backup;
pub mod comment;
pub mod config;
pub mod db;
pub mod deemix;
pub mod digging;
pub mod dump;
pub mod embeddings;
pub mod global_poller;
pub mod maintainer;
pub mod poller;
pub mod scan_cache;
pub mod spotify;
pub mod tasks;
pub mod traktor;
pub mod watch;

#[cfg(target_os = "macos")]
pub mod launch_agent;

/// Per-category mean embedding vectors used for auto-categorization.
pub type CategoryMeans = HashMap<i64, (String, Vec<f32>)>;

/// Application state shared across all route handlers via Axum's State extractor.
pub struct AppState {
    pub db: Pool<Sqlite>,
    pub config: crate::config::ServiceCredentials,
    pub task_manager: crate::tasks::TaskManager,
    pub embeddings: Mutex<Option<crate::embeddings::EmbeddingModel>>,
    pub category_means: tokio::sync::Mutex<Option<CategoryMeans>>,
    pub public_url: Option<String>,
}

// ── Embedded Frontend Assets ───────────────────────────────────────────────

#[derive(Embed)]
#[folder = "frontend/dist/"]
struct FrontendAssets;

/// Infer a MIME type from the file extension.
fn mime_for_path(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

/// Helper: serve the embedded index.html with the correct content type.
fn index_html_response() -> Response {
    FrontendAssets::get("index.html")
        .map(|content| {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(content.data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        })
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

/// Handler for the bare root path `/` — serves index.html.
async fn root_handler() -> Response {
    index_html_response()
}

/// Catch-all handler that serves embedded frontend assets.
///
/// - Exact file paths (e.g. `/app.js`, `/style.css`) return the file directly.
/// - `/` and any unknown path return `index.html` (SPA fallback for client-side
///   routing via hash fragments).
/// - `/api/*` paths are never routed here because the API router takes priority.
async fn static_handler(Path(path): Path<String>) -> Response {
    // Normalise: strip leading slash, default to index.html
    let asset_path = if path.is_empty() || path == "/" || path.starts_with("api/") {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };

    match FrontendAssets::get(asset_path) {
        Some(content) => Response::builder()
            .header(header::CONTENT_TYPE, mime_for_path(asset_path))
            .body(axum::body::Body::from(content.data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => {
            // SPA fallback — let the client-side router handle it
            FrontendAssets::get("index.html")
                .map(|_| index_html_response())
                .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
        }
    }
}

/// Build the Axum router from AppState. Extracted for testability.
/// Does NOT spawn background tasks (pollers, watchers, maintainer).
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .without_v07_checks()
        .merge(api::router())
        .route("/", get(root_handler))
        .route("/{*path}", get(static_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
