//! In-memory mock provider adapter for deterministic unit and integration testing.
//!
//! Conforms strictly to PROVIDER_INTEGRATION.md §2.

use crate::models::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelMetadata, ProviderMetadata,
    StreamChunk, UsageMetadata,
};
use crate::providers::{
    BoxedCompletionFuture, BoxedHealthFuture, BoxedModelDiscoveryFuture, BoxedStreamFuture,
    ProviderAdapter, ProviderError, ProviderStream,
};
use std::time::Duration;

/// In-memory mock provider adapter for deterministic unit and integration testing.
pub struct MockProviderAdapter {
    pub metadata: ProviderMetadata,
    pub models: Vec<ModelMetadata>,
    pub capabilities: Vec<String>,
    pub should_fail_with: Option<ProviderError>,
    pub simulated_delay: Option<Duration>,
    pub stream_chunks: Vec<String>,
    pub is_healthy: bool,
}

impl MockProviderAdapter {
    /// Create a new MockProviderAdapter with default single model and standard capabilities.
    pub fn new(provider_id: &str, display_name: &str) -> Self {
        let meta = ProviderMetadata {
            provider_id: provider_id.to_string(),
            display_name: display_name.to_string(),
            base_url: "https://mock.ai.internal/v1".to_string(),
            status: "operational".to_string(),
            enabled: true,
            models_count: 1,
            rate_limit_tier: "standard-tier".to_string(),
        };

        let default_model = ModelMetadata {
            model_id: format!("{provider_id}-model-1"),
            display_name: format!("{display_name} Base Model"),
            provider_id: provider_id.to_string(),
            supported_modalities: vec!["text".to_string(), "code".to_string()],
            context_window: 32768,
            max_output_tokens: 4096,
            streaming_supported: true,
            tool_calling_supported: false,
            speed_tier: "fast".to_string(),
            cost_tier: "free-tier".to_string(),
            available: true,
            capabilities: vec!["simple_chat".to_string(), "coding".to_string()],
            last_health_check: "2026-10-19T18:00:00Z".to_string(),
        };

        Self {
            metadata: meta,
            models: vec![default_model],
            capabilities: vec![
                "simple_chat".to_string(),
                "streaming".to_string(),
                "coding".to_string(),
            ],
            should_fail_with: None,
            simulated_delay: None,
            stream_chunks: vec![
                "Mock ".to_string(),
                "streaming ".to_string(),
                "response".to_string(),
            ],
            is_healthy: true,
        }
    }

    /// Replace the models list returned by discover_models.
    pub fn with_models(mut self, models: Vec<ModelMetadata>) -> Self {
        self.metadata.models_count = models.len();
        self.models = models;
        self
    }

    /// Set explicit capabilities for this adapter.
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Configure simulated error on operations.
    pub fn with_simulated_error(mut self, err: ProviderError) -> Self {
        self.should_fail_with = Some(err);
        self
    }

    /// Configure simulated latency/delay for execution.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.simulated_delay = Some(delay);
        self
    }

    /// Configure chunks yielded during streaming completion.
    pub fn with_stream_chunks(mut self, chunks: Vec<String>) -> Self {
        self.stream_chunks = chunks;
        self
    }

    /// Configure health status returned by check_health.
    pub fn with_health(mut self, healthy: bool) -> Self {
        self.is_healthy = healthy;
        self
    }

    /// Configure enabled state on metadata.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.metadata.enabled = enabled;
        self
    }
}

impl ProviderAdapter for MockProviderAdapter {
    fn provider_info(&self) -> ProviderMetadata {
        self.metadata.clone()
    }

    fn capabilities(&self) -> Vec<String> {
        self.capabilities.clone()
    }

    fn discover_models(&self) -> BoxedModelDiscoveryFuture<'_> {
        let res = if let Some(ref err) = self.should_fail_with {
            Err(err.clone())
        } else {
            Ok(self.models.clone())
        };
        Box::pin(std::future::ready(res))
    }

    fn complete<'a>(&'a self, req: &'a CompletionRequest) -> BoxedCompletionFuture<'a> {
        Box::pin(async move {
            // 1. Check timeout / delay simulation
            if let Some(delay) = self.simulated_delay {
                if delay > req.timeout {
                    tokio::time::sleep(req.timeout).await;
                    return Err(ProviderError::Timeout(req.timeout));
                }
                tokio::time::sleep(delay).await;
            }

            // 2. Simulated failure check
            if let Some(ref err) = self.should_fail_with {
                return Err(err.clone());
            }

            // 3. Model existence check
            if !self.models.is_empty() && !self.models.iter().any(|m| m.model_id == req.model_id) {
                return Err(ProviderError::ModelNotFound(req.model_id.clone()));
            }

            Ok(CompletionResponse {
                id: "chatcmpl_mock_12345".to_string(),
                model_used: req.model_id.clone(),
                provider_used: self.metadata.provider_id.clone(),
                is_fallback: false,
                routing_reason: None,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: format!("Mock completion response for model {}", req.model_id),
                },
                usage: Some(UsageMetadata {
                    prompt_tokens: 25,
                    completion_tokens: 35,
                    total_tokens: 60,
                    is_estimated: false,
                }),
            })
        })
    }

    fn stream<'a>(&'a self, req: &'a CompletionRequest) -> BoxedStreamFuture<'a> {
        Box::pin(async move {
            // 1. Check timeout / delay simulation
            if let Some(delay) = self.simulated_delay {
                if delay > req.timeout {
                    tokio::time::sleep(req.timeout).await;
                    return Err(ProviderError::Timeout(req.timeout));
                }
                tokio::time::sleep(delay).await;
            }

            // 2. Simulated failure check
            if let Some(ref err) = self.should_fail_with {
                return Err(err.clone());
            }

            // 3. Check streaming capability
            if !self.supports_capability("streaming") {
                return Err(ProviderError::CapabilityMismatch("streaming".to_string()));
            }

            // 4. Model existence check
            if !self.models.is_empty() && !self.models.iter().any(|m| m.model_id == req.model_id) {
                return Err(ProviderError::ModelNotFound(req.model_id.clone()));
            }

            // 5. Generate stream chunks
            let mut chunks: Vec<Result<StreamChunk, ProviderError>> = self
                .stream_chunks
                .iter()
                .map(|text| {
                    Ok(StreamChunk {
                        delta: text.clone(),
                        is_done: false,
                        usage: None,
                    })
                })
                .collect();

            // Final done chunk with usage
            chunks.push(Ok(StreamChunk {
                delta: String::new(),
                is_done: true,
                usage: Some(UsageMetadata {
                    prompt_tokens: 25,
                    completion_tokens: 35,
                    total_tokens: 60,
                    is_estimated: false,
                }),
            }));

            let stream: ProviderStream = Box::pin(futures_util::stream::iter(chunks));
            Ok(stream)
        })
    }

    fn check_health(&self) -> BoxedHealthFuture<'_> {
        let res = if let Some(ref err) = self.should_fail_with {
            Err(err.clone())
        } else {
            Ok(self.is_healthy)
        };
        Box::pin(std::future::ready(res))
    }
}
