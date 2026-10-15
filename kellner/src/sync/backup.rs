//! Library backup to music-api's object store (issue #365, leaves P4-1…P4-3).
//!
//! v1 transport (ADR-081 §10.4) is whole-file, idempotent and server-deduped:
//! walk the collection, canonicalise each audio file (clear the Comment tag via
//! `lofty`, exactly like `legacy/mmm/src/store.rs::canonicalise_to`), take the
//! SHA-256 of those canonical bytes as the object key, one bulk
//! `POST /objects/check`, then `PUT` only the missing objects. A second run
//! uploads nothing. Chunked/resumable upload is **v2** and not built here.
//!
//! Failures are per file and tolerated (ADR-061/062 spirit): a file that cannot
//! be read/canonicalised or an object that fails to upload doesn't abort the run,
//! but is counted in [`BackupStats::failed`] so the caller can exit non-zero.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use sha2::{Digest, Sha256};

use crate::client::api::ApiClient;

/// Directory names skipped while walking the collection (kellner's own trash).
const SKIP_DIRS: &[&str] = &[".kellner-trash"];

/// Audio extensions considered for backup (lowercase, without the dot).
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "mp4", "aac", "wav", "aiff", "aif", "ogg", "opus", "alac", "wma",
];

/// Options for one backup run.
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Directory to walk (already resolved by the caller).
    pub root: PathBuf,
    /// Classify only: `check` runs, but nothing is uploaded.
    pub dry_run: bool,
}

/// Tally of one backup run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupStats {
    /// Audio files encountered.
    pub scanned: usize,
    /// Distinct canonical content hashes among them.
    pub unique: usize,
    /// Objects already present server-side (skipped).
    pub skipped: usize,
    /// Objects newly uploaded.
    pub uploaded: usize,
    /// Files/objects that failed (canonicalisation or upload).
    pub failed: usize,
}

/// Canonicalise `src` (clear the Comment tag) and return
/// `(sha256_hex, canonical_bytes)`.
///
/// The returned bytes are exactly what must be uploaded under `sha256_hex`:
/// music-api re-hashes the body and answers `400` on any mismatch. Semantically
/// identical to `legacy/mmm/src/store.rs::canonicalise_to` — that byte-stability
/// is what makes a file with a different comment dedupe to the same object.
///
/// Any temporary file is removed on both success and failure.
pub fn canonicalise(src: &Path) -> Result<(String, Vec<u8>)> {
    let tmp = temp_path(src);
    let result = (|| -> Result<(String, Vec<u8>)> {
        std::fs::copy(src, &tmp)
            .with_context(|| format!("copy {} -> {}", src.display(), tmp.display()))?;
        let mut tagged = lofty::read_from_path(&tmp)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("lofty read {}", tmp.display()))?;
        if let Some(tag) = tagged.primary_tag_mut() {
            tag.remove_key(&lofty::tag::ItemKey::Comment);
        }
        tagged
            .save_to_path(&tmp, WriteOptions::default())
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("lofty save {}", tmp.display()))?;

        let bytes = std::fs::read(&tmp).with_context(|| format!("reading {}", tmp.display()))?;
        let hash = sha256_hex(&bytes);
        Ok((hash, bytes))
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    } else {
        // Best-effort cleanup; a leftover temp file is not a run failure.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// SHA-256 of a byte slice, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn temp_path(src: &Path) -> PathBuf {
    use rand::RngCore;
    let mut suffix = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut suffix);
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    std::env::temp_dir().join(format!(
        "kellner-canon-{}-{}-{}",
        std::process::id(),
        hex::encode(suffix),
        name
    ))
}

