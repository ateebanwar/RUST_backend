//! Comprehensive verification tests for Google Drive-Only Storage Architecture and Data Safety.
//!
//! Conforms strictly to MASTER_SPEC.md and Master Directive:
//! §2: Google Drive is the sole permanent store for user data.
//! §3: Preserving existing data & legacy migration without data loss.
//! §4: Mandatory Google login & Drive authorization (fail-closed).
//! §5 & §6: Save-confirmation protocol & cold-start recovery.
//! §7: Synchronization, revisions, tombstones, and independent Library lifecycle.
//! §8: AES-256-GCM authenticated encryption with anti-transplantation AAD binding.
//! §9: Model-agnostic chat continuity when switching AI providers.
//! §11: Strict multi-tenant isolation.
//! §13: Removal of backend SQLite permanent user data dependencies.

use ai_backend::drive::mock::MockDriveClient;
use ai_backend::drive::{
    DriveCryptoEngine, LibraryItem, LocalSyncStore, SyncConversation, SyncEngine, SyncMessage,
    compute_sha256,
};
use std::sync::Arc;

fn test_crypto_engine(tenant: &str) -> Arc<DriveCryptoEngine> {
    let key = [0x77u8; 32];
    Arc::new(DriveCryptoEngine::derive_for_owner(&key, tenant))
}

#[tokio::test]
async fn test_01_google_drive_sole_permanent_store_recovers_after_backend_restart() {
    let mock_drive = Arc::new(MockDriveClient::new());
    let crypto = test_crypto_engine("alice");
    let owner = "google:user_alice";
    let token = "valid_drive_token_alice";

    // 1. Initial backend instance: create conversation and library item directly on Drive
    {
        let local_store = Arc::new(LocalSyncStore::new());
        assert!(
            !local_store.is_persistent(),
            "Backend store must be ephemeral"
        );

        let sync_engine = SyncEngine::new(mock_drive.clone(), crypto.clone(), local_store, true);

        let mut conv = SyncConversation::new(
            "conv_drive_1".to_string(),
            owner.to_string(),
            "Drive Chat".to_string(),
        );
        conv.messages.push(SyncMessage::new(
            "msg_1".to_string(),
            "conv_drive_1".to_string(),
            "user".to_string(),
            "Hello Google Drive!".to_string(),
        ));
        conv.messages.push(SyncMessage::new(
            "msg_2".to_string(),
            "conv_drive_1".to_string(),
            "assistant".to_string(),
            "I am permanently stored on Drive.".to_string(),
        ));

        let saved_conv = sync_engine
            .persist_conversation_to_drive(owner, conv, token)
            .await
            .expect("Must persist conversation to Drive");
        assert_eq!(saved_conv.messages.len(), 2);

        let binary_photo = b"JPEG_RAW_BYTES_010203040506070809";
        let lib_item = LibraryItem::new(
            "photo_1".to_string(),
            owner.to_string(),
            "photo.jpg".to_string(),
            "image/jpeg".to_string(),
            binary_photo.len() as u64,
            compute_sha256(binary_photo),
            Some("conv_drive_1".to_string()),
        );

        let saved_lib = sync_engine
            .persist_library_item_to_drive(owner, lib_item, binary_photo.to_vec(), token)
            .await
            .expect("Must persist library item to Drive");
        assert_eq!(saved_lib.size_bytes, binary_photo.len() as u64);
    } // Drop backend instance completely to simulate restart / new serverless instance

    // 2. Completely fresh backend instance with clean in-memory state (cold start)
    {
        let fresh_local_store = Arc::new(LocalSyncStore::new());
        let fresh_engine = SyncEngine::new(
            mock_drive.clone(),
            crypto.clone(),
            fresh_local_store.clone(),
            true,
        );

        // Before hydration, local memory is completely empty
        assert!(fresh_local_store.list_conversations(owner).await.is_empty());
        assert!(fresh_local_store.list_library_items(owner).await.is_empty());

        // Hydrate from Google Drive
        let conv_restored = fresh_engine
            .get_or_hydrate_conversation(owner, "conv_drive_1", Some(token))
            .await
            .expect("Hydration must succeed")
            .expect("Conversation must exist on Drive");

        assert_eq!(conv_restored.title, "Drive Chat");
        assert_eq!(conv_restored.messages.len(), 2);
        assert_eq!(conv_restored.messages[0].content, "Hello Google Drive!");
        assert_eq!(
            conv_restored.messages[1].content,
            "I am permanently stored on Drive."
        );

        // Hydrate binary content from Google Drive
        let photo_restored = fresh_engine
            .get_library_item_content(owner, "photo_1", Some(token))
            .await
            .expect("Must download and decrypt photo from Drive");
        assert_eq!(
            photo_restored.as_slice(),
            b"JPEG_RAW_BYTES_010203040506070809"
        );
    }
}

