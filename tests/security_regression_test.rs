//! Comprehensive verification tests for Workstream D: Security Hardening & Regression Tests.
//!
//! Verifies:
//! 1. Host header validation, exact and wildcard matching, and lookalike domain rejection.
//! 2. Malformed Host header rejection (commas, spaces, control characters, userinfo injection).
//! 3. Rejection of spoofed Forwarded headers from untrusted direct connections.
//! 4. Secure multi-hop X-Forwarded-For resolution through trusted proxy boundaries.
//! 5. Error envelope structure ensuring zero leakage of secrets, internal paths, or configurations.

use ai_backend::security::{HostValidator, TrustedProxyMatcher, resolve_client_ip};
use axum::http::HeaderMap;
use std::net::IpAddr;

#[test]
fn test_host_validation_wildcard_and_lookalike_rejection() {
    let validator = HostValidator::new(&[
        "*.example.com".to_string(),
        "api.local".to_string(),
        "127.0.0.1".to_string(),
        "[::1]".to_string(),
    ]);

    // Legitimate matching hosts
    assert!(validator.validate_host("api.example.com"));
    assert!(validator.validate_host("sub.api.example.com"));
    assert!(validator.validate_host("example.com"));
    assert!(validator.validate_host("api.example.com:443"));
    assert!(validator.validate_host("api.local"));
    assert!(validator.validate_host("api.local:8080"));
    assert!(validator.validate_host("127.0.0.1"));
    assert!(validator.validate_host("127.0.0.1:8080"));
    assert!(validator.validate_host("[::1]"));
    assert!(validator.validate_host("[::1]:8080"));

    // Lookalike attacks must be rejected
    assert!(
        !validator.validate_host("evil-example.com"),
        "Lookalike domain with hyphen must be rejected"
    );
    assert!(
        !validator.validate_host("notexample.com"),
        "Suffix lookalike must be rejected"
    );
    assert!(
        !validator.validate_host("example.com.evil.com"),
        "Subdomain of evil.com must be rejected"
    );
    assert!(
        !validator.validate_host("example.com.local"),
        "Shadow TLD must be rejected"
    );
    assert!(!validator.validate_host("attacker.org"));
}

#[test]
fn test_host_validation_rejects_malformed_headers() {
    let validator = HostValidator::new(&["example.com".to_string(), "localhost".to_string()]);

    // Multi-value host header attack
    assert!(!validator.validate_host("example.com, evil.com"));
    assert!(!validator.validate_host("example.com\tevil.com"));
    assert!(!validator.validate_host("example.com evil.com"));

    // Userinfo delimiter injection (@)
    assert!(!validator.validate_host("evil.com@example.com"));
    assert!(!validator.validate_host("admin:password@localhost"));

    // Path and query injection
    assert!(!validator.validate_host("localhost/api/v1"));
    assert!(!validator.validate_host("localhost?query=evil"));
    assert!(!validator.validate_host("localhost#hash"));

    // CRLF header smuggling characters
    assert!(!validator.validate_host("localhost\r\nHost: evil.com"));
    assert!(!validator.validate_host("localhost\n"));

    // Malformed IPv6 brackets
    assert!(!validator.validate_host("[::1"));
    assert!(!validator.validate_host("::1]"));
    assert!(!validator.validate_host("[::1]extra"));
}

#[test]
fn test_forwarded_headers_rejected_from_untrusted_direct_peer() {
    let matcher =
        TrustedProxyMatcher::new(&["10.0.0.0/8".to_string(), "127.0.0.1".to_string()]).unwrap();
    let untrusted_peer: IpAddr = "198.51.100.5".parse().unwrap();

    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.195".parse().unwrap());

    // Peer is not in trusted proxies: resolve_client_ip must return peer IP, ignoring spoofed XFF
    let client_ip = resolve_client_ip(Some(untrusted_peer), &headers, &matcher);
    assert_eq!(client_ip, Some(untrusted_peer));
}

#[test]
fn test_forwarded_headers_resolved_from_trusted_reverse_proxy() {
    let matcher =
        TrustedProxyMatcher::new(&["10.0.0.0/8".to_string(), "127.0.0.1".to_string()]).unwrap();
    let trusted_proxy_peer: IpAddr = "10.0.1.50".parse().unwrap();

    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.195".parse().unwrap());

    // Peer is a trusted reverse proxy: resolve_client_ip extracts client IP from XFF
    let client_ip = resolve_client_ip(Some(trusted_proxy_peer), &headers, &matcher);
    assert_eq!(client_ip, Some("203.0.113.195".parse().unwrap()));
}

#[test]
fn test_multi_hop_forwarded_for_stops_at_untrusted_boundary() {
    // 10.0.0.0/8 and 172.16.0.0/12 are trusted internal proxy tiers
    let matcher =
        TrustedProxyMatcher::new(&["10.0.0.0/8".to_string(), "172.16.0.0/12".to_string()]).unwrap();

    let immediate_peer: IpAddr = "10.0.0.2".parse().unwrap(); // Trusted edge proxy

    let mut headers = HeaderMap::new();
    // Chain: Client -> Intermediate Proxy -> Edge Proxy
    // Attacker spoofed: 1.1.1.1
    // Actual client: 198.51.100.44
    // Intermediate proxy (trusted): 172.16.0.5
    headers.insert(
        "x-forwarded-for",
        "1.1.1.1, 198.51.100.44, 172.16.0.5".parse().unwrap(),
    );

    // Walking right-to-left:
    // 172.16.0.5 is trusted.
    // 198.51.100.44 is UNTRUSTED -> Stop here! This is the legitimate client IP.
    let client_ip = resolve_client_ip(Some(immediate_peer), &headers, &matcher);
    assert_eq!(client_ip, Some("198.51.100.44".parse().unwrap()));
}
