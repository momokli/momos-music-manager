//! music-api — an ISRC order/consume API in front of deemix.
//!
//! Consumers (MMM) place *orders* of ISRCs; this service resolves them on
//! Deezer, downloads the best available quality through a dedicated deemix
//! instance, derives the lossy variants and serves the files back by ISRC.

pub mod analyzer;
pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod deemix;
pub mod deezer;
pub mod logbuf;
pub mod models;
pub mod store;
pub mod tags;
pub mod transcode;
pub mod vault;
pub mod work;
pub mod worker;
pub mod ytdlp;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::middleware;
use axum::routing::get;
use axum::Router;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::Notify;

pub use config::Config;

/// Full build version (git-derived). Set by `build.rs`.
pub const VERSION: &str = env!("BUILD_VERSION");
pub const GIT_SHA: &str = env!("BUILD_GIT_SHA");
pub const GIT_DESCRIBE: &str = env!("BUILD_GIT_DESCRIBE");

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
    /// Recent service events (surfaced via `GET /logs`).
    pub logs: logbuf::LogBuffer,
    /// When set, the worker does not pick up new work.
    pub paused: Arc<std::sync::atomic::AtomicBool>,
    /// Bounds concurrent yt-dlp downloads (non-Deezer sources).
    pub ytdlp_sem: Arc<tokio::sync::Semaphore>,
    /// Bounds concurrent `finalize` (copy + transcode) jobs.
    pub finalize_sem: Arc<tokio::sync::Semaphore>,
    /// ISRCs with a `finalize` task currently in flight (dedupe guard).
    pub finalizing: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Bounds concurrent analyzer calls (BPM/key, CPU-heavy).
    pub analysis_sem: Arc<tokio::sync::Semaphore>,
    /// ISRCs with an analysis task currently in flight (dedupe guard).
    pub analyzing: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Backoff for analyzer calls so a dead analyzer is not retried every tick.
    pub analysis_backoff: worker::LoginBackoff,
    /// Artist-order ids with an ingest task currently in flight (dedupe guard).
    pub artist_ingesting: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Cached Deezer account tier for the configured ARL (`None` until probed).
    pub arl_tier: Arc<tokio::sync::RwLock<Option<deezer::ArlTier>>>,
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

/// Probe the configured ARL's Deezer tier once at startup and cache it.
///
/// Meant to be **spawned**, never awaited on the boot path: the network call is
/// bounded and all failures are swallowed into a warning. When the requested
/// bitrate is FLAC (`9`) but the tier cannot stream lossless, a warning is
/// logged so operators can see why downloads fall back.
pub async fn init_arl_tier(state: &Arc<AppState>) {
    let arl = state.config.deemix_arl.trim();
    if arl.is_empty() {
        tracing::warn!("DEEMIX_ARL not configured — Deezer account tier unknown");
        state
            .logs
            .push("warn", "DEEMIX_ARL not configured — Deezer tier unknown");
        return;
    }

    match deezer::fetch_arl_tier(&state.http, &state.config.deezer_gw_base, arl).await {
        Ok(Some(tier)) => {
            tracing::info!(
                offer = %tier.offer,
                lossless = tier.lossless,
                "Deezer ARL tier detected"
            );
            state.logs.push(
                "info",
                format!(
                    "Deezer ARL tier: {} (lossless={})",
                    tier.offer, tier.lossless
                ),
            );
            if state.config.deemix_bitrate == 9 && !tier.can_stream_flac() {
                let msg = format!(
                    "DEEMIX_BITRATE=9 requested but ARL tier '{}' has no lossless — downloads fall back",
                    tier.offer
                );
                tracing::warn!("{msg}");
                state.logs.push("warn", msg);
            }
            *state.arl_tier.write().await = Some(tier);
        }
        Ok(None) => {
            tracing::warn!("Deezer ARL tier unavailable (empty account data)");
            state
                .logs
                .push("warn", "Deezer ARL tier unavailable (empty account data)");
        }
        Err(e) => {
            tracing::warn!("failed to fetch Deezer ARL tier: {e:#}");
            state
                .logs
                .push("warn", format!("failed to fetch Deezer ARL tier: {e}"));
        }
    }
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
