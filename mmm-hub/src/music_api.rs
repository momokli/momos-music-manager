//! Adapter for the `music-api` service (ISRC-keyed order/consume, on .200:8710).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

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
