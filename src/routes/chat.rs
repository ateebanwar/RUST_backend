//! Chat completion and SSE streaming HTTP handlers.
//!
//! Conforms strictly to API_SPEC.md §5 and §6.

use crate::errors::AppError;
use crate::inference::ChatCompletionRequest;
use crate::models::CompletionResponse;
use crate::providers::ProviderStream;
use crate::{AppState, RequestId};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::json;
use std::convert::Infallible;
use std::time::Duration;

/// Handler for POST /api/v1/chat/completions (Buffered synchronous completion)
#[allow(clippy::result_large_err)]
pub async fn chat_completions(
    State(state): State<AppState>,
    Extension(RequestId(req_id)): Extension<RequestId>,
    Json(mut payload): Json<ChatCompletionRequest>,
) -> Result<(StatusCode, Json<CompletionResponse>), Response> {
    payload.stream = false;
    let response = state
        .inference
        .execute_completion(&payload)
        .await
        .map_err(|err| err.to_response_with_id(&req_id))?;
    Ok((StatusCode::OK, Json(response)))
}

enum SseStreamState {
    SendRouting(Event, ProviderStream),
    Streaming(ProviderStream),
    Terminated,
}

/// Handler for POST /api/v1/chat/stream (Server-Sent Events streaming completion)
#[allow(clippy::result_large_err)]
pub async fn chat_stream(
    State(state): State<AppState>,
    Extension(RequestId(req_id)): Extension<RequestId>,
    Json(mut payload): Json<ChatCompletionRequest>,
) -> Result<impl IntoResponse, Response> {
    payload.stream = true;
    let stream_res = state
        .inference
        .execute_stream(&payload)
        .await
        .map_err(|err| err.to_response_with_id(&req_id))?;

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

    let initial_state = SseStreamState::SendRouting(routing_event, stream_res.stream);

    let sse_stream = futures_util::stream::unfold(initial_state, |state| async move {
        match state {
            SseStreamState::SendRouting(routing_ev, inner) => Some((
                Ok::<Event, Infallible>(routing_ev),
                SseStreamState::Streaming(inner),
            )),
            SseStreamState::Streaming(mut inner) => match inner.next().await {
                Some(Ok(chunk)) => {
                    if chunk.is_done {
                        let done_ev = Event::default()
                            .event("done")
                            .data(json!({ "status": "completed" }).to_string());
                        Some((Ok(done_ev), SseStreamState::Terminated))
                    } else {
                        let chunk_ev = Event::default()
                            .event("chunk")
                            .data(json!({ "delta": chunk.delta }).to_string());
                        Some((Ok(chunk_ev), SseStreamState::Streaming(inner)))
                    }
                }
                Some(Err(err)) => {
                    let err_app: AppError = err.into();
                    let error_ev = Event::default().event("error").data(
                        json!({
                            "code": err_app.error_code(),
                            "message": err_app.client_message(),
                            "retryable": err_app.is_retryable(),
                        })
                        .to_string(),
                    );
                    Some((Ok(error_ev), SseStreamState::Terminated))
                }
                None => {
                    let done_ev = Event::default()
                        .event("done")
                        .data(json!({ "status": "completed" }).to_string());
                    Some((Ok(done_ev), SseStreamState::Terminated))
                }
            },
            SseStreamState::Terminated => None,
        }
    });

    Ok(Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}
