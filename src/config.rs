//! Application configuration loading, parsing, and validation.
//!
//! Provides typed configuration structures with strict validation, sensible
//! local-development defaults, and zero-leak debug formatting for secrets.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

/// Supported deployment environments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Production,
    Test,
}

impl Environment {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
            Self::Test => "test",
        }
    }
}

impl FromStr for Environment {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "development" | "dev" => Ok(Self::Development),
            "production" | "prod" => Ok(Self::Production),
            "test" => Ok(Self::Test),
            unknown => Err(ConfigError::InvalidValue {
                key: "APP_ENV",
                value: unknown.to_string(),
                reason: "Expected 'development', 'production', or 'test'",
            }),
        }
    }
}

/// Target deployment hosting mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentMode {
    Daemon,
    Serverless,
}

impl DeploymentMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Serverless => "serverless",
        }
    }
}

impl FromStr for DeploymentMode {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "daemon" | "standalone" => Ok(Self::Daemon),
            "serverless" | "vercel" | "lambda" => Ok(Self::Serverless),
            unknown => Err(ConfigError::InvalidValue {
                key: "DEPLOYMENT_MODE",
                value: unknown.to_string(),
                reason: "Expected 'daemon' or 'serverless'",
            }),
        }
    }
}

/// Strongly-typed application configuration.
#[derive(Clone)]
pub struct AppConfig {
    /// Host interface to bind to (e.g. "127.0.0.1" or "0.0.0.0").
    pub host: String,
    /// TCP port to listen on (e.g. 8080).
    pub port: u16,
    /// Active runtime environment.
    pub environment: Environment,
    /// Hosting deployment mode.
    pub deployment_mode: DeploymentMode,
    /// Maximum allowed request body size in bytes (default 4.5 MB).
    pub max_body_size_bytes: usize,
    /// Upstream request timeout ceiling in seconds (default 10s).
    pub upstream_timeout_secs: u64,
    /// Upstream Groq Cloud API key (loaded from GROQ_API_KEY).
    pub groq_api_key: Option<String>,
    /// Upstream Groq Cloud model override (loaded from GROQ_MODEL).
    pub groq_model: Option<String>,
    /// Upstream OpenRouter API key (loaded from OPENROUTER_API_KEY).
    pub openrouter_api_key: Option<String>,
    /// Upstream Google Gemini API key (loaded from GEMINI_API_KEY).
    /// Note: Google Gemini adapter integration is planned for a future multimodal phase;
    /// this configuration field is loaded and isolated, but currently marked unsupported in v1 router.
    pub gemini_api_key: Option<String>,
    /// Backend API key for authenticating protected routes (loaded from API_KEY).
    pub api_key: Option<String>,
}

impl AppConfig {
    /// Default local development configuration.
    pub fn default_local() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            environment: Environment::Development,
            deployment_mode: DeploymentMode::Daemon,
            max_body_size_bytes: 4_500_000, // 4.5 MB per API_SPEC.md §1.2
            upstream_timeout_secs: 10,      // 10s per DECISIONS.md DEC-002
            groq_api_key: None,
            groq_model: None,
            openrouter_api_key: None,
            gemini_api_key: None,
            api_key: None,
        }
    }

    /// Load configuration from environment variables with fallback to local defaults.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self::default_local();

        if let Ok(host_val) = std::env::var("HOST") {
            let host_trimmed = host_val.trim();
            if host_trimmed.is_empty() {
                return Err(ConfigError::InvalidValue {
                    key: "HOST",
                    value: host_val,
                    reason: "HOST cannot be empty",
                });
            }
            config.host = host_trimmed.to_string();
        }

        if let Ok(port_val) = std::env::var("PORT") {
            let parsed_port =
                port_val
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| ConfigError::InvalidValue {
                        key: "PORT",
                        value: port_val.clone(),
                        reason: "PORT must be an integer between 1 and 65535",
                    })?;
            config.port = parsed_port;
        }

        if let Ok(env_val) = std::env::var("APP_ENV") {
            config.environment = env_val.trim().parse()?;
        }

        if let Ok(mode_val) = std::env::var("DEPLOYMENT_MODE") {
            config.deployment_mode = mode_val.trim().parse()?;
        }

        if let Ok(body_limit_val) = std::env::var("MAX_BODY_SIZE_BYTES") {
            let parsed_limit =
                body_limit_val
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| ConfigError::InvalidValue {
                        key: "MAX_BODY_SIZE_BYTES",
                        value: body_limit_val.clone(),
                        reason: "Must be a valid byte count",
                    })?;
            config.max_body_size_bytes = parsed_limit;
        }

        if let Ok(timeout_val) = std::env::var("UPSTREAM_TIMEOUT_SECS") {
            let parsed_timeout =
                timeout_val
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| ConfigError::InvalidValue {
                        key: "UPSTREAM_TIMEOUT_SECS",
                        value: timeout_val.clone(),
                        reason: "Must be a valid integer number of seconds",
                    })?;
            config.upstream_timeout_secs = parsed_timeout;
        }

        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                config.groq_api_key = Some(trimmed.to_string());
            }
        }

        if let Ok(model) = std::env::var("GROQ_MODEL") {
            let trimmed = model.trim();
            if !trimmed.is_empty() {
                config.groq_model = Some(trimmed.to_string());
            }
        }

        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                config.openrouter_api_key = Some(trimmed.to_string());
            }
        }

        if let Ok(key) = std::env::var("GEMINI_API_KEY") {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                config.gemini_api_key = Some(trimmed.to_string());
            }
        }

        if let Ok(key) = std::env::var("API_KEY").or_else(|_| std::env::var("BACKEND_API_KEY")) {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                config.api_key = Some(trimmed.to_string());
            }
        }

        config.validate()?;
        Ok(config)
    }

    /// Check if API authentication is required on protected routes.
    /// In production, authentication is strictly mandatory.
    pub fn is_auth_required(&self) -> bool {
        self.environment == Environment::Production || self.api_key.is_some()
    }

    /// Constant-time validation of an incoming API key against configured key.
    /// In production without an API key configured, authentication always fails.
    pub fn validate_api_key(&self, candidate: &str) -> bool {
        if let Some(expected) = &self.api_key {
            let cand_bytes = candidate.as_bytes();
            let exp_bytes = expected.as_bytes();
            if cand_bytes.len() != exp_bytes.len() {
                return false;
            }
            let mut diff = 0u8;
            for (a, b) in cand_bytes.iter().zip(exp_bytes.iter()) {
                diff |= a ^ b;
            }
            diff == 0
        } else {
            // In non-production environments without an API key configured, auth is bypassed for dev
            self.environment != Environment::Production
        }
    }

    /// Validate configuration constraints.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::InvalidValue {
                key: "PORT",
                value: "0".to_string(),
                reason: "Port 0 is reserved and invalid for application binding",
            });
        }

        if self.host.parse::<IpAddr>().is_err() && self.host != "localhost" {
            return Err(ConfigError::InvalidValue {
                key: "HOST",
                value: self.host.clone(),
                reason: "HOST must be a valid IP address (e.g. 127.0.0.1, 0.0.0.0) or 'localhost'",
            });
        }

        if self.max_body_size_bytes < 1_024 || self.max_body_size_bytes > 10_000_000 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_BODY_SIZE_BYTES",
                value: self.max_body_size_bytes.to_string(),
                reason: "Max body size must be between 1 KB and 10 MB",
            });
        }

        if self.upstream_timeout_secs < 1 || self.upstream_timeout_secs > 30 {
            return Err(ConfigError::InvalidValue {
                key: "UPSTREAM_TIMEOUT_SECS",
                value: self.upstream_timeout_secs.to_string(),
                reason: "Upstream timeout must be between 1s and 30s",
            });
        }

        if self.api_key.as_ref().is_some_and(|k| k.len() < 8) {
            return Err(ConfigError::InvalidValue {
                key: "API_KEY",
                value: "[REDACTED]".to_string(),
                reason: "API_KEY must be at least 8 characters long",
            });
        }

        Ok(())
    }
}

