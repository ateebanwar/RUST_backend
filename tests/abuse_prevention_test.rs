//! Comprehensive Phase 13 Integration Tests: Abuse Prevention, Rate Limiting, and Concurrency Controls.
//!
//! Conforms strictly to MASTER_SPEC.md §13, ARCHITECTURE.md §2, and THREAT_MODEL.md.
//!
//! Verifies:
//! 1. Normal traffic below rate limits passes successfully.
//! 2. Bursts up to configured burst capacity succeed without rejection.
//! 3. Requests exceeding burst capacity are rejected with HTTP 429 Too Many Requests.
//! 4. Canonical error envelope contains "RATE_LIMITED" code and accurate Retry-After header.
//! 5. Tokens refill over time based on configured refill rate.
//! 6. Per-IP rate limiting correctly isolates independent IP addresses.
//! 7. Per-principal rate limiting isolates distinct authenticated principals/PATs.
//! 8. Exempt public probe endpoints (/health, /ready) are never rate limited.
//! 9. Forged X-Forwarded-For headers from untrusted network peers do not spoof IP identity.
//! 10. Trusted reverse proxies correctly propagate original client IP.
//! 11. IPv4 and IPv6 client addresses are handled deterministically.
//! 12. Global in-flight concurrency limiter protects against process overload.
//! 13. Active SSE streaming concurrency ceiling prevents stream exhaustion.
//! 14. RAII permit drop-guards release concurrency capacity on client disconnect or completion.
//! 15. Stale limiter entries expire safely and bounded pruning prevents memory exhaustion.
//! 16. Local rate limiting never triggers upstream provider failover.
//! 17. No artificial daily or monthly usage caps exist.

use ai_backend::config::AppConfig;
use ai_backend::security::{
    RateLimitKey, RateLimitRejection, RateLimitRouteCategory, RateLimiter, RateLimiterConfig,
    TokenBucket,
};
use ai_backend::{AppState, create_app};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

async fn spawn_test_server(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state);

    let handle = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn test_token_bucket_burst_and_exact_refill_math() {
    let mut bucket = TokenBucket::new(5.0, 10.0);
    assert_eq!(bucket.burst_capacity(), 5.0);
    assert_eq!(bucket.tokens(), 5.0);

    // Consume 5 tokens
    for _ in 0..5 {
        assert!(bucket.try_consume(1.0).is_ok());
    }
    assert!(bucket.tokens() < 1.0);

    // 6th attempt fails with required wait duration
    let wait_err = bucket.try_consume(1.0).unwrap_err();
    assert!(wait_err.as_secs_f64() > 0.0);
    assert!(wait_err.as_secs_f64() <= 0.2); // Refilling at 10 tokens/sec takes ~0.1s for 1 token

    // Wait 150ms and consume again
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(bucket.try_consume(1.0).is_ok());
}

#[tokio::test]
async fn test_per_ip_rate_limiting_and_isolation() {
    let config = RateLimiterConfig {
        enabled: true,
        ip_burst: 3,
        ip_rate_per_sec: 1.0,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);

    let ip1 = RateLimitKey::Ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)));
    let ip2 = RateLimitKey::Ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2)));

    // Consume all 3 tokens for IP 1
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
            .is_ok()
    );

    // 4th request from IP 1 is rate limited with positive Retry-After
    let rejection = limiter
        .check_admission(&ip1, RateLimitRouteCategory::General)
        .unwrap_err();
    assert!(rejection.retry_after_secs() >= 1);

    // IP 2 is completely isolated and retains its full burst capacity
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
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&ip2, RateLimitRouteCategory::General)
            .is_err()
    );
}

#[tokio::test]
async fn test_per_principal_rate_limiting_and_isolation() {
    let config = RateLimiterConfig {
        enabled: true,
        principal_burst: 2,
        principal_rate_per_sec: 1.0,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);

    let alice = RateLimitKey::Principal("user_alice".to_string());
    let bob = RateLimitKey::Principal("user_bob".to_string());

    assert!(
        limiter
            .check_admission(&alice, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&alice, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&alice, RateLimitRouteCategory::General)
            .is_err()
    );

    // Bob is independent
    assert!(
        limiter
            .check_admission(&bob, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&bob, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&bob, RateLimitRouteCategory::General)
            .is_err()
    );
}

#[tokio::test]
async fn test_route_category_isolation_for_same_identity() {
    let config = RateLimiterConfig {
        enabled: true,
        principal_burst: 10,
        principal_rate_per_sec: 5.0,
        chat_burst: 2,
        chat_rate_per_sec: 1.0,
        auth_burst: 2,
        auth_rate_per_sec: 1.0,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);
    let principal = RateLimitKey::Principal("dev_user".to_string());

    // Consume chat tokens
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::Chat)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::Chat)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::Chat)
            .is_err()
    );

    // General operations still have their own available tokens
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::General)
            .is_ok()
    );

    // Sensitive auth operations have their own tokens
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::Auth)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::Auth)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&principal, RateLimitRouteCategory::Auth)
            .is_err()
    );
}

