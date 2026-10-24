//! Server-Side Request Forgery (SSRF) and Outbound Network Protection Subsystem.
//!
//! Conforms strictly to Phase 14 specifications in MASTER_SPEC.md §14,
//! ARCHITECTURE.md §2, and THREAT_MODEL.md.
//!
//! Protects against:
//! - Outbound requests to unintended, private, loopback, or cloud-metadata destinations.
//! - DNS rebinding via connection address pinning.
//! - Protocol smuggling, userinfo credential injection, and unverified schemes.
//! - Unvalidated redirect bypasses (disabled by default).
//! - IPv4-mapped IPv6 evasion attacks.

use crate::security::HostValidator;
use reqwest::Url;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Detailed classification of an IP address for outbound network safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpClassification {
    /// Public globally routable IP address suitable for outbound provider traffic.
    SafePublic,
    /// Loopback address (127.0.0.0/8, ::1).
    ProhibitedLoopback,
    /// RFC 1918 or IPv6 Unique Local private network address (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7).
    ProhibitedPrivate,
    /// Link-local network address (169.254.0.0/16, fe80::/10).
    ProhibitedLinkLocal,
    /// Cloud instance metadata service endpoint (169.254.169.254, [fd00:ec2::254]).
    ProhibitedCloudMetadata,
    /// Unspecified address (0.0.0.0/8, ::).
    ProhibitedUnspecified,
    /// Multicast or broadcast address (224.0.0.0/4, 255.255.255.255, ff00::/8).
    ProhibitedMulticastBroadcast,
    /// Reserved, benchmark, or documentation address (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32, 198.18.0.0/15, 100.64.0.0/10).
    ProhibitedSpecialOrReserved,
}

impl IpClassification {
    /// Returns true if the IP classification represents a safe public destination.
    pub fn is_safe(&self) -> bool {
        matches!(self, Self::SafePublic)
    }

    /// User-friendly, sanitized description of the classification.
    pub fn description(&self) -> &'static str {
        match self {
            Self::SafePublic => "Safe Public IP",
            Self::ProhibitedLoopback => "Prohibited Loopback Address",
            Self::ProhibitedPrivate => "Prohibited RFC 1918 Private Address",
            Self::ProhibitedLinkLocal => "Prohibited Link-Local Address",
            Self::ProhibitedCloudMetadata => "Prohibited Cloud Instance Metadata Service",
            Self::ProhibitedUnspecified => "Prohibited Unspecified Address",
            Self::ProhibitedMulticastBroadcast => "Prohibited Multicast or Broadcast Address",
            Self::ProhibitedSpecialOrReserved => "Prohibited Reserved or Documentation Address",
        }
    }
}

/// Classify an IP address, safely extracting and inspecting IPv4-mapped IPv6 representations.
pub fn classify_ip(ip: &IpAddr) -> IpClassification {
    let effective = match ip {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                IpAddr::V4(v4)
            } else {
                let segs = v6.segments();
                // Check ::ffff:a.b.c.d
                if segs[0] == 0
                    && segs[1] == 0
                    && segs[2] == 0
                    && segs[3] == 0
                    && segs[4] == 0
                    && segs[5] == 0xffff
                {
                    IpAddr::V4(Ipv4Addr::new(
                        (segs[6] >> 8) as u8,
                        (segs[6] & 0xff) as u8,
                        (segs[7] >> 8) as u8,
                        (segs[7] & 0xff) as u8,
                    ))
                } else if segs[0] == 0
                    && segs[1] == 0
                    && segs[2] == 0
                    && segs[3] == 0
                    && segs[4] == 0
                    && segs[5] == 0
                    && !v6.is_loopback()
                    && !v6.is_unspecified()
                    && !(segs[6] == 0 && (segs[7] == 0 || segs[7] == 1))
                {
                    // Deprecated IPv4-compatible IPv6 (::a.b.c.d)
                    IpAddr::V4(Ipv4Addr::new(
                        (segs[6] >> 8) as u8,
                        (segs[6] & 0xff) as u8,
                        (segs[7] >> 8) as u8,
                        (segs[7] & 0xff) as u8,
                    ))
                } else {
                    IpAddr::V6(*v6)
                }
            }
        }
        IpAddr::V4(v4) => IpAddr::V4(*v4),
    };

    match effective {
        IpAddr::V4(v4) => classify_ipv4(v4),
        IpAddr::V6(v6) => classify_ipv6(v6),
    }
}

