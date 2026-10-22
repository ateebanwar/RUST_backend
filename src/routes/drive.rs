//! Google Drive chat and library synchronization HTTP routes.
//!
//! Conforms to MASTER_SPEC.md §15 & Phase 15 requirements:
//! - Protected by existing authentication and authorization middleware (scope `drive`).
//! - Extracts Google Drive OAuth token from `X-Drive-Token` header.
//! - Local-first: chats and library items can be read and created offline.
//! - Drive switch toggle (`POST /api/v1/drive/config`) defaults to OFF.
//! - Bidirectional sync (`POST /api/v1/drive/sync`) with LWW conflict resolution.
//! - Independent Library lifecycle: Deleting a conversation NEVER deletes Library items.
//! - Explicit Library deletion (`DELETE /api/v1/drive/library/{id}`).
//! - Reinstall and bootstrap restore (`POST /api/v1/drive/restore`).
//! - Safe logout (`POST /api/v1/drive/logout`) clears local cache without deleting remote data.

use crate::AppState;
use crate::auth::principal::AuthPrincipal;
use crate::drive::{
    CreateConversationRequest, CreateLibraryItemRequest, DriveConfigUpdate, DriveError,
    LibraryItem, SyncConversation, SyncResult, SyncStatus, compute_sha256, decode_base64,
    sanitize_filename,
};
use crate::errors::AppError;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Helper to extract authenticated owner subject from request extension.
fn get_owner_subject(principal: &Option<Extension<AuthPrincipal>>) -> Result<String, AppError> {
    match principal {
        Some(Extension(p)) => Ok(p.subject.clone()),
        None => Ok("anonymous_local".to_string()),
    }
}

/// Helper to extract `X-Drive-Token` header.
fn get_drive_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-drive-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// GET /api/v1/drive/status — Query synchronization status and pending change counts.
pub async fn get_sync_status(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
) -> Result<(StatusCode, Json<SyncStatus>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let status = state.sync_engine.get_status(&owner).await;
    Ok((StatusCode::OK, Json(status)))
}

/// POST /api/v1/drive/config — Update Drive synchronization configuration (on/off switch).
pub async fn update_sync_config(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Json(payload): Json<DriveConfigUpdate>,
) -> Result<(StatusCode, Json<SyncStatus>), AppError> {
    let owner = get_owner_subject(&principal)?;
    state.sync_engine.set_sync_enabled(payload.enabled).await;
    let status = state.sync_engine.get_status(&owner).await;
    Ok((StatusCode::OK, Json(status)))
}

/// POST /api/v1/drive/sync — Trigger bidirectional synchronization with Google Drive.
pub async fn trigger_sync(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<SyncResult>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers).ok_or(DriveError::MissingDriveToken)?;

    let result = state.sync_engine.sync(&owner, token).await?;
    Ok((StatusCode::OK, Json(result)))
}

/// GET /api/v1/drive/conversations — List all active conversations for the authenticated user.
pub async fn list_conversations(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
) -> Result<(StatusCode, Json<Vec<SyncConversation>>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let list = state
        .sync_engine
        .local_store()
        .list_conversations(&owner)
        .await;
    Ok((StatusCode::OK, Json(list)))
}

/// POST /api/v1/drive/conversations — Create or upsert a conversation locally (marked pending sync).
pub async fn create_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Json(payload): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<SyncConversation>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let id = payload
        .id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("conv_{}", ring::rand::SystemRandom::new().generate_sub_id()));

    let mut conv = SyncConversation::new(id, owner.clone(), payload.title);
    conv.messages = payload.messages;

    let saved = state
        .sync_engine
        .local_store()
        .upsert_conversation(&owner, conv)
        .await;

    Ok((StatusCode::CREATED, Json(saved)))
}

/// GET /api/v1/drive/conversations/{id} — Retrieve a single conversation by ID.
pub async fn get_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<SyncConversation>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let conv = state
        .sync_engine
        .local_store()
        .get_conversation(&owner, &id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Conversation '{id}' not found")))?;

    Ok((StatusCode::OK, Json(conv)))
}

/// DELETE /api/v1/drive/conversations/{id} — Soft-delete a conversation (tombstone).
///
/// CRITICAL: Deleting a conversation NEVER deletes its referenced Library items!
pub async fn delete_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let deleted = state
        .sync_engine
        .local_store()
        .delete_conversation(&owner, &id)
        .await;

    if deleted {
        Ok((
            StatusCode::OK,
            Json(json!({
                "status": "deleted",
                "conversation_id": id,
                "message": "Conversation soft-deleted. Referenced Library items remain preserved."
            })),
        ))
    } else {
        Err(AppError::NotFound(format!("Conversation '{id}' not found")))
    }
}

