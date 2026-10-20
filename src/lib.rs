//! ai-backend core library
//!
//! A high-performance, security-focused AI model aggregation backend in Rust.

pub mod config;
pub mod errors;
pub mod inference;
pub mod models;
pub mod providers;
pub mod quota;
pub mod registry;
pub mod reliability;
pub mod router;
pub mod routes;
pub mod security;
pub mod usage;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use config::AppConfig;
pub use errors::AppError;
pub use inference::{ChatCompletionRequest, InferenceService};
pub use providers::ProviderRegistry;
pub use quota::{ProviderQuota, QuotaService};
pub use registry::{ModelFilter, ModelRegistry, RegistryError};
pub use reliability::RetryPolicy;
pub use router::{
    ClassificationResult, MAX_PROMPT_LENGTH, ModelRouter, RoutingPolicy, RoutingPreviewRequest,
    RoutingPreviewResponse, ScoredCandidate, TaskCategory, TaskClassifier, TaskComplexity,
    TaskModality,
};
pub use usage::{UsageRecord, UsageService, UsageStorage};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Wrapper for tracing request IDs across extensions and handlers.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Shared application state accessible across HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub registry: Arc<ModelRegistry>,
    pub providers: Arc<ProviderRegistry>,
    pub router: Arc<ModelRouter>,
    pub inference: Arc<InferenceService>,
    pub usage: Arc<UsageService>,
    pub quota: Arc<QuotaService>,
    pub start_time: std::time::Instant,
}

