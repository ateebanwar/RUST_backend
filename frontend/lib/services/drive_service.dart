import 'dart:convert';
import 'package:http/http.dart' as http;
import '../config/app_config.dart';
import '../models/auth_user.dart';
import '../models/conversation.dart';
import '../models/drive_status.dart';

/// Client service communicating with the Rust backend's Google Drive endpoints.
/// 
/// Strictly adheres to backend architecture:
/// - Google Drive is the SOLE permanent storage.
/// - Requires `Authorization: Bearer <idToken>` and `X-Drive-Token: <driveAccessToken>`.
/// - Never writes user data to browser storage or local databases.
class DriveService {
  final http.Client _client;
  final String _baseUrl;

  DriveService({http.Client? client, String? baseUrl})
      : _client = client ?? http.Client(),
        _baseUrl = (baseUrl ?? AppConfig.backendBaseUrl).replaceAll(RegExp(r'/+$'), '');

  Map<String, String> _headers(AuthUser user, {bool includeDriveToken = true}) {
    final map = <String, String>{
      'Content-Type': 'application/json',
      'Accept': 'application/json',
      'Authorization': 'Bearer ${user.idToken}',
      'X-Client-Type': AppConfig.clientType,
    };
    if (includeDriveToken && user.hasDriveToken) {
      map['X-Drive-Token'] = user.driveAccessToken!;
    }
    return map;
  }

  /// GET /api/v1/drive/status
  Future<SyncStatus> getSyncStatus(AuthUser user) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/status');
    final response = await _client
        .get(uri, headers: _headers(user, includeDriveToken: false))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      final json = jsonDecode(response.body) as Map<String, dynamic>;
      return SyncStatus.fromJson(json);
    } else {
      throw Exception('Failed to get sync status: HTTP ${response.statusCode}');
    }
  }

  /// POST /api/v1/drive/sync
  Future<Map<String, dynamic>> triggerSync(AuthUser user) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/sync');
    final response = await _client
        .post(uri, headers: _headers(user, includeDriveToken: true))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      return jsonDecode(response.body) as Map<String, dynamic>;
    } else {
      throw Exception('Sync failed: HTTP ${response.statusCode} - ${response.body}');
    }
  }

  /// GET /api/v1/drive/conversations
  Future<List<SyncConversation>> listConversations(AuthUser user) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/conversations');
    final response = await _client
        .get(uri, headers: _headers(user, includeDriveToken: true))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      final list = jsonDecode(response.body) as List<dynamic>;
      return list
          .map((item) => SyncConversation.fromJson(item as Map<String, dynamic>))
          .where((conv) => !conv.deleted)
          .toList();
    } else {
      throw Exception('Failed to list conversations: HTTP ${response.statusCode}');
    }
  }

  /// GET /api/v1/drive/conversations/{id}
  Future<SyncConversation> getConversation(AuthUser user, String conversationId) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/conversations/$conversationId');
    final response = await _client
        .get(uri, headers: _headers(user, includeDriveToken: true))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      final json = jsonDecode(response.body) as Map<String, dynamic>;
      return SyncConversation.fromJson(json);
    } else {
      throw Exception('Failed to get conversation: HTTP ${response.statusCode}');
    }
  }

  /// POST /api/v1/drive/conversations
  Future<SyncConversation> saveConversation(
    AuthUser user,
    SyncConversation conversation,
  ) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/conversations');
    final body = jsonEncode({
      'id': conversation.id,
      'title': conversation.title,
      'messages': conversation.messages.map((m) => m.toJson()).toList(),
    });

    final response = await _client
        .post(uri, headers: _headers(user, includeDriveToken: true), body: body)
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 201 || response.statusCode == 200) {
      final json = jsonDecode(response.body) as Map<String, dynamic>;
      return SyncConversation.fromJson(json);
    } else {
      throw Exception('Failed to save conversation: HTTP ${response.statusCode} - ${response.body}');
    }
  }

  /// DELETE /api/v1/drive/conversations/{id}
  Future<bool> deleteConversation(AuthUser user, String conversationId) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/conversations/$conversationId');
    final response = await _client
        .delete(uri, headers: _headers(user, includeDriveToken: true))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      return true;
    } else if (response.statusCode == 404) {
      return false;
    } else {
      throw Exception('Failed to delete conversation: HTTP ${response.statusCode}');
    }
  }

  /// GET /api/v1/drive/library
  Future<List<LibraryItem>> listLibrary(AuthUser user) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/library');
    final response = await _client
        .get(uri, headers: _headers(user, includeDriveToken: true))
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 200) {
      final list = jsonDecode(response.body) as List<dynamic>;
      return list
          .map((item) => LibraryItem.fromJson(item as Map<String, dynamic>))
          .where((item) => !item.deleted)
          .toList();
    } else {
      throw Exception('Failed to list library items: HTTP ${response.statusCode}');
    }
  }

  /// POST /api/v1/drive/library
  Future<LibraryItem> uploadLibraryItem(
    AuthUser user, {
    required String filename,
    required String mimeType,
    required String base64Content,
    String? conversationId,
  }) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/library');
    final body = jsonEncode({
      'filename': filename,
      'mime_type': mimeType,
      'content_base64': base64Content,
      if (conversationId != null) 'conversation_id': conversationId,
    });

    final response = await _client
        .post(uri, headers: _headers(user, includeDriveToken: true), body: body)
        .timeout(AppConfig.requestTimeout);

    if (response.statusCode == 201 || response.statusCode == 200) {
      final json = jsonDecode(response.body) as Map<String, dynamic>;
      return LibraryItem.fromJson(json);
    } else {
      throw Exception('Failed to upload library item: HTTP ${response.statusCode} - ${response.body}');
    }
  }

  /// POST /api/v1/drive/logout
  Future<void> logoutSession(AuthUser user) async {
    final uri = Uri.parse('$_baseUrl/api/v1/drive/logout');
    try {
      await _client
          .post(uri, headers: _headers(user, includeDriveToken: false))
          .timeout(const Duration(seconds: 5));
    } catch (_) {
      // Best-effort cleanup
    }
  }
}
