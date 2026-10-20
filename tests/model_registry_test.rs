//! Integration tests for Phase 5 — Central Model Registry.
//!
//! Validates:
//! 1. Central model catalog ownership & metadata validation (MASTER_SPEC.md §18 Phase 5).
//! 2. Model lookup and canonical error mapping (API_SPEC.md §3.2).
//! 3. Query filtering by modality, speed tier, provider, and availability (API_SPEC.md §3.1).
//! 4. Availability state tracking & RFC3339 timestamps (MASTER_SPEC.md §18.5).
//! 5. Provider model discovery and synchronization (PROVIDER_INTEGRATION.md §2).
//! 6. Readiness contract and operational model resolution (API_SPEC.md §2.2).

use ai_backend::config::AppConfig;
use ai_backend::models::ModelMetadata;
use ai_backend::providers::MockProviderAdapter;
use ai_backend::registry::{ModelFilter, ModelRegistry, RegistryError};
use ai_backend::{AppState, dispatch_route};
use std::sync::Arc;

fn create_valid_test_model(id: &str, provider: &str) -> ModelMetadata {
    ModelMetadata {
        model_id: id.to_string(),
        display_name: format!("Display for {id}"),
        provider_id: provider.to_string(),
        supported_modalities: vec!["text".to_string(), "code".to_string()],
        context_window: 128_000,
        max_output_tokens: 4_096,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["chat".to_string(), "streaming".to_string()],
        last_health_check: "2026-10-19T00:00:00Z".to_string(),
    }
}

// ============================================================================
// 1. Model Metadata Validation Rules
// ============================================================================

#[test]
fn test_model_validation_empty_id_rejected() {
    let model = create_valid_test_model("", "test-provider");
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

#[test]
fn test_model_validation_invalid_id_characters_rejected() {
    let mut model = create_valid_test_model("model@with$symbols", "test-provider");
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));

    model.model_id = "model with spaces".to_string();
    assert!(ModelRegistry::validate_model(&model).is_err());
}

