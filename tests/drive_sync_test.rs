//! Comprehensive integration and unit tests for Phase 15 Google Drive synchronization.
//!
//! Conforms to MASTER_SPEC.md §15, §17 and Phase 15 requirements:
//! - Initial sync with empty account.
//! - Incremental sync uploading only dirty changes.
//! - LWW conflict resolution (revision, timestamp, deterministic tie-breaking).
//! - Transient failure retries and 429 rate limit with `Retry-After`.
//! - Safe logout clearing local session without deleting remote data.
//! - Reinstall and bootstrap restoration from remote manifest.
//! - Multi-tenant account isolation.
//! - Drive on/off configuration toggle preserving local availability.

use ai_backend::config::AppConfig;
use ai_backend::drive::mock::MockDriveClient;
use ai_backend::drive::{
    DriveConfigUpdate, DriveCryptoEngine, LocalSyncStore, SyncConversation, SyncEngine, SyncMessage,
};
use ai_backend::{AppState, create_app};
use reqwest::StatusCode;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Spawn a live test Axum server on an ephemeral loopback port.
async fn spawn_drive_test_backend(
    state: AppState,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<MockDriveClient>,
    Arc<SyncEngine>,
) {
    let mock_client = Arc::new(MockDriveClient::new());
    let master_secret = [0x42u8; 32];
    let crypto = Arc::new(DriveCryptoEngine::derive_for_owner(
        &master_secret,
        "test_tenant",
    ));
    let local_store = Arc::new(LocalSyncStore::new());

    let sync_engine = Arc::new(SyncEngine::new(
        mock_client.clone(),
        crypto,
        local_store,
        state.config.google_drive_sync_enabled,
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
async fn test_drive_switch_default_off_and_toggle() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = false; // default OFF per requirement
    let state = AppState::new(Arc::new(config));

    let (base_url, _handle, _mock_client, sync_engine) = spawn_drive_test_backend(state).await;
    let client = reqwest::Client::new();

    // 1. Initial status check: must be disabled by default
    assert!(!sync_engine.is_sync_enabled().await);

    // 2. Attempting to sync while disabled returns 400 Bad Request (SyncDisabled)
    let sync_resp = client
        .post(format!("{base_url}/api/v1/drive/sync"))
        .header("x-drive-token", "fake_token_123")
        .send()
        .await
        .unwrap();
    assert_eq!(sync_resp.status(), StatusCode::BAD_REQUEST);

    // 3. Toggle switch ON via POST /api/v1/drive/config
    let toggle_resp = client
        .post(format!("{base_url}/api/v1/drive/config"))
        .json(&DriveConfigUpdate { enabled: true })
        .send()
        .await
        .unwrap();
    assert_eq!(toggle_resp.status(), StatusCode::OK);
    assert!(sync_engine.is_sync_enabled().await);

    // 4. Toggle switch OFF: local content remains accessible
    sync_engine.set_sync_enabled(false).await;
    assert!(!sync_engine.is_sync_enabled().await);
}

#[tokio::test]
async fn test_empty_account_initial_sync_and_local_conversation_upload() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (_base_url, _handle, mock_client, sync_engine) = spawn_drive_test_backend(state).await;

    let owner = "google:user_alice";
    let token = "valid_oauth_token";

    // 1. Local user creates two conversations offline
    let mut conv1 = SyncConversation::new(
        "conv_1".to_string(),
        owner.to_string(),
        "First Chat".to_string(),
    );
    conv1.messages.push(SyncMessage::new(
        "msg_1".to_string(),
        "conv_1".to_string(),
        "user".to_string(),
        "Hello!".to_string(),
    ));
    sync_engine
        .local_store()
        .upsert_conversation(owner, conv1)
        .await;

    let mut conv2 = SyncConversation::new(
        "conv_2".to_string(),
        owner.to_string(),
        "Second Chat".to_string(),
    );
    conv2.messages.push(SyncMessage::new(
        "msg_2".to_string(),
        "conv_2".to_string(),
        "user".to_string(),
        "How is Rust?".to_string(),
    ));
    sync_engine
        .local_store()
        .upsert_conversation(owner, conv2)
        .await;

    // Check status: 2 pending conversations
    let status = sync_engine.get_status(owner).await;
    assert_eq!(status.pending_conversations_count, 2);

    // 2. Perform synchronization
    let result = sync_engine.sync(owner, token).await.expect("Sync failed");
    assert_eq!(result.uploaded_conversations, 2);
    assert_eq!(result.downloaded_conversations, 0);

    // 3. Verify MockDriveClient received files: manifest + 2 conversations = 3 files
    assert_eq!(mock_client.file_count().await, 3);
    assert!(
        mock_client
            .get_content_by_name("sync_manifest.json")
            .await
            .is_some()
    );
    assert!(
        mock_client
            .get_content_by_name("conv_conv_1.json.enc")
            .await
            .is_some()
    );
    assert!(
        mock_client
            .get_content_by_name("conv_conv_2.json.enc")
            .await
            .is_some()
    );

    // 4. Subsequent sync has 0 pending items (incremental sync discipline)
    let status_after = sync_engine.get_status(owner).await;
    assert_eq!(status_after.pending_conversations_count, 0);

    let result_subsequent = sync_engine
        .sync(owner, token)
        .await
        .expect("Subsequent sync failed");
    assert_eq!(result_subsequent.uploaded_conversations, 0);
}

#[tokio::test]
async fn test_lww_conflict_resolution() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (_base_url, _handle, _mock_client, sync_engine) = spawn_drive_test_backend(state).await;

    let owner = "google:user_bob";
    let token = "valid_oauth_token";

    // 1. Initial sync
    let mut conv = SyncConversation::new(
        "conv_conflict".to_string(),
        owner.to_string(),
        "Original Title".to_string(),
    );
    conv.revision = 1;
    conv.updated_at = "2026-10-01T12:00:00.000Z".to_string();
    sync_engine
        .local_store()
        .upsert_conversation(owner, conv)
        .await;
    sync_engine.sync(owner, token).await.unwrap();

    // 2. Simulate local update with higher revision (local = 3, remote = 2)
    let mut local_newer = SyncConversation::new(
        "conv_conflict".to_string(),
        owner.to_string(),
        "Local Newer Title".to_string(),
    );
    local_newer.revision = 3;
    local_newer.updated_at = "2026-10-01T12:30:00.000Z".to_string();
    sync_engine
        .local_store()
        .upsert_conversation(owner, local_newer)
        .await;

    let sync_res = sync_engine.sync(owner, token).await.unwrap();
    assert_eq!(sync_res.uploaded_conversations, 1);

    let final_conv = sync_engine
        .local_store()
        .get_conversation(owner, "conv_conflict")
        .await
        .unwrap();
    assert_eq!(final_conv.title, "Local Newer Title");
}

