//! Comprehensive integration verification tests for Phase 11 â€” Authentication & Authorization.
//!
//! Verifies:
//! 1. Public probes (/health, /ready) remain accessible without credentials.
//! 2. Protected endpoints reject missing credentials with HTTP 401 Unauthorized.
//! 3. Protected endpoints reject incorrect credentials (wrong Bearer token, wrong API key).
//! 4. Malformed credentials (empty string, whitespace, non-Bearer schemes) are rejected cleanly.
//! 5. Production mode fails closed at startup when credentials are unconfigured.
//! 6. Valid API keys and Personal Access Tokens (PATs) are accepted in constant time.
//! 7. Google Sign-In OIDC ID tokens (RS256) are cryptographically validated against JWKS offline.
//! 8. OIDC failure cases: wrong issuer, wrong audience, expired tokens, unknown kid, corrupted signature.
//! 9. Precedence & zero-fallback: invalid credentials never receive weaker fallback authentication.
//! 10. Streaming endpoints enforce authentication prior to opening SSE streams.
//! 11. Operational endpoints (/usage/*, /quotas) enforce authentication on external interfaces.
//! 12. Identity propagation: AuthPrincipal is correctly constructed with client type, subject, and roles.
//! 13. Zero secrets or partial matches are leaked in errors or diagnostic outputs.

use ai_backend::auth::{
    AuthPrincipal, AuthRole, AuthService, AuthType, ClientType, ConfiguredCredential, GoogleClaims,
    GoogleOidcValidator, MockJwksProvider, PatValidator, generate_secure_pat,
};
use ai_backend::config::{AppConfig, ConfigError, Environment};
use ai_backend::{AppState, create_app};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;

// Deterministic 2048-bit RSA test keypair fixtures (strictly dummy non-secret test keys)
const TEST_RSA_PRIVATE_KEY_PEM: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEA1DTqIrrH6kthRCIz56hLtjRhWdA82fbgVGfU5RgobK5VU2sz
UGZRRadvFYpaJCK27FMrIAg3aq15GnFoDIheLEbl1x3tD1WRtrC6iiO2sSzTRtAa
IwYKxkT8uBnN9pKXPRclb9YRauO70QHjc4LEXFySY6XN3q8pyUfGLFk6dOzpQQzV
GxKkJx9MyfH8mWjJtRm7klSAP34A+RUep12JsfM6pabakGKlPUq52vSozUJ9lgIM
Q3qZ+J3vPbAP9/2sPmspW4xLwTwoddjsmDgMDYk5Pd8OOCtsOp74pt9T+pf5RAHo
UGkUkS8VfOlgaKu1XBg8bwKYm+r/qkfodLAK7QIDAQABAoIBAHgKitwkjsxx4c7a
DkSghlvG278jthiSuCnnJp6GAPAh2jC7Nqbnilf9tDAaOI2yeYnMLlwLadXV1ejo
eDSGu8cOQBxlWGHRRQF0jcFxHHqzYbzwIajoVRX9lnxKVKGEITu0qT/Pd8DgnLFK
ATGpqoyY7VKjS4cJzXiIOomB3Dihd7gstoA39+K81Uef++KXuRTrSzLmSDdmQqlL
I1gBc6mwanxitr9Qbdoe2e5oyC6bVYmSTvZca30QO+YlPkig4faltJVHg3N09+yc
XxE2avfzp5PnwaMkpdfGivw0ZttviChkuKbLk4uRJeHQOvUn8ydw39WOSaviajlC
9lvDKL0CgYEA+PP2FZyFKlRFICkQwytajwNo6ibZJvSLQZ3/FPil2TXswGXGaNZv
UTk9/g0uoJschow9bnFJUjIube7DdyfuZBdmtV/3MpeHFXmglxXV+wXQC818s0np
OENl1TBsLsHlywudNLqbA6DGhar1VLxG3c5PZw6wF6UXe0TyezqIzEsCgYEA2jar
2XHmHlETeW7Qh9z4nG1d+zddP3EI2BDnZSKeDyl40jQhI5e/UsFYvWWqFs/MrbRf
Dn8+U1TNz5yZ07yLT4fPqtox1GLqL+MFRYbDu9gJuV71t6RSVzwrfsz/gkdBsG8X
ToB8miAnXaWCko1Hf8/+BekdhleadT2upUzQkqcCgYByVvorKtwueM3W24WLXhoV
nSdOg9Ewhxh+ZqaFZz4k6RN3ukbKV9KAp53FVSCZOyR612ZPl1BppAqXCxZTX8+p
wOW3YAAVCGtOfbI4Mhs5U44eHZ6sacHnyWy34amOQBfejdMCKMKRhAIp2j8bT+3s
aZkmaCSQd461WxFTb6rsyQKBgEizzF+/JgdVxeJAfHO3uJENkzFeyc+YTnUGdMHq
XbT5gUqvmZbtOQ5OYs1/cML26t+4567B0DcdWxDfwRSdjeCqhBMNbWqwpgvcXGZE
tzP/j4XWUJGlytVLs8YkcMCsEwPAj4hVIK6NJfB6c112eqrn1l+Lwo1+5HRJX8Wt
KRSjAoGANZGBENbSWa1kGkEwf3i4WrD/WMgLEL25w+Bcdz5y7xs20znqOKExQjSA
1qP4PWCIdfbeC2Kv1TKk2jHw9aPgqGVK9GjmvlJm6p6BdmHzNk9EgtAAQpu2mMl8
+4rheCW0HKu3tQzfzC7/oIfPoXyn/rwagjukSpOy+alSHF6WqRg=
-----END RSA PRIVATE KEY-----"#;

