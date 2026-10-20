//! Comprehensive verification tests for Workstream B: Durable Usage Spool.
//!
//! Verifies:
//! 1. Independent disk spool persistence decoupled from SQLite.
//! 2. Idempotent replay into SQLite storage upon database recovery.
//! 3. Restart recovery across process/instance boundaries.
//! 4. Safe dead-letter isolation of corrupted spool entries.
//! 5. Capacity limit enforcement and disk exhaustion bounds.

use ai_backend::usage::{
    RecordStatus, RequestMode, RequestOutcome, RoutingMode, SpoolError, UsageRecord, UsageSource,
    UsageSpooler, UsageStorage,
};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

fn unique_temp_spool_path(test_name: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("ai_backend_{test_name}_{now}.log"))
}

fn sample_record(id: &str) -> UsageRecord {
    UsageRecord {
        id: id.to_string(),
        provider_id: "groq-api".to_string(),
        model_id: "openai/gpt-oss-120b".to_string(),
        request_mode: RequestMode::Buffered,
        routing_mode: RoutingMode::Auto,
        start_time: 1700000000000,
        completion_time: 1700000000150,
        latency_ms: 150,
        outcome: RequestOutcome::Success,
        failure_category: None,
        prompt_tokens: Some(15),
        completion_tokens: Some(25),
        total_tokens: Some(40),
        usage_source: UsageSource::Reported,
        record_status: RecordStatus::Final,
    }
}

#[tokio::test]
async fn test_spool_append_and_drain_idempotently_into_sqlite() {
    let spool_path = unique_temp_spool_path("append_drain");
    let spool = UsageSpooler::new(&spool_path, 10 * 1024 * 1024);
    let storage = Arc::new(UsageStorage::new(":memory:").unwrap());

    let rec1 = sample_record("rec_1");
    let rec2 = sample_record("rec_2");

    spool
        .append_record(&rec1, "SQLite locked: database is busy")
        .await
        .expect("Append failed");
    spool
        .append_record(&rec2, "SQLite locked: database is busy")
        .await
        .expect("Append failed");

    let metrics = spool.metrics().await.expect("Metrics failed");
    assert_eq!(metrics.pending_entries, 2);

    // Drain into recovered SQLite storage
    let drain_res = spool
        .drain_to_storage(&storage, 100)
        .await
        .expect("Drain failed");
    assert_eq!(drain_res.processed, 2);
    assert_eq!(drain_res.succeeded, 2);
    assert_eq!(drain_res.failed, 0);

    // Verify storage has both records
    let stored = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);

    // Second drain should process 0 entries (file removed or empty)
    let drain_res_2 = spool
        .drain_to_storage(&storage, 100)
        .await
        .expect("Second drain failed");
    assert_eq!(drain_res_2.processed, 0);

    let _ = std::fs::remove_file(&spool_path);
}

#[tokio::test]
async fn test_spool_restart_recovery() {
    let spool_path = unique_temp_spool_path("restart_recovery");
    let storage = Arc::new(UsageStorage::new(":memory:").unwrap());

    // Phase 1: Instance writes to disk and drops
    {
        let spool1 = UsageSpooler::new(&spool_path, 10 * 1024 * 1024);
        spool1
            .append_record(&sample_record("persisted_before_crash_1"), "disk full")
            .await
            .unwrap();
        spool1
            .append_record(&sample_record("persisted_before_crash_2"), "disk full")
            .await
            .unwrap();
    }

    // Phase 2: Process restarts with fresh spooler instance pointing to same file
    let spool2 = UsageSpooler::new(&spool_path, 10 * 1024 * 1024);
    let metrics = spool2.metrics().await.unwrap();
    assert_eq!(metrics.pending_entries, 2);

    // Drain into storage
    let drain = spool2.drain_to_storage(&storage, 10).await.unwrap();
    assert_eq!(drain.succeeded, 2);

    let stored = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);

    let _ = std::fs::remove_file(&spool_path);
}

