//! Concrete OpenAI-compatible HTTP Provider Adapter.
//!
//! Provides production-ready integration with OpenAI-compatible providers
//! such as Groq Cloud and OpenRouter, implementing buffered completion,
//! Server-Sent Events (SSE) streaming, credential isolation, and normalized error mapping.
//!
//! Conforms strictly to PROVIDER_INTEGRATION.md §2, ARCHITECTURE.md §2, and SECURITY.md §1 & §4.

use crate::models::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelMetadata, ProviderMetadata,
    StreamChunk, UsageMetadata,
};
use crate::providers::{
    BoxedCompletionFuture, BoxedHealthFuture, BoxedModelDiscoveryFuture, BoxedStreamFuture,
    ProviderAdapter, ProviderError, ProviderStream,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
}

/// Internal request payload matching standard OpenAI chat completion API.
#[derive(Debug, Serialize)]
struct OpenAiChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAiStreamOptions>,
}

/// Internal response payload from OpenAI chat completion API.
#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    #[serde(default)]
    id: String,
    #[allow(dead_code)]
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiMessage>,
    delta: Option<OpenAiDelta>,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    #[allow(dead_code)]
    #[serde(default = "default_role")]
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    #[allow(dead_code)]
    reasoning: Option<String>,
}

fn default_role() -> String {
    "assistant".to_string()
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
    completion_tokens: usize,
    #[serde(default)]
    total_tokens: usize,
}

/// Production HTTP provider adapter communicating with OpenAI-compatible endpoints.
pub struct OpenAiCompatibleAdapter {
    provider_id: String,
    display_name: String,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
    rate_limit_tier: String,
    static_models: Vec<ModelMetadata>,
    model_wire_map: HashMap<String, String>,
    quota_service: Option<std::sync::Arc<crate::quota::QuotaService>>,
}

impl OpenAiCompatibleAdapter {
    /// Create a new OpenAI-compatible adapter.
    pub fn new(
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        Self {
            provider_id: provider_id.into(),
            display_name: display_name.into(),
            base_url: base_url.into(),
            api_key,
            client,
            rate_limit_tier: "standard-tier".to_string(),
            static_models: Vec::new(),
            model_wire_map: HashMap::new(),
            quota_service: None,
        }
    }

    /// Builder method to provide a custom HTTP client (useful for mock server testing).
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    /// Builder method to attach statically known model metadata.
    pub fn with_models(mut self, models: Vec<ModelMetadata>) -> Self {
        self.static_models = models;
        self
    }

    /// Builder method to configure wire-name mappings between catalog IDs and upstream model IDs.
    pub fn with_model_wire_map(mut self, map: HashMap<String, String>) -> Self {
        self.model_wire_map = map;
        self
    }

    /// Builder method to set rate limit tier.
    pub fn with_rate_limit_tier(mut self, tier: impl Into<String>) -> Self {
        self.rate_limit_tier = tier.into();
        self
    }

    /// Builder method to attach a QuotaService for automatic rate-limit telemetry extraction.
    pub fn with_quota_service(
        mut self,
        quota_service: std::sync::Arc<crate::quota::QuotaService>,
    ) -> Self {
        self.quota_service = Some(quota_service);
        self
    }

    /// Resolve internal model ID to upstream provider model identifier.
    fn resolve_upstream_model<'a>(&'a self, model_id: &'a str) -> &'a str {
        if let Some(mapped) = self.model_wire_map.get(model_id) {
            mapped.as_str()
        } else {
            model_id
        }
    }

    /// Extract safe retry_after seconds from HTTP response headers.
    fn extract_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
        headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
    }
}