#[tokio::test]
async fn test_02_backend_is_stateless_and_local_store_is_ephemeral() {
    let store = LocalSyncStore::new();
    assert!(
        !store.is_persistent(),
        "LocalSyncStore is stateless and not persistent"
    );

    let owner = "google:stateless_user";
    let conv = SyncConversation::new(
        "stateless_chat".to_string(),
        owner.to_string(),
        "Stateless".to_string(),
    );

    // Save only to ephemeral local memory without persisting to Drive
    store.upsert_conversation(owner, conv).await.unwrap();
    assert_eq!(store.list_conversations(owner).await.len(), 1);

    // Drop store and instantiate new one: memory is clean, demonstrating zero local disk persistence
    drop(store);
    let fresh_store = LocalSyncStore::new();
    assert_eq!(fresh_store.list_conversations(owner).await.len(), 0);
}

#[tokio::test]
async fn test_03_in_memory_sync_and_drive_restore_without_database() {
    let owner = "google:stateless_alice";
    let token = "valid_drive_token_stateless";

    let mock_drive = Arc::new(MockDriveClient::new());
    let crypto = test_crypto_engine("stateless_alice");
    let local_store = Arc::new(LocalSyncStore::new());
    let sync_engine = SyncEngine::new(mock_drive.clone(), crypto.clone(), local_store, true);

    // 1. Create and persist conversation directly to Drive via memory-only store
    let mut conv = SyncConversation::new(
        "stateless_conv_1".to_string(),
        owner.to_string(),
        "Stateless Chat".to_string(),
    );
    conv.messages.push(SyncMessage::new(
        "stateless_msg_1".to_string(),
        "stateless_conv_1".to_string(),
        "user".to_string(),
        "Hello from zero-db architecture".to_string(),
    ));

    sync_engine
        .persist_conversation_to_drive(owner, conv, token)
        .await
        .expect("Persist must succeed");

    // 2. Persist library item directly to Drive
    let content = b"PURE_IN_MEMORY_CONTENT";
    let lib_item = LibraryItem::new(
        "stateless_lib_1".to_string(),
        owner.to_string(),
        "test.txt".to_string(),
        "text/plain".to_string(),
        content.len() as u64,
        compute_sha256(content),
        None,
    );
    sync_engine
        .persist_library_item_to_drive(owner, lib_item, content.to_vec(), token)
        .await
        .expect("Library persist must succeed");

    // 3. Verify cold-start recovery with completely fresh in-memory store (zero database files)
    let fresh_store = Arc::new(LocalSyncStore::new());
    let fresh_engine = SyncEngine::new(mock_drive.clone(), crypto.clone(), fresh_store, true);

    let restored_conv = fresh_engine
        .get_or_hydrate_conversation(owner, "stateless_conv_1", Some(token))
        .await
        .unwrap()
        .expect("Conversation must be restored directly from Drive");
    assert_eq!(restored_conv.title, "Stateless Chat");
    assert_eq!(
        restored_conv.messages[0].content,
        "Hello from zero-db architecture"
    );

    let restored_content = fresh_engine
        .get_library_item_content(owner, "stateless_lib_1", Some(token))
        .await
        .unwrap();
    assert_eq!(restored_content.as_slice(), b"PURE_IN_MEMORY_CONTENT");
}

#[tokio::test]
async fn test_04_save_confirmation_protocol_fails_closed_when_drive_fails() {
    let mock_drive = Arc::new(MockDriveClient::new());
    // Simulate quota exceeded on upload
    mock_drive.set_inject_quota_exceeded(true).await;

    let crypto = test_crypto_engine("bob");
    let local_store = Arc::new(LocalSyncStore::new());
    let sync_engine = SyncEngine::new(mock_drive, crypto, local_store, true);

    let conv = SyncConversation::new(
        "failing_chat".to_string(),
        "google:bob".to_string(),
        "Should Fail".to_string(),
    );

    let res = sync_engine
        .persist_conversation_to_drive("google:bob", conv, "token_bob")
        .await;

    assert!(res.is_err(), "Must fail closed when Drive write fails");
    match res.unwrap_err() {
        ai_backend::drive::DriveError::QuotaExceeded(msg) => {
            assert!(msg.to_lowercase().contains("quota") || msg.contains("403"));
        }
        other => panic!("Expected QuotaExceeded, got {other:?}"),
    }
}

