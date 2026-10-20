//! Standalone Mock OpenAI-compatible Upstream Server.
//!
//! Serves ultra-fast, zero-cost mock chat completion and SSE streaming
//! responses for bounded local load testing of the ai-backend daemon.

use axum::Router;
use axum::response::IntoResponse;
use axum::routing::post;
use std::net::SocketAddr;

async fn mock_chat_completions(
    _headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let req_str = String::from_utf8_lossy(&body);
    let is_stream = req_str.contains("\"stream\":true") || req_str.contains("\"stream\": true");

    if is_stream {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(axum::body::Bytes::from(
                "data: {\"id\":\"chatcmpl_mock_stream\",\"choices\":[{\"delta\":{\"content\":\"Rust \"}}]}\n\n",
            )),
            Ok(axum::body::Bytes::from(
                "data: {\"id\":\"chatcmpl_mock_stream\",\"choices\":[{\"delta\":{\"content\":\"is \"}}]}\n\n",
            )),
            Ok(axum::body::Bytes::from(
                "data: {\"id\":\"chatcmpl_mock_stream\",\"choices\":[{\"delta\":{\"content\":\"blazing fast.\"}}]}\n\n",
            )),
            Ok(axum::body::Bytes::from("data: [DONE]\n\n")),
        ]);

        (
            [
                (axum::http::header::CONTENT_TYPE, "text/event-stream"),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
                (axum::http::header::CONNECTION, "keep-alive"),
            ],
            axum::body::Body::from_stream(stream),
        )
            .into_response()
    } else {
        let resp = serde_json::json!({
            "id": "chatcmpl_mock_loadtest",
            "model": "openai/gpt-oss-120b",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Benchmark response from simulated local provider."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 18,
                "completion_tokens": 8,
                "total_tokens": 26
            }
        });

        (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            axum::Json(resp),
        )
            .into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("MOCK_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8096);

    let app = Router::new()
        .route("/chat/completions", post(mock_chat_completions))
        .route("/openai/v1/chat/completions", post(mock_chat_completions));

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("[INFO] Mock upstream server running on http://{}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}
