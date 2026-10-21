//! Unified Authentication Service coordinating OIDC and PAT/API-Key validation.
//!
//! Enforces credentials precedence, route classification, operational telemetry
//! protection, fail-closed production rules, and downstream identity propagation.

use super::oidc::{GoogleOidcValidator, HttpJwksProvider, JwksProvider};
use super::pat::PatValidator;
use super::principal::{AuthPrincipal, ClientType};
use crate::config::{AppConfig, Environment};
use crate::errors::AppError;
use axum::http::HeaderMap;
use std::sync::Arc;

/// Central authentication service responsible for validating request credentials.
#[derive(Clone)]
pub struct AuthService {
    config: Arc<AppConfig>,
    pat_validator: Arc<PatValidator>,
    oidc_validator: Arc<GoogleOidcValidator>,
}

impl AuthService {
    /// Create a new AuthService from application configuration.
    pub fn new(config: Arc<AppConfig>) -> Self {
        // Collect all configured API keys
        let mut all_keys = Vec::new();
        if let Some(key) = &config.api_key {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                all_keys.push(trimmed.to_string());
            }
        }
        for key in &config.api_keys {
            let trimmed = key.trim();
            if !trimmed.is_empty() && !all_keys.contains(&trimmed.to_string()) {
                all_keys.push(trimmed.to_string());
            }
        }

        let pat_validator = Arc::new(PatValidator::from_api_keys(&all_keys));

