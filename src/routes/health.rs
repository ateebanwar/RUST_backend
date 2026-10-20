//! Health and readiness probe handlers and response schemas.
//!
//! Conforms strictly to API_SPEC.md §2.1 (Liveness) and §2.2 (Readiness).

use crate::AppState;
use crate::config::AppConfig;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Liveness response matching API_SPEC.md §2.1.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub timestamp: String,
}

impl HealthResponse {
    pub fn new() -> Self {
        Self {
            status: "healthy",
            version: env!("CARGO_PKG_VERSION"),
            timestamp: rfc3339_now(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"status":"{}","version":"{}","timestamp":"{}"}}"#,
                self.status, self.version, self.timestamp
            )
        })
    }
}

impl Default for HealthResponse {
    fn default() -> Self {
        Self::new()
    }
}

/// Readiness response matching API_SPEC.md §2.2.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReadyResponse {
    pub status: &'static str,
    pub active_providers: usize,
    pub registered_models: usize,
    pub deployment_mode: &'static str,
}

impl ReadyResponse {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            status: "not_ready",
            active_providers: 0,
            registered_models: 0,
            deployment_mode: config.deployment_mode.as_str(),
        }
    }

    pub fn new_with_counts(
        config: &AppConfig,
        active_providers: usize,
        registered_models: usize,
    ) -> Self {
        let status = if active_providers > 0 && registered_models > 0 {
            "ready"
        } else {
            "not_ready"
        };
        Self {
            status,
            active_providers,
            registered_models,
            deployment_mode: config.deployment_mode.as_str(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"status":"{}","active_providers":{},"registered_models":{},"deployment_mode":"{}"}}"#,
                self.status, self.active_providers, self.registered_models, self.deployment_mode
            )
        })
    }
}

/// Axum handler for GET /api/v1/health (Liveness probe).
pub async fn health_check() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse::new()))
}

/// Axum handler for GET /api/v1/ready (Readiness probe).
///
/// Conforms to API_SPEC.md §2.2:
/// - Returns HTTP 200 OK with `status: "ready"` if at least one operational provider
///   and operational model are available.
/// - Returns HTTP 503 Service Unavailable with `status: "not_ready"` if zero providers
///   or zero operational models are available.
pub async fn readiness_check(State(state): State<AppState>) -> (StatusCode, Json<ReadyResponse>) {
    let active_ids = state.providers.active_provider_ids();
    let active_provs = active_ids.len();
    let reg_models = if active_provs > 0 {
        state
            .registry
            .list_models(None, None, None)
            .into_iter()
            .filter(|m| m.available && active_ids.contains(&m.provider_id))
            .count()
    } else {
        0
    };

    let is_ready = active_provs > 0 && reg_models > 0;
    let status_code = if is_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status_code,
        Json(ReadyResponse::new_with_counts(
            &state.config,
            active_provs,
            reg_models,
        )),
    )
}

/// Generate RFC 3339 UTC timestamp without third-party crates.
fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Days since epoch
    let days = (secs / 86400) as i64;
    let time_of_day = (secs % 86400) as u32;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Convert days to civil date (simplified Gregorian algorithm)
    let (year, month, day) = days_to_date(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_date(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response_json() {
        let health = HealthResponse::new();
        assert_eq!(health.status, "healthy");
        assert_eq!(health.version, "0.1.0");
        assert!(health.timestamp.ends_with('Z'));

        let json = health.to_json();
        assert!(json.contains(r#""status":"healthy""#));
        assert!(json.contains(r#""version":"0.1.0""#));
        assert!(json.contains(r#""timestamp":""#));
    }

    #[test]
    fn test_ready_response_json() {
        let config = AppConfig::default_local();
        let not_ready = ReadyResponse::new(&config);
        assert_eq!(not_ready.status, "not_ready");
        assert_eq!(not_ready.active_providers, 0);
        assert_eq!(not_ready.registered_models, 0);
        assert_eq!(not_ready.deployment_mode, "daemon");

        let json = not_ready.to_json();
        assert_eq!(
            json,
            r#"{"status":"not_ready","active_providers":0,"registered_models":0,"deployment_mode":"daemon"}"#
        );

        let ready = ReadyResponse::new_with_counts(&config, 1, 3);
        assert_eq!(ready.status, "ready");
        assert_eq!(ready.active_providers, 1);
        assert_eq!(ready.registered_models, 3);
        assert_eq!(
            ready.to_json(),
            r#"{"status":"ready","active_providers":1,"registered_models":3,"deployment_mode":"daemon"}"#
        );
    }

    #[test]
    fn test_timestamp_rfc3339_format() {
        let ts = rfc3339_now();
        assert_eq!(ts.len(), 20); // e.g. "2026-10-19T17:00:00Z"
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
    }
}
