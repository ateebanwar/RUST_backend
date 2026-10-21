//! Security module: CIDR matching, trusted proxy verification, and host header validation.
//!
//! Conforms to MASTER_SPEC.md §13 & §14, ARCHITECTURE.md §2, and THREAT_MODEL.md.
pub mod encryption;
pub mod rate_limiter;
pub mod secret;
pub mod ssrf;

pub use encryption::{AesGcmCipher, EncryptionError};
pub use rate_limiter::{
    RateLimitKey, RateLimitRejection, RateLimitRouteCategory, RateLimiter, RateLimiterConfig,
    RequestPermit, StreamPermit, TokenBucket,
};
pub use secret::{SecretString, mask_credential};
pub use ssrf::{
    IpClassification, OutboundDestinationPolicy, SsrfError, ValidatedConnection,
    ValidatedDestination, classify_ip,
};

use std::net::IpAddr;

/// An IPv4 or IPv6 CIDR representation for IP allowlisting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpCidr {
    V4 {
        network: u32,
        mask: u32,
        prefix: u8,
    },
    V6 {
        network: u128,
        mask: u128,
        prefix: u8,
    },
}

impl IpCidr {
    /// Parse an exact IP or CIDR notation (e.g. "192.168.1.0/24" or "10.0.0.1" or "::1/128").
    pub fn parse(s: &str) -> Result<Self, String> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("Empty CIDR string".to_string());
        }

        if let Some((ip_str, prefix_str)) = trimmed.split_once('/') {
            let prefix: u8 = prefix_str
                .parse()
                .map_err(|_| format!("Invalid CIDR prefix '{}' in '{}'", prefix_str, trimmed))?;
            let ip: IpAddr = ip_str
                .parse()
                .map_err(|_| format!("Invalid IP address '{}' in CIDR '{}'", ip_str, trimmed))?;

            match ip {
                IpAddr::V4(v4) => {
                    if prefix > 32 {
                        return Err(format!(
                            "IPv4 prefix must be 0-32, got {prefix} in '{trimmed}'"
                        ));
                    }
                    let mask = if prefix == 0 {
                        0u32
                    } else {
                        !((1u32 << (32 - prefix)) - 1)
                    };
                    let network = u32::from(v4) & mask;
                    Ok(Self::V4 {
                        network,
                        mask,
                        prefix,
                    })
                }
                IpAddr::V6(v6) => {
                    if prefix > 128 {
                        return Err(format!(
                            "IPv6 prefix must be 0-128, got {prefix} in '{trimmed}'"
                        ));
                    }
                    let mask = if prefix == 0 {
                        0u128
                    } else {
                        !((1u128 << (128 - prefix)) - 1)
                    };
                    let network = u128::from(v6) & mask;
                    Ok(Self::V6 {
                        network,
                        mask,
                        prefix,
                    })
                }
            }
        } else {
            // Single IP address (implicit /32 or /128)
            let ip: IpAddr = trimmed
                .parse()
                .map_err(|_| format!("Invalid IP address '{}'", trimmed))?;
            match ip {
                IpAddr::V4(v4) => Ok(Self::V4 {
                    network: u32::from(v4),
                    mask: u32::MAX,
                    prefix: 32,
                }),
                IpAddr::V6(v6) => Ok(Self::V6 {
                    network: u128::from(v6),
                    mask: u128::MAX,
                    prefix: 128,
                }),
            }
        }
    }

    /// Check if a given IP address falls within this CIDR network.
    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (self, ip) {
            (Self::V4 { network, mask, .. }, IpAddr::V4(v4)) => (u32::from(*v4) & mask) == *network,
            (Self::V6 { network, mask, .. }, IpAddr::V6(v6)) => {
                (u128::from(*v6) & mask) == *network
            }
            _ => false,
        }
    }
}

/// Evaluates peer connections against configured trusted reverse-proxy CIDRs.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxyMatcher {
    cidrs: Vec<IpCidr>,
}

