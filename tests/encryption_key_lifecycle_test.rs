//! Comprehensive Security & Lifecycle Tests for Encryption Key Management (Phase 15).
//!
//! Verifies:
//! 1. Correctly configured 32-byte key is accepted.
//! 2. Valid key can encrypt and decrypt data.
//! 3. Existing-format ciphertext remains decryptable with the original valid key.
//! 4. Missing key is rejected.
//! 5. Empty key is rejected.
//! 6. Invalid encoding is rejected.
//! 7. Incorrect key length is rejected.
//! 8. Wrong key cannot silently decrypt existing ciphertext.
//! 9. Modified ciphertext fails authentication.
//! 10. Modified authentication tag fails authentication.
//! 11. Restarting with the same key preserves decryption.
//! 12. Restarting with a different key does not silently replace the original key.
//! 13. No automatic fallback key is generated when configuration is invalid.
//! 14. Failure does not overwrite or delete existing encrypted data.
//! 15. Error messages and logs do not disclose the key.
//! 16. Test-only configuration cannot bypass production key validation.

use ai_backend::config::{AppConfig, Environment};
use ai_backend::drive::crypto::{CryptoError, DriveCryptoEngine};
use ai_backend::drive::mock::MockDriveClient;
use ai_backend::drive::{LocalSyncStore, SyncConversation, SyncEngine};
use std::sync::Arc;

const VALID_KEY_HEX_1: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const VALID_KEY_HEX_2: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const OBSOLETE_FALLBACK_KEY_HEX: &str =
    "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";

#[test]
fn test_01_correctly_configured_32_byte_key_is_accepted() {
    let engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1)
        .expect("Valid 64-character hex key must be accepted");
    assert!(engine.is_configured(), "Engine must be marked configured");
    assert!(
        engine.key_fingerprint().is_some(),
        "Fingerprint must be present for configured engine"
    );
}

#[test]
fn test_02_valid_key_can_encrypt_and_decrypt_data() {
    let engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
    let owner = "google:alice_123";
    let resource_id = "conversation_456";
    let original_plaintext = b"Sensitive AI conversation contents that must be encrypted.";

    let ciphertext = engine
        .encrypt(owner, resource_id, original_plaintext)
        .expect("Encryption must succeed with valid key");

    assert_ne!(ciphertext, original_plaintext);
    // Nonce (12) + Tag (16) = 28 bytes overhead
    assert_eq!(ciphertext.len(), original_plaintext.len() + 28);

    let decrypted = engine
        .decrypt(owner, resource_id, &ciphertext)
        .expect("Decryption must succeed with same key and AAD");
    assert_eq!(decrypted, original_plaintext);
}

#[test]
fn test_03_existing_format_ciphertext_remains_decryptable_with_original_valid_key() {
    // Encrypt data using standard AES-256-GCM format
    let key_bytes = [0x33u8; 32];
    let hex_key = ai_backend::drive::crypto::encode_hex(&key_bytes);
    let engine_v1 = DriveCryptoEngine::from_hex(&hex_key).unwrap();

    let owner = "user_canonical";
    let resource = "resource_canonical";
    let data = b"Canonical test payload for backwards compatibility";

    let ciphertext = engine_v1.encrypt(owner, resource, data).unwrap();

    // Reconstruct new engine with the same valid key and verify it decrypts seamlessly
    let engine_v2 = DriveCryptoEngine::from_hex(&hex_key).unwrap();
    let recovered = engine_v2.decrypt(owner, resource, &ciphertext).unwrap();
    assert_eq!(recovered, data);
}

#[test]
fn test_04_missing_key_is_rejected() {
    // In AppConfig, if Drive sync is enabled, missing key is rejected
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    config.drive_encryption_key = None;
    let err = config.validate().expect_err("Missing key must be rejected");
    assert!(
        err.to_string().contains("DRIVE_ENCRYPTION_KEY"),
        "Error must identify missing DRIVE_ENCRYPTION_KEY: {err}"
    );

    // In DriveCryptoEngine, unconfigured engine fails closed
    let unconfigured = DriveCryptoEngine::unconfigured("No key provided");
    assert!(!unconfigured.is_configured());
    assert!(unconfigured.key_fingerprint().is_none());

    let enc_err = unconfigured
        .encrypt("owner", "res", b"data")
        .expect_err("Must fail closed");
    assert!(matches!(enc_err, CryptoError::KeyMissing(_)));

    let dec_err = unconfigured
        .decrypt("owner", "res", b"data")
        .expect_err("Must fail closed");
    assert!(matches!(dec_err, CryptoError::KeyMissing(_)));
}

