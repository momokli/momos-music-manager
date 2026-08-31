//! Parsing of the aggregated `SHA256SUMS` manifest published by CI.
//!
//! The manifest contains one `sha256sum`-style line per artifact, including a
//! **stable** name for the current rolling build:
//!
//! ```text
//! <hash>  momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz
//! <hash>  momos-music-manager-latest-linux-x64.tar.gz
//! ```
//!
//! The updater resolves the artifact for the current platform via the
//! **versioned** name (derived from the parsed version) and reads the
//! published version from that same entry.

use std::collections::HashMap;

use semver::Version;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest entry has no hash (line {line})")]
    MissingHash { line: usize },
    #[error("manifest entry has no file name (line {line})")]
    MissingName { line: usize },
    #[error("no artifact found for {name} in manifest")]
    ArtifactNotFound { name: String },
    #[error("no versioned entry found for {os_arch} in manifest")]
    VersionNotFound { os_arch: String },
    #[error("could not parse version from artifact name `{name}`")]
    InvalidVersion { name: String },
}

/// Parsed `SHA256SUMS` manifest (lowercase hex hash → file name).
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// file name → sha256 hex (lowercase)
    entries: HashMap<String, String>,
}

impl Manifest {
    /// Parse `sha256sum`-style text. Lines may have one or two spaces between
    /// hash and name (standard `sha256sum` output: two spaces, one for binary
    /// mode marker). Empty lines and `\r` are tolerated.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let mut entries = HashMap::new();
        for (idx, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let (hash, name) = line
                .split_once(' ')
                .or_else(|| line.split_once('\t'))
                .ok_or(ManifestError::MissingHash { line: idx + 1 })?;
            let hash = hash.trim();
            let name = name.trim();
            if hash.is_empty() {
                return Err(ManifestError::MissingHash { line: idx + 1 });
            }
            if name.is_empty() {
                return Err(ManifestError::MissingName { line: idx + 1 });
            }
            entries.insert(name.to_string(), hash.to_lowercase());
        }
        Ok(Self { entries })
    }

    /// SHA256 (lowercase hex) for an artifact name.
    pub fn artifact_hash(&self, name: &str) -> Option<&str> {
        self.entries.get(name).map(String::as_str)
    }

    /// Whether the manifest contains an entry for `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Number of entries (for diagnostics).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Version published for `os_arch` (e.g. `linux-x64`): find the versioned
    /// entry, strip the platform suffix from the **last** `-<os_arch>.`
    /// occurrence and parse the remaining version string (`1.1.0` or
    /// `1.1.0-dev+abc1234`). The `-latest-` entry fails semver parsing
    /// (`latest`) and is skipped.
    pub fn version_for(&self, os_arch: &str) -> Result<Version, ManifestError> {
        let prefix = "momos-music-manager-";
        let suffix = format!("-{os_arch}.");
        for name in self.entries.keys() {
            let Some(rest) = name.strip_prefix(prefix) else {
                continue;
            };
            // The version itself may contain `-` (e.g. `1.1.0-dev+abc1234`),
            // so cut at the LAST `-<os_arch>.` — the artifact extension
            // boundary — not the first `-`.
            let Some(idx) = rest.rfind(&suffix) else {
                continue;
            };
            let version_str = &rest[..idx];
            if let Ok(v) = Version::parse(version_str) {
                return Ok(v);
            }
        }
        Err(ManifestError::VersionNotFound {
            os_arch: os_arch.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    const SAMPLE: &str = "\
abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234  momos-music-manager-1.0.1-linux-x64.tar.gz
efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef  momos-music-manager-latest-linux-x64.tar.gz
1111222233334444555566667777888899990000aaaabbbbccccddddeeeeffff  momos-music-manager-1.0.1-macos-universal.dmg
";

    #[test]
    fn parses_and_looks_up() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(
            m.artifact_hash("momos-music-manager-latest-linux-x64.tar.gz"),
            Some("efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef")
        );
        assert!(m.contains("momos-music-manager-1.0.1-linux-x64.tar.gz"));
        assert!(!m.contains("nope"));
    }

    #[test]
    fn extracts_version() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.version_for("linux-x64").unwrap(), Version::new(1, 0, 1));
        assert!(m.version_for("windows-x64").is_err());
    }

    #[test]
    fn extracts_dev_version_with_build_metadata() {
        // Dev artifact: `1.1.0-dev+abc1234` contains `-` (pre-release) and
        // `+` (build metadata) — must parse as the full version string, not
        // be cut at the first `-`.
        let text = "\
abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234  momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz
efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef  momos-music-manager-latest-linux-x64.tar.gz
";
        let m = Manifest::parse(text).unwrap();
        assert_eq!(
            m.version_for("linux-x64").unwrap(),
            Version::parse("1.1.0-dev+abc1234").unwrap()
        );
        // Build metadata survives the round-trip.
        assert_eq!(
            m.version_for("linux-x64").unwrap().to_string(),
            "1.1.0-dev+abc1234"
        );
    }

    #[test]
    fn skips_latest_entries_and_unknown_os_arch() {
        // The stable `-latest-` entry must not be picked as the version, and
        // an os_arch that has no versioned entry yields VersionNotFound.
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.version_for("macos-universal").unwrap(), Version::new(1, 0, 1));
        assert_eq!(
            m.version_for("windows-x64").unwrap_err(),
            ManifestError::VersionNotFound {
                os_arch: "windows-x64".to_string()
            }
        );
    }

    #[test]
    fn tolerates_crlf_and_binary_mode_marker() {
        let text = "*abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234  momos-music-manager-1.0.1-linux-x64.tar.gz\r\n";
        let m = Manifest::parse(text).unwrap();
        assert!(m.contains("momos-music-manager-1.0.1-linux-x64.tar.gz"));
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(Manifest::parse("no-space-here\n").is_err());
        assert!(Manifest::parse("  momos-music-manager-1.0.1-linux-x64.tar.gz\n").is_err());
    }
}
