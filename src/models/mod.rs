//! Canonical model, provider, and completion domain types.
//!
//! Conforms to PROVIDER_INTEGRATION.md §2 and API_SPEC.md §3.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Normalized metadata describing an integrated AI provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub provider_id: String,
    pub display_name: String,
    #[serde(skip_serializing)]
    pub base_url: String,
    pub status: String, // "operational", "degraded", "offline"
    pub enabled: bool,
    pub models_count: usize,
    pub rate_limit_tier: String,
}

/// Normalized model capabilities and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelMetadata {
    pub model_id: String,
    pub display_name: String,
    pub provider_id: String,
    pub supported_modalities: Vec<String>, // "text", "code", "image", "document"
    pub context_window: usize,
    pub max_output_tokens: usize,
    pub streaming_supported: bool,
    pub tool_calling_supported: bool,
    pub speed_tier: String, // "fast", "balanced", "advanced"
    pub cost_tier: String,  // "free-tier", "low-cost", "premium"
    pub available: bool,
    pub capabilities: Vec<String>,
    pub last_health_check: String,
}

impl ModelMetadata {
    /// Return the upstream gateway handling this model, if applicable.
    pub fn gateway(&self) -> Option<&'static str> {
        if self.provider_id.starts_with("freellmapi") {
            Some("freellmapi")
        } else if self.provider_id.starts_with("omniroute") {
            Some("omniroute")
        } else if self.provider_id.starts_with("groq") {
            Some("groq")
        } else if self.provider_id.starts_with("openrouter") {
            Some("openrouter")
        } else {
            None
        }
    }
}

/// Individual chat message with role and content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String, // "system", "user", "assistant"
    pub content: String,
}

/// Default timeout for completion requests (10 seconds per SECURITY.md §3.2).
pub fn default_timeout() -> Duration {
    Duration::from_secs(10)
}

/// Normalized completion request passed to providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model_id: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    #[serde(default = "default_timeout")]
    pub timeout: Duration,
    #[serde(default)]
    pub stream: bool,
}

impl CompletionRequest {
    pub fn new(model_id: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model_id: model_id.into(),
            messages,
            temperature: None,
            max_tokens: None,
            timeout: default_timeout(),
            stream: false,
        }
    }
}

/// Normalized completion response returned from providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionResponse {
    pub id: String,
    pub model_used: String,
    pub provider_used: String,
    pub is_fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_reason: Option<String>,
    pub message: ChatMessage,
    pub usage: Option<UsageMetadata>,
}

/// Incremental streaming chunk yielded during streaming completion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamChunk {
    pub delta: String,
    pub is_done: bool,
    pub usage: Option<UsageMetadata>,
}

/// Normalized token usage accounting metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageMetadata {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub is_estimated: bool,
}

/// Type alias for UsageMetadata conforming to PROVIDER_INTEGRATION.md §2.
pub type NormalizedUsage = UsageMetadata;