#[test]
fn test_05_empty_key_is_rejected() {
    // In DriveCryptoEngine::from_hex
    let err = DriveCryptoEngine::from_hex("").expect_err("Empty string must be rejected");
    assert!(matches!(err, CryptoError::EmptyKey));

    // In AppConfig
    let mut config = AppConfig::default_local();
    config.drive_encryption_key = Some("".to_string());
    let config_err = config.validate().expect_err("Empty key must be rejected");
    assert!(config_err.to_string().contains("DRIVE_ENCRYPTION_KEY"));
}

#[test]
fn test_06_invalid_encoding_is_rejected() {
    // Non-hex characters
    let non_hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg";
    let err =
        DriveCryptoEngine::from_hex(non_hex).expect_err("Non-hex characters must be rejected");
    assert!(matches!(err, CryptoError::InvalidKeyEncoding(_)));

    // Whitespace inside or padding (must NOT be silently trimmed)
    let padded = " 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let err_padded = DriveCryptoEngine::from_hex(padded).expect_err("Padded key must be rejected");
    assert!(
        matches!(err_padded, CryptoError::InvalidKeyLength { .. })
            || matches!(err_padded, CryptoError::InvalidKeyEncoding(_))
    );

    let mut config = AppConfig::default_local();
    config.drive_encryption_key = Some(non_hex.to_string());
    assert!(config.validate().is_err());
}

#[test]
fn test_07_incorrect_key_length_is_rejected() {
    // Too short (62 hex chars = 31 bytes)
    let short_key = "a".repeat(62);
    let err_short =
        DriveCryptoEngine::from_hex(&short_key).expect_err("Short key must be rejected");
    assert!(matches!(
        err_short,
        CryptoError::InvalidKeyLength {
            expected: 64,
            actual: 62
        }
    ));

    // Too long (66 hex chars = 33 bytes)
    let long_key = "a".repeat(66);
    let err_long = DriveCryptoEngine::from_hex(&long_key).expect_err("Long key must be rejected");
    assert!(matches!(
        err_long,
        CryptoError::InvalidKeyLength {
            expected: 64,
            actual: 66
        }
    ));

    // Config validation also rejects incorrect lengths
    let mut config = AppConfig::default_local();
    config.drive_encryption_key = Some(short_key.to_string());
    assert!(config.validate().is_err());

    config.drive_encryption_key = Some(long_key.to_string());
    assert!(config.validate().is_err());
}

#[test]
fn test_08_wrong_key_cannot_silently_decrypt_existing_ciphertext() {
    let engine_1 = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
    let engine_2 = DriveCryptoEngine::from_hex(VALID_KEY_HEX_2).unwrap();

    let owner = "google:bob_99";
    let res = "res_1";
    let plaintext = b"Confidential business plans";

    let ciphertext = engine_1.encrypt(owner, res, plaintext).unwrap();

    let err = engine_2
        .decrypt(owner, res, &ciphertext)
        .expect_err("Wrong key MUST fail decryption");

    assert!(
        matches!(err, CryptoError::DecryptionFailed(_)),
        "Must return DecryptionFailed error, not partial or corrupted plaintext: {err:?}"
    );
}

#[test]
fn test_09_modified_ciphertext_fails_authentication() {
    let engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
    let owner = "google:carol";
    let res = "res_chat";
    let plaintext = b"Unmodified authentic text";

    let mut ciphertext = engine.encrypt(owner, res, plaintext).unwrap();

    // Modify a ciphertext byte (between nonce 12 and tag 16)
    let payload_pos = 14;
    ciphertext[payload_pos] ^= 0x40;

    let err = engine
        .decrypt(owner, res, &ciphertext)
        .expect_err("Tampered ciphertext must fail authentication");

    assert!(matches!(err, CryptoError::DecryptionFailed(_)));
}

#[test]
fn test_10_modified_authentication_tag_fails_authentication() {
    let engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
    let owner = "google:dave";
    let res = "res_tag";
    let plaintext = b"Data with authentication tag protection";

    let mut ciphertext = engine.encrypt(owner, res, plaintext).unwrap();

    // Modify the last byte of the 16-byte Poly1305 authentication tag
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 0x80;

    let err = engine
        .decrypt(owner, res, &ciphertext)
        .expect_err("Tampered authentication tag must fail authentication");

    assert!(matches!(err, CryptoError::DecryptionFailed(_)));
}

