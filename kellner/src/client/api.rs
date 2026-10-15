//! Authenticated client for music-api's content-addressed object store
//! (issue #365, leaves P4-1…P4-3; ADR-081 §10.4).
//!
//! Presents a Bearer credential (reusing [`crate::pairing::auth_header`]) and
//! retries transient failures with a bounded backoff, mirroring
//! [`crate::client::hub`]. The token is never logged: [`ApiClient`] does not
//! derive `Debug` and the token only ever appears in the `Authorization` header.
//!
//! Server contract (`music-api/src/store.rs`):
//! - `POST /objects/check` — `{"hashes":[…]} → {"present":[…],"missing":[…]}`;
//! - `PUT /objects/{sha256}` — stores the body under its own digest; the server
//!   hashes the received bytes and rejects a mismatch (`400`), so `hash` **must**
//!   be the SHA-256 of `bytes` (the canonicalised file).
//! - a valid key is exactly 64 lowercase hex chars.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

use crate::pairing::auth_header;

/// Upload attempts for transient failures (1 initial + 2 retries).
const MAX_ATTEMPTS: u32 = 3;
/// Base backoff; doubles per attempt (200 ms, 400 ms, …).
const BASE_BACKOFF_MS: u64 = 200;

/// Result of a bulk `POST /objects/check`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CheckResult {
    #[serde(default)]
    pub present: Vec<String>,
    #[serde(default)]
    pub missing: Vec<String>,
}

impl CheckResult {
    /// `true` when the server reports the hash as already stored.
    pub fn contains(&self, hash: &str) -> bool {
        self.present.iter().any(|h| h == hash)
    }
}

/// Outcome of a `PUT /objects/{sha256}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutOutcome {
    /// `true` when the server newly stored the bytes; `false` when the object
    /// already existed (idempotent re-upload).
    pub stored: bool,
}

#[derive(Serialize)]
struct CheckRequest<'a> {
    hashes: &'a [String],
}

#[derive(Deserialize)]
struct PutResponse {
    #[serde(default)]
    stored: bool,
}

/// A failure that remembers whether a retry could help.
struct ApiError {
    source: anyhow::Error,
    retryable: bool,
}

impl ApiError {
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

/// Client bound to one music-api base URL + Bearer token.
///
/// Deliberately **not** `Debug`-derived so the token can never be printed by a
/// stray `{:?}`.
pub struct ApiClient {
    base: String,
    token: String,
    client: reqwest::Client,
}

impl ApiClient {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Bulk existence check: one round trip for all candidate hashes.
    pub async fn check(&self, hashes: &[String]) -> Result<CheckResult> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.check_once(hashes).await {
                Ok(result) => return Ok(result),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn check_once(&self, hashes: &[String]) -> std::result::Result<CheckResult, ApiError> {
        let url = format!("{}/objects/check", self.base);
        let auth = auth_header(&self.token).map_err(ApiError::fatal)?;
        let resp = self
            .client
            .post(&url)
            .header(AUTHORIZATION, auth)
            .json(&CheckRequest { hashes })
            .send()
            .await
            .map_err(|e| ApiError::retryable(e.into()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let parsed: CheckResult = serde_json::from_str(&body)
                .with_context(|| format!("parsing music-api check response: {}", body.trim()))
                .map_err(ApiError::fatal)?;
            return Ok(parsed);
        }
        Err(classify(status, "objects/check", &body))
    }

    /// Upload the canonicalised bytes under their own SHA-256 key.
    ///
    /// Idempotent: a re-upload of an existing object returns
    /// [`PutOutcome::stored`] `== false`.
    pub async fn put(&self, hash: &str, bytes: Vec<u8>) -> Result<PutOutcome> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.put_once(hash, &bytes).await {
                Ok(outcome) => return Ok(outcome),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn put_once(
        &self,
        hash: &str,
        bytes: &[u8],
    ) -> std::result::Result<PutOutcome, ApiError> {
        let url = format!("{}/objects/{}", self.base, hash);
        let auth = auth_header(&self.token).map_err(ApiError::fatal)?;
        let resp = self
            .client
            .put(&url)
            .header(AUTHORIZATION, auth)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|e| ApiError::retryable(e.into()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            // 200 → `{"stored":false,"present":true}` (already there);
            // 201 → `{"stored":true,"size":n}`. A missing/opaque body is harmless.
            let parsed: PutResponse =
                serde_json::from_str(&body).unwrap_or(PutResponse { stored: false });
            return Ok(PutOutcome {
                stored: parsed.stored,
            });
        }
        Err(classify(status, &format!("objects/{hash}"), &body))
    }
}

/// `401` (bad token) / `400` (digest mismatch, bad key) / `413` (too large) are
/// permanent; `429` + `5xx` are transient. Matches [`crate::client::hub`].
fn classify(status: StatusCode, what: &str, body: &str) -> ApiError {
    let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
    let err = anyhow!("music-api rejected {what}: HTTP {status}: {}", body.trim());
    if retryable {
        ApiError::retryable(err)
    } else {
        ApiError::fatal(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trims_trailing_slash() {
        assert_eq!(
            ApiClient::new("https://music.test/", "t").base,
            "https://music.test"
        );
        assert_eq!(
            ApiClient::new("https://music.test///", "t").base,
            "https://music.test"
        );
    }

    #[test]
    fn check_response_parses_present_and_missing() {
        let json = r#"{"present":["aa","bb"],"missing":["cc"]}"#;
        let parsed: CheckResult = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.present, vec!["aa".to_string(), "bb".to_string()]);
        assert_eq!(parsed.missing, vec!["cc".to_string()]);
        assert!(parsed.contains("aa"));
        assert!(!parsed.contains("cc"));
    }

    #[test]
    fn check_response_tolerates_missing_fields() {
        let parsed: CheckResult = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, CheckResult::default());
    }

    #[test]
    fn put_response_parses_stored() {
        let created: PutResponse = serde_json::from_str(r#"{"stored":true,"size":12}"#).unwrap();
        assert!(created.stored);
        let existing: PutResponse =
            serde_json::from_str(r#"{"stored":false,"present":true}"#).unwrap();
        assert!(!existing.stored);
    }
}
