//! Domain models and types for Phase 8 Quota and Usage Tracking.
//!
//! Enforces exact integer token representations, explicit unknown/missing semantics,
//! and structured aggregation schemas conforming strictly to Phase 8 specifications.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// High-level outcome of an AI request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    /// Request succeeded and returned a full completion or completed stream.
    Success,
    /// Upstream provider rejected the request or returned an HTTP error (4xx/5xx).
    ProviderError,
    /// Upstream or network request timed out.
    Timeout,
    /// Client disconnected or aborted prior to normal completion.
    ClientCancelled,
    /// Request failed validation, routing, or provider resolution prior to dispatch.
    PreDispatchFailure,
    /// Network stream interrupted mid-stream.
    StreamInterrupted,
}

impl RequestOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::ProviderError => "provider_error",
            Self::Timeout => "timeout",
            Self::ClientCancelled => "client_cancelled",
            Self::PreDispatchFailure => "pre_dispatch_failure",
            Self::StreamInterrupted => "stream_interrupted",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "provider_error" => Self::ProviderError,
            "timeout" => Self::Timeout,
            "client_cancelled" => Self::ClientCancelled,
            "pre_dispatch_failure" => Self::PreDispatchFailure,
            "stream_interrupted" => Self::StreamInterrupted,
            _ => Self::ProviderError,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Request invocation mode (buffered vs streaming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestMode {
    Buffered,
    Streaming,
}

impl RequestMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Buffered => "buffered",
            Self::Streaming => "streaming",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "streaming" => Self::Streaming,
            _ => Self::Buffered,
        }
    }
}

/// Request routing modality (automated classification vs explicit manual selection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Auto,
    Manual,
}

impl RoutingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "manual" => Self::Manual,
            _ => Self::Auto,
        }
    }
}

/// Source authority for token usage numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// Explicitly reported by the upstream provider.
    Reported,
    /// Usage metadata was not reported by the provider or unavailable due to cancellation.
    Unavailable,
}

impl UsageSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "reported" => Self::Reported,
            _ => Self::Unavailable,
        }
    }
}

/// Finalization status of a usage record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    /// Finalized terminal record.
    Final,
    /// In-flight provisional tracking record.
    Provisional,
}

impl RecordStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Final => "final",
            Self::Provisional => "provisional",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "provisional" => Self::Provisional,
            _ => Self::Final,
        }
    }
}

/// Canonical persistent usage record for an individual request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageRecord {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub request_mode: RequestMode,
    pub routing_mode: RoutingMode,
    pub start_time: u64,
    pub completion_time: u64,
    pub latency_ms: u64,
    pub outcome: RequestOutcome,
    pub failure_category: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub usage_source: UsageSource,
    pub record_status: RecordStatus,
}

impl UsageRecord {
    /// Deterministic policy for token fields:
    /// Preserves exact raw provider telemetry when provided.
    /// If total_tokens is missing but prompt and completion tokens exist, sums them.
    /// Never invents or fabricates estimates.
    pub fn normalize_tokens(
        prompt: Option<usize>,
        completion: Option<usize>,
        total: Option<usize>,
    ) -> (Option<u64>, Option<u64>, Option<u64>, UsageSource) {
        match (prompt, completion, total) {
            (None, None, None) => (None, None, None, UsageSource::Unavailable),
            (p, c, t) => {
                let p_u64 = p.map(|v| v as u64);
                let c_u64 = c.map(|v| v as u64);
                let t_u64 = t.map(|v| v as u64).or_else(|| match (p_u64, c_u64) {
                    (Some(p_val), Some(c_val)) => Some(p_val.saturating_add(c_val)),
                    _ => None,
                });
                (p_u64, c_u64, t_u64, UsageSource::Reported)
            }
        }
    }
}

/// Time-range and dimension filters for usage queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageQueryFilter {
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub outcome: Option<String>,
    pub limit: Option<usize>,
}

/// High-level aggregation summary across usage records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub known_usage_requests: u64,
    pub unknown_usage_requests: u64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub by_request_mode: HashMap<String, u64>,
    pub by_routing_mode: HashMap<String, u64>,
    pub by_outcome: HashMap<String, u64>,
}

/// Aggregated usage summary grouped by provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsageSummary {
    pub provider_id: String,
    pub requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub known_usage_requests: u64,
    pub total_tokens: u64,
    pub avg_latency_ms: f64,
}

/// Aggregated usage summary grouped by model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageSummary {
    pub model_id: String,
    pub provider_id: String,
    pub requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub known_usage_requests: u64,
    pub total_tokens: u64,
    pub avg_latency_ms: f64,
}

/// Operational metrics for the durable usage retry queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryQueueMetrics {
    pub pending_count: usize,
    pub dead_letter_count: usize,
}

/// Outcome of a retry queue drain execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryDrainResult {
    pub succeeded: usize,
    pub retried: usize,
    pub dead_lettered: usize,
}