fn classify_ipv4(ip: Ipv4Addr) -> IpClassification {
    let octets = ip.octets();
    let num = u32::from(ip);

    // Cloud metadata: 169.254.169.254
    if octets == [169, 254, 169, 254] {
        return IpClassification::ProhibitedCloudMetadata;
    }

    // Link-local: 169.254.0.0/16
    if octets[0] == 169 && octets[1] == 254 {
        return IpClassification::ProhibitedLinkLocal;
    }

    // Loopback: 127.0.0.0/8
    if octets[0] == 127 {
        return IpClassification::ProhibitedLoopback;
    }

    // Unspecified / Current network: 0.0.0.0/8
    if octets[0] == 0 {
        return IpClassification::ProhibitedUnspecified;
    }

    // RFC 1918 Private:
    // 10.0.0.0/8
    if octets[0] == 10 {
        return IpClassification::ProhibitedPrivate;
    }
    // 172.16.0.0/12 (172.16.0.0 - 172.31.255.255)
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return IpClassification::ProhibitedPrivate;
    }
    // 192.168.0.0/16
    if octets[0] == 192 && octets[1] == 168 {
        return IpClassification::ProhibitedPrivate;
    }

    // Carrier-grade NAT: 100.64.0.0/10 (100.64.0.0 - 100.127.255.255)
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return IpClassification::ProhibitedSpecialOrReserved;
    }

    // IETF Protocol: 192.0.0.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return IpClassification::ProhibitedSpecialOrReserved;
    }

    // Documentation:
    // TEST-NET-1: 192.0.2.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return IpClassification::ProhibitedSpecialOrReserved;
    }
    // TEST-NET-2: 198.51.100.0/24
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return IpClassification::ProhibitedSpecialOrReserved;
    }
    // TEST-NET-3: 203.0.113.0/24
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return IpClassification::ProhibitedSpecialOrReserved;
    }

    // Benchmarking: 198.18.0.0/15 (198.18.0.0 - 198.19.255.255)
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return IpClassification::ProhibitedSpecialOrReserved;
    }

    // Multicast: 224.0.0.0/4 (224.0.0.0 - 239.255.255.255)
    if (224..=239).contains(&octets[0]) {
        return IpClassification::ProhibitedMulticastBroadcast;
    }

    // Reserved / Future use: 240.0.0.0/4 (includes broadcast 255.255.255.255)
    if octets[0] >= 240 {
        return IpClassification::ProhibitedMulticastBroadcast;
    }

    // Directed broadcast check
    if num == 0xFFFFFFFF {
        return IpClassification::ProhibitedMulticastBroadcast;
    }

    IpClassification::SafePublic
}

fn classify_ipv6(ip: Ipv6Addr) -> IpClassification {
    let segs = ip.segments();

    // Loopback ::1
    if ip.is_loopback() || segs == [0, 0, 0, 0, 0, 0, 0, 1] {
        return IpClassification::ProhibitedLoopback;
    }

    // Unspecified ::
    if ip.is_unspecified() || segs == [0, 0, 0, 0, 0, 0, 0, 0] {
        return IpClassification::ProhibitedUnspecified;
    }

    // AWS IPv6 IMDS: [fd00:ec2::254]
    if segs[0] == 0xfd00
        && segs[1] == 0xec2
        && segs[2] == 0
        && segs[3] == 0
        && segs[4] == 0
        && segs[5] == 0
        && segs[6] == 0
        && segs[7] == 0x254
    {
        return IpClassification::ProhibitedCloudMetadata;
    }

    // Unique Local Address (ULA): fc00::/7 (fc00... or fd00...)
    if (segs[0] & 0xfe00) == 0xfc00 {
        return IpClassification::ProhibitedPrivate;
    }

    // Link-Local: fe80::/10 (fe80... to febf...)
    if (segs[0] & 0xffc0) == 0xfe80 {
        return IpClassification::ProhibitedLinkLocal;
    }

    // Multicast: ff00::/8
    if (segs[0] & 0xff00) == 0xff00 {
        return IpClassification::ProhibitedMulticastBroadcast;
    }

    // Documentation: 2001:db8::/32
    if segs[0] == 0x2001 && segs[1] == 0x0db8 {
        return IpClassification::ProhibitedSpecialOrReserved;
    }

    // Discard-only: 100::/64
    if segs[0] == 0x0100 && segs[1] == 0 {
        return IpClassification::ProhibitedSpecialOrReserved;
    }

    IpClassification::SafePublic
}

