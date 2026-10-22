//! Domain models, schemas, and DTOs for Google Drive chat and library synchronization.
//!
//! Conforms to MASTER_SPEC.md §15 & Phase 15 requirements:
//! - Explicit schema versioning (`SCHEMA_VERSION = 1`).
//! - Manifest-driven index to avoid downloading all binaries during restore.
//! - Independent lifecycles for conversations and library items (deleting a conversation
//!   NEVER deletes its associated library items).
//! - Strong typing for stable identifiers, UTC RFC 3339 timestamps, and SHA-256 integrity checksums.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current supported synchronization schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// File name for remote sync manifest stored in private Google Drive `appDataFolder`.
pub const REMOTE_MANIFEST_FILENAME: &str = "sync_manifest.json";

/// Standard private application data space name in Google Drive.
pub const DRIVE_APPDATA_SPACE: &str = "appDataFolder";

/// Remote manifest tracking the indexed state of all conversations and library items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncManifest {
    /// Schema version of this manifest.
    pub schema_version: u32,
    /// Authorized owner subject identifier (e.g. `google:1029384756`).
    pub owner_subject: String,
    /// UTC timestamp of the last successful synchronization.
    pub last_sync_time: Option<String>,
    /// Index of all known conversations by `conversation_id`.
    pub conversations: HashMap<String, ConversationIndexEntry>,
    /// Index of all known library items by `library_item_id`.
    pub library_items: HashMap<String, LibraryIndexEntry>,
}

impl SyncManifest {
    /// Create a new, empty manifest for a specific owner.
    pub fn new(owner_subject: String) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            owner_subject,
            last_sync_time: None,
            conversations: HashMap::new(),
            library_items: HashMap::new(),
        }
    }
}

/// Lightweight index entry for a conversation within the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationIndexEntry {
    /// Unique stable conversation identifier.
    pub id: String,
    /// Title of the conversation.
    pub title: String,
    /// RFC 3339 UTC timestamp of the last update.
    pub updated_at: String,
    /// Monotonically increasing revision counter for LWW conflict resolution.
    pub revision: u64,
    /// Whether this conversation has been soft-deleted (tombstone).
    pub deleted: bool,
    /// RFC 3339 UTC timestamp of deletion if soft-deleted.
    pub deleted_at: Option<String>,
    /// Number of messages in this conversation.
    pub message_count: usize,
    /// Google Drive file ID for the encrypted conversation object, if uploaded.
    pub drive_file_id: Option<String>,
}

/// Lightweight index entry for a library item within the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryIndexEntry {
    /// Unique stable library item identifier.
    pub id: String,
    /// Original filename (sanitized).
    pub filename: String,
    /// MIME type of the file.
    pub mime_type: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Hex-encoded SHA-256 checksum of the raw content.
    pub checksum_sha256: String,
    /// RFC 3339 UTC timestamp of the last update.
    pub updated_at: String,
    /// Whether this library item has been explicitly deleted.
    pub deleted: bool,
    /// RFC 3339 UTC timestamp of deletion if deleted.
    pub deleted_at: Option<String>,
    /// Optional conversation ID where this file was uploaded or generated.
    pub conversation_id: Option<String>,
    /// Google Drive file ID for the encrypted binary object, if uploaded.
    pub drive_file_id: Option<String>,
}

/// Complete conversation payload synchronized with Google Drive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncConversation {
    /// Unique stable conversation identifier.
    pub id: String,
    /// Authorized owner subject identifier.
    pub owner_subject: String,
    /// Title of the conversation.
    pub title: String,
    /// RFC 3339 UTC creation timestamp.
    pub created_at: String,
    /// RFC 3339 UTC update timestamp.
    pub updated_at: String,
    /// Revision counter for conflict resolution.
    pub revision: u64,
    /// Schema version of this conversation.
    pub schema_version: u32,
    /// Whether this conversation is soft-deleted.
    pub deleted: bool,
    /// Timestamp of deletion if soft-deleted.
    pub deleted_at: Option<String>,
    /// Chronologically ordered messages in this conversation.
    pub messages: Vec<SyncMessage>,
}

impl SyncConversation {
    pub fn new(id: String, owner_subject: String, title: String) -> Self {
        let now = chrono_like_now();
        Self {
            id,
            owner_subject,
            title,
            created_at: now.clone(),
            updated_at: now,
            revision: 1,
            schema_version: CURRENT_SCHEMA_VERSION,
            deleted: false,
            deleted_at: None,
            messages: Vec::new(),
        }
    }
}

/// Individual message within a synchronized conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncMessage {
    /// Unique stable message identifier.
    pub id: String,
    /// Parent conversation identifier.
    pub conversation_id: String,
    /// Role: "user", "assistant", "system", etc.
    pub role: String,
    /// Message textual content.
    pub content: String,
    /// RFC 3339 UTC timestamp of creation.
    pub timestamp: String,
    /// Model identifier used for generation if assistant message.
    pub model_id: Option<String>,
    /// Provider identifier used for generation if assistant message.
    pub provider_id: Option<String>,
    /// Stable identifiers of any library items attached to this message.
    pub attachments: Vec<String>,
    /// Schema version of this message.
    pub schema_version: u32,
}

