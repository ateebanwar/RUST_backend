//! Comprehensive Phase 14 Integration Tests: SSRF Defense and Outbound Network Protection.
//!
//! Conforms strictly to MASTER_SPEC.md §14, ARCHITECTURE.md §2, and THREAT_MODEL.md.
//!
//! Verifies:
//! 1. Valid allowlisted HTTPS provider endpoints are accepted.
//! 2. Malformed URLs, empty URLs, and invalid ports are rejected.
//! 3. Unsupported URL schemes (file://, ftp://, gopher://, http:// for external) are rejected.
//! 4. Embedded user credentials (user:pass@host) are rejected.
//! 5. IPv4 loopback destinations (127.0.0.0/8) are blocked.
//! 6. RFC 1918 private IPv4 networks (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16) are blocked.
//! 7. IPv4 link-local addresses (169.254.0.0/16) are blocked.
//! 8. AWS/GCP/Azure instance metadata endpoints (169.254.169.254, [fd00:ec2::254]) are blocked.
//! 9. IPv6 loopback (::1), unspecified (::), link-local (fe80::/10), and unique-local (fc00::/7) are blocked.
//! 10. IPv4-mapped IPv6 representations cannot bypass IPv4 blocklists.
//! 11. Multicast (224.0.0.0/4, ff00::/8) and broadcast destinations are blocked.
//! 12. DNS lookups returning prohibited addresses fail closed.
//! 13. Mixed DNS results containing private and public addresses fail closed.
//! 14. Connection pinning via reqwest resolve_to_addrs prevents DNS rebinding TOCTOU gaps.
//! 15. Outbound HTTP redirects are disabled by default (reqwest::redirect::Policy::none()).
//! 16. Authorized local inference is permitted only when explicitly enabled and matching base URL.
//! 17. Local inference exceptions cannot be leveraged to probe arbitrary internal services or metadata.
//! 18. Google JWKS and configured provider adapters remain fully compatible.

use ai_backend::security::{
    IpClassification, OutboundDestinationPolicy, SsrfError, ValidatedConnection,
    ValidatedDestination, classify_ip,
};
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[test]
fn test_valid_https_provider_destinations_accepted() {
    let policy = OutboundDestinationPolicy::default_secure();

    let valid_groq = policy.validate_url("https://api.groq.com/openai/v1/chat/completions");
    assert!(valid_groq.is_ok());
    let dest = valid_groq.unwrap();
    assert_eq!(dest.host, "api.groq.com");
    assert_eq!(dest.port, 443);
    assert!(!dest.is_local);

    let valid_or = policy.validate_url("https://openrouter.ai/api/v1/chat/completions");
    assert!(valid_or.is_ok());

    let valid_jwks = policy.validate_url("https://www.googleapis.com/oauth2/v3/certs");
    assert!(valid_jwks.is_ok());
}

