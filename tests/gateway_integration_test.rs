//! Integration tests for FreeLLMAPI and OmniRoute gateway integrations.
//!
//! Validates:
//! 1. Configuration parsing, defaults, and zero-leak secret redaction.
//! 2. SSRF destination policy permitting authorized gateway loopback endpoints while blocking prohibited ports/IPs.
//! 3. Wire protocol compliance and Bearer authentication headers.
//! 4. Model namespacing, wire-name translation, and non-collision.
//! 5. Buffered chat completions across multiple models per gateway.
//! 6. Server-Sent Events (SSE) streaming through both gateways.
//! 7. Automatic routing and task classification selecting gateway models.
//! 8. Bounded failover when a gateway returns 429 or 503 errors.
//! 9. Fail-fast behavior for non-retryable errors (401 Unauthorized).

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::create_app;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Ephemeral mock HTTP server simulating OpenAI-compatible gateway endpoints.
async fn spawn_mock_gateway(
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
                let mut buf = vec![0u8; 16384];
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
                    400 => "400 Bad Request",
                    401 => "401 Unauthorized",
                    403 => "403 Forbidden",
                    404 => "404 Not Found",
                    429 => "429 Too Many Requests",
                    500 => "500 Internal Server Error",
                    503 => "503 Service Unavailable",
                    504 => "504 Gateway Timeout",
                    _ => "200 OK",
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

/// Helper: Spawn the full Axum test backend server on an ephemeral port.
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
// 1. CONFIGURATION & SECRET REDACTION TESTS
// =========================================================================

#[test]
fn test_gateway_configuration_and_secret_redaction() {
    let mut config = AppConfig::default_local();
    assert!(config.freellmapi_base_url.is_none());
    assert!(config.freellmapi_api_key.is_none());
    assert!(config.omniroute_base_url.is_none());
    assert!(config.omniroute_api_key.is_none());

    // Configure test values
    config.freellmapi_base_url = Some("http://127.0.0.1:3001/v1".to_string());
    config.freellmapi_api_key = Some("secret-freellm-token-12345".to_string());
    config.omniroute_base_url = Some("http://127.0.0.1:20128/v1".to_string());
    config.omniroute_api_key = Some("secret-omniroute-token-67890".to_string());

    // Verify debug representation redacts API keys
    let debug_output = format!("{:?}", config);
    assert!(!debug_output.contains("secret-freellm-token-12345"));
    assert!(!debug_output.contains("secret-omniroute-token-67890"));
    assert!(debug_output.contains(r#"freellmapi_base_url: Some("http://127.0.0.1:3001/v1")"#));
    assert!(debug_output.contains(r#"omniroute_base_url: Some("http://127.0.0.1:20128/v1")"#));
}

// =========================================================================
// 2. SSRF POLICY DESTINATION AUTHORIZATION TESTS
// =========================================================================

#[test]
fn test_gateway_ssrf_destination_authorization() {
    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some("http://127.0.0.1:3001/v1".to_string());
    config.freellmapi_api_key = Some("token".to_string());
    config.omniroute_base_url = Some("http://127.0.0.1:20128/v1".to_string());
    config.omniroute_api_key = Some("token".to_string());

    let policy = config.outbound_destination_policy();

    // Configured local gateway endpoints must be permitted
    let fl_dest = policy.validate_url("http://127.0.0.1:3001/v1/chat/completions");
    assert!(fl_dest.is_ok(), "FreeLLMAPI on port 3001 must be allowed");
    assert!(fl_dest.unwrap().is_local);

    let or_dest = policy.validate_url("http://127.0.0.1:20128/v1/chat/completions");
    assert!(or_dest.is_ok(), "OmniRoute on port 20128 must be allowed");
    assert!(or_dest.unwrap().is_local);

    // Arbitrary unconfigured loopback ports must be strictly rejected
    let bad_dest = policy.validate_url("http://127.0.0.1:9999/v1/chat/completions");
    assert!(bad_dest.is_err(), "Port 9999 must be rejected by SSRF");

    // Cloud metadata must be strictly rejected
    let meta_dest = policy.validate_url("http://169.254.169.254/latest/meta-data/");
    assert!(meta_dest.is_err(), "Metadata service must be rejected");
}

// =========================================================================
// 3. WIRE PROTOCOL & MODEL NAMESPACING TESTS
// =========================================================================

#[tokio::test]
async fn test_freellmapi_wire_protocol_and_model_mapping() {
    let (gateway_url, _h_gw) = spawn_mock_gateway(|path, req| {
        assert_eq!(path, "/chat/completions");
        let lower = req.to_lowercase();
        assert!(lower.contains("authorization: bearer test-freellm-key"));

        let body_str = req.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let parsed: Value = serde_json::from_str(body_str).unwrap_or_default();
        let requested_model = parsed["model"].as_str().unwrap_or("");

        // Wire mapping must resolve freellmapi-fast to gemini-2.5-flash
        assert_eq!(requested_model, "gemini-2.5-flash");

        let resp = json!({
            "id": "chatcmpl-freellm-1",
            "model": "gemini-2.5-flash",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "FreeLLMAPI response received"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 6,
                "total_tokens": 18
            }
        });

        (
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&resp).unwrap(),
        )
    })
    .await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(gateway_url);
    config.freellmapi_api_key = Some("test-freellm-key".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;

    let client = reqwest::Client::new();
    let req_body = json!({
        "routing_mode": "manual",
        "requested_model_id": "freellmapi-fast",
        "messages": [
            {"role": "user", "content": "Ping FreeLLMAPI"}
        ]
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", backend_url))
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body_json: Value = resp.json().await.unwrap();
    assert_eq!(body_json["provider_used"], "freellmapi-api");
    assert_eq!(body_json["model_used"], "freellmapi-fast");
    assert_eq!(
        body_json["message"]["content"],
        "FreeLLMAPI response received"
    );
}

#[tokio::test]
async fn test_omniroute_wire_protocol_and_model_mapping() {
    let (gateway_url, _h_gw) = spawn_mock_gateway(|path, req| {
        assert_eq!(path, "/chat/completions");
        let lower = req.to_lowercase();
        assert!(lower.contains("authorization: bearer test-omniroute-key"));

        let body_str = req.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let parsed: Value = serde_json::from_str(body_str).unwrap_or_default();
        let requested_model = parsed["model"].as_str().unwrap_or("");

        // Wire mapping must resolve omniroute-coding to deepseek-coder
        assert_eq!(requested_model, "deepseek-coder");

        let resp = json!({
            "id": "chatcmpl-omni-1",
            "model": "deepseek-coder",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "def hello(): return 'OmniRoute'"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 15,
                "completion_tokens": 8,
                "total_tokens": 23
            }
        });

        (
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&resp).unwrap(),
        )
    })
    .await;

    let mut config = AppConfig::default_local();
    config.omniroute_base_url = Some(gateway_url);
    config.omniroute_api_key = Some("test-omniroute-key".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;

    let client = reqwest::Client::new();
    let req_body = json!({
        "routing_mode": "manual",
        "requested_model_id": "omniroute-coding",
        "messages": [
            {"role": "user", "content": "Write hello in Python"}
        ]
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", backend_url))
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body_json: Value = resp.json().await.unwrap();
    assert_eq!(body_json["provider_used"], "omniroute-api");
    assert_eq!(body_json["model_used"], "omniroute-coding");
    assert_eq!(
        body_json["message"]["content"],
        "def hello(): return 'OmniRoute'"
    );
}

// =========================================================================
// 4. MULTI-MODEL SELECTION TESTS (>= 3 MODELS PER GATEWAY)
// =========================================================================

#[tokio::test]
async fn test_freellmapi_multi_model_selection() {
    let (gateway_url, _h_gw) = spawn_mock_gateway(|_path, req| {
        let body_str = req.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let parsed: Value = serde_json::from_str(body_str).unwrap_or_default();
        let upstream_model = parsed["model"].as_str().unwrap_or("unknown");

        let content = format!("Response from model {upstream_model}");
        let resp = json!({
            "id": format!("chatcmpl-{upstream_model}"),
            "model": upstream_model,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": content
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        (
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&resp).unwrap(),
        )
    })
    .await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(gateway_url);
    config.freellmapi_api_key = Some("test-key".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Test Model 1: freellmapi-fast (gemini-2.5-flash)
    // Test Model 2: freellmapi-balanced (llama-3.3-70b)
    // Test Model 3: freellmapi-reasoner (deepseek-r1)
    // Test Model 4: freellmapi/gemini-2.5-flash (namespaced prefix)
    let models_to_test = vec![
        ("freellmapi-fast", "gemini-2.5-flash"),
        ("freellmapi-balanced", "llama-3.3-70b"),
        ("freellmapi-reasoner", "deepseek-r1"),
        ("freellmapi/gemini-2.5-flash", "gemini-2.5-flash"),
    ];

    for (catalog_id, expected_upstream) in models_to_test {
        let req_body = json!({
            "routing_mode": "manual",
            "requested_model_id": catalog_id,
            "messages": [{"role": "user", "content": "Test prompt"}]
        });

        let resp = client
            .post(format!("{}/api/v1/chat/completions", backend_url))
            .json(&req_body)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), reqwest::StatusCode::OK);

        let body_json: Value = resp.json().await.unwrap();
        assert_eq!(body_json["provider_used"], "freellmapi-api");
        assert_eq!(body_json["model_used"], catalog_id);
        assert!(
            body_json["message"]["content"]
                .as_str()
                .unwrap()
                .contains(expected_upstream)
        );
    }
}

#[tokio::test]
async fn test_omniroute_multi_model_selection() {
    let (gateway_url, _h_gw) = spawn_mock_gateway(|_path, req| {
        let body_str = req.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let parsed: Value = serde_json::from_str(body_str).unwrap_or_default();
        let upstream_model = parsed["model"].as_str().unwrap_or("unknown");

        let content = format!("OmniRoute processed by {upstream_model}");
        let resp = json!({
            "id": format!("chatcmpl-{upstream_model}"),
            "model": upstream_model,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": content
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        (
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&resp).unwrap(),
        )
    })
    .await;

    let mut config = AppConfig::default_local();
    config.omniroute_base_url = Some(gateway_url);
    config.omniroute_api_key = Some("test-key".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Test Model 1: omniroute-fast (gpt-4o-mini)
    // Test Model 2: omniroute-coding (deepseek-coder)
    // Test Model 3: omniroute-reasoner (claude-3-5-haiku)
    // Test Model 4: omniroute/gpt-4o-mini (namespaced)
    let models_to_test = vec![
        ("omniroute-fast", "gpt-4o-mini"),
        ("omniroute-coding", "deepseek-coder"),
        ("omniroute-reasoner", "claude-3-5-haiku"),
        ("omniroute/gpt-4o-mini", "gpt-4o-mini"),
    ];

    for (catalog_id, expected_upstream) in models_to_test {
        let req_body = json!({
            "routing_mode": "manual",
            "requested_model_id": catalog_id,
            "messages": [{"role": "user", "content": "Test prompt"}]
        });

        let resp = client
            .post(format!("{}/api/v1/chat/completions", backend_url))
            .json(&req_body)
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), reqwest::StatusCode::OK);

        let body_json: Value = resp.json().await.unwrap();
        assert_eq!(body_json["provider_used"], "omniroute-api");
        assert_eq!(body_json["model_used"], catalog_id);
        assert!(
            body_json["message"]["content"]
                .as_str()
                .unwrap()
                .contains(expected_upstream)
        );
    }
}

// =========================================================================
// 5. SERVER-SENT EVENTS (SSE) STREAMING TESTS
// =========================================================================

#[tokio::test]
async fn test_freellmapi_and_omniroute_streaming() {
    let (fl_url, _h1) = spawn_mock_gateway(|_path, _req| {
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello from FreeLLMAPI!\"}}]}\n\ndata: [DONE]\n\n";
        (200, vec![("Content-Type", "text/event-stream")], sse_body.as_bytes().to_vec())
    }).await;

    let (omni_url, _h2) = spawn_mock_gateway(|_path, _req| {
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello from OmniRoute!\"}}]}\n\ndata: [DONE]\n\n";
        (200, vec![("Content-Type", "text/event-stream")], sse_body.as_bytes().to_vec())
    }).await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(fl_url);
    config.freellmapi_api_key = Some("test-key".to_string());
    config.omniroute_base_url = Some(omni_url);
    config.omniroute_api_key = Some("test-key".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Stream from FreeLLMAPI via /api/v1/chat/stream
    let resp_fl = client
        .post(format!("{}/api/v1/chat/stream", backend_url))
        .json(&json!({
            "routing_mode": "manual",
            "requested_model_id": "freellmapi-fast",
            "messages": [{"role": "user", "content": "Stream FreeLLMAPI"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_fl.status(), reqwest::StatusCode::OK);
    let ct_fl = resp_fl
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct_fl.contains("text/event-stream"));

    let stream_text = resp_fl.text().await.unwrap();
    assert!(stream_text.contains("event: routing"));
    assert!(stream_text.contains("event: chunk"));
    assert!(stream_text.contains("Hello from FreeLLMAPI!"));
    assert!(stream_text.contains("event: done"));

    // 2. Stream from OmniRoute via /api/v1/chat/stream
    let resp_omni = client
        .post(format!("{}/api/v1/chat/stream", backend_url))
        .json(&json!({
            "routing_mode": "manual",
            "requested_model_id": "omniroute-fast",
            "messages": [{"role": "user", "content": "Stream OmniRoute"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_omni.status(), reqwest::StatusCode::OK);
    let ct_omni = resp_omni
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct_omni.contains("text/event-stream"));

    let omni_text = resp_omni.text().await.unwrap();
    assert!(omni_text.contains("event: routing"));
    assert!(omni_text.contains("event: chunk"));
    assert!(omni_text.contains("Hello from OmniRoute!"));
    assert!(omni_text.contains("event: done"));
}

// =========================================================================
// 6. FAILOVER & RESILIENCE TESTS
// =========================================================================

#[tokio::test]
async fn test_gateway_failover_from_freellmapi_to_omniroute() {
    // FreeLLMAPI gateway returns 429 Too Many Requests with Retry-After: 5
    let (fl_url, _h1) = spawn_mock_gateway(|_path, _req| {
        (
            429,
            vec![("Content-Type", "application/json"), ("Retry-After", "5")],
            b"{\"error\":{\"message\":\"Rate limit exceeded\",\"type\":\"rate_limit_error\"}}"
                .to_vec(),
        )
    })
    .await;

    // OmniRoute gateway returns successful response
    let (omni_url, _h2) = spawn_mock_gateway(|_path, req| {
        let body_str = req.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let parsed: Value = serde_json::from_str(body_str).unwrap_or_default();
        let upstream_model = parsed["model"].as_str().unwrap_or("unknown");

        let resp = json!({
            "id": "chatcmpl-fallback-success",
            "model": upstream_model,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Recovered via failover to OmniRoute"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 6,
                "total_tokens": 16
            }
        });

        (
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&resp).unwrap(),
        )
    })
    .await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(fl_url);
    config.freellmapi_api_key = Some("key1".to_string());
    config.omniroute_base_url = Some(omni_url);
    config.omniroute_api_key = Some("key2".to_string());
    config.max_retries = 1;
    config.max_failover_candidates = 3;

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Automatic routing with a prompt
    let resp = client
        .post(format!("{}/api/v1/chat/completions", backend_url))
        .json(&json!({
            "routing_mode": "auto",
            "messages": [{"role": "user", "content": "Help me draft a greeting"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body_json: Value = resp.json().await.unwrap();
    // Verify failover was executed
    assert_eq!(body_json["is_fallback"], true);
    assert_eq!(
        body_json["message"]["content"],
        "Recovered via failover to OmniRoute"
    );
}

#[tokio::test]
async fn test_gateway_non_retryable_auth_error_fails_fast() {
    // Gateway returns 401 Unauthorized
    let (fl_url, _h) = spawn_mock_gateway(|_path, _req| {
        (
            401,
            vec![("Content-Type", "application/json")],
            b"{\"error\":{\"message\":\"Invalid API Key\",\"type\":\"invalid_request_error\"}}"
                .to_vec(),
        )
    })
    .await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(fl_url);
    config.freellmapi_api_key = Some("bad-key".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/v1/chat/completions", backend_url))
        .json(&json!({
            "routing_mode": "manual",
            "requested_model_id": "freellmapi-fast",
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();

    // 401 must fail-fast with HTTP 502 Bad Gateway upstream failure
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);

    let body_json: Value = resp.json().await.unwrap();
    assert_eq!(body_json["error"]["code"], "UPSTREAM_FAILURE");
}

#[tokio::test]
async fn test_manual_model_selection_without_fallback_never_silently_changes() {
    // FreeLLMAPI returns 429
    let (fl_url, _h1) = spawn_mock_gateway(|_path, _req| {
        (
            429,
            vec![("Content-Type", "application/json"), ("Retry-After", "5")],
            b"{\"error\":{\"message\":\"Rate limit exceeded\",\"type\":\"rate_limit_error\"}}"
                .to_vec(),
        )
    })
    .await;

    // OmniRoute returns 200
    let (omni_url, _h2) = spawn_mock_gateway(|_path, _req| {
        let resp = json!({
            "id": "chatcmpl-omni",
            "model": "gpt-4o-mini",
            "choices": [{"message": {"role": "assistant", "content": "OmniRoute response"}, "finish_reason": "stop"}]
        });
        (200, vec![("Content-Type", "application/json")], serde_json::to_vec(&resp).unwrap())
    })
    .await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(fl_url);
    config.omniroute_base_url = Some(omni_url);
    config.max_retries = 0;

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // In manual mode without allow_fallback (default is false), an explicit model must NOT silently change
    let resp = client
        .post(format!("{}/api/v1/chat/completions", backend_url))
        .json(&json!({
            "model": "freellmapi-fast",
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    let body_json: Value = resp.json().await.unwrap();
    assert_eq!(body_json["error"]["code"], "RATE_LIMITED");
}

#[tokio::test]
async fn test_manual_model_selection_with_fallback_enabled() {
    // FreeLLMAPI returns 429
    let (fl_url, _h1) = spawn_mock_gateway(|_path, _req| {
        (
            429,
            vec![("Content-Type", "application/json"), ("Retry-After", "5")],
            b"{\"error\":{\"message\":\"Rate limit exceeded\",\"type\":\"rate_limit_error\"}}"
                .to_vec(),
        )
    })
    .await;

    // OmniRoute returns 200
    let (omni_url, _h2) = spawn_mock_gateway(|_path, _req| {
        let resp = json!({
            "id": "chatcmpl-omni",
            "model": "gpt-4o-mini",
            "choices": [{"message": {"role": "assistant", "content": "Fallback to OmniRoute succeeded"}, "finish_reason": "stop"}]
        });
        (200, vec![("Content-Type", "application/json")], serde_json::to_vec(&resp).unwrap())
    })
    .await;

    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some(fl_url);
    config.omniroute_base_url = Some(omni_url);
    config.max_retries = 0;
    config.max_failover_candidates = 3;

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h_backend) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    // Explicit model with allow_fallback: true should fall back and report the actual model used
    let resp = client
        .post(format!("{}/api/v1/chat/completions", backend_url))
        .json(&json!({
            "model": "freellmapi-fast",
            "allow_fallback": true,
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body_json: Value = resp.json().await.unwrap();
    assert_eq!(body_json["is_fallback"], true);
    assert!(
        body_json["model_used"]
            .as_str()
            .unwrap()
            .starts_with("omniroute")
    );
    assert_eq!(body_json["provider_used"], "omniroute-api");
    assert_eq!(
        body_json["message"]["content"],
        "Fallback to OmniRoute succeeded"
    );
}

#[tokio::test]
async fn test_models_catalog_endpoint_includes_gateway_and_default_model() {
    let mut config = AppConfig::default_local();
    config.freellmapi_base_url = Some("http://127.0.0.1:3001/v1".to_string());
    config.omniroute_base_url = Some("http://127.0.0.1:20128/v1".to_string());
    config.default_model = Some("freellmapi-fast".to_string());

    let state = AppState::new(Arc::new(config));
    let (backend_url, _h) = spawn_test_backend(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/v1/models", backend_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body_json: Value = resp.json().await.unwrap();
    assert_eq!(body_json["default_model"], "freellmapi-fast");

    let models = body_json["models"].as_array().unwrap();
    assert!(!models.is_empty());

    // Verify FreeLLMAPI model has gateway: "freellmapi"
    let fl_model = models
        .iter()
        .find(|m| m["model_id"] == "freellmapi-fast")
        .unwrap();
    assert_eq!(fl_model["gateway"], "freellmapi");
    assert_eq!(fl_model["provider_id"], "freellmapi-api");

    // Verify OmniRoute model has gateway: "omniroute"
    let omni_model = models
        .iter()
        .find(|m| m["model_id"] == "omniroute-fast")
        .unwrap();
    assert_eq!(omni_model["gateway"], "omniroute");
    assert_eq!(omni_model["provider_id"], "omniroute-api");
}