/// Custom Debug implementation ensuring zero secrets or credentials are logged.
impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("environment", &self.environment.as_str())
            .field("deployment_mode", &self.deployment_mode.as_str())
            .field("max_body_size_bytes", &self.max_body_size_bytes)
            .field("upstream_timeout_secs", &self.upstream_timeout_secs)
            .field(
                "groq_api_key",
                &self.groq_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("groq_model", &self.groq_model)
            .field(
                "openrouter_api_key",
                &self.openrouter_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "gemini_api_key",
                &self.gemini_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Errors occurring during configuration loading or validation.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    InvalidValue {
        key: &'static str,
        value: String,
        reason: &'static str,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue { key, value, reason } => {
                write!(
                    f,
                    "Configuration error for '{}': invalid value '{}'. {}",
                    key, value, reason
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = AppConfig::default_local();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.deployment_mode, DeploymentMode::Daemon);
        assert_eq!(config.max_body_size_bytes, 4_500_000);
        assert_eq!(config.upstream_timeout_secs, 10);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_invalid_port_zero_rejected() {
        let mut config = AppConfig::default_local();
        config.port = 0;
        let err = config.validate().unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidValue {
                key: "PORT",
                value: "0".to_string(),
                reason: "Port 0 is reserved and invalid for application binding",
            }
        );
    }

    #[test]
    fn test_invalid_host_rejected() {
        let mut config = AppConfig::default_local();
        config.host = "999.999.999.999".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_host_ip_accepted() {
        let mut config = AppConfig::default_local();
        config.host = "0.0.0.0".to_string();
        assert!(config.validate().is_ok());

        config.host = "::1".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_environment_parsing() {
        assert_eq!(
            "development".parse::<Environment>().unwrap(),
            Environment::Development
        );
        assert_eq!(
            "production".parse::<Environment>().unwrap(),
            Environment::Production
        );
        assert_eq!("test".parse::<Environment>().unwrap(), Environment::Test);
        assert!("invalid_env".parse::<Environment>().is_err());
    }

    #[test]
    fn test_deployment_mode_parsing() {
        assert_eq!(
            "daemon".parse::<DeploymentMode>().unwrap(),
            DeploymentMode::Daemon
        );
        assert_eq!(
            "serverless".parse::<DeploymentMode>().unwrap(),
            DeploymentMode::Serverless
        );
        assert!("invalid_mode".parse::<DeploymentMode>().is_err());
    }

    #[test]
    fn test_debug_format_does_not_leak_secrets() {
        let mut config = AppConfig::default_local();
        config.api_key = Some("super_secret_auth_key_12345".to_string());
        config.groq_api_key = Some("gsk_top_secret_groq_key_9999".to_string());
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("AppConfig"));
        assert!(debug_str.contains("127.0.0.1"));
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("super_secret_auth_key_12345"));
        assert!(!debug_str.contains("gsk_top_secret_groq_key_9999"));
    }
}
