//! Quota and rate-limit tracking service.
//!
//! Maintains thread-safe, normalized quota telemetry extracted from provider HTTP response
//! headers with explicit staleness and scope isolation.

use crate::quota::model::{
    ProviderQuota, QuotaLimitType, QuotaLimitUnit, QuotaObservation, QuotaSource,
};
use reqwest::header::HeaderMap;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_TTL_SECS: u64 = 60;

/// Thread-safe service for monitoring provider rate limits and quotas.
#[derive(Clone, Default)]
pub struct QuotaService {
    quotas: Arc<RwLock<HashMap<String, ProviderQuota>>>,
}

impl QuotaService {
    pub fn new() -> Self {
        Self {
            quotas: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Construct unique storage key isolating provider and model scopes.
    fn make_key(provider_id: &str, model_id: Option<&str>, limit_type: QuotaLimitType) -> String {
        match model_id {
            Some(m) => format!("{provider_id}::{m}::{}", limit_type.as_str()),
            None => format!("{provider_id}::global::{}", limit_type.as_str()),
        }
    }

    /// Current Unix timestamp in seconds.
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Extract documented rate limit headers from HTTP response headers.
    pub fn extract_headers(
        provider_id: &str,
        model_id: Option<&str>,
        headers: &HeaderMap,
    ) -> QuotaObservation {
        let mut obs = QuotaObservation {
            provider_id: provider_id.to_string(),
            model_id: model_id.map(|s| s.to_string()),
            ..Default::default()
        };

        // Standard and Groq rate-limit headers
        if let Some(s) = headers
            .get("x-ratelimit-remaining-requests")
            .and_then(|v| v.to_str().ok())
        {
            obs.remaining_requests = s.trim().parse::<u64>().ok();
        }

        if let Some(s) = headers
            .get("x-ratelimit-limit-requests")
            .and_then(|v| v.to_str().ok())
        {
            obs.limit_requests = s.trim().parse::<u64>().ok();
        }

        if let Some(s) = headers
            .get("x-ratelimit-remaining-tokens")
            .and_then(|v| v.to_str().ok())
        {
            obs.remaining_tokens = s.trim().parse::<u64>().ok();
        }

        if let Some(s) = headers
            .get("x-ratelimit-limit-tokens")
            .and_then(|v| v.to_str().ok())
        {
            obs.limit_tokens = s.trim().parse::<u64>().ok();
        }

        if let Some(s) = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
        {
            obs.retry_after_secs = s.trim().parse::<u64>().ok();
        }

        obs
    }

    /// Record a parsed quota observation from HTTP response headers.
    pub fn record_observation(&self, obs: QuotaObservation) {
        let now = Self::now_secs();
        let mut store = self.quotas.write().unwrap();

        // 1. Requests quota
        if obs.remaining_requests.is_some() || obs.limit_requests.is_some() {
            let key = Self::make_key(
                &obs.provider_id,
                obs.model_id.as_deref(),
                QuotaLimitType::Requests,
            );
            let quota = ProviderQuota {
                provider_id: obs.provider_id.clone(),
                model_id: obs.model_id.clone(),
                limit_type: QuotaLimitType::Requests,
                limit_unit: QuotaLimitUnit::PerMinute,
                limit_value: obs.limit_requests,
                remaining_amount: obs.remaining_requests,
                reset_time_secs: None,
                retry_after_secs: obs.retry_after_secs,
                source: QuotaSource::ProviderHeader,
                observed_at: now,
                is_stale: false,
            };
            store.insert(key, quota);
        }

        // 2. Tokens quota
        if obs.remaining_tokens.is_some() || obs.limit_tokens.is_some() {
            let key = Self::make_key(
                &obs.provider_id,
                obs.model_id.as_deref(),
                QuotaLimitType::Tokens,
            );
            let quota = ProviderQuota {
                provider_id: obs.provider_id.clone(),
                model_id: obs.model_id.clone(),
                limit_type: QuotaLimitType::Tokens,
                limit_unit: QuotaLimitUnit::PerMinute,
                limit_value: obs.limit_tokens,
                remaining_amount: obs.remaining_tokens,
                reset_time_secs: None,
                retry_after_secs: obs.retry_after_secs,
                source: QuotaSource::ProviderHeader,
                observed_at: now,
                is_stale: false,
            };
            store.insert(key, quota);
        }
    }

    /// Record an explicit HTTP 429 rate limit event.
    pub fn record_rate_limited(
        &self,
        provider_id: &str,
        model_id: Option<&str>,
        retry_after_secs: Option<u64>,
    ) {
        let now = Self::now_secs();
        let mut store = self.quotas.write().unwrap();
        let key = Self::make_key(provider_id, model_id, QuotaLimitType::Requests);

        let quota = ProviderQuota {
            provider_id: provider_id.to_string(),
            model_id: model_id.map(|s| s.to_string()),
            limit_type: QuotaLimitType::Requests,
            limit_unit: QuotaLimitUnit::PerMinute,
            limit_value: None,
            remaining_amount: Some(0),
            reset_time_secs: retry_after_secs.map(|s| now + s),
            retry_after_secs,
            source: QuotaSource::ProviderHeader,
            observed_at: now,
            is_stale: false,
        };
        store.insert(key, quota);
    }

    /// Check if a provider or specific model is currently confirmed to be rate-limited.
    ///
    /// CONSERVATIVE ROUTING GUARANTEE:
    /// Returns true ONLY when:
    /// 1. There is an active retry_after duration that has not yet elapsed, OR
    /// 2. A fresh observation (< 60s old) explicitly reported remaining_amount == 0.
    ///
    /// Returns false if quota is unknown or observation has gone stale.
    pub fn is_rate_limited(&self, provider_id: &str, model_id: Option<&str>) -> bool {
        let now = Self::now_secs();
        let store = self.quotas.read().unwrap();

        // Check model-specific key first, then global provider key
        let keys = [
            Self::make_key(provider_id, model_id, QuotaLimitType::Requests),
            Self::make_key(provider_id, None, QuotaLimitType::Requests),
        ];

        for key in &keys {
            if let Some(quota) = store.get(key) {
                // If retry_after is active
                if quota
                    .retry_after_secs
                    .is_some_and(|retry_after| now < quota.observed_at + retry_after)
                {
                    return true;
                }
                // If remaining is explicitly zero and fresh
                if quota.remaining_amount == Some(0)
                    && now.saturating_sub(quota.observed_at) < DEFAULT_TTL_SECS
                {
                    return true;
                }
            }
        }

        false
    }

    /// Fetch all tracked quotas with updated staleness flags.
    pub fn get_quotas(&self) -> Vec<ProviderQuota> {
        let now = Self::now_secs();
        let store = self.quotas.read().unwrap();

        store
            .values()
            .map(|q| {
                let mut quota = q.clone();
                quota.is_stale = now.saturating_sub(quota.observed_at) >= DEFAULT_TTL_SECS;
                quota
            })
            .collect()
    }
}