impl ProviderAdapter for OpenAiCompatibleAdapter {
    fn provider_info(&self) -> ProviderMetadata {
        let is_operational =
            self.api_key.is_some() && !self.api_key.as_ref().unwrap().trim().is_empty();

        ProviderMetadata {
            provider_id: self.provider_id.clone(),
            display_name: self.display_name.clone(),
            base_url: self.base_url.clone(),
            status: if is_operational {
                "operational".to_string()
            } else {
                "offline".to_string()
            },
            enabled: is_operational,
            models_count: self.static_models.len(),
            rate_limit_tier: self.rate_limit_tier.clone(),
        }
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "simple_chat".to_string(),
            "fast_reasoning".to_string(),
            "coding".to_string(),
            "complex_analysis".to_string(),
            "deep_reasoning".to_string(),
            "streaming".to_string(),
        ]
    }

    fn discover_models(&self) -> BoxedModelDiscoveryFuture<'_> {
        let models = self.static_models.clone();
        Box::pin(async move { Ok(models) })
    }

    fn complete<'a>(&'a self, req: &'a CompletionRequest) -> BoxedCompletionFuture<'a> {
        Box::pin(async move {
            let api_key = self.api_key.as_deref().ok_or_else(|| {
                ProviderError::AuthenticationFailed(format!(
                    "Provider '{}' API key is not configured",
                    self.provider_id
                ))
            })?;

            let upstream_model = self.resolve_upstream_model(&req.model_id);
            let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

            let body = OpenAiChatRequest {
                model: upstream_model,
                messages: &req.messages,
                temperature: req.temperature,
                max_tokens: req.max_tokens,
                stream: false,
                stream_options: None,
            };

            let mut request_builder = self
                .client
                .post(&url)
                .bearer_auth(api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(req.timeout)
                .json(&body);

            // OpenRouter specific attribution headers
            if self.provider_id == "openrouter-api" {
                request_builder = request_builder
                    .header("HTTP-Referer", "https://github.com/rust-backend/ai-backend")
                    .header("X-Title", "Rust AI Backend");
            }

            let response = match request_builder.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    if err.is_timeout() {
                        return Err(ProviderError::Timeout(req.timeout));
                    }
                    if err.is_connect() {
                        return Err(ProviderError::NetworkFailure(
                            "Failed to connect to upstream provider endpoint".to_string(),
                        ));
                    }
                    return Err(ProviderError::NetworkFailure(
                        "Network transport failure communicating with provider".to_string(),
                    ));
                }
            };

            let status = response.status();
            if !status.is_success() {
                let status_code = status.as_u16();
                if status_code == 401 || status_code == 403 {
                    return Err(ProviderError::AuthenticationFailed(
                        "Upstream provider authentication failed".to_string(),
                    ));
                }
                if status_code == 429 {
                    let retry_after = Self::extract_retry_after(response.headers());
                    if let Some(qs) = &self.quota_service {
                        qs.record_rate_limited(&self.provider_id, Some(&req.model_id), retry_after);
                    }
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: retry_after,
                    });
                }
                if status_code == 404 {
                    return Err(ProviderError::ModelNotFound(req.model_id.clone()));
                }
                if status_code == 400 || status_code == 422 {
                    return Err(ProviderError::InvalidRequest(
                        "Upstream provider rejected request payload as invalid".to_string(),
                    ));
                }
                return Err(ProviderError::UpstreamError {
                    status_code,
                    message: "Upstream provider returned an error".to_string(),
                });
            }

            if let Some(qs) = &self.quota_service {
                let obs = crate::quota::QuotaService::extract_headers(
                    &self.provider_id,
                    Some(&req.model_id),
                    response.headers(),
                );
                qs.record_observation(obs);
            }

            let parsed: OpenAiChatResponse =
                response
                    .json()
                    .await
                    .map_err(|_| ProviderError::UpstreamError {
                        status_code: 502,
                        message: "Failed to deserialize upstream completion JSON".to_string(),
                    })?;

            let choice = parsed.choices.into_iter().next();
            let content = choice
                .and_then(|c| c.message)
                .and_then(|m| m.content)
                .unwrap_or_default();

            let usage = parsed.usage.map(|u| UsageMetadata {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                is_estimated: false,
            });

            Ok(CompletionResponse {
                id: if parsed.id.is_empty() {
                    format!("chatcmpl_{}", self.provider_id)
                } else {
                    parsed.id
                },
                model_used: req.model_id.clone(),
                provider_used: self.provider_id.clone(),
                is_fallback: false,
                routing_reason: None,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content,
                },
                usage,
            })
        })
    }

    fn stream<'a>(&'a self, req: &'a CompletionRequest) -> BoxedStreamFuture<'a> {
        Box::pin(async move {
            let api_key = self.api_key.as_deref().ok_or_else(|| {
                ProviderError::AuthenticationFailed(format!(
                    "Provider '{}' API key is not configured",
                    self.provider_id
                ))
            })?;

            let upstream_model = self.resolve_upstream_model(&req.model_id);
            let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

            let body = OpenAiChatRequest {
                model: upstream_model,
                messages: &req.messages,
                temperature: req.temperature,
                max_tokens: req.max_tokens,
                stream: true,
                stream_options: Some(OpenAiStreamOptions {
                    include_usage: true,
                }),
            };

            let mut request_builder = self
                .client
                .post(&url)
                .bearer_auth(api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .timeout(req.timeout)
                .json(&body);

            if self.provider_id == "openrouter-api" {
                request_builder = request_builder
                    .header("HTTP-Referer", "https://github.com/rust-backend/ai-backend")
                    .header("X-Title", "Rust AI Backend");
            }

            let response = match request_builder.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    if err.is_timeout() {
                        return Err(ProviderError::Timeout(req.timeout));
                    }
                    if err.is_connect() {
                        return Err(ProviderError::NetworkFailure(
                            "Failed to connect to upstream provider endpoint".to_string(),
                        ));
                    }
                    return Err(ProviderError::NetworkFailure(
                        "Network transport failure initiating stream".to_string(),
                    ));
                }
            };

            let status = response.status();
            if !status.is_success() {
                let status_code = status.as_u16();
                if status_code == 401 || status_code == 403 {
                    return Err(ProviderError::AuthenticationFailed(
                        "Upstream provider authentication failed".to_string(),
                    ));
                }
                if status_code == 429 {
                    let retry_after = Self::extract_retry_after(response.headers());
                    if let Some(qs) = &self.quota_service {
                        qs.record_rate_limited(&self.provider_id, Some(&req.model_id), retry_after);
                    }
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: retry_after,
                    });
                }
                if status_code == 404 {
                    return Err(ProviderError::ModelNotFound(req.model_id.clone()));
                }
                if status_code == 400 || status_code == 422 {
                    return Err(ProviderError::InvalidRequest(
                        "Upstream provider rejected request payload as invalid".to_string(),
                    ));
                }
                return Err(ProviderError::UpstreamError {
                    status_code,
                    message: "Upstream provider streaming error".to_string(),
                });
            }

            if let Some(qs) = &self.quota_service {
                let obs = crate::quota::QuotaService::extract_headers(
                    &self.provider_id,
                    Some(&req.model_id),
                    response.headers(),
                );
                qs.record_observation(obs);
            }

            // Stream byte chunks and parse SSE framing line-by-line via unfold
            let byte_stream = response.bytes_stream();
            let parsed_stream = parse_sse_stream(byte_stream);
            Ok(parsed_stream)
        })
    }

    fn check_health(&self) -> BoxedHealthFuture<'_> {
        Box::pin(async move {
            let api_key = match self.api_key.as_deref() {
                Some(k) if !k.trim().is_empty() => k,
                _ => return Ok(false),
            };

            let url = format!("{}/models", self.base_url.trim_end_matches('/'));
            let resp = self
                .client
                .get(&url)
                .bearer_auth(api_key)
                .timeout(Duration::from_secs(4))
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let code = r.status().as_u16();
                    // 200 OK or 400 (model listing blocked but server reachable)
                    Ok(code == 200 || code == 400)
                }
                Err(_) => Ok(false),
            }
        })
    }
}

