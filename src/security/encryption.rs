//! Authenticated encryption at rest using standard AES-256-GCM.
//!
//! Conforms to MASTER_SPEC.md §13 & §14:
//! - Standard AES-256-GCM authenticated encryption (`ring::aead`).
//! - Cryptographically random 96-bit nonces generated via `ring::rand::SystemRandom`.
//! - Associated data binding to prevent ciphertext transplantation across contexts.
//! - Constant-time authentication tag verification.
//! - Safe failure modes: invalid keys or corrupted ciphertexts fail closed.

use ring::aead::{
    AES_256_GCM, Aad, BoundKey, Nonce, NonceSequence, OpeningKey, SealingKey, UnboundKey,
};
use ring::rand::{SecureRandom, SystemRandom};
use std::fmt;

/// Length of standard AES-256 key in bytes.
pub const AES_256_KEY_LEN: usize = 32;

/// Length of standard AES-GCM nonce in bytes (96 bits).
pub const AES_GCM_NONCE_LEN: usize = 12;

/// Length of AES-GCM authentication tag in bytes (128 bits).
pub const AES_GCM_TAG_LEN: usize = 16;

/// Errors arising from encryption or decryption operations.
#[derive(Debug, PartialEq, Eq)]
pub enum EncryptionError {
    InvalidKeyLength { expected: usize, actual: usize },
    InvalidKeyEncoding(String),
    InvalidCiphertextLength { min_expected: usize, actual: usize },
    RngError,
    DecryptionFailed,
}

impl fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKeyLength { expected, actual } => {
                write!(
                    f,
                    "Invalid encryption key length: expected {expected} bytes, got {actual}"
                )
            }
            Self::InvalidKeyEncoding(msg) => write!(f, "Invalid key encoding: {msg}"),
            Self::InvalidCiphertextLength {
                min_expected,
                actual,
            } => {
                write!(
                    f,
                    "Invalid ciphertext length: expected at least {min_expected} bytes (nonce + tag), got {actual}"
                )
            }
            Self::RngError => write!(f, "Cryptographic random number generation failure"),
            Self::DecryptionFailed => {
                write!(f, "Decryption or authentication tag verification failed")
            }
        }
    }
}

impl std::error::Error for EncryptionError {}

/// Sequence yielding a single pre-determined nonce.
struct SingleNonceSequence(Option<[u8; AES_GCM_NONCE_LEN]>);

impl NonceSequence for SingleNonceSequence {
    fn advance(&mut self) -> Result<Nonce, ring::error::Unspecified> {
        let bytes = self.0.take().ok_or(ring::error::Unspecified)?;
        Nonce::try_assume_unique_for_key(&bytes)
    }
}

/// AES-256-GCM authenticated encryption cipher.
#[derive(Clone)]
pub struct AesGcmCipher {
    key_bytes: [u8; AES_256_KEY_LEN],
}

impl AesGcmCipher {
    /// Initialize cipher from a 32-byte (256-bit) raw key slice.
    pub fn new(key: &[u8]) -> Result<Self, EncryptionError> {
        if key.len() != AES_256_KEY_LEN {
            return Err(EncryptionError::InvalidKeyLength {
                expected: AES_256_KEY_LEN,
                actual: key.len(),
            });
        }
        let mut key_bytes = [0u8; AES_256_KEY_LEN];
        key_bytes.copy_from_slice(key);
        Ok(Self { key_bytes })
    }

