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

/// Build one `/orders` item from a track key: `source:id` becomes
/// `{source, externalId}`; a bare ISRC becomes `{isrc}`.
fn order_item(key: &str) -> serde_json::Value {
    match key.split_once(':') {
        Some((src, id)) if !src.is_empty() && !id.is_empty() => {
            serde_json::json!({ "source": src, "externalId": id })
        }
        _ => serde_json::json!({ "isrc": key }),
    }
}

/// `POST /orders` — order one or more ISRCs. Returns the order id.
pub async fn order(cfg: &Config, isrcs: &[String]) -> Result<String> {
    let url = format!("{}/orders", cfg.music_api_base);
    let items: Vec<_> = isrcs.iter().map(|i| order_item(i)).collect();
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
    let items: Vec<_> = isrcs.iter().map(|i| order_item(i)).collect();
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

// ── native integration: bulk metadata, live queue, event log ─────────────────

/// One object-store entry for an ISRC (presence proof for downloads/restore).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjRef {
    pub hash: String,
    pub size: i64,
}

/// Bulk metadata/state for one ISRC (from `GET /tracks`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMeta {
    pub isrc: String,
    pub state: Option<String>,
    pub deezer_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub source_format: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub objects: Vec<ObjRef>,
    /// True when the ISRC is not in the music-api ledger at all.
    #[serde(default)]
    pub unknown: bool,
}

impl TrackMeta {
    pub fn ready(&self) -> bool {
        self.state.as_deref() == Some("ready")
    }
    /// Total stored object bytes (sum over objects).
    pub fn object_bytes(&self) -> i64 {
        self.objects.iter().map(|o| o.size).sum()
    }
}

#[derive(Deserialize, Default)]
struct TracksResp {
    #[serde(default)]
    tracks: Vec<TrackMeta>,
}

/// `GET /tracks?isrcs=a,b,c` — bulk metadata + state + objects for many ISRCs.
pub async fn tracks(cfg: &Config, isrcs: &[String]) -> Result<Vec<TrackMeta>> {
    if isrcs.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{}/tracks", cfg.music_api_base);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg)?)
        .query(&[("isrcs", isrcs.join(","))])
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api {url} -> {}", resp.status());
    }
    let body: TracksResp = resp.json().await.unwrap_or_default();
    Ok(body.tracks)
}

/// One entry of the live deemix queue.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueueEntry {
    pub id: String,
    pub title: Option<String>,
    pub status: String,
    pub progress: Option<f64>,
}

#[derive(Deserialize)]
struct QueueResp {
    #[serde(default)]
    items: Vec<QueueEntry>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    counts: std::collections::BTreeMap<String, i64>,
}

/// `GET /queue` — typed live queue + per-state ISRC counts (empty on failure).
pub async fn live_queue(cfg: &Config) -> (Vec<QueueEntry>, Option<String>, Vec<(String, i64)>) {
    let url = format!("{}/queue", cfg.music_api_base);
    let resp = match reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg).unwrap_or_default())
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return (Vec::new(), None, Vec::new()),
    };
    match resp.json::<QueueResp>().await {
        Ok(b) => (b.items, b.error, b.counts.into_iter().collect()),
        Err(_) => (Vec::new(), None, Vec::new()),
    }
}

#[cfg(test)]
mod queue_parse_tests {
    use super::QueueResp;

    #[test]
    fn parses_counts_from_queue_payload() {
        let v: QueueResp =
            serde_json::from_str(r#"{"items":[],"counts":{"pending":2,"ready":1}}"#).unwrap();
        assert_eq!(v.counts.get("pending"), Some(&2));
        assert_eq!(v.counts.get("ready"), Some(&1));
    }

    #[test]
    fn tolerates_missing_counts() {
        let v: QueueResp = serde_json::from_str(r#"{"items":[]}"#).unwrap();
        assert!(v.counts.is_empty());
    }
}

/// One service event (from `GET /logs`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogLine {
    pub at: i64,
    pub level: String,
    pub msg: String,
}

#[derive(Deserialize)]
struct LogsResp {
    #[serde(default)]
    logs: Vec<LogLine>,
}

/// `GET /logs?limit=n` — recent service events (empty on failure).
pub async fn logs(cfg: &Config, limit: usize) -> Vec<LogLine> {
    let url = format!("{}/logs", cfg.music_api_base);
    let resp = match reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg).unwrap_or_default())
        .query(&[("limit", limit.to_string())])
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    resp.json::<LogsResp>()
        .await
        .map(|b| b.logs)
        .unwrap_or_default()
}

