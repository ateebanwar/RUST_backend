//! Read-only HTTP endpoints for usage analytics and provider quota inspection.
//!
//! Conforms strictly to Phase 8 specifications and Phase 12 Tenant Isolation:
//! - Protected by existing authentication and authorization middleware.
//! - Validated and bounded query parameters.
//! - Enforces strict tenant isolation and IDOR prevention: end-users may only
//!   inspect telemetry belonging to their authenticated subject.
//! - Administrators and services may query global aggregates or filter by subject.
//! - Zero secrets or prompt/completion content exposed.

use crate::auth::principal::{AuthPrincipal, AuthRole};
use crate::errors::AppError;
use crate::quota::ProviderQuota;
use crate::usage::model::{
    ModelUsageSummary, ProviderUsageSummary, UsageQueryFilter, UsageSummary,
};
use crate::{AppState, RequestId};
use axum::Json;
use axum::extract::{Extension, Query, State};
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
    pub subject: Option<String>,
    pub user_id: Option<String>,
}

/// Enforces resource ownership, tenant isolation, and IDOR prevention for usage queries.
fn resolve_authorized_filter(
    params: UsageQueryParams,
    principal: Option<&AuthPrincipal>,
) -> Result<UsageQueryFilter, AppError> {
    let mut filter = UsageQueryFilter {
        start_time: params.start_time,
        end_time: params.end_time,
        provider_id: params.provider,
        model_id: params.model,
        outcome: None,
        limit: params.limit.map(|l| l.min(1000)),
        subject: None,
    };

    let requested_subject = params.subject.or(params.user_id);

    match principal {
        Some(p) => {
            if p.is_admin() || p.has_role(&AuthRole::Service) {
                // Admin or Service callers may view global aggregates or filter by specific subject
                filter.subject = requested_subject;
            } else {
                // Standard User callers are strictly locked to their own subject identity
                if let Some(ref target) = requested_subject
                    && target != &p.subject
                {
                    return Err(AppError::Forbidden(format!(
                        "IDOR violation: Principal '{}' is forbidden from querying telemetry for subject '{target}'",
                        p.subject
                    )));
                }
                filter.subject = Some(p.subject.clone());
            }
        }
        None => {
            // Unauthenticated development bypass on loopback
            filter.subject = requested_subject.or_else(|| Some("dev_local_user".to_string()));
        }
    }

    Ok(filter)
}

/// Handler for GET /api/v1/usage/summary
#[allow(clippy::result_large_err)]
pub async fn get_usage_summary(
    State(state): State<AppState>,
    Extension(RequestId(req_id)): Extension<RequestId>,
    principal: Option<Extension<AuthPrincipal>>,
    Query(params): Query<UsageQueryParams>,
) -> Result<(StatusCode, Json<UsageSummary>), Response> {
    let filter = resolve_authorized_filter(params, principal.as_ref().map(|p| &p.0))
        .map_err(|e| e.to_response_with_id(&req_id))?;

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
    Extension(RequestId(req_id)): Extension<RequestId>,
    principal: Option<Extension<AuthPrincipal>>,
    Query(params): Query<UsageQueryParams>,
) -> Result<(StatusCode, Json<Vec<ProviderUsageSummary>>), Response> {
    let filter = resolve_authorized_filter(params, principal.as_ref().map(|p| &p.0))
        .map_err(|e| e.to_response_with_id(&req_id))?;

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
    Extension(RequestId(req_id)): Extension<RequestId>,
    principal: Option<Extension<AuthPrincipal>>,
    Query(params): Query<UsageQueryParams>,
) -> Result<(StatusCode, Json<Vec<ModelUsageSummary>>), Response> {
    let filter = resolve_authorized_filter(params, principal.as_ref().map(|p| &p.0))
        .map_err(|e| e.to_response_with_id(&req_id))?;

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