/// Helper function to parse raw byte stream into SSE StreamChunks using `futures_util::stream::unfold`.
fn parse_sse_stream<S>(byte_stream: S) -> ProviderStream
where
    S: Stream<Item = Result<axum::body::Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let stream = futures_util::stream::unfold(
        (byte_stream, String::new(), false),
        |(mut bytes_stream, mut buffer, mut terminated)| async move {
            if terminated {
                return None;
            }

            loop {
                // Check if buffer contains a complete newline
                if let Some(pos) = buffer.find('\n') {
                    let line: String = buffer.drain(..=pos).collect();
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with(':') {
                        continue;
                    }

                    if let Some(data_str) = trimmed.strip_prefix("data:") {
                        let payload = data_str.trim();
                        if payload == "[DONE]" {
                            terminated = true;
                            return Some((
                                Ok(StreamChunk {
                                    delta: String::new(),
                                    is_done: true,
                                    usage: None,
                                }),
                                (bytes_stream, buffer, terminated),
                            ));
                        }

                        if let Ok(chunk_resp) = serde_json::from_str::<OpenAiChatResponse>(payload)
                        {
                            let delta_content = chunk_resp
                                .choices
                                .first()
                                .and_then(|c| c.delta.as_ref())
                                .and_then(|d| d.content.as_deref())
                                .unwrap_or("");

                            let usage = chunk_resp.usage.map(|u| UsageMetadata {
                                prompt_tokens: u.prompt_tokens,
                                completion_tokens: u.completion_tokens,
                                total_tokens: u.total_tokens,
                                is_estimated: false,
                            });

                            if delta_content.is_empty() && usage.is_none() {
                                continue;
                            }

                            return Some((
                                Ok(StreamChunk {
                                    delta: delta_content.to_string(),
                                    is_done: false,
                                    usage,
                                }),
                                (bytes_stream, buffer, terminated),
                            ));
                        }
                    }
                    continue;
                }

                // Buffer does not have a newline; read more bytes from upstream
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            buffer.push_str(text);
                        }
                    }
                    Some(Err(_)) => {
                        terminated = true;
                        return Some((
                            Err(ProviderError::UpstreamError {
                                status_code: 502,
                                message: "Upstream SSE stream connection interrupted".to_string(),
                            }),
                            (bytes_stream, buffer, terminated),
                        ));
                    }
                    None => {
                        // End of stream from upstream
                        terminated = true;
                        return Some((
                            Ok(StreamChunk {
                                delta: String::new(),
                                is_done: true,
                                usage: None,
                            }),
                            (bytes_stream, buffer, terminated),
                        ));
                    }
                }
            }
        },
    );

    Box::pin(stream)
}
