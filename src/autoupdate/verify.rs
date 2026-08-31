//! Update check + download + verification chain.
//!
//! Verification order (strict — nothing is installed unless every step
//! passes):
//!
//! 1. fetch `SHA256SUMS.minisig` + `SHA256SUMS`
//! 2. verify the Ed25519 (minisign) signature over the manifest with the
//!    embedded public key
//! 3. resolve the platform artifact (`momos-music-manager-latest-<os>-<arch>`)
//!    and the published version from the manifest
//! 4. on apply: download the artifact over HTTPS and verify its SHA256
//!    against the manifest entry
//! 5. only then swap the binary (see [`crate::autoupdate::swap`])

use std::path::PathBuf;

use semver::Version;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::manifest::{Manifest, ManifestError};
use super::minisign::MinisignPublicKey;
use super::swap::{self, UpdateMarker};
use super::{keys, platform};

/// Default release channel checked by the updater (dev builds).
pub const DEFAULT_BASE_URL: &str =
    "https://github.com/momokli/momos-music-manager/releases/download/latest-main";

/// Default channel for release builds: GitHub redirects `releases/latest` to
/// the newest non-prerelease release (reqwest follows redirects by default).
pub const DEFAULT_RELEASE_BASE_URL: &str =
    "https://github.com/momokli/momos-music-manager/releases/latest";

/// How long the new binary must stay healthy before an update is committed.
pub const DEFAULT_HEALTH_GRACE_SECS: u64 = 60;

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("update disabled by configuration")]
    Disabled,
    #[error("platform not supported by the updater: {0}")]
    UnsupportedPlatform(String),
    #[error("network error fetching {url}: {source}")]
    Fetch {
        url: String,
        source: reqwest::Error,
    },
    #[error("HTTP {status} fetching {url}")]
    HttpStatus { url: String, status: u16 },
    #[error("manifest signature verification failed — refusing to update: {0}")]
    Signature(#[from] super::minisign::MinisignError),
    #[error("invalid manifest: {0}")]
    Manifest(#[from] ManifestError),
    #[error("no update available")]
    NoUpdate,
    #[error("artifact SHA256 mismatch — refusing to install")]
    ChecksumMismatch,
    #[error("no binary entry found in archive for `{name}`")]
    MissingBinary { name: String },
    #[error("swap failed: {0}")]
    Swap(#[from] swap::SwapError),
    #[error("current executable unavailable: {0}")]
    CurrentExe(std::io::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not determine current version")]
    CurrentVersion,
    #[error("channel mismatch: current {current_version} and available {available_version} are on different channels (dev vs release)")]
    ChannelMismatch {
        current_version: String,
        available_version: String,
    },
}

/// Fetch abstraction so the verification chain is testable without network.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError>;
}

/// Production fetcher backed by reqwest (rustls).
#[derive(Clone, Default)]
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!(
                    "momos-music-manager/",
                    env!("MMM_VERSION"),
                    " (autoupdater)"
                ))
                .build()
                .expect("reqwest client builds"),
        }
    }
}

#[async_trait::async_trait]
impl Fetcher for HttpFetcher {
    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|source| UpdateError::Fetch {
                url: url.to_string(),
                source,
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(UpdateError::HttpStatus {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|source| UpdateError::Fetch {
                url: url.to_string(),
                source,
            })
    }
}

/// Platform artifact description for the current build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformArtifact {
    /// `linux-x64`, `linux-arm64`, `windows-x64`, `windows-arm64`, `macos-universal`
    pub os_arch: String,
    /// `tar.gz`, `zip` or `dmg`
    pub ext: String,
    /// Name of the executable inside the archive / on disk.
    pub binary_name: String,
}

impl PlatformArtifact {
    pub fn stable_artifact_name(&self) -> String {
        format!("momos-music-manager-latest-{}.{}", self.os_arch, self.ext)
    }
}

/// Settings for the updater (built from config in the CLI layer).
#[derive(Debug, Clone)]
pub struct UpdateSettings {
    pub base_url: String,
    pub enabled: bool,
    pub health_grace_secs: u64,
    pub current_version: String,
    pub artifact: PlatformArtifact,
    /// Key used to verify the manifest signature (embedded release key by
    /// default; injectable for tests / staging).
    pub pubkey: MinisignPublicKey,
    /// Directory for the swap + marker (defaults to the current exe dir).
    pub install_dir: Option<PathBuf>,
}

