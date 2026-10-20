//! High-level usage tracking service.
//!
//! Provides the primary application interface for recording inference request outcomes
//! and querying aggregated telemetry while enforcing safe failure isolation.

use crate::models::UsageMetadata;
use crate::usage::model::{
    ModelUsageSummary, ProviderUsageSummary, RecordStatus, RequestMode, RequestOutcome,
    RetryDrainResult, RetryQueueMetrics, RoutingMode, UsageQueryFilter, UsageRecord, UsageSummary,
};
use crate::usage::storage::{StorageError, UsageStorage};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Thread-safe service for usage recording and analytics.
#[derive(Clone)]
pub struct UsageService {
    storage: Arc<UsageStorage>,
}

impl UsageService {
    /// Initialize with an existing storage handle.
    pub fn new(storage: Arc<UsageStorage>) -> Self {
        Self { storage }
    }

    /// Record a buffered completion attempt.
    ///
    /// In accordance with Phase 8 failure isolation policy:
    /// Storage write errors are logged as warnings and NEVER bubble up to fail the AI request.
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
        };

        if let Err(err) = self.storage.record(record.clone()).await {
            warn!(
                request_id = %request_id,
                error = %err,
                "UsageService: Failed to record buffered usage record (non-fatal), enqueuing to durable retry queue"
            );
            if let Err(enq_err) = self.storage.enqueue_retry(&record, &err.to_string()).await {
                warn!(
                    request_id = %request_id,
                    error = %enq_err,
                    "UsageService: Failed to enqueue record to durable retry queue"
                );
            }
        }
    }

    /// Record a streaming completion finalization.
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
        };

        if let Err(err) = self.storage.record(record.clone()).await {
            warn!(
                request_id = %request_id,
                error = %err,
                "UsageService: Failed to record streaming usage record (non-fatal), enqueuing to durable retry queue"
            );
            if let Err(enq_err) = self.storage.enqueue_retry(&record, &err.to_string()).await {
                warn!(
                    request_id = %request_id,
                    error = %enq_err,
                    "UsageService: Failed to enqueue record to durable retry queue"
                );
            }
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

    /// Run non-blocking WAL maintenance checkpoint.
    pub async fn maintenance_wal_checkpoint(&self) -> Result<(), StorageError> {
        self.storage.maintenance_wal_checkpoint().await
    }
}