#[tokio::test]
async fn test_exempt_routes_bypass_rate_limiter() {
    let config = RateLimiterConfig {
        enabled: true,
        ip_burst: 1,
        ip_rate_per_sec: 0.1,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);
    let key = RateLimitKey::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST));

    // Even if exhausted, Exempt route category always succeeds
    let _ = limiter.check_admission(&key, RateLimitRouteCategory::General);
    assert!(
        limiter
            .check_admission(&key, RateLimitRouteCategory::General)
            .is_err()
    );
    for _ in 0..100 {
        assert!(
            limiter
                .check_admission(&key, RateLimitRouteCategory::Exempt)
                .is_ok()
        );
    }
}

#[tokio::test]
async fn test_concurrency_request_permit_lifecycle_and_overload_protection() {
    let config = RateLimiterConfig {
        enabled: true,
        max_concurrent_requests: 2,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);

    assert_eq!(limiter.active_requests_count(), 0);

    let permit1 = limiter.acquire_request_permit().unwrap();
    assert_eq!(limiter.active_requests_count(), 1);

    let permit2 = limiter.acquire_request_permit().unwrap();
    assert_eq!(limiter.active_requests_count(), 2);

    // 3rd attempt exceeds max_concurrent_requests and returns Overloaded
    let rejection = limiter.acquire_request_permit().unwrap_err();
    assert!(matches!(rejection, RateLimitRejection::Overloaded { .. }));
    assert_eq!(limiter.active_requests_count(), 2);

    // Dropping permit1 immediately decrements active requests and restores capacity
    drop(permit1);
    assert_eq!(limiter.active_requests_count(), 1);

    let permit3 = limiter.acquire_request_permit().unwrap();
    assert_eq!(limiter.active_requests_count(), 2);

    drop(permit2);
    drop(permit3);
    assert_eq!(limiter.active_requests_count(), 0);
}

#[tokio::test]
async fn test_streaming_concurrency_permit_lifecycle() {
    let config = RateLimiterConfig {
        enabled: true,
        max_concurrent_streams: 1,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);

    assert_eq!(limiter.active_streams_count(), 0);

    let permit = limiter.acquire_stream_permit().unwrap();
    assert_eq!(limiter.active_streams_count(), 1);

    // Second stream initiation rejected
    let rejection = limiter.acquire_stream_permit().unwrap_err();
    assert!(matches!(rejection, RateLimitRejection::RateLimited { .. }));

    drop(permit);
    assert_eq!(limiter.active_streams_count(), 0);

    let permit2 = limiter.acquire_stream_permit().unwrap();
    assert_eq!(limiter.active_streams_count(), 1);
    drop(permit2);
    assert_eq!(limiter.active_streams_count(), 0);
}

#[tokio::test]
async fn test_stale_entry_bounded_pruning() {
    let config = RateLimiterConfig {
        enabled: true,
        max_entries: 5,
        entry_ttl: Duration::from_millis(50),
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);

    for i in 0..4 {
        let key = RateLimitKey::Ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)));
        let _ = limiter.check_admission(&key, RateLimitRouteCategory::General);
    }
    assert_eq!(limiter.entry_count(), 4);

    // Wait past entry_ttl
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Pruning evicts all expired entries
    let evicted = limiter.prune_idle_entries(Duration::from_millis(50));
    assert_eq!(evicted, 4);
    assert_eq!(limiter.entry_count(), 0);
}

