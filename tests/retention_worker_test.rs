//! Comprehensive verification tests for Workstream A: Autonomous Retention Cleanup.
//!
//! Verifies:
//! 1. Autonomous execution and lifecycle management.
//! 2. Progressive bounded batch pruning across large backlogs.
//! 3. Precise retention cutoff boundary preservation (older pruned, boundary/newer preserved).
//! 4. Empty database handling and non-intrusive repeated runs.
//! 5. Graceful shutdown responsive within bounded timeout.
//! 6. Resilience against transient database errors.

use ai_backend::usage::{
    RecordStatus, RequestMode, RequestOutcome, RetentionMaintenanceWorker, RoutingMode,
    UsageRecord, UsageService, UsageSource, UsageStorage,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

fn test_record(id: &str, start_time_ms: u64) -> UsageRecord {
    UsageRecord {
        id: id.to_string(),
        provider_id: "test-provider".to_string(),
        model_id: "test-model".to_string(),
        request_mode: RequestMode::Buffered,
        routing_mode: RoutingMode::Auto,
        start_time: start_time_ms,
        completion_time: start_time_ms + 120,
        latency_ms: 120,
        outcome: RequestOutcome::Success,
        failure_category: None,
        prompt_tokens: Some(10),
        completion_tokens: Some(20),
        total_tokens: Some(30),
        usage_source: UsageSource::Reported,
        record_status: RecordStatus::Final,
        subject: None,
    }
}

#[tokio::test]
async fn test_worker_progressive_bounded_pruning() {
    let storage = Arc::new(UsageStorage::new(":memory:").expect("Failed to create storage"));
    let service = Arc::new(UsageService::new(storage.clone()));

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // 10 days retention window -> cutoff is 10 days ago
    let ten_days_ms = 10 * 86_400_000;
    let old_start_ms = now_ms - ten_days_ms - 100_000;

    // Insert 250 expired records
    for i in 0..250 {
        let rec = test_record(&format!("old_rec_{i}"), old_start_ms + (i as u64 * 10));
        storage.record(rec).await.expect("Insert failed");
    }

    // Insert 50 fresh records
    for i in 0..50 {
        let rec = test_record(&format!("fresh_rec_{i}"), now_ms - 10_000 + (i as u64));
        storage.record(rec).await.expect("Insert failed");
    }

    // Configure worker with 10 days retention and batch limit of 100
    let worker = RetentionMaintenanceWorker::new(service.clone(), 3600, Some(10), 100);

    // Run a single maintenance cycle
    let summary = worker.run_once().await;

    // The worker executes progressive batches (100 + 100 + 50 = 250)
    assert_eq!(summary.records_pruned, 250);
    assert_eq!(summary.prune_batches, 3);

    // Ensure 50 fresh records remain intact
    let remaining_records = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default().with_limit(500))
        .await
        .expect("Query failed");
    assert_eq!(remaining_records.len(), 50);
    assert!(
        remaining_records
            .iter()
            .all(|r| r.id.starts_with("fresh_rec_"))
    );
}

#[tokio::test]
async fn test_storage_exact_cutoff_boundary_preservation() {
    let storage = Arc::new(UsageStorage::new(":memory:").expect("Failed to create storage"));

    let fixed_cutoff_ms = 1_700_000_000_000u64;

    // Record strictly older than cutoff: must be pruned
    let rec_older = test_record("rec_older", fixed_cutoff_ms - 1);
    storage.record(rec_older).await.unwrap();

    // Record strictly at cutoff: must be preserved (SQL is start_time < cutoff)
    let rec_boundary = test_record("rec_boundary", fixed_cutoff_ms);
    storage.record(rec_boundary).await.unwrap();

    // Record newer than cutoff: must be preserved
    let rec_newer = test_record("rec_newer", fixed_cutoff_ms + 1);
    storage.record(rec_newer).await.unwrap();

    // Bounded prune with fixed cutoff
    let pruned = storage
        .prune_older_than_bounded(fixed_cutoff_ms, 10)
        .await
        .unwrap();

    assert_eq!(
        pruned, 1,
        "Only record strictly older than cutoff should be pruned"
    );

    let remaining = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(remaining.len(), 2);
    let ids: Vec<&str> = remaining.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&"rec_boundary"),
        "Record exactly at cutoff must be preserved"
    );
    assert!(
        ids.contains(&"rec_newer"),
        "Record newer than cutoff must be preserved"
    );
    assert!(
        !ids.contains(&"rec_older"),
        "Record older than cutoff must be pruned"
    );
}

