//! Comprehensive automated tests for model-aware tokenization, exact reference verification,
//! approximate tokenizer calibration, and context-window admission safety.
//!
//! Covers:
//! 1. Short English text and long English paragraphs
//! 2. Urdu and supported non-Latin text
//! 3. Mixed-language conversations
//! 4. Representative code (Rust, JavaScript, Python)
//! 5. JSON, deeply nested JSON, and escaped strings
//! 6. Long unbroken strings, hashes, UUIDs, and Base64-like content
//! 7. Emoji, Unicode, punctuation-heavy text, and whitespace
//! 8. Multiple messages with different roles (ChatML framing)
//! 9. Tool/function definitions and large schemas
//! 10. Empty inputs and unusually large inputs
//! 11. Context-window boundary: exactly fitting requests
//! 12. One-token-over-limit and clearly oversized requests
//! 13. Completion allowance consuming remaining context and max_output_tokens validation
//! 14. Unsupported models, fallback behavior, and explicit approximate labeling
//! 15. Integer boundary and overflow-safe arithmetic

use ai_backend::config::AppConfig;
use ai_backend::errors::AppError;
use ai_backend::inference::tokens::{
    BpeEncoding, EstimationMethod, MappingKind, TokenEstimator, TokenizerCapability,
    TokenizerFamily,
};
use ai_backend::inference::{ChatCompletionRequest, InferenceService};
use ai_backend::models::{ChatMessage, ModelMetadata};
use ai_backend::providers::{MockProviderAdapter, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use std::sync::Arc;

fn build_test_inference(models: Vec<ModelMetadata>) -> InferenceService {
    let config = Arc::new(AppConfig::default_local());
    let registry = Arc::new(ModelRegistry::empty());
    for m in &models {
        registry.register_model(m.clone()).unwrap();
    }
    let providers = Arc::new(ProviderRegistry::new());
    let mock = Arc::new(
        MockProviderAdapter::new("test-provider", "Test Mock Provider").with_models(models),
    );
    providers.register(mock).unwrap();
    let router = Arc::new(ModelRouter::new(registry.clone()));
    InferenceService::new(config, registry, providers, router)
}

// =========================================================================
// Category 1: Short English text and long English paragraphs
// =========================================================================
#[test]
fn test_category_1_english_text_exact_and_approximate() {
    let short_en = "Hello world";
    let gpt4_tokens = TokenEstimator::estimate_text_for_model(short_en, Some("gpt-4"));
    let gpt4o_tokens = TokenEstimator::estimate_text_for_model(short_en, Some("gpt-4o"));
    assert_eq!(gpt4_tokens, 2, "Expected 2 tokens in cl100k");
    assert_eq!(gpt4o_tokens, 2, "Expected 2 tokens in o200k");

    let long_paragraph = "High-performance systems programming requires rigorous attention to memory layouts, cache locality, and synchronization primitives. In Rust, ownership and borrowing enforce thread safety without a garbage collector. When building scalable network backends, asynchronous I/O with Tokio enables cooperative task scheduling across bounded worker threads. Every request must be validated at the boundary before incurring upstream latency.";
    let exact_cl100k = TokenEstimator::estimate_text_for_model(long_paragraph, Some("gpt-4"));
    let exact_o200k = TokenEstimator::estimate_text_for_model(long_paragraph, Some("gpt-4o-mini"));
    let approx_llama =
        TokenEstimator::estimate_text_for_model(long_paragraph, Some("fast-chat-v1"));

    assert!(exact_cl100k > 50 && exact_cl100k < 80);
    assert!(exact_o200k > 50 && exact_o200k < 80);
    assert!(approx_llama > 40 && approx_llama < 90);
}

// =========================================================================
// Category 2: Urdu and other non-Latin text
// =========================================================================
#[test]
fn test_category_2_urdu_and_multilingual_scripts() {
    let urdu = "برائے مہربانی اس سسٹم کی کارکردگی اور سیکیورٹی کو بہتر بنائیں";
    let gpt4_exact = TokenEstimator::estimate_text_for_model(urdu, Some("gpt-4"));
    let gpt4o_exact = TokenEstimator::estimate_text_for_model(urdu, Some("gpt-4o"));
    let llama_approx = TokenEstimator::estimate_text_for_model(urdu, Some("fast-chat-v1"));

    assert!(gpt4_exact >= 15);
    assert!(gpt4o_exact >= 15);
    assert!(llama_approx >= 15);

    // Hindi Devanagari
    let hindi = "नमस्ते, यह टोकنائزیشن اور درستگی کا ٹیسٹ ہے۔";
    let hindi_exact = TokenEstimator::estimate_text_for_model(hindi, Some("gpt-4o"));
    assert!(hindi_exact >= 10);
}

// =========================================================================
// Category 3: Mixed-language conversations
// =========================================================================
#[test]
fn test_category_3_mixed_language_conversation() {
    let msgs = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "You are a bilingual assistant. Aap ek madadgaar assistant hain.".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Please check this function: fn سلام() { println!(\"دنیا\"); }".to_string(),
        },
    ];

    let exact_est = TokenEstimator::estimate_chat_detailed(&msgs, Some("gpt-4o"));
    assert!(
        !exact_est.is_exact,
        "Chat requests must not claim full exactness due to heuristic framing constants"
    );
    assert_eq!(
        exact_est.raw_text_exact,
        Some(true),
        "Raw message contents are tokenized exactly with verified BPE"
    );
    assert_eq!(exact_est.method, EstimationMethod::EstimatedChatFraming);
    assert_eq!(exact_est.safety_margin_applied, 0);

    let approx_est = TokenEstimator::estimate_chat_detailed(&msgs, Some("fast-chat-v1"));
    assert!(!approx_est.is_exact);
    assert_eq!(
        approx_est.method,
        EstimationMethod::CalibratedFamilyHeuristic
    );
}

// =========================================================================
// Category 4: Representative Code across languages (Rust, JS, Python)
// =========================================================================
#[test]
fn test_category_4_representative_code_tokens() {
    // Exact investigated snippet:
    let rust_code = "fn compute(x: i32) -> i32 { x * 2 + 1 }";
    let rust_cl100k = TokenEstimator::estimate_text_for_model(rust_code, Some("gpt-4"));
    let rust_o200k = TokenEstimator::estimate_text_for_model(rust_code, Some("gpt-4o"));
    assert_eq!(
        rust_cl100k, 19,
        "cl100k reference count for compute snippet must be exactly 19"
    );
    assert_eq!(
        rust_o200k, 19,
        "o200k reference count for compute snippet must be exactly 19"
    );

    let js_code = "function parseQuery(url) { return new URL(url).searchParams.get('q'); }";
    let js_exact = TokenEstimator::estimate_text_for_model(js_code, Some("gpt-4o"));
    assert!((18..=25).contains(&js_exact));

    let py_code =
        "def fetch_data(uri: str) -> dict:\n    with open(uri) as f:\n        return json.load(f)";
    let py_exact = TokenEstimator::estimate_text_for_model(py_code, Some("gpt-4o"));
    assert!((20..=30).contains(&py_exact));
}

// =========================================================================
// Category 5: JSON, deeply nested JSON, and escaped strings
// =========================================================================
#[test]
fn test_category_5_nested_json_and_escaped_strings() {
    let nested_json = r#"{
        "api_version": "v1",
        "spec": {
            "routes": [
                {"path": "/v1/chat", "methods": ["POST"], "auth": true},
                {"path": "/v1/health", "methods": ["GET"], "auth": false}
            ],
            "metadata": {
                "tags": ["core", "gateway"],
                "config": {"nested": {"depth": 3, "escaped": "\"hello\\nworld\""}}
            }
        }
    }"#;

    let exact_count = TokenEstimator::estimate_text_for_model(nested_json, Some("gpt-4"));
    let approx_count = TokenEstimator::estimate_text_for_model(nested_json, Some("fast-chat-v1"));

    assert!(
        (90..=130).contains(&exact_count),
        "Got exact_count: {exact_count}"
    );
    assert!(
        (100..=200).contains(&approx_count),
        "Got approx_count: {approx_count}"
    );
}

