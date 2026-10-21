//! Inbound request rate limiting, abuse prevention, and concurrency controls.
//!
//! Conforms strictly to Phase 13 specifications in MASTER_SPEC.md §13,
//! ARCHITECTURE.md §2, and THREAT_MODEL.md.
//!
//! Provides a configurable multi-dimensional token bucket rate limiter:
//! - Per-IP unauthenticated flood defense.
//! - Per-authenticated-principal generous personal/family rate limiting.
//! - Route-categorized thresholds (general, chat inference, sensitive auth).
//! - Global in-flight concurrency bounding (overload protection).
//! - Active SSE streaming concurrency bounding with RAII drop-guards.
//! - Autonomous bounded memory cleanup and stale entry eviction.
//! - Canonical HTTP 429 / 503 error envelopes with accurate `Retry-After` headers.

use crate::errors::AppError;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Standard token bucket algorithm with monotonic timing and bounded capacity.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    burst_capacity: f64,
    refill_rate_per_sec: f64,
}

impl TokenBucket {
    /// Create a new TokenBucket initialized to maximum burst capacity.
    pub fn new(burst_capacity: f64, refill_rate_per_sec: f64) -> Self {
        let burst = burst_capacity.max(1.0);
        let rate = refill_rate_per_sec.max(0.01);
        Self {
            tokens: burst,
            last_refill: Instant::now(),
            burst_capacity: burst,
            refill_rate_per_sec: rate,
        }
    }

    /// Attempt to consume the specified number of tokens.
    ///
    /// If sufficient tokens are available, decrements the balance and returns `Ok(())`.
    /// If insufficient tokens remain, returns `Err(wait_duration)` needed to accumulate
    /// the required tokens.
    pub fn try_consume(&mut self, cost: f64) -> Result<(), Duration> {
        let now = Instant::now();
        let elapsed_secs = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();

        if elapsed_secs.is_finite() && elapsed_secs > 0.0 {
            let refilled = elapsed_secs * self.refill_rate_per_sec;
            self.tokens = (self.tokens + refilled).min(self.burst_capacity);
            self.last_refill = now;
        }

        if self.tokens >= cost {
            self.tokens -= cost;
            Ok(())
        } else {
            let deficit = cost - self.tokens;
            let wait_secs = if self.refill_rate_per_sec > 0.0 {
                deficit / self.refill_rate_per_sec
            } else {
                1.0
            };
            Err(Duration::from_secs_f64(wait_secs.max(0.01)))
        }
    }

    /// Current available token balance.
    pub fn tokens(&self) -> f64 {
        self.tokens
    }

    /// Maximum burst capacity.
    pub fn burst_capacity(&self) -> f64 {
        self.burst_capacity
    }

    /// Refill rate in tokens per second.
    pub fn refill_rate_per_sec(&self) -> f64 {
        self.refill_rate_per_sec
    }
}

/// Identifiers for multi-dimensional rate limiting.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    /// Origin client IP address.
    Ip(IpAddr),
    /// Authenticated principal subject identifier (e.g. "user_123" or "dev_laptop_key").
    Principal(String),
}

/// Endpoint abuse and resource cost categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitRouteCategory {
    /// Health and readiness probes (strictly exempt from rate limits).
    Exempt,
    /// General lightweight read/write operations (models, providers, usage, quotas).
    General,
    /// Expensive generative AI inference operations (chat completion, streaming, routing preview).
    Chat,
    /// Sensitive credential lifecycle operations (PAT creation, rotation, revocation).
    Auth,
}

impl RateLimitRouteCategory {
    /// Determine route category from inbound request path.
    pub fn from_path(path: &str) -> Self {
        if path == "/api/v1/health" || path == "/api/v1/ready" {
            Self::Exempt
        } else if path.starts_with("/api/v1/chat/") || path == "/api/v1/routing/preview" {
            Self::Chat
        } else if path.starts_with("/api/v1/auth/tokens") {
            Self::Auth
        } else {
            Self::General
        }
    }
}

