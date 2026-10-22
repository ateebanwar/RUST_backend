//! High-level usage tracking service.
//!
//! Provides the primary application interface for recording inference request outcomes
//! and querying aggregated telemetry while enforcing safe failure isolation.
//! Conforms strictly to zero-database architecture (100% in-memory telemetry).

use crate::models::UsageMetadata;
use crate::usage::model::{
    ModelUsageSummary, ProviderUsageSummary, RecordStatus, RequestMode, RequestOutcome,
    RetryDrainResult, RetryQueueMetrics, RoutingMode, UsageQueryFilter, UsageRecord, UsageSummary,
};
use crate::usage::storage::{StorageError, UsageStorage};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Combined drain results representing in-memory retry drain.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CombinedDrainResult {
    pub sqlite: RetryDrainResult,
}

impl CombinedDrainResult {
    pub fn total_succeeded(&self) -> usize {
        self.sqlite.succeeded
    }
}

/// Thread-safe service for usage recording and analytics (100% in-memory).
#[derive(Clone)]
pub struct UsageService {
    storage: Arc<UsageStorage>,
}

impl UsageService {
    /// Initialize with an in-memory storage handle.
    pub fn new(storage: Arc<UsageStorage>) -> Self {
        Self { storage }
    }

    /// Access reference to underlying storage engine.
    pub fn storage(&self) -> &Arc<UsageStorage> {
        &self.storage
    }

    /// Record a buffered completion attempt in memory.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_buffered(
        &self,
        request_id: &str,
        provider_id: &str,
        model_id: &str,
        routing_mode: RoutingMode,
        start_instant: Instant,
        start_epoch_ms: u64,
        outcome: RequestOutcome,
        failure_category: Option<String>,
        usage: Option<&UsageMetadata>,
        subject: Option<&str>,
    ) {
        let latency_ms = start_instant.elapsed().as_millis() as u64;
        let completion_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let (prompt_tokens, completion_tokens, total_tokens, usage_source) = match usage {
            Some(u) => UsageRecord::normalize_tokens(
                Some(u.prompt_tokens),
                Some(u.completion_tokens),
                Some(u.total_tokens),
            ),
            None => UsageRecord::normalize_tokens(None, None, None),
        };

        let record = UsageRecord {
            id: request_id.to_string(),
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            request_mode: RequestMode::Buffered,
            routing_mode,
            start_time: start_epoch_ms,
            completion_time,
            latency_ms,
            outcome,
            failure_category,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            usage_source,
            record_status: RecordStatus::Final,
            subject: subject.map(ToString::to_string),
        };

        if let Err(err) = self.storage.record(record.clone()).await {
            warn!(
                request_id = %request_id,
                error = %err,
                "UsageService: In-memory record write failed (non-fatal), falling back to retry buffer"
            );
            let _ = self.storage.enqueue_retry(&record, &err.to_string()).await;
        }
    }

    /// Record a streaming completion finalization in memory.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_streaming(
        &self,
        request_id: &str,
        provider_id: &str,
        model_id: &str,
        routing_mode: RoutingMode,
        start_instant: Instant,
        start_epoch_ms: u64,
        outcome: RequestOutcome,
        failure_category: Option<String>,
        usage: Option<&UsageMetadata>,
        subject: Option<&str>,
    ) {
        let latency_ms = start_instant.elapsed().as_millis() as u64;
        let completion_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let (prompt_tokens, completion_tokens, total_tokens, usage_source) = match usage {
            Some(u) => UsageRecord::normalize_tokens(
                Some(u.prompt_tokens),
                Some(u.completion_tokens),
                Some(u.total_tokens),
            ),
            None => UsageRecord::normalize_tokens(None, None, None),
        };

        let record = UsageRecord {
            id: request_id.to_string(),
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            request_mode: RequestMode::Streaming,
            routing_mode,
            start_time: start_epoch_ms,
            completion_time,
            latency_ms,
            outcome,
            failure_category,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            usage_source,
            record_status: RecordStatus::Final,
            subject: subject.map(ToString::to_string),
        };

        if let Err(err) = self.storage.record(record.clone()).await {
            warn!(
                request_id = %request_id,
                error = %err,
                "UsageService: In-memory record write failed (non-fatal), falling back to retry buffer"
            );
            let _ = self.storage.enqueue_retry(&record, &err.to_string()).await;
        }
    }

    /// Query high-level usage summary.
    pub async fn get_summary(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<UsageSummary, StorageError> {
        self.storage.get_summary(filter).await
    }

    /// Query provider usage breakdowns.
    pub async fn get_providers(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<ProviderUsageSummary>, StorageError> {
        self.storage.get_providers(filter).await
    }

    /// Query model usage breakdowns.
    pub async fn get_models(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<ModelUsageSummary>, StorageError> {
        self.storage.get_models(filter).await
    }

    /// Query raw usage records with bounded limit.
    pub async fn get_records(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<UsageRecord>, StorageError> {
        self.storage.get_records(filter).await
    }

    /// Prune records older than the specified retention days.
    /// Returns the number of pruned records.
    pub async fn prune_retention(&self, days: u64) -> Result<usize, StorageError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let retention_ms = days.saturating_mul(86_400_000);
        let cutoff_ms = now_ms.saturating_sub(retention_ms);
        self.storage.prune_older_than(cutoff_ms).await
    }

    /// Synchronous retention pruning for startup maintenance.
    pub fn prune_retention_sync(&self, days: u64) -> Result<usize, StorageError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let retention_ms = days.saturating_mul(86_400_000);
        let cutoff_ms = now_ms.saturating_sub(retention_ms);
        self.storage.prune_older_than_sync(cutoff_ms)
    }

    /// Prune records older than the specified retention days using bounded batches.
    pub async fn prune_retention_bounded(
        &self,
        days: u64,
        batch_limit: usize,
    ) -> Result<usize, StorageError> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let retention_ms = days.saturating_mul(86_400_000);
        let cutoff_ms = now_ms.saturating_sub(retention_ms);
        self.storage
            .prune_older_than_bounded(cutoff_ms, batch_limit)
            .await
    }

    /// Drain pending retries from the durable retry queue.
    pub async fn drain_retries(
        &self,
        batch_limit: usize,
    ) -> Result<RetryDrainResult, StorageError> {
        self.storage.drain_retry_queue(batch_limit).await
    }

    /// Query operational metrics for the durable retry queue.
    pub async fn get_retry_queue_metrics(&self) -> Result<RetryQueueMetrics, StorageError> {
        self.storage.get_retry_queue_metrics().await
    }

    /// Run non-blocking WAL maintenance checkpoint (no-op in zero-database architecture).
    pub async fn maintenance_wal_checkpoint(&self) -> Result<(), StorageError> {
        self.storage.maintenance_wal_checkpoint().await
    }

    /// Drain pending retries.
    pub async fn drain_all_retries(
        &self,
        batch_limit: usize,
    ) -> Result<CombinedDrainResult, StorageError> {
        let sqlite_res = self.storage.drain_retry_queue(batch_limit).await?;
        Ok(CombinedDrainResult { sqlite: sqlite_res })
    }
}
