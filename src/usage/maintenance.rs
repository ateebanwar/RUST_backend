//! Autonomous periodic retention cleanup and retry drain maintenance worker.
//!
//! Conforms to MASTER_SPEC.md §8 & §14, ARCHITECTURE.md §2, and AGENTS.md.
//!
//! Provides an autonomous, lifecycle-managed background task that periodically:
//! 1. Prunes expired usage tracking records using bounded progressive batches.
//! 2. Drains pending fallback records from the independent disk spool into SQLite.
//! 3. Drains pending retries from SQLite's durable retry queue.
//! 4. Executes passive WAL maintenance checkpoints without blocking concurrent read/write queries.
//! 5. Responds immediately to application graceful shutdown signals.

use crate::usage::service::{CombinedDrainResult, UsageService};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::{info, trace, warn};

/// Summary of an individual maintenance run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceRunSummary {
    pub records_pruned: usize,
    pub prune_batches: usize,
    pub spool_recovered: usize,
    pub retry_recovered: usize,
    pub duration_ms: u64,
}

/// Autonomous background worker for database maintenance and retry recovery.
pub struct RetentionMaintenanceWorker {
    usage_service: Arc<UsageService>,
    interval: Duration,
    retention_days: Option<u64>,
    batch_size: usize,
    max_progressive_batches: usize,
}

impl RetentionMaintenanceWorker {
    /// Create a new maintenance worker instance.
    pub fn new(
        usage_service: Arc<UsageService>,
        interval_secs: u64,
        retention_days: Option<u64>,
        batch_size: usize,
    ) -> Self {
        let interval_secs = interval_secs.max(1);
        let batch_size = batch_size.clamp(1, 100_000);

        Self {
            usage_service,
            interval: Duration::from_secs(interval_secs),
            retention_days,
            batch_size,
            max_progressive_batches: 5, // Drain up to 5 consecutive batches per execution tick
        }
    }

    /// Run a single cycle of maintenance (retention pruning, retry drain, WAL checkpoint).
    /// Can be executed deterministically in unit and integration tests.
    pub async fn run_once(&self) -> MaintenanceRunSummary {
        let start = Instant::now();
        let mut total_pruned = 0;
        let mut prune_batches = 0;

        // 1. Progressive bounded retention cleanup
        if let Some(days) = self.retention_days.filter(|&d| d > 0) {
            for _ in 0..self.max_progressive_batches {
                match self
                    .usage_service
                    .prune_retention_bounded(days, self.batch_size)
                    .await
                {
                    Ok(pruned) => {
                        total_pruned += pruned;
                        prune_batches += 1;
                        if pruned < self.batch_size {
                            // Backlog exhausted for this cycle
                            break;
                        }
                    }
                    Err(err) => {
                        warn!(
                            retention_days = days,
                            batch_size = self.batch_size,
                            error = %err,
                            "Retention cleanup batch encountered a database error; will retry next cycle"
                        );
                        break;
                    }
                }
            }
        }

        // 2. Drain spool and SQLite retry queues
        let drain_res = self
            .usage_service
            .drain_all_retries(self.batch_size)
            .await
            .unwrap_or_else(|err| {
                warn!(
                    error = %err,
                    "Retry queue drain encountered an error during maintenance cycle"
                );
                CombinedDrainResult::default()
            });

        // 3. Passive WAL checkpoint
        if let Err(err) = self.usage_service.maintenance_wal_checkpoint().await {
            trace!(
                error = %err,
                "Passive WAL checkpoint skipped or deferred during maintenance cycle"
            );
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        if total_pruned > 0 || drain_res.total_succeeded() > 0 {
            info!(
                records_pruned = total_pruned,
                prune_batches,
                spool_drained = drain_res.spool.succeeded,
                retry_drained = drain_res.sqlite.succeeded,
                duration_ms,
                "Autonomous retention and retry maintenance cycle completed"
            );
        } else {
            trace!(
                duration_ms,
                "Autonomous maintenance cycle completed with zero expired records or pending retries"
            );
        }

        MaintenanceRunSummary {
            records_pruned: total_pruned,
            prune_batches,
            spool_recovered: drain_res.spool.succeeded,
            retry_recovered: drain_res.sqlite.succeeded,
            duration_ms,
        }
    }

    /// Autonomous execution loop that runs periodically until a shutdown signal is received.
    pub async fn run_loop(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!(
            interval_secs = self.interval.as_secs(),
            retention_days = ?self.retention_days,
            batch_size = self.batch_size,
            "Autonomous retention maintenance worker started"
        );

        let mut timer = tokio::time::interval(self.interval);
        // Do not immediately fire the first tick; allow startup initialization to complete
        timer.tick().await;

        loop {
            tokio::select! {
                _ = timer.tick() => {
                    self.run_once().await;
                }
                res = shutdown_rx.changed() => {
                    match res {
                        Ok(()) => {
                            if *shutdown_rx.borrow() {
                                info!("Retention maintenance worker received shutdown signal; stopping gracefully");
                                break;
                            }
                        }
                        Err(_) => {
                            // Sender dropped, meaning application is shutting down
                            info!("Retention maintenance worker shutdown channel closed; exiting loop");
                            break;
                        }
                    }
                }
            }
        }
    }
}
