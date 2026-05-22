//! Retry helpers for Spotify API rate-limit handling
//!
//! Provides [`extract_retry_after_secs`] for parsing Spotify's `Retry-After`
//! header from 429 responses, and [`client_error_retry_after_secs`] for
//! working directly on an rspotify `ClientError`.

use rspotify::ClientError;

/// Extract the Retry-After duration from a Spotify 429 rate-limit error.
/// Works directly on an rspotify::ClientError.
pub fn client_error_retry_after_secs(err: &ClientError) -> Option<u64> {
    if let ClientError::Http(http_err) = err
        && let rspotify::http::HttpError::StatusCode(response) = http_err.as_ref()
            && response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                && let Some(retry_after) = response.headers().get("retry-after") {
                    return retry_after.to_str().ok()?.parse::<u64>().ok();
                }
    None
}

/// Extract the Retry-After duration from a Spotify 429 rate-limit error.
/// Walks the anyhow error chain looking for rspotify::ClientError::Http
/// containing a 429 StatusCode with a retry-after header.
pub fn extract_retry_after_secs(err: &anyhow::Error) -> Option<u64> {
    for cause in err.chain() {
        if let Some(client_err) = cause.downcast_ref::<ClientError>()
            && let Some(secs) = client_error_retry_after_secs(client_err) {
                return Some(secs);
            }
    }
    None
}
