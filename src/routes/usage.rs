//! Read-only HTTP endpoints for usage analytics and provider quota inspection.
//!
//! Conforms strictly to Phase 8 specifications:
//! - Protected by existing API key middleware.
//! - Validated and bounded query parameters.
//! - Zero secrets or prompt/completion content exposed.

use crate::AppState;
use crate::quota::ProviderQuota;
use crate::usage::model::{
    ModelUsageSummary, ProviderUsageSummary, UsageQueryFilter, UsageSummary,
};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

/// Inbound query filter parameters for usage endpoints.
#[derive(Debug, Deserialize)]
pub struct UsageQueryParams {
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub limit: Option<usize>,
}

impl From<UsageQueryParams> for UsageQueryFilter {
    fn from(p: UsageQueryParams) -> Self {
        Self {
            start_time: p.start_time,
            end_time: p.end_time,
            provider_id: p.provider,
            model_id: p.model,
            outcome: None,
            limit: p.limit.map(|l| l.min(1000)),
        }
    }
}

/// Handler for GET /api/v1/usage/summary
#[allow(clippy::result_large_err)]
pub async fn get_usage_summary(
    State(state): State<AppState>,
    Query(params): Query<UsageQueryParams>,
) -> Result<(StatusCode, Json<UsageSummary>), Response> {
    let filter = params.into();
    let summary = state
        .usage
        .get_summary(filter)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    Ok((StatusCode::OK, Json(summary)))
}

/// Handler for GET /api/v1/usage/providers
#[allow(clippy::result_large_err)]
pub async fn get_usage_providers(
    State(state): State<AppState>,
    Query(params): Query<UsageQueryParams>,
) -> Result<(StatusCode, Json<Vec<ProviderUsageSummary>>), Response> {
    let filter = params.into();
    let providers = state
        .usage
        .get_providers(filter)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    Ok((StatusCode::OK, Json(providers)))
}

/// Handler for GET /api/v1/usage/models
#[allow(clippy::result_large_err)]
pub async fn get_usage_models(
    State(state): State<AppState>,
    Query(params): Query<UsageQueryParams>,
) -> Result<(StatusCode, Json<Vec<ModelUsageSummary>>), Response> {
    let filter = params.into();
    let models = state
        .usage
        .get_models(filter)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    Ok((StatusCode::OK, Json(models)))
}

/// Handler for GET /api/v1/quotas
pub async fn get_quotas(State(state): State<AppState>) -> (StatusCode, Json<Vec<ProviderQuota>>) {
    let quotas = state.quota.get_quotas();
    (StatusCode::OK, Json(quotas))
}
