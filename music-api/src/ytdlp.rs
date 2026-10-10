//! yt-dlp integration: metadata + downloads for YouTube and SoundCloud.
//!
//! yt-dlp is invoked as a subprocess. Metadata comes from `--dump-json`
//! (optionally `--flat-playlist` for playlists); downloads use
//! `--extract-audio --audio-format <fmt>`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::process::Command;

use crate::models::{MetadataPlaylist, MetadataTrack};

/// One entry as emitted by `yt-dlp --dump-json`.
#[derive(Debug, Deserialize)]
pub struct YtDlpEntry {
    pub id: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub webpage_url: String,
    pub extractor: String,
    pub extractor_key: String,
    pub isrc: Option<String>,
    pub thumbnail: Option<String>,
    #[serde(rename = "_type")]
    pub entry_type: Option<String>,
}

impl YtDlpEntry {
    pub fn provider(&self) -> &'static str {
        match self.extractor.as_str() {
            "youtube" => "youtube",
            "soundcloud" => "soundcloud",
            _ => "unknown",
        }
    }

    pub fn artist_name(&self) -> Option<String> {
        self.artist.clone().or_else(|| self.uploader.clone())
    }

    pub fn duration_ms(&self) -> Option<i64> {
        self.duration.map(|d| (d * 1000.0) as i64)
    }

    pub fn is_playlist(&self) -> bool {
        self.entry_type.as_deref() == Some("url")
            && (self.extractor_key == "YoutubeTab" || self.extractor_key == "SoundcloudTab")
    }

    pub fn into_metadata(self) -> MetadataTrack {
        let provider = self.provider().to_string();
        let artist = self.artist_name();
        let duration_ms = self.duration_ms();
        MetadataTrack {
            provider,
            provider_id: self.id,
            title: self.title,
            artist,
            album: self.album,
            duration_ms,
            url: self.webpage_url,
            isrc: self.isrc,
            artwork: self.thumbnail,
        }
    }
}

/// Classify a URL by provider. Returns `"youtube"` or `"soundcloud"`.
pub fn provider_for_url(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    if lower.contains("youtube.com")
        || lower.contains("youtu.be")
        || lower.contains("music.youtube.com")
    {
        "youtube"
    } else if lower.contains("soundcloud.com") {
        "soundcloud"
    } else {
        "unknown"
    }
}

fn ytdlp_bin() -> String {
    std::env::var("YTDLP").unwrap_or_else(|_| "yt-dlp".to_string())
}

/// Run yt-dlp with the given args and return the parsed JSON entries.
async fn dump_json(args: &[&str]) -> Result<Vec<YtDlpEntry>> {
    let bin = ytdlp_bin();
    let mut cmd = Command::new(&bin);
    cmd.arg("--dump-json")
        .arg("--no-warnings")
        .arg("--ignore-errors");
    if let Ok(cookies) = std::env::var("YTDLP_COOKIES") {
        if !cookies.is_empty() {
            cmd.arg("--cookies").arg(cookies);
        }
    }
    cmd.args(args);

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute {bin}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<YtDlpEntry>(line) {
            Ok(e) => entries.push(e),
            Err(e) => tracing::warn!("skipping unparseable yt-dlp line: {e}"),
        }
    }
    Ok(entries)
}

/// Fetch metadata for a single track URL.
pub async fn fetch_track(url: &str) -> Result<MetadataTrack> {
    let entries = dump_json(&["--no-playlist", url]).await?;
    let entry = entries
        .into_iter()
        .next()
        .context("yt-dlp returned no entry for track URL")?;
    Ok(entry.into_metadata())
}

/// Fetch metadata for a playlist URL (flat, no per-track network calls).
pub async fn fetch_playlist(url: &str) -> Result<MetadataPlaylist> {
    let entries = dump_json(&["--flat-playlist", url]).await?;

    // A channel/user page yields playlist entries; a playlist yields tracks.
    let playlist_entries: Vec<_> = entries.iter().filter(|e| e.is_playlist()).collect();
    let provider = provider_for_url(url).to_string();

    if !playlist_entries.is_empty() {
        // Channel page: return the first playlist's tracks? For now, flatten all
        // playlist entries as tracks is wrong — instead report the playlists.
        // Callers that want a single playlist should pass a playlist URL.
        let tracks = entries
            .into_iter()
            .filter(|e| !e.is_playlist())
            .map(|e| e.into_metadata())
            .collect::<Vec<_>>();
        return Ok(MetadataPlaylist {
            provider,
            provider_id: url.to_string(),
            name: None,
            track_count: tracks.len(),
            url: url.to_string(),
            tracks,
        });
    }

    let name = entries.first().and_then(|e| e.title.clone());
    let tracks: Vec<MetadataTrack> = entries.into_iter().map(|e| e.into_metadata()).collect();
    Ok(MetadataPlaylist {
        provider,
        provider_id: url.to_string(),
        name,
        track_count: tracks.len(),
        url: url.to_string(),
        tracks,
    })
}

/// Download a URL as audio into `out_dir`. Returns the path of the written file.
///
/// `format` is the target audio format (`flac`, `mp3`, ...). yt-dlp picks the
/// best available source and converts.
pub async fn download(url: &str, out_dir: &Path, format: &str) -> Result<PathBuf> {
    tokio::fs::create_dir_all(out_dir).await?;

    let bin = ytdlp_bin();
    let template = out_dir.join("%(id)s.%(ext)s");
    let mut cmd = Command::new(&bin);
    cmd.arg("--no-playlist")
        .arg("--no-warnings")
        .arg("-f")
        .arg("bestaudio/best")
        .arg("--extract-audio")
        .arg("--audio-format")
        .arg(format)
        .arg("-o")
        .arg(&template);
    if let Ok(cookies) = std::env::var("YTDLP_COOKIES") {
        if !cookies.is_empty() {
            cmd.arg("--cookies").arg(cookies);
        }
    }
    cmd.arg(url);

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute {bin}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} download failed: {stderr}");
    }

    // yt-dlp prints the final path on stdout with --print after_move, but we
    // didn't ask for it. Find the newest audio file in out_dir instead.
    find_newest_audio(out_dir).context("yt-dlp reported success but no audio file was found")
}

fn find_newest_audio(dir: &Path) -> Option<PathBuf> {
    const EXTS: &[&str] = &["flac", "mp3", "m4a", "opus", "ogg", "wav"];
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_classification() {
        assert_eq!(
            provider_for_url("https://www.youtube.com/watch?v=x"),
            "youtube"
        );
        assert_eq!(provider_for_url("https://youtu.be/x"), "youtube");
        assert_eq!(
            provider_for_url("https://music.youtube.com/watch?v=x"),
            "youtube"
        );
        assert_eq!(provider_for_url("https://soundcloud.com/a/b"), "soundcloud");
        assert_eq!(provider_for_url("https://example.com/x"), "unknown");
    }

    #[test]
    fn parses_entry() {
        let json = r#"{"id":"abc","title":"T","uploader":"U","duration":12.5,"webpage_url":"https://x","extractor":"youtube","extractor_key":"Youtube","_type":"url"}"#;
        let e: YtDlpEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.provider(), "youtube");
        assert_eq!(e.duration_ms(), Some(12500));
        assert_eq!(e.artist_name().as_deref(), Some("U"));
    }
}
