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

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
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

/// Write the ISRC into `path`'s primary tag, creating the tag when the file has
/// none. One code path for both containers: `lofty` maps [`ItemKey::Isrc`] to
/// ID3v2 `TSRC` (MP3) and the Vorbis comment `ISRC` (FLAC). Blocking file I/O is
/// offloaded to a worker thread. Idempotent — an existing ISRC is replaced.
pub async fn write_isrc(path: &Path, isrc: &str) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    let isrc = isrc.to_string();
    tokio::task::spawn_blocking(move || write_isrc_blocking(&path, &isrc))
        .await
        .map_err(|e| anyhow::anyhow!("write_isrc task failed: {e}"))?
}

fn write_isrc_blocking(path: &Path, isrc: &str) -> anyhow::Result<()> {
    let mut tagged =
        lofty::read_from_path(path).map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
    if tagged.primary_tag_mut().is_none() {
        let tag_type = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .primary_tag_mut()
        .ok_or_else(|| anyhow::anyhow!("no primary tag for {}", path.display()))?;
    tag.insert_text(ItemKey::Isrc, isrc.to_string());
    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| anyhow::anyhow!("save {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A private copy of a fixture (never mutate the checked-in file).
    fn fixture_copy(name: &str) -> PathBuf {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let dst = std::env::temp_dir().join(format!(
            "music-api-fixture-{}-{name}",
            std::process::id()
        ));
        std::fs::copy(&src, &dst).expect("copy fixture");
        dst
    }

    /// Read the ISRC back with `lofty` directly (ffprobe may be absent in CI).
    fn read_isrc_lofty(path: &Path) -> Option<String> {
        let f = lofty::read_from_path(path).ok()?;
        f.primary_tag()
            .and_then(|t| t.get_string(&ItemKey::Isrc))
            .map(str::to_string)
    }

    #[tokio::test]
    async fn write_isrc_round_trips_mp3_tsrc() {
        let path = fixture_copy("tiny.mp3");
        assert_eq!(read_isrc_lofty(&path), None, "fixture should start untagged");

        write_isrc(&path, "GBKQU2538819").await.unwrap();
        assert_eq!(
            read_isrc_lofty(&path).as_deref(),
            Some("GBKQU2538819"),
            "MP3 ISRC (ID3 TSRC) not readable"
        );

        // Idempotent: a second write replaces, does not duplicate.
        write_isrc(&path, "USQX91201487").await.unwrap();
        assert_eq!(read_isrc_lofty(&path).as_deref(), Some("USQX91201487"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn write_isrc_round_trips_flac_vorbis() {
        let path = fixture_copy("tiny.flac");
        write_isrc(&path, "FRZ039800054").await.unwrap();
        assert_eq!(
            read_isrc_lofty(&path).as_deref(),
            Some("FRZ039800054"),
            "FLAC ISRC (Vorbis ISRC) not readable"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_isrc_rejects_a_missing_file() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let missing = std::env::temp_dir().join("music-api-does-not-exist.mp3");
        assert!(rt.block_on(write_isrc(&missing, "X")).is_err());
    }

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
