//! In-memory thread-safe MockDriveClient for deterministic testing and simulation.
//!
//! Conforms to MASTER_SPEC.md §17 and Phase 15 requirements:
//! - 100% deterministic local execution without external Google credentials.
//! - Accurate simulation of Google Drive `appDataFolder` isolated file storage.
//! - Controllable failure injections: HTTP 401, 403 permission, 403 quota, 429 rate limit
//!   with `Retry-After`, and transient N-count failures that recover on retry.
//! - Inspection helpers for verifying uploaded encrypted files and manifests.

use super::client::{BoxedDriveFuture, DriveClient, DriveError, DriveFileMetadata};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::RwLock;

/// Simulated in-memory file stored in Mock Google Drive.
#[derive(Debug, Clone)]
pub struct MockFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub content: Vec<u8>,
    pub modified_time: String,
    pub spaces: String,
    pub owner_token: String,
}

/// Thread-safe in-memory mock client implementing `DriveClient`.
#[derive(Clone)]
pub struct MockDriveClient {
    files: Arc<RwLock<HashMap<String, MockFile>>>,
    next_id: Arc<AtomicU32>,
    inject_unauthorized: Arc<RwLock<bool>>,
    inject_permission_denied: Arc<RwLock<bool>>,
    inject_quota_exceeded: Arc<RwLock<bool>>,
    inject_rate_limited: Arc<RwLock<Option<u64>>>,
    inject_transient_failures: Arc<AtomicU32>,
}

impl Default for MockDriveClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockDriveClient {
    /// Create a new, clean mock Google Drive client.
    pub fn new() -> Self {
        Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU32::new(1000)),
            inject_unauthorized: Arc::new(RwLock::new(false)),
            inject_permission_denied: Arc::new(RwLock::new(false)),
            inject_quota_exceeded: Arc::new(RwLock::new(false)),
            inject_rate_limited: Arc::new(RwLock::new(None)),
            inject_transient_failures: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Configure simulated 401 Unauthorized / token expired.
    pub async fn set_inject_unauthorized(&self, inject: bool) {
        *self.inject_unauthorized.write().await = inject;
    }

    /// Configure simulated 403 Forbidden / missing Drive permission.
    pub async fn set_inject_permission_denied(&self, inject: bool) {
        *self.inject_permission_denied.write().await = inject;
    }

    /// Configure simulated 403 Forbidden / storage quota exceeded.
    pub async fn set_inject_quota_exceeded(&self, inject: bool) {
        *self.inject_quota_exceeded.write().await = inject;
    }

    /// Configure simulated 429 Too Many Requests with retry-after seconds.
    pub async fn set_inject_rate_limited(&self, retry_after: Option<u64>) {
        *self.inject_rate_limited.write().await = retry_after;
    }

    /// Configure N transient errors that fail with NetworkError before succeeding.
    pub fn set_transient_failures(&self, count: u32) {
        self.inject_transient_failures
            .store(count, Ordering::SeqCst);
    }

    /// Get total number of files currently stored.
    pub async fn file_count(&self) -> usize {
        self.files.read().await.len()
    }

    /// Retrieve raw content of a stored file by name.
    pub async fn get_content_by_name(&self, name: &str) -> Option<Vec<u8>> {
        let lock = self.files.read().await;
        for file in lock.values() {
            if file.name == name {
                return Some(file.content.clone());
            }
        }
        None
    }

    /// Clear all files in mock storage.
    pub async fn clear(&self) {
        self.files.write().await.clear();
    }

    /// Check and apply any configured failure injections.
    async fn check_injections(&self, token: &str) -> Result<(), DriveError> {
        if token.trim().is_empty() {
            return Err(DriveError::MissingDriveToken);
        }

        if *self.inject_unauthorized.read().await {
            return Err(DriveError::InvalidDriveToken(
                "Simulated expired or invalid token".to_string(),
            ));
        }

        if *self.inject_permission_denied.read().await {
            return Err(DriveError::PermissionDenied(
                "Simulated missing drive.appdata scope".to_string(),
            ));
        }

        if *self.inject_quota_exceeded.read().await {
            return Err(DriveError::QuotaExceeded(
                "Simulated Google Drive quota reached".to_string(),
            ));
        }

        if let Some(secs) = *self.inject_rate_limited.read().await {
            return Err(DriveError::RateLimited {
                message: "Simulated rate limit exceeded".to_string(),
                retry_after_secs: Some(secs),
            });
        }

        let transient = self.inject_transient_failures.load(Ordering::SeqCst);
        if transient > 0 {
            self.inject_transient_failures
                .fetch_sub(1, Ordering::SeqCst);
            return Err(DriveError::NetworkError(
                "Simulated transient network connection drop".to_string(),
            ));
        }

        Ok(())
    }
}

impl DriveClient for MockDriveClient {
    fn find_file<'a>(
        &'a self,
        token: &'a str,
        name: &'a str,
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, Option<DriveFileMetadata>> {
        Box::pin(async move {
            self.check_injections(token).await?;

            let lock = self.files.read().await;
            for file in lock.values() {
                if file.owner_token == token
                    && file.name == name
                    && (spaces.is_empty() || file.spaces == spaces)
                {
                    return Ok(Some(DriveFileMetadata {
                        id: file.id.clone(),
                        name: file.name.clone(),
                        mime_type: file.mime_type.clone(),
                        modified_time: Some(file.modified_time.clone()),
                        size: Some(file.content.len() as u64),
                    }));
                }
            }
            Ok(None)
        })
    }