const TEST_RSA_PUBLIC_KEY_PEM: &str = r#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA1DTqIrrH6kthRCIz56hL
tjRhWdA82fbgVGfU5RgobK5VU2szUGZRRadvFYpaJCK27FMrIAg3aq15GnFoDIhe
LEbl1x3tD1WRtrC6iiO2sSzTRtAaIwYKxkT8uBnN9pKXPRclb9YRauO70QHjc4LE
XFySY6XN3q8pyUfGLFk6dOzpQQzVGxKkJx9MyfH8mWjJtRm7klSAP34A+RUep12J
sfM6pabakGKlPUq52vSozUJ9lgIMQ3qZ+J3vPbAP9/2sPmspW4xLwTwoddjsmDgM
DYk5Pd8OOCtsOp74pt9T+pf5RAHoUGkUkS8VfOlgaKu1XBg8bwKYm+r/qkfodLAK
7QIDAQAB
-----END PUBLIC KEY-----"#;

const TEST_KID: &str = "test_key_kid_2026";
const TEST_CLIENT_ID: &str = "test-mobile-app.apps.googleusercontent.com";

/// Spawn a live test Axum server on an ephemeral loopback port.
async fn spawn_auth_test_backend(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state);

    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    (format!("http://127.0.0.1:{}", addr.port()), handle)
}

/// Helper to generate signed RS256 test JWTs with custom claims.
fn create_test_jwt(
    claims: &GoogleClaims,
    kid: Option<&str>,
    private_key_pem: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(|s| s.to_string());
    let encoding_key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes())?;
    jsonwebtoken::encode(&header, claims, &encoding_key)
}

// =========================================================================
// 1. PUBLIC PROBES VS. PROTECTED ENDPOINTS MATRIX
// =========================================================================

#[tokio::test]
async fn test_public_probes_bypass_authentication() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("configured_secret_key_12345".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. /api/v1/health is public
    let health_resp = client
        .get(format!("{base_url}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health_resp.status(), reqwest::StatusCode::OK);

    // 2. /api/v1/ready is public (returns 503 since no active upstream provider, but NOT 401)
    let ready_resp = client
        .get(format!("{base_url}/api/v1/ready"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        ready_resp.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn test_all_protected_endpoints_reject_missing_credentials() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("configured_secret_key_12345".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    let protected_routes: Vec<(&str, &str, Option<Value>)> = vec![
        (
            "POST",
            "/api/v1/chat/completions",
            Some(json!({ "messages": [{ "role": "user", "content": "hello" }] })),
        ),
        (
            "POST",
            "/api/v1/chat/stream",
            Some(json!({ "messages": [{ "role": "user", "content": "stream" }] })),
        ),
        ("GET", "/api/v1/models", None),
        ("GET", "/api/v1/models/fast-chat-v1", None),
        ("GET", "/api/v1/providers", None),
        (
            "POST",
            "/api/v1/routing/preview",
            Some(json!({ "prompt": "analyze code" })),
        ),
        ("GET", "/api/v1/usage/summary", None),
        ("GET", "/api/v1/usage/providers", None),
        ("GET", "/api/v1/usage/models", None),
        ("GET", "/api/v1/quotas", None),
    ];

    for (method, path, body) in protected_routes {
        let req_builder = match method {
            "POST" => client.post(format!("{base_url}{path}")),
            _ => client.get(format!("{base_url}{path}")),
        };

        let req_builder = if let Some(b) = body {
            req_builder.json(&b)
        } else {
            req_builder
        };

        let resp = req_builder.send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "Route '{method} {path}' must reject unauthenticated requests with 401"
        );

        let body_val: Value = resp.json().await.unwrap();
        assert_eq!(body_val["error"]["code"], "UNAUTHORIZED");
        assert_eq!(body_val["error"]["retryable"], false);
        assert!(body_val["error"]["request_id"].is_string());
    }
}

// =========================================================================
// 2. INVALID AND MALFORMED CREDENTIALS REJECTION
// =========================================================================

#[tokio::test]
async fn test_rejected_malformed_and_empty_credentials() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("valid_backend_key_123".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    let invalid_headers: Vec<(&str, &str)> = vec![
        ("Authorization", ""),
        ("Authorization", "   "),
        ("Authorization", "Bearer"),
        ("Authorization", "Bearer "),
        ("Authorization", "Bearer    "),
        ("Authorization", "Basic dXNlcjpwYXNz"),
        ("Authorization", "Token some_token"),
        ("Authorization", "Bearer wrong_key_value"),
        ("X-API-Key", ""),
        ("X-API-Key", "   "),
        ("X-API-Key", "invalid_key_attempt"),
    ];

    for (header_name, header_value) in invalid_headers {
        let resp = client
            .get(format!("{base_url}/api/v1/models"))
            .header(header_name, header_value)
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "Header '{header_name}: {header_value}' must be rejected with 401"
        );
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");
    }
}

// =========================================================================
// 3. VALID API KEY & PAT AUTHENTICATION
// =========================================================================

#[tokio::test]
async fn test_valid_api_key_and_pat_authentication() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("primary_api_key_secure_99".to_string());
    config.api_keys = vec!["secondary_api_key_abc_12".to_string()];
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Primary API key via X-API-Key
    let resp1 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-API-Key", "primary_api_key_secure_99")
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), reqwest::StatusCode::OK);

    // 2. Primary API key via Authorization: Bearer
    let resp2 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", "Bearer primary_api_key_secure_99")
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), reqwest::StatusCode::OK);

    // 3. Secondary API key from API_KEYS list via X-API-Key
    let resp3 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-API-Key", "secondary_api_key_abc_12")
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), reqwest::StatusCode::OK);

    // 4. Secondary API key from API_KEYS list via Bearer
    let resp4 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", "Bearer secondary_api_key_abc_12")
        .send()
        .await
        .unwrap();
    assert_eq!(resp4.status(), reqwest::StatusCode::OK);
}

