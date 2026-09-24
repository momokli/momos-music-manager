//! `music-api` consumer — orders missing ISRCs, polls open orders, imports
//! delivered files and triggers a scan of the destination folders.
//!
//! The demand is the **whole library** (every track without a linked file), but
//! Backpack tracks are ordered first so they never wait behind the backlog
//!
//! Replaces the old "submit a Spotify playlist URL at deemix" transport. The
//! per-ISRC state lives in `music_api_imports` (see [`crate::db::music_api`]).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{Pool, Sqlite};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::ServiceCredentials;
use crate::db;
use crate::music_api::MusicApiClient;
use crate::tasks::TaskManager;

/// Default FLAC destination when `MUSIC_API_FLAC_DIR` is unset.
const DEFAULT_FLAC_DIR: &str = "/Users/momo/Music/flacs";
/// Default MP3 destination when `MUSIC_API_MP3_DIR` is unset.
const DEFAULT_MP3_DIR: &str = "/Users/momo/Music/mp3";

/// Run the consumer loop until cancelled.
///
/// Logs once at `warn` and returns when `music_api` is not configured.
pub async fn start_music_api_consumer(
    db: Pool<Sqlite>,
    creds: ServiceCredentials,
    task_manager: TaskManager,
    cancel: CancellationToken,
) {
    if !creds.music_api.is_configured() {
        warn!(
            "music-api consumer not started: not configured \
             (need MUSIC_API_URL + MUSIC_API_TOKEN, enabled={})",
            creds.music_api.enabled
        );
        return;
    }

    let interval = creds.music_api.interval_secs.max(1);
    info!("music-api consumer started (interval: {interval}s)");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("music-api consumer shutting down");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {
                if let Err(e) = run_once(&db, &creds, &task_manager).await {
                    warn!("music-api consumer cycle failed: {e:#}");
                }
            }
        }
    }
}

/// Run a single consumer cycle now (used by the periodic loop).
pub async fn run_once(
    db: &Pool<Sqlite>,
    creds: &ServiceCredentials,
    task_manager: &TaskManager,
) -> Result<()> {
    if !creds.music_api.is_configured() {
        anyhow::bail!("music-api is not configured");
    }
    let base_url = creds
        .music_api
        .base_url
        .as_deref()
        .context("music-api base_url missing")?;
    let token = creds
        .music_api
        .token
        .as_deref()
        .context("music-api token missing")?;

    let client = MusicApiClient::new(base_url, token);
    run_cycle(db, &creds.music_api, &client, task_manager).await
}

/// One full cycle: order the demand, then consume every open order.
async fn run_cycle(
    db: &Pool<Sqlite>,
    config: &crate::music_api::MusicApiConfig,
    client: &MusicApiClient,
    task_manager: &TaskManager,
) -> Result<()> {
    // (b) Order the demand.
    let batch_size = config.batch_size.max(1);
    let demand = db::music_api::demand_isrcs(db, batch_size)
        .await
        .context("music-api: reading demand")?;
    if !demand.is_empty() {
        match client.create_order(&demand).await {
            Ok(order_id) => {
                for isrc in &demand {
                    if let Err(e) = db::music_api::upsert_ordered(db, isrc, &order_id).await {
                        warn!("music-api: failed to record order for {isrc}: {e}");
                    }
                }
                info!(
                    "music-api: ordered {} ISRC(s) (order {order_id})",
                    demand.len()
                );
            }
            Err(e) => warn!("music-api: create_order failed: {e:#}"),
        }
    }

    // (c) Consume open orders.
    let orders = client
        .list_orders(Some("open"))
        .await
        .context("music-api: listing open orders")?;

    let mut written_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut written = 0usize;

    for order in orders {
        let status = match client.get_order(&order.id).await {
            Ok(s) => s,
            Err(e) => {
                warn!("music-api: failed to read order {}: {e:#}", order.id);
                continue;
            }
        };

        for item in status.items {
            match item.state.as_str() {
                "ready" => match import_ready_item(db, client, &item, &mut written_dirs).await {
                    Ok(true) => written += 1,
                    Ok(false) => {}
                    Err(e) => warn!("music-api: failed to import ISRC {}: {e:#}", item.isrc),
                },
                "absent" | "failed" => {
                    // Terminal — never re-ordered.
                    if let Err(e) = db::music_api::set_state(
                        db,
                        &item.isrc,
                        &item.state,
                        None,
                        None,
                        item.error.as_deref(),
                    )
                    .await
                    {
                        warn!("music-api: failed to record {} state: {e}", item.isrc);
                    } else {
                        info!(
                            "music-api: ISRC {} is '{}' (terminal){}",
                            item.isrc,
                            item.state,
                            item.error
                                .as_deref()
                                .map(|e| format!(": {e}"))
                                .unwrap_or_default(),
                        );
                    }
                }
                // pending / downloading — not ready yet.
                _ => {}
            }
        }
    }

    // (d) Trigger a scan for every folder a file was written into.
    if written > 0 {
        for dir in &written_dirs {
            trigger_folder_scan(db, task_manager, dir).await;
        }
        info!(
            "music-api: imported {written} file(s) into {} folder(s)",
            written_dirs.len()
        );
    }

    Ok(())
}

