//! yt-dlp integration for fetching public playlists and tracks from YouTube and SoundCloud.
//! Uses yt-dlp subprocess with --dump-json --flat-playlist to extract metadata without downloading.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;
use tokio::process::Command as TokioCommand;

/// Track metadata as returned by yt-dlp --dump-json.
#[derive(Debug, Deserialize)]
pub struct YtDlpTrack {
    pub id: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<f64>,
    pub webpage_url: String,
    pub extractor: String, // "youtube" or "soundcloud"
    pub extractor_key: String,
    pub playlist: Option<String>,
    pub playlist_id: Option<String>,
    pub playlist_title: Option<String>,
    pub playlist_uploader: Option<String>,
    // SoundCloud specific
    pub isrc: Option<String>,
    pub genre: Option<String>,
    pub bpm: Option<f64>,
    #[serde(rename = "_type")]
    pub entry_type: Option<String>,
}

impl YtDlpTrack {
    pub fn service(&self) -> &'static str {
        match self.extractor.as_str() {
            "youtube" => "youtube",
            "soundcloud" => "soundcloud",
            _ => "unknown",
        }
    }

    pub fn service_track_id(&self) -> &str {
        &self.id
    }

    pub fn artists(&self) -> String {
        self.artist
            .clone()
            .unwrap_or_else(|| self.uploader.clone().unwrap_or_default())
    }

    pub fn duration_ms(&self) -> Option<i64> {
        self.duration.map(|d| (d * 1000.0) as i64)
    }

    /// Returns true if this entry is a playlist (not a track).
    pub fn is_playlist(&self) -> bool {
        self.entry_type.as_deref() == Some("url")
            && (self.extractor_key == "YoutubeTab" || self.extractor_key == "SoundcloudTab")
    }

    /// If this is a playlist entry, returns its playlist ID (the `id` field).
    pub fn playlist_id(&self) -> Option<&str> {
        if self.is_playlist() {
            Some(&self.id)
        } else {
            None
        }
    }
}

/// Fetch all entries (playlists or tracks) from a URL using yt-dlp.
async fn fetch_entries(url: &str) -> Result<Vec<YtDlpTrack>> {
    let output = TokioCommand::new("yt-dlp")
        .arg("--dump-json")
        .arg("--flat-playlist")
        .arg("--no-warnings")
        .arg("--ignore-errors")
        .arg(url)
        .output()
        .await
        .context("failed to execute yt-dlp")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: YtDlpTrack = serde_json::from_str(line)
            .with_context(|| format!("failed to parse yt-dlp JSON line: {}", line))?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Fetch playlist entries (not individual tracks) from a channel or user page.
/// Returns a list of playlist entries.
pub async fn fetch_user_playlists(url: &str) -> Result<Vec<YtDlpTrack>> {
    let entries = fetch_entries(url).await?;
    let playlists: Vec<_> = entries.into_iter().filter(|e| e.is_playlist()).collect();
    Ok(playlists)
}

/// Fetch tracks from a playlist URL (ignoring playlist entries).
pub async fn fetch_playlist_tracks(url: &str) -> Result<Vec<YtDlpTrack>> {
    let entries = fetch_entries(url).await?;
    let tracks: Vec<_> = entries.into_iter().filter(|e| !e.is_playlist()).collect();
    Ok(tracks)
}

/// Check if yt-dlp is available and works.
pub async fn check_ytdlp() -> Result<()> {
    let output = Command::new("yt-dlp")
        .arg("--version")
        .output()
        .context("failed to run yt-dlp --version")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!("yt-dlp check failed");
    }
}
