//! MMM Hub — CLI for the multi-user Spotify ingest + exploration service.
//!
//!   mmm-hub serve                  # web UI + JSON API + SQL console
//!   mmm-hub auth   --user <slug>   # one-time loopback Spotify link
//!   mmm-hub ingest --user <slug>   # pull likes + playlists right now
//!   mmm-hub backfill               # queue a full initial load (worker does it)
//!   mmm-hub query  "<sql>"         # run a read-only query from the terminal
//!   mmm-hub users                  # list hub users

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use mmm_hub::config::Config;
use mmm_hub::{
    analyze, analyzer, api, audio, db, features, freqblog, genres, ingest, pages, settings, spotify,
    tags, web, worker,
};

#[derive(Parser)]
#[command(
    name = "mmm-hub",
    version,
    about = "MMM Hub — multi-user music ingest + exploration"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP read surface + SQL console.
    Serve {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
    },
    /// One-time Spotify authorization via a loopback redirect.
    Auth {
        #[arg(long)]
        user: String,
        #[arg(long, default_value_t = 8888)]
        port: u16,
    },
    /// Ingest a user's Spotify likes + owned/collaborative playlists.
    Ingest {
        #[arg(long)]
        user: String,
    },
    /// Refresh playlist metadata (owned + followed) for a user.
    FetchPlaylists {
        #[arg(long)]
        user: String,
    },
    /// Set / reset a user's password.
    SetPassword {
        #[arg(long)]
        user: String,
        #[arg(long)]
        password: String,
    },
    /// Queue a full initial load: enable all playlists + (re)fetch likes for all users.
    Backfill,
    /// Backfill audio features (BPM/key/energy) via ReccoBeats.
    Features {
        #[arg(long, default_value_t = 2000)]
        limit: usize,
    },
    /// Backfill missing audio features via FreqBlog (respects the monthly budget).
    Freqblog {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Compute EffNet embeddings + local BPM/key for tracks that lack them.
    Analyze {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Rebuild the tag layer (playlists resolve to tags).
    ResolveTags,
    /// Backfill track genres via Last.fm (needs LASTFM_API_KEY).
    Genres {
        #[arg(long, default_value_t = 2000)]
        limit: usize,
    },
    /// Grant admin to a user.
    MakeAdmin {
        #[arg(long)]
        user: String,
    },
    /// Run a read-only SQL query against the hub DB.
    Query { sql: String },
    /// List hub users.
    Users,
    /// Insert deterministic demo data (no Spotify needed).
    SeedDemo,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { host, port } => cmd_serve(cfg, host, port).await,
        Command::Auth { user, port } => cmd_auth(cfg, &user, port).await,
        Command::Ingest { user } => cmd_ingest(cfg, &user).await,
        Command::FetchPlaylists { user } => cmd_fetch_playlists(cfg, &user).await,
        Command::SetPassword { user, password } => cmd_set_password(cfg, &user, &password).await,
        Command::Backfill => cmd_backfill(cfg).await,
        Command::Features { limit } => cmd_features(cfg, limit).await,
        Command::Freqblog { limit } => cmd_freqblog(cfg, limit).await,
        Command::Analyze { limit } => cmd_analyze(cfg, limit).await,
        Command::ResolveTags => cmd_resolve_tags(cfg).await,
        Command::Genres { limit } => cmd_genres(cfg, limit).await,
        Command::MakeAdmin { user } => cmd_make_admin(cfg, &user).await,
        Command::Query { sql } => cmd_query(cfg, &sql).await,
        Command::Users => cmd_users(cfg).await,
        Command::SeedDemo => cmd_seed_demo(cfg).await,
    }
}

