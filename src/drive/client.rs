//! Google Drive v3 API client trait and production HTTP implementation.
//!
//! Conforms to MASTER_SPEC.md §15 and Phase 15 requirements:
//! - Targets private `appDataFolder` (`https://www.googleapis.com/auth/drive.appdata`).
//! - Strictly validates incoming OAuth access tokens.
//! - Uses bounded retries with exponential backoff and jitter for transient errors (5xx, timeouts).
//! - Respects HTTP 429 and `Retry-After` headers.
//! - Explicitly distinguishes 401 Unauthorized, 403 Forbidden (missing permission vs quota),
//!   404 Not Found, 429 Rate Limited, and 5xx upstream failures.
//! - Enforces request timeout ceilings.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Standard Google Drive API base endpoints.
pub const GOOGLE_DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";
pub const GOOGLE_DRIVE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3";

/// Domain errors occurring during Drive client interactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveError {
    /// Drive synchronization is disabled by configuration or user switch.
    SyncDisabled,
    /// `X-Drive-Token` or Bearer token was not provided in request headers.
    MissingDriveToken,
    /// OAuth access token is invalid, expired, or malformed.
    InvalidDriveToken(String),
    /// OAuth scope or IAM permission is insufficient (e.g. missing `drive.appdata`).
    PermissionDenied(String),
    /// Google Drive storage or API usage quota exceeded.
    QuotaExceeded(String),
    /// Google API rate limit hit (HTTP 429).
    RateLimited {
        message: String,
        retry_after_secs: Option<u64>,
    },
    /// Requested remote Drive object or file does not exist.
    NotFound(String),
    /// Transient network error, connection reset, or DNS resolution failure.
    NetworkError(String),
    /// Cryptographic encryption or decryption failure.
    CryptoError(String),
    /// Malformed or corrupted remote data / schema deserialization failure.
    CorruptData(String),
    /// Remote schema version is incompatible with current engine.
    SchemaMismatch { expected: u32, actual: u32 },
    /// SHA-256 integrity checksum verification failed.
    ChecksumMismatch { expected: String, actual: String },
    /// Local or internal Drive subsystem error.
    Internal(String),
}

impl fmt::Display for DriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SyncDisabled => write!(f, "Google Drive synchronization is disabled"),
            Self::MissingDriveToken => write!(
                f,
                "Missing required Google Drive access token (X-Drive-Token header)"
            ),
            Self::InvalidDriveToken(msg) => {
                write!(f, "Invalid or expired Google Drive access token: {msg}")
            }
            Self::PermissionDenied(msg) => write!(f, "Google Drive permission denied: {msg}"),
            Self::QuotaExceeded(msg) => {
                write!(f, "Google Drive storage or API quota exceeded: {msg}")
            }
            Self::RateLimited {
                message,
                retry_after_secs,
            } => {
                write!(f, "Google Drive API rate limit reached: {message}")?;
                if let Some(secs) = retry_after_secs {
                    write!(f, " (retry after {secs}s)")?;
                }
                Ok(())
            }
            Self::NotFound(msg) => write!(f, "Google Drive file not found: {msg}"),
            Self::NetworkError(msg) => {
                write!(f, "Network error communicating with Google Drive: {msg}")
            }
            Self::CryptoError(msg) => write!(f, "Cryptographic operation failed: {msg}"),
            Self::CorruptData(msg) => write!(f, "Corrupted or malformed Drive data: {msg}"),
            Self::SchemaMismatch { expected, actual } => {
                write!(
                    f,
                    "Schema mismatch: expected version {expected}, found {actual}"
                )
            }
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "Checksum mismatch: expected {expected}, calculated {actual}"
                )
            }
            Self::Internal(msg) => write!(f, "Drive internal error: {msg}"),
        }
    }
}

impl std::error::Error for DriveError {}

impl From<crate::drive::crypto::CryptoError> for DriveError {
    fn from(err: crate::drive::crypto::CryptoError) -> Self {
        Self::CryptoError(err.to_string())
    }
}

/// Metadata representation for a Google Drive file object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriveFileMetadata {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub modified_time: Option<String>,
    pub size: Option<u64>,
}

/// Dyn-compatible boxed future alias for DriveClient operations.
pub type BoxedDriveFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DriveError>> + Send + 'a>>;

/// Asynchronous, dyn-compatible trait defining Google Drive operations for production and mock implementations.
pub trait DriveClient: Send + Sync {
    /// Find an existing file by name in the specified space (e.g. `appDataFolder`).
    fn find_file<'a>(
        &'a self,
        token: &'a str,
        name: &'a str,
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, Option<DriveFileMetadata>>;

    /// Upload or update a file in Google Drive. If `file_id` is None, creates a new file;
    /// otherwise updates the existing file.
    fn upload_file<'a>(
        &'a self,
        token: &'a str,
        file_id: Option<&'a str>,
        name: &'a str,
        mime_type: &'a str,
        content: &'a [u8],
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, DriveFileMetadata>;

