// Domain models for conversations and messages conforming to the backend
// Google Drive sync schema (`SyncConversation` and `SyncMessage`).

class SyncMessage {
  final String id;
  final String conversationId;
  final String role; // "user", "assistant", "system"
  final String content;
  final String timestamp;
  final String? modelId;
  final String? providerId;
  final List<String> attachments;
  final int schemaVersion;

  // Transient UI states (not serialized to backend)
  final bool isStreaming;
  final bool isSaved;
  final String? saveError;
  final bool isRetryable;

  SyncMessage({
    required this.id,
    required this.conversationId,
    required this.role,
    required this.content,
    required this.timestamp,
    this.modelId,
    this.providerId,
    this.attachments = const [],
    this.schemaVersion = 1,
    this.isStreaming = false,
    this.isSaved = true,
    this.saveError,
    this.isRetryable = false,
  });

  bool get isUser => role.toLowerCase() == 'user';
  bool get isAssistant => role.toLowerCase() == 'assistant';
  bool get isSystem => role.toLowerCase() == 'system';

  factory SyncMessage.fromJson(Map<String, dynamic> json) {
    return SyncMessage(
      id: json['id'] as String? ?? '',
      conversationId: json['conversation_id'] as String? ?? '',
      role: json['role'] as String? ?? 'user',
      content: json['content'] as String? ?? '',
      timestamp: json['timestamp'] as String? ?? DateTime.now().toUtc().toIso8601String(),
      modelId: json['model_id'] as String?,
      providerId: json['provider_id'] as String?,
      attachments: (json['attachments'] as List<dynamic>?)
              ?.map((e) => e.toString())
              .toList() ??
          const [],
      schemaVersion: json['schema_version'] as int? ?? 1,
      isSaved: true,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'id': id,
      'conversation_id': conversationId,
      'role': role,
      'content': content,
      'timestamp': timestamp,
      if (modelId != null) 'model_id': modelId,
      if (providerId != null) 'provider_id': providerId,
      'attachments': attachments,
      'schema_version': schemaVersion,
    };
  }

  SyncMessage copyWith({
    String? id,
    String? conversationId,
    String? role,
    String? content,
    String? timestamp,
    String? modelId,
    String? providerId,
    List<String>? attachments,
    int? schemaVersion,
    bool? isStreaming,
    bool? isSaved,
    String? saveError,
    bool? isRetryable,
  }) {
    return SyncMessage(
      id: id ?? this.id,
      conversationId: conversationId ?? this.conversationId,
      role: role ?? this.role,
      content: content ?? this.content,
      timestamp: timestamp ?? this.timestamp,
      modelId: modelId ?? this.modelId,
      providerId: providerId ?? this.providerId,
      attachments: attachments ?? this.attachments,
      schemaVersion: schemaVersion ?? this.schemaVersion,
      isStreaming: isStreaming ?? this.isStreaming,
      isSaved: isSaved ?? this.isSaved,
      saveError: saveError ?? this.saveError,
      isRetryable: isRetryable ?? this.isRetryable,
    );
  }
}

class SyncConversation {
  final String id;
  final String ownerSubject;
  final String title;
  final String createdAt;
  final String updatedAt;
  final int revision;
  final int schemaVersion;
  final bool deleted;
  final String? deletedAt;
  final List<SyncMessage> messages;

  SyncConversation({
    required this.id,
    required this.ownerSubject,
    required this.title,
    required this.createdAt,
    required this.updatedAt,
    this.revision = 1,
    this.schemaVersion = 1,
    this.deleted = false,
    this.deletedAt,
    this.messages = const [],
  });

  factory SyncConversation.newConversation({
    required String id,
    required String ownerSubject,
    String title = 'New Chat',
  }) {
    final now = DateTime.now().toUtc().toIso8601String();
    return SyncConversation(
      id: id,
      ownerSubject: ownerSubject,
      title: title,
      createdAt: now,
      updatedAt: now,
      revision: 1,
      schemaVersion: 1,
      deleted: false,
      messages: const [],
    );
  }

  factory SyncConversation.fromJson(Map<String, dynamic> json) {
    return SyncConversation(
      id: json['id'] as String? ?? '',
      ownerSubject: json['owner_subject'] as String? ?? '',
      title: json['title'] as String? ?? 'Untitled',
      createdAt: json['created_at'] as String? ?? '',
      updatedAt: json['updated_at'] as String? ?? '',
      revision: json['revision'] as int? ?? 1,
      schemaVersion: json['schema_version'] as int? ?? 1,
      deleted: json['deleted'] as bool? ?? false,
      deletedAt: json['deleted_at'] as String?,
      messages: (json['messages'] as List<dynamic>?)
              ?.map((m) => SyncMessage.fromJson(m as Map<String, dynamic>))
              .toList() ??
          const [],
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'id': id,
      'owner_subject': ownerSubject,
      'title': title,
      'created_at': createdAt,
      'updated_at': updatedAt,
      'revision': revision,
      'schema_version': schemaVersion,
      'deleted': deleted,
      if (deletedAt != null) 'deleted_at': deletedAt,
      'messages': messages.map((m) => m.toJson()).toList(),
    };
  }

  SyncConversation copyWith({
    String? id,
    String? ownerSubject,
    String? title,
    String? createdAt,
    String? updatedAt,
    int? revision,
    int? schemaVersion,
    bool? deleted,
    String? deletedAt,
    List<SyncMessage>? messages,
  }) {
    return SyncConversation(
      id: id ?? this.id,
      ownerSubject: ownerSubject ?? this.ownerSubject,
      title: title ?? this.title,
      createdAt: createdAt ?? this.createdAt,
      updatedAt: updatedAt ?? this.updatedAt,
      revision: revision ?? this.revision,
      schemaVersion: schemaVersion ?? this.schemaVersion,
      deleted: deleted ?? this.deleted,
      deletedAt: deletedAt ?? this.deletedAt,
      messages: messages ?? this.messages,
    );
  }
}
