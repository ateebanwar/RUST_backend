//! Authenticated principal, roles, permissions, and client types.
//!
//! Provides typed representations of authenticated client identities,
//! authorization roles, scopes, and client classification headers.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Client application classification derived from `X-Client-Type` header (API_SPEC.md §1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientType {
    /// Flutter mobile application (Android / iOS reference client).
    Flutter,
    /// Custom AI-powered developer IDE.
    CustomIde,
    /// VS Code or other desktop editor extension.
    VscodeExtension,
    /// Generic or automated API consumer.
    ApiClient,
}

impl ClientType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Flutter => "flutter",
            Self::CustomIde => "custom-ide",
            Self::VscodeExtension => "vscode-extension",
            Self::ApiClient => "api-client",
        }
    }
}

impl fmt::Display for ClientType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ClientType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "flutter" | "mobile" | "android" => Ok(Self::Flutter),
            "custom-ide" | "ide" => Ok(Self::CustomIde),
            "vscode-extension" | "vscode" | "extension" => Ok(Self::VscodeExtension),
            _ => Ok(Self::ApiClient),
        }
    }
}

/// The mechanism through which the principal was authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// Google Sign-In OpenID Connect (OIDC) ID token verified via RS256 JWKS.
    GoogleOidc,
    /// Personal Access Token (PAT) with constant-time cryptographic hash verification.
    PersonalAccessToken,
    /// Server/developer API key with constant-time verification.
    ApiKey,
    /// Development/test unauthenticated bypass (strictly prohibited in production).
    DevelopmentBypass,
}

impl fmt::Display for AuthType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GoogleOidc => write!(f, "google_oidc"),
            Self::PersonalAccessToken => write!(f, "personal_access_token"),
            Self::ApiKey => write!(f, "api_key"),
            Self::DevelopmentBypass => write!(f, "development_bypass"),
        }
    }
}

/// Authorization role assigned to an authenticated principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthRole {
    /// Standard authenticated end-user (e.g. mobile user or developer).
    User,
    /// System or service administrator.
    Admin,
    /// Automated machine or background service client.
    Service,
}

impl fmt::Display for AuthRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Admin => write!(f, "admin"),
            Self::Service => write!(f, "service"),
        }
    }
}

/// Strongly-typed identity and security context of an authenticated request.
///
/// Propagated downstream to route handlers and services via Axum request extensions.
#[derive(Clone, Serialize, Deserialize)]
pub struct AuthPrincipal {
    /// Unique subject identifier (e.g. Google OIDC `sub`, PAT identity, or key hash prefix).
    pub subject: String,
    /// Authentication mechanism used for this request.
    pub auth_type: AuthType,
    /// Calling client classification (e.g. Flutter, CustomIde).
    pub client_type: ClientType,
    /// User email address if authenticated via OIDC or associated with credential.
    pub email: Option<String>,
    /// Assigned authorization roles.
    pub roles: Vec<AuthRole>,
    /// Authorized operation scopes (e.g. "chat", "models:read", "routing:read", "usage:read").
    pub scopes: Vec<String>,
}

impl AuthPrincipal {
    /// Create a new authenticated principal.
    pub fn new(
        subject: impl Into<String>,
        auth_type: AuthType,
        client_type: ClientType,
        email: Option<String>,
        roles: Vec<AuthRole>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            auth_type,
            client_type,
            email,
            roles,
            scopes,
        }
    }

    /// Development bypass principal used when authentication is explicitly disabled in non-production.
    pub fn development_bypass(client_type: ClientType) -> Self {
        Self {
            subject: "dev_local_user".to_string(),
            auth_type: AuthType::DevelopmentBypass,
            client_type,
            email: Some("dev@localhost".to_string()),
            roles: vec![AuthRole::User, AuthRole::Admin],
            scopes: vec![
                "chat".to_string(),
                "models:read".to_string(),
                "routing:read".to_string(),
                "usage:read".to_string(),
                "auth:tokens".to_string(),
                "drive".to_string(),
            ],
        }
    }

    /// Check if principal has a specific authorization role.
    pub fn has_role(&self, role: &AuthRole) -> bool {
        self.roles.contains(role)
    }

    /// Check if principal has a specific authorized scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope || s == "*")
    }

    /// Check if principal is an administrator.
    pub fn is_admin(&self) -> bool {
        self.has_role(&AuthRole::Admin)
    }

    /// Check if principal is an end-user.
    pub fn is_user(&self) -> bool {
        self.has_role(&AuthRole::User)
    }
}

impl fmt::Debug for AuthPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthPrincipal")
            .field("subject", &self.subject)
            .field("auth_type", &self.auth_type)
            .field("client_type", &self.client_type)
            .field("email", &self.email.as_deref().map(|_| "[REDACTED]"))
            .field("roles", &self.roles)
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl fmt::Display for AuthPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Principal(sub={}, type={}, client={})",
            self.subject, self.auth_type, self.client_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_type_parsing() {
        assert_eq!(
            "flutter".parse::<ClientType>().unwrap(),
            ClientType::Flutter
        );
        assert_eq!("mobile".parse::<ClientType>().unwrap(), ClientType::Flutter);
        assert_eq!(
            "custom-ide".parse::<ClientType>().unwrap(),
            ClientType::CustomIde
        );
        assert_eq!("ide".parse::<ClientType>().unwrap(), ClientType::CustomIde);
        assert_eq!(
            "vscode-extension".parse::<ClientType>().unwrap(),
            ClientType::VscodeExtension
        );
        assert_eq!(
            "api-client".parse::<ClientType>().unwrap(),
            ClientType::ApiClient
        );
        assert_eq!(
            "unknown".parse::<ClientType>().unwrap(),
            ClientType::ApiClient
        );
    }

    #[test]
    fn test_principal_role_and_scope_checks() {
        let principal = AuthPrincipal::new(
            "sub_123",
            AuthType::ApiKey,
            ClientType::CustomIde,
            Some("dev@example.com".to_string()),
            vec![AuthRole::User],
            vec!["chat".to_string(), "models:read".to_string()],
        );

        assert!(principal.has_role(&AuthRole::User));
        assert!(!principal.has_role(&AuthRole::Admin));
        assert!(principal.has_scope("chat"));
        assert!(principal.has_scope("models:read"));
        assert!(!principal.has_scope("usage:admin"));
    }

    #[test]
    fn test_principal_debug_redacts_email() {
        let principal = AuthPrincipal::new(
            "sub_456",
            AuthType::GoogleOidc,
            ClientType::Flutter,
            Some("user@gmail.com".to_string()),
            vec![AuthRole::User],
            vec!["chat".to_string()],
        );

        let debug_str = format!("{principal:?}");
        assert!(!debug_str.contains("user@gmail.com"));
        assert!(debug_str.contains("[REDACTED]"));
    }
}