async fn cmd_serve(cfg: Config, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or(cfg.host.clone());
    let port = port.unwrap_or(cfg.port);

    let pool = db::connect(&cfg.database_url).await?;
    let ro_pool = db::connect_readonly(&cfg.database_url).await?;
    let cfg = settings::overlay_config(&pool, cfg).await;
    let state = api::AppState {
        pool,
        ro_pool,
        cfg: Arc::new(cfg.clone()),
        oauth_states: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = api::router(state.clone())
        .merge(web::router(state.clone()))
        .merge(pages::router(state.clone()));
    worker::spawn(state.pool.clone(), state.cfg.clone());

    let listener = TcpListener::bind((host.as_str(), port)).await?;
    let addr = listener.local_addr()?;
    tracing::info!("MMM Hub listening on http://{addr}");
    println!("MMM Hub auf http://{addr}");
    println!("  Web-UI:              GET  /");
    println!("  Spotify verbinden:   GET  /api/hub/services/spotify/connect");
    println!("  GET  /api/hub/health");
    println!("  GET  /api/hub/users");
    println!("  GET  /api/hub/playlists");
    println!("  GET  /api/hub/overlap");
    println!("  GET  /api/hub/tracks/{{id}}");
    println!("  POST /api/hub/query   {{\"sql\":\"SELECT ...\"}}");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn cmd_auth(cfg: Config, slug: &str, port: u16) -> Result<()> {
    let (client_id, client_secret) = cfg.spotify_creds()?;
    let pool = db::connect(&cfg.database_url).await?;
    let user_id = ingest::ensure_user(&pool, slug).await?;

    let (verifier, challenge) = spotify::pkce_pair();
    let state = spotify::random_state();
    let url = spotify::authorize_url(client_id, &cfg.spotify_redirect_uri, &state, &challenge);

    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("bind loopback 127.0.0.1:{port} for the OAuth callback"))?;

    println!("\nÖffne diese URL im Browser und bestätige den Zugriff:\n\n{url}\n");
    println!("Warte auf den Redirect auf {} …", cfg.spotify_redirect_uri);

    let (code, returned_state) = wait_for_callback(listener).await?;
    if returned_state != state {
        bail!("OAuth state mismatch — aborting (possible CSRF)");
    }

    let tokens = spotify::exchange_code(
        client_id,
        client_secret,
        &cfg.spotify_redirect_uri,
        &code,
        &verifier,
    )
    .await?;

    let (status, me) = spotify::api_get(&cfg.spotify_api_base, &tokens.access_token, "/me").await?;
    let (remote_id, display_name) = if status == 200 {
        (
            me["id"].as_str().map(str::to_string),
            me["display_name"].as_str().map(str::to_string),
        )
    } else {
        (None, None)
    };

    ingest::store_initial_tokens(
        &pool,
        user_id,
        &tokens,
        remote_id.as_deref(),
        display_name.as_deref(),
    )
    .await?;
    println!(
        "\n✓ Spotify verknüpft für '{slug}'{}.",
        display_name.map(|d| format!(" ({d})")).unwrap_or_default()
    );
    println!("  Jetzt: mmm-hub ingest --user {slug}");
    Ok(())
}

/// Accept a single OAuth redirect on the loopback listener and extract `code`.
async fn wait_for_callback(listener: TcpListener) -> Result<(String, String)> {
    let (mut socket, _) = listener.accept().await?;
    let mut buf = vec![0u8; 8192];
    let n = socket.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let request_line = request.lines().next().unwrap_or("");
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = urlencoding::decode(v)
            .map(|c| c.into_owned())
            .unwrap_or_default();
        match k {
            "code" => code = Some(v),
            "state" => state = Some(v),
            "error" => error = Some(v),
            _ => {}
        }
    }

    let body = match &error {
        Some(e) => format!("<html><body><h1>MMM Hub</h1><p>Spotify-Fehler: {e}</p></body></html>"),
        None => "<html><body><h1>MMM Hub</h1><p>Fertig — Fenster kann geschlossen werden.</p></body></html>".to_string(),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if let Some(e) = error {
        bail!("Spotify returned error: {e}");
    }
    match (code, state) {
        (Some(c), Some(s)) => Ok((c, s)),
        _ => bail!("callback did not contain code/state (target: {target})"),
    }
}

async fn cmd_ingest(cfg: Config, slug: &str) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    println!("Ingest für '{slug}' …");
    let summary = ingest::ingest_user(&pool, &cfg, slug).await?;
    println!("\n✓ Fertig:");
    println!("  Playlists gesamt:        {}", summary.playlists);
    println!("  davon gefolgt (Metadaten): {}", summary.followed);
    println!("  davon mit Items:         {}", summary.owned_with_items);
    println!("  Liked-Tracks:            {}", summary.liked_tracks);
    println!("  Playlist-Memberships:    {}", summary.memberships);
    Ok(())
}

