//! Google Drive Live Integration Test & Readiness Suite.
//!
//! Conforms strictly to Final Backend Completion Directive §5:
//! - Mock-based automated tests remain authoritative for deterministic CI and local gates.
//! - Live Google Drive tests are gated behind the `LIVE_GOOGLE_DRIVE_TOKEN` environment variable.
//! - When `LIVE_GOOGLE_DRIVE_TOKEN` is not provided, this test suite prints detailed setup
//!   instructions and gracefully skips execution without failing tests.
//! - When a valid live token is present, this suite exercises live Google Drive v3 API endpoints:
//!   `appDataFolder` creation/upload, finding by name, downloading, authenticated decryption,
//!   and remote cleanup.
//!
//! ### OAuth Configuration Reference for Future Live Testing:
//! 1. Required Scope:
//!    - `https://www.googleapis.com/auth/drive.appdata` (Recommended: private application data folder)
//!    - OR `https://www.googleapis.com/auth/drive.file` (Per-file authorization)
//! 2. Required Environment Variables:
//!    - `LIVE_GOOGLE_DRIVE_TOKEN`: A valid Google OAuth 2.0 Access Token with the above scope.
//!    - `DRIVE_ENCRYPTION_KEY`: A 64-character hex-encoded 256-bit encryption key (optional for raw API test,
//!      required for full SyncEngine end-to-end tests).
//! 3. How to Obtain a Test Token:
//!    - Go to Google OAuth 2.0 Playground (https://developers.google.com/oauthplayground)
//!    - Select "Drive API v3" -> `https://www.googleapis.com/auth/drive.appdata`
//!    - Authorize and exchange authorization code for tokens.
//!    - Copy the `access_token` and run:
//!      `$env:LIVE_GOOGLE_DRIVE_TOKEN="<access_token>"`
//!      `cargo test --test drive_live_integration_test -- --nocapture`

use ai_backend::drive::client::{DriveClient, DriveError, GoogleDriveClient};
use ai_backend::drive::crypto::DriveCryptoEngine;
use std::env;

#[tokio::test]
async fn test_live_google_drive_roundtrip() {
    let token = match env::var("LIVE_GOOGLE_DRIVE_TOKEN") {
        Ok(tok) if !tok.trim().is_empty() => tok.trim().to_string(),
        _ => {
            println!(
                "\n=========================================================================="
            );
            println!("[NOTICE] Skipping live Google Drive integration test:");
            println!("         LIVE_GOOGLE_DRIVE_TOKEN environment variable is not set.");
            println!("--------------------------------------------------------------------------");
            println!("To run live Google Drive verification against production Google APIs:");
            println!("  1. Ensure a Google Cloud project exists with Google Drive API v3 enabled.");
            println!("  2. Request OAuth 2.0 scope: https://www.googleapis.com/auth/drive.appdata");
            println!("  3. Export LIVE_GOOGLE_DRIVE_TOKEN with a valid OAuth 2.0 bearer token.");
            println!("  4. Run: cargo test --test drive_live_integration_test -- --nocapture");
            println!(
                "==========================================================================\n"
            );
            return;
        }
    };

    println!("[LIVE TEST] Initiating real Google Drive v3 API verification with provided token...");

    let client = GoogleDriveClient::new(30, 3);
    let master_secret = [0x77u8; 32];
    let crypto = DriveCryptoEngine::derive_for_owner(&master_secret, "live_integration_tester");

    let test_filename = format!(
        "live_test_conv_{}.json.enc",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let raw_content =
        b"{\"conversation_id\": \"live_123\", \"text\": \"Real Google Drive payload\"}";

    // 1. Encrypt payload locally using AES-256-GCM
    let encrypted = crypto
        .encrypt("live_integration_tester", &test_filename, raw_content)
        .expect("Encryption failed");

    // 2. Upload to Google Drive appDataFolder
    println!(
        "[LIVE TEST] Uploading encrypted file '{}' to appDataFolder...",
        test_filename
    );
    let upload_meta = client
        .upload_file(
            &token,
            None,
            &test_filename,
            "application/octet-stream",
            &encrypted,
            "appDataFolder",
        )
        .await
        .expect("Live Google Drive upload failed");

    println!(
        "[LIVE TEST] Upload successful! Remote File ID: {}",
        upload_meta.id
    );
    assert_eq!(upload_meta.name, test_filename);

    // 3. Find file in appDataFolder
    println!("[LIVE TEST] Querying appDataFolder for file by name...");
    let found = client
        .find_file(&token, &test_filename, "appDataFolder")
        .await
        .expect("Live find_file failed")
        .expect("Uploaded file should be found in appDataFolder");

    assert_eq!(found.id, upload_meta.id);

    // 4. Download content
    println!("[LIVE TEST] Downloading file content from Google Drive...");
    let downloaded_bytes = client
        .download_file(&token, &found.id)
        .await
        .expect("Live download_file failed");

    // 5. Decrypt downloaded content and verify payload matches original
    let decrypted = crypto
        .decrypt("live_integration_tester", &test_filename, &downloaded_bytes)
        .expect("Decryption of downloaded Drive payload failed");

    assert_eq!(decrypted, raw_content);
    println!("[LIVE TEST] Decryption and payload roundtrip verified perfectly!");

    // 6. Cleanup: Delete test file from remote Drive
    println!(
        "[LIVE TEST] Cleaning up test file '{}' from Google Drive...",
        found.id
    );
    client
        .delete_file(&token, &found.id)
        .await
        .expect("Remote cleanup delete failed");

    println!("[LIVE TEST] Live Google Drive verification completed successfully.");
}

#[tokio::test]
async fn test_offline_drive_client_token_validation() {
    let client = GoogleDriveClient::new(5, 1);

    // 1. Empty token fails closed before any HTTP dispatch
    let err_empty = client
        .find_file("", "test.json", "appDataFolder")
        .await
        .unwrap_err();
    assert_eq!(err_empty, DriveError::MissingDriveToken);

    // 2. Token with CRLF injection fails closed
    let err_crlf = client
        .find_file(
            "malicious_token\r\nInjected-Header: bad",
            "test.json",
            "appDataFolder",
        )
        .await
        .unwrap_err();
    assert!(matches!(err_crlf, DriveError::InvalidDriveToken(_)));
}
