//! Google Drive chat and library synchronization HTTP routes.
//!
//! Conforms to MASTER_SPEC.md §15 and Master Directive:
//! - Google Drive is the SOLE permanent storage for user data.
//! - The backend is a stateless processing layer.
//! - Both Google authentication (`AuthPrincipal`) and Drive authorization (`X-Drive-Token`)
//!   are strictly mandatory (fail-closed, no anonymous local fallback).
//! - Save-Confirmation: 201 Created / 200 OK is returned ONLY after the Google Drive operation
//!   and manifest commit succeed and are verified.
//! - Cold-start hydration: Conversations and Library items are fetched directly from Google Drive
//!   when the ephemeral in-memory working cache is cold.
//! - Independent Library lifecycle: Deleting a conversation NEVER deletes Library items.
//! - Reinstall and bootstrap restore (`POST /api/v1/drive/restore`).
//! - Safe logout (`POST /api/v1/drive/logout`) clears ephemeral in-memory cache without affecting Drive.

use crate::AppState;
use crate::auth::principal::AuthPrincipal;
use crate::drive::{
    CreateConversationRequest, CreateLibraryItemRequest, DriveConfigUpdate, LibraryItem,
    SyncConversation, SyncResult, SyncStatus, UpdateConversationRequest, compute_sha256,
    decode_base64, sanitize_filename,
};
use crate::errors::AppError;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Validate resource ID format to prevent path traversal or injection attacks.
fn validate_resource_id(id: &str, resource_type: &str) -> Result<(), AppError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidRequest(format!(
            "{resource_type} ID cannot be empty"
        )));
    }
    if trimmed.len() > 128 {
        return Err(AppError::InvalidRequest(format!(
            "{resource_type} ID exceeds maximum length of 128 characters"
        )));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AppError::InvalidRequest(format!(
            "Invalid {resource_type} ID: must contain only alphanumeric, underscore, or hyphen characters"
        )));
    }
    Ok(())
}

/// Extract authenticated owner subject from request extension.
///
/// Fails closed with 401 Unauthorized if principal is missing or invalid.
fn get_owner_subject(principal: &Option<Extension<AuthPrincipal>>) -> Result<String, AppError> {
    match principal {
        Some(Extension(p)) if !p.subject.trim().is_empty() => Ok(p.subject.clone()),
        _ => Err(AppError::Unauthorized(
            "Authentication required: Google login is mandatory for all user data operations"
                .to_string(),
        )),
    }
}

/// Extract optional `X-Drive-Token` header.
fn get_drive_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-drive-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// Extract and enforce required `X-Drive-Token` header.
///
/// Fails closed with 401 Unauthorized if token is missing or empty.
fn require_drive_token(headers: &HeaderMap) -> Result<&str, AppError> {
    get_drive_token(headers).ok_or_else(|| {
        AppError::Unauthorized(
            "Google Drive authorization required: missing or empty X-Drive-Token header"
                .to_string(),
        )
    })
}

/// GET /api/v1/drive/status — Query synchronization status and pending change counts.
pub async fn get_sync_status(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
) -> Result<(StatusCode, Json<SyncStatus>), AppError> {
    let owner = principal
        .as_ref()
        .map(|Extension(p)| p.subject.clone())
        .unwrap_or_else(|| "anonymous".to_string());
    let status = state.sync_engine.get_status(&owner).await;
    Ok((StatusCode::OK, Json(status)))
}

/// POST /api/v1/drive/config — Update Drive synchronization configuration (on/off switch).
pub async fn update_sync_config(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    Json(payload): Json<DriveConfigUpdate>,
) -> Result<(StatusCode, Json<SyncStatus>), AppError> {
    if payload.enabled && !state.sync_engine.crypto().is_configured() {
        return Err(AppError::InvalidRequest(
            "Cannot enable Google Drive synchronization: DRIVE_ENCRYPTION_KEY is not configured or invalid on the backend server. A valid 32-byte key (64 hex characters) is strictly required.".to_string(),
        ));
    }
    state.sync_engine.set_sync_enabled(payload.enabled).await;
    let owner = principal
        .as_ref()
        .map(|Extension(p)| p.subject.clone())
        .unwrap_or_else(|| "anonymous".to_string());
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
    let token = require_drive_token(&headers)?;

    let result = state.sync_engine.sync(&owner, token).await.map_err(|e| {
        tracing::error!(owner = %owner, error = %e, "Google Drive trigger_sync failed");
        AppError::from(e)
    })?;
    Ok((StatusCode::OK, Json(result)))
}

/// Safely redact owner subject identifier for logs without exposing full OAuth IDs.
fn redact_owner(owner: &str) -> String {
    if let Some((prefix, id)) = owner.split_once(':') {
        if id.len() > 6 {
            let start = &id[..3];
            let end = &id[id.len() - 3..];
            format!("{prefix}:{start}***{end} (len: {})", id.len())
        } else {
            format!("{prefix}:***")
        }
    } else if owner.len() > 6 {
        format!(
            "{}***{} (len: {})",
            &owner[..3],
            &owner[owner.len() - 3..],
            owner.len()
        )
    } else {
        "***".to_string()
    }
}

