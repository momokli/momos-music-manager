//! Vendored from Rusteer — MIT License.
//! Upstream: https://github.com/xScherpschutter/Rusteer (v0.1.0, commit de6561a0, 2026-03-09).
//! Copyright (c) 2026 xScherpschutter. See ./LICENSE.
//!
//! Adopted: `crypto`, `api/gateway` (trimmed, status-aware), `error`.
//! Everything else in this module is original music-api code.
//!
//! Track download only: no tagging, no album/playlist/batch, no CLI.
//!
//! # ARL hygiene
//!
//! The ARL is supplied once to [`login`], placed into the HTTP cookie jar and
//! never retained: neither [`Session`] nor the errors nor any log carry it.

mod crypto;
mod error;
mod gateway;

pub use error::Error;

use std::path::{Path, PathBuf};

/// Audio quality. Format strings are exactly those expected by Deezer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// 128 kbit/s MP3.
    #[default]
    Mp3_128,
    /// 320 kbit/s MP3.
    Mp3_320,
    /// Lossless FLAC.
    Flac,
}

impl Quality {
    /// Deezer API format string: `"MP3_128"` | `"MP3_320"` | `"FLAC"`.
    pub fn format(&self) -> &'static str {
        match self {
            Quality::Flac => "FLAC",
            Quality::Mp3_320 => "MP3_320",
            Quality::Mp3_128 => "MP3_128",
        }
    }

    /// File extension: `".mp3"` | `".flac"`.
    pub fn extension(&self) -> &'static str {
        match self {
            Quality::Flac => ".flac",
            Quality::Mp3_320 | Quality::Mp3_128 => ".mp3",
        }
    }
}

/// Result of a file download.
#[derive(Debug, Clone)]
pub struct Downloaded {
    /// Path the decrypted file was written to.
    pub path: PathBuf,
    /// Quality that was downloaded.
    pub quality: Quality,
    /// Size of the written file in bytes.
    pub bytes: u64,
}

/// An authenticated Deezer session.
///
/// Holds only the derived API/license tokens and an HTTP client — never the ARL.
pub struct Session {
    gateway: gateway::GatewayApi,
}

#[cfg(test)]
impl Session {
    /// Test-only constructor: a real `Session` whose underlying client has the
    /// given ARL placed in its cookie jar (no network I/O). Used to prove the
    /// ARL never surfaces through the session's real types.
    pub(crate) fn for_test(arl: &str) -> Self {
        Session {
            gateway: gateway::GatewayApi::for_test(arl),
        }
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("api_token", &"<redacted>")
            .field("license_token", &"<redacted>")
            .finish()
    }
}

/// Authenticate against Deezer using an ARL.
///
/// Returns [`Error::InvalidArl`] when the ARL is invalid or expired.
pub async fn login(arl: &str) -> Result<Session, Error> {
    let gateway = gateway::GatewayApi::new(arl).await?;
    Ok(Session { gateway })
}

/// Download a track, decrypted, into memory (no temp file).
///
/// The requested [`Quality`] is used verbatim (no fallback): an account without
/// the required tier yields [`Error::NoLosslessTier`].
pub async fn download_track(
    session: &Session,
    deezer_id: &str,
    quality: Quality,
) -> Result<Vec<u8>, Error> {
    let encrypted = fetch_encrypted(session, deezer_id, quality).await?;
    Ok(crypto::decrypt_track_bytes(&encrypted, deezer_id))
}

/// Download a track, decrypted, to `path` (extension is the caller's choice;
/// use [`Quality::extension`]).
pub async fn download_track_to_file(
    session: &Session,
    deezer_id: &str,
    quality: Quality,
    path: &Path,
) -> Result<Downloaded, Error> {
    let encrypted = fetch_encrypted(session, deezer_id, quality).await?;
    crypto::decrypt_track(&encrypted, deezer_id, path)?;
    let bytes = std::fs::metadata(path)?.len();
    Ok(Downloaded {
        path: path.to_path_buf(),
        quality,
        bytes,
    })
}

