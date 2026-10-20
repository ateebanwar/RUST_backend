//! Retry and failover policy definitions conforming to Phase 9 specification.
//!
//! Provides bounded exponential backoff with pseudo-random jitter, RFC 9110 compliant
//! `Retry-After` header parsing (delay-seconds and HTTP-date), transient vs non-retryable
//! error classification, and deadline-aware delay calculation.

use crate::providers::ProviderError;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);
static JITTER_COUNTER: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);

/// Compute pseudo-random jitter percentage strictly in range [75, 125].
///
/// Combines an atomic sequence counter with monotonic clock nanoseconds through
/// SplitMix64 mixing to produce well-distributed, non-correlated values across
/// concurrent requests with zero lock contention or external dependencies.
pub fn compute_jitter_pct() -> u64 {
    let count = JITTER_COUNTER.fetch_add(0x9E3779B97F4A7C15, Ordering::Relaxed);
    let nanos = PROCESS_START.elapsed().as_nanos() as u64;
    let mut x = count.wrapping_add(nanos);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^= x >> 31;
    75 + (x % 51) // Strictly in 75..=125
}

/// Parse an HTTP `Retry-After` header value conforming to RFC 9110 §10.2.3.
///
/// Supports both:
/// 1. `delay-seconds` (e.g. "120" -> 120 seconds).
/// 2. `HTTP-date` (e.g. "Sun, 06 Nov 2026 08:49:37 GMT").
///
/// Guarantees:
/// - Past dates safely clamp to `Some(0)` without underflowing.
/// - Future dates return the remaining duration in seconds until the target time.
/// - Unparseable or malformed values return `None`.
pub fn parse_retry_after_header(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }

    // 1. Try parsing as delay-seconds
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(secs);
    }

    // 2. Try parsing as RFC 7231 / RFC 9110 IMF-fixdate: "Sun, 06 Nov 2026 08:49:37 GMT"
    parse_http_date(trimmed)
}

/// Helper: Parse standard RFC 7231 IMF-fixdate into seconds relative to current wall-clock.
fn parse_http_date(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    // IMF-fixdate format: [day_of_week, day, month, year, time, "GMT" | "UTC"]
    if parts.len() != 6 {
        return None;
    }

    // Verify day-of-week abbreviation
    let dow = parts[0].trim_end_matches(',').to_ascii_lowercase();
    if !matches!(
        dow.as_str(),
        "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun"
    ) {
        return None;
    }

    let day: u32 = parts[1].parse().ok()?;

    let month: u32 = match parts[2].to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };

    let year: i64 = parts[3].parse().ok()?;
    if !(1970..=2100).contains(&year) {
        return None;
    }

    // Validate day of month bounds (including leap years)
    let max_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            if is_leap { 29 } else { 28 }
        }
        _ => return None,
    };
    if day < 1 || day > max_days {
        return None;
    }

    let time_parts: Vec<&str> = parts[4].split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let min: u32 = time_parts[1].parse().ok()?;
    let sec: u32 = time_parts[2].parse().ok()?;
    if hour >= 24 || min >= 60 || sec > 60 {
        return None;
    }

    // Validate timezone is GMT or UTC
    let tz = parts[5].to_ascii_uppercase();
    if tz != "GMT" && tz != "UTC" {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let target_epoch_secs = days
        .checked_mul(86400)?
        .checked_add((hour as i64) * 3600 + (min as i64) * 60 + (sec as i64))?;

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if target_epoch_secs <= now_secs {
        // Target is in the past: 0s wait
        Some(0)
    } else {
        Some((target_epoch_secs - now_secs) as u64)
    }
}

/// Convert Gregorian civil date to days since 1970-01-01 (Unix Epoch).
/// Based on Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// Convert days since Unix Epoch back to Gregorian civil date (year, month, day).
/// Based on Howard Hinnant's inverse algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// Format a Unix epoch timestamp as an RFC 7231 / RFC 9110 IMF-fixdate string.
pub fn format_http_date(epoch_secs: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

    let days = (epoch_secs / 86400) as i64;
    let day_secs = (epoch_secs % 86400) as u32;
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;

    let (y, m, d) = civil_from_days(days);
    let dow = DAYS[((days % 7 + 7) % 7) as usize];

    format!(
        "{dow}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        d,
        MONTHS[(m.saturating_sub(1)) as usize % 12],
        y,
        hour,
        min,
        sec
    )
}