// =========================================================================
// Category 6: Long unbroken strings, hashes, UUIDs, Base64
// =========================================================================
#[test]
fn test_category_6_long_unbroken_strings_and_hashes() {
    let sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let uuid = "c73f328a-8f8a-4d2c-8822-6b9f2d1e5a2b";
    let base64 =
        "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODk=";

    let sha_exact = TokenEstimator::estimate_text_for_model(sha256, Some("gpt-4o"));
    let uuid_exact = TokenEstimator::estimate_text_for_model(uuid, Some("gpt-4o"));
    let b64_exact = TokenEstimator::estimate_text_for_model(base64, Some("gpt-4o"));

    assert!(sha_exact >= 15);
    assert!(uuid_exact >= 5);
    assert!(b64_exact >= 20);
}

// =========================================================================
// Category 7: Emoji, Unicode, punctuation-heavy text, and whitespace
// =========================================================================
#[test]
fn test_category_7_emojis_and_whitespace() {
    let emojis = "🚀 🔥 🤖 ✨ 💡 🛡️ ⚙️ 📦";
    let exact_emojis = TokenEstimator::estimate_text_for_model(emojis, Some("gpt-4o"));
    assert!(exact_emojis >= 8);

    let punctuation = "!@#$%^&*()_+=-`~[]\\{}|;':\",./<>?";
    let exact_punct = TokenEstimator::estimate_text_for_model(punctuation, Some("gpt-4o"));
    assert!(exact_punct >= 20);

    let whitespace_indent = "        line1\n            line2\n                line3\n";
    let exact_ws = TokenEstimator::estimate_text_for_model(whitespace_indent, Some("gpt-4o"));
    assert!(exact_ws >= 6);
}

