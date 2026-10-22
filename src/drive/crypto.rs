//! Cryptographic engine for Google Drive payload and file encryption.
//!
//! Conforms to MASTER_SPEC.md §13 & §14 and Phase 15 requirements:
//! - AES-256-GCM authenticated encryption at rest (`ring::aead`).
//! - 96-bit cryptographically secure random nonces per payload.
//! - Associated Authenticated Data (AAD) binding to `owner_subject:resource_id`
//!   to strictly prevent ciphertext transplantation attacks.
//! - Constant-time tag verification to prevent padding or timing oracles.
//! - Hex and Base64 encoding/decoding utilities for safe transmission.
//! - SHA-256 integrity checksum verification.

use crate::security::{AES_256_KEY_LEN, AesGcmCipher, EncryptionError};
use ring::digest::{Context, SHA256};
use ring::hmac;
use std::fmt;

/// Errors arising during Drive cryptographic operations.
#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    InvalidKeyLength { expected: usize, actual: usize },
    EncryptionFailed(String),
    DecryptionFailed(String),
    Base64DecodeError(String),
    HexDecodeError(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKeyLength { expected, actual } => {
                write!(f, "Invalid key length: expected {expected}, got {actual}")
            }
            Self::EncryptionFailed(msg) => write!(f, "Encryption failed: {msg}"),
            Self::DecryptionFailed(msg) => {
                write!(f, "Decryption or tag verification failed: {msg}")
            }
            Self::Base64DecodeError(msg) => write!(f, "Base64 decode error: {msg}"),
            Self::HexDecodeError(msg) => write!(f, "Hex decode error: {msg}"),
        }
    }
}

impl std::error::Error for CryptoError {}

impl From<EncryptionError> for CryptoError {
    fn from(err: EncryptionError) -> Self {
        match err {
            EncryptionError::InvalidKeyLength { expected, actual } => {
                Self::InvalidKeyLength { expected, actual }
            }
            EncryptionError::DecryptionFailed => Self::DecryptionFailed(
                "Ciphertext authentication tag verification failed".to_string(),
            ),
            other => Self::EncryptionFailed(other.to_string()),
        }
    }
}

/// Dedicated cryptographic engine for securing user data prior to Google Drive synchronization.
#[derive(Clone)]
pub struct DriveCryptoEngine {
    cipher: AesGcmCipher,
}

impl DriveCryptoEngine {
    /// Initialize with a direct 32-byte AES-256 key.
    pub fn new(key: &[u8]) -> Result<Self, CryptoError> {
        let cipher = AesGcmCipher::new(key)?;
        Ok(Self { cipher })
    }

    /// Initialize from a 64-character hexadecimal key string.
    pub fn from_hex(hex_key: &str) -> Result<Self, CryptoError> {
        let cipher = AesGcmCipher::from_hex(hex_key)?;
        Ok(Self { cipher })
    }

    /// Derive a tenant-specific 32-byte key from a master secret and owner subject using HMAC-SHA256.
    pub fn derive_for_owner(master_secret: &[u8], owner_subject: &str) -> Self {
        let s_key = hmac::Key::new(hmac::HMAC_SHA256, master_secret);
        let tag = hmac::sign(&s_key, owner_subject.as_bytes());
        let mut key_bytes = [0u8; AES_256_KEY_LEN];
        key_bytes.copy_from_slice(&tag.as_ref()[..AES_256_KEY_LEN]);
        Self::new(&key_bytes).expect("Derived key has exact 32-byte length")
    }

    /// Format standard AAD binding string: `{owner_subject}:{resource_id}`.
    pub fn aad_binding(owner_subject: &str, resource_id: &str) -> Vec<u8> {
        format!("{owner_subject}:{resource_id}").into_bytes()
    }

    /// Encrypt plaintext with AAD binding to prevent cross-resource transplantation.
    pub fn encrypt(
        &self,
        owner_subject: &str,
        resource_id: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let aad = Self::aad_binding(owner_subject, resource_id);
        self.cipher.encrypt(plaintext, &aad).map_err(Into::into)
    }

    /// Decrypt ciphertext verifying AAD binding and 128-bit authentication tag.
    pub fn decrypt(
        &self,
        owner_subject: &str,
        resource_id: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let aad = Self::aad_binding(owner_subject, resource_id);
        self.cipher.decrypt(ciphertext, &aad).map_err(Into::into)
    }
}

/// Compute hex-encoded SHA-256 digest of input bytes.
pub fn compute_sha256(data: &[u8]) -> String {
    let mut ctx = Context::new(&SHA256);
    ctx.update(data);
    let digest = ctx.finish();
    encode_hex(digest.as_ref())
}

