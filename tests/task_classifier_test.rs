//! Comprehensive Phase 6 Task Classification test suite.
//!
//! Validates MASTER_SPEC.md §6.1, §6.2, §6.3, §18 Phase 6, API_SPEC.md §4, and ARCHITECTURE.md §2.

use ai_backend::config::AppConfig;
use ai_backend::errors::AppError;
use ai_backend::router::{
    ClassificationResult, MAX_PROMPT_LENGTH, RoutingPreviewRequest, RoutingPreviewResponse,
    TaskCategory, TaskClassifier, TaskComplexity, TaskModality,
};
use ai_backend::{AppState, dispatch_route};
use std::sync::Arc;

// ============================================================================
// 1. All Documented Task Categories Covered
// ============================================================================

#[test]
fn test_all_15_task_categories_classified() {
    let test_cases = [
        // (Prompt, Modality, ExpectedCategory, ExpectedComplexity)
        (
            "Can you help me with this question?",
            "text",
            TaskCategory::SimpleQuestions,
            TaskComplexity::Low,
        ),
        (
            "Hello there, good morning!",
            "text",
            TaskCategory::GeneralConversation,
            TaskComplexity::Low,
        ),
        (
            "Prove step by step why P != NP and analyze the theoretical implications",
            "text",
            TaskCategory::ComplexAnalysis,
            TaskComplexity::High,
        ),
        (
            "what is Tokio?",
            "text",
            TaskCategory::FastReasoning,
            TaskComplexity::Low,
        ),
        (
            "Write a Rust function to parse an ELF binary header.",
            "code",
            TaskCategory::CodingAndDebugging,
            TaskComplexity::Medium,
        ),
        (
            "Debug this async deadlock involving unsafe pointers and lifetimes in Rust",
            "text",
            TaskCategory::CodingAndDebugging,
            TaskComplexity::High,
        ),
        (
            "Design a distributed system architecture with microservices and cap theorem trade-offs",
            "text",
            TaskCategory::ArchitectureAndDesign,
            TaskComplexity::High,
        ),
        (
            "Calculate the integral of e^(-x^2) and solve for x",
            "text",
            TaskCategory::MathematicsAndCalculation,
            TaskComplexity::Medium,
        ),
        (
            "Prove by induction the binomial theorem and find matrix eigenvalue",
            "text",
            TaskCategory::MathematicsAndCalculation,
            TaskComplexity::High,
        ),
        (
            "Search the web for latest news and current price of Bitcoin in 2026",
            "text",
            TaskCategory::WebSearchAndResearch,
            TaskComplexity::Medium,
        ),
        (
            "Generate an image of a neon-lit cyberpunk city at dusk",
            "text",
            TaskCategory::ImageGeneration,
            TaskComplexity::Medium,
        ),
        (
            "Crop and remove background from this portrait and retouch edges",
            "image",
            TaskCategory::ImageEditing,
            TaskComplexity::Medium,
        ),
        (
            "Generate a video of a star collapsing into a neutron star with motion synthesis",
            "text",
            TaskCategory::VideoGeneration,
            TaskComplexity::High,
        ),
        (
            "Transcribe audio speech to text and summarize spoken notes",
            "text",
            TaskCategory::AudioProcessing,
            TaskComplexity::Medium,
        ),
        (
            "Draft a contract and formal policy document for internal software governance",
            "text",
            TaskCategory::DocumentCreation,
            TaskComplexity::Medium,
        ),
        (
            "Create a PowerPoint slide deck with 10 slides on cloud resilience",
            "text",
            TaskCategory::PowerPointGeneration,
            TaskComplexity::Medium,
        ),
        (
            "Extract into JSON format all tabular rows from this CSV dump",
            "text",
            TaskCategory::DataExtraction,
            TaskComplexity::Medium,
        ),
    ];

    for (prompt, modality, expected_cat, expected_comp) in test_cases {
        let result = TaskClassifier::classify_structured(prompt, modality)
            .unwrap_or_else(|e| panic!("Failed to classify prompt '{prompt}': {e}"));

        assert_eq!(
            result.category, expected_cat,
            "Mismatch category for prompt: '{prompt}'"
        );
        assert_eq!(
            result.complexity, expected_comp,
            "Mismatch complexity for prompt: '{prompt}'"
        );
        assert!(
            result.confidence >= 0.70 && result.confidence <= 1.0,
            "Confidence out of range: {}",
            result.confidence
        );
        assert!(!result.detected_intent.is_empty());
        assert!(!result.rationale.is_empty());

        // Also test backward-compatible classify()
        let (task_str, comp_str) = TaskClassifier::classify(prompt, modality);
        assert_eq!(task_str, expected_cat.as_str());
        assert_eq!(comp_str, expected_comp.as_str());
    }
}

