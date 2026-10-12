//! Embedded release-signing public key (Ed25519, minisign format).
//!
//! The autoupdater only installs updates whose `SHA256SUMS` manifest carries a
//! valid signature from this key. The corresponding **secret** key is stored
//! **only** as the GitHub Actions secret `MINISIGN_SECRET_KEY` (base64 of the
//! `minisign.key` file) and is never committed to the repository.
//!
//! # Rotation
//!
//! 1. Generate a new keypair:
//!    `minisign -G -p scripts/minisign.pub -s scripts/keys/minisign.key -W`
//! 2. Replace the `PUBLIC_KEY_B64` blob below (and `KEY_ID`) with the new
//!    public key, then commit.
//! 3. Store the new secret key as the CI secret `MINISIGN_SECRET_KEY`
//!    (base64: `base64 -w0 scripts/keys/minisign.key`).
//! 4. Release once with the new key (old signatures become invalid for the
//!    new binary — the updater refuses updates signed by the old key after
//!    the new binary is installed, which is the intended trust boundary).
//! 5. Document the rotation in `docs/RELEASE-ROADMAP.md` (M6).
//!
//! The file `scripts/minisign.pub` is the human-readable equivalent of the
//! embedded key and is used by CI to self-verify the signed manifest.

/// Full minisign public key blob (base64): `"Ed" || key_id[8] || pk[32]`.
pub const PUBLIC_KEY_B64: &str = "RWTQVWGr7Dp2/4CKtDwAsA/akrrvtKjE1tfXVexGn3JOyRC+1UkUiC6h";

/// Key id of the embedded key (little-endian, as stored in the blob).
pub const KEY_ID: [u8; 8] = [0xd0, 0x55, 0x61, 0xab, 0xec, 0x3a, 0x76, 0xff];

/// Raw 32-byte Ed25519 public key.
pub const PUBLIC_KEY_BYTES: [u8; 32] = [
    0x80, 0x8a, 0xb4, 0x3c, 0x00, 0xb0, 0x0f, 0xda, 0x92, 0xba, 0xef, 0xb4, 0xa8, 0xc4, 0xd6, 0xd7,
    0xd7, 0x55, 0xec, 0x46, 0x9f, 0x72, 0x4e, 0xc9, 0x10, 0xbe, 0xd5, 0x49, 0x14, 0x88, 0x2e, 0xa1,
];

/// Construct the [`ed25519_dalek::VerifyingKey`] for the embedded key.
/// Panics only if the embedded const is corrupt (guarded by tests).
pub fn verifying_key() -> ed25519_dalek::VerifyingKey {
    ed25519_dalek::VerifyingKey::from_bytes(&PUBLIC_KEY_BYTES)
        .expect("embedded Ed25519 public key is valid")
}

/// The minisign-style public key id of the embedded key.
pub fn key_id() -> [u8; 8] {
    KEY_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn embedded_key_matches_pubkey_blob() {
        // The const blob and the raw bytes must describe the same key.
        let blob = base64::engine::general_purpose::STANDARD
            .decode(PUBLIC_KEY_B64)
            .expect("pubkey blob is valid base64");
        assert_eq!(blob.len(), 42, "minisign pubkey blob is alg(2)+keyid(8)+pk(32)");
        assert_eq!(&blob[0..2], b"Ed", "minisign public key algorithm tag");
        assert_eq!(&blob[2..10], &KEY_ID, "key id in blob matches const");
        assert_eq!(&blob[10..42], &PUBLIC_KEY_BYTES, "pk in blob matches const");
    }

    #[test]
    fn embedded_key_matches_committed_pubkey_file() {
        // Guard against drift between the committed scripts/minisign.pub
        // (used by CI to self-verify the signed manifest) and the embedded key.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/minisign.pub");
        let file = std::fs::read_to_string(path).expect("scripts/minisign.pub exists");
        let blob_line = file
            .lines()
            .find(|l| l.starts_with('R') && !l.starts_with("untrusted"))
            .expect("pubkey file contains the base64 blob");
        assert_eq!(blob_line.trim(), PUBLIC_KEY_B64);
    }

    #[test]
    fn embedded_key_verifies_signed_manifest_from_cli_fixture() {
        // Cross-check the full pipeline against fixtures produced by the real
        // minisign CLI: a signed SHA256SUMS manifest (release-style layout).
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/autoupdate"
        );
        let manifest = std::fs::read(format!("{dir}/SHA256SUMS")).expect("manifest fixture");
        let sig = std::fs::read(format!("{dir}/SHA256SUMS.minisig")).expect("sig fixture");
        let key = crate::autoupdate::minisign::MinisignPublicKey::from_pubkey_file(
            &std::fs::read_to_string(format!("{dir}/minisign-test.pub")).expect("pubkey fixture"),
        )
        .expect("parse fixture pubkey");
        key.verify_bytes(&manifest, &sig).expect("fixture verifies");
    }
}
