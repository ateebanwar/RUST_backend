//! Comprehensive integration verification tests for Phase 11 — Authentication & Authorization.
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
    AuthPrincipal, AuthRole, AuthService, AuthType, ClientType, GoogleClaims, MockJwksProvider,
    PatValidator, generate_secure_pat,
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