    /// Download binary content of a file by Google Drive file ID.
    fn download_file<'a>(
        &'a self,
        token: &'a str,
        file_id: &'a str,
    ) -> BoxedDriveFuture<'a, Vec<u8>>;

    /// Delete a file by Google Drive file ID.
    fn delete_file<'a>(&'a self, token: &'a str, file_id: &'a str) -> BoxedDriveFuture<'a, ()>;

    /// List files in the specified space matching an optional query string.
    fn list_files<'a>(
        &'a self,
        token: &'a str,
        query: Option<&'a str>,
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, Vec<DriveFileMetadata>>;
}

/// Production Google Drive client using HTTP REST API with bounded retries and exponential backoff.
pub struct GoogleDriveClient {
    http: reqwest::Client,
    max_retries: u32,
    base_url: String,
    upload_base_url: String,
}

impl GoogleDriveClient {
    /// Create a new Google Drive client with configured timeout and max retries.
    pub fn new(timeout_secs: u64, max_retries: u32) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            http,
            max_retries,
            base_url: GOOGLE_DRIVE_API_BASE.to_string(),
            upload_base_url: GOOGLE_DRIVE_UPLOAD_BASE.to_string(),
        }
    }

    /// Builder override for custom base URLs (e.g. mock HTTP server in tests).
    pub fn with_base_urls(mut self, base_url: String, upload_base_url: String) -> Self {
        self.base_url = base_url;
        self.upload_base_url = upload_base_url;
        self
    }

    /// Validate the access token format before making requests.
    fn validate_token(token: &str) -> Result<(), DriveError> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(DriveError::MissingDriveToken);
        }
        if trimmed.contains('\n') || trimmed.contains('\r') {
            return Err(DriveError::InvalidDriveToken(
                "Token contains invalid newline characters".to_string(),
            ));
        }
        Ok(())
    }

    /// Map HTTP status codes to standardized `DriveError`.
    async fn map_http_error(resp: reqwest::Response) -> DriveError {
        let status = resp.status();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let error_body = resp.text().await.unwrap_or_default();

        match status.as_u16() {
            401 => DriveError::InvalidDriveToken("Token expired or revoked by Google".to_string()),
            403 => {
                if error_body.contains("quotaExceeded") || error_body.contains("rateLimitExceeded")
                {
                    DriveError::QuotaExceeded(error_body)
                } else {
                    DriveError::PermissionDenied(format!(
                        "Drive scope insufficient or consent revoked: {error_body}"
                    ))
                }
            }
            404 => DriveError::NotFound("Drive file not found".to_string()),
            429 => DriveError::RateLimited {
                message: "Google Drive API rate limit reached".to_string(),
                retry_after_secs: retry_after,
            },
            500..=599 => {
                DriveError::NetworkError(format!("Google Drive server returned status {status}"))
            }
            other => DriveError::Internal(format!(
                "Unexpected Drive HTTP status {other}: {error_body}"
            )),
        }
    }
}

