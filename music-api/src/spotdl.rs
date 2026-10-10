//! spotDL integration: a Spotify-search → YouTube download fallback.
//!
//! spotDL resolves a free-text query (or Spotify URL) via the Spotify API and
//! downloads the audio from YouTube. It is the second fallback after yt-dlp,
//! used when Deezer has no streamable track and the direct YouTube search
//! misses.
//!
//! Credentials live in spotDL's own config (`~/.config/spotdl/config.json`);
//! this module only shells out to the `spotdl` binary.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

fn spotdl_bin() -> String {
    std::env::var("SPOTDL").unwrap_or_else(|_| "spotdl".to_string())
}

/// Download a track by free-text query (e.g. `"Artist - Title"`) via spotDL.
/// Returns the path of the written audio file.
pub async fn download(query: &str, out_dir: &Path, format: &str) -> Result<PathBuf> {
    tokio::fs::create_dir_all(out_dir).await?;

    let bin = spotdl_bin();
    let template = out_dir.join("{artists} - {title}.{output-ext}");
    let output = Command::new(&bin)
        .arg("download")
        .arg(query)
        .arg("--output")
        .arg(&template)
        .arg("--format")
        .arg(format)
        .arg("--overwrite")
        .arg("force")
        .output()
        .await
        .with_context(|| format!("failed to execute {bin}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} download failed: {stderr}");
    }

    crate::ytdlp::find_newest_audio(out_dir)
        .context("spotDL reported success but no audio file was found")
}
