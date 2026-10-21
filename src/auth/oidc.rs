//! Google OpenID Connect (OIDC) ID token verification.
//!
//! Validates RS256 JWT signatures against Google's public JWKS,
//! verifies `iss`, `aud`, and `exp` claims, and extracts authenticated
//! subject identities. Supports deterministic offline mock JWKS for testing.

use super::principal::{AuthPrincipal, AuthRole, AuthType, ClientType};
use jsonwebtoken::{Algorithm, DecodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Standard Google token issuers.
pub const GOOGLE_ISSUERS: &[&str] = &["https://accounts.google.com", "accounts.google.com"];

/// Default Google public JWKS discovery URL.
pub const DEFAULT_GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";

/// Maximum duration to cache JWKS before refreshing (1 hour).
pub const DEFAULT_JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Bounded timeout for fetching JWKS certificates.
pub const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors encountered during OIDC ID token validation.
#[derive(Debug)]
pub enum OidcError {
    MalformedToken(String),
    MissingKeyId,
    UnsupportedAlgorithm(Algorithm),
    UnknownKeyId(String),
    InvalidSignature(String),
    ExpiredToken {
        exp: u64,
        now: u64,
    },
    InvalidIssuer {
        expected: Vec<String>,
        actual: String,
    },
    InvalidAudience(String),
    EmptySubject,
    JwksFetchFailed(String),
    UnconfiguredClientIds,
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedToken(msg) => write!(f, "Malformed JWT structure: {msg}"),
            Self::MissingKeyId => write!(f, "Missing or invalid Key ID (kid) in token header"),
            Self::UnsupportedAlgorithm(alg) => {
                write!(
                    f,
                    "Unsupported token algorithm (expected RS256, got {alg:?})"
                )
            }
            Self::UnknownKeyId(kid) => write!(f, "Unknown or unverified signing key ID: {kid}"),
            Self::InvalidSignature(msg) => {
                write!(f, "Cryptographic signature verification failed: {msg}")
            }
            Self::ExpiredToken { exp, now } => {
                write!(f, "Token expired at {exp} (current time: {now})")
            }
            Self::InvalidIssuer { expected, actual } => {
                write!(
                    f,
                    "Invalid token issuer: expected one of {expected:?}, got {actual}"
                )
            }
            Self::InvalidAudience(msg) => {
                write!(f, "Invalid token audience: {msg}")
            }
            Self::EmptySubject => write!(f, "Token subject (sub) claim is empty or missing"),
            Self::JwksFetchFailed(msg) => {
                write!(
                    f,
                    "Failed to retrieve JWKS certificates from remote endpoint: {msg}"
                )
            }
            Self::UnconfiguredClientIds => {
                write!(f, "No Google Client IDs configured in application")
            }
        }
    }
}

impl std::error::Error for OidcError {}

/// Standard Google OpenID Connect ID token claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleClaims {
    /// Token issuer (e.g. `https://accounts.google.com`).
    pub iss: String,
    /// Unique Google user subject identifier.
    pub sub: String,
    /// Audience: Google OAuth 2.0 Client ID(s).
    pub aud: serde_json::Value,
    /// Expiration timestamp in seconds since Unix epoch.
    pub exp: u64,
    /// Issued-at timestamp in seconds since Unix epoch.
    #[serde(default)]
    pub iat: Option<u64>,
    /// Not-before timestamp in seconds since Unix epoch.
    #[serde(default)]
    pub nbf: Option<u64>,
    /// Authorized party (OAuth client ID of the presenter).
    #[serde(default)]
    pub azp: Option<String>,
    /// User's email address.
    #[serde(default)]
    pub email: Option<String>,
    /// Whether user's email address is verified by Google.
    #[serde(default)]
    pub email_verified: Option<bool>,
    /// User's full name.
    #[serde(default)]
    pub name: Option<String>,
    /// URL to user's profile picture.
    #[serde(default)]
    pub picture: Option<String>,
}

impl Default for GoogleClaims {
    fn default() -> Self {
        Self {
            iss: "https://accounts.google.com".to_string(),
            sub: String::new(),
            aud: serde_json::Value::Null,
            exp: 0,
            iat: None,
            nbf: None,
            azp: None,
            email: None,
            email_verified: None,
            name: None,
            picture: None,
        }
    }
}

impl GoogleClaims {
    /// Check whether this token's audience matches any of the configured client IDs.
    pub fn matches_audience(&self, allowed_audiences: &[String]) -> bool {
        if allowed_audiences.is_empty() {
            return false;
        }

        match &self.aud {
            serde_json::Value::String(s) => allowed_audiences.iter().any(|aud| aud == s),
            serde_json::Value::Array(arr) => arr.iter().filter_map(|v| v.as_str()).any(|aud_str| {
                allowed_audiences
                    .iter()
                    .any(|configured| configured == aud_str)
            }),
            _ => false,
        }
    }
}

/// JSON Web Key structure returned by JWKS endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct JwkKey {
    pub kty: String,
    pub kid: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
}

