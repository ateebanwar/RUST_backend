//! Security module: CIDR matching, trusted proxy verification, and host header validation.
//!
//! Conforms to MASTER_SPEC.md §13 & §14, ARCHITECTURE.md §2, and THREAT_MODEL.md.

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
                .map(|h| h.trim().to_ascii_lowercase())
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

        let trimmed = host_header.trim();
        if trimmed.is_empty() {
            return false;
        }

        // Strip port if present: e.g. "api.example.com:8080" -> "api.example.com"
        // Also handle IPv6 brackets: "[::1]:8080" -> "::1"
        let host_part = if let Some(stripped) = trimmed.strip_prefix('[') {
            if let Some((ip6, _)) = stripped.split_once(']') {
                ip6
            } else {
                trimmed
            }
        } else if let Some((h, _port)) = trimmed.split_once(':') {
            h
        } else {
            trimmed
        };

        let lower = host_part.to_ascii_lowercase();
        self.allowed_hosts
            .iter()
            .any(|allowed| allowed == &lower || allowed == trimmed)
    }
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
