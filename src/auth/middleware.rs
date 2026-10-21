//! Unified Authentication and Request Hardening Middleware.
//!
//! Enforces:
//! 1. Request ID resolution and response correlation (`X-Request-ID`).
//! 2. HTTP Host header poisoning defense (`ALLOWED_HOSTS`).
//! 3. Forwarded header stripping from untrusted direct network peers (`TRUSTED_PROXIES`).
//! 4. Multi-client authentication (Google OIDC and PAT/API-Key).
//! 5. Downstream `AuthPrincipal` identity injection into request extensions.

use crate::errors::AppError;
use crate::{AppState, RequestId};
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Axum middleware function enforcing authentication, request ID tracing, and proxy security.
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    // 1. Resolve or generate Request ID (API_SPEC.md §1.1)
    let request_id_str = if let Some(header_val) = req.headers().get("x-request-id") {
        if let Ok(s) = header_val.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty()
                && trimmed.len() <= 128
                && trimmed
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                trimmed.to_string()
            } else {
                format!(
                    "req_{:012x}",
                    REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
                )
            }
        } else {
            format!(
                "req_{:012x}",
                REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            )
        }
    } else {
        format!(
            "req_{:012x}",
            REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    };

    req.extensions_mut()
        .insert(RequestId(request_id_str.clone()));

    // 2. Host Header Validation
    if !state.config.allowed_hosts.is_empty() {
        let host_val = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if !state.config.host_validator().validate_host(host_val) {
            let err = AppError::InvalidRequest("Invalid or untrusted Host header".to_string());
            return err.to_response_with_id(&request_id_str);
        }
    }

    // 3. Forwarded Header Protection against spoofing from untrusted peers
    let peer_ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip());

    let is_trusted_peer = match peer_ip {
        Some(ip) => state.config.trusted_proxy_matcher().is_trusted(&ip),
        None => false,
    };

    if !is_trusted_peer {
        req.headers_mut().remove("x-forwarded-for");
        req.headers_mut().remove("forwarded");
        req.headers_mut().remove("x-forwarded-proto");
        req.headers_mut().remove("x-forwarded-host");
    }

    // 4. Authenticate request via AuthService
    let path = req.uri().path();
    let query = req.uri().query();
    match state.auth.authenticate(req.headers(), path, query).await {
        Ok(Some(principal)) => {
            // 5. Authorize request against route required scope
            if let Err(authz_err) = crate::auth::AuthService::authorize(&principal, path) {
                return authz_err.to_response_with_id(&request_id_str);
            }
            req.extensions_mut().insert(principal);
        }
        Ok(None) => {
            // Intentionally unauthenticated public probe (/health or /ready)
        }
        Err(err) => {
            return err.to_response_with_id(&request_id_str);
        }
    }

    let mut response = next.run(req).await;

    // Attach X-Request-ID to response headers
    if let Ok(hdr_val) = HeaderValue::from_str(&request_id_str) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), hdr_val);
    }

    response
}
