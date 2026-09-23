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

    let http = reqwest::Client::builder()
        .cookie_store(true)
        .timeout(Duration::from_secs(60))
        .build()?;

    let state = Arc::new(AppState {
        pool,
        config,
        http,
        notify: Arc::new(Notify::new()),
        deemix_login: Default::default(),
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
