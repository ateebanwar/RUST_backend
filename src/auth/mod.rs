//! Authentication and authorization module.
//!
//! Provides multi-client identity verification, Google Sign-In OIDC ID token validation,
//! Personal Access Token (PAT) constant-time hashing and verification, authorization roles,
//! and Axum authentication middleware.

pub mod credential_store;
pub mod middleware;
pub mod oidc;
pub mod pat;
pub mod principal;
pub mod service;

pub use credential_store::{
    CreateTokenRequest, CreateTokenResponse, CredentialMetadata, CredentialStore,
    RotateTokenResponse,
};
pub use middleware::auth_middleware;
pub use oidc::{
    GoogleClaims, GoogleOidcValidator, HttpJwksProvider, JwksProvider, MockJwksProvider, OidcError,
};
pub use pat::{
    ConfiguredCredential, PatValidator, constant_time_compare_str, generate_secure_pat, hash_token,
};
pub use principal::{AuthPrincipal, AuthRole, AuthType, ClientType};
pub use service::AuthService;