impl TrustedProxyMatcher {
    /// Initialize with a slice of CIDR / IP strings.
    pub fn new(cidr_strings: &[String]) -> Result<Self, String> {
        let mut cidrs = Vec::with_capacity(cidr_strings.len());
        for s in cidr_strings {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                cidrs.push(IpCidr::parse(trimmed)?);
            }
        }
        Ok(Self { cidrs })
    }

    /// Returns true if the peer IP address is within any trusted proxy CIDR.
    pub fn is_trusted(&self, ip: &IpAddr) -> bool {
        self.cidrs.iter().any(|c| c.contains(ip))
    }

    /// Returns the number of configured CIDR rules.
    pub fn len(&self) -> usize {
        self.cidrs.len()
    }

    /// Returns true if no proxy CIDR rules are configured.
    pub fn is_empty(&self) -> bool {
        self.cidrs.is_empty()
    }
}

/// Validates incoming HTTP `Host` headers against an explicit allowlist.
#[derive(Debug, Clone, Default)]
pub struct HostValidator {
    allowed_hosts: Vec<String>,
}

impl HostValidator {
    /// Create validator from allowed hostnames/IPs.
    pub fn new(hosts: &[String]) -> Self {
        Self {
            allowed_hosts: hosts
                .iter()
                .map(|h| {
                    let s = h.trim().to_ascii_lowercase();
                    if s.starts_with('[') && s.ends_with(']') && s.len() >= 2 {
                        s[1..s.len() - 1].to_string()
                    } else {
                        s
                    }
                })
                .filter(|h| !h.is_empty())
                .collect(),
        }
    }

    /// Returns true if validation passes.
    /// If no explicit allowlist is configured, validation passes by default.
    pub fn validate_host(&self, host_header: &str) -> bool {
        if self.allowed_hosts.is_empty() {
            return true;
        }

        // Reject header values with forbidden characters on raw string before trimming
        // Prevents CRLF injection, smuggling, path/query injection, and whitespace bypasses
        if host_header.contains(',')
            || host_header.contains(' ')
            || host_header.contains('\t')
            || host_header.contains('\r')
            || host_header.contains('\n')
            || host_header.contains('@')
            || host_header.contains('/')
            || host_header.contains('\\')
            || host_header.contains('?')
            || host_header.contains('#')
        {
            return false;
        }

        let trimmed = host_header.trim();
        if trimmed.is_empty() {
            return false;
        }

        // Strip port if present: e.g. "api.example.com:8080" -> "api.example.com"
        // Also handle IPv6 brackets: "[::1]:8080" -> "::1"
        let host_part = if let Some(stripped) = trimmed.strip_prefix('[') {
            if let Some((ip6, rest)) = stripped.split_once(']') {
                if !rest.is_empty() && !rest.starts_with(':') {
                    return false; // Malformed IPv6 host like [::1]something
                }
                ip6
            } else {
                return false; // Unclosed bracket: malformed
            }
        } else if let Some((h, _port)) = trimmed.split_once(':') {
            h
        } else {
            trimmed
        };

        if host_part.is_empty() {
            return false;
        }

        let lower = host_part.to_ascii_lowercase();
        let bracketed = format!("[{lower}]");
        self.allowed_hosts.iter().any(|allowed| {
            if let Some(wildcard_suffix) = allowed.strip_prefix("*.") {
                // Wildcard subdomain matching: e.g. "*.example.com"
                // Matches "sub.example.com" or "a.b.example.com"
                // Rejects lookalikes like "evil-example.com" or "notexample.com"
                lower == wildcard_suffix || lower.ends_with(&format!(".{wildcard_suffix}"))
            } else {
                allowed == &lower || allowed == &bracketed || allowed == trimmed
            }
        })
    }
}

