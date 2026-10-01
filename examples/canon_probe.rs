//! Throwaway probe for Phase 1 of the remote-store plan: is "clear the Comment
//! tag and re-save" a *deterministic* canonicalisation?
//!
//! Run: `cargo run --example canon_probe -- <file> [<file>...]`
//!
//! It answers three questions per file:
//!   1. Is the canonical form byte-stable across two independent saves?
//!   2. Does it differ from the raw bytes (i.e. did it actually do something)?
//!   3. Do two files that differ only in their Comment canonicalise to the same
//!      object (the whole point of canonicalising for dedup)?

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::ItemKey;
use sha2::{Digest, Sha256};

fn hex(d: impl AsRef<[u8]>) -> String {
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_file(p: &Path) -> Result<String> {
    Ok(hex(Sha256::digest(std::fs::read(p)?)))
}

fn ext(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default()
}

/// Copy `src` to `dest`, then clear the Comment tag **in place**.
///
/// `save_to_path` on a *different* path fails for FLAC (lofty rewrites the file
/// in place), so the object is materialised as a copy first — which is also what
/// MMM would do when building the canonical object.
fn canonicalise(src: &Path, dest: &Path) -> Result<()> {
    std::fs::copy(src, dest)
        .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
    let mut tagged = lofty::read_from_path(dest)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("lofty read {}", dest.display()))?;
    if let Some(tag) = tagged.primary_tag_mut() {
        tag.remove_key(&ItemKey::Comment);
    }
    tagged
        .save_to_path(dest, WriteOptions::default())
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("lofty save {}", dest.display()))?;
    Ok(())
}

/// Copy `src` to `dest`, replacing its Comment with `comment`.
fn with_comment(src: &Path, dest: &Path, comment: &str) -> Result<()> {
    std::fs::copy(src, dest)?;
    let mut tagged = lofty::read_from_path(dest).map_err(|e| anyhow::anyhow!("{e}"))?;
    let tag_type = tagged.primary_tag_type();
    if let Some(tag) = tagged.primary_tag_mut() {
        tag.insert_text(ItemKey::Comment, comment.to_string());
    } else {
        let mut tag = lofty::tag::Tag::new(tag_type);
        tag.insert_text(ItemKey::Comment, comment.to_string());
        tagged.insert_tag(tag);
    }
    tagged.save_to_path(dest, WriteOptions::default())?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: canon_probe <file>...");
        std::process::exit(2);
    }

    let tmp = std::env::temp_dir().join(format!("canon-probe-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;

    for (i, arg) in args.iter().enumerate() {
        let src = PathBuf::from(arg);
        let raw = sha256_file(&src)?;

        let c1 = tmp.join(format!("{i}-a{}", ext(&src)));
        let c2 = tmp.join(format!("{i}-b{}", ext(&src)));
        canonicalise(&src, &c1)?;
        canonicalise(&src, &c2)?;
        let h1 = sha256_file(&c1)?;
        let h2 = sha256_file(&c2)?;

        println!("{}", src.display());
        println!("  raw        {}", &raw[..16]);
        println!("  canon#1    {}", &h1[..16]);
        println!("  canon#2    {}", &h2[..16]);
        println!(
            "  stable     {}",
            if h1 == h2 {
                "YES".to_string()
            } else {
                format!(
                    "NO  <-- NON-DETERMINISTIC ({} vs {})",
                    &h1[..12],
                    &h2[..12]
                )
            }
        );

        let raw_len = std::fs::metadata(&src)?.len();
        let canon_len = std::fs::metadata(&c1)?.len();
        println!("  size       raw {raw_len} -> canon {canon_len}");

        // Same audio, different comment → must be one object.
        let tagged_copy = tmp.join(format!("{i}-c{}", ext(&src)));
        with_comment(&src, &tagged_copy, "PROBE-DIFFERENT-COMMENT")?;
        let c3 = tmp.join(format!("{i}-d{}", ext(&src)));
        canonicalise(&tagged_copy, &c3)?;
        let h3 = sha256_file(&c3)?;
        println!(
            "  comment-independent  {}",
            if h3 == h1 { "YES" } else { "NO" }
        );
        println!();
    }
    Ok(())
}