#[test]
fn test_model_validation_empty_display_name_rejected() {
    let mut model = create_valid_test_model("valid-id", "test-provider");
    model.display_name = "".to_string();
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

#[test]
fn test_model_validation_empty_modalities_rejected() {
    let mut model = create_valid_test_model("valid-id", "test-provider");
    model.supported_modalities = vec![];
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

#[test]
fn test_model_validation_zero_context_window_rejected() {
    let mut model = create_valid_test_model("valid-id", "test-provider");
    model.context_window = 0;
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

#[test]
fn test_model_validation_output_exceeds_context_rejected() {
    let mut model = create_valid_test_model("valid-id", "test-provider");
    model.context_window = 4_096;
    model.max_output_tokens = 8_192;
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

#[test]
fn test_model_validation_invalid_speed_tier_rejected() {
    let mut model = create_valid_test_model("valid-id", "test-provider");
    model.speed_tier = "ultra-ludicrous".to_string();
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

#[test]
fn test_model_validation_invalid_cost_tier_rejected() {
    let mut model = create_valid_test_model("valid-id", "test-provider");
    model.cost_tier = "super-expensive".to_string();
    let err = ModelRegistry::validate_model(&model).unwrap_err();
    assert!(matches!(err, RegistryError::InvalidModel { .. }));
}

// ============================================================================
// 2. Registration and Duplicate Handling
// ============================================================================

#[test]
fn test_registration_and_duplicate_prevention() {
    let registry = ModelRegistry::new();
    let model = create_valid_test_model("new-unique-model", "test-provider");

    // First registration via register_new_model succeeds
    assert!(registry.register_new_model(model.clone()).is_ok());
    assert!(registry.has_model("new-unique-model"));

    // Second registration via register_new_model fails with DuplicateModel
    let dup_err = registry.register_new_model(model.clone()).unwrap_err();
    assert!(matches!(dup_err, RegistryError::DuplicateModel(_)));

    // Direct register_model overwrites/updates safely
    let mut updated_model = model;
    updated_model.display_name = "Updated Display Name".to_string();
    assert!(registry.register_model(updated_model).is_ok());

    let fetched = registry.get_model("new-unique-model").unwrap();
    assert_eq!(fetched.display_name, "Updated Display Name");
}

// ============================================================================
// 3. Query Filtering
// ============================================================================

#[test]
fn test_comprehensive_query_filtering() {
    let registry = ModelRegistry::new();
    registry.clear();

    let fast_text = ModelMetadata {
        model_id: "filter-fast-text".to_string(),
        display_name: "Fast Text".to_string(),
        provider_id: "prov-a".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 32_000,
        max_output_tokens: 4_096,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["chat".to_string()],
        last_health_check: "2026-10-19T00:00:00Z".to_string(),
    };

    let adv_code = ModelMetadata {
        model_id: "filter-adv-code".to_string(),
        display_name: "Adv Code".to_string(),
        provider_id: "prov-b".to_string(),
        supported_modalities: vec!["code".to_string()],
        context_window: 128_000,
        max_output_tokens: 8_192,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "advanced".to_string(),
        cost_tier: "premium".to_string(),
        available: false,
        capabilities: vec!["chat".to_string(), "code".to_string()],
        last_health_check: "2026-10-19T00:00:00Z".to_string(),
    };

    registry.register_model(fast_text).unwrap();
    registry.register_model(adv_code).unwrap();

    // 1. Modality filter: code
    let code_filter = ModelFilter::new().with_modality("code");
    let code_results = registry.query_models(&code_filter);
    assert!(code_results.iter().any(|m| m.model_id == "filter-adv-code"));
    assert!(
        !code_results
            .iter()
            .any(|m| m.model_id == "filter-fast-text")
    );

    // 2. Speed tier filter: advanced
    let adv_filter = ModelFilter::new().with_speed_tier("advanced");
    let adv_results = registry.query_models(&adv_filter);
    assert_eq!(adv_results.len(), 1);
    assert_eq!(adv_results[0].model_id, "filter-adv-code");

    // 3. Provider filter: prov-a
    let prov_filter = ModelFilter::new().with_provider("prov-a");
    let prov_results = registry.query_models(&prov_filter);
    assert_eq!(prov_results.len(), 1);
    assert_eq!(prov_results[0].model_id, "filter-fast-text");

    // 4. Available only: filter-adv-code is unavailable
    let avail_filter = ModelFilter::new().with_available_only(true);
    let avail_results = registry.query_models(&avail_filter);
    assert!(
        !avail_results
            .iter()
            .any(|m| m.model_id == "filter-adv-code")
    );
    assert!(
        avail_results
            .iter()
            .any(|m| m.model_id == "filter-fast-text")
    );

    // 5. Combined filter: code + operational -> empty
    let combined_filter = ModelFilter::new()
        .with_modality("code")
        .with_available_only(true);
    let combined_results = registry.query_models(&combined_filter);
    assert!(
        combined_results
            .iter()
            .all(|m| m.model_id != "filter-adv-code")
    );
}

// ============================================================================
// 4. Availability State Transitions
// ============================================================================

#[test]
fn test_availability_state_updates_and_timestamps() {
    let registry = ModelRegistry::new();
    let model = create_valid_test_model("avail-test-model", "prov-avail");
    registry.register_model(model).unwrap();

    // Initial state is operational (available: true)
    assert!(registry.get_model("avail-test-model").unwrap().available);

    // Update single model to unavailable
    assert!(
        registry
            .set_model_availability("avail-test-model", false)
            .is_ok()
    );
    let updated = registry.get_model("avail-test-model").unwrap();
    assert!(!updated.available);
    let ts = &updated.last_health_check;
    assert!(ts.contains("T") && ts.contains("Z"));

    // Bulk update for provider models to available
    let count = registry.set_provider_models_availability("prov-avail", true);
    assert_eq!(count, 1);
    assert!(registry.get_model("avail-test-model").unwrap().available);

    // Updating non-existent model returns ModelNotFound
    let err = registry
        .set_model_availability("ghost-model", true)
        .unwrap_err();
    assert!(matches!(err, RegistryError::ModelNotFound(_)));
}

// ============================================================================
// 5. HTTP Endpoints & Canonical JSON Contract
// ============================================================================

#[test]
fn test_http_model_lookup_success_and_not_found() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // Valid lookup
    let (status, headers, body) = dispatch_route("GET", "/api/v1/models/fast-chat-v1", &state);
    assert_eq!(status, 200);
    assert_eq!(headers[0], ("content-type", "application/json"));
    assert!(body.contains(r#""model_id":"fast-chat-v1""#));
    assert!(body.contains(r#""provider_id":"groq-api""#));

    // Non-existent lookup
    let (status_404, _, body_404) =
        dispatch_route("GET", "/api/v1/models/non-existent-model-xyz", &state);
    assert_eq!(status_404, 404);
    assert!(body_404.contains(r#""code":"NOT_FOUND""#));
    assert!(body_404.contains(r#""retryable":false"#));
    assert!(body_404.contains("non-existent-model-xyz"));
}

#[test]
fn test_http_models_query_parameters() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // 1. Query by provider
    let (status, _, body) = dispatch_route("GET", "/api/v1/models?provider=groq-api", &state);
    assert_eq!(status, 200);
    assert!(body.contains("fast-chat-v1"));
    assert!(!body.contains("deep-reasoner-v1"));

    // 2. Query by speed_tier
    let (status, _, body) = dispatch_route("GET", "/api/v1/models?speed_tier=advanced", &state);
    assert_eq!(status, 200);
    assert!(body.contains("deep-reasoner-v1"));
    assert!(!body.contains("fast-chat-v1"));

    // 3. Query by modality
    let (status, _, body) = dispatch_route("GET", "/api/v1/models?modality=code", &state);
    assert_eq!(status, 200);
    assert!(body.contains("deep-reasoner-v1"));

    // 4. Query with no matches
    let (status, _, body) =
        dispatch_route("GET", "/api/v1/models?provider=non-existent-prov", &state);
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"models":[]}"#);
}

// ============================================================================
// 6. Readiness Discrepancy Resolution & Provider Discovery
// ============================================================================

#[tokio::test]
async fn test_readiness_resolution_with_active_provider_and_discovery() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // 1. In initial production state, no live providers are registered in ProviderRegistry.
    // Readiness accurately reports 503 Service Unavailable, status: "not_ready", active_providers: 0, registered_models: 0.
    let (status, _, body) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status, 503);
    assert!(body.contains(r#""status":"not_ready""#));
    assert!(body.contains(r#""active_providers":0"#));
    assert!(body.contains(r#""registered_models":0"#));

    // Reference models remain queryable in catalog for client UI/routing
    let (status, _, models_body) = dispatch_route("GET", "/api/v1/models", &state);
    assert_eq!(status, 200);
    assert!(models_body.contains("fast-chat-v1"));

    // 2. Now register an active mock adapter into the provider registry
    let mock_adapter = Arc::new(MockProviderAdapter::new(
        "live-provider",
        "Live Operational Provider",
    ));
    state
        .providers
        .register(mock_adapter.clone())
        .expect("Registration must succeed");

    // 3. Sync discovered models from provider to model registry
    state
        .providers
        .sync_models_to_registry(&state.registry)
        .await
        .expect("Sync must succeed");

    // 4. Verify readiness now dynamically reflects the active operational models and reports 200 OK!
    let (status, _, body_ready) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status, 200);
    assert!(body_ready.contains(r#""status":"ready""#));
    assert!(body_ready.contains(r#""active_providers":1"#));
    // live-provider has 1 discovered operational model
    assert!(body_ready.contains(r#""registered_models":1"#));

    // 5. Providers endpoint returns live provider metadata
    let (status, _, body_providers) = dispatch_route("GET", "/api/v1/providers", &state);
    assert_eq!(status, 200);
    assert!(body_providers.contains("live-provider"));
    assert!(body_providers.contains("Live Operational Provider"));
}
