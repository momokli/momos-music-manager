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

    // CLI: `music-api ssh-ingest --source <name> | --all`
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ssh-ingest") {
        return run_ssh_ingest_cli(config, &args[1..]).await;
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

    // Optional background SSH pull-ingest (Part A).
    if !state.config.ssh_sources.is_empty() && !state.config.ssh_ingest_interval.is_zero() {
        let tick_state = state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick_state.config.ssh_ingest_interval);
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                for source in tick_state.config.ssh_sources.clone() {
                    let pool = tick_state.pool.clone();
                    let store = tick_state.store.clone();
                    let staging = tick_state.config.data_dir.join("ssh-staging");
                    if let Err(e) =
                        music_api::ssh_ingest::run_and_record(&pool, &store, &staging, &source)
                            .await
                    {
                        tracing::warn!("ssh-ingest {} failed: {e:#}", source.name);
                    }
                }
            }
        });
    }

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

/// `music-api ssh-ingest --source <name> | --all` — run a pull from the CLI.
async fn run_ssh_ingest_cli(config: Config, args: &[String]) -> anyhow::Result<()> {
    let mut source_name: Option<String> = None;
    let mut all = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => all = true,
            "--source" => {
                i += 1;
                source_name = args.get(i).cloned();
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
        i += 1;
    }

    let sources: Vec<_> = if all {
        config.ssh_sources.clone()
    } else if let Some(name) = &source_name {
        config
            .ssh_sources
            .iter()
            .filter(|s| &s.name == name)
            .cloned()
            .collect()
    } else {
        anyhow::bail!("usage: music-api ssh-ingest --source <name> | --all");
    };
    if sources.is_empty() {
        anyhow::bail!("no matching SSH source (configure SSH_SOURCES / SSH_SOURCES_FILE)");
    }

    let pool = connect_db(&config).await?;
    db::init(&pool).await?;
    let store =
        music_api::store::Store::new(config.store_root.clone(), config.store_max_upload_bytes);
    music_api::store::init(&pool, &store).await?;
    let staging = config.data_dir.join("ssh-staging");

    for source in sources {
        println!(
            "→ pulling {} ({}:{})",
            source.name, source.host, source.path
        );
        let (run_id, s) =
            music_api::ssh_ingest::run_and_record(&pool, &store, &staging, &source).await?;
        println!(
            "  run {run_id}: {} files, {} new, {} present, {} failed, {:.1} MiB new",
            s.files,
            s.stored,
            s.skipped,
            s.failed,
            s.bytes as f64 / 1048576.0
        );
    }
    Ok(())
}
