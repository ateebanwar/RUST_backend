//! Centralized application error handling and standardized JSON error envelopes.
//!
//! Conforms strictly to API_SPEC.md §1.3 with 10 canonical error codes,
//! accurate HTTP status code mappings, retryable flags, and zero-leak
//! sanitization of internal errors.

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::fmt;

/// Application-wide domain error variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    /// 400 Bad Request: Malformed JSON, failed validation, missing required fields.
    InvalidRequest(String),
    /// 401 Unauthorized: Missing, expired, or invalid auth token.
    Unauthorized(String),
    /// 403 Forbidden: Valid token, but insufficient permissions.
    Forbidden(String),
    /// 404 Not Found: Specified model ID, conversation, or resource does not exist.
    NotFound(String),
    /// 413 Payload Too Large: Inbound payload exceeds 4.5 MB body limit.
    PayloadTooLarge,
    /// 422 Unprocessable Entity: Requested task requires modality unsupported by model.
    UnsupportedModality(String),
    /// 429 Too Many Requests: Rate limit exceeded or upstream quota hit.
    RateLimited {
        message: String,
        retry_after_secs: Option<u64>,
    },
    /// 502 Bad Gateway: Upstream AI provider returned 5xx or unparsable response.
    UpstreamFailure(String),
    /// 504 Gateway Timeout: Upstream request exceeded 10-second deadline.
    GatewayTimeout(String),
    /// 500 Internal Server Error: Unhandled internal condition.
    InternalError(String),
    /// Configuration error causing request failure.
    ConfigError(String),
}

/// Standardized JSON error response envelope matching API_SPEC.md §1.3.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// Standardized JSON error body matching API_SPEC.md §1.3.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub request_id: String,
    pub details: Option<serde_json::Value>,
}

impl AppError {
    /// Canonical HTTP status code corresponding to this error.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::InvalidRequest(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::PayloadTooLarge => 413,
            Self::UnsupportedModality(_) => 422,
            Self::RateLimited { .. } => 429,
            Self::UpstreamFailure(_) => 502,
            Self::GatewayTimeout(_) => 504,
            Self::InternalError(_) | Self::ConfigError(_) => 500,
        }
    }

    /// Axum StatusCode instance.
    pub fn http_status(&self) -> StatusCode {
        StatusCode::from_u16(self.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Canonical error code string matching API_SPEC.md §1.3.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "INVALID_REQUEST",
            Self::Unauthorized(_) => "UNAUTHORIZED",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::UnsupportedModality(_) => "UNSUPPORTED_MODALITY",
            Self::RateLimited { .. } => "RATE_LIMITED",
            Self::UpstreamFailure(_) => "UPSTREAM_FAILURE",
            Self::GatewayTimeout(_) => "GATEWAY_TIMEOUT",
            Self::InternalError(_) | Self::ConfigError(_) => "INTERNAL_ERROR",
        }
    }

    /// Whether the client should consider this operation retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::UpstreamFailure(_) | Self::GatewayTimeout(_)
        )
    }

    /// Safe client-facing message that strips internal paths, credentials, and stack traces.
    pub fn client_message(&self) -> String {
        match self {
            Self::InvalidRequest(msg) => sanitize_message(msg),
            Self::Unauthorized(msg) => sanitize_message(msg),
            Self::Forbidden(msg) => sanitize_message(msg),
            Self::NotFound(msg) => sanitize_message(msg),
            Self::PayloadTooLarge => "Inbound payload exceeds 4.5 MB body limit".to_string(),
            Self::UnsupportedModality(msg) => sanitize_message(msg),
            Self::RateLimited { message, .. } => sanitize_message(message),
            Self::UpstreamFailure(_) => {
                "Upstream AI provider encountered an error. Operation may be retried.".to_string()
            }
            Self::GatewayTimeout(_) => {
                "Upstream request exceeded execution deadline (10s).".to_string()
            }
            Self::InternalError(_) | Self::ConfigError(_) => {
                "An internal server error occurred.".to_string()
            }
        }
    }

    /// Build typed ErrorEnvelope.
    pub fn to_envelope(&self, request_id: &str) -> ErrorEnvelope {
        let details = match self {
            Self::RateLimited {
                retry_after_secs: Some(secs),
                ..
            } => Some(serde_json::json!({ "retry_after_seconds": secs })),
            _ => None,
        };

        ErrorEnvelope {
            error: ErrorBody {
                code: self.error_code(),
                message: self.client_message(),
                retryable: self.is_retryable(),
                request_id: request_id.to_string(),
                details,
            },
        }
    }

    /// Format standardized JSON error envelope matching API_SPEC.md §1.3.
    pub fn to_json_envelope(&self, request_id: &str) -> String {
        serde_json::to_string(&self.to_envelope(request_id)).unwrap_or_else(|_| {
            r#"{"error":{"code":"INTERNAL_ERROR","message":"An internal error occurred","retryable":false,"request_id":"unknown","details":null}}"#.to_string()
        })
    }

    /// Format an Axum HTTP Response with the given request ID and x-request-id header.
    pub fn to_response_with_id(&self, request_id: &str) -> Response {
        let status = self.http_status();
        let envelope = self.to_envelope(request_id);
        let mut resp = (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            Json(envelope),
        )
            .into_response();

        if let Ok(hval) = header::HeaderValue::from_str(request_id) {
            resp.headers_mut()
                .insert(header::HeaderName::from_static("x-request-id"), hval);
        }
        resp
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.http_status();
        let envelope = self.to_envelope("req_auto");
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            Json(envelope),
        )
            .into_response()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} (status: {})",
            self.error_code(),
            self.client_message(),
            self.status_code()
        )
    }
}