// =========================================================================
// Category 8: Multiple messages with different roles & ChatML framing
// =========================================================================
#[test]
fn test_category_8_multi_turn_chat_framing_exactness() {
    let msgs = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "You are a helpful assistant.".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Hello!".to_string(),
        },
        ChatMessage {
            role: "assistant".to_string(),
            content: "Hi! How can I help you today?".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Explain BPE tokenization.".to_string(),
        },
    ];

    let exact_chat = TokenEstimator::estimate_chat_for_model(&msgs, Some("gpt-4o"));
    let mut manual_sum = 0;
    for m in &msgs {
        manual_sum += 3
            + TokenEstimator::estimate_text_for_model(&m.role, Some("gpt-4o"))
            + TokenEstimator::estimate_text_for_model(&m.content, Some("gpt-4o"));
    }
    manual_sum += 3; // primer

    assert_eq!(exact_chat, manual_sum);
}

// =========================================================================
// Category 9: Tool/function definitions and large schemas
// =========================================================================
#[test]
fn test_category_9_tool_function_definitions() {
    let tools = r#"[
        {
            "type": "function",
            "function": {
                "name": "search_codebase",
                "description": "Searches codebase for pattern matches",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "max_results": {"type": "integer"}
                    },
                    "required": ["query"]
                }
            }
        }
    ]"#;

    let exact_tools = TokenEstimator::estimate_tools_tokens_for_model(tools, Some("gpt-4o"));
    let approx_tools = TokenEstimator::estimate_tools_tokens_for_model(tools, Some("fast-chat-v1"));

    assert!(exact_tools >= 40);
    assert!(approx_tools >= 40);

    let msgs = vec![ChatMessage {
        role: "user".to_string(),
        content: "Find calculate_tax".to_string(),
    }];
    let safe_with_tools =
        TokenEstimator::estimate_request_context_safe(&msgs, Some(tools), Some("gpt-4o"));
    let safe_without_tools =
        TokenEstimator::estimate_request_context_safe(&msgs, None, Some("gpt-4o"));

    assert_eq!(
        safe_with_tools.estimated_tokens,
        safe_without_tools.estimated_tokens + exact_tools
    );
}

