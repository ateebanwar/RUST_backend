//! Retry and failover policy definitions conforming to Phase 9 specification.
//!
//! Provides bounded exponential backoff with jitter, transient vs non-retryable error
//! classification, and deadline-aware delay calculation.

use crate::providers::ProviderError;
use std::time::Duration;

/// Configuration parameters governing retry and failover execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts per individual candidate target for transient errors.
    pub max_retries_per_candidate: u32,
    /// Maximum number of distinct alternative candidates to attempt in automatic routing mode.
    pub max_failover_candidates: usize,
    /// Initial exponential backoff delay in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum backoff ceiling in milliseconds.
    pub max_backoff_ms: u64,
    /// Maximum allowable upstream `Retry-After` duration in seconds before refusing to wait.
    pub max_retry_after_secs: u64,
    /// Enforced overall request deadline in seconds across all attempts and sleep delays.
    pub overall_request_timeout_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries_per_candidate: 1,
            max_failover_candidates: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 1000,
            max_retry_after_secs: 5,
            overall_request_timeout_secs: 30,
        }
    }
}

/// Classify whether a provider error represents an eligible transient failure.
///
/// Transient failures qualify for bounded retry on the same candidate and/or
/// triggering failover to a compatible alternative model in automatic routing mode:
/// - Connection and network failures
/// - Upstream transport and gateway timeouts
/// - HTTP 429 (Rate Limited)
/// - Selected HTTP 5xx errors (500, 502, 503, 504) and HTTP 408 (Request Timeout)
pub fn is_transient_error(err: &ProviderError) -> bool {
    match err {
        ProviderError::Timeout(_) => true,
        ProviderError::NetworkFailure(_) => true,
        ProviderError::RateLimited { .. } => true,
        ProviderError::UpstreamError { status_code, .. } => {
            matches!(status_code, 408 | 429 | 500 | 502 | 503 | 504)
        }
        _ => false,
    }
}

/// Classify whether a provider error is strictly non-retryable.
///
/// Non-retryable errors must short-circuit immediately without retrying or failing over:
/// - Invalid payload / bad parameters (HTTP 400, 422)
/// - Authentication failures (HTTP 401, 403)
/// - Model not found (HTTP 404)
/// - Capability/modality mismatches
/// - Disabled or unconfigured providers
pub fn is_non_retryable_error(err: &ProviderError) -> bool {
    match err {
        ProviderError::InvalidRequest(_) => true,
        ProviderError::AuthenticationFailed(_) => true,
        ProviderError::ModelNotFound(_) => true,
        ProviderError::CapabilityMismatch(_) => true,
        ProviderError::ProviderDisabled(_) => true,
        ProviderError::ProviderNotFound(_) => true,
        ProviderError::QuotaExhausted(_) => true,
        ProviderError::UpstreamError { status_code, .. } => {
            // Client errors other than 408 / 429 are strictly non-retryable
            (400..=499).contains(status_code) && *status_code != 408 && *status_code != 429
        }
        _ => false,
    }
}

/// Calculate the delay duration before the next retry attempt.
///
/// Respects provider `retry_after_secs` up to `policy.max_retry_after_secs`.
/// If provider requests longer than `max_retry_after_secs`, returns `Duration::MAX`
/// signaling that waiting would violate bounded latency requirements.
/// Otherwise, applies exponential backoff with pseudo-random jitter.
pub fn calculate_backoff(attempt: u32, retry_after: Option<u64>, policy: &RetryPolicy) -> Duration {
    if let Some(secs) = retry_after {
        if secs > policy.max_retry_after_secs {
            return Duration::MAX;
        }
        return Duration::from_secs(secs);
    }

    // Exponential backoff: initial * 2^attempt clamped to max_backoff
    let exp_factor = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    let base_ms = policy
        .initial_backoff_ms
        .saturating_mul(exp_factor)
        .min(policy.max_backoff_ms);

    // Apply bounded jitter: between 75% and 125% of base_ms
    // Calculated deterministically using timestamp nanos to avoid external dependencies
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let jitter_pct = 75 + (nanos % 51); // 75..=125
    let jittered_ms = (base_ms.saturating_mul(jitter_pct as u64)) / 100;
    let final_ms = jittered_ms.max(1).min(policy.max_backoff_ms);

    Duration::from_millis(final_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_classification() {
        assert!(is_transient_error(&ProviderError::Timeout(
            Duration::from_secs(1)
        )));
        assert!(is_transient_error(&ProviderError::NetworkFailure(
            "connect failed".into()
        )));
        assert!(is_transient_error(&ProviderError::RateLimited {
            retry_after_secs: Some(2)
        }));
        assert!(is_transient_error(&ProviderError::UpstreamError {
            status_code: 502,
            message: "".into()
        }));
        assert!(is_transient_error(&ProviderError::UpstreamError {
            status_code: 503,
            message: "".into()
        }));
        assert!(is_transient_error(&ProviderError::UpstreamError {
            status_code: 408,
            message: "".into()
        }));

        assert!(!is_transient_error(&ProviderError::InvalidRequest(
            "bad params".into()
        )));
        assert!(!is_transient_error(&ProviderError::AuthenticationFailed(
            "bad key".into()
        )));
        assert!(!is_transient_error(&ProviderError::ModelNotFound(
            "m1".into()
        )));

        assert!(is_non_retryable_error(&ProviderError::InvalidRequest(
            "bad params".into()
        )));
        assert!(is_non_retryable_error(
            &ProviderError::AuthenticationFailed("bad key".into())
        ));
        assert!(is_non_retryable_error(&ProviderError::ModelNotFound(
            "m1".into()
        )));
        assert!(is_non_retryable_error(&ProviderError::CapabilityMismatch(
            "cap".into()
        )));
        assert!(is_non_retryable_error(&ProviderError::UpstreamError {
            status_code: 400,
            message: "".into()
        }));
        assert!(is_non_retryable_error(&ProviderError::UpstreamError {
            status_code: 422,
            message: "".into()
        }));
    }

    #[test]
    fn test_backoff_calculation_with_retry_after() {
        let policy = RetryPolicy {
            max_retry_after_secs: 5,
            ..Default::default()
        };

        // Within bound: respects retry_after
        let delay = calculate_backoff(0, Some(3), &policy);
        assert_eq!(delay, Duration::from_secs(3));

        // Exceeds bound: returns MAX
        let delay_exceeded = calculate_backoff(0, Some(10), &policy);
        assert_eq!(delay_exceeded, Duration::MAX);
    }

    #[test]
    fn test_exponential_backoff_bounded() {
        let policy = RetryPolicy {
            initial_backoff_ms: 100,
            max_backoff_ms: 500,
            ..Default::default()
        };

        let b0 = calculate_backoff(0, None, &policy);
        assert!(b0.as_millis() <= 500);

        let b5 = calculate_backoff(5, None, &policy);
        assert!(b5.as_millis() <= 500);
    }
}
