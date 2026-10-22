//! Google Drive chat and library synchronization module.
//!
//! Conforms to MASTER_SPEC.md §15 and Phase 15 requirements:
//! - Secure, private Google Drive `appDataFolder` remote persistence.
//! - AES-256-GCM encryption with AAD binding.
//! - Local-first offline-friendly storage.
//! - Independent Library lifecycle surviving conversation deletion.
//! - LWW conflict resolution and account restoration.

pub mod client;
pub mod crypto;
pub mod local_store;
pub mod mock;
pub mod models;
pub mod sync_engine;

pub use client::{DriveClient, DriveError, DriveFileMetadata, GoogleDriveClient};
pub use crypto::{DriveCryptoEngine, compute_sha256, decode_base64, encode_base64};
pub use local_store::{LocalStoreError, LocalSyncStore};
pub use mock::MockDriveClient;
pub use models::{
    CURRENT_SCHEMA_VERSION, ConversationIndexEntry, CreateConversationRequest,
    CreateLibraryItemRequest, DRIVE_APPDATA_SPACE, DriveConfigUpdate, LibraryIndexEntry,
    LibraryItem, REMOTE_MANIFEST_FILENAME, SyncConversation, SyncManifest, SyncMessage, SyncResult,
    SyncStatus, UpdateConversationRequest, chrono_like_now, sanitize_filename,
};
pub use sync_engine::SyncEngine;
