//! Comprehensive Phase 9 Integration Test Suite: Failover & Reliability.
//!
//! Validates:
//! 1. Bounded retry policy on eligible transient failures (500, 502, 503, 504, 408, 429, timeouts, network failures).
//! 2. Strict non-retryable short-circuiting (400, 401, 403, 404, 422, disabled providers).
//! 3. Bounded retry attempts and overall request deadline enforcement.
//! 4. Retry-After header parsing, bounded delays, and excessive delay avoidance.
//! 5. Automatic routing failover to compatible alternatives with capability and task verification.
//! 6. Selection loop prevention (no reselection of failed candidates).
//! 7. Strict manual routing invariant: never silently substitute a user-selected model.
//! 8. Pre-stream retry and failover vs mid-stream terminal error emission.
//! 9. Streaming interruption without restart or duplicate output.
//! 10. Exactly-once usage accounting and finalization across retries and failover.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::create_app;
use ai_backend::models::ModelMetadata;
use ai_backend::providers::{
    MockProviderAdapter, OpenAiCompatibleAdapter, ProviderError, ProviderRegistry,
};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use ai_backend::usage::UsageQueryFilter;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Helper: Create a controlled two-model registry (1 on groq-api, 1 on openrouter-api)
/// to allow deterministic attempt counting per provider.
fn create_two_model_registry() -> (Arc<ModelRegistry>, ModelMetadata, ModelMetadata) {
    let registry = Arc::new(ModelRegistry::empty());

    let m1 = ModelMetadata {
        model_id: "fast-chat-v1".to_string(),
        display_name: "Llama 3.3 70B Instruct (Groq)".to_string(),
        provider_id: "groq-api".to_string(),
        supported_modalities: vec!["text".to_string(), "code".to_string()],
        context_window: 131072,
        max_output_tokens: 8192,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec![
            "simple_chat".to_string(),
            "fast_reasoning".to_string(),
            "coding".to_string(),
        ],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    let m2 = ModelMetadata {
        model_id: "deep-reasoner-v1".to_string(),
        display_name: "DeepSeek R1 Advanced Reasoning".to_string(),
        provider_id: "openrouter-api".to_string(),
        supported_modalities: vec!["text".to_string(), "code".to_string()],
        context_window: 65536,
        max_output_tokens: 16384,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "advanced".to_string(),
        cost_tier: "low-cost".to_string(),
        available: true,
        capabilities: vec![
            "simple_chat".to_string(),
            "complex_analysis".to_string(),
            "deep_reasoning".to_string(),
            "coding".to_string(),
        ],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    registry.register_model(m1.clone()).unwrap();
    registry.register_model(m2.clone()).unwrap();

    (registry, m1, m2)
}

/// Helper: Spawn an ephemeral TCP server simulating an OpenAI-compatible HTTP upstream.
async fn spawn_mock_openai_upstream(
    handler: impl Fn(&str, &str) -> (u16, Vec<(&'static str, &'static str)>, Vec<u8>)
    + Send
    + Sync
    + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler);

    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = match socket.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };

                let req_str = String::from_utf8_lossy(&buf[..n]);
                let mut lines = req_str.lines();
                let first_line = lines.next().unwrap_or("");
                let mut path = "";
                if let Some(p) = first_line.split_whitespace().nth(1) {
                    path = p;
                }

                let (status, headers, body) = handler(path, &req_str);

                let status_text = match status {
                    200 => "200 OK",
                    401 => "401 Unauthorized",
                    404 => "404 Not Found",
                    429 => "429 Too Many Requests",
                    500 => "500 Internal Server Error",
                    502 => "502 Bad Gateway",
                    503 => "503 Service Unavailable",
                    504 => "504 Gateway Timeout",
                    _ => "400 Bad Request",
                };

                let mut resp_header = format!("HTTP/1.1 {}\r\n", status_text);
                for (k, v) in headers {
                    resp_header.push_str(&format!("{}: {}\r\n", k, v));
                }
                resp_header.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));

                let _ = socket.write_all(resp_header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });

    (format!("http://{}", addr), handle)
}

