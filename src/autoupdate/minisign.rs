//! Minimal [minisign](https://github.com/jedisct1/minisign) / signify
//! compatible signature verification (Ed25519).
//!
//! Supports both signature layouts produced by the minisign CLI:
//!
//! * `"Ed"` — legacy raw Ed25519 over the file bytes (minisign ≤ 0.11 default)
//! * `"ED"` — prehashed Ed25519 over a BLAKE2b-512 digest of the file
//!   (minisign 0.12 default; trusted comment contains `\thashed`)
//!
//! ## File formats
//!
//! Public key file (`minisign.pub`), one base64 line:
//! `"Ed" || key_id[8] || public_key[32]`
//!
//! Signature file (`<file>.minisig`):
//! ```text
//! untrusted comment: <free text>
//! <base64: sig_alg[2] || key_id[8] || signature[64]>
//! trusted comment: <free text, e.g. "timestamp:…\tfile:…[\thashed]">
//! <base64: global_signature[64]>
//! ```
//! The global signature covers `signature[64] || trusted_comment` (raw).

use base64::Engine;
use ed25519_dalek::Signature;
use thiserror::Error;

/// Errors produced while parsing/verifying minisign signatures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MinisignError {
    #[error("signature blob has invalid base64")]
    BadBase64,
    #[error("unexpected blob length {0} (expected {1})")]
    BadLength(usize, usize),
    #[error("unsupported signature algorithm tag")]
    UnsupportedAlgorithm,
    #[error("signature key id {0:016X} does not match public key id {1:016X}")]
    KeyIdMismatch(u64, u64),
    #[error("untrusted comment line missing")]
    MissingComment,
    #[error("signature line missing")]
    MissingSignature,
    #[error("trusted comment line missing")]
    MissingTrustedComment,
    #[error("global signature line missing")]
    MissingGlobalSignature,
    #[error("Ed25519 verification failed")]
    VerificationFailed,
    #[error("global (trusted comment) signature verification failed")]
    GlobalVerificationFailed,
}

/// Decoded Ed25519 public key in minisign form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinisignPublicKey {
    key_id: [u8; 8],
    pk: [u8; 32],
}

