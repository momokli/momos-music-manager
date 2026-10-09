//! Backend worker: the only place that talks to Spotify for sync.
//!
//! Its jobs, driven purely by flags in the DB (the frontend only toggles those):
//!   * liked tracks per connected account   (`hub_service_accounts.likes_status`)
//!   * playlist items for `enabled_for_fetch` playlists (`items_available = 0`)
//!
//! It runs continuously while there is work, one request stream at a time, with a
//! small page delay and hard backoff on 429 / dev-mode quota.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::ingest;
use crate::spotify;

const IDLE_WAIT: Duration = Duration::from_secs(10);
const BUSY_WAIT: Duration = Duration::from_secs(1);
const MAX_PLAYLISTS_PER_PASS: usize = 12;
const MAX_LIKES_ACCOUNTS_PER_PASS: usize = 2;
const PAGE_DELAY: Duration = Duration::from_millis(150);

pub fn spawn(pool: SqlitePool, cfg: Arc<Config>) {
    tokio::spawn(async move {
        loop {
            let worked = match run_once(&pool, &cfg).await {
                Ok(worked) => worked,
                Err(e) => {
                    tracing::warn!("worker pass failed: {e}");
                    false
                }
            };
            tokio::time::sleep(if worked { BUSY_WAIT } else { IDLE_WAIT }).await;
        }
    });
}

async fn run_once(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    // Likes first (cheap, one endpoint), then playlist items, then features.
    let likes = sync_likes_jobs(pool, cfg).await?;
    let playlists = playlist_jobs(pool, cfg).await?;
    let features = crate::features::sync_once(pool, cfg, 40).await.unwrap_or(0) > 0;
    Ok(likes || playlists || features)
}

// ── liked tracks ────────────────────────────────────────────────────────────

async fn sync_likes_jobs(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    let accounts = sqlx::query_as::<_, (i64, String)>(
        "SELECT a.user_id, u.slug
           FROM hub_service_accounts a
           JOIN hub_users u ON u.id = a.user_id
          WHERE a.service = 'spotify'
            AND a.access_token IS NOT NULL
            AND (a.likes_status IS NULL OR a.likes_status = 'queued')
          ORDER BY a.user_id
          LIMIT ?1",
    )
    .bind(MAX_LIKES_ACCOUNTS_PER_PASS as i64)
    .fetch_all(pool)
    .await?;

    let mut worked = false;
    for (user_id, slug) in accounts {
        worked = true;
        let token = match ingest::access_token(pool, cfg, user_id).await {
            Ok(t) => t,
            Err(e) => {
                set_likes_error(pool, user_id, &e.to_string()).await?;
                continue;
            }
        };
        match paged(&token, &spotify::api_url(&cfg.spotify_api_base, "/me/tracks?limit=50")).await? {
            Outcome::Ok(items) => {
                let n = ingest::store_likes(pool, user_id, &items).await?;
                sqlx::query(
                    "UPDATE hub_service_accounts
                        SET likes_synced_at = ?2, likes_status = 'done', likes_error = NULL
                      WHERE user_id = ?1 AND service = 'spotify'",
                )
                .bind(user_id)
                .bind(now_iso())
                .execute(pool)
                .await?;
                tracing::info!("synced {n} liked tracks for {slug}");
            }
            Outcome::Forbidden => {
                set_likes_error(pool, user_id, "Spotify 403 — Account nicht in der App-Allowlist").await?;
            }
            Outcome::RateLimited { retry_after } => {
                backoff(retry_after).await;
                return Ok(worked);
            }
            Outcome::Quota => {
                quota_backoff().await;
                return Ok(worked);
            }
            Outcome::Error(m) => set_likes_error(pool, user_id, &m).await?,
        }
    }
    Ok(worked)
}

async fn set_likes_error(pool: &SqlitePool, user_id: i64, msg: &str) -> Result<()> {
    sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_status = 'error', likes_error = ?2
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .bind(msg)
    .execute(pool)
    .await?;
    tracing::warn!("likes sync for user {user_id} failed: {msg}");
    Ok(())
}

// ── playlist items ──────────────────────────────────────────────────────────