#[test]
fn test_secure_pat_generation_and_validation() {
    let pat = generate_secure_pat().unwrap();
    assert!(pat.starts_with("pat_"));
    assert_eq!(pat.len(), 68); // "pat_" + 64 hex chars

    let mut validator = PatValidator::new();
    validator.add_credential(ai_backend::auth::ConfiguredCredential::from_pat(
        &pat,
        "dev_user_01",
        Some("developer@ide.local".to_string()),
        vec![AuthRole::User],
        vec!["chat".to_string()],
    ));

    let principal = validator.validate(&pat, ClientType::CustomIde).unwrap();
    assert_eq!(principal.subject, "dev_user_01");
    assert_eq!(principal.auth_type, AuthType::PersonalAccessToken);
    assert_eq!(principal.client_type, ClientType::CustomIde);
    assert_eq!(principal.email, Some("developer@ide.local".to_string()));
    assert!(principal.has_scope("chat"));

    // Incorrect token is rejected
    assert!(
        validator
            .validate("pat_wrong_token", ClientType::CustomIde)
            .is_none()
    );
}

// =========================================================================
// 4. GOOGLE SIGN-IN OIDC RS256 CRYPTOGRAPHIC VERIFICATION
// =========================================================================

#[tokio::test]
async fn test_google_oidc_valid_token_authentication() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "google_subject_uid_123456789".to_string(),
        aud: json!(TEST_CLIENT_ID),
        exp: now + 3600, // Valid for 1 hour
        iat: Some(now),
        nbf: None,
        azp: None,
        email: Some("engineer@flutter.app".to_string()),
        email_verified: Some(true),
        name: Some("Lead Mobile Engineer".to_string()),
        picture: None,
    };

    let valid_token = create_test_jwt(&claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM).unwrap();

    let mut mock_jwks = MockJwksProvider::new();
    let decoding_key = DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC_KEY_PEM.as_bytes()).unwrap();
    mock_jwks.add_key(TEST_KID, decoding_key);

    let config = Arc::new(AppConfig::default_local());
    let auth_service = Arc::new(AuthService::with_mock_jwks(
        config.clone(),
        Arc::new(mock_jwks),
        vec![TEST_CLIENT_ID.to_string()],
    ));

    let state = AppState::new(config).with_auth_service(auth_service);
    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Authenticate with Google OIDC ID token
    let resp = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {valid_token}"))
        .header("X-Client-Type", "flutter")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "Valid Google ID token must authenticate successfully"
    );
}

#[tokio::test]
async fn test_google_oidc_rejection_edge_cases() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut mock_jwks = MockJwksProvider::new();
    let decoding_key = DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC_KEY_PEM.as_bytes()).unwrap();
    mock_jwks.add_key(TEST_KID, decoding_key);

    let config = Arc::new(AppConfig::default_local());
    let auth_service = Arc::new(AuthService::with_mock_jwks(
        config.clone(),
        Arc::new(mock_jwks),
        vec![TEST_CLIENT_ID.to_string()],
    ));

    let state = AppState::new(config).with_auth_service(auth_service);
    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Case 1: Wrong Issuer (e.g. evil issuer)
    let bad_issuer_claims = GoogleClaims {
        iss: "https://evil-issuer.com".to_string(),
        sub: "user_123".to_string(),
        aud: json!(TEST_CLIENT_ID),
        exp: now + 3600,
        iat: Some(now),
        nbf: None,
        azp: None,
        email: Some("user@evil.com".to_string()),
        email_verified: Some(true),
        name: None,
        picture: None,
    };
    let token_bad_iss =
        create_test_jwt(&bad_issuer_claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM).unwrap();
    let resp1 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {token_bad_iss}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Case 2: Wrong Audience (mismatched Google client ID)
    let bad_aud_claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "user_123".to_string(),
        aud: json!("unauthorized-client.apps.googleusercontent.com"),
        exp: now + 3600,
        iat: Some(now),
        nbf: None,
        azp: None,
        email: None,
        email_verified: None,
        name: None,
        picture: None,
    };
    let token_bad_aud =
        create_test_jwt(&bad_aud_claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM).unwrap();
    let resp2 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {token_bad_aud}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Case 3: Expired Token (exp in the past beyond leeway)
    let expired_claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "user_123".to_string(),
        aud: json!(TEST_CLIENT_ID),
        exp: now - 3600, // expired 1 hour ago
        iat: Some(now - 7200),
        nbf: None,
        azp: None,
        email: None,
        email_verified: None,
        name: None,
        picture: None,
    };
    let token_expired =
        create_test_jwt(&expired_claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM).unwrap();
    let resp3 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {token_expired}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Case 4: Unknown Key ID (kid missing from JWKS)
    let valid_claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "user_123".to_string(),
        aud: json!(TEST_CLIENT_ID),
        exp: now + 3600,
        iat: Some(now),
        nbf: None,
        azp: None,
        email: None,
        email_verified: None,
        name: None,
        picture: None,
    };
    let token_unknown_kid = create_test_jwt(
        &valid_claims,
        Some("nonexistent_kid_99"),
        TEST_RSA_PRIVATE_KEY_PEM,
    )
    .unwrap();
    let resp4 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {token_unknown_kid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp4.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Case 5: Tampered token signature
    let parts: Vec<&str> = token_expired.split('.').collect();
    let tampered_token = format!("{}.{}.corrupted_signature_segment", parts[0], parts[1]);
    let resp5 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {tampered_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp5.status(), reqwest::StatusCode::UNAUTHORIZED);
}

// =========================================================================
// 5. PRECEDENCE & ZERO FALLBACK
// =========================================================================

#[tokio::test]
async fn test_invalid_credential_never_falls_back_to_dev_bypass() {
    let mut config = AppConfig::default_local();
    // In local development with NO keys configured, dev bypass is normally allowed
    config.api_key = None;
    config.api_keys.clear();
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Explicitly invalid token provided -> MUST be rejected, NOT fall back to bypass!
    let resp_bad = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", "Bearer invalid_secret_token_123")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_bad.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Supplied invalid credential must NEVER silently fall back to weaker dev bypass"
    );

    // 2. Explicitly invalid API key provided -> MUST be rejected
    let resp_bad_key = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-API-Key", "invalid_api_key_attempt")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_bad_key.status(), reqwest::StatusCode::UNAUTHORIZED);
}

