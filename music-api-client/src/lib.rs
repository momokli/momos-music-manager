//! Shared HTTP client for the `music-api` service.
//!
//! This crate holds the *service-facing* HTTP surface that more than one
//! consumer needs: Deezer metadata, audio bytes, orders and ISRC state. It is
//! deliberately free of any hub-specific coupling — no `sqlx`, no `tokio`, no
//! `chrono`, no `tracing`, and never a reference to `music-hub`'s `Config`.
//!
//! Callers build an [`ApiConfig`] once and pass it in. Hub-only helpers (vault,
//! `/tracks`, `/metadata`, worker control, `/logs`, `/queue`, order listing and
//! every DB helper) stay in the hub.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

/// Connection parameters for the `music-api` service.
#[derive(Debug, Clone, Default)]
pub struct ApiConfig {
    /// Base URL, e.g. `http://127.0.0.1:8710`.
    pub base: String,
    /// Bearer token; `None` when unset.
    pub token: Option<String>,
}

/// The bearer token, or the exact error the hub has always produced.
fn token(cfg: &ApiConfig) -> Result<&str> {
    cfg.token
        .as_deref()
        .context("MUSIC_API_TOKEN is not set")
}

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

// ── Deezer metadata (album sources) ─────────────────────────────────────────

/// One album as listed for an artist (`GET /deezer/artist/{id}/albums`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeezerAlbumSummary {
    pub album_id: String,
    pub title: String,
    pub release_date: Option<String>,
    pub record_type: Option<String>,
    pub cover_url: Option<String>,
    /// Album length in seconds, filled by music-api from its catalogue when the
    /// album is known. Absent (or `null`) for a cold catalogue.
    #[serde(default)]
    pub seconds: Option<i64>,
}

/// One page of an artist's albums.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeezerArtistAlbums {
    pub albums: Vec<DeezerAlbumSummary>,
    pub total: Option<i64>,
    pub next: Option<String>,
}

/// One track of a Deezer album.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeezerAlbumTrack {
    pub isrc: Option<String>,
    pub title: String,
    #[serde(default)]
    pub duration_ms: i64,
    pub position: Option<i64>,
}

/// Album metadata plus its tracklist.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeezerAlbum {
    pub album_id: String,
    pub title: String,
    pub release_date: Option<String>,
    pub record_type: Option<String>,
    pub cover_url: Option<String>,
    pub seconds: Option<i64>,
    #[serde(default)]
    pub tracks: Vec<DeezerAlbumTrack>,
}

/// One artist search hit.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeezerArtistHit {
    pub artist_id: String,
    pub name: String,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DeezerArtistHits {
    #[serde(default)]
    artists: Vec<DeezerArtistHit>,
}

/// Authenticated GET against music-api's Deezer metadata routes.
async fn deezer_get<T: serde::de::DeserializeOwned>(cfg: &ApiConfig, path: &str) -> Result<T> {
    let url = format!("{}{}", cfg.base, path);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(token(cfg)?)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("music-api antwortete {} fuer {path}", resp.status());
    }
    resp.json().await.with_context(|| format!("GET {url}"))
}

/// `GET /deezer/artist/{id}/albums` — one page of an artist's albums.
pub async fn deezer_artist_albums(
    cfg: &ApiConfig,
    artist_id: &str,
    limit: u32,
    index: u32,
) -> Result<DeezerArtistAlbums> {
    deezer_get(
        cfg,
        &format!("/deezer/artist/{artist_id}/albums?limit={limit}&index={index}"),
    )
    .await
}

/// `GET /deezer/album/{id}` — metadata plus tracklist (carries the ISRCs).
pub async fn deezer_album(cfg: &ApiConfig, album_id: &str) -> Result<DeezerAlbum> {
    deezer_get(cfg, &format!("/deezer/album/{}", urlencoding::encode(album_id))).await
}

/// `GET /deezer/search/artist?q=` — name search for the picker.
pub async fn deezer_search_artist(
    cfg: &ApiConfig,
    query: &str,
    limit: u32,
) -> Result<Vec<DeezerArtistHit>> {
    let hits: DeezerArtistHits = deezer_get(
        cfg,
        &format!(
            "/deezer/search/artist?q={}&limit={limit}",
            urlencoding::encode(query)
        ),
    )
    .await?;
    Ok(hits.artists)
}

/// `GET /isrc/{isrc}` — current state of one ISRC.
pub async fn status(cfg: &ApiConfig, isrc: &str) -> Result<IsrcState> {
    let url = format!("{}/isrc/{}", cfg.base, urlencoding::encode(isrc));
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

/// `GET /isrc/{isrc}/{format}` — fetch the actual audio bytes (e.g. `flac`).
/// `format` is a hub token; it is translated via [`api_format`].
pub async fn file_bytes(cfg: &ApiConfig, isrc: &str, format: &str) -> Result<Vec<u8>> {
    let api = api_format(format);
    let url = format!(
        "{}/isrc/{}/{}",
        cfg.base,
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
pub async fn file_bytes_any(cfg: &ApiConfig, isrc: &str, preferred: &str) -> Result<Vec<u8>> {
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
pub async fn order(cfg: &ApiConfig, isrcs: &[String]) -> Result<String> {
    let url = format!("{}/orders", cfg.base);
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
pub async fn order_with_priority(cfg: &ApiConfig, isrcs: &[String], priority: i32) -> Result<String> {
    order_with_priority_intent(cfg, isrcs, priority, "library", None).await
}

/// `POST /orders` with an explicit `intent`/`target` (U0b).
///
/// `intent: toys` tells music-api not to fetch FLAC or run the BPM/key
/// analysis; `target` is the store tier (`flac` | `mp3-320` | `mp3-128`).
pub async fn order_with_priority_intent(
    cfg: &ApiConfig,
    isrcs: &[String],
    priority: i32,
    intent: &str,
    target: Option<&str>,
) -> Result<String> {
    let url = format!("{}/orders", cfg.base);
    let items: Vec<_> = isrcs.iter().map(|i| order_item(i)).collect();
    let mut body = json!({ "items": items, "priority": priority, "intent": intent });
    if let Some(t) = target {
        body["target"] = json!(t);
    }
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token(cfg)?)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let v: serde_json::Value = resp.json().await.unwrap_or_default();
    Ok(v["orderId"].as_str().unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::order_item;

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
}