// ── control endpoints ────────────────────────────────────────────────────────

/// `GET /worker` — is the worker paused?
pub async fn worker_paused(cfg: &Config) -> bool {
    let url = format!("{}/worker", cfg.music_api_base);
    match reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg).unwrap_or_default())
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["paused"].as_bool())
            .unwrap_or(false),
        _ => false,
    }
}

/// Worker state + retry/cadence config (from `GET /worker`).
pub struct WorkerInfo {
    pub ok: bool,
    pub paused: bool,
    pub interval_secs: u64,
    pub max_attempts: i64,
}

/// `GET /worker` — pause state, cadence and retry cap (defaults when unavailable).
pub async fn worker_info(cfg: &Config) -> WorkerInfo {
    let url = format!("{}/worker", cfg.music_api_base);
    let v = match reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg).unwrap_or_default())
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok(),
        _ => None,
    };
    match v {
        Some(v) => WorkerInfo {
            ok: true,
            paused: v["paused"].as_bool().unwrap_or(false),
            interval_secs: v["intervalSecs"].as_u64().unwrap_or(0),
            max_attempts: v["maxAttempts"].as_i64().unwrap_or(0),
        },
        None => WorkerInfo {
            ok: false,
            paused: false,
            interval_secs: 0,
            max_attempts: 0,
        },
    }
}

/// `POST /worker/pause|resume`.
pub async fn set_worker_paused(cfg: &Config, paused: bool) -> Result<()> {
    let verb = if paused { "pause" } else { "resume" };
    let url = format!("{}/worker/{verb}", cfg.music_api_base);
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api {url} -> {}", resp.status());
    }
    Ok(())
}

/// `POST /isrc/{isrc}/retry`.
pub async fn retry_isrc(cfg: &Config, isrc: &str) -> Result<()> {
    let url = format!(
        "{}/isrc/{}/retry",
        cfg.music_api_base,
        urlencoding::encode(isrc)
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api {url} -> {}", resp.status());
    }
    Ok(())
}

/// `POST /orders/{id}/cancel`.
pub async fn cancel_order(cfg: &Config, id: &str) -> Result<()> {
    let url = format!(
        "{}/orders/{}/cancel",
        cfg.music_api_base,
        urlencoding::encode(id)
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api {url} -> {}", resp.status());
    }
    Ok(())
}

/// `POST /orders/{id}/priority`.
pub async fn set_order_priority(cfg: &Config, id: &str, priority: i64) -> Result<()> {
    let url = format!(
        "{}/orders/{}/priority",
        cfg.music_api_base,
        urlencoding::encode(id)
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .json(&serde_json::json!({ "priority": priority }))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api {url} -> {}", resp.status());
    }
    Ok(())
}

/// One order as returned by `GET /orders`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderRow {
    #[serde(rename = "id", alias = "orderId")]
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Deserialize, Default)]
struct OrdersResp {
    #[serde(default)]
    orders: Vec<OrderRow>,
}

/// `GET /orders?limit=n` — recent orders (empty on failure).
pub async fn orders(cfg: &Config, limit: usize) -> Vec<OrderRow> {
    let url = format!("{}/orders", cfg.music_api_base);
    let resp = match reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg).unwrap_or_default())
        .query(&[("limit", limit.to_string())])
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    resp.json::<OrdersResp>()
        .await
        .map(|b| b.orders)
        .unwrap_or_default()
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

// ── per-ISRC metadata (BPM/key push + range read) ────────────────────────────

/// One metadata push item for music-api's `POST /metadata`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetadataPush {
    pub isrc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camelot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// BPM/key rows from the hub's own analysis, to push to music-api. Pure DB read
/// (no network) — used by both the CLI and the ops console.
pub async fn analysis_metadata(pool: &SqlitePool, limit: usize) -> Result<Vec<MetadataPush>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT t.isrc AS isrc, a.bpm AS bpm, a.key AS key, a.camelot AS camelot, a.source AS source
           FROM hub_track_analysis a
           JOIN hub_tracks t ON t.id = a.track_id
          WHERE t.isrc IS NOT NULL AND TRIM(t.isrc) <> ''
          ORDER BY a.analyzed_at DESC
          LIMIT ?1",
    )
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| MetadataPush {
            isrc: r.get::<String, _>("isrc"),
            bpm: r.get::<Option<f64>, _>("bpm"),
            key: r.get::<Option<String>, _>("key"),
            camelot: r.get::<Option<String>, _>("camelot"),
            source: r.get::<Option<String>, _>("source"),
        })
        .collect())
}

