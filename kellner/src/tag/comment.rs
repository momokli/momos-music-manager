//! Comment-tag writing for backpack tracks (issue #367, #653, #664).
//!
//! The deterministic render/validate semantics live in the [`comment_template`]
//! crate (issue #664); this module keeps only the file-side write.
//! [`write_comment`] writes an already-rendered Comment into the file's primary
//! tag with `lofty`, but **only** when it differs from the existing Comment — an
//! unchanged render leaves the file (and its mtime) untouched.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::Tag;

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
    fn write_comment_writes_then_skips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test.mp3");
        std::fs::copy(fixture("test.mp3"), &dest).unwrap();

        assert!(write_comment(&dest, "Techno BPM-128").unwrap(), "first write");
        let mtime = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert!(!write_comment(&dest, "Techno BPM-128").unwrap(), "unchanged -> no write");
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