async fn cmd_make_admin(cfg: Config, slug: &str) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let res = sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE slug = ?1 COLLATE NOCASE")
        .bind(slug)
        .execute(&pool)
        .await?;
    if res.rows_affected() == 0 {
        bail!("user '{slug}' not found");
    }
    println!("✓ '{slug}' ist jetzt Admin.");
    Ok(())
}

async fn cmd_features(cfg: Config, limit: usize) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let cfg = settings::overlay_config(&pool, cfg).await;
    println!("Hole Audio-Features (ReccoBeats) … max {limit}");
    let (processed, exhausted) = features::backfill(&pool, &cfg, limit).await?;
    println!(
        "✓ {processed} Tracks abgeglichen{}",
        if exhausted { " (alle erledigt)" } else { "" }
    );
    Ok(())
}

async fn cmd_freqblog(cfg: Config, limit: usize) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let cfg = settings::overlay_config(&pool, cfg).await;
    if !freqblog::enabled(&cfg) {
        bail!("FREQBlog_API_KEY nicht gesetzt (env oder /admin).");
    }
    let remaining = freqblog::remaining(&pool, &cfg).await;
    println!(
        "FreqBlog: {remaining} von {} Requests im Monat {} übrig",
        cfg.freqblog_monthly_cap,
        freqblog::period_utc()
    );
    let (processed, exhausted) = freqblog::backfill(&pool, &cfg, limit).await?;
    println!(
        "✓ {processed} Tracks verarbeitet{}",
        if exhausted { " (Budget erreicht oder nichts mehr offen)" } else { "" }
    );
    Ok(())
}

async fn cmd_analyze(cfg: Config, limit: usize) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let cfg = settings::overlay_config(&pool, cfg).await;
    let (emb_pending, an_pending) = analyze::pending_counts(&pool).await;
    println!(
        "Analyse: {emb_pending} ohne Embedding, {an_pending} ohne BPM/Key"
    );
    let inproc = cfg.effnet_inprocess && audio::available() && cfg.effnet_model.is_some();
    if inproc {
        println!("Embeddings: in-process (Rust/ort).");
    } else if analyzer::enabled(&cfg) {
        println!("Embeddings + BPM/Key: Analyzer-Service (Essentia).");
    } else {
        println!("! Weder EFFNET_INPROCESS noch HUB_ANALYZER_URL — Analyse uebersprungen.");
    }

    let ids = sqlx::query_scalar::<_, i64>(
        "SELECT t.id FROM hub_tracks t
           LEFT JOIN hub_track_embeddings e ON e.track_id = t.id
           LEFT JOIN hub_track_analysis a ON a.track_id = t.id
          WHERE t.isrc IS NOT NULL AND trim(t.isrc) <> ''
            AND (e.track_id IS NULL OR a.track_id IS NULL)
          ORDER BY t.id LIMIT ?1",
    )
    .bind(limit as i64)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let (mut ok, mut failed) = (0usize, 0usize);
    for id in ids {
        match analyze::analyze_track(&pool, &cfg, id).await {
            Ok(_) => ok += 1,
            Err(e) => {
                failed += 1;
                if failed <= 5 {
                    println!("  ! track {id}: {e}");
                }
            }
        }
    }
    println!("✓ {ok} analysiert, {failed} fehlgeschlagen/uebersprungen");
    Ok(())
}

