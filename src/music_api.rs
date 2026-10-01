//! Client + configuration for the `music-api` ISRC order service.
//!
//! `music-api` sits in front of deemix: MMM places *orders* of ISRCs, the
//! service resolves them on Deezer, downloads through a dedicated deemix
//! instance and serves the files back by ISRC. See `music-api/README.md` for
//! the full contract.
//!
//! MMM knows the ISRC of every Backpack track, so the fragile Spotify-url
//! ingestion path disappears: orders are keyed by ISRC and Deezer's public
//! lookup needs no credentials.
//!
//! All routes except `/health` require `Authorization: Bearer <token>`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Configuration ─────────────────────────────────────────────────────────

/// Resolved `[music_api]` configuration (env > TOML > defaults).
#[derive(Debug, Clone)]
pub struct MusicApiConfig {
    /// Base URL of the service, e.g. `https://music-api.example.com`.
    pub base_url: Option<String>,
    /// Bearer token (`MUSIC_API_TOKEN`).
    pub token: Option<String>,
    /// Master switch; disabled short-circuits the consumer entirely.
    pub enabled: bool,
    /// How many ISRCs to order per cycle.
    pub batch_size: usize,
    /// Seconds between consumer cycles.
    pub interval_secs: u64,
}

impl Default for MusicApiConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            token: None,
            enabled: true,
            batch_size: 100,
            interval_secs: 900,
        }
    }
}

impl MusicApiConfig {
    /// `true` only when the feature is enabled *and* both the URL and the
    /// token are present. Without a token every request would 401.
    pub fn is_configured(&self) -> bool {
        self.enabled && self.base_url.is_some() && self.token.is_some()
    }
}

// ── Response types (camelCase as served by the service) ───────────────────

/// One order as returned by `GET /orders`.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OrderSummary {
    pub id: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Full order state (`GET /orders/{id}`).
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OrderStatus {
    pub order_id: String,
    pub status: String,
    pub items: Vec<IsrcStatus>,
}

/// One ISRC's state inside an order.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IsrcStatus {
    pub isrc: String,
    pub state: String,
    pub deezer_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub formats: Vec<String>,
    pub error: Option<String>,
}

/// `{ "orderId": "...", ... }` body of `POST /orders`.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct CreateOrderResponse {
    order_id: String,
}

/// `{ "orders": [...] }` body of `GET /orders`.
#[derive(Deserialize, Clone, Debug)]
struct OrderListResponse {
    orders: Vec<OrderSummary>,
}

/// Error body shared by every non-2xx response.
#[derive(Deserialize, Clone, Debug)]
struct ErrorBody {
    error: String,
}

// ── Client ────────────────────────────────────────────────────────────────

/// HTTP client for the `music-api` service.
pub struct MusicApiClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl MusicApiClient {
    /// Build a client with a fresh [`reqwest::Client`].
    pub fn new(base_url: &str, token: &str) -> Self {
        Self::with_http(reqwest::Client::new(), base_url, token)
    }

    /// Build a client around a caller-provided [`reqwest::Client`] (test seam).
    pub fn with_http(http: reqwest::Client, base_url: &str, token: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    /// Absolute URL for a service path.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    /// Turn a failed response into an error, preferring the JSON `error` field.
    async fn error_for(resp: reqwest::Response) -> anyhow::Error {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<ErrorBody>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| body.trim().to_string());
        anyhow::anyhow!("music-api request failed ({status}): {msg}")
    }