/// SSRF policy rejection errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsrfError {
    /// Provided destination URL string is unparsable.
    InvalidUrl(String),
    /// URL scheme is forbidden (e.g. `http://` for external, `file://`, `ftp://`).
    UnsupportedScheme(String),
    /// URL contains unexpected username or password credentials.
    UserinfoDisallowed,
    /// Destination hostname is missing or not allowed by policy.
    ProhibitedHost(String),
    /// Port is invalid or forbidden by policy.
    ProhibitedPort(u16),
    /// DNS lookup yielded no valid address records.
    DnsLookupEmpty(String),
    /// DNS lookup returned an address that is prohibited by SSRF policy.
    ProhibitedAddress {
        host: String,
        ip: IpAddr,
        classification: IpClassification,
    },
    /// DNS resolution failed with network or OS error.
    DnsResolutionFailed(String),
    /// Internal client building or transport error.
    ClientBuildFailed(String),
}

impl fmt::Display for SsrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(msg) => write!(f, "Invalid destination URL: {msg}"),
            Self::UnsupportedScheme(scheme) => write!(f, "Unsupported URL scheme '{scheme}'"),
            Self::UserinfoDisallowed => write!(f, "Embedded URL credentials are not permitted"),
            Self::ProhibitedHost(host) => write!(f, "Destination host '{host}' is not allowlisted"),
            Self::ProhibitedPort(port) => write!(f, "Destination port {port} is not allowed"),
            Self::DnsLookupEmpty(host) => {
                write!(f, "DNS lookup returned no addresses for '{host}'")
            }
            Self::ProhibitedAddress {
                host,
                ip,
                classification,
            } => {
                write!(
                    f,
                    "Destination host '{host}' resolved to prohibited address {ip} ({})",
                    classification.description()
                )
            }
            Self::DnsResolutionFailed(msg) => write!(f, "DNS resolution failed: {msg}"),
            Self::ClientBuildFailed(msg) => write!(f, "HTTP client configuration failed: {msg}"),
        }
    }
}

impl std::error::Error for SsrfError {}

/// Fully validated destination metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDestination {
    pub url: Url,
    pub host: String,
    pub port: u16,
    pub is_local: bool,
}

/// A validated outbound connection with resolved and pinned socket addresses.
#[derive(Debug, Clone)]
pub struct ValidatedConnection {
    pub host: String,
    pub port: u16,
    pub socket_addrs: Vec<SocketAddr>,
    pub is_local: bool,
}

