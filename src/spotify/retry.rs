//! Retry helpers for Spotify API rate-limit handling
//!
//! Provides [`extract_retry_after_secs`] for parsing Spotify's `Retry-After`
//! header from 429 responses, and [`client_error_retry_after_secs`] for
//! working directly on an rspotify `ClientError`.

use rspotify::ClientError;

/// Maximum seconds to wait on a single 429 backoff.
/// Spotify's normal rate limit resets in ~30s, but repeated abuse can
/// escalate the Retry-After to hours. Capping prevents the poller from
/// blocking for half a day.
pub const MAX_RETRY_WAIT_SECS: u64 = 300; // 5 minutes

/// Format a duration in seconds as a human-readable string.
/// e.g. 54056 → "15h 1m 56s", 65 → "1m 5s", 42 → "42s"
pub fn format_duration(total_secs: u64) -> String {
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{}h {:02}m {:02}s", h, m, s)
    } else if m > 0 {
        format!("{}m {:02}s", m, s)
    } else {
        format!("{}s", s)
    }
}

/// Extract the Retry-After duration from a Spotify 429 rate-limit error.
/// Works directly on an rspotify::ClientError.
pub fn client_error_retry_after_secs(err: &ClientError) -> Option<u64> {
    if let ClientError::Http(http_err) = err
        && let rspotify::http::HttpError::StatusCode(response) = http_err.as_ref()
        && response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        && let Some(retry_after) = response.headers().get("retry-after")
    {
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
            && let Some(secs) = client_error_retry_after_secs(client_err)
        {
            return Some(secs);
        }
    }
    None
}

/// Extract and clamp: returns the Retry-After seconds, capped at [`MAX_RETRY_WAIT_SECS`].
/// Use this in poller loops to avoid blocking for hours.
pub fn extract_retry_after_clamped(err: &anyhow::Error) -> Option<u64> {
    extract_retry_after_secs(err).map(|s| s.min(MAX_RETRY_WAIT_SECS))
}
