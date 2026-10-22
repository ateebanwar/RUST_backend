//! Comprehensive verification tests for LocalSyncStore persistent SQLite storage.
//!
//! Conforms to MASTER_SPEC.md §15, §17 and Phase 15 requirements:
//! 1. Write data, recreate the store, and verify the data remains.
//! 2. Restart/reinitialization behavior with existing persisted data.
//! 3. Missing or empty storage.
//! 4. Corrupted or invalid persisted data.
//! 5. Interrupted or failed writes without silent data loss.
//! 6. Concurrent operations and consistency.
//! 7. Repeated initialization and duplicate writes.
//! 8. Compatibility with existing functionality and encryption safeguards.
//! 9. Tenant isolation and logout cleanup.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::drive::{
    LibraryItem, LocalStoreError, LocalSyncStore, SyncConversation, SyncMessage, compute_sha256,
};
use rusqlite::Connection;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Generate a unique isolated test database path inside std::env::temp_dir().
fn isolated_test_db_path(test_name: &str) -> (std::path::PathBuf, String) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("ai_backend_sync_test_{test_name}_{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("sync_store.db").to_str().unwrap().to_string();
    (dir, db_path)
}

#[tokio::test]
async fn test_01_write_data_recreate_store_persists_all_fields() {
    let (temp_dir, db_path) = isolated_test_db_path("01_write_recreate");

    let owner = "google:user_alice";

    // 1. Open persistent store, write a rich conversation and a library item
    {
        let store = LocalSyncStore::open(&db_path).expect("Failed to open store");
        assert!(store.is_persistent());

        let mut conv = SyncConversation::new(
            "conv_persist_1".to_string(),
            owner.to_string(),
            "Durable Conversation".to_string(),
        );
        conv.messages.push(SyncMessage::new(
            "msg_1".to_string(),
            "conv_persist_1".to_string(),
            "user".to_string(),
            "Hello, persistence!".to_string(),
        ));
        conv.messages.push(SyncMessage::new(
            "msg_2".to_string(),
            "conv_persist_1".to_string(),
            "assistant".to_string(),
            "I will survive restart.".to_string(),
        ));

        let saved_conv = store.upsert_conversation(owner, conv).await.unwrap();
        assert_eq!(saved_conv.revision, 1);
        assert_eq!(saved_conv.messages.len(), 2);

        let binary_payload = b"TEST_BINARY_FILE_PAYLOAD_123456789";
        let lib_item = LibraryItem::new(
            "lib_persist_1".to_string(),
            owner.to_string(),
            "data.bin".to_string(),
            "application/octet-stream".to_string(),
            binary_payload.len() as u64,
            compute_sha256(binary_payload),
            Some("conv_persist_1".to_string()),
        );

        let saved_lib = store
            .save_library_item(owner, lib_item, Some(binary_payload.to_vec()))
            .await
            .unwrap();
        assert_eq!(saved_lib.size_bytes, binary_payload.len() as u64);

        // Store has pending changes
        let (pending_conv, pending_lib, _, _) = store.get_sync_stats(owner).await;
        assert_eq!(pending_conv, 1);
        assert_eq!(pending_lib, 1);
    } // Explicitly drop store handle to simulate full shutdown

    // 2. Reopen store from disk (simulating server restart)
    {
        let store2 = LocalSyncStore::open(&db_path).expect("Failed to reload store");
        assert!(store2.is_persistent());

        // Verify conversation survived
        let reloaded_conv = store2
            .get_conversation(owner, "conv_persist_1")
            .await
            .expect("Conversation must survive restart");
        assert_eq!(reloaded_conv.title, "Durable Conversation");
        assert_eq!(reloaded_conv.revision, 1);
        assert_eq!(reloaded_conv.messages.len(), 2);
        assert_eq!(reloaded_conv.messages[0].content, "Hello, persistence!");
        assert_eq!(reloaded_conv.messages[1].content, "I will survive restart.");

        // Verify library item survived
        let reloaded_lib = store2
            .get_library_item(owner, "lib_persist_1")
            .await
            .expect("Library item metadata must survive restart");
        assert_eq!(reloaded_lib.filename, "data.bin");
        assert_eq!(
            reloaded_lib.conversation_id,
            Some("conv_persist_1".to_string())
        );

        // Verify binary payload survived
        let reloaded_content = store2
            .get_library_content(owner, "lib_persist_1")
            .await
            .expect("Library binary payload must survive restart");
        assert_eq!(reloaded_content, b"TEST_BINARY_FILE_PAYLOAD_123456789");

        // Verify pending dirty flags survived
        let (pending_conv, pending_lib, _, _) = store2.get_sync_stats(owner).await;
        assert_eq!(
            pending_conv, 1,
            "Dirty conversation flag must survive restart"
        );
        assert_eq!(
            pending_lib, 1,
            "Dirty library item flag must survive restart"
        );

        let pending_conv_list = store2.get_pending_conversations(owner).await;
        assert_eq!(pending_conv_list.len(), 1);
        assert!(pending_conv_list[0].is_dirty);
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_02_restart_with_synced_state_and_tombs() {
    let (temp_dir, db_path) = isolated_test_db_path("02_synced_tombs");
    let owner = "google:user_bob";

    // 1. Setup synced items, tombstones, and sync stats
    {
        let store = LocalSyncStore::open(&db_path).unwrap();

        let conv = SyncConversation::new(
            "conv_tomb".to_string(),
            owner.to_string(),
            "To be deleted".to_string(),
        );
        store.upsert_conversation(owner, conv).await.unwrap();

        // Mark synced to Drive
        store
            .mark_conversation_synced(owner, "conv_tomb", "drive_file_conv_99".to_string())
            .await
            .unwrap();

        // Soft delete (tombstone)
        let deleted = store.delete_conversation(owner, "conv_tomb").await.unwrap();
        assert!(deleted);

        // Add library item and mark synced
        let lib = LibraryItem::new(
            "lib_synced".to_string(),
            owner.to_string(),
            "photo.jpg".to_string(),
            "image/jpeg".to_string(),
            4,
            "abc123sha".to_string(),
            Some("conv_tomb".to_string()),
        );
        store
            .save_library_item(owner, lib, Some(vec![1, 2, 3, 4]))
            .await
            .unwrap();
        store
            .mark_library_item_synced(owner, "lib_synced", "drive_file_lib_88".to_string())
            .await
            .unwrap();

        // Update sync status
        store
            .update_sync_status(
                owner,
                Some("2026-10-22T12:00:00Z".to_string()),
                Some("Temporary network glitch".to_string()),
            )
            .await
            .unwrap();
    } // Shutdown

    // 2. Reopen and verify tombstones, synced IDs, and independent library lifecycle
    {
        let store = LocalSyncStore::open(&db_path).unwrap();

        // Active list should not include soft-deleted conversation
        assert!(store.get_conversation(owner, "conv_tomb").await.is_none());
        assert_eq!(store.list_conversations(owner).await.len(), 0);

        // Raw query (for sync reconcile) must retrieve tombstone
        let tombstone = store
            .get_conversation_raw(owner, "conv_tomb")
            .await
            .expect("Tombstone must exist");
        assert!(tombstone.deleted);
        assert!(tombstone.deleted_at.is_some());
        assert_eq!(tombstone.revision, 2);

        // CRITICAL: Library item survived conversation deletion across restart!
        let lib = store
            .get_library_item(owner, "lib_synced")
            .await
            .expect("Library item must survive chat deletion and restart");
        assert_eq!(lib.filename, "photo.jpg");
        assert_eq!(lib.drive_file_id, Some("drive_file_lib_88".to_string()));
        assert_eq!(
            store.get_library_content(owner, "lib_synced").await,
            Some(vec![1, 2, 3, 4])
        );

        // Verify sync metadata survived
        let (_, _, last_sync, last_err) = store.get_sync_stats(owner).await;
        assert_eq!(last_sync, Some("2026-10-22T12:00:00Z".to_string()));
        assert_eq!(last_err, Some("Temporary network glitch".to_string()));
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_03_missing_or_empty_storage() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let nested_dir = std::env::temp_dir().join(format!("ai_backend_nested_{nanos}/sub/dir"));
    let db_path = nested_dir
        .join("auto_create.db")
        .to_str()
        .unwrap()
        .to_string();

    // Verify directory does not exist yet
    assert!(!nested_dir.exists());

    // 1. Open should create parent directories and initialize schema without error
    let store = LocalSyncStore::open(&db_path).expect("Store must auto-create missing directories");
    assert!(nested_dir.exists());

    // Empty store behaves correctly
    let owner = "google:user_empty";
    assert_eq!(store.list_conversations(owner).await.len(), 0);
    assert_eq!(store.list_library_items(owner).await.len(), 0);
    assert_eq!(store.get_sync_stats(owner).await, (0, 0, None, None));

    // Drop and reopen empty store
    drop(store);
    let store2 = LocalSyncStore::open(&db_path).expect("Reopening empty store must succeed");
    assert_eq!(store2.list_conversations(owner).await.len(), 0);

    let _ =
        std::fs::remove_dir_all(std::env::temp_dir().join(format!("ai_backend_nested_{nanos}")));
}

#[tokio::test]
async fn test_04_corrupted_or_invalid_persisted_data() {
    let (temp_dir, db_path) = isolated_test_db_path("04_corrupted");

    // Case A: File is completely corrupt (random non-sqlite bytes)
    {
        std::fs::write(&db_path, b"CORRUPTED_NON_SQLITE_GARBAGE_BYTES_1234567890").unwrap();
        let res = LocalSyncStore::open(&db_path);
        assert!(
            res.is_err(),
            "Opening corrupt database file must fail fast and closed"
        );
        match res.unwrap_err() {
            LocalStoreError::ConnectionError(_)
            | LocalStoreError::MigrationError(_)
            | LocalStoreError::SqliteError(_) => {}
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    // Case B: Valid SQLite DB, but a row contains malformed messages_json
    {
        std::fs::remove_file(&db_path).unwrap();
        let store = LocalSyncStore::open(&db_path).unwrap();
        let owner = "google:user_corrupt";

        // Insert valid conversation
        let conv = SyncConversation::new(
            "conv_valid".to_string(),
            owner.to_string(),
            "Valid Chat".to_string(),
        );
        store.upsert_conversation(owner, conv).await.unwrap();
        drop(store);

        // Manually inject a row with malformed JSON directly into SQLite
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT INTO local_sync_conversations (
                    id, owner_subject, title, created_at, updated_at, revision, schema_version, deleted, deleted_at, is_dirty, drive_file_id, messages_json
                ) VALUES ('conv_bad_json', ?1, 'Corrupted JSON Chat', '2026-10-22', '2026-10-22', 1, 1, 0, NULL, 0, NULL, '{MALFORMED_JSON_NOT_VALID...')",
                [owner],
            ).unwrap();
        }

        // Reopening store must NOT crash or fail — it safely recovers valid records
        let store_recovered = LocalSyncStore::open(&db_path)
            .expect("Store must safely handle row corruption during cache reload");

        // The valid conversation is fully preserved
        let valid_conv = store_recovered
            .get_conversation(owner, "conv_valid")
            .await
            .expect("Valid conversation must be preserved");
        assert_eq!(valid_conv.title, "Valid Chat");

        // The corrupt row defaulted to empty messages without aborting
        let corrupt_conv = store_recovered
            .get_conversation(owner, "conv_bad_json")
            .await
            .expect("Corrupt row safely recovered");
        assert_eq!(corrupt_conv.title, "Corrupted JSON Chat");
        assert_eq!(corrupt_conv.messages.len(), 0);
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_05_failed_writes_do_not_silently_appear_successful() {
    let (temp_dir, db_path) = isolated_test_db_path("05_write_failure");
    let owner = "google:user_failure";

    let store = LocalSyncStore::open(&db_path).unwrap();

    // Insert an initial conversation
    let conv = SyncConversation::new(
        "conv_fail_test".to_string(),
        owner.to_string(),
        "Initial Title".to_string(),
    );
    store.upsert_conversation(owner, conv).await.unwrap();

    // Simulate an interrupted write / DB lock failure by dropping tables or locking externally
    {
        let conn = Connection::open(&db_path).unwrap();
        // Drop the conversation table to simulate a catastrophic storage failure
        conn.execute("DROP TABLE local_sync_conversations", [])
            .unwrap();
    }

    // Attempting to upsert another conversation must return an Err, NOT silently appear successful
    let conv_new = SyncConversation::new(
        "conv_new_fail".to_string(),
        owner.to_string(),
        "Should Fail".to_string(),
    );
    let res = store.upsert_conversation(owner, conv_new).await;
    assert!(
        res.is_err(),
        "Failed write must return Err, never report silent success"
    );

    // Attempting to delete must also return an Err
    let del_res = store.delete_conversation(owner, "conv_fail_test").await;
    assert!(del_res.is_err(), "Delete on failed storage must return Err");

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_06_concurrent_operations_and_consistency() {
    let (temp_dir, db_path) = isolated_test_db_path("06_concurrency");
    let store = Arc::new(LocalSyncStore::open(&db_path).unwrap());
    let mut handles = Vec::new();

    // Spawn 10 concurrent tasks writing conversations and library items
    for task_idx in 0..10 {
        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            let owner = format!("google:tenant_{task_idx}");
            for item_idx in 0..10 {
                let conv_id = format!("conv_{task_idx}_{item_idx}");
                let conv = SyncConversation::new(
                    conv_id.clone(),
                    owner.clone(),
                    format!("Task {task_idx} Chat {item_idx}"),
                );
                store_clone.upsert_conversation(&owner, conv).await.unwrap();

                let lib_id = format!("lib_{task_idx}_{item_idx}");
                let lib = LibraryItem::new(
                    lib_id.clone(),
                    owner.clone(),
                    format!("file_{item_idx}.dat"),
                    "text/plain".to_string(),
                    10,
                    "sha256fake".to_string(),
                    Some(conv_id),
                );
                store_clone
                    .save_library_item(&owner, lib, Some(vec![task_idx as u8; 10]))
                    .await
                    .unwrap();
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all 100 conversations and 100 library items exist in memory
    for task_idx in 0..10 {
        let owner = format!("google:tenant_{task_idx}");
        assert_eq!(store.list_conversations(&owner).await.len(), 10);
        assert_eq!(store.list_library_items(&owner).await.len(), 10);
    }

    // Drop store and reopen from SQLite: verify all 100 survived with zero loss
    drop(store);
    let store_reloaded = LocalSyncStore::open(&db_path).unwrap();
    for task_idx in 0..10 {
        let owner = format!("google:tenant_{task_idx}");
        assert_eq!(store_reloaded.list_conversations(&owner).await.len(), 10);
        assert_eq!(store_reloaded.list_library_items(&owner).await.len(), 10);
    }

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_07_repeated_initialization_and_duplicate_writes() {
    let (temp_dir, db_path) = isolated_test_db_path("07_duplicates");
    let owner = "google:user_dup";

    // 1. Multiple opens are idempotent
    for _ in 0..3 {
        let store = LocalSyncStore::open(&db_path).unwrap();
        assert!(store.is_persistent());
    }

    let store = LocalSyncStore::open(&db_path).unwrap();

    // 2. Repeated upserts of the same conversation monotonically increments revision
    let conv = SyncConversation::new(
        "conv_dup".to_string(),
        owner.to_string(),
        "Version 1".to_string(),
    );
    let s1 = store.upsert_conversation(owner, conv).await.unwrap();
    assert_eq!(s1.revision, 1);

    let mut conv_v2 = s1.clone();
    conv_v2.title = "Version 2".to_string();
    let s2 = store.upsert_conversation(owner, conv_v2).await.unwrap();
    assert_eq!(s2.revision, 2);

    let mut conv_v3 = s2.clone();
    conv_v3.title = "Version 3".to_string();
    let s3 = store.upsert_conversation(owner, conv_v3).await.unwrap();
    assert_eq!(s3.revision, 3);

    // Exactly one row exists in SQLite with revision 3
    assert_eq!(store.list_conversations(owner).await.len(), 1);

    // Drop and reload: revision 3 preserved
    drop(store);
    let store2 = LocalSyncStore::open(&db_path).unwrap();
    let loaded = store2.get_conversation(owner, "conv_dup").await.unwrap();
    assert_eq!(loaded.title, "Version 3");
    assert_eq!(loaded.revision, 3);

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_08_compatibility_with_encryption_safeguards_and_app_state() {
    let (temp_dir, db_path) = isolated_test_db_path("08_app_state");
    let owner = "google:app_state_user";

    // 1. Initialize AppState with persistent database
    let mut config = AppConfig::default_local();
    config.database_path = db_path.clone();
    config.drive_encryption_key =
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
    config.google_drive_sync_enabled = true;

    let state = AppState::new(Arc::new(config.clone()));
    assert!(state.sync_engine.local_store().is_persistent());

    // 2. Add conversation via AppState sync_engine
    let conv = SyncConversation::new(
        "conv_app_state".to_string(),
        owner.to_string(),
        "AppState Chat".to_string(),
    );
    state
        .sync_engine
        .local_store()
        .upsert_conversation(owner, conv)
        .await
        .unwrap();

    // 3. Recreate AppState (simulating server restart)
    drop(state);
    let state2 = AppState::new(Arc::new(config.clone()));
    let reloaded = state2
        .sync_engine
        .local_store()
        .get_conversation(owner, "conv_app_state")
        .await
        .expect("AppState server restart must preserve local conversation");
    assert_eq!(reloaded.title, "AppState Chat");

    // 4. Verify fail-closed behavior when encryption key is missing
    let mut bad_config = config.clone();
    bad_config.drive_encryption_key = None; // Missing key
    let bad_state = AppState::new(Arc::new(bad_config));

    // Operations requiring crypto fail closed without corrupting stored data
    let sync_res = bad_state.sync_engine.sync(owner, "dummy_token").await;
    assert!(
        sync_res.is_err(),
        "Sync without encryption key must fail closed"
    );

    // Data remains safe and uncorrupted in store
    assert!(
        bad_state
            .sync_engine
            .local_store()
            .get_conversation(owner, "conv_app_state")
            .await
            .is_some()
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_09_logout_clears_user_session_and_preserves_other_tenants() {
    let (temp_dir, db_path) = isolated_test_db_path("09_tenant_logout");
    let tenant_a = "google:alice";
    let tenant_b = "google:bob";

    let store = LocalSyncStore::open(&db_path).unwrap();

    // Both tenants write data
    store
        .upsert_conversation(
            tenant_a,
            SyncConversation::new(
                "conv_a".to_string(),
                tenant_a.to_string(),
                "Alice Chat".to_string(),
            ),
        )
        .await
        .unwrap();
    store
        .save_library_item(
            tenant_a,
            LibraryItem::new(
                "lib_a".to_string(),
                tenant_a.to_string(),
                "a.txt".to_string(),
                "text/plain".to_string(),
                1,
                "shaA".to_string(),
                None,
            ),
            Some(vec![0xAA]),
        )
        .await
        .unwrap();

    store
        .upsert_conversation(
            tenant_b,
            SyncConversation::new(
                "conv_b".to_string(),
                tenant_b.to_string(),
                "Bob Chat".to_string(),
            ),
        )
        .await
        .unwrap();
    store
        .save_library_item(
            tenant_b,
            LibraryItem::new(
                "lib_b".to_string(),
                tenant_b.to_string(),
                "b.txt".to_string(),
                "text/plain".to_string(),
                1,
                "shaB".to_string(),
                None,
            ),
            Some(vec![0xBB]),
        )
        .await
        .unwrap();

    // Alice logs out
    store.clear_user_session(tenant_a).await.unwrap();

    // In memory: Alice is empty, Bob is intact
    assert!(store.get_conversation(tenant_a, "conv_a").await.is_none());
    assert!(store.get_library_item(tenant_a, "lib_a").await.is_none());
    assert!(store.get_conversation(tenant_b, "conv_b").await.is_some());
    assert!(store.get_library_item(tenant_b, "lib_b").await.is_some());

    // Drop and reload from SQLite (server restart)
    drop(store);
    let store2 = LocalSyncStore::open(&db_path).unwrap();

    // Persistent storage: Alice remains cleared, Bob remains 100% intact
    assert!(store2.get_conversation(tenant_a, "conv_a").await.is_none());
    assert!(store2.get_library_item(tenant_a, "lib_a").await.is_none());
    assert!(store2.get_conversation(tenant_b, "conv_b").await.is_some());
    assert!(store2.get_library_item(tenant_b, "lib_b").await.is_some());
    assert_eq!(
        store2.get_library_content(tenant_b, "lib_b").await,
        Some(vec![0xBB])
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}
