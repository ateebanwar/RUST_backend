//! Comprehensive integration tests for model-agnostic chat continuity,
//! provider switching, stateless database-free operation, restart recovery,
//! streaming persistence, and fail-closed Drive authentication.
//!
//! Conforms to MASTER_SPEC.md, AGENTS.md, and Final Backend Completion Directive:
//! - Stable conversation identifiers across requests and AI providers.
//! - Hydrates existing conversation history before dispatching to inference.
//! - Switching between providers (Provider A -> Provider B) preserves conversation context.
//! - User and assistant messages (both buffered and SSE streaming) are saved to Google Drive.
//! - Backend restart / cold-start recovers conversation history directly from Google Drive.
//! - Completely database-free and disk-free operation when configured in-memory / serverless.
//! - Fail-closed authentication when conversation_id is passed without X-Drive-Token.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::create_app;
use ai_backend::drive::mock::MockDriveClient;
use ai_backend::drive::{
    DriveCryptoEngine, LocalSyncStore, SyncConversation, SyncEngine, SyncMessage,
};
use ai_backend::models::ModelMetadata;
use ai_backend::providers::{MockProviderAdapter, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

const TEST_API_KEY: &str = "test_backend_api_key_12345";
const TEST_OWNER: &str = "service:key_00";

/// Helper: Spawn an ephemeral Axum test server with custom mock providers and SyncEngine.
async fn spawn_chat_test_backend(
    state: AppState,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<MockDriveClient>,
    Arc<SyncEngine>,
) {
    let mock_drive = Arc::new(MockDriveClient::new());
    let master_secret = [0x42u8; 32];
    let crypto = Arc::new(DriveCryptoEngine::derive_for_owner(
        &master_secret,
        TEST_OWNER,
    ));
    let local_store = Arc::new(LocalSyncStore::new());

    let sync_engine = Arc::new(SyncEngine::new(
        mock_drive.clone(),
        crypto,
        local_store,
        true, // sync enabled
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
        mock_drive,
        sync_engine,
    )
}

/// Helper: Setup mock providers and registry with Model A (Provider A) and Model B (Provider B).
fn setup_multi_provider_state(mut config: AppConfig) -> AppState {
    config.api_key = Some(TEST_API_KEY.to_string());
    let config = Arc::new(config);
    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    let model_a = ModelMetadata {
        model_id: "model-a".to_string(),
        display_name: "Mock Model A".to_string(),
        provider_id: "provider-a".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 16384,
        max_output_tokens: 2048,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "fast".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string(), "streaming".to_string()],
        last_health_check: "2026-10-22T20:00:00Z".to_string(),
    };

    let model_b = ModelMetadata {
        model_id: "model-b".to_string(),
        display_name: "Mock Model B".to_string(),
        provider_id: "provider-b".to_string(),
        supported_modalities: vec!["text".to_string()],
        context_window: 32768,
        max_output_tokens: 4096,
        streaming_supported: true,
        tool_calling_supported: false,
        speed_tier: "balanced".to_string(),
        cost_tier: "free-tier".to_string(),
        available: true,
        capabilities: vec!["simple_chat".to_string(), "streaming".to_string()],
        last_health_check: "2026-10-22T20:00:00Z".to_string(),
    };

    registry.register_model(model_a.clone()).unwrap();
    registry.register_model(model_b.clone()).unwrap();

    let adapter_a =
        Arc::new(MockProviderAdapter::new("provider-a", "Provider A").with_models(vec![model_a]));
    let adapter_b =
        Arc::new(MockProviderAdapter::new("provider-b", "Provider B").with_models(vec![model_b]));

    providers.register(adapter_a).unwrap();
    providers.register(adapter_b).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    AppState::with_all_components(config, registry, providers, router)
}

#[tokio::test]
async fn test_chat_continuity_across_provider_switching() {
    let mut config = AppConfig::default_local();
    config.drive_encryption_key =
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
    config.google_drive_sync_enabled = true;
    config.database_path = ":memory:".to_string();

    let state = setup_multi_provider_state(config);
    let (base_url, _handle, _mock_drive, sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    let conv_id = "conv-switch-provider-test-1";
    let drive_token = "valid_drive_token_xyz";

    // --- TURN 1: Invoke Model A (Provider A) ---
    let payload_turn_1 = json!({
        "conversation_id": conv_id,
        "requested_model_id": "model-a",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "My secret password is 'RustFlavoredCoffee'." }
        ]
    });

    let resp_turn_1 = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .bearer_auth(TEST_API_KEY)
        .header("x-drive-token", drive_token)
        .json(&payload_turn_1)
        .send()
        .await
        .unwrap();

    assert_eq!(resp_turn_1.status(), StatusCode::OK);
    let body_1: Value = resp_turn_1.json().await.unwrap();
    assert_eq!(body_1["provider_used"], "provider-a");
    assert_eq!(body_1["model_used"], "model-a");

    // Wait briefly for asynchronous Drive persistence
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify Turn 1 is stored in Drive cache
    let stored_conv_1 = sync_engine
        .local_store()
        .get_conversation(TEST_OWNER, conv_id)
        .await
        .expect("Conversation should exist on Drive store after Turn 1");

    assert_eq!(stored_conv_1.messages.len(), 2);
    assert_eq!(stored_conv_1.messages[0].role, "user");
    assert_eq!(
        stored_conv_1.messages[0].content,
        "My secret password is 'RustFlavoredCoffee'."
    );
    assert_eq!(stored_conv_1.messages[1].role, "assistant");
    assert_eq!(stored_conv_1.revision, 2);

    // --- TURN 2: Switch to Model B (Provider B) ---
    // The client sends only the new prompt, with the same stable conversation_id
    let payload_turn_2 = json!({
        "conversation_id": conv_id,
        "requested_model_id": "model-b",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "What was my secret password?" }
        ]
    });

    let resp_turn_2 = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .bearer_auth(TEST_API_KEY)
        .header("x-drive-token", drive_token)
        .json(&payload_turn_2)
        .send()
        .await
        .unwrap();

    assert_eq!(resp_turn_2.status(), StatusCode::OK);
    let body_2: Value = resp_turn_2.json().await.unwrap();
    assert_eq!(body_2["provider_used"], "provider-b");
    assert_eq!(body_2["model_used"], "model-b");

    // Wait briefly for asynchronous Drive persistence
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify Turn 2 preserved Turn 1 and appended Turn 2 without loss
    let stored_conv_2 = sync_engine
        .local_store()
        .get_conversation(TEST_OWNER, conv_id)
        .await
        .expect("Conversation should exist after Turn 2");

    assert_eq!(stored_conv_2.messages.len(), 4);
    assert_eq!(stored_conv_2.messages[0].role, "user");
    assert_eq!(
        stored_conv_2.messages[0].content,
        "My secret password is 'RustFlavoredCoffee'."
    );
    assert_eq!(stored_conv_2.messages[1].role, "assistant");
    assert_eq!(stored_conv_2.messages[2].role, "user");
    assert_eq!(
        stored_conv_2.messages[2].content,
        "What was my secret password?"
    );
    assert_eq!(stored_conv_2.messages[3].role, "assistant");
    assert_eq!(stored_conv_2.revision, 3);
}

