//! Comprehensive verification tests for Workstream C: Token Estimation & Context Safety.
//!
//! Verifies:
//! 1. Multilingual test corpus (Plain English, Roman Hinglish, Urdu, Emojis, Code, Tool JSON).
//! 2. Accurate tokenizer family detection and calibrated subword heuristics.
//! 3. Provenance and approximation transparency (`EstimationMethod`, `TokenEstimate`).
//! 4. Full request context estimation (messages, tool schemas, primer, framing).
//! 5. Context budget safety margin preventing upstream context overflow.

use ai_backend::inference::tokens::{EstimationMethod, TokenEstimator, TokenizerFamily};
use ai_backend::models::ChatMessage;

#[test]
fn test_corpus_plain_english_token_accuracy() {
    let text =
        "The quick brown fox jumps over the lazy dog. Performance and security are top priorities.";
    let tokens = TokenEstimator::estimate_text_for_family(text, TokenizerFamily::Llama);
    // 17 words, ~19-23 tokens in BPE
    assert!((16..=26).contains(&tokens), "Got {tokens} tokens");
}

#[test]
fn test_corpus_roman_hinglish_estimation() {
    let hinglish_prompt =
        "Bhai yeh code optimize kardo bina existing functionality break kiye. Jaldi reply karna.";
    let llama_tokens =
        TokenEstimator::estimate_text_for_family(hinglish_prompt, TokenizerFamily::Llama);
    let qwen_tokens =
        TokenEstimator::estimate_text_for_family(hinglish_prompt, TokenizerFamily::Qwen);

    // 13 words, ~14-22 tokens
    assert!(
        (12..=25).contains(&llama_tokens),
        "Llama tokens: {llama_tokens}"
    );
    assert!(
        (12..=25).contains(&qwen_tokens),
        "Qwen tokens: {qwen_tokens}"
    );
}

#[test]
fn test_corpus_urdu_script_estimation() {
    let urdu_text = "برائے مہربانی اس سسٹم کی کارکردگی اور سیکیورٹی کو بہتر بنائیں";
    let llama_tokens = TokenEstimator::estimate_text_for_family(urdu_text, TokenizerFamily::Llama);
    let qwen_tokens = TokenEstimator::estimate_text_for_family(urdu_text, TokenizerFamily::Qwen);

    // Urdu string has 60 characters, ~15-35 tokens
    assert!(llama_tokens >= 15, "Llama tokens: {llama_tokens}");
    assert!(qwen_tokens >= 15, "Qwen tokens: {qwen_tokens}");
}

#[test]
fn test_corpus_emojis_and_symbols() {
    let emoji_text = "AI System Launched! 🚀✨🤖💡🎉 Safe and verified.";
    let tokens = TokenEstimator::estimate_text_for_family(emoji_text, TokenizerFamily::Llama);
    // 5 emojis + English words
    assert!(tokens >= 10, "Got {tokens} tokens");
}

#[test]
fn test_corpus_code_and_tool_json_schemas() {
    let rust_code = r#"
        pub async fn handle_request(req: Request) -> Result<Response, AppError> {
            let user_id = req.headers().get("x-user-id")?;
            Ok(Response::ok(user_id))
        }
    "#;
    let code_tokens = TokenEstimator::estimate_text_for_family(rust_code, TokenizerFamily::Llama);
    assert!(
        code_tokens >= 25,
        "Got {code_tokens} tokens for code snippet"
    );

    let tool_schema = r#"{
        "name": "get_current_weather",
        "description": "Get the current weather in a given location",
        "parameters": {
            "type": "object",
            "properties": {
                "location": {"type": "string", "description": "The city and state, e.g. San Francisco, CA"},
                "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
            },
            "required": ["location"]
        }
    }"#;
    let tool_tokens = TokenEstimator::estimate_tools_tokens(tool_schema, TokenizerFamily::Llama);
    // Tools schema includes JSON tokens + 12 tokens framing overhead
    assert!(
        tool_tokens >= 40,
        "Got {tool_tokens} tokens for tool schema"
    );
}

#[test]
fn test_detailed_provenance_and_approximation_labeling() {
    // Known family model (Llama)
    let llama_estimate =
        TokenEstimator::estimate_text_detailed("Hello world", Some("llama-3.3-70b-versatile"));
    assert_eq!(llama_estimate.family, TokenizerFamily::Llama);
    assert_eq!(
        llama_estimate.method,
        EstimationMethod::CalibratedFamilyHeuristic
    );
    assert!(
        !llama_estimate.is_exact,
        "Must be clearly labeled as non-exact approximation"
    );
    assert!(llama_estimate.confidence_note.contains("Calibrated"));

    // Unknown/custom model defaults to conservative fallback
    let unknown_estimate =
        TokenEstimator::estimate_text_detailed("Hello world", Some("proprietary-custom-llm-v1"));
    assert_eq!(unknown_estimate.family, TokenizerFamily::Generic);
    assert_eq!(
        unknown_estimate.method,
        EstimationMethod::ConservativeFallback
    );
    assert!(!unknown_estimate.is_exact);
    assert!(unknown_estimate.confidence_note.contains("Conservative"));
}

