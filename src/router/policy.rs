//! Multi-criteria candidate scoring and intelligent model routing policy.
//!
//! Conforms strictly to MASTER_SPEC.md §6, §18 Phase 7, ARCHITECTURE.md §2, and DEC-010.

use crate::models::ModelMetadata;
use crate::router::classifier::{TaskCategory, TaskComplexity, TaskModality};
use std::cmp::Ordering;

/// Scored candidate model resulting from multi-criteria evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredCandidate {
    pub model_id: String,
    pub provider_id: String,
    pub total_score: i32,
    pub capability_score: i32,
    pub complexity_score: i32,
    pub speed_score: i32,
    pub cost_score: i32,
    pub capacity_score: i32,
    pub matched_capabilities: Vec<String>,
    pub selection_reason: String,
}

/// Routing policy engine evaluating candidate models.
pub struct RoutingPolicy;

impl RoutingPolicy {
    /// Check whether a model supports the requested modality.
    ///
    /// Modality matching rules:
    /// - Direct match: Model's `supported_modalities` explicitly contains the requested modality string.
    /// - Semantic compatibility: In this architecture, `code` is physically transmitted as text. A model
    ///   with `supported_modalities: ["text"]` that explicitly declares the `"coding"` capability
    ///   is recognized as supporting code tasks when `requested_modality: "code"` is provided.
    /// - Strict boundaries: Genuine multimodal formats (`image`, `video`, `audio`) strictly require
    ///   the matching explicit modality declared in `supported_modalities`.
    pub fn supports_modality(model: &ModelMetadata, modality: &TaskModality) -> bool {
        let mod_str = modality.as_str();
        if model
            .supported_modalities
            .iter()
            .any(|m| m.eq_ignore_ascii_case(mod_str))
        {
            return true;
        }

        // Semantic compatibility: Text-output models declaring "coding" capability support code generation
        if *modality == TaskModality::Code
            && model
                .supported_modalities
                .iter()
                .any(|m| m.eq_ignore_ascii_case("text"))
            && model
                .capabilities
                .iter()
                .any(|c| c.eq_ignore_ascii_case("coding"))
        {
            return true;
        }

        false
    }

    /// Evaluate and score a single model candidate against task, complexity, and modality.
    /// Returns `None` if the model does not support the requested modality.
    pub fn score_candidate(
        model: &ModelMetadata,
        task: &TaskCategory,
        complexity: &TaskComplexity,
        requested_modality: &TaskModality,
    ) -> Option<ScoredCandidate> {
        // 1. Hard Prerequisite: Modality Compatibility
        if !Self::supports_modality(model, requested_modality) {
            return None;
        }

        // 2. Multi-Criteria Scoring Components
        let (capability_score, matched_caps) = Self::compute_capability_score(model, task);
        let complexity_score = Self::compute_complexity_score(model, complexity);
        let speed_score = Self::compute_speed_score(model, complexity);
        let cost_score = Self::compute_cost_score(model);
        let capacity_score = Self::compute_capacity_score(model, complexity);

        let total_score =
            capability_score + complexity_score + speed_score + cost_score + capacity_score;
        let selection_reason =
            Self::generate_selection_reason(model, task, complexity, &matched_caps);

        Some(ScoredCandidate {
            model_id: model.model_id.clone(),
            provider_id: model.provider_id.clone(),
            total_score,
            capability_score,
            complexity_score,
            speed_score,
            cost_score,
            capacity_score,
            matched_capabilities: matched_caps,
            selection_reason,
        })
    }

    /// Rank all eligible candidates from a model slice, sorted by descending score
    /// with deterministic tie-breaking.
    pub fn rank_candidates(
        models: &[ModelMetadata],
        task: &TaskCategory,
        complexity: &TaskComplexity,
        requested_modality: &TaskModality,
    ) -> Vec<ScoredCandidate> {
        let mut scored: Vec<(ScoredCandidate, &ModelMetadata)> = models
            .iter()
            .filter(|m| m.available)
            .filter_map(|m| {
                Self::score_candidate(m, task, complexity, requested_modality).map(|sc| (sc, m))
            })
            .collect();

        // Sort descending by score, applying deterministic tie-breaking
        scored.sort_by(|(a_sc, a_m), (b_sc, b_m)| {
            Self::compare_candidates(a_sc, b_sc, a_m, b_m, complexity)
        });

        scored.into_iter().map(|(sc, _)| sc).collect()
    }