/// Helper: Spawn the full Axum test server on an ephemeral port.
async fn spawn_test_backend(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state);

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{}", addr), handle)
}

// =========================================================================
// 1. RETRY POLICY & TRANSIENT ERROR RECOVERY
// =========================================================================

#[tokio::test]
async fn test_retry_on_transient_failure_succeeds_on_second_attempt() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2;
    config_raw.initial_backoff_ms = 10;
    config_raw.max_backoff_ms = 50;
    let config = Arc::new(config_raw);

    let (registry, m1, _) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails transiently once with 503, then succeeds
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_transient_failures(
                1,
                ProviderError::UpstreamError {
                    status_code: 503,
                    message: "Service temporarily unavailable".to_string(),
                },
            ),
    );
    providers.register(primary.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [{ "role": "user", "content": "Explain retries" }],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.unwrap();

    // Succeeded on primary adapter via retry, NOT via failover
    assert_eq!(body["provider_used"], "groq-api");
    assert_eq!(body["is_fallback"], false);
    assert_eq!(primary.call_count(), 2);

    // Exactly one usage record recorded as Success
    let summary = state
        .usage
        .get_summary(UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.successful_requests, 1);
}

#[tokio::test]
async fn test_retry_on_transient_timeout_and_network_failure() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2;
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 20;
    let config = Arc::new(config_raw);

    let (registry, m1, _) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails transiently once with NetworkFailure, then succeeds
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_transient_failures(
                1,
                ProviderError::NetworkFailure("TCP reset by peer".to_string()),
            ),
    );
    providers.register(primary.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [{ "role": "user", "content": "Test transient network failure" }],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(primary.call_count(), 2);
}

// =========================================================================
// 2. NON-RETRYABLE ERROR SHORT-CIRCUITING
// =========================================================================

#[tokio::test]
async fn test_no_retry_or_failover_on_authentication_failure() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 3;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails with non-retryable AuthenticationFailed
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::AuthenticationFailed(
                "Invalid API Key provided".to_string(),
            )),
    );
    providers.register(primary.clone()).unwrap();

    // Fallback adapter configured and operational
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback").with_models(vec![m2]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [{ "role": "user", "content": "Hello auth test" }],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    // Must short-circuit immediately with 502 UPSTREAM_FAILURE (upstream auth failure sanitization)
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "UPSTREAM_FAILURE");

    // Primary was attempted exactly once; fallback was NEVER called
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 0);

    // Exactly one usage record recorded as ProviderError
    let summary = state
        .usage
        .get_summary(UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.successful_requests, 0);
}

#[tokio::test]
async fn test_no_retry_or_failover_on_invalid_request() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 3;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary fails with non-retryable 400 InvalidRequest
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::InvalidRequest(
                "Context length exceeded".to_string(),
            )),
    );
    providers.register(primary.clone()).unwrap();

    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback").with_models(vec![m2]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [{ "role": "user", "content": "Trigger invalid request" }],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 0);
}

// =========================================================================
// 3. RETRY LIMITS & DEADLINE ENFORCEMENT
// =========================================================================

#[tokio::test]
async fn test_retry_limits_exhaustion_on_single_candidate() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2; // 1 initial + 2 retries = 3 attempts total
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 15;
    let config = Arc::new(config_raw);

    let (registry, m1, _) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Single provider with 1 model persistently returns 500
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 500,
                message: "Internal server error".to_string(),
            }),
    );
    providers.register(primary.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [{ "role": "user", "content": "Fail repeatedly" }],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    // Verified: exactly 1 initial + 2 retries = 3 attempts on single candidate
    assert_eq!(primary.call_count(), 3);
}

