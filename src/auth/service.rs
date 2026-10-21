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
    /// Maximum permitted length of an individual credential string (8 KiB)
    /// to bound memory allocation and parsing overhead under malicious payloads.
    pub const MAX_CREDENTIAL_LEN: usize = 8192;

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

        let destination_policy = Arc::new(config.outbound_destination_policy());
        let jwks_provider = Arc::new(
            HttpJwksProvider::new(config.google_jwks_url.clone(), None)
                .with_destination_policy(destination_policy),
        );
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

    /// Attach a dynamic CredentialStore for runtime PAT lifecycle management.
    pub fn with_credential_store(
        mut self,
        store: Arc<super::credential_store::CredentialStore>,
    ) -> Self {
        self.pat_validator = Arc::new(self.pat_validator.as_ref().clone().with_store(store));
        self
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
        let normalized = path.trim_end_matches('/');
        normalized == "/api/v1/health" || normalized == "/api/v1/ready"
    }

    /// Determine if an incoming route path is operational telemetry data.
    pub fn is_operational_data(path: &str) -> bool {
        let normalized = path.trim_end_matches('/');
        normalized.starts_with("/api/v1/usage") || normalized == "/api/v1/quotas"
    }

    /// Percent-decode a URL query string slice.
    pub fn percent_decode(input: &str) -> String {
        fn decode_hex_nibble(b: u8) -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            }
        }

        let mut bytes = Vec::with_capacity(input.len());
        let input_bytes = input.as_bytes();
        let mut i = 0;
        while i < input_bytes.len() {
            if input_bytes[i] == b'%' && i + 2 < input_bytes.len() {
                if let (Some(n1), Some(n2)) = (
                    decode_hex_nibble(input_bytes[i + 1]),
                    decode_hex_nibble(input_bytes[i + 2]),
                ) {
                    bytes.push((n1 << 4) | n2);
                    i += 3;
                    continue;
                }
            } else if input_bytes[i] == b'+' {
                bytes.push(b' ');
                i += 1;
                continue;
            }
            bytes.push(input_bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Check whether a query string contains credential-bearing parameters.
    ///
    /// Correctly decodes percent-encoded parameters, handles duplicate keys, empty values,
    /// and matches recognized credential parameters (`api_key`, `access_token`, `token`,
    /// `auth_token`, `auth`, `bearer`, etc.).
    pub fn contains_credential_query(query: Option<&str>) -> bool {
        let Some(q) = query else {
            return false;
        };

        // Normalize delimiters: support standard '&' and ';' delimiters
        for pair in q.split(['&', ';']) {
            let pair_trimmed = pair.trim();
            if pair_trimmed.is_empty() {
                continue;
            }

            // Percent-decode first to prevent percent-encoding bypass (e.g. api%5fkey or %61%70%69...)
            let decoded_pair = Self::percent_decode(pair_trimmed);
            let raw_key = decoded_pair
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();

            if raw_key == "api_key"
                || raw_key == "apikey"
                || raw_key == "api-key"
                || raw_key == "access_token"
                || raw_key == "accesstoken"
                || raw_key == "access-token"
                || raw_key == "token"
                || raw_key == "auth_token"
                || raw_key == "authtoken"
                || raw_key == "auth-token"
                || raw_key == "auth"
                || raw_key == "bearer"
                || raw_key == "secret"
                || raw_key == "client_secret"
                || raw_key == "client-secret"
            {
                return true;
            }
        }
        false
    }

    /// Resolve the required authorization scope for an incoming route path per API_SPEC.md §1.1.
    pub fn required_scope_for_route(path: &str) -> Option<&'static str> {
        let normalized = path.trim_end_matches('/');
        if normalized == "/api/v1/health" || normalized == "/api/v1/ready" {
            None
        } else if normalized == "/api/v1/models"
            || normalized.starts_with("/api/v1/models/")
            || normalized == "/api/v1/providers"
        {
            Some("models:read")
        } else if normalized == "/api/v1/routing/preview" {
            Some("routing:read")
        } else if normalized == "/api/v1/chat/completions" || normalized == "/api/v1/chat/stream" {
            Some("chat")
        } else if normalized.starts_with("/api/v1/usage") || normalized == "/api/v1/quotas" {
            Some("usage:read")
        } else if normalized.starts_with("/api/v1/auth/tokens") {
            Some("auth:tokens")
        } else {
            None
        }
    }

    /// Verify that an authenticated principal has permission to access the specified route.
    pub fn authorize(principal: &AuthPrincipal, path: &str) -> Result<(), AppError> {
        if let Some(required_scope) = Self::required_scope_for_route(path)
            && !principal.has_scope(required_scope)
        {
            return Err(AppError::Forbidden(format!(
                "Principal '{}' lacks required scope '{required_scope}' for route '{path}'",
                principal.subject
            )));
        }
        Ok(())
    }

    /// Authenticate an incoming HTTP request.
    ///
    /// Evaluates headers and query strings against configured validators according to approved precedence:
    /// 1. Public probes (`/health`, `/ready`) bypass authentication.
    /// 2. If credentials are submitted via query parameters, strictly rejected with 401 Unauthorized.
    /// 3. If credentials are provided (Bearer token or `X-API-Key`):
    ///    - 3-segment JWTs are routed strictly to the Google OIDC validator.
    ///    - Non-JWT tokens or `X-API-Key` headers are routed strictly to the PAT/API Key validator.
    ///    - Invalid credentials NEVER fall back to another authentication method.
    /// 4. If no credentials are provided:
    ///    - On non-loopback network interfaces, strictly rejected with 401 Unauthorized for all protected routes.
    ///    - In Production, strictly rejected with 401 Unauthorized.
    ///    - In Development/Test, only permitted on loopback if authentication is not required.
    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        path: &str,
        query: Option<&str>,
    ) -> Result<Option<AuthPrincipal>, AppError> {
        // 1. Inbound query credential rejection: credentials MUST NOT be accepted in URL query parameters
        if Self::contains_credential_query(query) {
            return Err(AppError::Unauthorized(
                "Authentication credentials must not be passed in URL query parameters. Use request headers instead."
                    .to_string(),
            ));
        }

        // 2. Public probes are intentionally unauthenticated
        if Self::is_public_probe(path) {
            return Ok(None);
        }

        // 3. Prohibit duplicate authorization or api-key headers
        if headers.get_all("authorization").iter().count() > 1 {
            return Err(AppError::Unauthorized(
                "Duplicate Authorization headers are prohibited".to_string(),
            ));
        }
        if headers.get_all("x-api-key").iter().count() > 1 {
            return Err(AppError::Unauthorized(
                "Duplicate X-API-Key headers are prohibited".to_string(),
            ));
        }

        // 4. Prohibit conflicting simultaneous credentials (both Authorization and X-API-Key provided)
        let has_auth = headers.contains_key("authorization");
        let has_key = headers.contains_key("x-api-key");
        if has_auth && has_key {
            return Err(AppError::Unauthorized(
                "Conflicting authentication credentials: both Authorization and X-API-Key headers were provided"
                    .to_string(),
            ));
        }

        // 5. Parse client type header (defaults to ApiClient)
        let client_type = headers
            .get("x-client-type")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<ClientType>().ok())
            .unwrap_or(ClientType::ApiClient);

        // 6. Extract credentials safely, validating UTF-8 encoding
        let auth_header = match headers.get("authorization") {
            Some(val) => {
                let s = val.to_str().map_err(|_| {
                    AppError::Unauthorized(
                        "Invalid non-UTF-8 characters in Authorization header".to_string(),
                    )
                })?;
                Some(s.trim())
            }
            None => None,
        };

        let api_key_header = match headers.get("x-api-key") {
            Some(val) => {
                let s = val.to_str().map_err(|_| {
                    AppError::Unauthorized(
                        "Invalid non-UTF-8 characters in X-API-Key header".to_string(),
                    )
                })?;
                Some(s.trim())
            }
            None => None,
        };

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

            if token.len() > Self::MAX_CREDENTIAL_LEN {
                return Err(AppError::Unauthorized(
                    "Bearer token exceeds maximum permitted length of 8192 bytes".to_string(),
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

            if key.len() > Self::MAX_CREDENTIAL_LEN {
                return Err(AppError::Unauthorized(
                    "X-API-Key exceeds maximum permitted length of 8192 bytes".to_string(),
                ));
            }

            return match self.pat_validator.validate(key, client_type) {
                Some(principal) => Ok(Some(principal)),
                None => Err(AppError::Unauthorized(
                    "Invalid or unconfigured API key".to_string(),
                )),
            };
        }

        // 3. No credentials provided: evaluate policy, bind interface, and environment
        let external_bind = !self.config.is_loopback_host();

        // On non-loopback network interfaces, unauthenticated access is strictly prohibited for all protected routes
        if external_bind {
            return Err(AppError::Unauthorized(
                "Authentication required: unauthenticated access is prohibited on non-loopback network interfaces"
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

        // Permitted local development unauthenticated bypass strictly on loopback
        Ok(Some(AuthPrincipal::development_bypass(client_type)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::principal::AuthRole;

    #[test]
    fn test_public_probe_identification() {
        assert!(AuthService::is_public_probe("/api/v1/health"));
        assert!(AuthService::is_public_probe("/api/v1/health/"));
        assert!(AuthService::is_public_probe("/api/v1/ready"));
        assert!(AuthService::is_public_probe("/api/v1/ready/"));
        assert!(!AuthService::is_public_probe("/api/v1/models"));
        assert!(!AuthService::is_public_probe("/api/v1/chat/completions"));
    }

    #[test]
    fn test_operational_data_identification() {
        assert!(AuthService::is_operational_data("/api/v1/usage/summary"));
        assert!(AuthService::is_operational_data("/api/v1/quotas"));
        assert!(!AuthService::is_operational_data("/api/v1/models"));
    }

    #[test]
    fn test_route_scope_resolution() {
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/health"),
            None
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/models"),
            Some("models:read")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/models/gpt-4o"),
            Some("models:read")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/providers"),
            Some("models:read")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/routing/preview"),
            Some("routing:read")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/chat/completions"),
            Some("chat")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/chat/stream"),
            Some("chat")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/usage/summary"),
            Some("usage:read")
        );
        assert_eq!(
            AuthService::required_scope_for_route("/api/v1/quotas"),
            Some("usage:read")
        );
    }

    #[test]
    fn test_percent_decode_and_credential_query_detection() {
        assert!(AuthService::contains_credential_query(Some(
            "api_key=secret123"
        )));
        assert!(AuthService::contains_credential_query(Some("token=xyz")));
        assert!(AuthService::contains_credential_query(Some(
            "access_token=xyz"
        )));
        assert!(AuthService::contains_credential_query(Some(
            "%61%70%69%5f%6b%65%79=123"
        )));
        assert!(AuthService::contains_credential_query(Some(
            "foo=bar&api_key=123"
        )));
        assert!(!AuthService::contains_credential_query(Some(
            "model=gpt-4o&stream=true"
        )));
        assert!(!AuthService::contains_credential_query(None));
    }

    #[test]
    fn test_authorize_scope_enforcement() {
        let principal_chat_only = AuthPrincipal::new(
            "user:123",
            crate::auth::AuthType::ApiKey,
            ClientType::ApiClient,
            None,
            vec![AuthRole::User],
            vec!["chat".to_string()],
        );

        // Allowed on chat route
        assert!(AuthService::authorize(&principal_chat_only, "/api/v1/chat/completions").is_ok());

        // Denied on models route
        let err = AuthService::authorize(&principal_chat_only, "/api/v1/models").unwrap_err();
        match err {
            AppError::Forbidden(msg) => {
                assert!(msg.contains("lacks required scope 'models:read'"));
            }
            other => panic!("Expected Forbidden, got {other:?}"),
        }
    }
}
