//! Usage tracking and analytics module.
//!
//! Conforms strictly to Phase 8 specifications for accurate integer token accounting,
//! persistent SQLite telemetry storage, and fail-safe recording.

pub mod maintenance;
pub mod model;
pub mod service;
pub mod spool;
pub mod storage;

pub use maintenance::{MaintenanceRunSummary, RetentionMaintenanceWorker};
pub use model::{
    ModelUsageSummary, ProviderUsageSummary, RecordStatus, RequestMode, RequestOutcome,
    RetryDrainResult, RetryQueueMetrics, RoutingMode, UsageQueryFilter, UsageRecord, UsageSource,
    UsageSummary,
};
pub use service::{CombinedDrainResult, UsageService};
pub use spool::{SpoolDrainResult, SpoolEntry, SpoolError, SpoolMetrics, UsageSpooler};
pub use storage::{StorageError, UsageStorage};