#[derive(Deserialize)]
struct MetadataPushResp {
    #[serde(default)]
    upserted: usize,
}

/// `POST /metadata` — bulk upsert ISRC → {bpm, key, camelot}. Returns the count.
pub async fn push_metadata(cfg: &Config, items: &[MetadataPush]) -> Result<usize> {
    let url = format!("{}/metadata", cfg.music_api_base);
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .json(&json!({ "items": items }))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api /metadata -> {}", resp.status());
    }
    Ok(resp
        .json::<MetadataPushResp>()
        .await
        .map(|r| r.upserted)
        .unwrap_or(items.len()))
}

/// Push the hub's stored BPM/key to music-api (makes music-api the authority).
pub async fn push_analysis(pool: &SqlitePool, cfg: &Config, limit: usize) -> Result<usize> {
    let items = analysis_metadata(pool, limit).await?;
    if items.is_empty() {
        return Ok(0);
    }
    push_metadata(cfg, &items).await
}

// ── metadata range read (ISRC → BPM/key authority) ──────────────────────────

/// One row of music-api's `GET /metadata?isrcs=` response. camelCase like the
/// API (`musicalKey`, `updatedAt`); robust against missing fields.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct MetadataRow {
    #[serde(default)]
    pub isrc: String,
    #[serde(default)]
    pub bpm: Option<f64>,
    /// Canonical key field (`musicalKey`).
    #[serde(default)]
    pub musical_key: Option<String>,
    /// Output alias the API also echoes (`key`) — same value as `musical_key`.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub camelot: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

impl MetadataRow {
    /// The musical key, preferring `musicalKey` and falling back to the `key`
    /// output alias (the API sends both).
    pub fn musical_key(&self) -> Option<&str> {
        self.musical_key.as_deref().or(self.key.as_deref())
    }
}

#[derive(Deserialize, Default)]
struct MetadataRangeResp {
    #[serde(default)]
    items: Vec<MetadataRow>,
    #[serde(default)]
    missing: Vec<String>,
}

/// `GET /metadata?isrcs=a,b,c` — BPM/key for a set of ISRCs from the ISRC
/// authority. Returns `None` when the request could not be made or failed (so
/// callers can tell a transient outage from an all-unknown result),
/// `Some((items, missing))` otherwise; empty input yields `Some((vec![], vec![]))`.
pub async fn metadata_range(
    cfg: &Config,
    isrcs: &[String],
) -> Option<(Vec<MetadataRow>, Vec<String>)> {
    if isrcs.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let t = token(cfg).ok()?;
    let url = format!("{}/metadata", cfg.music_api_base);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(t)
        .query(&[("isrcs", isrcs.join(","))])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.json::<MetadataRangeResp>().await.ok()?;
    Some((body.items, body.missing))
}

// ── musicalKey → Camelot (§Z) ────────────────────────────────────────────────

/// Pitch class (sharps, C=0) for a root note letter + optional `#`/`b`.
fn note_pitch(letter: char, accidental: Option<char>) -> Option<i64> {
    let base = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    Some(match accidental {
        Some('#') => base + 1,
        Some('b') => base - 1,
        Some(_) => return None,
        None => base,
    })
}

/// Camelot notation (`^([1-9]|1[0-2])[AB]$`, case-insensitive) -> canonical.
fn split_camelot(s: &str) -> Option<String> {
    let up = s.trim().to_uppercase();
    let letter = up.chars().last()?;
    if !matches!(letter, 'A' | 'B') {
        return None;
    }
    let num: u8 = up[..up.len() - 1].parse().ok()?;
    if (1..=12).contains(&num) {
        Some(format!("{num}{letter}"))
    } else {
        None
    }
}

/// Traktor notation (`^([1-9]|1[0-2])[md]$`) -> Camelot (`m`→`A`, `d`→`B`).
fn split_traktor(s: &str) -> Option<String> {
    let up = s.trim().to_uppercase();
    let letter = match up.chars().last()? {
        'M' => 'A',
        'D' => 'B',
        _ => return None,
    };
    let num: u8 = up[..up.len() - 1].parse().ok()?;
    if (1..=12).contains(&num) {
        Some(format!("{num}{letter}"))
    } else {
        None
    }
}

