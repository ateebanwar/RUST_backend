//! Integration and policy test suite for Phase 7 — Intelligent Model Router.
//!
//! Conforms strictly to MASTER_SPEC.md §6, §18 Phase 7, API_SPEC.md §4, ARCHITECTURE.md §2,
//! and DEC-010.

use ai_backend::errors::AppError;
use ai_backend::models::ModelMetadata;
use ai_backend::registry::ModelRegistry;
use ai_backend::router::{ModelRouter, RoutingPreviewRequest, RoutingPreviewResponse};
use std::sync::Arc;

/// Helper to construct a custom test model metadata.
#[allow(clippy::too_many_arguments)]
fn make_test_model(
    id: &str,
    provider: &str,
    modalities: Vec<&str>,
    speed: &str,
    cost: &str,
    available: bool,
    capabilities: Vec<&str>,
    context: usize,
    output: usize,
) -> ModelMetadata {
    ModelMetadata {
        model_id: id.to_string(),
        display_name: format!("Test Model {id}"),
        provider_id: provider.to_string(),
        supported_modalities: modalities.into_iter().map(String::from).collect(),
        context_window: context,
        max_output_tokens: output,
        streaming_supported: true,
        tool_calling_supported: true,
        speed_tier: speed.to_string(),
        cost_tier: cost.to_string(),
        available,
        capabilities: capabilities.into_iter().map(String::from).collect(),
        last_health_check: "2026-10-19T20:00:00Z".to_string(),
    }
}

// ============================================================================
// 1. Multi-Criteria Candidate Scoring Tests
// ============================================================================

#[test]
fn test_scoring_coding_task_prefers_coding_and_reasoning_models() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Write a high-performance memory allocator in Rust using unsafe pointers"
            .to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision: RoutingPreviewResponse = router.preview(&req).unwrap();
    assert_eq!(decision.detected_task, "coding_and_debugging");
    assert_eq!(decision.selected_model_id, "deep-reasoner-v1");
    assert_eq!(decision.provider_id, "openrouter-api");
    assert!(
        decision.selection_reason.contains("reasoning")
            || decision.selection_reason.contains("programming")
    );
    // fast-chat-v1 also supports code and coding capability, so it must be a fallback
    assert!(
        decision
            .fallback_model_ids
            .contains(&"fast-chat-v1".to_string())
    );
    // balanced-assistant-v1 only supports text, so it must NOT be in fallbacks
    assert!(
        !decision
            .fallback_model_ids
            .contains(&"balanced-assistant-v1".to_string())
    );
}

#[test]
fn test_scoring_architecture_task_prefers_deep_reasoning() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Design a globally distributed multi-region database replication topology"
            .to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision: RoutingPreviewResponse = router.preview(&req).unwrap();
    assert_eq!(decision.detected_task, "architecture_and_design");
    assert_eq!(decision.estimated_complexity, "high");
    assert_eq!(decision.selected_model_id, "deep-reasoner-v1");
    assert!(
        decision
            .fallback_model_ids
            .contains(&"fast-chat-v1".to_string())
    );
    assert!(
        decision
            .fallback_model_ids
            .contains(&"balanced-assistant-v1".to_string())
    );
}

#[test]
fn test_scoring_fast_reasoning_prefers_fast_speed_tier() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "what is Tokio?".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision: RoutingPreviewResponse = router.preview(&req).unwrap();
    assert_eq!(decision.detected_task, "fast_reasoning");
    assert_eq!(decision.estimated_complexity, "low");
    assert_eq!(decision.selected_model_id, "fast-chat-v1");
    assert_eq!(decision.provider_id, "groq-api");
    assert!(
        decision.selection_reason.contains("fast") || decision.selection_reason.contains("factual")
    );
}

#[test]
fn test_scoring_general_conversation_prefers_balanced_cost_effective_model() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Hello, good morning! It is wonderful to meet you.".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision: RoutingPreviewResponse = router.preview(&req).unwrap();
    assert_eq!(decision.detected_task, "general_conversation");
    assert_eq!(decision.estimated_complexity, "low");
    // balanced-assistant-v1 or fast-chat-v1 selected
    assert!(
        decision.selected_model_id == "balanced-assistant-v1"
            || decision.selected_model_id == "fast-chat-v1"
    );
}

