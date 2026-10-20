//! Integration and end-to-end contract tests for ai-backend.
//!
//! Validates:
//! 1. Health & Readiness endpoints (API_SPEC.md §2).
//! 2. Model & Provider catalog endpoints (API_SPEC.md §3).
//! 3. Routing preview endpoint (API_SPEC.md §4).
//! 4. Canonical error envelopes & status codes (API_SPEC.md §1.3).
//! 5. Configuration validation & zero-secret leakage (SECURITY.md).

use ai_backend::config::{AppConfig, ConfigError, DeploymentMode, Environment};
use ai_backend::errors::AppError;
use ai_backend::router::RoutingPreviewRequest;
use ai_backend::{AppState, dispatch_route};
use std::sync::Arc;

#[test]
fn test_health_endpoint_contract() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let (status, headers, body) = dispatch_route("GET", "/api/v1/health", &state);

    // 1. Verify HTTP Status 200 OK
    assert_eq!(status, 200);

    // 2. Verify Content-Type Header
    assert_eq!(headers[0], ("content-type", "application/json"));

    // 3. Verify Required Fields per API_SPEC.md §2.1
    assert!(body.contains(r#""status":"healthy""#));
    assert!(body.contains(r#""version":"0.1.0""#));
    assert!(body.contains(r#""timestamp":""#));

    // 4. Verify No Secrets Leaked
    assert!(!body.contains("secret"));
    assert!(!body.contains("key"));
}

#[test]
fn test_readiness_endpoint_contract() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let (status, headers, body) = dispatch_route("GET", "/api/v1/ready", &state);

    // 1. Verify HTTP Status 503 Service Unavailable when 0 active providers exist per API_SPEC.md §2.2
    assert_eq!(status, 503);

    // 2. Verify Content-Type Header
    assert_eq!(headers[0], ("content-type", "application/json"));

    // 3. Verify Required Fields per API_SPEC.md §2.2
    assert!(body.contains(r#""status":"not_ready""#));
    assert!(body.contains(r#""active_providers":0"#));
    assert!(body.contains(r#""registered_models":0"#));
    assert!(body.contains(r#""deployment_mode":"daemon""#));
}

#[test]
fn test_models_endpoint_contract() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let (status, headers, body) = dispatch_route("GET", "/api/v1/models", &state);
    assert_eq!(status, 200);
    assert_eq!(headers[0], ("content-type", "application/json"));
    assert!(body.contains("fast-chat-v1"));
    assert!(body.contains("deep-reasoner-v1"));
    assert!(body.contains("balanced-assistant-v1"));
}

#[test]
fn test_providers_endpoint_contract() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let (status, headers, body) = dispatch_route("GET", "/api/v1/providers", &state);
    assert_eq!(status, 200);
    assert_eq!(headers[0], ("content-type", "application/json"));
    assert!(body.contains("groq-api"));
    assert!(body.contains("openrouter-api"));
}

#[test]
fn test_unknown_routes_return_canonical_404_envelope() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let (status, headers, body) = dispatch_route("GET", "/api/v1/unknown_endpoint", &state);

    // 1. Status 404
    assert_eq!(status, 404);
    assert_eq!(headers[0], ("content-type", "application/json"));

    // 2. Canonical JSON error envelope per API_SPEC.md §1.3
    assert!(body.contains(r#""code":"NOT_FOUND""#));
    assert!(body.contains(r#""retryable":false"#));
    assert!(body.contains(r#""request_id":""#));
}

#[test]
fn test_routing_preview_logic() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // 1. Auto routing for code
    let req = RoutingPreviewRequest {
        prompt: "Write a Rust function to parse an ELF binary header.".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let decision = state.router.preview(&req).unwrap();
    assert_eq!(decision.detected_task, "coding_and_debugging");
    assert_eq!(decision.selected_model_id, "deep-reasoner-v1");
    assert_eq!(decision.provider_id, "openrouter-api");
    assert!(
        decision
            .fallback_model_ids
            .contains(&"fast-chat-v1".to_string())
    );

    // 2. Manual routing with valid model
    let req_manual = RoutingPreviewRequest {
        prompt: "Explain Tokio tasks".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("fast-chat-v1".to_string()),
    };
    let decision_manual = state.router.preview(&req_manual).unwrap();
    assert_eq!(decision_manual.selected_model_id, "fast-chat-v1");
    assert_eq!(decision_manual.provider_id, "groq-api");

    // 3. Manual routing with nonexistent model
    let req_invalid = RoutingPreviewRequest {
        prompt: "Test".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("nonexistent-model-999".to_string()),
    };
    let err = state.router.preview(&req_invalid).unwrap_err();
    assert_eq!(err.status_code(), 404);
    assert_eq!(err.error_code(), "NOT_FOUND");
}

#[test]
fn test_valid_configuration_accepted() {
    let config = AppConfig {
        host: "0.0.0.0".to_string(),
        port: 9000,
        environment: Environment::Production,
        deployment_mode: DeploymentMode::Serverless,
        max_body_size_bytes: 4_500_000,
        upstream_timeout_secs: 10,
        api_key: None,
        groq_api_key: None,
        groq_model: None,
        openrouter_api_key: None,
        gemini_api_key: None,
    };
    assert!(config.validate().is_ok());
}

#[test]
fn test_invalid_configuration_rejected() {
    // Port 0 is rejected
    let mut config = AppConfig::default_local();
    config.port = 0;
    assert!(matches!(
        config.validate(),
        Err(ConfigError::InvalidValue { key: "PORT", .. })
    ));

    // Invalid IP is rejected
    config.port = 8080;
    config.host = "invalid.ip.address".to_string();
    assert!(matches!(
        config.validate(),
        Err(ConfigError::InvalidValue { key: "HOST", .. })
    ));

    // Excessive body limit (> 10MB) is rejected
    config.host = "127.0.0.1".to_string();
    config.max_body_size_bytes = 20_000_000;
    assert!(matches!(
        config.validate(),
        Err(ConfigError::InvalidValue {
            key: "MAX_BODY_SIZE_BYTES",
            ..
        })
    ));

    // Excessive upstream timeout (> 30s) is rejected
    config.max_body_size_bytes = 4_500_000;
    config.upstream_timeout_secs = 60;
    assert!(matches!(
        config.validate(),
        Err(ConfigError::InvalidValue {
            key: "UPSTREAM_TIMEOUT_SECS",
            ..
        })
    ));
}

#[test]
fn test_error_responses_do_not_leak_internal_details() {
    let internal_err = AppError::InternalError(
        "Failed to connect to postgres://admin:secret_pass@10.0.0.5/prod".to_string(),
    );
    let client_msg = internal_err.client_message();
    assert_eq!(client_msg, "An internal server error occurred.");
    assert!(!client_msg.contains("postgres"));
    assert!(!client_msg.contains("secret_pass"));
    assert!(!client_msg.contains("10.0.0.5"));

    let envelope = internal_err.to_json_envelope("req_test_leak");
    assert!(!envelope.contains("secret_pass"));
    assert!(!envelope.contains("postgres"));
    assert!(envelope.contains(r#""code":"INTERNAL_ERROR""#));
}
