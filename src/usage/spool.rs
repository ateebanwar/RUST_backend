//! Independent durable disk spool for usage tracking records.
//!
//! Provides a fail-safe, append-only disk spool completely decoupled from SQLite.
//! When the primary SQLite database experiences transient lockouts, corruption, or I/O failure,
//! usage records are flushed to this independent disk spool. Once the database recovers,
//! records are idempotently drained and replayed into SQLite storage.

use crate::usage::model::UsageRecord;
use crate::usage::storage::UsageStorage;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

/// Errors encountered during disk spool operations.
#[derive(Debug)]
pub enum SpoolError {
    IoError(String),
    SerializationError(String),
    CapacityExceeded(String),
    LockError(String),
}

impl std::fmt::Display for SpoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Spool I/O failure: {msg}"),
            Self::SerializationError(msg) => write!(f, "Spool serialization failure: {msg}"),
            Self::CapacityExceeded(msg) => write!(f, "Spool capacity exceeded: {msg}"),
            Self::LockError(msg) => write!(f, "Spool concurrency lock failure: {msg}"),
        }
    }
}

impl std::error::Error for SpoolError {}

/// An individual persisted record envelope inside the spool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolEntry {
    pub record: UsageRecord,
    pub failure_reason: String,
    pub enqueued_at_epoch_ms: u64,
    pub attempts: u32,
}

/// Statistics and metrics for the disk spool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpoolMetrics {
    pub file_size_bytes: u64,
    pub pending_entries: usize,
}

/// Outcome of a spool drain operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpoolDrainResult {
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub corrupted: usize,
}

/// Thread-safe append-only disk spool manager.
#[derive(Clone)]
pub struct UsageSpooler {
    spool_path: PathBuf,
    dead_letter_path: PathBuf,
    max_spool_bytes: u64,
    lock: Arc<Mutex<()>>,
}

impl UsageSpooler {
    /// Initialize spooler with file path and capacity budget.
    pub fn new(spool_path: impl Into<PathBuf>, max_spool_bytes: u64) -> Self {
        let spool_path = spool_path.into();
        let dead_letter_path = spool_path.with_extension("dead_letter");

        Self {
            spool_path,
            dead_letter_path,
            max_spool_bytes,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Access configured spool file path.
    pub fn spool_path(&self) -> &Path {
        &self.spool_path
    }

    /// Synchronously query spool operational metrics (file size, pending entries).
    pub fn metrics_sync(&self) -> Result<SpoolMetrics, SpoolError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|e| SpoolError::LockError(e.to_string()))?;

        if !self.spool_path.exists() {
            return Ok(SpoolMetrics {
                file_size_bytes: 0,
                pending_entries: 0,
            });
        }

        let metadata = std::fs::metadata(&self.spool_path)
            .map_err(|e| SpoolError::IoError(format!("Failed to get spool metadata: {e}")))?;

        let file = File::open(&self.spool_path)
            .map_err(|e| SpoolError::IoError(format!("Failed to open spool file: {e}")))?;

        let reader = BufReader::new(file);
        let count = reader
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .count();

        Ok(SpoolMetrics {
            file_size_bytes: metadata.len(),
            pending_entries: count,
        })
    }

    /// Asynchronously query spool operational metrics.
    pub async fn metrics(&self) -> Result<SpoolMetrics, SpoolError> {
        let spooler = self.clone();
        tokio::task::spawn_blocking(move || spooler.metrics_sync())
            .await
            .map_err(|e| SpoolError::IoError(format!("Blocking task join failed: {e}")))?
    }

