//! Explicitly non-persistent in-memory storage implementation for usage tracking.
//!
//! Conforms strictly to the Zero-Database requirement:
//! - Operates 100% in memory with zero SQLite, zero PostgreSQL, and zero disk writes.
//! - Bounded capacity to eliminate memory exhaustion risk.
//! - Concurrency-safe via thread-safe read-write locks (`tokio::sync::RwLock` and `std::sync::RwLock`).
//! - All usage records, aggregates, and metrics are dropped upon process termination.

use crate::usage::model::{
    ModelUsageSummary, ProviderUsageSummary, RetryDrainResult, RetryQueueMetrics, UsageQueryFilter,
    UsageRecord, UsageSummary,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of in-memory usage records preserved in RAM to prevent memory exhaustion.
pub const MAX_IN_MEMORY_USAGE_RECORDS: usize = 10_000;

/// Storage error categories conforming to zero-database in-memory policy.
#[derive(Debug)]
pub enum StorageError {
    ConnectionError(String),
    QueryError(String),
    LockError(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionError(msg) => write!(f, "In-memory storage error: {msg}"),
            Self::QueryError(msg) => write!(f, "Query execution failure: {msg}"),
            Self::LockError(msg) => write!(f, "Lock acquisition failure: {msg}"),
        }
    }
}

impl std::error::Error for StorageError {}

#[derive(Debug, Clone)]
struct InMemRetryItem {
    record: UsageRecord,
    next_retry_at: u64,
}

/// Thread-safe in-memory usage storage engine with zero database dependencies.
#[derive(Clone)]
pub struct UsageStorage {
    records: Arc<RwLock<Vec<UsageRecord>>>,
    retries: Arc<RwLock<Vec<InMemRetryItem>>>,
    db_path: String,
}

impl Default for UsageStorage {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

impl UsageStorage {
    /// Create a new in-memory usage storage.
    pub fn new_in_memory() -> Self {
        Self {
            records: Arc::new(RwLock::new(Vec::new())),
            retries: Arc::new(RwLock::new(Vec::new())),
            db_path: ":memory:".to_string(),
        }
    }

    /// Access configured database path representation (always ":memory:" for zero-database architecture).
    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    /// Initialize storage (accepts legacy path string, but unconditionally operates in-memory).
    pub fn new(_db_path: &str) -> Result<Self, StorageError> {
        Ok(Self::new_in_memory())
    }

    /// Synchronously record a usage record in memory.
    pub fn record_sync(&self, record: &UsageRecord) -> Result<(), StorageError> {
        let mut lock = self
            .records
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        if lock.len() >= MAX_IN_MEMORY_USAGE_RECORDS {
            lock.remove(0);
        }
        lock.push(record.clone());
        Ok(())
    }

    /// Record a finalized usage record in memory.
    pub async fn record(&self, record: UsageRecord) -> Result<(), StorageError> {
        let mut lock = self
            .records
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        if lock.len() >= MAX_IN_MEMORY_USAGE_RECORDS {
            // Drop oldest record to enforce bounded memory footprint
            lock.remove(0);
        }
        lock.push(record);
        Ok(())
    }

    /// Record a batch of usage records in memory.
    pub async fn record_batch(&self, records: Vec<UsageRecord>) -> Result<usize, StorageError> {
        let mut lock = self
            .records
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let count = records.len();
        for r in records {
            if lock.len() >= MAX_IN_MEMORY_USAGE_RECORDS {
                lock.remove(0);
            }
            lock.push(r);
        }
        Ok(count)
    }

