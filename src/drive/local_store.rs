//! Local-first storage and pending synchronization state management.
//!
//! Conforms to MASTER_SPEC.md §15 and Phase 15 requirements:
//! - Local-first: chats and library items are accessible immediately offline without waiting for Drive.
//! - Durable persistence: backed by SQLite with ACID transactions, surviving server restarts.
//! - Strict multi-tenant isolation: indexed by `owner_subject`; User A cannot view User B's content.
//! - Critical lifecycle rule: Soft-deleting a conversation NEVER deletes its associated Library items.
//! - Library items have an independent lifecycle and are only deleted via explicit Library deletion.
//! - Change tracking: maintains dirty flags (`pending_upload`, `pending_delete`) for incremental sync.
//! - Logout lifecycle: clears local user session and stored records without affecting remote Drive objects.

use super::models::{LibraryItem, SyncConversation, SyncMessage, chrono_like_now};
use rusqlite::{Connection, OpenFlags, params};
use std::collections::HashMap;
use std::path::Path;
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
            Self::ConnectionError(msg) => write!(f, "Database connection or I/O failure: {msg}"),
            Self::MigrationError(msg) => write!(f, "Database migration failure: {msg}"),
            Self::SqliteError(msg) => write!(f, "Database query or write failure: {msg}"),
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

/// Thread-safe local-first synchronization store with durable SQLite persistence.
#[derive(Clone)]
pub struct LocalSyncStore {
    stores: Arc<RwLock<HashMap<String, OwnerStore>>>,
    conn: Option<Arc<Mutex<Connection>>>,
}