// ============================================================================
// 2. Simple vs Complex Requests Across Domains
// ============================================================================

#[test]
fn test_simple_vs_complex_classification() {
    // Coding: Simple helper vs Complex concurrency/lifetimes
    let simple_code =
        TaskClassifier::classify_structured("fn add(a: i32, b: i32) -> i32 { a + b }", "code")
            .unwrap();
    assert_eq!(simple_code.category, TaskCategory::CodingAndDebugging);
    assert_eq!(simple_code.complexity, TaskComplexity::Medium);

    let complex_code = TaskClassifier::classify_structured(
        "Fix concurrency race condition and deadlock in async mutex with unsafe memory",
        "text",
    )
    .unwrap();
    assert_eq!(complex_code.category, TaskCategory::CodingAndDebugging);
    assert_eq!(complex_code.complexity, TaskComplexity::High);

    // Math: Simple arithmetic vs Formal proof
    let simple_math =
        TaskClassifier::classify_structured("Calculate 45 * 892 - 120", "text").unwrap();
    assert_eq!(
        simple_math.category,
        TaskCategory::MathematicsAndCalculation
    );
    assert_eq!(simple_math.complexity, TaskComplexity::Medium);

    let complex_math = TaskClassifier::classify_structured(
        "Mathematical proof by induction for differential equations and eigenvalue matrix",
        "text",
    )
    .unwrap();
    assert_eq!(
        complex_math.category,
        TaskCategory::MathematicsAndCalculation
    );
    assert_eq!(complex_math.complexity, TaskComplexity::High);

    // Conversation: Fast factual vs Deep reasoning
    let fast_q = TaskClassifier::classify_structured("what is an IP address?", "text").unwrap();
    assert_eq!(fast_q.category, TaskCategory::FastReasoning);
    assert_eq!(fast_q.complexity, TaskComplexity::Low);

    let deep_analysis = TaskClassifier::classify_structured(
        "Analyze why the Byzantine Generals Problem cannot be solved with 1/3 faulty nodes and derive proofs",
        "text",
    )
    .unwrap();
    assert_eq!(deep_analysis.category, TaskCategory::ComplexAnalysis);
    assert_eq!(deep_analysis.complexity, TaskComplexity::High);
}

// ============================================================================
// 3. Modality Overrides and Media Detection
// ============================================================================

#[test]
fn test_modality_overrides() {
    // Explicit image modality defaults to ImageGeneration unless editing cues present
    let img_gen = TaskClassifier::classify_structured("a futuristic car", "image").unwrap();
    assert_eq!(img_gen.category, TaskCategory::ImageGeneration);
    assert_eq!(img_gen.output_modality, TaskModality::Image);
    assert!(img_gen.requires_tools);

    let img_edit = TaskClassifier::classify_structured(
        "inpaint and remove the tree in the background",
        "image",
    )
    .unwrap();
    assert_eq!(img_edit.category, TaskCategory::ImageEditing);
    assert_eq!(img_edit.output_modality, TaskModality::Image);

    // Explicit video modality
    let vid = TaskClassifier::classify_structured("timelapse of a blooming rose", "video").unwrap();
    assert_eq!(vid.category, TaskCategory::VideoGeneration);
    assert_eq!(vid.output_modality, TaskModality::Video);
    assert_eq!(vid.complexity, TaskComplexity::High);

    // Explicit audio modality
    let audio = TaskClassifier::classify_structured("transcribe this meeting", "audio").unwrap();
    assert_eq!(audio.category, TaskCategory::AudioProcessing);
    assert_eq!(audio.output_modality, TaskModality::Audio);

    // Explicit document modality with presentation cues
    let slides =
        TaskClassifier::classify_structured("quarterly earnings deck presentation", "document")
            .unwrap();
    assert_eq!(slides.category, TaskCategory::PowerPointGeneration);
    assert_eq!(slides.output_modality, TaskModality::Document);

    let doc =
        TaskClassifier::classify_structured("engineering guidelines specification", "document")
            .unwrap();
    assert_eq!(doc.category, TaskCategory::DocumentCreation);
    assert_eq!(doc.output_modality, TaskModality::Document);
}