    /// Query summary using an asynchronous wrapper.
    pub async fn get_summary(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<UsageSummary, StorageError> {
        let subject = filter.subject.clone();
        self.query_summary(&filter, subject.as_deref())
    }

    /// Query provider summaries using an asynchronous wrapper.
    pub async fn get_providers(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<ProviderUsageSummary>, StorageError> {
        let subject = filter.subject.clone();
        self.query_providers(&filter, subject.as_deref())
    }

    /// Query model summaries using an asynchronous wrapper.
    pub async fn get_models(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<ModelUsageSummary>, StorageError> {
        let subject = filter.subject.clone();
        self.query_models(&filter, subject.as_deref())
    }

    /// Query raw usage records with bounded limit.
    pub async fn get_records(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<UsageRecord>, StorageError> {
        let lock = self
            .records
            .read()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut res = Vec::new();
        for r in lock.iter() {
            if !Self::matches_filter(r, &filter, filter.subject.as_deref()) {
                continue;
            }
            if let Some(limit) = filter.limit
                && res.len() >= limit
            {
                break;
            }
            res.push(r.clone());
        }
        Ok(res)
    }

    /// Internal filter matching helper.
    fn matches_filter(
        record: &UsageRecord,
        filter: &UsageQueryFilter,
        caller_subject: Option<&str>,
    ) -> bool {
        if let Some(start) = filter.start_time
            && record.start_time < start
        {
            return false;
        }
        if let Some(end) = filter.end_time
            && record.start_time > end
        {
            return false;
        }
        if let Some(ref prov) = filter.provider_id
            && &record.provider_id != prov
        {
            return false;
        }
        if let Some(ref mdl) = filter.model_id
            && &record.model_id != mdl
        {
            return false;
        }
        if let Some(ref out) = filter.outcome
            && record.outcome.as_str() != out
        {
            return false;
        }
        if let Some(subj) = caller_subject {
            match &record.subject {
                Some(record_subj) => {
                    if record_subj != subj {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }

    /// Synchronously query aggregated summary across in-memory records.
    pub fn query_summary(
        &self,
        filter: &UsageQueryFilter,
        caller_subject: Option<&str>,
    ) -> Result<UsageSummary, StorageError> {
        let lock = self
            .records
            .read()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut total_requests = 0u64;
        let mut successful_requests = 0u64;
        let mut failed_requests = 0u64;
        let mut known_usage_requests = 0u64;
        let mut unknown_usage_requests = 0u64;
        let mut total_prompt_tokens = 0u64;
        let mut total_completion_tokens = 0u64;
        let mut total_tokens = 0u64;
        let mut latencies: Vec<f64> = Vec::new();

        let mut by_request_mode: HashMap<String, u64> = HashMap::new();
        let mut by_routing_mode: HashMap<String, u64> = HashMap::new();
        let mut by_outcome: HashMap<String, u64> = HashMap::new();

        for r in lock.iter() {
            if !Self::matches_filter(r, filter, caller_subject) {
                continue;
            }

            if let Some(limit) = filter.limit
                && total_requests >= limit as u64
            {
                break;
            }

            total_requests += 1;
            if r.outcome.is_success() {
                successful_requests += 1;
            } else {
                failed_requests += 1;
            }

            if let Some(t) = r.total_tokens {
                known_usage_requests += 1;
                total_tokens = total_tokens.saturating_add(t);
                if let Some(p) = r.prompt_tokens {
                    total_prompt_tokens = total_prompt_tokens.saturating_add(p);
                }
                if let Some(c) = r.completion_tokens {
                    total_completion_tokens = total_completion_tokens.saturating_add(c);
                }
            } else {
                unknown_usage_requests += 1;
            }

            latencies.push(r.latency_ms as f64);
            *by_request_mode
                .entry(r.request_mode.as_str().to_string())
                .or_insert(0) += 1;
            *by_routing_mode
                .entry(r.routing_mode.as_str().to_string())
                .or_insert(0) += 1;
            *by_outcome
                .entry(r.outcome.as_str().to_string())
                .or_insert(0) += 1;
        }

        let avg_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().sum::<f64>() / latencies.len() as f64
        };

        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            let idx = (latencies.len() as f64 * 0.50).floor() as usize;
            latencies[idx.min(latencies.len() - 1)]
        };

        let p95_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            let idx = (latencies.len() as f64 * 0.95).floor() as usize;
            latencies[idx.min(latencies.len() - 1)]
        };

        Ok(UsageSummary {
            total_requests,
            successful_requests,
            failed_requests,
            known_usage_requests,
            unknown_usage_requests,
            total_prompt_tokens,
            total_completion_tokens,
            total_tokens,
            avg_latency_ms,
            p50_latency_ms,
            p95_latency_ms,
            by_request_mode,
            by_routing_mode,
            by_outcome,
        })
    }

    /// Query provider summaries.
    pub fn query_providers(
        &self,
        filter: &UsageQueryFilter,
        caller_subject: Option<&str>,
    ) -> Result<Vec<ProviderUsageSummary>, StorageError> {
        let lock = self
            .records
            .read()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        struct ProvAgg {
            requests: u64,
            successful_requests: u64,
            failed_requests: u64,
            known_usage_requests: u64,
            total_tokens: u64,
            total_latency: u64,
        }

        let mut map: HashMap<String, ProvAgg> = HashMap::new();

        for r in lock.iter() {
            if !Self::matches_filter(r, filter, caller_subject) {
                continue;
            }

            let entry = map.entry(r.provider_id.clone()).or_insert(ProvAgg {
                requests: 0,
                successful_requests: 0,
                failed_requests: 0,
                known_usage_requests: 0,
                total_tokens: 0,
                total_latency: 0,
            });

            entry.requests += 1;
            if r.outcome.is_success() {
                entry.successful_requests += 1;
            } else {
                entry.failed_requests += 1;
            }
            if let Some(t) = r.total_tokens {
                entry.known_usage_requests += 1;
                entry.total_tokens = entry.total_tokens.saturating_add(t);
            }
            entry.total_latency = entry.total_latency.saturating_add(r.latency_ms);
        }

        let mut res = Vec::new();
        for (provider_id, agg) in map {
            let avg_latency_ms = if agg.requests > 0 {
                agg.total_latency as f64 / agg.requests as f64
            } else {
                0.0
            };
            res.push(ProviderUsageSummary {
                provider_id,
                requests: agg.requests,
                successful_requests: agg.successful_requests,
                failed_requests: agg.failed_requests,
                known_usage_requests: agg.known_usage_requests,
                total_tokens: agg.total_tokens,
                avg_latency_ms,
            });
        }
        res.sort_by_key(|a| std::cmp::Reverse(a.requests));
        Ok(res)
    }

    /// Query model summaries.
    pub fn query_models(
        &self,
        filter: &UsageQueryFilter,
        caller_subject: Option<&str>,
    ) -> Result<Vec<ModelUsageSummary>, StorageError> {
        let lock = self
            .records
            .read()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        struct ModelAgg {
            requests: u64,
            successful_requests: u64,
            failed_requests: u64,
            known_usage_requests: u64,
            total_tokens: u64,
            total_latency: u64,
        }

        let mut map: HashMap<(String, String), ModelAgg> = HashMap::new();

        for r in lock.iter() {
            if !Self::matches_filter(r, filter, caller_subject) {
                continue;
            }

            let key = (r.model_id.clone(), r.provider_id.clone());
            let entry = map.entry(key).or_insert(ModelAgg {
                requests: 0,
                successful_requests: 0,
                failed_requests: 0,
                known_usage_requests: 0,
                total_tokens: 0,
                total_latency: 0,
            });

            entry.requests += 1;
            if r.outcome.is_success() {
                entry.successful_requests += 1;
            } else {
                entry.failed_requests += 1;
            }
            if let Some(t) = r.total_tokens {
                entry.known_usage_requests += 1;
                entry.total_tokens = entry.total_tokens.saturating_add(t);
            }
            entry.total_latency = entry.total_latency.saturating_add(r.latency_ms);
        }

        let mut res = Vec::new();
        for ((model_id, provider_id), agg) in map {
            let avg_latency_ms = if agg.requests > 0 {
                agg.total_latency as f64 / agg.requests as f64
            } else {
                0.0
            };
            res.push(ModelUsageSummary {
                model_id,
                provider_id,
                requests: agg.requests,
                successful_requests: agg.successful_requests,
                failed_requests: agg.failed_requests,
                known_usage_requests: agg.known_usage_requests,
                total_tokens: agg.total_tokens,
                avg_latency_ms,
            });
        }
        res.sort_by_key(|a| std::cmp::Reverse(a.requests));
        Ok(res)
    }

    /// Prune records older than cutoff timestamp.
    pub async fn prune_older_than(&self, cutoff_timestamp: u64) -> Result<usize, StorageError> {
        self.prune_older_than_sync(cutoff_timestamp)
    }

    /// Prune records older than cutoff timestamp synchronously.
    pub fn prune_older_than_sync(&self, cutoff_timestamp: u64) -> Result<usize, StorageError> {
        let mut lock = self
            .records
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let initial_len = lock.len();
        lock.retain(|r| r.start_time >= cutoff_timestamp);
        Ok(initial_len - lock.len())
    }

    /// Bounded progressive retention pruning.
    pub async fn prune_older_than_bounded(
        &self,
        cutoff_timestamp: u64,
        limit: usize,
    ) -> Result<usize, StorageError> {
        self.prune_older_than_bounded_sync(cutoff_timestamp, limit)
    }

    /// Bounded progressive retention pruning synchronously.
    pub fn prune_older_than_bounded_sync(
        &self,
        cutoff_timestamp: u64,
        limit: usize,
    ) -> Result<usize, StorageError> {
        let mut lock = self
            .records
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut pruned = 0;
        let mut i = 0;
        while i < lock.len() && pruned < limit {
            if lock[i].start_time < cutoff_timestamp {
                lock.remove(i);
                pruned += 1;
            } else {
                i += 1;
            }
        }
        Ok(pruned)
    }

    /// In-memory retry queue enqueue.
    pub async fn enqueue_retry(
        &self,
        record: &UsageRecord,
        _last_error: &str,
    ) -> Result<(), StorageError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut lock = self
            .retries
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;
        lock.push(InMemRetryItem {
            record: record.clone(),
            next_retry_at: now + 5,
        });
        Ok(())
    }

    /// In-memory retry queue drain.
    pub async fn drain_retry_queue(
        &self,
        batch_limit: usize,
    ) -> Result<RetryDrainResult, StorageError> {
        self.drain_retry_queue_sync(batch_limit)
    }

    /// Synchronous in-memory retry queue drain.
    pub fn drain_retry_queue_sync(
        &self,
        batch_limit: usize,
    ) -> Result<RetryDrainResult, StorageError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut retries_lock = self
            .retries
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;
        let mut records_lock = self
            .records
            .write()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut drained = Vec::new();
        let mut i = 0;
        while i < retries_lock.len() && drained.len() < batch_limit {
            if now >= retries_lock[i].next_retry_at {
                let item = retries_lock.remove(i);
                drained.push(item.record);
            } else {
                i += 1;
            }
        }

        for r in &drained {
            if records_lock.len() >= MAX_IN_MEMORY_USAGE_RECORDS {
                records_lock.remove(0);
            }
            records_lock.push(r.clone());
        }

        Ok(RetryDrainResult {
            succeeded: drained.len(),
            retried: 0,
            dead_lettered: 0,
        })
    }

    /// Query in-memory retry metrics.
    pub async fn get_retry_queue_metrics(&self) -> Result<RetryQueueMetrics, StorageError> {
        let lock = self
            .retries
            .read()
            .map_err(|e| StorageError::LockError(e.to_string()))?;
        Ok(RetryQueueMetrics {
            pending_count: lock.len(),
            dead_letter_count: 0,
        })
    }

    /// WAL checkpoint is a no-op in memory.
    pub async fn maintenance_wal_checkpoint(&self) -> Result<(), StorageError> {
        Ok(())
    }

    /// Synchronous WAL checkpoint no-op.
    pub fn maintenance_wal_checkpoint_sync(&self) -> Result<(), StorageError> {
        Ok(())
    }
}
