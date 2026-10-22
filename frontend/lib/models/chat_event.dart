// Typed representation of SSE events returned by `POST /api/v1/chat/stream`.

abstract class ChatSseEvent {}

class RoutingEvent extends ChatSseEvent {
  final String modelUsed;
  final String providerUsed;
  final bool isFallback;
  final String? taskCategory;

  RoutingEvent({
    required this.modelUsed,
    required this.providerUsed,
    required this.isFallback,
    this.taskCategory,
  });

  factory RoutingEvent.fromJson(Map<String, dynamic> json) {
    return RoutingEvent(
      modelUsed: json['model_used'] as String? ?? 'unknown',
      providerUsed: json['provider_used'] as String? ?? 'unknown',
      isFallback: json['is_fallback'] as bool? ?? false,
      taskCategory: json['task_category'] as String?,
    );
  }
}

class ChunkEvent extends ChatSseEvent {
  final String delta;

  ChunkEvent({required this.delta});

  factory ChunkEvent.fromJson(Map<String, dynamic> json) {
    return ChunkEvent(
      delta: json['delta'] as String? ?? '',
    );
  }
}

class UsageEvent extends ChatSseEvent {
  final int promptTokens;
  final int completionTokens;
  final int totalTokens;
  final String? source;

  UsageEvent({
    required this.promptTokens,
    required this.completionTokens,
    required this.totalTokens,
    this.source,
  });

  factory UsageEvent.fromJson(Map<String, dynamic> json) {
    return UsageEvent(
      promptTokens: json['prompt_tokens'] as int? ?? 0,
      completionTokens: json['completion_tokens'] as int? ?? 0,
      totalTokens: json['total_tokens'] as int? ?? 0,
      source: json['source'] as String?,
    );
  }
}

class SaveErrorEvent extends ChatSseEvent {
  final String code;
  final String message;
  final String? conversationId;
  final bool retryable;
  final String? errorDetails;

  SaveErrorEvent({
    required this.code,
    required this.message,
    this.conversationId,
    required this.retryable,
    this.errorDetails,
  });

  factory SaveErrorEvent.fromJson(Map<String, dynamic> json) {
    final details = json['details'];
    String? errDetails;
    if (details is Map && details['error'] != null) {
      errDetails = details['error'].toString();
    } else if (details != null) {
      errDetails = details.toString();
    }

    return SaveErrorEvent(
      code: json['code'] as String? ?? 'DRIVE_PERSISTENCE_FAILED',
      message: json['message'] as String? ??
          'Response generated, but saving to Google Drive failed.',
      conversationId: json['conversation_id'] as String?,
      retryable: json['retryable'] as bool? ?? true,
      errorDetails: errDetails,
    );
  }
}

class DoneEvent extends ChatSseEvent {
  final String status;
  final bool saved;
  final String? conversationId;
  final int? revision;
  final String? errorCode;
  final String? errorMessage;

  DoneEvent({
    required this.status,
    required this.saved,
    this.conversationId,
    this.revision,
    this.errorCode,
    this.errorMessage,
  });

  bool get isSaveFailed => status == 'completed_save_failed' || !saved;

  factory DoneEvent.fromJson(Map<String, dynamic> json) {
    final err = json['error'] as Map<String, dynamic>?;
    return DoneEvent(
      status: json['status'] as String? ?? 'completed',
      saved: json['saved'] as bool? ?? false,
      conversationId: json['conversation_id'] as String?,
      revision: json['revision'] as int?,
      errorCode: err?['code'] as String?,
      errorMessage: err?['message'] as String?,
    );
  }
}

class ErrorEvent extends ChatSseEvent {
  final String code;
  final String message;
  final bool retryable;

  ErrorEvent({
    required this.code,
    required this.message,
    required this.retryable,
  });

  factory ErrorEvent.fromJson(Map<String, dynamic> json) {
    return ErrorEvent(
      code: json['code'] as String? ?? 'UPSTREAM_FAILURE',
      message: json['message'] as String? ?? 'An upstream error occurred.',
      retryable: json['retryable'] as bool? ?? false,
    );
  }
}
