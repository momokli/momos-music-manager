//! Error type for the vendored Deezer downloader.
//!
//! Adapted from Rusteer `src/error.rs` (MIT, Copyright (c) 2026 xScherpschutter)
//! to `thiserror 1` and trimmed to the variants needed for track download.
//!
//! **ARL hygiene:** no variant ever carries the ARL (nor any string derived from
//! it). Errors carry only a Deezer track id or an API message.

use thiserror::Error;

/// Result alias used throughout the module.
pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Public error returned by the `deezer_dl` API.
///
/// This type intentionally never embeds the ARL: `Display`/`Debug` output is
/// safe to log.
#[derive(Debug, Error)]
pub enum Error {
    /// The provided ARL is invalid or expired (HTTP 401 / `USER_ID == 0`).
    #[error("invalid or expired Deezer ARL")]
    InvalidArl,
    /// The account tier does not allow the requested lossless format (HTTP 403).
    #[error("account tier does not allow lossless (FLAC)")]
    NoLosslessTier,
    /// The track is gone / region-restricted / unreadable (HTTP 404).
    #[error("track {0} is gone (403/404)")]
    TrackGone(String),
    /// Deezer rate-limited the request (HTTP 429).
    #[error("rate limited by Deezer (429)")]
    RateLimited,
    /// Any other Deezer API error (JSON body `errors[].message`).
    #[error("deezer api error: {0}")]
    Api(String),
    /// Cryptographic failure (key derivation / decryption).
    #[error("crypto error: {0}")]
    Crypto(String),
    /// Transport-level HTTP failure.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// Local file I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Map an HTTP status code to a status-aware error, *before* body parsing.
    ///
    /// Returns `None` for statuses that should fall through to body parsing.
    pub(crate) fn map_status(status: u16) -> Option<Error> {
        match status {
            401 => Some(Error::InvalidArl),
            403 => Some(Error::NoLosslessTier),
            404 => Some(Error::TrackGone(String::new())),
            429 => Some(Error::RateLimited),
            _ => None,
        }
    }

    /// Map a media endpoint `errors[].message` body message.
    pub(crate) fn map_media_error(msg: &str) -> Error {
        let lower = msg.to_ascii_lowercase();
        if lower.contains("right") || lower.contains("license") || lower.contains("lossless") {
            Error::NoLosslessTier
        } else {
            Error::Api(msg.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_status_maps_known_codes() {
        assert!(matches!(Error::map_status(401), Some(Error::InvalidArl)));
        assert!(matches!(Error::map_status(403), Some(Error::NoLosslessTier)));
        assert!(matches!(Error::map_status(404), Some(Error::TrackGone(_))));
        assert!(matches!(Error::map_status(429), Some(Error::RateLimited)));
        assert!(Error::map_status(500).is_none());
        assert!(Error::map_status(200).is_none());
    }

    #[test]
    fn map_media_error_detects_tier_problems() {
        assert!(matches!(
            Error::map_media_error("No rights on media"),
            Error::NoLosslessTier
        ));
        assert!(matches!(
            Error::map_media_error("license token invalid"),
            Error::NoLosslessTier
        ));
        assert!(matches!(
            Error::map_media_error("lossless not allowed"),
            Error::NoLosslessTier
        ));
        assert!(matches!(
            Error::map_media_error("something else"),
            Error::Api(_)
        ));
    }
}