#[tokio::test]
async fn test_overall_request_deadline_enforces_bounded_execution() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 5; // would try 6 times if not bounded by deadline
    config_raw.initial_backoff_ms = 10;
    config_raw.max_backoff_ms = 50;
    config_raw.overall_request_timeout_secs = 1; // 1 second total budget
    config_raw.upstream_timeout_secs = 1;
    let config = Arc::new(config_raw);

    let (upstream_url, _handle) = spawn_mock_openai_upstream(|_, _| {
        // Delay 600ms per attempt
        std::thread::sleep(Duration::from_millis(600));
        (503, vec![], b"Service Unavailable".to_vec())
    })
    .await;

    let (registry, m1, _) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    let adapter = Arc::new(
        OpenAiCompatibleAdapter::new(
            "groq-api",
            "Groq Upstream",
            &upstream_url,
            Some("key".to_string()),
        )
        .with_models(vec![m1]),
    );
    providers.register(adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let start = std::time::Instant::now();
    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Deadline test" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    let elapsed = start.elapsed();
    // Must return within ~2s rather than taking 6 * 600ms = 3.6s
    assert!(
        elapsed < Duration::from_millis(2500),
        "Request took too long: {:?}",
        elapsed
    );
    assert!(
        resp.status() == reqwest::StatusCode::GATEWAY_TIMEOUT
            || resp.status() == reqwest::StatusCode::BAD_GATEWAY
    );
}

// =========================================================================
// 4. RETRY-AFTER HANDLING & BOUNDS
// =========================================================================

#[tokio::test]
async fn test_retry_after_within_bound_is_respected_and_retried() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2;
    config_raw.max_retry_after_secs = 5;
    let config = Arc::new(config_raw);

    let (registry, m1, _) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary returns RateLimited with 0s Retry-After on first attempt, then succeeds
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_transient_failures(
                1,
                ProviderError::RateLimited {
                    retry_after_secs: Some(0),
                },
            ),
    );
    providers.register(primary.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Rate limit test" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(primary.call_count(), 2);
}

#[tokio::test]
async fn test_retry_after_exceeding_bound_aborts_candidate_immediately() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2;
    config_raw.max_retry_after_secs = 2; // Bound is 2 seconds
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary returns 429 with 60s Retry-After (> 2s max bound)
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::RateLimited {
                retry_after_secs: Some(60),
            }),
    );
    providers.register(primary.clone()).unwrap();

    // Fallback adapter available
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback").with_models(vec![m2]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let start = std::time::Instant::now();
    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Excessive retry-after" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    let elapsed = start.elapsed();
    // Must complete in under 2 seconds by immediately failing over to fallback instead of sleeping 60 seconds
    assert!(
        elapsed < Duration::from_secs(2),
        "Failed over too slowly: {:?}",
        elapsed
    );
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["provider_used"], "openrouter-api");
    assert_eq!(body["is_fallback"], true);

    // Primary attempted only 1 time (didn't retry because Retry-After exceeded max delay)
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 1);
}

// =========================================================================
// 5. AUTOMATIC FAILOVER & COMPATIBILITY
// =========================================================================

#[tokio::test]
async fn test_auto_failover_selects_compatible_alternative_after_exhaustion() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 1; // 1 initial + 1 retry = 2 attempts on primary
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 15;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails persistently with 502
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 502,
                message: "Bad Gateway".to_string(),
            }),
    );
    providers.register(primary.clone()).unwrap();

    // Fallback adapter operational
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback").with_models(vec![m2]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Automatic failover test" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(body["provider_used"], "openrouter-api");
    assert_eq!(body["is_fallback"], true);
    assert_eq!(primary.call_count(), 2);
    assert_eq!(fallback.call_count(), 1);
}

#[tokio::test]
async fn test_failover_loop_prevention_excludes_failed_candidates() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 1;
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 15;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Both primary and secondary fail with 503
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 503,
                message: "Out of capacity".to_string(),
            }),
    );
    providers.register(primary.clone()).unwrap();

    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback")
            .with_models(vec![m2])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 503,
                message: "Out of capacity".to_string(),
            }),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Loop prevention test" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    // When all candidates fail, returns BAD_GATEWAY
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);

    // Verified: primary was attempted 2 times (1 init + 1 retry) and NEVER re-selected
    assert_eq!(primary.call_count(), 2);
    // fallback was attempted 2 times (1 init + 1 retry) and NEVER re-selected
    assert_eq!(fallback.call_count(), 2);

    // Exactly one usage record recorded
    let summary = state
        .usage
        .get_summary(UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.successful_requests, 0);
}

// =========================================================================
// 6. MANUAL MODEL SELECTION INVARIANT (NO MODEL SWITCHING)
// =========================================================================

