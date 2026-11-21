//! CLI for the Tonies-Hub.
//!
//!   tonies-hub serve                          # web UI (default port 8090)
//!   tonies-hub add-user --slug <s> --password <p>

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sqlx::Row;
use tokio::net::TcpListener;

use tonies_hub::config::Config;
use tonies_hub::web::AppState;
use tonies_hub::{auth, db, settings, web};

#[derive(Parser)]
#[command(
    name = "tonies-hub",
    version = tonies_hub::VERSION,
    about = "Tonies-Hub — Eltern-UI für Kreativ-Tonies"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP UI.
    Serve {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
    },
    /// Create a user (or reset their password).
    AddUser {
        #[arg(long)]
        slug: String,
        /// Password; omit to read `TONIES_PASSWORD` from the environment.
        #[arg(long)]
        password: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { host, port } => serve(host, port).await,
        Command::AddUser { slug, password } => add_user(&slug, password).await,
    }
}

async fn serve(host: Option<String>, port: Option<u16>) -> Result<()> {
    let mut cfg = Config::from_env();
    if let Some(host) = host {
        cfg.host = host;
    }
    if let Some(port) = port {
        cfg.port = port;
    }

    let pool = db::connect(&cfg.database_url)
        .await
        .context("Tonies-DB konnte nicht geöffnet werden")?;
    settings::load_overlay(&pool, &mut cfg)
        .await
        .context("settings-Overlay fehlgeschlagen")?;

    let state = AppState {
        pool,
        config: std::sync::Arc::new(cfg.clone()),
    };
    let app = web::router(state);

    let listener = TcpListener::bind((cfg.host.as_str(), cfg.port))
        .await
        .with_context(|| format!("Bind auf {}:{} fehlgeschlagen", cfg.host, cfg.port))?;
    let addr = listener.local_addr()?;
    tracing::info!("tonies-hub lauscht auf http://{addr}");

    axum::serve(listener, app)
        .await
        .context("HTTP-Server beendet")?;
    Ok(())
}

async fn add_user(slug: &str, password: Option<String>) -> Result<()> {
    let slug = slug.trim();
    if slug.is_empty() {
        bail!("--slug darf nicht leer sein");
    }
    let password = password
        .or_else(|| std::env::var("TONIES_PASSWORD").ok())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Passwort fehlt (--password oder TONIES_PASSWORD)"))?;

    let cfg = Config::from_env();
    let pool = db::connect(&cfg.database_url)
        .await
        .context("Tonies-DB konnte nicht geöffnet werden")?;
    let hash = auth::hash_password(&password)?;
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO users (slug, display_name, password_hash, is_admin, created_at)
         VALUES (?1, ?1, ?2, 0, ?3)
         ON CONFLICT(slug) DO UPDATE SET password_hash = excluded.password_hash",
    )
    .bind(slug)
    .bind(&hash)
    .bind(&now)
    .execute(&pool)
    .await?;

    let row = sqlx::query("SELECT id FROM users WHERE slug = ?1")
        .bind(slug)
        .fetch_one(&pool)
        .await?;
    let id: i64 = row.try_get("id")?;
    tracing::info!("Benutzer '{slug}' (id {id}) angelegt/aktualisiert");
    println!("Benutzer '{slug}' (id {id}) bereit.");
    Ok(())
}