impl MinisignPublicKey {
    /// Parse a public key from the base64 blob line of a `minisign.pub` file
    /// (`"Ed" || key_id[8] || pk[32]`, 42 bytes).
    pub fn from_blob(b64: &str) -> Result<Self, MinisignError> {
        let blob = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|_| MinisignError::BadBase64)?;
        if blob.len() != 42 {
            return Err(MinisignError::BadLength(blob.len(), 42));
        }
        if &blob[0..2] != b"Ed" {
            return Err(MinisignError::UnsupportedAlgorithm);
        }
        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&blob[2..10]);
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&blob[10..42]);
        Ok(Self { key_id, pk })
    }

    /// Parse a public key from the full contents of a `minisign.pub` file
    /// (skips the `untrusted comment:` line).
    pub fn from_pubkey_file(contents: &str) -> Result<Self, MinisignError> {
        let blob = contents
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or(MinisignError::MissingSignature)?;
        Self::from_blob(blob)
    }

    pub fn key_id(&self) -> [u8; 8] {
        self.key_id
    }

    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        ed25519_dalek::VerifyingKey::from_bytes(&self.pk)
            .expect("minisign public key is a valid Ed25519 key")
    }

    /// Verify a signature file against a message.
    pub fn verify(&self, message: &[u8], signature_file: &str) -> Result<(), MinisignError> {
        self.verify_bytes(message, signature_file.as_bytes())
    }

    /// Verify a signature file (as bytes) against a message.
    pub fn verify_bytes(&self, message: &[u8], sig_file: &[u8]) -> Result<(), MinisignError> {
        let text = std::str::from_utf8(sig_file).map_err(|_| MinisignError::BadBase64)?;
        self.verify_text(message, text)
    }

    /// Verify a signature file (as text) against a message.
    pub fn verify_text(&self, message: &[u8], sig_file: &str) -> Result<(), MinisignError> {
        let mut lines = sig_file.lines().map(|l| l.trim_end_matches('\r'));
        let comment = lines.next().ok_or(MinisignError::MissingComment)?;
        if !comment.starts_with("untrusted comment:") {
            return Err(MinisignError::MissingComment);
        }
        let sig_b64 = lines.next().ok_or(MinisignError::MissingSignature)?;
        let trusted_line = lines.next().ok_or(MinisignError::MissingTrustedComment)?;
        if !trusted_line.starts_with("trusted comment:") {
            return Err(MinisignError::MissingTrustedComment);
        }
        let trusted_comment = trusted_line
            .strip_prefix("trusted comment:")
            .unwrap_or_default()
            .trim();
        let global_b64 = lines.next().ok_or(MinisignError::MissingGlobalSignature)?;

        // Signature blob: sig_alg[2] || key_id[8] || sig[64] (74 bytes).
        let blob = base64::engine::general_purpose::STANDARD
            .decode(sig_b64.trim())
            .map_err(|_| MinisignError::BadBase64)?;
        if blob.len() != 74 {
            return Err(MinisignError::BadLength(blob.len(), 74));
        }
        let sig_alg = &blob[0..2];
        let key_id: [u8; 8] = blob[2..10].try_into().expect("slice length checked");
        let sig_bytes: [u8; 64] = blob[10..74].try_into().expect("slice length checked");

        if key_id != self.key_id {
            return Err(MinisignError::KeyIdMismatch(
                u64::from_le_bytes(key_id),
                u64::from_le_bytes(self.key_id),
            ));
        }

        // Message to verify: raw file bytes ("Ed") or BLAKE2b-512 prehash ("ED").
        let signature = Signature::from_bytes(&sig_bytes);
        let vk = self.verifying_key();
        let result = match sig_alg {
            b"Ed" => vk.verify_strict(message, &signature),
            b"ED" => {
                use blake2::Digest;
                let digest = blake2::Blake2b512::digest(message);
                vk.verify_strict(&digest, &signature)
            }
            _ => return Err(MinisignError::UnsupportedAlgorithm),
        };
        result.map_err(|_| MinisignError::VerificationFailed)?;

        // Global signature over sig[64] || trusted_comment (raw, never prehashed).
        let global = base64::engine::general_purpose::STANDARD
            .decode(global_b64.trim())
            .map_err(|_| MinisignError::BadBase64)?;
        if global.len() != 64 {
            return Err(MinisignError::BadLength(global.len(), 64));
        }
        let mut global_msg = Vec::with_capacity(64 + trusted_comment.len());
        global_msg.extend_from_slice(&sig_bytes);
        global_msg.extend_from_slice(trusted_comment.as_bytes());
        let mut global_arr = [0u8; 64];
        global_arr.copy_from_slice(&global);
        let global_sig = Signature::from_bytes(&global_arr);
        vk.verify_strict(&global_msg, &global_sig)
            .map_err(|_| MinisignError::GlobalVerificationFailed)?;

        Ok(())
    }
}

