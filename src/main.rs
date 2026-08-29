use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use sqlx::{Pool, Sqlite, SqlitePool};
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::prelude::*;

use momos_music_manager::AppState;
use momos_music_manager::config::ServiceCredentials;
use momos_music_manager::tasks::TaskManager;

#[cfg(target_os = "macos")]
mod tray;

#[derive(Parser)]
#[command(name = "momos-music-manager")]
#[command(about = "Momo's Music Manager - Multi-service library sync for DJs")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the web server
    Serve {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        public_url: Option<String>,
        #[arg(long, default_value_t = false)]
        no_browser: bool,
    },
    /// Scan and import files from directory
    Scan {
        #[arg(help = "Directory to scan")]
        directory: String,
    },
    /// Show database status
    DbStatus,
    /// Scan a single file and print metadata
    ScanFile {
        #[arg(help = "Path to the file to scan")]
        path: String,
    },
    /// Export all database tables to dev-data/dump.json
    Dump {
        #[arg(long, default_value = "dev-data/dump.json")]
        output: String,
    },
    /// Import all database tables from dev-data/dump.json
    Restore {
        #[arg(long, default_value = "dev-data/dump.json")]
        input: String,
    },
    /// Install launch agent to auto-start on login (macOS only)
    InstallLaunchAgent,
    /// Remove the launch agent (macOS only)
    UninstallLaunchAgent,
    /// Show the status of the launch agent (macOS only)
    ServiceStatus,
    /// Deemix download queue actions
    Deemix {
        #[command(subcommand)]
        command: momos_music_manager::deemix::cli::DeemixCommand,
    },
    /// Telemetry: push DB snapshots or run the collector
    Telemetry {
        #[command(subcommand)]
        command: momos_music_manager::telemetry::TelemetryCommand,
    },
}

// ── CLI entry point ────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("warn,momos_music_manager=info,lofty=error")
    });

    let log_dir = std::path::PathBuf::from(std::env::var("MOMOS_LOG_DIR").unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".local/share/momos-music-manager/logs")
            .to_string_lossy()
            .to_string()
    }));
    std::fs::create_dir_all(&log_dir).ok();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "server.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let stdout_layer = tracing_subscriber::fmt::layer().with_ansi(true);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(non_blocking);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    std::mem::forget(_guard);
    dotenvy::dotenv_override().ok();

    let cli = Cli::parse();

    // Default to Serve when no subcommand (Finder .app launch has no args)
    let command = cli.command.unwrap_or(Commands::Serve {
        host: None,
        port: None,
        public_url: None,
        no_browser: false,
    });

    match command {
        Commands::Serve {
            host,
            port,
            public_url,
            no_browser,
        } => {
            let host = host.unwrap_or_else(|| ServiceCredentials::load().server_host);
            let port = port.unwrap_or_else(|| ServiceCredentials::load().server_port);

            #[cfg(target_os = "macos")]
            {
                let _ = no_browser; // used on non-macOS path only
                let (tx, rx) = std::sync::mpsc::channel::<tray::ServerShutdown>();
                let h = host.clone();
                let p = port;
                let pu = public_url.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
                    rt.block_on(async {
                        if let Err(e) = serve(h, p, pu, true).await {
                            tracing::error!("Server exited with error: {}", e);
                        }
                    });
                    let _ = tx.send(tray::ServerShutdown);
                });

                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tray::run(host, port, rx);
                }));
                return Ok(());
            }

            #[cfg(not(target_os = "macos"))]
            {
                let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
                rt.block_on(serve(host, port, public_url, no_browser))?;
                return Ok(());
            }
        }
        Commands::Scan { directory } => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(async {
                let db = create_db_pool().await?;
                scan_directory(&db, &directory).await
            })?;
        }
        Commands::DbStatus => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(async {
                let db = create_db_pool().await?;
                db_status(&db).await
            })?;
        }
        Commands::ScanFile { path } => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(async {
                let db = create_db_pool().await?;
                scan_single_file(&db, &path).await
            })?;
        }
        Commands::Dump { output } => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(async {
                let db = create_db_pool().await?;
                momos_music_manager::dump::export_dump(&db, &output).await
            })?;
        }
        Commands::Restore { input } => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(async {
                let db = create_db_pool().await?;
                momos_music_manager::dump::import_dump(&db, &input).await
            })?;
        }
        Commands::InstallLaunchAgent => {
            #[cfg(target_os = "macos")]
            {
                momos_music_manager::launch_agent::install()?;
                println!("Launch agent installed");
            }
            #[cfg(not(target_os = "macos"))]
            println!("Launch agents are only supported on macOS");
        }
        Commands::UninstallLaunchAgent => {
            #[cfg(target_os = "macos")]
            {
                momos_music_manager::launch_agent::uninstall()?;
                println!("Launch agent removed");
            }
            #[cfg(not(target_os = "macos"))]
            println!("Launch agents are only supported on macOS");
        }
        Commands::ServiceStatus => {
            #[cfg(target_os = "macos")]
            {
                match momos_music_manager::launch_agent::status() {
                    Ok(status) => println!("{}", status),
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            #[cfg(not(target_os = "macos"))]
            println!("Launch agents are only supported on macOS");
        }
        Commands::Deemix { command } => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(momos_music_manager::deemix::cli::run(command))?;
        }
        Commands::Telemetry { command } => {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            rt.block_on(momos_music_manager::telemetry::run(command))?;
        }
    }

    Ok(())
}