    /// Deterministic candidate comparison for sorting.
    pub fn compare_candidates(
        a_sc: &ScoredCandidate,
        b_sc: &ScoredCandidate,
        a_meta: &ModelMetadata,
        b_meta: &ModelMetadata,
        complexity: &TaskComplexity,
    ) -> Ordering {
        // Primary: Total Score descending
        b_sc.total_score
            .cmp(&a_sc.total_score)
            // Secondary: Cost Tier preference (free-tier > low-cost > premium)
            .then_with(|| {
                let cost_rank = |cost: &str| match cost.to_ascii_lowercase().as_str() {
                    "free-tier" => 3,
                    "low-cost" => 2,
                    _ => 1,
                };
                cost_rank(&b_meta.cost_tier).cmp(&cost_rank(&a_meta.cost_tier))
            })
            // Tertiary: Speed Tier preference (context-dependent)
            .then_with(|| {
                let speed_rank =
                    |speed: &str| match (complexity, speed.to_ascii_lowercase().as_str()) {
                        (TaskComplexity::Low, "fast") => 3,
                        (TaskComplexity::Low, "balanced") => 2,
                        (TaskComplexity::Low, _) => 1,
                        (TaskComplexity::High, "advanced") => 3,
                        (TaskComplexity::High, "balanced") => 2,
                        (TaskComplexity::High, _) => 1,
                        (TaskComplexity::Medium, "balanced") => 3,
                        (TaskComplexity::Medium, "advanced") => 2,
                        (TaskComplexity::Medium, _) => 1,
                    };
                speed_rank(&b_meta.speed_tier).cmp(&speed_rank(&a_meta.speed_tier))
            })
            // Quaternary: Context Window size descending
            .then_with(|| b_meta.context_window.cmp(&a_meta.context_window))
            // Final: Alphabetical model_id ascending for absolute determinism
            .then_with(|| a_sc.model_id.cmp(&b_sc.model_id))
    }

