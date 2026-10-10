//! Binary entry point for music-api.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::EnvFilter;

use music_api::{AppState, Config, build_router, connect_db, db, worker};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;

    for dir in [
        config.data_dir.clone(),
        config.flac_dir(),
        config.mp3_320_dir(),
        config.mp3_128_dir(),
        config.deemix_download_dir.clone(),
    ] {
        tokio::fs::create_dir_all(&dir).await?;
    }

    let pool = connect_db(&config).await?;
    db::init(&pool).await?;

    let store =
        music_api::store::Store::new(config.store_root.clone(), config.store_max_upload_bytes);
    music_api::store::init(&pool, &store).await?;

    let http = reqwest::Client::builder()
        .cookie_store(true)
        .timeout(Duration::from_secs(60))
        .build()?;

    // Warn early when the ARL cannot stream FLAC — otherwise every download
    // silently falls back to a lower bitrate (see issue #233).
    if let Some(tier) = music_api::deezer::check_arl_tier(&http, &config.deemix_arl).await {
        if tier.can_stream_flac() {
            info!("ARL tier: {} (lossless available)", tier.offer);
        } else {
            tracing::warn!(
                "ARL tier: {} — FLAC/HQ unavailable, downloads fall back to a lower bitrate",
                tier.offer
            );
        }
    }

    let state = Arc::new(AppState {
        pool,
        config,
        http,
        notify: Arc::new(Notify::new()),
        deemix_login: Default::default(),
        store,
    });

    let worker_state = state.clone();
    tokio::spawn(async move { worker::run(worker_state).await });

    let listener = tokio::net::TcpListener::bind(&state.config.bind).await?;
    info!(
        "music-api listening on {} (deemix {}, download dir {})",
        state.config.bind,
        state.config.deemix_url,
        state.config.deemix_download_dir.display()
    );

    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