/// Create a database pool from the configured URL (default: `app.db`).
async fn create_db_pool() -> Result<Pool<Sqlite>> {
    let config = ServiceCredentials::load();
    let url = &config.database_url;
    let pool = SqlitePool::connect(url).await?;
    momos_music_manager::db::init_db(&pool).await?;
    Ok(pool)
}

/// Start the HTTP server with all background tasks.
async fn serve(
    host: String,
    port: u16,
    public_url: Option<String>,
    no_browser: bool,
) -> Result<()> {
    let config = ServiceCredentials::load();
    let db = create_db_pool().await?;

    // Ensure task_history table exists (idempotent — safe on every startup)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_history (
            id TEXT PRIMARY KEY,
            task_type TEXT NOT NULL,
            service TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            progress_percent REAL,
            progress_message TEXT,
            result_summary TEXT,
            error_message TEXT,
            logs TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER,
            updated_at INTEGER NOT NULL
        )",
    )
    .execute(&db)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_task_history_created ON task_history(created_at DESC)",
    )
    .execute(&db)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_task_history_type ON task_history(task_type)")
        .execute(&db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_task_history_status ON task_history(status)")
        .execute(&db)
        .await?;

    let task_manager = TaskManager::new_with_pool(db.clone());

    let public_url = public_url.or_else(|| config.server_public_url.clone());

    // Clones for background tasks
    let poller_db = db.clone();
    let poller_config = config.clone();
    let watcher_db = db.clone();
    let poller_cancel = tokio_util::sync::CancellationToken::new();
    let global_poller_db = db.clone();
    let global_poller_config = config.clone();
    let global_interval = config.global_poll_interval_secs;
    let global_cancel = poller_cancel.clone();
    let maint_interval = config.maintainer_interval_secs;
    let maint_full_scan_max_age = config.maintainer_full_scan_max_age_secs;
    let maint_backup_discovery_interval = config.maintainer_backup_discovery_interval_secs;
    let maint_auto_prune = config.maintainer_auto_prune;
    let maint_auto_cleanup_dirs = config.maintainer_auto_cleanup_dirs;
    let maint_traktor_import = config.maintainer_traktor_import_enabled;
    let maint_tm = task_manager.clone();
    let maint_cancel = poller_cancel.clone();

    let telemetry_interval = config.telemetry_interval_secs;
    let telemetry_enabled = config.telemetry_enabled;
    let telemetry_config = config.clone();

    let state = Arc::new(AppState {
        db,
        config,
        task_manager,
        embeddings: Mutex::new(None),
        category_means: tokio::sync::Mutex::new(None),
        public_url,
    });

    // Refresh materialized tag tables so comment computation is correct from startup.
    // Non-fatal: log and continue if a refresh fails.
    if let Err(e) = momos_music_manager::db::refresh_file_resolved_tags(&state.db).await {
        tracing::error!("Failed to refresh file_resolved_tags at startup: {}", e);
    }
    if let Err(e) = momos_music_manager::db::refresh_track_resolved_tags(&state.db).await {
        tracing::error!("Failed to refresh track_resolved_tags at startup: {}", e);
    }
    tracing::info!("Materialized tag tables refreshed at startup");

    // Query subscription count so the poller startup log is accurate
    let sub_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions")
        .fetch_one(&poller_db)
        .await
        .unwrap_or(0);

    // Spawn subscription poller — polls subscribed playlists every 30s
    let poller_tm = state.task_manager.clone();
    let poller_handle = tokio::spawn(async move {
        momos_music_manager::poller::start_subscription_poller(
            poller_db,
            poller_config,
            poller_tm,
            poller_cancel,
            sub_count,
        )
        .await;
    });
    let _poller_handle = poller_handle;

    // Start folder watcher — polls active folders every 5 minutes
    let mut folder_watcher = momos_music_manager::watch::FolderWatcher::new(
        watcher_db,
        state.task_manager.clone(),
        momos_music_manager::watch::FolderWatcherConfig::default(),
    );
    if let Err(e) = folder_watcher.start() {
        tracing::error!("Failed to start folder watcher: {}", e);
    } else {
        tracing::info!("Folder watcher started");
    }

    // Spawn global playlist poller
    if global_interval > 0 && global_poller_config.is_spotify_configured() {
        let global_tm = state.task_manager.clone();
        tokio::spawn(async move {
            momos_music_manager::global_poller::start_global_poller(
                global_poller_db,
                global_poller_config,
                global_tm,
                global_interval,
                global_cancel,
            )
            .await;
        });
        info!(
            "Global playlist poller started (interval: {}s)",
            global_interval
        );
    } else {
        info!(
            "Global playlist poller disabled (interval={} or Spotify not configured)",
            global_interval
        );
    }

    let _folder_watcher = folder_watcher;

    // Auto-reconcile on startup
    let recon_db = state.db.clone();
    let recon_tm = state.task_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let folders: Vec<momos_music_manager::db::Folder> = sqlx::query_as::<_, momos_music_manager::db::Folder>(
            "SELECT * FROM folders WHERE backup_path IS NOT NULL AND backup_path != '' AND auto_backup = 1",
        )
        .fetch_all(&recon_db)
        .await
        .unwrap_or_default();
        for folder in folders {
            let unbacked = momos_music_manager::db::get_unbacked_up_files(&recon_db, folder.id)
                .await
                .unwrap_or_default();
            if !unbacked.is_empty() {
                tracing::info!(
                    "Auto-reconcile: folder '{}' has {} unbacked files - starting reconcile",
                    folder.folder_path,
                    unbacked.len()
                );
                momos_music_manager::tasks::start_backup_folder_task(
                    &recon_tm, &recon_db, folder.id,
                )
                .await;
            } else {
                tracing::info!(
                    "Auto-reconcile: folder '{}' already fully backed up",
                    folder.folder_path
                );
            }
        }
    });

    // Auto-backpack-sync on startup
    let bp_db = state.db.clone();
    let bp_tm = state.task_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE backpack = 1")
            .fetch_one(&bp_db)
            .await
            .unwrap_or(0);
        if count > 0 {
            tracing::info!(
                "Startup backpack sync: {} backpack tags found, starting sync",
                count
            );
            momos_music_manager::tasks::start_backpack_sync_task(&bp_tm, &bp_db).await;
        } else {
            tracing::info!("Startup backpack sync: no backpack tags, skipping");
        }
    });

    // Auto-backup-consistency on startup: remove stale file_locations.backup entries
    // for files that exist in the DB but are no longer on the NAS.
    let cc_db = state.db.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        #[derive(sqlx::FromRow)]
        struct FolderBackupRow {
            id: i64,
            backup_path: String,
        }
        let folders: Vec<FolderBackupRow> = sqlx::query_as(
            "SELECT id, backup_path FROM folders WHERE backup_path IS NOT NULL AND backup_path != ''"
        )
        .fetch_all(&cc_db)
        .await
        .unwrap_or_default();

        for folder in &folders {
            if let Some((ssh_host, remote_base)) = folder.backup_path.split_once(':') {
                let engine = momos_music_manager::backup::BackupEngine::new(ssh_host.to_string());
                let max_depth: u32 = 2;
                match engine.list_remote_files_full(remote_base, max_depth).await {
                    Ok(remote_files) if !remote_files.is_empty() => {
                        match momos_music_manager::db::cleanup_stale_backup_entries(
                            &cc_db,
                            folder.id,
                            &remote_files,
                        )
                        .await
                        {
                            Ok(n) if n > 0 => tracing::info!(
                                "Startup consistency: removed {} stale backup entries from folder #{}",
                                n,
                                folder.id
                            ),
                            Ok(_) => {
                                tracing::info!("Startup consistency: folder #{} clean", folder.id)
                            }
                            Err(e) => tracing::warn!(
                                "Startup consistency: folder #{} error: {}",
                                folder.id,
                                e
                            ),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(
                        "Startup consistency: can't list folder #{}: {}",
                        folder.id,
                        e
                    ),
                }
            }
        }
    });

    // Start maintainer
    if maint_interval > 0 {
        let maint_db = state.db.clone();
        tokio::spawn(async move {
            momos_music_manager::maintainer::start_maintainer(
                maint_db,
                maint_tm,
                maint_interval,
                maint_full_scan_max_age,
                maint_backup_discovery_interval,
                maint_auto_prune,
                maint_auto_cleanup_dirs,
                maint_traktor_import,
                maint_cancel,
            )
            .await;
        });
        info!("Maintainer started (interval: {}s)", maint_interval);
    } else {
        info!("Maintainer disabled (interval=0)");
    }

    // Auto-backup poller: every 10 min
    let auto_db = state.db.clone();
    let auto_tm = state.task_manager.clone();
    tokio::spawn(async move {
        momos_music_manager::auto_backup::start_auto_backup_poller(auto_db, auto_tm).await;
    });

    // Telemetry loop: periodic DB snapshot + metadata push
    if telemetry_enabled && telemetry_interval > 0 {
        let tel_db = state.db.clone();
        let tel_tm = state.task_manager.clone();
        tokio::spawn(async move {
            momos_music_manager::telemetry::start_telemetry_loop(
                tel_db,
                telemetry_config,
                tel_tm,
                telemetry_interval,
            )
            .await;
        });
        info!("Telemetry loop started (interval: {}s)", telemetry_interval);
    } else {
        info!(
            "Telemetry loop disabled (enabled={telemetry_enabled}, interval={telemetry_interval}s)"
        );
    }

    // Build the application with routes.
    let app = momos_music_manager::build_router(state);

    let address = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let actual_addr = listener.local_addr()?;
    info!("Listening on http://{addr}/", addr = actual_addr);
    info!(
        "🚀 Momo's Music Manager v{} started",
        env!("CARGO_PKG_VERSION")
    );

    // Auto-open browser on startup (unless --no-browser)
    if !no_browser {
        let url = format!("http://{}:{}", host, port);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let _ = webbrowser::open(&url);
        });
    }

    axum::serve(listener, app).await?;

    Ok(())
}

