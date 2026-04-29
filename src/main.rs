use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

use anyhow::Result;
use axum::{Router, response::Html, routing::get};
use clap::{Parser, Subcommand};
use dotenvy;
use sqlx::{Pool, Sqlite, SqlitePool, sqlite::SqliteConnectOptions};
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber;

mod api;
mod audio_extensions;
mod comment;
mod config;
mod db;
mod digging;
mod embeddings;
mod scan_cache;
mod spotify;

mod dump;
mod poller;
mod tasks;
mod traktor;
mod watch;

#[derive(Parser)]
#[command(name = "momos-music-manager")]
#[command(about = "Momo's Music Manager - Multi-service library sync for DJs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the web server
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value = "3000")]
        port: u16,
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
}

struct AppState {
    db: Pool<Sqlite>,
    config: crate::config::ServiceCredentials,
    task_manager: crate::tasks::TaskManager,
    embeddings: Mutex<Option<crate::embeddings::EmbeddingModel>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,momos_music_manager=debug,lofty=error")
            }),
        )
        .init();
    dotenvy::dotenv_override().ok();

    let cli = Cli::parse();

    // Initialize database connection
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:app.db".to_string());
    info!("Database: {database_url}");
    let options = SqliteConnectOptions::from_str(&database_url)?.create_if_missing(true);
    let db = SqlitePool::connect_with(options).await?;

    // Run migrations
    sqlx::migrate!().run(&db).await?;
    info!("Migrations complete");

    match cli.command {
        Commands::Serve { host, port } => {
            serve(db, host, port).await?;
        }
        Commands::Scan { directory } => {
            info!("Scanning: {directory}");
            scan_directory(&db, &directory).await?;
        }
        Commands::DbStatus => {
            info!("Database status");
            db_status(&db).await?;
        }
        Commands::ScanFile { path } => {
            info!("Scan file: {path}");
            scan_single_file(&db, &path).await?;
        }
        Commands::Dump { output } => {
            crate::dump::export_dump(&db, &output).await?;
        }
        Commands::Restore { input } => {
            crate::dump::import_dump(&db, &input).await?;
        }
    }

    Ok(())
}

async fn serve(db: Pool<Sqlite>, host: String, port: u16) -> Result<()> {
    let config = crate::config::ServiceCredentials::from_env();
    let task_manager = crate::tasks::TaskManager::new();

    // Clone for subscription poller (background task needs own ownership)
    let poller_db = db.clone();
    let poller_config = config.clone();
    let poller_cancel = tokio_util::sync::CancellationToken::new();

    let pruner_tm = task_manager.clone();

    let state = Arc::new(AppState {
        db,
        config,
        task_manager,
        embeddings: tokio::sync::Mutex::new(None),
    });

    // Spawn subscription poller — polls subscribed playlists every 30s
    let poller_handle = tokio::spawn(async move {
        crate::poller::start_subscription_poller(poller_db, poller_config, poller_cancel).await;
    });
    // Keep poller alive for the lifetime of the server
    let _poller_handle = poller_handle;

    // Spawn background task pruner — removes completed/failed/cancelled tasks
    // that are older than 5 minutes, checking every 60 seconds
    tokio::spawn(async move {
        let prune_age = std::time::Duration::from_secs(300); // 5 minutes
        let check_interval = std::time::Duration::from_secs(60);
        loop {
            tokio::time::sleep(check_interval).await;
            pruner_tm.prune_old_tasks(prune_age).await;
        }
    });

    // Build our application with routes
    let app = Router::new()
        .without_v07_checks()
        .route("/", get(root))
        .merge(api::router())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let address = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    info!("Serving HTTP on {host}:{port}");
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

    // Use db module to scan directory
    db::scan_directory(pool, path).await?;

    Ok(())
}

async fn scan_single_file(pool: &Pool<Sqlite>, path_str: &str) -> Result<()> {
    use crate::db;
    use std::path::Path;

    let path = Path::new(path_str);
    if !path.exists() {
        anyhow::bail!("File does not exist: {}", path_str);
    }

    println!("Scanning single file: {}", path.display());
    println!();

    // Extract metadata without storing to database
    let file = db::extract_audio_metadata_from_file(path).await?;

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

    // Also store it to database for verification
    println!("Storing to database...");
    match db::scan_and_store_file(pool, path).await {
        Ok(stored) => {
            println!("Stored with id={}", stored.id);
        }
        Err(e) => {
            println!("Failed to store: {}", e);
        }
    }

    Ok(())
}

async fn db_status(pool: &Pool<Sqlite>) -> Result<()> {
    use sqlx::Row;

    // Check tables
    let tables = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .fetch_all(pool)
        .await?;

    println!("Database tables:");
    for row in tables {
        let name: String = row.get("name");
        println!("  - {}", name);
    }

    // Count records in main tables
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

async fn root() -> Html<&'static str> {
    Html(include_str!("../frontend/index.html"))
}