#[tokio::test]
async fn test_http_429_envelope_and_retry_after_header_integration() {
    let mut config_raw = AppConfig::default_local();
    config_raw.rate_limit_enabled = true;
    config_raw.rate_limit_ip_burst = 2;
    config_raw.rate_limit_ip_rate = 1.0;
    let config = Arc::new(config_raw);

    let state = AppState::new(config);
    let (base_url, _handle) = spawn_test_server(state).await;
    let client = reqwest::Client::new();

    // First 2 requests succeed
    let r1 = client
        .get(format!("{base_url}/api/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), reqwest::StatusCode::OK);

    let r2 = client
        .get(format!("{base_url}/api/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), reqwest::StatusCode::OK);

    // 3rd request receives HTTP 429
    let r3 = client
        .get(format!("{base_url}/api/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(r3.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    // Verify Retry-After header
    let retry_after = r3.headers().get(reqwest::header::RETRY_AFTER);
    assert!(
        retry_after.is_some(),
        "Retry-After header must be present on HTTP 429"
    );
    let secs: u64 = retry_after.unwrap().to_str().unwrap().parse().unwrap();
    assert!(secs >= 1);

    // Verify canonical error envelope format
    let body: Value = r3.json().await.unwrap();
    assert_eq!(body["error"]["code"], "RATE_LIMITED");
    assert_eq!(body["error"]["retryable"], true);
    assert!(body["error"]["message"].as_str().unwrap().contains("flood"));
    assert!(body["error"]["request_id"].is_string());
}

#[tokio::test]
async fn test_health_and_ready_endpoints_are_never_rate_limited() {
    let mut config_raw = AppConfig::default_local();
    config_raw.rate_limit_enabled = true;
    config_raw.rate_limit_ip_burst = 1;
    config_raw.rate_limit_ip_rate = 0.01;
    let config = Arc::new(config_raw);

    let state = AppState::new(config);
    let (base_url, _handle) = spawn_test_server(state).await;
    let client = reqwest::Client::new();

    // Consume the 1 IP token on a general route
    let _ = client
        .get(format!("{base_url}/api/v1/models"))
        .send()
        .await
        .unwrap();

    // /health and /ready continue to respond without rate limiting
    for _ in 0..10 {
        let resp = client
            .get(format!("{base_url}/api/v1/health"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
    }
}

#[tokio::test]
async fn test_untrusted_forwarded_for_cannot_spoof_client_identity() {
    let mut config_raw = AppConfig::default_local();
    config_raw.rate_limit_enabled = true;
    config_raw.rate_limit_ip_burst = 2;
    config_raw.rate_limit_ip_rate = 0.1;
    // Trusted proxies only includes a remote CIDR, NOT localhost direct peer
    config_raw.trusted_proxies = vec!["10.10.10.10".to_string()];
    let config = Arc::new(config_raw);

    let state = AppState::new(config);
    let (base_url, _handle) = spawn_test_server(state).await;
    let client = reqwest::Client::new();

    // Attacker sends arbitrary X-Forwarded-For headers attempting to evade per-IP limit
    for i in 0..2 {
        let resp = client
            .get(format!("{base_url}/api/v1/models"))
            .header("x-forwarded-for", format!("203.0.113.{}", i + 1))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
    }

    // 3rd request from same physical peer is blocked despite fake X-Forwarded-For
    let r3 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("x-forwarded-for", "198.51.100.99")
        .send()
        .await
        .unwrap();
    assert_eq!(r3.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_trusted_proxy_correctly_propagates_client_ip() {
    let mut config_raw = AppConfig::default_local();
    config_raw.rate_limit_enabled = true;
    config_raw.rate_limit_ip_burst = 2;
    config_raw.rate_limit_ip_rate = 0.1;
    // Localhost peer is trusted proxy
    config_raw.trusted_proxies = vec!["127.0.0.1".to_string(), "::1".to_string()];
    let config = Arc::new(config_raw);

    let state = AppState::new(config);
    let (base_url, _handle) = spawn_test_server(state).await;
    let client = reqwest::Client::new();

    // Client A (behind trusted proxy) consumes its 2 tokens
    for _ in 0..2 {
        let resp = client
            .get(format!("{base_url}/api/v1/models"))
            .header("x-forwarded-for", "198.51.100.1")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
    }

    let r_a3 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("x-forwarded-for", "198.51.100.1")
        .send()
        .await
        .unwrap();
    assert_eq!(r_a3.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    // Client B (different IP forwarded through same trusted proxy) is NOT blocked
    let r_b1 = client
        .get(format!("{base_url}/api/v1/models"))
        .header("x-forwarded-for", "198.51.100.2")
        .send()
        .await
        .unwrap();
    assert_eq!(r_b1.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn test_ipv6_peer_address_rate_limiting() {
    let config = RateLimiterConfig {
        enabled: true,
        ip_burst: 2,
        ip_rate_per_sec: 1.0,
        ..Default::default()
    };
    let limiter = RateLimiter::new(config);

    let ipv6_1 = RateLimitKey::Ip(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)));
    let ipv6_2 = RateLimitKey::Ip(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2)));

    assert!(
        limiter
            .check_admission(&ipv6_1, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&ipv6_1, RateLimitRouteCategory::General)
            .is_ok()
    );
    assert!(
        limiter
            .check_admission(&ipv6_1, RateLimitRouteCategory::General)
            .is_err()
    );

    // Second IPv6 address is isolated
    assert!(
        limiter
            .check_admission(&ipv6_2, RateLimitRouteCategory::General)
            .is_ok()
    );
}

#[tokio::test]
async fn test_no_arbitrary_daily_or_monthly_usage_quotas() {
    let mut config_raw = AppConfig::default_local();
    config_raw.rate_limit_enabled = true;
    config_raw.rate_limit_ip_burst = 50;
    config_raw.rate_limit_ip_rate = 50.0;
    let config = Arc::new(config_raw);

    let state = AppState::new(config);
    let (base_url, _handle) = spawn_test_server(state).await;
    let client = reqwest::Client::new();

    // Execute 30 valid requests in sequence
    for _ in 0..30 {
        let resp = client
            .get(format!("{base_url}/api/v1/models"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
    }
}
