//! Personal Access Token (PAT) & API Key cryptographic validation.
//!
//! Enforces constant-time comparison via `subtle::ConstantTimeEq` and
//! SHA-256 token hashing to prevent timing side-channel attacks and
//! memory exposure.

use super::principal::{AuthPrincipal, AuthRole, AuthType, ClientType};
use subtle::ConstantTimeEq;

/// Length of SHA-256 digest in bytes.
const SHA256_DIGEST_LEN: usize = 32;

/// Standard Personal Access Token prefix.
pub const PAT_PREFIX: &str = "pat_";

/// Compute SHA-256 digest of an incoming candidate token or key string.
pub fn hash_token(token: &str) -> [u8; SHA256_DIGEST_LEN] {
    let digest = ring::digest::digest(&ring::digest::SHA256, token.as_bytes());
    let mut out = [0u8; SHA256_DIGEST_LEN];
    out.copy_from_slice(digest.as_ref());
    out
}

/// Constant-time comparison between two 32-byte digests using `subtle::ConstantTimeEq`.
pub fn constant_time_eq_32(a: &[u8; SHA256_DIGEST_LEN], b: &[u8; SHA256_DIGEST_LEN]) -> bool {
    a.ct_eq(b).into()
}

/// Constant-time comparison between candidate string and expected string by hashing both to 32-byte SHA-256 digests.
///
/// Hashing both operands prior to comparison guarantees operands are strictly identical in length (32 bytes),
/// eliminating any length-dependent timing side channels.
pub fn constant_time_compare_str(candidate: &str, expected: &str) -> bool {
    let cand_hash = hash_token(candidate);
    let exp_hash = hash_token(expected);
    constant_time_eq_32(&cand_hash, &exp_hash)
}

/// Generate a cryptographically secure Personal Access Token with standard `pat_` prefix.
pub fn generate_secure_pat() -> Result<String, ring::error::Unspecified> {
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = [0u8; 32];
    ring::rand::SecureRandom::fill(&rng, &mut bytes)?;
    let mut hex_str = String::with_capacity(PAT_PREFIX.len() + 64);
    hex_str.push_str(PAT_PREFIX);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(hex_str, "{b:02x}");
    }
    Ok(hex_str)
}

/// Configured API key or PAT descriptor stored in memory as a cryptographic hash.
#[derive(Clone)]
pub struct ConfiguredCredential {
    /// SHA-256 hash of the credential.
    pub token_hash: [u8; SHA256_DIGEST_LEN],
    /// Safe identifier prefix for tracing (e.g. "pat_a1b2..." or "api_key_01"), never the secret itself.
    pub key_id: String,
    /// Associated subject/user ID.
    pub subject: String,
    /// Associated email address if configured.
    pub email: Option<String>,
    /// Assigned roles.
    pub roles: Vec<AuthRole>,
    /// Authorized scopes.
    pub scopes: Vec<String>,
    /// Credential type (PAT vs ApiKey).
    pub auth_type: AuthType,
}

impl std::fmt::Debug for ConfiguredCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfiguredCredential")
            .field("key_id", &self.key_id)
            .field("subject", &self.subject)
            .field("token_hash", &"[REDACTED]")
            .field("roles", &self.roles)
            .field("scopes", &self.scopes)
            .field("auth_type", &self.auth_type)
            .finish()
    }
}

impl ConfiguredCredential {
    /// Create a new configured API key descriptor from plaintext key string.
    pub fn from_api_key(key: &str, key_id: impl Into<String>) -> Self {
        let key_id = key_id.into();
        Self {
            token_hash: hash_token(key),
            key_id: key_id.clone(),
            subject: format!("service:{key_id}"),
            email: None,
            roles: vec![AuthRole::Service],
            scopes: vec![
                "chat".to_string(),
                "models:read".to_string(),
                "routing:read".to_string(),
                "usage:read".to_string(),
            ],
            auth_type: AuthType::ApiKey,
        }
    }

