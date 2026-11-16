//! Offline contract tests for the public `deezer_dl` surface.
//!
//! These exercise only the pure/formatting parts of the public API
//! (`Quality`, `Error`). They perform **no** network I/O and need no ARL, so
//! they run in CI (unlike the manual live quality matrix for issue #509).

use music_api::deezer_dl::{Error, Quality};

#[test]
fn quality_format_strings_match_deezer_api() {
    assert_eq!(Quality::Mp3_128.format(), "MP3_128");
    assert_eq!(Quality::Mp3_320.format(), "MP3_320");
    assert_eq!(Quality::Flac.format(), "FLAC");
}

#[test]
fn quality_extensions_are_audio_file_suffixes() {
    assert_eq!(Quality::Mp3_128.extension(), ".mp3");
    assert_eq!(Quality::Mp3_320.extension(), ".mp3");
    assert_eq!(Quality::Flac.extension(), ".flac");
}

#[test]
fn quality_default_is_the_safest_tier() {
    assert_eq!(Quality::default(), Quality::Mp3_128);
}

#[test]
fn error_display_strings_are_stable_and_arl_free() {
    let cases = [
        (Error::InvalidArl, "invalid or expired Deezer ARL"),
        (
            Error::NoLosslessTier,
            "account tier does not allow lossless (FLAC)",
        ),
        (Error::RateLimited, "rate limited by Deezer (429)"),
        (Error::Api("boom".to_string()), "deezer api error: boom"),
        (Error::Crypto("bad key".to_string()), "crypto error: bad key"),
    ];
    for (err, expected) in cases {
        assert_eq!(err.to_string(), expected);
    }
}

#[test]
fn track_gone_display_carries_the_real_deezer_id() {
    let err = Error::TrackGone("3135556".to_string());
    let rendered = err.to_string();
    assert!(rendered.contains("3135556"), "missing id: {rendered}");
    // No empty/placeholder rendering.
    assert!(!rendered.contains("song.getData"), "placeholder: {rendered}");
    assert!(
        !rendered.contains("  "),
        "looks like an empty id slipped through: {rendered:?}"
    );
}
