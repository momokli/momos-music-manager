//! Parsing of the aggregated `SHA256SUMS` manifest published by CI.
//!
//! The manifest contains one `sha256sum`-style line per artifact, including a
//! **stable** name for the current rolling build:
//!
//! ```text
//! <hash>  momos-music-manager-1.0.1-linux-x64.tar.gz
//! <hash>  momos-music-manager-latest-linux-x64.tar.gz
//! ```
//!
//! The updater resolves the artifact for the current platform via the stable
//! `-latest-` name and derives the published version from the matching
//! versioned entry.

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
    /// entry whose artifact base matches, i.e. strip the `latest` middle
    /// segment from the stable name and locate the same artifact with a
    /// concrete `x.y.z` version.
    pub fn version_for(&self, os_arch: &str) -> Result<Version, ManifestError> {
        let stable = format!("momos-music-manager-latest-{os_arch}");
        let prefix = "momos-music-manager-";
        let mut candidates: Vec<&String> = self
            .entries
            .keys()
            .filter(|name| name.starts_with(prefix) && name.contains(os_arch))
            .collect();
        candidates.retain(|name| *name != &stable);
        // Prefer exact `-<os_arch>.` suffix matches over substring accidents
        // (e.g. `linux-x64` vs `linux-x64-extra`). The stable `-latest-` entry
        // also contains this pattern, but its version segment (`latest`) fails
        // semver parsing and is skipped below.
        let suffix = format!("-{os_arch}.");
        candidates.retain(|name| name.contains(&suffix));

        for name in candidates {
            let rest = name.strip_prefix(prefix).unwrap_or_default();
            // rest looks like `<version>-<os_arch>.<ext>`
            if let Some((ver, _)) = rest.split_once('-') {
                if let Ok(v) = Version::parse(ver) {
                    return Ok(v);
                }
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
