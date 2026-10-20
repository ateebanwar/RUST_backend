//! Integration tests for Phase 10: Token Optimization & Performance Baseline.
//!
//! Validates:
//! 1. Fast deterministic token estimation across English, code, Roman Hindi, and multilingual Unicode.
//! 2. ChatML framing token calculation including per-message overhead and assistant primer.
//! 3. Consecutive identical system prompt deduplication without altering conversational turn order.
//! 4. Whitespace compaction preserving internal indentation (essential for code blocks).
//! 5. Pre-dispatch context window bounds validation in manual routing mode.
//! 6. Capacity-aware candidate filtering in automatic routing mode based on estimated prompt tokens.
//! 7. Thread-safe LRU token cache concurrency and eviction under parallel load.
//! 8. End-to-end HTTP chat completion and SSE streaming dispatch with prompt optimization.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::create_app;
use ai_backend::errors::AppError;
use ai_backend::inference::ChatCompletionRequest;
use ai_backend::inference::tokens::TokenEstimator;
use ai_backend::models::{ChatMessage, ModelMetadata, ProviderMetadata};
use ai_backend::providers::{MockProviderAdapter, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use futures_util::StreamExt;
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Helper to spin up an in-process backend with mock provider for token optimization tests.
async fn setup_test_backend() -> (String, tokio::task::JoinHandle<()>, Arc<AppState>) {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_token_opt_key".to_string());
    let config = Arc::new(config);

    let registry = Arc::new(ModelRegistry::empty());
    let providers = Arc::new(ProviderRegistry::new());

    registry.register_provider(ProviderMetadata {
        provider_id: "mock-provider".to_string(),
        display_name: "Mock AI Provider".to_string(),
        base_url: "https://mock.ai.internal/v1".to_string(),
        status: "operational".to_string(),
        enabled: true,
        models_count: 2,
        rate_limit_tier: "free-tier".to_string(),
    });

    let small_model = ModelMetadata {
        model_id: "small-ctx-model".to_string(),
        display_name: "Small Context Model (4k)".to_string(),
        provider_id: "mock-provider".to_string(),
        supported_modalities: vec!["text".to_string(), "code".to_string()],
        context_window: 4096,
        max_output_tokens: 1024,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string(), "coding".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    let large_model = ModelMetadata {
        model_id: "large-ctx-model".to_string(),
        display_name: "Large Context Model (64k)".to_string(),
        provider_id: "mock-provider".to_string(),
        supported_modalities: vec!["text".to_string(), "code".to_string()],
        context_window: 8192,
        max_output_tokens: 2048,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: "balanced".to_string(),
        cost_tier: "premium".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string(), "coding".to_string()],
        last_health_check: "2026-10-20T00:00:00Z".to_string(),
    };

    registry.register_model(small_model.clone()).unwrap();
    registry.register_model(large_model.clone()).unwrap();

    let mock = Arc::new(
        MockProviderAdapter::new("mock-provider", "Mock AI Provider")
            .with_models(vec![small_model, large_model]),
    );

    providers.register(mock).unwrap();
    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state.clone());

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{}", addr), handle, Arc::new(state))
}

#[test]
fn test_token_estimator_text_accuracy() {
    assert_eq!(TokenEstimator::estimate_text(""), 0);
    assert_eq!(TokenEstimator::estimate_text("   \n\t  "), 0);

    // Short English phrases
    assert_eq!(TokenEstimator::estimate_text("Hello world"), 2);
    assert_eq!(TokenEstimator::estimate_text("Rust is blazing fast!"), 5);

    // Code snippets with punctuation and keywords
    let code = "fn compute(x: i32) -> i32 { x * 2 + 1 }";
    let code_tokens = TokenEstimator::estimate_text(code);
    assert!((10..=18).contains(&code_tokens));

    // Roman Hindi and Hinglish
    let hinglish = "bhai code review kar do aur performance optimize karo";
    let hinglish_tokens = TokenEstimator::estimate_text(hinglish);
    assert!((8..=14).contains(&hinglish_tokens));

    // Multilingual Devanagari Hindi and Chinese
    let hindi = "नमस्ते दुनिया";
    let hindi_tokens = TokenEstimator::estimate_text(hindi);
    assert!(hindi_tokens >= 2);

    let chinese = "快速排序算法实现";
    let chinese_tokens = TokenEstimator::estimate_text(chinese);
    assert!(chinese_tokens >= 4);
}

#[test]
fn test_token_estimator_chat_framing() {
    let msgs = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "You are a senior Rust systems architect.".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Explain lock-free data structures.".to_string(),
        },
    ];

    let estimated = TokenEstimator::estimate_chat(&msgs);
    assert!((15..=30).contains(&estimated));
}

#[test]
fn test_prompt_compaction_deduplicates_consecutive_system_messages() {
    let msgs = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "You are an AI coding assistant.".to_string(),
        },
        ChatMessage {
            role: "system".to_string(),
            content: "  You are an AI coding assistant.  ".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Write a binary search function.".to_string(),
        },
    ];

    let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
    assert_eq!(optimized.len(), 2);
    assert_eq!(optimized[0].role, "system");
    assert_eq!(optimized[0].content, "You are an AI coding assistant.");
    assert_eq!(optimized[1].role, "user");
    assert_eq!(metrics.duplicate_system_messages_removed, 1);
    assert!(metrics.tokens_saved > 0);
    assert!(metrics.optimized_estimated_tokens < metrics.original_estimated_tokens);
}