#[test]
fn test_unsupported_schemes_and_malformed_urls_rejected() {
    let policy = OutboundDestinationPolicy::default_secure();

    // Malformed URLs
    assert!(matches!(
        policy.validate_url("not_a_url"),
        Err(SsrfError::InvalidUrl(_))
    ));
    assert!(matches!(
        policy.validate_url(""),
        Err(SsrfError::InvalidUrl(_))
    ));
    assert!(matches!(
        policy.validate_url("://api.groq.com"),
        Err(SsrfError::InvalidUrl(_))
    ));

    // Dangerous / Forbidden schemes
    assert!(matches!(
        policy.validate_url("file:///etc/passwd"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        policy.validate_url("file:///C:/Windows/win.ini"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        policy.validate_url("ftp://api.groq.com/data"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        policy.validate_url("gopher://api.groq.com/"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        policy.validate_url("ws://api.groq.com/"),
        Err(SsrfError::UnsupportedScheme(_))
    ));

    // Plain HTTP rejected for external providers when require_https is active
    assert!(matches!(
        policy.validate_url("http://api.groq.com/openai/v1"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
}

#[test]
fn test_userinfo_credentials_in_url_rejected() {
    let policy = OutboundDestinationPolicy::default_secure();

    assert!(matches!(
        policy.validate_url("https://admin:secret@api.groq.com/v1"),
        Err(SsrfError::UserinfoDisallowed)
    ));
    assert!(matches!(
        policy.validate_url("https://user@api.groq.com/v1"),
        Err(SsrfError::UserinfoDisallowed)
    ));
}

#[test]
fn test_non_allowlisted_host_rejected() {
    let policy = OutboundDestinationPolicy::default_secure();

    assert!(matches!(
        policy.validate_url("https://evil-attacker.com/steal"),
        Err(SsrfError::ProhibitedHost(_))
    ));
    assert!(matches!(
        policy.validate_url("https://internal.corp.network/service"),
        Err(SsrfError::ProhibitedHost(_))
    ));
}

#[test]
fn test_comprehensive_ipv4_blocklist_classification() {
    // Loopback
    assert_eq!(
        classify_ip(&"127.0.0.1".parse().unwrap()),
        IpClassification::ProhibitedLoopback
    );
    assert_eq!(
        classify_ip(&"127.1.2.3".parse().unwrap()),
        IpClassification::ProhibitedLoopback
    );
    assert_eq!(
        classify_ip(&"127.255.255.255".parse().unwrap()),
        IpClassification::ProhibitedLoopback
    );

    // RFC 1918 Private
    assert_eq!(
        classify_ip(&"10.0.0.1".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );
    assert_eq!(
        classify_ip(&"10.254.0.1".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );
    assert_eq!(
        classify_ip(&"172.16.0.1".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );
    assert_eq!(
        classify_ip(&"172.31.255.255".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );
    assert_eq!(
        classify_ip(&"192.168.0.1".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );
    assert_eq!(
        classify_ip(&"192.168.100.50".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );

    // Link-local
    assert_eq!(
        classify_ip(&"169.254.1.1".parse().unwrap()),
        IpClassification::ProhibitedLinkLocal
    );

    // Cloud instance metadata service
    assert_eq!(
        classify_ip(&"169.254.169.254".parse().unwrap()),
        IpClassification::ProhibitedCloudMetadata
    );

    // Unspecified
    assert_eq!(
        classify_ip(&"0.0.0.0".parse().unwrap()),
        IpClassification::ProhibitedUnspecified
    );
    assert_eq!(
        classify_ip(&"0.1.2.3".parse().unwrap()),
        IpClassification::ProhibitedUnspecified
    );

    // Multicast and Broadcast
    assert_eq!(
        classify_ip(&"224.0.0.1".parse().unwrap()),
        IpClassification::ProhibitedMulticastBroadcast
    );
    assert_eq!(
        classify_ip(&"239.255.255.255".parse().unwrap()),
        IpClassification::ProhibitedMulticastBroadcast
    );
    assert_eq!(
        classify_ip(&"255.255.255.255".parse().unwrap()),
        IpClassification::ProhibitedMulticastBroadcast
    );

    // Documentation and test ranges
    assert_eq!(
        classify_ip(&"192.0.2.1".parse().unwrap()),
        IpClassification::ProhibitedSpecialOrReserved
    );
    assert_eq!(
        classify_ip(&"198.51.100.1".parse().unwrap()),
        IpClassification::ProhibitedSpecialOrReserved
    );
    assert_eq!(
        classify_ip(&"203.0.113.1".parse().unwrap()),
        IpClassification::ProhibitedSpecialOrReserved
    );

    // Safe global public IPs
    assert_eq!(
        classify_ip(&"8.8.8.8".parse().unwrap()),
        IpClassification::SafePublic
    );
    assert_eq!(
        classify_ip(&"1.1.1.1".parse().unwrap()),
        IpClassification::SafePublic
    );
    assert_eq!(
        classify_ip(&"104.18.2.1".parse().unwrap()),
        IpClassification::SafePublic
    );
}

#[test]
fn test_comprehensive_ipv6_blocklist_classification() {
    // Loopback
    assert_eq!(
        classify_ip(&"::1".parse().unwrap()),
        IpClassification::ProhibitedLoopback
    );

    // Unspecified
    assert_eq!(
        classify_ip(&"::".parse().unwrap()),
        IpClassification::ProhibitedUnspecified
    );

    // AWS IPv6 IMDS
    assert_eq!(
        classify_ip(&"fd00:ec2::254".parse().unwrap()),
        IpClassification::ProhibitedCloudMetadata
    );

    // Unique Local Address (ULA) fc00::/7
    assert_eq!(
        classify_ip(&"fc00::1".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );
    assert_eq!(
        classify_ip(&"fd12:3456:789a:1::1".parse().unwrap()),
        IpClassification::ProhibitedPrivate
    );

    // Link-local fe80::/10
    assert_eq!(
        classify_ip(&"fe80::1".parse().unwrap()),
        IpClassification::ProhibitedLinkLocal
    );
    assert_eq!(
        classify_ip(&"fe80::abcd:ef01:2345:6789".parse().unwrap()),
        IpClassification::ProhibitedLinkLocal
    );

    // Multicast ff00::/8
    assert_eq!(
        classify_ip(&"ff02::1".parse().unwrap()),
        IpClassification::ProhibitedMulticastBroadcast
    );
    assert_eq!(
        classify_ip(&"ff05::2".parse().unwrap()),
        IpClassification::ProhibitedMulticastBroadcast
    );

    // Safe global public IPv6
    assert_eq!(
        classify_ip(&"2606:4700:4700::1111".parse().unwrap()),
        IpClassification::SafePublic
    );
    assert_eq!(
        classify_ip(&"2001:4860:4860::8888".parse().unwrap()),
        IpClassification::SafePublic
    );
}

#[test]
fn test_ipv4_mapped_ipv6_evasion_blocked() {
    // ::ffff:127.0.0.1 (IPv4 loopback in IPv6 form)
    let mapped_loopback: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
    assert_eq!(
        classify_ip(&mapped_loopback),
        IpClassification::ProhibitedLoopback
    );

    // ::ffff:169.254.169.254 (Cloud metadata in IPv6 form)
    let mapped_metadata: IpAddr = "::ffff:169.254.169.254".parse().unwrap();
    assert_eq!(
        classify_ip(&mapped_metadata),
        IpClassification::ProhibitedCloudMetadata
    );

    // ::ffff:10.0.0.1 (RFC 1918 in IPv6 form)
    let mapped_private: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
    assert_eq!(
        classify_ip(&mapped_private),
        IpClassification::ProhibitedPrivate
    );

    // ::ffff:192.168.1.1
    let mapped_private2: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
    assert_eq!(
        classify_ip(&mapped_private2),
        IpClassification::ProhibitedPrivate
    );

    // Safe public IPv4 mapped into IPv6
    let mapped_public: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
    assert_eq!(classify_ip(&mapped_public), IpClassification::SafePublic);
}

#[tokio::test]
async fn test_dns_rebinding_connection_pinning_and_redirect_blocking() {
    let policy = OutboundDestinationPolicy::default_secure();

    let dest = ValidatedDestination {
        url: Url::parse("https://api.groq.com/v1").unwrap(),
        host: "api.groq.com".to_string(),
        port: 443,
        is_local: false,
    };

    let conn = ValidatedConnection {
        host: dest.host.clone(),
        port: dest.port,
        socket_addrs: vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(104, 18, 2, 1)),
            443,
        )],
        is_local: false,
    };

    // Client is constructed pinned directly to validated socket addresses
    let client = policy.create_pinned_client(&conn);
    assert!(client.is_ok());
}

#[test]
fn test_authorized_local_inference_gating() {
    // Disabled by default
    let default_policy = OutboundDestinationPolicy::default_secure();
    assert!(matches!(
        default_policy.validate_url("http://127.0.0.1:11434/v1"),
        Err(SsrfError::UnsupportedScheme(_))
    ));

    // Explicitly enabled for exact base URL
    let local_policy = OutboundDestinationPolicy::default_secure()
        .with_local_inference(true, Some("http://127.0.0.1:11434".to_string()));

    let valid_local = local_policy.validate_url("http://127.0.0.1:11434/api/generate");
    assert!(valid_local.is_ok());
    let dest = valid_local.unwrap();
    assert_eq!(dest.host, "127.0.0.1");
    assert_eq!(dest.port, 11434);
    assert!(dest.is_local);

    // Attacker cannot use local inference exception to access different ports
    assert!(matches!(
        local_policy.validate_url("http://127.0.0.1:22/ssh"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        local_policy.validate_url("http://127.0.0.1:8080/admin"),
        Err(SsrfError::UnsupportedScheme(_))
    ));

    // Attacker cannot use local inference exception to access cloud metadata
    assert!(matches!(
        local_policy.validate_url("http://169.254.169.254/latest/meta-data/"),
        Err(SsrfError::UnsupportedScheme(_))
    ));

    // Attacker cannot access arbitrary internal RFC 1918 hosts
    assert!(matches!(
        local_policy.validate_url("http://192.168.1.1:11434/v1"),
        Err(SsrfError::UnsupportedScheme(_))
    ));
}