#[tokio::test]
async fn test_spool_isolates_corrupted_lines_to_dead_letter() {
    let spool_path = unique_temp_spool_path("corrupt_isolation");
    let dead_letter_path = spool_path.with_extension("dead_letter");
    let spool = UsageSpooler::new(&spool_path, 10 * 1024 * 1024);
    let storage = Arc::new(UsageStorage::new(":memory:").unwrap());

    // Append 1 valid record
    spool
        .append_record(&sample_record("valid_rec_1"), "simulated error")
        .await
        .unwrap();

    // Inject corrupted/truncated JSON directly into the spool file
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&spool_path)
            .expect("Failed to open spool");
        writeln!(file, "{{ corrupted_incomplete_json: true").unwrap();
        writeln!(file, "NOT_JSON_AT_ALL").unwrap();
    }

    // Append another valid record
    spool
        .append_record(&sample_record("valid_rec_2"), "simulated error")
        .await
        .unwrap();

    // Drain spool: valid records must succeed, corrupted lines must dead-letter
    let drain = spool.drain_to_storage(&storage, 10).await.unwrap();
    assert_eq!(drain.succeeded, 2);
    assert_eq!(drain.corrupted, 2);

    // Verify valid records committed to SQLite
    let stored = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);

    // Verify dead letter file contains the corrupted lines
    assert!(dead_letter_path.exists());
    let dl_content = std::fs::read_to_string(&dead_letter_path).unwrap();
    assert!(dl_content.contains("corrupted_incomplete_json"));
    assert!(dl_content.contains("NOT_JSON_AT_ALL"));

    let _ = std::fs::remove_file(&spool_path);
    let _ = std::fs::remove_file(&dead_letter_path);
}

#[tokio::test]
async fn test_spool_capacity_ceiling_enforcement() {
    let spool_path = unique_temp_spool_path("capacity_limit");
    // Restrict spool to 500 bytes maximum ceiling
    let spool = UsageSpooler::new(&spool_path, 500);

    let rec1 = sample_record("rec_first");
    spool.append_record(&rec1, "err").await.unwrap();

    let rec2 = sample_record("rec_second");
    let err = spool.append_record(&rec2, "err").await.unwrap_err();

    assert!(matches!(err, SpoolError::CapacityExceeded(_)));

    let _ = std::fs::remove_file(&spool_path);
}

#[tokio::test]
async fn test_spool_duplicate_replay_idempotency_prevents_double_counting() {
    let storage = Arc::new(UsageStorage::new(":memory:").unwrap());
    let spool_path = unique_temp_spool_path("idempotent_replay");
    let spool = UsageSpooler::new(&spool_path, 1024 * 1024);

    let rec = sample_record("rec_duplicate_check");
    // Append record to spool
    spool.append_record(&rec, "transient error").await.unwrap();

    // First replay into SQLite
    let first_drain = spool.drain_to_storage(&storage, 10).await.unwrap();
    assert_eq!(first_drain.succeeded, 1);

    // Verify record in SQLite
    let records = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, "rec_duplicate_check");

    // Simulate ambiguous crash scenario where spool was re-populated with same record before acknowledgement
    spool.append_record(&rec, "retry attempt").await.unwrap();

    // Second replay into SQLite: must be idempotent and NOT create a duplicate record
    let second_drain = spool.drain_to_storage(&storage, 10).await.unwrap();
    assert_eq!(second_drain.succeeded, 1);

    // Verify exactly ONE record remains in SQLite (no duplicate row or token inflation)
    let records_after = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(
        records_after.len(),
        1,
        "Duplicate replay must not create duplicate SQLite rows"
    );
    assert_eq!(records_after[0].id, "rec_duplicate_check");

    let _ = std::fs::remove_file(&spool_path);
}

#[tokio::test]
async fn test_windows_spool_partial_drain_and_atomic_replacement() {
    let storage = Arc::new(UsageStorage::new(":memory:").unwrap());
    let spool_path = unique_temp_spool_path("win_atomic_replace");
    let spool = UsageSpooler::new(&spool_path, 1024 * 1024);

    let rec1 = sample_record("rec_part_1");
    let rec2 = sample_record("rec_part_2");
    let rec3 = sample_record("rec_part_3");

    spool.append_record(&rec1, "err1").await.unwrap();
    spool.append_record(&rec2, "err2").await.unwrap();
    spool.append_record(&rec3, "err3").await.unwrap();

    let initial_metrics = spool.metrics().await.unwrap();
    assert_eq!(initial_metrics.pending_entries, 3);

    // Partial drain: batch_limit = 1
    // Replays 1 record, rewrites temp file with 2 remaining records, and replaces spool_path
    let drain_part = spool.drain_to_storage(&storage, 1).await.unwrap();
    assert_eq!(drain_part.processed, 1);
    assert_eq!(drain_part.succeeded, 1);

    // Verify spool still exists and contains exactly 2 records
    assert!(
        spool_path.exists(),
        "Spool file must exist after partial drain"
    );
    let mid_metrics = spool.metrics().await.unwrap();
    assert_eq!(mid_metrics.pending_entries, 2);

    // Drain remaining 2 records
    let drain_rest = spool.drain_to_storage(&storage, 10).await.unwrap();
    assert_eq!(drain_rest.processed, 2);
    assert_eq!(drain_rest.succeeded, 2);

    // Verify spool file was deleted when empty
    assert!(
        !spool_path.exists(),
        "Spool file must be removed when all entries drained"
    );

    // Verify all 3 records committed to SQLite
    let all_records = storage
        .get_records(ai_backend::usage::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(all_records.len(), 3);
}
