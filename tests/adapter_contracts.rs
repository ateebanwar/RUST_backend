//! Integration tests for the Provider Adapter Framework.
//!
//! Conforms to TESTING.md §2.3, PROVIDER_INTEGRATION.md §2, and ARCHITECTURE.md §2.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::errors::AppError;
use ai_backend::models::{ChatMessage, CompletionRequest, ModelMetadata};
use ai_backend::providers::{
    MockProviderAdapter, ProviderAdapter, ProviderError, ProviderRegistry,
};
use ai_backend::registry::ModelRegistry;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_adapter_contract_success_and_metadata() {
    let adapter = MockProviderAdapter::new("test-groq", "Test Groq Provider");
    let info = adapter.provider_info();

    assert_eq!(info.provider_id, "test-groq");
    assert_eq!(info.display_name, "Test Groq Provider");
    assert!(info.enabled);
    assert_eq!(info.status, "operational");

    // Standard completion
    let req = CompletionRequest::new(
        "test-groq-model-1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "What is 2+2?".to_string(),
        }],
    );

    let res = adapter
        .complete(&req)
        .await
        .expect("Completion should succeed");
    assert_eq!(res.model_used, "test-groq-model-1");
    assert_eq!(res.provider_used, "test-groq");
    assert!(!res.is_fallback);
    assert_eq!(res.message.role, "assistant");
    assert!(res.message.content.contains("Mock completion response"));

    let usage = res.usage.expect("Usage metadata must be present");
    assert!(usage.total_tokens > 0);
    assert!(!usage.is_estimated);
}

#[tokio::test]
async fn test_adapter_contract_streaming_chunks() {
    let adapter = MockProviderAdapter::new("test-streaming", "Streaming Provider")
        .with_stream_chunks(vec![
            "Hello".to_string(),
            " world".to_string(),
            "!".to_string(),
        ]);

    let req = CompletionRequest::new("test-streaming-model-1", vec![]);
    let mut stream = adapter.stream(&req).await.expect("Stream should start");

    let mut collected_text = String::new();
    let mut saw_done = false;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.expect("Chunk must be valid");
        if chunk.is_done {
            saw_done = true;
            assert!(chunk.usage.is_some());
        } else {
            collected_text.push_str(&chunk.delta);
        }
    }

    assert!(saw_done, "Stream must culminate in a done chunk");
    assert_eq!(collected_text, "Hello world!");
}

#[tokio::test]
async fn test_adapter_contract_capability_reporting_and_enforcement() {
    // Adapter with only "simple_chat" capability (no "streaming" or "image_gen")
    let adapter = MockProviderAdapter::new("restricted-p", "Restricted Provider")
        .with_capabilities(vec!["simple_chat".to_string()]);

    assert!(adapter.supports_capability("simple_chat"));
    assert!(!adapter.supports_capability("streaming"));
    assert!(!adapter.supports_capability("image_generation"));

    let req = CompletionRequest::new("restricted-p-model-1", vec![]);

    // Complete succeeds because simple_chat is supported
    assert!(adapter.complete(&req).await.is_ok());

    // Stream fails with CapabilityMismatch because streaming is not supported
    match adapter.stream(&req).await {
        Err(ProviderError::CapabilityMismatch(cap)) => {
            assert_eq!(cap, "streaming");
        }
        _ => panic!("Expected CapabilityMismatch for unsupported streaming"),
    }
}

#[tokio::test]
async fn test_adapter_contract_timeout_enforcement() {
    // Simulate 50ms delay with a 10ms deadline
    let adapter =
        MockProviderAdapter::new("slow-p", "Slow Provider").with_delay(Duration::from_millis(50));

    let mut req = CompletionRequest::new("slow-p-model-1", vec![]);
    req.timeout = Duration::from_millis(10);

    match adapter.complete(&req).await {
        Err(ProviderError::Timeout(d)) => {
            assert_eq!(d, Duration::from_millis(10));
        }
        _ => panic!("Expected Timeout error when request deadline is exceeded"),
    }
}

#[tokio::test]
async fn test_adapter_contract_model_not_found() {
    let adapter = MockProviderAdapter::new("catalog-p", "Catalog Provider");
    let req = CompletionRequest::new("nonexistent-model-xyz", vec![]);

    match adapter.complete(&req).await {
        Err(ProviderError::ModelNotFound(m)) => {
            assert_eq!(m, "nonexistent-model-xyz");
        }
        _ => panic!("Expected ModelNotFound error"),
    }
}

