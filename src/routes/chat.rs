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
) -> Result<
    Option<(
        String,
        String,
        crate::drive::SyncConversation,
        Vec<crate::models::ChatMessage>,
    )>,
    Response,
> {
    let Some(ref conv_id) = payload.conversation_id else {
        return Ok(None);
    };

    let owner = match principal {
        Some(Extension(p)) if !p.subject.trim().is_empty() => p.subject.clone(),
        _ => {
            return Err(AppError::Unauthorized(
                "Authentication required for conversation continuity: valid Google login is mandatory".to_string(),
            )
            .to_response_with_id(req_id));
        }
    };

    let drive_token = match headers
        .get("x-drive-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        Some(t) => t,
        None => {
            return Err(AppError::Unauthorized(
                "Google Drive authorization required for conversation continuity: missing X-Drive-Token header".to_string(),
            )
            .to_response_with_id(req_id));
        }
    };

    let existing_conv = match state
        .sync_engine
        .get_or_hydrate_conversation(&owner, conv_id, Some(drive_token))
        .await
    {
        Ok(Some(c)) => {
            if c.owner_subject != owner {
                return Err(
                    AppError::Forbidden("Access denied to conversation".to_string())
                        .to_response_with_id(req_id),
                );
            }
            c
        }
        Ok(None) => {
            // Initialize new conversation record for this tenant
            crate::drive::SyncConversation::new(
                conv_id.clone(),
                owner.clone(),
                "New Conversation".to_string(),
            )
        }
        Err(e) => {
            return Err(
                AppError::InternalError(format!("Failed to retrieve conversation: {e}"))
                    .to_response_with_id(req_id),
            );
        }
    };

    // Reconstruct model-agnostic messages from Drive history
    let mut full_messages: Vec<crate::models::ChatMessage> = Vec::new();
    let payload_system = payload
        .messages
        .iter()
        .find(|m| m.role.eq_ignore_ascii_case("system"))
        .cloned();

    if let Some(sys) = payload_system {
        full_messages.push(sys);
    } else if let Some(sys_conv) = existing_conv
        .messages
        .iter()
        .find(|m| m.role.eq_ignore_ascii_case("system"))
    {
        full_messages.push(crate::models::ChatMessage {
            role: sys_conv.role.clone(),
            content: sys_conv.content.clone(),
        });
    }

    let existing_non_sys: Vec<&crate::drive::SyncMessage> = existing_conv
        .messages
        .iter()
        .filter(|m| !m.role.eq_ignore_ascii_case("system"))
        .collect();

    let incoming_non_sys: Vec<&crate::models::ChatMessage> = payload
        .messages
        .iter()
        .filter(|m| !m.role.eq_ignore_ascii_case("system"))
        .collect();

    let mut prefix_len = 0;
    while prefix_len < existing_non_sys.len() && prefix_len < incoming_non_sys.len() {
        let e = existing_non_sys[prefix_len];
        let p = incoming_non_sys[prefix_len];
        if e.role.eq_ignore_ascii_case(&p.role) && e.content == p.content {
            prefix_len += 1;
        } else {
            break;
        }
    }

    for e in &existing_non_sys {
        full_messages.push(crate::models::ChatMessage {
            role: e.role.clone(),
            content: e.content.clone(),
        });
    }

    let mut new_user_msgs = Vec::new();
    if prefix_len > 0 && prefix_len <= incoming_non_sys.len() {
        for p in &incoming_non_sys[prefix_len..] {
            full_messages.push((*p).clone());
            new_user_msgs.push((*p).clone());
        }
    } else {
        for p in incoming_non_sys {
            full_messages.push(p.clone());
            new_user_msgs.push(p.clone());
        }
    }

    payload.messages = full_messages;
    Ok(Some((
        owner,
        drive_token.to_string(),
        existing_conv,
        new_user_msgs,
    )))
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
            if let Some((owner, token_str, mut existing_conv, new_user_msgs)) = conv_context {
                if token_str.is_empty() || !state.sync_engine.crypto().is_configured() {
                    return Err(AppError::UpstreamFailure(
                        "Google Drive persistence failed: Drive token is empty or encryption key not configured".to_string(),
                    )
                    .to_response_with_id(&req_id));
                }
                for msg in new_user_msgs {
                    existing_conv.messages.push(crate::drive::SyncMessage::new(
                        format!("msg_{}", generate_msg_id()),
                        existing_conv.id.clone(),
                        msg.role,
                        msg.content,
                    ));
                }
                existing_conv.messages.push(crate::drive::SyncMessage::new(
                    format!("msg_{}", generate_msg_id()),
                    existing_conv.id.clone(),
                    response.message.role.clone(),
                    response.message.content.clone(),
                ));
                existing_conv.revision += 1;
                existing_conv.updated_at = crate::drive::chrono_like_now();
                state
                    .sync_engine
                    .persist_conversation_to_drive(&owner, existing_conv, &token_str)
                    .await
                    .map_err(|e| {
                        AppError::UpstreamFailure(format!("Google Drive persistence failed: {e}"))
                            .to_response_with_id(&req_id)
                    })?;
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

struct StreamContext {
    accumulated_content: String,
    conv_context: Option<(
        String,
        String,
        crate::drive::SyncConversation,
        Vec<crate::models::ChatMessage>,
    )>,
    sync_engine: Arc<crate::drive::SyncEngine>,
}

enum PersistResult {
    Saved {
        conversation_id: String,
        revision: u64,
    },
    SaveFailed {
        conversation_id: String,
        error: String,
    },
    NotRequested,
}

async fn execute_persistence(mut stream_ctx: StreamContext) -> PersistResult {
    if let Some((owner, token_str, mut existing_conv, new_user_msgs)) =
        stream_ctx.conv_context.take()
    {
        if token_str.is_empty() || !stream_ctx.sync_engine.crypto().is_configured() {
            return PersistResult::SaveFailed {
                conversation_id: existing_conv.id,
                error: "Google Drive sync requested but Drive token is empty or encryption key not configured".to_string(),
            };
        }

        for msg in new_user_msgs {
            existing_conv.messages.push(crate::drive::SyncMessage::new(
                format!("msg_{}", generate_msg_id()),
                existing_conv.id.clone(),
                msg.role,
                msg.content,
            ));
        }
        existing_conv.messages.push(crate::drive::SyncMessage::new(
            format!("msg_{}", generate_msg_id()),
            existing_conv.id.clone(),
            "assistant".to_string(),
            stream_ctx.accumulated_content,
        ));
        existing_conv.revision += 1;
        existing_conv.updated_at = crate::drive::chrono_like_now();

        let conv_id = existing_conv.id.clone();
        let _rev = existing_conv.revision;

        match stream_ctx
            .sync_engine
            .persist_conversation_to_drive(&owner, existing_conv, &token_str)
            .await
        {
            Ok(saved_conv) => PersistResult::Saved {
                conversation_id: saved_conv.id,
                revision: saved_conv.revision,
            },
            Err(e) => PersistResult::SaveFailed {
                conversation_id: conv_id,
                error: e.to_string(),
            },
        }
    } else {
        PersistResult::NotRequested
    }
}

fn create_terminal_events(
    persist_res: PersistResult,
    guard: Arc<StreamFinalizationGuard>,
) -> (Event, SseStreamState) {
    match persist_res {
        PersistResult::Saved {
            conversation_id,
            revision,
        } => {
            let done_ev = Event::default().event("done").data(
                json!({
                    "status": "completed",
                    "saved": true,
                    "conversation_id": conversation_id,
                    "revision": revision,
                })
                .to_string(),
            );
            (done_ev, SseStreamState::Terminated { _guard: guard })
        }
        PersistResult::SaveFailed {
            conversation_id,
            error,
        } => {
            let save_err_ev = Event::default().event("save_error").data(
                json!({
                    "code": "DRIVE_PERSISTENCE_FAILED",
                    "message": "Response generated successfully, but Google Drive persistence failed.",
                    "conversation_id": conversation_id,
                    "retryable": true,
                    "details": {
                        "error": error
                    }
                })
                .to_string(),
            );
            let done_ev = Event::default().event("done").data(
                json!({
                    "status": "completed_save_failed",
                    "saved": false,
                    "conversation_id": conversation_id,
                    "error": {
                        "code": "DRIVE_PERSISTENCE_FAILED",
                        "message": error
                    }
                })
                .to_string(),
            );
            (
                save_err_ev,
                SseStreamState::EmitTerminalDone { done_ev, guard },
            )
        }
        PersistResult::NotRequested => {
            let done_ev = Event::default().event("done").data(
                json!({
                    "status": "completed",
                    "saved": false,
                })
                .to_string(),
            );
            (done_ev, SseStreamState::Terminated { _guard: guard })
        }
    }
}

enum SseStreamState {
    SendRouting {
        routing_ev: Event,
        inner: ProviderStream,
        guard: Arc<StreamFinalizationGuard>,
        stream_ctx: StreamContext,
    },
    Streaming {
        inner: ProviderStream,
        guard: Arc<StreamFinalizationGuard>,
        stream_ctx: StreamContext,
    },
    EmitTerminalDone {
        done_ev: Event,
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

    let conv_context =
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

    let stream_ctx = StreamContext {
        accumulated_content: String::new(),
        conv_context,
        sync_engine: state.sync_engine.clone(),
    };

    let initial_state = SseStreamState::SendRouting {
        routing_ev: routing_event,
        inner: stream_res.stream,
        guard,
        stream_ctx,
    };

    let sse_stream = futures_util::stream::unfold(initial_state, |state| async move {
        match state {
            SseStreamState::SendRouting {
                routing_ev,
                inner,
                guard,
                stream_ctx,
            } => Some((
                Ok::<Event, Infallible>(routing_ev),
                SseStreamState::Streaming {
                    inner,
                    guard,
                    stream_ctx,
                },
            )),
            SseStreamState::Streaming {
                mut inner,
                guard,
                mut stream_ctx,
            } => match inner.next().await {
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

                        if !chunk.delta.is_empty() {
                            stream_ctx.accumulated_content.push_str(&chunk.delta);
                        }

                        let persist_res = execute_persistence(stream_ctx).await;
                        let (first_ev, next_state) = create_terminal_events(persist_res, guard);
                        Some((Ok(first_ev), next_state))
                    } else {
                        stream_ctx.accumulated_content.push_str(&chunk.delta);
                        let chunk_ev = Event::default()
                            .event("chunk")
                            .data(json!({ "delta": chunk.delta }).to_string());
                        Some((
                            Ok(chunk_ev),
                            SseStreamState::Streaming {
                                inner,
                                guard,
                                stream_ctx,
                            },
                        ))
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

                    let persist_res = execute_persistence(stream_ctx).await;
                    let (first_ev, next_state) = create_terminal_events(persist_res, guard);
                    Some((Ok(first_ev), next_state))
                }
            },
            SseStreamState::EmitTerminalDone { done_ev, guard } => {
                Some((Ok(done_ev), SseStreamState::Terminated { _guard: guard }))
            }
            SseStreamState::Terminated { .. } => None,
        }
    });

    Ok(Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}
