//! Route module registrations and Axum router assembly.

pub mod chat;
pub mod credentials;
pub mod drive;
pub mod health;
pub mod models;
pub mod routing;
pub mod usage;

use crate::AppState;
use axum::Router;
use axum::routing::{get, post};

/// Register all foundational, model discovery, chat completion, usage/quota, credential, and Drive sync routes on the Axum Router.
pub fn create_routes() -> Router<AppState> {
    Router::new()
        // Health & System Discovery (API_SPEC.md §2)
        .route("/api/v1/health", get(health::health_check))
        .route("/api/v1/ready", get(health::readiness_check))
        // Model Registry & Discovery (API_SPEC.md §3)
        .route("/api/v1/models", get(models::list_models))
        .route("/api/v1/models/{model_id}", get(models::get_model))
        .route("/api/v1/providers", get(models::list_providers))
        // Task Classification & Routing (API_SPEC.md §4)
        .route("/api/v1/routing/preview", post(routing::preview_routing))
        // Chat Completions & Streaming (API_SPEC.md §5 & §6)
        .route("/api/v1/chat/completions", post(chat::chat_completions))
        .route("/api/v1/chat/stream", post(chat::chat_stream))
        // Quota & Usage Tracking (Phase 8)
        .route("/api/v1/usage/summary", get(usage::get_usage_summary))
        .route("/api/v1/usage/providers", get(usage::get_usage_providers))
        .route("/api/v1/usage/models", get(usage::get_usage_models))
        .route("/api/v1/quotas", get(usage::get_quotas))
        // Dynamic Credential & PAT Lifecycle (Phase 12)
        .route(
            "/api/v1/auth/tokens",
            get(credentials::list_tokens).post(credentials::create_token),
        )
        .route(
            "/api/v1/auth/tokens/{id}",
            get(credentials::get_token).delete(credentials::revoke_token),
        )
        .route(
            "/api/v1/auth/tokens/{id}/rotate",
            post(credentials::rotate_token),
        )
        // Google Drive Synchronization & Library Management (Phase 15)
        .route("/api/v1/drive/status", get(drive::get_sync_status))
        .route("/api/v1/drive/config", post(drive::update_sync_config))
        .route("/api/v1/drive/sync", post(drive::trigger_sync))
        .route(
            "/api/v1/drive/conversations",
            get(drive::list_conversations).post(drive::create_conversation),
        )
        .route(
            "/api/v1/drive/conversations/{id}",
            get(drive::get_conversation).delete(drive::delete_conversation),
        )
        .route(
            "/api/v1/drive/library",
            get(drive::list_library_items).post(drive::create_library_item),
        )
        .route(
            "/api/v1/drive/library/{id}",
            get(drive::get_library_item).delete(drive::delete_library_item),
        )
        .route(
            "/api/v1/drive/library/{id}/content",
            get(drive::get_library_item_content),
        )
        .route("/api/v1/drive/restore", post(drive::restore_data))
        .route("/api/v1/drive/logout", post(drive::logout_user))
}