impl UpdateSettings {
    pub fn from_config(
        config: &crate::config::ServiceCredentials,
    ) -> Result<Self, UpdateError> {
        // Parse the current version once and fail fast: without a parseable
        // version the channel cannot be decided and comparisons would be wrong.
        let current_version = env!("MMM_VERSION").to_string();
        let current =
            Version::parse(&current_version).map_err(|_| UpdateError::CurrentVersion)?;

        // Channel-dependent default base URL: dev builds track the rolling
        // `latest-main` pre-release, release builds track the newest semver
        // release (`releases/latest`). An explicit override
        // (MOMOS_AUTOUPDATE_BASE_URL / [autoupdate] base_url) wins — any
        // value different from the built-in dev default is respected.
        let default_base_url = if current.pre.is_empty() {
            DEFAULT_RELEASE_BASE_URL
        } else {
            DEFAULT_BASE_URL
        };
        let base_url = if config.autoupdate_base_url == DEFAULT_BASE_URL {
            default_base_url.to_string()
        } else {
            config.autoupdate_base_url.clone()
        };

        Ok(Self {
            base_url,
            enabled: config.autoupdate_enabled,
            health_grace_secs: config.autoupdate_health_grace_secs,
            current_version,
            artifact: platform::current_artifact().ok_or_else(|| {
                UpdateError::UnsupportedPlatform(format!(
                    "{}/{}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ))
            })?,
            pubkey: MinisignPublicKey::from_blob(keys::PUBLIC_KEY_B64)
                .expect("embedded public key is valid"),
            install_dir: None,
        })
    }
}

/// Result of an update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Current version is the latest published one.
    UpToDate,
    /// New version available (manifest signed + verified).
    UpdateAvailable(UpdateInfo),
    /// The published version is on a different channel (dev vs release) —
    /// never auto-update across channels.
    ChannelMismatch {
        current_version: String,
        available_version: String,
    },
    /// Platform is not supported by the updater.
    UnsupportedPlatform,
    /// Updates disabled by configuration.
    Disabled,
}

impl UpdateStatus {
    /// Convenience wrapper around [`check`] for CLI use.
    pub async fn check<F: Fetcher>(
        settings: &UpdateSettings,
        fetcher: &F,
    ) -> Result<UpdateStatus, UpdateError> {
        check(settings, fetcher).await
    }

    /// Convenience wrapper around [`apply`] for CLI use.
    pub async fn apply<F: Fetcher>(
        settings: &UpdateSettings,
        fetcher: &F,
    ) -> Result<ApplyOutcome, UpdateError> {
        apply(settings, fetcher).await
    }
}

/// Verified information about an available update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub artifact_name: String,
    pub sha256: String,
    pub url: String,
}

/// Result of `update apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Binary swapped; restart the server to activate.
    Installed {
        new_version: String,
        old_version: String,
    },
    /// Platform artifact downloaded + verified, but not swapped (macOS DMG).
    DownloadedOnly { path: PathBuf, version: String },
}

/// Verify the manifest signature, resolve the platform artifact and compare
/// versions. Does **not** download the artifact.
pub async fn check<F: Fetcher>(
    settings: &UpdateSettings,
    fetcher: &F,
) -> Result<UpdateStatus, UpdateError> {
    if !settings.enabled {
        return Ok(UpdateStatus::Disabled);
    }
    match fetch_update_info(settings, fetcher).await {
        Ok(Some(info)) => Ok(UpdateStatus::UpdateAvailable(info)),
        Ok(None) => Ok(UpdateStatus::UpToDate),
        Err(UpdateError::ChannelMismatch {
            current_version,
            available_version,
        }) => Ok(UpdateStatus::ChannelMismatch {
            current_version,
            available_version,
        }),
        Err(e) => Err(e),
    }
}