/// Parse a `musicalKey` value into canonical Camelot notation, or `None` when it
/// is not a recognised key. Handles Camelot (`8A`), Traktor (`8m`/`8d`) and note
/// names (`C`, `Am`, `F#m`, `A-Minor`, `Db`), case-insensitively. Consistent with
/// [`crate::features::camelot`] (the inverse lives in
/// [`crate::features::camelot_to_pitch_mode`]).
pub fn parse_musical_key(s: &str) -> Option<String> {
    let raw = s.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(c) = split_camelot(raw) {
        return Some(c);
    }
    if let Some(c) = split_traktor(raw) {
        return Some(c);
    }
    // Note name: root letter + optional accidental + optional mode ending.
    let mut chars = raw.chars();
    let letter = chars.next()?;
    let mut accidental = None;
    let mut rest = String::new();
    let tail: Vec<char> = chars.collect();
    let mut idx = 0;
    if let Some(&c) = tail.first() {
        if c == '#' || c == 'b' {
            accidental = Some(c);
            idx = 1;
        }
    }
    rest.extend(tail[idx..].iter());
    let pitch = note_pitch(letter, accidental)?;
    let mode = match rest.trim_start_matches(['-', ' ']).to_ascii_lowercase().as_str() {
        "" | "major" | "maj" | "d" | "dur" => 1,
        "m" | "min" | "minor" => 0,
        _ => return None,
    };
    Some(crate::features::camelot(pitch.rem_euclid(12), mode))
}

/// `GET /vault/tracks?isrcs=a,b,c` — the vault's resolved best BPM/key per ISRC.
///
/// music-api is the Data Vault of music; it resolves the "right" value per track
/// by source precedence and returns
/// `{"tracks":[{"isrc","bpm","key","camelot","energy","source","updatedAt"}]}`.
///
/// Returns `None` when the request could not be made or failed (so callers can
/// tell a transient outage from an all-unknown result), `Some(tracks)` otherwise.
pub async fn vault_tracks(cfg: &Config, isrcs: &[String]) -> Option<Vec<serde_json::Value>> {
    if isrcs.is_empty() {
        return Some(Vec::new());
    }
    let t = token(cfg).ok()?;
    let url = format!("{}/vault/tracks", cfg.music_api_base);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(t)
        .query(&[("isrcs", isrcs.join(","))])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let items = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v.get("tracks").and_then(|x| x.as_array()).cloned())
        .unwrap_or_default();
    Some(items)
}

// ── Data Vault: frozen per-track view ────────────────────────────────────────

/// The frozen Data-Vault view of one track (`GET /vault/track/{isrc}`).
///
/// The hub is a pure consumer: it renders the HUB identity, the RESOLVED best
/// BPM/key, the LINKS (artists/album) and the transparent SATELLITES
/// (per-analyzer analysis, download state, per-source meta) straight from this
/// one response — no second cache, no divergent live panel.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultTrack {
    #[serde(default)]
    pub isrc: String,
    #[serde(default)]
    pub hub: VaultHub,
    #[serde(default)]
    pub resolved: VaultResolved,
    #[serde(default)]
    pub links: VaultLinks,
    #[serde(default)]
    pub satellites: VaultSatellites,
}

/// Track identity as the vault knows it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHub {
    pub title: Option<String>,
    pub duration_ms: Option<i64>,
    pub explicit: Option<bool>,
}

/// The vault's resolved "right" BPM/key/energy (picked by source precedence).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultResolved {
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    /// Analyzer the resolved value came from (precedence winner).
    pub source: Option<String>,
}

/// Links out of the vault: artists + album, each with provider ids.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultLinks {
    #[serde(default)]
    pub artists: Vec<VaultArtistLink>,
    pub album: Option<VaultAlbumLink>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultArtistLink {
    /// The artist hub's business key (normalised); `None` when unknown.
    #[serde(default)]
    pub name: Option<String>,
    /// Provider → id (e.g. `{"spotify": "…", "deezer": "…"}`).
    #[serde(default)]
    pub external_ids: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAlbumLink {
    pub title: Option<String>,
    pub release_date: Option<String>,
    #[serde(default)]
    pub external_ids: std::collections::BTreeMap<String, String>,
}

/// The transparent satellites behind a track: every analysis result, the
/// download state and every meta record, each with its source + load date.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSatellites {
    #[serde(default)]
    pub analysis: Vec<VaultAnalysis>,
    pub download: Option<VaultDownload>,
    #[serde(default)]
    pub meta: Vec<VaultMeta>,
}

