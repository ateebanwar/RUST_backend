//! Chat completion and SSE streaming HTTP handlers.
//!
//! Conforms strictly to API_SPEC.md §5 and §6, and Phase 8 Usage and Quota Tracking.

use crate::auth::principal::AuthPrincipal;
use crate::errors::AppError;
use crate::inference::ChatCompletionRequest;
use crate::models::{CompletionResponse, UsageMetadata};
use crate::providers::ProviderStream;
use crate::usage::UsageService;
use crate::usage::model::{RequestOutcome, RoutingMode};
use crate::{AppState, RequestId};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn generate_msg_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    crate::drive::crypto::encode_hex(&bytes)
}

/// Hydrate model-agnostic conversation context from Google Drive if a conversation_id is provided.
#[allow(clippy::result_large_err)]
async fn hydrate_conversation_context(
    state: &AppState,
    principal: &Option<Extension<AuthPrincipal>>,
    headers: &HeaderMap,
    payload: &mut ChatCompletionRequest,
    req_id: &str,
) -> Result<Option<(String, String, crate::drive::SyncConversation)>, Response> {
    let Some(ref conv_id) = payload.conversation_id else {
        return Ok(None);
    };

    let owner = match principal {
        Some(Extension(p)) if !p.subject.trim().is_empty() => p.subject.clone(),
        _ => {
            return Err(AppError::Unauthorized(
                "Authentication required for conversation continuity".to_string(),
            )
            .to_response_with_id(req_id));
        }
    };

    let drive_token = headers
        .get("x-drive-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let existing_conv_opt = match state
        .sync_engine
        .get_or_hydrate_conversation(&owner, conv_id, drive_token)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            return Err(
                AppError::InternalError(format!("Failed to retrieve conversation: {e}"))
                    .to_response_with_id(req_id),
            );
        }
    };

    let Some(existing_conv) = existing_conv_opt else {
        return Err(
            AppError::NotFound(format!("Conversation '{conv_id}' not found"))
                .to_response_with_id(req_id),
        );
    };

    if existing_conv.owner_subject != owner {
        return Err(
            AppError::Forbidden("Access denied to conversation".to_string())
                .to_response_with_id(req_id),
        );
    }

    // Reconstruct model-agnostic messages from Drive history
    let mut full_messages: Vec<crate::models::ChatMessage> = Vec::new();
    let payload_system = payload
        .messages
        .iter()
        .find(|m| m.role.eq_ignore_ascii_case("system"))
        .cloned();

    if let Some(sys) = payload_system {
        full_messages.push(sys);
    }

    for msg in &existing_conv.messages {
        if msg.role.eq_ignore_ascii_case("system")
            && full_messages
                .iter()
                .any(|m| m.role.eq_ignore_ascii_case("system"))
        {
            continue;
        }
        full_messages.push(crate::models::ChatMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
        });
    }

    for msg in &payload.messages {
        if !msg.role.eq_ignore_ascii_case("system") {
            if full_messages
                .last()
                .map(|last| last.role == msg.role && last.content == msg.content)
                .unwrap_or(false)
            {
                continue;
            }
            full_messages.push(msg.clone());
        }
    }

    payload.messages = full_messages;
    let token_str = drive_token.unwrap_or("").to_string();
    Ok(Some((owner, token_str, existing_conv)))
}