/// Outbound destination security policy enforcing allowlists and SSRF defenses.
#[derive(Debug, Clone)]
pub struct OutboundDestinationPolicy {
    /// Whether SSRF protections are actively enforced.
    pub enabled: bool,
    /// Explicitly allowlisted hostnames and wildcard subdomains (e.g. `*.googleapis.com`).
    pub allowed_hosts: Vec<String>,
    /// Whether explicitly configured local inference is permitted.
    pub allow_local_inference: bool,
    /// Explicit base URL for authorized local inference (e.g. `http://127.0.0.1:11434`).
    pub local_inference_base_url: Option<String>,
    /// Explicit authorized local gateway URLs (e.g. `http://127.0.0.1:3001/v1`, `http://127.0.0.1:20128/v1`).
    pub authorized_local_urls: Vec<String>,
    /// Whether external destinations strictly require HTTPS (default: true).
    pub require_https: bool,
    /// Maximum allowed HTTP redirects (default: 0, redirects disabled).
    pub max_redirects: usize,
    /// Connect timeout for outbound requests (default: 5 seconds).
    pub connect_timeout: Duration,
    /// Overall request timeout for outbound requests (default: 10 seconds).
    pub request_timeout: Duration,
    host_validator: HostValidator,
}

impl OutboundDestinationPolicy {
    /// Default secure destination policy permitting standard providers and Google JWKS.
    pub fn default_secure() -> Self {
        let allowed = vec![
            "api.groq.com".to_string(),
            "openrouter.ai".to_string(),
            "api.openai.com".to_string(),
            "www.googleapis.com".to_string(),
            "accounts.google.com".to_string(),
        ];
        let host_validator = HostValidator::new(&allowed);
        Self {
            enabled: true,
            allowed_hosts: allowed,
            allow_local_inference: false,
            local_inference_base_url: None,
            authorized_local_urls: Vec::new(),
            require_https: true,
            max_redirects: 0,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
            host_validator,
        }
    }

    /// Builder to configure policy with custom allowed host list.
    pub fn with_allowed_hosts(mut self, hosts: Vec<String>) -> Self {
        self.host_validator = HostValidator::new(&hosts);
        self.allowed_hosts = hosts;
        self
    }

    /// Builder to configure authorized local inference.
    pub fn with_local_inference(mut self, enabled: bool, base_url: Option<String>) -> Self {
        self.allow_local_inference = enabled;
        self.local_inference_base_url = base_url;
        self
    }

    /// Builder to configure authorized local gateway URLs.
    pub fn with_authorized_local_urls(mut self, urls: Vec<String>) -> Self {
        self.authorized_local_urls = urls;
        self
    }

    /// Builder to add an authorized local gateway URL.
    pub fn with_authorized_local_url(mut self, url: impl Into<String>) -> Self {
        self.authorized_local_urls.push(url.into());
        self
    }

    /// Validate the structure, scheme, credentials, and hostname of a candidate destination URL.
    pub fn validate_url(&self, raw_url: &str) -> Result<ValidatedDestination, SsrfError> {
        let url = Url::parse(raw_url).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

        // 1. Initial scheme validation
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(SsrfError::UnsupportedScheme(scheme.to_string()));
        }

        // 2. Prohibit userinfo / credentials in URLs
        if !url.username().is_empty() || url.password().is_some() {
            return Err(SsrfError::UserinfoDisallowed);
        }

        let host = url
            .host_str()
            .ok_or_else(|| SsrfError::InvalidUrl("Missing host component".to_string()))?
            .to_ascii_lowercase();

        let port = url.port_or_known_default().unwrap_or(match scheme {
            "https" => 443,
            "http" => 80,
            _ => 0,
        });

        // 3. Check for authorized local inference / gateway destination
        let matches_base = |candidate: &str| -> bool {
            if let Ok(parsed) = Url::parse(candidate) {
                let p_host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
                let p_port = parsed
                    .port_or_known_default()
                    .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
                host == p_host && port == p_port
            } else {
                false
            }
        };

        let is_authorized_local = if self.allow_local_inference
            && self
                .local_inference_base_url
                .as_deref()
                .map(matches_base)
                .unwrap_or(false)
        {
            true
        } else {
            self.authorized_local_urls.iter().any(|u| matches_base(u))
        };

        // 4. Scheme validation for external destinations
        if !is_authorized_local && self.require_https && scheme != "https" {
            return Err(SsrfError::UnsupportedScheme(scheme.to_string()));
        }

        // 5. Host allowlist check (if not authorized local)
        if !is_authorized_local && self.enabled && !self.host_validator.validate_host(&host) {
            return Err(SsrfError::ProhibitedHost(host));
        }

