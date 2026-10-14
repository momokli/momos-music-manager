//! Audio tag reading via `ffprobe`.
//!
//! The artist-order ingest needs the ISRC/title/artist/album/duration of every
//! file deemix produced. music-api already ships `ffprobe` (used by
//! [`crate::transcode::probe_bitrate`]) and configures it via `FFPROBE`, so we
//! read tags with it rather than adding a second tag library. `lofty` is present
//! in `Cargo.toml` but unused here: ffprobe is the established media probe, and
//! its JSON output gives a pure, fixture-testable parser.
//!
//! `ffprobe -show_format -show_streams -print_format json` emits the container
//! tags under `format.tags` (keys vary in case: `ISRC`/`isrc`, `TITLE`/`title`,
//! …) and the duration under `format.duration` (seconds, as a string).

use std::path::Path;

use serde::Deserialize;

/// The tags the vault ingest cares about, read from one audio file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioTags {
    /// Normalised ISRC (hyphens/spaces stripped, uppercased). `None` when absent.
    pub isrc: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    tags: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Parse the JSON emitted by `ffprobe -show_format -show_streams -print_format
/// json`. Malformed input yields the empty tag set (never an error) — a file
/// without readable tags is a best-effort skip, not a hard failure.
pub fn parse_ffprobe(json: &str) -> AudioTags {
    let Ok(out) = serde_json::from_str::<FfprobeOutput>(json) else {
        return AudioTags::default();
    };
    let Some(fmt) = out.format else {
        return AudioTags::default();
    };

    // ffprobe tag keys are case-insensitive (`ISRC` vs `isrc`).
    let tag = |key: &str| -> Option<String> {
        fmt.tags.as_ref().and_then(|tags| {
            tags.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .and_then(|(_, v)| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    };

    AudioTags {
        isrc: tag("isrc")
            .map(|s| crate::db::normalize_isrc(&s))
            .filter(|s| !s.is_empty()),
        title: tag("title"),
        artist: tag("artist"),
        album: tag("album"),
        duration_ms: fmt.duration.as_deref().and_then(parse_duration_ms),
    }
}

/// Seconds (as ffprobe prints them, e.g. `343.414000`) → whole milliseconds.
fn parse_duration_ms(secs: &str) -> Option<i64> {
    secs.trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| (s * 1000.0).round() as i64)
}

/// Read the tags of `path` with `ffprobe`. `None` when ffprobe is missing or the
/// file cannot be probed.
pub async fn read_tags(ffprobe: &str, path: &Path) -> Option<AudioTags> {
    let output = tokio::process::Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_format")
        .arg("-show_streams")
        .arg("-print_format")
        .arg("json")
        .arg(path)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_ffprobe(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic `ffprobe -show_format -show_streams -print_format json` body.
    const FIXTURE: &str = r#"{
        "streams": [
            { "index": 0, "codec_name": "flac", "codec_type": "audio", "duration": "343.414000" }
        ],
        "format": {
            "filename": "/downloads/Molchat Doma - Sudno.flac",
            "nb_streams": 1,
            "format_name": "flac",
            "duration": "343.414000",
            "tags": {
                "ISRC": "GBKQU2538819",
                "TITLE": "Sudno",
                "ARTIST": "Molchat Doma",
                "ALBUM": "Etazhi",
                "track": "1"
            }
        }
    }"#;

    #[test]
    fn parses_tags_and_duration() {
        let t = parse_ffprobe(FIXTURE);
        assert_eq!(t.isrc.as_deref(), Some("GBKQU2538819"));
        assert_eq!(t.title.as_deref(), Some("Sudno"));
        assert_eq!(t.artist.as_deref(), Some("Molchat Doma"));
        assert_eq!(t.album.as_deref(), Some("Etazhi"));
        assert_eq!(t.duration_ms, Some(343_414));
    }

    #[test]
    fn normalises_a_hyphenated_isrc() {
        let json = r#"{"format":{"tags":{"isrc":"gb-kqu-25-38819"}}}"#;
        assert_eq!(parse_ffprobe(json).isrc.as_deref(), Some("GBKQU2538819"));
    }

    #[test]
    fn missing_tags_are_none() {
        let json = r#"{"format":{"format_name":"mp3","duration":"12.5"}}"#;
        let t = parse_ffprobe(json);
        assert_eq!(t.isrc, None);
        assert_eq!(t.title, None);
        assert_eq!(t.duration_ms, Some(12_500));
    }

    #[test]
    fn malformed_json_is_empty_not_an_error() {
        assert_eq!(parse_ffprobe("not json"), AudioTags::default());
        assert_eq!(parse_ffprobe("{}"), AudioTags::default());
    }
}