    /// Create a new configured PAT descriptor from plaintext token string.
    pub fn from_pat(
        pat: &str,
        subject: impl Into<String>,
        email: Option<String>,
        roles: Vec<AuthRole>,
        scopes: Vec<String>,
    ) -> Self {
        let subject = subject.into();
        let key_id = if pat.starts_with(PAT_PREFIX) && pat.len() >= 10 {
            format!("{}...", &pat[..10])
        } else {
            "pat_credential".to_string()
        };

        Self {
            token_hash: hash_token(pat),
            key_id,
            subject,
            email,
            roles,
            scopes,
            auth_type: AuthType::PersonalAccessToken,
        }
    }
}

/// Thread-safe Personal Access Token and API Key validator.
#[derive(Clone, Default)]
pub struct PatValidator {
    credentials: Vec<ConfiguredCredential>,
}

impl PatValidator {
    /// Create a new empty PatValidator.
    pub fn new() -> Self {
        Self {
            credentials: Vec::new(),
        }
    }

    /// Create a PatValidator initialized from a list of configured API keys.
    pub fn from_api_keys(keys: &[String]) -> Self {
        let mut validator = Self::new();
        for (i, key) in keys.iter().enumerate() {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                validator.add_credential(ConfiguredCredential::from_api_key(
                    trimmed,
                    format!("key_{i:02}"),
                ));
            }
        }
        validator
    }

    /// Add a credential to the validator.
    pub fn add_credential(&mut self, cred: ConfiguredCredential) {
        self.credentials.push(cred);
    }

    /// Number of configured credentials.
    pub fn count(&self) -> usize {
        self.credentials.len()
    }

    /// Validate an incoming candidate token/key against all configured credentials.
    ///
    /// Evaluates in constant time using SHA-256 hash matching.
    /// Returns `Some(AuthPrincipal)` if matched, or `None` if invalid.
    pub fn validate(&self, candidate: &str, client_type: ClientType) -> Option<AuthPrincipal> {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return None;
        }

        let cand_hash = hash_token(trimmed);
        let mut matched: Option<&ConfiguredCredential> = None;

        // Iterate through all configured credentials to minimize early-exit timing discrepancies
        for cred in &self.credentials {
            if constant_time_eq_32(&cand_hash, &cred.token_hash) && matched.is_none() {
                matched = Some(cred);
            }
        }

        matched.map(|cred| {
            AuthPrincipal::new(
                &cred.subject,
                cred.auth_type,
                client_type,
                cred.email.clone(),
                cred.roles.clone(),
                cred.scopes.clone(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_comparison() {
        let secret = "super_secret_token_1234567890";
        assert!(constant_time_compare_str(secret, secret));
        assert!(!constant_time_compare_str(secret, "wrong_token"));
        assert!(!constant_time_compare_str(secret, ""));
    }

    #[test]
    fn test_generate_secure_pat_format() {
        let pat = generate_secure_pat().expect("Secure RNG failed");
        assert!(pat.starts_with(PAT_PREFIX));
        assert_eq!(pat.len(), PAT_PREFIX.len() + 64);
        assert!(
            pat[PAT_PREFIX.len()..]
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
    }

    #[test]
    fn test_pat_validator_matches_valid_key() {
        let valid_key = "secure_backend_api_key_test_99";
        let validator = PatValidator::from_api_keys(&[valid_key.to_string()]);

        let principal = validator.validate(valid_key, ClientType::CustomIde);
        assert!(principal.is_some());
        let p = principal.unwrap();
        assert_eq!(p.client_type, ClientType::CustomIde);
        assert_eq!(p.auth_type, AuthType::ApiKey);
        assert!(p.has_scope("chat"));
    }

    #[test]
    fn test_pat_validator_rejects_invalid_or_empty_key() {
        let valid_key = "secure_backend_api_key_test_99";
        let validator = PatValidator::from_api_keys(&[valid_key.to_string()]);

        assert!(
            validator
                .validate("wrong_key", ClientType::ApiClient)
                .is_none()
        );
        assert!(validator.validate("", ClientType::ApiClient).is_none());
        assert!(validator.validate("   ", ClientType::ApiClient).is_none());
    }
}