#[test]
fn test_11_restarting_with_same_key_preserves_decryption() {
    let owner = "user_restart_test";
    let res = "manifest_v1";
    let data = b"Persistence test across service restarts";

    // "Boot 1": Encrypt data
    let ciphertext = {
        let engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
        engine.encrypt(owner, res, data).unwrap()
    };

    // "Boot 2": Fresh process restart with exact same configured key
    let engine_after_restart = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
    let decrypted = engine_after_restart
        .decrypt(owner, res, &ciphertext)
        .expect("Decryption must succeed after restart with same key");

    assert_eq!(decrypted, data);
}

#[test]
fn test_12_restarting_with_different_key_does_not_silently_replace_original_key() {
    let owner = "user_key_swap";
    let res = "history_1";
    let data = b"Critical chat message";

    // Encrypt under Key 1
    let ciphertext = {
        let engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
        engine.encrypt(owner, res, data).unwrap()
    };

    // Restart with Key 2: Should fail securely without modifying ciphertext
    let engine_key_2 = DriveCryptoEngine::from_hex(VALID_KEY_HEX_2).unwrap();
    let result = engine_key_2.decrypt(owner, res, &ciphertext);
    assert!(
        result.is_err(),
        "Key 2 must not be able to decrypt Key 1 data"
    );

    // Restart back with Key 1: Ciphertext is intact and decryptable
    let engine_key_1_again = DriveCryptoEngine::from_hex(VALID_KEY_HEX_1).unwrap();
    let recovered = engine_key_1_again
        .decrypt(owner, res, &ciphertext)
        .expect("Ciphertext must remain completely intact and decryptable under original key");
    assert_eq!(recovered, data);
}

#[test]
fn test_13_no_automatic_fallback_key_is_generated_when_configuration_is_invalid() {
    // 1. Config with invalid key
    let mut config = AppConfig::default_local();
    config.drive_encryption_key = Some("invalid_short_key".to_string());
    assert!(
        config.validate().is_err(),
        "Invalid key in config must fail validation"
    );

    // 2. Initializing via default_sync_engine with invalid key results in unconfigured engine, NOT [0x5a; 32]
    let state = ai_backend::AppState::new(Arc::new(config.clone()));
    assert!(
        !state.sync_engine.crypto().is_configured(),
        "Must NOT configure a fallback key when DRIVE_ENCRYPTION_KEY is invalid"
    );

    // 3. Attempting to encrypt with this engine fails closed with KeyMissing
    let err = state
        .sync_engine
        .crypto()
        .encrypt("owner", "res", b"data")
        .expect_err("Must fail closed without fallback key");
    assert!(matches!(err, CryptoError::KeyMissing(_)));

    // 4. Data encrypted under the obsolete fallback key [0x5a; 32] CANNOT be decrypted
    let obsolete_engine = DriveCryptoEngine::from_hex(OBSOLETE_FALLBACK_KEY_HEX).unwrap();
    let legacy_ciphertext = obsolete_engine
        .encrypt("owner", "res", b"legacy secret")
        .unwrap();

    let dec_err = state
        .sync_engine
        .crypto()
        .decrypt("owner", "res", &legacy_ciphertext)
        .expect_err("Unconfigured engine must never decrypt legacy fallback data");
    assert!(matches!(dec_err, CryptoError::KeyMissing(_)));
}

#[tokio::test]
async fn test_14_failure_does_not_overwrite_or_delete_existing_encrypted_data() {
    let local_store = Arc::new(LocalSyncStore::new());
    let owner = "tenant_storage_safety";

    // Add conversation
    let mut conv = SyncConversation::new(
        "conv_preserved".to_string(),
        owner.to_string(),
        "Preserved Conversation".to_string(),
    );
    conv.updated_at = "2026-10-22T10:00:00Z".to_string();
    let _ = local_store
        .upsert_conversation(owner, conv.clone())
        .await
        .unwrap();

    // Create an unconfigured SyncEngine
    let mock_client = Arc::new(MockDriveClient::new());
    let unconfigured_crypto = Arc::new(DriveCryptoEngine::unconfigured("Testing failure safety"));
    let sync_engine = SyncEngine::new(mock_client, unconfigured_crypto, local_store.clone(), true);

    // Attempting to sync fails due to missing key
    let sync_result = sync_engine.sync(owner, "token_123").await;
    assert!(
        sync_result.is_err(),
        "Sync must fail when crypto is unconfigured"
    );

    // Verify conversation still exists and was NOT deleted or corrupted
    let retrieved = local_store
        .get_conversation(owner, "conv_preserved")
        .await
        .expect("Conversation must not be deleted after encryption/sync failure");
    assert_eq!(retrieved.id, "conv_preserved");
    assert_eq!(retrieved.title, "Preserved Conversation");
}

