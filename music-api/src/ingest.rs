//! Shared ingest core: canonicalise → hash → import → extract tags.
//!
//! Promoted from the one-shot `store-import` CLI (`src/bin/store_import.rs`) so
//! the SSH puller (Part A), the HTTP upload (Part B) and the CLI all share one
//! implementation. The store is content-addressed, so ingest is idempotent: an
//! object already present is skipped.

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::ItemKey;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};

use crate::store::{self, ImportMeta, Store};

/// Accepted audio extensions. `mp4` is included on purpose: the NAS stems are
/// `.stem.mp4` (same container as `.stem.m4a`).
pub const AUDIO_EXTS: &[&str] = &[
    "wav", "m4a", "mp4", "flac", "mp3", "aif", "aiff", "ogg", "wma", "alac", "dsf",
];

/// Stem parts are suffixed `_vocals`, `_bass`, … on WAV parts and their set.
const STEM_PARTS: &[&str] = &["vocals", "bass", "drums", "instrumental", "other"];

/// Lowercase hex of a byte slice.
pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Streaming SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(hasher.finalize()))
}

/// MIME type for an extension.
pub fn content_type(ext: &str) -> &'static str {
    match ext {
        "flac" => "audio/flac",
        "m4a" | "mp4" | "alac" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// Canonical form: the file with MMM's Comment tag cleared. WAV has no MMM
/// comment (stems are source files), so its raw bytes are used as-is.
///
/// Returns the SHA-256 of the canonical bytes. On failure the destination is
/// removed.
pub fn canonicalise(src: &Path, dest: &Path) -> Result<String> {
    let result = (|| -> Result<String> {
        std::fs::copy(src, dest)?;
        let mut tagged = lofty::read_from_path(dest).map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(tag) = tagged.primary_tag_mut() {
            tag.remove_key(&ItemKey::Comment);
        }
        tagged
            .save_to_path(dest, WriteOptions::default())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sha256_file(dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

/// The track a stem part belongs to: the parent directory when nested, otherwise
/// the file name with the stem `_part` and extension stripped.
pub fn classify(rel: &Path) -> (String, Option<String>) {
    let parent = rel.parent().filter(|p| !p.as_os_str().is_empty());
    let file_name = rel
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let stem_type = STEM_PARTS.iter().find_map(|part| {
        let needle = format!("_{part}");
        if file_name.to_lowercase().contains(&needle) {
            Some((*part).to_string())
        } else {
            None
        }
    });

    let group_key = match parent {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            // Strip a `.stem.m4a` / `.stem.mp4` suffix, then the extension.
            let base = file_name
                .strip_suffix(".stem.m4a")
                .or_else(|| file_name.strip_suffix(".stem.mp4"))
                .unwrap_or(&file_name);
            Path::new(base)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(base)
                .to_string()
        }
    };
    (group_key, stem_type)
}

/// Tags read from a file *before* canonicalisation clears the Comment.
#[derive(Debug, Default, Clone)]
pub struct ExtractedTags {
    pub isrc: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub duration_ms: Option<i64>,
}

/// Read tags via lofty. WAV (raw stem sources) has none — returns empty.
pub fn extract_tags(path: &Path) -> ExtractedTags {
    let mut out = ExtractedTags::default();
    let Ok(tagged) = lofty::read_from_path(path) else {
        return out;
    };
    out.duration_ms = Some(tagged.properties().duration().as_millis() as i64);
    let Some(tag) = tagged.primary_tag() else {
        return out;
    };
    let s = |k: ItemKey| tag.get_string(&k).map(|v| v.to_string());
    out.isrc = s(ItemKey::Isrc);
    out.title = s(ItemKey::TrackTitle);
    out.artist = s(ItemKey::TrackArtist);
    out.album = s(ItemKey::AlbumTitle);
    out.genre = s(ItemKey::Genre);
    out.comment = s(ItemKey::Comment);
    out.musical_key = s(ItemKey::InitialKey);
    out.bpm = s(ItemKey::Bpm).and_then(|v| v.parse().ok());
    out
}

/// Outcome of ingesting one file.
#[derive(Debug, Default, Clone)]
pub struct IngestOutcome {
    pub hash: String,
    pub size: u64,
    /// `true` when the bytes were newly stored, `false` on a dedup hit.
    pub stored: bool,
    pub group_key: String,
    pub stem_type: Option<String>,
    pub tags: ExtractedTags,
}

/// Canonicalise → hash → `store::import_file` → record tags + `track_metadata`.
///
/// Idempotent: an object already in the store is skipped (its metadata row is
/// still ensured). `overrides` wins over the file's own tags for the fields it
/// sets. The caller owns `src` and is responsible for removing it afterwards.
pub async fn ingest_file(
    pool: &Pool<Sqlite>,
    store: &Store,
    src: &Path,
    rel: &Path,
    overrides: &ImportMeta,
) -> Result<IngestOutcome> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    // Tags first — canonicalisation clears the Comment.
    let tags = extract_tags(src);

    // WAV: raw bytes. Everything else: canonicalise (clear the Comment tag).
    let (hash, canon_tmp) = if ext == "wav" {
        (sha256_file(src)?, None)
    } else {
        std::fs::create_dir_all(store.tmp_dir())?;
        let dest = store
            .tmp_dir()
            .join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
        (canonicalise(src, &dest)?, Some(dest))
    };

    let (group_key, stem_type) = classify(rel);
    let meta = ImportMeta {
        content_type: Some(content_type(&ext).to_string()),
        original_path: Some(rel.to_string_lossy().to_string()),
        isrc: overrides.isrc.clone().or_else(|| tags.isrc.clone()),
        name: overrides
            .name
            .clone()
            .or_else(|| src.file_name().and_then(|n| n.to_str()).map(str::to_string)),
        group_key: overrides
            .group_key
            .clone()
            .or_else(|| Some(group_key.clone())),
        stem_type: overrides.stem_type.clone().or_else(|| stem_type.clone()),
        title: overrides.title.clone().or_else(|| tags.title.clone()),
        artist: overrides.artist.clone().or_else(|| tags.artist.clone()),
        album: overrides.album.clone().or_else(|| tags.album.clone()),
        genre: overrides.genre.clone().or_else(|| tags.genre.clone()),
        comment: overrides.comment.clone().or_else(|| tags.comment.clone()),
        bpm: overrides.bpm.or(tags.bpm),
        musical_key: overrides
            .musical_key
            .clone()
            .or_else(|| tags.musical_key.clone()),
        duration_ms: overrides.duration_ms.or(tags.duration_ms),
    };

    let src_for_import = canon_tmp.as_deref().unwrap_or(src);
    let stored = store::import_file(pool, store, &hash, src_for_import, &meta).await?;
    if let Some(tmp) = &canon_tmp {
        // import_file moved it into the store, or left it when the object
        // already existed — either way a leftover staging copy is ours to drop.
        let _ = std::fs::remove_file(tmp);
    }

    // Feed the per-ISRC metadata table (Part C).
    if let Some(isrc) = meta.isrc.as_deref().filter(|s| !s.is_empty()) {
        crate::metadata::upsert_from_ingest(pool, isrc, &meta).await?;
    }

    let size = std::fs::metadata(store.path_for(&hash))
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(IngestOutcome {
        hash,
        size,
        stored,
        group_key,
        stem_type,
        tags,
    })
}

/// Minimal glob: `*` (any run) and `?` (one char). Enough for `*.stem.m4a`.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// `true` when `name` should be ingested given the source's include patterns
/// (empty = everything).
pub fn included(include: &[String], name: &str) -> bool {
    include.is_empty() || include.iter().any(|p| glob_match(p, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classify_flat_and_stem() {
        assert_eq!(
            classify(&PathBuf::from("Artist - Title.flac")),
            ("Artist - Title".to_string(), None)
        );
        assert_eq!(
            classify(&PathBuf::from("Artist - Title.stem.m4a")),
            ("Artist - Title".to_string(), None)
        );
        assert_eq!(
            classify(&PathBuf::from("Artist - Title.stem.mp4")),
            ("Artist - Title".to_string(), None)
        );
        assert_eq!(
            classify(&PathBuf::from("Artist - Title_vocals.wav")),
            (
                "Artist - Title_vocals".to_string(),
                Some("vocals".to_string())
            )
        );
        // Nested: the parent directory is the group.
        assert_eq!(
            classify(&PathBuf::from("007/HEIIS3DS.stem.mp4")),
            ("007".to_string(), None)
        );
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("flac"), "audio/flac");
        assert_eq!(content_type("mp4"), "audio/mp4");
        assert_eq!(content_type("m4a"), "audio/mp4");
        assert_eq!(content_type("xyz"), "application/octet-stream");
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.stem.m4a", "Artist - Title.stem.m4a"));
        assert!(!glob_match("*.stem.m4a", "Artist - Title.flac"));
        assert!(glob_match("*.flac", "x.flac"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(included(&[], "whatever"));
        assert!(included(&["*.flac".into(), "*.mp4".into()], "x.mp4"));
        assert!(!included(&["*.flac".into()], "x.mp4"));
    }

    #[test]
    fn extract_tags_on_non_audio_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("not-audio.bin");
        std::fs::write(&p, b"nope").unwrap();
        let t = extract_tags(&p);
        assert!(t.isrc.is_none() && t.title.is_none() && t.bpm.is_none());
    }
}
