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
    pub chunk_delay: Option<Duration>,
    pub stream_chunks: Vec<String>,
    pub is_healthy: bool,
    pub custom_usage: Option<Option<UsageMetadata>>,
    pub custom_stream_usage: Option<Option<UsageMetadata>>,
    pub call_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub failures_remaining: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub transient_mode: bool,
    pub stream_error: Option<ProviderError>,
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
            chunk_delay: None,
            stream_chunks: vec![
                "Mock ".to_string(),
                "streaming ".to_string(),
                "response".to_string(),
            ],
            is_healthy: true,
            custom_usage: None,
            custom_stream_usage: None,
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            failures_remaining: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            transient_mode: false,
            stream_error: None,
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

    /// Configure simulated delay between streaming chunks.
    pub fn with_chunk_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = Some(delay);
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

    /// Configure custom usage returned by complete.
    pub fn with_usage(mut self, usage: Option<UsageMetadata>) -> Self {
        self.custom_usage = Some(usage);
        self
    }

    /// Configure custom usage returned in terminal stream chunk.
    pub fn with_stream_usage(mut self, usage: Option<UsageMetadata>) -> Self {
        self.custom_stream_usage = Some(usage);
        self
    }

    /// Configure transient failure count before succeeding.
    pub fn with_transient_failures(mut self, count: usize, err: ProviderError) -> Self {
        self.failures_remaining
            .store(count, std::sync::atomic::Ordering::SeqCst);
        self.should_fail_with = Some(err);
        self.transient_mode = true;
        self
    }

    /// Configure error yielded during streaming generation.
    pub fn with_stream_error(mut self, err: ProviderError) -> Self {
        self.stream_error = Some(err);
        self
    }

    /// Return total invocation attempts observed by this mock.
    pub fn call_count(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::SeqCst)
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
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            // 1. Check timeout / delay simulation
            if let Some(delay) = self.simulated_delay {
                if delay > req.timeout {
                    tokio::time::sleep(req.timeout).await;
                    return Err(ProviderError::Timeout(req.timeout));
                }
                tokio::time::sleep(delay).await;
            }

            // 2. Simulated failure check
            let should_fail = if self.transient_mode {
                let remaining = self
                    .failures_remaining
                    .load(std::sync::atomic::Ordering::SeqCst);
                if remaining > 0 {
                    self.failures_remaining
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    self.should_fail_with.clone()
                } else {
                    None
                }
            } else {
                self.should_fail_with.clone()
            };

            if let Some(ref err) = should_fail {
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
                usage: self.custom_usage.clone().unwrap_or(Some(UsageMetadata {
                    prompt_tokens: 25,
                    completion_tokens: 35,
                    total_tokens: 60,
                    is_estimated: false,
                })),
            })
        })
    }

    fn stream<'a>(&'a self, req: &'a CompletionRequest) -> BoxedStreamFuture<'a> {
        Box::pin(async move {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            // 1. Check timeout / delay simulation
            if let Some(delay) = self.simulated_delay {
                if delay > req.timeout {
                    tokio::time::sleep(req.timeout).await;
                    return Err(ProviderError::Timeout(req.timeout));
                }
                tokio::time::sleep(delay).await;
            }

            // 2. Simulated failure check
            let should_fail = if self.transient_mode {
                let remaining = self
                    .failures_remaining
                    .load(std::sync::atomic::Ordering::SeqCst);
                if remaining > 0 {
                    self.failures_remaining
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    self.should_fail_with.clone()
                } else {
                    None
                }
            } else {
                self.should_fail_with.clone()
            };

            if let Some(ref err) = should_fail {
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

            let stream_usage = self
                .custom_stream_usage
                .clone()
                .unwrap_or(Some(UsageMetadata {
                    prompt_tokens: 25,
                    completion_tokens: 35,
                    total_tokens: 60,
                    is_estimated: false,
                }));

            let chunk_delay = self.chunk_delay;
            let stream_error = self.stream_error.clone();
            let stream: ProviderStream = if let Some(delay) = chunk_delay {
                let stream_chunks = self.stream_chunks.clone();
                let unfold_stream = futures_util::stream::unfold(
                    (0usize, stream_chunks, stream_usage, stream_error),
                    move |(idx, chunks, usage, err_opt)| async move {
                        tokio::time::sleep(delay).await;
                        if idx < chunks.len() {
                            let delta = chunks[idx].clone();
                            Some((
                                Ok(StreamChunk {
                                    delta,
                                    is_done: false,
                                    usage: None,
                                }),
                                (idx + 1, chunks, usage, err_opt),
                            ))
                        } else if idx == chunks.len() {
                            if let Some(err) = err_opt {
                                Some((Err(err), (idx + 1, chunks, None, None)))
                            } else {
                                Some((
                                    Ok(StreamChunk {
                                        delta: String::new(),
                                        is_done: true,
                                        usage,
                                    }),
                                    (idx + 1, chunks, None, None),
                                ))
                            }
                        } else {
                            None
                        }
                    },
                );
                Box::pin(unfold_stream)
            } else {
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

                if let Some(err) = stream_error {
                    chunks.push(Err(err));
                } else {
                    chunks.push(Ok(StreamChunk {
                        delta: String::new(),
                        is_done: true,
                        usage: stream_usage,
                    }));
                }

                Box::pin(futures_util::stream::iter(chunks))
            };

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