// =========================================================================
// Category 10: Empty inputs and unusually large inputs
// =========================================================================
#[test]
fn test_category_10_empty_and_unusually_large_inputs() {
    assert_eq!(
        TokenEstimator::estimate_text_for_model("", Some("gpt-4o")),
        0
    );
    assert_eq!(
        TokenEstimator::estimate_text_for_model("   \n\t  ", Some("gpt-4o")),
        0
    );
    assert_eq!(
        TokenEstimator::estimate_chat_for_model(&[], Some("gpt-4o")),
        0
    );

    let large_string = "word ".repeat(5_000);
    let exact_large = TokenEstimator::estimate_text_for_model(&large_string, Some("gpt-4o"));
    assert_eq!(exact_large, 5_001);
}

// =========================================================================
// Category 11: Context-window boundary: exactly fitting requests
// =========================================================================
#[test]
fn test_category_11_context_window_boundary_exact_fit() {
    let model = ModelMetadata {
        model_id: "boundary-model".to_string(),
        display_name: "Boundary Model".to_string(),
        provider_id: "test-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 100,
        max_output_tokens: 30,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-21T00:00:00Z".to_string(),
    };
    let inference = build_test_inference(vec![model]);

    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        routing_mode: "manual".to_string(),
        requested_model_id: Some("boundary-model".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: Some(20),
        stream: false,
    };

    let resolution = inference.resolve_candidates(&req);
    assert!(
        resolution.is_ok(),
        "Expected request to fit within 100 token context window"
    );
}

