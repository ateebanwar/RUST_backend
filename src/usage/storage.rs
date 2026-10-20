//! Persistent SQLite storage implementation for usage tracking.
//!
//! Provides concurrency-safe, parameterized SQLite storage with repeatable schema migrations,
//! robust index coverage, and asynchronous execution offloaded from Tokio worker threads.

use crate::usage::model::{
    ModelUsageSummary, ProviderUsageSummary, RecordStatus, RequestMode, RequestOutcome,
    RoutingMode, UsageQueryFilter, UsageRecord, UsageSource, UsageSummary,
};
use rusqlite::{Connection, OpenFlags, params};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// Storage error categories conforming to Phase 8 failure policies.
#[derive(Debug)]
pub enum StorageError {
    ConnectionError(String),
    MigrationError(String),
    QueryError(String),
    LockError(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionError(msg) => write!(f, "Database connection or I/O failure: {msg}"),
            Self::MigrationError(msg) => write!(f, "Database migration failure: {msg}"),
            Self::QueryError(msg) => write!(f, "Query execution failure: {msg}"),
            Self::LockError(msg) => write!(f, "Lock acquisition failure: {msg}"),
        }
    }
}

impl std::error::Error for StorageError {}

/// Thread-safe SQLite usage storage engine.
#[derive(Clone)]
pub struct UsageStorage {
    conn: Arc<Mutex<Connection>>,
    db_path: String,
}

impl UsageStorage {
    /// Access configured database path.
    pub fn db_path(&self) -> &str {
        &self.db_path
    }
    /// Initialize storage at the specified file path or in-memory (":memory:").
    pub fn new(db_path: &str) -> Result<Self, StorageError> {
        let is_memory = db_path == ":memory:" || db_path.contains("mode=memory");

        if !is_memory {
            let parent_dir = Path::new(db_path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty() && !p.exists());
            if let Some(parent) = parent_dir {
                std::fs::create_dir_all(parent).map_err(|e| {
                    StorageError::ConnectionError(format!(
                        "Failed to create database parent directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
        }

        let conn = if is_memory {
            Connection::open_in_memory().map_err(|e| {
                StorageError::ConnectionError(format!("Failed to open in-memory SQLite: {e}"))
            })?
        } else {
            Connection::open_with_flags(
                db_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| {
                StorageError::ConnectionError(format!(
                    "Failed to open SQLite database at '{db_path}': {e}"
                ))
            })?
        };

        // Enable WAL mode and foreign keys for high-performance concurrent reads
        if !is_memory {
            let _ = conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;",
            );
        }

        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_string(),
        };

        storage.run_migrations()?;
        info!(db_path = %db_path, "SQLite usage storage initialized successfully");
        Ok(storage)
    }

    /// Run idempotent schema migrations.
    fn run_migrations(&self) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS usage_records (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                request_mode TEXT NOT NULL,
                routing_mode TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                completion_time INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                outcome TEXT NOT NULL,
                failure_category TEXT,
                prompt_tokens INTEGER,
                completion_tokens INTEGER,
                total_tokens INTEGER,
                usage_source TEXT NOT NULL,
                record_status TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_usage_records_start_time ON usage_records(start_time);
            CREATE INDEX IF NOT EXISTS idx_usage_records_provider ON usage_records(provider_id);
            CREATE INDEX IF NOT EXISTS idx_usage_records_model ON usage_records(model_id);
            CREATE INDEX IF NOT EXISTS idx_usage_records_outcome ON usage_records(outcome);
            CREATE INDEX IF NOT EXISTS idx_usage_records_composite ON usage_records(start_time, provider_id, model_id);",
        )
        .map_err(|e| StorageError::MigrationError(format!("Schema migration failed: {e}")))?;

        // Record migration version 1
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
            params![now],
        )
        .map_err(|e| StorageError::MigrationError(format!("Migration log failed: {e}")))?;

        Ok(())
    }

    /// Asynchronously insert a usage record off Tokio's worker threads.
    pub async fn record(&self, record: UsageRecord) -> Result<(), StorageError> {
        let storage = self.clone();
        tokio::task::spawn_blocking(move || storage.record_sync(&record))
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Blocking task join failed: {e}")))?
    }

    /// Synchronous insert implementation using parameterized SQL.
    pub fn record_sync(&self, record: &UsageRecord) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let prompt_tokens = record.prompt_tokens.map(|v| v as i64);
        let completion_tokens = record.completion_tokens.map(|v| v as i64);
        let total_tokens = record.total_tokens.map(|v| v as i64);

        conn.execute(
            "INSERT OR REPLACE INTO usage_records (
                id, provider_id, model_id, request_mode, routing_mode,
                start_time, completion_time, latency_ms, outcome, failure_category,
                prompt_tokens, completion_tokens, total_tokens, usage_source, record_status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                record.id,
                record.provider_id,
                record.model_id,
                record.request_mode.as_str(),
                record.routing_mode.as_str(),
                record.start_time as i64,
                record.completion_time as i64,
                record.latency_ms as i64,
                record.outcome.as_str(),
                record.failure_category,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                record.usage_source.as_str(),
                record.record_status.as_str(),
            ],
        )
        .map_err(|e| {
            error!(error = %e, record_id = %record.id, "Failed to persist usage record");
            StorageError::QueryError(format!("Insert failed: {e}"))
        })?;