/// Download one `ready` item, write it to the format's destination and mark it
/// `imported`. Returns `Ok(true)` when a file was written.
///
/// An item without artist/title (or without a usable format) is left `ready` for
/// the next cycle and returns `Ok(false)`.
async fn import_ready_item(
    db: &Pool<Sqlite>,
    client: &MusicApiClient,
    item: &crate::music_api::IsrcStatus,
    written_dirs: &mut BTreeSet<PathBuf>,
) -> Result<bool> {
    let artist = item.artist.as_deref().unwrap_or_default().trim();
    let title = item.title.as_deref().unwrap_or_default().trim();
    if artist.is_empty() || title.is_empty() {
        warn!(
            "music-api: ISRC {} is ready but artist/title unknown — deferring",
            item.isrc
        );
        return Ok(false);
    }

    let (format, dir, ext) = match choose_format(&item.formats) {
        Some(choice) => choice,
        None => {
            warn!(
                "music-api: ISRC {} is ready but has no downloadable format ({:?})",
                item.isrc, item.formats
            );
            return Ok(false);
        }
    };

    let bytes = client.download_isrc(&item.isrc, format).await?;
    let filename = format!(
        "{} - {}.{}",
        sanitize_component(artist),
        sanitize_component(title),
        ext
    );
    let dir_path = PathBuf::from(dir);
    let path = dir_path.join(&filename);

    tokio::fs::create_dir_all(&dir_path)
        .await
        .with_context(|| format!("creating destination dir {}", dir_path.display()))?;
    tokio::fs::write(&path, &bytes)
        .await
        .with_context(|| format!("writing {}", path.display()))?;

    db::music_api::set_state(
        db,
        &item.isrc,
        "imported",
        Some(format),
        Some(&path.to_string_lossy()),
        None,
    )
    .await
    .context("recording imported state")?;

    written_dirs.insert(dir_path);
    info!(
        "music-api: imported {} ({}) -> {}",
        item.isrc,
        format,
        path.display()
    );
    Ok(true)
}

/// Pick the best available format and its destination directory.
///
/// `flac` > `320` > `128`. Returns `(service format, destination dir, file
/// extension)`.
fn choose_format(formats: &[String]) -> Option<(&'static str, String, &'static str)> {
    let has = |f: &str| formats.iter().any(|v| v == f);
    if has("flac") {
        Some(("flac", flac_dir(), "flac"))
    } else if has("320") {
        Some(("320", mp3_dir(), "mp3"))
    } else if has("128") {
        Some(("128", mp3_dir(), "mp3"))
    } else {
        None
    }
}

/// FLAC destination directory (`MUSIC_API_FLAC_DIR` or the default).
fn flac_dir() -> String {
    std::env::var("MUSIC_API_FLAC_DIR").unwrap_or_else(|_| DEFAULT_FLAC_DIR.to_string())
}

/// MP3 destination directory (`MUSIC_API_MP3_DIR` or the default).
fn mp3_dir() -> String {
    std::env::var("MUSIC_API_MP3_DIR").unwrap_or_else(|_| DEFAULT_MP3_DIR.to_string())
}

/// Strip filesystem-hostile characters (`/`, NUL, control chars) from a
/// filename component and trim surrounding whitespace.
fn sanitize_component(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '/' && *c != '\0' && !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// Scan the folder whose `folder_path` matches the destination dir; silently
/// skip when the user has no such configured folder.
async fn trigger_folder_scan(db: &Pool<Sqlite>, task_manager: &TaskManager, dir: &PathBuf) {
    let path = dir.to_string_lossy().to_string();
    let folder_id: Option<i64> = sqlx::query_scalar("SELECT id FROM folders WHERE folder_path = ?")
        .bind(&path)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();

    match folder_id {
        Some(folder_id) => {
            if let Err(e) = crate::tasks::start_scan_folder_task(
                task_manager,
                db,
                folder_id,
                db::ScanMode::Incremental { since: None },
            )
            .await
            {
                warn!("music-api: failed to scan folder {path}: {e:#}");
            } else {
                info!("music-api: triggered scan of {path}");
            }
        }
        None => {
            tracing::debug!("music-api: no configured folder for {path}; skipping scan");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_format_prefers_flac_then_320_then_128() {
        assert_eq!(
            choose_format(&["flac".into(), "320".into()]).unwrap().0,
            "flac"
        );
        assert_eq!(
            choose_format(&["320".into(), "128".into()]).unwrap().0,
            "320"
        );
        assert_eq!(choose_format(&["128".into()]).unwrap().0, "128");
        assert!(choose_format(&[]).is_none());
        assert!(choose_format(&["wav".into()]).is_none());
    }

    #[test]
    fn choose_format_extension_mp3_for_lossy() {
        assert_eq!(choose_format(&["320".into()]).unwrap().2, "mp3");
        assert_eq!(choose_format(&["128".into()]).unwrap().2, "mp3");
        assert_eq!(choose_format(&["flac".into()]).unwrap().2, "flac");
    }

    #[test]
    fn sanitize_component_strips_slashes_nul_and_control() {
        assert_eq!(sanitize_component("AC/DC"), "ACDC");
        assert_eq!(sanitize_component("a\0b"), "ab");
        assert_eq!(sanitize_component("  spaced  "), "spaced");
        assert_eq!(sanitize_component("normal name"), "normal name");
    }
}