    /// Initialize cipher from a 64-character hexadecimal key string.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub fn from_hex(hex_key: &str) -> Result<Self, EncryptionError> {
        if hex_key.len() != AES_256_KEY_LEN * 2 {
            return Err(EncryptionError::InvalidKeyLength {
                expected: AES_256_KEY_LEN,
                actual: hex_key.len() / 2,
            });
        }
        if !hex_key.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(EncryptionError::InvalidKeyEncoding(
                "Key contains invalid hexadecimal characters. Expected 64 hex characters (0-9, a-f, A-F)."
                    .to_string(),
            ));
        }

        let mut key_bytes = [0u8; AES_256_KEY_LEN];
        for (i, chunk) in hex_key.as_bytes().chunks_exact(2).enumerate() {
            let s = std::str::from_utf8(chunk)
                .map_err(|e| EncryptionError::InvalidKeyEncoding(e.to_string()))?;
            key_bytes[i] = u8::from_str_radix(s, 16)
                .map_err(|e| EncryptionError::InvalidKeyEncoding(e.to_string()))?;
        }
        Self::new(&key_bytes)
    }

    /// Encrypt plaintext with optional associated data.
    ///
    /// Output format: `[ 12-byte Nonce | Ciphertext | 16-byte Tag ]`.
    pub fn encrypt(&self, plaintext: &[u8], aad_data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        let rng = SystemRandom::new();
        let mut nonce_bytes = [0u8; AES_GCM_NONCE_LEN];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| EncryptionError::RngError)?;

        let unbound_key = UnboundKey::new(&AES_256_GCM, &self.key_bytes)
            .map_err(|_| EncryptionError::DecryptionFailed)?;
        let nonce_seq = SingleNonceSequence(Some(nonce_bytes));
        let mut sealing_key = SealingKey::new(unbound_key, nonce_seq);

        let mut in_out = plaintext.to_vec();
        sealing_key
            .seal_in_place_append_tag(Aad::from(aad_data), &mut in_out)
            .map_err(|_| EncryptionError::DecryptionFailed)?;

        // Output nonce prepended to encrypted payload
        let mut result = Vec::with_capacity(AES_GCM_NONCE_LEN + in_out.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&in_out);
        Ok(result)
    }

    /// Decrypt ciphertext with optional associated data.
    ///
    /// Expects input format: `[ 12-byte Nonce | Ciphertext | 16-byte Tag ]`.
    pub fn decrypt(&self, payload: &[u8], aad_data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        let min_len = AES_GCM_NONCE_LEN + AES_GCM_TAG_LEN;
        if payload.len() < min_len {
            return Err(EncryptionError::InvalidCiphertextLength {
                min_expected: min_len,
                actual: payload.len(),
            });
        }

        let (nonce_slice, ciphertext_and_tag) = payload.split_at(AES_GCM_NONCE_LEN);
        let mut nonce_bytes = [0u8; AES_GCM_NONCE_LEN];
        nonce_bytes.copy_from_slice(nonce_slice);

        let unbound_key = UnboundKey::new(&AES_256_GCM, &self.key_bytes)
            .map_err(|_| EncryptionError::DecryptionFailed)?;
        let nonce_seq = SingleNonceSequence(Some(nonce_bytes));
        let mut opening_key = OpeningKey::new(unbound_key, nonce_seq);

        let mut in_out = ciphertext_and_tag.to_vec();
        let plaintext_slice = opening_key
            .open_in_place(Aad::from(aad_data), &mut in_out)
            .map_err(|_| EncryptionError::DecryptionFailed)?;

        Ok(plaintext_slice.to_vec())
    }
}

impl fmt::Debug for AesGcmCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AesGcmCipher")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes_gcm_encrypt_decrypt_roundtrip() {
        let key = [0x42u8; AES_256_KEY_LEN];
        let cipher = AesGcmCipher::new(&key).unwrap();

        let plaintext = b"Hello, secure world! This is a confidential API key.";
        let aad = b"tenant:user_12345";

        let ciphertext = cipher.encrypt(plaintext, aad).unwrap();
        assert_ne!(ciphertext, plaintext);
        assert!(ciphertext.len() >= plaintext.len() + AES_GCM_NONCE_LEN + AES_GCM_TAG_LEN);

        let decrypted = cipher.decrypt(&ciphertext, aad).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_aes_gcm_tampered_ciphertext_fails() {
        let key = [0x55u8; AES_256_KEY_LEN];
        let cipher = AesGcmCipher::new(&key).unwrap();

        let plaintext = b"Sensitive secret";
        let aad = b"context_a";
        let mut ciphertext = cipher.encrypt(plaintext, aad).unwrap();

        // Tamper with one byte in the ciphertext payload
        let last_idx = ciphertext.len() - 1;
        ciphertext[last_idx] ^= 0xFF;

        let result = cipher.decrypt(&ciphertext, aad);
        assert_eq!(result, Err(EncryptionError::DecryptionFailed));
    }

    #[test]
    fn test_aes_gcm_aad_mismatch_fails() {
        let key = [0x77u8; AES_256_KEY_LEN];
        let cipher = AesGcmCipher::new(&key).unwrap();

        let plaintext = b"Confidential token";
        let ciphertext = cipher.encrypt(plaintext, b"tenant_A").unwrap();

        // Attempting to decrypt with different associated data must fail authentication
        let result = cipher.decrypt(&ciphertext, b"tenant_B");
        assert_eq!(result, Err(EncryptionError::DecryptionFailed));
    }

    #[test]
    fn test_aes_gcm_hex_key_init() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let cipher = AesGcmCipher::from_hex(hex).unwrap();
        let pt = b"Testing hex key initialization";
        let ct = cipher.encrypt(pt, b"").unwrap();
        assert_eq!(cipher.decrypt(&ct, b"").unwrap(), pt);
    }
}