impl AppState {
    pub fn new(config: Arc<AppConfig>) -> Self {
        let registry = Arc::new(ModelRegistry::new());
        let providers = Arc::new(ProviderRegistry::new());
        let quota = Arc::new(QuotaService::new());

        let usage_storage = match UsageStorage::new(&config.database_path) {
            Ok(s) => Arc::new(s),
            Err(err) => {
                tracing::error!(
                    path = %config.database_path,
                    error = %err,
                    "Failed to initialize SQLite usage storage at configured path, falling back to in-memory store"
                );
                Arc::new(
                    UsageStorage::new(":memory:")
                        .expect("Failed to initialize fallback in-memory SQLite storage"),
                )
            }
        };
        let usage = Arc::new(UsageService::new(usage_storage));

        // Enforce usage retention cleanup on startup if explicitly configured
        if let Some(retention_days) = config.usage_retention_days.filter(|&d| d > 0) {
            match usage.prune_retention_sync(retention_days) {
                Ok(pruned) => {
                    if pruned > 0 {
                        tracing::info!(
                            retention_days,
                            pruned_records = pruned,
                            "Enforced usage retention policy on startup"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        retention_days,
                        error = %err,
                        "Failed to execute startup usage retention cleanup"
                    );
                }
            }
        }

        // Register Groq adapter if GROQ_API_KEY is configured
        if let Some(groq_key) = &config.groq_api_key {
            let groq_fast_model = config
                .groq_model
                .clone()
                .unwrap_or_else(|| "openai/gpt-oss-120b".to_string());
            let groq_balanced_model = std::env::var("GROQ_BALANCED_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "openai/gpt-oss-20b".to_string());

            let groq_base_url = config
                .groq_base_url
                .as_deref()
                .unwrap_or("https://api.groq.com/openai/v1");

            let groq_adapter = Arc::new(
                providers::OpenAiCompatibleAdapter::new(
                    "groq-api",
                    "Groq Cloud Fast Inference",
                    groq_base_url,
                    Some(groq_key.clone()),
                )
                .with_rate_limit_tier("free-tier-standard")
                .with_quota_service(quota.clone())
                .with_model_wire_map({
                    let mut map = std::collections::HashMap::new();
                    map.insert("fast-chat-v1".to_string(), groq_fast_model.clone());
                    map.insert(
                        "balanced-assistant-v1".to_string(),
                        groq_balanced_model.clone(),
                    );
                    map.insert(groq_fast_model.clone(), groq_fast_model);
                    map.insert(groq_balanced_model.clone(), groq_balanced_model);
                    map.insert(
                        "openai/gpt-oss-120b".to_string(),
                        "openai/gpt-oss-120b".to_string(),
                    );
                    map.insert(
                        "openai/gpt-oss-20b".to_string(),
                        "openai/gpt-oss-20b".to_string(),
                    );
                    map.insert(
                        "qwen/qwen3.8-27b".to_string(),
                        "qwen/qwen3.8-27b".to_string(),
                    );
                    map
                }),
            );
            let _ = providers.register(groq_adapter);
        }

        // Register OpenRouter adapter if OPENROUTER_API_KEY is configured
        if let Some(or_key) = &config.openrouter_api_key {
            let or_adapter = Arc::new(
                providers::OpenAiCompatibleAdapter::new(
                    "openrouter-api",
                    "OpenRouter Unified Router",
                    "https://openrouter.ai/api/v1",
                    Some(or_key.clone()),
                )
                .with_rate_limit_tier("standard-tier")
                .with_quota_service(quota.clone())
                .with_model_wire_map({
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "deep-reasoner-v1".to_string(),
                        "deepseek/deepseek-r1".to_string(),
                    );
                    map
                }),
            );
            let _ = providers.register(or_adapter);
        }

        let router = Arc::new(ModelRouter::new(registry.clone()));
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );

        Self {
            config,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn with_components(
        config: Arc<AppConfig>,
        registry: Arc<ModelRegistry>,
        router: Arc<ModelRouter>,
    ) -> Self {
        let providers = Arc::new(ProviderRegistry::new());
        let quota = Arc::new(QuotaService::new());
        let storage = Arc::new(UsageStorage::new(":memory:").expect("In-memory storage failed"));
        let usage = Arc::new(UsageService::new(storage));
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );
        Self {
            config,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn with_all_components(
        config: Arc<AppConfig>,
        registry: Arc<ModelRegistry>,
        providers: Arc<ProviderRegistry>,
        router: Arc<ModelRouter>,
    ) -> Self {
        let quota = Arc::new(QuotaService::new());
        let storage = Arc::new(UsageStorage::new(":memory:").expect("In-memory storage failed"));
        let usage = Arc::new(UsageService::new(storage));
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );
        Self {
            config,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn with_usage_and_quota(
        config: Arc<AppConfig>,
        registry: Arc<ModelRegistry>,
        providers: Arc<ProviderRegistry>,
        router: Arc<ModelRouter>,
        usage: Arc<UsageService>,
        quota: Arc<QuotaService>,
    ) -> Self {
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );
        Self {
            config,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            start_time: std::time::Instant::now(),
        }
    }
}

/// Request ID resolution and API Authentication middleware.
async fn request_id_and_auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    // 1. Resolve or generate Request ID (API_SPEC.md §1.1)
    let request_id_str = if let Some(header_val) = req.headers().get("x-request-id") {
        if let Ok(s) = header_val.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty()
                && trimmed.len() <= 128
                && trimmed
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                trimmed.to_string()
            } else {
                format!(
                    "req_{:012x}",
                    REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
                )
            }
        } else {
            format!(
                "req_{:012x}",
                REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            )
        }
    } else {
        format!(
            "req_{:012x}",
            REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    };

    req.extensions_mut()
        .insert(RequestId(request_id_str.clone()));

    // 1b. Host Header Validation
    if !state.config.allowed_hosts.is_empty() {
        let host_val = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if !state.config.host_validator().validate_host(host_val) {
            let err = AppError::InvalidRequest("Invalid or untrusted Host header".to_string());
            return err.to_response_with_id(&request_id_str);
        }
    }

    // 1c. Forwarded Header Protection against spoofing from untrusted peers
    let peer_ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip());

    let is_trusted_peer = match peer_ip {
        Some(ip) => state.config.trusted_proxy_matcher().is_trusted(&ip),
        None => state.config.is_loopback_host(),
    };

    if !is_trusted_peer {
        req.headers_mut().remove("x-forwarded-for");
        req.headers_mut().remove("forwarded");
        req.headers_mut().remove("x-forwarded-proto");
        req.headers_mut().remove("x-forwarded-host");
    }

