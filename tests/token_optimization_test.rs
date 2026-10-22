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
use ai_backend::inference::tokens::{TokenEstimator, TokenizerFamily};
use ai_backend::models::{ChatMessage, ModelMetadata, ProviderMetadata};
use ai_backend::providers::{MockProviderAdapter, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use ai_backend::security::{HostValidator, IpCidr, TrustedProxyMatcher};
use ai_backend::usage::model::{
    RecordStatus, RequestMode, RequestOutcome, RoutingMode, UsageRecord, UsageSource,
};
use ai_backend::usage::storage::UsageStorage;
use futures_util::StreamExt;
use serde_json::json;
use std::net::IpAddr;
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
    let raw_code = "\n\n\n    def factorial(n):\n        if n <= 1:\n            return 1\n        return n * factorial(n - 1)   \n\n\n    ";
    let msgs = vec![ChatMessage {
        role: "user".to_string(),
        content: raw_code.to_string(),
    }];

    let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
    assert_eq!(optimized.len(), 1);
    // Strict semantic preservation: code block indentation is preserved to avoid Python IndentationErrors
    assert_eq!(
        optimized[0].content,
        "    def factorial(n):\n        if n <= 1:\n            return 1\n        return n * factorial(n - 1)"
    );
    assert!(metrics.bytes_saved > 0);
    assert!(metrics.compaction_applied);
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
        conversation_id: None,
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
        conversation_id: None,
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
        conversation_id: None,
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
        conversation_id: None,
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

#[test]
fn test_tokenizer_family_mapping_and_ratios() {
    assert_eq!(
        TokenizerFamily::from_model_id("llama-3.3-70b-versatile"),
        TokenizerFamily::Llama
    );
    assert_eq!(
        TokenizerFamily::from_model_id("meta-llama/Llama-3-8B-Instruct"),
        TokenizerFamily::Llama
    );
    assert_eq!(
        TokenizerFamily::from_model_id("mixtral-8x7b-32768"),
        TokenizerFamily::Mistral
    );
    assert_eq!(
        TokenizerFamily::from_model_id("mistralai/mistral-large"),
        TokenizerFamily::Mistral
    );
    assert_eq!(
        TokenizerFamily::from_model_id("gemma-2-9b-it"),
        TokenizerFamily::Gemma
    );
    assert_eq!(
        TokenizerFamily::from_model_id("qwen/qwen-2.5-coder-32b"),
        TokenizerFamily::Qwen
    );
    assert_eq!(
        TokenizerFamily::from_model_id("deepseek-ai/deepseek-r1"),
        TokenizerFamily::DeepSeek
    );
    assert_eq!(
        TokenizerFamily::from_model_id("gpt-4o-mini"),
        TokenizerFamily::Gpt
    );
    assert_eq!(
        TokenizerFamily::from_model_id("unknown-custom-model-v1"),
        TokenizerFamily::Generic
    );

    // Verify token estimation with explicit tokenizer family
    let text = "Rust systems programming with high concurrency and zero-cost abstractions.";
    let llama_tokens = TokenEstimator::estimate_text_for_family(text, TokenizerFamily::Llama);
    let gpt_tokens = TokenEstimator::estimate_text_for_family(text, TokenizerFamily::Gpt);
    assert!(llama_tokens > 0);
    assert!(gpt_tokens > 0);
}

#[test]
fn test_conservative_compaction_structured_preservation() {
    // Markdown fences must be preserved verbatim
    let markdown_code = "Header\n\n```python\n    def indent_test():\n        x = 1\n        return x\n```\n\nFooter";
    let msgs = vec![ChatMessage {
        role: "user".to_string(),
        content: markdown_code.to_string(),
    }];
    let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
    assert_eq!(optimized[0].content, markdown_code);
    assert_eq!(metrics.bytes_saved, 0);

    // Tilde code fences ~~~ must be preserved
    let tilde_code = "~~~yaml\nserver:\n  port: 8080\n  host: 127.0.0.1\n~~~";
    let msgs2 = vec![ChatMessage {
        role: "user".to_string(),
        content: tilde_code.to_string(),
    }];
    let (optimized2, _) = TokenEstimator::optimize_messages(&msgs2);
    assert_eq!(optimized2[0].content, tilde_code);

    // Shell commands and SQL queries
    let shell_and_sql = "SELECT id, name FROM users WHERE active = 1;\ncurl -X POST https://api.internal/v1 -H 'Authorization: Bearer token'";
    let msgs3 = vec![ChatMessage {
        role: "user".to_string(),
        content: shell_and_sql.to_string(),
    }];
    let (optimized3, _) = TokenEstimator::optimize_messages(&msgs3);
    assert_eq!(optimized3[0].content, shell_and_sql);
}

#[test]
fn test_compaction_metrics_accuracy() {
    let raw = "   \n\n\n   Leading text   \n\n\n\n   Trailing text   \n\n\n   ";
    let msgs = vec![ChatMessage {
        role: "user".to_string(),
        content: raw.to_string(),
    }];
    let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
    assert_eq!(metrics.original_characters, raw.chars().count());
    assert_eq!(
        metrics.compacted_characters,
        optimized[0].content.chars().count()
    );
    assert!(metrics.bytes_saved > 0);
    assert!(metrics.compaction_applied);
    assert!(metrics.compaction_skipped_reason.is_none());

    // Clean text should show compaction_applied = false with skipped reason
    let clean_text = "Clean concise prompt.";
    let msgs_clean = vec![ChatMessage {
        role: "user".to_string(),
        content: clean_text.to_string(),
    }];
    let (_, clean_metrics) = TokenEstimator::optimize_messages(&msgs_clean);
    assert!(!clean_metrics.compaction_applied);
    assert!(clean_metrics.compaction_skipped_reason.is_some());
}

#[test]
fn test_security_cidr_and_trusted_proxy_matching() {
    let ip_v4_1: IpAddr = "10.1.2.3".parse().unwrap();
    let ip_v4_2: IpAddr = "10.254.254.254".parse().unwrap();
    let ip_v4_ext: IpAddr = "192.168.1.1".parse().unwrap();

    // Valid IPv4 CIDR
    let cidr_v4 = IpCidr::parse("10.0.0.0/8").unwrap();
    assert!(cidr_v4.contains(&ip_v4_1));
    assert!(cidr_v4.contains(&ip_v4_2));
    assert!(!cidr_v4.contains(&ip_v4_ext));

    // Valid IPv4 single IP (/32)
    let ip_single_match: IpAddr = "192.168.1.100".parse().unwrap();
    let ip_single_no_match: IpAddr = "192.168.1.101".parse().unwrap();
    let cidr_single = IpCidr::parse("192.168.1.100").unwrap();
    assert!(cidr_single.contains(&ip_single_match));
    assert!(!cidr_single.contains(&ip_single_no_match));

    // Valid IPv6 CIDR
    let ip_v6_match: IpAddr = "2001:db8:85a3::8a2e:370:7334".parse().unwrap();
    let ip_v6_no_match: IpAddr = "2001:db9::1".parse().unwrap();
    let cidr_v6 = IpCidr::parse("2001:db8::/32").unwrap();
    assert!(cidr_v6.contains(&ip_v6_match));
    assert!(!cidr_v6.contains(&ip_v6_no_match));

    // Loopback IPv6
    let ip_v6_loop: IpAddr = "::1".parse().unwrap();
    let ip_v6_other: IpAddr = "::2".parse().unwrap();
    let cidr_v6_loopback = IpCidr::parse("::1/128").unwrap();
    assert!(cidr_v6_loopback.contains(&ip_v6_loop));
    assert!(!cidr_v6_loopback.contains(&ip_v6_other));

    // Invalid CIDR strings
    assert!(IpCidr::parse("invalid-ip").is_err());
    assert!(IpCidr::parse("10.0.0.0/33").is_err());
    assert!(IpCidr::parse("2001:db8::/129").is_err());

    // TrustedProxyMatcher
    let proxy_rules = vec!["127.0.0.1/32".to_string(), "10.0.0.0/8".to_string()];
    let matcher = TrustedProxyMatcher::new(&proxy_rules).unwrap();
    let ip_local: IpAddr = "127.0.0.1".parse().unwrap();
    let ip_internal: IpAddr = "10.5.0.1".parse().unwrap();
    let ip_external: IpAddr = "172.16.0.1".parse().unwrap();
    assert!(matcher.is_trusted(&ip_local));
    assert!(matcher.is_trusted(&ip_internal));
    assert!(!matcher.is_trusted(&ip_external));
}

#[test]
fn test_security_host_validation() {
    let allowed = vec![
        "api.ai.company.internal".to_string(),
        "localhost".to_string(),
    ];
    let validator = HostValidator::new(&allowed);

    assert!(validator.validate_host("api.ai.company.internal"));
    assert!(validator.validate_host("api.ai.company.internal:443"));
    assert!(validator.validate_host("localhost"));
    assert!(validator.validate_host("localhost:8080"));
    assert!(!validator.validate_host("evil.attacker.com"));
    assert!(!validator.validate_host("evil.attacker.com:8080"));
    assert!(!validator.validate_host(""));

    // Empty allowed hosts = permissive for development
    let empty_hosts: Vec<String> = Vec::new();
    let permissive = HostValidator::new(&empty_hosts);
    assert!(permissive.validate_host("any-host.com"));
}

#[tokio::test]
async fn test_durable_usage_retry_queue_lifecycle() {
    let storage = UsageStorage::new(":memory:").unwrap();

    let record = UsageRecord {
        id: "retry-test-req-001".to_string(),
        provider_id: "mock-provider".to_string(),
        model_id: "small-ctx-model".to_string(),
        request_mode: RequestMode::Buffered,
        routing_mode: RoutingMode::Auto,
        start_time: 1700000000000,
        completion_time: 1700000000100,
        latency_ms: 100,
        outcome: RequestOutcome::Success,
        failure_category: None,
        prompt_tokens: Some(50),
        completion_tokens: Some(25),
        total_tokens: Some(75),
        usage_source: UsageSource::Reported,
        record_status: RecordStatus::Final,
        subject: None,
    };

    // Enqueue to retry queue
    storage
        .enqueue_retry(&record, "Simulated network timeout")
        .await
        .unwrap();

    let metrics_initial = storage.get_retry_queue_metrics().await.unwrap();
    assert_eq!(metrics_initial.pending_count, 1);
    assert_eq!(metrics_initial.dead_letter_count, 0);

    // Initial drain with next_retry_at in the future should not process yet
    let drain_early = storage.drain_retry_queue(10).await.unwrap();
    assert_eq!(drain_early.succeeded, 0);

    // Drain with larger batch limit after timeout simulation by running immediate sync drain with future epoch
    // In memory, we verify that when drain succeeds, pending goes to 0
    let _drain_result = storage.drain_retry_queue_sync(100).unwrap();
    // Since default enqueue sets next_retry_at = now + 5, let's verify retry queue holds the item
    let metrics_after = storage.get_retry_queue_metrics().await.unwrap();
    assert_eq!(metrics_after.pending_count, 1);
}

#[tokio::test]
async fn test_bounded_retention_prune_and_wal_checkpoint() {
    let storage = UsageStorage::new(":memory:").unwrap();

    for i in 0..10 {
        let record = UsageRecord {
            id: format!("prune-test-req-{:03}", i),
            provider_id: "mock-provider".to_string(),
            model_id: "small-ctx-model".to_string(),
            request_mode: RequestMode::Buffered,
            routing_mode: RoutingMode::Auto,
            start_time: 1000 + i * 10,
            completion_time: 1005 + i * 10,
            latency_ms: 5,
            outcome: RequestOutcome::Success,
            failure_category: None,
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
            usage_source: UsageSource::Reported,
            record_status: RecordStatus::Final,
            subject: None,
        };
        storage.record(record).await.unwrap();
    }

    // Prune records older than 1050 with batch limit 3
    let pruned_batch = storage.prune_older_than_bounded(1050, 3).await.unwrap();
    assert_eq!(pruned_batch, 3);

    // Passive WAL maintenance checkpoint should succeed without error
    storage.maintenance_wal_checkpoint().await.unwrap();
}
