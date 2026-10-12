//! Last.fm similar-tracks adapter (free API key). Disabled unless
//! `LASTFM_API_KEY` is configured in the environment.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct Similar {
    pub name: String,
    pub artist: String,
    pub match_score: f64,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    similartracks: Option<SimilarTracks>,
}

#[derive(Deserialize)]
struct SimilarTracks {
    #[serde(default)]
    track: Vec<Track>,
}

#[derive(Deserialize)]
struct Track {
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist: Artist,
    #[serde(default, rename = "match")]
    match_: f64,
}

#[derive(Deserialize, Default)]
struct Artist {
    #[serde(default)]
    name: String,
}

/// `track.getSimilar` — needs artist + track name. Empty when no key is set.
pub async fn similar_tracks(cfg: &Config, artist: &str, track: &str) -> Result<Vec<Similar>> {
    let Some(key) = cfg.lastfm_api_key.as_deref() else {
        return Ok(Vec::new());
    };
    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=track.getsimilar&artist={}&track={}&api_key={}&format=json&limit=30",
        urlencoding::encode(artist),
        urlencoding::encode(track),
        urlencoding::encode(key),
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("GET last.fm")?;
    let body: Resp = resp.json().await.unwrap_or(Resp {
        similartracks: None,
    });
    Ok(body
        .similartracks
        .map(|s| s.track)
        .unwrap_or_default()
        .into_iter()
        .map(|t| Similar {
            name: t.name,
            artist: t.artist.name,
            match_score: t.match_,
        })
        .collect())
}