#[test]
fn test_15_error_messages_and_logs_do_not_disclose_the_key() {
    let secret_key = "a1".repeat(32);
    let engine = DriveCryptoEngine::from_hex(&secret_key).unwrap();

    // Check DriveCryptoEngine Debug format
    let debug_engine = format!("{engine:?}");
    assert!(
        !debug_engine.contains(&secret_key),
        "Engine Debug format must never contain secret key: {debug_engine}"
    );
    assert!(
        debug_engine.contains("key_fingerprint"),
        "Engine Debug should contain truncated fingerprint"
    );

    // Check AppConfig Debug format
    let mut config = AppConfig::default_local();
    config.drive_encryption_key = Some(secret_key.clone());
    let debug_config = format!("{config:?}");
    assert!(
        !debug_config.contains(&secret_key),
        "AppConfig Debug format must redact drive_encryption_key: {debug_config}"
    );
    assert!(
        debug_config.contains("[REDACTED]"),
        "AppConfig Debug format must show [REDACTED]"
    );

    // Check ConfigError formatting
    let err = config.validate();
    assert!(err.is_ok());

    let mut invalid_config = AppConfig::default_local();
    invalid_config.drive_encryption_key = Some("11223344invalid".to_string());
    let err = invalid_config.validate().unwrap_err();
    let err_str = format!("{err:?}");
    assert!(
        !err_str.contains("11223344invalid"),
        "ConfigError must not echo invalid key: {err_str}"
    );
}

#[test]
fn test_16_test_only_configuration_cannot_bypass_production_key_validation() {
    let mut prod_config = AppConfig::default_local();
    prod_config.environment = Environment::Production;
    prod_config.api_key = Some("super_secure_production_api_key_12345".to_string());
    prod_config.google_drive_sync_enabled = true;
    prod_config.drive_encryption_key = None;

    // Production environment strictly rejects missing key when Drive sync is enabled
    let err = prod_config
        .validate()
        .expect_err("Production must strictly reject missing Drive encryption key");
    assert!(
        err.to_string().contains("DRIVE_ENCRYPTION_KEY"),
        "Error must mention DRIVE_ENCRYPTION_KEY: {err}"
    );

    // Even if an invalid key is supplied in production, it is strictly rejected
    prod_config.drive_encryption_key = Some("short_key".to_string());
    let err2 = prod_config
        .validate()
        .expect_err("Production must strictly reject short Drive encryption key");
    assert!(err2.to_string().contains("DRIVE_ENCRYPTION_KEY"));
}

#[test]
fn test_17_startup_with_drive_sync_enabled_and_missing_or_invalid_key_fails() {
    // Missing key
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = true;
    config.drive_encryption_key = None;
    let err_missing = config.validate().unwrap_err();
    assert!(err_missing.to_string().contains("DRIVE_ENCRYPTION_KEY"));

    // Empty key
    config.drive_encryption_key = Some("".to_string());
    let err_empty = config.validate().unwrap_err();
    assert!(err_empty.to_string().contains("DRIVE_ENCRYPTION_KEY"));

    // Short key
    config.drive_encryption_key = Some("abcd".to_string());
    let err_short = config.validate().unwrap_err();
    assert!(err_short.to_string().contains("DRIVE_ENCRYPTION_KEY"));

    // Non-hex key
    config.drive_encryption_key = Some("z".repeat(64));
    let err_non_hex = config.validate().unwrap_err();
    assert!(err_non_hex.to_string().contains("DRIVE_ENCRYPTION_KEY"));
}

#[tokio::test]
async fn test_18_startup_with_drive_sync_disabled_and_encryption_dependent_operations_attempted() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = false;
    config.drive_encryption_key = None;
    assert!(
        config.validate().is_ok(),
        "Config is valid for non-Drive endpoints"
    );

    let state = ai_backend::AppState::new(Arc::new(config));
    assert!(!state.sync_engine.crypto().is_configured());

    // Encryption attempt fails closed
    let enc_err = state
        .sync_engine
        .crypto()
        .encrypt("owner", "res", b"data")
        .unwrap_err();
    assert!(matches!(enc_err, CryptoError::KeyMissing(_)));

    // Decryption attempt fails closed
    let dec_err = state
        .sync_engine
        .crypto()
        .decrypt("owner", "res", b"cipher")
        .unwrap_err();
    assert!(matches!(dec_err, CryptoError::KeyMissing(_)));

    // Sync attempt fails closed
    let sync_err = state.sync_engine.sync("owner", "token").await.unwrap_err();
    assert!(matches!(
        sync_err,
        ai_backend::drive::DriveError::SyncDisabled
    ));

    // Restore attempt fails closed with crypto error
    let restore_err = state
        .sync_engine
        .restore("owner", "token")
        .await
        .unwrap_err();
    assert!(matches!(
        restore_err,
        ai_backend::drive::DriveError::CryptoError(_)
    ));
}