// ============================================================================
// 2. Modality Hard Filtering & Exclusion
// ============================================================================

#[test]
fn test_modality_exclusion_in_auto_mode() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    // Requesting "code" modality should exclude text-only models
    let req = RoutingPreviewRequest {
        prompt: "Refactor this function to be thread-safe".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision = router.preview(&req).unwrap();
    // balanced-assistant-v1 is text-only, must NOT appear in selected or fallbacks
    assert_ne!(decision.selected_model_id, "balanced-assistant-v1");
    assert!(
        !decision
            .fallback_model_ids
            .contains(&"balanced-assistant-v1".to_string())
    );
}

#[test]
fn test_unsupported_modality_when_no_models_support_modality() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    // No pre-registered reference model supports "video"
    let req = RoutingPreviewRequest {
        prompt: "Generate a video of waves crashing against a cliff".to_string(),
        requested_modality: "video".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let err = router.preview(&req).unwrap_err();
    assert_eq!(err.status_code(), 422);
    assert_eq!(err.error_code(), "UNSUPPORTED_MODALITY");
    assert!(err.to_string().contains("video"));
}

// ============================================================================
// 3. Manual Model Selection & Validation
// ============================================================================

#[test]
fn test_manual_routing_valid_model() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Explain the actor model pattern".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("fast-chat-v1".to_string()),
    };

    let decision = router.preview(&req).unwrap();
    assert_eq!(decision.selected_model_id, "fast-chat-v1");
    assert_eq!(decision.provider_id, "groq-api");
    assert!(decision.selection_reason.contains("Manual selection"));
    // Other available text-compatible models must appear as fallbacks
    assert!(
        decision
            .fallback_model_ids
            .contains(&"balanced-assistant-v1".to_string())
    );
    assert!(
        decision
            .fallback_model_ids
            .contains(&"deep-reasoner-v1".to_string())
    );
    assert!(
        !decision
            .fallback_model_ids
            .contains(&"fast-chat-v1".to_string())
    );
}

#[test]
fn test_manual_routing_missing_preferred_model_id_rejected() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Hello".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: None,
    };

    let err = router.preview(&req).unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert_eq!(err.error_code(), "INVALID_REQUEST");
    assert!(err.to_string().contains("preferred_model_id is required"));
}

#[test]
fn test_manual_routing_nonexistent_model_returns_404() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Hello".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("ghost-model-999".to_string()),
    };

    let err = router.preview(&req).unwrap_err();
    assert_eq!(err.status_code(), 404);
    assert_eq!(err.error_code(), "NOT_FOUND");
}

#[test]
fn test_manual_routing_unavailable_model_returns_502() {
    let reg = Arc::new(ModelRegistry::new());
    reg.set_model_availability("fast-chat-v1", false).unwrap();
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Hello".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("fast-chat-v1".to_string()),
    };

    let err = router.preview(&req).unwrap_err();
    assert_eq!(err.status_code(), 502);
    assert_eq!(err.error_code(), "UPSTREAM_FAILURE");
    assert!(matches!(err, AppError::UpstreamFailure(ref msg) if msg.contains("unavailable")));
}

#[test]
fn test_manual_routing_modality_mismatch_returns_422() {
    let reg = Arc::new(ModelRegistry::new());
    let router = ModelRouter::new(reg);

    // balanced-assistant-v1 only supports "text", not "code"
    let req = RoutingPreviewRequest {
        prompt: "Write a function".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("balanced-assistant-v1".to_string()),
    };

    let err = router.preview(&req).unwrap_err();
    assert_eq!(err.status_code(), 422);
    assert_eq!(err.error_code(), "UNSUPPORTED_MODALITY");
    assert!(err.to_string().contains("modality"));
}

// ============================================================================
// 4. Safe Fallback Ordering When Preferred Model is Unavailable
// ============================================================================

