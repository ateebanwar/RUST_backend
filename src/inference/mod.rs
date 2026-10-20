//! Core inference orchestration service.
//!
//! Coordinates the end-to-end execution flow:
//! Request Validation -> Task Classification -> Model Routing -> Provider Resolution ->
//! Bounded Failover Execution -> Normalized Response & Streaming.
//!
//! Conforms strictly to MASTER_SPEC.md §6 & §18, ARCHITECTURE.md §2, and API_SPEC.md §5 & §6.

use crate::config::AppConfig;
use crate::errors::AppError;
use crate::models::{ChatMessage, CompletionRequest, CompletionResponse};
use crate::providers::{ProviderError, ProviderRegistry, ProviderStream};
use crate::registry::{ModelFilter, ModelRegistry};
use crate::router::ModelRouter;
use crate::router::classifier::{TaskClassifier, TaskModality};
use crate::router::policy::RoutingPolicy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Inbound request payload for chat completion endpoints conforming to API_SPEC.md §5.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default = "default_routing_mode")]
    pub routing_mode: String,
    pub requested_model_id: Option<String>,
    #[serde(default = "default_modality")]
    pub requested_modality: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub stream: bool,
}

fn default_routing_mode() -> String {
    "auto".to_string()
}

fn default_modality() -> String {
    "text".to_string()
}

impl ChatCompletionRequest {
    /// Validate inbound chat completion request against API_SPEC.md §5.1 constraints.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.messages.is_empty() {
            return Err(AppError::InvalidRequest(
                "messages array cannot be empty. Must contain between 1 and 50 messages."
                    .to_string(),
            ));
        }

        if self.messages.len() > 50 {
            return Err(AppError::InvalidRequest(
                "messages array exceeds maximum limit of 50 messages.".to_string(),
            ));
        }

        for (idx, msg) in self.messages.iter().enumerate() {
            let role = msg.role.trim().to_ascii_lowercase();
            if role != "system" && role != "user" && role != "assistant" {
                return Err(AppError::InvalidRequest(format!(
                    "Invalid role '{}' at messages[{idx}]. Must be 'system', 'user', or 'assistant'.",
                    msg.role
                )));
            }

            if msg.content.trim().is_empty() {
                return Err(AppError::InvalidRequest(format!(
                    "Content cannot be empty at messages[{idx}]."
                )));
            }

            if msg.content.chars().count() > 100_000 {
                return Err(AppError::InvalidRequest(format!(
                    "Message content at messages[{idx}] exceeds 100,000 character ceiling."
                )));
            }
        }

        if self
            .temperature
            .is_some_and(|temp| !(0.0..=2.0).contains(&temp))
        {
            return Err(AppError::InvalidRequest(
                "temperature must be between 0.0 and 2.0".to_string(),
            ));
        }

        if self
            .max_tokens
            .is_some_and(|tokens| tokens == 0 || tokens > 32_768)
        {
            return Err(AppError::InvalidRequest(
                "max_tokens must be between 1 and 32,768".to_string(),
            ));
        }

        if !self.routing_mode.eq_ignore_ascii_case("auto")
            && !self.routing_mode.eq_ignore_ascii_case("manual")
        {
            return Err(AppError::InvalidRequest(format!(
                "Invalid routing_mode '{}'. Must be 'auto' or 'manual'.",
                self.routing_mode
            )));
        }

        if self.routing_mode.eq_ignore_ascii_case("manual")
            && self
                .requested_model_id
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(AppError::InvalidRequest(
                "requested_model_id is required when routing_mode is 'manual'".to_string(),
            ));
        }

        Ok(())
    }

    /// Extract prompt text for task classification (prefers last user message).
    pub fn prompt_for_classification(&self) -> &str {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role.eq_ignore_ascii_case("user"))
            .map(|m| m.content.as_str())
            .unwrap_or_else(|| {
                self.messages
                    .last()
                    .map(|m| m.content.as_str())
                    .unwrap_or("")
            })
    }
}

/// Routing resolution details for logging and SSE event framing.
#[derive(Debug, Clone)]
pub struct RoutingResolution {
    pub selected_model_id: String,
    pub provider_id: String,
    pub detected_task: String,
    pub estimated_complexity: String,
    pub selection_reason: String,
    pub ordered_candidates: Vec<CandidateExecutionTarget>,
}

/// Target candidate model and provider pair for execution.
#[derive(Debug, Clone)]
pub struct CandidateExecutionTarget {
    pub model_id: String,
    pub provider_id: String,
    pub reason: String,
}

/// Result of initiating a streaming response.
pub struct StreamExecutionResult {
    pub resolution: RoutingResolution,
    pub is_fallback: bool,
    pub stream: ProviderStream,
}

/// Core inference orchestration engine.
pub struct InferenceService {
    config: Arc<AppConfig>,
    registry: Arc<ModelRegistry>,
    providers: Arc<ProviderRegistry>,
    router: Arc<ModelRouter>,
    quota_service: Option<Arc<crate::quota::QuotaService>>,
}

