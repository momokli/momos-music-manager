//! Configuration and persisted device identity for kellner.
//!
//! `~/.config/kellner/config.toml` holds **only** the hub-host override and the
//! device identity written after a successful `pair` (ADR-081 §10). No feature
//! config lives on the device — those toggles belong to the hub (#369).
//!
//! The device token is a credential: the file is written `0600` on Unix, and
//! [`DeviceIdentity`] uses a redacting [`std::fmt::Debug`] so a stray
//! `{:?}`/log line can never print it.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

/// Hub base URL used when neither file nor environment provides one.
pub const DEFAULT_HUB_URL: &str = "https://hub.zukkafabrik.de";

/// Parsed `config.toml` (all fields optional; defaults apply when absent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Hub base URL override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_url: Option<String>,
    /// music-api base URL override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music_api_url: Option<String>,
    /// Local music collection directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_dir: Option<PathBuf>,
    /// Explicit Traktor `collection.nml` path (env `KELLNER_TRAKTOR_NML`).
    /// Needed on Linux, which has no default Traktor location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traktor_nml: Option<PathBuf>,
    /// Traktor upload poll interval in seconds (env `KELLNER_TRAKTOR_POLL_SECS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traktor_poll_secs: Option<u64>,
    /// Persisted device identity, set after a successful `pair`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceIdentity>,
}

impl Config {
    /// Effective hub URL: file value, else [`DEFAULT_HUB_URL`].
    pub fn hub_url(&self) -> String {
        self.hub_url
            .clone()
            .unwrap_or_else(|| DEFAULT_HUB_URL.to_string())
    }

    /// Effective music-api URL, if configured.
    #[allow(dead_code)] // consumed by the delegated music-api calls in #364/#365
    pub fn music_api_url(&self) -> Option<String> {
        self.music_api_url.clone()
    }
}

/// The credential kellner presents to the hub **and** (delegated) to music-api.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: i64,
    pub device_name: String,
    pub device_token: String,
    pub paired_at: String,
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("device_token", &"***")
            .field("paired_at", &self.paired_at)
            .finish()
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Resolve the config file path.
///
/// Priority: `KELLNER_CONFIG` → `$XDG_CONFIG_HOME/kellner/config.toml` →
/// `$HOME/.config/kellner/config.toml` (the ADR-literal path wins over the
/// platform convention) → `directories` fallback.
pub fn config_path() -> Result<PathBuf> {
    if let Some(p) = env_nonempty("KELLNER_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    if let Some(xdg) = env_nonempty("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("kellner").join("config.toml"));
    }
    if let Some(home) = env_nonempty("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("kellner").join("config.toml"));
    }
    if let Some(dirs) = directories::ProjectDirs::from("de", "zukkafabrik", "kellner") {
        return Ok(dirs.config_dir().join("config.toml"));
    }
    Err(anyhow!(
        "cannot determine config path (set KELLNER_CONFIG or HOME)"
    ))
}

/// Load the config, applying environment overrides after the file values.
pub fn load() -> Result<Config> {
    let path = config_path()?;
    let mut cfg = if path.exists() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?
    } else {
        Config::default()
    };
    apply_env(&mut cfg);
    Ok(cfg)
}

/// Environment overrides beat file values.
fn apply_env(cfg: &mut Config) {
    if let Some(v) = env_nonempty("KELLNER_HUB_URL") {
        cfg.hub_url = Some(v);
    }
    if let Some(v) = env_nonempty("KELLNER_MUSIC_API_URL") {
        cfg.music_api_url = Some(v);
    }
    if let Some(v) = env_nonempty("KELLNER_COLLECTION_DIR") {
        cfg.collection_dir = Some(PathBuf::from(v));
    }
    if let Some(v) = env_nonempty("KELLNER_TRAKTOR_NML") {
        cfg.traktor_nml = Some(PathBuf::from(v));
    }
    if let Some(v) = env_nonempty("KELLNER_TRAKTOR_POLL_SECS") {
        cfg.traktor_poll_secs = v.trim().parse().ok();
    }
}

/// Persist the config to the resolved path.
pub fn save(cfg: &Config) -> Result<()> {
    save_to(&config_path()?, cfg)
}

/// Persist the config to an explicit path (atomically, `0600` on Unix).
pub fn save_to(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(cfg).context("serialising config")?;

    // Atomic temp + rename; the temp file is created private.
    let tmp = tmp_path(path);
    write_private(&tmp, text.as_bytes())
        .with_context(|| format!("writing config {}", path.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("replacing config {}", path.display()))?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());

    /// Serialise tests that mutate process environment variables.
    pub fn env_lock() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn identity(token: &str) -> DeviceIdentity {
        DeviceIdentity {
            device_id: 42,
            device_name: "test-mac".into(),
            device_token: token.into(),
            paired_at: "2026-10-15T00:00:00Z".into(),
        }
    }

    #[test]
    fn defaults_when_no_file() {
        let _guard = test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/config.toml");
        // SAFETY: single-threaded via env_lock.
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
            std::env::remove_var("KELLNER_HUB_URL");
        }
        let cfg = load().unwrap();
        assert_eq!(cfg.hub_url(), DEFAULT_HUB_URL);
        assert!(cfg.device.is_none());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let _guard = test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kellner/config.toml");
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
            std::env::remove_var("KELLNER_HUB_URL");
        }
        let cfg = Config {
            hub_url: Some("https://hub.example.test".into()),
            music_api_url: None,
            collection_dir: None,
            traktor_nml: None,
            traktor_poll_secs: None,
            device: Some(identity("deadbeef")),
        };
        save(&cfg).unwrap();

        let loaded = load().unwrap();
        assert_eq!(loaded.hub_url(), "https://hub.example.test");
        assert_eq!(loaded.device, Some(identity("deadbeef")));
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kellner/config.toml");
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
        }
        save(&Config {
            device: Some(identity("secret")),
            ..Config::default()
        })
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config must be private, got {mode:o}");
    }

    #[test]
    fn debug_redacts_device_token() {
        let id = identity("super-secret-token-value");
        let dbg = format!("{id:?}");
        assert!(!dbg.contains("super-secret-token-value"));
        assert!(dbg.contains("***"));
    }

    #[test]
    fn env_overrides_beat_file() {
        let _guard = test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "hub_url = \"https://from-file.test\"\n",
        )
        .unwrap();
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
            std::env::set_var("KELLNER_HUB_URL", "https://from-env.test");
        }
        let cfg = load().unwrap();
        assert_eq!(cfg.hub_url(), "https://from-env.test");
        unsafe {
            std::env::remove_var("KELLNER_HUB_URL");
        }
    }

    #[test]
    fn config_path_respects_override() {
        let _guard = test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.toml");
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
        }
        assert_eq!(config_path().unwrap(), path);
    }

    #[test]
    fn hub_url_not_logged_in_errors() {
        let token = "aaaa-bbbb-cccc-secret";
        let dir = tempfile::tempdir().unwrap();
        // A regular file blocks directory creation → save fails.
        let blocker = dir.path().join("blocker");
        std::fs::File::create(&blocker).unwrap().write_all(b"x").unwrap();
        let path = blocker.join("kellner").join("config.toml");

        let cfg = Config {
            device: Some(identity(token)),
            ..Config::default()
        };
        let err = save_to(&path, &cfg).unwrap_err();
        let rendered = format!("{err:#}");
        assert!(!rendered.contains(token), "error leaked token: {rendered}");
    }
}