#[test]
fn test_prompt_compaction_preserves_distinct_system_messages() {
    let msgs = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "System instruction A: Tone should be professional.".to_string(),
        },
        ChatMessage {
            role: "system".to_string(),
            content: "System instruction B: Output strictly valid JSON.".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Generate schema.".to_string(),
        },
    ];

    let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
    assert_eq!(optimized.len(), 3);
    assert_eq!(metrics.duplicate_system_messages_removed, 0);
    assert_eq!(metrics.tokens_saved, 0);
}

#[test]
fn test_prompt_compaction_trims_whitespace_preserves_code_indentation() {
    let raw_code = "\n\n\n    def factorial(n):\n        if n <= 1:\n            return 1\n        return n * factorial(n - 1)\n\n\n    ";
    let msgs = vec![ChatMessage {
        role: "user".to_string(),
        content: raw_code.to_string(),
    }];

    let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
    assert_eq!(optimized.len(), 1);
    assert_eq!(
        optimized[0].content,
        "def factorial(n):\n        if n <= 1:\n            return 1\n        return n * factorial(n - 1)"
    );
    assert!(metrics.tokens_saved > 0);
}

#[tokio::test]
async fn test_context_window_enforcement_in_manual_routing() {
    let (_, _h, state) = setup_test_backend().await;

    // Create an oversized prompt exceeding small-ctx-model (4,096 tokens)
    let big_prompt = "word ".repeat(5_000);
    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: big_prompt,
        }],
        routing_mode: "manual".to_string(),
        requested_model_id: Some("small-ctx-model".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
    };

    let result = state.inference.resolve_candidates(&req);
    assert!(result.is_err());
    match result.unwrap_err() {
        AppError::InvalidRequest(msg) => {
            assert!(
                msg.contains("exceed model 'small-ctx-model' context window (4096)"),
                "Expected context window limit error, got: '{msg}'"
            );
        }
        other => panic!("Expected AppError::InvalidRequest, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_capacity_aware_filtering_in_auto_routing() {
    let (_, _h, state) = setup_test_backend().await;

    // Case 1: Simple greeting prompt -> Low complexity general conversation; auto routing selects small-ctx-model (fast/free)
    let normal_prompt = "Hello, how are you doing today?";
    let req_normal = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: normal_prompt.to_string(),
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
    };

    let res_normal = state.inference.resolve_candidates(&req_normal).unwrap();
    assert_eq!(res_normal.selected_model_id, "small-ctx-model");

    // Case 2: Large prompt (~5,200 tokens) exceeding small-ctx-model (4096)
    // Automatic routing must automatically disqualify small-ctx-model and select large-ctx-model (8192)!
    let large_prompt = "algorithm analysis ".repeat(1_300);
    let req_large = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: large_prompt,
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
    };

    let res_large = state.inference.resolve_candidates(&req_large).unwrap();
    assert_eq!(
        res_large.selected_model_id, "large-ctx-model",
        "Auto routing must pick large-ctx-model because small-ctx-model was disqualified by context window"
    );

    // Case 3: Oversized prompt (~10,000 tokens) exceeding all operational models (4096 and 8192) but within 100k char limit
    let huge_prompt = "data ".repeat(10_000);
    let req_huge = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: huge_prompt,
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
    };

    let res_huge = state.inference.resolve_candidates(&req_huge);
    assert!(res_huge.is_err());
    match res_huge.unwrap_err() {
        AppError::InvalidRequest(msg) => {
            assert!(
                msg.contains("exceed context window of all available operational models"),
                "Expected all models exceeded error, got: '{msg}'"
            );
        }
        other => panic!("Expected AppError::InvalidRequest, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_token_cache_concurrency_and_eviction() {
    let mut handles = Vec::new();

    for thread_idx in 0..10 {
        let handle = tokio::spawn(async move {
            for i in 0..100 {
                let prompt = format!(
                    "Concurrent prompt testing thread {} iteration {}",
                    thread_idx, i
                );
                let first = TokenEstimator::estimate_text(&prompt);
                let second = TokenEstimator::estimate_text(&prompt);
                assert_eq!(first, second);
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
async fn test_e2e_buffered_chat_with_prompt_compaction() {
    let (base_url, _h, _state) = setup_test_backend().await;
    let client = reqwest::Client::new();

    let body = json!({
        "messages": [
            { "role": "system", "content": "You are a helpful coding assistant." },
            { "role": "system", "content": "You are a helpful coding assistant." },
            { "role": "user", "content": "    Hello, how are you doing today?    " }
        ],
        "routing_mode": "auto"
    });

    let resp = client
        .post(format!("{}/api/v1/chat/completions", base_url))
        .header("x-api-key", "test_token_opt_key")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json_resp: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json_resp["model_used"], "small-ctx-model");
    assert_eq!(json_resp["provider_used"], "mock-provider");
    assert_eq!(json_resp["message"]["role"], "assistant");
}

#[tokio::test]
async fn test_e2e_streaming_chat_with_prompt_compaction() {
    let (base_url, _h, _state) = setup_test_backend().await;
    let client = reqwest::Client::new();

    let body = json!({
        "messages": [
            { "role": "system", "content": "You are a helpful streaming assistant." },
            { "role": "system", "content": "You are a helpful streaming assistant." },
            { "role": "user", "content": "Stream response." }
        ],
        "routing_mode": "auto",
        "stream": true
    });

    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url))
        .header("x-api-key", "test_token_opt_key")
        .header("accept", "text/event-stream")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let mut stream = resp.bytes_stream();
    let mut received_bytes = Vec::new();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.unwrap();
        received_bytes.extend_from_slice(&chunk);
    }

    let stream_text = String::from_utf8_lossy(&received_bytes);
    assert!(stream_text.contains("event: done"));
    assert!(stream_text.contains("\"status\":\"completed\""));
}
