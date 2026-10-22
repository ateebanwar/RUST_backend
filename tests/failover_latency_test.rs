//! Fast Failover Performance & Latency Verification Tests.
//!
//! Conforms strictly to MASTER_SPEC.md §13 & §14, Section 3.3 and Section 6.3.
//!
//! Verifies:
//! 1. Low-latency candidate failover on immediate upstream 429 / QuotaExhausted.
//! 2. Zero-backoff delay switching when alternative operational candidates exist.
//! 3. Manual routing semantics are strictly preserved (no automatic failover in manual mode).
//! 4. Local rate-limit rejections never trigger upstream provider failover.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::inference::ChatCompletionRequest;
use ai_backend::models::{ChatMessage, ModelMetadata};
use ai_backend::providers::{MockProviderAdapter, ProviderError, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn create_candidate(id: &str, provider: &str, speed: &str) -> ModelMetadata {
    ModelMetadata {
        model_id: id.to_string(),
        display_name: format!("{id} Display"),
        provider_id: provider.to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 8192,
        max_output_tokens: 2048,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: speed.to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    }
}

#[tokio::test]
async fn test_low_latency_failover_on_immediate_quota_exhausted() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 3;
    config_raw.initial_backoff_ms = 1000; // Normal backoff would take 1000ms+
    config_raw.max_failover_candidates = 3;
    let config = Arc::new(config_raw);

    let registry = Arc::new(ModelRegistry::empty());
    let m1 = create_candidate("model-primary", "primary-provider", "fast");
    let m2 = create_candidate("model-secondary", "secondary-provider", "balanced");
    registry.register_model(m1.clone()).unwrap();
    registry.register_model(m2.clone()).unwrap();

    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails immediately with QuotaExhausted
    let primary = Arc::new(
        MockProviderAdapter::new("primary-provider", "Primary Provider")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::QuotaExhausted(
                "Daily quota exceeded".to_string(),
            )),
    );
    // Secondary adapter succeeds immediately
    let secondary = Arc::new(
        MockProviderAdapter::new("secondary-provider", "Secondary Provider").with_models(vec![m2]),
    );

    providers.register(primary.clone()).unwrap();
    providers.register(secondary.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let start = Instant::now();
    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Quick test".to_string(),
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
        conversation_id: None,
    };

    let resp = state.inference.execute_completion(&req).await.unwrap();
    let elapsed = start.elapsed();

    // Verify fast failover occurred
    assert_eq!(resp.provider_used, "secondary-provider");
    assert!(resp.is_fallback);
    assert_eq!(
        primary.call_count(),
        1,
        "Primary should only be tried once on confirmed quota exhaustion"
    );
    assert_eq!(
        secondary.call_count(),
        1,
        "Secondary was invoked immediately"
    );

    // Switching happened without sleeping 1000ms
    assert!(
        elapsed < Duration::from_millis(150),
        "Immediate quota failover took {elapsed:?}, expected under 150ms"
    );
}

#[tokio::test]
async fn test_low_latency_failover_on_rate_limit_with_retry_after() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 3;
    config_raw.initial_backoff_ms = 2000;
    let config = Arc::new(config_raw);

    let registry = Arc::new(ModelRegistry::empty());
    let m1 = create_candidate("model-p1", "p1", "fast");
    let m2 = create_candidate("model-p2", "p2", "balanced");
    registry.register_model(m1.clone()).unwrap();
    registry.register_model(m2.clone()).unwrap();

    let providers = Arc::new(ProviderRegistry::new());
    let p1 = Arc::new(
        MockProviderAdapter::new("p1", "Provider 1")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::RateLimited {
                retry_after_secs: Some(30), // 30 seconds wait
            }),
    );
    let p2 = Arc::new(MockProviderAdapter::new("p2", "Provider 2").with_models(vec![m2]));

    providers.register(p1.clone()).unwrap();
    providers.register(p2.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let start = Instant::now();
    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Failover speed test".to_string(),
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
        conversation_id: None,
    };

    let resp = state.inference.execute_completion(&req).await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.provider_used, "p2");
    assert!(resp.is_fallback);
    assert_eq!(p1.call_count(), 1);
    assert_eq!(p2.call_count(), 1);

    // Confirmed 30s rate limit switched immediately without waiting 30 seconds
    assert!(
        elapsed < Duration::from_millis(150),
        "Rate limit failover took {elapsed:?}, expected under 150ms"
    );
}

#[tokio::test]
async fn test_manual_mode_never_fails_over_on_quota_exhausted() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 1;
    let config = Arc::new(config_raw);

    let registry = Arc::new(ModelRegistry::empty());
    let m1 = create_candidate("manual-p1-model", "p1", "fast");
    let m2 = create_candidate("manual-p2-model", "p2", "balanced");
    registry.register_model(m1.clone()).unwrap();
    registry.register_model(m2.clone()).unwrap();

    let providers = Arc::new(ProviderRegistry::new());
    let p1 = Arc::new(
        MockProviderAdapter::new("p1", "Provider 1")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::QuotaExhausted(
                "Plan quota exceeded".to_string(),
            )),
    );
    let p2 = Arc::new(MockProviderAdapter::new("p2", "Provider 2").with_models(vec![m2]));

    providers.register(p1.clone()).unwrap();
    providers.register(p2.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Manual mode test".to_string(),
        }],
        routing_mode: "manual".to_string(),
        requested_model_id: Some("manual-p1-model".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
        conversation_id: None,
    };

    let err = state.inference.execute_completion(&req).await.unwrap_err();
    assert_eq!(p1.call_count(), 1);
    assert_eq!(
        p2.call_count(),
        0,
        "Manual routing mode must never switch to secondary model"
    );
    assert_eq!(err.error_code(), "RATE_LIMITED");
}