async fn scan_directory(pool: &Pool<Sqlite>, directory: &str) -> Result<()> {
    use std::path::Path;
    let path = Path::new(directory);
    if !path.exists() {
        anyhow::bail!("Directory does not exist: {}", directory);
    }
    info!("Scanning directory: {}", path.display());
    momos_music_manager::db::scan_directory(pool, path).await?;
    Ok(())
}

async fn scan_single_file(pool: &Pool<Sqlite>, path_str: &str) -> Result<()> {
    use std::path::Path;
    let path = Path::new(path_str);
    if !path.exists() {
        anyhow::bail!("File does not exist: {}", path_str);
    }
    println!("Scanning single file: {}", path.display());
    println!();

    let file = momos_music_manager::db::extract_audio_metadata_from_file(path).await?;

    println!("Metadata:");
    println!("  Title:       {:?}", file.title);
    println!("  Artist:      {:?}", file.artist);
    println!("  Album:       {:?}", file.album);
    println!("  Album Artist:{:?}", file.album_artist);
    println!("  Genre:       {:?}", file.genre);
    println!("  Year:        {:?}", file.year);
    println!(
        "  Track:       {:?}/{:?}",
        file.track_number, file.total_tracks
    );
    println!(
        "  Disc:        {:?}/{:?}",
        file.disc_number, file.total_discs
    );
    println!("  Composer:    {:?}", file.composer);
    println!("  Comment:     {:?}", file.comment);
    println!("  BPM:         {:?}", file.bpm);
    println!("  Key:         {:?}", file.musical_key);
    println!("  ISRC:        {:?}", file.isrc);
    println!("  Duration:    {:?} ms", file.duration_ms);
    println!("  File Type:   {}", file.file_type);
    println!("  File Size:   {} bytes", file.file_size);
    println!();

    println!("Storing to database...");
    let stored =
        momos_music_manager::db::scan_and_store_file(pool, Path::new(path_str), None).await?;
    println!("Stored with id: {}", stored.id);
    Ok(())
}