#[tokio::test]
async fn test_provider_registry_registration_and_lookup() {
    let registry = ProviderRegistry::new();
    assert_eq!(registry.active_providers_count(), 0);

    // 1. Successful registration
    let adapter = Arc::new(MockProviderAdapter::new("prov-1", "Provider One"));
    assert!(registry.register(adapter.clone()).is_ok());
    assert!(registry.has_provider("prov-1"));
    assert_eq!(registry.active_providers_count(), 1);

    // 2. Successful retrieval
    let retrieved = registry
        .get("prov-1")
        .expect("Must find registered provider");
    assert_eq!(retrieved.provider_info().provider_id, "prov-1");

    // 3. Duplicate registration cleanly updates without error
    let updated_adapter = Arc::new(MockProviderAdapter::new("prov-1", "Provider One Updated"));
    assert!(registry.register(updated_adapter).is_ok());
    let retrieved_updated = registry.get("prov-1").unwrap();
    assert_eq!(
        retrieved_updated.provider_info().display_name,
        "Provider One Updated"
    );

    // 4. Invalid empty ID registration rejected
    let empty_adapter = Arc::new(MockProviderAdapter::new("   ", "Empty ID Provider"));
    match registry.register(empty_adapter) {
        Err(ProviderError::InvalidRequest(_)) => {}
        _ => panic!("Expected InvalidRequest for empty provider ID"),
    }

    // 5. Unknown provider lookup returns ProviderNotFound
    match registry.get("nonexistent-provider") {
        Err(ProviderError::ProviderNotFound(p)) => {
            assert_eq!(p, "nonexistent-provider");
        }
        _ => panic!("Expected ProviderNotFound error"),
    }

    // 6. Disabled provider lookup returns ProviderDisabled
    let disabled = Arc::new(MockProviderAdapter::new("disabled-p", "Disabled").with_enabled(false));
    registry.register(disabled).unwrap();
    assert!(registry.has_provider("disabled-p"));
    assert_eq!(registry.active_providers_count(), 1); // Disabled provider doesn't increment active count
    match registry.get("disabled-p") {
        Err(ProviderError::ProviderDisabled(p)) => {
            assert_eq!(p, "disabled-p");
        }
        _ => panic!("Expected ProviderDisabled error"),
    }

    // 7. Deregistration
    assert!(registry.deregister("prov-1").is_some());
    assert!(!registry.has_provider("prov-1"));
    assert_eq!(registry.active_providers_count(), 0);
}

#[tokio::test]
async fn test_provider_registry_health_and_lifecycle_state() {
    let registry = ProviderRegistry::new();
    let healthy_adapter = Arc::new(MockProviderAdapter::new("p-healthy", "Healthy"));
    let failing_adapter =
        Arc::new(MockProviderAdapter::new("p-failing", "Failing").with_health(false));

    registry.register(healthy_adapter).unwrap();
    registry.register(failing_adapter).unwrap();

    // Check individual health
    assert!(registry.check_provider_health("p-healthy").await.unwrap());
    assert!(!registry.check_provider_health("p-failing").await.unwrap());

    // Batch check all health
    let all_res = registry.check_all_health().await;
    assert_eq!(all_res.len(), 2);
    assert_eq!(*all_res.get("p-healthy").unwrap(), Ok(true));
    assert_eq!(*all_res.get("p-failing").unwrap(), Ok(false));

    // Dynamic metadata reflects status
    let metadata_list = registry.list_metadata();
    let healthy_meta = metadata_list
        .iter()
        .find(|m| m.provider_id == "p-healthy")
        .unwrap();
    assert_eq!(healthy_meta.status, "operational");

    let failing_meta = metadata_list
        .iter()
        .find(|m| m.provider_id == "p-failing")
        .unwrap();
    assert_eq!(failing_meta.status, "offline");
}

#[tokio::test]
async fn test_provider_registry_model_discovery_sync() {
    let provider_reg = ProviderRegistry::new();
    let model_reg = ModelRegistry::new();

    let custom_model = ModelMetadata {
        model_id: "custom-vision-v1".to_string(),
        display_name: "Custom Vision Model".to_string(),
        provider_id: "vision-prov".to_string(),
        supported_modalities: vec!["image".to_string()],
        context_window: 16384,
        max_output_tokens: 4096,
        streaming_supported: false,
        tool_calling_supported: false,
        speed_tier: "balanced".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["image_analysis".to_string()],
        last_health_check: "2026-10-19T18:00:00Z".to_string(),
    };

    let adapter = Arc::new(
        MockProviderAdapter::new("vision-prov", "Vision Provider").with_models(vec![custom_model]),
    );

    provider_reg.register(adapter).unwrap();

    let synced_count = provider_reg
        .sync_models_to_registry(&model_reg)
        .await
        .expect("Model sync should succeed");

    assert_eq!(synced_count, 1);

    let retrieved = model_reg
        .get_model("custom-vision-v1")
        .expect("Model must be present in ModelRegistry after sync");
    assert_eq!(retrieved.display_name, "Custom Vision Model");
    assert_eq!(retrieved.provider_id, "vision-prov");
}

#[tokio::test]
async fn test_provider_registry_concurrency() {
    let registry = Arc::new(ProviderRegistry::new());

    // Register 10 adapters concurrently
    let mut handles = Vec::new();
    for i in 0..10 {
        let reg = registry.clone();
        handles.push(tokio::spawn(async move {
            let id = format!("concurrent-p-{i}");
            let adapter = Arc::new(MockProviderAdapter::new(&id, &format!("Provider {i}")));
            reg.register(adapter).unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(registry.active_providers_count(), 10);
    assert_eq!(registry.list_adapters().len(), 10);
}

#[test]
fn test_error_mapping_secret_isolation() {
    let sensitive_key = "sk_live_very_secret_api_key_99999";
    let err = ProviderError::AuthenticationFailed(format!("Auth failed with key {sensitive_key}"));
    let app_err: AppError = err.into();

    // Verify HTTP status 502
    assert_eq!(app_err.status_code(), 502);

    // Verify secret is NOT leaked in client message or envelope
    let client_msg = app_err.client_message();
    assert!(!client_msg.contains(sensitive_key));

    let envelope_json = app_err.to_json_envelope("req_test_sec");
    assert!(!envelope_json.contains(sensitive_key));
    assert!(envelope_json.contains(r#""code":"UPSTREAM_FAILURE""#));
}

#[test]
fn test_app_state_integration_with_provider_registry() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config.clone());

    // AppState initializes with empty provider registry
    assert_eq!(state.providers.active_providers_count(), 0);

    // Dynamically register an adapter into AppState
    let adapter = Arc::new(MockProviderAdapter::new("state-p", "State Provider"));
    state.providers.register(adapter).unwrap();
    assert_eq!(state.providers.active_providers_count(), 1);
    assert!(state.providers.has_provider("state-p"));
}