// =========================================================================
// 6. STREAMING ROUTE AUTHENTICATION ENFORCEMENT
// =========================================================================

#[tokio::test]
async fn test_streaming_route_enforces_auth_before_opening_stream() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("stream_auth_secret_key_99".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Request stream without credentials
    let resp = client
        .post(format!("{base_url}/api/v1/chat/stream"))
        .json(&json!({
            "model": "fast-chat-v1",
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Streaming endpoint must reject missing credentials with 401 before stream opens"
    );

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "Authentication failure must return canonical JSON error, not text/event-stream"
    );

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "UNAUTHORIZED");
}

// =========================================================================
// 7. OPERATIONAL ENDPOINT PROTECTION ON EXTERNAL INTERFACES
// =========================================================================

#[tokio::test]
async fn test_operational_endpoints_protected_on_external_interface() {
    let mut config = AppConfig::default_local();
    // Simulate binding to external interface without API key configured
    config.host = "0.0.0.0".to_string();
    config.api_key = None;
    config.api_keys.clear();
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Operational telemetry must be rejected on external bind when unauthenticated
    let resp_usage = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_usage.status(), reqwest::StatusCode::UNAUTHORIZED);

    let resp_quotas = client
        .get(format!("{base_url}/api/v1/quotas"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_quotas.status(), reqwest::StatusCode::UNAUTHORIZED);
}

// =========================================================================
// 8. PRODUCTION FAIL-CLOSED CONFIGURATION & SECRETS ISOLATION
// =========================================================================

#[test]
fn test_production_fails_closed_without_auth_config() {
    let mut config = AppConfig::default_local();
    config.environment = Environment::Production;
    config.api_key = None;
    config.api_keys.clear();
    config.google_client_id = None;
    config.google_client_ids.clear();

    let err = config.validate().unwrap_err();
    match err {
        ConfigError::InvalidValue { key, reason, .. } => {
            assert_eq!(key, "APP_ENV");
            assert!(reason.contains("Production environment requires at least one valid API_KEY"));
        }
    }
}

#[test]
fn test_auth_principal_and_config_secret_isolation() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("super_confidential_key_xyz987".to_string());
    config.api_keys = vec!["another_secret_key_1234".to_string()];

    let debug_config = format!("{config:?}");
    assert!(!debug_config.contains("super_confidential_key_xyz987"));
    assert!(!debug_config.contains("another_secret_key_1234"));
    assert!(debug_config.contains("[REDACTED]"));

    let principal = AuthPrincipal::new(
        "uid_sensitive_123",
        AuthType::GoogleOidc,
        ClientType::Flutter,
        Some("sensitive_user@example.com".to_string()),
        vec![AuthRole::User],
        vec!["chat".to_string()],
    );

    let debug_principal = format!("{principal:?}");
    assert!(!debug_principal.contains("sensitive_user@example.com"));
    assert!(debug_principal.contains("[REDACTED]"));
}

// =========================================================================
// 13. QUERY PARAMETER CREDENTIAL REJECTION TESTS (SECTION 5 MANDATE)
// =========================================================================

#[tokio::test]
async fn test_query_parameter_credentials_strictly_rejected() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_valid_api_key_8899".to_string());
    // Even if auth_required is explicitly false (development bypass on loopback),
    // query parameter credentials MUST be rejected!
    config.auth_required = Some(false);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    let query_credential_urls = [
        format!("{base_url}/api/v1/models?api_key=test_valid_api_key_8899"),
        format!("{base_url}/api/v1/models?apikey=test_valid_api_key_8899"),
        format!("{base_url}/api/v1/models?access_token=some_token_xyz"),
        format!("{base_url}/api/v1/models?token=some_token_xyz"),
        format!("{base_url}/api/v1/models?auth_token=some_token_xyz"),
        format!("{base_url}/api/v1/models?bearer=some_token_xyz"),
    ];

    for url in query_credential_urls {
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "Query credential in '{url}' must be rejected with HTTP 401"
        );

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("query parameters"),
            "Error message should explain that query credentials are prohibited"
        );
    }
}

// =========================================================================
// 14. ROUTE SCOPE AUTHORIZATION ENFORCEMENT TESTS (SECTION 9 MANDATE)
// =========================================================================

#[tokio::test]
async fn test_route_scope_authorization_enforcement() {
    let mut config = AppConfig::default_local();
    config.auth_required = Some(true);

    // Create a PAT with ONLY models:read scope
    let models_only_pat = "pat_models_read_only_abcdef1234567890abcdef1234567890abcdef12345678";
    let cred = ConfiguredCredential::from_pat(
        models_only_pat,
        "dev_models_auditor",
        None,
        vec![AuthRole::User],
        vec!["models:read".to_string()],
    );

    let mut pat_validator = PatValidator::new();
    pat_validator.add_credential(cred);

    let auth_service = Arc::new(AuthService::with_components(
        Arc::new(config.clone()),
        Arc::new(pat_validator),
        Arc::new(GoogleOidcValidator::new(
            Arc::new(MockJwksProvider::new()),
            vec![],
        )),
    ));

    let state = AppState::new(Arc::new(config)).with_auth_service(auth_service);
    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Permitted: GET /api/v1/models requires models:read
    let resp_ok = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {models_only_pat}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_ok.status(),
        reqwest::StatusCode::OK,
        "Principal with models:read scope must access /api/v1/models"
    );

    // 2. Forbidden: POST /api/v1/chat/completions requires chat scope
    let resp_chat_forbidden = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .header("Authorization", format!("Bearer {models_only_pat}"))
        .header("Content-Type", "application/json")
        .body(r#"{"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_chat_forbidden.status(),
        reqwest::StatusCode::FORBIDDEN,
        "Principal without chat scope must be rejected with HTTP 403 on chat route"
    );
    let chat_body: serde_json::Value = resp_chat_forbidden.json().await.unwrap();
    assert_eq!(chat_body["error"]["code"], "FORBIDDEN");
    assert!(
        chat_body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("lacks required scope 'chat'")
    );

    // 3. Forbidden: POST /api/v1/routing/preview requires routing:read scope
    let resp_routing_forbidden = client
        .post(format!("{base_url}/api/v1/routing/preview"))
        .header("Authorization", format!("Bearer {models_only_pat}"))
        .header("Content-Type", "application/json")
        .body(r#"{"prompt":"test"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_routing_forbidden.status(),
        reqwest::StatusCode::FORBIDDEN,
        "Principal without routing:read scope must be rejected with HTTP 403 on routing preview"
    );
    let routing_body: serde_json::Value = resp_routing_forbidden.json().await.unwrap();
    assert_eq!(routing_body["error"]["code"], "FORBIDDEN");

    // 4. Forbidden: GET /api/v1/usage/summary requires usage:read scope
    let resp_usage_forbidden = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .header("Authorization", format!("Bearer {models_only_pat}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_usage_forbidden.status(),
        reqwest::StatusCode::FORBIDDEN,
        "Principal without usage:read scope must be rejected with HTTP 403 on usage endpoint"
    );
}

