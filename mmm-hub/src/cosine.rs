//! cosine.club similar-tracks adapter.
//!
//! cosine.club is an audio-similarity search engine over 2M+ (mostly electronic /
//! underground) tracks, built on the same `discogs-effnet` model we plan to run
//! locally. Its API is free (120 req/min); we enable it only when `COSINECLUB_API`
//! is configured. It gives the digging view breadth (records our own catalog and
//! the streaming services never see), and each hit carries a YouTube/Discogs link.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

use crate::config::Config;

/// A track suggested by cosine.club.
#[derive(Debug, Clone)]
pub struct Similar {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub track: String,
    pub score: Option<f64>,
    pub video_uri: Option<String>,
    pub external_link: Option<String>,
}

#[derive(Deserialize)]
struct BulkResp {
    #[serde(default)]
    data: Option<BulkData>,
}

#[derive(Deserialize)]
struct BulkData {
    #[serde(default)]
    results: Vec<BulkResult>,
}

#[derive(Deserialize)]
struct BulkResult {
    #[serde(default)]
    similar_tracks: Vec<Track>,
}

#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    data: Vec<Track>,
}

#[derive(Deserialize)]
struct SimilarResp {
    #[serde(default)]
    data: Option<SimilarData>,
}

#[derive(Deserialize)]
struct SimilarData {
    #[serde(default)]
    similar_tracks: Vec<Track>,
}

#[derive(Deserialize)]
struct Track {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    track: String,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    video_uri: Option<String>,
    #[serde(default)]
    external_link: Option<String>,
}

impl Track {
    fn into_similar(self) -> Similar {
        // cosine sometimes returns `track` as the bare title and `name` as
        // "Artist - Title"; prefer the explicit fields and fall back sensibly.
        let title = if self.track.is_empty() {
            self.name.clone()
        } else {
            self.track.clone()
        };
        Similar {
            id: self.id,
            name: self.name,
            artist: self.artist,
            track: title,
            score: self.score,
            video_uri: self.video_uri,
            external_link: self.external_link,
        }
    }
}

/// Is a cosine.club API key configured?
pub fn enabled(cfg: &Config) -> bool {
    cfg.cosine_api_key
        .as_deref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("mmm-hub/0.9 (+https://hub.zukkafabrik.de)")
        .build()
        .context("build reqwest client")
}

fn base(cfg: &Config) -> &str {
    cfg.cosine_base.trim_end_matches('/')
}

/// Similar tracks for a seed given by artist + title.
///
/// Tries `POST /search/bulk` (exact "Artist - Track" match, one round-trip) and
/// falls back to `GET /search` → `GET /tracks/{id}/similar` when the exact match
/// misses (common for our suffixed/underground titles). Returns empty when no key
/// is configured or nothing is found — never errors the caller hard.
pub async fn similar_tracks(
    cfg: &Config,
    artist: &str,
    title: &str,
    limit: usize,
) -> Result<Vec<Similar>> {
    let Some(key) = cfg.cosine_api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(Vec::new());
    };
    let artist = artist.trim();
    let title = title.trim();
    if artist.is_empty() || title.is_empty() {
        return Ok(Vec::new());
    }
    let http = client()?;

    // 1. Bulk exact match → similar in one call.
    let bulk_url = format!("{}/search/bulk", base(cfg));
    let bulk_req = http
        .post(&bulk_url)
        .bearer_auth(key)
        .json(&serde_json::json!({
            "tracks": [format!("{artist} - {title}")],
            "similar_limit": limit,
        }))
        .send()
        .await;
    if let Ok(resp) = bulk_req {
        if resp.status().is_success() {
            let body: BulkResp = resp.json().await.unwrap_or(BulkResp { data: None });
            let out: Vec<Similar> = body
                .data
                .map(|d| d.results)
                .unwrap_or_default()
                .into_iter()
                .flat_map(|r| r.similar_tracks)
                .map(Track::into_similar)
                .collect();
            if !out.is_empty() {
                return Ok(out);
            }
        }
    }

    // 2. Fallback: fuzzy search → similar by id.
    let search_url = format!("{}/search", base(cfg));
    let search_req = http
        .get(&search_url)
        .bearer_auth(key)
        .query(&[("q", format!("{artist} {title}")), ("limit", "5".to_string())])
        .send()
        .await;
    let mut hit: Option<String> = None;
    if let Ok(resp) = search_req {
        if resp.status().is_success() {
            let body: SearchResp = resp.json().await.unwrap_or(SearchResp { data: vec![] });
            hit = body.data.into_iter().map(|t| t.id).find(|id| !id.is_empty());
        }
    }
    let Some(id) = hit else {
        return Ok(Vec::new());
    };

    let sim_url = format!("{}/tracks/{}/similar", base(cfg), urlencoding::encode(&id));
    let sim_req = http
        .get(&sim_url)
        .bearer_auth(key)
        .query(&[("limit", limit.to_string())])
        .send()
        .await;
    if let Ok(resp) = sim_req {
        if resp.status().is_success() {
            let body: SimilarResp = resp.json().await.unwrap_or(SimilarResp { data: None });
            return Ok(body
                .data
                .map(|d| d.similar_tracks)
                .unwrap_or_default()
                .into_iter()
                .map(Track::into_similar)
                .collect());
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_without_key() {
        let cfg = Config::for_test("sqlite::memory:");
        assert!(!enabled(&cfg));
    }

    #[test]
    fn enabled_with_key() {
        let mut cfg = Config::for_test("sqlite::memory:");
        cfg.cosine_api_key = Some("cosine_test".into());
        assert!(enabled(&cfg));
    }

    #[test]
    fn parses_bulk_similar() {
        let json = r#"{"success":true,"data":{"results":[{"query":"A - B","track":{},
            "similar_tracks":[{"id":"1","name":"X - Y","artist":"X","track":"Y",
            "score":0.87,"video_uri":"https://youtu.be/1","external_link":"https://discogs.com/release/1"}]}],
            "unmatched":[]}}"#;
        let body: BulkResp = serde_json::from_str(json).unwrap();
        let tracks: Vec<Similar> = body
            .data
            .unwrap()
            .results
            .into_iter()
            .flat_map(|r| r.similar_tracks)
            .map(Track::into_similar)
            .collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].artist, "X");
        assert_eq!(tracks[0].track, "Y");
        assert_eq!(tracks[0].score, Some(0.87));
        assert_eq!(tracks[0].video_uri.as_deref(), Some("https://youtu.be/1"));
    }

    #[test]
    fn track_falls_back_to_name_when_title_empty() {
        let t = Track {
            id: "9".into(),
            name: "Artist - Title".into(),
            artist: "Artist".into(),
            track: String::new(),
            score: None,
            video_uri: None,
            external_link: None,
        };
        assert_eq!(t.into_similar().track, "Artist - Title");
    }

    #[tokio::test]
    async fn no_key_returns_empty_without_network() {
        let cfg = Config::for_test("sqlite::memory:");
        let out = similar_tracks(&cfg, "A", "B", 10).await.unwrap();
        assert!(out.is_empty());
    }
}
