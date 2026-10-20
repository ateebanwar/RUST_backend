//! Usage tracking and analytics module.
//!
//! Conforms strictly to Phase 8 specifications for accurate integer token accounting,
//! persistent SQLite telemetry storage, and fail-safe recording.

pub mod model;
pub mod service;
pub mod storage;

pub use model::{
    ModelUsageSummary, ProviderUsageSummary, RecordStatus, RequestMode, RequestOutcome,
    RoutingMode, UsageQueryFilter, UsageRecord, UsageSource, UsageSummary,
};
pub use service::UsageService;
pub use storage::{StorageError, UsageStorage};