// =========================================================================
// 15. EXTERNAL BIND & CLIENT TYPE PRIVILEGE ISOLATION (SECTION 6 & 9)
// =========================================================================

#[tokio::test]
async fn test_non_loopback_external_bind_blocks_all_protected_routes() {
    let mut config = AppConfig::default_local();
    // Simulate non-loopback external host (e.g. 0.0.0.0)
    config.host = "0.0.0.0".to_string();
    config.api_key = Some("configured_key_for_external_bind".to_string());
    config.auth_required = Some(false); // even if auth_required is false!
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Verify all protected endpoints reject unauthenticated access when host is non-loopback
    let protected_paths = [
        "/api/v1/models",
        "/api/v1/providers",
        "/api/v1/usage/summary",
        "/api/v1/quotas",
    ];

    for path in protected_paths {
        let resp = client
            .get(format!("{base_url}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "Non-loopback bind must reject unauthenticated requests to '{path}'"
        );
    }
}

#[tokio::test]
async fn test_client_type_header_cannot_grant_privileges() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_primary_api_key_12345".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Passing X-Client-Type alone without credentials must still be rejected
    let resp = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-Client-Type", "flutter")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let resp_ide = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-Client-Type", "custom-ide")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_ide.status(), reqwest::StatusCode::UNAUTHORIZED);
}

// =========================================================================
// 16. CONFIGURATION CONSTRAINTS & PUBLIC PROBE TRAILING SLASH
// =========================================================================

#[test]
fn test_config_non_loopback_host_enforces_auth_required() {
    let mut config = AppConfig::default_local();
    config.host = "0.0.0.0".to_string();
    config.api_key = None;
    config.api_keys = vec![];
    config.google_client_id = None;
    config.google_client_ids = vec![];

    // Non-loopback host automatically makes auth required even without explicit auth_required
    assert!(config.is_auth_required());
    assert!(!config.is_loopback_host());
}

#[test]
fn test_config_production_requires_https_google_jwks_url() {
    let mut config = AppConfig::default_local();
    config.environment = Environment::Production;
    config.api_key = Some("valid_production_key_12345".to_string());
    config.google_jwks_url = Some("http://insecure-jwks.example.com/certs".to_string());

    let err = config.validate().unwrap_err();
    match err {
        ConfigError::InvalidValue { key, reason, .. } => {
            assert_eq!(key, "GOOGLE_JWKS_URL");
            assert!(reason.contains("HTTPS protocol in production"));
        }
    }
}

#[tokio::test]
async fn test_public_probes_support_trailing_slashes() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("valid_secret_key_12345".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Verify trailing slash variants of public probes bypass authentication
    let resp_health = client
        .get(format!("{base_url}/api/v1/health/"))
        .send()
        .await
        .unwrap();
    // In Axum, whether 200 or 404, it must NOT return 401 Unauthorized
    assert_ne!(resp_health.status(), reqwest::StatusCode::UNAUTHORIZED);

    let resp_ready = client
        .get(format!("{base_url}/api/v1/ready/"))
        .send()
        .await
        .unwrap();
    assert_ne!(resp_ready.status(), reqwest::StatusCode::UNAUTHORIZED);
}

// =========================================================================
// 17. PERCENT-ENCODED QUERY REJECTION & OIDC AZP CLAIM SECURITY
// =========================================================================

#[tokio::test]
async fn test_query_parameter_percent_encoded_credentials_strictly_rejected() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_secret_key_12345".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // URLs with percent-encoded query parameters or hyphenated parameter variations
    let evasion_attempt_urls = vec![
        format!("{base_url}/api/v1/models?api%5fkey=some_secret_123"),
        format!("{base_url}/api/v1/models?api%5Fkey=some_secret_123"),
        format!("{base_url}/api/v1/models?%61%70%69%5f%6b%65%79=some_secret_123"),
        format!("{base_url}/api/v1/models?access%5ftoken=some_secret_123"),
        format!("{base_url}/api/v1/models?token%3dsome_secret_123"),
        format!("{base_url}/api/v1/models?api-key=some_secret_123"),
        format!("{base_url}/api/v1/models?access-token=some_secret_123"),
        format!("{base_url}/api/v1/models?auth-token=some_secret_123"),
        format!("{base_url}/api/v1/models?secret=some_secret_123"),
        format!("{base_url}/api/v1/models?bearer=some_secret_123"),
        format!("{base_url}/api/v1/models?other=valid;api_key=some_secret_123"),
    ];

    for url in evasion_attempt_urls {
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "Percent-encoded query credential in '{url}' must be rejected with HTTP 401"
        );

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("query parameters"),
            "Error message must indicate query parameters are prohibited for credentials"
        );
    }

    // Verify legitimate non-credential queries are NOT rejected as query credentials
    // (with valid header credentials, they succeed)
    let resp_legit = client
        .get(format!(
            "{base_url}/api/v1/models?limit=10&offset=0&category=fast"
        ))
        .header("Authorization", "Bearer test_secret_key_12345")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_legit.status(),
        reqwest::StatusCode::OK,
        "Legitimate non-credential query parameters must not be rejected"
    );
}

