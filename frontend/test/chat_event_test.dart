import 'package:flutter_test/flutter_test.dart';
import 'package:ai_assistant_frontend/models/chat_event.dart';

void main() {
  group('ChatSseEvent JSON Parsing and Persistence Status', () {
    test('RoutingEvent parses model and provider', () {
      final json = {
        'model_used': 'fast-chat-v1',
        'provider_used': 'groq-api',
        'is_fallback': false,
        'task_category': 'coding',
      };
      final ev = RoutingEvent.fromJson(json);
      expect(ev.modelUsed, 'fast-chat-v1');
      expect(ev.providerUsed, 'groq-api');
      expect(ev.isFallback, false);
      expect(ev.taskCategory, 'coding');
    });

    test('ChunkEvent parses delta string', () {
      final json = {'delta': 'Hello, world!'};
      final ev = ChunkEvent.fromJson(json);
      expect(ev.delta, 'Hello, world!');
    });

    test('UsageEvent parses token counts', () {
      final json = {
        'prompt_tokens': 12,
        'completion_tokens': 45,
        'total_tokens': 57,
        'source': 'provider_reported',
      };
      final ev = UsageEvent.fromJson(json);
      expect(ev.promptTokens, 12);
      expect(ev.completionTokens, 45);
      expect(ev.totalTokens, 57);
      expect(ev.source, 'provider_reported');
    });

    test('SaveErrorEvent parses Google Drive persistence error', () {
      final json = {
        'code': 'DRIVE_PERSISTENCE_FAILED',
        'message': 'Response generated successfully, but Google Drive persistence failed.',
        'conversation_id': 'conv_123',
        'retryable': true,
        'details': {
          'error': 'Remote manifest conflict or expired drive token',
        },
      };
      final ev = SaveErrorEvent.fromJson(json);
      expect(ev.code, 'DRIVE_PERSISTENCE_FAILED');
      expect(ev.retryable, true);
      expect(ev.conversationId, 'conv_123');
      expect(ev.errorDetails, contains('Remote manifest conflict'));
    });

    test('DoneEvent parses terminal completed with saved: true', () {
      final json = {
        'status': 'completed',
        'saved': true,
        'conversation_id': 'conv_123',
        'revision': 2,
      };
      final ev = DoneEvent.fromJson(json);
      expect(ev.status, 'completed');
      expect(ev.saved, true);
      expect(ev.isSaveFailed, false);
      expect(ev.revision, 2);
    });

    test('DoneEvent parses terminal completed_save_failed with saved: false', () {
      final json = {
        'status': 'completed_save_failed',
        'saved': false,
        'conversation_id': 'conv_123',
        'error': {
          'code': 'DRIVE_PERSISTENCE_FAILED',
          'message': 'Drive token is empty or encryption key not configured',
        },
      };
      final ev = DoneEvent.fromJson(json);
      expect(ev.status, 'completed_save_failed');
      expect(ev.saved, false);
      expect(ev.isSaveFailed, true);
      expect(ev.errorCode, 'DRIVE_PERSISTENCE_FAILED');
      expect(ev.errorMessage, contains('Drive token is empty'));
    });
  });
}
