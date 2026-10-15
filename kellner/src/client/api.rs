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

/// Delivery formats music-api can serve, in kellner's preference order.
pub const DELIVERY_FORMATS: &[&str] = &["flac", "320", "128"];

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

/// One order line (`POST /orders`): an ISRC to acquire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderItem {
    pub isrc: String,
}

#[derive(Serialize)]
struct OrderRequest {
    items: Vec<OrderItem>,
    priority: i64,
}

/// Accepted order echoed by `POST /orders`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub count: usize,
}

/// Per-ISRC status row from `GET /tracks?isrcs=…`.
///
/// `unknown: true` rows carry no other field (the server never omits them, so
/// indexes stay aligned). Absence of `formats`/`state` is tolerated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TrackStatus {
    #[serde(default)]
    pub isrc: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub unknown: bool,
}

impl TrackStatus {
    /// `true` once the file is servable.
    pub fn is_ready(&self) -> bool {
        self.state.as_deref() == Some("ready") && !self.unknown
    }

    /// Non-recoverable outcome (explicit failure, error text or unknown ISRC) —
    /// neither ready nor still pending.
    pub fn is_failed(&self) -> bool {
        if self.unknown || self.error.as_deref().is_some_and(|e| !e.is_empty()) {
            return true;
        }
        matches!(self.state.as_deref(), Some("failed" | "error" | "absent"))
    }
}

/// Bulk answer from `GET /tracks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TracksResponse {
    #[serde(default)]
    pub tracks: Vec<TrackStatus>,
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

    /// Place one order for `isrcs` (`POST /orders`).
    ///
    /// The ISRCs are sorted + deduplicated so the request body is
    /// deterministic regardless of caller order; `priority` is passed through.
    pub async fn order(&self, isrcs: &[String], priority: i64) -> Result<OrderResponse> {
        let mut sorted: Vec<String> = isrcs.to_vec();
        sorted.sort();
        sorted.dedup();
        let req = OrderRequest {
            items: sorted.into_iter().map(|isrc| OrderItem { isrc }).collect(),
            priority,
        };
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.order_once(&req).await {
                Ok(resp) => return Ok(resp),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn order_once(&self, req: &OrderRequest) -> std::result::Result<OrderResponse, ApiError> {
        let url = format!("{}/orders", self.base);
        let auth = auth_header(&self.token).map_err(ApiError::fatal)?;
        let resp = self
            .client
            .post(&url)
            .header(AUTHORIZATION, auth)
            .json(req)
            .send()
            .await
            .map_err(|e| ApiError::retryable(e.into()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let parsed: OrderResponse = serde_json::from_str(&body)
                .with_context(|| format!("parsing music-api order response: {}", body.trim()))
                .map_err(ApiError::fatal)?;
            return Ok(parsed);
        }
        Err(classify(status, "orders", &body))
    }

    /// Bulk status for `isrcs` (`GET /tracks?isrcs=…`), one round trip per call.
    pub async fn tracks(&self, isrcs: &[String]) -> Result<TracksResponse> {
        let mut sorted: Vec<String> = isrcs.to_vec();
        sorted.sort();
        sorted.dedup();
        let csv = sorted.join(",");
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.tracks_once(&csv).await {
                Ok(resp) => return Ok(resp),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn tracks_once(&self, csv: &str) -> std::result::Result<TracksResponse, ApiError> {
        let url = format!("{}/tracks", self.base);
        let auth = auth_header(&self.token).map_err(ApiError::fatal)?;
        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, auth)
            .query(&[("isrcs", csv)])
            .send()
            .await
            .map_err(|e| ApiError::retryable(e.into()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let parsed: TracksResponse = serde_json::from_str(&body)
                .with_context(|| format!("parsing music-api tracks response: {}", body.trim()))
                .map_err(ApiError::fatal)?;
            return Ok(parsed);
        }
        Err(classify(status, "tracks", &body))
    }

    /// Stream one delivered file (`GET /isrc/{isrc}/{format}`) into memory.
    ///
    /// `format` ∈ `flac|320|128`; a `404` means nothing is servable in that
    /// format. Transient `429`/`5xx` are retried like every other route.
    pub async fn fetch_file(&self, isrc: &str, format: &str) -> Result<Vec<u8>> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.fetch_file_once(isrc, format).await {
                Ok(bytes) => return Ok(bytes),
                Err(e) if e.retryable && attempt < MAX_ATTEMPTS => {
                    let backoff = BASE_BACKOFF_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff)).await;
                }
                Err(e) => return Err(e.source),
            }
        }
    }

    async fn fetch_file_once(
        &self,
        isrc: &str,
        format: &str,
    ) -> std::result::Result<Vec<u8>, ApiError> {
        let url = format!("{}/isrc/{}/{}", self.base, isrc, format);
        let auth = auth_header(&self.token).map_err(ApiError::fatal)?;
        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| ApiError::retryable(e.into()))?;

        let status = resp.status();
        if status.is_success() {
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ApiError::retryable(e.into()))?;
            return Ok(bytes.to_vec());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(classify(status, &format!("isrc/{isrc}/{format}"), &body))
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