impl SyncMessage {
    pub fn new(id: String, conversation_id: String, role: String, content: String) -> Self {
        Self {
            id,
            conversation_id,
            role,
            content,
            timestamp: chrono_like_now(),
            model_id: None,
            provider_id: None,
            attachments: Vec::new(),
            schema_version: CURRENT_SCHEMA_VERSION,
        }
    }
}

/// Complete metadata for an independent Library item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryItem {
    /// Unique stable library item identifier.
    pub id: String,
    /// Authorized owner subject identifier.
    pub owner_subject: String,
    /// Original filename.
    pub filename: String,
    /// Validated MIME type.
    pub mime_type: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Hex-encoded SHA-256 checksum of raw content.
    pub checksum_sha256: String,
    /// RFC 3339 UTC creation timestamp.
    pub created_at: String,
    /// RFC 3339 UTC update timestamp.
    pub updated_at: String,
    /// Originating conversation identifier, if attached to a chat.
    pub conversation_id: Option<String>,
    /// Google Drive file ID once uploaded.
    pub drive_file_id: Option<String>,
    /// Whether this library item has been explicitly deleted.
    pub deleted: bool,
    /// Timestamp of deletion if deleted.
    pub deleted_at: Option<String>,
    /// Schema version of this library item.
    pub schema_version: u32,
}

impl LibraryItem {
    pub fn new(
        id: String,
        owner_subject: String,
        filename: String,
        mime_type: String,
        size_bytes: u64,
        checksum_sha256: String,
        conversation_id: Option<String>,
    ) -> Self {
        let now = chrono_like_now();
        Self {
            id,
            owner_subject,
            filename: sanitize_filename(&filename),
            mime_type,
            size_bytes,
            checksum_sha256,
            created_at: now.clone(),
            updated_at: now,
            conversation_id,
            drive_file_id: None,
            deleted: false,
            deleted_at: None,
            schema_version: CURRENT_SCHEMA_VERSION,
        }
    }
}

/// Current synchronization status reported to clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncStatus {
    /// Whether Google Drive sync is currently enabled.
    pub enabled: bool,
    /// RFC 3339 UTC timestamp of last successful sync.
    pub last_sync_time: Option<String>,
    /// Number of local conversations awaiting sync upload.
    pub pending_conversations_count: usize,
    /// Number of local library items awaiting sync upload.
    pub pending_library_count: usize,
    /// Error category of the last sync attempt, if failed.
    pub last_error: Option<String>,
}

/// Configuration update payload for Drive sync setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveConfigUpdate {
    /// Desired synchronization enabled state.
    pub enabled: bool,
}

/// Request payload to create a new conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversationRequest {
    /// Optional client-provided ID (otherwise server generates one).
    pub id: Option<String>,
    /// Title of the conversation.
    pub title: String,
    /// Initial messages if any.
    #[serde(default)]
    pub messages: Vec<SyncMessage>,
}

/// Request payload to update an existing conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConversationRequest {
    /// Updated title if changing.
    pub title: Option<String>,
    /// Appended or updated messages.
    #[serde(default)]
    pub messages: Option<Vec<SyncMessage>>,
}

/// Request payload to upload or create a library item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateLibraryItemRequest {
    /// Optional client-provided ID.
    pub id: Option<String>,
    /// Original filename.
    pub filename: String,
    /// MIME type.
    pub mime_type: String,
    /// Optional conversation reference.
    pub conversation_id: Option<String>,
    /// Base64-encoded raw file content.
    pub content_base64: String,
}

/// Result summary returned after a sync cycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncResult {
    pub uploaded_conversations: usize,
    pub downloaded_conversations: usize,
    pub uploaded_library_items: usize,
    pub downloaded_library_items: usize,
    pub conflicts_resolved: usize,
    pub last_sync_time: String,
}

/// Helper function to generate an ISO-8601 / RFC 3339 UTC timestamp string.
pub fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    // Days since unix epoch (1970-01-01)
    let mut days = (secs / 86400) as i64;
    let mut day_secs = (secs % 86400) as u32;

    let mut year = 1970i32;
    loop {
        let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if is_leap { 366 } else { 365 };
        if days >= days_in_year {
            days -= days_in_year;
            year += 1;
        } else {
            break;
        }
    }

    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let month_days = [
        31,
        if is_leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for &d in &month_days {
        if days >= d as i64 {
            days -= d as i64;
            month += 1;
        } else {
            break;
        }
    }
    let day = days + 1;

    let hour = day_secs / 3600;
    day_secs %= 3600;
    let min = day_secs / 60;
    let sec = day_secs % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, min, sec, millis
    )
}

/// Sanitize filename by stripping directory traversals, control characters, and slashes.
pub fn sanitize_filename(name: &str) -> String {
    let basename = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("unnamed_file")
        .trim();

    let filtered: String = basename
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != ':' && *c != '"')
        .collect();

    let cleaned = filtered.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "unnamed_file".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(
            sanitize_filename("C:\\Windows\\system32\\calc.exe"),
            "calc.exe"
        );
        assert_eq!(sanitize_filename("valid_image.png"), "valid_image.png");
        assert_eq!(sanitize_filename("..."), "unnamed_file");
    }

    #[test]
    fn test_chrono_like_now() {
        let ts = chrono_like_now();
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.chars().filter(|&c| c == '-').count(), 2);
        assert_eq!(ts.chars().filter(|&c| c == ':').count(), 2);
    }
}