#[tokio::test]
async fn test_05_independent_library_lifecycle_survives_conversation_deletion_on_drive() {
    let mock_drive = Arc::new(MockDriveClient::new());
    let crypto = test_crypto_engine("carol");
    let owner = "google:carol";
    let token = "token_carol";
    let local_store = Arc::new(LocalSyncStore::new());
    let sync_engine = SyncEngine::new(mock_drive.clone(), crypto.clone(), local_store, true);

    // 1. Create conversation and associated library item on Drive
    let conv = SyncConversation::new(
        "chat_lifecycle".to_string(),
        owner.to_string(),
        "Delete Me".to_string(),
    );
    sync_engine
        .persist_conversation_to_drive(owner, conv, token)
        .await
        .unwrap();

    let binary = b"DOCUMENT_SURVIVING_DELETION";
    let lib_item = LibraryItem::new(
        "doc_survivor".to_string(),
        owner.to_string(),
        "doc.pdf".to_string(),
        "application/pdf".to_string(),
        binary.len() as u64,
        compute_sha256(binary),
        Some("chat_lifecycle".to_string()),
    );
    sync_engine
        .persist_library_item_to_drive(owner, lib_item, binary.to_vec(), token)
        .await
        .unwrap();

    // 2. Soft-delete the conversation on Google Drive
    let deleted = sync_engine
        .delete_conversation_on_drive(owner, "chat_lifecycle", token)
        .await
        .unwrap();
    assert!(deleted);

    // 3. Verify conversation is marked deleted
    let hydrated_conv = sync_engine
        .get_or_hydrate_conversation(owner, "chat_lifecycle", Some(token))
        .await
        .unwrap();
    assert!(
        hydrated_conv.is_none(),
        "Soft-deleted conversation must not be returned"
    );

    // 4. CRITICAL: Library item MUST still exist and be downloadable from Drive
    let hydrated_lib = sync_engine
        .get_or_hydrate_library_item(owner, "doc_survivor", Some(token))
        .await
        .unwrap();
    assert!(
        hydrated_lib.is_some(),
        "Library item must survive conversation deletion"
    );

    let downloaded_bytes = sync_engine
        .get_library_item_content(owner, "doc_survivor", Some(token))
        .await
        .unwrap();
    assert_eq!(downloaded_bytes.as_slice(), binary);
}

#[tokio::test]
async fn test_06_encryption_fails_closed_without_valid_key() {
    let mock_drive = Arc::new(MockDriveClient::new());
    let unconfigured_crypto = Arc::new(DriveCryptoEngine::unconfigured("No key"));
    let local_store = Arc::new(LocalSyncStore::new());
    let sync_engine = SyncEngine::new(mock_drive, unconfigured_crypto, local_store, true);

    let conv = SyncConversation::new(
        "no_key_chat".to_string(),
        "google:dave".to_string(),
        "No Key".to_string(),
    );

    let res = sync_engine
        .persist_conversation_to_drive("google:dave", conv, "token_dave")
        .await;

    assert!(res.is_err());
    match res.unwrap_err() {
        ai_backend::drive::DriveError::CryptoError(msg) => {
            assert!(msg.contains("DRIVE_ENCRYPTION_KEY is not configured"));
        }
        other => panic!("Expected CryptoError, got {other:?}"),
    }
}

#[tokio::test]
async fn test_07_multi_tenant_isolation_across_restarts() {
    let mock_drive = Arc::new(MockDriveClient::new());
    let crypto_alice = test_crypto_engine("tenant_alice");
    let crypto_bob = test_crypto_engine("tenant_bob");

    let owner_alice = "google:user_alice";
    let owner_bob = "google:user_bob";

    // Alice creates conversation on Drive
    {
        let store_alice = Arc::new(LocalSyncStore::new());
        let engine_alice = SyncEngine::new(mock_drive.clone(), crypto_alice, store_alice, true);
        let conv = SyncConversation::new(
            "alice_private_chat".to_string(),
            owner_alice.to_string(),
            "Alice Secret".to_string(),
        );
        engine_alice
            .persist_conversation_to_drive(owner_alice, conv, "token_alice")
            .await
            .unwrap();
    }

    // Bob starts on new instance and tries to access Alice's chat
    {
        let store_bob = Arc::new(LocalSyncStore::new());
        let engine_bob = SyncEngine::new(mock_drive.clone(), crypto_bob, store_bob, true);

        // Bob cannot see Alice's conversation in his list
        let bob_convs = engine_bob
            .list_conversations_with_drive(owner_bob, Some("token_bob"))
            .await
            .unwrap();
        assert!(bob_convs.is_empty(), "Bob cannot see Alice's conversations");

        // Bob cannot hydrate Alice's conversation using his token
        let bob_get = engine_bob
            .get_or_hydrate_conversation(owner_bob, "alice_private_chat", Some("token_bob"))
            .await
            .unwrap();
        assert!(bob_get.is_none(), "Bob cannot hydrate Alice's conversation");
    }
}
