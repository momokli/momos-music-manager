//! Push one file through MMM's real `StoreClient` (canonicalise → sha256 → PUT
//! → HEAD), against a live store.
//!
//! Usage: `cargo run --example store_push -- <base_url> <token> <file>`
//!
//! Used to verify the store sync's upload path against the deployed `.200`
//! service without waiting for the full library pass.

use std::path::PathBuf;

use anyhow::{Context, Result};

use momos_music_manager::store::{canonicalise_to, StoreClient};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: store_push <base_url> <token> <file>");
        std::process::exit(2);
    }
    let (base_url, token, file) = (&args[0], &args[1], PathBuf::from(&args[2]));

    let tmp = std::env::temp_dir().join(format!("store-push-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    // lofty needs a recognisable extension to determine the container format.
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_string();
    let canonical = tmp.join(format!("canonical.{ext}"));

    let hash = canonicalise_to(&file, &canonical).context("canonicalising")?;
    println!("file        {}", file.display());
    println!("content_hash {hash}");
    println!(
        "canonical   {} bytes (raw {} bytes)",
        std::fs::metadata(&canonical)?.len(),
        std::fs::metadata(&file)?.len()
    );

    let client = StoreClient::new(base_url, token);

    let before = client.head(&hash).await.context("HEAD before")?;
    println!("HEAD before {before}");

    let stored = client
        .put(&hash, &canonical, &file.display().to_string(), None)
        .await
        .context("PUT")?;
    println!("PUT stored  {stored}  (false = already present)");

    let after = client.head(&hash).await.context("HEAD after")?;
    println!("HEAD after  {after}");

    let _ = std::fs::remove_file(&canonical);
    assert!(after, "the object must exist after a successful PUT");
    Ok(())
}