/// JWKS certificate payload.
#[derive(Debug, Clone, Deserialize)]
pub struct JwksResponse {
    pub keys: Vec<JwkKey>,
}

/// Provider trait for resolving RSA decoding keys by Key ID (`kid`).
pub trait JwksProvider: Send + Sync {
    /// Retrieve the RSA decoding key for a given `kid`.
    fn get_decoding_key<'a>(
        &'a self,
        kid: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<DecodingKey, OidcError>> + Send + 'a>>;
}

/// In-memory cache entry for a JWKS key.
struct CachedKeys {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Instant,
}

/// Production HTTP JWKS Provider fetching keys from Google certificate endpoints.
pub struct HttpJwksProvider {
    jwks_url: String,
    http_client: reqwest::Client,
    cache: RwLock<Option<CachedKeys>>,
    last_fetch_attempt: RwLock<Option<Instant>>,
    cache_ttl: Duration,
}

impl HttpJwksProvider {
    /// Maximum allowed JWKS response size in bytes (256 KB).
    pub const MAX_JWKS_RESPONSE_BYTES: usize = 262_144;
    /// Minimum interval between outbound refresh attempts on unknown kid (10 seconds).
    pub const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

    /// Create a new HttpJwksProvider with the specified JWKS URL and cache TTL.
    pub fn new(jwks_url: Option<String>, cache_ttl: Option<Duration>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(JWKS_FETCH_TIMEOUT)
            .build()
            .unwrap_or_default();

        Self {
            jwks_url: jwks_url.unwrap_or_else(|| DEFAULT_GOOGLE_JWKS_URL.to_string()),
            http_client,
            cache: RwLock::new(None),
            last_fetch_attempt: RwLock::new(None),
            cache_ttl: cache_ttl.unwrap_or(DEFAULT_JWKS_CACHE_TTL),
        }
    }

    /// Fetch keys from the remote JWKS URL and update the cache.
    async fn fetch_and_cache(&self) -> Result<HashMap<String, DecodingKey>, OidcError> {
        {
            let mut attempt_lock = self.last_fetch_attempt.write().await;
            *attempt_lock = Some(Instant::now());
        }

        let response = self
            .http_client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|err| OidcError::JwksFetchFailed(err.to_string()))?;

        if !response.status().is_success() {
            return Err(OidcError::JwksFetchFailed(format!(
                "HTTP {} from {}",
                response.status(),
                self.jwks_url
            )));
        }

        if let Some(content_length) = response.content_length()
            && content_length as usize > Self::MAX_JWKS_RESPONSE_BYTES
        {
            return Err(OidcError::JwksFetchFailed(format!(
                "JWKS content-length ({content_length}) exceeds maximum allowed 256 KB"
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|err| OidcError::JwksFetchFailed(err.to_string()))?;

        if bytes.len() > Self::MAX_JWKS_RESPONSE_BYTES {
            return Err(OidcError::JwksFetchFailed(format!(
                "JWKS response size ({} bytes) exceeds maximum allowed 256 KB",
                bytes.len()
            )));
        }

        let jwks: JwksResponse = serde_json::from_slice(&bytes)
            .map_err(|err| OidcError::JwksFetchFailed(err.to_string()))?;

        let mut key_map = HashMap::new();
        for key in jwks.keys {
            if key.kty != "RSA" {
                continue;
            }
            if let (Some(kid), Some(n), Some(e)) = (key.kid, key.n, key.e)
                && let Ok(decoding_key) = DecodingKey::from_rsa_components(&n, &e)
            {
                key_map.insert(kid, decoding_key);
            }
        }

        let mut lock = self.cache.write().await;
        *lock = Some(CachedKeys {
            keys: key_map.clone(),
            fetched_at: Instant::now(),
        });

        Ok(key_map)
    }
}

impl JwksProvider for HttpJwksProvider {
    fn get_decoding_key<'a>(
        &'a self,
        kid: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<DecodingKey, OidcError>> + Send + 'a>> {
        Box::pin(async move {
            // Check read cache first
            {
                let read_lock = self.cache.read().await;
                if let Some(cached) = &*read_lock
                    && cached.fetched_at.elapsed() < self.cache_ttl
                    && let Some(key) = cached.keys.get(kid)
                {
                    return Ok(key.clone());
                }
            }

            // Check refresh throttle: avoid flooding Google if a recent attempt already failed
            {
                let attempt_lock = self.last_fetch_attempt.read().await;
                if let Some(last_attempt) = *attempt_lock
                    && last_attempt.elapsed() < Self::MIN_REFRESH_INTERVAL
                {
                    // Return UnknownKeyId rather than pounding upstream endpoint
                    return Err(OidcError::UnknownKeyId(kid.to_string()));
                }
            }

            // Refresh from remote
            let fresh_keys = self.fetch_and_cache().await?;
            fresh_keys
                .get(kid)
                .cloned()
                .ok_or_else(|| OidcError::UnknownKeyId(kid.to_string()))
        })
    }
}

/// Deterministic in-memory Mock JWKS Provider for offline testing without network access.
#[derive(Clone, Default)]
pub struct MockJwksProvider {
    keys: HashMap<String, DecodingKey>,
}

impl MockJwksProvider {
    pub fn new() -> Self {
        Self {
            keys: HashMap::new(),
        }
    }

