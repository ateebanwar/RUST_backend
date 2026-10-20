//! Comprehensive regression tests for the Readiness Contract.
//!
//! Validates:
//! 1. Application startup with zero upstream provider adapters (returns 503, status: "not_ready").
//! 2. Reference catalog models/providers do not inflate operational provider/model counts.
//! 3. Readiness status and counters accurately represent the no-provider state.
//! 4. Registered adapter that is not operational/healthy (offline or disabled) does not report ready.
//! 5. Operational provider registration and model discovery/synchronization transition readiness to 200 OK.
//! 6. Provider becoming unavailable after being operational transitions readiness back to 503.
//! 7. Multiple providers exist where only a subset are operational (only operational providers and their models count).
//! 8. Model counts remain accurate when providers have different numbers of discoverable models.
//! 9. Provider discovery or synchronization failure does not corrupt previously valid registry state.
//! 10. Liveness behavior (GET /api/v1/health) remains 200 OK and strictly decoupled from provider readiness.

use ai_backend::config::AppConfig;
use ai_backend::models::ModelMetadata;
use ai_backend::providers::{MockProviderAdapter, ProviderError};
use ai_backend::{AppState, dispatch_route};
use std::sync::Arc;

// ============================================================================
// Scenario 1, 2, 3: Clean startup, reference catalog isolation, and no-provider state
// ============================================================================