        Ok(ValidatedDestination {
            url,
            host,
            port,
            is_local: is_authorized_local,
        })
    }

    /// Asynchronously resolve and validate DNS records, pinning the connection to validated IPs.
    pub async fn resolve_and_pin(
        &self,
        destination: &ValidatedDestination,
    ) -> Result<ValidatedConnection, SsrfError> {
        let host_port = format!("{}:{}", destination.host, destination.port);

        // Perform asynchronous DNS resolution
        let socket_addrs: Vec<SocketAddr> = tokio::net::lookup_host(&host_port)
            .await
            .map_err(|e| SsrfError::DnsResolutionFailed(e.to_string()))?
            .collect();

        if socket_addrs.is_empty() {
            return Err(SsrfError::DnsLookupEmpty(destination.host.clone()));
        }

        // Validate every resolved IP address
        if !destination.is_local && self.enabled {
            for addr in &socket_addrs {
                let classification = classify_ip(&addr.ip());
                if !classification.is_safe() {
                    return Err(SsrfError::ProhibitedAddress {
                        host: destination.host.clone(),
                        ip: addr.ip(),
                        classification,
                    });
                }
            }
        }

        Ok(ValidatedConnection {
            host: destination.host.clone(),
            port: destination.port,
            socket_addrs,
            is_local: destination.is_local,
        })
    }

    /// Create a hardened, SSRF-safe `reqwest::Client` pinned to the validated socket addresses.
    ///
    /// Prevents DNS rebinding TOCTOU attacks by resolving strictly through the pinned addresses.
    /// Disables redirects to prevent redirect-based SSRF bypass.
    pub fn create_pinned_client(
        &self,
        connection: &ValidatedConnection,
    ) -> Result<reqwest::Client, SsrfError> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .redirect(reqwest::redirect::Policy::none());

        // Pin the domain to the validated addresses to prevent DNS rebinding
        if !connection.socket_addrs.is_empty() {
            builder = builder.resolve_to_addrs(&connection.host, &connection.socket_addrs);
        }

        builder
            .build()
            .map_err(|e| SsrfError::ClientBuildFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_classification_boundaries() {
        assert_eq!(
            classify_ip(&"127.0.0.1".parse().unwrap()),
            IpClassification::ProhibitedLoopback
        );
        assert_eq!(
            classify_ip(&"127.255.255.255".parse().unwrap()),
            IpClassification::ProhibitedLoopback
        );
        assert_eq!(
            classify_ip(&"10.0.0.1".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        assert_eq!(
            classify_ip(&"172.16.0.1".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        assert_eq!(
            classify_ip(&"172.31.255.254".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        assert_eq!(
            classify_ip(&"192.168.1.1".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        assert_eq!(
            classify_ip(&"169.254.1.1".parse().unwrap()),
            IpClassification::ProhibitedLinkLocal
        );
        assert_eq!(
            classify_ip(&"169.254.169.254".parse().unwrap()),
            IpClassification::ProhibitedCloudMetadata
        );
        assert_eq!(
            classify_ip(&"0.0.0.0".parse().unwrap()),
            IpClassification::ProhibitedUnspecified
        );
        assert_eq!(
            classify_ip(&"224.0.0.1".parse().unwrap()),
            IpClassification::ProhibitedMulticastBroadcast
        );
        assert_eq!(
            classify_ip(&"255.255.255.255".parse().unwrap()),
            IpClassification::ProhibitedMulticastBroadcast
        );
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
        assert_eq!(
            classify_ip(&"8.8.8.8".parse().unwrap()),
            IpClassification::SafePublic
        );
        assert_eq!(
            classify_ip(&"1.1.1.1".parse().unwrap()),
            IpClassification::SafePublic
        );
    }

    #[test]
    fn test_ipv6_classification_boundaries() {
        assert_eq!(
            classify_ip(&"::1".parse().unwrap()),
            IpClassification::ProhibitedLoopback
        );
        assert_eq!(
            classify_ip(&"::".parse().unwrap()),
            IpClassification::ProhibitedUnspecified
        );
        assert_eq!(
            classify_ip(&"fe80::1".parse().unwrap()),
            IpClassification::ProhibitedLinkLocal
        );
        assert_eq!(
            classify_ip(&"fc00::1".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        assert_eq!(
            classify_ip(&"fd12:3456:789a::1".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        assert_eq!(
            classify_ip(&"fd00:ec2::254".parse().unwrap()),
            IpClassification::ProhibitedCloudMetadata
        );
        assert_eq!(
            classify_ip(&"ff02::1".parse().unwrap()),
            IpClassification::ProhibitedMulticastBroadcast
        );
        assert_eq!(
            classify_ip(&"2001:db8::1".parse().unwrap()),
            IpClassification::ProhibitedSpecialOrReserved
        );
        assert_eq!(
            classify_ip(&"2606:4700:4700::1111".parse().unwrap()),
            IpClassification::SafePublic
        );
    }

    #[test]
    fn test_ipv4_mapped_ipv6_classification() {
        // ::ffff:127.0.0.1 should be classified as loopback
        assert_eq!(
            classify_ip(&"::ffff:127.0.0.1".parse().unwrap()),
            IpClassification::ProhibitedLoopback
        );
        // ::ffff:169.254.169.254 should be classified as metadata
        assert_eq!(
            classify_ip(&"::ffff:169.254.169.254".parse().unwrap()),
            IpClassification::ProhibitedCloudMetadata
        );
        // ::ffff:192.168.1.1 should be classified as private
        assert_eq!(
            classify_ip(&"::ffff:192.168.1.1".parse().unwrap()),
            IpClassification::ProhibitedPrivate
        );
        // ::ffff:8.8.8.8 should be safe public
        assert_eq!(
            classify_ip(&"::ffff:8.8.8.8".parse().unwrap()),
            IpClassification::SafePublic
        );
    }

    #[test]
    fn test_url_validation_rules() {
        let policy = OutboundDestinationPolicy::default_secure();

        // Valid allowlisted HTTPS
        assert!(
            policy
                .validate_url("https://api.groq.com/openai/v1")
                .is_ok()
        );
        assert!(policy.validate_url("https://openrouter.ai/api/v1").is_ok());
        assert!(
            policy
                .validate_url("https://www.googleapis.com/oauth2/v3/certs")
                .is_ok()
        );

        // Insecure scheme rejected
        assert!(matches!(
            policy.validate_url("http://api.groq.com/openai/v1"),
            Err(SsrfError::UnsupportedScheme(_))
        ));

        // Prohibited schemes rejected
        assert!(matches!(
            policy.validate_url("file:///etc/passwd"),
            Err(SsrfError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            policy.validate_url("gopher://127.0.0.1:70/"),
            Err(SsrfError::UnsupportedScheme(_))
        ));

        // Embedded credentials rejected
        assert!(matches!(
            policy.validate_url("https://user:pass@api.groq.com/"),
            Err(SsrfError::UserinfoDisallowed)
        ));

        // Unallowlisted host rejected
        assert!(matches!(
            policy.validate_url("https://evil.attacker.com/"),
            Err(SsrfError::ProhibitedHost(_))
        ));
        assert!(matches!(
            policy.validate_url("https://169.254.169.254/"),
            Err(SsrfError::ProhibitedHost(_))
        ));
    }

    #[test]
    fn test_authorized_local_inference_gating() {
        let policy = OutboundDestinationPolicy::default_secure()
            .with_local_inference(true, Some("http://127.0.0.1:11434".to_string()));

        // Authorized local endpoint is allowed
        let dest = policy
            .validate_url("http://127.0.0.1:11434/api/generate")
            .unwrap();
        assert!(dest.is_local);

        // Unauthorized local port is blocked
        assert!(policy.validate_url("http://127.0.0.1:22/").is_err());
        // Cloud metadata is still blocked
        assert!(policy.validate_url("http://169.254.169.254/").is_err());
    }
}
