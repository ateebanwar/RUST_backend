//! Dynamic Personal Access Token & Credential Lifecycle Manager.
//!
//! Conforms strictly to zero-database architecture:
//! - Operates 100% in memory with zero SQLite, zero disk files, zero persistence.
//! - Cryptographically secure token generation (`ring::rand`).
//! - One-way SHA-256 storage hashing — secrets are never stored in plaintext.
//! - Single-exposure token semantics — plaintext returned once upon creation or rotation.
//! - Masked credential representations (`pat_a1b2...ef01`) for list/inspection endpoints.
//! - Strict resource ownership and tenant isolation (cross-user access/revocation blocked).
//! - Privilege escalation defense — callers cannot grant scopes exceeding their own.
//! - Immediate, atomic revocation and rotation invalidation.

use super::pat::{constant_time_eq_32, generate_secure_pat, hash_token};
use super::principal::{AuthPrincipal, AuthRole, AuthType, ClientType};
use crate::errors::AppError;
use crate::security::mask_credential;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Non-secret metadata representing a configured or dynamic credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialMetadata {
    /// Unique credential identifier (e.g. `cred_01h...`).
    pub id: String,
    /// Human-readable label for the credential.
    pub name: String,
    /// The subject identity who owns this credential.
    pub owner_subject: String,
    /// Safe masked representation (e.g. `pat_1234...ef56`), never the full secret.
    pub masked_token: String,
    /// Assigned authorization roles.
    pub roles: Vec<AuthRole>,
    /// Authorized scopes.
    pub scopes: Vec<String>,
    /// Creation epoch timestamp in seconds.
    pub created_at: u64,
    /// Optional expiration epoch timestamp in seconds.
    pub expires_at: Option<u64>,
    /// Whether this credential has been revoked.
    pub revoked: bool,
    /// Optional revocation epoch timestamp in seconds.
    pub revoked_at: Option<u64>,
}

/// Internal in-memory representation pairing metadata with SHA-256 token hash.
#[derive(Clone)]
struct CachedCredential {
    metadata: CredentialMetadata,
    token_hash: [u8; 32],
}

/// Request payload to generate a new Personal Access Token.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTokenRequest {
    /// Descriptive name for the token.
    pub name: String,
    /// Optional target owner subject (Admin only; non-admin specifying another subject is rejected with 403).
    #[serde(default)]
    pub owner_subject: Option<String>,
    /// Optional subset of scopes to grant (cannot exceed caller's scopes).
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Optional expiration duration in seconds.
    #[serde(default)]
    pub expires_in_secs: Option<u64>,
}

/// Outbound response for token creation, revealing the plaintext token ONCE.
#[derive(Debug, Clone, Serialize)]
pub struct CreateTokenResponse {
    /// The plaintext token value. Must be saved by the client immediately.
    pub token: String,
    /// Non-secret credential metadata.
    pub metadata: CredentialMetadata,
}

/// Outbound response for token rotation, revealing the new plaintext token ONCE.
#[derive(Debug, Clone, Serialize)]
pub struct RotateTokenResponse {
    /// The new plaintext token value.
    pub new_token: String,
    /// Updated credential metadata.
    pub metadata: CredentialMetadata,
}

