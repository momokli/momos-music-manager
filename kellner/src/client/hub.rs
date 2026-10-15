//! Authenticated client for the hub's device-gated routes (issue #364, leaf P1-3).
//!
//! Presents the stored device token as `Authorization: Bearer <token>` (via
//! [`crate::pairing::auth_header`]) and retries transient failures with a
//! bounded backoff. The token is never logged.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::pairing::auth_header;

/// Upload attempts for transient failures (1 initial + 2 retries).
const MAX_ATTEMPTS: u32 = 3;
/// Base backoff; doubles per attempt (200 ms, 400 ms, …).
const BASE_BACKOFF_MS: u64 = 200;

/// Stats echoed by `POST /api/hub/traktor/import`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ImportStats {
    #[serde(default)]
    pub entries: usize,
    #[serde(default)]
    pub matched: usize,
    #[serde(default)]
    pub playlists: usize,
    #[serde(default)]
    pub with_bpm: usize,
    #[serde(default)]
    pub with_key: usize,
}

#[derive(Debug, Deserialize)]
struct ImportResponse {
    #[serde(default)]
    stats: ImportStats,
}

/// One backpack as listed by `GET /api/hub/backpacks` (issue #367).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Backpack {
    /// Server-side backpack id.
    pub id: i64,
    /// Human-readable name.
    #[serde(default)]
    pub name: String,
    /// `true` when the backpack is currently active (synced by `backpack`).
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Deserialize)]
struct ListBackpacksResponse {
    #[serde(default)]
    backpacks: Vec<Backpack>,
}

/// One resolved track from a backpack export (issue #367).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportTrack {
    /// Server-side track id.
    pub track_id: i64,
    /// ISRC (empty when the hub has none).
    #[serde(default)]
    pub isrc: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artists: String,
    /// Resolved tag names (alphabetical on the hub side).
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub bpm: Option<f64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub energy: Option<f64>,
}

/// Answer of `GET /api/hub/backpacks/{id}/export` (issue #367).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportResponse {
    #[serde(default)]
    pub backpack: Backpack,
    #[serde(default)]
    pub tracks: Vec<ExportTrack>,
}

/// A failure that remembers whether a retry could help.
struct UploadError {
    source: anyhow::Error,
    retryable: bool,
}

impl UploadError {
    fn fatal(source: anyhow::Error) -> Self {
        Self {
            source,
            retryable: false,
        }
    }

    fn retryable(source: anyhow::Error) -> Self {
        Self {
            source,
            retryable: true,
        }
    }
}

/// Client bound to one hub base URL + device token.
pub struct HubClient {
    base: String,
    token: String,
    client: reqwest::Client,
}