impl InferenceService {
    /// Create a new InferenceService instance.
    pub fn new(
        config: Arc<AppConfig>,
        registry: Arc<ModelRegistry>,
        providers: Arc<ProviderRegistry>,
        router: Arc<ModelRouter>,
    ) -> Self {
        Self {
            config,
            registry,
            providers,
            router,
            quota_service: None,
        }
    }

    /// Builder method to attach a QuotaService for rate-limit-aware candidate prioritization.
    pub fn with_quota_service(mut self, quota_service: Arc<crate::quota::QuotaService>) -> Self {
        self.quota_service = Some(quota_service);
        self
    }

    /// Access underlying providers registry.
    pub fn providers(&self) -> &ProviderRegistry {
        &self.providers
    }

    /// Access underlying model router.
    pub fn router(&self) -> &ModelRouter {
        &self.router
    }

    /// Access underlying model registry.
    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// Resolve candidate execution targets based on request routing mode, policy scoring, and modalities.
    pub fn resolve_candidates(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<RoutingResolution, AppError> {
        let prompt = req.prompt_for_classification();
        let validated_prompt = TaskClassifier::validate_prompt(prompt)?;
        let parsed_modality = TaskModality::from_str_normalized(&req.requested_modality)?;

        let classification =
            TaskClassifier::classify_structured(validated_prompt, parsed_modality.as_str())?;
        let detected_task = classification.category.as_str().to_string();
        let estimated_complexity = classification.complexity.as_str().to_string();

        if req.routing_mode.eq_ignore_ascii_case("manual") {
            let preferred_id = match req.requested_model_id.as_deref() {
                Some(id) if !id.trim().is_empty() => id.trim(),
                _ => {
                    return Err(AppError::InvalidRequest(
                        "requested_model_id is required when routing_mode is 'manual'".to_string(),
                    ));
                }
            };
            let model = self.registry.get_model(preferred_id).ok_or_else(|| {
                AppError::NotFound(format!("Model '{preferred_id}' not found in registry"))
            })?;

            if !model.available {
                return Err(AppError::UpstreamFailure(format!(
                    "Model '{preferred_id}' is currently marked unavailable"
                )));
            }

            if !RoutingPolicy::supports_modality(&model, &parsed_modality) {
                return Err(AppError::UnsupportedModality(format!(
                    "Model '{preferred_id}' does not support requested modality '{}'",
                    parsed_modality.as_str()
                )));
            }

            let primary = CandidateExecutionTarget {
                model_id: model.model_id.clone(),
                provider_id: model.provider_id.clone(),
                reason: format!(
                    "Manual selection routed to user-specified model '{preferred_id}'."
                ),
            };

            // Rank remaining available models as fallback targets
            let all_available = self
                .registry
                .query_models(&ModelFilter::new().with_available_only(true));
            let mut ordered = vec![primary.clone()];
            let fallbacks = RoutingPolicy::rank_candidates(
                &all_available,
                &classification.category,
                &classification.complexity,
                &parsed_modality,
            );

            for fb in fallbacks {
                if fb.model_id != primary.model_id {
                    ordered.push(CandidateExecutionTarget {
                        model_id: fb.model_id,
                        provider_id: fb.provider_id,
                        reason: fb.selection_reason,
                    });
                }
            }

            Ok(RoutingResolution {
                selected_model_id: primary.model_id,
                provider_id: primary.provider_id,
                detected_task,
                estimated_complexity,
                selection_reason: primary.reason,
                ordered_candidates: ordered,
            })
        } else {
            // Automatic routing: verify catalog has models supporting modality
            let all_models = self.registry.query_models(&ModelFilter::new());
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
                .query_models(&ModelFilter::new().with_available_only(true));

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

            let mut ordered_candidates: Vec<CandidateExecutionTarget> = scored
                .iter()
                .map(|sc| CandidateExecutionTarget {
                    model_id: sc.model_id.clone(),
                    provider_id: sc.provider_id.clone(),
                    reason: sc.selection_reason.clone(),
                })
                .collect();

            // Conservative Quota Awareness: In automatic routing mode, deprioritize candidates
            // that are currently confirmed to be rate-limited, without altering relative priority
            // among non-rate-limited candidates.
            if let Some(quota_svc) = &self.quota_service {
                ordered_candidates.sort_by_key(|c| {
                    if quota_svc.is_rate_limited(&c.provider_id, Some(&c.model_id)) {
                        1
                    } else {
                        0
                    }
                });
            }

            let primary = &ordered_candidates[0];

            Ok(RoutingResolution {
                selected_model_id: primary.model_id.clone(),
                provider_id: primary.provider_id.clone(),
                detected_task,
                estimated_complexity,
                selection_reason: primary.reason.clone(),
                ordered_candidates,
            })
        }
    }

    /// Execute a standard buffered chat completion with bounded failover.
    pub async fn execute_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<CompletionResponse, AppError> {
        req.validate()?;
        let resolution = self.resolve_candidates(req)?;

        let timeout = Duration::from_secs(self.config.upstream_timeout_secs);
        let max_attempts = resolution.ordered_candidates.len().min(3); // bounded retry ceiling
        let mut last_error: Option<ProviderError> = None;

        for (attempt_idx, candidate) in resolution.ordered_candidates[..max_attempts]
            .iter()
            .enumerate()
        {
            let adapter = match self.providers.get(&candidate.provider_id) {
                Ok(a) => a,
                Err(err) => {
                    warn!(
                        provider_id = candidate.provider_id.as_str(),
                        model_id = candidate.model_id.as_str(),
                        error = %err,
                        "Skipping candidate due to adapter lookup failure"
                    );
                    last_error = Some(err);
                    continue;
                }
            };

            let completion_req = CompletionRequest {
                model_id: candidate.model_id.clone(),
                messages: req.messages.clone(),
                temperature: req.temperature,
                max_tokens: req.max_tokens,
                timeout,
                stream: false,
            };

            info!(
                attempt = attempt_idx + 1,
                provider_id = candidate.provider_id.as_str(),
                model_id = candidate.model_id.as_str(),
                "Dispatching completion to provider adapter"
            );

            match adapter.complete(&completion_req).await {
                Ok(mut response) => {
                    response.is_fallback = attempt_idx > 0;
                    response.routing_reason = Some(candidate.reason.clone());
                    return Ok(response);
                }
                Err(provider_err) => {
                    warn!(
                        attempt = attempt_idx + 1,
                        provider_id = candidate.provider_id.as_str(),
                        model_id = candidate.model_id.as_str(),
                        error = %provider_err,
                        "Provider adapter completion execution failed"
                    );

                    // Check if error is non-retryable (invalid request, bad parameters)
                    if let ProviderError::InvalidRequest(_) = provider_err {
                        return Err(provider_err.into());
                    }
                    if let ProviderError::CapabilityMismatch(_) = provider_err {
                        return Err(provider_err.into());
                    }

                    last_error = Some(provider_err);
                }
            }
        }

        // All candidates exhausted
        if let Some(err) = last_error {
            match err {
                ProviderError::ProviderNotFound(p) => Err(AppError::UpstreamFailure(format!(
                    "No active provider adapter registered for '{p}' to execute inference"
                ))),
                _ => Err(err.into()),
            }
        } else {
            Err(AppError::UpstreamFailure(
                "No active providers available to execute inference".to_string(),
            ))
        }
    }

    /// Initiate a streaming chat completion with bounded failover.
    pub async fn execute_stream(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<StreamExecutionResult, AppError> {
        req.validate()?;
        let resolution = self.resolve_candidates(req)?;

        let timeout = Duration::from_secs(self.config.upstream_timeout_secs);
        let max_attempts = resolution.ordered_candidates.len().min(3);
        let mut last_error: Option<ProviderError> = None;

        for (attempt_idx, candidate) in resolution.ordered_candidates[..max_attempts]
            .iter()
            .enumerate()
        {
            let adapter = match self.providers.get(&candidate.provider_id) {
                Ok(a) => a,
                Err(err) => {
                    warn!(
                        provider_id = candidate.provider_id.as_str(),
                        model_id = candidate.model_id.as_str(),
                        error = %err,
                        "Skipping stream candidate due to adapter lookup failure"
                    );
                    last_error = Some(err);
                    continue;
                }
            };

            let completion_req = CompletionRequest {
                model_id: candidate.model_id.clone(),
                messages: req.messages.clone(),
                temperature: req.temperature,
                max_tokens: req.max_tokens,
                timeout,
                stream: true,
            };

            match adapter.stream(&completion_req).await {
                Ok(stream) => {
                    let mut actual_resolution = resolution.clone();
                    actual_resolution.selected_model_id = candidate.model_id.clone();
                    actual_resolution.provider_id = candidate.provider_id.clone();
                    actual_resolution.selection_reason = candidate.reason.clone();

                    return Ok(StreamExecutionResult {
                        resolution: actual_resolution,
                        is_fallback: attempt_idx > 0,
                        stream,
                    });
                }
                Err(provider_err) => {
                    warn!(
                        attempt = attempt_idx + 1,
                        provider_id = candidate.provider_id.as_str(),
                        model_id = candidate.model_id.as_str(),
                        error = %provider_err,
                        "Provider adapter stream initiation failed"
                    );

                    if let ProviderError::InvalidRequest(_) = provider_err {
                        return Err(provider_err.into());
                    }
                    if let ProviderError::CapabilityMismatch(_) = provider_err {
                        return Err(provider_err.into());
                    }

                    last_error = Some(provider_err);
                }
            }
        }

        if let Some(err) = last_error {
            match err {
                ProviderError::ProviderNotFound(p) => Err(AppError::UpstreamFailure(format!(
                    "No active provider adapter registered for '{p}' to execute streaming inference"
                ))),
                _ => Err(err.into()),
            }
        } else {
            Err(AppError::UpstreamFailure(
                "No active providers available to execute streaming inference".to_string(),
            ))
        }
    }
}