#[tokio::test]
async fn test_manual_model_selection_strictly_preserves_target_never_fails_over() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 1;
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 15;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter hosting "fast-chat-v1" fails persistently
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 503,
                message: "Service Unavailable".to_string(),
            }),
    );
    providers.register(primary.clone()).unwrap();

    // Secondary fallback adapter hosting compatible models is operational
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback").with_models(vec![m2]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // User explicitly requests manual routing for "fast-chat-v1"
    let payload = json!({
        "messages": [{ "role": "user", "content": "Manual routing failover test" }],
        "routing_mode": "manual",
        "requested_model_id": "fast-chat-v1"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    // INVARIANT: In manual mode, we NEVER switch to another model.
    // The request MUST fail with BAD_GATEWAY.
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);

    // Primary attempted 2 times (1 + 1 retry)
    assert_eq!(primary.call_count(), 2);
    // Fallback was STRICTLY NEVER CALLED
    assert_eq!(fallback.call_count(), 0);
}

// =========================================================================
// 7. STREAMING RELIABILITY: PRE-STREAM VS MID-STREAM
// =========================================================================

#[tokio::test]
async fn test_streaming_pre_stream_transient_failure_retries_and_succeeds() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2;
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 20;
    let config = Arc::new(config_raw);

    let (registry, m1, _) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails pre-stream once with 503, then succeeds on retry
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_transient_failures(
                1,
                ProviderError::UpstreamError {
                    status_code: 503,
                    message: "Stream connection failed".to_string(),
                },
            )
            .with_stream_chunks(vec!["Hello ".to_string(), "world!".to_string()]),
    );
    providers.register(primary.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Streaming pre-stream retry" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body_text = resp.text().await.unwrap();

    assert!(body_text.contains("event: routing"));
    assert!(body_text.contains("\"is_fallback\":false"));
    assert!(body_text.contains("Hello "));
    assert!(body_text.contains("world!"));
    assert!(body_text.contains("event: done"));

    assert_eq!(primary.call_count(), 2);

    // Exactly one usage record
    let summary = state
        .usage
        .get_summary(UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.successful_requests, 1);
}

#[tokio::test]
async fn test_streaming_midstream_interruption_never_restarts_or_duplicates() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 2;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Upstream server for primary emits 1 chunk, then closes socket abruptly
    let (primary_url, _h1) = spawn_mock_openai_upstream(|_, _| {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Chunk Alpha\"}}]}\n\n";
        (
            200,
            vec![("Content-Type", "text/event-stream")],
            sse.as_bytes().to_vec(),
        )
    })
    .await;

    let primary = Arc::new(
        OpenAiCompatibleAdapter::new(
            "groq-api",
            "Groq Primary",
            &primary_url,
            Some("key".to_string()),
        )
        .with_models(vec![m1]),
    );
    providers.register(primary).unwrap();

    // Secondary fallback adapter configured
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback").with_models(vec![m2]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Midstream test" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body_text = resp.text().await.unwrap();

    // 1. Initial chunk was received
    assert!(body_text.contains("Chunk Alpha"));

    // 2. Stream terminated cleanly with event: done
    assert!(body_text.contains("event: done"));

    // 3. Fallback was NEVER invoked (no partial output duplication)
    assert_eq!(
        fallback.call_count(),
        0,
        "Fallback must NEVER be invoked after stream headers/content delivered"
    );

    // 4. Exactly one usage record
    let summary = state
        .usage
        .get_summary(UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(summary.total_requests, 1);
}

// =========================================================================
// 8. EXACTLY-ONCE USAGE ACCOUNTING ACROSS RETRIES AND FAILOVER
// =========================================================================