/// Thread-safe in-memory storage engine for dynamic Personal Access Tokens.
#[derive(Clone)]
pub struct CredentialStore {
    cache: Arc<RwLock<HashMap<String, CachedCredential>>>,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore {
    /// Initialize a new in-memory CredentialStore.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Compatibility constructor for tests or callers previously passing connection handles.
    pub fn new_with_conn<T>(_conn: T) -> Result<Self, AppError> {
        Ok(Self::new())
    }

    /// Create a new Personal Access Token for the authenticated principal.
    pub fn create_token(
        &self,
        caller: &AuthPrincipal,
        req: CreateTokenRequest,
    ) -> Result<CreateTokenResponse, AppError> {
        let name_trimmed = req.name.trim();
        if name_trimmed.is_empty() || name_trimmed.len() > 128 {
            return Err(AppError::InvalidRequest(
                "Credential name must be between 1 and 128 characters".to_string(),
            ));
        }

        // Scope privilege escalation check:
        // A non-admin user cannot grant scopes they do not possess themselves.
        if !caller.is_admin() {
            for s in req.scopes.as_deref().unwrap_or_default() {
                if !caller.has_scope(s) {
                    return Err(AppError::Forbidden(format!(
                        "Privilege escalation denied: caller lacks scope '{s}' and cannot grant it"
                    )));
                }
            }
        }

        // Target subject determination:
        // Admin callers can create tokens on behalf of another subject.
        // Non-admin callers cannot create tokens for anyone other than themselves.
        let target_subject = match req.owner_subject {
            Some(target) => {
                let target_trimmed = target.trim();
                if target_trimmed.is_empty() {
                    return Err(AppError::InvalidRequest(
                        "owner_subject cannot be empty".to_string(),
                    ));
                }
                if target_trimmed != caller.subject && !caller.is_admin() {
                    return Err(AppError::Forbidden(
                        "Only administrators may create credentials on behalf of other subjects"
                            .to_string(),
                    ));
                }
                target_trimmed.to_string()
            }
            None => caller.subject.clone(),
        };

        let granted_scopes = req.scopes.unwrap_or_else(|| caller.scopes.clone());
        let granted_roles = if caller.is_admin() && target_subject != caller.subject {
            vec![AuthRole::User]
        } else {
            caller.roles.clone()
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let expires_at = req.expires_in_secs.map(|secs| now.saturating_add(secs));

        // Generate 64-hex-character token
        let token_plaintext = generate_secure_pat().map_err(|_| {
            AppError::InternalError("Failed to generate secure random token".to_string())
        })?;
        let token_hash = hash_token(&token_plaintext);
        let rand_id = generate_secure_pat().map_err(|_| {
            AppError::InternalError("Failed to generate secure random ID".to_string())
        })?;
        let cred_id = format!("cred_{}", &rand_id[..20]);
        let masked = mask_credential(&token_plaintext);

        let metadata = CredentialMetadata {
            id: cred_id.clone(),
            name: name_trimmed.to_string(),
            owner_subject: target_subject,
            masked_token: masked,
            roles: granted_roles,
            scopes: granted_scopes,
            created_at: now,
            expires_at,
            revoked: false,
            revoked_at: None,
        };

        let cached = CachedCredential {
            metadata: metadata.clone(),
            token_hash,
        };

        let mut cache_lock = self
            .cache
            .write()
            .map_err(|e| AppError::InternalError(format!("Cache write lock error: {e}")))?;
        cache_lock.insert(cred_id, cached);

        Ok(CreateTokenResponse {
            token: token_plaintext,
            metadata,
        })
    }

    /// List credentials belonging to the caller (or all credentials for admin).
    pub fn list_tokens(
        &self,
        caller: &AuthPrincipal,
        target_subject: Option<&str>,
    ) -> Result<Vec<CredentialMetadata>, AppError> {
        let cache_lock = self
            .cache
            .read()
            .map_err(|e| AppError::InternalError(format!("Cache read lock error: {e}")))?;

        let mut results = Vec::new();
        for item in cache_lock.values() {
            if let Some(target) = target_subject {
                if item.metadata.owner_subject == target
                    && (caller.is_admin() || caller.subject == target)
                {
                    results.push(item.metadata.clone());
                }
            } else if caller.is_admin() || item.metadata.owner_subject == caller.subject {
                results.push(item.metadata.clone());
            }
        }

        results.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        Ok(results)
    }

    /// Get non-secret metadata for a specific credential ID with IDOR protection.
    pub fn get_token_metadata(
        &self,
        caller: &AuthPrincipal,
        token_id: &str,
    ) -> Result<CredentialMetadata, AppError> {
        let cache_lock = self
            .cache
            .read()
            .map_err(|e| AppError::InternalError(format!("Cache read lock error: {e}")))?;

        match cache_lock.get(token_id) {
            Some(item) => {
                if !caller.is_admin() && item.metadata.owner_subject != caller.subject {
                    return Err(AppError::NotFound(format!(
                        "Credential '{token_id}' not found"
                    )));
                }
                Ok(item.metadata.clone())
            }
            None => Err(AppError::NotFound(format!(
                "Credential '{token_id}' not found"
            ))),
        }
    }

    /// Alias for get_token_metadata.
    pub fn get_token(
        &self,
        caller: &AuthPrincipal,
        token_id: &str,
    ) -> Result<CredentialMetadata, AppError> {
        self.get_token_metadata(caller, token_id)
    }

    /// Rotate an existing Personal Access Token.
    pub fn rotate_token(
        &self,
        caller: &AuthPrincipal,
        token_id: &str,
    ) -> Result<RotateTokenResponse, AppError> {
        let mut cache_lock = self
            .cache
            .write()
            .map_err(|e| AppError::InternalError(format!("Cache write lock error: {e}")))?;

        let item = match cache_lock.get_mut(token_id) {
            Some(i) => i,
            None => {
                return Err(AppError::NotFound(format!(
                    "Credential '{token_id}' not found"
                )));
            }
        };

        if !caller.is_admin() && item.metadata.owner_subject != caller.subject {
            return Err(AppError::NotFound(format!(
                "Credential '{token_id}' not found"
            )));
        }

        if item.metadata.revoked {
            return Err(AppError::InvalidRequest(format!(
                "Credential '{token_id}' has already been revoked and cannot be rotated"
            )));
        }

        let new_token = generate_secure_pat().map_err(|_| {
            AppError::InternalError("Failed to generate secure random token".to_string())
        })?;
        let new_hash = hash_token(&new_token);
        let new_masked = mask_credential(&new_token);

        item.token_hash = new_hash;
        item.metadata.masked_token = new_masked;

        Ok(RotateTokenResponse {
            new_token,
            metadata: item.metadata.clone(),
        })
    }

    /// Revoke a Personal Access Token immediately.
    pub fn revoke_token(&self, caller: &AuthPrincipal, token_id: &str) -> Result<(), AppError> {
        let mut cache_lock = self
            .cache
            .write()
            .map_err(|e| AppError::InternalError(format!("Cache write lock error: {e}")))?;

        let item = match cache_lock.get_mut(token_id) {
            Some(i) => i,
            None => {
                return Err(AppError::NotFound(format!(
                    "Credential '{token_id}' not found"
                )));
            }
        };

        if !caller.is_admin() && item.metadata.owner_subject != caller.subject {
            return Err(AppError::NotFound(format!(
                "Credential '{token_id}' not found"
            )));
        }

        if item.metadata.revoked {
            return Ok(());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        item.metadata.revoked = true;
        item.metadata.revoked_at = Some(now);

        Ok(())
    }

    /// Validate an incoming token in constant time.
    pub fn validate_token(
        &self,
        token_plaintext: &str,
        candidate_client_type: ClientType,
    ) -> Option<AuthPrincipal> {
        let incoming_hash = hash_token(token_plaintext);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let cache_lock = self.cache.read().ok()?;

        for item in cache_lock.values() {
            if constant_time_eq_32(&item.token_hash, &incoming_hash) {
                if item.metadata.revoked {
                    return None;
                }
                if item.metadata.expires_at.is_some_and(|exp| now > exp) {
                    return None;
                }

                let client_type = if candidate_client_type != ClientType::CustomIde {
                    candidate_client_type
                } else if item.metadata.name.to_lowercase().contains("vscode") {
                    ClientType::VscodeExtension
                } else if item.metadata.name.to_lowercase().contains("flutter")
                    || item.metadata.name.to_lowercase().contains("mobile")
                {
                    ClientType::Flutter
                } else {
                    candidate_client_type
                };

                return Some(AuthPrincipal::new(
                    item.metadata.owner_subject.clone(),
                    AuthType::PersonalAccessToken,
                    client_type,
                    None,
                    item.metadata.roles.clone(),
                    item.metadata.scopes.clone(),
                ));
            }
        }

        None
    }
}