// ============================================================================
// 4. Mixed-Intent and Conflicting Signals
// ============================================================================

#[test]
fn test_mixed_intent_handling() {
    // Code prompt requested under video modality -> Modality override takes precedence
    let res =
        TaskClassifier::classify_structured("fn render_frame(time: f64) -> VideoBuffer", "video")
            .unwrap();
    assert_eq!(res.category, TaskCategory::VideoGeneration);

    // Code prompt that asks for slide creation
    let res2 = TaskClassifier::classify_structured(
        "Create a slide deck presentation explaining Rust memory safety",
        "text",
    )
    .unwrap();
    assert_eq!(res2.category, TaskCategory::PowerPointGeneration);

    // Architecture vs Code: Architecture keywords prioritize high-level engineering
    let res3 = TaskClassifier::classify_structured(
        "Design the system architecture and microservices database schema for our rust backend",
        "text",
    )
    .unwrap();
    assert_eq!(res3.category, TaskCategory::ArchitectureAndDesign);
    assert_eq!(res3.complexity, TaskComplexity::High);
}

// ============================================================================
// 5. Empty and Whitespace-Only Inputs
// ============================================================================

#[test]
fn test_empty_and_whitespace_prompts_rejected() {
    let empty_cases = ["", "   ", "\t", "\n", "\r\n", "   \t \n  "];

    for empty in empty_cases {
        let err = TaskClassifier::classify_structured(empty, "text").unwrap_err();
        match err {
            AppError::InvalidRequest(msg) => {
                assert!(
                    msg.contains("cannot be empty"),
                    "Expected 'cannot be empty', got: '{msg}'"
                );
            }
            other => panic!("Expected AppError::InvalidRequest, got: {other:?}"),
        }
    }

    // Backward-compatible classify() returns safe fallback for empty input
    let (cat, comp) = TaskClassifier::classify("", "text");
    assert_eq!(cat, "general_conversation");
    assert_eq!(comp, "low");
}

// ============================================================================
// 6. Input Length Bounds Enforcement (100,000 characters)
// ============================================================================

#[test]
fn test_prompt_size_bounds() {
    // Exactly MAX_PROMPT_LENGTH (100,000 chars) should be accepted
    let exact_prompt = "a".repeat(MAX_PROMPT_LENGTH);
    let result = TaskClassifier::classify_structured(&exact_prompt, "text");
    assert!(result.is_ok(), "Max prompt length should be accepted");

    // MAX_PROMPT_LENGTH + 1 should be rejected
    let oversized = "a".repeat(MAX_PROMPT_LENGTH + 1);
    let err = TaskClassifier::classify_structured(&oversized, "text").unwrap_err();
    match err {
        AppError::InvalidRequest(msg) => {
            assert!(
                msg.contains("exceeds 100000 characters limit"),
                "Expected exceeds 100,000 limit, got '{msg}'"
            );
        }
        other => panic!("Expected InvalidRequest, got {other:?}"),
    }
}

// ============================================================================
// 7. Unsupported Modality & Invalid Parameters Validation
// ============================================================================

#[test]
fn test_invalid_modality_rejected() {
    let invalid_modalities = ["quantum", "hologram", "3d_model", "unknown_format"];

    for invalid in invalid_modalities {
        let err = TaskClassifier::classify_structured("hello", invalid).unwrap_err();
        match err {
            AppError::InvalidRequest(msg) => {
                assert!(
                    msg.contains("Unsupported modality"),
                    "Expected unsupported modality message, got '{msg}'"
                );
            }
            other => panic!("Expected InvalidRequest, got {other:?}"),
        }
    }
}

