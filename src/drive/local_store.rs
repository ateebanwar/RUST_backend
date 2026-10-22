//! Local-first storage and pending synchronization state management.
//!
//! Conforms to MASTER_SPEC.md §15 and Phase 15 requirements:
//! - Local-first: chats and library items are accessible immediately offline without waiting for Drive.
//! - Strict multi-tenant isolation: indexed by `owner_subject`; User A cannot view User B's content.
//! - Critical lifecycle rule: Soft-deleting a conversation NEVER deletes its associated Library items.
//! - Library items have an independent lifecycle and are only deleted via explicit Library deletion.
//! - Change tracking: maintains dirty flags (`pending_upload`, `pending_delete`) for incremental sync.
//! - Logout lifecycle: clears local in-memory session/cache without affecting remote Drive objects.

use super::models::{LibraryItem, SyncConversation, chrono_like_now};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// Thread-safe local-first synchronization store.
#[derive(Clone, Default)]
pub struct LocalSyncStore {
    stores: Arc<RwLock<HashMap<String, OwnerStore>>>,
}

impl LocalSyncStore {
    /// Create a new, clean local synchronization store.
    pub fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
        }
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

    /// Retrieve a conversation by ID for an owner, returning None if not found or deleted.
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

    /// Retrieve a conversation including tombstones (used during sync reconciliation).
    pub async fn get_conversation_raw(&self, owner: &str, id: &str) -> Option<SyncConversation> {
        let lock = self.stores.read().await;
        lock.get(owner)?
            .conversations
            .get(id)
            .map(|r| r.conversation.clone())
    }

    /// Upsert a conversation into the local store, incrementing revision and marking dirty.
    pub async fn upsert_conversation(
        &self,
        owner: &str,
        mut conv: SyncConversation,
    ) -> SyncConversation {
        let now = chrono_like_now();
        conv.owner_subject = owner.to_string();
        conv.updated_at = now;

        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();

        let drive_file_id = if let Some(existing) = store.conversations.get(conv.id.as_str()) {
            conv.revision = existing.conversation.revision + 1;
            existing.drive_file_id.clone()
        } else {
            conv.revision = 1;
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

        result
    }

    /// Soft-delete a conversation (tombstone).
    ///
    /// CRITICAL RULE: Deleting a conversation NEVER deletes its associated Library items!
    pub async fn delete_conversation(&self, owner: &str, id: &str) -> bool {
        let now = chrono_like_now();
        let mut lock = self.stores.write().await;
        let Some(store) = lock.get_mut(owner) else {
            return false;
        };

        if let Some(record) = store.conversations.get_mut(id) {
            record.conversation.deleted = true;
            record.conversation.deleted_at = Some(now.clone());
            record.conversation.updated_at = now;
            record.conversation.revision += 1;
            record.is_dirty = true;
            true
        } else {
            false
        }
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

    /// Save or update a library item, caching raw content in memory and marking dirty.
    pub async fn save_library_item(
        &self,
        owner: &str,
        mut item: LibraryItem,
        content: Option<Vec<u8>>,
    ) -> LibraryItem {
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

        result
    }

    /// Explicitly delete a library item (tombstone).
    pub async fn delete_library_item(&self, owner: &str, id: &str) -> bool {
        let now = chrono_like_now();
        let mut lock = self.stores.write().await;
        let Some(store) = lock.get_mut(owner) else {
            return false;
        };

        if let Some(record) = store.library_items.get_mut(id) {
            record.item.deleted = true;
            record.item.deleted_at = Some(now.clone());
            record.item.updated_at = now;
            record.content = None; // Purge raw binary cache
            record.is_dirty = true;
            true
        } else {
            false
        }
    }

    /// Retrieve conversations that need to be uploaded to Google Drive.
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

    /// Retrieve library items that need to be uploaded to Google Drive.
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

    /// Mark a conversation as successfully synchronized.
    pub async fn mark_conversation_synced(&self, owner: &str, id: &str, drive_file_id: String) {
        let mut lock = self.stores.write().await;
        if let Some(record) = lock
            .get_mut(owner)
            .and_then(|s| s.conversations.get_mut(id))
        {
            record.is_dirty = false;
            record.drive_file_id = Some(drive_file_id);
        }
    }

    /// Mark a library item as successfully synchronized.
    pub async fn mark_library_item_synced(&self, owner: &str, id: &str, drive_file_id: String) {
        let mut lock = self.stores.write().await;
        if let Some(record) = lock
            .get_mut(owner)
            .and_then(|s| s.library_items.get_mut(id))
        {
            record.is_dirty = false;
            record.drive_file_id = Some(drive_file_id);
        }
    }

    /// Apply an authoritative remote conversation update during sync restore or download.
    pub async fn apply_remote_conversation(
        &self,
        owner: &str,
        conv: SyncConversation,
        drive_file_id: String,
    ) {
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
    }

    /// Apply an authoritative remote library item metadata update during sync.
    pub async fn apply_remote_library_item(
        &self,
        owner: &str,
        item: LibraryItem,
        drive_file_id: String,
    ) {
        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();
        let existing_content = store
            .library_items
            .get(&item.id)
            .and_then(|r| r.content.clone());
        store.library_items.insert(
            item.id.clone(),
            LocalLibraryRecord {
                item,
                content: existing_content,
                is_dirty: false,
                drive_file_id: Some(drive_file_id),
            },
        );
    }

    /// Record sync status timestamp and error state.
    pub async fn update_sync_status(
        &self,
        owner: &str,
        last_sync: Option<String>,
        last_error: Option<String>,
    ) {
        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();
        if last_sync.is_some() {
            store.last_sync_time = last_sync;
        }
        store.last_error = last_error;
    }

    /// Retrieve sync status numbers for an owner.
    pub async fn get_sync_stats(
        &self,
        owner: &str,
    ) -> (usize, usize, Option<String>, Option<String>) {
        let lock = self.stores.read().await;
        if let Some(store) = lock.get(owner) {
            let pending_conv = store.conversations.values().filter(|r| r.is_dirty).count();
            let pending_lib = store.library_items.values().filter(|r| r.is_dirty).count();
            (
                pending_conv,
                pending_lib,
                store.last_sync_time.clone(),
                store.last_error.clone(),
            )
        } else {
            (0, 0, None, None)
        }
    }

    /// Clear local in-memory session and cache for an owner on logout or account switch.
    ///
    /// DOES NOT delete remote Drive data.
    pub async fn clear_user_session(&self, owner: &str) {
        let mut lock = self.stores.write().await;
        lock.remove(owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_chat_deletion_does_not_delete_library_item() {
        let store = LocalSyncStore::new();
        let owner = "google:alice";

        // 1. Create a conversation
        let conv = SyncConversation::new(
            "conv_1".to_string(),
            owner.to_string(),
            "Chat 1".to_string(),
        );
        store.upsert_conversation(owner, conv).await;

        // 2. Create a library item associated with that conversation
        let lib = LibraryItem::new(
            "lib_1".to_string(),
            owner.to_string(),
            "photo.png".to_string(),
            "image/png".to_string(),
            100,
            "abc123sha256".to_string(),
            Some("conv_1".to_string()),
        );
        store
            .save_library_item(owner, lib, Some(vec![1, 2, 3, 4]))
            .await;

        // Verify both exist
        assert!(store.get_conversation(owner, "conv_1").await.is_some());
        assert!(store.get_library_item(owner, "lib_1").await.is_some());

        // 3. Delete the conversation
        let deleted = store.delete_conversation(owner, "conv_1").await;
        assert!(deleted);
        assert!(store.get_conversation(owner, "conv_1").await.is_none());

        // 4. CRITICAL VERIFICATION: Library item MUST still exist!
        let lib_item = store.get_library_item(owner, "lib_1").await;
        assert!(
            lib_item.is_some(),
            "Library item MUST survive chat deletion!"
        );
        assert_eq!(lib_item.unwrap().filename, "photo.png");
        assert_eq!(
            store.get_library_content(owner, "lib_1").await,
            Some(vec![1, 2, 3, 4])
        );

        // 5. Explicit deletion of Library item works
        let lib_deleted = store.delete_library_item(owner, "lib_1").await;
        assert!(lib_deleted);
        assert!(store.get_library_item(owner, "lib_1").await.is_none());
    }

    #[tokio::test]
    async fn test_owner_isolation() {
        let store = LocalSyncStore::new();
        let owner_a = "google:alice";
        let owner_b = "google:bob";

        let conv_a = SyncConversation::new(
            "conv_a".to_string(),
            owner_a.to_string(),
            "Alice Chat".to_string(),
        );
        store.upsert_conversation(owner_a, conv_a).await;

        // Bob should not see Alice's conversation
        assert!(store.get_conversation(owner_b, "conv_a").await.is_none());
        assert_eq!(store.list_conversations(owner_b).await.len(), 0);
    }
}
