//! Comprehensive integration tests for real inference implementation, HTTP adapters,
//! chat completion and SSE streaming endpoints, bounded failover, authentication,
//! request ID tracking, and security controls.
//!
//! Validates:
//! - GAP-001: Chat completion & SSE streaming endpoints.
//! - GAP-002: Real HTTP provider adapter & mock HTTP server integration.
//! - GAP-003: Router-to-provider execution pipeline.
//! - GAP-005: Bounded failover and non-retryable short-circuiting.
//! - GAP-006: Request ID generation and header propagation.
//! - GAP-007: Constant-time API key authentication middleware.
//! - GAP-009: Provider catalog status derivation and offline fallback.
//! - GAP-010: Provider base_url omission and secret redaction.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::create_app;
use ai_backend::models::{ChatMessage, CompletionRequest};
use ai_backend::providers::{
    MockProviderAdapter, OpenAiCompatibleAdapter, ProviderAdapter, ProviderError, ProviderRegistry,
};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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
                    429 => "429 Too Many Requests",
                    500 => "500 Internal Server Error",
                    502 => "502 Bad Gateway",
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
// 1. REAL HTTP PROVIDER ADAPTER TESTS (GAP-002)
// =========================================================================

#[tokio::test]
async fn test_real_http_adapter_completion_success() {
    let (upstream_url, _handle) = spawn_mock_openai_upstream(|path, req_str| {
        assert_eq!(path, "/chat/completions");
        assert!(
            req_str
                .to_lowercase()
                .contains("authorization: bearer test_secret_key")
        );
        assert!(
            req_str
                .to_lowercase()
                .contains("content-type: application/json")
        );

        let body = json!({
            "id": "chatcmpl_mock_12345",
            "model": "llama-3.3-70b-versatile",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Rust is blazing fast and memory-efficient."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 8,
                "total_tokens": 20
            }
        });

        (
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&body).unwrap(),
        )
    })
    .await;

    let adapter = OpenAiCompatibleAdapter::new(
        "mock-http-provider",
        "Mock HTTP Provider",
        &upstream_url,
        Some("test_secret_key".to_string()),
    );

    let req = CompletionRequest::new(
        "fast-chat-v1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "Tell me about Rust.".to_string(),
        }],
    );

    let response = adapter
        .complete(&req)
        .await
        .expect("Completion must succeed");
    assert_eq!(response.id, "chatcmpl_mock_12345");
    assert_eq!(response.model_used, "fast-chat-v1");
    assert_eq!(response.provider_used, "mock-http-provider");
    assert_eq!(
        response.message.content,
        "Rust is blazing fast and memory-efficient."
    );
    assert_eq!(response.message.role, "assistant");

    let usage = response.usage.expect("Usage metadata must be present");
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.completion_tokens, 8);
    assert_eq!(usage.total_tokens, 20);
    assert!(!usage.is_estimated);
}

#[tokio::test]
async fn test_real_http_adapter_streaming_sse() {
    let (upstream_url, _handle) = spawn_mock_openai_upstream(|path, _| {
        assert_eq!(path, "/chat/completions");

        let sse_body = "data: {\"id\":\"cmpl_1\",\"choices\":[{\"delta\":{\"content\":\"Chunk1\"}}]}\n\n\
                        data: {\"id\":\"cmpl_1\",\"choices\":[{\"delta\":{\"content\":\" Chunk2\"}}]}\n\n\
                        data: [DONE]\n\n";

        (
            200,
            vec![("Content-Type", "text/event-stream")],
            sse_body.as_bytes().to_vec(),
        )
    })
    .await;

    let adapter = OpenAiCompatibleAdapter::new(
        "mock-streaming-provider",
        "Mock Streaming Provider",
        &upstream_url,
        Some("test_stream_key".to_string()),
    );

    let req = CompletionRequest::new(
        "fast-chat-v1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "Stream something.".to_string(),
        }],
    );

    let mut stream = adapter.stream(&req).await.expect("Stream must initiate");
    let mut chunks = Vec::new();

    while let Some(item) = stream.next().await {
        let chunk = item.expect("Stream chunk must be Ok");
        if chunk.is_done {
            break;
        }
        chunks.push(chunk.delta);
    }

    assert_eq!(chunks, vec!["Chunk1".to_string(), " Chunk2".to_string()]);
}