/// Configuration governing abuse prevention, burst capacity, and concurrency bounds.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Whether rate limiting and abuse controls are actively enforced.
    pub enabled: bool,
    /// Unauthenticated IP burst limit (default: 100 requests).
    pub ip_burst: u32,
    /// Unauthenticated IP sustained refill rate (default: 20.0 req/s).
    pub ip_rate_per_sec: f64,
    /// Authenticated principal general burst limit (default: 200 requests).
    pub principal_burst: u32,
    /// Authenticated principal general sustained refill rate (default: 50.0 req/s).
    pub principal_rate_per_sec: f64,
    /// Chat inference burst limit per principal (default: 60 requests).
    pub chat_burst: u32,
    /// Chat inference sustained refill rate per principal (default: 15.0 req/s).
    pub chat_rate_per_sec: f64,
    /// Sensitive credential management burst limit per principal (default: 40 requests).
    pub auth_burst: u32,
    /// Sensitive credential management refill rate per principal (default: 10.0 req/s).
    pub auth_rate_per_sec: f64,
    /// Maximum global in-flight HTTP requests before temporary overload rejection (default: 200).
    pub max_concurrent_requests: usize,
    /// Maximum simultaneous active streaming connections (default: 50).
    pub max_concurrent_streams: usize,
    /// Maximum tracked limiter keys in memory before LRU eviction (default: 10,000).
    pub max_entries: usize,
    /// Idle entry expiration time-to-live before pruning (default: 300 seconds).
    pub entry_ttl: Duration,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ip_burst: 100,
            ip_rate_per_sec: 20.0,
            principal_burst: 200,
            principal_rate_per_sec: 50.0,
            chat_burst: 60,
            chat_rate_per_sec: 15.0,
            auth_burst: 40,
            auth_rate_per_sec: 10.0,
            max_concurrent_requests: 200,
            max_concurrent_streams: 50,
            max_entries: 10_000,
            entry_ttl: Duration::from_secs(300),
        }
    }
}

/// Rejection details when an inbound request exceeds rate limits or concurrency ceilings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitRejection {
    /// Request rejected due to token bucket depletion.
    RateLimited {
        retry_after_secs: u64,
        reason: String,
    },
    /// Request rejected due to process-wide concurrency exhaustion.
    Overloaded {
        retry_after_secs: u64,
        reason: String,
    },
}

impl RateLimitRejection {
    /// Suggested Retry-After duration in integer seconds.
    pub fn retry_after_secs(&self) -> u64 {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            Self::Overloaded {
                retry_after_secs, ..
            } => *retry_after_secs,
        }
    }

    /// Converts rejection into canonical application error envelope.
    pub fn to_app_error(&self) -> AppError {
        match self {
            Self::RateLimited {
                retry_after_secs,
                reason,
            } => AppError::RateLimited {
                message: reason.clone(),
                retry_after_secs: Some(*retry_after_secs),
            },
            Self::Overloaded {
                retry_after_secs,
                reason,
            } => AppError::RateLimited {
                message: format!("Temporary service overload: {reason}"),
                retry_after_secs: Some(*retry_after_secs),
            },
        }
    }
}

struct BucketEntry {
    bucket: TokenBucket,
    last_access: Instant,
}

/// RAII permit representing an in-flight HTTP request.
/// Decrements global in-flight counter upon drop.
#[derive(Debug)]
pub struct RequestPermit {
    counter: Arc<AtomicUsize>,
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// RAII permit representing an active SSE streaming request.
/// Decrements active stream counter upon drop (including client disconnects or panics).
#[derive(Debug)]
pub struct StreamPermit {
    counter: Arc<AtomicUsize>,
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Centralized, production-grade rate limiting and abuse prevention service.
pub struct RateLimiter {
    config: RateLimiterConfig,
    buckets: RwLock<HashMap<(RateLimitKey, RateLimitRouteCategory), BucketEntry>>,
    active_requests: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
}

impl RateLimiter {
    /// Initialize the rate limiter with the specified configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            buckets: RwLock::new(HashMap::new()),
            active_requests: Arc::new(AtomicUsize::new(0)),
            active_streams: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Access active configuration.
    pub fn config(&self) -> &RateLimiterConfig {
        &self.config
    }