/// Create a minisign-format signature (prehashed `"ED"`, like minisign 0.12)
/// for `message` with the given key. Used by tests and local tooling.
#[cfg(any(test, feature = "test-util"))]
pub fn sign_prehashed(
    signing_key: &ed25519_dalek::SigningKey,
    message: &[u8],
    trusted_comment: &str,
) -> String {
    use blake2::Digest;
    use ed25519_dalek::Signer;

    let digest = blake2::Blake2b512::digest(message);
    let sig = signing_key.sign(&digest).to_bytes();

    let mut blob = Vec::with_capacity(74);
    blob.extend_from_slice(b"ED");
    blob.extend_from_slice(&signing_key.verifying_key().to_bytes()[0..8]); // key id = first 8 pk bytes
    blob.extend_from_slice(&sig);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(&blob);

    let global_msg = [sig.as_slice(), trusted_comment.as_bytes()].concat();
    let global = signing_key.sign(&global_msg).to_bytes();
    let global_b64 = base64::engine::general_purpose::STANDARD.encode(global);

    format!(
        "untrusted comment: signature from test key\n{sig_b64}\ntrusted comment: {trusted_comment}\n{global_b64}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only keypair (committed under tests/fixtures/autoupdate/ for
    /// reproducibility — NOT the release key).
    const TEST_PUB_B64: &str = "RWQ5lRTEQryEOM7Pn8hAHK/2B8tnDYwFjN6pVJdcV7SSlMXfX6h9WQvA";

    #[test]
    fn parses_pubkey_file_with_comment() {
        let contents = "untrusted comment: minisign public key 399514C442BC8438\nRWQ5lRTEQryEOM7Pn8hAHK/2B8tnDYwFjN6pVJdcV7SSlMXfX6h9WQvA\n";
        let key = MinisignPublicKey::from_pubkey_file(contents).unwrap();
        assert_eq!(key.key_id(), [0x39, 0x95, 0x14, 0xc4, 0x42, 0xbc, 0x84, 0x38]);
    }

    #[test]
    fn rejects_garbage_blob() {
        assert!(MinisignPublicKey::from_blob("not-base64!!").is_err());
        assert!(MinisignPublicKey::from_blob("AAAA").is_err(), "too short");
        // Wrong algorithm tag.
        let mut blob = vec![0u8; 42];
        blob[0] = b'E';
        blob[1] = b'X';
        assert_eq!(
            MinisignPublicKey::from_blob(&base64::engine::general_purpose::STANDARD.encode(&blob)),
            Err(MinisignError::UnsupportedAlgorithm)
        );
    }

    #[test]
    fn verifies_real_minisign_cli_fixture_prehashed() {
        // Created with minisign 0.12: `minisign -S -s <key> -m hello.txt`
        // (default prehashed "ED" format).
        let msg = b"hello autoupdater\n";
        let sig = std::fs::read(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/autoupdate/hello.txt.minisig"),
        )
        .expect("fixture exists");
        let key = MinisignPublicKey::from_pubkey_file(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/autoupdate/minisign-test.pub"
            ))
            .expect("test pubkey exists"),
        )
        .expect("parse test pubkey");
        key.verify_bytes(msg, &sig).expect("fixture verifies");
    }

    #[test]
    fn detects_tampered_message() {
        let key = MinisignPublicKey::from_blob(TEST_PUB_B64).unwrap();
        let signer = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let sig = sign_prehashed(&signer, b"original", "timestamp:1\tfile:x");
        // Signature belongs to `signer`, not to the TEST_PUB key.
        assert!(key.verify_bytes(b"original", sig.as_bytes()).is_err());
    }

    #[test]
    fn roundtrip_sign_and_verify() {
        let signer = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let pubkey_bytes = signer.verifying_key().to_bytes();
        // Build a minisign pubkey blob: "Ed" || key_id(8) || pk(32).
        let mut blob = Vec::with_capacity(42);
        blob.extend_from_slice(b"Ed");
        blob.extend_from_slice(&pubkey_bytes[0..8]);
        blob.extend_from_slice(&pubkey_bytes);
        let pub_b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        let key = MinisignPublicKey::from_blob(&pub_b64).unwrap();

        let sig = sign_prehashed(&signer, b"hello", "timestamp:1\tfile:x\thashed");
        key.verify_bytes(b"hello", sig.as_bytes()).expect("valid");

        // Tampered message must fail.
        assert_eq!(
            key.verify_bytes(b"hello!", sig.as_bytes()),
            Err(MinisignError::VerificationFailed)
        );
        // Tampered global signature must fail.
        let mut lines: Vec<String> = sig.lines().map(String::from).collect();
        let n = lines.len();
        let mut global = base64::engine::general_purpose::STANDARD
            .decode(lines[n - 1].trim())
            .unwrap();
        global[0] ^= 0xff;
        lines[n - 1] = base64::engine::general_purpose::STANDARD.encode(&global);
        let tampered = lines.join("\n");
        assert_eq!(
            key.verify_bytes(b"hello", tampered.as_bytes()),
            Err(MinisignError::GlobalVerificationFailed)
        );
    }

    #[test]
    fn key_id_mismatch_is_reported() {
        let signer = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let other = ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]);
        let pubkey_bytes = other.verifying_key().to_bytes();
        let mut blob = Vec::with_capacity(42);
        blob.extend_from_slice(b"Ed");
        blob.extend_from_slice(&pubkey_bytes[0..8]);
        blob.extend_from_slice(&pubkey_bytes);
        let pub_b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        let key = MinisignPublicKey::from_blob(&pub_b64).unwrap();

        let sig = sign_prehashed(&signer, b"hello", "timestamp:1\tfile:x");
        assert!(matches!(
            key.verify_bytes(b"hello", sig.as_bytes()),
            Err(MinisignError::KeyIdMismatch(..))
        ));
    }
}
