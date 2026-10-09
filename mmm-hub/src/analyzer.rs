//! Client for the local BPM/key analyzer service (Essentia on the hub host,
//! e.g. `http://127.0.0.1:8711`, see `deploy/analyzer/`). The service takes a
//! server-local audio path and returns BPM + key + Camelot. Disabled unless
//! `HUB_ANALYZER_URL` is configured.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::config::Config;

#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    /// 1280-d Discogs-EffNet embedding, when requested and available.
    pub embedding: Option<Vec<f32>>,
    /// Top genres `(label, score)`, descending.
    pub genres: Vec<(String, f32)>,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    bpm: Option<f64>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    camelot: Option<String>,
    #[serde(default)]
    embedding: Option<Vec<f32>>,
    #[serde(default)]
    genres: Option<Vec<GenreItem>>,
}

#[derive(Deserialize)]
struct GenreItem {
    #[serde(default)]
    label: String,
    #[serde(default)]
    score: f32,
}

/// Is an analyzer base URL configured?
pub fn enabled(cfg: &Config) -> bool {
    cfg.analyzer_base
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// `POST {base}/analyze {"path": ..., "embed": bool}` — analyze one
/// server-local audio file. Returns `Ok(None)` when no analyzer is configured.
pub async fn analyze(cfg: &Config, path: &str, embed: bool) -> Result<Option<Analysis>> {
    let Some(base) = cfg
        .analyzer_base
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let url = format!("{}/analyze", base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .context("build reqwest client")?;
    let resp = client
        .post(&url)
        .json(&json!({ "path": path, "embed": embed }))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("analyzer {url} -> {status}: {body}");
    }
    let r: Resp = resp.json().await.unwrap_or(Resp {
        bpm: None,
        key: None,
        camelot: None,
        embedding: None,
        genres: None,
    });
    Ok(Some(Analysis {
        bpm: r.bpm,
        key: r.key,
        camelot: r.camelot,
        embedding: r.embedding,
        genres: r
            .genres
            .unwrap_or_default()
            .into_iter()
            .map(|g| (g.label, g.score))
            .collect(),
    }))
}
