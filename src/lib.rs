//! ai-backend core library
//!
//! A high-performance, security-focused AI model aggregation backend in Rust.

pub mod auth;
pub mod config;
pub mod drive;
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
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;

pub use auth::{
    AuthPrincipal, AuthRole, AuthService, AuthType, ClientType, GoogleClaims, GoogleOidcValidator,
    HttpJwksProvider, JwksProvider, MockJwksProvider, PatValidator, auth_middleware,
};
pub use auth::{CredentialMetadata, CredentialStore};
pub use config::AppConfig;
pub use drive::{DriveClient, DriveError, GoogleDriveClient, MockDriveClient, SyncEngine};
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

/// Wrapper for tracing request IDs across extensions and handlers.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Shared application state accessible across HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub auth: Arc<AuthService>,
    pub credentials: Arc<crate::auth::CredentialStore>,
    pub registry: Arc<ModelRegistry>,
    pub providers: Arc<ProviderRegistry>,
    pub router: Arc<ModelRouter>,
    pub inference: Arc<InferenceService>,
    pub usage: Arc<UsageService>,
    pub quota: Arc<QuotaService>,
    pub rate_limiter: Arc<crate::security::RateLimiter>,
    pub destination_policy: Arc<crate::security::OutboundDestinationPolicy>,
    pub sync_engine: Arc<crate::drive::SyncEngine>,
    pub start_time: std::time::Instant,
}

