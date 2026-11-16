//! Runtime configuration, read from the environment (dotenv-friendly).

use std::path::PathBuf;
use std::time::Duration;

use crate::deezer_dl::Quality;

/// The single legacy ARL variable name still honoured (#518).
///
/// The constant is the one deliberate exception to the "no sidecar brand in
/// `src`" acceptance: the old name must appear somewhere to be read. Every
/// other occurrence is gone.
const LEGACY_ARL_ENV: &str = "DEEMIX_ARL"; // #518 legacy-env-alias — bewusste Ausnahme zur Grep-Akzeptanz

#[derive(Debug, Clone)]
pub struct Config {
    /// Bearer token the API requires on every non-public route.
    pub token: String,
    /// Address the HTTP server binds to.
    pub bind: String,
    /// Deezer ARL used to authenticate native downloads (never logged).
    pub deezer_arl: String,
    /// Requested download quality (`DOWNLOAD_QUALITY`): `flac` | `mp3-320` |
    /// `mp3-128`. Default `flac`.
    pub download_quality: Quality,
    /// Our own store: `flac/`, `320/`, `128/` live below this.
    pub data_dir: PathBuf,
    /// Root of the content-addressed object store.
    pub store_root: PathBuf,
    /// Maximum size of a single uploaded object.
    pub store_max_upload_bytes: u64,
    /// Deezer public API base (no auth required for ISRC lookups).
    pub deezer_base: String,
    /// Deezer gw-light endpoint used to probe the ARL's account tier
    /// (`deezer.getUserData`).
    pub deezer_gw_base: String,
    /// ffmpeg binary used for the 320/128 transcodes.
    pub ffmpeg: String,
    /// ffprobe binary used to classify the delivered lossy file.
    pub ffprobe: String,
    /// yt-dlp binary, used to acquire non-Deezer sources (e.g. SoundCloud, YouTube).
    pub ytdlp: String,
    /// Scratch dir yt-dlp downloads into (below `data_dir` by default).
    pub ytdlp_dir: PathBuf,
    /// How many yt-dlp downloads may run concurrently.
    pub ytdlp_concurrency: usize,
    /// How many background `finalize` (copy + transcode) jobs may run
    /// concurrently.
    pub transcode_concurrency: usize,
    /// How often the worker advances the pipeline.
    pub worker_interval: Duration,
    /// How long to wait for a single native download before giving up.
    pub download_timeout: Duration,
    /// Bounded retries: a transient failure becomes terminal after this many attempts.
    pub queue_max_attempts: u32,
    /// Base delay for the exponential retry backoff (seconds).
    pub queue_retry_base_secs: i64,
    /// Upper bound on the retry backoff (seconds).
    pub queue_retry_max_secs: i64,
    /// Maximum number of downloads dispatched at once. Orders are still
    /// accepted without limit; only the *dispatch* is bounded, so at most this
    /// many rows sit in `downloading`. Priority decides *which* pending rows
    /// fill the free slots.
    pub max_inflight: usize,
    /// Base URL of the BPM/key analyzer service (Essentia), e.g.
    /// `http://127.0.0.1:8711`. `POST {url}/analyze` with a server-local file
    /// path. `None` (unset or blank) disables analysis entirely.
    pub analyzer_url: Option<String>,
    /// How many analyzer calls may run concurrently (analysis is CPU-heavy).
    pub analysis_concurrency: usize,
    /// Analyzer precedence for the vault resolver: the first analyzer in this
    /// list that has a value wins, per field. Default `traktor,essentia,reccobeats`.
    pub analysis_precedence: Vec<String>,
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
        let store_root = PathBuf::from(
            std::env::var("STORE_ROOT")
                .unwrap_or_else(|_| data_dir.join("objects").display().to_string()),
        );
        let ytdlp_dir = PathBuf::from(
            std::env::var("YTDLP_DIR")
                .unwrap_or_else(|_| data_dir.join("ytdlp").display().to_string()),
        );

        let download_quality = parse_download_quality(
            &std::env::var("DOWNLOAD_QUALITY").unwrap_or_else(|_| "flac".to_string()),
        )?;