    // 2. Authentication enforcement (API_SPEC.md §1.1)
    let path = req.uri().path();
    let is_public_probe = path == "/api/v1/health" || path == "/api/v1/ready";
    let is_operational_data = path.starts_with("/api/v1/usage") || path == "/api/v1/quotas";

    // Operational endpoints on non-loopback bind address must always be authenticated
    // to prevent exposing telemetry over an externally reachable unauthenticated server.
    let operational_protection_required = is_operational_data && !state.config.is_loopback_host();

    if !is_public_probe && (state.config.is_auth_required() || operational_protection_required) {
        let is_authorized = if operational_protection_required && state.config.api_key.is_none() {
            // When bound to an external address without an API key, operational data access is rejected.
            false
        } else {
            req.headers()
                .get("x-api-key")
                .and_then(|hdr| hdr.to_str().ok())
                .is_some_and(|key| state.config.validate_api_key(key.trim()))
                || req
                    .headers()
                    .get("authorization")
                    .and_then(|hdr| hdr.to_str().ok())
                    .and_then(|val| val.trim().strip_prefix("Bearer "))
                    .is_some_and(|tok| state.config.validate_api_key(tok.trim()))
        };

        if !is_authorized {
            let err = AppError::Unauthorized(
                if operational_protection_required && state.config.api_key.is_none() {
                    "Authentication required: operational telemetry is protected on external interfaces"
                        .to_string()
                } else {
                    "Missing or invalid authentication credentials".to_string()
                },
            );
            return err.to_response_with_id(&request_id_str);
        }
    }

    let mut response = next.run(req).await;

    // Attach X-Request-ID to response headers
    if let Ok(hdr_val) = HeaderValue::from_str(&request_id_str) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), hdr_val);
    }

    response
}