#[test]
fn test_context_window_safe_budget_applies_safety_margin() {
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: "You are a specialized Rust backend expert.".to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: "Explain Tokio task scheduling and cancellation safety.".to_string(),
        },
    ];

    let raw_tokens =
        TokenEstimator::estimate_chat_for_model(&messages, Some("llama-3.3-70b-versatile"));
    let safe_estimate = TokenEstimator::estimate_request_context_safe(
        &messages,
        None,
        Some("llama-3.3-70b-versatile"),
    );

    // Safe estimate must exceed raw tokens by the applied safety margin (at least 4 tokens)
    assert!(safe_estimate.estimated_tokens > raw_tokens);
    assert!(safe_estimate.safety_margin_applied >= 4);
    assert_eq!(
        safe_estimate.estimated_tokens,
        raw_tokens + safe_estimate.safety_margin_applied
    );
}

#[test]
fn test_registered_reference_model_family_aliases() {
    assert_eq!(
        TokenizerFamily::from_model_id("fast-chat-v1"),
        TokenizerFamily::Llama
    );
    assert_eq!(
        TokenizerFamily::from_model_id("deep-reasoner-v1"),
        TokenizerFamily::DeepSeek
    );
    assert_eq!(
        TokenizerFamily::from_model_id("balanced-assistant-v1"),
        TokenizerFamily::Mistral
    );
    assert_eq!(
        TokenizerFamily::from_model_id("unregistered-generic-v2"),
        TokenizerFamily::Generic
    );
}

#[test]
fn test_corpus_empty_and_whitespace_inputs() {
    assert_eq!(
        TokenEstimator::estimate_text_for_family("", TokenizerFamily::Llama),
        0
    );
    assert_eq!(
        TokenEstimator::estimate_text_for_family("   \t\n  \r\n", TokenizerFamily::Llama),
        0
    );
    assert_eq!(
        TokenEstimator::estimate_chat_for_family(&[], TokenizerFamily::Llama),
        0
    );
}

#[test]
fn test_corpus_mixed_script_boundary() {
    let mixed = "Hello world! بھائی یہ چیک کرو: fn main() { println!(\"🚀\"); } नमस्ते";
    let tokens = TokenEstimator::estimate_text_for_family(mixed, TokenizerFamily::Llama);
    // Contains English, Urdu, Hindi/Devanagari, Rust code, and Emoji
    assert!(tokens >= 20, "Got {tokens} tokens for mixed-script input");

    let estimate = TokenEstimator::estimate_text_detailed(mixed, Some("fast-chat-v1"));
    assert_eq!(estimate.family, TokenizerFamily::Llama);
    assert_eq!(estimate.method, EstimationMethod::CalibratedFamilyHeuristic);
    assert!(!estimate.is_exact);
}

#[test]
fn test_reproduce_code_underestimation_and_safety_margin_compensation() {
    let snippet = "fn compute(x: i32) -> i32 { x * 2 + 1 }";
    let raw_tokens = TokenEstimator::estimate_text_for_family(snippet, TokenizerFamily::Llama);

    // Let's test request context safe estimation:
    let msg = vec![ChatMessage {
        role: "user".to_string(),
        content: snippet.to_string(),
    }];
    let safe_estimate =
        TokenEstimator::estimate_request_context_safe(&msg, None, Some("fast-chat-v1"));

    // Verify raw tokens vs safe estimate
    assert!(safe_estimate.estimated_tokens > raw_tokens);
    assert!(safe_estimate.safety_margin_applied >= 4);
    assert!(!safe_estimate.is_exact);
}

#[test]
fn test_exact_bpe_reference_vs_approximate_comparison() {
    let snippet = "fn compute(x: i32) -> i32 { x * 2 + 1 }";

    // Exact BPE tokenization (GPT-4 / GPT-4o)
    let gpt_estimate = TokenEstimator::estimate_text_detailed(snippet, Some("gpt-4o"));
    assert!(gpt_estimate.is_exact);
    assert_eq!(gpt_estimate.method, EstimationMethod::ExactTokenizer);
    assert_eq!(gpt_estimate.estimated_tokens, 19);
    assert_eq!(gpt_estimate.safety_margin_applied, 0);

    // Approximate calibrated tokenization (Llama 3 / Fast-Chat)
    let llama_estimate = TokenEstimator::estimate_text_detailed(snippet, Some("fast-chat-v1"));
    assert!(!llama_estimate.is_exact);
    assert_eq!(
        llama_estimate.method,
        EstimationMethod::CalibratedFamilyHeuristic
    );
    assert_eq!(llama_estimate.estimated_tokens, 17);

    // Safe context estimation for Llama applies +15% margin (minimum 8 tokens)
    let msg = vec![ChatMessage {
        role: "user".to_string(),
        content: snippet.to_string(),
    }];
    let llama_safe =
        TokenEstimator::estimate_request_context_safe(&msg, None, Some("fast-chat-v1"));
    assert!(!llama_safe.is_exact);
    assert!(llama_safe.safety_margin_applied >= 8);

    // Safe context estimation for GPT utilizes verified BPE raw text tokenization with standard
    // chat framing overhead. Because wire-level message framing is heuristic (+3 framing, +3 primer),
    // request-level is_exact is false, raw_text_exact is Some(true), and safety margin is 0.
    let gpt_safe = TokenEstimator::estimate_request_context_safe(&msg, None, Some("gpt-4o"));
    assert!(
        !gpt_safe.is_exact,
        "Full request cannot be marked exact due to estimated framing"
    );
    assert_eq!(
        gpt_safe.raw_text_exact,
        Some(true),
        "Raw text BPE count is exact"
    );
    assert_eq!(gpt_safe.method, EstimationMethod::EstimatedChatFraming);
    assert_eq!(gpt_safe.safety_margin_applied, 0);
    // 3 tokens framing + 1 token role ("user") + 19 tokens content + 3 tokens primer = 26 tokens
    assert_eq!(gpt_safe.estimated_tokens, 26);
}
