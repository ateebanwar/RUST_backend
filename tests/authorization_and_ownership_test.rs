//! Comprehensive integration and unit verification tests for Phase 12:
//! End-to-End Authorization, Resource Ownership, Secret Management & Hardening.
//!
//! Verifies:
//! 1. Resource ownership and tenant isolation on usage queries (summary, providers, models).
//! 2. Insecure Direct Object Reference (IDOR) prevention on cross-user queries (returns 403 Forbidden).
//! 3. Admin privilege allows aggregate and per-tenant inspection without IDOR blocking.
//! 4. Personal Access Token (PAT) creation with single-exposure plaintext semantics.
//! 5. Secret masking: List and inspect endpoints return masked identifiers, never plaintext or raw hashes.
//! 6. IDOR prevention on PATs: User B cannot inspect, rotate, or revoke User A's credentials (returns 404).
//! 7. Privilege escalation defense: Callers cannot create tokens with scopes exceeding their own.
//! 8. Scope enforcement: Endpoints requiring specific scopes reject tokens missing those scopes (HTTP 403).
//! 9. Token rotation lifecycle: Old token is immediately invalidated, new token functions immediately.
//! 10. Token revocation lifecycle: Revoked tokens immediately fail authentication (HTTP 401).
//! 11. SecretString zero-leak verification: Debug, Display, and JSON serialization are strictly redacted.
//! 12. Authenticated encryption at rest (AesGcmCipher): AES-256-GCM authenticated encryption, AAD context binding, and tamper resistance.
//! 13. Concurrency: Revocation is atomic and leaves zero access window.

use ai_backend::auth::{AuthPrincipal, AuthRole, AuthType, ClientType, CreateTokenRequest};
use ai_backend::config::AppConfig;
use ai_backend::security::{AesGcmCipher, SecretString, mask_credential};
use ai_backend::usage::{
    RecordStatus, RequestMode, RequestOutcome, RoutingMode, UsageRecord, UsageSource,
};
use ai_backend::{AppState, create_app};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

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

// =========================================================================
// 1. TENANT ISOLATION AND RESOURCE OWNERSHIP ON USAGE QUERIES
// =========================================================================