    fn upload_file<'a>(
        &'a self,
        token: &'a str,
        file_id: Option<&'a str>,
        name: &'a str,
        mime_type: &'a str,
        content: &'a [u8],
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, DriveFileMetadata> {
        Box::pin(async move {
            self.check_injections(token).await?;

            let now = crate::drive::models::chrono_like_now();
            let mut lock = self.files.write().await;

            let id = match file_id {
                Some(existing_id) => {
                    if let Some(existing) = lock.get_mut(existing_id) {
                        if existing.owner_token != token {
                            return Err(DriveError::NotFound(format!(
                                "File ID {existing_id} does not exist"
                            )));
                        }
                        existing.content = content.to_vec();
                        existing.modified_time = now.clone();
                        existing.mime_type = mime_type.to_string();
                        existing.id.clone()
                    } else {
                        return Err(DriveError::NotFound(format!(
                            "File ID {existing_id} does not exist"
                        )));
                    }
                }
                None => {
                    let generated_id = format!(
                        "mock_drive_file_{}",
                        self.next_id.fetch_add(1, Ordering::SeqCst)
                    );
                    let file = MockFile {
                        id: generated_id.clone(),
                        name: name.to_string(),
                        mime_type: mime_type.to_string(),
                        content: content.to_vec(),
                        modified_time: now.clone(),
                        spaces: spaces.to_string(),
                        owner_token: token.to_string(),
                    };
                    lock.insert(generated_id.clone(), file);
                    generated_id
                }
            };

            Ok(DriveFileMetadata {
                id,
                name: name.to_string(),
                mime_type: mime_type.to_string(),
                modified_time: Some(now),
                size: Some(content.len() as u64),
            })
        })
    }

    fn download_file<'a>(
        &'a self,
        token: &'a str,
        file_id: &'a str,
    ) -> BoxedDriveFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.check_injections(token).await?;

            let lock = self.files.read().await;
            if let Some(file) = lock.get(file_id) {
                if file.owner_token == token {
                    Ok(file.content.clone())
                } else {
                    Err(DriveError::NotFound(format!("File {file_id} not found")))
                }
            } else {
                Err(DriveError::NotFound(format!("File {file_id} not found")))
            }
        })
    }

    fn delete_file<'a>(&'a self, token: &'a str, file_id: &'a str) -> BoxedDriveFuture<'a, ()> {
        Box::pin(async move {
            self.check_injections(token).await?;

            let mut lock = self.files.write().await;
            if let Some(file) = lock.get(file_id)
                && file.owner_token == token
            {
                lock.remove(file_id);
            }
            Ok(())
        })
    }

    fn list_files<'a>(
        &'a self,
        token: &'a str,
        _query: Option<&'a str>,
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, Vec<DriveFileMetadata>> {
        Box::pin(async move {
            self.check_injections(token).await?;

            let lock = self.files.read().await;
            let mut list = Vec::new();
            for file in lock.values() {
                if file.owner_token == token && (spaces.is_empty() || file.spaces == spaces) {
                    list.push(DriveFileMetadata {
                        id: file.id.clone(),
                        name: file.name.clone(),
                        mime_type: file.mime_type.clone(),
                        modified_time: Some(file.modified_time.clone()),
                        size: Some(file.content.len() as u64),
                    });
                }
            }
            Ok(list)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_upload_find_download_delete() {
        let mock = MockDriveClient::new();
        let token = "valid_test_token";

        // 1. Upload
        let meta = mock
            .upload_file(
                token,
                None,
                "test.txt",
                "text/plain",
                b"hello world",
                "appDataFolder",
            )
            .await
            .expect("Upload failed");
        assert_eq!(meta.name, "test.txt");

        // 2. Find
        let found = mock
            .find_file(token, "test.txt", "appDataFolder")
            .await
            .expect("Find failed")
            .expect("File should exist");
        assert_eq!(found.id, meta.id);

        // 3. Download
        let content = mock
            .download_file(token, &meta.id)
            .await
            .expect("Download failed");
        assert_eq!(content, b"hello world");

        // 4. Delete
        mock.delete_file(token, &meta.id)
            .await
            .expect("Delete failed");
        let deleted = mock
            .find_file(token, "test.txt", "appDataFolder")
            .await
            .unwrap();
        assert!(deleted.is_none());
    }

    #[tokio::test]
    async fn test_mock_failure_injections() {
        let mock = MockDriveClient::new();
        let token = "valid_token";

        // 401 Unauthorized
        mock.set_inject_unauthorized(true).await;
        let err = mock.download_file(token, "id1").await.unwrap_err();
        assert!(matches!(err, DriveError::InvalidDriveToken(_)));
        mock.set_inject_unauthorized(false).await;

        // 403 Permission Denied
        mock.set_inject_permission_denied(true).await;
        let err = mock.download_file(token, "id1").await.unwrap_err();
        assert!(matches!(err, DriveError::PermissionDenied(_)));
        mock.set_inject_permission_denied(false).await;

        // 403 Quota
        mock.set_inject_quota_exceeded(true).await;
        let err = mock.download_file(token, "id1").await.unwrap_err();
        assert!(matches!(err, DriveError::QuotaExceeded(_)));
        mock.set_inject_quota_exceeded(false).await;

        // 429 Rate Limit
        mock.set_inject_rate_limited(Some(5)).await;
        let err = mock.download_file(token, "id1").await.unwrap_err();
        assert!(matches!(
            err,
            DriveError::RateLimited {
                retry_after_secs: Some(5),
                ..
            }
        ));
        mock.set_inject_rate_limited(None).await;
    }
}
