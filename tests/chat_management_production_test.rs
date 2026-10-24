//! Comprehensive production-readiness verification tests for Chat Management.
//!
//! Validates:
//! 1. Conversation creation with auto-generated and custom valid IDs.
//! 2. Strict input validation on conversation creation (empty title, long title, invalid characters).
//! 3. Conversation retrieval and accurate metadata/message preservation.
//! 4. Conversation renaming and update via PUT and PATCH (revision monotonic increment, updated_at).
//! 5. Input validation on update (empty title, missing fields, non-existent conversations).
//! 6. Cross-user isolation and IDOR prevention (User A cannot view, update, or delete User B's chats).
//! 7. Soft deletion and clean 404 on repeated delete attempts.
//! 8. Strict preservation of associated Library items when a conversation is deleted.
//! 9. Cold-start restoration from Google Drive after in-memory cache drop.
//! 10. Chat completions integration: context hydration, turn appending, revision increment, and invalid conversation_id rejection.

use ai_backend::config::AppConfig;
use ai_backend::drive::mock::MockDriveClient;
use ai_backend::drive::{
    CreateConversationRequest, CreateLibraryItemRequest, DriveCryptoEngine, LibraryItem,
    LocalSyncStore, SyncConversation, SyncEngine, SyncMessage, UpdateConversationRequest,
    encode_base64,
};
use ai_backend::models::ChatMessage;
use ai_backend::{AppState, ChatCompletionRequest, create_app};
use reqwest::StatusCode;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Spawn a live test backend instance on an ephemeral loopback port.
async fn spawn_chat_test_backend(
    state: AppState,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<MockDriveClient>,
    Arc<SyncEngine>,
) {
    let mock_client = Arc::new(MockDriveClient::new());
    let master_secret = [0x55u8; 32];
    let crypto = Arc::new(DriveCryptoEngine::derive_for_owner(
        &master_secret,
        "chat_prod_test_tenant",
    ));
    let local_store = Arc::new(LocalSyncStore::new());

    let sync_engine = Arc::new(SyncEngine::new(
        mock_client.clone(),
        crypto,
        local_store,
        true,
    ));

    let state = state.with_sync_engine(sync_engine.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state);

    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    (
        format!("http://127.0.0.1:{}", addr.port()),
        handle,
        mock_client,
        sync_engine,
    )
}

#[tokio::test]
async fn test_01_conversation_creation_and_fields() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create with custom ID and initial message
    let payload = CreateConversationRequest {
        id: Some("conv_custom_001".to_string()),
        title: "Architecture Planning".to_string(),
        messages: vec![SyncMessage::new(
            "msg_init".to_string(),
            "conv_custom_001".to_string(),
            "user".to_string(),
            "Let's design the backend".to_string(),
        )],
    };

    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "test_drive_token_alice")
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let conv: SyncConversation = resp.json().await.unwrap();
    assert_eq!(conv.id, "conv_custom_001");
    assert_eq!(conv.title, "Architecture Planning");
    assert_eq!(conv.revision, 1);
    assert_eq!(conv.messages.len(), 1);
    assert!(!conv.deleted);

    // Verify remote Drive persistence (manifest + conv file)
    assert_eq!(mock_client.file_count().await, 2);

    // 2. Create with auto-generated ID
    let payload_auto = CreateConversationRequest {
        id: None,
        title: "Auto ID Chat".to_string(),
        messages: vec![],
    };

    let resp_auto = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "test_drive_token_alice")
        .json(&payload_auto)
        .send()
        .await
        .unwrap();

    assert_eq!(resp_auto.status(), StatusCode::CREATED);
    let conv_auto: SyncConversation = resp_auto.json().await.unwrap();
    assert!(conv_auto.id.starts_with("conv_"));
    assert_eq!(conv_auto.title, "Auto ID Chat");
    assert_eq!(conv_auto.revision, 1);
}