/// Full apply: check + download + SHA256 verification + atomic swap.
pub async fn apply<F: Fetcher>(
    settings: &UpdateSettings,
    fetcher: &F,
) -> Result<ApplyOutcome, UpdateError> {
    if !settings.enabled {
        return Err(UpdateError::Disabled);
    }
    let Some(info) = fetch_update_info(settings, fetcher).await? else {
        return Err(UpdateError::NoUpdate);
    };

    tracing::info!(
        "autoupdate: downloading {} ({})",
        info.artifact_name,
        info.url
    );
    let bytes = fetcher.get_bytes(&info.url).await?;

    // SHA256 verification against the signed manifest — before anything is
    // written to disk.
    let actual = hex_digest(&bytes);
    if !actual.eq_ignore_ascii_case(&info.sha256) {
        return Err(UpdateError::ChecksumMismatch);
    }

    if settings.artifact.ext == "dmg" {
        // macOS: DMGs cannot be atomically swapped from a running binary in v1
        // (the executable lives inside an .app bundle). Download + verify and
        // let the user install — documented limitation.
        let dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("Downloads");
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(format!(
            "momos-music-manager-{}-macos-universal.dmg",
            info.version
        ));
        std::fs::write(&target, &bytes)?;
        return Ok(ApplyOutcome::DownloadedOnly {
            path: target,
            version: info.version,
        });
    }

    // Extract the binary from the archive (still fully in memory).
    let binary_bytes = extract_binary(
        &bytes,
        &settings.artifact.ext,
        &settings.artifact.binary_name,
    )?;

    let install_dir = settings
        .install_dir
        .clone()
        .unwrap_or_else(swap::exe_dir);
    let binary_name = settings.artifact.binary_name.clone();

    // Marker before the swap so a crash mid-swap is recoverable.
    let marker = UpdateMarker {
        old_version: settings.current_version.clone(),
        new_version: info.version.clone(),
        created_at_unix: chrono::Utc::now().timestamp(),
        start_count: 0,
        committed: false,
    };
    swap::write_marker(&install_dir, &marker)?;
    swap::swap_binary(&install_dir, &binary_name, &binary_bytes)?;

    Ok(ApplyOutcome::Installed {
        new_version: info.version,
        old_version: settings.current_version.clone(),
    })
}

/// Fetch + verify the manifest and build [`UpdateInfo`] if an update is
/// published on the same channel. Returns `Ok(None)` when already up to date.
async fn fetch_update_info<F: Fetcher>(
    settings: &UpdateSettings,
    fetcher: &F,
) -> Result<Option<UpdateInfo>, UpdateError> {
    let artifact = &settings.artifact;

    let sig_url = format!("{}/SHA256SUMS.minisig", settings.base_url);
    let manifest_url = format!("{}/SHA256SUMS", settings.base_url);

    let sig_bytes = fetcher.get_bytes(&sig_url).await?;
    let manifest_bytes = fetcher.get_bytes(&manifest_url).await?;

    // 1. Ed25519 verification of the manifest (before parsing anything).
    settings.pubkey.verify_bytes(&manifest_bytes, &sig_bytes)?;

    // 2. Resolve the published version from the verified manifest.
    let manifest_text = String::from_utf8_lossy(&manifest_bytes);
    let manifest = Manifest::parse(&manifest_text)?;
    let latest = manifest.version_for(&artifact.os_arch)?;

    let current = Version::parse(&settings.current_version)
        .map_err(|_| UpdateError::CurrentVersion)?;

    // 3. Channel guards: a dev build (`-dev+<sha>`) must never auto-update to
    //    a stable release and vice versa (rolling dev channel vs semver
    //    release channel).
    let current_is_dev = !current.pre.is_empty();
    let available_is_dev = !latest.pre.is_empty();
    if current_is_dev != available_is_dev {
        return Err(UpdateError::ChannelMismatch {
            current_version: settings.current_version.clone(),
            available_version: latest.to_string(),
        });
    }

    // 4. Version comparison. On the rolling dev channel `1.1.0-dev+shaA` and
    //    `1.1.0-dev+shaB` are precedence-equal (semver ignores build
    //    metadata), so a different SHA is detected via string comparison.
    let same_precedence_new_sha = latest == current
        && current_is_dev
        && latest.to_string() != settings.current_version;
    if !(latest > current || same_precedence_new_sha) {
        return Ok(None);
    }

    // 5. Artifact via the *versioned* name (exists on both channels;
    //    `Version::to_string()` preserves build metadata).
    let artifact_name = format!(
        "momos-music-manager-{}-{}.{}",
        latest, artifact.os_arch, artifact.ext
    );
    let sha256 = manifest
        .artifact_hash(&artifact_name)
        .ok_or_else(|| ManifestError::ArtifactNotFound {
            name: artifact_name.clone(),
        })?
        .to_string();

    let url = format!("{}/{}", settings.base_url, artifact_name);

    Ok(Some(UpdateInfo {
        version: latest.to_string(),
        artifact_name,
        sha256,
        url,
    }))
}