/// GET /api/v1/drive/conversations — List all active conversations for the authenticated user.
///
/// Hydrates transparently from Google Drive if the in-memory cache is cold.
pub async fn list_conversations(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Vec<SyncConversation>>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers);
    let has_drive_token = token.is_some();
    let drive_token_len = token.map(|t| t.len()).unwrap_or(0);
    tracing::debug!(
        owner = %redact_owner(&owner),
        has_drive_token,
        drive_token_len,
        "GET /api/v1/drive/conversations received"
    );

    let list = state
        .sync_engine
        .list_conversations_with_drive(&owner, token)
        .await
        .map_err(|e| {
            tracing::error!(owner = %owner, error = %e, "Google Drive list_conversations failed");
            AppError::from(e)
        })?;

    tracing::debug!(
        owner = %redact_owner(&owner),
        count = list.len(),
        "list_conversations returning conversations"
    );

    Ok((StatusCode::OK, Json(list)))
}

/// POST /api/v1/drive/conversations — Create or upsert a conversation on Google Drive.
///
/// Confirms save only after Drive file write and manifest commit succeed.
pub async fn create_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Json(payload): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<SyncConversation>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = require_drive_token(&headers)?;

    let title = payload.title.trim();
    if title.is_empty() {
        return Err(AppError::InvalidRequest(
            "Conversation title cannot be empty".to_string(),
        ));
    }
    if title.len() > 500 {
        return Err(AppError::InvalidRequest(
            "Conversation title exceeds maximum length of 500 characters".to_string(),
        ));
    }

    if let Some(ref custom_id) = payload.id {
        validate_resource_id(custom_id, "conversation")?;
    }

    if payload.messages.len() > 1000 {
        return Err(AppError::InvalidRequest(
            "Initial messages array exceeds maximum limit of 1000 messages".to_string(),
        ));
    }

    let id = payload
        .id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("conv_{}", ring::rand::SystemRandom::new().generate_sub_id()));

    let mut conv = SyncConversation::new(id, owner.clone(), title.to_string());
    conv.messages = payload.messages;

    let saved = state
        .sync_engine
        .persist_conversation_to_drive(&owner, conv, token)
        .await
        .map_err(|e| {
            tracing::error!(owner = %owner, error = %e, "Google Drive persist_conversation failed");
            AppError::from(e)
        })?;

    Ok((StatusCode::CREATED, Json(saved)))
}

/// GET /api/v1/drive/conversations/{id} — Retrieve a single conversation by ID.
///
/// Hydrates transparently from Google Drive if not cached locally.
pub async fn get_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<SyncConversation>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers);

    validate_resource_id(&id, "conversation")?;

    let conv = state
        .sync_engine
        .get_or_hydrate_conversation(&owner, &id, token)
        .await
        .map_err(|e| {
            tracing::error!(owner = %owner, conversation_id = %id, error = %e, "Google Drive get_conversation failed");
            AppError::from(e)
        })?
        .ok_or_else(|| AppError::NotFound(format!("Conversation '{id}' not found")))?;

    if conv.owner_subject != owner {
        return Err(AppError::Forbidden(
            "Access denied to conversation".to_string(),
        ));
    }

    Ok((StatusCode::OK, Json(conv)))
}

/// PUT / PATCH /api/v1/drive/conversations/{id} — Update an existing conversation (rename or update messages).
///
/// Confirms save only after Drive file write and manifest commit succeed.
pub async fn update_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<UpdateConversationRequest>,
) -> Result<(StatusCode, Json<SyncConversation>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = require_drive_token(&headers)?;

    validate_resource_id(&id, "conversation")?;

    if payload.title.is_none() && payload.messages.is_none() {
        return Err(AppError::InvalidRequest(
            "At least one field ('title' or 'messages') must be provided to update".to_string(),
        ));
    }

    if let Some(ref t) = payload.title {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidRequest(
                "Conversation title cannot be empty".to_string(),
            ));
        }
        if trimmed.len() > 500 {
            return Err(AppError::InvalidRequest(
                "Conversation title exceeds maximum length of 500 characters".to_string(),
            ));
        }
    }

    if let Some(ref msgs) = payload.messages
        && msgs.len() > 1000
    {
        return Err(AppError::InvalidRequest(
            "Conversation messages array exceeds maximum limit of 1000 messages".to_string(),
        ));
    }

    let mut conv = state
        .sync_engine
        .get_or_hydrate_conversation(&owner, &id, Some(token))
        .await
        .map_err(|e| {
            tracing::error!(
                owner = %owner,
                conversation_id = %id,
                error = %e,
                "Google Drive get_conversation failed during update"
            );
            AppError::from(e)
        })?
        .ok_or_else(|| AppError::NotFound(format!("Conversation '{id}' not found")))?;

    if conv.deleted {
        return Err(AppError::NotFound(format!("Conversation '{id}' not found")));
    }

    if conv.owner_subject != owner {
        return Err(AppError::Forbidden(
            "Access denied to conversation".to_string(),
        ));
    }

    if let Some(t) = payload.title {
        conv.title = t.trim().to_string();
    }
    if let Some(msgs) = payload.messages {
        conv.messages = msgs;
    }
    conv.revision += 1;
    conv.updated_at = crate::drive::chrono_like_now();

    let saved = state
        .sync_engine
        .persist_conversation_to_drive(&owner, conv, token)
        .await
        .map_err(|e| {
            tracing::error!(
                owner = %owner,
                conversation_id = %id,
                error = %e,
                "Google Drive persist_conversation failed during update"
            );
            AppError::from(e)
        })?;

    Ok((StatusCode::OK, Json(saved)))
}

