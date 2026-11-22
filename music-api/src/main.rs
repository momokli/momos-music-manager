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

    // Per-file catalogue backfill (#700). Read-only on media files; `--apply`
    // writes `track_files` rows, `--dry-run` only reports. Never on the boot
    // path: needs file IO and ffprobe.
    if std::env::args().skip(1).any(|a| a == "archive-backfill") {
        let args: Vec<String> = std::env::args().collect();
        let apply = args.iter().any(|a| a == "--apply");
        let dry = args.iter().any(|a| a == "--dry-run");
        if apply == dry {
            anyhow::bail!("usage: music-api archive-backfill <--dry-run|--apply>");
        }
        let pool = connect_db(&config).await?;
        db::init(&pool).await?;
        let store = music_api::store::Store::new(
            config.store_root.clone(),
            config.store_max_upload_bytes,
        );
        music_api::store::init(&pool, &store).await?;
        let report =
            music_api::archive::backfill(&pool, &config, &store, apply).await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        if !report.missing_files.is_empty() {
            eprintln!(
                "warning: {} catalogued file(s) are missing on disk",
                report.missing_files.len()
            );
        }
        return Ok(());
    }

    // Legacy archive relocation (#702). Moves only provably-duplicate legacy
    // copies under `_archive/` (rename, never delete/copy). `--dry-run` writes
    // neither file nor row. Never on the boot path: needs file IO + ffmpeg.
    if std::env::args().skip(1).any(|a| a == "archive-legacy") {
        let args: Vec<String> = std::env::args().collect();
        let apply = args.iter().any(|a| a == "--apply");
        let dry = args.iter().any(|a| a == "--dry-run");
        if apply == dry {
            anyhow::bail!("usage: music-api archive-legacy <--dry-run|--apply>");
        }
        let pool = connect_db(&config).await?;
        db::init(&pool).await?;
        let store = music_api::store::Store::new(
            config.store_root.clone(),
            config.store_max_upload_bytes,
        );
        music_api::store::init(&pool, &store).await?;
        let report =
            music_api::archive::archive_legacy(&pool, &config, &store, apply).await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        if report.aborted {
            eprintln!(
                "warning: aborted — {}",
                report.aborted_reason.as_deref().unwrap_or("unknown reason")
            );
            // A partial/aborted relocation is a failure, not a success: signal
            // it with a non-zero exit code so scripts can detect it.
            std::process::exit(2);
        }
        return Ok(());
    }

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

    // One-shot repair of degraded rows (#699): a track with a `path_*` on disk
    // is servable and must not stay `failed`/`absent`/`pending`. `--dry-run`
    // (the default) only reports; `--apply` stats each path and re-derives the
    // state (never a blind `UPDATE`; `error` is preserved).
    if std::env::args().skip(1).any(|a| a == "repair-states") {
        let apply = std::env::args().skip(1).any(|a| a == "--apply");
        let pool = connect_db(&config).await?;
        db::init(&pool).await?;
        let stats = db::repair_states(&pool, apply).await?;
        println!(
            "repair-states ({}): {} checked, {} {}, {} with paths missing on disk",
            if apply { "apply" } else { "dry-run" },
            stats.checked,
            stats.repaired,
            if apply { "repaired" } else { "repairable" },
            stats.paths_missing_on_disk,
        );
        return Ok(());
    }

    for dir in [
        config.data_dir.clone(),
        config.flac_dir(),
        config.mp3_320_dir(),
        config.mp3_128_dir(),
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
        native_session: Arc::new(tokio::sync::RwLock::new(None)),
        native_login: Default::default(),
        native_fake: None,
        store,
        logs: music_api::logbuf::LogBuffer::new(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ytdlp_sem: Arc::new(tokio::sync::Semaphore::new(ytdlp_concurrency)),
        finalize_sem: Arc::new(tokio::sync::Semaphore::new(transcode_concurrency)),
        analysis_sem: Arc::new(tokio::sync::Semaphore::new(analysis_concurrency)),
        analyzing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        analysis_backoff: Default::default(),
        arl_tier: Arc::new(tokio::sync::RwLock::new(None)),
    });

    let worker_state = state.clone();
    tokio::spawn(async move { worker::run(worker_state).await });

    // Tier probe is deliberately off the boot path: a spawned task with its own
    // timeout, so a slow/dead Deezer gw-light never blocks startup.
    let arl_state = state.clone();
    tokio::spawn(async move { music_api::init_arl_tier(&arl_state).await });

    let listener = tokio::net::TcpListener::bind(&state.config.bind).await?;
    info!(
        version = music_api::VERSION,
        git_sha = music_api::GIT_SHA,
        quality = ?state.config.download_quality,
        "music-api listening on {} (native Deezer downloads)",
        state.config.bind
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
