//! Gateway API client for authenticated Deezer operations.
//!
//! Vendored from Rusteer `src/api/gateway.rs` (MIT, Copyright (c) 2026
//! xScherpschutter). See ../THIRD_PARTY.md. Trimmed to the track-download path;
//! `call_api`/`get_media_url` now check the HTTP status *before* parsing the
//! body, and the ARL is never stored on the struct (only placed in the cookie
//! jar).

use reqwest::{cookie::Jar, Client, Url};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::error;

use crate::deezer_dl::error::{Error, Result};

/// Gateway API private endpoint.
const GATEWAY_URL: &str = "https://www.deezer.com/ajax/gw-light.php";

/// Media URL endpoint.
const MEDIA_URL: &str = "https://media.deezer.com/v1/get_url";

/// Song server URL template.
const SONG_SERVER_URL: &str = "https://e-cdns-proxy-{n}.dzcdn.net/mobile/1/{hash}";

/// Gateway API client with authentication.
///
/// Holds only the derived tokens and the HTTP client. The ARL passed to
/// [`GatewayApi::new`] is written into the cookie jar and never retained.
#[derive(Debug)]
pub struct GatewayApi {
    client: Client,
    api_token: String,
    license_token: String,
}

/// Song data from the Gateway API.
#[derive(Debug, Clone)]
pub struct SongData {
    /// Song ID.
    pub id: String,
    /// Song title.
    pub title: String,
    /// MD5 hash of the audio file origin.
    pub md5_origin: String,
    /// Media version.
    pub media_version: String,
    /// Track token for media URL requests.
    pub track_token: Option<String>,
    /// Whether the track is readable/available.
    pub readable: bool,
    /// Raw JSON data for additional fields.
    pub raw: Value,
}

/// Media URL data.
#[derive(Debug, Clone)]
pub struct MediaUrl {
    /// The actual media URL.
    pub url: String,
    /// Format (e.g., "FLAC", "MP3_320").
    pub format: String,
    /// Cipher type (e.g., "BF_CBC_STRIPE").
    pub cipher: String,
}

impl GatewayApi {
    /// Create a new Gateway API client with an ARL token.
    ///
    /// Sets the `arl` cookie, then fetches `deezer.getUserData` to obtain the
    /// API and license tokens. Returns [`Error::InvalidArl`] when the ARL is
    /// invalid or expired (`USER_ID == 0`).
    pub async fn new(arl: &str) -> Result<Self> {
        let jar = Arc::new(Jar::default());
        let url = "https://www.deezer.com".parse::<Url>().unwrap();
        jar.add_cookie_str(&format!("arl={}", arl), &url);

        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .cookie_provider(jar)
            .build()
            .map_err(Error::Http)?;

        let mut api = Self {
            client,
            api_token: "null".to_string(),
            license_token: String::new(),
        };

        api.refresh_token().await?;

        Ok(api)
    }

    /// Refresh the API and license tokens from `deezer.getUserData`.
    async fn refresh_token(&mut self) -> Result<()> {
        let user_data = self.get_user_data().await?;

        let user_id = user_data
            .get("USER")
            .and_then(|u| u.get("USER_ID"))
            .and_then(|id| id.as_u64())
            .unwrap_or(0);

        if user_id == 0 {
            return Err(Error::InvalidArl);
        }

        self.api_token = user_data
            .get("checkForm")
            .and_then(|t| t.as_str())
            .unwrap_or("null")
            .to_string();

        self.license_token = user_data
            .get("USER")
            .and_then(|u| u.get("OPTIONS"))
            .and_then(|o| o.get("license_token"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        Ok(())
    }

    /// Make a request to the Gateway API, mapping the status before parsing.
    async fn call_api(&self, method: &str, json_data: Option<Value>) -> Result<Value> {
        let params = [
            ("api_version", "1.0"),
            ("api_token", &self.api_token),
            ("input", "3"),
            ("method", method),
        ];

        // Always send a JSON body (Deezer returns 411 without Content-Length).
        let body = json_data.unwrap_or_else(|| json!({}));

        let response = self
            .client
            .post(GATEWAY_URL)
            .query(&params)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;

        // Status-aware mapping BEFORE body parsing (401/403/404/429).
        if let Some(err) = Error::map_status(status.as_u16()) {
            if matches!(err, Error::TrackGone(_)) {
                // Enrich with the method for diagnostics (never the ARL).
                return Err(Error::TrackGone(method.to_string()));
            }
            return Err(err);
        }

        let result: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                let preview = if text.len() > 500 {
                    format!("{}...", &text[..500])
                } else {
                    text.clone()
                };
                error!(
                    "Failed to parse Gateway response (status {}): {}",
                    status, preview
                );
                return Err(Error::Api(format!(
                    "Invalid JSON response (status {}): {}",
                    status, e
                )));
            }
        };

        let results = result.get("results").cloned().unwrap_or(Value::Null);

        if results.is_null() {
            if let Some(error) = result.get("error") {
                let error_msg = error.to_string();
                error!("Gateway API error: {}", error_msg);
                return Err(Error::Api(error_msg));
            }
        }

        Ok(results)
    }

