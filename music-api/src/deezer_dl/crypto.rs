//! Cryptographic utilities for Deezer audio decryption.
//!
//! Vendored from Rusteer `src/crypto/mod.rs` (MIT, Copyright (c) 2026
//! xScherpschutter). See ../THIRD_PARTY.md. FLAC-structure debug helpers were
//! dropped; an in-memory `decrypt_track_bytes` was added.
//!
//! # Encryption scheme
//!
//! Deezer uses a stripe encryption scheme:
//! - Audio is divided into 2048-byte blocks
//! - Every 3rd block (0, 3, 6, 9...) is encrypted with Blowfish CBC
//! - Other blocks are left unencrypted
//! - The encryption key is derived from the song ID

use md5::{Digest, Md5};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tracing::{debug, warn};

use crate::deezer_dl::error::{Error, Result};

/// Deezer's secret key for Blowfish key derivation.
const SECRET_KEY: &[u8] = b"g4el58wc0zvf9na1";

/// Blowfish initialization vector.
const BLOWFISH_IV: [u8; 8] = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];

/// Block size for audio encryption.
const BLOCK_SIZE: usize = 2048;

/// Blowfish cipher block size.
const BF_BLOCK_SIZE: usize = 8;

/// Compute MD5 hash of a string and return as hex string.
pub fn md5_hex(data: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Compute MD5 hash of bytes and return as hex string.
pub fn md5_hex_bytes(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

/// Generate a song hash using song ID, MD5, and media version.
///
/// This is used for legacy URL generation.
pub fn gen_song_hash(song_id: &str, song_md5: &str, media_version: &str) -> String {
    use sha1::{Digest as Sha1Digest, Sha1};

    let data = format!("{}{}{}", song_md5, media_version, song_id);
    let mut hasher = Sha1::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Calculate the Blowfish decryption key for a given song ID.
///
/// The key is derived by XORing:
/// - First 16 chars of MD5(song_id)
/// - Second 16 chars of MD5(song_id)
/// - Deezer's secret key
pub fn calc_blowfish_key(song_id: &str) -> Vec<u8> {
    let hash = md5_hex(song_id);
    let hash_bytes = hash.as_bytes();

    let mut key = Vec::with_capacity(16);
    for i in 0..16 {
        let byte = hash_bytes[i] ^ hash_bytes[i + 16] ^ SECRET_KEY[i];
        key.push(byte);
    }

    key
}

/// Decrypt a chunk using Blowfish CBC mode.
fn decrypt_blowfish_cbc(data: &[u8], key: &[u8]) -> Vec<u8> {
    use blowfish::Blowfish;
    use cipher::generic_array::GenericArray;
    use cipher::BlockDecrypt;
    use cipher::KeyInit;

    if data.len() % BF_BLOCK_SIZE != 0 {
        warn!(
            "Data length {} is not a multiple of {} bytes",
            data.len(),
            BF_BLOCK_SIZE
        );
    }

    // Create cipher with the key (using BigEndian byte order)
    let cipher: Blowfish<byteorder::BE> =
        Blowfish::new_from_slice(key).expect("Invalid key length for Blowfish");

    let mut result = data.to_vec();
    let mut prev_block = BLOWFISH_IV.to_vec();

    // Decrypt each 8-byte block with CBC mode
    for chunk in result.chunks_mut(BF_BLOCK_SIZE) {
        if chunk.len() < BF_BLOCK_SIZE {
            break;
        }

        // Save ciphertext for next CBC iteration
        let ciphertext = chunk.to_vec();

        // Decrypt block
        let block = GenericArray::from_mut_slice(chunk);
        cipher.decrypt_block(block);

        // XOR with previous ciphertext (CBC mode)
        for (byte, prev) in chunk.iter_mut().zip(prev_block.iter()) {
            *byte ^= prev;
        }

        prev_block = ciphertext;
    }

    result
}

/// Decrypt a 2048-byte audio block using Blowfish.
pub fn decrypt_blowfish_chunk(data: &[u8], key: &[u8]) -> Vec<u8> {
    decrypt_blowfish_cbc(data, key)
}

/// Decrypt Deezer audio bytes in memory and return the plaintext.
///
/// Applies the same stripe scheme as [`decrypt_track`] without touching disk.
pub fn decrypt_track_bytes(encrypted_data: &[u8], song_id: &str) -> Vec<u8> {
    let key = calc_blowfish_key(song_id);
    let mut out = Vec::with_capacity(encrypted_data.len());
    let mut block_count = 0;

    for chunk in encrypted_data.chunks(BLOCK_SIZE) {
        if block_count % 3 == 0 && chunk.len() == BLOCK_SIZE {
            out.extend_from_slice(&decrypt_blowfish_chunk(chunk, &key));
        } else {
            out.extend_from_slice(chunk);
        }
        block_count += 1;
    }

    out
}

/// Decrypt a Deezer audio track to a file.
///
/// Implements Deezer's stripe encryption scheme: process data in 2048-byte
/// blocks, only every 3rd block is encrypted (Blowfish CBC); the key is derived
/// from the song ID.
pub fn decrypt_track(encrypted_data: &[u8], song_id: &str, output_path: &Path) -> Result<()> {
    let key = calc_blowfish_key(song_id);

    let mut output = File::create(output_path)?;
    let mut block_count = 0;

    for chunk in encrypted_data.chunks(BLOCK_SIZE) {
        let processed = if block_count % 3 == 0 && chunk.len() == BLOCK_SIZE {
            decrypt_blowfish_chunk(chunk, &key)
        } else {
            chunk.to_vec()
        };

        output.write_all(&processed)?;
        block_count += 1;
    }

    Ok(())
}

/// Decrypt a Deezer audio track from a reader (streaming) to a file.
pub fn decrypt_track_streaming<R: Read>(
    reader: &mut R,
    song_id: &str,
    output_path: &Path,
) -> Result<()> {
    let key = calc_blowfish_key(song_id);

    let mut output = File::create(output_path)?;
    let mut buffer = [0u8; BLOCK_SIZE];
    let mut block_count = 0;
    let mut accumulated = Vec::new();

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        accumulated.extend_from_slice(&buffer[..bytes_read]);

        // Process complete blocks
        while accumulated.len() >= BLOCK_SIZE {
            let block: Vec<u8> = accumulated.drain(..BLOCK_SIZE).collect();

            let processed = if block_count % 3 == 0 {
                decrypt_blowfish_chunk(&block, &key)
            } else {
                block
            };

            output.write_all(&processed)?;
            block_count += 1;
        }
    }

    // Write any remaining data (partial block, not encrypted)
    if !accumulated.is_empty() {
        debug!("Writing final partial block of {} bytes", accumulated.len());
        output.write_all(&accumulated)?;
    }

    Ok(())
}

/// Decrypt using AES-CTR mode (newer Deezer content).
pub fn decrypt_aes_ctr(data: &[u8], key: &[u8], nonce: &[u8]) -> Result<Vec<u8>> {
    use aes::Aes128;
    use ctr::cipher::{KeyIvInit, StreamCipher};
    use ctr::Ctr128BE;

    if key.len() != 16 {
        return Err(Error::Crypto(format!(
            "Invalid AES key length: {} (expected 16)",
            key.len()
        )));
    }

    if nonce.len() != 16 {
        return Err(Error::Crypto(format!(
            "Invalid nonce length: {} (expected 16)",
            nonce.len()
        )));
    }

    let mut cipher = Ctr128BE::<Aes128>::new_from_slices(key, nonce)
        .map_err(|e| Error::Crypto(format!("Failed to create AES cipher: {}", e)))?;

    let mut result = data.to_vec();
    cipher.apply_keystream(&mut result);

    Ok(result)
}

/// Encryption parameters for a track.
#[derive(Debug, Clone)]
pub struct EncryptionParams {
    /// Type of encryption: "blowfish" or "aes".
    pub encryption_type: String,
    /// Track ID (for Blowfish key derivation).
    pub track_id: String,
    /// MD5 origin (for Blowfish).
    pub md5_origin: Option<String>,
    /// AES key (hex encoded).
    pub key: Option<String>,
    /// AES nonce (hex encoded).
    pub nonce: Option<String>,
}

/// Decrypt a file using the appropriate method based on parameters.
pub fn decrypt_file(
    encrypted_data: &[u8],
    params: &EncryptionParams,
    output_path: &Path,
) -> Result<()> {
    match params.encryption_type.as_str() {
        "aes" => {
            let key = params
                .key
                .as_ref()
                .ok_or_else(|| Error::Crypto("Missing AES key".to_string()))?;
            let nonce = params
                .nonce
                .as_ref()
                .ok_or_else(|| Error::Crypto("Missing AES nonce".to_string()))?;

            let key_bytes = hex::decode(key)
                .map_err(|e| Error::Crypto(format!("Invalid key hex: {}", e)))?;
            let nonce_bytes = hex::decode(nonce)
                .map_err(|e| Error::Crypto(format!("Invalid nonce hex: {}", e)))?;

            let decrypted = decrypt_aes_ctr(encrypted_data, &key_bytes, &nonce_bytes)?;

            let mut output = File::create(output_path)?;
            output.write_all(&decrypted)?;

            debug!("Successfully decrypted AES file to {:?}", output_path);
            Ok(())
        }
        _ => decrypt_track(encrypted_data, &params.track_id, output_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md5_hex() {
        assert_eq!(md5_hex("test"), "098f6bcd4621d373cade4e832627b4f6");
    }

    #[test]
    fn test_calc_blowfish_key_deterministic_and_len() {
        let key = calc_blowfish_key("3135556");
        assert_eq!(key.len(), 16);
        assert_eq!(hex::encode(&key), "6c6c666b39662c37652575603c643439");
        // Deterministic.
        assert_eq!(key, calc_blowfish_key("3135556"));
    }

    #[test]
    fn test_gen_song_hash_vector_and_len() {
        let hash = gen_song_hash(
            "3135556",
            "d41d8cd98f00b204e9800998ecf8427e",
            "1",
        );
        assert_eq!(hash, "a2cdfd837a708783c2fa86da4e72bec0ec268540");
        assert_eq!(hash.len(), 40);
        // Always 40 hex chars, regardless of input.
        assert_eq!(gen_song_hash("x", "y", "z").len(), 40);
    }

    #[test]
    fn test_decrypt_track_bytes_stripe_passthrough() {
        // Non-encrypted blocks (block index 1) pass through unchanged; the
        // function must not panic and must preserve total length.
        let data = vec![7u8; BLOCK_SIZE * 3];
        let out = decrypt_track_bytes(&data, "3135556");
        assert_eq!(out.len(), data.len());
        // Block 1 (unencrypted) is byte-identical.
        assert_eq!(&out[BLOCK_SIZE..2 * BLOCK_SIZE], &data[BLOCK_SIZE..2 * BLOCK_SIZE]);
    }

    #[test]
    fn test_decrypt_aes_ctr_rejects_bad_key_len() {
        let err = decrypt_aes_ctr(&[0u8; 4], &[0u8; 8], &[0u8; 16]).unwrap_err();
        assert!(matches!(err, Error::Crypto(_)));
    }

    #[test]
    fn test_decrypt_track_roundtrip_file() {
        let original = vec![0u8; BLOCK_SIZE * 4];
        let temp_dir = std::env::temp_dir();
        let output_path = temp_dir.join("test_deezer_dl_decrypt.bin");
        decrypt_track(&original, "test_song_id", &output_path).unwrap();
        let written = std::fs::read(&output_path).unwrap();
        assert_eq!(written.len(), original.len());
        let _ = std::fs::remove_file(&output_path);
    }
}