impl std::error::Error for AppError {}

/// Sanitize user-visible message by removing any potential secret patterns.
fn sanitize_message(msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("bearer")
    {
        "Invalid authentication credentials or restricted parameter.".to_string()
    } else {
        msg.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_error_codes() {
        assert_eq!(
            AppError::InvalidRequest("bad data".into()).error_code(),
            "INVALID_REQUEST"
        );
        assert_eq!(
            AppError::Unauthorized("no token".into()).error_code(),
            "UNAUTHORIZED"
        );
        assert_eq!(
            AppError::Forbidden("no perm".into()).error_code(),
            "FORBIDDEN"
        );
        assert_eq!(
            AppError::NotFound("not found".into()).error_code(),
            "NOT_FOUND"
        );
        assert_eq!(AppError::PayloadTooLarge.error_code(), "PAYLOAD_TOO_LARGE");
        assert_eq!(
            AppError::UnsupportedModality("vision".into()).error_code(),
            "UNSUPPORTED_MODALITY"
        );
        assert_eq!(
            AppError::RateLimited {
                message: "slow down".into(),
                retry_after_secs: Some(5)
            }
            .error_code(),
            "RATE_LIMITED"
        );
        assert_eq!(
            AppError::UpstreamFailure("503".into()).error_code(),
            "UPSTREAM_FAILURE"
        );
        assert_eq!(
            AppError::GatewayTimeout("10s".into()).error_code(),
            "GATEWAY_TIMEOUT"
        );
        assert_eq!(
            AppError::InternalError("panic".into()).error_code(),
            "INTERNAL_ERROR"
        );
        assert_eq!(
            AppError::ConfigError("missing port".into()).error_code(),
            "INTERNAL_ERROR"
        );
    }

    #[test]
    fn test_status_code_mappings() {
        assert_eq!(AppError::InvalidRequest("".into()).status_code(), 400);
        assert_eq!(AppError::Unauthorized("".into()).status_code(), 401);
        assert_eq!(AppError::Forbidden("".into()).status_code(), 403);
        assert_eq!(AppError::NotFound("".into()).status_code(), 404);
        assert_eq!(AppError::PayloadTooLarge.status_code(), 413);
        assert_eq!(AppError::UnsupportedModality("".into()).status_code(), 422);
        assert_eq!(
            AppError::RateLimited {
                message: "".into(),
                retry_after_secs: None
            }
            .status_code(),
            429
        );
        assert_eq!(AppError::UpstreamFailure("".into()).status_code(), 502);
        assert_eq!(AppError::GatewayTimeout("".into()).status_code(), 504);
        assert_eq!(AppError::InternalError("".into()).status_code(), 500);
        assert_eq!(AppError::ConfigError("".into()).status_code(), 500);
    }

    #[test]
    fn test_retryable_flags() {
        assert!(!AppError::InvalidRequest("".into()).is_retryable());
        assert!(!AppError::Unauthorized("".into()).is_retryable());
        assert!(!AppError::NotFound("".into()).is_retryable());
        assert!(!AppError::InternalError("".into()).is_retryable());

        assert!(
            AppError::RateLimited {
                message: "".into(),
                retry_after_secs: None
            }
            .is_retryable()
        );
        assert!(AppError::UpstreamFailure("".into()).is_retryable());
        assert!(AppError::GatewayTimeout("".into()).is_retryable());
    }

    #[test]
    fn test_internal_error_sanitization() {
        let err =
            AppError::InternalError("DB password 'super_secret' failed at /etc/shadow".into());
        let client_msg = err.client_message();
        assert_eq!(client_msg, "An internal server error occurred.");
        assert!(!client_msg.contains("password"));
        assert!(!client_msg.contains("/etc/shadow"));
    }

    #[test]
    fn test_secret_sanitization_in_user_errors() {
        let err = AppError::InvalidRequest("Invalid api_key provided: sk-live-123456789".into());
        let client_msg = err.client_message();
        assert_eq!(
            client_msg,
            "Invalid authentication credentials or restricted parameter."
        );
        assert!(!client_msg.contains("sk-live"));
    }

    #[test]
    fn test_json_envelope_format() {
        let err = AppError::RateLimited {
            message: "Provider rate limit exceeded.".into(),
            retry_after_secs: Some(10),
        };
        let envelope = err.to_json_envelope("req_test_123");
        assert!(envelope.contains(r#""code":"RATE_LIMITED""#));
        assert!(envelope.contains(r#""retryable":true"#));
        assert!(envelope.contains(r#""request_id":"req_test_123""#));
        assert!(envelope.contains(r#""retry_after_seconds":10"#));
    }
}
