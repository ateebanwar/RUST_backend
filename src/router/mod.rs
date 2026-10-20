//! Intelligent task classification and model routing policy engine.
//!
//! Conforms to ARCHITECTURE.md §2 and API_SPEC.md §4.

use crate::errors::AppError;
use crate::registry::ModelRegistry;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Inbound request schema for POST /api/v1/routing/preview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingPreviewRequest {
    pub prompt: String,
    #[serde(default = "default_modality")]
    pub requested_modality: String,
    #[serde(default = "default_routing_mode")]
    pub routing_mode: String, // "auto" or "manual"
    pub preferred_model_id: Option<String>,
}

fn default_modality() -> String {
    "text".to_string()
}

fn default_routing_mode() -> String {
    "auto".to_string()
}

/// Outbound response schema matching API_SPEC.md §4.1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutingPreviewResponse {
    pub detected_task: String,
    pub estimated_complexity: String,
    pub selected_model_id: String,
    pub provider_id: String,
    pub selection_reason: String,
    pub fallback_model_ids: Vec<String>,
}

pub mod classifier;
pub mod policy;

pub use classifier::{
    ClassificationResult, MAX_PROMPT_LENGTH, TaskCategory, TaskClassifier, TaskComplexity,
    TaskModality,
};
pub use policy::{RoutingPolicy, ScoredCandidate};

/// Intelligent Model Router selecting candidate models according to policy.
pub struct ModelRouter {
    registry: Arc<ModelRegistry>,
}

impl ModelRouter {
    pub fn new(registry: Arc<ModelRegistry>) -> Self {
        Self { registry }
    }