    /// Get user data (includes `checkForm` token and license token).
    async fn get_user_data(&self) -> Result<Value> {
        self.call_api("deezer.getUserData", None).await
    }

    /// Get detailed song data.
    pub async fn get_song_data(&self, song_id: &str) -> Result<SongData> {
        let json_data = json!({ "sng_id": song_id });

        let result = self.call_api("song.getData", Some(json_data)).await?;

        if result.is_null() {
            return Err(Error::TrackGone(song_id.to_string()));
        }

        let md5_origin = result
            .get("MD5_ORIGIN")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(SongData {
            id: result
                .get("SNG_ID")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            title: result
                .get("SNG_TITLE")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            readable: !md5_origin.is_empty(),
            md5_origin,
            media_version: result
                .get("MEDIA_VERSION")
                .and_then(|v| v.as_str())
                .unwrap_or("1")
                .to_string(),
            track_token: result
                .get("TRACK_TOKEN")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            raw: result,
        })
    }

    /// Get media URLs for downloading.
    ///
    /// `quality` is a Deezer format string ("FLAC", "MP3_320", "MP3_128").
    /// Status and `errors[].message` are mapped to status-aware errors.
    pub async fn get_media_url(
        &self,
        track_tokens: &[String],
        quality: &str,
    ) -> Result<Vec<MediaUrl>> {
        let json_data = json!({
            "license_token": self.license_token,
            "media": [
                {
                    "type": "FULL",
                    "formats": [
                        {
                            "cipher": "BF_CBC_STRIPE",
                            "format": quality
                        }
                    ]
                }
            ],
            "track_tokens": track_tokens
        });

        let response = self.client.post(MEDIA_URL).json(&json_data).send().await?;
        let status = response.status();

        // Status-aware mapping BEFORE body parsing.
        if let Some(err) = Error::map_status(status.as_u16()) {
            return Err(err);
        }

        let result: Value = response.json().await?;

        // Media-level errors (e.g. no lossless right) come in the body.
        if let Some(errors) = result.get("errors").and_then(|e| e.as_array()) {
            if let Some(first_error) = errors.first() {
                let msg = first_error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                return Err(Error::map_media_error(msg));
            }
        }

        let media_data = result
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::Api("No media data".to_string()))?;

        let mut urls = Vec::new();

        for item in media_data {
            if let Some(media_arr) = item.get("media").and_then(|m| m.as_array()) {
                for media in media_arr {
                    if let Some(sources) = media.get("sources").and_then(|s| s.as_array()) {
                        for source in sources {
                            if let Some(url) = source.get("url").and_then(|u| u.as_str()) {
                                urls.push(MediaUrl {
                                    url: url.to_string(),
                                    format: media
                                        .get("format")
                                        .and_then(|f| f.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    cipher: media
                                        .get("cipher")
                                        .and_then(|c| c.get("type"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("BF_CBC_STRIPE")
                                        .to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(urls)
    }

    /// Generate a legacy song URL.
    pub fn get_song_url(n: u8, song_hash: &str) -> String {
        SONG_SERVER_URL
            .replace("{n}", &n.to_string())
            .replace("{hash}", song_hash)
    }

    /// Whether a license token (premium access) was obtained.
    pub fn has_license_token(&self) -> bool {
        !self.license_token.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_song_url_generation() {
        let url = GatewayApi::get_song_url(2, "abc123");
        assert!(url.contains("e-cdns-proxy-2"));
        assert!(url.contains("abc123"));
    }
}