/// Shared: resolve track token, request exactly `quality`, fetch the bytes.
async fn fetch_encrypted(
    session: &Session,
    deezer_id: &str,
    quality: Quality,
) -> Result<Vec<u8>, Error> {
    let song = session.gateway.get_song_data(deezer_id).await?;
    if !song.readable {
        return Err(Error::TrackGone(deezer_id.to_string()));
    }
    let track_token = song
        .track_token
        .clone()
        .ok_or_else(|| Error::Api("no track token".to_string()))?;

    let urls = session
        .gateway
        .get_media_url(&[track_token], quality.format())
        .await?;
    let media = urls
        .into_iter()
        .next()
        .ok_or(Error::NoLosslessTier)?;

    match session.gateway.fetch_bytes(&media.url).await {
        Ok(bytes) => Ok(bytes),
        // Media 404: report the real Deezer id, not an empty placeholder.
        Err(Error::TrackGone(_)) => Err(Error::TrackGone(deezer_id.to_string())),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_format() {
        assert_eq!(Quality::Mp3_128.format(), "MP3_128");
        assert_eq!(Quality::Mp3_320.format(), "MP3_320");
        assert_eq!(Quality::Flac.format(), "FLAC");
    }

    #[test]
    fn quality_extension() {
        assert_eq!(Quality::Mp3_128.extension(), ".mp3");
        assert_eq!(Quality::Mp3_320.extension(), ".mp3");
        assert_eq!(Quality::Flac.extension(), ".flac");
    }

    #[test]
    fn quality_default_is_mp3_128() {
        assert_eq!(Quality::default(), Quality::Mp3_128);
    }

    /// A dummy ARL that must never surface anywhere.
    const DUMMY_ARL: &str = "this-is-a-fake-arl-1234567890";

    /// ARL hygiene: the ARL actually flows through the session path, and no
    /// rendering of the session's real types or of any error leaks it.
    #[test]
    fn arl_never_in_error_rendering() {
        // Build a real `Session` with the dummy ARL in its cookie jar; this is
        // the exact path `login` takes after a successful `getUserData`.
        let session = Session::for_test(DUMMY_ARL);

        let session_debug = format!("{:?}", session);
        let gateway_debug = format!("{:?}", session.gateway);
        assert!(
            !session_debug.contains(DUMMY_ARL),
            "Session Debug leaked ARL: {session_debug}"
        );
        assert!(
            !gateway_debug.contains(DUMMY_ARL),
            "Gateway Debug leaked ARL: {gateway_debug}"
        );

        let errors = vec![
            Error::InvalidArl,
            Error::NoLosslessTier,
            Error::TrackGone("3135556".to_string()),
            Error::RateLimited,
            Error::Api("some api message".to_string()),
            Error::Crypto("crypto detail".to_string()),
            Error::Io(std::io::Error::other("io detail")),
        ];

        for err in &errors {
            let display = err.to_string();
            let debug = format!("{:?}", err);
            assert!(
                !display.contains(DUMMY_ARL),
                "Display leaked ARL: {display}"
            );
            assert!(!debug.contains(DUMMY_ARL), "Debug leaked ARL: {debug}");
            // No ARL-like fragment either.
            assert!(
                !display.contains("fake-arl"),
                "Display leaked ARL fragment: {display}"
            );
        }
    }

    /// A real `Session` Debug render must redact the tokens verbatim: it would
    /// go red the moment the manual `Debug` were replaced by a derived one.
    #[test]
    fn session_debug_is_redacted() {
        let session = Session::for_test(DUMMY_ARL);
        let rendered = format!("{:?}", session);
        assert!(rendered.contains("<redacted>"), "not redacted: {rendered}");
        assert!(!rendered.contains("test-api-token"), "api token leaked: {rendered}");
        assert!(
            !rendered.contains("test-license-token"),
            "license token leaked: {rendered}"
        );
        assert!(!rendered.contains(DUMMY_ARL), "ARL leaked: {rendered}");
    }
}