#[tokio::test]
async fn test_real_http_adapter_error_taxonomy_and_secret_redaction() {
    // 1. Test 401 Authentication Failure (secrets redacted)
    let (auth_fail_url, _h1) = spawn_mock_openai_upstream(|_, _| {
        (
            401,
            vec![("Content-Type", "application/json")],
            b"{\"error\": \"Unauthorized token\"}".to_vec(),
        )
    })
    .await;

    let adapter_auth = OpenAiCompatibleAdapter::new(
        "test-groq",
        "Test Groq",
        &auth_fail_url,
        Some("leaked_secret_val".to_string()),
    );
    let req = CompletionRequest::new(
        "m1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
        }],
    );
    let err = adapter_auth.complete(&req).await.unwrap_err();
    assert!(matches!(err, ProviderError::AuthenticationFailed(_)));
    assert!(!format!("{:?}", err).contains("leaked_secret_val"));

    // 2. Test 429 Rate Limiting with Retry-After header
    let (rate_limit_url, _h2) = spawn_mock_openai_upstream(|_, _| {
        (
            429,
            vec![("Content-Type", "application/json"), ("Retry-After", "15")],
            b"{\"error\": \"Rate limit exceeded\"}".to_vec(),
        )
    })
    .await;

    let adapter_rate = OpenAiCompatibleAdapter::new(
        "test-rate",
        "Test Rate",
        &rate_limit_url,
        Some("k".to_string()),
    );
    let err_rate = adapter_rate.complete(&req).await.unwrap_err();
    assert_eq!(
        err_rate,
        ProviderError::RateLimited {
            retry_after_secs: Some(15)
        }
    );

    // 3. Test 502 Upstream Error
    let (upstream_err_url, _h3) = spawn_mock_openai_upstream(|_, _| {
        (
            502,
            vec![("Content-Type", "text/plain")],
            b"Bad Gateway".to_vec(),
        )
    })
    .await;

    let adapter_502 = OpenAiCompatibleAdapter::new(
        "test-502",
        "Test 502",
        &upstream_err_url,
        Some("k".to_string()),
    );
    let err_502 = adapter_502.complete(&req).await.unwrap_err();
    assert!(matches!(
        err_502,
        ProviderError::UpstreamError {
            status_code: 502,
            ..
        }
    ));

    // 4. Test Timeout Mapping
    let (timeout_url, _h4) = spawn_mock_openai_upstream(|_, _| {
        std::thread::sleep(Duration::from_millis(150));
        (200, vec![], b"OK".to_vec())
    })
    .await;

    let adapter_to = OpenAiCompatibleAdapter::new(
        "test-timeout",
        "Test Timeout",
        &timeout_url,
        Some("k".to_string()),
    );
    let mut timeout_req = req.clone();
    timeout_req.timeout = Duration::from_millis(30);
    let err_timeout = adapter_to.complete(&timeout_req).await.unwrap_err();
    assert!(matches!(err_timeout, ProviderError::Timeout(_)));
}

// =========================================================================
// 2. CHAT COMPLETION & STREAMING API ENDPOINTS (GAP-001 & GAP-003)
// =========================================================================

#[tokio::test]
async fn test_chat_completions_endpoint_end_to_end() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    // Register active mock provider
    let mock_adapter = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Cloud")
            .with_models(registry.list_models(None, None, None)),
    );
    providers.register(mock_adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [
            { "role": "user", "content": "What is the capital of France?" }
        ],
        "routing_mode": "auto",
        "temperature": 0.7,
        "max_tokens": 150
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .expect("HTTP request must succeed");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Verify X-Request-ID header generated
    let req_id = resp
        .headers()
        .get("x-request-id")
        .expect("x-request-id header missing");
    assert!(!req_id.to_str().unwrap().is_empty());

    let body: Value = resp.json().await.expect("Valid JSON response");
    assert_eq!(body["message"]["role"], "assistant");
    assert!(
        body["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Mock completion response")
    );
    assert_eq!(body["provider_used"], "groq-api");
    assert_eq!(body["is_fallback"], false);
    assert!(body["usage"]["total_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn test_chat_stream_endpoint_end_to_end() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    let mock_adapter = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Cloud")
            .with_models(registry.list_models(None, None, None))
            .with_stream_chunks(vec![
                "Thinking".to_string(),
                "...".to_string(),
                " Done!".to_string(),
            ]),
    );
    providers.register(mock_adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [
            { "role": "user", "content": "Stream me a response" }
        ],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&payload)
        .send()
        .await
        .expect("Stream HTTP request must succeed");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/event-stream"));

    let body_text = resp.text().await.expect("Read stream text");
    assert!(body_text.contains("event: routing"));
    assert!(body_text.contains("event: chunk"));
    assert!(body_text.contains("Thinking"));
    assert!(body_text.contains("Done!"));
    assert!(body_text.contains("event: done"));
}

// =========================================================================
// 3. BOUNDED FAILOVER & RETRY LIMITS (GAP-005)
// =========================================================================

