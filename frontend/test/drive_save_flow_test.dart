import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:ai_assistant_frontend/models/auth_user.dart';
import 'package:ai_assistant_frontend/models/conversation.dart';
import 'package:ai_assistant_frontend/services/auth_service.dart';
import 'package:ai_assistant_frontend/services/chat_service.dart';
import 'package:ai_assistant_frontend/services/drive_service.dart';
import 'package:ai_assistant_frontend/state/app_state.dart';

void main() {
  group('Google Drive Pre-Connection & Persistence Flow', () {
    test('streamChat omits conversation_id when user has no drive token', () async {
      Map<String, dynamic>? interceptedBody;
      Map<String, String>? interceptedHeaders;

      final mockClient = MockClient.streaming((request, bodyStream) async {
        interceptedHeaders = request.headers;
        final bodyBytes = await bodyStream.toBytes();
        interceptedBody = jsonDecode(utf8.decode(bodyBytes)) as Map<String, dynamic>;

        final ssePayload = 'event: chunk\ndata: {"delta":"Hello"}\n\nevent: done\ndata: {"status":"completed","saved":false}\n\n';
        return http.StreamedResponse(
          Stream.value(utf8.encode(ssePayload)),
          200,
          headers: {'content-type': 'text/event-stream'},
        );
      });

      final chatService = ChatService(client: mockClient);
      final userWithoutDrive = AuthUser(
        id: 'google:user_123',
        name: 'Test User',
        email: 'test@example.com',
        idToken: 'valid_id_token_123',
        driveAccessToken: null,
      );

      final events = await chatService
          .streamChat(
            user: userWithoutDrive,
            conversationId: 'conv_pre_drive_1',
            messages: [SyncMessage(id: '1', conversationId: 'conv_pre_drive_1', role: 'user', content: 'Hi', timestamp: '')],
            dedicatedClient: mockClient,
          )
          .toList();

      expect(events.length, 2);
      expect(interceptedHeaders?.containsKey('X-Drive-Token'), isFalse);
      expect(interceptedBody?.containsKey('conversation_id'), isFalse);
    });

    test('streamChat includes conversation_id and X-Drive-Token when user has drive token', () async {
      Map<String, dynamic>? interceptedBody;
      Map<String, String>? interceptedHeaders;

      final mockClient = MockClient.streaming((request, bodyStream) async {
        interceptedHeaders = request.headers;
        final bodyBytes = await bodyStream.toBytes();
        interceptedBody = jsonDecode(utf8.decode(bodyBytes)) as Map<String, dynamic>;

        final ssePayload = 'event: chunk\ndata: {"delta":"Response"}\n\nevent: done\ndata: {"status":"completed","saved":true,"conversation_id":"conv_post_drive_1","revision":1}\n\n';
        return http.StreamedResponse(
          Stream.value(utf8.encode(ssePayload)),
          200,
          headers: {'content-type': 'text/event-stream'},
        );
      });

      final chatService = ChatService(client: mockClient);
      final userWithDrive = AuthUser(
        id: 'google:user_123',
        name: 'Test User',
        email: 'test@example.com',
        idToken: 'valid_id_token_123',
        driveAccessToken: 'valid_drive_token_abc',
      );

      final events = await chatService
          .streamChat(
            user: userWithDrive,
            conversationId: 'conv_post_drive_1',
            messages: [SyncMessage(id: '1', conversationId: 'conv_post_drive_1', role: 'user', content: 'Save me', timestamp: '')],
            dedicatedClient: mockClient,
          )
          .toList();

      expect(events.length, 2);
      expect(interceptedHeaders?['x-drive-token'], 'valid_drive_token_abc');
      expect(interceptedBody?['conversation_id'], 'conv_post_drive_1');
    });

    test('authorizeDrive retroactively saves active pre-connection conversation', () async {
      int saveConversationCalls = 0;
      SyncConversation? savedConversationPayload;

      final mockHttpClient = MockClient((request) async {
        if (request.url.path.endsWith('/api/v1/drive/conversations') && request.method == 'POST') {
          saveConversationCalls++;
          final bodyJson = jsonDecode(request.body) as Map<String, dynamic>;
          savedConversationPayload = SyncConversation.fromJson(bodyJson);
          return http.Response(
            jsonEncode({
              'id': bodyJson['id'],
              'owner_subject': 'google:user_123',
              'title': bodyJson['title'] ?? 'Pre-Drive Chat',
              'created_at': DateTime.now().toUtc().toIso8601String(),
              'updated_at': DateTime.now().toUtc().toIso8601String(),
              'revision': 1,
              'schema_version': 1,
              'deleted': false,
              'messages': bodyJson['messages'] ?? [],
            }),
            201,
            headers: {'content-type': 'application/json'},
          );
        } else if (request.url.path.endsWith('/api/v1/drive/status')) {
          return http.Response(
            jsonEncode({
              'enabled': true,
              'last_sync_time': DateTime.now().toUtc().toIso8601String(),
              'pending_conversations_count': 0,
              'pending_library_count': 0,
              'last_error': null,
            }),
            200,
            headers: {'content-type': 'application/json'},
          );
        } else if (request.url.path.endsWith('/api/v1/drive/conversations') && request.method == 'GET') {
          return http.Response('[]', 200, headers: {'content-type': 'application/json'});
        }
        return http.Response('{"error":{"message":"Not found"}}', 404);
      });

      final authService = AuthService();
      await authService.launchDevSession(); // Starts as authenticated mock
      final driveService = DriveService(client: mockHttpClient);
      final chatService = ChatService(client: mockHttpClient);

      final appState = AppState(
        authService: authService,
        chatService: chatService,
        driveService: driveService,
      );

      // Start new chat and add pre-connection messages
      appState.startNewChat();
      appState.messages.add(
        SyncMessage(
          id: 'msg_pre_1',
          conversationId: appState.currentConversation!.id,
          role: 'user',
          content: 'Question sent before Drive was connected',
          timestamp: DateTime.now().toUtc().toIso8601String(),
          isSaved: false,
        ),
      );
      appState.messages.add(
        SyncMessage(
          id: 'msg_pre_2',
          conversationId: appState.currentConversation!.id,
          role: 'assistant',
          content: 'Answer generated before Drive was connected',
          timestamp: DateTime.now().toUtc().toIso8601String(),
          isSaved: false,
        ),
      );

      expect(appState.messages.every((m) => !m.isSaved), isTrue);

      // Connect Google Drive
      final authorized = await appState.authorizeDrive();
      expect(authorized, isTrue);

      // Verify that saveConversation was called retroactively
      expect(saveConversationCalls, 1);
      expect(savedConversationPayload?.messages.length, 2);
      expect(savedConversationPayload?.messages[0].content, 'Question sent before Drive was connected');
      expect(appState.messages.every((m) => m.isSaved), isTrue);
      expect(appState.driveSyncError, isNull);
    });
  });
}
