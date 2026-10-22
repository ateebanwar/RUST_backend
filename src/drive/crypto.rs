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
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CryptoError {
    /// Encryption key is not configured.
    KeyMissing(String),
    /// Encryption key is empty.
    EmptyKey,
    /// Key length does not match required AES-256 size (32 bytes / 64 hex characters).
    InvalidKeyLength { expected: usize, actual: usize },
    /// Key string contains non-hexadecimal characters or malformed encoding.
    InvalidKeyEncoding(String),
    /// Cryptographic sealing failure (e.g. CSPRNG failure).
    EncryptionFailed(String),
    /// Decryption or Poly1305 authentication tag verification failed.
    DecryptionFailed(String),
    /// Base64 decode error.
    Base64DecodeError(String),
    /// Hex decode error.
    HexDecodeError(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyMissing(msg) => write!(f, "Encryption key missing: {msg}"),
            Self::EmptyKey => write!(f, "Encryption key must not be empty"),
            Self::InvalidKeyLength { expected, actual } => {
                write!(f, "Invalid key length: expected {expected}, got {actual}")
            }
            Self::InvalidKeyEncoding(msg) => write!(f, "Invalid key encoding: {msg}"),
            Self::EncryptionFailed(msg) => write!(f, "Encryption failed: {msg}"),
            Self::DecryptionFailed(msg) => {
                write!(
                    f,
                    "Decryption or authentication tag verification failed: {msg}"
                )
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
            EncryptionError::InvalidKeyEncoding(msg) => Self::InvalidKeyEncoding(msg),
            EncryptionError::DecryptionFailed => Self::DecryptionFailed(
                "Ciphertext authentication tag verification failed".to_string(),
            ),
            other => Self::EncryptionFailed(other.to_string()),
        }
    }
}

/// Internal state of the Drive cryptographic engine.
#[derive(Clone)]
enum DriveCryptoState {
    Configured {
        cipher: AesGcmCipher,
        key_fingerprint: String,
    },
    Unconfigured {
        reason: String,
    },
}

/// Dedicated cryptographic engine for securing user data prior to Google Drive synchronization.
#[derive(Clone)]
pub struct DriveCryptoEngine {
    state: DriveCryptoState,
}

impl DriveCryptoEngine {
    /// Initialize with a direct 32-byte AES-256 key.
    pub fn new(key: &[u8]) -> Result<Self, CryptoError> {
        if key.is_empty() {
            return Err(CryptoError::EmptyKey);
        }
        if key.len() != AES_256_KEY_LEN {
            return Err(CryptoError::InvalidKeyLength {
                expected: AES_256_KEY_LEN,
                actual: key.len(),
            });
        }
        let cipher = AesGcmCipher::new(key)?;
        let key_fingerprint = compute_sha256(key);
        Ok(Self {
            state: DriveCryptoState::Configured {
                cipher,
                key_fingerprint,
            },
        })
    }

    /// Initialize from an exact 64-character hexadecimal key string.
    ///
    /// Conforms to strict security requirements:
    /// - Rejects empty keys.
    /// - Rejects keys whose length != 64 (never silently trims, truncates, or pads).
    /// - Rejects non-hex characters.
    /// - Decodes to exactly 32 bytes for AES-256-GCM.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn from_hex(hex_key: &str) -> Result<Self, CryptoError> {
        if hex_key.is_empty() {
            return Err(CryptoError::EmptyKey);
        }
        if hex_key.len() != AES_256_KEY_LEN * 2 {
            return Err(CryptoError::InvalidKeyLength {
                expected: AES_256_KEY_LEN * 2,
                actual: hex_key.len(),
            });
        }
        if !hex_key.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CryptoError::InvalidKeyEncoding(
                "Key contains invalid hexadecimal characters. Expected 64 hex characters (0-9, a-f, A-F)."
                    .to_string(),
            ));
        }

        let mut key_bytes = [0u8; AES_256_KEY_LEN];
        for (i, chunk) in hex_key.as_bytes().chunks_exact(2).enumerate() {
            let s = std::str::from_utf8(chunk)
                .map_err(|e| CryptoError::InvalidKeyEncoding(e.to_string()))?;
            key_bytes[i] = u8::from_str_radix(s, 16)
                .map_err(|e| CryptoError::InvalidKeyEncoding(e.to_string()))?;
        }

        Self::new(&key_bytes)
    }

    /// Create an unconfigured crypto engine with an explicit reason.
    ///
    /// Calling `encrypt` or `decrypt` will fail closed with `CryptoError::KeyMissing`.
    pub fn unconfigured(reason: impl Into<String>) -> Self {
        Self {
            state: DriveCryptoState::Unconfigured {
                reason: reason.into(),
            },
        }
    }

    /// Check if the crypto engine is configured with a valid encryption key.
    pub fn is_configured(&self) -> bool {
        matches!(self.state, DriveCryptoState::Configured { .. })
    }

    /// Return the SHA-256 fingerprint of the configured key, if configured.
    pub fn key_fingerprint(&self) -> Option<&str> {
        match &self.state {
            DriveCryptoState::Configured {
                key_fingerprint, ..
            } => Some(key_fingerprint),
            DriveCryptoState::Unconfigured { .. } => None,
        }
    }

    /// Derive a tenant-specific 32-byte key from a master secret and owner subject using HMAC-SHA256.
    ///
    /// Provided for isolated test fixtures. Never used as a fallback in production code.
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
        match &self.state {
            DriveCryptoState::Configured { cipher, .. } => {
                let aad = Self::aad_binding(owner_subject, resource_id);
                cipher.encrypt(plaintext, &aad).map_err(Into::into)
            }
            DriveCryptoState::Unconfigured { reason } => Err(CryptoError::KeyMissing(format!(
                "Cannot encrypt data: encryption key is not configured ({reason}). Set DRIVE_ENCRYPTION_KEY environment variable (64-character hexadecimal string)."
            ))),
        }
    }

    /// Decrypt ciphertext verifying AAD binding and 128-bit authentication tag.
    pub fn decrypt(
        &self,
        owner_subject: &str,
        resource_id: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        match &self.state {
            DriveCryptoState::Configured { cipher, .. } => {
                let aad = Self::aad_binding(owner_subject, resource_id);
                cipher.decrypt(ciphertext, &aad).map_err(|e| match e {
                    EncryptionError::DecryptionFailed => CryptoError::DecryptionFailed(
                        "Decryption or authentication tag verification failed. Ciphertext may have been modified, or decrypted with the wrong key."
                            .to_string(),
                    ),
                    other => other.into(),
                })
            }
            DriveCryptoState::Unconfigured { reason } => Err(CryptoError::KeyMissing(format!(
                "Cannot decrypt data: encryption key is not configured ({reason}). The original 32-byte key (DRIVE_ENCRYPTION_KEY) is required."
            ))),
        }
    }
}

impl fmt::Debug for DriveCryptoEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.state {
            DriveCryptoState::Configured {
                key_fingerprint, ..
            } => {
                let prefix = if key_fingerprint.len() >= 8 {
                    &key_fingerprint[..8]
                } else {
                    key_fingerprint
                };
                f.debug_struct("DriveCryptoEngine")
                    .field("configured", &true)
                    .field("key_fingerprint", &format!("{prefix}..."))
                    .finish()
            }
            DriveCryptoState::Unconfigured { reason } => f
                .debug_struct("DriveCryptoEngine")
                .field("configured", &false)
                .field("reason", reason)
                .finish(),
        }
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
