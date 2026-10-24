//! Model discovery and catalog HTTP handlers.
//!
//! Conforms to API_SPEC.md §3.

use crate::AppState;
use crate::errors::AppError;
use crate::models::{ModelMetadata, ProviderMetadata};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

/// Query parameters for GET /api/v1/models.
#[derive(Debug, Deserialize)]
pub struct ListModelsParams {
    pub modality: Option<String>,
    pub speed_tier: Option<String>,
    pub provider: Option<String>,
    pub available_only: Option<bool>,
}

/// Individual model entry in GET /api/v1/models response, enriching ModelMetadata with gateway info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalogEntry {
    #[serde(flatten)]
    pub metadata: ModelMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
}

impl From<ModelMetadata> for ModelCatalogEntry {
    fn from(m: ModelMetadata) -> Self {
        let gateway = m.gateway().map(|s| s.to_string());
        Self {
            metadata: m,
            gateway,
        }
    }
}

/// Outbound JSON response schema for GET /api/v1/models.
#[derive(Debug, Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelCatalogEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

/// Outbound JSON response schema for GET /api/v1/providers.
#[derive(Debug, Serialize)]
pub struct ListProvidersResponse {
    pub providers: Vec<ProviderMetadata>,
}

/// Handler for GET /api/v1/models
pub async fn list_models(
    State(state): State<AppState>,
    Query(params): Query<ListModelsParams>,
) -> (StatusCode, Json<ListModelsResponse>) {
    let filter = crate::registry::ModelFilter {
        modality: params.modality,
        speed_tier: params.speed_tier,
        provider: params.provider,
        available_only: params.available_only,
    };
    let models = state.registry.query_models(&filter);
    let catalog_models: Vec<ModelCatalogEntry> = models.into_iter().map(Into::into).collect();
    (
        StatusCode::OK,
        Json(ListModelsResponse {
            models: catalog_models,
            default_model: state.config.default_model.clone(),
        }),
    )
}

/// Handler for GET /api/v1/models/{model_id}
pub async fn get_model(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<(StatusCode, Json<ModelMetadata>), AppError> {
    match state.registry.lookup(&model_id) {
        Ok(model) => Ok((StatusCode::OK, Json(model))),
        Err(err) => Err(err.into()),
    }
}

/// Handler for GET /api/v1/providers
pub async fn list_providers(
    State(state): State<AppState>,
) -> (StatusCode, Json<ListProvidersResponse>) {
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
    (StatusCode::OK, Json(ListProvidersResponse { providers }))
}
