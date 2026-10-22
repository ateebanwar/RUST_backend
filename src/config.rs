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
    /// Upstream Groq Cloud API base URL override (loaded from GROQ_BASE_URL).
    pub groq_base_url: Option<String>,
    /// Upstream OpenRouter API key (loaded from OPENROUTER_API_KEY).
    pub openrouter_api_key: Option<String>,
    /// Upstream Google Gemini API key (loaded from GEMINI_API_KEY).
    /// Note: Google Gemini adapter integration is planned for a future multimodal phase;
    /// this configuration field is loaded and isolated, but currently marked unsupported in v1 router.
    pub gemini_api_key: Option<String>,
    /// Backend API key for authenticating protected routes (loaded from API_KEY).
    pub api_key: Option<String>,
    /// List of configured backend API keys (loaded from API_KEYS).
    pub api_keys: Vec<String>,
    /// Google OAuth 2.0 Client ID for Google Sign-In verification (loaded from GOOGLE_CLIENT_ID).
    pub google_client_id: Option<String>,
    /// Additional allowed Google OAuth 2.0 Client IDs (loaded from GOOGLE_CLIENT_IDS).
    pub google_client_ids: Vec<String>,
    /// Custom Google public JWKS discovery URL override (loaded from GOOGLE_JWKS_URL).
    pub google_jwks_url: Option<String>,
    /// Explicit override for requiring authentication in non-production (loaded from AUTH_REQUIRED).
    pub auth_required: Option<bool>,
    /// Path to persistent SQLite database for usage tracking (loaded from DATABASE_PATH).
    pub database_path: String,
    /// Usage history retention window in days (loaded from USAGE_RETENTION_DAYS).
    pub usage_retention_days: Option<u64>,
    /// Maximum retries per candidate for transient errors (loaded from MAX_RETRIES).
    pub max_retries: u32,
    /// Maximum candidate failover targets to attempt in auto mode (loaded from MAX_FAILOVER_CANDIDATES).
    pub max_failover_candidates: usize,
    /// Initial retry backoff in milliseconds (loaded from INITIAL_BACKOFF_MS).
    pub initial_backoff_ms: u64,
    /// Maximum backoff ceiling in milliseconds (loaded from MAX_BACKOFF_MS).
    pub max_backoff_ms: u64,
    /// Maximum accepted upstream Retry-After delay in seconds (loaded from MAX_RETRY_AFTER_SECS).
    pub max_retry_after_secs: u64,
    /// Overall request processing deadline in seconds (loaded from OVERALL_REQUEST_TIMEOUT_SECS).
    pub overall_request_timeout_secs: u64,
    /// Trusted reverse proxy IP/CIDR addresses (loaded from TRUSTED_PROXIES).
    pub trusted_proxies: Vec<String>,
    /// Allowed HTTP Host header values (loaded from ALLOWED_HOSTS).
    pub allowed_hosts: Vec<String>,
    /// Autonomous usage retention cleanup cycle interval in seconds (loaded from USAGE_CLEANUP_INTERVAL_SECS, default 3600).
    pub usage_cleanup_interval_secs: u64,
    /// Bounded batch deletion limit for autonomous usage retention cleanup (loaded from USAGE_CLEANUP_BATCH_SIZE, default 1000).
    pub usage_cleanup_batch_size: usize,
    /// Path to independent local disk spool for durable usage fallback (loaded from USAGE_SPOOL_PATH, default "data/usage_spool.log").
    pub usage_spool_path: String,
    /// Maximum capacity ceiling in bytes for independent usage spool (loaded from USAGE_MAX_SPOOL_BYTES, default 10MB).
    pub usage_max_spool_bytes: u64,

    // --- Phase 13: Abuse Prevention and Rate Limiting ---
    /// Whether rate limiting and abuse controls are actively enforced (loaded from RATE_LIMIT_ENABLED, default true).
    pub rate_limit_enabled: bool,
    /// Unauthenticated IP burst limit (loaded from RATE_LIMIT_IP_BURST, default 60).
    pub rate_limit_ip_burst: u32,
    /// Unauthenticated IP sustained refill rate in req/s (loaded from RATE_LIMIT_IP_RATE, default 10.0).
    pub rate_limit_ip_rate: f64,
    /// Authenticated principal general burst limit (loaded from RATE_LIMIT_PRINCIPAL_BURST, default 120).
    pub rate_limit_principal_burst: u32,
    /// Authenticated principal general sustained refill rate in req/s (loaded from RATE_LIMIT_PRINCIPAL_RATE, default 20.0).
    pub rate_limit_principal_rate: f64,
    /// Chat inference burst limit per principal (loaded from RATE_LIMIT_CHAT_BURST, default 30).
    pub rate_limit_chat_burst: u32,
    /// Chat inference sustained refill rate in req/s (loaded from RATE_LIMIT_CHAT_RATE, default 5.0).
    pub rate_limit_chat_rate: f64,
    /// Sensitive credential management burst limit per principal (loaded from RATE_LIMIT_AUTH_BURST, default 10).
    pub rate_limit_auth_burst: u32,
    /// Sensitive credential management sustained refill rate in req/s (loaded from RATE_LIMIT_AUTH_RATE, default 2.0).
    pub rate_limit_auth_rate: f64,
    /// Maximum global in-flight HTTP requests before temporary overload rejection (loaded from MAX_CONCURRENT_REQUESTS, default 100).
    pub max_concurrent_requests: usize,
    /// Maximum simultaneous active streaming connections (loaded from MAX_CONCURRENT_STREAMS, default 20).
    pub max_concurrent_streams: usize,

    // --- Phase 14: SSRF and Outbound Network Protection ---
    /// Whether SSRF protections are actively enforced (loaded from SSRF_PROTECTION_ENABLED, default true).
    pub ssrf_protection_enabled: bool,
    /// Explicitly allowlisted outbound hostnames and wildcards (loaded from ALLOWED_OUTBOUND_HOSTS).
    pub allowed_outbound_hosts: Vec<String>,
    /// Whether explicitly configured local inference is permitted (loaded from ALLOW_LOCAL_INFERENCE, default false).
    pub allow_local_inference: bool,
    /// Explicit base URL for authorized local inference (loaded from LOCAL_INFERENCE_BASE_URL).
    pub local_inference_base_url: Option<String>,
    /// Maximum allowed HTTP redirects (loaded from MAX_OUTBOUND_REDIRECTS, default 0).
    pub max_outbound_redirects: usize,
    /// Connect timeout for outbound requests in seconds (loaded from OUTBOUND_CONNECT_TIMEOUT_SECS, default 5).
    pub outbound_connect_timeout_secs: u64,

    // --- Phase 15: Google Drive Chat and Library Synchronization ---
    /// Whether Google Drive sync is enabled (loaded from GOOGLE_DRIVE_SYNC_ENABLED, default false).
    pub google_drive_sync_enabled: bool,
    /// Remote application data folder name in Google Drive (loaded from GOOGLE_DRIVE_FOLDER_NAME, default "ai_backend_app_data").
    pub google_drive_folder_name: String,
    /// Timeout for Google Drive API operations in seconds (loaded from GOOGLE_DRIVE_TIMEOUT_SECS, default 30).
    pub google_drive_timeout_secs: u64,
    /// Maximum retries for transient Google Drive failures (loaded from GOOGLE_DRIVE_MAX_RETRIES, default 3).
    pub google_drive_max_retries: u32,
    /// Optional master key for Drive encryption (loaded from DRIVE_ENCRYPTION_KEY, 64-char hex string).
    pub drive_encryption_key: Option<String>,
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
            groq_base_url: None,
            openrouter_api_key: None,
            gemini_api_key: None,
            api_key: None,
            api_keys: Vec::new(),
            google_client_id: None,
            google_client_ids: Vec::new(),
            google_jwks_url: None,
            auth_required: None,
            database_path: "data/usage.db".to_string(),
            usage_retention_days: None,
            max_retries: 1,
            max_failover_candidates: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 1000,
            max_retry_after_secs: 5,
            overall_request_timeout_secs: 30,
            trusted_proxies: vec!["127.0.0.1".to_string(), "::1".to_string()],
            allowed_hosts: Vec::new(),
            usage_cleanup_interval_secs: 3600,
            usage_cleanup_batch_size: 1000,
            usage_spool_path: "data/usage_spool.log".to_string(),
            usage_max_spool_bytes: 10 * 1024 * 1024, // 10 MB default ceiling
            rate_limit_enabled: true,
            rate_limit_ip_burst: 100,
            rate_limit_ip_rate: 20.0,
            rate_limit_principal_burst: 200,
            rate_limit_principal_rate: 50.0,
            rate_limit_chat_burst: 60,
            rate_limit_chat_rate: 15.0,
            rate_limit_auth_burst: 40,
            rate_limit_auth_rate: 10.0,
            max_concurrent_requests: 200,
            max_concurrent_streams: 50,
            ssrf_protection_enabled: true,
            allowed_outbound_hosts: vec![
                "api.groq.com".to_string(),
                "openrouter.ai".to_string(),
                "api.openai.com".to_string(),
                "www.googleapis.com".to_string(),
                "accounts.google.com".to_string(),
                "localhost".to_string(),
                "127.0.0.1".to_string(),
            ],
            allow_local_inference: false,
            local_inference_base_url: None,
            max_outbound_redirects: 0,
            outbound_connect_timeout_secs: 5,
            google_drive_sync_enabled: false,
            google_drive_folder_name: "ai_backend_app_data".to_string(),
            google_drive_timeout_secs: 30,
            google_drive_max_retries: 3,
            drive_encryption_key: None,
        }
    }

    /// Default serverless configuration with in-memory usage storage and ephemeral spooling.
    pub fn default_serverless() -> Self {
        let mut cfg = Self::default_local();
        cfg.deployment_mode = DeploymentMode::Serverless;
        cfg.database_path = ":memory:".to_string();
        cfg.usage_spool_path = "/tmp/usage_spool.log".to_string();
        cfg
    }

    /// Parse .env key-value pairs from raw string content.
    pub fn parse_dotenv(content: &str) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for line in content.lines() {
            let mut line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(stripped) = line.strip_prefix("export ") {
                line = stripped.trim();
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim().to_string();
                let mut v = v.trim();
                if (v.starts_with('"') || v.starts_with('\'')) && v.len() >= 2 {
                    let quote_char = v.chars().next().unwrap();
                    if let Some(end_idx) = v[1..].find(quote_char) {
                        v = &v[1..=end_idx];
                    }
                } else if let Some(idx) = v.find('#') {
                    let before_comment = &v[..idx];
                    if before_comment.ends_with(|c: char| c.is_whitespace()) {
                        v = before_comment.trim();
                    }
                }
                if !k.is_empty() {
                    map.insert(k, v.to_string());
                }
            }
        }
        map
    }

    /// Helper to read local .env key-value pairs without external dependencies.
    fn load_dotenv() -> std::collections::HashMap<String, String> {
        if let Ok(content) = std::fs::read_to_string(".env") {
            Self::parse_dotenv(&content)
        } else {
            std::collections::HashMap::new()
        }
    }

    /// Load configuration from environment variables with fallback to local .env and defaults.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self::default_local();
        let dotenv_map = Self::load_dotenv();

        let get_var = |key: &str| -> Option<String> {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    dotenv_map
                        .get(key)
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                })
        };

        if let Some(host_val) = get_var("HOST") {
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

        if let Some(port_val) = get_var("PORT") {
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

        if let Some(env_val) = get_var("APP_ENV") {
            config.environment = env_val.trim().parse()?;
        }

        if let Some(mode_val) = get_var("DEPLOYMENT_MODE") {
            config.deployment_mode = mode_val.trim().parse()?;
        }

        if let Some(body_limit_val) = get_var("MAX_BODY_SIZE_BYTES") {
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

        if let Some(timeout_val) = get_var("UPSTREAM_TIMEOUT_SECS") {
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

        if let Some(key) = get_var("GROQ_API_KEY") {
            config.groq_api_key = Some(key);
        }

        if let Some(model) = get_var("GROQ_MODEL") {
            config.groq_model = Some(model);
        }

        if let Some(url) = get_var("GROQ_BASE_URL") {
            config.groq_base_url = Some(url);
        }

        if let Some(key) = get_var("OPENROUTER_API_KEY") {
            config.openrouter_api_key = Some(key);
        }

        if let Some(key) = get_var("GEMINI_API_KEY") {
            config.gemini_api_key = Some(key);
        }

        if let Some(key) = get_var("API_KEY").or_else(|| get_var("BACKEND_API_KEY")) {
            config.api_key = Some(key);
        }

        if let Some(keys) = get_var("API_KEYS") {
            config.api_keys = keys
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        if let Some(id) = get_var("GOOGLE_CLIENT_ID") {
            config.google_client_id = Some(id);
        }

        if let Some(ids) = get_var("GOOGLE_CLIENT_IDS") {
            config.google_client_ids = ids
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        if let Some(jwks_url) = get_var("GOOGLE_JWKS_URL") {
            config.google_jwks_url = Some(jwks_url);
        }

        if let Some(auth_req) = get_var("AUTH_REQUIRED").and_then(|r| r.trim().parse::<bool>().ok())
        {
            config.auth_required = Some(auth_req);
        }

        if let Some(db) = get_var("DATABASE_PATH") {
            config.database_path = db;
        } else if config.deployment_mode == DeploymentMode::Serverless {
            config.database_path = ":memory:".to_string();
        }

        if let Some(days) =
            get_var("USAGE_RETENTION_DAYS").and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.usage_retention_days = Some(days);
        }

        if let Some(r) = get_var("MAX_RETRIES").and_then(|r| r.trim().parse::<u32>().ok()) {
            config.max_retries = r;
        }

        if let Some(c) =
            get_var("MAX_FAILOVER_CANDIDATES").and_then(|r| r.trim().parse::<usize>().ok())
        {
            config.max_failover_candidates = c;
        }

        if let Some(ms) = get_var("INITIAL_BACKOFF_MS").and_then(|r| r.trim().parse::<u64>().ok()) {
            config.initial_backoff_ms = ms;
        }

        if let Some(ms) = get_var("MAX_BACKOFF_MS").and_then(|r| r.trim().parse::<u64>().ok()) {
            config.max_backoff_ms = ms;
        }

        if let Some(secs) =
            get_var("MAX_RETRY_AFTER_SECS").and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.max_retry_after_secs = secs;
        }

        if let Some(secs) = get_var("OVERALL_REQUEST_TIMEOUT_SECS")
            .or_else(|| get_var("REQUEST_TIMEOUT_SECS"))
            .and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.overall_request_timeout_secs = secs;
        }

        if let Some(proxies) = get_var("TRUSTED_PROXIES") {
            config.trusted_proxies = proxies
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        if let Some(hosts) = get_var("ALLOWED_HOSTS") {
            config.allowed_hosts = hosts
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        if let Some(secs) =
            get_var("USAGE_CLEANUP_INTERVAL_SECS").and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.usage_cleanup_interval_secs = secs;
        }

        if let Some(batch) =
            get_var("USAGE_CLEANUP_BATCH_SIZE").and_then(|r| r.trim().parse::<usize>().ok())
        {
            config.usage_cleanup_batch_size = batch;
        }

        if let Some(spool) = get_var("USAGE_SPOOL_PATH") {
            config.usage_spool_path = spool;
        } else if config.deployment_mode == DeploymentMode::Serverless {
            config.usage_spool_path = "/tmp/usage_spool.log".to_string();
        }

        if let Some(bytes) =
            get_var("USAGE_MAX_SPOOL_BYTES").and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.usage_max_spool_bytes = bytes;
        }

        if let Some(rle) = get_var("RATE_LIMIT_ENABLED").and_then(|r| r.trim().parse::<bool>().ok())
        {
            config.rate_limit_enabled = rle;
        }

        if let Some(burst) =
            get_var("RATE_LIMIT_IP_BURST").and_then(|r| r.trim().parse::<u32>().ok())
        {
            config.rate_limit_ip_burst = burst;
        }

        if let Some(rate) = get_var("RATE_LIMIT_IP_RATE").and_then(|r| r.trim().parse::<f64>().ok())
        {
            config.rate_limit_ip_rate = rate;
        }

        if let Some(burst) =
            get_var("RATE_LIMIT_PRINCIPAL_BURST").and_then(|r| r.trim().parse::<u32>().ok())
        {
            config.rate_limit_principal_burst = burst;
        }

        if let Some(rate) =
            get_var("RATE_LIMIT_PRINCIPAL_RATE").and_then(|r| r.trim().parse::<f64>().ok())
        {
            config.rate_limit_principal_rate = rate;
        }

        if let Some(burst) =
            get_var("RATE_LIMIT_CHAT_BURST").and_then(|r| r.trim().parse::<u32>().ok())
        {
            config.rate_limit_chat_burst = burst;
        }

        if let Some(rate) =
            get_var("RATE_LIMIT_CHAT_RATE").and_then(|r| r.trim().parse::<f64>().ok())
        {
            config.rate_limit_chat_rate = rate;
        }

        if let Some(burst) =
            get_var("RATE_LIMIT_AUTH_BURST").and_then(|r| r.trim().parse::<u32>().ok())
        {
            config.rate_limit_auth_burst = burst;
        }

        if let Some(rate) =
            get_var("RATE_LIMIT_AUTH_RATE").and_then(|r| r.trim().parse::<f64>().ok())
        {
            config.rate_limit_auth_rate = rate;
        }

        if let Some(mcr) =
            get_var("MAX_CONCURRENT_REQUESTS").and_then(|r| r.trim().parse::<usize>().ok())
        {
            config.max_concurrent_requests = mcr;
        }

        if let Some(mcs) =
            get_var("MAX_CONCURRENT_STREAMS").and_then(|r| r.trim().parse::<usize>().ok())
        {
            config.max_concurrent_streams = mcs;
        }

        if let Some(ssrf) =
            get_var("SSRF_PROTECTION_ENABLED").and_then(|r| r.trim().parse::<bool>().ok())
        {
            config.ssrf_protection_enabled = ssrf;
        }

        if let Some(hosts) = get_var("ALLOWED_OUTBOUND_HOSTS") {
            config.allowed_outbound_hosts = hosts
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        if let Some(ali) =
            get_var("ALLOW_LOCAL_INFERENCE").and_then(|r| r.trim().parse::<bool>().ok())
        {
            config.allow_local_inference = ali;
        }

        if let Some(url) = get_var("LOCAL_INFERENCE_BASE_URL") {
            config.local_inference_base_url = Some(url);
        }

        if let Some(red) =
            get_var("MAX_OUTBOUND_REDIRECTS").and_then(|r| r.trim().parse::<usize>().ok())
        {
            config.max_outbound_redirects = red;
        }

        if let Some(timeout) =
            get_var("OUTBOUND_CONNECT_TIMEOUT_SECS").and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.outbound_connect_timeout_secs = timeout;
        }

        if let Some(gdse) =
            get_var("GOOGLE_DRIVE_SYNC_ENABLED").and_then(|r| r.trim().parse::<bool>().ok())
        {
            config.google_drive_sync_enabled = gdse;
        }

        if let Some(folder) = get_var("GOOGLE_DRIVE_FOLDER_NAME") {
            config.google_drive_folder_name = folder;
        }

        if let Some(timeout) =
            get_var("GOOGLE_DRIVE_TIMEOUT_SECS").and_then(|r| r.trim().parse::<u64>().ok())
        {
            config.google_drive_timeout_secs = timeout;
        }

        if let Some(retries) =
            get_var("GOOGLE_DRIVE_MAX_RETRIES").and_then(|r| r.trim().parse::<u32>().ok())
        {
            config.google_drive_max_retries = retries;
        }

        if let Some(key) = get_var("DRIVE_ENCRYPTION_KEY") {
            config.drive_encryption_key = Some(key);
        }

        config.validate()?;
        Ok(config)
    }

    /// Create a TrustedProxyMatcher from configured trusted proxies.
    pub fn trusted_proxy_matcher(&self) -> crate::security::TrustedProxyMatcher {
        crate::security::TrustedProxyMatcher::new(&self.trusted_proxies).unwrap_or_default()
    }

    /// Create a HostValidator from configured allowed hosts.
    pub fn host_validator(&self) -> crate::security::HostValidator {
        crate::security::HostValidator::new(&self.allowed_hosts)
    }

    /// Create RateLimiterConfig from application configuration.
    pub fn rate_limiter_config(&self) -> crate::security::RateLimiterConfig {
        crate::security::RateLimiterConfig {
            enabled: self.rate_limit_enabled,
            ip_burst: self.rate_limit_ip_burst,
            ip_rate_per_sec: self.rate_limit_ip_rate,
            principal_burst: self.rate_limit_principal_burst,
            principal_rate_per_sec: self.rate_limit_principal_rate,
            chat_burst: self.rate_limit_chat_burst,
            chat_rate_per_sec: self.rate_limit_chat_rate,
            auth_burst: self.rate_limit_auth_burst,
            auth_rate_per_sec: self.rate_limit_auth_rate,
            max_concurrent_requests: self.max_concurrent_requests,
            max_concurrent_streams: self.max_concurrent_streams,
            max_entries: 10_000,
            entry_ttl: std::time::Duration::from_secs(300),
        }
    }

    /// Create OutboundDestinationPolicy from application configuration.
    pub fn outbound_destination_policy(&self) -> crate::security::OutboundDestinationPolicy {
        crate::security::OutboundDestinationPolicy::default_secure()
            .with_allowed_hosts(self.allowed_outbound_hosts.clone())
            .with_local_inference(
                self.allow_local_inference,
                self.local_inference_base_url.clone(),
            )
    }

    /// Create RetryPolicy from current application configuration.
    pub fn retry_policy(&self) -> crate::reliability::RetryPolicy {
        crate::reliability::RetryPolicy {
            max_retries_per_candidate: self.max_retries,
            max_failover_candidates: self.max_failover_candidates,
            initial_backoff_ms: self.initial_backoff_ms,
            max_backoff_ms: self.max_backoff_ms,
            max_retry_after_secs: self.max_retry_after_secs,
            overall_request_timeout_secs: self.overall_request_timeout_secs,
        }
    }

    /// Check if API authentication is required on protected routes.
    /// In production or on non-loopback network interfaces, authentication is strictly mandatory.
    pub fn is_auth_required(&self) -> bool {
        // In production or on non-loopback hosts, authentication is strictly and unconditionally required
        if self.environment == Environment::Production || !self.is_loopback_host() {
            return true;
        }
        if let Some(explicit) = self.auth_required {
            return explicit;
        }
        self.api_key.is_some()
            || !self.api_keys.is_empty()
            || self.google_client_id.is_some()
            || !self.google_client_ids.is_empty()
    }

    /// Check if the configured host bind address is loopback (accessible only locally).
    pub fn is_loopback_host(&self) -> bool {
        let trimmed = self.host.trim();
        if trimmed.eq_ignore_ascii_case("localhost") {
            return true;
        }
        // Strip IPv6 brackets if present (e.g. "[::1]")
        let unbracketed = trimmed.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = unbracketed.parse::<IpAddr>() {
            ip.is_loopback()
        } else {
            false
        }
    }

    /// Constant-time validation of an incoming API key against configured keys.
    /// In production without an API key configured, authentication always fails.
    pub fn validate_api_key(&self, candidate: &str) -> bool {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return false;
        }

        let has_configured_keys = self.api_key.is_some() || !self.api_keys.is_empty();
        if has_configured_keys {
            if let Some(expected) = &self.api_key
                && crate::auth::pat::constant_time_compare_str(trimmed, expected)
            {
                return true;
            }
            for expected in &self.api_keys {
                if crate::auth::pat::constant_time_compare_str(trimmed, expected) {
                    return true;
                }
            }
            false
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

        let host_trimmed = self.host.trim();
        let host_unbracketed = host_trimmed.trim_start_matches('[').trim_end_matches(']');
        if host_unbracketed.parse::<IpAddr>().is_err()
            && !host_trimmed.eq_ignore_ascii_case("localhost")
        {
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

        // Fail-closed production and external-bind enforcement
        let has_key = self.api_key.as_ref().is_some_and(|k| !k.trim().is_empty())
            || self.api_keys.iter().any(|k| !k.trim().is_empty());
        let has_google = self
            .google_client_id
            .as_ref()
            .is_some_and(|id| !id.trim().is_empty())
            || self
                .google_client_ids
                .iter()
                .any(|id| !id.trim().is_empty());

        if self.environment == Environment::Production {
            if self.auth_required == Some(false) {
                return Err(ConfigError::InvalidValue {
                    key: "AUTH_REQUIRED",
                    value: "false".to_string(),
                    reason: "AUTH_REQUIRED cannot be set to false in production environment",
                });
            }

            if !has_key && !has_google {
                return Err(ConfigError::InvalidValue {
                    key: "APP_ENV",
                    value: "production".to_string(),
                    reason: "Production environment requires at least one valid API_KEY, API_KEYS, or GOOGLE_CLIENT_ID configured for authentication",
                });
            }

            if let Some(ref jwks_url) = self.google_jwks_url
                && !jwks_url.starts_with("https://")
            {
                return Err(ConfigError::InvalidValue {
                    key: "GOOGLE_JWKS_URL",
                    value: jwks_url.clone(),
                    reason: "GOOGLE_JWKS_URL must use HTTPS protocol in production",
                });
            }
        }

        if self.api_key.as_ref().is_some_and(|k| k.len() < 8) {
            return Err(ConfigError::InvalidValue {
                key: "API_KEY",
                value: "[REDACTED]".to_string(),
                reason: "API_KEY must be at least 8 characters long",
            });
        }

        for key in &self.api_keys {
            if key.len() < 8 {
                return Err(ConfigError::InvalidValue {
                    key: "API_KEYS",
                    value: "[REDACTED]".to_string(),
                    reason: "Configured API keys in API_KEYS must be at least 8 characters long",
                });
            }
        }

        if self.usage_retention_days == Some(0) {
            return Err(ConfigError::InvalidValue {
                key: "USAGE_RETENTION_DAYS",
                value: "0".to_string(),
                reason: "Usage retention days must be greater than zero when configured",
            });
        }

        if self.max_retries > 5 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_RETRIES",
                value: self.max_retries.to_string(),
                reason: "MAX_RETRIES must not exceed 5",
            });
        }

        if self.max_failover_candidates == 0 || self.max_failover_candidates > 10 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_FAILOVER_CANDIDATES",
                value: self.max_failover_candidates.to_string(),
                reason: "MAX_FAILOVER_CANDIDATES must be between 1 and 10",
            });
        }

        if self.initial_backoff_ms == 0 || self.initial_backoff_ms > 5000 {
            return Err(ConfigError::InvalidValue {
                key: "INITIAL_BACKOFF_MS",
                value: self.initial_backoff_ms.to_string(),
                reason: "INITIAL_BACKOFF_MS must be between 1 and 5000 ms",
            });
        }

        if self.max_backoff_ms < self.initial_backoff_ms {
            return Err(ConfigError::InvalidValue {
                key: "MAX_BACKOFF_MS",
                value: self.max_backoff_ms.to_string(),
                reason: "MAX_BACKOFF_MS must be greater than or equal to INITIAL_BACKOFF_MS",
            });
        }

        if self.max_retry_after_secs == 0 || self.max_retry_after_secs > 60 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_RETRY_AFTER_SECS",
                value: self.max_retry_after_secs.to_string(),
                reason: "MAX_RETRY_AFTER_SECS must be between 1 and 60 seconds",
            });
        }

        if self.overall_request_timeout_secs == 0 || self.overall_request_timeout_secs > 120 {
            return Err(ConfigError::InvalidValue {
                key: "OVERALL_REQUEST_TIMEOUT_SECS",
                value: self.overall_request_timeout_secs.to_string(),
                reason: "OVERALL_REQUEST_TIMEOUT_SECS must be between 1 and 120 seconds",
            });
        }

        for proxy_cidr in &self.trusted_proxies {
            if crate::security::IpCidr::parse(proxy_cidr).is_err() {
                return Err(ConfigError::InvalidValue {
                    key: "TRUSTED_PROXIES",
                    value: proxy_cidr.clone(),
                    reason: "Invalid CIDR or IP format in TRUSTED_PROXIES",
                });
            }
        }

        if self.usage_cleanup_interval_secs == 0 {
            return Err(ConfigError::InvalidValue {
                key: "USAGE_CLEANUP_INTERVAL_SECS",
                value: "0".to_string(),
                reason: "USAGE_CLEANUP_INTERVAL_SECS must be at least 1 second to avoid busy-loops",
            });
        }

        if self.usage_cleanup_batch_size == 0 || self.usage_cleanup_batch_size > 100_000 {
            return Err(ConfigError::InvalidValue {
                key: "USAGE_CLEANUP_BATCH_SIZE",
                value: self.usage_cleanup_batch_size.to_string(),
                reason: "USAGE_CLEANUP_BATCH_SIZE must be between 1 and 100,000",
            });
        }

        if self.usage_spool_path.trim().is_empty() {
            return Err(ConfigError::InvalidValue {
                key: "USAGE_SPOOL_PATH",
                value: self.usage_spool_path.clone(),
                reason: "USAGE_SPOOL_PATH cannot be empty",
            });
        }

        if self.usage_max_spool_bytes < 1024 || self.usage_max_spool_bytes > 1_000_000_000 {
            return Err(ConfigError::InvalidValue {
                key: "USAGE_MAX_SPOOL_BYTES",
                value: self.usage_max_spool_bytes.to_string(),
                reason: "USAGE_MAX_SPOOL_BYTES must be between 1 KB (1024) and 1 GB (1,000,000,000)",
            });
        }

        // --- Phase 13: Rate limiting validation ---
        if self.rate_limit_ip_burst == 0 {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_IP_BURST",
                value: "0".to_string(),
                reason: "RATE_LIMIT_IP_BURST must be greater than 0",
            });
        }
        if self.rate_limit_ip_rate <= 0.0 || !self.rate_limit_ip_rate.is_finite() {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_IP_RATE",
                value: self.rate_limit_ip_rate.to_string(),
                reason: "RATE_LIMIT_IP_RATE must be positive finite number",
            });
        }
        if self.rate_limit_principal_burst == 0 {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_PRINCIPAL_BURST",
                value: "0".to_string(),
                reason: "RATE_LIMIT_PRINCIPAL_BURST must be greater than 0",
            });
        }
        if self.rate_limit_principal_rate <= 0.0 || !self.rate_limit_principal_rate.is_finite() {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_PRINCIPAL_RATE",
                value: self.rate_limit_principal_rate.to_string(),
                reason: "RATE_LIMIT_PRINCIPAL_RATE must be positive finite number",
            });
        }
        if self.rate_limit_chat_burst == 0 {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_CHAT_BURST",
                value: "0".to_string(),
                reason: "RATE_LIMIT_CHAT_BURST must be greater than 0",
            });
        }
        if self.rate_limit_chat_rate <= 0.0 || !self.rate_limit_chat_rate.is_finite() {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_CHAT_RATE",
                value: self.rate_limit_chat_rate.to_string(),
                reason: "RATE_LIMIT_CHAT_RATE must be positive finite number",
            });
        }
        if self.rate_limit_auth_burst == 0 {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_AUTH_BURST",
                value: "0".to_string(),
                reason: "RATE_LIMIT_AUTH_BURST must be greater than 0",
            });
        }
        if self.rate_limit_auth_rate <= 0.0 || !self.rate_limit_auth_rate.is_finite() {
            return Err(ConfigError::InvalidValue {
                key: "RATE_LIMIT_AUTH_RATE",
                value: self.rate_limit_auth_rate.to_string(),
                reason: "RATE_LIMIT_AUTH_RATE must be positive finite number",
            });
        }
        if self.max_concurrent_requests == 0 || self.max_concurrent_requests > 100_000 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_CONCURRENT_REQUESTS",
                value: self.max_concurrent_requests.to_string(),
                reason: "MAX_CONCURRENT_REQUESTS must be between 1 and 100,000",
            });
        }
        if self.max_concurrent_streams == 0 || self.max_concurrent_streams > 10_000 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_CONCURRENT_STREAMS",
                value: self.max_concurrent_streams.to_string(),
                reason: "MAX_CONCURRENT_STREAMS must be between 1 and 10,000",
            });
        }

        // --- Phase 14: SSRF validation ---
        if self.max_outbound_redirects > 10 {
            return Err(ConfigError::InvalidValue {
                key: "MAX_OUTBOUND_REDIRECTS",
                value: self.max_outbound_redirects.to_string(),
                reason: "MAX_OUTBOUND_REDIRECTS must not exceed 10",
            });
        }
        if self.outbound_connect_timeout_secs == 0 || self.outbound_connect_timeout_secs > 60 {
            return Err(ConfigError::InvalidValue {
                key: "OUTBOUND_CONNECT_TIMEOUT_SECS",
                value: self.outbound_connect_timeout_secs.to_string(),
                reason: "OUTBOUND_CONNECT_TIMEOUT_SECS must be between 1 and 60 seconds",
            });
        }
        if self.allow_local_inference {
            if let Some(ref base_url) = self.local_inference_base_url {
                if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
                    return Err(ConfigError::InvalidValue {
                        key: "LOCAL_INFERENCE_BASE_URL",
                        value: base_url.clone(),
                        reason: "LOCAL_INFERENCE_BASE_URL must begin with http:// or https://",
                    });
                }
            } else {
                return Err(ConfigError::InvalidValue {
                    key: "LOCAL_INFERENCE_BASE_URL",
                    value: "None".to_string(),
                    reason: "LOCAL_INFERENCE_BASE_URL must be specified when ALLOW_LOCAL_INFERENCE is true",
                });
            }
        }

        // --- Phase 15: Google Drive encryption key validation ---
        if let Some(ref key) = self.drive_encryption_key {
            if key.is_empty() {
                return Err(ConfigError::InvalidValue {
                    key: "DRIVE_ENCRYPTION_KEY",
                    value: "[REDACTED]".to_string(),
                    reason: "DRIVE_ENCRYPTION_KEY must not be empty",
                });
            }

            if key.len() != 64 {
                return Err(ConfigError::InvalidValue {
                    key: "DRIVE_ENCRYPTION_KEY",
                    value: "[REDACTED]".to_string(),
                    reason: "DRIVE_ENCRYPTION_KEY must be exactly 64 hexadecimal characters (32 bytes for AES-256)",
                });
            }

            if !key.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ConfigError::InvalidValue {
                    key: "DRIVE_ENCRYPTION_KEY",
                    value: "[REDACTED]".to_string(),
                    reason: "DRIVE_ENCRYPTION_KEY must contain only hexadecimal characters (0-9, a-f, A-F)",
                });
            }
        }

        if self.google_drive_sync_enabled && self.drive_encryption_key.is_none() {
            return Err(ConfigError::InvalidValue {
                key: "DRIVE_ENCRYPTION_KEY",
                value: "[MISSING]".to_string(),
                reason: "DRIVE_ENCRYPTION_KEY is required when Google Drive synchronization is enabled (GOOGLE_DRIVE_SYNC_ENABLED=true)",
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
            .field("groq_base_url", &self.groq_base_url)
            .field(
                "openrouter_api_key",
                &self.openrouter_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "gemini_api_key",
                &self.gemini_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field(
                "api_keys",
                &format!("[REDACTED ({} keys)]", self.api_keys.len()),
            )
            .field("google_client_id", &self.google_client_id)
            .field("google_client_ids", &self.google_client_ids)
            .field("google_jwks_url", &self.google_jwks_url)
            .field("auth_required", &self.auth_required)
            .field("database_path", &self.database_path)
            .field("usage_retention_days", &self.usage_retention_days)
            .field("max_retries", &self.max_retries)
            .field("max_failover_candidates", &self.max_failover_candidates)
            .field("initial_backoff_ms", &self.initial_backoff_ms)
            .field("max_backoff_ms", &self.max_backoff_ms)
            .field("max_retry_after_secs", &self.max_retry_after_secs)
            .field(
                "overall_request_timeout_secs",
                &self.overall_request_timeout_secs,
            )
            .field("trusted_proxies", &self.trusted_proxies)
            .field("allowed_hosts", &self.allowed_hosts)
            .field(
                "usage_cleanup_interval_secs",
                &self.usage_cleanup_interval_secs,
            )
            .field("usage_cleanup_batch_size", &self.usage_cleanup_batch_size)
            .field("usage_spool_path", &self.usage_spool_path)
            .field("usage_max_spool_bytes", &self.usage_max_spool_bytes)
            .field("rate_limit_enabled", &self.rate_limit_enabled)
            .field("rate_limit_ip_burst", &self.rate_limit_ip_burst)
            .field("rate_limit_ip_rate", &self.rate_limit_ip_rate)
            .field(
                "rate_limit_principal_burst",
                &self.rate_limit_principal_burst,
            )
            .field("rate_limit_principal_rate", &self.rate_limit_principal_rate)
            .field("rate_limit_chat_burst", &self.rate_limit_chat_burst)
            .field("rate_limit_chat_rate", &self.rate_limit_chat_rate)
            .field("rate_limit_auth_burst", &self.rate_limit_auth_burst)
            .field("rate_limit_auth_rate", &self.rate_limit_auth_rate)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("max_concurrent_streams", &self.max_concurrent_streams)
            .field("ssrf_protection_enabled", &self.ssrf_protection_enabled)
            .field("allowed_outbound_hosts", &self.allowed_outbound_hosts)
            .field("allow_local_inference", &self.allow_local_inference)
            .field("local_inference_base_url", &self.local_inference_base_url)
            .field("max_outbound_redirects", &self.max_outbound_redirects)
            .field(
                "outbound_connect_timeout_secs",
                &self.outbound_connect_timeout_secs,
            )
            .field("google_drive_sync_enabled", &self.google_drive_sync_enabled)
            .field("google_drive_folder_name", &self.google_drive_folder_name)
            .field(
                "drive_encryption_key",
                &self.drive_encryption_key.as_ref().map(|_| "[REDACTED]"),
            )
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

    #[test]
    fn test_dotenv_parser_handles_edge_cases() {
        let env_content = r#"
            # Comment line
            PORT=9000
            export HOST="0.0.0.0" # inline comment
            GROQ_API_KEY='secret_key_val'
            EMPTY_VAL=
            NO_EQUAL_SIGN_LINE
            WITH_EQUALS=part1=part2
            UNQUOTED_WITH_COMMENT=my_value # trailing comment
        "#;
        let map = AppConfig::parse_dotenv(env_content);
        assert_eq!(map.get("PORT").unwrap(), "9000");
        assert_eq!(map.get("HOST").unwrap(), "0.0.0.0");
        assert_eq!(map.get("GROQ_API_KEY").unwrap(), "secret_key_val");
        assert_eq!(map.get("EMPTY_VAL").unwrap(), "");
        assert_eq!(map.get("WITH_EQUALS").unwrap(), "part1=part2");
        assert_eq!(map.get("UNQUOTED_WITH_COMMENT").unwrap(), "my_value");
        assert!(!map.contains_key("NO_EQUAL_SIGN_LINE"));
    }

    #[test]
    fn test_production_fails_closed_without_auth_credentials() {
        let mut config = AppConfig::default_local();
        config.environment = Environment::Production;
        config.api_key = None;
        config.api_keys.clear();
        config.google_client_id = None;
        config.google_client_ids.clear();

        let err = config.validate().unwrap_err();
        match err {
            ConfigError::InvalidValue { key, reason, .. } => {
                assert_eq!(key, "APP_ENV");
                assert!(reason.contains("requires at least one valid API_KEY"));
            }
        }

        // Passes with an API key
        config.api_key = Some("super_secure_production_key_123".to_string());
        assert!(config.validate().is_ok());

        // Passes with Google Client ID
        config.api_key = None;
        config.google_client_id = Some("123456789-abc.apps.googleusercontent.com".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_short_api_keys_in_list_rejected() {
        let mut config = AppConfig::default_local();
        config.api_keys = vec!["short".to_string()];
        let err = config.validate().unwrap_err();
        match err {
            ConfigError::InvalidValue { key, reason, .. } => {
                assert_eq!(key, "API_KEYS");
                assert!(reason.contains("at least 8 characters"));
            }
        }
    }
}