/// Extract the binary from an in-memory archive.
pub fn extract_binary(
    archive: &[u8],
    ext: &str,
    binary_name: &str,
) -> Result<Vec<u8>, UpdateError> {
    match ext {
        "tar.gz" | "tgz" => {
            let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
            let mut tar = tar::Archive::new(decoder);
            for entry in tar.entries()? {
                let mut entry = entry?;
                let path = entry.path()?.into_owned();
                if path
                    .file_name()
                    .map(|n| n == binary_name)
                    .unwrap_or(false)
                {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut entry, &mut buf)?;
                    return Ok(buf);
                }
            }
            Err(UpdateError::MissingBinary {
                name: binary_name.to_string(),
            })
        }
        "zip" => {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| {
                UpdateError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            })?;
            for i in 0..zip.len() {
                let mut file = zip.by_index(i).map_err(|e| {
                    UpdateError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;
                let name = file.name().to_string();
                let base = name.rsplit(['/', '\\']).next().unwrap_or(&name);
                if base == binary_name {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut file, &mut buf)?;
                    return Ok(buf);
                }
            }
            Err(UpdateError::MissingBinary {
                name: binary_name.to_string(),
            })
        }
        other => Err(UpdateError::MissingBinary {
            name: format!("unsupported archive type `{other}`"),
        }),
    }
}