        let config = Self {
            token,
            bind: std::env::var("MUSIC_API_BIND").unwrap_or_else(|_| "0.0.0.0:8710".to_string()),
            deezer_arl: arl_from_env(),
            download_quality,
            data_dir,
            store_root,
            store_max_upload_bytes: env_parse(
                "STORE_MAX_UPLOAD_BYTES",
                crate::store::DEFAULT_MAX_UPLOAD_BYTES,
            ),
            deezer_base: std::env::var("DEEZER_BASE")
                .unwrap_or_else(|_| "https://api.deezer.com".to_string())
                .trim_end_matches('/')
                .to_string(),
            deezer_gw_base: std::env::var("DEEZER_GW_BASE")
                .unwrap_or_else(|_| "https://www.deezer.com/ajax/gw-light.php".to_string())
                .trim_end_matches('/')
                .to_string(),
            ffmpeg: std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string()),
            ffprobe: std::env::var("FFPROBE").unwrap_or_else(|_| "ffprobe".to_string()),
            ytdlp: std::env::var("YTDLP_BIN").unwrap_or_else(|_| "yt-dlp".to_string()),
            ytdlp_dir,
            ytdlp_concurrency: env_parse("YTDLP_CONCURRENCY", 2usize),
            transcode_concurrency: env_parse("TRANSCODE_CONCURRENCY", 4usize),
            worker_interval: Duration::from_secs(env_parse("WORKER_INTERVAL_SECS", 5u64)),
            download_timeout: Duration::from_secs(env_parse("DOWNLOAD_TIMEOUT_SECS", 900u64)),
            queue_max_attempts: env_parse("QUEUE_MAX_ATTEMPTS", 3u32).max(1),
            queue_retry_base_secs: env_parse("QUEUE_RETRY_BASE_SECS", 60i64).max(1),
            queue_retry_max_secs: env_parse("QUEUE_RETRY_MAX_SECS", 1800i64).max(1),
            max_inflight: env_parse("MAX_INFLIGHT", 5usize).max(1),
            analyzer_url: analyzer_url_from_env(),
            analysis_concurrency: env_parse("ANALYSIS_CONCURRENCY", 1usize).max(1),
            analysis_precedence: env_list(
                "ANALYSIS_PRECEDENCE",
                &["traktor", "essentia", "reccobeats"],
            ),
        };

        Ok(config)
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

/// Resolve the ARL from the new (`DEEZER_ARL`) and legacy names. The new name
/// wins; the legacy name is the fallback. Pure, so it is testable without
/// mutating global process env.
fn resolve_arl(new: &str, legacy: &str) -> String {
    let new = new.trim();
    if !new.is_empty() {
        new.to_string()
    } else {
        legacy.trim().to_string()
    }
}

/// Read the ARL from the environment, warning once (variable name only, never
/// the value) when only the legacy name is set.
fn arl_from_env() -> String {
    let deezer = std::env::var("DEEZER_ARL").unwrap_or_default();
    let legacy = std::env::var(LEGACY_ARL_ENV).unwrap_or_default();
    if deezer.trim().is_empty() && !legacy.trim().is_empty() {
        tracing::warn!("{LEGACY_ARL_ENV} is deprecated — use DEEZER_ARL instead");
    }
    resolve_arl(&deezer, &legacy)
}

/// Parse `DOWNLOAD_QUALITY` (`flac` | `mp3-320` | `mp3-128`, case-insensitive).
/// An unknown value is a hard error: a typo must not silently pick a default.
fn parse_download_quality(raw: &str) -> anyhow::Result<Quality> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "flac" => Ok(Quality::Flac),
        "mp3-320" => Ok(Quality::Mp3_320),
        "mp3-128" => Ok(Quality::Mp3_128),
        other => anyhow::bail!(
            "invalid DOWNLOAD_QUALITY '{other}' — expected flac, mp3-320 or mp3-128"
        ),
    }
}

/// The analyzer base URL: `ANALYZER_URL` wins, `HUB_ANALYZER_URL` is the
/// fallback. A blank/whitespace value is treated as "not configured" (analysis
/// disabled).
fn analyzer_url_from_env() -> Option<String> {
    std::env::var("ANALYZER_URL")
        .or_else(|_| std::env::var("HUB_ANALYZER_URL"))
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .unwrap_or(default)
}

/// A comma-separated list from the environment (lowercased, trimmed, blanks
/// dropped). Falls back to `default` when unset or blank.
fn env_list(key: &str, default: &[&str]) -> Vec<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => default.iter().map(|s| s.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_quality_parses_case_insensitively() {
        assert_eq!(parse_download_quality("flac").unwrap(), Quality::Flac);
        assert_eq!(parse_download_quality("MP3-320").unwrap(), Quality::Mp3_320);
        assert_eq!(
            parse_download_quality("  mp3-128 ").unwrap(),
            Quality::Mp3_128
        );
    }

    #[test]
    fn download_quality_rejects_unknown_values() {
        assert!(parse_download_quality("wav").is_err());
        assert!(parse_download_quality("320").is_err());
        assert!(parse_download_quality("").is_err());
    }

    #[test]
    fn arl_alias_prefers_the_new_name() {
        // The new name wins when both are present.
        assert_eq!(resolve_arl("new-arl", "legacy-arl"), "new-arl");
        // Only the legacy name set → fallback to it.
        assert_eq!(resolve_arl("", "legacy-arl"), "legacy-arl");
        assert_eq!(resolve_arl("   ", "legacy-arl"), "legacy-arl");
        // Neither set → empty.
        assert_eq!(resolve_arl("", ""), "");
        assert_eq!(resolve_arl("  ", "  "), "");
    }
}
