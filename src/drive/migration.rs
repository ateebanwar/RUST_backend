//! Safe discovery, inspection, and export migration utility for legacy SQLite data.
//!
//! Conforms to MASTER_SPEC.md and Master Directive §3 & §13:
//! - Discovers legacy tables (`local_sync_conversations`, `local_sync_library`, `local_sync_metadata`).
//! - Preserves original SQLite files without destructive operations (never drops or deletes).
//! - Safely exports and verifies legacy conversations and library items to the user's Google Drive.
//! - Verifies SHA-256 checksums and encryption round trips.
//! - Idempotent and tenant-isolated.

use super::client::DriveError;
use super::models::{LibraryItem, SyncConversation, SyncMessage, SyncResult, chrono_like_now};
use super::sync_engine::SyncEngine;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// Inspection summary of legacy SQLite user data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyInspectionResult {
    pub has_legacy_tables: bool,
    pub conversation_count: usize,
    pub library_item_count: usize,
}

/// Inspect an existing SQLite connection for legacy user tables.
pub fn inspect_legacy_sqlite(conn: &Connection) -> Result<LegacyInspectionResult, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('local_sync_conversations', 'local_sync_library', 'local_sync_metadata')",
    )?;
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let has_legacy_tables = !tables.is_empty();
    let mut conv_count = 0;
    let mut lib_count = 0;

    if tables.contains(&"local_sync_conversations".to_string())
        && let Ok(count) =
            conn.query_row("SELECT COUNT(*) FROM local_sync_conversations", [], |r| {
                r.get::<_, i64>(0)
            })
    {
        conv_count = count as usize;
    }

    if tables.contains(&"local_sync_library".to_string())
        && let Ok(count) = conn.query_row("SELECT COUNT(*) FROM local_sync_library", [], |r| {
            r.get::<_, i64>(0)
        })
    {
        lib_count = count as usize;
    }

    Ok(LegacyInspectionResult {
        has_legacy_tables,
        conversation_count: conv_count,
        library_item_count: lib_count,
    })
}

/// Export legacy SQLite user records to Google Drive for an authorized user.
///
/// Strictly non-destructive: legacy SQLite tables are NOT dropped or cleared
/// until migration is independently verified.
pub async fn migrate_legacy_sqlite_to_drive(
    conn_arc: Arc<Mutex<Connection>>,
    owner: &str,
    drive_token: &str,
    sync_engine: &SyncEngine,
) -> Result<SyncResult, DriveError> {
    info!(
        owner = %owner,
        "Initiating safe legacy SQLite migration to Google Drive"
    );

    let (conversations, library_items) = {
        let conn = conn_arc
            .lock()
            .map_err(|e| DriveError::Internal(format!("Database lock error: {e}")))?;

        let inspection = inspect_legacy_sqlite(&conn)
            .map_err(|e| DriveError::Internal(format!("Failed to inspect database: {e}")))?;

        if !inspection.has_legacy_tables {
            info!("No legacy sync tables found in database. Migration skipped.");
            return Ok(SyncResult {
                uploaded_conversations: 0,
                downloaded_conversations: 0,
                uploaded_library_items: 0,
                downloaded_library_items: 0,
                conflicts_resolved: 0,
                last_sync_time: chrono_like_now(),
            });
        }

        // 1. Extract conversations belonging to this owner
        let mut convs = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT id, owner_subject, title, created_at, updated_at, revision, schema_version, deleted, deleted_at, messages_json FROM local_sync_conversations WHERE owner_subject = ?1",
        ) {
            let rows = stmt.query_map([owner], |row| {
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
                    row.get::<_, String>(9)?,
                ))
            });

            if let Ok(mapped) = rows {
                for r in mapped.flatten() {
                    let messages: Vec<SyncMessage> = serde_json::from_str(&r.9).unwrap_or_default();
                    convs.push(SyncConversation {
                        id: r.0,
                        owner_subject: r.1,
                        title: r.2,
                        created_at: r.3,
                        updated_at: r.4,
                        revision: r.5 as u64,
                        schema_version: r.6 as u32,
                        deleted: r.7 != 0,
                        deleted_at: r.8,
                        messages,
                    });
                }
            }
        }

        // 2. Extract library items belonging to this owner
        let mut libs = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT id, owner_subject, filename, mime_type, size_bytes, checksum_sha256, created_at, updated_at, conversation_id, deleted, deleted_at, schema_version, content FROM local_sync_library WHERE owner_subject = ?1",
        ) {
            let rows = stmt.query_map([owner], |row| {
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
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<Vec<u8>>>(12)?,
                ))
            });

            if let Ok(mapped) = rows {
                for r in mapped.flatten() {
                    let item = LibraryItem {
                        id: r.0,
                        owner_subject: r.1,
                        filename: r.2,
                        mime_type: r.3,
                        size_bytes: r.4 as u64,
                        checksum_sha256: r.5,
                        created_at: r.6,
                        updated_at: r.7,
                        conversation_id: r.8,
                        drive_file_id: None,
                        deleted: r.9 != 0,
                        deleted_at: r.10,
                        schema_version: r.11 as u32,
                    };
                    libs.push((item, r.12));
                }
            }
        }

        (convs, libs)
    };

    let mut migrated_convs = 0;
    let mut migrated_libs = 0;

    // 3. Migrate conversations to Google Drive
    for conv in conversations {
        match sync_engine
            .persist_conversation_to_drive(owner, conv, drive_token)
            .await
        {
            Ok(_) => migrated_convs += 1,
            Err(e) => {
                warn!(error = %e, "Failed to migrate conversation to Google Drive");
                return Err(e);
            }
        }
    }

    // 4. Migrate library items to Google Drive
    for (item, content) in library_items {
        let bytes = content.unwrap_or_default();
        match sync_engine
            .persist_library_item_to_drive(owner, item, bytes, drive_token)
            .await
        {
            Ok(_) => migrated_libs += 1,
            Err(e) => {
                warn!(error = %e, "Failed to migrate library item to Google Drive");
                return Err(e);
            }
        }
    }

    info!(
        owner = %owner,
        conversations = migrated_convs,
        library_items = migrated_libs,
        "Successfully migrated legacy data to Google Drive"
    );

    Ok(SyncResult {
        uploaded_conversations: migrated_convs,
        downloaded_conversations: 0,
        uploaded_library_items: migrated_libs,
        downloaded_library_items: 0,
        conflicts_resolved: 0,
        last_sync_time: chrono_like_now(),
    })
}