/// Safely resolve the origin client IP address considering reverse proxy trust boundaries.
///
/// If `peer_ip` is not within `trusted_proxies`, all forwarded headers are rejected as untrusted,
/// returning the direct `peer_ip`.
///
/// If `peer_ip` is trusted, parses `X-Forwarded-For` from right to left, returning the rightmost
/// IP that is NOT a trusted proxy (the true external client).
pub fn resolve_client_ip(
    peer_ip: Option<IpAddr>,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &TrustedProxyMatcher,
) -> Option<IpAddr> {
    let peer = peer_ip?;

    if !trusted_proxies.is_trusted(&peer) {
        return Some(peer);
    }

    // Peer is a verified trusted proxy: inspect X-Forwarded-For
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()) {
        let ips: Vec<IpAddr> = xff
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .collect();

        if ips.is_empty() {
            return Some(peer);
        }

        // Walk backwards from rightmost (closest to our proxy) to leftmost
        // Find the first untrusted IP
        for ip in ips.iter().rev() {
            if !trusted_proxies.is_trusted(ip) {
                return Some(*ip);
            }
        }

        // If all IPs in chain are trusted proxies, the origin is the leftmost proxy
        return Some(ips[0]);
    }

    Some(peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cidr_v4_parsing_and_matching() {
        let cidr = IpCidr::parse("192.168.1.0/24").unwrap();
        assert!(cidr.contains(&"192.168.1.1".parse().unwrap()));
        assert!(cidr.contains(&"192.168.1.254".parse().unwrap()));
        assert!(!cidr.contains(&"192.168.2.1".parse().unwrap()));
        assert!(!cidr.contains(&"10.0.0.1".parse().unwrap()));

        let single_ip = IpCidr::parse("127.0.0.1").unwrap();
        assert!(single_ip.contains(&"127.0.0.1".parse().unwrap()));
        assert!(!single_ip.contains(&"127.0.0.2".parse().unwrap()));
    }

    #[test]
    fn test_cidr_v6_parsing_and_matching() {
        let cidr = IpCidr::parse("::1/128").unwrap();
        assert!(cidr.contains(&"::1".parse().unwrap()));
        assert!(!cidr.contains(&"::2".parse().unwrap()));

        let fd00 = IpCidr::parse("fd00::/8").unwrap();
        assert!(fd00.contains(&"fd00::1".parse().unwrap()));
        assert!(fd00.contains(&"fdff:ffff::1".parse().unwrap()));
        assert!(!fd00.contains(&"2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn test_invalid_cidr_strings_rejected() {
        assert!(IpCidr::parse("").is_err());
        assert!(IpCidr::parse("invalid-ip").is_err());
        assert!(IpCidr::parse("192.168.1.1/33").is_err());
        assert!(IpCidr::parse("::1/129").is_err());
        assert!(IpCidr::parse("192.168.1.1/abc").is_err());
    }

    #[test]
    fn test_trusted_proxy_matcher() {
        let matcher = TrustedProxyMatcher::new(&[
            "127.0.0.1".to_string(),
            "::1".to_string(),
            "10.0.0.0/8".to_string(),
        ])
        .unwrap();

        assert!(matcher.is_trusted(&"127.0.0.1".parse().unwrap()));
        assert!(matcher.is_trusted(&"::1".parse().unwrap()));
        assert!(matcher.is_trusted(&"10.1.2.3".parse().unwrap()));
        assert!(!matcher.is_trusted(&"192.168.1.1".parse().unwrap()));
        assert!(!matcher.is_trusted(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_host_validator() {
        let validator = HostValidator::new(&[
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "api.example.com".to_string(),
        ]);

        assert!(validator.validate_host("localhost"));
        assert!(validator.validate_host("localhost:8080"));
        assert!(validator.validate_host("127.0.0.1:8080"));
        assert!(validator.validate_host("api.example.com"));
        assert!(validator.validate_host("API.EXAMPLE.COM:443"));
        assert!(!validator.validate_host("evil.com"));
        assert!(!validator.validate_host("evil.com:8080"));
        assert!(!validator.validate_host(""));
    }
}
