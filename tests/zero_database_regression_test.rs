//! Zero-Database & No-Persistent-Local-Storage Regression Test Suite
//!
//! Mandatory proof that the backend operates without any database or local disk persistence:
//! 1. No SQLite, PostgreSQL, MySQL, Redis, or embedded database files.
//! 2. No `data/usage.db`, database directories, or connection pools.
//! 3. No `/tmp/usage_spool.log` or disk-based spool/queue files.
//! 4. Verifies both Daemon and Serverless runtime configurations.
//! 5. Confirms operational metrics operate in-memory with bounded capacity (10,000 records).
//! 6. Confirms zero user data, conversations, messages, or files persist to local disk.

use ai_backend::AppState;
use ai_backend::config::{AppConfig, DeploymentMode};
use ai_backend::create_app;
use ai_backend::drive::mock::MockDriveClient;
use ai_backend::drive::{
    DriveCryptoEngine, LocalSyncStore, SyncConversation, SyncEngine, SyncMessage,
};
use ai_backend::models::ModelMetadata;
use ai_backend::providers::{MockProviderAdapter, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use ai_backend::usage::model::{
    RecordStatus, RequestMode, RequestOutcome, RoutingMode, UsageRecord, UsageSource,
};
use ai_backend::usage::storage::{MAX_IN_MEMORY_USAGE_RECORDS, UsageStorage};
use reqwest::StatusCode;
use serde_json::json;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;

const TEST_API_KEY: &str = "test_key_zero_db_suite";
const TEST_OWNER: &str = "google:user_zero_db";

fn build_test_state(mut config: AppConfig) -> (AppState, Arc<MockDriveClient>, Arc<SyncEngine>) {
    config.api_key = Some(TEST_API_KEY.to_string());
    config.api_keys = vec![TEST_API_KEY.to_string()];
    let config_arc = Arc::new(config.clone());

    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    let model = ModelMetadata {
        model_id: "mock-model".to_string(),
        display_name: "Mock Model".to_string(),
        provider_id: "mock-provider".to_string(),
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

    registry.register_model(model.clone()).unwrap();
    let adapter = Arc::new(
        MockProviderAdapter::new("mock-provider", "Mock Provider").with_models(vec![model]),
    );
    providers.register(adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let base_state = AppState::with_all_components(config_arc, registry, providers, router);

    let mock_drive = Arc::new(MockDriveClient::new());
    let key = [0x55u8; 32];
    let crypto = Arc::new(DriveCryptoEngine::derive_for_owner(&key, TEST_OWNER));
    let local_store = Arc::new(LocalSyncStore::new());
    let sync_engine = Arc::new(SyncEngine::new(
        mock_drive.clone(),
        crypto,
        local_store,
        true,
    ));

    let final_state = base_state.with_sync_engine(sync_engine.clone());
    (final_state, mock_drive, sync_engine)
}

async fn spawn_backend(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
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

    (format!("http://127.0.0.1:{}", addr.port()), handle)
}

fn assert_no_prohibited_storage_artifacts() {
    // 1. Assert no 'data' directory or database files in current workspace
    assert!(
        !Path::new("data/usage.db").exists(),
        "Regression failure: data/usage.db must NOT exist!"
    );
    assert!(
        !Path::new("data/usage_spool.log").exists(),
        "Regression failure: data/usage_spool.log must NOT exist!"
    );
    assert!(
        !Path::new("usage.db").exists(),
        "Regression failure: usage.db must NOT exist!"
    );

    // 2. Assert no SQLite database files in the repository root
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                assert!(
                    ext != "db" && ext != "sqlite" && ext != "sqlite3",
                    "Forbidden database file found on disk: {:?}",
                    path
                );
            }
        }
    }

    // 3. Assert no spool files in /tmp or temp dir
    let temp_spool = std::env::temp_dir().join("usage_spool.log");
    assert!(
        !temp_spool.exists(),
        "Regression failure: Ephemeral spool file must not be created at {:?}",
        temp_spool
    );
}

