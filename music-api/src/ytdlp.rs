//! yt-dlp acquisition for non-Deezer sources (SoundCloud, YouTube).
//!
//! Deezer tracks go through deemix; sources that give us only an external id
//! (a SoundCloud numeric track id or a YouTube video id) are downloaded here.
//! yt-dlp resolves the SoundCloud numeric id via the
//! `api.soundcloud.com/tracks/<id>` endpoint and the YouTube video id via the
//! canonical `youtube.com/watch?v=<id>` URL.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Canonical yt-dlp URL for a numeric SoundCloud track id.
pub fn soundcloud_url(id: &str) -> String {
    format!("https://api.soundcloud.com/tracks/{}", id.trim())
}

/// Canonical yt-dlp URL for a YouTube video id.
pub fn youtube_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={}", id.trim())
}

/// Download the best audio stream for `url` into `<base>/<stem>/audio.*` and
/// return the written file.
pub async fn download_audio(bin: &str, url: &str, base: &Path, stem: &str) -> Result<PathBuf> {
    let dir = base.join(stem);
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await?;
    let out = dir.join("audio.%(ext)s");

    let output = tokio::process::Command::new(bin)
        .arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--restrict-filenames")
        .arg("-f")
        .arg("bestaudio/best")
        .arg("-o")
        .arg(&out)
        .arg(url)
        .output()
        .await
        .with_context(|| format!("run {bin}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp failed: {}", err.trim());
    }
    find_output(&dir).ok_or_else(|| anyhow::anyhow!("yt-dlp produced no file in {}", dir.display()))
}

fn find_output(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_file() && name.starts_with("audio.") {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{soundcloud_url, youtube_url};

    #[test]
    fn builds_soundcloud_api_url() {
        assert_eq!(
            soundcloud_url("1837565340"),
            "https://api.soundcloud.com/tracks/1837565340"
        );
        assert_eq!(
            soundcloud_url(" 42 "),
            "https://api.soundcloud.com/tracks/42"
        );
    }

    #[test]
    fn builds_youtube_watch_url() {
        assert_eq!(
            youtube_url("dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            youtube_url(" dQw4w9WgXcQ "),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }
}
