//! Provider Quota and Rate-Limit Awareness module.
//!
//! Conforms strictly to Phase 8 specifications:
//! - Local usage accounting is strictly isolated from provider quota information.
//! - Missing headers mean unknown, NOT unlimited.
//! - Conservative routing integration: manual model selection is always preserved.

pub mod model;
pub mod service;

pub use model::{ProviderQuota, QuotaLimitType, QuotaLimitUnit, QuotaObservation, QuotaSource};
pub use service::QuotaService;