        Ok(())
    }

    /// Prune records with start_time strictly before cutoff_epoch_ms.
    /// Returns the number of pruned records.
    pub fn prune_older_than_sync(&self, cutoff_epoch_ms: u64) -> Result<usize, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let deleted = conn
            .execute(
                "DELETE FROM usage_records WHERE start_time < ?",
                params![cutoff_epoch_ms as i64],
            )
            .map_err(|e| StorageError::QueryError(format!("Prune failed: {e}")))?;

        Ok(deleted)
    }

    /// Asynchronously prune records older than cutoff_epoch_ms off Tokio executor threads.
    pub async fn prune_older_than(&self, cutoff_epoch_ms: u64) -> Result<usize, StorageError> {
        let storage = self.clone();
        tokio::task::spawn_blocking(move || storage.prune_older_than_sync(cutoff_epoch_ms))
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Blocking task join failed: {e}")))?
    }

    /// Asynchronously fetch aggregated summary.
    pub async fn get_summary(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<UsageSummary, StorageError> {
        let storage = self.clone();
        tokio::task::spawn_blocking(move || storage.get_summary_sync(&filter))
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Blocking task join failed: {e}")))?
    }

    /// Synchronous summary aggregation.
    pub fn get_summary_sync(
        &self,
        filter: &UsageQueryFilter,
    ) -> Result<UsageSummary, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut sql = "SELECT 
                COUNT(*) as total_reqs,
                COALESCE(SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END), 0) as success_reqs,
                COALESCE(SUM(CASE WHEN outcome != 'success' THEN 1 ELSE 0 END), 0) as fail_reqs,
                COALESCE(SUM(CASE WHEN total_tokens IS NOT NULL THEN 1 ELSE 0 END), 0) as known_tokens_reqs,
                COALESCE(SUM(CASE WHEN total_tokens IS NULL THEN 1 ELSE 0 END), 0) as unknown_tokens_reqs,
                COALESCE(SUM(prompt_tokens), 0) as sum_prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as sum_completion_tokens,
                COALESCE(SUM(total_tokens), 0) as sum_total_tokens,
                COALESCE(AVG(latency_ms), 0.0) as avg_latency
            FROM usage_records WHERE 1=1".to_string();

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(st) = filter.start_time {
            sql.push_str(" AND start_time >= ?");
            params_vec.push(Box::new(st as i64));
        }
        if let Some(et) = filter.end_time {
            sql.push_str(" AND start_time <= ?");
            params_vec.push(Box::new(et as i64));
        }
        if let Some(ref p) = filter.provider_id {
            sql.push_str(" AND provider_id = ?");
            params_vec.push(Box::new(p.clone()));
        }
        if let Some(ref m) = filter.model_id {
            sql.push_str(" AND model_id = ?");
            params_vec.push(Box::new(m.clone()));
        }

        let rusqlite_params: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();

        let (
            total_requests,
            successful_requests,
            failed_requests,
            known_usage_requests,
            unknown_usage_requests,
            total_prompt_tokens,
            total_completion_tokens,
            total_tokens,
            avg_latency_ms,
        ): (u64, u64, u64, u64, u64, u64, u64, u64, f64) = conn
            .query_row(&sql, rusqlite_params.as_slice(), |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, i64>(4)? as u64,
                    row.get::<_, i64>(5)? as u64,
                    row.get::<_, i64>(6)? as u64,
                    row.get::<_, i64>(7)? as u64,
                    row.get::<_, f64>(8)?,
                ))
            })
            .map_err(|e| StorageError::QueryError(format!("Summary query failed: {e}")))?;

        // Fetch percentiles (p50, p95)
        let mut latencies_sql = "SELECT latency_ms FROM usage_records WHERE 1=1".to_string();
        if let Some(st) = filter.start_time {
            latencies_sql.push_str(&format!(" AND start_time >= {st}"));
        }
        if let Some(et) = filter.end_time {
            latencies_sql.push_str(&format!(" AND start_time <= {et}"));
        }
        if let Some(ref p) = filter.provider_id {
            latencies_sql.push_str(&format!(" AND provider_id = '{p}'"));
        }
        if let Some(ref m) = filter.model_id {
            latencies_sql.push_str(&format!(" AND model_id = '{m}'"));
        }
        latencies_sql.push_str(" ORDER BY latency_ms ASC");

        let mut stmt = conn.prepare(&latencies_sql).map_err(|e| {
            StorageError::QueryError(format!("Latency query preparation failed: {e}"))
        })?;
        let latencies_iter = stmt
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|e| StorageError::QueryError(format!("Latency query failed: {e}")))?;

        let mut latencies: Vec<f64> = Vec::new();
        for val in latencies_iter.flatten() {
            latencies.push(val as f64);
        }

        let p50_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            latencies[latencies.len() / 2]
        };

        let p95_latency_ms = if latencies.is_empty() {
            0.0
        } else {
            let idx = (latencies.len() as f64 * 0.95) as usize;
            latencies[std::cmp::min(idx, latencies.len() - 1)]
        };

        // Group by request mode
        let mut by_request_mode = HashMap::new();
        let mut req_mode_stmt = conn
            .prepare("SELECT request_mode, COUNT(*) FROM usage_records GROUP BY request_mode")
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        let rows = req_mode_stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        for r in rows.flatten() {
            by_request_mode.insert(r.0, r.1);
        }

        // Group by routing mode
        let mut by_routing_mode = HashMap::new();
        let mut routing_stmt = conn
            .prepare("SELECT routing_mode, COUNT(*) FROM usage_records GROUP BY routing_mode")
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        let rows = routing_stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        for r in rows.flatten() {
            by_routing_mode.insert(r.0, r.1);
        }

        // Group by outcome
        let mut by_outcome = HashMap::new();
        let mut outcome_stmt = conn
            .prepare("SELECT outcome, COUNT(*) FROM usage_records GROUP BY outcome")
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        let rows = outcome_stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        for r in rows.flatten() {
            by_outcome.insert(r.0, r.1);
        }

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

    /// Asynchronously fetch provider summaries.
    pub async fn get_providers(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<ProviderUsageSummary>, StorageError> {
        let storage = self.clone();
        tokio::task::spawn_blocking(move || storage.get_providers_sync(&filter))
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Blocking task join failed: {e}")))?
    }

    pub fn get_providers_sync(
        &self,
        filter: &UsageQueryFilter,
    ) -> Result<Vec<ProviderUsageSummary>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut sql = "SELECT 
                provider_id,
                COUNT(*) as requests,
                COALESCE(SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END), 0) as successful_requests,
                COALESCE(SUM(CASE WHEN outcome != 'success' THEN 1 ELSE 0 END), 0) as failed_requests,
                COALESCE(SUM(CASE WHEN total_tokens IS NOT NULL THEN 1 ELSE 0 END), 0) as known_usage_requests,
                COALESCE(SUM(total_tokens), 0) as total_tokens,
                COALESCE(AVG(latency_ms), 0.0) as avg_latency
            FROM usage_records WHERE 1=1".to_string();

        if let Some(st) = filter.start_time {
            sql.push_str(&format!(" AND start_time >= {st}"));
        }
        if let Some(et) = filter.end_time {
            sql.push_str(&format!(" AND start_time <= {et}"));
        }
        if let Some(ref p) = filter.provider_id {
            sql.push_str(&format!(" AND provider_id = '{p}'"));
        }

        sql.push_str(" GROUP BY provider_id ORDER BY requests DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ProviderUsageSummary {
                    provider_id: row.get(0)?,
                    requests: row.get::<_, i64>(1)? as u64,
                    successful_requests: row.get::<_, i64>(2)? as u64,
                    failed_requests: row.get::<_, i64>(3)? as u64,
                    known_usage_requests: row.get::<_, i64>(4)? as u64,
                    total_tokens: row.get::<_, i64>(5)? as u64,
                    avg_latency_ms: row.get(6)?,
                })
            })
            .map_err(|e| StorageError::QueryError(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows.flatten() {
            results.push(r);
        }
        Ok(results)
    }

    /// Asynchronously fetch model summaries.
    pub async fn get_models(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<ModelUsageSummary>, StorageError> {
        let storage = self.clone();
        tokio::task::spawn_blocking(move || storage.get_models_sync(&filter))
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Blocking task join failed: {e}")))?
    }

    pub fn get_models_sync(
        &self,
        filter: &UsageQueryFilter,
    ) -> Result<Vec<ModelUsageSummary>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut sql = "SELECT 
                model_id,
                provider_id,
                COUNT(*) as requests,
                COALESCE(SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END), 0) as successful_requests,
                COALESCE(SUM(CASE WHEN outcome != 'success' THEN 1 ELSE 0 END), 0) as failed_requests,
                COALESCE(SUM(CASE WHEN total_tokens IS NOT NULL THEN 1 ELSE 0 END), 0) as known_usage_requests,
                COALESCE(SUM(total_tokens), 0) as total_tokens,
                COALESCE(AVG(latency_ms), 0.0) as avg_latency
            FROM usage_records WHERE 1=1".to_string();

        if let Some(st) = filter.start_time {
            sql.push_str(&format!(" AND start_time >= {st}"));
        }
        if let Some(et) = filter.end_time {
            sql.push_str(&format!(" AND start_time <= {et}"));
        }
        if let Some(ref p) = filter.provider_id {
            sql.push_str(&format!(" AND provider_id = '{p}'"));
        }
        if let Some(ref m) = filter.model_id {
            sql.push_str(&format!(" AND model_id = '{m}'"));
        }

        sql.push_str(" GROUP BY model_id, provider_id ORDER BY requests DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ModelUsageSummary {
                    model_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    requests: row.get::<_, i64>(2)? as u64,
                    successful_requests: row.get::<_, i64>(3)? as u64,
                    failed_requests: row.get::<_, i64>(4)? as u64,
                    known_usage_requests: row.get::<_, i64>(5)? as u64,
                    total_tokens: row.get::<_, i64>(6)? as u64,
                    avg_latency_ms: row.get(7)?,
                })
            })
            .map_err(|e| StorageError::QueryError(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows.flatten() {
            results.push(r);
        }
        Ok(results)
    }

    /// Asynchronously retrieve raw usage records with bounding limit.
    pub async fn get_records(
        &self,
        filter: UsageQueryFilter,
    ) -> Result<Vec<UsageRecord>, StorageError> {
        let storage = self.clone();
        tokio::task::spawn_blocking(move || storage.get_records_sync(&filter))
            .await
            .map_err(|e| StorageError::ConnectionError(format!("Blocking task join failed: {e}")))?
    }

    pub fn get_records_sync(
        &self,
        filter: &UsageQueryFilter,
    ) -> Result<Vec<UsageRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::LockError(e.to_string()))?;

        let mut sql = "SELECT 
                id, provider_id, model_id, request_mode, routing_mode,
                start_time, completion_time, latency_ms, outcome, failure_category,
                prompt_tokens, completion_tokens, total_tokens, usage_source, record_status
            FROM usage_records WHERE 1=1"
            .to_string();

        if let Some(st) = filter.start_time {
            sql.push_str(&format!(" AND start_time >= {st}"));
        }
        if let Some(et) = filter.end_time {
            sql.push_str(&format!(" AND start_time <= {et}"));
        }
        if let Some(ref p) = filter.provider_id {
            sql.push_str(&format!(" AND provider_id = '{p}'"));
        }
        if let Some(ref m) = filter.model_id {
            sql.push_str(&format!(" AND model_id = '{m}'"));
        }

        sql.push_str(" ORDER BY start_time DESC");
        let limit = filter.limit.unwrap_or(100).min(1000);
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::QueryError(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let req_mode_str: String = row.get(3)?;
                let routing_mode_str: String = row.get(4)?;
                let outcome_str: String = row.get(8)?;
                let source_str: String = row.get(13)?;
                let status_str: String = row.get(14)?;

                Ok(UsageRecord {
                    id: row.get(0)?,
                    provider_id: row.get(1)?,
                    model_id: row.get(2)?,
                    request_mode: RequestMode::from_str_lossy(&req_mode_str),
                    routing_mode: RoutingMode::from_str_lossy(&routing_mode_str),
                    start_time: row.get::<_, i64>(5)? as u64,
                    completion_time: row.get::<_, i64>(6)? as u64,
                    latency_ms: row.get::<_, i64>(7)? as u64,
                    outcome: RequestOutcome::from_str_lossy(&outcome_str),
                    failure_category: row.get(9)?,
                    prompt_tokens: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
                    completion_tokens: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                    total_tokens: row.get::<_, Option<i64>>(12)?.map(|v| v as u64),
                    usage_source: UsageSource::from_str_lossy(&source_str),
                    record_status: RecordStatus::from_str_lossy(&status_str),
                })
            })
            .map_err(|e| StorageError::QueryError(e.to_string()))?;

        let mut results = Vec::new();
        for r in rows.flatten() {
            results.push(r);
        }
        Ok(results)
    }
}