    /// Compute capability score and collect matched capability tags.
    fn compute_capability_score(model: &ModelMetadata, task: &TaskCategory) -> (i32, Vec<String>) {
        let mut score = 0;
        let mut matched = Vec::new();
        let caps = &model.capabilities;

        let has_cap = |tag: &str| -> bool { caps.iter().any(|c| c.eq_ignore_ascii_case(tag)) };

        match task {
            TaskCategory::CodingAndDebugging => {
                if has_cap("coding") {
                    score += 40;
                    matched.push("coding".to_string());
                }
                if has_cap("deep_reasoning") {
                    score += 10;
                    matched.push("deep_reasoning".to_string());
                }
            }
            TaskCategory::ArchitectureAndDesign => {
                if has_cap("deep_reasoning") {
                    score += 40;
                    matched.push("deep_reasoning".to_string());
                }
                if has_cap("complex_analysis") {
                    score += 15;
                    matched.push("complex_analysis".to_string());
                }
                if has_cap("coding") {
                    score += 5;
                    matched.push("coding".to_string());
                }
            }
            TaskCategory::ComplexAnalysis => {
                if has_cap("complex_analysis") {
                    score += 40;
                    matched.push("complex_analysis".to_string());
                }
                if has_cap("deep_reasoning") {
                    score += 15;
                    matched.push("deep_reasoning".to_string());
                }
            }
            TaskCategory::MathematicsAndCalculation => {
                if has_cap("deep_reasoning") {
                    score += 40;
                    matched.push("deep_reasoning".to_string());
                }
                if has_cap("complex_analysis") {
                    score += 10;
                    matched.push("complex_analysis".to_string());
                }
            }
            TaskCategory::FastReasoning => {
                if has_cap("fast_reasoning") {
                    score += 45;
                    matched.push("fast_reasoning".to_string());
                }
                if has_cap("simple_chat") {
                    score += 10;
                    matched.push("simple_chat".to_string());
                }
            }
            TaskCategory::SimpleQuestions => {
                if has_cap("simple_chat") {
                    score += 45;
                    matched.push("simple_chat".to_string());
                }
                if has_cap("general_conversation") {
                    score += 35;
                    matched.push("general_conversation".to_string());
                }
            }
            TaskCategory::GeneralConversation => {
                if has_cap("general_conversation") {
                    score += 45;
                    matched.push("general_conversation".to_string());
                }
                if has_cap("simple_chat") {
                    score += 40;
                    matched.push("simple_chat".to_string());
                }
            }
            TaskCategory::DocumentCreation => {
                if has_cap("document_creation") {
                    score += 45;
                    matched.push("document_creation".to_string());
                }
                if model
                    .supported_modalities
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case("document"))
                {
                    score += 15;
                    matched.push("modality:document".to_string());
                }
            }
            TaskCategory::PowerPointGeneration => {
                if has_cap("powerpoint_generation") {
                    score += 45;
                    matched.push("powerpoint_generation".to_string());
                }
                if has_cap("document_creation") {
                    score += 25;
                    matched.push("document_creation".to_string());
                }
            }
            TaskCategory::DataExtraction => {
                if has_cap("data_extraction") {
                    score += 45;
                    matched.push("data_extraction".to_string());
                }
                if has_cap("coding") {
                    score += 20;
                    matched.push("coding".to_string());
                }
                if has_cap("fast_reasoning") {
                    score += 10;
                    matched.push("fast_reasoning".to_string());
                }
            }
            TaskCategory::ImageGeneration => {
                if has_cap("image_generation") {
                    score += 50;
                    matched.push("image_generation".to_string());
                }
            }
            TaskCategory::ImageEditing => {
                if has_cap("image_editing") {
                    score += 50;
                    matched.push("image_editing".to_string());
                }
            }
            TaskCategory::VideoGeneration => {
                if has_cap("video_generation") {
                    score += 50;
                    matched.push("video_generation".to_string());
                }
            }
            TaskCategory::AudioProcessing => {
                if has_cap("audio_processing") {
                    score += 50;
                    matched.push("audio_processing".to_string());
                }
            }
            TaskCategory::WebSearchAndResearch => {
                if has_cap("web_search") {
                    score += 50;
                    matched.push("web_search".to_string());
                }
                if has_cap("fast_reasoning") {
                    score += 25;
                    matched.push("fast_reasoning".to_string());
                }
            }
        }

        // Baseline generic fallback capability score if no primary matches
        if score == 0 {
            score = 10;
        }