fn default_sync_engine(config: &AppConfig) -> Arc<crate::drive::SyncEngine> {
    let drive_crypto = if let Some(hex_key) = &config.drive_encryption_key {
        Arc::new(
            crate::drive::DriveCryptoEngine::from_hex(hex_key).unwrap_or_else(|_| {
                let master = [0x5au8; 32];
                crate::drive::DriveCryptoEngine::derive_for_owner(&master, "ai-backend-default")
            }),
        )
    } else {
        let master = [0x5au8; 32];
        Arc::new(crate::drive::DriveCryptoEngine::derive_for_owner(
            &master,
            "ai-backend-default",
        ))
    };
    let drive_client = Arc::new(crate::drive::GoogleDriveClient::new(
        config.google_drive_timeout_secs,
        config.google_drive_max_retries,
    ));
    let local_sync_store = Arc::new(crate::drive::LocalSyncStore::new());
    Arc::new(crate::drive::SyncEngine::new(
        drive_client,
        drive_crypto,
        local_sync_store,
        config.google_drive_sync_enabled,
    ))
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
        let usage_spool = Arc::new(crate::usage::UsageSpooler::new(
            &config.usage_spool_path,
            config.usage_max_spool_bytes,
        ));
        let usage = Arc::new(UsageService::with_spool(usage_storage.clone(), usage_spool));

        let credentials = Arc::new(
            crate::auth::CredentialStore::new(usage_storage.conn())
                .expect("Failed to initialize CredentialStore"),
        );
        let auth =
            Arc::new(AuthService::new(config.clone()).with_credential_store(credentials.clone()));

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

        let rate_limiter = Arc::new(crate::security::RateLimiter::new(
            config.rate_limiter_config(),
        ));
        let destination_policy = Arc::new(config.outbound_destination_policy());

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
                .with_destination_policy(destination_policy.clone())
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
                .with_destination_policy(destination_policy.clone())
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
            config: config.clone(),
            auth,
            credentials,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            rate_limiter,
            destination_policy,
            sync_engine: default_sync_engine(&config),
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
        let usage = Arc::new(UsageService::new(storage.clone()));
        let credentials = Arc::new(
            crate::auth::CredentialStore::new(storage.conn())
                .expect("In-memory CredentialStore failed"),
        );
        let auth =
            Arc::new(AuthService::new(config.clone()).with_credential_store(credentials.clone()));
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );
        let rate_limiter = Arc::new(crate::security::RateLimiter::new(
            config.rate_limiter_config(),
        ));
        let destination_policy = Arc::new(config.outbound_destination_policy());

        Self {
            config: config.clone(),
            auth,
            credentials,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            rate_limiter,
            destination_policy,
            sync_engine: default_sync_engine(&config),
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
        let usage = Arc::new(UsageService::new(storage.clone()));
        let credentials = Arc::new(
            crate::auth::CredentialStore::new(storage.conn())
                .expect("In-memory CredentialStore failed"),
        );
        let auth =
            Arc::new(AuthService::new(config.clone()).with_credential_store(credentials.clone()));
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );
        let rate_limiter = Arc::new(crate::security::RateLimiter::new(
            config.rate_limiter_config(),
        ));
        let destination_policy = Arc::new(config.outbound_destination_policy());

        Self {
            config: config.clone(),
            auth,
            credentials,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            rate_limiter,
            destination_policy,
            sync_engine: default_sync_engine(&config),
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
        let credentials = Arc::new(
            crate::auth::CredentialStore::new(usage.storage().conn())
                .expect("Failed to initialize CredentialStore"),
        );
        let auth =
            Arc::new(AuthService::new(config.clone()).with_credential_store(credentials.clone()));
        let inference = Arc::new(
            InferenceService::new(
                config.clone(),
                registry.clone(),
                providers.clone(),
                router.clone(),
            )
            .with_quota_service(quota.clone()),
        );
        let rate_limiter = Arc::new(crate::security::RateLimiter::new(
            config.rate_limiter_config(),
        ));
        let destination_policy = Arc::new(config.outbound_destination_policy());

        Self {
            config: config.clone(),
            auth,
            credentials,
            registry,
            providers,
            router,
            inference,
            usage,
            quota,
            rate_limiter,
            destination_policy,
            sync_engine: default_sync_engine(&config),
            start_time: std::time::Instant::now(),
        }
    }

    /// Builder helper to replace AuthService (e.g. for deterministic mock JWKS in tests).
    pub fn with_auth_service(mut self, auth: Arc<AuthService>) -> Self {
        self.auth = auth;
        self
    }

    /// Builder helper to replace CredentialStore.
    pub fn with_credentials(mut self, credentials: Arc<crate::auth::CredentialStore>) -> Self {
        self.credentials = credentials;
        self
    }

    /// Builder helper to replace RateLimiter.
    pub fn with_rate_limiter(mut self, rate_limiter: Arc<crate::security::RateLimiter>) -> Self {
        self.rate_limiter = rate_limiter;
        self
    }

    /// Builder helper to replace OutboundDestinationPolicy.
    pub fn with_destination_policy(
        mut self,
        destination_policy: Arc<crate::security::OutboundDestinationPolicy>,
    ) -> Self {
        self.destination_policy = destination_policy;
        self
    }

    /// Builder helper to replace SyncEngine (e.g. for deterministic mock Drive in tests).
    pub fn with_sync_engine(mut self, sync_engine: Arc<crate::drive::SyncEngine>) -> Self {
        self.sync_engine = sync_engine;
        self
    }
}

/// Spawns the autonomous background retention and retry maintenance task.
/// Returns a Tokio JoinHandle that completes when graceful shutdown is signaled.
pub fn start_retention_maintenance(
    state: Arc<AppState>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let worker = crate::usage::RetentionMaintenanceWorker::new(
        state.usage.clone(),
        state.config.usage_cleanup_interval_secs,
        state.config.usage_retention_days,
        state.config.usage_cleanup_batch_size,
    );
    tokio::spawn(async move {
        worker.run_loop(shutdown_rx).await;
    })
}

/// Request ID resolution, Host validation, proxy security, and API Authentication middleware.
pub async fn request_id_and_auth_middleware(
    state: State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    crate::auth::auth_middleware(state, req, next).await
}

/// Create the full Axum router with registered endpoints, body limits, middleware, and fallback error handler.
pub fn create_app(state: AppState) -> axum::Router {
    let body_limit = state.config.max_body_size_bytes;
    routes::create_routes()
        .fallback(fallback_handler)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::auth_middleware,
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
