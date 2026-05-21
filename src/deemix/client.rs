use std::collections::HashMap;

use anyhow::{Context, Result};
use reqwest::Method;
use serde_json;
use sqlx::{Pool, Row, Sqlite};
use tracing::info;

use crate::deemix::models::{
    DeemixActionResult, DeemixLoginResponse, DeemixQueueItem, DeemixQueueResponse,
};

/// HTTP client for the deemix-pyweb web API.
///
/// Uses cookie-based auth: the `connect.sid` session cookie lives only in
/// the reqwest cookie jar (in-memory). On HTTP 401, auto-re-authenticates
/// using the stored ARL from the `service_config` DB table.
#[allow(dead_code)]
pub struct DeemixClient {
    http_client: reqwest::Client,
    base_url: String,
    db: Pool<Sqlite>,
}

#[allow(dead_code)]
impl DeemixClient {
    /// Create a new DeemixClient.
    ///
    /// The reqwest client is built with `cookie_store(true)` to retain
    /// the `connect.sid` session cookie between requests.
    pub fn new(base_url: &str, db: Pool<Sqlite>) -> Self {
        let http_client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .expect("Failed to build reqwest client for DeemixClient");

        Self {
            http_client,
            base_url: base_url.trim_end_matches('/').to_string(),
            db,
        }
    }

    /// Authenticate with an ARL (Account Registration Link).
    ///
    /// POST `/api/loginArl` with `{"status": 1, "arl": "<ARL>"}`.
    /// The session cookie (`connect.sid`) is automatically stored in the
    /// reqwest cookie jar.
    pub async fn login_arl(&self, arl: &str) -> Result<DeemixLoginResponse> {
        let body = serde_json::json!({"status": 1, "arl": arl});
        let resp = self
            .http_client
            .post(format!("{}/api/loginArl", self.base_url))
            .json(&body)
            .send()
            .await
            .context("Failed to send login_arl request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Deemix login failed with status {}: {}", status, text);
        }

        // Read the full text, log it, then try to parse
        let text = resp
            .text()
            .await
            .context("Failed to read login_arl response body")?;
        tracing::debug!("Deemix login_arl raw response: {}", text);

