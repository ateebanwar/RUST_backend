import 'dart:async';
import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import '../config/app_config.dart';
import '../models/auth_user.dart';
import '../models/chat_event.dart';
import '../models/conversation.dart';
import '../models/model_info.dart';

/// Service for chat completions and SSE streaming conforming to API_SPEC.md.
/// 
/// Real-time streaming is executed via `POST /api/v1/chat/stream` using an
/// incremental SSE line parser that handles token chunks, routing metadata,
/// usage metrics, Google Drive `save_error`, and terminal `done` events.
class ChatService {
  final http.Client _client;
  final String _baseUrl;

  ChatService({http.Client? client, String? baseUrl})
      : _client = client ?? http.Client(),
        _baseUrl = (baseUrl ?? AppConfig.backendBaseUrl).replaceAll(RegExp(r'/+$'), '');

  Map<String, String> _headers(AuthUser user, {bool sse = false}) {
    final map = <String, String>{
      'Content-Type': 'application/json',
      'Accept': sse ? 'text/event-stream' : 'application/json',
      'Authorization': 'Bearer ${user.idToken}',
      'X-Client-Type': AppConfig.clientType,
    };
    if (user.hasDriveToken) {
      map['X-Drive-Token'] = user.driveAccessToken!;
    }
    return map;
  }

  /// GET /api/v1/models ?" Retrieve available model catalog
  Future<List<ModelInfo>> getModels(AuthUser user) async {
    final uri = Uri.parse('$_baseUrl/api/v1/models');
    final response = await _client
        .get(uri, headers: _headers(user))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      final json = jsonDecode(response.body) as Map<String, dynamic>;
      final modelsList = json['models'] as List<dynamic>? ?? [];
      return modelsList
          .map((m) => ModelInfo.fromJson(m as Map<String, dynamic>))
          .where((m) => m.available)
          .toList();
    } else {
      throw Exception('Failed to load models: HTTP ${response.statusCode}');
    }
  }

  /// GET /api/v1/health ?" Check server health
  Future<bool> checkHealth() async {
    try {
      final uri = Uri.parse('$_baseUrl/api/v1/health');
      final response = await _client.get(uri).timeout(const Duration(seconds: 5));
      return response.statusCode == 200;
    } catch (_) {
      return false;
    }
  }

  /// POST /api/v1/chat/stream ?" Real-time SSE streaming chat completion
  Stream<ChatSseEvent> streamChat({
    required AuthUser user,
    required String conversationId,
    required List<SyncMessage> messages,
    String routingMode = 'auto',
    String? requestedModelId,
    double temperature = 0.7,
    http.Client? dedicatedClient,
  }) async* {
    final client = dedicatedClient ?? http.Client();
    final uri = Uri.parse('$_baseUrl/api/v1/chat/stream');

    final payload = {
      'messages': messages
          .map((m) => {
                'role': m.role,
                'content': m.content,
              })
          .toList(),
      'conversation_id': conversationId,
      'routing_mode': routingMode,
      if (requestedModelId != null) 'requested_model_id': requestedModelId,
      'temperature': temperature,
      'stream': true,
    };

    final request = http.Request('POST', uri);
    request.headers.addAll(_headers(user, sse: true));
    request.body = jsonEncode(payload);

    http.StreamedResponse streamedResponse;
    try {
      streamedResponse = await client.send(request).timeout(AppConfig.streamConnectTimeout);
    } catch (e) {
      yield ErrorEvent(
        code: 'NETWORK_FAILURE',
        message: 'Could not connect to AI backend: $e',
        retryable: true,
      );
      return;
    }

    if (streamedResponse.statusCode != 200) {
      final errBody = await streamedResponse.stream.bytesToString();
      String errMsg = 'HTTP ${streamedResponse.statusCode}';
      try {
        final errJson = jsonDecode(errBody) as Map<String, dynamic>;
        if (errJson['error']?['message'] != null) {
          errMsg = errJson['error']['message'].toString();
        }
      } catch (_) {
        if (errBody.isNotEmpty) errMsg = errBody;
      }

      yield ErrorEvent(
        code: 'REQUEST_FAILED',
        message: errMsg,
        retryable: streamedResponse.statusCode >= 500 || streamedResponse.statusCode == 429,
      );
      return;
    }

    String currentEvent = 'chunk';
    final linesStream = streamedResponse.stream
        .transform(utf8.decoder)
        .transform(const LineSplitter());

    await for (final rawLine in linesStream) {
      final line = rawLine.trim();
      if (line.isEmpty) {
        // Empty line resets event name to default
        currentEvent = 'chunk';
        continue;
      }

      // Heartbeat comment frame
      if (line.startsWith(':')) {
        continue;
      }

      if (line.startsWith('event:')) {
        currentEvent = line.substring(6).trim();
        continue;
      }

      if (line.startsWith('data:')) {
        final dataStr = line.substring(5).trim();
        if (dataStr.isEmpty) continue;

        try {
          final dataJson = jsonDecode(dataStr) as Map<String, dynamic>;

          switch (currentEvent) {
            case 'routing':
              yield RoutingEvent.fromJson(dataJson);
              break;
            case 'chunk':
              yield ChunkEvent.fromJson(dataJson);
              break;
            case 'usage':
              yield UsageEvent.fromJson(dataJson);
              break;
            case 'save_error':
              yield SaveErrorEvent.fromJson(dataJson);
              break;
            case 'done':
              yield DoneEvent.fromJson(dataJson);
              break;
            case 'error':
              yield ErrorEvent.fromJson(dataJson);
              break;
            default:
              if (kDebugMode) {
                print('Unhandled SSE event: $currentEvent');
              }
              break;
          }
        } catch (e) {
          if (kDebugMode) {
            print('Error decoding SSE data: $dataStr ($e)');
          }
        }
      }
    }
  }

  /// POST /api/v1/chat/completions ?" Buffered synchronous chat completion
  Future<Map<String, dynamic>> sendBufferedChat({
    required AuthUser user,
    required String conversationId,
    required List<SyncMessage> messages,
    String routingMode = 'auto',
    String? requestedModelId,
    double temperature = 0.7,
  }) async {
    final uri = Uri.parse('$_baseUrl/api/v1/chat/completions');
    final payload = {
      'messages': messages
          .map((m) => {
                'role': m.role,
                'content': m.content,
              })
          .toList(),
      'conversation_id': conversationId,
      'routing_mode': routingMode,
      if (requestedModelId != null) 'requested_model_id': requestedModelId,
      'temperature': temperature,
      'stream': false,
    };

    final response = await _client
        .post(uri, headers: _headers(user), body: jsonEncode(payload))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      return jsonDecode(response.body) as Map<String, dynamic>;
    } else {
      throw Exception('Chat completion failed: HTTP ${response.statusCode} - ${response.body}');
    }
  }
}