/// Handler for POST /api/v1/chat/completions (Buffered synchronous completion)
#[allow(clippy::result_large_err)]
pub async fn chat_completions(
    State(state): State<AppState>,
    Extension(RequestId(req_id)): Extension<RequestId>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Json(mut payload): Json<ChatCompletionRequest>,
) -> Result<(StatusCode, Json<CompletionResponse>), Response> {
    let start_instant = Instant::now();
    let start_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let conv_context =
        hydrate_conversation_context(&state, &principal, &headers, &mut payload, &req_id).await?;

    let routing_mode = if payload.routing_mode.eq_ignore_ascii_case("manual") {
        RoutingMode::Manual
    } else {
        RoutingMode::Auto
    };

    let caller_subject = principal.as_ref().map(|p| p.0.subject.as_str());

    payload.stream = false;

    match state.inference.execute_completion(&payload).await {
        Ok(response) => {
            state
                .usage
                .record_buffered(
                    &req_id,
                    &response.provider_used,
                    &response.model_used,
                    routing_mode,
                    start_instant,
                    start_epoch_ms,
                    RequestOutcome::Success,
                    None,
                    response.usage.as_ref(),
                    caller_subject,
                )
                .await;

            // Automatically persist updated conversation turns back to Google Drive
            if let Some((owner, token_str, mut existing_conv)) = conv_context
                && !token_str.is_empty()
                && state.sync_engine.crypto().is_configured()
            {
                for msg in &payload.messages {
                    if msg.role.eq_ignore_ascii_case("user")
                        && !existing_conv
                            .messages
                            .iter()
                            .any(|m| m.role == msg.role && m.content == msg.content)
                    {
                        existing_conv.messages.push(crate::drive::SyncMessage::new(
                            format!("msg_{}", generate_msg_id()),
                            existing_conv.id.clone(),
                            msg.role.clone(),
                            msg.content.clone(),
                        ));
                    }
                }
                existing_conv.messages.push(crate::drive::SyncMessage::new(
                    format!("msg_{}", generate_msg_id()),
                    existing_conv.id.clone(),
                    response.message.role.clone(),
                    response.message.content.clone(),
                ));
                existing_conv.revision += 1;
                existing_conv.updated_at = crate::drive::chrono_like_now();
                let _ = state
                    .sync_engine
                    .persist_conversation_to_drive(&owner, existing_conv, &token_str)
                    .await;
            }

            Ok((StatusCode::OK, Json(response)))
        }
        Err(err) => {
            let outcome = match &err {
                AppError::GatewayTimeout(_) => RequestOutcome::Timeout,
                AppError::RateLimited { .. } | AppError::UpstreamFailure(_) => {
                    RequestOutcome::ProviderError
                }
                AppError::InvalidRequest(_)
                | AppError::UnsupportedModality(_)
                | AppError::NotFound(_) => RequestOutcome::PreDispatchFailure,
                _ => RequestOutcome::ProviderError,
            };

            // Resolve candidates on demand only on the error path for telemetry recording
            let resolution_candidate = state.inference.resolve_candidates(&payload).ok();

            let (prov, mdl) = match &resolution_candidate {
                Some(r) => (r.provider_id.as_str(), r.selected_model_id.as_str()),
                None => (
                    "unresolved",
                    payload
                        .requested_model_id
                        .as_deref()
                        .unwrap_or("unresolved"),
                ),
            };

            state
                .usage
                .record_buffered(
                    &req_id,
                    prov,
                    mdl,
                    routing_mode,
                    start_instant,
                    start_epoch_ms,
                    outcome,
                    Some(err.error_code().to_string()),
                    None,
                    caller_subject,
                )
                .await;

            Err(err.to_response_with_id(&req_id))
        }
    }
}

/// Drop guard ensuring that client disconnects or cancellations are finalized exactly once.
struct StreamFinalizationGuard {
    finalized: Arc<AtomicBool>,
    usage_service: Arc<UsageService>,
    request_id: String,
    provider_id: String,
    model_id: String,
    routing_mode: RoutingMode,
    start_instant: Instant,
    start_epoch_ms: u64,
    latest_usage: Arc<std::sync::Mutex<Option<UsageMetadata>>>,
    subject: Option<String>,
    _stream_permit: Option<crate::security::StreamPermit>,
}

impl Drop for StreamFinalizationGuard {
    fn drop(&mut self) {
        // If not already finalized by normal completion or upstream error,
        // the client connection was aborted/cancelled prematurely.
        if !self.finalized.swap(true, Ordering::SeqCst) {
            let usage = self.latest_usage.lock().unwrap().clone();
            let usage_svc = self.usage_service.clone();
            let req_id = self.request_id.clone();
            let prov = self.provider_id.clone();
            let mdl = self.model_id.clone();
            let r_mode = self.routing_mode;
            let s_inst = self.start_instant;
            let s_epoch = self.start_epoch_ms;
            let subj = self.subject.clone();

            tokio::spawn(async move {
                usage_svc
                    .record_streaming(
                        &req_id,
                        &prov,
                        &mdl,
                        r_mode,
                        s_inst,
                        s_epoch,
                        RequestOutcome::ClientCancelled,
                        Some("client_disconnected".to_string()),
                        usage.as_ref(),
                        subj.as_deref(),
                    )
                    .await;
            });
        }
    }
}