/// Create the full Axum router with registered endpoints, body limits, middleware, and fallback error handler.
pub fn create_app(state: AppState) -> axum::Router {
    let body_limit = state.config.max_body_size_bytes;
    routes::create_routes()
        .fallback(fallback_handler)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_id_and_auth_middleware,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

/// Fallback handler for unmatched routes, returning canonical 404 JSON envelopes.
async fn fallback_handler(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    method: axum::http::Method,
) -> impl axum::response::IntoResponse {
    AppError::NotFound(format!("Route '{method} {uri}' not found"))
}

/// Dispatches an in-memory HTTP route request against registered handlers.
/// Used for deterministic unit and integration verification.
pub fn dispatch_route(
    method: &str,
    path: &str,
    state: &AppState,
) -> (u16, [(&'static str, &'static str); 1], String) {
    if method == "GET" && path == "/api/v1/health" {
        let res = routes::health::HealthResponse::new();
        return (200, [("content-type", "application/json")], res.to_json());
    }

    if method == "GET" && path == "/api/v1/ready" {
        let active_ids = state.providers.active_provider_ids();
        let active_provs = active_ids.len();
        let reg_models = if active_provs > 0 {
            state
                .registry
                .list_models(None, None, None)
                .into_iter()
                .filter(|m| m.available && active_ids.contains(&m.provider_id))
                .count()
        } else {
            0
        };
        let is_ready = active_provs > 0 && reg_models > 0;
        let status_code = if is_ready { 200 } else { 503 };
        let res =
            routes::health::ReadyResponse::new_with_counts(&state.config, active_provs, reg_models);
        return (
            status_code,
            [("content-type", "application/json")],
            res.to_json(),
        );
    }

    if method == "GET" && path == "/api/v1/providers" {
        let providers = if state.providers.active_providers_count() > 0 {
            state.providers.list_metadata()
        } else {
            let mut list = state.registry.list_providers();
            for p in &mut list {
                p.status = "offline".to_string();
                p.enabled = false;
            }
            list
        };
        let json = serde_json::to_string(&routes::models::ListProvidersResponse { providers })
            .unwrap_or_default();
        return (200, [("content-type", "application/json")], json);
    }

    if method == "GET" && (path == "/api/v1/models" || path.starts_with("/api/v1/models?")) {
        let query_str = path.split_once('?').map(|(_, q)| q).unwrap_or("");
        let mut filter = registry::ModelFilter::new();
        for pair in query_str.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                match k {
                    "modality" => filter = filter.with_modality(v),
                    "speed_tier" => filter = filter.with_speed_tier(v),
                    "provider" => filter = filter.with_provider(v),
                    "available_only" => {
                        if let Ok(b) = v.parse::<bool>() {
                            filter = filter.with_available_only(b);
                        }
                    }
                    _ => {}
                }
            }
        }
        let models = state.registry.query_models(&filter);
        let json = serde_json::to_string(&routes::models::ListModelsResponse { models })
            .unwrap_or_default();
        return (200, [("content-type", "application/json")], json);
    }

    if method == "GET" && path.starts_with("/api/v1/models/") {
        let model_id = &path["/api/v1/models/".len()..];
        let model_id = model_id.split('?').next().unwrap_or(model_id);
        return match state.registry.lookup(model_id) {
            Ok(model) => {
                let json = serde_json::to_string(&model).unwrap_or_default();
                (200, [("content-type", "application/json")], json)
            }
            Err(err) => {
                let app_err: AppError = err.into();
                (
                    app_err.status_code(),
                    [("content-type", "application/json")],
                    app_err.to_json_envelope("req_test_lookup"),
                )
            }
        };
    }

    let err = AppError::NotFound(format!("Route '{method} {path}' not found"));
    (
        err.status_code(),
        [("content-type", "application/json")],
        err.to_json_envelope("req_test_unknown"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_initialization() {
        let config = Arc::new(AppConfig::default_local());
        let state = AppState::new(config.clone());
        assert_eq!(state.config.port, 8080);
        assert!(!state.registry.list_models(None, None, None).is_empty());
    }

    #[test]
    fn test_dispatch_health_endpoint() {
        let config = Arc::new(AppConfig::default_local());
        let state = AppState::new(config);
        let (status, headers, body) = dispatch_route("GET", "/api/v1/health", &state);
        assert_eq!(status, 200);
        assert_eq!(headers[0], ("content-type", "application/json"));
        assert!(body.contains(r#""status":"healthy""#));
        assert!(body.contains(r#""version":"0.1.0""#));
    }

    #[test]
    fn test_dispatch_ready_endpoint() {
        let config = Arc::new(AppConfig::default_local());
        let state = AppState::new(config);
        let (status, headers, body) = dispatch_route("GET", "/api/v1/ready", &state);
        assert_eq!(status, 503);
        assert_eq!(headers[0], ("content-type", "application/json"));
        assert!(body.contains(r#""status":"not_ready""#));
        assert!(body.contains(r#""active_providers":0"#));
        assert!(body.contains(r#""registered_models":0"#));
        assert!(body.contains(r#""deployment_mode":"daemon""#));
    }

    #[test]
    fn test_dispatch_models_endpoint() {
        let config = Arc::new(AppConfig::default_local());
        let state = AppState::new(config);
        let (status, headers, body) = dispatch_route("GET", "/api/v1/models", &state);
        assert_eq!(status, 200);
        assert_eq!(headers[0], ("content-type", "application/json"));
        assert!(body.contains("fast-chat-v1"));
        assert!(body.contains("deep-reasoner-v1"));
    }

    #[test]
    fn test_dispatch_providers_endpoint() {
        let config = Arc::new(AppConfig::default_local());
        let state = AppState::new(config);
        let (status, headers, body) = dispatch_route("GET", "/api/v1/providers", &state);
        assert_eq!(status, 200);
        assert_eq!(headers[0], ("content-type", "application/json"));
        assert!(body.contains("groq-api"));
        assert!(body.contains("openrouter-api"));
    }

    #[test]
    fn test_dispatch_unknown_route_returns_404() {
        let config = Arc::new(AppConfig::default_local());
        let state = AppState::new(config);
        let (status, headers, body) = dispatch_route("GET", "/api/v1/nonexistent", &state);
        assert_eq!(status, 404);
        assert_eq!(headers[0], ("content-type", "application/json"));
        assert!(body.contains(r#""code":"NOT_FOUND""#));
        assert!(body.contains(r#""retryable":false"#));
    }
}
