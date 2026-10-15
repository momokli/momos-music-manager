//! Comment-tag rendering + writing for backpack tracks (issue #367, P3-1…P3-4).
//!
//! [`render_comment`] is pure and deterministic: tag names are trimmed, sorted
//! alphabetically, deduplicated and joined with `", "`; the optional audio
//! suffix (`BPM`/key/`Energy`) is appended only when requested. [`write_comment`]
//! writes the render into the file's primary tag with `lofty`, but **only** when
//! it differs from the existing Comment — an unchanged render leaves the file
//! (and its mtime) untouched.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::Tag;

/// Format an audio number deterministically: an integral value prints without a
/// decimal point (`128.0` → `"128"`), a fractional one as-is (`127.5`).
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Render the Comment text for one track.
///
/// Tags are trimmed, empties dropped, sorted alphabetically and deduplicated,
/// then joined with `", "`; an empty result (and no audio suffix) is `""`.
/// With `include_audio`, the present `bpm`/`key`/`energy` parts are appended as
/// `BPM <bpm> | <key> | Energy <energy>`, separated from the tags by two spaces.
/// Deterministic for a given input.
pub fn render_comment(
    tags: &[String],
    bpm: Option<f64>,
    key: Option<&str>,
    energy: Option<f64>,
    include_audio: bool,
) -> String {
    let mut names: Vec<String> = tags
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    names.sort();
    names.dedup();
    let mut out = names.join(", ");

    if include_audio {
        let mut parts: Vec<String> = Vec::new();
        if let Some(b) = bpm {
            parts.push(format!("BPM {}", fmt_num(b)));
        }
        if let Some(k) = key {
            let k = k.trim();
            if !k.is_empty() {
                parts.push(k.to_string());
            }
        }
        if let Some(e) = energy {
            parts.push(format!("Energy {}", fmt_num(e)));
        }
        if !parts.is_empty() {
            if !out.is_empty() {
                out.push_str("  ");
            }
            out.push_str(&parts.join(" | "));
        }
    }

    out
}

/// Write `comment` into the file's primary tag.
///
/// Returns `Ok(true)` when the file was written (the Comment differed), and
/// `Ok(false)` when the existing Comment already equals `comment` (no write, so
/// the mtime is preserved). Formats are covered by `lofty` (FLAC/MP3/M4A/…).
/// An empty `comment` clears the Comment field.
pub fn write_comment(path: &Path, comment: &str) -> Result<bool> {
    let mut tagged = lofty::read_from_path(path)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("lofty read {}", path.display()))?;

    let existing = tagged
        .primary_tag()
        .and_then(|t| t.comment())
        .map(|c| c.to_string())
        .unwrap_or_default();
    if existing == comment {
        return Ok(false);
    }

    let tag_type = tagged.primary_tag_type();
    let tag = match tagged.primary_tag_mut() {
        Some(t) => t,
        None => {
            tagged.insert_tag(Tag::new(tag_type));
            tagged
                .primary_tag_mut()
                .expect("just inserted a tag of the primary type")
        }
    };
    if comment.is_empty() {
        tag.remove_comment();
    } else {
        tag.set_comment(comment.to_string());
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("lofty save {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn render_sorts_and_dedupes_tags() {
        let tags = vec!["b".into(), "a".into(), "b".into(), "  c ".into()];
        assert_eq!(render_comment(&tags, None, None, None, false), "a, b, c");
    }

    #[test]
    fn render_empty_tags_is_empty() {
        assert_eq!(render_comment(&[], None, None, None, false), "");
        assert_eq!(render_comment(&["  ".into()], None, None, None, false), "");
    }

    #[test]
    fn render_audio_suffix_only_present_parts() {
        assert_eq!(
            render_comment(&["Techno".into()], Some(128.0), Some("A-Minor"), Some(7.0), true),
            "Techno  BPM 128 | A-Minor | Energy 7"
        );
        // Only bpm.
        assert_eq!(
            render_comment(&["Techno".into()], Some(128.0), None, None, true),
            "Techno  BPM 128"
        );
        // Fractional values keep their decimals.
        assert_eq!(
            render_comment(&[], Some(127.5), None, Some(6.5), true),
            "BPM 127.5 | Energy 6.5"
        );
        // Blank key is ignored.
        assert_eq!(
            render_comment(&[], None, Some("  "), None, true),
            ""
        );
    }

    #[test]
    fn render_audio_suppressed_when_disabled() {
        assert_eq!(
            render_comment(&["Techno".into()], Some(128.0), Some("A-Minor"), Some(7.0), false),
            "Techno"
        );
    }

    #[test]
    fn render_is_deterministic() {
        let tags = vec!["zeta".into(), "alpha".into(), "mu".into()];
        let first = render_comment(&tags, Some(120.0), Some("C-Major"), Some(5.0), true);
        for _ in 0..10 {
            assert_eq!(
                render_comment(&tags, Some(120.0), Some("C-Major"), Some(5.0), true),
                first
            );
        }
        // Order of the input must not change the output.
        let reordered = vec!["mu".into(), "zeta".into(), "alpha".into()];
        assert_eq!(
            render_comment(&reordered, Some(120.0), Some("C-Major"), Some(5.0), true),
            first
        );
    }

    #[test]
    fn write_comment_writes_then_skips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test.mp3");
        std::fs::copy(fixture("test.mp3"), &dest).unwrap();

        assert!(write_comment(&dest, "Techno  BPM 128").unwrap(), "first write");
        let mtime = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert!(!write_comment(&dest, "Techno  BPM 128").unwrap(), "unchanged -> no write");
        assert_eq!(
            std::fs::metadata(&dest).unwrap().modified().unwrap(),
            mtime,
            "mtime preserved on an unchanged render"
        );
        assert!(write_comment(&dest, "House").unwrap(), "changed -> write");
    }

    #[test]
    fn write_comment_clears_on_empty() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test.flac");
        std::fs::copy(fixture("test.flac"), &dest).unwrap();

        assert!(write_comment(&dest, "Techno").unwrap());
        assert!(write_comment(&dest, "").unwrap(), "clearing changes the file");
        assert!(!write_comment(&dest, "").unwrap(), "already empty -> no write");
    }
}