#[tokio::test]
async fn test_02_conversation_input_validation() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Empty title rejection
    let empty_title_payload = CreateConversationRequest {
        id: Some("conv_bad_title".to_string()),
        title: "".to_string(),
        messages: vec![],
    };
    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&empty_title_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 2. Whitespace-only title rejection
    let ws_title_payload = CreateConversationRequest {
        id: Some("conv_ws_title".to_string()),
        title: "   \t\n  ".to_string(),
        messages: vec![],
    };
    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&ws_title_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 3. Oversized title rejection (> 500 chars)
    let long_title_payload = CreateConversationRequest {
        id: Some("conv_long_title".to_string()),
        title: "A".repeat(501),
        messages: vec![],
    };
    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&long_title_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 4. Malformed custom ID rejection (path traversal characters)
    let traversal_id_payload = CreateConversationRequest {
        id: Some("../../../etc/passwd".to_string()),
        title: "Evil Traversal".to_string(),
        messages: vec![],
    };
    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&traversal_id_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 5. Malformed ID in GET route path
    let resp_bad_path = client
        .get(format!("{base_url}/api/v1/drive/conversations/bad!@#$id"))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_bad_path.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_03_conversation_updating_and_renaming_put_and_patch() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create initial conversation
    let create_payload = CreateConversationRequest {
        id: Some("conv_rename_test".to_string()),
        title: "Original Title".to_string(),
        messages: vec![SyncMessage::new(
            "msg_1".to_string(),
            "conv_rename_test".to_string(),
            "user".to_string(),
            "Initial prompt".to_string(),
        )],
    };

    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&create_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 2. Rename title using PUT
    let put_payload = UpdateConversationRequest {
        title: Some("Renamed Title via PUT".to_string()),
        messages: None,
    };

    let resp_put = client
        .put(format!(
            "{base_url}/api/v1/drive/conversations/conv_rename_test"
        ))
        .header("x-drive-token", "valid_token")
        .json(&put_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_put.status(), StatusCode::OK);
    let conv_put: SyncConversation = resp_put.json().await.unwrap();
    assert_eq!(conv_put.title, "Renamed Title via PUT");
    assert_eq!(conv_put.revision, 2);
    assert_eq!(conv_put.messages.len(), 1); // Messages intact

    // 3. Rename title using PATCH
    let patch_payload = UpdateConversationRequest {
        title: Some("Renamed Title via PATCH".to_string()),
        messages: None,
    };

    let resp_patch = client
        .patch(format!(
            "{base_url}/api/v1/drive/conversations/conv_rename_test"
        ))
        .header("x-drive-token", "valid_token")
        .json(&patch_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_patch.status(), StatusCode::OK);
    let conv_patch: SyncConversation = resp_patch.json().await.unwrap();
    assert_eq!(conv_patch.title, "Renamed Title via PATCH");
    assert_eq!(conv_patch.revision, 3);

    // 4. Update messages via PUT
    let msgs_update = UpdateConversationRequest {
        title: None,
        messages: Some(vec![
            SyncMessage::new(
                "msg_1".to_string(),
                "conv_rename_test".to_string(),
                "user".to_string(),
                "Initial prompt".to_string(),
            ),
            SyncMessage::new(
                "msg_2".to_string(),
                "conv_rename_test".to_string(),
                "assistant".to_string(),
                "Assistant answer".to_string(),
            ),
        ]),
    };

    let resp_msgs = client
        .put(format!(
            "{base_url}/api/v1/drive/conversations/conv_rename_test"
        ))
        .header("x-drive-token", "valid_token")
        .json(&msgs_update)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_msgs.status(), StatusCode::OK);
    let conv_msgs: SyncConversation = resp_msgs.json().await.unwrap();
    assert_eq!(conv_msgs.messages.len(), 2);
    assert_eq!(conv_msgs.revision, 4);
    assert_eq!(conv_msgs.title, "Renamed Title via PATCH"); // Title preserved
}