/// DELETE /api/v1/drive/conversations/{id} — Soft-delete a conversation on Google Drive.
///
/// CRITICAL: Deleting a conversation NEVER deletes its referenced Library items!
pub async fn delete_conversation(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = require_drive_token(&headers)?;

    validate_resource_id(&id, "conversation")?;

    let deleted = state
        .sync_engine
        .delete_conversation_on_drive(&owner, &id, token)
        .await
        .map_err(|e| {
            tracing::error!(owner = %owner, conversation_id = %id, error = %e, "Google Drive delete_conversation failed");
            AppError::from(e)
        })?;

    if deleted {
        Ok((
            StatusCode::OK,
            Json(json!({
                "status": "deleted",
                "conversation_id": id,
                "message": "Conversation soft-deleted on Google Drive. Referenced Library items remain preserved."
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
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Vec<LibraryItem>>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers);

    let list = state
        .sync_engine
        .list_library_items_with_drive(&owner, token)
        .await?;

    Ok((StatusCode::OK, Json(list)))
}

/// POST /api/v1/drive/library — Create and upload a new library item to Google Drive.
///
/// Confirms save only after Drive binary upload and manifest commit succeed.
pub async fn create_library_item(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Json(payload): Json<CreateLibraryItemRequest>,
) -> Result<(StatusCode, Json<LibraryItem>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = require_drive_token(&headers)?;

    let clean_filename = sanitize_filename(&payload.filename);
    if clean_filename.trim().is_empty() {
        return Err(AppError::InvalidRequest(
            "Library item filename cannot be empty or contain only invalid characters".to_string(),
        ));
    }
    if payload.mime_type.trim().is_empty() {
        return Err(AppError::InvalidRequest(
            "Library item mime_type cannot be empty".to_string(),
        ));
    }
    if let Some(ref custom_id) = payload.id {
        validate_resource_id(custom_id, "library_item")?;
    }
    if let Some(ref conv_id) = payload.conversation_id {
        validate_resource_id(conv_id, "conversation")?;
    }

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
        .persist_library_item_to_drive(&owner, item, raw_bytes, token)
        .await?;

    Ok((StatusCode::CREATED, Json(saved)))
}

/// GET /api/v1/drive/library/{id} — Retrieve metadata for a library item.
pub async fn get_library_item(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<LibraryItem>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = get_drive_token(&headers);

    validate_resource_id(&id, "library_item")?;

    let item = state
        .sync_engine
        .get_or_hydrate_library_item(&owner, &id, token)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Library item '{id}' not found")))?;

    if item.owner_subject != owner {
        return Err(AppError::Forbidden(
            "Access denied to library item".to_string(),
        ));
    }

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

    validate_resource_id(&id, "library_item")?;

    let meta = state
        .sync_engine
        .get_or_hydrate_library_item(&owner, &id, token)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Library item '{id}' not found")))?;

    if meta.owner_subject != owner {
        return Err(AppError::Forbidden(
            "Access denied to library item".to_string(),
        ));
    }

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

/// DELETE /api/v1/drive/library/{id} — Explicitly delete a library item on Google Drive.
pub async fn delete_library_item(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let owner = get_owner_subject(&principal)?;
    let token = require_drive_token(&headers)?;

    validate_resource_id(&id, "library_item")?;

    let deleted = state
        .sync_engine
        .delete_library_item_on_drive(&owner, &id, token)
        .await?;

    if deleted {
        Ok((
            StatusCode::OK,
            Json(json!({
                "status": "deleted",
                "library_item_id": id,
                "message": "Library item explicitly deleted on Google Drive."
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
    let token = require_drive_token(&headers)?;

    let result = state.sync_engine.restore(&owner, token).await?;
    Ok((StatusCode::OK, Json(result)))
}

/// POST /api/v1/drive/logout — Clear local in-memory session/cache for user.
///
/// Remote Google Drive data is NEVER deleted.
pub async fn logout_user(
    State(state): State<AppState>,
    principal: Option<Extension<AuthPrincipal>>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let owner = get_owner_subject(&principal)?;
    state
        .sync_engine
        .local_store()
        .clear_user_session(&owner)
        .await?;

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