/// One analyzer's analysis result (one row per analyzer).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAnalysis {
    pub analyzer: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    pub load_date: Option<String>,
}

/// The download satellite (object-store state for the ISRC).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDownload {
    pub state: Option<String>,
    pub source_format: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    pub load_date: Option<String>,
}

/// One meta record (one row per record source).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMeta {
    pub record_source: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub load_date: Option<String>,
}

/// `GET /vault/track/{isrc}` — the frozen per-track Data-Vault view.
///
/// Returns `None` when the request could not be made or failed (unknown ISRC →
/// 404, outage, missing token), so callers can omit the vault section entirely.
pub async fn vault_track(cfg: &Config, isrc: &str) -> Option<VaultTrack> {
    let t = token(cfg).ok()?;
    let url = format!(
        "{}/vault/track/{}",
        cfg.music_api_base,
        urlencoding::encode(isrc)
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(t)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<VaultTrack>().await.ok()
}

#[cfg(test)]
mod tests {
    use super::{MetadataRow, VaultTrack, order_item, parse_musical_key};

    #[test]
    fn metadata_row_deserialises_camelcase() {
        let r: MetadataRow = serde_json::from_value(serde_json::json!({
            "isrc": "USAAA0000009",
            "bpm": 126.0,
            "musicalKey": "11m",
            "key": "11m",
            "camelot": "8A",
            "source": "essentia",
            "updatedAt": 1700000000
        }))
        .expect("deserialise metadata row");
        assert_eq!(r.isrc, "USAAA0000009");
        assert_eq!(r.bpm, Some(126.0));
        assert_eq!(r.musical_key.as_deref(), Some("11m"));
        assert_eq!(r.musical_key(), Some("11m"));
        assert_eq!(r.camelot.as_deref(), Some("8A"));
        assert_eq!(r.source.as_deref(), Some("essentia"));
        assert_eq!(r.updated_at, 1700000000);
    }

    #[test]
    fn metadata_row_tolerates_missing_fields() {
        let r: MetadataRow = serde_json::from_value(serde_json::json!({ "isrc": "X" }))
            .expect("minimal metadata row");
        assert_eq!(r.isrc, "X");
        assert_eq!(r.bpm, None);
        assert!(r.camelot.is_none());
        assert_eq!(r.updated_at, 0);
    }

    #[test]
    fn metadata_row_falls_back_to_key_alias() {
        let r: MetadataRow =
            serde_json::from_value(serde_json::json!({ "isrc": "X", "key": "8A" }))
                .expect("key alias");
        assert_eq!(r.musical_key, None);
        assert_eq!(r.musical_key(), Some("8A"));
    }

    #[test]
    fn parse_musical_key_handles_known_notations() {
        assert_eq!(parse_musical_key("8A").as_deref(), Some("8A"));
        assert_eq!(parse_musical_key("8a").as_deref(), Some("8A"));
        assert_eq!(parse_musical_key("8m").as_deref(), Some("8A")); // Traktor minor
        assert_eq!(parse_musical_key("8d").as_deref(), Some("8B")); // Traktor major
        assert_eq!(parse_musical_key("11m").as_deref(), Some("11A"));
        assert_eq!(parse_musical_key("C").as_deref(), Some("8B")); // C major
        assert_eq!(parse_musical_key("Am").as_deref(), Some("8A")); // A minor
        assert_eq!(parse_musical_key("F#m").as_deref(), Some("11A")); // F# minor
        assert_eq!(parse_musical_key("A-Minor").as_deref(), Some("8A"));
        assert_eq!(parse_musical_key("Db").as_deref(), Some("3B")); // Db major
        assert_eq!(parse_musical_key("").as_deref(), None);
        assert_eq!(parse_musical_key("xx").as_deref(), None);
        assert_eq!(parse_musical_key("13A").as_deref(), None);
    }

    #[test]
    fn parse_musical_key_round_trips_camelot_wheel() {
        for pitch in 0..12 {
            for mode in [0, 1] {
                let c = crate::features::camelot(pitch, mode);
                assert_eq!(parse_musical_key(&c).as_deref(), Some(c.as_str()));
                let (p, m) = crate::features::camelot_to_pitch_mode(&c).unwrap();
                assert_eq!(parse_musical_key(&c).as_deref(), Some(c.as_str()));
                assert_eq!(crate::features::camelot(p, m), c);
            }
        }
    }

    #[test]
    fn order_item_splits_composite_keys() {
        assert_eq!(
            order_item("soundcloud:1837565340"),
            serde_json::json!({ "source": "soundcloud", "externalId": "1837565340" })
        );
        assert_eq!(
            order_item("USQX91201487"),
            serde_json::json!({ "isrc": "USQX91201487" })
        );
    }

    #[test]
    fn vault_track_deserialises_the_frozen_shape() {
        let json = serde_json::json!({
            "isrc": "USAAA0000009",
            "hub": { "title": "Anthem", "durationMs": 210000, "explicit": true },
            "resolved": {
                "bpm": 126.0, "key": "11m", "camelot": "8A", "energy": 0.75,
                "source": "essentia"
            },
            "links": {
                "artists": [
                    { "name": "Alice", "externalIds": { "spotify": "sp1", "deezer": "dz1" } }
                ],
                "album": {
                    "title": "Shared", "releaseDate": "2024-01-01",
                    "externalIds": { "spotify": "al1" }
                }
            },
            "satellites": {
                "analysis": [
                    { "analyzer": "traktor", "bpm": 125.0, "key": "11m", "camelot": "8A",
                      "energy": 0.7, "loadDate": "2026-01-01T00:00:00+00:00" },
                    { "analyzer": "essentia", "bpm": 126.0, "key": "11m", "camelot": "8A",
                      "energy": 0.75, "loadDate": "2026-01-02T00:00:00+00:00" }
                ],
                "download": {
                    "state": "ready", "sourceFormat": "flac",
                    "formats": ["flac", "mp3-320"], "loadDate": "2026-01-03T00:00:00+00:00"
                },
                "meta": [
                    { "recordSource": "deezer", "title": "Anthem", "artist": "Alice",
                      "album": "Shared", "durationMs": 210000,
                      "loadDate": "2026-01-01T00:00:00+00:00" }
                ]
            }
        });
        let v: VaultTrack = serde_json::from_value(json).expect("deserialise vault track");
        assert_eq!(v.isrc, "USAAA0000009");
        assert_eq!(v.hub.title.as_deref(), Some("Anthem"));
        assert_eq!(v.hub.duration_ms, Some(210000));
        assert_eq!(v.hub.explicit, Some(true));
        assert_eq!(v.resolved.bpm, Some(126.0));
        assert_eq!(v.resolved.source.as_deref(), Some("essentia"));
        assert_eq!(v.links.artists.len(), 1);
        assert_eq!(v.links.artists[0].name.as_deref(), Some("Alice"));
        assert_eq!(
            v.links.artists[0]
                .external_ids
                .get("spotify")
                .map(String::as_str),
            Some("sp1")
        );
        let album = v.links.album.expect("album link");
        assert_eq!(album.title.as_deref(), Some("Shared"));
        assert_eq!(album.release_date.as_deref(), Some("2024-01-01"));
        assert_eq!(v.satellites.analysis.len(), 2);
        assert_eq!(
            v.satellites.analysis[0].analyzer.as_deref(),
            Some("traktor")
        );
        let dl = v.satellites.download.expect("download satellite");
        assert_eq!(dl.state.as_deref(), Some("ready"));
        assert_eq!(dl.source_format.as_deref(), Some("flac"));
        assert_eq!(dl.formats, vec!["flac", "mp3-320"]);
        assert_eq!(v.satellites.meta.len(), 1);
        assert_eq!(
            v.satellites.meta[0].record_source.as_deref(),
            Some("deezer")
        );
    }

    #[test]
    fn vault_track_tolerates_missing_sections() {
        let v: VaultTrack =
            serde_json::from_value(serde_json::json!({ "isrc": "X" })).expect("minimal vault");
        assert_eq!(v.isrc, "X");
        assert!(v.links.artists.is_empty());
        assert!(v.satellites.analysis.is_empty());
        assert!(v.satellites.download.is_none());
    }
}