        serde_json::from_str::<DeemixLoginResponse>(&text).map_err(|e| {
            let preview = if text.len() > 500 {
                format!("{}...", &text[..500])
            } else {
                text.clone()
            };
            tracing::error!(
                "Failed to parse DeemixLoginResponse: {}. Raw response (first 500 chars): {}",
                e,
                preview
            );
            anyhow::anyhow!(
                "Failed to parse login_arl response: {}. Response: {}",
                e,
                preview
            )
        })
    }

    /// Create a DeemixClient by reading config from the `service_config` DB table.
    /// Returns None if deemix is not configured or not connected.
    pub async fn from_db(pool: Pool<Sqlite>) -> Option<Self> {
        let config = sqlx::query_as::<_, crate::db::ServiceConfig>(
            "SELECT * FROM service_config WHERE service = 'deemix'",
        )
        .fetch_optional(&pool)
        .await
        .ok()??;

        if !config.is_connected {
            return None;
        }

        // Parse host from metadata_json, fall back to default
        let host = config
            .metadata_json
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v.get("host").and_then(|h| h.as_str().map(String::from)))
            .unwrap_or_else(|| "http://localhost:6595".to_string());

        Some(DeemixClient::new(&host, pool.clone()))
    }

    /// Test the connection to deemix.
    ///
    /// GET `/api/getQueue` — returns `true` if the response is 200 OK.
    pub async fn test_connection(&self) -> Result<bool> {
        let resp = self.authed_request(Method::GET, "/api/getQueue").await?;
        Ok(resp.status().is_success())
    }

    /// Get all queue items from deemix.
    ///
    /// GET `/api/getQueue` — returns the `queue` field as a HashMap keyed by UUID.
    pub async fn get_queue(&self) -> Result<HashMap<String, DeemixQueueItem>> {
        let resp = self.authed_request(Method::GET, "/api/getQueue").await?;

        let text = resp
            .text()
            .await
            .context("Failed to read getQueue response body")?;

        serde_json::from_str::<DeemixQueueResponse>(&text)
            .map(|queue_resp| queue_resp.queue)
            .map_err(|e| {
                let preview = if text.len() > 500 {
                    format!("{}… ({} bytes total)", &text[..500], text.len())
                } else {
                    text.clone()
                };
                anyhow::anyhow!(
                    "Failed to parse getQueue response: {}. Preview: {}",
                    e,
                    preview
                )
            })
    }

    /// Add a Spotify playlist URL to the deemix download queue.
    ///
    /// POST `/api/addToQueue` with `{"url": "..."}`.
    /// The deemix API returns HTTP 200 even on failure; we parse the JSON
    /// `result` field to detect errors like "NotLoggedIn".
    pub async fn add_to_queue(&self, url: &str) -> Result<()> {
        let body = serde_json::json!({"url": url});
        let resp = self
            .authed_request_with_body(Method::POST, "/api/addToQueue", Some(&body))
            .await?;

        // Check result — the deemix API returns HTTP 200 with result:false on errors
        self.ensure_action_success(resp, "addToQueue", Some(url))
            .await?;
        info!("Added URL to deemix queue: {}", url);
        Ok(())
    }

    /// Retry a failed download in the deemix queue.
    ///
    /// POST `/api/retryDownload` with `{"uuid": "..."}`.
    pub async fn retry_download(&self, uuid: &str) -> Result<()> {
        let body = serde_json::json!({"uuid": uuid});
        let resp = self
            .authed_request_with_body(Method::POST, "/api/retryDownload", Some(&body))
            .await?;

        self.ensure_action_success(resp, "retryDownload", Some(uuid))
            .await?;
        info!("Retried download in deemix queue: {}", uuid);
        Ok(())
    }

    /// Ensure a Spotify playlist URL is queued for download.
    ///
    /// If the playlist is already in the deemix queue (any status), re-triggers
    /// via `retry_download` to re-scan for new tracks. If not found, adds it
    /// fresh via `add_to_queue`.
    pub async fn ensure_queued(&self, spotify_url: &str) -> Result<()> {
        let queue = self.get_queue().await?;

        for (uuid, item) in &queue {
            let item_url = format!("https://open.spotify.com/playlist/{}", item.id);
            if item_url == spotify_url {
                return self.retry_download(uuid).await;
            }
        }

        self.add_to_queue(spotify_url).await
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Make an authenticated API call with auto-re-auth on HTTP 401.
    ///
    /// If the response is 401 Unauthorized, it loads the stored ARL from the
    /// database, calls `login_arl()` to get a fresh session cookie, then
    /// retries the original request once.
    async fn authed_request(&self, method: Method, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http_client
            .request(method.clone(), &url)
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            info!("Deemix session expired, re-authenticating...");
            let arl = self.load_arl_from_db().await?;
            self.login_arl(&arl).await?;
            // Retry once
            return self
                .http_client
                .request(method, &url)
                .send()
                .await
                .map_err(Into::into);
        }

        Ok(resp)
    }

    /// Make an authenticated API call with an optional JSON body and auto-re-auth on 401.
    async fn authed_request_with_body(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http_client.request(method.clone(), &url);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            info!("Deemix session expired, re-authenticating...");
            let arl = self.load_arl_from_db().await?;
            self.login_arl(&arl).await?;
            // Retry once with body
            let mut retry_req = self.http_client.request(method, &url);
            if let Some(b) = body {
                retry_req = retry_req.json(b);
            }
            return retry_req.send().await.map_err(Into::into);
        }

        Ok(resp)
    }

    /// Load the ARL from the `service_config` DB table for re-authentication.
    async fn load_arl_from_db(&self) -> Result<String> {
        let row = sqlx::query("SELECT access_token FROM service_config WHERE service = 'deemix'")
            .fetch_one(&self.db)
            .await
            .context("Failed to load deemix ARL from database — is deemix configured?")?;

        let arl: String = row
            .try_get("access_token")
            .context("Deemix access_token (ARL) is NULL")?;

        Ok(arl)
    }

    /// Check the `result` field of a deemix action response (addToQueue, retryDownload).
    ///
    /// The deemix API returns HTTP 200 even on errors, with `{"result": false, "errid": "NotLoggedIn"}`.
    /// This method parses the response, and if it detects a session expiry, re-authenticates
    /// and retries the action once.
    async fn ensure_action_success(
        &self,
        resp: reqwest::Response,
        action_name: &str,
        identifier: Option<&str>,
    ) -> Result<()> {
        let text = resp
            .text()
            .await
            .context(format!("Failed to read {} response body", action_name))?;

        let action_result: DeemixActionResult = serde_json::from_str(&text).context(format!(
            "Failed to parse {} response: {}",
            action_name, text
        ))?;

        if action_result.result {
            return Ok(());
        }

        // Check if the error is a session expiry — retry with fresh auth
        if action_result.errid.as_deref() == Some("NotLoggedIn") {
            info!(
                "Deemix {} failed with NotLoggedIn, re-authenticating...",
                action_name
            );
            let arl = self.load_arl_from_db().await?;
            self.login_arl(&arl).await?;

            // Retry with a fresh request
            let url = format!("{}/api/{}", self.base_url, action_name);
            let body = match action_name {
                "addToQueue" => identifier
                    .map(|id| serde_json::json!({"url": id}))
                    .unwrap_or(serde_json::json!({})),
                "retryDownload" => identifier
                    .map(|id| serde_json::json!({"uuid": id}))
                    .unwrap_or(serde_json::json!({})),
                _ => serde_json::json!({}),
            };
            let retry_resp = self
                .http_client
                .post(&url)
                .json(&body)
                .send()
                .await
                .context(format!("Failed to retry {} after re-auth", action_name))?;

            let retry_text = retry_resp
                .text()
                .await
                .context(format!("Failed to read retry {} response", action_name))?;
            let retry_result: DeemixActionResult =
                serde_json::from_str(&retry_text).context(format!(
                    "Failed to parse retry {} response: {}",
                    action_name, retry_text
                ))?;

            if retry_result.result {
                return Ok(());
            }

            anyhow::bail!(
                "Deemix {} failed after re-auth: {} - {}",
                action_name,
                retry_result.errid.as_deref().unwrap_or("unknown"),
                retry_text
            );
        }

        anyhow::bail!(
            "Deemix {} failed: {} - {}",
            action_name,
            action_result.errid.as_deref().unwrap_or("unknown"),
            text
        );
    }
}