pub fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine;
    use std::collections::HashMap;

    /// In-memory fetcher serving fixture files.
    pub(crate) struct MockFetcher {
        pub files: HashMap<String, Vec<u8>>,
    }

    impl MockFetcher {
        pub fn new(files: HashMap<String, Vec<u8>>) -> Self {
            Self { files }
        }
    }

    #[async_trait::async_trait]
    impl Fetcher for MockFetcher {
        async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
            self.files
                .get(url)
                .cloned()
                .ok_or_else(|| UpdateError::HttpStatus {
                    url: url.to_string(),
                    status: 404,
                })
        }
    }

    /// Test-only signing key (NOT the release key).
    fn test_signer() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
    }

    fn test_pubkey() -> MinisignPublicKey {
        let pk = test_signer().verifying_key().to_bytes();
        let mut blob = Vec::with_capacity(42);
        blob.extend_from_slice(b"Ed");
        blob.extend_from_slice(&pk[0..8]);
        blob.extend_from_slice(&pk);
        MinisignPublicKey::from_blob(&base64::engine::general_purpose::STANDARD.encode(&blob))
            .unwrap()
    }

    const BASE: &str = "https://example.invalid/latest-main";

    fn test_settings(os_arch: &str, ext: &str, current: &str) -> UpdateSettings {
        UpdateSettings {
            base_url: BASE.to_string(),
            enabled: true,
            health_grace_secs: 5,
            current_version: current.to_string(),
            artifact: PlatformArtifact {
                os_arch: os_arch.to_string(),
                ext: ext.to_string(),
                binary_name: "momos-music-manager".to_string(),
            },
            pubkey: test_pubkey(),
            install_dir: None,
        }
    }

    /// Insert a signed manifest (+ optional artifact) into the mock fetcher.
    /// The artifact is served under its **versioned** name (the updater no
    /// longer downloads via the stable `-latest-` name).
    fn signed_fixture(
        files: &mut HashMap<String, Vec<u8>>,
        manifest: &str,
        version: &str,
        artifact_bytes: Option<Vec<u8>>,
    ) {
        files.insert(
            format!("{BASE}/SHA256SUMS"),
            manifest.as_bytes().to_vec(),
        );
        let sig = super::super::minisign::sign_prehashed(
            &test_signer(),
            manifest.as_bytes(),
            "timestamp:1\tfile:SHA256SUMS\thashed",
        );
        files.insert(
            format!("{BASE}/SHA256SUMS.minisig"),
            sig.as_bytes().to_vec(),
        );
        if let Some(bytes) = artifact_bytes {
            files.insert(
                format!("{BASE}/momos-music-manager-{version}-linux-x64.tar.gz"),
                bytes,
            );
        }
    }

    fn sample_manifest(sha: &str, version: &str) -> String {
        format!(
            "{sha}  momos-music-manager-{version}-linux-x64.tar.gz\n{sha}  momos-music-manager-latest-linux-x64.tar.gz\n"
        )
    }

    fn tar_gz_with_binary(content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "momos-music-manager", content)
            .unwrap();
        builder.finish().unwrap();
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn hex_digest_matches_sha256sum() {
        assert_eq!(
            hex_digest(b"hello\n"),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
    }

    #[test]
    fn extract_binary_from_tar_gz() {
        let gz = tar_gz_with_binary(b"bin");
        let out = extract_binary(&gz, "tar.gz", "momos-music-manager").unwrap();
        assert_eq!(out, b"bin");
    }

    #[test]
    fn extract_binary_from_zip() {
        let mut writer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut writer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("momos-music-manager.exe", options).unwrap();
            std::io::Write::write_all(&mut zip, b"winbin").unwrap();
            zip.finish().unwrap();
        }
        let bytes = writer.into_inner();
        let out = extract_binary(&bytes, "zip", "momos-music-manager.exe").unwrap();
        assert_eq!(out, b"winbin");
    }

    #[tokio::test]
    async fn check_rejects_unsigned_or_tampered_manifest() {
        let mut files = HashMap::new();
        files.insert(
            format!("{BASE}/SHA256SUMS"),
            sample_manifest("abc", "2.0.0").into_bytes(),
        );
        files.insert(
            format!("{BASE}/SHA256SUMS.minisig"),
            b"garbage".to_vec(),
        );
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        assert!(matches!(
            check(&settings, &fetcher).await.unwrap_err(),
            UpdateError::Signature(_)
        ));
    }

    #[tokio::test]
    async fn check_rejects_manifest_signed_by_wrong_key() {
        let mut files = HashMap::new();
        let manifest = sample_manifest("abc", "2.0.0");
        // Signed by the correct test key…
        signed_fixture(&mut files, &manifest, "2.0.0", None);
        // …but the settings use the *embedded release* key → must be refused.
        let mut settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        settings.pubkey = MinisignPublicKey::from_blob(keys::PUBLIC_KEY_B64).unwrap();
        let fetcher = MockFetcher::new(files);
        assert!(matches!(
            check(&settings, &fetcher).await.unwrap_err(),
            UpdateError::Signature(_)
        ));
    }

    #[tokio::test]
    async fn check_returns_uptodate_for_equal_version() {
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest("abc", "1.0.1"), "1.0.1", None);
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        assert_eq!(check(&settings, &fetcher).await.unwrap(), UpdateStatus::UpToDate);
    }

    #[tokio::test]
    async fn check_reports_newer_version() {
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest("abc", "2.0.0"), "2.0.0", None);
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        match check(&settings, &fetcher).await.unwrap() {
            UpdateStatus::UpdateAvailable(info) => {
                assert_eq!(info.version, "2.0.0");
                assert_eq!(info.artifact_name, "momos-music-manager-2.0.0-linux-x64.tar.gz");
            }
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_verifies_checksum_before_swap() {
        let archive = tar_gz_with_binary(b"newbin");
        let wrong_sha = "0".repeat(64);
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest(&wrong_sha, "2.0.0"), "2.0.0", Some(archive));
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        assert!(matches!(
            apply(&settings, &fetcher).await.unwrap_err(),
            UpdateError::ChecksumMismatch
        ));
    }

    #[tokio::test]
    async fn check_against_real_http_server() {
        // End-to-end through HttpFetcher + a local HTTP server serving the
        // committed minisign-CLI-signed fixtures.
        use axum::{Router, routing::get};

        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/autoupdate"
        );
        let manifest = std::fs::read(format!("{dir}/SHA256SUMS")).unwrap();
        let sig = std::fs::read(format!("{dir}/SHA256SUMS.minisig")).unwrap();

        let app = Router::new()
            .route(
                "/SHA256SUMS",
                get(move || {
                    let m = manifest.clone();
                    async move {
                        axum::response::Response::new(axum::body::Body::from(m))
                    }
                }),
            )
            .route(
                "/SHA256SUMS.minisig",
                get(move || {
                    let s = sig.clone();
                    async move {
                        axum::response::Response::new(axum::body::Body::from(s))
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Fixtures are signed with the committed TEST key.
        let test_pub = MinisignPublicKey::from_pubkey_file(
            &std::fs::read_to_string(format!("{dir}/minisign-test.pub")).unwrap(),
        )
        .unwrap();
        let mut settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        settings.base_url = format!("http://{addr}");
        settings.pubkey = test_pub;

        let fetcher = HttpFetcher::new();
        match check(&settings, &fetcher).await.unwrap() {
            UpdateStatus::UpdateAvailable(info) => {
                assert_eq!(info.version, "2.0.0");
                assert_eq!(info.artifact_name, "momos-music-manager-2.0.0-linux-x64.tar.gz");
            }
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }
        server.abort();
    }

    #[tokio::test]
    async fn apply_swaps_binary_into_install_dir() {
        let archive = tar_gz_with_binary(b"newbin");
        let sha = hex_digest(&archive);
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest(&sha, "2.0.0"), "2.0.0", Some(archive));
        let fetcher = MockFetcher::new(files);

        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("momos-music-manager");
        std::fs::write(&bin_path, b"oldbin").unwrap();

        let mut settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        settings.install_dir = Some(dir.path().to_path_buf());

        let outcome = apply(&settings, &fetcher).await.unwrap();
        assert_eq!(
            outcome,
            ApplyOutcome::Installed {
                new_version: "2.0.0".to_string(),
                old_version: "1.0.1".to_string()
            }
        );
        // New binary in place, old one preserved as .bak, marker written.
        assert_eq!(std::fs::read(&bin_path).unwrap(), b"newbin");
        assert_eq!(
            std::fs::read(dir.path().join("momos-music-manager.bak")).unwrap(),
            b"oldbin"
        );
        assert!(dir.path().join("update-state.json").exists());
    }

    // ── Channel logic (US4) ────────────────────────────────────────────

    #[tokio::test]
    async fn dev_build_updates_to_newer_dev_sha() {
        // Rolling dev channel: `1.1.0-dev+abc1234` vs `1.1.0-dev+def5678` are
        // precedence-equal (semver ignores build metadata) — a different SHA
        // must still yield UpdateAvailable.
        let mut files = HashMap::new();
        signed_fixture(
            &mut files,
            &sample_manifest("abc", "1.1.0-dev+def5678"),
            "1.1.0-dev+def5678",
            None,
        );
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0-dev+abc1234");
        match check(&settings, &fetcher).await.unwrap() {
            UpdateStatus::UpdateAvailable(info) => {
                assert_eq!(info.version, "1.1.0-dev+def5678");
                assert_eq!(
                    info.artifact_name,
                    "momos-music-manager-1.1.0-dev+def5678-linux-x64.tar.gz"
                );
            }
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dev_build_same_sha_is_uptodate() {
        let mut files = HashMap::new();
        signed_fixture(
            &mut files,
            &sample_manifest("abc", "1.1.0-dev+abc1234"),
            "1.1.0-dev+abc1234",
            None,
        );
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0-dev+abc1234");
        assert_eq!(
            check(&settings, &fetcher).await.unwrap(),
            UpdateStatus::UpToDate
        );
    }

    #[tokio::test]
    async fn dev_build_rejects_release_manifest() {
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest("abc", "1.1.0"), "1.1.0", None);
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0-dev+abc1234");
        assert!(matches!(
            check(&settings, &fetcher).await.unwrap(),
            UpdateStatus::ChannelMismatch { .. }
        ));
    }

    #[tokio::test]
    async fn release_build_rejects_dev_manifest() {
        let mut files = HashMap::new();
        signed_fixture(
            &mut files,
            &sample_manifest("abc", "1.1.0-dev+def5678"),
            "1.1.0-dev+def5678",
            None,
        );
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0");
        assert!(matches!(
            check(&settings, &fetcher).await.unwrap(),
            UpdateStatus::ChannelMismatch { .. }
        ));
    }

    #[tokio::test]
    async fn release_build_updates_to_newer_release_with_versioned_artifact() {
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest("abc", "1.1.0"), "1.1.0", None);
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.0.1");
        match check(&settings, &fetcher).await.unwrap() {
            UpdateStatus::UpdateAvailable(info) => {
                assert_eq!(info.version, "1.1.0");
                assert_eq!(
                    info.artifact_name,
                    "momos-music-manager-1.1.0-linux-x64.tar.gz"
                );
                assert_eq!(
                    info.url,
                    format!("{BASE}/momos-music-manager-1.1.0-linux-x64.tar.gz")
                );
            }
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_rejects_channel_mismatch() {
        let mut files = HashMap::new();
        signed_fixture(&mut files, &sample_manifest("abc", "1.1.0"), "1.1.0", None);
        let fetcher = MockFetcher::new(files);
        let settings = test_settings("linux-x64", "tar.gz", "1.1.0-dev+abc1234");
        assert!(matches!(
            apply(&settings, &fetcher).await.unwrap_err(),
            UpdateError::ChannelMismatch { .. }
        ));
    }
}