async fn cmd_resolve_tags(cfg: Config) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let s = tags::rebuild(&pool).await?;
    println!(
        "✓ Tag-Layer neu berechnet: {} Tags, {} Quellen, {} Track-Tag-Zuordnungen",
        s.tags, s.sources, s.resolved
    );
    Ok(())
}

async fn cmd_genres(cfg: Config, limit: usize) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let cfg = settings::overlay_config(&pool, cfg).await;
    if cfg.lastfm_api_key.is_none() {
        println!("! LASTFM_API_KEY nicht gesetzt — Genres übersprungen.");
        return Ok(());
    }
    let (processed, exhausted) = genres::backfill(&pool, &cfg, limit).await?;
    println!(
        "✓ {processed} Tracks mit Genres{}",
        if exhausted { " (alle erledigt)" } else { "" }
    );
    Ok(())
}

async fn cmd_backfill(cfg: Config) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let playlists = sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 1, fetch_status = 'queued', items_available = 0, fetch_error = NULL",
    )
    .execute(&pool)
    .await?;
    let accounts = sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_status = 'queued', likes_synced_at = NULL, likes_error = NULL
          WHERE service = 'spotify' AND access_token IS NOT NULL",
    )
    .execute(&pool)
    .await?;
    println!(
        "✓ Backfill eingereiht: {} Playlists aktiviert, {} Accounts fuer Likes markiert.",
        playlists.rows_affected(),
        accounts.rows_affected()
    );
    println!("  Der Worker arbeitet im Hintergrund (429-schonend) — Fortschritt auf der Webseite.");
    Ok(())
}

async fn cmd_set_password(cfg: Config, slug: &str, password: &str) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let hash = bcrypt::hash(password, 10)?;
    let res = sqlx::query("UPDATE hub_users SET password_hash = ?2 WHERE slug = ?1 COLLATE NOCASE")
        .bind(slug)
        .bind(&hash)
        .execute(&pool)
        .await?;
    if res.rows_affected() == 0 {
        bail!("user '{slug}' not found");
    }
    println!("✓ Passwort für '{slug}' gesetzt.");
    Ok(())
}

async fn cmd_fetch_playlists(cfg: Config, slug: &str) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    let s = ingest::fetch_playlists(&pool, &cfg, slug).await?;
    println!(
        "✓ {} Playlists ({} eigene, {} gefolgt)",
        s.total, s.owned, s.followed
    );
    Ok(())
}

async fn cmd_query(cfg: Config, sql: &str) -> Result<()> {
    // Ensure the DB + migrations exist, then open the read-only pool.
    let _ = db::connect(&cfg.database_url).await?;
    let ro = db::connect_readonly(&cfg.database_url).await?;
    let (columns, rows) = api::run_readonly_query(&ro, sql).await?;
    print_table(&columns, &rows);
    Ok(())
}

fn print_table(columns: &[String], rows: &[serde_json::Value]) {
    println!("{}", columns.join(" | "));
    for row in rows {
        let line = columns
            .iter()
            .map(|c| match row.get(c) {
                Some(serde_json::Value::Null) | None => String::new(),
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" | ");
        println!("{line}");
    }
}

async fn cmd_seed_demo(cfg: Config) -> Result<()> {
    let pool = db::connect(&cfg.database_url).await?;
    ingest::seed_demo(&pool).await?;
    println!("✓ Demo-Daten eingefügt (momo, simon, jonas).");
    println!("  Beispiel: mmm-hub query \"SELECT title, artists, user_ids FROM hub_v_shared_tracks\"");
    Ok(())
}

async fn cmd_users(cfg: Config) -> Result<()> {
    let _ = db::connect(&cfg.database_url).await?;
    let ro = db::connect_readonly(&cfg.database_url).await?;
    let (columns, rows) = api::run_readonly_query(
        &ro,
        "SELECT id, slug, display_name FROM hub_users ORDER BY id",
    )
    .await?;
    print_table(&columns, &rows);
    Ok(())
}