    /// Register a decoding key with a given Key ID (`kid`).
    pub fn add_key(&mut self, kid: impl Into<String>, key: DecodingKey) {
        self.keys.insert(kid.into(), key);
    }
}

impl JwksProvider for MockJwksProvider {
    fn get_decoding_key<'a>(
        &'a self,
        kid: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<DecodingKey, OidcError>> + Send + 'a>> {
        let result = self
            .keys
            .get(kid)
            .cloned()
            .ok_or_else(|| OidcError::UnknownKeyId(kid.to_string()));
        Box::pin(async move { result })
    }
}

/// Google OpenID Connect ID token validator.
pub struct GoogleOidcValidator {
    jwks_provider: Arc<dyn JwksProvider>,
    client_ids: Vec<String>,
    clock_skew_secs: u64,
}

impl GoogleOidcValidator {
    /// Create a new GoogleOidcValidator with the given JWKS provider and configured client IDs.
    pub fn new(jwks_provider: Arc<dyn JwksProvider>, client_ids: Vec<String>) -> Self {
        Self {
            jwks_provider,
            client_ids,
            clock_skew_secs: 60, // 60 seconds clock skew per RFC 7519
        }
    }

    /// Configure clock skew in seconds.
    pub fn with_clock_skew_secs(mut self, skew: u64) -> Self {
        self.clock_skew_secs = skew;
        self
    }

    /// Check if at least one Google Client ID is configured.
    pub fn is_configured(&self) -> bool {
        !self.client_ids.is_empty()
    }

    /// Cryptographically validate an incoming Google OIDC ID token.
    ///
    /// Validates signature, issuer, audience, and expiration before constructing
    /// an authenticated `AuthPrincipal`.
    pub async fn validate(
        &self,
        token: &str,
        client_type: ClientType,
    ) -> Result<AuthPrincipal, OidcError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(OidcError::MalformedToken("Token is empty".to_string()));
        }

        if self.client_ids.is_empty() {
            return Err(OidcError::UnconfiguredClientIds);
        }

        // 1. Decode token header to inspect alg and kid
        let header: Header = jsonwebtoken::decode_header(token)
            .map_err(|err| OidcError::MalformedToken(err.to_string()))?;

        if header.alg != Algorithm::RS256 {
            return Err(OidcError::UnsupportedAlgorithm(header.alg));
        }

        let kid = header.kid.ok_or(OidcError::MissingKeyId)?;

        // 2. Retrieve RSA decoding key from JWKS provider
        let decoding_key = self.jwks_provider.get_decoding_key(&kid).await?;

        // 3. Configure strict validation
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(GOOGLE_ISSUERS);
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);
        validation.leeway = self.clock_skew_secs;
        validation.validate_nbf = true;
        // We validate audience explicitly below to support array-based or multi-client aud
        validation.validate_aud = false;

        // 4. Verify cryptographic signature and claims
        let token_data = jsonwebtoken::decode::<GoogleClaims>(token, &decoding_key, &validation)
            .map_err(|err| match err.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    OidcError::ExpiredToken { exp: 0, now }
                }
                jsonwebtoken::errors::ErrorKind::InvalidIssuer => OidcError::InvalidIssuer {
                    expected: GOOGLE_ISSUERS.iter().map(|s| s.to_string()).collect(),
                    actual: "invalid".to_string(),
                },
                _ => OidcError::InvalidSignature(err.to_string()),
            })?;

        let claims = token_data.claims;

        // 5. Explicit validation of audience
        if !claims.matches_audience(&self.client_ids) {
            return Err(OidcError::InvalidAudience(format!(
                "Token audience does not match configured Google client IDs ({:?})",
                self.client_ids
            )));
        }

        // 6. Validate authorized party (azp) if present
        if let Some(azp) = &claims.azp
            && !self.client_ids.contains(azp)
            && !claims.matches_audience(&self.client_ids)
        {
            return Err(OidcError::InvalidAudience(format!(
                "Token azp '{azp}' does not match configured Google client IDs"
            )));
        }

        // 7. Validate issued-at timestamp (iat) is not in the future beyond clock skew
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Some(iat) = claims.iat
            && iat > now.saturating_add(self.clock_skew_secs)
        {
            return Err(OidcError::InvalidSignature(
                "Token issued in the future (iat exceeds current time + clock skew)".to_string(),
            ));
        }

        // 8. Validate subject is non-empty
        let sub = claims.sub.trim();
        if sub.is_empty() {
            return Err(OidcError::EmptySubject);
        }

        // 9. Construct AuthPrincipal
        Ok(AuthPrincipal::new(
            format!("google:{sub}"),
            AuthType::GoogleOidc,
            client_type,
            claims.email,
            vec![AuthRole::User],
            vec![
                "chat".to_string(),
                "models:read".to_string(),
                "routing:read".to_string(),
                "usage:read".to_string(),
            ],
        ))
    }
}