    /// Check rate limit admission for the given key and route category.
    pub fn check_admission(
        &self,
        key: &RateLimitKey,
        category: RateLimitRouteCategory,
    ) -> Result<(), RateLimitRejection> {
        if !self.config.enabled || category == RateLimitRouteCategory::Exempt {
            return Ok(());
        }

        let (burst, rate) = match (key, category) {
            (RateLimitKey::Ip(_), RateLimitRouteCategory::Auth) => {
                (self.config.auth_burst as f64, self.config.auth_rate_per_sec)
            }
            (RateLimitKey::Ip(_), _) => (self.config.ip_burst as f64, self.config.ip_rate_per_sec),
            (RateLimitKey::Principal(_), RateLimitRouteCategory::Chat) => {
                (self.config.chat_burst as f64, self.config.chat_rate_per_sec)
            }
            (RateLimitKey::Principal(_), RateLimitRouteCategory::Auth) => {
                (self.config.auth_burst as f64, self.config.auth_rate_per_sec)
            }
            (RateLimitKey::Principal(_), _) => (
                self.config.principal_burst as f64,
                self.config.principal_rate_per_sec,
            ),
        };

        let mut buckets = self.buckets.write().unwrap_or_else(|p| p.into_inner());

        // Opportunistic bounded memory pruning when key count exceeds threshold
        if buckets.len() >= self.config.max_entries {
            let ttl = self.config.entry_ttl;
            let now = Instant::now();
            buckets.retain(|_, entry| now.saturating_duration_since(entry.last_access) < ttl);
        }

        let bucket_key = (key.clone(), category);
        let entry = buckets.entry(bucket_key).or_insert_with(|| BucketEntry {
            bucket: TokenBucket::new(burst, rate),
            last_access: Instant::now(),
        });

        match entry.bucket.try_consume(1.0) {
            Ok(()) => {
                entry.last_access = Instant::now();
                Ok(())
            }
            Err(wait_dur) => {
                let retry_after = (wait_dur.as_secs_f64().ceil() as u64).max(1);
                Err(RateLimitRejection::RateLimited {
                    retry_after_secs: retry_after,
                    reason: match key {
                        RateLimitKey::Ip(_) => {
                            "Client IP request flood threshold exceeded. Please retry later."
                                .to_string()
                        }
                        RateLimitKey::Principal(_) => {
                            "Principal rate limit burst exceeded. Please slow down your requests."
                                .to_string()
                        }
                    },
                })
            }
        }
    }

    /// Acquire an in-flight request permit, guarding against global process overload.
    pub fn acquire_request_permit(&self) -> Result<RequestPermit, RateLimitRejection> {
        if !self.config.enabled {
            return Ok(RequestPermit {
                counter: self.active_requests.clone(),
            });
        }

        let current = self.active_requests.load(Ordering::Relaxed);
        if current >= self.config.max_concurrent_requests {
            return Err(RateLimitRejection::Overloaded {
                retry_after_secs: 1,
                reason: "Backend is currently operating at maximum concurrent capacity. Please retry shortly.".to_string(),
            });
        }

        self.active_requests.fetch_add(1, Ordering::SeqCst);
        Ok(RequestPermit {
            counter: self.active_requests.clone(),
        })
    }

    /// Acquire a stream concurrency permit, guarding against excessive parallel SSE streams.
    pub fn acquire_stream_permit(&self) -> Result<StreamPermit, RateLimitRejection> {
        if !self.config.enabled {
            return Ok(StreamPermit {
                counter: self.active_streams.clone(),
            });
        }

        let current = self.active_streams.load(Ordering::Relaxed);
        if current >= self.config.max_concurrent_streams {
            return Err(RateLimitRejection::RateLimited {
                retry_after_secs: 2,
                reason: "Active streaming concurrency limit reached. Please wait for an existing stream to complete.".to_string(),
            });
        }

        self.active_streams.fetch_add(1, Ordering::SeqCst);
        Ok(StreamPermit {
            counter: self.active_streams.clone(),
        })
    }

    /// Prune entries older than the specified duration, returning the number of evicted keys.
    pub fn prune_idle_entries(&self, max_idle: Duration) -> usize {
        let mut buckets = self.buckets.write().unwrap_or_else(|p| p.into_inner());
        let before_len = buckets.len();
        let now = Instant::now();
        buckets.retain(|_, entry| now.saturating_duration_since(entry.last_access) < max_idle);
        before_len.saturating_sub(buckets.len())
    }

    /// Count of currently tracked keys in memory.
    pub fn entry_count(&self) -> usize {
        self.buckets.read().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Current number of active in-flight requests.
    pub fn active_requests_count(&self) -> usize {
        self.active_requests.load(Ordering::Relaxed)
    }

    /// Current number of active streaming connections.
    pub fn active_streams_count(&self) -> usize {
        self.active_streams.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_burst_and_refill() {
        let mut bucket = TokenBucket::new(5.0, 10.0);
        assert_eq!(bucket.tokens(), 5.0);

        // Consume 5 tokens immediately
        for _ in 0..5 {
            assert!(bucket.try_consume(1.0).is_ok());
        }

        // 6th attempt fails with required wait duration
        let err = bucket.try_consume(1.0);
        assert!(err.is_err());
        let wait_dur = err.unwrap_err();
        assert!(wait_dur.as_secs_f64() > 0.0);
        assert!(wait_dur.as_secs_f64() <= 0.15);
    }

    #[test]
    fn test_token_bucket_refills_over_time() {
        let mut bucket = TokenBucket::new(2.0, 100.0);
        assert!(bucket.try_consume(2.0).is_ok());
        assert!(bucket.try_consume(1.0).is_err());

        std::thread::sleep(Duration::from_millis(30));
        // At 100 tokens/sec, 30ms refills ~3 tokens, capped at burst (2.0)
        assert!(bucket.try_consume(1.0).is_ok());
    }

    #[test]
    fn test_rate_limiter_ip_isolation() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            ip_burst: 2,
            ip_rate_per_sec: 1.0,
            ..Default::default()
        });

