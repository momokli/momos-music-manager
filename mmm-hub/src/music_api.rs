//! Adapter for the `music-api` service (ISRC-keyed order/consume, on .200:8710).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::Config;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct IsrcState {
    pub isrc: Option<String>,
    pub state: Option<String>,
    pub deezer_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub formats: Option<Vec<String>>,
    pub error: Option<String>,
}

impl IsrcState {
    /// In the music-api ledger and not an "unknown ISRC" error.
    pub fn known(&self) -> bool {
        self.error.is_none() && self.state.is_some()
    }
    pub fn ready(&self) -> bool {
        self.state.as_deref() == Some("ready")
    }
    pub fn formats_str(&self) -> String {
        self.formats
            .as_ref()
            .map(|f| f.join(", "))
            .unwrap_or_default()
    }
}

fn token(cfg: &Config) -> Result<&str> {
    cfg.music_api_token
        .as_deref()
        .context("MUSIC_API_TOKEN is not set")
}

/// `GET /isrc/{isrc}` — current state of one ISRC.
pub async fn status(cfg: &Config, isrc: &str) -> Result<IsrcState> {
    let url = format!("{}/isrc/{}", cfg.music_api_base, urlencoding::encode(isrc));
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg)?)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let body: IsrcState = resp.json().await.unwrap_or_default();
    Ok(body)
}

/// `GET /isrc/{isrc}/{format}` — fetch the actual audio bytes (e.g. `flac`).
/// Errors if the service returns a non-success status (not yet downloaded etc.).
pub async fn file_bytes(cfg: &Config, isrc: &str, format: &str) -> Result<Vec<u8>> {
    let url = format!(
        "{}/isrc/{}/{}",
        cfg.music_api_base,
        urlencoding::encode(isrc),
        urlencoding::encode(format)
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg)?)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("music-api {url} -> {status}");
    }
    Ok(resp.bytes().await.context("read music-api body")?.to_vec())
}

/// `POST /orders` — order one or more ISRCs. Returns the order id.
pub async fn order(cfg: &Config, isrcs: &[String]) -> Result<String> {
    let url = format!("{}/orders", cfg.music_api_base);
    let items: Vec<_> = isrcs.iter().map(|i| json!({ "isrc": i })).collect();
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .json(&json!({ "items": items }))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let v: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok(v["orderId"].as_str().unwrap_or_default().to_string())
}

// ── state cache + bulk helpers (progress / order-only-missing) ───────────────

async fn store_state(pool: &SqlitePool, isrc: &str, state: &str) {
    let _ = sqlx::query(
        "INSERT INTO hub_music_state (isrc, state, checked_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(isrc) DO UPDATE SET state = excluded.state, checked_at = excluded.checked_at",
    )
    .bind(isrc)
    .bind(state)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await;
}

/// Refresh the cached music-api state for up to `cap` ISRCs (bounded concurrency).
/// Returns `(ready, total, refreshed)`.
pub async fn refresh_states(
    pool: &SqlitePool,
    cfg: &Config,
    isrcs: &[String],
    cap: usize,
) -> (usize, usize, usize) {
    if cfg.music_api_token.is_none() {
        return (0, 0, 0);
    }
    let list: Vec<String> = isrcs.iter().take(cap).cloned().collect();
    let total = list.len();
    let mut ready = 0usize;
    let mut refreshed = 0usize;
    for chunk in list.chunks(8) {
        let mut set = tokio::task::JoinSet::new();
        for isrc in chunk {
            let cfg = cfg.clone();
            let isrc = isrc.clone();
            set.spawn(async move {
                let state = status(&cfg, &isrc)
                    .await
                    .ok()
                    .and_then(|s| s.state)
                    .unwrap_or_default();
                (isrc, state)
            });
        }
        while let Some(Ok((isrc, state))) = set.join_next().await {
            if !state.is_empty() {
                store_state(pool, &isrc, &state).await;
                refreshed += 1;
                if state == "ready" {
                    ready += 1;
                }
            }
        }
    }
    (ready, total, refreshed)
}

/// ISRCs whose cached state is not `ready` (missing from cache = not ready).
pub async fn missing_isrcs(pool: &SqlitePool, isrcs: &[String]) -> Vec<String> {
    let mut ready: std::collections::HashSet<String> = std::collections::HashSet::new();
    for chunk in isrcs.chunks(900) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT isrc FROM hub_music_state WHERE state = 'ready' AND isrc IN (",
        );
        let mut sep = qb.separated(", ");
        for i in chunk {
            sep.push_bind(i.clone());
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            use sqlx::Row;
            ready.insert(r.get::<Option<String>, _>("isrc").unwrap_or_default());
        }
    }
    isrcs
        .iter()
        .filter(|i| !ready.contains(*i))
        .cloned()
        .collect()
}

/// `(ready, total)` from the **cached** state (no network) — for cheap progress
/// indicators that are polled by the UI.
pub async fn cached_counts(pool: &SqlitePool, isrcs: &[String]) -> (usize, usize) {
    let mut ready = 0usize;
    let mut seen = 0usize;
    for chunk in isrcs.chunks(900) {
        let mut qb =
            sqlx::QueryBuilder::new("SELECT isrc, state FROM hub_music_state WHERE isrc IN (");
        let mut sep = qb.separated(", ");
        for i in chunk {
            sep.push_bind(i.clone());
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            use sqlx::Row;
            seen += 1;
            if r.get::<Option<String>, _>("state").as_deref() == Some("ready") {
                ready += 1;
            }
        }
    }
    (ready, seen)
}
