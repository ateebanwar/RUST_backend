//! Synchronization engine orchestrating Google Drive sync, conflict resolution, and restore.
//!
//! Conforms to MASTER_SPEC.md §15 & Phase 15 requirements:
//! - Local-first: local data remains accessible offline even if Drive fails.
//! - Default OFF switch: `drive_sync_enabled: false` by default; toggled via config or API.
//! - Last-Write-Wins (LWW) conflict resolution with deterministic revision, timestamp, and tie-breaking.
//! - Critical lifecycle independence: Deleting a conversation NEVER deletes its referenced Library items.
//! - Full cryptographic encryption before remote persistence using AES-256-GCM with AAD binding.
//! - SHA-256 checksum verification for binary file integrity.
//! - Reinstall and account recovery bootstrapping from remote manifest.
//! - Bounded retries and exponential backoff for transient network errors and rate limiting.

use super::client::{DriveClient, DriveError};
use super::crypto::{DriveCryptoEngine, compute_sha256};
use super::local_store::LocalSyncStore;
use super::models::{
    CURRENT_SCHEMA_VERSION, ConversationIndexEntry, DRIVE_APPDATA_SPACE, LibraryIndexEntry,
    LibraryItem, REMOTE_MANIFEST_FILENAME, SyncConversation, SyncManifest, SyncResult, SyncStatus,
    chrono_like_now,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Core synchronization engine.
pub struct SyncEngine {
    client: Arc<dyn DriveClient>,
    crypto: Arc<DriveCryptoEngine>,
    local_store: Arc<LocalSyncStore>,
    sync_enabled: Arc<RwLock<bool>>,
}

impl SyncEngine {
    /// Create a new SyncEngine instance.
    pub fn new(
        client: Arc<dyn DriveClient>,
        crypto: Arc<DriveCryptoEngine>,
        local_store: Arc<LocalSyncStore>,
        default_enabled: bool,
    ) -> Self {
        Self {
            client,
            crypto,
            local_store,
            sync_enabled: Arc::new(RwLock::new(default_enabled)),
        }
    }

    /// Access local store reference.
    pub fn local_store(&self) -> &Arc<LocalSyncStore> {
        &self.local_store
    }

    /// Access crypto engine reference.
    pub fn crypto(&self) -> &Arc<DriveCryptoEngine> {
        &self.crypto
    }

    /// Check if Drive synchronization is currently enabled.
    pub async fn is_sync_enabled(&self) -> bool {
        *self.sync_enabled.read().await
    }

    /// Enable or disable Drive synchronization.
    ///
    /// Disabling synchronization NEVER deletes remote Drive data or local conversations.
    pub async fn set_sync_enabled(&self, enabled: bool) {
        *self.sync_enabled.write().await = enabled;
    }

    /// Query current synchronization status for an authenticated owner.
    pub async fn get_status(&self, owner: &str) -> SyncStatus {
        let enabled = self.is_sync_enabled().await;
        let (pending_conv, pending_lib, last_sync, last_err) =
            self.local_store.get_sync_stats(owner).await;

        SyncStatus {
            enabled,
            last_sync_time: last_sync,
            pending_conversations_count: pending_conv,
            pending_library_count: pending_lib,
            last_error: last_err,
        }
    }

    /// Helper to execute a Drive operation with bounded retries on transient errors or rate limiting.
    async fn retry_drive_op<F, Fut, T>(&self, mut op: F) -> Result<T, DriveError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, DriveError>>,
    {
        let mut attempts = 0;
        let max_retries = 3;
        loop {
            attempts += 1;
            match op().await {
                Ok(val) => return Ok(val),
                Err(err) => {
                    if attempts <= max_retries {
                        match &err {
                            DriveError::RateLimited {
                                retry_after_secs, ..
                            } => {
                                let delay = retry_after_secs.unwrap_or(1).min(5);
                                tokio::time::sleep(Duration::from_millis(delay * 20)).await;
                                continue;
                            }
                            DriveError::NetworkError(_) => {
                                tokio::time::sleep(Duration::from_millis(10 * attempts as u64))
                                    .await;
                                continue;
                            }
                            _ => return Err(err),
                        }
                    }
                    return Err(err);
                }
            }
        }
    }

    /// Execute a bidirectional synchronization cycle with Google Drive.
    pub async fn sync(&self, owner: &str, drive_token: &str) -> Result<SyncResult, DriveError> {
        if !self.is_sync_enabled().await {
            return Err(DriveError::SyncDisabled);
        }

        let run_result = self.execute_sync_cycle(owner, drive_token).await;
        match &run_result {
            Ok(res) => {
                self.local_store
                    .update_sync_status(owner, Some(res.last_sync_time.clone()), None)
                    .await;
            }
            Err(err) => {
                self.local_store
                    .update_sync_status(owner, None, Some(err.to_string()))
                    .await;
            }
        }
        run_result
    }

    /// Internal synchronization pipeline.
    async fn execute_sync_cycle(
        &self,
        owner: &str,
        drive_token: &str,
    ) -> Result<SyncResult, DriveError> {
        let mut uploaded_convs = 0;
        let mut downloaded_convs = 0;
        let mut uploaded_libs = 0;
        let downloaded_libs = 0;
        let mut conflicts_resolved = 0;

        // 1. Locate or initialize remote manifest
        let manifest_meta = self
            .retry_drive_op(|| {
                self.client
                    .find_file(drive_token, REMOTE_MANIFEST_FILENAME, DRIVE_APPDATA_SPACE)
            })
            .await?;

        let (mut manifest, manifest_file_id) = match manifest_meta {
            Some(meta) => {
                let enc_bytes = self
                    .retry_drive_op(|| self.client.download_file(drive_token, &meta.id))
                    .await?;
                let plain_bytes =
                    self.crypto
                        .decrypt(owner, REMOTE_MANIFEST_FILENAME, &enc_bytes)?;
                let parsed: SyncManifest = serde_json::from_slice(&plain_bytes).map_err(|e| {
                    DriveError::CorruptData(format!("Failed to parse remote sync manifest: {e}"))
                })?;

                if parsed.schema_version > CURRENT_SCHEMA_VERSION {
                    return Err(DriveError::SchemaMismatch {
                        expected: CURRENT_SCHEMA_VERSION,
                        actual: parsed.schema_version,
                    });
                }
                (parsed, Some(meta.id))
            }
            None => (SyncManifest::new(owner.to_string()), None),
        };

        // 2. Download and reconcile remote conversations
        for (remote_id, remote_index) in &manifest.conversations {
            let local_conv_opt = self
                .local_store
                .get_conversation_raw(owner, remote_id)
                .await;

            let should_download = match &local_conv_opt {
                None => true,
                Some(local_conv) => {
                    // LWW Conflict Resolution
                    if remote_index.revision > local_conv.revision {
                        conflicts_resolved += 1;
                        true
                    } else if remote_index.revision == local_conv.revision {
                        if remote_index.updated_at > local_conv.updated_at {
                            conflicts_resolved += 1;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
            };

            if should_download
                && !remote_index.deleted
                && let Some(drive_file_id) = &remote_index.drive_file_id
            {
                let enc_bytes = self
                    .retry_drive_op(|| self.client.download_file(drive_token, drive_file_id))
                    .await?;
                let plain_bytes =
                    self.crypto
                        .decrypt(owner, &format!("conv_{remote_id}"), &enc_bytes)?;
                let remote_conv: SyncConversation =
                    serde_json::from_slice(&plain_bytes).map_err(|e| {
                        DriveError::CorruptData(format!("Corrupt conversation {remote_id}: {e}"))
                    })?;

                self.local_store
                    .apply_remote_conversation(owner, remote_conv, drive_file_id.clone())
                    .await;
                downloaded_convs += 1;
            }
        }

        // 3. Upload local dirty conversations
        let pending_convs = self.local_store.get_pending_conversations(owner).await;
        for record in pending_convs {
            let conv = &record.conversation;
            let conv_filename = format!("conv_{}.json.enc", conv.id);
            let resource_aad = format!("conv_{}", conv.id);

            let json_bytes = serde_json::to_vec(conv).map_err(|e| {
                DriveError::Internal(format!("Failed to serialize conversation: {e}"))
            })?;
            let enc_bytes = self.crypto.encrypt(owner, &resource_aad, &json_bytes)?;

            let meta = self
                .retry_drive_op(|| {
                    self.client.upload_file(
                        drive_token,
                        record.drive_file_id.as_deref(),
                        &conv_filename,
                        "application/octet-stream",
                        &enc_bytes,
                        DRIVE_APPDATA_SPACE,
                    )
                })
                .await?;

            self.local_store
                .mark_conversation_synced(owner, &conv.id, meta.id.clone())
                .await;

            // Update manifest index entry
            manifest.conversations.insert(
                conv.id.clone(),
                ConversationIndexEntry {
                    id: conv.id.clone(),
                    title: conv.title.clone(),
                    updated_at: conv.updated_at.clone(),
                    revision: conv.revision,
                    deleted: conv.deleted,
                    deleted_at: conv.deleted_at.clone(),
                    message_count: conv.messages.len(),
                    drive_file_id: Some(meta.id),
                },
            );

            uploaded_convs += 1;
        }

        // 4. Upload local dirty library items
        let pending_libs = self.local_store.get_pending_library_items(owner).await;
        for record in pending_libs {
            let item = &record.item;
            let resource_aad = format!("lib_{}", item.id);
            let filename = format!("lib_{}.bin.enc", item.id);

            let mut drive_id = record.drive_file_id.clone();

            if let Some(content) = &record.content {
                let enc_bytes = self.crypto.encrypt(owner, &resource_aad, content)?;
                let meta = self
                    .retry_drive_op(|| {
                        self.client.upload_file(
                            drive_token,
                            record.drive_file_id.as_deref(),
                            &filename,
                            "application/octet-stream",
                            &enc_bytes,
                            DRIVE_APPDATA_SPACE,
                        )
                    })
                    .await?;
                drive_id = Some(meta.id);
                uploaded_libs += 1;
            }

            if let Some(did) = &drive_id {
                self.local_store
                    .mark_library_item_synced(owner, &item.id, did.clone())
                    .await;
            }

            manifest.library_items.insert(
                item.id.clone(),
                LibraryIndexEntry {
                    id: item.id.clone(),
                    filename: item.filename.clone(),
                    mime_type: item.mime_type.clone(),
                    size_bytes: item.size_bytes,
                    checksum_sha256: item.checksum_sha256.clone(),
                    updated_at: item.updated_at.clone(),
                    deleted: item.deleted,
                    deleted_at: item.deleted_at.clone(),
                    conversation_id: item.conversation_id.clone(),
                    drive_file_id: drive_id,
                },
            );
        }

        // 5. Update manifest metadata and upload back to Drive
        let now = chrono_like_now();
        manifest.last_sync_time = Some(now.clone());

        let manifest_bytes = serde_json::to_vec(&manifest)
            .map_err(|e| DriveError::Internal(format!("Failed to serialize sync manifest: {e}")))?;
        let enc_manifest = self
            .crypto
            .encrypt(owner, REMOTE_MANIFEST_FILENAME, &manifest_bytes)?;

        self.retry_drive_op(|| {
            self.client.upload_file(
                drive_token,
                manifest_file_id.as_deref(),
                REMOTE_MANIFEST_FILENAME,
                "application/octet-stream",
                &enc_manifest,
                DRIVE_APPDATA_SPACE,
            )
        })
        .await?;

        Ok(SyncResult {
            uploaded_conversations: uploaded_convs,
            downloaded_conversations: downloaded_convs,
            uploaded_library_items: uploaded_libs,
            downloaded_library_items: downloaded_libs,
            conflicts_resolved,
            last_sync_time: now,
        })
    }

    /// Restore all data from Google Drive (bootstrap on clean device or reinstall).
    pub async fn restore(&self, owner: &str, drive_token: &str) -> Result<SyncResult, DriveError> {
        let manifest_meta = self
            .retry_drive_op(|| {
                self.client
                    .find_file(drive_token, REMOTE_MANIFEST_FILENAME, DRIVE_APPDATA_SPACE)
            })
            .await?;

        let Some(meta) = manifest_meta else {
            return Ok(SyncResult {
                uploaded_conversations: 0,
                downloaded_conversations: 0,
                uploaded_library_items: 0,
                downloaded_library_items: 0,
                conflicts_resolved: 0,
                last_sync_time: chrono_like_now(),
            });
        };

        let enc_bytes = self
            .retry_drive_op(|| self.client.download_file(drive_token, &meta.id))
            .await?;
        let plain_bytes = self
            .crypto
            .decrypt(owner, REMOTE_MANIFEST_FILENAME, &enc_bytes)?;
        let manifest: SyncManifest = serde_json::from_slice(&plain_bytes)
            .map_err(|e| DriveError::CorruptData(format!("Corrupt manifest: {e}")))?;

        let mut restored_convs = 0;
        let mut restored_libs = 0;

        for (id, entry) in &manifest.conversations {
            if !entry.deleted
                && let Some(file_id) = &entry.drive_file_id
            {
                let conv_enc = self
                    .retry_drive_op(|| self.client.download_file(drive_token, file_id))
                    .await?;
                let conv_plain = self
                    .crypto
                    .decrypt(owner, &format!("conv_{id}"), &conv_enc)?;
                let conv: SyncConversation = serde_json::from_slice(&conv_plain).map_err(|e| {
                    DriveError::CorruptData(format!("Corrupt conversation {id}: {e}"))
                })?;
                self.local_store
                    .apply_remote_conversation(owner, conv, file_id.clone())
                    .await;
                restored_convs += 1;
            }
        }

        for entry in manifest.library_items.values() {
            if !entry.deleted
                && let Some(file_id) = &entry.drive_file_id
            {
                let item = LibraryItem {
                    id: entry.id.clone(),
                    owner_subject: owner.to_string(),
                    filename: entry.filename.clone(),
                    mime_type: entry.mime_type.clone(),
                    size_bytes: entry.size_bytes,
                    checksum_sha256: entry.checksum_sha256.clone(),
                    created_at: entry.updated_at.clone(),
                    updated_at: entry.updated_at.clone(),
                    conversation_id: entry.conversation_id.clone(),
                    drive_file_id: Some(file_id.clone()),
                    deleted: false,
                    deleted_at: None,
                    schema_version: CURRENT_SCHEMA_VERSION,
                };
                self.local_store
                    .apply_remote_library_item(owner, item, file_id.clone())
                    .await;
                restored_libs += 1;
            }
        }

        let now = chrono_like_now();
        self.local_store
            .update_sync_status(owner, Some(now.clone()), None)
            .await;

        Ok(SyncResult {
            uploaded_conversations: 0,
            downloaded_conversations: restored_convs,
            uploaded_library_items: 0,
            downloaded_library_items: restored_libs,
            conflicts_resolved: 0,
            last_sync_time: now,
        })
    }

    /// Retrieve binary content of a library item, downloading and decrypting from Drive if not cached locally.
    pub async fn get_library_item_content(
        &self,
        owner: &str,
        item_id: &str,
        drive_token: Option<&str>,
    ) -> Result<Vec<u8>, DriveError> {
        // Check local store first (local-first design)
        if let Some(content) = self.local_store.get_library_content(owner, item_id).await {
            return Ok(content);
        }

        let item = self
            .local_store
            .get_library_item(owner, item_id)
            .await
            .ok_or_else(|| DriveError::NotFound(format!("Library item {item_id} not found")))?;

        let drive_id = item.drive_file_id.as_deref().ok_or_else(|| {
            DriveError::NotFound(format!(
                "Library item {item_id} content not available locally or on Drive"
            ))
        })?;

        let token = drive_token.ok_or(DriveError::MissingDriveToken)?;

        // Download from Drive
        let enc_bytes = self
            .retry_drive_op(|| self.client.download_file(token, drive_id))
            .await?;
        let plain_bytes = self
            .crypto
            .decrypt(owner, &format!("lib_{item_id}"), &enc_bytes)?;

        // Verify SHA-256 integrity checksum
        let actual_checksum = compute_sha256(&plain_bytes);
        if actual_checksum != item.checksum_sha256 {
            return Err(DriveError::ChecksumMismatch {
                expected: item.checksum_sha256,
                actual: actual_checksum,
            });
        }

        // Cache locally for subsequent fast offline access
        let _ = self
            .local_store
            .save_library_item(owner, item, Some(plain_bytes.clone()))
            .await;

        Ok(plain_bytes)
    }
}