/// Encode bytes to lowercase hex string.
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Standard Base64 table (RFC 4648).
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode slice of bytes to standard Base64 string.
pub fn encode_base64(data: &[u8]) -> String {
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    let (chunks, remainder) = data.as_chunks::<3>();

    for chunk in chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        result.push(BASE64_ALPHABET[((n >> 18) & 63) as usize] as char);
        result.push(BASE64_ALPHABET[((n >> 12) & 63) as usize] as char);
        result.push(BASE64_ALPHABET[((n >> 6) & 63) as usize] as char);
        result.push(BASE64_ALPHABET[(n & 63) as usize] as char);
    }

    match remainder.len() {
        1 => {
            let n = (remainder[0] as u32) << 16;
            result.push(BASE64_ALPHABET[((n >> 18) & 63) as usize] as char);
            result.push(BASE64_ALPHABET[((n >> 12) & 63) as usize] as char);
            result.push('=');
            result.push('=');
        }
        2 => {
            let n = ((remainder[0] as u32) << 16) | ((remainder[1] as u32) << 8);
            result.push(BASE64_ALPHABET[((n >> 18) & 63) as usize] as char);
            result.push(BASE64_ALPHABET[((n >> 12) & 63) as usize] as char);
            result.push(BASE64_ALPHABET[((n >> 6) & 63) as usize] as char);
            result.push('=');
        }
        _ => {}
    }

    result
}

/// Decode standard Base64 string to bytes.
pub fn decode_base64(input: &str) -> Result<Vec<u8>, CryptoError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut values = Vec::with_capacity(input.len());
    for b in input.bytes() {
        if b == b'=' {
            break;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => {
                return Err(CryptoError::Base64DecodeError(format!(
                    "Invalid character: {b}"
                )));
            }
        };
        values.push(val);
    }

    let mut output = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        match chunk.len() {
            4 => {
                let n = ((chunk[0] as u32) << 18)
                    | ((chunk[1] as u32) << 12)
                    | ((chunk[2] as u32) << 6)
                    | (chunk[3] as u32);
                output.push((n >> 16) as u8);
                output.push((n >> 8) as u8);
                output.push(n as u8);
            }
            3 => {
                let n = ((chunk[0] as u32) << 18)
                    | ((chunk[1] as u32) << 12)
                    | ((chunk[2] as u32) << 6);
                output.push((n >> 16) as u8);
                output.push((n >> 8) as u8);
            }
            2 => {
                let n = ((chunk[0] as u32) << 18) | ((chunk[1] as u32) << 12);
                output.push((n >> 16) as u8);
            }
            1 => {
                return Err(CryptoError::Base64DecodeError(
                    "Invalid single-character chunk".to_string(),
                ));
            }
            _ => {}
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_roundtrip() {
        let cases = vec![
            b"".to_vec(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
            vec![0, 1, 2, 255, 128, 64],
        ];

        for original in cases {
            let encoded = encode_base64(&original);
            let decoded = decode_base64(&encoded).expect("Decode should succeed");
            assert_eq!(decoded, original);
        }
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [42u8; 32];
        let engine = DriveCryptoEngine::new(&key).unwrap();

        let owner = "google:user_123";
        let res_id = "conv_abc";
        let plaintext = b"Hello, secure Google Drive chat synchronization!";

        let ciphertext = engine.encrypt(owner, res_id, plaintext).unwrap();
        assert_ne!(ciphertext, plaintext);

        let decrypted = engine.decrypt(owner, res_id, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_aad_binding_prevents_ciphertext_transplantation() {
        let key = [42u8; 32];
        let engine = DriveCryptoEngine::new(&key).unwrap();

        let owner_a = "google:user_a";
        let owner_b = "google:user_b";
        let res_id = "conv_abc";
        let plaintext = b"Confidential conversation";

        let ciphertext = engine.encrypt(owner_a, res_id, plaintext).unwrap();

        // Attempting to decrypt under different owner MUST fail
        let result = engine.decrypt(owner_b, res_id, &ciphertext);
        assert!(result.is_err());

        // Attempting to decrypt with different resource ID MUST fail
        let result_res = engine.decrypt(owner_a, "conv_xyz", &ciphertext);
        assert!(result_res.is_err());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let key = [42u8; 32];
        let engine = DriveCryptoEngine::new(&key).unwrap();

        let owner = "google:user_123";
        let res_id = "conv_abc";
        let plaintext = b"Original content";

        let mut ciphertext = engine.encrypt(owner, res_id, plaintext).unwrap();
        // Flip one byte in ciphertext
        let len = ciphertext.len();
        ciphertext[len - 5] ^= 0x01;

        let result = engine.decrypt(owner, res_id, &ciphertext);
        assert!(result.is_err());
    }
}