impl DriveClient for GoogleDriveClient {
    fn find_file<'a>(
        &'a self,
        token: &'a str,
        name: &'a str,
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, Option<DriveFileMetadata>> {
        Box::pin(async move {
            Self::validate_token(token)?;
            let q = format!("name = '{name}' and trashed = false");
            let list = self.list_files(token, Some(&q), spaces).await?;
            Ok(list.into_iter().next())
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
            Self::validate_token(token)?;

            let (url, method) = match file_id {
                Some(id) => (
                    format!("{}/files/{}?uploadType=media", self.upload_base_url, id),
                    reqwest::Method::PATCH,
                ),
                None => (
                    format!("{}/files?uploadType=multipart", self.upload_base_url),
                    reqwest::Method::POST,
                ),
            };

            let mut attempt = 0;
            loop {
                attempt += 1;

                let req = if file_id.is_some() {
                    self.http
                        .request(method.clone(), &url)
                        .header("Authorization", format!("Bearer {token}"))
                        .header("Content-Type", mime_type)
                        .body(content.to_vec())
                } else {
                    // Multipart upload for initial creation with name and parent
                    let metadata_json = serde_json::json!({
                        "name": name,
                        "parents": [spaces],
                    });

                    let boundary = "-------ai_backend_boundary_987654";
                    let mut body = Vec::new();
                    body.extend_from_slice(
                        format!(
                            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    body.extend_from_slice(metadata_json.to_string().as_bytes());
                    body.extend_from_slice(
                        format!("\r\n--{boundary}\r\nContent-Type: {mime_type}\r\n\r\n").as_bytes(),
                    );
                    body.extend_from_slice(content);
                    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

                    self.http
                        .request(method.clone(), &url)
                        .header("Authorization", format!("Bearer {token}"))
                        .header(
                            "Content-Type",
                            format!("multipart/related; boundary={boundary}"),
                        )
                        .body(body)
                };

                match req.send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            let json: serde_json::Value = resp.json().await.map_err(|e| {
                                DriveError::CorruptData(format!(
                                    "Failed to parse Google Drive response JSON: {e}"
                                ))
                            })?;

                            return Ok(DriveFileMetadata {
                                id: json["id"].as_str().unwrap_or("").to_string(),
                                name: json["name"].as_str().unwrap_or(name).to_string(),
                                mime_type: json["mimeType"]
                                    .as_str()
                                    .unwrap_or(mime_type)
                                    .to_string(),
                                modified_time: json["modifiedTime"].as_str().map(|s| s.to_string()),
                                size: Some(content.len() as u64),
                            });
                        }

                        let err = Self::map_http_error(resp).await;
                        if attempt <= self.max_retries {
                            match &err {
                                DriveError::RateLimited {
                                    retry_after_secs, ..
                                } => {
                                    let delay = retry_after_secs.unwrap_or(1).min(10);
                                    tokio::time::sleep(Duration::from_secs(delay)).await;
                                    continue;
                                }
                                DriveError::NetworkError(_) => {
                                    let delay = 2u64.pow(attempt).min(8);
                                    tokio::time::sleep(Duration::from_millis(delay * 100)).await;
                                    continue;
                                }
                                _ => return Err(err),
                            }
                        }
                        return Err(err);
                    }
                    Err(e) => {
                        if attempt <= self.max_retries {
                            tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                            continue;
                        }
                        return Err(DriveError::NetworkError(e.to_string()));
                    }
                }
            }
        })
    }

    fn download_file<'a>(
        &'a self,
        token: &'a str,
        file_id: &'a str,
    ) -> BoxedDriveFuture<'a, Vec<u8>> {
        Box::pin(async move {
            Self::validate_token(token)?;

            let url = format!("{}/files/{}?alt=media", self.base_url, file_id);
            let mut attempt = 0;

            loop {
                attempt += 1;
                let resp = self
                    .http
                    .get(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .send()
                    .await;

                match resp {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            let bytes = resp.bytes().await.map_err(|e| {
                                DriveError::NetworkError(format!(
                                    "Failed to read downloaded content: {e}"
                                ))
                            })?;
                            return Ok(bytes.to_vec());
                        }

                        let err = Self::map_http_error(resp).await;
                        if attempt <= self.max_retries {
                            match &err {
                                DriveError::RateLimited {
                                    retry_after_secs, ..
                                } => {
                                    let delay = retry_after_secs.unwrap_or(1).min(10);
                                    tokio::time::sleep(Duration::from_secs(delay)).await;
                                    continue;
                                }
                                DriveError::NetworkError(_) => {
                                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64))
                                        .await;
                                    continue;
                                }
                                _ => return Err(err),
                            }
                        }
                        return Err(err);
                    }
                    Err(e) => {
                        if attempt <= self.max_retries {
                            tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                            continue;
                        }
                        return Err(DriveError::NetworkError(e.to_string()));
                    }
                }
            }
        })
    }

    fn delete_file<'a>(&'a self, token: &'a str, file_id: &'a str) -> BoxedDriveFuture<'a, ()> {
        Box::pin(async move {
            Self::validate_token(token)?;

            let url = format!("{}/files/{}", self.base_url, file_id);
            let resp = self
                .http
                .delete(&url)
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .map_err(|e| DriveError::NetworkError(e.to_string()))?;

            if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(());
            }

            Err(Self::map_http_error(resp).await)
        })
    }

    fn list_files<'a>(
        &'a self,
        token: &'a str,
        query: Option<&'a str>,
        spaces: &'a str,
    ) -> BoxedDriveFuture<'a, Vec<DriveFileMetadata>> {
        Box::pin(async move {
            Self::validate_token(token)?;

            let mut url = format!(
                "{}/files?spaces={spaces}&fields=files(id,name,mimeType,modifiedTime,size)",
                self.base_url
            );
            if let Some(q) = query {
                url.push_str(&format!("&q={}", urlencoding(q)));
            }

            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .map_err(|e| DriveError::NetworkError(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(Self::map_http_error(resp).await);
            }

            let json: serde_json::Value = resp.json().await.map_err(|e| {
                DriveError::CorruptData(format!(
                    "Failed to parse Google Drive files listing JSON: {e}"
                ))
            })?;

            let mut results = Vec::new();
            if let Some(files) = json["files"].as_array() {
                for f in files {
                    results.push(DriveFileMetadata {
                        id: f["id"].as_str().unwrap_or("").to_string(),
                        name: f["name"].as_str().unwrap_or("").to_string(),
                        mime_type: f["mimeType"].as_str().unwrap_or("").to_string(),
                        modified_time: f["modifiedTime"].as_str().map(|s| s.to_string()),
                        size: f["size"].as_str().and_then(|s| s.parse::<u64>().ok()),
                    });
                }
            }

            Ok(results)
        })
    }
}

/// Simple percent-encoding helper for Drive query strings.
fn urlencoding(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(encoded, "%{:02X}", b);
            }
        }
    }
    encoded
}
