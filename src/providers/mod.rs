//! Provider adapter abstraction framework, lifecycle management, and mock implementation.
//!
//! Conforms strictly to PROVIDER_INTEGRATION.md §2 and ARCHITECTURE.md §2.

pub mod mock;
pub mod openai_compatible;
pub mod registry;

pub use mock::MockProviderAdapter;
pub use openai_compatible::OpenAiCompatibleAdapter;
pub use registry::ProviderRegistry;

use crate::errors::AppError;
use crate::models::{
    CompletionRequest, CompletionResponse, ModelMetadata, ProviderMetadata, StreamChunk,
};
use futures_util::stream::BoxStream;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Normalized provider error enum matching PROVIDER_INTEGRATION.md §2 and ARCHITECTURE.md §2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    AuthenticationFailed(String),
    RateLimited { retry_after_secs: Option<u64> },
    QuotaExhausted(String),
    Timeout(Duration),
    NetworkFailure(String),
    InvalidRequest(String),
    UpstreamError { status_code: u16, message: String },
    ModelNotFound(String),
    CapabilityMismatch(String),
    ProviderNotFound(String),
    ProviderDisabled(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthenticationFailed(msg) => write!(f, "Provider authentication failed: {msg}"),
            Self::RateLimited { retry_after_secs } => {
                write!(
                    f,
                    "Provider rate limit hit. Retry after: {retry_after_secs:?}s"
                )
            }
            Self::QuotaExhausted(p) => write!(f, "Quota exhausted for provider: {p}"),
            Self::Timeout(t) => write!(f, "Provider request timed out after {t:?}"),
            Self::NetworkFailure(msg) => write!(f, "Network failure reaching provider: {msg}"),
            Self::InvalidRequest(msg) => write!(f, "Invalid payload for provider: {msg}"),
            Self::UpstreamError {
                status_code,
                message,
            } => {
                write!(f, "Upstream provider error ({status_code}): {message}")
            }
            Self::ModelNotFound(m) => write!(f, "Model not found on provider: {m}"),
            Self::CapabilityMismatch(c) => write!(f, "Capability mismatch on provider: {c}"),
            Self::ProviderNotFound(p) => write!(f, "Provider '{p}' not found in registry"),
            Self::ProviderDisabled(p) => write!(f, "Provider '{p}' is disabled"),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<ProviderError> for AppError {
    fn from(err: ProviderError) -> Self {
        match err {
            ProviderError::AuthenticationFailed(_) => {
                // Secret isolation: sanitize error message so upstream keys/secrets are never returned
                AppError::UpstreamFailure("Upstream provider authentication failed".to_string())
            }
            ProviderError::RateLimited { retry_after_secs } => AppError::RateLimited {
                message: "Upstream AI provider rate limit exceeded".to_string(),
                retry_after_secs,
            },
            ProviderError::QuotaExhausted(provider) => AppError::RateLimited {
                message: format!("Upstream quota exhausted for provider '{provider}'"),
                retry_after_secs: None,
            },
            ProviderError::Timeout(duration) => AppError::GatewayTimeout(format!(
                "Upstream provider request timed out after {duration:?}"
            )),
            ProviderError::NetworkFailure(msg) => {
                AppError::UpstreamFailure(format!("Network failure connecting to provider: {msg}"))
            }
            ProviderError::InvalidRequest(msg) => AppError::InvalidRequest(msg),
            ProviderError::UpstreamError {
                status_code,
                message,
            } => AppError::UpstreamFailure(format!(
                "Upstream provider returned status {status_code}: {message}"
            )),
            ProviderError::ModelNotFound(model) => {
                AppError::NotFound(format!("Model '{model}' not found on provider"))
            }
            ProviderError::CapabilityMismatch(cap) => AppError::UnsupportedModality(format!(
                "Requested capability '{cap}' is unsupported"
            )),
            ProviderError::ProviderNotFound(provider) => {
                AppError::NotFound(format!("Provider '{provider}' not found in registry"))
            }
            ProviderError::ProviderDisabled(provider) => {
                AppError::InvalidRequest(format!("Provider '{provider}' is disabled"))
            }
        }
    }
}

/// Boxed stream of streaming chunks or provider errors.
pub type ProviderStream = BoxStream<'static, Result<StreamChunk, ProviderError>>;

/// Boxed future yielding discovered models or provider error.
pub type BoxedModelDiscoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<ModelMetadata>, ProviderError>> + Send + 'a>>;

/// Boxed future yielding a completion response or provider error.
pub type BoxedCompletionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CompletionResponse, ProviderError>> + Send + 'a>>;

/// Boxed future yielding a streaming response or provider error.
pub type BoxedStreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderStream, ProviderError>> + Send + 'a>>;

/// Boxed future yielding a health check result or provider error.
pub type BoxedHealthFuture<'a> =
    Pin<Box<dyn Future<Output = Result<bool, ProviderError>> + Send + 'a>>;

/// Primary asynchronous trait implemented by all provider adapters.
pub trait ProviderAdapter: Send + Sync {
    /// Return static identification metadata for this provider.
    fn provider_info(&self) -> ProviderMetadata;

    /// Return declared capabilities of this provider.
    fn capabilities(&self) -> Vec<String> {
        vec!["simple_chat".to_string()]
    }

    /// Check whether this adapter supports a given capability.
    fn supports_capability(&self, capability: &str) -> bool {
        self.capabilities()
            .iter()
            .any(|c| c.eq_ignore_ascii_case(capability))
    }

    /// Discover or return statically registered models supported by this adapter.
    fn discover_models(&self) -> BoxedModelDiscoveryFuture<'_>;

    /// Execute a standard buffered chat completion.
    fn complete<'a>(&'a self, req: &'a CompletionRequest) -> BoxedCompletionFuture<'a>;

    /// Execute a streaming chat completion yielding incremental chunks.
    fn stream<'a>(&'a self, req: &'a CompletionRequest) -> BoxedStreamFuture<'a>;

    /// Check health and operational reachability of the provider endpoint.
    fn check_health(&self) -> BoxedHealthFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ChatMessage;
    use futures_util::StreamExt;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_mock_adapter_completion() {
        let adapter = MockProviderAdapter::new("test-provider", "Test Provider");
        let req = CompletionRequest::new(
            "test-provider-model-1",
            vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
        );

        let res = adapter.complete(&req).await.unwrap();
        assert_eq!(res.model_used, "test-provider-model-1");
        assert_eq!(res.provider_used, "test-provider");
        assert!(res.message.content.contains("Mock completion"));
        assert_eq!(res.usage.unwrap().total_tokens, 60);
    }

    #[tokio::test]
    async fn test_mock_adapter_streaming() {
        let adapter = MockProviderAdapter::new("stream-provider", "Streaming Provider");
        let req = CompletionRequest::new("stream-provider-model-1", vec![]);

        let mut stream = adapter.stream(&req).await.unwrap();
        let mut deltas = Vec::new();
        let mut final_done = false;

        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.unwrap();
            if chunk.is_done {
                final_done = true;
                assert!(chunk.usage.is_some());
            } else {
                deltas.push(chunk.delta);
            }
        }

        assert!(final_done);
        assert_eq!(deltas, vec!["Mock ", "streaming ", "response"]);
    }

    #[tokio::test]
    async fn test_mock_adapter_failure_simulation() {
        let adapter = MockProviderAdapter::new("test-fail", "Failing Provider")
            .with_simulated_error(ProviderError::RateLimited {
                retry_after_secs: Some(10),
            });

        let req = CompletionRequest::new("test-fail-model-1", vec![]);

        let err = adapter.complete(&req).await.unwrap_err();
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_secs: Some(10)
            }
        );
    }

    #[tokio::test]
    async fn test_mock_adapter_timeout_handling() {
        let adapter = MockProviderAdapter::new("slow-provider", "Slow Provider")
            .with_delay(Duration::from_millis(50));

        let mut req = CompletionRequest::new("slow-provider-model-1", vec![]);
        req.timeout = Duration::from_millis(10); // Shorter than delay

        let err = adapter.complete(&req).await.unwrap_err();
        assert_eq!(err, ProviderError::Timeout(Duration::from_millis(10)));
    }

    #[tokio::test]
    async fn test_mock_adapter_capability_mismatch() {
        let adapter = MockProviderAdapter::new("text-only", "Text Only Provider")
            .with_capabilities(vec!["simple_chat".to_string()]); // No "streaming"

        let req = CompletionRequest::new("text-only-model-1", vec![]);
        match adapter.stream(&req).await {
            Err(err) => {
                assert_eq!(
                    err,
                    ProviderError::CapabilityMismatch("streaming".to_string())
                );
            }
            Ok(_) => panic!("Expected CapabilityMismatch error"),
        }
    }

    #[tokio::test]
    async fn test_mock_adapter_model_not_found() {
        let adapter = MockProviderAdapter::new("test-p", "Test Provider");
        let req = CompletionRequest::new("nonexistent-model", vec![]);
        let err = adapter.complete(&req).await.unwrap_err();
        assert_eq!(
            err,
            ProviderError::ModelNotFound("nonexistent-model".to_string())
        );
    }

    #[tokio::test]
    async fn test_mock_adapter_health_check() {
        let healthy = MockProviderAdapter::new("healthy-p", "Healthy Provider");
        assert!(healthy.check_health().await.unwrap());

        let unhealthy =
            MockProviderAdapter::new("unhealthy-p", "Unhealthy Provider").with_health(false);
        assert!(!unhealthy.check_health().await.unwrap());

        let err_provider = MockProviderAdapter::new("err-p", "Error Provider")
            .with_simulated_error(ProviderError::NetworkFailure(
                "connection refused".to_string(),
            ));
        assert!(err_provider.check_health().await.is_err());
    }

    #[test]
    fn test_provider_error_to_app_error_mapping() {
        let err = ProviderError::RateLimited {
            retry_after_secs: Some(5),
        };
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 429);
        assert_eq!(app_err.error_code(), "RATE_LIMITED");

        let err = ProviderError::Timeout(Duration::from_secs(10));
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 504);
        assert_eq!(app_err.error_code(), "GATEWAY_TIMEOUT");

        let err = ProviderError::AuthenticationFailed("leaked_key_123".to_string());
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 502);
        // Ensure secret isolation: leaked_key_123 must NOT appear in the user-facing message
        assert!(!app_err.client_message().contains("leaked_key_123"));

        let err = ProviderError::CapabilityMismatch("vision".to_string());
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 422);

        let err = ProviderError::ProviderNotFound("unknown-prov".to_string());
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 404);

        let err = ProviderError::ProviderDisabled("disabled-prov".to_string());
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 400);
    }

    #[tokio::test]
    async fn test_provider_registry_lifecycle() {
        let reg = ProviderRegistry::new();
        assert_eq!(reg.active_providers_count(), 0);

        let adapter = Arc::new(MockProviderAdapter::new("p1", "Provider 1"));
        reg.register(adapter).unwrap();

        assert!(reg.has_provider("p1"));
        assert_eq!(reg.active_providers_count(), 1);

        let retrieved = reg.get("p1").unwrap();
        assert_eq!(retrieved.provider_info().provider_id, "p1");

        // Disabled provider check
        let disabled_adapter =
            Arc::new(MockProviderAdapter::new("p2", "Provider 2").with_enabled(false));
        reg.register(disabled_adapter).unwrap();
        assert!(reg.has_provider("p2"));
        assert_eq!(reg.active_providers_count(), 1); // p2 is disabled so active count remains 1

        match reg.get("p2") {
            Err(err) => assert_eq!(err, ProviderError::ProviderDisabled("p2".to_string())),
            Ok(_) => panic!("Expected ProviderDisabled error"),
        }

        // Unknown provider
        match reg.get("nonexistent") {
            Err(err) => assert_eq!(
                err,
                ProviderError::ProviderNotFound("nonexistent".to_string())
            ),
            Ok(_) => panic!("Expected ProviderNotFound error"),
        }

        // Deregister
        assert!(reg.deregister("p1").is_some());
        assert!(!reg.has_provider("p1"));
        assert_eq!(reg.active_providers_count(), 0);
    }

    #[tokio::test]
    async fn test_provider_registry_health_checks() {
        let reg = ProviderRegistry::new();
        let adapter1 = Arc::new(MockProviderAdapter::new("h1", "Health 1").with_health(true));
        let adapter2 = Arc::new(MockProviderAdapter::new("h2", "Health 2").with_health(false));

        reg.register(adapter1).unwrap();
        reg.register(adapter2).unwrap();

        let h1_res = reg.check_provider_health("h1").await.unwrap();
        assert!(h1_res);

        let h2_res = reg.check_provider_health("h2").await.unwrap();
        assert!(!h2_res);

        let all = reg.check_all_health().await;
        assert_eq!(all.len(), 2);
        assert!(all.get("h1").unwrap().as_ref().unwrap());
        assert!(!all.get("h2").unwrap().as_ref().unwrap());
    }

    #[tokio::test]
    async fn test_provider_registry_sync_models() {
        let provider_reg = ProviderRegistry::new();
        let model_reg = crate::registry::ModelRegistry::new();

        let adapter = Arc::new(MockProviderAdapter::new("sync-p", "Sync Provider"));
        provider_reg.register(adapter).unwrap();

        let count = provider_reg
            .sync_models_to_registry(&model_reg)
            .await
            .unwrap();
        assert_eq!(count, 1);

        let model = model_reg.get_model("sync-p-model-1");
        assert!(model.is_some());
        assert_eq!(model.unwrap().provider_id, "sync-p");
    }
}