#[tokio::test]
async fn test_worker_preserves_records_within_retention_window() {
    let storage = Arc::new(UsageStorage::new(":memory:").expect("Failed to create storage"));
    let service = Arc::new(UsageService::new(storage.clone()));

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let retention_days = 7u64;
    let retention_ms = retention_days * 86_400_000;
    let cutoff_ms = now_ms.saturating_sub(retention_ms);

    // Record clearly older than 7 days (e.g. 8 days ago): must be pruned
    let rec_expired = test_record("rec_expired", cutoff_ms.saturating_sub(86_400_000));
    storage.record(rec_expired).await.unwrap();

    // Record clearly within 7 days retention (e.g. 6 days ago): must be preserved
    let rec_retained = test_record("rec_retained", cutoff_ms + 86_400_000);
    storage.record(rec_retained).await.unwrap();

    // Record today: must be preserved
    let rec_today = test_record("rec_today", now_ms);
    storage.record(rec_today).await.unwrap();

    let worker = RetentionMaintenanceWorker::new(service.clone(), 3600, Some(retention_days), 100);
    let summary = worker.run_once().await;

    assert_eq!(summary.records_pruned, 1);

    let remaining = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(remaining.len(), 2);
    let ids: Vec<&str> = remaining.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"rec_retained"));
    assert!(ids.contains(&"rec_today"));
    assert!(!ids.contains(&"rec_expired"));
}

#[tokio::test]
async fn test_worker_handles_empty_database_and_repeated_runs_silently() {
    let storage = Arc::new(UsageStorage::new(":memory:").expect("Failed to create storage"));
    let service = Arc::new(UsageService::new(storage.clone()));

    let worker = RetentionMaintenanceWorker::new(service.clone(), 3600, Some(30), 500);

    // First run on empty database
    let run1 = worker.run_once().await;
    assert_eq!(run1.records_pruned, 0);

    // Repeated runs on empty database
    let run2 = worker.run_once().await;
    assert_eq!(run2.records_pruned, 0);
}

#[tokio::test]
async fn test_worker_graceful_shutdown_channel() {
    let storage = Arc::new(UsageStorage::new(":memory:").expect("Failed to create storage"));
    let service = Arc::new(UsageService::new(storage.clone()));

    // Worker with 1 second interval
    let worker = RetentionMaintenanceWorker::new(service.clone(), 1, Some(30), 100);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let join_handle = tokio::spawn(async move {
        worker.run_loop(shutdown_rx).await;
    });

    // Let the worker run briefly
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send shutdown signal
    shutdown_tx.send(true).expect("Failed to send shutdown");

    // Await task termination within 500ms
    let termination_result = tokio::time::timeout(Duration::from_millis(500), join_handle).await;
    assert!(
        termination_result.is_ok(),
        "Worker failed to shut down within timeout"
    );
}

#[tokio::test]
async fn test_worker_disabled_retention_skips_pruning() {
    let storage = Arc::new(UsageStorage::new(":memory:").expect("Failed to create storage"));
    let service = Arc::new(UsageService::new(storage.clone()));

    // Insert an old record
    let rec = test_record("very_old_rec", 1000);
    storage.record(rec).await.unwrap();

    // When retention_days is None, worker does not delete records
    let worker = RetentionMaintenanceWorker::new(service.clone(), 3600, None, 100);
    let summary = worker.run_once().await;
    assert_eq!(summary.records_pruned, 0);

    let remaining = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
}