/// GET /api/v1/drive/library — List all active library items for the authenticated user.
pub async fn list_library_items(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
) -> Result<(StatusCode, Json<Vec<LibraryItem>>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let list = state
        .sync_engine
        .local_store()
        .list_library_items(&owner)
        .await;
    Ok((StatusCode::OK, Json(list)))
}

/// POST /api/v1/drive/library — Create or upload a new library item.
pub async fn create_library_item(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Json(payload): Json<CreateLibraryItemRequest>,
) -> Result<(StatusCode, Json<LibraryItem>), AppError> {
    let owner = get_owner_subject(&principal)?;

    let clean_filename = sanitize_filename(&payload.filename);
    let raw_bytes = decode_base64(&payload.content_base64)
        .map_err(|e| AppError::InvalidRequest(format!("Invalid base64 payload: {e}")))?;

    if raw_bytes.len() > state.config.max_body_size_bytes {
        return Err(AppError::PayloadTooLarge);
    }

    let checksum = compute_sha256(&raw_bytes);
    let id = payload
        .id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("lib_{}", ring::rand::SystemRandom::new().generate_sub_id()));

    let item = LibraryItem::new(
        id,
        owner.clone(),
        clean_filename,
        payload.mime_type,
        raw_bytes.len() as u64,
        checksum,
        payload.conversation_id,
    );

    let saved = state
        .sync_engine
        .local_store()
        .save_library_item(&owner, item, Some(raw_bytes))
        .await;

    Ok((StatusCode::CREATED, Json(saved)))
}

/// GET /api/v1/drive/library/{id} — Retrieve metadata for a library item.
pub async fn get_library_item(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<LibraryItem>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let item = state
        .sync_engine
        .local_store()
        .get_library_item(&owner, &id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Library item '{id}' not found")))?;

    Ok((StatusCode::OK, Json(item)))
}

/// GET /api/v1/drive/library/{id}/content — Download raw binary content of a library item.
pub async fn get_library_item_content(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers);

    let meta = state
        .sync_engine
        .local_store()
        .get_library_item(&owner, &id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Library item '{id}' not found")))?;

    let bytes = state
        .sync_engine
        .get_library_item_content(&owner, &id, token)
        .await?;

    let content_type = HeaderValue::from_str(&meta.mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", meta.filename))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));

    let mut resp = bytes.into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    resp.headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);

    Ok(resp)
}

/// DELETE /api/v1/drive/library/{id} — Explicitly delete a library item.
pub async fn delete_library_item(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let deleted = state
        .sync_engine
        .local_store()
        .delete_library_item(&owner, &id)
        .await;

    if deleted {
        Ok((
            StatusCode::OK,
            Json(json!({
                "status": "deleted",
                "library_item_id": id,
                "message": "Library item explicitly deleted."
            })),
        ))
    } else {
        Err(AppError::NotFound(format!("Library item '{id}' not found")))
    }
}

/// POST /api/v1/drive/restore — Bootstrap restore on a new device or reinstall from remote Drive manifest.
pub async fn restore_data(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<SyncResult>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers).ok_or(DriveError::MissingDriveToken)?;

    let result = state.sync_engine.restore(&owner, token).await?;
    Ok((StatusCode::OK, Json(result)))
}

/// POST /api/v1/drive/logout — Clear local in-memory session/cache for user.
///
/// Remote Drive data is NEVER deleted.
pub async fn logout_user(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let owner = get_owner_subject(&principal)?;
    state
        .sync_engine
        .local_store()
        .clear_user_session(&owner)
        .await;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "logged_out",
            "owner": owner,
            "message": "Local user session cleared. Remote Google Drive history remains intact."
        })),
    ))
}

/// Helper trait for generating random IDs.
trait GenerateSubId {
    fn generate_sub_id(&self) -> String;
}

impl GenerateSubId for ring::rand::SystemRandom {
    fn generate_sub_id(&self) -> String {
        use ring::rand::SecureRandom;
        let mut bytes = [0u8; 16];
        self.fill(&mut bytes).unwrap_or_default();
        crate::drive::crypto::encode_hex(&bytes)
    }
}