// ============================================================================
// 8. Unicode and Multilingual Support
// ============================================================================

#[test]
fn test_unicode_and_multilingual_inputs() {
    // Multilingual Chinese
    let res_zh =
        TaskClassifier::classify_structured("这是一个关于系统的常规对话测试", "text").unwrap();
    assert_eq!(res_zh.category, TaskCategory::GeneralConversation);
    assert_eq!(res_zh.complexity, TaskComplexity::Low);

    // Multilingual French coding query
    let res_fr = TaskClassifier::classify_structured(
        "Comment implémenter un trait async en rust pour éviter les deadlock?",
        "text",
    )
    .unwrap();
    assert_eq!(res_fr.category, TaskCategory::CodingAndDebugging);
    assert_eq!(res_fr.complexity, TaskComplexity::High);

    // Multilingual German simple question
    let res_de =
        TaskClassifier::classify_structured("Was ist ein Betriebssystem?", "text").unwrap();
    assert_eq!(res_de.category, TaskCategory::SimpleQuestions);

    // Arabic short question
    let res_ar = TaskClassifier::classify_structured("هل يمكنك مساعدتي؟", "text").unwrap();
    assert_eq!(res_ar.category, TaskCategory::SimpleQuestions);

    // Emojis and multibyte characters in code snippet
    let res_emoji = TaskClassifier::classify_structured(
        "fn print_status() { println!(\"🚀 Status: OK 🔒\"); }",
        "code",
    )
    .unwrap();
    assert_eq!(res_emoji.category, TaskCategory::CodingAndDebugging);
}

// ============================================================================
// 9. Determinism Over Repeated Executions
// ============================================================================

#[test]
fn test_classification_determinism() {
    let prompt = "Explain how async deadlock occurs with atomic operations and unsafe memory";
    let first = TaskClassifier::classify_structured(prompt, "text").unwrap();

    for _ in 0..1000 {
        let current = TaskClassifier::classify_structured(prompt, "text").unwrap();
        assert_eq!(first.category, current.category);
        assert_eq!(first.complexity, current.complexity);
        assert_eq!(first.confidence, current.confidence);
        assert_eq!(first.input_modality, current.input_modality);
        assert_eq!(first.output_modality, current.output_modality);
        assert_eq!(first.requires_tools, current.requires_tools);
        assert_eq!(first.detected_intent, current.detected_intent);
        assert_eq!(first.rationale, current.rationale);
    }
}

// ============================================================================
// 10. Classification Result Typed Serialization
// ============================================================================