#[test]
fn test_auto_routing_skips_unavailable_models_to_healthy_fallback() {
    let reg = Arc::new(ModelRegistry::new());
    // Disable deep-reasoner-v1
    reg.set_model_availability("deep-reasoner-v1", false)
        .unwrap();
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Write a Rust function to parse an ELF binary header.".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    // Since deep-reasoner-v1 is unavailable, fast-chat-v1 (which also supports code)
    // must be selected as the primary!
    let decision = router.preview(&req).unwrap();
    assert_eq!(decision.selected_model_id, "fast-chat-v1");
    assert_eq!(decision.provider_id, "groq-api");
    // deep-reasoner-v1 must NOT be in fallbacks because it is unavailable
    assert!(
        !decision
            .fallback_model_ids
            .contains(&"deep-reasoner-v1".to_string())
    );
}

#[test]
fn test_no_operational_models_available_returns_502() {
    let reg = Arc::new(ModelRegistry::new());
    // Mark all code-capable models unavailable
    reg.set_model_availability("deep-reasoner-v1", false)
        .unwrap();
    reg.set_model_availability("fast-chat-v1", false).unwrap();
    let router = ModelRouter::new(reg);

    let req = RoutingPreviewRequest {
        prompt: "Write a Rust function".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let err = router.preview(&req).unwrap_err();
    assert_eq!(err.status_code(), 502);
    assert_eq!(err.error_code(), "UPSTREAM_FAILURE");
    assert!(matches!(
        err,
        AppError::UpstreamFailure(ref msg) if msg.contains("No operational model available")
    ));
}

// ============================================================================
// 5. Deterministic Tie-Breaking & Dynamic Custom Catalog
// ============================================================================

#[test]
fn test_deterministic_tie_breaking() {
    let reg = Arc::new(ModelRegistry::empty());

    // Register two models with identical capabilities and scores, but different cost tiers
    let m_free = make_test_model(
        "model-b-free",
        "provider-x",
        vec!["text"],
        "fast",
        "free-tier",
        true,
        vec!["simple_chat"],
        32768,
        4096,
    );
    let m_paid = make_test_model(
        "model-a-paid",
        "provider-x",
        vec!["text"],
        "fast",
        "low-cost",
        true,
        vec!["simple_chat"],
        32768,
        4096,
    );

    reg.register_model(m_paid).unwrap();
    reg.register_model(m_free).unwrap();

    let router = ModelRouter::new(reg);
    let req = RoutingPreviewRequest {
        prompt: "Hello there".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision = router.preview(&req).unwrap();
    // model-b-free must win because free-tier is preferred over low-cost
    assert_eq!(decision.selected_model_id, "model-b-free");
    assert_eq!(
        decision.fallback_model_ids,
        vec!["model-a-paid".to_string()]
    );
}

#[test]
fn test_alphabetical_tie_breaking_for_identical_scores() {
    let reg = Arc::new(ModelRegistry::empty());

    // Register two completely identical models differing only by model_id
    let m1 = make_test_model(
        "alpha-model",
        "provider-x",
        vec!["text"],
        "fast",
        "free-tier",
        true,
        vec!["simple_chat"],
        32768,
        4096,
    );
    let m2 = make_test_model(
        "beta-model",
        "provider-x",
        vec!["text"],
        "fast",
        "free-tier",
        true,
        vec!["simple_chat"],
        32768,
        4096,
    );

    reg.register_model(m2).unwrap();
    reg.register_model(m1).unwrap();

    let router = ModelRouter::new(reg);
    let req = RoutingPreviewRequest {
        prompt: "Hello".to_string(),
        requested_modality: "text".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision = router.preview(&req).unwrap();
    // "alpha-model" comes before "beta-model" alphabetically
    assert_eq!(decision.selected_model_id, "alpha-model");
    assert_eq!(decision.fallback_model_ids, vec!["beta-model".to_string()]);
}

// ============================================================================
// 6. Multimodal Custom Catalog Routing
// ============================================================================

#[test]
fn test_multimodal_image_and_video_model_routing() {
    let reg = Arc::new(ModelRegistry::empty());

    let img_model = make_test_model(
        "flux-pro-v1",
        "bfl-api",
        vec!["text", "image"],
        "balanced",
        "low-cost",
        true,
        vec!["image_generation"],
        16384,
        2048,
    );
    let vid_model = make_test_model(
        "kling-video-v1",
        "kuaishou-api",
        vec!["text", "video"],
        "advanced",
        "premium",
        true,
        vec!["video_generation"],
        16384,
        2048,
    );

    reg.register_model(img_model).unwrap();
    reg.register_model(vid_model).unwrap();

    let router = ModelRouter::new(reg);

    // 1. Image generation prompt
    let req_img = RoutingPreviewRequest {
        prompt: "Generate an image of a cybernetic dragon".to_string(),
        requested_modality: "image".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let decision_img = router.preview(&req_img).unwrap();
    assert_eq!(decision_img.detected_task, "image_generation");
    assert_eq!(decision_img.selected_model_id, "flux-pro-v1");
    assert_eq!(decision_img.provider_id, "bfl-api");

    // 2. Video generation prompt
    let req_vid = RoutingPreviewRequest {
        prompt: "Generate a video of a hummingbird in slow motion".to_string(),
        requested_modality: "video".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };
    let decision_vid = router.preview(&req_vid).unwrap();
    assert_eq!(decision_vid.detected_task, "video_generation");
    assert_eq!(decision_vid.selected_model_id, "kling-video-v1");
    assert_eq!(decision_vid.provider_id, "kuaishou-api");
}

// ============================================================================
// 7. Modality Semantics: Text-Output Models with Coding Capability
// ============================================================================

#[test]
fn test_text_output_model_with_coding_capability_eligible_for_code_modality() {
    let reg = Arc::new(ModelRegistry::empty());

    // Model 1: Declares only "text" modality, but explicitly has "coding" capability
    let text_coder = make_test_model(
        "text-coder-v1",
        "provider-c",
        vec!["text"],
        "balanced",
        "low-cost",
        true,
        vec!["coding"],
        32768,
        4096,
    );

    // Model 2: Declares only "text" modality, but lacks "coding" capability
    let text_general = make_test_model(
        "text-general-v1",
        "provider-c",
        vec!["text"],
        "fast",
        "free-tier",
        true,
        vec!["simple_chat", "general_conversation"],
        16384,
        2048,
    );

    reg.register_model(text_coder).unwrap();
    reg.register_model(text_general).unwrap();

    let router = ModelRouter::new(reg);

    // 1. Auto routing with requested_modality: "code"
    let req_auto = RoutingPreviewRequest {
        prompt: "Write a Rust binary parser".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "auto".to_string(),
        preferred_model_id: None,
    };

    let decision = router.preview(&req_auto).unwrap();
    // text-coder-v1 is recognized as code-capable despite modality terminology
    assert_eq!(decision.selected_model_id, "text-coder-v1");
    // text-general-v1 does NOT have coding capability, so it must be excluded
    assert!(
        !decision
            .fallback_model_ids
            .contains(&"text-general-v1".to_string())
    );

    // 2. Manual routing with text-coder-v1 and requested_modality: "code"
    let req_manual_ok = RoutingPreviewRequest {
        prompt: "Write a function".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("text-coder-v1".to_string()),
    };
    let decision_manual = router.preview(&req_manual_ok).unwrap();
    assert_eq!(decision_manual.selected_model_id, "text-coder-v1");

    // 3. Manual routing with text-general-v1 and requested_modality: "code" -> 422
    let req_manual_bad = RoutingPreviewRequest {
        prompt: "Write a function".to_string(),
        requested_modality: "code".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("text-general-v1".to_string()),
    };
    let err_bad = router.preview(&req_manual_bad).unwrap_err();
    assert_eq!(err_bad.status_code(), 422);
    assert_eq!(err_bad.error_code(), "UNSUPPORTED_MODALITY");

    // 4. Genuine binary modality (video) constraint remains strictly enforced -> 422
    let req_video = RoutingPreviewRequest {
        prompt: "Generate video".to_string(),
        requested_modality: "video".to_string(),
        routing_mode: "manual".to_string(),
        preferred_model_id: Some("text-coder-v1".to_string()),
    };
    let err_video = router.preview(&req_video).unwrap_err();
    assert_eq!(err_video.status_code(), 422);
    assert_eq!(err_video.error_code(), "UNSUPPORTED_MODALITY");
}