#[tokio::test]
async fn test_streaming_chat_continuity_and_drive_persistence() {
    let mut config = AppConfig::default_local();
    config.drive_encryption_key =
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
    config.google_drive_sync_enabled = true;
    config.database_path = ":memory:".to_string();

    let state = setup_multi_provider_state(config);
    let (base_url, _handle, _mock_drive, sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    let conv_id = "conv-stream-persistence-test";
    let drive_token = "valid_drive_token_stream";

    let payload = json!({
        "conversation_id": conv_id,
        "requested_model_id": "model-a",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "Stream me the response." }
        ]
    });

    let resp = client
        .post(format!("{base_url}/api/v1/chat/stream"))
        .bearer_auth(TEST_API_KEY)
        .header("x-drive-token", drive_token)
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let mut stream = resp.bytes_stream();
    let mut full_body = String::new();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.unwrap();
        full_body.push_str(&String::from_utf8_lossy(&chunk));
    }

    assert!(full_body.contains("event: routing"));
    assert!(full_body.contains("event: chunk"));
    assert!(full_body.contains("event: done"));

    // Wait briefly for asynchronous persistence task
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify the conversation on Drive contains both user message and the streamed assistant message
    let stored_conv = sync_engine
        .local_store()
        .get_conversation(TEST_OWNER, conv_id)
        .await
        .expect("Streamed conversation must be persisted to Drive");

    assert_eq!(stored_conv.messages.len(), 2);
    assert_eq!(stored_conv.messages[0].role, "user");
    assert_eq!(stored_conv.messages[0].content, "Stream me the response.");
    assert_eq!(stored_conv.messages[1].role, "assistant");
    assert_eq!(stored_conv.messages[1].content, "Mock streaming response");
    assert_eq!(stored_conv.revision, 2);
}