        (score, matched)
    }

    /// Compute complexity alignment score.
    fn compute_complexity_score(model: &ModelMetadata, complexity: &TaskComplexity) -> i32 {
        let speed = model.speed_tier.to_ascii_lowercase();
        match complexity {
            TaskComplexity::High => match speed.as_str() {
                "advanced" => 30,
                "balanced" => 10,
                _ => 5,
            },
            TaskComplexity::Medium => match speed.as_str() {
                "balanced" => 25,
                "advanced" => 20,
                _ => 15,
            },
            TaskComplexity::Low => match speed.as_str() {
                "fast" => 30,
                "balanced" => 15,
                _ => 0, // Discourage expensive/advanced models on low-complexity tasks
            },
        }
    }

    /// Compute speed and latency suitability score.
    fn compute_speed_score(model: &ModelMetadata, complexity: &TaskComplexity) -> i32 {
        let speed = model.speed_tier.to_ascii_lowercase();
        match complexity {
            TaskComplexity::Low => match speed.as_str() {
                "fast" => 15,
                "balanced" => 8,
                _ => 0,
            },
            TaskComplexity::Medium => match speed.as_str() {
                "balanced" => 15,
                "fast" => 10,
                _ => 10,
            },
            TaskComplexity::High => match speed.as_str() {
                "advanced" => 15,
                "balanced" => 8,
                _ => 5,
            },
        }
    }

    /// Compute cost tier preference score (prefer free or low-cost per MASTER_SPEC.md §2.9).
    fn compute_cost_score(model: &ModelMetadata) -> i32 {
        match model.cost_tier.to_ascii_lowercase().as_str() {
            "free-tier" => 15,
            "low-cost" => 10,
            _ => 2,
        }
    }

    /// Compute capacity score (context window & token depth).
    fn compute_capacity_score(model: &ModelMetadata, complexity: &TaskComplexity) -> i32 {
        let mut score = 0;
        match complexity {
            TaskComplexity::High => {
                if model.context_window >= 65536 {
                    score += 5;
                }
                if model.max_output_tokens >= 8192 {
                    score += 5;
                }
            }
            TaskComplexity::Medium => {
                if model.context_window >= 32768 {
                    score += 3;
                }
                if model.max_output_tokens >= 4096 {
                    score += 2;
                }
            }
            TaskComplexity::Low => {
                // Generous context is not prioritized for low-complexity queries
                score += 1;
            }
        }
        score
    }

    /// Generate transparent, explainable selection reason.
    fn generate_selection_reason(
        model: &ModelMetadata,
        task: &TaskCategory,
        _complexity: &TaskComplexity,
        matched_caps: &[String],
    ) -> String {
        match task {
            TaskCategory::CodingAndDebugging => {
                if model.speed_tier.eq_ignore_ascii_case("advanced")
                    || matched_caps.contains(&"deep_reasoning".to_string())
                {
                    "Task identified as complex systems programming requiring deep reasoning capability."
                        .to_string()
                } else {
                    format!(
                        "Coding task routed to '{}' matching programming capability.",
                        model.display_name
                    )
                }
            }
            TaskCategory::ArchitectureAndDesign => {
                "Systems architecture design query routed to deep reasoning model.".to_string()
            }
            TaskCategory::MathematicsAndCalculation => {
                "Mathematical computation query routed to deep reasoning model.".to_string()
            }
            TaskCategory::ComplexAnalysis => {
                "High-complexity analytical reasoning query routed to advanced reasoning model."
                    .to_string()
            }
            TaskCategory::FastReasoning => {
                "Low-latency factual query routed to fast balanced inference tier.".to_string()
            }
            TaskCategory::SimpleQuestions => {
                "Low-latency question routed to fast balanced inference tier.".to_string()
            }
            TaskCategory::GeneralConversation => {
                "Standard conversational query routed to balanced general assistant.".to_string()
            }
            TaskCategory::DocumentCreation => {
                format!(
                    "Document creation query routed to '{}' with document handling capability.",
                    model.display_name
                )
            }
            TaskCategory::PowerPointGeneration => {
                format!(
                    "Presentation generation task routed to '{}' matching structured output capability.",
                    model.display_name
                )
            }
            TaskCategory::DataExtraction => {
                format!(
                    "Data extraction query routed to '{}' with structured data extraction capability.",
                    model.display_name
                )
            }
            TaskCategory::ImageGeneration => {
                format!(
                    "Image generation task routed to '{}' with image synthesis capability.",
                    model.display_name
                )
            }
            TaskCategory::ImageEditing => {
                format!(
                    "Image editing task routed to '{}' with image manipulation capability.",
                    model.display_name
                )
            }
            TaskCategory::VideoGeneration => {
                format!(
                    "Video generation task routed to '{}' with video synthesis capability.",
                    model.display_name
                )
            }
            TaskCategory::AudioProcessing => {
                format!(
                    "Audio task routed to '{}' with audio processing capability.",
                    model.display_name
                )
            }
            TaskCategory::WebSearchAndResearch => {
                format!(
                    "Search and research task routed to '{}' with search retrieval capability.",
                    model.display_name
                )
            }
        }
    }
}