#[tokio::test]
async fn test_google_oidc_azp_validation_rejects_unauthorized_client_id() {
    let mut mock_jwks = MockJwksProvider::new();
    let decoding_key = DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC_KEY_PEM.as_bytes())
        .expect("Valid RSA public key");
    mock_jwks.add_key(TEST_KID, decoding_key);

    let validator = GoogleOidcValidator::new(Arc::new(mock_jwks), vec![TEST_CLIENT_ID.to_string()]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Token has valid audience (TEST_CLIENT_ID), but unauthorized presenter azp!
    let claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "google_user_sub_998877".to_string(),
        aud: serde_json::Value::String(TEST_CLIENT_ID.to_string()),
        exp: now + 3600,
        iat: Some(now - 10),
        nbf: Some(now - 10),
        azp: Some("unauthorized-foreign-app.apps.googleusercontent.com".to_string()),
        email: Some("attacker@example.com".to_string()),
        email_verified: Some(true),
        name: Some("Foreign Client User".to_string()),
        picture: None,
    };

    let token = create_test_jwt(&claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM)
        .expect("Signed test JWT");

    let result = validator.validate(&token, ClientType::Flutter).await;
    assert!(
        result.is_err(),
        "Token with unauthorized azp presenter must be rejected"
    );

    let err = result.unwrap_err();
    match err {
        ai_backend::auth::OidcError::InvalidAudience(msg) => {
            assert!(
                msg.contains("azp") && msg.contains("does not match"),
                "Error should specifically identify unauthorized azp: {msg}"
            );
        }
        other => panic!("Expected InvalidAudience error for azp mismatch, got: {other:?}"),
    }

    // Now test token with matching authorized azp: must succeed!
    let mut valid_azp_claims = claims;
    valid_azp_claims.azp = Some(TEST_CLIENT_ID.to_string());
    let valid_token = create_test_jwt(&valid_azp_claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM)
        .expect("Signed test JWT");

    let valid_result = validator.validate(&valid_token, ClientType::Flutter).await;
    assert!(
        valid_result.is_ok(),
        "Token with matching azp must be accepted"
    );
}

#[test]
fn test_config_production_rejects_auth_required_false() {
    let mut config = AppConfig::default_local();
    config.environment = Environment::Production;
    config.api_key = Some("valid_production_key_12345".to_string());
    config.auth_required = Some(false); // Explicitly attempting to disable auth in production

    let err = config.validate().unwrap_err();
    match err {
        ConfigError::InvalidValue { key, reason, .. } => {
            assert_eq!(key, "AUTH_REQUIRED");
            assert!(
                reason.contains("cannot be set to false in production"),
                "Expected error rejecting AUTH_REQUIRED=false in production: {reason}"
            );
        }
    }
}

#[test]
fn test_config_loopback_bracketed_ipv6_and_case_insensitive_localhost() {
    let mut config = AppConfig::default_local();

    // 1. Bracketed IPv6 loopback
    config.host = "[::1]".to_string();
    assert!(config.is_loopback_host());
    assert!(config.validate().is_ok());

    // 2. Standard IPv6 loopback
    config.host = "::1".to_string();
    assert!(config.is_loopback_host());
    assert!(config.validate().is_ok());

    // 3. Case-insensitive LOCALHOST
    config.host = "LOCALHOST".to_string();
    assert!(config.is_loopback_host());
    assert!(config.validate().is_ok());

    // 4. Wildcard/external non-loopback
    config.host = "0.0.0.0".to_string();
    assert!(!config.is_loopback_host());
}

#[test]
fn test_config_is_auth_required_precedence_in_production_and_external_bind() {
    let mut config = AppConfig::default_local();

    // Even if auth_required is explicitly Some(false):
    config.auth_required = Some(false);

    // If host is non-loopback (e.g. 0.0.0.0), is_auth_required MUST still return true!
    config.host = "0.0.0.0".to_string();
    config.environment = Environment::Development;
    assert!(
        config.is_auth_required(),
        "Non-loopback bind must force is_auth_required to true regardless of auth_required"
    );

    // If environment is Production, is_auth_required MUST still return true!
    config.host = "127.0.0.1".to_string();
    config.environment = Environment::Production;
    assert!(
        config.is_auth_required(),
        "Production environment must force is_auth_required to true regardless of auth_required"
    );

    // Only in development ON loopback can auth_required = Some(false) return false
    config.environment = Environment::Development;
    assert!(
        !config.is_auth_required(),
        "Development on loopback with explicit false must return false"
    );
}