async fn db_status(pool: &Pool<Sqlite>) -> Result<()> {
    use sqlx::Row;

    let tables = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .fetch_all(pool)
        .await?;

    println!("Database tables:");
    for row in tables {
        let name: String = row.get("name");
        println!("  - {}", name);
    }

    let files_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let service_tracks_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM service_tracks")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let tags_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    println!("\nRecord counts:");
    println!("  Files: {}", files_count);
    println!("  Service tracks: {}", service_tracks_count);
    println!("  Tags: {}", tags_count);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use sqlx::SqlitePool;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn args<'a>(args: &'a [&'a str]) -> Vec<&'a str> {
        let mut v = vec!["momos-music-manager"];
        v.extend_from_slice(args);
        v
    }

    #[test]
    fn test_cli_serve_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["serve", "--host", "127.0.0.1", "--port", "3000"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("serve"));
    }

    #[test]
    fn test_cli_serve_parses_minimal() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["serve"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("serve"));
    }

    #[test]
    fn test_cli_scan_file_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["scan-file", "/path/to/file.flac"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("scan-file"));
    }

    #[test]
    fn test_cli_dump_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["dump"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("dump"));
    }

    #[test]
    fn test_cli_dump_custom_output() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["dump", "--output", "/tmp/custom.json"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("dump"));
        let sub = matches.subcommand_matches("dump").unwrap();
        let output: String = sub.get_one::<String>("output").cloned().unwrap();
        assert_eq!(output, "/tmp/custom.json");
    }

    #[test]
    fn test_cli_restore_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["restore", "--input", "/tmp/dump.json"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("restore"));
    }

    #[test]
    fn test_cli_scan_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["scan", "/music/stems"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("scan"));
    }

    #[test]
    fn test_cli_db_status_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["db-status"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("db-status"));
    }

    #[test]
    fn test_cli_invalid_subcommand() {
        let result = Cli::command().try_get_matches_from(args(&["bogus"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_serve_public_url() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["serve", "--public-url", "https://example.com"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("serve"));
    }

    #[test]
    fn test_cli_help_contains_all_subcommands() {
        let mut cmd = Cli::command();
        let help = cmd.render_help().to_string();
        // Every top-level subcommand name should appear in the help text
        assert!(help.contains("serve"), "help should mention 'serve'");
        assert!(help.contains("scan"), "help should mention 'scan'");
        assert!(
            help.contains("scan-file"),
            "help should mention 'scan-file'"
        );
        assert!(help.contains("dump"), "help should mention 'dump'");
        assert!(help.contains("restore"), "help should mention 'restore'");
        assert!(
            help.contains("db-status"),
            "help should mention 'db-status'"
        );
        assert!(help.contains("deemix"), "help should mention 'deemix'");
        assert!(
            help.contains("telemetry"),
            "help should mention 'telemetry'"
        );
    }

    #[test]
    fn test_cli_serve_invalid_port_value() {
        let result =
            Cli::command().try_get_matches_from(args(&["serve", "--port", "not-a-number"]));
        assert!(result.is_err(), "non-numeric --port should fail");
    }

    #[test]
    fn test_cli_scan_missing_path() {
        // The `scan` subcommand requires a positional directory argument
        let result = Cli::command().try_get_matches_from(args(&["scan"]));
        assert!(result.is_err(), "scan without a path should fail");
    }

    #[test]
    fn test_cli_install_launch_agent_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["install-launch-agent"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("install-launch-agent"));
    }

    #[test]
    fn test_cli_uninstall_launch_agent_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["uninstall-launch-agent"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("uninstall-launch-agent"));
    }

    #[test]
    fn test_cli_service_status_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["service-status"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("service-status"));
    }

    #[test]
    fn test_cli_deemix_requires_subcommand() {
        // The `deemix` subcommand has its own sub-subcommands
        let result = Cli::command().try_get_matches_from(args(&["deemix"]));
        assert!(
            result.is_err(),
            "deemix without a sub-subcommand should fail"
        );
    }

    #[test]
    fn test_cli_telemetry_requires_subcommand() {
        // The `telemetry` subcommand has its own sub-subcommands
        let result = Cli::command().try_get_matches_from(args(&["telemetry"]));
        assert!(
            result.is_err(),
            "telemetry without a sub-subcommand should fail"
        );
    }

    #[test]
    fn test_cli_telemetry_push_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["telemetry", "push"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("telemetry"));
    }

    #[test]
    fn test_cli_telemetry_receive_parses() {
        let matches = Cli::command()
            .try_get_matches_from(args(&["telemetry", "receive"]))
            .unwrap();
        assert_eq!(matches.subcommand_name(), Some("telemetry"));
    }

    #[tokio::test]
    async fn test_build_router_creates_router() {
        // Verify that build_router() returns without panicking when given
        // a valid AppState with an in-memory database. Full route-level
        // testing is done in the integration tests.
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");
        let config = ServiceCredentials::load();
        let task_manager = TaskManager::new();
        let state = Arc::new(AppState {
            db: pool,
            config,
            task_manager,
            embeddings: Mutex::new(None),
            category_means: tokio::sync::Mutex::new(None),
            public_url: None,
        });
        let _router = momos_music_manager::build_router(state);
    }
}