        let ip1 = RateLimitKey::Ip("192.0.2.1".parse().unwrap());
        let ip2 = RateLimitKey::Ip("192.0.2.2".parse().unwrap());

        // IP 1 consumes all burst
        assert!(
            limiter
                .check_admission(&ip1, RateLimitRouteCategory::General)
                .is_ok()
        );
        assert!(
            limiter
                .check_admission(&ip1, RateLimitRouteCategory::General)
                .is_ok()
        );
        assert!(
            limiter
                .check_admission(&ip1, RateLimitRouteCategory::General)
                .is_err()
        );

        // IP 2 is completely unaffected
        assert!(
            limiter
                .check_admission(&ip2, RateLimitRouteCategory::General)
                .is_ok()
        );
        assert!(
            limiter
                .check_admission(&ip2, RateLimitRouteCategory::General)
                .is_ok()
        );
        assert!(
            limiter
                .check_admission(&ip2, RateLimitRouteCategory::General)
                .is_err()
        );
    }

    #[test]
    fn test_rate_limiter_principal_isolation() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            principal_burst: 3,
            principal_rate_per_sec: 1.0,
            ..Default::default()
        });

        let user1 = RateLimitKey::Principal("user_alice".to_string());
        let user2 = RateLimitKey::Principal("user_bob".to_string());

        for _ in 0..3 {
            assert!(
                limiter
                    .check_admission(&user1, RateLimitRouteCategory::General)
                    .is_ok()
            );
        }
        assert!(
            limiter
                .check_admission(&user1, RateLimitRouteCategory::General)
                .is_err()
        );

        // User 2 has independent bucket
        assert!(
            limiter
                .check_admission(&user2, RateLimitRouteCategory::General)
                .is_ok()
        );
    }

    #[test]
    fn test_exempt_routes_never_rate_limited() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            ip_burst: 1,
            ip_rate_per_sec: 0.1,
            ..Default::default()
        });

        let key = RateLimitKey::Ip("127.0.0.1".parse().unwrap());
        for _ in 0..100 {
            assert!(
                limiter
                    .check_admission(&key, RateLimitRouteCategory::Exempt)
                    .is_ok()
            );
        }
    }

    #[test]
    fn test_request_permit_drop_releases_concurrency() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            max_concurrent_requests: 2,
            ..Default::default()
        });

        let permit1 = limiter.acquire_request_permit().unwrap();
        let permit2 = limiter.acquire_request_permit().unwrap();
        assert_eq!(limiter.active_requests_count(), 2);

        // 3rd attempt exceeds concurrency
        assert!(limiter.acquire_request_permit().is_err());

        // Drop permit1 -> frees slot
        drop(permit1);
        assert_eq!(limiter.active_requests_count(), 1);
        let permit3 = limiter.acquire_request_permit().unwrap();
        assert_eq!(limiter.active_requests_count(), 2);
        drop(permit2);
        drop(permit3);
        assert_eq!(limiter.active_requests_count(), 0);
    }

    #[test]
    fn test_stream_permit_drop_releases_concurrency() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            max_concurrent_streams: 1,
            ..Default::default()
        });

        let permit = limiter.acquire_stream_permit().unwrap();
        assert_eq!(limiter.active_streams_count(), 1);
        assert!(limiter.acquire_stream_permit().is_err());

        drop(permit);
        assert_eq!(limiter.active_streams_count(), 0);
        assert!(limiter.acquire_stream_permit().is_ok());
    }

    #[test]
    fn test_prune_idle_entries() {
        let limiter = RateLimiter::new(RateLimiterConfig::default());
        let ip1 = RateLimitKey::Ip("198.51.100.1".parse().unwrap());
        let ip2 = RateLimitKey::Ip("198.51.100.2".parse().unwrap());

        limiter
            .check_admission(&ip1, RateLimitRouteCategory::General)
            .unwrap();
        limiter
            .check_admission(&ip2, RateLimitRouteCategory::General)
            .unwrap();
        assert_eq!(limiter.entry_count(), 2);

        // Pruning with 0s max_idle evicts entries
        std::thread::sleep(Duration::from_millis(5));
        let pruned = limiter.prune_idle_entries(Duration::from_millis(2));
        assert_eq!(pruned, 2);
        assert_eq!(limiter.entry_count(), 0);
    }
}