/// Run one backup: walk, canonicalise, dedup server-side, upload what's missing.
pub async fn run_backup(client: &ApiClient, opts: &BackupOptions) -> Result<BackupStats> {
    let mut stats = BackupStats::default();
    let mut by_hash: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    for path in walk_audio(&opts.root) {
        stats.scanned += 1;
        match canonicalise(&path) {
            Ok((hash, bytes)) => {
                by_hash.entry(hash).or_insert(bytes);
            }
            Err(e) => {
                stats.failed += 1;
                eprintln!(
                    "kellner: backup: canonicalising {} failed: {e:#}",
                    path.display()
                );
            }
        }
    }

    stats.unique = by_hash.len();
    if by_hash.is_empty() {
        report(&stats, opts);
        return Ok(stats);
    }

    let hashes: Vec<String> = by_hash.keys().cloned().collect();
    let checked = client
        .check(&hashes)
        .await
        .context("music-api object check failed")?;

    for (hash, bytes) in &by_hash {
        if checked.contains(hash) {
            stats.skipped += 1;
            continue;
        }
        if opts.dry_run {
            continue;
        }
        match client.put(hash, bytes.clone()).await {
            Ok(_) => stats.uploaded += 1,
            Err(e) => {
                stats.failed += 1;
                eprintln!("kellner: backup: uploading {hash} failed: {e:#}");
            }
        }
    }

    report(&stats, opts);
    Ok(stats)
}

fn report(stats: &BackupStats, opts: &BackupOptions) {
    let missing = stats.unique - stats.skipped;
    if opts.dry_run {
        println!(
            "kellner: backup --dry-run: scanned {} file(s) ({} unique); would upload {} object(s), {} already present",
            stats.scanned, stats.unique, missing, stats.skipped
        );
    } else {
        println!(
            "kellner: backup: scanned {} file(s) ({} unique); uploaded {}, {} already present",
            stats.scanned, stats.unique, stats.uploaded, stats.skipped
        );
    }
}

/// Recursively collect audio files under `root`, skipping [`SKIP_DIRS`].
///
/// A directory that cannot be read is reported and skipped — never fatal.
pub fn walk_audio(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_into(root, &mut out);
    out.sort();
    out
}

fn walk_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("kellner: backup: cannot read {}: {e}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if SKIP_DIRS.contains(&name) {
                    continue;
                }
            }
            walk_into(&path, out);
        } else if file_type.is_file() && is_audio(&path) {
            out.push(path);
        }
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tagged_fixture(dir: &Path, name: &str, comment: Option<&str>) -> PathBuf {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("test.mp3");
        let dest = dir.join(name);
        std::fs::copy(&src, &dest).unwrap();
        if let Some(comment) = comment {
            let mut tagged = lofty::read_from_path(&dest).unwrap();
            let tag = tagged.primary_tag_mut().expect("fixture has a primary tag");
            tag.insert_text(lofty::tag::ItemKey::Comment, comment.to_string());
            tagged.save_to_path(&dest, WriteOptions::default()).unwrap();
        }
        dest
    }

    #[test]
    fn comment_only_difference_yields_same_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_tagged_fixture(dir.path(), "a.mp3", Some("from kellner A"));
        let b = write_tagged_fixture(dir.path(), "b.mp3", Some("from kellner B"));

        let (ha, ba) = canonicalise(&a).unwrap();
        let (hb, bb) = canonicalise(&b).unwrap();
        assert_eq!(ha, hb, "files differing only in Comment must dedupe");
        assert_eq!(ba, bb, "canonical bytes must be identical");
        assert_eq!(ha, sha256_hex(&ba), "key must be the body digest");
        assert_eq!(ha.len(), 64);
    }

    #[test]
    fn file_without_comment_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let plain = write_tagged_fixture(dir.path(), "plain.mp3", None);
        let commented = write_tagged_fixture(dir.path(), "c.mp3", Some("something"));

        let (h_plain, b_plain) = canonicalise(&plain).unwrap();
        let (h_comm, _) = canonicalise(&commented).unwrap();
        assert_eq!(h_plain, h_comm, "a stray comment must not change the key");
        assert_eq!(h_plain, sha256_hex(&b_plain));
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn walk_skips_non_audio_and_trash() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join(".kellner-trash");
        std::fs::create_dir_all(&trash).unwrap();
        write_tagged_fixture(dir.path(), "keep.mp3", None);
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        std::fs::write(trash.join("gone.mp3"), b"x").unwrap();

        let found = walk_audio(dir.path());
        assert_eq!(
            found.len(),
            1,
            "only the audio file outside trash: {found:?}"
        );
        assert!(found[0].ends_with("keep.mp3"));
    }
}
