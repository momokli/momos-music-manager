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

/// Decode the first audio stream to 16-bit little-endian PCM and return its
/// MD5 (`ffmpeg -f md5`). Two files with identical audio yield the same digest,
/// files with different audio do not — the evidence used by `archive-legacy`
/// before it relocates a duplicate.
///
/// `ffprobe` cannot do this: `-f md5` fails on `-map` ("Option not found") and
/// `-show_entries stream=md5` is empty for FLAC. `None` when the file is
/// missing or ffmpeg cannot decode it (caller keeps the file and reports a skip).
pub async fn pcm_md5(ffmpeg: &str, src: &Path) -> Option<String> {
    let output = tokio::process::Command::new(ffmpeg)
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(src)
        .arg("-map")
        .arg("0:a:0")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg("-f")
        .arg("md5")
        .arg("-")
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|l| l.trim().strip_prefix("MD5="))
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
}

/// Probe the audio bit rate (bits/s) of a file. `None` when the container does
/// not report one — callers must decide on a fallback.
///
/// The tier fallback can land on any quality, so a delivered MP3 is only
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

#[cfg(test)]
mod tests {
    use super::*;

    /// ffmpeg-gated: two identical decodes share a digest, different audio does
    /// not. Skipped (with a notice) when ffmpeg is unavailable.
    #[tokio::test]
    async fn pcm_md5_is_stable_and_discriminating() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.flac");
        let b = tmp.path().join("b.flac");
        let c = tmp.path().join("c.flac");
        let gen = |freq: &str, path: &std::path::Path| {
            std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", &format!("sine=frequency={freq}:duration=0.5"), "-c:a", "flac"])
                .arg(path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !gen("440", &a) {
            eprintln!("skipping: ffmpeg not available");
            return;
        }
        std::fs::copy(&a, &b).unwrap();
        gen("880", &c);
        let ma = pcm_md5("ffmpeg", &a).await.expect("md5 a");
        let mb = pcm_md5("ffmpeg", &b).await.expect("md5 b");
        let mc = pcm_md5("ffmpeg", &c).await.expect("md5 c");
        assert_eq!(ma, mb, "identical audio must share the PCM MD5");
        assert_ne!(ma, mc, "different audio must not share the PCM MD5");
        assert!(pcm_md5("ffmpeg", &tmp.path().join("missing.flac"))
            .await
            .is_none());
    }
}