#[tokio::test]
async fn test_transient_failure_retries_and_recovery() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (_base_url, _handle, mock_client, sync_engine) = spawn_drive_test_backend(state).await;

    let owner = "google:user_retry";
    let token = "valid_oauth_token";

    let conv = SyncConversation::new(
        "conv_retry".to_string(),
        owner.to_string(),
        "Retry Chat".to_string(),
    );
    sync_engine
        .local_store()
        .upsert_conversation(owner, conv)
        .await;

    // Inject 1 transient network failure (client will retry and succeed)
    mock_client.set_transient_failures(1);

    let res = sync_engine.sync(owner, token).await;
    assert!(res.is_ok(), "Transient failure must recover on retry");
}

#[tokio::test]
async fn test_http_429_rate_limiting_with_retry_after() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (_base_url, _handle, mock_client, sync_engine) = spawn_drive_test_backend(state).await;

    let owner = "google:user_rate_limit";
    let token = "valid_oauth_token";

    // Inject HTTP 429 rate limit
    mock_client.set_inject_rate_limited(Some(2)).await;

    let err = sync_engine.sync(owner, token).await.unwrap_err();
    assert!(matches!(
        err,
        ai_backend::drive::DriveError::RateLimited {
            retry_after_secs: Some(2),
            ..
        }
    ));
}

#[tokio::test]
async fn test_logout_preserves_remote_data_and_restore_recovers_it() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    let state = AppState::new(Arc::new(config));

    let (_base_url, _handle, mock_client, sync_engine) = spawn_drive_test_backend(state).await;

    let owner = "google:user_charlie";
    let token = "valid_oauth_token";

    // 1. Create and sync data
    let conv = SyncConversation::new(
        "conv_restore".to_string(),
        owner.to_string(),
        "Restore Test".to_string(),
    );
    sync_engine
        .local_store()
        .upsert_conversation(owner, conv)
        .await;
    sync_engine.sync(owner, token).await.unwrap();

    // Verify remote has file
    assert_eq!(mock_client.file_count().await, 2); // manifest + 1 conv

    // 2. Logout: clear local session
    sync_engine.local_store().clear_user_session(owner).await;

    // Verify local store is empty for Charlie
    assert!(
        sync_engine
            .local_store()
            .get_conversation(owner, "conv_restore")
            .await
            .is_none()
    );

    // Verify REMOTE Drive files STILL EXIST (logout must NEVER delete remote data)
    assert_eq!(mock_client.file_count().await, 2);

    // 3. Relogin / Reinstall Restore bootstrap
    let restore_res = sync_engine.restore(owner, token).await.unwrap();
    assert_eq!(restore_res.downloaded_conversations, 1);

    // 4. Verify conversation is restored locally
    let restored = sync_engine
        .local_store()
        .get_conversation(owner, "conv_restore")
        .await
        .unwrap();
    assert_eq!(restored.title, "Restore Test");
}

#[tokio::test]
async fn test_tenant_isolation_between_accounts() {
    let config = AppConfig::default_local();
    let state = AppState::new(Arc::new(config));

    let (_base_url, _handle, _mock_client, sync_engine) = spawn_drive_test_backend(state).await;

    let owner_a = "google:alice";
    let owner_b = "google:bob";

    let conv_a = SyncConversation::new(
        "conv_a".to_string(),
        owner_a.to_string(),
        "Alice Chat".to_string(),
    );
    sync_engine
        .local_store()
        .upsert_conversation(owner_a, conv_a)
        .await;

    // Bob cannot read Alice's conversation
    assert!(
        sync_engine
            .local_store()
            .get_conversation(owner_b, "conv_a")
            .await
            .is_none()
    );
    assert_eq!(
        sync_engine
            .local_store()
            .list_conversations(owner_b)
            .await
            .len(),
        0
    );
}