#[tokio::test]
async fn test_exactly_once_usage_recording_with_retries_and_failover() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 1;
    config_raw.initial_backoff_ms = 5;
    config_raw.max_backoff_ms = 15;
    let config = Arc::new(config_raw);

    let (registry, m1, m2) = create_two_model_registry();
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails twice (exhausting retries on m1)
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m1])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 500,
                message: "Internal server error".to_string(),
            }),
    );
    providers.register(primary.clone()).unwrap();

    // Fallback adapter fails once transiently, then succeeds on attempt 2
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback")
            .with_models(vec![m2])
            .with_transient_failures(
                1,
                ProviderError::UpstreamError {
                    status_code: 503,
                    message: "Temporary glitch".to_string(),
                },
            ),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state.clone()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "Usage idempotency test" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["provider_used"], "openrouter-api");
    assert_eq!(body["is_fallback"], true);

    // Primary: 2 calls (1 init + 1 retry)
    assert_eq!(primary.call_count(), 2);
    // Fallback: 2 calls (1 init failure + 1 retry success)
    assert_eq!(fallback.call_count(), 2);

    // Verify usage storage has EXACTLY ONE row recorded
    let summary = state
        .usage
        .get_summary(UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(
        summary.total_requests, 1,
        "Exactly one request record must exist despite 4 total adapter attempts"
    );
    assert_eq!(summary.successful_requests, 1);
    assert_eq!(summary.total_tokens, 60); // standard mock usage: 25 prompt + 35 completion
}

// =========================================================================
// 9. CAPABILITY & TASK COMPATIBILITY DURING ALTERNATIVE SELECTION
// =========================================================================

#[tokio::test]
async fn test_failover_respects_task_and_modality_compatibility() {
    let mut config_raw = AppConfig::default_local();
    config_raw.max_retries = 1;
    let config = Arc::new(config_raw);

    let registry = Arc::new(ModelRegistry::empty());

    // Primary model: text & code on groq-api
    let m_primary = ModelMetadata {
        model_id: "text-model-1".to_string(),
        display_name: "Text Primary".to_string(),
        provider_id: "groq-api".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 32768,
        max_output_tokens: 4096,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    // Incompatible candidate: image-only on vision-api
    let m_incompatible = ModelMetadata {
        model_id: "image-model-1".to_string(),
        display_name: "Vision Only".to_string(),
        provider_id: "vision-api".to_string(),
        supported_modalities: vec!["image".to_string()], // INCOMPATIBLE with text request
        context_window: 16384,
        max_output_tokens: 2048,
        streaming_supported: false,
        tool_calling_supported: false,
        speed_tier: "balanced".to_string(),
        cost_tier: "premium".to_string(),
        available: true,
        capabilities: vec!["image_generation".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    // Compatible fallback candidate: text on openrouter-api
    let m_compatible_fallback = ModelMetadata {
        model_id: "text-model-2".to_string(),
        display_name: "Text Fallback".to_string(),
        provider_id: "openrouter-api".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 65536,
        max_output_tokens: 8192,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "balanced".to_string(),
        cost_tier: "low-cost".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    registry.register_model(m_primary.clone()).unwrap();
    registry.register_model(m_incompatible.clone()).unwrap();
    registry
        .register_model(m_compatible_fallback.clone())
        .unwrap();

    let providers = Arc::new(ProviderRegistry::new());

    // Primary fails
    let primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(vec![m_primary])
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 502,
                message: "Primary Bad Gateway".to_string(),
            }),
    );
    providers.register(primary.clone()).unwrap();

    // Incompatible vision adapter
    let vision = Arc::new(
        MockProviderAdapter::new("vision-api", "Vision Provider").with_models(vec![m_incompatible]),
    );
    providers.register(vision.clone()).unwrap();

    // Compatible text fallback
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Text Fallback")
            .with_models(vec![m_compatible_fallback]),
    );
    providers.register(fallback.clone()).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [{ "role": "user", "content": "Hello text question" }],
        "routing_mode": "auto",
        "requested_modality": "text"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.unwrap();

    // Succeeded on compatible fallback
    assert_eq!(body["provider_used"], "openrouter-api");
    assert_eq!(body["model_used"], "text-model-2");
    assert_eq!(body["is_fallback"], true);

    // Primary was attempted
    assert_eq!(primary.call_count(), 2);
    // Incompatible vision adapter was STRICTLY NEVER CALLED
    assert_eq!(
        vision.call_count(),
        0,
        "Incompatible candidate model must never be selected during failover"
    );
    // Compatible text fallback was called
    assert_eq!(fallback.call_count(), 1);
}