async fn playlist_jobs(pool: &SqlitePool, cfg: &Config) -> Result<bool> {
    let jobs = sqlx::query_as::<_, (i64, i64, String, String)>(
        "SELECT p.id, p.user_id, p.playlist_id, u.slug
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
          WHERE p.service = 'spotify'
            AND p.enabled_for_fetch = 1
            AND p.items_available = 0
          ORDER BY p.id
          LIMIT ?1",
    )
    .bind(MAX_PLAYLISTS_PER_PASS as i64)
    .fetch_all(pool)
    .await?;

    let mut tokens: HashMap<i64, String> = HashMap::new();
    let mut worked = false;

    for (local_id, user_id, playlist_id, slug) in jobs {
        worked = true;
        let token = if let Some(t) = tokens.get(&user_id) {
            t.clone()
        } else {
            match ingest::access_token(pool, cfg, user_id).await {
                Ok(t) => {
                    tokens.insert(user_id, t.clone());
                    t
                }
                Err(e) => {
                    fail(pool, local_id, &format!("auth: {e}")).await?;
                    continue;
                }
            }
        };

        let url = spotify::api_url(&cfg.spotify_api_base, &format!("/playlists/{playlist_id}/items?limit=50"));
        match paged(&token, &url).await? {
            Outcome::Ok(items) => {
                let n = ingest::store_playlist_items(pool, local_id, &items).await?;
                sqlx::query(
                    "UPDATE hub_playlists
                        SET items_available = 1, fetched_at = ?2,
                            fetch_status = 'done', fetch_error = NULL
                      WHERE id = ?1",
                )
                .bind(local_id)
                .bind(now_iso())
                .execute(pool)
                .await?;
                tracing::info!("fetched {n} tracks for playlist {local_id} ({slug})");
            }
            Outcome::Forbidden => {
                fail(pool, local_id, "kein Zugriff (Spotify 403)").await?;
            }
            Outcome::RateLimited { retry_after } => {
                backoff(retry_after).await;
                return Ok(worked);
            }
            Outcome::Quota => {
                quota_backoff().await;
                return Ok(worked);
            }
            Outcome::Error(m) => fail(pool, local_id, &m).await?,
        }
    }
    Ok(worked)
}

async fn fail(pool: &SqlitePool, id: i64, msg: &str) -> Result<()> {
    sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 0, fetch_status = 'error', fetch_error = ?2
          WHERE id = ?1",
    )
    .bind(id)
    .bind(msg)
    .execute(pool)
    .await?;
    tracing::warn!("playlist {id} fetch failed: {msg}");
    Ok(())
}

// ── paging + backoff ────────────────────────────────────────────────────────

enum Outcome {
    Ok(Vec<Value>),
    Forbidden,
    RateLimited { retry_after: Option<u64> },
    Quota,
    Error(String),
}

/// Follow `next` links, mapping 429/403 to outcomes instead of erroring.
async fn paged(token: &str, first_url: &str) -> Result<Outcome> {
    let mut url = first_url.to_string();
    let mut items = Vec::new();
    loop {
        let page = spotify::get_page(token, &url).await?;
        match page.status {
            200 => {}
            429 if page.quota_exceeded => return Ok(Outcome::Quota),
            429 => {
                return Ok(Outcome::RateLimited {
                    retry_after: page.retry_after,
                });
            }
            403 => return Ok(Outcome::Forbidden),
            s => return Ok(Outcome::Error(format!("HTTP {s}"))),
        }
        if let Some(arr) = page.body["items"].as_array() {
            items.extend(arr.iter().cloned());
        }
        match page.body["next"].as_str() {
            Some(next) if !next.is_empty() => url = next.to_string(),
            _ => break,
        }
        tokio::time::sleep(PAGE_DELAY).await;
    }
    Ok(Outcome::Ok(items))
}

async fn backoff(retry_after: Option<u64>) {
    let secs = retry_after.unwrap_or(30).max(1);
    tracing::warn!("Spotify rate limit (429); backing off {secs}s");
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

async fn quota_backoff() {
    tracing::warn!("Spotify dev-mode quota exceeded; backing off 10m");
    tokio::time::sleep(Duration::from_secs(600)).await;
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}