impl std::fmt::Debug for LocalSyncStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSyncStore")
            .field("is_persistent", &self.is_persistent())
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
            conn: None,
        }
    }

    /// Alias for in-memory store.
    pub fn in_memory() -> Self {
        Self::new()
    }

    /// Initialize a persistent store using an existing SQLite connection handle.
    pub fn from_connection(conn: Arc<Mutex<Connection>>) -> Result<Self, LocalStoreError> {
        Self::run_migrations_on(&conn)?;
        let loaded = Self::load_cache_from(&conn)?;
        Ok(Self {
            stores: Arc::new(RwLock::new(loaded)),
            conn: Some(conn),
        })
    }

    /// Open or create a persistent store at the specified file path or in-memory (":memory:").
    pub fn open(db_path: &str) -> Result<Self, LocalStoreError> {
        let is_memory = db_path == ":memory:" || db_path.contains("mode=memory");

        if !is_memory
            && let Some(parent) = Path::new(db_path).parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                LocalStoreError::ConnectionError(format!(
                    "Failed to create database parent directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }

        let conn = if is_memory {
            Connection::open_in_memory().map_err(|e| {
                LocalStoreError::ConnectionError(format!("Failed to open in-memory SQLite: {e}"))
            })?
        } else {
            Connection::open_with_flags(
                db_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| {
                LocalStoreError::ConnectionError(format!(
                    "Failed to open SQLite database at '{db_path}': {e}"
                ))
            })?
        };

        if !is_memory {
            let _ = conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;",
            );
        }

        Self::from_connection(Arc::new(Mutex::new(conn)))
    }

    /// Check if this store is backed by durable persistent storage.
    pub fn is_persistent(&self) -> bool {
        self.conn.is_some()
    }

    /// Run idempotent schema migrations on the provided database handle.
    fn run_migrations_on(conn_arc: &Arc<Mutex<Connection>>) -> Result<(), LocalStoreError> {
        let conn = conn_arc
            .lock()
            .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS local_sync_conversations (
                id TEXT NOT NULL,
                owner_subject TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                revision INTEGER NOT NULL,
                schema_version INTEGER NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0,
                deleted_at TEXT,
                is_dirty INTEGER NOT NULL DEFAULT 1,
                drive_file_id TEXT,
                messages_json TEXT NOT NULL,
                PRIMARY KEY (owner_subject, id)
            );
            CREATE INDEX IF NOT EXISTS idx_sync_conv_owner ON local_sync_conversations(owner_subject);
            CREATE INDEX IF NOT EXISTS idx_sync_conv_dirty ON local_sync_conversations(owner_subject, is_dirty);

            CREATE TABLE IF NOT EXISTS local_sync_library (
                id TEXT NOT NULL,
                owner_subject TEXT NOT NULL,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                checksum_sha256 TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                conversation_id TEXT,
                drive_file_id TEXT,
                deleted INTEGER NOT NULL DEFAULT 0,
                deleted_at TEXT,
                schema_version INTEGER NOT NULL,
                is_dirty INTEGER NOT NULL DEFAULT 1,
                content BLOB,
                PRIMARY KEY (owner_subject, id)
            );
            CREATE INDEX IF NOT EXISTS idx_sync_lib_owner ON local_sync_library(owner_subject);
            CREATE INDEX IF NOT EXISTS idx_sync_lib_dirty ON local_sync_library(owner_subject, is_dirty);

            CREATE TABLE IF NOT EXISTS local_sync_metadata (
                owner_subject TEXT PRIMARY KEY,
                last_sync_time TEXT,
                last_error TEXT
            );",
        )
        .map_err(|e| LocalStoreError::MigrationError(format!("Failed to run LocalSyncStore migrations: {e}")))?;

        Ok(())
    }

    /// Load all persisted rows into in-memory cache on startup.
    fn load_cache_from(
        conn_arc: &Arc<Mutex<Connection>>,
    ) -> Result<HashMap<String, OwnerStore>, LocalStoreError> {
        let conn = conn_arc
            .lock()
            .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

        let mut stores: HashMap<String, OwnerStore> = HashMap::new();

        // 1. Conversations
        let mut conv_stmt = conn
            .prepare(
                "SELECT id, owner_subject, title, created_at, updated_at, revision, schema_version, deleted, deleted_at, is_dirty, drive_file_id, messages_json FROM local_sync_conversations",
            )
            .map_err(|e| LocalStoreError::SqliteError(e.to_string()))?;

        let conv_rows = conv_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                ))
            })
            .map_err(|e| LocalStoreError::SqliteError(e.to_string()))?;

        for row_res in conv_rows {
            match row_res {
                Ok((
                    id,
                    owner,
                    title,
                    created_at,
                    updated_at,
                    revision,
                    schema_version,
                    deleted,
                    deleted_at,
                    is_dirty,
                    drive_file_id,
                    messages_json,
                )) => {
                    let messages: Vec<SyncMessage> = match serde_json::from_str(&messages_json) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!(
                                owner = %owner,
                                conv_id = %id,
                                error = %e,
                                "Corrupted messages_json in database; defaulting to empty messages"
                            );
                            Vec::new()
                        }
                    };
                    let conv = SyncConversation {
                        id: id.clone(),
                        owner_subject: owner.clone(),
                        title,
                        created_at,
                        updated_at,
                        revision: revision as u64,
                        schema_version: schema_version as u32,
                        deleted: deleted != 0,
                        deleted_at,
                        messages,
                    };
                    let record = LocalConversationRecord {
                        conversation: conv,
                        is_dirty: is_dirty != 0,
                        drive_file_id,
                    };
                    stores
                        .entry(owner)
                        .or_default()
                        .conversations
                        .insert(id, record);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to read conversation row from local_sync_conversations");
                }
            }
        }

        // 2. Library items
        let mut lib_stmt = conn
            .prepare(
                "SELECT id, owner_subject, filename, mime_type, size_bytes, checksum_sha256, created_at, updated_at, conversation_id, drive_file_id, deleted, deleted_at, schema_version, is_dirty, content FROM local_sync_library",
            )
            .map_err(|e| LocalStoreError::SqliteError(e.to_string()))?;

        let lib_rows = lib_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, Option<Vec<u8>>>(14)?,
                ))
            })
            .map_err(|e| LocalStoreError::SqliteError(e.to_string()))?;

        for row_res in lib_rows {
            match row_res {
                Ok((
                    id,
                    owner,
                    filename,
                    mime_type,
                    size_bytes,
                    checksum_sha256,
                    created_at,
                    updated_at,
                    conversation_id,
                    drive_file_id,
                    deleted,
                    deleted_at,
                    schema_version,
                    is_dirty,
                    content,
                )) => {
                    let item = LibraryItem {
                        id: id.clone(),
                        owner_subject: owner.clone(),
                        filename,
                        mime_type,
                        size_bytes: size_bytes as u64,
                        checksum_sha256,
                        created_at,
                        updated_at,
                        conversation_id,
                        drive_file_id: drive_file_id.clone(),
                        deleted: deleted != 0,
                        deleted_at,
                        schema_version: schema_version as u32,
                    };
                    let record = LocalLibraryRecord {
                        item,
                        content,
                        is_dirty: is_dirty != 0,
                        drive_file_id,
                    };
                    stores
                        .entry(owner)
                        .or_default()
                        .library_items
                        .insert(id, record);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to read library item row from local_sync_library");
                }
            }
        }

        // 3. Metadata
        let mut meta_stmt = conn
            .prepare("SELECT owner_subject, last_sync_time, last_error FROM local_sync_metadata")
            .map_err(|e| LocalStoreError::SqliteError(e.to_string()))?;

        let meta_rows = meta_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| LocalStoreError::SqliteError(e.to_string()))?;

        for (owner, last_sync, last_err) in meta_rows.flatten() {
            let store = stores.entry(owner).or_default();
            store.last_sync_time = last_sync;
            store.last_error = last_err;
        }

        Ok(stores)
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

    /// Upsert a conversation into the local store, incrementing revision, persisting, and marking dirty.
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

        let drive_file_id = if let Some(existing) = store.conversations.get(conv.id.as_str()) {
            conv.revision = existing.conversation.revision + 1;
            existing.drive_file_id.clone()
        } else {
            conv.revision = 1;
            None
        };

        let messages_json = serde_json::to_string(&conv.messages)?;

        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "INSERT INTO local_sync_conversations (
                    id, owner_subject, title, created_at, updated_at, revision, schema_version, deleted, deleted_at, is_dirty, drive_file_id, messages_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11)
                ON CONFLICT(owner_subject, id) DO UPDATE SET
                    title = excluded.title,
                    updated_at = excluded.updated_at,
                    revision = excluded.revision,
                    schema_version = excluded.schema_version,
                    deleted = excluded.deleted,
                    deleted_at = excluded.deleted_at,
                    is_dirty = 1,
                    drive_file_id = excluded.drive_file_id,
                    messages_json = excluded.messages_json",
                params![
                    conv.id,
                    owner,
                    conv.title,
                    conv.created_at,
                    conv.updated_at,
                    conv.revision as i64,
                    conv.schema_version as i64,
                    if conv.deleted { 1i64 } else { 0i64 },
                    conv.deleted_at,
                    drive_file_id,
                    messages_json,
                ],
            )?;
        }

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

        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "UPDATE local_sync_conversations
                 SET deleted = 1, deleted_at = ?1, updated_at = ?2, revision = ?3, is_dirty = 1
                 WHERE owner_subject = ?4 AND id = ?5",
                params![now, now, new_revision as i64, owner, id],
            )?;
        }

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

        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "INSERT INTO local_sync_library (
                    id, owner_subject, filename, mime_type, size_bytes, checksum_sha256, created_at, updated_at, conversation_id, drive_file_id, deleted, deleted_at, schema_version, is_dirty, content
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1, ?14)
                ON CONFLICT(owner_subject, id) DO UPDATE SET
                    filename = excluded.filename,
                    mime_type = excluded.mime_type,
                    size_bytes = excluded.size_bytes,
                    checksum_sha256 = excluded.checksum_sha256,
                    updated_at = excluded.updated_at,
                    conversation_id = excluded.conversation_id,
                    drive_file_id = excluded.drive_file_id,
                    deleted = excluded.deleted,
                    deleted_at = excluded.deleted_at,
                    schema_version = excluded.schema_version,
                    is_dirty = 1,
                    content = CASE WHEN excluded.content IS NOT NULL THEN excluded.content ELSE local_sync_library.content END",
                params![
                    item.id,
                    owner,
                    item.filename,
                    item.mime_type,
                    item.size_bytes as i64,
                    item.checksum_sha256,
                    item.created_at,
                    item.updated_at,
                    item.conversation_id,
                    drive_file_id,
                    if item.deleted { 1i64 } else { 0i64 },
                    item.deleted_at,
                    item.schema_version as i64,
                    content,
                ],
            )?;
        }

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

    /// Explicitly delete a library item (tombstone).
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

        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "UPDATE local_sync_library
                 SET deleted = 1, deleted_at = ?1, updated_at = ?2, is_dirty = 1, content = NULL
                 WHERE owner_subject = ?3 AND id = ?4",
                params![now, now, owner, id],
            )?;
        }

        record.item.deleted = true;
        record.item.deleted_at = Some(now.clone());
        record.item.updated_at = now;
        record.content = None; // Purge raw binary cache
        record.is_dirty = true;
        Ok(true)
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
    pub async fn mark_conversation_synced(
        &self,
        owner: &str,
        id: &str,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        if let Some(record) = lock
            .get_mut(owner)
            .and_then(|s| s.conversations.get_mut(id))
        {
            if let Some(conn_arc) = &self.conn {
                let conn = conn_arc
                    .lock()
                    .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

                conn.execute(
                    "UPDATE local_sync_conversations
                     SET is_dirty = 0, drive_file_id = ?1
                     WHERE owner_subject = ?2 AND id = ?3",
                    params![drive_file_id, owner, id],
                )?;
            }
            record.is_dirty = false;
            record.drive_file_id = Some(drive_file_id);
        }
        Ok(())
    }

    /// Mark a library item as successfully synchronized.
    pub async fn mark_library_item_synced(
        &self,
        owner: &str,
        id: &str,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        if let Some(record) = lock
            .get_mut(owner)
            .and_then(|s| s.library_items.get_mut(id))
        {
            if let Some(conn_arc) = &self.conn {
                let conn = conn_arc
                    .lock()
                    .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

                conn.execute(
                    "UPDATE local_sync_library
                     SET is_dirty = 0, drive_file_id = ?1
                     WHERE owner_subject = ?2 AND id = ?3",
                    params![drive_file_id, owner, id],
                )?;
            }
            record.is_dirty = false;
            record.drive_file_id = Some(drive_file_id);
        }
        Ok(())
    }

    /// Apply an authoritative remote conversation update during sync restore or download.
    pub async fn apply_remote_conversation(
        &self,
        owner: &str,
        conv: SyncConversation,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let messages_json = serde_json::to_string(&conv.messages)?;

        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "INSERT INTO local_sync_conversations (
                    id, owner_subject, title, created_at, updated_at, revision, schema_version, deleted, deleted_at, is_dirty, drive_file_id, messages_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11)
                ON CONFLICT(owner_subject, id) DO UPDATE SET
                    title = excluded.title,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    revision = excluded.revision,
                    schema_version = excluded.schema_version,
                    deleted = excluded.deleted,
                    deleted_at = excluded.deleted_at,
                    is_dirty = 0,
                    drive_file_id = excluded.drive_file_id,
                    messages_json = excluded.messages_json",
                params![
                    conv.id,
                    owner,
                    conv.title,
                    conv.created_at,
                    conv.updated_at,
                    conv.revision as i64,
                    conv.schema_version as i64,
                    if conv.deleted { 1i64 } else { 0i64 },
                    conv.deleted_at,
                    drive_file_id,
                    messages_json,
                ],
            )?;
        }

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

    /// Apply an authoritative remote library item metadata update during sync.
    pub async fn apply_remote_library_item(
        &self,
        owner: &str,
        item: LibraryItem,
        drive_file_id: String,
    ) -> Result<(), LocalStoreError> {
        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();
        let existing_content = store
            .library_items
            .get(&item.id)
            .and_then(|r| r.content.clone());

        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "INSERT INTO local_sync_library (
                    id, owner_subject, filename, mime_type, size_bytes, checksum_sha256, created_at, updated_at, conversation_id, drive_file_id, deleted, deleted_at, schema_version, is_dirty, content
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, NULL)
                ON CONFLICT(owner_subject, id) DO UPDATE SET
                    filename = excluded.filename,
                    mime_type = excluded.mime_type,
                    size_bytes = excluded.size_bytes,
                    checksum_sha256 = excluded.checksum_sha256,
                    updated_at = excluded.updated_at,
                    conversation_id = excluded.conversation_id,
                    drive_file_id = excluded.drive_file_id,
                    deleted = excluded.deleted,
                    deleted_at = excluded.deleted_at,
                    schema_version = excluded.schema_version,
                    is_dirty = 0",
                params![
                    item.id,
                    owner,
                    item.filename,
                    item.mime_type,
                    item.size_bytes as i64,
                    item.checksum_sha256,
                    item.created_at,
                    item.updated_at,
                    item.conversation_id,
                    drive_file_id,
                    if item.deleted { 1i64 } else { 0i64 },
                    item.deleted_at,
                    item.schema_version as i64,
                ],
            )?;
        }

        store.library_items.insert(
            item.id.clone(),
            LocalLibraryRecord {
                item,
                content: existing_content,
                is_dirty: false,
                drive_file_id: Some(drive_file_id),
            },
        );
        Ok(())
    }

    /// Record sync status timestamp and error state.
    pub async fn update_sync_status(
        &self,
        owner: &str,
        last_sync: Option<String>,
        last_error: Option<String>,
    ) -> Result<(), LocalStoreError> {
        if let Some(conn_arc) = &self.conn {
            let conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            conn.execute(
                "INSERT INTO local_sync_metadata (owner_subject, last_sync_time, last_error)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(owner_subject) DO UPDATE SET
                    last_sync_time = COALESCE(excluded.last_sync_time, local_sync_metadata.last_sync_time),
                    last_error = excluded.last_error",
                params![owner, last_sync, last_error],
            )?;
        }

        let mut lock = self.stores.write().await;
        let store = lock.entry(owner.to_string()).or_default();
        if last_sync.is_some() {
            store.last_sync_time = last_sync;
        }
        store.last_error = last_error;
        Ok(())
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

    /// Clear local session and cache for an owner on logout or account switch.
    ///
    /// DOES NOT delete remote Drive data.
    pub async fn clear_user_session(&self, owner: &str) -> Result<(), LocalStoreError> {
        if let Some(conn_arc) = &self.conn {
            let mut conn = conn_arc
                .lock()
                .map_err(|e| LocalStoreError::LockError(e.to_string()))?;

            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM local_sync_conversations WHERE owner_subject = ?1",
                params![owner],
            )?;
            tx.execute(
                "DELETE FROM local_sync_library WHERE owner_subject = ?1",
                params![owner],
            )?;
            tx.execute(
                "DELETE FROM local_sync_metadata WHERE owner_subject = ?1",
                params![owner],
            )?;
            tx.commit()?;
        }

        let mut lock = self.stores.write().await;
        lock.remove(owner);
        Ok(())
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
        store.upsert_conversation(owner, conv).await.unwrap();

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
            .await
            .unwrap();

        // Verify both exist
        assert!(store.get_conversation(owner, "conv_1").await.is_some());
        assert!(store.get_library_item(owner, "lib_1").await.is_some());

        // 3. Delete the conversation
        let deleted = store.delete_conversation(owner, "conv_1").await.unwrap();
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
        let lib_deleted = store.delete_library_item(owner, "lib_1").await.unwrap();
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
        store.upsert_conversation(owner_a, conv_a).await.unwrap();

        // Bob should not see Alice's conversation
        assert!(store.get_conversation(owner_b, "conv_a").await.is_none());
        assert_eq!(store.list_conversations(owner_b).await.len(), 0);
    }
}