#[test]
fn test_scenario_1_2_3_clean_startup_reference_isolation_and_no_provider_state() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // 1. Clean startup: zero upstream provider adapters are registered
    assert_eq!(state.providers.active_providers_count(), 0);
    assert_eq!(state.providers.list_adapters().len(), 0);

    // 2. Reference catalog contains models and providers for UI/routing preview
    let catalog_models = state.registry.list_models(None, None, None);
    assert_eq!(catalog_models.len(), 3);
    let reference_providers = state.registry.list_providers();
    assert_eq!(reference_providers.len(), 2);

    // 3. Readiness probe MUST return 503 Service Unavailable with status: "not_ready"
    // Reference models/providers MUST NOT inflate active_providers or registered_models
    let (status, headers, body) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status, 503);
    assert_eq!(headers[0], ("content-type", "application/json"));
    assert!(body.contains(r#""status":"not_ready""#));
    assert!(body.contains(r#""active_providers":0"#));
    assert!(body.contains(r#""registered_models":0"#));
    assert!(body.contains(r#""deployment_mode":"daemon""#));
}

// ============================================================================
// Scenario 4: Registered adapter that is not operational/healthy
// ============================================================================

#[tokio::test]
async fn test_scenario_4_adapter_registered_but_offline_or_disabled() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // Register a mock adapter
    let adapter = Arc::new(MockProviderAdapter::new("offline-prov", "Offline Provider"));
    state.providers.register(adapter.clone()).unwrap();

    // Mark provider as offline
    state
        .providers
        .set_status("offline-prov", "offline")
        .unwrap();

    // Sync models
    state
        .providers
        .sync_models_to_registry(&state.registry)
        .await
        .unwrap();

    // Readiness must remain 503 not_ready because the provider is offline
    let (status, _, body) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status, 503);
    assert!(body.contains(r#""status":"not_ready""#));
    assert!(body.contains(r#""active_providers":0"#));
    assert!(body.contains(r#""registered_models":0"#));

    // Now test a disabled adapter
    let disabled_adapter = Arc::new(
        MockProviderAdapter::new("disabled-prov", "Disabled Provider").with_enabled(false),
    );
    state.providers.register(disabled_adapter).unwrap();

    let (status_dis, _, body_dis) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status_dis, 503);
    assert!(body_dis.contains(r#""status":"not_ready""#));
    assert!(body_dis.contains(r#""active_providers":0"#));
    assert!(body_dis.contains(r#""registered_models":0"#));
}

// ============================================================================
// Scenario 5: Operational provider registration and synchronization
// ============================================================================

#[tokio::test]
async fn test_scenario_5_operational_provider_registration_and_sync() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let adapter = Arc::new(MockProviderAdapter::new("op-prov", "Operational Provider"));
    state.providers.register(adapter).unwrap();
    state
        .providers
        .sync_models_to_registry(&state.registry)
        .await
        .unwrap();

    let (status, _, body) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status, 200);
    assert!(body.contains(r#""status":"ready""#));
    assert!(body.contains(r#""active_providers":1"#));
    assert!(body.contains(r#""registered_models":1"#));
}

// ============================================================================
// Scenario 6: Provider becomes unavailable after being operational
// ============================================================================

#[tokio::test]
async fn test_scenario_6_provider_transition_to_unavailable() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    let adapter = Arc::new(MockProviderAdapter::new(
        "transient-prov",
        "Transient Provider",
    ));
    state.providers.register(adapter).unwrap();
    state
        .providers
        .sync_models_to_registry(&state.registry)
        .await
        .unwrap();

    // Initially ready (200 OK)
    let (status_1, _, body_1) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status_1, 200);
    assert!(body_1.contains(r#""status":"ready""#));
    assert!(body_1.contains(r#""active_providers":1"#));

    // Outage occurs: set provider to offline
    state
        .providers
        .set_status("transient-prov", "offline")
        .unwrap();

    // Now unready (503 Service Unavailable, status: not_ready)
    let (status_2, _, body_2) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status_2, 503);
    assert!(body_2.contains(r#""status":"not_ready""#));
    assert!(body_2.contains(r#""active_providers":0"#));
    assert!(body_2.contains(r#""registered_models":0"#));

    // Recovery occurs: set provider back to operational
    state
        .providers
        .set_status("transient-prov", "operational")
        .unwrap();

    let (status_3, _, body_3) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status_3, 200);
    assert!(body_3.contains(r#""status":"ready""#));
    assert!(body_3.contains(r#""active_providers":1"#));
    assert!(body_3.contains(r#""registered_models":1"#));
}

// ============================================================================
// Scenario 7 & 8: Multiple providers with partial operational states & variable models
// ============================================================================

#[tokio::test]
async fn test_scenario_7_8_multiple_providers_partial_operational_and_variable_models() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // Provider A: operational with 2 models
    let model_a1 = ModelMetadata {
        model_id: "prov-a-m1".to_string(),
        display_name: "A Model 1".to_string(),
        provider_id: "prov-a".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 64_000,
        max_output_tokens: 4_096,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["chat".to_string()],
        last_health_check: "2026-10-19T00:00:00Z".to_string(),
    };
    let model_a2 = ModelMetadata {
        model_id: "prov-a-m2".to_string(),
        display_name: "A Model 2".to_string(),
        provider_id: "prov-a".to_string(),
        supported_modalities: vec!["code".to_string()],
        context_window: 128_000,
        max_output_tokens: 8_192,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "advanced".to_string(),
        cost_tier: "premium".to_string(),
        available: true,
        capabilities: vec!["coding".to_string()],
        last_health_check: "2026-10-19T00:00:00Z".to_string(),
    };
    let adapter_a = Arc::new(
        MockProviderAdapter::new("prov-a", "Provider A").with_models(vec![model_a1, model_a2]),
    );

    // Provider B: offline with 3 models
    let model_b1 = ModelMetadata {
        model_id: "prov-b-m1".to_string(),
        display_name: "B Model 1".to_string(),
        provider_id: "prov-b".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 32_000,
        max_output_tokens: 2_048,
        streaming_supported: false,
        tool_calling_supported: false,
        speed_tier: "balanced".to_string(),
        cost_tier: "low-cost".to_string(),
        available: true,
        capabilities: vec!["chat".to_string()],
        last_health_check: "2026-10-19T00:00:00Z".to_string(),
    };
    let adapter_b =
        Arc::new(MockProviderAdapter::new("prov-b", "Provider B").with_models(vec![model_b1]));

    state.providers.register(adapter_a).unwrap();
    state.providers.register(adapter_b).unwrap();

    // Mark Provider B as offline
    state.providers.set_status("prov-b", "offline").unwrap();

    // Sync all models to registry
    state
        .providers
        .sync_models_to_registry(&state.registry)
        .await
        .unwrap();

    // Readiness must ONLY count Provider A (1 active provider) and its 2 models (2 registered models)
    // Provider B's models MUST NOT be counted because Provider B is offline!
    let (status, _, body) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status, 200);
    assert!(body.contains(r#""status":"ready""#));
    assert!(body.contains(r#""active_providers":1"#));
    assert!(body.contains(r#""registered_models":2"#));

    // When Provider B becomes operational, active_providers becomes 2 and registered_models becomes 3
    state.providers.set_status("prov-b", "operational").unwrap();

    let (status_both, _, body_both) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status_both, 200);
    assert!(body_both.contains(r#""status":"ready""#));
    assert!(body_both.contains(r#""active_providers":2"#));
    assert!(body_both.contains(r#""registered_models":3"#));
}

// ============================================================================
// Scenario 9: Provider discovery failure preserves existing catalog state
// ============================================================================

#[tokio::test]
async fn test_scenario_9_discovery_failure_preserves_valid_catalog_state() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // Register a valid operational provider
    let adapter_good = Arc::new(MockProviderAdapter::new("good-prov", "Good Provider"));
    state.providers.register(adapter_good).unwrap();
    state
        .providers
        .sync_models_to_registry(&state.registry)
        .await
        .unwrap();

    // Initial operational count: 1 active provider, 1 registered model
    assert_eq!(state.providers.active_providers_count(), 1);
    assert!(state.registry.has_model("good-prov-model-1"));

    // Now register a failing provider
    let adapter_failing = Arc::new(
        MockProviderAdapter::new("fail-prov", "Failing Provider").with_simulated_error(
            ProviderError::NetworkFailure("Discovery failed".to_string()),
        ),
    );
    state.providers.register(adapter_failing).unwrap();

    // Synchronize models: good provider models must survive
    let sync_res = state
        .providers
        .sync_models_to_registry(&state.registry)
        .await;
    // sync_models_to_registry propagates the discovery error from the failing provider
    assert!(sync_res.is_err());

    // Previously valid models MUST NOT be corrupted or erased
    assert!(state.registry.has_model("good-prov-model-1"));
    assert!(state.registry.has_model("fast-chat-v1")); // Reference model also intact
}

// ============================================================================
// Scenario 10: Liveness probe remains 200 OK independent of readiness state
// ============================================================================

#[test]
fn test_scenario_10_liveness_strictly_independent_of_readiness() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // In clean startup state (zero providers, readiness is 503 not_ready)
    let (ready_status, _, ready_body) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(ready_status, 503);
    assert!(ready_body.contains(r#""status":"not_ready""#));

    // Liveness probe (GET /api/v1/health) MUST return 200 OK healthy
    let (health_status, _, health_body) = dispatch_route("GET", "/api/v1/health", &state);
    assert_eq!(health_status, 200);
    assert!(health_body.contains(r#""status":"healthy""#));
    assert!(health_body.contains(r#""version":"0.1.0""#));
}
