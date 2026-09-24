//! Client for the remote content-addressed object store on `.200`.
//!
//! The store is a plain HTTP service (the same `music-api` deployment, see
//! `plans/proposed/remote-object-store.md`). Objects are keyed by the SHA-256 of
//! their bytes; the service verifies the digest of what it receives against the
//! key, so a `PUT` cannot claim a wrong key. Unlike MMM's own API the store does
//! **not** wrap responses in a `{data:…}` envelope.
//!
//! # Canonicalisation
//!
//! Files of the same audio often differ only by MMM's **Comment** tag, which is
//! generated and therefore differs between copies. The object stored is the file
//! with the comment cleared ([`canonicalise_to`]); its SHA-256 is the object key
//! and `files.content_hash`. `examples/canon_probe.rs` proved this is byte-stable
//! and comment-independent on FLAC/MP3/stem-M4A.
//!
//! # API
//!
//! | Method | Path                | Use                                              |
//! | ------ | ------------------- | ------------------------------------------------ |
//! | `PUT`  | `/objects/{sha256}` | upload (idempotent; `400` on digest mismatch)    |
//! | `HEAD` | `/objects/{sha256}` | exists?                                          |
//! | `GET`  | `/objects/{sha256}` | download (`Range` supported)                     |
//! | `POST` | `/objects/check`    | bulk `{hashes:[…]}` → `{present:[…],missing:[…]}`|
//! | `GET`  | `/objects`          | paged listing                                    |

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::ItemKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ── Configuration ──────────────────────────────────────────────────────────

/// Resolved `[store]` configuration (env > TOML > default).
///
/// Opt-in: [`StoreConfig::default`] is disabled and unconfigured, so nothing
/// touches the store until the user sets it up.
#[derive(Debug, Clone, Default)]
pub struct StoreConfig {
    /// Master switch; disabled short-circuits the sync task entirely.
    pub enabled: bool,
    /// Base URL of the store, e.g. `http://192.168.8.200:8080`.
    pub base_url: Option<String>,
    /// Bearer token (`STORE_TOKEN`).
    pub token: Option<String>,
}

impl StoreConfig {
    /// `true` only when the feature is enabled *and* both the URL and the token
    /// are present (without a token every request would 401).
    pub fn is_configured(&self) -> bool {
        self.enabled && self.base_url.is_some() && self.token.is_some()
    }
}

// ── Response types ─────────────────────────────────────────────────────────

/// Result of `POST /objects/check`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckResult {
    /// Hashes the store already holds.
    #[serde(default)]
    pub present: Vec<String>,
    /// Hashes the store does not hold.
    #[serde(default)]
    pub missing: Vec<String>,
}

/// Body of a successful `PUT /objects/{hash}`.
#[derive(Debug, Default, Deserialize)]
struct PutResponse {
    /// `true` when the bytes were newly stored, `false` when already present.
    #[serde(default)]
    stored: bool,
}

/// Error body shared by the store's non-2xx responses.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: String,
}

// ── Client ─────────────────────────────────────────────────────────────────

/// HTTP client for the remote object store.
pub struct StoreClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl StoreClient {
    /// Build a client with a fresh [`reqwest::Client`].
    pub fn new(base_url: &str, token: &str) -> Self {
        Self::with_http(reqwest::Client::new(), base_url, token)
    }

    /// Build a client around a caller-provided [`reqwest::Client`] (test seam).
    pub fn with_http(http: reqwest::Client, base_url: &str, token: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    /// Absolute URL for a store path.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    /// Turn a failed response into an error, preferring the JSON `error` field.
    async fn error_for(resp: reqwest::Response) -> anyhow::Error {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<ErrorBody>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| body.trim().to_string());
        anyhow::anyhow!("store request failed ({status}): {msg}")
    }