enum SseStreamState {
    SendRouting {
        routing_ev: Event,
        inner: ProviderStream,
        guard: Arc<StreamFinalizationGuard>,
    },
    Streaming {
        inner: ProviderStream,
        guard: Arc<StreamFinalizationGuard>,
    },
    Terminated {
        _guard: Arc<StreamFinalizationGuard>,
    },
}

/// Handler for POST /api/v1/chat/stream (Server-Sent Events streaming completion)
#[allow(clippy::result_large_err)]
pub async fn chat_stream(
    State(state): State<AppState>,
    Extension(RequestId(req_id)): Extension<RequestId>,
    principal: Option<Extension<AuthPrincipal>>,
    headers: HeaderMap,
    Json(mut payload): Json<ChatCompletionRequest>,
) -> Result<impl IntoResponse, Response> {
    let start_instant = Instant::now();
    let start_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let _conv_context =
        hydrate_conversation_context(&state, &principal, &headers, &mut payload, &req_id).await?;

    let routing_mode = if payload.routing_mode.eq_ignore_ascii_case("manual") {
        RoutingMode::Manual
    } else {
        RoutingMode::Auto
    };

    let caller_subject = principal.as_ref().map(|p| p.0.subject.clone());

    let stream_permit = match state.rate_limiter.acquire_stream_permit() {
        Ok(permit) => permit,
        Err(rej) => {
            return Err(rej.to_app_error().to_response_with_id(&req_id));
        }
    };

    payload.stream = true;

    let stream_res = match state.inference.execute_stream(&payload).await {
        Ok(res) => res,
        Err(err) => {
            let outcome = match &err {
                AppError::GatewayTimeout(_) => RequestOutcome::Timeout,
                AppError::RateLimited { .. } | AppError::UpstreamFailure(_) => {
                    RequestOutcome::ProviderError
                }
                AppError::InvalidRequest(_)
                | AppError::UnsupportedModality(_)
                | AppError::NotFound(_) => RequestOutcome::PreDispatchFailure,
                _ => RequestOutcome::ProviderError,
            };

            let fallback_model = payload
                .requested_model_id
                .clone()
                .unwrap_or_else(|| "unresolved".to_string());

            state
                .usage
                .record_streaming(
                    &req_id,
                    "unresolved",
                    &fallback_model,
                    routing_mode,
                    start_instant,
                    start_epoch_ms,
                    outcome,
                    Some(err.error_code().to_string()),
                    None,
                    caller_subject.as_deref(),
                )
                .await;

            return Err(err.to_response_with_id(&req_id));
        }
    };

    let model_used = stream_res.resolution.selected_model_id.clone();
    let provider_used = stream_res.resolution.provider_id.clone();
    let task_category = stream_res.resolution.detected_task.clone();
    let is_fallback = stream_res.is_fallback;

    let routing_event = Event::default().event("routing").data(
        json!({
            "model_used": model_used,
            "provider_used": provider_used,
            "is_fallback": is_fallback,
            "task_category": task_category,
        })
        .to_string(),
    );

    let guard = Arc::new(StreamFinalizationGuard {
        finalized: Arc::new(AtomicBool::new(false)),
        usage_service: state.usage.clone(),
        request_id: req_id,
        provider_id: provider_used,
        model_id: model_used,
        routing_mode,
        start_instant,
        start_epoch_ms,
        latest_usage: Arc::new(std::sync::Mutex::new(None)),
        subject: caller_subject,
        _stream_permit: Some(stream_permit),
    });

    let initial_state = SseStreamState::SendRouting {
        routing_ev: routing_event,
        inner: stream_res.stream,
        guard,
    };

    let sse_stream = futures_util::stream::unfold(initial_state, |state| async move {
        match state {
            SseStreamState::SendRouting {
                routing_ev,
                inner,
                guard,
            } => Some((
                Ok::<Event, Infallible>(routing_ev),
                SseStreamState::Streaming { inner, guard },
            )),
            SseStreamState::Streaming { mut inner, guard } => match inner.next().await {
                Some(Ok(chunk)) => {
                    if let Some(u) = chunk.usage {
                        *guard.latest_usage.lock().unwrap() = Some(u);
                    }

                    if chunk.is_done {
                        if !guard.finalized.swap(true, Ordering::SeqCst) {
                            let usage = guard.latest_usage.lock().unwrap().clone();
                            let usage_svc = guard.usage_service.clone();
                            let req_id = guard.request_id.clone();
                            let prov = guard.provider_id.clone();
                            let mdl = guard.model_id.clone();
                            let r_mode = guard.routing_mode;
                            let s_inst = guard.start_instant;
                            let s_epoch = guard.start_epoch_ms;
                            let subj = guard.subject.clone();

                            tokio::spawn(async move {
                                usage_svc
                                    .record_streaming(
                                        &req_id,
                                        &prov,
                                        &mdl,
                                        r_mode,
                                        s_inst,
                                        s_epoch,
                                        RequestOutcome::Success,
                                        None,
                                        usage.as_ref(),
                                        subj.as_deref(),
                                    )
                                    .await;
                            });
                        }

                        let done_ev = Event::default()
                            .event("done")
                            .data(json!({ "status": "completed" }).to_string());
                        Some((Ok(done_ev), SseStreamState::Terminated { _guard: guard }))
                    } else {
                        let chunk_ev = Event::default()
                            .event("chunk")
                            .data(json!({ "delta": chunk.delta }).to_string());
                        Some((Ok(chunk_ev), SseStreamState::Streaming { inner, guard }))
                    }
                }
                Some(Err(err)) => {
                    if !guard.finalized.swap(true, Ordering::SeqCst) {
                        let usage = guard.latest_usage.lock().unwrap().clone();
                        let usage_svc = guard.usage_service.clone();
                        let req_id = guard.request_id.clone();
                        let prov = guard.provider_id.clone();
                        let mdl = guard.model_id.clone();
                        let r_mode = guard.routing_mode;
                        let s_inst = guard.start_instant;
                        let s_epoch = guard.start_epoch_ms;
                        let subj = guard.subject.clone();

                        tokio::spawn(async move {
                            usage_svc
                                .record_streaming(
                                    &req_id,
                                    &prov,
                                    &mdl,
                                    r_mode,
                                    s_inst,
                                    s_epoch,
                                    RequestOutcome::StreamInterrupted,
                                    Some("stream_interrupted".to_string()),
                                    usage.as_ref(),
                                    subj.as_deref(),
                                )
                                .await;
                        });
                    }

                    let err_app: AppError = err.into();
                    let error_ev = Event::default().event("error").data(
                        json!({
                            "code": err_app.error_code(),
                            "message": err_app.client_message(),
                            "retryable": err_app.is_retryable(),
                        })
                        .to_string(),
                    );
                    Some((Ok(error_ev), SseStreamState::Terminated { _guard: guard }))
                }
                None => {
                    if !guard.finalized.swap(true, Ordering::SeqCst) {
                        let usage = guard.latest_usage.lock().unwrap().clone();
                        let usage_svc = guard.usage_service.clone();
                        let req_id = guard.request_id.clone();
                        let prov = guard.provider_id.clone();
                        let mdl = guard.model_id.clone();
                        let r_mode = guard.routing_mode;
                        let s_inst = guard.start_instant;
                        let s_epoch = guard.start_epoch_ms;
                        let subj = guard.subject.clone();

                        tokio::spawn(async move {
                            usage_svc
                                .record_streaming(
                                    &req_id,
                                    &prov,
                                    &mdl,
                                    r_mode,
                                    s_inst,
                                    s_epoch,
                                    RequestOutcome::Success,
                                    None,
                                    usage.as_ref(),
                                    subj.as_deref(),
                                )
                                .await;
                        });
                    }

                    let done_ev = Event::default()
                        .event("done")
                        .data(json!({ "status": "completed" }).to_string());
                    Some((Ok(done_ev), SseStreamState::Terminated { _guard: guard }))
                }
            },
            SseStreamState::Terminated { .. } => None,
        }
    });

    Ok(Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}
