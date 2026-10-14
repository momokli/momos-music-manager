//! Binary entry point for music-api.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::info;
use tracing_subscriber::EnvFilter;

use music_api::{build_router, connect_db, db, worker, AppState, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!(
            "music-api {} ({})",
            music_api::VERSION,
            music_api::GIT_DESCRIBE
        );
        return Ok(());
    }

    let config = Config::from_env()?;

    // One-shot vault backfill. Deliberately **off the boot path**: the server
    // never blocks on it. Batched, transactional and idempotent — safe to re-run.
    if std::env::args().skip(1).any(|a| a == "backfill-vault") {
        let batch = std::env::var("VAULT_BACKFILL_BATCH")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(500)
            .max(1);
        let pool = connect_db(&config).await?;
        db::init(&pool).await?;
        let stats = music_api::vault::backfill(&pool, batch).await?;
        println!(
            "backfill-vault: {} tracks, {} analyses, {} batches (batch size {})",
            stats.tracks, stats.analyses, stats.batches, batch
        );
        return Ok(());
    }

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

    let ytdlp_concurrency = config.ytdlp_concurrency;
    let transcode_concurrency = config.transcode_concurrency;
    let analysis_concurrency = config.analysis_concurrency;
    let state = Arc::new(AppState {
        pool,
        config,
        http,
        notify: Arc::new(Notify::new()),
        deemix_login: Default::default(),
        store,
        logs: music_api::logbuf::LogBuffer::new(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ytdlp_sem: Arc::new(tokio::sync::Semaphore::new(ytdlp_concurrency)),
        finalize_sem: Arc::new(tokio::sync::Semaphore::new(transcode_concurrency)),
        finalizing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_sem: Arc::new(tokio::sync::Semaphore::new(analysis_concurrency)),
        analyzing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_backoff: Default::default(),
        artist_ingesting: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
    });

    let worker_state = state.clone();
    tokio::spawn(async move { worker::run(worker_state).await });

    let listener = tokio::net::TcpListener::bind(&state.config.bind).await?;
    info!(
        version = music_api::VERSION,
        git_sha = music_api::GIT_SHA,
        "music-api listening on {} (deemix {}, download dir {})",
        state.config.bind,
        state.config.deemix_url,
        state.config.deemix_download_dir.display()
    );
    match state.config.analyzer_url.as_deref() {
        Some(url) => info!("BPM/key analysis enabled (analyzer {url})"),
        None => info!("BPM/key analysis disabled (ANALYZER_URL not set)"),
    }

    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
