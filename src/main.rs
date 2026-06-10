use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use sqlx::{Pool, Sqlite, SqlitePool};
use tokio::sync::Mutex;
use tracing::info;

use momos_music_manager::AppState;
use momos_music_manager::config::ServiceCredentials;
use momos_music_manager::tasks::TaskManager;

#[derive(Parser)]
#[command(name = "momos-music-manager")]
#[command(about = "Momo's Music Manager - Multi-service library sync for DJs")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
}

// ── CLI entry point ────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("warn,momos_music_manager=info,lofty=error")
            }),
        )
        .init();
    dotenvy::dotenv_override().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            host,
            port,
            public_url,
        } => {
            let host = host.unwrap_or_else(|| ServiceCredentials::load().server_host);
            let port = port.unwrap_or_else(|| ServiceCredentials::load().server_port);
            serve(host, port, public_url).await?;
        }
        Commands::Scan { directory } => {
            let db = create_db_pool().await?;
            scan_directory(&db, &directory).await?;
        }
        Commands::DbStatus => {
            let db = create_db_pool().await?;
            db_status(&db).await?;
        }
        Commands::ScanFile { path } => {
            let db = create_db_pool().await?;
            scan_single_file(&db, &path).await?;
        }
        Commands::Dump { output } => {
            let db = create_db_pool().await?;
            momos_music_manager::dump::export_dump(&db, &output).await?;
        }
        Commands::Restore { input } => {
            let db = create_db_pool().await?;
            momos_music_manager::dump::import_dump(&db, &input).await?;
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
            momos_music_manager::deemix::cli::run(command).await?;
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
async fn serve(host: String, port: u16, public_url: Option<String>) -> Result<()> {
    let config = ServiceCredentials::load();
    let db = create_db_pool().await?;
    let task_manager = TaskManager::new();

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
    let poller_handle = tokio::spawn(async move {
        momos_music_manager::poller::start_subscription_poller(
            poller_db,
            poller_config,
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
        tokio::spawn(async move {
            momos_music_manager::global_poller::start_global_poller(
                global_poller_db,
                global_poller_config,
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
        let interval = Duration::from_secs(600);
        loop {
            tokio::time::sleep(interval).await;
            let folders: Vec<momos_music_manager::db::Folder> = match sqlx::query_as::<_, momos_music_manager::db::Folder>(
                "SELECT * FROM folders WHERE auto_backup = 1 AND backup_path IS NOT NULL AND backup_path != ''"
            )
            .fetch_all(&auto_db)
            .await
            {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("Auto-backup: failed to query folders: {}", e);
                    continue;
                }
            };
            for folder in &folders {
                let unbacked =
                    match momos_music_manager::db::get_unbacked_up_files(&auto_db, folder.id).await
                    {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!(
                                "Auto-backup: failed to check files for folder {}: {}",
                                folder.id,
                                e
                            );
                            continue;
                        }
                    };
                if !unbacked.is_empty() {
                    tracing::info!(
                        "Auto-backup: folder '{}' has {} unbacked files — starting backup task",
                        folder.folder_path,
                        unbacked.len()
                    );
                    momos_music_manager::tasks::start_backup_folder_task(
                        &auto_tm, &auto_db, folder.id,
                    )
                    .await;
                }
            }
        }
    });

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
    let stored = momos_music_manager::db::scan_and_store_file(pool, Path::new(path_str)).await?;
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