    /// `HEAD /objects/{hash}` — does the object exist? `404` → `false`.
    pub async fn head(&self, hash: &str) -> Result<bool> {
        let resp = self
            .http
            .head(self.url(&format!("objects/{hash}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("store: HEAD /objects/{hash}")?;
        match resp.status() {
            s if s.is_success() => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            _ => Err(Self::error_for(resp).await),
        }
    }

    /// `POST /objects/check` — bulk presence check for a list of hashes.
    pub async fn check(&self, hashes: &[String]) -> Result<CheckResult> {
        #[derive(Serialize)]
        struct Request<'a> {
            hashes: &'a [String],
        }

        let resp = self
            .http
            .post(self.url("objects/check"))
            .bearer_auth(&self.token)
            .json(&Request { hashes })
            .send()
            .await
            .context("store: POST /objects/check")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        resp.json()
            .await
            .context("store: parsing /objects/check response")
    }

    /// `PUT /objects/{hash}` — upload the bytes at `path`.
    ///
    /// Returns `true` when the object was newly stored, `false` when the store
    /// already had it (a `200`, not an error). The store rejects the request with
    /// `400` when the body's digest does not match `hash`.
    pub async fn put(
        &self,
        hash: &str,
        path: &Path,
        original_path: &str,
        isrc: Option<&str>,
    ) -> Result<bool> {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("store: reading {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        let mut req = self
            .http
            .put(self.url(&format!("objects/{hash}")))
            .bearer_auth(&self.token)
            .header("X-Original-Path", original_path)
            .header("X-Name", name)
            .body(bytes);
        if let Some(isrc) = isrc {
            req = req.header("X-ISRC", isrc);
        }

        let resp = req.send().await.context("store: PUT /objects/{hash}")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let status = resp.status();
        let stored = resp
            .json::<PutResponse>()
            .await
            .map(|p| p.stored)
            .unwrap_or(status == reqwest::StatusCode::CREATED);
        Ok(stored)
    }

    /// `GET /objects/{hash}` — download an object to `dest`.
    pub async fn get_to(&self, hash: &str, dest: &Path) -> Result<()> {
        let resp = self
            .http
            .get(self.url(&format!("objects/{hash}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("store: GET /objects/{hash}")?;
        if !resp.status().is_success() {
            return Err(Self::error_for(resp).await);
        }
        let bytes = resp
            .bytes()
            .await
            .context("store: reading GET /objects/{hash} body")?;
        tokio::fs::write(dest, &bytes)
            .await
            .with_context(|| format!("store: writing {}", dest.display()))?;
        Ok(())
    }
}

// ── Canonicalisation + hashing ─────────────────────────────────────────────

/// Lowercase hex encoding of a digest.
fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file's bytes, streamed (large files never fully in memory).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(hasher.finalize()))
}

/// Write the canonicalised copy of `src` (Comment tag cleared) to `dest` and
/// return its SHA-256.
///
/// The object form is *the file with MMM's Comment cleared* — the comment is the
/// only per-copy difference and is regenerable from the DB, so two local files
/// that differ only by comment collapse to one object.
///
/// `lofty`'s `save_to_path` rewrites FLAC in place, so `src` is copied to `dest`
/// first and the copy is edited. On any error `dest` is removed, so a failed
/// canonicalisation never leaves a partial object behind.
pub fn canonicalise_to(src: &Path, dest: &Path) -> Result<String> {
    let result = (|| -> Result<String> {
        std::fs::copy(src, dest)
            .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
        let mut tagged = lofty::read_from_path(dest)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("lofty read {}", dest.display()))?;
        if let Some(tag) = tagged.primary_tag_mut() {
            tag.remove_key(&ItemKey::Comment);
        }
        tagged
            .save_to_path(dest, WriteOptions::default())
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("lofty save {}", dest.display()))?;
        sha256_file(dest)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_file_matches_known_vector() {
        // SHA-256("abc") — a fixed public test vector.
        let dir = std::env::temp_dir().join(format!("store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.txt");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn canonicalise_to_cleans_up_on_error() {
        let dir = std::env::temp_dir().join(format!("store-test-err-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file lofty cannot parse as audio → canonicalisation fails after the
        // copy succeeded, and the destination must not be left behind.
        let src = dir.join("not-audio.bin");
        std::fs::write(&src, b"definitely not audio").unwrap();
        let dest = dir.join("canon.flac");

        let err = canonicalise_to(&src, &dest);
        assert!(err.is_err(), "garbage input must fail canonicalisation");
        assert!(
            !dest.exists(),
            "the temp output must be removed on the error path"
        );

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn store_config_requires_enabled_url_and_token() {
        assert!(!StoreConfig::default().is_configured());

        let cfg = StoreConfig {
            enabled: true,
            base_url: Some("http://x".to_string()),
            token: Some("t".to_string()),
        };
        assert!(cfg.is_configured());

        // Explicitly disabled wins over set URL/token.
        let disabled = StoreConfig {
            enabled: false,
            ..cfg.clone()
        };
        assert!(!disabled.is_configured());
    }

    #[test]
    fn check_result_parses_present_and_missing() {
        let parsed: CheckResult =
            serde_json::from_str(r#"{"present":["a"],"missing":["b"]}"#).unwrap();
        assert_eq!(parsed.present, vec!["a".to_string()]);
        assert_eq!(parsed.missing, vec!["b".to_string()]);
    }
}
