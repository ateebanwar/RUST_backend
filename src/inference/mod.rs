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
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub mod tokens;
pub use tokens::{
    BpeEncoding, EstimationMethod, MappingKind, ModelTokenizerResolution, TokenCompactionMetrics,
    TokenEstimate, TokenEstimator, TokenizerCapability, TokenizerFamily,
};

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
    pub estimated_prompt_tokens: usize,
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

        // Effective message compaction: ensure context estimation and admission evaluate
        // the exact message representation that will be dispatched to the model provider.
        let (effective_messages, _) = TokenEstimator::optimize_messages_for_model(
            &req.messages,
            req.requested_model_id.as_deref(),
        );

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

            // Output token capacity validation: do not silently reduce requested max_tokens
            let completion_allowance = match req.max_tokens {
                Some(tokens) => {
                    if tokens > model.max_output_tokens {
                        return Err(AppError::InvalidRequest(format!(
                            "Requested max_tokens ({tokens}) exceeds model '{preferred_id}' maximum output token limit of {}",
                            model.max_output_tokens
                        )));
                    }
                    tokens
                }
                None => 256.min(model.max_output_tokens),
            };

            let context_estimate = TokenEstimator::estimate_request_context_safe(
                &effective_messages,
                None,
                Some(&model.model_id),
            );
            let estimated_prompt_tokens = context_estimate.estimated_tokens;

            let total_budget = estimated_prompt_tokens
                .checked_add(completion_allowance)
                .ok_or_else(|| {
                    AppError::InvalidRequest("Token budget arithmetic overflow".to_string())
                })?;

            if total_budget > model.context_window {
                return Err(AppError::InvalidRequest(format!(
                    "Prompt estimated tokens ({estimated_prompt_tokens}) plus completion allowance ({completion_allowance}) exceed model '{preferred_id}' context window ({})",
                    model.context_window
                )));
            }

            let primary = CandidateExecutionTarget {
                model_id: model.model_id.clone(),
                provider_id: model.provider_id.clone(),
                reason: format!(
                    "Manual selection routed to user-specified model '{preferred_id}'."
                ),
            };

            Ok(RoutingResolution {
                selected_model_id: primary.model_id.clone(),
                provider_id: primary.provider_id.clone(),
                detected_task,
                estimated_complexity,
                selection_reason: primary.reason.clone(),
                ordered_candidates: vec![primary],
                estimated_prompt_tokens,
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

            // Validate requested output tokens against maximum capacity of operational models
            // before conflating with context window exhaustion.
            if let Some(requested) = req.max_tokens {
                let max_output_available = available_models
                    .iter()
                    .map(|m| m.max_output_tokens)
                    .max()
                    .unwrap_or(0);
                if requested > max_output_available {
                    return Err(AppError::InvalidRequest(format!(
                        "Requested max_tokens ({requested}) exceeds maximum output token limit of all available operational models ({max_output_available})"
                    )));
                }
            }

            // Capacity-aware context window filtering: only evaluate candidates that can fit prompt tokens + completion allowance
            let min_completion_allowance = req.max_tokens.unwrap_or(256);
            let fitting_models: Vec<crate::models::ModelMetadata> = available_models
                .into_iter()
                .filter(|m| {
                    let allowance = match req.max_tokens {
                        Some(requested) => {
                            if requested > m.max_output_tokens {
                                return false;
                            }
                            requested
                        }
                        None => 256.min(m.max_output_tokens),
                    };

                    let est = TokenEstimator::estimate_request_context_safe(
                        &effective_messages,
                        None,
                        Some(&m.model_id),
                    );

                    match est.estimated_tokens.checked_add(allowance) {
                        Some(total) => total <= m.context_window,
                        None => false,
                    }
                })
                .collect();

            if fitting_models.is_empty() {
                let fallback_prompt_tokens =
                    TokenEstimator::estimate_chat_for_model(&effective_messages, None);
                return Err(AppError::InvalidRequest(format!(
                    "Prompt estimated tokens ({fallback_prompt_tokens}) plus completion allowance ({min_completion_allowance}) exceed context window of all available operational models"
                )));
            }

            let scored = RoutingPolicy::rank_candidates(
                &fitting_models,
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
            let estimated_prompt_tokens = TokenEstimator::estimate_request_context_safe(
                &effective_messages,
                None,
                Some(&primary.model_id),
            )
            .estimated_tokens;

            Ok(RoutingResolution {
                selected_model_id: primary.model_id.clone(),
                provider_id: primary.provider_id.clone(),
                detected_task,
                estimated_complexity,
                selection_reason: primary.reason.clone(),
                ordered_candidates,
                estimated_prompt_tokens,
            })
        }
    }

    /// Execute a standard buffered chat completion with bounded retry, failover, and overall deadline enforcement.
    pub async fn execute_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<CompletionResponse, AppError> {
        req.validate()?;
        let (optimized_messages, compaction) = TokenEstimator::optimize_messages_for_model(
            &req.messages,
            req.requested_model_id.as_deref(),
        );
        if compaction.tokens_saved > 0 {
            info!(
                original_tokens = compaction.original_estimated_tokens,
                optimized_tokens = compaction.optimized_estimated_tokens,
                tokens_saved = compaction.tokens_saved,
                duplicates_removed = compaction.duplicate_system_messages_removed,
                "Optimized prompt tokens before provider dispatch"
            );
        }

        let resolution = self.resolve_candidates(req)?;

        let policy = self.config.retry_policy();
        let deadline = Instant::now() + Duration::from_secs(policy.overall_request_timeout_secs);
        let max_candidates = if req.routing_mode.eq_ignore_ascii_case("manual") {
            1
        } else {
            resolution
                .ordered_candidates
                .len()
                .min(policy.max_failover_candidates)
        };

        let mut failed_model_ids: HashSet<String> = HashSet::new();
        let mut last_error: Option<ProviderError> = None;
        let mut candidate_attempt_count = 0;

        for candidate in resolution.ordered_candidates.iter().take(max_candidates) {
            if failed_model_ids.contains(&candidate.model_id) {
                continue;
            }

            let adapter = match self.providers.get(&candidate.provider_id) {
                Ok(a) => a,
                Err(err) => {
                    warn!(
                        provider_id = candidate.provider_id.as_str(),
                        model_id = candidate.model_id.as_str(),
                        error = %err,
                        "Skipping candidate due to adapter lookup failure"
                    );
                    failed_model_ids.insert(candidate.model_id.clone());
                    last_error = Some(err);
                    continue;
                }
            };

            for retry_idx in 0..=policy.max_retries_per_candidate {
                let now = Instant::now();
                if now >= deadline {
                    return Err(AppError::GatewayTimeout(
                        "Overall request processing deadline exceeded".to_string(),
                    ));
                }
                let remaining = deadline - now;
                let attempt_timeout =
                    Duration::from_secs(self.config.upstream_timeout_secs).min(remaining);

                let completion_req = CompletionRequest {
                    model_id: candidate.model_id.clone(),
                    messages: optimized_messages.clone(),
                    temperature: req.temperature,
                    max_tokens: req.max_tokens,
                    timeout: attempt_timeout,
                    stream: false,
                };

                info!(
                    candidate_index = candidate_attempt_count + 1,
                    retry_attempt = retry_idx + 1,
                    provider_id = candidate.provider_id.as_str(),
                    model_id = candidate.model_id.as_str(),
                    "Dispatching completion to provider adapter"
                );

                match adapter.complete(&completion_req).await {
                    Ok(mut response) => {
                        response.is_fallback = candidate_attempt_count > 0;
                        response.routing_reason = Some(candidate.reason.clone());
                        return Ok(response);
                    }
                    Err(provider_err) => {
                        warn!(
                            candidate_index = candidate_attempt_count + 1,
                            retry_attempt = retry_idx + 1,
                            provider_id = candidate.provider_id.as_str(),
                            model_id = candidate.model_id.as_str(),
                            error = %provider_err,
                            "Provider adapter completion execution failed"
                        );

                        if crate::reliability::is_non_retryable_error(&provider_err) {
                            return Err(provider_err.into());
                        }

                        last_error = Some(provider_err.clone());

                        // Fast failover optimization: In automatic routing mode, if the upstream candidate
                        // hits confirmed rate limiting or quota exhaustion and an eligible alternative candidate with
                        // an operational registered provider remains, abandon this candidate immediately and switch candidates.
                        let has_alternative_candidate =
                            !req.routing_mode.eq_ignore_ascii_case("manual")
                                && resolution
                                    .ordered_candidates
                                    .iter()
                                    .take(max_candidates)
                                    .any(|c| {
                                        c.model_id != candidate.model_id
                                            && !failed_model_ids.contains(&c.model_id)
                                            && self.providers.get(&c.provider_id).is_ok()
                                    });

                        let should_fast_failover = has_alternative_candidate
                            && match &provider_err {
                                ProviderError::QuotaExhausted(_) => true,
                                ProviderError::RateLimited { retry_after_secs } => {
                                    // If Retry-After is > 0, immediate failover to ready candidate avoids unnecessary client latency
                                    retry_after_secs.map(|s| s > 0).unwrap_or(false)
                                }
                                _ => false,
                            };

                        if should_fast_failover {
                            info!(
                                provider_id = candidate.provider_id.as_str(),
                                model_id = candidate.model_id.as_str(),
                                "Upstream rate-limit/quota hit with positive delay; performing fast failover to next eligible candidate"
                            );
                            break;
                        }

                        if crate::reliability::is_transient_error(&provider_err)
                            && retry_idx < policy.max_retries_per_candidate
                        {
                            let retry_after = match &provider_err {
                                ProviderError::RateLimited { retry_after_secs } => {
                                    *retry_after_secs
                                }
                                _ => None,
                            };
                            let backoff = crate::reliability::calculate_backoff(
                                retry_idx,
                                retry_after,
                                &policy,
                            );
                            if backoff == Duration::MAX {
                                warn!(
                                    provider_id = candidate.provider_id.as_str(),
                                    model_id = candidate.model_id.as_str(),
                                    "Upstream Retry-After exceeds max allowable wait bound; abandoning candidate"
                                );
                                break;
                            }

                            let now_post = Instant::now();
                            if now_post + backoff >= deadline {
                                return Err(AppError::GatewayTimeout(
                                    "Overall request processing deadline exceeded during retry backoff"
                                        .to_string(),
                                ));
                            }
                            tokio::time::sleep(backoff).await;
                            continue;
                        }

                        break;
                    }
                }
            }

            failed_model_ids.insert(candidate.model_id.clone());
            candidate_attempt_count += 1;

            if req.routing_mode.eq_ignore_ascii_case("manual") {
                break;
            }
        }

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

    /// Initiate a streaming chat completion with bounded retry, failover, and overall deadline enforcement.
    pub async fn execute_stream(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<StreamExecutionResult, AppError> {
        req.validate()?;
        let (optimized_messages, compaction) = TokenEstimator::optimize_messages_for_model(
            &req.messages,
            req.requested_model_id.as_deref(),
        );
        if compaction.tokens_saved > 0 {
            info!(
                original_tokens = compaction.original_estimated_tokens,
                optimized_tokens = compaction.optimized_estimated_tokens,
                tokens_saved = compaction.tokens_saved,
                duplicates_removed = compaction.duplicate_system_messages_removed,
                "Optimized stream prompt tokens before provider dispatch"
            );
        }

        let resolution = self.resolve_candidates(req)?;

        let policy = self.config.retry_policy();
        let deadline = Instant::now() + Duration::from_secs(policy.overall_request_timeout_secs);
        let max_candidates = if req.routing_mode.eq_ignore_ascii_case("manual") {
            1
        } else {
            resolution
                .ordered_candidates
                .len()
                .min(policy.max_failover_candidates)
        };

        let mut failed_model_ids: HashSet<String> = HashSet::new();
        let mut last_error: Option<ProviderError> = None;
        let mut candidate_attempt_count = 0;

        for candidate in resolution.ordered_candidates.iter().take(max_candidates) {
            if failed_model_ids.contains(&candidate.model_id) {
                continue;
            }

            let adapter = match self.providers.get(&candidate.provider_id) {
                Ok(a) => a,
                Err(err) => {
                    warn!(
                        provider_id = candidate.provider_id.as_str(),
                        model_id = candidate.model_id.as_str(),
                        error = %err,
                        "Skipping stream candidate due to adapter lookup failure"
                    );
                    failed_model_ids.insert(candidate.model_id.clone());
                    last_error = Some(err);
                    continue;
                }
            };

            for retry_idx in 0..=policy.max_retries_per_candidate {
                let now = Instant::now();
                if now >= deadline {
                    return Err(AppError::GatewayTimeout(
                        "Overall request processing deadline exceeded".to_string(),
                    ));
                }
                let remaining = deadline - now;
                let attempt_timeout =
                    Duration::from_secs(self.config.upstream_timeout_secs).min(remaining);

                let completion_req = CompletionRequest {
                    model_id: candidate.model_id.clone(),
                    messages: optimized_messages.clone(),
                    temperature: req.temperature,
                    max_tokens: req.max_tokens,
                    timeout: attempt_timeout,
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
                            is_fallback: candidate_attempt_count > 0,
                            stream,
                        });
                    }
                    Err(provider_err) => {
                        warn!(
                            candidate_index = candidate_attempt_count + 1,
                            retry_attempt = retry_idx + 1,
                            provider_id = candidate.provider_id.as_str(),
                            model_id = candidate.model_id.as_str(),
                            error = %provider_err,
                            "Provider adapter stream initiation failed"
                        );

                        if crate::reliability::is_non_retryable_error(&provider_err) {
                            return Err(provider_err.into());
                        }

                        last_error = Some(provider_err.clone());

                        // Fast failover optimization: In automatic routing mode, if the upstream candidate
                        // hits confirmed rate limiting or quota exhaustion and an eligible alternative candidate with
                        // an operational registered provider remains, abandon this candidate immediately and switch candidates.
                        let has_alternative_candidate =
                            !req.routing_mode.eq_ignore_ascii_case("manual")
                                && resolution
                                    .ordered_candidates
                                    .iter()
                                    .take(max_candidates)
                                    .any(|c| {
                                        c.model_id != candidate.model_id
                                            && !failed_model_ids.contains(&c.model_id)
                                            && self.providers.get(&c.provider_id).is_ok()
                                    });

                        let should_fast_failover = has_alternative_candidate
                            && match &provider_err {
                                ProviderError::QuotaExhausted(_) => true,
                                ProviderError::RateLimited { retry_after_secs } => {
                                    // If Retry-After is > 0, immediate failover to ready candidate avoids unnecessary client latency
                                    retry_after_secs.map(|s| s > 0).unwrap_or(false)
                                }
                                _ => false,
                            };

                        if should_fast_failover {
                            info!(
                                provider_id = candidate.provider_id.as_str(),
                                model_id = candidate.model_id.as_str(),
                                "Upstream rate-limit/quota hit with positive delay; performing fast failover to next eligible candidate"
                            );
                            break;
                        }

                        if crate::reliability::is_transient_error(&provider_err)
                            && retry_idx < policy.max_retries_per_candidate
                        {
                            let retry_after = match &provider_err {
                                ProviderError::RateLimited { retry_after_secs } => {
                                    *retry_after_secs
                                }
                                _ => None,
                            };
                            let backoff = crate::reliability::calculate_backoff(
                                retry_idx,
                                retry_after,
                                &policy,
                            );
                            if backoff == Duration::MAX {
                                warn!(
                                    provider_id = candidate.provider_id.as_str(),
                                    model_id = candidate.model_id.as_str(),
                                    "Upstream Retry-After exceeds max allowable wait bound for stream; abandoning candidate"
                                );
                                break;
                            }

                            let now_post = Instant::now();
                            if now_post + backoff >= deadline {
                                return Err(AppError::GatewayTimeout(
                                    "Overall request processing deadline exceeded during stream retry backoff"
                                        .to_string(),
                                ));
                            }
                            tokio::time::sleep(backoff).await;
                            continue;
                        }

                        break;
                    }
                }
            }

            failed_model_ids.insert(candidate.model_id.clone());
            candidate_attempt_count += 1;

            if req.routing_mode.eq_ignore_ascii_case("manual") {
                break;
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
