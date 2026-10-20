//! Domain models for provider quota and rate-limit tracking.
//!
//! Enforces normalized representations, scope isolation, and explicit unknown semantics.

use serde::{Deserialize, Serialize};

/// Type of rate limit / quota constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLimitType {
    Requests,
    Tokens,
}

impl QuotaLimitType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::Tokens => "tokens",
        }
    }
}

/// Time unit or window for quota enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLimitUnit {
    PerMinute,
    PerDay,
    Unknown,
}

impl QuotaLimitUnit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PerMinute => "rpm",
            Self::PerDay => "rpd",
            Self::Unknown => "unknown",
        }
    }
}

/// Source authority providing the quota observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    /// Authoritative HTTP response headers from provider.
    ProviderHeader,
    /// Configured locally in application configuration.
    LocalConfig,
    /// Unknown or unverified source.
    Unknown,
}

/// Normalized snapshot of an observed provider quota or rate limit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderQuota {
    pub provider_id: String,
    pub model_id: Option<String>,
    pub limit_type: QuotaLimitType,
    pub limit_unit: QuotaLimitUnit,
    pub limit_value: Option<u64>,
    pub remaining_amount: Option<u64>,
    pub reset_time_secs: Option<u64>,
    pub retry_after_secs: Option<u64>,
    pub source: QuotaSource,
    pub observed_at: u64,
    pub is_stale: bool,
}

/// Raw parsed observation extracted from provider HTTP response headers.
#[derive(Debug, Clone, Default)]
pub struct QuotaObservation {
    pub provider_id: String,
    pub model_id: Option<String>,
    pub limit_requests: Option<u64>,
    pub remaining_requests: Option<u64>,
    pub limit_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub retry_after_secs: Option<u64>,
}