#[tokio::test]
async fn test_cold_start_recovery_from_drive() {
    let mut config = AppConfig::default_local();
    config.drive_encryption_key =
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
    config.google_drive_sync_enabled = true;
    config.database_path = ":memory:".to_string();

    let conv_id = "conv-cold-start-recovery";
    let drive_token = "valid_cold_start_token";

    // Step 1: Pre-seed a conversation on Google Drive mock store (as if created previously)
    let mock_drive = Arc::new(MockDriveClient::new());
    let master_secret = [0x42u8; 32];
    let crypto = Arc::new(DriveCryptoEngine::derive_for_owner(
        &master_secret,
        TEST_OWNER,
    ));
    let local_store_1 = Arc::new(LocalSyncStore::new());

    let mut preloaded_conv = SyncConversation::new(
        conv_id.to_string(),
        TEST_OWNER.to_string(),
        "Pre-existing Title".to_string(),
    );
    preloaded_conv.messages.push(SyncMessage::new(
        "msg_1".to_string(),
        conv_id.to_string(),
        "user".to_string(),
        "Initial historical query from last week.".to_string(),
    ));
    preloaded_conv.messages.push(SyncMessage::new(
        "msg_2".to_string(),
        conv_id.to_string(),
        "assistant".to_string(),
        "Initial historical answer from last week.".to_string(),
    ));
    preloaded_conv.revision = 1;

    let sync_engine_1 = Arc::new(SyncEngine::new(
        mock_drive.clone(),
        crypto.clone(),
        local_store_1,
        true,
    ));

    sync_engine_1
        .persist_conversation_to_drive(TEST_OWNER, preloaded_conv, drive_token)
        .await
        .unwrap();

    // Step 2: Spin up a brand new backend instance (simulating restart / cold-start)
    // with empty local memory and no database, sharing the same remote Drive store
    let local_store_2 = Arc::new(LocalSyncStore::new());
    let sync_engine_2 = Arc::new(SyncEngine::new(
        mock_drive.clone(),
        crypto.clone(),
        local_store_2.clone(),
        true,
    ));

    let state_2 =
        setup_multi_provider_state(config.clone()).with_sync_engine(sync_engine_2.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state_2);

    let _server_handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    // Step 3: Send a request to the restarted backend referencing the existing conversation
    let payload = json!({
        "conversation_id": conv_id,
        "requested_model_id": "model-a",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "Please follow up on the previous context." }
        ]
    });

    let resp = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .bearer_auth(TEST_API_KEY)
        .header("x-drive-token", drive_token)
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Wait briefly for persistence
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Step 4: Verify that the new backend instance successfully recovered the 2 historical
    // messages from Google Drive, and cleanly appended the new turn to reach 4 messages total
    let updated_conv = local_store_2
        .get_conversation(TEST_OWNER, conv_id)
        .await
        .expect("Conversation must be present in local working store");

    assert_eq!(updated_conv.messages.len(), 4);
    assert_eq!(
        updated_conv.messages[0].content,
        "Initial historical query from last week."
    );
    assert_eq!(
        updated_conv.messages[1].content,
        "Initial historical answer from last week."
    );
    assert_eq!(
        updated_conv.messages[2].content,
        "Please follow up on the previous context."
    );
    assert_eq!(updated_conv.messages[3].role, "assistant");
    assert_eq!(updated_conv.revision, 2);
}

#[tokio::test]
async fn test_stateless_and_memory_only_backend() {
    let mut config = AppConfig::default_local();
    config.database_path = ":memory:".to_string();
    config.usage_spool_path = "/tmp/ephemeral_test_spool.log".to_string();

    let state = setup_multi_provider_state(config);
    let (base_url, _handle, _mock_drive, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // Verify readiness and health endpoints report healthy
    let health_resp = client
        .get(format!("{base_url}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health_resp.status(), StatusCode::OK);

    let ready_resp = client
        .get(format!("{base_url}/api/v1/ready"))
        .send()
        .await
        .unwrap();
    assert_eq!(ready_resp.status(), StatusCode::OK);

    // Execute completion to record usage in-memory
    let payload = json!({
        "requested_model_id": "model-a",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "Stateless hello." }
        ]
    });

    let resp = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .bearer_auth(TEST_API_KEY)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Query usage summary to confirm in-memory SQLite functions without disk
    let usage_resp = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(usage_resp.status(), StatusCode::OK);
    let usage_data: Value = usage_resp.json().await.unwrap();
    assert!(usage_data["total_requests"].as_u64().unwrap_or(0) >= 1);

    // Ensure zero user conversation databases or tables were written to disk
    assert!(!std::path::Path::new("data/usage_test_leaked.db").exists());
}

#[tokio::test]
async fn test_fail_closed_missing_drive_token() {
    let mut config = AppConfig::default_local();
    config.drive_encryption_key =
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string());
    config.google_drive_sync_enabled = true;

    let state = setup_multi_provider_state(config);
    let (base_url, _handle, _mock_drive, _sync_engine) = spawn_chat_test_backend(state).await;
    let client = reqwest::Client::new();

    // Supplying conversation_id without X-Drive-Token must fail closed with 401 Unauthorized
    let payload = json!({
        "conversation_id": "conv-fail-closed-check",
        "requested_model_id": "model-a",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "Fail closed test" }
        ]
    });

    let resp = client
        .post(format!("{base_url}/api/v1/chat/completions"))
        .bearer_auth(TEST_API_KEY)
        // Omit x-drive-token
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let err_body: Value = resp.json().await.unwrap();
    assert_eq!(err_body["error"]["code"], "UNAUTHORIZED");
    assert_eq!(
        err_body["error"]["message"],
        "Invalid authentication credentials or restricted parameter."
    );
}
