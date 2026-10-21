//! Dynamic Personal Access Token & Credential Lifecycle Manager.
//!
//! Enforces:
//! 1. Cryptographically secure token generation (`ring::rand`).
//! 2. One-way SHA-256 storage hashing — secrets are never persisted in plaintext.
//! 3. Single-exposure token semantics — plaintext returned once upon creation or rotation.
//! 4. Masked credential representations (`pat_a1b2...ef01`) for list/inspection endpoints.
//! 5. Strict resource ownership and tenant isolation (cross-user access/revocation blocked).
//! 6. Privilege escalation defense — callers cannot grant scopes exceeding their own.
//! 7. Immediate, atomic revocation and rotation invalidation.

use super::pat::{constant_time_eq_32, generate_secure_pat, hash_token};
use super::principal::{AuthPrincipal, AuthRole, AuthType, ClientType};
use crate::errors::AppError;
use crate::security::mask_credential;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
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

/// Thread-safe storage engine for dynamic Personal Access Tokens.
#[derive(Clone)]
pub struct CredentialStore {
    conn: Arc<Mutex<Connection>>,
    cache: Arc<RwLock<HashMap<String, CachedCredential>>>,
}

impl CredentialStore {
    /// Initialize with an existing SQLite connection handle.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, AppError> {
        let store = Self {
            conn,
            cache: Arc::new(RwLock::new(HashMap::new())),
        };
        store.run_migrations()?;
        store.refresh_cache()?;
        Ok(store)
    }

    /// Initialize SQLite table schema and indexes.
    fn run_migrations(&self) -> Result<(), AppError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AppError::InternalError(format!("CredentialStore lock error: {e}")))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS credentials (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                owner_subject TEXT NOT NULL,
                token_hash BLOB NOT NULL,
                masked_token TEXT NOT NULL,
                roles_json TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                revoked INTEGER NOT NULL DEFAULT 0,
                revoked_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_credentials_owner ON credentials(owner_subject);
            CREATE INDEX IF NOT EXISTS idx_credentials_revoked ON credentials(revoked);",
        )
        .map_err(|e| {
            AppError::InternalError(format!("Failed to create credentials schema: {e}"))
        })?;

        Ok(())
    }

    /// Refresh in-memory validation cache from database.
    fn refresh_cache(&self) -> Result<(), AppError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AppError::InternalError(format!("CredentialStore lock error: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, name, owner_subject, token_hash, masked_token, roles_json, scopes_json, created_at, expires_at, revoked, revoked_at FROM credentials",
            )
            .map_err(|e| AppError::InternalError(format!("Failed to prepare cache load: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let owner_subject: String = row.get(2)?;
                let token_hash_bytes: Vec<u8> = row.get(3)?;
                let masked_token: String = row.get(4)?;
                let roles_json: String = row.get(5)?;
                let scopes_json: String = row.get(6)?;
                let created_at: i64 = row.get(7)?;
                let expires_at: Option<i64> = row.get(8)?;
                let revoked_int: i64 = row.get(9)?;
                let revoked_at: Option<i64> = row.get(10)?;

                let mut token_hash = [0u8; 32];
                if token_hash_bytes.len() == 32 {
                    token_hash.copy_from_slice(&token_hash_bytes);
                }

                let roles: Vec<AuthRole> = serde_json::from_str(&roles_json).unwrap_or_default();
                let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();

                Ok(CachedCredential {
                    metadata: CredentialMetadata {
                        id,
                        name,
                        owner_subject,
                        masked_token,
                        roles,
                        scopes,
                        created_at: created_at as u64,
                        expires_at: expires_at.map(|v| v as u64),
                        revoked: revoked_int != 0,
                        revoked_at: revoked_at.map(|v| v as u64),
                    },
                    token_hash,
                })
            })
            .map_err(|e| AppError::InternalError(format!("Failed to query credentials: {e}")))?;

        let mut new_cache = HashMap::new();
        for item in rows.flatten() {
            new_cache.insert(item.metadata.id.clone(), item);
        }

        let mut cache_lock = self
            .cache
            .write()
            .map_err(|e| AppError::InternalError(format!("Cache write lock poisoned: {e}")))?;
        *cache_lock = new_cache;

        Ok(())
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
        let granted_scopes = if let Some(requested_scopes) = req.scopes {
            if !caller.is_admin() && !caller.has_scope("*") {
                for s in &requested_scopes {
                    if !caller.has_scope(s) {
                        return Err(AppError::Forbidden(format!(
                            "Privilege escalation denied: Caller cannot grant scope '{s}' which they do not possess"
                        )));
                    }
                }
            }
            requested_scopes
        } else {
            // Default to caller's scopes if not specified
            caller.scopes.clone()
        };

        let raw_token = generate_secure_pat().map_err(|_| {
            AppError::InternalError("Failed to generate secure random token".to_string())
        })?;
        let token_hash = hash_token(&raw_token);
        let masked = mask_credential(&raw_token);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let expires_at = req.expires_in_secs.map(|secs| now.saturating_add(secs));

        let cred_id = format!("cred_{:016x}", now ^ (token_hash[0] as u64));

        // Subject ownership & IDOR resolution
        let owner_subject = match req.owner_subject {
            Some(target) => {
                let trimmed = target.trim();
                if trimmed.is_empty() {
                    caller.subject.clone()
                } else {
                    if !caller.is_admin() && trimmed != caller.subject {
                        return Err(AppError::Forbidden(format!(
                            "IDOR violation: Principal '{}' is forbidden from creating tokens for subject '{trimmed}'",
                            caller.subject
                        )));
                    }
                    trimmed.to_string()
                }
            }
            None => caller.subject.clone(),
        };

        let assigned_roles = if caller.is_admin() && owner_subject != caller.subject {
            vec![AuthRole::User]
        } else {
            caller.roles.clone()
        };

        let metadata = CredentialMetadata {
            id: cred_id.clone(),
            name: name_trimmed.to_string(),
            owner_subject,
            masked_token: masked,
            roles: assigned_roles,
            scopes: granted_scopes,
            created_at: now,
            expires_at,
            revoked: false,
            revoked_at: None,
        };

        // Write to database
        {
            let conn = self
                .conn
                .lock()
                .map_err(|e| AppError::InternalError(format!("Database lock error: {e}")))?;

            let roles_json = serde_json::to_string(&metadata.roles)
                .map_err(|e| AppError::InternalError(e.to_string()))?;
            let scopes_json = serde_json::to_string(&metadata.scopes)
                .map_err(|e| AppError::InternalError(e.to_string()))?;

            conn.execute(
                "INSERT INTO credentials (
                    id, name, owner_subject, token_hash, masked_token, roles_json, scopes_json, created_at, expires_at, revoked, revoked_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, NULL)",
                params![
                    metadata.id,
                    metadata.name,
                    metadata.owner_subject,
                    token_hash.as_slice(),
                    metadata.masked_token,
                    roles_json,
                    scopes_json,
                    metadata.created_at as i64,
                    metadata.expires_at.map(|v| v as i64),
                ],
            )
            .map_err(|e| AppError::InternalError(format!("Failed to insert credential: {e}")))?;
        }

        // Update in-memory cache
        {
            let mut cache_lock = self
                .cache
                .write()
                .map_err(|e| AppError::InternalError(format!("Cache lock error: {e}")))?;
            cache_lock.insert(
                metadata.id.clone(),
                CachedCredential {
                    metadata: metadata.clone(),
                    token_hash,
                },
            );
        }

        Ok(CreateTokenResponse {
            token: raw_token,
            metadata,
        })
    }

    /// List credentials with tenant isolation.
    ///
    /// Standard users see strictly their own credentials.
    /// Attempting to view another user's credentials returns 403 Forbidden.
    pub fn list_tokens(
        &self,
        caller: &AuthPrincipal,
        filter_owner: Option<&str>,
    ) -> Result<Vec<CredentialMetadata>, AppError> {
        let target_subject = match filter_owner {
            Some(requested) => {
                if !caller.is_admin() && requested != caller.subject {
                    return Err(AppError::Forbidden(format!(
                        "IDOR violation: Principal '{}' is forbidden from listing credentials for subject '{requested}'",
                        caller.subject
                    )));
                }
                Some(requested)
            }
            None => {
                if caller.is_admin() {
                    None // Admin viewing all
                } else {
                    Some(caller.subject.as_str()) // User viewing own
                }
            }
        };

        let cache_lock = self
            .cache
            .read()
            .map_err(|e| AppError::InternalError(format!("Cache lock error: {e}")))?;

        let mut results: Vec<CredentialMetadata> = cache_lock
            .values()
            .filter(|c| {
                if let Some(target) = target_subject {
                    c.metadata.owner_subject == target
                } else {
                    true
                }
            })
            .map(|c| c.metadata.clone())
            .collect();

        results.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        Ok(results)
    }

    /// Retrieve metadata for an individual credential with ownership enforcement.
    pub fn get_token(
        &self,
        caller: &AuthPrincipal,
        token_id: &str,
    ) -> Result<CredentialMetadata, AppError> {
        let cache_lock = self
            .cache
            .read()
            .map_err(|e| AppError::InternalError(format!("Cache lock error: {e}")))?;

        let cred = cache_lock
            .get(token_id)
            .ok_or_else(|| AppError::NotFound(format!("Credential '{token_id}' not found")))?;

        if !caller.is_admin() && cred.metadata.owner_subject != caller.subject {
            // Return 404 to prevent resource existence enumeration (IDOR mitigation)
            return Err(AppError::NotFound(format!(
                "Credential '{token_id}' not found"
            )));
        }

        Ok(cred.metadata.clone())
    }

    /// Revoke a credential immediately.
    pub fn revoke_token(&self, caller: &AuthPrincipal, token_id: &str) -> Result<(), AppError> {
        // Enforce ownership
        let cred_meta = self.get_token(caller, token_id)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Update database
        {
            let conn = self
                .conn
                .lock()
                .map_err(|e| AppError::InternalError(format!("Database lock error: {e}")))?;

            conn.execute(
                "UPDATE credentials SET revoked = 1, revoked_at = ?1 WHERE id = ?2",
                params![now as i64, cred_meta.id],
            )
            .map_err(|e| AppError::InternalError(format!("Failed to revoke credential: {e}")))?;
        }

        // Atomically update in-memory cache
        {
            let mut cache_lock = self
                .cache
                .write()
                .map_err(|e| AppError::InternalError(format!("Cache lock error: {e}")))?;
            if let Some(c) = cache_lock.get_mut(&cred_meta.id) {
                c.metadata.revoked = true;
                c.metadata.revoked_at = Some(now);
            }
        }

        Ok(())
    }

    /// Rotate a credential: atomically revokes old token and generates new plaintext token once.
    pub fn rotate_token(
        &self,
        caller: &AuthPrincipal,
        token_id: &str,
    ) -> Result<RotateTokenResponse, AppError> {
        let cred_meta = self.get_token(caller, token_id)?;

        let new_raw_token = generate_secure_pat().map_err(|_| {
            AppError::InternalError("Failed to generate secure random token".to_string())
        })?;
        let new_hash = hash_token(&new_raw_token);
        let new_masked = mask_credential(&new_raw_token);

        // Update database
        {
            let conn = self
                .conn
                .lock()
                .map_err(|e| AppError::InternalError(format!("Database lock error: {e}")))?;

            conn.execute(
                "UPDATE credentials SET token_hash = ?1, masked_token = ?2, revoked = 0, revoked_at = NULL WHERE id = ?3",
                params![new_hash.as_slice(), new_masked, cred_meta.id],
            )
            .map_err(|e| AppError::InternalError(format!("Failed to rotate credential: {e}")))?;
        }

        let mut updated_meta = cred_meta;
        updated_meta.masked_token = new_masked;
        updated_meta.revoked = false;
        updated_meta.revoked_at = None;

        // Update in-memory cache
        {
            let mut cache_lock = self
                .cache
                .write()
                .map_err(|e| AppError::InternalError(format!("Cache lock error: {e}")))?;
            if let Some(c) = cache_lock.get_mut(&updated_meta.id) {
                c.metadata = updated_meta.clone();
                c.token_hash = new_hash;
            }
        }

        Ok(RotateTokenResponse {
            new_token: new_raw_token,
            metadata: updated_meta,
        })
    }

    /// Validate a candidate token against active credentials in constant time.
    pub fn validate_token(
        &self,
        candidate: &str,
        client_type: ClientType,
    ) -> Option<AuthPrincipal> {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return None;
        }

        let cand_hash = hash_token(trimmed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let cache_lock = self.cache.read().ok()?;

        let mut matched: Option<CredentialMetadata> = None;

        // Iterate through all cached credentials to preserve constant-time verification properties
        for cred in cache_lock.values() {
            if constant_time_eq_32(&cand_hash, &cred.token_hash) {
                // Check revocation and expiration
                if !cred.metadata.revoked {
                    if let Some(exp) = cred.metadata.expires_at {
                        if exp > now {
                            matched = Some(cred.metadata.clone());
                        }
                    } else {
                        matched = Some(cred.metadata.clone());
                    }
                }
            }
        }

        matched.map(|m| {
            AuthPrincipal::new(
                m.owner_subject,
                AuthType::PersonalAccessToken,
                client_type,
                None,
                m.roles,
                m.scopes,
            )
        })
    }
}