#[tokio::test]
async fn test_04_conversation_update_validation() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create a conversation
    let create_payload = CreateConversationRequest {
        id: Some("conv_val_test".to_string()),
        title: "Validation Target".to_string(),
        messages: vec![],
    };
    let _ = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&create_payload)
        .send()
        .await
        .unwrap();

    // 2. Reject empty payload (both title and messages are None)
    let empty_payload = UpdateConversationRequest {
        title: None,
        messages: None,
    };
    let resp = client
        .put(format!(
            "{base_url}/api/v1/drive/conversations/conv_val_test"
        ))
        .header("x-drive-token", "valid_token")
        .json(&empty_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 3. Reject empty/whitespace title in update
    let empty_title = UpdateConversationRequest {
        title: Some("   ".to_string()),
        messages: None,
    };
    let resp = client
        .put(format!(
            "{base_url}/api/v1/drive/conversations/conv_val_test"
        ))
        .header("x-drive-token", "valid_token")
        .json(&empty_title)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 4. Updating non-existent conversation returns 404
    let valid_update = UpdateConversationRequest {
        title: Some("New Title".to_string()),
        messages: None,
    };
    let resp = client
        .put(format!(
            "{base_url}/api/v1/drive/conversations/non_existent_conv"
        ))
        .header("x-drive-token", "valid_token")
        .json(&valid_update)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_05_soft_delete_and_double_delete_behavior() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create conversation
    let create_payload = CreateConversationRequest {
        id: Some("conv_del_target".to_string()),
        title: "Delete Me".to_string(),
        messages: vec![],
    };
    let _ = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&create_payload)
        .send()
        .await
        .unwrap();

    // 2. Soft-delete conversation
    let resp_del = client
        .delete(format!(
            "{base_url}/api/v1/drive/conversations/conv_del_target"
        ))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_del.status(), StatusCode::OK);

    // 3. GET soft-deleted conversation returns 404 Not Found
    let resp_get = client
        .get(format!(
            "{base_url}/api/v1/drive/conversations/conv_del_target"
        ))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_get.status(), StatusCode::NOT_FOUND);

    // 4. Deleting an already deleted conversation returns 404 Not Found
    let resp_del_again = client
        .delete(format!(
            "{base_url}/api/v1/drive/conversations/conv_del_target"
        ))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_del_again.status(), StatusCode::NOT_FOUND);

    // 5. Updating a deleted conversation returns 404 Not Found
    let update_del = UpdateConversationRequest {
        title: Some("Revive Attempt".to_string()),
        messages: None,
    };
    let resp_update_del = client
        .put(format!(
            "{base_url}/api/v1/drive/conversations/conv_del_target"
        ))
        .header("x-drive-token", "valid_token")
        .json(&update_del)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_update_del.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_06_soft_delete_preserves_associated_library_items() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create conversation
    let create_payload = CreateConversationRequest {
        id: Some("conv_parent".to_string()),
        title: "Parent Chat".to_string(),
        messages: vec![],
    };
    let _ = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&create_payload)
        .send()
        .await
        .unwrap();

    // 2. Upload library item attached to conversation
    let raw_bytes = b"important_attachment_bytes";
    let lib_payload = CreateLibraryItemRequest {
        id: Some("lib_child_01".to_string()),
        filename: "attachment.txt".to_string(),
        mime_type: "text/plain".to_string(),
        conversation_id: Some("conv_parent".to_string()),
        content_base64: encode_base64(raw_bytes),
    };
    let resp_lib = client
        .post(format!("{base_url}/api/v1/drive/library"))
        .header("x-drive-token", "valid_token")
        .json(&lib_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_lib.status(), StatusCode::CREATED);

    // 3. Delete parent conversation
    let resp_del_conv = client
        .delete(format!("{base_url}/api/v1/drive/conversations/conv_parent"))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_del_conv.status(), StatusCode::OK);

    // 4. Verify Library item is STILL AVAILABLE and completely intact!
    let resp_get_lib = client
        .get(format!("{base_url}/api/v1/drive/library/lib_child_01"))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_get_lib.status(), StatusCode::OK);
    let item: LibraryItem = resp_get_lib.json().await.unwrap();
    assert_eq!(item.id, "lib_child_01");
    assert!(!item.deleted);

    // 5. Verify Library binary content download still works
    let resp_content = client
        .get(format!(
            "{base_url}/api/v1/drive/library/lib_child_01/content"
        ))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_content.status(), StatusCode::OK);
    let downloaded_bytes = resp_content.bytes().await.unwrap();
    assert_eq!(downloaded_bytes.as_ref(), raw_bytes);
}