#[tokio::test]
async fn test_bounded_failover_primary_fails_fallback_succeeds() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails with 502 UpstreamError
    let failing_primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(registry.list_models(None, None, None))
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 502,
                message: "Gateway failure".to_string(),
            }),
    );
    providers.register(failing_primary).unwrap();

    // Secondary fallback adapter succeeds
    let successful_fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback")
            .with_models(registry.list_models(None, None, None)),
    );
    providers.register(successful_fallback).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [
            { "role": "user", "content": "Test failover execution" }
        ],
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

    // Verify failover occurred and was flagged
    assert_eq!(body["provider_used"], "openrouter-api");
    assert_eq!(body["is_fallback"], true);
    assert!(
        body["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Mock completion response")
    );
}

#[tokio::test]
async fn test_non_retryable_error_does_not_attempt_failover() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails with non-retryable InvalidRequest
    let failing_primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(registry.list_models(None, None, None))
            .with_simulated_error(ProviderError::InvalidRequest(
                "Invalid context length for model".to_string(),
            )),
    );
    providers.register(failing_primary).unwrap();

    // Fallback adapter configured
    let fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback")
            .with_models(registry.list_models(None, None, None)),
    );
    providers.register(fallback).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let payload = json!({
        "messages": [
            { "role": "user", "content": "Trigger invalid request" }
        ],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    // Must short-circuit immediately with 400 Bad Request without falling back
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
}

// =========================================================================
// 4. REQUEST VALIDATION & ERROR NORMALIZATION (GAP-001)
// =========================================================================

#[tokio::test]
async fn test_request_validation_errors() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);
    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Case 1: Empty messages array -> 400
    let resp1 = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({ "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), reqwest::StatusCode::BAD_REQUEST);
    let b1: Value = resp1.json().await.unwrap();
    assert_eq!(b1["error"]["code"], "INVALID_REQUEST");

    // Case 2: Invalid message role -> 400
    let resp2 = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "invalid_role", "content": "hello" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), reqwest::StatusCode::BAD_REQUEST);

    // Case 3: Temperature > 2.0 -> 400
    let resp3 = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "hello" }],
            "temperature": 2.5
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp3.status(), reqwest::StatusCode::BAD_REQUEST);

    // Case 4: Manual routing non-existent model -> 404
    let resp4 = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "hello" }],
            "routing_mode": "manual",
            "requested_model_id": "nonexistent-model"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp4.status(), reqwest::StatusCode::NOT_FOUND);
    let b4: Value = resp4.json().await.unwrap();
    assert_eq!(b4["error"]["code"], "NOT_FOUND");
}

// =========================================================================
// 5. AUTHENTICATION & REQUEST ID PROPAGATION (GAP-006 & GAP-007)
// =========================================================================

#[tokio::test]
async fn test_request_id_propagation() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);
    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Client passes custom x-request-id
    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .header("x-request-id", "trace-client-id-abc123")
        .json(&json!({ "messages": [] })) // Fails validation
        .send()
        .await
        .unwrap();

    let resp_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(resp_id, "trace-client-id-abc123");

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["request_id"], "trace-client-id-abc123");
}

