//! Routing preview HTTP handler.
//!
//! Conforms to API_SPEC.md §4.

use crate::AppState;
use crate::errors::AppError;
use crate::router::{RoutingPreviewRequest, RoutingPreviewResponse};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

/// Handler for POST /api/v1/routing/preview (Dry run)
pub async fn preview_routing(
    State(state): State<AppState>,
    Json(payload): Json<RoutingPreviewRequest>,
) -> Result<(StatusCode, Json<RoutingPreviewResponse>), AppError> {
    let response = state.router.preview(&payload)?;
    Ok((StatusCode::OK, Json(response)))
}
