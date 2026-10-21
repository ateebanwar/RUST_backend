//! Secure wrapper types and masking utilities for sensitive credentials and secrets.
//!
//! Enforces zero-leak policies:
//! 1. `Debug` formats strictly as `"[REDACTED]"`.
//! 2. `Display` formats strictly as `"[REDACTED]"`.
//! 3. `Serialize` serializes strictly as `"[REDACTED]"` to prevent accidental API disclosure.
//! 4. Plaintext value accessible only via explicit `expose_secret()`.
//! 5. Constant-time equality comparison via SHA-256 digests.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A protected string type that guards secrets against inadvertent logging,
/// formatting, debugging, and JSON serialization leakage.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a plaintext string in a `SecretString`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Explicitly access the underlying plaintext secret.
    ///
    /// This method is intentionally named to encourage audits of any code path
    /// that directly references raw credential material.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Length of the secret in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Constant-time comparison between this secret and a candidate string.
    pub fn constant_time_eq(&self, candidate: &str) -> bool {
        crate::auth::pat::constant_time_compare_str(&self.0, candidate)
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"[REDACTED]\"")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

/// Safely mask a token or API key for logging, listing, or tracing.
///
/// Preserves a small non-secret prefix and suffix (e.g. `pat_a1b2...ef01`),
/// ensuring the full credential value is never disclosed.
pub fn mask_credential(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.len() <= 8 {
        return "********".to_string();
    }

    if trimmed.starts_with("pat_") && trimmed.len() >= 12 {
        let prefix = &trimmed[..8];
        let suffix = &trimmed[trimmed.len() - 4..];
        format!("{prefix}...{suffix}")
    } else if trimmed.len() >= 12 {
        let prefix = &trimmed[..4];
        let suffix = &trimmed[trimmed.len() - 4..];
        format!("{prefix}...{suffix}")
    } else {
        "********".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_string_redacted_in_debug_and_display() {
        let secret = SecretString::new("super_secret_api_key_12345");
        assert_eq!(format!("{secret:?}"), "\"[REDACTED]\"");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(secret.expose_secret(), "super_secret_api_key_12345");
    }

    #[test]
    fn test_secret_string_redacted_in_serialization() {
        let secret = SecretString::new("live_credential_test");
        let serialized = serde_json::to_string(&secret).unwrap();
        assert_eq!(serialized, "\"[REDACTED]\"");
        assert!(!serialized.contains("live_credential_test"));
    }

    #[test]
    fn test_secret_string_constant_time_comparison() {
        let secret = SecretString::new("pat_0123456789abcdef");
        assert!(secret.constant_time_eq("pat_0123456789abcdef"));
        assert!(!secret.constant_time_eq("pat_wrong"));
    }

    #[test]
    fn test_mask_credential() {
        assert_eq!(
            mask_credential("pat_1234567890abcdef1234567890abcdef"),
            "pat_1234...cdef"
        );
        assert_eq!(
            mask_credential("sk-ant-api03-abcdefghijklmnop"),
            "sk-a...mnop"
        );
        assert_eq!(mask_credential("short"), "********");
        assert_eq!(mask_credential(""), "");
    }
}