    /// `POST /orders` — place an order for the given ISRCs, return its id.
    pub async fn create_order(&self, isrcs: &[String]) -> Result<String> {
        #[derive(Serialize)]
        struct Item<'a> {
            isrc: &'a str,
        }
        #[derive(Serialize)]
        struct Request<'a> {
            items: Vec<Item<'a>>,
        }

        let body = Request {
            items: isrcs.iter().map(|s| Item { isrc: s }).collect(),
        };
        let resp = self
            .http
            .post(self.url("orders"))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("music-api: POST /orders")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let parsed: CreateOrderResponse = resp
            .json()
            .await
            .context("music-api: parsing POST /orders response")?;
        Ok(parsed.order_id)
    }

    /// `GET /orders`, optionally filtered by `status`.
    pub async fn list_orders(&self, status: Option<&str>) -> Result<Vec<OrderSummary>> {
        let mut req = self.http.get(self.url("orders")).bearer_auth(&self.token);
        if let Some(status) = status {
            req = req.query(&[("status", status)]);
        }
        let resp = req.send().await.context("music-api: GET /orders")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let parsed: OrderListResponse = resp
            .json()
            .await
            .context("music-api: parsing GET /orders response")?;
        Ok(parsed.orders)
    }

    /// `GET /orders/{id}` — full item-level state of one order.
    pub async fn get_order(&self, order_id: &str) -> Result<OrderStatus> {
        let resp = self
            .http
            .get(self.url(&format!("orders/{order_id}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("music-api: GET /orders/{id}")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        resp.json()
            .await
            .context("music-api: parsing GET /orders/{id} response")
    }

    /// `GET /isrc/{isrc}/{format}` — raw audio bytes.
    pub async fn download_isrc(&self, isrc: &str, format: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(self.url(&format!("isrc/{isrc}/{format}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("music-api: GET /isrc/{isrc}/{format}")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let bytes = resp
            .bytes()
            .await
            .context("music-api: reading ISRC file body")?;
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_list_parses_camel_case() {
        let json = r#"{"orders":[{"id":"abc","status":"open","createdAt":10,"updatedAt":20}]}"#;
        let parsed: OrderListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.orders.len(), 1);
        assert_eq!(parsed.orders[0].id, "abc");
        assert_eq!(parsed.orders[0].status, "open");
        assert_eq!(parsed.orders[0].created_at, 10);
        assert_eq!(parsed.orders[0].updated_at, 20);
    }

    #[test]
    fn empty_orders_list_parses() {
        let parsed: OrderListResponse = serde_json::from_str(r#"{"orders":[]}"#).unwrap();
        assert!(parsed.orders.is_empty());
    }

    #[test]
    fn order_status_parses_items_and_ignores_extra_fields() {
        let json = r#"{
            "orderId": "o-1",
            "status": "open",
            "createdAt": 5,
            "updatedAt": 7,
            "items": [
                {
                    "isrc": "USQX91201487",
                    "state": "ready",
                    "deezerId": "836932812",
                    "title": "Sudno",
                    "artist": "Molchat Doma",
                    "formats": ["flac", "320", "128"],
                    "error": null
                }
            ]
        }"#;
        let parsed: OrderStatus = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.order_id, "o-1");
        assert_eq!(parsed.status, "open");
        assert_eq!(parsed.items.len(), 1);
        let item = &parsed.items[0];
        assert_eq!(item.isrc, "USQX91201487");
        assert_eq!(item.state, "ready");
        assert_eq!(item.deezer_id.as_deref(), Some("836932812"));
        assert_eq!(item.title.as_deref(), Some("Sudno"));
        assert_eq!(item.artist.as_deref(), Some("Molchat Doma"));
        assert_eq!(item.formats, vec!["flac", "320", "128"]);
        assert!(item.error.is_none());
    }

    #[test]
    fn isrc_status_tolerates_missing_optional_fields() {
        let json = r#"{"isrc":"X","state":"pending","formats":[]}"#;
        let parsed: IsrcStatus = serde_json::from_str(json).unwrap();
        assert!(parsed.deezer_id.is_none());
        assert!(parsed.title.is_none());
        assert!(parsed.artist.is_none());
        assert!(parsed.error.is_none());
        assert!(parsed.formats.is_empty());
    }

    #[test]
    fn error_body_parses() {
        let parsed: ErrorBody = serde_json::from_str(r#"{"error":"bad isrc"}"#).unwrap();
        assert_eq!(parsed.error, "bad isrc");
    }
}