    /// Access the underlying model registry.
    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// Preview routing decision without invoking upstream providers.
    pub fn preview(&self, req: &RoutingPreviewRequest) -> Result<RoutingPreviewResponse, AppError> {
        let validated_prompt = TaskClassifier::validate_prompt(&req.prompt)?;
        let parsed_modality = TaskModality::from_str_normalized(&req.requested_modality)?;

        if !req.routing_mode.eq_ignore_ascii_case("auto")
            && !req.routing_mode.eq_ignore_ascii_case("manual")
        {
            return Err(AppError::InvalidRequest(format!(
                "Invalid routing_mode '{}'. Must be 'auto' or 'manual'",
                req.routing_mode
            )));
        }

        let classification =
            TaskClassifier::classify_structured(validated_prompt, parsed_modality.as_str())?;
        let detected_task = classification.category.as_str().to_string();
        let estimated_complexity = classification.complexity.as_str().to_string();

        if req.routing_mode.eq_ignore_ascii_case("manual") {
            let preferred_id = req.preferred_model_id.as_deref().ok_or_else(|| {
                AppError::InvalidRequest(
                    "preferred_model_id is required when routing_mode is 'manual'".to_string(),
                )
            })?;

            let model = self.registry.get_model(preferred_id).ok_or_else(|| {
                AppError::NotFound(format!(
                    "Preferred model '{preferred_id}' not found in registry"
                ))
            })?;

            if !model.available {
                return Err(AppError::UpstreamFailure(format!(
                    "Model '{preferred_id}' is currently marked unavailable"
                )));
            }

            if !RoutingPolicy::supports_modality(&model, &parsed_modality) {
                return Err(AppError::UnsupportedModality(format!(
                    "Preferred model '{preferred_id}' does not support requested modality '{}'",
                    parsed_modality.as_str()
                )));
            }

            // Rank remaining available compatible models as fallback candidates
            let all_available = self
                .registry
                .query_models(&crate::registry::ModelFilter::new().with_available_only(true));
            let fallbacks: Vec<String> = RoutingPolicy::rank_candidates(
                &all_available,
                &classification.category,
                &classification.complexity,
                &parsed_modality,
            )
            .into_iter()
            .filter(|c| c.model_id != model.model_id)
            .map(|c| c.model_id)
            .collect();

            Ok(RoutingPreviewResponse {
                detected_task,
                estimated_complexity,
                selected_model_id: model.model_id.clone(),
                provider_id: model.provider_id.clone(),
                selection_reason: format!(
                    "Manual selection routed to user-specified model '{preferred_id}'."
                ),
                fallback_model_ids: fallbacks,
            })
        } else {
            // Retrieve all models to check if the requested modality is supported by any model in catalog
            let all_models = self
                .registry
                .query_models(&crate::registry::ModelFilter::new());
            let any_modality_supported = all_models
                .iter()
                .any(|m| RoutingPolicy::supports_modality(m, &parsed_modality));

            if !any_modality_supported {
                return Err(AppError::UnsupportedModality(format!(
                    "No models in catalog support requested modality '{}'",
                    parsed_modality.as_str()
                )));
            }

            // Retrieve all available models from registry
            let available_models = self
                .registry
                .query_models(&crate::registry::ModelFilter::new().with_available_only(true));

            // Rank candidates using multi-criteria scoring
            let scored = RoutingPolicy::rank_candidates(
                &available_models,
                &classification.category,
                &classification.complexity,
                &parsed_modality,
            );

            if scored.is_empty() {
                return Err(AppError::UpstreamFailure(format!(
                    "No operational model available matching task '{}' and modality '{}'",
                    classification.category.as_str(),
                    parsed_modality.as_str()
                )));
            }

            let best = &scored[0];
            let fallbacks: Vec<String> = scored[1..].iter().map(|c| c.model_id.clone()).collect();

            Ok(RoutingPreviewResponse {
                detected_task,
                estimated_complexity,
                selected_model_id: best.model_id.clone(),
                provider_id: best.provider_id.clone(),
                selection_reason: best.selection_reason.clone(),
                fallback_model_ids: fallbacks,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_classifier_code_detection() {
        let (task, complexity) =
            TaskClassifier::classify("fn parse_elf_header(data: &[u8]) -> Result<Header>", "code");
        assert_eq!(task, "coding_and_debugging");
        assert_eq!(complexity, "medium");

        let (task, complexity) =
            TaskClassifier::classify("Explain how async unsafe memory works in Rust", "text");
        assert_eq!(task, "coding_and_debugging");
        assert_eq!(complexity, "high");
    }

    #[test]
    fn test_task_classifier_complex_analysis() {
        let (task, complexity) = TaskClassifier::classify(
            "Prove step by step why P != NP or analyze the implications",
            "text",
        );
        assert_eq!(task, "complex_analysis");
        assert_eq!(complexity, "high");
    }

    #[test]
    fn test_task_classifier_fast_query() {
        let (task, complexity) = TaskClassifier::classify("what is Tokio?", "text");
        assert_eq!(task, "fast_reasoning");
        assert_eq!(complexity, "low");
    }

    #[test]
    fn test_auto_routing_preview() {
        let reg = Arc::new(ModelRegistry::new());
        let router = ModelRouter::new(reg);

        let req = RoutingPreviewRequest {
            prompt: "Write a Rust function to parse an ELF binary header.".to_string(),
            requested_modality: "code".to_string(),
            routing_mode: "auto".to_string(),
            preferred_model_id: None,
        };

        let res = router.preview(&req).unwrap();
        assert_eq!(res.detected_task, "coding_and_debugging");
        assert_eq!(res.selected_model_id, "deep-reasoner-v1");
        assert_eq!(res.provider_id, "openrouter-api");
        assert!(res.fallback_model_ids.contains(&"fast-chat-v1".to_string()));
    }

    #[test]
    fn test_manual_routing_preview() {
        let reg = Arc::new(ModelRegistry::new());
        let router = ModelRouter::new(reg);

        let req = RoutingPreviewRequest {
            prompt: "Hello".to_string(),
            requested_modality: "text".to_string(),
            routing_mode: "manual".to_string(),
            preferred_model_id: Some("fast-chat-v1".to_string()),
        };

        let res = router.preview(&req).unwrap();
        assert_eq!(res.selected_model_id, "fast-chat-v1");
        assert_eq!(res.provider_id, "groq-api");
    }

    #[test]
    fn test_manual_routing_nonexistent_model_returns_not_found() {
        let reg = Arc::new(ModelRegistry::new());
        let router = ModelRouter::new(reg);

        let req = RoutingPreviewRequest {
            prompt: "Hello".to_string(),
            requested_modality: "text".to_string(),
            routing_mode: "manual".to_string(),
            preferred_model_id: Some("invalid-model-xyz".to_string()),
        };

        let err = router.preview(&req).unwrap_err();
        assert_eq!(err.status_code(), 404);
        assert_eq!(err.error_code(), "NOT_FOUND");
    }
}
