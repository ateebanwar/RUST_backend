//! Ephemeral in-memory storage and request working cache.
//!
//! Conforms to MASTER_SPEC.md and Master Directive §2 & §13:
//! - Stateless processing layer: Google Drive is the sole permanent store for user data.
//! - No permanent backend persistence: No SQLite tables (`local_sync_conversations`,
//!   `local_sync_library`, `local_sync_metadata`), no local JSON files, no disk caches.
//! - Ephemeral memory: Caches conversations, messages, and library items in memory during active
//!   request processing, and drops them across restarts.
//! - Multi-tenant isolation: Strictly partitioned by `owner_subject`.
//! - Independent Library lifecycle: Deleting a conversation NEVER deletes its associated Library items.
//! - Change tracking: Maintains sync status and dirty tracking for Drive commit protocols.
//! - Safe logout: Clears in-memory session without affecting remote Drive objects.

use super::models::{LibraryItem, SyncConversation, chrono_like_now};
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

/// Errors occurring in LocalSyncStore operations.
#[derive(Debug)]
pub enum LocalStoreError {
    ConnectionError(String),
    MigrationError(String),
    SqliteError(String),
    SerializationError(String),
    LockError(String),
}

impl std::fmt::Display for LocalStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionError(msg) => write!(f, "Connection failure: {msg}"),
            Self::MigrationError(msg) => write!(f, "Migration failure: {msg}"),
            Self::SqliteError(msg) => write!(f, "Database failure: {msg}"),
            Self::SerializationError(msg) => write!(f, "Serialization failure: {msg}"),
            Self::LockError(msg) => write!(f, "Lock acquisition failure: {msg}"),
        }
    }
}

impl std::error::Error for LocalStoreError {}

impl From<rusqlite::Error> for LocalStoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::SqliteError(e.to_string())
    }
}

impl From<serde_json::Error> for LocalStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::SerializationError(e.to_string())
    }
}

impl From<LocalStoreError> for crate::drive::DriveError {
    fn from(e: LocalStoreError) -> Self {
        crate::drive::DriveError::Internal(e.to_string())
    }
}

impl From<LocalStoreError> for crate::errors::AppError {
    fn from(e: LocalStoreError) -> Self {
        crate::errors::AppError::InternalError(e.to_string())
    }
}

/// Entry tracking local conversation state and sync status.
#[derive(Debug, Clone)]
pub struct LocalConversationRecord {
    pub conversation: SyncConversation,
    pub is_dirty: bool,
    pub drive_file_id: Option<String>,
}

/// Entry tracking local library item state, binary payload, and sync status.
#[derive(Debug, Clone)]
pub struct LocalLibraryRecord {
    pub item: LibraryItem,
    pub content: Option<Vec<u8>>,
    pub is_dirty: bool,
    pub drive_file_id: Option<String>,
}

/// Per-owner in-memory store.
#[derive(Debug, Default, Clone)]
struct OwnerStore {
    conversations: HashMap<String, LocalConversationRecord>,
    library_items: HashMap<String, LocalLibraryRecord>,
    last_sync_time: Option<String>,
    last_error: Option<String>,
}

/// Thread-safe ephemeral in-memory synchronization and working cache.
///
/// In accordance with Master Directive §2, permanent backend storage is strictly
/// prohibited. Google Drive is the sole permanent source of truth.
#[derive(Clone)]
pub struct LocalSyncStore {
    stores: Arc<RwLock<HashMap<String, OwnerStore>>>,
}

impl std::fmt::Debug for LocalSyncStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSyncStore")
            .field("is_persistent", &false)
            .finish()
    }
}

impl Default for LocalSyncStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalSyncStore {
    /// Create a new, clean in-memory local synchronization store (transient).
    pub fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Alias for in-memory store.
    pub fn in_memory() -> Self {
        Self::new()
    }

    /// Backwards-compatible constructor for opening store.
    ///
    /// Per Master Directive §2 & §13, backend database persistence of user content
    /// is eliminated in favor of Google Drive. Returns an ephemeral in-memory store.
    pub fn open(_db_path: &str) -> Result<Self, LocalStoreError> {
        Ok(Self::new())
    }

    /// Backwards-compatible constructor from existing connection handle.
    pub fn from_connection(_conn: Arc<Mutex<Connection>>) -> Result<Self, LocalStoreError> {
        Ok(Self::new())
    }

    /// Check if this store is backed by durable persistent storage.
    ///
    /// Always returns `false` per Master Directive §2.2: the backend is a stateless
    /// processing layer; Google Drive is the only permanent source of truth.
    pub fn is_persistent(&self) -> bool {
        false
    }