#[tokio::test]
async fn test_19_runtime_attempts_to_enable_sync_without_valid_key_rejected() {
    let mut config = AppConfig::default_local();
    config.google_drive_sync_enabled = false;
    config.drive_encryption_key = None;
    let state = ai_backend::AppState::new(Arc::new(config));

    let update = ai_backend::drive::DriveConfigUpdate { enabled: true };
    let res = ai_backend::routes::drive::update_sync_config(
        axum::extract::State(state),
        None,
        axum::Json(update),
    )
    .await;

    assert!(
        res.is_err(),
        "Runtime request to enable sync without key MUST fail"
    );
    let err = res.unwrap_err();
    assert_eq!(err.status_code(), 400);
    // Verify typed internal variant contains DRIVE_ENCRYPTION_KEY diagnostic
    match &err {
        ai_backend::errors::AppError::InvalidRequest(raw_msg) => {
            assert!(
                raw_msg.contains("DRIVE_ENCRYPTION_KEY"),
                "Internal diagnostic must mention DRIVE_ENCRYPTION_KEY: {raw_msg}"
            );
        }
        other => panic!("Expected AppError::InvalidRequest, got: {other:?}"),
    }
    // Verify that the external HTTP client message is safely sanitized (zero leak)
    assert_eq!(
        err.client_message(),
        "Invalid authentication credentials or restricted parameter."
    );
}

#[test]
fn test_20_data_integrity_sha256_preservation_under_failure_scenarios() {
    let key_hex = VALID_KEY_HEX_1;
    let engine = DriveCryptoEngine::from_hex(key_hex).unwrap();
    let owner = "tenant_fixture_owner";
    let res = "doc_fixture_1";
    let original_payload = b"CRITICAL USER DOCUMENT: CANNOT BE CORRUPTED";

    // 1. Encrypt fixture
    let original_ciphertext = engine.encrypt(owner, res, original_payload).unwrap();
    let original_ciphertext_hash = ai_backend::drive::crypto::compute_sha256(&original_ciphertext);

    // 2. Trigger wrong-key decryption attempt
    let wrong_engine = DriveCryptoEngine::from_hex(VALID_KEY_HEX_2).unwrap();
    let fixture_copy = original_ciphertext.clone();
    let wrong_key_result = wrong_engine.decrypt(owner, res, &fixture_copy);
    assert!(wrong_key_result.is_err());
    // Verify fixture_copy remains byte-for-byte unchanged
    assert_eq!(fixture_copy, original_ciphertext);
    assert_eq!(
        ai_backend::drive::crypto::compute_sha256(&fixture_copy),
        original_ciphertext_hash
    );

    // 3. Trigger unconfigured engine attempt
    let unconfigured_engine = DriveCryptoEngine::unconfigured("No key configured");
    let unconfigured_result = unconfigured_engine.decrypt(owner, res, &fixture_copy);
    assert!(unconfigured_result.is_err());
    assert_eq!(fixture_copy, original_ciphertext);
    assert_eq!(
        ai_backend::drive::crypto::compute_sha256(&fixture_copy),
        original_ciphertext_hash
    );

    // 4. Trigger tampered ciphertext attempt (verify tamper fails authentication)
    let mut corrupted_buffer = original_ciphertext.clone();
    corrupted_buffer[15] ^= 0xEE;
    let tampered_result = engine.decrypt(owner, res, &corrupted_buffer);
    assert!(tampered_result.is_err());
    // Original fixture itself was never modified
    assert_eq!(
        ai_backend::drive::crypto::compute_sha256(&original_ciphertext),
        original_ciphertext_hash
    );

    // 5. Successful decryption still recovers exact original plaintext with matching hash
    let recovered = engine.decrypt(owner, res, &original_ciphertext).unwrap();
    assert_eq!(recovered, original_payload);
    assert_eq!(
        ai_backend::drive::crypto::compute_sha256(&recovered),
        ai_backend::drive::crypto::compute_sha256(original_payload)
    );
}