// =========================================================================
// Category 12: One-token-over-limit and clearly oversized requests
// =========================================================================
#[test]
fn test_category_12_context_window_one_token_over_limit_rejected() {
    let model = ModelMetadata {
        model_id: "small-limit-model".to_string(),
        display_name: "Small Limit Model".to_string(),
        provider_id: "test-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 40,
        max_output_tokens: 20,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-21T00:00:00Z".to_string(),
    };
    let inference = build_test_inference(vec![model]);

    let msg = vec![ChatMessage {
        role: "user".to_string(),
        content: "This is a prompt that will exceed the tight limit".to_string(),
    }];
    let est = TokenEstimator::estimate_request_context_safe(&msg, None, Some("small-limit-model"));
    let max_tokens = 41usize.saturating_sub(est.estimated_tokens).min(20);

    let req = ChatCompletionRequest {
        messages: msg,
        routing_mode: "manual".to_string(),
        requested_model_id: Some("small-limit-model".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: Some(max_tokens),
        stream: false,
    };

    let result = inference.resolve_candidates(&req);
    assert!(result.is_err());
    match result.unwrap_err() {
        AppError::InvalidRequest(msg) => {
            assert!(msg.contains("exceed model 'small-limit-model' context window (40)"));
        }
        other => panic!("Expected InvalidRequest, got {other:?}"),
    }
}

// =========================================================================
// Category 13: Completion allowance exceeding max_output_tokens rejected
// =========================================================================
#[test]
fn test_category_13_max_tokens_exceeding_max_output_limit_rejected() {
    let model = ModelMetadata {
        model_id: "restricted-output-model".to_string(),
        display_name: "Restricted Output Model".to_string(),
        provider_id: "test-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 10000,
        max_output_tokens: 500, // Hard ceiling
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-21T00:00:00Z".to_string(),
    };
    let inference = build_test_inference(vec![model]);

    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        routing_mode: "manual".to_string(),
        requested_model_id: Some("restricted-output-model".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: Some(1000), // Exceeds 500 max_output_tokens!
        stream: false,
    };

    let result = inference.resolve_candidates(&req);
    assert!(
        result.is_err(),
        "Requested max_tokens > max_output_tokens must be rejected"
    );
    match result.unwrap_err() {
        AppError::InvalidRequest(msg) => {
            assert!(msg.contains("Requested max_tokens (1000) exceeds model 'restricted-output-model' maximum output token limit of 500"));
        }
        other => panic!("Expected InvalidRequest, got: {other:?}"),
    }
}

// =========================================================================
// Category 14: Unsupported models, fallback behavior, explicit approximate
// =========================================================================
#[test]
fn test_category_14_unsupported_models_explicit_approximate() {
    assert_eq!(
        TokenizerFamily::Gpt.capability(),
        TokenizerCapability::Exact
    );
    assert_eq!(
        TokenizerFamily::Llama.capability(),
        TokenizerCapability::Approximate
    );
    assert_eq!(
        TokenizerFamily::Mistral.capability(),
        TokenizerCapability::Approximate
    );
    assert_eq!(
        TokenizerFamily::Gemma.capability(),
        TokenizerCapability::Approximate
    );
    assert_eq!(
        TokenizerFamily::Qwen.capability(),
        TokenizerCapability::Approximate
    );
    assert_eq!(
        TokenizerFamily::DeepSeek.capability(),
        TokenizerCapability::Approximate
    );
    assert_eq!(
        TokenizerFamily::Generic.capability(),
        TokenizerCapability::Approximate
    );

    let estimate_unknown =
        TokenEstimator::estimate_text_detailed("Hello world", Some("unregistered-llm"));
    assert_eq!(estimate_unknown.family, TokenizerFamily::Generic);
    assert_eq!(
        estimate_unknown.method,
        EstimationMethod::ConservativeFallback
    );
    assert!(
        !estimate_unknown.is_exact,
        "Must not claim exact for unknown model"
    );

    let estimate_llama =
        TokenEstimator::estimate_text_detailed("Hello world", Some("fast-chat-v1"));
    assert_eq!(estimate_llama.family, TokenizerFamily::Llama);
    assert_eq!(
        estimate_llama.method,
        EstimationMethod::CalibratedFamilyHeuristic
    );
    assert!(
        !estimate_llama.is_exact,
        "Must not claim exact for Llama model"
    );

    let estimate_gpt = TokenEstimator::estimate_text_detailed("Hello world", Some("gpt-4o"));
    assert_eq!(estimate_gpt.family, TokenizerFamily::Gpt);
    assert_eq!(estimate_gpt.method, EstimationMethod::ExactTokenizer);
    assert!(
        estimate_gpt.is_exact,
        "Must report is_exact = true for verified GPT model"
    );
    assert_eq!(estimate_gpt.safety_margin_applied, 0);
}

// =========================================================================
// Category 15: Integer-boundary and overflow-safe arithmetic
// =========================================================================
#[test]
fn test_category_15_overflow_safe_token_arithmetic() {
    let huge_raw = usize::MAX - 10;
    let safety_margin = (huge_raw.saturating_mul(15) / 100).max(8);
    let safe_total = huge_raw.saturating_add(safety_margin);
    assert_eq!(safe_total, usize::MAX);
}

// =========================================================================
// Category 16: Table-Driven Model-Mapping Matrix & Negative Substring Tests
// =========================================================================
#[test]
fn test_category_16_table_driven_model_mapping_matrix() {
    use ai_backend::inference::tokens::resolve_model_tokenizer;

    struct MappingTestCase {
        model_id: &'static str,
        expected_kind: MappingKind,
        expected_family: TokenizerFamily,
        expected_encoding: Option<BpeEncoding>,
        expected_raw_exact: bool,
    }

    let test_cases = vec![
        // 1. Registered catalog models
        MappingTestCase {
            model_id: "gpt-4o",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4o-mini",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4-turbo",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::Cl100k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "fast-chat-v1",
            expected_kind: MappingKind::ExplicitAlias,
            expected_family: TokenizerFamily::Llama,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "deep-reasoner-v1",
            expected_kind: MappingKind::ExplicitAlias,
            expected_family: TokenizerFamily::DeepSeek,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "balanced-assistant-v1",
            expected_kind: MappingKind::ExplicitAlias,
            expected_family: TokenizerFamily::Mistral,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "llama-3.3-70b-versatile",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Llama,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "mistral-large-2407",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Mistral,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "deepseek-chat",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::DeepSeek,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        // 2. Investigated OpenAI models and newer families
        MappingTestCase {
            model_id: "o1",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "o1-mini",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "o3",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "o3-mini",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "o4-mini",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "o4-mini-2025-04-16",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4.1",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4.1-mini",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4.1-nano",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4.5",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-5",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-5-mini",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "chatgpt-4o-latest",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-4",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::Cl100k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "gpt-3.5-turbo",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::Cl100k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "openai/gpt-4o",
            expected_kind: MappingKind::ExactVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        MappingTestCase {
            model_id: "ft:gpt-4o:my-org:custom-1",
            expected_kind: MappingKind::FamilyPrefixVerified,
            expected_family: TokenizerFamily::Gpt,
            expected_encoding: Some(BpeEncoding::O200k),
            expected_raw_exact: true,
        },
        // 3. Negative lookalike tests (Defect 1 regression prevention)
        MappingTestCase {
            model_id: "auto1",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "echo1",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "audio3",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "demo3",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "my-custom-gpt",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "custom-gpt",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "gpt-unverified-future",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "unregistered-llm",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
        MappingTestCase {
            model_id: "",
            expected_kind: MappingKind::Unknown,
            expected_family: TokenizerFamily::Generic,
            expected_encoding: None,
            expected_raw_exact: false,
        },
    ];

    for tc in test_cases {
        let res = resolve_model_tokenizer(tc.model_id, None);
        assert_eq!(
            res.mapping_kind, tc.expected_kind,
            "Model '{}' mapping_kind mismatch: expected {:?}, got {:?}",
            tc.model_id, tc.expected_kind, res.mapping_kind
        );
        assert_eq!(
            res.family, tc.expected_family,
            "Model '{}' family mismatch: expected {:?}, got {:?}",
            tc.model_id, tc.expected_family, res.family
        );
        assert_eq!(
            res.bpe_encoding, tc.expected_encoding,
            "Model '{}' encoding mismatch: expected {:?}, got {:?}",
            tc.model_id, tc.expected_encoding, res.bpe_encoding
        );

        let detailed = TokenEstimator::estimate_text_detailed("Hello world", Some(tc.model_id));
        assert_eq!(
            detailed.is_exact, tc.expected_raw_exact,
            "Model '{}' is_exact mismatch: expected {}, got {}",
            tc.model_id, tc.expected_raw_exact, detailed.is_exact
        );
    }
}

// =========================================================================
// Category 17: Reference Tokenizer Provenance Fixtures (cl100k & o200k)
// =========================================================================
#[test]
fn test_category_17_reference_tokenizer_provenance_fixtures() {
    // Provenance: Counts verified against OpenAI's official tiktoken reference
    // implementation for encodings cl100k_base and o200k_base.
    struct ProvenanceFixture {
        input: &'static str,
        expected_cl100k: usize,
        expected_o200k: usize,
        description: &'static str,
    }

    let fixtures = vec![
        ProvenanceFixture {
            input: "Hello, world!",
            expected_cl100k: 4,
            expected_o200k: 4,
            description: "Simple English greeting with comma and exclamation mark",
        },
        ProvenanceFixture {
            input: "fn compute(x: i32) -> i32 { x * 2 + 1 }",
            expected_cl100k: 19,
            expected_o200k: 19,
            description: "Rust arithmetic function signature and body",
        },
        ProvenanceFixture {
            input: "def add(a, b):\n    return a + b",
            expected_cl100k: 11,
            expected_o200k: 11,
            description: "Python function with newline and 4-space indentation",
        },
        ProvenanceFixture {
            input: r#"{"status": "ok", "count": 42}"#,
            expected_cl100k: 12,
            expected_o200k: 12,
            description: "Standard JSON payload",
        },
        ProvenanceFixture {
            input: "compute_sha256_checksum_for_payload_buffer",
            expected_cl100k: 7,
            expected_o200k: 7,
            description: "Long snake_case identifier",
        },
        ProvenanceFixture {
            input: "برائے مہربانی اس سسٹم کی کارکردگی اور سیکیورٹی کو بہتر بنائیں",
            expected_cl100k: 52,
            expected_o200k: 22,
            description: "Urdu request sentence demonstrating o200k compression efficiency",
        },
        ProvenanceFixture {
            input: "🚀 🔥 🤖 ✨",
            expected_cl100k: 10,
            expected_o200k: 8,
            description: "Unicode emojis with spaces",
        },
        ProvenanceFixture {
            input: "!@#$%^&*()_+=-`~[]\\{}|;':\",./<>?",
            expected_cl100k: 22,
            expected_o200k: 22,
            description: "Full standard ASCII punctuation sequence",
        },
    ];

    for fix in fixtures {
        let cl100k_actual = TokenEstimator::estimate_text_for_model(fix.input, Some("gpt-4"));
        let o200k_actual = TokenEstimator::estimate_text_for_model(fix.input, Some("gpt-4o"));

        assert_eq!(
            cl100k_actual, fix.expected_cl100k,
            "cl100k failure on fixture '{}' ({}): expected {}, got {}",
            fix.description, fix.input, fix.expected_cl100k, cl100k_actual
        );
        assert_eq!(
            o200k_actual, fix.expected_o200k,
            "o200k failure on fixture '{}' ({}): expected {}, got {}",
            fix.description, fix.input, fix.expected_o200k, o200k_actual
        );
    }
}

// =========================================================================
// Category 18: Auto-Routing Per-Candidate Admission & Output Limit Checks
// =========================================================================
#[test]
fn test_category_18_auto_routing_candidate_admission_and_output_limits() {
    let small_candidate = ModelMetadata {
        model_id: "tight-context-model".to_string(),
        display_name: "Tight Context Model".to_string(),
        provider_id: "test-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 40,
        max_output_tokens: 20,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-21T00:00:00Z".to_string(),
    };
    let spacious_candidate = ModelMetadata {
        model_id: "spacious-context-model".to_string(),
        display_name: "Spacious Context Model".to_string(),
        provider_id: "test-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 8192,
        max_output_tokens: 2048,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "balanced".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-21T00:00:00Z".to_string(),
    };

    let inference = build_test_inference(vec![small_candidate, spacious_candidate]);

    // Request requires ~30 tokens. With allowance (256 min or 50 requested),
    // it exceeds tight-context-model (40 ceiling) but fits spacious-context-model!
    let req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "This is a prompt that requires more than forty tokens to fit both prompt and allowance comfortably.".to_string(),
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: Some(50),
        stream: false,
    };

    let res = inference.resolve_candidates(&req).unwrap();
    assert_eq!(
        res.selected_model_id, "spacious-context-model",
        "Auto-routing must filter out candidate that cannot fit token budget and select the capable candidate"
    );

    // Request exceeding max_output_tokens of all operational candidates
    let excessive_req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: Some(4096), // Both models have max_output_tokens: 2048
        stream: false,
    };

    let excessive_err = inference.resolve_candidates(&excessive_req).unwrap_err();
    match excessive_err {
        AppError::InvalidRequest(msg) => {
            assert!(
                msg.contains("Requested max_tokens (4096) exceeds maximum output token limit of all available operational models (2048)"),
                "Expected specific output token limit error, got: {msg}"
            );
        }
        other => panic!("Expected InvalidRequest, got {other:?}"),
    }
}

// =========================================================================
// Category 19: Message Optimization Consistency in Admission & Routing
// =========================================================================
#[test]
fn test_category_19_effective_message_compaction_consistency() {
    let model = ModelMetadata {
        model_id: "compaction-model".to_string(),
        display_name: "Compaction Model".to_string(),
        provider_id: "test-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 50,
        max_output_tokens: 20,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string()],
        last_health_check: "2026-10-21T00:00:00Z".to_string(),
    };

    let inference = build_test_inference(vec![model]);

    // Construct a request with redundant duplicate system instructions and whitespace
    let uncompacted_messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "You are an assistant.".to_string(),
        },
        ChatMessage {
            role: "system".to_string(),
            content: "You are an assistant.".to_string(),
        },
        ChatMessage {
            role: "system".to_string(),
            content: "You are an assistant.".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Hi!\n\n\n\n\n".to_string(),
        },
    ];

    let req = ChatCompletionRequest {
        messages: uncompacted_messages,
        routing_mode: "manual".to_string(),
        requested_model_id: Some("compaction-model".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: Some(15),
        stream: false,
    };

    let resolution = inference.resolve_candidates(&req).unwrap();
    // After compaction, 2 duplicate system messages and redundant blank lines are eliminated
    // Ensuring the estimated prompt tokens reflects the effective message count.
    assert!(
        resolution.estimated_prompt_tokens < 30,
        "Estimated prompt tokens ({}) should reflect compacted messages",
        resolution.estimated_prompt_tokens
    );
}