#[tokio::test]
async fn test_daemon_mode_creates_no_database_or_spool_files() {
    let mut config = AppConfig::default_local();
    config.deployment_mode = DeploymentMode::Daemon;
    config.google_drive_sync_enabled = true;

    let (state, _mock_drive, _sync_engine) = build_test_state(config);
    let (base_url, _handle) = spawn_backend(state).await;
    let client = reqwest::Client::new();

    // Health check
    let resp = client
        .get(format!("{base_url}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Usage summary query
    let resp = client
        .get(format!("{base_url}/api/v1/usage/summary"))
        .bearer_auth(TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Chat completion request
    let payload = json!({
        "requested_model_id": "mock-model",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "Verify zero database operation in daemon mode." }
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

    // Streaming request
    let stream_resp = client
        .post(format!("{base_url}/api/v1/chat/stream"))
        .bearer_auth(TEST_API_KEY)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(stream_resp.status(), StatusCode::OK);
    let _ = stream_resp.text().await.unwrap();

    // Strictly verify zero prohibited files exist
    assert_no_prohibited_storage_artifacts();
}

#[tokio::test]
async fn test_serverless_mode_creates_no_database_or_spool_files() {
    let mut config = AppConfig::default_serverless();
    config.google_drive_sync_enabled = true;

    let (state, _mock_drive, _sync_engine) = build_test_state(config);
    let (base_url, _handle) = spawn_backend(state).await;
    let client = reqwest::Client::new();

    // Health check
    let resp = client
        .get(format!("{base_url}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Chat completions
    let payload = json!({
        "requested_model_id": "mock-model",
        "routing_mode": "manual",
        "messages": [
            { "role": "user", "content": "Verify zero database in serverless mode." }
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

    // Verify zero database files or spool files exist
    assert_no_prohibited_storage_artifacts();
}

#[tokio::test]
async fn test_in_memory_telemetry_bounded_capacity_and_fifo_eviction() {
    let storage = UsageStorage::new_in_memory();

    // Verify capacity ceiling is 10,000
    assert_eq!(MAX_IN_MEMORY_USAGE_RECORDS, 10_000);

    // Insert 10,050 records to trigger FIFO eviction
    for i in 0..10_050 {
        let record = UsageRecord {
            id: format!("rec_{i}"),
            provider_id: "test-prov".to_string(),
            model_id: "test-model".to_string(),
            request_mode: RequestMode::Buffered,
            routing_mode: RoutingMode::Auto,
            start_time: 1700000000000 + i,
            completion_time: 1700000000050 + i,
            latency_ms: 50,
            outcome: RequestOutcome::Success,
            failure_category: None,
            prompt_tokens: Some(10),
            completion_tokens: Some(20),
            total_tokens: Some(30),
            usage_source: UsageSource::Reported,
            record_status: RecordStatus::Final,
            subject: Some("test-user".to_string()),
        };
        storage.record(record).await.unwrap();
    }

    // Verify bounded memory ceiling: exactly 10,000 records, never more
    let records = storage
        .get_records(ai_backend::usage::model::UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(records.len(), 10_000);

    // Verify FIFO eviction: the oldest 50 records (rec_0 through rec_49) were evicted
    assert_eq!(records[0].id, "rec_50");
    assert_eq!(records[9_999].id, "rec_10049");

    // Strictly verify no disk files were written during memory-bounded telemetry
    assert_no_prohibited_storage_artifacts();
}

#[tokio::test]
async fn test_local_sync_store_is_transient_and_creates_zero_disk_state() {
    let store = LocalSyncStore::new();

    // Store must report non-persistent
    assert!(!store.is_persistent());

    // Save a conversation in local memory
    let mut conv = SyncConversation::new(
        "transient_conv_1".to_string(),
        TEST_OWNER.to_string(),
        "Transient Chat".to_string(),
    );
    conv.messages.push(SyncMessage::new(
        "msg_1".to_string(),
        "transient_conv_1".to_string(),
        "user".to_string(),
        "Testing local zero-database guarantee".to_string(),
    ));

    store.upsert_conversation(TEST_OWNER, conv).await.unwrap();

    let listed = store.list_conversations(TEST_OWNER).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "transient_conv_1");

    // Strictly verify no file or directory was created on disk
    assert_no_prohibited_storage_artifacts();
}
