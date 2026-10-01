//! `store-import` — one-time migration of an on-disk tree into the object store.
//!
//! Walks `SOURCE_DIR`, canonicalises every audio file exactly like MMM does
//! (clearing the Comment tag, except for WAV where the raw bytes are used),
//! hashes the result and places it into the content-addressed store. It writes a
//! manifest (`<relpath>\t<hash>\t<size>\t<group_key>\t<stem_type>`) so the caller
//! can reconcile another database afterwards.
//!
//! Usage:
//!   `STORE_ROOT=… DATABASE_URL=sqlite:… store-import <SOURCE_DIR> [MANIFEST_TSV]`
//!
//! Running it twice is safe: an existing object is skipped.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::ItemKey;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use walkdir::WalkDir;

use music_api::store::{self, ImportMeta, Store};

const AUDIO_EXTS: &[&str] = &[
    "wav", "m4a", "flac", "mp3", "aif", "aiff", "ogg", "wma", "alac", "dsf",
];

/// Stem parts are suffixed `_vocals`, `_bass`, … on both WAV parts and their set.
const STEM_PARTS: &[&str] = &["vocals", "bass", "drums", "instrumental", "other"];

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
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

/// Canonical form: the file with MMM's Comment tag cleared. WAV has no MMM
/// comment (stems are source files), so its raw bytes are used as-is.
fn canonicalise(src: &Path, dest: &Path) -> Result<String> {
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

fn content_type(ext: &str) -> &'static str {
    match ext {
        "flac" => "audio/flac",
        "m4a" | "alac" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// The track a stem part belongs to: the parent directory when nested, otherwise
/// the file name with the stem `_part` and extension stripped.
fn classify(rel: &Path) -> (String, Option<String>) {
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
            let base = file_name.trim_end_matches(".stem.m4a");
            // Drop the extension for flat files (`Artist - Title.flac` → title).
            let base = Path::new(base)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(base);
            base.to_string()
        }
    };
    (group_key, stem_type)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let source = PathBuf::from(
        args.next()
            .context("usage: store-import <SOURCE_DIR> [MANIFEST_TSV]")?,
    );
    let manifest_path = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "store-import.tsv".to_string()),
    );
    if !source.is_dir() {
        anyhow::bail!("{} is not a directory", source.display());
    }

    let data_dir = PathBuf::from(
        std::env::var("DATA_DIR").unwrap_or_else(|_| "/opt/music-api/data".to_string()),
    );
    let store_root = PathBuf::from(
        std::env::var("STORE_ROOT")
            .unwrap_or_else(|_| data_dir.join("objects").display().to_string()),
    );
    let db_path: PathBuf = match std::env::var("DATABASE_URL") {
        Ok(url) => PathBuf::from(url.trim_start_matches("sqlite:")),
        Err(_) => data_dir.join("music-api.db"),
    };

    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(10))
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await?;

    let store = Store::new(store_root.clone(), store::DEFAULT_MAX_UPLOAD_BYTES);
    store::init(&pool, &store).await?;

    let tmp_canon = store.tmp_dir().join("import-canon");
    std::fs::create_dir_all(&tmp_canon)?;

    let mut manifest = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&manifest_path)
        .with_context(|| format!("opening manifest {}", manifest_path.display()))?;

    let mut files = 0usize;
    let mut stored = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut bytes_new: u64 = 0;

    for entry in WalkDir::new(&source).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let Some(ext) = ext else { continue };
        if !AUDIO_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let rel = path.strip_prefix(&source).unwrap_or(path);
        files += 1;

        let (group_key, stem_type) = classify(rel);
        let meta = ImportMeta {
            content_type: Some(content_type(&ext).to_string()),
            original_path: Some(rel.to_string_lossy().to_string()),
            isrc: None,
            name: path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string()),
            group_key: Some(group_key.clone()),
            stem_type: stem_type.clone(),
        };

        // WAV: raw bytes. Everything else: canonicalise (clear the Comment tag).
        let hash_result = if ext == "wav" {
            sha256_file(path).map(|h| (h, None))
        } else {
            let dest = tmp_canon.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
            canonicalise(path, &dest).map(|h| (h, Some(dest)))
        };

        let (hash, canon_tmp) = match hash_result {
            Ok(v) => v,
            Err(e) => {
                eprintln!("FAIL {}: {e:#}", rel.display());
                failed += 1;
                continue;
            }
        };

        let obj = store.path_for(&hash);
        let src_for_import = canon_tmp.as_deref().unwrap_or(path);
        match store::import_file(&pool, &store, &hash, src_for_import, &meta).await {
            Ok(true) => {
                if let Ok(m) = std::fs::metadata(&obj) {
                    bytes_new += m.len();
                }
                stored += 1;
                println!("NEW  {}  {}", &hash[..12], rel.display());
            }
            Ok(false) => {
                skipped += 1;
                // import_file left the source in place when the object existed.
                if let Some(tmp) = &canon_tmp {
                    let _ = std::fs::remove_file(tmp);
                }
            }
            Err(e) => {
                eprintln!("FAIL {}: {e:#}", rel.display());
                if let Some(tmp) = &canon_tmp {
                    let _ = std::fs::remove_file(tmp);
                }
                failed += 1;
                continue;
            }
        }

        // The staging copy is no longer needed: WAVs were moved into place (or
        // the object already existed), canonical temps were moved or removed.
        // Deleting a path that was already renamed away is a harmless no-op.
        let _ = std::fs::remove_file(path);

        let size = std::fs::metadata(&obj).map(|m| m.len()).unwrap_or(0);
        writeln!(
            manifest,
            "{}\t{}\t{}\t{}\t{}",
            rel.to_string_lossy(),
            hash,
            size,
            group_key,
            stem_type.unwrap_or_default()
        )?;

        if files % 200 == 0 {
            manifest.flush().ok();
            println!(
                "… {files} scanned, {stored} new, {skipped} present, {failed} failed, {:.1} GiB new",
                bytes_new as f64 / 1073741824.0
            );
        }
    }

    manifest.flush().ok();
    println!(
        "done: {files} files, {stored} new, {skipped} present, {failed} failed, {:.1} GiB new; manifest {}",
        bytes_new as f64 / 1073741824.0,
        manifest_path.display()
    );
    Ok(())
}
