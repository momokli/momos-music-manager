//! Runtime configuration, read from the environment (dotenv-friendly).

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// Bearer token the API requires on every non-public route.
    pub token: String,
    /// Address the HTTP server binds to.
    pub bind: String,
    /// Base URL of the deemix instance that performs the downloads.
    pub deemix_url: String,
    /// Deezer ARL used to authenticate with deemix.
    pub deemix_arl: String,
    /// Requested deemix bitrate (1 = 128, 3 = 320, 9 = FLAC).
    pub deemix_bitrate: u8,
    /// Directory deemix writes its downloads into (as seen by this process).
    pub deemix_download_dir: PathBuf,
    /// Our own store: `flac/`, `320/`, `128/` live below this.
    pub data_dir: PathBuf,
    /// Deezer public API base (no auth required for ISRC lookups).
    pub deezer_base: String,
    /// ffmpeg binary used for the 320/128 transcodes.
    pub ffmpeg: String,
    /// ffprobe binary used to classify the delivered lossy file.
    pub ffprobe: String,
    /// How often the worker advances the pipeline.
    pub worker_interval: Duration,
    /// How long to wait for a single deemix download before giving up.
    pub download_timeout: Duration,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("MUSIC_API_TOKEN")
            .map_err(|_| anyhow::anyhow!("MUSIC_API_TOKEN is required"))?;
        if token.trim().is_empty() {
            anyhow::bail!("MUSIC_API_TOKEN must not be empty");
        }

        let data_dir = PathBuf::from(
            std::env::var("DATA_DIR").unwrap_or_else(|_| "/opt/music-api/data".to_string()),
        );

        Ok(Self {
            token,
            bind: std::env::var("MUSIC_API_BIND").unwrap_or_else(|_| "0.0.0.0:8710".to_string()),
            deemix_url: std::env::var("DEEMIX_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:6595".to_string())
                .trim_end_matches('/')
                .to_string(),
            deemix_arl: std::env::var("DEEMIX_ARL").unwrap_or_default(),
            deemix_bitrate: env_parse("DEEMIX_BITRATE", 9u8),
            deemix_download_dir: PathBuf::from(
                std::env::var("DEEMIX_DOWNLOAD_DIR")
                    .unwrap_or_else(|_| data_dir.join("incoming").display().to_string()),
            ),
            data_dir,
            deezer_base: std::env::var("DEEZER_BASE")
                .unwrap_or_else(|_| "https://api.deezer.com".to_string())
                .trim_end_matches('/')
                .to_string(),
            ffmpeg: std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string()),
            ffprobe: std::env::var("FFPROBE").unwrap_or_else(|_| "ffprobe".to_string()),
            worker_interval: Duration::from_secs(env_parse("WORKER_INTERVAL_SECS", 5u64)),
            download_timeout: Duration::from_secs(env_parse("DOWNLOAD_TIMEOUT_SECS", 900u64)),
        })
    }

    pub fn flac_dir(&self) -> PathBuf {
        self.data_dir.join("flac")
    }
    pub fn mp3_320_dir(&self) -> PathBuf {
        self.data_dir.join("320")
    }
    pub fn mp3_128_dir(&self) -> PathBuf {
        self.data_dir.join("128")
    }
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .unwrap_or(default)
}