#[test]
fn test_classification_result_serialization() {
    let result = ClassificationResult {
        category: TaskCategory::ArchitectureAndDesign,
        complexity: TaskComplexity::High,
        input_modality: TaskModality::Text,
        output_modality: TaskModality::Text,
        requires_tools: false,
        confidence: 0.92,
        detected_intent: "Software architecture and engineering".to_string(),
        rationale: "Prompt requests systems architecture design.".to_string(),
    };

    let json = serde_json::to_string(&result).expect("Serialization failed");
    assert!(json.contains(r#""category":"architecture_and_design""#));
    assert!(json.contains(r#""complexity":"high""#));
    assert!(json.contains(r#""input_modality":"text""#));
    assert!(json.contains(r#""output_modality":"text""#));
    assert!(json.contains(r#""requires_tools":false"#));
    assert!(json.contains(r#""confidence":0.92"#));

    let deserialized: ClassificationResult =
        serde_json::from_str(&json).expect("Deserialization failed");
    assert_eq!(result, deserialized);
}

// ============================================================================
// 11. Integration with Router Preview & Error Handling
// ============================================================================

#[test]
fn test_routing_preview_integration_success() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // 1. Coding task preview
    let req = RoutingPreviewRequest {
        prompt: "Write a Rust function to parse an ELF binary header.".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let preview: RoutingPreviewResponse = state.router.preview(&req).unwrap();
    assert_eq!(preview.detected_task, "coding_and_debugging");
    assert_eq!(preview.estimated_complexity, "medium");
    assert_eq!(preview.selected_model_id, "deep-reasoner-v1");
    assert_eq!(preview.provider_id, "openrouter-api");
    assert!(
        preview
            .fallback_model_ids
            .contains(&"fast-chat-v1".to_string())
    );

    // 2. Systems Architecture query
    let req_arch = RoutingPreviewRequest {
        prompt: "Design a distributed system architecture with high availability and microservices"
            .to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let preview_arch = state.router.preview(&req_arch).unwrap();
    assert_eq!(preview_arch.detected_task, "architecture_and_design");
    assert_eq!(preview_arch.estimated_complexity, "high");
    assert_eq!(preview_arch.selected_model_id, "deep-reasoner-v1");

    // 3. Fast reasoning query
    let req_fast = RoutingPreviewRequest {
        prompt: "what is Tokio?".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let preview_fast = state.router.preview(&req_fast).unwrap();
    assert_eq!(preview_fast.detected_task, "fast_reasoning");
    assert_eq!(preview_fast.selected_model_id, "fast-chat-v1");
}

#[test]
fn test_routing_preview_invalid_requests() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // Empty prompt returns 400 InvalidRequest
    let req_empty = RoutingPreviewRequest {
        prompt: "   ".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let err = state.router.preview(&req_empty).unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert_eq!(err.error_code(), "INVALID_REQUEST");
    let envelope = err.to_json_envelope("req_test");
    assert!(envelope.contains(r#""code":"INVALID_REQUEST""#));
    assert!(envelope.contains(r#""retryable":false"#));
    assert!(envelope.contains("Prompt cannot be empty"));

    // Invalid modality returns 400 InvalidRequest
    let req_mod = RoutingPreviewRequest {
        prompt: "Hello".to_string(),
        requested_modality: "quantum_field".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let err_mod = state.router.preview(&req_mod).unwrap_err();
    assert_eq!(err_mod.status_code(), 400);
    assert_eq!(err_mod.error_code(), "INVALID_REQUEST");
    assert!(
        err_mod
            .to_string()
            .contains("Unsupported modality 'quantum_field'")
    );

    // Invalid routing mode returns 400 InvalidRequest
    let req_mode = RoutingPreviewRequest {
        prompt: "Hello".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "unsupported_mode".to_string(),
        preferred_model_id: None,
    };
    let err_mode = state.router.preview(&req_mode).unwrap_err();
    assert_eq!(err_mode.status_code(), 400);
    assert_eq!(err_mode.error_code(), "INVALID_REQUEST");
}

// ============================================================================
// 12. Non-Regression of Health & Readiness
// ============================================================================

#[test]
fn test_health_and_readiness_non_regression() {
    let config = Arc::new(AppConfig::default_local());
    let state = AppState::new(config);

    // Health returns 200
    let (status, _, body) = dispatch_route("GET", "/api/v1/health", &state);
    assert_eq!(status, 200);
    assert!(body.contains(r#""status":"healthy""#));

    // Ready returns 503 when 0 active providers exist
    let (status_ready, _, body_ready) = dispatch_route("GET", "/api/v1/ready", &state);
    assert_eq!(status_ready, 503);
    assert!(body_ready.contains(r#""status":"not_ready""#));
}

// ============================================================================
// 13. Roman Hindi & Hinglish Classification Coverage
// ============================================================================

#[test]
fn test_roman_hindi_hinglish_classification() {
    let hinglish_cases = [
        // Coding & Debugging
        (
            "is code me bug fix karo aur error hatao",
            TaskCategory::CodingAndDebugging,
            TaskComplexity::Medium,
        ),
        (
            "mujhe ek python function bana ke do jo prime numbers check kare",
            TaskCategory::CodingAndDebugging,
            TaskComplexity::Medium,
        ),
        (
            "ye script likho jo file download kare aur async memory deadlock na ho",
            TaskCategory::CodingAndDebugging,
            TaskComplexity::High,
        ),
        // Systems Architecture
        (
            "system architecture design karo microservices ke sath aur cap theorem explain karo",
            TaskCategory::ArchitectureAndDesign,
            TaskComplexity::High,
        ),
        // Mathematics & Calculation
        (
            "ye math problem solve karo aur integral calculate karo",
            TaskCategory::MathematicsAndCalculation,
            TaskComplexity::Medium,
        ),
        (
            "binomial theorem prove karo by induction aur eigenvalue calculate karo",
            TaskCategory::MathematicsAndCalculation,
            TaskComplexity::High,
        ),
        // Web Search & Research
        (
            "bitcoin ka current price search karo online",
            TaskCategory::WebSearchAndResearch,
            TaskComplexity::Medium,
        ),
        (
            "internet pe search karo latest news kya hai 2026 me",
            TaskCategory::WebSearchAndResearch,
            TaskComplexity::Medium,
        ),
        // Image Generation
        (
            "ek sundar image generate karo jisme pahad aur nadi ho",
            TaskCategory::ImageGeneration,
            TaskComplexity::Medium,
        ),
        (
            "photo bana do neon cyberpunk style me",
            TaskCategory::ImageGeneration,
            TaskComplexity::Medium,
        ),
        // Image Editing
        (
            "is photo ka background remove karo aur retouch karo",
            TaskCategory::ImageEditing,
            TaskComplexity::Medium,
        ),
        // Video Generation
        (
            "ek video generate karo motion ke sath space travel par",
            TaskCategory::VideoGeneration,
            TaskComplexity::High,
        ),
        // Audio Processing
        (
            "audio transcribe karke text me convert karo",
            TaskCategory::AudioProcessing,
            TaskComplexity::Medium,
        ),
        // Document Creation
        (
            "formal contract document draft karo software governance ke liye",
            TaskCategory::DocumentCreation,
            TaskComplexity::Medium,
        ),
        // PowerPoint Generation
        (
            "ek presentation bana do 10 slides ki cloud computing par",
            TaskCategory::PowerPointGeneration,
            TaskComplexity::Medium,
        ),
        // Data Extraction
        (
            "is data ko json me convert karo table format se",
            TaskCategory::DataExtraction,
            TaskComplexity::Medium,
        ),
        // Complex Analysis
        (
            "step by step explain karo aur analyze karo ki P vs NP kyu hard hai",
            TaskCategory::ComplexAnalysis,
            TaskComplexity::High,
        ),
        // Fast Reasoning (Factual queries)
        (
            "Tokio kya hota hai?",
            TaskCategory::FastReasoning,
            TaskComplexity::Low,
        ),
        (
            "Rust kya hai?",
            TaskCategory::FastReasoning,
            TaskComplexity::Low,
        ),
        // Simple Questions (Conversational questions)
        (
            "kaise ho bhai?",
            TaskCategory::SimpleQuestions,
            TaskComplexity::Low,
        ),
        (
            "kya haal hai?",
            TaskCategory::SimpleQuestions,
            TaskComplexity::Low,
        ),
        // General Conversation
        (
            "namaste dost, shukriya",
            TaskCategory::GeneralConversation,
            TaskComplexity::Low,
        ),
    ];

    for (prompt, expected_cat, expected_comp) in hinglish_cases {
        let res = TaskClassifier::classify_structured(prompt, "text")
            .unwrap_or_else(|e| panic!("Failed to classify Hinglish prompt '{prompt}': {e}"));

        assert_eq!(
            res.category, expected_cat,
            "Category mismatch for Hinglish prompt: '{prompt}'"
        );
        assert_eq!(
            res.complexity, expected_comp,
            "Complexity mismatch for Hinglish prompt: '{prompt}'"
        );
    }
}

// ============================================================================
// 14. Conflicting Signals & Edge-Case Classification
// ============================================================================

#[test]
fn test_conflicting_and_ambiguous_cues() {
    // 1. Text mentions "search" and "code", but modality is "code"
    let res1 = TaskClassifier::classify_structured(
        "search for a sorting algorithm and write code",
        "code",
    )
    .unwrap();
    assert_eq!(res1.category, TaskCategory::CodingAndDebugging);

    // 2. Text mentions "video" and "presentation", but requested modality is "video"
    let res2 = TaskClassifier::classify_structured(
        "create presentation video of annual conference",
        "video",
    )
    .unwrap();
    assert_eq!(res2.category, TaskCategory::VideoGeneration);

    // 3. Prompt asks for a slide deck about coding
    let res3 = TaskClassifier::classify_structured(
        "Create a slide deck with 5 slides on Rust concurrency and async fn",
        "text",
    )
    .unwrap();
    assert_eq!(res3.category, TaskCategory::PowerPointGeneration);

    // 4. Prompt with ambiguous query falls back cleanly
    let res4 =
        TaskClassifier::classify_structured("something interesting to consider today", "text")
            .unwrap();
    assert_eq!(res4.category, TaskCategory::GeneralConversation);
    assert_eq!(res4.complexity, TaskComplexity::Low);
    assert!(res4.confidence > 0.5);
}

// ============================================================================
// 15. In-Process Performance Microbenchmark
// ============================================================================

#[test]
fn test_task_classifier_performance_benchmark() {
    let benchmark_prompts = [
        (
            "Write a Rust function to parse an ELF binary header.",
            "code",
        ),
        (
            "Design a distributed system architecture with microservices and cap theorem",
            "text",
        ),
        ("Calculate the integral of e^(-x^2) and solve for x", "text"),
        ("what is Tokio?", "text"),
        ("Tokio kya hota hai?", "text"),
        (
            "ek presentation bana do 10 slides ki cloud computing par",
            "text",
        ),
        ("is photo ka background remove karo", "text"),
        ("Hello, how are you doing today?", "text"),
        (
            "Prove by induction the binomial theorem and compute matrix eigenvalues",
            "text",
        ),
        (
            "Extract into JSON format all rows from this CSV table dump",
            "text",
        ),
    ];

    let total_iterations = 10_000;
    let mut latencies_micros: Vec<u128> = Vec::with_capacity(total_iterations);

    let benchmark_start = std::time::Instant::now();

    for i in 0..total_iterations {
        let (prompt, modality) = benchmark_prompts[i % benchmark_prompts.len()];
        let iter_start = std::time::Instant::now();
        let _ = TaskClassifier::classify_structured(prompt, modality).unwrap();
        latencies_micros.push(iter_start.elapsed().as_micros());
    }

    let total_elapsed = benchmark_start.elapsed();
    latencies_micros.sort_unstable();

    let p50 = latencies_micros[total_iterations * 50 / 100];
    let p95 = latencies_micros[total_iterations * 95 / 100];
    let p99 = latencies_micros[total_iterations * 99 / 100];
    let min = latencies_micros[0];
    let max = latencies_micros[total_iterations - 1];
    let mean: f64 = latencies_micros.iter().sum::<u128>() as f64 / total_iterations as f64;
    let ops_per_sec = total_iterations as f64 / total_elapsed.as_secs_f64();

    println!("\n--- TASK CLASSIFIER IN-PROCESS MICROBENCHMARK ---");
    println!("Total Samples: {}", total_iterations);
    println!("Total Duration: {:.4}s", total_elapsed.as_secs_f64());
    println!("Throughput: {:.2} ops/sec", ops_per_sec);
    println!("Min: {} µs ({:.4} ms)", min, min as f64 / 1000.0);
    println!("p50 (Median): {} µs ({:.4} ms)", p50, p50 as f64 / 1000.0);
    println!("Mean: {:.2} µs ({:.4} ms)", mean, mean / 1000.0);
    println!("p95: {} µs ({:.4} ms)", p95, p95 as f64 / 1000.0);
    println!("p99: {} µs ({:.4} ms)", p99, p99 as f64 / 1000.0);
    println!("Max: {} µs ({:.4} ms)", max, max as f64 / 1000.0);

    // p95 internal overhead must be well below the MASTER_SPEC.md §3.1 limit of 10 ms (10,000 µs)
    assert!(
        p95 < 1_000,
        "p95 latency ({} µs) exceeded 1,000 µs (1.0 ms) threshold",
        p95
    );
}