        // Collect all configured Google Client IDs
        let mut client_ids = Vec::new();
        if let Some(id) = &config.google_client_id {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                client_ids.push(trimmed.to_string());
            }
        }
        for id in &config.google_client_ids {
            let trimmed = id.trim();
            if !trimmed.is_empty() && !client_ids.contains(&trimmed.to_string()) {
                client_ids.push(trimmed.to_string());
            }
        }

        let jwks_provider = Arc::new(HttpJwksProvider::new(config.google_jwks_url.clone(), None));
        let oidc_validator = Arc::new(GoogleOidcValidator::new(jwks_provider, client_ids));

        Self {
            config,
            pat_validator,
            oidc_validator,
        }
    }

    /// Create an AuthService with custom components (e.g. for offline testing with mock JWKS).
    pub fn with_components(
        config: Arc<AppConfig>,
        pat_validator: Arc<PatValidator>,
        oidc_validator: Arc<GoogleOidcValidator>,
    ) -> Self {
        Self {
            config,
            pat_validator,
            oidc_validator,
        }
    }

    /// Create an AuthService configured with a mock JWKS provider for offline deterministic tests.
    pub fn with_mock_jwks(
        config: Arc<AppConfig>,
        mock_jwks: Arc<dyn JwksProvider>,
        google_client_ids: Vec<String>,
    ) -> Self {
        let mut all_keys = Vec::new();
        if let Some(key) = &config.api_key {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                all_keys.push(trimmed.to_string());
            }
        }
        for key in &config.api_keys {
            let trimmed = key.trim();
            if !trimmed.is_empty() && !all_keys.contains(&trimmed.to_string()) {
                all_keys.push(trimmed.to_string());
            }
        }

        let pat_validator = Arc::new(PatValidator::from_api_keys(&all_keys));
        let oidc_validator = Arc::new(GoogleOidcValidator::new(mock_jwks, google_client_ids));

        Self {
            config,
            pat_validator,
            oidc_validator,
        }
    }

    /// Access the underlying PAT validator.
    pub fn pat_validator(&self) -> &PatValidator {
        &self.pat_validator
    }

    /// Access the underlying OIDC validator.
    pub fn oidc_validator(&self) -> &GoogleOidcValidator {
        &self.oidc_validator
    }

    /// Determine if an incoming route path is an unauthenticated public probe.
    pub fn is_public_probe(path: &str) -> bool {
        path == "/api/v1/health" || path == "/api/v1/ready"
    }

    /// Determine if an incoming route path is operational telemetry data.
    pub fn is_operational_data(path: &str) -> bool {
        path.starts_with("/api/v1/usage") || path == "/api/v1/quotas"
    }

    /// Authenticate an incoming HTTP request.
    ///
    /// Evaluates headers against configured validators according to approved precedence:
    /// 1. Public probes (`/health`, `/ready`) bypass authentication.
    /// 2. If credentials are provided (Bearer token or `X-API-Key`):
    ///    - 3-segment JWTs are routed strictly to the Google OIDC validator.
    ///    - Non-JWT tokens or `X-API-Key` headers are routed strictly to the PAT/API Key validator.
    ///    - Invalid credentials NEVER fall back to another authentication method.
    /// 3. If no credentials are provided:
    ///    - In Production, strictly rejected with 401 Unauthorized.
    ///    - On external interfaces for operational data, strictly rejected with 401 Unauthorized.
    ///    - In Development/Test, only permitted if authentication is not required.
    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        path: &str,
    ) -> Result<Option<AuthPrincipal>, AppError> {
        // 1. Public probes are intentionally unauthenticated
        if Self::is_public_probe(path) {
            return Ok(None);
        }

        // Parse client type header (defaults to ApiClient)
        let client_type = headers
            .get("x-client-type")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<ClientType>().ok())
            .unwrap_or(ClientType::ApiClient);

        // Extract credentials
        let auth_header = headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.trim());

        let api_key_header = headers
            .get("x-api-key")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.trim());

        // Check for empty or malformed authorization headers
        if let Some(auth_str) = auth_header {
            if auth_str.is_empty() {
                return Err(AppError::Unauthorized(
                    "Authorization header cannot be empty".to_string(),
                ));
            }

            if !auth_str.starts_with("Bearer ") {
                return Err(AppError::Unauthorized(
                    "Unsupported authorization scheme: Bearer token expected".to_string(),
                ));
            }

            let token = auth_str["Bearer ".len()..].trim();
            if token.is_empty() {
                return Err(AppError::Unauthorized(
                    "Bearer token cannot be empty or whitespace".to_string(),
                ));
            }

            // Route based on token structure: JWT (3 segments separated by '.') vs PAT/Key
            let is_jwt = token.split('.').count() == 3;
            if is_jwt {
                // Cryptographic Google OIDC ID token validation
                return match self.oidc_validator.validate(token, client_type).await {
                    Ok(principal) => Ok(Some(principal)),
                    Err(err) => {
                        tracing::debug!(error = %err, "Google OIDC token validation failed");
                        Err(AppError::Unauthorized(
                            "Invalid or expired Google ID token".to_string(),
                        ))
                    }
                };
            } else {
                // Personal Access Token / API Key validation
                return match self.pat_validator.validate(token, client_type) {
                    Some(principal) => Ok(Some(principal)),
                    None => Err(AppError::Unauthorized(
                        "Invalid or revoked access token".to_string(),
                    )),
                };
            }
        }

        // Check X-API-Key header
        if let Some(key) = api_key_header {
            if key.is_empty() {
                return Err(AppError::Unauthorized(
                    "X-API-Key header cannot be empty or whitespace".to_string(),
                ));
            }

            return match self.pat_validator.validate(key, client_type) {
                Some(principal) => Ok(Some(principal)),
                None => Err(AppError::Unauthorized(
                    "Invalid or unconfigured API key".to_string(),
                )),
            };
        }

        // 3. No credentials provided: evaluate policy and environment
        let is_operational = Self::is_operational_data(path);
        let external_bind = !self.config.is_loopback_host();

        // Operational data on external interface must always be authenticated
        if is_operational && external_bind {
            return Err(AppError::Unauthorized(
                "Authentication required: operational telemetry is protected on external interfaces"
                    .to_string(),
            ));
        }

        // In Production, authentication is unconditionally mandatory
        if self.config.environment == Environment::Production {
            return Err(AppError::Unauthorized(
                "Missing authentication credentials on protected route".to_string(),
            ));
        }

        // In Development / Test, check if authentication is required
        if self.config.is_auth_required() {
            return Err(AppError::Unauthorized(
                "Missing authentication credentials on protected route".to_string(),
            ));
        }

        // Permitted local development unauthenticated bypass
        Ok(Some(AuthPrincipal::development_bypass(client_type)))
    }
}