    /// Synchronously append a usage record to the spool file.
    pub fn append_record_sync(
        &self,
        record: &UsageRecord,
        failure_reason: &str,
    ) -> Result<(), SpoolError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|e| SpoolError::LockError(e.to_string()))?;

        // Ensure parent directory exists
        if let Some(parent) = self
            .spool_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty() && !p.exists())
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                SpoolError::IoError(format!(
                    "Failed to create spool directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = SpoolEntry {
            record: record.clone(),
            failure_reason: failure_reason.to_string(),
            enqueued_at_epoch_ms: now_ms,
            attempts: 1,
        };

        let json_line = serde_json::to_string(&entry)
            .map_err(|e| SpoolError::SerializationError(e.to_string()))?;
        let entry_bytes = json_line.len() as u64 + 1; // +1 for newline

        // Check file size + incoming entry size to strictly prevent exceeding capacity ceiling
        let current_bytes = std::fs::metadata(&self.spool_path)
            .map(|m| m.len())
            .unwrap_or(0);

        if current_bytes.saturating_add(entry_bytes) > self.max_spool_bytes {
            let err_msg = format!(
                "Spool file '{}' capacity ceiling ({} bytes) exceeded by incoming entry ({} bytes, current {} bytes)",
                self.spool_path.display(),
                self.max_spool_bytes,
                entry_bytes,
                current_bytes
            );
            error!(record_id = %record.id, "{}", err_msg);
            return Err(SpoolError::CapacityExceeded(err_msg));
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.spool_path)
            .map_err(|e| {
                SpoolError::IoError(format!(
                    "Failed to open spool file '{}': {e}",
                    self.spool_path.display()
                ))
            })?;

        writeln!(file, "{json_line}").map_err(|e| {
            SpoolError::IoError(format!(
                "Failed to write to spool file '{}': {e}",
                self.spool_path.display()
            ))
        })?;

        // Flush to ensure data is committed to operating system buffers
        file.flush().map_err(|e| {
            SpoolError::IoError(format!(
                "Failed to flush spool file '{}': {e}",
                self.spool_path.display()
            ))
        })?;

        Ok(())
    }

    /// Asynchronously append a usage record to the spool file off worker threads.
    pub async fn append_record(
        &self,
        record: &UsageRecord,
        failure_reason: &str,
    ) -> Result<(), SpoolError> {
        let spooler = self.clone();
        let record = record.clone();
        let reason = failure_reason.to_string();
        tokio::task::spawn_blocking(move || spooler.append_record_sync(&record, &reason))
            .await
            .map_err(|e| SpoolError::IoError(format!("Blocking join failed: {e}")))?
    }

    /// Synchronously drain pending spool entries and replay them into SQLite storage.
    pub fn drain_to_storage_sync(
        &self,
        storage: &UsageStorage,
        batch_limit: usize,
    ) -> Result<SpoolDrainResult, SpoolError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|e| SpoolError::LockError(e.to_string()))?;

        if !self.spool_path.exists() {
            return Ok(SpoolDrainResult {
                processed: 0,
                succeeded: 0,
                failed: 0,
                corrupted: 0,
            });
        }

        let file = File::open(&self.spool_path).map_err(|e| {
            SpoolError::IoError(format!(
                "Failed to open spool for reading '{}': {e}",
                self.spool_path.display()
            ))
        })?;

        let reader = BufReader::new(file);
        let mut lines = Vec::new();
        for line_res in reader.lines() {
            match line_res {
                Ok(line) if !line.trim().is_empty() => lines.push(line),
                _ => {}
            }
        }

        if lines.is_empty() {
            let _ = std::fs::remove_file(&self.spool_path);
            return Ok(SpoolDrainResult {
                processed: 0,
                succeeded: 0,
                failed: 0,
                corrupted: 0,
            });
        }

        let mut processed = 0;
        let mut succeeded = 0;
        let mut failed = 0;
        let mut corrupted = 0;
        let mut uncommitted_lines = Vec::new();

        for line in lines {
            if processed >= batch_limit {
                uncommitted_lines.push(line);
                continue;
            }

            processed += 1;
            match serde_json::from_str::<SpoolEntry>(&line) {
                Ok(entry) => {
                    // Replay entry idempotently into SQLite storage
                    match storage.record_sync(&entry.record) {
                        Ok(_) => {
                            succeeded += 1;
                        }
                        Err(err) => {
                            failed += 1;
                            warn!(
                                record_id = %entry.record.id,
                                error = %err,
                                "Spool drain replay failed; entry retained in spool"
                            );
                            uncommitted_lines.push(line);
                        }
                    }
                }
                Err(err) => {
                    corrupted += 1;
                    warn!(
                        error = %err,
                        "Corrupted spool line encountered; moving to dead letter spool"
                    );
                    // Dead-letter corrupted payload so it does not block the pipeline
                    if let Ok(mut dl_file) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.dead_letter_path)
                    {
                        let _ = writeln!(dl_file, "{line}");
                        let _ = dl_file.flush();
                    }
                }
            }
        }

        // Rewrite spool file with remaining uncommitted entries using atomic replacement
        if uncommitted_lines.is_empty() {
            let _ = std::fs::remove_file(&self.spool_path);
        } else {
            let temp_path = self.spool_path.with_extension("tmp");
            let mut temp_file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temp_path)
                .map_err(|e| {
                    SpoolError::IoError(format!(
                        "Failed to create temp spool file '{}': {e}",
                        temp_path.display()
                    ))
                })?;

            for remaining in uncommitted_lines {
                writeln!(temp_file, "{remaining}").map_err(|e| {
                    SpoolError::IoError(format!(
                        "Failed to write to temp spool '{}': {e}",
                        temp_path.display()
                    ))
                })?;
            }
            temp_file.flush().map_err(|e| {
                SpoolError::IoError(format!(
                    "Failed to flush temp spool '{}': {e}",
                    temp_path.display()
                ))
            })?;
            drop(temp_file);

            std::fs::rename(&temp_path, &self.spool_path).map_err(|e| {
                SpoolError::IoError(format!(
                    "Failed to atomically replace spool file '{}': {e}",
                    self.spool_path.display()
                ))
            })?;
        }

        if succeeded > 0 {
            info!(
                succeeded,
                failed, corrupted, "Durable spool drained and replayed into SQLite storage"
            );
        }

        Ok(SpoolDrainResult {
            processed,
            succeeded,
            failed,
            corrupted,
        })
    }

    /// Asynchronously drain spool entries and replay them into SQLite storage.
    pub async fn drain_to_storage(
        &self,
        storage: &UsageStorage,
        batch_limit: usize,
    ) -> Result<SpoolDrainResult, SpoolError> {
        let spooler = self.clone();
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || spooler.drain_to_storage_sync(&storage, batch_limit))
            .await
            .map_err(|e| SpoolError::IoError(format!("Blocking join failed: {e}")))?
    }

    /// Get current spool file metrics.
    pub fn get_metrics_sync(&self) -> SpoolMetrics {
        let file_size_bytes = std::fs::metadata(&self.spool_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let pending_entries = if let Ok(file) = File::open(&self.spool_path) {
            BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter(|l| !l.trim().is_empty())
                .count()
        } else {
            0
        };

        SpoolMetrics {
            file_size_bytes,
            pending_entries,
        }
    }
}