impl HubClient {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Upload raw NML bytes to the hub, retrying transient failures.
    pub async fn upload_traktor(&self, nml: Vec<u8>) -> Result<ImportStats> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.upload_once(nml.clone()).await {
                Ok(stats) => return Ok(stats),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn upload_once(&self, nml: Vec<u8>) -> std::result::Result<ImportStats, UploadError> {
        let url = format!("{}/api/hub/traktor/import", self.base);
        let auth = auth_header(&self.token).map_err(UploadError::fatal)?;
        let resp = self
            .client
            .post(&url)
            .header(AUTHORIZATION, auth)
            .header(CONTENT_TYPE, "application/xml")
            .body(nml)
            .send()
            .await
            .map_err(|e| UploadError::retryable(e.into()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let parsed: ImportResponse = serde_json::from_str(&body)
                .with_context(|| format!("parsing hub response: {}", body.trim()))
                .map_err(UploadError::fatal)?;
            return Ok(parsed.stats);
        }

        // 401 (bad token) / 400 (bad NML) are permanent; 429 + 5xx are transient.
        let retryable =
            status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
        Err(UploadError {
            source: anyhow!("hub rejected traktor import: HTTP {status}: {}", body.trim()),
            retryable,
        })
    }

    /// The owner's backpacks (`GET /api/hub/backpacks`, #367).
    pub async fn list_backpacks(&self) -> Result<Vec<Backpack>> {
        let url = format!("{}/api/hub/backpacks", self.base);
        let parsed: ListBackpacksResponse = self.get_json(&url, "backpacks").await?;
        Ok(parsed.backpacks)
    }

    /// The resolved tracks of one backpack (`GET /api/hub/backpacks/{id}/export`, #367).
    pub async fn export_backpack(&self, id: i64) -> Result<ExportResponse> {
        let url = format!("{}/api/hub/backpacks/{}/export", self.base, id);
        self.get_json(&url, &format!("backpacks/{id}/export")).await
    }

    /// `GET` a JSON body with the shared bounded-backoff retry policy: `401`
    /// (bad token) is permanent, `429` + `5xx` are retried. The token is only
    /// ever sent in the `Authorization` header (never logged).
    async fn get_json<T: DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.get_json_once(url, what).await {
                Ok(v) => return Ok(v),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn get_json_once<T: DeserializeOwned>(
        &self,
        url: &str,
        what: &str,
    ) -> std::result::Result<T, UploadError> {
        let auth = auth_header(&self.token).map_err(UploadError::fatal)?;
        let resp = self
            .client
            .get(url)
            .header(AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| UploadError::retryable(e.into()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str(&body)
                .with_context(|| format!("parsing hub response for {what}: {}", body.trim()))
                .map_err(UploadError::fatal);
        }

        let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
        Err(UploadError {
            source: anyhow!("hub rejected {what}: HTTP {status}: {}", body.trim()),
            retryable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_response_parses_stats() {
        let json = r#"{"ok":true,"stats":{"entries":3,"matched":2,"playlists":3,"with_bpm":1,"with_key":0}}"#;
        let parsed: ImportResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.stats.entries, 3);
        assert_eq!(parsed.stats.matched, 2);
        assert_eq!(parsed.stats.playlists, 3);
        assert_eq!(parsed.stats.with_bpm, 1);
    }

    #[test]
    fn import_response_tolerates_missing_stats() {
        let parsed: ImportResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.stats, ImportStats::default());
    }

    #[test]
    fn base_url_trims_trailing_slash() {
        assert_eq!(HubClient::new("https://h.test/", "t").base, "https://h.test");
    }

    #[test]
    fn list_backpacks_response_parses() {
        let json = r#"{"backpacks":[{"id":1,"name":"Party","is_active":true},{"id":2,"name":"Chill","is_active":false}]}"#;
        let parsed: ListBackpacksResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.backpacks.len(), 2);
        assert_eq!(parsed.backpacks[0], Backpack { id: 1, name: "Party".into(), is_active: true });
        assert!(!parsed.backpacks[1].is_active);
    }

    #[test]
    fn list_backpacks_tolerates_empty_and_missing() {
        let empty: ListBackpacksResponse = serde_json::from_str(r#"{"backpacks":[]}"#).unwrap();
        assert!(empty.backpacks.is_empty());
        let missing: ListBackpacksResponse = serde_json::from_str("{}").unwrap();
        assert!(missing.backpacks.is_empty());
    }

    #[test]
    fn export_response_parses_full_and_nullable() {
        let json = r#"{
            "backpack":{"id":1,"name":"Party"},
            "tracks":[
              {"track_id":9,"isrc":"DEABC1234567","title":"t","artists":"a","tags":["b","a"],"bpm":128.0,"key":"A-Minor","energy":7.0},
              {"track_id":10,"isrc":"DEABC7654321","title":"u","artists":"c","tags":[],"bpm":null,"key":null,"energy":null}
            ]
        }"#;
        let parsed: ExportResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.backpack.id, 1);
        assert_eq!(parsed.backpack.name, "Party");
        // The export's backpack omits `is_active`; it defaults to false.
        assert!(!parsed.backpack.is_active);
        assert_eq!(parsed.tracks.len(), 2);
        let first = &parsed.tracks[0];
        assert_eq!(first.track_id, 9);
        assert_eq!(first.isrc, "DEABC1234567");
        assert_eq!(first.tags, vec!["b".to_string(), "a".to_string()]);
        assert_eq!(first.bpm, Some(128.0));
        assert_eq!(first.key.as_deref(), Some("A-Minor"));
        assert_eq!(first.energy, Some(7.0));
        let second = &parsed.tracks[1];
        assert!(second.bpm.is_none() && second.key.is_none() && second.energy.is_none());
    }

    #[test]
    fn export_response_tolerates_missing_fields() {
        let parsed: ExportResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, ExportResponse::default());
    }
}
