//! Authenticated client for the hub's device-gated routes (issue #364, leaf P1-3).
//!
//! Presents the stored device token as `Authorization: Bearer <token>` (via
//! [`crate::pairing::auth_header`]) and retries transient failures with a
//! bounded backoff. The token is never logged.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

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
}
