//! ffmpeg transcoding: derive 320/128 kbps MP3 from the delivered source.

use std::path::Path;

use anyhow::Context;

/// Transcode `src` to an MP3 at `kbps` into `dst`, creating parent dirs.
pub async fn to_mp3(ffmpeg: &str, src: &Path, dst: &Path, kbps: u32) -> anyhow::Result<()> {
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let output = tokio::process::Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(src)
        .arg("-vn")
        .arg("-b:a")
        .arg(format!("{kbps}k"))
        .arg(dst)
        .output()
        .await
        .with_context(|| format!("failed to run {ffmpeg}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "ffmpeg failed for {} ({}): {}",
            src.display(),
            output.status,
            stderr.trim()
        );
    }
    Ok(())
}

/// Probe the audio bit rate (bits/s) of a file. `None` when the container does
/// not report one — callers must decide on a fallback.
///
/// deemix's bitrate fallback can land on any tier, so a delivered MP3 is only
/// labelled by what it actually is, not by what was requested.
pub async fn probe_bitrate(ffprobe: &str, src: &Path) -> Option<u64> {
    let output = tokio::process::Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=bit_rate")
        .arg("-of")
        .arg("default=nw=1:nk=1")
        .arg(src)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}
