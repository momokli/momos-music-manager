//! music-api — an ISRC order/consume API in front of deemix.
//!
//! Consumers (MMM) place *orders* of ISRCs; this service resolves them on
//! Deezer, downloads the best available quality through a dedicated deemix
//! instance, derives the lossy variants and serves the files back by ISRC.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod deemix;
pub mod deezer;
pub mod models;
pub mod spotdl;
pub mod store;
pub mod transcode;
pub mod worker;
pub mod ytdlp;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::middleware;
use axum::routing::get;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::Notify;

pub use config::Config;

/// Shared application state handed to every handler and the worker.
pub struct AppState {
    pub pool: sqlx::Pool<sqlx::Sqlite>,
    pub config: Config,
    pub http: reqwest::Client,
    /// Wakes the worker when a new order arrives.
    pub notify: Arc<Notify>,
    /// Login-attempt backoff for the deemix session.
    pub deemix_login: worker::LoginBackoff,
    /// Content-addressed object store.
    pub store: store::Store,
}

/// Build the fully-stated router: public `/health`, everything else behind the
/// bearer check.
pub fn build_router(state: Arc<AppState>) -> Router {
    let protected = api::router().layer(middleware::from_fn_with_state(
        state.clone(),
        auth::require_bearer,
    ));
    Router::new()
        .route("/health", get(api::health))
        .merge(protected)
        .with_state(state)
}

/// Open (creating if needed) the SQLite database backing the service.
pub async fn connect_db(config: &Config) -> anyhow::Result<sqlx::Pool<sqlx::Sqlite>> {
    let path: PathBuf = match std::env::var("DATABASE_URL") {
        Ok(url) => PathBuf::from(url.trim_start_matches("sqlite:")),
        Err(_) => config.data_dir.join("music-api.db"),
    };

    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?)
}