/// Calculate the delay duration before the next retry attempt with deterministic jitter.
///
/// Respects provider `retry_after_secs` up to `policy.max_retry_after_secs`.
/// If provider requests longer than `max_retry_after_secs`, returns `Duration::MAX`
/// signaling that waiting would violate bounded latency requirements.
/// Otherwise, applies exponential backoff clamped to [75%, 125%] jitter and `policy.max_backoff_ms`.
pub fn calculate_backoff_with_jitter(
    attempt: u32,
    retry_after: Option<u64>,
    policy: &RetryPolicy,
    jitter_pct: u64,
) -> Duration {
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

    // Apply bounded jitter: clamped to [75, 125]
    let clamped_jitter = jitter_pct.clamp(75, 125);
    let jittered_ms = (base_ms.saturating_mul(clamped_jitter)) / 100;
    let final_ms = jittered_ms.max(1).min(policy.max_backoff_ms);

    Duration::from_millis(final_ms)
}

/// Calculate the delay duration before the next retry attempt using non-locking pseudo-random jitter.
pub fn calculate_backoff(attempt: u32, retry_after: Option<u64>, policy: &RetryPolicy) -> Duration {
    calculate_backoff_with_jitter(attempt, retry_after, policy, compute_jitter_pct())
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
    fn test_backoff_calculation_deterministic() {
        let policy = RetryPolicy {
            initial_backoff_ms: 100,
            max_backoff_ms: 1000,
            max_retry_after_secs: 5,
            ..Default::default()
        };

        // Attempt 0: base = 100ms
        assert_eq!(
            calculate_backoff_with_jitter(0, None, &policy, 75),
            Duration::from_millis(75)
        );
        assert_eq!(
            calculate_backoff_with_jitter(0, None, &policy, 100),
            Duration::from_millis(100)
        );
        assert_eq!(
            calculate_backoff_with_jitter(0, None, &policy, 125),
            Duration::from_millis(125)
        );

        // Attempt 1: base = 200ms
        assert_eq!(
            calculate_backoff_with_jitter(1, None, &policy, 100),
            Duration::from_millis(200)
        );

        // Clamping to max_backoff
        assert_eq!(
            calculate_backoff_with_jitter(5, None, &policy, 100),
            Duration::from_millis(1000)
        );
    }

    #[test]
    fn test_compute_jitter_pct_bounds_and_entropy() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let j = compute_jitter_pct();
            assert!((75..=125).contains(&j), "Jitter {j} out of [75, 125] range");
            seen.insert(j);
        }
        // Ensure values are not stuck on a constant
        assert!(
            seen.len() > 30,
            "Jitter distribution too narrow: {} distinct values",
            seen.len()
        );
    }

    #[test]
    fn test_parse_retry_after_delay_seconds() {
        assert_eq!(parse_retry_after_header("120"), Some(120));
        assert_eq!(parse_retry_after_header("0"), Some(0));
        assert_eq!(parse_retry_after_header("  45  "), Some(45));
        assert_eq!(parse_retry_after_header("\"60\""), Some(60));
        assert_eq!(parse_retry_after_header("-5"), None);
        assert_eq!(parse_retry_after_header(""), None);
        assert_eq!(parse_retry_after_header("invalid"), None);
    }

    #[test]
    fn test_parse_retry_after_http_date() {
        // 1. Past date: must safely clamp to Some(0) without underflowing
        let past = "Sun, 06 Nov 1994 08:49:37 GMT";
        assert_eq!(parse_retry_after_header(past), Some(0));

        // 2. Future date formatted via helper
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let future_epoch = now_epoch + 15;
        let formatted = format_http_date(future_epoch);
        let parsed = parse_retry_after_header(&formatted);
        assert!(parsed.is_some());
        let secs = parsed.unwrap();
        // Allow minor execution jitter (13..=15s)
        assert!(
            (13..=15).contains(&secs),
            "Parsed {} from future date",
            secs
        );

        // 3. Malformed dates return None
        assert_eq!(
            parse_retry_after_header("Sun, 32 Jan 2026 00:00:00 GMT"),
            None
        );
        assert_eq!(
            parse_retry_after_header("Mon, 01 Jan 2026 25:00:00 GMT"),
            None
        );
        assert_eq!(
            parse_retry_after_header("Tue, 20 Oct 2026 12:00:00 EST"),
            None
        ); // Non-GMT
    }
}