#[tokio::test]
async fn test_google_oidc_multi_audience_requires_azp() {
    let mut mock_jwks = MockJwksProvider::new();
    let decoding_key = DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC_KEY_PEM.as_bytes())
        .expect("Valid RSA public key");
    mock_jwks.add_key(TEST_KID, decoding_key);

    let validator = GoogleOidcValidator::new(
        Arc::new(mock_jwks),
        vec![
            TEST_CLIENT_ID.to_string(),
            "second-authorized-client.apps.googleusercontent.com".to_string(),
        ],
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Token has multiple audiences in an array, but NO azp claim!
    let mut claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "multi_aud_user_12345".to_string(),
        aud: serde_json::json!([
            TEST_CLIENT_ID,
            "second-authorized-client.apps.googleusercontent.com"
        ]),
        exp: now + 3600,
        iat: Some(now - 10),
        nbf: Some(now - 10),
        azp: None, // Missing azp on multi-audience token per OIDC Core 1.0 §3.1.3.7
        email: Some("user@example.com".to_string()),
        email_verified: Some(true),
        name: Some("Multi Audience User".to_string()),
        picture: None,
    };

    let token = create_test_jwt(&claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM)
        .expect("Signed test JWT");

    let result = validator.validate(&token, ClientType::Flutter).await;
    assert!(
        result.is_err(),
        "OIDC Core 1.0 requires azp when ID token contains multiple audiences"
    );

    let err = result.unwrap_err();
    match err {
        ai_backend::auth::OidcError::InvalidAudience(msg) => {
            assert!(
                msg.contains("multiple audiences must contain an authorized party (azp)"),
                "Expected rejection message for missing azp on multi-audience token: {msg}"
            );
        }
        other => panic!("Expected InvalidAudience error, got: {other:?}"),
    }

    // Now supply valid matching azp: must succeed!
    claims.azp = Some(TEST_CLIENT_ID.to_string());
    let valid_token = create_test_jwt(&claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM)
        .expect("Signed test JWT");

    let valid_result = validator.validate(&valid_token, ClientType::Flutter).await;
    assert!(
        valid_result.is_ok(),
        "Token with multiple audiences and matching azp must be accepted"
    );
}

#[tokio::test]
async fn test_duplicate_and_conflicting_headers_prohibited() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("header_security_test_key_123".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Duplicate Authorization headers
    let resp_dup_auth = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", "Bearer header_security_test_key_123")
        .header("Authorization", "Bearer conflicting_key_456")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp_dup_auth.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Duplicate Authorization headers must be rejected"
    );
    let body: Value = resp_dup_auth.json().await.unwrap();
    assert_eq!(body["error"]["code"], "UNAUTHORIZED");

    // 2. Duplicate X-API-Key headers (HTTP layer sanitizes message via defense-in-depth)
    let resp_dup_key = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-API-Key", "header_security_test_key_123")
        .header("X-API-Key", "conflicting_key_456")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp_dup_key.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Duplicate X-API-Key headers must be rejected"
    );
    let body_key: Value = resp_dup_key.json().await.unwrap();
    assert_eq!(body_key["error"]["code"], "UNAUTHORIZED");

    // 3. Conflicting simultaneous credentials (both Authorization and X-API-Key)
    let resp_conflict = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", "Bearer header_security_test_key_123")
        .header("X-API-Key", "header_security_test_key_123")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp_conflict.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Simultaneous Authorization and X-API-Key must be rejected"
    );
    let body_conflict: Value = resp_conflict.json().await.unwrap();
    assert_eq!(body_conflict["error"]["code"], "UNAUTHORIZED");

    // 4. Direct AuthService unit validation to verify exact diagnostic errors
    let auth_service = AuthService::new(Arc::new(AppConfig::default_local()));

    // Verify duplicate Authorization error
    let mut h_dup_auth = axum::http::HeaderMap::new();
    h_dup_auth.append("authorization", "Bearer token1".parse().unwrap());
    h_dup_auth.append("authorization", "Bearer token2".parse().unwrap());
    let err_dup_auth = auth_service
        .authenticate(&h_dup_auth, "/api/v1/models", None)
        .await
        .unwrap_err();
    match err_dup_auth {
        ai_backend::errors::AppError::Unauthorized(msg) => {
            assert_eq!(msg, "Duplicate Authorization headers are prohibited");
        }
        other => panic!("Expected Unauthorized, got {other:?}"),
    }

    // Verify duplicate X-API-Key error
    let mut h_dup_key = axum::http::HeaderMap::new();
    h_dup_key.append("x-api-key", "key1".parse().unwrap());
    h_dup_key.append("x-api-key", "key2".parse().unwrap());
    let err_dup_key = auth_service
        .authenticate(&h_dup_key, "/api/v1/models", None)
        .await
        .unwrap_err();
    match err_dup_key {
        ai_backend::errors::AppError::Unauthorized(msg) => {
            assert_eq!(msg, "Duplicate X-API-Key headers are prohibited");
        }
        other => panic!("Expected Unauthorized, got {other:?}"),
    }

    // Verify conflicting simultaneous credentials error
    let mut h_conflict = axum::http::HeaderMap::new();
    h_conflict.insert("authorization", "Bearer token1".parse().unwrap());
    h_conflict.insert("x-api-key", "key1".parse().unwrap());
    let err_conflict = auth_service
        .authenticate(&h_conflict, "/api/v1/models", None)
        .await
        .unwrap_err();
    match err_conflict {
        ai_backend::errors::AppError::Unauthorized(msg) => {
            assert!(msg.contains("Conflicting authentication credentials"));
        }
        other => panic!("Expected Unauthorized, got {other:?}"),
    }
}

#[tokio::test]
async fn test_oversized_credentials_rejected() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("valid_key_oversized_test_123".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Oversized Bearer token (> 8192 bytes)
    let huge_token = "A".repeat(8193);
    let resp_huge_token = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", format!("Bearer {huge_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp_huge_token.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Bearer token exceeding 8192 bytes must be rejected with 401"
    );
    let body_token: Value = resp_huge_token.json().await.unwrap();
    assert_eq!(body_token["error"]["code"], "UNAUTHORIZED");

    // 2. Oversized X-API-Key (> 8192 bytes)
    let huge_key = "K".repeat(8193);
    let resp_huge_key = client
        .get(format!("{base_url}/api/v1/models"))
        .header("X-API-Key", huge_key)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp_huge_key.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "X-API-Key exceeding 8192 bytes must be rejected with 401"
    );
    let body_key: Value = resp_huge_key.json().await.unwrap();
    assert_eq!(body_key["error"]["code"], "UNAUTHORIZED");

    // 3. Direct AuthService unit check for exact diagnostic strings
    let auth_service = AuthService::new(Arc::new(AppConfig::default_local()));

    let mut h_huge_token = axum::http::HeaderMap::new();
    let bearer_huge = format!("Bearer {}", "A".repeat(8193));
    h_huge_token.insert("authorization", bearer_huge.parse().unwrap());
    let err_token = auth_service
        .authenticate(&h_huge_token, "/api/v1/models", None)
        .await
        .unwrap_err();
    match err_token {
        ai_backend::errors::AppError::Unauthorized(msg) => {
            assert!(msg.contains("8192 bytes"));
        }
        other => panic!("Expected Unauthorized, got {other:?}"),
    }

    let mut h_huge_key = axum::http::HeaderMap::new();
    h_huge_key.insert("x-api-key", "K".repeat(8193).parse().unwrap());
    let err_key = auth_service
        .authenticate(&h_huge_key, "/api/v1/models", None)
        .await
        .unwrap_err();
    match err_key {
        ai_backend::errors::AppError::Unauthorized(msg) => {
            assert!(msg.contains("8192 bytes"));
        }
        other => panic!("Expected Unauthorized, got {other:?}"),
    }
}

