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
    /// Classified delivered format: `flac` | `mp3-320` | `mp3-128`.
    pub source_format: Option<String>,
    pub error: Option<String>,
}

#[allow(dead_code)]
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

/// A cached music-api state for one ISRC, read from `hub_music_state` (no
/// network). Used by the track page and the downloads overview.
#[derive(Debug, Clone, Default)]
pub struct CachedState {
    pub state: String,
    pub source_format: Option<String>,
    pub formats: Vec<String>,
    pub deezer_id: Option<String>,
    /// Error/reason reported by music-api (e.g. `no data`, `download timeout`).
    pub error: Option<String>,
}

impl CachedState {
    pub fn ready(&self) -> bool {
        self.state == "ready"
    }

    /// Human label like `ready · flac` / `pending` / `absent · no data` — the
    /// error reason is appended when present so the UI can show *why*.
    pub fn label(&self) -> String {
        let fmt = self
            .source_format
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.formats.first().cloned());
        let base = match fmt {
            Some(f) if !self.state.is_empty() => format!("{} · {}", self.state, f),
            Some(f) => f,
            None => self.state.clone(),
        };
        match self.error.clone().filter(|e| !e.is_empty()) {
            Some(e) if base.is_empty() => e,
            Some(e) => format!("{base} · {e}"),
            None => base,
        }
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

/// Map a hub-facing format token to the token the music-api service accepts
/// (`flac` | `320` | `128`). The hub UI offers `flac`/`mp3`/`wav`/`m4a`; only
/// flac + mp3 are actually delivered, so mp3* collapse to `320`.
pub fn api_format(format: &str) -> String {
    match format.to_lowercase().as_str() {
        "flac" => "flac".to_string(),
        "128" | "mp3-128" => "128".to_string(),
        "320" | "mp3" | "mp3-320" => "320".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod api_format_tests {
    use super::api_format;
    #[test]
    fn maps_hub_formats_to_music_api_tokens() {
        assert_eq!(api_format("flac"), "flac");
        assert_eq!(api_format("mp3"), "320");
        assert_eq!(api_format("mp3-320"), "320");
        assert_eq!(api_format("128"), "128");
        assert_eq!(api_format("mp3-128"), "128");
    }
}

/// `GET /isrc/{isrc}/{format}` — fetch the actual audio bytes (e.g. `flac`).
/// `format` is a hub token; it is translated via [`api_format`].
pub async fn file_bytes(cfg: &Config, isrc: &str, format: &str) -> Result<Vec<u8>> {
    let api = api_format(format);
    let url = format!(
        "{}/isrc/{}/{}",
        cfg.music_api_base,
        urlencoding::encode(isrc),
        urlencoding::encode(&api)
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

/// Fetch audio bytes, falling back across formats until one is available.
/// Tries `preferred` first, then 128 / 320 / flac. Use this when the caller
/// only cares about getting *some* playable audio (player, downloads).
pub async fn file_bytes_any(cfg: &Config, isrc: &str, preferred: &str) -> Result<Vec<u8>> {
    let mut tried: Vec<String> = Vec::new();
    let mut last_err: Option<anyhow::Error> = None;
    for f in [preferred, "128", "320", "flac"] {
        let api = api_format(f);
        if tried.iter().any(|t| t == &api) {
            continue;
        }
        tried.push(api.clone());
        match file_bytes(cfg, isrc, f).await {
            Ok(b) => return Ok(b),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no audio format available")))
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

/// `POST /orders` with an explicit `priority` (higher = earlier). Returns the
/// order id. Used by the playlist "Priorisieren" action.
pub async fn order_with_priority(cfg: &Config, isrcs: &[String], priority: i32) -> Result<String> {
    let url = format!("{}/orders", cfg.music_api_base);
    let items: Vec<_> = isrcs.iter().map(|i| json!({ "isrc": i })).collect();
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .json(&json!({ "items": items, "priority": priority }))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let v: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok(v["orderId"].as_str().unwrap_or_default().to_string())
}

// ── state cache + bulk helpers (progress / order-only-missing) ───────────────

async fn store_state(
    pool: &SqlitePool,
    isrc: &str,
    state: &str,
    source_format: Option<&str>,
    formats: Option<&str>,
    deezer_id: Option<&str>,
    error: Option<&str>,
) {
    let _ = sqlx::query(
        "INSERT INTO hub_music_state (isrc, state, source_format, formats, deezer_id, error, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(isrc) DO UPDATE SET
             state = excluded.state,
             source_format = excluded.source_format,
             formats = excluded.formats,
             deezer_id = COALESCE(excluded.deezer_id, hub_music_state.deezer_id),
             error = excluded.error,
             checked_at = excluded.checked_at",
    )
    .bind(isrc)
    .bind(state)
    .bind(source_format)
    .bind(formats)
    .bind(deezer_id)
    .bind(error)
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
                let st = status(&cfg, &isrc).await.ok();
                (isrc, st)
            });
        }
        while let Some(Ok((isrc, st))) = set.join_next().await {
            let Some(st) = st else { continue };
            let state = st.state.clone().filter(|s| !s.is_empty());
            let error = st.error.clone().filter(|s| !s.is_empty());
            // Keep rows that carry an error even without a state, so the reason
            // (e.g. "unknown ISRC") is still visible in the UI.
            if state.is_none() && error.is_none() {
                continue;
            }
            let state = state.unwrap_or_else(|| "unknown".to_string());
            let formats = st
                .formats
                .as_ref()
                .filter(|f| !f.is_empty())
                .map(|f| f.join(","));
            store_state(
                pool,
                &isrc,
                &state,
                st.source_format.as_deref(),
                formats.as_deref(),
                st.deezer_id.as_deref(),
                error.as_deref(),
            )
            .await;
            refreshed += 1;
            if state == "ready" {
                ready += 1;
            }
        }
    }
    (ready, total, refreshed)
}

/// Read the cached state (incl. format) for one ISRC — no network call.
pub async fn cached_state(pool: &SqlitePool, isrc: &str) -> Option<CachedState> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT state, source_format, formats, deezer_id, error FROM hub_music_state WHERE isrc = ?1",
    )
    .bind(isrc)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    Some(CachedState {
        state: row.get::<Option<String>, _>("state").unwrap_or_default(),
        source_format: row.get::<Option<String>, _>("source_format"),
        formats: row
            .get::<Option<String>, _>("formats")
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        deezer_id: row.get::<Option<String>, _>("deezer_id"),
        error: row.get::<Option<String>, _>("error"),
    })
}

/// Aggregate counts over the whole `hub_music_state` cache (downloads page).
#[derive(Debug, Clone, Default)]
pub struct CacheSummary {
    pub total: usize,
    pub ready: usize,
    pub ready_flac: usize,
    pub ready_320: usize,
    pub ready_128: usize,
    pub ready_other: usize,
    pub pending: usize,
    pub downloading: usize,
    pub absent: usize,
    pub failed: usize,
    /// Rows that carry an error/reason (regardless of state).
    pub errors: usize,
}

pub async fn cache_summary(pool: &SqlitePool) -> CacheSummary {
    use sqlx::Row;
    let mut s = CacheSummary::default();
    let rows = sqlx::query("SELECT state, source_format, formats, error FROM hub_music_state")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for r in rows {
        s.total += 1;
        if r.get::<Option<String>, _>("error")
            .filter(|e| !e.is_empty())
            .is_some()
        {
            s.errors += 1;
        }
        let state = r.get::<Option<String>, _>("state").unwrap_or_default();
        match state.as_str() {
            "ready" => {
                s.ready += 1;
                let sf = r.get::<Option<String>, _>("source_format");
                let formats = r.get::<Option<String>, _>("formats").unwrap_or_default();
                match format_bucket(sf.as_deref(), &formats) {
                    "flac" => s.ready_flac += 1,
                    "320" => s.ready_320 += 1,
                    "128" => s.ready_128 += 1,
                    _ => s.ready_other += 1,
                }
            }
            "pending" => s.pending += 1,
            "downloading" => s.downloading += 1,
            "absent" => s.absent += 1,
            "failed" => s.failed += 1,
            _ => {}
        }
    }
    s
}

/// Coarse format bucket for the ready distribution: `flac` | `320` | `128` | `other`.
pub fn format_bucket(source_format: Option<&str>, formats: &str) -> &'static str {
    let mut hay = String::new();
    if let Some(s) = source_format {
        hay.push_str(s);
        hay.push(' ');
    }
    hay.push_str(formats);
    let h = hay.to_lowercase();
    if h.contains("flac") {
        "flac"
    } else if h.contains("320") {
        "320"
    } else if h.contains("128") {
        "128"
    } else {
        "other"
    }
}

/// `GET /queue` — the live music-api download queue (title/artist/priority).
/// Returns an empty array when the endpoint is unavailable (e.g. an older
/// music-api without PR #239), so callers never have to special-case a 404.
pub async fn queue(cfg: &Config) -> Result<serde_json::Value> {
    let url = format!("{}/queue", cfg.music_api_base);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg)?)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        return Ok(serde_json::Value::Array(Vec::new()));
    }
    Ok(resp
        .json::<serde_json::Value>()
        .await
        .unwrap_or(serde_json::Value::Array(Vec::new())))
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
