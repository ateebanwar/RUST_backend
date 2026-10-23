import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:ai_assistant_frontend/services/auth_service.dart';
import 'package:ai_assistant_frontend/services/drive_service.dart';

void main() {
  group('Runtime Trace: Silent Drive Authorization & X-Drive-Token Header', () {
    test('Simulated Google Sign-In with silent Drive scope inheritance verifies token presence', () async {
      final authService = AuthService();

      // Launch session simulating Google sign-in where drive scope was already granted
      await authService.signInWithCustomTokens(
        idToken: 'google_id_token_sample_123',
        driveToken: 'ya29.sample_drive_access_token_456',
        name: 'Ateeb',
        email: 'ateeb@example.com',
      );

      expect(authService.isAuthenticated, isTrue);
      expect(authService.currentUser?.hasDriveToken, isTrue);

      Map<String, String>? capturedHeaders;
      final mockClient = MockClient((request) async {
        if (request.url.path.endsWith('/api/v1/drive/conversations')) {
          capturedHeaders = request.headers;
          return http.Response(jsonEncode([]), 200);
        }
        return http.Response('Not Found', 404);
      });

      final driveService = DriveService(client: mockClient);
      await driveService.listConversations(authService.currentUser!);

      // Assert runtime evidence of header
      expect(capturedHeaders, isNotNull);
      expect(capturedHeaders!.containsKey('X-Drive-Token'), isTrue);
      expect(capturedHeaders!['X-Drive-Token'], isNotEmpty);
      expect(capturedHeaders!['X-Drive-Token'], isNot(equals('')));
    });

    test('Google Sign-In WITHOUT Drive scope does NOT send X-Drive-Token', () async {
      final authService = AuthService();

      // Launch session simulating Google sign-in where drive scope has NOT been granted
      await authService.signInWithCustomTokens(
        idToken: 'google_id_token_sample_123',
        driveToken: null,
        name: 'Ateeb',
        email: 'ateeb@example.com',
      );

      expect(authService.isAuthenticated, isTrue);
      expect(authService.currentUser?.hasDriveToken, isFalse);

      Map<String, String>? capturedHeaders;
      final mockClient = MockClient((request) async {
        if (request.url.path.endsWith('/api/v1/drive/conversations')) {
          capturedHeaders = request.headers;
          return http.Response(jsonEncode([]), 200);
        }
        return http.Response('Not Found', 404);
      });

      final driveService = DriveService(client: mockClient);
      await driveService.listConversations(authService.currentUser!);

      // Assert runtime evidence of header absence
      expect(capturedHeaders, isNotNull);
      expect(capturedHeaders!.containsKey('X-Drive-Token'), isFalse);
    });
  });
}
