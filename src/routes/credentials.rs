//! Personal Access Token and Credential Lifecycle HTTP handlers.
//!
//! Conforms to Phase 12 Secret Management and Resource Ownership:
//! - Protected by existing authentication and authorization middleware (scope `auth:tokens`).
//! - Plaintext tokens returned ONCE only upon creation and rotation.
//! - List and lookup operations strictly return masked credentials (`pat_1234...ef01`).
//! - Enforces strict tenant isolation and IDOR defenses:
//!   Users can only list, view, rotate, or revoke their own credentials.
//!   Administrators can manage credentials across tenants.

use crate::AppState;
use crate::auth::principal::AuthPrincipal;
use crate::auth::{
    CreateTokenRequest, CreateTokenResponse, CredentialMetadata, RotateTokenResponse,
};
use crate::errors::AppError;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

/// Query parameters for listing credentials.
#[derive(Debug, Deserialize)]
pub struct ListTokensQuery {
    pub owner: Option<String>,
}

/// Handler for POST /api/v1/auth/tokens (Generate new Personal Access Token)
pub async fn create_token(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Json(payload): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<CreateTokenResponse>), AppError> {
    let p = principal.ok_or_else(|| {
        AppError::Unauthorized("Authentication required to manage credentials".to_string())
    })?;

    let response = state.credentials.create_token(&p.0, payload)?;
    Ok((StatusCode::CREATED, Json(response)))
}

/// Handler for GET /api/v1/auth/tokens (List credentials with tenant isolation)
pub async fn list_tokens(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Query(query): Query<ListTokensQuery>,
) -> Result<(StatusCode, Json<Vec<CredentialMetadata>>), AppError> {
    let p = principal.ok_or_else(|| {
        AppError::Unauthorized("Authentication required to manage credentials".to_string())
    })?;

    let list = state
        .credentials
        .list_tokens(&p.0, query.owner.as_deref())?;
    Ok((StatusCode::OK, Json(list)))
}

/// Handler for GET /api/v1/auth/tokens/{id} (Inspect credential metadata)
pub async fn get_token(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<CredentialMetadata>), AppError> {
    let p = principal.ok_or_else(|| {
        AppError::Unauthorized("Authentication required to manage credentials".to_string())
    })?;

    let meta = state.credentials.get_token(&p.0, &id)?;
    Ok((StatusCode::OK, Json(meta)))
}

/// Handler for POST /api/v1/auth/tokens/{id}/rotate (Rotate credential)
pub async fn rotate_token(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<RotateTokenResponse>), AppError> {
    let p = principal.ok_or_else(|| {
        AppError::Unauthorized("Authentication required to manage credentials".to_string())
    })?;

    let response = state.credentials.rotate_token(&p.0, &id)?;
    Ok((StatusCode::OK, Json(response)))
}

/// Handler for DELETE /api/v1/auth/tokens/{id} (Revoke credential)
pub async fn revoke_token(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let p = principal.ok_or_else(|| {
        AppError::Unauthorized("Authentication required to manage credentials".to_string())
    })?;

    state.credentials.revoke_token(&p.0, &id)?;
    Ok(StatusCode::NO_CONTENT)
}
