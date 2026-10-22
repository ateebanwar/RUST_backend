//! Usage tracking and analytics module.
//!
//! Conforms strictly to zero-database specifications:
//! - Operates 100% in-memory with zero SQLite, zero disk files, zero spool files.
//! - Accurate integer token accounting and non-persistent telemetry.

pub mod maintenance;
pub mod model;
pub mod service;
pub mod storage;

pub use maintenance::{MaintenanceRunSummary, RetentionMaintenanceWorker};
pub use model::{
    ModelUsageSummary, ProviderUsageSummary, RecordStatus, RequestMode, RequestOutcome,
    RetryDrainResult, RetryQueueMetrics, RoutingMode, UsageQueryFilter, UsageRecord, UsageSource,
    UsageSummary,
};
pub use service::{CombinedDrainResult, UsageService};
pub use storage::{StorageError, UsageStorage};