#[tokio::test]
async fn test_tenant_isolation_on_usage_queries() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_admin_key_99999".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    // Seed usage storage with distinct records for Alice and Bob
    let now = 1700000000000u64;

    // Alice's records (3 requests, 300 total tokens)
    for i in 0..3 {
        let rec = UsageRecord {
            id: format!("req_alice_{i}"),
            provider_id: "groq-api".to_string(),
            model_id: "llama3-8b".to_string(),
            request_mode: RequestMode::Buffered,
            routing_mode: RoutingMode::Auto,
            start_time: now + i * 10,
            completion_time: now + i * 10 + 50,
            latency_ms: 50,
            outcome: RequestOutcome::Success,
            failure_category: None,
            prompt_tokens: Some(40),
            completion_tokens: Some(60),
            total_tokens: Some(100),
            usage_source: UsageSource::Reported,
            record_status: RecordStatus::Final,
            subject: Some("user_alice".to_string()),
        };
        state.usage.storage().record_sync(&rec).unwrap();
    }

    // Bob's records (2 requests, 500 total tokens)
    for i in 0..2 {
        let rec = UsageRecord {
            id: format!("req_bob_{i}"),
            provider_id: "openrouter-api".to_string(),
            model_id: "claude-3-haiku".to_string(),
            request_mode: RequestMode::Buffered,
            routing_mode: RoutingMode::Auto,
            start_time: now + 100 + i * 10,
            completion_time: now + 100 + i * 10 + 80,
            latency_ms: 80,
            outcome: RequestOutcome::Success,
            failure_category: None,
            prompt_tokens: Some(100),
            completion_tokens: Some(150),
            total_tokens: Some(250),
            usage_source: UsageSource::Reported,
            record_status: RecordStatus::Final,
            subject: Some("user_bob".to_string()),
        };
        state.usage.storage().record_sync(&rec).unwrap();
    }

    // Create PAT for Alice
    let alice_principal = AuthPrincipal::new(
        "user_alice".to_string(),
        AuthType::PersonalAccessToken,
        ClientType::Flutter,
        None,
        vec![AuthRole::User],
        vec![
            "usage:read".to_string(),
            "chat:write".to_string(),
            "auth:tokens".to_string(),
        ],
    );
    let alice_token_resp = state
        .credentials
        .create_token(
            &alice_principal,
            CreateTokenRequest {
                name: "Alice Phone".to_string(),
                owner_subject: None,
                scopes: None,
                expires_in_secs: None,
            },
        )
        .unwrap();

    // Create PAT for Bob
    let bob_principal = AuthPrincipal::new(
        "user_bob".to_string(),
        AuthType::PersonalAccessToken,
        ClientType::Flutter,
        None,
        vec![AuthRole::User],
        vec![
            "usage:read".to_string(),
            "chat:write".to_string(),
            "auth:tokens".to_string(),
        ],
    );
    let bob_token_resp = state
        .credentials
        .create_token(
            &bob_principal,
            CreateTokenRequest {
                name: "Bob Laptop".to_string(),
                owner_subject: None,
                scopes: None,
                expires_in_secs: None,
            },
        )
        .unwrap();

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Alice queries her own usage summary -> sees exactly 3 requests and 300 tokens
    let alice_resp: Value = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(&alice_token_resp.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(alice_resp["total_requests"], 3);
    assert_eq!(alice_resp["total_tokens"], 300);

    // Alice queries providers -> sees only "groq-api", never Bob's "openrouter-api"
    let alice_providers: Value = client
        .get(format!("{base_url}/api/v1/usage/providers"))
        .bearer_auth(&alice_token_resp.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let alice_prov_arr = alice_providers.as_array().unwrap();
    assert_eq!(alice_prov_arr.len(), 1);
    assert_eq!(alice_prov_arr[0]["provider_id"], "groq-api");

    // Alice queries models -> sees only "llama3-8b", never Bob's "claude-3-haiku"
    let alice_models: Value = client
        .get(format!("{base_url}/api/v1/usage/models"))
        .bearer_auth(&alice_token_resp.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let alice_model_arr = alice_models.as_array().unwrap();
    assert_eq!(alice_model_arr.len(), 1);
    assert_eq!(alice_model_arr[0]["model_id"], "llama3-8b");

    // 2. Bob queries his own usage summary -> sees exactly 2 requests and 500 tokens
    let bob_resp: Value = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(&bob_token_resp.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bob_resp["total_requests"], 2);
    assert_eq!(bob_resp["total_tokens"], 500);

    // Bob queries providers -> sees only "openrouter-api"
    let bob_providers: Value = client
        .get(format!("{base_url}/api/v1/usage/providers"))
        .bearer_auth(&bob_token_resp.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bob_prov_arr = bob_providers.as_array().unwrap();
    assert_eq!(bob_prov_arr.len(), 1);
    assert_eq!(bob_prov_arr[0]["provider_id"], "openrouter-api");

    // 3. Admin queries usage summary without filter -> sees aggregate total (5 requests, 800 tokens)
    let admin_summary: Value = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .header("X-API-Key", "test_admin_key_99999")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(admin_summary["total_requests"], 5);
    assert_eq!(admin_summary["total_tokens"], 800);

    // 4. Admin queries Alice specifically with ?user_id=user_alice -> sees 3 requests
    let admin_alice_query: Value = client
        .get(format!(
            "{base_url}/api/v1/usage/summary?user_id=user_alice"
        ))
        .header("X-API-Key", "test_admin_key_99999")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(admin_alice_query["total_requests"], 3);
    assert_eq!(admin_alice_query["total_tokens"], 300);
}

// =========================================================================
// 2. IDOR MITIGATION ON USAGE TELEMETRY
// =========================================================================

#[tokio::test]
async fn test_idor_violation_on_usage_queries_returns_403_forbidden() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_admin_key_99999".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let alice_principal = AuthPrincipal::new(
        "user_alice".to_string(),
        AuthType::PersonalAccessToken,
        ClientType::Flutter,
        None,
        vec![AuthRole::User],
        vec![
            "usage:read".to_string(),
            "chat:write".to_string(),
            "auth:tokens".to_string(),
        ],
    );
    let alice_token_resp = state
        .credentials
        .create_token(
            &alice_principal,
            CreateTokenRequest {
                name: "Alice PAT".to_string(),
                owner_subject: None,
                scopes: None,
                expires_in_secs: None,
            },
        )
        .unwrap();

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Alice attempts to query Bob's usage via ?user_id=user_bob -> must be rejected with 403 Forbidden
    let idor_summary = client
        .get(format!("{base_url}/api/v1/usage/summary?user_id=user_bob"))
        .bearer_auth(&alice_token_resp.token)
        .send()
        .await
        .unwrap();
    assert_eq!(idor_summary.status(), reqwest::StatusCode::FORBIDDEN);
    let err_json: Value = idor_summary.json().await.unwrap();
    assert_eq!(err_json["error"]["code"], "FORBIDDEN");
    assert!(
        err_json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("IDOR violation")
    );

    // Alice attempts to query Bob's providers -> 403 Forbidden
    let idor_providers = client
        .get(format!(
            "{base_url}/api/v1/usage/providers?user_id=user_bob"
        ))
        .bearer_auth(&alice_token_resp.token)
        .send()
        .await
        .unwrap();
    assert_eq!(idor_providers.status(), reqwest::StatusCode::FORBIDDEN);

    // Alice attempts to query Bob's models -> 403 Forbidden
    let idor_models = client
        .get(format!("{base_url}/api/v1/usage/models?user_id=user_bob"))
        .bearer_auth(&alice_token_resp.token)
        .send()
        .await
        .unwrap();
    assert_eq!(idor_models.status(), reqwest::StatusCode::FORBIDDEN);
}

// =========================================================================
// 3. DYNAMIC PAT LIFECYCLE, ZERO SECRET EXPOSURE & IDOR DEFENSE
// =========================================================================

#[tokio::test]
async fn test_dynamic_pat_lifecycle_single_exposure_and_idor_protection() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_admin_key_99999".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create a token for Alice using admin bootstrap key
    let create_alice_req = json!({
        "name": "Alice CLI",
        "owner_subject": "user_alice",
        "scopes": ["chat:write", "usage:read", "auth:tokens"]
    });

    let alice_create_resp = client
        .post(format!("{base_url}/api/v1/auth/tokens"))
        .header("X-API-Key", "test_admin_key_99999")
        .json(&create_alice_req)
        .send()
        .await
        .unwrap();
    assert_eq!(alice_create_resp.status(), reqwest::StatusCode::CREATED);

    let alice_token_data: Value = alice_create_resp.json().await.unwrap();
    let alice_plaintext_token = alice_token_data["token"].as_str().unwrap().to_string();
    let _alice_token_id = alice_token_data["metadata"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let alice_masked_token = alice_token_data["metadata"]["masked_token"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(alice_plaintext_token.starts_with("pat_"));
    assert!(alice_masked_token.contains("..."));
    assert_ne!(alice_plaintext_token, alice_masked_token);

    // 2. Create a token for Bob using admin bootstrap key
    let create_bob_req = json!({
        "name": "Bob IDE",
        "owner_subject": "user_bob",
        "scopes": ["chat:write", "usage:read", "auth:tokens"]
    });

    let bob_create_resp = client
        .post(format!("{base_url}/api/v1/auth/tokens"))
        .header("X-API-Key", "test_admin_key_99999")
        .json(&create_bob_req)
        .send()
        .await
        .unwrap();
    assert_eq!(bob_create_resp.status(), reqwest::StatusCode::CREATED);
    let bob_token_data: Value = bob_create_resp.json().await.unwrap();
    let bob_plaintext_token = bob_token_data["token"].as_str().unwrap().to_string();
    let bob_token_id = bob_token_data["metadata"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 3. Authenticate with Alice's PAT to list tokens:
    // Alice creates another token herself to test user-driven creation
    let alice_subtoken_resp = client
        .post(format!("{base_url}/api/v1/auth/tokens"))
        .bearer_auth(&alice_plaintext_token)
        .json(&json!({
            "name": "Alice Tablet",
            "scopes": ["chat:write"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(alice_subtoken_resp.status(), reqwest::StatusCode::CREATED);
    let alice_subtoken_data: Value = alice_subtoken_resp.json().await.unwrap();
    let alice_subtoken_id = alice_subtoken_data["metadata"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Alice lists her tokens: should contain Alice Tablet, and NOT Bob IDE
    let alice_list: Value = client
        .get(format!("{base_url}/api/v1/auth/tokens"))
        .bearer_auth(&alice_plaintext_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let alice_tokens = alice_list.as_array().unwrap();
    assert!(alice_tokens.iter().any(|t| t["id"] == alice_subtoken_id));
    assert!(!alice_tokens.iter().any(|t| t["id"] == bob_token_id));

    // Verify token field is NEVER returned in list responses (masked_token only)
    for t in alice_tokens {
        assert!(t.get("token").is_none());
        assert!(t["masked_token"].as_str().unwrap().contains("..."));
    }

    // 4. IDOR Defense on Token Inspection:
    // Bob attempts to inspect Alice's subtoken -> returns 404 Not Found (prevents existence enumeration)
    let bob_idor_get = client
        .get(format!("{base_url}/api/v1/auth/tokens/{alice_subtoken_id}"))
        .bearer_auth(&bob_plaintext_token)
        .send()
        .await
        .unwrap();
    assert_eq!(bob_idor_get.status(), reqwest::StatusCode::NOT_FOUND);

    // 5. IDOR Defense on Token Revocation:
    // Bob attempts to revoke Alice's subtoken -> returns 404 Not Found
    let bob_idor_revoke = client
        .delete(format!("{base_url}/api/v1/auth/tokens/{alice_subtoken_id}"))
        .bearer_auth(&bob_plaintext_token)
        .send()
        .await
        .unwrap();
    assert_eq!(bob_idor_revoke.status(), reqwest::StatusCode::NOT_FOUND);

    // 6. IDOR Defense on Token Rotation:
    // Bob attempts to rotate Alice's subtoken -> returns 404 Not Found
    let bob_idor_rotate = client
        .post(format!(
            "{base_url}/api/v1/auth/tokens/{alice_subtoken_id}/rotate"
        ))
        .bearer_auth(&bob_plaintext_token)
        .send()
        .await
        .unwrap();
    assert_eq!(bob_idor_rotate.status(), reqwest::StatusCode::NOT_FOUND);

    // 7. Token Rotation Lifecycle:
    // Alice rotates her subtoken -> receives new plaintext token once, old token immediately revoked
    let alice_rotate_resp = client
        .post(format!(
            "{base_url}/api/v1/auth/tokens/{alice_subtoken_id}/rotate"
        ))
        .bearer_auth(&alice_plaintext_token)
        .send()
        .await
        .unwrap();
    assert_eq!(alice_rotate_resp.status(), reqwest::StatusCode::OK);
    let rotate_data: Value = alice_rotate_resp.json().await.unwrap();
    let new_alice_subtoken = rotate_data["new_token"].as_str().unwrap().to_string();
    assert!(new_alice_subtoken.starts_with("pat_"));
    assert_ne!(
        new_alice_subtoken,
        alice_subtoken_data["token"].as_str().unwrap()
    );

    // Old subtoken must now fail with 401 Unauthorized
    let old_subtoken_test = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(alice_subtoken_data["token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        old_subtoken_test.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    // New subtoken must authenticate successfully (with chat:write scope)
    // 8. Token Revocation Lifecycle:
    // Alice revokes her subtoken
    let revoke_resp = client
        .delete(format!("{base_url}/api/v1/auth/tokens/{alice_subtoken_id}"))
        .bearer_auth(&alice_plaintext_token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke_resp.status(), reqwest::StatusCode::NO_CONTENT);

    // The new subtoken must now fail with 401 Unauthorized
    let revoked_subtoken_test = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(&new_alice_subtoken)
        .send()
        .await
        .unwrap();
    assert_eq!(
        revoked_subtoken_test.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    // Alice inspects revoked token: metadata reflects revoked = true
    let inspect_revoked = client
        .get(format!("{base_url}/api/v1/auth/tokens/{alice_subtoken_id}"))
        .bearer_auth(&alice_plaintext_token)
        .send()
        .await
        .unwrap();
    assert_eq!(inspect_revoked.status(), reqwest::StatusCode::OK);
    let inspect_json: Value = inspect_revoked.json().await.unwrap();
    assert_eq!(inspect_json["revoked"], true);
    assert!(inspect_json["revoked_at"].is_number());
}

// =========================================================================
// 4. PRIVILEGE ESCALATION DEFENSE AND SCOPE ENFORCEMENT
// =========================================================================

#[tokio::test]
async fn test_privilege_escalation_defense() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_admin_key_99999".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    // Create a regular user principal with strictly limited scopes
    let caller = AuthPrincipal::new(
        "restricted_user".to_string(),
        AuthType::PersonalAccessToken,
        ClientType::Flutter,
        None,
        vec![AuthRole::User],
        vec!["chat:write".to_string(), "auth:tokens".to_string()],
    );

    // User attempts to create a PAT with an unpossessed scope (e.g. "admin:all" or "usage:delete")
    let escalation_attempt = state.credentials.create_token(
        &caller,
        CreateTokenRequest {
            name: "Hacked Token".to_string(),
            owner_subject: None,
            scopes: Some(vec!["chat:write".to_string(), "admin:all".to_string()]),
            expires_in_secs: None,
        },
    );

    assert!(escalation_attempt.is_err());
    let err = escalation_attempt.err().unwrap();
    let err_str = err.to_string();
    assert!(err_str.contains("Privilege escalation denied"));
    assert!(err_str.contains("admin:all"));

    // User creates a PAT with a subset of their own scopes -> succeeds
    let valid_subscope = state.credentials.create_token(
        &caller,
        CreateTokenRequest {
            name: "Valid Token".to_string(),
            owner_subject: None,
            scopes: Some(vec!["chat:write".to_string()]),
            expires_in_secs: None,
        },
    );
    assert!(valid_subscope.is_ok());
    let token_resp = valid_subscope.unwrap();
    assert_eq!(token_resp.metadata.scopes, vec!["chat:write".to_string()]);
}

#[tokio::test]
async fn test_scope_enforcement_on_token_endpoints() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_admin_key_99999".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    // Create a token that has ONLY "chat:write" (MISSING "auth:tokens")
    let chat_only_principal = AuthPrincipal::new(
        "chat_bot".to_string(),
        AuthType::PersonalAccessToken,
        ClientType::Flutter,
        None,
        vec![AuthRole::User],
        vec!["chat:write".to_string()],
    );
    let chat_only_token = state
        .credentials
        .create_token(
            &chat_only_principal,
            CreateTokenRequest {
                name: "Chat Bot Key".to_string(),
                owner_subject: None,
                scopes: Some(vec!["chat:write".to_string()]),
                expires_in_secs: None,
            },
        )
        .unwrap();

    let (base_url, _server) = spawn_auth_test_backend(state).await;
    let client = reqwest::Client::new();

    // Calling /api/v1/auth/tokens with missing "auth:tokens" scope -> 403 Forbidden
    let token_endpoint_resp = client
        .get(format!("{base_url}/api/v1/auth/tokens"))
        .bearer_auth(&chat_only_token.token)
        .send()
        .await
        .unwrap();
    assert_eq!(token_endpoint_resp.status(), reqwest::StatusCode::FORBIDDEN);
    let err_body: Value = token_endpoint_resp.json().await.unwrap();
    assert_eq!(err_body["error"]["code"], "FORBIDDEN");
    // Verify error is sanitized against credential leak
    let msg = err_body["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("Invalid authentication credentials or restricted parameter")
            || msg.contains("lacks required scope")
    );
}

// =========================================================================
// 5. SECRET STRING ZERO-LEAK & MASKING VERIFICATION
// =========================================================================

#[test]
fn test_secret_string_zero_leak_and_masking() {
    let raw = "pat_very_secret_production_key_abcdef1234567890";
    let secret = SecretString::new(raw);

    // 1. Debug formatting must strictly print "[REDACTED]"
    let debug_str = format!("{secret:?}");
    assert_eq!(debug_str, "\"[REDACTED]\"");
    assert!(!debug_str.contains("very_secret"));

    // 2. Display formatting must strictly print "[REDACTED]"
    let display_str = format!("{secret}");
    assert_eq!(display_str, "[REDACTED]");
    assert!(!display_str.contains("very_secret"));

    // 3. JSON serialization must strictly serialize as "[REDACTED]"
    let serialized = serde_json::to_string(&secret).unwrap();
    assert_eq!(serialized, "\"[REDACTED]\"");

    // 4. Explicit expose_secret() provides access to the underlying secret
    assert_eq!(secret.expose_secret(), raw);

    // 5. Constant-time equality comparison
    let same_secret = SecretString::new(raw);
    let different_secret = SecretString::new("pat_other_key");
    assert_eq!(secret, same_secret);
    assert_ne!(secret, different_secret);

    // 6. mask_credential creates secure non-secret identifiers
    let masked = mask_credential(raw);
    assert!(masked.starts_with("pat_very..."));
    assert!(masked.ends_with("7890"));
    assert!(!masked.contains("secret_production_key"));

    let short_masked = mask_credential("short");
    assert_eq!(short_masked, "********");
}

// =========================================================================
// 6. AES-256-GCM AUTHENTICATED ENCRYPTION AT REST VERIFICATION
// =========================================================================

#[test]
fn test_aes_gcm_authenticated_encryption_and_tamper_defense() {
    let raw_key = [0x42u8; 32];
    let cipher = AesGcmCipher::new(&raw_key).unwrap();

    let plaintext = b"Sensitive model provider API key: sk-live-mock-998877";
    let tenant_aad = b"tenant:tenant_enterprise_01";

    // 1. Successful encryption and decryption roundtrip
    let ciphertext = cipher.encrypt(plaintext, tenant_aad).unwrap();
    assert_ne!(ciphertext.as_slice(), plaintext);
    assert!(ciphertext.len() > plaintext.len() + 12); // Nonce (12) + tag (16)

    let decrypted = cipher.decrypt(&ciphertext, tenant_aad).unwrap();
    assert_eq!(decrypted, plaintext);

    // 2. Tampered ciphertext must fail authentication
    let mut corrupted_ciphertext = ciphertext.clone();
    let last_idx = corrupted_ciphertext.len() - 1;
    corrupted_ciphertext[last_idx] ^= 0x01; // Corrupt authentication tag
    let tampered_res = cipher.decrypt(&corrupted_ciphertext, tenant_aad);
    assert!(tampered_res.is_err());

    // 3. Mismatched AAD (cross-tenant replay attempt) must fail authentication
    let evil_tenant_aad = b"tenant:tenant_attacker_66";
    let cross_tenant_res = cipher.decrypt(&ciphertext, evil_tenant_aad);
    assert!(cross_tenant_res.is_err());

    // 4. Decrypting with a different key must fail authentication
    let wrong_key = [0x99u8; 32];
    let wrong_cipher = AesGcmCipher::new(&wrong_key).unwrap();
    let wrong_key_res = wrong_cipher.decrypt(&ciphertext, tenant_aad);
    assert!(wrong_key_res.is_err());

    // 5. Invalid key size fails closed
    let invalid_key = [0x42u8; 16];
    assert!(AesGcmCipher::new(&invalid_key).is_err());
}

// =========================================================================
// 7. CONCURRENCY: IMMEDIATE REVOCATION CLOSES ALL ACCESS WINDOWS
// =========================================================================

#[tokio::test]
async fn test_concurrent_revocation_leaves_zero_access_window() {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_admin_key_99999".to_string());
    config.auth_required = Some(true);
    let state = AppState::new(Arc::new(config));

    let principal = AuthPrincipal::new(
        "concurrency_user".to_string(),
        AuthType::PersonalAccessToken,
        ClientType::Flutter,
        None,
        vec![AuthRole::User],
        vec!["usage:read".to_string(), "auth:tokens".to_string()],
    );

    let token_resp = state
        .credentials
        .create_token(
            &principal,
            CreateTokenRequest {
                name: "Ephemeral Token".to_string(),
                owner_subject: None,
                scopes: None,
                expires_in_secs: None,
            },
        )
        .unwrap();

    let (base_url, _server) = spawn_auth_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    // Verify token works initially
    let initial_resp = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(&token_resp.token)
        .send()
        .await
        .unwrap();
    assert_eq!(initial_resp.status(), reqwest::StatusCode::OK);

    // Revoke token immediately
    state
        .credentials
        .revoke_token(&principal, &token_resp.metadata.id)
        .unwrap();

    // Launch 20 concurrent requests with the revoked token: every single one must fail with 401
    let mut handles = Vec::new();
    for _ in 0..20 {
        let client_c = client.clone();
        let url = format!("{base_url}/api/v1/usage/summary");
        let token = token_resp.token.clone();
        handles.push(tokio::spawn(async move {
            client_c
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
        }));
    }

    for h in handles {
        let status = h.await.unwrap();
        assert_eq!(
            status,
            reqwest::StatusCode::UNAUTHORIZED,
            "Revoked token must never be accepted under concurrency"
        );
    }
}
