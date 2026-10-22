// Domain models for Google Drive sync status and Library items.

class SyncStatus {
  final bool syncEnabled;
  final String? lastSyncTime;
  final int pendingConversations;
  final int pendingLibraryItems;
  final String? lastError;

  SyncStatus({
    required this.syncEnabled,
    this.lastSyncTime,
    this.pendingConversations = 0,
    this.pendingLibraryItems = 0,
    this.lastError,
  });

  factory SyncStatus.fromJson(Map<String, dynamic> json) {
    return SyncStatus(
      syncEnabled: json['sync_enabled'] as bool? ?? false,
      lastSyncTime: json['last_sync_time'] as String?,
      pendingConversations: json['pending_conversations'] as int? ?? 0,
      pendingLibraryItems: json['pending_library_items'] as int? ?? 0,
      lastError: json['last_error'] as String?,
    );
  }
}

class LibraryItem {
  final String id;
  final String filename;
  final String mimeType;
  final int sizeBytes;
  final String checksumSha256;
  final String updatedAt;
  final String? conversationId;
  final bool deleted;

  LibraryItem({
    required this.id,
    required this.filename,
    required this.mimeType,
    required this.sizeBytes,
    required this.checksumSha256,
    required this.updatedAt,
    this.conversationId,
    this.deleted = false,
  });

  factory LibraryItem.fromJson(Map<String, dynamic> json) {
    return LibraryItem(
      id: json['id'] as String? ?? '',
      filename: json['filename'] as String? ?? 'file',
      mimeType: json['mime_type'] as String? ?? 'application/octet-stream',
      sizeBytes: json['size_bytes'] as int? ?? 0,
      checksumSha256: json['checksum_sha256'] as String? ?? '',
      updatedAt: json['updated_at'] as String? ?? '',
      conversationId: json['conversation_id'] as String?,
      deleted: json['deleted'] as bool? ?? false,
    );
  }

  String get formattedSize {
    if (sizeBytes < 1024) return '$sizeBytes B';
    if (sizeBytes < 1024 * 1024) return '${(sizeBytes / 1024).toStringAsFixed(1)} KB';
    return '${(sizeBytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
}