    /// List active (non-deleted) conversations for an owner.
    pub async fn list_conversations(&self, owner: &str) -> Vec<SyncConversation> {
        let lock = self.stores.read().await;
        if let Some(store) = lock.get(owner) {
            store
                .conversations
                .values()
                .filter(|r| !r.conversation.deleted)
                .map(|r| r.conversation.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Retrieve conversation by ID for an owner (active only).
    pub async fn get_conversation(&self, owner: &str, id: &str) -> Option<SyncConversation> {
        let lock = self.stores.read().await;
        let store = lock.get(owner)?;
        let record = store.conversations.get(id)?;
        if record.conversation.deleted {
            None
        } else {
            Some(record.conversation.clone())
        }
    }

    /// Retrieve conversation raw state by ID (including soft-deleted).
    pub async fn get_conversation_raw(&self, owner: &str, id: &str) -> Option<SyncConversation> {
        let lock = self.stores.read().await;
        let store = lock.get(owner)?;
        store.conversations.get(id).map(|r| r.conversation.clone())
    }

    /// Create or update a conversation locally, bumping revision and marking dirty.
    pub async fn upsert_conversation(
        &self,
        owner: &str,
        mut conv: SyncConversation,
    ) -> Result<SyncConversation, LocalStoreError> {
        let now = chrono_like_now();
        conv.owner_subject = owner.to_string();
        conv.updated_at = now;

        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();

        let drive_file_id = if let Some(existing) = store.conversations.get(&conv.id) {
            conv.revision = existing.conversation.revision + 1;
            existing.drive_file_id.clone()
        } else {
            None
        };

        let result = conv.clone();
        store.conversations.insert(
            conv.id.clone(),
            LocalConversationRecord {
                conversation: conv,
                is_dirty: true,
                drive_file_id,
            },
        );

        Ok(result)
    }

    /// Soft-delete a conversation (tombstone).
    ///
    /// CRITICAL RULE: Deleting a conversation NEVER deletes its associated Library items!
    pub async fn delete_conversation(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<bool, LocalStoreError> {
        let now = chrono_like_now();
        let mut lock = self.stores.write().await;
        let Some(store) = lock.get_mut(owner) else {
            return Ok(false);
        };

        let Some(record) = store.conversations.get_mut(id) else {
            return Ok(false);
        };

        let new_revision = record.conversation.revision + 1;
        record.conversation.deleted = true;
        record.conversation.deleted_at = Some(now.clone());
        record.conversation.updated_at = now;
        record.conversation.revision = new_revision;
        record.is_dirty = true;
        Ok(true)
    }

    /// List active (non-deleted) library items for an owner.
    pub async fn list_library_items(&self, owner: &str) -> Vec<LibraryItem> {
        let lock = self.stores.read().await;
        if let Some(store) = lock.get(owner) {
            store
                .library_items
                .values()
                .filter(|r| !r.item.deleted)
                .map(|r| r.item.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Retrieve library item metadata by ID for an owner.
    pub async fn get_library_item(&self, owner: &str, id: &str) -> Option<LibraryItem> {
        let lock = self.stores.read().await;
        let store = lock.get(owner)?;
        let record = store.library_items.get(id)?;
        if record.item.deleted {
            None
        } else {
            Some(record.item.clone())
        }
    }

    /// Retrieve raw binary content of a library item.
    pub async fn get_library_content(&self, owner: &str, id: &str) -> Option<Vec<u8>> {
        let lock = self.stores.read().await;
        let store = lock.get(owner)?;
        let record = store.library_items.get(id)?;
        if record.item.deleted {
            None
        } else {
            record.content.clone()
        }
    }

    /// Cache downloaded decrypted binary content in ephemeral memory for subsequent requests.
    pub async fn cache_library_content(&self, owner: &str, id: &str, content: &[u8]) {
        let mut lock = self.stores.write().await;
        if let Some(store) = lock.get_mut(owner)
            && let Some(record) = store.library_items.get_mut(id)
        {
            record.content = Some(content.to_vec());
        }
    }

    /// Save or update a library item, persisting metadata and binary payload, and marking dirty.
    pub async fn save_library_item(
        &self,
        owner: &str,
        mut item: LibraryItem,
        content: Option<Vec<u8>>,
    ) -> Result<LibraryItem, LocalStoreError> {
        let now = chrono_like_now();
        item.owner_subject = owner.to_string();
        item.updated_at = now;

        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();

        let drive_file_id = store
            .library_items
            .get(item.id.as_str())
            .and_then(|r| r.drive_file_id.clone());

        let result = item.clone();
        store.library_items.insert(
            item.id.clone(),
            LocalLibraryRecord {
                item,
                content,
                is_dirty: true,
                drive_file_id,
            },
        );

        Ok(result)
    }

    /// Explicitly delete a library item.
    pub async fn delete_library_item(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<bool, LocalStoreError> {
        let now = chrono_like_now();
        let mut lock = self.stores.write().await;
        let Some(store) = lock.get_mut(owner) else {
            return Ok(false);
        };

        let Some(record) = store.library_items.get_mut(id) else {
            return Ok(false);
        };

        record.item.deleted = true;
        record.item.deleted_at = Some(now.clone());
        record.item.updated_at = now;
        record.is_dirty = true;
        record.content = None; // Free memory on delete
        Ok(true)
    }

    /// Retrieve dirty conversations awaiting synchronization.
    pub async fn get_pending_conversations(&self, owner: &str) -> Vec<LocalConversationRecord> {
        let lock = self.stores.read().await;
        if let Some(store) = lock.get(owner) {
            store
                .conversations
                .values()
                .filter(|r| r.is_dirty)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Mark a conversation clean and record its assigned remote Drive file ID.
    pub async fn mark_conversation_synced(
        &self,
        owner: &str,
        id: &str,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        let Some(store) = lock.get_mut(owner) else {
            return Ok(());
        };

        if let Some(record) = store.conversations.get_mut(id) {
            record.is_dirty = false;
            record.drive_file_id = Some(drive_file_id);
        }

        Ok(())
    }

    /// Apply a conversation downloaded from remote Drive during sync, setting `is_dirty: false`.
    pub async fn apply_remote_conversation(
        &self,
        owner: &str,
        conv: SyncConversation,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();

        store.conversations.insert(
            conv.id.clone(),
            LocalConversationRecord {
                conversation: conv,
                is_dirty: false,
                drive_file_id: Some(drive_file_id),
            },
        );

        Ok(())
    }

    /// Retrieve dirty library items awaiting synchronization.
    pub async fn get_pending_library_items(&self, owner: &str) -> Vec<LocalLibraryRecord> {
        let lock = self.stores.read().await;
        if let Some(store) = lock.get(owner) {
            store
                .library_items
                .values()
                .filter(|r| r.is_dirty)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Mark a library item clean and record its assigned remote Drive file ID.
    pub async fn mark_library_item_synced(
        &self,
        owner: &str,
        id: &str,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        let Some(store) = lock.get_mut(owner) else {
            return Ok(());
        };

        if let Some(record) = store.library_items.get_mut(id) {
            record.is_dirty = false;
            record.drive_file_id = Some(drive_file_id);
        }

        Ok(())
    }

    /// Apply a library item downloaded from remote Drive during sync, setting `is_dirty: false`.
    pub async fn apply_remote_library_item(
        &self,
        owner: &str,
        item: LibraryItem,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();

        store.library_items.insert(
            item.id.clone(),
            LocalLibraryRecord {
                item,
                content: None,
                is_dirty: false,
                drive_file_id: Some(drive_file_id),
            },
        );

        Ok(())
    }

    /// Query pending sync counts and timestamps for an owner.
    pub async fn get_sync_stats(
        &self,
        owner: &str,
    ) -> (usize, usize, Option<String>, Option<String>) {
        let lock = self.stores.read().await;
        if let Some(store) = lock.get(owner) {
            let pending_convs = store.conversations.values().filter(|r| r.is_dirty).count();
            let pending_libs = store.library_items.values().filter(|r| r.is_dirty).count();
            (
                pending_convs,
                pending_libs,
                store.last_sync_time.clone(),
                store.last_error.clone(),
            )
        } else {
            (0, 0, None, None)
        }
    }

    /// Update last synchronization time or record last sync error.
    pub async fn update_sync_status(
        &self,
        owner: &str,
        last_sync: Option<String>,
        last_error: Option<String>,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();

        if let Some(t) = last_sync {
            store.last_sync_time = Some(t);
            store.last_error = None;
        }
        if let Some(err) = last_error {
            store.last_error = Some(err);
        }

        Ok(())
    }

    /// Clear local in-memory session and cached records for a user on logout.
    ///
    /// Remote Google Drive records are NEVER deleted.
    pub async fn clear_user_session(&self, owner: &str) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        lock.remove(owner);
        Ok(())
    }
}