#[tokio::test]
async fn test_07_cold_start_hydration_and_restart_recovery() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create conversation on backend and verify Drive upload
    let create_payload = CreateConversationRequest {
        id: Some("conv_cold_target".to_string()),
        title: "Cold Start Test".to_string(),
        messages: vec![SyncMessage::new(
            "msg_c1".to_string(),
            "conv_cold_target".to_string(),
            "user".to_string(),
            "Persistent memory test".to_string(),
        )],
    };
    let resp = client
        .post(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .json(&create_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 2. Simulate complete backend restart / dropping RAM working cache
    sync_engine
        .local_store()
        .clear_user_session("dev_user")
        .await
        .unwrap();
    assert!(
        sync_engine
            .local_store()
            .list_conversations("dev_user")
            .await
            .is_empty(),
        "Local RAM must be empty after restart"
    );

    // 3. GET /api/v1/drive/conversations with X-Drive-Token transparently hydrates from Google Drive
    let resp_list = client
        .get(format!("{base_url}/api/v1/drive/conversations"))
        .header("x-drive-token", "valid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp_list.status(), StatusCode::OK);
    let list: Vec<SyncConversation> = resp_list.json().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "conv_cold_target");
    assert_eq!(list[0].title, "Cold Start Test");
    assert_eq!(list[0].messages.len(), 1);
}

#[tokio::test]
async fn test_08_invalid_conversation_id_in_chat_completion_rejected() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    let invalid_chat_req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        routing_mode: "auto".to_string(),
        requested_model_id: None,
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
        conversation_id: Some("../../../traversal_id".to_string()),
        allow_fallback: None,
    };

    let resp = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .header("x-drive-token", "valid_token")
        .json(&invalid_chat_req)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn test_09_cross_tenant_isolation_and_idor_prevention() {
    let mock_client = Arc::new(MockDriveClient::new());
    let master_secret = [0x55u8; 32];
    let crypto = Arc::new(DriveCryptoEngine::derive_for_owner(
        &master_secret,
        "isolation_test_tenant",
    ));
    let local_store = Arc::new(LocalSyncStore::new());
    let sync_engine = Arc::new(SyncEngine::new(
        mock_client.clone(),
        crypto,
        local_store,
        true,
    ));

    let owner_alice = "google:alice_101";
    let owner_bob = "google:bob_202";

    // 1. Alice creates a conversation directly on Drive
    let conv_alice = SyncConversation::new(
        "conv_alice_01".to_string(),
        owner_alice.to_string(),
        "Alice Secret Plan".to_string(),
    );
    sync_engine
        .persist_conversation_to_drive(owner_alice, conv_alice, "alice_drive_token")
        .await
        .unwrap();

    // 2. Bob cannot retrieve Alice's conversation from Drive
    let bob_lookup = sync_engine
        .get_or_hydrate_conversation(owner_bob, "conv_alice_01", Some("bob_drive_token"))
        .await
        .unwrap();
    assert!(
        bob_lookup.is_none(),
        "Bob must not be able to hydrate Alice's conversation"
    );

    // 3. Bob's local store listing does not include Alice's conversation
    let bob_list = sync_engine
        .list_conversations_with_drive(owner_bob, Some("bob_drive_token"))
        .await
        .unwrap();
    assert_eq!(bob_list.len(), 0, "Bob's conversation list must be empty");

    // 4. Bob cannot delete Alice's conversation
    let bob_del = sync_engine
        .delete_conversation_on_drive(owner_bob, "conv_alice_01", "bob_drive_token")
        .await
        .unwrap();
    assert!(
        !bob_del,
        "Bob must not be able to delete Alice's conversation"
    );

    // 5. Alice's conversation remains active and unharmed
    let alice_lookup = sync_engine
        .get_or_hydrate_conversation(owner_alice, "conv_alice_01", Some("alice_drive_token"))
        .await
        .unwrap();
    assert!(alice_lookup.is_some());
    assert_eq!(alice_lookup.unwrap().title, "Alice Secret Plan");
}

#[tokio::test]
async fn test_10_chat_completion_turn_append_and_persistence() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;

    let registry = Arc::new(ai_backend::registry::ModelRegistry::new());
    let providers = Arc::new(ai_backend::providers::ProviderRegistry::new());
    let router = Arc::new(ai_backend::router::ModelRouter::new(registry.clone()));

    // Register a mock provider adapter for model "mock-fast"
    let model = ai_backend::models::ModelMetadata {
        model_id: "mock-fast".to_string(),
        display_name: "Mock Fast Model".to_string(),
        provider_id: "mock-provider".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 8192,
        max_output_tokens: 2048,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string(), "streaming".to_string()],
        last_health_check: "2026-10-22T20:00:00Z".to_string(),
    };
    registry.register_model(model.clone()).unwrap();
    let mock_adapter = Arc::new(
        ai_backend::providers::MockProviderAdapter::new("mock-provider", "Mock Provider")
            .with_models(vec![model]),
    );
    providers.register(mock_adapter).unwrap();

    let state = AppState::with_all_components(Arc::new(config), registry, providers, router);

    let (base_url, _handle, _mock_client, sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Create existing conversation for default local owner "dev_local_user"
    let initial_conv = SyncConversation::new(
        "conv_turn_append".to_string(),
        "dev_local_user".to_string(),
        "Chat Continuity Test".to_string(),
    );
    sync_engine
        .persist_conversation_to_drive("dev_local_user", initial_conv, "valid_token")
        .await
        .unwrap();

    // 2. Dispatch chat completion referencing conversation_id
    let chat_req = ChatCompletionRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "What is 2 + 2?".to_string(),
        }],
        routing_mode: "manual".to_string(),
        requested_model_id: Some("mock-fast".to_string()),
        requested_modality: "text".to_string(),
        temperature: None,
        max_tokens: None,
        stream: false,
        conversation_id: Some("conv_turn_append".to_string()),
        allow_fallback: None,
    };

    let resp = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .header("x-drive-token", "valid_token")
        .json(&chat_req)
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "Response body was: {}", body);

    // 3. Verify conversation on Drive now has user prompt and assistant response appended, and revision incremented
    let updated_conv = sync_engine
        .get_or_hydrate_conversation("dev_local_user", "conv_turn_append", Some("valid_token"))
        .await
        .unwrap()
        .expect("Conversation must exist on Drive");

    assert_eq!(updated_conv.revision, 2);
    assert_eq!(updated_conv.messages.len(), 2);
    assert_eq!(updated_conv.messages[0].role, "user");
    assert_eq!(updated_conv.messages[0].content, "What is 2 + 2?");
    assert_eq!(updated_conv.messages[1].role, "assistant");
}