#[tokio::test]
async fn test_invalid_utf8_header_rejected() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("valid_key_utf8_test_123".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Non-ASCII, non-UTF-8 bytes (0x80, 0x81) in Authorization header
    let invalid_bytes_header = reqwest::header::HeaderValue::from_bytes(b"Bearer \x80\x81\x82")
        .expect("Valid HTTP bytes representation");

    let resp = client
        .get(format!("{base_url}/api/v1/models"))
        .header("Authorization", invalid_bytes_header)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Non-UTF-8 Authorization header must be rejected"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Invalid non-UTF-8 characters in Authorization header"),
        "Expected error message for non-UTF-8 header: {body:?}"
    );
}

#[tokio::test]
async fn test_concurrent_oidc_authentication_and_jwks_concurrency() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut mock_jwks = MockJwksProvider::new();
    let decoding_key = DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC_KEY_PEM.as_bytes()).unwrap();
    mock_jwks.add_key(TEST_KID, decoding_key);

    let mut config = AppConfig::default_local();
    config.api_key = Some("concurrent_api_key_test_99".to_string());
    config.auth_required = Some(true);
    let config = Arc::new(config);

    let auth_service = Arc::new(AuthService::with_mock_jwks(
        config.clone(),
        Arc::new(mock_jwks),
        vec![TEST_CLIENT_ID.to_string()],
    ));

    let state = AppState::new(config).with_auth_service(auth_service);
    let (base_url, _server) = spawn_auth_test_backend(state).await;

    // 1. Valid OIDC claims
    let valid_claims = GoogleClaims {
        iss: "https://accounts.google.com".to_string(),
        sub: "concurrent_user_001".to_string(),
        aud: json!(TEST_CLIENT_ID),
        exp: now + 3600,
        iat: Some(now - 10),
        nbf: Some(now - 10),
        azp: Some(TEST_CLIENT_ID.to_string()),
        email: Some("concurrent@example.com".to_string()),
        email_verified: Some(true),
        name: Some("Concurrent User".to_string()),
        picture: None,
    };
    let valid_oidc_token =
        create_test_jwt(&valid_claims, Some(TEST_KID), TEST_RSA_PRIVATE_KEY_PEM).unwrap();

    // 2. Unknown kid OIDC claims (triggers unknown kid path)
    let unknown_kid_token = create_test_jwt(
        &valid_claims,
        Some("unknown_rotated_kid"),
        TEST_RSA_PRIVATE_KEY_PEM,
    )
    .unwrap();

    // Launch 30 concurrent requests (10 valid OIDC, 10 unknown kid, 10 API key)
    let mut join_set = tokio::task::JoinSet::new();

    for i in 0..30 {
        let base_url = base_url.clone();
        let valid_oidc = valid_oidc_token.clone();
        let unknown_oidc = unknown_kid_token.clone();

        join_set.spawn(async move {
            let client = reqwest::Client::new();
            let case_type = i % 3;
            let resp = match case_type {
                0 => {
                    // Valid OIDC token
                    client
                        .get(format!("{base_url}/api/v1/models"))
                        .header("Authorization", format!("Bearer {valid_oidc}"))
                        .header("X-Client-Type", "flutter")
                        .send()
                        .await
                        .expect("Valid OIDC request failed to execute")
                }
                1 => {
                    // Unknown kid OIDC token
                    client
                        .get(format!("{base_url}/api/v1/models"))
                        .header("Authorization", format!("Bearer {unknown_oidc}"))
                        .header("X-Client-Type", "flutter")
                        .send()
                        .await
                        .expect("Unknown kid request failed to execute")
                }
                _ => {
                    // Valid API key
                    client
                        .get(format!("{base_url}/api/v1/models"))
                        .header("X-API-Key", "concurrent_api_key_test_99")
                        .header("X-Client-Type", "custom-ide")
                        .send()
                        .await
                        .expect("API key request failed to execute")
                }
            };
            (case_type, resp.status())
        });
    }

    let mut valid_oidc_count = 0;
    let mut unknown_kid_count = 0;
    let mut api_key_count = 0;

    while let Some(res) = join_set.join_next().await {
        let (case_type, status) = res.expect("Concurrent task panicked");
        match case_type {
            0 => {
                assert_eq!(
                    status,
                    reqwest::StatusCode::OK,
                    "Concurrent valid OIDC token must succeed with 200 OK"
                );
                valid_oidc_count += 1;
            }
            1 => {
                assert_eq!(
                    status,
                    reqwest::StatusCode::UNAUTHORIZED,
                    "Concurrent unknown-kid token must fail closed with 401 Unauthorized"
                );
                unknown_kid_count += 1;
            }
            _ => {
                assert_eq!(
                    status,
                    reqwest::StatusCode::OK,
                    "Concurrent valid API key must succeed with 200 OK"
                );
                api_key_count += 1;
            }
        }
    }

    assert_eq!(valid_oidc_count, 10);
    assert_eq!(unknown_kid_count, 10);
    assert_eq!(api_key_count, 10);
}