#[tokio::test]
async fn test_api_key_authentication_enforcement() {
    let mut config_raw = AppConfig::default_local();
    config_raw.api_key = Some("super_secret_auth_token_xyz99".to_string());
    let config = Arc::new(config_raw);
    let state = AppState::new(config);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Missing auth credentials on protected endpoint -> 401
    let resp_unauth = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({ "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_unauth.status(), reqwest::StatusCode::UNAUTHORIZED);
    let b_unauth: Value = resp_unauth.json().await.unwrap();
    assert_eq!(b_unauth["error"]["code"], "UNAUTHORIZED");

    // 2. Invalid Bearer token -> 401
    let resp_bad_token = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .bearer_auth("wrong_token_123")
        .json(&json!({ "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_bad_token.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 3. Valid Bearer token -> reaches route handler (returns 502 since 0 active providers)
    let resp_auth_bearer = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .bearer_auth("super_secret_auth_token_xyz99")
        .json(&json!({ "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_auth_bearer.status(), reqwest::StatusCode::BAD_GATEWAY);

    // 4. Valid X-API-Key header -> authorized
    let resp_auth_header = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .header("X-API-Key", "super_secret_auth_token_xyz99")
        .json(&json!({ "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_auth_header.status(), reqwest::StatusCode::BAD_GATEWAY);

    // 5. Public probe endpoints bypass authentication
    let resp_health = client
        .get(format!("{}/api/v1/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_health.status(), reqwest::StatusCode::OK);

    let resp_ready = client
        .get(format!("{}/api/v1/ready", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_ready.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
}

// =========================================================================
// 6. PROVIDER CATALOG & SECRET HYGIENE (GAP-009 & GAP-010)
// =========================================================================

#[tokio::test]
async fn test_provider_catalog_omits_base_url_and_marks_offline_when_empty() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);
    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/v1/providers", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let raw_text = resp.text().await.unwrap();

    // 1. base_url MUST NEVER appear in public provider responses
    assert!(!raw_text.contains("base_url"));
    assert!(!raw_text.contains("https://"));

    // 2. When active providers count is 0, catalog reflects offline status (GAP-009)
    let body: Value = serde_json::from_str(&raw_text).unwrap();
    let providers_arr = body["providers"].as_array().unwrap();
    for p in providers_arr {
        assert_eq!(p["status"], "offline");
        assert_eq!(p["enabled"], false);
    }
}

// =========================================================================
// 7. CONFIGURATION VALIDATION & ERROR REJECTION
// =========================================================================

#[test]
fn test_invalid_config_rejections() {
    use ai_backend::config::{DeploymentMode, Environment};

    // 1. Port 0 is rejected
    let mut c1 = AppConfig::default_local();
    c1.port = 0;
    assert!(c1.validate().is_err());

    // 2. Upstream timeout 0s or > 30s is rejected
    let mut c2 = AppConfig::default_local();
    c2.upstream_timeout_secs = 0;
    assert!(c2.validate().is_err());
    c2.upstream_timeout_secs = 31;
    assert!(c2.validate().is_err());

    // 3. Max body size < 1KB or > 10MB is rejected
    let mut c3 = AppConfig::default_local();
    c3.max_body_size_bytes = 500;
    assert!(c3.validate().is_err());
    c3.max_body_size_bytes = 15_000_000;
    assert!(c3.validate().is_err());

    // 4. API_KEY shorter than 8 chars is rejected
    let mut c4 = AppConfig::default_local();
    c4.api_key = Some("short".to_string());
    assert!(c4.validate().is_err());

    // 5. Valid config is accepted
    let mut c5 = AppConfig::default_local();
    c5.environment = Environment::Production;
    c5.deployment_mode = DeploymentMode::Serverless;
    c5.api_key = Some("valid_secret_key_888".to_string());
    assert!(c5.validate().is_ok());
}

// =========================================================================
// 8. PRODUCTION SECURITY CONTROLS & UNSAFE ACCESS PREVENTION
// =========================================================================

#[tokio::test]
async fn test_production_security_blocks_unauthenticated_access_when_no_key() {
    use ai_backend::config::Environment;

    // In production without an API key configured:
    let mut config_raw = AppConfig::default_local();
    config_raw.environment = Environment::Production;
    config_raw.api_key = None;
    let config = Arc::new(config_raw);
    let state = AppState::new(config);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Protected route: chat completions MUST return 401 Unauthorized
    let resp_chat = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({ "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_chat.status(), reqwest::StatusCode::UNAUTHORIZED);
    let b: Value = resp_chat.json().await.unwrap();
    assert_eq!(b["error"]["code"], "UNAUTHORIZED");

    // 2. Protected route: chat stream MUST return 401 Unauthorized
    let resp_stream = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({ "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_stream.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 3. Protected route: models list MUST return 401 Unauthorized
    let resp_models = client
        .get(format!("{}/api/v1/models", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_models.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 4. Protected route: routing preview MUST return 401 Unauthorized
    let resp_preview = client
        .post(format!("{}/api/v1/routing/preview", base_url))
        .json(&json!({ "prompt": "Hello" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_preview.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 5. Public liveness probe remains accessible without auth
    let resp_health = client
        .get(format!("{}/api/v1/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_health.status(), reqwest::StatusCode::OK);

    // 6. Public readiness probe remains accessible without auth
    let resp_ready = client
        .get(format!("{}/api/v1/ready", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_ready.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
}

// =========================================================================
// 9. MALFORMED UPSTREAM RESPONSES & ERROR NORMALIZATION
// =========================================================================

#[tokio::test]
async fn test_malformed_upstream_response_mapping() {
    let (upstream_url, _handle) = spawn_mock_openai_upstream(|_, _| {
        // Upstream returns 200 OK but corrupt non-JSON HTML body
        (
            200,
            vec![("Content-Type", "text/html")],
            b"<html><body>502 Bad Gateway Cloudflare Error</body></html>".to_vec(),
        )
    })
    .await;

    let adapter = OpenAiCompatibleAdapter::new(
        "mock-broken-provider",
        "Mock Broken Provider",
        &upstream_url,
        Some("test_secret_key".to_string()),
    );

    let req = CompletionRequest::new(
        "fast-chat-v1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
    );

    let err = adapter.complete(&req).await.unwrap_err();
    match err {
        ProviderError::UpstreamError {
            status_code,
            message,
        } => {
            assert_eq!(status_code, 502);
            assert!(message.contains("Failed to deserialize upstream completion JSON"));
            // Ensure raw upstream HTML is sanitized and not echoed in error message
            assert!(!message.contains("<html>"));
            assert!(!message.contains("Cloudflare"));
        }
        other => panic!("Expected UpstreamError, got: {:?}", other),
    }
}

// =========================================================================
// 10. REQUEST BODY SIZE LIMITS (4.5 MB ENFORCEMENT)
// =========================================================================

#[tokio::test]
async fn test_request_body_size_limit_rejection() {
    let mut config_raw = AppConfig::default_local();
    // Configure small body limit to test enforcement deterministically
    config_raw.max_body_size_bytes = 2048; // 2 KB
    let config = Arc::new(config_raw);
    let state = AppState::new(config);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Payload with 4 KB content exceeds 2 KB limit
    let large_text = "a".repeat(4096);
    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": large_text }]
        }))
        .send()
        .await
        .unwrap();

    // Axum DefaultBodyLimit returns 413 Payload Too Large
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
}

// =========================================================================
// 11. TIMEOUT ENFORCEMENT & CANCELLED STREAMS
// =========================================================================

#[tokio::test]
async fn test_upstream_timeout_mapping() {
    let (upstream_url, _handle) = spawn_mock_openai_upstream(|_, _| {
        // Sleep for 1.5 seconds to trigger timeout
        std::thread::sleep(Duration::from_millis(1500));
        (
            200,
            vec![("Content-Type", "application/json")],
            b"{}".to_vec(),
        )
    })
    .await;

    let adapter = OpenAiCompatibleAdapter::new(
        "mock-timeout-provider",
        "Mock Timeout Provider",
        &upstream_url,
        Some("test_key".to_string()),
    );

    let mut req = CompletionRequest::new(
        "fast-chat-v1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
    );
    // Explicit tight timeout of 300ms
    req.timeout = Duration::from_millis(300);

    let err = adapter.complete(&req).await.unwrap_err();
    match err {
        ProviderError::Timeout(d) => {
            assert_eq!(d, Duration::from_millis(300));
        }
        other => panic!("Expected Timeout error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_streaming_midstream_connection_interruption() {
    let (upstream_url, _handle) = spawn_mock_openai_upstream(|_, _| {
        // Upstream sends 1 valid chunk and then aborts without [DONE]
        let sse_body =
            "data: {\"id\":\"cmpl_1\",\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n";
        (
            200,
            vec![("Content-Type", "text/event-stream")],
            sse_body.as_bytes().to_vec(),
        )
    })
    .await;

    let adapter = OpenAiCompatibleAdapter::new(
        "mock-stream-abort-provider",
        "Mock Stream Abort Provider",
        &upstream_url,
        Some("test_key".to_string()),
    );

    let req = CompletionRequest::new(
        "fast-chat-v1",
        vec![ChatMessage {
            role: "user".to_string(),
            content: "Hi".to_string(),
        }],
    );

    let mut stream = adapter.stream(&req).await.unwrap();

    // Chunk 1 arrives normally
    let chunk1 = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk1.delta, "Hello ");
    assert!(!chunk1.is_done);

    // After upstream finishes its response body, EOF yields is_done: true terminal chunk
    let chunk2 = stream.next().await.unwrap().unwrap();
    assert!(chunk2.is_done);

    // Stream closes gracefully
    assert!(stream.next().await.is_none());
}

// =========================================================================
// 12. ZERO SECRET LEAKAGE IN DEBUG OUTPUTS & RESPONSES
// =========================================================================

#[test]
fn test_secret_redaction_and_zero_credential_leak() {
    let mut config = AppConfig::default_local();
    config.groq_api_key = Some("gsk_ultra_secret_groq_key_99999".to_string());
    config.openrouter_api_key = Some("sk-or-v1-super_secret_openrouter_token".to_string());
    config.gemini_api_key = Some("AIzaSySecretGeminiToken12345".to_string());
    config.api_key = Some("backend_master_auth_secret_77777".to_string());

    let debug_output = format!("{:?}", config);

    // 1. All sensitive keys MUST be marked [REDACTED]
    assert!(debug_output.contains("groq_api_key: Some(\"[REDACTED]\")"));
    assert!(debug_output.contains("openrouter_api_key: Some(\"[REDACTED]\")"));
    assert!(debug_output.contains("gemini_api_key: Some(\"[REDACTED]\")"));
    assert!(debug_output.contains("api_key: Some(\"[REDACTED]\")"));

    // 2. Cleartext secrets MUST NEVER appear anywhere in debug representation
    assert!(!debug_output.contains("gsk_ultra_secret_groq_key_99999"));
    assert!(!debug_output.contains("sk-or-v1-super_secret_openrouter_token"));
    assert!(!debug_output.contains("AIzaSySecretGeminiToken12345"));
    assert!(!debug_output.contains("backend_master_auth_secret_77777"));
}

// =========================================================================
// 13. STREAMING LIFECYCLE, PRE-STREAM FALLBACK & NO-PARTIAL FALLBACK
// =========================================================================

#[tokio::test]
async fn test_streaming_pre_stream_fallback_on_retryable_error() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter fails when initiating stream with 502 UpstreamError
    let failing_primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(registry.list_models(None, None, None))
            .with_simulated_error(ProviderError::UpstreamError {
                status_code: 502,
                message: "Primary stream failed to connect".to_string(),
            }),
    );
    providers.register(failing_primary).unwrap();

    // Secondary adapter succeeds and streams
    let successful_fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback")
            .with_models(registry.list_models(None, None, None))
            .with_stream_chunks(vec!["Fallback stream chunk".to_string()]),
    );
    providers.register(successful_fallback).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "hello" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body_text = resp.text().await.unwrap();

    // Must have routed to fallback provider
    assert!(body_text.contains("event: routing"));
    assert!(body_text.contains("\"is_fallback\":true"));
    assert!(body_text.contains("openrouter-api"));
    assert!(body_text.contains("Fallback stream chunk"));
    assert!(body_text.contains("event: done"));
}

#[tokio::test]
async fn test_streaming_pre_stream_no_fallback_on_non_retryable_error() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    // Primary adapter returns non-retryable 400 InvalidRequest
    let failing_primary = Arc::new(
        MockProviderAdapter::new("groq-api", "Groq Primary")
            .with_models(registry.list_models(None, None, None))
            .with_simulated_error(ProviderError::InvalidRequest("Bad parameters".to_string())),
    );
    providers.register(failing_primary).unwrap();

    // Secondary fallback adapter should NEVER be called
    let successful_fallback = Arc::new(
        MockProviderAdapter::new("openrouter-api", "OpenRouter Fallback")
            .with_models(registry.list_models(None, None, None)),
    );
    providers.register(successful_fallback).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "hello" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    // Non-retryable error returns immediately as 400 Bad Request
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn test_streaming_no_fallback_after_partial_output() {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    // Create an ephemeral mock server for primary that sends 1 chunk then emits error
    let (primary_url, _h) = spawn_mock_openai_upstream(|_, _| {
        let sse_body =
            "data: {\"choices\":[{\"delta\":{\"content\":\"Initial partial text\"}}]}\n\n";
        (
            200,
            vec![("Content-Type", "text/event-stream")],
            sse_body.as_bytes().to_vec(),
        )
    })
    .await;

    let primary_adapter = Arc::new(
        OpenAiCompatibleAdapter::new(
            "groq-api",
            "Groq Primary",
            &primary_url,
            Some("key1".to_string()),
        )
        .with_models(registry.list_models(None, None, None)),
    );
    providers.register(primary_adapter).unwrap();

    // Secondary adapter call tracker
    let secondary_call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let s_count_clone = secondary_call_count.clone();

    let (secondary_url, _h2) = spawn_mock_openai_upstream(move |_, _| {
        s_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (
            200,
            vec![("Content-Type", "text/event-stream")],
            b"data: [DONE]\n\n".to_vec(),
        )
    })
    .await;

    let secondary_adapter = Arc::new(
        OpenAiCompatibleAdapter::new(
            "openrouter-api",
            "OpenRouter Secondary",
            &secondary_url,
            Some("key2".to_string()),
        )
        .with_models(registry.list_models(None, None, None)),
    );
    providers.register(secondary_adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "hello" }],
            "routing_mode": "auto"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body_text = resp.text().await.unwrap();

    // 1. Partial chunk was delivered
    assert!(body_text.contains("Initial partial text"));

    // 2. Stream terminated with event: done after EOF
    assert!(body_text.contains("event: done"));

    // 3. Secondary adapter was strictly NEVER called (call count == 0)
    assert_eq!(
        secondary_call_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "Secondary adapter must NEVER be called once streaming to client has already begun"
    );
}

// =========================================================================
// 14. DETERMINISTIC STREAMING CANCELLATION & DISCONNECT OBSERVATION
// =========================================================================

#[tokio::test]
async fn test_streaming_client_disconnect_cancels_upstream_connection() {
    let connection_closed_notify = Arc::new(tokio::sync::Notify::new());
    let notify_clone = connection_closed_notify.clone();
    let first_chunk_sent = Arc::new(tokio::sync::Notify::new());
    let first_chunk_sent_clone = first_chunk_sent.clone();

    // Spawn an upstream TCP server that streams infinite chunks until socket is closed
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = listener.local_addr().unwrap();

    let _mock_server = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;

            let response_header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = socket.write_all(response_header.as_bytes()).await;

            // Send chunk 1
            let chunk_data = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
            let chunk_framed = format!("{:X}\r\n{}\r\n", chunk_data.len(), chunk_data);
            let _ = socket.write_all(chunk_framed.as_bytes()).await;
            first_chunk_sent_clone.notify_one();

            // Loop trying to write chunks or detect socket disconnect
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let chunk_data = "data: {\"choices\":[{\"delta\":{\"content\":\" more\"}}]}\n\n";
                let chunk_framed = format!("{:X}\r\n{}\r\n", chunk_data.len(), chunk_data);
                if socket.write_all(chunk_framed.as_bytes()).await.is_err() {
                    // Upstream client (ai-backend adapter) disconnected / dropped connection!
                    notify_clone.notify_one();
                    break;
                }
            }
        }
    });

    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    let adapter = Arc::new(
        OpenAiCompatibleAdapter::new(
            "groq-api",
            "Groq Upstream",
            format!("http://{}", upstream_addr),
            Some("test_key".to_string()),
        )
        .with_models(registry.list_models(None, None, None)),
    );
    providers.register(adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let (base_url, _server) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Start streaming request from client
    let mut resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .json(&json!({
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Wait until upstream has sent the first chunk
    first_chunk_sent.notified().await;

    // Read partial response (at least routing or first chunk)
    let chunk = resp.chunk().await.unwrap();
    assert!(chunk.is_some());

    // Explicitly DROP the client response to simulate client disconnection!
    drop(resp);
    drop(client);

    // Now wait for the upstream server's synchronization signal indicating socket disconnect
    let cancelled =
        tokio::time::timeout(Duration::from_secs(4), connection_closed_notify.notified()).await;
    assert!(
        cancelled.is_ok(),
        "Upstream server MUST detect client disconnection and drop socket within timeout"
    );
}

// =========================================================================
// 15. SCORING POLICY WEIGHTS & RECONCILIATION AUDIT (GAP-011)
// =========================================================================

#[test]
fn test_scoring_formula_and_weights_determinism() {
    use ai_backend::models::ModelMetadata;
    use ai_backend::router::policy::RoutingPolicy;
    use ai_backend::router::{TaskCategory, TaskComplexity, TaskModality};

    let fast_model = ModelMetadata {
        model_id: "test-fast".to_string(),
        display_name: "Fast Model".to_string(),
        provider_id: "groq-api".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 131072,
        max_output_tokens: 8192,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    let advanced_model = ModelMetadata {
        model_id: "test-adv".to_string(),
        display_name: "Advanced Model".to_string(),
        provider_id: "openrouter-api".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 65536,
        max_output_tokens: 16384,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "advanced".to_string(),
        cost_tier: "low-cost".to_string(),
        available: true,
        capabilities: vec!["complex_analysis".to_string(), "deep_reasoning".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    let modality = TaskModality::Text;

    // 1. Low Complexity Task: 'fast' model receives +30 (complexity) + 15 (speed) = 45 points
    let scored_fast_low = RoutingPolicy::score_candidate(
        &fast_model,
        &TaskCategory::SimpleQuestions,
        &TaskComplexity::Low,
        &modality,
    )
    .unwrap();
    // Capability: 45, Complexity: 30, Speed: 15, Cost: 15, Capacity: 1 = 106 pts
    assert_eq!(scored_fast_low.total_score, 106);

    // 2. High Complexity Task: 'advanced' model receives +30 (complexity) + 15 (speed) = 45 points
    let scored_adv_high = RoutingPolicy::score_candidate(
        &advanced_model,
        &TaskCategory::ComplexAnalysis,
        &TaskComplexity::High,
        &modality,
    )
    .unwrap();
    // Capability: 55, Complexity: 30, Speed: 15, Cost: 10, Capacity: 10 = 120 pts
    assert_eq!(scored_adv_high.total_score, 120);

    // 3. Mathematical proof that total score is bounded, positive, and free of overflow
    assert!(scored_fast_low.total_score <= 125);
    assert!(scored_adv_high.total_score <= 125);
}

#[tokio::test]
async fn test_openai_adapter_reasoning_isolation_and_content_integrity() {
    let (upstream_url, _handle) = spawn_mock_openai_upstream(|_path, body| {
        let is_stream = body.contains("\"stream\":true") || body.contains("\"stream\": true");
        if is_stream {
            // Emits: 1 reasoning chunk, 1 content chunk, 1 usage chunk
            let sse = "data: {\"choices\":[{\"delta\":{\"content\":null,\"reasoning\":\"internal thought\"}}]}\n\n\
                       data: {\"choices\":[{\"delta\":{\"content\":\"Final answer\"}}]}\n\n\
                       data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n\
                       data: [DONE]\n\n";
            (
                200,
                vec![("content-type", "text/event-stream")],
                sse.as_bytes().to_vec(),
            )
        } else if body.contains("empty-content") {
            // Case 1: Pure empty content response without reasoning
            let resp = json!({
                "id": "chatcmpl_empty_content",
                "model": "openai/gpt-oss-120b",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 0,
                    "total_tokens": 5
                }
            });
            (
                200,
                vec![("content-type", "application/json")],
                serde_json::to_vec(&resp).unwrap(),
            )
        } else if body.contains("reasoning-only") {
            // Case 2: Model ran out of tokens during reasoning; content is empty string
            let resp = json!({
                "id": "chatcmpl_reasoning_only",
                "model": "openai/gpt-oss-120b",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning": "Internal chain of thought that must not leak"
                    },
                    "finish_reason": "length"
                }],
                "usage": {
                    "prompt_tokens": 12,
                    "completion_tokens": 10,
                    "total_tokens": 22
                }
            });
            (
                200,
                vec![("content-type", "application/json")],
                serde_json::to_vec(&resp).unwrap(),
            )
        } else {
            // Case 2: Normal response with both content and reasoning
            let resp = json!({
                "id": "chatcmpl_normal",
                "model": "openai/gpt-oss-120b",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "User-facing answer",
                        "reasoning": "Internal thought"
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 8,
                    "completion_tokens": 6,
                    "total_tokens": 14
                }
            });
            (
                200,
                vec![("content-type", "application/json")],
                serde_json::to_vec(&resp).unwrap(),
            )
        }
    })
    .await;

    let adapter = OpenAiCompatibleAdapter::new(
        "test-provider",
        "Test Reasoning Provider",
        &upstream_url,
        Some("test_key".to_string()),
    );

    // 1. Verify empty content (null/absent) defaults cleanly to empty string and captures usage
    let req_empty = CompletionRequest {
        model_id: "test-model".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "empty-content prompt".to_string(),
        }],
        temperature: None,
        max_tokens: None,
        stream: false,
        timeout: Duration::from_secs(5),
    };
    let res_empty = adapter.complete(&req_empty).await.unwrap();
    assert_eq!(
        res_empty.message.content, "",
        "Empty content must resolve to empty string without error"
    );
    assert_eq!(res_empty.usage.as_ref().unwrap().prompt_tokens, 5);
    assert_eq!(res_empty.usage.as_ref().unwrap().completion_tokens, 0);
    assert_eq!(res_empty.usage.as_ref().unwrap().total_tokens, 5);

    // 2. Verify reasoning-only output does NOT leak into content, but usage is preserved
    let req_reasoning_only = CompletionRequest {
        model_id: "test-model".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "reasoning-only prompt".to_string(),
        }],
        temperature: None,
        max_tokens: None,
        stream: false,
        timeout: Duration::from_secs(5),
    };
    let res1 = adapter.complete(&req_reasoning_only).await.unwrap();
    assert_eq!(
        res1.message.content, "",
        "Reasoning must never be substituted as user-facing content"
    );
    assert_eq!(res1.usage.as_ref().unwrap().prompt_tokens, 12);
    assert_eq!(res1.usage.as_ref().unwrap().completion_tokens, 10);
    assert_eq!(res1.usage.as_ref().unwrap().total_tokens, 22);

    // 3. Verify normal content output is cleanly extracted and preserved
    let req_normal = CompletionRequest {
        model_id: "test-model".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "normal prompt".to_string(),
        }],
        temperature: None,
        max_tokens: None,
        stream: false,
        timeout: Duration::from_secs(5),
    };
    let res2 = adapter.complete(&req_normal).await.unwrap();
    assert_eq!(res2.message.content, "User-facing answer");
    assert_eq!(res2.usage.as_ref().unwrap().total_tokens, 14);

    // 4. Verify streaming deltas: reasoning chunks are filtered out; only content deltas emitted
    let mut stream = adapter.stream(&req_normal).await.unwrap();

    // First content chunk
    let chunk1 = stream.next().await.unwrap().unwrap();
    assert_eq!(
        chunk1.delta, "Final answer",
        "Stream chunk must contain user-facing content, not reasoning"
    );

    // Usage chunk
    let chunk2 = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk2.delta, "");
    assert_eq!(chunk2.usage.as_ref().unwrap().total_tokens, 15);

    // Completion chunk
    let chunk3 = stream.next().await.unwrap().unwrap();
    assert!(chunk3.is_done);
}
